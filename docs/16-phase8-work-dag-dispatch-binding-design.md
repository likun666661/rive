# Phase 8: Work DAG / Dispatch Binding MVP

Phase 8 的目标，是把 Phase 7 已经跑通的 agent-to-agent delegation edge，挂到一个可审计、可恢复、可推导状态的 Work DAG 上。

Phase 7 已经证明：

```text
orchestrator agent
  -> team send --to worker --runner opencode|codex --wait
  -> worker dispatch
  -> worker calls team status/report
  -> delegation result from dispatch projection
```

Phase 8 要证明：

```text
human/orchestrator creates Work DAG
  -> ready work node is delegated to worker
  -> worker dispatch/report binds to that node
  -> worker submits snapshot/artifact refs
  -> node projection becomes reviewable
  -> explicit accept/validation makes node done
  -> dependent nodes become ready
```

这不是 planner，也不是后台 scheduler。它只是把任务结构、依赖和完成约束放进 Rive 的事实系统。

## 1. Phase 8 想解决什么问题

现在 Rive 已经有几块底座：

1. snapshot evidence：能保存 worker 工作现场。
2. fact ledger：能记录 agent 结构化事实。
3. dispatch ledger：能记录一次执行尝试。
4. runner adapters：能真实启动 OpenCode / Codex。
5. `team send --wait`：orchestrator 能派生 worker 并等 report。

缺的是 Work DAG：

- 一个目标如何拆成多个节点。
- 节点之间的依赖、验证、取代关系如何记录。
- 哪些节点 ready，哪些节点 blocked，为什么 blocked。
- worker 完成后提交的 ref 如何绑定到节点。
- `report done` 之后为什么不能直接算 node done。
- orchestrator 如何围绕同一个 DAG 派生多个 worker 协同工作。

Phase 8 解决的是 **work structure + execution binding + reviewable projection**。

## 2. Codex 给 Phase 8 的边界

Codex 没有通用 Work DAG。它把几件事分开：

```text
thread_spawn_edges   = parent-child execution tree
mailbox/status       = runtime-owned communication and status projection
agent_jobs           = assigned-thread result ledger
rollout trace        = debug/replay evidence
```

Rive 不应该照搬 Codex thread tree 做任务图。Phase 8 应该拆三层：

```text
Work DAG          = task semantics, dependencies, completion constraints
Execution Tree    = dispatch/delegation/runner attempts
Result/Ref Ledger = worker report submits snapshot/artifact/workspace refs
```

这三层都存在同一个 `.rive/rive.db`，但投影和语义必须分开。

## 3. Rust 实现原则

不要在 Rust 里做互相引用的对象链表，也不要维护长期存活的 `Node { children: Vec<&Node> }` 结构。

Phase 8 的 graph 应按关系型/索引型方式实现：

```text
work_nodes(id, kind, title, status input fields, node_version)
work_edges(id, from_node_id, to_node_id, edge_type, graph_version)
work_bindings(node_id, dispatch_id?, artifact_ref?, evidence_ref?)
```

查询时临时构建 projection index：

```text
nodes_by_id: HashMap<WorkNodeId, WorkNodeRecord>
incoming_by_node: HashMap<WorkNodeId, Vec<WorkEdgeRecord>>
outgoing_by_node: HashMap<WorkNodeId, Vec<WorkEdgeRecord>>
dispatches_by_node: HashMap<WorkNodeId, Vec<DispatchRecord>>
```

这些内存结构只是 query-time projection，可以随时丢弃重建。事实源仍然是 SQLite event/table。

## 4. Non-goals

Phase 8 明确不做：

- LLM auto-planner。
- daemon scheduler。
- background async queue。
- PTY attach。
- TUI。
- arbitrary cyclic graph。
- worker mesh。
- marketplace / role preset。
- 从 stdout/final answer/trace 推断 node state。
- `team report done` 直接把 node 标为 done。

## 5. Work DAG Model

### work_nodes

最小字段：

