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

fn add_worker(temp: &TempDir, name: &str) {
    add_agent(temp, name, "worker");
}

fn add_agent(temp: &TempDir, name: &str, role: &str) {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("agent")
            .arg("add")
            .arg(name)
            .arg("--role")
            .arg(role)
            .arg("--token")
            .arg(format!("{name}-token")),
    );
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn write_fake_opencode_reporter(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
STATE="${RIVE_STATE_WORKSPACE:-$RIVE_WORKSPACE}"
COUNT_FILE="$STATE/workflow-scheduler-invocations.txt"
if [ -f "$COUNT_FILE" ]; then COUNT=$(cat "$COUNT_FILE"); else COUNT=0; fi
COUNT=$((COUNT + 1))
printf '%s\n' "$COUNT" > "$COUNT_FILE"
RESULT="$RIVE_WORKSPACE/workflow-node-$COUNT.txt"
printf 'RIVE_WORKFLOW_NODE_%s_OK\n' "$COUNT" > "$RESULT"
SNAPSHOT_ID=$(rive snapshot capture --path "$RESULT" --label workflow-fake-worker --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'workflow fake worker done\n' | team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --artifact-ref "file:workflow-node-$COUNT.txt" --command-id "workflow-report-$RIVE_RUN_ID" --stdin >/dev/null
printf '{"final":"RIVE_WORKFLOW_FAKE_OK"}\n'
"#,
    );
}

fn write_fake_codex_reporter(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
STATE="${RIVE_STATE_WORKSPACE:-$RIVE_WORKSPACE}"
COUNT_FILE="$STATE/workflow-codex-scheduler-invocations.txt"
if [ -f "$COUNT_FILE" ]; then COUNT=$(cat "$COUNT_FILE"); else COUNT=0; fi
COUNT=$((COUNT + 1))
printf '%s\n' "$COUNT" > "$COUNT_FILE"
RESULT="$RIVE_WORKSPACE/workflow-codex-node-$COUNT.txt"
printf 'RIVE_WORKFLOW_CODEX_NODE_%s_OK\n' "$COUNT" > "$RESULT"
SNAPSHOT_ID=$(rive snapshot capture --path "$RESULT" --label workflow-fake-codex-worker --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'workflow fake codex worker done\n' | team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --artifact-ref "file:workflow-codex-node-$COUNT.txt" --command-id "workflow-codex-report-$RIVE_RUN_ID" --stdin >/dev/null
printf '{"final":"RIVE_WORKFLOW_FAKE_CODEX_OK"}\n'
"#,
    );
}

fn write_fake_codex_reporter_then_hangs(path: &Path) {
    write_executable(
        path,
        r#"#!/bin/sh
set -eu
STATE="${RIVE_STATE_WORKSPACE:-$RIVE_WORKSPACE}"
COUNT_FILE="$STATE/workflow-codex-hanging-invocations.txt"
if [ -f "$COUNT_FILE" ]; then COUNT=$(cat "$COUNT_FILE"); else COUNT=0; fi
COUNT=$((COUNT + 1))
printf '%s\n' "$COUNT" > "$COUNT_FILE"
RESULT="$RIVE_WORKSPACE/workflow-codex-hanging-node-$COUNT.txt"
printf 'RIVE_WORKFLOW_CODEX_HANGING_NODE_%s_OK\n' "$COUNT" > "$RESULT"
SNAPSHOT_ID=$(rive snapshot capture --path "$RESULT" --label workflow-fake-codex-hanging-worker --agent "$RIVE_AGENT_ID" --dispatch "$RIVE_DISPATCH_ID" | sed -n 's/.*"snapshot_id": "\([^"]*\)".*/\1/p' | head -n 1)
printf 'workflow fake codex worker reported before hanging\n' | team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --artifact-ref "file:workflow-codex-hanging-node-$COUNT.txt" --command-id "workflow-codex-hanging-report-$RIVE_RUN_ID" --stdin >/dev/null
sleep 60
"#,
    );
}

fn write_workflow_package(temp: &TempDir) -> std::path::PathBuf {
    let package = temp.path().join("workflow-package");
    fs::create_dir_all(package.join("prompts")).unwrap();
    fs::write(
        package.join("workflow.yaml"),
        r#"api_version: rive.workflow/v0
id: test.workflow
version: 1
title: Test workflow for {{env}}
params:
  env:
    type: enum
    values: [prd, stg]
    default: prd
  slack_channel:
    type: string
    required: true
  allow_slack_post:
    type: boolean
    default: false
nodes:
  scan:
    kind: task
    title: Scan {{env}}
    prompt:
      file: prompts/scan.md
    output_contract:
      format: markdown
      required_sections: [signals]
  judge:
    kind: review
    title: Judge for {{slack_channel}}
    consumes: [scan]
    prompt:
      inline: "Judge {{env}} for {{slack_channel}}"
    capability_policy:
      gated_allow:
        slack.post: "{{allow_slack_post}}"
    output_contract:
      format: markdown
      required_sections: [summary]
edges:
  - type: decomposes_to
    from: root
    to: scan
  - type: decomposes_to
    from: root
    to: judge
  - type: depends_on
    from: judge
    to: scan
"#,
    )
    .unwrap();
    fs::write(package.join("prompts/scan.md"), "Scan {{env}} signals.\n").unwrap();
    package
}

