//! Sentence and paragraph segmentation for the simple-english lint
//! engine.
//!
//! Simplified from the TypeScript original. Splits text into sentences
//! (delimited by `.`, `!`, `?` plus closing delimiters) and groups them
//! into paragraphs. Paragraphs break at blank lines and at markdown
//! block boundaries (headings, list items, thematic breaks). Handles a
//! small set of known abbreviations to avoid spurious splits.

use crate::types::{Paragraph, Sentence};

/// Known abbreviations that should not trigger a sentence boundary.
const ABBREVIATIONS: &[&str] = &[
    "e.g.", "i.e.", "etc.", "vs.", "Fig.", "No.",
];

/// Closing delimiters that can follow a sentence-ending punctuation mark.
const CLOSING_DELIMS: &[char] = &[
    '"', '\u{201d}', '\u{00bb}', '\u{203a}', ')', ']', '}',
];

/// Segment `text` into sentences. Each sentence carries its 1-based
/// line number, 1-based column of its first non-whitespace character,
/// and the word count.
///
/// When `merge_lines` is false, each line closes its own sentence.
/// The source kinds pass false. Their comment lines stay separate, so
/// a long run of period-free lines cannot build one giant sentence.
pub fn segment_sentences(text: &str, merge_lines: bool) -> Vec<Sentence> {
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
            if let Some((l, c)) = open {
                close_sentence(&mut sentences, &mut parts, l, c);
            }
            open = None;
            continue;
        }

        let (kind, content_start) = block_analysis(line);

        // A thematic break or setext underline carries no prose. Treat
        // it like a blank line: close the open sentence and start
        // fresh.
        if kind == BlockKind::ThematicBreak {
            if let Some((l, c)) = open {
                close_sentence(&mut sentences, &mut parts, l, c);
            }
            open = None;
            continue;
        }

        // A new block must not accumulate into the open sentence. Close
        // the open sentence first so this line starts its own sentence.
        // Without this, consecutive no-period list items merge into one
        // long pseudo-sentence.
        if kind != BlockKind::None && !parts.is_empty() {
            let (sl, sc) = open.unwrap_or((i, 0));
            close_sentence(&mut sentences, &mut parts, sl, sc);
            open = None;
        }

        // Strip the block marker (`#`, `- `, `1. `) so it does not
        // count as a word and its `.` does not act as a terminator.
        let content = &line[content_start..];
        let marker_len = content_start;

        let trimmed = content.trim_start();
        let leading_ws = marker_len + (content.len() - trimmed.len());
        if open.is_none() {
            open = Some((i, leading_ws));
        }

        let mut last_end = 0usize;
        for (_start, end) in sentence_ends(content) {
            let part = &content[last_end..end];
            if !part.trim().is_empty() {
                parts.push(part.trim().to_string());
            }
            let start_line = open.map(|(l, _)| l).unwrap_or(i);
            let start_col = open.map(|(_, c)| c).unwrap_or(marker_len + last_end);
            close_sentence(&mut sentences, &mut parts, start_line, start_col);
            open = None;
            last_end = end;
        }

        let rest = &content[last_end..];
        if !rest.trim().is_empty() {
            open = Some((i, marker_len + last_end + (rest.len() - rest.trim_start().len())));
            parts.push(rest.trim().to_string());
        }

        // A single-line block (a heading) must not continue onto the
        // next line. Close its sentence at end of line.
        if kind == BlockKind::SingleLine && !parts.is_empty() {
            let (sl, sc) = open.unwrap_or((i, 0));
            close_sentence(&mut sentences, &mut parts, sl, sc);
            open = None;
        }

        // Source kinds pass merge_lines = false. Each extracted
        // comment line closes its own sentence, so no run of
        // period-free lines grows into one giant pseudo-sentence.
        if !merge_lines && !parts.is_empty() {
            let (sl, sc) = open.unwrap_or((i, 0));
            close_sentence(&mut sentences, &mut parts, sl, sc);
            open = None;
        }
    }

    if let Some((l, c)) = open {
        close_sentence(&mut sentences, &mut parts, l, c);
    }

    sentences
}

