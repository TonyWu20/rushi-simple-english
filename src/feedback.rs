//! Violation formatting and rule-summary text for prompt injection.

use crate::types::LintConfig;

/// Format a list of violations into a human-readable feedback string.
pub fn format_violations(
    path: &str,
    heading: &str,
    violations: &[crate::types::Violation],
) -> String {
    if violations.is_empty() {
        return String::new();
    }

    let details: Vec<String> = violations
        .iter()
        .map(|v| {
            let sugg = v
                .suggestions
                .as_ref()
                .and_then(|s| s.first())
                .map(|s| format!(". Suggested fix: use \"{}\".", s))
                .unwrap_or_default();
            format!(
                "- line {}, column {} [{}]: {}{}",
                v.line, v.column, v.rule_id, v.message, sugg
            )
        })
        .collect();

    format!(
        "{} {}:\n{}",
        heading, path, details.join("\n")
    )
}

/// Build the static rule-summary text that is injected into the model
/// prompt via the `model.before` window.
pub fn rule_summary(config: &LintConfig) -> String {
    let max_words = config.max_sentence_words.unwrap_or(25);
    let resolved = |id: &str| -> Option<String> {
        match config.rules.get(id) {
            Some(s) if s == "off" => None,
            _ => Some(default_severity_label(id).to_string()),
        }
    };

    let mut sections: Vec<String> = Vec::new();

    // ASD-STE100 group
    let ste_rules: Vec<(&str, Option<String>)> = vec![
        ("contraction", resolved("contraction")),
        ("dictionary-not-approved-word", resolved("dictionary-not-approved-word")),
        ("paragraph-length", resolved("paragraph-length")),
        ("phrasal-verb", resolved("phrasal-verb")),
        ("semicolon", resolved("semicolon")),
        ("sentence-length", Some("hard".to_string())),
        ("verb-progressive", resolved("verb-progressive")),
        ("verb-passive", resolved("verb-passive")),
        ("verb-perfect", resolved("verb-perfect")),
    ];
    let ste_lines: Vec<String> = ste_rules
        .iter()
        .filter_map(|(id, sev)| {
            let sev = sev.as_deref()?;
            let summary = summary_for_rule(id, max_words);
            Some(format!("- [{}] {}", sev, summary))
        })
        .collect();
    if !ste_lines.is_empty() {
        sections.push(format!(
            "### ASD-STE100 Simplified Technical English\n\n{}",
            ste_lines.join("\n")
        ));
    }

    // Directive validation
    let directive_lines: Vec<String> = vec![
        ("invalid-suppression", "Name registered rule IDs in suppression directives."),
    ]
    .into_iter()
    .filter_map(|(id, summary)| {
        resolved(id).map(|sev| format!("- [{}] {}", sev, summary))
    })
    .collect();
    if !directive_lines.is_empty() {
        sections.push(format!(
            "### Directive validation\n\n{}",
            directive_lines.join("\n")
        ));
    }

    // House-style rules
    let house_rules: Vec<(&str, Option<String>)> = vec![
        ("hedging", resolved("hedging")),
        ("marketing", resolved("marketing")),
    ];
    let house_lines: Vec<String> = house_rules
        .iter()
        .filter_map(|(id, sev)| {
            let sev = sev.as_deref()?;
            let summary = match *id {
                "hedging" => "Remove hedging phrases.",
                "marketing" => "Use factual language instead of marketing language.",
                _ => "",
            };
            Some(format!("- [{}] {}", sev, summary))
        })
        .collect();
    if !house_lines.is_empty() {
        sections.push(format!(
            "### House-style rules\n\n{}",
            house_lines.join("\n")
        ));
    }

    let rules_section = if sections.is_empty() {
        "No writing rules are enabled.".to_string()
    } else {
        format!(
            "Apply these enabled rules to prose that you write or edit:\n\n{}",
            sections.join("\n\n")
        )
    };

    format!(
        "## Writing rules\n\n{}\n\nWrites, edits, and git commit messages reject hard violations. Correct the reported text and retry. Soft violations produce warnings.",
        rules_section
    )
}

fn default_severity_label(id: &str) -> &'static str {
    match id {
        "contraction" => "hard",
        "dictionary-not-approved-word" => "hard",
        "hedging" => "soft",
        "invalid-suppression" => "hard",
        "marketing" => "soft",
        "paragraph-length" => "hard",
        "phrasal-verb" => "hard",
        "semicolon" => "hard",
        "sentence-length" => "hard",
        "verb-progressive" => "hard",
        "verb-passive" => "soft",
        "verb-perfect" => "hard",
        _ => "hard",
    }
}

fn summary_for_rule(id: &str, max_words: usize) -> String {
    match id {
        "contraction" => "Do not use contractions. Write the words in full.".to_string(),
        "dictionary-not-approved-word" => "Use approved words from the STE dictionary.".to_string(),
        "paragraph-length" => "Use no more than six sentences in one paragraph.".to_string(),
        "phrasal-verb" => "Use an approved single-word verb instead of a phrasal verb.".to_string(),
        "semicolon" => "Do not use semicolons in prose. Write two sentences. Code fences are exempt.".to_string(),
        "sentence-length" => format!("Keep each sentence to {} words or fewer.", max_words),
        "verb-progressive" => "Do not use progressive verb forms.".to_string(),
        "verb-passive" => "Prefer active voice.".to_string(),
        "verb-perfect" => "Do not use perfect verb forms.".to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Severity;

    #[test]
    fn format_violations_includes_rule_id() {
        let v = crate::types::Violation {
            rule_id: "semicolon",
            severity: Severity::Hard,
            message: "Do not use a semicolon. Write two sentences.".into(),
            suggestions: None,
            line: 3,
            column: 5,
        };
        let out = format_violations("test.md", "Writing rules blocked write for", &[v]);
        assert!(out.contains("[semicolon]"));
        assert!(out.contains("line 3"));
    }

    #[test]
    fn rule_summary_mentions_rules() {
        let config = LintConfig::default();
        let summary = rule_summary(&config);
        assert!(summary.contains("ASD-STE100"));
        assert!(summary.contains("contraction"));
        assert!(summary.contains("semicolon"));
    }

    #[test]
    fn rule_summary_respects_off() {
        let mut config = LintConfig::default();
        config.rules.insert("semicolon".to_string(), "off".to_string());
        let summary = rule_summary(&config);
        assert!(!summary.contains("[hard] Do not use semicolons"));
    }
}
