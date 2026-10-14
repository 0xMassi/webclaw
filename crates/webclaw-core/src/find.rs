//! Find elements by their text, or every element like an example.
//!
//! Text search looks inside `<body>` only. An element's text is its visible
//! text, descendants included, whitespace collapsed; `script`, `style`,
//! `noscript` and `template` content does not count. A text search returns the deepest elements that match: an element
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
pub struct FoundElement {
    /// A CSS selector that picks this element alone on this page: its path
    /// from the nearest ancestor with an id no other element has, or from
    /// the root. The path keeps its last 64 steps, so on a page nested deeper
    /// than that with no such id it can pick more.
    pub selector: String,
    /// The element's visible text.
    pub text: String,
}

/// Every element like an example, and one CSS selector that picks them all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SimilarSet {
    /// Picks exactly `matches` on this page; reuse it with [`crate::select()`]
    /// or [`crate::select_records()`].
    pub selector: String,
    /// The example and every element like it, in document order.
    pub matches: Vec<FoundElement>,
}

/// Why a search could not run. Echoed input is cut to its first 100
/// characters.
#[derive(Debug, thiserror::Error)]
pub enum FindError {
    /// A CSS selector or XPath expression that does not parse.
    #[error("invalid selector `{0}`")]
    Selector(String),
    /// Text to find that is empty once whitespace is collapsed, or a regular
    /// expression that matches empty text; either would match every element.
    #[error("nothing to find: `{0}` matches every element")]
    Empty(String),
    /// An XPath example that gives a number, string or boolean instead of
    /// nodes.
    #[error("the example must select an element, text or attribute node: `{0}`")]
    NotNodes(String),
    /// Text to find, in characters, too long to search for.
    #[error("the text to find is too long ({0} characters)")]
    TooLong(usize),
    /// A regular expression that does not compile.
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
    /// `text` is collapsed like the page text; empty text is an error.
    pub fn text(text: &str, exact: bool, case_sensitive: bool) -> Result<Self, FindError> {
        let collapsed = collapse(text);
        if collapsed.is_empty() {
            return Err(FindError::Empty(short(text)));
        }
        let escaped = regex::escape(&collapsed);
        let pattern = if exact {
            format!("^{escaped}$")
        } else {
            escaped
        };
        Self::build(&pattern, case_sensitive, text).map_err(|error| match error {
            FindError::Regex { .. } => FindError::TooLong(collapsed.chars().count()),
            other => other,
        })
    }

