//! Git commit-message extraction from bash commands.
//!
//! Mirrors the logic in `src/adapter/commit-message.ts` from the original
//! project: detects `git commit` invocations, extracts the `-m` /
//! `--message` argument, and flags dynamic messages (containing `$` or
//! backticks) that cannot be statically checked.

/// A single extracted commit-message fragment.
#[derive(Debug, Clone)]
pub struct CommitMessage {
    /// The static message text.
    pub message: String,
    /// `true` when the message contains shell interpolation (`$VAR` or
    /// backticks) that cannot be statically linted.
    pub requires_explicit_message: bool,
}

/// Scan a bash command for `git commit` invocations and extract their
/// messages.
///
/// Returns an empty slice when no `git commit` is present (the caller
/// should then skip linting).
pub fn find_commit_invocations(command: &str) -> Vec<CommitMessage> {
    let cmd_lower = command.to_lowercase();
    if !cmd_lower.contains("git commit") {
        return Vec::new();
    }

    let mut out: Vec<CommitMessage> = Vec::new();

    // Simple state-machine scan for `git commit` followed by optional
    // flags, then a `-m <msg>` or `--message=<msg>` argument.
    let tokens = tokenize_shell(command);

    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];

        // Look for "git" followed by "commit"
        if token == "git" && i + 1 < tokens.len() && tokens[i + 1] == "commit" {
            i += 2;

            // Scan forward through commit flags until we hit a `-m` /
            // `--message` argument.
            while i < tokens.len() {
                let flag = &tokens[i];

                if flag == "-m" || flag == "--message" {
                    i += 1;
                    if i < tokens.len() {
                        let msg = tokens[i].clone();
                        let dynamic = msg.contains('$') || msg.contains('`');
                        out.push(CommitMessage {
                            message: msg,
                            requires_explicit_message: dynamic,
                        });
                        break;
                    }
                } else if let Some(rest) = flag.strip_prefix("--message=") {
                    let msg = rest.to_string();
                    let dynamic = msg.contains('$') || msg.contains('`');
                    out.push(CommitMessage {
                        message: msg,
                        requires_explicit_message: dynamic,
                    });
                    break;
                } else if flag.starts_with('-') && flag != "-" {
                    // Other flag (e.g. --amend, --all); skip it.
                    i += 1;
                } else {
                    // Hit a non-flag argument; stop scanning this commit.
                    break;
                }
            }
        }

        i += 1;
    }

    out
}

/// Very small shell-tokeniser: splits on whitespace, preserves quoted
/// strings as single tokens, does not expand variables.
fn tokenize_shell(cmd: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;

    for c in cmd.chars() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            ch if !in_single && !in_double => match ch {
                ' ' | '\t' | ';' | '&' => {
                    if !cur.is_empty() {
                        tokens.push(std::mem::take(&mut cur));
                    }
                }
                _ => cur.push(ch),
            },
            _ => {
                cur.push(c);
            }
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_git_commit() {
        let msgs = find_commit_invocations("ls -la");
        assert!(msgs.is_empty());
    }

    #[test]
    fn simple_commit() {
        let msgs = find_commit_invocations("git commit -m 'fix: bug'");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].message, "fix: bug");
        assert!(!msgs[0].requires_explicit_message);
    }

    #[test]
    fn commit_with_amend() {
        let msgs = find_commit_invocations("git commit --amend -m 'new msg'");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].message, "new msg");
    }

    #[test]
    fn dynamic_message() {
        let msgs = find_commit_invocations("git commit -m \"fix: $(echo hi)\"");
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].requires_explicit_message);
    }

    #[test]
    fn long_message_flag() {
        let msgs = find_commit_invocations("git commit --message=update docs");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].message, "update");
    }

    #[test]
    fn long_message_flag_quoted() {
        let msgs = find_commit_invocations("git commit --message=\"update docs\"");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].message, "update docs");
    }
}
