# Maka Session Lifecycle / Manager 现状分析

基线: `05ca5a3..4dd1bf1` (阅读基线 `4dd1bf1`)

---

## scope

### 读过的文件

| 文件 | 行数 | 关键类/函数 |
|------|------|-----------|
| `packages/runtime/src/session-manager.ts` | 819 | `SessionManager`, `BackendRegistry`, `ActiveSession`, `sendMessage()`, `stopSession()`, `recoverInterruptedSessions()`, `headerToSummary()`, `statusFromEvent()`, `turnStatusFromEvent()` |
| `packages/runtime/src/stream-watchdog.ts` | 120 | `StreamWatchdog`, `StreamWatchdogPhase`, `formatStreamWatchdogError()` |
| `packages/runtime/src/async-queue.ts` | 78 | `AsyncEventQueue<T>` |
| `packages/core/src/session.ts` | 336 | `SessionHeader`, `SessionStatus`, `TurnRecord`, `StoredMessage`, `deriveTurnRecords()` |
| `packages/core/src/events.ts` | 383 | `SessionEvent` union (15 variants), `SessionCommand` union, `ToolResultContent` |
| `packages/core/src/runtime-inputs.ts` | 60 | `UserMessageInput`, `CreateSessionInput`, `BranchFromTurnInput`, `RetryTurnInput` |
| `packages/runtime/src/ai-sdk-backend.ts` | ~750→~750 (重构) | `AiSdkBackend`, `AgentBackend` 接口, `AiSdkBackendInput` |
| `packages/runtime/src/model-adapter.ts` | 272 (新增) | `ModelAdapter`, `ModelFactory`, `RepairableAiSdkToolCall` |
| `packages/runtime/src/tool-runtime.ts` | 608 (新增) | `ToolRuntime`, `MakaTool`, `MakaToolContext`, `DEFAULT_PERMISSION_TIMEOUT_MS` |
| `packages/runtime/src/run-trace.ts` | 152 (新增) | `RunTrace`, `RunTraceEvent`, `RunTraceRecorder` |
| `packages/runtime/src/permission-engine.ts` | diff ~15行 | `PermissionEngine.expireRequest()` 新增 |
| 测试: `__tests__/session-manager.test.ts` | 787 | 17 个测试用例 |
| 测试: `__tests__/stream-watchdog.test.ts` | 169 | 6 个测试用例 |
| 桌面测试: `session-startup-recovery-contract.test.ts` | 34 | 3 个合约断言 |
| 桌面测试: `session-message-lifecycle-contract.test.ts` | 104 | 消息生命周期 UI 合约 |
| 桌面测试: `session-status-presentation.test.ts` | 334 | 状态/阻塞原因/错误类的中文呈现合约 |

### Commit Delta 摘要

- **`packages/runtime/src/session-manager.ts`** — **无变更** (0 行 diff)
- **`packages/runtime/src/async-queue.ts`** — **无变更**
- **`packages/core/src/session.ts`** — **无变更**
- **`packages/core/src/events.ts`** — **无变更**
- **`packages/core/src/runtime-inputs.ts`** — **无变更**
- **`packages/runtime/src/stream-watchdog.ts`** — `paused: boolean` → `pauseCount: number` (支持嵌套 pause/resume)
- **`packages/runtime/src/permission-engine.ts`** — 新增 `expireRequest()` 方法
- **`packages/runtime/src/ai-sdk-backend.ts`** — 大重构: 提取 `ToolRuntime` / `ModelAdapter` / `RunTrace` 为独立类
- **`packages/runtime/src/model-adapter.ts`** — 新增 (272 行): 封装 ai-sdk `streamText` 调用、模型解析、tool repair
- **`packages/runtime/src/tool-runtime.ts`** — 新增 (608 行): 工具注册/执行/权限门控/子代理并发限制
- **`packages/runtime/src/run-trace.ts`** — 新增 (152 行): 诊断追踪 (不改变 renderer 事件)

---

## problem

Session Lifecycle 是 Maka 的核心难题，因为它是**唯一贯穿三条正交轴的状态机**:

1. **持久化轴 (JSONL)** — Session 的 header + 消息流必须以 append-only 方式写入磁盘，支持崩溃恢复。消息先于 backend 启动写入 (`session-manager.ts:309-317`)，确保即使 backend 初始化失败，用户消息也不会丢失。

