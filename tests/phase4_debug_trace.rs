use std::fs;
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
    assert!(!output.status.success());
    serde_json::from_slice(&output.stdout).expect("stdout should be json")
}

#[test]
fn codex_hook_ingest_show_session_and_filters_work_without_business_mutation() {
    let temp = TempDir::new().unwrap();
    run_json(rive_cmd().arg("init").arg(temp.path()));

    let first = run_json_with_stdin(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("ingest")
            .arg("--adapter")
            .arg("codex-hook")
            .arg("--agent")
            .arg("agent-a")
            .arg("--dispatch")
            .arg("disp-a")
            .arg("--stdin"),
        r#"{
          "hook_event_name": "SessionStart",
          "session_id": "codex_s_1",
          "turn_id": "turn_1",
          "cwd": "/tmp/rive-demo",
          "model": "gpt-5",
          "permission_mode": "default",
          "transcript_path": "/tmp/codex/transcript.jsonl"
        }"#,
    );
    assert_eq!(
        first["protocol"]["trace_event"]["event_kind"],
        "session_started"
    );
    assert_eq!(
        first["protocol"]["trace_event"]["agent_id"].as_str(),
        Some("agent-a")
    );
    assert!(first["protocol"]["raw_event"]["payload_hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    let trace_event_id = first["protocol"]["trace_event"]["trace_event_id"]
        .as_str()
        .unwrap();
    let trace_session_id = first["protocol"]["trace_event"]["trace_session_id"]
        .as_str()
        .unwrap();

    run_json_with_stdin(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("ingest")
            .arg("--adapter")
            .arg("codex-hook")
            .arg("--stdin"),
        r#"{
          "hook_event_name": "PostToolUse",
          "session_id": "codex_s_1",
          "turn_id": "turn_1",
          "tool_name": "shell",
          "tool_use_id": "tool_1",
          "tool_input": { "cmd": "cargo test" },
          "tool_response": { "exit_code": 0, "stdout": "ok" }
        }"#,
    );

    let show = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("show")
            .arg(trace_event_id),
    );
    assert_eq!(show["protocol"]["raw_payload"]["session_id"], "codex_s_1");
    assert_eq!(
        show["protocol"]["raw_event"]["payload_hash"],
        first["protocol"]["raw_event"]["payload_hash"]
    );

    let list = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("list")
            .arg("--adapter")
            .arg("codex-hook")
            .arg("--agent")
            .arg("agent-a")
            .arg("--dispatch")
            .arg("disp-a"),
    );
    assert_eq!(list["protocol"]["events"].as_array().unwrap().len(), 1);

    let session = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("session")
            .arg(trace_session_id),
    );
    assert_eq!(session["protocol"]["events"].as_array().unwrap().len(), 2);
    assert_eq!(session["protocol"]["events"][0]["sequence"], 1);
    assert_eq!(session["protocol"]["events"][1]["sequence"], 2);

    let export = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("export")
            .arg(trace_session_id),
    );
    assert_eq!(
        export["protocol"]["session"]["trace_session_id"],
        trace_session_id
    );

    let conn = Connection::open(temp.path().join(".rive/rive.db")).unwrap();
    let business_events: i64 = conn
        .query_row("select count(*) from events", [], |row| row.get(0))
        .unwrap();
    let facts: i64 = conn
        .query_row("select count(*) from facts", [], |row| row.get(0))
        .unwrap();
    let snapshots: i64 = conn
        .query_row("select count(*) from snapshots", [], |row| row.get(0))
        .unwrap();
    let dispatches: i64 = conn
        .query_row("select count(*) from dispatches", [], |row| row.get(0))
        .unwrap();
    assert_eq!(business_events, 0);
    assert_eq!(facts, 0);
    assert_eq!(snapshots, 0);
    assert_eq!(dispatches, 0);
}

