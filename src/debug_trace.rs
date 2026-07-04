use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceAdapter {
    CodexHook,
    OpenCodePlugin,
}

impl TraceAdapter {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "codex-hook" => Ok(Self::CodexHook),
            "opencode-plugin" => Ok(Self::OpenCodePlugin),
            _ => Err(anyhow!("unsupported trace adapter: {value}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodexHook => "codex-hook",
            Self::OpenCodePlugin => "opencode-plugin",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DebugTraceRawRecord {
    pub raw_event_id: String,
    pub trace_session_id: Option<String>,
    pub workspace_id: String,
    pub adapter: String,
    pub external_event_type: String,
    pub external_event_id: Option<String>,
    pub sequence: i64,
    pub received_at: DateTime<Utc>,
    pub payload_hash: String,
    pub payload_blob_ref: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DebugTraceEventRecord {
    pub trace_event_id: String,
    pub raw_event_id: String,
    pub trace_session_id: Option<String>,
    pub workspace_id: String,
    pub adapter: String,
    pub event_kind: String,
    pub occurred_at: Option<DateTime<Utc>>,
    pub sequence: i64,
    pub agent_id: Option<String>,
    pub run_id: Option<String>,
    pub dispatch_id: Option<String>,
    pub external_session_id: Option<String>,
    pub external_turn_id: Option<String>,
    pub external_tool_id: Option<String>,
    pub summary: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct DebugTraceSessionRecord {
    pub trace_session_id: String,
    pub workspace_id: String,
    pub adapter: String,
    pub external_session_id: Option<String>,
    pub agent_id: Option<String>,
    pub run_id: Option<String>,
    pub dispatch_id: Option<String>,
    pub cwd: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub metadata: Value,
}

#[derive(Debug)]
pub struct IngestTraceInput {
    pub adapter: TraceAdapter,
    pub payload: Vec<u8>,
    pub agent_id: Option<String>,
    pub run_id: Option<String>,
    pub dispatch_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IngestTraceProtocol {
    pub raw_event: DebugTraceRawRecord,
    pub trace_event: DebugTraceEventRecord,
}

#[derive(Debug, Serialize)]
pub struct TraceListProtocol {
    pub events: Vec<DebugTraceEventRecord>,
}

#[derive(Debug, Serialize)]
pub struct TraceShowProtocol {
    pub raw_event: DebugTraceRawRecord,
    pub trace_event: DebugTraceEventRecord,
    pub raw_payload: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct TraceSessionProtocol {
    pub session: DebugTraceSessionRecord,
    pub events: Vec<DebugTraceEventRecord>,
}

#[derive(Debug, Serialize)]
pub struct TraceInstallProtocol {
    pub target: String,
    pub path: String,
    pub status: String,
}

#[derive(Debug, Default)]
pub struct TraceListFilter {
    pub adapter: Option<String>,
    pub agent_id: Option<String>,
    pub dispatch_id: Option<String>,
    pub trace_session_id: Option<String>,
}

#[derive(Debug, Default)]
pub struct TraceUsageFilter {
    pub run_id: Option<String>,
    pub agent_id: Option<String>,
    pub dispatch_id: Option<String>,
    pub correlated_run_ids: std::collections::BTreeSet<String>,
    pub correlated_dispatch_ids: std::collections::BTreeSet<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TraceUsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_tokens: i64,
    pub non_cache_tokens: i64,
    pub tool_output_bytes: i64,
    pub trace_event_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceUsageRunProtocol {
    pub run_id: String,
    pub adapter: Option<String>,
    pub agent_id: Option<String>,
    pub dispatch_id: Option<String>,
    pub usage_available: bool,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_tokens: i64,
    pub non_cache_tokens: i64,
    pub tool_output_bytes: i64,
    pub trace_event_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceUsageProtocol {
    pub runs: Vec<TraceUsageRunProtocol>,
    pub totals: TraceUsageTotals,
}

pub struct DebugTraceStore {
    conn: Connection,
}

impl DebugTraceStore {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            conn: Connection::open(path)?,
        })
    }

    pub fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS debug_trace_sessions (
              trace_session_id TEXT PRIMARY KEY,
              workspace_id TEXT NOT NULL,
              adapter TEXT NOT NULL,
              external_session_id TEXT,
              agent_id TEXT,
              run_id TEXT,
              dispatch_id TEXT,
              cwd TEXT,
              started_at TEXT NOT NULL,
              ended_at TEXT,
              metadata_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_debug_trace_sessions_lookup
              ON debug_trace_sessions(workspace_id, adapter, external_session_id);

            CREATE TABLE IF NOT EXISTS debug_trace_raw_events (
              raw_event_id TEXT PRIMARY KEY,
              trace_session_id TEXT,
              workspace_id TEXT NOT NULL,
              adapter TEXT NOT NULL,
              external_event_type TEXT NOT NULL,
              external_event_id TEXT,
              sequence INTEGER NOT NULL,
              received_at TEXT NOT NULL,
              payload_hash TEXT NOT NULL,
              payload_blob_ref TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_debug_trace_raw_session
              ON debug_trace_raw_events(trace_session_id, sequence);

            CREATE TABLE IF NOT EXISTS debug_trace_events (
              trace_event_id TEXT PRIMARY KEY,
              raw_event_id TEXT NOT NULL,
              trace_session_id TEXT,
              workspace_id TEXT NOT NULL,
              adapter TEXT NOT NULL,
              event_kind TEXT NOT NULL,
              occurred_at TEXT,
              sequence INTEGER NOT NULL,
              agent_id TEXT,
              run_id TEXT,
              dispatch_id TEXT,
              external_session_id TEXT,
              external_turn_id TEXT,
              external_tool_id TEXT,
              summary_json TEXT NOT NULL,
              FOREIGN KEY(raw_event_id) REFERENCES debug_trace_raw_events(raw_event_id)
            );
            CREATE INDEX IF NOT EXISTS idx_debug_trace_events_session
              ON debug_trace_events(trace_session_id, sequence);
            CREATE INDEX IF NOT EXISTS idx_debug_trace_events_adapter
              ON debug_trace_events(adapter, sequence);
            CREATE INDEX IF NOT EXISTS idx_debug_trace_events_agent
              ON debug_trace_events(agent_id, sequence);
            CREATE INDEX IF NOT EXISTS idx_debug_trace_events_dispatch
              ON debug_trace_events(dispatch_id, sequence);
            "#,
        )?;
        Ok(())
    }

    pub fn ingest(
        &self,
        workspace: &Workspace,
        input: IngestTraceInput,
    ) -> Result<IngestTraceProtocol> {
        self.init_schema()?;
        let payload: Value = serde_json::from_slice(&input.payload)
            .map_err(|err| anyhow!("invalid trace payload json: {err}"))?;
        let received_at = Utc::now();
        let normalized = normalize(input.adapter, &payload, &input);
        let payload_sha = sha256_hex(&input.payload);
        let payload_hash = format!("sha256:{payload_sha}");
        let payload_blob_ref = write_payload_blob(workspace, &payload_sha, &input.payload)?;
        let trace_session_id =
            self.ensure_session(workspace, input.adapter, &normalized, received_at)?;
        let sequence = self.next_sequence(trace_session_id.as_deref())?;
        let raw_event = DebugTraceRawRecord {
            raw_event_id: prefixed_id("raw"),
            trace_session_id: trace_session_id.clone(),
            workspace_id: workspace_id(workspace),
            adapter: input.adapter.as_str().to_string(),
            external_event_type: normalized.external_event_type.clone(),
            external_event_id: normalized.external_event_id.clone(),
            sequence,
            received_at,
            payload_hash,
            payload_blob_ref,
        };
        let trace_event = DebugTraceEventRecord {
            trace_event_id: prefixed_id("trace"),
            raw_event_id: raw_event.raw_event_id.clone(),
            trace_session_id,
            workspace_id: workspace_id(workspace),
            adapter: input.adapter.as_str().to_string(),
            event_kind: normalized.event_kind,
            occurred_at: normalized.occurred_at,
            sequence,
            agent_id: normalized.agent_id.or(input.agent_id),
            run_id: normalized.run_id.or(input.run_id),
            dispatch_id: normalized.dispatch_id.or(input.dispatch_id),
            external_session_id: normalized.external_session_id,
            external_turn_id: normalized.external_turn_id,
            external_tool_id: normalized.external_tool_id,
            summary: normalized.summary,
        };

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            r#"
            INSERT INTO debug_trace_raw_events (
              raw_event_id, trace_session_id, workspace_id, adapter, external_event_type,
              external_event_id, sequence, received_at, payload_hash, payload_blob_ref
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                raw_event.raw_event_id,
                raw_event.trace_session_id,
                raw_event.workspace_id,
                raw_event.adapter,
                raw_event.external_event_type,
                raw_event.external_event_id,
                raw_event.sequence,
                raw_event.received_at.to_rfc3339(),
                raw_event.payload_hash,
                raw_event.payload_blob_ref,
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO debug_trace_events (
              trace_event_id, raw_event_id, trace_session_id, workspace_id, adapter,
              event_kind, occurred_at, sequence, agent_id, run_id, dispatch_id,
              external_session_id, external_turn_id, external_tool_id, summary_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            "#,
            params![
                trace_event.trace_event_id,
                trace_event.raw_event_id,
                trace_event.trace_session_id,
                trace_event.workspace_id,
                trace_event.adapter,
                trace_event.event_kind,
                trace_event.occurred_at.map(|time| time.to_rfc3339()),
                trace_event.sequence,
                trace_event.agent_id,
                trace_event.run_id,
                trace_event.dispatch_id,
                trace_event.external_session_id,
                trace_event.external_turn_id,
                trace_event.external_tool_id,
                serde_json::to_string(&trace_event.summary)?,
            ],
        )?;
        tx.commit()?;
        Ok(IngestTraceProtocol {
            raw_event,
            trace_event,
        })
    }

    pub fn list_events(&self, filter: TraceListFilter) -> Result<Vec<DebugTraceEventRecord>> {
        self.init_schema()?;
        let mut stmt = self.conn.prepare(
            r#"
            SELECT trace_event_id, raw_event_id, trace_session_id, workspace_id, adapter,
                   event_kind, occurred_at, sequence, agent_id, run_id, dispatch_id,
                   external_session_id, external_turn_id, external_tool_id, summary_json
            FROM debug_trace_events
            WHERE (?1 IS NULL OR adapter = ?1)
              AND (?2 IS NULL OR agent_id = ?2)
              AND (?3 IS NULL OR dispatch_id = ?3)
              AND (?4 IS NULL OR trace_session_id = ?4)
            ORDER BY sequence DESC
            "#,
        )?;
        let rows = stmt.query_map(
            params![
                filter.adapter,
                filter.agent_id,
                filter.dispatch_id,
                filter.trace_session_id,
            ],
            row_to_trace_event,
        )?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    pub fn show_event(&self, id: &str, include_payload: bool) -> Result<TraceShowProtocol> {
        self.init_schema()?;
        let trace_event = match self.get_trace_event(id)? {
            Some(event) => event,
            None => self
                .get_trace_event_by_raw_id(id)?
                .ok_or_else(|| anyhow!("debug trace event not found: {id}"))?,
        };
        let raw_event = self
            .get_raw_event(&trace_event.raw_event_id)?
            .ok_or_else(|| {
                anyhow!(
                    "debug trace raw event missing: {}",
                    trace_event.raw_event_id
                )
            })?;
        let raw_payload = if include_payload {
            Some(read_payload_blob(
                &self.workspace_from_id(&raw_event.workspace_id),
                &raw_event.payload_blob_ref,
            )?)
        } else {
            None
        };
        Ok(TraceShowProtocol {
            raw_event,
            trace_event,
            raw_payload,
        })
    }

    pub fn session(&self, trace_session_id: &str) -> Result<TraceSessionProtocol> {
        self.init_schema()?;
        let session = self
            .get_session(trace_session_id)?
            .ok_or_else(|| anyhow!("debug trace session not found: {trace_session_id}"))?;
        let mut events = self.list_events(TraceListFilter {
            trace_session_id: Some(trace_session_id.to_string()),
            ..TraceListFilter::default()
        })?;
        events.sort_by_key(|event| event.sequence);
        Ok(TraceSessionProtocol { session, events })
    }

    fn ensure_session(
        &self,
        workspace: &Workspace,
        adapter: TraceAdapter,
        normalized: &NormalizedEvent,
        started_at: DateTime<Utc>,
    ) -> Result<Option<String>> {
        let Some(external_session_id) = normalized.external_session_id.as_deref() else {
            return Ok(None);
        };
        if let Some(existing) = self.find_session(
            &workspace_id(workspace),
            adapter.as_str(),
            external_session_id,
        )? {
            return Ok(Some(existing.trace_session_id));
        }
        let session = DebugTraceSessionRecord {
            trace_session_id: prefixed_id("trs"),
            workspace_id: workspace_id(workspace),
            adapter: adapter.as_str().to_string(),
            external_session_id: Some(external_session_id.to_string()),
            agent_id: normalized.agent_id.clone(),
            run_id: normalized.run_id.clone(),
            dispatch_id: normalized.dispatch_id.clone(),
            cwd: normalized.cwd.clone(),
            started_at,
            ended_at: None,
            metadata: json!({
                "adapter": adapter.as_str(),
                "external_event_type": normalized.external_event_type,
            }),
        };
        self.conn.execute(
            r#"
            INSERT INTO debug_trace_sessions (
              trace_session_id, workspace_id, adapter, external_session_id,
              agent_id, run_id, dispatch_id, cwd, started_at, ended_at, metadata_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10)
            "#,
            params![
                session.trace_session_id,
                session.workspace_id,
                session.adapter,
                session.external_session_id,
                session.agent_id,
                session.run_id,
                session.dispatch_id,
                session.cwd,
                session.started_at.to_rfc3339(),
                serde_json::to_string(&session.metadata)?,
            ],
        )?;
        Ok(Some(session.trace_session_id))
    }

    fn next_sequence(&self, trace_session_id: Option<&str>) -> Result<i64> {
        if let Some(trace_session_id) = trace_session_id {
            let current: i64 = self.conn.query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM debug_trace_events WHERE trace_session_id = ?1",
                params![trace_session_id],
                |row| row.get(0),
            )?;
            Ok(current + 1)
        } else {
            let current: i64 = self.conn.query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM debug_trace_events",
                [],
                |row| row.get(0),
            )?;
            Ok(current + 1)
        }
    }

    fn find_session(
        &self,
        workspace_id: &str,
        adapter: &str,
        external_session_id: &str,
    ) -> Result<Option<DebugTraceSessionRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT trace_session_id, workspace_id, adapter, external_session_id, agent_id,
                   run_id, dispatch_id, cwd, started_at, ended_at, metadata_json
            FROM debug_trace_sessions
            WHERE workspace_id = ?1 AND adapter = ?2 AND external_session_id = ?3
            "#,
        )?;
        let mut rows = stmt.query(params![workspace_id, adapter, external_session_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_trace_session(row)?))
        } else {
            Ok(None)
        }
    }

    fn get_session(&self, trace_session_id: &str) -> Result<Option<DebugTraceSessionRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT trace_session_id, workspace_id, adapter, external_session_id, agent_id,
                   run_id, dispatch_id, cwd, started_at, ended_at, metadata_json
            FROM debug_trace_sessions
            WHERE trace_session_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![trace_session_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_trace_session(row)?))
        } else {
            Ok(None)
        }
    }

