//! YouTube video structured extractor.
//!
//! YouTube embeds the full player configuration in a
//! `ytInitialPlayerResponse` JavaScript assignment at the top of
//! every `/watch`, `/shorts`, and `youtu.be` HTML page. We reuse the
//! core crate's already-proven regex + parse to surface typed JSON
//! from it: video id, title, author + channel id, view count,
//! duration, upload date, keywords, thumbnails, caption-track URLs.
//!
//! The caption URLs in the web page carry `exp=xpe` and answer with an
//! empty body unless the request proves it ran YouTube's web player. The
//! Android app's player API (a POST) returns caption URLs without that
//! requirement, so the extractor asks it for the tracks and returns the
//! transcript of the first manual track (else the first track) as
//! `transcript`. When that API is unavailable, `transcript` is null and
//! `caption_tracks` keeps the page's URLs.
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
use super::og::parse_og;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "youtube_video",
    label: "YouTube video",
    description: "Returns video id, title, channel, view count, duration, upload date, thumbnails, keywords, caption-track URLs, and the transcript. Falls back to OG metadata on consent / age-gate pages.",
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

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let video_id = parse_video_id(url).ok_or_else(|| {
        FetchError::Build(format!("youtube_video: cannot parse video id from '{url}'"))
    })?;

    let canonical = format!("https://www.youtube.com/watch?v={video_id}");
    let resp = client.fetch(&canonical).await?;
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "youtube returned status {} for {canonical}",
            resp.status
        )));
    }

    let mut payload = match extract_player_response(&resp.html) {
        Some(player) => build_player_payload(&player, &resp.html, url, &canonical, &video_id),
        None => build_og_fallback(&resp.html, url, &canonical, &video_id),
    };
    add_transcript(client, &video_id, &mut payload).await;
    Ok(payload)
}

const PLAYER_API: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";
const APP_VERSION: &str = "20.10.38";

async fn add_transcript(client: &dyn Fetcher, video_id: &str, payload: &mut Value) {
    let Some(tracks) = app_player(client, video_id)
        .await
        .map(|player| app_caption_tracks(&player))
        .filter(|tracks| !tracks.is_empty())
    else {
        return;
    };
    let track = tracks
        .iter()
        .find(|t| !t["url"].as_str().unwrap_or("").contains("kind=asr"))
        .unwrap_or(&tracks[0]);
    payload["transcript"] = fetch_transcript(client, track).await.unwrap_or(Value::Null);
    payload["caption_tracks"] = Value::Array(tracks);
}

async fn app_player(client: &dyn Fetcher, video_id: &str) -> Option<Value> {
    let body = json!({
        "context": {"client": {
            "clientName": "ANDROID",
            "clientVersion": APP_VERSION,
            "androidSdkVersion": 30,
            "hl": "en",
        }},
        "videoId": video_id,
    })
    .to_string();
    let agent = format!("com.google.android.youtube/{APP_VERSION} (Linux; U; Android 11) gzip");
    let resp = client
        .post_json(PLAYER_API, &body, &[("user-agent", &agent)])
        .await
        .ok()?;
    if resp.status != 200 {
        return None;
    }
    serde_json::from_str(&resp.html).ok()
}

fn app_caption_tracks(player: &Value) -> Vec<Value> {
    player
        .pointer("/captions/playerCaptionsTracklistRenderer/captionTracks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let name = t
                .pointer("/name/simpleText")
                .or_else(|| t.pointer("/name/runs/0/text"));
            Some(json!({
                "url":  t.get("baseUrl")?.as_str()?,
                "lang": t.get("languageCode"),
                "name": name,
            }))
        })
        .collect()
}

async fn fetch_transcript(client: &dyn Fetcher, track: &Value) -> Option<Value> {
    let url = json3_url(track["url"].as_str()?)?;
    let resp = client.fetch(&url).await.ok()?;
    if resp.status != 200 {
        return None;
    }
    let segments = transcript_segments(&resp.html)?;
    let text = segments
        .iter()
        .filter_map(|s| s["text"].as_str())
        .collect::<Vec<_>>()
        .join(" ");
    Some(json!({
        "lang":     track["lang"],
        "segments": segments,
        "text":     text,
    }))
}

fn json3_url(base: &str) -> Option<String> {
    let (path, query) = base.split_once('?')?;
    let mut params: Vec<&str> = query
        .split('&')
        .filter(|p| !p.starts_with("fmt="))
        .collect();
    params.push("fmt=json3");
    Some(format!("{path}?{}", params.join("&")))
}

fn transcript_segments(json3: &str) -> Option<Vec<Value>> {
    let doc: Value = serde_json::from_str(json3).ok()?;
    let seconds = |e: &Value, key: &str| e.get(key).and_then(Value::as_f64).unwrap_or(0.0) / 1000.0;
    let segments = doc
        .get("events")?
        .as_array()?
        .iter()
        .filter_map(|e| {
            let raw: String = e
                .get("segs")?
                .as_array()?
                .iter()
                .filter_map(|s| s.get("utf8")?.as_str())
                .collect();
            let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            (!text.is_empty()).then(|| {
                json!({
                    "start":    seconds(e, "tStartMs"),
                    "duration": seconds(e, "dDurationMs"),
                    "text":     text,
                })
            })
        })
        .collect();
    Some(segments)
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
        "transcript":   Value::Null,
    })
}

// ---------------------------------------------------------------------------
// OG fallback path (degraded payload)
// ---------------------------------------------------------------------------

