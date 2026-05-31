use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::debug_trace::{install_opencode_plugin, DebugTraceStore, TraceListFilter};
use crate::dispatch::{
    agent_protocol, dispatch_protocol, AddAgentInput, CreateDispatchInput, CreateDispatchOutcome,
    DispatchService,
};
use crate::store::{AgentRecord, AgentRole, DispatchRecord, DispatchState, EventStore};
use crate::workspace::Workspace;

#[derive(Debug)]
pub struct OpenCodeRunnerInput {
    pub agent: String,
    pub title: String,
    pub command_id: String,
    pub agent_token: Option<String>,
    pub opencode_bin: Option<PathBuf>,
    pub timeout_seconds: u64,
    pub snapshot_paths: Vec<PathBuf>,
    pub task_body: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub struct RunnerResponseProtocol {
    pub runner: RunnerProtocol,
    pub agent: crate::dispatch::AgentProtocol,
    pub dispatch: crate::dispatch::DispatchProtocol,
    pub trace: RunnerTraceProtocol,
}

#[derive(Debug, Serialize)]
pub struct RunnerProtocol {
    pub kind: &'static str,
    pub run_id: String,
    pub opencode_bin: String,
    pub exit_code: Option<i32>,
    pub stdout_ref: String,
    pub stderr_ref: String,
    pub child_executed: bool,
}

#[derive(Debug, Serialize)]
pub struct RunnerTraceProtocol {
    pub adapter: &'static str,
    pub event_count: usize,
    pub session_ids: Vec<String>,
}

pub struct OpenCodeRunner<'a> {
    workspace: &'a Workspace,
    event_store: &'a EventStore,
    trace_store: &'a DebugTraceStore,
    blob_store: &'a crate::snapshot::LocalSnapshotStore<'a>,
}

impl<'a> OpenCodeRunner<'a> {
    pub fn new(
        workspace: &'a Workspace,
        event_store: &'a EventStore,
        trace_store: &'a DebugTraceStore,
        blob_store: &'a crate::snapshot::LocalSnapshotStore<'a>,
    ) -> Self {
        Self {
            workspace,
            event_store,
            trace_store,
            blob_store,
        }
    }

    pub fn run(&self, input: OpenCodeRunnerInput) -> Result<RunnerResponseProtocol> {
        if input.timeout_seconds == 0 {
            return Err(anyhow!("opencode timeout must be greater than zero"));
        }
        if input.task_body.is_empty() {
            return Err(anyhow!("runner task body is required"));
        }

        install_opencode_plugin(self.workspace)?;

        let (agent, token) = self.resolve_agent(&input)?;
        let service = DispatchService::new(self.workspace, self.event_store, self.blob_store);
        let dispatch_outcome = service.create_dispatch(CreateDispatchInput {
            command_id: input.command_id.clone(),
            target_agent: agent.agent_id.clone(),
            title: input.title.clone(),
            body: input.task_body.clone(),
        })?;
        let (dispatch, dispatch_idempotency, should_execute) = match dispatch_outcome {
            CreateDispatchOutcome::Inserted(dispatch) => (dispatch, "inserted", true),
            CreateDispatchOutcome::Replayed(dispatch) => (dispatch, "replayed", false),
        };

        let run_id = prefixed_id("run");
        let run_dir = self.workspace.debug_runs_dir().join(&run_id);
        fs::create_dir_all(&run_dir)?;
        let stdout_path = run_dir.join("stdout.jsonl");
        let stderr_path = run_dir.join("stderr.log");

        let opencode_bin = resolve_opencode_bin(input.opencode_bin.as_deref())?;
        let prompt = build_prompt(
            self.workspace,
            &agent,
            &dispatch,
            &input.title,
            &input.task_body,
            &input.snapshot_paths,
        );
        fs::write(run_dir.join("prompt.txt"), &prompt)?;

        let mut exit_code = None;
        if should_execute {
            let process_input = OpenCodeProcessInput {
                opencode_bin: &opencode_bin,
                workspace: self.workspace,
                agent: &agent,
                token: &token,
                dispatch: &dispatch,
                run_id: &run_id,
                prompt: &prompt,
                timeout_seconds: input.timeout_seconds,
            };
            let output = run_opencode_process(process_input)?;
            exit_code = output.exit_code;
            fs::write(&stdout_path, &output.stdout)?;
            fs::write(&stderr_path, &output.stderr)?;
            if output.timed_out {
                return Err(anyhow!("opencode timeout"));
            }
            if output.exit_code.unwrap_or(1) != 0 {
                return Err(anyhow!("opencode exit failed: {:?}", output.exit_code));
            }
        } else {
            fs::write(&stdout_path, b"")?;
            fs::write(&stderr_path, b"")?;
        }

        let dispatch = self
            .event_store
            .get_dispatch(&dispatch.dispatch_id)?
            .ok_or_else(|| anyhow!("dispatch not found: {}", dispatch.dispatch_id))?;
        if matches!(
            dispatch.state,
            DispatchState::Open | DispatchState::Cancelled
        ) {
            return Err(anyhow!("dispatch not reported: {}", dispatch.dispatch_id));
        }

        let trace = self.trace_summary(&run_id, &dispatch.dispatch_id)?;
        Ok(RunnerResponseProtocol {
            runner: RunnerProtocol {
                kind: "opencode",
                run_id,
                opencode_bin: opencode_bin.display().to_string(),
                exit_code,
                stdout_ref: path_relative_to(&stdout_path, &self.workspace.root)?,
                stderr_ref: path_relative_to(&stderr_path, &self.workspace.root)?,
                child_executed: should_execute,
            },
            agent: agent_protocol(&agent),
            dispatch: dispatch_protocol(&dispatch, dispatch_idempotency),
            trace,
        })
    }

