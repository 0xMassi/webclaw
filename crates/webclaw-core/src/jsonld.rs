/// Schema-aware JSON-LD classification.
///
/// The existing `structured_data::extract_json_ld` returns raw parsed
/// `serde_json::Value`s. This module classifies them into the typed
/// `JsonLdSchema` enum that the M4 CLI flags (`--prefer-structured`,
/// `--articles-from-jsonld`) route on.
///
/// Design: a thin classifier on top of the existing parser. We do NOT
/// re-implement JSON-LD parsing — we accept the same `Vec<Value>` that
/// `ExtractionResult.structured_data` already carries, and produce a
/// typed view useful for downstream formatting.
use serde::Serialize;
use serde_json::Value;

/// Article reference extracted from an ItemList / LiveBlogPosting.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArticleRef {
    pub title: Option<String>,
    pub url: Option<String>,
    pub published: Option<String>,
    pub position: Option<u64>,
}

/// One update from a LiveBlogPosting.liveBlogUpdate array.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveUpdate {
    pub headline: Option<String>,
    pub url: Option<String>,
    pub published: Option<String>,
}

/// Classified JSON-LD record. Mirrors the schema.org types that webclaw
/// callers care about most: ItemList (Reuters category pages, Pitchfork
/// index), LiveBlogPosting (Le Monde live updates), NewsArticle / Article
/// (most outlets), Review (Pitchfork album reviews), and chrome types
/// (WebPage, WebSite, SiteNavigationElement) that downstream formatters
/// usually drop.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "schema", rename_all = "PascalCase")]
pub enum JsonLdSchema {
    /// `@type=ItemList` — possibly nested inside `CollectionPage.mainEntity`.
    ItemList {
        items: Vec<ArticleRef>,
        number_of_items: Option<u64>,
    },
    /// `@type=LiveBlogPosting` — Le Monde / Guardian live coverage.
    LiveBlogPosting {
        headline: Option<String>,
        updates: Vec<LiveUpdate>,
    },
    /// `@type=NewsArticle` / `Article` / `BlogPosting`.
    NewsArticle {
        headline: Option<String>,
        body: Option<String>,
        date_published: Option<String>,
        author: Option<String>,
    },
    /// `@type=Review` — Pitchfork album reviews.
    Review {
        headline: Option<String>,
        review_body: Option<String>,
        rated_item: Option<String>,
        author: Option<String>,
        date_published: Option<String>,
    },
    /// Chrome types: WebPage, WebSite, SiteNavigationElement, BreadcrumbList.
    /// Formatters typically drop these unless explicitly asked to surface.
    WebPageOrChrome { raw_type: String },
    /// Recognised schema.org type we don't have a typed variant for yet.
    /// The raw value is preserved so callers can still emit it.
    Unknown {
        raw_type: String,
        raw: Box<serde_json::Value>,
    },
}

impl JsonLdSchema {
    /// Convenience: is this a content-bearing schema (vs WebPage chrome)?
    pub fn is_content(&self) -> bool {
        !matches!(self, JsonLdSchema::WebPageOrChrome { .. })
    }

    /// Convenience: short stable string for the schema kind, used by probe.py.
    pub fn kind(&self) -> &'static str {
        match self {
            JsonLdSchema::ItemList { .. } => "ItemList",
            JsonLdSchema::LiveBlogPosting { .. } => "LiveBlogPosting",
            JsonLdSchema::NewsArticle { .. } => "NewsArticle",
            JsonLdSchema::Review { .. } => "Review",
            JsonLdSchema::WebPageOrChrome { .. } => "WebPageOrChrome",
            JsonLdSchema::Unknown { .. } => "Unknown",
        }
    }
}

/// Classify a single JSON-LD value. Descends into `mainEntity` once
/// (Reuters `CollectionPage.mainEntity` → ItemList).
pub fn classify_value(v: &Value) -> Option<JsonLdSchema> {
    let obj = v.as_object()?;
    let raw_type = type_string(obj.get("@type"))?;
    let lower = raw_type.to_ascii_lowercase();

    match lower.as_str() {
        "itemlist" => Some(parse_itemlist(obj)),
        "liveblogposting" => Some(parse_liveblog(obj)),
        "newsarticle" | "article" | "blogposting" | "reportagenewsarticle" => {
            Some(parse_news_article(obj))
        }
        "review" => Some(parse_review(obj)),
        // Chrome / navigation types — explicit list.
        "webpage" | "website" | "sitenavigationelement" | "breadcrumblist"
        | "collectionpage" => {
            // CollectionPage may wrap an ItemList in mainEntity. If so, lift it.
            if let Some(main) = obj.get("mainEntity") {
                if let Some(inner) = classify_value(main) {
                    return Some(inner);
                }
            }
            Some(JsonLdSchema::WebPageOrChrome { raw_type })
        }
        _ => Some(JsonLdSchema::Unknown {
            raw_type,
            raw: Box::new(v.clone()),
        }),
    }
}

