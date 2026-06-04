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

fn init_git_workspace() -> TempDir {
    let temp = TempDir::new().unwrap();
    run_git(&temp, ["init"]);
    fs::write(temp.path().join(".gitignore"), ".rive/\n").unwrap();
    fs::write(temp.path().join("base.txt"), "base\n").unwrap();
    run_git(&temp, ["add", ".gitignore", "base.txt"]);
    run_git(
        &temp,
        [
            "-c",
            "user.name=Rive Test",
            "-c",
            "user.email=rive@example.test",
            "commit",
            "-m",
            "base",
        ],
    );
    run_json(rive_cmd().arg("init").arg(temp.path()));
    temp
}

fn run_git<const N: usize>(temp: &TempDir, args: [&str; N]) {
    let output = Command::new("git")
        .current_dir(temp.path())
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("command should spawn");
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<Value>(&output.stdout).unwrap()["protocol"]["work_node_id"]
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

fn accept_work_require_branch(temp: &TempDir, work: &str, command_id: &str) -> Value {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("accept")
            .arg(work)
            .arg("--require-committed-branch")
            .arg("--command-id")
            .arg(command_id),
    )
}

fn accept_work_require_branch_expect_error(temp: &TempDir, work: &str, command_id: &str) -> Value {
    run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("accept")
            .arg(work)
            .arg("--require-committed-branch")
            .arg("--command-id")
            .arg(command_id),
    )
}

fn branch_commit(temp: &TempDir, integration_id: &str, command_id: &str) -> Value {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .env("RIVE_WORKSPACE_BACKEND", "local-fake")
            .arg("branch")
            .arg("commit")
            .arg(integration_id)
            .arg("--command-id")
            .arg(command_id),
    )
}

fn branch_commit_default(temp: &TempDir, integration_id: &str, command_id: &str) -> Value {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("branch")
            .arg("commit")
            .arg(integration_id)
            .arg("--command-id")
            .arg(command_id),
    )
}

fn branch_abort(temp: &TempDir, integration_id: &str, command_id: &str) -> Value {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .env("RIVE_WORKSPACE_BACKEND", "local-fake")
            .arg("branch")
            .arg("abort")
            .arg(integration_id)
            .arg("--command-id")
            .arg(command_id),
    )
}

