//! Sentence and paragraph segmentation for the simple-english lint engine.
//!
//! Simplified from the TypeScript original. Splits text into sentences
//! (delimited by `.`, `!`, `?` plus closing delimiters) and groups them
//! into paragraphs (separated by blank lines, via the `split-paragraphs`
//! crate which also handles CRLF/CR line endings).
//! Handles a small set of known abbreviations to avoid spurious splits.

use split_paragraphs::SplitParagraphs;

use crate::types::{Paragraph, Sentence};

/// Known abbreviations that should not trigger a sentence boundary.
const ABBREVIATIONS: &[&str] = &[
    "e.g.", "i.e.", "etc.", "vs.", "Fig.", "No.",
];

/// Closing delimiters that can follow a sentence-ending punctuation mark.
const CLOSING_DELIMS: &[char] = &[
    '"', '\u{201d}', '\u{00bb}', '\u{203a}', ')', ']', '}',
];

/// Segment `text` into sentences. Each sentence carries its 1-based line
/// number, 1-based column of its first non-whitespace character, and the
/// word count.
pub fn segment_sentences(text: &str) -> Vec<Sentence> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut sentences: Vec<Sentence> = Vec::new();
    let mut open: Option<(usize, usize)> = None; // (line_idx, col)
    let mut parts: Vec<String> = Vec::new();

    fn close_sentence(
        sentences: &mut Vec<Sentence>,
        parts: &mut Vec<String>,
        start_line: usize,
        start_col: usize,
    ) {
        let text = parts.join(" ").trim().to_string();
        if !text.is_empty() {
            let word_count = text.split_whitespace().count();
            sentences.push(Sentence {
                text,
                line: start_line + 1,
                column: start_col + 1,
                word_count,
            });
        }
        parts.clear();
    }

    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            close_sentence(&mut sentences, &mut parts, 0, 0);
            open = None;
            continue;
        }

        let trimmed = line.trim_start();
        let leading_ws = line.len() - trimmed.len();
        if open.is_none() {
            open = Some((i, leading_ws));
        }

        let mut last_end = 0usize;
        for (_start, end) in sentence_ends(line) {
            let part = &line[last_end..end];
            if !part.trim().is_empty() {
                parts.push(part.trim().to_string());
            }
            let start_line = open.map(|(l, _)| l).unwrap_or(i);
            let start_col = open.map(|(_, c)| c).unwrap_or(last_end);
            close_sentence(&mut sentences, &mut parts, start_line, start_col);
            open = None;
            last_end = end;
        }

        let rest = &line[last_end..];
        if !rest.trim().is_empty() {
            open = Some((i, last_end + (rest.len() - rest.trim_start().len())));
            parts.push(rest.trim().to_string());
        }
    }

    if let Some((l, c)) = open {
        close_sentence(&mut sentences, &mut parts, l, c);
    }

    sentences
}

/// Returns `(start, end)` byte ranges in `line` where a sentence ends.
///
/// All returned indices are **byte offsets** on char boundaries, so the
/// caller can slice the source string with `&line[last..end]` without
/// panicking on multibyte characters.
fn sentence_ends(line: &str) -> Vec<(usize, usize)> {
    let mut ends: Vec<(usize, usize)> = Vec::new();
    let len = line.len();
    let mut i = 0usize;

    while i < len {
        let ch = line[i..].chars().next().unwrap();
        let ch_bytes = ch.len_utf8();
        match ch {
            '.' | '!' | '?' | '\u{3002}' | '\u{ff01}' | '\u{ff1f}' => {
                let start = i;
                let mut end = i + ch_bytes;
                // Consume consecutive terminators: "..." "?!?!" "。！"
                while end < len {
                    let c = line[end..].chars().next().unwrap();
                    if matches!(c, '.' | '!' | '?' | '\u{3002}' | '\u{ff01}' | '\u{ff1f}') {
                        end += c.len_utf8();
                    } else {
                        break;
                    }
                }
                // Consume closing delimiters
                while end < len {
                    let c = line[end..].chars().next().unwrap();
                    if CLOSING_DELIMS.contains(&c) {
                        end += c.len_utf8();
                    } else {
                        break;
                    }
                }

                // Check if this is a known abbreviation (e.g. "e.g.")
                let before = &line[..start];
                let is_abbrev = ABBREVIATIONS.iter().any(|abbr| {
                    before.ends_with(abbr)
                });

                if !is_abbrev {
                    ends.push((start, end));
                }
                i = end;
                continue;
            }
            _ => {
                i += ch_bytes;
            }
        }
    }

    ends
}

