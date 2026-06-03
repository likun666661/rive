use std::collections::BTreeSet;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use rive::branch::{backend_from_env, branch_integration_protocol, BranchService};
use rive::debug_trace::{
    install_codex_hook, install_opencode_plugin, uninstall_managed, usage_for_workspace,
    DebugTraceStore, IngestTraceInput, TraceAdapter, TraceListFilter, TraceListProtocol,
    TraceUsageFilter,
};
use rive::dispatch::{
    agent_protocol, dispatch_protocol, AddAgentInput, AddAgentProtocol, AgentListProtocol,
    CancelDispatchCommand, CancelDispatchOutcome, CreateDispatchInput, CreateDispatchOutcome,
    DispatchListProtocol, DispatchService,
};
use rive::facts::{protocol_from_fact, FactDisplay, FactListDisplay, FactListProtocol};
use rive::output::{Envelope, ErrorEnvelope};
use rive::runner::{
    CodexRunner, CodexRunnerInput, OpenCodeRunner, OpenCodeRunnerInput, OrchestratorRunner,
    OrchestratorRunnerInput, SchedulerResumeInput, SchedulerRunInput, SchedulerService,
    SchedulerStatusInput,
};
use rive::snapshot::{
    read_manifest, CaptureDisplay, CaptureOptions, CaptureProtocol, LocalFsEvidenceWorkspace,
    LocalSnapshotStore, SnapshotCapture, SnapshotListDisplay, SnapshotListProtocol,
    SnapshotShowDisplay, SnapshotShowProtocol, SnapshotSummaryProtocol,
};
use rive::store::{AgentRole, EventStore};
use rive::work::{
    work_edge_protocol, work_node_protocol, AddWorkEdgeInput, CreateWorkNodeInput, WorkEdgeType,
    WorkNodeKind, WorkService, WorkStatusInput,
};
use rive::workflow::{
    WorkflowImportInput, WorkflowRunInput, WorkflowSchedulerRequest, WorkflowService,
};
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
    Work {
        #[command(subcommand)]
        command: WorkCommands,
    },
    Branch {
        #[command(subcommand)]
        command: BranchCommands,
    },
    Debug {
        #[command(subcommand)]
        command: DebugCommands,
    },
    Runner {
        #[command(subcommand)]
        command: RunnerCommands,
    },
    Scheduler {
        #[command(subcommand)]
        command: SchedulerCommands,
    },
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommands,
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
        #[arg(long = "work")]
        work_node_id: Option<String>,
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
enum WorkCommands {
    Create {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        title: String,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long)]
        stdin: bool,
    },
    Edge {
        #[command(subcommand)]
        command: WorkEdgeCommands,
    },
    Graph {
        #[command(subcommand)]
        command: WorkGraphCommands,
    },
    List,
    Show {
        work_node_id: String,
    },
    Inspect {
        work_node_id: String,
    },
    Accept {
        work_node_id: String,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long = "require-committed-branch")]
        require_committed_branch: bool,
        #[arg(long)]
        stdin: bool,
    },
    Reopen {
        work_node_id: String,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long)]
        stdin: bool,
    },
}

#[derive(Subcommand)]
enum BranchCommands {
    List,
    Show {
        id: String,
    },
    Commit {
        integration_id: String,
        #[arg(long = "command-id")]
        command_id: String,
    },
    Abort {
        integration_id: String,
        #[arg(long = "command-id")]
        command_id: String,
    },
    Reject {
        integration_id: String,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long)]
        stdin: bool,
    },
}

#[derive(Subcommand)]
enum WorkEdgeCommands {
    Add {
        #[arg(long = "type")]
        edge_type: String,
        #[arg(long = "from")]
        from_node_id: String,
        #[arg(long = "to")]
        to_node_id: String,
        #[arg(long = "command-id")]
        command_id: String,
    },
}

#[derive(Subcommand)]
enum WorkGraphCommands {
    Inspect {
        #[arg(long = "root")]
        root_work_node_id: String,
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
    Usage {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        dispatch: Option<String>,
        #[arg(long = "work")]
        work_node_id: Option<String>,
        #[arg(long = "root")]
        root_work_node_id: Option<String>,
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

#[derive(Subcommand)]
enum RunnerCommands {
    Orchestrator {
        #[arg(long)]
        runner: String,
        #[arg(long)]
        agent: String,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long = "agent-token")]
        agent_token: Option<String>,
        #[arg(long = "worker")]
        workers: Vec<String>,
        #[arg(long = "acceptance-command")]
        acceptance_command: Option<String>,
        #[arg(long = "opencode-bin")]
        opencode_bin: Option<PathBuf>,
        #[arg(long = "timeout-seconds", default_value_t = 600)]
        timeout_seconds: u64,
        #[arg(long)]
        stdin: bool,
    },
    Opencode {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        title: String,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long = "agent-token")]
        agent_token: Option<String>,
        #[arg(long = "opencode-bin")]
        opencode_bin: Option<PathBuf>,
        #[arg(long = "timeout-seconds", default_value_t = 300)]
        timeout_seconds: u64,
        #[arg(long = "snapshot-path")]
        snapshot_paths: Vec<PathBuf>,
        #[arg(long)]
        stdin: bool,
    },
    Codex {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        title: String,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long = "agent-token")]
        agent_token: Option<String>,
        #[arg(long = "codex-bin")]
        codex_bin: Option<PathBuf>,
        #[arg(long = "timeout-seconds", default_value_t = 300)]
        timeout_seconds: u64,
        #[arg(long = "snapshot-path")]
        snapshot_paths: Vec<PathBuf>,
        #[arg(long = "trust-project")]
        trust_project: bool,
        #[arg(long)]
        stdin: bool,
    },
}

