//! Split extracted markdown into chunks for retrieval.
//!
//! Chunks are packed from whole blocks (paragraphs, lists, tables, code
//! fences) and stay under a token budget. A block larger than the budget is
//! cut at line breaks, then sentence ends, then spaces: a cut code fence is
//! reopened in every piece and a cut table repeats its header rows. Each
//! chunk records the headings it sits under.

use std::ops::Range;

use serde::Serialize;

use crate::focus::{Block, split_blocks};

/// One retrieval unit cut from a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Chunk {
    /// The page the chunk came from, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Position of the chunk in its page, from 0.
    pub index: usize,
    /// Headings the chunk sits under, outermost first.
    pub heading_path: Vec<String>,
    /// Approximate token count of `text`, from [`approx_tokens`].
    pub tokens: usize,
    /// The chunk's markdown.
    pub text: String,
}

#[derive(Clone)]
struct Part {
    text: String,
    block: usize,
    joiner: &'static str,
}

struct Piece {
    part: Part,
    tokens: usize,
    heading: bool,
    path: Vec<usize>,
}

/// Rough token count, without a tokenizer: one token per six ASCII letters
/// of a word, per two other letters, per three digits and per two symbols in
/// a row, and one per Chinese, Japanese or Korean character. On web markdown
/// it lands within about 20% of common BPE tokenizers for nine chunks in ten.
pub fn approx_tokens(s: &str) -> usize {
    #[derive(Clone, Copy, PartialEq)]
    enum Run {
        Space,
        Letter,
        Digit,
        Symbol,
    }
    let count = |run, len: usize, wide: usize| match run {
        Run::Space => 0,
        Run::Letter => (len - wide).div_ceil(6) + wide.div_ceil(2),
        Run::Digit => len.div_ceil(3),
        Run::Symbol => len.div_ceil(2),
    };
    let (mut tokens, mut run, mut len, mut wide) = (0, Run::Space, 0, 0);
    for c in s.chars() {
        let kind = if is_cjk(c) {
            tokens += 1;
            Run::Space
        } else if c.is_whitespace() {
            Run::Space
        } else if c.is_alphabetic() {
            Run::Letter
        } else if c.is_numeric() {
            Run::Digit
        } else {
            Run::Symbol
        };
        if kind != run {
            tokens += count(run, len, wide);
            (run, len, wide) = (kind, 0, 0);
        }
        len += 1;
        if kind == Run::Letter && !c.is_ascii() {
            wide += 1;
        }
    }
    tokens + count(run, len, wide)
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3040}'..='\u{30ff}' | '\u{ac00}'..='\u{d7af}' | '\u{3400}'..='\u{4dbf}')
}

/// `markdown` cut into chunks of at most about `max_tokens` tokens. With
/// `overlap` above 0, each chunk after the first starts with up to that many
/// tokens from the end of the one before (capped at half of `max_tokens`).
pub fn chunk(markdown: &str, max_tokens: usize, overlap: usize) -> Vec<Chunk> {
    let max = max_tokens.max(1);
    let overlap = overlap.min(max / 2);
    let budget = max - overlap;
    let blocks = split_blocks(markdown);
    let mut pieces = Vec::new();
    let mut lead = 0;
    for (i, block) in blocks.iter().enumerate() {
        let path = heading_path(&blocks, i);
        let room = if block.level == 0 {
            budget.saturating_sub(lead).max(budget / 2).max(1)
        } else {
            budget
        };
        lead = if block.level > 0 {
            lead + approx_tokens(block.text)
        } else {
            0
        };
        for (text, joiner) in split_block(block.text, room) {
            pieces.push(Piece {
                tokens: approx_tokens(&text),
                part: Part {
                    text,
                    block: i,
                    joiner,
                },
                heading: block.level > 0,
                path: path.clone(),
            });
        }
    }

    let mut groups: Vec<(Vec<Part>, Vec<usize>)> = Vec::new();
    let mut carry = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut size = 0;
    for p in 0..pieces.len() {
        if !current.is_empty() && size + pieces[p].tokens > max {
            let mut next = Vec::new();
            let mut moved = 0;
            while !pieces[p].heading
                && let Some(&i) = current.last()
                && pieces[i].heading
                && moved + pieces[i].tokens + pieces[p].tokens <= max
            {
                moved += pieces[i].tokens;
                next.push(i);
                current.pop();
            }
            next.reverse();
            let tail = overlap_tail(&pieces, &current, overlap);
            if !current.is_empty() {
                groups.push((std::mem::take(&mut carry), std::mem::take(&mut current)));
            }
            current = next;
            size = moved;
            let tail_size = approx_tokens(&join(&tail));
            if size + tail_size + pieces[p].tokens <= max {
                size += tail_size;
                carry = tail;
            }
        }
        current.push(p);
        size += pieces[p].tokens;
    }
    if !current.is_empty() {
        groups.push((carry, current));
    }

    groups
        .into_iter()
        .enumerate()
        .map(|(index, (mut parts, members))| {
            parts.extend(members.iter().map(|&i| pieces[i].part.clone()));
            let text = join(&parts);
            Chunk {
                url: None,
                index,
                heading_path: pieces[members[0]]
                    .path
                    .iter()
                    .map(|&h| heading_text(blocks[h].text))
                    .collect(),
                tokens: approx_tokens(&text),
                text,
            }
        })
        .collect()
}

