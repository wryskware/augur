//! Everything augur knows about *where you are standing* when you ask.
//!
//! The whole point of the tool is that `find` is not a PowerShell command, so
//! getting the shell right matters more than anything else here.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use crate::config::ShellCfg;

// `PowerShell` is a proper noun; renaming it to satisfy the lint would make
// every match arm read worse than the lint it silences.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    PowerShell,
    Pwsh,
    Cmd,
    Bash,
    Zsh,
    Fish,
    Sh,
    Nu,
}

impl Shell {
    pub fn name(self) -> &'static str {
        match self {
            Shell::PowerShell => "powershell",
            Shell::Pwsh => "pwsh",
            Shell::Cmd => "cmd",
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish",
            Shell::Sh => "sh",
            Shell::Nu => "nu",
        }
    }

    /// Human-facing description of the dialect, used in the prompt so the model
    /// cannot mistake PowerShell for POSIX.
    pub fn dialect(self) -> &'static str {
        match self {
            Shell::PowerShell => {
                "Windows PowerShell 5.1 (POSIX tools like find/grep/ls are NOT available)"
            }
            Shell::Pwsh => "PowerShell 7+ (cross-platform; POSIX tools are not guaranteed)",
            Shell::Cmd => "Windows cmd.exe batch syntax",
            Shell::Bash => "GNU bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish (note: no $(), use ())",
            Shell::Sh => "POSIX sh",
            Shell::Nu => "nushell (structured pipelines, not POSIX)",
        }
    }

    pub fn parse(raw: &str) -> Option<Shell> {
        // Lowercase first: `POWERSHELL.EXE` is a perfectly normal thing for the
        // process table to hand back on Windows.
        let lowered = raw
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(raw)
            .to_ascii_lowercase();
        let stem = lowered.trim_end_matches(".exe");
        Some(match stem {
            "powershell" | "windowspowershell" => Shell::PowerShell,
            "pwsh" | "pwsh-preview" => Shell::Pwsh,
            "cmd" => Shell::Cmd,
            "bash" | "git-bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            "sh" | "dash" | "ash" => Shell::Sh,
            "nu" | "nushell" => Shell::Nu,
            _ => return None,
        })
    }

    /// How to hand this shell a single command string: `(program, args)` where
    /// the command itself is the final argument.
    pub fn invocation(self, command: &str) -> (String, Vec<String>) {
        let (program, flags): (&str, &[&str]) = match self {
            Shell::PowerShell => ("powershell", &["-NoProfile", "-Command"]),
            Shell::Pwsh => ("pwsh", &["-NoLogo", "-NoProfile", "-Command"]),
            Shell::Cmd => ("cmd", &["/C"]),
            Shell::Bash => ("bash", &["-c"]),
            Shell::Zsh => ("zsh", &["-c"]),
            Shell::Fish => ("fish", &["-c"]),
            Shell::Sh => ("sh", &["-c"]),
            Shell::Nu => ("nu", &["-c"]),
        };
        let mut args: Vec<String> = flags.iter().map(|s| s.to_string()).collect();
        args.push(command.to_string());
        (program.to_string(), args)
    }

    /// The invocation shape, shown to the model so it knows how its command
    /// will be executed (quoting, `;` vs `&&`, and so on).
    pub fn invocation_hint(self) -> &'static str {
        match self {
            Shell::PowerShell => "powershell -NoProfile -Command \"<command>\"",
            Shell::Pwsh => "pwsh -NoLogo -NoProfile -Command \"<command>\"",
            Shell::Cmd => "cmd /C \"<command>\"",
            Shell::Bash => "bash -c \"<command>\"",
            Shell::Zsh => "zsh -c \"<command>\"",
            Shell::Fish => "fish -c \"<command>\"",
            Shell::Sh => "sh -c \"<command>\"",
            Shell::Nu => "nu -c \"<command>\"",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnvContext {
    pub os: String,
    pub shell: Shell,
    /// True when the shell was guessed rather than read from config or the
    /// process tree — worth hedging about in the prompt.
    pub shell_guessed: bool,
    pub cwd: PathBuf,
    /// Non-TTY stdin, captured verbatim so `cargo build 2>&1 | ask "why"` works.
    pub piped_stdin: Option<String>,
}

impl EnvContext {
    pub fn detect(shell_cfg: &ShellCfg) -> EnvContext {
        let (shell, shell_guessed) = detect_shell(shell_cfg);
        EnvContext {
            os: os_description(),
            shell,
            shell_guessed,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            piped_stdin: read_piped_stdin(),
        }
    }
}

/// Config override, then the process tree, then `$SHELL`, then the platform
/// default. Only the last two count as guessing.
fn detect_shell(cfg: &ShellCfg) -> (Shell, bool) {
    if let Some(kind) = cfg.kind.as_deref()
        && !kind.eq_ignore_ascii_case("auto")
        && let Some(shell) = Shell::parse(kind)
    {
        return (shell, false);
    }
    if let Some(shell) = detect_from_process_tree() {
        return (shell, false);
    }
    if let Ok(sh) = std::env::var("SHELL")
        && let Some(shell) = Shell::parse(&sh)
    {
        return (shell, true);
    }
    if cfg!(windows) {
        (Shell::PowerShell, true)
    } else {
        (Shell::Sh, true)
    }
}

/// Walk up the process tree looking for something that looks like a shell.
/// Refreshes one PID at a time rather than enumerating every process — the
/// full sweep costs tens of milliseconds on Windows and we only need a chain.
fn detect_from_process_tree() -> Option<Shell> {
    use sysinfo::{ProcessesToUpdate, System};

    let mut sys = System::new();
    let mut pid = sysinfo::get_current_pid().ok()?;
    for _ in 0..8 {
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);
        let proc = sys.process(pid)?;
        let name = proc.name().to_string_lossy().to_string();
        if let Some(shell) = Shell::parse(&name) {
            return Some(shell);
        }
        pid = proc.parent()?;
    }
    None
}

