use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::dispatch::dispatch_protocol;
use crate::snapshot::SnapshotStore;
use crate::store::{
    BindWorkDispatchInput, DispatchState, EventRecord, EventStore, IdempotencyResolution,
    InsertWorkEdgeInput, InsertWorkNodeInput, InsertWorkRefBindingInput, UpdateWorkNodeStatusInput,
    WorkEdgeRecord, WorkNodeRecord, WorkRefBindingRecord,
};
use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkNodeKind {
    Objective,
    Task,
    Check,
    Review,
}

impl WorkNodeKind {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "objective" => Ok(Self::Objective),
            "task" => Ok(Self::Task),
            "check" => Ok(Self::Check),
            "review" => Ok(Self::Review),
            _ => Err(anyhow!("invalid work node kind: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Objective => "objective",
            Self::Task => "task",
            Self::Check => "check",
            Self::Review => "review",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkEdgeType {
    DecomposesTo,
    DependsOn,
    Validates,
    Supersedes,
}

impl WorkEdgeType {
    pub fn parse(value: &str) -> Result<Self> {
        match value.replace('-', "_").as_str() {
            "decomposes_to" => Ok(Self::DecomposesTo),
            "depends_on" => Ok(Self::DependsOn),
            "validates" => Ok(Self::Validates),
            "supersedes" => Ok(Self::Supersedes),
            _ => Err(anyhow!("invalid work edge type: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::DecomposesTo => "decomposes_to",
            Self::DependsOn => "depends_on",
            Self::Validates => "validates",
            Self::Supersedes => "supersedes",
        }
    }
}

#[derive(Debug)]
pub struct CreateWorkNodeInput {
    pub command_id: String,
    pub kind: WorkNodeKind,
    pub title: String,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub struct AddWorkEdgeInput {
    pub command_id: String,
    pub edge_type: WorkEdgeType,
    pub from_node_id: String,
    pub to_node_id: String,
}

#[derive(Debug)]
pub struct WorkStatusInput {
    pub command_id: String,
    pub work_node_id: String,
    pub reason: Vec<u8>,
}

#[derive(Debug)]
pub struct BindWorkDispatchCommand {
    pub work_node_id: String,
    pub dispatch_id: String,
}

#[derive(Debug)]
pub struct BindWorkRefsCommand {
    pub dispatch_id: String,
    pub fact_event_id: String,
    pub snapshot_ids: Vec<String>,
    pub artifact_refs: Vec<String>,
    pub workspace_ref: Option<String>,
    pub diff_ref: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkNodeProtocol {
    pub work_node_id: String,
    pub command_id: String,
    pub kind: String,
    pub title: String,
    pub body_hash: Option<String>,
    pub body_blob_ref: Option<String>,
    pub status_input: String,
    pub node_version: i64,
    pub graph_version: i64,
    pub accepted_event_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub idempotency_status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct WorkEdgeProtocol {
    pub work_edge_id: String,
    pub command_id: String,
    pub edge_type: String,
    pub from_node_id: String,
    pub to_node_id: String,
    pub graph_version: i64,
    pub created_at: DateTime<Utc>,
    pub idempotency_status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct WorkProjectionProtocol {
    pub work_node_id: String,
    pub state: String,
    pub derived_from: Vec<String>,
    pub missing_requirements: Vec<String>,
    pub allowed_next_actions: Vec<&'static str>,
    pub latest_dispatch_id: Option<String>,
    pub latest_report_status: Option<String>,
    pub node_version: i64,
    pub graph_version: i64,
}

#[derive(Debug, Serialize)]
pub struct WorkInspectProtocol {
    pub node: WorkNodeProtocol,
    pub projection: WorkProjectionProtocol,
    pub dependencies: Vec<String>,
    pub outgoing_edges: Vec<WorkEdgeProtocol>,
    pub dispatches: Vec<crate::dispatch::DispatchProtocol>,
    pub refs: Vec<WorkRefBindingProtocol>,
}

#[derive(Debug, Serialize)]
pub struct WorkListProtocol {
    pub nodes: Vec<WorkNodeProtocol>,
}

#[derive(Debug, Serialize)]
pub struct WorkRefBindingProtocol {
    pub work_node_id: String,
    pub dispatch_id: String,
    pub fact_event_id: String,
    pub snapshot_id: Option<String>,
    pub artifact_ref: Option<String>,
    pub workspace_ref: Option<String>,
    pub diff_ref: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct WorkService<'a, S: SnapshotStore> {
    workspace: &'a Workspace,
    store: &'a EventStore,
    blob_store: &'a S,
}

impl<'a, S: SnapshotStore> WorkService<'a, S> {
    pub fn new(workspace: &'a Workspace, store: &'a EventStore, blob_store: &'a S) -> Self {
        Self {
            workspace,
            store,
            blob_store,
        }
    }

    pub fn create_node(
        &self,
        input: CreateWorkNodeInput,
    ) -> Result<(WorkNodeRecord, &'static str)> {
        self.ensure_schema()?;
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        if input.title.trim().is_empty() {
            return Err(anyhow!("missing work title"));
        }
        let (body_hash, body_blob_ref) = if input.body.is_empty() {
            (None, None)
        } else {
            let body_sha = sha256_hex(&input.body);
            (
                Some(format!("sha256:{body_sha}")),
                Some(self.blob_store.write_blob(&body_sha, &input.body)?),
            )
        };
        let work_node_id = prefixed_id("work");
        let event_id = prefixed_id("evt");
        let created_at = Utc::now();
        let payload = json!({
            "protocol_version": "rive.work.v0",
            "event_id": event_id,
            "command_id": input.command_id,
            "event_type": "work.node.created",
            "workspace_id": workspace_id(self.workspace),
            "work_node_id": work_node_id,
            "kind": input.kind.as_str(),
            "title": input.title,
            "body_hash": body_hash,
            "body_blob_ref": body_blob_ref,
            "created_at": created_at,
        });
        let insert = InsertWorkNodeInput {
            event: EventRecord {
                event_id,
                event_type: "work.node.created".to_string(),
                created_at,
                payload,
            },
            command_id: input.command_id,
            work_node_id,
            kind: input.kind.as_str().to_string(),
            title: input.title,
            body_hash,
            body_blob_ref,
        };
        match self.store.insert_work_node_idempotent(&insert)? {
            IdempotencyResolution::Inserted(node) => Ok((node, "inserted")),
            IdempotencyResolution::Replayed(node) => Ok((node, "replayed")),
            IdempotencyResolution::Conflict(_) => Err(anyhow!("idempotency conflict")),
        }
    }

    pub fn add_edge(&self, input: AddWorkEdgeInput) -> Result<(WorkEdgeRecord, &'static str)> {
        self.ensure_schema()?;
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        if input.from_node_id == input.to_node_id {
            return Err(anyhow!("work graph cycle"));
        }
        self.store
            .get_work_node(&input.from_node_id)?
            .ok_or_else(|| anyhow!("work node not found: {}", input.from_node_id))?;
        self.store
            .get_work_node(&input.to_node_id)?
            .ok_or_else(|| anyhow!("work node not found: {}", input.to_node_id))?;

        let edge_type = input.edge_type.as_str().to_string();
        let edges = self.store.list_work_edges()?;
        if would_create_cycle(&edges, &input.from_node_id, &input.to_node_id) {
            return Err(anyhow!("work graph cycle"));
        }

        let graph_version = self.store.next_work_graph_version()?;
        let work_edge_id = prefixed_id("wedge");
        let event_id = prefixed_id("evt");
        let created_at = Utc::now();
        let payload = json!({
            "protocol_version": "rive.work.v0",
            "event_id": event_id,
            "command_id": input.command_id,
            "event_type": "work.edge.created",
            "workspace_id": workspace_id(self.workspace),
            "work_edge_id": work_edge_id,
            "edge_type": edge_type,
            "from_node_id": input.from_node_id,
            "to_node_id": input.to_node_id,
            "graph_version": graph_version,
            "created_at": created_at,
        });
        let insert = InsertWorkEdgeInput {
            event: EventRecord {
                event_id,
                event_type: "work.edge.created".to_string(),
                created_at,
                payload,
            },
            command_id: input.command_id,
            work_edge_id,
            edge_type,
            from_node_id: input.from_node_id,
            to_node_id: input.to_node_id,
            graph_version,
        };
        match self.store.insert_work_edge_idempotent(&insert)? {
            IdempotencyResolution::Inserted(edge) => Ok((edge, "inserted")),
            IdempotencyResolution::Replayed(edge) => Ok((edge, "replayed")),
            IdempotencyResolution::Conflict(_) => Err(anyhow!("idempotency conflict")),
        }
    }

    pub fn accept_node(&self, input: WorkStatusInput) -> Result<(WorkNodeRecord, &'static str)> {
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        if !self.is_status_command_replay(&input, true)? {
            let projection = self.inspect_projection(&input.work_node_id)?;
            if projection.state != "reviewable" {
                return Err(anyhow!(
                    "work node not reviewable: {} is {}",
                    input.work_node_id,
                    projection.state
                ));
            }
        }
        self.update_node_status(input, "active", true, "work.node.accepted")
    }

    pub fn reopen_node(&self, input: WorkStatusInput) -> Result<(WorkNodeRecord, &'static str)> {
        self.update_node_status(input, "active", false, "work.node.reopened")
    }

    pub fn bind_dispatch(&self, input: BindWorkDispatchCommand) -> Result<()> {
        self.ensure_schema()?;
        self.store
            .get_work_node(&input.work_node_id)?
            .ok_or_else(|| anyhow!("work node not found: {}", input.work_node_id))?;
        self.store
            .get_dispatch(&input.dispatch_id)?
            .ok_or_else(|| anyhow!("dispatch not found: {}", input.dispatch_id))?;
        self.store.bind_work_dispatch(&BindWorkDispatchInput {
            work_node_id: input.work_node_id,
            dispatch_id: input.dispatch_id,
            binding_kind: "execution".to_string(),
            created_at: Utc::now(),
        })
    }

    pub fn bind_refs_for_report(
        &self,
        input: BindWorkRefsCommand,
    ) -> Result<Option<WorkProjectionProtocol>> {
        if !self.store.has_work_schema()? {
            return Ok(None);
        }
        let Some(binding) = self.store.get_work_dispatch_binding(&input.dispatch_id)? else {
            return Ok(None);
        };
        for snapshot_id in &input.snapshot_ids {
            self.store
                .insert_work_ref_binding_idempotent(&InsertWorkRefBindingInput {
                    work_node_id: binding.work_node_id.clone(),
                    dispatch_id: input.dispatch_id.clone(),
                    fact_event_id: input.fact_event_id.clone(),
                    snapshot_id: Some(snapshot_id.clone()),
                    artifact_ref: None,
                    workspace_ref: None,
                    diff_ref: None,
                    created_at: Utc::now(),
                })?;
        }
        for artifact_ref in &input.artifact_refs {
            self.store
                .insert_work_ref_binding_idempotent(&InsertWorkRefBindingInput {
                    work_node_id: binding.work_node_id.clone(),
                    dispatch_id: input.dispatch_id.clone(),
                    fact_event_id: input.fact_event_id.clone(),
                    snapshot_id: None,
                    artifact_ref: Some(artifact_ref.clone()),
                    workspace_ref: None,
                    diff_ref: None,
                    created_at: Utc::now(),
                })?;
        }
        if input.workspace_ref.is_some() || input.diff_ref.is_some() {
            self.store
                .insert_work_ref_binding_idempotent(&InsertWorkRefBindingInput {
                    work_node_id: binding.work_node_id.clone(),
                    dispatch_id: input.dispatch_id,
                    fact_event_id: input.fact_event_id,
                    snapshot_id: None,
                    artifact_ref: None,
                    workspace_ref: input.workspace_ref,
                    diff_ref: input.diff_ref,
                    created_at: Utc::now(),
                })?;
        }
        Ok(Some(self.inspect_projection(&binding.work_node_id)?))
    }

    pub fn list_nodes(&self) -> Result<WorkListProtocol> {
        self.ensure_schema()?;
        let graph_version = self.graph_version()?;
        Ok(WorkListProtocol {
            nodes: self
                .store
                .list_work_nodes()?
                .iter()
                .map(|node| work_node_protocol(node, graph_version, "read"))
                .collect(),
        })
    }

    pub fn show_node(&self, work_node_id: &str) -> Result<WorkNodeProtocol> {
        self.ensure_schema()?;
        let node = self
            .store
            .get_work_node(work_node_id)?
            .ok_or_else(|| anyhow!("work node not found: {work_node_id}"))?;
        Ok(work_node_protocol(&node, self.graph_version()?, "read"))
    }

    pub fn inspect(&self, work_node_id: &str) -> Result<WorkInspectProtocol> {
        self.ensure_schema()?;
        let node = self
            .store
            .get_work_node(work_node_id)?
            .ok_or_else(|| anyhow!("work node not found: {work_node_id}"))?;
        let graph_version = self.graph_version()?;
        let projection = self.inspect_projection(work_node_id)?;
        let edges = self.store.list_work_edges()?;
        let dispatches_by_id = self
            .store
            .list_dispatches()?
            .into_iter()
            .map(|dispatch| (dispatch.dispatch_id.clone(), dispatch))
            .collect::<HashMap<_, _>>();
        let bindings = self.store.list_work_dispatch_bindings()?;
        let dispatches = bindings
            .iter()
            .filter(|binding| binding.work_node_id == work_node_id)
            .filter_map(|binding| dispatches_by_id.get(&binding.dispatch_id))
            .map(|dispatch| dispatch_protocol(dispatch, "read"))
            .collect();
        let refs = self
            .store
            .list_work_ref_bindings()?
            .into_iter()
            .filter(|binding| binding.work_node_id == work_node_id)
            .map(work_ref_protocol)
            .collect();
        Ok(WorkInspectProtocol {
            node: work_node_protocol(&node, graph_version, "read"),
            projection,
            dependencies: dependencies_for(work_node_id, &edges),
            outgoing_edges: edges
                .iter()
                .filter(|edge| edge.from_node_id == work_node_id)
                .map(|edge| work_edge_protocol(edge, "read"))
                .collect(),
            dispatches,
            refs,
        })
    }

    pub fn inspect_projection(&self, work_node_id: &str) -> Result<WorkProjectionProtocol> {
        self.ensure_schema()?;
        let nodes = self.store.list_work_nodes()?;
        let node = nodes
            .iter()
            .find(|node| node.work_node_id == work_node_id)
            .ok_or_else(|| anyhow!("work node not found: {work_node_id}"))?;
        let edges = self.store.list_work_edges()?;
        let graph_version = graph_version_from_edges(&edges);
        let dispatches = self.store.list_dispatches()?;
        let dispatches_by_id = dispatches
            .iter()
            .map(|dispatch| (dispatch.dispatch_id.clone(), dispatch))
            .collect::<HashMap<_, _>>();
        let bindings = self.store.list_work_dispatch_bindings()?;
        let refs = self.store.list_work_ref_bindings()?;

        let mut derived_from = vec![node.work_node_id.clone()];
        let mut missing_requirements = Vec::new();
        let dependencies = dependencies_for(work_node_id, &edges);
        for dependency_id in &dependencies {
            let dependency_projection = self.inspect_projection(dependency_id)?;
            if dependency_projection.state != "done" {
                missing_requirements.push(format!("dependency:{dependency_id}"));
            }
            derived_from.push(dependency_id.clone());
        }
        let children = children_for(work_node_id, &edges);
        for child_id in &children {
            let child_projection = self.inspect_projection(child_id)?;
            if child_projection.state != "done" {
                missing_requirements.push(format!("child:{child_id}"));
            }
            derived_from.push(child_id.clone());
        }

        let node_bindings = bindings
            .iter()
            .filter(|binding| binding.work_node_id == work_node_id)
            .collect::<Vec<_>>();
        let latest_dispatch = node_bindings
            .iter()
            .filter_map(|binding| dispatches_by_id.get(&binding.dispatch_id).copied())
            .max_by_key(|dispatch| dispatch.updated_at);
        let latest_refs = latest_dispatch
            .map(|dispatch| {
                refs.iter()
                    .filter(|binding| binding.dispatch_id == dispatch.dispatch_id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if let Some(dispatch) = latest_dispatch {
            derived_from.push(dispatch.dispatch_id.clone());
            if let Some(event_id) = &dispatch.latest_fact_event_id {
                derived_from.push(event_id.clone());
            }
            for binding in &latest_refs {
                derived_from.push(binding.fact_event_id.clone());
                if let Some(snapshot_id) = &binding.snapshot_id {
                    derived_from.push(snapshot_id.clone());
                }
            }
        }

        let state = if node.status_input == "cancelled" {
            "cancelled"
        } else if node.status_input == "superseded" {
            "superseded"
        } else if node.accepted_event_id.is_some() {
            "done"
        } else if !missing_requirements.is_empty() {
            "blocked"
        } else if let Some(dispatch) = latest_dispatch {
            match dispatch.state {
                DispatchState::Open => "running",
                DispatchState::Reported => {
                    let has_snapshot = latest_refs
                        .iter()
                        .any(|binding| binding.snapshot_id.is_some());
                    if dispatch.latest_report_status.as_deref() == Some("done") && has_snapshot {
                        "reviewable"
                    } else {
                        "needs_attention"
                    }
                }
                DispatchState::Blocked => "blocked",
                DispatchState::Failed | DispatchState::Cancelled => "needs_attention",
            }
        } else if !children.is_empty() {
            "reviewable"
        } else {
            "ready"
        };

        if state == "reviewable"
            && latest_refs
                .iter()
                .all(|binding| binding.snapshot_id.is_none())
        {
            missing_requirements.push("snapshot".to_string());
        }

        Ok(WorkProjectionProtocol {
            work_node_id: work_node_id.to_string(),
            state: state.to_string(),
            derived_from,
            missing_requirements,
            allowed_next_actions: allowed_next_actions(state),
            latest_dispatch_id: latest_dispatch.map(|dispatch| dispatch.dispatch_id.clone()),
            latest_report_status: latest_dispatch
                .and_then(|dispatch| dispatch.latest_report_status.clone()),
            node_version: node.node_version,
            graph_version,
        })
    }

    pub fn projection_for_dispatch(
        &self,
        dispatch_id: &str,
    ) -> Result<Option<WorkProjectionProtocol>> {
        if !self.store.has_work_schema()? {
            return Ok(None);
        }
        let Some(binding) = self.store.get_work_dispatch_binding(dispatch_id)? else {
            return Ok(None);
        };
        Ok(Some(self.inspect_projection(&binding.work_node_id)?))
    }

    pub fn graph_version(&self) -> Result<i64> {
        self.ensure_schema()?;
        Ok(graph_version_from_edges(&self.store.list_work_edges()?))
    }

    fn update_node_status(
        &self,
        input: WorkStatusInput,
        status_input: &str,
        accepted: bool,
        event_type: &str,
    ) -> Result<(WorkNodeRecord, &'static str)> {
        self.ensure_schema()?;
        if input.command_id.trim().is_empty() {
            return Err(anyhow!("missing command id"));
        }
        self.store
            .get_work_node(&input.work_node_id)?
            .ok_or_else(|| anyhow!("work node not found: {}", input.work_node_id))?;
        let event_id = prefixed_id("evt");
        let created_at = Utc::now();
        let accepted_event_id = accepted.then(|| event_id.clone());
        let reason_hash = if input.reason.is_empty() {
            None
        } else {
            Some(format!("sha256:{}", sha256_hex(&input.reason)))
        };
        let payload = json!({
            "protocol_version": "rive.work.v0",
            "event_id": event_id,
            "command_id": input.command_id,
            "event_type": event_type,
            "workspace_id": workspace_id(self.workspace),
            "work_node_id": input.work_node_id,
            "status_input": status_input,
            "accepted_event_id": accepted_event_id,
            "reason_hash": reason_hash,
            "created_at": created_at,
        });
        let update = UpdateWorkNodeStatusInput {
            event: EventRecord {
                event_id,
                event_type: event_type.to_string(),
                created_at,
                payload,
            },
            command_id: input.command_id,
            work_node_id: input.work_node_id,
            status_input: status_input.to_string(),
            accepted_event_id,
        };
        match self.store.update_work_node_status_idempotent(&update)? {
            IdempotencyResolution::Inserted(node) => Ok((node, "inserted")),
            IdempotencyResolution::Replayed(node) => Ok((node, "replayed")),
            IdempotencyResolution::Conflict(_) => Err(anyhow!("idempotency conflict")),
        }
    }

    fn is_status_command_replay(&self, input: &WorkStatusInput, accepted: bool) -> Result<bool> {
        let Some((work_node_id, status_input, accepted_event_id)) =
            self.store.get_work_node_status_command(&input.command_id)?
        else {
            return Ok(false);
        };
        Ok(work_node_id == input.work_node_id
            && status_input == "active"
            && accepted == accepted_event_id.is_some())
    }

    fn ensure_schema(&self) -> Result<()> {
        self.store.init_work_schema()
    }
}

pub fn work_node_protocol(
    node: &WorkNodeRecord,
    graph_version: i64,
    idempotency_status: &'static str,
) -> WorkNodeProtocol {
    WorkNodeProtocol {
        work_node_id: node.work_node_id.clone(),
        command_id: node.command_id.clone(),
        kind: node.kind.clone(),
        title: node.title.clone(),
        body_hash: node.body_hash.clone(),
        body_blob_ref: node.body_blob_ref.clone(),
        status_input: node.status_input.clone(),
        node_version: node.node_version,
        graph_version,
        accepted_event_id: node.accepted_event_id.clone(),
        created_at: node.created_at,
        updated_at: node.updated_at,
        idempotency_status,
    }
}

pub fn work_edge_protocol(
    edge: &WorkEdgeRecord,
    idempotency_status: &'static str,
) -> WorkEdgeProtocol {
    WorkEdgeProtocol {
        work_edge_id: edge.work_edge_id.clone(),
        command_id: edge.command_id.clone(),
        edge_type: edge.edge_type.clone(),
        from_node_id: edge.from_node_id.clone(),
        to_node_id: edge.to_node_id.clone(),
        graph_version: edge.graph_version,
        created_at: edge.created_at,
        idempotency_status,
    }
}

fn work_ref_protocol(binding: WorkRefBindingRecord) -> WorkRefBindingProtocol {
    WorkRefBindingProtocol {
        work_node_id: binding.work_node_id,
        dispatch_id: binding.dispatch_id,
        fact_event_id: binding.fact_event_id,
        snapshot_id: binding.snapshot_id,
        artifact_ref: binding.artifact_ref,
        workspace_ref: binding.workspace_ref,
        diff_ref: binding.diff_ref,
        created_at: binding.created_at,
    }
}

fn dependencies_for(work_node_id: &str, edges: &[WorkEdgeRecord]) -> Vec<String> {
    edges
        .iter()
        .filter(|edge| edge.from_node_id == work_node_id && edge.edge_type == "depends_on")
        .map(|edge| edge.to_node_id.clone())
        .collect()
}

fn children_for(work_node_id: &str, edges: &[WorkEdgeRecord]) -> Vec<String> {
    edges
        .iter()
        .filter(|edge| edge.from_node_id == work_node_id && edge.edge_type == "decomposes_to")
        .map(|edge| edge.to_node_id.clone())
        .collect()
}

fn would_create_cycle(edges: &[WorkEdgeRecord], from: &str, to: &str) -> bool {
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in edges {
        outgoing
            .entry(edge.from_node_id.as_str())
            .or_default()
            .push(edge.to_node_id.as_str());
    }
    outgoing.entry(from).or_default().push(to);
    let mut stack = vec![to];
    let mut seen = HashSet::new();
    while let Some(node) = stack.pop() {
        if node == from {
            return true;
        }
        if !seen.insert(node) {
            continue;
        }
        if let Some(next) = outgoing.get(node) {
            stack.extend(next.iter().copied());
        }
    }
    false
}

fn graph_version_from_edges(edges: &[WorkEdgeRecord]) -> i64 {
    edges
        .iter()
        .map(|edge| edge.graph_version)
        .max()
        .unwrap_or(0)
}

fn allowed_next_actions(state: &str) -> Vec<&'static str> {
    match state {
        "ready" => vec!["delegate", "cancel", "inspect"],
        "running" => vec!["inspect_dispatch", "inspect"],
        "blocked" => vec!["inspect_requirements", "reopen", "inspect"],
        "reviewable" => vec!["accept", "reopen", "delegate_again", "inspect"],
        "done" => vec!["reopen", "inspect"],
        "needs_attention" => vec!["inspect_dispatch", "reopen", "delegate_again", "inspect"],
        "cancelled" | "superseded" => vec!["reopen", "inspect"],
        _ => vec!["inspect"],
    }
}

fn workspace_id(workspace: &Workspace) -> String {
    workspace.root.display().to_string()
}

fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}