fn write_mixed_runner_workflow(temp: &TempDir) -> std::path::PathBuf {
    let package = temp.path().join("mixed-runner-workflow");
    fs::create_dir_all(&package).unwrap();
    fs::write(
        package.join("workflow.yaml"),
        r#"api_version: rive.workflow/v0
id: mixed.runner.workflow
version: 1
title: Mixed runner workflow
params:
  slack_channel:
    type: string
    required: true
nodes:
  scan:
    kind: task
    runner: opencode
    worker: worker-a
    acceptance_mode: auto-reported
    title: Scan evidence
    prompt:
      inline: "Scan evidence for {{slack_channel}}."
    output_contract:
      format: markdown
  judge:
    kind: review
    runner: codex
    worker: codex-worker
    acceptance_mode: auto-reported
    title: Judge evidence
    consumes: [scan]
    prompt:
      inline: "Judge scan output."
    output_contract:
      format: markdown
edges:
  - type: decomposes_to
    from: root
    to: scan
  - type: decomposes_to
    from: root
    to: judge
  - type: depends_on
    from: judge
    to: scan
"#,
    )
    .unwrap();
    package
}

fn write_bad_node_worker_workflow(temp: &TempDir, worker: &str) -> std::path::PathBuf {
    let path = temp.path().join(format!("bad-node-worker-{worker}.yaml"));
    fs::write(
        &path,
        format!(
            r#"api_version: rive.workflow/v0
id: bad.node.worker.{worker}
version: 1
title: Bad node worker
nodes:
  only:
    kind: task
    runner: opencode
    worker: {worker}
    title: Only
    prompt:
      inline: "Do it."
    output_contract:
      format: markdown
edges:
  - type: decomposes_to
    from: root
    to: only
"#
        ),
    )
    .unwrap();
    path
}

fn write_versioned_workflow(temp: &TempDir, version: i64, title: &str) -> std::path::PathBuf {
    let path = temp.path().join(format!("versioned-{version}.yaml"));
    fs::write(
        &path,
        format!(
            r#"api_version: rive.workflow/v0
id: versioned.workflow
version: {version}
title: {title}
nodes:
  only:
    kind: task
    title: Only v{version}
    prompt:
      inline: "Do one thing in v{version}."
    output_contract:
      format: markdown
edges:
  - type: decomposes_to
    from: root
    to: only
"#
        ),
    )
    .unwrap();
    path
}

fn write_single_file_workflow(temp: &TempDir) -> std::path::PathBuf {
    let path = temp.path().join("single-workflow.yaml");
    fs::write(
        &path,
        r#"api_version: rive.workflow/v0
id: single.workflow
version: 1
title: Single file workflow
nodes:
  only:
    kind: task
    title: Only node
    prompt:
      inline: "Do one thing."
    output_contract:
      format: markdown
edges:
  - type: decomposes_to
    from: root
    to: only
"#,
    )
    .unwrap();
    path
}

fn db_count(temp: &TempDir, table: &str) -> i64 {
    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn db_count_where(temp: &TempDir, table: &str, predicate: &str) -> i64 {
    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    conn.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE {predicate}"),
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn db_text(temp: &TempDir, sql: &str) -> String {
    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}

fn db_execute(temp: &TempDir, sql: &str) {
    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    conn.execute(sql, []).unwrap();
}

fn import_workflow(temp: &TempDir, path: &Path, command_id: &str) -> Value {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("import")
            .arg(path)
            .arg("--command-id")
            .arg(command_id),
    )
}

fn import_workflow_bump_if_changed(temp: &TempDir, path: &Path, command_id: &str) -> Value {
    run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("import")
            .arg(path)
            .arg("--command-id")
            .arg(command_id)
            .arg("--bump-if-changed"),
    )
}