fn os_description() -> String {
    use sysinfo::System;
    let long = System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string());
    match System::kernel_version() {
        Some(kernel) => format!("{long} (kernel {kernel})"),
        None => long,
    }
}

fn read_piped_stdin() -> Option<String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let mut buf: Vec<u8> = Vec::new();
    // A binary or gigantic pipe is not worth forwarding; cap it.
    stdin.take(64 * 1024).read_to_end(&mut buf).ok()?;
    piped_text(&buf)
}

/// Bytes off the pipe to the text augur forwards.
///
/// Lossy on purpose: the 64 KiB cap lands wherever it lands, and losing a whole
/// build log because its tail was cut mid-character is far worse than one
/// replacement glyph. Empty or whitespace-only input is still `None`.
fn piped_text(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    if text.trim().is_empty() {
        return None;
    }
    Some(text.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shell_names_from_paths_and_exes() {
        assert_eq!(Shell::parse("pwsh.exe"), Some(Shell::Pwsh));
        assert_eq!(
            Shell::parse(r"C:\Windows\System32\cmd.exe"),
            Some(Shell::Cmd)
        );
        assert_eq!(Shell::parse("/usr/bin/zsh"), Some(Shell::Zsh));
        assert_eq!(Shell::parse("PowerShell.EXE"), Some(Shell::PowerShell));
        assert_eq!(Shell::parse("code"), None);
    }

    #[test]
    fn a_pipe_cut_mid_character_keeps_the_content_it_did_get() {
        let full = "error: naïve café ☕";
        // Drop one byte of the trailing three-byte character, exactly what the
        // 64 KiB cap does to a large non-ASCII log.
        let truncated = &full.as_bytes()[..full.len() - 1];
        let text = piped_text(truncated).expect("a split character must not discard the pipe");
        assert!(text.starts_with("error: naïve café"), "{text}");
    }

    #[test]
    fn empty_and_whitespace_only_pipes_stay_none() {
        assert_eq!(piped_text(b""), None);
        assert_eq!(piped_text(b"  \r\n\t "), None);
    }

    #[test]
    fn invocation_puts_the_command_last() {
        let (program, args) = Shell::Pwsh.invocation("Get-ChildItem");
        assert_eq!(program, "pwsh");
        assert_eq!(args.last().unwrap(), "Get-ChildItem");
        assert!(args.contains(&"-NoProfile".to_string()));
    }
}