#[derive(Subcommand)]
enum SchedulerCommands {
    Run {
        #[arg(long = "root")]
        root_work_node_id: String,
        #[arg(long)]
        runner: String,
        #[arg(long = "worker")]
        workers: Vec<String>,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long = "max-parallel", default_value_t = 1)]
        max_parallel: usize,
        #[arg(long = "acceptance-mode", default_value = "manual")]
        acceptance_mode: String,
        #[arg(long = "workspace-mode", default_value = "shared")]
        workspace_mode: String,
        #[arg(long = "opencode-bin")]
        opencode_bin: Option<PathBuf>,
        #[arg(long = "timeout-seconds", default_value_t = 300)]
        timeout_seconds: u64,
    },
    Status {
        #[arg(long = "run")]
        scheduler_run_id: Option<String>,
        #[arg(long = "root")]
        root_work_node_id: Option<String>,
    },
    Resume {
        #[arg(long = "run")]
        scheduler_run_id: Option<String>,
        #[arg(long = "root")]
        root_work_node_id: Option<String>,
        #[arg(long = "worker")]
        workers: Vec<String>,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long = "max-parallel")]
        max_parallel: Option<usize>,
        #[arg(long = "acceptance-mode")]
        acceptance_mode: Option<String>,
        #[arg(long = "workspace-mode", default_value = "shared")]
        workspace_mode: String,
        #[arg(long = "opencode-bin")]
        opencode_bin: Option<PathBuf>,
        #[arg(long = "timeout-seconds", default_value_t = 300)]
        timeout_seconds: u64,
    },
}

