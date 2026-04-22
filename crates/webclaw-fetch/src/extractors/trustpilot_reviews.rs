//! Trustpilot company reviews extractor.
//!
//! `trustpilot.com/review/{domain}` pages are always behind AWS WAF's
//! "Verifying your connection" interstitial, so this extractor always
//! routes through [`cloud::smart_fetch_html`]. Without
//! `WEBCLAW_API_KEY` / `WEBCLAW_CLOUD_API_KEY` it returns a clean
//! "set API key" error; with one it escalates to api.webclaw.io.
//!
//! ## 2025 JSON-LD schema
//!
//! Trustpilot replaced the old single-Organization + aggregateRating
//! shape with three separate JSON-LD blocks:
//!
//! 1. `Organization` block for Trustpilot the platform itself
//!    (company info, addresses, social profiles). Not the business
//!    being reviewed. We detect and skip this.
//! 2. `Dataset` block with a csvw:Table mainEntity that contains the
//!    per-star-bucket counts for the target business plus a Total
//!    column. The Dataset's `name` is the business display name.
//! 3. `aiSummary` + `aiSummaryReviews` block: the AI-generated
//!    summary of reviews plus the individual review objects
//!    (consumer, dates, rating, title, text, language, likes).
//!
//! Plus `metadata.title` from the page head parses as
//! `"{name} is rated \"{label}\" with {rating} / 5 on Trustpilot"` and
//! `metadata.description` carries `"{N} customers have already said"`.
//! We use both as extra signal when the Dataset block is absent.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::cloud::{self, CloudError};
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "trustpilot_reviews",
    label: "Trustpilot reviews",
    description: "Returns business name, aggregate rating, star distribution, recent reviews, and the AI summary for a Trustpilot /review/{domain} page.",
    url_patterns: &["https://www.trustpilot.com/review/{domain}"],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if !matches!(host, "www.trustpilot.com" | "trustpilot.com") {
        return false;
    }
    url.contains("/review/")
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let fetched = cloud::smart_fetch_html(client, client.cloud(), url)
        .await
        .map_err(cloud_to_fetch_err)?;

    let mut data = parse(&fetched.html, url)?;
    if let Some(obj) = data.as_object_mut() {
        obj.insert(
            "data_source".into(),
            match fetched.source {
                cloud::FetchSource::Local => json!("local"),
                cloud::FetchSource::Cloud => json!("cloud"),
            },
        );
    }
    Ok(data)
}

/// Pure parser. Kept public so the cloud pipeline can reuse it on its
/// own fetched HTML without going through the async extract path.
pub fn parse(html: &str, url: &str) -> Result<Value, FetchError> {
    let domain = parse_review_domain(url).ok_or_else(|| {
        FetchError::Build(format!(
            "trustpilot_reviews: cannot parse /review/{{domain}} from '{url}'"
        ))
    })?;

    let blocks = webclaw_core::structured_data::extract_json_ld(html);

    // The business Dataset block has `about.@id` pointing to the target
    // domain's Organization (e.g. `.../Organization/anthropic.com`).
    let dataset = find_business_dataset(&blocks, &domain);

    // The aiSummary block: not typed (no `@type`), detect by key.
    let ai_block = find_ai_summary_block(&blocks);

    // Business name: Dataset > metadata.title regex > URL domain.
    let business_name = dataset
        .as_ref()
        .and_then(|d| get_string(d, "name"))
        .or_else(|| parse_name_from_og_title(html))
        .or_else(|| Some(domain.clone()));

    // Rating distribution from the csvw:Table columns. Each column has
    // csvw:name like "1 star" / "Total" and a single cell with the
    // integer count.
    let distribution = dataset.as_ref().and_then(parse_star_distribution);
    let (rating_from_dist, total_from_dist) = distribution
        .as_ref()
        .map(compute_rating_stats)
        .unwrap_or((None, None));

    // Page-title / page-description fallbacks. OG title format:
    // "Anthropic is rated \"Bad\" with 1.5 / 5 on Trustpilot"
    let (rating_label, rating_from_og) = parse_rating_from_og_title(html);
    let total_from_desc = parse_review_count_from_og_description(html);

    // Recent reviews carried by the aiSummary block.
    let recent_reviews: Vec<Value> = ai_block
        .as_ref()
        .and_then(|a| a.get("aiSummaryReviews"))
        .and_then(|arr| arr.as_array())
        .map(|arr| arr.iter().map(extract_review).collect())
        .unwrap_or_default();

    let ai_summary = ai_block
        .as_ref()
        .and_then(|a| a.get("aiSummary"))
        .and_then(|s| s.get("summary"))
        .and_then(|t| t.as_str())
        .map(String::from);

    Ok(json!({
        "url":               url,
        "domain":            domain,
        "business_name":     business_name,
        "rating_label":      rating_label,
        "average_rating":    rating_from_dist.or(rating_from_og),
        "review_count":      total_from_dist.or(total_from_desc),
        "rating_distribution": distribution,
        "ai_summary":        ai_summary,
        "recent_reviews":    recent_reviews,
        "review_count_listed": recent_reviews.len(),
    }))
}

