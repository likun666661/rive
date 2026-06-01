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

fn parse_time_for_sql(value: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
        })
}