#[derive(Subcommand)]
enum WorkflowCommands {
    Validate {
        path: PathBuf,
    },
    Import {
        path: PathBuf,
        #[arg(long = "command-id")]
        command_id: String,
    },
    List,
    Show {
        template_id: String,
        #[arg(long)]
        version: Option<i64>,
    },
    Run {
        template_id: String,
        #[arg(long = "param")]
        params: Vec<String>,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long = "no-scheduler")]
        no_scheduler: bool,
        #[arg(long, default_value = "opencode")]
        runner: String,
        #[arg(long = "worker")]
        workers: Vec<String>,
        #[arg(long = "max-parallel", default_value_t = 1)]
        max_parallel: usize,
        #[arg(long = "acceptance-mode", default_value = "manual")]
        acceptance_mode: String,
        #[arg(long = "workspace-mode", default_value = "shared")]
        workspace_mode: String,
        #[arg(long = "opencode-bin")]
        opencode_bin: Option<PathBuf>,
        #[arg(long = "timeout-seconds", default_value_t = 300)]
        timeout_seconds: u64,
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
                let source_root = std::env::var("RIVE_WORKSPACE")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| workspace.root.clone());
                let source = LocalFsEvidenceWorkspace::new(&source_root, &scope)?;
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
                work_node_id,
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
                let work = if let Some(work_node_id) = work_node_id {
                    let work_service = WorkService::new(&workspace, &store, &snapshot_store);
                    work_service.bind_dispatch(rive::work::BindWorkDispatchCommand {
                        work_node_id,
                        dispatch_id: dispatch.dispatch_id.clone(),
                    })?;
                    work_service.projection_for_dispatch(&dispatch.dispatch_id)?
                } else {
                    None
                };
                let protocol = dispatch_protocol(&dispatch, idempotency_status);
                let display = serde_json::json!({
                    "summary": format!("Dispatch {} is {}", dispatch.dispatch_id, dispatch.state.as_str()),
                });
                if let Some(work) = work {
                    print_json(&Envelope::new(
                        serde_json::json!({
                            "dispatch": protocol,
                            "work": work,
                        }),
                        display,
                    ))
                } else {
                    print_json(&Envelope::new(protocol, display))
                }
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
        Commands::Work { command } => {
            let workspace = find_workspace(&std::env::current_dir()?)
                .map_err(|_| anyhow!("workspace not initialized"))?;
            let store = EventStore::open(&workspace.db_path())?;
            store.init_schema()?;
            let snapshot_store = LocalSnapshotStore::new(&workspace);
            let service = WorkService::new(&workspace, &store, &snapshot_store);
            match command {
                WorkCommands::Create {
                    kind,
                    title,
                    command_id,
                    stdin,
                } => {
                    let mut body = Vec::new();
                    if stdin {
                        std::io::stdin().read_to_end(&mut body)?;
                    }
                    let (node, idempotency_status) = service.create_node(CreateWorkNodeInput {
                        command_id,
                        kind: WorkNodeKind::parse(&kind)?,
                        title,
                        body,
                    })?;
                    let protocol =
                        work_node_protocol(&node, service.graph_version()?, idempotency_status);
                    let display = serde_json::json!({
                        "summary": format!("Work node {} is {}", protocol.work_node_id, protocol.status_input),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                WorkCommands::Edge { command } => match command {
                    WorkEdgeCommands::Add {
                        edge_type,
                        from_node_id,
                        to_node_id,
                        command_id,
                    } => {
                        let (edge, idempotency_status) = service.add_edge(AddWorkEdgeInput {
                            command_id,
                            edge_type: WorkEdgeType::parse(&edge_type)?,
                            from_node_id,
                            to_node_id,
                        })?;
                        let protocol = work_edge_protocol(&edge, idempotency_status);
                        let display = serde_json::json!({
                            "summary": format!(
                                "Work edge {} {} -> {}",
                                protocol.edge_type, protocol.from_node_id, protocol.to_node_id
                            ),
                        });
                        print_json(&Envelope::new(protocol, display))
                    }
                },
                WorkCommands::Graph { command } => match command {
                    WorkGraphCommands::Inspect { root_work_node_id } => {
                        let protocol = service.inspect_graph(&root_work_node_id)?;
                        let display = serde_json::json!({
                            "summary": format!(
                                "Work graph {} hygiene {}",
                                protocol.root_work_node_id,
                                protocol.hygiene_state
                            ),
                        });
                        print_json(&Envelope::new(protocol, display))
                    }
                },
                WorkCommands::List => {
                    let protocol = service.list_nodes()?;
                    let display = serde_json::json!({
                        "summary": format!("{} work nodes", protocol.nodes.len()),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                WorkCommands::Show { work_node_id } => {
                    let protocol = service.show_node(&work_node_id)?;
                    let display = serde_json::json!({
                        "summary": format!("Work node {} {}", protocol.work_node_id, protocol.title),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                WorkCommands::Inspect { work_node_id } => {
                    let protocol = service.inspect(&work_node_id)?;
                    let display = serde_json::json!({
                        "summary": format!(
                            "Work node {} is {}",
                            protocol.node.work_node_id, protocol.projection.state
                        ),
                        "explanation": format!(
                            "{} missing requirements",
                            protocol.projection.missing_requirements.len()
                        ),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                WorkCommands::Accept {
                    work_node_id,
                    command_id,
                    require_committed_branch,
                    stdin,
                } => {
                    let mut reason = Vec::new();
                    if stdin {
                        std::io::stdin().read_to_end(&mut reason)?;
                    }
                    let (node, idempotency_status) = service.accept_node(WorkStatusInput {
                        command_id,
                        work_node_id,
                        reason,
                        require_committed_branch,
                    })?;
                    let protocol =
                        work_node_protocol(&node, service.graph_version()?, idempotency_status);
                    let display = serde_json::json!({
                        "summary": format!("Accepted work node {}", protocol.work_node_id),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                WorkCommands::Reopen {
                    work_node_id,
                    command_id,
                    stdin,
                } => {
                    let mut reason = Vec::new();
                    if stdin {
                        std::io::stdin().read_to_end(&mut reason)?;
                    }
                    let (node, idempotency_status) = service.reopen_node(WorkStatusInput {
                        command_id,
                        work_node_id,
                        reason,
                        require_committed_branch: false,
                    })?;
                    let protocol =
                        work_node_protocol(&node, service.graph_version()?, idempotency_status);
                    let display = serde_json::json!({
                        "summary": format!("Reopened work node {}", protocol.work_node_id),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
            }
        }
        Commands::Branch { command } => {
            let workspace = find_workspace(&std::env::current_dir()?)
                .map_err(|_| anyhow!("workspace not initialized"))?;
            let store = EventStore::open(&workspace.db_path())?;
            store.init_schema()?;
            store.init_work_schema()?;
            let service = BranchService::new(&workspace, &store);
            match command {
                BranchCommands::List => {
                    let integrations = service.list()?;
                    let protocol = serde_json::json!({
                        "integrations": integrations.iter().map(branch_integration_protocol).collect::<Vec<_>>(),
                    });
                    let display = serde_json::json!({
                        "summary": format!("{} branch integrations", integrations.len()),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                BranchCommands::Show { id } => {
                    let (branch, integration) = service.show(&id)?;
                    let protocol = serde_json::json!({
                        "branch": branch,
                        "integration": integration.as_ref().map(branch_integration_protocol),
                    });
                    let display = serde_json::json!({
                        "summary": format!("Branch {} {}", branch.branch_id, branch.state),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                BranchCommands::Commit {
                    integration_id,
                    command_id,
                } => {
                    let backend = backend_from_env();
                    let (integration, idempotency_status) =
                        service.commit(backend.as_ref(), &integration_id, &command_id)?;
                    let protocol = serde_json::json!({
                        "integration": branch_integration_protocol(&integration),
                        "idempotency_status": idempotency_status,
                    });
                    let display = serde_json::json!({
                        "summary": format!("Committed branch integration {}", integration.integration_id),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                BranchCommands::Abort {
                    integration_id,
                    command_id,
                } => {
                    let backend = backend_from_env();
                    let (integration, idempotency_status) =
                        service.abort(backend.as_ref(), &integration_id, &command_id)?;
                    let protocol = serde_json::json!({
                        "integration": branch_integration_protocol(&integration),
                        "idempotency_status": idempotency_status,
                    });
                    let display = serde_json::json!({
                        "summary": format!("Aborted branch integration {}", integration.integration_id),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
                BranchCommands::Reject {
                    integration_id,
                    command_id,
                    stdin,
                } => {
                    let mut reason = Vec::new();
                    if stdin {
                        std::io::stdin().read_to_end(&mut reason)?;
                    }
                    let (integration, idempotency_status) =
                        service.reject(&integration_id, &command_id, &reason)?;
                    let protocol = serde_json::json!({
                        "integration": branch_integration_protocol(&integration),
                        "idempotency_status": idempotency_status,
                    });
                    let display = serde_json::json!({
                        "summary": format!("Rejected branch integration {}", integration.integration_id),
                    });
                    print_json(&Envelope::new(protocol, display))
                }
            }
        }
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
                DebugTraceCommands::Usage {
                    run,
                    agent,
                    dispatch,
                    work_node_id,
                    root_work_node_id,
                } => {
                    let workspace = find_workspace(&std::env::current_dir()?)?;
                    let trace_store = DebugTraceStore::open(&workspace.db_path())?;
                    let store = EventStore::open(&workspace.db_path())?;
                    store.init_schema()?;
                    let snapshot_store = LocalSnapshotStore::new(&workspace);
                    let work_service = WorkService::new(&workspace, &store, &snapshot_store);
                    let dispatch = if let Some(work_node_id) = work_node_id {
                        store
                            .list_work_dispatch_bindings()?
                            .into_iter()
                            .find(|binding| binding.work_node_id == work_node_id)
                            .map(|binding| binding.dispatch_id)
                            .or(dispatch)
                    } else {
                        dispatch
                    };
                    let mut correlated_run_ids = BTreeSet::new();
                    let mut correlated_dispatch_ids = BTreeSet::new();
                    if let Some(root_work_node_id) = root_work_node_id {
                        let graph = work_service.inspect_graph(&root_work_node_id)?;
                        let scoped_nodes = graph.scoped_nodes.into_iter().collect::<BTreeSet<_>>();
                        for binding in store.list_work_root_bindings_for_root(&root_work_node_id)? {
                            if let Some(run_id) = binding.created_by_run_id {
                                correlated_run_ids.insert(run_id);
                            }
                        }
                        for binding in store.list_work_dispatch_bindings()? {
                            if scoped_nodes.contains(&binding.work_node_id) {
                                correlated_dispatch_ids.insert(binding.dispatch_id);
                            }
                        }
                    }
                    let protocol = usage_for_workspace(
                        &workspace,
                        &trace_store,
                        TraceUsageFilter {
                            run_id: run,
                            agent_id: agent,
                            dispatch_id: dispatch,
                            correlated_run_ids,
                            correlated_dispatch_ids,
                        },
                    )?;
                    let display = serde_json::json!({
                        "summary": format!(
                            "{} runs, {} total tokens",
                            protocol.runs.len(),
                            protocol.totals.total_tokens
                        ),
                        "debug_note": "Usage is a debug read model only and does not affect Rive protocol state.",
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
        Commands::Runner { command } => match command {
            RunnerCommands::Orchestrator {
                runner,
                agent,
                command_id,
                agent_token,
                workers,
                acceptance_command,
                opencode_bin,
                timeout_seconds,
                stdin,
            } => {
                if !stdin {
                    return Err(anyhow!("runner orchestrator requires --stdin"));
                }
                let workspace = find_workspace(&std::env::current_dir()?)
                    .map_err(|_| anyhow!("workspace not initialized"))?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let trace_store = DebugTraceStore::open(&workspace.db_path())?;
                trace_store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let orchestrator =
                    OrchestratorRunner::new(&workspace, &store, &trace_store, &snapshot_store);
                let mut objective = Vec::new();
                std::io::stdin().read_to_end(&mut objective)?;
                let protocol = orchestrator.run(OrchestratorRunnerInput {
                    runner,
                    agent,
                    command_id,
                    agent_token,
                    opencode_bin,
                    timeout_seconds,
                    workers,
                    acceptance_command,
                    objective,
                })?;
                let display = serde_json::json!({
                    "summary": format!(
                        "Orchestrator runner {} ended with root {} {}",
                        protocol.runner.run_id,
                        protocol.runner.root_work_node_id,
                        protocol.root_work.state
                    ),
                    "trace_note": "Debug trace is for Rive diagnostics only; orchestrator success is based on root work projection.",
                });
                print_json(&Envelope::new(protocol, display))
            }
            RunnerCommands::Opencode {
                agent,
                title,
                command_id,
                agent_token,
                opencode_bin,
                timeout_seconds,
                snapshot_paths,
                stdin,
            } => {
                if !stdin {
                    return Err(anyhow!("runner opencode requires --stdin"));
                }
                let workspace = find_workspace(&std::env::current_dir()?)
                    .map_err(|_| anyhow!("workspace not initialized"))?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let trace_store = DebugTraceStore::open(&workspace.db_path())?;
                trace_store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let runner = OpenCodeRunner::new(&workspace, &store, &trace_store, &snapshot_store);
                let mut task_body = Vec::new();
                std::io::stdin().read_to_end(&mut task_body)?;
                let protocol = runner.run(OpenCodeRunnerInput {
                    agent,
                    title,
                    command_id,
                    agent_token,
                    opencode_bin,
                    timeout_seconds,
                    snapshot_paths,
                    task_body,
                })?;
                let display = serde_json::json!({
                    "summary": format!(
                        "OpenCode runner {} ended with dispatch {} {}",
                        protocol.runner.run_id,
                        protocol.dispatch.dispatch_id,
                        protocol.dispatch.state
                    ),
                    "trace_note": "Debug trace is for Rive diagnostics only; dispatch success is based on ledger projection.",
                });
                print_json(&Envelope::new(protocol, display))
            }
            RunnerCommands::Codex {
                agent,
                title,
                command_id,
                agent_token,
                codex_bin,
                timeout_seconds,
                snapshot_paths,
                trust_project,
                stdin,
            } => {
                if !stdin {
                    return Err(anyhow!("runner codex requires --stdin"));
                }
                let workspace = find_workspace(&std::env::current_dir()?)
                    .map_err(|_| anyhow!("workspace not initialized"))?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let trace_store = DebugTraceStore::open(&workspace.db_path())?;
                trace_store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let runner = CodexRunner::new(&workspace, &store, &trace_store, &snapshot_store);
                let mut task_body = Vec::new();
                std::io::stdin().read_to_end(&mut task_body)?;
                let protocol = runner.run(CodexRunnerInput {
                    agent,
                    title,
                    command_id,
                    agent_token,
                    codex_bin,
                    timeout_seconds,
                    snapshot_paths,
                    task_body,
                    trust_project,
                })?;
                let display = serde_json::json!({
                    "summary": format!(
                        "Codex runner {} ended with dispatch {} {}",
                        protocol.runner.run_id,
                        protocol.dispatch.dispatch_id,
                        protocol.dispatch.state
                    ),
                    "trace_note": "Debug trace is for Rive diagnostics only; dispatch success is based on ledger projection.",
                });
                print_json(&Envelope::new(protocol, display))
            }
        },
        Commands::Scheduler { command } => match command {
            SchedulerCommands::Run {
                root_work_node_id,
                runner,
                workers,
                command_id,
                max_parallel,
                acceptance_mode,
                workspace_mode,
                opencode_bin,
                timeout_seconds,
            } => {
                let workspace = find_workspace(&std::env::current_dir()?)
                    .map_err(|_| anyhow!("workspace not initialized"))?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let trace_store = DebugTraceStore::open(&workspace.db_path())?;
                trace_store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let scheduler =
                    SchedulerService::new(&workspace, &store, &trace_store, &snapshot_store);
                let protocol = scheduler.run(SchedulerRunInput {
                    root_work_node_id,
                    runner,
                    workers,
                    command_id,
                    max_parallel,
                    acceptance_mode,
                    workspace_mode,
                    opencode_bin,
                    timeout_seconds,
                })?;
                let display = serde_json::json!({
                    "summary": format!(
                        "Scheduler {} ended {} with root {} {}",
                        protocol.scheduler.scheduler_run_id,
                        protocol.scheduler.state,
                        protocol.scheduler.root_work_node_id,
                        protocol.root_work.state
                    ),
                    "trace_note": "Debug trace is for Rive diagnostics only; scheduler success is based on Work DAG projection.",
                });
                print_json(&Envelope::new(protocol, display))
            }
            SchedulerCommands::Status {
                scheduler_run_id,
                root_work_node_id,
            } => {
                let workspace = find_workspace(&std::env::current_dir()?)
                    .map_err(|_| anyhow!("workspace not initialized"))?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let trace_store = DebugTraceStore::open(&workspace.db_path())?;
                trace_store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let scheduler =
                    SchedulerService::new(&workspace, &store, &trace_store, &snapshot_store);
                let protocol = scheduler.status(SchedulerStatusInput {
                    scheduler_run_id,
                    root_work_node_id,
                })?;
                let display = serde_json::json!({
                    "summary": format!(
                        "Scheduler status root {} {}",
                        protocol.root_work.work_node_id,
                        protocol.root_work.state
                    ),
                    "trace_note": "Debug trace is for Rive diagnostics only; scheduler status is based on Work DAG and scheduler ledgers.",
                });
                print_json(&Envelope::new(protocol, display))
            }
            SchedulerCommands::Resume {
                scheduler_run_id,
                root_work_node_id,
                workers,
                command_id,
                max_parallel,
                acceptance_mode,
                workspace_mode,
                opencode_bin,
                timeout_seconds,
            } => {
                let workspace = find_workspace(&std::env::current_dir()?)
                    .map_err(|_| anyhow!("workspace not initialized"))?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let trace_store = DebugTraceStore::open(&workspace.db_path())?;
                trace_store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let scheduler =
                    SchedulerService::new(&workspace, &store, &trace_store, &snapshot_store);
                let protocol = scheduler.resume(SchedulerResumeInput {
                    scheduler_run_id,
                    root_work_node_id,
                    workers,
                    command_id,
                    max_parallel,
                    acceptance_mode,
                    workspace_mode,
                    opencode_bin,
                    timeout_seconds,
                })?;
                let display = serde_json::json!({
                    "summary": format!(
                        "Scheduler resume {} ended {} with root {} {}",
                        protocol.scheduler.scheduler_run_id,
                        protocol.scheduler.state,
                        protocol.scheduler.root_work_node_id,
                        protocol.root_work.state
                    ),
                    "trace_note": "Debug trace is for Rive diagnostics only; resume success is based on Work DAG projection.",
                });
                print_json(&Envelope::new(protocol, display))
            }
        },
        Commands::Workflow { command } => match command {
            WorkflowCommands::Validate { path } => {
                let workspace = find_workspace(&std::env::current_dir()?).unwrap_or_else(|_| {
                    let root = std::env::current_dir().expect("current dir");
                    rive::workspace::Workspace { root }
                });
                let store = EventStore::open(&workspace.db_path()).or_else(|_| {
                    let temp = tempfile_db_path();
                    EventStore::open(&temp)
                })?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let service = WorkflowService::new(&workspace, &store, &snapshot_store);
                let protocol = service.validate_path(&path)?;
                let display = serde_json::json!({
                    "summary": format!(
                        "Workflow {}@{} valid with {} nodes",
                        protocol.template_id, protocol.version, protocol.node_count
                    ),
                });
                print_json(&Envelope::new(protocol, display))
            }
            WorkflowCommands::Import { path, command_id } => {
                let workspace = find_workspace(&std::env::current_dir()?)
                    .map_err(|_| anyhow!("workspace not initialized"))?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let service = WorkflowService::new(&workspace, &store, &snapshot_store);
                let protocol = service.import(WorkflowImportInput { path, command_id })?;
                let display = serde_json::json!({
                    "summary": format!(
                        "Imported workflow {}@{} {}",
                        protocol.template_id, protocol.version, protocol.idempotency_status
                    ),
                });
                print_json(&Envelope::new(protocol, display))
            }
            WorkflowCommands::List => {
                let workspace = find_workspace(&std::env::current_dir()?)
                    .map_err(|_| anyhow!("workspace not initialized"))?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let service = WorkflowService::new(&workspace, &store, &snapshot_store);
                let protocol = service.list()?;
                let display = serde_json::json!({
                    "summary": format!("{} workflow templates", protocol.templates.len()),
                });
                print_json(&Envelope::new(protocol, display))
            }
            WorkflowCommands::Show {
                template_id,
                version,
            } => {
                let workspace = find_workspace(&std::env::current_dir()?)
                    .map_err(|_| anyhow!("workspace not initialized"))?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let service = WorkflowService::new(&workspace, &store, &snapshot_store);
                let protocol = service.show(&template_id, version)?;
                let display = serde_json::json!({
                    "summary": format!(
                        "Workflow {}@{} {}",
                        protocol.template_id, protocol.version, protocol.template_hash
                    ),
                });
                print_json(&Envelope::new(protocol, display))
            }
            WorkflowCommands::Run {
                template_id,
                params,
                command_id,
                no_scheduler,
                runner,
                workers,
                max_parallel,
                acceptance_mode,
                workspace_mode,
                opencode_bin,
                timeout_seconds,
            } => {
                let workspace = find_workspace(&std::env::current_dir()?)
                    .map_err(|_| anyhow!("workspace not initialized"))?;
                let store = EventStore::open(&workspace.db_path())?;
                store.init_schema()?;
                let snapshot_store = LocalSnapshotStore::new(&workspace);
                let service = WorkflowService::new(&workspace, &store, &snapshot_store);
                let scheduler_request = if no_scheduler {
                    None
                } else {
                    if runner != "opencode" {
                        return Err(anyhow!("scheduler runner not supported: {runner}"));
                    }
                    if workers.is_empty() {
                        return Err(anyhow!("scheduler worker is required"));
                    }
                    if max_parallel == 0 {
                        return Err(anyhow!("scheduler max parallel must be greater than zero"));
                    }
                    if timeout_seconds == 0 {
                        return Err(anyhow!("runner timeout must be greater than zero"));
                    }
                    if !matches!(
                        acceptance_mode.as_str(),
                        "manual" | "auto-reported" | "auto-committed"
                    ) {
                        return Err(anyhow!("invalid acceptance mode: {acceptance_mode}"));
                    }
                    if !matches!(workspace_mode.as_str(), "shared" | "worktree") {
                        return Err(anyhow!("workspace mode not supported: {workspace_mode}"));
                    }
                    Some(WorkflowSchedulerRequest {
                        runner,
                        workers,
                        max_parallel,
                        acceptance_mode,
                        workspace_mode,
                        timeout_seconds,
                        opencode_bin: opencode_bin.clone(),
                    })
                };
                let mut protocol = service.run(WorkflowRunInput {
                    template_id,
                    command_id: command_id.clone(),
                    params: parse_params(params)?,
                    scheduler_request: scheduler_request.clone(),
                })?;
                if let Some(request) = scheduler_request {
                    if protocol.idempotency_status == "inserted" {
                        let trace_store = DebugTraceStore::open(&workspace.db_path())?;
                        trace_store.init_schema()?;
                        let scheduler = SchedulerService::new(
                            &workspace,
                            &store,
                            &trace_store,
                            &snapshot_store,
                        );
                        let scheduler_command_id =
                            format!("workflow:{}:scheduler", protocol.workflow_run_id);
                        match scheduler.run(SchedulerRunInput {
                            root_work_node_id: protocol.root_work_node_id.clone(),
                            runner: request.runner,
                            workers: request.workers,
                            command_id: scheduler_command_id.clone(),
                            max_parallel: request.max_parallel,
                            acceptance_mode: request.acceptance_mode,
                            workspace_mode: request.workspace_mode,
                            opencode_bin: request.opencode_bin,
                            timeout_seconds: request.timeout_seconds,
                        }) {
                            Ok(scheduler_protocol) => {
                                protocol = service.attach_scheduler(
                                    &protocol.workflow_run_id,
                                    &scheduler_protocol.scheduler.scheduler_run_id,
                                    &scheduler_protocol.scheduler.state,
                                    "inserted",
                                )?;
                            }
                            Err(err) => {
                                if let Some(run) =
                                    store.get_scheduler_run_by_command_id(&scheduler_command_id)?
                                {
                                    let _ = service.attach_scheduler(
                                        &protocol.workflow_run_id,
                                        &run.scheduler_run_id,
                                        &run.state,
                                        "inserted",
                                    )?;
                                }
                                return Err(err);
                            }
                        }
                    }
                }
                let display = serde_json::json!({
                    "summary": format!(
                        "Workflow run {} root {} state {}",
                        protocol.workflow_run_id, protocol.root_work_node_id, protocol.state
                    ),
                });
                print_json(&Envelope::new(protocol, display))
            }
        },
    }
}

fn tempfile_db_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "rive-workflow-validate-{}.db",
        uuid::Uuid::new_v4().simple()
    ))
}

fn parse_params(values: Vec<String>) -> Result<Vec<(String, String)>> {
    values
        .into_iter()
        .map(|value| {
            let (key, value) = value
                .split_once('=')
                .ok_or_else(|| anyhow!("workflow param must be key=value: {value}"))?;
            Ok((key.to_string(), value.to_string()))
        })
        .collect()
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn error_envelope(error: &anyhow::Error) -> ErrorEnvelope {
    let message = error.to_string();
    let lower = message.to_lowercase();
    let (code, action) = if lower.contains("workspace not initialized") {
        ("workspace_not_initialized", "run_rive_init")
    } else if lower.contains("no .rive workspace") {
        ("workspace_not_found", "run_rive_init")
    } else if lower.contains("workflow template version conflict") {
        ("workflow_template_version_conflict", "fix_arguments")
    } else if lower.contains("workflow template not found") {
        ("workflow_template_not_found", "fix_arguments")
    } else if lower.contains("workflow scheduler execution not supported") {
        ("workflow_scheduler_not_supported", "fix_arguments")
    } else if lower.contains("unsupported workflow api version") {
        ("workflow_api_version_unsupported", "fix_arguments")
    } else if lower.contains("workflow graph cycle") {
        ("workflow_graph_cycle", "fix_arguments")
    } else if lower.contains("workflow edge endpoint not found") {
        ("workflow_edge_endpoint_not_found", "fix_arguments")
    } else if lower.contains("workflow consumes must be dependency predecessor")
        || lower.contains("workflow consumes unknown node")
    {
        ("workflow_consumes_invalid", "fix_arguments")
    } else if lower.contains("workflow gated capability") {
        ("workflow_capability_gate_invalid", "fix_arguments")
    } else if lower.contains("workflow missing param") {
        ("workflow_param_missing", "fix_arguments")
    } else if lower.contains("workflow unknown param") {
        ("workflow_param_unknown", "fix_arguments")
    } else if lower.contains("workflow invalid enum param")
        || lower.contains("workflow invalid boolean param")
        || lower.contains("workflow param must be key=value")
    {
        ("workflow_param_invalid", "fix_arguments")
    } else if lower.contains("workflow unresolved template variable") {
        ("workflow_template_unresolved", "fix_arguments")
    } else if lower.contains("runner agent token required") {
        ("runner_agent_token_required", "fix_arguments")
    } else if lower.contains("invalid agent token") {
        ("agent_token_invalid", "stop_and_report")
    } else if lower.contains("runner agent must be worker")
        || lower.contains("runner agent must be orchestrator")
    {
        ("runner_agent_role_invalid", "fix_arguments")
    } else if lower.contains("runner worker must be worker") {
        ("runner_worker_role_invalid", "fix_arguments")
    } else if lower.contains("orchestrator worker is required") {
        ("orchestrator_worker_required", "fix_arguments")
    } else if lower.contains("orchestrator objective is required") {
        ("orchestrator_objective_required", "fix_arguments")
    } else if lower.contains("scheduler worker is required") {
        ("scheduler_worker_required", "fix_arguments")
    } else if lower.contains("scheduler runner not supported") {
        ("scheduler_runner_not_supported", "fix_arguments")
    } else if lower.contains("scheduler max parallel") {
        ("scheduler_max_parallel_invalid", "fix_arguments")
    } else if lower.contains("invalid acceptance mode") {
        ("invalid_acceptance_mode", "fix_arguments")
    } else if lower.contains("workspace mode not supported") {
        ("workspace_mode_not_supported", "fix_arguments")
    } else if lower.contains("worktree backend unavailable") {
        ("worktree_backend_unavailable", "fix_installation")
    } else if lower.contains("worktree create failed") {
        ("worktree_create_failed", "inspect_backend")
    } else if lower.contains("worktree commit failed") {
        ("worktree_commit_failed", "inspect_backend")
    } else if lower.contains("worktree abort failed") {
        ("worktree_abort_failed", "inspect_backend")
    } else if lower.contains("worktree ref not committed") {
        ("worktree_ref_not_committed", "inspect_branch")
    } else if lower.contains("worktree not found") {
        ("worktree_not_found", "inspect_branch")
    } else if lower.contains("branch not pending") {
        ("branch_not_pending", "inspect_branch")
    } else if lower.contains("branch integration conflict") {
        ("branch_integration_conflict", "inspect_branch")
    } else if lower.contains("branch not found") {
        ("branch_not_found", "inspect_branch")
    } else if lower.contains("work node already claimed") {
        ("work_node_already_claimed", "inspect_projection")
    } else if lower.contains("work scheduler stalled") {
        ("work_scheduler_stalled", "inspect_projection")
    } else if lower.contains("work not done") {
        ("work_not_done", "inspect_projection")
    } else if lower.contains("work graph not closed") {
        ("work_graph_not_closed", "inspect_projection")
    } else if lower.contains("orchestrator workspace mutation") {
        ("orchestrator_workspace_mutation", "inspect_projection")
    } else if lower.contains("opencode not found") {
        ("opencode_not_found", "fix_installation")
    } else if lower.contains("codex not found") {
        ("codex_not_found", "fix_installation")
    } else if lower.contains("opencode timeout") {
        ("opencode_timeout", "inspect_projection")
    } else if lower.contains("codex timeout") {
        ("codex_timeout", "inspect_projection")
    } else if lower.contains("opencode exit failed") {
        ("opencode_exit_failed", "inspect_projection")
    } else if lower.contains("codex exit failed") {
        ("codex_exit_failed", "inspect_projection")
    } else if lower.contains("dispatch not reported") {
        ("dispatch_not_reported", "inspect_projection")
    } else if lower.contains("work graph cycle") {
        ("work_graph_cycle", "inspect_projection")
    } else if lower.contains("invalid work edge type") {
        ("invalid_work_edge_type", "fix_arguments")
    } else if lower.contains("invalid work node kind") {
        ("invalid_work_node_kind", "fix_arguments")
    } else if lower.contains("work node not ready") {
        ("work_node_not_ready", "inspect_projection")
    } else if lower.contains("work node not reviewable") {
        ("work_node_not_reviewable", "inspect_projection")
    } else if lower.contains("work node not found") {
        ("work_node_not_found", "fix_arguments")
    } else if lower.contains("dispatch already bound to work node") {
        ("work_dispatch_binding_conflict", "inspect_projection")
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
