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

fn create_work(temp: &TempDir, command_id: &str, title: &str) -> String {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("create")
            .arg("--kind")
            .arg("task")
            .arg("--title")
            .arg(title)
            .arg("--command-id")
            .arg(command_id),
    )["protocol"]["work_node_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn create_work_with_body(temp: &TempDir, command_id: &str, title: &str, body: &str) -> String {
    let mut command = rive_cmd();
    command
        .current_dir(temp.path())
        .arg("work")
        .arg("create")
        .arg("--kind")
        .arg("task")
        .arg("--title")
        .arg(title)
        .arg("--command-id")
        .arg(command_id)
        .arg("--stdin");
    run_json_with_stdin(&mut command, body)["protocol"]["work_node_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn inspect_work(temp: &TempDir, work_node_id: &str) -> Value {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("inspect")
            .arg(work_node_id),
    )
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn write_phase8_worker(path: &Path, result_name: &str) {
    let script = format!(
        r#"#!/bin/sh
set -eu
COUNT_FILE="$RIVE_WORKSPACE/phase8-invocations.txt"
if [ -f "$COUNT_FILE" ]; then COUNT=$(cat "$COUNT_FILE"); else COUNT=0; fi
COUNT=$((COUNT + 1))
printf '%s\n' "$COUNT" > "$COUNT_FILE"
printf 'RIVE_PHASE8_FAKE_OK\n' > "$RIVE_WORKSPACE/{result_name}"
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE/{result_name}" --label phase8-fake-worker --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'phase8 worker done\n' | team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --artifact-ref "file:{result_name}" --command-id "phase8-report-$RIVE_RUN_ID" --stdin >/dev/null
printf '{{"final":"RIVE_PHASE8_FAKE_OK"}}\n'
"#
    );
    write_executable(path, &script);
}

fn team_send_work_command(
    temp: &TempDir,
    worker_bin: &Path,
    work_node_id: &str,
    command_id: &str,
) -> Command {
    let mut command = team_cmd();
    command
        .current_dir(temp.path())
        .env("RIVE_WORKSPACE", temp.path())
        .env("RIVE_AGENT_ID", "orch")
        .env("RIVE_AGENT_TOKEN", "orch-token")
        .env("RIVE_RUN_ID", "run-orch")
        .arg("send")
        .arg("--work")
        .arg(work_node_id)
        .arg("--to")
        .arg("worker")
        .arg("--runner")
        .arg("opencode")
        .arg("--title")
        .arg("phase 8 fake work")
        .arg("--command-id")
        .arg(command_id)
        .arg("--wait")
        .arg("--timeout-seconds")
        .arg("10")
        .arg("--opencode-bin")
        .arg(worker_bin)
        .arg("--stdin");
    command
}

#[test]
fn work_node_can_be_delegated_and_accepted() {
    let temp = init_workspace();
    add_agent(&temp, "orch", "orchestrator", "orch-token");
    add_agent(&temp, "worker", "worker", "worker-token");
    let work = create_work_with_body(
        &temp,
        "work-single",
        "single node",
        "Node body acceptance: produce phase8-result.txt and report evidence.",
    );
    let fake = temp.path().join("fake-opencode-worker");
    write_phase8_worker(&fake, "phase8-result.txt");

    let response = run_json_with_stdin(
        &mut team_send_work_command(&temp, &fake, &work, "phase8-send-single"),
        "Create phase8-result.txt and report done.\n",
    );
    assert_eq!(response["protocol"]["dispatch"]["state"], "reported");
    assert_eq!(
        response["protocol"]["work"]["state"],
        Value::String("reviewable".to_string())
    );
    let worker_run_id = response["protocol"]["delegation"]["worker_run_id"]
        .as_str()
        .unwrap();
    let prompt = fs::read_to_string(
        temp.path()
            .join(".rive/debug/runs")
            .join(worker_run_id)
            .join("prompt.txt"),
    )
    .unwrap();
    assert!(prompt.contains("Node body acceptance: produce phase8-result.txt and report evidence."));
    assert!(prompt.contains("Delegation request:"));
    assert!(prompt.contains("Create phase8-result.txt and report done."));
    assert!(prompt.contains("Make source/artifact edits only under `$RIVE_WORKSPACE`"));

    let inspected = inspect_work(&temp, &work);
    assert_eq!(inspected["protocol"]["projection"]["state"], "reviewable");
    assert!(inspected["protocol"]["refs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["snapshot_id"].as_str().is_some()));

    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("accept")
            .arg(&work)
            .arg("--command-id")
            .arg("accept-single"),
    );
    assert_eq!(
        inspect_work(&temp, &work)["protocol"]["projection"]["state"],
        "done"
    );
}

#[test]
fn dependency_unlock_and_cycle_rejection_are_projected() {
    let temp = init_workspace();
    add_agent(&temp, "orch", "orchestrator", "orch-token");
    add_agent(&temp, "worker", "worker", "worker-token");
    let a = create_work(&temp, "work-a", "A");
    let b = create_work(&temp, "work-b", "B");
    let c = create_work(&temp, "work-c", "C");

    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("edge")
            .arg("add")
            .arg("--type")
            .arg("depends-on")
            .arg("--from")
            .arg(&b)
            .arg("--to")
            .arg(&a)
            .arg("--command-id")
            .arg("edge-b-a"),
    );
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("edge")
            .arg("add")
            .arg("--type")
            .arg("depends-on")
            .arg("--from")
            .arg(&c)
            .arg("--to")
            .arg(&b)
            .arg("--command-id")
            .arg("edge-c-b"),
    );

    assert_eq!(
        inspect_work(&temp, &b)["protocol"]["projection"]["state"],
        "blocked"
    );
    let cycle_error = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("edge")
            .arg("add")
            .arg("--type")
            .arg("depends-on")
            .arg("--from")
            .arg(&a)
            .arg("--to")
            .arg(&c)
            .arg("--command-id")
            .arg("edge-a-c"),
        None,
    );
    assert_eq!(cycle_error["protocol"]["code"], "work_graph_cycle");

    let fake = temp.path().join("fake-opencode-worker-a");
    write_phase8_worker(&fake, "phase8-a.txt");
    let report = run_json_with_stdin(
        &mut team_send_work_command(&temp, &fake, &a, "phase8-send-a"),
        "Create phase8-a.txt and report done.\n",
    );
    assert_eq!(report["protocol"]["work"]["state"], "reviewable");
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("accept")
            .arg(&a)
            .arg("--command-id")
            .arg("accept-a"),
    );
    assert_eq!(
        inspect_work(&temp, &b)["protocol"]["projection"]["state"],
        "ready"
    );
    assert_eq!(
        inspect_work(&temp, &c)["protocol"]["projection"]["state"],
        "blocked"
    );
}

