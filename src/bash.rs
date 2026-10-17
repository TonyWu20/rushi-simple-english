//! Post-check of bash file writes for the `tool.after` window.
//!
//! The harness exposes file writes through many shell forms. No
//! static parser can enumerate them. This module inverts the
//! problem: at `tool.before` it snapshots the previous content of
//! every path token the command mentions. At `tool.after` it reads
//! the current content back and lints the files that actually
//! changed on disk.
//!
//! A file with open hard violations lints in full on its next
//! change. The check runs until the file is clean. There is no
//! gate cap.
//!
//! Sidecar files live in the session dir:
//!
//! - `simple-english-sidecar.json` — per-call path snapshots
//! - `simple-english-open.json` — per-file open-violation state

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::engine;
use crate::feedback;
use crate::types::{LintConfig, LintKind, Severity};

/// Largest file size the hook snapshots or lints, in bytes.
pub const MAX_FILE_BYTES: usize = 1024 * 1024;

const SIDECAR_FILE: &str = "simple-english-sidecar.json";
const OPEN_FILE: &str = "simple-english-open.json";

/// One snapshotted file in one tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapFile {
    /// Absolute file path.
    pub path: String,
    /// Content at snapshot time. `None` when the file was absent.
    pub prev: Option<String>,
}

/// One tool call's snapshot entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapCall {
    pub id: String,
    pub files: Vec<SnapFile>,
}

/// The sidecar state written at `tool.before`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Sidecar {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub calls: Vec<SnapCall>,
}

/// Open-violation state, persisted per session.
///
/// `files` maps an absolute path to the last hard violation count.
/// A listed file lints in full on its next change.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenState {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub files: BTreeMap<String, usize>,
}

/// Path to the sidecar file in the session dir.
pub fn sidecar_path(dir: &Path) -> PathBuf {
    dir.join(SIDECAR_FILE)
}

