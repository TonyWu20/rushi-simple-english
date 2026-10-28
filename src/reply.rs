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

/// How many text lines the indexed tail read decodes (the P8
/// consumer path). The window must reach back past the last
/// `assistant_message` with content. Lines after it are bounded:
/// the idle markers land after the pipeline, and a gate chain adds
/// at most `MAX_GATE_COUNT` user-plus-assistant pairs. A miss is
/// safe: it escalates to the full read.
const TAIL_LINES: usize = 128;

/// The stream-byte budget of the no-index window scan (a plain
/// legacy log, or a pre-P8 zstd stream). A reply longer than the
/// budget truncates to a non-parsing line, the scan misses, and
/// the full read decides.
const TAIL_BYTE_BUDGET: u64 = 1_048_576;

/// Read the last `assistant_message` with non-empty `content` from
/// `<session>/events.jsonl`. The read goes through the kernel's
/// shared reader (kernel `docs/session-log-compression.md`, the
/// consumer contract). The gate only wants one event at the tail,
/// so it pays the cheapest tier that can hold it:
///
/// 1. the indexed tail read (`tail_lines`): with the frame index
///    sidecar, decodes only the record ranges at the tail;
/// 2. the no-index window scan (`read_log_tail`): the last budget
///    bytes, for plain legacy logs and pre-P8 streams;
/// 3. the full read (`read_log_text`): the reply sits past the
///    window, or the tail read degraded. Its error fails the step
///    (FT-005): the gate never runs blind.
///
/// `Ok(None)` when the log is absent (a fresh session, no reply
/// yet) or holds no assistant message with content.
pub fn read_last_assistant_message(dir: &Path) -> Result<Option<String>, std::io::Error> {
    let path = dir.join("events.jsonl");
    let stream = match rushi_common::logstream::open_indexed(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };

    let window = if stream.has_index() {
        stream.tail_lines(TAIL_LINES).join("\n")
    } else {
        // No index (a plain legacy log, or a pre-P8 stream): the
        // no-index window scan. The coverage flag is not needed: a
        // miss or a degraded read escalates to the full read.
        rushi_common::logstream::read_log_tail(&path, TAIL_BYTE_BUDGET)?.0
    };
    if let Some(reply) = last_assistant_in_text(&window) {
        return Ok(Some(reply));
    }

    let full = rushi_common::logstream::read_log_text(&path)?;
    Ok(last_assistant_in_text(&full))
}

