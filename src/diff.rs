//! Diff-aware linting.
//!
//! The naive approach lints each `write`/`edit` payload in isolation and
//! reports every violation, even ones that already existed in the file
//! before the agent touched it. That floods the user with noise on
//! pre-existing code.
//!
//! This module fixes that: it computes which *lines of the new content*
//! actually changed relative to the previous content, lints the full new
//! content, then discards any violation that lands on an unchanged line.
//! New files (no previous content) are linted in full.

use crate::engine;
use crate::types::{LintConfig, LintKind, LintReport, LintSummary, Severity, Violation};

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
fn changed_lines(old: &str, new: &str) -> std::collections::HashSet<usize> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    if old_lines.is_empty() && new_lines.is_empty() {
        return std::collections::HashSet::new();
    }
    if old_lines.is_empty() {
        return (1..=new_lines.len()).collect();
    }

    // LCS is exact but O(n*m); fall back to the cheap prefix/suffix
    // heuristic when either side is large.
    const CELL_CAP: u64 = 4_000_000;
    let n = old_lines.len() as u64;
    let m = new_lines.len() as u64;
    if n * m > CELL_CAP {
        return prefix_suffix_changed(&old_lines, &new_lines);
    }

    lcs_changed(&old_lines, &new_lines)
}

/// Exact changed-line set via LCS over the two line arrays.
fn lcs_changed(old: &[&str], new: &[&str]) -> std::collections::HashSet<usize> {
    let n = old.len();
    let m = new.len();
    let cols = m + 1;
    let mut dp = vec![0u32; (n + 1) * cols];

    for i in 1..=n {
        for j in 1..=m {
            let match_len = if old[i - 1] == new[j - 1] {
                dp[(i - 1) * cols + j - 1] + 1
            } else {
                dp[(i - 1) * cols + j].max(dp[i * cols + j - 1])
            };
            dp[i * cols + j] = match_len;
        }
    }

    // Walk back, marking new-lines that are part of the alignment.
    let mut i = n;
    let mut j = m;
    let mut matched = vec![false; m];
    while i > 0 && j > 0 {
        if old[i - 1] == new[j - 1] {
            matched[j - 1] = true;
            i -= 1;
            j -= 1;
        } else if dp[(i - 1) * cols + j] >= dp[i * cols + j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }

    (0..m)
        .filter(|&idx| !matched[idx])
        .map(|idx| idx + 1)
        .collect()
}

/// Cheap fallback: changed lines are the ones between the common prefix
/// and the common suffix.
fn prefix_suffix_changed(old: &[&str], new: &[&str]) -> std::collections::HashSet<usize> {
    let n = old.len();
    let m = new.len();

    let mut prefix = 0usize;
    while prefix < n && prefix < m && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < n - prefix && suffix < m - prefix && old[n - 1 - suffix] == new[m - 1 - suffix] {
        suffix += 1;
    }

    let start = prefix + 1; // 1-based
    let end = m.saturating_sub(suffix) + 1; // 1-based inclusive
    if start > end {
        return std::collections::HashSet::new();
    }
    (start..=end).collect()
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
