//! Adapters that turn a prompt into a subprocess call and a subprocess call
//! back into raw text.
//!
//! augur deliberately does not speak HTTP to any model vendor. Every provider
//! is a CLI you have already authenticated, which is why there is no API key
//! anywhere in the config. A future HTTP provider would slot in as another
//! [`ProviderKind`] arm in [`build_invocation`] plus a branch in [`execute`].

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::config::{Effort, ProviderKind, ResolvedProvider};

pub struct Request<'a> {
    pub prompt: &'a str,
    pub model: &'a str,
    pub effort: Effort,
    /// Present only when the provider can be forced to obey it.
    pub schema: Option<serde_json::Value>,
}

pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<String>,
    /// When set, the reply is read from this file rather than from stdout.
    pub last_message_file: Option<PathBuf>,
    /// Holds the temp directory open for the lifetime of the call.
    _scratch: Option<tempfile::TempDir>,
}

impl Invocation {
    /// A copy-pasteable rendering of the command line, for `--verbose`.
    pub fn display(&self) -> String {
        let mut parts = vec![self.program.clone()];
        parts.extend(self.args.iter().cloned());
        shell_words::join(parts.iter().map(String::as_str))
    }
}

pub struct Reply {
    pub text: String,
    pub stderr: String,
}

/// Is the provider's binary actually on PATH?
pub fn locate(provider: &ResolvedProvider) -> Option<PathBuf> {
    which::which(&provider.command).ok()
}

pub fn build_invocation(provider: &ResolvedProvider, req: &Request) -> Result<Invocation> {
    match provider.kind {
        ProviderKind::Codex => build_codex(provider, req),
        ProviderKind::Claude => Ok(build_claude(provider, req)),
        ProviderKind::Command => build_command(provider, req),
    }
}

// ---------------------------------------------------------------------------
// codex exec
// ---------------------------------------------------------------------------

fn build_codex(provider: &ResolvedProvider, req: &Request) -> Result<Invocation> {
    let scratch = tempfile::Builder::new()
        .prefix("augur-")
        .tempdir()
        .context("creating a scratch directory for the codex call")?;

    let last_message = scratch.path().join("reply.txt");
    let mut args: Vec<String> = vec!["exec".into()];

    if !req.model.is_empty() {
        args.push("-m".into());
        args.push(req.model.to_string());
    }
    // The value of `-c` is parsed as TOML, so the quotes are load-bearing.
    args.push("-c".into());
    args.push(format!("model_reasoning_effort=\"{}\"", req.effort));

    args.push("-s".into());
    args.push(provider.sandbox.clone());
    // `codex exec` is non-interactive and has no -a/--ask-for-approval flag;
    // setting the policy through config is the only way to say it here, and it
    // matters because there is no one on this end to answer a prompt.
    args.push("-c".into());
    args.push("approval_policy=\"never\"".into());
    args.push("--skip-git-repo-check".into());
    args.push("--ephemeral".into());
    args.push("--color".into());
    args.push("never".into());

    if let Some(schema) = &req.schema {
        let schema_path = scratch.path().join("schema.json");
        std::fs::write(&schema_path, serde_json::to_vec_pretty(schema)?)
            .context("writing the response schema")?;
        args.push("--output-schema".into());
        args.push(path_arg(&schema_path));
    }

    args.push("-o".into());
    args.push(path_arg(&last_message));
    args.extend(provider.extra_args.iter().cloned());
    // Explicitly read the prompt from stdin: it avoids command-line length
    // limits and every layer of quoting between here and the model.
    args.push("-".into());

    Ok(Invocation {
        program: provider.command.clone(),
        args,
        stdin: Some(req.prompt.to_string()),
        last_message_file: Some(last_message),
        _scratch: Some(scratch),
    })
}

// ---------------------------------------------------------------------------
// claude -p
// ---------------------------------------------------------------------------

