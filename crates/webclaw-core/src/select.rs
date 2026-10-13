//! Pick values out of a page with CSS selectors.
//!
//! A selector may end in `::text` (the element's text, descendants included,
//! whitespace collapsed; the default), `::attr(name)` or `::html` (the
//! element's outer HTML). `href` and `src` attributes resolve against the
//! page URL when one is given.

use scraper::{ElementRef, Html, Selector};
use serde_json::{Map, Value};
use url::Url;

/// A selector that does not parse.
#[derive(Debug, thiserror::Error)]
#[error("invalid selector `{0}`")]
pub struct SelectError(pub String);

enum Take {
    Text,
    Html,
    Attr(String),
}

struct Query {
    css: Option<Selector>,
    take: Take,
}

impl Query {
    fn parse(query: &str) -> Result<Self, SelectError> {
        let query = query.trim();
        let (css, take) = if let Some(css) = query.strip_suffix("::text") {
            (css, Take::Text)
        } else if let Some(css) = query.strip_suffix("::html") {
            (css, Take::Html)
        } else if let Some((css, name)) = query
            .strip_suffix(')')
            .and_then(|rest| rest.rsplit_once("::attr("))
        {
            (css, Take::Attr(name.trim().to_ascii_lowercase()))
        } else {
            (query, Take::Text)
        };
        let css = css.trim();
        let css = if css.is_empty() {
            None
        } else {
            Some(Selector::parse(css).map_err(|_| SelectError(query.to_string()))?)
        };
        Ok(Self { css, take })
    }

    fn value(&self, element: ElementRef, base: Option<&Url>) -> Option<String> {
        match &self.take {
            Take::Text => Some(
                element
                    .text()
                    .flat_map(str::split_whitespace)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            Take::Html => Some(element.html()),
            Take::Attr(name) => {
                let value = element.value().attr(name)?;
                Some(match (name.as_str(), base) {
                    ("href" | "src", Some(base)) => base
                        .join(value)
                        .map_or_else(|_| value.to_string(), String::from),
                    _ => value.to_string(),
                })
            }
        }
    }
}

/// Every value `selector` picks from `html`, in document order. Elements
/// without the requested attribute are skipped.
pub fn select(html: &str, url: Option<&str>, selector: &str) -> Result<Vec<String>, SelectError> {
    let query = Query::parse(selector)?;
    let css = query
        .css
        .as_ref()
        .ok_or_else(|| SelectError(selector.into()))?;
    let base = url.and_then(|u| Url::parse(u).ok());
    let document = Html::parse_document(html);
    Ok(document
        .select(css)
        .filter_map(|element| query.value(element, base.as_ref()))
        .collect())
}

/// One record per element `selector` matches. Each field selector runs
/// inside that element and keeps its first value, or `null`; a field
/// selector with no CSS part, such as `::attr(href)`, reads the element itself.
pub fn select_records(
    html: &str,
    url: Option<&str>,
    selector: &str,
    fields: &[(String, String)],
) -> Result<Vec<Map<String, Value>>, SelectError> {
    let css = Query::parse(selector)?
        .css
        .ok_or_else(|| SelectError(selector.into()))?;
    let fields = fields
        .iter()
        .map(|(name, field)| Ok((name, Query::parse(field)?)))
        .collect::<Result<Vec<_>, SelectError>>()?;
    let base = url.and_then(|u| Url::parse(u).ok());
    let document = Html::parse_document(html);
    Ok(document
        .select(&css)
        .map(|element| {
            fields
                .iter()
                .map(|(name, query)| {
                    let value = match &query.css {
                        Some(css) => element
                            .select(css)
                            .find_map(|inner| query.value(inner, base.as_ref())),
                        None => query.value(element, base.as_ref()),
                    };
                    (name.to_string(), value.map_or(Value::Null, Value::String))
                })
                .collect()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><body><ol>
        <li class="book"><h3><a href="/b/1" title="A Light">A Light...</a></h3><p class="price">£51.77</p></li>
        <li class="book"><h3><a href="/b/2" title="Tipping">Tipping...</a></h3></li>
        <li><a>no href</a></li>
    </ol><p id="intro">Hello <b>big</b>
       world</p></body></html>"#;

    #[test]
    fn text_is_the_default_and_includes_descendants() {
        assert_eq!(select(PAGE, None, "#intro").unwrap(), ["Hello big world"]);
        assert_eq!(
            select(PAGE, None, "#intro::text").unwrap(),
            ["Hello big world"]
        );
    }

    #[test]
    fn attributes_resolve_links_and_skip_missing_ones() {
        assert_eq!(
            select(
                PAGE,
                Some("https://books.example/catalogue/"),
                "li a::attr(href)"
            )
            .unwrap(),
            ["https://books.example/b/1", "https://books.example/b/2"]
        );
        assert_eq!(
            select(PAGE, None, "h3 a::attr(title)").unwrap(),
            ["A Light", "Tipping"]
        );
    }

    #[test]
    fn html_returns_the_outer_markup() {
        assert_eq!(
            select(PAGE, None, "#intro b::html").unwrap(),
            ["<b>big</b>"]
        );
    }

    #[test]
    fn records_take_the_first_value_per_field_or_null() {
        let fields = [
            ("title".to_string(), "h3 a::attr(title)".to_string()),
            ("price".to_string(), ".price".to_string()),
            ("class".to_string(), "::attr(class)".to_string()),
        ];
        let records = select_records(PAGE, None, "li.book", &fields).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["title"], "A Light");
        assert_eq!(records[0]["price"], "£51.77");
        assert_eq!(records[0]["class"], "book");
        assert_eq!(records[1]["price"], Value::Null);
    }

    #[test]
    fn a_bad_selector_is_an_error() {
        assert!(select(PAGE, None, "li[").is_err());
        assert!(select(PAGE, None, "::text").is_err());
        assert!(select_records(PAGE, None, "li", &[("x".into(), "a[".into())]).is_err());
    }
}
