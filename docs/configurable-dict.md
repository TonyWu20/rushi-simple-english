# Configurable dictionary decisions

## 2026-10-21: project `.simple-english.toml` with blacklist and whitelist

### Context

The banned word lists live in `src/dictionaries.rs`. They compile
into the binary. A project could not extend the lists or approve
exceptions without a rebuild. The project config was
`.simple-english.json`. It set rule severities and length caps only.

### Decision

1. Move the project config to TOML. A `.simple-english.toml` in
   the project root replaces `.simple-english.json`. The
   `SIMPLE_ENGLISH_CONFIG` environment variable names a TOML path.
   The hook and the `rushi-ste` CLI share the lookup.

2. Add `[[blacklist]]` entries. Each entry holds an `unapproved`
   list of word or phrase forms. A `suggestions` list is optional.
   Without it, the suggested fix is `delete`. Entries check under
   the existing `dictionary-not-approved-word` rule. Their
   severity follows the rule override in `[rules]`.

3. Add a `[whitelist]` table. Its `words` key names exact word and
   phrase forms that never get flagged. Matching is
   case-insensitive. It applies to every phrase-list rule: the STE
   dictionary, phrasal verbs, hedging, marketing. It covers
   project blacklist entries too. A whitelisted form wins over a
   blacklist entry.

4. Keep the existing keys in the same file: `rules`,
   `max_sentence_words`, `max_paragraph_sentences`,
   `exempt_block_quotes`.

### Rationale

TOML is the one config format now. One file holds the whole
config. The user chose it over keeping two formats.

A `[blacklist]` entry reuses the `PhraseEntry` shape from
`src/dictionaries.rs`. The `toml` crate gives `PhraseEntry` a
`Deserialize` derive. The whitelist builds one `HashSet` of
lowercased forms per lint run. Each phrase check is a set lookup.

JSON support drops. A `.simple-english.json` file is silently
ignored. The README records the migration.

### Consequences

- Projects add banned words and approved exceptions without a
  rebuild. The built-in dictionary is unchanged.
- The crate gains the `toml` dependency (v1.1).
- Top-level keys must sit before the first table header. TOML
  ends the top-level scope at the header. The README and the
  config test note the rule.
- `cargo add toml` refreshed `Cargo.lock` for the whole tree.
- The version stays 0.5.0. That is the in-development version
  behind the CLI feature. This feature joins it.

(exit code 0)
