//! augur — `ask` a question at the shell prompt.

mod cli;
mod config;
mod context;
mod exec;
mod history;
mod interact;
mod prompt;
mod provider;
mod render;
mod response;
mod safety;

use std::io::Write;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser};

use crate::cli::{Cli, ConfigAction, Sub};
use crate::render::Theme;
use crate::response::ParseTier;

/// Reserved for augur's own failures, so a passed-through command exit code is
/// mostly unambiguous.
const EXIT_AUGUR_ERROR: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            let theme = Theme::detect(false);
            eprintln!("{} {}", theme.red("ask:"), err);
            for cause in err.chain().skip(1) {
                eprintln!("  {}", theme.dim(&format!("↳ {cause}")));
            }
            ExitCode::from(EXIT_AUGUR_ERROR)
        }
    }
}

fn run(cli: &Cli) -> Result<u8> {
    match &cli.command {
        Some(sub) => subcommand(cli, sub),
        None => ask(cli),
    }
}

// ---------------------------------------------------------------------------
// The main path
// ---------------------------------------------------------------------------

fn ask(cli: &Cli) -> Result<u8> {
    let question = cli.question();
    if question.trim().is_empty() {
        Cli::command().print_help()?;
        println!();
        return Ok(EXIT_AUGUR_ERROR);
    }

    let (cfg, cfg_path, created) = config::Config::load_or_init()?;
    let plain = cli.json || cli.raw;
    let theme = if plain {
        Theme::plain()
    } else {
        Theme::detect(cfg.ui.markdown)
    };

    if created && !cli.quiet && !plain {
        eprintln!(
            "{}",
            render::warn(&theme, &format!("created {}", cfg_path.display()))
        );
    }

    let overrides = config::Overrides {
        provider: cli.provider.clone(),
        model: cli.model.clone(),
        effort: cli.effort,
        profile: cli.profile.clone(),
    };
    let settings = config::resolve(&cfg, &overrides, &config::EnvOverrides::from_env())?;
    let guard = safety::Guard::new(&cfg.safety)?;
    let ctx = context::EnvContext::detect(&cfg.shell);

    let prompt_text = prompt::build(
        &cfg.prompt,
        &ctx,
        &question,
        !cli.no_context,
        !settings.provider.supports_schema,
    )?;

    let request = provider::Request {
        prompt: &prompt_text,
        model: &settings.model,
        effort: settings.effort,
        schema: settings
            .provider
            .supports_schema
            .then(response::json_schema),
    };
    let invocation = provider::build_invocation(&settings.provider, &request)?;

    if cli.verbose {
        eprintln!("{}", theme.dim(&format!("  $ {}", invocation.display())));
    }

    let mut spinner = if cfg.ui.spinner && !cli.quiet && !plain {
        render::Spinner::start(
            format!(
                "{} · {} · {}",
                settings.provider.name, settings.model, settings.effort
            ),
            &theme,
        )
    } else {
        render::Spinner::disabled()
    };
    let reply = provider::execute(&invocation, &ctx.cwd);
    spinner.finish();
    let elapsed = spinner.elapsed();
    let reply = reply?;

    if cli.verbose && !reply.stderr.trim().is_empty() {
        eprintln!("{}", theme.dim("  ── provider stderr ──"));
        for line in reply
            .stderr
            .lines()
            .rev()
            .take(10)
            .collect::<Vec<_>>()
            .iter()
            .rev()
        {
            eprintln!("{}", theme.dim(&format!("  {line}")));
        }
    }

    if cli.raw {
        print!("{}", reply.text);
        if !reply.text.ends_with('\n') {
            println!();
        }
        return Ok(0);
    }

    let (answer, tier) = response::parse(&reply.text);

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&answer)?);
        return Ok(0);
    }

    if !cli.quiet {
        eprintln!(
            "{}",
            render::banner(
                &theme,
                &settings.provider.name,
                &settings.model,
                settings.effort.as_str(),
                elapsed.as_secs_f32(),
            )
        );
    }
    if cli.verbose && tier != ParseTier::Strict {
        let label = match tier {
            ParseTier::Strict => "strict",
            ParseTier::Extracted => "extracted from surrounding text",
            ParseTier::Prose => "not JSON at all; treated as prose",
        };
        eprintln!(
            "{}",
            render::warn(&theme, &format!("reply parsed: {label}"))
        );
    }

    print_answer(&theme, &answer, &guard);

    if cfg.ui.history {
        let entry = history::Entry::now(
            &question,
            &settings.provider.name,
            &settings.model,
            settings.effort.as_str(),
            ctx.shell.name(),
            &ctx.cwd.to_string_lossy(),
            &answer,
        );
        if let Err(err) = history::append(&entry, cfg.ui.history_limit)
            && cli.verbose
        {
            eprintln!(
                "{}",
                render::warn(&theme, &format!("history not written: {err}"))
            );
        }
    }

    let outcome = interact::offer(
        &theme,
        &answer,
        ctx.shell,
        &guard,
        interact::Options {
            yes: cli.yes,
            no_run: cli.no_run,
            copy: cli.copy,
        },
    )?;

    Ok(match outcome {
        interact::Outcome::Ran(code) => u8::try_from(code).unwrap_or(1),
        _ => 0,
    })
}

