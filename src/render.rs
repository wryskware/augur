//! Everything the user sees: the spinner, the answer body, and the command
//! blocks. All chrome goes to stderr so that `ask ... > file` and `--json`
//! stay pipeable.

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::response::{Answer, CommandSuggestion, Snippet};
use crate::safety::Risk;

const INDENT: &str = "  ";

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub color: bool,
    pub markdown: bool,
    pub width: usize,
}

impl Theme {
    pub fn detect(markdown: bool) -> Theme {
        let color = color_supported();
        Theme {
            color,
            markdown,
            width: terminal_width(),
        }
    }

    pub fn plain() -> Theme {
        Theme {
            color: false,
            markdown: false,
            width: 80,
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }
    pub fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }
    pub fn cyan(&self, text: &str) -> String {
        self.paint("36", text)
    }
    pub fn red(&self, text: &str) -> String {
        self.paint("1;31", text)
    }
    pub fn yellow(&self, text: &str) -> String {
        self.paint("33", text)
    }
    pub fn green(&self, text: &str) -> String {
        self.paint("32", text)
    }
}

fn color_supported() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    // The answer body goes to stdout, so that is the stream whose redirection
    // decides whether escape codes are wanted.
    if !std::io::stdout().is_terminal() {
        return false;
    }
    if matches!(std::env::var("TERM").as_deref(), Ok("dumb")) {
        return false;
    }
    ansi_enabled()
}

#[cfg(windows)]
fn ansi_enabled() -> bool {
    // Also *turns on* virtual-terminal processing as a side effect, which is
    // what makes escape codes work in a plain conhost window.
    termimad::crossterm::ansi_support::supports_ansi()
}

#[cfg(not(windows))]
fn ansi_enabled() -> bool {
    true
}

fn terminal_width() -> usize {
    termimad::crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .clamp(40, 100)
}

// ---------------------------------------------------------------------------
// Spinner
// ---------------------------------------------------------------------------

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const ASCII_FRAMES: [&str; 4] = ["-", "\\", "|", "/"];

pub struct Spinner {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    started: Instant,
}

impl Spinner {
    /// A no-op spinner, for non-TTY output and `--quiet`.
    pub fn disabled() -> Spinner {
        Spinner {
            stop: Arc::new(AtomicBool::new(true)),
            handle: None,
            started: Instant::now(),
        }
    }

