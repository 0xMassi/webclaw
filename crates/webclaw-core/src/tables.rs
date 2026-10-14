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
    /// Set when the table was cut short to keep memory bounded: it holds its
    /// first rows only, and the tables after it on the page are left out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

const MAX_COLSPAN: usize = 1000;
const MAX_ROWSPAN: usize = 65534;
const MAX_SLOTS: usize = 1_000_000;
const MAX_TABLE_BYTES: usize = 32 << 20;
const MAX_PAGE_BYTES: usize = 64 << 20;
const SLOT_BYTES: usize = 32;
const MAX_CELL_BYTES: usize = 64 << 10;
const MAX_PAGE_NODES: usize = 10_000_000;
const EMPTY: u32 = u32::MAX;
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
/// Very large tables are cut short and flagged with [`Table::truncated`],
/// and a cell keeps its first 64 KB of text.
pub fn tables(html: &str) -> Vec<Table> {
    let mut doc = Html::parse_document(html);
    noise::strip_hidden(&mut doc);
    let mut budget = Budget {
        bytes: MAX_PAGE_BYTES,
        nodes: MAX_PAGE_NODES,
    };
    let mut found: Vec<Table> = Vec::new();
    for table in doc.select(&TABLE) {
        if budget.bytes == 0 {
            if let Some(last) = found.last_mut() {
                last.truncated = true;
            }
            break;
        }
        found.extend(data_table(table, &mut budget));
    }
    found
}

struct Budget {
    bytes: usize,
    nodes: usize,
}

struct Row<'a> {
    cells: Vec<ElementRef<'a>>,
    in_head: bool,
    section_end: usize,
}

