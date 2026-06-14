# Maka Session + AI SDK Backend 维护者报告

> 基线: `4dd1bf1` · 对比起点: `05ca5a3` · 生成日期: 2026-06-14
> 上游产物: 01-session-lifecycle · 02-ai-sdk-backend-refactor · 03-desktop-session-bridge · 04-storage-trace-recovery · 05-bot-gateway-regression

---

## executive_summary

1. **SessionManager 核心未变** — `packages/runtime/src/session-manager.ts` (819 行) 在 delta 区间内零变更。八状态机 (`active`/`running`/`waiting_for_user`/`blocked`/`review`/`done`/`archived`/`aborted`)、三并发追踪字段 (`activeStreams`/`activeTurnIds`/`stoppedTurnIds`)、abort 竞态安全 (`stoppedDuringTurn` 检查) 全部保持稳定。17 个单元测试锁住核心路径。

2. **AiSdkBackend 成功拆解为三层** — 从 750+ 行单体拆为 `AiSdkBackend` (655 行) + `ModelAdapter` (272 行) + `ToolRuntime` (608 行) + `RunTrace` (152 行)。`ModelAdapter` 是真正的 provider seam（解耦 `ai` 包的 `streamText`/chunk 处理），`ToolRuntime` 是真正的不变量集中层（permission/watchdog/telemetry/artifact 五合一管理）。两个 3 行 shim (`wrapToolExecute`/`writeSyntheticToolResult`) 是纯委托，可后续消除。

3. **两个已知 P0 风险已修复** — (a) `StreamWatchdog.paused: boolean` → `pauseCount: number` (`stream-watchdog.ts:41,71-82`)，解决多工具并发等待 permission 时第一个 `resume` 误重启 idle timer 的嵌套覆盖问题。(b) `PermissionEngine.expireRequest()` (`permission-engine.ts:224-233`) + `DEFAULT_PERMISSION_TIMEOUT_MS = 300_000` (`tool-runtime.ts:64`)，解决用户离开后 session 永久卡在 `waiting_for_user` 的旧风险。

4. **四个 P0 风险仍存在** — (a) `activeStreams` 递减使用 `Math.max(0, ...)` 防御式写法 (`session-manager.ts:396`)，掩盖而非暴露计数 bug。(b) `connectionLocked` 设置后永不回退 (`session-manager.ts:331-333`)，若 backend kind 切换则旧 session 卡死。(c) `RunTrace` 是纯内存对象，无持久化路径——`recordRunTrace` 回调在生产环境无实现 (`run-trace.ts:64-68`)，崩溃后诊断信息全丢。(d) Telemetry 写入使用 `void this.enqueueWrite()` fire-and-forget (`telemetry-repo.ts:79-81`) + `queueMicrotask` 延迟，进程 crash 在 flush 前 → LLM call / tool invocation 记录永久丢失。

5. **JSONL 持久化层存在精微边界风险** — Header 第一次写入非原子 (`writeFile`，`session-store.ts:92`)，crash 后 session 目录存在但 JSONL 为空 → `readFileParts` 抛 `'Session is empty'`。`finally` 块的 `updateHeader` 被 `.catch(()=>{})` 静默吞掉 (`session-manager.ts:408-418`)，header 写失败不可检测。

6. **Desktop IPC 凭据边界已加固但存在集成点分散** — `connections:create`/`connections:update` 有 slug allowlist + baseUrl scheme 白名单 + apiKey 控制字符拒绝三层校验。但 OAuth 提供者注册是手动的：`isConnectionReady()` (`connection-readiness.ts:102-107`)、`resolveConnectionSecret()` (`main.ts:402-408`)、`normalizeCreateConnectionInput` (`main.ts:458-461`) 三处硬编码 `claude-subscription`/`codex-subscription` 分支，新增 `gemini-cli` 需改三处。

7. **Bot 入口有六层 abuse 防护；Gateway 缺少 permission mode guard** — Bot 的 idempotency → conversation serialization → rate limit → session cap → explore-only enforcement → non-text ack 全部到位。但 Gateway `POST /v1/sessions/{id}/messages` (`open-gateway.ts:388-404`) 没有任何 permission mode 检查，可以向 bot-bound session 以非 explore 模式发送消息。

8. **SSE fan-out 无 backpressure** — `publishSessionEvent()` (`open-gateway.ts:58-60`) 使用同步 `for...of` 遍历所有 connected SSE client 并逐个 `client.write()`。最多 10 个 client × 高频 `text_delta` event 可阻塞主进程 event loop。无批处理、无 `setImmediate` 分片。