#[test]
fn workflow_import_registers_immutable_template_without_business_side_effects() {
    let temp = init_workspace();
    let package = write_workflow_package(&temp);

    let validated = run_json(rive_cmd().arg("workflow").arg("validate").arg(&package));
    assert_eq!(validated["protocol"]["template_id"], "test.workflow");
    assert_eq!(validated["protocol"]["node_count"], 2);

    let imported = import_workflow(&temp, &package, "wf-import-1");
    assert_eq!(imported["protocol"]["idempotency_status"], "inserted");
    assert_eq!(imported["protocol"]["template_id"], "test.workflow");

    assert_eq!(db_count(&temp, "events"), 0);
    assert_eq!(db_count(&temp, "dispatches"), 0);
    assert_eq!(db_count(&temp, "facts"), 0);
    assert_eq!(db_count(&temp, "snapshots"), 0);
    assert_eq!(db_count(&temp, "branch_workspaces"), 0);
    assert_eq!(db_count(&temp, "branch_integrations"), 0);
    assert_eq!(db_count(&temp, "workflow_templates"), 1);
    assert_eq!(db_count(&temp, "workflow_template_versions"), 1);

    let list = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("list"),
    );
    assert_eq!(list["protocol"]["templates"].as_array().unwrap().len(), 1);
    assert_eq!(
        list["protocol"]["templates"][0]["template_id"],
        "test.workflow"
    );

    let show = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("show")
            .arg("test.workflow"),
    );
    assert_eq!(show["protocol"]["template_id"], "test.workflow");
    assert_eq!(
        show["protocol"]["spec"]["nodes"]["scan"]["prompt"]["inline"],
        "Scan {{env}} signals.\n"
    );

    let replay = import_workflow(&temp, &package, "wf-import-1");
    assert_eq!(replay["protocol"]["idempotency_status"], "replayed");

    fs::write(package.join("prompts/scan.md"), "Changed prompt bytes.\n").unwrap();
    let changed = run_json(rive_cmd().arg("workflow").arg("validate").arg(&package));
    assert_ne!(
        validated["protocol"]["template_hash"],
        changed["protocol"]["template_hash"]
    );
    let conflict = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("import")
            .arg(&package)
            .arg("--command-id")
            .arg("wf-import-1"),
    );
    assert_eq!(conflict["protocol"]["code"], "idempotency_conflict");
}

#[test]
fn workflow_import_can_bump_version_when_prompt_hash_changes() {
    let temp = init_workspace();
    let package = write_workflow_package(&temp);

    let v1 = import_workflow(&temp, &package, "wf-import-bump-base");
    assert_eq!(v1["protocol"]["version"], 1);
    let original_hash = v1["protocol"]["template_hash"]
        .as_str()
        .unwrap()
        .to_string();

    fs::write(
        package.join("prompts/scan.md"),
        "Changed prompt bytes for cron wrapper.\n",
    )
    .unwrap();

    let bumped = import_workflow_bump_if_changed(&temp, &package, "wf-import-bump-changed");
    assert_eq!(bumped["protocol"]["idempotency_status"], "inserted");
    assert_eq!(bumped["protocol"]["version"], 2);
    assert_eq!(bumped["protocol"]["source_version"], 1);
    assert_eq!(bumped["protocol"]["bump_if_changed"], true);
    assert_ne!(bumped["protocol"]["template_hash"], original_hash);

    let show_latest = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("show")
            .arg("test.workflow"),
    );
    assert_eq!(show_latest["protocol"]["version"], 2);
    assert_eq!(
        show_latest["protocol"]["spec"]["nodes"]["scan"]["prompt"]["inline"],
        "Changed prompt bytes for cron wrapper.\n"
    );

    let replay = import_workflow_bump_if_changed(&temp, &package, "wf-import-bump-changed");
    assert_eq!(replay["protocol"]["idempotency_status"], "replayed");
    assert_eq!(db_count(&temp, "workflow_template_versions"), 2);
}

