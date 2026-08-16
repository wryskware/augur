# augur

Ask your terminal a question. Get an answer, or a runnable command.

```console
$ ask list all the files under this directory recursively

  PowerShell has no `find`; Get-ChildItem is the equivalent, and -File keeps
  directories out of the listing.

  ❯ Get-ChildItem -File -Recurse

  [enter] run   [e] edit   [c] copy   [n] dismiss
```

The binary is `ask`. The project is `augur`.

## Why

You know what you want. You do not know the local syntax — because you switched
shells, or platforms, or because the flag is just unmemorable. `augur` closes
that gap without a browser tab, and then offers to run the thing.

It knows which shell you are actually standing in, so it will not hand PowerShell
a `find` invocation.

## Install

### Prerequisites

- **Rust 1.85+** (2024 edition). `rustup` from [rustup.rs](https://rustup.rs) if
  you don't have it.
- **At least one provider CLI, already logged in.** augur never talks to a model
  vendor directly and holds no API keys — it drives a tool you have already
  authenticated. Any one of these is enough:

  | Provider  | Install                          | Auth            | Response schema |
  | --------- | -------------------------------- | --------------- | --------------- |
  | `codex`   | `npm i -g @openai/codex`         | `codex login`   | enforced        |
  | `claude`  | [claude.com/code](https://claude.com/code) | `claude` once | asked for |
  | `command` | any binary you can run           | its own         | asked for       |

  `codex` is the default and the only one whose reply shape can be *enforced*
  rather than requested, so it degrades least. Anything else — `ollama`,
  `gemini`, a shell script — works through the `command` provider; see
  [Configuration](#configuration).

### From GitHub

```console
cargo install --git https://github.com/wryskware/augur
```

### From a clone

```console
git clone https://github.com/wryskware/augur
cd augur
cargo install --path .
```

Both install a binary named **`ask`** into `~/.cargo/bin`. If that isn't on your
PATH, add it:

```console
# bash / zsh — in ~/.bashrc or ~/.zshrc
export PATH="$HOME/.cargo/bin:$PATH"
```

```powershell
# PowerShell — in $PROFILE
$env:PATH = "$HOME\.cargo\bin;$env:PATH"
```

### Verify

```console
ask providers          # which provider CLIs are actually reachable
ask "say hello"        # writes ~/.augur/config.toml on first run
```

The first run creates a fully commented `~/.augur/config.toml`. Point it at the
model you want:

```console
ask config set model gpt-5.6-luna
ask config set effort high
ask config set provider codex
```

### Shell completions

```console
# bash
ask completions bash > ~/.local/share/bash-completion/completions/ask

# zsh   (somewhere on your $fpath)
ask completions zsh > ~/.zfunc/_ask

# fish
ask completions fish > ~/.config/fish/completions/ask.fish

# PowerShell — append to $PROFILE
ask completions powershell | Out-String | Invoke-Expression
```

### Uninstall

```console
cargo uninstall augur
rm -rf ~/.augur          # config and history
```

## Use

```console
ask how do I undo the last commit but keep the changes
ask "what's the difference between --force and --force-with-lease"
cargo build 2>&1 | ask "what's wrong"
ask -e low "what port does postgres use"
ask --profile quick "pretty-print this json file"
```

Quotes are optional. If a question starts with a dash, or you want a flag after
it, use `--`:

```console
ask -- --force-with-lease, what does it actually protect against
```

### Flags

| Flag | Effect |
| --- | --- |
| `-m, --model <MODEL>` | override the configured model |
| `-e, --effort <LEVEL>` | `minimal`, `low`, `medium`, `high`, `xhigh` |
| `-p, --provider <NAME>` | route through a different provider |
| `--profile <NAME>` | apply a named provider/model/effort bundle |
| `-y, --yes` | run the command without asking (never a destructive one) |
| `-n, --no-run` | print only |
| `-c, --copy` | copy to clipboard instead of running |
| `--json` | emit the structured response; scriptable |
| `--raw` | print the provider's reply verbatim |
| `--no-context` | omit the os/shell/cwd block from the prompt |
| `-v, --verbose` | show the provider command line, timing, and parse tier |
| `-q, --quiet` | answer only |

### Subcommands

```console
ask config path | show | edit | init
ask config set <key> <value>     # ask config set model gpt-5.6-luna
ask config get <key>
ask providers
ask last [--run]
ask completions <bash|zsh|fish|powershell>
```

## Configuration

`~/.augur/config.toml`, created on first run. `$AUGUR_CONFIG` overrides the
path; `$AUGUR_HOME` overrides the directory.

Precedence, highest first:

```
CLI flag  >  --profile  >  AUGUR_MODEL / AUGUR_EFFORT / AUGUR_PROVIDER  >  config file  >  builtin
```

`ask config set` edits the file surgically — comments and layout survive — and
refuses any value that would leave the config invalid.

Adding a provider is a config change, not a code change:

```toml
[providers.ollama]
kind            = "command"
command         = "ollama"
args            = ["run", "{model}", "{prompt}"]
supports_schema = false
```

Placeholders: `{model}`, `{effort}`, `{prompt}`, `{schema_file}`. An argument
that reduces to nothing but an unset placeholder is dropped. Omit `{prompt}` and
the prompt is written to the process's stdin instead.

## Safety

A command is gated when the model flags it *or* when it matches a regex in
`[safety].deny`. The local list can only escalate the model's verdict, never
soften it — so a model that forgets to flag `git reset --hard` still gets caught.

Gated commands require a typed confirmation. `--yes` does not bypass this, by
design.

The patterns are anchored to avoid crying wolf: `rm -rf /` is caught,
`rm -rf ./build` is not.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | answered, and nothing was run |
| *n* | a command ran and exited with *n* |
| 2 | augur itself failed (bad config, provider missing, provider errored) |

## Limitations

- With input piped in, there is no terminal left to read a keypress from, so the
  answer prints but the run prompt is skipped. Use `--yes` or `--copy`.
- The run menu selects among up to nine commands, since it reads one digit.
- `codex` runs in a `read-only` sandbox: it may inspect files to ground an
  answer, never modify them. Change it under `[providers.codex] sandbox`.

## License

MIT.