/// The last `assistant_message` with non-empty `content` in a
/// decoded log text. Lines are JSONL events. A torn first line of
/// a window is not valid JSON, and it skips.
fn last_assistant_in_text(text: &str) -> Option<String> {
    for line in text.lines().rev() {
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
            read_last_assistant_message(dir.path()).unwrap().as_deref(),
            Some("hello there")
        );
    }

    #[test]
    fn read_last_assistant_message_none_when_no_content() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("events.jsonl"), "{}\n").unwrap();
        assert_eq!(read_last_assistant_message(dir.path()).unwrap(), None);
    }

    #[test]
    fn read_last_assistant_message_no_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_last_assistant_message(dir.path()).unwrap(),
            None,
            "an absent log means no reply yet, not an error"
        );
    }

    /// One zstd frame per kernel log line: `LogLine::commit`
    /// compresses the line bytes (newline-terminated) into one
    /// frame and appends it.
    fn zstd_stream(dir: &Path, lines: &[&str]) {
        let frames: Vec<u8> = lines
            .iter()
            .map(|l| rushi_common::logstream::compress_frame(l.as_bytes()).unwrap())
            .collect::<Vec<Vec<u8>>>()
            .into_iter()
            .flatten()
            .collect();
        fs::write(dir.join("events.jsonl"), frames).unwrap();
    }

    /// One frame-index sidecar record per frame.
    fn write_index(dir: &Path, lines: &[&str]) {
        use rushi_common::logstream::{IndexRecord, encode_index, index_path};
        let mut records = Vec::new();
        let mut offset = 0u64;
        for (pos, line) in lines.iter().enumerate() {
            let frame = rushi_common::logstream::compress_frame(line.as_bytes()).unwrap();
            records.push(IndexRecord {
                seq_start: pos as u64 + 1,
                frame_offset: offset,
                frame_len: frame.len() as u64,
                text_len: line.len() as u64,
            });
            offset += frame.len() as u64;
        }
        fs::write(
            index_path(&dir.join("events.jsonl")),
            encode_index(&records),
        )
        .unwrap();
    }

    /// Newline-terminated event lines, as the kernel commits them.
    fn event_lines(user: &str, asst: &str) -> Vec<String> {
        vec![user.to_string() + "\n", asst.to_string() + "\n"]
    }

    #[test]
    fn read_last_assistant_message_reads_zstd_stream() {
        // A zstd frame stream without the index sidecar takes the
        // no-index window scan. Plain legacy logs take the same
        // path, so the pre-zstd behavior is unchanged.
        let dir = tempfile::tempdir().unwrap();
        let user = r#"{"v":1,"type":"user_message","ts":"t","seq":1,"content":"do the task"}"#;
        let asst =
            r#"{"v":1,"type":"assistant_message","ts":"t","seq":2,"content":"You're done."}"#;
        let lines = event_lines(user, asst);
        zstd_stream(
            dir.path(),
            &lines.iter().map(|l| l.as_str()).collect::<Vec<&str>>(),
        );
        assert_eq!(
            read_last_assistant_message(dir.path()).unwrap().as_deref(),
            Some("You're done.")
        );
    }

    #[test]
    fn read_last_assistant_message_reads_indexed_tail() {
        // The P8 consumer path: with the frame index sidecar, the
        // read decodes only the record ranges at the tail.
        let dir = tempfile::tempdir().unwrap();
        let user = r#"{"v":1,"type":"user_message","ts":"t","seq":1,"content":"do the task"}"#;
        let asst =
            r#"{"v":1,"type":"assistant_message","ts":"t","seq":2,"content":"You're done."}"#;
        let lines = event_lines(user, asst);
        let refs: Vec<&str> = lines.iter().map(|l| l.as_str()).collect();
        zstd_stream(dir.path(), &refs);
        write_index(dir.path(), &refs);
        assert_eq!(
            read_last_assistant_message(dir.path()).unwrap().as_deref(),
            Some("You're done.")
        );
    }

    #[test]
    fn read_last_assistant_message_escapes_past_the_tail_window() {
        // The reply sits past the indexed tail window (more than
        // `TAIL_LINES` lines back). The tail read misses. The
        // full read decides, and finds the reply.
        let dir = tempfile::tempdir().unwrap();
        let user = r#"{"v":1,"type":"user_message","ts":"t","seq":1,"content":"do the task"}"#;
        let asst =
            r#"{"v":1,"type":"assistant_message","ts":"t","seq":2,"content":"You're done."}"#;
        let mut lines = event_lines(user, asst);
        for i in 0..(super::TAIL_LINES + 32) {
            lines.push(format!(
                "{{\"id\":\"ext{0}\",\"ts\":\"t\",\"type\":\"ext_status\",\"v\":1,\"value\":\"x\"}}\n",
                i
            ));
        }
        let refs: Vec<&str> = lines.iter().map(|l| l.as_str()).collect();
        zstd_stream(dir.path(), &refs);
        write_index(dir.path(), &refs);
        assert_eq!(
            read_last_assistant_message(dir.path()).unwrap().as_deref(),
            Some("You're done.")
        );
    }

    #[test]
    fn read_last_assistant_message_grown_past_its_index() {
        // A stream that grew past its index (a commit after the
        // last index write) drops the indexed tail. The read falls
        // back and still finds the reply.
        let dir = tempfile::tempdir().unwrap();
        let user = r#"{"v":1,"type":"user_message","ts":"t","seq":1,"content":"do the task"}"#;
        let lines = event_lines(user, "");
        let refs: Vec<&str> = lines.iter().map(|l| l.as_str()).collect();
        zstd_stream(dir.path(), &refs);
        write_index(dir.path(), &refs);
        // A fresh model turn: the reply frame lands after the
        // index end.
        let asst =
            r#"{"v":1,"type":"assistant_message","ts":"t","seq":2,"content":"You're done."}"#;
        let frame =
            rushi_common::logstream::compress_frame((asst.to_string() + "\n").as_bytes()).unwrap();
        use std::io::Write;
        fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join("events.jsonl"))
            .unwrap()
            .write_all(&frame)
            .unwrap();
        assert_eq!(
            read_last_assistant_message(dir.path()).unwrap().as_deref(),
            Some("You're done.")
        );
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
        let mut s = ReplyState {
            version: 1,
            hard: 2,
            soft: 1,
            gate_count: 1,
            ..Default::default()
        };
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