    pub fn start(label: String, theme: &Theme) -> Spinner {
        if !std::io::stderr().is_terminal() {
            return Spinner::disabled();
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let color = theme.color;
        let started = Instant::now();
        let handle = std::thread::spawn(move || {
            let frames: &[&str] = if color { &FRAMES } else { &ASCII_FRAMES };
            let mut i = 0usize;
            while !flag.load(Ordering::Relaxed) {
                let elapsed = started.elapsed().as_secs_f32();
                let frame = frames[i % frames.len()];
                let line = if color {
                    format!("\x1b[36m{frame}\x1b[0m \x1b[2m{label} · {elapsed:.1}s\x1b[0m")
                } else {
                    format!("{frame} {label} · {elapsed:.1}s")
                };
                let mut err = std::io::stderr();
                let _ = write!(err, "\r{INDENT}{line}\x1b[K");
                let _ = err.flush();
                i += 1;
                std::thread::sleep(std::time::Duration::from_millis(90));
            }
        });
        Spinner {
            stop,
            handle: Some(handle),
            started,
        }
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    pub fn finish(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
            let mut err = std::io::stderr();
            let _ = write!(err, "\r\x1b[K");
            let _ = err.flush();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.finish();
    }
}

// ---------------------------------------------------------------------------
// Answer body
// ---------------------------------------------------------------------------

pub fn answer_body(theme: &Theme, answer: &Answer) -> String {
    let text = answer.answer.trim();
    if text.is_empty() {
        return String::new();
    }
    if theme.markdown {
        indent(&markdown(theme, text))
    } else {
        indent(&wrap(text, theme.width.saturating_sub(INDENT.len())))
    }
}

fn markdown(theme: &Theme, src: &str) -> String {
    let mut skin = if theme.color {
        let mut skin = termimad::MadSkin::default();
        use termimad::crossterm::style::Color;
        skin.set_headers_fg(Color::Cyan);
        skin.bold.set_fg(Color::White);
        skin.inline_code.set_fg(Color::Cyan);
        skin.code_block.set_fg(Color::Cyan);
        skin.italic.set_fg(Color::Grey);
        skin
    } else {
        termimad::MadSkin::no_style()
    };
    // termimad draws its own rules edge to edge; keep them inside our margin.
    skin.paragraph.left_margin = 0;
    let width = theme.width.saturating_sub(INDENT.len()).max(20);
    format!("{}", skin.text(src, Some(width)))
}

fn wrap(text: &str, width: usize) -> String {
    let width = width.max(20);
    let mut out = String::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                out.push_str(line.trim_end());
                out.push('\n');
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn indent(text: &str) -> String {
    text.trim_end()
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{INDENT}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Commands and snippets
// ---------------------------------------------------------------------------

/// One command block. `ordinal` is `Some(n)` when there is more than one and
/// the user will be picking by number.
pub fn command_block(
    theme: &Theme,
    suggestion: &CommandSuggestion,
    risk: &Risk,
    ordinal: Option<usize>,
) -> String {
    let destructive = risk.is_destructive();
    let marker = if destructive { "!" } else { "❯" };
    let marker = if theme.color && !destructive {
        theme.cyan(marker)
    } else if destructive {
        theme.red(if theme.color { marker } else { "!" })
    } else {
        marker.to_string()
    };

    let mut out = String::new();
    let lead = match ordinal {
        Some(n) => format!("{INDENT}{} {marker} ", theme.dim(&n.to_string())),
        None => format!("{INDENT}{marker} "),
    };
    out.push_str(&lead);
    out.push_str(&theme.bold(suggestion.command.trim()));

    if let Some(reason) = risk.explain() {
        out.push('\n');
        out.push_str(&format!(
            "{INDENT}{INDENT}{}",
            theme.red(&format!("! {reason}"))
        ));
    }
    if let Some(note) = suggestion.note.as_deref()
        && !note.trim().is_empty()
    {
        out.push('\n');
        out.push_str(&format!("{INDENT}{INDENT}{}", theme.dim(note.trim())));
    }
    out
}

pub fn snippet_block(theme: &Theme, snippet: &Snippet) -> String {
    let mut out = String::new();
    let header = match (&snippet.filename, &snippet.language) {
        (Some(name), _) if !name.trim().is_empty() => name.trim().to_string(),
        (_, Some(lang)) if !lang.trim().is_empty() => lang.trim().to_string(),
        _ => "snippet".to_string(),
    };
    out.push_str(&format!(
        "{INDENT}{}\n",
        theme.dim(&format!("— {header} —"))
    ));
    for line in snippet.code.trim_end().lines() {
        out.push_str(&format!("{INDENT}{}\n", theme.green(line)));
    }
    out.trim_end().to_string()
}

/// The `[enter] run  [e] edit …` footer.
pub fn key_hint(theme: &Theme, pairs: &[(&str, &str)]) -> String {
    let rendered: Vec<String> = pairs
        .iter()
        .map(|(key, label)| format!("{}{}", theme.cyan(&format!("[{key}] ")), theme.dim(label)))
        .collect();
    format!("{INDENT}{}", rendered.join(theme.dim("   ").as_str()))
}

pub fn banner(theme: &Theme, provider: &str, model: &str, effort: &str, secs: f32) -> String {
    format!(
        "{INDENT}{}",
        theme.dim(&format!("{provider} · {model} · {effort} · {secs:.1}s"))
    )
}

pub fn warn(theme: &Theme, message: &str) -> String {
    format!("{INDENT}{}", theme.yellow(&format!("! {message}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety::{Reason, Risk};

    fn theme() -> Theme {
        Theme::plain()
    }

    fn cmd(text: &str) -> CommandSuggestion {
        CommandSuggestion {
            command: text.into(),
            shell: None,
            note: None,
            destructive: false,
        }
    }

    #[test]
    fn command_block_indents_and_marks() {
        let block = command_block(&theme(), &cmd("ls -la"), &Risk::Safe, None);
        assert_eq!(block, "  ❯ ls -la");
    }

    #[test]
    fn destructive_blocks_state_the_reason() {
        let risk = Risk::Destructive(Reason::Pattern("rm -rf /".into()));
        let block = command_block(&theme(), &cmd("rm -rf /"), &risk, None);
        assert!(block.contains("! matches a safety rule: rm -rf /"));
    }

    #[test]
    fn numbered_blocks_carry_their_ordinal() {
        let block = command_block(&theme(), &cmd("ls"), &Risk::Safe, Some(2));
        assert!(block.starts_with("  2 ❯ ls"));
    }

    #[test]
    fn plain_wrapping_respects_the_width() {
        let wrapped = wrap("aaa bbb ccc ddd eee fff ggg hhh", 20);
        assert!(wrapped.lines().all(|l| l.chars().count() <= 20));
    }

    #[test]
    fn plain_theme_emits_no_escape_codes() {
        let answer = Answer {
            answer: "hello **world**".into(),
            ..Default::default()
        };
        let body = answer_body(&theme(), &answer);
        assert!(!body.contains('\x1b'));
    }
}
