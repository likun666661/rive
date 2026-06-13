# Maka 架构总览

> 阅读基线: `335220a` | 深度档位: `architecture` | 语言: 中文
> 上游产物: 01-core-contracts / 02-runtime-backends-tools / 03-storage-persistence / 04-desktop-main-ipc / 05-renderer-ui / 06-docs-tests-roadmap

---

## 1. executive_summary

1. **Maka 是一个本地桌面 AI 编程工作台**，以 Electron 为壳，通过 Vercel AI SDK 统一对接 Anthropic、OpenAI、DeepSeek、Google Gemini、Ollama 等十余种 LLM provider，在用户本地文件系统上执行 Bash / Read / Write / Edit / Glob / Grep / RiveWorkflow / Office 文档读写等工具操作。

2. **五层架构**：`@maka/core`（共享类型契约，41 个 subpath export）→ `@maka/storage`（JSONL / settings / artifact 持久化）→ `@maka/runtime`（SessionManager、AiSdkBackend、PermissionEngine、builtin-tools、materializer、telemetry）→ `@maka/ui`（可复用组件 + stream helpers）→ `apps/desktop`（Electron main/preload/renderer 三层 IPC 桥接）。每一层有明确的 import 禁入规则。

3. **JSONL 是唯一真相来源（single source of truth）**：session header 作为 JSONL 第一行，后续消息 append-only 写入。`tool_call` 消息先于 permission 裁决写入 JSONL，确保进程崩溃后 materializer 可正确还原 orphan tool_call 为 `interrupted` 状态。UI 只是消费方，不持有 truth。

4. **安全边界是双重的**：编译期靠 `@maka/core` 的 `as const` 枚举 + `Result<T, E>` normalizer + disjoint union type（如 `MEMORY_SOURCES` vs `MEMORY_CANDIDATE_SOURCES`）；运行时靠 `credential-store.ts` 的 `safeStorage` 加密 + 文件路径 `realpath` + `isInside` containment + `redactSecrets` 多层脱敏。

5. **Permission 系统是 3 模式 × 11 类别的纯函数矩阵**：`preToolUse()`（core 层）无 I/O、无时钟、无 UUID——先 `categorizeBash` 分类，再查 `PERMISSION_POLICY` 矩阵，最后考虑 `turnRemembered` 白名单。`PermissionEngine`（runtime 层）管理 turn-scoped `parked` Promise 注册表，与 `StreamWatchdog.pause/resume` 联动。

6. **Renderer 是不可信客户端**：`sandbox: true`、`contextIsolation: true`、`nodeIntegration: false`。所有文件读写/模型调用/凭据存储/子进程 spawn 在 main 进程完成。renderer 通过 `window.maka.*` preload API（128 个 `ipcMain.handle` 通道）获取数据，且自己对 streaming text 做二次 `redactSecrets` + per-delta cap + per-session total cap。

7. **Bot 桥接打通外部 IM → agent 回路**：Telegram / Feishu / Discord / DingTalk / QQ / WeChat 六个平台的 bridge 在 main 进程运行，入站消息 → `SessionManager.sendMessage()` → `AiSdkBackend` → 回复 → 通过平台 API 发回。Bot 模式下无 permission UI，完全依赖 `explore` 或 `execute` 模式的自动决策。

8. **Visual Smoke 是 UI 回归测试的基础设施**：通过 `MAKA_VISUAL_SMOKE_FIXTURE` 注入预置 session/message/tool/theme 状态，冻结 `Date.now()`、暂停动画、锁定 locale，生成可 hash 对比的确定性截图。当前 32+ scenario × 8 variant = 256 PNG。

9. **最值得继续精读的模块**：① `AiSdkBackend.wrapToolExecute()` 中的 permission gating 异步竞态（parked/resume 路径）；② `credential-store.ts` 的 `safeStorage` 故障模式；③ 六个 `isInside` 实现的差异审计；④ `local-memory-service.ts` 的 9 条隐私门运行时执行；⑤ `main.ts` 128 个 IPC handler 的入参完整性校验。

10. **上游报告间的矛盾点**：① 01-core-contracts 标注 `workspace.ts` 无测试覆盖，但 04 确认 workspace instructions 有独立测试文件；② 03 指出 `settings.json` 含明文 bot token/代理密码，与 04 的 `credential-store` 加密体系形成安全缺口；③ 06 的 fixture scenario 数量（32+）与 `full-product-test-plan.md` 列出的（18）不一致；④ 05 断言"不用 JSDOM 做 React 渲染测试"与 05 自身列出的 20+ 个 pure-helper test 是同一策略，但缺少交互测试覆盖。

---