2. **实时流轴 (Event Stream)** — `AiSdkBackend` 将 SDK 原生事件归一化为 `SessionEvent` 联合体 (15 种事件类型)，通过 `AsyncEventQueue` 单生产者-单消费者管道传递给 `SessionManager.sendMessage()` 的 `for await` 循环，再通过 IPC 桥转发给 renderer。permission 事件在此轴上产生 `waiting_for_user` 状态转换。

3. **UI 状态轴 (SessionStatus)** — 8 种状态 (`active` / `running` / `waiting_for_user` / `blocked` / `review` / `done` / `archived` / `aborted`) 从事件流派生，通过 `statusFromEvent()` (`session-manager.ts:741-759`) 映射。renderer 通过 `presentSessionStatus()` 和 `describeBlockedReason()` 将枚举值转换为中文 UI 文案。

这三条轴的交叉点造成了以下难题:

- **竞态**: 用户可以在一个 session 上并发发送多条消息 (第 2 条消息在第一条尚未完成时到达)。`ActiveSession.activeStreams` 计数器 (`session-manager.ts:123,341,396`) 确保 session 状态到所有流结束后才回落。
- **Abort 竞态**: 用户在流进行中按 stop 按钮 → `stopSession()` 将 turnId 加入 `stoppedTurnIds` → 流循环中的 `statusFromEvent()` 检查该集合 (`session-manager.ts:360-368`) → 即使 backend 仍发出 `complete` 事件，最终状态也强制为 `aborted`。
- **恢复竞态**: 应用崩溃时，JSONL 中可能残留 `status: 'running'` 的 session。`recoverInterruptedSessions()` 扫描所有非 archived session，对于 `turn_state.status === 'running'` 的 turn 标记为 `failed` (errorClass: `app_restarted`)，将 session 状态重置为 `active`。
- **权限超时**: `ToolRuntime` 引入 `DEFAULT_PERMISSION_TIMEOUT_MS = 300_000` (`tool-runtime.ts:64`)，通过 `PermissionEngine.expireRequest()` 在超时后主动失败已挂起的权限请求，解决"用户离开后 session 永久卡在 waiting_for_user" 的旧风险。

---

## current_design

当前代码分为**四层**，层间通过接口解耦:

### 第 1 层: 类型定义层 (`packages/core`)

- `session.ts` — 定义磁盘格式: `SessionHeader` (JSONL 第 1 行) + `StoredMessage` 联合体 (第 2 行起)。引入了 `TurnStateMessage` 和 `SystemNoteMessage` 作为"状态快照"和"审计注释"，使恢复和调试不需要回溯整个消息流。
- `events.ts` — 定义归一化事件流: `SessionEvent` (15 种类型)。`PermissionRequestEvent` → `PermissionDecisionAckEvent` 形成双向审计链路。
- `runtime-inputs.ts` — 定义 API 输入类型: `UserMessageInput` 携带 lineage 字段 (`parentTurnId`, `retriedFromTurnId`, `regeneratedFromTurnId`, `branchOfTurnId`, `parentSessionId`)。

### 第 2 层: 编排层 (`packages/runtime/src/session-manager.ts`)

`SessionManager` 是唯一的公共 API 入口。它不直接与 ai-sdk 交互，而是通过 `AgentBackend` 接口 (定义在 `ai-sdk-backend.ts:90-108`)。

核心数据结构 `ActiveSession` (`:118-128`) 跟踪:
- `backend` — 当前活跃的 `AgentBackend` 实例 (懒加载)
- `cachedHeader` — 最新 header 的本地缓存，减少存储层读取
- `activeStreams` — 活跃流计数器
- `activeTurnIds` — 当前正在执行的 turn ID 集合
- `stoppedTurnIds` — 已被用户手动停止的 turn ID 集合
- `activeTurnLineage` — turn 的 lineage 信息 (用于 abort 时写 `turn_state`)

### 第 3 层: 适配层 (`packages/runtime/src/ai-sdk-backend.ts` + 新增子模块)

`AiSdkBackend` 在 `05ca5a→4dd1bf1` 区间内被重构为三个独立类:

