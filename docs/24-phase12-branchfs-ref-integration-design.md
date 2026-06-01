# Phase 12: BranchFS Workspace Branch / Ref Integration MVP

Phase 12 的目标，是把 Phase 11 的 shared workspace worker execution 升级为 **branch workspace execution**：每个 worker 在独立 branch filesystem view 里工作，完成后提交 branch/ref，由 Rive 显式集成。

BranchFS 比 git worktree 更贴这个阶段的问题：它是 FUSE-based copy-on-write branching filesystem，支持 instant branch creation、`@branch` virtual paths、多 agent 并行、commit-to-parent 和 abort。它避免多个 worker 直接踩同一个 workspace，也避免把隔离能力硬绑到 git worktree 的限制上。

参考：

- BranchFS repo: <https://github.com/multikernel/branchfs>
- BranchFS docs/crate: <https://docs.rs/branchfs/latest/branchfs/>
- Branch context paper: <https://arxiv.org/abs/2602.08199>

## 1. Phase 12 想解决什么问题

Phase 11 已经证明：

```text
Work DAG
  -> runtime scheduler finds ready leaf nodes
  -> launches parallel OpenCode workers
  -> workers report snapshots/refs
  -> scheduler accepts under explicit policy
  -> root reaches done
```

但 Phase 11 仍然有一个结构性问题：并发 workers 默认写同一个 workspace。Phase 12 要解决：

1. 每个 worker 必须有隔离 filesystem view。
2. Worker output 必须变成可查询、可 abort、可 commit 的 branch ref。
3. 集成必须是显式 ledger event：commit/abort/conflict/reject 都要入账。
4. Work node `done` 仍然来自 accept event，不来自 branchfs commit 本身。

目标链路：

```text
scheduler selects ready node
  -> BranchWorkspaceBackend creates worker branch
  -> worker runs in branch path
  -> worker reports snapshot + branch_ref
  -> Rive commits/aborts/rejects branch explicitly
  -> integration event updates ref ledger
  -> scheduler/human accepts work node only after integration policy passes
```

## 2. Non-goals

Phase 12 明确不做：

- Full BranchFS reimplementation.
- Kernel/process isolation beyond BranchFS filesystem semantics.
- GitHub PR creation.
- remote push.
- automatic semantic conflict resolution.
- daemon scheduler.
- PTY attach.
- full SWE-bench batch runner.
- AgentFS as primary storage.

If BranchFS is unavailable locally, tests should use a fake/local backend that implements the same Rive interface. That fallback is for CI/tests, not the design center.

## 3. BranchFS Facts We Rely On

From BranchFS README:

- It is a FUSE filesystem for speculative branching over an existing filesystem.
- Branch creation is copy-on-write and isolated.
- Branches can be accessed through `@branch` virtual paths under one mount.
- Commit merges a leaf branch into its parent; abort discards a leaf branch.
- macOS is supported through macFUSE; control can be done via `.branchfs_ctl` writes because ioctl can be inconsistent.

Rive should treat these as backend capabilities, not as business facts. A BranchFS commit only becomes Rive coordination fact after Rive records an integration event.

## 4. Workspace Branch Backend Interface

Introduce an internal abstraction:

```text
BranchWorkspaceBackend
  ensure_mount(base_workspace) -> mount_id / mount_path
  create_branch(root_id, work_node_id, dispatch_id, run_id) -> BranchWorkspace
  branch_path(branch) -> path
  diff(branch) -> diff_ref / summary
  commit(branch) -> branch_commit_ref / changed_files
  abort(branch) -> aborted
  status(branch) -> clean/dirty/committed/aborted/conflict
```

Phase 12 should ship:

```text
BranchFsBackend      # real branchfs adapter when branchfs is installed
LocalFakeBranchBackend # tests only, emulates branch paths under .rive/branches
```

The rest of Rive must depend on the trait, not directly on `.branchfs_ctl` or `@branch` layout.

## 5. Branch Ref Ledger

Add branch integration tables/projections:

```text
branch_workspaces
  branch_id
  backend               # branchfs | local-fake
  root_work_node_id
  work_node_id
  dispatch_id
  run_id
  branch_name
  branch_path
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

`branch_ref` should be opaque and backend-specific:

```text
branchfs:<mount-id>:<branch-name>
```

The Work DAG only references it through `work_ref_bindings.workspace_ref` or a dedicated branch integration record. Branch topology is not Work DAG topology.

## 6. Scheduler Integration

Extend scheduler:

```text
rive scheduler run \
  --root <root> \
  --runner opencode \
  --worker <worker>... \
  --max-parallel <n> \
  --workspace-mode shared|branchfs \
  --acceptance-mode manual|auto-reported|auto-committed
