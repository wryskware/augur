//! The `[enter] run  [e] edit  [c] copy  [n] dismiss` loop.
//!
//! Single keypresses, because the whole tool is worth about four seconds and a
//! confirmation that needs Enter is one keystroke too many — except for
//! destructive commands, where the extra keystrokes are the entire point.

use std::io::IsTerminal;

use anyhow::Result;
use termimad::crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
use termimad::crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use crate::context::Shell;
use crate::exec;
use crate::render::{Theme, key_hint, warn};
use crate::response::{Answer, CommandSuggestion};
use crate::safety::Guard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A command ran; carries its exit code.
    Ran(i32),
    Copied,
    Dismissed,
    /// There was nothing to offer, or no one there to ask.
    Nothing,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Run the single suggested command without asking. Never applies to a
    /// destructive one.
    pub yes: bool,
    /// Print, but never offer to run.
    pub no_run: bool,
    /// Copy the first command and exit.
    pub copy: bool,
}

enum Key {
    Enter,
    Char(char),
    Escape,
    Interrupt,
}

pub fn offer(
    theme: &Theme,
    answer: &Answer,
    shell: Shell,
    guard: &Guard,
    opts: Options,
) -> Result<Outcome> {
    if answer.commands.is_empty() {
        if opts.copy
            && let Some(snippet) = &answer.snippet
        {
            copy(theme, &snippet.code)?;
            return Ok(Outcome::Copied);
        }
        return Ok(Outcome::Nothing);
    }

    if opts.copy {
        copy(theme, &answer.commands[0].command)?;
        return Ok(Outcome::Copied);
    }
    if opts.no_run {
        return Ok(Outcome::Nothing);
    }

    if opts.yes {
        let chosen = &answer.commands[0];
        if guard.assess(chosen).is_destructive() && guard.confirms_destructive() {
            eprintln!(
                "{}",
                warn(
                    theme,
                    "--yes does not auto-run destructive commands; not running it"
                )
            );
            return Ok(Outcome::Nothing);
        }
        return Ok(Outcome::Ran(exec::run(shell, &chosen.command)?));
    }

    // Interactive from here on; without a terminal there is no one to ask.
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Ok(Outcome::Nothing);
    }

    let mut selected = if answer.commands.len() == 1 {
        Some(0usize)
    } else {
        None
    };

    loop {
        // Built out here so the hints vec can borrow it.
        let numeric = numeric_hint(answer.commands.len());
        let hints: Vec<(&str, &str)> = match selected {
            Some(_) if answer.commands.len() > 1 => {
                vec![
                    ("enter", "run"),
                    ("e", "edit"),
                    ("c", "copy"),
                    ("b", "back"),
                    ("n", "dismiss"),
                ]
            }
            Some(_) => vec![
                ("enter", "run"),
                ("e", "edit"),
                ("c", "copy"),
                ("n", "dismiss"),
            ],
            None => vec![
                (numeric.as_str(), "choose"),
                ("c", "copy 1"),
                ("n", "dismiss"),
            ],
        };
        eprintln!();
        eprintln!("{}", key_hint(theme, &hints));

        let key = read_key()?;
        eprint!("\x1b[2A\r\x1b[J");

        match (key, selected) {
            (Key::Interrupt, _) | (Key::Escape, None) => return Ok(Outcome::Dismissed),
            (Key::Char('n' | 'q'), _) => return Ok(Outcome::Dismissed),
            (Key::Escape, Some(_)) if answer.commands.len() > 1 => selected = None,
            (Key::Escape, Some(_)) => return Ok(Outcome::Dismissed),
            (Key::Char('b'), Some(_)) if answer.commands.len() > 1 => selected = None,

            (Key::Char(c), None) if c.is_ascii_digit() => {
                let index = c.to_digit(10).unwrap_or(0) as usize;
                if index >= 1 && index <= answer.commands.len() {
                    selected = Some(index - 1);
                }
            }
            (Key::Char('c'), None) => {
                copy(theme, &answer.commands[0].command)?;
                return Ok(Outcome::Copied);
            }

            (Key::Enter | Key::Char('r'), Some(index)) => {
                let chosen = &answer.commands[index];
                if !confirm_if_destructive(theme, guard, chosen)? {
                    return Ok(Outcome::Dismissed);
                }
                return Ok(Outcome::Ran(exec::run(shell, &chosen.command)?));
            }
            (Key::Char('c'), Some(index)) => {
                copy(theme, &answer.commands[index].command)?;
                return Ok(Outcome::Copied);
            }
            (Key::Char('e'), Some(index)) => {
                let original = &answer.commands[index];
                let Some(edited) = edit(&original.command)? else {
                    return Ok(Outcome::Dismissed);
                };
                if edited.trim().is_empty() {
                    return Ok(Outcome::Dismissed);
                }
                // Re-assess: the user may have edited a safe command into a
                // dangerous one, and the model's flag no longer describes it.
                let revised = CommandSuggestion {
                    command: edited,
                    shell: original.shell.clone(),
                    note: None,
                    destructive: false,
                };
                if !confirm_if_destructive(theme, guard, &revised)? {
                    return Ok(Outcome::Dismissed);
                }
                return Ok(Outcome::Ran(exec::run(shell, &revised.command)?));
            }
            _ => {}
        }
    }
}