    fn resolve_agent(&self, input: &OpenCodeRunnerInput) -> Result<(AgentRecord, String)> {
        if let Some(agent) = self.event_store.get_agent(&input.agent)? {
            let token = input
                .agent_token
                .clone()
                .ok_or_else(|| anyhow!("runner agent token required"))?;
            if agent.token_hash != token_hash(&token) {
                return Err(anyhow!("invalid agent token"));
            }
            if agent.role != AgentRole::Worker {
                return Err(anyhow!("runner agent must be worker"));
            }
            return Ok((agent, token));
        }

        let service = DispatchService::new(self.workspace, self.event_store, self.blob_store);
        let outcome = service.add_agent(AddAgentInput {
            name: input.agent.clone(),
            role: AgentRole::Worker,
            token: None,
        })?;
        Ok((outcome.agent, outcome.token))
    }

    fn trace_summary(&self, run_id: &str, dispatch_id: &str) -> Result<RunnerTraceProtocol> {
        let mut events = self.trace_store.list_events(TraceListFilter {
            adapter: Some("opencode-plugin".to_string()),
            agent_id: None,
            dispatch_id: Some(dispatch_id.to_string()),
            trace_session_id: None,
        })?;
        events.retain(|event| event.run_id.as_deref() == Some(run_id));
        let session_ids = events
            .iter()
            .filter_map(|event| event.trace_session_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(RunnerTraceProtocol {
            adapter: "opencode-plugin",
            event_count: events.len(),
            session_ids,
        })
    }
}

#[derive(Debug)]
struct ProcessOutput {
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

struct OpenCodeProcessInput<'a> {
    opencode_bin: &'a Path,
    workspace: &'a Workspace,
    agent: &'a AgentRecord,
    token: &'a str,
    dispatch: &'a DispatchRecord,
    run_id: &'a str,
    prompt: &'a str,
    timeout_seconds: u64,
}

fn run_opencode_process(input: OpenCodeProcessInput<'_>) -> Result<ProcessOutput> {
    let bin_dir = std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("cannot resolve current executable directory"))?;
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", bin_dir.display(), old_path.to_string_lossy());

    let mut child = Command::new(input.opencode_bin)
        .current_dir(&input.workspace.root)
        .arg("run")
        .arg("--format")
        .arg("json")
        .arg("--dangerously-skip-permissions")
        .arg(input.prompt)
        .env("RIVE_WORKSPACE", &input.workspace.root)
        .env("RIVE_AGENT_ID", &input.agent.agent_id)
        .env("RIVE_AGENT_TOKEN", input.token)
        .env("RIVE_RUN_ID", input.run_id)
        .env("RIVE_DISPATCH_ID", &input.dispatch.dispatch_id)
        .env("PATH", path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                anyhow!("opencode not found")
            } else {
                anyhow!("opencode launch failed: {err}")
            }
        })?;

    let started = Instant::now();
    let timeout = Duration::from_secs(input.timeout_seconds);
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            return Ok(ProcessOutput {
                exit_code: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
                timed_out: false,
            });
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let output = child.wait_with_output()?;
            return Ok(ProcessOutput {
                exit_code: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
                timed_out: true,
            });
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn resolve_opencode_bin(opencode_bin: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = opencode_bin {
        if !path.exists() {
            return Err(anyhow!("opencode not found: {}", path.display()));
        }
        return Ok(path.to_path_buf());
    }
    Ok(PathBuf::from("opencode"))
}

pub fn build_prompt(
    workspace: &Workspace,
    agent: &AgentRecord,
    dispatch: &DispatchRecord,
    title: &str,
    body: &[u8],
    snapshot_paths: &[PathBuf],
) -> String {
    let body = String::from_utf8_lossy(body);
    let mut snapshot_instructions = String::new();
    if snapshot_paths.is_empty() {
        snapshot_instructions
            .push_str("Capture evidence for the files you create or modify before reporting.\n");
    } else {
        snapshot_instructions.push_str("Suggested evidence capture commands:\n");
        for path in snapshot_paths {
            let _ = writeln!(
                snapshot_instructions,
                "- rive snapshot capture --path {} --label opencode-runner-result",
                path.display()
            );
        }
    }

    format!(
        r#"You are running inside a Rive dispatch.

Workspace:
- root: {workspace}

Agent:
- id: {agent_id}
- name: {agent_name}
- role: worker

Dispatch:
- id: {dispatch_id}
- title: {title}

Rive protocol:
- Use `team status --dispatch {dispatch_id} --snapshot <snapshot_id> --command-id <unique-id> --stdin` for progress updates. Status does not close the dispatch.
- Use `team report --dispatch {dispatch_id} --status done|blocked|failed --snapshot <snapshot_id> --command-id <unique-id> --stdin` to close, block, or fail the dispatch.
- Before report, capture evidence with `rive snapshot capture`.
- A natural language final answer is not a Rive report.

{snapshot_instructions}
Task:
{body}
"#,
        workspace = workspace.root.display(),
        agent_id = agent.agent_id,
        agent_name = agent.name,
        dispatch_id = dispatch.dispatch_id,
        title = title,
        snapshot_instructions = snapshot_instructions,
        body = body,
    )
}

fn token_hash(token: &str) -> String {
    format!("sha256:{}", sha256_hex(token.as_bytes()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn path_relative_to(path: &Path, root: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(root)
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))?
        .to_string_lossy()
        .to_string())
}
