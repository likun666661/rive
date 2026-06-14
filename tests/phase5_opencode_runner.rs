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

fn write_happy_opencode(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
test -n "$RIVE_WORKSPACE"
test -n "$RIVE_AGENT_ID"
test -n "$RIVE_AGENT_TOKEN"
test -n "$RIVE_RUN_ID"
test -n "$RIVE_DISPATCH_ID"
printf 'RIVE_PHASE5_FAKE_OK\n' > "$RIVE_WORKSPACE/phase5-result.txt"
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE/phase5-result.txt" --label fake-opencode-result --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'still working\n' | team status --dispatch "$RIVE_DISPATCH_ID" --snapshot "$SNAPSHOT_ID" --command-id "fake-status-$RIVE_RUN_ID" --stdin >/dev/null
printf 'done with evidence\n' | team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "fake-report-$RIVE_RUN_ID" --stdin >/dev/null
printf '{"type":"message.part.updated","properties":{"sessionID":"fake-opencode-session","messageID":"msg_1","part":{"type":"text","text":"debug only RIVE_PHASE5_FAKE_OK"}}}' | rive debug trace ingest --adapter opencode-plugin --agent "$RIVE_AGENT_ID" --run "$RIVE_RUN_ID" --dispatch "$RIVE_DISPATCH_ID" --stdin >/dev/null
printf '{"final":"RIVE_PHASE5_FAKE_OK"}\n'
"#,
    );
}

fn write_stdout_only_opencode(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
printf '{"final":"RIVE_TRACE_OK but no team report"}\n'
"#,
    );
}

fn write_counting_opencode(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
COUNT_FILE="$RIVE_WORKSPACE/invocations.txt"
if [ -f "$COUNT_FILE" ]; then
  COUNT=$(cat "$COUNT_FILE")
else
  COUNT=0
fi
COUNT=$((COUNT + 1))
printf '%s\n' "$COUNT" > "$COUNT_FILE"
printf 'RIVE_PHASE5_REPLAY_OK\n' > "$RIVE_WORKSPACE/replay-result.txt"
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE/replay-result.txt" --label fake-opencode-replay --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'done once\n' | team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "fake-replay-report-$RIVE_RUN_ID" --stdin >/dev/null
printf '{"final":"RIVE_PHASE5_REPLAY_OK"}\n'
"#,
    );
}

fn write_env_probe_opencode(path: &Path) {
    let rive_bin = env!("CARGO_BIN_EXE_rive");
    let team_bin = env!("CARGO_BIN_EXE_team");
    write_executable(
        path,
        &format!(
            r#"#!/bin/sh
set -eu
RUN_ROOT="$RIVE_WORKSPACE/.rive/debug/runs/$RIVE_RUN_ID"
test "$XDG_DATA_HOME" = "$RUN_ROOT/opencode-data"
test "$XDG_CACHE_HOME" = "$RUN_ROOT/opencode-cache"
test "$XDG_STATE_HOME" = "$RUN_ROOT/opencode-state"
test "$TMPDIR" = "$RUN_ROOT/opencode-tmp"
test -d "$XDG_DATA_HOME"
test -d "$XDG_CACHE_HOME"
test -d "$XDG_STATE_HOME"
test -d "$TMPDIR"
printf 'data=%s\ncache=%s\nstate=%s\ntmp=%s\n' "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_STATE_HOME" "$TMPDIR" > "$RIVE_WORKSPACE/opencode-env.txt"
SNAPSHOT_ID=$("{rive_bin}" snapshot capture --path "$RIVE_WORKSPACE/opencode-env.txt" --label fake-opencode-env --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'env probe done\n' | "{team_bin}" report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "fake-env-report-$RIVE_RUN_ID" --stdin >/dev/null
printf '{{"final":"RIVE_PHASE5_ENV_OK"}}\n'
"#
        ),
    );
}

fn runner_command(temp: &TempDir, opencode_bin: &Path, command_id: &str) -> Command {
    let mut command = rive_cmd();
    command
        .current_dir(temp.path())
        .arg("runner")
        .arg("opencode")
        .arg("--agent")
        .arg("runner-worker")
        .arg("--title")
        .arg("phase 5 fake task")
        .arg("--command-id")
        .arg(command_id)
        .arg("--opencode-bin")
        .arg(opencode_bin)
        .arg("--timeout-seconds")
        .arg("10")
        .arg("--snapshot-path")
        .arg("phase5-result.txt")
        .arg("--stdin");
    command
}

fn runner_command_with_token(
    temp: &TempDir,
    opencode_bin: &Path,
    command_id: &str,
    token: &str,
) -> Command {
    let mut command = runner_command(temp, opencode_bin, command_id);
    command.arg("--agent-token").arg(token);
    command
}

#[test]
fn opencode_runner_isolates_global_state_dirs() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-opencode-env");
    write_env_probe_opencode(&fake);

    let response = run_json_with_stdin(
        &mut runner_command(&temp, &fake, "runner-env"),
        "Probe OpenCode environment.\n",
    );

    let run_id = response["protocol"]["runner"]["run_id"].as_str().unwrap();
    let run_root = fs::canonicalize(temp.path())
        .unwrap()
        .join(".rive/debug/runs")
        .join(run_id);
    let env = fs::read_to_string(temp.path().join("opencode-env.txt")).unwrap();
    assert!(env.contains(&format!(
        "data={}",
        run_root.join("opencode-data").display()
    )));
    assert!(env.contains(&format!(
        "cache={}",
        run_root.join("opencode-cache").display()
    )));
    assert!(env.contains(&format!(
        "state={}",
        run_root.join("opencode-state").display()
    )));
    assert!(env.contains(&format!("tmp={}", run_root.join("opencode-tmp").display())));
}