```text
work_node_id
kind                 # objective | task | check | review
title
description_hash?
description_blob_ref?
acceptance_criteria_hash?
acceptance_criteria_blob_ref?
created_by_agent_id?
created_by_human_id?
status_input         # active | cancelled | superseded
node_version
created_at
updated_at
```

`status_input` 不是 projection state。它只记录不可由依赖推导的事实输入，例如取消、取代、重新打开。

### work_edges

最小 edge types：

```text
decomposes_to   # parent -> child
depends_on      # node -> prerequisite
validates       # review/check node -> target node
supersedes      # new node -> old node
```

约束：

- v0 是 DAG。
- 写入 `decomposes_to / depends_on / validates` 时必须检测 cycle。
- 返工、循环、重试用 `reopen / supersede / retry dispatch` event 表达，不把 Work DAG 变成循环执行图。
- `dispatch` 不是 graph edge，只是 node execution binding。

### work_dispatch_bindings

一个 work node 可以有多个 dispatch attempts。

```text
work_node_id
dispatch_id
binding_kind        # execution | review | validation
created_by_event_id
created_at
```

当前 node 的 active/latest dispatch 是 projection，不是 topology。

### work_ref_bindings

worker report 可以提交 refs。

```text
work_node_id
dispatch_id
fact_event_id
snapshot_id?
artifact_ref?
workspace_ref?
diff_ref?
created_at
```

Phase 8 最小 ref 是 Phase 1/2 的 `snapshot_id`。`artifact_ref` / `workspace_ref` / `diff_ref` 可以先作为 optional string refs，后续再接 Git worktree commit/branch 或 AgentFS snapshot。

## 6. Commands

### Human-facing `rive work`

```text
rive work create --kind objective|task|check|review --title <title> [--stdin]
rive work edge add --type decomposes-to|depends-on|validates|supersedes --from <node> --to <node> --command-id <id>
rive work list
rive work show <node>
rive work inspect <node>
rive work accept <node> --command-id <id> [--stdin]
rive work reopen <node> --command-id <id> [--stdin]
```

`inspect` 是 Phase 8 的核心 debug view。它必须解释：

- 当前 projection state。
- state 由哪些 facts 推导。
- 缺哪些 requirements。
- allowed next actions。

### Agent-facing `team send --work`

Phase 7 `team send` 增加：

```text
team send \
  --work <work_node_id> \
  --to <worker> \
  --runner opencode|codex \
  --title <title> \
  --command-id <id> \
  --wait \
  --stdin
```

Runtime 校验：

- actor role is orchestrator。
- work node exists。
- work node projection allows delegation。
- target is worker。
- existing Phase 7 send validation still applies。

Side effects：

1. Create/replay delegation as Phase 7。
2. Create/replay worker dispatch。
3. Bind dispatch to work node.
4. Launch worker only on inserted delegation, never on replay.
5. Return delegation + dispatch + work node projection.

### Worker report ref

Existing `team report` gains optional work ref arguments:

```text
team report \
  --dispatch <dispatch_id> \
  --status done|blocked|failed \
  --snapshot <snapshot_id> \
  [--artifact-ref <ref>]... \
  [--workspace-ref <ref>] \
  [--diff-ref <ref>] \
  --command-id <id> \
  --stdin
```

If the dispatch is bound to a work node, report writes:

- existing dispatch/fact transition。
- `work_ref_binding` for the submitted snapshot/artifact/workspace refs。
- projection input that can make the node `reviewable`。

It does **not** write node `done`。

## 7. Projection States

Work node projection states:

```text
proposed       # exists, not ready because parent/deps incomplete or missing criteria
ready          # all predecessors satisfied, no active dispatch
running        # has open bound dispatch
blocked        # latest bound report/status is blocked, or missing requirements
reviewable     # worker reported done and required refs exist
done           # accepted or validated
cancelled      # explicit cancel
superseded     # superseded by another node
needs_attention # inconsistent/failed/cancelled execution requires human/orchestrator action
```

Protocol read model:

