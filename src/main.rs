//! `harness-hook-simple-english` — ASD-STE100 writing-rule hook for
//! the rushi harness.
//!
//! Ported from jyooi/agent-simple-english (Claude Code plugin + pi
//! extension).  Enforces Simplified Technical English rules on agent
//! prose, edits, and git commit messages.
//!
//! Registered on two harness hook windows:
//!
//! - **`tool.before`** — inspects pending `write`, `edit`, and
//!   `bash` (git commit) calls.  Hard violations block the call;
//!   soft violations are logged to stderr.
//! - **`model.before`** — injects a cache-stable rule summary into
//!   `request.prompt_fragments` so the model knows the active rules.
//!
//! ## Protocol
//!
//! Input (stdin): one JSON object with at least `{"window":"<name>"}`.
//!
//! Output (stdout): exactly one JSON object.
//! - `{}` — no decision, window default applies.
//! - `{"decision":"block","payload":{"reason":"…","calls":[…]}}` —
//!   block the listed tool calls (tool.before only).
//! - `{"decision":"transform","payload":{"request":{…}}}` — replace
//!   the model request (model.before only).
//!
//! Exit codes: 0 = success, 2 = blocking default for the window.

use std::io::Read;
use std::process::exit;

use serde_json::json;

mod commit;
mod config;
mod dictionaries;
mod engine;
mod feedback;
mod rules;
mod sentences;
mod types;

use types::{LintKind, Severity, Violation};

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
        _ => {
            // Not a window we handle: no-op.
            println!("{}", json!({}));
        }
    }
}

// ── tool.before ─────────────────────────────────────────────────────────────

/// Inspect pending tool calls. For each `write`, `edit`, or `bash`
/// (git commit) call, lint the content and block on hard violations.
fn handle_tool_before(payload: &serde_json::Value) {
    let calls = payload
        .get("calls")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();

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

        let lint_result: Option<engine::LintReport> = match name {
            "write" => {
                let path = arguments.get("path").and_then(|p| p.as_str()).unwrap_or("");
                let content = arguments.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let kind = classify_path(path);
                let report = engine::lint(kind, content, &config);
                Some(report)
            }
            "edit" => {
                let path = arguments.get("path").and_then(|p| p.as_str()).unwrap_or("");
                let kind = classify_path(path);
                let edits = arguments.get("edits").and_then(|e| e.as_array()).cloned()
                    .unwrap_or_default();
                let mut all_violations: Vec<Violation> = Vec::new();
                for edit in &edits {
                    let new_text = edit.get("newText").and_then(|t| t.as_str()).unwrap_or("");
                    let report = engine::lint(kind, new_text, &config);
                    all_violations.extend(report.violations);
                }
                let total = all_violations.len();
                let hard = all_violations.iter().filter(|v| v.severity == Severity::Hard).count();
                Some(engine::LintReport {
                    violations: all_violations,
                    summary: engine::LintSummary { total, hard },
                })
            }
            "bash" => {
                let command = arguments.get("command").and_then(|c| c.as_str()).unwrap_or("");
                let invocations = commit::find_commit_invocations(command);
                if invocations.is_empty() {
                    continue; // Not a git commit; nothing to lint.
                }
                let mut any_dynamic = false;
                let mut all_violations: Vec<Violation> = Vec::new();
                for inv in &invocations {
                    if inv.requires_explicit_message {
                        any_dynamic = true;
                        continue;
                    }
                    let report = engine::lint(LintKind::CommitMessage, &inv.message, &config);
                    all_violations.extend(report.violations);
                }
                if any_dynamic {
                    // Cannot statically lint dynamic messages; fail closed.
                    blocked_ids.push(id.clone());
                    reasons.push(
                        "Writing rules could not check the git commit message. \
                         Use git commit with a static -m or --message argument."
                            .to_string(),
                    );
                    continue;
                }
                let total = all_violations.len();
                let hard = all_violations.iter().filter(|v| v.severity == Severity::Hard).count();
                Some(engine::LintReport {
                    violations: all_violations,
                    summary: engine::LintSummary { total, hard },
                })
            }
            _ => continue, // Unknown tool; skip.
        };

        let report = match lint_result {
            Some(r) => r,
            None => continue,
        };
        let violations = report.violations;

        let hard: Vec<Violation> = violations
            .iter()
            .filter(|v| v.severity == Severity::Hard)
            .cloned()
            .collect();
        let soft: Vec<Violation> = violations
            .iter()
            .filter(|v| v.severity == Severity::Soft)
            .cloned()
            .collect();

        if hard.is_empty() {
            if !soft.is_empty() {
                let path = get_display_path(&arguments, name);
                let feedback = feedback::format_violations(&path, "Writing-rule warnings for", &soft);
                eprintln!("{feedback}");
            }
            continue;
        }

        let path = get_display_path(&arguments, name);
        let summary_note = if report.summary.total > 1 {
            format!(" ({} total, {} hard)", report.summary.total, report.summary.hard)
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

/// Determine a human-readable path label for feedback messages.
fn get_display_path(arguments: &serde_json::Value, tool_name: &str) -> String {
    match tool_name {
        "write" | "edit" => arguments
            .get("path")
            .and_then(|p| p.as_str())
            .unwrap_or("file")
            .to_string(),
        "bash" => "commit message".to_string(),
        _ => "content".to_string(),
    }
}

// ── model.before ────────────────────────────────────────────────────────────

/// Inject the rule summary into the model request's prompt fragments.
///
/// This is idempotent: the transform is a pure function of (request,
/// config), so consecutive calls with unchanged state produce
/// byte-identical output (prefix-cache stable).
fn handle_model_before(payload: &serde_json::Value) {
    let request = payload
        .get("request")
        .cloned()
        .unwrap_or(json!({}));

    let config = config::load_config();
    let summary = feedback::rule_summary(&config);

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

// ── Helpers ─────────────────────────────────────────────────────────────────

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
        return LintKind::ProseFile; // treated as prose but effectively no-op
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
    println!("  tool.before  — gates write / edit / bash (git commit) calls");
    println!("  model.before — injects the writing-rule summary into the prompt");
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
    println!("Exit codes: 0 = ok, 2 = blocking default");
    println!();
    println!("Config: .simple-english.json in cwd or $SIMPLE_ENGLISH_CONFIG");
}

#[allow(dead_code)]
fn _unused_exit() {
    exit(0);
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
}
