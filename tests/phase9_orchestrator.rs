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
    let rive_bin = env!("CARGO_BIN_EXE_rive");
    let team_bin = env!("CARGO_BIN_EXE_team");
    write_executable(
        path,
        &format!(
            r#"#!/bin/sh
set -eu
if [ -n "${{RIVE_ORCHESTRATOR_ROOT_WORK_ID:-}}" ]; then
  COUNT_FILE="$RIVE_WORKSPACE/phase9-orchestrator-count.txt"
  if [ -f "$COUNT_FILE" ]; then COUNT=$(cat "$COUNT_FILE"); else COUNT=0; fi
  COUNT=$((COUNT + 1))
  printf '%s\n' "$COUNT" > "$COUNT_FILE"
  IMPL=$(printf 'Implement the file change.\n' | "{team_bin}" work create --kind task --title implementation --command-id "phase9-impl-$RIVE_RUN_ID" --stdin | sed -n 's/.*"work_node_id": "\([^"]*\)".*/\1/p' | head -n 1)
  "{team_bin}" work edge add --type decomposes-to --from "$RIVE_ORCHESTRATOR_ROOT_WORK_ID" --to "$IMPL" --command-id "phase9-root-impl-$RIVE_RUN_ID" >/dev/null
  printf 'Create phase9-worker-result.txt and report done.\n' | "{team_bin}" send --work "$IMPL" --to worker --runner opencode --title "implementation" --command-id "phase9-send-$RIVE_RUN_ID" --wait --timeout-seconds 10 --opencode-bin "$0" --stdin >/dev/null
  "{team_bin}" work inspect "$IMPL" >/dev/null
  printf 'implementation evidence accepted\n' | "{team_bin}" work accept "$IMPL" --command-id "phase9-accept-impl-$RIVE_RUN_ID" --stdin >/dev/null
  "{team_bin}" work inspect "$RIVE_ORCHESTRATOR_ROOT_WORK_ID" >/dev/null
  printf 'root accepted\n' | "{team_bin}" work accept "$RIVE_ORCHESTRATOR_ROOT_WORK_ID" --command-id "phase9-accept-root-$RIVE_RUN_ID" --stdin >/dev/null
  printf '{{"final":"RIVE_PHASE9_ORCHESTRATOR_OK"}}\n'
else
  printf 'RIVE_PHASE9_WORKER_OK\n' > "$RIVE_WORKSPACE/phase9-worker-result.txt"
  SNAPSHOT_ID=$("{rive_bin}" snapshot capture --path "$RIVE_WORKSPACE/phase9-worker-result.txt" --label phase9-worker --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
  printf 'worker done\n' | "{team_bin}" report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --artifact-ref "file:phase9-worker-result.txt" --command-id "phase9-worker-report-$RIVE_RUN_ID" --stdin >/dev/null
  printf '{{"final":"RIVE_PHASE9_WORKER_OK"}}\n'
fi
"#
        ),
    );
}

fn write_stdout_only_opencode(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
printf '{"final":"I am done, but I did not use team work accept"}\n'
"#,
    );
}

#[test]
fn team_work_mutations_are_orchestrator_only() {
    let temp = init_workspace();
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

    let error = run_json_expect_error(
        team_cmd()
            .current_dir(temp.path())
            .env("RIVE_WORKSPACE", temp.path())
            .env("RIVE_AGENT_ID", worker_id)
            .env("RIVE_AGENT_TOKEN", "worker-token")
            .arg("work")
            .arg("create")
            .arg("--kind")
            .arg("task")
            .arg("--title")
            .arg("worker mutation")
            .arg("--command-id")
            .arg("worker-create"),
        None,
    );
    assert_eq!(error["protocol"]["code"], "agent_role_not_allowed");
}

#[test]
fn orchestrator_runner_drives_work_dag_with_fake_opencode() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-opencode");
    write_happy_opencode(&fake);

    let response = run_json_with_stdin(
        rive_cmd()
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
            .arg("phase9-orchestrator")
            .arg("--opencode-bin")
            .arg(&fake)
            .arg("--timeout-seconds")
            .arg("20")
            .arg("--stdin"),
        "Create a worker result file and accept the root.\n",
    );

    assert_eq!(response["protocol"]["root_work"]["state"], "done");
    assert_eq!(response["protocol"]["runner"]["child_executed"], true);
    let root = response["protocol"]["runner"]["root_work_node_id"]
        .as_str()
        .unwrap();
    assert_eq!(
        run_json(
            rive_cmd()
                .current_dir(temp.path())
                .arg("work")
                .arg("inspect")
                .arg(root)
        )["protocol"]["projection"]["state"],
        "done"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("phase9-worker-result.txt")).unwrap(),
        "RIVE_PHASE9_WORKER_OK\n"
    );
}

#[test]
fn orchestrator_runner_replay_does_not_relaunch_opencode() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-opencode");
    write_happy_opencode(&fake);
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("agent")
            .arg("add")
            .arg("orchestrator")
            .arg("--role")
            .arg("orchestrator")
            .arg("--token")
            .arg("orchestrator-token"),
    );
    let command = || {
        let mut cmd = rive_cmd();
        cmd.current_dir(temp.path())
            .arg("runner")
            .arg("orchestrator")
            .arg("--runner")
            .arg("opencode")
            .arg("--agent")
            .arg("orchestrator")
            .arg("--agent-token")
            .arg("orchestrator-token")
            .arg("--worker")
            .arg("worker")
            .arg("--command-id")
            .arg("phase9-replay")
            .arg("--opencode-bin")
            .arg(&fake)
            .arg("--timeout-seconds")
            .arg("20")
            .arg("--stdin");
        cmd
    };

    let first = run_json_with_stdin(&mut command(), "Do it once.\n");
    let second = run_json_with_stdin(&mut command(), "Do it once.\n");
    assert_eq!(first["protocol"]["root_work"]["state"], "done");
    assert_eq!(second["protocol"]["root_work"]["state"], "done");
    assert_eq!(second["protocol"]["runner"]["child_executed"], false);
    assert_eq!(
        fs::read_to_string(temp.path().join("phase9-orchestrator-count.txt")).unwrap(),
        "1\n"
    );
}

#[test]
fn orchestrator_stdout_success_without_root_done_is_rejected() {
    let temp = init_workspace();
    let fake = temp.path().join("fake-opencode");
    write_stdout_only_opencode(&fake);
    let error = run_json_expect_error(
        rive_cmd()
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
            .arg("phase9-no-done")
            .arg("--opencode-bin")
            .arg(&fake)
            .arg("--timeout-seconds")
            .arg("20")
            .arg("--stdin"),
        Some("Only stdout claims success.\n"),
    );
    assert_eq!(error["protocol"]["code"], "work_not_done");
}
