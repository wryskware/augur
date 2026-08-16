//! A JSONL log of answers, so `ask last` can reprint or rerun the previous one
//! without paying for the question twice.

use std::io::Write;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config;
use crate::response::Answer;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Unix seconds. Absent on entries written by a build with a broken clock.
    #[serde(default)]
    pub at: u64,
    pub question: String,
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub shell: String,
    pub cwd: String,
    pub answer: Answer,
}

impl Entry {
    pub fn now(
        question: &str,
        provider: &str,
        model: &str,
        effort: &str,
        shell: &str,
        cwd: &str,
        answer: &Answer,
    ) -> Entry {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Entry {
            at,
            question: question.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            effort: effort.to_string(),
            shell: shell.to_string(),
            cwd: cwd.to_string(),
            answer: answer.clone(),
        }
    }
}

/// Append an entry, trimming the file when it outgrows `limit`.
///
/// History is a convenience, never a correctness concern, so failures here are
/// reported by the caller at most as a warning.
pub fn append(entry: &Entry, limit: usize) -> Result<()> {
    let path = config::history_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let line = serde_json::to_string(entry).context("serializing a history entry")?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    writeln!(file, "{line}")?;
    drop(file);

    trim(&path, limit)
}

/// Rewrite the file to its last `limit` lines. Only pays the read/write cost
/// once every `limit / 4` appends.
fn trim(path: &std::path::Path, limit: usize) -> Result<()> {
    if limit == 0 {
        return Ok(());
    }
    let slack = limit + (limit / 4).max(8);
    let text = std::fs::read_to_string(path)?;
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() <= slack {
        return Ok(());
    }
    let kept = &lines[lines.len() - limit..];
    std::fs::write(path, format!("{}\n", kept.join("\n")))?;
    Ok(())
}

/// The most recent entry that still parses. A single corrupt trailing line
/// should not hide the whole history.
pub fn last() -> Result<Option<Entry>> {
    let path = config::history_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    for line in text.lines().rev() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<Entry>(line) {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}
