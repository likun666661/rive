use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::debug_trace::{
    install_codex_hook, install_opencode_plugin, DebugTraceStore, TraceListFilter,
};
use crate::dispatch::{
    agent_protocol, dispatch_protocol, AddAgentInput, CreateDispatchInput, CreateDispatchOutcome,
    DispatchService,
};
use crate::facts::ActorEnv;
use crate::store::{
    AgentRecord, AgentRole, CompleteDelegationInput, DelegationRecord, DispatchRecord,
    DispatchState, EventStore, IdempotencyResolution, InsertAgentRunInput, InsertDelegationInput,
};
use crate::work::{
    BindWorkDispatchCommand, BindWorkRootCommand, CreateWorkNodeInput, WorkNodeKind,
    WorkProjectionProtocol, WorkService,
};
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

#[derive(Debug)]
pub struct CodexRunnerInput {
    pub agent: String,
    pub title: String,
    pub command_id: String,
    pub agent_token: Option<String>,
    pub codex_bin: Option<PathBuf>,
    pub timeout_seconds: u64,
    pub snapshot_paths: Vec<PathBuf>,
    pub task_body: Vec<u8>,
    pub trust_project: bool,
}

#[derive(Debug)]
pub struct OrchestratorRunnerInput {
    pub runner: String,
    pub agent: String,
    pub command_id: String,
    pub agent_token: Option<String>,
    pub opencode_bin: Option<PathBuf>,
    pub timeout_seconds: u64,
    pub workers: Vec<String>,
    pub acceptance_command: Option<String>,
    pub objective: Vec<u8>,
}

#[derive(Debug)]
struct RunnerInput {
    agent: String,
    title: String,
    command_id: String,
    agent_token: Option<String>,
    binary: Option<PathBuf>,
    timeout_seconds: u64,
    snapshot_paths: Vec<PathBuf>,
    task_body: Vec<u8>,
    trust_project: bool,
}

impl From<OpenCodeRunnerInput> for RunnerInput {
    fn from(input: OpenCodeRunnerInput) -> Self {
        Self {
            agent: input.agent,
            title: input.title,
            command_id: input.command_id,
            agent_token: input.agent_token,
            binary: input.opencode_bin,
            timeout_seconds: input.timeout_seconds,
            snapshot_paths: input.snapshot_paths,
            task_body: input.task_body,
            trust_project: false,
        }
    }
}

impl From<CodexRunnerInput> for RunnerInput {
    fn from(input: CodexRunnerInput) -> Self {
        Self {
            agent: input.agent,
            title: input.title,
            command_id: input.command_id,
            agent_token: input.agent_token,
            binary: input.codex_bin,
            timeout_seconds: input.timeout_seconds,
            snapshot_paths: input.snapshot_paths,
            task_body: input.task_body,
            trust_project: input.trust_project,
        }
    }
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
    pub binary: String,
    pub opencode_bin: Option<String>,
    pub codex_bin: Option<String>,
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

#[derive(Debug)]
pub struct TeamSendInput {
    pub actor: ActorEnv,
    pub work_node_id: Option<String>,
    pub target: String,
    pub runner: String,
    pub title: String,
    pub command_id: String,
    pub opencode_bin: Option<PathBuf>,
    pub codex_bin: Option<PathBuf>,
    pub timeout_seconds: u64,
    pub snapshot_paths: Vec<PathBuf>,
    pub task_body: Vec<u8>,
    pub wait: bool,
    pub trust_project: bool,
}

#[derive(Debug, Serialize)]
pub struct TeamSendResponseProtocol {
    pub ok: bool,
    pub action: &'static str,
    pub command_id: String,
    pub child_executed: bool,
    pub expected_next_action: &'static str,
    pub delegation: DelegationProtocol,
    pub dispatch: crate::dispatch::DispatchProtocol,
    pub work: Option<WorkProjectionProtocol>,
    pub trace: RunnerTraceProtocol,
}

#[derive(Debug, Serialize)]
pub struct OrchestratorRunnerResponseProtocol {
    pub runner: OrchestratorRunnerProtocol,
    pub agent: crate::dispatch::AgentProtocol,
    pub root_work: WorkProjectionProtocol,
    pub workers: Vec<crate::dispatch::AgentProtocol>,
    pub trace: RunnerTraceProtocol,
}

#[derive(Debug, Serialize)]
pub struct OrchestratorRunnerProtocol {
    pub kind: &'static str,
    pub adapter: &'static str,
    pub run_id: String,
    pub command_id: String,
    pub root_work_node_id: String,
    pub binary: String,
    pub opencode_bin: Option<String>,
    pub exit_code: Option<i32>,
    pub stdout_ref: String,
    pub stderr_ref: String,
    pub child_executed: bool,
    pub idempotency_status: &'static str,
    pub audit: WorkspaceAuditProtocol,
    pub usage_summary: Option<crate::debug_trace::TraceUsageTotals>,
}

#[derive(Debug, Serialize, Clone)]
pub struct WorkspaceAuditProtocol {
    pub checked: bool,
    pub changed_paths: Vec<String>,
    pub allowed_paths: Vec<String>,
    pub denied_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DelegationProtocol {
    pub command_id: String,
    pub source_agent_id: String,
    pub source_run_id: Option<String>,
    pub target_agent_id: String,
    pub worker_run_id: String,
    pub dispatch_id: String,
    pub runner: String,
    pub state: String,
    pub child_executed: bool,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub idempotency_status: &'static str,
}

pub struct OpenCodeRunner<'a> {
    core: RunnerCore<'a, OpenCodeAdapter>,
}

impl<'a> OpenCodeRunner<'a> {
    pub fn new(
        workspace: &'a Workspace,
        event_store: &'a EventStore,
        trace_store: &'a DebugTraceStore,
        blob_store: &'a crate::snapshot::LocalSnapshotStore<'a>,
    ) -> Self {
        Self {
            core: RunnerCore::new(
                workspace,
                event_store,
                trace_store,
                blob_store,
                OpenCodeAdapter,
            ),
        }
    }

