use std::fs;
use std::process::Command;

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

#[test]
fn init_capture_show_and_evidence_list_work_without_agentfs() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("README.md"), "# Demo\n").unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
    fs::create_dir_all(temp.path().join(".git")).unwrap();
    fs::write(temp.path().join(".git/config"), "ignored\n").unwrap();

    let init = run_json(rive_cmd().arg("init").arg(temp.path()));
    assert!(init["protocol"]["db_path"]
        .as_str()
        .unwrap()
        .ends_with(".rive/rive.db"));
    assert!(temp.path().join(".rive/evidence/snapshots").is_dir());
    assert!(temp.path().join(".rive/evidence/blobs").is_dir());

    let capture = run_json(
        rive_cmd()
            .arg("snapshot")
            .arg("capture")
            .arg("--path")
            .arg(temp.path())
            .arg("--label")
            .arg("cli-test"),
    );
    let snapshot_id = capture["protocol"]["snapshot_id"].as_str().unwrap();
    assert!(snapshot_id.starts_with("snap_"));
    assert_eq!(capture["protocol"]["backend"], "local");
    assert_eq!(capture["protocol"]["file_count"], 2);
    assert!(capture["protocol"]["manifest_hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));

    let list = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("snapshot")
            .arg("list"),
    );
    assert_eq!(list["protocol"]["snapshots"].as_array().unwrap().len(), 1);

    let show = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("snapshot")
            .arg("show")
            .arg(snapshot_id),
    );
    let files = show["protocol"]["files"].as_array().unwrap();
    assert!(files.iter().any(|file| file["path"] == "README.md"));
    assert!(files.iter().any(|file| file["path"] == "src/lib.rs"));
    assert!(files.iter().all(|file| file["blob_ref"]
        .as_str()
        .unwrap()
        .starts_with(".rive/evidence/blobs/")));

    let evidence = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("evidence")
            .arg("list"),
    );
    let events = evidence["protocol"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["event_type"], "evidence.snapshot_captured");
    assert_eq!(events[0]["payload"]["snapshot_id"], snapshot_id);
}

#[test]
fn capture_relative_path_is_resolved_from_current_directory() {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join("subdir")).unwrap();
    fs::write(temp.path().join("root.txt"), "root\n").unwrap();
    fs::write(temp.path().join("subdir/nested.txt"), "nested\n").unwrap();

    run_json(rive_cmd().arg("init").arg(temp.path()));
    let capture = run_json(
        rive_cmd()
            .current_dir(temp.path().join("subdir"))
            .arg("snapshot")
            .arg("capture")
            .arg("--path")
            .arg("."),
    );

    let snapshot_id = capture["protocol"]["snapshot_id"].as_str().unwrap();
    let show = run_json(
        rive_cmd()
            .current_dir(temp.path())
            .arg("snapshot")
            .arg("show")
            .arg(snapshot_id),
    );
    let files = show["protocol"]["files"].as_array().unwrap();
    assert!(files.iter().any(|file| file["path"] == "subdir/nested.txt"));
    assert!(!files.iter().any(|file| file["path"] == "root.txt"));
}

#[test]
fn team_self_check_reports_missing_env_without_fact_side_effects() {
    let output = team_cmd().arg("self-check").output().unwrap();
    assert!(!output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["protocol"]["ok"], false);
    assert!(payload["protocol"]["missing_env"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "RIVE_AGENT_ID"));
}