- **`ModelAdapter`** (`model-adapter.ts:99-272`) — 封装 ai-sdk `streamText` 调用。负责模型解析 (`resolveModel()`), 流启动 (`startStream()`), tool call repair (`repairToolCall`), 以及从 `ai-sdk` 的 `LanguageModelV2` / `streamText` 类型中解耦 Maka 核心。
- **`ToolRuntime`** (`tool-runtime.ts`) — 封装工具注册、权限门控 (`PermissionEngine.evaluate()`)、工具执行、输出流管理 (`emitOutput`)、子代理并发限制 (`MAX_ACTIVE_SUBAGENT_TOOLS_PER_TURN = 5`)、权限超时 (`DEFAULT_PERMISSION_TIMEOUT_MS`)、以及 `MakaTool` / `MakaToolContext` 接口定义。
- **`RunTrace`** (`run-trace.ts`) — 可选的诊断追踪，记录 turn/model/tool/permission/abort/usage 阶段的事件，不修改 renderer 事件流。

### 第 4 层: 基础设施层

- **`AsyncEventQueue`** (`async-queue.ts`) — 单生产者-单消费者 FIFO。`AiSdkBackend` 的 SDK 流回调和 `canUseTool` 回调都向同一个队列 push 事件；`sendMessage()` 通过 `for await` 消费。支持 `close()` (正常结束) 和 `error()` (异常终止)。
- **`StreamWatchdog`** (`stream-watchdog.ts`) — 两阶段超时监控: `connect` (30s 内无任何事件) → `idle` (120s 内无事件)。`pause()` / `resume()` 在 permission 等待期间暂停计时。**`pauseCount` 改进** (`stream-watchdog.ts:40,71-82`): 从 `boolean` 改为计数器，支持嵌套 pause (多个工具同时等待 permission 时 pause 多次，最后一次 resume 才重启计时)。
- **`PermissionEngine`** (`permission-engine.ts`) — **新增 `expireRequest()`** (`:224-233`): 允许运行时主动失败某个挂起的权限请求，不终止整个 turn。用于实现权限超时功能。

---

## source_evidence

| 文件 | 函数/构造 | 证据 | 影响 |
|------|----------|------|------|
| `session-manager.ts:299-431` | `sendMessage()` | 用户消息先写 JSONL (`:317`), 再锁 connection (`:331-333`), 再启 backend (`:336`), 最后进入事件循环 (`:352-386`) | 确保消息不丢失，即使 backend 启动失败 |
| `session-manager.ts:111-128` | `ActiveSession` | `activeStreams`, `activeTurnIds`, `stoppedTurnIds` 三个并发追踪字段 | 支持同 session 多 turn 并发 + stop 竞态安全 |
| `session-manager.ts:360-368` | `sendMessage()` 流循环 | `stoppedDuringTurn` 检查: 如果 turnId 在 `stoppedTurnIds` 中，`complete` 事件不覆盖 abort 状态 | 解决 "stop 后 backend 仍发出 complete 事件" 的竞态 |
| `session-manager.ts:433-459` | `stopSession()` | 调用 `backend.stop('user_stop')`, 将所有活跃 turnId 加入 `stoppedTurnIds`, 写 `abort` SystemNote | 确保 abort 在 backend + header + JSONL 三级都被记录 |
| `session-manager.ts:155-184` | `recoverInterruptedSessions()` | 扫描 `status !== 'archived'` 的 session, 找 `turn_state.status === 'running'` 的 turn, 标记 `failed` + `app_restarted` | 应用重启后自动恢复卡住的 session |
| `session-manager.ts:761-775` | `turnStatusFromEvent()` | 将 `complete` / `abort` / `error` 事件映射到 `TurnRecord.status` | 统一 turn 状态转换规则 |
| `stream-watchdog.ts:40,71-82` | `pauseCount` | `boolean` → `number` 计数器, 支持嵌套 pause | 修复多工具并发等待 permission 时的 pause 覆盖问题 |
| `permission-engine.ts:224-233` | `expireRequest()` | 新增方法: 主动失败单个挂起的权限请求 | 支持 `ToolRuntime` 的权限超时逻辑 |
| `tool-runtime.ts:62-64` | 常量 | `MAX_ACTIVE_SUBAGENT_TOOLS_PER_TURN = 5`, `DEFAULT_PERMISSION_TIMEOUT_MS = 300_000` | 子代理并发门控 + 权限超时门控 |
| `model-adapter.ts:99-150` | `ModelAdapter` | 从 `AiSdkBackend` 提取的独立类, 封装 `streamText` + tool repair | 解耦 ai-sdk 依赖, 使 `AiSdkBackend` 可单独测试 |
| `session.ts:237-253` | `TurnStateMessage` | 带有 `status`, `errorClass`, `abortSource`, `partialOutputRetained` 的 JSONL 消息 | 为恢复逻辑提供精确的状态快照 |
| `session.ts:255-267` | `TurnRecord` | 从 `TurnStateMessage` 派生的只读摘要 | renderer 通过 `deriveTurnRecords()` 消费, 不需要解析原始事件 |
| `events.ts:272-288` | `PermissionRequestEvent` | 携带 `requestId`, `toolUseId`, `toolName`, `category`, `reason`, `args` | 双向审计链路: request → response → ack 全部可追溯 |
| `events.ts:295-301` | `PermissionDecisionAckEvent` | 回显用户的 permission 决策 (allow/deny) | UI 和 JSONL 都能看到同一决策结果 |

