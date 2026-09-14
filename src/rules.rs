//! Rule implementations for the simple-english lint engine.
//!
//! Each rule inspects already-extracted prose lines and returns a list of
//! violations.  Rules are independent and order-independent; the engine
//! (engine.rs) collects their results.

use crate::dictionaries;
use crate::types::{Severity, Violation};

use regex::Regex;
use std::sync::OnceLock;

// ── Shared helpers ─────────────────────────────────────────────────────────

/// Case-insensitive word-boundary check for a single phrase.  Returns the
/// list of lines and columns where `phrase` occurs.
fn find_phrase(lines: &[String], phrase: &str) -> Vec<(usize, usize)> {
    let mut hits = Vec::new();
    let lower_phrase = phrase.to_lowercase();
    for (i, line) in lines.iter().enumerate() {
        let lower_line = line.to_lowercase();
        let mut start = 0usize;
        while let Some(pos) = lower_line[start..].find(&lower_phrase) {
            let abs = start + pos;
            // Check word-boundary on both sides
            let before_ok = abs == 0
                || !line[abs - 1..].chars().next().unwrap().is_alphanumeric();
            let after_abs = abs + phrase.len();
            let after_ok = after_abs >= line.len()
                || !line[after_abs..]
                    .chars()
                    .next()
                    .map(|c| c.is_alphanumeric())
                    .unwrap_or(false);
            if before_ok && after_ok {
                hits.push((i + 1, abs + 1));
            }
            start = abs + 1;
        }
    }
    hits
}

// ── Contractions ───────────────────────────────────────────────────────────

const CONTRACTION_RE: &str = r"\b\w+['\u{2019}](?:t|re|ve|ll|d|m)\b";
const CONTRACTION_S_RE: &str = r"\b(?:it|he|she|that|what|who|there|here|let|one|where|how|everyone|everybody|something|nothing|somebody|nobody)['\u{2019}]s\b";

fn contraction_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&(r"(?i)".to_string() + CONTRACTION_RE)).unwrap())
}

fn contraction_s_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&(r"(?i)".to_string() + CONTRACTION_S_RE)).unwrap()
    })
}

pub fn check_contractions(lines: &[String], severity: Severity) -> Vec<Violation> {
    let mut out: Vec<Violation> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        for m in contraction_re().find_iter(line) {
            out.push(Violation {
                rule_id: "contraction",
                severity,
                message: format!(
                    "Do not use a contraction. Write the words in full. Found \"{}\".",
                    m.as_str()
                ),
                suggestions: None,
                line: i + 1,
                column: m.start() + 1,
            });
        }
        for m in contraction_s_re().find_iter(line) {
            out.push(Violation {
                rule_id: "contraction",
                severity,
                message: format!(
                    "Do not use a contraction. Write the words in full. Found \"{}\".",
                    m.as_str()
                ),
                suggestions: None,
                line: i + 1,
                column: m.start() + 1,
            });
        }
    }
    out
}

// ── Semicolon ──────────────────────────────────────────────────────────────

/// Check a prose line for semicolons. The engine runs this for the
/// prose kinds only; source files skip it because a semicolon there
/// is code punctuation.
pub fn check_semicolons(lines: &[String], severity: Severity) -> Vec<Violation> {
    let re = Regex::new(r";").unwrap();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        for m in re.find_iter(line) {
            out.push(Violation {
                rule_id: "semicolon",
                severity,
                message: "Do not use a semicolon in prose. Write two sentences.".into(),
                suggestions: None,
                line: i + 1,
                column: m.start() + 1,
            });
        }
    }
    out
}

// ── Phrase-list rules (phrasal verbs, hedging, marketing, STE dict) ───────

fn phrase_violations(
    lines: &[String],
    entries: &[dictionaries::PhraseEntry],
    rule_id: &'static str,
    severity: Severity,
) -> Vec<Violation> {
    let mut out = Vec::new();
    for entry in entries {
        for phrase in &entry.unapproved {
            for (line_no, col) in find_phrase(lines, phrase) {
                let sugg = entry
                    .suggestions
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "delete".to_string());
                out.push(Violation {
                    rule_id,
                    severity,
                    message: format!(
                        "Use an approved word instead of \"{}\".",
                        phrase
                    ),
                    suggestions: Some(vec![sugg]),
                    line: line_no,
                    column: col,
                });
            }
        }
    }
    out
}

pub fn check_phrasal_verbs(
    lines: &[String],
    severity: Severity,
) -> Vec<Violation> {
    phrase_violations(
        lines,
        &dictionaries::phrasal_verbs(),
        "phrasal-verb",
        severity,
    )
}

pub fn check_hedging(lines: &[String], severity: Severity) -> Vec<Violation> {
    phrase_violations(lines, &dictionaries::hedging(), "hedging", severity)
}

