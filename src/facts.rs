use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::snapshot::{read_manifest, SnapshotStore};
use crate::store::{EventRecord, EventStore, FactRecord, IdempotencyResolution, InsertFactInput};
use crate::workspace::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactType {
    Status,
    Report,
    Observation,
}

impl FactType {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "status" => Ok(Self::Status),
            "report" => Ok(Self::Report),
            "observation" => Ok(Self::Observation),
            _ => Err(anyhow!("invalid fact type: {value}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Report => "report",
            Self::Observation => "observation",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActorEnv {
    pub workspace: String,
    pub agent_id: String,
    pub agent_token: String,
    pub run_id: Option<String>,
}

#[derive(Debug)]
pub struct RecordFactInput {
    pub command_id: String,
    pub actor: ActorEnv,
    pub fact_type: FactType,
    pub snapshot_ids: Vec<String>,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub enum RecordFactOutcome {
    Inserted(FactRecord),
    Replayed(FactRecord),
}

#[derive(Debug, Serialize)]
pub struct FactProtocol {
    pub event_id: String,
    pub command_id: String,
    pub protocol_version: &'static str,
    pub workspace_id: String,
    pub actor: FactActorProtocol,
    pub fact_type: String,
    pub body_hash: String,
    pub body_blob_ref: String,
    pub evidence_refs: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub idempotency_status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct FactActorProtocol {
    pub kind: &'static str,
    pub agent_id: String,
    pub run_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FactDisplay {
    pub summary: String,
}

#[derive(Debug, Serialize)]
pub struct FactListProtocol {
    pub facts: Vec<FactProtocol>,
}

#[derive(Debug, Serialize)]
pub struct FactListDisplay {
    pub summary: String,
}

pub struct FactRecorder<'a, S: SnapshotStore> {
    workspace: &'a Workspace,
    event_store: &'a EventStore,
    blob_store: &'a S,
}

impl<'a, S: SnapshotStore> FactRecorder<'a, S> {
    pub fn new(workspace: &'a Workspace, event_store: &'a EventStore, blob_store: &'a S) -> Self {
        Self {
            workspace,
            event_store,
            blob_store,
        }
    }

    pub fn record(&self, input: RecordFactInput) -> Result<RecordFactOutcome> {
        validate_actor(&input.actor)?;
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        if input.snapshot_ids.is_empty() {
            return Err(anyhow!("evidence snapshot is required"));
        }
        if input.body.is_empty() {
            return Err(anyhow!("fact body is required"));
        }

        let mut evidence_refs = Vec::new();
        for snapshot_id in &input.snapshot_ids {
            let snapshot = self
                .event_store
                .get_snapshot(snapshot_id)?
                .ok_or_else(|| anyhow!("evidence not found: {snapshot_id}"))?;
            read_manifest(self.workspace, &snapshot)?;
            evidence_refs.push(json!({
                "kind": "snapshot",
                "snapshot_id": snapshot.snapshot_id,
                "manifest_hash": snapshot.manifest_hash,
            }));
        }

        let body_sha = sha256_hex(&input.body);
        let body_hash = format!("sha256:{body_sha}");
        let evidence_refs = serde_json::Value::Array(evidence_refs);
        let workspace_id = self.workspace.root.display().to_string();
        if let Some(existing) = self.event_store.get_fact_by_command_id(&input.command_id)? {
            if existing.actor_agent_id == input.actor.agent_id
                && existing.workspace_id == workspace_id
                && existing.actor_run_id == input.actor.run_id
                && existing.fact_type == input.fact_type.as_str()
                && existing.body_hash == body_hash
                && existing.evidence_refs == evidence_refs
            {
                return Ok(RecordFactOutcome::Replayed(existing));
            }
            return Err(anyhow!("idempotency conflict"));
        }

        let body_blob_ref = self.blob_store.write_blob(&body_sha, &input.body)?;
        let event_id = prefixed_id("evt");
        let created_at = Utc::now();
        let fact_type = input.fact_type.as_str().to_string();
        let payload = json!({
            "protocol_version": "rive.fact.v0",
            "event_id": event_id,
            "command_id": input.command_id,
            "event_type": "agent.fact.recorded",
            "workspace_id": workspace_id,
            "actor": {
                "kind": "agent",
                "agent_id": input.actor.agent_id,
                "run_id": input.actor.run_id,
            },
            "fact_type": fact_type,
            "body_hash": body_hash,
            "body_blob_ref": body_blob_ref,
            "evidence_refs": evidence_refs.clone(),
            "created_at": created_at,
        });
        let event = EventRecord {
            event_id,
            event_type: "agent.fact.recorded".to_string(),
            created_at,
            payload,
        };
        let insert = InsertFactInput {
            event,
            command_id: input.command_id,
            workspace_id,
            actor_agent_id: input.actor.agent_id,
            actor_run_id: input.actor.run_id,
            fact_type,
            body_hash,
            body_blob_ref,
            evidence_refs,
        };

        match self.event_store.insert_fact_idempotent(&insert)? {
            IdempotencyResolution::Inserted(record) => Ok(RecordFactOutcome::Inserted(record)),
            IdempotencyResolution::Replayed(record) => Ok(RecordFactOutcome::Replayed(record)),
            IdempotencyResolution::Conflict(_) => Err(anyhow!("idempotency conflict")),
        }
    }
}

pub fn protocol_from_fact(record: &FactRecord, idempotency_status: &'static str) -> FactProtocol {
    FactProtocol {
        event_id: record.event_id.clone(),
        command_id: record.command_id.clone(),
        protocol_version: "rive.fact.v0",
        workspace_id: record.workspace_id.clone(),
        actor: FactActorProtocol {
            kind: "agent",
            agent_id: record.actor_agent_id.clone(),
            run_id: record.actor_run_id.clone(),
        },
        fact_type: record.fact_type.clone(),
        body_hash: record.body_hash.clone(),
        body_blob_ref: record.body_blob_ref.clone(),
        evidence_refs: record.evidence_refs.clone(),
        created_at: record.created_at,
        idempotency_status,
    }
}

fn validate_actor(actor: &ActorEnv) -> Result<()> {
    if actor.workspace.trim().is_empty()
        || actor.agent_id.trim().is_empty()
        || actor.agent_token.trim().is_empty()
    {
        return Err(anyhow!("actor not authenticated"));
    }
    Ok(())
}

fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}