fn branch_reject(temp: &TempDir, integration_id: &str, command_id: &str, reason: &str) -> Value {
    let mut command = rive_cmd();
    command
        .current_dir(temp.path())
        .arg("branch")
        .arg("reject")
        .arg(integration_id)
        .arg("--command-id")
        .arg(command_id)
        .arg("--stdin");
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("command should spawn");
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(reason.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn branch_conflict_show(temp: &TempDir, id: &str) -> Value {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("branch")
            .arg("conflict")
            .arg("show")
            .arg(id),
    )
}

fn branch_conflict_reject(
    temp: &TempDir,
    conflict_id: &str,
    command_id: &str,
    reason: &str,
) -> Value {
    let mut command = rive_cmd();
    command
        .current_dir(temp.path())
        .arg("branch")
        .arg("conflict")
        .arg("reject")
        .arg(conflict_id)
        .arg("--command-id")
        .arg(command_id)
        .arg("--stdin");
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("command should spawn");
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(reason.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn branch_conflict_retry_from_parent(
    temp: &TempDir,
    conflict_id: &str,
    command_id: &str,
    fake: &Path,
) -> Value {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("branch")
            .arg("conflict")
            .arg("retry-from-parent")
            .arg(conflict_id)
            .arg("--worker")
            .arg("worker-a")
            .arg("--worker")
            .arg("worker-b")
            .arg("--command-id")
            .arg(command_id)
            .arg("--opencode-bin")
            .arg(fake)
            .arg("--timeout-seconds")
            .arg("10"),
    )
}

fn first_branch_integration_id(temp: &TempDir) -> String {
    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    conn.query_row(
        "select integration_id from branch_integrations order by created_at limit 1",
        [],
        |row| row.get::<_, String>(0),
    )
    .unwrap()
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

fn branch_scheduler_command(
    temp: &TempDir,
    fake: &Path,
    root: &str,
    command_id: &str,
    acceptance_mode: &str,
) -> Command {
    let mut command = scheduler_command_with_mode(temp, fake, root, command_id, acceptance_mode);
    command.arg("--workspace-mode").arg("worktree");
    command.env("RIVE_WORKSPACE_BACKEND", "local-fake");
    command
}

fn real_worktree_scheduler_command_without_path(
    temp: &TempDir,
    fake: &Path,
    root: &str,
    command_id: &str,
) -> Command {
    let mut command = scheduler_command_with_mode(temp, fake, root, command_id, "manual");
    command.arg("--workspace-mode").arg("worktree");
    command.env("PATH", "/nonexistent");
    command
}

fn scheduler_command(temp: &TempDir, fake: &Path, root: &str, command_id: &str) -> Command {
    scheduler_command_with_mode(temp, fake, root, command_id, "auto-reported")
}

fn scheduler_resume_command(
    temp: &TempDir,
    fake: &Path,
    scheduler_run_id: &str,
    command_id: &str,
    acceptance_mode: &str,
) -> Command {
    let mut command = rive_cmd();
    command
        .current_dir(temp.path())
        .arg("scheduler")
        .arg("resume")
        .arg("--run")
        .arg(scheduler_run_id)
        .arg("--worker")
        .arg("worker-a")
        .arg("--worker")
        .arg("worker-b")
        .arg("--command-id")
        .arg(command_id)
        .arg("--acceptance-mode")
        .arg(acceptance_mode)
        .arg("--opencode-bin")
        .arg(fake)
        .arg("--timeout-seconds")
        .arg("10");
    command
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

fn write_certificate_error_no_report_worker(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
printf '{"type":"error","message":"unknown certificate verification error"}\n'
"#,
    );
}

fn write_branch_worker(path: &Path) {
    let rive_bin = env!("CARGO_BIN_EXE_rive");
    let team_bin = env!("CARGO_BIN_EXE_team");
    write_executable(
        path,
        &format!(
            r#"#!/bin/sh
set -eu
test -n "$RIVE_WORKSPACE_REF"
test -n "$RIVE_STATE_WORKSPACE"
printf '%s\n' "$RIVE_WORKSPACE" >> "$RIVE_STATE_WORKSPACE/.rive/phase12-branch-paths.txt"
printf 'parent=%s\nstate=%s\nbranch=%s\n' "$RIVE_STATE_WORKSPACE" "$RIVE_STATE_WORKSPACE" "$RIVE_WORKSPACE_REF" > "$RIVE_WORKSPACE/phase12-branch-result.txt"
SNAPSHOT_ID=$("{rive_bin}" snapshot capture --path "$RIVE_WORKSPACE/phase12-branch-result.txt" --label phase12-branch-worker --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'worktree worker done\n' | "{team_bin}" report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --workspace-ref "$RIVE_WORKSPACE_REF" --command-id "phase12-report-$RIVE_RUN_ID" --stdin >/dev/null
printf '{{"type":"step_finish","tokens":{{"input":5,"output":3,"reasoning":0,"cache":{{"read":1}},"total":9}}}}\n'
"#
        ),
    );
}

fn write_branch_modify_delete_worker(path: &Path) {
    let rive_bin = env!("CARGO_BIN_EXE_rive");
    let team_bin = env!("CARGO_BIN_EXE_team");
    write_executable(
        path,
        &format!(
            r#"#!/bin/sh
set -eu
test -n "$RIVE_WORKSPACE_REF"
test -n "$RIVE_STATE_WORKSPACE"
printf 'modified in branch\n' > "$RIVE_WORKSPACE/modify-me.txt"
rm "$RIVE_WORKSPACE/delete-me.txt"
SNAPSHOT_ID=$("{rive_bin}" snapshot capture --path "$RIVE_WORKSPACE/modify-me.txt" --label phase12-branch-modify-delete --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'worktree worker modified and deleted\n' | "{team_bin}" report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --workspace-ref "$RIVE_WORKSPACE_REF" --command-id "phase12-moddel-report-$RIVE_RUN_ID" --stdin >/dev/null
"#
        ),
    );
}

fn write_branch_conflict_worker(path: &Path) {
    let rive_bin = env!("CARGO_BIN_EXE_rive");
    let team_bin = env!("CARGO_BIN_EXE_team");
    write_executable(
        path,
        &format!(
            r#"#!/bin/sh
set -eu
printf 'worker change\n' > "$RIVE_WORKSPACE/conflict.txt"
SNAPSHOT_ID=$("{rive_bin}" snapshot capture --path "$RIVE_WORKSPACE/conflict.txt" --label phase16-conflict-worker --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'conflict worker done\n' | "{team_bin}" report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --workspace-ref "$RIVE_WORKSPACE_REF" --command-id "phase16-conflict-report-$RIVE_RUN_ID" --stdin >/dev/null
"#
        ),
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
fn scheduler_classifies_no_report_runner_stdout_errors() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase16-cert-root", "root");
    let node = create_work(&temp, "phase16-cert-node", "certificate probe");
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &node,
        "edge-phase16-cert-root-node",
    );

    let fake = temp.path().join("fake-opencode-certificate-no-report");
    write_certificate_error_no_report_worker(&fake);
    let error = run_json_expect_error(&mut scheduler_command(
        &temp,
        &fake,
        &root,
        "phase16-cert-no-report",
    ));
    assert_eq!(error["protocol"]["code"], "dispatch_not_reported");

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let (failure_kind, retryable, suggested_action, detail): (String, bool, String, String) = conn
        .query_row(
            "select failure_kind, retryable, suggested_action, detail from scheduler_node_failures",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(failure_kind, "certificate_error");
    assert!(retryable);
    assert_eq!(suggested_action, "retry_after_certificate_fix");
    assert!(detail.contains("unknown certificate verification error"));
    assert!(detail.contains("dispatch not reported"));

    let status = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("scheduler")
            .arg("status")
            .arg("--root")
            .arg(&root),
    );
    let node_run = &status["protocol"]["node_runs"][0];
    assert_eq!(node_run["failure"]["failure_kind"], "certificate_error");
    assert!(node_run["activity"]["stdout_tail"]
        .as_str()
        .unwrap()
        .contains("unknown certificate verification error"));
    assert!(node_run["activity"]["stdout_ref"]
        .as_str()
        .unwrap()
        .contains(".rive/debug/runs/"));
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

#[test]
fn branch_scheduler_manual_creates_pending_integration_without_parent_write() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase12-manual-root", "root");
    let a = create_work(&temp, "phase12-manual-a", "A");
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &a,
        "edge-phase12-manual-root-a",
    );
    let fake = temp.path().join("fake-opencode-branch");
    write_branch_worker(&fake);

    let response = run_json(&mut branch_scheduler_command(
        &temp,
        &fake,
        &root,
        "phase12-branch-manual",
        "manual",
    ));
    assert_eq!(response["protocol"]["scheduler"]["state"], "waiting_review");
    assert_eq!(
        inspect_work(&temp, &a)["protocol"]["projection"]["state"],
        "reviewable"
    );
    assert!(!temp.path().join("phase12-branch-result.txt").exists());
    let paths = fs::read_to_string(temp.path().join(".rive/phase12-branch-paths.txt")).unwrap();
    assert!(paths.contains(".rive/worktrees/"));
    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let pending: i64 = conn
        .query_row(
            "select count(*) from branch_integrations where state='pending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 1);
}

#[test]
fn worktree_scheduler_uses_real_git_worktree_and_commit_applies_patch() {
    let temp = init_git_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase12-real-worktree-root", "root");
    let a = create_work_with_body(
        &temp,
        "phase12-real-worktree-a",
        "A",
        "Body acceptance: write phase12 branch result only inside the active editable workspace.",
    );
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &a,
        "edge-phase12-real-worktree-root-a",
    );
    let fake = temp.path().join("fake-opencode-real-worktree");
    write_branch_worker(&fake);

    let mut command =
        scheduler_command_with_mode(&temp, &fake, &root, "phase12-real-worktree", "manual");
    command.arg("--workspace-mode").arg("worktree");
    let response = run_json(&mut command);
    assert_eq!(response["protocol"]["scheduler"]["state"], "waiting_review");
    assert_eq!(
        inspect_work(&temp, &a)["protocol"]["projection"]["state"],
        "reviewable"
    );
    assert!(!temp.path().join("phase12-branch-result.txt").exists());

    let paths = fs::read_to_string(temp.path().join(".rive/phase12-branch-paths.txt")).unwrap();
    assert!(paths.contains(".rive/worktrees/"));
    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let (integration_id, branch_path, backend, worker_run_id, branch_ref): (
        String,
        String,
        String,
        String,
        String,
    ) = conn
        .query_row(
            "select i.integration_id, b.branch_path, b.backend, sn.worker_run_id, b.branch_ref from branch_integrations i join branch_workspaces b on b.branch_id=i.branch_id join scheduler_node_runs sn on sn.dispatch_id=i.dispatch_id order by i.created_at limit 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(backend, "git-worktree");
    assert!(Path::new(&branch_path).exists());
    drop(conn);
    let prompt = fs::read_to_string(
        temp.path()
            .join(".rive/debug/runs")
            .join(&worker_run_id)
            .join("prompt.txt"),
    )
    .unwrap();
    let state_root = temp.path().canonicalize().unwrap();
    assert!(prompt.contains(&format!("editable_root: {branch_path}")));
    assert!(prompt.contains(&format!("state_root: {}", state_root.display())));
    assert!(prompt.contains("Make all source/artifact edits there"));
    assert!(prompt.contains("Use it only implicitly through `rive`/`team` commands"));
    assert!(prompt.contains(
        "Body acceptance: write phase12 branch result only inside the active editable workspace."
    ));
    assert!(prompt.contains(&format!("ref: {branch_ref}")));
    assert!(prompt.contains("include `--workspace-ref \"$RIVE_WORKSPACE_REF\"`"));

    let commit = branch_commit_default(&temp, &integration_id, "phase12-real-worktree-commit");
    assert_eq!(commit["protocol"]["integration"]["state"], "committed");
    assert!(commit["protocol"]["integration"]["commit_ref"]
        .as_str()
        .unwrap()
        .starts_with("git-worktree-apply:"));
    let parent_result = fs::read_to_string(temp.path().join("phase12-branch-result.txt")).unwrap();
    assert!(parent_result.contains("parent="));
    assert!(parent_result.contains("state="));
    assert!(parent_result.contains(&format!(
        "branch={}",
        commit["protocol"]["integration"]["branch_ref"]
            .as_str()
            .unwrap()
    )));
    assert!(!Path::new(&branch_path).exists());

    let accepted = accept_work_require_branch(&temp, &a, "phase12-real-worktree-accept");
    assert_eq!(accepted["protocol"]["status_input"], "active");
    assert_eq!(
        inspect_work(&temp, &a)["protocol"]["projection"]["state"],
        "done"
    );
}

#[test]
fn worktree_branch_starts_from_current_parent_workspace_state() {
    let temp = init_git_workspace();
    fs::write(
        temp.path().join("accepted-parent.txt"),
        "accepted upstream\n",
    )
    .unwrap();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase16-worktree-baseline-root", "root");
    let a = create_work(
        &temp,
        "phase16-worktree-baseline-a",
        "A sees accepted parent state",
    );
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &a,
        "edge-phase16-worktree-baseline-root-a",
    );
    let fake = temp.path().join("fake-opencode-worktree-baseline");
    let rive_bin = env!("CARGO_BIN_EXE_rive");
    let team_bin = env!("CARGO_BIN_EXE_team");
    write_executable(
        &fake,
        &format!(
            r#"#!/bin/sh
set -eu
test -f "$RIVE_WORKSPACE/accepted-parent.txt"
cat "$RIVE_WORKSPACE/accepted-parent.txt" > "$RIVE_WORKSPACE/worker-output.txt"
SNAPSHOT_ID=$("{rive_bin}" snapshot capture --path "$RIVE_WORKSPACE/worker-output.txt" --label phase16-worktree-baseline --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'baseline worker done\n' | "{team_bin}" report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --workspace-ref "$RIVE_WORKSPACE_REF" --command-id "phase16-baseline-report-$RIVE_RUN_ID" --stdin >/dev/null
"#
        ),
    );

    let mut command = scheduler_command_with_mode(
        &temp,
        &fake,
        &root,
        "phase16-worktree-baseline",
        "auto-committed",
    );
    command.arg("--workspace-mode").arg("worktree");
    let response = run_json(&mut command);
    assert_eq!(response["protocol"]["scheduler"]["state"], "completed");
    assert_eq!(response["protocol"]["root_work"]["state"], "done");
    assert_eq!(
        fs::read_to_string(temp.path().join("worker-output.txt")).unwrap(),
        "accepted upstream\n"
    );
}

#[test]
fn branch_scheduler_real_backend_missing_is_stable_error() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase12-missing-worktree-root", "root");
    let a = create_work(&temp, "phase12-missing-worktree-a", "A");
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &a,
        "edge-phase12-missing-worktree-root-a",
    );
    let fake = temp.path().join("fake-opencode-missing-worktree");
    write_branch_worker(&fake);

    let error = run_json_expect_error(&mut real_worktree_scheduler_command_without_path(
        &temp,
        &fake,
        &root,
        "phase12-worktree-missing",
    ));
    assert_eq!(error["protocol"]["code"], "worktree_backend_unavailable");
    assert_eq!(
        inspect_work(&temp, &a)["protocol"]["projection"]["state"],
        "ready"
    );
}

#[test]
fn branch_commit_is_required_before_guarded_accept() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase12-commit-root", "root");
    let a = create_work(&temp, "phase12-commit-a", "A");
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &a,
        "edge-phase12-commit-root-a",
    );
    let fake = temp.path().join("fake-opencode-branch-commit");
    write_branch_worker(&fake);

    run_json(&mut branch_scheduler_command(
        &temp,
        &fake,
        &root,
        "phase12-branch-commit-manual",
        "manual",
    ));
    let blocked =
        accept_work_require_branch_expect_error(&temp, &a, "phase12-accept-before-commit");
    assert_eq!(blocked["protocol"]["code"], "worktree_ref_not_committed");

    let integration_id = first_branch_integration_id(&temp);
    let commit = branch_commit(&temp, &integration_id, "phase12-branch-commit");
    assert_eq!(
        commit["protocol"]["integration"]["state"],
        serde_json::Value::String("committed".to_string())
    );
    assert!(temp.path().join("phase12-branch-result.txt").exists());

    let accepted = accept_work_require_branch(&temp, &a, "phase12-accept-after-commit");
    assert_eq!(accepted["protocol"]["status_input"], "active");
    assert_eq!(
        inspect_work(&temp, &a)["protocol"]["projection"]["state"],
        "done"
    );
}

#[test]
fn branch_commit_applies_modified_and_deleted_files() {
    let temp = init_workspace();
    fs::write(temp.path().join("modify-me.txt"), "original\n").unwrap();
    fs::write(temp.path().join("delete-me.txt"), "delete\n").unwrap();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase12-moddel-root", "root");
    let a = create_work(&temp, "phase12-moddel-a", "A");
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &a,
        "edge-phase12-moddel-root-a",
    );
    let fake = temp.path().join("fake-opencode-branch-moddel");
    write_branch_modify_delete_worker(&fake);

    run_json(&mut branch_scheduler_command(
        &temp,
        &fake,
        &root,
        "phase12-branch-moddel-manual",
        "manual",
    ));
    assert_eq!(
        fs::read_to_string(temp.path().join("modify-me.txt")).unwrap(),
        "original\n"
    );
    assert!(temp.path().join("delete-me.txt").exists());

    let integration_id = first_branch_integration_id(&temp);
    branch_commit(&temp, &integration_id, "phase12-branch-moddel-commit");
    assert_eq!(
        fs::read_to_string(temp.path().join("modify-me.txt")).unwrap(),
        "modified in branch\n"
    );
    assert!(!temp.path().join("delete-me.txt").exists());
}

#[test]
fn branch_reject_and_abort_are_explicit_terminal_events() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase12-reject-root", "root");
    let a = create_work(&temp, "phase12-reject-a", "A");
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &a,
        "edge-phase12-reject-root-a",
    );
    let fake = temp.path().join("fake-opencode-branch-reject");
    write_branch_worker(&fake);

    run_json(&mut branch_scheduler_command(
        &temp,
        &fake,
        &root,
        "phase12-branch-reject-manual",
        "manual",
    ));
    let integration_id = first_branch_integration_id(&temp);
    let rejected = branch_reject(&temp, &integration_id, "phase12-branch-reject", "bad patch");
    assert_eq!(
        rejected["protocol"]["integration"]["state"],
        serde_json::Value::String("rejected".to_string())
    );
    let still_blocked =
        accept_work_require_branch_expect_error(&temp, &a, "phase12-accept-after-reject");
    assert_eq!(
        still_blocked["protocol"]["code"],
        "worktree_ref_not_committed"
    );

    let root2 = create_work(&temp, "phase12-abort-root", "root abort");
    let b = create_work(&temp, "phase12-abort-b", "B");
    add_edge(
        &temp,
        "decomposes-to",
        &root2,
        &b,
        "edge-phase12-abort-root-b",
    );
    let fake2 = temp.path().join("fake-opencode-branch-abort");
    write_branch_worker(&fake2);
    run_json(&mut branch_scheduler_command(
        &temp,
        &fake2,
        &root2,
        "phase12-branch-abort-manual",
        "manual",
    ));
    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let abort_integration_id: String = conn
        .query_row(
            "select integration_id from branch_integrations where state='pending' order by created_at limit 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(conn);
    let aborted = branch_abort(&temp, &abort_integration_id, "phase12-branch-abort");
    assert_eq!(
        aborted["protocol"]["integration"]["state"],
        serde_json::Value::String("aborted".to_string())
    );
}

