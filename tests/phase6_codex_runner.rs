use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use rusqlite::Connection;
use serde_json::Value;
use tempfile::TempDir;

fn rive_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rive"))
}

fn run_json(command: &mut Command) -> Value {
    let output = command.output().expect("command should run");
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout should be json")
}

fn run_json_with_stdin(command: &mut Command, stdin: &str) -> Value {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("command should spawn");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
    }
    let output = child.wait_with_output().expect("command should run");
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout should be json")
}

fn run_json_expect_error(command: &mut Command, stdin: Option<&str>) -> Value {
    let output = if let Some(stdin) = stdin {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("command should spawn");
        {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(stdin.as_bytes())
                .unwrap();
        }
        child.wait_with_output().expect("command should run")
    } else {
        command.output().expect("command should run")
    };
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout should be json")
}

fn init_workspace() -> TempDir {
    let temp = TempDir::new().unwrap();
    run_json(rive_cmd().arg("init").arg(temp.path()));
    temp
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn write_happy_codex(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
test "$1" = "exec"
printf '%s\n' "$*" > "$RIVE_WORKSPACE/codex-argv.txt"
test -n "$RIVE_WORKSPACE"
test -n "$RIVE_AGENT_ID"
test -n "$RIVE_AGENT_TOKEN"
test -n "$RIVE_RUN_ID"
test -n "$RIVE_DISPATCH_ID"
test -n "$CODEX_HOME"
printf 'RIVE_PHASE6_FAKE_OK\n' > "$RIVE_WORKSPACE/codex-result.txt"
printf '%s\n' "$CODEX_HOME" > "$RIVE_WORKSPACE/codex-home.txt"
if [ -f "$CODEX_HOME/config.toml" ]; then
  cp "$CODEX_HOME/config.toml" "$RIVE_WORKSPACE/codex-home-config.txt"
fi
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE/codex-result.txt" --label fake-codex-result --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'codex still working\n' | team status --dispatch "$RIVE_DISPATCH_ID" --snapshot "$SNAPSHOT_ID" --command-id "fake-codex-status-$RIVE_RUN_ID" --stdin >/dev/null
printf 'codex done\n' | team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "fake-codex-report-$RIVE_RUN_ID" --stdin >/dev/null
printf '{"hook_event_name":"SessionStart","session_id":"fake-codex-session","cwd":"%s"}' "$RIVE_WORKSPACE" | rive debug trace ingest --adapter codex-hook --agent "$RIVE_AGENT_ID" --run "$RIVE_RUN_ID" --dispatch "$RIVE_DISPATCH_ID" --stdin >/dev/null
printf '{"final":"RIVE_PHASE6_FAKE_OK"}\n'
"#,
    );
}

fn write_stdout_only_codex(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
printf '{"final":"I completed the task successfully"}\n'
"#,
    );
}

fn write_counting_codex(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
COUNT_FILE="$RIVE_WORKSPACE/codex-invocations.txt"
if [ -f "$COUNT_FILE" ]; then
  COUNT=$(cat "$COUNT_FILE")
else
  COUNT=0
fi
COUNT=$((COUNT + 1))
printf '%s\n' "$COUNT" > "$COUNT_FILE"
printf 'RIVE_PHASE6_REPLAY_OK\n' > "$RIVE_WORKSPACE/codex-replay-result.txt"
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE/codex-replay-result.txt" --label fake-codex-replay --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'done once\n' | team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "fake-codex-replay-report-$RIVE_RUN_ID" --stdin >/dev/null
printf '{"final":"RIVE_PHASE6_REPLAY_OK"}\n'
"#,
    );
}

fn runner_command(temp: &TempDir, codex_bin: &Path, command_id: &str) -> Command {
    runner_command_with_trust(temp, codex_bin, command_id, true)
}

fn runner_command_without_trust(temp: &TempDir, codex_bin: &Path, command_id: &str) -> Command {
    runner_command_with_trust(temp, codex_bin, command_id, false)
}

