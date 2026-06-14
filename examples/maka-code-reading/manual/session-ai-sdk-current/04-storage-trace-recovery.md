# 当前 Storage / JSONL Recovery / Telemetry Trace 现状分析

> 基线: `4dd1bf1` · 对比起点: `05ca5a3` · 深度: maintainer

---

## scope

| 类别 | 文件 | 行数 | 职责 |
|------|------|------|------|
| **持久化—会话事件日志** | `packages/storage/src/session-store.ts` | 397 | JSONL 会话记录（header + StoredMessage 行） |
| **持久化—用量统计** | `packages/storage/src/telemetry-repo.ts` | 305 | `telemetry.json` 单文件 upsert 模式 |
| **持久化—产物** | `packages/storage/src/artifact-store.ts` | 291 | `metadata.jsonl` + 文件系统产物 |
| **内存 trace（不持久）** | `packages/runtime/src/run-trace.ts` | 152 | RunTrace 事件模型 + recorder 回调 |
| **运行时编排** | `packages/runtime/src/session-manager.ts` | 819 | sendMessage / recovery / turn 生命周期 |
| **工具产物派生** | `packages/runtime/src/tool-artifacts.ts` | 234 | Write/Edit/Bash → ArtifactCandidate |
| **工具输出流** | `packages/runtime/src/tool-output-delta.ts` | 105 | stdout/stderr 分块 + 脱敏 |
| **后端接线** | `packages/runtime/src/ai-sdk-backend.ts` | 655 | RunTrace 构造 / telemetry 回调注入点 |
| **测试—会话存储** | `packages/storage/src/__tests__/session-store.test.ts` | 588 | JSONL corruption / migration / preview |
| **测试—用量统计** | `packages/storage/src/__tests__/telemetry-repo.test.ts` | 175 | upsert / 过滤 / 分桶 / pricing |
| **测试—AI SDK 后端** | `packages/runtime/src/__tests__/ai-sdk-backend.test.ts` | 1340 | 用法 / trace / 权限 / 工具修复 |

---

## problem

**核心难题**: 当 AI SDK stream、tool execution 或 app process crash 中断时，持久化层（JSONL + telemetry.json + 产物 metadata）只能恢复"行级已落盘"的状态，而**帧内状态（partially-streamed assistant text、中间 tool 调用链、cost 计算上下文）在 crash 时刻全部丢失**。`RunTrace` 是纯内存诊断层，从不写盘，崩溃后无迹可寻。三条持久化路径（session JSONL、telemetry、artifact metadata）各自独立写入，之间没有事务边界——可能出现"session 已记录 tool_call 但 telemetry 未记录 tool invocation"的状态。

---

## current_design

### 1. Durable Event Log（Session JSONL）

**文件格式**: `<sessionsRoot>/<sessionId>/session.jsonl`

```
line 0: SessionHeader (JSON)       ← 原子重写 (tempfile + rename)
line 1: StoredMessage (JSON)       ← appendFile 追加
line 2: StoredMessage (JSON)
...
```

**关键特性**:
- 同一 session 的写操作通过 `writeQueue` (per-session Promise chain) 串行化
- `updateHeader` 是一次**全量原子重写**（读全部行 → 改 header 行 → 写 temp → rename）
- `appendMessage`/`appendMessages` 是一次 `fs.appendFile` 追加
- JSONL 坏行在读取时处理: 中间坏行 → `system_note`("jsonl_parse_error")；末尾截断行 → 静默丢弃
- `migrateHeader` 处理旧字段兼容（`backend: 'claude'` → `'ai-sdk'`，缺失 `permissionMode` → `'ask'`）

### 2. Telemetry Repo

**文件格式**: `<workspaceRoot>/telemetry.json`

```json
{
  "usageRecords": [...],
  "toolInvocations": [...],
  "pricingOverrides": [...]
}
```