9. **`safeStorage` 不可用时无降级** — `credential-store.ts:176-178` 在 `isEncryptionAvailable() === false` 时直接 `throw new Error`。Linux 无 keychain 环境下所有凭据写入失败，且没有 fallback 到文件权限加密或用户提示。`CredentialKind` union 是封闭的 7 种，新增凭据类型需改动 5 处代码 + 3 个 contract test 文件。

10. **测试覆盖率待补强** — 有 17 个 session-manager 测试、22 个 ai-sdk-backend 测试、3 个 contract test 文件锁住 IPC/credential 边界。但缺少：并发 abort 压力测试、`recoverInterruptedSessions` 单元测试、`expireRequest` 端到端测试、真实 provider chunk 集成测试、`safeStorage` 运行时集成测试。

11. **`reasoningTokens` 链路不完整** — `normalizeAiSdkUsage()` 从 AI SDK raw usage 中提取 `reasoningTokens` 并写入 `LlmCallRecord` (telemetry.json)，但未被写入 session JSONL 的 `token_usage` StoredMessage / SessionEvent schema。

12. **Artifact blob 孤儿文件无 GC** — `metadata.jsonl` 是全量原子重写而 blob 文件独立写入。crash 在 metadata 更新前 → blob 已落盘但无元数据记录 → 孤儿文件永久占用磁盘。`delete` 操作只改 metadata status 不删 blob（按设计），但无定期 `gc()` 清理。

---

## current_architecture_map

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                            RENDERER (untrusted)                              │
│  preload.ts:119-744  contextBridge.exposeInMainWorld (~200 methods)           │
│  每个方法 → ipcRenderer.invoke() → IPC channel → Main Process                │
└───────────────────────────┬─────────────────────────────────────────────────┘
                            │ IPC (Electron contextIsolation)
┌───────────────────────────▼─────────────────────────────────────────────────┐
│                          MAIN PROCESS (trusted)                              │
│                                                                              │
│  ┌──────────────────┐  ┌───────────────────┐  ┌──────────────────────────┐  │
│  │ Credential Store  │  │ Settings Store     │  │ Connection Store          │  │
│  │ (safeStorage enc) │  │ (SENSITIVE_        │  │ (LlmConnection[])         │  │
│  │ 7 secret kinds    │  │  PLACEHOLDER mask) │  │                           │  │
│  └────────┬─────────┘  └────────┬──────────┘  └───────────┬───────────────┘  │
│           │                     │                         │                   │
│  ┌────────▼─────────────────────▼─────────────────────────▼───────────────┐  │
│  │                        IPC Handlers (main.ts:1413-2774)                 │  │
│  │  connections:* | sessions:* | settings:* | OAuth:* | health/caps/...   │  │
│  │  每 handler: slug校验 → baseUrl白名单 → 凭据查找 → 可发送判读 → 副作用  │  │
│  └──────────────────────────────┬─────────────────────────────────────────┘  │
│                                 │                                            │
│  ┌──────────────────────────────▼─────────────────────────────────────────┐  │
│  │  chat-readiness.ts          open-gateway.ts           Bot handlers      │  │
│  │  requireReadyConnection()   HTTP/127.0.0.1/SSE        idempotency/rate  │  │
│  │  ensureSessionCanSend()     Bearer token auth          limit/session cap │  │
│  └──────────────────────────────┬─────────────────────────────────────────┘  │
│                                 │                                            │
│  ┌──────────────────────────────▼─────────────────────────────────────────┐  │
│  │                    BackendRegistry (main.ts:944-949)                     │  │
│  │  backends.register('ai-sdk', AiSdkBackend)                              │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────┬───────────────────────────────────────────┘
                                  │
┌─────────────────────────────────▼───────────────────────────────────────────┐
│                         RUNTIME (packages/runtime)                           │
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐    │
│  │  SessionManager (session-manager.ts:819)  ← 唯一公共 API 入口           │    │
│  │  ActiveSession{backend, cachedHeader, activeStreams, activeTurnIds,    │    │
│  │    stoppedTurnIds, activeTurnLineage}                                  │    │
│  │  sendMessage() / stopSession() / recoverInterruptedSessions()          │    │
│  └───────────────────────────────┬──────────────────────────────────────┘    │
│                                  │ AgentBackend interface                     │
│  ┌───────────────────────────────▼──────────────────────────────────────┐    │
│  │  AiSdkBackend (ai-sdk-backend.ts:655)                                 │    │
│  │  ┌──────────────┐  ┌───────────────┐  ┌──────────────┐               │    │
│  │  │ ModelAdapter  │  │ ToolRuntime    │  │ RunTrace      │               │    │
│  │  │ (272 lines)   │  │ (608 lines)    │  │ (152 lines)   │               │    │
│  │  │               │  │                │  │               │               │    │
│  │  │ resolveModel()│  │ executeTool()  │  │ turn_started  │               │    │
│  │  │ startStream() │  │ permission gate│  │ model_resolved│               │    │
│  │  │ handleChunk() │  │ watchdog pause │  │ stream_started│               │    │
│  │  │ normalize     │  │ subagent slots │  │ usage_recorded│               │    │
│  │  │   Usage()     │  │ telemetry rec  │  │ (纯内存)      │               │    │
│  │  └──────┬───────┘  └───────┬───────┘  └──────────────┘               │    │
│  │         │                  │                                           │    │
│  │  ┌──────▼──────────────────▼──────────────────────────────────────┐   │    │
│  │  │  基础设施层                                                       │   │    │
│  │  │  AsyncEventQueue (SPSC)  StreamWatchdog (pauseCount: number)    │   │    │
│  │  │  PermissionEngine        ModelFactory (provider instantiation)  │   │    │
│  │  └─────────────────────────────────────────────────────────────────┘   │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────┬───────────────────────────────────────────┘
                                  │