fn print_answer(theme: &Theme, answer: &response::Answer, guard: &safety::Guard) {
    let body = render::answer_body(theme, answer);
    if !body.is_empty() {
        println!();
        println!("{body}");
    }

    if !answer.commands.is_empty() {
        println!();
        let numbered = answer.commands.len() > 1;
        for (index, suggestion) in answer.commands.iter().enumerate() {
            let risk = guard.assess(suggestion);
            println!(
                "{}",
                render::command_block(theme, suggestion, &risk, numbered.then_some(index + 1))
            );
        }
    }

    if let Some(snippet) = &answer.snippet {
        println!();
        println!("{}", render::snippet_block(theme, snippet));
    }
    let _ = std::io::stdout().flush();
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

fn subcommand(cli: &Cli, sub: &Sub) -> Result<u8> {
    match sub {
        Sub::Config { action } => config_command(action),
        Sub::Providers => providers_command(),
        Sub::Last { run } => last_command(cli, *run),
        Sub::Completions { shell } => {
            let mut command = Cli::command();
            let name = command.get_name().to_string();
            clap_complete::generate(*shell, &mut command, name, &mut std::io::stdout());
            Ok(0)
        }
    }
}

fn config_command(action: &ConfigAction) -> Result<u8> {
    let path = config::config_path()?;
    let theme = Theme::detect(false);

    match action {
        ConfigAction::Path => {
            println!("{}", path.display());
            Ok(0)
        }
        ConfigAction::Show => {
            if !path.exists() {
                eprintln!(
                    "{}",
                    render::warn(&theme, &format!("no config yet at {}", path.display()))
                );
                eprintln!("{}", theme.dim("  run `ask config init` to create one"));
                return Ok(1);
            }
            print!("{}", std::fs::read_to_string(&path)?);
            Ok(0)
        }
        ConfigAction::Init { force } => {
            if path.exists() && !force {
                bail!(
                    "{} already exists; pass --force to overwrite it",
                    path.display()
                );
            }
            config::write_starter_config(&path)?;
            println!("{} {}", theme.green("wrote"), path.display());
            Ok(0)
        }
        ConfigAction::Edit => {
            if !path.exists() {
                config::write_starter_config(&path)?;
            }
            let editor = std::env::var("VISUAL")
                .or_else(|_| std::env::var("EDITOR"))
                .unwrap_or_else(|_| {
                    if cfg!(windows) {
                        "notepad".to_string()
                    } else {
                        "vi".to_string()
                    }
                });
            // The editor string may carry flags, e.g. `code --wait`.
            let parts = shell_words::split(&editor)
                .with_context(|| format!("could not parse $EDITOR ({editor})"))?;
            let (program, args) = parts.split_first().context("$EDITOR is set but empty")?;
            let status = std::process::Command::new(program)
                .args(args)
                .arg(&path)
                .status()
                .with_context(|| format!("launching {program}"))?;
            Ok(u8::try_from(status.code().unwrap_or(0)).unwrap_or(1))
        }
        ConfigAction::Set { key, value } => {
            let existing = if path.exists() {
                std::fs::read_to_string(&path)?
            } else {
                config::STARTER_CONFIG.to_string()
            };
            let updated = config::set_key(&existing, key, value)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, updated)?;
            println!(
                "{} {} = {}",
                theme.green("set"),
                config::expand_key(key),
                theme.bold(value)
            );
            Ok(0)
        }
        ConfigAction::Get { key } => {
            let text = if path.exists() {
                std::fs::read_to_string(&path)?
            } else {
                config::STARTER_CONFIG.to_string()
            };
            match config::get_key(&text, key)? {
                Some(value) => {
                    println!("{value}");
                    Ok(0)
                }
                None => {
                    eprintln!(
                        "{}",
                        render::warn(&theme, &format!("`{}` is not set", config::expand_key(key)))
                    );
                    Ok(1)
                }
            }
        }
    }
}

