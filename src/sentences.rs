//! Sentence and paragraph segmentation for the simple-english lint engine.
//!
//! Simplified from the TypeScript original. Splits text into sentences
//! (delimited by `.`, `!`, `?` plus closing delimiters) and groups them
//! into paragraphs (separated by blank lines or Markdown headings).
//! Handles a small set of known abbreviations to avoid spurious splits.

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
fn sentence_ends(line: &str) -> Vec<(usize, usize)> {
    let mut ends: Vec<(usize, usize)> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        match chars[i] {
            '.' | '!' | '?' => {
                let mut end = i + 1;
                // Consume consecutive terminators: "..." "?!?!"
                while end < chars.len() && matches!(chars[end], '.' | '!' | '?') {
                    end += 1;
                }
                // Consume closing delimiters
                while end < chars.len()
                    && CLOSING_DELIMS.contains(&chars[end])
                {
                    end += 1;
                }

                // Check if this is a known abbreviation (e.g. "e.g.")
                let before: String = chars[..i].iter().collect();
                let is_abbrev = ABBREVIATIONS.iter().any(|abbr| {
                    before.ends_with(abbr)
                });

                if !is_abbrev {
                    ends.push((i, end));
                }
                i = end;
                continue;
            }
            _ => {
                i += 1;
            }
        }
    }

    ends
}

/// Group sentences into paragraphs. A new paragraph starts after a blank
/// line or a Markdown heading line.
pub fn segment_paragraphs(
    text: &str,
    sentences: &[Sentence],
) -> Vec<Paragraph> {
    let lines: Vec<&str> = text.split('\n').collect();

    // Find paragraph boundaries: a paragraph spans a run of consecutive
    // non-blank, non-heading lines.
    let mut paragraphs: Vec<(usize, usize)> = Vec::new(); // (start_line_idx, end_line_idx)
    let mut start: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            if let Some(s) = start.take() {
                paragraphs.push((s, i - 1));
            }
            continue;
        }
        if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start.take() {
        paragraphs.push((s, lines.len().saturating_sub(1)));
    }

    // Assign sentences to the paragraph whose line range contains the
    // sentence's starting line.
    let mut result: Vec<Paragraph> = Vec::new();
    for &(para_start, para_end) in &paragraphs {
        let mut psents: Vec<Sentence> = Vec::new();
        for s in sentences {
            // s.line is 1-based; para_start/para_end are 0-based
            if s.line >= para_start + 1 && s.line <= para_end + 1 {
                psents.push(s.clone());
            }
        }
        if !psents.is_empty() {
            result.push(Paragraph {
                sentences: psents,
                line: para_start + 1,
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
    fn empty_text() {
        let sents = segment_sentences("");
        assert!(sents.is_empty());
    }
}
