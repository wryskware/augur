//! End-to-end tests over the real `ask` binary.
//!
//! Every one of these routes through a fixture "provider" — a two-line script
//! that prints a canned reply — so the whole pipeline (config → prompt → argv →
//! parse → render → safety → run) is exercised without a network call or an
//! API key.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::PredicateStrExt;
use predicates::str::contains;

/// Marker printed by the one command the tests actually execute.
const RAN_MARKER: &str = "augur-really-ran-this";

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new(payload: &str) -> Sandbox {
        Sandbox::with_exit(payload, 0)
    }

    fn with_exit(payload: &str, exit_code: i32) -> Sandbox {
        let dir = tempfile::Builder::new()
            .prefix("augur-test-")
            .tempdir()
            .unwrap();
        let script = write_fixture(dir.path(), payload, exit_code);
        write_config(dir.path(), &script);
        Sandbox { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn config(&self) -> PathBuf {
        self.dir.path().join("config.toml")
    }

    /// An `ask` invocation wired to this sandbox. stdin is an empty pipe, which
    /// is what a non-interactive caller looks like.
    fn ask(&self) -> Command {
        let mut cmd = Command::cargo_bin("ask").unwrap();
        cmd.env("AUGUR_HOME", self.path())
            .env("AUGUR_CONFIG", self.config())
            .env("NO_COLOR", "1")
            .env_remove("AUGUR_MODEL")
            .env_remove("AUGUR_EFFORT")
            .env_remove("AUGUR_PROVIDER")
            .current_dir(self.path())
            .write_stdin("");
        cmd
    }
}

#[cfg(windows)]
fn write_fixture(dir: &Path, payload: &str, exit_code: i32) -> PathBuf {
    std::fs::write(dir.join("payload.txt"), payload).unwrap();
    let script = dir.join("fixture.cmd");
    std::fs::write(
        &script,
        format!("@echo off\r\ntype \"%~dp0payload.txt\"\r\nexit /b {exit_code}\r\n"),
    )
    .unwrap();
    script
}

#[cfg(not(windows))]
fn write_fixture(dir: &Path, payload: &str, exit_code: i32) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(dir.join("payload.txt"), payload).unwrap();
    let script = dir.join("fixture.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ncat \"$(dirname \"$0\")/payload.txt\"\nexit {exit_code}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn write_config(dir: &Path, script: &Path) {
    let shell = if cfg!(windows) { "powershell" } else { "sh" };
    let command = script.display().to_string().replace('\\', "\\\\");
    // The fixture ignores --model/--effort; they are here so that `-v` exposes
    // what resolution actually decided.
    let config = format!(
        r#"# a comment that must survive `config set`
provider = "fixture"

[defaults]
model  = "config-model"
effort = "low"

[ui]
spinner  = false
history  = false
markdown = false

[shell]
kind = "{shell}"

[providers.fixture]
kind            = "command"
command         = "{command}"
args            = ["--model", "{{model}}", "--effort", "{{effort}}"]
supports_schema = false

[profiles.p]
model  = "profile-model"
effort = "medium"
"#
    );
    std::fs::write(dir.join("config.toml"), config).unwrap();
}

fn json_reply(command: &str, destructive: bool) -> String {
    format!(
        r#"{{"answer":"Prose about it.","commands":[{{"command":"{command}","shell":"sh","note":"","destructive":{destructive}}}],"snippet":null}}"#
    )
}

// ---------------------------------------------------------------------------
// Parsing and output modes
// ---------------------------------------------------------------------------

#[test]
fn json_mode_emits_the_structured_answer_and_nothing_else() {
    let sandbox = Sandbox::new(&json_reply("ls -la", false));
    let out = sandbox
        .ask()
        .args(["--json", "list files"])
        .assert()
        .success();

    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout must be pure JSON");
    assert_eq!(parsed["commands"][0]["command"], "ls -la");
    assert_eq!(parsed["answer"], "Prose about it.");
}

#[test]
fn a_prose_reply_still_yields_a_runnable_command() {
    // No JSON anywhere — the prose tier has to rescue the fenced one-liner.
    let payload = "PowerShell has no `find`.\n\n```powershell\nGet-ChildItem -Recurse -File\n```\n";
    let sandbox = Sandbox::new(payload);
    let out = sandbox
        .ask()
        .args(["--json", "list files"])
        .assert()
        .success();

    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        parsed["answer"]
            .as_str()
            .unwrap()
            .contains("PowerShell has no")
    );
    assert_eq!(
        parsed["commands"][0]["command"],
        "Get-ChildItem -Recurse -File"
    );
}