fn providers_command() -> Result<u8> {
    let (cfg, _) = config::Config::load_lenient()?;
    let theme = Theme::detect(false);
    let settings = config::resolve(
        &cfg,
        &config::Overrides::default(),
        &config::EnvOverrides::from_env(),
    );
    let active = settings.as_ref().ok().map(|s| s.provider.name.clone());

    for name in cfg.provider_names() {
        let resolved = match cfg.resolve_provider(&name) {
            Ok(resolved) => resolved,
            Err(err) => {
                println!("  {}  {}", theme.red(&name), theme.dim(&err.to_string()));
                continue;
            }
        };
        let marker = if active.as_deref() == Some(name.as_str()) {
            "*"
        } else {
            " "
        };
        let (status, detail) = match provider::locate(&resolved) {
            Some(path) => (theme.green("found"), path.display().to_string()),
            None => (
                theme.red("missing"),
                format!("`{}` is not on PATH", resolved.command),
            ),
        };
        let schema = if resolved.supports_schema {
            "schema enforced"
        } else {
            "schema by prompt"
        };
        println!(
            "{} {:<10} {}  {}",
            theme.cyan(marker),
            theme.bold(&name),
            status,
            theme.dim(&format!("{detail} · {schema}"))
        );
    }

    if let Err(err) = settings {
        eprintln!("{}", render::warn(&theme, &err.to_string()));
        return Ok(1);
    }
    Ok(0)
}

fn last_command(cli: &Cli, run: bool) -> Result<u8> {
    let (cfg, _) = config::Config::load_lenient()?;
    let theme = if cli.json {
        Theme::plain()
    } else {
        Theme::detect(cfg.ui.markdown)
    };

    let Some(entry) = history::last()? else {
        eprintln!("{}", render::warn(&theme, "no history yet"));
        return Ok(1);
    };

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&entry)?);
        return Ok(0);
    }

    let guard = safety::Guard::new(&cfg.safety)?;
    if !cli.quiet {
        eprintln!(
            "{}",
            theme.dim(&format!("  {} · {}", entry.question, entry.model))
        );
    }
    print_answer(&theme, &entry.answer, &guard);

    if !run {
        return Ok(0);
    }
    let shell = context::Shell::parse(&entry.shell)
        .unwrap_or_else(|| context::EnvContext::detect(&cfg.shell).shell);
    let outcome = interact::offer(
        &theme,
        &entry.answer,
        shell,
        &guard,
        interact::Options {
            yes: cli.yes,
            no_run: false,
            copy: cli.copy,
        },
    )?;
    Ok(match outcome {
        interact::Outcome::Ran(code) => u8::try_from(code).unwrap_or(1),
        _ => 0,
    })
}