fn build_claude(provider: &ResolvedProvider, req: &Request) -> Invocation {
    let mut args: Vec<String> = vec!["-p".into(), "--output-format".into(), "text".into()];
    if !req.model.is_empty() {
        args.push("--model".into());
        args.push(req.model.to_string());
    }
    args.extend(provider.extra_args.iter().cloned());

    Invocation {
        program: provider.command.clone(),
        args,
        stdin: Some(req.prompt.to_string()),
        last_message_file: None,
        _scratch: None,
    }
}

// ---------------------------------------------------------------------------
// arbitrary binary, driven by an argv template
// ---------------------------------------------------------------------------

fn build_command(provider: &ResolvedProvider, req: &Request) -> Result<Invocation> {
    let mut scratch = None;
    let schema_path = match &req.schema {
        Some(schema) => {
            let dir = tempfile::Builder::new()
                .prefix("augur-")
                .tempdir()
                .context("creating a scratch directory")?;
            let path = dir.path().join("schema.json");
            std::fs::write(&path, serde_json::to_vec_pretty(schema)?)
                .context("writing the response schema")?;
            let rendered = path_arg(&path);
            scratch = Some(dir);
            Some(rendered)
        }
        None => None,
    };

    let mut prompt_inlined = false;
    let mut args = Vec::new();
    for raw in provider.args.iter().chain(provider.extra_args.iter()) {
        let mut arg = raw.clone();
        if arg.contains("{prompt}") {
            arg = arg.replace("{prompt}", req.prompt);
            prompt_inlined = true;
        }
        arg = arg.replace("{model}", req.model);
        arg = arg.replace("{effort}", req.effort.as_str());
        arg = arg.replace("{schema_file}", schema_path.as_deref().unwrap_or(""));
        // An argument that was nothing but an unset placeholder is dropped
        // rather than passed along as an empty string.
        if arg.is_empty() && !raw.is_empty() {
            continue;
        }
        args.push(arg);
    }

    if args.is_empty() {
        bail!(
            "provider `{}` produced an empty argument list",
            provider.name
        );
    }

    Ok(Invocation {
        program: provider.command.clone(),
        args,
        stdin: (!prompt_inlined).then(|| req.prompt.to_string()),
        last_message_file: None,
        _scratch: scratch,
    })
}

/// Temp paths on Windows can come back as `\\?\C:\...`, which some CLIs choke
/// on when they re-parse the argument.
fn path_arg(path: &Path) -> String {
    let s = path.to_string_lossy().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

pub fn execute(inv: &Invocation, cwd: &Path) -> Result<Reply> {
    // Rust's `Command` only ever appends `.exe`, so a PATHEXT shim — which is
    // how npm-installed CLIs like `codex` and `gemini` land on Windows — is
    // invisible to it. Resolve the real path up front.
    let program = which::which(&inv.program).unwrap_or_else(|_| PathBuf::from(&inv.program));

    let mut command = Command::new(&program);
    command
        .args(&inv.args)
        .current_dir(cwd)
        .stdin(if inv.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "`{}` is not on your PATH.\n  Install it, or point augur elsewhere with `ask config set providers.<name>.command <path>`",
                inv.program
            )
        } else {
            anyhow::Error::new(err).context(format!("launching `{}`", inv.program))
        }
    })?;

    // Feed stdin from its own thread so a provider that streams a lot to stdout
    // cannot deadlock against a prompt we have not finished writing.
    if let Some(text) = inv.stdin.clone() {
        let mut handle = child.stdin.take().expect("stdin was piped");
        std::thread::spawn(move || {
            let _ = handle.write_all(text.as_bytes());
            let _ = handle.flush();
        });
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("waiting for `{}`", inv.program))?;

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    let text = match &inv.last_message_file {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(contents) if !contents.trim().is_empty() => contents,
            // The adapter asked for a file and got nothing; stdout is the only
            // thing left worth trying.
            _ => stdout,
        },
        None => stdout,
    };

    if text.trim().is_empty() {
        if !output.status.success() {
            bail!(
                "`{}` exited with {} and returned nothing.\n{}",
                inv.program,
                exit_description(&output.status),
                indent(tail(&stderr, 20))
            );
        }
        bail!("`{}` returned an empty response", inv.program);
    }

    Ok(Reply { text, stderr })
}