#[test]
fn a_multiline_script_is_not_offered_as_a_command() {
    let payload = "Do this:\n\n```bash\ncd /tmp\nls -la\n```\n";
    let sandbox = Sandbox::new(payload);
    let out = sandbox.ask().args(["--json", "q"]).assert().success();

    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["commands"].as_array().unwrap().len(), 0);
}

#[test]
fn raw_mode_passes_the_provider_output_through_untouched() {
    let sandbox = Sandbox::new("literally anything at all");
    sandbox
        .ask()
        .args(["--raw", "q"])
        .assert()
        .success()
        .stdout(contains("literally anything at all"));
}

#[test]
fn rendered_output_shows_the_prose_and_the_command() {
    let sandbox = Sandbox::new(&json_reply("ls -la", false));
    sandbox
        .ask()
        .args(["--no-run", "q"])
        .assert()
        .success()
        .stdout(contains("Prose about it."))
        .stdout(contains("❯ ls -la"));
}

// ---------------------------------------------------------------------------
// Running, and refusing to run
// ---------------------------------------------------------------------------

#[test]
fn yes_runs_a_safe_command() {
    let sandbox = Sandbox::new(&json_reply(&format!("echo {RAN_MARKER}"), false));
    sandbox
        .ask()
        .args(["--yes", "q"])
        .assert()
        .success()
        .stdout(contains(RAN_MARKER));
}

#[test]
fn yes_refuses_a_command_the_model_flagged_destructive() {
    let sandbox = Sandbox::new(&json_reply(&format!("echo {RAN_MARKER}"), true));
    let out = sandbox.ask().args(["--yes", "q"]).assert().success();

    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    // Once for the displayed command block; a second occurrence would be the
    // program's own output, i.e. it ran.
    assert_eq!(
        stdout.matches(RAN_MARKER).count(),
        1,
        "a destructive command must not run under --yes"
    );
    out.stderr(contains("does not auto-run destructive"));
}

#[test]
fn yes_refuses_a_command_the_deny_list_caught_even_when_the_model_said_it_was_safe() {
    // The model claims destructive:false; the local guard must overrule it.
    let sandbox = Sandbox::new(&json_reply("git reset --hard", false));
    sandbox
        .ask()
        .args(["--yes", "q"])
        .assert()
        .success()
        .stderr(contains("does not auto-run destructive"));
}

#[test]
fn no_run_never_executes_anything() {
    let sandbox = Sandbox::new(&json_reply(&format!("echo {RAN_MARKER}"), false));
    let out = sandbox.ask().args(["--no-run", "q"]).assert().success();

    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    // The command is *shown*, so the marker appears once — but not a second
    // time as program output.
    assert_eq!(stdout.matches(RAN_MARKER).count(), 1);
}

// ---------------------------------------------------------------------------
// Settings precedence
// ---------------------------------------------------------------------------

/// The fixture's argv is echoed by `-v`, which is how these assert what won.
fn resolved_argv(sandbox: &Sandbox, extra: &[&str], env: &[(&str, &str)]) -> String {
    let mut cmd = sandbox.ask();
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.arg("-v");
    cmd.args(extra);
    cmd.arg("q");
    let out = cmd.assert().success();
    String::from_utf8(out.get_output().stderr.clone()).unwrap()
}

#[test]
fn config_file_supplies_the_default_model_and_effort() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    let argv = resolved_argv(&sandbox, &[], &[]);
    assert!(argv.contains("config-model"), "{argv}");
    assert!(argv.contains("--effort low"), "{argv}");
}

#[test]
fn env_beats_the_config_file() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    let argv = resolved_argv(&sandbox, &[], &[("AUGUR_MODEL", "env-model")]);
    assert!(argv.contains("env-model"), "{argv}");
}

#[test]
fn a_profile_beats_env() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    let argv = resolved_argv(
        &sandbox,
        &["--profile", "p"],
        &[("AUGUR_MODEL", "env-model")],
    );
    assert!(argv.contains("profile-model"), "{argv}");
    assert!(argv.contains("--effort medium"), "{argv}");
}

#[test]
fn a_flag_beats_everything() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    let argv = resolved_argv(
        &sandbox,
        &["--profile", "p", "-m", "flag-model", "-e", "xhigh"],
        &[("AUGUR_MODEL", "env-model")],
    );
    assert!(argv.contains("flag-model"), "{argv}");
    assert!(argv.contains("--effort xhigh"), "{argv}");
}