/// Classify a `Vec<Value>` (matches `ExtractionResult.structured_data`'s shape).
/// Returns one `JsonLdSchema` per input value.
pub fn classify_all(values: &[Value]) -> Vec<JsonLdSchema> {
    values.iter().filter_map(classify_value).collect()
}

/// Find the FIRST schema among the classified items that is a content-bearing
/// type useful for routing. Priority: ItemList > LiveBlogPosting > Review >
/// NewsArticle > Unknown > WebPageOrChrome.
pub fn primary_schema(schemas: &[JsonLdSchema]) -> Option<&JsonLdSchema> {
    let priority = |s: &JsonLdSchema| -> u8 {
        match s {
            JsonLdSchema::ItemList { .. } => 0,
            JsonLdSchema::LiveBlogPosting { .. } => 1,
            JsonLdSchema::Review { .. } => 2,
            JsonLdSchema::NewsArticle { .. } => 3,
            JsonLdSchema::Unknown { .. } => 4,
            JsonLdSchema::WebPageOrChrome { .. } => 5,
        }
    };
    schemas.iter().min_by_key(|s| priority(s))
}

// ----------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------

fn type_string(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => a
            .iter()
            .find_map(|x| x.as_str().map(str::to_string)),
        _ => None,
    }
}

fn str_field(obj: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn u64_field(obj: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
    obj.get(key).and_then(|v| v.as_u64())
}

fn author_string(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("name").and_then(|n| n.as_str()).map(str::to_string),
        Value::Array(a) => {
            let names: Vec<String> = a
                .iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(s.clone()),
                    Value::Object(o) => o.get("name").and_then(|n| n.as_str()).map(str::to_string),
                    _ => None,
                })
                .collect();
            if names.is_empty() {
                None
            } else {
                Some(names.join(", "))
            }
        }
        _ => None,
    }
}

fn item_reviewed_string(v: Option<&Value>) -> Option<String> {
    let v = v?;
    let obj = v.as_object()?;
    obj.get("name").and_then(|n| n.as_str()).map(str::to_string)
}

fn parse_itemlist(obj: &serde_json::Map<String, Value>) -> JsonLdSchema {
    let mut items = Vec::new();
    if let Some(arr) = obj.get("itemListElement").and_then(|v| v.as_array()) {
        for entry in arr {
            let Some(e) = entry.as_object() else { continue };
            // Two shapes seen in the wild:
            // (1) ListItem with {position, url, name}.
            // (2) ListItem with {position, item: {url, name, datePublished}}.
            let inner_obj = e.get("item").and_then(|v| v.as_object()).unwrap_or(e);

            let position = u64_field(e, "position").or_else(|| u64_field(inner_obj, "position"));
            let url = str_field(inner_obj, "url").or_else(|| str_field(e, "url"));
            let title = str_field(inner_obj, "name")
                .or_else(|| str_field(e, "name"))
                .or_else(|| str_field(inner_obj, "headline"))
                .or_else(|| str_field(e, "headline"));
            let published = str_field(inner_obj, "datePublished")
                .or_else(|| str_field(e, "datePublished"));

            items.push(ArticleRef {
                title,
                url,
                published,
                position,
            });
        }
    }
    let number_of_items = u64_field(obj, "numberOfItems");
    JsonLdSchema::ItemList {
        items,
        number_of_items,
    }
}

fn parse_liveblog(obj: &serde_json::Map<String, Value>) -> JsonLdSchema {
    let headline = str_field(obj, "headline");
    let mut updates = Vec::new();
    if let Some(arr) = obj.get("liveBlogUpdate").and_then(|v| v.as_array()) {
        for entry in arr {
            let Some(e) = entry.as_object() else { continue };
            updates.push(LiveUpdate {
                headline: str_field(e, "headline"),
                url: str_field(e, "url"),
                published: str_field(e, "datePublished"),
            });
        }
    }
    JsonLdSchema::LiveBlogPosting { headline, updates }
}

fn parse_news_article(obj: &serde_json::Map<String, Value>) -> JsonLdSchema {
    let headline = str_field(obj, "headline");
    // articleBody is the canonical field; some sites use description.
    let body = str_field(obj, "articleBody").or_else(|| str_field(obj, "description"));
    let date_published = str_field(obj, "datePublished");
    let author = author_string(obj.get("author"));
    JsonLdSchema::NewsArticle {
        headline,
        body,
        date_published,
        author,
    }
}

