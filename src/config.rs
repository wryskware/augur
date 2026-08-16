//! Configuration: `~/.augur/config.toml`, its defaults, and the precedence
//! rules that turn a config file plus CLI flags into a concrete [`Settings`].
//!
//! Precedence, highest first:
//!   CLI flag > `--profile` > `AUGUR_*` env > `[defaults]` in the file > builtin.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

pub const APP_DIR: &str = ".augur";
pub const CONFIG_FILE: &str = "config.toml";
pub const HISTORY_FILE: &str = "history.jsonl";

pub const BUILTIN_PROVIDER: &str = "codex";
pub const BUILTIN_MODEL: &str = "gpt-5.6-luna";
pub const BUILTIN_EFFORT: Effort = Effort::High;

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// `~/.augur`, or `$AUGUR_HOME` when set.
pub fn augur_dir() -> Result<PathBuf> {
    if let Some(dir) = non_empty_env("AUGUR_HOME") {
        return Ok(PathBuf::from(dir));
    }
    let home = dirs::home_dir().context("could not determine your home directory")?;
    Ok(home.join(APP_DIR))
}

/// `~/.augur/config.toml`, or `$AUGUR_CONFIG` when set.
pub fn config_path() -> Result<PathBuf> {
    if let Some(path) = non_empty_env("AUGUR_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    Ok(augur_dir()?.join(CONFIG_FILE))
}

pub fn history_path() -> Result<PathBuf> {
    Ok(augur_dir()?.join(HISTORY_FILE))
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

// ---------------------------------------------------------------------------
// Effort
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Minimal,
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    #[value(name = "xhigh")]
    XHigh,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
        }
    }
}

impl fmt::Display for Effort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Config file shape
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Name of the provider used when no flag/profile/env says otherwise.
    pub provider: Option<String>,
    pub defaults: Defaults,
    pub ui: Ui,
    pub safety: Safety,
    pub shell: ShellCfg,
    pub prompt: PromptCfg,
    pub providers: BTreeMap<String, ProviderCfg>,
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub model: Option<String>,
    pub effort: Option<Effort>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    /// Render the answer as markdown rather than plain text.
    pub markdown: bool,
    /// Show the elapsed-time spinner while the provider thinks.
    pub spinner: bool,
    /// Record answers to `~/.augur/history.jsonl` so `ask last` works.
    pub history: bool,
    /// Entries kept in the history file before it is trimmed.
    pub history_limit: usize,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            markdown: true,
            spinner: true,
            history: true,
            history_limit: 200,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Safety {
    /// Destructive commands always require a typed confirmation, never Enter.
    pub confirm_destructive: bool,
    /// Regexes that mark a command destructive regardless of what the model said.
    pub deny: Vec<String>,
}

impl Default for Safety {
    fn default() -> Self {
        Self {
            confirm_destructive: true,
            deny: default_deny_patterns(),
        }
    }
}