    pub fn run(&self, input: OpenCodeRunnerInput) -> Result<RunnerResponseProtocol> {
        self.core.run(input.into())
    }
}

pub struct CodexRunner<'a> {
    core: RunnerCore<'a, CodexAdapter>,
}

impl<'a> CodexRunner<'a> {
    pub fn new(
        workspace: &'a Workspace,
        event_store: &'a EventStore,
        trace_store: &'a DebugTraceStore,
        blob_store: &'a crate::snapshot::LocalSnapshotStore<'a>,
    ) -> Self {
        Self {
            core: RunnerCore::new(
                workspace,
                event_store,
                trace_store,
                blob_store,
                CodexAdapter,
            ),
        }
    }

    pub fn run(&self, input: CodexRunnerInput) -> Result<RunnerResponseProtocol> {
        self.core.run(input.into())
    }
}

pub struct OrchestratorRunner<'a> {
    workspace: &'a Workspace,
    event_store: &'a EventStore,
    trace_store: &'a DebugTraceStore,
    blob_store: &'a crate::snapshot::LocalSnapshotStore<'a>,
}

impl<'a> OrchestratorRunner<'a> {
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

    pub fn run(
        &self,
        input: OrchestratorRunnerInput,
    ) -> Result<OrchestratorRunnerResponseProtocol> {
        if input.runner != "opencode" {
            return Err(anyhow!("runner not supported: {}", input.runner));
        }
        if input.timeout_seconds == 0 {
            return Err(anyhow!("opencode timeout must be greater than zero"));
        }
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        if input.objective.is_empty() {
            return Err(anyhow!("orchestrator objective is required"));
        }
        if input.workers.is_empty() {
            return Err(anyhow!("orchestrator worker is required"));
        }

        install_opencode_plugin(self.workspace)?;
        let (orchestrator, token) = self.resolve_orchestrator(&input)?;
        let workers = self.resolve_workers(&input.workers)?;
        let root_body =
            orchestrator_root_body(&input.objective, &workers, &input.acceptance_command);
        let work_service = WorkService::new(self.workspace, self.event_store, self.blob_store);
        let root_command_id = format!("orchestrator:{}:root", input.command_id);
        let (root, root_idempotency) = work_service.create_node(CreateWorkNodeInput {
            command_id: root_command_id,
            kind: WorkNodeKind::Objective,
            title: format!("orchestrator objective {}", input.command_id),
            body: root_body,
        })?;
        let should_execute = root_idempotency == "inserted";
        let run_id = if should_execute {
            prefixed_id("run")
        } else {
            format!("replay-{}", root.work_node_id)
        };
        work_service.bind_root(BindWorkRootCommand {
            root_work_node_id: root.work_node_id.clone(),
            work_node_id: root.work_node_id.clone(),
            created_by_agent_id: Some(orchestrator.agent_id.clone()),
            created_by_run_id: Some(run_id.clone()),
        })?;
        let run_dir = self.workspace.debug_runs_dir().join(&run_id);
        fs::create_dir_all(&run_dir)?;
        let stdout_path = run_dir.join("stdout.jsonl");
        let stderr_path = run_dir.join("stderr.log");
        let planner_bin = run_dir.join("planner-bin");
        prepare_planner_bin(&planner_bin)?;
        let binary = OpenCodeAdapter.resolve_binary(input.opencode_bin.as_deref())?;
        let prompt = build_orchestrator_prompt(
            self.workspace,
            &input.objective,
            &root.work_node_id,
            &workers,
            input.acceptance_command.as_deref(),
        );
        fs::write(run_dir.join("prompt.txt"), &prompt)?;

        let mut exit_code = None;
        let mut audit = WorkspaceAuditProtocol {
            checked: false,
            changed_paths: Vec::new(),
            allowed_paths: Vec::new(),
            denied_paths: Vec::new(),
        };
        if should_execute {
            let baseline = WorkspaceMutationBaseline::capture(self.workspace)?;
            self.event_store.insert_agent_run(&InsertAgentRunInput {
                run_id: run_id.clone(),
                agent_id: orchestrator.agent_id.clone(),
                token_hash: token_hash(&token),
                created_at: Utc::now(),
            })?;
            let mut command = Command::new(&binary);
            command
                .current_dir(&self.workspace.root)
                .arg("run")
                .arg("--format")
                .arg("json")
                .arg("--dangerously-skip-permissions")
                .arg(&prompt);
            apply_orchestrator_env(
                &mut command,
                OrchestratorEnvInput {
                    workspace: self.workspace,
                    agent: &orchestrator,
                    token: &token,
                    run_id: &run_id,
                    root_work_node_id: &root.work_node_id,
                    workers: &workers,
                    planner_bin: &planner_bin,
                },
            );
            let output = run_child_process(&mut command, input.timeout_seconds)?;
            exit_code = output.exit_code;
            fs::write(&stdout_path, &output.stdout)?;
            fs::write(&stderr_path, &output.stderr)?;
            if output.timed_out {
                return Err(anyhow!("opencode timeout"));
            }
            if output.exit_code.unwrap_or(1) != 0 {
                return Err(anyhow!("opencode exit failed: {:?}", output.exit_code));
            }
            audit = audit_workspace_mutation(self.workspace, self.event_store, &baseline)?;
            if !audit.denied_paths.is_empty() {
                return Err(anyhow!(
                    "orchestrator workspace mutation: {}",
                    audit.denied_paths.join(",")
                ));
            }
        } else {
            fs::write(&stdout_path, b"")?;
            fs::write(&stderr_path, b"")?;
        }

        let graph = work_service.inspect_graph(&root.work_node_id)?;
        if graph.hygiene_state != "clean" {
            return Err(anyhow!("work graph not closed: {}", root.work_node_id));
        }
        let root_projection = work_service.inspect_projection(&root.work_node_id)?;
        if root_projection.state != "done" {
            return Err(anyhow!(
                "work not done: root {} is {}",
                root.work_node_id,
                root_projection.state
            ));
        }
        let trace = self.trace_summary(&run_id)?;
        let usage_summary = crate::debug_trace::usage_for_workspace(
            self.workspace,
            self.trace_store,
            crate::debug_trace::TraceUsageFilter {
                run_id: Some(run_id.clone()),
                ..Default::default()
            },
        )
        .ok()
        .map(|usage| usage.totals);
        let binary = binary.display().to_string();
        Ok(OrchestratorRunnerResponseProtocol {
            runner: OrchestratorRunnerProtocol {
                kind: "orchestrator",
                adapter: "opencode",
                run_id,
                command_id: input.command_id,
                root_work_node_id: root.work_node_id,
                binary: binary.clone(),
                opencode_bin: Some(binary),
                exit_code,
                stdout_ref: path_relative_to(&stdout_path, &self.workspace.root)?,
                stderr_ref: path_relative_to(&stderr_path, &self.workspace.root)?,
                child_executed: should_execute,
                idempotency_status: root_idempotency,
                audit,
                usage_summary,
            },
            agent: agent_protocol(&orchestrator),
            root_work: root_projection,
            workers: workers.iter().map(agent_protocol).collect(),
            trace,
        })
    }

