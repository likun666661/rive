# Phase 2: Agent Fact Ledger + Evidence Binding MVP

Phase 2 要解决的问题不是任务图，也不是 PTY 编排，而是更底层的事实链路：

```text
agent 声明的结构化事实 -> runtime 接受的 fact event -> Phase 1 evidence/snapshot -> 可查询、可复查
```

Phase 1 已经能保存 agent 工作现场的证据快照。Phase 2 要让外部 CLI agent 能把一条结构化事实写入 Rive，并把这条事实绑定到已有证据。这样后续 `team report`、dispatch、Work Graph、review/approval 都能引用事实和证据，而不是直接相信自然语言输出或文件变化。

## 1. 想解决什么问题

外部 CLI agent 现在有两类输出：

- 自然语言：例如“我完成了”“测试通过了”“我发现问题 X”。
- 工作现场：文件、diff、manifest、hash、artifact、terminal transcript。

这两类都不能直接成为 Rive 的协调事实。

自然语言没有稳定 schema，不能重放、幂等、校验、查询。工作现场只能证明“当时环境里有什么”，不能表达 agent 声明了什么。Phase 2 的目标是在两者之间加一条受 runtime 约束的事实入口：

```text
team fact record
  -> schema / actor / workspace / evidence validation
  -> append-only fact event
  -> queryable read model
```

因此 Phase 2 解决的是：

1. **结构化事实写入**：agent 必须通过 tool-like CLI 写 fact，不能靠自然语言推进系统理解。
2. **事实和证据绑定**：每条 fact 可以引用 Phase 1 的 `snapshot_id` / `evidence_ref`，并由 runtime 校验。
3. **事实不越权**：fact 只是“agent 声明了一件事并引用证据”，不改变 dispatch、task、node、graph 状态。

## 2. 为什么这么做

Rive 的核心边界是：只有 runtime 接受的结构化 command/event 才能改变事实系统。

Phase 1 让证据可保存，但没有 fact。若下一步直接做 dispatch 或 Work Graph，就会遇到一个问题：dispatch report、review、node completion 到底引用什么？如果只引用自然语言，就会回到 prompt 约束；如果只引用 snapshot，就缺少 agent 的结构化声明。

Phase 2 先补齐最小 fact ledger：

```text
snapshot/evidence 是材料
fact event 是声明
后续 dispatch/report/graph 可以消费 fact event
```

这样 Phase 3 可以把 `team status/report` 建在 fact event 之上，Phase 4 可以让 Work Graph 的 projection 引用 fact/evidence，而不是让 graph 直接读取自然语言或文件系统。

## 3. Phase 2 的最小用户流

```text
rive init
rive snapshot capture --label before-report
team fact record --type report --snapshot <snapshot_id> --stdin
rive fact show <event_id>
```

验收判断：

- 有效 snapshot 可以被 fact 引用。
- 无效或跨 workspace snapshot 会被拒绝。
- fact body 被存为 blob/hash，而不是只存在 terminal 输出里。
- `rive fact show` 能复查 fact 和 evidence 绑定。
- fact 不会让任何 dispatch/task/node/graph 进入 done 或 reported。

## 4. Agent-Facing CLI ABI

Phase 2 新增最小 `team` 命令：

```text
team fact record \
  --type status|report|observation \
  --snapshot <snapshot_id> \
  --command-id <idempotency_key> \
  --stdin
```

规则：

- `--stdin` 是 fact body 的推荐输入方式，避免 shell quoting 和长文本问题。
- `--command-id` 是写操作幂等键。重复同一个 command id 必须返回第一次写入的结果。
- `--snapshot` 可以出现多次，表示这条 fact 引用多份 evidence。
- `--type` 只使用稳定枚举。Phase 2 先支持 `status`、`report`、`observation`。
- `team` 输出 JSON envelope。agent 决策只能依赖 `protocol` 字段，不能解析 `display.message`。

Phase 2 先不把 `fact record` 等同于正式 `team report`。它只是底层事实入口。

## 5. Human-Facing Query

Phase 2 新增查询面：

```text
rive fact list
rive fact show <event_id>
```

或在实现中复用：

```text
rive log --type fact
```

无论命令名如何，read model 必须分层：

