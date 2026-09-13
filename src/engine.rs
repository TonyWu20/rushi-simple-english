//! Core lint orchestration.
//!
//! `lint()` extracts prose from the source text based on the content
//! kind, segments it into sentences and paragraphs, runs all enabled
//! rules, and collects violations into a `LintReport`.

use crate::rules;
use crate::sentences::{segment_paragraphs, segment_sentences};
use crate::types::{LintConfig, LintKind, Severity, Violation};

// Re-export key types for external consumers.
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
    let prose_text = prose_lines.join("\n");
    let sentences = segment_sentences(&prose_text);
    let paragraphs = segment_paragraphs(&prose_text, &sentences);

    // 3. Run each enabled rule and collect violations.
    let mut violations: Vec<Violation> = Vec::new();

    // Sentence-length
    if let Some(sev) = config.resolve_rule("sentence-length") {
        violations.extend(check_sentence_length(&sentences, max_sentence_words, sev));
    }

    // Paragraph-length
    if let Some(sev) = config.resolve_rule("paragraph-length") {
        violations.extend(check_paragraph_length(&paragraphs, sev));
    }

    // Contraction
    if let Some(sev) = config.resolve_rule("contraction") {
        violations.extend(rules::check_contractions(&prose_lines, sev));
    }

    // Semicolon
    if let Some(sev) = config.resolve_rule("semicolon") {
        violations.extend(rules::check_semicolons(&prose_lines, sev));
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
/// Prose and commit-message kinds blank out fenced code blocks so the
/// prose rules skip code. Blanked lines keep the line count, so a
/// `Violation.line` still names the source line. Exposed so `diff` can
/// diff in the same prose coordinate system that the engine uses for
/// `Violation.line`.
pub(crate) fn extract_prose_lines(kind: LintKind, text: &str) -> Vec<String> {
    match kind {
        LintKind::ProseFile | LintKind::CommitMessage => {
            let lines: Vec<String> = text.lines().map(String::from).collect();
            blank_out_fenced_code(&lines)
        }
        LintKind::SlashSource => extract_slash_comments(text),
        LintKind::HashSource => extract_hash_comments(text),
    }
}

/// Blank out the lines of fenced code blocks.
///
/// A fence line has at most three leading spaces, then three or more
/// backticks or tildes. A backtick opening fence does not open when
/// its info string holds a backtick. A closing fence uses the same
/// character, has at least the opening run length, and holds no text
/// after the run. Each blanked line keeps its byte length, so rule
/// line and column offsets stay aligned with the source text.
fn blank_out_fenced_code(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut open: Option<(char, usize)> = None; // (fence char, run length)
    for line in lines {
        let blank = " ".repeat(line.len());
        let Some((ch, run)) = fence_run(line) else {
            out.push(if open.is_none() { line.clone() } else { blank });
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

/// Return the leading backtick or tilde run when the line can open or
/// close a fence.
///
/// A fence candidate has at most three leading spaces, then three or
/// more backticks or tildes.
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

/// Extract `//` and `/* */` comment text from source code.
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
        if let Some(pos) = trimmed.find("//") {
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

/// Extract `#` comment lines from source code.
fn extract_hash_comments(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
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

/// Check paragraph-length violations (> 6 sentences).
fn check_paragraph_length(
    paragraphs: &[crate::types::Paragraph],
    severity: Severity,
) -> Vec<Violation> {
    const MAX_SENTENCES: usize = 6;
    paragraphs
        .iter()
        .filter(|p| p.sentences.len() > MAX_SENTENCES)
        .map(|p| Violation {
            rule_id: "paragraph-length",
            severity,
            message: format!(
                "Paragraph has {} sentences; the maximum is {}.",
                p.sentences.len(),
                MAX_SENTENCES
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
}