## 2. architecture_map

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         apps/desktop (Electron)                         │
│                                                                         │
│  ┌──────────────────────────────────────────────────────────────────┐   │
│  │  Renderer Process (sandbox: true, contextIsolation: true)         │   │
│  │                                                                    │   │
│  │  main.tsx (AppShell, ~3240 lines)                                 │   │
│  │    ├─ streamingBySession / thinkingBySession / liveToolsBySession │   │
│  │    ├─ permissionBySession / sessionEventHealthBySession           │   │
│  │    ├─ sessionStatusGroups / chatConnectionAlert                   │   │
│  │    ├─ handleEvent() — 唯一事件写入点                               │   │
│  │    │    text_delta → applyAssistantDelta (5-layer redact + cap)   │   │
│  │    │    tool_start / tool_output_delta / tool_result → upsertTool │   │
│  │    │    permission_request → setPermissionBySession               │   │
│  │    │    error/abort → clearStreaming + markInFlightToolsInterrupted│   │
│  │    └─ SettingsModal / CommandPalette / SearchModal / ArtifactPane │   │
│  │                                                                    │   │
│  │  packages/ui (reusable)                                           │   │
│  │    ├─ assistant-stream.ts   → applyAssistantDelta                 │   │
│  │    ├─ smooth-stream.ts      → useSmoothStreamContent (EMA + RAF)  │   │
│  │    ├─ tool-output-stream.ts → applyToolOutputChunk (dedup-by-seq) │   │
│  │    ├─ artifact-preview-registry.ts → resolvePreviewKind           │   │
│  │    ├─ materialize.ts        → materializeTurns / materializeTools │   │
│  │    └─ maka-uri.ts           → parseMakaUri (allowlist router)     │   │
│  │                                                                    │   │
│  │  不能: 读写文件 | spawn 进程 | 访问密钥链 | 直接发起 AI HTTP 请求 │   │
│  └──────────────┬─────────────────────────────────────────────────────┘   │
│                  │ contextBridge (128 channels)                            │
│                  │ ipcRenderer.invoke / ipcRenderer.on                    │
│  ┌──────────────▼─────────────────────────────────────────────────────┐   │
│  │  Preload (preload.ts, 745 lines)                                   │   │
│  │    window.maka = { sessions (18), connections (9), memory (11),   │   │
│  │      settings (6 + bots 6 + wechat 2), artifacts (5 + event),     │   │
│  │      plans (8 + 2 events), usage (5), skills (3), context (3),    │   │
│  │      gateway (2), appWindow (2), app (4), visualSmoke (2),        │   │
│  │      quickChat (1), permissions/capabilities/health (1 each),     │   │
│  │      claudeSubscription (8), codexSubscription (7),               │   │
│  │      cursorSubscription (7), antigravitySubscription (7),         │   │
│  │      dailyReview (2), webSearch (2), search (1),                  │   │
│  │      workspaceInstructions (3), onboarding (3) }                  │   │
│  └──────────────┬─────────────────────────────────────────────────────┘   │
│                  │ Electron IPC (序列化边界, unknown → 手动校验)          │
│  ┌──────────────▼─────────────────────────────────────────────────────┐   │
│  │  Main Process (main.ts, 3443 lines + 26 个辅助文件)                │   │
│  │                                                                    │   │
│  │  ┌────────────────────────────┐  ┌─────────────────────────────┐   │
│  │  │ credential-store.ts (123行)│  │ open-gateway.ts (1451行)    │   │
│  │  │  safeStorage 加密          │  │  本地 HTTP server 127.0.0.1 │   │
│  │  │  credentials.json (atomic) │  │  Bearer token 鉴权          │   │
│  │  │  withQueue 串行化          │  │  REST + SSE endpoints       │   │
│  │  └────────────────────────────┘  └─────────────────────────────┘   │
│  │  ┌────────────────────────────┐  ┌─────────────────────────────┐   │
│  │  │ local-memory-service.ts    │  │ explore-agent-tool.ts       │   │
│  │  │  MEMORY.md 管理             │  │  只读代码探索 (1151行)      │   │
│  │  │  incognito 阻断             │  │  路径 containment           │   │
│  │  │  safeMode 检测              │  │  符号链接拒绝               │   │
│  │  └────────────────────────────┘  └─────────────────────────────┘   │
│  │  ┌────────────────────────────┐  ┌─────────────────────────────┐   │
│  │  │ office-document-tool.ts    │  │ rive-cli.ts + rive-workflow-│   │
│  │  │  officecli 子进程 (635行)  │  │   tool.ts → 外部副作用 ⚠️    │   │
│  │  │  .docx/.xlsx/.pptx 白名单  │  │  rive CLI spawn (414+309行) │   │
│  │  └────────────────────────────┘  └─────────────────────────────┘   │
│  │  ┌────────────────────────────┐  ┌─────────────────────────────┐   │
│  │  │ open-path-guard.ts (71行)  │  │ project-context.ts (72行)   │   │
│  │  │  4 种路径安全打开           │  │  Git root 探测               │   │
│  │  └────────────────────────────┘  └─────────────────────────────┘   │
│  │  ┌────────────────────────────┐  ┌─────────────────────────────┐   │
│  │  │ onboarding-service.ts      │  │ workspace-instructions.ts   │   │
│  │  │  新手引导状态推导 (232行)   │  │  AGENTS.md/CLAUDE.md (245行)│   │
│  │  └────────────────────────────┘  └─────────────────────────────┘   │
│  └──────────────┬─────────────────────────────────────────────────────┘   │
└─────────────────┼───────────────────────────────────────────────────────┘
                  │
  ┌───────────────┼───────────────────────────────────────────────────────┐
  │               ▼                                                       │
  │  @maka/runtime (packages/runtime/src/)                                │
  │                                                                       │
  │  SessionManager (session-manager.ts)                                  │
  │    ├─ sendMessage() → AsyncIterable<SessionEvent>                    │
  │    ├─ stopSession() / retryTurn() / regenerateTurn() / branchFromTurn│
  │    ├─ recoverInterruptedSessions()                                    │
  │    └─ ensureActive() → BackendRegistry → AgentBackend                │
  │                                                                       │
  │  BackendRegistry → factory(kind)                                      │
  │    ├─ 'ai-sdk' → AiSdkBackend (ai-sdk-backend.ts)                     │
  │    │    ├─ send() → dynamic import('ai') → streamText()              │
  │    │    ├─ handleStreamChunk() → chunk → SessionEvent                 │
  │    │    ├─ wrapToolExecute() → permission gating seam                 │
  │    │    └─ repairMakaToolCall() → tool name 修复                      │
  │    └─ 'fake'  → FakeBackend (55行, 确定性测试桩)                      │
  │                                                                       │
  │  PermissionEngine (permission-engine.ts)                              │
  │    ├─ evaluate() → allow/block/prompt (turn-scoped remembered Set)   │
  │    ├─ recordResponse() → resolve parked Promise                       │
  │    └─ endTurn() → reject 所有未决 parked                              │
  │                                                                       │
  │  Builtin Tools (builtin-tools.ts)                                     │
  │    ├─ Bash  (permissionRequired: true)  → resolveWritableInsideCwd   │
  │    ├─ Read  (permissionRequired: false) → resolveExistingInsideCwd    │
  │    ├─ Write (permissionRequired: true)  → resolveWritableInsideCwd   │
  │    ├─ Edit  (permissionRequired: true)  → resolveExistingInsideCwd   │
  │    ├─ Glob  (permissionRequired: false) → 上限 200 文件               │
  │    └─ Grep  (permissionRequired: false) → rg 子进程                   │
  │                                                                       │
  │  Message Materializer (materializer.ts)                               │
  │    ├─ materializeSession() → ChatItem[] (2-pass JSONL)                │
  │    └─ applyAppendedMessage() → 增量更新                               │
  │                                                                       │
  │  Stream Watchdog (stream-watchdog.ts)                                 │
  │    ├─ connect timeout 30s / idle timeout 120s                         │
  │    └─ pause()/resume() for permission wait                            │
  │                                                                       │
  │  Tool Output Delta (tool-output-delta.ts)                             │
  │    ├─ stdout/stderr 行级分片 + seq 单调递增                            │
  │    └─ redactSecrets 跨 chunk 脱敏                                     │
  │                                                                       │
  │  Tool Artifacts (tool-artifacts.ts)                                   │
  │    └─ deriveToolArtifactCandidates() → Write/Edit/Bash → candidates  │
  │                                                                       │
  │  Model Factory (model-factory.ts)                                     │
  │    ├─ Provider dispatch: anthropic → createAnthropic().chat()         │
  │    │                   openai → gpt-5: responses(), 其他: chat()     │
  │    │                   google → createGoogleGenerativeAI().chat()     │
  │    │                   deepseek/ollama → createOpenAICompatible()     │
  │    └─ subscription 特有 headers 注入                                  │
  │                                                                       │
  │  Telemetry (telemetry/)                                               │
  │    ├─ builtin-pricing.ts → 20 款模型硬编码定价                         │
  │    ├─ cost.ts → computeCost() token×单价                              │
  │    ├─ record-llm-call.ts → queueMicrotask 持久化                      │
  │    └─ record-tool-invocation.ts → fire-and-forget                     │
  │                                                                       │
  │  Network (network/)                                                   │
  │    ├─ proxy-env.ts / proxy-dispatcher.ts / proxy-parser.ts            │
  │    ├─ bypass-matcher.ts → CIDR + 通配符域名                           │
  │    └─ active-proxy-state.ts → 单例                                    │
  │                                                                       │
  │  Bot Registry (bots/)                                                 │
  │    ├─ bot-registry.ts → 多平台桥接生命周期                             │
  │    ├─ base-adapter.ts → 抽象类模板                                    │
  │    └─ simple-bridge.ts → Telegram 长轮询 + Feishu                     │
  └───────────────┬───────────────────────────────────────────────────────┘
                  │
  ┌───────────────┼───────────────────────────────────────────────────────┐
  │               ▼                                                       │
  │  @maka/core (packages/core/src/ — 36+ 文件, 41 subpath exports)      │
  │                                                                       │
  │  ┌──────────────────────────────────────────────────────────────┐     │
  │  │ Event Stream      │ events.ts (383行)                        │     │
  │  │                   │  SessionEvent (14 variants)               │     │
  │  │                   │  SessionCommand (4 variants)              │     │
  │  │                   │  ToolOutputDeltaEvent (seq monotonic)     │     │
  │  ├───────────────────┼──────────────────────────────────────────┤     │
  │  │ Session Storage   │ session.ts (336行)                       │     │
  │  │                   │  SessionHeader + StoredMessage (8 types)  │     │
  │  │                   │  deriveTurnRecords() 纯函数               │     │
  │  ├───────────────────┼──────────────────────────────────────────┤     │
  │  │ Permission        │ permission.ts (389行)                    │     │
  │  │                   │  PERMISSION_MODES × TOOL_CATEGORIES       │     │
  │  │                   │  preToolUse() → POLICY[3][11] matrix     │     │
  │  │                   │  categorizeBash() → 6 层分类               │     │
  │  ├───────────────────┼──────────────────────────────────────────┤     │
  │  │ Connection        │ connections.ts (55行)                    │     │
  │  │                   │ llm-connections.ts (521行)                │     │
  │  │                   │  PROVIDER_DEFAULTS (12 providers)         │     │
  │  │                   │  normalizeConnectionBaseUrl()             │     │
  │  ├───────────────────┼──────────────────────────────────────────┤     │
  │  │ Settings          │ settings.ts (858行)                      │     │
  │  │                   │  mergeSettings / normalizeSettings         │     │
  │  │                   │  coerceReadinessForCurrentState()         │     │
  │  ├───────────────────┼──────────────────────────────────────────┤     │
  │  │ Memory (PR-MEMORY-1)│ memory.ts (562行)                      │     │
  │  │                   │  9 隐私门 G1–G9                           │     │
  │  │                   │  validateMemoryWriteRequest() 11步       │     │
  │  ├───────────────────┼──────────────────────────────────────────┤     │
  │  │ Search            │ search.ts (277行)                        │     │
  │  │                   │  6 种 SearchSourceKind                    │     │
  │  │                   │  normalizeSearchQuery/Domain/Url          │     │
  │  ├───────────────────┼──────────────────────────────────────────┤     │
  │  │ Bot Events        │ bot-events.ts (275行)                    │     │
  │  │                   │  botSourceEventKey() 幂等                  │     │
  │  │                   │  isPlaintextResetCommand/HelpCommand      │     │
  │  ├───────────────────┼──────────────────────────────────────────┤     │
  │  │ Misc              │ artifacts.ts, workspace.ts,               │     │
  │  │                   │  runtime-inputs.ts, voice.ts,             │     │
  │  │                   │  capabilities.ts, health.ts,              │     │
  │  │                   │  plan-reminders.ts, incognito.ts,         │     │
  │  │                   │  onboarding.ts, redaction.ts,             │     │
  │  │                   │  oauth-subscription.ts, web-search.ts,    │     │
  │  │                   │  explore-agent.ts, daily-review.ts        │     │
  │  └───────────────────┴──────────────────────────────────────────┘     │
  │                                                                       │
  │  核心设计模式:                                                         │
  │    as const array → union type (关闭枚举)                              │
  │    Result<{ok:true, value} | {ok:false, reason, message}> normalizer   │
  │    pure evaluator (无 UUID/时钟/I/O)                                   │
  │    单一 shape 原则 (定义一次, 多进程消费)                               │
  │  禁止 import: IPC / storage / runtime / electron / renderer           │
  └───────────────┬───────────────────────────────────────────────────────┘
                  │
  ┌───────────────┼───────────────────────────────────────────────────────┐
  │               ▼                                                       │
  │  @maka/storage (packages/storage/src/ — 7 个测试文件)                 │
  │                                                                       │
  │  文件布局 (workspaceRoot 下):                                          │
  │    settings.json           → AppSettings (单文件 JSON, atomic write)  │
  │    llm-connections.json    → LlmConnection[] + defaultSlug             │
  │    telemetry.json          → usageRecords + toolInvocations           │
  │    plan-reminders.json     → PlanReminder[]                            │
  │    sessions/{id}/session.jsonl → header(line1) + messages(append-only)│
  │    artifacts/metadata.jsonl → ArtifactRecord[]                        │
  │    artifacts/{sessionId}/{artifactId}-{name} → 文件内容               │
  │                                                                       │
  │  SessionStore (session-store.ts)                                      │
  │    create() / readHeader() / appendMessages() / updateHeader()        │
  │    withQueue() — per-session 串行化                                    │
  │    writeAtomic() — temp + rename 原子写入                              │
  │    migrateHeader() → backend/pModel/permissionMode/status 升级        │
  │                                                                       │
  │  ConnectionStore (connection-store.ts)                                │
  │    CRUD + 智能缓存失效 (apiKey/baseUrl变更→清除models/testStatus)      │
  │    save() → upsert 语义                                               │
  │    **不存储 secret** (apiKey 在 credential-store.ts)                  │
  │                                                                       │
  │  SettingsStore (settings-store.ts)                                    │
  │    get() → normalizeSettings() schema 迁移                             │
  │    update() → mergeSettings() → atomic write                          │
  │    usageStats() 聚合 → byProvider/byModel/byTool                      │
  │                                                                       │
  │  ArtifactStore (artifact-store.ts)                                    │
  │    resolveArtifactPath() → isSafeRelativeArtifactPath + realpath      │
  │    readText/readBinary (MIME sniff: PNG/JPEG/GIF/WEBP/PDF/SVG)       │
  │    sanitizeArtifactName() → 120 字符截断                               │
  │                                                                       │
  │  TelemetryRepo (telemetry-repo.ts)                                    │
  │    insertLlmCall() / insertToolInvocation() — fire-and-forget          │
  │    logs() → 分页过滤; buckets() → 分组聚合                             │
  │                                                                       │
  │  PlanReminderStore (plan-reminder-store.ts)                           │
  │    CRUD + markTriggered() + snooze() + listDue()                      │
  │    normalizePersistedPlanReminder() 严格反序列化校验                    │
  └───────────────────────────────────────────────────────────────────────┘
