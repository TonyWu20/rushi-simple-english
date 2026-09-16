//! `harness-hook-simple-english` — ASD-STE100 writing-rule hook for
//! the rushi harness.
//!
//! Ported from jyooi/agent-simple-english (Claude Code plugin + pi
//! extension) to the rushi harness hook ABI.
//!
//! Registered on three harness hook windows:
//!
//! - **`tool.before`** — inspects pending `write`, `edit`, and `bash`
//!   (git commit) calls.  Hard violations block the call; soft
//!   violations are logged to stderr.  Write and edit calls use
//!   diff-aware linting: the previous file content is read from disk,
//!   and only violations that are *new* to the change are reported.
//! - **`model.before`** — injects a cache-stable rule summary into
//!   `request.prompt_fragments`, plus any pending reply-gate
//!   feedback from the previous `run.idle` evaluation.
//! - **`run.idle`** — lints the last assistant reply.  When hard
//!   violations are found, the loop is continued with a silent
//!   refire (`log_message: false` + `refire: true`, kernel issues
//!   #4 and #6): the loop runs a model turn in place and logs no
//!   user message at all.  The correction prompt is carried to that
//!   refired call by this hook's `model.before` prompt fragment, so
//!   the model revises the gated reply within the same run.  The
//!   kernel bounds the silent loop (`[run] max_silent_refires`,
//!   default 2); when refire is unsupported or disabled the pending
//!   feedback simply waits for the next model call.  The lint
//!   result is written to a state file that the TUI status widget
//!   reads.
//!
//! ## Protocol
//!
//! Input (stdin): one JSON object with at least `{"window":"<name>"}`.
//!
//! Output (stdout): exactly one JSON object.
//! - `{}` — no decision; the window default applies.
//! - `{"decision":"block","payload":{"reason":"...","calls":[...]}}`
//!   — block the listed tool calls (tool.before only).
//! - `{"decision":"transform","payload":{"request":{...}}}` — replace
//!   the model request (model.before only).
//! - `{"decision":"continue","payload":{"message":"..."}}` — inject a
//!   follow-up user message and continue the loop (run.idle only).
//! - `{"decision":"continue","payload":{"log_message":false}}` —
//!   continue the loop without logging a follow-up user message
//!   (kernel issue #4). The hook routes its own text to the model
//!   through the `model.before` transform.
//! - `{"decision":"continue","payload":{"log_message":false,
//!   "refire":true}}` — continue the loop and run a model turn in
//!   place, logging no user message at all (kernel issue #6). This
//!   hook routes the correction prompt to the refired call through
//!   the `model.before` transform. Bounded by the kernel's `[run]
//!   max_silent_refires` (default 2; 0 disables).
//!
//! Exit codes: 0 = success, 2 = blocking default for the window.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::json;

mod commit;
mod config;
mod diff;
mod dictionaries;
mod engine;
mod feedback;
mod reply;
mod rules;
mod sentences;
mod types;

use types::{LintKind, Severity};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }

    let payload = read_stdin_json();
    let window = payload
        .get("window")
        .and_then(|w| w.as_str())
        .unwrap_or("");

    match window {
        "tool.before" => handle_tool_before(&payload),
        "model.before" => handle_model_before(&payload),
        "run.idle" => handle_run_idle(&payload),
        _ => {
            println!("{}", json!({}));
        }
    }
}

// ── tool.before ─────────────────────────────────────────────────────────

