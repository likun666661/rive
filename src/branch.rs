use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::store::{
    BranchIntegrationRecord, BranchWorkspaceRecord, EventStore, InsertBranchIntegrationInput,
    InsertBranchWorkspaceInput, RecordBranchCommandInput, UpdateBranchIntegrationInput,
    UpdateBranchWorkspaceInput,
};
use crate::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct BranchWorkspace {
    pub branch_id: String,
    pub backend: String,
    pub root_work_node_id: String,
    pub work_node_id: String,
    pub dispatch_id: String,
    pub run_id: String,
    pub branch_name: String,
    pub branch_path: PathBuf,
    pub branch_ref: String,
}

#[derive(Debug, Clone)]
pub struct BranchCommitResult {
    pub commit_ref: String,
    pub changed_files: Vec<String>,
}

pub trait BranchWorkspaceBackend {
    fn backend_name(&self) -> &'static str;
    fn ensure_available(&self, workspace: &Workspace) -> Result<()>;
    fn create_branch(
        &self,
        workspace: &Workspace,
        root_work_node_id: &str,
        work_node_id: &str,
        dispatch_id: &str,
        run_id: &str,
    ) -> Result<BranchWorkspace>;
    fn commit(
        &self,
        workspace: &Workspace,
        branch: &BranchWorkspaceRecord,
    ) -> Result<BranchCommitResult>;
    fn abort(&self, workspace: &Workspace, branch: &BranchWorkspaceRecord) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct LocalFakeBranchBackend;

impl LocalFakeBranchBackend {
    fn branches_root(workspace: &Workspace) -> PathBuf {
        workspace.rive_dir().join("worktrees")
    }
}

impl BranchWorkspaceBackend for LocalFakeBranchBackend {
    fn backend_name(&self) -> &'static str {
        "local-fake"
    }

    fn ensure_available(&self, _workspace: &Workspace) -> Result<()> {
        Ok(())
    }

    fn create_branch(
        &self,
        workspace: &Workspace,
        root_work_node_id: &str,
        work_node_id: &str,
        dispatch_id: &str,
        run_id: &str,
    ) -> Result<BranchWorkspace> {
        let branch_id = prefixed_id("branch");
        let branch_name = branch_name(run_id, work_node_id);
        let branch_path = Self::branches_root(workspace).join(&branch_name);
        if branch_path.exists() {
            fs::remove_dir_all(&branch_path)
                .with_context(|| format!("remove stale branch {}", branch_path.display()))?;
        }
        fs::create_dir_all(&branch_path)?;
        copy_tree(&workspace.root, &branch_path, CopyMode::ParentToBranch)?;
        Ok(BranchWorkspace {
            branch_id,
            backend: self.backend_name().to_string(),
            root_work_node_id: root_work_node_id.to_string(),
            work_node_id: work_node_id.to_string(),
            dispatch_id: dispatch_id.to_string(),
            run_id: run_id.to_string(),
            branch_name: branch_name.clone(),
            branch_path,
            branch_ref: format!("git-worktree:{}:{}", workspace_id(workspace), branch_name),
        })
    }

    fn commit(
        &self,
        workspace: &Workspace,
        branch: &BranchWorkspaceRecord,
    ) -> Result<BranchCommitResult> {
        let branch_path = PathBuf::from(&branch.branch_path);
        if !branch_path.is_dir() {
            return Err(anyhow!("branch not found: {}", branch.branch_id));
        }
        let changed_files = changed_files(&workspace.root, &branch_path)?;
        apply_deletions(&workspace.root, &branch_path)?;
        copy_tree(&branch_path, &workspace.root, CopyMode::BranchToParent)?;
        Ok(BranchCommitResult {
            commit_ref: format!("local-fake-commit:{}", branch.branch_name),
            changed_files,
        })
    }