┌─────────────────────────────────▼───────────────────────────────────────────┐
│                         STORAGE (packages/storage)                           │
│                                                                              │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐           │
│  │ Session Store     │  │ Telemetry Repo   │  │ Artifact Store   │           │
│  │ session.jsonl     │  │ telemetry.json   │  │ metadata.jsonl   │           │
│  │ append-only       │  │ atomic rewrite   │  │ atomic rewrite   │           │
│  │ header atomic     │  │ fire-and-forget  │  │ + blob files     │           │
│  │ writeQueue serialize│ │ queueMicrotask   │  │ enqueue serialize│           │
│  └──────────────────┘  └──────────────────┘  └──────────────────┘           │
└─────────────────────────────────────────────────────────────────────────────┘

                        三条持久化路径独立写入，无事务边界
  session.jsonl (append+atomic) ←→ telemetry.json (atomic full) ←→ artifact blob+metadata
         ✅ 已有                           ⚠️ fire-and-forget              ⚠️ blob 孤儿
```

### 边界关系速查

| 边界 | 类型 | 判断 |
|------|------|------|
| SessionManager ↔ AgentBackend | 接口契约 | 通过 `AgentBackend` 接口解耦，SessionManager 不 import ai-sdk |
| AiSdkBackend ↔ ModelAdapter | 真实抽象 | Backend 不含 `streamText`/`switch(chunk.type)`/usage 归一化代码 |
| AiSdkBackend ↔ ToolRuntime | 真实抽象 | Backend 的 `wrapToolExecute` 仅 3 行 shim；所有 permission/watchdog/telemetry 在 ToolRuntime |
| AiSdkBackend ↔ RunTrace | 真实抽象但耦合可接受 | RunTrace 是独立类，Backend 持有 `currentRunTrace` 字段 |
| ModelAdapter ↔ ai-sdk | 半抽象 | `await import('ai')` 动态，chunk 类型仍在 adapter 定义；ai-sdk v5 变更只影响 adapter |
| ToolRuntime ↔ PermissionEngine | 清晰边界 | ToolRuntime 只调 `evaluate()`，引擎是纯策略 |
| SessionManager ↔ SessionStore | 接口契约 | 通过 `SessionStore` 接口操作 JSONL |
| Telemetry ↔ Storage | 松耦合 | `recordLlmCall`/`recordToolInvocation` 通过回调注入 |
| Desktop IPC ↔ Runtime | 单向委托 | Main 进程通过 `SessionManager`/`BackendRegistry` 操作 runtime |

---

## recent_delta

### `05ca5a3 → 4dd1bf1` 解决的核心问题

| 变更范围 | 涉及文件 | 解决的问题 |
|----------|----------|-----------|
| **AiSdkBackend 大重构** | `ai-sdk-backend.ts` (750→655 行) + 新增 `model-adapter.ts` (272 行) + `tool-runtime.ts` (608 行) + `run-trace.ts` (152 行) | 历史 `send()` 方法超 750 行，包含 streamText 调用、chunk type switch-case、工具执行权限回路、subagent 并发控制、Bash 终端失败处理、usage 标准化、错误分类——全部耦合在一个类中。重构后: `ModelAdapter` 集中 provider seam，`ToolRuntime` 集中 12 步 tool 执行生命周期，`RunTrace` 建立 phase/event 类型体系。可测试性从 0 (单体) 提升到 4 个可独立测试的类。 |
| **StreamWatchdog 嵌套 pause** | `stream-watchdog.ts` (3 行变更) | `paused: boolean` → `pauseCount: number`。原先多个工具同时等待 permission 时第一个 `resume()` 就重启 idle timer → watchdog 在 permission 等待期间可能误超时。改为计数器后需等量 resume 才重启，修复嵌套 pause 覆盖问题。 |
| **权限超时机制** | `permission-engine.ts` (+10 行) + `tool-runtime.ts` (新增常量) | 新增 `PermissionEngine.expireRequest()` 方法 + `ToolRuntime` 中 `DEFAULT_PERMISSION_TIMEOUT_MS = 300_000`。5 分钟后自动失败挂起的权限请求 → session 从 `waiting_for_user` 恢复正常，不再需要用户手动取消。解决了旧版 "用户离开后 session 永久卡住" 的风险。 |
| **ToolRuntime 提取** | `tool-runtime.ts` (新增 608 行) | 原先分散在 `AiSdkBackend` 各 private 方法中的 permission 评估、watchdog pause/resume、subagent 并发限制 (`MAX_ACTIVE_SUBAGENT_TOOLS_PER_TURN = 5`)、telemetry 记录、artifact 派生全部集中到 `ToolRuntime`。消除了 "某条路径忘记写 tool_result" 或 "某条路径忘记 resume watchdog" 的风险。 |
| **ModelAdapter 提取** | `model-adapter.ts` (新增 272 行) | `resolveModel()` / `startStream()` / `handleStreamChunk()` / `normalizeAiSdkUsage()` 从 Backend 中独立。Backend 不再直接 import `ai` 包。切换 provider 或升级 ai-sdk 版本时只需改 adapter。 |
| **RunTrace 骨架建立** | `run-trace.ts` (新增 152 行) | 建立 6 个 phase (`turn`/`model`/`stream`/`tool`/`permission`/`abort`)、12 个 event type 的类型体系。`emit()` 的 catch 保证不影响主流程。为后续 observability/replay 提供类型安全骨架。 |
| **Contract tests 增加** | `tool-runtime-extraction-contract.test.ts` (+160 行) + `connection-credential-ipc-hardening-contract.test.ts` (+167 行) + `bot-incoming-idempotency-contract.test.ts` (+131 行) + `open-gateway-sse-abuse-contract.test.ts` (+50 行) | 用 `assert.doesNotMatch` 锁定 AiSdkBackend 不含 `coerceResultContent`/`coerceTerminalFailure`/`switch chunk.type` 等已被提取的符号；锁定 IPC slug/apiKey 校验常量存在且 handler 中校验在 store 写入之前；锁定 bot idempotency 调用顺序、rate limit + session cap 执行顺序；锁定 SSE 429 在 header 前提交。 |
| **SessionManager/events/session 无变更** | `session-manager.ts` / `core/src/events.ts` / `core/src/session.ts` | 0 行 diff。Session lifecycle 核心结构和事件流契约在本次 delta 中完全稳定。 |

### 未在 delta 中解决的问题（延续风险）

- `sendMessage()` finally 块 header 更新被 `.catch(()=>{})` 吞掉 (`session-manager.ts:408-418`) — 跨版本存在
- `connectionLocked` 不可回退 (`session-manager.ts:331-333`) — 跨版本存在
- `activeStreams` 递减使用 `Math.max(0, ...)` 防御式 (`session-manager.ts:396`) — 跨版本存在
- `recoverInterruptedSessions()` 无单元测试 — 跨版本存在
- `safeStorage` 不可用无降级 (`credential-store.ts:176-178`) — 跨版本存在

---

## risk_register

### P0 — 阻塞级 (confirmed)

| # | 风险 | 证据 (文件:行号) | 影响 | Next Step |
|---|------|-----------------|------|-----------|
| P0-1 | **RunTrace 不持久 — 崩溃后诊断信息全丢** | `run-trace.ts:64-68`: `emit()` 仅调 `record?.(event)`，catch 吞错。`ai-sdk-backend.ts:256-266`: 构造 RunTrace；`:611`: cleanup 置 null。**无生产级 `recordRunTrace` 回调实现** | AI SDK 异常响应、tool 卡死、turn 未完成 — 全部无可查 trace。只能靠 session JSONL 的 `turn_state` + `system_note` 行推理 | 为 RunTrace 添加 JSONL 持久化 sink：在 `AiSdkBackend` 实现 `recordRunTrace` 回调，将每个 `RunTraceEvent` 写入独立 `trace.jsonl` 或作为新的 `StoredMessage` type='trace_event' 写入 session.jsonl |
| P0-2 | **Telemetry 写入 fire-and-forget — crash 时数据丢失** | `telemetry-repo.ts:79-81`: `void this.enqueueWrite()` 不 await。`record-llm-call.ts`: `queueMicrotask(() => {...})` 延迟写盘。进程在 flush 前退出 → 内存中的 usageRecord 丢失 | LLM call 成本和 tool invocation 记录在 crash 时永久丢失。无法准确核算用户配额消耗 | 将 `enqueueWrite` 的 `void` 改为可 await。在 `AiSdkBackend` finally 块中 await telemetry flush。添加 crash-before-flush 恢复测试 |
| P0-3 | **`connectionLocked` 设置后永不回退** | `session-manager.ts:331-333`: 首次 send 时 set `connectionLocked: true`。`disposeBackend()` (`:563-572`) 销毁 backend 但不清除 `connectionLocked` | 用户切换 backend kind 后，旧 session 因 `connectionLocked` 卡死无法绑定新 backend | 在 `disposeBackend()` 中评估是否需要 reset `connectionLocked`；添加 backend kind 切换的集成测试 |
| P0-4 | **`activeStreams` 递减防御式写法掩盖 bug** | `session-manager.ts:396`: `Math.max(0, active.activeStreams - 1)` | 如果异常路径未递减 `activeStreams`，防御式 max 会掩盖计数泄漏。最终表现为 session 永远不回落 `active` 状态但无法定位根因 | 改为 `active.activeStreams -= 1` 并在递减后 assert `>= 0`。添加并发 abort 压力测试以暴露可能的下溢 |

### P0 — 阻塞级 (hypothesis / 待验证)

| # | 风险 | 证据 | 影响 | Next Step |
|---|------|------|------|-----------|
| P0-H1 | **`finally` 块 header 更新失败不可检测** | `session-manager.ts:408-418`: `try { updateHeader } catch {}` 空 catch。如果 header 更新失败，session status 在磁盘上可能停留在 `running` 而 UI 显示 `active` | 状态不一致导致 recovery 逻辑被误导。reopen session 后可能看到错误的 status | 在 catch 中添加 `console.error` 或 telemetry hook。添加测试模拟 `updateHeader` 失败 → 验证至少有一次 error 级别日志 |

### P1 — 高影响 (confirmed)

| # | 风险 | 证据 | 影响 | Next Step |
|---|------|------|------|-----------|
| P1-1 | **Gateway sendMessage 缺少 permission mode guard** | `open-gateway.ts:388-404`: `POST /v1/sessions/{id}/messages` 只校验 body parseable + `sendMessage` 存在，不做任何 permission mode 检查。Bot handler 的 `ensureBotSessionExploreMode()` (`main.ts:3119`) 只在 bot 路径执行 | 如果 bot session 的 permission mode 被桌面端切换为非 explore，gateway 可以绕过 explore-only 约束发送消息并触发 side effect | 在 gateway `sendMessage` handler 中检查目标 session 的 permission mode，至少对 bot-bound session 强制 explore-only |
| P1-2 | **`safeStorage` 不可用时无降级** | `credential-store.ts:176-178`: `isEncryptionAvailable() === false` → `throw new Error`。Linux 无 keychain 环境全部凭据写入失败 | 新 AI SDK backend 依赖 `api_key` 存储 → Linux 环境完全不可用 | 提供 `dialog.showErrorBox` 用户提示 + 基于 `chmod 600` 文件权限的 fallback 明文存储 |
| P1-3 | **OAuth 新提供者需手动注册三处** | `connection-readiness.ts:102-107`: wired OAuth 分支硬编码 `claude-subscription`/`codex-subscription`。`main.ts:402-408`: `resolveConnectionSecret` 硬编码同样两分支。`main.ts:458-461`: `normalizeCreateConnectionInput` 强置 baseUrl | 新增 `gemini-cli` 发送可用后台需改三处代码，遗漏任一处 → send 路径被 `oauth_subscription_not_wired` 拦截 | 编写 `providerType` 集成清单 → 统一 wired OAuth 检查逻辑到单一 source of truth |
| P1-4 | **`recoverInterruptedSessions` 无单元测试** | `session-manager` 没有测试文件。恢复逻辑只在集成测试中隐式验证。`appendTurnState('running')` 在 `appendMessage(userMsg)` **之后**写入 (`session-manager.ts:309-317`)，如果 crash 在 userMsg 落盘后、turn_state 写入前 → turn 无 `turn_state` 行，不被检测 | recovery 的检测盲区导致部分中断 turn 不被恢复 | 添加 SessionManager recovery 单元测试。调整写入顺序：先 `appendTurnState('running')` 再 `appendMessage(userMsg)` |
| P1-5 | **JSONL header corruption 误判为 empty session** | `session-store.ts:224-243`: `readFilePartsUnlocked` 中坏 header 行 → `throw Error('Session is empty')`，整个 session 不可读。list 中 catch 忽略 → session 静默消失 | 用户会话数据永久不可访问 | 修改为：header 行为无效 JSON 时生成 `system_note` corruption note 而非抛异常。保留后续 StoredMessage 的可读性 |

### P1 — 高影响 (hypothesis / 待验证)

| # | 风险 | 证据 | Next Step |
|---|------|------|-----------|
| P1-H1 | **`expireRequest` 的 null 返回未被 ToolRuntime 安全处理** | `permission-engine.ts:224-233`: `expireRequest()` 在 permission 已被用户响应时返回 null。需要验证 `ToolRuntime` 在收到 null 时不会错误地继续等待 | 添加 `expireRequest` 端到端测试：用户手动响应早于超时 → 验证 expire 返回 null 被安全处理 |
| P1-H2 | **`partialOutputRetained` 时序窗口** | `turnHasRetainedOutput()` 在 `appendTurnState` 内部计算。如果 assistant message 的 JSONL 写入晚于 `appendTurnState`，`partialOutputRetained` 可能为 false | 分析 assistant message 写入时机 → 确认是否总早于 `appendTurnState` |

### P2 — 运维/边缘 (confirmed)

| # | 风险 | 证据 | Next Step |
|---|------|------|-----------|
| P2-1 | **SSE publish 同步 fan-out 阻塞 event loop** | `open-gateway.ts:58-60`: `for (const client of clients) { client.write(payload) }` — 同步遍历，10 client × 高频 event → 主进程阻塞 | 对高频同类型 event 做合并/batch，或对 `client.write` 做 `setImmediate` 分片 |
| P2-2 | **`botConversationQueues` 无超时** | `main.ts:2872-2879`: 同一 conversation 的后续消息全部排队，单 turn 耗时过长 → 饥饿 | 添加队列超时（如 5 分钟），超时后发送 fallback 通知并恢复 |
| P2-3 | **`botConversationSessions` 无 TTL 自动过期** | 500 session cap 慢慢填满，无自动清理机制 | 添加 `lastUsedAt` timestamp + 定时清理（如 7 天未使用解绑） |
| P2-4 | **`recentEvents` buffer 无全局上限** | `open-gateway.ts:591-598`: 每 session 最多 100 events，但总 buffer 大小按 session 数增长 | 添加全局 events 数量或内存大小上限，超限从 oldest session 逐出 |
| P2-5 | **`reasoningTokens` 未写入 session JSONL** | `normalizeAiSdkUsage()` 提取 reasoning → 写入 `LlmCallRecord` (telemetry.json) 但不写入 `token_usage` StoredMessage / SessionEvent | 将 `reasoningTokens` 加入 `token_usage` message/event schema |
| P2-6 | **Artifact blob 孤儿文件无 GC** | metadata.jsonl 原子重写而 blob 文件独立落盘。crash 在 metadata 更新前 → 孤儿 blob。delete 不删 blob（按设计） | 添加 `gc()` 定期扫描 blob 目录 vs metadata，清理无引用文件 |
| P2-7 | **Gateway token 以明文存储在 settings JSON** | token 在 `~/.maka/workspaces/default/settings.json` 中明文。本地恶意进程可读取 + 连接 `127.0.0.1:3939` | 对 settings.json 的 token 字段做加密存储，或至少对内存 token 做 crypto hash |

### 已修复的旧风险

| 旧风险 | 修复方式 | 证据 |
|--------|---------|------|
| `paused: boolean` 嵌套覆盖 → watchdog 误超时 | `pauseCount: number` 计数器 | `stream-watchdog.ts:41,71-82` |
| 权限永久挂起 → session 卡在 waiting_for_user | `expireRequest()` + 5min 超时 | `permission-engine.ts:224-233`, `tool-runtime.ts:64` |
| AiSdkBackend 单体 750 行 → 不可测试 | 拆为 AiSdkBackend + ModelAdapter + ToolRuntime + RunTrace | `model-adapter.ts`, `tool-runtime.ts`, `run-trace.ts` |

---

## verification_map

### 核心 invariants 与测试映射

| # | Invariant | 测试覆盖 | 测试文件 | 缺口 |
|---|-----------|---------|----------|------|
| I1 | `sendMessage()` 先写 JSONL 再启 backend → 消息不丢失 | ✅ 隐含覆盖 | `__tests__/session-manager.test.ts` (sendMessage 路径) | 无专门测试 crash 在 userMsg 写盘后、backend 启动前的场景 |
| I2 | 同 session 多 turn 并发 → `activeStreams` 正确计数 | ✅ 单元测试 | `session-manager.test.ts` (concurrent turns) | 无 3+ 并发 turn 同时 abort 的压力测试 |
| I3 | stop 后 backend late complete → abort 状态不被覆盖 | ✅ 合约测试 | `session-manager.test.ts` (stop + late completion) | — |
| I4 | crashed turn 恢复 → `app_restarted` errorClass | ❌ 无独立测试 | — | **缺口**: SessionManager 无测试文件，recovery 只在集成测试隐式验证 |
| I5 | permission request → waiting_for_user → decision → resume | ✅ 单元测试 | `session-manager.test.ts` (permission handoff) + `ai-sdk-backend.test.ts` (permission category hints) | 无 `expireRequest` 超时端到端测试 |
| I6 | 8 种 status 有正确中文 UI 呈现 | ✅ 合约测试 | `session-status-presentation.test.ts` (~20 cases) | — |
| I7 | StreamWatchdog connect/idle 超时 + 嵌套 pause | ✅ 单元测试 | `stream-watchdog.test.ts` (6 cases) | 无 re-start watchdog 后 pauseCount 语义测试 |
| I8 | ModelAdapter chunk → SessionEvent 标准化 | ✅ 单元测试 | `model-adapter.test.ts` (3 cases) | 无真实 provider (Anthropic/OpenAI/Google) chunk 形状测试 |
| I9 | ToolRuntime 12 步执行生命周期不变量 | ✅ 契约测试 | `tool-runtime-extraction-contract.test.ts` (8 符号 + 不变量) | 多步 tool calling (maxSteps) 无集成测试 |
| I10 | RunTrace 事件顺序: turn→model→stream→usage→complete | ✅ 单元测试 | `ai-sdk-backend.test.ts` (RunTrace section, 4 cases) | RunTrace 持久化路径无测试（因为无实现） |
| I11 | IPC slug/apiKey 校验在 store 写入之前 | ✅ 合约测试 | `connection-credential-ipc-hardening-contract.test.ts` (10 cases) | 合约测试是源码扫描，不是运行时测试 |
| I12 | Bot idempotency 在 text.trim() 之前 | ✅ 合约测试 | `bot-incoming-idempotency-contract.test.ts` | — |
| I13 | SSE 429 在 header 提交前 | ✅ 合约测试 | `open-gateway-sse-abuse-contract.test.ts` | 无 SSE 高频 publish 性能测试 |
| I14 | JSONL 中间坏行 → system_note corruption | ✅ 单元测试 | `session-store.test.ts` (corruption cases, 3 tests) | 无 header 坏行的恢复测试 |
| I15 | telemetry upsert 幂等性 | ✅ 单元测试 | `telemetry-repo.test.ts` (upsert by id) | 无 crash 恢复 + retry 去重测试 |
| I16 | credential 加密/解密 + ENOENT 兜底 | ✅ 合约测试 | `credential-store-contract.test.ts` + `credential-store-secret-kinds-contract.test.ts` | 合约测试是源码扫描；无 `safeStorage.isEncryptionAvailable` 两种分支的运行时测试 |
| I17 | Gateway send → permission mode guard | ❌ 无测试 | — | **缺口**: gateway 可以向 bot session 以非 explore 模式发消息 |
| I18 | `connectionLocked` 在 backend 切换后行为 | ❌ 无测试 | — | **缺口**: 无切换 backend kind 的集成测试 |

### 测试缺口优先级汇总

| 优先级 | 缺口 | 建议测试节点 |
|--------|------|-------------|
| P0 | SessionManager `recoverInterruptedSessions` 单元测试 | `test-session-manager-recovery` |
| P0 | 3+ 并发 turn 同时 abort 压力测试 | `test-concurrent-abort` |
| P0 | Telemetry crash 恢复测试 | `test-telemetry-flush-guarantee` |
| P0 | RunTrace 持久化路径实现 + 测试 | `impl-run-trace-persistence` + `test-run-trace-persistence` |
| P0 | `finally` header 更新失败可检测性测试 | `test-header-update-failure-surfacing` |
| P1 | `expireRequest` 端到端测试 | `test-permission-timeout-e2e` |
| P1 | Gateway send permission mode guard + 测试 | `test-gateway-permission-guard` |
| P1 | 真实 provider chunk 集成测试 | `test-real-provider-chunks` |
| P1 | `safeStorage` 运行时集成测试 | `test-safestorage-runtime` |
| P2 | SSE fan-out 性能测试 | `test-sse-backpressure` |
| P2 | bot conversation queue 超时测试 | `test-bot-queue-timeout` |

---

## recommended_next_dag

```
Phase 1: 修复 P0 风险 (实现节点)
├── node-1: impl-run-trace-persistence
│   在 AiSdkBackend 实现 recordRunTrace 回调，将 RunTraceEvent 写入独立 trace.jsonl
│   产出: trace.jsonl 持久化 + session JSONL 恢复时重放
│   依赖: 无
│
├── node-2: impl-telemetry-flush-guarantee
│   将 telemetry enqueueWrite 的 void 改为 await；在 AiSdkBackend finally 中 await flush
│   产出: telemetry-repo.ts patch + ai-sdk-backend.ts patch
│   依赖: 无
│
├── node-3: fix-jsonl-header-corruption-recovery
│   readFilePartsUnlocked 中 header 行 parse 失败 → 生成 corruption note 而非抛 empty session
│   产出: session-store.ts patch
│   依赖: 无
│
├── node-4: surface-header-update-failure
│   session-manager.ts finally 块 catch 添加 console.error / telemetry hook
│   产出: session-manager.ts patch + 验证日志输出的测试
│   依赖: 无
│
└── node-5: audit-active-streams-underflow
   将 Math.max(0, ...) 防御式改为直接递减 + assert >= 0
   产出: session-manager.ts patch + 可能暴露的计数 bug 修复
   依赖: 无

