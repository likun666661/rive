use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub event_id: String,
    pub event_type: String,
    pub created_at: DateTime<Utc>,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRecord {
    pub snapshot_id: String,
    pub event_id: String,
    pub created_at: DateTime<Utc>,
    pub backend: String,
    pub capture_root: String,
    pub label: Option<String>,
    pub agent_id: Option<String>,
    pub dispatch_id: Option<String>,
    pub manifest_path: String,
    pub manifest_hash: String,
    pub file_count: u64,
    pub total_bytes: u64,
}

pub struct EventStore {
    conn: Connection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactRecord {
    pub event_id: String,
    pub command_id: String,
    pub created_at: DateTime<Utc>,
    pub workspace_id: String,
    pub actor_agent_id: String,
    pub actor_run_id: Option<String>,
    pub fact_type: String,
    pub body_hash: String,
    pub body_blob_ref: String,
    pub evidence_refs: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentRole {
    Orchestrator,
    Worker,
}

impl AgentRole {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "orchestrator" => Ok(Self::Orchestrator),
            "worker" => Ok(Self::Worker),
            _ => anyhow::bail!("invalid agent role: {value}"),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Orchestrator => "orchestrator",
            Self::Worker => "worker",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub agent_id: String,
    pub name: String,
    pub role: AgentRole,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunRecord {
    pub run_id: String,
    pub agent_id: String,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DispatchState {
    Open,
    Reported,
    Blocked,
    Failed,
    Cancelled,
}

impl DispatchState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Reported => "reported",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_open_for_status(&self) -> bool {
        matches!(self, Self::Open | Self::Blocked)
    }

    pub fn is_open_for_report(&self) -> bool {
        matches!(self, Self::Open | Self::Blocked)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchRecord {
    pub dispatch_id: String,
    pub created_event_id: String,
    pub command_id: String,
    pub target_agent_id: String,
    pub title: String,
    pub body_hash: String,
    pub body_blob_ref: String,
    pub state: DispatchState,
    pub latest_fact_event_id: Option<String>,
    pub latest_report_status: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct InsertDispatchInput {
    pub event: EventRecord,
    pub command_id: String,
    pub dispatch_id: String,
    pub target_agent_id: String,
    pub title: String,
    pub body_hash: String,
    pub body_blob_ref: String,
}

#[derive(Debug, Clone)]
pub struct DispatchTransitionInput {
    pub fact: InsertFactInput,
    pub dispatch_id: String,
    pub next_state: DispatchState,
    pub latest_report_status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CancelDispatchInput {
    pub event: EventRecord,
    pub command_id: String,
    pub dispatch_id: String,
    pub reason_hash: String,
}

#[derive(Debug, Clone)]
pub struct InsertAgentRunInput {
    pub run_id: String,
    pub agent_id: String,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationRecord {
    pub command_id: String,
    pub source_agent_id: String,
    pub source_run_id: Option<String>,
    pub target_agent_id: String,
    pub worker_run_id: String,
    pub dispatch_id: String,
    pub runner: String,
    pub request_hash: String,
    pub state: String,
    pub child_executed: bool,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct InsertDelegationInput {
    pub command_id: String,
    pub source_agent_id: String,
    pub source_run_id: Option<String>,
    pub target_agent_id: String,
    pub worker_run_id: String,
    pub dispatch_id: String,
    pub runner: String,
    pub request_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CompleteDelegationInput {
    pub command_id: String,
    pub state: String,
    pub child_executed: bool,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct InsertFactInput {
    pub event: EventRecord,
    pub command_id: String,
    pub workspace_id: String,
    pub actor_agent_id: String,
    pub actor_run_id: Option<String>,
    pub fact_type: String,
    pub body_hash: String,
    pub body_blob_ref: String,
    pub evidence_refs: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkNodeRecord {
    pub work_node_id: String,
    pub command_id: String,
    pub kind: String,
    pub title: String,
    pub body_hash: Option<String>,
    pub body_blob_ref: Option<String>,
    pub status_input: String,
    pub node_version: i64,
    pub accepted_event_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkEdgeRecord {
    pub work_edge_id: String,
    pub command_id: String,
    pub edge_type: String,
    pub from_node_id: String,
    pub to_node_id: String,
    pub graph_version: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkDispatchBindingRecord {
    pub work_node_id: String,
    pub dispatch_id: String,
    pub binding_kind: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkRefBindingRecord {
    pub work_node_id: String,
    pub dispatch_id: String,
    pub fact_event_id: String,
    pub snapshot_id: Option<String>,
    pub artifact_ref: Option<String>,
    pub workspace_ref: Option<String>,
    pub diff_ref: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkRootBindingRecord {
    pub root_work_node_id: String,
    pub work_node_id: String,
    pub created_by_agent_id: Option<String>,
    pub created_by_run_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkNoteRecord {
    pub note_id: String,
    pub command_id: String,
    pub event_id: String,
    pub work_node_id: String,
    pub note_kind: String,
    pub body_hash: String,
    pub body_blob_ref: String,
    pub actor_agent_id: String,
    pub actor_run_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerRunRecord {
    pub scheduler_run_id: String,
    pub command_id: String,
    pub root_work_node_id: String,
    pub runner: String,
    pub max_parallel: i64,
    pub acceptance_mode: String,
    pub request_hash: String,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerNodeRunRecord {
    pub node_run_id: String,
    pub scheduler_run_id: String,
    pub work_node_id: String,
    pub dispatch_id: Option<String>,
    pub worker_agent_id: String,
    pub worker_run_id: Option<String>,
    pub state: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchWorkspaceRecord {
    pub branch_id: String,
    pub backend: String,
    pub root_work_node_id: String,
    pub work_node_id: String,
    pub dispatch_id: String,
    pub run_id: String,
    pub branch_name: String,
    pub branch_path: String,
    pub branch_ref: String,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchIntegrationRecord {
    pub integration_id: String,
    pub branch_id: String,
    pub work_node_id: String,
    pub dispatch_id: String,
    pub fact_event_id: Option<String>,
    pub branch_ref: String,
    pub diff_ref: Option<String>,
    pub state: String,
    pub commit_ref: Option<String>,
    pub rejection_reason_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplateRecord {
    pub template_id: String,
    pub latest_version: i64,
    pub latest_hash: String,
    pub title: String,
    pub source_ref: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplateVersionRecord {
    pub template_id: String,
    pub version: i64,
    pub template_hash: String,
    pub source_ref: String,
    pub spec_json: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRunRecord {
    pub workflow_run_id: String,
    pub command_id: String,
    pub template_id: String,
    pub template_version: i64,
    pub template_hash: String,
    pub params_json: Value,
    pub params_hash: String,
    pub request_hash: String,
    pub root_work_node_id: String,
    pub scheduler_run_id: Option<String>,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRunNodeRecord {
    pub workflow_run_id: String,
    pub node_template_id: String,
    pub work_node_id: String,
    pub output_contract_json: Value,
    pub capability_policy_json: Value,
}

#[derive(Debug, Clone)]
pub struct InsertWorkNodeInput {
    pub event: EventRecord,
    pub command_id: String,
    pub work_node_id: String,
    pub kind: String,
    pub title: String,
    pub body_hash: Option<String>,
    pub body_blob_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InsertSchedulerRunInput {
    pub scheduler_run_id: String,
    pub command_id: String,
    pub root_work_node_id: String,
    pub runner: String,
    pub max_parallel: i64,
    pub acceptance_mode: String,
    pub request_hash: String,
    pub state: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpdateSchedulerRunStateInput {
    pub scheduler_run_id: String,
    pub state: String,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct InsertSchedulerNodeRunInput {
    pub node_run_id: String,
    pub scheduler_run_id: String,
    pub work_node_id: String,
    pub worker_agent_id: String,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpdateSchedulerNodeRunInput {
    pub node_run_id: String,
    pub dispatch_id: Option<String>,
    pub worker_run_id: Option<String>,
    pub state: String,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct InsertBranchWorkspaceInput {
    pub branch_id: String,
    pub backend: String,
    pub root_work_node_id: String,
    pub work_node_id: String,
    pub dispatch_id: String,
    pub run_id: String,
    pub branch_name: String,
    pub branch_path: String,
    pub branch_ref: String,
    pub state: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpdateBranchWorkspaceInput {
    pub branch_id: String,
    pub state: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct InsertBranchIntegrationInput {
    pub integration_id: String,
    pub branch_id: String,
    pub work_node_id: String,
    pub dispatch_id: String,
    pub fact_event_id: Option<String>,
    pub branch_ref: String,
    pub diff_ref: Option<String>,
    pub state: String,
    pub commit_ref: Option<String>,
    pub rejection_reason_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpdateBranchIntegrationInput {
    pub integration_id: String,
    pub state: String,
    pub commit_ref: Option<String>,
    pub rejection_reason_hash: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RecordBranchCommandInput {
    pub command_id: String,
    pub integration_id: String,
    pub action: String,
    pub request_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpsertWorkflowTemplateInput {
    pub template_id: String,
    pub version: i64,
    pub template_hash: String,
    pub title: String,
    pub source_ref: String,
    pub spec_json: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct InsertWorkflowImportCommandInput {
    pub command_id: String,
    pub template_id: String,
    pub version: i64,
    pub template_hash: String,
    pub request_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct InsertWorkflowRunInput {
    pub workflow_run_id: String,
    pub command_id: String,
    pub template_id: String,
    pub template_version: i64,
    pub template_hash: String,
    pub params_json: Value,
    pub params_hash: String,
    pub request_hash: String,
    pub root_work_node_id: String,
    pub scheduler_run_id: Option<String>,
    pub state: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpdateWorkflowRunSchedulerInput {
    pub workflow_run_id: String,
    pub scheduler_run_id: String,
    pub state: String,
}

#[derive(Debug, Clone)]
pub struct InsertWorkflowRunNodeInput {
    pub workflow_run_id: String,
    pub node_template_id: String,
    pub work_node_id: String,
    pub output_contract_json: Value,
    pub capability_policy_json: Value,
}

#[derive(Debug, Clone)]
pub struct InsertWorkEdgeInput {
    pub event: EventRecord,
    pub command_id: String,
    pub work_edge_id: String,
    pub edge_type: String,
    pub from_node_id: String,
    pub to_node_id: String,
    pub graph_version: i64,
}

#[derive(Debug, Clone)]
pub struct BindWorkDispatchInput {
    pub work_node_id: String,
    pub dispatch_id: String,
    pub binding_kind: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct InsertWorkRefBindingInput {
    pub work_node_id: String,
    pub dispatch_id: String,
    pub fact_event_id: String,
    pub snapshot_id: Option<String>,
    pub artifact_ref: Option<String>,
    pub workspace_ref: Option<String>,
    pub diff_ref: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpdateWorkNodeStatusInput {
    pub event: EventRecord,
    pub command_id: String,
    pub work_node_id: String,
    pub status_input: String,
    pub accepted_event_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BindWorkRootInput {
    pub root_work_node_id: String,
    pub work_node_id: String,
    pub created_by_agent_id: Option<String>,
    pub created_by_run_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct InsertWorkNoteInput {
    pub event: EventRecord,
    pub command_id: String,
    pub note_id: String,
    pub work_node_id: String,
    pub note_kind: String,
    pub body_hash: String,
    pub body_blob_ref: String,
    pub actor_agent_id: String,
    pub actor_run_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum IdempotencyResolution<T> {
    Inserted(T),
    Replayed(T),
    Conflict(T),
}

impl EventStore {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            conn: Connection::open(path)?,
        })
    }

    pub fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS events (
              event_id TEXT PRIMARY KEY,
              event_type TEXT NOT NULL,
              created_at TEXT NOT NULL,
              payload_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS snapshots (
              snapshot_id TEXT PRIMARY KEY,
              event_id TEXT NOT NULL UNIQUE,
              created_at TEXT NOT NULL,
              backend TEXT NOT NULL,
              capture_root TEXT NOT NULL,
              label TEXT,
              agent_id TEXT,
              dispatch_id TEXT,
              manifest_path TEXT NOT NULL,
              manifest_hash TEXT NOT NULL,
              file_count INTEGER NOT NULL,
              total_bytes INTEGER NOT NULL,
              FOREIGN KEY(event_id) REFERENCES events(event_id)
            );
            CREATE TABLE IF NOT EXISTS facts (
              event_id TEXT PRIMARY KEY,
              command_id TEXT NOT NULL UNIQUE,
              created_at TEXT NOT NULL,
              workspace_id TEXT NOT NULL,
              actor_agent_id TEXT NOT NULL,
              actor_run_id TEXT,
              fact_type TEXT NOT NULL,
              body_hash TEXT NOT NULL,
              body_blob_ref TEXT NOT NULL,
              evidence_refs_json TEXT NOT NULL,
              FOREIGN KEY(event_id) REFERENCES events(event_id)
            );
            CREATE TABLE IF NOT EXISTS agents (
              agent_id TEXT PRIMARY KEY,
              name TEXT NOT NULL UNIQUE,
              role TEXT NOT NULL,
              token_hash TEXT NOT NULL,
              created_at TEXT NOT NULL,
              status TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS dispatches (
              dispatch_id TEXT PRIMARY KEY,
              created_event_id TEXT NOT NULL,
              command_id TEXT NOT NULL UNIQUE,
              target_agent_id TEXT NOT NULL,
              title TEXT NOT NULL,
              body_hash TEXT NOT NULL,
              body_blob_ref TEXT NOT NULL,
              state TEXT NOT NULL,
              latest_fact_event_id TEXT,
              latest_report_status TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              FOREIGN KEY(created_event_id) REFERENCES events(event_id),
              FOREIGN KEY(target_agent_id) REFERENCES agents(agent_id)
            );
            CREATE TABLE IF NOT EXISTS dispatch_cancellations (
              command_id TEXT PRIMARY KEY,
              event_id TEXT NOT NULL UNIQUE,
              dispatch_id TEXT NOT NULL,
              reason_hash TEXT NOT NULL,
              FOREIGN KEY(event_id) REFERENCES events(event_id),
              FOREIGN KEY(dispatch_id) REFERENCES dispatches(dispatch_id)
            );
            CREATE TABLE IF NOT EXISTS agent_runs (
              run_id TEXT PRIMARY KEY,
              agent_id TEXT NOT NULL,
              token_hash TEXT NOT NULL,
              created_at TEXT NOT NULL,
              FOREIGN KEY(agent_id) REFERENCES agents(agent_id)
            );
            CREATE TABLE IF NOT EXISTS delegations (
              command_id TEXT PRIMARY KEY,
              source_agent_id TEXT NOT NULL,
              source_run_id TEXT,
              target_agent_id TEXT NOT NULL,
              worker_run_id TEXT NOT NULL,
              dispatch_id TEXT NOT NULL,
              runner TEXT NOT NULL,
              request_hash TEXT NOT NULL,
              state TEXT NOT NULL,
              child_executed INTEGER NOT NULL,
              created_at TEXT NOT NULL,
              completed_at TEXT,
              FOREIGN KEY(source_agent_id) REFERENCES agents(agent_id),
              FOREIGN KEY(target_agent_id) REFERENCES agents(agent_id),
              FOREIGN KEY(worker_run_id) REFERENCES agent_runs(run_id),
              FOREIGN KEY(dispatch_id) REFERENCES dispatches(dispatch_id)
            );
            "#,
        )?;
        Ok(())
    }

    pub fn init_work_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS work_nodes (
              work_node_id TEXT PRIMARY KEY,
              command_id TEXT NOT NULL UNIQUE,
              kind TEXT NOT NULL,
              title TEXT NOT NULL,
              body_hash TEXT,
              body_blob_ref TEXT,
              status_input TEXT NOT NULL,
              node_version INTEGER NOT NULL,
              accepted_event_id TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS work_edges (
              work_edge_id TEXT PRIMARY KEY,
              command_id TEXT NOT NULL UNIQUE,
              edge_type TEXT NOT NULL,
              from_node_id TEXT NOT NULL,
              to_node_id TEXT NOT NULL,
              graph_version INTEGER NOT NULL,
              created_at TEXT NOT NULL,
              UNIQUE(edge_type, from_node_id, to_node_id),
              FOREIGN KEY(from_node_id) REFERENCES work_nodes(work_node_id),
              FOREIGN KEY(to_node_id) REFERENCES work_nodes(work_node_id)
            );
            CREATE TABLE IF NOT EXISTS work_node_status_commands (
              command_id TEXT PRIMARY KEY,
              event_id TEXT NOT NULL UNIQUE,
              work_node_id TEXT NOT NULL,
              status_input TEXT NOT NULL,
              accepted_event_id TEXT,
              FOREIGN KEY(work_node_id) REFERENCES work_nodes(work_node_id)
            );
            CREATE TABLE IF NOT EXISTS work_dispatch_bindings (
              dispatch_id TEXT PRIMARY KEY,
              work_node_id TEXT NOT NULL,
              binding_kind TEXT NOT NULL,
              created_at TEXT NOT NULL,
              FOREIGN KEY(work_node_id) REFERENCES work_nodes(work_node_id),
              FOREIGN KEY(dispatch_id) REFERENCES dispatches(dispatch_id)
            );
            CREATE TABLE IF NOT EXISTS work_ref_bindings (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              fact_event_id TEXT NOT NULL,
              work_node_id TEXT NOT NULL,
              dispatch_id TEXT NOT NULL,
              snapshot_id TEXT,
              artifact_ref TEXT,
              workspace_ref TEXT,
              diff_ref TEXT,
              created_at TEXT NOT NULL,
              UNIQUE(fact_event_id, snapshot_id, artifact_ref, workspace_ref, diff_ref),
              FOREIGN KEY(work_node_id) REFERENCES work_nodes(work_node_id),
              FOREIGN KEY(dispatch_id) REFERENCES dispatches(dispatch_id),
              FOREIGN KEY(fact_event_id) REFERENCES facts(event_id)
            );
            CREATE TABLE IF NOT EXISTS work_root_bindings (
              root_work_node_id TEXT NOT NULL,
              work_node_id TEXT NOT NULL PRIMARY KEY,
              created_by_agent_id TEXT,
              created_by_run_id TEXT,
              created_at TEXT NOT NULL,
              FOREIGN KEY(root_work_node_id) REFERENCES work_nodes(work_node_id),
              FOREIGN KEY(work_node_id) REFERENCES work_nodes(work_node_id)
            );
            CREATE INDEX IF NOT EXISTS idx_work_root_bindings_root
              ON work_root_bindings(root_work_node_id);
            CREATE TABLE IF NOT EXISTS work_notes (
              note_id TEXT PRIMARY KEY,
              command_id TEXT NOT NULL UNIQUE,
              event_id TEXT NOT NULL UNIQUE,
              work_node_id TEXT NOT NULL,
              note_kind TEXT NOT NULL,
              body_hash TEXT NOT NULL,
              body_blob_ref TEXT NOT NULL,
              actor_agent_id TEXT NOT NULL,
              actor_run_id TEXT,
              created_at TEXT NOT NULL,
              FOREIGN KEY(event_id) REFERENCES events(event_id),
              FOREIGN KEY(work_node_id) REFERENCES work_nodes(work_node_id),
              FOREIGN KEY(actor_agent_id) REFERENCES agents(agent_id)
            );
            CREATE INDEX IF NOT EXISTS idx_work_notes_node
              ON work_notes(work_node_id, created_at);
            CREATE TABLE IF NOT EXISTS scheduler_runs (
              scheduler_run_id TEXT PRIMARY KEY,
              command_id TEXT NOT NULL UNIQUE,
              root_work_node_id TEXT NOT NULL,
              runner TEXT NOT NULL,
              max_parallel INTEGER NOT NULL,
              acceptance_mode TEXT NOT NULL,
              request_hash TEXT NOT NULL,
              state TEXT NOT NULL,
              created_at TEXT NOT NULL,
              completed_at TEXT,
              FOREIGN KEY(root_work_node_id) REFERENCES work_nodes(work_node_id)
            );
            CREATE TABLE IF NOT EXISTS scheduler_node_runs (
              node_run_id TEXT PRIMARY KEY,
              scheduler_run_id TEXT NOT NULL,
              work_node_id TEXT NOT NULL,
              dispatch_id TEXT,
              worker_agent_id TEXT NOT NULL,
              worker_run_id TEXT,
              state TEXT NOT NULL,
              started_at TEXT NOT NULL,
              completed_at TEXT,
              FOREIGN KEY(scheduler_run_id) REFERENCES scheduler_runs(scheduler_run_id),
              FOREIGN KEY(work_node_id) REFERENCES work_nodes(work_node_id),
              FOREIGN KEY(dispatch_id) REFERENCES dispatches(dispatch_id),
              FOREIGN KEY(worker_agent_id) REFERENCES agents(agent_id)
            );
            CREATE INDEX IF NOT EXISTS idx_scheduler_node_runs_work_state
              ON scheduler_node_runs(work_node_id, state);
            CREATE INDEX IF NOT EXISTS idx_scheduler_node_runs_scheduler
              ON scheduler_node_runs(scheduler_run_id);
            CREATE TABLE IF NOT EXISTS branch_workspaces (
              branch_id TEXT PRIMARY KEY,
              backend TEXT NOT NULL,
              root_work_node_id TEXT NOT NULL,
              work_node_id TEXT NOT NULL,
              dispatch_id TEXT NOT NULL,
              run_id TEXT NOT NULL,
              branch_name TEXT NOT NULL,
              branch_path TEXT NOT NULL,
              branch_ref TEXT NOT NULL UNIQUE,
              state TEXT NOT NULL,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              FOREIGN KEY(root_work_node_id) REFERENCES work_nodes(work_node_id),
              FOREIGN KEY(work_node_id) REFERENCES work_nodes(work_node_id),
              FOREIGN KEY(dispatch_id) REFERENCES dispatches(dispatch_id),
              FOREIGN KEY(run_id) REFERENCES agent_runs(run_id)
            );
            CREATE INDEX IF NOT EXISTS idx_branch_workspaces_work
              ON branch_workspaces(work_node_id, state);
            CREATE TABLE IF NOT EXISTS branch_integrations (
              integration_id TEXT PRIMARY KEY,
              branch_id TEXT NOT NULL UNIQUE,
              work_node_id TEXT NOT NULL,
              dispatch_id TEXT NOT NULL,
              fact_event_id TEXT,
              branch_ref TEXT NOT NULL,
              diff_ref TEXT,
              state TEXT NOT NULL,
              commit_ref TEXT,
              rejection_reason_hash TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              FOREIGN KEY(branch_id) REFERENCES branch_workspaces(branch_id),
              FOREIGN KEY(work_node_id) REFERENCES work_nodes(work_node_id),
              FOREIGN KEY(dispatch_id) REFERENCES dispatches(dispatch_id),
              FOREIGN KEY(fact_event_id) REFERENCES facts(event_id)
            );
            CREATE INDEX IF NOT EXISTS idx_branch_integrations_work
              ON branch_integrations(work_node_id, state);
            CREATE TABLE IF NOT EXISTS branch_commands (
              command_id TEXT PRIMARY KEY,
              integration_id TEXT NOT NULL,
              action TEXT NOT NULL,
              request_hash TEXT NOT NULL,
              created_at TEXT NOT NULL,
              FOREIGN KEY(integration_id) REFERENCES branch_integrations(integration_id)
            );
            CREATE TABLE IF NOT EXISTS workflow_templates (
              template_id TEXT PRIMARY KEY,
              latest_version INTEGER NOT NULL,
              latest_hash TEXT NOT NULL,
              title TEXT NOT NULL,
              source_ref TEXT NOT NULL,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS workflow_template_versions (
              template_id TEXT NOT NULL,
              version INTEGER NOT NULL,
              template_hash TEXT NOT NULL,
              source_ref TEXT NOT NULL,
              spec_json TEXT NOT NULL,
              created_at TEXT NOT NULL,
              PRIMARY KEY(template_id, version),
              UNIQUE(template_id, template_hash),
              FOREIGN KEY(template_id) REFERENCES workflow_templates(template_id)
            );
            CREATE TABLE IF NOT EXISTS workflow_import_commands (
              command_id TEXT PRIMARY KEY,
              template_id TEXT NOT NULL,
              version INTEGER NOT NULL,
              template_hash TEXT NOT NULL,
              request_hash TEXT NOT NULL,
              created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS workflow_runs (
              workflow_run_id TEXT PRIMARY KEY,
              command_id TEXT NOT NULL UNIQUE,
              template_id TEXT NOT NULL,
              template_version INTEGER NOT NULL,
              template_hash TEXT NOT NULL,
              params_json TEXT NOT NULL,
              params_hash TEXT NOT NULL,
              request_hash TEXT NOT NULL,
              root_work_node_id TEXT NOT NULL,
              scheduler_run_id TEXT,
              state TEXT NOT NULL,
              created_at TEXT NOT NULL,
              completed_at TEXT,
              FOREIGN KEY(root_work_node_id) REFERENCES work_nodes(work_node_id)
            );
            CREATE TABLE IF NOT EXISTS workflow_run_nodes (
              workflow_run_id TEXT NOT NULL,
              node_template_id TEXT NOT NULL,
              work_node_id TEXT NOT NULL,
              output_contract_json TEXT NOT NULL,
              capability_policy_json TEXT NOT NULL,
              PRIMARY KEY(workflow_run_id, node_template_id),
              FOREIGN KEY(workflow_run_id) REFERENCES workflow_runs(workflow_run_id),
              FOREIGN KEY(work_node_id) REFERENCES work_nodes(work_node_id)
            );
            "#,
        )?;
        Ok(())
    }

    pub fn has_work_schema(&self) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'work_dispatch_bindings'",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn insert_event(&self, event: &EventRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events (event_id, event_type, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                event.event_id,
                event.event_type,
                event.created_at.to_rfc3339(),
                serde_json::to_string(&event.payload)?,
            ],
        )?;
        Ok(())
    }

    pub fn insert_snapshot(&self, snapshot: &SnapshotRecord) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO snapshots (
              snapshot_id, event_id, created_at, backend, capture_root, label,
              agent_id, dispatch_id, manifest_path, manifest_hash, file_count, total_bytes
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            "#,
            params![
                snapshot.snapshot_id,
                snapshot.event_id,
                snapshot.created_at.to_rfc3339(),
                snapshot.backend,
                snapshot.capture_root,
                snapshot.label,
                snapshot.agent_id,
                snapshot.dispatch_id,
                snapshot.manifest_path,
                snapshot.manifest_hash,
                snapshot.file_count,
                snapshot.total_bytes,
            ],
        )?;
        Ok(())
    }

    pub fn list_snapshots(&self) -> Result<Vec<SnapshotRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT snapshot_id, event_id, created_at, backend, capture_root, label,
                   agent_id, dispatch_id, manifest_path, manifest_hash, file_count, total_bytes
            FROM snapshots
            ORDER BY created_at DESC
            "#,
        )?;
        let rows = stmt.query_map([], row_to_snapshot)?;
        let mut snapshots = Vec::new();
        for row in rows {
            snapshots.push(row?);
        }
        Ok(snapshots)
    }

    pub fn get_snapshot(&self, snapshot_id: &str) -> Result<Option<SnapshotRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT snapshot_id, event_id, created_at, backend, capture_root, label,
                   agent_id, dispatch_id, manifest_path, manifest_hash, file_count, total_bytes
            FROM snapshots
            WHERE snapshot_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![snapshot_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_snapshot(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_events_by_type(&self, event_type: &str) -> Result<Vec<EventRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, event_type, created_at, payload_json FROM events WHERE event_type = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![event_type], row_to_event)?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    pub fn get_event_by_id(&self, event_id: &str) -> Result<Option<EventRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, event_type, created_at, payload_json FROM events WHERE event_id = ?1",
        )?;
        let mut rows = stmt.query(params![event_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_event(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn insert_fact_idempotent(
        &self,
        input: &InsertFactInput,
    ) -> Result<IdempotencyResolution<FactRecord>> {
        if let Some(existing) = self.get_fact_by_command_id(&input.command_id)? {
            if existing.actor_agent_id == input.actor_agent_id
                && existing.workspace_id == input.workspace_id
                && existing.actor_run_id == input.actor_run_id
                && existing.fact_type == input.fact_type
                && existing.body_hash == input.body_hash
                && existing.evidence_refs == input.evidence_refs
            {
                return Ok(IdempotencyResolution::Replayed(existing));
            }
            return Ok(IdempotencyResolution::Conflict(existing));
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO events (event_id, event_type, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                input.event.event_id,
                input.event.event_type,
                input.event.created_at.to_rfc3339(),
                serde_json::to_string(&input.event.payload)?,
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO facts (
              event_id, command_id, created_at, workspace_id, actor_agent_id,
              actor_run_id, fact_type, body_hash, body_blob_ref, evidence_refs_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                input.event.event_id,
                input.command_id,
                input.event.created_at.to_rfc3339(),
                input.workspace_id,
                input.actor_agent_id,
                input.actor_run_id,
                input.fact_type,
                input.body_hash,
                input.body_blob_ref,
                serde_json::to_string(&input.evidence_refs)?,
            ],
        )?;
        tx.commit()?;

        Ok(IdempotencyResolution::Inserted(FactRecord {
            event_id: input.event.event_id.clone(),
            command_id: input.command_id.clone(),
            created_at: input.event.created_at,
            workspace_id: input.workspace_id.clone(),
            actor_agent_id: input.actor_agent_id.clone(),
            actor_run_id: input.actor_run_id.clone(),
            fact_type: input.fact_type.clone(),
            body_hash: input.body_hash.clone(),
            body_blob_ref: input.body_blob_ref.clone(),
            evidence_refs: input.evidence_refs.clone(),
        }))
    }

    pub fn list_facts(&self) -> Result<Vec<FactRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT event_id, command_id, created_at, workspace_id, actor_agent_id,
                   actor_run_id, fact_type, body_hash, body_blob_ref, evidence_refs_json
            FROM facts
            ORDER BY created_at DESC
            "#,
        )?;
        let rows = stmt.query_map([], row_to_fact)?;
        let mut facts = Vec::new();
        for row in rows {
            facts.push(row?);
        }
        Ok(facts)
    }

    pub fn get_fact_by_event_id(&self, event_id: &str) -> Result<Option<FactRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT event_id, command_id, created_at, workspace_id, actor_agent_id,
                   actor_run_id, fact_type, body_hash, body_blob_ref, evidence_refs_json
            FROM facts
            WHERE event_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![event_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_fact(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_fact_by_command_id(&self, command_id: &str) -> Result<Option<FactRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT event_id, command_id, created_at, workspace_id, actor_agent_id,
                   actor_run_id, fact_type, body_hash, body_blob_ref, evidence_refs_json
            FROM facts
            WHERE command_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![command_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_fact(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn insert_agent(&self, agent: &AgentRecord) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO agents (agent_id, name, role, token_hash, created_at, status)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                agent.agent_id,
                agent.name,
                agent.role.as_str(),
                agent.token_hash,
                agent.created_at.to_rfc3339(),
                agent.status,
            ],
        )?;
        Ok(())
    }

    pub fn insert_agent_with_event(&self, event: &EventRecord, agent: &AgentRecord) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO events (event_id, event_type, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                event.event_id,
                event.event_type,
                event.created_at.to_rfc3339(),
                serde_json::to_string(&event.payload)?,
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO agents (agent_id, name, role, token_hash, created_at, status)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                agent.agent_id,
                agent.name,
                agent.role.as_str(),
                agent.token_hash,
                agent.created_at.to_rfc3339(),
                agent.status,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_agents(&self) -> Result<Vec<AgentRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT agent_id, name, role, token_hash, created_at, status FROM agents ORDER BY name",
        )?;
        let rows = stmt.query_map([], row_to_agent)?;
        let mut agents = Vec::new();
        for row in rows {
            agents.push(row?);
        }
        Ok(agents)
    }

    pub fn get_agent(&self, name_or_id: &str) -> Result<Option<AgentRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT agent_id, name, role, token_hash, created_at, status
            FROM agents
            WHERE agent_id = ?1 OR name = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![name_or_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_agent(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn insert_agent_run(&self, input: &InsertAgentRunInput) -> Result<AgentRunRecord> {
        self.conn.execute(
            r#"
            INSERT INTO agent_runs (run_id, agent_id, token_hash, created_at)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                input.run_id,
                input.agent_id,
                input.token_hash,
                input.created_at.to_rfc3339(),
            ],
        )?;
        Ok(AgentRunRecord {
            run_id: input.run_id.clone(),
            agent_id: input.agent_id.clone(),
            token_hash: input.token_hash.clone(),
            created_at: input.created_at,
        })
    }

    pub fn get_agent_run(&self, run_id: &str) -> Result<Option<AgentRunRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT run_id, agent_id, token_hash, created_at
            FROM agent_runs
            WHERE run_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![run_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_agent_run(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn insert_dispatch_idempotent(
        &self,
        input: &InsertDispatchInput,
    ) -> Result<IdempotencyResolution<DispatchRecord>> {
        if let Some(existing) = self.get_dispatch_by_command_id(&input.command_id)? {
            if existing.target_agent_id == input.target_agent_id
                && existing.title == input.title
                && existing.body_hash == input.body_hash
            {
                return Ok(IdempotencyResolution::Replayed(existing));
            }
            return Ok(IdempotencyResolution::Conflict(existing));
        }

        let now = input.event.created_at;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO events (event_id, event_type, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                input.event.event_id,
                input.event.event_type,
                input.event.created_at.to_rfc3339(),
                serde_json::to_string(&input.event.payload)?,
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO dispatches (
              dispatch_id, created_event_id, command_id, target_agent_id, title,
              body_hash, body_blob_ref, state, latest_fact_event_id,
              latest_report_status, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, ?9, ?10)
            "#,
            params![
                input.dispatch_id,
                input.event.event_id,
                input.command_id,
                input.target_agent_id,
                input.title,
                input.body_hash,
                input.body_blob_ref,
                DispatchState::Open.as_str(),
                now.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )?;
        tx.commit()?;

        Ok(IdempotencyResolution::Inserted(DispatchRecord {
            dispatch_id: input.dispatch_id.clone(),
            created_event_id: input.event.event_id.clone(),
            command_id: input.command_id.clone(),
            target_agent_id: input.target_agent_id.clone(),
            title: input.title.clone(),
            body_hash: input.body_hash.clone(),
            body_blob_ref: input.body_blob_ref.clone(),
            state: DispatchState::Open,
            latest_fact_event_id: None,
            latest_report_status: None,
            created_at: now,
            updated_at: now,
        }))
    }

    pub fn transition_dispatch_with_fact_idempotent(
        &self,
        input: &DispatchTransitionInput,
    ) -> Result<IdempotencyResolution<(FactRecord, DispatchRecord)>> {
        if let Some(existing_fact) = self.get_fact_by_command_id(&input.fact.command_id)? {
            let dispatch = self
                .get_dispatch(&input.dispatch_id)?
                .ok_or_else(|| anyhow::anyhow!("dispatch not found: {}", input.dispatch_id))?;
            if existing_fact.actor_agent_id == input.fact.actor_agent_id
                && existing_fact.workspace_id == input.fact.workspace_id
                && existing_fact.actor_run_id == input.fact.actor_run_id
                && existing_fact.fact_type == input.fact.fact_type
                && existing_fact.body_hash == input.fact.body_hash
                && existing_fact.evidence_refs == input.fact.evidence_refs
                && dispatch.latest_fact_event_id.as_deref() == Some(existing_fact.event_id.as_str())
            {
                return Ok(IdempotencyResolution::Replayed((existing_fact, dispatch)));
            }
            return Ok(IdempotencyResolution::Conflict((existing_fact, dispatch)));
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO events (event_id, event_type, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                input.fact.event.event_id,
                input.fact.event.event_type,
                input.fact.event.created_at.to_rfc3339(),
                serde_json::to_string(&input.fact.event.payload)?,
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO facts (
              event_id, command_id, created_at, workspace_id, actor_agent_id,
              actor_run_id, fact_type, body_hash, body_blob_ref, evidence_refs_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                input.fact.event.event_id,
                input.fact.command_id,
                input.fact.event.created_at.to_rfc3339(),
                input.fact.workspace_id,
                input.fact.actor_agent_id,
                input.fact.actor_run_id,
                input.fact.fact_type,
                input.fact.body_hash,
                input.fact.body_blob_ref,
                serde_json::to_string(&input.fact.evidence_refs)?,
            ],
        )?;
        tx.execute(
            r#"
            UPDATE dispatches
            SET state = ?1,
                latest_fact_event_id = ?2,
                latest_report_status = ?3,
                updated_at = ?4
            WHERE dispatch_id = ?5
            "#,
            params![
                input.next_state.as_str(),
                input.fact.event.event_id,
                input.latest_report_status,
                input.fact.event.created_at.to_rfc3339(),
                input.dispatch_id,
            ],
        )?;
        tx.commit()?;

        let fact = FactRecord {
            event_id: input.fact.event.event_id.clone(),
            command_id: input.fact.command_id.clone(),
            created_at: input.fact.event.created_at,
            workspace_id: input.fact.workspace_id.clone(),
            actor_agent_id: input.fact.actor_agent_id.clone(),
            actor_run_id: input.fact.actor_run_id.clone(),
            fact_type: input.fact.fact_type.clone(),
            body_hash: input.fact.body_hash.clone(),
            body_blob_ref: input.fact.body_blob_ref.clone(),
            evidence_refs: input.fact.evidence_refs.clone(),
        };
        let dispatch = self
            .get_dispatch(&input.dispatch_id)?
            .ok_or_else(|| anyhow::anyhow!("dispatch not found: {}", input.dispatch_id))?;
        Ok(IdempotencyResolution::Inserted((fact, dispatch)))
    }

    pub fn list_dispatches(&self) -> Result<Vec<DispatchRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT dispatch_id, created_event_id, command_id, target_agent_id, title,
                   body_hash, body_blob_ref, state, latest_fact_event_id,
                   latest_report_status, created_at, updated_at
            FROM dispatches
            ORDER BY created_at DESC
            "#,
        )?;
        let rows = stmt.query_map([], row_to_dispatch)?;
        let mut dispatches = Vec::new();
        for row in rows {
            dispatches.push(row?);
        }
        Ok(dispatches)
    }

    pub fn list_dispatches_for_agent(&self, agent_id: &str) -> Result<Vec<DispatchRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT dispatch_id, created_event_id, command_id, target_agent_id, title,
                   body_hash, body_blob_ref, state, latest_fact_event_id,
                   latest_report_status, created_at, updated_at
            FROM dispatches
            WHERE target_agent_id = ?1
            ORDER BY created_at DESC
            "#,
        )?;
        let rows = stmt.query_map(params![agent_id], row_to_dispatch)?;
        let mut dispatches = Vec::new();
        for row in rows {
            dispatches.push(row?);
        }
        Ok(dispatches)
    }

    pub fn get_dispatch(&self, dispatch_id: &str) -> Result<Option<DispatchRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT dispatch_id, created_event_id, command_id, target_agent_id, title,
                   body_hash, body_blob_ref, state, latest_fact_event_id,
                   latest_report_status, created_at, updated_at
            FROM dispatches
            WHERE dispatch_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![dispatch_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_dispatch(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_dispatch_by_command_id(&self, command_id: &str) -> Result<Option<DispatchRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT dispatch_id, created_event_id, command_id, target_agent_id, title,
                   body_hash, body_blob_ref, state, latest_fact_event_id,
                   latest_report_status, created_at, updated_at
            FROM dispatches
            WHERE command_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![command_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_dispatch(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn cancel_dispatch(&self, dispatch_id: &str, updated_at: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            r#"
            UPDATE dispatches
            SET state = ?1, updated_at = ?2
            WHERE dispatch_id = ?3
            "#,
            params![
                DispatchState::Cancelled.as_str(),
                updated_at.to_rfc3339(),
                dispatch_id,
            ],
        )?;
        Ok(())
    }

    pub fn cancel_dispatch_idempotent(
        &self,
        input: &CancelDispatchInput,
    ) -> Result<IdempotencyResolution<DispatchRecord>> {
        if let Some(existing) = self.get_dispatch_cancellation_by_command_id(&input.command_id)? {
            let (_event_id, dispatch_id, reason_hash) = existing;
            let dispatch = self
                .get_dispatch(&dispatch_id)?
                .ok_or_else(|| anyhow::anyhow!("dispatch not found: {dispatch_id}"))?;
            if dispatch_id == input.dispatch_id && reason_hash == input.reason_hash {
                return Ok(IdempotencyResolution::Replayed(dispatch));
            }
            return Ok(IdempotencyResolution::Conflict(dispatch));
        }

        let dispatch = self
            .get_dispatch(&input.dispatch_id)?
            .ok_or_else(|| anyhow::anyhow!("dispatch not found: {}", input.dispatch_id))?;
        if !matches!(dispatch.state, DispatchState::Open | DispatchState::Blocked) {
            return Err(anyhow::anyhow!(
                "dispatch closed: {} is {}",
                input.dispatch_id,
                dispatch.state.as_str()
            ));
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO events (event_id, event_type, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                input.event.event_id,
                input.event.event_type,
                input.event.created_at.to_rfc3339(),
                serde_json::to_string(&input.event.payload)?,
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO dispatch_cancellations (command_id, event_id, dispatch_id, reason_hash)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                input.command_id,
                input.event.event_id,
                input.dispatch_id,
                input.reason_hash,
            ],
        )?;
        tx.execute(
            r#"
            UPDATE dispatches
            SET state = ?1,
                updated_at = ?2
            WHERE dispatch_id = ?3
            "#,
            params![
                DispatchState::Cancelled.as_str(),
                input.event.created_at.to_rfc3339(),
                input.dispatch_id,
            ],
        )?;
        tx.commit()?;

        let dispatch = self
            .get_dispatch(&input.dispatch_id)?
            .ok_or_else(|| anyhow::anyhow!("dispatch not found: {}", input.dispatch_id))?;
        Ok(IdempotencyResolution::Inserted(dispatch))
    }

    fn get_dispatch_cancellation_by_command_id(
        &self,
        command_id: &str,
    ) -> Result<Option<(String, String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT event_id, dispatch_id, reason_hash
            FROM dispatch_cancellations
            WHERE command_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![command_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?, row.get(2)?)))
        } else {
            Ok(None)
        }
    }

    pub fn insert_delegation_idempotent(
        &self,
        input: &InsertDelegationInput,
    ) -> Result<IdempotencyResolution<DelegationRecord>> {
        if let Some(existing) = self.get_delegation_by_command_id(&input.command_id)? {
            if existing.source_agent_id == input.source_agent_id
                && existing.source_run_id == input.source_run_id
                && existing.target_agent_id == input.target_agent_id
                && existing.worker_run_id == input.worker_run_id
                && existing.dispatch_id == input.dispatch_id
                && existing.runner == input.runner
                && existing.request_hash == input.request_hash
            {
                return Ok(IdempotencyResolution::Replayed(existing));
            }
            return Ok(IdempotencyResolution::Conflict(existing));
        }

        self.conn.execute(
            r#"
            INSERT INTO delegations (
              command_id, source_agent_id, source_run_id, target_agent_id,
              worker_run_id, dispatch_id, runner, request_hash, state,
              child_executed, created_at, completed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'created', 0, ?9, NULL)
            "#,
            params![
                input.command_id,
                input.source_agent_id,
                input.source_run_id,
                input.target_agent_id,
                input.worker_run_id,
                input.dispatch_id,
                input.runner,
                input.request_hash,
                input.created_at.to_rfc3339(),
            ],
        )?;
        Ok(IdempotencyResolution::Inserted(
            self.get_delegation_by_command_id(&input.command_id)?
                .expect("inserted delegation should be readable"),
        ))
    }

    pub fn complete_delegation(&self, input: &CompleteDelegationInput) -> Result<DelegationRecord> {
        self.conn.execute(
            r#"
            UPDATE delegations
            SET state = ?1,
                child_executed = ?2,
                completed_at = ?3
            WHERE command_id = ?4
            "#,
            params![
                input.state,
                if input.child_executed { 1_i64 } else { 0_i64 },
                input.completed_at.to_rfc3339(),
                input.command_id,
            ],
        )?;
        self.get_delegation_by_command_id(&input.command_id)?
            .ok_or_else(|| anyhow::anyhow!("delegation not found: {}", input.command_id))
    }

    pub fn get_delegation_by_command_id(
        &self,
        command_id: &str,
    ) -> Result<Option<DelegationRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT command_id, source_agent_id, source_run_id, target_agent_id,
                   worker_run_id, dispatch_id, runner, request_hash, state,
                   child_executed, created_at, completed_at
            FROM delegations
            WHERE command_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![command_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_delegation(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn insert_work_node_idempotent(
        &self,
        input: &InsertWorkNodeInput,
    ) -> Result<IdempotencyResolution<WorkNodeRecord>> {
        if let Some(existing) = self.get_work_node_by_command_id(&input.command_id)? {
            if existing.kind == input.kind
                && existing.title == input.title
                && existing.body_hash == input.body_hash
            {
                return Ok(IdempotencyResolution::Replayed(existing));
            }
            return Ok(IdempotencyResolution::Conflict(existing));
        }

        let now = input.event.created_at;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO events (event_id, event_type, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                input.event.event_id,
                input.event.event_type,
                input.event.created_at.to_rfc3339(),
                serde_json::to_string(&input.event.payload)?,
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO work_nodes (
              work_node_id, command_id, kind, title, body_hash, body_blob_ref,
              status_input, node_version, accepted_event_id, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 1, NULL, ?7, ?8)
            "#,
            params![
                input.work_node_id,
                input.command_id,
                input.kind,
                input.title,
                input.body_hash,
                input.body_blob_ref,
                now.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )?;
        tx.commit()?;
        Ok(IdempotencyResolution::Inserted(
            self.get_work_node(&input.work_node_id)?
                .expect("inserted work node should be readable"),
        ))
    }

    pub fn get_work_node(&self, work_node_id: &str) -> Result<Option<WorkNodeRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT work_node_id, command_id, kind, title, body_hash, body_blob_ref,
                   status_input, node_version, accepted_event_id, created_at, updated_at
            FROM work_nodes
            WHERE work_node_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![work_node_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_work_node(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_work_node_by_command_id(&self, command_id: &str) -> Result<Option<WorkNodeRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT work_node_id, command_id, kind, title, body_hash, body_blob_ref,
                   status_input, node_version, accepted_event_id, created_at, updated_at
            FROM work_nodes
            WHERE command_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![command_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_work_node(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_work_nodes(&self) -> Result<Vec<WorkNodeRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT work_node_id, command_id, kind, title, body_hash, body_blob_ref,
                   status_input, node_version, accepted_event_id, created_at, updated_at
            FROM work_nodes
            ORDER BY created_at ASC
            "#,
        )?;
        let rows = stmt.query_map([], row_to_work_node)?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    pub fn insert_work_edge_idempotent(
        &self,
        input: &InsertWorkEdgeInput,
    ) -> Result<IdempotencyResolution<WorkEdgeRecord>> {
        if let Some(existing) = self.get_work_edge_by_command_id(&input.command_id)? {
            if existing.edge_type == input.edge_type
                && existing.from_node_id == input.from_node_id
                && existing.to_node_id == input.to_node_id
            {
                return Ok(IdempotencyResolution::Replayed(existing));
            }
            return Ok(IdempotencyResolution::Conflict(existing));
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO events (event_id, event_type, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                input.event.event_id,
                input.event.event_type,
                input.event.created_at.to_rfc3339(),
                serde_json::to_string(&input.event.payload)?,
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO work_edges (
              work_edge_id, command_id, edge_type, from_node_id, to_node_id,
              graph_version, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                input.work_edge_id,
                input.command_id,
                input.edge_type,
                input.from_node_id,
                input.to_node_id,
                input.graph_version,
                input.event.created_at.to_rfc3339(),
            ],
        )?;
        tx.commit()?;
        Ok(IdempotencyResolution::Inserted(
            self.get_work_edge_by_command_id(&input.command_id)?
                .expect("inserted work edge should be readable"),
        ))
    }

    pub fn get_work_edge_by_command_id(&self, command_id: &str) -> Result<Option<WorkEdgeRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT work_edge_id, command_id, edge_type, from_node_id, to_node_id,
                   graph_version, created_at
            FROM work_edges
            WHERE command_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![command_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_work_edge(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_work_edges(&self) -> Result<Vec<WorkEdgeRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT work_edge_id, command_id, edge_type, from_node_id, to_node_id,
                   graph_version, created_at
            FROM work_edges
            ORDER BY created_at ASC
            "#,
        )?;
        let rows = stmt.query_map([], row_to_work_edge)?;
        let mut edges = Vec::new();
        for row in rows {
            edges.push(row?);
        }
        Ok(edges)
    }

    pub fn next_work_graph_version(&self) -> Result<i64> {
        let max_version: Option<i64> =
            self.conn
                .query_row("SELECT MAX(graph_version) FROM work_edges", [], |row| {
                    row.get(0)
                })?;
        Ok(max_version.unwrap_or(0) + 1)
    }

    pub fn update_work_node_status_idempotent(
        &self,
        input: &UpdateWorkNodeStatusInput,
    ) -> Result<IdempotencyResolution<WorkNodeRecord>> {
        if let Some(existing) = self.get_work_node_status_command(&input.command_id)? {
            let (work_node_id, status_input, accepted_event_id) = existing;
            let node = self
                .get_work_node(&work_node_id)?
                .ok_or_else(|| anyhow::anyhow!("work node not found: {work_node_id}"))?;
            if work_node_id == input.work_node_id
                && status_input == input.status_input
                && accepted_event_id.is_some() == input.accepted_event_id.is_some()
            {
                return Ok(IdempotencyResolution::Replayed(node));
            }
            return Ok(IdempotencyResolution::Conflict(node));
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO events (event_id, event_type, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                input.event.event_id,
                input.event.event_type,
                input.event.created_at.to_rfc3339(),
                serde_json::to_string(&input.event.payload)?,
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO work_node_status_commands (
              command_id, event_id, work_node_id, status_input, accepted_event_id
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                input.command_id,
                input.event.event_id,
                input.work_node_id,
                input.status_input,
                input.accepted_event_id,
            ],
        )?;
        tx.execute(
            r#"
            UPDATE work_nodes
            SET status_input = ?1,
                accepted_event_id = ?2,
                node_version = node_version + 1,
                updated_at = ?3
            WHERE work_node_id = ?4
            "#,
            params![
                input.status_input,
                input.accepted_event_id,
                input.event.created_at.to_rfc3339(),
                input.work_node_id,
            ],
        )?;
        tx.commit()?;
        Ok(IdempotencyResolution::Inserted(
            self.get_work_node(&input.work_node_id)?
                .ok_or_else(|| anyhow::anyhow!("work node not found: {}", input.work_node_id))?,
        ))
    }

    pub fn get_work_node_status_command(
        &self,
        command_id: &str,
    ) -> Result<Option<(String, String, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT work_node_id, status_input, accepted_event_id
            FROM work_node_status_commands
            WHERE command_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![command_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?, row.get(2)?)))
        } else {
            Ok(None)
        }
    }

    pub fn bind_work_dispatch(&self, input: &BindWorkDispatchInput) -> Result<()> {
        if let Some(existing) = self.get_work_dispatch_binding(&input.dispatch_id)? {
            if existing.work_node_id == input.work_node_id
                && existing.binding_kind == input.binding_kind
            {
                return Ok(());
            }
            anyhow::bail!("dispatch already bound to work node: {}", input.dispatch_id);
        }
        self.conn.execute(
            r#"
            INSERT OR IGNORE INTO work_dispatch_bindings (
              dispatch_id, work_node_id, binding_kind, created_at
            ) VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                input.dispatch_id,
                input.work_node_id,
                input.binding_kind,
                input.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_work_dispatch_binding(
        &self,
        dispatch_id: &str,
    ) -> Result<Option<WorkDispatchBindingRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT work_node_id, dispatch_id, binding_kind, created_at
            FROM work_dispatch_bindings
            WHERE dispatch_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![dispatch_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_work_dispatch_binding(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_work_dispatch_bindings(&self) -> Result<Vec<WorkDispatchBindingRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT work_node_id, dispatch_id, binding_kind, created_at
            FROM work_dispatch_bindings
            ORDER BY created_at ASC
            "#,
        )?;
        let rows = stmt.query_map([], row_to_work_dispatch_binding)?;
        let mut bindings = Vec::new();
        for row in rows {
            bindings.push(row?);
        }
        Ok(bindings)
    }

    pub fn insert_work_ref_binding_idempotent(
        &self,
        input: &InsertWorkRefBindingInput,
    ) -> Result<()> {
        let existing: i64 = self.conn.query_row(
            r#"
            SELECT COUNT(*)
            FROM work_ref_bindings
            WHERE fact_event_id = ?1
              AND dispatch_id = ?2
              AND ((snapshot_id = ?3) OR (snapshot_id IS NULL AND ?3 IS NULL))
              AND ((artifact_ref = ?4) OR (artifact_ref IS NULL AND ?4 IS NULL))
              AND ((workspace_ref = ?5) OR (workspace_ref IS NULL AND ?5 IS NULL))
              AND ((diff_ref = ?6) OR (diff_ref IS NULL AND ?6 IS NULL))
            "#,
            params![
                input.fact_event_id,
                input.dispatch_id,
                input.snapshot_id,
                input.artifact_ref,
                input.workspace_ref,
                input.diff_ref,
            ],
            |row| row.get(0),
        )?;
        if existing > 0 {
            return Ok(());
        }
        self.conn.execute(
            r#"
            INSERT OR IGNORE INTO work_ref_bindings (
              fact_event_id, work_node_id, dispatch_id, snapshot_id,
              artifact_ref, workspace_ref, diff_ref, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                input.fact_event_id,
                input.work_node_id,
                input.dispatch_id,
                input.snapshot_id,
                input.artifact_ref,
                input.workspace_ref,
                input.diff_ref,
                input.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_work_ref_bindings(&self) -> Result<Vec<WorkRefBindingRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT work_node_id, dispatch_id, fact_event_id, snapshot_id,
                   artifact_ref, workspace_ref, diff_ref, created_at
            FROM work_ref_bindings
            ORDER BY created_at ASC
            "#,
        )?;
        let rows = stmt.query_map([], row_to_work_ref_binding)?;
        let mut bindings = Vec::new();
        for row in rows {
            bindings.push(row?);
        }
        Ok(bindings)
    }

    pub fn bind_work_root(&self, input: &BindWorkRootInput) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR IGNORE INTO work_root_bindings (
              root_work_node_id, work_node_id, created_by_agent_id, created_by_run_id, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                input.root_work_node_id,
                input.work_node_id,
                input.created_by_agent_id,
                input.created_by_run_id,
                input.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_work_root_bindings(&self) -> Result<Vec<WorkRootBindingRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT root_work_node_id, work_node_id, created_by_agent_id, created_by_run_id, created_at
            FROM work_root_bindings
            ORDER BY created_at ASC
            "#,
        )?;
        let rows = stmt.query_map([], row_to_work_root_binding)?;
        let mut bindings = Vec::new();
        for row in rows {
            bindings.push(row?);
        }
        Ok(bindings)
    }

    pub fn list_work_root_bindings_for_root(
        &self,
        root_work_node_id: &str,
    ) -> Result<Vec<WorkRootBindingRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT root_work_node_id, work_node_id, created_by_agent_id, created_by_run_id, created_at
            FROM work_root_bindings
            WHERE root_work_node_id = ?1
            ORDER BY created_at ASC
            "#,
        )?;
        let rows = stmt.query_map(params![root_work_node_id], row_to_work_root_binding)?;
        let mut bindings = Vec::new();
        for row in rows {
            bindings.push(row?);
        }
        Ok(bindings)
    }

    pub fn insert_work_note_idempotent(
        &self,
        input: &InsertWorkNoteInput,
    ) -> Result<IdempotencyResolution<WorkNoteRecord>> {
        if let Some(existing) = self.get_work_note_by_command_id(&input.command_id)? {
            if existing.work_node_id == input.work_node_id
                && existing.note_kind == input.note_kind
                && existing.body_hash == input.body_hash
                && existing.actor_agent_id == input.actor_agent_id
                && existing.actor_run_id == input.actor_run_id
            {
                return Ok(IdempotencyResolution::Replayed(existing));
            }
            return Ok(IdempotencyResolution::Conflict(existing));
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO events (event_id, event_type, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                input.event.event_id,
                input.event.event_type,
                input.event.created_at.to_rfc3339(),
                serde_json::to_string(&input.event.payload)?,
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO work_notes (
              note_id, command_id, event_id, work_node_id, note_kind,
              body_hash, body_blob_ref, actor_agent_id, actor_run_id, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                input.note_id,
                input.command_id,
                input.event.event_id,
                input.work_node_id,
                input.note_kind,
                input.body_hash,
                input.body_blob_ref,
                input.actor_agent_id,
                input.actor_run_id,
                input.event.created_at.to_rfc3339(),
            ],
        )?;
        tx.commit()?;
        Ok(IdempotencyResolution::Inserted(
            self.get_work_note_by_command_id(&input.command_id)?
                .expect("inserted work note should be readable"),
        ))
    }

    pub fn get_work_note_by_command_id(&self, command_id: &str) -> Result<Option<WorkNoteRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT note_id, command_id, event_id, work_node_id, note_kind,
                   body_hash, body_blob_ref, actor_agent_id, actor_run_id, created_at
            FROM work_notes
            WHERE command_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![command_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_work_note(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_work_notes_for_node(&self, work_node_id: &str) -> Result<Vec<WorkNoteRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT note_id, command_id, event_id, work_node_id, note_kind,
                   body_hash, body_blob_ref, actor_agent_id, actor_run_id, created_at
            FROM work_notes
            WHERE work_node_id = ?1
            ORDER BY created_at ASC
            "#,
        )?;
        let rows = stmt.query_map(params![work_node_id], row_to_work_note)?;
        let mut notes = Vec::new();
        for row in rows {
            notes.push(row?);
        }
        Ok(notes)
    }

    pub fn insert_scheduler_run_idempotent(
        &self,
        input: &InsertSchedulerRunInput,
    ) -> Result<IdempotencyResolution<SchedulerRunRecord>> {
        if let Some(existing) = self.get_scheduler_run_by_command_id(&input.command_id)? {
            if existing.root_work_node_id == input.root_work_node_id
                && existing.runner == input.runner
                && existing.max_parallel == input.max_parallel
                && existing.acceptance_mode == input.acceptance_mode
                && existing.request_hash == input.request_hash
            {
                return Ok(IdempotencyResolution::Replayed(existing));
            }
            return Ok(IdempotencyResolution::Conflict(existing));
        }
        self.conn.execute(
            r#"
            INSERT INTO scheduler_runs (
              scheduler_run_id, command_id, root_work_node_id, runner, max_parallel,
              acceptance_mode, request_hash, state, created_at, completed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)
            "#,
            params![
                input.scheduler_run_id,
                input.command_id,
                input.root_work_node_id,
                input.runner,
                input.max_parallel,
                input.acceptance_mode,
                input.request_hash,
                input.state,
                input.created_at.to_rfc3339(),
            ],
        )?;
        Ok(IdempotencyResolution::Inserted(
            self.get_scheduler_run(&input.scheduler_run_id)?
                .expect("inserted scheduler run should be readable"),
        ))
    }

    pub fn get_scheduler_run(&self, scheduler_run_id: &str) -> Result<Option<SchedulerRunRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT scheduler_run_id, command_id, root_work_node_id, runner, max_parallel,
                   acceptance_mode, request_hash, state, created_at, completed_at
            FROM scheduler_runs
            WHERE scheduler_run_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![scheduler_run_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_scheduler_run(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_scheduler_run_by_command_id(
        &self,
        command_id: &str,
    ) -> Result<Option<SchedulerRunRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT scheduler_run_id, command_id, root_work_node_id, runner, max_parallel,
                   acceptance_mode, request_hash, state, created_at, completed_at
            FROM scheduler_runs
            WHERE command_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![command_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_scheduler_run(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn latest_scheduler_run_for_root(
        &self,
        root_work_node_id: &str,
    ) -> Result<Option<SchedulerRunRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT scheduler_run_id, command_id, root_work_node_id, runner, max_parallel,
                   acceptance_mode, request_hash, state, created_at, completed_at
            FROM scheduler_runs
            WHERE root_work_node_id = ?1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )?;
        let mut rows = stmt.query(params![root_work_node_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_scheduler_run(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn update_scheduler_run_state(
        &self,
        input: &UpdateSchedulerRunStateInput,
    ) -> Result<SchedulerRunRecord> {
        self.conn.execute(
            r#"
            UPDATE scheduler_runs
            SET state = ?1, completed_at = ?2
            WHERE scheduler_run_id = ?3
            "#,
            params![
                input.state,
                input.completed_at.map(|time| time.to_rfc3339()),
                input.scheduler_run_id,
            ],
        )?;
        self.get_scheduler_run(&input.scheduler_run_id)?
            .ok_or_else(|| anyhow::anyhow!("scheduler run not found: {}", input.scheduler_run_id))
    }

    pub fn insert_scheduler_node_run_claim(
        &self,
        input: &InsertSchedulerNodeRunInput,
    ) -> Result<SchedulerNodeRunRecord> {
        if self
            .list_active_scheduler_node_runs_for_work_node(&input.work_node_id)?
            .iter()
            .any(|run| run.scheduler_run_id != input.scheduler_run_id)
        {
            anyhow::bail!("work node already claimed: {}", input.work_node_id);
        }
        self.conn.execute(
            r#"
            INSERT INTO scheduler_node_runs (
              node_run_id, scheduler_run_id, work_node_id, dispatch_id, worker_agent_id,
              worker_run_id, state, started_at, completed_at
            ) VALUES (?1, ?2, ?3, NULL, ?4, NULL, 'claimed', ?5, NULL)
            "#,
            params![
                input.node_run_id,
                input.scheduler_run_id,
                input.work_node_id,
                input.worker_agent_id,
                input.started_at.to_rfc3339(),
            ],
        )?;
        self.get_scheduler_node_run(&input.node_run_id)?
            .ok_or_else(|| anyhow::anyhow!("scheduler node run not found: {}", input.node_run_id))
    }

    pub fn update_scheduler_node_run(
        &self,
        input: &UpdateSchedulerNodeRunInput,
    ) -> Result<SchedulerNodeRunRecord> {
        self.conn.execute(
            r#"
            UPDATE scheduler_node_runs
            SET dispatch_id = ?1, worker_run_id = ?2, state = ?3, completed_at = ?4
            WHERE node_run_id = ?5
            "#,
            params![
                input.dispatch_id,
                input.worker_run_id,
                input.state,
                input.completed_at.map(|time| time.to_rfc3339()),
                input.node_run_id,
            ],
        )?;
        self.get_scheduler_node_run(&input.node_run_id)?
            .ok_or_else(|| anyhow::anyhow!("scheduler node run not found: {}", input.node_run_id))
    }

    pub fn get_scheduler_node_run(
        &self,
        node_run_id: &str,
    ) -> Result<Option<SchedulerNodeRunRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT node_run_id, scheduler_run_id, work_node_id, dispatch_id, worker_agent_id,
                   worker_run_id, state, started_at, completed_at
            FROM scheduler_node_runs
            WHERE node_run_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![node_run_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_scheduler_node_run(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_scheduler_node_runs_for_scheduler(
        &self,
        scheduler_run_id: &str,
    ) -> Result<Vec<SchedulerNodeRunRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT node_run_id, scheduler_run_id, work_node_id, dispatch_id, worker_agent_id,
                   worker_run_id, state, started_at, completed_at
            FROM scheduler_node_runs
            WHERE scheduler_run_id = ?1
            ORDER BY started_at ASC
            "#,
        )?;
        let rows = stmt.query_map(params![scheduler_run_id], row_to_scheduler_node_run)?;
        let mut runs = Vec::new();
        for row in rows {
            runs.push(row?);
        }
        Ok(runs)
    }

    pub fn list_active_scheduler_node_runs_for_work_node(
        &self,
        work_node_id: &str,
    ) -> Result<Vec<SchedulerNodeRunRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT node_run_id, scheduler_run_id, work_node_id, dispatch_id, worker_agent_id,
                   worker_run_id, state, started_at, completed_at
            FROM scheduler_node_runs
            WHERE work_node_id = ?1 AND state IN ('claimed', 'running')
            ORDER BY started_at ASC
            "#,
        )?;
        let rows = stmt.query_map(params![work_node_id], row_to_scheduler_node_run)?;
        let mut runs = Vec::new();
        for row in rows {
            runs.push(row?);
        }
        Ok(runs)
    }

    pub fn insert_branch_workspace(
        &self,
        input: &InsertBranchWorkspaceInput,
    ) -> Result<BranchWorkspaceRecord> {
        self.conn.execute(
            r#"
            INSERT INTO branch_workspaces (
              branch_id, backend, root_work_node_id, work_node_id, dispatch_id, run_id,
              branch_name, branch_path, branch_ref, state, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)
            "#,
            params![
                input.branch_id,
                input.backend,
                input.root_work_node_id,
                input.work_node_id,
                input.dispatch_id,
                input.run_id,
                input.branch_name,
                input.branch_path,
                input.branch_ref,
                input.state,
                input.created_at.to_rfc3339(),
            ],
        )?;
        self.get_branch_workspace(&input.branch_id)?
            .ok_or_else(|| anyhow::anyhow!("branch not found: {}", input.branch_id))
    }

    pub fn update_branch_workspace(
        &self,
        input: &UpdateBranchWorkspaceInput,
    ) -> Result<BranchWorkspaceRecord> {
        self.conn.execute(
            r#"
            UPDATE branch_workspaces
            SET state = ?1, updated_at = ?2
            WHERE branch_id = ?3
            "#,
            params![input.state, input.updated_at.to_rfc3339(), input.branch_id,],
        )?;
        self.get_branch_workspace(&input.branch_id)?
            .ok_or_else(|| anyhow::anyhow!("branch not found: {}", input.branch_id))
    }

    pub fn get_branch_workspace(&self, branch_id: &str) -> Result<Option<BranchWorkspaceRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT branch_id, backend, root_work_node_id, work_node_id, dispatch_id, run_id,
                   branch_name, branch_path, branch_ref, state, created_at, updated_at
            FROM branch_workspaces
            WHERE branch_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![branch_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_branch_workspace(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_branch_workspace_by_ref(
        &self,
        branch_ref: &str,
    ) -> Result<Option<BranchWorkspaceRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT branch_id, backend, root_work_node_id, work_node_id, dispatch_id, run_id,
                   branch_name, branch_path, branch_ref, state, created_at, updated_at
            FROM branch_workspaces
            WHERE branch_ref = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![branch_ref])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_branch_workspace(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn insert_branch_integration(
        &self,
        input: &InsertBranchIntegrationInput,
    ) -> Result<BranchIntegrationRecord> {
        self.conn.execute(
            r#"
            INSERT INTO branch_integrations (
              integration_id, branch_id, work_node_id, dispatch_id, fact_event_id, branch_ref,
              diff_ref, state, commit_ref, rejection_reason_hash, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)
            "#,
            params![
                input.integration_id,
                input.branch_id,
                input.work_node_id,
                input.dispatch_id,
                input.fact_event_id,
                input.branch_ref,
                input.diff_ref,
                input.state,
                input.commit_ref,
                input.rejection_reason_hash,
                input.created_at.to_rfc3339(),
            ],
        )?;
        self.get_branch_integration(&input.integration_id)?
            .ok_or_else(|| {
                anyhow::anyhow!("branch integration not found: {}", input.integration_id)
            })
    }

    pub fn update_branch_integration(
        &self,
        input: &UpdateBranchIntegrationInput,
        event_type: &str,
        command_id: &str,
    ) -> Result<BranchIntegrationRecord> {
        let event_id = format!("evt_{}", uuid::Uuid::new_v4().simple());
        let created_at = input.updated_at;
        self.insert_event(&EventRecord {
            event_id,
            event_type: event_type.to_string(),
            created_at,
            payload: serde_json::json!({
                "event_type": event_type,
                "command_id": command_id,
                "integration_id": input.integration_id,
                "state": input.state,
                "commit_ref": input.commit_ref,
                "rejection_reason_hash": input.rejection_reason_hash,
                "created_at": created_at,
            }),
        })?;
        self.conn.execute(
            r#"
            UPDATE branch_integrations
            SET state = ?1, commit_ref = ?2, rejection_reason_hash = ?3, updated_at = ?4
            WHERE integration_id = ?5
            "#,
            params![
                input.state,
                input.commit_ref,
                input.rejection_reason_hash,
                input.updated_at.to_rfc3339(),
                input.integration_id,
            ],
        )?;
        self.get_branch_integration(&input.integration_id)?
            .ok_or_else(|| {
                anyhow::anyhow!("branch integration not found: {}", input.integration_id)
            })
    }

    pub fn get_branch_integration(
        &self,
        integration_id: &str,
    ) -> Result<Option<BranchIntegrationRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT integration_id, branch_id, work_node_id, dispatch_id, fact_event_id,
                   branch_ref, diff_ref, state, commit_ref, rejection_reason_hash,
                   created_at, updated_at
            FROM branch_integrations
            WHERE integration_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![integration_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_branch_integration(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_branch_integration_by_branch_id(
        &self,
        branch_id: &str,
    ) -> Result<Option<BranchIntegrationRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT integration_id, branch_id, work_node_id, dispatch_id, fact_event_id,
                   branch_ref, diff_ref, state, commit_ref, rejection_reason_hash,
                   created_at, updated_at
            FROM branch_integrations
            WHERE branch_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![branch_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_branch_integration(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_branch_integrations(&self) -> Result<Vec<BranchIntegrationRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT integration_id, branch_id, work_node_id, dispatch_id, fact_event_id,
                   branch_ref, diff_ref, state, commit_ref, rejection_reason_hash,
                   created_at, updated_at
            FROM branch_integrations
            ORDER BY created_at ASC
            "#,
        )?;
        let rows = stmt.query_map([], row_to_branch_integration)?;
        let mut integrations = Vec::new();
        for row in rows {
            integrations.push(row?);
        }
        Ok(integrations)
    }

    pub fn get_branch_command(&self, command_id: &str) -> Result<Option<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT integration_id, action FROM branch_commands WHERE command_id = ?1")?;
        let mut rows = stmt.query(params![command_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?)))
        } else {
            Ok(None)
        }
    }

    pub fn record_branch_command(&self, input: &RecordBranchCommandInput) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO branch_commands (
              command_id, integration_id, action, request_hash, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                input.command_id,
                input.integration_id,
                input.action,
                input.request_hash,
                input.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn upsert_workflow_template(
        &self,
        input: &UpsertWorkflowTemplateInput,
    ) -> Result<WorkflowTemplateVersionRecord> {
        if let Some(existing) =
            self.get_workflow_template_version(&input.template_id, input.version)?
        {
            if existing.template_hash == input.template_hash {
                return Ok(existing);
            }
            return Err(anyhow::anyhow!(
                "workflow template version conflict: {}@{}",
                input.template_id,
                input.version
            ));
        }
        let existing_template = self.get_workflow_template(&input.template_id)?;
        let tx = self.conn.unchecked_transaction()?;
        if let Some(existing_template) = &existing_template {
            if input.version >= existing_template.latest_version {
                tx.execute(
                    r#"
                    UPDATE workflow_templates
                    SET latest_version = ?2,
                        latest_hash = ?3,
                        title = ?4,
                        source_ref = ?5,
                        updated_at = ?6
                    WHERE template_id = ?1
                    "#,
                    params![
                        input.template_id,
                        input.version,
                        input.template_hash,
                        input.title,
                        input.source_ref,
                        input.created_at.to_rfc3339(),
                    ],
                )?;
            }
        } else {
            tx.execute(
                r#"
                INSERT INTO workflow_templates (
                  template_id, latest_version, latest_hash, title, source_ref, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                "#,
                params![
                    input.template_id,
                    input.version,
                    input.template_hash,
                    input.title,
                    input.source_ref,
                    input.created_at.to_rfc3339(),
                ],
            )?;
        }
        tx.execute(
            r#"
            INSERT INTO workflow_template_versions (
              template_id, version, template_hash, source_ref, spec_json, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                input.template_id,
                input.version,
                input.template_hash,
                input.source_ref,
                serde_json::to_string(&input.spec_json)?,
                input.created_at.to_rfc3339(),
            ],
        )?;
        tx.commit()?;
        self.get_workflow_template_version(&input.template_id, input.version)?
            .ok_or_else(|| anyhow::anyhow!("workflow template not found: {}", input.template_id))
    }

    pub fn get_workflow_import_command(
        &self,
        command_id: &str,
    ) -> Result<Option<(String, i64, String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT template_id, version, template_hash, request_hash
            FROM workflow_import_commands
            WHERE command_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![command_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        } else {
            Ok(None)
        }
    }

    pub fn insert_workflow_import_command(
        &self,
        input: &InsertWorkflowImportCommandInput,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO workflow_import_commands (
              command_id, template_id, version, template_hash, request_hash, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                input.command_id,
                input.template_id,
                input.version,
                input.template_hash,
                input.request_hash,
                input.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_workflow_templates(&self) -> Result<Vec<WorkflowTemplateRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT template_id, latest_version, latest_hash, title, source_ref, created_at, updated_at
            FROM workflow_templates
            ORDER BY template_id ASC
            "#,
        )?;
        let rows = stmt.query_map([], row_to_workflow_template)?;
        let mut templates = Vec::new();
        for row in rows {
            templates.push(row?);
        }
        Ok(templates)
    }

    pub fn get_workflow_template(
        &self,
        template_id: &str,
    ) -> Result<Option<WorkflowTemplateRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT template_id, latest_version, latest_hash, title, source_ref, created_at, updated_at
            FROM workflow_templates
            WHERE template_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![template_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_workflow_template(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_workflow_template_version(
        &self,
        template_id: &str,
        version: i64,
    ) -> Result<Option<WorkflowTemplateVersionRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT template_id, version, template_hash, source_ref, spec_json, created_at
            FROM workflow_template_versions
            WHERE template_id = ?1 AND version = ?2
            "#,
        )?;
        let mut rows = stmt.query(params![template_id, version])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_workflow_template_version(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_latest_workflow_template_version(
        &self,
        template_id: &str,
    ) -> Result<Option<WorkflowTemplateVersionRecord>> {
        if let Some(template) = self.get_workflow_template(template_id)? {
            self.get_workflow_template_version(template_id, template.latest_version)
        } else {
            Ok(None)
        }
    }

    pub fn get_workflow_run_by_command_id(
        &self,
        command_id: &str,
    ) -> Result<Option<WorkflowRunRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT workflow_run_id, command_id, template_id, template_version, template_hash,
                   params_json, params_hash, request_hash, root_work_node_id, scheduler_run_id,
                   state, created_at, completed_at
            FROM workflow_runs
            WHERE command_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![command_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_workflow_run(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_workflow_run(&self, workflow_run_id: &str) -> Result<Option<WorkflowRunRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT workflow_run_id, command_id, template_id, template_version, template_hash,
                   params_json, params_hash, request_hash, root_work_node_id, scheduler_run_id,
                   state, created_at, completed_at
            FROM workflow_runs
            WHERE workflow_run_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![workflow_run_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_workflow_run(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn insert_workflow_run(&self, input: &InsertWorkflowRunInput) -> Result<WorkflowRunRecord> {
        self.conn.execute(
            r#"
            INSERT INTO workflow_runs (
              workflow_run_id, command_id, template_id, template_version, template_hash,
              params_json, params_hash, request_hash, root_work_node_id, scheduler_run_id,
              state, created_at, completed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL)
            "#,
            params![
                input.workflow_run_id,
                input.command_id,
                input.template_id,
                input.template_version,
                input.template_hash,
                serde_json::to_string(&input.params_json)?,
                input.params_hash,
                input.request_hash,
                input.root_work_node_id,
                input.scheduler_run_id,
                input.state,
                input.created_at.to_rfc3339(),
            ],
        )?;
        self.get_workflow_run(&input.workflow_run_id)?
            .ok_or_else(|| anyhow::anyhow!("workflow run not found: {}", input.workflow_run_id))
    }

    pub fn update_workflow_run_scheduler(
        &self,
        input: &UpdateWorkflowRunSchedulerInput,
    ) -> Result<WorkflowRunRecord> {
        self.conn.execute(
            r#"
            UPDATE workflow_runs
            SET scheduler_run_id = ?2,
                state = ?3
            WHERE workflow_run_id = ?1
            "#,
            params![input.workflow_run_id, input.scheduler_run_id, input.state],
        )?;
        self.get_workflow_run(&input.workflow_run_id)?
            .ok_or_else(|| anyhow::anyhow!("workflow run not found: {}", input.workflow_run_id))
    }

    pub fn insert_workflow_run_node(&self, input: &InsertWorkflowRunNodeInput) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO workflow_run_nodes (
              workflow_run_id, node_template_id, work_node_id, output_contract_json,
              capability_policy_json
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                input.workflow_run_id,
                input.node_template_id,
                input.work_node_id,
                serde_json::to_string(&input.output_contract_json)?,
                serde_json::to_string(&input.capability_policy_json)?,
            ],
        )?;
        Ok(())
    }

    pub fn list_workflow_run_nodes(
        &self,
        workflow_run_id: &str,
    ) -> Result<Vec<WorkflowRunNodeRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT workflow_run_id, node_template_id, work_node_id, output_contract_json,
                   capability_policy_json
            FROM workflow_run_nodes
            WHERE workflow_run_id = ?1
            ORDER BY node_template_id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![workflow_run_id], row_to_workflow_run_node)?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }
}

fn row_to_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotRecord> {
    let created_at: String = row.get(2)?;
    let created_at = parse_time_for_sql(&created_at)?;
    let file_count: i64 = row.get(10)?;
    let total_bytes: i64 = row.get(11)?;
    Ok(SnapshotRecord {
        snapshot_id: row.get(0)?,
        event_id: row.get(1)?,
        created_at,
        backend: row.get(3)?,
        capture_root: row.get(4)?,
        label: row.get(5)?,
        agent_id: row.get(6)?,
        dispatch_id: row.get(7)?,
        manifest_path: row.get(8)?,
        manifest_hash: row.get(9)?,
        file_count: file_count.max(0) as u64,
        total_bytes: total_bytes.max(0) as u64,
    })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRecord> {
    let created_at: String = row.get(2)?;
    let payload_json: String = row.get(3)?;
    Ok(EventRecord {
        event_id: row.get(0)?,
        event_type: row.get(1)?,
        created_at: parse_time_for_sql(&created_at)?,
        payload: serde_json::from_str(&payload_json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(err))
        })?,
    })
}

fn row_to_fact(row: &rusqlite::Row<'_>) -> rusqlite::Result<FactRecord> {
    let created_at: String = row.get(2)?;
    let evidence_refs_json: String = row.get(9)?;
    Ok(FactRecord {
        event_id: row.get(0)?,
        command_id: row.get(1)?,
        created_at: parse_time_for_sql(&created_at)?,
        workspace_id: row.get(3)?,
        actor_agent_id: row.get(4)?,
        actor_run_id: row.get(5)?,
        fact_type: row.get(6)?,
        body_hash: row.get(7)?,
        body_blob_ref: row.get(8)?,
        evidence_refs: serde_json::from_str(&evidence_refs_json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(err))
        })?,
    })
}

fn row_to_agent(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentRecord> {
    let created_at: String = row.get(4)?;
    let role: String = row.get(2)?;
    let role = match role.as_str() {
        "orchestrator" => AgentRole::Orchestrator,
        "worker" => AgentRole::Worker,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                format!("invalid role: {role}").into(),
            ))
        }
    };
    Ok(AgentRecord {
        agent_id: row.get(0)?,
        name: row.get(1)?,
        role,
        token_hash: row.get(3)?,
        created_at: parse_time_for_sql(&created_at)?,
        status: row.get(5)?,
    })
}

fn row_to_agent_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentRunRecord> {
    let created_at: String = row.get(3)?;
    Ok(AgentRunRecord {
        run_id: row.get(0)?,
        agent_id: row.get(1)?,
        token_hash: row.get(2)?,
        created_at: parse_time_for_sql(&created_at)?,
    })
}

fn row_to_dispatch(row: &rusqlite::Row<'_>) -> rusqlite::Result<DispatchRecord> {
    let state: String = row.get(7)?;
    let state = match state.as_str() {
        "open" => DispatchState::Open,
        "reported" => DispatchState::Reported,
        "blocked" => DispatchState::Blocked,
        "failed" => DispatchState::Failed,
        "cancelled" => DispatchState::Cancelled,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Text,
                format!("invalid dispatch state: {state}").into(),
            ))
        }
    };
    let created_at: String = row.get(10)?;
    let updated_at: String = row.get(11)?;
    Ok(DispatchRecord {
        dispatch_id: row.get(0)?,
        created_event_id: row.get(1)?,
        command_id: row.get(2)?,
        target_agent_id: row.get(3)?,
        title: row.get(4)?,
        body_hash: row.get(5)?,
        body_blob_ref: row.get(6)?,
        state,
        latest_fact_event_id: row.get(8)?,
        latest_report_status: row.get(9)?,
        created_at: parse_time_for_sql(&created_at)?,
        updated_at: parse_time_for_sql(&updated_at)?,
    })
}

fn row_to_delegation(row: &rusqlite::Row<'_>) -> rusqlite::Result<DelegationRecord> {
    let created_at: String = row.get(10)?;
    let completed_at: Option<String> = row.get(11)?;
    let child_executed: i64 = row.get(9)?;
    Ok(DelegationRecord {
        command_id: row.get(0)?,
        source_agent_id: row.get(1)?,
        source_run_id: row.get(2)?,
        target_agent_id: row.get(3)?,
        worker_run_id: row.get(4)?,
        dispatch_id: row.get(5)?,
        runner: row.get(6)?,
        request_hash: row.get(7)?,
        state: row.get(8)?,
        child_executed: child_executed != 0,
        created_at: parse_time_for_sql(&created_at)?,
        completed_at: completed_at
            .as_deref()
            .map(parse_time_for_sql)
            .transpose()?,
    })
}

fn row_to_work_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkNodeRecord> {
    let created_at: String = row.get(9)?;
    let updated_at: String = row.get(10)?;
    Ok(WorkNodeRecord {
        work_node_id: row.get(0)?,
        command_id: row.get(1)?,
        kind: row.get(2)?,
        title: row.get(3)?,
        body_hash: row.get(4)?,
        body_blob_ref: row.get(5)?,
        status_input: row.get(6)?,
        node_version: row.get(7)?,
        accepted_event_id: row.get(8)?,
        created_at: parse_time_for_sql(&created_at)?,
        updated_at: parse_time_for_sql(&updated_at)?,
    })
}

fn row_to_work_edge(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkEdgeRecord> {
    let created_at: String = row.get(6)?;
    Ok(WorkEdgeRecord {
        work_edge_id: row.get(0)?,
        command_id: row.get(1)?,
        edge_type: row.get(2)?,
        from_node_id: row.get(3)?,
        to_node_id: row.get(4)?,
        graph_version: row.get(5)?,
        created_at: parse_time_for_sql(&created_at)?,
    })
}

fn row_to_work_dispatch_binding(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkDispatchBindingRecord> {
    let created_at: String = row.get(3)?;
    Ok(WorkDispatchBindingRecord {
        work_node_id: row.get(0)?,
        dispatch_id: row.get(1)?,
        binding_kind: row.get(2)?,
        created_at: parse_time_for_sql(&created_at)?,
    })
}

fn row_to_work_ref_binding(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkRefBindingRecord> {
    let created_at: String = row.get(7)?;
    Ok(WorkRefBindingRecord {
        work_node_id: row.get(0)?,
        dispatch_id: row.get(1)?,
        fact_event_id: row.get(2)?,
        snapshot_id: row.get(3)?,
        artifact_ref: row.get(4)?,
        workspace_ref: row.get(5)?,
        diff_ref: row.get(6)?,
        created_at: parse_time_for_sql(&created_at)?,
    })
}

fn row_to_work_root_binding(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkRootBindingRecord> {
    let created_at: String = row.get(4)?;
    Ok(WorkRootBindingRecord {
        root_work_node_id: row.get(0)?,
        work_node_id: row.get(1)?,
        created_by_agent_id: row.get(2)?,
        created_by_run_id: row.get(3)?,
        created_at: parse_time_for_sql(&created_at)?,
    })
}

fn row_to_work_note(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkNoteRecord> {
    let created_at: String = row.get(9)?;
    Ok(WorkNoteRecord {
        note_id: row.get(0)?,
        command_id: row.get(1)?,
        event_id: row.get(2)?,
        work_node_id: row.get(3)?,
        note_kind: row.get(4)?,
        body_hash: row.get(5)?,
        body_blob_ref: row.get(6)?,
        actor_agent_id: row.get(7)?,
        actor_run_id: row.get(8)?,
        created_at: parse_time_for_sql(&created_at)?,
    })
}

fn row_to_scheduler_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<SchedulerRunRecord> {
    let created_at: String = row.get(8)?;
    let completed_at: Option<String> = row.get(9)?;
    Ok(SchedulerRunRecord {
        scheduler_run_id: row.get(0)?,
        command_id: row.get(1)?,
        root_work_node_id: row.get(2)?,
        runner: row.get(3)?,
        max_parallel: row.get(4)?,
        acceptance_mode: row.get(5)?,
        request_hash: row.get(6)?,
        state: row.get(7)?,
        created_at: parse_time_for_sql(&created_at)?,
        completed_at: completed_at
            .as_deref()
            .map(parse_time_for_sql)
            .transpose()?,
    })
}

fn row_to_scheduler_node_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<SchedulerNodeRunRecord> {
    let started_at: String = row.get(7)?;
    let completed_at: Option<String> = row.get(8)?;
    Ok(SchedulerNodeRunRecord {
        node_run_id: row.get(0)?,
        scheduler_run_id: row.get(1)?,
        work_node_id: row.get(2)?,
        dispatch_id: row.get(3)?,
        worker_agent_id: row.get(4)?,
        worker_run_id: row.get(5)?,
        state: row.get(6)?,
        started_at: parse_time_for_sql(&started_at)?,
        completed_at: completed_at
            .as_deref()
            .map(parse_time_for_sql)
            .transpose()?,
    })
}

fn row_to_branch_workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<BranchWorkspaceRecord> {
    let created_at: String = row.get(10)?;
    let updated_at: String = row.get(11)?;
    Ok(BranchWorkspaceRecord {
        branch_id: row.get(0)?,
        backend: row.get(1)?,
        root_work_node_id: row.get(2)?,
        work_node_id: row.get(3)?,
        dispatch_id: row.get(4)?,
        run_id: row.get(5)?,
        branch_name: row.get(6)?,
        branch_path: row.get(7)?,
        branch_ref: row.get(8)?,
        state: row.get(9)?,
        created_at: parse_time_for_sql(&created_at)?,
        updated_at: parse_time_for_sql(&updated_at)?,
    })
}

fn row_to_branch_integration(row: &rusqlite::Row<'_>) -> rusqlite::Result<BranchIntegrationRecord> {
    let created_at: String = row.get(10)?;
    let updated_at: String = row.get(11)?;
    Ok(BranchIntegrationRecord {
        integration_id: row.get(0)?,
        branch_id: row.get(1)?,
        work_node_id: row.get(2)?,
        dispatch_id: row.get(3)?,
        fact_event_id: row.get(4)?,
        branch_ref: row.get(5)?,
        diff_ref: row.get(6)?,
        state: row.get(7)?,
        commit_ref: row.get(8)?,
        rejection_reason_hash: row.get(9)?,
        created_at: parse_time_for_sql(&created_at)?,
        updated_at: parse_time_for_sql(&updated_at)?,
    })
}

fn row_to_workflow_template(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowTemplateRecord> {
    let created_at: String = row.get(5)?;
    let updated_at: String = row.get(6)?;
    Ok(WorkflowTemplateRecord {
        template_id: row.get(0)?,
        latest_version: row.get(1)?,
        latest_hash: row.get(2)?,
        title: row.get(3)?,
        source_ref: row.get(4)?,
        created_at: parse_time_for_sql(&created_at)?,
        updated_at: parse_time_for_sql(&updated_at)?,
    })
}

fn row_to_workflow_template_version(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkflowTemplateVersionRecord> {
    let spec_json: String = row.get(4)?;
    let created_at: String = row.get(5)?;
    Ok(WorkflowTemplateVersionRecord {
        template_id: row.get(0)?,
        version: row.get(1)?,
        template_hash: row.get(2)?,
        source_ref: row.get(3)?,
        spec_json: parse_json_for_sql(&spec_json)?,
        created_at: parse_time_for_sql(&created_at)?,
    })
}

fn row_to_workflow_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowRunRecord> {
    let params_json: String = row.get(5)?;
    let created_at: String = row.get(11)?;
    let completed_at: Option<String> = row.get(12)?;
    Ok(WorkflowRunRecord {
        workflow_run_id: row.get(0)?,
        command_id: row.get(1)?,
        template_id: row.get(2)?,
        template_version: row.get(3)?,
        template_hash: row.get(4)?,
        params_json: parse_json_for_sql(&params_json)?,
        params_hash: row.get(6)?,
        request_hash: row.get(7)?,
        root_work_node_id: row.get(8)?,
        scheduler_run_id: row.get(9)?,
        state: row.get(10)?,
        created_at: parse_time_for_sql(&created_at)?,
        completed_at: completed_at
            .as_deref()
            .map(parse_time_for_sql)
            .transpose()?,
    })
}

fn row_to_workflow_run_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowRunNodeRecord> {
    let output_contract_json: String = row.get(3)?;
    let capability_policy_json: String = row.get(4)?;
    Ok(WorkflowRunNodeRecord {
        workflow_run_id: row.get(0)?,
        node_template_id: row.get(1)?,
        work_node_id: row.get(2)?,
        output_contract_json: parse_json_for_sql(&output_contract_json)?,
        capability_policy_json: parse_json_for_sql(&capability_policy_json)?,
    })
}

fn parse_time_for_sql(value: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
        })
}

fn parse_json_for_sql(value: &str) -> rusqlite::Result<Value> {
    serde_json::from_str(value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}