**关键特性**:
- 全量文件，每次写入是原子 tempfile + rename
- 写入通过 `enqueueWrite` Promise chain 串行化
- `insertLlmCall` / `insertToolInvocation` 按 `id` upsert（同 id 后写覆盖先写）
- `load()` 惰性首次读盘；文件不存在或缺损 → `emptyFile()`
- **写入触发**: `recordLlmCall` / `recordToolInvocation` 在 `queueMicrotask` 中**异步 fire-and-forget** 执行，不阻塞 turn 主路径

### 3. Artifact Store

**文件**: `artifacts/<sessionId>/<id>-<name>` (blob) + `artifacts/metadata.jsonl` (索引)

- metadata.jsonl 是全量原子重写（tempfile + rename），无增量追加
- 每条记录按 `id` upsert
- 路径遍历防护: `isSafeRelativeArtifactPath` 拒绝 `..`、绝对路径、空段
- 产物操作（create/append/delete）通过 `enqueue` Promise chain 串行化

### 4. Runtime Trace（RunTrace）

**纯内存构造**:
- `RunTrace` 在 `AiSdkBackend.send()` 开头创建，在 `cleanupAfterTurn()` 中置 `null`
- 事件通过可选的 `record?: RunTraceRecorder` 回调抛出
- `record` 回调失败被 try/catch 静默吞掉（注释: "Tracing is diagnostic-only and must not perturb model/tool execution"）
- 没有任何内置持久化路径——调用方必须自行实现 `record` 回调
- 测试中 `recordRunTrace` 指向 `Array.push` 用于断言，生产环境未见落盘实现

### 5. SessionManager Recovery

`recoverInterruptedSessions()` 流程:
1. 列出所有非 archived 的 session
2. 读每个 session 的 messages
3. 检测 `turn_state` 状态为 `'running'` 的 turn → 标记为 `'failed'` + `errorClass: 'app_restarted'`
4. 检测 `turn_state` 为 `'completed'` 但无 `assistant` 消息的 turn → 标记为 `'failed'` + 保留原始 `errorClass`
5. 将 session status 从 `'running'`/`'waiting_for_user'` 翻转为 `'active'`
6. 写 `turn_state` 失败标记（best-effort，catch 不抛）

---

## source_evidence

