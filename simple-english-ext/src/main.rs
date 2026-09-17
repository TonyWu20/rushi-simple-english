//! The `simple-english-ext` TUI status widget.
//!
//! Renders the writing-rule reply status in the host-reserved row slot
//! above the input box (docs/ui-extension.md section 4, `row`
//! capability). The hook (`harness-hook-simple-english` on the
//! `run.idle` window) lints the last assistant reply and writes
//! `<session>/simple-english-state.json`. This extension reads that
//! state file on every `row` op (tick cadence) and renders:
//!
//!   " Writing-rule reply: n hard, m soft "
//!
//! Red when `n > 0`, amber when only soft violations, green when
//! clean. No state file means no row.
//!
//! Session-dir resolution (issue #3): when the host hands a
//! pre-resolved `session_dir` on the op (rushi-tui#14), the state
//! file is read directly from there and the extension never touches
//! `CONFIG` / `RUSHI_CWD`. Older hosts that omit `session_dir` fall
//! back to the `CONFIG` + `RUSHI_CWD` derivation, resolved lazily on
//! the first such op and cached.
//!
//! Protocol (docs/ui-extension.md section 4): one JSON op per line on
//! stdin, one JSON reply per op on stdout. Unknown ops get no reply.

use std::io::{BufRead, Write};

use serde::Deserialize;
use serde_json::json;

/// One host op. Fields the extension does not use are optional: serde
/// skips unknown fields, so the host may grow the op later.
#[derive(Debug, Deserialize)]
struct Op {
    #[serde(default)]
    v: Option<u64>,
    #[serde(default)]
    op: Option<String>,
    /// The active session name, when the host reports one.
    #[serde(default)]
    session: Option<String>,
    /// The host-resolved absolute session directory (`sessions_root`
    /// joined with the session name, rushi-tui#14). When present it
    /// takes precedence over the `CONFIG` / `RUSHI_CWD` fallback
    /// derivation (issue #3).
    #[serde(default)]
    session_dir: Option<String>,
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::LineWriter::new(stdout.lock());

    // Fallback sessions root: resolved lazily from the `CONFIG` env var
    // the same way goal-ext does (`[paths] sessions_root`; a relative
    // value anchors to `RUSHI_CWD` when the host exported it, else to
    // the config directory). Only touched when an op lacks
    // `session_dir`, so a host that hands over the resolved path
    // (rushi-tui#14) never makes us read `CONFIG` / `RUSHI_CWD`.
    let mut fallback_root: Option<Option<String>> = None;

    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let Ok(op) = serde_json::from_str::<Op>(&line) else {
            continue;
        };
        if op.v != Some(1) {
            continue;
        }
        if let Some("row") = op.op.as_deref() {
            let session_dir = op.session_dir.as_deref().filter(|s| !s.is_empty());
            let root: Option<&str> = match session_dir {
                // Host supplied a session dir: skip the fallback
                // resolution entirely.
                Some(_) => None,
                None => {
                    if fallback_root.is_none() {
                        fallback_root = Some(std::env::var("CONFIG")
                            .ok()
                            .and_then(|c| sessions_root(std::path::Path::new(&c))));
                    }
                    fallback_root.as_ref().unwrap().as_deref()
                }
            };
            let lines = build_row_lines(
                op.session.as_deref(),
                session_dir,
                root,
            );
            let reply = json!({
                "v": 1,
                "op": "row_spec",
                "lines": lines,
            });
            let _ = writeln!(out, "{reply}");
            let _ = out.flush();
        }
        // Unknown ops: no reply.
    }
}

