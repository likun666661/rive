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

fn parse_time_for_sql(value: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
        })
}
