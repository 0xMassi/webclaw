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

use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::client::FetchClient;
use crate::error::FetchError;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "youtube_video",
    label: "YouTube video",
    description: "Returns video id, title, channel, view count, duration, upload date, thumbnails, keywords, and caption-track URLs.",
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

    let player = extract_player_response(&resp.html).ok_or_else(|| {
        FetchError::BodyDecode(format!(
            "youtube_video: no ytInitialPlayerResponse on {canonical} (video may be private, region-blocked, or removed)"
        ))
    })?;

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

    let caption_tracks = webclaw_core::youtube::extract_caption_tracks(&resp.html);
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

    Ok(json!({
        "url":          url,
        "canonical_url":canonical,
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
    }))
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
    use regex::Regex;
    use std::sync::OnceLock;
    // Same regex as webclaw_core::youtube. Duplicated here because
    // core's regex is module-private. Kept in lockstep; changes are
    // rare and we cover with tests in both places.
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE
        .get_or_init(|| Regex::new(r"var\s+ytInitialPlayerResponse\s*=\s*(\{.+?\})\s*;").unwrap());
    let json_str = re.captures(html)?.get(1)?.as_str();
    serde_json::from_str(json_str).ok()
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
}