/// A word character for the dotted-identifier guard: an ASCII letter,
/// a digit, or an underscore.
fn is_word_char(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
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
                // Dotted identifiers and numeric literals are code.
                // A dot with word characters on both sides is an
                // identifier separator, not a sentence end.
                // A dot after a digit is an IP or version piece.
                if ch == '.' {
                    let before = line[..start].chars().next_back();
                    let after = line[start + 1..].chars().next();
                    let identifier_dot =
                        before.is_some_and(is_word_char) && after.is_some_and(is_word_char);
                    let numeric_dot = before.is_some_and(|c| c.is_ascii_digit());
                    if identifier_dot || numeric_dot {
                        i += 1;
                        continue;
                    }
                }
                // A `#!` shebang at line start is code, not prose.
                let shebang = ch == '!' && start == 1 && line.starts_with("#!");
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

                if !is_abbrev && !shebang {
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
/// A paragraph begins on the first line, after any blank line, or on a
/// line that starts a markdown block (a heading, a list item, or a
/// thematic break). Consecutive non-boundary lines inside one block
/// form a single paragraph, so a wrapped paragraph stays whole while a
/// bulleted or numbered list becomes one paragraph per item.
///
/// Each returned `Paragraph` carries the 1-based line number of its
/// first non-blank line and the subset of `sentences` that start on
/// lines belonging to that paragraph.
pub fn segment_paragraphs(
    text: &str,
    sentences: &[Sentence],
) -> Vec<Paragraph> {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        return Vec::new();
    }

    // Assign each line to a paragraph index. A new paragraph starts at
    // line zero, after any blank line, or at a markdown block boundary.
    let mut para_of_line: Vec<usize> = Vec::with_capacity(lines.len());
    let mut current = 0usize;
    let mut prev_blank = true;
    for (i, line) in lines.iter().enumerate() {
        let blank = line.trim().is_empty();
        let new_block = !blank && block_analysis(line).0 != BlockKind::None;
        if i > 0 && (prev_blank || new_block) {
            current += 1;
        }
        para_of_line.push(current);
        prev_blank = blank;
    }

    let para_count = current + 1;

    // The 1-based first non-blank line number of each paragraph.
    let mut para_first_line = vec![0usize; para_count];
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let p = para_of_line[i];
        if para_first_line[p] == 0 {
            para_first_line[p] = i + 1;
        }
    }

    // Group sentences by the paragraph that owns their start line. A
    // sentence starts on the line holding its first non-blank
    // character.
    let mut groups: Vec<Vec<Sentence>> = vec![Vec::new(); para_count];
    for s in sentences {
        let start = s.line.saturating_sub(1);
        if start < lines.len() && !lines[start].trim().is_empty() {
            let p = para_of_line[start];
            groups[p].push(s.clone());
        }
    }

    groups
        .into_iter()
        .enumerate()
        .filter(|(_, g)| !g.is_empty())
        .map(|(p, group)| Paragraph {
            sentences: group,
            line: para_first_line[p],
            column: 1,
        })
        .collect()
}

/// The kind of markdown block a line starts, and where the block's
/// prose content begins (a byte offset into the line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    /// A regular prose line: a wrapped continuation of the previous
    /// block, or plain text.
    None,
    /// A single-line block (an ATX heading). Its sentence must close
    /// at end of line.
    SingleLine,
    /// A list item. Its content may continue on the next indented
    /// line, but the line starts a new block.
    Item,
    /// A thematic break or setext heading underline. It carries no
    /// prose and acts as a separator.
    ThematicBreak,
}