fn build_og_fallback(html: &str, url: &str, canonical: &str, video_id: &str) -> Value {
    let og_meta = parse_og(html);
    let title = og_meta.raw("title");
    let description = og_meta.raw("description");
    let thumbnail = og_meta.raw("image");
    let channel = meta_name(html, "author");

    json!({
        "url":          url,
        "canonical_url":canonical,
        "data_source":  "og_fallback",
        "video_id":     video_id,
        "title":        title,
        "description":  description,
        "author":       channel,
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
        "transcript":   Value::Null,
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

    fn recorded(name: &str) -> String {
        std::fs::read_to_string(format!("testdata/youtube/{name}")).unwrap()
    }

    #[test]
    fn caption_tracks_come_from_the_app_player_response() {
        let player: Value =
            serde_json::from_str(&recorded("player-android-jNQXAC9IVRw.json")).unwrap();
        let tracks = app_caption_tracks(&player);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0]["lang"], "en");
        assert_eq!(tracks[0]["name"], "English");
        assert_eq!(tracks[1]["lang"], "de");
        let url = tracks[0]["url"].as_str().unwrap();
        assert!(url.starts_with("https://www.youtube.com/api/timedtext?v=jNQXAC9IVRw"));
        assert!(!url.contains("exp=xpe"));
    }

    #[test]
    fn transcript_url_asks_for_json3() {
        let url =
            json3_url("https://www.youtube.com/api/timedtext?v=abc&signature=x.y&lang=en&fmt=srv3")
                .unwrap();
        assert_eq!(
            url,
            "https://www.youtube.com/api/timedtext?v=abc&signature=x.y&lang=en&fmt=json3"
        );
        assert_eq!(
            json3_url("https://www.youtube.com/api/timedtext?v=abc&lang=en").unwrap(),
            "https://www.youtube.com/api/timedtext?v=abc&lang=en&fmt=json3"
        );
    }

    #[test]
    fn json3_becomes_transcript_segments() {
        let segments = transcript_segments(&recorded("timedtext-jNQXAC9IVRw-en.json3")).unwrap();
        assert_eq!(segments.len(), 6);
        assert_eq!(segments[0]["start"], 1.2);
        assert_eq!(segments[0]["duration"], 2.16);
        assert_eq!(
            segments[0]["text"],
            "All right, so here we are, in front of the elephants"
        );
        assert_eq!(
            segments[5]["text"],
            "and that's pretty much all there is to say"
        );
        assert!(transcript_segments("").is_none());
    }

    const WATCH_PAGE: &str = r#"<html><body><script>
var ytInitialPlayerResponse = {"videoDetails":{"videoId":"jNQXAC9IVRw","title":"Me at the zoo","author":"jawed"},"captions":{"playerCaptionsTracklistRenderer":{"captionTracks":[{"baseUrl":"https://www.youtube.com/api/timedtext?v=jNQXAC9IVRw&exp=xpe&lang=en","name":{"simpleText":"English"},"languageCode":"en"}]}}};
</script></body></html>"#;

    struct RecordedYoutube {
        app_player: bool,
    }

    #[async_trait::async_trait]
    impl Fetcher for RecordedYoutube {
        async fn fetch(&self, url: &str) -> Result<crate::client::FetchResult, FetchError> {
            let html = if url.contains("/api/timedtext") {
                assert!(
                    url.contains("fmt=json3") && !url.contains("exp=xpe"),
                    "{url}"
                );
                recorded("timedtext-jNQXAC9IVRw-en.json3")
            } else {
                WATCH_PAGE.to_string()
            };
            Ok(page(url, html))
        }

        async fn post_json(
            &self,
            url: &str,
            body: &str,
            _headers: &[(&str, &str)],
        ) -> Result<crate::client::FetchResult, FetchError> {
            assert!(url.starts_with("https://www.youtube.com/youtubei/v1/player"));
            assert!(body.contains("\"videoId\":\"jNQXAC9IVRw\""));
            if !self.app_player {
                return Err(FetchError::Build("no POST".into()));
            }
            Ok(page(url, recorded("player-android-jNQXAC9IVRw.json")))
        }
    }

    fn page(url: &str, html: String) -> crate::client::FetchResult {
        crate::client::FetchResult {
            html,
            document_bytes: None,
            status: 200,
            url: url.to_string(),
            headers: http::HeaderMap::new(),
            elapsed: std::time::Duration::ZERO,
        }
    }

    #[tokio::test]
    async fn extract_returns_the_transcript_text() {
        let v = extract(
            &RecordedYoutube { app_player: true },
            "https://www.youtube.com/watch?v=jNQXAC9IVRw",
        )
        .await
        .unwrap();
        assert_eq!(v["title"], "Me at the zoo");
        assert_eq!(v["transcript"]["lang"], "en");
        assert_eq!(v["transcript"]["segments"].as_array().unwrap().len(), 6);
        assert!(
            v["transcript"]["text"]
                .as_str()
                .unwrap()
                .starts_with("All right, so here we are, in front of the elephants the cool thing")
        );
        assert_eq!(v["caption_tracks"].as_array().unwrap().len(), 2);
        assert!(
            !v["caption_tracks"][0]["url"]
                .as_str()
                .unwrap()
                .contains("exp=xpe")
        );
    }

    #[tokio::test]
    async fn extract_without_the_app_player_keeps_page_tracks() {
        let v = extract(
            &RecordedYoutube { app_player: false },
            "https://www.youtube.com/watch?v=jNQXAC9IVRw",
        )
        .await
        .unwrap();
        assert!(v["transcript"].is_null());
        assert_eq!(v["caption_tracks"].as_array().unwrap().len(), 1);
        assert!(
            v["caption_tracks"][0]["url"]
                .as_str()
                .unwrap()
                .contains("exp=xpe")
        );
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
