//! Path classification into a `LintKind`.
//!
//! Shared by the hook binary and the `rushi-ste` CLI.

use crate::types::LintKind;

/// Classify a file path into a `LintKind`.
pub fn classify_path(path: &str) -> LintKind {
    let lower = path.to_lowercase();

    // Well-known extensionless legal filenames (LICENSE, COPYING, and
    // the LICENSE.* / COPYING* variants). Canonical license text
    // cannot be reworded to the STE dictionary, so these skip
    // linting entirely.
    let basename = lower.rsplit('/').next().unwrap_or("");
    if basename == "license" || basename.starts_with("license.") || basename.starts_with("copying")
    {
        return LintKind::Skip;
    }

    let ext = lower
        .rsplit('.')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("");

    // Skipped extensions: data files, binary artifacts, generated
    // output. No prose is extracted, so these lint as a no-op.
    // Binary kinds also protect the post-check: a `.gz` mention in
    // a bash command never snapshots.
    const SKIP: &[&str] = &[
        "css", "scss", "less", "json", "jsonc", "svg", "xml", "typ", "csv", "tsv", "lock", "gz",
        "zip", "tar", "bz2", "xz", "7z", "rar", "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico",
        "tiff", "pdf", "bin", "exe", "so", "dylib", "wasm", "o", "a", "class", "jar", "pyc", "db",
        "sqlite",
    ];
    if SKIP.contains(&ext) {
        return LintKind::Skip;
    }

    match ext {
        "html" | "htm" => LintKind::ProseFile, // simplified: treat as prose
        "rs" | "go" | "java" | "c" | "h" | "cpp" | "hpp" | "cc" | "cs" | "swift" | "kt"
        | "scala" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => LintKind::SlashSource,
        "sh" | "bash" | "zsh" | "py" | "rb" | "yaml" | "yml" | "toml" | "pl" | "nix" => {
            LintKind::HashSource
        }
        _ => LintKind::ProseFile,
    }
}
