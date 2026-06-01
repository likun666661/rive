# Phase 11: Work DAG Scheduler / Parallel OpenCode Worker Pool MVP

Phase 11 的目标，是把 Phase 9/10 已经跑通的 OpenCode orchestrator control loop，拆成更清楚的两层：

```text
orchestrator:
  creates / maintains the Work DAG and control notes

runtime scheduler:
  finds ready nodes
  launches multiple OpenCode workers
  binds dispatches / refs
  advances accepted nodes by explicit policy
```

Phase 9/10 证明了一个真实 OpenCode orchestrator 可以创建 DAG、派生 worker、accept 节点，并在 SWE-bench smoke 里闭环。但那条路径仍然让 orchestrator 自己串行执行 `team send --wait`。Phase 11 要证明：DAG 一旦存在，Rive runtime 可以作为本地 scheduler 并发调度 ready work nodes，让 orchestrator 不再承担 worker pool 和重试循环。

## 1. Phase 11 想解决什么问题

当前系统已经有：

1. Work DAG：节点、依赖、root hygiene、reviewable/done projection。
2. Runner：OpenCode / Codex workers 可以真实执行 dispatch。
3. Delegation：`team send --work --wait` 可以从 orchestrator 派生 worker。
4. Sandbox：orchestrator 只能写控制面事实，不能直接写 workspace。

缺的是 runtime-owned scheduling：

- 多个 ready nodes 怎么并发启动。
- 依赖节点完成后，后续节点怎么自动解锁并进入执行。
- replay 怎么保证不重复启动 worker。
- worker stdout/final answer 看起来成功但没 report 时，scheduler 怎么拒绝成功。
- scheduler 怎么把执行记录绑定回 Work DAG，而不让 execution graph 污染 topology。

Phase 11 解决的是 **ready-node scheduling + worker pool execution + explicit accept policy**。

## 2. Non-goals

Phase 11 明确不做：

- 长驻 daemon。
- remote workers。
- PTY attach。
- Codex 作为主路径。
- 自动 planner / 自动拆任务。
- Worker-to-worker mesh。
- Git branch / commit ref 管理。
- patch merge / conflict resolution。
- full SWE-bench batch runner。
- 从 stdout/final answer/trace 推进业务状态。

本阶段仍然 OpenCode-only。Codex runner 保持回归测试，不作为 Phase 11 主验证路径。

## 3. Command Shape

新增 foreground scheduler command：

```text
rive scheduler run \
  --root <root_work_node_id> \
  --runner opencode \
  --worker <worker-agent>... \
  --command-id <idempotency-key> \
  --max-parallel <n> \
  --acceptance-mode manual|auto-reported \
  [--opencode-bin <path>] \
  [--timeout-seconds <seconds>]
```

MVP 只支持：

```text
--runner opencode
--acceptance-mode manual|auto-reported
```

`manual` 是默认值：

- scheduler 只执行 ready nodes。
- worker `team report done` 后 node 进入 `reviewable`。
- scheduler 停在 `waiting_review`，由 human/orchestrator 用 `rive work accept` / `team work accept` 明确接受。

`auto-reported` 是显式测试/automation policy：

- 只有 worker dispatch 已 `reported` 且 latest report status 为 `done`。
- 只有 report 绑定了 snapshot/ref。
- runtime scheduler 写入显式 `work.node.accepted` event。
- report 本身仍然不直接等于 done。

## 4. Scheduler Projection

新增 scheduler read model：

```text
scheduler_run_id
command_id
root_work_node_id
runner
max_parallel
acceptance_mode
state                 # running | completed | waiting_review | stalled | failed
created_at
completed_at?
```

Node execution attempts are separate:

```text
scheduler_node_runs
  scheduler_run_id
  work_node_id
  dispatch_id
  worker_agent_id
  worker_run_id
  state               # claimed | running | reported | accepted | failed
  started_at
  completed_at?
```

These tables are scheduler projection / execution ledger. They are not Work DAG topology. Work topology still only comes from `work_nodes` and `work_edges`.

## 5. Ready Node Selection

