//! Core lint orchestration.
//!
//! `lint()` is the main entry point: it extracts prose from the source
//! text based on the content kind, segments it into sentences and
//! paragraphs, runs all enabled rules, and collects violations.

use crate::rules;
use crate::sentences::{segment_paragraphs, segment_sentences};
use crate::types::{
    default_rule_settings, LintConfig, LintKind, LintReport, RuleSetting, Severity, Violation,
};

/// Lint `text` of the given `kind`, returning a report of all violations.
pub fn lint(kind: LintKind, text: &str, config: &LintConfig) -> LintReport {
    let options = LintOptions {
        kind,
        rules: resolve_rule_settings(config),
        max_sentence_words: config
            .max_sentence_words
            .unwrap_or(DEFAULT_MAX_SENTENCE_WORDS),
    };

    // 1. Extract prose lines from the source.
    let prose_lines = extract_prose_lines(kind, text);

    if prose_lines.is_empty() {
        return LintReport {
            violations: vec![],
            summary: crate::types::LintSummary {
                total: 0,
                hard: 0,
            },
        };
    }

    // 2. Segment sentences and paragraphs.
    let prose_text = prose_lines.join("\n");
    let sentences = segment_sentences(&prose_text);
    let paragraphs = segment_paragraphs(&prose_text, &sentences);

    // 3. Run rules.
    let mut violations: Vec<Violation> = Vec::new();

    // Sentence-length.
    if let Some(sev) = options.rules.get("sentence-length") {
        violations.extend(check_sentence_length(&sentences, options.max_sentence_words, *sev));
    }

    // Paragraph-length.
    if let Some(sev) = options.rules.get("paragraph-length") {
        violations.extend(check_paragraph_length(&paragraphs, *sev));
    }

    // Contraction.
    if let Some(sev) = options.rules.get("contraction") {
        violations.extend(rules::check_contractions(&prose_lines, *sev));
    }

    // Semicolon.
    if let Some(sev) = options.rules.get("semicolon") {
        violations.extend(rules::check_semicolons(&prose_lines, *sev));
    }

    // Phrasal verbs.
    if let Some(sev) = options.rules.get("phrasal-verb") {
        violations.extend(rules::check_phrasal_verbs(&prose_lines, *sev));
    }

    // Hedging.
    if let Some(sev) = options.rules.get("hedging") {
        violations.extend(rules::check_hedging(&prose_lines, *sev));
    }

    // Marketing.
    if let Some(sev) = options.rules.get("marketing") {
        violations.extend(rules::check_marketing(&prose_lines, *sev));
    }

    // STE dictionary.
    if let Some(sev) = options.rules.get("dictionary-not-approved-word") {
        violations.extend(rules::check_ste_dictionary(&prose_lines, *sev));
    }

    // Suppression directive validation.
    if let Some(sev) = options.rules.get("invalid-suppression") {
        violations.extend(rules::check_suppression(
            &prose_lines,
            rules::REGISTERED_RULE_IDS,
            *sev,
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

    // 5. Resolve per-rule severity from config (overrides).
    for v in &mut violations {
        if let Some(setting) = options.rules.get(v.rule_id) {
            v.severity = *setting;
        }
    }

    // 6. Sort by line, then column.
    violations.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.column.cmp(&b.column)));

    let hard = violations
        .iter()
        .filter(|v| v.severity == Severity::Hard)
        .count();
    let total = violations.len();

    LintReport {
        violations,
        summary: crate::types::LintSummary { total, hard },
    }
}

/// The default max words per sentence.
pub const DEFAULT_MAX_SENTENCE_WORDS: usize = 25;

/// Resolved lint options (post-config).
struct LintOptions {
    kind: LintKind,
    rules: std::collections::BTreeMap<&'static str, RuleSetting>,
    max_sentence_words: usize,
}

fn resolve_rule_settings(config: &LintConfig) -> std::collections::BTreeMap<&'static str, RuleSetting> {
    let mut settings = default_rule_settings();
    for (rule_id, severity_str) in &config.rules {
        let sev = match severity_str.as_str() {
            "hard" => Some(Severity::Hard),
            "soft" => Some(Severity::Soft),
            "off" => None,
            _ => None,
        };
        if let Some(static_id) = match rule_id.as_str() {
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
        } {
            settings.insert(static_id, sev);
        }
    }
    settings
}

/// Extract prose lines from the source text based on the content kind.
fn extract_prose_lines(kind: LintKind, text: &str) -> Vec<String> {
    match kind {
        LintKind::ProseFile | LintKind::CommitMessage => {
            text.lines().map(String::from).collect()
        }
        LintKind::SlashSource => {
            extract_slash_comments(text)
        }
        LintKind::HashSource => {
            extract_hash_comments(text)
        }
    }
}

/// Extract `//` and `/* */` comment text from source code.
fn extract_slash_comments(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if in_block {
            if let Some(end) = trimmed.find("*/") {
                out.push(trimmed[..end + 2].to_string());
                in_block = false;
            } else {
                out.push(trimmed.to_string());
            }
            continue;
        }
        if let Some(pos) = trimmed.find("//") {
            // Only treat as a comment if `//` is at the start or after
            // whitespace / opening brace (not inside a string, but we
            // keep it simple for the hook use case).
            let comment = &trimmed[pos..];
            out.push(comment.to_string());
        }
        if trimmed.starts_with("/*") {
            in_block = !trimmed.contains("*/");
            out.push(trimmed.to_string());
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
                Some(trimmed.to_string())
            } else {
                None
            }
        })
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
        .map(|s| Violation {
            rule_id: "sentence-length",
            severity,
            message: format!(
                "Sentence has {} words; the maximum is {}.",
                s.word_count, max_words
            ),
            suggestions: None,
            line: s.line,
            column: s.column,
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
/// line-number → set of suppressed rule IDs.
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

        let target_line = i + 2; // 1-based, next line
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

    #[test]
    fn lint_clean_prose() {
        let config = LintConfig::default();
        let report = lint(LintKind::ProseFile, "This is a clean sentence. It is short.", &config);
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
        let long_sentence = "This is a sentence that has many words and it definitely exceeds the twenty five word limit that is set by default in the configuration file for this tool";
        let report = lint(LintKind::ProseFile, &format!("{}. ", long_sentence), &config);
        assert!(report
            .violations
            .iter()
            .any(|v| v.rule_id == "sentence-length"));
    }

    #[test]
    fn lint_detects_phrasal_verb() {
        let config = LintConfig::default();
        let report = lint(LintKind::ProseFile, "We will kick off the project.", &config);
        assert!(report
            .violations
            .iter()
            .any(|v| v.rule_id == "phrasal-verb"));
    }

    #[test]
    fn lint_off_rule_suppresses() {
        let mut config = LintConfig::default();
        config.rules.insert("semicolon".to_string(), "off".to_string());
        let report = lint(LintKind::ProseFile, "One; two.", &config);
        assert!(!report
            .violations
            .iter()
            .any(|v| v.rule_id == "semicolon"));
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
    fn lint_suppression_directive() {
        let config = LintConfig::default();
        let text = "// ste-disable-next-line semicolon\nOne; two.\n// ste-disable-next-line bogus-rule\nThree; four.";
        let report = lint(LintKind::ProseFile, text, &config);
        // The semicolon on line 2 is suppressed; the one on line 4 is not.
        let semi_violations: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.rule_id == "semicolon")
            .collect();
        assert_eq!(semi_violations.len(), 1);
        assert_eq!(semi_violations[0].line, 4);
    }
}