/// Build the row content for the current session.
///
/// Returns a list of `[text, style]` line pairs, matching the
/// `row_spec` shape goal-ext uses. Empty list = the host renders no
/// row.
///
/// State-file path resolution:
/// - When `session_dir` is present and non-empty, read
///   `<session_dir>/simple-english-state.json` directly. The host
///   already resolved the session dir (rushi-tui#14); the extension
///   does no `CONFIG`/`RUSHI_CWD` derivation.
/// - Otherwise fall back to `<sessions_root>/<session>/…`, or to the
///   session value used directly as a directory when no root exists.
fn build_row_lines(
    session: Option<&str>,
    session_dir: Option<&str>,
    sessions_root: Option<&str>,
) -> Vec<serde_json::Value> {
    // 1. Host-supplied session dir wins (rushi-tui#14, issue #3).
    let state_path = match session_dir.filter(|d| !d.is_empty()) {
        Some(dir) => std::path::Path::new(dir).join("simple-english-state.json"),
        None => {
            // 2. Fallback: derive from session + sessions root.
            let session = match session {
                Some(s) if !s.is_empty() => s,
                _ => return Vec::new(),
            };
            match sessions_root {
                Some(root) => std::path::Path::new(root).join(session).join("simple-english-state.json"),
                None => {
                    // The host did not give us a sessions root: try the
                    // session value as a direct directory (it may already
                    // be a path).
                    std::path::Path::new(session).join("simple-english-state.json")
                }
            }
        }
    };

    render_state(&state_path)
}

/// Read the state file and build the row line(s) for it.
///
/// Missing or unparseable file → empty list (no row).
fn render_state(state_path: &std::path::Path) -> Vec<serde_json::Value> {
    let Ok(raw) = std::fs::read_to_string(state_path) else {
        return Vec::new(); // No state yet: no row.
    };
    let Ok(state) = serde_json::from_str::<State>(&raw) else {
        return Vec::new();
    };

    let text = format!(
        " Writing-rule reply: {} hard, {} soft ",
        state.hard, state.soft
    );

    let style = if state.hard > 0 {
        json!({"fg": "red", "bold": true})
    } else if state.soft > 0 {
        json!({"fg": "yellow", "bold": true})
    } else {
        json!({"fg": "green"})
    };

    vec![json!([text, style])]
}

/// The subset of the hook's state file this widget reads.
#[derive(Debug, Deserialize)]
struct State {
    #[serde(default)]
    hard: usize,
    #[serde(default)]
    soft: usize,
}

