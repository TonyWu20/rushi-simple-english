//! Core lint orchestration.
//!
//! `lint()` extracts prose from the source text based on the kind.
//! It segments sentences and paragraphs. It runs the enabled rules
//! that apply to the kind. It collects violations into a report.
//! The semicolon rule runs only for the prose kinds. It skips source
//! kinds because a semicolon there is code punctuation.

use crate::rules;
use crate::sentences::{segment_paragraphs, segment_sentences};
use crate::types::{LintConfig, LintKind, Severity, Violation};
pub use crate::types::{LintReport, LintSummary};

/// The default maximum words per sentence.
pub const DEFAULT_MAX_SENTENCE_WORDS: usize = 25;

/// Lint `text` of the given `kind`, returning a report of all violations.
pub fn lint(kind: LintKind, text: &str, config: &LintConfig) -> LintReport {
    let max_sentence_words = config
        .max_sentence_words
        .unwrap_or(DEFAULT_MAX_SENTENCE_WORDS);

    // 1. Extract prose lines from the source.
    let mut prose_lines = extract_prose_lines(kind, text);

    // 1b. Exempt block-quoted lines when the option is set.
    if config.exempt_block_quotes {
        for line in prose_lines.iter_mut() {
            if line.trim_start().starts_with('>') {
                *line = " ".repeat(line.len());
            }
        }
    }

    if prose_lines.is_empty() {
        return LintReport {
            violations: Vec::new(),
            summary: LintSummary { total: 0, hard: 0 },
        };
    }

    // 2. Segment sentences and paragraphs.
    // Source kinds keep each extracted comment line its own sentence.
    // Merging would let a run of period-free comment lines grow into
    // one giant pseudo-sentence.
    let prose_text = prose_lines.join("\n");
    let merge_lines = !matches!(kind, LintKind::SlashSource | LintKind::HashSource);
    let sentences = segment_sentences(&prose_text, merge_lines);
    let paragraphs = segment_paragraphs(&prose_text, &sentences);

    // 3. Run each enabled rule and collect violations.
    let mut violations: Vec<Violation> = Vec::new();

    // Sentence-length
    if let Some(sev) = config.resolve_rule("sentence-length") {
        violations.extend(check_sentence_length(&sentences, max_sentence_words, sev));
    }

    // Paragraph-length. The cap is configurable via
    // `max_paragraph_sentences`. Source kinds default off, because
    // their comments are sparse. Prose kinds default to 6.
    if let Some(sev) = config.resolve_rule("paragraph-length") {
        let source_kind = matches!(kind, LintKind::SlashSource | LintKind::HashSource);
        let max = config
            .max_paragraph_sentences
            .unwrap_or_else(|| if source_kind { 0 } else { 6 });
        if max > 0 {
            violations.extend(check_paragraph_length(&paragraphs, max, sev));
        }
    }

    // Contraction
    if let Some(sev) = config.resolve_rule("contraction") {
        violations.extend(rules::check_contractions(&prose_lines, sev));
    }

    // Semicolon. Runs for the prose kinds only. In source files a
    // semicolon is code punctuation, so it stays out of reports even
    // when it sits inside a comment.
    if matches!(kind, LintKind::ProseFile | LintKind::CommitMessage) {
        if let Some(sev) = config.resolve_rule("semicolon") {
            violations.extend(rules::check_semicolons(&prose_lines, sev));
        }
    }

    // Phrasal verbs
    if let Some(sev) = config.resolve_rule("phrasal-verb") {
        violations.extend(rules::check_phrasal_verbs(&prose_lines, sev));
    }

    // Hedging
    if let Some(sev) = config.resolve_rule("hedging") {
        violations.extend(rules::check_hedging(&prose_lines, sev));
    }

    // Marketing
    if let Some(sev) = config.resolve_rule("marketing") {
        violations.extend(rules::check_marketing(&prose_lines, sev));
    }

    // STE dictionary
    if let Some(sev) = config.resolve_rule("dictionary-not-approved-word") {
        violations.extend(rules::check_ste_dictionary(&prose_lines, sev));
    }

    // Suppression directive validation
    if let Some(sev) = config.resolve_rule("invalid-suppression") {
        violations.extend(rules::check_suppression(
            &prose_lines,
            rules::REGISTERED_RULE_IDS,
            sev,
        ));
    }

    // 4. Filter out suppressed violations.
    let suppressed = suppressed_rule_ids_by_line(&prose_lines);
    violations.retain(|v| {
        !suppressed
            .get(&v.line)
            .map(|ids| ids.contains(&v.rule_id))
            .unwrap_or(false)
    });

    // 5. Sort by line, then column.
    violations.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.column.cmp(&b.column)));

    let hard = violations
        .iter()
        .filter(|v| v.severity == Severity::Hard)
        .count();
    let total = violations.len();

    LintReport {
        violations,
        summary: LintSummary { total, hard },
    }
}