    fn get_raw_event(&self, raw_event_id: &str) -> Result<Option<DebugTraceRawRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT raw_event_id, trace_session_id, workspace_id, adapter, external_event_type,
                   external_event_id, sequence, received_at, payload_hash, payload_blob_ref
            FROM debug_trace_raw_events
            WHERE raw_event_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![raw_event_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_raw_event(row)?))
        } else {
            Ok(None)
        }
    }

    fn get_trace_event(&self, trace_event_id: &str) -> Result<Option<DebugTraceEventRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT trace_event_id, raw_event_id, trace_session_id, workspace_id, adapter,
                   event_kind, occurred_at, sequence, agent_id, run_id, dispatch_id,
                   external_session_id, external_turn_id, external_tool_id, summary_json
            FROM debug_trace_events
            WHERE trace_event_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![trace_event_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_trace_event(row)?))
        } else {
            Ok(None)
        }
    }

    fn get_trace_event_by_raw_id(
        &self,
        raw_event_id: &str,
    ) -> Result<Option<DebugTraceEventRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT trace_event_id, raw_event_id, trace_session_id, workspace_id, adapter,
                   event_kind, occurred_at, sequence, agent_id, run_id, dispatch_id,
                   external_session_id, external_turn_id, external_tool_id, summary_json
            FROM debug_trace_events
            WHERE raw_event_id = ?1
            "#,
        )?;
        let mut rows = stmt.query(params![raw_event_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_trace_event(row)?))
        } else {
            Ok(None)
        }
    }

    fn workspace_from_id(&self, workspace_id: &str) -> Workspace {
        Workspace {
            root: workspace_id.into(),
        }
    }
}

