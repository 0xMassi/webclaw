//! Find elements by their text, or every element like an example.
//!
//! An element's text is its visible text, descendants included, whitespace
//! collapsed; `script`, `style`, `noscript` and `template` content does not
//! count. A text search returns the deepest elements that match: an element
//! is left out when one of its child elements matches too.

use std::collections::{HashMap, HashSet};

use ego_tree::NodeId;
use ego_tree::iter::Edge;
use regex::{Regex, RegexBuilder};
use scraper::{ElementRef, Html, Node, Selector};
use serde::Serialize;

use crate::select::SelectError;

const HIDDEN: [&str; 4] = ["script", "style", "noscript", "template"];
const MAX_STEPS: usize = 64;
const MAX_CLASSES: usize = 32;

/// One element a search found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Found {
    /// A CSS selector that picks this element alone on this page.
    pub selector: String,
    /// The element's visible text.
    pub text: String,
}

/// Every element like an example, and one CSS selector that picks them all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Similar {
    /// Picks exactly `matches` on this page; reuse it with [`crate::select()`]
    /// or [`crate::select_records()`].
    pub selector: String,
    /// The example and every element like it, in document order.
    pub matches: Vec<Found>,
}

/// Why a search could not run. Echoed input is cut to its first 100
/// characters.
#[derive(Debug, thiserror::Error)]
pub enum FindError {
    /// A CSS selector or XPath expression that does not parse.
    #[error("invalid selector `{0}`")]
    Selector(String),
    /// A regular expression, or text to find, that does not compile.
    #[error("invalid regex `{pattern}`: {reason}")]
    Regex {
        /// The pattern, shortened.
        pattern: String,
        /// Why it does not compile.
        reason: String,
    },
}

impl From<SelectError> for FindError {
    fn from(error: SelectError) -> Self {
        Self::Selector(short(&error.0))
    }
}

/// What an element's text has to match.
#[derive(Debug, Clone)]
pub struct TextMatch(Regex);

impl TextMatch {
    /// Text that contains `text`, or equals it when `exact`. Whitespace in
    /// `text` is collapsed like the page text.
    pub fn text(text: &str, exact: bool, case_sensitive: bool) -> Result<Self, FindError> {
        let escaped = regex::escape(&collapse(text));
        let pattern = if exact {
            format!("^{escaped}$")
        } else {
            escaped
        };
        Self::build(&pattern, case_sensitive, text)
    }

    /// Text in which the regular expression `pattern` finds a match.
    pub fn regex(pattern: &str, case_sensitive: bool) -> Result<Self, FindError> {
        Self::build(pattern, case_sensitive, pattern)
    }

    fn build(pattern: &str, case_sensitive: bool, source: &str) -> Result<Self, FindError> {
        RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map(Self)
            .map_err(|error| FindError::Regex {
                pattern: short(source),
                reason: match error {
                    regex::Error::Syntax(message) => message
                        .lines()
                        .rev()
                        .find_map(|line| line.strip_prefix("error: "))
                        .unwrap_or("syntax error")
                        .to_string(),
                    other => short(&other.to_string()),
                },
            })
    }
}