fn heading_path(blocks: &[Block<'_>], i: usize) -> Vec<usize> {
    let mut path = Vec::new();
    if blocks[i].level > 0 {
        path.push(i);
    }
    let mut parent = blocks[i].heading;
    while let Some(h) = parent {
        path.push(h);
        parent = blocks[h].heading;
    }
    path.reverse();
    path
}

fn heading_text(text: &str) -> String {
    crate::markdown::strip_markdown(text.trim_start_matches('#').trim())
        .trim()
        .to_string()
}

fn join(parts: &[Part]) -> String {
    let mut out = String::new();
    for (k, part) in parts.iter().enumerate() {
        if k > 0 {
            out.push_str(if parts[k - 1].block == part.block {
                part.joiner
            } else {
                "\n\n"
            });
        }
        out.push_str(&part.text);
    }
    out
}

fn overlap_tail(pieces: &[Piece], members: &[usize], overlap: usize) -> Vec<Part> {
    if overlap == 0 {
        return Vec::new();
    }
    let mut start = members.len();
    while start > 0 {
        let parts: Vec<Part> = members[start - 1..]
            .iter()
            .map(|&i| pieces[i].part.clone())
            .collect();
        if approx_tokens(&join(&parts)) > overlap {
            break;
        }
        start -= 1;
    }
    if start < members.len() {
        return members[start..]
            .iter()
            .map(|&i| pieces[i].part.clone())
            .collect();
    }
    let Some(&last) = members.last() else {
        return Vec::new();
    };
    let part = &pieces[last].part;
    if fence_marker(&part.text).is_some() || part.text.starts_with('|') {
        return Vec::new();
    }
    let mut offset = part.text.len();
    for unit in units(&part.text, 1).iter().rev() {
        if approx_tokens(&part.text[offset - unit.len()..]) > overlap {
            break;
        }
        offset -= unit.len();
    }
    Some(part.text[offset..].trim())
        .filter(|t| !t.is_empty())
        .map(|t| Part {
            text: t.to_string(),
            ..part.clone()
        })
        .into_iter()
        .collect()
}

fn split_block(text: &str, budget: usize) -> Vec<(String, &'static str)> {
    if approx_tokens(text) <= budget {
        return vec![(text.to_string(), "\n\n")];
    }
    let lines: Vec<&str> = text.lines().collect();
    if let Some(marker) = fence_marker(text) {
        let last = lines.len() - 1;
        let closed = last > 0 && lines[last].trim().starts_with(marker.as_str());
        let (end, close) = if closed {
            (last, lines[last].trim())
        } else {
            (lines.len(), marker.as_str())
        };
        let open = lines[0];
        let inner = lines[1..end].join("\n");
        let room = budget
            .saturating_sub(approx_tokens(open) + approx_tokens(close))
            .max(1);
        return pack(&inner, room, 0)
            .into_iter()
            .map(|r| (format!("{open}\n{}\n{close}", &inner[r]), "\n\n"))
            .collect();
    }
    let header = lines[..lines.len().min(2)].join("\n");
    if lines.len() > 2
        && lines[0].trim_start().starts_with('|')
        && is_separator(lines[1])
        && approx_tokens(&header) <= budget / 4
    {
        let rows = lines[2..].join("\n");
        let room = budget - approx_tokens(&header);
        return pack(&rows, room, 0)
            .into_iter()
            .map(|r| (format!("{header}\n{}", &rows[r]), "\n\n"))
            .collect();
    }
    let mut prev = 0;
    pack(text, budget, 0)
        .into_iter()
        .map(|r| {
            let joiner = if text[prev..r.start].contains('\n') {
                "\n"
            } else {
                " "
            };
            prev = r.end;
            (text[r].to_string(), joiner)
        })
        .collect()
}

fn pack(text: &str, max: usize, level: usize) -> Vec<Range<usize>> {
    if level > 2 || approx_tokens(text) <= max {
        return trimmed(text, 0).into_iter().collect();
    }
    let mut out = Vec::new();
    let (mut start, mut end) = (0, 0);
    for unit in units(text, level) {
        let unit_end = end + unit.len();
        if approx_tokens(&text[start..unit_end]) > max {
            if end > start {
                out.extend(trimmed(&text[start..end], start));
                start = end;
            }
            if approx_tokens(&text[start..unit_end]) > max {
                let at = start;
                out.extend(
                    pack(&text[at..unit_end], max, level + 1)
                        .into_iter()
                        .map(|r| r.start + at..r.end + at),
                );
                start = unit_end;
            }
        }
        end = unit_end;
    }
    out.extend(trimmed(&text[start..], start));
    out
}

fn trimmed(text: &str, offset: usize) -> Option<Range<usize>> {
    let lead = text.len() - text.trim_start().len();
    let body = text.trim();
    (!body.is_empty()).then(|| offset + lead..offset + lead + body.len())
}

fn units(text: &str, level: usize) -> Vec<&str> {
    match level {
        0 => text.split_inclusive('\n').collect(),
        1 => {
            let mut out = Vec::new();
            let mut start = 0;
            let mut chars = text.char_indices().peekable();
            while let Some((i, c)) = chars.next() {
                let next = chars.peek().map(|&(_, n)| n);
                let ends = matches!(c, '。' | '！' | '？')
                    || (matches!(c, '.' | '!' | '?') && next.is_some_and(char::is_whitespace));
                if ends {
                    let mut cut = i + c.len_utf8();
                    while let Some(&(j, n)) = chars.peek() {
                        if !n.is_whitespace() {
                            break;
                        }
                        cut = j + n.len_utf8();
                        chars.next();
                    }
                    out.push(&text[start..cut]);
                    start = cut;
                }
            }
            if start < text.len() {
                out.push(&text[start..]);
            }
            out
        }
        _ => text.split_inclusive(char::is_whitespace).collect(),
    }
}

fn fence_marker(text: &str) -> Option<String> {
    let first = text.trim_start().chars().next()?;
    if !matches!(first, '`' | '~') {
        return None;
    }
    let run: String = text
        .trim_start()
        .chars()
        .take_while(|&c| c == first)
        .collect();
    (run.len() >= 3).then_some(run)
}

fn is_separator(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('|')
        && line.contains('-')
        && line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

#[cfg(test)]
mod tests {
    use super::{approx_tokens, chunk};

    const PAGE: &str = "# Rust\n\nRust is a language.\n\n## History\n\nGraydon Hoare began Rust in 2006 as a personal project.\n\nMozilla sponsored it from 2009.\n\n## Tooling\n\nCargo is the package manager.\n\n```\nfn main() {\n\n    println!(\"hi\");\n}\n```\n";

    #[test]
    fn a_run_of_headings_is_cut_like_any_other_text() {
        let page = (0..500)
            .map(|i| format!("## Story title number {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let chunks = chunk(&page, 64, 0);
        assert!(chunks.len() > 10, "{}", chunks.len());
        assert!(chunks.iter().all(|c| c.tokens <= 64), "{chunks:?}");
    }

    #[test]
    fn a_heading_ladder_taller_than_the_chunk_keeps_chunks_under_size() {
        let page = (0..20)
            .map(|i| {
                let ladder = (1..=6)
                    .map(|l| {
                        format!(
                            "{} Section {i}.{l} with a fairly long heading text here",
                            "#".repeat(l)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                format!("{ladder}\n\nShort body.")
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let chunks = chunk(&page, 64, 0);
        assert!(chunks.iter().all(|c| c.tokens <= 64), "{chunks:?}");
        for c in chunks.iter().filter(|c| c.text.contains("Short body.")) {
            assert!(c.text.contains("###### Section"), "{c:?}");
        }
    }

    #[test]
    fn a_long_paragraph_under_tall_headings_gets_the_room_they_leave() {
        let headings = (1..=4)
            .map(|l| format!("{} Heading level {l} that takes some room", "#".repeat(l)))
            .collect::<Vec<_>>()
            .join("\n\n");
        let page = format!(
            "{headings}\n\n{}",
            "Words keep coming in this paragraph. ".repeat(40)
        );
        let chunks = chunk(&page, 60, 0);
        assert!(chunks.iter().all(|c| c.tokens <= 60), "{chunks:?}");
    }

    #[test]
    fn approx_tokens_counts_words_digits_symbols_and_cjk_characters() {
        assert_eq!(approx_tokens(""), 0);
        assert_eq!(approx_tokens("  \n "), 0);
        assert_eq!(approx_tokens("The cat sat."), 4);
        assert_eq!(approx_tokens("extraordinary"), 3);
        assert_eq!(approx_tokens("1234567"), 3);
        assert_eq!(approx_tokens("[a](https://x.io)"), 10);
        assert_eq!(approx_tokens("東京都庁"), 4);
        assert_eq!(approx_tokens("Tokyo 東京"), 3);
        assert_eq!(approx_tokens("القاهرة"), 4);
        assert_eq!(approx_tokens("città"), 2);
    }

    #[test]
    fn a_small_page_is_one_chunk_under_its_title() {
        let chunks = chunk(PAGE, 1000, 0);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, PAGE.trim());
        assert_eq!(chunks[0].heading_path, ["Rust"]);
        assert_eq!(chunks[0].tokens, approx_tokens(PAGE.trim()));
    }

    #[test]
    fn chunks_follow_sections_and_keep_their_heading_path() {
        let chunks = chunk(PAGE, 20, 0);
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        let history = chunks.iter().find(|c| c.text.contains("Graydon")).unwrap();
        assert!(
            history.text.starts_with("## History\n\nGraydon"),
            "{texts:?}"
        );
        assert_eq!(history.heading_path, ["Rust", "History"]);
        assert!(
            chunks.iter().all(|c| !c.text.ends_with("## History")),
            "{texts:?}"
        );
        assert_eq!(
            chunks.iter().map(|c| c.index).collect::<Vec<_>>(),
            (0..chunks.len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn code_fences_stay_whole_when_they_fit() {
        let chunks = chunk(PAGE, 20, 0);
        assert!(
            chunks.iter().any(|c| c
                .text
                .contains("```\nfn main() {\n\n    println!(\"hi\");\n}\n```")),
            "{chunks:?}"
        );
    }

    #[test]
    fn a_long_paragraph_is_cut_at_sentence_ends() {
        let sentence = "The quick brown fox jumps over the lazy dog again. ";
        let page = format!("# Fox\n\n{}", sentence.repeat(20));
        let chunks = chunk(&page, 40, 0);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.tokens <= 40, "{c:?}");
            let body = c.text.trim_start_matches("# Fox").trim();
            assert!(
                body.starts_with("The quick") && body.ends_with("again."),
                "{c:?}"
            );
        }
    }

    #[test]
    fn a_heading_and_the_start_of_its_long_paragraph_fit_one_chunk() {
        let sentence = "Cargo builds code and gets the crates it needs. ";
        let page = format!("# Cargo\n\n{}", sentence.repeat(12));
        let chunks = chunk(&page, 60, 0);
        assert!(
            chunks[0].text.starts_with("# Cargo\n\nCargo builds"),
            "{chunks:?}"
        );
        assert!(chunks.iter().all(|c| c.tokens <= 60), "{chunks:?}");
    }

    #[test]
    fn a_long_code_fence_is_reopened_in_every_piece() {
        let body: String = (0..60).map(|i| format!("let value_{i} = {i};\n")).collect();
        let page = format!("```rust\n{body}```");
        let chunks = chunk(&page, 50, 0);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(
                c.text.starts_with("```rust\n") && c.text.ends_with("\n```"),
                "{c:?}"
            );
            assert!(
                c.text
                    .lines()
                    .all(|l| l.starts_with("let value_") || l.starts_with("```")),
                "{c:?}"
            );
        }
    }

    #[test]
    fn a_long_table_repeats_its_header_and_keeps_rows_whole() {
        let rows: String = (0..40)
            .map(|i| format!("| item {i} | {i}.00 |\n"))
            .collect();
        let page = format!("| name | price |\n| --- | --- |\n{rows}");
        let chunks = chunk(&page, 60, 0);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(
                c.text
                    .starts_with("| name | price |\n| --- | --- |\n| item "),
                "{c:?}"
            );
            assert!(
                c.text
                    .lines()
                    .all(|l| l.starts_with('|') && l.ends_with('|')),
                "{c:?}"
            );
        }
    }

    #[test]
    fn a_table_with_a_huge_header_row_is_cut_by_lines_without_repeating_it() {
        let header = (0..80)
            .map(|i| format!("col{i}"))
            .collect::<Vec<_>>()
            .join(" | ");
        let rows: String = (0..30).map(|i| format!("| row {i} | {i} |\n")).collect();
        let page = format!("| {header} |\n| --- | --- |\n{rows}");
        let chunks = chunk(&page, 60, 0);
        assert!(chunks.len() > 1);
        assert_eq!(
            chunks.iter().filter(|c| c.text.contains("col79")).count(),
            1
        );
        assert!(chunks.iter().all(|c| c.tokens <= 60), "{chunks:?}");
    }

    #[test]
    fn overlap_repeats_the_end_of_the_previous_chunk() {
        let page = (0..12)
            .map(|i| format!("Paragraph {i} says something short."))
            .collect::<Vec<_>>()
            .join("\n\n");
        let chunks = chunk(&page, 30, 10);
        assert!(chunks.len() > 2);
        for pair in chunks.windows(2) {
            let last = pair[0].text.rsplit("\n\n").next().unwrap();
            assert!(pair[1].text.starts_with(last), "{pair:?}");
            assert!(pair[1].tokens <= 30, "{pair:?}");
        }
    }

    #[test]
    fn pieces_of_one_paragraph_rejoin_with_their_own_spacing() {
        let page = "Rust is a language that many people use for systems work today.";
        assert_eq!(chunk(page, 20, 8)[0].text, page);
        let sentence = "The quick brown fox jumps over the lazy dog again. ";
        let chunks = chunk(&sentence.repeat(20), 40, 15);
        assert!(chunks.len() > 2);
        for pair in chunks.windows(2) {
            assert!(!pair[1].text.contains('\n'), "{pair:?}");
            assert!(pair[1].text.starts_with("The quick"), "{pair:?}");
            assert!(pair[1].tokens <= 40, "{pair:?}");
        }
    }

    #[test]
    fn no_text_is_lost_without_overlap() {
        let sentence = "Words keep coming in this paragraph. ";
        let page = format!(
            "# A\n\n{}\n\n## B\n\n- one\n- two\n- three\n\n{}",
            sentence.repeat(30),
            sentence.repeat(5)
        );
        let squash = |s: &str| s.split_whitespace().collect::<String>();
        let joined: String = chunk(&page, 50, 0)
            .iter()
            .map(|c| squash(&c.text))
            .collect();
        assert_eq!(joined, squash(&page));
    }

    #[test]
    fn empty_markdown_has_no_chunks() {
        assert!(chunk("", 100, 0).is_empty());
        assert!(chunk("\n\n  \n", 100, 0).is_empty());
    }
}
