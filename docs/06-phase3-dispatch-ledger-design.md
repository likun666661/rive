# Phase 3: Dispatch Ledger + Team Report/Status MVP

Phase 3 的目的，是把 Phase 2 的通用 fact ledger 往协作协议推进一层：让 agent 的 `status` / `report` 不再只是孤立事实，而是绑定到一次明确的工作请求，也就是 dispatch。

Phase 3 仍然不做 Work Graph、不做 PTY、不做 agent 进程管理。它先解决一个更基础的问题：

```text
谁把什么工作派给谁？
被派的人是否有权汇报？
这次派活当前是否 open / reported / blocked / failed / cancelled？
这次汇报引用了哪些 evidence？
```

## 1. Phase 3 想解决什么问题

Phase 2 已经有了：

```text
team fact record -> agent.fact.recorded -> evidence_ref/snapshot_ref
```

这证明 Rive 可以记录“某 agent 声明了一条结构化事实，并引用证据”。但它还不能表达协作中的执行尝试。

一个 report 只有 fact 还不够，因为 runtime 还不知道：

- report 回应的是哪一次派活。
- 这次派活是否仍然 open。
- report 的 actor 是否就是 assigned worker。
- status 是否只是进度更新，还是关闭 dispatch。
- cancel/report 并发时谁赢。
- 重复 report 是幂等 replay，还是非法重复终态。

Phase 3 要引入 dispatch ledger，解决“结构化事实如何绑定到一次工作请求”的问题。

## 2. 为什么 Phase 3 先做 dispatch，而不是 Work Graph

Work Graph 表达的是工作结构和完成约束；dispatch 表达的是一次执行尝试。两者必须分开。

如果直接做 Work Graph，会马上混淆三件事：

- `task node done` 是目标约束满足。
- `dispatch reported` 是 worker 汇报了一次执行尝试。
- `fact recorded` 是 agent 声明了一条可审计事实。

Phase 3 先把 execution attempt 这一层做稳：

```text
dispatch -> status/report/cancel -> evidence-bound fact -> dispatch projection
```

Phase 4 再让 Work Graph node 绑定 dispatch。这样 graph 的 `ready/done/reviewable` 才能引用 dispatch/fact/evidence，而不是直接相信自然语言。

## 3. Phase 3 的最小闭环

```text
rive init
rive agent add worker-a --role worker
rive dispatch create --target worker-a --title "check X" --stdin
rive snapshot capture --label worker-report
team report --dispatch <dispatch_id> --status done --snapshot <snapshot_id> --command-id <id> --stdin
rive dispatch show <dispatch_id>
```

验收判断：

- dispatch 创建后是 open。
- assigned worker 可以 report。
- 非 assigned worker 不能 report。
- report fact 绑定 dispatch 和 evidence。
- `status` 不关闭 dispatch；`report` 才能进入终态或 blocked/failed。
- 重复 `command_id` replay 同一结果。
- 同 `command_id` 不同 payload 返回 conflict。
- cancel/report 竞争有明确错误。
- 仍然没有 Work Graph done 语义。

## 4. 最小对象模型

### Agent

Phase 3 需要最小 agent registry。它不是 PTY run manager，只是权限和 dispatch target 的身份表。

```json
{
  "agent_id": "agent_...",
  "name": "worker-a",
  "role": "worker",
  "token_hash": "sha256:...",
  "created_at": "...",
  "status": "active"
}
```

初始 role：

- `orchestrator`
- `worker`

Phase 3 可以先允许 human CLI 创建 agent/token，不启动进程。

### Dispatch

dispatch 是一次执行尝试，不是 Work Graph node。

```json
{
  "dispatch_id": "disp_...",
  "created_event_id": "evt_...",
  "created_by": { "kind": "human" },
  "target_agent_id": "agent_...",
  "title": "check X",
  "body_blob_ref": "blob:...",
  "body_hash": "sha256:...",
  "state": "open",
  "latest_fact_event_id": null,
  "created_at": "...",
  "updated_at": "..."
}
```