#[test]
fn branch_scheduler_auto_committed_commits_then_accepts() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase12-auto-root", "root");
    let a = create_work(&temp, "phase12-auto-a", "A");
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &a,
        "edge-phase12-auto-root-a",
    );
    let fake = temp.path().join("fake-opencode-branch-auto");
    write_branch_worker(&fake);

    let response = run_json(&mut branch_scheduler_command(
        &temp,
        &fake,
        &root,
        "phase12-branch-auto",
        "auto-committed",
    ));
    assert_eq!(response["protocol"]["scheduler"]["state"], "completed");
    assert_eq!(response["protocol"]["root_work"]["state"], "done");
    assert!(temp.path().join("phase12-branch-result.txt").exists());
    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let committed: i64 = conn
        .query_row(
            "select count(*) from branch_integrations where state='committed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(committed, 1);
}

#[test]
fn scheduler_resume_supersedes_stale_attempt_and_reruns_node() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase13-resume-root", "root");
    let a = create_work(&temp, "phase13-resume-a", "A");
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &a,
        "edge-phase13-resume-root-a",
    );

    let no_report = temp.path().join("fake-opencode-resume-no-report");
    write_no_report_worker(&no_report);
    let failed = run_json_expect_error(&mut scheduler_command_with_mode(
        &temp,
        &no_report,
        &root,
        "phase13-resume-crashed",
        "manual",
    ));
    assert_eq!(failed["protocol"]["code"], "dispatch_not_reported");

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let scheduler_run_id: String = conn
        .query_row(
            "select scheduler_run_id from scheduler_runs where command_id='phase13-resume-crashed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let node_run_id: String = conn
        .query_row(
            "select node_run_id from scheduler_node_runs where scheduler_run_id=?1",
            [&scheduler_run_id],
            |row| row.get(0),
        )
        .unwrap();
    let old_dispatch_id: String = conn
        .query_row(
            "select dispatch_id from scheduler_node_runs where node_run_id=?1",
            [&node_run_id],
            |row| row.get(0),
        )
        .unwrap();
    drop(conn);

    let worker = temp.path().join("fake-opencode-resume-worker");
    write_scheduler_worker(&worker);
    let mut resume = scheduler_resume_command(
        &temp,
        &worker,
        &scheduler_run_id,
        "phase13-resume-command",
        "manual",
    );
    resume.arg("--failed");
    let resumed = run_json(&mut resume);
    assert_eq!(resumed["protocol"]["scheduler"]["state"], "waiting_review");
    assert_eq!(resumed["protocol"]["scheduler"]["child_executed"], true);
    assert_eq!(
        inspect_work(&temp, &a)["protocol"]["projection"]["state"],
        "reviewable"
    );

    let new_scheduler_run_id = resumed["protocol"]["scheduler"]["scheduler_run_id"]
        .as_str()
        .unwrap();
    let status = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("scheduler")
            .arg("status")
            .arg("--run")
            .arg(new_scheduler_run_id),
    );
    assert_eq!(status["protocol"]["root_work"]["state"], "blocked");
    assert_eq!(
        status["protocol"]["active_node_runs"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert!(status["protocol"]["waiting_review_nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node.as_str() == Some(&a)));

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let old_node_state: String = conn
        .query_row(
            "select state from scheduler_node_runs where node_run_id=?1",
            [&node_run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_node_state, "superseded");
    let old_dispatch_state: String = conn
        .query_row(
            "select state from dispatches where dispatch_id=?1",
            [&old_dispatch_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_dispatch_state, "cancelled");
    let reported_dispatches: i64 = conn
        .query_row(
            "select count(*) from dispatches where state='reported'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reported_dispatches, 1);
    let failure_kind: String = conn
        .query_row(
            "select failure_kind from scheduler_node_failures where node_run_id=?1",
            [&node_run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(failure_kind, "dispatch_not_reported");
}

#[test]
fn work_retry_reruns_failed_node_without_manual_ledger_edits() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase16-work-retry-root", "root");
    let a = create_work(&temp, "phase16-work-retry-a", "A");
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &a,
        "edge-phase16-work-retry-root-a",
    );

    let no_report = temp.path().join("fake-opencode-work-retry-no-report");
    write_no_report_worker(&no_report);
    let failed = run_json_expect_error(&mut scheduler_command_with_mode(
        &temp,
        &no_report,
        &root,
        "phase16-work-retry-failed",
        "manual",
    ));
    assert_eq!(failed["protocol"]["code"], "dispatch_not_reported");

    let worker = temp.path().join("fake-opencode-work-retry-worker");
    write_scheduler_worker(&worker);
    let retried = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("retry")
            .arg(&a)
            .arg("--worker")
            .arg("worker-a")
            .arg("--worker")
            .arg("worker-b")
            .arg("--command-id")
            .arg("phase16-work-retry-command")
            .arg("--acceptance-mode")
            .arg("manual")
            .arg("--opencode-bin")
            .arg(&worker)
            .arg("--timeout-seconds")
            .arg("10"),
    );
    assert_eq!(retried["protocol"]["scheduler"]["state"], "waiting_review");
    assert_eq!(
        inspect_work(&temp, &a)["protocol"]["projection"]["state"],
        "reviewable"
    );

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let superseded: i64 = conn
        .query_row(
            "select count(*) from scheduler_node_runs where state='superseded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(superseded, 1);
    let cancelled: i64 = conn
        .query_row(
            "select count(*) from dispatches where state='cancelled'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cancelled, 1);
    let reported: i64 = conn
        .query_row(
            "select count(*) from dispatches where state='reported'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reported, 1);
}

#[test]
fn worktree_commit_conflict_records_read_model_and_rejects_safely() {
    let temp = init_git_workspace();
    fs::write(temp.path().join("conflict.txt"), "base\n").unwrap();
    run_git(&temp, ["add", "conflict.txt"]);
    run_git(&temp, ["commit", "-m", "add conflict fixture"]);
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase16-conflict-root", "root");
    let a = create_work(&temp, "phase16-conflict-a", "A");
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &a,
        "edge-phase16-conflict-root-a",
    );
    let fake = temp.path().join("fake-opencode-conflict");
    write_branch_conflict_worker(&fake);

    let mut command =
        scheduler_command_with_mode(&temp, &fake, &root, "phase16-conflict-scheduler", "manual");
    command.arg("--workspace-mode").arg("worktree");
    let response = run_json(&mut command);
    assert_eq!(response["protocol"]["scheduler"]["state"], "waiting_review");
    fs::write(temp.path().join("conflict.txt"), "parent current\n").unwrap();

    let integration_id = first_branch_integration_id(&temp);
    let error = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("branch")
            .arg("commit")
            .arg(&integration_id)
            .arg("--command-id")
            .arg("phase16-conflict-commit"),
    );
    assert_eq!(error["protocol"]["code"], "worktree_patch_conflict");
    assert_eq!(
        fs::read_to_string(temp.path().join("conflict.txt")).unwrap(),
        "parent current\n"
    );

    let conflict = branch_conflict_show(&temp, &integration_id);
    assert_eq!(
        conflict["protocol"]["conflict"]["integration_id"],
        serde_json::Value::String(integration_id.clone())
    );
    assert!(conflict["protocol"]["conflict"]["conflict_files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file.as_str() == Some("conflict.txt")));
    assert!(conflict["protocol"]["conflict"]["business_conflict_files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file.as_str() == Some("conflict.txt")));
    assert_eq!(
        conflict["protocol"]["conflict"]["runtime_conflict_files"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert!(conflict["protocol"]["conflict"]["suggested_actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action.as_str() == Some("retry-from-parent")));
    let conflict_id = conflict["protocol"]["conflict"]["conflict_id"]
        .as_str()
        .unwrap()
        .to_string();
    let rejected = branch_conflict_reject(
        &temp,
        &conflict_id,
        "phase16-conflict-reject",
        "keep parent current",
    );
    assert_eq!(rejected["protocol"]["conflict"]["state"], "rejected");
    assert_eq!(rejected["protocol"]["integration"]["state"], "rejected");
}

#[test]
fn branch_conflict_retry_from_parent_rejects_and_reruns_work() {
    let temp = init_git_workspace();
    fs::write(temp.path().join("conflict.txt"), "base\n").unwrap();
    run_git(&temp, ["add", "conflict.txt"]);
    run_git(&temp, ["commit", "-m", "add retry conflict fixture"]);
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let root = create_work(&temp, "phase17-conflict-retry-root", "root");
    let a = create_work(&temp, "phase17-conflict-retry-a", "A");
    add_edge(
        &temp,
        "decomposes-to",
        &root,
        &a,
        "edge-phase17-conflict-retry-root-a",
    );
    let fake = temp.path().join("fake-opencode-conflict-retry");
    write_branch_conflict_worker(&fake);

    let mut command = scheduler_command_with_mode(
        &temp,
        &fake,
        &root,
        "phase17-conflict-retry-scheduler",
        "manual",
    );
    command.arg("--workspace-mode").arg("worktree");
    let response = run_json(&mut command);
    assert_eq!(response["protocol"]["scheduler"]["state"], "waiting_review");
    fs::write(temp.path().join("conflict.txt"), "parent current\n").unwrap();

    let integration_id = first_branch_integration_id(&temp);
    let error = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("branch")
            .arg("commit")
            .arg(&integration_id)
            .arg("--command-id")
            .arg("phase17-conflict-retry-commit"),
    );
    assert_eq!(error["protocol"]["code"], "worktree_patch_conflict");
    let conflict = branch_conflict_show(&temp, &integration_id);
    let conflict_id = conflict["protocol"]["conflict"]["conflict_id"]
        .as_str()
        .unwrap()
        .to_string();

    let bad_worker_error = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("branch")
            .arg("conflict")
            .arg("retry-from-parent")
            .arg(&conflict_id)
            .arg("--worker")
            .arg("missing-worker")
            .arg("--command-id")
            .arg("phase17-conflict-retry-bad-worker")
            .arg("--opencode-bin")
            .arg(&fake)
            .arg("--timeout-seconds")
            .arg("10"),
    );
    assert_eq!(bad_worker_error["protocol"]["code"], "agent_not_found");
    assert_eq!(
        branch_conflict_show(&temp, &conflict_id)["protocol"]["conflict"]["state"],
        "open"
    );

    let retried = branch_conflict_retry_from_parent(
        &temp,
        &conflict_id,
        "phase17-conflict-retry-from-parent",
        &fake,
    );
    assert_eq!(retried["protocol"]["conflict"]["state"], "rejected");
    assert_eq!(
        retried["protocol"]["rejected_integration"]["state"],
        "rejected"
    );
    assert_eq!(
        retried["protocol"]["scheduler"]["scheduler"]["state"],
        "waiting_review"
    );
    assert_eq!(
        inspect_work(&temp, &a)["protocol"]["projection"]["state"],
        "reviewable"
    );

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let rejected_integrations: i64 = conn
        .query_row(
            "select count(*) from branch_integrations where state='rejected'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rejected_integrations, 1);
    let pending_integrations: i64 = conn
        .query_row(
            "select count(*) from branch_integrations where state='pending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending_integrations, 1);
    let superseded: i64 = conn
        .query_row(
            "select count(*) from scheduler_node_runs where state='superseded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(superseded, 1);
}

#[test]
fn scheduler_status_can_inspect_root_without_scheduler_run() {
    let temp = init_workspace();
    let root = create_work(&temp, "phase13-status-root", "root");
    let status = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("scheduler")
            .arg("status")
            .arg("--root")
            .arg(&root),
    );
    assert!(status["protocol"]["scheduler"].is_null());
    assert_eq!(status["protocol"]["root_work"]["work_node_id"], root);
    assert_eq!(status["protocol"]["root_work"]["state"], "ready");
    assert_eq!(
        status["protocol"]["active_node_runs"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}