```json
{
  "protocol": {
    "event_id": "...",
    "fact_type": "report",
    "actor": { "kind": "agent", "id": "..." },
    "body_hash": "sha256:...",
    "body_blob_ref": "...",
    "evidence_refs": ["snapshot:..."],
    "created_at": "..."
  },
  "display": {
    "summary": "...",
    "message": "..."
  }
}
```

`display` 只服务人类阅读，不参与 agent 决策、状态迁移或恢复逻辑。

## 6. Fact Event Envelope

Phase 2 的最小 fact event：

```json
{
  "protocol_version": "rive.fact.v0",
  "event_id": "evt_...",
  "command_id": "cmd_...",
  "event_type": "agent.fact.recorded",
  "workspace_id": "w_...",
  "actor": {
    "kind": "agent",
    "agent_id": "agent_...",
    "run_id": "run_..."
  },
  "fact_type": "report",
  "body_hash": "sha256:...",
  "body_blob_ref": "blob:...",
  "evidence_refs": [
    {
      "kind": "snapshot",
      "snapshot_id": "snap_...",
      "manifest_hash": "sha256:..."
    }
  ],
  "created_at": "2026-05-31T19:00:00Z"
}
```

Phase 2 可以先把 fact event 写入 SQLite `events` 表，并增加 fact-specific query table 或 JSON projection。实现可以保守，但读写语义必须稳定。

## 7. Runtime Validation

`team fact record` 必须经过 runtime 校验：

- `command_id` 必填，重复调用返回第一次结果。
- actor 从 `RIVE_AGENT_ID` / `RIVE_AGENT_TOKEN` / workspace env 推导，不信任 CLI 参数自报身份。
- workspace 必须存在。
- `fact_type` 必须是允许枚举。
- body 必须写入 blob，记录 hash。
- 每个 snapshot/evidence ref 必须存在。
- evidence 必须属于同一个 workspace。
- manifest hash 必须能重新验证或和存储记录一致。
- fact event 写入成功后才返回 `ok: true`。

失败返回稳定错误 envelope，例如：

```json
{
  "ok": false,
  "protocol": {
    "code": "evidence_not_found",
    "retryable": false,
    "expected_next_action": "fix_arguments"
  },
  "display": {
    "message": "snapshot does not exist in this workspace"
  }
}
```

Phase 2 推荐的错误码：

- `missing_command_id`
- `invalid_fact_type`
- `workspace_not_found`
- `actor_not_authenticated`
- `evidence_not_found`
- `evidence_workspace_mismatch`
- `evidence_integrity_error`
- `idempotency_conflict`
- `body_too_large`

## 8. Non-Scope

Phase 2 不做：

- Work Graph。
- dispatch state machine。
- `team report/status` 的正式语义。
- orchestrator/worker role 权限。
- PTY delivery。
- AgentFS 主路径或完整 wrapper。
- 自动 completion、review、approval。

最重要的约束是：`agent.fact.recorded` 不推进任何业务状态。它只是后续业务协议可以引用的事实材料。

## 9. Implementation Plan

建议拆成两个任务并行：

1. **实现任务**
   - 扩展 SQLite store：fact event、body blob、idempotency record、fact query。
   - 扩展 `team` CLI：`team fact record`。
   - 扩展 `rive` CLI：`rive fact list/show`。
   - 复用 Phase 1 snapshot/evidence store 校验 evidence refs。
   - 保持 protocol/display response contract。

2. **测试任务**
   - e2e：init -> snapshot capture -> fact record -> fact show。
   - invalid snapshot 被拒绝。
   - cross-workspace snapshot 被拒绝。
   - manifest hash mismatch 被拒绝。
   - duplicate command id 返回同一 event。
   - changed body with same command id 返回 conflict。
   - fact body hash/blob 可复查。
   - fact 不改变 dispatch/task/node/graph 状态。
   - 无 AgentFS 依赖可运行。

## 10. Phase 2 完成后的下一步

Phase 2 完成后，Rive 会有：

```text
evidence snapshot
fact event
fact -> evidence binding
fact query/replay
```

这会成为 Phase 3 的基础。Phase 3 可以开始把 `team status/report`、agent identity、dispatch binding、role permission 接到 fact ledger 上。Work Graph 应该等到 fact/dispatch 两层都稳定后再实现。
