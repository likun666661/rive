use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

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
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout should be json: {err}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn run_json_expect_error(command: &mut Command) -> Value {
    let output = command.output().expect("command should run");
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout should be json: {err}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
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

fn add_worker(temp: &TempDir, name: &str) {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("agent")
            .arg("add")
            .arg(name)
            .arg("--role")
            .arg("worker")
            .arg("--token")
            .arg(format!("{name}-token")),
    );
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

fn add_edge(temp: &TempDir, edge_type: &str, from: &str, to: &str, command_id: &str) {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("edge")
            .arg("add")
            .arg("--type")
            .arg(edge_type)
            .arg("--from")
            .arg(from)
            .arg("--to")
            .arg(to)
            .arg("--command-id")
            .arg(command_id),
    );
}

fn inspect_work(temp: &TempDir, work: &str) -> Value {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("inspect")
            .arg(work),
    )
}

fn scheduler_command_with_mode(
    temp: &TempDir,
    fake: &Path,
    root: &str,
    command_id: &str,
    acceptance_mode: &str,
) -> Command {
    scheduler_command_full(temp, fake, root, command_id, acceptance_mode, "2")
}

fn scheduler_command_full(
    temp: &TempDir,
    fake: &Path,
    root: &str,
    command_id: &str,
    acceptance_mode: &str,
    max_parallel: &str,
) -> Command {
    let mut command = rive_cmd();
    command
        .current_dir(temp.path())
        .arg("scheduler")
        .arg("run")
        .arg("--root")
        .arg(root)
        .arg("--runner")
        .arg("opencode")
        .arg("--worker")
        .arg("worker-a")
        .arg("--worker")
        .arg("worker-b")
        .arg("--command-id")
        .arg(command_id)
        .arg("--max-parallel")
        .arg(max_parallel)
        .arg("--acceptance-mode")
        .arg(acceptance_mode)
        .arg("--opencode-bin")
        .arg(fake)
        .arg("--timeout-seconds")
        .arg("10");
    command
}

fn scheduler_command(temp: &TempDir, fake: &Path, root: &str, command_id: &str) -> Command {
    scheduler_command_with_mode(temp, fake, root, command_id, "auto-reported")
}

fn write_scheduler_worker(path: &Path) {
    let rive_bin = env!("CARGO_BIN_EXE_rive");
    let team_bin = env!("CARGO_BIN_EXE_team");
    write_executable(
        path,
        &format!(
            r#"#!/bin/sh
set -eu
COUNT_FILE="$RIVE_WORKSPACE/phase11-invocations.txt"
printf '1\n' >> "$COUNT_FILE"
PROMPT_FILE="$RIVE_WORKSPACE/.rive/debug/runs/$RIVE_RUN_ID/prompt.txt"
WORK_ID=$(sed -n 's/^- id: \(work_[a-z0-9]*\)$/\1/p' "$PROMPT_FILE" | head -n 1)
TITLE=$(sed -n 's/^- title: \(.*\)$/\1/p' "$PROMPT_FILE" | head -n 1)
case "$TITLE" in
  A*) RESULT="phase11-a.txt" ;;
  B*) RESULT="phase11-b.txt" ;;
  C*) RESULT="phase11-c.txt" ;;
  *) RESULT="phase11-$WORK_ID.txt" ;;
esac
printf 'RIVE_PHASE11_%s_OK\n' "$TITLE" > "$RIVE_WORKSPACE/$RESULT"
SNAPSHOT_ID=$("{rive_bin}" snapshot capture --path "$RIVE_WORKSPACE/$RESULT" --label phase11-worker --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'scheduler worker done\n' | "{team_bin}" report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --artifact-ref "file:$RESULT" --command-id "phase11-report-$RIVE_RUN_ID" --stdin >/dev/null
printf '{{"type":"step_finish","tokens":{{"input":7,"output":3,"reasoning":1,"cache":{{"read":2}},"total":13}}}}\n'
"#
        ),
    );
}