pub fn usage_for_workspace(
    workspace: &Workspace,
    store: &DebugTraceStore,
    filter: TraceUsageFilter,
) -> Result<TraceUsageProtocol> {
    let mut events = store.list_events(TraceListFilter {
        agent_id: filter.agent_id,
        dispatch_id: filter.dispatch_id.clone(),
        ..TraceListFilter::default()
    })?;
    if let Some(run_id) = &filter.run_id {
        events.retain(|event| event.run_id.as_deref() == Some(run_id));
    }
    let has_correlation_filter =
        !filter.correlated_run_ids.is_empty() || !filter.correlated_dispatch_ids.is_empty();
    if has_correlation_filter {
        events.retain(|event| {
            event
                .run_id
                .as_ref()
                .is_some_and(|run_id| filter.correlated_run_ids.contains(run_id))
                || event
                    .dispatch_id
                    .as_ref()
                    .is_some_and(|dispatch_id| filter.correlated_dispatch_ids.contains(dispatch_id))
        });
    }
    let mut by_run: std::collections::BTreeMap<String, Vec<DebugTraceEventRecord>> =
        std::collections::BTreeMap::new();
    for event in events {
        if let Some(run_id) = &event.run_id {
            by_run.entry(run_id.clone()).or_default().push(event);
        }
    }
    if let Some(run_id) = filter.run_id {
        by_run.entry(run_id).or_default();
    }
    for run_id in filter.correlated_run_ids {
        by_run.entry(run_id).or_default();
    }
    let mut runs = Vec::new();
    let mut totals = TraceUsageTotals::default();
    for (run_id, events) in by_run {
        let mut run = usage_for_run(workspace, &run_id, &events)?;
        run.trace_event_count = events.len() as i64;
        totals.input_tokens += run.input_tokens;
        totals.output_tokens += run.output_tokens;
        totals.reasoning_tokens += run.reasoning_tokens;
        totals.cache_read_tokens += run.cache_read_tokens;
        totals.total_tokens += run.total_tokens;
        totals.non_cache_tokens += run.non_cache_tokens;
        totals.tool_output_bytes += run.tool_output_bytes;
        totals.trace_event_count += run.trace_event_count;
        runs.push(run);
    }
    Ok(TraceUsageProtocol { runs, totals })
}