    fn abort(&self, _workspace: &Workspace, branch: &BranchWorkspaceRecord) -> Result<()> {
        let branch_path = PathBuf::from(&branch.branch_path);
        if branch_path.exists() {
            fs::remove_dir_all(&branch_path)
                .with_context(|| format!("abort branch {}", branch_path.display()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct GitWorktreeBackend {
    pub binary: PathBuf,
}

impl Default for GitWorktreeBackend {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("git"),
        }
    }
}

impl GitWorktreeBackend {
    fn worktrees_root(workspace: &Workspace) -> PathBuf {
        workspace.rive_dir().join("worktrees")
    }

    fn run_git<I, S>(&self, cwd: &Path, args: I) -> Result<std::process::Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        Command::new(&self.binary)
            .current_dir(cwd)
            .args(args)
            .output()
            .map_err(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    anyhow!("worktree backend unavailable: git not found")
                } else {
                    anyhow!("git launch failed: {err}")
                }
            })
    }

    fn run_git_checked<I, S>(&self, cwd: &Path, args: I, error_prefix: &str) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = self.run_git(cwd, args)?;
        if !output.status.success() {
            return Err(anyhow!(
                "{error_prefix}: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn git_root(&self, workspace: &Workspace) -> Result<PathBuf> {
        let output = self.run_git(
            &workspace.root,
            ["rev-parse", "--show-toplevel"].iter().copied(),
        )?;
        if !output.status.success() {
            return Err(anyhow!(
                "worktree backend unavailable: workspace is not a git repository"
            ));
        }
        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let root = PathBuf::from(root)
            .canonicalize()
            .with_context(|| "canonicalize git root")?;
        if root != workspace.root {
            return Err(anyhow!(
                "worktree backend unavailable: Rive workspace must be git root for worktree mode"
            ));
        }
        Ok(root)
    }

    fn cleanup_worktree(
        &self,
        workspace: &Workspace,
        branch: &BranchWorkspaceRecord,
    ) -> Result<()> {
        let branch_path = PathBuf::from(&branch.branch_path);
        let _ = self.run_git_checked(
            &workspace.root,
            [
                "worktree",
                "remove",
                "--force",
                branch_path.to_string_lossy().as_ref(),
            ],
            "worktree remove failed",
        );
        let _ = self.run_git_checked(
            &workspace.root,
            ["branch", "-D", &branch.branch_name],
            "worktree branch delete failed",
        );
        if branch_path.exists() {
            fs::remove_dir_all(&branch_path)
                .with_context(|| format!("remove worktree {}", branch_path.display()))?;
        }
        Ok(())
    }
}

impl BranchWorkspaceBackend for GitWorktreeBackend {
    fn backend_name(&self) -> &'static str {
        "git-worktree"
    }

    fn ensure_available(&self, workspace: &Workspace) -> Result<()> {
        self.git_root(workspace)?;
        Ok(())
    }

    fn create_branch(
        &self,
        workspace: &Workspace,
        root_work_node_id: &str,
        work_node_id: &str,
        dispatch_id: &str,
        run_id: &str,
    ) -> Result<BranchWorkspace> {
        self.ensure_available(workspace)?;
        let branch_id = prefixed_id("branch");
        let branch_name = branch_name(run_id, work_node_id);
        let branch_path = Self::worktrees_root(workspace).join(&branch_name);
        if branch_path.exists() {
            fs::remove_dir_all(&branch_path)
                .with_context(|| format!("remove stale worktree {}", branch_path.display()))?;
        }
        fs::create_dir_all(Self::worktrees_root(workspace))?;
        self.run_git_checked(
            &workspace.root,
            [
                "worktree",
                "add",
                "-b",
                &branch_name,
                branch_path.to_string_lossy().as_ref(),
                "HEAD",
            ],
            "worktree create failed",
        )?;
        Ok(BranchWorkspace {
            branch_id,
            backend: self.backend_name().to_string(),
            root_work_node_id: root_work_node_id.to_string(),
            work_node_id: work_node_id.to_string(),
            dispatch_id: dispatch_id.to_string(),
            run_id: run_id.to_string(),
            branch_name: branch_name.clone(),
            branch_path,
            branch_ref: format!("git-worktree:{}:{}", workspace_id(workspace), branch_name),
        })
    }

    fn commit(
        &self,
        workspace: &Workspace,
        branch: &BranchWorkspaceRecord,
    ) -> Result<BranchCommitResult> {
        self.ensure_available(workspace)?;
        let branch_path = PathBuf::from(&branch.branch_path);
        if !branch_path.is_dir() {
            return Err(anyhow!("worktree not found: {}", branch.branch_id));
        }
        self.run_git_checked(&branch_path, ["add", "-N", "."], "worktree diff failed")?;
        let changed_output = self.run_git(&branch_path, ["diff", "--name-only", "HEAD"])?;
        if !changed_output.status.success() {
            return Err(anyhow!(
                "worktree diff failed: {}",
                String::from_utf8_lossy(&changed_output.stderr)
            ));
        }
        let mut changed_files = String::from_utf8_lossy(&changed_output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        changed_files.sort();
        changed_files.dedup();

        let patch_output = self.run_git(&branch_path, ["diff", "--binary", "HEAD"])?;
        if !patch_output.status.success() {
            return Err(anyhow!(
                "worktree diff failed: {}",
                String::from_utf8_lossy(&patch_output.stderr)
            ));
        }
        if !patch_output.stdout.is_empty() {
            let mut apply = Command::new(&self.binary)
                .current_dir(&workspace.root)
                .arg("apply")
                .arg("--whitespace=nowarn")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|err| anyhow!("worktree commit failed: {err}"))?;
            apply
                .stdin
                .as_mut()
                .ok_or_else(|| anyhow!("worktree commit failed: missing stdin"))?
                .write_all(&patch_output.stdout)?;
            let output = apply.wait_with_output()?;
            if !output.status.success() {
                return Err(anyhow!(
                    "worktree commit failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
        self.cleanup_worktree(workspace, branch)?;
        let commit_hash = sha256_hex(&patch_output.stdout);
        Ok(BranchCommitResult {
            commit_ref: format!("git-worktree-apply:{}:{}", branch.branch_name, commit_hash),
            changed_files,
        })
    }

    fn abort(&self, workspace: &Workspace, branch: &BranchWorkspaceRecord) -> Result<()> {
        self.ensure_available(workspace)?;
        self.cleanup_worktree(workspace, branch)
            .map_err(|err| anyhow!("worktree abort failed: {err}"))
    }
}

pub fn backend_from_env() -> Box<dyn BranchWorkspaceBackend + Send + Sync> {
    match std::env::var("RIVE_WORKSPACE_BACKEND")
        .unwrap_or_else(|_| "git-worktree".to_string())
        .as_str()
    {
        "local-fake" => Box::new(LocalFakeBranchBackend),
        _ => Box::new(GitWorktreeBackend::default()),
    }
}

pub struct BranchService<'a> {
    workspace: &'a Workspace,
    store: &'a EventStore,
}

impl<'a> BranchService<'a> {
    pub fn new(workspace: &'a Workspace, store: &'a EventStore) -> Self {
        Self { workspace, store }
    }

    pub fn create_workspace(
        &self,
        backend: &dyn BranchWorkspaceBackend,
        root_work_node_id: &str,
        work_node_id: &str,
        dispatch_id: &str,
        run_id: &str,
    ) -> Result<BranchWorkspaceRecord> {
        self.store.init_work_schema()?;
        let branch = backend.create_branch(
            self.workspace,
            root_work_node_id,
            work_node_id,
            dispatch_id,
            run_id,
        )?;
        self.store
            .insert_branch_workspace(&InsertBranchWorkspaceInput {
                branch_id: branch.branch_id,
                backend: branch.backend,
                root_work_node_id: branch.root_work_node_id,
                work_node_id: branch.work_node_id,
                dispatch_id: branch.dispatch_id,
                run_id: branch.run_id,
                branch_name: branch.branch_name,
                branch_path: branch.branch_path.display().to_string(),
                branch_ref: branch.branch_ref,
                state: "created".to_string(),
                created_at: Utc::now(),
            })
    }

    pub fn ensure_pending_for_report(
        &self,
        dispatch_id: &str,
        fact_event_id: &str,
        workspace_ref: &str,
    ) -> Result<Option<BranchIntegrationRecord>> {
        self.validate_workspace_ref_for_report(dispatch_id, workspace_ref)?;
        if let Some(branch) = self.store.get_branch_workspace_by_ref(workspace_ref)? {
            let existing = self
                .store
                .get_branch_integration_by_branch_id(&branch.branch_id)?;
            if let Some(existing) = existing {
                if existing.dispatch_id != dispatch_id {
                    return Err(anyhow!(
                        "branch integration conflict: {} belongs to dispatch {}",
                        workspace_ref,
                        existing.dispatch_id
                    ));
                }
                return Ok(Some(existing));
            }
            self.store
                .update_branch_workspace(&UpdateBranchWorkspaceInput {
                    branch_id: branch.branch_id.clone(),
                    state: "reported".to_string(),
                    updated_at: Utc::now(),
                })?;
            let integration =
                self.store
                    .insert_branch_integration(&InsertBranchIntegrationInput {
                        integration_id: prefixed_id("brint"),
                        branch_id: branch.branch_id,
                        work_node_id: branch.work_node_id,
                        dispatch_id: dispatch_id.to_string(),
                        fact_event_id: Some(fact_event_id.to_string()),
                        branch_ref: workspace_ref.to_string(),
                        diff_ref: None,
                        state: "pending".to_string(),
                        commit_ref: None,
                        rejection_reason_hash: None,
                        created_at: Utc::now(),
                    })?;
            return Ok(Some(integration));
        }
        if is_managed_workspace_ref(workspace_ref) {
            return Err(anyhow!("worktree not found: {workspace_ref}"));
        }
        Ok(None)
    }

    pub fn validate_workspace_ref_for_report(
        &self,
        dispatch_id: &str,
        workspace_ref: &str,
    ) -> Result<()> {
        if !is_managed_workspace_ref(workspace_ref) {
            return Ok(());
        }
        self.store.init_work_schema()?;
        let branch = self
            .store
            .get_branch_workspace_by_ref(workspace_ref)?
            .ok_or_else(|| anyhow!("worktree not found: {workspace_ref}"))?;
        if branch.dispatch_id != dispatch_id {
            return Err(anyhow!(
                "branch integration conflict: {} belongs to dispatch {}",
                workspace_ref,
                branch.dispatch_id
            ));
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<BranchIntegrationRecord>> {
        self.store.init_work_schema()?;
        self.store.list_branch_integrations()
    }

    pub fn show(
        &self,
        id: &str,
    ) -> Result<(BranchWorkspaceRecord, Option<BranchIntegrationRecord>)> {
        self.store.init_work_schema()?;
        if let Some(integration) = self.store.get_branch_integration(id)? {
            let branch = self
                .store
                .get_branch_workspace(&integration.branch_id)?
                .ok_or_else(|| anyhow!("branch not found: {}", integration.branch_id))?;
            return Ok((branch, Some(integration)));
        }
        let branch = self
            .store
            .get_branch_workspace(id)?
            .ok_or_else(|| anyhow!("branch not found: {id}"))?;
        let integration = self
            .store
            .get_branch_integration_by_branch_id(&branch.branch_id)?;
        Ok((branch, integration))
    }

    pub fn commit(
        &self,
        backend: &dyn BranchWorkspaceBackend,
        integration_id: &str,
        command_id: &str,
    ) -> Result<(BranchIntegrationRecord, &'static str)> {
        self.transition_with_backend(
            backend,
            integration_id,
            command_id,
            "commit",
            |backend, workspace, branch| backend.commit(workspace, branch).map(Some),
        )
    }

    pub fn abort(
        &self,
        backend: &dyn BranchWorkspaceBackend,
        integration_id: &str,
        command_id: &str,
    ) -> Result<(BranchIntegrationRecord, &'static str)> {
        self.transition_with_backend(
            backend,
            integration_id,
            command_id,
            "abort",
            |backend, workspace, branch| {
                backend.abort(workspace, branch)?;
                Ok(None)
            },
        )
    }

    pub fn reject(
        &self,
        integration_id: &str,
        command_id: &str,
        reason: &[u8],
    ) -> Result<(BranchIntegrationRecord, &'static str)> {
        let integration = self
            .store
            .get_branch_integration(integration_id)?
            .ok_or_else(|| anyhow!("branch not found: {integration_id}"))?;
        if let Some(existing) = self.store.get_branch_command(command_id)? {
            if existing.0 == integration_id && existing.1 == "reject" {
                return Ok((integration, "replayed"));
            }
            return Err(anyhow!("idempotency conflict"));
        }
        if integration.state != "pending" {
            return Err(anyhow!("branch not pending: {integration_id}"));
        }
        let reason_hash = if reason.is_empty() {
            None
        } else {
            Some(format!("sha256:{}", sha256_hex(reason)))
        };
        let updated = self.store.update_branch_integration(
            &UpdateBranchIntegrationInput {
                integration_id: integration_id.to_string(),
                state: "rejected".to_string(),
                commit_ref: None,
                rejection_reason_hash: reason_hash,
                updated_at: Utc::now(),
            },
            "branch.integration.rejected",
            command_id,
        )?;
        self.store
            .record_branch_command(&RecordBranchCommandInput {
                command_id: command_id.to_string(),
                integration_id: integration_id.to_string(),
                action: "reject".to_string(),
                request_hash: format!("sha256:{}", sha256_hex(reason)),
                created_at: Utc::now(),
            })?;
        Ok((updated, "inserted"))
    }

    fn transition_with_backend<F>(
        &self,
        backend: &dyn BranchWorkspaceBackend,
        integration_id: &str,
        command_id: &str,
        action: &str,
        apply: F,
    ) -> Result<(BranchIntegrationRecord, &'static str)>
    where
        F: FnOnce(
            &dyn BranchWorkspaceBackend,
            &Workspace,
            &BranchWorkspaceRecord,
        ) -> Result<Option<BranchCommitResult>>,
    {
        let integration = self
            .store
            .get_branch_integration(integration_id)?
            .ok_or_else(|| anyhow!("branch not found: {integration_id}"))?;
        if let Some(existing) = self.store.get_branch_command(command_id)? {
            if existing.0 == integration_id && existing.1 == action {
                return Ok((integration, "replayed"));
            }
            return Err(anyhow!("idempotency conflict"));
        }
        if integration.state != "pending" {
            return Err(anyhow!("branch not pending: {integration_id}"));
        }
        let branch = self
            .store
            .get_branch_workspace(&integration.branch_id)?
            .ok_or_else(|| anyhow!("branch not found: {}", integration.branch_id))?;
        let result = apply(backend, self.workspace, &branch)?;
        let (state, event_type, commit_ref) = match action {
            "commit" => (
                "committed",
                "branch.integration.committed",
                result.map(|result| result.commit_ref),
            ),
            "abort" => ("aborted", "branch.integration.aborted", None),
            _ => unreachable!(),
        };
        self.store
            .update_branch_workspace(&UpdateBranchWorkspaceInput {
                branch_id: branch.branch_id,
                state: state.to_string(),
                updated_at: Utc::now(),
            })?;
        let updated = self.store.update_branch_integration(
            &UpdateBranchIntegrationInput {
                integration_id: integration_id.to_string(),
                state: state.to_string(),
                commit_ref,
                rejection_reason_hash: None,
                updated_at: Utc::now(),
            },
            event_type,
            command_id,
        )?;
        self.store
            .record_branch_command(&RecordBranchCommandInput {
                command_id: command_id.to_string(),
                integration_id: integration_id.to_string(),
                action: action.to_string(),
                request_hash: sha256_hex(format!("{integration_id}:{action}").as_bytes()),
                created_at: Utc::now(),
            })?;
        Ok((updated, "inserted"))
    }
}

#[derive(Debug, Serialize)]
pub struct BranchIntegrationProtocol {
    pub integration_id: String,
    pub branch_id: String,
    pub work_node_id: String,
    pub dispatch_id: String,
    pub branch_ref: String,
    pub state: String,
    pub commit_ref: Option<String>,
}

pub fn branch_integration_protocol(
    integration: &BranchIntegrationRecord,
) -> BranchIntegrationProtocol {
    BranchIntegrationProtocol {
        integration_id: integration.integration_id.clone(),
        branch_id: integration.branch_id.clone(),
        work_node_id: integration.work_node_id.clone(),
        dispatch_id: integration.dispatch_id.clone(),
        branch_ref: integration.branch_ref.clone(),
        state: integration.state.clone(),
        commit_ref: integration.commit_ref.clone(),
    }
}

enum CopyMode {
    ParentToBranch,
    BranchToParent,
}

fn copy_tree(src: &Path, dst: &Path, mode: CopyMode) -> Result<()> {
    for entry in WalkDir::new(src).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if path == src {
            continue;
        }
        let rel = path_relative_to(path, src)?;
        if should_skip(&rel, &mode) {
            continue;
        }
        let target = dst.join(&rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(path, &target)
                .with_context(|| format!("copy {} -> {}", path.display(), target.display()))?;
        }
    }
    Ok(())
}

fn should_skip(rel: &str, mode: &CopyMode) -> bool {
    let first = rel.split('/').next().unwrap_or("");
    match mode {
        CopyMode::ParentToBranch => matches!(first, ".rive" | ".git" | "target"),
        CopyMode::BranchToParent => matches!(first, ".rive" | ".git" | "target"),
    }
}

fn changed_files(parent: &Path, branch: &Path) -> Result<Vec<String>> {
    let parent = file_digest_map(parent)?;
    let branch = file_digest_map(branch)?;
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

fn apply_deletions(parent: &Path, branch: &Path) -> Result<()> {
    let parent_files = file_set(parent)?;
    let branch_files = file_set(branch)?;
    for rel in parent_files.difference(&branch_files) {
        let path = parent.join(rel);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("remove deleted branch file {}", path.display()))?;
            prune_empty_parents(parent, path.parent())?;
        }
    }
    Ok(())
}

fn prune_empty_parents(root: &Path, mut current: Option<&Path>) -> Result<()> {
    while let Some(path) = current {
        if path == root {
            break;
        }
        match fs::remove_dir(path) {
            Ok(()) => current = path.parent(),
            Err(err) if err.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => break,
            Err(err) => return Err(err).with_context(|| format!("remove dir {}", path.display())),
        }
    }
    Ok(())
}

fn file_digest_map(root: &Path) -> Result<std::collections::BTreeMap<String, String>> {
    let mut files = std::collections::BTreeMap::new();
    if !root.exists() {
        return Ok(files);
    }
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_file() {
            let rel = path_relative_to(entry.path(), root)?;
            if !should_skip(&rel, &CopyMode::BranchToParent) {
                files.insert(rel, sha256_hex(&fs::read(entry.path())?));
            }
        }
    }
    Ok(files)
}

fn file_set(root: &Path) -> Result<BTreeSet<String>> {
    Ok(file_digest_map(root)?.into_keys().collect())
}

fn path_relative_to(path: &Path, root: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(root)
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))?
        .to_string_lossy()
        .trim_start_matches('/')
        .to_string())
}

fn branch_name(run_id: &str, work_node_id: &str) -> String {
    format!("rive-{}-{}", short_id(run_id), short_id(work_node_id))
}

fn short_id(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(12)
        .collect()
}

fn workspace_id(workspace: &Workspace) -> String {
    format!("workspace:{}", workspace.root.display())
}

fn is_managed_workspace_ref(workspace_ref: &str) -> bool {
    workspace_ref.starts_with("git-worktree:")
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn prefixed_id(prefix: &str) -> String {
    format!("{}_{}", prefix, Uuid::new_v4().simple())
}