/// Extract prose lines from the source text based on the content kind.
///
/// Prose and commit-message kinds blank out fenced code blocks, inline
/// code spans, and markdown table rows so the prose rules skip
/// non-prose structure. Blanked lines keep the line count and byte
/// layout, so a `Violation.line` still names the source line. Exposed
/// so `diff` can diff in the same prose coordinate system that the
/// engine uses for `Violation.line`.
pub(crate) fn extract_prose_lines(kind: LintKind, text: &str) -> Vec<String> {
    match kind {
        LintKind::ProseFile | LintKind::CommitMessage => {
            let lines: Vec<String> = text.lines().map(String::from).collect();
            let lines = blank_out_non_prose(&lines);
            blank_out_markdown_tables(&lines)
        }
        LintKind::SlashSource => extract_slash_comments(text),
        LintKind::HashSource => extract_hash_comments(text),
    }
}

/// Blank out the lines that hold no prose.
///
/// Four kinds of lines are blanked, in order of precedence:
///
/// - Fenced code blocks. A fence line has at most three leading
///   spaces, then three or more backticks or tildes. A backtick
///   opening fence does not open when its info string holds a
///   backtick. A closing fence uses the same character, has at
///   least the opening run length, and holds no text after the run.
/// - HTML comments. A line that starts with `<!--` opens a comment.
///   The comment closes on the first line holding `-->`. Comment
///   lines are blanked in full. A mixed line (prose plus comment) is
///   kept as prose.
/// - Indented code blocks. A line with four or more leading spaces
///   outside a fence is code, not prose.
///
/// Everything else is processed for inline backtick code spans. Each
/// blanked line keeps its byte length, so rule line and column
/// offsets stay aligned with the source text.
fn blank_out_non_prose(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut open: Option<(char, usize)> = None; // (fence char, run length)
    let mut in_html_comment = false;
    for line in lines {
        let blank = " ".repeat(line.len());
        let Some((ch, run)) = fence_run(line) else {
            if open.is_none() {
                if in_html_comment {
                    if line.contains("-->") {
                        in_html_comment = false;
                    }
                    out.push(blank);
                    continue;
                }
                if line.trim_start().starts_with("<!--") {
                    if !line.contains("-->") {
                        in_html_comment = true;
                    }
                    out.push(blank);
                    continue;
                }
                let indent = line.len() - line.trim_start().len();
                if indent >= 4 {
                    out.push(blank);
                    continue;
                }
                out.push(blank_out_inline_spans(line));
            } else {
                out.push(blank);
            }
            continue;
        };
        match open {
            None => {
                let rest = &line.trim_start()[run..];
                if ch == '`' && rest.contains('`') {
                    // Not a fence: a backtick in the info string.
                    out.push(line.clone());
                } else {
                    open = Some((ch, run));
                    out.push(blank);
                }
            }
            Some((och, on)) => {
                let rest = &line.trim_start()[run..];
                let closes = ch == och && run >= on && rest.trim().is_empty();
                if closes {
                    open = None;
                }
                out.push(blank);
            }
        }
    }
    out
}

