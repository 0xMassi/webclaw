//! YouTube video structured extractor.
//!
//! YouTube embeds the full player configuration in a
//! `ytInitialPlayerResponse` JavaScript assignment at the top of
//! every `/watch`, `/shorts`, and `youtu.be` HTML page. We reuse the
//! core crate's already-proven regex + parse to surface typed JSON
//! from it: video id, title, author + channel id, view count,
//! duration, upload date, keywords, thumbnails, caption-track URLs.
//!
//! Auto-dispatched: YouTube host is unique and the `v=` or `/shorts/`
//! shape is stable.
//!
//! ## Fallback
//!
//! `ytInitialPlayerResponse` is missing on EU-consent interstitials,
//! some live-stream pre-show pages, and age-gated videos. In those
//! cases we drop down to OG tags for `title`, `description`,
//! `thumbnail`, and `channel`, and return a `data_source:
//! "og_fallback"` payload so the caller can tell they got a degraded
//! shape (no view count, duration, captions).

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::client::FetchClient;
use crate::error::FetchError;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "youtube_video",
    label: "YouTube video",
    description: "Returns video id, title, channel, view count, duration, upload date, thumbnails, keywords, and caption-track URLs. Falls back to OG metadata on consent / age-gate pages.",
    url_patterns: &[
        "https://www.youtube.com/watch?v={id}",
        "https://youtu.be/{id}",
        "https://www.youtube.com/shorts/{id}",
    ],
};

pub fn matches(url: &str) -> bool {
    webclaw_core::youtube::is_youtube_url(url)
        || url.contains("youtube.com/shorts/")
        || url.contains("youtube-nocookie.com/embed/")
}

pub async fn extract(client: &FetchClient, url: &str) -> Result<Value, FetchError> {
    let video_id = parse_video_id(url).ok_or_else(|| {
        FetchError::Build(format!("youtube_video: cannot parse video id from '{url}'"))
    })?;

    // Always fetch the canonical /watch URL. /shorts/ and youtu.be
    // sometimes serve a thinner page without the player blob.
    let canonical = format!("https://www.youtube.com/watch?v={video_id}");
    let resp = client.fetch(&canonical).await?;
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "youtube returned status {} for {canonical}",
            resp.status
        )));
    }

    if let Some(player) = extract_player_response(&resp.html) {
        return Ok(build_player_payload(
            &player, &resp.html, url, &canonical, &video_id,
        ));
    }

    // No player blob. Fall back to OG tags so the call still returns
    // something useful for consent / age-gate pages.
    Ok(build_og_fallback(&resp.html, url, &canonical, &video_id))
}

// ---------------------------------------------------------------------------
// Player-blob path (rich payload)
// ---------------------------------------------------------------------------