fn write_no_report_worker(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
COUNT_FILE="$RIVE_WORKSPACE/phase11-no-report-count.txt"
if [ -f "$COUNT_FILE" ]; then COUNT=$(cat "$COUNT_FILE"); else COUNT=0; fi
COUNT=$((COUNT + 1))
printf '%s\n' "$COUNT" > "$COUNT_FILE"
printf '{"final":"looks done but no team report"}\n'
"#,
    );
}

fn setup_graph(temp: &TempDir) -> (String, String, String, String) {
    let root = create_work(temp, "phase11-root", "root");
    let a = create_work(temp, "phase11-a", "A");
    let b = create_work(temp, "phase11-b", "B");
    let c = create_work(temp, "phase11-c", "C");
    add_edge(temp, "decomposes-to", &root, &a, "edge-root-a");
    add_edge(temp, "decomposes-to", &root, &b, "edge-root-b");
    add_edge(temp, "decomposes-to", &root, &c, "edge-root-c");
    add_edge(temp, "depends-on", &c, &a, "edge-c-a");
    add_edge(temp, "depends-on", &c, &b, "edge-c-b");
    (root, a, b, c)
}

#[test]
fn scheduler_auto_reported_runs_ready_nodes_and_accepts_root() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let (root, a, b, c) = setup_graph(&temp);
    let fake = temp.path().join("fake-opencode-scheduler");
    write_scheduler_worker(&fake);

    let response = run_json(&mut scheduler_command(
        &temp,
        &fake,
        &root,
        "phase11-sched-happy",
    ));
    assert_eq!(response["protocol"]["scheduler"]["state"], "completed");
    assert_eq!(response["protocol"]["root_work"]["state"], "done");
    assert_eq!(
        response["protocol"]["launched_nodes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(response["protocol"]["usage_summary"]["total_tokens"], 39);
    assert_eq!(
        inspect_work(&temp, &a)["protocol"]["projection"]["state"],
        "done"
    );
    assert_eq!(
        inspect_work(&temp, &b)["protocol"]["projection"]["state"],
        "done"
    );
    assert_eq!(
        inspect_work(&temp, &c)["protocol"]["projection"]["state"],
        "done"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("phase11-invocations.txt"))
            .unwrap()
            .lines()
            .count(),
        3
    );
    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let accepted_events: i64 = conn
        .query_row(
            "select count(*) from events where event_type='work.node.accepted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(accepted_events, 4);
}

#[test]
fn scheduler_manual_mode_waits_for_review() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase11-manual-root", "root");
    let a = create_work(&temp, "phase11-manual-a", "A");
    add_edge(&temp, "decomposes-to", &root, &a, "edge-manual-root-a");
    let fake = temp.path().join("fake-opencode-scheduler");
    write_scheduler_worker(&fake);
    let mut command =
        scheduler_command_with_mode(&temp, &fake, &root, "phase11-sched-manual", "manual");
    let response = run_json(&mut command);
    assert_eq!(response["protocol"]["scheduler"]["state"], "waiting_review");
    assert_eq!(
        inspect_work(&temp, &a)["protocol"]["projection"]["state"],
        "reviewable"
    );
    assert_eq!(
        inspect_work(&temp, &root)["protocol"]["projection"]["state"],
        "blocked"
    );
}

