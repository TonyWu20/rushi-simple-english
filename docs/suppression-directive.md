# Suppression directive matching

## Context

The `ste-disable-next-line` check matched any line that contained the
marker. Two false positives followed.

1. Doc comments that explain the feature name the marker mid-line.
   The check flagged them as invalid suppressions.
2. The same match armed the suppression map. A mention mid-line
   could suppress the next line for no reason.

## Decision

`src/rules.rs` adds `directive_rest(line)`. It returns the text
after the marker when the line is a directive.

1. A line is a directive only when the marker is first on the line.
2. Leading whitespace may precede the marker.
3. One optional comment marker (`//`, `///`, or `#`) may precede
   the marker. Source kinds already strip their markers. Prose
   kinds keep them, so both forms resolve.
4. One optional doc bang (`!`) may precede the marker. This keeps
   `/// ste-disable-next-line ...` working in source files.
5. `check_suppression` and `suppressed_rule_ids_by_line` both use
   the one matcher.

## Consequences

- A doc line that mentions the marker never fires the rule.
- A directive still works in source, prose, and commit-message
  kinds.
- Suppression and validation share one match. They can never
  disagree.