fn cloud_to_fetch_err(e: CloudError) -> FetchError {
    FetchError::Build(e.to_string())
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

/// Pull the target domain from `trustpilot.com/review/{domain}`.
fn parse_review_domain(url: &str) -> Option<String> {
    let after = url.split("/review/").nth(1)?;
    let stripped = after
        .split(['?', '#'])
        .next()?
        .trim_end_matches('/')
        .split('/')
        .next()
        .unwrap_or("");
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

// ---------------------------------------------------------------------------
// JSON-LD block walkers
// ---------------------------------------------------------------------------

/// Find the Dataset block whose `about.@id` references the target
/// domain's Organization. Falls through to any Dataset if the @id
/// check doesn't match (Trustpilot occasionally varies the URL).
fn find_business_dataset(blocks: &[Value], domain: &str) -> Option<Value> {
    let mut fallback_any_dataset: Option<Value> = None;
    for block in blocks {
        for node in walk_graph(block) {
            if !is_dataset(&node) {
                continue;
            }
            if dataset_about_matches_domain(&node, domain) {
                return Some(node);
            }
            if fallback_any_dataset.is_none() {
                fallback_any_dataset = Some(node);
            }
        }
    }
    fallback_any_dataset
}

fn is_dataset(v: &Value) -> bool {
    v.get("@type")
        .and_then(|t| t.as_str())
        .is_some_and(|s| s == "Dataset")
}

fn dataset_about_matches_domain(v: &Value, domain: &str) -> bool {
    let about_id = v
        .get("about")
        .and_then(|a| a.get("@id"))
        .and_then(|id| id.as_str());
    let Some(id) = about_id else {
        return false;
    };
    id.contains(&format!("/Organization/{domain}"))
}

/// The aiSummary / aiSummaryReviews block has no `@type`, so match by
/// presence of the `aiSummary` key.
fn find_ai_summary_block(blocks: &[Value]) -> Option<Value> {
    for block in blocks {
        for node in walk_graph(block) {
            if node.get("aiSummary").is_some() {
                return Some(node);
            }
        }
    }
    None
}

/// Flatten each block (and its `@graph`) into a list of nodes we can
/// iterate over. Handles both `@graph: [ ... ]` (array) and
/// `@graph: { ... }` (single object) shapes — Trustpilot uses both.
fn walk_graph(block: &Value) -> Vec<Value> {
    let mut out = vec![block.clone()];
    if let Some(graph) = block.get("@graph") {
        match graph {
            Value::Array(arr) => out.extend(arr.iter().cloned()),
            Value::Object(_) => out.push(graph.clone()),
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Rating distribution (csvw:Table)
// ---------------------------------------------------------------------------

/// Parse the per-star distribution from the Dataset block. Returns
/// `{"1_star": {count, percent}, ..., "total": {count, percent}}`.
fn parse_star_distribution(dataset: &Value) -> Option<Value> {
    let columns = dataset
        .get("mainEntity")?
        .get("csvw:tableSchema")?
        .get("csvw:columns")?
        .as_array()?;
    let mut out = serde_json::Map::new();
    for col in columns {
        let name = col.get("csvw:name").and_then(|n| n.as_str())?;
        let cell = col.get("csvw:cells").and_then(|c| c.as_array())?.first()?;
        let count = cell
            .get("csvw:value")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok());
        let percent = cell
            .get("csvw:notes")
            .and_then(|n| n.as_array())
            .and_then(|arr| arr.first())
            .and_then(|s| s.as_str())
            .map(String::from);
        let key = normalise_star_key(name);
        out.insert(
            key,
            json!({
                "count":   count,
                "percent": percent,
            }),
        );
    }
    if out.is_empty() {
        None
    } else {
        Some(Value::Object(out))
    }
}

/// "1 star" -> "one_star", "Total" -> "total". Easier to consume than
/// the raw "1 star" key which fights YAML/JS property access.
fn normalise_star_key(name: &str) -> String {
    let trimmed = name.trim().to_lowercase();
    match trimmed.as_str() {
        "1 star" => "one_star".into(),
        "2 stars" => "two_stars".into(),
        "3 stars" => "three_stars".into(),
        "4 stars" => "four_stars".into(),
        "5 stars" => "five_stars".into(),
        "total" => "total".into(),
        other => other.replace(' ', "_"),
    }
}

/// Compute average rating (weighted by bucket) and total count from the
/// parsed distribution. Returns `(average, total)`.
fn compute_rating_stats(distribution: &Value) -> (Option<String>, Option<i64>) {
    let Some(obj) = distribution.as_object() else {
        return (None, None);
    };
    let get_count = |key: &str| -> i64 {
        obj.get(key)
            .and_then(|v| v.get("count"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    };
    let one = get_count("one_star");
    let two = get_count("two_stars");
    let three = get_count("three_stars");
    let four = get_count("four_stars");
    let five = get_count("five_stars");
    let total_bucket = one + two + three + four + five;
    let total = obj
        .get("total")
        .and_then(|v| v.get("count"))
        .and_then(|v| v.as_i64())
        .unwrap_or(total_bucket);
    if total == 0 {
        return (None, Some(0));
    }
    let weighted = one + (two * 2) + (three * 3) + (four * 4) + (five * 5);
    let avg = weighted as f64 / total_bucket.max(1) as f64;
    // One decimal place, matching how Trustpilot displays the score.
    (Some(format!("{avg:.1}")), Some(total))
}

// ---------------------------------------------------------------------------
// OG / meta-tag fallbacks
// ---------------------------------------------------------------------------

/// Regex out the business name from the standard Trustpilot OG title
/// shape: `"{name} is rated \"{label}\" with {rating} / 5 on Trustpilot"`.
fn parse_name_from_og_title(html: &str) -> Option<String> {
    let title = og(html, "title")?;
    // "Anthropic is rated \"Bad\" with 1.5 / 5 on Trustpilot"
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^(.+?)\s+is rated\b").unwrap());
    re.captures(&title)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Pull the rating label (e.g. "Bad", "Excellent") and numeric value
/// from the OG title.
fn parse_rating_from_og_title(html: &str) -> (Option<String>, Option<String>) {
    let Some(title) = og(html, "title") else {
        return (None, None);
    };
    static RE: OnceLock<Regex> = OnceLock::new();
    // "Anthropic is rated \"Bad\" with 1.5 / 5 on Trustpilot"
    let re = RE.get_or_init(|| {
        Regex::new(r#"is rated\s*[\\"]+([^"\\]+)[\\"]+\s*with\s*([\d.]+)\s*/\s*5"#).unwrap()
    });
    let Some(caps) = re.captures(&title) else {
        return (None, None);
    };
    (
        caps.get(1).map(|m| m.as_str().trim().to_string()),
        caps.get(2).map(|m| m.as_str().to_string()),
    )
}

/// Parse "hear what 226 customers have already said" from the OG
/// description tag.
fn parse_review_count_from_og_description(html: &str) -> Option<i64> {
    let desc = og(html, "description")?;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(\d[\d,]*)\s+customers").unwrap());
    re.captures(&desc)?
        .get(1)?
        .as_str()
        .replace(',', "")
        .parse::<i64>()
        .ok()
}

fn og(html: &str, prop: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+property="og:([a-z_]+)"[^>]+content="([^"]+)""#).unwrap()
    });
    for c in re.captures_iter(html) {
        if c.get(1).is_some_and(|m| m.as_str() == prop) {
            let raw = c.get(2).map(|m| m.as_str())?;
            return Some(html_unescape(raw));
        }
    }
    None
}

/// Minimal HTML entity unescaping for the three entities the
/// synthesize_html escaper might produce. Keeps us off a heavier dep.
fn html_unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn get_string(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str().map(String::from))
}

// ---------------------------------------------------------------------------
// Review extraction
// ---------------------------------------------------------------------------

fn extract_review(r: &Value) -> Value {
    json!({
        "id":          r.get("id").and_then(|v| v.as_str()),
        "rating":      r.get("rating").and_then(|v| v.as_i64()),
        "title":       r.get("title").and_then(|v| v.as_str()),
        "text":        r.get("text").and_then(|v| v.as_str()),
        "language":    r.get("language").and_then(|v| v.as_str()),
        "source":      r.get("source").and_then(|v| v.as_str()),
        "likes":       r.get("likes").and_then(|v| v.as_i64()),
        "author":      r.get("consumer").and_then(|c| c.get("displayName")).and_then(|v| v.as_str()),
        "author_country": r.get("consumer").and_then(|c| c.get("countryCode")).and_then(|v| v.as_str()),
        "author_review_count": r.get("consumer").and_then(|c| c.get("numberOfReviews")).and_then(|v| v.as_i64()),
        "verified":    r.get("consumer").and_then(|c| c.get("isVerified")).and_then(|v| v.as_bool()),
        "date_experienced": r.get("dates").and_then(|d| d.get("experiencedDate")).and_then(|v| v.as_str()),
        "date_published":   r.get("dates").and_then(|d| d.get("publishedDate")).and_then(|v| v.as_str()),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_trustpilot_review_urls() {
        assert!(matches("https://www.trustpilot.com/review/stripe.com"));
        assert!(matches("https://trustpilot.com/review/example.com"));
        assert!(!matches("https://www.trustpilot.com/"));
        assert!(!matches("https://example.com/review/foo"));
    }

    #[test]
    fn parse_review_domain_handles_query_and_slash() {
        assert_eq!(
            parse_review_domain("https://www.trustpilot.com/review/anthropic.com"),
            Some("anthropic.com".into())
        );
        assert_eq!(
            parse_review_domain("https://www.trustpilot.com/review/anthropic.com/"),
            Some("anthropic.com".into())
        );
        assert_eq!(
            parse_review_domain("https://www.trustpilot.com/review/anthropic.com?stars=5"),
            Some("anthropic.com".into())
        );
    }

    #[test]
    fn normalise_star_key_covers_all_buckets() {
        assert_eq!(normalise_star_key("1 star"), "one_star");
        assert_eq!(normalise_star_key("2 stars"), "two_stars");
        assert_eq!(normalise_star_key("5 stars"), "five_stars");
        assert_eq!(normalise_star_key("Total"), "total");
    }

    #[test]
    fn compute_rating_stats_weighted_average() {
        // 100 1-stars, 100 5-stars → avg 3.0 over 200 reviews.
        let dist = json!({
            "one_star":   { "count": 100, "percent": "50%" },
            "two_stars":  { "count": 0,   "percent": "0%" },
            "three_stars":{ "count": 0,   "percent": "0%" },
            "four_stars": { "count": 0,   "percent": "0%" },
            "five_stars": { "count": 100, "percent": "50%" },
            "total":      { "count": 200, "percent": "100%" },
        });
        let (avg, total) = compute_rating_stats(&dist);
        assert_eq!(avg.as_deref(), Some("3.0"));
        assert_eq!(total, Some(200));
    }

    #[test]
    fn parse_og_title_extracts_name_and_rating() {
        let html = r#"<meta property="og:title" content="Anthropic is rated &quot;Bad&quot; with 1.5 / 5 on Trustpilot">"#;
        assert_eq!(parse_name_from_og_title(html), Some("Anthropic".into()));
        let (label, rating) = parse_rating_from_og_title(html);
        assert_eq!(label.as_deref(), Some("Bad"));
        assert_eq!(rating.as_deref(), Some("1.5"));
    }

    #[test]
    fn parse_review_count_from_og_description_picks_number() {
        let html = r#"<meta property="og:description" content="Do you agree? Voice your opinion today and hear what 226 customers have already said.">"#;
        assert_eq!(parse_review_count_from_og_description(html), Some(226));
    }

    #[test]
    fn parse_full_fixture_assembles_all_fields() {
        let html = r##"<html><head>
<meta property="og:title" content="Anthropic is rated &quot;Bad&quot; with 1.5 / 5 on Trustpilot">
<meta property="og:description" content="Voice your opinion today and hear what 226 customers have already said.">
<script type="application/ld+json">
{"@context":"https://schema.org","@graph":[
  {"@id":"https://www.trustpilot.com/#/schema/Organization/1","@type":"Organization","name":"Trustpilot"}
]}
</script>
<script type="application/ld+json">
{"@context":["https://schema.org",{"csvw":"http://www.w3.org/ns/csvw#"}],
 "@graph":{"@id":"https://www.trustpilot.com/#/schema/DataSet/anthropic.com/1",
 "@type":"Dataset",
 "about":{"@id":"https://www.trustpilot.com/#/schema/Organization/anthropic.com"},
 "name":"Anthropic",
 "mainEntity":{"@type":"csvw:Table","csvw:tableSchema":{"csvw:columns":[
   {"csvw:name":"1 star","csvw:cells":[{"csvw:value":"196","csvw:notes":["87%"]}]},
   {"csvw:name":"2 stars","csvw:cells":[{"csvw:value":"9","csvw:notes":["4%"]}]},
   {"csvw:name":"3 stars","csvw:cells":[{"csvw:value":"5","csvw:notes":["2%"]}]},
   {"csvw:name":"4 stars","csvw:cells":[{"csvw:value":"1","csvw:notes":["0%"]}]},
   {"csvw:name":"5 stars","csvw:cells":[{"csvw:value":"15","csvw:notes":["7%"]}]},
   {"csvw:name":"Total","csvw:cells":[{"csvw:value":"226","csvw:notes":["100%"]}]}
 ]}}}}
</script>
<script type="application/ld+json">
{"aiSummary":{"modelVersion":"2.0.0","summary":"Mixed reviews."},
 "aiSummaryReviews":[
  {"id":"abc","rating":1,"title":"Bad","text":"Didn't work.","language":"en",
   "source":"Organic","likes":2,"consumer":{"displayName":"W.FRH","countryCode":"DE","numberOfReviews":69,"isVerified":false},
   "dates":{"experiencedDate":"2026-01-05T00:00:00.000Z","publishedDate":"2026-01-05T16:29:31.000Z"}}]}
</script>
</head></html>"##;
        let v = parse(html, "https://www.trustpilot.com/review/anthropic.com").unwrap();
        assert_eq!(v["domain"], "anthropic.com");
        assert_eq!(v["business_name"], "Anthropic");
        assert_eq!(v["rating_label"], "Bad");
        assert_eq!(v["review_count"], 226);
        assert_eq!(v["rating_distribution"]["one_star"]["count"], 196);
        assert_eq!(v["rating_distribution"]["total"]["count"], 226);
        assert_eq!(v["ai_summary"], "Mixed reviews.");
        assert_eq!(v["recent_reviews"].as_array().unwrap().len(), 1);
        assert_eq!(v["recent_reviews"][0]["author"], "W.FRH");
        assert_eq!(v["recent_reviews"][0]["rating"], 1);
        assert_eq!(v["recent_reviews"][0]["title"], "Bad");
    }

    #[test]
    fn parse_falls_back_to_og_when_no_jsonld() {
        let html = r#"<meta property="og:title" content="Anthropic is rated &quot;Bad&quot; with 1.5 / 5 on Trustpilot">
<meta property="og:description" content="Voice your opinion today and hear what 226 customers have already said.">"#;
        let v = parse(html, "https://www.trustpilot.com/review/anthropic.com").unwrap();
        assert_eq!(v["domain"], "anthropic.com");
        assert_eq!(v["business_name"], "Anthropic");
        assert_eq!(v["average_rating"], "1.5");
        assert_eq!(v["review_count"], 226);
        assert_eq!(v["rating_label"], "Bad");
    }

    #[test]
    fn parse_returns_ok_with_url_domain_when_nothing_else() {
        let v = parse(
            "<html><head></head></html>",
            "https://www.trustpilot.com/review/example.com",
        )
        .unwrap();
        assert_eq!(v["domain"], "example.com");
        assert_eq!(v["business_name"], "example.com");
    }
}