fn parse_review(obj: &serde_json::Map<String, Value>) -> JsonLdSchema {
    let headline = str_field(obj, "headline").or_else(|| str_field(obj, "name"));
    let review_body = str_field(obj, "reviewBody").or_else(|| str_field(obj, "description"));
    let rated_item = item_reviewed_string(obj.get("itemReviewed"));
    let author = author_string(obj.get("author"));
    let date_published = str_field(obj, "datePublished");
    JsonLdSchema::Review {
        headline,
        review_body,
        rated_item,
        author,
        date_published,
    }
}

// ----------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Test 1: ItemList JSON-LD with 3 itemListElement entries.
    #[test]
    fn test_jsonld_parse_itemlist() {
        let v = json!({
            "@context": "https://schema.org",
            "@type": "ItemList",
            "numberOfItems": 3,
            "itemListElement": [
                {"@type": "ListItem", "position": 1, "url": "https://a.example/1", "name": "First"},
                {"@type": "ListItem", "position": 2, "url": "https://a.example/2", "name": "Second"},
                {"@type": "ListItem", "position": 3, "url": "https://a.example/3", "name": "Third"},
            ]
        });
        let s = classify_value(&v).expect("classify");
        match s {
            JsonLdSchema::ItemList { items, number_of_items } => {
                assert_eq!(number_of_items, Some(3));
                assert_eq!(items.len(), 3);
                assert_eq!(items[0].position, Some(1));
                assert_eq!(items[0].url.as_deref(), Some("https://a.example/1"));
                assert_eq!(items[0].title.as_deref(), Some("First"));
                assert_eq!(items[2].position, Some(3));
            }
            other => panic!("expected ItemList, got {other:?}"),
        }
    }

    /// Test 2: LiveBlogPosting with 2 liveBlogUpdate entries.
    #[test]
    fn test_jsonld_parse_liveblog() {
        let v = json!({
            "@type": "LiveBlogPosting",
            "headline": "Election Night Live",
            "liveBlogUpdate": [
                {"headline": "Polls closing", "url": "https://x/1", "datePublished": "2026-05-23T19:00:00Z"},
                {"headline": "First results", "url": "https://x/2", "datePublished": "2026-05-23T19:15:00Z"},
            ]
        });
        let s = classify_value(&v).expect("classify");
        match s {
            JsonLdSchema::LiveBlogPosting { headline, updates } => {
                assert_eq!(headline.as_deref(), Some("Election Night Live"));
                assert_eq!(updates.len(), 2);
                assert_eq!(updates[0].headline.as_deref(), Some("Polls closing"));
                assert_eq!(updates[1].url.as_deref(), Some("https://x/2"));
            }
            other => panic!("expected LiveBlogPosting, got {other:?}"),
        }
    }

    /// Test 3: NewsArticle with articleBody.
    #[test]
    fn test_jsonld_parse_newsarticle() {
        let v = json!({
            "@type": "NewsArticle",
            "headline": "Big Story",
            "articleBody": "Lorem ipsum dolor sit amet.",
            "datePublished": "2026-05-23",
            "author": {"@type": "Person", "name": "Jane Doe"},
        });
        let s = classify_value(&v).expect("classify");
        match s {
            JsonLdSchema::NewsArticle { headline, body, date_published, author } => {
                assert_eq!(headline.as_deref(), Some("Big Story"));
                assert_eq!(body.as_deref(), Some("Lorem ipsum dolor sit amet."));
                assert_eq!(date_published.as_deref(), Some("2026-05-23"));
                assert_eq!(author.as_deref(), Some("Jane Doe"));
            }
            other => panic!("expected NewsArticle, got {other:?}"),
        }
    }

    /// Test 4: Review with reviewBody and itemReviewed.
    #[test]
    fn test_jsonld_parse_review() {
        let v = json!({
            "@type": "Review",
            "headline": "Images of Life",
            "reviewBody": "A bountiful, baroque, eccentric record.",
            "itemReviewed": {"@type": "MusicRecording", "name": "Images of Life"},
            "author": [{"@type": "Person", "name": "Critic A"}],
            "datePublished": "2026-05-23",
        });
        let s = classify_value(&v).expect("classify");
        match s {
            JsonLdSchema::Review { headline, review_body, rated_item, author, date_published } => {
                assert_eq!(headline.as_deref(), Some("Images of Life"));
                assert_eq!(review_body.as_deref(), Some("A bountiful, baroque, eccentric record."));
                assert_eq!(rated_item.as_deref(), Some("Images of Life"));
                assert_eq!(author.as_deref(), Some("Critic A"));
                assert_eq!(date_published.as_deref(), Some("2026-05-23"));
            }
            other => panic!("expected Review, got {other:?}"),
        }
    }

    /// Test 5: Unknown @type (Recipe) returns Unknown variant, doesn't crash.
    #[test]
    fn test_jsonld_parse_unknown_type() {
        let v = json!({
            "@type": "Recipe",
            "name": "Banana Bread",
            "recipeYield": "1 loaf",
        });
        let s = classify_value(&v).expect("classify");
        match s {
            JsonLdSchema::Unknown { raw_type, .. } => {
                assert_eq!(raw_type, "Recipe");
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    /// Test 6: SiteNavigationElement returns WebPageOrChrome.
    #[test]
    fn test_jsonld_parse_webpage_dropped() {
        let v = json!({
            "@type": "SiteNavigationElement",
            "name": "Main nav",
        });
        let s = classify_value(&v).expect("classify");
        assert!(matches!(s, JsonLdSchema::WebPageOrChrome { .. }));
        if let JsonLdSchema::WebPageOrChrome { raw_type } = s {
            assert_eq!(raw_type, "SiteNavigationElement");
        }
    }

    /// Test 7: Malformed Value (no @type at all) returns None, doesn't panic.
    /// The "truncated JSON" case is the parser's responsibility (already
    /// handled in structured_data.rs); the classifier sees only valid Values.
    #[test]
    fn test_jsonld_parse_malformed_no_crash() {
        // Empty object — no @type.
        let v1 = json!({});
        assert!(classify_value(&v1).is_none());

        // Bare string — not an object at all.
        let v2 = json!("garbage");
        assert!(classify_value(&v2).is_none());

        // @type is not a string or array.
        let v3 = json!({"@type": 42});
        assert!(classify_value(&v3).is_none());

        // Array of mixed garbage.
        let v4 = json!([1, "two", {"@type": "Article", "headline": "ok"}]);
        // classify_value on the array itself returns None (not an object),
        // but classify_all extracts the one Article.
        let all = classify_all(v4.as_array().unwrap());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].kind(), "NewsArticle");
    }

    /// Test 8: CollectionPage with nested mainEntity ItemList — lifts the inner.
    /// This is the Reuters shape phase A confirmed.
    #[test]
    fn test_jsonld_collectionpage_lifts_mainentity_itemlist() {
        let v = json!({
            "@type": "CollectionPage",
            "mainEntity": {
                "@type": "ItemList",
                "numberOfItems": 2,
                "itemListElement": [
                    {"@type": "ListItem", "position": 1, "url": "https://r.example/1"},
                    {"@type": "ListItem", "position": 2, "url": "https://r.example/2"},
                ]
            }
        });
        let s = classify_value(&v).expect("classify");
        match s {
            JsonLdSchema::ItemList { items, number_of_items } => {
                assert_eq!(items.len(), 2);
                assert_eq!(number_of_items, Some(2));
            }
            other => panic!("expected lifted ItemList, got {other:?}"),
        }
    }

    /// Test 9: primary_schema picks ItemList over NewsArticle and WebPage.
    #[test]
    fn test_primary_schema_picks_itemlist_first() {
        let schemas = vec![
            JsonLdSchema::WebPageOrChrome { raw_type: "WebPage".into() },
            JsonLdSchema::NewsArticle {
                headline: Some("x".into()),
                body: None,
                date_published: None,
                author: None,
            },
            JsonLdSchema::ItemList {
                items: vec![],
                number_of_items: None,
            },
        ];
        let p = primary_schema(&schemas).expect("primary");
        assert!(matches!(p, JsonLdSchema::ItemList { .. }));
    }

    /// Test 10: ListItem with nested `item` object (alternate shape).
    #[test]
    fn test_jsonld_itemlist_with_nested_item_shape() {
        let v = json!({
            "@type": "ItemList",
            "itemListElement": [
                {
                    "@type": "ListItem",
                    "position": 1,
                    "item": {
                        "@type": "NewsArticle",
                        "url": "https://x/1",
                        "name": "Wrapped Title",
                        "datePublished": "2026-05-23",
                    }
                },
            ]
        });
        let s = classify_value(&v).expect("classify");
        match s {
            JsonLdSchema::ItemList { items, .. } => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].url.as_deref(), Some("https://x/1"));
                assert_eq!(items[0].title.as_deref(), Some("Wrapped Title"));
                assert_eq!(items[0].published.as_deref(), Some("2026-05-23"));
                assert_eq!(items[0].position, Some(1));
            }
            other => panic!("expected ItemList, got {other:?}"),
        }
    }
}
