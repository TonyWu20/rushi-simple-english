# harness-hook-simple-english

Port of [jyooi/agent-simple-english](https://github.com/jyooi/agent-simple-english)
to the rushi harness hook ABI.

Enforces ASD-STE100 Simplified Technical English writing rules on agent
prose, file edits, and git commit messages.

## Windows

| Window         | Behaviour                                                       |
|----------------|-----------------------------------------------------------------|
| `tool.before`  | Lints `write`, `edit`, and `bash` (git commit) calls.          |
| `model.before` | Injects the active rule summary into the prompt fragment.       |

## Rules

| Rule ID                         | Default  | Checks                          |
|---------------------------------|----------|---------------------------------|
| `contraction`                   | hard     | `'re`, `'ve`, `'ll`, `'d`, `'m`, `'s` contractions |
| `semicolon`                     | hard     | Any `;` in prose                 |
| `sentence-length`               | hard     | Sentences over 25 words         |
| `paragraph-length`              | hard     | Paragraphs over 6 sentences     |
| `phrasal-verb`                  | hard     | "carry out", "spin up", etc.    |
| `dictionary-not-approved-word`  | hard     | ASD-STE100 not-approved words   |
| `invalid-suppression`           | hard     | Bad `ste-disable-next-line`     |
| `verb-progressive`              | hard     | *(skipped — needs POS tagger)*  |
| `verb-passive`                  | soft     | *(skipped — needs POS tagger)*   |
| `verb-perfect`                  | hard     | *(skipped — needs POS tagger)*   |
| `hedging`                       | soft     | "it is important to note", etc. |
| `marketing`                     | soft     | "seamless", "robust", etc.      |

## Config

Place `.simple-english.json` in the project root or set
`$SIMPLE_ENGLISH_CONFIG` to a config path.

```json
{
  "rules": { "semicolon": "soft" },
  "max_sentence_words": 20,
  "exempt_block_quotes": true
}
```

## Registration

In `config.toml`:

```toml
[[hooks.on]]
window  = "tool.before"
command = "harness-hook-simple-english"

[[hooks.on]]
window  = "model.before"
command = "harness-hook-simple-english"
```

## Build

```sh
cargo build --release
```

The binary lands in `target/release/harness-hook-simple-english`.