/// The leading backtick or tilde run when the line is a fence line.
///
/// The line has at most three leading spaces, then three or more
/// backticks or tildes.
fn fence_run(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if indent > 3 {
        return None;
    }
    let mut chars = trimmed.chars();
    let ch = chars.next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let run = 1 + chars.take_while(|&c| c == ch).count();
    if run >= 3 {
        Some((ch, run))
    } else {
        None
    }
}

/// Replace single-line backtick code spans with spaces.
///
/// A span starts at a backtick run and ends at the next run of the
/// same length. An escaped backtick never opens or closes a span.
/// An open span does not cross a line break, so its backticks stay
/// literal. Each span keeps its byte length, so rule column offsets
/// stay aligned with the source line.
fn blank_out_inline_spans(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out: Vec<u8> = bytes.to_vec();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let open = i;
        let mut run = 0usize;
        while i + run < bytes.len() && bytes[i + run] == b'`' {
            run += 1;
        }
        if bytes[open.saturating_sub(1)] == b'\\' {
            i += 1;
            continue;
        }
        let mut j = i + run;
        while j < bytes.len() {
            if bytes[j] != b'`' {
                j += 1;
                continue;
            }
            let close = j;
            let mut close_run = 0usize;
            while close + close_run < bytes.len() && bytes[close + close_run] == b'`' {
                close_run += 1;
            }
            if close_run == run && bytes[close.saturating_sub(1)] != b'\\' {
                for k in open..close + close_run {
                    out[k] = b' ';
                }
                i = close + close_run;
                break;
            }
            j = close + close_run;
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| line.to_string())
}