| # | Evidence | Durability Boundary | Failure Mode |
|---|----------|---------------------|--------------|
| 1 | `session-store.ts:224-243` `readFilePartsUnlocked` | 中间坏行 → `system_note` 注入；末尾截断行（无 `\n`）→ 静默丢弃 | 截断若发生在 header 行 → `throw Error('Session is empty')`，整个 session 不可读 |
| 2 | `session-store.ts:152-159` `appendMessages` | `appendFile` 非原子追加；如果 crash 发生在 `appendFile` 写一半，最后一行可能截断 | 重启后截断行被丢弃，该条 StoredMessage 永久丢失 |
| 3 | `session-store.ts:246-251` `writeAtomic` | tempfile + rename 原子替换；旧文件在 rename 前完整 | crash 在 writeFile(temp) 和 rename 之间 → 留下 `.tmp` 文件，session.jsonl 回退到上一次原子写入的状态 |
| 4 | `session-store.ts:92` header 创建 | header 的第一次写入也是 `writeFile`（非原子），但 session 此时无消息 | crash 后目录存在且 JSONL 为空 → 下次 `readFileParts` 抛 `'Session is empty'`，list 中 catch 忽略 |
| 5 | `telemetry-repo.ts:202-213` `enqueueWrite` → `write` | tempfile + rename 原子全量替换 | crash 在写入中 → 丢失 crash 前 `queueMicrotask` 中尚未冲刷到持久化链的 telemetry 记录 |
| 6 | `telemetry-repo.ts:79-81` `insertLlmCall` | 修改内存 `this.file`，然后 `void this.enqueueWrite()`（fire-and-forget） | 在 `enqueueWrite()` Promise 完成前 crash → 该记录永久丢失（即使 `insertLlmCall` 已返回） |
| 7 | `run-trace.ts:63-68` `emit` | `record?.(event)` 可选回调，失败静默吞掉 | `record` 回调未设置 → 所有 trace 事件被 `?.` 跳过；回调设置但 crash → trace 丢失 |
| 8 | `ai-sdk-backend.ts:484-501` `recordLlmCall` | `this.input.recordLlmCall?.(...)` 在 finally 块中同步调用 | `recordLlmCall` 内部 `queueMicrotask` 延迟写 → telemetry.json 不是同步落盘的；crash 在 `queueMicrotask` 执行前 → telemetry 记录丢失 |
| 9 | `ai-sdk-backend.ts:256-266` RunTrace 创建 | `this.currentRunTrace = trace`，在 `cleanupAfterTurn` 中置 `null` | crash 后 trace 对象随进程销毁，无恢复可能 |
| 10 | `session-manager.ts:155-184` `recoverInterruptedSessions` | 通过 `turn_state` 行检测中断，标记 failed | 如果 crash 发生在 `appendTurnState('running')` 之后但任何 `turn_state` 更新之前 → 能恢复；如果 crash 发生在 `appendMessage(userMsg)` 之后但 `appendTurnState` 之前 → turn 无 `turn_state` 行，不会被检测到 |
| 11 | `artifact-store.ts:180-186` `writeMetadataUnlocked` | tempfile + rename 原子全量替换 | crash 在 enqueue chain 中 → 产物 metadata 回退到上一次原子写入的状态，但 blob 文件可能已落盘（无事务回滚） |
| 12 | `session-manager.ts:309-317` `sendMessage` | user message 先落盘，然后 `appendTurnState('running')`，再启动 backend | 如果 crash 在 user message 落盘后、backend.start 前 → turn 显示 running 但无 assistant output → recovery 可检测 |
| 13 | `session-manager.ts:394-430` finally 块 | `updateHeader` + `appendMessage(session_resume)` 在 finally 中 best-effort 执行 | crash 在 finally 期间 → header 信息（lastUsedAt, lastMessageAt, status）可能不准确 |

---

## recovery_flow

### 正常写入流程

```
sendMessage(sessionId, input)
│
├─ [1] store.appendMessage(userMsg)          ← JSONL 追加，同步等待
├─ [2] appendTurnState('running')             ← JSONL 追加 turn_state
├─ [3] store.updateHeader(connectionLocked)   ← 原子重写 header
├─ [4] backend.send({turnId, text, context})
│   ├─ new RunTrace(...)                     ← 内存 trace 创建
│   ├─ trace.turnStarted()                   ← trace 事件 (不持久)
│   ├─ modelAdapter.resolveModel()
│   │   ├─ trace.modelResolved()             ← trace 事件
│   │   └─ 失败 → trace.modelResolveFailed() + error event + complete
│   ├─ streamText.fullStream → normalize → queue
│   │   ├─ text_delta → queue.push(...)       ← SessionEvent (内存)
│   │   ├─ tool_call → wrapToolExecute()
│   │   │   ├─ appendMessage(toolCallMessage) ← JSONL
│   │   │   ├─ PermissionEngine.evaluate()
│   │   │   ├─ run impl / deny / timeout
│   │   │   ├─ appendMessage(toolResultMessage) ← JSONL
│   │   │   ├─ recordToolInvocation(record)   ← telemetry (queueMicrotask)
│   │   │   └─ recordToolArtifactsSafely()    ← artifact store
│   │   └─ finish → normalizeAiSdkUsage()
│   │       ├─ appendMessage(tokenUsageMessage) ← JSONL
│   │       ├─ trace.usageRecorded()          ← trace 事件
│   │       └─ queue.push(tokenUsageEvent)    ← SessionEvent
│   └─ finally: recordLlmCall(record)         ← telemetry (queueMicrotask)
│
├─ [5] for await (ev of queue) yield ev       ← 消费 NormalizedEvent
│
└─ [6] finally:
    ├─ updateHeader(lastUsedAt, lastMessageAt, hasUnread, status)  ← 原子重写
    └─ appendMessage(session_resume note)      ← JSONL 追加 (best-effort)
```