/// Inspect pending tool calls. For `write`, `edit`, and `bash` (git
/// commit) calls, lint the content and block on hard violations.
///
/// Write and edit calls use diff-aware linting: the previous file
/// content is read from disk, and only violations that are *new* to
/// the change are reported. This avoids flagging pre-existing issues on
/// lines the agent did not touch.
fn handle_tool_before(payload: &serde_json::Value) {
    let calls = payload
        .get("calls")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();

    let session_dir = resolve_session_dir(payload);
    let config = config::load_config();

    let mut blocked_ids: Vec<String> = Vec::new();
    let mut reasons: Vec<String> = Vec::new();

    for call in &calls {
        let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let id = call
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or("")
            .to_string();
        let arguments = call.get("arguments").cloned().unwrap_or(json!({}));

        let report: Option<engine::LintReport> = match name {
            "write" => {
                let path = arguments
                    .get("file_path")
                    .and_then(|p| p.as_str())
                    .unwrap_or("");
                let content = arguments
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                let kind = classify_path(path);
                let prev = resolve_previous_content(path, &session_dir);
                Some(diff::lint_write(kind, content, prev.as_deref(), &config))
            }
            "edit" => {
                let path = arguments
                    .get("file_path")
                    .and_then(|p| p.as_str())
                    .unwrap_or("");
                let old_string = arguments
                    .get("old_string")
                    .and_then(|o| o.as_str())
                    .unwrap_or("");
                let new_string = arguments
                    .get("new_string")
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                let replace_all = arguments
                    .get("replace_all")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false);
                let kind = classify_path(path);
                let prev_content = resolve_previous_content(path, &session_dir);
                match prev_content {
                    None => {
                        // The file is missing: the edit tool will fail
                        // at execution. Lint the new string in
                        // isolation as the fallback.
                        Some(engine::lint(kind, new_string, &config))
                    }
                    Some(file_content) => match diff::lint_edit(
                        kind,
                        &file_content,
                        old_string,
                        new_string,
                        replace_all,
                        &config,
                    ) {
                        Ok(r) => Some(r),
                        Err(e) => {
                            eprintln!(
                                "[simple-english] edit lint skipped ({path}): {e}"
                            );
                            continue;
                        }
                    },
                }
            }
            "bash" => {
                let command = arguments
                    .get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                let invocations = commit::find_commit_invocations(command);
                if invocations.is_empty() {
                    continue; // Not a git commit; nothing to lint.
                }
                let mut any_dynamic = false;
                let mut all_violations: Vec<types::Violation> = Vec::new();
                for inv in &invocations {
                    if inv.requires_explicit_message {
                        any_dynamic = true;
                        continue;
                    }
                    let report =
                        engine::lint(LintKind::CommitMessage, &inv.message, &config);
                    all_violations.extend(report.violations);
                }
                if any_dynamic {
                    // Cannot statically lint dynamic messages; fail
                    // closed.
                    blocked_ids.push(id.clone());
                    reasons.push(
                        "Writing rules could not check the git commit message. \
                         Use git commit with a static -m or --message argument."
                            .to_string(),
                    );
                    continue;
                }
                let hard = all_violations
                    .iter()
                    .filter(|v| v.severity == Severity::Hard)
                    .count();
                let total = all_violations.len();
                Some(engine::LintReport {
                    violations: all_violations,
                    summary: engine::LintSummary { total, hard },
                })
            }
            _ => continue, // Unknown tool; skip.
        };

        let report = match report {
            Some(r) => r,
            None => continue,
        };
        let violations = report.violations;

        let hard: Vec<types::Violation> = violations
            .iter()
            .filter(|v| v.severity == Severity::Hard)
            .cloned()
            .collect();
        let soft: Vec<types::Violation> = violations
            .iter()
            .filter(|v| v.severity == Severity::Soft)
            .cloned()
            .collect();

        if hard.is_empty() {
            if !soft.is_empty() {
                let path = get_display_path(&arguments, name);
                let feedback =
                    feedback::format_violations(&path, "Writing-rule warnings for", &soft);
                eprintln!("{feedback}");
            }
            continue;
        }

        let path = get_display_path(&arguments, name);
        let summary_note = if report.summary.total > 1 {
            format!(
                " ({} total, {} hard)",
                report.summary.total, report.summary.hard
            )
        } else {
            String::new()
        };
        let reason = format!(
            "{}{}\n{}",
            "Writing rules blocked",
            summary_note,
            feedback::format_violations(&path, "", &hard)
        );
        blocked_ids.push(id);
        reasons.push(reason);
    }

    if blocked_ids.is_empty() {
        println!("{}", json!({}));
        return;
    }

    let combined_reason = reasons.join("\n\n");
    let resp = json!({
        "decision": "block",
        "payload": {
            "reason": combined_reason,
            "calls": blocked_ids,
        }
    });
    println!("{resp}");
}

