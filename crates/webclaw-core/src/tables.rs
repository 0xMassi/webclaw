//! Data tables as rows of cell text.
//!
//! Every `<table>` on the page is laid out on a grid the way a browser does,
//! so a cell spanning several rows or columns fills each slot it covers.
//! Tables used for page layout are skipped: those marked
//! `role="presentation"`, those with fewer than two columns of text, and
//! those without a header row that wrap another table, hold blocks such as
//! paragraphs or lists, hold nothing but links, or have a single body row.

use ego_tree::iter::Edge;
use scraper::node::Node;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

use crate::noise;

/// One data table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    /// The `<caption>`, or a title row spanning the whole table above the headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    /// One name per column, empty when the table has no header row. Stacked
    /// header rows give each column its distinct names joined with ` / `.
    pub headers: Vec<String>,
    /// Body rows, each as wide as the table. Empty rows are left out.
    pub rows: Vec<Vec<String>>,
}

const MAX_COLSPAN: usize = 1000;
const MAX_ROWSPAN: usize = 65534;
const MAX_SLOTS: usize = 1_000_000;
const SKIP: &[&str] = &["script", "style", "noscript", "template"];
const BLOCK: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "dd",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
];

static TABLE: LazyLock<Selector> = LazyLock::new(|| Selector::parse("table").unwrap());

/// Every data table in `html`, in document order. Hidden tables and cells
/// are left out, the way [`crate::page_links`] leaves out hidden links.
pub fn tables(html: &str) -> Vec<Table> {
    let mut doc = Html::parse_document(html);
    noise::strip_hidden(&mut doc);
    doc.select(&TABLE).filter_map(data_table).collect()
}

struct Row<'a> {
    cells: Vec<ElementRef<'a>>,
    in_head: bool,
}

fn own_rows(table: ElementRef<'_>) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    for child in table.children().filter_map(ElementRef::wrap) {
        let name = child.value().name();
        let trs: Vec<ElementRef<'_>> = match name {
            "tr" => vec![child],
            "thead" | "tbody" | "tfoot" => child
                .children()
                .filter_map(ElementRef::wrap)
                .filter(|el| el.value().name() == "tr")
                .collect(),
            _ => continue,
        };
        rows.extend(trs.into_iter().map(|tr| {
            Row {
                cells: tr
                    .children()
                    .filter_map(ElementRef::wrap)
                    .filter(|el| matches!(el.value().name(), "td" | "th"))
                    .collect(),
                in_head: name == "thead",
            }
        }));
    }
    rows
}

fn span(cell: ElementRef<'_>, name: &str, limit: usize) -> usize {
    match cell.value().attr(name).map(|v| v.trim().parse::<usize>()) {
        Some(Ok(0)) => limit,
        Some(Ok(n)) => n.clamp(1, limit),
        _ => 1,
    }
}