pub fn check_marketing(lines: &[String], severity: Severity) -> Vec<Violation> {
    phrase_violations(lines, &dictionaries::marketing(), "marketing", severity)
}

pub fn check_ste_dictionary(
    lines: &[String],
    severity: Severity,
) -> Vec<Violation> {
    phrase_violations(
        lines,
        &dictionaries::ste_dictionary(),
        "dictionary-not-approved-word",
        severity,
    )
}

// ── Suppression directives ─────────────────────────────────────────────────

/// Validate `ste-disable-next-line` directives: every rule ID named in the
/// directive must be a registered rule.
pub fn check_suppression(
    lines: &[String],
    registered: &[&str],
    severity: Severity,
) -> Vec<Violation> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("ste-disable-next-line") {
            continue;
        }
        // Extract rule IDs from the directive
        let directive = line
            .split("ste-disable-next-line")
            .nth(1)
            .unwrap_or("");
        let names: Vec<&str> = directive
            .split(|c: char| c.is_whitespace() || c == ',' || c == ':')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        let unknown: Vec<&str> = names
            .iter()
            .filter(|id| !registered.contains(*id))
            .copied()
            .collect();

        if unknown.is_empty() && names.is_empty() {
            out.push(Violation {
                rule_id: "invalid-suppression",
                severity,
                message: "Suppression directive must name at least one rule id."
                    .to_string(),
                suggestions: None,
                line: i + 1,
                column: line.find("ste-disable-next-line").unwrap_or(0) + 1,
            });
        } else if !unknown.is_empty() {
            let names_str: Vec<String> = unknown
                .iter()
                .map(|id| format!("\"{}\"", id))
                .collect();
            out.push(Violation {
                rule_id: "invalid-suppression",
                severity,
                message: format!(
                    "Suppression directive names unknown rule ids: {}.",
                    names_str.join(", ")
                ),
                suggestions: None,
                line: i + 1,
                column: line.find("ste-disable-next-line").unwrap_or(0) + 1,
            });
        }
    }
    out
}

// ── Registered rule IDs (used for suppression validation) ─────────────────

pub const REGISTERED_RULE_IDS: &[&str] = &[
    "contraction",
    "dictionary-not-approved-word",
    "hedging",
    "invalid-suppression",
    "marketing",
    "paragraph-length",
    "phrasal-verb",
    "semicolon",
    "sentence-length",
    "verb-progressive",
    "verb-passive",
    "verb-perfect",
];

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(String::from).collect()
    }

    #[test]
    fn detects_contractions() {
        let v = check_contractions(&lines("Don't do it; won't stop."), Severity::Hard);
        assert!(!v.is_empty());
        assert!(v.iter().all(|x| x.rule_id == "contraction"));
    }

    #[test]
    fn detects_semicolon() {
        let v = check_semicolons(&lines("one; two"), Severity::Hard);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rule_id, "semicolon");
    }

    #[test]
    fn no_false_positive_semicolon_in_code() {
        // Semicolons in code are not in prose lines
        let v = check_semicolons(&lines("let x = 1;"), Severity::Hard);
        assert!(!v.is_empty()); // "1;" has a semicolon
    }

    #[test]
    fn detects_phrasal_verb() {
        let v = check_phrasal_verbs(&lines("We will carry out the plan."), Severity::Hard);
        assert!(!v.is_empty());
        assert!(v[0].rule_id == "phrasal-verb");
    }

    #[test]
    fn detects_marketing() {
        let v = check_marketing(
            &lines("This is a seamless and powerful solution."),
            Severity::Soft,
        );
        assert!(v.len() >= 2);
    }

    #[test]
    fn detects_hedging() {
        let v = check_hedging(
            &lines("It is important to note that this is a test."),
            Severity::Soft,
        );
        assert!(!v.is_empty());
    }

    #[test]
    fn detects_ste_word() {
        let v = check_ste_dictionary(&lines("We need to utilize this tool."), Severity::Hard);
        assert!(!v.is_empty());
        assert!(v[0].rule_id == "dictionary-not-approved-word");
    }

    #[test]
    fn suppression_unknown_rule() {
        let v = check_suppression(
            &lines("// ste-disable-next-line bogus-rule"),
            REGISTERED_RULE_IDS,
            Severity::Hard,
        );
        assert!(!v.is_empty());
        assert_eq!(v[0].rule_id, "invalid-suppression");
    }

    #[test]
    fn suppression_valid_rule() {
        let v = check_suppression(
            &lines("// ste-disable-next-line semicolon"),
            REGISTERED_RULE_IDS,
            Severity::Hard,
        );
        assert!(v.is_empty());
    }
}