    fn resolve_orchestrator(
        &self,
        input: &OrchestratorRunnerInput,
    ) -> Result<(AgentRecord, String)> {
        if let Some(agent) = self.event_store.get_agent(&input.agent)? {
            let token = input
                .agent_token
                .clone()
                .ok_or_else(|| anyhow!("runner agent token required"))?;
            if agent.token_hash != token_hash(&token) {
                return Err(anyhow!("invalid agent token"));
            }
            if agent.role != AgentRole::Orchestrator {
                return Err(anyhow!("runner agent must be orchestrator"));
            }
            return Ok((agent, token));
        }
        let service = DispatchService::new(self.workspace, self.event_store, self.blob_store);
        let outcome = service.add_agent(AddAgentInput {
            name: input.agent.clone(),
            role: AgentRole::Orchestrator,
            token: None,
        })?;
        Ok((outcome.agent, outcome.token))
    }

    fn resolve_workers(&self, workers: &[String]) -> Result<Vec<AgentRecord>> {
        let service = DispatchService::new(self.workspace, self.event_store, self.blob_store);
        let mut records = Vec::new();
        for worker in workers {
            if let Some(agent) = self.event_store.get_agent(worker)? {
                if agent.role != AgentRole::Worker {
                    return Err(anyhow!("runner worker must be worker: {}", worker));
                }
                records.push(agent);
            } else {
                records.push(
                    service
                        .add_agent(AddAgentInput {
                            name: worker.clone(),
                            role: AgentRole::Worker,
                            token: None,
                        })?
                        .agent,
                );
            }
        }
        Ok(records)
    }