fn usage_for_run(
    workspace: &Workspace,
    run_id: &str,
    events: &[DebugTraceEventRecord],
) -> Result<TraceUsageRunProtocol> {
    let mut totals = TraceUsageTotals::default();
    let stdout_path = workspace.debug_runs_dir().join(run_id).join("stdout.jsonl");
    if let Ok(bytes) = fs::read(&stdout_path) {
        totals.tool_output_bytes = bytes.len() as i64;
        for line in bytes.split(|byte| *byte == b'\n') {
            if line.iter().all(|byte| byte.is_ascii_whitespace()) {
                continue;
            }
            let Ok(value) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if let Some(tokens) = value
                .get("tokens")
                .or_else(|| value.pointer("/properties/tokens"))
            {
                add_tokens(&mut totals, tokens);
            }
        }
    }
    let usage_available = totals.total_tokens > 0
        || totals.input_tokens > 0
        || totals.output_tokens > 0
        || totals.reasoning_tokens > 0
        || totals.cache_read_tokens > 0;
    let adapter = events.iter().map(|event| event.adapter.clone()).next();
    let agent_id = events.iter().find_map(|event| event.agent_id.clone());
    let dispatch_id = events.iter().find_map(|event| event.dispatch_id.clone());
    Ok(TraceUsageRunProtocol {
        run_id: run_id.to_string(),
        adapter,
        agent_id,
        dispatch_id,
        usage_available,
        input_tokens: totals.input_tokens,
        output_tokens: totals.output_tokens,
        reasoning_tokens: totals.reasoning_tokens,
        cache_read_tokens: totals.cache_read_tokens,
        total_tokens: totals.total_tokens,
        non_cache_tokens: totals.non_cache_tokens,
        tool_output_bytes: totals.tool_output_bytes,
        trace_event_count: events.len() as i64,
    })
}