/// What picks the example for [`find_similar`]: the first element it matches
/// in document order.
#[derive(Debug, Clone, Copy)]
pub enum Example<'a> {
    /// A CSS selector.
    Css(&'a str),
    /// An XPath 1.0 expression; its first element node is the example.
    XPath(&'a str),
    /// The first element [`find_text`] returns.
    Text(&'a TextMatch),
}

/// The deepest elements whose text matches, in document order.
pub fn find_text(html: &str, text: &TextMatch) -> Vec<Found> {
    let document = Html::parse_document(html);
    found_all(matching(&document, text))
}

/// Every element like the example. The search starts from the example's path
/// from the root (its last 64 steps), each step its tag and first 32 classes, and
/// widens it while every element that adds shares a child tag with the
/// example (or has no child elements, like it) and sits under a parent
/// holding no earlier match. It cuts steps from the top as long as one step
/// with a class is left, and drops the classes of a step, all at once or one
/// at a time, tried under the whole path and under its shortest tail that
/// starts at a step with a class and picks the same elements; dropped classes
/// that every match carries come back. So a class that only some items carry
/// goes, while one that tells two parts of an item apart stays. The selector
/// returned is that shortest tail. `None` when nothing matches the example.
pub fn find_similar(html: &str, example: Example) -> Result<Option<Similar>, FindError> {
    let document = Html::parse_document(html);
    let example = match example {
        Example::Css(css) => document
            .select(&Selector::parse(css).map_err(|_| FindError::Selector(short(css)))?)
            .next(),
        Example::XPath(expression) => {
            crate::xpath::first_element(html, expression)?.and_then(|index| {
                document
                    .root_element()
                    .descendants()
                    .filter_map(ElementRef::wrap)
                    .nth(index)
            })
        }
        Example::Text(text) => matching(&document, text).into_iter().next(),
    };
    let Some(example) = example else {
        return Ok(None);
    };
    let example_shape = shape(example);
    let mut steps: Vec<(String, Vec<String>)> = example
        .ancestors()
        .filter_map(ElementRef::wrap)
        .take(MAX_STEPS - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .chain([example])
        .map(|element| {
            let classes = element
                .value()
                .classes()
                .map(ident)
                .filter(|class| Selector::parse(&format!(".{class}")).is_ok())
                .take(MAX_CLASSES)
                .collect();
            (ident(element.value().name()), classes)
        })
        .collect();
    let mut from = 0;
    let mut matched = pick(&document, &steps, from)?;
    let mut changed = true;
    while changed {
        changed = false;
        for cut in from + 1..steps.len() {
            if !anchored(&steps, cut) {
                break;
            }
            let more = pick(&document, &steps, cut)?;
            if more.len() == matched.len() {
                continue;
            }
            if alike(&matched, &more, &example_shape) {
                from = cut;
                matched = more;
                changed = true;
            }
            break;
        }
        let mut tight = tail(&document, &steps, from, &matched)?;
        for level in (from..steps.len()).rev() {
            let classes = steps[level].1.clone();
            let singles = classes
                .iter()
                .cloned()
                .map(Some)
                .filter(|_| classes.len() > 1);
            for drop in std::iter::once(None).chain(singles) {
                let mut wider = steps.clone();
                wider[level]
                    .1
                    .retain(|c| drop.as_ref().is_some_and(|d| d != c));
                if level < from || wider[level].1.len() == steps[level].1.len() {
                    continue;
                }
                for at in [from, tight] {
                    if level < at || !anchored(&wider, at) {
                        continue;
                    }
                    let more = pick(&document, &wider, at)?;
                    if more.len() > matched.len() && alike(&matched, &more, &example_shape) {
                        steps = wider;
                        matched = more;
                        from = at;
                        tight = tail(&document, &steps, from, &matched)?;
                        changed = true;
                        break;
                    }
                }
            }
            for class in classes {
                if level < from || steps[level].1.contains(&class) {
                    continue;
                }
                let mut narrower = steps.clone();
                narrower[level].1.push(class);
                if pick(&document, &narrower, from)?.len() == matched.len() {
                    steps = narrower;
                }
            }
        }
    }
    let start = tail(&document, &steps, from, &matched)?;
    Ok(Some(Similar {
        selector: render(&steps, start),
        matches: found_all(matched),
    }))
}

fn matching<'a>(document: &'a Html, text: &TextMatch) -> Vec<ElementRef<'a>> {
    let mut hits = Vec::new();
    let mut hidden = 0usize;
    for edge in document.root_element().traverse() {
        match edge {
            Edge::Open(node) => {
                let Some(element) = ElementRef::wrap(node) else {
                    continue;
                };
                if HIDDEN.contains(&element.value().name()) {
                    hidden += 1;
                } else if hidden == 0 && text.0.is_match(&visible_text(element)) {
                    hits.push(element);
                }
            }
            Edge::Close(node) => {
                if matches!(node.value(), Node::Element(e) if HIDDEN.contains(&e.name())) {
                    hidden -= 1;
                }
            }
        }
    }
    let ids: HashSet<NodeId> = hits.iter().map(|e| e.id()).collect();
    hits.into_iter()
        .filter(|e| !e.children().any(|c| ids.contains(&c.id())))
        .collect()
}

fn tail(
    document: &Html,
    steps: &[(String, Vec<String>)],
    from: usize,
    matched: &[ElementRef],
) -> Result<usize, FindError> {
    for start in (from..steps.len()).rev() {
        if !steps[start].1.is_empty() && pick(document, steps, start)? == matched {
            return Ok(start);
        }
    }
    Ok(from)
}

fn anchored(steps: &[(String, Vec<String>)], from: usize) -> bool {
    from == 0 || steps[from..].iter().any(|(_, classes)| !classes.is_empty())
}

fn alike(matched: &[ElementRef], more: &[ElementRef], example_shape: &HashSet<&str>) -> bool {
    let known: HashSet<NodeId> = matched.iter().map(|e| e.id()).collect();
    let parents: HashSet<Option<NodeId>> = matched
        .iter()
        .filter(|_| matched.len() > 1)
        .map(|e| e.parent().map(|p| p.id()))
        .collect();
    more.iter().all(|e| {
        known.contains(&e.id())
            || (fits(&shape(*e), example_shape) && !parents.contains(&e.parent().map(|p| p.id())))
    })
}

fn pick<'a>(
    document: &'a Html,
    steps: &[(String, Vec<String>)],
    from: usize,
) -> Result<Vec<ElementRef<'a>>, FindError> {
    let css = render(steps, from);
    let selector = Selector::parse(&css).map_err(|_| FindError::Selector(short(&css)))?;
    Ok(document.select(&selector).collect())
}