---

## lifecycle_flow

### 1. Create Session
```
用户点击 "New Chat" → renderer IPC → desktop main → SessionManager.createSession(input)
  → store.create(input) → 返回 SessionHeader (status: 'active')
  → headerToSummary(header) → 返回给 renderer
```
此时没有 backend 实例，首次 `sendMessage()` 时才通过 `ensureActive()` 懒加载。

### 2. Open / Select Session
```
renderer IPC → SessionManager.getMessages(sessionId)
  → store.readMessages(sessionId) → StoredMessage[]
```
UI 层清除旧消息 → 异步读取 → 仅在 `activeIdRef` 仍匹配时 `setMessages(next)` (合约锁定在 `session-message-lifecycle-contract.test.ts:29-35`)。

### 3. Send Message (核心 `sendMessage()` 流)
```
1. store.readHeader(sessionId)                              — 读 header
2. store.appendMessage(sessionId, userMsg)                  — 先写用户消息到 JSONL
3. appendTurnState(sessionId, turnId, 'running')            — 标记 turn 为 running
4. IF !header.connectionLocked:
     store.updateHeader(sessionId, {connectionLocked: true})— 锁 connection
5. ensureActive(sessionId, header) → backend                — 懒加载/复用 backend
6. updateStatus(sessionId, 'running')                       — session → running
7. activeStreams++, activeTurnIds.add(turnId)               — 追踪并发
8. for await (ev of backend.send({...})):                   — 迭代事件流
     a. lastTs = ev.ts
     b. status = statusFromEvent(ev)                        — permission_request → waiting_for_user
        IF !stoppedDuringTurn: updateStatus(...)
     c. IF ev.type IN (complete, abort) AND !turnFailed:
          sawCompletion = true
          finalStatus = stoppedDuringTurn ? 'aborted' : statusFromEvent
          turnStatus = turnStatusFromEvent(ev)
          IF !stoppedDuringTurn: appendTurnState(...)
     d. IF ev.type === 'error':
          turnFailed = true
          finalStatus = statusFromEvent(ev)
          appendTurnState(sessionId, turnId, 'failed', ...)
     e. yield ev                                               — 转发给 IPC bridge
9. CATCH (error):
     appendTurnState(sessionId, turnId, 'failed', ...)     — 标记 turn 失败
     throw error
10. FINALLY:
     activeStreams--, activeTurnIds.delete(turnId)         — 清理并发追踪
     nextStatus = activeStreams > 0 ? 'running' : finalStatus
     store.updateHeader(sessionId, {lastUsedAt, hasUnread, status}) — 更新 header
     IF sawCompletion:
       store.appendMessage(sessionId, systemNote('session_resume')) — 调试用审计注释
```

