use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

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

fn run_json_expect_error(command: &mut Command, stdin: &str) -> Value {
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

fn write_legal_opencode(path: &Path) {
    let rive_bin = env!("CARGO_BIN_EXE_rive");
    let team_bin = env!("CARGO_BIN_EXE_team");
    write_executable(
        path,
        &format!(
            r#"#!/bin/sh
set -eu
if [ -n "${{RIVE_ORCHESTRATOR_ROOT_WORK_ID:-}}" ]; then
  python -c 'print("blocked")' 2> "$RIVE_WORKSPACE/.rive/phase10-denied.txt" || true
  printf 'planning legal delegation\n' | "{team_bin}" work note "$RIVE_ORCHESTRATOR_ROOT_WORK_ID" --kind progress --command-id "phase10-note-$RIVE_RUN_ID" --stdin >/dev/null
  CHILD=$(printf 'worker should write phase10-result.txt\n' | "{team_bin}" work create --kind task --title implementation --command-id "phase10-child-$RIVE_RUN_ID" --stdin | sed -n 's/.*"work_node_id": "\([^"]*\)".*/\1/p' | head -n 1)
  "{team_bin}" work edge add --type decomposes-to --from "$RIVE_ORCHESTRATOR_ROOT_WORK_ID" --to "$CHILD" --command-id "phase10-edge-$RIVE_RUN_ID" >/dev/null
  printf 'Create phase10-result.txt and report done.\n' | "{team_bin}" send --work "$CHILD" --to worker --runner opencode --title implementation --command-id "phase10-send-$RIVE_RUN_ID" --wait --timeout-seconds 10 --opencode-bin "$0" --stdin >/dev/null
  printf 'child accepted\n' | "{team_bin}" work accept "$CHILD" --command-id "phase10-accept-child-$RIVE_RUN_ID" --stdin >/dev/null
  "{team_bin}" work graph inspect --root "$RIVE_ORCHESTRATOR_ROOT_WORK_ID" >/dev/null
  printf 'root accepted\n' | "{team_bin}" work accept "$RIVE_ORCHESTRATOR_ROOT_WORK_ID" --command-id "phase10-accept-root-$RIVE_RUN_ID" --stdin >/dev/null
  printf '{{"type":"step_finish","tokens":{{"input":10,"output":5,"reasoning":2,"cache":{{"read":3}},"total":20}}}}\n'
else
  if [ -n "${{RIVE_ORCHESTRATOR_ROOT_WORK_ID:-}}" ]; then exit 9; fi
  printf 'RIVE_PHASE10_WORKER_OK\n' > "$RIVE_WORKSPACE/phase10-result.txt"
  SNAPSHOT_ID=$("{rive_bin}" snapshot capture --path "$RIVE_WORKSPACE/phase10-result.txt" --label phase10-worker --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
  printf 'worker done\n' | "{team_bin}" report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --artifact-ref "file:phase10-result.txt" --command-id "phase10-report-$RIVE_RUN_ID" --stdin >/dev/null
  printf '{{"final":"worker done"}}\n'
fi
"#
        ),
    );
}

fn write_mutating_opencode(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
printf hacked > "$RIVE_WORKSPACE/source.py"
printf '{"final":"mutated directly"}\n'
"#,
    );
}

fn write_mutating_existing_artifact_opencode(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
printf hacked > "$RIVE_WORKSPACE/phase10-result.txt"
printf '{"final":"mutated existing artifact directly"}\n'
"#,
    );
}

fn write_orphan_opencode(path: &Path) {
    let team_bin = env!("CARGO_BIN_EXE_team");
    write_executable(
        path,
        &format!(
            r#"#!/bin/sh
set -eu
if [ -n "${{RIVE_ORCHESTRATOR_ROOT_WORK_ID:-}}" ]; then
  CHILD=$(printf child | "{team_bin}" work create --kind task --title child --command-id "phase10-child-$RIVE_RUN_ID" --stdin | sed -n 's/.*"work_node_id": "\([^"]*\)".*/\1/p' | head -n 1)
  ORPHAN=$(printf orphan | "{team_bin}" work create --kind task --title orphan --command-id "phase10-orphan-$RIVE_RUN_ID" --stdin | sed -n 's/.*"work_node_id": "\([^"]*\)".*/\1/p' | head -n 1)
  "{team_bin}" work edge add --type decomposes-to --from "$RIVE_ORCHESTRATOR_ROOT_WORK_ID" --to "$CHILD" --command-id "phase10-edge-$RIVE_RUN_ID" >/dev/null
  printf child | "{team_bin}" work note "$CHILD" --kind progress --command-id "phase10-note-child-$RIVE_RUN_ID" --stdin >/dev/null
  printf '{{"child":"'$CHILD'","orphan":"'$ORPHAN'"}}\n'
else
  printf '{{"final":"worker unused"}}\n'
fi
"#
        ),
    );
}