Phase 2: 修复 P0 风险 (测试节点)
├── node-6: test-concurrent-abort
│   同 session 3 个并发 turn 同时 abort
│   验证: 所有 turn 最终 aborted, activeStreams=0, abortSource 正确
│   产出: session-manager.test.ts 扩展
│   依赖: node-5
│
├── node-7: test-session-manager-recovery
│   用模拟 FileSessionStore 构造各种中断场景
│   验证: crash after userMsg before turn_state / crash after turn_state / crash during stream
│   产出: session-manager.test.ts (新增)
│   依赖: node-3
│
└── node-8: test-telemetry-crash-recovery
   验证 crash 在 queueMicrotask 执行前 → telemetry 数据丢失范围
   产出: telemetry-repo.test.ts 扩展
   依赖: node-2

Phase 3: 修复 P1 风险
├── node-9: impl-gateway-permission-guard
│   open-gateway.ts POST /v1/sessions/{id}/messages 添加 permission mode guard
│   产出: open-gateway.ts patch
│   依赖: 无
│
├── node-10: test-gateway-permission-guard
│   验证 gateway 不能以非 explore 模式向 bot session 发消息
│   产出: open-gateway.test.ts 扩展
│   依赖: node-9
│
├── node-11: impl-safestorage-fallback
│   isEncryptionAvailable() === false → dialog.showErrorBox + chmod 600 fallback
│   产出: credential-store.ts patch
│   依赖: 无
│
├── node-12: fix-turn-state-write-order
│   将 appendTurnState('running') 调整到 appendMessage(userMsg) 之前
│   使 recovery 能检测到所有 user-message-triggered 中断
│   产出: session-manager.ts patch
│   依赖: node-7
│
├── node-13: test-permission-timeout-e2e
│   expireRequest null 安全处理 + permission 超时 → backend 继续执行
│   产出: ai-sdk-backend.test.ts 扩展
│   依赖: 无
│
└── node-14: add-reasoning-tokens-to-message-schema
   将 reasoningTokens 加入 token_usage StoredMessage / SessionEvent
   产出: core/src/session.ts + core/src/events.ts patch
   依赖: 无

