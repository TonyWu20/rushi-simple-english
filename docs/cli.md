# rushi-ste — Standalone CLI

`rushi-ste` checks text against the ASD-STE100 writing rules.
It stands beside the `harness-hook-simple-english` hook binary.
Both share the lint core in the `simple_english` lib target.

## Usage

    rushi-ste something.md
    cat something.md | rushi-ste

With no file argument, the tool reads stdin.
The value `-` also selects stdin.
The report labels stdin input as `-`.

## Options

| Flag            | Effect                                                          |
|-----------------|-----------------------------------------------------------------|
| `-h, --help`    | Show the help text. Exit 0.                                     |
| `-V, --version` | Show the version. Exit 0.                                       |
| `--json`        | Print the report as one JSON object.                            |
| `--hard-only`   | Exit 1 only when a hard violation exists. Soft ones still print |
| `--kind <KIND>` | Force the lint kind. Values: `prose-file`, `slash-source`, `hash-source`, `commit-message` |

## Kind inference

A file argument infers the kind from the extension.
The table is the `classify_path` logic the hook shares with the CLI.
Source extensions lint comment-only.
Data and binary extensions skip, as do the `LICENSE` and `COPYING` families.
Stdin input defaults to `prose-file`.
The `--kind` flag overrides inference.

## Streams and exit codes

The violation report goes to **stdout**.
It is the tool's output, not an operational diagnostic.
This follows the linter convention: `eslint`, `shellcheck`,
`flake8`, and `gitleaks` all write findings to stdout.
The pass/fail verdict is the exit code.

| Channel | Content                                                        |
|---------|----------------------------------------------------------------|
| stdout  | The report: summary line plus one detail line per violation. In `--json` mode, one JSON object. |
| stderr  | Operational errors: unreadable file, unknown flag, bad kind. Plus the skip-kind note. |
| exit 0  | Clean. Or only soft violations with `--hard-only`.             |
| exit 1  | Violations found (default: hard or soft).                      |
| exit 2  | Usage or input error.                                          |

## Human report format

    something.md: 2 violation(s), 2 hard, 0 soft
    - line 3, column 17 [hard] [phrasal-verb] Use an approved word instead of "carry out". Suggested fix: use "do".

A clean file prints one line: `something.md: clean`.

## JSON report

    {
      "file": "something.md",
      "kind": "prose-file",
      "total": 2, "hard": 2, "soft": 0,
      "violations": [
        {
          "line": 3, "column": 17,
          "rule": "phrasal-verb", "severity": "hard",
          "message": "Use an approved word instead of \"carry out\".",
          "suggestions": ["do"]
        }
      ]
    }

## Config

The CLI reuses the hook config lookup.
It reads `.simple-english.json` in the current working directory.
The `SIMPLE_ENGLISH_CONFIG` environment variable names an explicit path.
Both forms work the same as in the hook.

## Build

    cargo build --release

The binary lands in `target/release/rushi-ste`.

The Nix flake exposes a `rushi-ste` package.
It carries only the checker binary, in `$out/bin`.

    nix run .#rushi-ste -- --help

The hook package still carries both binaries.
The kernel only runs the registered hook command.
The extra binary is inert.