### 坏行跳过（JSONL Recovery）

**中间坏行（有换行终结）**:
- 输入: `{"type":"assistant","id":"broken"` + `\n` → 这是一个语法无效的完整行
- 输出: 原行被替换为 `system_note` 类型消息，`code: 'jsonl_parse_error'`, `lineNumber` 精确指向坏行
- 后续有效行正常解析

**末尾截断行（无换行终结）**:
- 输入: 文件最后一行是 `{"type":"assistant","id":"partial"` (无 `\n`)
- 判断 `!endsWithNewline && entry.lineNumber === lastLineNumber` → 静默丢弃
- 原理: 认为这是写入未完成导致的截断，不是真正的 corrupt 行

**末尾截断行（有换行终结）**:
- 输入: 文件最后一行是 `{"type":"assistant","id":"durably-broken"` + `\n`
- 判断: `endsWithNewline` 为 true → **不**触发截断保护 → 被视为真正的 corrupt 行 → 生成 `jsonl_parse_error` system_note

### Partial Session 恢复

```
recoverInterruptedSessions()
│
├─ list() → 过滤 status !== 'archived'
│
├─ for each session:
│   ├─ readMessages(sessionId)
│   │   ├─ 成功 → interruptedTurnRecoveries(messages)
│   │   │   ├─ 按 turnId 分组所有消息
│   │   │   ├─ 检查最新的 turn_state.status === 'running' → 恢复为 failed + 'app_restarted'
│   │   │   └─ 检查 turn_state.status === 'completed' 但无 assistant 消息 → 恢复为 failed + 原 errorClass
│   │   └─ 失败 → 如果 status 为 running/waiting_for_user → 翻转为 active（best-effort）
│   │
│   ├─ 对每个 recovery: appendTurnState('failed', lineage, {errorClass})
│   │
│   └─ 翻 session status → 'active' (best-effort)
│
└─ return recovered session IDs
```

### Telemetry 写入

```
AiSdkBackend.send() finally:
  ├─ recordLlmCall?.({
  │     sessionId, turnId, providerId, modelId,
  │     inputTokens, outputTokens, cachedInputTokens, cacheWriteInputTokens,
  │     reasoningTokens, totalTokens, latencyMs, status, errorClass, startedAt
  │   })
  │   └─ recordLlmCall(deps, record)   ← runtime/telemetry/record-llm-call.ts
  │       └─ queueMicrotask(() => {
  │             computeCost() → costUsd
  │             repo.insertLlmCall({...record, id, costUsd, date, ts})
  │               └─ this.file.usageRecords = upsertById(...)  ← 内存
  │               └─ void this.enqueueWrite()                  ← fire-and-forget 写盘
  │           })
  │
tool execute 路径:
  └─ recordToolInvocation?.({
        toolName, toolCallId, durationMs, status, argsSummary, errorClass, bytesIn, bytesOut, startedAt
      })
      └─ recordToolInvocation(deps, record)
          └─ queueMicrotask(() => {
                repo.insertToolInvocation({...record, id, date, ts})
              })
```

---

## tests

### 现有 Recovery / Corruption 测试