#[test]
fn scheduler_auto_reported_accepts_existing_reviewable_then_continues() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase11-existing-root", "root");
    let a = create_work(&temp, "phase11-existing-a", "A");
    let c = create_work(&temp, "phase11-existing-c", "C");
    add_edge(&temp, "decomposes-to", &root, &a, "edge-existing-root-a");
    add_edge(&temp, "decomposes-to", &root, &c, "edge-existing-root-c");
    add_edge(&temp, "depends-on", &c, &a, "edge-existing-c-a");
    let fake = temp.path().join("fake-opencode-scheduler");
    write_scheduler_worker(&fake);

    let manual = run_json(&mut scheduler_command_with_mode(
        &temp,
        &fake,
        &root,
        "phase11-existing-manual",
        "manual",
    ));
    assert_eq!(manual["protocol"]["scheduler"]["state"], "waiting_review");
    assert_eq!(
        inspect_work(&temp, &a)["protocol"]["projection"]["state"],
        "reviewable"
    );
    assert_eq!(
        inspect_work(&temp, &c)["protocol"]["projection"]["state"],
        "blocked"
    );

    let auto = run_json(&mut scheduler_command(
        &temp,
        &fake,
        &root,
        "phase11-existing-auto",
    ));
    assert_eq!(auto["protocol"]["scheduler"]["state"], "completed");
    assert_eq!(auto["protocol"]["root_work"]["state"], "done");
    assert_eq!(
        inspect_work(&temp, &a)["protocol"]["projection"]["state"],
        "done"
    );
    assert_eq!(
        inspect_work(&temp, &c)["protocol"]["projection"]["state"],
        "done"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("phase11-invocations.txt"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[test]
fn scheduler_replay_does_not_relaunch_workers() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let (root, _, _, _) = setup_graph(&temp);
    let fake = temp.path().join("fake-opencode-scheduler");
    write_scheduler_worker(&fake);
    let first = run_json(&mut scheduler_command(
        &temp,
        &fake,
        &root,
        "phase11-sched-replay",
    ));
    let second = run_json(&mut scheduler_command(
        &temp,
        &fake,
        &root,
        "phase11-sched-replay",
    ));
    assert_eq!(first["protocol"]["scheduler"]["state"], "completed");
    assert_eq!(second["protocol"]["scheduler"]["child_executed"], false);
    assert_eq!(
        fs::read_to_string(temp.path().join("phase11-invocations.txt"))
            .unwrap()
            .lines()
            .count(),
        3
    );
}

#[test]
fn scheduler_rejects_stdout_success_without_report() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let (root, _, _, _) = setup_graph(&temp);
    let fake = temp.path().join("fake-opencode-no-report");
    write_no_report_worker(&fake);
    let error = run_json_expect_error(&mut scheduler_command(
        &temp,
        &fake,
        &root,
        "phase11-sched-no-report",
    ));
    assert_eq!(error["protocol"]["code"], "dispatch_not_reported");
    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let failed_node_runs: i64 = conn
        .query_row(
            "select count(*) from scheduler_node_runs where state='failed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(failed_node_runs, 2);
    let active_node_runs: i64 = conn
        .query_row(
            "select count(*) from scheduler_node_runs where state in ('claimed', 'running')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active_node_runs, 0);
}

#[test]
fn scheduler_idempotency_conflict_and_dirty_graph_are_rejected() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let (root, _, _, _) = setup_graph(&temp);
    let orphan = create_work(&temp, "phase11-orphan", "orphan");
    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    conn.execute(
        "insert into work_root_bindings (root_work_node_id, work_node_id, created_by_agent_id, created_by_run_id, created_at) values (?1, ?2, null, null, '2026-06-01T00:00:00Z')",
        [&root, &orphan],
    )
    .unwrap();
    drop(conn);
    let fake = temp.path().join("fake-opencode-scheduler");
    write_scheduler_worker(&fake);
    let dirty = run_json_expect_error(&mut scheduler_command(
        &temp,
        &fake,
        &root,
        "phase11-sched-dirty",
    ));
    assert_eq!(dirty["protocol"]["code"], "work_graph_not_closed");

    let clean_root = create_work(&temp, "phase11-conflict-root", "clean root");
    let clean_child = create_work(&temp, "phase11-conflict-child", "A");
    add_edge(
        &temp,
        "decomposes-to",
        &clean_root,
        &clean_child,
        "edge-conflict-root-child",
    );
    let ok = run_json(&mut scheduler_command(
        &temp,
        &fake,
        &clean_root,
        "phase11-sched-conflict",
    ));
    assert_eq!(ok["protocol"]["scheduler"]["state"], "completed");
    let conflict = run_json_expect_error(&mut scheduler_command_full(
        &temp,
        &fake,
        &clean_root,
        "phase11-sched-conflict",
        "auto-reported",
        "1",
    ));
    assert_eq!(conflict["protocol"]["code"], "idempotency_conflict");
}
