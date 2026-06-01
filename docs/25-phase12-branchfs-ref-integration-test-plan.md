# Phase 12 BranchFS Workspace Branch / Ref Integration 测试计划

## 1. 测试目标

Phase 12 验证 worker output 可以从 shared workspace mutation 升级为 BranchFS branch ref：

```text
BranchFS branch workspace
  -> worker changes files in isolated branch
  -> team report with branch_ref
  -> branch integration pending
  -> rive branch commit / abort / reject
  -> integration committed / aborted / rejected
  -> work accept / scheduler auto-committed
```

验收重点：

- workers 不直接写 parent workspace。
- branch/ref 是事实系统可查询对象。
- commit/abort/reject/conflict 是显式 ledger event。
- work done 仍然由 accept event 推进，不由 BranchFS commit 或 stdout 推进。

## 2. Backend Availability

### BranchFS unavailable

Run:

```text
rive scheduler run --workspace-mode branchfs ...
```

when `branchfs` binary or mount support is unavailable.

Expected:

- stable error `branch_backend_unavailable` or `branch_mount_failed`.
- no worker process starts.
- no Work DAG state changes.

### Test fake backend

Unit/integration tests may use a local fake backend:

```text
RIVE_BRANCH_BACKEND=local-fake
```

Expected:

- fake backend implements the same `BranchWorkspaceBackend` contract.
- production command defaults to real BranchFS unless explicitly test-configured.

## 3. Branch Workspace Tests

### Create branch workspace

Given root/work/dispatch/run:

Expected:

- `branch_workspaces` row exists.
- branch path is distinct from parent workspace.
- `RIVE_WORKSPACE` for worker is the branch path.
- `RIVE_STATE_WORKSPACE` points to parent workspace.
- parent workspace is unchanged before commit.

### Worker report binds branch ref

Worker calls:

```text
team report \
  --dispatch <dispatch> \
  --status done \
  --snapshot <snapshot> \
  --workspace-ref branchfs:<mount>:<branch> \
  --command-id <id>
```

Expected:

- dispatch `reported/done`.
- work node `reviewable`.
- `work_ref_bindings.workspace_ref` stores the branch ref.
- `branch_integrations` has pending integration.
- node is not `done` yet.

### Branch commit

Run:

```text
rive branch commit <integration_id> --command-id commit-1
```

Expected:

- backend commit is called once.
- parent workspace receives branch changes.
- integration state becomes `committed`.
- `branch.integration.committed` event is written.
- replay returns same projection without a second backend commit.

### Branch abort

Run:

```text
rive branch abort <integration_id> --command-id abort-1
```

Expected:

- backend abort is called.
- parent workspace stays unchanged.
- integration state becomes `aborted`.
- work node is not accepted.

### Branch reject

Run:

```text
rive branch reject <integration_id> --command-id reject-1 --stdin
```

Expected:

- rejection reason body/hash stored.
- integration state becomes `rejected`.
- optional backend abort policy is explicit.
- work node remains not done.

### Commit conflict

Fake backend returns conflict.

Expected:

- stable code `branch_integration_conflict`.
- integration state becomes `conflict`.
- parent workspace stays recoverable.
- work node is not accepted.

## 4. Work Accept Guard

Before branch commit:

```text
rive work accept <node> --require-committed-branch
```

Expected:

- returns `branch_ref_not_committed`.

After branch commit:

- command succeeds.
- writes `work.node.accepted`.

## 5. Scheduler BranchFS Tests

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
  --workspace-mode branchfs \
  --acceptance-mode auto-committed
```

Expected:

- A/B run in separate branch paths.
- parent workspace remains unchanged until branch commit.
- scheduler commits A/B branch integrations.
- A/B and root become done through explicit accept events.
- branch integration rows are committed.

### Parallel branch isolation

Two workers write the same path differently.

Expected:

- writes are isolated in separate branches.
- parent workspace does not see either change until commit.
- first commit behavior follows BranchFS/backend policy.
- conflict/reject/abort is explicit; no silent overwrite.

### Replay no duplicate branches

Replay same scheduler command.

Expected:

- no worker relaunch.
- no duplicate branch workspace rows.
- no duplicate commits/integrations.

### Manual mode

Run with:

```text
--workspace-mode branchfs
--acceptance-mode manual
```

Expected:

- workers report branch refs.
- nodes become reviewable.
- integrations remain pending.
- scheduler state `waiting_review`.
- no branch commit or accept event.

## 6. Real BranchFS Smoke

If local BranchFS and macFUSE are available:

1. Mount BranchFS on a temp project.
2. Run a one-node OpenCode worker through `--workspace-mode branchfs`.
3. Worker creates `phase12-branchfs.txt`.
4. Worker reports branch ref.
5. `rive branch commit` applies changes to parent.

Expected:

- parent file appears only after commit.
- integration state is `committed`.
- work node can be accepted after committed branch.
- trace/debug remains debug-only.

If BranchFS is unavailable on the machine, record the skip reason and run fake backend tests instead. This is not a failure for CI, but real smoke is required before calling the adapter production-ready.

## 7. Regression

Run:

```text
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Required:

- Phase 11 shared workspace scheduler still passes.
- Phase 10 sandbox tests still pass.
- Phase 8/9 Work DAG tests still pass.
- Existing dispatch/fact/snapshot tests still pass.

## 8. Non-goal Checks

Phase 12 must not introduce:

- GitHub PR creation.
- remote push.
- daemon scheduler.
- PTY tables.
- stdout/final answer/trace based state transitions.
- direct dependency on BranchFS layout outside `BranchWorkspaceBackend`.