fn add_tokens(totals: &mut TraceUsageTotals, tokens: &Value) {
    let input = int_at(tokens, &["input"]).unwrap_or(0);
    let output = int_at(tokens, &["output"]).unwrap_or(0);
    let reasoning = int_at(tokens, &["reasoning"]).unwrap_or(0);
    let cache_read = int_at(tokens, &["cache", "read"])
        .or_else(|| int_at(tokens, &["cache_read"]))
        .unwrap_or(0);
    let total = int_at(tokens, &["total"]).unwrap_or(input + output + reasoning + cache_read);
    totals.input_tokens += input;
    totals.output_tokens += output;
    totals.reasoning_tokens += reasoning;
    totals.cache_read_tokens += cache_read;
    totals.total_tokens += total;
    totals.non_cache_tokens += total.saturating_sub(cache_read);
}

#[derive(Debug)]
struct NormalizedEvent {
    external_event_type: String,
    external_event_id: Option<String>,
    event_kind: String,
    occurred_at: Option<DateTime<Utc>>,
    agent_id: Option<String>,
    run_id: Option<String>,
    dispatch_id: Option<String>,
    external_session_id: Option<String>,
    external_turn_id: Option<String>,
    external_tool_id: Option<String>,
    cwd: Option<String>,
    summary: Value,
}

fn normalize(adapter: TraceAdapter, payload: &Value, input: &IngestTraceInput) -> NormalizedEvent {
    match adapter {
        TraceAdapter::CodexHook => normalize_codex(payload, input),
        TraceAdapter::OpenCodePlugin => normalize_opencode(payload, input),
    }
}

fn normalize_codex(payload: &Value, input: &IngestTraceInput) -> NormalizedEvent {
    let external_event_type = string_at(payload, &["hook_event_name"])
        .or_else(|| string_at(payload, &["type"]))
        .unwrap_or_else(|| "unknown".to_string());
    let event_kind = match external_event_type.as_str() {
        "SessionStart" => "session_started",
        "UserPromptSubmit" => "user_prompt",
        "PreToolUse" => "tool_call_started",
        "PostToolUse" => {
            if int_at(payload, &["tool_response", "exit_code"]).is_some_and(|code| code != 0) {
                "tool_call_failed"
            } else {
                "tool_call_completed"
            }
        }
        "PermissionRequest" => "permission_requested",
        "SubagentStart" => "subagent_started",
        "SubagentStop" => "subagent_stopped",
        "Stop" => "session_ended",
        "PreCompact" | "PostCompact" => "session_status_changed",
        _ => "unknown",
    };
    let session_id = string_at(payload, &["session_id"]);
    NormalizedEvent {
        external_event_type,
        external_event_id: string_at(payload, &["event_id"])
            .or_else(|| string_at(payload, &["id"])),
        event_kind: event_kind.to_string(),
        occurred_at: parse_optional_time(payload),
        agent_id: string_at(payload, &["agent_id"]).or_else(|| input.agent_id.clone()),
        run_id: string_at(payload, &["run_id"]).or_else(|| input.run_id.clone()),
        dispatch_id: string_at(payload, &["dispatch_id"]).or_else(|| input.dispatch_id.clone()),
        external_session_id: session_id,
        external_turn_id: string_at(payload, &["turn_id"]),
        external_tool_id: string_at(payload, &["tool_use_id"]),
        cwd: string_at(payload, &["cwd"]),
        summary: json!({
            "hook_event_name": value_at(payload, &["hook_event_name"]),
            "tool_name": value_at(payload, &["tool_name"]),
            "transcript_path": value_at(payload, &["transcript_path"]),
            "model": value_at(payload, &["model"]),
            "permission_mode": value_at(payload, &["permission_mode"]),
        }),
    }
}