/// Read `[paths] sessions_root` from the config file named by the
/// `CONFIG` env var. A relative value (the common case, e.g.
/// "sessions") resolves against `RUSHI_CWD` when the TUI host
/// exported it (the host working dir — the same base the kernel
/// uses for its session dirs; under a Nix build the config file
/// lives in the read-only store, so its directory must not be
/// used). Without `RUSHI_CWD` the config directory is the base.
/// Defaults to `<base>/sessions` when the key is absent.
fn sessions_root(config_path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(config_path).ok()?;
    let v: toml::Value = text.parse().ok()?;
    let config_dir = config_path.parent()?.to_path_buf();
    let base_dir = std::env::var_os("RUSHI_CWD")
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or(config_dir);
    if let Some(root) = v
        .get("paths")
        .and_then(|p| p.get("sessions_root"))
        .and_then(|r| r.as_str())
    {
        let p = std::path::Path::new(root);
        return Some(if p.is_absolute() {
            p.to_string_lossy().into_owned()
        } else {
            base_dir.join(p).to_string_lossy().into_owned()
        });
    }
    Some(base_dir.join("sessions").to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_hidden_without_session() {
        assert!(build_row_lines(None, None, Some("/root")).is_empty());
        assert!(build_row_lines(Some(""), None, Some("/root")).is_empty());
    }

    #[test]
    fn row_hidden_without_state_file() {
        let lines = build_row_lines(
            Some("no-such-session"),
            None,
            Some("/nonexistent-root"),
        );
        assert!(lines.is_empty());
    }

    #[test]
    fn row_shows_hard_counts_in_red() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("s1");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("simple-english-state.json"),
            r#"{"hard": 2, "soft": 1}"#,
        )
        .unwrap();
        let lines = build_row_lines(
            Some("s1"),
            None,
            Some(dir.path().to_str().unwrap()),
        );
        assert_eq!(lines.len(), 1);
        let pair = lines[0].as_array().expect("a [text, style] pair");
        assert!(pair[0].as_str().unwrap().contains("2 hard, 1 soft"));
        assert_eq!(
            pair[1].get("fg"),
            Some(&serde_json::Value::String("red".into()))
        );
    }

    #[test]
    fn row_shows_soft_only_in_yellow() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("s1");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("simple-english-state.json"),
            r#"{"hard": 0, "soft": 3}"#,
        )
        .unwrap();
        let lines = build_row_lines(
            Some("s1"),
            None,
            Some(dir.path().to_str().unwrap()),
        );
        let pair = lines[0].as_array().unwrap();
        assert_eq!(
            pair[1].get("fg"),
            Some(&serde_json::Value::String("yellow".into()))
        );
    }

    #[test]
    fn row_shows_clean_in_green() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("s1");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("simple-english-state.json"),
            r#"{"hard": 0, "soft": 0}"#,
        )
        .unwrap();
        let lines = build_row_lines(
            Some("s1"),
            None,
            Some(dir.path().to_str().unwrap()),
        );
        let pair = lines[0].as_array().unwrap();
        assert_eq!(
            pair[1].get("fg"),
            Some(&serde_json::Value::String("green".into()))
        );
    }

    // ── issue #3: host-supplied session_dir ──

    #[test]
    fn row_prefers_session_dir_over_sessions_root() {
        // Two locations with different state: the host session_dir and a
        // sessions_root that would otherwise be used. The host dir wins.
        let host_root = tempfile::tempdir().unwrap();
        let host_sess = host_root.path().join("sess");
        std::fs::create_dir_all(&host_sess).unwrap();
        std::fs::write(
            host_sess.join("simple-english-state.json"),
            r#"{"hard": 4, "soft": 2}"#,
        )
        .unwrap();

        let fallback_root = tempfile::tempdir().unwrap();
        let fallback_sess = fallback_root.path().join("sess");
        std::fs::create_dir_all(&fallback_sess).unwrap();
        std::fs::write(
            fallback_sess.join("simple-english-state.json"),
            r#"{"hard": 9, "soft": 9}"#,
        )
        .unwrap();

        let lines = build_row_lines(
            Some("sess"),
            Some(host_sess.to_str().unwrap()),
            Some(fallback_root.path().to_str().unwrap()),
        );
        assert_eq!(lines.len(), 1);
        let pair = lines[0].as_array().unwrap();
        // Reflects the host dir's state (4/2), not the fallback root (9/9).
        assert!(pair[0].as_str().unwrap().contains("4 hard, 2 soft"));
        assert_eq!(
            pair[1].get("fg"),
            Some(&serde_json::Value::String("red".into()))
        );
    }

    #[test]
    fn row_session_dir_without_session_name() {
        // A session_dir is self-contained: it works with no session
        // name and no sessions_root (the host resolves the full path).
        let dir = tempfile::tempdir().unwrap();
        let sess_dir = dir.path().join("my-sess");
        std::fs::create_dir_all(&sess_dir).unwrap();
        std::fs::write(
            sess_dir.join("simple-english-state.json"),
            r#"{"hard": 1, "soft": 1}"#,
        )
        .unwrap();

        let lines = build_row_lines(
            None, // no session name
            Some(sess_dir.to_str().unwrap()),
            None, // no sessions_root
        );
        assert_eq!(lines.len(), 1);
        let pair = lines[0].as_array().unwrap();
        assert!(pair[0].as_str().unwrap().contains("1 hard, 1 soft"));
    }

    #[test]
    fn row_empty_session_dir_uses_sessions_root() {
        // An empty session_dir is treated as absent; the sessions_root
        // derivation still applies.
        let dir = tempfile::tempdir().unwrap();
        let sess = dir.path().join("fb");
        std::fs::create_dir_all(&sess).unwrap();
        std::fs::write(
            sess.join("simple-english-state.json"),
            r#"{"hard": 1, "soft": 0}"#,
        )
        .unwrap();

        let lines = build_row_lines(
            Some("fb"),
            Some(""), // empty session_dir is ignored
            Some(dir.path().to_str().unwrap()),
        );
        assert_eq!(lines.len(), 1);
        let pair = lines[0].as_array().unwrap();
        assert!(pair[0].as_str().unwrap().contains("1 hard, 0 soft"));
    }

    #[test]
    fn row_session_dir_without_state_file_is_hidden() {
        // A session_dir with no state file renders no row.
        let dir = tempfile::tempdir().unwrap();
        let lines = build_row_lines(
            Some("s"),
            Some(dir.path().to_str().unwrap()),
            None,
        );
        assert!(lines.is_empty());
    }
}
