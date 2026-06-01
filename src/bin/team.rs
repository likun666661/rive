use std::io::Read;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use rive::debug_trace::DebugTraceStore;
use rive::dispatch::{
    dispatch_fact_protocol, dispatch_protocol, DispatchFactInput, DispatchFactOutcome,
    DispatchListProtocol, DispatchService, ReportStatus,
};
use rive::facts::{
    protocol_from_fact, ActorEnv, FactDisplay, FactRecorder, FactType, RecordFactInput,
    RecordFactOutcome,
};
use rive::output::{Envelope, ErrorEnvelope};
use rive::runner::{TeamSendInput, TeamSendService};
use rive::snapshot::LocalSnapshotStore;
use rive::store::{AgentRecord, EventStore};
use rive::workspace::find_workspace;
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Parser)]
#[command(name = "team")]
#[command(version)]
#[command(about = "Rive agent-facing CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    SelfCheck,
    List,
    Status {
        #[arg(long)]
        dispatch: String,
        #[arg(long = "snapshot")]
        snapshots: Vec<String>,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long)]
        stdin: bool,
    },
    Report {
        #[arg(long)]
        dispatch: String,
        #[arg(long)]
        status: String,
        #[arg(long = "snapshot")]
        snapshots: Vec<String>,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long)]
        stdin: bool,
    },
    Send {
        #[arg(long = "to")]
        target: String,
        #[arg(long)]
        runner: String,
        #[arg(long)]
        title: String,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long)]
        wait: bool,
        #[arg(long = "timeout-seconds", default_value_t = 300)]
        timeout_seconds: u64,
        #[arg(long = "snapshot-path")]
        snapshot_paths: Vec<std::path::PathBuf>,
        #[arg(long = "opencode-bin")]
        opencode_bin: Option<std::path::PathBuf>,
        #[arg(long = "codex-bin")]
        codex_bin: Option<std::path::PathBuf>,
        #[arg(long = "trust-project")]
        trust_project: bool,
        #[arg(long)]
        stdin: bool,
    },
    Fact {
        #[command(subcommand)]
        command: FactCommands,
    },
}

#[derive(Subcommand)]
enum FactCommands {
    Record {
        #[arg(long = "type")]
        fact_type: String,
        #[arg(long = "snapshot")]
        snapshots: Vec<String>,
        #[arg(long = "command-id")]
        command_id: String,
        #[arg(long)]
        stdin: bool,
    },
}