fn normalize_opencode(payload: &Value, input: &IngestTraceInput) -> NormalizedEvent {
    let external_event_type = string_at(payload, &["type"])
        .or_else(|| string_at(payload, &["event", "type"]))
        .unwrap_or_else(|| "unknown".to_string());
    let part_type = first_string_at(
        payload,
        &[&["part", "type"], &["properties", "part", "type"]],
    );
    let tool_status = first_string_at(
        payload,
        &[
            &["state", "status"],
            &["part", "state", "status"],
            &["properties", "state", "status"],
            &["properties", "part", "state", "status"],
        ],
    );
    let event_kind = match (
        external_event_type.as_str(),
        part_type.as_deref(),
        tool_status.as_deref(),
    ) {
        ("message.part.updated", Some("tool"), Some("completed")) => "tool_call_completed",
        ("message.part.updated", Some("tool"), _) => "tool_call_started",
        ("session.created", _, _) => "session_started",
        ("session.status" | "session.updated", _, _) => "session_status_changed",
        ("session.idle", _, _) => "session_idle",
        ("session.error", _, _) => "session_error",
        ("message.updated", _, _) => "assistant_output",
        ("message.part.updated", _, _) => "assistant_output_delta",
        ("tool.execute.before", _, _) => "tool_call_started",
        ("tool.execute.after", _, _) => "tool_call_completed",
        ("permission.asked", _, _) => "permission_requested",
        ("permission.replied", _, _) => "permission_resolved",
        ("command.executed", _, _) => "command_executed",
        ("session.diff", _, _) => "file_changed",
        _ => "unknown",
    };
    let session_id = first_string_at(
        payload,
        &[
            &["session", "id"],
            &["sessionID"],
            &["properties", "session", "id"],
            &["properties", "sessionID"],
            &["properties", "info", "sessionID"],
            &["properties", "part", "sessionID"],
        ],
    );
    let text_preview = first_string_at(
        payload,
        &[
            &["text"],
            &["part", "text"],
            &["message", "text"],
            &["properties", "text"],
            &["properties", "part", "text"],
            &["properties", "message", "text"],
        ],
    )
    .map(|text| truncate_for_summary(&text, 240));
    NormalizedEvent {
        external_event_type,
        external_event_id: string_at(payload, &["id"])
            .or_else(|| string_at(payload, &["event_id"])),
        event_kind: event_kind.to_string(),
        occurred_at: parse_time_from_paths(
            payload,
            &[
                &["time"],
                &["timestamp"],
                &["properties", "time"],
                &["properties", "timestamp"],
                &["properties", "info", "time", "created"],
            ],
        ),
        agent_id: first_string_at(
            payload,
            &[
                &["agent_id"],
                &["agent"],
                &["properties", "agent"],
                &["properties", "info", "agent"],
            ],
        )
        .or_else(|| input.agent_id.clone()),
        run_id: string_at(payload, &["run_id"]).or_else(|| input.run_id.clone()),
        dispatch_id: string_at(payload, &["dispatch_id"]).or_else(|| input.dispatch_id.clone()),
        external_session_id: session_id,
        external_turn_id: first_string_at(
            payload,
            &[
                &["message", "id"],
                &["part", "id"],
                &["messageID"],
                &["properties", "message", "id"],
                &["properties", "part", "messageID"],
                &["properties", "part", "id"],
                &["properties", "info", "id"],
            ],
        ),
        external_tool_id: first_string_at(
            payload,
            &[
                &["tool", "id"],
                &["toolID"],
                &["callID"],
                &["part", "callID"],
                &["properties", "tool", "id"],
                &["properties", "toolID"],
                &["properties", "callID"],
                &["properties", "part", "callID"],
            ],
        ),
        cwd: first_string_at(
            payload,
            &[
                &["cwd"],
                &["properties", "cwd"],
                &["properties", "info", "path", "cwd"],
            ],
        ),
        summary: json!({
            "type": value_at(payload, &["type"]),
            "tool_name": first_value_at(
                payload,
                &[
                    &["tool", "name"],
                    &["tool"],
                    &["part", "tool"],
                    &["properties", "tool", "name"],
                    &["properties", "tool"],
                    &["properties", "part", "tool"],
                ],
            ),
            "tool_status": first_value_at(
                payload,
                &[
                    &["state", "status"],
                    &["part", "state", "status"],
                    &["properties", "state", "status"],
                    &["properties", "part", "state", "status"],
                ],
            ),
            "tool_input_preview": first_preview_at(
                payload,
                &[
                    &["input"],
                    &["state", "input"],
                    &["part", "state", "input"],
                    &["properties", "input"],
                    &["properties", "part", "state", "input"],
                ],
                240,
            ),
            "tool_output_preview": first_preview_at(
                payload,
                &[
                    &["output"],
                    &["state", "output"],
                    &["part", "state", "output"],
                    &["properties", "output"],
                    &["properties", "part", "state", "output"],
                ],
                240,
            ),
            "session_status": first_value_at(
                payload,
                &[
                    &["session", "status"],
                    &["status"],
                    &["properties", "session", "status"],
                    &["properties", "status"],
                ],
            ),
            "message_role": first_value_at(
                payload,
                &[
                    &["message", "role"],
                    &["properties", "message", "role"],
                    &["properties", "info", "role"],
                ],
            ),
            "part_type": first_value_at(
                payload,
                &[
                    &["part", "type"],
                    &["properties", "part", "type"],
                ],
            ),
            "text_preview": text_preview,
        }),
    }
}