最小 state：

- `open`
- `reported`
- `blocked`
- `failed`
- `cancelled`

`reported` 是 dispatch 的执行尝试终态，不等于 task/graph done。

### Dispatch Fact Binding

`team status` / `team report` 仍然复用 Phase 2 fact 机制，但 payload 中增加 dispatch binding：

```json
{
  "event_type": "dispatch.reported",
  "dispatch_id": "disp_...",
  "fact_event_id": "evt_...",
  "report_status": "done",
  "evidence_refs": ["snapshot:..."]
}
```

实现上可以选择：

- 一个 event 同时包含 fact payload 和 dispatch transition payload。
- 或先写 `agent.fact.recorded`，再同事务写 `dispatch.reported`。

关键要求：fact write 和 dispatch projection mutation 必须在同一事务里，不允许出现 fact 写了但 dispatch 没变，或 dispatch 变了但 fact 丢了。

## 5. Command Surface

### Human CLI

```text
rive agent add <name> --role orchestrator|worker
rive agent list
rive agent show <name-or-id>

rive dispatch create --target <agent> --title <title> --command-id <id> --stdin
rive dispatch list
rive dispatch show <dispatch_id>
rive dispatch cancel <dispatch_id> --command-id <id> --reason <text>
```

Phase 3 的 `rive dispatch create` 可以由 human 使用。后续 `team send` 再复用同一 dispatch creation path。

### Agent-Facing CLI

```text
team status \
  --dispatch <dispatch_id> \
  --snapshot <snapshot_id> \
  --command-id <id> \
  --stdin

team report \
  --dispatch <dispatch_id> \
  --status done|blocked|failed \
  --snapshot <snapshot_id> \
  --command-id <id> \
  --stdin

team list
```

规则：

- `team status` 写 progress fact，但不关闭 dispatch。
- `team report --status done` 将 dispatch projection 变为 `reported`。
- `team report --status blocked` 将 dispatch projection 变为 `blocked`。
- `team report --status failed` 将 dispatch projection 变为 `failed`。
- `team list` 只读当前 agent 可见 dispatch/agent 信息。

Phase 3 先不做 `team send`。原因是 send 涉及 orchestrator-to-worker delivery；delivery 要等 PTY/adapters 设计，不应该混进 no-PTY dispatch ledger。

## 6. Runtime Validation

### 通用校验

- `command_id` 必填。
- workspace 存在。
- actor env 必须有效：`RIVE_WORKSPACE`、`RIVE_AGENT_ID`、`RIVE_AGENT_TOKEN`。
- actor token 必须匹配 agent registry。
- fact body 必须通过 stdin 写入并保存为 blob/hash。
- snapshot/evidence 必须属于同 workspace，并通过 manifest integrity 校验。
- response 继续使用 `protocol/display` 分层。

### Dispatch create

- target agent 必须存在且 role 是 `worker`。
- body/title 必须非空。
- 同 `command_id` replay 返回同一 dispatch。
- 同 `command_id` 不同 payload 返回 `idempotency_conflict`。

### Status

- dispatch 必须存在。
- dispatch 必须是 `open` 或 `blocked`。
- actor 必须是 dispatch 的 target worker。
- `status` 只能写 progress fact，不得关闭 dispatch。

### Report

- dispatch 必须存在。
- actor 必须是 dispatch 的 target worker。
- dispatch 必须仍可 report：`open` 或 `blocked`。
- report status 必须是 `done|blocked|failed`。
- `done` -> `reported`。
- `blocked` -> `blocked`。
- `failed` -> `failed`。

### Cancel

- Phase 3 先只允许 human cancel。
- cancel open/blocked dispatch -> `cancelled`。
- cancel 已终态 dispatch 返回 conflict，不重开状态。

## 7. Idempotency and Conflict Rules

所有写命令都必须有 `command_id`。