    fn trace_summary(&self, run_id: &str) -> Result<RunnerTraceProtocol> {
        let mut events = self.trace_store.list_events(TraceListFilter {
            adapter: Some("opencode-plugin".to_string()),
            agent_id: None,
            dispatch_id: None,
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

struct RunnerCore<'a, A: RunnerAdapter> {
    workspace: &'a Workspace,
    event_store: &'a EventStore,
    trace_store: &'a DebugTraceStore,
    blob_store: &'a crate::snapshot::LocalSnapshotStore<'a>,
    adapter: A,
}

impl<'a, A: RunnerAdapter> RunnerCore<'a, A> {
    fn new(
        workspace: &'a Workspace,
        event_store: &'a EventStore,
        trace_store: &'a DebugTraceStore,
        blob_store: &'a crate::snapshot::LocalSnapshotStore<'a>,
        adapter: A,
    ) -> Self {
        Self {
            workspace,
            event_store,
            trace_store,
            blob_store,
            adapter,
        }
    }

    fn run(&self, input: RunnerInput) -> Result<RunnerResponseProtocol> {
        if input.timeout_seconds == 0 {
            return Err(anyhow!(
                "{} timeout must be greater than zero",
                self.adapter.kind()
            ));
        }
        if input.task_body.is_empty() {
            return Err(anyhow!("runner task body is required"));
        }

        self.adapter.install_trace(self.workspace)?;

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
        let stdout_path = run_dir.join(self.adapter.stdout_file_name());
        let stderr_path = run_dir.join("stderr.log");

        let binary = self.adapter.resolve_binary(input.binary.as_deref())?;
        let prompt = self.adapter.build_prompt(RunnerPromptContext {
            workspace: self.workspace,
            agent: &agent,
            dispatch: &dispatch,
            title: &input.title,
            body: &input.task_body,
            snapshot_paths: &input.snapshot_paths,
        });
        fs::write(run_dir.join("prompt.txt"), &prompt)?;

        let mut exit_code = None;
        if should_execute {
            let mut command = self.adapter.build_command(RunnerProcessInput {
                binary: &binary,
                workspace: self.workspace,
                run_dir: &run_dir,
                agent: &agent,
                token: &token,
                dispatch: &dispatch,
                run_id: &run_id,
                prompt: &prompt,
                trust_project: input.trust_project,
            })?;
            let output = run_child_process(&mut command, input.timeout_seconds)?;
            exit_code = output.exit_code;
            fs::write(&stdout_path, &output.stdout)?;
            fs::write(&stderr_path, &output.stderr)?;
            if output.timed_out {
                return Err(anyhow!("{} timeout", self.adapter.kind()));
            }
            if output.exit_code.unwrap_or(1) != 0 {
                return Err(anyhow!(
                    "{} exit failed: {:?}",
                    self.adapter.kind(),
                    output.exit_code
                ));
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
        let binary = binary.display().to_string();
        Ok(RunnerResponseProtocol {
            runner: RunnerProtocol {
                kind: self.adapter.kind(),
                run_id,
                binary: binary.clone(),
                opencode_bin: (self.adapter.kind() == "opencode").then(|| binary.clone()),
                codex_bin: (self.adapter.kind() == "codex").then(|| binary.clone()),
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

    fn run_existing(
        &self,
        input: ExistingRunnerInput<'_>,
    ) -> Result<(DispatchRecord, RunnerProtocol, RunnerTraceProtocol)> {
        self.adapter.install_trace(self.workspace)?;
        let run_dir = self.workspace.debug_runs_dir().join(input.run_id);
        fs::create_dir_all(&run_dir)?;
        let stdout_path = run_dir.join(self.adapter.stdout_file_name());
        let stderr_path = run_dir.join("stderr.log");
        let binary = self.adapter.resolve_binary(input.binary)?;
        let prompt = self.adapter.build_prompt(RunnerPromptContext {
            workspace: self.workspace,
            agent: input.agent,
            dispatch: input.dispatch,
            title: input.title,
            body: input.task_body,
            snapshot_paths: input.snapshot_paths,
        });
        fs::write(run_dir.join("prompt.txt"), &prompt)?;

        let mut command = self.adapter.build_command(RunnerProcessInput {
            binary: &binary,
            workspace: self.workspace,
            run_dir: &run_dir,
            agent: input.agent,
            token: input.token,
            dispatch: input.dispatch,
            run_id: input.run_id,
            prompt: &prompt,
            trust_project: input.trust_project,
        })?;
        let output = run_child_process(&mut command, input.timeout_seconds)?;
        fs::write(&stdout_path, &output.stdout)?;
        fs::write(&stderr_path, &output.stderr)?;
        if output.timed_out {
            return Err(anyhow!("{} timeout", self.adapter.kind()));
        }
        if output.exit_code.unwrap_or(1) != 0 {
            return Err(anyhow!(
                "{} exit failed: {:?}",
                self.adapter.kind(),
                output.exit_code
            ));
        }

        let dispatch = self
            .event_store
            .get_dispatch(&input.dispatch.dispatch_id)?
            .ok_or_else(|| anyhow!("dispatch not found: {}", input.dispatch.dispatch_id))?;
        if matches!(
            dispatch.state,
            DispatchState::Open | DispatchState::Cancelled
        ) {
            return Err(anyhow!("dispatch not reported: {}", dispatch.dispatch_id));
        }
        let trace = self.trace_summary(input.run_id, &dispatch.dispatch_id)?;
        let binary = binary.display().to_string();
        Ok((
            dispatch,
            RunnerProtocol {
                kind: self.adapter.kind(),
                run_id: input.run_id.to_string(),
                binary: binary.clone(),
                opencode_bin: (self.adapter.kind() == "opencode").then(|| binary.clone()),
                codex_bin: (self.adapter.kind() == "codex").then(|| binary.clone()),
                exit_code: output.exit_code,
                stdout_ref: path_relative_to(&stdout_path, &self.workspace.root)?,
                stderr_ref: path_relative_to(&stderr_path, &self.workspace.root)?,
                child_executed: true,
            },
            trace,
        ))
    }

    fn resolve_agent(&self, input: &RunnerInput) -> Result<(AgentRecord, String)> {
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
            adapter: Some(self.adapter.trace_adapter().to_string()),
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
            adapter: self.adapter.trace_adapter(),
            event_count: events.len(),
            session_ids,
        })
    }
}

struct ExistingRunnerInput<'a> {
    agent: &'a AgentRecord,
    token: &'a str,
    dispatch: &'a DispatchRecord,
    run_id: &'a str,
    title: &'a str,
    task_body: &'a [u8],
    snapshot_paths: &'a [PathBuf],
    binary: Option<&'a Path>,
    timeout_seconds: u64,
    trust_project: bool,
}

pub struct TeamSendService<'a> {
    workspace: &'a Workspace,
    event_store: &'a EventStore,
    trace_store: &'a DebugTraceStore,
    blob_store: &'a crate::snapshot::LocalSnapshotStore<'a>,
}

impl<'a> TeamSendService<'a> {
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

    pub fn send(&self, input: TeamSendInput) -> Result<TeamSendResponseProtocol> {
        if !input.wait {
            return Err(anyhow!("wait required"));
        }
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        if input.title.trim().is_empty() {
            return Err(anyhow!("missing dispatch title"));
        }
        if input.task_body.is_empty() {
            return Err(anyhow!("dispatch body is required"));
        }
        if input.timeout_seconds == 0 {
            return Err(anyhow!("runner timeout must be greater than zero"));
        }

        let source = self.authenticate_actor(&input.actor)?;
        if source.role != AgentRole::Orchestrator {
            return Err(anyhow!("agent role not allowed"));
        }
        let target = self
            .event_store
            .get_agent(&input.target)?
            .ok_or_else(|| anyhow!("target agent not found: {}", input.target))?;
        if target.role != AgentRole::Worker {
            return Err(anyhow!("target role invalid"));
        }
        let runner = RunnerKind::parse(&input.runner)?;
        let request_hash = team_send_request_hash(&input, &source, &target, runner.as_str());
        let work_service = WorkService::new(self.workspace, self.event_store, self.blob_store);

        if let Some(existing) = self
            .event_store
            .get_delegation_by_command_id(&input.command_id)?
        {
            if existing.source_agent_id != source.agent_id
                || existing.source_run_id != input.actor.run_id
                || existing.target_agent_id != target.agent_id
                || existing.runner != runner.as_str()
                || existing.request_hash != request_hash
            {
                return Err(anyhow!("idempotency conflict"));
            }
            let dispatch = self
                .event_store
                .get_dispatch(&existing.dispatch_id)?
                .ok_or_else(|| anyhow!("dispatch not found: {}", existing.dispatch_id))?;
            if matches!(
                dispatch.state,
                DispatchState::Open | DispatchState::Cancelled
            ) {
                return Err(anyhow!("dispatch not reported: {}", dispatch.dispatch_id));
            }
            let trace =
                self.trace_summary_for(runner, &existing.worker_run_id, &dispatch.dispatch_id)?;
            let work = work_projection_for_dispatch(&work_service, &dispatch.dispatch_id)?;
            return Ok(self.response(existing, dispatch, "replayed", false, trace, work));
        }

        if let Some(work_node_id) = &input.work_node_id {
            let projection = work_service.inspect_projection(work_node_id)?;
            if !projection.allowed_next_actions.contains(&"delegate") {
                return Err(anyhow!(
                    "work node not ready: {work_node_id} is {}",
                    projection.state
                ));
            }
        }

        let worker_run_id = prefixed_id("run");
        let worker_token = prefixed_id("tok");
        self.event_store.insert_agent_run(&InsertAgentRunInput {
            run_id: worker_run_id.clone(),
            agent_id: target.agent_id.clone(),
            token_hash: token_hash(&worker_token),
            created_at: Utc::now(),
        })?;

        let service = DispatchService::new(self.workspace, self.event_store, self.blob_store);
        let dispatch_command_id = format!("team-send:{}:dispatch", input.command_id);
        let dispatch = match service.create_dispatch(CreateDispatchInput {
            command_id: dispatch_command_id,
            target_agent: target.agent_id.clone(),
            title: input.title.clone(),
            body: input.task_body.clone(),
        })? {
            CreateDispatchOutcome::Inserted(dispatch)
            | CreateDispatchOutcome::Replayed(dispatch) => dispatch,
        };
        if let Some(work_node_id) = &input.work_node_id {
            work_service.bind_dispatch(BindWorkDispatchCommand {
                work_node_id: work_node_id.clone(),
                dispatch_id: dispatch.dispatch_id.clone(),
            })?;
        }

        let delegation =
            match self
                .event_store
                .insert_delegation_idempotent(&InsertDelegationInput {
                    command_id: input.command_id.clone(),
                    source_agent_id: source.agent_id,
                    source_run_id: input.actor.run_id.clone(),
                    target_agent_id: target.agent_id.clone(),
                    worker_run_id: worker_run_id.clone(),
                    dispatch_id: dispatch.dispatch_id.clone(),
                    runner: runner.as_str().to_string(),
                    request_hash,
                    created_at: Utc::now(),
                })? {
                IdempotencyResolution::Inserted(delegation) => delegation,
                IdempotencyResolution::Replayed(delegation) => delegation,
                IdempotencyResolution::Conflict(_) => return Err(anyhow!("idempotency conflict")),
            };

        let (dispatch, trace) = match runner {
            RunnerKind::OpenCode => {
                let core = RunnerCore::new(
                    self.workspace,
                    self.event_store,
                    self.trace_store,
                    self.blob_store,
                    OpenCodeAdapter,
                );
                let (dispatch, _, trace) = core.run_existing(ExistingRunnerInput {
                    agent: &target,
                    token: &worker_token,
                    dispatch: &dispatch,
                    run_id: &worker_run_id,
                    title: &input.title,
                    task_body: &input.task_body,
                    snapshot_paths: &input.snapshot_paths,
                    binary: input.opencode_bin.as_deref(),
                    timeout_seconds: input.timeout_seconds,
                    trust_project: false,
                })?;
                (dispatch, trace)
            }
            RunnerKind::Codex => {
                let core = RunnerCore::new(
                    self.workspace,
                    self.event_store,
                    self.trace_store,
                    self.blob_store,
                    CodexAdapter,
                );
                let (dispatch, _, trace) = core.run_existing(ExistingRunnerInput {
                    agent: &target,
                    token: &worker_token,
                    dispatch: &dispatch,
                    run_id: &worker_run_id,
                    title: &input.title,
                    task_body: &input.task_body,
                    snapshot_paths: &input.snapshot_paths,
                    binary: input.codex_bin.as_deref(),
                    timeout_seconds: input.timeout_seconds,
                    trust_project: input.trust_project,
                })?;
                (dispatch, trace)
            }
        };
        let delegation = self
            .event_store
            .complete_delegation(&CompleteDelegationInput {
                command_id: delegation.command_id,
                state: "completed".to_string(),
                child_executed: true,
                completed_at: Utc::now(),
            })?;
        let work = work_projection_for_dispatch(&work_service, &dispatch.dispatch_id)?;
        Ok(self.response(delegation, dispatch, "inserted", true, trace, work))
    }

    fn authenticate_actor(&self, actor: &ActorEnv) -> Result<AgentRecord> {
        let agent = self
            .event_store
            .get_agent(&actor.agent_id)?
            .ok_or_else(|| anyhow!("agent not found: {}", actor.agent_id))?;
        let presented_hash = token_hash(&actor.agent_token);
        let run_token_matches = match actor.run_id.as_deref() {
            Some(run_id) => self.event_store.get_agent_run(run_id)?.is_some_and(|run| {
                run.agent_id == agent.agent_id && run.token_hash == presented_hash
            }),
            None => false,
        };
        if agent.token_hash != presented_hash && !run_token_matches {
            return Err(anyhow!("invalid agent token"));
        }
        Ok(agent)
    }

    fn trace_summary_for(
        &self,
        runner: RunnerKind,
        run_id: &str,
        dispatch_id: &str,
    ) -> Result<RunnerTraceProtocol> {
        let adapter = match runner {
            RunnerKind::OpenCode => "opencode-plugin",
            RunnerKind::Codex => "codex-hook",
        };
        let mut events = self.trace_store.list_events(TraceListFilter {
            adapter: Some(adapter.to_string()),
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
            adapter,
            event_count: events.len(),
            session_ids,
        })
    }

    fn response(
        &self,
        delegation: DelegationRecord,
        dispatch: DispatchRecord,
        idempotency_status: &'static str,
        child_executed: bool,
        trace: RunnerTraceProtocol,
        work: Option<WorkProjectionProtocol>,
    ) -> TeamSendResponseProtocol {
        TeamSendResponseProtocol {
            ok: true,
            action: "team.send",
            command_id: delegation.command_id.clone(),
            child_executed,
            expected_next_action: "inspect_dispatch",
            delegation: delegation_protocol(&delegation, idempotency_status),
            dispatch: crate::dispatch::dispatch_protocol(&dispatch, idempotency_status),
            work,
            trace,
        }
    }
}

fn work_projection_for_dispatch(
    work_service: &WorkService<'_, crate::snapshot::LocalSnapshotStore<'_>>,
    dispatch_id: &str,
) -> Result<Option<WorkProjectionProtocol>> {
    work_service.projection_for_dispatch(dispatch_id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunnerKind {
    OpenCode,
    Codex,
}

impl RunnerKind {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "opencode" => Ok(Self::OpenCode),
            "codex" => Ok(Self::Codex),
            _ => Err(anyhow!("runner not supported: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::OpenCode => "opencode",
            Self::Codex => "codex",
        }
    }
}

fn delegation_protocol(
    delegation: &DelegationRecord,
    idempotency_status: &'static str,
) -> DelegationProtocol {
    DelegationProtocol {
        command_id: delegation.command_id.clone(),
        source_agent_id: delegation.source_agent_id.clone(),
        source_run_id: delegation.source_run_id.clone(),
        target_agent_id: delegation.target_agent_id.clone(),
        worker_run_id: delegation.worker_run_id.clone(),
        dispatch_id: delegation.dispatch_id.clone(),
        runner: delegation.runner.clone(),
        state: delegation.state.clone(),
        child_executed: delegation.child_executed,
        created_at: delegation.created_at,
        completed_at: delegation.completed_at,
        idempotency_status,
    }
}

trait RunnerAdapter {
    fn kind(&self) -> &'static str;
    fn default_binary(&self) -> &'static str;
    fn trace_adapter(&self) -> &'static str;
    fn stdout_file_name(&self) -> &'static str;
    fn install_trace(&self, workspace: &Workspace) -> Result<()>;

    fn resolve_binary(&self, binary: Option<&Path>) -> Result<PathBuf> {
        if let Some(path) = binary {
            if !path.exists() {
                return Err(anyhow!("{} not found: {}", self.kind(), path.display()));
            }
            return Ok(path.to_path_buf());
        }
        Ok(PathBuf::from(self.default_binary()))
    }

    fn build_prompt(&self, context: RunnerPromptContext<'_>) -> String {
        build_common_prompt(context, None)
    }

    fn build_command(&self, input: RunnerProcessInput<'_>) -> Result<Command>;
}

struct OpenCodeAdapter;

impl RunnerAdapter for OpenCodeAdapter {
    fn kind(&self) -> &'static str {
        "opencode"
    }

    fn default_binary(&self) -> &'static str {
        "opencode"
    }

    fn trace_adapter(&self) -> &'static str {
        "opencode-plugin"
    }

    fn stdout_file_name(&self) -> &'static str {
        "stdout.jsonl"
    }

    fn install_trace(&self, workspace: &Workspace) -> Result<()> {
        install_opencode_plugin(workspace)?;
        Ok(())
    }

    fn build_command(&self, input: RunnerProcessInput<'_>) -> Result<Command> {
        let mut command = Command::new(input.binary);
        command
            .current_dir(&input.workspace.root)
            .arg("run")
            .arg("--format")
            .arg("json")
            .arg("--dangerously-skip-permissions")
            .arg(input.prompt);
        apply_common_env(&mut command, &input);
        Ok(command)
    }
}

struct CodexAdapter;

impl RunnerAdapter for CodexAdapter {
    fn kind(&self) -> &'static str {
        "codex"
    }

    fn default_binary(&self) -> &'static str {
        "codex"
    }

    fn trace_adapter(&self) -> &'static str {
        "codex-hook"
    }

    fn stdout_file_name(&self) -> &'static str {
        "stdout.jsonl"
    }

