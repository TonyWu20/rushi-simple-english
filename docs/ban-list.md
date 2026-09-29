# Ban-list decisions

## 2026-09-29: ban `ledger`, bump to 0.4.1

### Context

`src/dictionaries.rs` holds the banned word lists. The single-word
list is `ste_dictionary()`. The hard rule
`dictionary-not-approved-word` checks it.

### Decision

1. Ban the single word `ledger`. The suggested replacement is `log`.
   The entry sits after the one-word precedent in
   `ste_dictionary()`.

2. Add the unit test `rules::tests::detects_ledger` in
   `src/rules.rs`.

3. Bump the version 0.4.0 to 0.4.1 as a patch. This is a user
   decision, 2026-09-29. A new banned word is a content change.
   It is not a new capability.

### Rationale

The plural form `ledgers` stays unbanned. This matches the one-form
precedent set by the earlier one-word entry.

A patch bump keeps the version level. The flake reads the version
from `Cargo.toml`. No other file holds the version.

### Consequences

- Prose that uses `ledger` now blocks at the `tool.before` and
  `run.idle` gates.
- `Cargo.lock` tracks 0.4.1.
