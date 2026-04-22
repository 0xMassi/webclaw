//! Instagram profile structured extractor.
//!
//! Hits Instagram's internal `web_profile_info` endpoint at
//! `instagram.com/api/v1/users/web_profile_info/?username=X`. The
//! `x-ig-app-id` header is Instagram's own public web-app id (not a
//! secret) — the same value Instagram's own JavaScript bundle sends.
//!
//! Returns the full profile (bio, exact follower count, verified /
//! business flags, profile picture) plus the **12 most recent posts**
//! with shortcodes, like counts, types, thumbnails, and caption
//! previews. Callers can fan out to `/v1/scrape/instagram_post` per
//! shortcode to get the full caption + media.
//!
//! Pagination beyond 12 requires authenticated cookies + a CSRF token;
//! we accept that as the practical ceiling for the unauth path. The
//! cloud (with stored sessions) can paginate later as a follow-up.
//!
//! Falls back to OG-tag scraping of the public profile page if the API
//! returns 401/403 — Instagram has tightened this endpoint multiple
//! times, so we keep the second path warm.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "instagram_profile",
    label: "Instagram profile",
    description: "Returns full profile metadata + the 12 most recent posts (shortcode, url, type, likes, thumbnail).",
    url_patterns: &["https://www.instagram.com/{username}/"],
};

/// Instagram's own public web-app identifier. Sent by their JS bundle
/// on every API call, accepted by the unauth endpoint, not a secret.
const IG_APP_ID: &str = "936619743392459";

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if !matches!(host, "www.instagram.com" | "instagram.com") {
        return false;
    }
    let path = url
        .split("://")
        .nth(1)
        .and_then(|s| s.split_once('/'))
        .map(|(_, p)| p)
        .unwrap_or("");
    let stripped = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    let segs: Vec<&str> = stripped.split('/').filter(|s| !s.is_empty()).collect();
    segs.len() == 1 && !RESERVED.contains(&segs[0])
}

const RESERVED: &[&str] = &[
    "p",
    "reel",
    "reels",
    "tv",
    "explore",
    "stories",
    "directory",
    "accounts",
    "about",
    "developer",
    "press",
    "api",
    "ads",
    "blog",
    "fragments",
    "terms",
    "privacy",
    "session",
    "login",
    "signup",
];

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let username = parse_username(url).ok_or_else(|| {
        FetchError::Build(format!(
            "instagram_profile: cannot parse username from '{url}'"
        ))
    })?;

    let api_url =
        format!("https://www.instagram.com/api/v1/users/web_profile_info/?username={username}");
    let extra_headers: &[(&str, &str)] = &[
        ("x-ig-app-id", IG_APP_ID),
        ("accept", "*/*"),
        ("sec-fetch-site", "same-origin"),
        ("x-requested-with", "XMLHttpRequest"),
    ];
    let resp = client.fetch_with_headers(&api_url, extra_headers).await?;

    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "instagram_profile: '{username}' not found"
        )));
    }
    // Auth wall fallback: Instagram occasionally tightens this endpoint
    // and starts returning 401/403/302 to a login page. When that
    // happens we still want to give the caller something useful — the
    // OG tags from the public HTML page (no posts list, but bio etc).
    if !(200..300).contains(&resp.status) {
        return og_fallback(client, &username, url, resp.status).await;
    }

    let body: ApiResponse = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("instagram_profile parse: {e}")))?;
    let user = body.data.user;

    let recent_posts: Vec<Value> = user
        .edge_owner_to_timeline_media
        .as_ref()
        .map(|m| m.edges.iter().map(|e| post_summary(&e.node)).collect())
        .unwrap_or_default();

    Ok(json!({
        "url":               url,
        "canonical_url":     format!("https://www.instagram.com/{username}/"),
        "username":          user.username.unwrap_or(username),
        "data_completeness": "api",
        "user_id":           user.id,
        "full_name":         user.full_name,
        "biography":         user.biography,
        "biography_links":   user.bio_links,
        "external_url":      user.external_url,
        "category":          user.category_name,
        "follower_count":    user.edge_followed_by.map(|c| c.count),
        "following_count":   user.edge_follow.map(|c| c.count),
        "post_count":        user.edge_owner_to_timeline_media.as_ref().map(|m| m.count),
        "is_verified":       user.is_verified,
        "is_private":        user.is_private,
        "is_business":       user.is_business_account,
        "is_professional":   user.is_professional_account,
        "profile_pic_url":   user.profile_pic_url_hd.or(user.profile_pic_url),
        "recent_posts":      recent_posts,
    }))
}