| 测试文件 | 测试名 | 覆盖场景 |
|----------|--------|----------|
| `session-store.test.ts:225` | `recovers readable messages around a corrupt JSONL message line` | 中间坏行生成 `jsonl_parse_error` system_note |
| `session-store.test.ts:261` | `silently drops a truncated tail JSONL message line` | 无 `\n` 尾部截断静默丢弃 |
| `session-store.test.ts:285` | `reports a corrupt tail JSONL message line when it was newline-terminated` | 有 `\n` 尾部坏行生成 system_note |
| `session-store.test.ts:106` | `rejects traversal-style session ids` | sessionId 注入防护 |
| `session-store.test.ts:119` | `migrates legacy headers without permissionMode to ask` | `backend: 'claude'` → `'ai-sdk'` 迁移 |
| `session-store.test.ts:156` | `migrates legacy headers without model to default` | 缺失 `model` → `'default'` |
| `session-store.test.ts:190` | `migrates archived legacy headers to archived status` | `isArchived: true` → `status: 'archived'` |
| `session-store.test.ts:373` | `listTurns derives latest persisted turn states and lineage` | turn_state + partialOutputRetained |
| `session-store.test.ts:405` | `listTurns projects legacy message-only turns as completed` | 无 turn_state 行的 legacy 数据 |
| `telemetry-repo.test.ts:9` | `upserts LLM calls by id` | 同 id 后写覆盖 |
| `telemetry-repo.test.ts:28` | `filters logs by range, status, provider, model, and pagination` | 多维过滤 |
| `telemetry-repo.test.ts:84` | `builds provider, model, day, hour, and tool buckets` | 分组聚合 |
| `telemetry-repo.test.ts:106` | `persists pricing overrides and reloads them from disk` | 跨实例持久化 |
| `ai-sdk-backend.test.ts:246` | `normalizes cache and reasoning tokens to messages, events, and telemetry` | usage → token_usage message + event + LlmCallRecord 三路一致性 |
| `ai-sdk-backend.test.ts:332` | `records turn, model, usage, and completion trace events` | RunTrace 事件顺序和内容 |
| `ai-sdk-backend.test.ts:405` | `trace recorder failures are best-effort` | RunTrace 不影响主路径 |
| `ai-sdk-backend.test.ts:41` | `generalizes model setup errors before emitting renderer events` | 密钥脱敏 |
| `ai-sdk-backend.test.ts:50` | `redacts and caps synthetic tool error text before storage` | 工具错误脱敏+截断 |

### 测试缺口

| 缺口 | 严重度 | 说明 |
|------|--------|------|
| **Telemetry crash 恢复** | 高 | 无测试验证 `queueMicrotask` 延迟写 + process crash 场景下的数据丢失范围 |
| **Session JSONL header corruption** | 高 | 无测试验证第一行（header）为无效 JSON 时的行为；当前代码会抛 `'Session is empty'` |
| **appendFile 中 crash 的截断恢复** | 高 | 无测试模拟 `appendFile` 写一半的截断场景（当前只测静态截断文件） |
| **SessionManager.recoverInterruptedSessions 的 E2E 行为** | 高 | session-manager 本身无单元测试文件；recovery 路径仅在集成测试中隐式覆盖 |
| **Artifact metadata 与 blob 的不一致** | 中 | 无测试验证 metadata.jsonl 原子写入失败后 blob 文件已落盘的不一致状态 |
| **RunTrace 持久化路径** | 高 | RunTrace 没有任何持久化实现，也没有测试覆盖持久化场景（因为它本身不持久） |
| **threading 边界** | 中 | 无测试验证 session writeQueue 在并发 append + updateHeader 下的正确性 |
| **Telemetry 双写幂等性** | 中 | 无测试验证 `recordLlmCall` + `recordToolInvocation` 在 turn retry 场景下不会重复记录 |
| **reasoning tokens 的端到端传递** | 中 | `session-store.test.ts` 未测试 `token_usage` 消息中的 `reasoningTokens` 字段 |
| **partial stream 恢复** | 高 | 无测试模拟 stream 中途 crash 后 assistant 消息已 `appendMessage` 一部分的场景 |

---

## risks

### 🔴 P0: RunTrace 不持久 — 崩溃后诊断信息全丢

`RunTrace` 是纯内存对象:
- 构造: `AiSdkBackend.send()` 行 256-266
- 置空: `cleanupAfterTurn()` 行 611
- 唯一出口: `record?: RunTraceRecorder` 可选回调

当前代码中 **没有任何生产级 call site 实现 `recordRunTrace`**，它只是一个预留的钩子。这意味着:
- 如果 AI SDK 返回异常 stream 响应 → 无 trace 可查
- 如果 tool execution 卡死在权限等待 → 无 trace 可查
- 如果用户报告 "turn 没有正常完成" → 只能靠 session JSONL 中的 `turn_state` + `system_note` 行推理