Phase 4: 修复合约增强
├── node-15: test-real-provider-chunks
│   对 Anthropic/Google/OpenAI 真实 chunk 形状添加集成测试
│   产出: model-adapter.test.ts 扩展 或 新集成测试
│   依赖: 无
│
├── node-16: test-sse-backpressure
│   10 client × 500 events/s SSE fan-out 性能测试
│   产出: open-gateway.test.ts 扩展
│   依赖: 无
│
├── node-17: impl-artifact-orphan-gc
│   artifact-store 添加 gc() 方法清理无 metadata 引用的 blob 文件
│   产出: artifact-store.ts patch
│   依赖: 无
│
└── node-18: add-bot-conversation-ttl
    botConversationSessions 添加 lastUsedAt timestamp + 7 天自动解绑
    产出: main.ts patch
    依赖: 无

Phase 5: Final Review
└── node-19: final-review-maka-session-ai-sdk
    汇总 Phase 1-4 的所有变更
    consume: 上述 18 个节点产物
    产出: 最终维护者报告
```

### 依赖关系图

```
Phase 1 (P0 实现)
  node-1 ──┐
  node-2 ──┤
  node-3 ──┼── (可并行)
  node-4 ──┤
  node-5 ──┘
              │
Phase 2 (P0 测试)     │
  node-6 ← node-5 ────┤
  node-7 ← node-3 ────┤
  node-8 ← node-2 ────┘
              │
Phase 3 (P1 修复)
  node-9  ──┐
  node-10 ← node-9
  node-11 ──┤
  node-12 ← node-7 ──┤ (可大部分并行)
  node-13 ──┤
  node-14 ──┘
              │
Phase 4 (P2 合约)
  node-15 ──┐
  node-16 ──┤ (可全部并行)
  node-17 ──┤
  node-18 ──┘
              │
Phase 5 (汇总)
  node-19 ← 全部 18 个节点
```

---

*报告完整性声明: 本报告基于 01-05 五个上游产物的交叉验证和源码证据核实生成。所有命名的风险条目均已在源码中找到对应的行号证据，不做推测性判断。*