fn own_rows(table: ElementRef<'_>) -> Vec<Row<'_>> {
    let mut sections: Vec<(&str, Vec<ElementRef<'_>>)> = Vec::new();
    for child in table.children().filter_map(ElementRef::wrap) {
        match child.value().name() {
            "tr" => match sections.last_mut() {
                Some(("tr", trs)) => trs.push(child),
                _ => sections.push(("tr", vec![child])),
            },
            name @ ("thead" | "tbody" | "tfoot") => sections.push((
                name,
                child
                    .children()
                    .filter_map(ElementRef::wrap)
                    .filter(|el| el.value().name() == "tr")
                    .collect(),
            )),
            _ => {}
        }
    }
    sections.sort_by_key(|(name, _)| match *name {
        "thead" => 0,
        "tfoot" => 2,
        _ => 1,
    });
    let mut rows = Vec::new();
    for (name, trs) in sections {
        let section_end = rows.len() + trs.len();
        rows.extend(trs.into_iter().map(|tr| {
            Row {
                cells: tr
                    .children()
                    .filter_map(ElementRef::wrap)
                    .filter(|el| matches!(el.value().name(), "td" | "th"))
                    .collect(),
                in_head: name == "thead",
                section_end,
            }
        }));
    }
    rows
}

fn number(value: Option<&str>) -> Option<usize> {
    let value = value?.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let value = value.strip_prefix('+').unwrap_or(value);
    let digits = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    (digits > 0).then(|| {
        value[..digits].bytes().fold(0usize, |n, b| {
            n.saturating_mul(10).saturating_add(usize::from(b - b'0'))
        })
    })
}

fn colspan(cell: ElementRef<'_>) -> usize {
    number(cell.value().attr("colspan"))
        .filter(|&n| n > 0)
        .map_or(1, |n| n.min(MAX_COLSPAN))
}

fn rowspan(cell: ElementRef<'_>, left: usize) -> usize {
    match number(cell.value().attr("rowspan")) {
        Some(0) => left,
        Some(n) => n.min(MAX_ROWSPAN).min(left),
        None => 1,
    }
}

fn data_table(table: ElementRef<'_>, budget: &mut Budget) -> Option<Table> {
    if matches!(table.value().attr("role"), Some("presentation" | "none")) {
        return None;
    }
    let rows = own_rows(table);
    let mut placed = Vec::new();
    let mut grid: Vec<Vec<u32>> = vec![Vec::new(); rows.len()];
    let mut slots = 0;
    let mut truncated = false;
    'fill: for (r, row) in rows.iter().enumerate() {
        let mut c = 0;
        for &cell in &row.cells {
            while grid[r].get(c).is_some_and(|&id| id != EMPTY) {
                c += 1;
            }
            let colspan = colspan(cell);
            let rowspan = rowspan(cell, row.section_end - r);
            slots += colspan * rowspan;
            if slots > MAX_SLOTS {
                truncated = true;
                break 'fill;
            }
            let id = placed.len() as u32;
            placed.push(cell);
            for line in &mut grid[r..r + rowspan] {
                if line.len() < c + colspan {
                    line.resize(c + colspan, EMPTY);
                }
                line[c..c + colspan].fill(id);
            }
            c += colspan;
        }
    }
    let width = grid.iter().map(Vec::len).max().unwrap_or(0);
    let kept = rows.len().min(MAX_SLOTS / width.max(1));
    truncated |= kept < rows.len();

    let mut start = 0;
    while start < kept
        && width > 1
        && rows[start].cells.len() == 1
        && colspan(rows[start].cells[0]) >= width
    {
        start += 1;
    }
    let mut body = start;
    while body < kept
        && !rows[body].cells.is_empty()
        && (rows[body].in_head || rows[body].cells.iter().all(|c| c.value().name() == "th"))
    {
        body += 1;
    }
    let cells = || rows[start..kept].iter().flat_map(|row| &row.cells);
    if start == body
        && (has_table(table)
            || cells().all(|&cell| links_only(cell))
            || rows[start..kept]
                .iter()
                .filter(|row| row.cells.len() > 1)
                .flat_map(|row| &row.cells)
                .any(|&cell| has_block(cell)))
    {
        return None;
    }

    let mut texts = Vec::with_capacity(placed.len());
    for &cell in &placed {
        match cell_text(cell, &mut budget.nodes) {
            Some(text) => texts.push(text),
            None => {
                budget.bytes = 0;
                truncated = true;
                break;
            }
        }
    }
    let text = |r: usize, c: usize| {
        grid[r]
            .get(c)
            .and_then(|&id| texts.get(id as usize))
            .map_or("", String::as_str)
    };
    let caption = table
        .children()
        .filter_map(ElementRef::wrap)
        .find(|el| el.value().name() == "caption")
        .and_then(|el| cell_text(el, &mut budget.nodes))
        .filter(|t| !t.is_empty())
        .or_else(|| {
            (0..start)
                .map(|r| text(r, 0))
                .find(|t| !t.is_empty())
                .map(str::to_string)
        });
    let body_rows: Vec<usize> = (body..kept)
        .filter(|&r| (0..width).any(|c| !text(r, c).is_empty()))
        .collect();
    let text_columns = (0..width)
        .filter(|&c| body_rows.iter().any(|&r| !text(r, c).is_empty()))
        .count();
    if body_rows.is_empty() || text_columns < 2 || (start == body && body_rows.len() < 2) {
        return None;
    }
    let mut left = MAX_TABLE_BYTES.min(budget.bytes);
    let mut spend = |cells: Vec<String>| {
        let cost: usize = cells.iter().map(|s| s.len() + SLOT_BYTES).sum();
        (cost <= left).then(|| {
            left -= cost;
            cells
        })
    };
    let headers = if start == body {
        Vec::new()
    } else {
        let names = (0..width)
            .map(|c| {
                let mut names: Vec<&str> = Vec::new();
                for r in start..body {
                    if !text(r, c).is_empty() && !names.contains(&text(r, c)) {
                        names.push(text(r, c));
                    }
                }
                names.join(" / ")
            })
            .collect();
        spend(names)?
    };
    let mut out = Vec::new();
    for &r in &body_rows {
        match spend((0..width).map(|c| text(r, c).to_string()).collect()) {
            Some(cells) => out.push(cells),
            None => {
                truncated = true;
                break;
            }
        }
    }
    budget.bytes = if truncated {
        0
    } else {
        budget.bytes - (MAX_TABLE_BYTES.min(budget.bytes) - left)
    };
    (!out.is_empty()).then_some(Table {
        caption,
        headers,
        rows: out,
        truncated,
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

fn cell_text(cell: ElementRef<'_>, nodes: &mut usize) -> Option<String> {
    let mut out = String::new();
    let mut skip = None;
    for edge in cell.traverse() {
        *nodes = nodes.checked_sub(1)?;
        if out.len() > 2 * MAX_CELL_BYTES {
            break;
        }
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
    let mut text = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.len() > MAX_CELL_BYTES {
        let mut end = MAX_CELL_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    Some(text)
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
        assert!(t.truncated);
    }

    fn bytes(t: &Table) -> usize {
        t.headers
            .iter()
            .chain(t.rows.iter().flatten())
            .map(|s| s.len() + SLOT_BYTES)
            .sum()
    }

    #[test]
    fn padding_a_wide_row_stays_within_the_slot_cap() {
        let html = format!(
            "<table><tr>{}</tr>{}<tr><td>a</td><td>b</td></tr></table>",
            r#"<th colspan="1000">h</th>"#.repeat(1000),
            "<tr></tr>".repeat(200)
        );
        assert!(tables(&html).is_empty());
    }

    #[test]
    fn spanned_text_is_cut_at_the_table_budget() {
        let html = format!(
            r#"<table><tr><th>a</th><th>b</th></tr><tr><td colspan="1000" rowspan="600">{}</td><td>b</td></tr>{}</table>"#,
            "x".repeat(1000),
            "<tr><td>c</td></tr>".repeat(599)
        );
        let t = one(&html);
        assert!(t.truncated);
        assert!(bytes(&t) <= MAX_TABLE_BYTES);
        assert!(!t.rows.is_empty());
    }

    #[test]
    fn all_tables_share_one_page_budget() {
        let one_table = format!(
            r#"<table><tr><th>a</th><th>b</th></tr><tr><td colspan="1000" rowspan="20">{}</td><td>b</td></tr>{}</table>"#,
            "x".repeat(1000),
            "<tr><td>c</td></tr>".repeat(19)
        );
        let found = tables(&one_table.repeat(6));
        assert_eq!(found.len(), 4);
        assert!(found[..3].iter().all(|t| !t.truncated));
        assert!(found[3].truncated);
        assert!(found.iter().map(bytes).sum::<usize>() <= MAX_PAGE_BYTES);
    }

    #[test]
    fn a_cell_keeps_its_first_64_kb() {
        let html = format!(
            "<table><tr><th>a</th><th>b</th></tr><tr><td>{}</td><td>1</td></tr></table>",
            "word ".repeat(40_000)
        );
        let t = one(&html);
        assert!(t.rows[0][0].len() <= MAX_CELL_BYTES);
        assert!(t.rows[0][0].starts_with("word word"));
    }

    #[test]
    fn deeply_nested_tables_stop_at_the_work_budget() {
        let depth = 2_000;
        let html = format!(
            "{}{}",
            "<table><tr><th>h</th><th>k</th></tr><tr><td>x".repeat(depth),
            "</td><td>y</td></tr></table>".repeat(depth)
        );
        let found = tables(&html);
        assert!(found.last().unwrap().truncated);
        assert!(found.len() < depth);
    }

    #[test]
    fn spans_parse_like_a_browser() {
        let t = one(r#"<table><tr><th>a</th><th>b</th><th>c</th></tr>
            <tr><td colspan="0">x</td><td>y</td><td>z</td></tr>
            <tr><td colspan="2px">p</td><td>q</td></tr>
            <tr><td colspan=" 2.5">r</td><td>s</td></tr></table>"#);
        assert_eq!(t.headers, ["a", "b", "c"]);
        assert_eq!(
            t.rows,
            strings(&[&["x", "y", "z"], &["p", "p", "q"], &["r", "r", "s"]])
        );
    }

    #[test]
    fn rowspans_stop_at_their_section() {
        let t = one(
            r#"<table><thead><tr><th rowspan="0">A</th><th>B</th></tr><tr><th>B2</th></tr></thead>
            <tbody><tr><td rowspan="3">1</td><td>2</td></tr></tbody>
            <tbody><tr><td>3</td><td>4</td></tr></tbody></table>"#,
        );
        assert_eq!(t.headers, ["A", "B / B2"]);
        assert_eq!(t.rows, strings(&[&["1", "2"], &["3", "4"]]));
    }

    #[test]
    fn tfoot_rows_come_last() {
        let t = one(r#"<table><thead><tr><th>a</th><th>b</th></tr></thead>
            <tfoot><tr><td>sum</td><td>9</td></tr></tfoot>
            <tbody><tr><td>x</td><td>4</td></tr><tr><td>y</td><td>5</td></tr></tbody></table>"#);
        assert_eq!(t.rows, strings(&[&["x", "4"], &["y", "5"], &["sum", "9"]]));
    }

    #[test]
    fn a_full_width_legend_row_does_not_make_a_grid_layout() {
        let t = one(r#"<table><tr><td>x</td><td>A</td><td>B</td></tr>
            <tr><td>A</td><td></td><td>1-0</td></tr><tr><td>B</td><td>0-2</td><td></td></tr>
            <tr><td colspan="3"><div>Legend: home rows</div></td></tr></table>"#);
        assert_eq!(t.rows.len(), 4);
        assert_eq!(t.rows[1], ["A", "", "1-0"]);
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