    fn install_trace(&self, workspace: &Workspace) -> Result<()> {
        install_codex_hook(workspace)?;
        Ok(())
    }

    fn build_prompt(&self, context: RunnerPromptContext<'_>) -> String {
        build_common_prompt(
            context,
            Some(
                "Codex-specific hints:\n- Use shell commands in this workspace when editing or inspecting files.\n- Codex hooks are debug-only; they do not close the dispatch.\n",
            ),
        )
    }

    fn build_command(&self, input: RunnerProcessInput<'_>) -> Result<Command> {
        let mut command = Command::new(input.binary);
        command
            .current_dir(&input.workspace.root)
            .arg("exec")
            .arg("--enable")
            .arg("codex_hooks")
            .arg("-c")
            .arg("shell_environment_policy.inherit=all")
            .arg("--dangerously-bypass-approvals-and-sandbox")
            .arg("--skip-git-repo-check")
            .arg("--json")
            .arg("--cd")
            .arg(&input.workspace.root);
        if input.trust_project {
            command.arg("-c").arg(format!(
                "projects.\"{}\".trust_level=\"trusted\"",
                input.workspace.root.display()
            ));
        }
        command.arg(input.prompt);
        command.env("CODEX_HOME", prepare_isolated_codex_home(input.run_dir)?);
        apply_common_env(&mut command, &input);
        Ok(command)
    }
}