fn data_table(table: ElementRef<'_>) -> Option<Table> {
    if matches!(table.value().attr("role"), Some("presentation" | "none")) {
        return None;
    }
    let rows = own_rows(table);
    let mut grid: Vec<Vec<Option<String>>> = vec![Vec::new(); rows.len()];
    let mut slots = 0;
    'fill: for (r, row) in rows.iter().enumerate() {
        let mut c = 0;
        for &cell in &row.cells {
            while grid[r].get(c).is_some_and(Option::is_some) {
                c += 1;
            }
            let colspan = span(cell, "colspan", MAX_COLSPAN);
            let rowspan = span(cell, "rowspan", MAX_ROWSPAN.min(rows.len() - r));
            slots += colspan * rowspan;
            if slots > MAX_SLOTS {
                break 'fill;
            }
            let text = cell_text(cell);
            for line in &mut grid[r..r + rowspan] {
                if line.len() < c + colspan {
                    line.resize(c + colspan, None);
                }
                for slot in &mut line[c..c + colspan] {
                    *slot = Some(text.clone());
                }
            }
            c += colspan;
        }
    }
    let width = grid.iter().map(Vec::len).max().unwrap_or(0);
    let grid: Vec<Vec<String>> = grid
        .into_iter()
        .map(|line| {
            let mut line: Vec<String> = line.into_iter().map(Option::unwrap_or_default).collect();
            line.resize(width, String::new());
            line
        })
        .collect();

    let mut caption = table
        .children()
        .filter_map(ElementRef::wrap)
        .find(|el| el.value().name() == "caption")
        .map(cell_text)
        .filter(|t| !t.is_empty());
    let mut start = 0;
    while start < rows.len()
        && width > 1
        && rows[start].cells.len() == 1
        && span(rows[start].cells[0], "colspan", MAX_COLSPAN) >= width
    {
        if caption.is_none() {
            caption = Some(grid[start][0].clone()).filter(|t| !t.is_empty());
        }
        start += 1;
    }
    let mut body = start;
    while body < rows.len()
        && !rows[body].cells.is_empty()
        && (rows[body].in_head || rows[body].cells.iter().all(|c| c.value().name() == "th"))
    {
        body += 1;
    }
    let head = &grid[start..body];
    let body_rows: Vec<Vec<String>> = grid[body..]
        .iter()
        .filter(|line| line.iter().any(|t| !t.is_empty()))
        .cloned()
        .collect();
    let text_columns = (0..width)
        .filter(|&c| body_rows.iter().any(|line| !line[c].is_empty()))
        .count();
    if body_rows.is_empty() || text_columns < 2 {
        return None;
    }
    if head.is_empty()
        && (body_rows.len() < 2
            || has_table(table)
            || rows[start..]
                .iter()
                .flat_map(|row| &row.cells)
                .all(|&cell| links_only(cell))
            || rows[start..]
                .iter()
                .flat_map(|row| &row.cells)
                .any(|&cell| has_block(cell)))
    {
        return None;
    }
    let headers = if head.is_empty() {
        Vec::new()
    } else {
        (0..width)
            .map(|c| {
                let mut names: Vec<&str> = Vec::new();
                for line in head {
                    if !line[c].is_empty() && !names.contains(&line[c].as_str()) {
                        names.push(&line[c]);
                    }
                }
                names.join(" / ")
            })
            .collect()
    };
    Some(Table {
        caption,
        headers,
        rows: body_rows,
    })
}

fn has_table(table: ElementRef<'_>) -> bool {
    table
        .descendants()
        .skip(1)
        .filter_map(ElementRef::wrap)
        .any(|el| el.value().name() == "table")
}

fn links_only(cell: ElementRef<'_>) -> bool {
    cell.descendants().all(|node| match node.value() {
        Node::Text(text) if !text.trim().is_empty() => node
            .ancestors()
            .take_while(|a| a.id() != cell.id())
            .any(|a| a.value().as_element().is_some_and(|el| el.name() == "a")),
        _ => true,
    })
}

fn has_block(cell: ElementRef<'_>) -> bool {
    cell.descendants()
        .filter_map(ElementRef::wrap)
        .any(|el| BLOCK.contains(&el.value().name()))
}

