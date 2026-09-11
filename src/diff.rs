//! Diff-aware linting.
//!
//! The naive approach lints each `write`/`edit` payload in isolation and
//! reports every violation, even ones that already existed in the file
//! before the agent touched it. That floods the user with noise on
//! pre-existing code.
//!
//! This module fixes that: it computes which *lines of the new content*
//! actually changed relative to the previous content (via the `similar`
//! crate), lints the full new content, then discards any violation that
//! lands on an unchanged line. New files (no previous content) are
//! linted in full.

use crate::engine;
use crate::types::{LintConfig, LintKind, LintReport, LintSummary, Severity, Violation};
use similar::{DiffTag, TextDiff};

/// Lint `new_content`, keeping only violations on lines that changed
/// relative to `old_content`. When `old_content` is `None` (a brand-new
/// file), every line is treated as changed and all violations are kept.
pub fn lint_write(
    kind: LintKind,
    new_content: &str,
    old_content: Option<&str>,
    config: &LintConfig,
) -> LintReport {
    let changed = changed_lines(old_content.unwrap_or(""), new_content);
    let report = engine::lint(kind, new_content, config);
    filter_to_changed(report, &changed)
}

/// Apply a single `edit` tool call (`old_string` -> `new_string`, with an
/// optional `replace_all`) to `file_content`, then lint the result and
/// keep only the violations on lines the edit actually touched.
///
/// Returns `Err` when the edit cannot be applied (empty `old_string`,
/// `old_string` not found, or not unique when `replace_all` is false).
/// Callers treat `Err` as "cannot lint, let the tool fail naturally."
pub fn lint_edit(
    kind: LintKind,
    file_content: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    config: &LintConfig,
) -> Result<LintReport, String> {
    let new_content = apply_edit(file_content, old_string, new_string, replace_all)?;
    let changed = changed_lines(file_content, &new_content);
    let report = engine::lint(kind, &new_content, config);
    Ok(filter_to_changed(report, &changed))
}

/// Apply one edit, mirroring the harness `edit` tool's semantics.
///
/// - empty `old_string` is an error
/// - `old_string` must occur at least once
/// - when `replace_all` is false, `old_string` must be unique
pub fn apply_edit(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<String, String> {
    if old.is_empty() {
        return Err("old_string is empty".to_string());
    }
    let count = content.matches(old).count();
    if count == 0 {
        return Err("old_string not found in file".to_string());
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "old_string appears {count} times; set replace_all or use more context"
        ));
    }
    let result = if replace_all {
        content.replace(old, new)
    } else {
        content.replacen(old, new, 1)
    };
    Ok(result)
}

/// Return the 1-based line numbers in `new` that differ from `old`.
///
/// Uses the `similar` crate's line-level diff. An `Insert` or `Replace`
/// op marks its new-side line range as changed; an `Equal` op leaves
/// its lines untouched. `Delete` ops touch only the old side.
fn changed_lines(old: &str, new: &str) -> std::collections::HashSet<usize> {
    let diff = TextDiff::from_lines(old, new);
    let mut changed = std::collections::HashSet::new();
    for op in diff.ops() {
        if matches!(op.tag(), DiffTag::Insert | DiffTag::Replace) {
            for line in op.new_range() {
                changed.insert(line + 1); // 1-based
            }
        }
    }
    changed
}

/// Keep only violations whose 1-based line is in `changed`.
fn filter_to_changed(report: LintReport, changed: &std::collections::HashSet<usize>) -> LintReport {
    let violations: Vec<Violation> = report
        .violations
        .into_iter()
        .filter(|v| changed.contains(&v.line))
        .collect();
    let hard = violations
        .iter()
        .filter(|v| v.severity == Severity::Hard)
        .count();
    LintReport {
        summary: LintSummary {
            total: violations.len(),
            hard,
        },
        violations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_lines_detects_middle_edit() {
        let old = "line one\nline two\nline three\n";
        let new = "line one\nline two CHANGED\nline three\n";
        let changed = changed_lines(old, new);
        assert!(changed.contains(&2), "line 2 changed");
        assert!(!changed.contains(&1), "line 1 unchanged");
        assert!(!changed.contains(&3), "line 3 unchanged");
    }

    #[test]
    fn changed_lines_new_file_is_all() {
        let changed = changed_lines("", "a\nb\nc\n");
        assert_eq!(changed, std::collections::HashSet::from([1usize, 2, 3]));
    }

    #[test]
    fn changed_lines_identical_is_empty() {
        let text = "same\nsame\n";
        assert!(changed_lines(text, text).is_empty());
    }

    #[test]
    fn apply_edit_unique() {
        let out = apply_edit("foo\nbar\n", "bar", "BAR", false).unwrap();
        assert_eq!(out, "foo\nBAR\n");
    }

    #[test]
    fn apply_edit_ambiguous_without_replace_all() {
        let res = apply_edit("x\nx\n", "x", "y", false);
        assert!(res.is_err());
    }

    #[test]
    fn apply_edit_replace_all() {
        let out = apply_edit("x\nx\n", "x", "y", true).unwrap();
        assert_eq!(out, "y\ny\n");
    }

    #[test]
    fn apply_edit_empty_old_is_error() {
        assert!(apply_edit("abc", "", "z", false).is_err());
    }

    #[test]
    fn lint_edit_filters_preexisting_violation() {
        // File already has a semicolon on line 1 (pre-existing). The edit
        // only touches line 2, so the old violation must not resurface.
        let config = LintConfig::default();
        let file = "bad; old line\nplain line\n";
        let report =
            lint_edit(LintKind::ProseFile, file, "plain line", "plain; new line", false, &config)
                .unwrap();
        let lines: Vec<usize> = report.violations.iter().map(|v| v.line).collect();
        assert!(
            lines.iter().all(|l| *l == 2),
            "only the new line's violation is reported: {report:?}"
        );
    }

    #[test]
    fn lint_edit_reports_new_violation() {
        let config = LintConfig::default();
        let file = "good line\n";
        let report =
            lint_edit(LintKind::ProseFile, file, "good line", "bad; line", false, &config)
                .unwrap();
        assert!(report.summary.hard >= 1);
    }

    #[test]
    fn lint_edit_missing_old_is_error() {
        let config = LintConfig::default();
        let res =
            lint_edit(LintKind::ProseFile, "a\n", "not-there", "x; y", false, &config);
        assert!(res.is_err());
    }

    #[test]
    fn lint_write_existing_file_filters_unchanged() {
        let config = LintConfig::default();
        let old = "fine line\n";
        let new = "fine line\nbad; new line\n";
        let report = lint_write(LintKind::ProseFile, new, Some(old), &config);
        let lines: Vec<usize> = report.violations.iter().map(|v| v.line).collect();
        assert!(lines.iter().all(|l| *l == 2), "{report:?}");
    }

    #[test]
    fn lint_write_new_file_is_full() {
        let config = LintConfig::default();
        let report = lint_write(LintKind::ProseFile, "bad; text", None, &config);
        assert!(report.summary.hard >= 1);
    }
}
