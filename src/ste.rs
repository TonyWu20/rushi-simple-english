//! `rushi-ste` — standalone ASD-STE100 writing-rule checker.
//!
//! Checks one file, or stdin when no file is given. The violation
//! report is the tool's output and goes to stdout. Operational
//! errors and notes go to stderr. The pass/fail verdict is the exit
//! code, so a shell or CI job can branch on it.
//!
//! Usage:
//!   rushi-ste something.md
//!   cat something.md | rushi-ste
//!
//! Exit codes:
//!   0  clean, or only soft violations with `--hard-only`
//!   1  violations found
//!   2  usage or input error

use std::io::Read;
use std::process::ExitCode;

use serde_json::json;

use simple_english::classify;
use simple_english::config;
use simple_english::engine;
use simple_english::types::LintKind;

const HELP: &str = "rushi-ste — standalone ASD-STE100 writing-rule checker

Usage:
  rushi-ste [OPTIONS] [FILE]

With no FILE, or with FILE as '-', read stdin.

Options:
  -h, --help         Show this help and exit
  -V, --version      Show the version and exit
      --json         Emit the report as a JSON object
      --hard-only    Exit 1 only when a hard violation is found
      --kind <KIND>  Lint kind: prose-file | slash-source | hash-source
                     | commit-message. Default: inferred from the file
                     extension; stdin is prose-file.

The report goes to stdout. Errors and notes go to stderr.
Exit codes: 0 = clean, 1 = violations found, 2 = usage or input error.

Config: .simple-english.toml in the cwd or $SIMPLE_ENGLISH_CONFIG.";

/// Parsed CLI arguments.
#[derive(Debug)]
struct Cli {
    file: Option<String>,
    json: bool,
    hard_only: bool,
    kind: Option<LintKind>,
}

fn parse_kind(value: &str) -> Option<LintKind> {
    match value {
        "prose-file" => Some(LintKind::ProseFile),
        "slash-source" => Some(LintKind::SlashSource),
        "hash-source" => Some(LintKind::HashSource),
        "commit-message" => Some(LintKind::CommitMessage),
        _ => None,
    }
}

fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut file: Option<String> = None;
    let mut json = false;
    let mut hard_only = false;
    let mut kind: Option<LintKind> = None;

    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if let Some(value) = a.strip_prefix("--kind=") {
            kind = Some(parse_kind(value).ok_or_else(|| {
                format!("unknown kind \"{value}\". Expected prose-file, slash-source, hash-source, or commit-message")
            })?);
        } else {
            match a {
                "--json" => json = true,
                "--hard-only" => hard_only = true,
                "--kind" => {
                    i += 1;
                    let value = args
                        .get(i)
                        .ok_or_else(|| "--kind needs a value".to_string())?
                        .as_str();
                    kind = Some(parse_kind(value).ok_or_else(|| {
                        format!("unknown kind \"{value}\". Expected prose-file, slash-source, hash-source, or commit-message")
                    })?);
                }
                s if s.starts_with('-') && s != "-" => {
                    return Err(format!("unknown option \"{s}\". Try --help."));
                }
                _ => {
                    if file.is_some() {
                        return Err(format!("unexpected extra argument \"{a}\". Give one file."));
                    }
                    file = Some(a.to_string());
                }
            }
        }
        i += 1;
    }

    Ok(Cli {
        file,
        json,
        hard_only,
        kind,
    })
}

fn kind_name(kind: LintKind) -> &'static str {
    match kind {
        LintKind::ProseFile => "prose-file",
        LintKind::SlashSource => "slash-source",
        LintKind::HashSource => "hash-source",
        LintKind::CommitMessage => "commit-message",
        LintKind::Skip => "skip",
    }
}

/// One human-readable violation line.
fn format_violation_line(v: &simple_english::types::Violation) -> String {
    let sugg = v
        .suggestions
        .as_ref()
        .and_then(|s| s.first())
        .map(|s| format!(" Suggested fix: use \"{s}\"."))
        .unwrap_or_default();
    format!(
        "- line {}, column {} [{}] [{}] {}{}",
        v.line, v.column, v.severity, v.rule_id, v.message, sugg
    )
}

