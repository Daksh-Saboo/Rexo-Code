//! Turns one line of REPL input into either a command or a plain prompt for
//! the model. Kept dependency-free (no shell-lexer crate) since the syntax
//! we need to support — `/name arg "quoted arg"` and `>`/`> path` — is
//! small enough to hand-roll and unit-test directly.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedLine {
    /// A `/command` (or the special `>` workspace-selector shortcut,
    /// internally named `"workspace-select"`) with its arguments already
    /// split and unquoted.
    Command { name: String, args: Vec<String> },
    /// `!<command>` — run `<command>` directly through the OS shell,
    /// bypassing the model entirely for this one line. A bare `!` (no
    /// command on the same line) toggles persistent shell mode instead —
    /// that's stateful REPL behavior, not a parsing concern, so it's
    /// handled directly in `cli::tui::run_inner` the same way `exit` and
    /// `{?}` are, rather than represented here.
    Shell(String),
    /// Anything else — sent to the model as-is (untouched, not tokenized).
    Prompt(String),
}

/// Split `input` on whitespace, honoring `"double"` and `'single'` quoted
/// segments so paths like `"I:\My Projects\App"` survive as one argument.
/// An unterminated quote is treated as running to the end of the string
/// rather than erroring — this is a REPL convenience tool, not a shell.
pub fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;

    for ch in input.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    quote = Some(ch);
                    in_token = true;
                } else if ch.is_whitespace() {
                    if in_token {
                        tokens.push(std::mem::take(&mut current));
                        in_token = false;
                    }
                } else {
                    current.push(ch);
                    in_token = true;
                }
            }
        }
    }
    if in_token {
        tokens.push(current);
    }

    tokens
}

/// Parse one raw REPL line into a [`ParsedLine`]. Empty/whitespace-only
/// lines are represented as an empty `Prompt` — the caller decides whether
/// to skip those.
pub fn parse_line(input: &str) -> ParsedLine {
    let trimmed = input.trim();

    if trimmed == ">" || trimmed.starts_with("> ") || trimmed.starts_with(">\t") {
        let rest = trimmed[1..].trim();
        let args = if rest.is_empty() {
            Vec::new()
        } else {
            tokenize(rest)
        };
        return ParsedLine::Command {
            name: "workspace-select".to_string(),
            args,
        };
    }

    // A bare `!` is handled by the caller (toggles persistent shell mode
    // — see `ParsedLine::Shell`'s doc comment); only `!<something>` is a
    // one-shot shell command here.
    if let Some(rest) = trimmed.strip_prefix('!') {
        let rest = rest.trim();
        if !rest.is_empty() {
            return ParsedLine::Shell(rest.to_string());
        }
    }

    if let Some(rest) = trimmed.strip_prefix('/') {
        let mut tokens = tokenize(rest);
        if tokens.is_empty() {
            // Bare "/" with nothing after it — treat as /help rather than
            // an error; it's the most helpful guess at what was meant.
            return ParsedLine::Command {
                name: "help".to_string(),
                args: Vec::new(),
            };
        }
        let name = tokens.remove(0).to_lowercase();
        return ParsedLine::Command { name, args: tokens };
    }

    ParsedLine::Prompt(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_a_prompt() {
        assert_eq!(parse_line("hello"), ParsedLine::Prompt("hello".to_string()));
        assert_eq!(
            parse_line("fix the auth bug"),
            ParsedLine::Prompt("fix the auth bug".to_string())
        );
    }

    #[test]
    fn slash_command_with_no_args() {
        assert_eq!(
            parse_line("/model"),
            ParsedLine::Command {
                name: "model".to_string(),
                args: vec![]
            }
        );
    }

    #[test]
    fn slash_command_is_case_insensitive_on_name_only() {
        assert_eq!(
            parse_line("/MODEL Kimi-K3"),
            ParsedLine::Command {
                name: "model".to_string(),
                args: vec!["Kimi-K3".to_string()]
            }
        );
    }

    #[test]
    fn slash_command_with_quoted_argument() {
        assert_eq!(
            parse_line("/workspace \"I:\\My Projects\\Test App\""),
            ParsedLine::Command {
                name: "workspace".to_string(),
                args: vec!["I:\\My Projects\\Test App".to_string()]
            }
        );
    }

    #[test]
    fn slash_command_with_multiple_args() {
        assert_eq!(
            parse_line("/permissions edit always"),
            ParsedLine::Command {
                name: "permissions".to_string(),
                args: vec!["edit".to_string(), "always".to_string()]
            }
        );
    }

    #[test]
    fn bare_slash_defaults_to_help() {
        assert_eq!(
            parse_line("/"),
            ParsedLine::Command {
                name: "help".to_string(),
                args: vec![]
            }
        );
    }

    #[test]
    fn workspace_selector_shortcut_bare() {
        assert_eq!(
            parse_line(">"),
            ParsedLine::Command {
                name: "workspace-select".to_string(),
                args: vec![]
            }
        );
    }

    #[test]
    fn workspace_selector_shortcut_with_path() {
        assert_eq!(
            parse_line("> I:\\Pro"),
            ParsedLine::Command {
                name: "workspace-select".to_string(),
                args: vec!["I:\\Pro".to_string()]
            }
        );
    }

    #[test]
    fn tokenize_handles_mixed_quotes_and_bare_words() {
        let tokens = tokenize("model \"my custom model\" 'another one' plain");
        assert_eq!(
            tokens,
            vec![
                "model".to_string(),
                "my custom model".to_string(),
                "another one".to_string(),
                "plain".to_string(),
            ]
        );
    }

    #[test]
    fn does_not_confuse_a_url_with_a_prompt() {
        assert_eq!(
            parse_line("/base-url http://localhost:11434/v1"),
            ParsedLine::Command {
                name: "base-url".to_string(),
                args: vec!["http://localhost:11434/v1".to_string()]
            }
        );
    }

    #[test]
    fn bang_prefix_with_a_command_is_shell() {
        assert_eq!(parse_line("!ls -la"), ParsedLine::Shell("ls -la".to_string()));
        assert_eq!(parse_line("!git status"), ParsedLine::Shell("git status".to_string()));
    }

    #[test]
    fn bang_prefix_trims_surrounding_whitespace() {
        assert_eq!(parse_line("  !  echo hi  "), ParsedLine::Shell("echo hi".to_string()));
    }

    #[test]
    fn bare_bang_is_not_a_shell_command() {
        // The bare-`!` toggle is handled by the caller (run_inner) *before*
        // it ever reaches the parser — see `ParsedLine::Shell`'s doc
        // comment and `run_inner`'s bare-trigger checks (same pattern as
        // "exit"/"{?}"). In isolation, the parser itself just falls
        // through to treating it as ordinary (empty-ish) prompt text.
        assert_eq!(parse_line("!"), ParsedLine::Prompt("!".to_string()));
    }
}