```

### 文本架构说明

- **renderer** (React, sandboxed) 通过 `window.maka.*` preload API 调用 128 个 IPC 通道，自己不做任何文件 I/O 或网络请求。
- **preload** 是纯桥接层，`contextBridge.exposeInMainWorld` 暴露类型化 API shape，但运行时无校验。
- **main process** 是安全边界的所有者：`credential-store.ts` 使用 Electron `safeStorage` 加密 API key；文件操作走 `realpath` + `isInside` containment；子进程 spawn 在 main 完成。
- **runtime** 是协议真相持有者：`SessionManager` 管理 JSONL 写入 → `BackendRegistry` 构建 `AiSdkBackend` → `PermissionEngine` gate 工具执行 → `materializer` 物化消息 → `telemetry` 记录用量。
- **core** 是跨进程共享的类型契约层，normalizer/validator/pure evaluator 确保编译期安全，但运行时 enforcement 依赖各消费方的正确实现。
- **storage** 是持久化实现层，JSONL append-only + header atomic write + single-file JSON atomic write。

---

## 3. deep_read_index

| 序号 | 主题 | 建议阅读文件 | 为什么值得深读 | 预期产物 |
|------|------|-------------|---------------|----------|
| D1 | AiSdkBackend 完整 stream→tool→permission→resume 链路 | `packages/runtime/src/ai-sdk-backend.ts` (wrapToolExecute, handleStreamChunk, repairMakaToolCall) | 权限挂起/恢复是最大异步竞态风险点；`handleStreamChunk` 的 chunk 回退链 (`chunk.text ?? chunk.textDelta ?? chunk.delta`) 是 provider 抽象泄漏的关键防线 | 竞态分析文档 + 边界 case 测试清单 |
| D2 | Permission Engine 与 Core Policy 逐行对照 | `packages/runtime/src/permission-engine.ts` + `packages/core/src/permission.ts` | `categorizeBash` 的 6 层分类与 `PERMISSION_POLICY` 的 3×11 矩阵是否一致？`turnRemembered` scopeKey 是否存在不同 args 意外共享白名单？ | 决策矩阵审计报告 |
| D3 | 六个 `isInside` 实现的差异审计 | `main.ts:659`、`workspace-instructions.ts:218`、`office-document-tool.ts:627`、`explore-agent-tool.ts:962`、`local-memory-service.ts:420`、`open-path-guard.ts:67` | 路径 containment 逻辑不一致是最危险的 traversal 漏洞来源。需确认 Windows `\\?\` prefix、macOS `/private/var` 别名是否被正确处理 | 差异矩阵 + 绕过路径 PoC |
| D4 | Credential Store 故障模式 | `apps/desktop/src/main/credential-store.ts` + `main.ts:resolveConnectionSecret` | `catch(() => {})` 静默吞错误可能导致凭据写入失败但 UI 显示成功；`safeStorage.isEncryptionAvailable()` 为 false 时的降级行为不明 | 故障树分析 + 修复建议 |
| D5 | IPC Handler 入参校验完整性 | `apps/desktop/src/main/main.ts:registerIpc()` (128 个 handler) | 每个 handler 收到的是 `unknown`，但校验代码风格不一（有的用 `typeof` guard、有的用 shape check、有的直接 trust）；缺少任何 handler 都可能被恶意 renderer 利用 | Handler 校验矩阵 + fuzzing 输入清单 |
| D6 | Bot 桥接端到端安全 | `packages/runtime/src/bots/simple-bridge.ts` + `bot-registry.ts` + `main.ts:2777-2961` | Bot 模式下无 permission UI，explore 模式自动批准 side-effect 工具；`allowedUserIds` 白名单检查的具体代码路径未确认 | Bot 安全边界文档 |
| D7 | Memory 9 门运行时实现 | `packages/core/src/memory.ts` + `apps/desktop/src/main/local-memory-service.ts` | contract 层定义了 G1–G9，但运行时 enforcement 依赖 `local-memory-service.ts` 的正确调用。需确认每一步是否在 handler 层正确实现 | 隐私门覆盖率矩阵 |
| D8 | OpenGateway 攻击面 | `apps/desktop/src/main/open-gateway.ts` (1451 行) | SSE 连接无上限保护、token 无 rate limit、`decodeURIComponent` 在 `sessionStateMatch` 中可能有路径遍历风险 | 攻击面报告 |
| D9 | Visual Smoke 基础设施 | `apps/desktop/src/main/visual-smoke-fixture.ts` + `scripts/capture-screenshots.mjs` | fixture 场景与 `full-product-test-plan.md` 的对齐度、pixel-level diff 是否已实现、与 CI 的集成 | 基础设施完备度审计 |
| D10 | JSONL 损坏恢复机制 | `packages/storage/src/session-store.ts:readFilePartsUnlocked` | 当前单行 JSON parse 失败会导致整个 session 不可加载，无 per-line try/catch | 修复方案 + 修复工具设计 |
| D11 | Rive CLI 参数注入审计 | `apps/desktop/src/main/rive-cli.ts` + `rive-workflow-tool.ts` | `params` key 有 regex 校验 `/^[A-Za-z0-9_.-]+$/`，但 value 通过 `String()` 转字符串后直接拼入 `key=value`，rive CLI 对 `=` 后内容是否做 shell 展开 | 注入路径清单 |
| D12 | Settings 敏感字段明文存储 | `packages/storage/src/settings-store.ts` + `packages/core/src/settings.ts` | Telegram bot token、代理密码以明文存在 `settings.json`，与 credential-store 的加密体系形成安全缺口 | 敏感数据迁移方案 |

---

## 4. cross_module_flows

### 4.1 App Boot 启动链路

```
app.whenReady()  [main.ts]
  ├─ seedVisualSmokeFixture() 或 ensureBootstrapConnection()
  │    ├─ 从 MAKA_VISUAL_SMOKE_FIXTURE / MAKA_ANTHROPIC_API_KEY 环境变量注入
  │    └─ → ConnectionStore.create() → credentialStore.setSecret()
  │
  ├─ SettingsStore.get() → normalizeSettings()  [@maka/core settings.ts]
  │    └─ → setActiveProxy() → buildProxyDispatcher()  [@maka/runtime network/]
  │
  ├─ TelemetryRepo.load() → 内存加载 telemetry.json
  ├─ recoverInterruptedSessionsOnStartup()
  │    └─ → SessionStore.list() → SessionManager.recoverInterruptedSessions()
  │         └─ 将 running 状态 turn → failed (写入 turn_state system_note)
  │
  ├─ BotRegistry.applySettings(settings.botChat)
  │    └─ → reconcileOne() per platform → SimpleBotBridge.start() [Telegram]
  │         └─ pollTelegram() loop 开始轮询
  │
  ├─ openGateway.sync(settings.openGateway)
  │    └─ → node:http.createServer() 启动在 127.0.0.1:3939
  │
  ├─ createWindow()
  │    ├─ BrowserWindow({ sandbox: true, contextIsolation: true, preload })
  │    ├─ loadURL (dev) / loadFile (packaged)
  │    └─ setWindowOpenHandler → 拦截外部 URL
  │
  └─ refreshPlanReminderTimers()
       └─ → PlanReminderStore.listDue() → setInterval 调度
