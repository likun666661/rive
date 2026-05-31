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
    let event_kind = match external_event_type.as_str() {
        "session.created" => "session_started",
        "session.status" | "session.updated" => "session_status_changed",
        "session.idle" => "session_idle",
        "session.error" => "session_error",
        "message.updated" => "assistant_output",
        "message.part.updated" => "assistant_output_delta",
        "tool.execute.before" => "tool_call_started",
        "tool.execute.after" => "tool_call_completed",
        "permission.asked" => "permission_requested",
        "permission.replied" => "permission_resolved",
        "command.executed" => "command_executed",
        "session.diff" => "file_changed",
        _ => "unknown",
    };
    let session_id =
        string_at(payload, &["session", "id"]).or_else(|| string_at(payload, &["sessionID"]));
    NormalizedEvent {
        external_event_type,
        external_event_id: string_at(payload, &["id"])
            .or_else(|| string_at(payload, &["event_id"])),
        event_kind: event_kind.to_string(),
        occurred_at: parse_optional_time(payload),
        agent_id: string_at(payload, &["agent_id"]).or_else(|| input.agent_id.clone()),
        run_id: string_at(payload, &["run_id"]).or_else(|| input.run_id.clone()),
        dispatch_id: string_at(payload, &["dispatch_id"]).or_else(|| input.dispatch_id.clone()),
        external_session_id: session_id,
        external_turn_id: string_at(payload, &["message", "id"])
            .or_else(|| string_at(payload, &["part", "id"])),
        external_tool_id: string_at(payload, &["tool", "id"]),
        cwd: string_at(payload, &["cwd"]),
        summary: json!({
            "type": value_at(payload, &["type"]),
            "tool_name": value_at(payload, &["tool", "name"]),
            "session_status": value_at(payload, &["session", "status"]),
        }),
    }
}

pub fn install_codex_hook(workspace: &Workspace) -> Result<TraceInstallProtocol> {
    let dir = workspace.debug_dir().join("adapters");
    fs::create_dir_all(&dir)?;
    let path = dir.join("codex-rive-trace-hook.sh");
    let content = "#!/bin/sh\n# RIVE-MANAGED-CODEX-TRACE-HOOK\nrive debug trace ingest --adapter codex-hook --stdin >/dev/null 2>/dev/null || true\nexit 0\n";
    let status = write_managed_file(&path, content, "RIVE-MANAGED-CODEX-TRACE-HOOK")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms)?;
    }
    Ok(TraceInstallProtocol {
        target: "codex".to_string(),
        path: path.display().to_string(),
        status,
    })
}

pub fn install_opencode_plugin(workspace: &Workspace) -> Result<TraceInstallProtocol> {
    let dir = workspace.root.join(".opencode").join("plugins");
    fs::create_dir_all(&dir)?;
    let path = dir.join("rive-trace.ts");
    let content = r#"// RIVE-MANAGED-OPENCODE-TRACE-PLUGIN
const encoder = new TextEncoder()

async function ingest(payload: unknown) {
  try {
    const proc = new Deno.Command("rive", {
      args: ["debug", "trace", "ingest", "--adapter", "opencode-plugin", "--stdin"],
      stdin: "piped",
      stdout: "null",
      stderr: "null",
    }).spawn()
    const writer = proc.stdin.getWriter()
    await writer.write(encoder.encode(JSON.stringify(payload)))
    await writer.close()
    await proc.status
  } catch (_) {
    // Debug trace must never alter OpenCode behavior.
  }
}

export default async function RiveTracePlugin() {
  return {
    event: async ({ event }: { event: unknown }) => {
      await ingest(event)
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
            workspace
                .debug_dir()
                .join("adapters")
                .join("codex-rive-trace-hook.sh"),
            "RIVE-MANAGED-CODEX-TRACE-HOOK",
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

fn string_at(payload: &Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(payload, |value, key| value.get(*key))
        .and_then(Value::as_str)
        .map(str::to_string)
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