/// Build the per-post summary the caller fans out from. Includes a
/// constructed `url` so the loop is `for p in recent_posts: scrape('instagram_post', p.url)`.
fn post_summary(n: &MediaNode) -> Value {
    let kind = classify(n);
    let url = match kind {
        "reel" => format!(
            "https://www.instagram.com/reel/{}/",
            n.shortcode.as_deref().unwrap_or("")
        ),
        _ => format!(
            "https://www.instagram.com/p/{}/",
            n.shortcode.as_deref().unwrap_or("")
        ),
    };
    let caption = n
        .edge_media_to_caption
        .as_ref()
        .and_then(|c| c.edges.first())
        .and_then(|e| e.node.text.clone());
    json!({
        "shortcode":     n.shortcode,
        "url":           url,
        "kind":          kind,
        "is_video":      n.is_video.unwrap_or(false),
        "video_views":   n.video_view_count,
        "thumbnail_url": n.thumbnail_src.clone().or_else(|| n.display_url.clone()),
        "display_url":   n.display_url,
        "like_count":    n.edge_media_preview_like.as_ref().map(|c| c.count),
        "comment_count": n.edge_media_to_comment.as_ref().map(|c| c.count),
        "taken_at":      n.taken_at_timestamp,
        "caption":       caption,
        "alt_text":      n.accessibility_caption,
        "dimensions":    n.dimensions.as_ref().map(|d| json!({"width": d.width, "height": d.height})),
        "product_type":  n.product_type,
    })
}

/// Best-effort post-type classification. `clips` is reels; `feed` is
/// the regular grid. Sidecar = multi-photo carousel.
fn classify(n: &MediaNode) -> &'static str {
    if n.product_type.as_deref() == Some("clips") {
        return "reel";
    }
    match n.typename.as_deref() {
        Some("GraphSidecar") => "carousel",
        Some("GraphVideo") => "video",
        Some("GraphImage") => "photo",
        _ => "post",
    }
}

/// Fallback when the API path is blocked: hit the public profile HTML,
/// pull whatever OG tags we can. Returns less data and explicitly
/// flags `data_completeness: "og_only"` so callers know.
async fn og_fallback(
    client: &dyn Fetcher,
    username: &str,
    original_url: &str,
    api_status: u16,
) -> Result<Value, FetchError> {
    let canonical = format!("https://www.instagram.com/{username}/");
    let resp = client.fetch(&canonical).await?;
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "instagram_profile: api status {api_status}, html status {} for {username}",
            resp.status
        )));
    }
    let og = parse_og_tags(&resp.html);
    let (followers, following, posts) =
        parse_counts_from_og_description(og.get("description").map(String::as_str));

    Ok(json!({
        "url":               original_url,
        "canonical_url":     canonical,
        "username":          username,
        "data_completeness": "og_only",
        "fallback_reason":   format!("api returned {api_status}"),
        "full_name":         parse_full_name(&og.get("title").cloned().unwrap_or_default()),
        "follower_count":    followers,
        "following_count":   following,
        "post_count":        posts,
        "profile_pic_url":   og.get("image").cloned(),
        "biography":         null_value(),
        "is_verified":       null_value(),
        "is_business":       null_value(),
        "recent_posts":      Vec::<Value>::new(),
    }))
}

fn null_value() -> Value {
    Value::Null
}

// ---------------------------------------------------------------------------
// URL parsing
// ---------------------------------------------------------------------------

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

fn parse_username(url: &str) -> Option<String> {
    let path = url.split("://").nth(1)?.split_once('/').map(|(_, p)| p)?;
    let stripped = path.split(['?', '#']).next()?.trim_end_matches('/');
    stripped
        .split('/')
        .find(|s| !s.is_empty())
        .map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// OG-fallback helpers (kept self-contained — same shape as the previous
// version we shipped, retained as the safety net)
// ---------------------------------------------------------------------------

fn parse_og_tags(html: &str) -> std::collections::HashMap<String, String> {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+property="og:([a-z_]+)"[^>]+content="([^"]+)""#).unwrap()
    });
    let mut out = std::collections::HashMap::new();
    for c in re.captures_iter(html) {
        let k = c
            .get(1)
            .map(|m| m.as_str().to_lowercase())
            .unwrap_or_default();
        let v = c
            .get(2)
            .map(|m| html_decode(m.as_str()))
            .unwrap_or_default();
        out.entry(k).or_insert(v);
    }
    out
}