fn exit_description(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "no exit code (killed by a signal)".to_string(),
    }
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

fn indent(text: String) -> String {
    text.lines()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderKind;

    fn provider(kind: ProviderKind, args: Vec<&str>) -> ResolvedProvider {
        ResolvedProvider {
            name: "test".into(),
            kind,
            command: "fake".into(),
            args: args.into_iter().map(String::from).collect(),
            extra_args: vec![],
            sandbox: "read-only".into(),
            supports_schema: false,
        }
    }

    fn request(prompt: &str, schema: Option<serde_json::Value>) -> Request<'_> {
        Request {
            prompt,
            model: "gpt-5.6-luna",
            effort: Effort::High,
            schema,
        }
    }

    #[test]
    fn codex_passes_model_effort_and_reads_the_prompt_from_stdin() {
        let p = provider(ProviderKind::Codex, vec![]);
        let inv = build_codex(&p, &request("hello", None)).unwrap();
        assert_eq!(inv.args[0], "exec");
        assert!(inv.args.windows(2).any(|w| w == ["-m", "gpt-5.6-luna"]));
        assert!(
            inv.args
                .contains(&"model_reasoning_effort=\"high\"".to_string())
        );
        assert!(inv.args.contains(&"approval_policy=\"never\"".to_string()));
        assert_eq!(inv.args.last().unwrap(), "-");
        assert_eq!(inv.stdin.as_deref(), Some("hello"));
        assert!(inv.last_message_file.is_some());
    }

    #[test]
    fn codex_only_writes_a_schema_file_when_given_a_schema() {
        let p = provider(ProviderKind::Codex, vec![]);
        let without = build_codex(&p, &request("hi", None)).unwrap();
        assert!(!without.args.contains(&"--output-schema".to_string()));

        let with = build_codex(&p, &request("hi", Some(serde_json::json!({})))).unwrap();
        assert!(with.args.contains(&"--output-schema".to_string()));
    }

    #[test]
    fn command_template_substitutes_and_inlines_the_prompt() {
        let p = provider(ProviderKind::Command, vec!["run", "{model}", "{prompt}"]);
        let inv = build_command(&p, &request("why is the sky blue", None)).unwrap();
        assert_eq!(inv.args, vec!["run", "gpt-5.6-luna", "why is the sky blue"]);
        assert!(
            inv.stdin.is_none(),
            "an inlined prompt must not also go to stdin"
        );
    }

    #[test]
    fn command_template_falls_back_to_stdin_without_a_prompt_placeholder() {
        let p = provider(ProviderKind::Command, vec!["--effort", "{effort}"]);
        let inv = build_command(&p, &request("q", None)).unwrap();
        assert_eq!(inv.args, vec!["--effort", "high"]);
        assert_eq!(inv.stdin.as_deref(), Some("q"));
    }

    #[test]
    fn unset_placeholder_arguments_are_dropped_not_blanked() {
        let p = provider(
            ProviderKind::Command,
            vec!["--schema", "{schema_file}", "go"],
        );
        let inv = build_command(&p, &request("q", None)).unwrap();
        assert_eq!(inv.args, vec!["--schema", "go"]);
    }

    #[test]
    fn claude_gets_print_mode_and_the_model_flag() {
        let p = provider(ProviderKind::Claude, vec![]);
        let inv = build_claude(&p, &request("q", None));
        assert!(inv.args.contains(&"-p".to_string()));
        assert!(
            inv.args
                .windows(2)
                .any(|w| w == ["--model", "gpt-5.6-luna"])
        );
        assert_eq!(inv.stdin.as_deref(), Some("q"));
    }
}