fn build_player_payload(
    player: &Value,
    html: &str,
    url: &str,
    canonical: &str,
    video_id: &str,
) -> Value {
    let video_details = player.get("videoDetails");
    let microformat = player
        .get("microformat")
        .and_then(|m| m.get("playerMicroformatRenderer"));

    let thumbnails: Vec<Value> = video_details
        .and_then(|vd| vd.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();

    let keywords: Vec<Value> = video_details
        .and_then(|vd| vd.get("keywords"))
        .and_then(|k| k.as_array())
        .cloned()
        .unwrap_or_default();

    let caption_tracks = webclaw_core::youtube::extract_caption_tracks(html);
    let captions: Vec<Value> = caption_tracks
        .iter()
        .map(|c| {
            json!({
                "url":  c.url,
                "lang": c.lang,
                "name": c.name,
            })
        })
        .collect();

    json!({
        "url":          url,
        "canonical_url":canonical,
        "data_source":  "player_response",
        "video_id":     video_id,
        "title":        get_str(video_details, "title"),
        "description":  get_str(video_details, "shortDescription"),
        "author":       get_str(video_details, "author"),
        "channel_id":   get_str(video_details, "channelId"),
        "channel_url":  get_str(microformat, "ownerProfileUrl"),
        "view_count":   get_int(video_details, "viewCount"),
        "length_seconds": get_int(video_details, "lengthSeconds"),
        "is_live":      video_details.and_then(|vd| vd.get("isLiveContent")).and_then(|v| v.as_bool()),
        "is_private":   video_details.and_then(|vd| vd.get("isPrivate")).and_then(|v| v.as_bool()),
        "is_unlisted":  microformat.and_then(|m| m.get("isUnlisted")).and_then(|v| v.as_bool()),
        "allow_ratings":video_details.and_then(|vd| vd.get("allowRatings")).and_then(|v| v.as_bool()),
        "category":     get_str(microformat, "category"),
        "upload_date":  get_str(microformat, "uploadDate"),
        "publish_date": get_str(microformat, "publishDate"),
        "keywords":     keywords,
        "thumbnails":   thumbnails,
        "caption_tracks": captions,
    })
}

// ---------------------------------------------------------------------------
// OG fallback path (degraded payload)
// ---------------------------------------------------------------------------

fn build_og_fallback(html: &str, url: &str, canonical: &str, video_id: &str) -> Value {
    let title = og(html, "title");
    let description = og(html, "description");
    let thumbnail = og(html, "image");
    // YouTube sets `<meta name="channel_name" ...>` on some pages but
    // OG-only pages reliably carry `og:video:tag` and the channel in
    // `<link itemprop="name">`. We keep this lean: just what's stable.
    let channel = meta_name(html, "author");

    json!({
        "url":          url,
        "canonical_url":canonical,
        "data_source":  "og_fallback",
        "video_id":     video_id,
        "title":        title,
        "description":  description,
        "author":       channel,
        // OG path: these are null so the caller doesn't have to guess.
        "channel_id":   None::<String>,
        "channel_url":  None::<String>,
        "view_count":   None::<i64>,
        "length_seconds": None::<i64>,
        "is_live":      None::<bool>,
        "is_private":   None::<bool>,
        "is_unlisted":  None::<bool>,
        "allow_ratings":None::<bool>,
        "category":     None::<String>,
        "upload_date":  None::<String>,
        "publish_date": None::<String>,
        "keywords":     Vec::<Value>::new(),
        "thumbnails":   thumbnail.as_ref().map(|t| vec![json!({"url": t})]).unwrap_or_default(),
        "caption_tracks": Vec::<Value>::new(),
    })
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

fn parse_video_id(url: &str) -> Option<String> {
    // youtu.be/{id}
    if let Some(after) = url.split("youtu.be/").nth(1) {
        let id = after
            .split(['?', '#', '/'])
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    // youtube.com/shorts/{id}
    if let Some(after) = url.split("youtube.com/shorts/").nth(1) {
        let id = after
            .split(['?', '#', '/'])
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    // youtube-nocookie.com/embed/{id}
    if let Some(after) = url.split("/embed/").nth(1) {
        let id = after
            .split(['?', '#', '/'])
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    // youtube.com/watch?v={id} (also matches youtube.com/watch?foo=bar&v={id})
    if let Some(q) = url.split_once('?').map(|(_, q)| q)
        && let Some(id) = q
            .split('&')
            .find_map(|p| p.strip_prefix("v=").map(|v| v.to_string()))
    {
        let id = id.split(['#', '/']).next().unwrap_or(&id).to_string();
        if !id.is_empty() {
            return Some(id);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Player-response parsing
// ---------------------------------------------------------------------------

fn extract_player_response(html: &str) -> Option<Value> {
    // Same regex as webclaw_core::youtube. Duplicated here because
    // core's regex is module-private. Kept in lockstep; changes are
    // rare and we cover with tests in both places.
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE
        .get_or_init(|| Regex::new(r"var\s+ytInitialPlayerResponse\s*=\s*(\{.+?\})\s*;").unwrap());
    let json_str = re.captures(html)?.get(1)?.as_str();
    serde_json::from_str(json_str).ok()
}

// ---------------------------------------------------------------------------
// Meta-tag helpers (for OG fallback)
// ---------------------------------------------------------------------------

fn og(html: &str, prop: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+property="og:([a-z_]+)"[^>]+content="([^"]+)""#).unwrap()
    });
    for c in re.captures_iter(html) {
        if c.get(1).is_some_and(|m| m.as_str() == prop) {
            return c.get(2).map(|m| m.as_str().to_string());
        }
    }
    None
}

fn meta_name(html: &str, name: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+name="([^"]+)"[^>]+content="([^"]+)""#).unwrap()
    });
    for c in re.captures_iter(html) {
        if c.get(1).is_some_and(|m| m.as_str() == name) {
            return c.get(2).map(|m| m.as_str().to_string());
        }
    }
    None
}

fn get_str(v: Option<&Value>, key: &str) -> Option<String> {
    v.and_then(|x| x.get(key))
        .and_then(|x| x.as_str().map(String::from))
}

fn get_int(v: Option<&Value>, key: &str) -> Option<i64> {
    v.and_then(|x| x.get(key)).and_then(|x| {
        x.as_i64()
            .or_else(|| x.as_str().and_then(|s| s.parse::<i64>().ok()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_watch_urls() {
        assert!(matches("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(matches("https://youtu.be/dQw4w9WgXcQ"));
        assert!(matches("https://www.youtube.com/shorts/abc123"));
        assert!(matches(
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ"
        ));
    }

    #[test]
    fn rejects_non_video_urls() {
        assert!(!matches("https://www.youtube.com/"));
        assert!(!matches("https://www.youtube.com/channel/abc"));
        assert!(!matches("https://example.com/watch?v=abc"));
    }

    #[test]
    fn parse_video_id_from_each_shape() {
        assert_eq!(
            parse_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".into())
        );
        assert_eq!(
            parse_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=10s"),
            Some("dQw4w9WgXcQ".into())
        );
        assert_eq!(
            parse_video_id("https://www.youtube.com/watch?feature=share&v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".into())
        );
        assert_eq!(
            parse_video_id("https://youtu.be/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".into())
        );
        assert_eq!(
            parse_video_id("https://youtu.be/dQw4w9WgXcQ?t=30"),
            Some("dQw4w9WgXcQ".into())
        );
        assert_eq!(
            parse_video_id("https://www.youtube.com/shorts/abc123"),
            Some("abc123".into())
        );
    }

    #[test]
    fn extract_player_response_happy_path() {
        let html = r#"
<html><body>
<script>
var ytInitialPlayerResponse = {"videoDetails":{"videoId":"abc","title":"T","author":"A","viewCount":"100","lengthSeconds":"60","shortDescription":"d"}};
</script>
</body></html>
"#;
        let v = extract_player_response(html).unwrap();
        let vd = v.get("videoDetails").unwrap();
        assert_eq!(vd.get("title").unwrap().as_str(), Some("T"));
    }

    #[test]
    fn og_fallback_extracts_basics_from_meta_tags() {
        let html = r##"
<html><head>
<meta property="og:title" content="Example Video Title">
<meta property="og:description" content="A cool video description.">
<meta property="og:image" content="https://i.ytimg.com/vi/abc/maxresdefault.jpg">
<meta name="author" content="Example Channel">
</head></html>"##;
        let v = build_og_fallback(
            html,
            "https://www.youtube.com/watch?v=abc",
            "https://www.youtube.com/watch?v=abc",
            "abc",
        );
        assert_eq!(v["data_source"], "og_fallback");
        assert_eq!(v["title"], "Example Video Title");
        assert_eq!(v["description"], "A cool video description.");
        assert_eq!(v["author"], "Example Channel");
        assert_eq!(
            v["thumbnails"][0]["url"],
            "https://i.ytimg.com/vi/abc/maxresdefault.jpg"
        );
        assert!(v["view_count"].is_null());
        assert!(v["caption_tracks"].as_array().unwrap().is_empty());
    }
}
