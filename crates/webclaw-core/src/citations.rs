//! Markdown with numbered link citations.
use std::collections::HashMap;

use url::Url;

/// Rewrite the inline links and images of `markdown` as numbered citations.
///
/// `[text](url)` becomes `text[^1]` and `![alt](src)` becomes `![alt][^2]`,
/// markdown footnote marks. A `## Links` list at the end maps each
/// number to its URL, one `[^1]: url` footnote per line. Numbers follow the
/// order of first appearance, and a URL that comes back anywhere keeps its
/// first number. They start after the highest numeric footnote the page
/// already has, so its own `[^1]` keeps its meaning. Links to a spot on the
/// same page (`#section`, or `page_url` with a fragment) and `javascript:`
/// links keep their text and lose the URL. Fenced code blocks and inline
/// code are never rewritten. With nothing to cite, no list is
/// added.
pub fn to_citations(markdown: &str, page_url: Option<&str>) -> String {
    to_citations_after(markdown, page_url, 0).0
}

/// Like [`to_citations`], with numbers starting above `after` as well.
///
/// Returns the text and the highest number in play: its own marks, the
/// page's footnotes and `after`. Pass that number to the next page when
/// several pages are printed one after the other, so their marks never
/// share a number.
pub fn to_citations_after(markdown: &str, page_url: Option<&str>, after: usize) -> (String, usize) {
    let (lines, open_fence) = split_code(markdown);
    let base = lines
        .iter()
        .filter(|(_, prose)| *prose)
        .map(|(line, _)| highest_footnote(line))
        .max()
        .unwrap_or(0)
        .max(after);
    let mut refs = Refs {
        page: page_url.and_then(|u| Url::parse(u).ok()).map(|mut u| {
            u.set_fragment(None);
            u
        }),
        base,
        urls: Vec::new(),
        numbers: HashMap::new(),
    };
    let mut out = String::with_capacity(markdown.len());
    for (line, prose) in lines {
        if prose {
            out.push_str(&refs.rewrite(line, false));
        } else {
            out.push_str(line);
        }
    }
    if refs.urls.is_empty() {
        return (out, base);
    }
    out.truncate(out.trim_end().len());
    if let Some((c, n)) = open_fence {
        out.push('\n');
        out.push_str(&(c as char).to_string().repeat(n));
    }
    out.push_str("\n\n## Links\n");
    for (i, url) in refs.urls.iter().enumerate() {
        out.push_str(&format!("\n[^{}]: {url}", base + i + 1));
    }
    (out, base + refs.urls.len())
}

type Fence = (u8, usize);

fn split_code(markdown: &str) -> (Vec<(&str, bool)>, Option<Fence>) {
    let mut lines = Vec::new();
    let mut fence: Option<Fence> = None;
    for line in markdown.split_inclusive('\n') {
        let prose = match (fence, fence_of(line)) {
            (Some((c, n)), Some((fc, fnn, info)))
                if fc == c && fnn >= n && info.trim().is_empty() =>
            {
                fence = None;
                false
            }
            (Some(_), _) => false,
            (None, Some((c, n, info))) if c == b'~' || !info.contains('`') => {
                fence = Some((c, n));
                false
            }
            (None, _) => true,
        };
        lines.push((line, prose));
    }
    (lines, fence)
}

fn highest_footnote(line: &str) -> usize {
    line.match_indices("[^")
        .filter_map(|(i, _)| {
            let rest = &line[i + 2..];
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            ((1..=9).contains(&digits) && rest.as_bytes().get(digits) == Some(&b']'))
                .then(|| rest[..digits].parse::<usize>().ok())
                .flatten()
        })
        .max()
        .unwrap_or(0)
}

fn fence_of(line: &str) -> Option<(u8, usize, &str)> {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return None;
    }
    let c = *rest.as_bytes().first()?;
    let n = rest.bytes().take_while(|&b| b == c).count();
    ((c == b'`' || c == b'~') && n >= 3).then(|| (c, n, &rest[n..]))
}

struct Refs {
    page: Option<Url>,
    base: usize,
    urls: Vec<String>,
    numbers: HashMap<String, usize>,
}