#[test]
fn workflow_run_instantiates_work_graph_and_mapping_without_scheduler() {
    let temp = init_workspace();
    let package = write_workflow_package(&temp);
    import_workflow(&temp, &package, "wf-import-run");

    let run = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--param")
            .arg("env=stg")
            .arg("--command-id")
            .arg("wf-run-1")
            .arg("--no-scheduler"),
    );
    assert_eq!(run["protocol"]["idempotency_status"], "inserted");
    assert_eq!(run["protocol"]["template_id"], "test.workflow");
    assert_eq!(run["protocol"]["state"], "instantiated");
    assert_eq!(run["protocol"]["nodes"].as_array().unwrap().len(), 2);
    let root = run["protocol"]["root_work_node_id"].as_str().unwrap();

    let graph = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("graph")
            .arg("inspect")
            .arg("--root")
            .arg(root),
    );
    assert_eq!(graph["protocol"]["root_work_node_id"], root);
    assert_eq!(
        graph["protocol"]["reachable_nodes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(db_count(&temp, "workflow_runs"), 1);
    assert_eq!(db_count(&temp, "workflow_run_nodes"), 2);
    assert_eq!(db_count(&temp, "dispatches"), 0);
    assert_eq!(db_count(&temp, "facts"), 0);
    assert_eq!(db_count(&temp, "snapshots"), 0);

    let nodes = run["protocol"]["nodes"].as_array().unwrap();
    let scan_work = nodes
        .iter()
        .find(|node| node["node_template_id"] == "scan")
        .unwrap()["work_node_id"]
        .as_str()
        .unwrap();
    let judge_work = nodes
        .iter()
        .find(|node| node["node_template_id"] == "judge")
        .unwrap()["work_node_id"]
        .as_str()
        .unwrap();
    let scan_projection = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("inspect")
            .arg(scan_work),
    );
    assert_eq!(scan_projection["protocol"]["projection"]["state"], "ready");
    let judge_projection = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("inspect")
            .arg(judge_work),
    );
    assert_eq!(
        judge_projection["protocol"]["projection"]["state"],
        "blocked"
    );
    let missing = judge_projection["protocol"]["projection"]["missing_requirements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(missing
        .iter()
        .any(|requirement| *requirement == format!("dependency:{scan_work}")));

    let replay = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--param")
            .arg("env=stg")
            .arg("--command-id")
            .arg("wf-run-1")
            .arg("--no-scheduler"),
    );
    assert_eq!(replay["protocol"]["idempotency_status"], "replayed");
    assert_eq!(
        replay["protocol"]["workflow_run_id"],
        run["protocol"]["workflow_run_id"]
    );

    let conflict = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#other")
            .arg("--command-id")
            .arg("wf-run-1")
            .arg("--no-scheduler"),
    );
    assert_eq!(conflict["protocol"]["code"], "idempotency_conflict");
}

#[test]
fn workflow_run_can_execute_scheduler_and_replay_without_relaunch() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "worker-b");
    let package = write_workflow_package(&temp);
    import_workflow(&temp, &package, "wf-import-scheduler");
    let fake = temp.path().join("fake-opencode-workflow");
    write_fake_opencode_reporter(&fake);

    let run = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--command-id")
            .arg("wf-run-scheduler")
            .arg("--worker")
            .arg("worker-a")
            .arg("--max-parallel")
            .arg("1")
            .arg("--acceptance-mode")
            .arg("auto-reported")
            .arg("--opencode-bin")
            .arg(&fake)
            .arg("--timeout-seconds")
            .arg("10"),
    );
    assert_eq!(run["protocol"]["idempotency_status"], "inserted");
    assert_eq!(run["protocol"]["state"], "completed");
    assert_eq!(run["protocol"]["root_projection"]["state"], "done");
    assert_eq!(run["protocol"]["scheduler"]["state"], "completed");
    assert_eq!(db_count(&temp, "workflow_runs"), 1);
    assert_eq!(db_count(&temp, "scheduler_runs"), 1);
    assert_eq!(db_count(&temp, "scheduler_node_runs"), 2);
    assert_eq!(
        fs::read_to_string(temp.path().join("workflow-scheduler-invocations.txt")).unwrap(),
        "2\n"
    );

    let replay = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--command-id")
            .arg("wf-run-scheduler")
            .arg("--worker")
            .arg("worker-a")
            .arg("--max-parallel")
            .arg("1")
            .arg("--acceptance-mode")
            .arg("auto-reported")
            .arg("--opencode-bin")
            .arg(&fake)
            .arg("--timeout-seconds")
            .arg("10"),
    );
    assert_eq!(replay["protocol"]["idempotency_status"], "replayed");
    assert_eq!(replay["protocol"]["scheduler"]["state"], "completed");
    assert_eq!(
        fs::read_to_string(temp.path().join("workflow-scheduler-invocations.txt")).unwrap(),
        "2\n"
    );

    let conflict = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--command-id")
            .arg("wf-run-scheduler")
            .arg("--worker")
            .arg("worker-b")
            .arg("--acceptance-mode")
            .arg("auto-reported")
            .arg("--opencode-bin")
            .arg(&fake),
    );
    assert_eq!(conflict["protocol"]["code"], "idempotency_conflict");
}

