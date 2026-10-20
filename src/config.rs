//! Configuration loading for the simple-english hook.
//!
//! Looks for a `.simple-english.toml` config file in the current working
//! directory. Falls back to defaults when not found.

use crate::types::LintConfig;
use std::path::{Path, PathBuf};

/// Load the lint config from the project root.
///
/// Lookup order:
/// 1. `$SIMPLE_ENGLISH_CONFIG` environment variable (explicit path)
/// 2. `.simple-english.toml` in the current working directory
/// 3. Defaults
pub fn load_config() -> LintConfig {
    if let Ok(path) = std::env::var("SIMPLE_ENGLISH_CONFIG") {
        if let Some(cfg) = load_from_path(PathBuf::from(path).as_path()) {
            return cfg;
        }
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let candidate = cwd.join(".simple-english.toml");
    if candidate.is_file() {
        if let Some(cfg) = load_from_path(&candidate) {
            return cfg;
        }
    }

    LintConfig::default()
}

/// Read and parse one TOML config file.
///
/// Returns `None` when the file is missing, unreadable, or does not
/// parse. The caller falls back to defaults in that case.
pub fn load_from_path(path: &Path) -> Option<LintConfig> {
    let raw = std::fs::read_to_string(path).ok()?;
    toml::from_str(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_when_no_file() {
        let config = LintConfig::default();
        assert!(config.rules.is_empty());
        assert!(config.max_sentence_words.is_none());
        assert!(config.max_paragraph_sentences.is_none());
        assert!(config.blacklist.is_empty());
        assert!(config.whitelist.words.is_empty());
    }

    #[test]
    fn paragraph_cap_parses_from_toml() {
        let cfg = toml::from_str::<LintConfig>("max_paragraph_sentences = 8").unwrap();
        assert_eq!(cfg.max_paragraph_sentences, Some(8));
    }

    #[test]
    fn full_config_parses_from_toml() {
        // Top-level keys must sit before the first table header.
        // A header ends the top-level scope for the rest of the file.
        let raw = r#"
            max_sentence_words = 20
            exempt_block_quotes = true

            [rules]
            semicolon = "soft"
            dictionary-not-approved-word = "off"

            [[blacklist]]
            unapproved = ["utilize", "utilizes", "utilized", "utilizing"]
            suggestions = ["use"]

            [[blacklist]]
            unapproved = ["deep dive"]

            [whitelist]
            words = ["utilize", "kick off"]
        "#;
        let cfg = toml::from_str::<LintConfig>(raw).unwrap();
        assert_eq!(cfg.rules.get("semicolon").map(String::as_str), Some("soft"));
        assert_eq!(
            cfg.rules
                .get("dictionary-not-approved-word")
                .map(String::as_str),
            Some("off")
        );
        assert_eq!(cfg.max_sentence_words, Some(20));
        assert!(cfg.exempt_block_quotes);
        assert_eq!(cfg.blacklist.len(), 2);
        assert_eq!(
            cfg.blacklist[0].unapproved,
            vec!["utilize", "utilizes", "utilized", "utilizing"]
        );
        assert_eq!(cfg.blacklist[0].suggestions, vec!["use"]);
        assert_eq!(cfg.blacklist[1].unapproved, vec!["deep dive"]);
        assert!(cfg.blacklist[1].suggestions.is_empty());
        assert_eq!(cfg.whitelist.words, vec!["utilize", "kick off"]);
    }

    #[test]
    fn missing_file_is_none() {
        assert!(load_from_path(std::path::Path::new("/nonexistent/se.toml")).is_none());
    }

    #[test]
    fn bad_toml_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("se.toml");
        std::fs::write(&path, "not [valid toml").unwrap();
        assert!(load_from_path(&path).is_none());
    }

    #[test]
    fn load_from_path_parses_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("se.toml");
        std::fs::write(&path, "max_sentence_words = 30").unwrap();
        let cfg = load_from_path(&path).unwrap();
        assert_eq!(cfg.max_sentence_words, Some(30));
    }
}