    /// Text in which the regular expression `pattern` finds a match. A
    /// pattern that matches empty text is an error.
    pub fn regex(pattern: &str, case_sensitive: bool) -> Result<Self, FindError> {
        let built = Self::build(pattern, case_sensitive, pattern)?;
        if built.0.is_match("") {
            return Err(FindError::Empty(short(pattern)));
        }
        Ok(built)
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
pub enum FindExample<'a> {
    /// A CSS selector; a `::text`, `::html` or `::attr(NAME)` ending is
    /// ignored.
    Css(&'a str),
    /// An XPath 1.0 expression; the element of its first node is the
    /// example, so `//a/@href` or `//a/text()` pick the `a`.
    XPath(&'a str),
    /// The first element [`find_text`] returns.
    Text(&'a TextMatch),
}

/// The deepest elements whose text matches, in document order.
pub fn find_text(html: &str, text: &TextMatch) -> Vec<FoundElement> {
    let document = Html::parse_document(html);
    found_all(&document, matching(&document, text))
}

/// Every element like the example. The search starts from the example's path
/// from the root (its last 64 steps), each step its tag and first 32 classes, and
/// widens it while every element that adds shares a child tag with the
/// example (or has no child elements, like it), sits under a parent holding
/// no earlier match, and shares a class with the example (when both have
/// classes), as does its ancestor at the step whose classes were dropped.
/// The class test is skipped for an element whose parent already holds two
/// matches at that step, since it is then another item of a list rather
/// than another part of one item. It cuts steps from the top as long as one step
/// with a class is left, and drops the classes of a step, all at once or one
/// at a time, tried under the whole path and under its shortest tail that
/// starts at a step with a class and picks the same elements; dropped classes
/// that every match carries come back. So a class that only some items carry
/// goes, while one that tells two parts of an item apart stays. The selector
/// returned is that shortest tail. `None` when nothing matches the example.
pub fn find_similar(html: &str, example: FindExample) -> Result<Option<SimilarSet>, FindError> {
    let document = Html::parse_document(html);
    let example = match example {
        FindExample::Css(css) => {
            let selector = crate::select::Query::parse(css)
                .ok()
                .and_then(|query| query.css)
                .ok_or_else(|| FindError::Selector(short(css)))?;
            document.select(&selector).next()
        }
        FindExample::XPath(expression) => {
            crate::xpath::first_element(html, expression)?.and_then(|index| {
                document
                    .root_element()
                    .descendants()
                    .filter_map(ElementRef::wrap)
                    .nth(index)
            })
        }
        FindExample::Text(text) => matching(&document, text).into_iter().next(),
    };
    let Some(example) = example else {
        return Ok(None);
    };
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
            if alike(&matched, &more, example, 0) {
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
                    if more.len() > matched.len()
                        && alike(&matched, &more, example, steps.len() - 1 - level)
                    {
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
    Ok(Some(SimilarSet {
        selector: render(&steps, start),
        matches: found_all(&document, matched),
    }))
}

fn matching<'a>(document: &'a Html, text: &TextMatch) -> Vec<ElementRef<'a>> {
    let mut hits = Vec::new();
    let mut hidden = 0usize;
    let body = document
        .root_element()
        .children()
        .filter_map(ElementRef::wrap)
        .find(|e| e.value().name() == "body")
        .unwrap_or(document.root_element());
    for edge in body.traverse() {
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

fn alike(matched: &[ElementRef], more: &[ElementRef], example: ElementRef, level: usize) -> bool {
    let known: HashSet<NodeId> = matched.iter().map(|e| e.id()).collect();
    let parents: HashSet<Option<NodeId>> = matched
        .iter()
        .filter(|_| matched.len() > 1)
        .map(|e| e.parent().map(|p| p.id()))
        .collect();
    let example_shape = shape(example);
    let example_classes = classes(Some(example));
    let example_step = classes(up(example, level));
    let items = siblings(matched, 0);
    let step_items = siblings(matched, level);
    more.iter().all(|e| {
        let step = up(*e, level);
        let other_part = lone(Some(*e), &items) && apart(&classes(Some(*e)), &example_classes);
        let other_step = lone(step, &step_items) && apart(&classes(step), &example_step);
        known.contains(&e.id())
            || (fits(&shape(*e), &example_shape)
                && !parents.contains(&e.parent().map(|p| p.id()))
                && !other_part
                && !other_step)
    })
}

fn siblings(matched: &[ElementRef], levels: usize) -> HashMap<Option<NodeId>, usize> {
    let mut seen = HashSet::new();
    let mut count = HashMap::new();
    for element in matched.iter().filter_map(|e| up(*e, levels)) {
        if seen.insert(element.id()) {
            *count.entry(element.parent().map(|p| p.id())).or_default() += 1;
        }
    }
    count
}

fn lone(element: Option<ElementRef>, siblings: &HashMap<Option<NodeId>, usize>) -> bool {
    element.is_some_and(|e| {
        siblings
            .get(&e.parent().map(|p| p.id()))
            .copied()
            .unwrap_or(0)
            < 2
    })
}

fn up(element: ElementRef, levels: usize) -> Option<ElementRef> {
    match levels {
        0 => Some(element),
        _ => element
            .ancestors()
            .filter_map(ElementRef::wrap)
            .nth(levels - 1),
    }
}

fn classes<'a>(element: Option<ElementRef<'a>>) -> HashSet<&'a str> {
    element.map_or_else(HashSet::new, |e| e.value().classes().collect())
}

fn apart(classes: &HashSet<&str>, example: &HashSet<&str>) -> bool {
    !classes.is_empty() && !example.is_empty() && classes.is_disjoint(example)
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

fn found_all(document: &Html, elements: Vec<ElementRef>) -> Vec<FoundElement> {
    let mut ids: HashMap<&str, usize> = HashMap::new();
    for element in document
        .root_element()
        .descendants()
        .filter_map(ElementRef::wrap)
    {
        if let Some(id) = element.value().id() {
            *ids.entry(id).or_default() += 1;
        }
    }
    let mut steps: HashMap<NodeId, String> = HashMap::new();
    elements
        .into_iter()
        .map(|element| {
            let mut path = Vec::new();
            let mut node = element;
            while path.len() < MAX_STEPS {
                if let Some(id) = node
                    .value()
                    .id()
                    .filter(|id| !id.is_empty() && ids[id] == 1)
                {
                    path.push(format!("#{}", ident(id)));
                    break;
                }
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
            FoundElement {
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

pub(crate) fn short(text: &str) -> String {
    match text.char_indices().nth(100) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ident(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars == ['-'] {
        return "\\-".into();
    }
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let leading_digit = c.is_ascii_digit() && (i == 0 || (i == 1 && chars[0] == '-'));
        match c {
            '\0' => out.push('\u{FFFD}'),
            c if c.is_ascii_control() || leading_digit => {
                out.push_str(&format!("\\{:x} ", c as u32))
            }
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

    fn texts(found: &[FoundElement]) -> Vec<&str> {
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
        let similar = find_similar(SHOP, FindExample::Text(&text))
            .unwrap()
            .unwrap();
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
        let similar = find_similar(SHOP, FindExample::Css("article.sale"))
            .unwrap()
            .unwrap();
        assert_eq!(similar.matches.len(), 3);
        let similar = find_similar(SHOP, FindExample::Css("p.price"))
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
        let similar = find_similar(page, FindExample::Text(&text))
            .unwrap()
            .unwrap();
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
        let similar = find_similar(page, FindExample::Css("a.active"))
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
        let similar = find_similar(&page, FindExample::Text(&text))
            .unwrap()
            .unwrap();
        assert_eq!(texts(&similar.matches), ["a", "b"]);
    }

    #[test]
    fn similar_from_xpath() {
        let similar = find_similar(SHOP, FindExample::XPath("//p[@class='label']"))
            .unwrap()
            .unwrap();
        assert_eq!(texts(&similar.matches), ["New", "Sale", "Old"]);
    }

    #[test]
    fn nothing_matches_the_example() {
        assert_eq!(find_similar(SHOP, FindExample::Css("table")).unwrap(), None);
        assert!(find_similar(SHOP, FindExample::Css("li[")).is_err());
    }

    #[test]
    fn odd_class_names_are_escaped() {
        let page = r#"<ul><li class="w-1/2 2col md:flex -1">a</li><li class="w-1/2 2col md:flex -1">b</li></ul>"#;
        let similar = find_similar(page, FindExample::Css("li")).unwrap().unwrap();
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
        let similar = find_similar(&page, FindExample::Text(&text))
            .unwrap()
            .unwrap();
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
        let similar = find_similar(&page, FindExample::Text(&text))
            .unwrap()
            .unwrap();
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
        let err = find_similar(SHOP, FindExample::Css(&format!("{}[", "a".repeat(100_000))))
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

    #[test]
    fn only_the_body_is_searched() {
        let page =
            "<html><head><title>Shop page</title></head><body><p>Shop page</p></body></html>";
        let found = find_text(page, &TextMatch::text("shop page", true, false).unwrap());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].selector, "html > body > p");
    }

    #[test]
    fn empty_text_and_empty_matching_regexes_are_refused() {
        assert!(TextMatch::text("", false, false).is_err());
        assert!(TextMatch::text("  \n ", true, false).is_err());
        assert!(TextMatch::regex("x*", false).is_err());
        assert!(TextMatch::regex("^", false).is_err());
        assert!(TextMatch::regex("x+", false).is_ok());
    }

    #[test]
    fn identifiers_are_escaped_the_css_way() {
        for class in [
            "-1", "-2x", "2col", "-", "--x", "a.b", "w-1/2", "日本", "a\"b",
        ] {
            let page = format!(
                r#"<p class="{}">yes</p><p>no</p>"#,
                class.replace('"', "&quot;")
            );
            let css = format!("p.{}", ident(class));
            assert_eq!(
                crate::select(&page, None, &css).unwrap(),
                ["yes"],
                "{class} -> {css}"
            );
        }
    }

    #[test]
    fn an_example_picked_through_a_value_uses_its_element() {
        for example in [
            FindExample::Css("h3 a::attr(href)"),
            FindExample::Css("h3 a::text"),
            FindExample::XPath("//h3/a/@href"),
            FindExample::XPath("//h3/a/text()"),
        ] {
            let similar = find_similar(SHOP, example).unwrap().unwrap();
            assert_eq!(
                texts(&similar.matches),
                ["A Light", "Tipping", "Soumission"],
                "{example:?}"
            );
        }
    }

    #[test]
    fn match_selectors_stay_short() {
        let page = format!(
            "{}{}{}",
            "<div>".repeat(200),
            "<p>x</p>".repeat(50),
            "</div>".repeat(200)
        );
        let found = find_text(&page, &TextMatch::text("x", true, false).unwrap());
        assert_eq!(found.len(), 50);
        assert!(found.iter().all(|f| f.selector.split(" > ").count() <= 64));
        let page = r#"<div id="main"><ul><li>a</li><li>b</li></ul></div><div id="x"></div><div id="x"><p>b</p></div>"#;
        let found = find_text(page, &TextMatch::text("b", true, false).unwrap());
        assert_eq!(found[0].selector, "#main > ul > li:nth-of-type(2)");
        assert!(
            found[1].selector.starts_with("html > body"),
            "{}",
            found[1].selector
        );
        for f in &found {
            assert_eq!(crate::select(page, None, &f.selector).unwrap(), ["b"]);
        }
    }

    #[test]
    fn a_sibling_part_with_other_classes_stays_out() {
        let page = r#"<ol>
            <li><h2><span class="company"><a>Acme</a></span><span class="location"><a>Paris</a></span></h2></li>
            <li><h2><span class="company"><a>Beta</a></span><span class="location"><a>Rome</a></span></h2></li>
            <li><h2><span class="company"><a>Gamma</a></span><span class="location"><a>Oslo</a></span></h2></li>
        </ol>"#;
        let text = TextMatch::text("beta", true, false).unwrap();
        let similar = find_similar(page, FindExample::Text(&text))
            .unwrap()
            .unwrap();
        assert_eq!(texts(&similar.matches), ["Acme", "Beta", "Gamma"]);
    }

    #[test]
    fn items_with_wholly_different_classes_stay_in() {
        let zebra = format!(
            "<ul>{}</ul>",
            (0..10)
                .map(|i| format!("<li class=\"row-{}\"><a>n{i}</a></li>", ["b", "a"][i % 2]))
                .collect::<String>()
        );
        let ends = format!(
            "<ul>{}</ul>",
            (0..10)
                .map(|i| {
                    let class = match i {
                        0 => "first",
                        9 => "last",
                        _ => "mid",
                    };
                    format!("<li class=\"{class}\"><a>n{i}</a></li>")
                })
                .collect::<String>()
        );
        let grid = format!(
            "<div class=\"grid\">{}</div>",
            (0..9)
                .map(|i| {
                    let class = ["card-big", "card-small", "card-ad"][i % 3];
                    format!("<div class=\"{class}\"><h3>n{i}</h3></div>")
                })
                .collect::<String>()
        );
        for (page, items) in [(&zebra, 10), (&ends, 10), (&grid, 9)] {
            for example in ["n3", "n4"] {
                let text = TextMatch::text(example, true, false).unwrap();
                let similar = find_similar(page, FindExample::Text(&text))
                    .unwrap()
                    .unwrap();
                assert_eq!(similar.matches.len(), items, "{example} in {page}");
            }
        }
    }

    #[test]
    fn a_huge_text_says_it_is_too_long() {
        let err = TextMatch::text(&"a".repeat(1_000_000), false, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("too long") && !err.contains("regex"), "{err}");
    }

    #[test]
    fn an_xpath_example_must_select_nodes() {
        let err = find_similar(SHOP, FindExample::XPath("count(//li)"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("element, text or attribute"), "{err}");
        assert_eq!(
            find_similar(SHOP, FindExample::XPath("//table")).unwrap(),
            None
        );
    }
}