#[test]
fn workflow_status_repairs_effective_state_from_root_projection() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    let package = write_workflow_package(&temp);
    import_workflow(&temp, &package, "wf-import-status");
    let fake = temp.path().join("fake-opencode-workflow-status");
    write_fake_opencode_reporter(&fake);

    let run = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--command-id")
            .arg("wf-run-status")
            .arg("--worker")
            .arg("worker-a")
            .arg("--max-parallel")
            .arg("1")
            .arg("--acceptance-mode")
            .arg("auto-reported")
            .arg("--opencode-bin")
            .arg(&fake)
            .arg("--timeout-seconds")
            .arg("10"),
    );
    let workflow_run_id = run["protocol"]["workflow_run_id"].as_str().unwrap();
    assert_eq!(run["protocol"]["state"], "completed");

    db_execute(
        &temp,
        "UPDATE workflow_runs SET state='instantiated', scheduler_run_id=NULL, completed_at=NULL",
    );

    let status = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("status")
            .arg("--run")
            .arg(workflow_run_id),
    );
    assert_eq!(status["protocol"]["stored_state"], "instantiated");
    assert_eq!(status["protocol"]["state"], "completed");
    assert_eq!(
        status["protocol"]["effective_state_reason"],
        "root_projection_done"
    );
    assert_eq!(status["protocol"]["scheduler"]["state"], "completed");
    assert_eq!(
        status["protocol"]["debug"]["scheduler_node_runs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        status["protocol"]["debug"]["scheduler_node_runs"][0]["stdout_ref"]
            .as_str()
            .unwrap()
            .contains(".rive/debug/runs/")
    );
    assert_eq!(
        db_text(&temp, "SELECT state FROM workflow_runs LIMIT 1"),
        "completed"
    );
}

#[test]
fn workflow_scheduler_stops_codex_child_after_report() {
    let temp = init_workspace();
    add_worker(&temp, "codex-worker");
    let package = write_workflow_package(&temp);
    import_workflow(&temp, &package, "wf-import-codex-hanging");
    let fake = temp.path().join("fake-codex-hanging-workflow");
    write_fake_codex_reporter_then_hangs(&fake);

    let run = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--command-id")
            .arg("wf-run-codex-hanging")
            .arg("--runner")
            .arg("codex")
            .arg("--worker")
            .arg("codex-worker")
            .arg("--acceptance-mode")
            .arg("auto-reported")
            .arg("--codex-bin")
            .arg(&fake)
            .arg("--trust-project")
            .arg("--timeout-seconds")
            .arg("3"),
    );

    assert_eq!(run["protocol"]["state"], "completed");
    assert_eq!(run["protocol"]["root_projection"]["state"], "done");
    assert_eq!(
        fs::read_to_string(temp.path().join("workflow-codex-hanging-invocations.txt")).unwrap(),
        "2\n"
    );
    assert_eq!(
        db_count_where(&temp, "scheduler_node_runs", "state = 'accepted'"),
        2
    );
}

#[test]
fn workflow_run_can_execute_codex_scheduler_with_fake_binary() {
    let temp = init_workspace();
    add_worker(&temp, "codex-worker");
    let package = write_workflow_package(&temp);
    import_workflow(&temp, &package, "wf-import-codex-scheduler");
    let fake = temp.path().join("fake-codex-workflow");
    write_fake_codex_reporter(&fake);

    let run = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--command-id")
            .arg("wf-run-codex-scheduler")
            .arg("--runner")
            .arg("codex")
            .arg("--worker")
            .arg("codex-worker")
            .arg("--acceptance-mode")
            .arg("auto-reported")
            .arg("--codex-bin")
            .arg(&fake)
            .arg("--trust-project")
            .arg("--timeout-seconds")
            .arg("10"),
    );
    assert_eq!(run["protocol"]["state"], "completed");
    assert_eq!(run["protocol"]["root_projection"]["state"], "done");
    assert_eq!(run["protocol"]["scheduler"]["state"], "completed");
    assert_eq!(
        db_text(&temp, "SELECT runner FROM scheduler_runs LIMIT 1"),
        "codex"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("workflow-codex-scheduler-invocations.txt")).unwrap(),
        "2\n"
    );

    let replay = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--command-id")
            .arg("wf-run-codex-scheduler")
            .arg("--runner")
            .arg("codex")
            .arg("--worker")
            .arg("codex-worker")
            .arg("--acceptance-mode")
            .arg("auto-reported")
            .arg("--codex-bin")
            .arg(&fake)
            .arg("--trust-project")
            .arg("--timeout-seconds")
            .arg("10"),
    );
    assert_eq!(replay["protocol"]["idempotency_status"], "replayed");
    assert_eq!(
        fs::read_to_string(temp.path().join("workflow-codex-scheduler-invocations.txt")).unwrap(),
        "2\n"
    );
}

