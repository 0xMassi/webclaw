//! Keep the parts of an extracted page that answer a question.
//!
//! The markdown is split into blocks (paragraphs, lists, tables, code
//! fences), each scored against the question with BM25 over its own words
//! and the headings above it. Blocks scoring at least half the best score
//! stay, in page order, under their headings; the page title always stays.
//! Words match on their first six letters, so most inflected forms meet;
//! Chinese, Japanese and Korean text matches by character pairs.

use std::collections::{HashMap, HashSet};

use crate::ExtractionResult;

const K1: f64 = 1.2;
const B: f64 = 0.75;
const KEEP_RATIO: f64 = 0.5;
const STEM: usize = 6;

struct Block<'a> {
    text: &'a str,
    heading: Option<usize>,
    level: usize,
}

/// `markdown` cut down to the blocks most relevant to `query`. Returns the
/// input unchanged when no block shares a word with the query.
pub fn focus(markdown: &str, query: &str) -> String {
    let blocks = split_blocks(markdown);
    let wanted: HashSet<String> = terms(query).into_iter().collect();
    if blocks.is_empty() || wanted.is_empty() {
        return markdown.to_string();
    }
    let docs: Vec<Vec<String>> = (0..blocks.len())
        .map(|i| {
            let mut words = terms(blocks[i].text);
            let mut parent = blocks[i].heading;
            while let Some(h) = parent {
                words.extend(terms(blocks[h].text));
                parent = blocks[h].heading;
            }
            words
        })
        .collect();
    let scores = bm25(&docs, &wanted);
    let best = scores.iter().cloned().fold(0.0, f64::max);
    if best <= 0.0 {
        return markdown.to_string();
    }
    let mut keep = vec![false; blocks.len()];
    for (i, score) in scores.iter().enumerate() {
        if *score >= best * KEEP_RATIO {
            keep[i] = true;
            let mut parent = blocks[i].heading;
            while let Some(h) = parent {
                keep[h] = true;
                parent = blocks[h].heading;
            }
        }
    }
    if let Some(title) = blocks.iter().position(|b| b.level == 1) {
        keep[title] = true;
    }
    blocks
        .iter()
        .zip(keep)
        .filter(|(_, kept)| *kept)
        .map(|(block, _)| block.text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Apply [`focus`] to the markdown of `result`, then rebuild its plain text
/// and word count from what was kept.
pub fn focus_extraction(result: &mut ExtractionResult, query: &str) {
    result.content.markdown = focus(&result.content.markdown, query);
    result.content.plain_text = crate::markdown::strip_markdown(&result.content.markdown);
    result.metadata.word_count = crate::extractor::word_count(&result.content.plain_text);
}

fn split_blocks(markdown: &str) -> Vec<Block<'_>> {
    let mut ranges = Vec::new();
    let mut start: Option<usize> = None;
    let mut fence: Option<(char, usize)> = None;
    let mut offset = 0;
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim();
        let marker = trimmed
            .chars()
            .next()
            .filter(|c| matches!(c, '`' | '~'))
            .map(|c| (c, trimmed.chars().take_while(|&x| x == c).count()))
            .filter(|&(_, len)| len >= 3);
        fence = match (fence, marker) {
            (None, Some(open)) => Some(open),
            (Some((c, len)), Some((mark, n)))
                if c == mark && n >= len && trimmed[n..].trim().is_empty() =>
            {
                None
            }
            (open, _) => open,
        };
        if fence.is_none() && (trimmed.is_empty() || heading_level(trimmed) > 0) {
            if let Some(from) = start.take() {
                ranges.push((from, offset));
            }
            if !trimmed.is_empty() {
                ranges.push((offset, offset + line.len()));
            }
        } else if start.is_none() {
            start = Some(offset);
        }
        offset += line.len();
    }
    if let Some(from) = start {
        ranges.push((from, markdown.len()));
    }
    let mut blocks: Vec<Block<'_>> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for (from, to) in ranges {
        let text = markdown[from..to].trim();
        if text.is_empty() {
            continue;
        }
        let level = heading_level(text);
        if level > 0 {
            while stack.last().is_some_and(|&h| blocks[h].level >= level) {
                stack.pop();
            }
        }
        blocks.push(Block {
            text,
            heading: stack.last().copied(),
            level,
        });
        if level > 0 {
            stack.push(blocks.len() - 1);
        }
    }
    blocks
}

fn heading_level(text: &str) -> usize {
    let hashes = text.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && text[hashes..].starts_with(' ') {
        hashes
    } else {
        0
    }
}

