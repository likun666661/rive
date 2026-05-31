# 第七章：Phase 3 Dispatch Ledger 测试计划

## 1. 测试目标

Phase 3 只验证 **Dispatch Ledger + Team Report/Status MVP**。

测试目标是确认 Phase 2 的 evidence-bound fact 可以绑定到一次明确 dispatch，并由 runtime 强校验 actor、ownership、dispatch state、idempotency 和 evidence：

```text
rive agent add
  -> rive dispatch create
  -> team status/report --dispatch ... --snapshot ...
  -> fact event + dispatch event/projection
  -> rive dispatch show/list
```

本阶段不测试 Work Graph、任务拆分、PTY delivery、agent process start/attach、`team send`、AgentFS importer/backend。

## 2. 关键验收线

1. `rive agent add/list/show` 能创建和查询最小 agent registry。
2. `rive dispatch create` 能创建 open dispatch，并绑定 target worker。
3. `team status` 必须写 dispatch-bound progress fact，但不能关闭 dispatch。
4. `team report --status done|blocked|failed` 必须写 fact，并原子更新 dispatch projection。
5. 只有 assigned worker 能 `status/report` 对应 dispatch。
6. closed dispatch 上的 `status/report/cancel` 必须被拒绝。
7. `command_id` replay 返回第一次结果；同 `command_id` 不同 payload 返回 conflict。
8. report/status 必须沿用 Phase 2 snapshot/evidence manifest integrity 校验。
9. dispatch projection/read model 必须分 `protocol` / `display`，`allowed_next_actions` 是枚举。
10. dispatch reported/blocked/failed/cancelled 不是 Work Graph done，不写 Work Graph 表或事件。
11. 无 AgentFS 安装时完整可用。

## 3. 测试分层

### Unit Tests

- Agent role enum：只接受 `orchestrator`、`worker`。
- Agent token：token 不应明文持久化；验证 token hash / token match。
- Dispatch state enum：只接受 `open`、`reported`、`blocked`、`failed`、`cancelled`。
- Report status enum：只接受 `done`、`blocked`、`failed`。
- State transitions：
  - `open -> reported|blocked|failed|cancelled`
  - `blocked -> reported|blocked|failed|cancelled`
  - `reported|failed|cancelled` 为 closed，不可再 status/report/cancel。
- `team status` transition：`open|blocked` 保持非终态，只更新 latest fact/status metadata。
- Idempotency：同 command replay 返回同一 dispatch/fact/projection。
- Conflict：同 command id 不同 payload 返回 `idempotency_conflict`。
- Ownership：actor agent 必须等于 dispatch target。
- Read model：`protocol` 字段稳定可机器读取，`display` 不参与控制流。

### Integration Tests

- `rive init` 后添加 worker agent，并能 `agent list/show`。
- `rive dispatch create --target <worker>` 创建 open dispatch。
- assigned worker `team status` 成功，dispatch 仍为 `open` 或 `blocked` 语义下的非终态。
- assigned worker `team report --status done` 成功，dispatch 进入 `reported`，latest fact/evidence 可查询。
- assigned worker `team report --status blocked` 成功，dispatch 进入 `blocked`，仍允许后续 `status` 和 `report done|failed`。
- assigned worker `team report --status failed` 成功，dispatch 进入 `failed`，后续写入被拒绝。
- `rive dispatch cancel` 对 open/blocked dispatch 成功，后续 report/status 被拒绝。
- non-assigned worker `team status/report` 被拒绝。
- invalid snapshot / manifest tamper 被拒绝，不改变 dispatch state。
- invalid token 被拒绝。
- `team list` 只展示当前 agent 可见 dispatch。

### CLI Contract Tests

- 成功命令 exit code 为 0，stdout 是 JSON envelope。
- 失败命令 exit code 非 0，并输出稳定 JSON error envelope。
- 所有写命令必须要求 `--command-id`。
- `team status/report` fact body 从 stdin 读取，支持长文本和换行。
- `--snapshot` 可出现多次，多 evidence refs 在 dispatch/fact read model 中保留。
- `rive dispatch show` 对不存在 dispatch 返回 `dispatch_not_found` 或等价稳定错误。
- agent-facing decision 字段只出现在 `protocol`：`state`、`dispatch_id`、`latest_fact_event_id`、`allowed_next_actions`、`code`、`expected_next_action`。

## 4. 端到端验收流程

最小人工验收流程：

```bash
tmp=$(mktemp -d)
cd "$tmp"
echo "work evidence" > result.txt

rive init .
agent_json=$(rive agent add worker-a --role worker)
agent_id=<从 agent_json.protocol.agent_id 读取>
agent_token=<从 agent_json.protocol.token 读取，或按实现约定读取>

dispatch_json=$(printf 'please check result.txt\n' | \
  rive dispatch create \
    --target worker-a \
    --title "check result" \
    --command-id cmd_dispatch_create_001 \
    --stdin)
dispatch_id=<从 dispatch_json.protocol.dispatch_id 读取>

snap_json=$(rive snapshot capture --label worker-report)
snapshot_id=<从 snap_json.protocol.snapshot_id 读取>

printf 'result checked and ready\n' | \
  RIVE_WORKSPACE="$tmp" RIVE_AGENT_ID="$agent_id" RIVE_AGENT_TOKEN="$agent_token" \
  team report \
    --dispatch "$dispatch_id" \
    --status done \
    --snapshot "$snapshot_id" \
    --command-id cmd_report_001 \
    --stdin

rive dispatch show "$dispatch_id"
```