pub fn install_codex_hook(workspace: &Workspace) -> Result<TraceInstallProtocol> {
    let script_dir = workspace.debug_dir().join("adapters");
    fs::create_dir_all(&script_dir)?;
    let script_path = script_dir.join("codex-rive-trace-hook.sh");
    let content = r#"#!/bin/sh
# RIVE-MANAGED-CODEX-TRACE-HOOK
args="debug trace ingest --adapter codex-hook --stdin"
if [ -n "${RIVE_AGENT_ID:-}" ]; then args="$args --agent $RIVE_AGENT_ID"; fi
if [ -n "${RIVE_RUN_ID:-}" ]; then args="$args --run $RIVE_RUN_ID"; fi
if [ -n "${RIVE_DISPATCH_ID:-}" ]; then args="$args --dispatch $RIVE_DISPATCH_ID"; fi
rive $args >/dev/null 2>/dev/null || true
exit 0
"#;
    let script_status = write_managed_file(&script_path, content, "RIVE-MANAGED-CODEX-TRACE-HOOK")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)?;
    }
    let config_dir = workspace.root.join(".codex");
    fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join("hooks.json");
    let command = script_path.to_string_lossy();
    let config = serde_json::to_string_pretty(&json!({
        "_rive_managed": "RIVE-MANAGED-CODEX-TRACE-HOOKS",
        "hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": command}]}],
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": command}]}],
            "PreToolUse": [{"hooks": [{"type": "command", "command": command}]}],
            "PostToolUse": [{"hooks": [{"type": "command", "command": command}]}],
            "PermissionRequest": [{"hooks": [{"type": "command", "command": command}]}],
            "SubagentStart": [{"hooks": [{"type": "command", "command": command}]}],
            "SubagentStop": [{"hooks": [{"type": "command", "command": command}]}],
            "Stop": [{"hooks": [{"type": "command", "command": command}]}]
        }
    }))?;
    let config_status =
        write_managed_file(&config_path, &config, "RIVE-MANAGED-CODEX-TRACE-HOOKS")?;
    Ok(TraceInstallProtocol {
        target: "codex".to_string(),
        path: config_path.display().to_string(),
        status: format!("config:{config_status}; script:{script_status}"),
    })
}

pub fn install_opencode_plugin(workspace: &Workspace) -> Result<TraceInstallProtocol> {
    install_opencode_plugin_at(&workspace.root)
}

pub fn install_opencode_plugin_at(root: &Path) -> Result<TraceInstallProtocol> {
    let dir = root.join(".opencode").join("plugins");
    fs::create_dir_all(&dir)?;
    let path = dir.join("rive-trace.ts");
    let content = r#"// RIVE-MANAGED-OPENCODE-TRACE-PLUGIN
import { mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

function ingest(payload: unknown) {
  let dir: string | undefined
  try {
    dir = mkdtempSync(join(tmpdir(), "rive-opencode-trace-"))
    const payloadPath = join(dir, "payload.json")
    writeFileSync(payloadPath, JSON.stringify(payload))
    const args = ["debug", "trace", "ingest", "--adapter", "opencode-plugin", "--stdin"]
    if (process.env.RIVE_AGENT_ID) args.push("--agent", process.env.RIVE_AGENT_ID)
    if (process.env.RIVE_RUN_ID) args.push("--run", process.env.RIVE_RUN_ID)
    if (process.env.RIVE_DISPATCH_ID) args.push("--dispatch", process.env.RIVE_DISPATCH_ID)
    Bun.spawnSync(["rive", ...args], {
      stdin: Bun.file(payloadPath),
      stdout: "ignore",
      stderr: "ignore",
    })
  } catch (_) {
    // Debug trace must never alter OpenCode behavior.
  } finally {
    if (dir) {
      try {
        rmSync(dir, { recursive: true, force: true })
      } catch (_) {
        // Debug trace must never alter OpenCode behavior.
      }
    }
  }
}

export default async function RiveTracePlugin() {
  return {
    event: ({ event }: { event: unknown }) => {
      ingest(event)
    },
  }
}
"#;
    let status = write_managed_file(&path, content, "RIVE-MANAGED-OPENCODE-TRACE-PLUGIN")?;
    Ok(TraceInstallProtocol {
        target: "opencode".to_string(),
        path: path.display().to_string(),
        status,
    })
}

pub fn uninstall_managed(workspace: &Workspace, target: &str) -> Result<TraceInstallProtocol> {
    let (path, marker) = match target {
        "codex" => (
            workspace.root.join(".codex").join("hooks.json"),
            "RIVE-MANAGED-CODEX-TRACE-HOOKS",
        ),
        "opencode" => (
            workspace
                .root
                .join(".opencode")
                .join("plugins")
                .join("rive-trace.ts"),
            "RIVE-MANAGED-OPENCODE-TRACE-PLUGIN",
        ),
        _ => return Err(anyhow!("unsupported trace install target: {target}")),
    };
    let status = if path.exists() {
        let content = fs::read_to_string(&path)?;
        if content.contains(marker) {
            fs::remove_file(&path)?;
            "removed"
        } else {
            "skipped_existing_user_file"
        }
    } else {
        "skipped_missing"
    };
    if target == "codex" {
        let script_path = workspace
            .debug_dir()
            .join("adapters")
            .join("codex-rive-trace-hook.sh");
        if script_path.exists() {
            let content = fs::read_to_string(&script_path)?;
            if content.contains("RIVE-MANAGED-CODEX-TRACE-HOOK") {
                fs::remove_file(&script_path)?;
            }
        }
    }
    Ok(TraceInstallProtocol {
        target: target.to_string(),
        path: path.display().to_string(),
        status: status.to_string(),
    })
}

fn write_managed_file(path: &Path, content: &str, marker: &str) -> Result<String> {
    if path.exists() {
        let existing = fs::read_to_string(path)?;
        if !existing.contains(marker) {
            return Ok("skipped_existing_user_file".to_string());
        }
        if existing == content {
            return Ok("unchanged".to_string());
        }
    }
    fs::write(path, content)?;
    Ok("written".to_string())
}

fn write_payload_blob(workspace: &Workspace, sha: &str, bytes: &[u8]) -> Result<String> {
    let (prefix, rest) = sha.split_at(2);
    let dir = workspace.debug_trace_payloads_dir().join(prefix);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{rest}.json"));
    if !path.exists() {
        fs::write(&path, bytes)
            .with_context(|| format!("write trace payload {}", path.display()))?;
    }
    path_relative_to(&path, &workspace.root)
}

fn read_payload_blob(workspace: &Workspace, blob_ref: &str) -> Result<Value> {
    let path = workspace.root.join(blob_ref);
    let bytes =
        fs::read(&path).with_context(|| format!("read trace payload {}", path.display()))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn path_relative_to(path: &Path, root: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(root)
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))?
        .to_string_lossy()
        .to_string())
}

