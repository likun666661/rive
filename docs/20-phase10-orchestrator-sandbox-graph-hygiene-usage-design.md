# Phase 10: Orchestrator Capability Sandbox / Graph Hygiene / Usage MVP

Phase 10 的目标，是把 Phase 9 跑通的 OpenCode orchestrator 从“能完成任务”收紧成“可控、可审计、可解释成本”的工程系统。

Phase 9 已经证明：

```text
OpenCode orchestrator
  -> team work create/edge/inspect/accept
  -> team send --work --runner opencode --wait
  -> OpenCode worker reports snapshot/ref
  -> root work projection done
  -> SWE-bench targeted test pass
```

Phase 10 要解决 Phase 9 暴露出的三个问题：

1. Orchestrator 权限太大，理论上可以绕过 worker 直接改 workspace。
2. Orchestrator 可能创建未连接的临时 work node，root done 仍然成立但 graph hygiene 不干净。
3. SWE-bench smoke token 消耗只能临时解析 OpenCode stdout，Rive 还不能系统回答成本问题。

## 1. Phase 10 想解决什么问题

Phase 10 的核心边界是：

```text
orchestrator:
  can write control-plane facts
  can read workspace context
  cannot directly mutate implementation workspace

worker:
  can mutate workspace/artifacts
  must report facts/snapshots/refs

runtime:
  can enforce/audit capability boundaries
  can inspect graph hygiene
  can aggregate usage/cost debug metrics
```

这不是把 orchestrator 变成只读 agent。它仍然可以写 Work DAG 和进展记录。限制的是 workspace mutation：代码修改、产物写入、测试执行、patch/ref 生成应该由 worker 或 validator 完成。

## 2. Non-goals

Phase 10 明确不做：

- 完整 OS sandbox / container isolation。
- macOS sandbox profile / Docker / VM。
- 多 worker scheduler。
- 自动 planner 重写。
- Patch merge / PR creation。
- 精确账单系统。
- 跨 provider 统一价格表。
- 大规模 SWE-bench 批跑。

Phase 10 先做 MVP：capability guard + workspace mutation audit + graph hygiene projection + OpenCode usage aggregation。

## 3. Capability Boundary

### Orchestrator allowed

Orchestrator 可以直接使用：

```text
team work create/list/show/inspect/edge add/accept/reopen/note
team send --work --runner opencode --wait
rive work/list/show/inspect/debug read commands
read-only repo inspection commands:
  ls, find, rg, grep, cat, sed, head, tail
  git status, git diff, git log, git show, git grep
```

Orchestrator 可以写 control-plane facts：

```text
work.node.created
work.edge.created
work.node.accepted
work.node.reopened
work.note.recorded
delegation.*
```

### Orchestrator denied

Orchestrator 默认不能直接做：

```text
write source files
write worker artifacts
run tests as source of validation
apply patches
create commits/branches
execute package managers or build systems as implementation actions
```

Examples that must be denied or audited:

```text
python -m pytest
pytest
cargo test
npm test
git commit
git checkout -b
apply_patch
python scripts that write files
shell redirection that mutates source files
```

### Worker allowed

Worker can:

```text
edit files
run tests
capture snapshot
team status/report
submit artifact/workspace/diff refs
```

Worker still cannot mutate Work DAG topology unless a later phase explicitly permits worker-proposed follow-up nodes.

## 4. Phase 10 Enforcement Strategy

Phase 10 should not pretend to be a perfect sandbox. A CLI agent with shell access can bypass simple wrappers. The MVP should combine two layers:

### Layer A: restricted orchestrator PATH

`rive runner orchestrator` creates a run-local planner tool directory:

```text
.rive/debug/runs/<run_id>/planner-bin/
```

It injects this directory at the front of PATH for the orchestrator run.

Planner PATH includes:

- Rive binaries: `team`, `rive`。
- Read wrappers for allowed commands。
- Deny wrappers for common mutation/execution commands。

The deny wrapper returns stable error text/code, for example:

```text
orchestrator_capability_denied
```

### Layer B: workspace mutation audit

Before launching orchestrator, runner records a workspace baseline:

```text
git status --porcelain
git diff --binary
file manifest excluding .rive/
```

After orchestrator exits, runner checks for direct workspace mutation outside `.rive/`.

If orchestrator changed implementation workspace directly, runner must fail with:

```text
orchestrator_workspace_mutation
```

This catches bypasses such as shell redirection, direct Python writes, or editor commands. It is not full prevention, but it turns a policy violation into a durable failure instead of silent success.

### Worker PATH must not inherit planner restriction

When orchestrator calls `team send`, the worker runner must receive normal worker capabilities.

