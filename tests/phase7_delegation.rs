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

fn team_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_team"))
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

fn add_agent(temp: &TempDir, name: &str, role: &str, token: &str) -> Value {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("agent")
            .arg("add")
            .arg(name)
            .arg("--role")
            .arg(role)
            .arg("--token")
            .arg(token),
    )
}

fn write_happy_worker(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
test -n "$RIVE_WORKSPACE"
test -n "$RIVE_AGENT_ID"
test -n "$RIVE_AGENT_TOKEN"
test -n "$RIVE_RUN_ID"
test -n "$RIVE_DISPATCH_ID"
printf 'RIVE_PHASE7_FAKE_OK\n' > "$RIVE_WORKSPACE/phase7-result.txt"
printf '%s\n' "$RIVE_AGENT_TOKEN" > "$RIVE_WORKSPACE/worker-token-leak-check.txt"
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE/phase7-result.txt" --label phase7-fake-worker --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'delegated worker still running\n' | team status --dispatch "$RIVE_DISPATCH_ID" --snapshot "$SNAPSHOT_ID" --command-id "phase7-status-$RIVE_RUN_ID" --stdin >/dev/null
printf 'delegated worker done\n' | team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "phase7-report-$RIVE_RUN_ID" --stdin >/dev/null
printf '{"type":"message.part.updated","properties":{"sessionID":"phase7-fake-session","messageID":"msg_1","part":{"type":"text","text":"debug only"}}}' | rive debug trace ingest --adapter opencode-plugin --agent "$RIVE_AGENT_ID" --run "$RIVE_RUN_ID" --dispatch "$RIVE_DISPATCH_ID" --stdin >/dev/null
printf '{"final":"RIVE_PHASE7_FAKE_OK"}\n'
"#,
    );
}

fn write_stdout_only_worker(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
printf '{"final":"I finished, but I did not call team report"}\n'
"#,
    );
}

fn write_counting_worker(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
COUNT_FILE="$RIVE_WORKSPACE/phase7-invocations.txt"
if [ -f "$COUNT_FILE" ]; then
  COUNT=$(cat "$COUNT_FILE")
else
  COUNT=0
fi
COUNT=$((COUNT + 1))
printf '%s\n' "$COUNT" > "$COUNT_FILE"
printf 'RIVE_PHASE7_REPLAY_OK\n' > "$RIVE_WORKSPACE/phase7-replay.txt"
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE/phase7-replay.txt" --label phase7-replay --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'done once\n' | team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "phase7-replay-report-$RIVE_RUN_ID" --stdin >/dev/null
printf '{"final":"RIVE_PHASE7_REPLAY_OK"}\n'
"#,
    );
}

fn team_send_command(temp: &TempDir, worker_bin: &Path, command_id: &str) -> Command {
    team_send_command_to(temp, worker_bin, command_id, "worker")
}

fn team_send_command_to(
    temp: &TempDir,
    worker_bin: &Path,
    command_id: &str,
    target: &str,
) -> Command {
    let mut command = team_cmd();
    command
        .current_dir(temp.path())
        .env("RIVE_WORKSPACE", temp.path())
        .env("RIVE_AGENT_ID", "orch")
        .env("RIVE_AGENT_TOKEN", "orch-token")
        .env("RIVE_RUN_ID", "run-orch")
        .arg("send")
        .arg("--to")
        .arg(target)
        .arg("--runner")
        .arg("opencode")
        .arg("--title")
        .arg("phase 7 fake delegation")
        .arg("--command-id")
        .arg(command_id)
        .arg("--wait")
        .arg("--timeout-seconds")
        .arg("10")
        .arg("--snapshot-path")
        .arg("phase7-result.txt")
        .arg("--opencode-bin")
        .arg(worker_bin)
        .arg("--stdin");
    command
}

