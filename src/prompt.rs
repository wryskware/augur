//! Assembles the text sent to the provider: instructions, where the user is
//! standing, anything they piped in, and the question itself.

use std::fmt::Write as _;

use anyhow::{Context, Result};

use crate::config::PromptCfg;
use crate::context::EnvContext;
use crate::response;

pub const BUILTIN_PREAMBLE: &str = "\
You are augur. You answer someone standing at a shell prompt who wants to get
back to work.

- Answer the question that was asked. No preamble, no restating the question,
  no offers of further help.
- If a command answers it, give the exact command for the shell named under
  ENVIRONMENT. Never hand POSIX syntax to PowerShell or cmd: `find`, `grep`,
  `ls`, `head`, `which` and `wc` are not PowerShell commands, and the
  PowerShell aliases that shadow some of those take different arguments.
- Prefer one command to a pipeline of four, and something already installed to
  something the user must install first.
- Always write at least one sentence of prose, even alongside a command: what it
  does, which flag matters, or why the obvious answer from another shell fails
  here. Two or three sentences is the ceiling. Never restate the command in
  prose — it is already shown.
- If the honest answer is explanatory, give prose and no command. Do not invent
  a command just to have something to show.
- Mark a command destructive when running it deletes or overwrites data,
  changes system state, installs or uninstalls software, rewrites git history,
  or sends data to a third party. When it is arguable, mark it.
- Use the working directory and any piped input as context. Only inspect files
  when the question cannot be answered without doing so.";

/// Build the full prompt. `describe_schema` is set when the provider cannot be
/// forced into the schema and has to be asked in prose instead.
pub fn build(
    cfg: &PromptCfg,
    ctx: &EnvContext,
    question: &str,
    include_context: bool,
    describe_schema: bool,
) -> Result<String> {
    let mut out = String::new();

    let preamble = match (&cfg.preamble, &cfg.preamble_file) {
        (Some(inline), _) => inline.clone(),
        (None, Some(path)) => std::fs::read_to_string(path)
            .with_context(|| format!("reading prompt.preamble_file {}", path.display()))?,
        (None, None) => BUILTIN_PREAMBLE.to_string(),
    };
    out.push_str(preamble.trim_end());
    if let Some(extra) = &cfg.extra
        && !extra.trim().is_empty()
    {
        let _ = write!(
            out,
            "\n\nStanding preferences from the user:\n{}",
            extra.trim()
        );
    }

    if include_context {
        let _ = write!(
            out,
            "\n\n## ENVIRONMENT\nos: {}\nshell: {} — {}\ncommands are run as: {}\ncwd: {}",
            ctx.os,
            ctx.shell.name(),
            ctx.shell.dialect(),
            ctx.shell.invocation_hint(),
            ctx.cwd.display(),
        );
        if ctx.shell_guessed {
            out.push_str(
                "\n(the shell was inferred, not confirmed — mention it if your answer depends on which shell this is)",
            );
        }
    }

    if let Some(stdin) = &ctx.piped_stdin {
        let _ = write!(
            out,
            "\n\n## PIPED INPUT\nThe user piped this into augur. It is almost certainly what the question is about.\n\n```\n{}\n```",
            stdin.trim_end()
        );
    }

    let _ = write!(out, "\n\n## QUESTION\n{}", question.trim());

    if describe_schema {
        let schema =
            serde_json::to_string_pretty(&response::json_schema()).expect("schema serializes");
        let _ = write!(
            out,
            "\n\n## RESPONSE FORMAT\nReply with a single JSON object and nothing else — no prose \
             outside it, no code fence, no trailing commentary. It must validate against this \
             schema:\n\n{schema}",
        );
    }

    out.push('\n');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Shell;

    fn ctx() -> EnvContext {
        EnvContext {
            os: "Windows 11".into(),
            shell: Shell::PowerShell,
            shell_guessed: false,
            cwd: std::path::PathBuf::from(r"C:\work"),
            piped_stdin: None,
        }
    }

    #[test]
    fn names_the_shell_and_the_question() {
        let p = build(&PromptCfg::default(), &ctx(), "list files", true, false).unwrap();
        assert!(p.contains("shell: powershell"));
        assert!(p.contains(r"cwd: C:\work"));
        assert!(p.trim_end().ends_with("list files"));
        assert!(!p.contains("RESPONSE FORMAT"));
    }

    #[test]
    fn no_context_flag_drops_the_environment_block() {
        let p = build(&PromptCfg::default(), &ctx(), "q", false, false).unwrap();
        assert!(!p.contains("## ENVIRONMENT"));
    }

    #[test]
    fn schema_is_described_only_when_it_cannot_be_enforced() {
        let p = build(&PromptCfg::default(), &ctx(), "q", true, true).unwrap();
        assert!(p.contains("## RESPONSE FORMAT"));
        assert!(p.contains("\"destructive\""));
    }

    #[test]
    fn custom_preamble_replaces_and_extra_appends() {
        let cfg = PromptCfg {
            preamble: Some("BE TERSE".into()),
            preamble_file: None,
            extra: Some("I use ripgrep".into()),
        };
        let p = build(&cfg, &ctx(), "q", false, false).unwrap();
        assert!(p.starts_with("BE TERSE"));
        assert!(p.contains("I use ripgrep"));
        assert!(!p.contains("You are augur"));
    }

    #[test]
    fn piped_stdin_is_fenced_into_its_own_section() {
        let mut c = ctx();
        c.piped_stdin = Some("error[E0277]: boom".into());
        let p = build(&PromptCfg::default(), &c, "why", true, false).unwrap();
        assert!(p.contains("## PIPED INPUT"));
        assert!(p.contains("error[E0277]: boom"));
    }

    #[test]
    fn shell_uncertainty_is_disclosed_to_the_model() {
        let mut c = ctx();
        c.shell_guessed = true;
        let p = build(&PromptCfg::default(), &c, "q", true, false).unwrap();
        assert!(p.contains("inferred, not confirmed"));
    }
}