// ---------------------------------------------------------------------------
// Failure modes
// ---------------------------------------------------------------------------

#[test]
fn a_failing_provider_reports_its_stderr_and_exits_two() {
    let sandbox = Sandbox::with_exit("", 3);
    sandbox
        .ask()
        .args(["q"])
        .assert()
        .code(2)
        .stderr(contains("exit code 3"));
}

#[test]
fn an_unknown_provider_names_the_ones_that_do_exist() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    sandbox
        .ask()
        .args(["-p", "nope", "q"])
        .assert()
        .code(2)
        .stderr(contains("unknown provider"))
        .stderr(contains("fixture"));
}

#[test]
fn an_unknown_profile_names_the_ones_that_do_exist() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    sandbox
        .ask()
        .args(["--profile", "nope", "q"])
        .assert()
        .code(2)
        .stderr(contains("unknown profile"));
}

#[test]
fn a_bad_deny_regex_is_reported_rather_than_panicking() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    let config =
        std::fs::read_to_string(sandbox.config()).unwrap() + "\n[safety]\ndeny = [\"(unclosed\"]\n";
    std::fs::write(sandbox.config(), config).unwrap();

    sandbox
        .ask()
        .args(["q"])
        .assert()
        .code(2)
        .stderr(contains("[safety].deny"));
}

#[test]
fn an_unknown_config_key_is_a_clear_error_not_a_silent_ignore() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    let config = std::fs::read_to_string(sandbox.config())
        .unwrap()
        .replace("spinner", "spinnner");
    std::fs::write(sandbox.config(), config).unwrap();

    sandbox
        .ask()
        .args(["q"])
        .assert()
        .code(2)
        .stderr(contains("spinnner"));
}

#[test]
fn no_question_prints_help_rather_than_calling_a_provider() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    sandbox
        .ask()
        .assert()
        .code(2)
        .stdout(contains("Ask your terminal a question"));
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

#[test]
fn subcommands_win_over_being_read_as_a_question() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    sandbox
        .ask()
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(contains("config.toml"));
}

#[test]
fn config_set_preserves_comments_and_config_get_returns_it_unquoted() {
    let sandbox = Sandbox::new(&json_reply("ls", false));

    sandbox
        .ask()
        .args(["config", "set", "model", "changed-model"])
        .assert()
        .success();

    let text = std::fs::read_to_string(sandbox.config()).unwrap();
    assert!(
        text.contains("# a comment that must survive"),
        "comments were lost"
    );

    sandbox
        .ask()
        .args(["config", "get", "model"])
        .assert()
        .success()
        // Unquoted, so it can be used directly in a shell substitution.
        .stdout(predicates::str::diff("changed-model\n").from_utf8());
}

#[test]
fn config_set_rejects_a_value_that_would_break_the_config() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    sandbox
        .ask()
        .args(["config", "set", "effort", "supersonic"])
        .assert()
        .code(2);

    // And left the file alone.
    let text = std::fs::read_to_string(sandbox.config()).unwrap();
    assert!(text.contains("effort = \"low\""));
}

#[test]
fn providers_lists_the_configured_ones_and_marks_the_active_one() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    sandbox
        .ask()
        .args(["providers"])
        .assert()
        .success()
        .stdout(contains("fixture"))
        .stdout(contains("codex"));
}

#[test]
fn last_replays_the_previous_answer_without_asking_again() {
    let sandbox = Sandbox::new(&json_reply("ls -la", false));
    // History is off in the base config; turn it on for this one.
    let config = std::fs::read_to_string(sandbox.config())
        .unwrap()
        .replace("history  = false", "history  = true");
    std::fs::write(sandbox.config(), config).unwrap();

    sandbox
        .ask()
        .args(["--no-run", "remember me"])
        .assert()
        .success();
    sandbox
        .ask()
        .args(["last"])
        .assert()
        .success()
        .stdout(contains("❯ ls -la"))
        .stderr(contains("remember me"));
}

#[test]
fn completions_are_generated_for_each_supported_shell() {
    let sandbox = Sandbox::new(&json_reply("ls", false));
    for shell in ["bash", "zsh", "fish", "powershell"] {
        sandbox
            .ask()
            .args(["completions", shell])
            .assert()
            .success()
            .stdout(contains("ask"));
    }
}
