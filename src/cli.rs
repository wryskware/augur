//! The command-line surface.

use clap::{Parser, Subcommand};

use crate::config::Effort;

const ABOUT: &str = "Ask your terminal a question; get an answer or a runnable command.";

const AFTER_HELP: &str = "\
EXAMPLES
  ask list all the files under this directory recursively
  ask \"how do I undo the last commit but keep the changes\"
  cargo build 2>&1 | ask \"what's wrong\"
  ask -e low \"what port does postgres use\"
  ask --profile quick \"pretty-print this json file\"

NOTES
  Quotes are optional. If your question starts with a dash, or you want a flag
  to appear after it, separate them with `--`:  ask -- --help means what?

  Config lives at ~/.augur/config.toml ($AUGUR_CONFIG overrides). Precedence is
  flag > --profile > AUGUR_* env > config file > builtin.
";

#[derive(Debug, Parser)]
#[command(
    name = "ask",
    version,
    about = ABOUT,
    after_help = AFTER_HELP,
    // Without this, `ask config show` is parsed as a three-word question.
    subcommand_precedence_over_arg = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Sub>,

    /// The question. Quotes optional.
    #[arg(value_name = "QUESTION", num_args = 1..)]
    pub question: Vec<String>,

    /// Model to ask, overriding the configured default.
    #[arg(short, long, value_name = "MODEL", global = true)]
    pub model: Option<String>,

    /// Reasoning effort, overriding the configured default.
    #[arg(short, long, value_name = "LEVEL", global = true)]
    pub effort: Option<Effort>,

    /// Provider to route through (codex, claude, or one you configured).
    #[arg(short, long, value_name = "NAME", global = true)]
    pub provider: Option<String>,

    /// A named provider/model/effort bundle from [profiles].
    #[arg(long, value_name = "NAME", global = true)]
    pub profile: Option<String>,

    /// Run the suggested command without asking. Never runs destructive ones.
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Print the answer but never offer to run anything.
    #[arg(short = 'n', long)]
    pub no_run: bool,

    /// Copy the command (or snippet) to the clipboard instead of running it.
    #[arg(short = 'c', long)]
    pub copy: bool,

    /// Emit the structured response as JSON, with no terminal chrome.
    #[arg(long, conflicts_with = "raw")]
    pub json: bool,

    /// Print the provider's reply verbatim, unparsed.
    #[arg(long)]
    pub raw: bool,

    /// Omit the os/shell/cwd block from the prompt.
    #[arg(long)]
    pub no_context: bool,

    /// Show the exact provider command line, timing, and how the reply parsed.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Answer only: no spinner, no banner.
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,
}

impl Cli {
    pub fn question(&self) -> String {
        self.question.join(" ")
    }
}

#[derive(Debug, Subcommand)]
pub enum Sub {
    /// Inspect and change ~/.augur/config.toml.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// List configured providers and whether their binaries are on PATH.
    Providers,
    /// Reprint the previous answer, without asking again.
    Last {
        /// Go straight to the run prompt.
        #[arg(long)]
        run: bool,
    },
    /// Print a shell completion script.
    Completions {
        #[arg(value_name = "SHELL")]
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print the path to the config file.
    Path,
    /// Print the config file.
    Show,
    /// Open the config file in $VISUAL, $EDITOR, or the platform default.
    Edit,
    /// Write a fresh commented config file.
    Init {
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Set a dotted key, preserving comments.
    ///
    /// `model`, `effort` and `provider` are accepted as shorthand for their
    /// fully-qualified paths.
    Set {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE")]
        value: String,
    },
    /// Print the value of a dotted key.
    Get {
        #[arg(value_name = "KEY")]
        key: String,
    },
}
