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

fn add_worker_agent(temp: &TempDir) -> String {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("agent")
            .arg("add")
            .arg("agent-a")
            .arg("--role")
            .arg("worker")
            .arg("--token")
            .arg("token-a"),
    )["protocol"]["agent"]["agent_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn team_fact_record_binds_snapshot_and_is_queryable() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("work.txt"), "ready\n").unwrap();
    run_json(rive_cmd().arg("init").arg(temp.path()));
    let agent_id = add_worker_agent(&temp);
    let capture = run_json(
        rive_cmd()
            .arg("snapshot")
            .arg("capture")
            .arg("--path")
            .arg(temp.path()),
    );
    let snapshot_id = capture["protocol"]["snapshot_id"].as_str().unwrap();

    let fact = run_json_with_stdin(
        team_cmd()
            .env("RIVE_WORKSPACE", temp.path())
            .env("RIVE_AGENT_ID", &agent_id)
            .env("RIVE_AGENT_TOKEN", "token-a")
            .env("RIVE_RUN_ID", "run-a")
            .arg("fact")
            .arg("record")
            .arg("--type")
            .arg("report")
            .arg("--snapshot")
            .arg(snapshot_id)
            .arg("--command-id")
            .arg("cmd-1")
            .arg("--stdin"),
        "I verified the workspace.\n",
    );
    let event_id = fact["protocol"]["event_id"].as_str().unwrap();
    assert_eq!(fact["protocol"]["fact_type"], "report");
    assert_eq!(fact["protocol"]["actor"]["run_id"], "run-a");
    assert_eq!(fact["protocol"]["idempotency_status"], "inserted");
    assert!(fact["protocol"]["body_hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_eq!(
        fact["protocol"]["evidence_refs"][0]["snapshot_id"].as_str(),
        Some(snapshot_id)
    );

    let show = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("fact")
            .arg("show")
            .arg(event_id),
    );
    assert_eq!(show["protocol"]["event_id"], event_id);
    assert_eq!(show["protocol"]["idempotency_status"], "read");

    let list = run_json(rive_cmd().current_dir(temp.path()).arg("fact").arg("list"));
    assert_eq!(list["protocol"]["facts"].as_array().unwrap().len(), 1);

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let event_count: i64 = conn
        .query_row(
            "select count(*) from events where event_type = 'agent.fact.recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(event_count, 1);
}

#[test]
fn duplicate_command_id_replays_same_fact_and_changed_body_conflicts() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("work.txt"), "ready\n").unwrap();
    run_json(rive_cmd().arg("init").arg(temp.path()));
    let agent_id = add_worker_agent(&temp);
    let capture = run_json(
        rive_cmd()
            .arg("snapshot")
            .arg("capture")
            .arg("--path")
            .arg(temp.path()),
    );
    let snapshot_id = capture["protocol"]["snapshot_id"].as_str().unwrap();

    let base_command = |body: &'static str| {
        run_json_with_stdin(
            team_cmd()
                .env("RIVE_WORKSPACE", temp.path())
                .env("RIVE_AGENT_ID", &agent_id)
                .env("RIVE_AGENT_TOKEN", "token-a")
                .arg("fact")
                .arg("record")
                .arg("--type")
                .arg("status")
                .arg("--snapshot")
                .arg(snapshot_id)
                .arg("--command-id")
                .arg("cmd-repeat")
                .arg("--stdin"),
            body,
        )
    };

    let first = base_command("same body\n");
    let replay = base_command("same body\n");
    assert_eq!(
        first["protocol"]["event_id"],
        replay["protocol"]["event_id"]
    );
    assert_eq!(replay["protocol"]["idempotency_status"], "replayed");

    let error = run_json_expect_error(
        team_cmd()
            .env("RIVE_WORKSPACE", temp.path())
            .env("RIVE_AGENT_ID", &agent_id)
            .env("RIVE_AGENT_TOKEN", "token-a")
            .arg("fact")
            .arg("record")
            .arg("--type")
            .arg("status")
            .arg("--snapshot")
            .arg(snapshot_id)
            .arg("--command-id")
            .arg("cmd-repeat")
            .arg("--stdin"),
        Some("different body\n"),
    );
    assert_eq!(error["protocol"]["code"], "idempotency_conflict");
}

#[test]
fn invalid_snapshot_is_rejected() {
    let temp = TempDir::new().unwrap();
    run_json(rive_cmd().arg("init").arg(temp.path()));
    let agent_id = add_worker_agent(&temp);

    let error = run_json_expect_error(
        team_cmd()
            .env("RIVE_WORKSPACE", temp.path())
            .env("RIVE_AGENT_ID", &agent_id)
            .env("RIVE_AGENT_TOKEN", "token-a")
            .arg("fact")
            .arg("record")
            .arg("--type")
            .arg("observation")
            .arg("--snapshot")
            .arg("snap_missing")
            .arg("--command-id")
            .arg("cmd-missing")
            .arg("--stdin"),
        Some("body\n"),
    );
    assert_eq!(error["protocol"]["code"], "evidence_not_found");
}

#[test]
fn cross_workspace_snapshot_is_rejected() {
    let source = TempDir::new().unwrap();
    fs::write(source.path().join("source.txt"), "source\n").unwrap();
    run_json(rive_cmd().arg("init").arg(source.path()));
    let capture = run_json(
        rive_cmd()
            .arg("snapshot")
            .arg("capture")
            .arg("--path")
            .arg(source.path()),
    );
    let source_snapshot_id = capture["protocol"]["snapshot_id"].as_str().unwrap();

    let target = TempDir::new().unwrap();
    run_json(rive_cmd().arg("init").arg(target.path()));
    let agent_id = add_worker_agent(&target);
    let error = run_json_expect_error(
        team_cmd()
            .env("RIVE_WORKSPACE", target.path())
            .env("RIVE_AGENT_ID", &agent_id)
            .env("RIVE_AGENT_TOKEN", "token-a")
            .arg("fact")
            .arg("record")
            .arg("--type")
            .arg("report")
            .arg("--snapshot")
            .arg(source_snapshot_id)
            .arg("--command-id")
            .arg("cmd-cross")
            .arg("--stdin"),
        Some("body\n"),
    );
    assert_eq!(error["protocol"]["code"], "evidence_not_found");
}

#[test]
fn manifest_integrity_error_is_rejected() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("work.txt"), "ready\n").unwrap();
    run_json(rive_cmd().arg("init").arg(temp.path()));
    let agent_id = add_worker_agent(&temp);
    let capture = run_json(
        rive_cmd()
            .arg("snapshot")
            .arg("capture")
            .arg("--path")
            .arg(temp.path()),
    );
    let snapshot_id = capture["protocol"]["snapshot_id"].as_str().unwrap();

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let manifest_path: String = conn
        .query_row(
            "select manifest_path from snapshots where snapshot_id = ?1",
            [snapshot_id],
            |row| row.get(0),
        )
        .unwrap();
    fs::write(temp.path().join(manifest_path), "{ tampered").unwrap();

    let error = run_json_expect_error(
        team_cmd()
            .env("RIVE_WORKSPACE", temp.path())
            .env("RIVE_AGENT_ID", &agent_id)
            .env("RIVE_AGENT_TOKEN", "token-a")
            .arg("fact")
            .arg("record")
            .arg("--type")
            .arg("report")
            .arg("--snapshot")
            .arg(snapshot_id)
            .arg("--command-id")
            .arg("cmd-integrity")
            .arg("--stdin"),
        Some("body\n"),
    );
    assert_eq!(error["protocol"]["code"], "evidence_integrity_error");
}