fn bm25(docs: &[Vec<String>], wanted: &HashSet<String>) -> Vec<f64> {
    let n = docs.len() as f64;
    let avg = docs.iter().map(Vec::len).sum::<usize>() as f64 / n;
    let mut df: HashMap<&str, f64> = HashMap::new();
    for doc in docs {
        let unique: HashSet<&str> = doc.iter().map(String::as_str).collect();
        for term in unique.into_iter().filter(|t| wanted.contains(*t)) {
            *df.entry(term).or_default() += 1.0;
        }
    }
    docs.iter()
        .map(|doc| {
            let mut tf: HashMap<&str, f64> = HashMap::new();
            for term in doc.iter().filter(|t| wanted.contains(*t)) {
                *tf.entry(term).or_default() += 1.0;
            }
            let len = doc.len() as f64;
            tf.iter()
                .map(|(term, f)| {
                    let d = df[term];
                    let idf = ((n - d + 0.5) / (d + 0.5) + 1.0).ln();
                    idf * f * (K1 + 1.0) / (f + K1 * (1.0 - B + B * len / avg.max(1.0)))
                })
                .sum()
        })
        .collect()
}

fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        let chars: Vec<char> = word.chars().collect();
        if chars.iter().any(|&c| is_cjk(c)) {
            if chars.len() == 1 {
                out.push(word.to_string());
            }
            out.extend(chars.windows(2).map(|pair| pair.iter().collect()));
        } else {
            let word = word
                .strip_prefix("ال")
                .filter(|rest| rest.chars().count() > 2)
                .unwrap_or(word);
            out.push(word.chars().take(STEM).collect());
        }
    }
    out
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{ac00}'..='\u{d7af}')
}

#[cfg(test)]
mod tests {
    use super::focus;

    const PAGE: &str = "# Rust\n\nRust is a language.\n\n## History\n\nGraydon Hoare began Rust in 2006 as a personal project.\n\nMozilla sponsored it from 2009.\n\n## Tooling\n\nCargo is the package manager.\n\n```\nfn main() {\n\n    println!(\"hi\");\n}\n```\n";

    #[test]
    fn keeps_the_matching_block_with_its_heading_and_the_title() {
        let out = focus(PAGE, "Who began the language as a personal project?");
        assert!(out.contains("Graydon Hoare"), "{out}");
        assert!(out.contains("## History"), "{out}");
        assert!(out.starts_with("# Rust"), "{out}");
        assert!(!out.contains("Cargo"), "{out}");
    }

    #[test]
    fn a_heading_match_keeps_its_section() {
        let out = focus(PAGE, "tooling");
        assert!(out.contains("Cargo is the package manager"), "{out}");
        assert!(!out.contains("Graydon"), "{out}");
    }

    #[test]
    fn code_fences_stay_whole() {
        let out = focus(PAGE, "println main");
        assert!(
            out.contains("fn main() {\n\n    println!(\"hi\");\n}"),
            "{out}"
        );
    }

    #[test]
    fn word_forms_match_on_their_first_six_letters() {
        let page =
            "# Roma\n\nLa città è attraversata dal fiume Tevere.\n\nIl clima è mediterraneo.";
        let out = focus(page, "Quale fiume attraversa la città?");
        assert!(out.contains("Tevere"), "{out}");
        assert!(!out.contains("clima"), "{out}");
    }

    #[test]
    fn a_tilde_line_inside_a_backtick_fence_does_not_close_it() {
        let page = "# Doc\n\n```\nlet a = 1;\n~~~\n\nlet zebra = 2;\n```\n\nUnrelated prose.";
        let out = focus(page, "zebra");
        assert!(
            out.contains("```\nlet a = 1;\n~~~\n\nlet zebra = 2;\n```"),
            "{out}"
        );
    }

    #[test]
    fn plain_text_and_word_count_follow_the_kept_markdown() {
        let mut result = crate::extract(
            "<html><body><article><h1>Rust</h1><p>Rust is a language that many people use for systems work today.</p><h2>Tooling</h2><p>Cargo is the package manager and build tool that ships with every install.</p><h2>History</h2><p>Graydon Hoare began it in 2006 as a personal project at home.</p></article></body></html>",
            None,
        )
        .unwrap();
        super::focus_extraction(&mut result, "tooling");
        assert!(
            result
                .content
                .plain_text
                .contains("Cargo is the package manager"),
            "{}",
            result.content.plain_text
        );
        assert!(
            !result.content.plain_text.contains("Graydon"),
            "{}",
            result.content.plain_text
        );
        assert_eq!(
            result.metadata.word_count,
            crate::extractor::word_count(&result.content.plain_text)
        );
    }

    #[test]
    fn no_overlap_returns_the_page() {
        assert_eq!(focus(PAGE, "zebra"), PAGE);
    }

    #[test]
    fn cjk_text_matches_by_character_pairs() {
        let page = "# 東京都\n\n東京都庁は新宿区にある。\n\n人口は約1400万人。";
        let out = focus(page, "東京都庁はどこにありますか");
        assert!(out.contains("新宿"), "{out}");
        assert!(!out.contains("人口"), "{out}");
    }
}