#[test]
fn opencode_runner_reports_dispatch_with_fake_opencode() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-opencode");
    write_happy_opencode(&fake);

    let response = run_json_with_stdin(
        &mut runner_command(&temp, &fake, "runner-happy"),
        "Create phase5-result.txt and report done.\n",
    );

    assert_eq!(response["protocol"]["runner"]["kind"], "opencode");
    assert_eq!(response["protocol"]["runner"]["child_executed"], true);
    assert_eq!(response["protocol"]["runner"]["exit_code"], 0);
    assert_eq!(response["protocol"]["dispatch"]["state"], "reported");
    assert_eq!(
        response["protocol"]["dispatch"]["latest_report_status"],
        "done"
    );
    assert_eq!(response["protocol"]["trace"]["adapter"], "opencode-plugin");
    assert_eq!(response["protocol"]["trace"]["event_count"], 1);

    let dispatch_id = response["protocol"]["dispatch"]["dispatch_id"]
        .as_str()
        .unwrap();
    let show = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("dispatch")
            .arg("show")
            .arg(dispatch_id),
    );
    assert_eq!(show["protocol"]["state"], "reported");

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let facts: i64 = conn
        .query_row("select count(*) from facts", [], |row| row.get(0))
        .unwrap();
    let snapshots: i64 = conn
        .query_row("select count(*) from snapshots", [], |row| row.get(0))
        .unwrap();
    let trace_events: i64 = conn
        .query_row("select count(*) from debug_trace_events", [], |row| {
            row.get(0)
        })
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
    assert_eq!(trace_events, 1);
    assert_eq!(work_graph_tables, 0);
}

#[test]
fn stdout_success_without_team_report_is_not_success() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-opencode-stdout-only");
    write_stdout_only_opencode(&fake);

    let error = run_json_expect_error(
        &mut runner_command(&temp, &fake, "runner-no-report"),
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
fn existing_agent_requires_plaintext_token_and_validates_it() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-opencode");
    write_happy_opencode(&fake);
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("agent")
            .arg("add")
            .arg("runner-worker")
            .arg("--role")
            .arg("worker")
            .arg("--token")
            .arg("runner-token"),
    );

    let missing = run_json_expect_error(
        &mut runner_command(&temp, &fake, "runner-token-missing"),
        Some("Should not run.\n"),
    );
    assert_eq!(missing["protocol"]["code"], "runner_agent_token_required");

    let mut wrong_token = runner_command(&temp, &fake, "runner-token-wrong");
    wrong_token.arg("--agent-token").arg("wrong-token");
    let wrong = run_json_expect_error(&mut wrong_token, Some("Should not run.\n"));
    assert_eq!(wrong["protocol"]["code"], "agent_token_invalid");
}

#[test]
fn missing_opencode_binary_is_reported() {
    let temp = init_workspace();
    let missing_bin = temp.path().join("missing-opencode");
    let error = run_json_expect_error(
        &mut runner_command(&temp, &missing_bin, "runner-missing-bin"),
        Some("Should fail before child launch.\n"),
    );
    assert_eq!(error["protocol"]["code"], "opencode_not_found");
}

#[test]
fn replayed_runner_command_does_not_execute_child_again() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-opencode-counting");
    write_counting_opencode(&fake);
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("agent")
            .arg("add")
            .arg("runner-worker")
            .arg("--role")
            .arg("worker")
            .arg("--token")
            .arg("runner-token"),
    );

    let first = run_json_with_stdin(
        &mut runner_command_with_token(&temp, &fake, "runner-replay", "runner-token"),
        "Create replay-result.txt and report done.\n",
    );
    assert_eq!(first["protocol"]["dispatch"]["state"], "reported");
    assert_eq!(first["protocol"]["runner"]["child_executed"], true);
    assert_eq!(
        fs::read_to_string(temp.path().join("invocations.txt")).unwrap(),
        "1\n"
    );

    let second = run_json_with_stdin(
        &mut runner_command_with_token(&temp, &fake, "runner-replay", "runner-token"),
        "Create replay-result.txt and report done.\n",
    );
    assert_eq!(
        second["protocol"]["dispatch"]["idempotency_status"],
        "replayed"
    );
    assert_eq!(second["protocol"]["dispatch"]["state"], "reported");
    assert_eq!(second["protocol"]["runner"]["child_executed"], false);
    assert_eq!(second["protocol"]["runner"]["exit_code"], Value::Null);
    assert_eq!(
        fs::read_to_string(temp.path().join("invocations.txt")).unwrap(),
        "1\n"
    );
}