struct RunnerPromptContext<'a> {
    workspace: &'a Workspace,
    agent: &'a AgentRecord,
    dispatch: &'a DispatchRecord,
    title: &'a str,
    body: &'a [u8],
    snapshot_paths: &'a [PathBuf],
}

struct RunnerProcessInput<'a> {
    binary: &'a Path,
    workspace: &'a Workspace,
    run_dir: &'a Path,
    agent: &'a AgentRecord,
    token: &'a str,
    dispatch: &'a DispatchRecord,
    run_id: &'a str,
    prompt: &'a str,
    trust_project: bool,
}

fn prepare_isolated_codex_home(run_dir: &Path) -> Result<PathBuf> {
    let codex_home = run_dir.join("codex-home");
    fs::create_dir_all(&codex_home)?;
    let source_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")));
    if let Some(source_home) = source_home {
        for file_name in ["auth.json", "installation_id"] {
            let source = source_home.join(file_name);
            if source.exists() {
                let target = codex_home.join(file_name);
                if !target.exists() {
                    fs::copy(&source, &target).with_context(|| {
                        format!(
                            "failed to copy Codex {} into isolated runner home",
                            file_name
                        )
                    })?;
                }
            }
        }
    }
    Ok(codex_home)
}

fn team_send_request_hash(
    input: &TeamSendInput,
    source: &AgentRecord,
    target: &AgentRecord,
    runner: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.agent_id.as_bytes());
    hasher.update(b"\0");
    if let Some(run_id) = &input.actor.run_id {
        hasher.update(run_id.as_bytes());
    }
    hasher.update(b"\0");
    hasher.update(target.agent_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(runner.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.title.as_bytes());
    hasher.update(b"\0");
    if let Some(work_node_id) = &input.work_node_id {
        hasher.update(work_node_id.as_bytes());
    }
    hasher.update(b"\0");
    hasher.update(&input.task_body);
    for path in &input.snapshot_paths {
        hasher.update(b"\0");
        hasher.update(path.to_string_lossy().as_bytes());
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

#[derive(Debug)]
struct ProcessOutput {
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

fn prepare_planner_bin(planner_bin: &Path) -> Result<()> {
    fs::create_dir_all(planner_bin)?;
    let bin_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    write_executable_script(
        &planner_bin.join("team"),
        &format!(
            "#!/bin/sh\nexec \"{}\" \"$@\"\n",
            bin_dir.join("team").display()
        ),
    )?;
    write_executable_script(
        &planner_bin.join("rive"),
        &format!(
            "#!/bin/sh\nexec \"{}\" \"$@\"\n",
            bin_dir.join("rive").display()
        ),
    )?;
    let deny = "#!/bin/sh\necho orchestrator_capability_denied >&2\nexit 126\n";
    for command in [
        "python",
        "python3",
        "pytest",
        "cargo",
        "npm",
        "node",
        "apply_patch",
    ] {
        write_executable_script(&planner_bin.join(command), deny)?;
    }
    write_executable_script(
        &planner_bin.join("git"),
        r#"#!/bin/sh
case "${1:-}" in
  status|diff|log|show|grep|rev-parse)
    exec /usr/bin/git "$@"
    ;;
  *)
    echo orchestrator_capability_denied >&2
    exit 126
    ;;
esac
"#,
    )?;
    Ok(())
}

fn write_executable_script(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

struct WorkspaceMutationBaseline {
    files: BTreeMap<String, String>,
}

impl WorkspaceMutationBaseline {
    fn capture(workspace: &Workspace) -> Result<Self> {
        Ok(Self {
            files: workspace_file_hashes(workspace)?,
        })
    }
}

fn audit_workspace_mutation(
    workspace: &Workspace,
    store: &EventStore,
    baseline: &WorkspaceMutationBaseline,
) -> Result<WorkspaceAuditProtocol> {
    let current = workspace_file_hashes(workspace)?;
    let mut changed = BTreeSet::new();
    for (path, hash) in &current {
        if baseline.files.get(path) != Some(hash) {
            changed.insert(path.clone());
        }
    }
    for path in baseline.files.keys() {
        if !current.contains_key(path) {
            changed.insert(path.clone());
        }
    }
    let allowed = allowed_workspace_mutation_paths(workspace, store)?;
    let denied = changed
        .iter()
        .filter(|path| !allowed.contains(*path))
        .cloned()
        .collect::<Vec<_>>();
    Ok(WorkspaceAuditProtocol {
        checked: true,
        changed_paths: changed.into_iter().collect(),
        allowed_paths: allowed.into_iter().collect(),
        denied_paths: denied,
    })
}

fn workspace_file_hashes(workspace: &Workspace) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    for entry in WalkDir::new(&workspace.root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_audit_ignored(entry.path(), &workspace.root))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let relative = path_relative_to(path, &workspace.root)?;
        let bytes = fs::read(path)?;
        files.insert(relative, format!("sha256:{}", sha256_hex(&bytes)));
    }
    Ok(files)
}

