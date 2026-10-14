//! Pick values out of a page with XPath 1.0.
//!
//! The page is parsed by the same HTML parser as [`crate::select()`], so both
//! see the same tree. Elements give their text, descendants included,
//! whitespace collapsed; text nodes give their text the same way and are
//! skipped when only whitespace; attributes give their value, with `href`
//! and `src` resolved against the page URL when one is given. Expressions
//! that return a number, string or boolean give one value. Elements nested
//! deeper than 512 levels are placed in their deepest allowed ancestor, as
//! libxml2 does, so a hostile page cannot exhaust the stack.

use serde_json::{Map, Value};
use sxd_document::Package;
use sxd_document::dom::{Document, Element};
use sxd_xpath::nodeset::Node;
use sxd_xpath::{Context, Factory, XPath};
use url::Url;

use crate::select::SelectError;

/// Every value `expression` picks from `html`, in document order.
pub fn select_xpath(
    html: &str,
    url: Option<&str>,
    expression: &str,
) -> Result<Vec<String>, SelectError> {
    let compiled = compile(expression)?;
    let base = url.and_then(|u| Url::parse(u).ok());
    let package = tree(html);
    let document = package.as_document();
    let context = Context::new();
    let value = compiled
        .evaluate(&context, document.root())
        .map_err(|_| SelectError(expression.into()))?;
    Ok(values(value, base.as_ref()))
}

/// One record per node `expression` matches. Each field expression runs with
/// that node as its context, so it usually starts with `.`, and keeps its
/// first value, or `null`.
pub fn select_xpath_records(
    html: &str,
    url: Option<&str>,
    expression: &str,
    fields: &[(String, String)],
) -> Result<Vec<Map<String, Value>>, SelectError> {
    let compiled = compile(expression)?;
    let fields = fields
        .iter()
        .map(|(name, field)| Ok((name, compile(field)?, field)))
        .collect::<Result<Vec<_>, SelectError>>()?;
    let base = url.and_then(|u| Url::parse(u).ok());
    let package = tree(html);
    let document = package.as_document();
    let context = Context::new();
    let sxd_xpath::Value::Nodeset(matches) = compiled
        .evaluate(&context, document.root())
        .map_err(|_| SelectError(expression.into()))?
    else {
        return Err(SelectError(expression.into()));
    };
    matches
        .document_order()
        .into_iter()
        .map(|node| {
            fields
                .iter()
                .map(|(name, field, source)| {
                    let value = field
                        .evaluate(&context, node)
                        .map_err(|_| SelectError(source.to_string()))?;
                    let first = values(value, base.as_ref()).into_iter().next();
                    Ok((name.to_string(), first.map_or(Value::Null, Value::String)))
                })
                .collect()
        })
        .collect()
}

pub(crate) fn first_element(html: &str, expression: &str) -> Result<Option<usize>, SelectError> {
    let compiled = compile(expression)?;
    let package = tree(html);
    let document = package.as_document();
    let sxd_xpath::Value::Nodeset(nodes) = compiled
        .evaluate(&Context::new(), document.root())
        .map_err(|_| SelectError(expression.into()))?
    else {
        return Ok(None);
    };
    let Some(target) = nodes
        .document_order()
        .into_iter()
        .find_map(|node| node.element().or_else(|| node.parent()?.element()))
    else {
        return Ok(None);
    };
    let mut pending: Vec<Element> = document
        .root()
        .children()
        .into_iter()
        .filter_map(|child| child.element())
        .collect();
    let mut index = 0;
    while let Some(element) = pending.pop() {
        if element == target {
            return Ok(Some(index));
        }
        index += 1;
        pending.extend(
            element
                .children()
                .into_iter()
                .rev()
                .filter_map(|child| child.element()),
        );
    }
    Ok(None)
}

fn compile(expression: &str) -> Result<XPath, SelectError> {
    Factory::new()
        .build(expression)
        .ok()
        .flatten()
        .ok_or_else(|| SelectError(expression.into()))
}