#[test]
fn decomposed_parent_becomes_reviewable_without_own_snapshot_requirement() {
    let temp = init_workspace();
    add_agent(&temp, "orch", "orchestrator", "orch-token");
    add_agent(&temp, "worker", "worker", "worker-token");
    let root = create_work(&temp, "work-root", "root");
    let child = create_work(&temp, "work-child", "child");
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("edge")
            .arg("add")
            .arg("--type")
            .arg("decomposes-to")
            .arg("--from")
            .arg(&root)
            .arg("--to")
            .arg(&child)
            .arg("--command-id")
            .arg("edge-root-child"),
    );

    let fake = temp.path().join("fake-opencode-child");
    write_phase8_worker(&fake, "phase8-child.txt");
    let response = run_json_with_stdin(
        &mut team_send_work_command(&temp, &fake, &child, "phase8-send-child"),
        "Create phase8-child.txt and report done.\n",
    );
    assert_eq!(response["protocol"]["work"]["state"], "reviewable");
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("accept")
            .arg(&child)
            .arg("--command-id")
            .arg("accept-child"),
    );

    let root_inspect = inspect_work(&temp, &root);
    assert_eq!(
        root_inspect["protocol"]["projection"]["state"],
        "reviewable"
    );
    assert_eq!(
        root_inspect["protocol"]["projection"]["missing_requirements"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn team_send_work_replay_does_not_reexecute_worker() {
    let temp = init_workspace();
    add_agent(&temp, "orch", "orchestrator", "orch-token");
    add_agent(&temp, "worker", "worker", "worker-token");
    let work = create_work(&temp, "work-replay", "replay node");
    let fake = temp.path().join("fake-opencode-worker");
    write_phase8_worker(&fake, "phase8-replay.txt");

    let first = run_json_with_stdin(
        &mut team_send_work_command(&temp, &fake, &work, "phase8-send-replay"),
        "Create phase8-replay.txt and report done.\n",
    );
    let second = run_json_with_stdin(
        &mut team_send_work_command(&temp, &fake, &work, "phase8-send-replay"),
        "Create phase8-replay.txt and report done.\n",
    );

    assert_eq!(first["protocol"]["dispatch"]["state"], "reported");
    assert_eq!(second["protocol"]["child_executed"], false);
    assert_eq!(
        second["protocol"]["dispatch"]["dispatch_id"],
        first["protocol"]["dispatch"]["dispatch_id"]
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("phase8-invocations.txt")).unwrap(),
        "1\n"
    );

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let bindings: i64 = conn
        .query_row("select count(*) from work_dispatch_bindings", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(bindings, 1);
}

#[test]
fn accept_requires_reviewable_and_report_replay_does_not_duplicate_refs() {
    let temp = init_workspace();
    add_agent(&temp, "orch", "orchestrator", "orch-token");
    let worker = add_agent(&temp, "worker", "worker", "worker-token");
    let worker_id = worker["protocol"]["agent"]["agent_id"].as_str().unwrap();
    let work = create_work(&temp, "work-accept-guard", "accept guard");

    let accept_error = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("accept")
            .arg(&work)
            .arg("--command-id")
            .arg("accept-before-report"),
        None,
    );
    assert_eq!(accept_error["protocol"]["code"], "work_node_not_reviewable");

    let fake = temp.path().join("fake-opencode-worker-accept");
    write_phase8_worker(&fake, "phase8-accept.txt");
    let response = run_json_with_stdin(
        &mut team_send_work_command(&temp, &fake, &work, "phase8-send-accept"),
        "Create phase8-accept.txt and report done.\n",
    );
    assert_eq!(response["protocol"]["work"]["state"], "reviewable");
    let dispatch_id = response["protocol"]["dispatch"]["dispatch_id"]
        .as_str()
        .unwrap()
        .to_string();
    let run_id = response["protocol"]["delegation"]["worker_run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let snapshot_id = inspect_work(&temp, &work)["protocol"]["refs"][0]["snapshot_id"]
        .as_str()
        .unwrap()
        .to_string();

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let refs_before: i64 = conn
        .query_row("select count(*) from work_ref_bindings", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(refs_before, 2);

    let replay = run_json_with_stdin(
        team_cmd()
            .current_dir(temp.path())
            .env("RIVE_WORKSPACE", temp.path())
            .env("RIVE_AGENT_ID", worker_id)
            .env("RIVE_AGENT_TOKEN", "worker-token")
            .env("RIVE_RUN_ID", &run_id)
            .arg("report")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--status")
            .arg("done")
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--artifact-ref")
            .arg("file:phase8-accept.txt")
            .arg("--command-id")
            .arg(format!("phase8-report-{run_id}"))
            .arg("--stdin"),
        "phase8 worker done\n",
    );
    assert_eq!(replay["protocol"]["fact"]["idempotency_status"], "replayed");
    let refs_after: i64 = conn
        .query_row("select count(*) from work_ref_bindings", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(refs_after, refs_before);

    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("accept")
            .arg(&work)
            .arg("--command-id")
            .arg("accept-after-report"),
    );
    let accepted_replay = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("accept")
            .arg(&work)
            .arg("--command-id")
            .arg("accept-after-report"),
    );
    assert_eq!(
        accepted_replay["protocol"]["idempotency_status"],
        "replayed"
    );
}