fn render_json(label: &str, kind: LintKind, report: &engine::LintReport) -> serde_json::Value {
    json!({
        "file": label,
        "kind": kind_name(kind),
        "total": report.summary.total,
        "hard": report.summary.hard,
        "soft": report.summary.total - report.summary.hard,
        "violations": report.violations.iter().map(|v| {
            json!({
                "line": v.line,
                "column": v.column,
                "rule": v.rule_id,
                "severity": v.severity.as_str(),
                "message": v.message,
                "suggestions": v.suggestions,
            })
        }).collect::<Vec<_>>(),
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("rushi-ste {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let cli = match parse_args(&args) {
        Ok(cli) => cli,
        Err(msg) => {
            eprintln!("rushi-ste: {msg}");
            return ExitCode::from(2);
        }
    };

    // Read the input: a file, or stdin (no file, or "-").
    let (label, text) = match &cli.file {
        Some(path) if path != "-" => match std::fs::read_to_string(path) {
            Ok(text) => (path.clone(), text),
            Err(e) => {
                eprintln!("rushi-ste: cannot read {path}: {e}");
                return ExitCode::from(2);
            }
        },
        _ => {
            let mut buf = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                eprintln!("rushi-ste: cannot read stdin: {e}");
                return ExitCode::from(2);
            }
            ("-".to_string(), buf)
        }
    };

    // Resolve the lint kind: an explicit flag wins, then the file
    // extension, then prose for stdin.
    let config = config::load_config();
    let kind = cli.kind.unwrap_or_else(|| {
        if label == "-" {
            LintKind::ProseFile
        } else {
            classify::classify_path(&label)
        }
    });

    let report = engine::lint(kind, &text, &config);

    let total = report.summary.total;
    let hard = report.summary.hard;
    let soft = total - hard;

    if cli.json {
        println!("{}", render_json(&label, kind, &report));
    } else if total == 0 {
        println!("{label}: clean");
    } else {
        println!("{label}: {total} violation(s), {hard} hard, {soft} soft");
        for v in &report.violations {
            println!("{}", format_violation_line(v));
        }
    }

    if matches!(kind, LintKind::Skip) {
        eprintln!("note: {label} has no lintable prose (kind: skip).");
    }

    let failed = if cli.hard_only { hard > 0 } else { total > 0 };
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, String> {
        parse_args(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn no_args_means_stdin() {
        let cli = parse(&[]).unwrap();
        assert!(cli.file.is_none());
        assert!(!cli.json);
        assert!(!cli.hard_only);
        assert!(cli.kind.is_none());
    }

    #[test]
    fn file_positional_arg() {
        let cli = parse(&["something.md"]).unwrap();
        assert_eq!(cli.file.as_deref(), Some("something.md"));
    }

    #[test]
    fn dash_means_stdin() {
        let cli = parse(&["-"]).unwrap();
        assert_eq!(cli.file.as_deref(), Some("-"));
    }

    #[test]
    fn flag_forms() {
        let cli = parse(&["--json", "--hard-only", "--kind", "hash-source", "f.sh"]).unwrap();
        assert!(cli.json);
        assert!(cli.hard_only);
        assert_eq!(cli.kind, Some(LintKind::HashSource));
        assert_eq!(cli.file.as_deref(), Some("f.sh"));
    }

    #[test]
    fn kind_equals_form() {
        let cli = parse(&["--kind=commit-message"]).unwrap();
        assert_eq!(cli.kind, Some(LintKind::CommitMessage));
    }

    #[test]
    fn unknown_kind_rejected() {
        let err = parse(&["--kind", "bogus"]).unwrap_err();
        assert!(err.contains("unknown kind"));
    }

    #[test]
    fn unknown_flag_rejected() {
        let err = parse(&["--wat"]).unwrap_err();
        assert!(err.contains("unknown option"));
    }

    #[test]
    fn two_files_rejected() {
        let err = parse(&["a.md", "b.md"]).unwrap_err();
        assert!(err.contains("unexpected extra argument"));
    }

    #[test]
    fn kind_names_round_trip() {
        assert_eq!(kind_name(LintKind::ProseFile), "prose-file");
        assert_eq!(kind_name(LintKind::Skip), "skip");
    }

    #[test]
    fn json_report_shape() {
        let v = simple_english::types::Violation {
            rule_id: "contraction",
            severity: simple_english::types::Severity::Hard,
            message: "Contraction \"don't\".".into(),
            suggestions: Some(vec!["do not".to_string()]),
            line: 2,
            column: 5,
        };
        let report = engine::LintReport {
            violations: vec![v],
            summary: engine::LintSummary { total: 1, hard: 1 },
        };
        let value = render_json("-", LintKind::ProseFile, &report);
        assert_eq!(value["file"], "-");
        assert_eq!(value["total"], 1);
        assert_eq!(value["hard"], 1);
        assert_eq!(value["soft"], 0);
        assert_eq!(value["violations"][0]["rule"], "contraction");
        assert_eq!(value["violations"][0]["suggestions"][0], "do not");
    }
}