```

**跨模块关键点**:
- `main.ts` → `SettingsStore` → `normalizeSettings`（core 层）→ `proxy-dispatcher`（runtime 层）
- `BotRegistry`（runtime bots/）→ `SimpleBotBridge`（runtime bots/）→ `SessionManager.sendMessage`（runtime 层）的闭环在 boot 阶段即建立
- `OpenGateway` 在 boot 阶段启动 HTTP server，与 renderer 启动并行

### 4.2 Send Message / Model Stream 发送消息链路

```
Renderer: Composer.onSend(text)
  → AppShell.send(text)  [apps/desktop/src/renderer/main.tsx]
  → window.maka.sessions.send(sessionId, {type:'send', turnId, text})
  → Preload: ipcRenderer.invoke('sessions:send', sessionId, command)
  → Main: ipcMain.handle('sessions:send')  [main.ts]
      1. normalizeSessionSendCommand(command)  ← 形状校验
      2. ensureSessionCanSend(sessionId)
         ├─ SessionStore.readHeader(sessionId)  [@maka/storage]
         ├─ ConnectionStore.get(slug)           [@maka/storage]
         ├─ resolveConnectionSecret(slug)
         │    ├─ OAuth → getAccessTokenInternal()
         │    └─ API key → credentialStore.getSecret()  [Electron safeStorage]
         └─ requireReadyConnection() → isConnectionReady()  [@maka/core]
      3. validateRendererAttachments(attachments) ← 附件审批
      4. SessionManager.sendMessage(sessionId, {turnId, text})  [@maka/runtime]
         ├─ SessionStore.appendMessage(userMsg)  → JSONL 写 UserMessage
         ├─ SessionStore.updateHeader({connectionLocked: true})
         ├─ BackendRegistry.build(kind, ctx) → AiSdkBackend
         │    ├─ modelFactory({connection, apiKey, modelId}) → LanguageModelV2
         │    ├─ dynamic import('ai') → {streamText, stepCountIs}
         │    ├─ build tools dict (wrapToolExecute per tool)
         │    └─ background pump: streamText() → handleStreamChunk → queue
         │         ├─ text-delta → TextDeltaEvent
         │         ├─ reasoning → ThinkingDeltaEvent
         │         ├─ tool-call → IGNORE (wrapToolExecute 已 emit)
         │         ├─ finish → token usage → TokenUsageEvent
         │         └─ end → AssistantMessage + CompleteEvent
         └─ yield from queue (AsyncEventQueue<SessionEvent>)
      5. streamEvents(sessionId, iterator, turnId)
         └─ → ipcMain → renderer (sessions:event:*)
               → handleEvent() in AppShell
                 ├─ text_delta → applyAssistantDelta (5-layer redact+cap)
                 ├─ tool_start → upsertTool
                 ├─ tool_output_delta → appendToolOutputChunk
                 └─ complete → refreshMessages

  [并发] OpenGateway: /v1/sessions/{id}/events SSE push
  [并发] Bot bridge: 若是 bot 触发的 session，回复通过 BotRegistry.sendMessage 发回
```

**跨模块关键点**:
- API key 从 `credentialStore`（main）→ `resolveConnectionSecret`（main）→ `AiSdkBackend.send()`（runtime）→ `streamText` HTTP header 注入，全程不经过 renderer
- SessionEvent 流同时分发到 renderer（IPC）和 OpenGateway（SSE）
- `handleStreamChunk` 中的 chunk 回退链（`chunk.text ?? chunk.textDelta ?? chunk.delta`）是应对 AI SDK provider 格式差异的防御层

### 4.3 Tool Call + Permission 工具调用权限链路

```
streamText 自动调用 tool.execute(args, ctx)  [Vercel AI SDK]
  │
  ▼