// ── model.before ──────────────────────────────────────────────────────────

/// Inject the rule summary into the model request's prompt fragments.
/// Also injects the pending reply-gate feedback from the previous
/// `run.idle` evaluation, when present.
///
/// The transform is a pure function of (request, config,
/// pending-feedback). Consecutive calls with unchanged state produce
/// byte-identical output, so the prefix cache stays stable.
fn handle_model_before(payload: &serde_json::Value) {
    let request = payload.get("request").cloned().unwrap_or(json!({}));

    let config = config::load_config();
    let mut summary = feedback::rule_summary(&config);

    // Inject the pending reply-gate feedback, then clear it so it is
    // injected exactly once.
    let session_dir = resolve_session_dir(payload);
    if let Some(dir) = &session_dir {
        if let Some(state) = reply::load_state(dir) {
            if let Some(fb) = &state.pending_feedback {
                if !fb.is_empty() {
                    summary.push_str(&format!("\n\n### Pending reply feedback\n\n{fb}"));
                }
                let mut cleared = state;
                cleared.pending_feedback = None;
                let _ = reply::save_state(dir, &cleared);
            }
        }
    }

    let mut req = request;
    let mut frags: Vec<serde_json::Value> = req
        .get("prompt_fragments")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();

    // Remove any existing "simple-english" fragment (idempotent).
    frags.retain(|p| p.get(0).and_then(|i| i.as_str()) != Some("simple-english"));
    // Append our fragment.
    frags.push(json!(["simple-english", summary]));

    if frags.is_empty() {
        if let Some(obj) = req.as_object_mut() {
            obj.remove("prompt_fragments");
        }
    } else {
        req["prompt_fragments"] = json!(frags);
    }

    let resp = json!({
        "decision": "transform",
        "payload": {
            "request": req,
        }
    });
    println!("{resp}");
}

// ── run.idle ──────────────────────────────────────────────────────────────

/// Lint the last assistant reply and gate the loop when hard
/// violations are present.
///
/// - Reads the last `assistant_message` from the session's
///   `events.jsonl`.
/// - Lints the reply text as `ProseFile`.
/// - Writes the hard/soft counts to a state file for the TUI widget.
/// - Emits a silent refire `continue` (`log_message: false`,
///   `refire: true`, kernel issues #4 and #6) when hard violations
///   are found. The reply must not have been gated yet. The loop
///   runs a model turn in place and logs no user message at all; the
///   correction prompt is delivered to that refired call through
///   this hook's `model.before` prompt fragment, so the model
///   revises the reply within the same run. The kernel's per-run
///   cap (`[run] max_silent_refires`, default 2) bounds the silent
///   loop; when refire is unsupported or disabled the pending
///   feedback waits for the next model call instead.
/// - Emits `{}` otherwise (the window default is `stop`).
fn handle_run_idle(payload: &serde_json::Value) {
    let session_dir = match resolve_session_dir(payload) {
        Some(d) => d,
        None => {
            println!("{}", json!({}));
            return;
        }
    };

    let resp = run_idle_decision(&session_dir, &config::load_config());
    println!("{resp}");
}