/// Classify the markdown block that `line` starts.
///
/// Returns the block kind and the byte offset where the block's prose
/// content begins (past any leading marker such as `#`, `- `, or
/// `1.`). At most three leading spaces are allowed, matching CommonMark
/// block indentation. These are structural boundaries, so an open
/// sentence and an open paragraph must close before such a line.
fn block_analysis(line: &str) -> (BlockKind, usize) {
    let t = line.trim_start();
    let indent = line.len() - t.len();
    if indent > 3 || t.is_empty() {
        return (BlockKind::None, 0);
    }

    // Thematic break or setext heading underline: three or more of the
    // same `-`, `_`, `*`, or `=`, with optional spaces between.
    let solid: Vec<char> = t.chars().filter(|c| !c.is_whitespace()).collect();
    if solid.len() >= 3
        && solid
            .iter()
            .all(|c| matches!(c, '-' | '_' | '*' | '='))
        && solid.iter().all(|c| c == &solid[0])
    {
        return (BlockKind::ThematicBreak, 0);
    }

    // Blockquote prefix: one or more `>`, then an optional space or
    // tab. A quote line continues the blockquote block rather than
    // starting a new one, so it is not a block boundary. The prefix is
    // stripped so it does not count as a word.
    if t.starts_with('>') {
        let quotes = t.chars().take_while(|&c| c == '>').count();
        let after = &t[quotes..];
        let space = after.starts_with(' ') || after.starts_with('\t');
        let skip = quotes + usize::from(space);
        return (BlockKind::None, indent + skip);
    }

    // ATX heading: one to six `#`, then a space, a tab, or end of
    // line.
    if t.starts_with('#') {
        let hashes = t.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) {
            let after = &t[hashes..];
            let space = after.starts_with(' ') || after.starts_with('\t');
            if after.is_empty() || space {
                let skip = hashes + usize::from(space);
                return (BlockKind::SingleLine, indent + skip);
            }
        }
        return (BlockKind::None, 0);
    }

    let first = t.chars().next().unwrap_or_default();

    // Bullet list item: `-`, `*`, or `+` followed by a space or tab.
    if matches!(first, '-' | '*' | '+') {
        let second = t.chars().nth(1);
        if matches!(second, Some(' ') | Some('\t')) {
            return (BlockKind::Item, indent + 2);
        }
        return (BlockKind::None, 0);
    }

    // Numbered list item: one to nine digits, then `.` or `)`, then a
    // space, a tab, or end of line.
    if first.is_ascii_digit() {
        let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
        if (1..=9).contains(&digits) {
            let marker = t.as_bytes()[digits];
            if marker == b'.' || marker == b')' {
                let follow = &t[digits + 1..];
                let space = follow.starts_with(' ') || follow.starts_with('\t');
                if follow.is_empty() || space {
                    let skip = digits + 1 + usize::from(space);
                    return (BlockKind::Item, indent + skip);
                }
            }
        }
        return (BlockKind::None, 0);
    }

    (BlockKind::None, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_simple_sentences() {
        let sents = segment_sentences("Hello world. How are you?", true);
        assert_eq!(sents.len(), 2);
        assert_eq!(sents[0].text, "Hello world.");
        assert_eq!(sents[1].text, "How are you?");
    }

    #[test]
    fn word_count() {
        let sents = segment_sentences("One two three.", true);
        assert_eq!(sents[0].word_count, 3);
    }

    #[test]
    fn multi_line_sentence() {
        let sents = segment_sentences(
            "This is a long sentence that\nspans multiple lines.",
            true,
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
        let sents = segment_sentences(text, true);
        let paras = segment_paragraphs(text, &sents);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].sentences.len(), 2);
        assert_eq!(paras[1].sentences.len(), 1);
    }

    #[test]
    fn paragraphs_crlf() {
        let text = "Line one.\r\nLine two.\r\n\r\nLine three.";
        let sents = segment_sentences(text, true);
        let paras = segment_paragraphs(text, &sents);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].line, 1);
        assert_eq!(paras[1].line, 4);
    }

    #[test]
    fn paragraphs_whitespace_only_line_breaks() {
        let text = "Para one.\n   \nPara two.";
        let sents = segment_sentences(text, true);
        let paras = segment_paragraphs(text, &sents);
        assert_eq!(paras.len(), 2);
    }

    #[test]
    fn paragraphs_multiple_sentences_line_numbers() {
        let text = "S one.\nS two.\n\nS three.\nS four.\nS five.";
        let sents = segment_sentences(text, true);
        let paras = segment_paragraphs(text, &sents);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].line, 1);
        assert_eq!(paras[0].sentences.len(), 2);
        assert_eq!(paras[1].line, 4);
        assert_eq!(paras[1].sentences.len(), 3);
    }

    #[test]
    fn empty_text() {
        let sents = segment_sentences("", true);
        assert!(sents.is_empty());
    }

    /// Regression test: a line containing multibyte characters (e.g. the
    /// ellipsis `…` and em-dash `—`) followed by a sentence terminator
    /// must not panic on a char-boundary violation, and must still split
    /// at the ASCII terminator after the multibyte run.
    #[test]
    fn multibyte_chars_before_terminator() {
        let line = "- **`table_grid`** (rewritten) \u{2014} wide cells wrap, truncated with `\u{2026}`. A data row spans.";
        let sents = segment_sentences(line, true);
        // Two sentences: the first ends at the '.' after the ellipsis,
        // the second is the tail. Before the byte-index fix this panicked
        // because the char index was sliced as a byte index mid-'…'.
        // The list marker `- ` is stripped from the sentence text.
        assert_eq!(sents.len(), 2);
        assert_eq!(
            sents[0].text,
            "**`table_grid`** (rewritten) \u{2014} wide cells wrap, truncated with `\u{2026}`."
        );
        assert_eq!(sents[1].text, "A data row spans.");
    }

    #[test]
    fn blockquote_prefix_is_stripped_from_sentence_text() {
        // A quoted line: the `>` prefix is stripped so it does not
        // count as a word. Consecutive quote lines accumulate into
        // one sentence (same as a wrapped paragraph).
        let text = "> quoted first line\n> second quoted line.\n";
        let sents = segment_sentences(text, true);
        assert_eq!(sents.len(), 1);
        assert_eq!(sents[0].text, "quoted first line second quoted line.");
    }

    #[test]
    fn cjk_sentence() {
        let sents = segment_sentences("你好世界。这是测试！", true);
        assert_eq!(sents.len(), 2);
        assert_eq!(sents[0].text, "你好世界。");
        assert_eq!(sents[1].text, "这是测试！");
    }

    #[test]
    fn consecutive_bullets_do_not_merge_into_one_sentence() {
        // Regression: no-period bullet lines must not accumulate into one
        // long pseudo-sentence across list items.
        let text = "- First item with no period\n- Second item with no period\n- Third item with no period\n";
        let sents = segment_sentences(text, true);
        // Each bullet is its own sentence (3), not one merged 21-word sentence.
        assert_eq!(sents.len(), 3);
        for s in &sents {
            assert!(s.word_count <= 6, "sentence grew too large: {} ({})", s.text, s.word_count);
        }
    }

    #[test]
    fn numbered_list_items_are_separate_paragraphs() {
        // Regression: consecutive numbered list items are separate blocks,
        // so a long numbered list must not read as one giant paragraph.
        let text = "1. First item passes.\n2. Second item passes.\n3. Third item passes.\n4. Fourth item passes.\n5. Fifth item passes.\n6. Sixth item passes.\n7. Seventh item passes.\n";
        let sents = segment_sentences(text, true);
        let paras = segment_paragraphs(text, &sents);
        assert!(paras.len() >= 7, "each list item should be its own paragraph, got {}", paras.len());
        assert!(paras
            .iter()
            .all(|p| p.sentences.len() <= 6), "no list paragraph exceeds the limit");
    }

    #[test]
    fn list_items_are_separate_paragraphs() {
        // Regression: consecutive bullets are separate blocks, so a 7-item
        // list must not read as one 7-sentence paragraph.
        let text = "- One.\n- Two.\n- Three.\n- Four.\n- Five.\n- Six.\n- Seven.\n";
        let sents = segment_sentences(text, true);
        let paras = segment_paragraphs(text, &sents);
        assert_eq!(paras.len(), 7);
        assert!(paras
            .iter()
            .all(|p| p.sentences.len() <= 6), "no list paragraph exceeds the limit");
    }

    #[test]
    fn heading_is_its_own_block_boundary() {
        // A heading after prose starts a fresh sentence, not a merge.
        // The `##` marker is stripped from the sentence text, and the
        // heading must not merge with the body line that follows it.
        let text = "Intro text.\n## Section heading with many trailing words here\nBody line after the heading follows here.\n";
        let sents = segment_sentences(text, true);
        assert_eq!(sents[0].text, "Intro text.");
        assert_eq!(sents.len(), 3);
        let heading = sents
            .iter()
            .find(|s| s.text.starts_with("Section heading"))
            .expect("heading sentence missing");
        // The marker is stripped and the heading must not merge with
        // the body line that follows it.
        assert!(!heading.text.contains("Body line"));
        assert_eq!(sents[2].text, "Body line after the heading follows here.");
    }

    #[test]
    fn dotted_identifiers_do_not_split() {
        // `run.idle` and `e2e.sh` are dotted identifiers. Their dots
        // carry word characters on both sides, so they do not end a
        // sentence. Only the trailing period splits.
        let sents = segment_sentences("Use run.idle and e2e.sh now.", true);
        assert_eq!(sents.len(), 1, "{sents:?}");
        assert_eq!(sents[0].text, "Use run.idle and e2e.sh now.");
    }

    #[test]
    fn numeric_literals_do_not_split() {
        // `127.0.0.1` and `v1.2` are numeric literals. A dot whose
        // left side ends in a digit is an IP or version piece, not a
        // terminator.
        let sents = segment_sentences("Bind 127.0.0.1 and v1.2 now.", true);
        assert_eq!(sents.len(), 1, "{sents:?}");
        assert_eq!(sents[0].text, "Bind 127.0.0.1 and v1.2 now.");
    }

    #[test]
    fn shebang_line_does_not_split() {
        // A `#!` shebang at line start is code. Its `!` does not
        // end a sentence: no terminator sits in the line at all.
        assert!(sentence_ends("#!/bin/sh").is_empty());
        let sents = segment_sentences("#!/bin/sh", true);
        assert_eq!(sents.len(), 1, "{sents:?}");
        assert_eq!(sents[0].text, "#!/bin/sh");
    }

    #[test]
    fn source_mode_keeps_comment_lines_separate() {
        // merge_lines = false: each line closes its own sentence. A
        // run of period-free lines must not build one giant
        // pseudo-sentence.
        let text = "alpha beta\ngamma delta\ndelta zeta.";
        let sents = segment_sentences(text, false);
        assert_eq!(sents.len(), 3, "{sents:?}");
        assert_eq!(sents[0].text, "alpha beta");
        let merged = segment_sentences(text, true);
        assert_eq!(merged.len(), 1, "{merged:?}");
    }
}
