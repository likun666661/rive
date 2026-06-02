# Phase 12: Git Worktree Workspace / Ref Integration MVP

Phase 12 replaces the BranchFS direction with a lower-friction default: **git worktree-backed worker workspaces**.

BranchFS remains an interesting research direction, but requiring macFUSE / kernel extension approval is too much for the default local user experience. Git worktree is already available in normal developer environments, requires no OS-level approval, and is sufficient for the Phase 12 goal: keep parallel worker file mutations out of the parent workspace until Rive explicitly integrates them.

## 1. Problem

Phase 11 can schedule ready Work DAG nodes in parallel, but workers still mutate the shared workspace by default. That creates three practical risks:

- parallel workers can overwrite each other;
- a worker's output is not an explicit integration object;
- work acceptance can happen without a clear patch/ref integration step.

Phase 12 solves this by running each worker in a separate git worktree and recording the worker output as a workspace ref.

```text
scheduler selects ready node
  -> WorktreeWorkspaceBackend creates isolated git worktree
  -> worker edits files in that worktree
  -> worker reports snapshot + workspace_ref
  -> Rive records pending integration
  -> Rive commit/abort/reject explicitly integrates or discards it
  -> work accept can require committed integration
```

## 2. Non-goals

Phase 12 does not implement:

- BranchFS / FUSE / macFUSE integration;
- GitHub PR creation;
- remote push;
- semantic merge conflict resolution;
- daemon scheduler;
- PTY attach;
- full SWE-bench batch runner.

Git worktree is isolation for local file changes, not a security sandbox.

## 3. Backend Interface

The runtime should keep depending on a backend abstraction, not raw filesystem layout:

```text
BranchWorkspaceBackend
  ensure_available(base_workspace)
  create_branch(root_id, work_node_id, dispatch_id, run_id) -> BranchWorkspace
  commit(branch) -> commit_ref / changed_files
  abort(branch)
```

Phase 12 default backend:

```text
GitWorktreeBackend
  create: git worktree add -b <branch_name> .rive/worktrees/<branch_name> HEAD
  commit: generate patch from worker worktree and apply it to parent workspace
  abort: git worktree remove --force and delete temp branch
```

Test backend:

```text
LocalFakeBranchBackend
  emulates isolated workspaces under .rive/worktrees
```

The fake backend is only for tests/fallback. The production default is git worktree.

## 4. Ledger

Phase 12 keeps a branch/ref ledger. Naming is intentionally generic because a git worktree output is still tied to a git branch/ref:

```text
branch_workspaces
  branch_id
  backend               # git-worktree | local-fake
  root_work_node_id
  work_node_id
  dispatch_id
  run_id
  branch_name
  branch_path
  branch_ref
  state                 # created | reported | committed | aborted | rejected | conflict
  created_at
  updated_at

branch_integrations
  integration_id
  branch_id
  work_node_id
  dispatch_id
  fact_event_id?
  branch_ref
  diff_ref?
  state                 # pending | committed | aborted | rejected | conflict
  commit_ref?
  created_at
  updated_at
```

`workspace_ref` / `branch_ref` is opaque:

```text
git-worktree:<workspace-id>:<branch-name>
```

The Work DAG references this through `work_ref_bindings.workspace_ref` and branch integration records. Worktree topology is not Work DAG topology.

## 5. Scheduler Integration

Extend scheduler:

```text
rive scheduler run \
  --root <root> \
  --runner opencode \
  --worker <worker>... \
  --max-parallel <n> \
  --workspace-mode shared|worktree \
  --acceptance-mode manual|auto-reported|auto-committed
```

Worktree mode:

1. scheduler creates one git worktree per worker node;
2. worker `RIVE_WORKSPACE` points to the worktree path;
3. Rive state still writes to parent `.rive` via `RIVE_STATE_WORKSPACE=<parent>`;
4. worker receives `RIVE_WORKSPACE_REF=git-worktree:...`;
5. worker reports that ref with `team report --workspace-ref "$RIVE_WORKSPACE_REF"`;
6. integration remains pending until commit/abort/reject.

`manual`:

- worker report makes the node `reviewable`;
- integration stays `pending`;
- human/orchestrator can inspect and then commit/reject/abort.

`auto-committed`:

- worker report makes the node `reviewable`;
- scheduler applies the worktree patch to the parent workspace;
- scheduler records a committed integration event;
- scheduler accepts the work node.

## 6. Worker Prompt Contract

Worker prompt must be explicit:

```text
You are running in an isolated git worktree workspace.

Rules:
1. Edit files normally in $RIVE_WORKSPACE.
2. Capture evidence with `rive snapshot capture`.
3. Report using `team report --workspace-ref "$RIVE_WORKSPACE_REF"`.
4. Do not run git worktree remove/merge/apply yourself.
5. Do not mutate Work DAG topology.
6. Natural language success is not enough; call `team report`.
```

Rive runtime owns integration decisions.

## 7. Commands

Human-facing integration commands:

```text
rive branch list [--work <node>] [--state pending|committed|rejected|conflict]
rive branch show <branch_id|integration_id>
rive branch commit <integration_id> --command-id <id>
rive branch abort <integration_id> --command-id <id>
rive branch reject <integration_id> --command-id <id> --stdin
```

`commit`:

- verifies integration is pending;
- applies worktree patch to parent workspace;
- records `branch.integration.committed`;
- removes the temporary worktree/branch;
- does not itself mark work done unless caller also accepts the work node or scheduler policy does so.

`abort`:

- removes the temporary worktree/branch;
- records `branch.integration.aborted`;
- work remains not done.

`reject`:

- records rejection reason;
- work remains not done.

## 8. Work Accept Guard

```text
rive work accept <node> --require-committed-branch
```

If the node has worktree integration records, this guard requires at least one committed integration for that node.

Scheduler `auto-committed` uses:

```text
worker report done
  -> reviewable
  -> worktree patch apply succeeds
  -> branch integration committed
  -> work accept
  -> done
```

If patch apply fails:

```text
integration remains pending/conflict
node remains reviewable / needs_attention
scheduler returns worktree_commit_failed or branch_integration_conflict
```

## 9. Error Codes

Stable errors:

```text
worktree_backend_unavailable
worktree_create_failed
worktree_not_found
branch_not_pending
worktree_commit_failed
worktree_abort_failed
branch_integration_conflict
worktree_ref_not_committed
workspace_mode_not_supported
```

## 10. Success Criteria

Phase 12 is complete when:

1. Rive can create isolated worker workspaces through the backend abstraction.
2. Default production backend is git worktree.
3. Workers run in isolated worktree paths and report `git-worktree:` refs.
4. Worktree refs are recorded in Rive ledger and queryable.
5. `rive branch commit/abort/reject` writes explicit integration events.
6. Scheduler `auto-committed` applies worker patches and then accepts work nodes.
7. Parent workspace is unchanged until commit.
8. Invalid worktree refs are rejected before writing fact/report side effects.
9. No stdout/final answer/trace based state transitions are introduced.