Scheduler should compute ready nodes from existing Work projection, not from ad hoc state:

```text
candidate:
  reachable from root
  not root
  projection.state == ready
  no outgoing decomposes_to edge
  no open dispatch bound to the node
  not already claimed by this scheduler run
```

Phase 11 should not dispatch aggregate parent nodes by default. A parent with `decomposes_to` children is completed by its children plus explicit accept policy.

Preflight:

- reject cycle or corrupted graph if detected.
- reject root-scoped orphan/unconnected nodes before scheduling.
- allow incomplete reachable nodes; they are the point of scheduling.

If there are no candidates and root is not done:

- if there are reviewable nodes and mode is `manual`, return `waiting_review`。
- otherwise return `work_scheduler_stalled` with missing requirements from projection.

## 6. Parallel Execution

`--max-parallel` controls how many worker processes can be active.

Implementation can be simple:

- foreground process only, no daemon.
- use worker threads or child process handles.
- each child gets a distinct `run_id` and run-scoped token.
- each child launch creates / reuses dispatch + `work_dispatch_binding`.
- replay of the scheduler command must not relaunch completed child runs.

Important invariant:

```text
child success = dispatch projection
not stdout
not final answer
not debug trace
```

If a worker exits 0 but does not call `team report`, scheduler must treat that node as not completed.

## 7. Worker Prompt Contract

Scheduler-generated worker prompt should be narrower than orchestrator prompt:

```text
You are a Rive worker assigned to one work node.

Work node:
<work_node_id>

Root:
<root_work_node_id>

Rules:
1. Inspect your assigned node with `rive work inspect <node>` if needed.
2. Make only implementation changes required for this node.
3. Capture evidence with `rive snapshot capture`.
4. Report with `team report --dispatch <dispatch> --status done|blocked|failed --snapshot <id>`.
5. Include `--artifact-ref`, `--workspace-ref`, or `--diff-ref` when you create a result.
6. Do not mutate Work DAG topology.
7. Do not claim success in natural language without `team report`.
```

Workers are not allowed to write graph topology. They can inspect assigned work and report execution facts.

## 8. Acceptance Policy

Phase 11 keeps the Phase 8 rule:

```text
team report done -> work node reviewable
work accept event -> work node done
```

Scheduler may write the accept event only under an explicit policy:

```text
manual:
  never auto-accept.

auto-reported:
  auto-accept a reviewable node only when:
    latest dispatch status is done
    at least one snapshot/ref is bound
    node is still reviewable
```

Root aggregate accept:

- If all decomposed children are done and root becomes reviewable, scheduler may accept root under `auto-reported`.
- In `manual`, scheduler returns waiting_review and leaves root reviewable.

This preserves auditability: done always has an accept event, even when the actor is `runtime_scheduler`.

## 9. Idempotency and Claims

Scheduler command idempotency:

- same command_id + same root/workers/policy returns the same scheduler run projection.
- replay does not relaunch child processes.
- same command_id + different request returns `idempotency_conflict`.

Node claim:

- before launching a worker, scheduler writes an execution claim for the work node.
- a second active scheduler cannot claim the same ready node.
- stale claim recovery is out of scope for Phase 11; return a stable `work_node_already_claimed`.

## 10. Protocol / Display Boundary

Scheduler responses must keep normative and display fields separate:

```text
protocol:
  scheduler
  root_work
  launched_nodes
  completed_nodes
  waiting_review_nodes
  stalled_nodes
  usage_summary?

display:
  summary
  explanation
```

Agent decisions must depend only on protocol fields.

## 11. Success Criteria

Phase 11 is complete when:

1. A pre-created Work DAG can be scheduled by `rive scheduler run` with OpenCode workers.
2. Two independent ready nodes can run under `--max-parallel 2`.
3. A dependent node runs only after prerequisites become done.
4. `manual` mode stops at reviewable nodes.
5. `auto-reported` mode explicitly accepts reported nodes and can drive root to done.
6. Replay does not relaunch workers.
7. Worker stdout/final answer without `team report` is not success.
8. Trace/usage remain debug-only.