### 4. Stream Events (Backend 内部)
```
AiSdkBackend.send(input):
  1. ModelAdapter.resolveModel()            — 解析 LanguageModel
  2. ModelAdapter.startStream({...})        — 调用 ai-sdk streamText
  3. AsyncEventQueue.push(text_delta)       — 文本增量
  4. AsyncEventQueue.push(tool_start)       — 工具调用开始
  5. ToolRuntime.executeTool(...)           — 执行工具
     a. IF permissionRequired:
          PermissionEngine.evaluate() → 挂起 → 暂停 watchdog.pause()
          ↓ 用户响应 ↑
          watchdog.resume() → continue
     b. tool.impl(args, ctx)                — 实际执行
  6. AsyncEventQueue.push(tool_result)      — 工具结果
  7. AsyncEventQueue.push(complete)         — 完成
  8. AsyncEventQueue.close()
```

### 5. Abort Turn
```
用户点击 Stop 按钮:
  renderer IPC → SessionManager.stopSession(sessionId, {source: 'stop_button'})
    → active.backend.stop('user_stop')      — 取消 backend 的 AbortController
    → for each activeTurnId:
        active.stoppedTurnIds.add(turnId)    — 标记该 turn 已被停止
    → updateStatus(sessionId, 'aborted')
    → for each activeTurnId:
        appendTurnState(sessionId, turnId, 'aborted', ..., {abortSource: 'renderer.stop_button'})
    → appendMessage(sessionId, systemNote('abort', {source: 'renderer.stop_button'}))
```
`sendMessage()` 的流循环在后续事件 (complete/error) 到达时，通过 `stoppedDuringTurn` 检查确保 abort 状态不会被覆盖。

### 6. Persist (JSONL 写入时序)
```
每条消息写入:
  store.appendMessage(sessionId, message)
    → storage 层: append-only 追加到 JSONL 文件末尾

Header 更新:
  store.updateHeader(sessionId, patch)
    → storage 层: read-rewrite-write (atomic temp + rename)
    → 同一 session 的 header 写入串行化 (per spec §5.2)
```

### 7. Recover (启动恢复)
```
App 启动 → desktop main.ts:3601
  → recoverInterruptedSessionsOnStartup()
    → runtime.recoverInterruptedSessions()
      → store.list() → 过滤 status !== 'archived'
      → for each session:
          messages = store.readMessages(session.id)
          recoveries = interruptedTurnRecoveries(messages)
            → 找 latest turn_state.status === 'running' → 标记 failed
            → 找 completed 但无 assistant 消息且有 failed state → 标记 failed
          → for each recovery:
              appendTurnState(sessionId, turnId, 'failed', lineage, {errorClass})
          → IF status was 'running' or 'waiting_for_user':
              updateStatus(sessionId, 'active')
```

---

## tests

### 已有测试覆盖的 Invariants

| 测试文件 | 用例数 | 覆盖的 invariant |
|---------|--------|-----------------|
| `session-manager.test.ts` | 17 | permission mode 更新 → 写 audit note; 流运行中拒绝 mode 切换; 多 turn 并发 → 最后完成才回落到 active; backend 配置变更 → rebuild backend; metadata-only update → 保留 backend; 流运行中拒绝 backend 配置变更; backend build 失败 → 标记 turn failed + session blocked; turn 运行时 session=running, 完成后=active; permission handoff → waiting_for_user; 等待 permission 时拒绝 mode 切换; error 事件 → blocked + turn failed; error 后 late complete → 不覆盖 error; abort 事件 → aborted; partial output 保留; stopSession 记录 abortSource; stop 后 late completion → 仍为 aborted; 启动恢复: running turns → failed app_restarted; 恢复: 消息读取失败也重置 status; retry → 新 sibling turn; regenerate → 新 sibling turn; branch → 新 session 带 lineage + 消息拷贝 |
| `stream-watchdog.test.ts` | 6 | connect 超时 → fire; activity 切换 idle timeout + reset 时钟; pause 抑制 timeout; 嵌套 pause → 需要匹配 resume 才 restart; stop 取消 timer |
| `session-startup-recovery-contract.test.ts` | 3 | runtime 暴露 recoverInterruptedSessions; desktop 在 createWindow 前运行恢复; UI 只在 genuinely running turn 上显示 in-progress |
| `session-message-lifecycle-contract.test.ts` | 1 (多断言) | 选择 session 时先清空再读取; late read 仅当 still active 时才 setMessages; 读失败用 generalized copy; catch 不做第二次清空; retry 用 ref-backed guard 防重入 |
| `session-status-presentation.test.ts` | ~20 | 所有 8 种 status 有中文 label + tone; blockedReason 永不泄露 enum 值; NO_REAL_CONNECTION → 中文文案; describeTurnErrorClass 覆盖 timeout/auth/rate_limit/network/tool_failed/provider_unavailable/app_restarted; deriveFailedTurnRecovery 四种 action 路径 |
| `ai-sdk-backend.test.ts` | 新增 846+ 行 | 模型解析、流事件标准化、工具执行、权限门控、子代理限制、权限超时 (推测) |