fn render(steps: &[(String, Vec<String>)], from: usize) -> String {
    steps[from..]
        .iter()
        .map(|(tag, classes)| {
            classes
                .iter()
                .fold(tag.clone(), |step, class| format!("{step}.{class}"))
        })
        .collect::<Vec<_>>()
        .join(" > ")
}

fn fits(shape: &HashSet<&str>, example: &HashSet<&str>) -> bool {
    shape == example || !shape.is_disjoint(example)
}

fn shape<'a>(element: ElementRef<'a>) -> HashSet<&'a str> {
    element
        .children()
        .filter_map(ElementRef::wrap)
        .map(|c| c.value().name())
        .collect()
}

fn found_all(elements: Vec<ElementRef>) -> Vec<Found> {
    let mut steps: HashMap<NodeId, String> = HashMap::new();
    elements
        .into_iter()
        .map(|element| {
            let mut path = Vec::new();
            let mut node = element;
            loop {
                if !steps.contains_key(&node.id()) {
                    sibling_steps(node, &mut steps);
                }
                path.push(steps[&node.id()].clone());
                match node.ancestors().find_map(ElementRef::wrap) {
                    Some(parent) => node = parent,
                    None => break,
                }
            }
            path.reverse();
            Found {
                selector: path.join(" > "),
                text: visible_text(element),
            }
        })
        .collect()
}