fn numeric_hint(count: usize) -> String {
    // The menu only reads a single digit, so nine is the ceiling.
    if count >= 9 {
        "1-9".to_string()
    } else {
        format!("1-{count}")
    }
}

/// Returns false when the user declined. Destructive commands need a typed
/// confirmation; nothing here can be satisfied by leaning on Enter.
fn confirm_if_destructive(
    theme: &Theme,
    guard: &Guard,
    suggestion: &CommandSuggestion,
) -> Result<bool> {
    let risk = guard.assess(suggestion);
    if !risk.is_destructive() || !guard.confirms_destructive() {
        return Ok(true);
    }
    eprintln!();
    if let Some(reason) = risk.explain() {
        eprintln!("{}", warn(theme, &reason));
    }
    let answer = inquire::Confirm::new("Run it anyway?")
        .with_default(false)
        .prompt();
    Ok(matches!(answer, Ok(true)))
}

fn edit(current: &str) -> Result<Option<String>> {
    match inquire::Text::new("").with_initial_value(current).prompt() {
        Ok(text) => Ok(Some(text)),
        Err(
            inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted,
        ) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn copy(theme: &Theme, text: &str) -> Result<()> {
    match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())) {
        Ok(()) => {
            eprintln!("{}", warn(theme, "copied to clipboard").replace("! ", "· "));
            Ok(())
        }
        Err(err) => {
            eprintln!(
                "{}",
                warn(theme, &format!("could not reach the clipboard: {err}"))
            );
            eprintln!("{text}");
            Ok(())
        }
    }
}

/// One keypress, with raw mode always restored — including on error, since
/// leaving a terminal in raw mode is a worse outcome than the error itself.
fn read_key() -> Result<Key> {
    enable_raw_mode()?;
    let result = read_key_inner();
    let _ = disable_raw_mode();
    result
}

fn read_key_inner() -> Result<Key> {
    loop {
        match read()? {
            Event::Key(event) => {
                // Windows delivers key-release events too; acting on both would
                // process every keystroke twice.
                if event.kind != KeyEventKind::Press {
                    continue;
                }
                let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
                return Ok(match event.code {
                    KeyCode::Enter => Key::Enter,
                    KeyCode::Esc => Key::Escape,
                    KeyCode::Char('c' | 'd') if ctrl => Key::Interrupt,
                    KeyCode::Char(c) => Key::Char(c.to_ascii_lowercase()),
                    _ => continue,
                });
            }
            _ => continue,
        }
    }
}
