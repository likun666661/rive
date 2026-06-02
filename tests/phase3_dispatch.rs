use std::fs;
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
    assert!(!output.status.success());
    serde_json::from_slice(&output.stdout).expect("stdout should be json")
}

struct Fixture {
    temp: TempDir,
    worker_id: String,
    worker_token: String,
}

fn fixture() -> Fixture {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("work.txt"), "ready\n").unwrap();
    run_json(rive_cmd().arg("init").arg(temp.path()));
    let agent = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("agent")
            .arg("add")
            .arg("worker-a")
            .arg("--role")
            .arg("worker")
            .arg("--token")
            .arg("worker-token"),
    );
    Fixture {
        temp,
        worker_id: agent["protocol"]["agent"]["agent_id"]
            .as_str()
            .unwrap()
            .to_string(),
        worker_token: agent["protocol"]["token"].as_str().unwrap().to_string(),
    }
}

fn capture_snapshot(temp: &TempDir) -> String {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("snapshot")
            .arg("capture")
            .arg("--path")
            .arg(temp.path()),
    )["protocol"]["snapshot_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn create_dispatch(temp: &TempDir, command_id: &str) -> String {
    run_json_with_stdin(
        rive_cmd()
            .current_dir(temp.path())
            .arg("dispatch")
            .arg("create")
            .arg("--target")
            .arg("worker-a")
            .arg("--title")
            .arg("check X")
            .arg("--command-id")
            .arg(command_id)
            .arg("--stdin"),
        "Please check X.\n",
    )["protocol"]["dispatch_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn worker_can_report_assigned_dispatch_with_evidence() {
    let fixture = fixture();
    let dispatch_id = create_dispatch(&fixture.temp, "create-1");
    let snapshot_id = capture_snapshot(&fixture.temp);

    let report = run_json_with_stdin(
        team_cmd()
            .env("RIVE_WORKSPACE", fixture.temp.path())
            .env("RIVE_AGENT_ID", &fixture.worker_id)
            .env("RIVE_AGENT_TOKEN", &fixture.worker_token)
            .arg("report")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--status")
            .arg("done")
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--command-id")
            .arg("report-1")
            .arg("--stdin"),
        "Done with evidence.\n",
    );
    assert_eq!(report["protocol"]["dispatch"]["state"], "reported");
    assert_eq!(report["protocol"]["fact"]["fact_type"], "report");
    assert_eq!(report["protocol"]["fact"]["idempotency_status"], "inserted");

    let show = run_json(
        rive_cmd()
            .current_dir(fixture.temp.path())
            .arg("dispatch")
            .arg("show")
            .arg(&dispatch_id),
    );
    assert_eq!(show["protocol"]["state"], "reported");
    assert_eq!(show["protocol"]["latest_report_status"], "done");
    assert_eq!(
        show["protocol"]["latest_fact_event_id"],
        report["protocol"]["fact"]["event_id"]
    );
}

#[test]
fn invalid_branch_workspace_ref_is_rejected_before_fact_write() {
    let fixture = fixture();
    let dispatch_id = create_dispatch(&fixture.temp, "create-invalid-worktree-ref");
    let snapshot_id = capture_snapshot(&fixture.temp);

    let error = run_json_expect_error(
        team_cmd()
            .env("RIVE_WORKSPACE", fixture.temp.path())
            .env("RIVE_AGENT_ID", &fixture.worker_id)
            .env("RIVE_AGENT_TOKEN", &fixture.worker_token)
            .arg("report")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--status")
            .arg("done")
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--workspace-ref")
            .arg("git-worktree:missing:branch")
            .arg("--command-id")
            .arg("report-invalid-worktree-ref")
            .arg("--stdin"),
        Some("Done with invalid worktree ref.\n"),
    );
    assert_eq!(error["protocol"]["code"], "worktree_not_found");

    let show = run_json(
        rive_cmd()
            .current_dir(fixture.temp.path())
            .arg("dispatch")
            .arg("show")
            .arg(&dispatch_id),
    );
    assert_eq!(show["protocol"]["state"], "open");
    let conn = Connection::open(fixture.temp.path().join(".rive/rive.db")).unwrap();
    let facts: i64 = conn
        .query_row("select count(*) from facts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(facts, 0);
}

#[test]
fn status_update_does_not_close_dispatch() {
    let fixture = fixture();
    let dispatch_id = create_dispatch(&fixture.temp, "create-status");
    let snapshot_id = capture_snapshot(&fixture.temp);

    let status = run_json_with_stdin(
        team_cmd()
            .env("RIVE_WORKSPACE", fixture.temp.path())
            .env("RIVE_AGENT_ID", &fixture.worker_id)
            .env("RIVE_AGENT_TOKEN", &fixture.worker_token)
            .arg("status")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--command-id")
            .arg("status-1")
            .arg("--stdin"),
        "Still working.\n",
    );
    assert_eq!(status["protocol"]["dispatch"]["state"], "open");
    assert_eq!(status["protocol"]["fact"]["fact_type"], "status");
}

#[test]
fn non_assigned_worker_and_closed_dispatch_are_rejected() {
    let fixture = fixture();
    let other = run_json(
        rive_cmd()
            .current_dir(fixture.temp.path())
            .arg("agent")
            .arg("add")
            .arg("worker-b")
            .arg("--role")
            .arg("worker")
            .arg("--token")
            .arg("other-token"),
    );
    let other_id = other["protocol"]["agent"]["agent_id"].as_str().unwrap();
    let dispatch_id = create_dispatch(&fixture.temp, "create-closed");
    let snapshot_id = capture_snapshot(&fixture.temp);

    let error = run_json_expect_error(
        team_cmd()
            .env("RIVE_WORKSPACE", fixture.temp.path())
            .env("RIVE_AGENT_ID", other_id)
            .env("RIVE_AGENT_TOKEN", "other-token")
            .arg("report")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--status")
            .arg("done")
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--command-id")
            .arg("wrong-worker")
            .arg("--stdin"),
        Some("Nope.\n"),
    );
    assert_eq!(error["protocol"]["code"], "dispatch_not_assigned");

    run_json_with_stdin(
        team_cmd()
            .env("RIVE_WORKSPACE", fixture.temp.path())
            .env("RIVE_AGENT_ID", &fixture.worker_id)
            .env("RIVE_AGENT_TOKEN", &fixture.worker_token)
            .arg("report")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--status")
            .arg("done")
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--command-id")
            .arg("close-once")
            .arg("--stdin"),
        "Done.\n",
    );
    let closed = run_json_expect_error(
        team_cmd()
            .env("RIVE_WORKSPACE", fixture.temp.path())
            .env("RIVE_AGENT_ID", &fixture.worker_id)
            .env("RIVE_AGENT_TOKEN", &fixture.worker_token)
            .arg("status")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--command-id")
            .arg("closed-status")
            .arg("--stdin"),
        Some("Still doing things.\n"),
    );
    assert_eq!(closed["protocol"]["code"], "dispatch_closed");
}

#[test]
fn command_id_replay_and_conflict_are_enforced() {
    let fixture = fixture();
    let dispatch_id = create_dispatch(&fixture.temp, "create-idem");
    let snapshot_id = capture_snapshot(&fixture.temp);

    let first = run_json_with_stdin(
        team_cmd()
            .env("RIVE_WORKSPACE", fixture.temp.path())
            .env("RIVE_AGENT_ID", &fixture.worker_id)
            .env("RIVE_AGENT_TOKEN", &fixture.worker_token)
            .arg("status")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--command-id")
            .arg("status-repeat")
            .arg("--stdin"),
        "Same body.\n",
    );
    let replay = run_json_with_stdin(
        team_cmd()
            .env("RIVE_WORKSPACE", fixture.temp.path())
            .env("RIVE_AGENT_ID", &fixture.worker_id)
            .env("RIVE_AGENT_TOKEN", &fixture.worker_token)
            .arg("status")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--command-id")
            .arg("status-repeat")
            .arg("--stdin"),
        "Same body.\n",
    );
    assert_eq!(
        first["protocol"]["fact"]["event_id"],
        replay["protocol"]["fact"]["event_id"]
    );
    assert_eq!(replay["protocol"]["fact"]["idempotency_status"], "replayed");

    let conflict = run_json_expect_error(
        team_cmd()
            .env("RIVE_WORKSPACE", fixture.temp.path())
            .env("RIVE_AGENT_ID", &fixture.worker_id)
            .env("RIVE_AGENT_TOKEN", &fixture.worker_token)
            .arg("status")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--command-id")
            .arg("status-repeat")
            .arg("--stdin"),
        Some("Different body.\n"),
    );
    assert_eq!(conflict["protocol"]["code"], "idempotency_conflict");

    run_json_with_stdin(
        team_cmd()
            .env("RIVE_WORKSPACE", fixture.temp.path())
            .env("RIVE_AGENT_ID", &fixture.worker_id)
            .env("RIVE_AGENT_TOKEN", &fixture.worker_token)
            .arg("report")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--status")
            .arg("done")
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--command-id")
            .arg("close-after-status")
            .arg("--stdin"),
        "Done.\n",
    );
    let replay_after_close = run_json_with_stdin(
        team_cmd()
            .env("RIVE_WORKSPACE", fixture.temp.path())
            .env("RIVE_AGENT_ID", &fixture.worker_id)
            .env("RIVE_AGENT_TOKEN", &fixture.worker_token)
            .arg("status")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--command-id")
            .arg("status-repeat")
            .arg("--stdin"),
        "Same body.\n",
    );
    assert_eq!(
        replay_after_close["protocol"]["fact"]["event_id"],
        first["protocol"]["fact"]["event_id"]
    );
    assert_eq!(
        replay_after_close["protocol"]["fact"]["idempotency_status"],
        "replayed"
    );
}

#[test]
fn dispatch_cancel_blocks_later_report_and_does_not_create_graph_state() {
    let fixture = fixture();
    let dispatch_id = create_dispatch(&fixture.temp, "create-cancel");
    let snapshot_id = capture_snapshot(&fixture.temp);

    let cancel = run_json(
        rive_cmd()
            .current_dir(fixture.temp.path())
            .arg("dispatch")
            .arg("cancel")
            .arg(&dispatch_id)
            .arg("--command-id")
            .arg("cancel-1")
            .arg("--reason")
            .arg("human stopped it"),
    );
    assert_eq!(cancel["protocol"]["state"], "cancelled");

    let cancel_replay = run_json(
        rive_cmd()
            .current_dir(fixture.temp.path())
            .arg("dispatch")
            .arg("cancel")
            .arg(&dispatch_id)
            .arg("--command-id")
            .arg("cancel-1")
            .arg("--reason")
            .arg("human stopped it"),
    );
    assert_eq!(cancel_replay["protocol"]["state"], "cancelled");
    assert_eq!(cancel_replay["protocol"]["idempotency_status"], "replayed");

    let cancel_conflict = run_json_expect_error(
        rive_cmd()
            .current_dir(fixture.temp.path())
            .arg("dispatch")
            .arg("cancel")
            .arg(&dispatch_id)
            .arg("--command-id")
            .arg("cancel-1")
            .arg("--reason")
            .arg("different reason"),
        None,
    );
    assert_eq!(cancel_conflict["protocol"]["code"], "idempotency_conflict");

    let error = run_json_expect_error(
        team_cmd()
            .env("RIVE_WORKSPACE", fixture.temp.path())
            .env("RIVE_AGENT_ID", &fixture.worker_id)
            .env("RIVE_AGENT_TOKEN", &fixture.worker_token)
            .arg("report")
            .arg("--dispatch")
            .arg(&dispatch_id)
            .arg("--status")
            .arg("done")
            .arg("--snapshot")
            .arg(&snapshot_id)
            .arg("--command-id")
            .arg("after-cancel")
            .arg("--stdin"),
        Some("Done anyway.\n"),
    );
    assert_eq!(error["protocol"]["code"], "dispatch_closed");

    let conn = Connection::open(fixture.temp.path().join(".rive/rive.db")).unwrap();
    let graph_tables: i64 = conn
        .query_row(
            "select count(*) from sqlite_master where type = 'table' and name like 'work_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(graph_tables, 0);
}