wrapToolExecute(tool, turnId, queue)(args, ctx)  [ai-sdk-backend.ts]
  │
  ├─ 1. write ToolCallMessage → SessionStore.appendMessage()  [@maka/storage]
  │      ★ 先于 permission 写入 JSONL — 崩溃恢复保证
  ├─ 2. emit ToolStartEvent → queue
  │
  ├─ 3. permissionRequired === true?
  │      YES → PermissionEngine.evaluate({ toolName, args, mode })
  │      │       [permission-engine.ts]
  │      │     ├─ preToolUse(category, mode, turnRemembered)
  │      │     │    [@maka/core permission.ts] — 纯函数
  │      │     │    ├─ categorizeBash(command) → category
  │      │     │    │    [@maka/core permission.ts] — 6 层分类
  │      │     │    └─ PERMISSION_POLICY[mode][category] → allow/block/prompt
  │      │     │
  │      │     ├─ kind='allow'   → 继续步骤 4
  │      │     ├─ kind='block'   → write synthetic error ToolResultMessage
  │      │     └─ kind='prompt'  → emit PermissionRequestEvent → queue
  │      │                          watchdog.pause()  [stream-watchdog.ts]
  │      │                          await parked ★ 挂起整个 stream
  │      │                            │
  │      │                            │  Renderer: handleEvent('permission_request')
  │      │                            │    → setPermissionBySession → <PermissionDialog>
  │      │                            │    → 用户点击 Allow/Deny
  │      │                            │    → window.maka.sessions.respondToPermission(decision)
  │      │                            │    → IPC → main → SessionManager.respondToPermission()
  │      │                            │    → PermissionEngine.recordResponse(requestId, decision)
  │      │                            │         ├─ resolve parked Promise
  │      │                            │         └─ if rememberForTurn → remembered.add(scopeKey)
  │      │                            │
  │      │                            ├─ decision='allow' → watchdog.resume() → 继续步骤 4
  │      │                            └─ decision='deny'  → write synthetic ToolResultMessage → return
  │      │
  │      NO (Read/Glob/Grep) → 跳过权限, 直接步骤 4
  │
  ├─ 4. run tool.impl(args, { cwd, abortSignal, emitOutput })
  │      └─ [builtin-tools.ts]
  │           ├─ resolveWritableInsideCwd / resolveExistingInsideCwd
  │           │    └─ 路径 containment → fs.realpath + isInside
  │           ├─ 执行成功 → coerceResultContent() → ToolResultEvent
  │           └─ 执行失败 → synthetic/terminal-failure ToolResultEvent
  │
  ├─ 5. recordToolArtifactsSafely(deriveToolArtifactCandidates)
  │      [tool-artifacts.ts → artifact-store.ts]
  │      └─ fire-and-forget
  │
  └─ 6. recordToolInvocation({status, durationMs, bytesIn, bytesOut})
         [telemetry/record-tool-invocation.ts]
         └─ queueMicrotask → TelemetryRepo.insertToolInvocation()
```

**跨模块关键点**:
- Permission 路径跨越 6 层：renderer → IPC → main → SessionManager → PermissionEngine → preToolUse (core)
- StreamWatchdog.pause/resume 与 PermissionEngine.parked 的协同是异步竞态关键点
- Tool 执行结果影响到 artifact derivation（fire-and-forget）和 telemetry recording（fire-and-forget），两者不应阻塞 agent 循环

### 4.4 Provider Credential Save / Test 凭据保存测试链路

```
Renderer: Settings → 模型 → 添加连接
  → window.maka.connections.create({slug, name, providerType, apiKey, baseUrl})
  → IPC → main: ipcMain.handle('connections:create')
      1. normalizeCreateConnectionInput(input)
         ├─ OAuth provider → 忽略 renderer 传的 baseUrl, 使用 PROVIDER_DEFAULTS
         └─ 非 OAuth → normalizeConnectionBaseUrl(input.baseUrl)  [@maka/core]
              └─ scheme allowlist: http/https only (拒绝 javascript/file/data)
              └─ 拒绝 localhost IP, oversize (>2048)

      2. ConnectionStore.create(normalizedInput)  [@maka/storage]
         ├─ validateSlug(slug) → 唯一性检查
         ├─ 构建 LlmConnection (不含 apiKey)
         └─ writeAtomic(llm-connections.json)

      3. if (apiKey) credentialStore.setSecret(slug, 'api_key', apiKey)
         [credential-store.ts]
         ├─ safeStorage.encryptString(apiKey) → Buffer
         ├─ base64 encode
         ├─ writeAtomic(credentials.json)
         │    └─ temp → rename 原子写入
         └─ withQueue 串行化 (防并发写竞态)

      4. emitConnectionListChanged()

  → 自动触发: connections:fetchModels
       ├─ resolveConnectionSecret(slug) → safeStorage.decryptString
       ├─ fetchProviderModels() [model-fetcher.ts]
       │    └─ proxiedFetch() → provider API (/v1/models etc.)
       └─ ConnectionStore.update(slug, {models, modelsFetchedAt, modelSource:'fetched'})

Renderer: 测试连接按钮
  → window.maka.connections.test(slug)
  → IPC → main
      ├─ resolveConnectionSecret(slug)
      ├─ testConnection(connection, apiKey)
      │    └─ 临时 model → streamText("ping") → abort after first token
      └─ ConnectionStore.update(slug, {lastTestStatus, lastTestAt, lastTestMessage})
```

**跨模块关键点**:
- API key 的持久化路径是 `credential-store.ts → safeStorage → credentials.json`，与 connection 元数据路径 `connection-store.ts → llm-connections.json` 是分离的
- `UpdateConnectionInput.patch.apiKey` 通过 IPC 传递但不持久化到 `llm-connections.json`，仅触发 `connection-store.ts` 中的缓存失效逻辑
- `normalizeConnectionBaseUrl` (core 层) 在 IPC 边界做 scheme allowlist gate，但 baseUrl 本身的网络可达性由 renderer 填写的值在 main 侧再次校验

### 4.5 Artifact Materialization 工件物化与预览链路

```
Tool 执行完成
  → wrapToolExecute 步骤 5: recordToolArtifactsSafely()
       [@maka/runtime tool-artifacts.ts]
       ├─ deriveToolArtifactCandidates({ toolName, args, result, cwd })
       │    ├─ Write tool → result.path → ArtifactCandidate(kindForPath)
       │    ├─ Edit tool  → args.path → ArtifactCandidate(kind='diff')
       │    └─ Bash tool  → parse command stdout redirect '>' → ArtifactCandidate
       │
       └─ → Fire-and-forget → artifact recorder
            └─ [desktop main]
                 ├─ ArtifactStore.create({sessionId, turnId, name, kind, content})
                 │    [@maka/storage artifact-store.ts]
                 │    ├─ sanitizeArtifactName(name) → 去非法字符/cap 120
                 │    ├─ writeFile(artifacts/{sessionId}/{uuid}-{name}, content)
                 │    └─ append metadata.jsonl → ArtifactRecord
                 └─ emit artifacts:changed → renderer

Renderer: ArtifactPane
  → mount → window.maka.artifacts.list(sessionId)
  → subscribe artifacts:changed → refresh
  → 用户点击 artifact row → ArtifactPreview 按 kind 分支
       ├─ file: window.maka.artifacts.readText(id)
       │    → main: ArtifactStore.readText(id)
       │         ├─ prepareRead() → resolveArtifactPath()
       │         │    ├─ isSafeRelativeArtifactPath() → 拒绝 absolute/../空段/URL/null
       │         │    ├─ realpath() → 解析符号链接
       │         │    └─ isInsideOrSamePath(artifactRoot, resolved) → containment
       │         └─ readFile(utf8) → 返回 text
       │    → renderer: <pre>{text}</pre>
       │
       ├─ diff: readText → <pre> with line-tagged spans (add/del/hunk)
       │
       ├─ html: readText → <iframe sandbox="allow-scripts" srcdoc> (无 allow-same-origin)
       │
       ├─ image: resolvePreviewKind(input)
       │    [@maka/ui artifact-preview-registry.ts]
       │    ├─ MIME allowlist match (png/jpeg/gif/webp/avif)
       │    ├─ ext fallback (.png/.jpg/.jpeg/.gif/.webp/.avif)
       │    ├─ image payload cap check (2MB base64)
       │    └─ readBinary → <img src="data:<safeMime>;base64,..." />
       │
       └─ pdf: readBinary → <embed type="application/pdf" src="data:...base64" />

  工具栏操作:
    ├─ 在 Finder 中打开: window.maka.app.openArtifactPath(id)
    │    → main: shell.showItemInFolder(resolved) ← 不经过 openPath guard
    ├─ 另存为: window.maka.app.saveArtifactAs(id)
    │    → main: dialog.showSaveDialog + copyFile
    └─ 删除: soft delete (status: 'deleted')