```json
{
  "state": "reviewable",
  "derived_from": ["disp_...", "evt_...", "snap_..."],
  "missing_requirements": [],
  "allowed_next_actions": ["accept", "create_validation_node", "reopen", "delegate_again"],
  "latest_dispatch_id": "disp_...",
  "latest_report_status": "done",
  "node_version": 3,
  "graph_version": 8
}
```

Display read model can include human explanation, but agents must branch only on protocol fields.

## 8. Readiness Rules

v0 uses all-predecessor DAG semantics.

A node is `ready` when:

- it is not cancelled/superseded/done。
- all `depends_on` prerequisites are done。
- if it has parent `decomposes_to` relation, parent is active。
- required validation nodes are either not required yet or done by policy。
- there is no open bound dispatch。

A node is `blocked` when:

- any dependency is not done。
- latest bound dispatch is blocked。
- required artifact/snapshot/ref is missing after a done report。
- there is a failed/cancelled execution without recovery。

A node is `reviewable` when:

- latest bound dispatch is reported done。
- required snapshot/ref is present and valid。
- dependencies remain satisfied。
- no acceptance/validation event has marked it done yet。

A node is `done` only when:

- `rive work accept` writes an accepted event; or
- a `validates` review/check node passes and policy accepts the target.

## 9. Idempotency and CAS

All graph mutation commands require `command_id`.

Suggested scopes:

```text
work.create: actor + command_id
work.edge.add: actor + command_id
work.accept: actor + command_id
work.reopen: actor + command_id
team.send --work: actor + command_id
```

Graph mutations should use expected versions:

```text
expected_node_version?
expected_graph_version?
```

Conflict examples:

- adding an edge with stale graph version -> `graph_version_conflict`
- accepting a node whose projection is not reviewable -> `work_node_not_reviewable`
- delegating a blocked node -> `work_node_not_ready`
- adding edge that creates cycle -> `work_graph_cycle`

Replay returns the original result and must not relaunch workers.

## 10. Execution Binding

Phase 8 should not replace dispatch/delegation. It should bind them.

```text
work node
  -> dispatch attempt 1
  -> dispatch attempt 2
  -> validation dispatch
```

Execution attempt states remain dispatch states:

```text
open | reported | blocked | failed | cancelled
```

Work node state is a projection over:

- work node fields。
- graph edges。
- bound dispatches。
- report facts。
- submitted refs。
- accept/reopen/cancel/supersede events。

This prevents execution history from polluting DAG topology.

## 11. Multiple Workers

Phase 8 should support an orchestrator dispatching multiple ready nodes to workers.

MVP does not need a scheduler. The orchestrator can do:

```bash
rive work inspect <root>
team send --work <node_a> --to worker-a --runner opencode --wait ...
team send --work <node_b> --to worker-b --runner codex --wait ...
```

Projection must make this safe:

- a node with open dispatch is not ready for another execution unless policy allows retry。
- independent ready nodes can be delegated separately。
- parent node completion depends on child nodes and acceptance policy, not on one worker report。

## 12. Trace Boundary

Codex/OpenCode trace remains debug-only.

Trace may help explain:

- what prompt a worker saw。
- which commands it ran。
- why it failed to call `team report`。

Trace cannot:

- make a work node ready/reviewable/done。
- substitute for `team report`。
- substitute for snapshot/artifact refs。
- become graph topology。

## 13. Phase 8 Completion Criteria

Phase 8 passes when:

1. Work nodes and DAG edges are persisted and queryable.
2. Cycle creation is rejected.
3. `rive work inspect` returns protocol projection with reasons and allowed actions.
4. `team send --work <node>` binds a worker dispatch to the node.
5. Worker report with snapshot/ref moves node to `reviewable`, not `done`.
6. `rive work accept` moves reviewable node to `done`.
7. Dependent nodes become ready only after predecessors are done.
8. `command_id` replay does not create duplicate edges, dispatches, or worker runs.
9. Existing Phase 7 real agent-to-agent delegation still works.
10. At least one real orchestrator uses multiple workers across a small DAG and reaches accepted/done nodes through the ledger.
