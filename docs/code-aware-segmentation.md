# Code-aware segmentation

## Context

The sentence and paragraph counters treated code text as prose.
Dotted identifiers and shebangs inflated the sentence counts.
Shell scripts and commit messages tripped `paragraph-length` and
`sentence-length` on lines that read fine.

## Decision

`src/sentences.rs` changed in three ways.

1. `sentence_ends` guards dotted identifiers. A dot with word
   characters on both sides does not split.
2. `sentence_ends` guards numeric literals. A dot whose left side
   ends in a digit does not split, so IPs and versions stay whole.
3. `segment_sentences` takes a `merge_lines` flag. The source kinds
   pass false, so each extracted comment line closes its own
   sentence. No run of period-free lines builds one giant
   pseudo-sentence.

`src/engine.rs` changed in two ways.

1. `extract_hash_comments` skips shebang lines.
2. The paragraph cap is configurable. The `max_paragraph_sentences`
   key in `.simple-english.json` sets it. A value of 0 turns the
   rule off.

The `#!` shebang guard sits in `sentence_ends`. It covers the prose
kinds, where the line keeps its `#`. The hash extraction skip
covers the source kinds, where the `#` is stripped first.

## Defaults

Prose kinds keep the cap of 6. Source kinds default off, because
their comments are sparse.

## Consequences

- `run.idle`, `e2e.sh`, `127.0.0.1`, and `v1.2` stay whole in
  comments and commit bodies.
- A shell script with many comment lines passes `paragraph-length`
  without config.
- A long header comment block no longer reads as one sentence.
  Each comment line is its own short sentence.
- Prose files keep their behavior, except that dotted identifiers
  no longer split sentences.

## Tests

- `sentences.rs` guards: dotted identifiers, numeric literals,
  shebang lines, and source-mode line separation.
- `engine.rs` acceptance tests: the `run.idle` commit body, a 20-line
  comment file, and the configurable cap.

## Commits

- See the pull request for issue #1 on the remote.
