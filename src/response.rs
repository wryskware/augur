//! The shape augur asks providers to answer in, and the forgiving parser that
//! turns whatever actually came back into that shape.
//!
//! `codex` can be *forced* to emit this via `--output-schema`. Everything else
//! is merely asked nicely, so the parser degrades in tiers rather than failing.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Answer {
    /// Markdown prose. Always present, even in the prose-fallback tier.
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub commands: Vec<CommandSuggestion>,
    #[serde(default)]
    pub snippet: Option<Snippet>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandSuggestion {
    pub command: String,
    #[serde(default)]
    pub shell: Option<String>,
    /// One line on why this command, shown under it.
    #[serde(default)]
    pub note: Option<String>,
    /// The model's own assessment. augur's deny-list can override it upward,
    /// never downward.
    #[serde(default)]
    pub destructive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snippet {
    #[serde(default)]
    pub language: Option<String>,
    pub code: String,
    #[serde(default)]
    pub filename: Option<String>,
}

impl Answer {
    pub fn is_empty(&self) -> bool {
        self.answer.trim().is_empty() && self.commands.is_empty() && self.snippet.is_none()
    }
}

/// Which tier of [`parse`] produced a result. Surfaced under `--verbose`
/// because "the model ignored the schema" is a useful thing to be able to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseTier {
    /// The whole payload was our JSON object.
    Strict,
    /// It was buried in a ```json fence or in surrounding chatter.
    Extracted,
    /// It was never JSON; the text is the answer.
    Prose,
}

/// The JSON Schema handed to providers that can enforce it.
///
/// Written for OpenAI strict structured-output rules: every property listed in
/// `required`, `additionalProperties: false` everywhere, optionality expressed
/// as a nullable type rather than by omission.
pub fn json_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["answer", "commands", "snippet"],
        "properties": {
            "answer": {
                "type": "string",
                "description": "Markdown prose answering the question. Never empty, even when a command is supplied: say what the command does or which flag matters, and call out anything surprising about this shell — for example that the POSIX tool the user reached for does not exist here, or that the local equivalent takes different arguments. One or two sentences. Do not restate the command verbatim."
            },
            "commands": {
                "type": "array",
                "description": "Zero or more commands that answer the question, ready to paste into the user's shell. Empty when the answer is purely explanatory. Prefer one command over several.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["command", "shell", "note", "destructive"],
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The exact command, in the user's shell dialect. One command per array entry; a pipeline counts as one."
                        },
                        "shell": {
                            "type": "string",
                            "description": "Shell this is written for: powershell, pwsh, cmd, bash, zsh, fish, sh, or nu."
                        },
                        "note": {
                            "type": "string",
                            "description": "A short line adding something the prose did not already say — a caveat, a useful alternative flag, or a gotcha. Empty string when there is nothing left to add. Never a paraphrase of the prose."
                        },
                        "destructive": {
                            "type": "boolean",
                            "description": "True if running this deletes or overwrites data, changes system state, sends data to a third party, or is otherwise hard to undo."
                        }
                    }
                }
            },
            "snippet": {
                "type": ["object", "null"],
                "description": "Source code that answers the question, when the answer is code rather than a command. Null otherwise.",
                "additionalProperties": false,
                "required": ["language", "code", "filename"],
                "properties": {
                    "language": { "type": "string" },
                    "code": { "type": "string" },
                    "filename": { "type": ["string", "null"] }
                }
            }
        }
    })
}

/// Three tiers, cheapest first. Never fails: the last tier always succeeds.
pub fn parse(raw: &str) -> (Answer, ParseTier) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (Answer::default(), ParseTier::Prose);
    }

    if let Some(answer) = try_answer(trimmed) {
        return (answer, ParseTier::Strict);
    }

    for candidate in json_candidates(trimmed) {
        if let Some(answer) = try_answer(&candidate) {
            return (answer, ParseTier::Extracted);
        }
    }

    (lift_shell_fences(trimmed), ParseTier::Prose)
}

/// Accept a JSON object only if it looks like *our* object. Without this a
/// model that replies with some unrelated JSON blob would parse into an
/// all-defaults [`Answer`] and render as nothing at all.
fn try_answer(text: &str) -> Option<Answer> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let obj = value.as_object()?;
    if !obj.contains_key("answer") && !obj.contains_key("commands") {
        return None;
    }
    let answer: Answer = serde_json::from_value(value).ok()?;
    if answer.is_empty() {
        None
    } else {
        Some(answer)
    }
}

