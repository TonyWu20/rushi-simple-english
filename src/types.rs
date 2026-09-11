//! Core types for the simple-english hook.

use std::collections::BTreeMap;

/// Severity of a rule violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Hard,
    Soft,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Hard => "hard",
            Severity::Soft => "soft",
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single rule violation.
#[derive(Debug, Clone)]
pub struct Violation {
    pub rule_id: &'static str,
    pub severity: Severity,
    pub message: String,
    pub suggestions: Option<Vec<String>>,
    pub line: usize,
    pub column: usize,
}


/// Summary of a lint pass.
#[derive(Debug, Clone, Default)]
pub struct LintSummary {
    pub total: usize,
    pub hard: usize,
}

/// The full lint report.
#[derive(Debug, Clone)]
pub struct LintReport {
    pub violations: Vec<Violation>,
    pub summary: LintSummary,
}

/// Content kind determines how prose is extracted from the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintKind {
    ProseFile,
    SlashSource,
    HashSource,
    CommitMessage,
}

/// Per-rule setting: severity or off.
pub type RuleSetting = Option<Severity>;

/// Configuration for the lint engine.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct LintConfig {
    /// Per-rule overrides. Keys are rule IDs, values are "hard", "soft", or "off".
    #[serde(default)]
    pub rules: BTreeMap<String, String>,
    /// Maximum words per sentence.
    pub max_sentence_words: Option<usize>,
    /// Whether to exempt block-quoted content from linting.
    #[serde(default)]
    pub exempt_block_quotes: bool,
}

impl LintConfig {
    /// Resolve the effective setting for a rule, falling back to defaults.
    pub fn resolve_rule(&self, rule_id: &str) -> RuleSetting {
        if let Some(setting) = self.rules.get(rule_id) {
            return match setting.as_str() {
                "hard" => Some(Severity::Hard),
                "soft" => Some(Severity::Soft),
                "off" => None,
                _ => None,
            };
        }
        // Default severities
        match rule_id {
            "contraction" => Some(Severity::Hard),
            "dictionary-not-approved-word" => Some(Severity::Hard),
            "hedging" => Some(Severity::Soft),
            "invalid-suppression" => Some(Severity::Hard),
            "marketing" => Some(Severity::Soft),
            "paragraph-length" => Some(Severity::Hard),
            "phrasal-verb" => Some(Severity::Hard),
            "semicolon" => Some(Severity::Hard),
            "sentence-length" => Some(Severity::Hard),
            "verb-progressive" => Some(Severity::Hard),
            "verb-passive" => Some(Severity::Soft),
            "verb-perfect" => Some(Severity::Hard),
            _ => None,
        }
    }
}

/// A sentence with position information.
#[derive(Debug, Clone)]
pub struct Sentence {
    /// The text of the sentence (including terminal punctuation).
    pub text: String,
    pub line: usize,
    pub column: usize,
    pub word_count: usize,
}

/// A paragraph containing sentences.
#[derive(Debug, Clone)]
pub struct Paragraph {
    pub sentences: Vec<Sentence>,
    pub line: usize,
    pub column: usize,
}

