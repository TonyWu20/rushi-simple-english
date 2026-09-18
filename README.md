# harness-hook-simple-english

Port of [jyooi/agent-simple-english](https://github.com/jyooi/agent-simple-english)
to the rushi harness hook ABI.

Enforces ASD-STE100 Simplified Technical English writing rules on agent
prose, file edits, and git commit messages.

## Windows

| Window         | Behaviour                                                       |
|----------------|-----------------------------------------------------------------|
| `tool.before`  | Lints `write`, `edit`, and `bash` (git commit) calls with diff-aware filtering. |
| `model.before` | Injects the byte-stable rule summary into `prompt_fragments`. Does not touch `request.input`. |
| `run.idle`     | Lints the last assistant reply. Gates the loop on hard violations. |

## Rules

| Rule ID                         | Default  | Checks                          |
|---------------------------------|----------|---------------------------------|
| `contraction`                   | hard     | `'re`, `'ve`, `'ll`, `'d`, `'m`, `'s` contractions |
| `semicolon`                     | hard     | Any `;` in prose (fenced code is skipped) |
| `sentence-length`               | hard     | Sentences over 25 words         |
| `paragraph-length`              | hard     | Paragraphs over the cap. Prose defaults to 6. Source kinds default off. |
| `phrasal-verb`                  | hard     | "carry out", "spin up", etc.    |
| `dictionary-not-approved-word`  | hard     | ASD-STE100 not-approved words   |
| `invalid-suppression`           | hard     | Bad `ste-disable-next-line`     |
| `verb-progressive`              | hard     | *(skipped — needs POS tagger)*  |
| `verb-passive`                  | soft     | *(skipped — needs POS tagger)*   |
| `verb-perfect`                  | hard     | *(skipped — needs POS tagger)*   |
| `hedging`                       | soft     | "it is important to note", etc. |
| `marketing`                     | soft     | "seamless", "robust", etc.      |

## Code exemption

Prose and commit-message text skips code. A fenced block starts with
three or more backticks or tildes, with at most three leading
spaces. Lines inside the fence are not linted. A backtick code span
is also skipped. A span holds one line. This keeps rules such as
`semicolon` from tripping on code.

Sentence splitting is code-aware. Dotted identifiers like `run.idle`
and numeric literals like `127.0.0.1` do not end sentences.
A `#!` shebang at line start is code, not prose.
Source-file comment lines close their own sentences. A run of
period-free comment lines must not grow into one giant sentence.

## Config

Place `.simple-english.json` in the project root or set
`$SIMPLE_ENGLISH_CONFIG` to a config path.

```json
{
  "rules": { "semicolon": "soft" },
  "max_sentence_words": 20,
  "max_paragraph_sentences": 8,
  "exempt_block_quotes": true
}
```

Set `max_paragraph_sentences` to change the paragraph cap.
Use 0 to turn the rule off.
Prose kinds default to 6. Source kinds default off.

## Diff-aware linting

`write` and `edit` calls are linted against the previous file content on
disk. Only violations on lines that changed are reported; pre-existing
violations on untouched lines are suppressed. New files (no previous
content) are linted in full.

## Reply gating (`run.idle`)

When the loop goes idle, the hook lints the last assistant reply.
On hard violations it emits a logged follow-up `continue` with a
`message`. The kernel logs that message as a `user_message` in the
`follow` queue and drains it as a new model turn. The model revises
the flagged lines within the same run.

- The follow-up is a genuine `user_message`. The TUI shows it as a
  user panel. That is the accepted cost of retiring the silent
  refire mechanism (FT-003).
- The message lists the hard and soft violations. It tells the model
  to revise only the flagged lines, keep the meaning, not re-post
  the reply, and not answer its own open questions.
- The `model.before` fragment stays byte-stable. It carries the rule
  summary only, so the cached prompt prefix holds.
- The TUI row widget shows the counts until the reply is clean.
- Each reply identity can be gated at most once. Up to 3 consecutive
  gates run before the hook stops (`MAX_GATE_COUNT`). The run stops
  with the last violating reply standing.

State is persisted in `<session>/simple-english-state.json` (`hard`,
`soft`, `last_identity`, `gate_count`, `gated_replies`).

## TUI status widget

The `simple-english-ext/` subdirectory contains a TUI extension that
reads the state file and renders a one-line status in the row slot:

    Writing-rule reply: n hard, m soft

Red when hard > 0, yellow when soft-only, green when clean.

## Registration

In `config.toml`:

```toml
[[hooks.on]]
window  = "tool.before"
command = "harness-hook-simple-english"

[[hooks.on]]
window  = "model.before"
command = "harness-hook-simple-english"

[[hooks.on]]
window  = "run.idle"
command = "harness-hook-simple-english"
```

For the TUI widget, place (or symlink) `simple-english-ext/` under
`ui_extensions/simple-english/` in the extensions tree and build it:

```sh
cd simple-english-ext && cargo build --release
```

The binary lands in `simple-english-ext/target/release/simple-english-ext`.

## Build

```sh
cargo build --release
```

The hook binary lands in `target/release/harness-hook-simple-english`.