**`recordRunTrace` 未连接到任何持久化层。**

### 🔴 P0: JSONL Tail Corruption 的边界判断不完美

`readFilePartsUnlocked`（`session-store.ts:239`）的判断逻辑:
```typescript
if (!endsWithNewline && entry.lineNumber === lastLineNumber) continue;
```

这**只能**检测"最后一行无 `\n`"的截断——即文件恰好以未完成的行结束。以下场景无法正确恢复:
1. `appendFile` 写入 `"\n"` 后 crash → `endsWithNewline=true`，最后一行空行被过滤（无影响）
2. `appendFile` 写入一半 JSON + `\n` 后 crash → `endsWithNewline=true` → 坏行被标记为 `jsonl_parse_error` → 正确
3. **多个 `appendMessage` 调用中的 crash**: 第二次 `appendFile` 写入 `"{...}\n{..."` (无末尾 `\n`) → 只有最后一个截断行被丢弃，但倒数第二行可能是不完整的 JSON（如果第一个 `\n` 恰好落盘）

### 🟡 P1: Telemetry 写入非同步 — queueMicrotask + fire-and-forget

```
recordLlmCall (record-llm-call.ts:13):  queueMicrotask(() => { ... })
                                            ↓
                                    repo.insertLlmCall(...)    ← 修改内存 this.file
                                            ↓
                                    void this.enqueueWrite()    ← fire-and-forget Promise chain
```

崩溃窗口:
1. `insertLlmCall` 修改了 `this.file.usageRecords`（内存）
2. `enqueueWrite` 返回一个 Promise 但不被 await
3. 如果进程在 `enqueueWrite` 的 Promise 开始执行前退出 → **内存中的 usageRecord 丢失，文件未更新**
4. 如果进程在 `write(tempPath)` 和 `rename(tempPath, path)` 之间退出 → `.tmp` 文件残留，原 `telemetry.json` 未损坏但缺少最新记录

### 🟡 P1: Tool invocation telemetry 在 turn 中而非 finally 中

`recordToolInvocation` 在 `tool-runtime.ts` 的工具执行完成后立即调用（在 `wrapToolExecute` 内部）——如果工具执行成功但整个 turn 最终因 stream error 未完成，tool telemetry 已写入但 LLM call telemetry 可能在 finally 中还没写入。

### 🟡 P1: Artifact Ref Consistency

- `metadata.jsonl` 是全量原子重写，但 blob 文件是独立写入的
- 场景: `create` 先写 blob 文件，再 `append` 更新 metadata → 如果 crash 在 `append` → `enqueue` chain 中 → blob 文件已落盘但 metadata 无记录 → **孤儿文件**
- 场景: `delete` 只改 metadata status='deleted'，不删 blob → 磁盘空间泄漏（按设计）

### 🟢 P2: Cache / Reasoning Tokens 传递链

| 环节 | cache | reasoning | 状态 |
|------|-------|-----------|------|
| AI SDK raw usage | `inputTokenDetails.cacheReadTokens` / `cacheWriteTokens` | `outputTokenDetails.reasoningTokens` | ✅ 有 |
| `normalizeAiSdkUsage` | `cachedInputTokens` / `cacheWriteInputTokens` | `reasoningTokens` | ✅ 有 |
| `token_usage` StoredMessage | `cacheRead` / `cacheCreation` | 无 | ⚠️ `reasoningTokens` 不在 message schema |
| `token_usage` SessionEvent | `cacheRead` / `cacheCreation` | 无 | ⚠️ 同上 |
| `LlmCallRecord` (telemetry) | `cachedInputTokens` / `cacheWriteInputTokens` | `reasoningTokens` | ✅ 有 |
| Persisted `telemetry.json` | ✅ | ✅ | ✅ 有 |
| RunTrace `usageRecorded` | ✅ | ✅ | ✅ 有 (但不持久) |

**结论**: reasoning tokens 进入 telemetry.json 持久化 ✅，但**不在 session JSONL 的 `token_usage` 消息中** ⚠️