fn is_audit_ignored(path: &Path, root: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    relative.components().next().is_some_and(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(name.as_ref(), ".rive" | ".opencode" | ".git")
    })
}

fn allowed_workspace_mutation_paths(
    workspace: &Workspace,
    store: &EventStore,
) -> Result<BTreeSet<String>> {
    let mut allowed = BTreeSet::new();
    if !store.has_work_schema()? {
        return Ok(allowed);
    }
    for binding in store.list_work_ref_bindings()? {
        if let Some(artifact_ref) = binding.artifact_ref {
            if let Some(path) = artifact_ref.strip_prefix("file:") {
                allowed.insert(path.trim_start_matches("./").to_string());
            }
        }
        if let Some(snapshot_id) = binding.snapshot_id {
            let Some(snapshot) = store.get_snapshot(&snapshot_id)? else {
                continue;
            };
            let manifest = crate::snapshot::read_manifest(workspace, &snapshot)?;
            for file in manifest.files {
                allowed.insert(file.path.trim_start_matches("./").to_string());
            }
        }
    }
    Ok(allowed)
}

fn apply_common_env(command: &mut Command, input: &RunnerProcessInput<'_>) {
    let bin_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let old_path = std::env::var_os("RIVE_WORKER_BASE_PATH")
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    let path = format!("{}:{}", bin_dir.display(), old_path.to_string_lossy());
    command
        .env_remove("RIVE_ORCHESTRATOR_ROOT_WORK_ID")
        .env_remove("RIVE_AVAILABLE_WORKERS")
        .env_remove("RIVE_ORCHESTRATOR_CAPABILITY_PROFILE")
        .env("RIVE_WORKSPACE", &input.workspace.root)
        .env("RIVE_AGENT_ID", &input.agent.agent_id)
        .env("RIVE_AGENT_TOKEN", input.token)
        .env("RIVE_RUN_ID", input.run_id)
        .env("RIVE_DISPATCH_ID", &input.dispatch.dispatch_id)
        .env("PATH", path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

fn run_child_process(command: &mut Command, timeout_seconds: u64) -> Result<ProcessOutput> {
    let mut child = command.spawn().map_err(|err| {
        let program = command.get_program().to_string_lossy();
        if err.kind() == std::io::ErrorKind::NotFound {
            let kind = if program.contains("codex") {
                "codex"
            } else {
                "opencode"
            };
            anyhow!("{kind} not found")
        } else {
            anyhow!("{program} launch failed: {err}")
        }
    })?;

    let started = Instant::now();
    let timeout = Duration::from_secs(timeout_seconds);
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

fn build_common_prompt(context: RunnerPromptContext<'_>, adapter_hints: Option<&str>) -> String {
    let body = String::from_utf8_lossy(context.body);
    let mut snapshot_instructions = String::new();
    if context.snapshot_paths.is_empty() {
        snapshot_instructions
            .push_str("Capture evidence for the files you create or modify before reporting.\n");
    } else {
        snapshot_instructions.push_str("Suggested evidence capture commands:\n");
        for path in context.snapshot_paths {
            let _ = writeln!(
                snapshot_instructions,
                "- rive snapshot capture --path {} --label runner-result",
                path.display()
            );
        }
    }
    let adapter_hints = adapter_hints.unwrap_or("");

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
{adapter_hints}
Task:
{body}
"#,
        workspace = context.workspace.root.display(),
        agent_id = context.agent.agent_id,
        agent_name = context.agent.name,
        dispatch_id = context.dispatch.dispatch_id,
        title = context.title,
        snapshot_instructions = snapshot_instructions,
        adapter_hints = adapter_hints,
        body = body,
    )
}

fn orchestrator_root_body(
    objective: &[u8],
    workers: &[AgentRecord],
    acceptance_command: &Option<String>,
) -> Vec<u8> {
    let payload = json!({
        "objective_sha256": format!("sha256:{}", sha256_hex(objective)),
        "objective": String::from_utf8_lossy(objective),
        "workers": workers
            .iter()
            .map(|worker| json!({
                "agent_id": worker.agent_id,
                "name": worker.name,
                "role": worker.role.as_str(),
            }))
            .collect::<Vec<_>>(),
        "acceptance_command": acceptance_command,
    });
    serde_json::to_vec_pretty(&payload).expect("orchestrator root payload should serialize")
}

fn build_orchestrator_prompt(
    workspace: &Workspace,
    objective: &[u8],
    root_work_node_id: &str,
    workers: &[AgentRecord],
    acceptance_command: Option<&str>,
) -> String {
    let objective = String::from_utf8_lossy(objective);
    let mut worker_lines = String::new();
    for worker in workers {
        let _ = writeln!(
            worker_lines,
            "- {} ({}) use `--to {}` and `--runner opencode`",
            worker.name, worker.agent_id, worker.name
        );
    }
    let acceptance = acceptance_command.unwrap_or("(none provided)");
    format!(
        r#"You are the Rive Orchestrator for this workspace.

Workspace:
- root: {workspace}

Root work node:
- id: {root_work_node_id}

Available workers:
{worker_lines}
All worker delegations in this phase must use `--runner opencode`.

Acceptance command:
{acceptance}

Goal:
{objective}

Rules:
1. Use `team work` to create and maintain a Work DAG under the root node.
2. For a simple objective, create exactly one child implementation node with `team work create`, then connect it to the root with `team work edge add --type decomposes-to --from {root_work_node_id} --to <child>`.
3. Do not add `depends-on`, `validates`, or extra validation nodes unless the acceptance command requires them. Any unfinished dependency will block the root.
4. Use `team work inspect <node>` before delegating and after each worker report.
5. Delegate work with `team send --work <node> --runner opencode --wait --stdin`.
6. Workers must use `rive snapshot capture` and `team report`; natural language completion is not enough.
7. A reported node is only `reviewable`. Use `team work accept` only after checking artifacts, snapshots, or test output.
8. If tests fail or evidence is incomplete, use `team work reopen` or create a follow-up node. Do not rewrite history.
9. Final success requires the root objective projection to be `done`.
10. stdout/final answer/debug trace do not count as completion.

Required final action:
- Inspect the root node.
- Accept the root with `team work accept {root_work_node_id} --command-id <unique-id> --stdin` only when the Work DAG proves the objective is complete.
"#,
        workspace = workspace.root.display(),
        root_work_node_id = root_work_node_id,
        worker_lines = worker_lines,
        acceptance = acceptance,
        objective = objective,
    )
}

struct OrchestratorEnvInput<'a> {
    workspace: &'a Workspace,
    agent: &'a AgentRecord,
    token: &'a str,
    run_id: &'a str,
    root_work_node_id: &'a str,
    workers: &'a [AgentRecord],
    planner_bin: &'a Path,
}

fn apply_orchestrator_env(command: &mut Command, input: OrchestratorEnvInput<'_>) {
    let bin_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let base_path = format!("{}:{}", bin_dir.display(), old_path.to_string_lossy());
    let path = format!("{}:{}", input.planner_bin.display(), base_path);
    let available_workers = input
        .workers
        .iter()
        .map(|worker| worker.name.clone())
        .collect::<Vec<_>>()
        .join(",");
    command
        .env_remove("RIVE_DISPATCH_ID")
        .env("RIVE_WORKSPACE", &input.workspace.root)
        .env("RIVE_AGENT_ID", &input.agent.agent_id)
        .env("RIVE_AGENT_TOKEN", input.token)
        .env("RIVE_RUN_ID", input.run_id)
        .env("RIVE_ORCHESTRATOR_ROOT_WORK_ID", input.root_work_node_id)
        .env("RIVE_AVAILABLE_WORKERS", available_workers)
        .env("RIVE_ORCHESTRATOR_CAPABILITY_PROFILE", "planner")
        .env("RIVE_WORKER_BASE_PATH", base_path)
        .env("PATH", path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

pub fn build_prompt(
    workspace: &Workspace,
    agent: &AgentRecord,
    dispatch: &DispatchRecord,
    title: &str,
    body: &[u8],
    snapshot_paths: &[PathBuf],
) -> String {
    build_common_prompt(
        RunnerPromptContext {
            workspace,
            agent,
            dispatch,
            title,
            body,
            snapshot_paths,
        },
        None,
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