/// Blank out markdown table lines (header, delimiter, and data rows).
///
/// Table rows hold no prose, and a long row of cells can exceed the
/// sentence-length limit even though every cell is short, so tables are
/// excluded from prose linting the way fenced code is. A table block is
/// a header row that contains a pipe, followed immediately by a
/// delimiter row (only pipes, dashes, colons, and spaces, with at
/// least one dash), and then any consecutive data rows that contain a
/// pipe. Each blanked line keeps its byte length, so rule column
/// offsets stay aligned with the source line.
fn blank_out_markdown_tables(lines: &[String]) -> Vec<String> {
    let mut blank = vec![false; lines.len()];
    let mut i = 0usize;
    while i < lines.len() {
        let is_delim = is_table_delimiter_row(&lines[i]);
        let has_header = i > 0 && is_table_row(&lines[i - 1]);
        if is_delim && has_header {
            blank[i - 1] = true;
            blank[i] = true;
            i += 1;
            while i < lines.len() && is_table_row(&lines[i]) && !is_table_delimiter_row(&lines[i]) {
                blank[i] = true;
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    lines
        .iter()
        .enumerate()
        .map(|(idx, line)| {
            if blank[idx] {
                " ".repeat(line.len())
            } else {
                line.clone()
            }
        })
        .collect()
}

/// A GFM table row: up to three leading spaces, then a pipe anywhere in
/// the row.
fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim_start();
    let leading = line.len() - trimmed.len();
    leading <= 3 && trimmed.contains('|')
}

/// A GFM table delimiter row, like `| --- | :---: |`.
fn is_table_delimiter_row(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed.contains('-')
        && trimmed
            .chars()
            .all(|c| matches!(c, '|' | ':' | '-' | ' '))
}

/// Extract `//` and `/* */` comment text from source code.
///
/// A `//` inside a string literal does not open a comment. The scan
/// tracks string state, so `http://x` in a string stays code.
fn extract_slash_comments(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if in_block {
            if let Some(end) = trimmed.find("*/") {
                let comment_text = trimmed[..end + 2].trim();
                if !comment_text.is_empty() {
                    out.push(comment_text.to_string());
                }
                in_block = false;
            } else {
                out.push(trimmed.to_string());
            }
            continue;
        }
        if let Some(pos) = find_comment_start(trimmed) {
            let comment = &trimmed[pos + 2..].trim();
            if !comment.is_empty() {
                out.push(comment.to_string());
            }
        }
        if trimmed.starts_with("/*") {
            if !trimmed.contains("*/") {
                in_block = true;
                // Remove the opening `/*`
                let after_open = &trimmed[2..];
                let body = after_open.trim();
                if !body.is_empty() {
                    out.push(body.to_string());
                }
            } else {
                // Single-line block comment: /* text */
                let content = &trimmed[2..];
                if let Some(close_pos) = content.find("*/") {
                    let body = content[..close_pos].trim();
                    if !body.is_empty() {
                        out.push(body.to_string());
                    }
                }
            }
        }
    }
    out
}

/// Find the `//` comment start on a source line.
///
/// A `//` inside a string literal is not a comment. The scan tracks
/// string state, so `http://x` in a string does not open a comment.
/// Char literals hold one char only. They cannot hold two slashes.
/// They need no tracking.
fn find_comment_start(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        let c = bytes[i];
        if in_string {
            match c {
                b'\\' => i += 2,
                b'"' => {
                    i += 1;
                    in_string = false;
                }
                _ => i += 1,
            }
            continue;
        }
        match c {
            b'"' => {
                in_string = true;
                i += 1;
            }
            b'/' if bytes[i + 1] == b'/' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Extract `#` comment lines from source code.
///
/// A shebang line (`#!`) is code, not prose. It is skipped, so its
/// `!` never acts as a sentence terminator.
fn extract_hash_comments(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("#!") {
                // A shebang. Code, not prose.
                None
            } else if trimmed.starts_with('#') {
                let comment = &trimmed[1..].trim();
                Some(comment.to_string())
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Check sentence-length violations.
fn check_sentence_length(
    sentences: &[crate::types::Sentence],
    max_words: usize,
    severity: Severity,
) -> Vec<Violation> {
    sentences
        .iter()
        .filter(|s| s.word_count > max_words)
        .map(|s| {
            let snippet: String = s
                .text
                .chars()
                .take(60)
                .collect::<String>()
                + if s.text.chars().count() > 60 { "..." } else { "" };
            Violation {
                rule_id: "sentence-length",
                severity,
                message: format!(
                    "Sentence has {} words; the maximum is {}. \"{}\"",
                    s.word_count, max_words, snippet
                ),
                suggestions: None,
                line: s.line,
                column: s.column,
            }
        })
        .collect()
}

/// Check paragraph-length violations. `max_sentences` is the
/// per-paragraph cap. A cap of zero disables the check.
fn check_paragraph_length(
    paragraphs: &[crate::types::Paragraph],
    max_sentences: usize,
    severity: Severity,
) -> Vec<Violation> {
    if max_sentences == 0 {
        return Vec::new();
    }
    paragraphs
        .iter()
        .filter(|p| p.sentences.len() > max_sentences)
        .map(|p| Violation {
            rule_id: "paragraph-length",
            severity,
            message: format!(
                "Paragraph has {} sentences; the maximum is {}.",
                p.sentences.len(),
                max_sentences
            ),
            suggestions: None,
            line: p.line,
            column: p.column,
        })
        .collect()
}

/// Parse `ste-disable-next-line` directives and return a map of
/// line-number (1-based) to the set of suppressed rule IDs.
fn suppressed_rule_ids_by_line(
    lines: &[String],
) -> std::collections::HashMap<usize, std::collections::HashSet<&'static str>> {
    let mut result: std::collections::HashMap<usize, std::collections::HashSet<&'static str>> =
        std::collections::HashMap::new();

    for (i, line) in lines.iter().enumerate() {
        if !line.contains("ste-disable-next-line") {
            continue;
        }
        let after_marker = line
            .split_once("ste-disable-next-line")
            .map(|(_, rest)| rest)
            .unwrap_or("");
        let names: Vec<&str> = after_marker
            .split(|c: char| c.is_whitespace() || c == ',' || c == ':')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        // The directive suppresses rules on the NEXT line (1-based).
        let target_line = i + 2;
        let entry = result.entry(target_line).or_default();
        for name in names {
            let static_id: Option<&'static str> = match name {
                "contraction" => Some("contraction"),
                "dictionary-not-approved-word" => Some("dictionary-not-approved-word"),
                "hedging" => Some("hedging"),
                "invalid-suppression" => Some("invalid-suppression"),
                "marketing" => Some("marketing"),
                "paragraph-length" => Some("paragraph-length"),
                "phrasal-verb" => Some("phrasal-verb"),
                "semicolon" => Some("semicolon"),
                "sentence-length" => Some("sentence-length"),
                "verb-progressive" => Some("verb-progressive"),
                "verb-passive" => Some("verb-passive"),
                "verb-perfect" => Some("verb-perfect"),
                _ => None,
            };
            if let Some(id) = static_id {
                entry.insert(id);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LintConfig;

    #[test]
    fn lint_clean_prose() {
        let config = LintConfig::default();
        let report = lint(
            LintKind::ProseFile,
            "This is a clean sentence. It is short.",
            &config,
        );
        assert_eq!(report.summary.total, 0);
    }

    #[test]
    fn lint_detects_semicolon() {
        let config = LintConfig::default();
        let report = lint(LintKind::ProseFile, "One; two.", &config);
        assert!(report.summary.hard >= 1);
        assert!(report
            .violations
            .iter()
            .any(|v| v.rule_id == "semicolon"));
    }

    #[test]
    fn lint_detects_long_sentence() {
        let config = LintConfig::default();
        let long = "This is a sentence that has many words and it definitely exceeds the twenty five word limit that is set by default in the configuration file for this tool";
        let report = lint(LintKind::ProseFile, &format!("{}. ", long), &config);
        assert!(report
            .violations
            .iter()
            .any(|v| v.rule_id == "sentence-length"));
    }

    #[test]
    fn lint_detects_phrasal_verb() {
        let config = LintConfig::default();
        let report = lint(
            LintKind::ProseFile,
            "We will kick off the project.",
            &config,
        );
        assert!(report
            .violations
            .iter()
            .any(|v| v.rule_id == "phrasal-verb"));
    }

    #[test]
    fn lint_off_rule_suppresses() {
        let mut config = LintConfig::default();
        config
            .rules
            .insert("semicolon".to_string(), "off".to_string());
        let report = lint(LintKind::ProseFile, "One; two.", &config);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.rule_id == "semicolon")
        );
    }

    #[test]
    fn lint_commit_message_kind() {
        let config = LintConfig::default();
        let report = lint(LintKind::CommitMessage, "fix; update thing", &config);
        assert!(report
            .violations
            .iter()
            .any(|v| v.rule_id == "semicolon"));
    }

    #[test]
    fn lint_semicolon_is_skipped_for_slash_source() {
        // A semicolon in Rust code or in a Rust comment must not
        // fire. The agent writes Rust with `;` all the time.
        let config = LintConfig::default();
        let text = "fn main() {\n    let a = 1; // one; two\n}\n";
        let report = lint(LintKind::SlashSource, text, &config);
        assert!(
            !report.violations.iter().any(|v| v.rule_id == "semicolon"),
            "{report:?}"
        );
    }

    #[test]
    fn lint_semicolon_is_skipped_for_hash_source() {
        // A semicolon in a `#` comment must not fire either.
        let config = LintConfig::default();
        let text = "x = 1\n# one; two\n";
        let report = lint(LintKind::HashSource, text, &config);
        assert!(
            !report.violations.iter().any(|v| v.rule_id == "semicolon"),
            "{report:?}"
        );
    }

    #[test]
    fn lint_slash_url_string_is_exempt() {
        // A URL in a string literal is code. The `//` in it must not
        // open a comment. No prose comes out of this line at all.
        let config = LintConfig::default();
        let text = "fn f() {\n    let u = \"http://x.example/a\";\n}\n";
        let report = lint(LintKind::SlashSource, text, &config);
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn lint_slash_comment_after_string_is_prose() {
        // A `//` after the closing quote still opens a comment.
        // An escaped quote in the string must not break the scan.
        let config = LintConfig::default();
        let text = "fn f() {\n    let s = \"a\\\"b\"; // kick off\n}\n";
        let report = lint(LintKind::SlashSource, text, &config);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.rule_id == "phrasal-verb"),
            "{report:?}"
        );
    }

    #[test]
    fn slash_source_prose_rules_still_apply() {
        // The skip is semicolon-only. Other prose rules still run on
        // source comments, so a phrasal verb in one still fires.
        let config = LintConfig::default();
        let text = "fn main() {\n    // kick off the build\n}\n";
        let report = lint(LintKind::SlashSource, text, &config);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.rule_id == "phrasal-verb"),
            "{report:?}"
        );
    }

    #[test]
    fn lint_fenced_code_is_exempt() {
        let config = LintConfig::default();
        let text = "Good prose here.\n```rust\nlet a = 1; // one\nlet b = a; // two\n```\nMore prose after the fence.";
        let report = lint(LintKind::ProseFile, text, &config);
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn lint_semicolon_outside_fence_is_reported_with_source_line() {
        let config = LintConfig::default();
        let text = "```sh\necho hi; echo bye\n```\nOne; two.";
        let report = lint(LintKind::ProseFile, text, &config);
        let semi: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.rule_id == "semicolon")
            .collect();
        assert_eq!(semi.len(), 1, "{report:?}");
        assert_eq!(semi[0].line, 4, "{report:?}");
    }

    #[test]
    fn lint_tilde_fence_is_exempt() {
        let config = LintConfig::default();
        let text = "Intro.\n~~~\nx = 1;\n~~~\nOutro.";
        let report = lint(LintKind::ProseFile, text, &config);
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn lint_longer_close_fence_is_needed() {
        let config = LintConfig::default();
        let text = "````\n```\na = 1;\n````\nTail.";
        let report = lint(LintKind::ProseFile, text, &config);
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn lint_unclosed_fence_blanks_to_end() {
        let config = LintConfig::default();
        let text = "Intro.\n```python\nx = 1;\ny = 2;\n";
        let report = lint(LintKind::ProseFile, text, &config);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.rule_id == "semicolon"),
            "{report:?}"
        );
    }

    #[test]
    fn lint_markdown_table_is_exempt() {
        // A GFM table row with many cells must not count as one long
        // sentence: blanking the table block keeps prose linting clean.
        let config = LintConfig::default();
        let table = concat!(
            "Intro prose.\n",
            "| Document | Status | Updated | Notes |\n",
            "|---|---|---|---|\n",
            "| `a-plan.md` | Implemented | 2026-09-13 | Option A worker plus Option B memo, staged with test gates |\n",
            "| `b-audit.md` | Audit | 2026-09-13 | Independent perf test for the frame budget, passing |\n",
            "Outro prose.\n",
        );
        let report = lint(LintKind::ProseFile, table, &config);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.rule_id == "sentence-length"),
            "{report:?}"
        );
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.rule_id == "paragraph-length"),
            "{report:?}"
        );
    }

    #[test]
    fn lint_indented_code_is_exempt() {
        // An indented code block (four leading spaces) is code, not
        // prose. Semicolons in it must not fire.
        let config = LintConfig::default();
        let text = "Intro.\n    let a = 1; let b = 2; let c = 3; let d = 4;\nOutro.\n";
        let report = lint(LintKind::ProseFile, text, &config);
        let semi: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.rule_id == "semicolon")
            .collect();
        assert!(semi.is_empty(), "{semi:?}");
    }

    #[test]
    fn lint_html_comment_is_exempt() {
        // HTML comments are structure, not prose. Long comments must
        // not trip sentence-length.
        let config = LintConfig::default();
        let text = "Intro.\n<!-- This comment is long enough to exceed the sentence length limit when counted as prose words here -->\nOutro.\n";
        let report = lint(LintKind::ProseFile, text, &config);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.rule_id == "sentence-length"),
            "{report:?}"
        );
    }

    #[test]
    fn lint_multi_line_html_comment_is_exempt() {
        let config = LintConfig::default();
        let text = "Intro.\n<!--\nmulti line comment with; semicolons inside\n-->\nOutro.\n";
        let report = lint(LintKind::ProseFile, text, &config);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.rule_id == "semicolon"),
            "{report:?}"
        );
    }

    #[test]
    fn lint_table_inside_fence_is_not_blanked() {
        // Pipe rows inside a fenced code block are code, not a table:
        // the fence blanking already removed them, so no rule fires.
        let config = LintConfig::default();
        let text = "```md\n| a | b c d e f g h i j k l m n o p q r |\n|---|---|\n```\n\nOne; two.\n";
        let report = lint(LintKind::ProseFile, text, &config);
        let semi: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.rule_id == "semicolon")
            .collect();
        assert_eq!(semi.len(), 1, "{report:?}");
    }

    #[test]
    fn lint_prose_line_with_single_pipe_is_not_blanked() {
        // A prose line that merely contains a pipe (no delimiter row)
        // stays prose: semicolons in it are still reported.
        let config = LintConfig::default();
        let text = "The value a|b here; one more.\nShort tail.\n";
        let report = lint(LintKind::ProseFile, text, &config);
        let semi: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.rule_id == "semicolon")
            .collect();
        assert_eq!(semi.len(), 1, "{report:?}");
    }

    #[test]
    fn lint_indented_fence_is_exempt() {
        let config = LintConfig::default();
        let text = "Intro.\n   ```\nx = 1;\n   ```\n";
        let report = lint(LintKind::ProseFile, text, &config);
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn lint_backtick_in_info_string_is_not_a_fence() {
        let config = LintConfig::default();
        let text = "```a`b\ncode; here\n";
        let report = lint(LintKind::ProseFile, text, &config);
        let semi: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.rule_id == "semicolon")
            .collect();
        assert_eq!(semi.len(), 1, "the line is prose, not a fence: {report:?}");
    }

    #[test]
    fn lint_commit_message_fence_is_exempt() {
        let config = LintConfig::default();
        let text = "Fix the shell loop.\n\n```sh\nfor i in 1 2; do echo $i; done\n```";
        let report = lint(LintKind::CommitMessage, text, &config);
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn lint_inline_code_span_is_exempt() {
        let config = LintConfig::default();
        let text = "Use `let x = 1;` now.";
        let report = lint(LintKind::ProseFile, text, &config);
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn lint_inline_span_keeps_prose_violations() {
        let config = LintConfig::default();
        let text = "Use `let x = 1;` now. One; two.";
        let report = lint(LintKind::ProseFile, text, &config);
        let semi: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.rule_id == "semicolon")
            .collect();
        assert_eq!(semi.len(), 1, "{report:?}");
        assert_eq!(semi[0].line, 1, "{report:?}");
        assert_eq!(semi[0].column, 26, "{report:?}");
    }

    #[test]
    fn lint_double_backtick_span_is_exempt() {
        let config = LintConfig::default();
        let text = "Run ``a; b`` now.";
        let report = lint(LintKind::ProseFile, text, &config);
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn lint_open_span_does_not_cross_lines() {
        let config = LintConfig::default();
        let text = "Use `let x = 1;\nThen go.";
        let report = lint(LintKind::ProseFile, text, &config);
        let semi: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.rule_id == "semicolon")
            .collect();
        assert_eq!(semi.len(), 1, "{report:?}");
        assert_eq!(semi[0].line, 1, "{report:?}");
        assert_eq!(semi[0].column, 15, "{report:?}");
    }

    #[test]
    fn lint_escaped_backtick_is_not_a_span() {
        let config = LintConfig::default();
        let text = "Use \\`x\\`; here.";
        let report = lint(LintKind::ProseFile, text, &config);
        let semi: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.rule_id == "semicolon")
            .collect();
        assert_eq!(semi.len(), 1, "{report:?}");
        assert_eq!(semi[0].column, 10, "{report:?}");
    }

    #[test]
    fn lint_span_in_fence_line_is_not_blanked() {
        let config = LintConfig::default();
        let text = "```sh\necho `x`; done\n```";
        let report = lint(LintKind::ProseFile, text, &config);
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn lint_suppression_directive() {
        let config = LintConfig::default();
        let text = "One; two.\n// ste-disable-next-line semicolon\nThree; four.";
        let report = lint(LintKind::ProseFile, text, &config);
        let semi: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.rule_id == "semicolon")
            .collect();
        // The semicolon on line 3 is suppressed by the directive on line 2;
        // the semicolon on line 1 is NOT suppressed.
        assert_eq!(semi.len(), 1);
        assert_eq!(semi[0].line, 1);
    }

    #[test]
    fn hash_source_shebang_and_dotted_code_are_excluded() {
        let config = LintConfig::default();
        let text = "#!/bin/sh\n# Watch the run.idle window now.\n# Bind 127.0.0.1 and v1.2 here.\n";
        let report = lint(LintKind::HashSource, text, &config);
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn commit_body_with_dotted_identifier_passes() {
        let config = LintConfig::default();
        let report = lint(LintKind::CommitMessage, "Fix the run.idle continue path", &config);
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn hash_source_many_comments_pass_paragraph_length() {
        // 20 comment lines, 8 with a dotted identifier. The source
        // kinds default the paragraph cap off, so the file passes.
        let config = LintConfig::default();
        let lines: Vec<String> = (0..20)
            .map(|i| {
                if i < 12 {
                    "# Watch the window state now.".to_string()
                } else {
                    format!("# The run.idle window {i} is open.")
                }
            })
            .collect();
        let text = lines.join("\n");
        let report = lint(LintKind::HashSource, &text, &config);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.rule_id == "paragraph-length"),
            "{report:?}"
        );
    }

    #[test]
    fn source_comment_lines_do_not_merge_into_pseudo_sentences() {
        // A run of period-free comment lines must not build one long
        // pseudo-sentence. Source kinds close each comment line as
        // its own sentence.
        let config = LintConfig::default();
        let lines: Vec<String> = (0..3)
            .map(|i| format!("# one two three four five six seven eight nine ten eleven twelve {i}"))
            .collect();
        let text = lines.join("\n");
        let report = lint(LintKind::HashSource, &text, &config);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.rule_id == "sentence-length"),
            "{report:?}"
        );
    }

    #[test]
    fn paragraph_cap_is_configurable() {
        let mut config = LintConfig::default();
        let text = "One. Two. Three. Four. Five. Six. Seven. Eight.\n";
        // Prose kinds default to a cap of 6.
        let report = lint(LintKind::ProseFile, text, &config);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.rule_id == "paragraph-length"),
            "{report:?}"
        );
        config.max_paragraph_sentences = Some(8);
        let report = lint(LintKind::ProseFile, text, &config);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.rule_id == "paragraph-length"),
            "{report:?}"
        );
        // A cap of zero disables the rule.
        config.max_paragraph_sentences = Some(0);
        let report = lint(LintKind::ProseFile, text, &config);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.rule_id == "paragraph-length"),
            "{report:?}"
        );
    }

    #[test]
    fn source_kind_paragraph_cap_is_off_by_default() {
        let config = LintConfig::default();
        let text = "# One. # Two. # Three. # Four. # Five. # Six. # Seven. # Eight.\n";
        let report = lint(LintKind::HashSource, text, &config);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.rule_id == "paragraph-length"),
            "{report:?}"
        );
    }
}
