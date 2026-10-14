//! Split extracted markdown into chunks for retrieval.
//!
//! Chunks are packed from whole blocks (paragraphs, lists, tables, code
//! fences) and stay under a token budget. A block larger than the budget is
//! cut at line breaks, then sentence ends, then spaces, then between
//! characters (never inside an accented letter or emoji sequence): a cut code fence is
//! reopened in every piece and a cut table repeats its header rows. Each
//! chunk records the headings it sits under.

use std::ops::Range;

use std::sync::LazyLock;

use regex::Regex;
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
    /// Headings in effect where the chunk starts, outermost first. A chunk
    /// that runs on into a later section keeps the path of its start.
    pub heading_path: Vec<String>,
    /// Approximate token count of `text`, from [`approx_tokens`].
    pub tokens: usize,
    /// The chunk's markdown.
    pub text: String,
}

static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").expect("valid link pattern"));

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
/// a row, one per Chinese, Japanese or Korean character, and one for the
/// space before a number (counted at the start of the text too, so the
/// count of a piece never falls below its share of the whole).
pub fn approx_tokens(s: &str) -> usize {
    let mut tally = Tally::default();
    s.chars().for_each(|c| tally.push(c));
    tally.total()
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Run {
    #[default]
    Space,
    Letter,
    Digit,
    Symbol,
}

#[derive(Clone, Copy, Default)]
struct Tally {
    tokens: usize,
    run: Run,
    len: usize,
    wide: usize,
    glued: bool,
}

impl Tally {
    fn push(&mut self, c: char) {
        let kind = if is_cjk(c) {
            self.tokens += 1;
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
        if kind != self.run {
            self.tokens = self.total() + usize::from(kind == Run::Digit && !self.glued);
            (self.run, self.len, self.wide) = (kind, 0, 0);
        }
        self.glued = !c.is_whitespace();
        self.len += 1;
        if kind == Run::Letter && !c.is_ascii() {
            self.wide += 1;
        }
    }

    fn total(&self) -> usize {
        self.tokens
            + match self.run {
                Run::Space => 0,
                Run::Letter => (self.len - self.wide).div_ceil(6) + self.wide.div_ceil(2),
                Run::Digit => self.len.div_ceil(3),
                Run::Symbol => self.len.div_ceil(2),
            }
    }
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
    let (mut start, mut size) = (members.len(), 0);
    while start > 0 && size + pieces[members[start - 1]].tokens <= overlap {
        size += pieces[members[start - 1]].tokens;
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
    let (mut offset, mut size) = (part.text.len(), 0);
    for unit in units(&part.text, 1).iter().rev() {
        size += approx_tokens(unit);
        if size > overlap {
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
    if let Some(marker) = fence_marker(text).filter(|_| lines.len() > 1) {
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
    let short = LINK.replace_all(&header, "$1");
    let head = if approx_tokens(&header) <= budget / 2 {
        header.as_str()
    } else {
        short.as_ref()
    };
    if lines.len() > 2
        && lines[0].trim_start().starts_with('|')
        && is_separator(lines[1])
        && approx_tokens(head) <= budget / 2
    {
        let rows = lines[2..].join("\n");
        let room = budget - approx_tokens(head);
        return pack(&rows, room, 0)
            .into_iter()
            .map(|r| (format!("{head}\n{}", &rows[r]), "\n\n"))
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
    if approx_tokens(text) <= max {
        return trimmed(text, 0).into_iter().collect();
    }
    if level > 2 {
        return cut_characters(text, max);
    }
    let mut out = Vec::new();
    let (mut start, mut end, mut size) = (0, 0, 0);
    for unit in units(text, level) {
        let tokens = approx_tokens(unit);
        let unit_end = end + unit.len();
        if size + tokens > max && end > start {
            out.extend(trimmed(&text[start..end], start));
            (start, size) = (end, 0);
        }
        if tokens > max {
            let at = end;
            out.extend(
                pack(unit, max, level + 1)
                    .into_iter()
                    .map(|r| r.start + at..r.end + at),
            );
            (start, size) = (unit_end, 0);
        } else {
            size += tokens;
        }
        end = unit_end;
    }
    out.extend(trimmed(&text[start..], start));
    out
}

fn cut_characters(text: &str, max: usize) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let (mut start, mut cluster, mut tally, mut joined) = (0, 0, Tally::default(), false);
    for (i, c) in text.char_indices().chain([(text.len(), ' ')]) {
        let end = i == text.len();
        let extends = !end && (joined || extends_character(c));
        joined = c == '\u{200d}';
        if !extends && i > cluster {
            if tally.total() > max && cluster > start {
                out.extend(trimmed(&text[start..cluster], start));
                start = cluster;
                tally = Tally::default();
                text[cluster..i].chars().for_each(|c| tally.push(c));
            }
            cluster = i;
        }
        if !end {
            tally.push(c);
        }
    }
    out.extend(trimmed(&text[start..], start));
    out
}

// ponytail: common combining ranges only; unicode-segmentation if rarer scripts split mid-cluster
fn extends_character(c: char) -> bool {
    matches!(c,
        '\u{300}'..='\u{36f}' | '\u{483}'..='\u{489}' | '\u{591}'..='\u{5c7}' | '\u{610}'..='\u{61a}'
        | '\u{64b}'..='\u{65f}' | '\u{670}' | '\u{6d6}'..='\u{6ed}' | '\u{900}'..='\u{903}'
        | '\u{93a}'..='\u{94f}' | '\u{951}'..='\u{957}' | '\u{962}'..='\u{963}' | '\u{e31}'
        | '\u{e34}'..='\u{e3a}' | '\u{e47}'..='\u{e4e}' | '\u{1ab0}'..='\u{1aff}' | '\u{1dc0}'..='\u{1dff}'
        | '\u{200c}'..='\u{200d}' | '\u{20d0}'..='\u{20ff}' | '\u{fe00}'..='\u{fe0f}' | '\u{fe20}'..='\u{fe2f}'
        | '\u{1f3fb}'..='\u{1f3ff}' | '\u{e0020}'..='\u{e007f}')
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
    let line = text.trim_start().lines().next().unwrap_or_default();
    let run: String = line.chars().take_while(|&c| c == first).collect();
    (run.len() >= 3 && (first == '~' || !line[run.len()..].contains('`'))).then_some(run)
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
    fn a_run_without_spaces_is_cut_at_character_boundaries() {
        let page = format!("Intro.\n\n{}\n\nOutro.", "a".repeat(5000));
        let chunks = chunk(&page, 64, 0);
        assert!(chunks.iter().all(|c| c.tokens <= 64), "{chunks:?}");
        let squash = |s: &str| s.split_whitespace().collect::<String>();
        let joined: String = chunks.iter().map(|c| squash(&c.text)).collect();
        assert_eq!(joined, squash(&page));
    }

    #[test]
    fn a_cut_without_spaces_keeps_emoji_sequences_and_accents_whole() {
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}".repeat(300);
        let chunks = chunk(&family, 16, 0);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.tokens <= 16, "{c:?}");
            assert!(
                c.text.starts_with('\u{1f468}') && c.text.ends_with('\u{1f467}'),
                "{c:?}"
            );
        }
        let accents = "e\u{301}".repeat(400);
        for c in chunk(&accents, 16, 0) {
            assert!(
                c.text.starts_with('e') && c.text.ends_with('\u{301}'),
                "{c:?}"
            );
        }
    }

    #[test]
    fn a_one_line_block_that_starts_like_a_fence_is_kept() {
        let page = format!("```inline``` {}", "word ".repeat(400));
        let chunks = chunk(&page, 64, 0);
        let words: usize = chunks.iter().map(|c| c.text.matches("word").count()).sum();
        assert_eq!(words, 400, "{chunks:?}");
    }

    #[test]
    fn a_huge_paragraph_is_chunked_in_linear_time() {
        let paragraph = (0..10_000)
            .map(|i| format!("Sentence {i} says a few words about it."))
            .collect::<Vec<_>>()
            .join(" ");
        for overlap in [0, 10_000] {
            let started = std::time::Instant::now();
            let chunks = chunk(&paragraph, 20_000, overlap);
            assert!(chunks.len() > 1);
            assert!(started.elapsed().as_secs() < 2, "{:?}", started.elapsed());
        }
    }

    #[test]
    fn a_number_after_a_space_costs_a_token_for_the_space() {
        assert_eq!(approx_tokens("a 1 2 3"), 7);
        assert_eq!(approx_tokens("page 195"), 3);
        assert_eq!(approx_tokens("1234567"), 4);
    }

    #[test]
    fn a_fence_that_touches_text_is_its_own_block() {
        let code: String = (0..200)
            .map(|i| format!("echo step {i} && make target{i}\n"))
            .collect();
        let before = format!("# Setup\n\nRun this:\n```sh\n{code}```\n");
        let after = format!(
            "```py\n{code}```\nAfter the code. {}",
            "More words here. ".repeat(40)
        );
        for page in [before, after] {
            for c in chunk(&page, 64, 0) {
                let fences = c.text.lines().filter(|l| l.starts_with("```")).count();
                assert_eq!(fences % 2, 0, "{c:?}");
                if c.text.contains("echo step") {
                    assert!(
                        c.text.contains("```sh\n") || c.text.contains("```py\n"),
                        "{c:?}"
                    );
                }
                if c.text.contains("More words") {
                    assert!(!c.text.trim_end().ends_with("```"), "{c:?}");
                }
            }
        }
    }

    #[test]
    fn backticks_in_the_info_string_do_not_open_a_fence() {
        let page = "Intro.\n\n```inline``` code here\n\n## Next\n\nOutro.";
        let kept = crate::focus::focus(page, "outro");
        assert_eq!(kept, "## Next\n\nOutro.");
    }

    #[test]
    fn a_table_header_full_of_links_is_repeated_without_them() {
        let header = (0..8)
            .map(|i| format!("[Month {i}](https://example.org/wiki/Month_number_{i})"))
            .collect::<Vec<_>>()
            .join(" | ");
        let rows: String = (0..90)
            .map(|i| format!("| {} |\n", vec![format!("{i}.5"); 8].join(" | ")))
            .collect();
        let page = format!("| {header} |\n|{}\n{rows}", "---|".repeat(8));
        for (size, head) in [
            (512, format!("| {header} |")),
            (128, "| Month 0 | Month 1 |".to_string()),
        ] {
            let chunks = chunk(&page, size, 0);
            assert!(chunks.len() > 2);
            for c in &chunks {
                assert!(c.text.starts_with(&head), "{c:?}");
                assert!(c.tokens <= size, "{c:?}");
            }
        }
    }

    #[test]
    fn approx_tokens_counts_words_digits_symbols_and_cjk_characters() {
        assert_eq!(approx_tokens(""), 0);
        assert_eq!(approx_tokens("  \n "), 0);
        assert_eq!(approx_tokens("The cat sat."), 4);
        assert_eq!(approx_tokens("extraordinary"), 3);
        assert_eq!(approx_tokens("1234567"), 4);
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
