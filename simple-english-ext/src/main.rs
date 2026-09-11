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
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::LineWriter::new(stdout.lock());

    // Resolve the sessions root once at startup, the same way
    // goal-ext does: the `CONFIG` env var names the harness config
    // file; `[paths] sessions_root` names the sessions root.
    let sessions_root = std::env::var("CONFIG")
        .ok()
        .and_then(|c| sessions_root(std::path::Path::new(&c)));

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
        match op.op.as_deref() {
            Some("row") => {
                let lines = build_row_lines(
                    op.session.as_deref(),
                    sessions_root.as_deref(),
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
            _ => {}
        }
    }
}

/// Build the row content for the current session.
///
/// Returns a list of `[text, style]` line pairs, matching the
/// `row_spec` shape goal-ext uses. Empty list = the host renders no
/// row.
fn build_row_lines(session: Option<&str>, sessions_root: Option<&str>) -> Vec<serde_json::Value> {
    let session = match session {
        Some(s) if !s.is_empty() => s,
        _ => return Vec::new(),
    };

    let state_path = match sessions_root {
        Some(root) => std::path::Path::new(root).join(session).join("simple-english-state.json"),
        None => {
            // The host did not give us a sessions root: try the session
            // value as a direct directory (it may already be a path).
            std::path::Path::new(session).join("simple-english-state.json")
        }
    };

    let Ok(raw) = std::fs::read_to_string(&state_path) else {
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
/// `CONFIG` env var. Relative paths resolve against the config
/// directory. Defaults to `<config_dir>/sessions` when absent.
fn sessions_root(config_path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(config_path).ok()?;
    let v: toml::Value = text.parse().ok()?;
    let config_dir = config_path.parent()?.to_path_buf();
    if let Some(root) = v
        .get("paths")
        .and_then(|p| p.get("sessions_root"))
        .and_then(|r| r.as_str())
    {
        let p = std::path::Path::new(root);
        return Some(if p.is_absolute() {
            p.to_string_lossy().into_owned()
        } else {
            config_dir.join(p).to_string_lossy().into_owned()
        });
    }
    Some(config_dir.join("sessions").to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_hidden_without_session() {
        assert!(build_row_lines(None, Some("/root")).is_empty());
        assert!(build_row_lines(Some(""), Some("/root")).is_empty());
    }

    #[test]
    fn row_hidden_without_state_file() {
        let lines = build_row_lines(Some("no-such-session"), Some("/nonexistent-root"));
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
        let lines = build_row_lines(Some("s1"), Some(dir.path().to_str().unwrap()));
        assert_eq!(lines.len(), 1);
        let pair = lines[0].as_array().expect("a [text, style] pair");
        assert!(pair[0].as_str().unwrap().contains("2 hard, 1 soft"));
        assert_eq!(pair[1].get("fg"), Some(&serde_json::Value::String("red".into())));
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
        let lines = build_row_lines(Some("s1"), Some(dir.path().to_str().unwrap()));
        let pair = lines[0].as_array().unwrap();
        assert_eq!(pair[1].get("fg"), Some(&serde_json::Value::String("yellow".into())));
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
        let lines = build_row_lines(Some("s1"), Some(dir.path().to_str().unwrap()));
        let pair = lines[0].as_array().unwrap();
        assert_eq!(pair[1].get("fg"), Some(&serde_json::Value::String("green".into())));
    }
}
