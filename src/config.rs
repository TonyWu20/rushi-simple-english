//! Configuration loading for the simple-english hook.
//!
//! Looks for a `.simple-english.json` config file in the current working
//! directory. Falls back to defaults when not found.

use crate::types::LintConfig;
use std::path::PathBuf;

/// Load the lint config from the project root.
///
/// Lookup order:
/// 1. `$SIMPLE_ENGLISH_CONFIG` environment variable (explicit path)
/// 2. `.simple-english.json` in the current working directory
/// 3. Defaults
pub fn load_config() -> LintConfig {
    if let Ok(path) = std::env::var("SIMPLE_ENGLISH_CONFIG") {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<LintConfig>(&raw) {
                return cfg;
            }
        }
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let candidate = cwd.join(".simple-english.json");
    if candidate.is_file() {
        if let Ok(raw) = std::fs::read_to_string(&candidate) {
            if let Ok(cfg) = serde_json::from_str::<LintConfig>(&raw) {
                return cfg;
            }
        }
    }

    LintConfig::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_when_no_file() {
        let config = LintConfig::default();
        assert!(config.rules.is_empty());
        assert!(config.max_sentence_words.is_none());
    }
}