| 场景 | 规则 |
| --- | --- |
| 重放同一 `command_id` + 同 payload | 返回第一次结果 |
| 同 `command_id` + 不同 payload | `idempotency_conflict` |
| report 已终态 dispatch | `dispatch_closed` |
| cancel 已终态 dispatch | `dispatch_closed` |
| status closed dispatch | `dispatch_closed` |
| 非 assigned worker report/status | `dispatch_not_assigned` |
| invalid snapshot | `evidence_not_found` |
| manifest tamper | `evidence_integrity_error` |

Phase 3 不需要复杂分布式 CAS；SQLite transaction + current state check 足够。但事件和 projection mutation 必须原子。

## 8. Read Models

### `rive dispatch show`

必须暴露 protocol 字段：

```json
{
  "protocol": {
    "dispatch_id": "disp_...",
    "state": "reported",
    "target_agent_id": "agent_...",
    "created_event_id": "evt_...",
    "latest_fact_event_id": "evt_...",
    "latest_report_status": "done",
    "evidence_refs": [
      { "kind": "snapshot", "snapshot_id": "snap_..." }
    ],
    "allowed_next_actions": []
  },
  "display": {
    "title": "check X",
    "summary": "reported by worker-a"
  }
}
```

`allowed_next_actions` 是给 agent/human automation 的稳定枚举，不是自然语言。

初始枚举：

- `report`
- `status`
- `cancel`
- `inspect_fact`
- `inspect_evidence`
- `none`

### `team list`

当前 worker 至少能看到：

- 自己的 open/blocked dispatch。
- dispatch title。
- latest state。
- allowed next actions。

## 9. Error Envelope

继续使用 Phase 2 的错误 envelope：

```json
{
  "protocol": {
    "ok": false,
    "code": "dispatch_not_assigned",
    "retryable": false,
    "expected_next_action": "stop_and_report"
  },
  "display": {
    "message": "agent is not assigned to this dispatch"
  }
}
```

新增推荐错误码：

- `agent_not_found`
- `agent_token_invalid`
- `dispatch_not_found`
- `dispatch_not_assigned`
- `dispatch_closed`
- `invalid_dispatch_status`
- `invalid_report_status`
- `idempotency_conflict`
- `evidence_not_found`
- `evidence_integrity_error`

`display.message` 不承载协议语义。

## 10. Non-Scope

Phase 3 不做：

- Work Graph node/edge/projection。
- task decomposition。
- task done/review/approval semantics。
- PTY prompt injection。
- agent process start/stop/attach。
- `team send` delivery。
- AgentFS importer or backend.

这意味着 Phase 3 完成后，Rive 能管理 dispatch attempt，但不能说整个任务图完成。

## 11. Implementation Plan

建议拆两个任务：

1. **实现任务**
   - Agent registry table + `rive agent add/list/show`。
   - Dispatch table + event/projection writes.
   - `rive dispatch create/list/show/cancel`。
   - `team status/report/list`。
   - Reuse Phase 2 fact body/evidence binding.
   - Idempotency and conflict handling.
   - Tests for ownership, closed dispatch, invalid evidence, replay/conflict.

2. **测试任务**
   - valid flow: init -> agent add -> dispatch create -> snapshot -> team report -> dispatch show.
   - `team status` does not close dispatch.
   - non-assigned worker rejected.
   - closed dispatch report rejected.
   - cancel/report conflict.
   - duplicate command replay / changed payload conflict.
   - evidence integrity validation.
   - no Work Graph state/table/event mutation.
   - no AgentFS dependency.

## 12. Phase 3 完成后的下一步

Phase 3 完成后，Rive 会有：

```text
evidence snapshot
fact event
dispatch ledger
dispatch-bound status/report
dispatch projection
```

这会给 Phase 4 的 Work Graph 提供基础：Work Graph node 可以绑定 dispatch，node projection 可以引用 dispatch/fact/evidence 来判断 reviewable/done，而不是让 graph 直接相信自然语言。