### 测试缺口 (Gaps)

| 优先级 | 缺口 | 说明 |
|-------|------|------|
| P0 | **无 session-manager 并发压力测试** | 现有测试用 `Gate` 模拟并发，但未测试同 session 上 3+ 个并发 turn 同时 abort 的场景。`activeStreams` 和 `stoppedTurnIds` 在极端并发下的正确性没有形式化验证。 |
| P0 | **无 `sendMessage()` 在 `finally` 块中 header 更新失败的端到端测试** | `finally` 块中的 `updateHeader` 用了 `.catch(()=>{})` swallow (`session-manager.ts:409-418`)，意味着如果 header 更新失败，session 状态实际停留在 `running` 但 UI 显示 `active`。缺少测试验证这种不一致的 detectability。 |
| P1 | **`recoverInterruptedSessions()` 的性能边界** | 当有 1000+ 个 session 时，启动时逐个 `readMessages` 可能显著延长启动时间。无性能测试或批量优化。 |
| P1 | **`expireRequest()` 的集成测试缺失** | `permission-engine.test.ts` 只有 25 行 diff，缺少对 `expireRequest()` 在 `ToolRuntime` 中的端到端测试 (权限超时 → expire → backend 继续执行)。 |
| P1 | **`turnHasRetainedOutput()` 仅在 `appendTurnState` 时计算** | 如果 assistant message 是在 `appendTurnState` 之后才写入的 (backend 侧写 JSONL 的时机不确定)，`partialOutputRetained` 可能为 false。这个时间窗口没有被测试覆盖。 |
| P2 | **`copyMessagesThroughTurnBoundary()` 的 branch 行为** | 只过滤掉 `turn_state` 消息，但 `system_note` (如 `abort`) 也会被拷贝过去。这可能在新 session 中产生误导性的审计注释。无测试覆盖。 |
| P2 | **`StreamWatchdog.pauseCount` reset 语义** | `start()` 重置 `pauseCount = 0`，但如果旧 watchdog 在被 stop 后 re-start，pauseCount 的嵌套引用关系丢失。当前测试未覆盖 re-start 场景。 |

---

## risks

### P0: 已缓解
- ~~`paused: boolean` 覆盖问题~~ — **已修复**。原先多个工具同时等待 permission 时，第一个 resume 就重启 idle timer (`05ca5a3` 代码使用 `paused: boolean`)。`4dd1bf1` 改为 `pauseCount: number` 计数器 (`stream-watchdog.ts:40,71-82`)，嵌套 pause → 需要等量 resume 才重启计时。
- ~~权限永久挂起~~ — **已修复**。新增 `DEFAULT_PERMISSION_TIMEOUT_MS = 300_000` (`tool-runtime.ts:64`) + `PermissionEngine.expireRequest()` (`permission-engine.ts:224-233`)。5 分钟后自动失败挂起的权限请求，不再需要用户手动取消。

### P0: 仍存在
- **`activeStreams` 递减不精确** — `session-manager.ts:396` 使用 `Math.max(0, active.activeStreams - 1)`，这是一个防御性写法，但如果计数逻辑本身有 bug (如异常路径未递减)，防御性 max 会掩盖问题而非暴露它。
- **`connectionLocked` 锁后无法回退** — 一旦 set 为 true (`session-manager.ts:331-333`)，永不回退。如果用户在 connectionLocked 后切换 backend kind，旧 lock 仍阻止 session 与新 backend 绑定。虽然 `disposeBackend` 会销毁 backend，但 `connectionLocked` 字段本身不清除。

