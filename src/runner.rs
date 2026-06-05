use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::branch::{backend_from_env, BranchService};
use crate::debug_trace::{
    install_codex_hook, install_opencode_plugin, DebugTraceStore, TraceListFilter,
};
use crate::dispatch::{
    agent_protocol, dispatch_protocol, AddAgentInput, CancelDispatchCommand, CreateDispatchInput,
    CreateDispatchOutcome, DispatchService,
};
use crate::facts::ActorEnv;
use crate::store::{
    AgentRecord, AgentRole, CompleteDelegationInput, DelegationRecord, DispatchRecord,
    DispatchState, EventStore, IdempotencyResolution, InsertAgentRunInput, InsertDelegationInput,
    InsertSchedulerNodeFailureInput, InsertSchedulerNodeRunInput, InsertSchedulerRunInput,
    SchedulerNodeFailureRecord, SchedulerNodeRunRecord, SchedulerRunRecord,
    UpdateSchedulerNodeRunInput, UpdateSchedulerRunStateInput, WorkNodeRecord,
    WorkRefBindingRecord,
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
pub struct SchedulerRunInput {
    pub root_work_node_id: String,
    pub runner: String,
    pub workers: Vec<String>,
    pub command_id: String,
    pub max_parallel: usize,
    pub acceptance_mode: String,
    pub workspace_mode: String,
    pub opencode_bin: Option<PathBuf>,
    pub codex_bin: Option<PathBuf>,
    pub timeout_seconds: u64,
    pub trust_project: bool,
}

#[derive(Debug)]
pub struct SchedulerResumeInput {
    pub scheduler_run_id: Option<String>,
    pub root_work_node_id: Option<String>,
    pub work_node_id: Option<String>,
    pub workers: Vec<String>,
    pub command_id: String,
    pub max_parallel: Option<usize>,
    pub acceptance_mode: Option<String>,
    pub workspace_mode: String,
    pub opencode_bin: Option<PathBuf>,
    pub codex_bin: Option<PathBuf>,
    pub timeout_seconds: u64,
    pub trust_project: bool,
    pub failed: bool,
}

#[derive(Debug)]
pub struct SchedulerStatusInput {
    pub scheduler_run_id: Option<String>,
    pub root_work_node_id: Option<String>,
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

#[derive(Debug, Serialize)]
pub struct SchedulerRunResponseProtocol {
    pub scheduler: SchedulerRunProtocol,
    pub root_work: WorkProjectionProtocol,
    pub launched_nodes: Vec<SchedulerNodeRunProtocol>,
    pub completed_nodes: Vec<String>,
    pub waiting_review_nodes: Vec<String>,
    pub stalled_nodes: Vec<String>,
    pub usage_summary: Option<crate::debug_trace::TraceUsageTotals>,
}

#[derive(Debug, Serialize)]
pub struct SchedulerStatusResponseProtocol {
    pub scheduler: Option<SchedulerRunProtocol>,
    pub root_work: WorkProjectionProtocol,
    pub node_runs: Vec<SchedulerNodeRunProtocol>,
    pub active_node_runs: Vec<SchedulerNodeRunProtocol>,
    pub waiting_review_nodes: Vec<String>,
    pub unfinished_nodes: Vec<String>,
    pub usage_summary: Option<crate::debug_trace::TraceUsageTotals>,
}

#[derive(Debug, Serialize)]
pub struct SchedulerRunProtocol {
    pub scheduler_run_id: String,
    pub command_id: String,
    pub root_work_node_id: String,
    pub runner: String,
    pub max_parallel: i64,
    pub acceptance_mode: String,
    pub state: String,
    pub child_executed: bool,
    pub idempotency_status: &'static str,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct SchedulerNodeRunProtocol {
    pub node_run_id: String,
    pub scheduler_run_id: String,
    pub work_node_id: String,
    pub dispatch_id: Option<String>,
    pub worker_agent_id: String,
    pub worker_run_id: Option<String>,
    pub state: String,
    pub failure: Option<WorkerFailureProtocol>,
    pub activity: SchedulerNodeActivityProtocol,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct SchedulerNodeActivityProtocol {
    pub prompt_ref: Option<String>,
    pub stdout_ref: Option<String>,
    pub stderr_ref: Option<String>,
    pub stdout_tail: Option<String>,
    pub stderr_tail: Option<String>,
    pub trace: SchedulerTraceActivityProtocol,
    pub recent_trace_events: Vec<String>,
    pub branch_path: Option<String>,
    pub branch_ref: Option<String>,
    pub changed_files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SchedulerTraceActivityProtocol {
    pub sample_count: usize,
    pub latest_sequence: Option<i64>,
    pub latest_event_kind: Option<String>,
    pub latest_occurred_at: Option<DateTime<Utc>>,
    pub samples: Vec<SchedulerTraceSampleProtocol>,
}

#[derive(Debug, Serialize)]
pub struct SchedulerTraceSampleProtocol {
    pub trace_event_id: String,
    pub sequence: i64,
    pub adapter: String,
    pub event_kind: String,
    pub occurred_at: Option<DateTime<Utc>>,
    pub external_session_id: Option<String>,
    pub external_turn_id: Option<String>,
    pub external_tool_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_status: Option<String>,
    pub tool_input_preview: Option<String>,
    pub tool_output_preview: Option<String>,
    pub text_preview: Option<String>,
    pub session_status: Option<String>,
    pub message_role: Option<String>,
    pub part_type: Option<String>,
    pub summary: serde_json::Value,
}

#[derive(Debug, Serialize, Clone)]
pub struct WorkerFailureProtocol {
    pub failure_kind: String,
    pub retryable: bool,
    pub suggested_action: String,
    pub detail: String,
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

pub struct SchedulerService<'a> {
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
            let baseline = WorkspaceMutationBaseline::capture(self.workspace, self.event_store)?;
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

impl<'a> SchedulerService<'a> {
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

    pub fn run(&self, input: SchedulerRunInput) -> Result<SchedulerRunResponseProtocol> {
        let runner = RunnerKind::parse(&input.runner)?;
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        if input.max_parallel == 0 {
            return Err(anyhow!("scheduler max parallel must be greater than zero"));
        }
        if input.timeout_seconds == 0 {
            return Err(anyhow!("runner timeout must be greater than zero"));
        }
        let acceptance_mode = AcceptanceMode::parse(&input.acceptance_mode)?;
        let workspace_mode = WorkspaceMode::parse(&input.workspace_mode)?;
        self.event_store.init_work_schema()?;
        if workspace_mode == WorkspaceMode::Worktree {
            backend_from_env().ensure_available(self.workspace)?;
        }
        let work_service = WorkService::new(self.workspace, self.event_store, self.blob_store);
        let graph = work_service.inspect_graph(&input.root_work_node_id)?;
        if !graph.orphan_nodes.is_empty() || !graph.unconnected_nodes.is_empty() {
            return Err(anyhow!(
                "work graph not closed: {}",
                input.root_work_node_id
            ));
        }
        let workers = self.resolve_workers(&input.workers)?;
        let worker_ids = workers
            .iter()
            .map(|worker| worker.agent_id.clone())
            .collect::<Vec<_>>();
        let request_hash = scheduler_request_hash(SchedulerRequestHashInput {
            root_work_node_id: &input.root_work_node_id,
            runner: &input.runner,
            worker_ids: &worker_ids,
            max_parallel: input.max_parallel,
            acceptance_mode: acceptance_mode.as_str(),
            workspace_mode: workspace_mode.as_str(),
            timeout_seconds: input.timeout_seconds,
            binary: scheduler_binary_for_runner(&runner, &input),
            trust_project: input.trust_project,
        });
        let inserted =
            self.event_store
                .insert_scheduler_run_idempotent(&InsertSchedulerRunInput {
                    scheduler_run_id: prefixed_id("sched"),
                    command_id: input.command_id.clone(),
                    root_work_node_id: input.root_work_node_id.clone(),
                    runner: input.runner.clone(),
                    max_parallel: input.max_parallel as i64,
                    acceptance_mode: acceptance_mode.as_str().to_string(),
                    request_hash,
                    state: "running".to_string(),
                    created_at: Utc::now(),
                })?;
        let (mut scheduler_run, idempotency_status, should_execute) = match inserted {
            IdempotencyResolution::Inserted(run) => (run, "inserted", true),
            IdempotencyResolution::Replayed(run) => (run, "replayed", false),
            IdempotencyResolution::Conflict(_) => return Err(anyhow!("idempotency conflict")),
        };

        let mut child_executed = false;
        let mut stalled_nodes = Vec::new();
        if should_execute {
            match self.execute_scheduler(
                &scheduler_run,
                &workers,
                &input,
                acceptance_mode,
                workspace_mode,
            ) {
                Ok(state) => {
                    scheduler_run = self.event_store.update_scheduler_run_state(
                        &UpdateSchedulerRunStateInput {
                            scheduler_run_id: scheduler_run.scheduler_run_id.clone(),
                            state,
                            completed_at: Some(Utc::now()),
                        },
                    )?;
                    child_executed = !self
                        .event_store
                        .list_scheduler_node_runs_for_scheduler(&scheduler_run.scheduler_run_id)?
                        .is_empty();
                }
                Err(err) => {
                    let _ = self.event_store.update_scheduler_run_state(
                        &UpdateSchedulerRunStateInput {
                            scheduler_run_id: scheduler_run.scheduler_run_id.clone(),
                            state: "failed".to_string(),
                            completed_at: Some(Utc::now()),
                        },
                    )?;
                    return Err(err);
                }
            }
        }
        if !should_execute && scheduler_run.state == "running" {
            scheduler_run =
                self.event_store
                    .update_scheduler_run_state(&UpdateSchedulerRunStateInput {
                        scheduler_run_id: scheduler_run.scheduler_run_id.clone(),
                        state: "stalled".to_string(),
                        completed_at: Some(Utc::now()),
                    })?;
        }

        let node_runs = self
            .event_store
            .list_scheduler_node_runs_for_scheduler(&scheduler_run.scheduler_run_id)?;
        let completed_nodes = node_runs
            .iter()
            .filter(|run| matches!(run.state.as_str(), "accepted" | "reported"))
            .map(|run| run.work_node_id.clone())
            .collect::<Vec<_>>();
        let waiting_review_nodes = self.waiting_review_nodes(&input.root_work_node_id)?;
        if scheduler_run.state == "stalled" {
            stalled_nodes = self.ready_or_blocked_nodes(&input.root_work_node_id)?;
        }
        let root_work = work_service.inspect_projection(&input.root_work_node_id)?;
        let usage_summary = self.usage_summary_for_runs(&node_runs)?;
        Ok(SchedulerRunResponseProtocol {
            scheduler: scheduler_run_protocol(&scheduler_run, child_executed, idempotency_status),
            root_work,
            launched_nodes: node_runs
                .iter()
                .map(|run| self.scheduler_node_run_protocol(run))
                .collect(),
            completed_nodes,
            waiting_review_nodes,
            stalled_nodes,
            usage_summary,
        })
    }

    pub fn status(&self, input: SchedulerStatusInput) -> Result<SchedulerStatusResponseProtocol> {
        self.event_store.init_work_schema()?;
        let requested_root = input.root_work_node_id.clone();
        let scheduler_run =
            self.resolve_scheduler_run(input.scheduler_run_id, requested_root.clone())?;
        let root_work_node_id = requested_root
            .or_else(|| {
                scheduler_run
                    .as_ref()
                    .map(|run| run.root_work_node_id.clone())
            })
            .as_ref()
            .ok_or_else(|| anyhow!("scheduler root or run is required"))?
            .clone();
        self.scheduler_status_response(scheduler_run, &root_work_node_id)
    }

    pub fn resume(&self, input: SchedulerResumeInput) -> Result<SchedulerRunResponseProtocol> {
        self.event_store.init_work_schema()?;
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        if input.timeout_seconds == 0 {
            return Err(anyhow!("runner timeout must be greater than zero"));
        }
        let inferred_scheduler_run_id =
            if input.scheduler_run_id.is_none() && input.root_work_node_id.is_none() {
                input.work_node_id.as_ref().and_then(|work_node_id| {
                    self.event_store
                        .latest_scheduler_node_run_for_work_node(work_node_id)
                        .ok()
                        .flatten()
                        .map(|run| run.scheduler_run_id)
                })
            } else {
                None
            };
        let source_run = self.resolve_scheduler_run(
            input.scheduler_run_id.clone().or(inferred_scheduler_run_id),
            input.root_work_node_id.clone(),
        )?;
        let root_work_node_id = input
            .root_work_node_id
            .clone()
            .or_else(|| source_run.as_ref().map(|run| run.root_work_node_id.clone()))
            .ok_or_else(|| anyhow!("scheduler root or run is required"))?;
        let runner = source_run
            .as_ref()
            .map(|run| run.runner.clone())
            .unwrap_or_else(|| "opencode".to_string());
        let runner_kind = RunnerKind::parse(&runner)?;
        let max_parallel = input
            .max_parallel
            .or_else(|| {
                source_run
                    .as_ref()
                    .and_then(|run| usize::try_from(run.max_parallel).ok())
            })
            .unwrap_or(1);
        let acceptance_mode = input
            .acceptance_mode
            .clone()
            .or_else(|| source_run.as_ref().map(|run| run.acceptance_mode.clone()))
            .unwrap_or_else(|| "manual".to_string());
        let acceptance = AcceptanceMode::parse(&acceptance_mode)?;
        let workspace_mode = WorkspaceMode::parse(&input.workspace_mode)?;
        if workspace_mode == WorkspaceMode::Worktree {
            backend_from_env().ensure_available(self.workspace)?;
        }
        let workers = self.resolve_workers(&input.workers)?;
        let worker_ids = workers
            .iter()
            .map(|worker| worker.agent_id.clone())
            .collect::<Vec<_>>();
        let request_hash = scheduler_request_hash(SchedulerRequestHashInput {
            root_work_node_id: &root_work_node_id,
            runner: &runner,
            worker_ids: &worker_ids,
            max_parallel,
            acceptance_mode: acceptance.as_str(),
            workspace_mode: workspace_mode.as_str(),
            timeout_seconds: input.timeout_seconds,
            binary: scheduler_binary_for_runner_resume(&runner_kind, &input),
            trust_project: input.trust_project,
        });
        let inserted =
            self.event_store
                .insert_scheduler_run_idempotent(&InsertSchedulerRunInput {
                    scheduler_run_id: prefixed_id("sched"),
                    command_id: input.command_id.clone(),
                    root_work_node_id: root_work_node_id.clone(),
                    runner: runner.clone(),
                    max_parallel: max_parallel as i64,
                    acceptance_mode: acceptance.as_str().to_string(),
                    request_hash,
                    state: "running".to_string(),
                    created_at: Utc::now(),
                })?;
        let (mut scheduler_run, idempotency_status, should_execute) = match inserted {
            IdempotencyResolution::Inserted(run) => (run, "inserted", true),
            IdempotencyResolution::Replayed(run) => (run, "replayed", false),
            IdempotencyResolution::Conflict(_) => return Err(anyhow!("idempotency conflict")),
        };
        let mut child_executed = false;
        if should_execute {
            let stale_nodes = self.supersede_retryable_attempts(
                &root_work_node_id,
                source_run.as_ref(),
                input.work_node_id.as_deref(),
                input.failed,
            )?;
            match self.execute_scheduler_with_initial_nodes(
                &scheduler_run,
                &workers,
                &SchedulerRunInput {
                    root_work_node_id,
                    runner,
                    workers: input.workers,
                    command_id: input.command_id,
                    max_parallel,
                    acceptance_mode,
                    workspace_mode: input.workspace_mode,
                    opencode_bin: input.opencode_bin,
                    codex_bin: input.codex_bin,
                    timeout_seconds: input.timeout_seconds,
                    trust_project: input.trust_project,
                },
                acceptance,
                workspace_mode,
                stale_nodes,
            ) {
                Ok(state) => {
                    scheduler_run = self.event_store.update_scheduler_run_state(
                        &UpdateSchedulerRunStateInput {
                            scheduler_run_id: scheduler_run.scheduler_run_id.clone(),
                            state,
                            completed_at: Some(Utc::now()),
                        },
                    )?;
                    child_executed = !self
                        .event_store
                        .list_scheduler_node_runs_for_scheduler(&scheduler_run.scheduler_run_id)?
                        .is_empty();
                }
                Err(err) => {
                    let _ = self.event_store.update_scheduler_run_state(
                        &UpdateSchedulerRunStateInput {
                            scheduler_run_id: scheduler_run.scheduler_run_id.clone(),
                            state: "failed".to_string(),
                            completed_at: Some(Utc::now()),
                        },
                    )?;
                    return Err(err);
                }
            }
        }
        self.scheduler_run_response(scheduler_run, child_executed, idempotency_status)
    }

    fn scheduler_run_response(
        &self,
        scheduler_run: SchedulerRunRecord,
        child_executed: bool,
        idempotency_status: &'static str,
    ) -> Result<SchedulerRunResponseProtocol> {
        let work_service = WorkService::new(self.workspace, self.event_store, self.blob_store);
        let node_runs = self
            .event_store
            .list_scheduler_node_runs_for_scheduler(&scheduler_run.scheduler_run_id)?;
        let completed_nodes = node_runs
            .iter()
            .filter(|run| matches!(run.state.as_str(), "accepted" | "reported"))
            .map(|run| run.work_node_id.clone())
            .collect::<Vec<_>>();
        let waiting_review_nodes = self.waiting_review_nodes(&scheduler_run.root_work_node_id)?;
        let stalled_nodes = if matches!(scheduler_run.state.as_str(), "stalled" | "failed") {
            self.ready_or_blocked_nodes(&scheduler_run.root_work_node_id)?
        } else {
            Vec::new()
        };
        let root_work = work_service.inspect_projection(&scheduler_run.root_work_node_id)?;
        let usage_summary = self.usage_summary_for_runs(&node_runs)?;
        Ok(SchedulerRunResponseProtocol {
            scheduler: scheduler_run_protocol(&scheduler_run, child_executed, idempotency_status),
            root_work,
            launched_nodes: node_runs
                .iter()
                .map(|run| self.scheduler_node_run_protocol(run))
                .collect(),
            completed_nodes,
            waiting_review_nodes,
            stalled_nodes,
            usage_summary,
        })
    }

    fn scheduler_status_response(
        &self,
        scheduler_run: Option<SchedulerRunRecord>,
        root_work_node_id: &str,
    ) -> Result<SchedulerStatusResponseProtocol> {
        let work_service = WorkService::new(self.workspace, self.event_store, self.blob_store);
        let node_runs = if let Some(run) = &scheduler_run {
            self.event_store
                .list_scheduler_node_runs_for_scheduler(&run.scheduler_run_id)?
        } else {
            Vec::new()
        };
        let reachable = work_service
            .inspect_graph(root_work_node_id)?
            .reachable_nodes
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut active_node_runs = Vec::new();
        for node_id in &reachable {
            active_node_runs.extend(
                self.event_store
                    .list_active_scheduler_node_runs_for_work_node(node_id)?,
            );
        }
        let root_work = work_service.inspect_projection(root_work_node_id)?;
        let waiting_review_nodes = self.waiting_review_nodes(root_work_node_id)?;
        let unfinished_nodes = self.ready_or_blocked_nodes(root_work_node_id)?;
        let usage_summary = self.usage_summary_for_runs(&node_runs)?;
        Ok(SchedulerStatusResponseProtocol {
            scheduler: scheduler_run
                .as_ref()
                .map(|run| scheduler_run_protocol(run, false, "status")),
            root_work,
            node_runs: node_runs
                .iter()
                .map(|run| self.scheduler_node_run_protocol(run))
                .collect(),
            active_node_runs: active_node_runs
                .iter()
                .map(|run| self.scheduler_node_run_protocol(run))
                .collect(),
            waiting_review_nodes,
            unfinished_nodes,
            usage_summary,
        })
    }

    fn resolve_scheduler_run(
        &self,
        scheduler_run_id: Option<String>,
        root_work_node_id: Option<String>,
    ) -> Result<Option<SchedulerRunRecord>> {
        match (scheduler_run_id, root_work_node_id) {
            (Some(run_id), _) => self
                .event_store
                .get_scheduler_run(&run_id)?
                .ok_or_else(|| anyhow!("scheduler run not found: {run_id}"))
                .map(Some),
            (None, Some(root)) => self.event_store.latest_scheduler_run_for_root(&root),
            (None, None) => Err(anyhow!("scheduler root or run is required")),
        }
    }

    fn supersede_retryable_attempts(
        &self,
        root_work_node_id: &str,
        source_run: Option<&SchedulerRunRecord>,
        target_work_node_id: Option<&str>,
        include_failed: bool,
    ) -> Result<Vec<WorkNodeRecord>> {
        let work_service = WorkService::new(self.workspace, self.event_store, self.blob_store);
        let reachable = work_service
            .inspect_graph(root_work_node_id)?
            .reachable_nodes
            .into_iter()
            .collect::<BTreeSet<_>>();
        let explicit_target_retry = target_work_node_id.is_some();
        let include_reported_retry = include_failed
            && (explicit_target_retry
                || source_run
                    .map(|source| matches!(source.state.as_str(), "failed" | "superseded"))
                    .unwrap_or(false));
        let mut retry_nodes = BTreeMap::new();
        for node_id in reachable {
            if let Some(target) = target_work_node_id {
                if node_id != target {
                    continue;
                }
            }
            let mut attempts = self
                .event_store
                .list_active_scheduler_node_runs_for_work_node(&node_id)?;
            if include_failed {
                attempts.extend(
                    self.event_store
                        .list_scheduler_node_runs_for_work_node(&node_id)?
                        .into_iter()
                        .filter(|run| {
                            run.state == "failed"
                                || (include_reported_retry && run.state == "reported")
                        }),
                );
                attempts.sort_by_key(|run| run.started_at);
                attempts.dedup_by(|a, b| a.node_run_id == b.node_run_id);
            }
            for run in attempts {
                if let Some(source) = source_run {
                    if run.scheduler_run_id != source.scheduler_run_id {
                        continue;
                    }
                }
                if let Some(dispatch_id) = &run.dispatch_id {
                    if let Some(dispatch) = self.event_store.get_dispatch(dispatch_id)? {
                        if matches!(dispatch.state, DispatchState::Reported) {
                            let should_retry_reported = include_failed
                                && (explicit_target_retry
                                    || source_run
                                        .map(|source| source.state == "failed")
                                        .unwrap_or(false))
                                && self.reported_dispatch_has_uncommitted_branch_integration(
                                    &node_id,
                                    &dispatch.dispatch_id,
                                )?;
                            if should_retry_reported {
                                self.event_store.update_scheduler_node_run(
                                    &UpdateSchedulerNodeRunInput {
                                        node_run_id: run.node_run_id.clone(),
                                        dispatch_id: run.dispatch_id.clone(),
                                        worker_run_id: run.worker_run_id.clone(),
                                        state: "superseded".to_string(),
                                        completed_at: Some(Utc::now()),
                                    },
                                )?;
                                if let Some(node) = self.event_store.get_work_node(&node_id)? {
                                    retry_nodes.insert(node.work_node_id.clone(), node);
                                }
                                continue;
                            }
                            self.event_store.update_scheduler_node_run(
                                &UpdateSchedulerNodeRunInput {
                                    node_run_id: run.node_run_id.clone(),
                                    dispatch_id: run.dispatch_id.clone(),
                                    worker_run_id: run.worker_run_id.clone(),
                                    state: "reported".to_string(),
                                    completed_at: Some(Utc::now()),
                                },
                            )?;
                            continue;
                        }
                        if matches!(dispatch.state, DispatchState::Open | DispatchState::Blocked) {
                            let dispatch_service = DispatchService::new(
                                self.workspace,
                                self.event_store,
                                self.blob_store,
                            );
                            let _ = dispatch_service.cancel_dispatch(CancelDispatchCommand {
                                command_id: format!(
                                    "scheduler-resume:{}:cancel:{}",
                                    root_work_node_id, run.node_run_id
                                ),
                                dispatch_id: dispatch.dispatch_id,
                                reason: "scheduler resume superseded stale attempt".to_string(),
                            })?;
                        }
                    }
                }
                self.event_store
                    .update_scheduler_node_run(&UpdateSchedulerNodeRunInput {
                        node_run_id: run.node_run_id.clone(),
                        dispatch_id: run.dispatch_id,
                        worker_run_id: run.worker_run_id,
                        state: "superseded".to_string(),
                        completed_at: Some(Utc::now()),
                    })?;
                if let Some(node) = self.event_store.get_work_node(&node_id)? {
                    retry_nodes.insert(node.work_node_id.clone(), node);
                }
            }
        }
        if let Some(source) = source_run {
            if matches!(source.state.as_str(), "running" | "stalled" | "failed") {
                let _ =
                    self.event_store
                        .update_scheduler_run_state(&UpdateSchedulerRunStateInput {
                            scheduler_run_id: source.scheduler_run_id.clone(),
                            state: "superseded".to_string(),
                            completed_at: Some(Utc::now()),
                        })?;
            }
        }
        Ok(retry_nodes.into_values().collect())
    }

    fn reported_dispatch_has_uncommitted_branch_integration(
        &self,
        work_node_id: &str,
        dispatch_id: &str,
    ) -> Result<bool> {
        Ok(self
            .event_store
            .list_branch_integrations()?
            .into_iter()
            .any(|integration| {
                integration.work_node_id == work_node_id
                    && integration.dispatch_id == dispatch_id
                    && integration.state != "committed"
            }))
    }

    fn execute_scheduler(
        &self,
        scheduler_run: &SchedulerRunRecord,
        workers: &[AgentRecord],
        input: &SchedulerRunInput,
        acceptance_mode: AcceptanceMode,
        workspace_mode: WorkspaceMode,
    ) -> Result<String> {
        self.execute_scheduler_with_initial_nodes(
            scheduler_run,
            workers,
            input,
            acceptance_mode,
            workspace_mode,
            Vec::new(),
        )
    }

    fn execute_scheduler_with_initial_nodes(
        &self,
        scheduler_run: &SchedulerRunRecord,
        workers: &[AgentRecord],
        input: &SchedulerRunInput,
        acceptance_mode: AcceptanceMode,
        workspace_mode: WorkspaceMode,
        initial_nodes: Vec<WorkNodeRecord>,
    ) -> Result<String> {
        let work_service = WorkService::new(self.workspace, self.event_store, self.blob_store);
        let mut worker_index = 0usize;
        if !initial_nodes.is_empty() {
            for chunk in initial_nodes.chunks(input.max_parallel) {
                let batch = chunk
                    .iter()
                    .cloned()
                    .map(|node| {
                        let worker = workers[worker_index % workers.len()].clone();
                        worker_index += 1;
                        (node, worker)
                    })
                    .collect::<Vec<_>>();
                self.run_node_batch(scheduler_run, batch, input, acceptance_mode, workspace_mode)?;
            }
        }
        let mut made_progress = true;
        while made_progress {
            made_progress = false;
            let root_projection = work_service.inspect_projection(&input.root_work_node_id)?;
            if root_projection.state == "done" {
                return Ok("completed".to_string());
            }
            let mut candidates = self.ready_leaf_candidates(&input.root_work_node_id)?;
            let already_run = self
                .event_store
                .list_scheduler_node_runs_for_scheduler(&scheduler_run.scheduler_run_id)?
                .into_iter()
                .map(|run| run.work_node_id)
                .collect::<BTreeSet<_>>();
            candidates.retain(|node| !already_run.contains(&node.work_node_id));
            let batch = candidates
                .into_iter()
                .take(input.max_parallel)
                .map(|node| {
                    let worker = workers[worker_index % workers.len()].clone();
                    worker_index += 1;
                    (node, worker)
                })
                .collect::<Vec<_>>();
            if !batch.is_empty() {
                self.run_node_batch(scheduler_run, batch, input, acceptance_mode, workspace_mode)?;
                made_progress = true;
            }
            if matches!(
                acceptance_mode,
                AcceptanceMode::AutoReported | AcceptanceMode::AutoCommitted
            ) {
                if self.accept_reviewable_nodes(
                    scheduler_run,
                    &input.root_work_node_id,
                    acceptance_mode,
                )? > 0
                {
                    made_progress = true;
                }
                let root_projection = work_service.inspect_projection(&input.root_work_node_id)?;
                if root_projection.state == "reviewable" {
                    work_service.accept_node(crate::work::WorkStatusInput {
                        command_id: format!(
                            "scheduler:{}:accept:{}",
                            scheduler_run.scheduler_run_id, input.root_work_node_id
                        ),
                        work_node_id: input.root_work_node_id.clone(),
                        reason: format!("scheduler {} root acceptance", acceptance_mode.as_str())
                            .into_bytes(),
                        require_committed_branch: acceptance_mode == AcceptanceMode::AutoCommitted,
                    })?;
                    return Ok("completed".to_string());
                }
                if work_service
                    .inspect_projection(&input.root_work_node_id)?
                    .state
                    == "done"
                {
                    return Ok("completed".to_string());
                }
            }
        }
        if !self
            .waiting_review_nodes(&input.root_work_node_id)?
            .is_empty()
            && acceptance_mode == AcceptanceMode::Manual
        {
            return Ok("waiting_review".to_string());
        }
        let root_projection = work_service.inspect_projection(&input.root_work_node_id)?;
        if root_projection.state == "done" {
            Ok("completed".to_string())
        } else {
            Err(anyhow!(
                "work scheduler stalled: {}",
                input.root_work_node_id
            ))
        }
    }

    fn run_node_batch(
        &self,
        scheduler_run: &SchedulerRunRecord,
        batch: Vec<(WorkNodeRecord, AgentRecord)>,
        input: &SchedulerRunInput,
        acceptance_mode: AcceptanceMode,
        workspace_mode: WorkspaceMode,
    ) -> Result<()> {
        let dispatch_service =
            DispatchService::new(self.workspace, self.event_store, self.blob_store);
        let work_service = WorkService::new(self.workspace, self.event_store, self.blob_store);
        let mut launches = Vec::new();
        for (node, worker) in batch {
            let node_run =
                self.event_store
                    .insert_scheduler_node_run_claim(&InsertSchedulerNodeRunInput {
                        node_run_id: prefixed_id("schednode"),
                        scheduler_run_id: scheduler_run.scheduler_run_id.clone(),
                        work_node_id: node.work_node_id.clone(),
                        worker_agent_id: worker.agent_id.clone(),
                        started_at: Utc::now(),
                    })?;
            let worker_run_id = prefixed_id("run");
            let worker_token = prefixed_id("tok");
            self.event_store.insert_agent_run(&InsertAgentRunInput {
                run_id: worker_run_id.clone(),
                agent_id: worker.agent_id.clone(),
                token_hash: token_hash(&worker_token),
                created_at: Utc::now(),
            })?;
            let dispatch_command_id = format!(
                "scheduler:{}:{}:dispatch",
                scheduler_run.scheduler_run_id, node.work_node_id
            );
            let mut branch_ref = None;
            let mut branch_path = None;
            let task_body = scheduler_task_body(self.workspace, &node, None);
            let dispatch = match dispatch_service.create_dispatch(CreateDispatchInput {
                command_id: dispatch_command_id,
                target_agent: worker.agent_id.clone(),
                title: node.title.clone(),
                body: task_body.clone(),
            })? {
                CreateDispatchOutcome::Inserted(dispatch)
                | CreateDispatchOutcome::Replayed(dispatch) => dispatch,
            };
            work_service.bind_dispatch(BindWorkDispatchCommand {
                work_node_id: node.work_node_id.clone(),
                dispatch_id: dispatch.dispatch_id.clone(),
            })?;
            if workspace_mode == WorkspaceMode::Worktree {
                let backend = backend_from_env();
                let branch = BranchService::new(self.workspace, self.event_store)
                    .create_workspace(
                        backend.as_ref(),
                        &scheduler_run.root_work_node_id,
                        &node.work_node_id,
                        &dispatch.dispatch_id,
                        &worker_run_id,
                    )?;
                branch_path = Some(PathBuf::from(branch.branch_path));
                branch_ref = Some(branch.branch_ref);
            }
            let task_body = scheduler_task_body(self.workspace, &node, branch_ref.as_deref());
            self.event_store
                .update_scheduler_node_run(&UpdateSchedulerNodeRunInput {
                    node_run_id: node_run.node_run_id.clone(),
                    dispatch_id: Some(dispatch.dispatch_id.clone()),
                    worker_run_id: Some(worker_run_id.clone()),
                    state: "running".to_string(),
                    completed_at: None,
                })?;
            let runner = RunnerKind::parse(&input.runner)?;
            let binary = scheduler_binary_for_runner(&runner, input).map(Path::to_path_buf);
            launches.push(SchedulerLaunch {
                node_run_id: node_run.node_run_id,
                work_node_id: node.work_node_id,
                worker,
                worker_run_id,
                worker_token,
                dispatch,
                title: node.title,
                task_body,
                runner,
                binary,
                timeout_seconds: input.timeout_seconds,
                trust_project: input.trust_project,
                branch_ref,
                branch_path,
            });
        }

        let results = self.execute_launches(launches)?;
        let mut first_error = None;
        for result in results {
            match result.result {
                Ok(dispatch_id) => {
                    let dispatch = self
                        .event_store
                        .get_dispatch(&dispatch_id)?
                        .ok_or_else(|| anyhow!("dispatch not found: {dispatch_id}"))?;
                    let projection = work_service.inspect_projection(&result.work_node_id)?;
                    if projection.state != "reviewable" && projection.state != "done" {
                        self.event_store.update_scheduler_node_run(
                            &UpdateSchedulerNodeRunInput {
                                node_run_id: result.node_run_id.clone(),
                                dispatch_id: Some(dispatch.dispatch_id.clone()),
                                worker_run_id: Some(result.worker_run_id.clone()),
                                state: "failed".to_string(),
                                completed_at: Some(Utc::now()),
                            },
                        )?;
                        self.record_node_failure(
                            &result.node_run_id,
                            &format!("work scheduler stalled: {}", result.work_node_id),
                        )?;
                        first_error.get_or_insert_with(|| {
                            anyhow!("work scheduler stalled: {}", result.work_node_id)
                        });
                        continue;
                    }
                    let mut state = "reported";
                    if acceptance_mode == AcceptanceMode::AutoCommitted
                        && projection.state == "reviewable"
                    {
                        if let Some(integration) =
                            BranchService::new(self.workspace, self.event_store)
                                .list()?
                                .into_iter()
                                .find(|integration| {
                                    integration.work_node_id == result.work_node_id
                                        && integration.dispatch_id == dispatch.dispatch_id
                                })
                        {
                            let backend = backend_from_env();
                            if let Err(err) = BranchService::new(self.workspace, self.event_store)
                                .commit(
                                    backend.as_ref(),
                                    &integration.integration_id,
                                    &format!(
                                        "scheduler:{}:branch-commit:{}",
                                        scheduler_run.scheduler_run_id, result.work_node_id
                                    ),
                                )
                            {
                                self.event_store.update_scheduler_node_run(
                                    &UpdateSchedulerNodeRunInput {
                                        node_run_id: result.node_run_id.clone(),
                                        dispatch_id: Some(dispatch.dispatch_id.clone()),
                                        worker_run_id: Some(result.worker_run_id.clone()),
                                        state: "failed".to_string(),
                                        completed_at: Some(Utc::now()),
                                    },
                                )?;
                                self.record_node_failure(&result.node_run_id, &err.to_string())?;
                                first_error.get_or_insert(err);
                                continue;
                            }
                        }
                        if let Err(err) = work_service.accept_node(crate::work::WorkStatusInput {
                            command_id: format!(
                                "scheduler:{}:accept:{}",
                                scheduler_run.scheduler_run_id, result.work_node_id
                            ),
                            work_node_id: result.work_node_id.clone(),
                            reason: b"scheduler auto-committed acceptance".to_vec(),
                            require_committed_branch: true,
                        }) {
                            self.event_store.update_scheduler_node_run(
                                &UpdateSchedulerNodeRunInput {
                                    node_run_id: result.node_run_id.clone(),
                                    dispatch_id: Some(dispatch.dispatch_id.clone()),
                                    worker_run_id: Some(result.worker_run_id.clone()),
                                    state: "failed".to_string(),
                                    completed_at: Some(Utc::now()),
                                },
                            )?;
                            self.record_node_failure(&result.node_run_id, &err.to_string())?;
                            first_error.get_or_insert(err);
                            continue;
                        }
                        state = "accepted";
                    } else if acceptance_mode == AcceptanceMode::AutoReported
                        && projection.state == "reviewable"
                    {
                        work_service.accept_node(crate::work::WorkStatusInput {
                            command_id: format!(
                                "scheduler:{}:accept:{}",
                                scheduler_run.scheduler_run_id, result.work_node_id
                            ),
                            work_node_id: result.work_node_id.clone(),
                            reason: b"scheduler auto-reported acceptance".to_vec(),
                            require_committed_branch: false,
                        })?;
                        state = "accepted";
                    }
                    self.event_store
                        .update_scheduler_node_run(&UpdateSchedulerNodeRunInput {
                            node_run_id: result.node_run_id,
                            dispatch_id: Some(dispatch.dispatch_id),
                            worker_run_id: Some(result.worker_run_id),
                            state: state.to_string(),
                            completed_at: Some(Utc::now()),
                        })?;
                }
                Err(err) => {
                    let detail = err.to_string();
                    self.event_store
                        .update_scheduler_node_run(&UpdateSchedulerNodeRunInput {
                            node_run_id: result.node_run_id.clone(),
                            dispatch_id: Some(result.dispatch_id),
                            worker_run_id: Some(result.worker_run_id),
                            state: "failed".to_string(),
                            completed_at: Some(Utc::now()),
                        })?;
                    self.record_node_failure(&result.node_run_id, &detail)?;
                    first_error.get_or_insert(err);
                }
            }
        }
        if let Some(err) = first_error {
            return Err(err);
        }
        Ok(())
    }

    fn record_node_failure(&self, node_run_id: &str, detail: &str) -> Result<()> {
        let failure = classify_worker_failure(detail);
        self.event_store
            .record_scheduler_node_failure(&InsertSchedulerNodeFailureInput {
                node_run_id: node_run_id.to_string(),
                failure_kind: failure.failure_kind,
                retryable: failure.retryable,
                suggested_action: failure.suggested_action,
                detail: failure.detail,
                created_at: Utc::now(),
            })?;
        Ok(())
    }

    fn execute_launches(
        &self,
        launches: Vec<SchedulerLaunch>,
    ) -> Result<Vec<SchedulerLaunchResult>> {
        let (tx, rx) = mpsc::channel();
        let workspace = self.workspace.clone();
        for launch in launches {
            let tx = tx.clone();
            let workspace = workspace.clone();
            thread::spawn(move || {
                let result = run_scheduler_launch(&workspace, &launch);
                let _ = tx.send(SchedulerLaunchResult {
                    node_run_id: launch.node_run_id,
                    work_node_id: launch.work_node_id,
                    dispatch_id: launch.dispatch.dispatch_id,
                    worker_run_id: launch.worker_run_id,
                    result,
                });
            });
        }
        drop(tx);
        let mut results = Vec::new();
        for result in rx {
            results.push(result);
        }
        Ok(results)
    }

    fn resolve_workers(&self, workers: &[String]) -> Result<Vec<AgentRecord>> {
        if workers.is_empty() {
            return Err(anyhow!("scheduler worker is required"));
        }
        let mut records = Vec::new();
        for worker in workers {
            let record = self
                .event_store
                .get_agent(worker)?
                .ok_or_else(|| anyhow!("agent not found: {worker}"))?;
            if record.role != AgentRole::Worker {
                return Err(anyhow!("scheduler worker must be worker"));
            }
            records.push(record);
        }
        Ok(records)
    }

    fn ready_leaf_candidates(&self, root_work_node_id: &str) -> Result<Vec<WorkNodeRecord>> {
        let work_service = WorkService::new(self.workspace, self.event_store, self.blob_store);
        let graph = work_service.inspect_graph(root_work_node_id)?;
        let reachable = graph.reachable_nodes.into_iter().collect::<BTreeSet<_>>();
        let decomposed_parents = self
            .event_store
            .list_work_edges()?
            .iter()
            .filter(|edge| edge.edge_type == "decomposes_to")
            .map(|edge| edge.from_node_id.clone())
            .collect::<BTreeSet<_>>();
        let open_dispatch_nodes = open_dispatch_nodes(self.event_store)?;
        let mut candidates = Vec::new();
        for node in self.event_store.list_work_nodes()? {
            if node.work_node_id == root_work_node_id
                || !reachable.contains(&node.work_node_id)
                || decomposed_parents.contains(&node.work_node_id)
                || open_dispatch_nodes.contains(&node.work_node_id)
                || !self
                    .event_store
                    .list_active_scheduler_node_runs_for_work_node(&node.work_node_id)?
                    .is_empty()
            {
                continue;
            }
            let projection = work_service.inspect_projection(&node.work_node_id)?;
            if projection.state == "ready" {
                candidates.push(node);
            }
        }
        Ok(candidates)
    }

    fn accept_reviewable_nodes(
        &self,
        scheduler_run: &SchedulerRunRecord,
        root_work_node_id: &str,
        acceptance_mode: AcceptanceMode,
    ) -> Result<usize> {
        let work_service = WorkService::new(self.workspace, self.event_store, self.blob_store);
        let mut accepted = 0usize;
        for node_id in work_service
            .inspect_graph(root_work_node_id)?
            .reachable_nodes
        {
            if node_id == root_work_node_id {
                continue;
            }
            let projection = work_service.inspect_projection(&node_id)?;
            if projection.state == "reviewable" {
                if acceptance_mode == AcceptanceMode::AutoCommitted {
                    if let Some(integration) = BranchService::new(self.workspace, self.event_store)
                        .list()?
                        .into_iter()
                        .find(|integration| {
                            integration.work_node_id == node_id && integration.state == "pending"
                        })
                    {
                        let backend = backend_from_env();
                        BranchService::new(self.workspace, self.event_store).commit(
                            backend.as_ref(),
                            &integration.integration_id,
                            &format!(
                                "scheduler:{}:branch-commit:{}",
                                scheduler_run.scheduler_run_id, node_id
                            ),
                        )?;
                    }
                }
                work_service.accept_node(crate::work::WorkStatusInput {
                    command_id: format!(
                        "scheduler:{}:accept:{}",
                        scheduler_run.scheduler_run_id, node_id
                    ),
                    work_node_id: node_id,
                    reason: format!("scheduler {} acceptance", acceptance_mode.as_str())
                        .into_bytes(),
                    require_committed_branch: acceptance_mode == AcceptanceMode::AutoCommitted,
                })?;
                accepted += 1;
            }
        }
        Ok(accepted)
    }

    fn waiting_review_nodes(&self, root_work_node_id: &str) -> Result<Vec<String>> {
        let work_service = WorkService::new(self.workspace, self.event_store, self.blob_store);
        let mut nodes = Vec::new();
        for node_id in work_service
            .inspect_graph(root_work_node_id)?
            .reachable_nodes
        {
            if work_service.inspect_projection(&node_id)?.state == "reviewable" {
                nodes.push(node_id);
            }
        }
        nodes.sort();
        Ok(nodes)
    }

    fn ready_or_blocked_nodes(&self, root_work_node_id: &str) -> Result<Vec<String>> {
        let work_service = WorkService::new(self.workspace, self.event_store, self.blob_store);
        let mut nodes = Vec::new();
        for node_id in work_service
            .inspect_graph(root_work_node_id)?
            .reachable_nodes
        {
            let state = work_service.inspect_projection(&node_id)?.state;
            if state != "done" {
                nodes.push(node_id);
            }
        }
        nodes.sort();
        Ok(nodes)
    }

    fn usage_summary_for_runs(
        &self,
        node_runs: &[SchedulerNodeRunRecord],
    ) -> Result<Option<crate::debug_trace::TraceUsageTotals>> {
        let run_ids = node_runs
            .iter()
            .filter_map(|run| run.worker_run_id.clone())
            .collect::<BTreeSet<_>>();
        if run_ids.is_empty() {
            return Ok(None);
        }
        let usage = crate::debug_trace::usage_for_workspace(
            self.workspace,
            self.trace_store,
            crate::debug_trace::TraceUsageFilter {
                correlated_run_ids: run_ids,
                ..Default::default()
            },
        )?;
        Ok(Some(usage.totals))
    }

    fn scheduler_node_run_protocol(
        &self,
        run: &SchedulerNodeRunRecord,
    ) -> SchedulerNodeRunProtocol {
        scheduler_node_run_protocol(
            run,
            self.event_store
                .get_scheduler_node_failure(&run.node_run_id)
                .ok()
                .flatten(),
            self.scheduler_node_activity(run)
                .unwrap_or_else(|_| empty_scheduler_node_activity()),
        )
    }

    fn scheduler_node_activity(
        &self,
        run: &SchedulerNodeRunRecord,
    ) -> Result<SchedulerNodeActivityProtocol> {
        let Some(worker_run_id) = run.worker_run_id.as_ref() else {
            return Ok(empty_scheduler_node_activity());
        };
        let run_ref = debug_run_ref(self.workspace, worker_run_id);
        let run_dir = self.workspace.debug_runs_dir().join(worker_run_id);
        let stdout_path = run_dir.join("stdout.jsonl");
        let stderr_path = run_dir.join("stderr.log");
        let prompt_path = run_dir.join("prompt.txt");
        let stdout_ref = stdout_path
            .exists()
            .then(|| format!("{run_ref}/stdout.jsonl"));
        let stderr_ref = stderr_path
            .exists()
            .then(|| format!("{run_ref}/stderr.log"));
        let prompt_ref = prompt_path
            .exists()
            .then(|| format!("{run_ref}/prompt.txt"));
        let stdout_tail = read_tail_if_exists(&stdout_path, 12);
        let stderr_tail = read_tail_if_exists(&stderr_path, 12);
        let trace = self.recent_trace_activity(worker_run_id, run.dispatch_id.as_deref());
        let recent_trace_events = trace
            .samples
            .iter()
            .map(trace_sample_compat_summary)
            .collect();
        let (branch_path, branch_ref, changed_files) =
            self.branch_activity_for_dispatch(run.dispatch_id.as_deref());
        Ok(SchedulerNodeActivityProtocol {
            prompt_ref,
            stdout_ref,
            stderr_ref,
            stdout_tail,
            stderr_tail,
            trace,
            recent_trace_events,
            branch_path,
            branch_ref,
            changed_files,
        })
    }

    fn recent_trace_activity(
        &self,
        run_id: &str,
        dispatch_id: Option<&str>,
    ) -> SchedulerTraceActivityProtocol {
        let samples = self
            .trace_store
            .list_events(TraceListFilter {
                dispatch_id: dispatch_id.map(ToOwned::to_owned),
                ..TraceListFilter::default()
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|event| {
                event.run_id.as_deref() == Some(run_id)
                    || dispatch_id.is_some_and(|id| event.dispatch_id.as_deref() == Some(id))
            })
            .take(10)
            .map(trace_sample_protocol)
            .collect::<Vec<_>>();
        let latest = samples.first();
        SchedulerTraceActivityProtocol {
            sample_count: samples.len(),
            latest_sequence: latest.map(|sample| sample.sequence),
            latest_event_kind: latest.map(|sample| sample.event_kind.clone()),
            latest_occurred_at: latest.and_then(|sample| sample.occurred_at),
            samples,
        }
    }

    fn branch_activity_for_dispatch(
        &self,
        dispatch_id: Option<&str>,
    ) -> (Option<String>, Option<String>, Vec<String>) {
        let Some(dispatch_id) = dispatch_id else {
            return (None, None, Vec::new());
        };
        let Some(integration) = self
            .event_store
            .list_branch_integrations()
            .unwrap_or_default()
            .into_iter()
            .find(|integration| integration.dispatch_id == dispatch_id)
        else {
            return (None, None, Vec::new());
        };
        let Some(branch) = self
            .event_store
            .get_branch_workspace(&integration.branch_id)
            .ok()
            .flatten()
        else {
            return (None, Some(integration.branch_ref), Vec::new());
        };
        let changed_files =
            changed_files_for_activity(&self.workspace.root, Path::new(&branch.branch_path))
                .unwrap_or_default();
        (
            Some(branch.branch_path),
            Some(integration.branch_ref),
            changed_files,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AcceptanceMode {
    Manual,
    AutoReported,
    AutoCommitted,
}

impl AcceptanceMode {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "manual" => Ok(Self::Manual),
            "auto-reported" => Ok(Self::AutoReported),
            "auto-committed" => Ok(Self::AutoCommitted),
            _ => Err(anyhow!("invalid acceptance mode: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::AutoReported => "auto-reported",
            Self::AutoCommitted => "auto-committed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspaceMode {
    Shared,
    Worktree,
}

impl WorkspaceMode {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "shared" => Ok(Self::Shared),
            "worktree" => Ok(Self::Worktree),
            _ => Err(anyhow!("workspace mode not supported: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Worktree => "worktree",
        }
    }
}

struct SchedulerLaunch {
    node_run_id: String,
    work_node_id: String,
    worker: AgentRecord,
    worker_run_id: String,
    worker_token: String,
    dispatch: DispatchRecord,
    title: String,
    task_body: Vec<u8>,
    runner: RunnerKind,
    binary: Option<PathBuf>,
    timeout_seconds: u64,
    trust_project: bool,
    branch_ref: Option<String>,
    branch_path: Option<PathBuf>,
}

struct SchedulerLaunchResult {
    node_run_id: String,
    work_node_id: String,
    dispatch_id: String,
    worker_run_id: String,
    result: Result<String>,
}

fn run_scheduler_launch(workspace: &Workspace, launch: &SchedulerLaunch) -> Result<String> {
    let event_store = EventStore::open(&workspace.db_path())?;
    event_store.init_schema()?;
    let trace_store = DebugTraceStore::open(&workspace.db_path())?;
    trace_store.init_schema()?;
    let blob_store = crate::snapshot::LocalSnapshotStore::new(workspace);
    let input = ExistingRunnerInput {
        agent: &launch.worker,
        token: &launch.worker_token,
        dispatch: &launch.dispatch,
        run_id: &launch.worker_run_id,
        title: &launch.title,
        task_body: &launch.task_body,
        snapshot_paths: &[],
        binary: launch.binary.as_deref(),
        timeout_seconds: launch.timeout_seconds,
        trust_project: launch.trust_project,
        workspace_override: launch.branch_path.as_deref(),
        branch_ref: launch.branch_ref.as_deref(),
    };
    let (dispatch, _, _) = match launch.runner {
        RunnerKind::OpenCode => RunnerCore::new(
            workspace,
            &event_store,
            &trace_store,
            &blob_store,
            OpenCodeAdapter,
        )
        .run_existing(input)?,
        RunnerKind::Codex => RunnerCore::new(
            workspace,
            &event_store,
            &trace_store,
            &blob_store,
            CodexAdapter,
        )
        .run_existing(input)?,
    };
    Ok(dispatch.dispatch_id)
}

fn scheduler_run_protocol(
    run: &SchedulerRunRecord,
    child_executed: bool,
    idempotency_status: &'static str,
) -> SchedulerRunProtocol {
    SchedulerRunProtocol {
        scheduler_run_id: run.scheduler_run_id.clone(),
        command_id: run.command_id.clone(),
        root_work_node_id: run.root_work_node_id.clone(),
        runner: run.runner.clone(),
        max_parallel: run.max_parallel,
        acceptance_mode: run.acceptance_mode.clone(),
        state: run.state.clone(),
        child_executed,
        idempotency_status,
        created_at: run.created_at,
        completed_at: run.completed_at,
    }
}

fn scheduler_node_run_protocol(
    run: &SchedulerNodeRunRecord,
    failure: Option<SchedulerNodeFailureRecord>,
    activity: SchedulerNodeActivityProtocol,
) -> SchedulerNodeRunProtocol {
    SchedulerNodeRunProtocol {
        node_run_id: run.node_run_id.clone(),
        scheduler_run_id: run.scheduler_run_id.clone(),
        work_node_id: run.work_node_id.clone(),
        dispatch_id: run.dispatch_id.clone(),
        worker_agent_id: run.worker_agent_id.clone(),
        worker_run_id: run.worker_run_id.clone(),
        state: run.state.clone(),
        failure: failure.map(worker_failure_protocol),
        activity,
        started_at: run.started_at,
        completed_at: run.completed_at,
    }
}

fn empty_scheduler_node_activity() -> SchedulerNodeActivityProtocol {
    SchedulerNodeActivityProtocol {
        prompt_ref: None,
        stdout_ref: None,
        stderr_ref: None,
        stdout_tail: None,
        stderr_tail: None,
        trace: empty_scheduler_trace_activity(),
        recent_trace_events: Vec::new(),
        branch_path: None,
        branch_ref: None,
        changed_files: Vec::new(),
    }
}

fn empty_scheduler_trace_activity() -> SchedulerTraceActivityProtocol {
    SchedulerTraceActivityProtocol {
        sample_count: 0,
        latest_sequence: None,
        latest_event_kind: None,
        latest_occurred_at: None,
        samples: Vec::new(),
    }
}

fn trace_sample_protocol(
    event: crate::debug_trace::DebugTraceEventRecord,
) -> SchedulerTraceSampleProtocol {
    SchedulerTraceSampleProtocol {
        trace_event_id: event.trace_event_id,
        sequence: event.sequence,
        adapter: event.adapter,
        event_kind: event.event_kind,
        occurred_at: event.occurred_at,
        external_session_id: event.external_session_id,
        external_turn_id: event.external_turn_id,
        external_tool_id: event.external_tool_id,
        tool_name: summary_string(&event.summary, "tool_name"),
        tool_status: summary_string(&event.summary, "tool_status"),
        tool_input_preview: summary_string(&event.summary, "tool_input_preview"),
        tool_output_preview: summary_string(&event.summary, "tool_output_preview"),
        text_preview: summary_string(&event.summary, "text_preview"),
        session_status: summary_string(&event.summary, "session_status"),
        message_role: summary_string(&event.summary, "message_role"),
        part_type: summary_string(&event.summary, "part_type"),
        summary: event.summary,
    }
}

fn trace_sample_compat_summary(sample: &SchedulerTraceSampleProtocol) -> String {
    let summary = serde_json::to_string(&sample.summary).unwrap_or_default();
    truncate_chars(&format!("{}: {summary}", sample.event_kind), 300)
}

fn summary_string(summary: &serde_json::Value, key: &str) -> Option<String> {
    match summary.get(key) {
        Some(serde_json::Value::String(value)) => Some(value.clone()),
        Some(serde_json::Value::Number(value)) => Some(value.to_string()),
        Some(serde_json::Value::Bool(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn debug_run_ref(workspace: &Workspace, run_id: &str) -> String {
    workspace
        .debug_runs_dir()
        .join(run_id)
        .strip_prefix(&workspace.root)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| format!(".rive/debug/runs/{run_id}"))
}

fn read_tail_if_exists(path: &Path, max_lines: usize) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| tail_lines(&value, max_lines))
        .filter(|value| !value.trim().is_empty())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn changed_files_for_activity(parent: &Path, branch: &Path) -> Result<Vec<String>> {
    if !branch.exists() {
        return Ok(Vec::new());
    }
    let parent = file_digest_map_for_activity(parent)?;
    let branch = file_digest_map_for_activity(branch)?;
    let mut changed = parent
        .keys()
        .chain(branch.keys())
        .filter(|path| parent.get(*path) != branch.get(*path))
        .cloned()
        .collect::<Vec<_>>();
    changed.sort();
    changed.dedup();
    Ok(changed)
}

fn file_digest_map_for_activity(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = path_relative_to(entry.path(), root)?;
        if scheduler_activity_should_skip(&rel) {
            continue;
        }
        let bytes = fs::read(entry.path())?;
        files.insert(rel, sha256_hex(&bytes));
    }
    Ok(files)
}

fn scheduler_activity_should_skip(rel: &str) -> bool {
    matches!(
        rel.split('/').next().unwrap_or(""),
        ".rive" | ".git" | ".opencode" | "target"
    )
}

fn worker_failure_protocol(record: SchedulerNodeFailureRecord) -> WorkerFailureProtocol {
    WorkerFailureProtocol {
        failure_kind: record.failure_kind,
        retryable: record.retryable,
        suggested_action: record.suggested_action,
        detail: record.detail,
    }
}

fn classify_worker_failure(detail: &str) -> WorkerFailureProtocol {
    let lower = detail.to_lowercase();
    let (failure_kind, retryable, suggested_action) =
        if lower.contains("certificate") || lower.contains("x509") || lower.contains("tls") {
            ("certificate_error", true, "retry_after_certificate_fix")
        } else if lower.contains("network")
            || lower.contains("econnreset")
            || lower.contains("enotfound")
            || lower.contains("connection")
        {
            ("network_error", true, "retry_failed")
        } else if lower.contains("model") || lower.contains("unsupported") {
            ("model_error", false, "fix_model_or_runner")
        } else if lower.contains("not found")
            || lower.contains("no such file")
            || lower.contains("permission denied")
            || lower.contains("launch failed")
        {
            ("worker_environment_error", false, "fix_installation")
        } else if lower.contains("timeout") {
            ("timeout", true, "retry_with_longer_timeout")
        } else if lower.contains("dispatch not reported") {
            ("dispatch_not_reported", true, "retry_work")
        } else if lower.contains("worktree patch conflict")
            || lower.contains("worktree commit failed")
        {
            ("worktree_patch_conflict", false, "inspect_branch_conflict")
        } else if lower.contains("exit failed") {
            ("process_exit_failed", true, "inspect_runner_logs")
        } else {
            ("unknown_worker_failure", true, "inspect_runner_logs")
        };
    WorkerFailureProtocol {
        failure_kind: failure_kind.to_string(),
        retryable,
        suggested_action: suggested_action.to_string(),
        detail: detail.to_string(),
    }
}

struct SchedulerRequestHashInput<'a> {
    root_work_node_id: &'a str,
    runner: &'a str,
    worker_ids: &'a [String],
    max_parallel: usize,
    acceptance_mode: &'a str,
    workspace_mode: &'a str,
    timeout_seconds: u64,
    binary: Option<&'a Path>,
    trust_project: bool,
}

fn scheduler_request_hash(input: SchedulerRequestHashInput<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.root_work_node_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.runner.as_bytes());
    hasher.update(b"\0");
    for worker_id in input.worker_ids {
        hasher.update(worker_id.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(input.max_parallel.to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(input.acceptance_mode.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.workspace_mode.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.timeout_seconds.to_string().as_bytes());
    if let Some(binary) = input.binary {
        hasher.update(b"\0");
        hasher.update(binary.to_string_lossy().as_bytes());
    }
    hasher.update(b"\0");
    hasher.update(input.trust_project.to_string().as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn scheduler_binary_for_runner<'a>(
    runner: &RunnerKind,
    input: &'a SchedulerRunInput,
) -> Option<&'a Path> {
    match runner {
        RunnerKind::OpenCode => input.opencode_bin.as_deref(),
        RunnerKind::Codex => input.codex_bin.as_deref(),
    }
}

fn scheduler_binary_for_runner_resume<'a>(
    runner: &RunnerKind,
    input: &'a SchedulerResumeInput,
) -> Option<&'a Path> {
    match runner {
        RunnerKind::OpenCode => input.opencode_bin.as_deref(),
        RunnerKind::Codex => input.codex_bin.as_deref(),
    }
}

fn scheduler_task_body(
    workspace: &Workspace,
    node: &WorkNodeRecord,
    branch_ref: Option<&str>,
) -> Vec<u8> {
    let body_section = work_node_body_text(workspace, node);
    let branch_section = branch_ref
        .map(|branch_ref| {
            format!(
                "\nWorkspace ref:\n- ref: {branch_ref}\n- Use `--workspace-ref \"$RIVE_WORKSPACE_REF\"` when reporting.\n"
            )
        })
        .unwrap_or_default();
    format!(
        r#"You are a Rive worker assigned to one work node.

Editable workspace contract:
- `$RIVE_WORKSPACE` is the only source/artifact workspace you may edit.
- `$RIVE_STATE_WORKSPACE` is only for Rive ledger commands. Do not edit source files there.
- If `$RIVE_WORKSPACE` and `$RIVE_STATE_WORKSPACE` differ, capture evidence from `$RIVE_WORKSPACE`.

Work node:
- id: {}
- title: {}
- body:
{}
{}

Rules:
1. Read the work node body above as the authoritative objective and acceptance criteria.
2. Inspect your assigned node with `rive work inspect {}` if you need ledger context.
3. Make only implementation changes required for this node, under `$RIVE_WORKSPACE`.
4. Report with `team report --dispatch $RIVE_DISPATCH_ID --status done|blocked|failed --snapshot <id> --command-id <unique-id> --stdin`.
5. Include `--artifact-ref`, `--workspace-ref`, or `--diff-ref` when you create a result.
6. Do not mutate Work DAG topology.
7. Do not claim success in natural language without `team report`.
"#,
        node.work_node_id,
        node.title,
        body_section,
        branch_section,
        node.work_node_id
    )
    .into_bytes()
}

fn team_send_work_task_body(
    workspace: &Workspace,
    node: &WorkNodeRecord,
    request_body: &[u8],
) -> Vec<u8> {
    let node_body = work_node_body_text(workspace, node);
    let request_body = String::from_utf8_lossy(request_body);
    format!(
        r#"You are a Rive worker delegated to a specific Work node.

Work node:
- id: {}
- title: {}
- body:
{}

Delegation request:
{}

Rules:
1. Treat the Work node body as the authoritative objective and acceptance criteria.
2. Treat the delegation request as additional execution guidance.
3. Make source/artifact edits only under `$RIVE_WORKSPACE`.
4. Capture evidence and close the dispatch with `team report`; natural language completion is not enough.
"#,
        node.work_node_id, node.title, node_body, request_body
    )
    .into_bytes()
}

fn work_node_body_text(workspace: &Workspace, node: &WorkNodeRecord) -> String {
    let Some(blob_ref) = &node.body_blob_ref else {
        return "(empty)".to_string();
    };
    let path = workspace.root.join(blob_ref);
    match fs::read(&path) {
        Ok(bytes) if bytes.is_empty() => "(empty)".to_string(),
        Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        Err(error) => format!("(unreadable body_blob_ref {blob_ref}: {error})"),
    }
}

fn open_dispatch_nodes(store: &EventStore) -> Result<BTreeSet<String>> {
    let dispatches = store
        .list_dispatches()?
        .into_iter()
        .filter(|dispatch| matches!(dispatch.state, DispatchState::Open | DispatchState::Blocked))
        .map(|dispatch| dispatch.dispatch_id)
        .collect::<BTreeSet<_>>();
    Ok(store
        .list_work_dispatch_bindings()?
        .into_iter()
        .filter(|binding| dispatches.contains(&binding.dispatch_id))
        .map(|binding| binding.work_node_id)
        .collect())
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
            active_workspace: self.workspace.root.as_path(),
            branch_ref: None,
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
                workspace_override: None,
                branch_ref: None,
            })?;
            let dispatch_id = dispatch.dispatch_id.clone();
            let output = run_child_process_until_reported(
                &mut command,
                input.timeout_seconds,
                self.event_store,
                &dispatch_id,
            )?;
            exit_code = output.exit_code;
            fs::write(&stdout_path, &output.stdout)?;
            fs::write(&stderr_path, &output.stderr)?;
            if output.timed_out {
                return Err(anyhow!("{} timeout", self.adapter.kind()));
            }
            if output.exit_code.unwrap_or(1) != 0 {
                return Err(anyhow!(
                    "{} exit failed: {:?}: {}",
                    self.adapter.kind(),
                    output.exit_code,
                    process_failure_excerpt(&output)
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
            active_workspace: input
                .workspace_override
                .unwrap_or(self.workspace.root.as_path()),
            branch_ref: input.branch_ref,
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
            workspace_override: input.workspace_override,
            branch_ref: input.branch_ref,
        })?;
        let dispatch_id = input.dispatch.dispatch_id.clone();
        let output = run_child_process_until_reported(
            &mut command,
            input.timeout_seconds,
            self.event_store,
            &dispatch_id,
        )?;
        fs::write(&stdout_path, &output.stdout)?;
        fs::write(&stderr_path, &output.stderr)?;
        if output.timed_out {
            return Err(anyhow!("{} timeout", self.adapter.kind()));
        }
        if output.exit_code.unwrap_or(1) != 0 {
            return Err(anyhow!(
                "{} exit failed: {:?}: {}",
                self.adapter.kind(),
                output.exit_code,
                process_failure_excerpt(&output)
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
            return Err(anyhow!(
                "dispatch not reported: {}: {}",
                dispatch.dispatch_id,
                process_failure_excerpt(&output)
            ));
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
    workspace_override: Option<&'a Path>,
    branch_ref: Option<&'a str>,
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

        let bound_work_node = if let Some(work_node_id) = &input.work_node_id {
            let node = self
                .event_store
                .get_work_node(work_node_id)?
                .ok_or_else(|| anyhow!("work node not found: {work_node_id}"))?;
            let projection = work_service.inspect_projection(work_node_id)?;
            if !projection.allowed_next_actions.contains(&"delegate") {
                return Err(anyhow!(
                    "work node not ready: {work_node_id} is {}",
                    projection.state
                ));
            }
            Some(node)
        } else {
            None
        };
        let dispatch_body = bound_work_node
            .as_ref()
            .map(|node| team_send_work_task_body(self.workspace, node, &input.task_body))
            .unwrap_or_else(|| input.task_body.clone());

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
            body: dispatch_body.clone(),
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
                    task_body: &dispatch_body,
                    snapshot_paths: &input.snapshot_paths,
                    binary: input.opencode_bin.as_deref(),
                    timeout_seconds: input.timeout_seconds,
                    trust_project: false,
                    workspace_override: None,
                    branch_ref: None,
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
                    task_body: &dispatch_body,
                    snapshot_paths: &input.snapshot_paths,
                    binary: input.codex_bin.as_deref(),
                    timeout_seconds: input.timeout_seconds,
                    trust_project: input.trust_project,
                    workspace_override: None,
                    branch_ref: None,
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
            .current_dir(input.workspace_override.unwrap_or(&input.workspace.root))
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
            .arg(input.workspace_override.unwrap_or(&input.workspace.root));
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
    active_workspace: &'a Path,
    branch_ref: Option<&'a str>,
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
    workspace_override: Option<&'a Path>,
    branch_ref: Option<&'a str>,
}

fn prepare_isolated_codex_home(run_dir: &Path) -> Result<PathBuf> {
    let codex_home = run_dir.join("codex-home");
    fs::create_dir_all(&codex_home)?;
    let source_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")));
    if let Some(source_home) = source_home {
        copy_codex_model_config(&source_home, &codex_home)?;
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

fn copy_codex_model_config(source_home: &Path, codex_home: &Path) -> Result<()> {
    let source = source_home.join("config.toml");
    let target = codex_home.join("config.toml");
    if !source.exists() || target.exists() {
        return Ok(());
    }

    let source_config = fs::read_to_string(&source)?;
    let mut copied = String::new();
    for line in source_config.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            break;
        }
        if trimmed.starts_with("model") || trimmed.starts_with("personality") {
            copied.push_str(line);
            copied.push('\n');
        }
    }

    if !copied.trim().is_empty() {
        fs::write(target, copied)?;
    }
    Ok(())
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

fn process_failure_excerpt(output: &ProcessOutput) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = tail_lines(&stderr, 12);
    let stdout = tail_lines(&stdout, 12);
    match (stderr.trim().is_empty(), stdout.trim().is_empty()) {
        (false, false) => format!("stderr: {stderr}; stdout: {stdout}"),
        (false, true) => format!("stderr: {stderr}"),
        (true, false) => format!("stdout: {stdout}"),
        (true, true) => "no stdout/stderr captured".to_string(),
    }
}

fn tail_lines(value: &str, max_lines: usize) -> String {
    let mut lines = value.lines().rev().take(max_lines).collect::<Vec<_>>();
    lines.reverse();
    lines.join("\n")
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
    work_ref_binding_keys: BTreeSet<String>,
}

impl WorkspaceMutationBaseline {
    fn capture(workspace: &Workspace, store: &EventStore) -> Result<Self> {
        Ok(Self {
            files: workspace_file_hashes(workspace)?,
            work_ref_binding_keys: work_ref_binding_keys(store)?,
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
    let allowed =
        allowed_workspace_mutation_paths(workspace, store, &baseline.work_ref_binding_keys)?;
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
    baseline_ref_keys: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let mut allowed = BTreeSet::new();
    if !store.has_work_schema()? {
        return Ok(allowed);
    }
    for binding in store.list_work_ref_bindings()? {
        if baseline_ref_keys.contains(&work_ref_binding_key(&binding)) {
            continue;
        }
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

fn work_ref_binding_keys(store: &EventStore) -> Result<BTreeSet<String>> {
    if !store.has_work_schema()? {
        return Ok(BTreeSet::new());
    }
    Ok(store
        .list_work_ref_bindings()?
        .iter()
        .map(work_ref_binding_key)
        .collect::<BTreeSet<_>>())
}

fn work_ref_binding_key(binding: &WorkRefBindingRecord) -> String {
    format!(
        "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
        binding.work_node_id,
        binding.dispatch_id,
        binding.fact_event_id,
        binding.snapshot_id.as_deref().unwrap_or(""),
        binding.artifact_ref.as_deref().unwrap_or(""),
        binding.workspace_ref.as_deref().unwrap_or(""),
        binding.diff_ref.as_deref().unwrap_or("")
    )
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
    let worker_workspace = input.workspace_override.unwrap_or(&input.workspace.root);
    command
        .env_remove("RIVE_ORCHESTRATOR_ROOT_WORK_ID")
        .env_remove("RIVE_AVAILABLE_WORKERS")
        .env_remove("RIVE_ORCHESTRATOR_CAPABILITY_PROFILE")
        .env("RIVE_WORKSPACE", worker_workspace)
        .env("RIVE_STATE_WORKSPACE", &input.workspace.root)
        .env("RIVE_AGENT_ID", &input.agent.agent_id)
        .env("RIVE_AGENT_TOKEN", input.token)
        .env("RIVE_RUN_ID", input.run_id)
        .env("RIVE_DISPATCH_ID", &input.dispatch.dispatch_id)
        .env("PATH", path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(branch_ref) = input.branch_ref {
        command.env("RIVE_WORKSPACE_REF", branch_ref);
        command.env("RIVE_BRANCH_REF", branch_ref);
    } else {
        command.env_remove("RIVE_WORKSPACE_REF");
        command.env_remove("RIVE_BRANCH_REF");
    }
}

fn run_child_process(command: &mut Command, timeout_seconds: u64) -> Result<ProcessOutput> {
    run_child_process_until(command, timeout_seconds, || Ok(false))
}

fn run_child_process_until_reported(
    command: &mut Command,
    timeout_seconds: u64,
    event_store: &EventStore,
    dispatch_id: &str,
) -> Result<ProcessOutput> {
    run_child_process_until(command, timeout_seconds, || {
        let dispatch = event_store
            .get_dispatch(dispatch_id)?
            .ok_or_else(|| anyhow!("dispatch not found: {dispatch_id}"))?;
        Ok(!matches!(dispatch.state, DispatchState::Open))
    })
}

fn run_child_process_until<F>(
    command: &mut Command,
    timeout_seconds: u64,
    mut should_stop: F,
) -> Result<ProcessOutput>
where
    F: FnMut() -> Result<bool>,
{
    #[cfg(unix)]
    {
        command.process_group(0);
    }
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
        if should_stop()? {
            terminate_child(&mut child);
            let output = child.wait_with_output()?;
            return Ok(ProcessOutput {
                exit_code: output.status.code().or(Some(0)),
                stdout: output.stdout,
                stderr: output.stderr,
                timed_out: false,
            });
        }
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
            terminate_child(&mut child);
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

fn terminate_child(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    #[cfg(unix)]
    {
        let process_group = format!("-{}", child.id());
        let _ = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(&process_group)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        thread::sleep(Duration::from_millis(200));
        if matches!(child.try_wait(), Ok(None)) {
            let _ = Command::new("/bin/kill")
                .arg("-KILL")
                .arg(&process_group)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

fn build_common_prompt(context: RunnerPromptContext<'_>, adapter_hints: Option<&str>) -> String {
    let body = String::from_utf8_lossy(context.body);
    let branch_instructions = context
        .branch_ref
        .map(|branch_ref| {
            format!(
                "\nWorkspace ref:\n- ref: {branch_ref}\n- When reporting, include `--workspace-ref \"$RIVE_WORKSPACE_REF\"`.\n"
            )
        })
        .unwrap_or_default();
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

Workspace contract:
- editable_root: {active_workspace}
- state_root: {state_workspace}
- `$RIVE_WORKSPACE` points to editable_root. Make all source/artifact edits there.
- `$RIVE_STATE_WORKSPACE` points to state_root. Use it only implicitly through `rive`/`team` commands.
- Do not edit source files under state_root when editable_root differs.

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
- If a workspace ref is provided, include it in the final `team report`.
- A natural language final answer is not a Rive report.

{branch_instructions}
{snapshot_instructions}
{adapter_hints}
Task:
{body}
"#,
        active_workspace = context.active_workspace.display(),
        state_workspace = context.workspace.root.display(),
        agent_id = context.agent.agent_id,
        agent_name = context.agent.name,
        dispatch_id = context.dispatch.dispatch_id,
        title = context.title,
        branch_instructions = branch_instructions,
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
            active_workspace: workspace.root.as_path(),
            branch_ref: None,
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