/// Patterns that force a destructive-confirmation prompt.
///
/// Deliberately conservative: a false positive costs one keypress, a false
/// negative costs a filesystem. Anchored so that `rm -rf ./build` does not trip
/// the root-deletion rule.
pub fn default_deny_patterns() -> Vec<String> {
    [
        // Any `rm` whose target is *exactly* root or home, regardless of flag
        // order or count. Anchoring on the target rather than on `-rf` is what
        // keeps `rm -rf ./build` quiet while still catching `rm -fr /`.
        r"(?i)\brm\s+([-\w]+\s+)*(/|/\*|~|\$HOME)(\s|$)",
        r"(?i)Remove-Item[^|;]*-Recurse[^|;]*\b[A-Z]:\\?(\s|$)",
        r"(?i)\bRemove-Item[^|;]*\s(/|~)(\s|$)",
        // A deletion fed from a pipeline has no visible target, so its scope is
        // whatever the left-hand side happened to produce. That is a different
        // risk from `rm -rf ./build`, which names what it will destroy.
        r"(?i)\|\s*Remove-Item\b",
        r"(?i)\|\s*(xargs\s+)?(rm|unlink)(\s|$)",
        r"(?i)\b(mkfs|fdisk|diskpart)\b",
        r"(?i)\bformat\s+[a-z]:",
        r"(?i)\bdd\s+if=",
        r"(?i)\bcurl\b[^|]*\|\s*(ba|z)?sh\b",
        r"(?i)\bwget\b[^|]*\|\s*(ba|z)?sh\b",
        r"(?i)Invoke-(WebRequest|RestMethod)[^|]*\|[^|]*Invoke-Expression",
        r"(?i)\biex\b\s*\(",
        // No lookahead in the `regex` crate, so `--force-with-lease` is excluded
        // by requiring whitespace or end-of-string right after `--force`.
        r"(?i)\bgit\s+push\b[^|;]*\s--force(\s|$)",
        r"(?i)\bgit\s+push\b[^|;]*\s-f(\s|$)",
        r"(?i)\bgit\s+reset\s+--hard\b",
        r"(?i)\bgit\s+clean\b[^|;]*-\w*[fd]",
        r"(?i)\bshutdown\b|\bReboot-Computer\b|\bStop-Computer\b",
        r"(?i)\bchmod\s+-R\s+777\b",
        r"(?i):\(\)\s*\{\s*:\|:&\s*\}\s*;:",
        r"(?i)\bReg(istry)?\s+delete\b|\bRemove-ItemProperty\b.*HKLM",
        r"(?i)\bdrop\s+(database|table)\b",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellCfg {
    /// `auto`, or one of: powershell, pwsh, cmd, bash, zsh, fish, sh.
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PromptCfg {
    /// Replaces the built-in preamble entirely.
    pub preamble: Option<String>,
    /// Same, but read from a file. Loses to `preamble` if both are set.
    pub preamble_file: Option<PathBuf>,
    /// Appended after the built-in preamble. Use this for standing preferences.
    pub extra: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// `codex exec` — the only adapter that can *enforce* the response schema.
    Codex,
    /// `claude -p`.
    Claude,
    /// Any binary at all, driven by an argv template.
    Command,
}

/// A provider table entry. Every field is optional so that a user block can
/// overlay a builtin without restating it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderCfg {
    pub kind: Option<ProviderKind>,
    pub command: Option<String>,
    /// Argv template. `kind = "command"` only. Placeholders: `{model}`,
    /// `{effort}`, `{prompt}`, `{schema_file}`.
    pub args: Option<Vec<String>>,
    /// Appended verbatim after the adapter's own flags.
    pub extra_args: Option<Vec<String>>,
    /// codex only: `-s <sandbox>`.
    pub sandbox: Option<String>,
    /// Whether the provider can be *forced* to emit our JSON schema. When
    /// false, the schema is described in the prompt instead.
    pub supports_schema: Option<bool>,
}

impl ProviderCfg {
    /// `over` wins field by field; unset fields fall through to `self`.
    fn overlay(&self, over: &ProviderCfg) -> ProviderCfg {
        ProviderCfg {
            kind: over.kind.or(self.kind),
            command: over.command.clone().or_else(|| self.command.clone()),
            args: over.args.clone().or_else(|| self.args.clone()),
            extra_args: over.extra_args.clone().or_else(|| self.extra_args.clone()),
            sandbox: over.sandbox.clone().or_else(|| self.sandbox.clone()),
            supports_schema: over.supports_schema.or(self.supports_schema),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
}

/// A provider entry with every hole filled in — what the adapters actually run.
#[derive(Debug, Clone)]
pub struct ResolvedProvider {
    pub name: String,
    pub kind: ProviderKind,
    pub command: String,
    pub args: Vec<String>,
    pub extra_args: Vec<String>,
    pub sandbox: String,
    pub supports_schema: bool,
}

// ---------------------------------------------------------------------------
// Builtin providers
// ---------------------------------------------------------------------------

fn builtin_providers() -> BTreeMap<String, ProviderCfg> {
    let mut map = BTreeMap::new();
    map.insert(
        "codex".to_string(),
        ProviderCfg {
            kind: Some(ProviderKind::Codex),
            command: Some("codex".into()),
            sandbox: Some("read-only".into()),
            supports_schema: Some(true),
            ..Default::default()
        },
    );
    map.insert(
        "claude".to_string(),
        ProviderCfg {
            kind: Some(ProviderKind::Claude),
            command: Some("claude".into()),
            supports_schema: Some(false),
            ..Default::default()
        },
    );
    map
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

impl Config {
    pub fn parse(text: &str) -> Result<Config> {
        toml::from_str(text).context("invalid config file")
    }

    /// Load the config, creating a commented starter file on first run.
    pub fn load_or_init() -> Result<(Config, PathBuf, bool)> {
        let path = config_path()?;
        if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let cfg = Config::parse(&text).with_context(|| format!("in {}", path.display()))?;
            return Ok((cfg, path, false));
        }
        write_starter_config(&path)?;
        let cfg = Config::parse(STARTER_CONFIG).expect("builtin starter config must parse");
        Ok((cfg, path, true))
    }

    /// Load without ever writing to disk. Missing file yields defaults.
    pub fn load_lenient() -> Result<(Config, PathBuf)> {
        let path = config_path()?;
        if !path.exists() {
            return Ok((Config::default(), path));
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg = Config::parse(&text).with_context(|| format!("in {}", path.display()))?;
        Ok((cfg, path))
    }

    /// Merge the named provider over its builtin counterpart and fill defaults.
    pub fn resolve_provider(&self, name: &str) -> Result<ResolvedProvider> {
        let builtins = builtin_providers();
        let base = builtins.get(name);
        let user = self.providers.get(name);

        let merged = match (base, user) {
            (Some(b), Some(u)) => b.overlay(u),
            (Some(b), None) => b.clone(),
            (None, Some(u)) => u.clone(),
            (None, None) => {
                let mut known: Vec<&str> = builtins
                    .keys()
                    .chain(self.providers.keys())
                    .map(String::as_str)
                    .collect();
                known.sort_unstable();
                known.dedup();
                bail!(
                    "unknown provider `{name}`. configured: {}",
                    known.join(", ")
                );
            }
        };

        let kind = merged.kind.ok_or_else(|| {
            anyhow!("provider `{name}` is missing `kind` (codex | claude | command)")
        })?;
        let command = merged
            .command
            .ok_or_else(|| anyhow!("provider `{name}` is missing `command`"))?;
        let args = merged.args.unwrap_or_default();
        if kind == ProviderKind::Command && args.is_empty() {
            bail!("provider `{name}` has kind = \"command\" but no `args` template");
        }

        Ok(ResolvedProvider {
            name: name.to_string(),
            kind,
            command,
            args,
            extra_args: merged.extra_args.unwrap_or_default(),
            sandbox: merged.sandbox.unwrap_or_else(|| "read-only".into()),
            supports_schema: merged
                .supports_schema
                .unwrap_or(kind == ProviderKind::Codex),
        })
    }

    pub fn provider_names(&self) -> Vec<String> {
        let mut names: Vec<String> = builtin_providers()
            .into_keys()
            .chain(self.providers.keys().cloned())
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

// ---------------------------------------------------------------------------
// Precedence
// ---------------------------------------------------------------------------

/// What the CLI contributed. Kept separate so precedence is testable without
/// constructing a full clap parse.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub profile: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub provider: ResolvedProvider,
    pub model: String,
    pub effort: Effort,
}

/// Environment-sourced overrides, read once so tests can inject their own.
#[derive(Debug, Clone, Default)]
pub struct EnvOverrides {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
}

impl EnvOverrides {
    pub fn from_env() -> Self {
        let effort =
            non_empty_env("AUGUR_EFFORT").and_then(|raw| match raw.to_ascii_lowercase().as_str() {
                "minimal" => Some(Effort::Minimal),
                "low" => Some(Effort::Low),
                "medium" => Some(Effort::Medium),
                "high" => Some(Effort::High),
                "xhigh" => Some(Effort::XHigh),
                _ => None,
            });
        Self {
            provider: non_empty_env("AUGUR_PROVIDER"),
            model: non_empty_env("AUGUR_MODEL"),
            effort,
        }
    }
}

pub fn resolve(cfg: &Config, cli: &Overrides, env: &EnvOverrides) -> Result<Settings> {
    let profile = match &cli.profile {
        Some(name) => Some(
            cfg.profiles
                .get(name)
                .ok_or_else(|| {
                    let mut names: Vec<&str> = cfg.profiles.keys().map(String::as_str).collect();
                    names.sort_unstable();
                    if names.is_empty() {
                        anyhow!("unknown profile `{name}`; no profiles are defined")
                    } else {
                        anyhow!("unknown profile `{name}`. defined: {}", names.join(", "))
                    }
                })?
                .clone(),
        ),
        None => None,
    };

    let provider_name = cli
        .provider
        .clone()
        .or_else(|| profile.as_ref().and_then(|p| p.provider.clone()))
        .or_else(|| env.provider.clone())
        .or_else(|| cfg.provider.clone())
        .unwrap_or_else(|| BUILTIN_PROVIDER.to_string());

    let model = cli
        .model
        .clone()
        .or_else(|| profile.as_ref().and_then(|p| p.model.clone()))
        .or_else(|| env.model.clone())
        .or_else(|| cfg.defaults.model.clone())
        .unwrap_or_else(|| BUILTIN_MODEL.to_string());

    let effort = cli
        .effort
        .or_else(|| profile.as_ref().and_then(|p| p.effort))
        .or(env.effort)
        .or(cfg.defaults.effort)
        .unwrap_or(BUILTIN_EFFORT);

    Ok(Settings {
        provider: cfg.resolve_provider(&provider_name)?,
        model,
        effort,
    })
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

pub fn write_starter_config(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, STARTER_CONFIG).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Short keys that expand to their fully-qualified dotted path, so
/// `ask config set model X` does the obvious thing.
pub fn expand_key(key: &str) -> String {
    match key {
        "model" => "defaults.model".to_string(),
        "effort" => "defaults.effort".to_string(),
        _ => key.to_string(),
    }
}

/// Surgically set a dotted key, preserving comments and layout, then validate
/// the result by parsing it back into [`Config`].
pub fn set_key(doc_text: &str, key: &str, value: &str) -> Result<String> {
    use toml_edit::{DocumentMut, Item, Value};

    let key = expand_key(key);
    let mut doc: DocumentMut = doc_text
        .parse()
        .context("existing config is not valid TOML")?;

    let parts: Vec<&str> = key.split('.').collect();
    if parts.iter().any(|p| p.is_empty()) {
        bail!("`{key}` is not a valid dotted key");
    }

    // A bare word like `high` is not valid TOML on its own; fall back to a
    // string literal, matching how `codex -c` treats its values.
    let parsed: Value = value
        .parse::<Value>()
        .unwrap_or_else(|_| Value::from(value.to_string()));

    let mut node = doc.as_item_mut();
    for part in &parts[..parts.len() - 1] {
        let table = node
            .as_table_like_mut()
            .ok_or_else(|| anyhow!("`{}` is not a table", part))?;
        if table.get(part).is_none() {
            table.insert(part, Item::Table(toml_edit::Table::new()));
        }
        node = table.get_mut(part).expect("just inserted");
    }
    let leaf = parts[parts.len() - 1];
    let table = node
        .as_table_like_mut()
        .ok_or_else(|| anyhow!("`{key}` does not name a settable field"))?;
    table.insert(leaf, Item::Value(parsed));

    let updated = doc.to_string();
    Config::parse(&updated)
        .with_context(|| format!("setting `{key}` to `{value}` produced an invalid config"))?;
    Ok(updated)
}

/// Read a dotted key back out of a parsed document, for `ask config get`.
pub fn get_key(doc_text: &str, key: &str) -> Result<Option<String>> {
    use toml_edit::DocumentMut;

    let key = expand_key(key);
    let doc: DocumentMut = doc_text.parse().context("config is not valid TOML")?;
    let mut node = doc.as_item();
    for part in key.split('.') {
        match node.as_table_like().and_then(|t| t.get(part)) {
            Some(next) => node = next,
            None => return Ok(None),
        }
    }
    Ok(Some(match node.as_value() {
        // Unquoted, so `ask config get model` is usable in a shell substitution.
        Some(toml_edit::Value::String(s)) => s.value().clone(),
        Some(v) => v.to_string().trim().to_string(),
        None => node.to_string().trim().to_string(),
    }))
}

pub const STARTER_CONFIG: &str = r##"# augur — configuration for the `ask` command.
# Docs: https://github.com/wryskware/augur
#
# Precedence, highest first:
#   CLI flag  >  --profile  >  AUGUR_* env vars  >  this file  >  builtin

# Provider used when nothing else says otherwise.
provider = "codex"

[defaults]
model  = "gpt-5.6-luna"
effort = "high"          # minimal | low | medium | high | xhigh

[ui]
markdown      = true     # render the answer as markdown
spinner       = true     # elapsed-time spinner while the provider thinks
history       = true     # record answers so `ask last` works
history_limit = 200

[safety]
# Destructive commands always require a typed confirmation — never a bare Enter,
# and `--yes` does not bypass this.
confirm_destructive = true
# Regexes that force the destructive prompt no matter what the model claimed.
# Setting this key replaces the built-in list; see `ask providers --help`.
# deny = ['(?i)\brm\s+-rf\s+/']

[shell]
# auto | powershell | pwsh | cmd | bash | zsh | fish | sh
kind = "auto"

[prompt]
# `preamble` replaces the built-in instructions outright; `extra` appends to
# them. Use `extra` for standing preferences.
# extra = "Prefer ripgrep over grep. I use Windows Terminal with PowerShell 7."

# --- providers -------------------------------------------------------------
# `codex` and `claude` are built in; blocks here overlay them field by field.

[providers.codex]
kind    = "codex"
command = "codex"
sandbox = "read-only"    # read-only | workspace-write | danger-full-access
# extra_args = ["--search"]

[providers.claude]
kind    = "claude"
command = "claude"

# Any other binary can be a provider. Placeholders in `args`:
#   {model}  {effort}  {prompt}  {schema_file}
# An argument that consists solely of an unset placeholder is dropped.
# If no {prompt} appears, the prompt is written to the process's stdin.
#
# [providers.ollama]
# kind            = "command"
# command         = "ollama"
# args            = ["run", "{model}", "{prompt}"]
# supports_schema = false
#
# [providers.gemini]
# kind            = "command"
# command         = "gemini"
# args            = ["-m", "{model}", "-p", "{prompt}"]
# supports_schema = false

# --- profiles --------------------------------------------------------------
# Named bundles: `ask --profile quick "..."`

[profiles.quick]
provider = "codex"
model    = "gpt-5.6-luna"
effort   = "low"
"##;
