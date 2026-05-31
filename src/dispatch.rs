use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::facts::{protocol_from_fact, ActorEnv, FactProtocol};
use crate::snapshot::{read_manifest, SnapshotStore};
use crate::store::{
    AgentRecord, AgentRole, CancelDispatchInput, DispatchRecord, DispatchState,
    DispatchTransitionInput, EventRecord, EventStore, FactRecord, IdempotencyResolution,
    InsertDispatchInput, InsertFactInput,
};
use crate::workspace::Workspace;

#[derive(Debug)]
pub struct AddAgentInput {
    pub name: String,
    pub role: AgentRole,
    pub token: Option<String>,
}

#[derive(Debug)]
pub struct AddAgentOutcome {
    pub agent: AgentRecord,
    pub token: String,
}

#[derive(Debug)]
pub struct CreateDispatchInput {
    pub command_id: String,
    pub target_agent: String,
    pub title: String,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub enum CreateDispatchOutcome {
    Inserted(DispatchRecord),
    Replayed(DispatchRecord),
}

#[derive(Debug)]
pub struct CancelDispatchCommand {
    pub command_id: String,
    pub dispatch_id: String,
    pub reason: String,
}

#[derive(Debug)]
pub enum CancelDispatchOutcome {
    Inserted(DispatchRecord),
    Replayed(DispatchRecord),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportStatus {
    Done,
    Blocked,
    Failed,
}

impl ReportStatus {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "done" => Ok(Self::Done),
            "blocked" => Ok(Self::Blocked),
            "failed" => Ok(Self::Failed),
            _ => Err(anyhow!("invalid report status: {value}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
        }
    }