/// Load the sidecar. Returns `None` when absent or corrupt.
pub fn load_sidecar(dir: &Path) -> Option<Sidecar> {
    let raw = fs::read_to_string(sidecar_path(dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Save the sidecar. The session dir must already exist.
pub fn save_sidecar(dir: &Path, sidecar: &Sidecar) -> std::io::Result<()> {
    let raw = serde_json::to_string(sidecar)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(sidecar_path(dir), raw)
}

/// Remove the sidecar after the `tool.after` step consumed it.
pub fn clear_sidecar(dir: &Path) {
    let _ = fs::remove_file(sidecar_path(dir));
}

/// Path to the open-violation state file in the session dir.
pub fn open_path(dir: &Path) -> PathBuf {
    dir.join(OPEN_FILE)
}

/// Load the open-violation state. Returns the default when absent
/// or corrupt.
pub fn load_open(dir: &Path) -> OpenState {
    let Ok(raw) = fs::read_to_string(open_path(dir)) else {
        return OpenState::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Save the open-violation state.
pub fn save_open(dir: &Path, state: &OpenState) -> std::io::Result<()> {
    let raw = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(open_path(dir), raw)
}

// ── Path-mention scan ─────────────────────────────────────────────────────

/// Match a path-like token: a run of path characters that ends in a
/// dot plus a two-to-six letter extension.
///
/// The match catches plain, relative, and quoted paths, flag values
/// (`--file=a.md`), and tokens after `2>` redirects. It skips
/// version numbers (digit endings) and bare words without an
/// extension. Globs break the token at `*`, so `docs/*.md` yields
/// the degenerate token `.md`; the disk check makes it harmless.
fn path_token_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z0-9._/~-]*\.[A-Za-z]{2,6}").expect("static pattern"))
}

/// Find path-like tokens in a bash command string.
///
/// Scans the whole command. The post-check does not need to know
/// whether a token is a write target, a read source, or a comment.
/// The disk diff filters out everything that did not change.
/// Results are deduplicated in first-seen order.
pub fn find_path_mentions(command: &str) -> Vec<String> {
    let re = path_token_re();
    let mut out: Vec<String> = Vec::new();
    for m in re.find_iter(command) {
        let tok = m.as_str().to_string();
        if !out.contains(&tok) {
            out.push(tok);
        }
    }
    out
}

/// Resolve a path token to an absolute path for snapshotting.
///
/// A leading `~` expands to `$HOME`. Relative tokens resolve
/// against the session's project cwd, like `resolve_file_path`.
pub fn resolve_mention(token: &str, session_dir: &Option<PathBuf>) -> PathBuf {
    if let Some(rest) = token.strip_prefix('~') {
        let home = std::env::var("HOME").unwrap_or_default();
        return PathBuf::from(home).join(rest);
    }
    crate::resolve_file_path(token, session_dir)
}

/// Snapshot the mentioned files of one bash command.
///
/// Only files that classify as lintable are snapshotted. Files of
/// the skip kind and files over `MAX_FILE_BYTES` are dropped. A
/// missing file snapshots with `prev: None` (a new file lints in
/// full at `tool.after`).
pub fn snapshot_call(call_id: &str, command: &str, session_dir: &Option<PathBuf>) -> SnapCall {
    let mut files: Vec<SnapFile> = Vec::new();
    for token in find_path_mentions(command) {
        let abs = resolve_mention(&token, session_dir);
        let kind = crate::classify::classify_path(&abs.to_string_lossy());
        if kind == LintKind::Skip {
            continue;
        }
        let Some(meta) = fs::metadata(&abs).ok() else {
            files.push(SnapFile {
                path: abs.to_string_lossy().into_owned(),
                prev: None,
            });
            continue;
        };
        if meta.len() > MAX_FILE_BYTES as u64 {
            eprintln!(
                "[simple-english] snapshot skipped (too large): {}",
                abs.display()
            );
            continue;
        }
        // Non-UTF-8 or unreadable content snapshots as absent. The
        // post-check skips binary content by NUL guard.
        let prev = fs::read_to_string(&abs).ok();
        files.push(SnapFile {
            path: abs.to_string_lossy().into_owned(),
            prev,
        });
    }
    SnapCall {
        id: call_id.to_string(),
        files,
    }
}

// ── Post-check ────────────────────────────────────────────────────────────

/// Post-check one call's snapshotted files against the disk.
///
/// Lints only files whose content changed since the snapshot. A
/// changed file with open violations lints in full; all other
/// changed files lint diff-aware against their snapshot. Returns
/// the report text to splice into the tool result, or `None`.
/// Updates `open` for every checked file.
pub fn check_call(call: &SnapCall, open: &mut OpenState, config: &LintConfig) -> Option<String> {
    let mut hard_sections: Vec<String> = Vec::new();
    let mut soft_sections: Vec<String> = Vec::new();

    for f in &call.files {
        // Absent or not a regular file: the write is gone. Clear
        // any open-violation record for it.
        match fs::metadata(&f.path) {
            Ok(m) if m.is_file() => {}
            _ => {
                open.files.remove(&f.path);
                continue;
            }
        }
        let cur = match fs::read_to_string(&f.path) {
            Ok(s) => s,
            Err(_) => {
                eprintln!(
                    "[simple-english] post-check skipped (unreadable): {}",
                    f.path
                );
                continue;
            }
        };
        // Unchanged since the snapshot: nothing to report.
        if f.prev.as_deref() == Some(cur.as_str()) {
            continue;
        }
        // Binary content is not prose.
        if cur.contains('\0') {
            eprintln!("[simple-english] post-check skipped (binary): {}", f.path);
            continue;
        }
        let kind = crate::classify::classify_path(&f.path);
        if kind == LintKind::Skip {
            continue;
        }
        let whole = open.files.contains_key(&f.path);
        let report = if whole {
            engine::lint(kind, &cur, config)
        } else {
            crate::diff::lint_write(kind, &cur, f.prev.as_deref(), config)
        };
        let hard: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.severity == Severity::Hard)
            .cloned()
            .collect();
        let soft: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.severity == Severity::Soft)
            .cloned()
            .collect();

        let display = display_path(&f.path);
        if hard.is_empty() {
            open.files.remove(&f.path);
            if !soft.is_empty() {
                soft_sections.push(feedback::format_violations(
                    &display,
                    "Writing-rule warnings in",
                    &soft,
                ));
            }
            continue;
        }
        open.files.insert(f.path.clone(), hard.len());
        hard_sections.push(feedback::format_violations(
            &display,
            "Writing-rule violations in",
            &hard,
        ));
    }

    if hard_sections.is_empty() && soft_sections.is_empty() {
        return None;
    }

    let mut text = String::from("[simple-english] Post-check of files changed by this command.\n");
    for s in &hard_sections {
        text.push_str(s);
        text.push('\n');
    }
    for s in &soft_sections {
        text.push_str(s);
        text.push('\n');
    }
    text.push_str(
        "Fix the hard violations. A later write to a listed file re-runs \
         this check until the file is clean.",
    );
    Some(text)
}

/// A stable label for reports: the absolute path as snapshotted.
fn display_path(abs: &str) -> String {
    abs.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_heredoc_target() {
        let cmd = "cat <<'EOF' > notes.md\nhello\nEOF\n";
        assert_eq!(find_path_mentions(cmd), vec!["notes.md".to_string()]);
    }

    #[test]
    fn mention_dedupes() {
        let cmd = "echo 'a' > docs/a.md && echo 'b' >> docs/a.md";
        assert_eq!(find_path_mentions(cmd), vec!["docs/a.md".to_string()]);
    }

    #[test]
    fn mention_flag_value() {
        let cmd = "tool --file=out.txt";
        assert_eq!(find_path_mentions(cmd), vec!["out.txt".to_string()]);
    }

    #[test]
    fn mention_two_files() {
        let cmd = "cp a.md b.md";
        assert_eq!(
            find_path_mentions(cmd),
            vec!["a.md".to_string(), "b.md".to_string()]
        );
    }

    #[test]
    fn mention_home_token() {
        let cmd = "cat ~/notes.md";
        assert_eq!(find_path_mentions(cmd), vec!["~/notes.md".to_string()]);
    }

    #[test]
    fn mention_stderr_redirect() {
        let cmd = "2> err.log cmd";
        assert_eq!(find_path_mentions(cmd), vec!["err.log".to_string()]);
    }

    #[test]
    fn mention_ignores_version_numbers() {
        let cmd = "cargo update rust-1.85.2";
        assert!(find_path_mentions(cmd).is_empty());
    }

    #[test]
    fn mention_ignores_bare_words() {
        let cmd = "git commit -m \"fix\"";
        assert!(find_path_mentions(cmd).is_empty());
    }

    #[test]
    fn mention_python_script() {
        let cmd = "python3 writer.py --input data";
        // `writer.py` carries a two-letter extension; it matches.
        assert_eq!(find_path_mentions(cmd), vec!["writer.py".to_string()]);
    }

    #[test]
    fn sidecar_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = Sidecar {
            version: 1,
            calls: vec![SnapCall {
                id: "c1".to_string(),
                files: vec![SnapFile {
                    path: "/tmp/a.md".to_string(),
                    prev: Some("hello".to_string()),
                }],
            }],
        };
        save_sidecar(dir.path(), &sidecar).unwrap();
        let loaded = load_sidecar(dir.path()).unwrap();
        assert_eq!(loaded.calls.len(), 1);
        assert_eq!(loaded.calls[0].files[0].prev.as_deref(), Some("hello"));
        clear_sidecar(dir.path());
        assert!(load_sidecar(dir.path()).is_none());
    }

    #[test]
    fn open_state_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = OpenState::default();
        state.files.insert("/tmp/a.md".to_string(), 3);
        save_open(dir.path(), &state).unwrap();
        let loaded = load_open(dir.path());
        assert_eq!(loaded.files.get("/tmp/a.md"), Some(&3));
    }

    /// Seed a fake session dir with a project cwd and a target file.
    fn seed(dir: &Path, target: &str, content: &str) -> PathBuf {
        let cwd = dir.join("project");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(dir.join("cwd"), cwd.to_string_lossy().as_ref()).unwrap();
        let p = cwd.join(target);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn snapshot_call_records_prev_and_new() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "notes.md", "old text");
        let p = seed(dir.path(), "x.json", "{}");
        let p_str = p.to_string_lossy().into_owned();
        let cmd = format!("cat <<EOF > notes.md\nnew\nEOF\nls {p_str}");
        let session = Some(dir.path().to_path_buf());
        let snap = snapshot_call("c1", &cmd, &session);
        // The json file is the skip kind: not snapshotted.
        let md = snap
            .files
            .iter()
            .find(|f| f.path.ends_with("notes.md"))
            .unwrap();
        assert_eq!(md.prev.as_deref(), Some("old text"));
        assert!(!snap.files.iter().any(|f| f.path.ends_with("x.json")));
    }

    #[test]
    fn check_call_reports_new_hard_violation() {
        let dir = tempfile::tempdir().unwrap();
        let p = seed(dir.path(), "notes.md", "clean start");
        let session = Some(dir.path().to_path_buf());
        let cmd = format!("echo rewrite > {}", p.to_string_lossy());
        let snap = snapshot_call("c1", &cmd, &session);
        // Simulate the write having happened.
        // A 26-word sentence breaches the default 25-word cap.
        // 26 words breach the default 25-word cap.
        let bad = format!("{}.", "lorem ".repeat(26).trim());
        fs::write(&p, bad).unwrap();
        let mut open = OpenState::default();
        let report = check_call(&snap, &mut open, &LintConfig::default()).unwrap();
        assert!(report.contains("notes.md"), "{report}");
        assert!(report.contains("sentence-length"), "{report}");
        assert!(!open.files.is_empty());
    }

    #[test]
    fn check_call_unchanged_file_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "notes.md", "clean");
        let session = Some(dir.path().to_path_buf());
        let snap = snapshot_call("c1", "ls", &session);
        assert!(snap.files.is_empty());
        let mut open = OpenState::default();
        assert!(check_call(&snap, &mut open, &LintConfig::default()).is_none());
    }

    #[test]
    fn check_call_settles_on_clean_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let p = seed(dir.path(), "notes.md", "old clean text");
        let session = Some(dir.path().to_path_buf());
        let snap = snapshot_call("c1", "echo x > notes.md", &session);
        let mut open = OpenState::default();
        // First pass: bad rewrite opens the file.
        // 26 words breach the default 25-word cap.
        let bad = format!("{}.", "lorem ".repeat(26).trim());
        fs::write(&p, bad).unwrap();
        assert!(check_call(&snap, &mut open, &LintConfig::default()).is_some());
        // Second pass: clean rewrite settles it.
        let snap2 = snapshot_call("c2", "echo y > notes.md", &session);
        fs::write(&p, "Clean text.").unwrap();
        let report = check_call(&snap2, &mut open, &LintConfig::default());
        assert!(report.is_none());
        assert!(open.files.is_empty());
    }

    #[test]
    fn check_call_open_file_lints_in_full() {
        let dir = tempfile::tempdir().unwrap();
        let p = seed(dir.path(), "notes.md", "old clean text");
        let session = Some(dir.path().to_path_buf());
        let snap = snapshot_call("c1", "echo x > notes.md", &session);
        let mut open = OpenState::default();
        // 26 words breach the default 25-word cap.
        let bad = format!("{}.", "lorem ".repeat(26).trim());
        fs::write(&p, &bad).unwrap();
        let r1 = check_call(&snap, &mut open, &LintConfig::default());
        assert!(r1.is_some());
        // Rewrite: still bad, and only one line changed from the
        // snapshot. A diff-aware lint would miss most of it. The
        // open flag forces a full lint.
        let snap2 = snapshot_call("c2", "echo y > notes.md", &session);
        let bad2 = format!("{bad}\nMore bad text here.");
        fs::write(&p, &bad2).unwrap();
        let r2 = check_call(&snap2, &mut open, &LintConfig::default()).unwrap();
        assert!(r2.contains("sentence-length"), "{r2}");
        assert!(!open.files.is_empty());
    }

    #[test]
    fn check_call_binary_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let p = seed(dir.path(), "notes.md", "clean");
        let session = Some(dir.path().to_path_buf());
        let snap = snapshot_call("c1", "dd > notes.md", &session);
        fs::write(&p, "a\0b").unwrap();
        let mut open = OpenState::default();
        assert!(check_call(&snap, &mut open, &LintConfig::default()).is_none());
    }

    #[test]
    fn check_call_deleted_file_clears_open() {
        let dir = tempfile::tempdir().unwrap();
        let p = seed(dir.path(), "notes.md", "old clean text");
        let session = Some(dir.path().to_path_buf());
        let snap = snapshot_call("c1", "rm notes.md", &session);
        let mut open = OpenState::default();
        open.files.insert(p.to_string_lossy().into_owned(), 1);
        fs::remove_file(&p).unwrap();
        let report = check_call(&snap, &mut open, &LintConfig::default());
        assert!(report.is_none());
        assert!(open.files.is_empty());
    }
}