/// Group sentences into paragraphs.
///
/// Paragraph boundaries are detected by the `split-paragraphs` crate,
/// which splits on blank (or whitespace-only) lines and supports both
/// `\n` and `\r\n` line endings.
///
/// Each returned `Paragraph` carries the 1-based line number of its
/// first line and the subset of `sentences` that start on lines within
/// the paragraph.
pub fn segment_paragraphs(
    text: &str,
    sentences: &[Sentence],
) -> Vec<Paragraph> {
    let para_texts: Vec<&str> = text.paragraphs().collect();
    if para_texts.is_empty() {
        return Vec::new();
    }

    // Pre-compute byte offsets where each 0-based line starts, so we can
    // map a paragraph's byte offset to a 1-based line number in O(log n).
    let line_starts: Vec<usize> = {
        let mut offsets = vec![0usize];
        for (i, c) in text.char_indices() {
            if c == '\n' {
                offsets.push(i + 1);
            }
        }
        offsets
    };

    /// Returns the 1-based line number that contains byte offset `pos`.
    fn line_at(line_starts: &[usize], pos: usize) -> usize {
        match line_starts.binary_search(&pos) {
            Ok(i) => i + 1, // exact match: 0-based line i → 1-based i+1
            Err(i) => i,     // insertion point: pos is in 0-based line i-1 → 1-based i
        }
    }

    let mut search_from = 0usize;
    let mut result: Vec<Paragraph> = Vec::with_capacity(para_texts.len());

    for para_text in &para_texts {
        // Locate this paragraph's start in the original text. Paragraphs
        // are in order and non-overlapping, so we advance `search_from`
        // past each one to avoid matching an earlier duplicate.
        let pos = text[search_from..]
            .find(para_text)
            .map(|off| off + search_from)
            .unwrap_or(search_from);
        search_from = pos + para_text.len();

        let start_line = line_at(&line_starts, pos);
        let para_line_count = para_text.lines().count().max(1);
        let end_line = start_line + para_line_count - 1;

        let para_sents: Vec<Sentence> = sentences
            .iter()
            .filter(|s| s.line >= start_line && s.line <= end_line)
            .cloned()
            .collect();

        if !para_sents.is_empty() {
            result.push(Paragraph {
                sentences: para_sents,
                line: start_line,
                column: 1,
            });
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_simple_sentences() {
        let sents = segment_sentences("Hello world. How are you?");
        assert_eq!(sents.len(), 2);
        assert_eq!(sents[0].text, "Hello world.");
        assert_eq!(sents[1].text, "How are you?");
    }

    #[test]
    fn word_count() {
        let sents = segment_sentences("One two three.");
        assert_eq!(sents[0].word_count, 3);
    }

    #[test]
    fn multi_line_sentence() {
        let sents = segment_sentences(
            "This is a long sentence that\nspans multiple lines.",
        );
        assert_eq!(sents.len(), 1);
        assert_eq!(
            sents[0].text,
            "This is a long sentence that spans multiple lines."
        );
    }

    #[test]
    fn paragraphs_split_by_blank_line() {
        let text = "First para one.\nSecond sentence.\n\nNew para.";
        let sents = segment_sentences(text);
        let paras = segment_paragraphs(text, &sents);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].sentences.len(), 2);
        assert_eq!(paras[1].sentences.len(), 1);
    }

    #[test]
    fn paragraphs_crlf() {
        let text = "Line one.\r\nLine two.\r\n\r\nLine three.";
        let sents = segment_sentences(text);
        let paras = segment_paragraphs(text, &sents);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].line, 1);
        assert_eq!(paras[1].line, 4);
    }

    #[test]
    fn paragraphs_whitespace_only_line_breaks() {
        let text = "Para one.\n   \nPara two.";
        let sents = segment_sentences(text);
        let paras = segment_paragraphs(text, &sents);
        assert_eq!(paras.len(), 2);
    }

    #[test]
    fn paragraphs_multiple_sentences_line_numbers() {
        let text = "S one.\nS two.\n\nS three.\nS four.\nS five.";
        let sents = segment_sentences(text);
        let paras = segment_paragraphs(text, &sents);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].line, 1);
        assert_eq!(paras[0].sentences.len(), 2);
        assert_eq!(paras[1].line, 4);
        assert_eq!(paras[1].sentences.len(), 3);
    }

    #[test]
    fn empty_text() {
        let sents = segment_sentences("");
        assert!(sents.is_empty());
    }

    /// Regression test: a line containing multibyte characters (e.g. the
    /// ellipsis `…` and em-dash `—`) followed by a sentence terminator
    /// must not panic on a char-boundary violation, and must still split
    /// at the ASCII terminator after the multibyte run.
    #[test]
    fn multibyte_chars_before_terminator() {
        let line = "- **`table_grid`** (rewritten) \u{2014} wide cells wrap, truncated with `\u{2026}`. A data row spans.";
        let sents = segment_sentences(line);
        // Two sentences: the first ends at the '.' after the ellipsis,
        // the second is the tail. Before the byte-index fix this panicked
        // because the char index was sliced as a byte index mid-'…'.
        assert_eq!(sents.len(), 2);
        assert_eq!(
            sents[0].text,
            "- **`table_grid`** (rewritten) \u{2014} wide cells wrap, truncated with `\u{2026}`."
        );
        assert_eq!(sents[1].text, "A data row spans.");
    }

    #[test]
    fn cjk_sentence() {
        let sents = segment_sentences("你好世界。这是测试！");
        assert_eq!(sents.len(), 2);
        assert_eq!(sents[0].text, "你好世界。");
        assert_eq!(sents[1].text, "这是测试！");
    }
}