#[test]
fn workflow_scheduler_respects_node_level_runner_and_worker_policy() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    add_worker(&temp, "codex-worker");
    let package = write_mixed_runner_workflow(&temp);
    import_workflow(&temp, &package, "wf-import-mixed-runner");
    let fake_opencode = temp.path().join("fake-opencode-mixed-workflow");
    let fake_codex = temp.path().join("fake-codex-mixed-workflow");
    write_fake_opencode_reporter(&fake_opencode);
    write_fake_codex_reporter(&fake_codex);

    let run = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("mixed.runner.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--command-id")
            .arg("wf-run-mixed-runner")
            .arg("--runner")
            .arg("opencode")
            .arg("--worker")
            .arg("worker-a")
            .arg("--max-parallel")
            .arg("2")
            .arg("--acceptance-mode")
            .arg("auto-reported")
            .arg("--opencode-bin")
            .arg(&fake_opencode)
            .arg("--codex-bin")
            .arg(&fake_codex)
            .arg("--trust-project")
            .arg("--timeout-seconds")
            .arg("10"),
    );

    assert_eq!(run["protocol"]["state"], "completed");
    assert_eq!(run["protocol"]["root_projection"]["state"], "done");
    assert_eq!(
        fs::read_to_string(temp.path().join("workflow-scheduler-invocations.txt")).unwrap(),
        "1\n"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("workflow-codex-scheduler-invocations.txt")).unwrap(),
        "1\n"
    );
    assert_eq!(
        db_count_where(&temp, "scheduler_node_runs", "runner = 'opencode'"),
        1
    );
    assert_eq!(
        db_count_where(&temp, "scheduler_node_runs", "runner = 'codex'"),
        1
    );
    assert_eq!(
        db_count_where(
            &temp,
            "scheduler_node_runs",
            "acceptance_mode = 'auto-reported' AND state = 'accepted'"
        ),
        2
    );
    let scheduler_run_id = run["protocol"]["scheduler"]["scheduler_run_id"]
        .as_str()
        .unwrap();
    let status = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("scheduler")
            .arg("status")
            .arg("--run")
            .arg(scheduler_run_id),
    );
    let node_runs = status["protocol"]["node_runs"].as_array().unwrap();
    assert_eq!(node_runs.len(), 2);
    assert!(node_runs.iter().any(|run| run["runner"] == "opencode"));
    assert!(node_runs.iter().any(|run| run["runner"] == "codex"));
    assert_eq!(
        db_count_where(
            &temp,
            "scheduler_node_runs",
            "runner = 'opencode' AND worker_agent_id = (SELECT agent_id FROM agents WHERE name = 'worker-a')"
        ),
        1
    );
    assert_eq!(
        db_count_where(
            &temp,
            "scheduler_node_runs",
            "runner = 'codex' AND worker_agent_id = (SELECT agent_id FROM agents WHERE name = 'codex-worker')"
        ),
        1
    );

    let conflict = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("mixed.runner.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--command-id")
            .arg("wf-run-mixed-runner")
            .arg("--runner")
            .arg("opencode")
            .arg("--worker")
            .arg("codex-worker")
            .arg("--max-parallel")
            .arg("2")
            .arg("--acceptance-mode")
            .arg("auto-reported")
            .arg("--opencode-bin")
            .arg(&fake_opencode)
            .arg("--codex-bin")
            .arg(&fake_codex)
            .arg("--trust-project")
            .arg("--timeout-seconds")
            .arg("10"),
    );
    assert_eq!(conflict["protocol"]["code"], "idempotency_conflict");
}

#[test]
fn workflow_scheduler_args_are_checked_before_instantiation() {
    let temp = init_workspace();
    let package = write_workflow_package(&temp);
    import_workflow(&temp, &package, "wf-import-scheduler-args");
    let error = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--command-id")
            .arg("wf-run-no-worker"),
    );
    assert_eq!(error["protocol"]["code"], "scheduler_worker_required");
    assert_eq!(db_count(&temp, "workflow_runs"), 0);
    assert_eq!(db_count(&temp, "work_nodes"), 0);
    assert_eq!(db_count(&temp, "scheduler_runs"), 0);

    let missing_worker = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--command-id")
            .arg("wf-run-missing-worker")
            .arg("--worker")
            .arg("no-such-worker"),
    );
    assert_eq!(missing_worker["protocol"]["code"], "agent_not_found");
    assert_eq!(db_count(&temp, "workflow_runs"), 0);
    assert_eq!(db_count(&temp, "work_nodes"), 0);
    assert_eq!(db_count(&temp, "scheduler_runs"), 0);

    add_agent(&temp, "not-worker", "orchestrator");
    let non_worker = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--command-id")
            .arg("wf-run-non-worker")
            .arg("--worker")
            .arg("not-worker"),
    );
    assert_eq!(non_worker["protocol"]["code"], "runner_worker_role_invalid");
    assert_eq!(db_count(&temp, "workflow_runs"), 0);
    assert_eq!(db_count(&temp, "work_nodes"), 0);
    assert_eq!(db_count(&temp, "scheduler_runs"), 0);
}