fn cell_text(cell: ElementRef<'_>) -> String {
    let mut out = String::new();
    let mut skip = None;
    for edge in cell.traverse() {
        match (edge, skip) {
            (Edge::Open(node), None) => match node.value() {
                Node::Text(text) => out.push_str(text),
                Node::Element(el) if SKIP.contains(&el.name()) => skip = Some(node.id()),
                Node::Element(el) if el.name() == "br" || BLOCK.contains(&el.name()) => {
                    out.push(' ')
                }
                _ => {}
            },
            (Edge::Close(node), Some(id)) if node.id() == id => skip = None,
            (Edge::Close(node), None) => {
                if let Node::Element(el) = node.value()
                    && BLOCK.contains(&el.name())
                {
                    out.push(' ');
                }
            }
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(html: &str) -> Table {
        let mut found = tables(html);
        assert_eq!(found.len(), 1, "{found:?}");
        found.remove(0)
    }

    fn strings(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|row| row.iter().map(|s| s.to_string()).collect())
            .collect()
    }

    #[test]
    fn header_row_and_body() {
        let t = one(r#"<table><caption> Prices </caption>
            <thead><tr><th>Item</th><th>Price</th></tr></thead>
            <tbody><tr><td>Tea</td><td>£2<br>per cup</td></tr><tr><td>Cake</td><td><b>£3</b></td></tr></tbody>
            </table>"#);
        assert_eq!(t.caption.as_deref(), Some("Prices"));
        assert_eq!(t.headers, ["Item", "Price"]);
        assert_eq!(t.rows, strings(&[&["Tea", "£2 per cup"], &["Cake", "£3"]]));
    }

    #[test]
    fn spans_fill_every_slot() {
        let t = one(r#"<table>
            <tr><th rowspan="2">Country</th><th colspan="2">Population</th></tr>
            <tr><th>Total</th><th>Share</th></tr>
            <tr><td rowspan="2">A</td><td>10</td><td>1%</td></tr>
            <tr><td colspan="2">n/a</td></tr>
            <tr><td>B</td><td>20</td></tr>
            </table>"#);
        assert_eq!(
            t.headers,
            ["Country", "Population / Total", "Population / Share"]
        );
        assert_eq!(
            t.rows,
            strings(&[&["A", "10", "1%"], &["A", "n/a", "n/a"], &["B", "20", ""]])
        );
    }

    #[test]
    fn title_row_becomes_caption() {
        let t = one(r#"<table><tr><th colspan="2">Results</th></tr>
            <tr><th>Team</th><th>Points</th></tr><tr><td>Reds</td><td>3</td></tr></table>"#);
        assert_eq!(t.caption.as_deref(), Some("Results"));
        assert_eq!(t.headers, ["Team", "Points"]);
        assert_eq!(t.rows, strings(&[&["Reds", "3"]]));
    }

    #[test]
    fn headerless_grid_is_data() {
        let t = one("<table><tr><td>a</td><td>1</td></tr><tr><td>b</td><td>2</td></tr></table>");
        assert!(t.headers.is_empty());
        assert_eq!(t.rows, strings(&[&["a", "1"], &["b", "2"]]));
    }

    #[test]
    fn layout_tables_are_skipped() {
        assert!(tables(r#"<table role="presentation"><tr><th>a</th><th>b</th></tr><tr><td>1</td><td>2</td></tr></table>"#).is_empty());
        assert!(tables("<table><tr><td><p>Intro</p></td><td><div>Side</div></td></tr><tr><td>x</td><td>y</td></tr></table>").is_empty());
        assert!(
            tables("<table><tr><td>only one cell</td></tr><tr><td>row</td></tr></table>")
                .is_empty()
        );
        assert!(tables(r#"<table><tr><td><a href="/p">Prev</a></td><td><a href="/n">Next</a></td></tr><tr><td><a href="/">Home</a></td><td></td></tr></table>"#).is_empty());
        assert!(tables(r#"<table><tr><td><img src="dot.gif"></td><td><a href="/a">A</a></td></tr><tr><td><img src="dot.gif"></td><td><a href="/b">B</a></td></tr></table>"#).is_empty());
    }

    #[test]
    fn wrapper_with_a_header_row_is_kept() {
        let found = tables(
            r#"<table><tr><th>Series</th><th>Price</th></tr>
            <tr><td><table><tr><td></td><td>Gasoline</td></tr></table></td><td>4.2</td></tr>
            <tr><td><table><tr><td></td><td>Diesel</td></tr></table></td><td>5.1</td></tr></table>"#,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].headers, ["Series", "Price"]);
        assert_eq!(
            found[0].rows,
            strings(&[&["Gasoline", "4.2"], &["Diesel", "5.1"]])
        );
    }

    #[test]
    fn nested_table_is_kept_and_its_wrapper_skipped() {
        let found = tables(
            r#"<table><tr><td>Menu</td><td>
            <table><tr><th>k</th><th>v</th></tr><tr><td>x</td><td>1</td></tr></table>
            </td></tr><tr><td>a</td><td>b</td></tr></table>"#,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].headers, ["k", "v"]);
        assert_eq!(found[0].rows, strings(&[&["x", "1"]]));
    }

    #[test]
    fn hidden_and_script_text_left_out() {
        let t = one(r#"<table><tr><th>n</th><th>v</th></tr>
            <tr><td><span style="display:none">0001</span>One</td><td>1<script>x()</script></td></tr>
            <tr style="display:none"><td>gone</td><td>0</td></tr></table>"#);
        assert_eq!(t.rows, strings(&[&["One", "1"]]));
    }

    #[test]
    fn huge_spans_stay_bounded() {
        let mut html = String::from("<table><tr><th>a</th><th>b</th></tr>");
        for _ in 0..3000 {
            html.push_str(r#"<tr><td colspan="1000">x</td><td>y</td></tr>"#);
        }
        html.push_str("</table>");
        let t = one(&html);
        assert_eq!(t.headers.len(), 1001);
        assert!(t.rows.len() * 1001 <= MAX_SLOTS);
    }

    #[test]
    fn junk_spans_count_as_one() {
        let t = one(
            r#"<table><tr><th colspan="x">a</th><th rowspan="-2">b</th></tr><tr><td>1</td><td>2</td></tr></table>"#,
        );
        assert_eq!(t.headers, ["a", "b"]);
        assert_eq!(t.rows, strings(&[&["1", "2"]]));
    }
}
