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

fn parse_time_for_sql(value: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
        })
}
