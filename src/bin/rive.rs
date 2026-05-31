use std::io::Read;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use rive::debug_trace::{
    install_codex_hook, install_opencode_plugin, uninstall_managed, DebugTraceStore,
    IngestTraceInput, TraceAdapter, TraceListFilter, TraceListProtocol,
};
use rive::dispatch::{
    agent_protocol, dispatch_protocol, AddAgentInput, AddAgentProtocol, AgentListProtocol,
    CancelDispatchCommand, CancelDispatchOutcome, CreateDispatchInput, CreateDispatchOutcome,
    DispatchListProtocol, DispatchService,
};
use rive::facts::{protocol_from_fact, FactDisplay, FactListDisplay, FactListProtocol};
use rive::output::{Envelope, ErrorEnvelope};
use rive::snapshot::{
    read_manifest, CaptureDisplay, CaptureOptions, CaptureProtocol, LocalFsEvidenceWorkspace,
    LocalSnapshotStore, SnapshotCapture, SnapshotListDisplay, SnapshotListProtocol,
    SnapshotShowDisplay, SnapshotShowProtocol, SnapshotSummaryProtocol,
};
use rive::store::{AgentRole, EventStore};
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
    Fact {
        #[command(subcommand)]
        command: FactCommands,
    },
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },
    Dispatch {
        #[command(subcommand)]
        command: DispatchCommands,
    },
    Debug {
        #[command(subcommand)]
        command: DebugCommands,
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

#[derive(Subcommand)]
enum FactCommands {
    List,
    Show { event_id: String },
}

#[derive(Subcommand)]
enum AgentCommands {
    Add {
        name: String,
        #[arg(long)]
        role: String,
        #[arg(long)]
        token: Option<String>,
    },
    List,
    Show {
        name_or_id: String,
    },
}

#[derive(Subcommand)]
enum DispatchCommands {
    Create {
        #[arg(long)]
        target: String,
        #[arg(long)]
        title: String,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long)]
        stdin: bool,
    },
    List,
    Show {
        dispatch_id: String,
    },
    Cancel {
        dispatch_id: String,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum DebugCommands {
    Trace {
        #[command(subcommand)]
        command: DebugTraceCommands,
    },
}

#[derive(Subcommand)]
enum DebugTraceCommands {
    Ingest {
        #[arg(long)]
        adapter: String,
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        dispatch: Option<String>,
    },
    List {
        #[arg(long)]
        adapter: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        dispatch: Option<String>,
        #[arg(long)]
        session: Option<String>,
    },
    Show {
        id: String,
    },
    Session {
        trace_session_id: String,
    },
    Export {
        trace_session_id: String,
    },
    Install {
        target: String,
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
    Uninstall {
        target: String,
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
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
        Commands::Fact { command } => match command {
            FactCommands::List => {
                let workspace = find_workspace(&std::env::current_dir()?)?;
                let store = EventStore::open(&workspace.db_path())?;
                let facts = store.list_facts()?;
                let protocol = FactListProtocol {
                    facts: facts
                        .iter()
                        .map(|fact| protocol_from_fact(fact, "read"))
                        .collect(),
                };
                let display = FactListDisplay {
                    summary: format!("{} facts", facts.len()),
                };
                print_json(&Envelope::new(protocol, display))
            }
            FactCommands::Show { event_id } => {
                let workspace = find_workspace(&std::env::current_dir()?)?;
                let store = EventStore::open(&workspace.db_path())?;
                let fact = store
                    .get_fact_by_event_id(&event_id)?
                    .ok_or_else(|| anyhow!("fact not found: {event_id}"))?;
                let protocol = protocol_from_fact(&fact, "read");
                let display = FactDisplay {
                    summary: format!("{} fact {}", protocol.fact_type, protocol.event_id),
                };
                print_json(&Envelope::new(protocol, display))
            }
        },
        Commands::Agent { command } => match command {
            AgentCommands::Add { name, role, token } => {
                let workspace = find_workspace(&std::env::current_dir()?)?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let service = DispatchService::new(&workspace, &store, &snapshot_store);
                let outcome = service.add_agent(AddAgentInput {
                    name,
                    role: AgentRole::parse(&role)?,
                    token,
                })?;
                let protocol = AddAgentProtocol {
                    agent: agent_protocol(&outcome.agent),
                    token: outcome.token,
                };
                let display = serde_json::json!({
                    "summary": format!("Added {} agent {}", protocol.agent.role, protocol.agent.name),
                    "token_note": "Store this token; only its hash is persisted.",
                });
                print_json(&Envelope::new(protocol, display))
            }
            AgentCommands::List => {
                let workspace = find_workspace(&std::env::current_dir()?)?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let agents = store.list_agents()?;
                let protocol = AgentListProtocol {
                    agents: agents.iter().map(agent_protocol).collect(),
                };
                let display = serde_json::json!({
                    "summary": format!("{} agents", agents.len()),
                });
                print_json(&Envelope::new(protocol, display))
            }
            AgentCommands::Show { name_or_id } => {
                let workspace = find_workspace(&std::env::current_dir()?)?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let agent = store
                    .get_agent(&name_or_id)?
                    .ok_or_else(|| anyhow!("agent not found: {name_or_id}"))?;
                let protocol = agent_protocol(&agent);
                let display = serde_json::json!({
                    "summary": format!("{} agent {}", protocol.role, protocol.name),
                });
                print_json(&Envelope::new(protocol, display))
            }
        },
        Commands::Dispatch { command } => match command {
            DispatchCommands::Create {
                target,
                title,
                command_id,
                stdin,
            } => {
                if !stdin {
                    return Err(anyhow!("dispatch create requires --stdin"));
                }
                let workspace = find_workspace(&std::env::current_dir()?)?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let service = DispatchService::new(&workspace, &store, &snapshot_store);
                let mut body = Vec::new();
                std::io::stdin().read_to_end(&mut body)?;
                let outcome = service.create_dispatch(CreateDispatchInput {
                    command_id,
                    target_agent: target,
                    title,
                    body,
                })?;
                let (dispatch, idempotency_status) = match outcome {
                    CreateDispatchOutcome::Inserted(dispatch) => (dispatch, "inserted"),
                    CreateDispatchOutcome::Replayed(dispatch) => (dispatch, "replayed"),
                };
                let protocol = dispatch_protocol(&dispatch, idempotency_status);
                let display = serde_json::json!({
                    "summary": format!("Dispatch {} is {}", protocol.dispatch_id, protocol.state),
                });
                print_json(&Envelope::new(protocol, display))
            }
            DispatchCommands::List => {
                let workspace = find_workspace(&std::env::current_dir()?)?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let dispatches = store.list_dispatches()?;
                let protocol = DispatchListProtocol {
                    dispatches: dispatches
                        .iter()
                        .map(|dispatch| dispatch_protocol(dispatch, "read"))
                        .collect(),
                };
                let display = serde_json::json!({
                    "summary": format!("{} dispatches", dispatches.len()),
                });
                print_json(&Envelope::new(protocol, display))
            }
            DispatchCommands::Show { dispatch_id } => {
                let workspace = find_workspace(&std::env::current_dir()?)?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let dispatch = store
                    .get_dispatch(&dispatch_id)?
                    .ok_or_else(|| anyhow!("dispatch not found: {dispatch_id}"))?;
                let protocol = dispatch_protocol(&dispatch, "read");
                let display = serde_json::json!({
                    "summary": format!("Dispatch {} is {}", protocol.dispatch_id, protocol.state),
                });
                print_json(&Envelope::new(protocol, display))
            }
            DispatchCommands::Cancel {
                dispatch_id,
                command_id,
                reason,
            } => {
                let workspace = find_workspace(&std::env::current_dir()?)?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let service = DispatchService::new(&workspace, &store, &snapshot_store);
                let outcome = service.cancel_dispatch(CancelDispatchCommand {
                    command_id,
                    dispatch_id,
                    reason,
                })?;
                let (dispatch, idempotency_status) = match outcome {
                    CancelDispatchOutcome::Inserted(dispatch) => (dispatch, "inserted"),
                    CancelDispatchOutcome::Replayed(dispatch) => (dispatch, "replayed"),
                };
                let protocol = dispatch_protocol(&dispatch, idempotency_status);
                let display = serde_json::json!({
                    "summary": format!("Dispatch {} is {}", protocol.dispatch_id, protocol.state),
                });
                print_json(&Envelope::new(protocol, display))
            }
        },
        Commands::Debug { command } => match command {
            DebugCommands::Trace { command } => match command {
                DebugTraceCommands::Ingest {
                    adapter,
                    stdin,
                    agent,
                    run,
                    dispatch,
                } => {
                    if !stdin {
                        return Err(anyhow!("debug trace ingest requires --stdin"));
                    }
                    let workspace = find_workspace(&std::env::current_dir()?)?;
                    let trace_store = DebugTraceStore::open(&workspace.db_path())?;
                    trace_store.init_schema()?;
                    let mut payload = Vec::new();
                    std::io::stdin().read_to_end(&mut payload)?;
                    let protocol = trace_store.ingest(
                        &workspace,
                        IngestTraceInput {
                            adapter: TraceAdapter::parse(&adapter)?,
                            payload,
                            agent_id: agent,
                            run_id: run,
                            dispatch_id: dispatch,
                        },
                    )?;
                    let display = serde_json::json!({
                        "summary": format!(
                            "Ingested {} trace event {}",
                            protocol.trace_event.adapter,
                            protocol.trace_event.trace_event_id
                        ),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                DebugTraceCommands::List {
                    adapter,
                    agent,
                    dispatch,
                    session,
                } => {
                    let workspace = find_workspace(&std::env::current_dir()?)?;
                    let trace_store = DebugTraceStore::open(&workspace.db_path())?;
                    let events = trace_store.list_events(TraceListFilter {
                        adapter,
                        agent_id: agent,
                        dispatch_id: dispatch,
                        trace_session_id: session,
                    })?;
                    let protocol = TraceListProtocol { events };
                    let display = serde_json::json!({
                        "summary": format!("{} debug trace events", protocol.events.len()),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                DebugTraceCommands::Show { id } => {
                    let workspace = find_workspace(&std::env::current_dir()?)?;
                    let trace_store = DebugTraceStore::open(&workspace.db_path())?;
                    let protocol = trace_store.show_event(&id, true)?;
                    let display = serde_json::json!({
                        "summary": format!(
                            "Debug trace {} ({})",
                            protocol.trace_event.trace_event_id,
                            protocol.trace_event.event_kind
                        ),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                DebugTraceCommands::Session { trace_session_id }
                | DebugTraceCommands::Export { trace_session_id } => {
                    let workspace = find_workspace(&std::env::current_dir()?)?;
                    let trace_store = DebugTraceStore::open(&workspace.db_path())?;
                    let protocol = trace_store.session(&trace_session_id)?;
                    let display = serde_json::json!({
                        "summary": format!(
                            "Debug trace session {} with {} events",
                            protocol.session.trace_session_id,
                            protocol.events.len()
                        ),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                DebugTraceCommands::Install { target, workspace } => {
                    let start = workspace.unwrap_or(std::env::current_dir()?);
                    let workspace = find_workspace(&start)?;
                    let protocol = match target.as_str() {
                        "codex" => install_codex_hook(&workspace)?,
                        "opencode" => install_opencode_plugin(&workspace)?,
                        _ => return Err(anyhow!("unsupported trace install target: {target}")),
                    };
                    let display = serde_json::json!({
                        "summary": format!(
                            "Trace adapter {} install {} at {}",
                            protocol.target,
                            protocol.status,
                            protocol.path
                        ),
                        "privacy_note": "This adapter records local agent CLI inputs, outputs, and tool events for Rive debug only.",
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                DebugTraceCommands::Uninstall { target, workspace } => {
                    let start = workspace.unwrap_or(std::env::current_dir()?);
                    let workspace = find_workspace(&start)?;
                    let protocol = uninstall_managed(&workspace, &target)?;
                    let display = serde_json::json!({
                        "summary": format!(
                            "Trace adapter {} uninstall {} at {}",
                            protocol.target,
                            protocol.status,
                            protocol.path
                        ),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
            },
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
    } else if lower.contains("invalid trace payload json") {
        ("invalid_trace_payload", "fix_arguments")
    } else if lower.contains("unsupported trace adapter") {
        ("unsupported_trace_adapter", "fix_arguments")
    } else if lower.contains("debug trace event not found") {
        ("debug_trace_not_found", "fix_arguments")
    } else if lower.contains("debug trace session not found") {
        ("debug_trace_session_not_found", "fix_arguments")
    } else if lower.contains("unsupported trace install target") {
        ("unsupported_trace_install_target", "fix_arguments")
    } else if lower.contains("invalid agent role") {
        ("invalid_agent_role", "fix_arguments")
    } else if lower.contains("dispatch target must be worker") {
        ("invalid_dispatch_target", "fix_arguments")
    } else if lower.contains("dispatch closed") {
        ("dispatch_closed", "inspect_projection")
    } else if lower.contains("missing command id") {
        ("missing_command_id", "fix_arguments")
    } else if lower.contains("evidence not found") {
        ("evidence_not_found", "fix_arguments")
    } else if lower.contains("idempotency conflict") {
        ("idempotency_conflict", "inspect_projection")
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
