use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::snapshot::SnapshotStore;
use crate::store::{
    EventStore, InsertWorkflowImportCommandInput, InsertWorkflowRunInput,
    InsertWorkflowRunNodeInput, SchedulerNodeRunRecord, SchedulerRunRecord,
    UpdateWorkflowRunSchedulerInput, UpsertWorkflowTemplateInput, WorkflowRunNodeRecord,
    WorkflowRunRecord, WorkflowTemplateRecord, WorkflowTemplateVersionRecord,
};
use crate::work::{
    AddWorkEdgeInput, BindWorkRootCommand, CreateWorkNodeInput, WorkEdgeType, WorkNodeKind,
    WorkProjectionProtocol, WorkService,
};
use crate::workspace::Workspace;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WorkflowSpec {
    pub api_version: String,
    pub id: String,
    pub version: i64,
    pub title: String,
    #[serde(default)]
    pub params: BTreeMap<String, ParamSpec>,
    #[serde(default)]
    pub defaults: Value,
    pub nodes: BTreeMap<String, NodeSpec>,
    pub edges: Vec<EdgeSpec>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ParamSpec {
    #[serde(rename = "type")]
    pub param_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub default: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NodeSpec {
    pub kind: String,
    #[serde(default)]
    pub runner: Option<String>,
    pub title: String,
    pub prompt: PromptSpec,
    #[serde(default)]
    pub consumes: Vec<String>,
    #[serde(default)]
    pub capability_policy: Value,
    pub output_contract: Value,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PromptSpec {
    #[serde(default)]
    pub inline: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EdgeSpec {
    #[serde(rename = "type")]
    pub edge_type: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct WorkflowValidationProtocol {
    pub template_id: String,
    pub version: i64,
    pub title: String,
    pub template_hash: String,
    pub node_count: usize,
    pub edge_count: usize,
    pub source_ref: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowImportProtocol {
    pub template_id: String,
    pub version: i64,
    pub source_version: i64,
    pub template_hash: String,
    pub title: String,
    pub source_ref: String,
    pub bump_if_changed: bool,
    pub idempotency_status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct WorkflowTemplateListProtocol {
    pub templates: Vec<WorkflowTemplateSummaryProtocol>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowTemplateSummaryProtocol {
    pub template_id: String,
    pub latest_version: i64,
    pub latest_hash: String,
    pub title: String,
    pub source_ref: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowTemplateShowProtocol {
    pub template_id: String,
    pub version: i64,
    pub template_hash: String,
    pub title: String,
    pub source_ref: String,
    pub spec: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowRunProtocol {
    pub workflow_run_id: String,
    pub template_id: String,
    pub template_version: i64,
    pub template_hash: String,
    pub params_hash: String,
    pub root_work_node_id: String,
    pub state: String,
    pub stored_state: String,
    pub effective_state_reason: String,
    pub idempotency_status: &'static str,
    pub scheduler: Option<WorkflowRunSchedulerProtocol>,
    pub nodes: Vec<WorkflowRunNodeProtocol>,
    pub root_projection: WorkProjectionProtocol,
    pub debug: WorkflowRunDebugProtocol,
}

#[derive(Debug, Serialize)]
pub struct WorkflowRunSchedulerProtocol {
    pub scheduler_run_id: String,
    pub runner: String,
    pub state: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowRunNodeProtocol {
    pub node_template_id: String,
    pub work_node_id: String,
    pub output_contract: Value,
    pub capability_policy: Value,
}

#[derive(Debug, Serialize)]
pub struct WorkflowRunDebugProtocol {
    pub summary: String,
    pub scheduler_node_runs: Vec<WorkflowRunDebugNodeProtocol>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowRunDebugNodeProtocol {
    pub node_run_id: String,
    pub work_node_id: String,
    pub dispatch_id: Option<String>,
    pub worker_agent_id: String,
    pub worker_run_id: Option<String>,
    pub state: String,
    pub stdout_ref: Option<String>,
    pub stderr_ref: Option<String>,
    pub prompt_ref: Option<String>,
}

#[derive(Debug)]
pub struct WorkflowImportInput {
    pub path: PathBuf,
    pub command_id: String,
    pub bump_if_changed: bool,
}

#[derive(Debug)]
pub struct WorkflowRunInput {
    pub template_id: String,
    pub command_id: String,
    pub params: Vec<(String, String)>,
    pub scheduler_request: Option<WorkflowSchedulerRequest>,
}

#[derive(Debug)]
pub struct WorkflowStatusInput {
    pub workflow_run_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowSchedulerRequest {
    pub runner: String,
    pub workers: Vec<String>,
    pub max_parallel: usize,
    pub acceptance_mode: String,
    pub workspace_mode: String,
    pub timeout_seconds: u64,
    pub opencode_bin: Option<PathBuf>,
    pub codex_bin: Option<PathBuf>,
    pub trust_project: bool,
}

pub struct WorkflowService<'a, B: SnapshotStore> {
    workspace: &'a Workspace,
    store: &'a EventStore,
    blob_store: &'a B,
}

impl<'a, B: SnapshotStore> WorkflowService<'a, B> {
    pub fn new(workspace: &'a Workspace, store: &'a EventStore, blob_store: &'a B) -> Self {
        Self {
            workspace,
            store,
            blob_store,
        }
    }

    pub fn validate_path(&self, path: &Path) -> Result<WorkflowValidationProtocol> {
        let loaded = LoadedWorkflow::load(path)?;
        Ok(loaded.validation_protocol())
    }

    pub fn import(&self, input: WorkflowImportInput) -> Result<WorkflowImportProtocol> {
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        self.store.init_work_schema()?;
        let loaded = LoadedWorkflow::load(&input.path)?;
        let target_version = self.import_target_version(&loaded, input.bump_if_changed)?;
        let request_hash = import_request_hash(
            &loaded.spec.id,
            target_version,
            &loaded.template_hash,
            input.bump_if_changed,
        );
        if let Some(existing) = self.store.get_workflow_import_command(&input.command_id)? {
            let (template_id, version, template_hash, existing_request_hash) = existing;
            if template_id == loaded.spec.id
                && version == target_version
                && template_hash == loaded.template_hash
                && import_request_hash_matches(
                    &existing_request_hash,
                    &request_hash,
                    &loaded.template_hash,
                    input.bump_if_changed,
                )
            {
                return Ok(WorkflowImportProtocol {
                    template_id,
                    version,
                    source_version: loaded.spec.version,
                    template_hash,
                    title: loaded.spec.title,
                    source_ref: loaded.source_ref,
                    bump_if_changed: input.bump_if_changed,
                    idempotency_status: "replayed",
                });
            }
            return Err(anyhow!("idempotency conflict"));
        }
        let now = Utc::now();
        self.store
            .upsert_workflow_template(&UpsertWorkflowTemplateInput {
                template_id: loaded.spec.id.clone(),
                version: target_version,
                template_hash: loaded.template_hash.clone(),
                title: loaded.spec.title.clone(),
                source_ref: loaded.source_ref.clone(),
                spec_json: loaded.normalized_spec.clone(),
                created_at: now,
            })?;
        self.store
            .insert_workflow_import_command(&InsertWorkflowImportCommandInput {
                command_id: input.command_id,
                template_id: loaded.spec.id.clone(),
                version: target_version,
                template_hash: loaded.template_hash.clone(),
                request_hash,
                created_at: now,
            })?;
        Ok(WorkflowImportProtocol {
            template_id: loaded.spec.id,
            version: target_version,
            source_version: loaded.spec.version,
            template_hash: loaded.template_hash,
            title: loaded.spec.title,
            source_ref: loaded.source_ref,
            bump_if_changed: input.bump_if_changed,
            idempotency_status: "inserted",
        })
    }

    fn import_target_version(&self, loaded: &LoadedWorkflow, bump_if_changed: bool) -> Result<i64> {
        if !bump_if_changed {
            return Ok(loaded.spec.version);
        }
        if let Some(existing_hash) = self
            .store
            .get_workflow_template_version_by_hash(&loaded.spec.id, &loaded.template_hash)?
        {
            return Ok(existing_hash.version);
        }
        if let Some(existing_version) = self
            .store
            .get_workflow_template_version(&loaded.spec.id, loaded.spec.version)?
        {
            if existing_version.template_hash != loaded.template_hash {
                let latest = self
                    .store
                    .get_workflow_template(&loaded.spec.id)?
                    .map(|template| template.latest_version)
                    .unwrap_or(loaded.spec.version);
                return Ok(latest + 1);
            }
        }
        Ok(loaded.spec.version)
    }

    pub fn list(&self) -> Result<WorkflowTemplateListProtocol> {
        self.store.init_work_schema()?;
        let templates = self
            .store
            .list_workflow_templates()?
            .into_iter()
            .map(workflow_template_summary_protocol)
            .collect();
        Ok(WorkflowTemplateListProtocol { templates })
    }

    pub fn show(
        &self,
        template_id: &str,
        version: Option<i64>,
    ) -> Result<WorkflowTemplateShowProtocol> {
        self.store.init_work_schema()?;
        let record = if let Some(version) = version {
            self.store
                .get_workflow_template_version(template_id, version)?
        } else {
            self.store
                .get_latest_workflow_template_version(template_id)?
        }
        .ok_or_else(|| anyhow!("workflow template not found: {template_id}"))?;
        Ok(workflow_template_show_protocol(&record))
    }

    pub fn run(&self, input: WorkflowRunInput) -> Result<WorkflowRunProtocol> {
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        self.store.init_work_schema()?;
        let version = self
            .store
            .get_latest_workflow_template_version(&input.template_id)?
            .ok_or_else(|| anyhow!("workflow template not found: {}", input.template_id))?;
        let spec = spec_from_version(&version)?;
        let params = resolve_params(&spec, &input.params)?;
        let params_hash = hash_json(&params)?;
        let request_hash = request_hash_for_run(&version, &params_hash, &input.scheduler_request)?;
        if let Some(existing) = self
            .store
            .get_workflow_run_by_command_id(&input.command_id)?
        {
            if existing.template_id == version.template_id
                && existing.template_version == version.version
                && existing.template_hash == version.template_hash
                && existing.params_hash == params_hash
                && existing.request_hash == request_hash
            {
                return self.run_protocol(existing, "replayed");
            }
            return Err(anyhow!("idempotency conflict"));
        }

        let work_service = WorkService::new(self.workspace, self.store, self.blob_store);
        let run_id = prefixed_id("wfrun");
        let root_body = format!(
            "Workflow template: {}@{}\nTemplate hash: {}\nParams: {}\n",
            version.template_id,
            version.version,
            version.template_hash,
            serde_json::to_string_pretty(&params)?
        );
        let (root, _) = work_service.create_node(CreateWorkNodeInput {
            command_id: format!("workflow:{}:root", input.command_id),
            kind: WorkNodeKind::Objective,
            title: render_text(&spec.title, &params, &BTreeMap::new())?,
            body: root_body.into_bytes(),
        })?;
        work_service.bind_root(BindWorkRootCommand {
            root_work_node_id: root.work_node_id.clone(),
            work_node_id: root.work_node_id.clone(),
            created_by_agent_id: None,
            created_by_run_id: Some(run_id.clone()),
        })?;

        let mut node_map = HashMap::new();
        for (node_id, node) in &spec.nodes {
            let local_params = prompt_local_params(node);
            let title = render_text(&node.title, &params, &local_params)?;
            let body = render_node_body(node_id, node, &params)?;
            let (work_node, _) = work_service.create_node(CreateWorkNodeInput {
                command_id: format!("workflow:{}:node:{node_id}", input.command_id),
                kind: WorkNodeKind::parse(&node.kind)?,
                title,
                body: body.into_bytes(),
            })?;
            work_service.bind_root(BindWorkRootCommand {
                root_work_node_id: root.work_node_id.clone(),
                work_node_id: work_node.work_node_id.clone(),
                created_by_agent_id: None,
                created_by_run_id: Some(run_id.clone()),
            })?;
            node_map.insert(node_id.clone(), work_node.work_node_id.clone());
        }

        for edge in &spec.edges {
            let from = resolve_edge_endpoint(&edge.from, &root.work_node_id, &node_map)?;
            let to = resolve_edge_endpoint(&edge.to, &root.work_node_id, &node_map)?;
            work_service.add_edge(AddWorkEdgeInput {
                command_id: format!(
                    "workflow:{}:edge:{}:{}:{}",
                    input.command_id, edge.edge_type, edge.from, edge.to
                ),
                edge_type: WorkEdgeType::parse(&edge.edge_type)?,
                from_node_id: from,
                to_node_id: to,
            })?;
        }

        let now = Utc::now();
        let run = self.store.insert_workflow_run(&InsertWorkflowRunInput {
            workflow_run_id: run_id.clone(),
            command_id: input.command_id.clone(),
            template_id: version.template_id.clone(),
            template_version: version.version,
            template_hash: version.template_hash.clone(),
            params_json: params.clone(),
            params_hash,
            request_hash,
            root_work_node_id: root.work_node_id.clone(),
            scheduler_run_id: None,
            state: "instantiated".to_string(),
            created_at: now,
        })?;

        for (node_id, work_node_id) in node_map {
            let node = spec.nodes.get(&node_id).expect("node exists");
            self.store
                .insert_workflow_run_node(&InsertWorkflowRunNodeInput {
                    workflow_run_id: run.workflow_run_id.clone(),
                    node_template_id: node_id,
                    work_node_id,
                    output_contract_json: node.output_contract.clone(),
                    capability_policy_json: node.capability_policy.clone(),
                })?;
        }

        self.run_protocol(run, "inserted")
    }

    pub fn attach_scheduler(
        &self,
        workflow_run_id: &str,
        scheduler_run_id: &str,
        state: &str,
        idempotency_status: &'static str,
    ) -> Result<WorkflowRunProtocol> {
        self.store.init_work_schema()?;
        let run = self
            .store
            .update_workflow_run_scheduler(&UpdateWorkflowRunSchedulerInput {
                workflow_run_id: workflow_run_id.to_string(),
                scheduler_run_id: scheduler_run_id.to_string(),
                state: state.to_string(),
            })?;
        self.run_protocol(run, idempotency_status)
    }

    pub fn status(&self, input: WorkflowStatusInput) -> Result<WorkflowRunProtocol> {
        self.store.init_work_schema()?;
        let run = self
            .store
            .get_workflow_run(&input.workflow_run_id)?
            .ok_or_else(|| anyhow!("workflow run not found: {}", input.workflow_run_id))?;
        self.run_protocol(run, "status")
    }

    fn run_protocol(
        &self,
        mut run: WorkflowRunRecord,
        idempotency_status: &'static str,
    ) -> Result<WorkflowRunProtocol> {
        let work_service = WorkService::new(self.workspace, self.store, self.blob_store);
        let root_projection = work_service.inspect_projection(&run.root_work_node_id)?;
        let nodes = self
            .store
            .list_workflow_run_nodes(&run.workflow_run_id)?
            .into_iter()
            .map(workflow_run_node_protocol)
            .collect();
        let scheduler_record = if let Some(scheduler_run_id) = &run.scheduler_run_id {
            self.store.get_scheduler_run(scheduler_run_id)?
        } else {
            self.store
                .latest_scheduler_run_for_root(&run.root_work_node_id)?
        };
        let node_runs = if let Some(scheduler) = &scheduler_record {
            self.store
                .list_scheduler_node_runs_for_scheduler(&scheduler.scheduler_run_id)?
        } else {
            Vec::new()
        };
        let (effective_state, effective_state_reason) =
            effective_workflow_state(&run, scheduler_record.as_ref(), &root_projection);
        let stored_state = run.state.clone();
        if effective_state != run.state {
            run = self.store.update_workflow_run_state(
                &run.workflow_run_id,
                &effective_state,
                workflow_completed_at(&effective_state),
            )?;
        };
        let scheduler = scheduler_record.map(|scheduler| WorkflowRunSchedulerProtocol {
            scheduler_run_id: scheduler.scheduler_run_id,
            runner: scheduler.runner,
            state: scheduler.state,
        });
        let debug = workflow_run_debug_protocol(self.workspace, scheduler.as_ref(), &node_runs);
        Ok(WorkflowRunProtocol {
            workflow_run_id: run.workflow_run_id,
            template_id: run.template_id,
            template_version: run.template_version,
            template_hash: run.template_hash,
            params_hash: run.params_hash,
            root_work_node_id: run.root_work_node_id,
            state: effective_state,
            stored_state,
            effective_state_reason,
            idempotency_status,
            scheduler,
            nodes,
            root_projection,
            debug,
        })
    }
}

fn effective_workflow_state(
    run: &WorkflowRunRecord,
    scheduler: Option<&SchedulerRunRecord>,
    root_projection: &WorkProjectionProtocol,
) -> (String, String) {
    if root_projection.state == "done" {
        return ("completed".to_string(), "root_projection_done".to_string());
    }
    if let Some(scheduler) = scheduler {
        return match scheduler.state.as_str() {
            "failed" => ("failed".to_string(), "scheduler_failed".to_string()),
            "stalled" => ("stalled".to_string(), "scheduler_stalled".to_string()),
            "waiting_review" => (
                "waiting_review".to_string(),
                "scheduler_waiting_review".to_string(),
            ),
            "running" => ("running".to_string(), "scheduler_running".to_string()),
            "completed" => (
                "stalled".to_string(),
                format!("scheduler_completed_root_{}", root_projection.state),
            ),
            other => (other.to_string(), "scheduler_state".to_string()),
        };
    }
    (run.state.clone(), "stored_state".to_string())
}

fn workflow_completed_at(state: &str) -> Option<DateTime<Utc>> {
    matches!(state, "completed" | "failed" | "stalled").then(Utc::now)
}

fn workflow_run_debug_protocol(
    workspace: &Workspace,
    scheduler: Option<&WorkflowRunSchedulerProtocol>,
    node_runs: &[SchedulerNodeRunRecord],
) -> WorkflowRunDebugProtocol {
    let scheduler_runner = scheduler.map(|scheduler| scheduler.runner.as_str());
    let failed = node_runs.iter().filter(|run| run.state == "failed").count();
    let active = node_runs
        .iter()
        .filter(|run| matches!(run.state.as_str(), "claimed" | "running"))
        .count();
    let summary = if failed > 0 {
        "scheduler has failed node runs; inspect each node's stdout_ref/stderr_ref/prompt_ref"
            .to_string()
    } else if active > 0 {
        "scheduler has active node runs".to_string()
    } else if node_runs.is_empty() {
        "no scheduler node runs recorded".to_string()
    } else {
        "scheduler node runs recorded".to_string()
    };
    WorkflowRunDebugProtocol {
        summary,
        scheduler_node_runs: node_runs
            .iter()
            .map(|run| workflow_debug_node_protocol(workspace, scheduler_runner, run))
            .collect(),
    }
}

fn workflow_debug_node_protocol(
    workspace: &Workspace,
    scheduler_runner: Option<&str>,
    run: &SchedulerNodeRunRecord,
) -> WorkflowRunDebugNodeProtocol {
    let stdout_file = match scheduler_runner {
        Some("opencode") | Some("codex") => "stdout.jsonl",
        _ => "stdout.jsonl",
    };
    let debug_ref = |file_name: &str| {
        run.worker_run_id
            .as_ref()
            .map(|run_id| format!("{}/{}", debug_run_ref(workspace, run_id), file_name))
    };
    WorkflowRunDebugNodeProtocol {
        node_run_id: run.node_run_id.clone(),
        work_node_id: run.work_node_id.clone(),
        dispatch_id: run.dispatch_id.clone(),
        worker_agent_id: run.worker_agent_id.clone(),
        worker_run_id: run.worker_run_id.clone(),
        state: run.state.clone(),
        stdout_ref: debug_ref(stdout_file),
        stderr_ref: debug_ref("stderr.log"),
        prompt_ref: debug_ref("prompt.txt"),
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

fn import_request_hash(
    template_id: &str,
    version: i64,
    template_hash: &str,
    bump_if_changed: bool,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(template_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(version.to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(template_hash.as_bytes());
    hasher.update(b"\0");
    hasher.update(if bump_if_changed {
        b"bump".as_slice()
    } else {
        b"fixed".as_slice()
    });
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn import_request_hash_matches(
    existing: &str,
    current: &str,
    legacy_template_hash: &str,
    bump_if_changed: bool,
) -> bool {
    existing == current || (!bump_if_changed && existing == legacy_template_hash)
}

struct LoadedWorkflow {
    spec: WorkflowSpec,
    normalized_spec: Value,
    template_hash: String,
    source_ref: String,
}

impl LoadedWorkflow {
    fn load(path: &Path) -> Result<Self> {
        let (workflow_path, package_root) = if path.is_dir() {
            (path.join("workflow.yaml"), path.to_path_buf())
        } else {
            (
                path.to_path_buf(),
                path.parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from(".")),
            )
        };
        let yaml = fs::read_to_string(&workflow_path)
            .with_context(|| format!("read workflow {}", workflow_path.display()))?;
        let mut spec: WorkflowSpec = serde_yaml::from_str(&yaml)
            .with_context(|| format!("parse workflow {}", workflow_path.display()))?;
        resolve_prompt_files(&mut spec, &package_root)?;
        validate_spec(&spec)?;
        let normalized_spec = serde_json::to_value(&spec)?;
        let template_hash = hash_json(&normalized_spec)?;
        Ok(Self {
            spec,
            normalized_spec,
            template_hash,
            source_ref: workflow_path.display().to_string(),
        })
    }

    fn validation_protocol(&self) -> WorkflowValidationProtocol {
        WorkflowValidationProtocol {
            template_id: self.spec.id.clone(),
            version: self.spec.version,
            title: self.spec.title.clone(),
            template_hash: self.template_hash.clone(),
            node_count: self.spec.nodes.len(),
            edge_count: self.spec.edges.len(),
            source_ref: self.source_ref.clone(),
        }
    }
}

fn resolve_prompt_files(spec: &mut WorkflowSpec, package_root: &Path) -> Result<()> {
    for (node_id, node) in &mut spec.nodes {
        if node.prompt.inline.is_none() {
            if let Some(file) = &node.prompt.file {
                let prompt_path = package_root.join(file);
                let content = fs::read_to_string(&prompt_path).with_context(|| {
                    format!("read prompt for node {node_id}: {}", prompt_path.display())
                })?;
                node.prompt.inline = Some(content);
            }
        }
    }
    Ok(())
}

fn validate_spec(spec: &WorkflowSpec) -> Result<()> {
    if spec.api_version != "rive.workflow/v0" {
        return Err(anyhow!(
            "unsupported workflow api version: {}",
            spec.api_version
        ));
    }
    validate_id("workflow id", &spec.id)?;
    if spec.version < 1 {
        return Err(anyhow!("workflow version must be positive"));
    }
    if spec.nodes.is_empty() {
        return Err(anyhow!("workflow nodes are required"));
    }
    for (name, param) in &spec.params {
        validate_id("workflow param", name)?;
        match param.param_type.as_str() {
            "string" | "duration" | "integer" | "boolean" => {}
            "enum" => {
                if param.values.is_empty() {
                    return Err(anyhow!("workflow enum param values required: {name}"));
                }
            }
            other => return Err(anyhow!("unsupported workflow param type: {other}")),
        }
    }
    for (node_id, node) in &spec.nodes {
        validate_id("workflow node id", node_id)?;
        WorkNodeKind::parse(&node.kind)?;
        if node
            .prompt
            .inline
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return Err(anyhow!("workflow prompt missing for node: {node_id}"));
        }
        if node.output_contract.is_null() {
            return Err(anyhow!(
                "workflow output contract missing for node: {node_id}"
            ));
        }
        validate_gated_capabilities(spec, node_id, &node.capability_policy)?;
    }
    for edge in &spec.edges {
        WorkEdgeType::parse(&edge.edge_type)?;
        validate_endpoint(spec, &edge.from)?;
        validate_endpoint(spec, &edge.to)?;
    }
    validate_consumes_dependencies(spec)?;
    ensure_dag(spec)?;
    Ok(())
}

fn validate_id(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(anyhow!("invalid {label}: {value}"));
    }
    Ok(())
}

fn validate_endpoint(spec: &WorkflowSpec, endpoint: &str) -> Result<()> {
    if endpoint == "root" || spec.nodes.contains_key(endpoint) {
        Ok(())
    } else {
        Err(anyhow!("workflow edge endpoint not found: {endpoint}"))
    }
}

fn validate_consumes_dependencies(spec: &WorkflowSpec) -> Result<()> {
    let dependency_index = depends_on_adjacency(spec);
    for (node_id, node) in &spec.nodes {
        let dependency_closure = dependency_closure(node_id, &dependency_index)?;
        for consumed in &node.consumes {
            if !spec.nodes.contains_key(consumed) {
                return Err(anyhow!(
                    "workflow consumes unknown node: {node_id} -> {consumed}"
                ));
            }
            if !dependency_closure.contains(consumed.as_str()) {
                return Err(anyhow!(
                    "workflow consumes must be dependency predecessor: {node_id} -> {consumed}"
                ));
            }
        }
    }
    Ok(())
}

fn depends_on_adjacency(spec: &WorkflowSpec) -> HashMap<&str, Vec<&str>> {
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &spec.edges {
        if edge.edge_type.replace('-', "_") == "depends_on" {
            adjacency
                .entry(edge.from.as_str())
                .or_default()
                .push(edge.to.as_str());
        }
    }
    adjacency
}

fn dependency_closure<'a>(
    node_id: &'a str,
    adjacency: &HashMap<&'a str, Vec<&'a str>>,
) -> Result<HashSet<&'a str>> {
    let mut closure = HashSet::new();
    let mut visiting = HashSet::new();
    fn visit<'a>(
        node_id: &'a str,
        adjacency: &HashMap<&'a str, Vec<&'a str>>,
        closure: &mut HashSet<&'a str>,
        visiting: &mut HashSet<&'a str>,
    ) -> Result<()> {
        if visiting.contains(node_id) {
            return Err(anyhow!("workflow graph cycle"));
        }
        visiting.insert(node_id);
        if let Some(dependencies) = adjacency.get(node_id) {
            for dependency in dependencies {
                if closure.insert(dependency) {
                    visit(dependency, adjacency, closure, visiting)?;
                }
            }
        }
        visiting.remove(node_id);
        Ok(())
    }
    visit(node_id, adjacency, &mut closure, &mut visiting)?;
    Ok(closure)
}

fn validate_gated_capabilities(spec: &WorkflowSpec, node_id: &str, policy: &Value) -> Result<()> {
    let Some(gated) = policy.get("gated_allow").and_then(Value::as_object) else {
        return Ok(());
    };
    for (capability, gate) in gated {
        let Some(gate_text) = gate.as_str() else {
            return Err(anyhow!(
                "workflow gated capability must reference boolean param: {node_id}.{capability}"
            ));
        };
        let param = gate_text
            .strip_prefix("{{")
            .and_then(|value| value.strip_suffix("}}"))
            .map(str::trim)
            .ok_or_else(|| {
                anyhow!(
                    "workflow gated capability must use {{param}} syntax: {node_id}.{capability}"
                )
            })?;
        match spec.params.get(param) {
            Some(spec) if spec.param_type == "boolean" => {}
            _ => {
                return Err(anyhow!(
                    "workflow gated capability must use boolean param: {node_id}.{capability}"
                ))
            }
        }
    }
    Ok(())
}

fn ensure_dag(spec: &WorkflowSpec) -> Result<()> {
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &spec.edges {
        adjacency
            .entry(edge.from.as_str())
            .or_default()
            .push(edge.to.as_str());
    }
    fn visit<'a>(
        node: &'a str,
        adjacency: &HashMap<&'a str, Vec<&'a str>>,
        visiting: &mut HashSet<&'a str>,
        visited: &mut HashSet<&'a str>,
    ) -> Result<()> {
        if visiting.contains(node) {
            return Err(anyhow!("workflow graph cycle"));
        }
        if visited.contains(node) {
            return Ok(());
        }
        visiting.insert(node);
        if let Some(next) = adjacency.get(node) {
            for child in next {
                visit(child, adjacency, visiting, visited)?;
            }
        }
        visiting.remove(node);
        visited.insert(node);
        Ok(())
    }
    let mut starts = BTreeSet::new();
    starts.insert("root");
    for node_id in spec.nodes.keys() {
        starts.insert(node_id.as_str());
    }
    for start in starts {
        visit(start, &adjacency, &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn resolve_params(spec: &WorkflowSpec, provided: &[(String, String)]) -> Result<Value> {
    let mut values = serde_json::Map::new();
    let provided_map = provided.iter().cloned().collect::<BTreeMap<_, _>>();
    for (key, value) in &provided_map {
        if !spec.params.contains_key(key) {
            return Err(anyhow!("workflow unknown param: {key}"));
        }
        values.insert(
            key.clone(),
            parse_param_value(spec.params.get(key).unwrap(), value)?,
        );
    }
    for (key, param) in &spec.params {
        if values.contains_key(key) {
            continue;
        }
        if let Some(default) = &param.default {
            values.insert(key.clone(), default.clone());
        } else if param.required {
            return Err(anyhow!("workflow missing param: {key}"));
        }
    }
    Ok(Value::Object(values))
}

fn parse_param_value(spec: &ParamSpec, value: &str) -> Result<Value> {
    match spec.param_type.as_str() {
        "string" | "duration" => Ok(Value::String(value.to_string())),
        "enum" => {
            if spec.values.iter().any(|allowed| allowed == value) {
                Ok(Value::String(value.to_string()))
            } else {
                Err(anyhow!("workflow invalid enum param: {value}"))
            }
        }
        "integer" => Ok(Value::Number(value.parse::<i64>()?.into())),
        "boolean" => match value {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(anyhow!("workflow invalid boolean param: {value}")),
        },
        _ => Err(anyhow!(
            "unsupported workflow param type: {}",
            spec.param_type
        )),
    }
}

fn render_node_body(node_id: &str, node: &NodeSpec, params: &Value) -> Result<String> {
    let local_params = prompt_local_params(node);
    let prompt = node.prompt.inline.as_deref().unwrap_or_default();
    let rendered_prompt = render_text(prompt, params, &local_params)?;
    Ok(format!(
        "Workflow node template: {node_id}\nRunner: {}\nConsumes: {}\n\nPrompt:\n{}\n\nOutput contract:\n{}\n\nCapability policy:\n{}\n",
        node.runner.as_deref().unwrap_or("opencode"),
        serde_json::to_string(&node.consumes)?,
        rendered_prompt,
        serde_json::to_string_pretty(&node.output_contract)?,
        serde_json::to_string_pretty(&node.capability_policy)?,
    ))
}

fn prompt_local_params(node: &NodeSpec) -> BTreeMap<String, Value> {
    node.prompt.params.clone()
}

fn render_text(
    text: &str,
    params: &Value,
    local_params: &BTreeMap<String, Value>,
) -> Result<String> {
    let mut rendered = text.to_string();
    let object = params
        .as_object()
        .ok_or_else(|| anyhow!("workflow params invalid"))?;
    for (key, value) in object {
        rendered = rendered.replace(&format!("{{{{{key}}}}}"), &render_value(value));
    }
    for (key, value) in local_params {
        rendered = rendered.replace(&format!("{{{{{key}}}}}"), &render_value(value));
    }
    if rendered.contains("{{") {
        return Err(anyhow!("workflow unresolved template variable"));
    }
    Ok(rendered)
}

fn render_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        _ => value.to_string(),
    }
}

fn resolve_edge_endpoint(
    endpoint: &str,
    root_work_node_id: &str,
    node_map: &HashMap<String, String>,
) -> Result<String> {
    if endpoint == "root" {
        Ok(root_work_node_id.to_string())
    } else {
        node_map
            .get(endpoint)
            .cloned()
            .ok_or_else(|| anyhow!("workflow edge endpoint not found: {endpoint}"))
    }
}

fn spec_from_version(version: &WorkflowTemplateVersionRecord) -> Result<WorkflowSpec> {
    serde_json::from_value(version.spec_json.clone()).map_err(Into::into)
}

fn workflow_template_summary_protocol(
    record: WorkflowTemplateRecord,
) -> WorkflowTemplateSummaryProtocol {
    WorkflowTemplateSummaryProtocol {
        template_id: record.template_id,
        latest_version: record.latest_version,
        latest_hash: record.latest_hash,
        title: record.title,
        source_ref: record.source_ref,
        updated_at: record.updated_at,
    }
}

fn workflow_template_show_protocol(
    record: &WorkflowTemplateVersionRecord,
) -> WorkflowTemplateShowProtocol {
    let title = record
        .spec_json
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    WorkflowTemplateShowProtocol {
        template_id: record.template_id.clone(),
        version: record.version,
        template_hash: record.template_hash.clone(),
        title,
        source_ref: record.source_ref.clone(),
        spec: record.spec_json.clone(),
        created_at: record.created_at,
    }
}

fn workflow_run_node_protocol(record: WorkflowRunNodeRecord) -> WorkflowRunNodeProtocol {
    WorkflowRunNodeProtocol {
        node_template_id: record.node_template_id,
        work_node_id: record.work_node_id,
        output_contract: record.output_contract_json,
        capability_policy: record.capability_policy_json,
    }
}

fn request_hash_for_run(
    template: &WorkflowTemplateVersionRecord,
    params_hash: &str,
    scheduler_request: &Option<WorkflowSchedulerRequest>,
) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(template.template_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(template.version.to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(template.template_hash.as_bytes());
    hasher.update(b"\0");
    hasher.update(params_hash.as_bytes());
    hasher.update(b"\0");
    match scheduler_request {
        Some(request) => {
            hasher.update(b"with-scheduler\0");
            hasher.update(serde_json::to_vec(request)?);
        }
        None => hasher.update(b"no-scheduler"),
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn hash_json(value: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(value)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn prefixed_id(prefix: &str) -> String {
    format!("{}_{}", prefix, Uuid::new_v4().simple())
}
