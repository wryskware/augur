//! Running the command the user chose, in the shell they are actually using.

use std::process::Command;

use anyhow::{Context, Result};

use crate::context::Shell;

/// Run `command` through `shell`, inheriting stdio so output streams live.
/// Returns the child's exit code (128 when it was killed by a signal).
pub fn run(shell: Shell, command: &str) -> Result<i32> {
    let (program, args) = shell.invocation(command);
    let status = Command::new(&program)
        .args(&args)
        .status()
        .with_context(|| format!("running `{program}`"))?;
    Ok(status.code().unwrap_or(128))
}