impl Refs {
    fn rewrite(&mut self, s: &str, nested: bool) -> String {
        let b = s.as_bytes();
        let code = code_spans(b);
        let pair = pairs(b, &code);
        let mut out = String::with_capacity(s.len());
        let (mut i, mut copied) = (0, 0);
        while i < b.len() {
            match b[i] {
                b'`' if code[i] != NONE => i = code[i],
                b'[' if pair[i] != NONE
                    && b.get(pair[i] + 1) == Some(&b'(')
                    && pair[pair[i] + 1] != NONE =>
                {
                    let (close, end) = (pair[i], pair[pair[i] + 1]);
                    let Some(url) = destination(&s[close + 2..end]) else {
                        i += 1;
                        continue;
                    };
                    let image = i > 0 && b[i - 1] == b'!';
                    out.push_str(&s[copied..if image { i - 1 } else { i }]);
                    let text = &s[i + 1..close];
                    let text = if nested {
                        text.to_string()
                    } else {
                        self.rewrite(text, true)
                    };
                    let cited = self.cite(text, url, image);
                    let line_start = !nested && out.trim_start_matches([' ', '\t']).is_empty();
                    let cited = if line_start { block_safe(cited) } else { cited };
                    out.push_str(&cited);
                    let next = b.get(end + 1);
                    if cited.ends_with(']') && next == Some(&b'(')
                        || line_start && cited.starts_with("[^") && next == Some(&b':')
                    {
                        out.push('\\');
                    }
                    i = end + 1;
                    copied = i;
                }
                _ => i += 1,
            }
        }
        out.push_str(&s[copied..]);
        out
    }

    fn cite(&mut self, text: String, url: &str, image: bool) -> String {
        if url.is_empty() || self.same_page(url) || starts_with_ignore_case(url, "javascript:") {
            return text;
        }
        let n = *self.numbers.entry(url.to_string()).or_insert_with(|| {
            self.urls.push(url.to_string());
            self.base + self.urls.len()
        });
        if image {
            format!("![{text}][^{n}]")
        } else {
            format!("{text}[^{n}]")
        }
    }

    fn same_page(&self, url: &str) -> bool {
        if url.starts_with('#') {
            return true;
        }
        let (Some(page), Ok(mut target)) = (&self.page, Url::parse(url)) else {
            return false;
        };
        if target.fragment().is_none() {
            return false;
        }
        target.set_fragment(None);
        &target == page
    }
}

const NONE: usize = usize::MAX;

fn code_spans(b: &[u8]) -> Vec<usize> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let len = b[i..].iter().take_while(|&&c| c == b'`').count();
        if len > 0 {
            runs.push((i, len));
        }
        i += len.max(1);
    }
    let mut next = vec![NONE; runs.len()];
    let mut seen: HashMap<usize, usize> = HashMap::new();
    for r in (0..runs.len()).rev() {
        if let Some(&n) = seen.get(&runs[r].1) {
            next[r] = n;
        }
        seen.insert(runs[r].1, r);
    }
    let mut end = vec![NONE; b.len()];
    let mut r = 0;
    while r < runs.len() {
        match next[r] {
            NONE => r += 1,
            n => {
                end[runs[r].0] = runs[n].0 + runs[n].1;
                r = n + 1;
            }
        }
    }
    end
}