#[derive(Debug, Serialize)]
struct SelfCheckProtocol {
    ok: bool,
    missing_env: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct SelfCheckDisplay {
    summary: String,
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
        Commands::SelfCheck => {
            let required = ["RIVE_WORKSPACE", "RIVE_AGENT_ID", "RIVE_AGENT_TOKEN"];
            let missing_env: Vec<&'static str> = required
                .iter()
                .copied()
                .filter(|key| {
                    std::env::var(key)
                        .ok()
                        .filter(|value| !value.is_empty())
                        .is_none()
                })
                .collect();
            let ok = missing_env.is_empty();
            let envelope = Envelope::new(
                SelfCheckProtocol { ok, missing_env },
                SelfCheckDisplay {
                    summary: if ok {
                        "team environment is ready".to_string()
                    } else {
                        "team environment is missing required variables".to_string()
                    },
                },
            );
            println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
            if !ok {
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::List => list_team_dispatches(),
        Commands::Status {
            dispatch,
            snapshots,
            command_id,
            stdin,
        } => record_dispatch_status(dispatch, snapshots, command_id, stdin),
        Commands::Report {
            dispatch,
            status,
            snapshots,
            command_id,
            stdin,
        } => record_dispatch_report(dispatch, status, snapshots, command_id, stdin),
        Commands::Send {
            target,
            runner,
            title,
            command_id,
            wait,
            timeout_seconds,
            snapshot_paths,
            opencode_bin,
            codex_bin,
            trust_project,
            stdin,
        } => send_delegation(
            target,
            runner,
            title,
            command_id,
            wait,
            timeout_seconds,
            snapshot_paths,
            opencode_bin,
            codex_bin,
            trust_project,
            stdin,
        ),
        Commands::Fact { command } => match command {
            FactCommands::Record {
                fact_type,
                snapshots,
                command_id,
                stdin,
            } => record_fact(fact_type, snapshots, command_id, stdin),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn send_delegation(
    target: String,
    runner: String,
    title: String,
    command_id: String,
    wait: bool,
    timeout_seconds: u64,
    snapshot_paths: Vec<std::path::PathBuf>,
    opencode_bin: Option<std::path::PathBuf>,
    codex_bin: Option<std::path::PathBuf>,
    trust_project: bool,
    stdin: bool,
) -> Result<()> {
    if !stdin {
        return Err(anyhow!("team send requires --stdin"));
    }
    let actor = actor_from_env()?;
    let workspace = find_workspace(std::path::Path::new(&actor.workspace))?;
    let store = EventStore::open(&workspace.db_path())?;
    store.init_schema()?;
    let trace_store = DebugTraceStore::open(&workspace.db_path())?;
    trace_store.init_schema()?;
    let snapshot_store = LocalSnapshotStore::new(&workspace);
    let service = TeamSendService::new(&workspace, &store, &trace_store, &snapshot_store);
    let mut task_body = Vec::new();
    std::io::stdin().read_to_end(&mut task_body)?;
    let protocol = service.send(TeamSendInput {
        actor,
        target,
        runner,
        title,
        command_id,
        opencode_bin,
        codex_bin,
        timeout_seconds,
        snapshot_paths,
        task_body,
        wait,
        trust_project,
    })?;
    let display = serde_json::json!({
        "summary": format!(
            "Delegation {} completed with dispatch {} {}",
            protocol.command_id,
            protocol.dispatch.dispatch_id,
            protocol.dispatch.state
        ),
        "trace_note": "Debug trace is for Rive diagnostics only; dispatch success is based on ledger projection.",
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&Envelope::new(protocol, display))?
    );
    Ok(())
}

fn list_team_dispatches() -> Result<()> {
    let actor = actor_from_env()?;
    let workspace = find_workspace(std::path::Path::new(&actor.workspace))?;
    let store = EventStore::open(&workspace.db_path())?;
    store.init_schema()?;
    let agent = authenticate_actor(&store, &actor)?;
    let dispatches = store.list_dispatches_for_agent(&agent.agent_id)?;
    let protocol = DispatchListProtocol {
        dispatches: dispatches
            .iter()
            .map(|dispatch| dispatch_protocol(dispatch, "read"))
            .collect(),
    };
    let display = serde_json::json!({
        "summary": format!("{} dispatches assigned to {}", dispatches.len(), agent.name),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&Envelope::new(protocol, display))?
    );
    Ok(())
}

fn record_dispatch_status(
    dispatch: String,
    snapshots: Vec<String>,
    command_id: String,
    stdin: bool,
) -> Result<()> {
    if !stdin {
        return Err(anyhow!("team status requires --stdin"));
    }
    let actor = actor_from_env()?;
    let workspace = find_workspace(std::path::Path::new(&actor.workspace))?;
    let store = EventStore::open(&workspace.db_path())?;
    store.init_schema()?;
    authenticate_actor(&store, &actor)?;
    let snapshot_store = LocalSnapshotStore::new(&workspace);
    let service = DispatchService::new(&workspace, &store, &snapshot_store);
    let mut body = Vec::new();
    std::io::stdin().read_to_end(&mut body)?;
    let outcome = service.record_status(DispatchFactInput {
        command_id,
        actor,
        dispatch_id: dispatch,
        snapshot_ids: snapshots,
        body,
    })?;
    let (fact, dispatch, idempotency_status) = match outcome {
        DispatchFactOutcome::Inserted { fact, dispatch } => (fact, dispatch, "inserted"),
        DispatchFactOutcome::Replayed { fact, dispatch } => (fact, dispatch, "replayed"),
    };
    let protocol = dispatch_fact_protocol(&fact, &dispatch, idempotency_status);
    let display = serde_json::json!({
        "summary": format!("Recorded status for dispatch {}", protocol.dispatch.dispatch_id),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&Envelope::new(protocol, display))?
    );
    Ok(())
}

fn record_dispatch_report(
    dispatch: String,
    status: String,
    snapshots: Vec<String>,
    command_id: String,
    stdin: bool,
) -> Result<()> {
    if !stdin {
        return Err(anyhow!("team report requires --stdin"));
    }
    let report_status = ReportStatus::parse(&status)?;
    let actor = actor_from_env()?;
    let workspace = find_workspace(std::path::Path::new(&actor.workspace))?;
    let store = EventStore::open(&workspace.db_path())?;
    store.init_schema()?;
    let snapshot_store = LocalSnapshotStore::new(&workspace);
    let service = DispatchService::new(&workspace, &store, &snapshot_store);
    let mut body = Vec::new();
    std::io::stdin().read_to_end(&mut body)?;
    let outcome = service.record_report(
        DispatchFactInput {
            command_id,
            actor,
            dispatch_id: dispatch,
            snapshot_ids: snapshots,
            body,
        },
        report_status,
    )?;
    let (fact, dispatch, idempotency_status) = match outcome {
        DispatchFactOutcome::Inserted { fact, dispatch } => (fact, dispatch, "inserted"),
        DispatchFactOutcome::Replayed { fact, dispatch } => (fact, dispatch, "replayed"),
    };
    let protocol = dispatch_fact_protocol(&fact, &dispatch, idempotency_status);
    let display = serde_json::json!({
        "summary": format!("Recorded {} report for dispatch {}", status, protocol.dispatch.dispatch_id),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&Envelope::new(protocol, display))?
    );
    Ok(())
}

fn record_fact(
    fact_type: String,
    snapshots: Vec<String>,
    command_id: String,
    stdin: bool,
) -> Result<()> {
    if !stdin {
        return Err(anyhow!("team fact record requires --stdin"));
    }
    let actor = actor_from_env()?;
    let workspace = find_workspace(std::path::Path::new(&actor.workspace))?;
    let store = EventStore::open(&workspace.db_path())?;
    store.init_schema()?;
    authenticate_actor(&store, &actor)?;
    let snapshot_store = LocalSnapshotStore::new(&workspace);
    let recorder = FactRecorder::new(&workspace, &store, &snapshot_store);
    let mut body = Vec::new();
    std::io::stdin().read_to_end(&mut body)?;
    let outcome = recorder.record(RecordFactInput {
        command_id,
        actor,
        fact_type: FactType::parse(&fact_type)?,
        snapshot_ids: snapshots,
        body,
    })?;

    let (record, idempotency_status) = match outcome {
        RecordFactOutcome::Inserted(record) => (record, "inserted"),
        RecordFactOutcome::Replayed(record) => (record, "replayed"),
    };
    let protocol = protocol_from_fact(&record, idempotency_status);
    let display = FactDisplay {
        summary: format!("Recorded {} fact {}", protocol.fact_type, protocol.event_id),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&Envelope::new(protocol, display))?
    );
    Ok(())
}

fn actor_from_env() -> Result<ActorEnv> {
    Ok(ActorEnv {
        workspace: required_env("RIVE_WORKSPACE")?,
        agent_id: required_env("RIVE_AGENT_ID")?,
        agent_token: required_env("RIVE_AGENT_TOKEN")?,
        run_id: std::env::var("RIVE_RUN_ID")
            .ok()
            .filter(|value| !value.is_empty()),
    })
}

fn required_env(key: &str) -> Result<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("actor not authenticated: missing {key}"))
}

fn authenticate_actor(store: &EventStore, actor: &ActorEnv) -> Result<AgentRecord> {
    let agent = store
        .get_agent(&actor.agent_id)?
        .ok_or_else(|| anyhow!("agent not found: {}", actor.agent_id))?;
    let presented_hash = token_hash(&actor.agent_token);
    let run_token_matches = match actor.run_id.as_deref() {
        Some(run_id) => store
            .get_agent_run(run_id)?
            .is_some_and(|run| run.agent_id == agent.agent_id && run.token_hash == presented_hash),
        None => false,
    };
    if agent.token_hash != presented_hash && !run_token_matches {
        return Err(anyhow!("invalid agent token"));
    }
    Ok(agent)
}

fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn error_envelope(error: &anyhow::Error) -> ErrorEnvelope {
    let message = error.to_string();
    let lower = message.to_lowercase();
    let (code, retryable, action) = if lower.contains("missing command id") {
        ("missing_command_id", false, "fix_arguments")
    } else if lower.contains("wait required") {
        ("wait_required", false, "fix_arguments")
    } else if lower.contains("runner not supported") {
        ("runner_not_supported", false, "fix_arguments")
    } else if lower.contains("target agent not found") {
        ("target_agent_not_found", false, "fix_arguments")
    } else if lower.contains("target role invalid") {
        ("target_role_invalid", false, "fix_arguments")
    } else if lower.contains("opencode not found") {
        ("opencode_not_found", false, "fix_installation")
    } else if lower.contains("codex not found") {
        ("codex_not_found", false, "fix_installation")
    } else if lower.contains("opencode timeout") {
        ("opencode_timeout", false, "inspect_projection")
    } else if lower.contains("codex timeout") {
        ("codex_timeout", false, "inspect_projection")
    } else if lower.contains("opencode exit failed") {
        ("opencode_exit_failed", false, "inspect_projection")
    } else if lower.contains("codex exit failed") {
        ("codex_exit_failed", false, "inspect_projection")
    } else if lower.contains("dispatch not reported") {
        ("dispatch_not_reported", false, "inspect_projection")
    } else if lower.contains("invalid report status") {
        ("invalid_report_status", false, "fix_arguments")
    } else if lower.contains("invalid fact type") {
        ("invalid_fact_type", false, "fix_arguments")
    } else if lower.contains("actor not authenticated") {
        ("actor_not_authenticated", false, "stop_and_report")
    } else if lower.contains("invalid agent token") {
        ("agent_token_invalid", false, "stop_and_report")
    } else if lower.contains("agent not found") {
        ("agent_not_found", false, "stop_and_report")
    } else if lower.contains("dispatch not assigned") {
        ("dispatch_not_assigned", false, "stop_and_report")
    } else if lower.contains("dispatch closed") {
        ("dispatch_closed", false, "inspect_projection")
    } else if lower.contains("dispatch not found") {
        ("dispatch_not_found", false, "fix_arguments")
    } else if lower.contains("agent role not allowed") {
        ("agent_role_not_allowed", false, "stop_and_report")
    } else if lower.contains("actor role not allowed") {
        ("actor_role_not_allowed", false, "stop_and_report")
    } else if lower.contains("no .rive workspace") {
        ("workspace_not_found", false, "fix_arguments")
    } else if lower.contains("evidence not found") {
        ("evidence_not_found", false, "fix_arguments")
    } else if lower.contains("manifest hash mismatch") {
        ("evidence_integrity_error", false, "inspect_projection")
    } else if lower.contains("idempotency conflict") {
        ("idempotency_conflict", false, "inspect_projection")
    } else {
        ("command_failed", false, "inspect_error")
    };

    ErrorEnvelope::new(code, retryable, action, message)
}