fn sibling_steps(element: ElementRef, steps: &mut HashMap<NodeId, String>) {
    let siblings: Vec<ElementRef> = match element.parent() {
        Some(parent) => parent.children().filter_map(ElementRef::wrap).collect(),
        None => vec![element],
    };
    let mut total: HashMap<&str, usize> = HashMap::new();
    for sibling in &siblings {
        *total.entry(sibling.value().name()).or_default() += 1;
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for sibling in siblings {
        let name = sibling.value().name();
        let n = seen.entry(name).or_default();
        *n += 1;
        let step = if total[name] > 1 {
            format!("{}:nth-of-type({n})", ident(name))
        } else {
            ident(name)
        };
        steps.insert(sibling.id(), step);
    }
}

fn visible_text(element: ElementRef) -> String {
    let mut text = String::new();
    let mut hidden = 0usize;
    for edge in element.traverse() {
        match edge {
            Edge::Open(node) => match node.value() {
                Node::Element(e) if HIDDEN.contains(&e.name()) => hidden += 1,
                Node::Text(t) if hidden == 0 => text.push_str(t),
                _ => {}
            },
            Edge::Close(node) => {
                if matches!(node.value(), Node::Element(e) if HIDDEN.contains(&e.name())) {
                    hidden -= 1;
                }
            }
        }
    }
    collapse(&text)
}

fn short(text: &str) -> String {
    match text.char_indices().nth(100) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ident(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        match c {
            '0'..='9' if i == 0 => out.push_str(&format!("\\{:x} ", c as u32)),
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => out.push(c),
            c if !c.is_ascii() => out.push(c),
            c => {
                out.push('\\');
                out.push(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOP: &str = r#"<html><body>
        <nav><ul><li><a href="/">Home</a></li><li><a href="/shop">Shop</a></li></ul></nav>
        <ol class="row">
          <li class="col"><article class="pod"><h3><a href="/b/1">A Light</a></h3><p class="price">£51.77</p><p class="label">New</p></article></li>
          <li class="col"><article class="pod sale"><h3><a href="/b/2">Tipping</a></h3><p class="price">£<span>53</span>.74</p><p class="label">Sale</p></article></li>
          <li class="col"><article class="pod"><h3><a href="/b/3">Soumission</a></h3><p class="price">£50.10</p><p class="label">Old</p></article></li>
        </ol>
        <script>var x = "Tipping";</script>
        </body></html>"#;

    fn texts(found: &[Found]) -> Vec<&str> {
        found.iter().map(|f| f.text.as_str()).collect()
    }

    #[test]
    fn text_search_returns_the_deepest_match_and_ignores_scripts() {
        let found = find_text(SHOP, &TextMatch::text("tipping", false, false).unwrap());
        assert_eq!(texts(&found), ["Tipping"]);
        assert_eq!(
            found[0].selector,
            "html > body > ol > li:nth-of-type(2) > article > h3 > a"
        );
        assert_eq!(
            crate::select(SHOP, None, &found[0].selector).unwrap(),
            ["Tipping"]
        );
    }

    #[test]
    fn text_split_by_inline_tags_still_matches() {
        let found = find_text(SHOP, &TextMatch::text("£53.74", true, false).unwrap());
        assert_eq!(texts(&found), ["£53.74"]);
        assert!(found[0].selector.ends_with("p:nth-of-type(1)"));
    }

    #[test]
    fn exact_and_case_options() {
        let page = "<p>Shop now</p><p>shop</p>";
        let any = |exact, case| {
            texts(&find_text(
                page,
                &TextMatch::text("Shop", exact, case).unwrap(),
            ))
            .join("|")
        };
        assert_eq!(any(false, false), "Shop now|shop");
        assert_eq!(any(true, false), "shop");
        assert_eq!(any(false, true), "Shop now");
        assert_eq!(any(true, true), "");
    }

    #[test]
    fn regex_search() {
        let found = find_text(SHOP, &TextMatch::regex(r"^£5\d\.\d\d$", true).unwrap());
        assert_eq!(texts(&found), ["£51.77", "£53.74", "£50.10"]);
        assert!(TextMatch::regex("(", false).is_err());
    }

    #[test]
    fn similar_from_text_finds_the_siblings_and_a_reusable_selector() {
        let text = TextMatch::text("Tipping", true, false).unwrap();
        let similar = find_similar(SHOP, Example::Text(&text)).unwrap().unwrap();
        assert_eq!(
            texts(&similar.matches),
            ["A Light", "Tipping", "Soumission"]
        );
        assert_eq!(similar.selector, "article.pod > h3 > a");
        assert_eq!(
            crate::select(SHOP, None, &similar.selector).unwrap(),
            ["A Light", "Tipping", "Soumission"]
        );
    }

    #[test]
    fn a_variant_class_is_dropped_but_a_distinguishing_one_is_kept() {
        let similar = find_similar(SHOP, Example::Css("article.sale"))
            .unwrap()
            .unwrap();
        assert_eq!(similar.matches.len(), 3);
        let similar = find_similar(SHOP, Example::Css("p.price"))
            .unwrap()
            .unwrap();
        assert_eq!(texts(&similar.matches), ["£51.77", "£53.74", "£50.10"]);
        assert_eq!(similar.selector, "p.price");
    }

    #[test]
    fn an_example_with_a_badge_still_finds_the_plain_items() {
        let page = r#"<ul class="news">
            <li class="live"><h3><span><b>Live</b> <em>Vote count</em></span></h3></li>
            <li><h3><span><em>Storm warning</em></span></h3></li>
            <li><h3><span><em>Markets</em></span></h3></li>
        </ul>"#;
        let text = TextMatch::text("live vote count", true, false).unwrap();
        let similar = find_similar(page, Example::Text(&text)).unwrap().unwrap();
        assert_eq!(
            texts(&similar.matches),
            ["Live Vote count", "Storm warning", "Markets"]
        );
        assert_eq!(similar.selector, "ul.news > li > h3 > span");
    }

    #[test]
    fn an_active_item_finds_its_siblings() {
        let page = r#"<nav><a class="tab">Home</a><a class="tab active">Docs</a><a class="tab">Blog</a></nav>
            <p><a class="more">Read more</a></p>"#;
        let similar = find_similar(page, Example::Css("a.active"))
            .unwrap()
            .unwrap();
        assert_eq!(texts(&similar.matches), ["Home", "Docs", "Blog"]);
        assert_eq!(similar.selector, "a.tab");
    }

    #[test]
    fn a_deep_page_stays_bounded() {
        let page = format!(
            "{}<p class=x>a</p><p class=x>b</p>{}",
            "<div class=d>".repeat(3000),
            "</div>".repeat(3000)
        );
        let text = TextMatch::text("b", true, false).unwrap();
        let similar = find_similar(&page, Example::Text(&text)).unwrap().unwrap();
        assert_eq!(texts(&similar.matches), ["a", "b"]);
    }

    #[test]
    fn similar_from_xpath() {
        let similar = find_similar(SHOP, Example::XPath("//p[@class='label']"))
            .unwrap()
            .unwrap();
        assert_eq!(texts(&similar.matches), ["New", "Sale", "Old"]);
    }

    #[test]
    fn nothing_matches_the_example() {
        assert_eq!(find_similar(SHOP, Example::Css("table")).unwrap(), None);
        assert!(find_similar(SHOP, Example::Css("li[")).is_err());
    }

    #[test]
    fn odd_class_names_are_escaped() {
        let page = r#"<ul><li class="w-1/2 2col md:flex -1">a</li><li class="w-1/2 2col md:flex -1">b</li></ul>"#;
        let similar = find_similar(page, Example::Css("li")).unwrap().unwrap();
        assert_eq!(texts(&similar.matches), ["a", "b"]);
        assert_eq!(
            crate::select(page, None, &similar.selector).unwrap(),
            ["a", "b"]
        );
    }

    #[test]
    fn many_siblings_stay_fast() {
        let page = format!(
            "<ul>{}</ul>",
            (0..100_000)
                .map(|i| format!("<li>item {i}</li>"))
                .collect::<String>()
        );
        let start = std::time::Instant::now();
        let found = find_text(&page, &TextMatch::text("item 99999", true, false).unwrap());
        assert_eq!(
            found[0].selector,
            "html > body > ul > li:nth-of-type(100000)"
        );
        let text = TextMatch::text("item 7", true, false).unwrap();
        let similar = find_similar(&page, Example::Text(&text)).unwrap().unwrap();
        assert_eq!(similar.matches.len(), 100_000);
        assert!(start.elapsed().as_secs() < 30, "{:?}", start.elapsed());
    }

    #[test]
    fn classes_per_step_are_capped() {
        let classes = (0..200)
            .map(|j| format!("c{j}"))
            .collect::<Vec<_>>()
            .join(" ");
        let page = format!(
            "<ul>{}</ul>",
            (0..20)
                .map(|i| format!("<li class=\"{classes} k{i}\">t{i}</li>"))
                .collect::<String>()
        );
        let text = TextMatch::text("t3", true, false).unwrap();
        let similar = find_similar(&page, Example::Text(&text)).unwrap().unwrap();
        assert_eq!(similar.matches.len(), 20);
        for step in similar.selector.split(" > ") {
            assert!(step.matches('.').count() <= MAX_CLASSES, "{step}");
        }
    }

    #[test]
    fn errors_give_the_reason_and_stay_short() {
        let err = TextMatch::regex("a(b", false).unwrap_err().to_string();
        assert!(err.contains("unclosed group"), "{err}");
        let err = TextMatch::regex(&format!("{}(", "a".repeat(400_000)), false)
            .unwrap_err()
            .to_string();
        assert!(err.len() < 400, "{} bytes", err.len());
        let err = find_similar(SHOP, Example::Css(&format!("{}[", "a".repeat(100_000))))
            .unwrap_err()
            .to_string();
        assert!(err.len() < 400, "{} bytes", err.len());
    }

    #[test]
    fn template_content_is_not_searched() {
        let page = "<template><div><p>secret</p></div></template><p>open secret</p>";
        let found = find_text(page, &TextMatch::text("secret", false, false).unwrap());
        assert_eq!(texts(&found), ["open secret"]);
        assert_eq!(
            crate::select(page, None, &found[0].selector).unwrap(),
            ["open secret"]
        );
    }
}
