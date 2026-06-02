# Phase 12 Git Worktree Workspace / Ref Integration 测试计划

## 1. 测试目标

Phase 12 验证 worker output 可以从 shared workspace mutation 升级为 git worktree ref：

```text
git worktree workspace
  -> worker changes files in isolated worktree
  -> team report with workspace_ref
  -> branch integration pending
  -> rive branch commit / abort / reject
  -> integration committed / aborted / rejected
  -> work accept / scheduler auto-committed
```

验收重点：

- workers 不直接写 parent workspace；
- worktree/ref 是事实系统可查询对象；
- commit/abort/reject/conflict 是显式 ledger event；
- work done 仍然由 accept event 推进，不由 patch apply 或 stdout 推进。

## 2. Backend Availability

### Git unavailable or workspace is not git root

Run:

```text
rive scheduler run --workspace-mode worktree ...
```

when `git` is unavailable, workspace is not a git repository, or the Rive workspace is not the git root.

Expected:

- stable error `worktree_backend_unavailable`;
- no worker process starts;
- no Work DAG state changes.

### Test fake backend

Tests may use:

```text
RIVE_WORKSPACE_BACKEND=local-fake
```

Expected:

- fake backend implements the same integration contract;
- production default remains git worktree.

## 3. Worktree Workspace Tests

### Create worktree workspace

Expected:

- `branch_workspaces` row exists with backend `git-worktree`;
- worker path is distinct from parent workspace;
- `RIVE_WORKSPACE` for worker is the worktree path;
- `RIVE_STATE_WORKSPACE` points to parent workspace;
- parent workspace is unchanged before commit.

### Worker report binds workspace ref

Worker calls:

```text
team report \
  --dispatch <dispatch> \
  --status done \
  --snapshot <snapshot> \
  --workspace-ref git-worktree:<workspace>:<branch> \
  --command-id <id>
```

Expected:

- dispatch `reported/done`;
- work node `reviewable`;
- `work_ref_bindings.workspace_ref` stores the ref;
- `branch_integrations` has pending integration;
- node is not `done` yet.

### Invalid workspace ref preflight

Run `team report --workspace-ref git-worktree:not-real`.

Expected:

- stable error `worktree_not_found`;
- no fact row is written;
- dispatch stays `open`;
- no work ref binding is written.

### Worktree commit

Run:

```text
rive branch commit <integration_id> --command-id commit-1
```

Expected:

- parent workspace receives worker changes;
- modified files, new files, and deleted files are applied;
- integration state becomes `committed`;
- `branch.integration.committed` event is written;
- temporary worktree/branch is removed;
- replay returns same projection without a second patch apply.

### Worktree abort

Run:

```text
rive branch abort <integration_id> --command-id abort-1
```

Expected:

- temporary worktree/branch is removed;
- parent workspace stays unchanged;
- integration state becomes `aborted`;
- work node is not accepted.

### Worktree reject

Run:

```text
rive branch reject <integration_id> --command-id reject-1 --stdin
```

Expected:

- rejection reason hash is stored;
- integration state becomes `rejected`;
- work node remains not done.

## 4. Work Accept Guard

Before worktree commit:

```text
rive work accept <node> --require-committed-branch
```

Expected:

- returns `worktree_ref_not_committed`.

After worktree commit:

- command succeeds;
- writes `work.node.accepted`;
- node becomes `done`.

## 5. Scheduler Worktree Tests

### Auto-committed happy path

Graph:

```text
root
  decomposes_to A
  decomposes_to B
```

Run:

```text
rive scheduler run \
  --root <root> \
  --runner opencode \
  --worker worker-a \
  --worker worker-b \
  --max-parallel 2 \
  --workspace-mode worktree \
  --acceptance-mode auto-committed
```

Expected:

- A/B run in separate worktree paths;
- parent workspace remains unchanged until commit;
- scheduler applies A/B patches;
- A/B and root become done through explicit accept events;
- branch integration rows are committed.

### Parallel isolation

Two workers write the same path differently.

Expected:

- writes are isolated in separate worktrees;
- first commit applies according to patch policy;
- conflicting later apply returns a stable commit/conflict error;
- no silent overwrite.

### Replay no duplicate worktrees

Replay same scheduler command.

Expected:

- no worker relaunch;
- no duplicate worktree rows;
- no duplicate commits/integrations.

### Manual mode

Run with:

```text
--workspace-mode worktree
--acceptance-mode manual
```

Expected:

- workers report worktree refs;
- nodes become reviewable;
- integrations remain pending;
- scheduler state `waiting_review`;
- no patch apply or accept event.

## 6. Real Git Worktree Smoke

On a temp git repo:

1. `git init`, create base commit.
2. `rive init`.
3. Run one-node OpenCode/fake worker through `--workspace-mode worktree`.
4. Worker creates `phase12-worktree.txt`.
5. Worker reports workspace ref.
6. `rive branch commit` applies worker patch to parent.
7. `rive work accept --require-committed-branch` marks node done.

Expected:

- parent file appears only after commit;
- integration state is `committed`;
- work node can be accepted after committed worktree ref;
- trace/debug remains debug-only.

## 7. Regression

Run:

```text
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Required:

- Phase 11 shared workspace scheduler still passes;
- Phase 10 sandbox tests still pass;
- Phase 8/9 Work DAG tests still pass;
- existing dispatch/fact/snapshot tests still pass.

## 8. Non-goal Checks

Phase 12 must not introduce:

- BranchFS / FUSE / macFUSE dependency;
- GitHub PR creation;
- remote push;
- daemon scheduler;
- PTY tables;
- stdout/final answer/trace based state transitions.