/// Compute the `run.idle` decision for one session and persist the
/// updated state file. Pure w.r.t. stdout so it is unit-testable.
fn run_idle_decision(
    session_dir: &Path,
    config: &types::LintConfig,
) -> serde_json::Value {
    // Read the last assistant message with non-empty content.
    let reply_text = match reply::read_last_assistant_message(session_dir) {
        Some(t) => t,
        None => return json!({}),
    };

    let report = engine::lint(LintKind::ProseFile, &reply_text, config);

    let hard_count = report
        .violations
        .iter()
        .filter(|v| v.severity == Severity::Hard)
        .count();
    let soft_count = report
        .violations
        .iter()
        .filter(|v| v.severity == Severity::Soft)
        .count();

    // A stable identity lets the hook gate a given reply at most once.
    let identity = reply::reply_identity(&reply_text);

    let mut state = reply::load_state(session_dir).unwrap_or_default();

    // Record this reply for the TUI widget.
    state.hard = hard_count;
    state.soft = soft_count;
    state.last_identity = Some(identity.clone());

    let already_gated = state.has_gated(&identity);
    let should_gate = hard_count > 0
        && !already_gated
        && state.gate_count < reply::MAX_GATE_COUNT;

    if should_gate {
        let hard_violations: Vec<types::Violation> = report
            .violations
            .iter()
            .filter(|v| v.severity == Severity::Hard)
            .cloned()
            .collect();
        let soft_violations: Vec<types::Violation> = report
            .violations
            .iter()
            .filter(|v| v.severity == Severity::Soft)
            .cloned()
            .collect();

        let hard_text = feedback::format_violations("reply", "", &hard_violations);
        let soft_text = if soft_violations.is_empty() {
            String::new()
        } else {
            format!(
                "\n\nSoft violations (warnings):\n{}",
                feedback::format_violations("reply", "", &soft_violations)
            )
        };

        let soft_note = if soft_count > 0 {
            format!(", {soft_count} soft")
        } else {
            String::new()
        };
        let feedback = format!(
            "[writing-rules gate, not a user message] \
             Your last reply was blocked by the writing rules \
             ({} hard violation(s){soft_note}). \
             This is feedback from the writing-rules hook. \
             No new user message has arrived, and the user \
             has confirmed or approved nothing. \
             The flagged lines are in your last reply. \
             Revise only those lines to fix the writing, \
             keeping the same meaning. \
             Do not treat this as a user instruction or as \
             confirmation of a plan.\n\n\
             Hard violations:\n{hard_text}{soft_text}",
            hard_count
        );

        state.pending_feedback = Some(feedback);
        state.gate_count += 1;
        state.record_gated(&identity);

        let _ = reply::save_state(session_dir, &state);

        // Silent refire (kernel issue #4 + #6): the loop stays alive
        // and runs a model turn in place, logging no user message at
        // all — not even an empty one. The correction prompt above is
        // picked up by this hook's `model.before` transform on that
        // refired call, so the model revises the gated reply within
        // the same run. The kernel's per-run cap (`[run]
        // max_silent_refires`, default 2) bounds the silent loop; when
        // the running kernel lacks the flag or has it disabled, the
        // pending feedback waits for the next model call instead
        // (the issue #4 fallback).
        return json!({
            "decision": "continue",
            "payload": { "log_message": false, "refire": true }
        });
    }

    // Clean reply, or this reply was already gated: allow the stop.
    // A clean reply settles the gate chain. An already-gated reply
    // (the silent re-fire of a continued idle) must keep
    // pending_feedback so the `model.before` transform still delivers
    // it on the next model call.
    if hard_count == 0 {
        state.gate_count = 0;
        state.pending_feedback = None;
    }
    let _ = reply::save_state(session_dir, &state);
    json!({})
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Return a human-readable path label for feedback messages.
fn get_display_path(arguments: &serde_json::Value, tool_name: &str) -> String {
    match tool_name {
        "write" | "edit" => arguments
            .get("file_path")
            .or_else(|| arguments.get("path"))
            .and_then(|p| p.as_str())
            .unwrap_or("file")
            .to_string(),
        "bash" => "commit message".to_string(),
        _ => "content".to_string(),
    }
}

/// Classify a file path into a `LintKind`.
fn classify_path(path: &str) -> LintKind {
    let lower = path.to_lowercase();
    let ext = lower
        .rsplit('.')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("");

    // Skipped extensions: data files, generated artifacts.
    const SKIP: &[&str] = &[
        "css", "scss", "less", "json", "jsonc", "svg", "xml",
        "typ", "csv", "tsv", "lock",
    ];
    if SKIP.contains(&ext) {
        return LintKind::ProseFile; // treated as prose, effectively a no-op
    }

    match ext {
        "html" | "htm" => LintKind::ProseFile, // simplified: treat as prose
        "rs" | "go" | "java" | "c" | "h" | "cpp" | "hpp" | "cc"
        | "cs" | "swift" | "kt" | "scala"
        | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => LintKind::SlashSource,
        "sh" | "bash" | "zsh" | "py" | "rb" | "yaml" | "yml" | "toml" | "pl" => {
            LintKind::HashSource
        }
        _ => LintKind::ProseFile,
    }
}

/// Resolve the session directory from the payload or the environment.
///
/// The harness passes `session` (the session directory path) in the
/// `tool.before` and `run.idle` payloads. The `model.before` payload
/// also carries `session`. As a fallback, the `SESSION` and
/// `SESSIONS_ROOT` environment variables are set by the harness for
/// every hook invocation.
fn resolve_session_dir(payload: &serde_json::Value) -> Option<PathBuf> {
    // 1. The `session` field from the payload.
    if let Some(s) = payload.get("session").and_then(|s| s.as_str()) {
        let p = PathBuf::from(s);
        if p.is_absolute() {
            if p.is_dir() {
                return Some(p);
            }
        } else if let Ok(cwd) = std::env::current_dir() {
            let resolved = cwd.join(&p);
            if resolved.is_dir() {
                return Some(resolved);
            }
        }
    }

    // 2. The SESSION + SESSIONS_ROOT environment variables.
    if let (Ok(session_name), Ok(root)) =
        (std::env::var("SESSION"), std::env::var("SESSIONS_ROOT"))
    {
        let p = PathBuf::from(&root).join(&session_name);
        if p.is_dir() {
            return Some(p);
        }
    }

    None
}

/// Read the previous content of a file for diff-aware linting.
///
/// Relative paths resolve against the session's `cwd` file. Returns
/// `None` when the file does not exist (new file) or cannot be read.
fn resolve_previous_content(path: &str, session_dir: &Option<PathBuf>) -> Option<String> {
    let abs_path = resolve_file_path(path, session_dir);
    std::fs::read_to_string(&abs_path).ok()
}

/// Resolve a possibly-relative file path against the session's project
/// working directory (the `cwd` file inside the session dir).
fn resolve_file_path(path: &str, session_dir: &Option<PathBuf>) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        return p;
    }
    // Try the session's cwd file first.
    if let Some(dir) = session_dir {
        let cwd_file = dir.join("cwd");
        if let Ok(cwd_str) = std::fs::read_to_string(&cwd_file) {
            let project_cwd = PathBuf::from(cwd_str.trim());
            return project_cwd.join(path);
        }
    }
    // Fall back to the process cwd.
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(path)
}