#[test]
fn scheduler_preflights_node_policy_workers_before_side_effects() {
    let temp = init_workspace();
    add_worker(&temp, "worker-a");
    let missing_package = write_bad_node_worker_workflow(&temp, "no-such-worker");
    import_workflow(&temp, &missing_package, "wf-import-bad-node-worker");
    let run = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("bad.node.worker.no-such-worker")
            .arg("--command-id")
            .arg("wf-run-bad-node-worker")
            .arg("--no-scheduler"),
    );
    let root = run["protocol"]["root_work_node_id"].as_str().unwrap();
    let fake = temp.path().join("fake-opencode-bad-node-policy");
    write_fake_opencode_reporter(&fake);

    let error = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("scheduler")
            .arg("run")
            .arg("--root")
            .arg(root)
            .arg("--runner")
            .arg("opencode")
            .arg("--worker")
            .arg("worker-a")
            .arg("--command-id")
            .arg("sched-bad-node-worker")
            .arg("--acceptance-mode")
            .arg("auto-reported")
            .arg("--opencode-bin")
            .arg(&fake)
            .arg("--timeout-seconds")
            .arg("10"),
    );
    assert_eq!(error["protocol"]["code"], "agent_not_found");
    assert_eq!(db_count(&temp, "scheduler_runs"), 0);
    assert_eq!(db_count(&temp, "scheduler_node_runs"), 0);
    assert_eq!(db_count(&temp, "dispatches"), 0);

    let resume_error = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("scheduler")
            .arg("resume")
            .arg("--root")
            .arg(root)
            .arg("--worker")
            .arg("worker-a")
            .arg("--command-id")
            .arg("sched-resume-bad-node-worker")
            .arg("--acceptance-mode")
            .arg("auto-reported")
            .arg("--opencode-bin")
            .arg(&fake)
            .arg("--timeout-seconds")
            .arg("10"),
    );
    assert_eq!(resume_error["protocol"]["code"], "agent_not_found");
    assert_eq!(db_count(&temp, "scheduler_runs"), 0);
    assert_eq!(db_count(&temp, "scheduler_node_runs"), 0);
    assert_eq!(db_count(&temp, "dispatches"), 0);

    add_agent(&temp, "not-worker", "orchestrator");
    let non_worker_package = write_bad_node_worker_workflow(&temp, "not-worker");
    import_workflow(&temp, &non_worker_package, "wf-import-non-worker-policy");
    let non_worker_run = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("bad.node.worker.not-worker")
            .arg("--command-id")
            .arg("wf-run-non-worker-policy")
            .arg("--no-scheduler"),
    );
    let non_worker_root = non_worker_run["protocol"]["root_work_node_id"]
        .as_str()
        .unwrap();
    let non_worker_error = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("scheduler")
            .arg("run")
            .arg("--root")
            .arg(non_worker_root)
            .arg("--runner")
            .arg("opencode")
            .arg("--worker")
            .arg("worker-a")
            .arg("--command-id")
            .arg("sched-non-worker-policy")
            .arg("--acceptance-mode")
            .arg("auto-reported")
            .arg("--opencode-bin")
            .arg(&fake)
            .arg("--timeout-seconds")
            .arg("10"),
    );
    assert_eq!(
        non_worker_error["protocol"]["code"],
        "runner_worker_role_invalid"
    );
    assert_eq!(db_count(&temp, "scheduler_runs"), 0);
    assert_eq!(db_count(&temp, "scheduler_node_runs"), 0);
    assert_eq!(db_count(&temp, "dispatches"), 0);
}

#[test]
fn workflow_params_and_gates_are_validated() {
    let temp = init_workspace();
    let package = write_workflow_package(&temp);
    import_workflow(&temp, &package, "wf-import-params");

    let missing = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--command-id")
            .arg("wf-run-missing")
            .arg("--no-scheduler"),
    );
    assert_eq!(missing["protocol"]["code"], "workflow_param_missing");

    let invalid_enum = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("test.workflow")
            .arg("--param")
            .arg("slack_channel=#alerts")
            .arg("--param")
            .arg("env=qa")
            .arg("--command-id")
            .arg("wf-run-invalid")
            .arg("--no-scheduler"),
    );
    assert_eq!(invalid_enum["protocol"]["code"], "workflow_param_invalid");

    let invalid_gate = temp.path().join("invalid-gate.yaml");
    fs::write(
        &invalid_gate,
        r#"api_version: rive.workflow/v0
id: invalid.gate
version: 1
title: Invalid gate
params:
  gate:
    type: string
nodes:
  only:
    kind: task
    title: Only
    prompt:
      inline: "Do it."
    capability_policy:
      gated_allow:
        slack.post: "{{gate}}"
    output_contract:
      format: markdown
edges:
  - type: decomposes_to
    from: root
    to: only
"#,
    )
    .unwrap();
    let invalid_gate_error = run_json_expect_error(
        rive_cmd()
            .arg("workflow")
            .arg("validate")
            .arg(&invalid_gate),
    );
    assert_eq!(
        invalid_gate_error["protocol"]["code"],
        "workflow_capability_gate_invalid"
    );
}

