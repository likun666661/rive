# 第五章：Phase 2 Fact Ledger 测试计划

## 1. 测试目标

Phase 2 只验证 **Agent Fact Ledger + Evidence Binding MVP**。

测试目标是确认外部 CLI agent 能通过 tool-like `team` 命令写入一条结构化 fact，并把这条 fact 绑定到 Phase 1 已捕获的 evidence snapshot：

```text
team fact record
  -> runtime validation
  -> agent.fact.recorded event
  -> body blob/hash
  -> evidence_refs / snapshot_refs
  -> rive fact list/show read model
```

本阶段不测试 Work Graph、dispatch 状态机、PTY delivery、正式 `team report/status` 语义、orchestrator/worker 权限模型，也不要求安装 AgentFS。

## 2. 关键验收线

1. `team fact record` 必须写入 `agent.fact.recorded` event。
2. fact body 必须保存为 blob，并记录稳定 `body_hash` / `body_blob_ref`。
3. fact 必须能引用一个或多个 Phase 1 snapshot/evidence。
4. runtime 必须校验 snapshot 存在、属于同一 workspace，且 manifest hash 可验证。
5. `command_id` 必填；重复同一 `command_id` 返回第一次写入结果。
6. 同一 `command_id` 携带不同 body/type/evidence 时必须返回 `idempotency_conflict` 或等价稳定错误。
7. `rive fact list/show` 必须输出 `protocol` / `display` 两层 read model。
8. fact event 只能表达 agent 声明，不能推进 dispatch、task、node、graph 状态。
9. 无 AgentFS 安装时完整可用。

## 3. 测试分层

### Unit Tests

- Fact type enum：只接受 `status`、`report`、`observation`。
- Body hashing：相同 body 得到相同 sha256；body 变化 hash 变化。
- Body blob：blob ref 能反查原始 body，hash 与内容一致。
- Evidence validation：snapshot id / evidence ref 解析、存在性、workspace 归属、manifest hash 校验。
- Idempotency：同一 `command_id` replay 返回同一 `event_id` 和 read model。
- Idempotency conflict：同一 `command_id` 但 body/type/evidence 变化返回稳定 conflict。
- Error envelope：错误只依赖 `protocol.code` / `retryable` / `expected_next_action`，不解析 `display.message`。
- Read model：`protocol` 字段稳定可机器读取，`display` 字段只用于人类说明。

### Integration Tests

- `rive init` 后能执行 snapshot capture，再记录 fact。
- `team fact record --type report --snapshot <snapshot_id> --command-id <id> --stdin` 生成 fact event。
- `rive fact show <event_id>` 能展示 fact body hash、blob ref、actor、fact type、evidence refs。
- `rive fact list` 能列出 fact events，并能按时间稳定排序。
- 重复执行同一 command id 返回第一次结果，不新增 fact event。
- 无效 snapshot 被拒绝，不写 fact event。
- 跨 workspace snapshot 被拒绝，不写 fact event。
- 破坏 manifest 或 manifest hash 后，fact record 返回 `evidence_integrity_error` 或等价稳定错误。
- 未初始化 workspace 执行 `team fact record` 返回 `workspace_not_found` 或等价稳定错误。
- 未提供 agent env/token 时返回 `actor_not_authenticated` 或等价稳定错误。

### CLI Contract Tests

- 成功命令 exit code 为 0，stdout 是 JSON envelope。
- 失败命令 exit code 非 0，stderr 或 stdout 按实现约定输出 JSON error envelope。
- `team fact record` 写操作必须要求 `--command-id`。
- fact body 默认从 stdin 读取，长文本和换行不会被 shell quoting 破坏。
- `--snapshot` 可出现多次，多 evidence refs 在 read model 中完整保留。
- `rive fact show` 对不存在 event id 返回稳定 `fact_not_found` 或等价错误。
- 所有 agent 可依赖字段都在 `protocol`，自然语言说明只在 `display`。

## 4. 端到端验收流程

最小人工验收流程：

