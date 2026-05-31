use std::io::Read;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use rive::facts::{
    protocol_from_fact, ActorEnv, FactDisplay, FactRecorder, FactType, RecordFactInput,
    RecordFactOutcome,
};
use rive::output::{Envelope, ErrorEnvelope};
use rive::snapshot::LocalSnapshotStore;
use rive::store::EventStore;
use rive::workspace::find_workspace;
use serde::Serialize;

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

fn error_envelope(error: &anyhow::Error) -> ErrorEnvelope {
    let message = error.to_string();
    let lower = message.to_lowercase();
    let (code, retryable, action) = if lower.contains("missing command id") {
        ("missing_command_id", false, "fix_arguments")
    } else if lower.contains("invalid fact type") {
        ("invalid_fact_type", false, "fix_arguments")
    } else if lower.contains("actor not authenticated") {
        ("actor_not_authenticated", false, "stop_and_report")
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