#[test]
fn workflow_consumes_must_be_dependency_predecessor() {
    let temp = init_workspace();
    let invalid = temp.path().join("invalid-consumes.yaml");
    fs::write(
        &invalid,
        r#"api_version: rive.workflow/v0
id: invalid.consumes
version: 1
title: Invalid consumes
nodes:
  producer:
    kind: task
    title: Producer
    prompt:
      inline: "Produce evidence."
    output_contract:
      format: markdown
  judge:
    kind: review
    title: Judge
    consumes: [producer]
    prompt:
      inline: "Judge producer output."
    output_contract:
      format: markdown
edges:
  - type: decomposes_to
    from: root
    to: producer
  - type: decomposes_to
    from: root
    to: judge
"#,
    )
    .unwrap();
    let error = run_json_expect_error(rive_cmd().arg("workflow").arg("validate").arg(&invalid));
    assert_eq!(error["protocol"]["code"], "workflow_consumes_invalid");
}

#[test]
fn workflow_importing_old_version_does_not_move_latest_pointer_backwards() {
    let temp = init_workspace();
    let v2 = write_versioned_workflow(&temp, 2, "Version two");
    let v1 = write_versioned_workflow(&temp, 1, "Version one");

    let import_v2 = import_workflow(&temp, &v2, "import-versioned-v2");
    assert_eq!(import_v2["protocol"]["version"], 2);
    let import_v1 = import_workflow(&temp, &v1, "import-versioned-v1");
    assert_eq!(import_v1["protocol"]["version"], 1);

    let show_latest = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("show")
            .arg("versioned.workflow"),
    );
    assert_eq!(show_latest["protocol"]["version"], 2);
    assert_eq!(show_latest["protocol"]["title"], "Version two");

    let list = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("list"),
    );
    assert_eq!(list["protocol"]["templates"][0]["latest_version"], 2);
}

#[test]
fn workflow_single_file_and_sentinel_example_validate() {
    let temp = init_workspace();
    let single = write_single_file_workflow(&temp);
    let single_validation = run_json(rive_cmd().arg("workflow").arg("validate").arg(&single));
    assert_eq!(
        single_validation["protocol"]["template_id"],
        "single.workflow"
    );
    assert_eq!(single_validation["protocol"]["node_count"], 1);

    let sentinel =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflows/sentinel-prod-debug");
    let validation = run_json(rive_cmd().arg("workflow").arg("validate").arg(&sentinel));
    assert_eq!(validation["protocol"]["template_id"], "sentinel.prod-debug");
    assert_eq!(validation["protocol"]["node_count"], 6);

    let imported = import_workflow(&temp, &sentinel, "sentinel-import-1");
    assert_eq!(imported["protocol"]["template_id"], "sentinel.prod-debug");
    let run = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("workflow")
            .arg("run")
            .arg("sentinel.prod-debug")
            .arg("--param")
            .arg("slack_channel=#incidents")
            .arg("--command-id")
            .arg("sentinel-run-1")
            .arg("--no-scheduler"),
    );
    assert_eq!(run["protocol"]["nodes"].as_array().unwrap().len(), 6);
    let nodes = run["protocol"]["nodes"].as_array().unwrap();
    let final_work = nodes
        .iter()
        .find(|node| node["node_template_id"] == "final-judge-and-slack")
        .unwrap()["work_node_id"]
        .as_str()
        .unwrap();
    let global_work = nodes
        .iter()
        .find(|node| node["node_template_id"] == "global-signal-scan")
        .unwrap()["work_node_id"]
        .as_str()
        .unwrap();
    let global_projection = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("inspect")
            .arg(global_work),
    );
    assert_eq!(
        global_projection["protocol"]["projection"]["state"],
        "ready"
    );
    let final_projection = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("work")
            .arg("inspect")
            .arg(final_work),
    );
    assert_eq!(
        final_projection["protocol"]["projection"]["state"],
        "blocked"
    );
    let missing = final_projection["protocol"]["projection"]["missing_requirements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(missing
        .iter()
        .any(|requirement| *requirement == format!("dependency:{global_work}")));
}