#[test]
fn team_send_wait_delegates_to_fake_worker() {
    let temp = init_workspace();
    let orch = add_agent(&temp, "orch", "orchestrator", "orch-token");
    let worker = add_agent(&temp, "worker", "worker", "worker-long-token");
    let orch_id = orch["protocol"]["agent"]["agent_id"].as_str().unwrap();
    let worker_id = worker["protocol"]["agent"]["agent_id"].as_str().unwrap();
    let fake = temp.path().join("fake-opencode-worker");
    write_happy_worker(&fake);

    let response = run_json_with_stdin(
        &mut team_send_command(&temp, &fake, "phase7-send-happy"),
        "Create phase7-result.txt and report done.\n",
    );

    assert_eq!(response["protocol"]["action"], "team.send");
    assert_eq!(response["protocol"]["child_executed"], true);
    assert_eq!(
        response["protocol"]["delegation"]["source_agent_id"],
        orch_id
    );
    assert_eq!(
        response["protocol"]["delegation"]["target_agent_id"],
        worker_id
    );
    assert_eq!(response["protocol"]["delegation"]["runner"], "opencode");
    assert_eq!(response["protocol"]["dispatch"]["state"], "reported");
    assert_eq!(
        response["protocol"]["dispatch"]["latest_report_status"],
        "done"
    );
    assert_eq!(response["protocol"]["trace"]["adapter"], "opencode-plugin");
    assert_eq!(response["protocol"]["trace"]["event_count"], 1);
    assert_eq!(
        fs::read_to_string(temp.path().join("phase7-result.txt")).unwrap(),
        "RIVE_PHASE7_FAKE_OK\n"
    );

    let output = serde_json::to_string(&response).unwrap();
    assert!(!output.contains("worker-long-token"));
    assert!(!output.contains("tok_"));

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let facts: i64 = conn
        .query_row("select count(*) from facts", [], |row| row.get(0))
        .unwrap();
    let snapshots: i64 = conn
        .query_row("select count(*) from snapshots", [], |row| row.get(0))
        .unwrap();
    let dispatches: i64 = conn
        .query_row("select count(*) from dispatches", [], |row| row.get(0))
        .unwrap();
    let agent_runs: i64 = conn
        .query_row("select count(*) from agent_runs", [], |row| row.get(0))
        .unwrap();
    let work_or_pty_tables: i64 = conn
        .query_row(
            "select count(*) from sqlite_master where type='table' and (name like 'work_%' or name like '%pty%')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(facts, 2);
    assert_eq!(snapshots, 1);
    assert_eq!(dispatches, 1);
    assert_eq!(agent_runs, 1);
    assert_eq!(work_or_pty_tables, 0);
}

#[test]
fn team_send_rejects_non_orchestrator_and_bad_targets() {
    let temp = init_workspace();
    add_agent(&temp, "orch", "orchestrator", "orch-token");
    add_agent(&temp, "worker", "worker", "worker-token");
    add_agent(&temp, "other-orch", "orchestrator", "other-token");
    let fake = temp.path().join("fake-opencode-worker");
    write_happy_worker(&fake);

    let mut worker_actor = team_send_command(&temp, &fake, "phase7-worker-cannot-send");
    worker_actor
        .env("RIVE_AGENT_ID", "worker")
        .env("RIVE_AGENT_TOKEN", "worker-token");
    let error = run_json_expect_error(&mut worker_actor, Some("Should not run.\n"));
    assert_eq!(error["protocol"]["code"], "agent_role_not_allowed");

    let mut unknown_target =
        team_send_command_to(&temp, &fake, "phase7-unknown-target", "missing-worker");
    let error = run_json_expect_error(&mut unknown_target, Some("Should not run.\n"));
    assert_eq!(error["protocol"]["code"], "target_agent_not_found");

    let mut bad_role = team_send_command_to(&temp, &fake, "phase7-bad-role", "other-orch");
    let error = run_json_expect_error(&mut bad_role, Some("Should not run.\n"));
    assert_eq!(error["protocol"]["code"], "target_role_invalid");

    let mut no_wait = team_cmd();
    no_wait
        .current_dir(temp.path())
        .env("RIVE_WORKSPACE", temp.path())
        .env("RIVE_AGENT_ID", "orch")
        .env("RIVE_AGENT_TOKEN", "orch-token")
        .env("RIVE_RUN_ID", "run-orch")
        .arg("send")
        .arg("--to")
        .arg("worker")
        .arg("--runner")
        .arg("opencode")
        .arg("--title")
        .arg("no wait")
        .arg("--command-id")
        .arg("phase7-no-wait")
        .arg("--opencode-bin")
        .arg(&fake)
        .arg("--stdin");
    let error = run_json_expect_error(&mut no_wait, Some("Should not run.\n"));
    assert_eq!(error["protocol"]["code"], "wait_required");
}

#[test]
fn team_send_stdout_success_without_report_is_not_success() {
    let temp = init_workspace();
    add_agent(&temp, "orch", "orchestrator", "orch-token");
    add_agent(&temp, "worker", "worker", "worker-token");
    let fake = temp.path().join("fake-opencode-no-report");
    write_stdout_only_worker(&fake);

    let error = run_json_expect_error(
        &mut team_send_command(&temp, &fake, "phase7-no-report"),
        Some("Print success without reporting.\n"),
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
fn team_send_replay_does_not_reexecute_worker() {
    let temp = init_workspace();
    add_agent(&temp, "orch", "orchestrator", "orch-token");
    add_agent(&temp, "worker", "worker", "worker-token");
    let fake = temp.path().join("fake-opencode-counting");
    write_counting_worker(&fake);

    let first = run_json_with_stdin(
        &mut team_send_command(&temp, &fake, "phase7-replay"),
        "Create phase7-replay.txt and report once.\n",
    );
    assert_eq!(first["protocol"]["dispatch"]["state"], "reported");
    assert_eq!(first["protocol"]["child_executed"], true);

    let second = run_json_with_stdin(
        &mut team_send_command(&temp, &fake, "phase7-replay"),
        "Create phase7-replay.txt and report once.\n",
    );
    assert_eq!(
        second["protocol"]["delegation"]["idempotency_status"],
        "replayed"
    );
    assert_eq!(second["protocol"]["child_executed"], false);
    assert_eq!(
        second["protocol"]["dispatch"]["dispatch_id"],
        first["protocol"]["dispatch"]["dispatch_id"]
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("phase7-invocations.txt")).unwrap(),
        "1\n"
    );

    let conflict = run_json_expect_error(
        &mut team_send_command(&temp, &fake, "phase7-replay"),
        Some("Changed body should conflict.\n"),
    );
    assert_eq!(conflict["protocol"]["code"], "idempotency_conflict");
}