```

**跨模块关键点**:
- Artifact path 安全三次关：`isSafeRelativeArtifactPath` → `realpath` → `isInsideOrSamePath`
- Renderer 从不持有可能的文件系统路径：`ArtifactRecord.relativePath` 在 main 侧计算，renderer 通过 artifact ID 读取
- `recordToolArtifactsSafely` 是 fire-and-forget——artifact 持久化失败只通过 `onWarning` 回调通知，不阻塞 agent 循环
- Artifact 删除是 soft delete（`status: 'deleted'`），无物理删除或自动清理策略

### 4.6 Settings Update 设置更新链路

```
Renderer: SettingsModal → 修改任意设置
  → window.maka.settings.update(patch)
  → IPC → main: ipcMain.handle('settings:update')
      1. mergeSettings(current, patch)  [@maka/core settings.ts]
         ├─ 深度合并 AppSettings
         ├─ bot channel credential 一致性校验
         │    └─ masked token reconciliation (如果 patch 传了占位符, 保留旧值)
         └─ coerceReadinessForCurrentState()
              └─ 无 credentials → readiness 从 credential-claiming 降级到 scaffolded

      2. SettingsStore.update(merged)  [@maka/storage]
         ├─ normalizeSettings(merged)  [@maka/core]
         │    ├─ 闭合枚举校验 (palette fallback, bot readiness 降级)
         │    ├─ legacy field stripping (如 toastPosition)
         │    └─ normalizeAllowedUserIds()
         └─ writeAtomic(settings.json)

      3. 级联副作用 (在 main.ts handler 中触发):
         ├─ 代理变更 → setActiveProxy(network.proxy)
         │    └─ → buildProxyDispatcher() → undici ProxyAgent / SOCKS5 Agent
         ├─ Bot 设置变更 → botRegistry.applySettings(botChat)
         │    └─ → reconcileOne() → 停旧 bridge / 启新 bridge
         ├─ OpenGateway 设置变更 → openGateway.sync(openGateway)
         │    └─ → 启动/停止/重启 HTTP server
         └─ PlanReminder 设置变更 → refreshPlanReminderTimers()

Renderer: SettingsModal 关闭
  → closeSettings()
  ├─ onboarding.refresh() → OnboardingService 重新推导状态
  │    └─ window.maka.onboarding.getSnapshot()
  │         → main: deriveOnboardingState(connections, sessions, milestones, secrets)
  └─ refreshMemoryActive()
       └─ window.maka.memory.getState()
            → main: LocalMemoryService → read MEMORY.md + parseLocalMemoryMarkdown
```

**跨模块关键点**:
- Settings 更新后级联触发 runtime (proxy)、bots (bridge 启停)、openGateway (HTTP server) 的 reconfiguration
- `masked token reconciliation` 机制：renderer 传回的 `apiKey: "••••••••"` 占位符不会覆盖实际存储的明文 token
- Bot channel credential 在 settings.json 中以明文存在，但传给 renderer 时通过 `maskAppSettings` 掩码
- `normalizeSettings` 是 fail-close 的——未知字段被静默丢弃，未知枚举值回退到安全默认值

### 4.7 Bot Bridge 机器人桥接端到端链路

```
BotRegistry.applySettings(settings.botChat)
  → reconcileOne(platform, channelSettings) per platform
      ├─ !enabled → stop existing bridge
      ├─ !isImplemented → scaffoldStatus (not implemented)
      └─ implemented → stop old → build new → wire listeners → start()

SimpleBotBridge.start() [Telegram]
  ├─ telegramApi(getMe) → 验证 token, 填充 identity (bot name/username)
  ├─ running=true, readiness='credentials_valid'
  └─ pollTelegram() loop:
       ├─ telegramApi(getUpdates, {offset, timeout: 15s})
       ├─ 遍历 updates:
       │    ├─ botSourceEventKey(platform, chatId, messageId) → 幂等 key
       │    │    └─ 检查 botRecentSourceEventKeys Map (上限 1000)
       │    ├─ allowedUserIds 白名单过滤
       │    │    └─ 空数组 = 无限制 (sentinel 行为, 需确认测试覆盖)
       │    ├─ isPlaintextResetCommand(text) → reset session
       │    ├─ isPlaintextHelpCommand(text) → 返回帮助信息
       │    └─ emitIncomingMessage({platform, userId, chatId, text})
       │         └─ → onIncomingMessage callback (main.ts)
       │              └─ → findOrCreateBotSession(userId, platform)
       │                   ├─ SessionStore.create({name, permissionMode: 'explore'})
       │                   │    ★ Bot session 默认 explore 模式——自动批准工具
       │                   └─ SessionManager.sendMessage(sessionId, {text})
       │                        └─ → AiSdkBackend.send()
       │                             └─ yield SessionEvent[]
       │                                  └─ → callback → onBotReply
       │                                       └─ → BotRegistry.sendMessage(platform, chatId, text)
       │                                            └─ → SimpleBotBridge.sendMessage()
       │                                                 ├─ splitForTelegram (UTF-16 分片)
       │                                                 ├─ telegramApi(sendMessage) per chunk
       │                                                 ├─ 429 retry (exactly 1 retry, 1s–30s)
       │                                                 └─ ephemeralTtlMs → setTimeout deleteMessage
       └─ offset = lastUpdateId + 1

  [其他平台]
  ├─ Feishu: tenant_access_token 验证 + webhook
  ├─ Discord/DingTalk/QQ: gateway 实现状态未知 (需精读确认)
  └─ WeChat: 二维码轮询登录 (WeChatScanLoginModal)