fn values(value: sxd_xpath::Value, base: Option<&Url>) -> Vec<String> {
    match value {
        sxd_xpath::Value::Nodeset(nodes) => nodes
            .document_order()
            .into_iter()
            .filter_map(|node| node_value(node, base))
            .collect(),
        sxd_xpath::Value::Number(0.0) => vec!["0".into()],
        sxd_xpath::Value::Number(n) if n.is_finite() && n.fract() == 0.0 => vec![format!("{n:.0}")],
        sxd_xpath::Value::Number(n) => vec![n.to_string()],
        sxd_xpath::Value::String(s) => vec![s],
        sxd_xpath::Value::Boolean(b) => vec![b.to_string()],
    }
}

fn node_value(node: Node, base: Option<&Url>) -> Option<String> {
    match node {
        Node::Attribute(attribute) => {
            let value = attribute.value();
            Some(match (attribute.name().local_part(), base) {
                ("href" | "src", Some(base)) => base
                    .join(value)
                    .map_or_else(|_| value.to_string(), String::from),
                _ => value.to_string(),
            })
        }
        Node::Text(_) => Some(collapse(&node.string_value())).filter(|t| !t.is_empty()),
        _ => Some(collapse(&node.string_value())),
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

const MAX_DEPTH: usize = 512;

fn tree(html: &str) -> Package {
    let page = scraper::Html::parse_document(html);
    let package = Package::new();
    {
        let document = package.as_document();
        let html_root = page.root_element();
        let root = element(&document, html_root.value());
        for node in page.tree.root().children() {
            match node.value() {
                scraper::Node::Comment(comment) => document
                    .root()
                    .append_child(document.create_comment(&comment.comment)),
                scraper::Node::Element(_) if node.id() == html_root.id() => {
                    document.root().append_child(root)
                }
                _ => {}
            }
        }
        let mut pending: Vec<_> = html_root.children().rev().map(|c| (c, root, 1)).collect();
        while let Some((node, parent, depth)) = pending.pop() {
            match node.value() {
                scraper::Node::Element(source) => {
                    let created = element(&document, source);
                    parent.append_child(created);
                    let (inner, depth) = if depth < MAX_DEPTH {
                        (created, depth + 1)
                    } else {
                        (parent, depth)
                    };
                    pending.extend(node.children().rev().map(|c| (c, inner, depth)));
                }
                scraper::Node::Text(text) => parent.append_child(document.create_text(&text.text)),
                scraper::Node::Comment(comment) => {
                    parent.append_child(document.create_comment(&comment.comment))
                }
                _ => {}
            }
        }
    }
    package
}

fn element<'d>(document: &Document<'d>, source: &scraper::node::Element) -> Element<'d> {
    let created = document.create_element(source.name());
    for (name, value) in source.attrs() {
        created.set_attribute_value(name, value);
    }
    created
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><head><title> Books  </title></head><body><ol>
        <li class="book"><h3><a href="/b/1" title="A Light">A Light...</a></h3><p class="price">£51.77</p></li>
        <li class="book"><h3><a href="/b/2" title="Tipping">Tipping...</a></h3></li>
        <li><a>no href</a></li>
    </ol><p id="intro">Hello <b>big</b>
       world</p><!-- note --></body></html>"#;

    #[test]
    fn elements_give_their_collapsed_text() {
        assert_eq!(
            select_xpath(PAGE, None, "//p[@id='intro']").unwrap(),
            ["Hello big world"]
        );
        assert_eq!(
            select_xpath("<p class=p>£<span>51</span>.77</p>", None, "//p").unwrap(),
            ["£51.77"]
        );
    }

    #[test]
    fn attributes_resolve_links() {
        assert_eq!(
            select_xpath(
                PAGE,
                Some("https://books.example/catalogue/"),
                "//li/h3/a/@href"
            )
            .unwrap(),
            ["https://books.example/b/1", "https://books.example/b/2"]
        );
        assert_eq!(
            select_xpath(PAGE, None, "//h3/a/@title").unwrap(),
            ["A Light", "Tipping"]
        );
    }

    #[test]
    fn whitespace_only_text_nodes_are_skipped() {
        assert_eq!(
            select_xpath(PAGE, None, "//ol/text()").unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            select_xpath(PAGE, None, "//title/text()").unwrap(),
            ["Books"]
        );
    }

    #[test]
    fn numbers_strings_and_booleans_give_one_value() {
        assert_eq!(select_xpath(PAGE, None, "count(//li)").unwrap(), ["3"]);
        assert_eq!(
            select_xpath(PAGE, None, "count(//li) div 2").unwrap(),
            ["1.5"]
        );
        assert_eq!(
            select_xpath(PAGE, None, "normalize-space(//title)").unwrap(),
            ["Books"]
        );
        assert_eq!(select_xpath(PAGE, None, "boolean(//b)").unwrap(), ["true"]);
    }

    #[test]
    fn comments_outside_the_root_element_are_kept_in_order() {
        let page = "<!-- before --><html><body>x</body></html><!-- after -->";
        assert_eq!(
            select_xpath(page, None, "//comment()").unwrap(),
            ["before", "after"]
        );
        assert_eq!(select_xpath(page, None, "//body").unwrap(), ["x"]);
    }

    #[test]
    fn whole_numbers_keep_every_digit() {
        let page = "<span>100000000000000000000</span><i>-3</i>";
        assert_eq!(
            select_xpath(page, None, "number(//span)").unwrap(),
            ["100000000000000000000"]
        );
        assert_eq!(select_xpath(page, None, "number(//i)").unwrap(), ["-3"]);
        assert_eq!(select_xpath(page, None, "-0").unwrap(), ["0"]);
    }

    #[test]
    fn comments_and_odd_attribute_names_survive() {
        assert_eq!(select_xpath(PAGE, None, "//comment()").unwrap(), ["note"]);
        let vue = r##"<div :class="a" @click="go" xlink:href="#x" data-id="7">v</div>"##;
        assert_eq!(select_xpath(vue, None, "//div/@data-id").unwrap(), ["7"]);
        assert_eq!(select_xpath(vue, None, "count(//div/@*)").unwrap(), ["4"]);
    }

    #[test]
    fn records_run_fields_from_each_match() {
        let fields = [
            ("title".to_string(), ".//h3/a/@title".to_string()),
            ("price".to_string(), ".//p[@class='price']".to_string()),
            ("class".to_string(), "@class".to_string()),
        ];
        let records = select_xpath_records(PAGE, None, "//li[@class='book']", &fields).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["title"], "A Light");
        assert_eq!(records[0]["price"], "£51.77");
        assert_eq!(records[0]["class"], "book");
        assert_eq!(records[1]["price"], Value::Null);
    }

    #[test]
    fn a_deeply_nested_page_does_not_overflow_a_small_stack() {
        let depth = 8_000;
        let page = format!("{}x{}", "<div>".repeat(depth), "</div>".repeat(depth));
        let (count, text) = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || {
                (
                    select_xpath(&page, None, "count(//div)").unwrap(),
                    select_xpath(&page, None, "normalize-space(//body)").unwrap(),
                )
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(count, [depth.to_string()]);
        assert_eq!(text, ["x"]);
    }

    #[test]
    fn bad_expressions_are_errors() {
        assert!(select_xpath(PAGE, None, "//li[").is_err());
        assert!(select_xpath(PAGE, None, "").is_err());
        assert!(select_xpath_records(PAGE, None, "count(//li)", &[]).is_err());
        assert!(select_xpath_records(PAGE, None, "//li", &[("x".into(), "@".into())]).is_err());
    }
}
