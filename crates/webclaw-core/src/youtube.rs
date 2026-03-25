use once_cell::sync::Lazy;
/// YouTube video metadata extraction from `ytInitialPlayerResponse` embedded JSON.
///
/// YouTube embeds the full player config (title, author, view count, description,
/// duration, upload date) in a `<script>` tag as a JS variable assignment. This
/// module parses that blob and formats it as structured markdown, giving LLMs a
/// clean representation without needing the YouTube API.
use regex::Regex;
use tracing::debug;

/// Regex to find the ytInitialPlayerResponse assignment in a <script> block.
/// YouTube uses: `var ytInitialPlayerResponse = {...};`
static YT_PLAYER_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"var\s+ytInitialPlayerResponse\s*=\s*(\{.+?\})\s*;").unwrap());

/// Check if a URL is a YouTube video page.
pub fn is_youtube_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("youtube.com/watch") || lower.contains("youtu.be/")
}

/// Extracted YouTube video metadata.
#[derive(Debug)]
struct VideoMeta {
    title: String,
    author: String,
    view_count: String,
    upload_date: String,
    description: String,
    duration: String,
}

/// Try to extract YouTube video metadata from the page HTML.
/// Returns structured markdown if successful, None if the page doesn't contain
/// ytInitialPlayerResponse or parsing fails.
pub fn try_extract(html: &str) -> Option<String> {
    let json_str = YT_PLAYER_RE.captures(html)?.get(1)?.as_str();

    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let video_details = value.get("videoDetails")?;
    let microformat = value
        .get("microformat")
        .and_then(|m| m.get("playerMicroformatRenderer"));

    let title = video_details
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled")
        .to_string();

    let author = video_details
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let view_count = video_details
        .get("viewCount")
        .and_then(|v| v.as_str())
        .map(format_view_count)
        .unwrap_or_else(|| "N/A".to_string());

    let upload_date = microformat
        .and_then(|m| m.get("uploadDate"))
        .or_else(|| microformat.and_then(|m| m.get("publishDate")))
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let description = video_details
        .get("shortDescription")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let duration_secs = video_details
        .get("lengthSeconds")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let duration = format_duration(duration_secs);

    let meta = VideoMeta {
        title,
        author,
        view_count,
        upload_date,
        description,
        duration,
    };

    debug!(
        title = %meta.title,
        author = %meta.author,
        "extracted YouTube video metadata"
    );

    Some(format_markdown(&meta))
}

