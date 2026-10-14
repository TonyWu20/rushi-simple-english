# Bash write post-check (`tool.after`)

## Context

This hook gates `write`, `edit`, and `bash` (git commit) calls at
`tool.before`. In a harness configured with the `bash` tool only,
all file writes flow through `bash`. The pre-check cannot gate them:
the agent has many ways to write a file. `echo`, heredoc, `tee`,
`sed -i`, a script, a pipe. No static parser can enumerate the forms.
An allow-list of write forms leaves silent gaps.

## Decision (2026-10-15, user)

Check the disk, not the command.

1. **Pure post-check.** No static `tool.before` block on bash write
   forms. The commit gate stays. `tool.before` gains a snapshot
   step. `tool.after` gains the check.
2. **Code files lint comment-only.** Code targets resolve to
   `SlashSource` or `HashSource` via the existing `classify_path`.
   Data and binary targets resolve to `Skip` and never lint.
3. **Path tokens anywhere.** The mention scan takes path-like tokens
   anywhere in the command. No write-verb context is required. The
   disk diff filters out reads.
4. **Run until clean.** No gate cap. A file with open hard
   violations lints in full on its next change. The check repeats
   until a pass comes back clean.

Supporting decisions made by the implementer:

5. Binary extensions join the skip list in `classify_path` (`gz`,
   `zip`, `tar`, `png`, `pdf`, `so`, `db`, and the rest). A NUL
   guard skips binary content at lint time.
6. One MiB caps the snapshot and the linted content.
7. The post-check reuses diff-aware linting for changed files with
   no open violations. Open files lint in full.
8. Soft violations splice into the report as warnings. Only hard
   violations open a file.

## Mechanics

- `src/bash.rs` (new): the mention scan, the sidecar, the open
  state, and the post-check.
- `tool.before`: for each bash call, scan the command for path
  tokens. A token is a run of path characters ending in a dot plus
  a two-to-six letter extension. Resolve it against the session
  cwd. Snapshot the previous content into
  `<session>/simple-english-sidecar.json`.
- `tool.after`: read the sidecar. Re-read each file. Lint the files
  that changed. Splice a report into the call's tool result via a
  `results` state map. The original command output and exit code
  stay. Persist open-violation state in
  `<session>/simple-english-open.json`.

## Consequences

- Prose written through any shell form gets checked. The one miss
  is a file written by a script without the path in the command.
- No call is blocked by the new path. The write has already
  happened. Feedback lands in the tool result.
- The model keeps fixing an open file until a pass is clean.
- `classify_path` changes affect the existing `write` and `edit`
  gates too. Binary writes that used to lint as prose now skip.

## Registration

The hook must sit on both `tool.before` and `tool.after` in
`config.toml`. Without the `tool.after` pipeline entry the
snapshot writes nothing into the loop.

## Tests

- `bash::tests`: mention scan, sidecar and open-state round trips,
  snapshot, post-check, open-file full lint, settle, binary,
  deleted file.
- `tests::tool_after_state_*`: splice, silent on unchanged, noop
  without a sidecar.
- `tests::classify_gz_is_skipped`, `tests::classify_png_is_skipped`.

## Commits

- Version bump 0.4.1 to 0.5.0. New capability. The flake reads the
  version from `Cargo.toml`.
