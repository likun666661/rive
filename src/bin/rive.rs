use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use rive::output::{Envelope, ErrorEnvelope};
use rive::snapshot::{
    read_manifest, CaptureDisplay, CaptureOptions, CaptureProtocol, LocalFsEvidenceWorkspace,
    LocalSnapshotStore, SnapshotCapture, SnapshotListDisplay, SnapshotListProtocol,
    SnapshotShowDisplay, SnapshotShowProtocol, SnapshotSummaryProtocol,
};
use rive::store::EventStore;
use rive::workspace::{find_workspace, init_workspace};

#[derive(Parser)]
#[command(name = "rive")]
#[command(version)]
#[command(about = "Rive snapshot evidence CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init {
        workspace: Option<PathBuf>,
    },
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommands,
    },
    Evidence {
        #[command(subcommand)]
        command: EvidenceCommands,
    },
}

#[derive(Subcommand)]
enum SnapshotCommands {
    Capture {
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        dispatch: Option<String>,
    },
    List,
    Show {
        snapshot_id: String,
    },
}

#[derive(Subcommand)]
enum EvidenceCommands {
    List,
}

fn main() {
    if let Err(error) = run() {
        let envelope = error_envelope(&error);
        println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init { workspace } => {
            let root = workspace.unwrap_or(std::env::current_dir()?);
            let workspace = init_workspace(&root)?;
            let protocol = serde_json::json!({
                "workspace_root": workspace.root,
                "rive_dir": workspace.rive_dir(),
                "db_path": workspace.db_path(),
            });
            let display = serde_json::json!({
                "summary": format!("Initialized Rive workspace at {}", workspace.root.display())
            });
            print_json(&Envelope::new(protocol, display))
        }
        Commands::Snapshot { command } => match command {
            SnapshotCommands::Capture {
                path,
                label,
                agent,
                dispatch,
            } => {
                let current_dir = std::env::current_dir()?;
                let start = match path.as_ref() {
                    Some(path) if path.is_absolute() => path.clone(),
                    Some(path) => current_dir.join(path),
                    None => current_dir.clone(),
                };
                let workspace = find_workspace(&start)?;
                let scope = path
                    .map(|path| {
                        if path.is_absolute() {
                            path
                        } else {
                            current_dir.join(path)
                        }
                    })
                    .unwrap_or_else(|| workspace.root.clone());
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let source = LocalFsEvidenceWorkspace::new(&workspace.root, &scope)?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let capture = SnapshotCapture::new(&source, &snapshot_store, &store);
                let snapshot = capture.capture(CaptureOptions {
                    label,
                    agent_id: agent,
                    dispatch_id: dispatch,
                    ..CaptureOptions::default()
                })?;
                let protocol = CaptureProtocol {
                    snapshot_id: snapshot.snapshot_id.clone(),
                    event_id: snapshot.event_id.clone(),
                    manifest_hash: snapshot.manifest_hash.clone(),
                    manifest_path: snapshot.manifest_path.clone(),
                    backend: snapshot.backend.clone(),
                    capture_root: snapshot.capture_root.clone(),
                    file_count: snapshot.file_count,
                    total_bytes: snapshot.total_bytes,
                };
                let display = CaptureDisplay {
                    summary: format!(
                        "Captured {} files from {}",
                        snapshot.file_count, snapshot.capture_root
                    ),
                    label: snapshot.label.clone(),
                };
                print_json(&Envelope::new(protocol, display))
            }
            SnapshotCommands::List => {
                let workspace = find_workspace(&std::env::current_dir()?)?;
                let store = EventStore::open(&workspace.db_path())?;
                let snapshots = store.list_snapshots()?;
                let protocol = SnapshotListProtocol {
                    snapshots: snapshots
                        .iter()
                        .map(|snapshot| SnapshotSummaryProtocol {
                            snapshot_id: snapshot.snapshot_id.clone(),
                            event_id: snapshot.event_id.clone(),
                            manifest_hash: snapshot.manifest_hash.clone(),
                            backend: snapshot.backend.clone(),
                            capture_root: snapshot.capture_root.clone(),
                            created_at: snapshot.created_at,
                            file_count: snapshot.file_count,
                            total_bytes: snapshot.total_bytes,
                        })
                        .collect(),
                };
                let display = SnapshotListDisplay {
                    summary: format!("{} snapshots", snapshots.len()),
                };
                print_json(&Envelope::new(protocol, display))
            }
            SnapshotCommands::Show { snapshot_id } => {
                let workspace = find_workspace(&std::env::current_dir()?)?;
                let store = EventStore::open(&workspace.db_path())?;
                let snapshot = store
                    .get_snapshot(&snapshot_id)?
                    .ok_or_else(|| anyhow!("snapshot not found: {snapshot_id}"))?;
                let manifest = read_manifest(&workspace, &snapshot)?;
                let protocol = SnapshotShowProtocol {
                    snapshot_id: snapshot.snapshot_id.clone(),
                    event_id: snapshot.event_id.clone(),
                    manifest_hash: snapshot.manifest_hash.clone(),
                    manifest_path: snapshot.manifest_path.clone(),
                    backend: snapshot.backend.clone(),
                    capture_root: snapshot.capture_root.clone(),
                    created_at: snapshot.created_at,
                    label: snapshot.label.clone(),
                    agent_id: snapshot.agent_id.clone(),
                    dispatch_id: snapshot.dispatch_id.clone(),
                    file_count: snapshot.file_count,
                    total_bytes: snapshot.total_bytes,
                    files: manifest.files,
                    skipped: manifest.skipped,
                };
                let display = SnapshotShowDisplay {
                    summary: format!(
                        "Snapshot {} captured {} files",
                        snapshot.snapshot_id, snapshot.file_count
                    ),
                    label: snapshot.label.clone(),
                };
                print_json(&Envelope::new(protocol, display))
            }
        },
        Commands::Evidence { command } => match command {
            EvidenceCommands::List => {
                let workspace = find_workspace(&std::env::current_dir()?)?;
                let store = EventStore::open(&workspace.db_path())?;
                let events = store.list_events_by_type("evidence.snapshot_captured")?;
                let protocol = serde_json::json!({
                    "events": events,
                });
                let display = serde_json::json!({
                    "summary": format!("{} evidence events", events.len()),
                });
                print_json(&Envelope::new(protocol, display))
            }
        },
    }
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn error_envelope(error: &anyhow::Error) -> ErrorEnvelope {
    let message = error.to_string();
    let lower = message.to_lowercase();
    let (code, action) = if lower.contains("no .rive workspace") {
        ("workspace_not_found", "run_rive_init")
    } else if lower.contains("does not exist") || lower.contains("not found") {
        ("not_found", "fix_arguments")
    } else if lower.contains("permission denied") {
        ("permission_denied", "fix_permissions")
    } else if lower.contains("must stay inside workspace") || lower.contains("escapes workspace") {
        ("path_outside_workspace", "fix_arguments")
    } else if lower.contains("manifest hash mismatch") {
        ("evidence_integrity_error", "inspect_evidence_store")
    } else {
        ("command_failed", "inspect_error")
    };

    ErrorEnvelope::new(code, false, action, message)
}