#[test]
fn opencode_and_unknown_events_are_preserved() {
    let temp = TempDir::new().unwrap();
    run_json(rive_cmd().arg("init").arg(temp.path()));

    let opencode = run_json_with_stdin(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("ingest")
            .arg("--adapter")
            .arg("opencode-plugin")
            .arg("--stdin"),
        r#"{
          "type": "tool.execute.after",
          "session": { "id": "opencode_s_1" },
          "tool": { "id": "tool_1", "name": "bash" },
          "output": { "exit": 0, "stdout": "ok" }
        }"#,
    );
    assert_eq!(
        opencode["protocol"]["trace_event"]["event_kind"],
        "tool_call_completed"
    );
    assert_eq!(
        opencode["protocol"]["trace_event"]["external_session_id"],
        "opencode_s_1"
    );

    let wrapped = run_json_with_stdin(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("ingest")
            .arg("--adapter")
            .arg("opencode-plugin")
            .arg("--stdin"),
        r#"{
          "id": "evt_wrapped_1",
          "type": "message.part.updated",
          "properties": {
            "sessionID": "opencode_s_2",
            "time": 1780242657644,
            "part": {
              "id": "part_1",
              "messageID": "msg_1",
              "sessionID": "opencode_s_2",
              "type": "text",
              "text": "RIVE_TRACE_OK"
            }
          }
        }"#,
    );
    assert_eq!(
        wrapped["protocol"]["trace_event"]["external_session_id"],
        "opencode_s_2"
    );
    assert_eq!(
        wrapped["protocol"]["trace_event"]["external_turn_id"],
        "msg_1"
    );
    assert_eq!(
        wrapped["protocol"]["trace_event"]["summary"]["text_preview"],
        "RIVE_TRACE_OK"
    );
    assert!(wrapped["protocol"]["trace_event"]["occurred_at"].is_string());
    let wrapped_session_id = wrapped["protocol"]["trace_event"]["trace_session_id"]
        .as_str()
        .unwrap();
    let wrapped_session = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("session")
            .arg(wrapped_session_id),
    );
    assert_eq!(
        wrapped_session["protocol"]["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let wrapped_tool = run_json_with_stdin(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("ingest")
            .arg("--adapter")
            .arg("opencode-plugin")
            .arg("--stdin"),
        r#"{
          "id": "evt_wrapped_tool_1",
          "type": "message.part.updated",
          "properties": {
            "sessionID": "opencode_s_2",
            "time": 1780242657650,
            "part": {
              "type": "tool",
              "tool": "bash",
              "callID": "call_1",
              "sessionID": "opencode_s_2",
              "messageID": "msg_1",
              "state": {
                "status": "completed",
                "input": { "command": "pwd" },
                "output": "/tmp/rive\n"
              }
            }
          }
        }"#,
    );
    assert_eq!(
        wrapped_tool["protocol"]["trace_event"]["event_kind"],
        "tool_call_completed"
    );
    assert_eq!(
        wrapped_tool["protocol"]["trace_event"]["external_tool_id"],
        "call_1"
    );
    assert_eq!(
        wrapped_tool["protocol"]["trace_event"]["summary"]["tool_name"],
        "bash"
    );

    let unknown = run_json_with_stdin(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("ingest")
            .arg("--adapter")
            .arg("opencode-plugin")
            .arg("--stdin"),
        r#"{
          "type": "vendor.new.event",
          "session": { "id": "unknown_s_1" },
          "payload": { "still": "preserved" }
        }"#,
    );
    assert_eq!(unknown["protocol"]["trace_event"]["event_kind"], "unknown");
    let raw_id = unknown["protocol"]["raw_event"]["raw_event_id"]
        .as_str()
        .unwrap();
    let show = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("show")
            .arg(raw_id),
    );
    assert_eq!(
        show["protocol"]["raw_payload"]["payload"]["still"],
        "preserved"
    );

    let filtered = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("list")
            .arg("--adapter")
            .arg("opencode-plugin"),
    );
    assert_eq!(filtered["protocol"]["events"].as_array().unwrap().len(), 4);
}

#[test]
fn trace_errors_and_install_templates_are_stable() {
    let temp = TempDir::new().unwrap();
    run_json(rive_cmd().arg("init").arg(temp.path()));

    let invalid_json = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("ingest")
            .arg("--adapter")
            .arg("codex-hook")
            .arg("--stdin"),
        Some("{ broken"),
    );
    assert_eq!(invalid_json["protocol"]["code"], "invalid_trace_payload");

    let unsupported = run_json_expect_error(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("ingest")
            .arg("--adapter")
            .arg("missing")
            .arg("--stdin"),
        Some("{}"),
    );
    assert_eq!(unsupported["protocol"]["code"], "unsupported_trace_adapter");

    let codex = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("install")
            .arg("codex")
            .arg("--workspace")
            .arg(temp.path()),
    );
    let codex_path = codex["protocol"]["path"].as_str().unwrap();
    let codex_hooks_json = fs::read_to_string(codex_path).unwrap();
    assert!(codex_hooks_json.contains("RIVE-MANAGED-CODEX-TRACE-HOOKS"));
    assert!(codex_hooks_json.contains("SessionStart"));
    assert!(codex_hooks_json.contains("PostToolUse"));
    let codex_hook_path = temp
        .path()
        .join(".rive/debug/adapters/codex-rive-trace-hook.sh");
    let codex_hook = fs::read_to_string(codex_hook_path).unwrap();
    assert!(codex_hook.contains("rive debug trace ingest --adapter codex-hook --stdin"));
    assert!(codex_hook.contains("|| true"));
    let codex_again = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("install")
            .arg("codex")
            .arg("--workspace")
            .arg(temp.path()),
    );
    assert_eq!(
        codex_again["protocol"]["status"],
        "config:unchanged; script:unchanged"
    );

    let opencode = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("install")
            .arg("opencode")
            .arg("--workspace")
            .arg(temp.path()),
    );
    let opencode_path = opencode["protocol"]["path"].as_str().unwrap();
    let opencode_plugin = fs::read_to_string(opencode_path).unwrap();
    assert!(opencode_plugin.contains("opencode-plugin"));
    assert!(opencode_plugin.contains(
        r#"const args = ["debug", "trace", "ingest", "--adapter", "opencode-plugin", "--stdin"]"#
    ));
    assert!(opencode_plugin.contains(r#"if (process.env.RIVE_RUN_ID)"#));
    assert!(opencode_plugin.contains(r#"Bun.spawnSync(["rive", ...args]"#));
    assert!(opencode_plugin.contains("mkdtempSync"));
    assert!(opencode_plugin.contains("try {"));
    assert!(opencode_plugin.contains("Debug trace must never alter OpenCode behavior"));

    let uninstall = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("debug")
            .arg("trace")
            .arg("uninstall")
            .arg("opencode")
            .arg("--workspace")
            .arg(temp.path()),
    );
    assert_eq!(uninstall["protocol"]["status"], "removed");
    assert!(!std::path::Path::new(opencode_path).exists());
}