fn runner_command_with_trust(
    temp: &TempDir,
    codex_bin: &Path,
    command_id: &str,
    trust_project: bool,
) -> Command {
    let mut command = rive_cmd();
    command
        .current_dir(temp.path())
        .arg("runner")
        .arg("codex")
        .arg("--agent")
        .arg("codex-worker")
        .arg("--title")
        .arg("phase 6 fake task")
        .arg("--command-id")
        .arg(command_id)
        .arg("--codex-bin")
        .arg(codex_bin)
        .arg("--timeout-seconds")
        .arg("10")
        .arg("--snapshot-path")
        .arg("codex-result.txt")
        .arg("--stdin");
    if trust_project {
        command.arg("--trust-project");
    }
    command
}

fn runner_command_with_token(
    temp: &TempDir,
    codex_bin: &Path,
    command_id: &str,
    token: &str,
) -> Command {
    let mut command = runner_command(temp, codex_bin, command_id);
    command.arg("--agent-token").arg(token);
    command
}

#[test]
fn codex_isolated_home_preserves_top_level_model_config() {
    let temp = init_workspace();
    let home = TempDir::new().unwrap();
    let codex_home = home.path().join(".codex");
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(
        codex_home.join("config.toml"),
        r#"personality = "pragmatic"
model = "gpt-5.5"
model_reasoning_effort = "low"

[projects."/tmp/should-not-leak"]
trust_level = "trusted"
"#,
    )
    .unwrap();

    let fake = temp.path().join("fake-codex");
    write_happy_codex(&fake);
    let mut command = runner_command_without_trust(&temp, &fake, "codex-config-copy");
    command.env("HOME", home.path()).env_remove("CODEX_HOME");
    run_json_with_stdin(&mut command, "Create codex-result.txt and report done.\n");

    let config = fs::read_to_string(temp.path().join("codex-home-config.txt")).unwrap();
    assert!(config.contains(r#"model = "gpt-5.5""#));
    assert!(config.contains(r#"model_reasoning_effort = "low""#));
    assert!(config.contains(r#"personality = "pragmatic""#));
    assert!(!config.contains("[projects."));
    assert!(!config.contains("should-not-leak"));
}

#[test]
fn codex_runner_reports_dispatch_with_fake_codex() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-codex");
    write_happy_codex(&fake);

    let response = run_json_with_stdin(
        &mut runner_command(&temp, &fake, "codex-happy"),
        "Create codex-result.txt and report done.\n",
    );

    assert_eq!(response["protocol"]["runner"]["kind"], "codex");
    assert_eq!(response["protocol"]["runner"]["child_executed"], true);
    assert_eq!(response["protocol"]["runner"]["exit_code"], 0);
    assert_eq!(response["protocol"]["dispatch"]["state"], "reported");
    assert_eq!(
        response["protocol"]["dispatch"]["latest_report_status"],
        "done"
    );
    assert_eq!(response["protocol"]["trace"]["adapter"], "codex-hook");
    assert_eq!(response["protocol"]["trace"]["event_count"], 1);

    let argv = fs::read_to_string(temp.path().join("codex-argv.txt")).unwrap();
    assert!(argv.contains("--enable codex_hooks"));
    assert!(argv.contains("--dangerously-bypass-approvals-and-sandbox"));
    assert!(argv.contains("--skip-git-repo-check"));
    assert!(argv.contains("trust_level"));
    let codex_home = fs::read_to_string(temp.path().join("codex-home.txt")).unwrap();
    assert!(codex_home.contains(".rive/debug/runs/"));
    assert!(codex_home.contains("codex-home"));

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let facts: i64 = conn
        .query_row("select count(*) from facts", [], |row| row.get(0))
        .unwrap();
    let snapshots: i64 = conn
        .query_row("select count(*) from snapshots", [], |row| row.get(0))
        .unwrap();
    let work_graph_tables: i64 = conn
        .query_row(
            "select count(*) from sqlite_master where type='table' and name like 'work_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(facts, 2);
    assert_eq!(snapshots, 1);
    assert_eq!(work_graph_tables, 0);
}

#[test]
fn codex_trust_project_controls_trust_override_only() {
    let trusted = init_workspace();
    let trusted_fake = trusted.path().join("fake-codex");
    write_happy_codex(&trusted_fake);
    run_json_with_stdin(
        &mut runner_command(&trusted, &trusted_fake, "codex-trusted"),
        "Report done.\n",
    );
    let trusted_argv = fs::read_to_string(trusted.path().join("codex-argv.txt")).unwrap();
    assert!(trusted_argv.contains("trust_level"));

    let untrusted = init_workspace();
    let untrusted_fake = untrusted.path().join("fake-codex");
    write_happy_codex(&untrusted_fake);
    run_json_with_stdin(
        &mut runner_command_without_trust(&untrusted, &untrusted_fake, "codex-untrusted"),
        "Report done.\n",
    );
    let untrusted_argv = fs::read_to_string(untrusted.path().join("codex-argv.txt")).unwrap();
    assert!(!untrusted_argv.contains("trust_level"));
    let codex_home = fs::read_to_string(untrusted.path().join("codex-home.txt")).unwrap();
    assert!(codex_home.contains(".rive/debug/runs/"));
    assert!(codex_home.contains("codex-home"));
}

#[test]
fn codex_stdout_success_without_team_report_is_not_success() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-codex-stdout-only");
    write_stdout_only_codex(&fake);

    let error = run_json_expect_error(
        &mut runner_command(&temp, &fake, "codex-no-report"),
        Some("Pretend to finish without reporting.\n"),
    );
    assert_eq!(error["protocol"]["code"], "dispatch_not_reported");

    let dispatches = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("dispatch")
            .arg("list"),
    );
    assert_eq!(dispatches["protocol"]["dispatches"][0]["state"], "open");
}

#[test]
fn codex_replayed_runner_command_does_not_execute_child_again() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-codex-counting");
    write_counting_codex(&fake);
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("agent")
            .arg("add")
            .arg("codex-worker")
            .arg("--role")
            .arg("worker")
            .arg("--token")
            .arg("codex-token"),
    );

    let first = run_json_with_stdin(
        &mut runner_command_with_token(&temp, &fake, "codex-replay", "codex-token"),
        "Create codex-replay-result.txt and report done.\n",
    );
    assert_eq!(first["protocol"]["dispatch"]["state"], "reported");
    assert_eq!(first["protocol"]["runner"]["child_executed"], true);

    let second = run_json_with_stdin(
        &mut runner_command_with_token(&temp, &fake, "codex-replay", "codex-token"),
        "Create codex-replay-result.txt and report done.\n",
    );
    assert_eq!(
        second["protocol"]["dispatch"]["idempotency_status"],
        "replayed"
    );
    assert_eq!(second["protocol"]["runner"]["child_executed"], false);
    assert_eq!(second["protocol"]["runner"]["exit_code"], Value::Null);
    assert_eq!(
        fs::read_to_string(temp.path().join("codex-invocations.txt")).unwrap(),
        "1\n"
    );
}

#[test]
fn codex_existing_agent_requires_token_and_missing_binary_is_adapter_specific() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-codex");
    write_happy_codex(&fake);
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("agent")
            .arg("add")
            .arg("codex-worker")
            .arg("--role")
            .arg("worker")
            .arg("--token")
            .arg("codex-token"),
    );

    let missing_token = run_json_expect_error(
        &mut runner_command(&temp, &fake, "codex-token-missing"),
        Some("Should not run.\n"),
    );
    assert_eq!(
        missing_token["protocol"]["code"],
        "runner_agent_token_required"
    );

    let mut wrong_token = runner_command(&temp, &fake, "codex-token-wrong");
    wrong_token.arg("--agent-token").arg("wrong-token");
    let wrong = run_json_expect_error(&mut wrong_token, Some("Should not run.\n"));
    assert_eq!(wrong["protocol"]["code"], "agent_token_invalid");

    let missing_bin_workspace = init_workspace();
    let missing_bin = missing_bin_workspace.path().join("missing-codex");
    let missing = run_json_expect_error(
        &mut runner_command(&missing_bin_workspace, &missing_bin, "codex-missing-bin"),
        Some("Should fail before child launch.\n"),
    );
    assert_eq!(missing["protocol"]["code"], "codex_not_found");
}