fn pairs(b: &[u8], code: &[usize]) -> Vec<usize> {
    let mut pair = vec![NONE; b.len()];
    let (mut brackets, mut parens) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < b.len() {
        if code[i] != NONE {
            i = code[i];
            continue;
        }
        match b[i] {
            b'\\' => i += 1,
            b'[' => brackets.push(i),
            b'(' => parens.push(i),
            b']' => {
                if let Some(open) = brackets.pop() {
                    pair[open] = i;
                }
            }
            b')' => {
                if let Some(open) = parens.pop() {
                    pair[open] = i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    pair
}

fn destination(dest: &str) -> Option<&str> {
    let dest = dest.trim();
    let (url, rest) = match dest.strip_prefix('<') {
        Some(inner) => {
            let end = inner.find('>')?;
            (&inner[..end], &inner[end + 1..])
        }
        None => dest.split_at(dest.find(char::is_whitespace).unwrap_or(dest.len())),
    };
    let rest = rest.trim().as_bytes();
    let titled = rest.is_empty()
        || (rest.len() >= 2
            && matches!(
                (rest[0], rest[rest.len() - 1]),
                (b'"', b'"') | (b'\'', b'\'') | (b'(', b')')
            ));
    titled.then_some(url)
}

fn block_safe(s: String) -> String {
    let b = s.as_bytes();
    let spaced = |i: usize| matches!(b.get(i), None | Some(b' ' | b'\t'));
    let hashes = b.iter().take_while(|&&c| c == b'#').count();
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    let at = if (1..=6).contains(&hashes) && spaced(hashes)
        || b.first() == Some(&b'>')
        || matches!(b.first(), Some(b'-' | b'+' | b'*')) && spaced(1)
    {
        0
    } else if (1..=9).contains(&digits)
        && matches!(b.get(digits), Some(b'.' | b')'))
        && spaced(digits + 1)
    {
        digits
    } else {
        return s;
    };
    format!("{}\\{}", &s[..at], &s[at..])
}

fn starts_with_ignore_case(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
}

#[cfg(test)]
mod tests {
    use super::{to_citations, to_citations_after};

    #[test]
    fn pages_printed_together_continue_the_numbers() {
        let (first, last) = to_citations_after("[a](https://a.com)", None, 0);
        assert_eq!(
            (first.as_str(), last),
            ("a[^1]\n\n## Links\n\n[^1]: https://a.com", 1)
        );
        let (second, last) =
            to_citations_after("x[^4] [b](https://b.com) [a](https://a.com)", None, last);
        assert_eq!(
            (second.as_str(), last),
            (
                "x[^4] b[^5] a[^6]\n\n## Links\n\n[^5]: https://b.com\n[^6]: https://a.com",
                6
            )
        );
        assert_eq!(
            to_citations_after("no links", None, 6),
            ("no links".to_string(), 6)
        );
    }

    #[test]
    fn numbers_links_by_first_appearance_and_dedups() {
        let md = "See [Rust](https://rust-lang.org) and [docs](https://doc.rust-lang.org).\n\nAgain [the site](https://rust-lang.org).";
        assert_eq!(
            to_citations(md, None),
            "See Rust[^1] and docs[^2].\n\nAgain the site[^1].\n\n## Links\n\n[^1]: https://rust-lang.org\n[^2]: https://doc.rust-lang.org"
        );
    }

    #[test]
    fn cites_images_and_linked_images() {
        let md =
            "![Logo](https://a.com/l.png)\n\n[![Shot](https://a.com/s.png)](https://a.com/page)";
        assert_eq!(
            to_citations(md, None),
            "![Logo][^1]\n\n![Shot][^2][^3]\n\n## Links\n\n[^1]: https://a.com/l.png\n[^2]: https://a.com/s.png\n[^3]: https://a.com/page"
        );
    }

    #[test]
    fn same_page_anchors_and_javascript_keep_text_only() {
        let md = "Jump to [Intro](#intro), note [[1]](https://w.org/wiki/X#cite-1), [run](JavaScript:void(0)), [other](https://w.org/wiki/Y#top).";
        assert_eq!(
            to_citations(md, Some("https://w.org/wiki/X")),
            "Jump to Intro, note [1], run, other[^1].\n\n## Links\n\n[^1]: https://w.org/wiki/Y#top"
        );
    }

    #[test]
    fn leaves_code_alone() {
        let md = "Use `[a](b)` here.\n\n```md\n[x](https://x.com)\n```\n\n~~~\n```\n[y](https://y.com)\n~~~\n\n[z](https://z.com)";
        assert_eq!(
            to_citations(md, None),
            "Use `[a](b)` here.\n\n```md\n[x](https://x.com)\n```\n\n~~~\n```\n[y](https://y.com)\n~~~\n\nz[^1]\n\n## Links\n\n[^1]: https://z.com"
        );
    }

    #[test]
    fn fences_follow_commonmark() {
        let md = "````\n```\n[in](https://in.com)\n```\n[in2](https://in2.com)\n````\n  ~~~\n[t](https://t.com)\n  ~~~~\n```inline``` and [a](https://a.com)";
        assert_eq!(
            to_citations(md, None),
            "````\n```\n[in](https://in.com)\n```\n[in2](https://in2.com)\n````\n  ~~~\n[t](https://t.com)\n  ~~~~\n```inline``` and a[^1]\n\n## Links\n\n[^1]: https://a.com"
        );
    }

    #[test]
    fn unclosed_fence_is_closed_before_the_list() {
        let md = "[a](https://a.com)\n\n~~~~\n[b](https://b.com)\n~~~";
        assert_eq!(
            to_citations(md, None),
            "a[^1]\n\n~~~~\n[b](https://b.com)\n~~~\n~~~~\n\n## Links\n\n[^1]: https://a.com"
        );
    }

    #[test]
    fn paren_after_a_mark_is_escaped() {
        let md = "[docs](https://d.com)(PDF), ![i](https://i.com/i.png)(x), [[1]](#n)(y)";
        assert_eq!(
            to_citations(md, None),
            "docs[^1]\\(PDF), ![i][^2]\\(x), [1]\\(y)\n\n## Links\n\n[^1]: https://d.com\n[^2]: https://i.com/i.png"
        );
    }

    #[test]
    fn marks_at_line_start_make_no_new_blocks() {
        let md = "[](https://e.com): starts\n[## Title](https://t.com)\n  [> q](https://q.com)\n[1. One](https://o.com)\n[- item](https://i.com)\nmid [## x](https://x.com) [2024 results](https://r.com)";
        assert_eq!(
            to_citations(md, None),
            "[^1]\\: starts\n\\## Title[^2]\n  \\> q[^3]\n1\\. One[^4]\n\\- item[^5]\nmid ## x[^6] 2024 results[^7]\n\n## Links\n\n[^1]: https://e.com\n[^2]: https://t.com\n[^3]: https://q.com\n[^4]: https://o.com\n[^5]: https://i.com\n[^6]: https://x.com\n[^7]: https://r.com"
        );
    }

    #[test]
    fn numbers_start_after_the_page_own_footnotes() {
        let md = "Page note[^1] and [^x] [^18446744073709551615] and [link](https://l.com).\n\n[^1]: The page own footnote.\n\n```\n[^9]\n```";
        assert_eq!(
            to_citations(md, None),
            "Page note[^1] and [^x] [^18446744073709551615] and link[^2].\n\n[^1]: The page own footnote.\n\n```\n[^9]\n```\n\n## Links\n\n[^2]: https://l.com"
        );
    }

    #[test]
    fn handles_parens_titles_tables_and_non_links() {
        let md = "| [Rust](https://en.wikipedia.org/wiki/Rust_(language)) | [b](<https://b.com/x> \"Title\") |\n\n[not a link] (x) \\[esc](https://e.com) [open(https://o.com)";
        assert_eq!(
            to_citations(md, None),
            "| Rust[^1] | b[^2] |\n\n[not a link] (x) \\[esc](https://e.com) [open(https://o.com)\n\n## Links\n\n[^1]: https://en.wikipedia.org/wiki/Rust_(language)\n[^2]: https://b.com/x"
        );
    }

    #[test]
    fn no_links_returns_markdown_unchanged() {
        let md = "# Title\n\nPlain text with [brackets] and `code`.\n";
        assert_eq!(to_citations(md, None), md);
    }

    #[test]
    fn many_links_keep_order_and_unicode_text() {
        let md: String = (1..=300)
            .map(|i| format!("[café {i}](https://e.com/{i}) "))
            .collect();
        let out = to_citations(&md, None);
        assert!(out.starts_with("café 1[^1] café 2[^2]"));
        assert!(out.contains("café 300[^300]"));
        assert!(out.ends_with("[^299]: https://e.com/299\n[^300]: https://e.com/300"));
    }

    #[test]
    fn spaced_destination_is_not_a_link() {
        let md = "Text ([a](https://a.com and more) here) [b](https://b.com)";
        assert_eq!(
            to_citations(md, None),
            "Text ([a](https://a.com and more) here) b[^1]\n\n## Links\n\n[^1]: https://b.com"
        );
    }

    #[test]
    fn pathological_input_stays_linear() {
        let md = format!(
            "{}{}{}",
            "[".repeat(100_000),
            "[a](".repeat(100_000),
            (1..2000)
                .map(|n| "`".repeat(n % 50 + 1) + " ")
                .collect::<String>()
        );
        let started = std::time::Instant::now();
        assert_eq!(to_citations(&md, None), md);
        let descending: String = (1001..=2000).rev().map(|n| "`".repeat(n) + "a").collect();
        let descending = format!("x {descending} [x](https://x.com)");
        assert!(to_citations(&descending, None).contains("x[^1]"));
        assert!(started.elapsed().as_secs() < 2);
    }
}
