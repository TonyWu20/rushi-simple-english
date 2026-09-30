# Path classification: nix and legal files

## Context

`classify_path` sent two file classes to the catch-all `ProseFile`.

1. `.nix` files: `nix` matched no extension arm. The hook
   treated Nix code as prose. Top-level lines carry `;` as Nix
   syntax. The hard `semicolon` rule blocked those edits.
2. Extensionless legal files (`LICENSE`, `COPYING`): the catch-all
   made them prose. MIT license text uses words the STE dictionary
   flags. The hook blocked license writes at all.

## Decision

`src/types.rs` adds a `LintKind::Skip` variant.

1. `classify_path` skips well-known legal filenames before the
   extension match: `LICENSE`, `LICENSE.*`, `COPYING*`.
2. The skipped extensions (`json`, `css`, ...) now resolve to the
   same `Skip` kind. This makes the skip list a true no-op. It
   matched the code comment that already called it a no-op.
3. `classify_path` adds `nix` to the `HashSource` arm. Nix uses
   `#` comments. The `semicolon` rule skips source kinds.

## Consequences

- A `flake.nix` edit with `;` at the top level passes.
- A `LICENSE` write passes. The license text stays intact.
- The skipped extensions now lint as a no-op. Before, they
  linted as prose.
- Prose files keep all rules. A `notes.md` write still blocks on
  hard violations.

## Tests

- `main.rs`: `classify_nix_is_hash`, `classify_license_is_skipped`,
  `classify_json_is_skipped`.
- `engine.rs`: `skip_kind_extracts_no_prose`.

## Commits

- See the pull request for issue #4 on the remote.
