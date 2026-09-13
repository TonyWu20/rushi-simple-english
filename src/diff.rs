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
///
/// The "changed" set is computed in *prose* coordinates (the extracted
/// comment lines for source kinds), the same coordinate system the
/// engine uses for `Violation.line`.
pub fn lint_write(
    kind: LintKind,
    new_content: &str,
    old_content: Option<&str>,
    config: &LintConfig,
) -> LintReport {
    let changed = changed_prose_lines(kind, old_content.unwrap_or(""), new_content);
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
    let changed = changed_prose_lines(kind, file_content, &new_content);
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

/// Return the 1-based *prose line* numbers in `new` that differ from
/// `old`, for a given content `kind`.
///
/// The engine reports `Violation.line` in *prose* coordinates: for the
/// source kinds the prose is the extracted comment text, a compacted
/// subset of the file, so the prose index is not the file line number.
/// To filter violations against the lines an edit actually touched, the
/// diff must therefore run over the extracted prose lines, not the raw
/// file. Diffing raw file lines mixed the two coordinate systems and
/// made pre-existing violations false-positive on unrelated edits.
///
/// Uses the `similar` crate's line-level diff over the joined prose text.
/// An `Insert` or `Replace` op marks its new-side line range as changed;
/// an `Equal` op leaves its lines untouched. `Delete` ops touch only the
/// old side.
fn changed_prose_lines(
    kind: LintKind,
    old: &str,
    new: &str,
) -> std::collections::HashSet<usize> {
    let old_prose = engine::extract_prose_lines(kind, old).join("\n");
    let new_prose = engine::extract_prose_lines(kind, new).join("\n");
    let diff = TextDiff::from_lines(&old_prose, &new_prose);
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
        let changed = changed_prose_lines(LintKind::ProseFile, old, new);
        assert!(changed.contains(&2), "line 2 changed");
        assert!(!changed.contains(&1), "line 1 unchanged");
        assert!(!changed.contains(&3), "line 3 unchanged");
    }

    #[test]
    fn changed_lines_new_file_is_all() {
        let changed = changed_prose_lines(LintKind::ProseFile, "", "a\nb\nc\n");
        assert_eq!(changed, std::collections::HashSet::from([1usize, 2, 3]));
    }

    #[test]
    fn changed_lines_identical_is_empty() {
        let text = "same\nsame\n";
        assert!(changed_prose_lines(LintKind::ProseFile, text, text).is_empty());
    }

    #[test]
    fn slash_source_preexisting_violation_does_not_resurface() {
        // A `.rs` file (SlashSource): the file-line numbers of the
        // comments differ from the prose-line indices the engine reports.
        // The semicolon is on a comment that the edit does not touch, so
        // it must not resurface even though the edited file line number
        // happens to match that prose index.
        let config = LintConfig::default();
        let old = "// intro\nfn main() {\n    let a = 1; // bad; semicolon\n}\n";
        let report = lint_edit(
            LintKind::SlashSource,
            old,
            "fn main() {",
            "fn main2() {",
            false,
            &config,
        )
        .unwrap();
        assert!(
            report.violations.is_empty(),
            "pre-existing semicolon on an unchanged prose line was kept: {report:?}"
        );
    }

    #[test]
    fn slash_source_new_violation_is_reported() {
        // A new comment introduces a semicolon on a changed prose line.
        let config = LintConfig::default();
        let old = "fn main() {\n    let a = 1;\n}\n";
        let report = lint_edit(
            LintKind::SlashSource,
            old,
            "let a = 1;",
            "let a = 1; // bad; note",
            false,
            &config,
        )
        .unwrap();
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.rule_id == "semicolon"),
            "new semicolon on a changed prose line was dropped: {report:?}"
        );
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

    #[test]
    fn lint_write_new_file_fenced_code_is_exempt() {
        let config = LintConfig::default();
        let new = "Title.\n\n```sh\nrun a; run b\n```\n";
        let report = lint_write(LintKind::ProseFile, new, None, &config);
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn lint_edit_inside_fence_is_exempt() {
        let config = LintConfig::default();
        let file = "Title.\n\n```sh\nrun a\n```\n";
        let report =
            lint_edit(LintKind::ProseFile, file, "run a", "run a; run b", false, &config)
                .unwrap();
        assert_eq!(report.summary.total, 0, "{report:?}");
    }

    #[test]
    fn lint_edit_inside_inline_span_is_exempt() {
        let config = LintConfig::default();
        let file = "Title.\nUse `run a` now.\n";
        let report = lint_edit(
            LintKind::ProseFile,
            file,
            "run a",
            "run a; run b",
            false,
            &config,
        )
        .unwrap();
        assert_eq!(report.summary.total, 0, "{report:?}");
    }
}