fn row_to_raw_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<DebugTraceRawRecord> {
    let received_at: String = row.get(7)?;
    Ok(DebugTraceRawRecord {
        raw_event_id: row.get(0)?,
        trace_session_id: row.get(1)?,
        workspace_id: row.get(2)?,
        adapter: row.get(3)?,
        external_event_type: row.get(4)?,
        external_event_id: row.get(5)?,
        sequence: row.get(6)?,
        received_at: parse_time_for_sql(&received_at)?,
        payload_hash: row.get(8)?,
        payload_blob_ref: row.get(9)?,
    })
}

fn row_to_trace_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<DebugTraceEventRecord> {
    let occurred_at: Option<String> = row.get(6)?;
    let summary_json: String = row.get(14)?;
    Ok(DebugTraceEventRecord {
        trace_event_id: row.get(0)?,
        raw_event_id: row.get(1)?,
        trace_session_id: row.get(2)?,
        workspace_id: row.get(3)?,
        adapter: row.get(4)?,
        event_kind: row.get(5)?,
        occurred_at: occurred_at.as_deref().map(parse_time_for_sql).transpose()?,
        sequence: row.get(7)?,
        agent_id: row.get(8)?,
        run_id: row.get(9)?,
        dispatch_id: row.get(10)?,
        external_session_id: row.get(11)?,
        external_turn_id: row.get(12)?,
        external_tool_id: row.get(13)?,
        summary: serde_json::from_str(&summary_json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                14,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })?,
    })
}

fn row_to_trace_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<DebugTraceSessionRecord> {
    let started_at: String = row.get(8)?;
    let ended_at: Option<String> = row.get(9)?;
    let metadata_json: String = row.get(10)?;
    Ok(DebugTraceSessionRecord {
        trace_session_id: row.get(0)?,
        workspace_id: row.get(1)?,
        adapter: row.get(2)?,
        external_session_id: row.get(3)?,
        agent_id: row.get(4)?,
        run_id: row.get(5)?,
        dispatch_id: row.get(6)?,
        cwd: row.get(7)?,
        started_at: parse_time_for_sql(&started_at)?,
        ended_at: ended_at.as_deref().map(parse_time_for_sql).transpose()?,
        metadata: serde_json::from_str(&metadata_json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })?,
    })
}

fn parse_optional_time(payload: &Value) -> Option<DateTime<Utc>> {
    string_at(payload, &["time"])
        .or_else(|| string_at(payload, &["timestamp"]))
        .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
        .map(|time| time.with_timezone(&Utc))
}

fn parse_time_from_paths(payload: &Value, paths: &[&[&str]]) -> Option<DateTime<Utc>> {
    for path in paths {
        let value = path.iter().try_fold(payload, |value, key| value.get(*key));
        match value {
            Some(Value::String(text)) => {
                if let Ok(time) = DateTime::parse_from_rfc3339(text) {
                    return Some(time.with_timezone(&Utc));
                }
            }
            Some(Value::Number(number)) => {
                if let Some(ms) = number.as_i64() {
                    if let Some(time) = DateTime::<Utc>::from_timestamp_millis(ms) {
                        return Some(time);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_time_for_sql(value: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })
}

fn value_at(payload: &Value, path: &[&str]) -> Value {
    path.iter()
        .try_fold(payload, |value, key| value.get(*key))
        .cloned()
        .unwrap_or(Value::Null)
}

fn first_value_at(payload: &Value, paths: &[&[&str]]) -> Value {
    paths
        .iter()
        .find_map(|path| {
            path.iter()
                .try_fold(payload, |value, key| value.get(*key))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

fn first_preview_at(payload: &Value, paths: &[&[&str]], max_chars: usize) -> Value {
    let value = first_value_at(payload, paths);
    match value {
        Value::Null => Value::Null,
        Value::String(text) => Value::String(truncate_for_summary(&text, max_chars)),
        other => Value::String(truncate_for_summary(&other.to_string(), max_chars)),
    }
}

fn string_at(payload: &Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(payload, |value, key| value.get(*key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn first_string_at(payload: &Value, paths: &[&[&str]]) -> Option<String> {
    paths.iter().find_map(|path| string_at(payload, path))
}

fn truncate_for_summary(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            output.push_str("...");
            return output;
        }
        output.push(ch);
    }
    output
}

fn int_at(payload: &Value, path: &[&str]) -> Option<i64> {
    path.iter()
        .try_fold(payload, |value, key| value.get(*key))
        .and_then(Value::as_i64)
}

fn workspace_id(workspace: &Workspace) -> String {
    workspace.root.display().to_string()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}