fn read_stdin_json() -> serde_json::Value {
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() || buf.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(&buf).unwrap_or(json!({}))
}

fn print_help() {
    println!("harness-hook-simple-english — ASD-STE100 writing-rule hook");
    println!();
    println!("Windows:");
    println!(
        "  tool.before  — gates write / edit / bash (git commit) calls"
    );
    println!("  model.before — injects the writing-rule summary into the prompt");
    println!("  run.idle     — lints the last assistant reply, gates on hard violations");
    println!();
    println!("Input (stdin): {{\"window\":\"<name>\", ...}}");
    println!("Output (stdout):");
    println!("  {{}}  — no decision, window default applies");
    println!(
        "  {{\"decision\":\"block\",\"payload\":{{\"reason\":...,\"calls\":[...]}}}} — block listed calls"
    );
    println!(
        "  {{\"decision\":\"transform\",\"payload\":{{\"request\":{{...}}}}}} — replace model request"
    );
    println!(
        "  {{\"decision\":\"continue\",\"payload\":{{\"message\":\"...\"}}}} — inject follow-up, continue loop"
    );
    println!(
        "  {{\"decision\":\"continue\",\"payload\":{{\"log_message\":false}}}} — continue loop, log no follow user message"
    );
    println!(
        "  {{\"decision\":\"continue\",\"payload\":{{\"log_message\":false,\"refire\":true}}}} — silent refire: run a model turn in place, log no user message (kernel #4 + #6)"
    );
    println!("Exit codes: 0 = ok, 2 = blocking default");
    println!();
    println!("Config: .simple-english.json in cwd or $SIMPLE_ENGLISH_CONFIG");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_md_is_prose() {
        assert_eq!(classify_path("README.md"), LintKind::ProseFile);
    }

    #[test]
    fn classify_rs_is_slash() {
        assert_eq!(classify_path("src/main.rs"), LintKind::SlashSource);
    }

    #[test]
    fn classify_py_is_hash() {
        assert_eq!(classify_path("run.py"), LintKind::HashSource);
    }

    #[test]
    fn classify_json_is_prose_skipped() {
        assert_eq!(classify_path("config.json"), LintKind::ProseFile);
    }

    #[test]
    fn classify_no_extension() {
        assert_eq!(classify_path("Makefile"), LintKind::ProseFile);
    }

    #[test]
    fn display_path_prefers_file_path() {
        let args = json!({"file_path": "docs/a.md", "path": "legacy.md"});
        assert_eq!(get_display_path(&args, "write"), "docs/a.md");
    }

    #[test]
    fn display_path_falls_back_to_path() {
        let args = json!({"path": "docs/a.md"});
        assert_eq!(get_display_path(&args, "edit"), "docs/a.md");
    }

    /// Seed a session dir with one user turn and one assistant reply.
    fn seed_session(dir: &Path, reply: &str) {
        let user = r#"{"v":1,"type":"user_message","ts":"t","content":"do the task"}"#;
        let asst = format!(
            "{{\"v\":1,\"type\":\"assistant_message\",\"ts\":\"t\",\"content\":{}}}",
            json!(reply)
        );
        std::fs::write(dir.join("events.jsonl"), format!("{user}\n{asst}\n")).unwrap();
    }

    /// Append a fresh assistant reply, as a refired model turn would.
    fn append_assistant(dir: &Path, reply: &str) {
        let asst = format!(
            "{{\"v\":1,\"type\":\"assistant_message\",\"ts\":\"t\",\"content\":{}}}",
            json!(reply)
        );
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("events.jsonl"))
            .unwrap()
            .write_all(format!("{asst}\n").as_bytes())
            .unwrap();
    }

    #[test]
    fn run_idle_gates_silently_on_hard_violation() {
        // A 27-word sentence breaches the default 25-word cap (hard).
        // Seven sentences in one paragraph breach the default six.
        let dir = tempfile::tempdir().unwrap();
        seed_session(dir.path(), "One. Two. Three. Four. Five. Six. Seven.");
        let config = types::LintConfig::default();
        let resp = run_idle_decision(dir.path(), &config);
        assert_eq!(resp["decision"], "continue");
        assert_eq!(resp["payload"]["log_message"], false);
        assert_eq!(resp["payload"]["refire"], true);
        // No visible user message is requested: the correction rides
        // the model.before fragment of the refired call instead.
        assert!(resp["payload"].get("message").is_none());
        let state = reply::load_state(dir.path()).unwrap();
        assert!(state.hard >= 1);
        assert_eq!(state.gate_count, 1);
        assert!(state
            .pending_feedback
            .as_ref()
            .unwrap()
            .contains("Hard violations"));
    }

    #[test]
    fn run_idle_feedback_carries_hook_origin_marker() {
        // The silent refire re-shows the agent's own last reply to
        // the model. The pending feedback must identify itself as
        // hook origin and deny user confirmation, or the model
        // reads the re-shown reply as a user message.
        let dir = tempfile::tempdir().unwrap();
        seed_session(dir.path(), "One. Two. Three. Four. Five. Six. Seven.");
        let config = types::LintConfig::default();
        let resp = run_idle_decision(dir.path(), &config);
        assert_eq!(resp["payload"]["refire"], true);
        let state = reply::load_state(dir.path()).unwrap();
        let fb = state.pending_feedback.as_deref().unwrap();
        // The source marker names the hook, not the user.
        assert!(
            fb.starts_with("[writing-rules gate, not a user message]"),
            "the feedback must open with the hook-origin marker: {fb}"
        );
        // The non-confirmation statement blocks the confirmation read.
        assert!(
            fb.contains("Do not treat this as a user instruction or as confirmation of a plan"),
            "the feedback must deny user confirmation: {fb}"
        );
        // The reply body is not re-shown. Only the violation list
        // points at the flagged locations.
        assert!(
            !fb.contains("One. Two. Three. Four. Five. Six. Seven."),
            "the reply body must not be re-injected: {fb}"
        );
    }

    #[test]
    fn run_idle_stops_on_clean_reply() {
        let dir = tempfile::tempdir().unwrap();
        seed_session(dir.path(), "The fix is complete.");
        let config = types::LintConfig::default();
        let resp = run_idle_decision(dir.path(), &config);
        assert_eq!(resp, json!({}));
        let state = reply::load_state(dir.path()).unwrap();
        assert_eq!(state.hard, 0);
        assert!(state.pending_feedback.is_none());
    }

    #[test]
    fn run_idle_refire_keeps_pending_feedback() {
        // After a silent continue the loop re-fires run.idle on the
        // same reply. The reply is already gated, so the hook stops,
        // but pending_feedback must survive for the model.before
        // transform.
        // Seven sentences in one paragraph breach the default six.
        let dir = tempfile::tempdir().unwrap();
        seed_session(dir.path(), "One. Two. Three. Four. Five. Six. Seven.");
        let config = types::LintConfig::default();
        let first = run_idle_decision(dir.path(), &config);
        assert_eq!(first["decision"], "continue");
        let second = run_idle_decision(dir.path(), &config);
        assert_eq!(second, json!({}));
        let state = reply::load_state(dir.path()).unwrap();
        assert!(
            state.pending_feedback.is_some(),
            "the refire must not clear pending_feedback"
        );
        assert_eq!(state.gate_count, 1, "the refire must not re-gate");
    }

    #[test]
    fn run_idle_gate_chain_on_new_replies() {
        // The refire chain: each revised reply is a new identity, so
        // the gate keeps requesting refires, until MAX_GATE_COUNT
        // consecutive gates are reached and the hook lets the loop
        // stop (the kernel's own refire cap usually fires earlier).
        let dir = tempfile::tempdir().unwrap();
        seed_session(dir.path(), "One. Two. Three. Four. Five. Six. Seven.");
        let config = types::LintConfig::default();
        let mut decisions = Vec::new();
        let mut n = 1;
        for _ in 0..5 {
            let d = run_idle_decision(dir.path(), &config);
            decisions.push(d.clone());
            if d["decision"].as_str() == Some("continue") {
                // The refired model turn produces a new reply that
                // still breaches the paragraph cap.
                n += 1;
                append_assistant(
                    dir.path(),
                    &format!("Revised {n}. One. Two. Three. Four. Five. Six. Seven."),
                );
            }
        }
        // Three refires, then stop at the MAX_GATE_COUNT limit.
        assert_eq!(decisions.len(), 5);
        for d in &decisions[..3] {
            assert_eq!(d["decision"], "continue");
            assert_eq!(d["payload"]["refire"], true);
        }
        assert_eq!(decisions[3], json!({}));
        assert_eq!(decisions[4], json!({}));
        let state = reply::load_state(dir.path()).unwrap();
        assert_eq!(state.gate_count, 3);
        assert!(state.pending_feedback.is_some(),
            "the stopped chain keeps its feedback for the next model call");
    }
}