/// Format seconds into human-readable duration (e.g., "1:23:45" or "12:34").
fn format_duration(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Format a raw view count string with commas (e.g., "1234567" -> "1,234,567").
fn format_view_count(raw: &str) -> String {
    let Ok(n) = raw.parse::<u64>() else {
        return raw.to_string();
    };

    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// A caption track URL extracted from ytInitialPlayerResponse.
#[derive(Debug, Clone)]
pub struct CaptionTrack {
    pub url: String,
    pub lang: String,
    pub name: String,
}

/// Extract caption track URLs from ytInitialPlayerResponse JSON.
/// Returns empty vec if no captions are available.
pub fn extract_caption_tracks(html: &str) -> Vec<CaptionTrack> {
    let Some(json_str) = YT_PLAYER_RE.captures(html).and_then(|c| c.get(1)) else {
        return vec![];
    };

    let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str.as_str()) else {
        return vec![];
    };

    let Some(tracks) = value
        .get("captions")
        .and_then(|c| c.get("playerCaptionsTracklistRenderer"))
        .and_then(|r| r.get("captionTracks"))
        .and_then(|t| t.as_array())
    else {
        return vec![];
    };

    tracks
        .iter()
        .filter_map(|t| {
            let url = t.get("baseUrl")?.as_str()?.to_string();
            let lang = t
                .get("languageCode")
                .and_then(|v| v.as_str())
                .unwrap_or("en")
                .to_string();
            let name = t
                .get("name")
                .and_then(|v| v.get("simpleText"))
                .and_then(|v| v.as_str())
                .unwrap_or(&lang)
                .to_string();
            Some(CaptionTrack { url, lang, name })
        })
        .collect()
}

/// Parse YouTube timed text XML into plain transcript text.
/// The XML format is: `<transcript><text start="0" dur="1.5">Hello</text>...</transcript>`
pub fn parse_timed_text(xml: &str) -> String {
    // Simple regex-based parsing to avoid adding an XML crate dependency.
    // Extract text content between <text ...>...</text> tags.
    static TEXT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<text[^>]*>([^<]*)</text>").unwrap());

    let mut lines: Vec<String> = Vec::new();
    for cap in TEXT_RE.captures_iter(xml) {
        let text = cap[1].trim();
        if text.is_empty() {
            continue;
        }
        // Decode XML entities
        let decoded = text
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&apos;", "'")
            .replace("\n", " ");
        lines.push(decoded);
    }

    lines.join(" ")
}

/// Format extracted metadata into structured markdown.
fn format_markdown(meta: &VideoMeta) -> String {
    let mut md = format!("# {}\n\n", meta.title);

    md.push_str(&format!(
        "**Channel:** {} | **Views:** {} | **Published:** {} | **Duration:** {}\n\n",
        meta.author, meta.view_count, meta.upload_date, meta.duration
    ));

    if !meta.description.is_empty() {
        md.push_str("## Description\n\n");
        md.push_str(&meta.description);
        md.push('\n');
    }

    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_youtube_urls() {
        assert!(is_youtube_url(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        ));
        assert!(is_youtube_url("https://youtube.com/watch?v=abc123"));
        assert!(is_youtube_url("https://youtu.be/dQw4w9WgXcQ"));
        assert!(!is_youtube_url("https://example.com"));
        assert!(!is_youtube_url("https://vimeo.com/123456"));
    }

    #[test]
    fn format_duration_short() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(65), "1:05");
        assert_eq!(format_duration(3661), "1:01:01");
        assert_eq!(format_duration(754), "12:34");
    }

    #[test]
    fn format_view_count_values() {
        assert_eq!(format_view_count("500"), "500");
        assert_eq!(format_view_count("1500"), "1.5K");
        assert_eq!(format_view_count("1234567"), "1.2M");
    }

    #[test]
    fn extracts_from_mock_html() {
        let html = r#"
        <html><head><title>Test Video</title></head>
        <body>
        <script>
        var ytInitialPlayerResponse = {"videoDetails":{"title":"Rust in 100 Seconds","author":"Fireship","viewCount":"5432100","shortDescription":"Learn Rust in 100 seconds.","lengthSeconds":"120"},"microformat":{"playerMicroformatRenderer":{"uploadDate":"2023-01-15"}}};
        </script>
        </body></html>
        "#;

        let result = try_extract(html).unwrap();
        assert!(result.contains("# Rust in 100 Seconds"));
        assert!(result.contains("**Channel:** Fireship"));
        assert!(result.contains("5.4M"));
        assert!(result.contains("2023-01-15"));
        assert!(result.contains("2:00"));
        assert!(result.contains("Learn Rust in 100 seconds."));
    }

    #[test]
    fn returns_none_for_non_youtube_html() {
        let html = "<html><body><p>Hello world</p></body></html>";
        assert!(try_extract(html).is_none());
    }

    #[test]
    fn handles_missing_optional_fields() {
        let html = r#"
        <html><body>
        <script>
        var ytInitialPlayerResponse = {"videoDetails":{"title":"Minimal Video","author":"Someone","viewCount":"100","shortDescription":"","lengthSeconds":"60"}};
        </script>
        </body></html>
        "#;

        let result = try_extract(html).unwrap();
        assert!(result.contains("# Minimal Video"));
        assert!(result.contains("**Channel:** Someone"));
        // Upload date should be "Unknown" when microformat is missing
        assert!(result.contains("Unknown"));
    }
}