```bash
tmp=$(mktemp -d)
cd "$tmp"
echo "hello" > report.txt

rive init .
snap_json=$(rive snapshot capture --path . --label before-report)
snap_id=<从 snap_json.protocol.snapshot_id 读取>

printf 'implemented initial snapshot evidence flow\n' | \
  RIVE_AGENT_ID=agent_test RIVE_AGENT_TOKEN=test_token \
  team fact record \
    --type report \
    --snapshot "$snap_id" \
    --command-id cmd_phase2_001 \
    --stdin

rive fact list
rive fact show <event-id>
```

检查点：

- `team fact record` 返回 `ok=true`。
- response 中有 `event_id`、`fact_type`、`body_hash`、`body_blob_ref`、`evidence_refs`。
- `evidence_refs[0].snapshot_id` 等于 capture 生成的 snapshot。
- `rive fact show <event-id>` 能复查 body hash/blob ref 和 evidence ref。
- SQLite events 中新增 `agent.fact.recorded`，但没有 dispatch/task/node/graph 相关 event。

## 5. Negative / Recovery Tests

### 无效 evidence

```bash
printf 'body\n' | team fact record \
  --type report \
  --snapshot snap_missing \
  --command-id cmd_missing_snapshot \
  --stdin
```

期望：

- 命令失败。
- `protocol.code` 为 `evidence_not_found` 或等价稳定 code。
- 不写入 `agent.fact.recorded`。

### 跨 workspace evidence

流程：

1. workspace A capture snapshot。
2. workspace B 执行 `team fact record --snapshot <A snapshot>`。

期望：

- 命令失败。
- 返回 `evidence_workspace_mismatch` 或等价稳定 code。
- workspace B 不写 fact event。

### manifest integrity

流程：

1. capture snapshot。
2. 手动篡改 snapshot manifest 或存储记录。
3. 执行 `team fact record --snapshot <snapshot>`。

期望：

- 命令失败。
- 返回 `evidence_integrity_error` 或等价稳定 code。
- 不写 fact event。

### command id replay

流程：

1. 用 `command_id=cmd_same`、body A 写 fact。
2. 用相同 `command_id=cmd_same`、body A 再写一次。

期望：

- 第二次返回第一次的 `event_id`。
- event count 不增加。

### command id conflict

流程：

1. 用 `command_id=cmd_conflict`、body A 写 fact。
2. 用相同 `command_id=cmd_conflict`、body B 写 fact。

期望：

- 第二次失败。
- 返回 `idempotency_conflict` 或等价稳定 code。
- 不写第二条 fact event。

## 6. Business-State Non-Mutation Tests

Phase 2 必须守住这个不变量：

```text
agent.fact.recorded != dispatch reported
agent.fact.recorded != task done
agent.fact.recorded != node ready/done
```

测试要求：

- fact record 前后，store 中不出现 dispatch/task/node/graph projection 表写入。
- 如果实现中已经存在 placeholder 表，fact record 不改变这些表的 row count 或状态字段。
- `rive fact list/show` 可以展示 fact，但 `rive snapshot/evidence` 之外的业务命令不应把它解释成完成。
- read model 文案可以说“recorded a report fact”，不能说“task completed”。

## 7. AgentFS 相关测试

Phase 2 不依赖 AgentFS。

必须保留 Phase 1 的约束：

```text
PATH 中没有 agentfs 时，rive init / snapshot capture / team fact record / rive fact show 仍然通过。
```

如果实现中预留 AgentFS importer/backend：

- AgentFS 只能产出或导入 evidence refs。
- `team fact record` 仍然只接受 runtime 可验证的 evidence ref。
- AgentFS 文件变化不能绕过 `team fact record` 直接写 fact event。
- AgentFS 不能绕过 runtime 改 dispatch/task/node/graph projection。

## 8. Done Definition

task #25 完成条件：

- 测试计划覆盖 Phase 2 范围、非范围和错误语义。
- 实现完成后，unit/integration/CLI contract tests 全部通过。
- 人工 e2e 验证 `init -> snapshot capture -> team fact record -> rive fact show/list` 闭环。
- invalid / cross-workspace / integrity error / idempotency replay / idempotency conflict 都有稳定行为。
- fact body blob/hash 和 evidence binding 可复查。
- 没有 AgentFS 的环境下测试通过。
- fact event 不推进 dispatch/task/node/graph 状态。