```

**跨模块关键点**:
- Bot session 默认使用 `explore` permission mode——这意味着 agent 的 side-effect 工具（Write/Edit/Bash）可能在无用户确认的情况下自动执行
- `botSourceEventKey` 的幂等机制依赖 `sourceMessageId` 不变性，平台 message ID 格式变更可能导致重复回复
- `botRecentSourceEventKeys` Map 上限 1000，超限后旧 key 被覆盖，可能允许重放攻击
- Bot 回复通过 `splitForTelegram` UTF-16 分片发送，每片独立 API 调用，任一片失败会导致消息不完整

### 4.8 Rive Workflow Tool 链路

```
Agent → RiveWorkflow tool invocation (permissionRequired: true, categoryHint: 'custom_tool')
  → main (tool impl): buildRiveWorkflowTool().impl(args, { cwd, abortSignal, emitOutput })

    1. buildRiveCommand(args)  [rive-workflow-tool.ts]
       ├─ zod schema 校验:
       │    action ∈ {workflow_run, workflow_status, work_status, workflow_list,
       │              work_output, work_retry, snapshot_capture, work_resume, ...}
       │    params[].key → /^[A-Za-z0-9_.-]+$/ (允许! @ # $ % ^ & * _ - .)
       │    params[].value → String(value) → max 2000 chars
       │    opencodeBin/codexBin → max 2000 chars
       │    workers → max 20
       │    timeoutMs → max 3600000 (1h)
       ├─ switch(action) → 构建 CLI args 数组
       └─ 参数注入: --param key=value

    2. runRiveCli(args, { cwd, env, abortSignal, timeoutMs })  [rive-cli.ts]
       ├─ resolveRiveBinary() → MAKA_RIVE_BIN || RIVE_BIN || 'rive'
       ├─ spawn(bin, args, { cwd, env: MAKA_RIVE_ENV + proxy env, detached })
       ├─ stdout/stderr 累积 (2MB cap)
       ├─ redactRiveText() 实时输出脱敏
       ├─ SIGTERM → 5s → SIGKILL 超时终止
       └─ 解析 JSON envelope → { ok, protocol, display }

    3. 返回 RiveWorkflowToolResult
       ├─ ok: true → successResult (投影 protocol/display/workers/artifacts 字段)
       └─ ok: false → failureResult (reason + message)

外部副作用 ⚠️:
  - workflow_run → 启动多 agent 工作流 (子进程执行任意代码)
  - work_retry → 重试失败的 work node
  - scheduler_resume → 恢复调度器
  - env var MAKA_RIVE_BIN 可覆盖二进制路径 (agent 可控)
```

**跨模块关键点**:
- Rive CLI 是第三方二进制，执行具有外部副作用的命令
- `params` key 有 regex 校验，但 `value` 通过 `String()` 转字符串后直接拼到 `key=value`——如果 rive CLI 对等号后内容做 shell 展开，可能存在注入
- `opencodeBin` / `codexBin` 参数允许 agent 指定二进制路径，zod `max(2000)` 限制较弱
- 超时最长 1h + detached spawn → 子进程可能在 Maka 退出后仍存活

---

## 5. risks

### 5.1 安全 (Security)

| 风险 | 来源报告 | 严重度 | 详情 |
|------|---------|--------|------|
| `settings.json` 明文存储敏感字段 | 03, 04 | **严重** | Telegram bot token、HTTP 代理密码以明文 JSON 存在 workspace 目录。用户备份/云同步可能导致凭据泄露。与 credential-store 的 `safeStorage` 加密体系不一致 |
| 六个 `isInside` 实现逻辑不一致 | 04 | **严重** | `main.ts` 版本额外检查 `rel !== '..'`，`workspace-instructions.ts` 版本缺 `!rel.startsWith(sep)`。macOS `/private/var` 别名、Windows `\\?\` prefix 可能绕过所有六个实现 |
| OpenGateway 本地端口扫描 | 04 | 高 | `127.0.0.1:3939` 对同机所有进程开放, Bearer token 无 rate limit, SSE 连接无上限保护, `decodeURIComponent` 在 `sessionStateMatch` 中有路径遍历风险 |
| Rive CLI 参数注入 | 04 | 高 | `params.value` 通过 `String()` 拼接进 CLI args，`opencodeBin`/`codexBin` 可由 agent 指定路径。可执行 `workflow_run`（多 agent 代码执行） |
| Memory 9-gate 类型约束可被绕过 | 01 | 高 | `MEMORY_SOURCES`/`MEMORY_CANDIDATE_SOURCES` disjoint union 仅 TypeScript 层面安全。运行时若绕过 `validateMemoryWriteRequest` 直接构造 `DurableMemoryEntry`，类型系统不会阻挡 |
| 128 个 IPC handler 入参无自动化 fuzzing | 04 | 高 | 每个 handler 手动校验 `unknown`，风格不一。新增 handler 无安全审查机制 |
| Credential-store 静默吞错误 | 04 | 中 | `withQueue` 内部 `.catch(() => {})` 可能导致凭据写入失败但 UI 显示成功 |
| Session JSONL 含 tool args 敏感信息 | 03 | 中 | `ToolCallMessage.args` 可能逐字包含用户通过工具参数传入的数据库密码/API key |
| Bot 模式 permission 决策 | 02, 04 | 中 | Bot session 默认 `explore` 模式，side-effect 工具自动执行。无 user-facing permission UI |
| `PermissionRequestEvent.args` 为 `unknown` | 01 | 中 | 无结构化校验，错误 shape 可能导致 renderer 渲染异常 |
| `external-link-guard.ts` URL 过滤宽松 | 04 | 中 | 允许 `http:`/`https:`/`mailto:` 不加区分。`openAuthUrl` 仅用 `authRequestId` 映射但不验证源 |
| Artifact 路径 `relativePath` leak risk | 01, 03 | 低 | 注释标注 "never exposed as filesystem path to renderer"，但无代码层 enforcement |

**上游报告间需人工复核的矛盾**:
- 03 报告指出 `settings.json` 含明文 bot token（§2.3、§6.2），04 报告指出 bot token 通过 `maskAppSettings` 掩码后传 renderer，但 main 侧 `settings.json` 的磁盘格式仍是明文。需确认 `settings-store.ts` → `normalizeSettings()` 是否有计划将 bot token 迁移到 credential-store。**建议源码路径**: `packages/storage/src/settings-store.ts:76` (update 方法) + `apps/desktop/src/main/settings-ipc-helpers.ts:maskAppSettings`。

### 5.2 可靠性 (Reliability)

| 风险 | 来源报告 | 严重度 | 详情 |
|------|---------|--------|------|
| JSONL 单行损坏 → 整个 session 不可用 | 03 | 高 | `readFilePartsUnlocked` 中的 `JSON.parse(line)` 无 per-line try/catch。任一行损坏导致整文件读取失败 |
| `appendFile` 非 crash-safe | 03 | 高 | 无 WAL/校验和/事务日志。`appendFile` 中途崩溃 → 最后一行截断 |
| Telemetry `fire-and-forget` 数据丢失 | 03 | 中 | `insertLlmCall`/`insertToolInvocation` 不 await 写入。进程退出时若 queue 未 drain → 数据丢失。无 `before-quit` 钩子 |
| Permission parked Promise 永不 resolve | 02 | 中 | 用户长时间不响应（如关闭电脑），parked Promise 不超时不自反。`endTurn('aborted')` 可 reject，但某些场景（如 bot）无 abort 信号源 |
| `sendMessage` 吞 header 更新失败 | 02 | 中 | `session-manager.ts:415-418` 在 finally 块 catch header 更新错误但不传播。UI 看到正确事件流但 JSONL 不完整 → reload 后 materializer 显示 `interrupted` |
| `schemaVersion: 1` 空有字段 | 03 | 中 | 字段已写入但从未被读取用于版本决策。未来 schema 升级只能靠试探法 |
| 大 session 性能线性退化 | 03 | 中 | `list()` 和 `usageStats()` 需读取全部消息。10k+ 消息无分页/索引 |
| FakeBackend ≠ AiSdkBackend 行为漂移 | 02 | 中 | FakeBackend 不经过 PermissionEngine、不写 TokenUsage、不触发 telemetry。如果 UI 依赖这些事件类型，FakeBackend 表现不一致 |
| 成本计算硬编码 | 02 | 低 | `builtin-pricing.ts` 定价表为 2026-05-20 快照。新增模型需手动更新。未定价模型静默返回 0 |
| Artifact 磁盘无限增长 | 03 | 低 | 只有 soft delete（`status: 'deleted'`），无物理删除或 GC 策略 |

### 5.3 产品体验 (Product Experience)

| 风险 | 来源报告 | 严重度 | 详情 |
|------|---------|--------|------|
| Streaming Markdown 频繁 re-render | 05 | 中 | `ReactMarkdown` 随 smooth-stream 帧更新触发全量 re-render（上限 256KB）。大文档可能掉帧 |
| Optimistic user message 孤儿 | 05 | 中 | send IPC 成功但 `refreshMessagesUntilTurn` poll 被 dispose → 留下 `optimistic-user-<turnId>` 假消息 |
| `sessionsRef` / `activeIdRef` stale-closure | 05 | 中 | renderer 同时维护 useState 和 useRef 两套状态。异步回调中的 ref 可能读取过期值 |
| `thinkingBySession` 残留 | 05 | 低 | `thinking_complete` 后、`text_complete` 前被 abort → thinking text 可能残留在 UI |
| Session status 枚举映射不完整 | 05 | 低 | runtime 新增 `SessionBlockedReason` 值但 renderer 未更新映射 → fallback 到通用文案 |
| Bot 消息回复不完整 | 02 | 中 | Telegram 回复通过 UTF-16 分片发送，每片独立 API 调用。任一片失败导致消息截断 |

### 5.4 测试覆盖 (Test Coverage)

| 风险 | 来源报告 | 严重度 | 详情 |
|------|---------|--------|------|
| 无 React 组件级渲染测试 | 05 | 高 | 所有 renderer 测试是 pure function/contract test。AppShell send/stop/permission/composer 交互路径无自动化覆盖 |
| AiSdkBackend 端到端流测试缺失 | 02 | 中 | 现有测试主要测 `handleStreamChunk` 和 `wrapToolExecute` 独立行为，缺少完整 stream→tool→permission→resume 流程 |
| Bot bridge 端到端测试缺失 | 02 | 中 | 无 `bot-registry.test.ts` 验证 settings apply → reconcile → stop → start 串行化行为 |
| Network proxy 测试缺失 | 02 | 中 | `proxy-dispatcher.ts`、`bypass-matcher.ts` 无测试文件 |
| Credential-store 测试缺失 | 04 | 高 | safeStorage 加密/解密循环无自动化测试 |
| `artifacts.ts` / `workspace.ts` / `runtime-inputs.ts` 无测试 | 01 | 中 | 仅有 type 定义但缺少 normalizer，且无 type assignability 测试 |
| `SessionEvent` union 13/14 variant 无 type assignability 测试 | 01 | 中 | `events.test.ts` 仅 36 行，只测了 `ToolOutputDeltaEvent` |
| Visual smoke 截图 diff 仅做尺寸/完整性 check | 06 | 中 | pixel-level diff 尚未实现（PR-IR-02 v3 promised）。UI layout regression 可能被漏掉 |
| Screenshot stable gate 仅 3/32+ scenarios | 06 | 中 | 覆盖率过低，大部分 UI surface 无视觉回归保护 |
| `smoke.md` 17 条路径 vs 32+ fixture scenarios | 06 | 低 | 多条 UI surface 缺少手动 smoke path |
| Permission engine async 竞态无测试 | 02 | 中 | `parked.resolve()` 后 `wrapToolExecute` 恢复执行的路径无覆盖 |

**上游报告间需人工复核的矛盾**:
- 01 报告标注 `workspace.ts` 缺少测试文件，但 04 报告列出 `workspace-instructions.test.ts` 存在。这两个是不同的文件——`workspace.ts`（core 层类型定义）和 `workspace-instructions.ts`（main 层文件读写）。01 报告关于 `workspace.ts` 的测试缺失判断是正确的。需在代码层确认：**源码路径**: `packages/core/src/workspace.ts` vs `apps/desktop/src/main/workspace-instructions.ts`。

---

## 6. next_dag

以下节点设计为可并行的精读任务，每节点有独立验收产物。

```
Round 2 DAG
─────────────────────────────────────────────────────────────────────────────
│                                                                           │
│  Node A: Permission & Tool Safety Audit                                  │
│  ├─ 输入: 02-runtime, 01-core                                            │
│  ├─ 阅读: ai-sdk-backend.ts:wrapToolExecute, permission-engine.ts,       │
│  │         builtin-tools.ts (6 tools), permission.ts (core)              │
│  ├─ 产出: 竞态分析文档 (parked/resume/abort/reject 四条路径)               │
│  │         + categorizeBash vs PERMISSION_POLICY 一致性验证矩阵            │
│  │         + resolveWritableInsideCwd 符号链接绕过 PoC                    │
│  └─ 验收: 至少发现 1 个已知或新 race condition                            │
│                                                                           │
│  Node B: IPC Surface Security Audit                                      │
│  ├─ 输入: 04-desktop                                                     │
│  ├─ 阅读: main.ts:registerIpc() 全部 128 个 handler, preload.ts          │
│  ├─ 产出: Handler 校验矩阵 (每个 handler 的输入校验方式/安全等级)           │
│  │         + 标注 high-risk handlers (入参直接用作文件路径/spawn 参数)    │
│  └─ 验收: 至少标注 top-10 需要加固的 handler                              │
│                                                                           │
│  Node C: Path Containment Audit                                          │
│  ├─ 输入: 04-desktop, 03-storage                                         │
│  ├─ 阅读: 六个 isInside/isInsideOrSamePath 实现 + artifact-store.ts      │
│  │         resolveArtifactPath + open-path-guard.ts                      │
│  ├─ 产出: 差异矩阵 (每个实现的逻辑差异 + 绕过可能性分析)                    │
│  │         + macOS/Windows 特定别名绕过测试清单                            │
│  └─ 验收: 提供至少 3 个潜在绕过路径                                       │
│                                                                           │
│  Node D: Credential & Settings Security Audit                            │
│  ├─ 输入: 03-storage, 04-desktop                                         │
│  ├─ 阅读: credential-store.ts, settings-store.ts, settings-ipc-helpers.ts│
│  │         settings.ts (core)                                            │
│  ├─ 产出: 凭据生命周期文档 (从输入到加密持久化到使用到清除)                  │
│  │         + settings.json 敏感字段清单 + 迁移到 credential-store 方案     │
│  └─ 验收: 确认所有 API key/token 不经过 renderer 内存                      │
│                                                                           │
│  Node E: Bot Bridge & OpenGateway Attack Surface                          │
│  ├─ 输入: 02-runtime, 04-desktop                                         │
│  ├─ 阅读: simple-bridge.ts, bot-registry.ts, open-gateway.ts,            │
│  │         main.ts:bot handler section                                   │
│  ├─ 产出: Bot 安全边界文档 (explore 模式下的工具决策矩阵 + 可执行的        │
│  │         危险操作清单) + OpenGateway 攻击面报告 (token brute-force,      │
│  │         SSE exhaustion, path traversal) + rate limit 建议              │
│  └─ 验收: OpenGateway 至少 5 个攻击向量                                   │
│                                                                           │
│  Node F: Memory 9-Gate Runtime Enforcement                                │
│  ├─ 输入: 01-core, 04-desktop                                            │
│  ├─ 阅读: memory.ts (core) + local-memory-service.ts (main)              │
│  │         + memory-threat-model.md                                      │
│  ├─ 产出: G1–G9 运行时覆盖率矩阵 (每门的 contract test ↔ handler 实现     │
│  │         对照) + 标注未 enforce 的门                                    │
│  └─ 验收: 每条门都有对应的 handler 代码行号                               │
│                                                                           │
│  Node G: JSONL Durability & Migration                                     │
│  ├─ 输入: 03-storage                                                     │
│  ├─ 阅读: session-store.ts (readFilePartsUnlocked, migrateHeader,         │
│  │         writeAtomic, appendFile), session.ts (core)                   │
│  ├─ 产出: JSONL 损坏场景矩阵 (单行损坏/header 损坏/截断/并发写)             │
│  │         + 恢复方案设计 + schema version 驱动迁移方案                    │
│  └─ 验收: 提供 JSONL 修复工具设计文档                                      │
│                                                                           │
│  Node H: Telemetry & Cost Completeness                                    │
│  ├─ 输入: 02-runtime, 03-storage                                         │
│  ├─ 阅读: telemetry-repo.ts, builtin-pricing.ts, cost.ts,                │
│  │         record-llm-call.ts, record-tool-invocation.ts                 │
│  ├─ 产出: 数据丢失窗口分析 (fire-and-forget 模式下的最大丢失量)             │
│  │         + before-quit 钩子设计 + pricing 更新机制评估                  │
│  └─ 验收: 确认 reasoning token (Claude thinking / OpenAI o-series) 的     │
│           计费是否完整                                                    │
│                                                                           │
│  Node I: Rive CLI & External Tool Injection Audit                         │
│  ├─ 输入: 04-desktop                                                     │
│  ├─ 阅读: rive-cli.ts, rive-workflow-tool.ts, office-document-tool.ts,   │
│  │         explore-agent-tool.ts                                         │
│  ├─ 产出: 外部工具调用矩阵 (每个 action 的副作用等级 + 参数注入可能性)      │
│  │         + spawn 超时/清理策略分析                                       │
│  └─ 验收: 每个工具的外部副作用明确列出                                     │
│                                                                           │
│  Node J: Visual Smoke & Test Infrastructure Gap                           │
│  ├─ 输入: 06-docs, 05-renderer                                           │
│  ├─ 阅读: capture-screenshots.mjs, diff-screenshots.mjs,                  │
│  │         visual-smoke-fixture.ts, smoke.md, full-product-test-plan.md   │
│  ├─ 产出: fixture scenario 完备度矩阵 (32+ scenarios ↔ UI surface 映射)    │
│  │         + 截图覆盖率报告 + CI 集成建议                                  │
│  └─ 验收: 列出未覆盖 screenshot 的 UI surface, 标注补充优先级              │
│                                                                           │
└───────────────────────────────────────────────────────────────────────────┘
```

### 并行度建议

- **Wave 1** (并行): A, B, C, D — 安全审计类节点，相互独立
- **Wave 2** (并行): E, F, G, H, I — 领域深度节点，可并行
- **Wave 3**: J — 测试基础设施完整性，依赖前两波确认的代码理解

### 精读深度建议

Round 2 节点应从"粗读 (architecture)"升级到"精读 (detailed)"：逐函数追踪调用链、标注竞态窗口、提供实际行号引用。Round 3 可进入"安全审计 (security audit)"深度：fuzzing + bypass PoC + patch proposal。

---

> 本报告基于上游 6 个 reader 节点产物综合生成：
> - `01-core-contracts.md` (482 行, 类型契约与测试覆盖)
> - `02-runtime-backends-tools.md` (489 行, 运行时编排与工具执行)
> - `03-storage-persistence.md` (443 行, 持久化层与数据安全)
> - `04-desktop-main-ipc.md` (588 行, main/preload IPC 桥接)
> - `05-renderer-ui.md` (504 行, UI 层状态管理与安全)
> - `06-docs-tests-roadmap.md` (428 行, 文档/测试/roadmap)
>
> 无上游产物缺失。已标注 3 处上游报告间矛盾需人工复核。
>
> 生成时间: 2026-06-13 | 深度档位: architecture