### 🟢 P2: 缺少 E2E 恢复测试

`SessionManager.recoverInterruptedSessions` 是恢复逻辑的唯一实现，但 `session-manager` 本身没有测试文件。所有恢复行为只在集成测试中隐式验证。

---

## next_actions

### 即时行动（P0）

| # | Action | 依赖 | 产出 |
|---|--------|------|------|
| 1 | **为 RunTrace 添加 JSONL 持久化路径**: 在 `AiSdkBackend` 或调用方实现 `recordRunTrace` 回调，将每个 `RunTraceEvent` 以 `system_note`(kind='trace') 或独立 `trace.jsonl` 写入 session 目录 | 设计决策: trace 是否需要独立 JSONL 还是复用 session.jsonl | `trace.jsonl` 或新的 StoredMessage type |
| 2 | **修复 JSONL header corruption 的空 session 误判**: `readFilePartsUnlocked` 中当 `lines[0]` 为 bad JSON 时应生成 corruption note 而非抛 `'Session is empty'` | — | patch to `session-store.ts` |
| 3 | **为 Telemetry 写入添加 flush 保证**: 将 `enqueueWrite` 的 `void` 改为可 await 的 Promise，在 `AiSdkBackend` finally 块中 await telemetry flush | `TelemetryRepo.enqueueWrite` 需要返回 Promise（当前已返回，但被 void） | patch to `telemetry-repo.ts` + `ai-sdk-backend.ts` |

### 短期行动（P1）

| # | Action | 依赖 | 产出 |
|---|--------|------|------|
| 4 | **为 `appendFile` 添加 write-ahead 标记**: 在每次 append 前写一个 `.wal` 标记文件，crash 后凭标记判断最后一条消息是否完整 | — | FileSessionStore 的 write protocol |
| 5 | **统一 `turn_state` 写入时机**: 确保 `appendTurnState('running')` 始终在 `appendMessage(userMsg)` 之前写入（当前在之后），让 recovery 能检测到所有 user-message-triggered 中断 | — | session-manager.sendMessage 写入顺序调整 |
| 6 | **添加 SessionManager `recoverInterruptedSessions` 单元测试**: 用模拟 FileSessionStore 构造各种中断场景 | — | `session-manager.test.ts` |

### 中期行动（P2）

| # | Action | 依赖 | 产出 |
|---|--------|------|------|
| 7 | **将 reasoningTokens 加入 `token_usage` StoredMessage / SessionEvent schema** | core 类型定义修改 | schema v2 |
| 8 | **Artifact orphan blob cleanup**: 定期扫描 blob 目录 vs metadata，清理无引用的孤儿文件 | artifact-store 增加 `gc()` 方法 | — |
| 9 | **为 telemetry.json 增加 corrupt JSON 恢复**: 当前 `normalizeFile` 对 bad JSON 返回 emptyFile，丢失所有历史数据；可改为可选恢复策略 | — | `FileTelemetryRepo.load` 增强 |
| 10 | **为 RunTrace 实现可恢复的 turn journal**: 将 trace 事件写入 session JSONL（新增 `trace_event` message type），使 crash 后可重放 turn 诊断 | #1 | `StoredMessage.type = 'trace_event'` |

### 架构建议

```
                     ┌─────────────────────────┐
                     │    AiSdkBackend.send()   │
                     └────────┬────────────────┘
                              │
              ┌───────────────┼───────────────┐
              ▼               ▼               ▼
        session.jsonl   telemetry.json   trace.jsonl *
       (append+atomic)  (atomic full)   (append, new)
              │               │               │
         already ✅     already ✅       NOT PERSISTED
                         (fire-forget)    (plan: action #1/#10)
```

`*`: 建议新增独立 `trace.jsonl` 或复用 session.jsonl 中的 `StoredMessage.type = 'trace_event'`。考虑到 trace 事件量大（每个 turn 5-15 事件）且仅用于诊断，独立 JSONL 更干净，避免污染 normal message list。