检查点：

- dispatch 创建后 state 为 `open`。
- report 后 state 为 `reported`。
- `latest_fact_event_id` 存在，并能关联到 fact/evidence。
- `latest_report_status` 为 `done`。
- `allowed_next_actions` 不包含 `report` / `status` / `cancel`。
- 没有 Work Graph 表、事件或 done 状态被写入。

## 5. Required Negative Tests

### `team status` 不关闭 dispatch

流程：

1. create open dispatch。
2. assigned worker 调 `team status --dispatch ...`。
3. `rive dispatch show`。

期望：

- status 命令成功。
- dispatch 仍可继续 report/cancel。
- state 不变成 `reported` 或 `failed`。
- latest fact/evidence 可查询。

### 非 assigned worker 拒绝

流程：

1. dispatch target 为 worker-a。
2. worker-b 用自己的 token 调 `team status/report`。

期望：

- 命令失败。
- `protocol.code` 为 `dispatch_not_assigned`。
- dispatch state 和 latest fact 不变。

### closed dispatch 拒绝

覆盖三类 closed：

- `reported` 后再次 status/report。
- `failed` 后再次 status/report/cancel。
- `cancelled` 后 status/report/cancel。

期望：

- 返回 `dispatch_closed`。
- 不写新的 dispatch transition。
- 不改变 latest terminal state。

### cancel/report conflict

流程：

1. open dispatch 被 human cancel。
2. worker 随后 report。

期望：

- cancel 成功，dispatch 进入 `cancelled`。
- report 返回 `dispatch_closed`。
- 如果 report 先成功，后续 cancel 返回 `dispatch_closed`。

### command id replay / conflict

流程：

1. 同一 `command_id` + 同 payload 重放 `dispatch create`、`team status`、`team report`、`dispatch cancel`。
2. 同一 `command_id` + 不同 payload 重放上述写命令。

期望：

- 同 payload replay 返回第一次结果，不新增 event。
- 不同 payload 返回 `idempotency_conflict`，不产生副作用。

### evidence integrity

流程：

1. capture snapshot。
2. 篡改 manifest。
3. `team report/status --snapshot <snapshot>`。

期望：

- 返回 `evidence_integrity_error`。
- 不写 fact。
- 不改变 dispatch state。

### invalid token

流程：

1. 使用 assigned worker 的 agent id，但给错 token。
2. 调 `team status/report/list`。

期望：

- 返回 `agent_token_invalid`。
- 不泄露可见 dispatch 列表。
- 不写任何 fact/dispatch event。

## 6. Dispatch Projection Tests

`rive dispatch show` 必须能解释当前 state：

- `open`：`allowed_next_actions` 至少包含 `report`、`status`、`cancel`。
- `blocked`：`allowed_next_actions` 至少包含 `report`、`status`、`cancel`。
- `reported`：允许 `inspect_fact` / `inspect_evidence`，不允许 `report` / `status` / `cancel`。
- `failed`：允许 `inspect_fact` / `inspect_evidence`，不允许继续写入。
- `cancelled`：不允许 `report` / `status` / `cancel`。

projection 字段必须来自 ledger/projection，不从 natural language body 推导。

## 7. Business-State Non-Mutation Tests

Phase 3 必须守住：

```text
dispatch.reported != work_node done
dispatch.blocked != work_node blocked
dispatch.failed != work_graph failed
dispatch.cancelled != task cancelled
```

测试要求：

- Phase 3 store 中不出现 Work Graph node/edge/projection 表。
- 不出现 `work.*`、`task.*`、`node.*`、`graph.*` event。
- `rive dispatch show` 可以说 dispatch `reported`，但不能说 task completed。
- dispatch read model 不暴露 Work Graph ready/done/reviewable 状态。

## 8. AgentFS 相关测试

Phase 3 不依赖 AgentFS。

必须证明：

```text
PATH 中没有 agentfs 时，rive init / agent add / dispatch create / snapshot capture /
team status/report/list / dispatch show 仍然通过。
```

如果后续实现预留 AgentFS seam：

- AgentFS 只能提供 evidence backend/importer。
- dispatch state 只能由 runtime command/event 改变。
- AgentFS 文件变化不能绕过 `team status/report` 改 dispatch projection。

## 9. Done Definition

task #29 完成条件：

- 测试计划覆盖 Phase 3 范围、非范围、错误语义和 projection 边界。
- 实现完成后，unit/integration/CLI contract tests 全部通过。
- 手动 e2e 验证 `init -> agent add -> dispatch create -> snapshot -> team report -> dispatch show`。
- `team status` 不关闭 dispatch。
- non-assigned worker、closed dispatch、cancel/report conflict、invalid token、invalid evidence、manifest tamper 都有稳定行为。
- command id replay/conflict 对所有写命令有效。
- report/status fact 绑定 evidence，并能复查 body blob/hash。
- 没有 AgentFS 的环境下测试通过。
- dispatch 状态不推进 Work Graph/task/node 业务状态。
