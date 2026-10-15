//! Markdown with numbered link citations.
use std::collections::HashMap;

use url::Url;

/// Rewrite the inline links and images of `markdown` as numbered citations.
///
/// `[text](url)` becomes `text[^1]` and `![alt](src)` becomes `![alt][^2]`,
/// markdown footnote marks. A `## References` list at the end maps each
/// number to its URL, one `[^1]: url` footnote per line. Numbers follow the
/// order of first appearance, and a URL that comes back anywhere keeps its
/// first number. Links to a spot on the same page (`#section`, or `page_url`
/// with a fragment) and `javascript:` links keep their text and lose the URL.
/// Fenced code blocks and inline code are never rewritten. With nothing to
/// cite, no References list is added.
pub fn to_citations(markdown: &str, page_url: Option<&str>) -> String {
    let mut refs = Refs {
        page: page_url.and_then(|u| Url::parse(u).ok()).map(|mut u| {
            u.set_fragment(None);
            u
        }),
        urls: Vec::new(),
        numbers: HashMap::new(),
    };
    let mut out = String::with_capacity(markdown.len());
    let mut fence: Option<&str> = None;
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
        let inside = fence.is_some();
        match (fence, marker) {
            (None, Some(m)) => fence = Some(m),
            (Some(f), Some(m)) if f == m => fence = None,
            _ => {}
        }
        if inside || marker.is_some() {
            out.push_str(line);
        } else {
            out.push_str(&refs.rewrite(line, false));
        }
    }
    if refs.urls.is_empty() {
        return out;
    }
    out.truncate(out.trim_end().len());
    out.push_str("\n\n## References\n");
    for (i, url) in refs.urls.iter().enumerate() {
        out.push_str(&format!("\n[^{}]: {url}", i + 1));
    }
    out
}

struct Refs {
    page: Option<Url>,
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
                    out.push_str(&self.cite(text, url, image));
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
            self.urls.len()
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

fn starts_with_ignore_case(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
}

#[cfg(test)]
mod tests {
    use super::to_citations;

    #[test]
    fn numbers_links_by_first_appearance_and_dedups() {
        let md = "See [Rust](https://rust-lang.org) and [docs](https://doc.rust-lang.org).\n\nAgain [the site](https://rust-lang.org).";
        assert_eq!(
            to_citations(md, None),
            "See Rust[^1] and docs[^2].\n\nAgain the site[^1].\n\n## References\n\n[^1]: https://rust-lang.org\n[^2]: https://doc.rust-lang.org"
        );
    }

    #[test]
    fn cites_images_and_linked_images() {
        let md =
            "![Logo](https://a.com/l.png)\n\n[![Shot](https://a.com/s.png)](https://a.com/page)";
        assert_eq!(
            to_citations(md, None),
            "![Logo][^1]\n\n![Shot][^2][^3]\n\n## References\n\n[^1]: https://a.com/l.png\n[^2]: https://a.com/s.png\n[^3]: https://a.com/page"
        );
    }

    #[test]
    fn same_page_anchors_and_javascript_keep_text_only() {
        let md = "Jump to [Intro](#intro), note [[1]](https://w.org/wiki/X#cite-1), [run](JavaScript:void(0)), [other](https://w.org/wiki/Y#top).";
        assert_eq!(
            to_citations(md, Some("https://w.org/wiki/X")),
            "Jump to Intro, note [1], run, other[^1].\n\n## References\n\n[^1]: https://w.org/wiki/Y#top"
        );
    }

    #[test]
    fn leaves_code_alone() {
        let md = "Use `[a](b)` here.\n\n```md\n[x](https://x.com)\n```\n\n~~~\n```\n[y](https://y.com)\n~~~\n\n[z](https://z.com)";
        assert_eq!(
            to_citations(md, None),
            "Use `[a](b)` here.\n\n```md\n[x](https://x.com)\n```\n\n~~~\n```\n[y](https://y.com)\n~~~\n\nz[^1]\n\n## References\n\n[^1]: https://z.com"
        );
    }

    #[test]
    fn handles_parens_titles_tables_and_non_links() {
        let md = "| [Rust](https://en.wikipedia.org/wiki/Rust_(language)) | [b](<https://b.com/x> \"Title\") |\n\n[not a link] (x) \\[esc](https://e.com) [open(https://o.com)";
        assert_eq!(
            to_citations(md, None),
            "| Rust[^1] | b[^2] |\n\n[not a link] (x) \\[esc](https://e.com) [open(https://o.com)\n\n## References\n\n[^1]: https://en.wikipedia.org/wiki/Rust_(language)\n[^2]: https://b.com/x"
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
            "Text ([a](https://a.com and more) here) b[^1]\n\n## References\n\n[^1]: https://b.com"
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