fn parse_full_name(og_title: &str) -> Option<String> {
    if og_title.is_empty() {
        return None;
    }
    let decoded = html_decode(og_title);
    let trimmed = decoded.split('(').next().unwrap_or(&decoded).trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_counts_from_og_description(desc: Option<&str>) -> (Option<i64>, Option<i64>, Option<i64>) {
    let Some(text) = desc else {
        return (None, None, None);
    };
    let decoded = html_decode(text);
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)([\d.,]+[KMB]?)\s*Followers,\s*([\d.,]+[KMB]?)\s*Following,\s*([\d.,]+[KMB]?)\s*Posts").unwrap()
    });
    if let Some(c) = re.captures(&decoded) {
        return (
            c.get(1).and_then(|m| parse_compact_number(m.as_str())),
            c.get(2).and_then(|m| parse_compact_number(m.as_str())),
            c.get(3).and_then(|m| parse_compact_number(m.as_str())),
        );
    }
    (None, None, None)
}

fn parse_compact_number(s: &str) -> Option<i64> {
    let s = s.trim();
    let (num_str, mul) = match s.chars().last() {
        Some('K') => (&s[..s.len() - 1], 1_000i64),
        Some('M') => (&s[..s.len() - 1], 1_000_000i64),
        Some('B') => (&s[..s.len() - 1], 1_000_000_000i64),
        _ => (s, 1i64),
    };
    let cleaned: String = num_str.chars().filter(|c| *c != ',').collect();
    cleaned.parse::<f64>().ok().map(|f| (f * mul as f64) as i64)
}

fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#064;", "@")
        .replace("&#x2022;", "•")
        .replace("&hellip;", "…")
}

// ---------------------------------------------------------------------------
// Instagram web_profile_info API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ApiResponse {
    data: ApiData,
}

#[derive(Deserialize)]
struct ApiData {
    user: User,
}

#[derive(Deserialize)]
struct User {
    id: Option<String>,
    username: Option<String>,
    full_name: Option<String>,
    biography: Option<String>,
    bio_links: Option<Vec<serde_json::Value>>,
    external_url: Option<String>,
    category_name: Option<String>,
    profile_pic_url: Option<String>,
    profile_pic_url_hd: Option<String>,
    is_verified: Option<bool>,
    is_private: Option<bool>,
    is_business_account: Option<bool>,
    is_professional_account: Option<bool>,
    edge_followed_by: Option<EdgeCount>,
    edge_follow: Option<EdgeCount>,
    edge_owner_to_timeline_media: Option<MediaEdges>,
}

#[derive(Deserialize)]
struct EdgeCount {
    count: i64,
}

#[derive(Deserialize)]
struct MediaEdges {
    count: i64,
    edges: Vec<MediaEdge>,
}

#[derive(Deserialize)]
struct MediaEdge {
    node: MediaNode,
}

#[derive(Deserialize)]
struct MediaNode {
    #[serde(rename = "__typename")]
    typename: Option<String>,
    shortcode: Option<String>,
    is_video: Option<bool>,
    video_view_count: Option<i64>,
    display_url: Option<String>,
    thumbnail_src: Option<String>,
    accessibility_caption: Option<String>,
    taken_at_timestamp: Option<i64>,
    product_type: Option<String>,
    dimensions: Option<Dimensions>,
    edge_media_preview_like: Option<EdgeCount>,
    edge_media_to_comment: Option<EdgeCount>,
    edge_media_to_caption: Option<CaptionEdges>,
}

#[derive(Deserialize)]
struct Dimensions {
    width: i64,
    height: i64,
}

#[derive(Deserialize)]
struct CaptionEdges {
    edges: Vec<CaptionEdge>,
}

#[derive(Deserialize)]
struct CaptionEdge {
    node: CaptionNode,
}

#[derive(Deserialize)]
struct CaptionNode {
    text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_profile_urls() {
        assert!(matches("https://www.instagram.com/ticketswave"));
        assert!(matches("https://www.instagram.com/ticketswave/"));
        assert!(matches("https://instagram.com/0xmassi/?hl=en"));
        assert!(!matches("https://www.instagram.com/p/DT-RICMjeK5/"));
        assert!(!matches("https://www.instagram.com/explore"));
        assert!(!matches("https://www.instagram.com/"));
        assert!(!matches("https://example.com/foo"));
    }

    #[test]
    fn parse_full_name_strips_handle() {
        assert_eq!(
            parse_full_name("Ticket Wave (&#064;ticketswave) &#x2022; Instagram photos and videos"),
            Some("Ticket Wave".into())
        );
    }

    #[test]
    fn compact_number_handles_kmb() {
        assert_eq!(parse_compact_number("18K"), Some(18_000));
        assert_eq!(parse_compact_number("1.5M"), Some(1_500_000));
        assert_eq!(parse_compact_number("1,234"), Some(1_234));
        assert_eq!(parse_compact_number("641"), Some(641));
    }
}
