# augur — agent orientation

A Rust CLI. Binary name is **`ask`**, crate/repo name is **`augur`**. Standalone
repo inside the `wryskware/` workspace container; it is not a workspace member
and must never gain one.

Read `README.md` first for what the tool does. This file is about how it is
built.

## Commands

```console
cargo build
cargo test                          # 31 unit + 25 integration
cargo clippy --all-targets -- -D warnings
cargo fmt
./target/debug/ask -v "..."         # -v prints the provider argv and parse tier
```

Tests never hit the network. `tests/cli.rs` writes a two-line `.cmd`/`.sh`
fixture that echoes a canned reply and points a `kind = "command"` provider at
it, so the full pipeline runs offline.

## Layout

| File | Owns |
| --- | --- |
| `cli.rs` | clap types only; no logic |
| `config.rs` | config file shape, builtin providers, precedence, TOML editing |
| `context.rs` | OS string, shell detection, cwd, piped stdin |
| `prompt.rs` | the preamble and how the prompt is assembled |
| `response.rs` | the JSON schema and the three-tier parser |
| `provider.rs` | adapters (codex / claude / command) + subprocess execution |
| `safety.rs` | destructive-command detection |
| `render.rs` | spinner, markdown, command blocks, all styling |
| `interact.rs` | the keypress menu |
| `exec.rs` | running the chosen command in the detected shell |
| `history.rs` | `~/.augur/history.jsonl`, backing `ask last` |

## Invariants

1. **augur speaks no HTTP to a model vendor.** Every provider is a subprocess.
   There are no API keys in the config and there should never be. An HTTP
   provider, if it ever lands, is a new `ProviderKind` arm in
   `provider::build_invocation` plus a branch in `provider::execute` — not a new
   auth story.
2. **The safety guard escalates only.** `safety::Guard::assess` may turn the
   model's `destructive: false` into true; nothing may turn true into false.
   `--yes` never bypasses a destructive gate.
3. **Parsing never hard-fails.** `response::parse` returns three tiers and the
   last one always succeeds. A provider that ignores the schema still produces a
   usable answer.
4. **Chrome on stderr, content on stdout.** Spinner, banner, key hints and
   warnings go to stderr; the answer, commands and `--json` go to stdout, so
   `ask ... > file` and `| jq` both work. Color is decided by stdout being a TTY.
5. **Deny-list patterns must be anchored.** Every addition to
   `config::default_deny_patterns` needs a matching false-positive test in
   `safety.rs`. A guard that fires on `rm -rf ./build` gets trained away and then
   protects nothing.
6. `deny_unknown_fields` is on every config struct on purpose — a typo in a
   hand-edited config should be an error, not a silent default.

## Platform notes

- Rust's `Command` only ever appends `.exe`, so npm shims (`codex.cmd`) are
  invisible to it. `provider::execute` resolves through `which` first. Do not
  remove that.
- Windows delivers key-*release* events as well as press; `interact.rs` filters
  on `KeyEventKind::Press` or every keystroke registers twice.
- The `regex` crate has no lookaround. Deny patterns must be written without it.
- `codex exec` has no `-a/--ask-for-approval` flag — that is on the interactive
  command. The policy goes through `-c approval_policy="never"`.

## Prompt changes

`prompt::BUILTIN_PREAMBLE` and the `description` strings in
`response::json_schema` are both live prompt surface — the schema descriptions
do at least as much steering as the preamble does. Change either and re-run a
real query; the unit tests only check that the text is present, not that it
works.