Implementation rule:

- `rive runner orchestrator` stores original PATH in an env var such as `RIVE_WORKER_BASE_PATH`。
- `team send` / shared runner core uses `RIVE_WORKER_BASE_PATH` for worker processes when present。
- Worker env must remove orchestrator-only vars:

```text
RIVE_ORCHESTRATOR_ROOT_WORK_ID
RIVE_AVAILABLE_WORKERS
RIVE_ORCHESTRATOR_CAPABILITY_PROFILE
```

This prevents the planner sandbox from accidentally weakening workers.

## 5. Orchestrator Progress Updates

Orchestrator needs to update progress. That should be a control-plane write, not a workspace write.

Add:

```text
team work note <work_node_id>
  --kind progress|decision|blocker|risk|validation
  --command-id <id>
  --stdin
```

Semantics:

- Orchestrator-only in v0。
- Writes `work.note.recorded` event。
- Shows in `work inspect` as notes。
- Does not directly change `ready/reviewable/done` state。
- Can be referenced in `derived_from` or display explanation, but not as completion proof.

This gives orchestrator a structured way to say:

- why it created a node,
- why a node is blocked,
- why it accepted/reopened,
- what it plans next.

## 6. Graph Hygiene

Phase 9 showed an orphan work node can appear without blocking root success. Phase 10 should make graph hygiene explicit.

### Root-scoped nodes

When `RIVE_ORCHESTRATOR_ROOT_WORK_ID` is present, every `team work create` should bind the new node to that root scope:

```text
work_root_bindings
  root_work_node_id
  work_node_id
  created_by_agent_id
  created_by_run_id
  created_at
```

Human-created `rive work` nodes can remain unscoped unless a root is specified.

### Hygiene projection

Add:

```text
rive work graph inspect --root <root>
team work graph inspect --root <root>
```

Protocol output:

```text
root_work_node_id
state
hygiene_state        # clean | dirty
orphan_nodes
unconnected_nodes
incomplete_reachable_nodes
reviewable_unaccepted_nodes
missing_requirements
allowed_next_actions
```

### Root accept guard

When orchestrator accepts the root node, runtime must check:

- all root-scoped nodes are reachable from root,
- all reachable child nodes required by `decomposes_to/depends_on/validates` are done,
- no root-scoped orphan active/reviewable nodes exist,
- no blocked/needs_attention reachable node remains.

If not clean:

```text
work_graph_not_closed
```

This is stronger than Phase 9: root `done` now means the root-scoped graph is closed, not merely that direct children are accepted.

## 7. Usage Accounting

Phase 9 SWE-bench smoke showed OpenCode already emits token usage in stdout JSONL, e.g. `step_finish.tokens`.

Phase 10 should standardize this into Rive debug metrics:

```text
rive debug trace usage
  [--run <run_id>]
  [--agent <agent_id>]
  [--dispatch <dispatch_id>]
  [--work <work_node_id>]
  [--root <root_work_node_id>]
```

Output protocol:

```text
runs[]
  run_id
  adapter
  agent_id?
  dispatch_id?
  work_node_id?
  input_tokens
  output_tokens
  reasoning_tokens
  cache_read_tokens
  total_tokens
  tool_output_bytes
  trace_event_count

totals
  input_tokens
  output_tokens
  reasoning_tokens
  cache_read_tokens
  total_tokens
  non_cache_tokens
  trace_event_count
```

Implementation details:

- Prefer provider/native usage fields from OpenCode JSONL.
- If usage is absent, return `usage_unavailable` for that run and still report trace event count / output bytes.
- Do not write usage into evidence/fact/work state.
- Usage is debug/read model only.

## 8. Runner Response Changes

`rive runner orchestrator` can include a debug-only summary:

```text
usage_summary:
  total_tokens
  non_cache_tokens
  trace_event_count
  usage_available
```

This is non-normative:

- does not affect success,
- does not affect work projection,
- does not become evidence by default.

## 9. Phase 10 Done Criteria

Phase 10 is done when:

1. Orchestrator runner uses a planner capability profile by default.
2. Orchestrator can write Work DAG/control notes but cannot silently mutate workspace source files.
3. Worker runner does not inherit planner restriction and can still write artifacts/tests.
4. Root-scoped graph hygiene catches orphan/unconnected nodes.
5. Root accept fails with `work_graph_not_closed` when hygiene is dirty.
6. `rive debug trace usage` reports OpenCode token usage for Phase 9-style runs.
7. Real OpenCode orchestrator e2e still passes under capability guard.
8. SWE-bench smoke still passes or fails with a clear capability/hygiene error that points to the next implementation gap.
