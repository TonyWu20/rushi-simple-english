//! Reply-gate state and last-assistant-message extraction for the
//! `run.idle` window.
//!
//! The harness appends the full session transcript to
//! `<session>/events.jsonl`.  The last `assistant_message` event with
//! non-empty `content` is the model's user-facing reply.  This module
//! reads that reply, records the lint result in a sidecar state file,
//! and tracks which replies have already been gated so the loop cannot
//! oscillate.
//!
//! State file: `<session>/simple-english-state.json`

use std::fs;
use std::hash::Hasher;
use std::path::Path;

use fnv::FnvHasher;
use serde::{Deserialize, Serialize};

/// On-disk state persisted per session.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReplyState {
    #[serde(default)]
    pub version: u32,
    /// Hard violation count for the most recent linted reply.
    #[serde(default)]
    pub hard: usize,
    /// Soft violation count for the most recent linted reply.
    #[serde(default)]
    pub soft: usize,
    /// Identity of the most recently gated reply.
    #[serde(default)]
    pub last_identity: Option<String>,
    /// Consecutive gate count (resets on a clean reply).
    #[serde(default)]
    pub gate_count: usize,
    /// Identities of replies that have already been gated (dedup).
    #[serde(default)]
    pub gated_replies: Vec<String>,
}

impl ReplyState {
    /// True when this identity has already been gated.
    pub fn has_gated(&self, identity: &str) -> bool {
        self.gated_replies.iter().any(|g| g == identity)
    }

    /// Record that this identity was gated (deduped, capped at 16).
    pub fn record_gated(&mut self, identity: &str) {
        if !self.has_gated(identity) {
            self.gated_replies.push(identity.to_string());
        }
        if self.gated_replies.len() > 16 {
            self.gated_replies.drain(0..self.gated_replies.len() - 16);
        }
    }
}

const STATE_FILE: &str = "simple-english-state.json";

/// Maximum consecutive gates for one logical reply chain. Prevents
/// infinite loops when the model keeps producing the same bad reply.
pub const MAX_GATE_COUNT: usize = 3;

pub fn state_path(dir: &Path) -> std::path::PathBuf {
    dir.join(STATE_FILE)
}

/// Load the reply-gate state. Returns `None` when the file is absent
/// or corrupt.
pub fn load_state(dir: &Path) -> Option<ReplyState> {
    let raw = fs::read_to_string(state_path(dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Save the reply-gate state. The parent dir (session dir) must
/// already exist.
pub fn save_state(dir: &Path, state: &ReplyState) -> std::io::Result<()> {
    let raw = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(state_path(dir), raw)
}

/// Read the last `assistant_message` with non-empty `content` from
/// `<session>/events.jsonl`. Scans from the tail for efficiency.
pub fn read_last_assistant_message(dir: &Path) -> Option<String> {
    let raw = fs::read_to_string(dir.join("events.jsonl")).ok()?;
    for line in raw.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("assistant_message") {
            let content = v
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            if !content.trim().is_empty() {
                return Some(content);
            }
        }
    }
    None
}

/// Stable FNV-1a 64-bit identity for a reply text. Deterministic across
/// process runs (unlike `DefaultHasher` with random state).
pub fn reply_identity(content: &str) -> String {
    let mut hasher = FnvHasher::default();
    hasher.write(content.as_bytes());
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_last_assistant_message_scans_from_tail() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("events.jsonl"),
            r#"{"v":1,"type":"user_message","ts":"t","content":"hi"}
{"v":1,"type":"assistant_message","ts":"t","content":""}
{"v":1,"type":"assistant_message","ts":"t","content":"hello there"}
{"v":1,"type":"tool_call","ts":"t"}
"#,
        )
        .unwrap();
        assert_eq!(
            read_last_assistant_message(dir.path()).as_deref(),
            Some("hello there")
        );
    }

    #[test]
    fn read_last_assistant_message_none_when_no_content() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("events.jsonl"), "{}\n").unwrap();
        assert_eq!(read_last_assistant_message(dir.path()), None);
    }

    #[test]
    fn read_last_assistant_message_no_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_last_assistant_message(dir.path()), None);
    }

    #[test]
    fn identity_is_stable_and_distinct() {
        assert_eq!(reply_identity("abc"), reply_identity("abc"));
        assert_ne!(reply_identity("abc"), reply_identity("abd"));
        // 64-bit hex string
        assert_eq!(reply_identity("test").len(), 16);
    }

    #[test]
    fn state_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ReplyState::default();
        s.version = 1;
        s.hard = 2;
        s.soft = 1;
        s.gate_count = 1;
        s.record_gated("id-1");
        save_state(dir.path(), &s).unwrap();

        let loaded = load_state(dir.path()).unwrap();
        assert_eq!(loaded.hard, 2);
        assert_eq!(loaded.soft, 1);
        assert_eq!(loaded.gate_count, 1);
        assert!(loaded.has_gated("id-1"));
        assert!(!loaded.has_gated("id-2"));
        assert_eq!(loaded.gated_replies.len(), 1);
    }

    #[test]
    fn state_dedup_and_cap() {
        let mut s = ReplyState::default();
        s.record_gated("a");
        s.record_gated("a"); // dup
        assert_eq!(s.gated_replies.len(), 1);
        // Push enough to exceed the cap of 16
        for i in 0..20 {
            s.record_gated(&format!("id-{i}"));
        }
        assert_eq!(s.gated_replies.len(), 16);
        // Oldest entries were drained
        assert!(!s.gated_replies.contains(&"a".to_string()));
    }
}