### P1: 形态变换
- **`AiSdkBackend` 从单体转为多类** — `05ca5a3` 的单体 750 行 `AiSdkBackend` 在 `4dd1bf1` 拆分为 `AiSdkBackend` + `ModelAdapter` (272 行) + `ToolRuntime` (608 行) + `RunTrace` (152 行)。这改善了可测试性，但引入了新的类间耦合点: `ToolRuntime` 依赖 `PermissionEngine` + `StreamWatchdog` (通过 `getPermissionPauseTarget`) + `RunTraceLike`; `ModelAdapter` 需要 `repairToolCall` 回调。如果这些依赖的初始化顺序不当，可能产生静默的 null-check bug。
- **`expireRequest()` 的单点故障** — 如果 `ToolRuntime` 的权限超时 timer 和 `PermissionEngine.expireRequest()` 之间的时序错位 (如 timer fire 时 permission 已经被用户手动响应)，`expireRequest()` 返回 null 但 timer 侧可能已经写了 error log。需要确认 `ToolRuntime` 在 `expireRequest` 返回 null 时的行为。

### P2: 架构层面
- **`SessionManager` 是全局单点** — 一个 `SessionManager` 实例管理所有 session 的 `ActiveSession` Map。如果 `disposeBackend()` 的 `catch` 块 swallow 了重要错误 (`session-manager.ts:569`)，内存泄漏 (backend 未正确释放) 无法被检测。
- **`finally` 块的 header 更新是不可靠的** — `updateHeader` 在 `finally` 中被 `.catch(()=>{})` swallow (`session-manager.ts:415-418`)，意味着持久化失败不会抛出也不会被上层感知。在高负载下，这可能导致 session status 在磁盘上与实际不一致。

---

## next_actions

以下动作为可拆分为 Rive DAG 节点的工程任务:

### 节点 1: P0 并发压力测试 (节点 ID 建议: `test-concurrent-abort`)
- 在 `session-manager.test.ts` 中添加测试: 同 session 上 3 个并发 turn 同时被 abort，验证:
  - 所有 3 个 turn 的 `turn_state.status` 最终为 `aborted`
  - `activeStreams` 最终为 0
  - session status 最终为 `aborted`
  - 每个 turn 的 `abortSource` 被正确记录
- 产出: 测试增量 + 可能的 `activeStreams` 修复

### 节点 2: P0 `finally` 异常检测 (节点 ID 建议: `surfce-header-update-failure`)
- 在 `finally` 块的 `updateHeader` catch 中添加 console.error 或 telemetry hook
- 添加测试: 模拟 `updateHeader` 在 `finally` 中失败，验证至少有一次 error 级别的日志输出
- 评估是否需要在 UI 层显示"状态同步失败"的指示器

### 节点 3: P1 恢复性能优化 (节点 ID 建议: `batch-recovery-optimize`)
- 评估 `store.list()` → 逐个 `readMessages()` 在 1000+ session 场景下的启动时间
- 考虑批量读取方案: 在 `SessionStore` 接口中添加 `readLatestTurnStates(sessionIds: string[]) → Map<string, TurnStateMessage>` 
- 产出: 性能分析报告 + 可能的接口扩展

### 节点 4: P1 `expireRequest` 集成测试 (节点 ID 建议: `test-permission-timeout-e2e`)
- 在 `ai-sdk-backend.test.ts` 或新测试文件中添加:
  - 发起 permission_request → 等待 `permissionTimeoutMs` → 验证 expireRequest 被调用 → 验证 backend 继续执行 (不卡住)
  - 用户手动响应早于超时 → 验证 expireRequest 返回 null 被安全处理
- 产出: 测试增量

### 节点 5: P1 `partialOutputRetained` 时序修正 (节点 ID 建议: `fix-partial-output-timing`)
- 分析 `turnHasRetainedOutput()` 的当前调用时机 (`appendTurnState` 内部)
- 确认 assistant message 的 JSONL 写入是否总是早于 `appendTurnState`
- 如果存在时序窗口，将 `partialOutputRetained` 的计算推迟到所有消息写入之后

### 节点 6: P2 branch 消息拷贝审计 (节点 ID 建议: `audit-branch-copy`)
- 检查 `copyMessagesThroughTurnBoundary()` 拷贝 `system_note` (如 abort/mode_change) 到子 session 是否合理
- 决定是否需要在 branch 时过滤掉某些 `system_note` 类型
- 产出: 决策文档 + 可能的过滤逻辑调整