```

Phase 12 adds:

```text
--workspace-mode branchfs
--acceptance-mode auto-committed
```

In branchfs mode:

1. scheduler creates a branch for each worker node.
2. worker `RIVE_WORKSPACE` points at the branch path, e.g. `<mount>/@rive-<run>-<node>`.
3. Rive state still writes to parent `.rive` via `RIVE_STATE_WORKSPACE=<parent>`.
4. worker reports `workspace_ref=branchfs:<mount-id>:<branch-name>`.
5. scheduler may commit branch only under `auto-committed`.

`manual`:

- run worker in branch.
- worker report makes node `reviewable`.
- integration remains pending.
- human/orchestrator can inspect and commit/reject later.

`auto-committed`:

- worker report makes node `reviewable`.
- scheduler commits the BranchFS branch into parent.
- scheduler records branch integration committed event.
- scheduler accepts node.

## 7. Worker Prompt Contract

BranchFS worker prompt must be explicit:

```text
You are running in an isolated BranchFS branch workspace.

Rules:
1. Edit files normally in $RIVE_WORKSPACE.
2. Capture evidence with `rive snapshot capture`.
3. Report using `team report --workspace-ref "$RIVE_BRANCH_REF"`.
4. Do not call branchfs commit/abort directly.
5. Do not mutate Work DAG topology.
6. Natural language success is not enough; call `team report`.
```

The worker should not commit/abort the branch itself. Rive runtime owns integration decisions.

## 8. Branch Commands

Human-facing:

```text
rive branch list [--work <node>] [--state pending|committed|rejected|conflict]
rive branch show <branch_id|integration_id>
rive branch commit <integration_id> --command-id <id>
rive branch abort <integration_id> --command-id <id>
rive branch reject <integration_id> --command-id <id> --stdin
```

`commit`:

- verifies integration is pending/reported.
- asks backend to commit branch.
- records `branch.integration.committed`.
- does not itself mark work done unless caller also does `work accept` or scheduler policy does so.

`abort`:

- asks backend to abort branch.
- records `branch.integration.aborted`.
- work remains not done.

`reject`:

- records human/runtime rejection reason.
- may abort branch depending policy.

## 9. Work Accept Guard

Add optional guard:

```text
rive work accept <node> --require-committed-branch
```

If the node has a branch integration, this guard requires a committed integration before accept.

Scheduler `auto-committed` uses this policy internally:

```text
worker report done
  -> reviewable
  -> branch commit succeeds
  -> branch integration committed
  -> work accept
  -> done
```

If BranchFS commit fails or conflicts:

```text
branch integration -> conflict
node remains reviewable/needs_attention
scheduler returns branch_integration_conflict
```

## 10. Idempotency

All branch write commands must be idempotent:

- same scheduler command replay does not create duplicate branches or rerun workers.
- same branch commit command replay returns same committed integration.
- abort/reject replay returns same projection.
- same command id with different branch/integration returns `idempotency_conflict`.

Branch identity should include:

```text
root_work_node_id
work_node_id
dispatch_id
run_id
backend
branch_name
```

## 11. Error Codes

Stable errors:

```text
branch_backend_unavailable
branch_mount_failed
branch_create_failed
branch_not_found
branch_not_pending
branch_commit_failed
branch_abort_failed
branch_integration_conflict
branch_ref_not_committed
workspace_mode_not_supported
```

## 12. Success Criteria

Phase 12 is complete when:

1. Rive can create branch workspaces through a backend abstraction.
2. Real BranchFS backend is used when `branchfs` is available.
3. Workers can run in isolated branch paths and report `branch_ref`.
4. Branch refs are recorded in Rive ledger and queryable.
5. `rive branch commit/abort/reject` writes explicit integration events.
6. Scheduler `auto-committed` can commit branch outputs and then accept work nodes.
7. Parallel workers do not write the parent workspace until branch commit.
8. BranchFS absence has a clear error or test-only fake backend path.
9. No stdout/final answer/trace based state transitions are introduced.