/// Fenced ```json blocks (last first, since models tend to conclude with the
/// real answer), then the outermost balanced brace span.
fn json_candidates(text: &str) -> Vec<String> {
    let mut out = fenced_blocks(text);
    out.reverse();
    if let Some(span) = balanced_object(text) {
        out.push(span);
    }
    out
}

fn fenced_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("```") {
        let after = &rest[open + 3..];
        let (lang, body) = match after.find('\n') {
            Some(nl) => (after[..nl].trim(), &after[nl + 1..]),
            None => break,
        };
        let Some(close) = body.find("```") else { break };
        let content = &body[..close];
        if lang.is_empty()
            || lang.eq_ignore_ascii_case("json")
            || lang.eq_ignore_ascii_case("jsonc")
        {
            blocks.push(content.to_string());
        }
        rest = &body[close + 3..];
    }
    blocks
}

/// The span from the first `{` to its matching `}`, ignoring braces inside
/// strings so that a command containing `{}` cannot end the scan early.
fn balanced_object(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

const SHELL_LANGS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "shell",
    "console",
    "powershell",
    "ps",
    "ps1",
    "pwsh",
    "posh",
    "cmd",
    "bat",
    "batch",
    "nu",
];

/// Prose-tier rescue: a plain-text reply whose only code fence is a shell
/// command still deserves the run prompt.
fn lift_shell_fences(text: &str) -> Answer {
    let mut commands = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("```") {
        let after = &rest[open + 3..];
        let (lang, body) = match after.find('\n') {
            Some(nl) => (after[..nl].trim().to_ascii_lowercase(), &after[nl + 1..]),
            None => break,
        };
        let Some(close) = body.find("```") else { break };
        let content = body[..close].trim();
        if SHELL_LANGS.contains(&lang.as_str()) && !content.is_empty() {
            // Multi-line blocks are scripts, not one-liners; leave those in the
            // prose where they can be read rather than offering to run them.
            if !content.contains('\n') {
                commands.push(CommandSuggestion {
                    command: content.trim_start_matches("$ ").trim().to_string(),
                    shell: if lang.is_empty() {
                        None
                    } else {
                        Some(lang.clone())
                    },
                    note: None,
                    destructive: false,
                });
            }
        }
        rest = &body[close + 3..];
    }
    Answer {
        answer: text.to_string(),
        commands,
        snippet: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_tier_takes_a_clean_object() {
        let raw = r#"{"answer":"use gci","commands":[{"command":"Get-ChildItem -Recurse","shell":"powershell","note":"","destructive":false}],"snippet":null}"#;
        let (answer, tier) = parse(raw);
        assert_eq!(tier, ParseTier::Strict);
        assert_eq!(answer.commands[0].command, "Get-ChildItem -Recurse");
    }

    #[test]
    fn extracted_tier_digs_json_out_of_a_fence() {
        let raw = "Sure!\n\n```json\n{\"answer\":\"hi\",\"commands\":[]}\n```\n\nHope that helps.";
        let (answer, tier) = parse(raw);
        assert_eq!(tier, ParseTier::Extracted);
        assert_eq!(answer.answer, "hi");
    }

    #[test]
    fn extracted_tier_survives_braces_inside_strings() {
        let raw = r#"here you go: {"answer":"use ${x} and }{ chars","commands":[]} done"#;
        let (answer, tier) = parse(raw);
        assert_eq!(tier, ParseTier::Extracted);
        assert!(answer.answer.contains("}{"));
    }

    #[test]
    fn unrelated_json_is_not_mistaken_for_an_answer() {
        let (answer, tier) = parse(r#"{"error":"rate limited"}"#);
        assert_eq!(tier, ParseTier::Prose);
        assert!(answer.answer.contains("rate limited"));
    }

    #[test]
    fn prose_tier_lifts_a_single_line_shell_fence() {
        let raw = "Use this:\n\n```powershell\nGet-ChildItem -Recurse -File\n```\n";
        let (answer, tier) = parse(raw);
        assert_eq!(tier, ParseTier::Prose);
        assert_eq!(answer.commands.len(), 1);
        assert_eq!(answer.commands[0].command, "Get-ChildItem -Recurse -File");
    }

    #[test]
    fn prose_tier_leaves_multiline_scripts_alone() {
        let raw = "```bash\ncd /tmp\nls -la\n```";
        let (answer, _) = parse(raw);
        assert!(answer.commands.is_empty());
    }
}