    fn dispatch_state(self) -> DispatchState {
        match self {
            Self::Done => DispatchState::Reported,
            Self::Blocked => DispatchState::Blocked,
            Self::Failed => DispatchState::Failed,
        }
    }
}

#[derive(Debug)]
pub struct DispatchFactInput {
    pub command_id: String,
    pub actor: ActorEnv,
    pub dispatch_id: String,
    pub snapshot_ids: Vec<String>,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub enum DispatchFactOutcome {
    Inserted {
        fact: FactRecord,
        dispatch: DispatchRecord,
    },
    Replayed {
        fact: FactRecord,
        dispatch: DispatchRecord,
    },
}

#[derive(Debug, Serialize)]
pub struct AgentProtocol {
    pub agent_id: String,
    pub name: String,
    pub role: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct AddAgentProtocol {
    pub agent: AgentProtocol,
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct AgentListProtocol {
    pub agents: Vec<AgentProtocol>,
}

#[derive(Debug, Serialize)]
pub struct DispatchProtocol {
    pub dispatch_id: String,
    pub command_id: String,
    pub target_agent_id: String,
    pub title: String,
    pub body_hash: String,
    pub body_blob_ref: String,
    pub state: String,
    pub latest_fact_event_id: Option<String>,
    pub latest_report_status: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub allowed_next_actions: Vec<&'static str>,
    pub idempotency_status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct DispatchListProtocol {
    pub dispatches: Vec<DispatchProtocol>,
}

#[derive(Debug, Serialize)]
pub struct DispatchFactProtocol {
    pub fact: FactProtocol,
    pub dispatch: DispatchProtocol,
}

pub struct DispatchService<'a, S: SnapshotStore> {
    workspace: &'a Workspace,
    store: &'a EventStore,
    blob_store: &'a S,
}

impl<'a, S: SnapshotStore> DispatchService<'a, S> {
    pub fn new(workspace: &'a Workspace, store: &'a EventStore, blob_store: &'a S) -> Self {
        Self {
            workspace,
            store,
            blob_store,
        }
    }

    pub fn add_agent(&self, input: AddAgentInput) -> Result<AddAgentOutcome> {
        if input.name.trim().is_empty() {
            return Err(anyhow!("missing agent name"));
        }
        let token = input.token.unwrap_or_else(|| prefixed_id("tok"));
        let agent = AgentRecord {
            agent_id: prefixed_id("agent"),
            name: input.name,
            role: input.role,
            token_hash: token_hash(&token),
            created_at: Utc::now(),
            status: "idle".to_string(),
        };
        let event_id = prefixed_id("evt");
        let payload = json!({
            "protocol_version": "rive.dispatch.v0",
            "event_id": event_id,
            "event_type": "agent.added",
            "workspace_id": workspace_id(self.workspace),
            "agent_id": agent.agent_id,
            "name": agent.name,
            "role": agent.role.as_str(),
            "status": agent.status,
            "created_at": agent.created_at,
        });
        self.store.insert_agent_with_event(
            &EventRecord {
                event_id,
                event_type: "agent.added".to_string(),
                created_at: agent.created_at,
                payload,
            },
            &agent,
        )?;
        Ok(AddAgentOutcome { agent, token })
    }

    pub fn create_dispatch(&self, input: CreateDispatchInput) -> Result<CreateDispatchOutcome> {
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        if input.title.trim().is_empty() {
            return Err(anyhow!("missing dispatch title"));
        }
        if input.body.is_empty() {
            return Err(anyhow!("dispatch body is required"));
        }
        let target = self
            .store
            .get_agent(&input.target_agent)?
            .ok_or_else(|| anyhow!("agent not found: {}", input.target_agent))?;
        if target.role != AgentRole::Worker {
            return Err(anyhow!("dispatch target must be worker"));
        }

        let body_sha = sha256_hex(&input.body);
        let body_hash = format!("sha256:{body_sha}");
        if let Some(existing) = self.store.get_dispatch_by_command_id(&input.command_id)? {
            if existing.target_agent_id == target.agent_id
                && existing.title == input.title
                && existing.body_hash == body_hash
            {
                return Ok(CreateDispatchOutcome::Replayed(existing));
            }
            return Err(anyhow!("idempotency conflict"));
        }

        let body_blob_ref = self.blob_store.write_blob(&body_sha, &input.body)?;
        let dispatch_id = prefixed_id("disp");
        let event_id = prefixed_id("evt");
        let created_at = Utc::now();
        let payload = json!({
            "protocol_version": "rive.dispatch.v0",
            "event_id": event_id,
            "command_id": input.command_id,
            "event_type": "dispatch.created",
            "workspace_id": workspace_id(self.workspace),
            "dispatch_id": dispatch_id,
            "target_agent_id": target.agent_id,
            "title": input.title,
            "body_hash": body_hash,
            "body_blob_ref": body_blob_ref,
            "created_at": created_at,
        });
        let insert = InsertDispatchInput {
            event: EventRecord {
                event_id,
                event_type: "dispatch.created".to_string(),
                created_at,
                payload,
            },
            command_id: input.command_id,
            dispatch_id,
            target_agent_id: target.agent_id,
            title: input.title,
            body_hash,
            body_blob_ref,
        };
        match self.store.insert_dispatch_idempotent(&insert)? {
            IdempotencyResolution::Inserted(record) => Ok(CreateDispatchOutcome::Inserted(record)),
            IdempotencyResolution::Replayed(record) => Ok(CreateDispatchOutcome::Replayed(record)),
            IdempotencyResolution::Conflict(_) => Err(anyhow!("idempotency conflict")),
        }
    }

    pub fn cancel_dispatch(&self, input: CancelDispatchCommand) -> Result<CancelDispatchOutcome> {
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        let reason_hash = format!("sha256:{}", sha256_hex(input.reason.as_bytes()));
        let event_id = prefixed_id("evt");
        let created_at = Utc::now();
        let payload = json!({
            "protocol_version": "rive.dispatch.v0",
            "event_id": event_id,
            "command_id": input.command_id,
            "event_type": "dispatch.cancelled",
            "workspace_id": workspace_id(self.workspace),
            "dispatch_id": input.dispatch_id,
            "reason_hash": reason_hash,
            "reason": input.reason,
            "created_at": created_at,
        });
        let cancel = CancelDispatchInput {
            event: EventRecord {
                event_id,
                event_type: "dispatch.cancelled".to_string(),
                created_at,
                payload,
            },
            command_id: input.command_id,
            dispatch_id: input.dispatch_id,
            reason_hash,
        };
        match self.store.cancel_dispatch_idempotent(&cancel)? {
            IdempotencyResolution::Inserted(record) => Ok(CancelDispatchOutcome::Inserted(record)),
            IdempotencyResolution::Replayed(record) => Ok(CancelDispatchOutcome::Replayed(record)),
            IdempotencyResolution::Conflict(_) => Err(anyhow!("idempotency conflict")),
        }
    }

    pub fn record_status(&self, input: DispatchFactInput) -> Result<DispatchFactOutcome> {
        self.record_dispatch_fact(input, "status", None)
    }

    pub fn record_report(
        &self,
        input: DispatchFactInput,
        status: ReportStatus,
    ) -> Result<DispatchFactOutcome> {
        self.record_dispatch_fact(input, "report", Some(status))
    }

    fn record_dispatch_fact(
        &self,
        input: DispatchFactInput,
        fact_type: &'static str,
        report_status: Option<ReportStatus>,
    ) -> Result<DispatchFactOutcome> {
        validate_actor(&input.actor)?;
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        if input.body.is_empty() {
            return Err(anyhow!("fact body is required"));
        }

        let actor = self.authenticate_actor(&input.actor)?;
        if actor.role != AgentRole::Worker {
            return Err(anyhow!("actor role not allowed"));
        }
        let dispatch = self
            .store
            .get_dispatch(&input.dispatch_id)?
            .ok_or_else(|| anyhow!("dispatch not found: {}", input.dispatch_id))?;
        if dispatch.target_agent_id != actor.agent_id {
            return Err(anyhow!("dispatch not assigned to actor"));
        }

        let evidence_refs = self.build_evidence_refs(&input.snapshot_ids)?;
        let body_sha = sha256_hex(&input.body);
        let body_hash = format!("sha256:{body_sha}");
        let workspace_id = workspace_id(self.workspace);
        if let Some(existing) = self.store.get_fact_by_command_id(&input.command_id)? {
            let current = self
                .store
                .get_dispatch(&input.dispatch_id)?
                .ok_or_else(|| anyhow!("dispatch not found: {}", input.dispatch_id))?;
            if existing.actor_agent_id == input.actor.agent_id
                && existing.workspace_id == workspace_id
                && existing.actor_run_id == input.actor.run_id
                && existing.fact_type == fact_type
                && existing.body_hash == body_hash
                && existing.evidence_refs == evidence_refs
                && self.fact_event_dispatch_id(&existing)? == Some(input.dispatch_id.clone())
            {
                return Ok(DispatchFactOutcome::Replayed {
                    fact: existing,
                    dispatch: current,
                });
            }
            return Err(anyhow!("idempotency conflict"));
        }

        match report_status {
            Some(_) if !dispatch.state.is_open_for_report() => {
                return Err(anyhow!(
                    "dispatch closed: {} is {}",
                    dispatch.dispatch_id,
                    dispatch.state.as_str()
                ))
            }
            None if !dispatch.state.is_open_for_status() => {
                return Err(anyhow!(
                    "dispatch closed: {} is {}",
                    dispatch.dispatch_id,
                    dispatch.state.as_str()
                ))
            }
            _ => {}
        }

        let body_blob_ref = self.blob_store.write_blob(&body_sha, &input.body)?;
        let event_id = prefixed_id("evt");
        let created_at = Utc::now();
        let event_type = if report_status.is_some() {
            "dispatch.reported"
        } else {
            "dispatch.status_updated"
        };
        let next_state = report_status
            .map(ReportStatus::dispatch_state)
            .unwrap_or_else(|| dispatch.state.clone());
        let latest_report_status = report_status
            .map(|status| status.as_str().to_string())
            .or_else(|| dispatch.latest_report_status.clone());
        let payload = json!({
            "protocol_version": "rive.dispatch.v0",
            "event_id": event_id,
            "command_id": input.command_id,
            "event_type": event_type,
            "workspace_id": workspace_id,
            "actor": {
                "kind": "agent",
                "agent_id": actor.agent_id,
                "run_id": input.actor.run_id,
            },
            "dispatch_id": input.dispatch_id,
            "fact_type": fact_type,
            "report_status": report_status.map(ReportStatus::as_str),
            "body_hash": body_hash,
            "body_blob_ref": body_blob_ref,
            "evidence_refs": evidence_refs.clone(),
            "created_at": created_at,
        });
        let insert = DispatchTransitionInput {
            fact: InsertFactInput {
                event: EventRecord {
                    event_id,
                    event_type: event_type.to_string(),
                    created_at,
                    payload,
                },
                command_id: input.command_id,
                workspace_id,
                actor_agent_id: input.actor.agent_id,
                actor_run_id: input.actor.run_id,
                fact_type: fact_type.to_string(),
                body_hash,
                body_blob_ref,
                evidence_refs,
            },
            dispatch_id: input.dispatch_id,
            next_state,
            latest_report_status,
        };

        match self
            .store
            .transition_dispatch_with_fact_idempotent(&insert)?
        {
            IdempotencyResolution::Inserted((fact, dispatch)) => {
                Ok(DispatchFactOutcome::Inserted { fact, dispatch })
            }
            IdempotencyResolution::Replayed((fact, dispatch)) => {
                Ok(DispatchFactOutcome::Replayed { fact, dispatch })
            }
            IdempotencyResolution::Conflict(_) => Err(anyhow!("idempotency conflict")),
        }
    }

    fn authenticate_actor(&self, actor: &ActorEnv) -> Result<AgentRecord> {
        let agent = self
            .store
            .get_agent(&actor.agent_id)?
            .ok_or_else(|| anyhow!("agent not found: {}", actor.agent_id))?;
        if agent.token_hash != token_hash(&actor.agent_token) {
            return Err(anyhow!("invalid agent token"));
        }
        Ok(agent)
    }

    fn build_evidence_refs(&self, snapshot_ids: &[String]) -> Result<serde_json::Value> {
        if snapshot_ids.is_empty() {
            return Err(anyhow!("evidence snapshot is required"));
        }
        let mut evidence_refs = Vec::new();
        for snapshot_id in snapshot_ids {
            let snapshot = self
                .store
                .get_snapshot(snapshot_id)?
                .ok_or_else(|| anyhow!("evidence not found: {snapshot_id}"))?;
            read_manifest(self.workspace, &snapshot)?;
            evidence_refs.push(json!({
                "kind": "snapshot",
                "snapshot_id": snapshot.snapshot_id,
                "manifest_hash": snapshot.manifest_hash,
            }));
        }
        Ok(serde_json::Value::Array(evidence_refs))
    }

    fn fact_event_dispatch_id(&self, fact: &FactRecord) -> Result<Option<String>> {
        let event = self
            .store
            .get_event_by_id(&fact.event_id)?
            .ok_or_else(|| anyhow!("fact event missing: {}", fact.event_id))?;
        Ok(event
            .payload
            .get("dispatch_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string))
    }
}

pub fn agent_protocol(agent: &AgentRecord) -> AgentProtocol {
    AgentProtocol {
        agent_id: agent.agent_id.clone(),
        name: agent.name.clone(),
        role: agent.role.as_str().to_string(),
        status: agent.status.clone(),
        created_at: agent.created_at,
    }
}

pub fn dispatch_protocol(
    dispatch: &DispatchRecord,
    idempotency_status: &'static str,
) -> DispatchProtocol {
    DispatchProtocol {
        dispatch_id: dispatch.dispatch_id.clone(),
        command_id: dispatch.command_id.clone(),
        target_agent_id: dispatch.target_agent_id.clone(),
        title: dispatch.title.clone(),
        body_hash: dispatch.body_hash.clone(),
        body_blob_ref: dispatch.body_blob_ref.clone(),
        state: dispatch.state.as_str().to_string(),
        latest_fact_event_id: dispatch.latest_fact_event_id.clone(),
        latest_report_status: dispatch.latest_report_status.clone(),
        created_at: dispatch.created_at,
        updated_at: dispatch.updated_at,
        allowed_next_actions: allowed_next_actions(&dispatch.state),
        idempotency_status,
    }
}

pub fn dispatch_fact_protocol(
    fact: &FactRecord,
    dispatch: &DispatchRecord,
    idempotency_status: &'static str,
) -> DispatchFactProtocol {
    DispatchFactProtocol {
        fact: protocol_from_fact(fact, idempotency_status),
        dispatch: dispatch_protocol(dispatch, idempotency_status),
    }
}

fn allowed_next_actions(state: &DispatchState) -> Vec<&'static str> {
    match state {
        DispatchState::Open | DispatchState::Blocked => {
            vec!["status", "report", "cancel", "inspect_evidence"]
        }
        DispatchState::Reported | DispatchState::Failed | DispatchState::Cancelled => {
            vec!["inspect_evidence"]
        }
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

fn workspace_id(workspace: &Workspace) -> String {
    workspace.root.display().to_string()
}

fn token_hash(token: &str) -> String {
    format!("sha256:{}", sha256_hex(token.as_bytes()))
}

fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}