fn orchestrator_command(temp: &TempDir, fake: &Path, command_id: &str) -> Command {
    let mut command = rive_cmd();
    command
        .current_dir(temp.path())
        .arg("runner")
        .arg("orchestrator")
        .arg("--runner")
        .arg("opencode")
        .arg("--agent")
        .arg("orchestrator")
        .arg("--worker")
        .arg("worker")
        .arg("--command-id")
        .arg(command_id)
        .arg("--opencode-bin")
        .arg(fake)
        .arg("--timeout-seconds")
        .arg("20")
        .arg("--stdin");
    command
}

#[test]
fn orchestrator_sandbox_allows_control_plane_and_worker_writes() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-opencode");
    write_legal_opencode(&fake);
    let response = run_json_with_stdin(
        &mut orchestrator_command(&temp, &fake, "phase10-legal"),
        "Delegate work legally.\n",
    );
    assert_eq!(response["protocol"]["root_work"]["state"], "done");
    assert!(response["protocol"]["runner"]["audit"]["denied_paths"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        fs::read_to_string(temp.path().join("phase10-result.txt")).unwrap(),
        "RIVE_PHASE10_WORKER_OK\n"
    );
    assert!(
        fs::read_to_string(temp.path().join(".rive/phase10-denied.txt"))
            .unwrap()
            .contains("orchestrator_capability_denied")
    );
    let root = response["protocol"]["runner"]["root_work_node_id"]
        .as_str()
        .unwrap();
    let graph = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("graph")
            .arg("inspect")
            .arg("--root")
            .arg(root),
    );
    assert_eq!(graph["protocol"]["hygiene_state"], "clean");
    assert_eq!(graph["protocol"]["state"], "done");
    let usage = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("usage")
            .arg("--run")
            .arg(response["protocol"]["runner"]["run_id"].as_str().unwrap()),
    );
    assert_eq!(usage["protocol"]["totals"]["total_tokens"], 20);
    assert_eq!(usage["protocol"]["totals"]["cache_read_tokens"], 3);
}

#[test]
fn orchestrator_direct_workspace_mutation_is_rejected() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-opencode");
    write_mutating_opencode(&fake);
    let error = run_json_expect_error(
        &mut orchestrator_command(&temp, &fake, "phase10-mutation"),
        "Try direct mutation.\n",
    );
    assert_eq!(error["protocol"]["code"], "orchestrator_workspace_mutation");
}

#[test]
fn historical_worker_ref_does_not_allow_later_orchestrator_mutation() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-opencode");
    write_legal_opencode(&fake);
    run_json_with_stdin(
        &mut orchestrator_command(&temp, &fake, "phase10-historical-legal"),
        "Create a legitimate worker artifact.\n",
    );

    write_mutating_existing_artifact_opencode(&fake);
    let mut second = rive_cmd();
    second
        .current_dir(temp.path())
        .arg("runner")
        .arg("orchestrator")
        .arg("--runner")
        .arg("opencode")
        .arg("--agent")
        .arg("orchestrator-two")
        .arg("--worker")
        .arg("worker-two")
        .arg("--command-id")
        .arg("phase10-historical-mutation")
        .arg("--opencode-bin")
        .arg(&fake)
        .arg("--timeout-seconds")
        .arg("20")
        .arg("--stdin");
    let error = run_json_expect_error(
        &mut second,
        "Mutate the previous worker artifact directly.\n",
    );
    assert_eq!(error["protocol"]["code"], "orchestrator_workspace_mutation");
}

#[test]
fn root_accept_is_blocked_by_root_scoped_orphan() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-opencode");
    write_orphan_opencode(&fake);
    let error = run_json_expect_error(
        &mut orchestrator_command(&temp, &fake, "phase10-orphan"),
        "Create an orphan.\n",
    );
    assert_eq!(error["protocol"]["code"], "work_graph_not_closed");
}

#[test]
fn worker_cannot_write_work_note() {
    let temp = init_workspace();
    let work = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("create")
            .arg("--kind")
            .arg("task")
            .arg("--title")
            .arg("note target")
            .arg("--command-id")
            .arg("note-target"),
    )["protocol"]["work_node_id"]
        .as_str()
        .unwrap()
        .to_string();
    let worker = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("agent")
            .arg("add")
            .arg("worker")
            .arg("--role")
            .arg("worker")
            .arg("--token")
            .arg("worker-token"),
    );
    let worker_id = worker["protocol"]["agent"]["agent_id"].as_str().unwrap();
    let mut command = team_cmd();
    command
        .current_dir(temp.path())
        .env("RIVE_WORKSPACE", temp.path())
        .env("RIVE_AGENT_ID", worker_id)
        .env("RIVE_AGENT_TOKEN", "worker-token")
        .arg("work")
        .arg("note")
        .arg(&work)
        .arg("--kind")
        .arg("progress")
        .arg("--command-id")
        .arg("worker-note")
        .arg("--stdin");
    let error = run_json_expect_error(&mut command, "worker progress\n");
    assert_eq!(error["protocol"]["code"], "agent_role_not_allowed");
}
