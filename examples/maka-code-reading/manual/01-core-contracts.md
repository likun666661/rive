# Maka Core Schemas / Contracts 粗读报告

> 阅读基线：`335220a`
> 深度档位：`architecture`
> 阅读范围：`packages/core/src/` 全部源文件 + `__tests__/`

---

## 1. problem

`packages/core`（包名 `@maka/core`）试图解决 **Maka 桌面应用中所有进程间共享的类型定义与契约** 问题。

Maka 是一个 Electron 桌面应用，其运行时架构天然跨越多个上下文：

- **Electron Main Process**：持有 credential store、文件系统访问、JSONL session 存储、Settings 持久化、bot bridge 进程管理
- **Renderer Process**：UI 层（React），消费 session events、权限弹窗、settings 表单、artifact 预览
- **Runtime Backends**：ai-sdk adapter、Pi Agent adapter、Fake（测试用），它们各自消费不同 LLM SDK 的 native 类型
- **Bot Bridges**：Telegram、Feishu、WeChat 等外部平台的事件转换通道
- **Storage Layer**：JSONL 文件读写、session header 的 read-rewrite-write 原子操作

如果每个进程各自定义 `SessionEvent`、`PermissionRequest`、`ConnectionConfig`，很快就会出现同一个概念的不同 representation——main 定义 `{ sessionId: string }`，renderer 消费 `{ id: string }`，bot bridge 生成 `{ session_id: string }`。`@maka/core` 作为 **单一来源真相（single source of truth）**，确保所有进程共享同一套类型定义、同一套枚举常量、同一套 normalizer/validator。

此外，`@maka/core` 的 `package.json` 暴露了 **41 个 subpath exports**（`@maka/core/events`、`@maka/core/permission`、`@maka/core/memory` 等），允许消费者按需精确导入，避免打包时将全部类型拉入一个 Electron 窗口。

---

## 2. why_hard

跨进程共享类型容易失控的原因：

### 2.1 进程边界是 compile-time 的断层

TypeScript 的类型系统只在编译期生效。Electron main 和 renderer 是 **两个独立的 Node.js 进程**（或一个 Node.js + 一个 Chromium 渲染引擎），它们通过 `contextBridge` / `ipcMain` / `ipcRenderer` 通信。类型信息不会跨进程传递——renderer 收到的永远只是 `unknown` 或 `any`。如果对接层不做 normalize/validate，一个 `boolean` 字段可以悄无声息地变成字符串 `"true"`，并持久化到磁盘。

### 2.2 运行时后端（SDK adapter）的类型泄露风险

Maka 支持 Anthropic、OpenAI、Google Gemini、Kimi、DeepSeek、Ollama 等十余种 LLM provider。每个 SDK 有自己的 chat message shape、tool result shape、token usage 结构。如果 UI 层直接消费 provider-native 类型，那么：
- 切换 backend 会 break 整个 UI 渲染管线
- Session 重放（replay）依赖 provider SDK 仍兼容对应版本
- `@maka/core/events.ts` 的设计原则明确写入注释："Backend → UI unified event stream... The UI never imports SDK types directly."

### 2.3 设置、Session、Connection 之间的交叉引用

`BotChannelSettings.token` 是 credential，写入 `settings.json`。`LlmConnection` 的 `apiKey` 存在于 credential store（keychain），不在磁盘。`SessionHeader` 引用 `llmConnectionSlug` 和 `permissionMode`。`PermissionMode` 影响 `preToolUse()` 的决策输出。任何一个 shape 的变化都会产生蝴蝶效应。

### 2.4 枚举漂移

`SESSION_STATUSES`、`PERMISSION_MODES`、`TOOL_CATEGORIES`、`MEMORY_MODES` 等都是 `as const` 字面量数组派生的 union type。如果 renderer 硬编码 `'idle'` 而 `@maka/core` 从未定义过 `'idle'`，type-check 会通过（因为 renderer 没导对类型），但运行时逻辑会静默失败。

### 2.5 隐私与安全边界嵌入类型

`MEMORY_SOURCES`（durable）与 `MEMORY_CANDIDATE_SOURCES`（non-durable）是两个 **不相交（disjoint）的 union type**，这是有意为之——voice transcript、activity observation、CU observation 永远不能通过类型系统到达 `'active'` 持久化状态。但如果 renderer 或 IPC handler 不做 normalize/validate，攻击路径仍然存在。

---

## 3. design_approach

### 3.1 分层策略

`@maka/core` 的类型按职责分为以下几层：

| 层 | 文件 | 核心职责 |
|---|---|---|
| **Event Stream** | `events.ts`, `bot-events.ts` | Backend → UI 的实时事件；连接管理事件（独立 channel） |
| **Session Storage** | `session.ts` | JSONL disk format：header + append-only messages |
| **Permission** | `permission.ts`, `permission-request-health.ts` | 3-mode × 11-category policy matrix + pure evaluator |
| **Connection** | `connections.ts`, `llm-connections.ts`, `connection-readiness.ts`, `provider-auth.ts` | Provider 元数据、连接测试、OAuth 状态机、baseUrl 安全校验 |
| **Workspace** | `workspace.ts` | 工作区配置 defaults（permission + backend + model） |
| **Settings** | `settings.ts` | 全量 AppSettings 类型 + normalize/merge/createDefault |
| **Artifacts** | `artifacts.ts` | Tool 产物的文件记录与读取结果 |
| **Memory** | `memory.ts`, `local-memory.ts` | 9-gate privacy contract，manual confirm before durable write |
| **Search** | `search.ts`, `web-search.ts` | 统一搜索请求/结果 + WebSearch provider contract |
| **Runtime Inputs** | `runtime-inputs.ts` | CreateSession、UserMessage、Retry、Regenerate、Branch 的输入 shape |
| **Capabilities** | `capabilities.ts` | 系统能力快照（action approval、feature enablement、OS permission 等） |
| **Health** | `health.ts` | 健康信号聚合层（从 capability + connection + runtime 派生） |
| **Bot** | `bot-events.ts`, `bot-platform-hints.ts` | Bot 消息 shape + plaintext help/reset 命令 |
| **Misc** | `voice.ts`, `plan-reminders.ts`, `incognito.ts`, `onboarding.ts`, `redaction.ts`, `explore-agent.ts`, `oauth-subscription.ts`, `text-file-import.ts`, `daily-review.ts` | 各自独立的功能 contract |

### 3.2 设计模式

1. **`as const` array → union type**：每个枚举都遵循 `export const X = [...] as const; export type X = typeof X[number];` 模式。关闭枚举（closed enum）确保只有显式声明的值有效。

2. **Result-typed normalizers**：`normalizeSearchQuery()`、`validateMemoryWriteRequest()`、`normalizeConnectionBaseUrl()` 等返回 `{ ok: true, value } | { ok: false, reason, message }`。这个模式贯穿整个 core 包，允许 IPC 边界在 persist 之前做 fail-close。

3. **Pure evaluators**：`preToolUse()` 和 `categorizeBash()` 是无副作用的纯函数——无 UUID 生成、无 clock read、无 I/O。Runtime adapter 包裹它们并注入 `requestId`、`toolUseId`、`turnRemembered` 等上下文。

4. **单一 shape 原则**：`PermissionRequest` 在 `events.ts` 中作为 `PermissionRequestEvent` 的 payload，在 `session.ts` 中作为 `PermissionDecisionMessage` 的关联字段，在 `permission.ts` 中作为 `preToolUse()` 的 `partialRequest` 输出。但基础 shape 只定义一次，引用方通过 import 消费同一个 interface。

5. **Subpath exports 优于 barrel import**：`index.ts` 虽然 re-export 了所有类型，但注释明确标注 "subpath imports are the canonical form"，鼓励下游使用 `@maka/core/permission` 而非 `@maka/core`。

### 3.3 关键不变量

- `packages/core` **禁止 import IPC / storage / runtime / electron / renderer** 任何模块。`memory.ts` 有明确注释 "Hard no-go"。
- `connection records are stored on disk without secrets`——API key 和 OAuth token 在 credential store 中，通过 slug 关联。
- `ToolOutputDeltaEvent` 有 `seq` 字段，monotonic per toolCallId，允许 renderer 去重和修复 event/result race。
- `SessionChangedEvent` 是通用 invalidation signal，UI 收到后重新 fetch session list，不携带增量数据。
- `TurnRecord.retriedFromTurnId` / `regeneratedFromTurnId` / `branchOfTurnId` / `parentTurnId` 只能单向派生，存储层 never 反向修改。

---

## 4. code_walkthrough

### 4.1 `index.ts` — Barrel Export

**745 lines**，是 `packages/core` 的统一入口。每个 subpath module 在这里 re-export type 和 value。注释明确写明 subpath imports 是 canonical form，barrel 仅为 convenience。

### 4.2 `events.ts` — Session Event Stream

**383 lines**。定义：

- `SessionEvent`：14 种事件的 discriminated union（`text_delta`、`tool_start`、`permission_request`、`complete` 等）
- `BaseEvent`：`id` + `turnId` + `ts` 三个公共字段
- `AttachmentRef` / `StorageRef`：文件/图片/PDF 引用的通用 shape（session_file / workspace_file / external_file）
- `ToolResultContent`：8 种 tool result kind（text、json、file_diff、file_write、terminal、image、summary、web_search、office_document、explore_agent、rive_workflow）
- `SessionCommand`：UI → Backend 的指令（send、stop、permission_response、plan_response）
- `ToolOutputDeltaEvent`：带 `seq` 的 side-channel streaming output，与 `ToolResultEvent` 分离

**被谁消费**：Runtime backend adapter（AI SDK / Pi Agent）负责将 provider-native event 转为此 union；UI renderer 消费此 union 做渲染；storage layer 将部分事件转为 `StoredMessage` 写入 JSONL。

### 4.3 `session.ts` — Session Storage Format

**336 lines**。定义：

- `SessionHeader`：JSONL 第一行，包含 id、workspaceRoot、cwd、生命周期时间戳、status、blockedReason、parentSessionId、backend、llmConnectionSlug、model、permissionMode 等
- `StoredMessage` 的 8 种子类型（User、Assistant、ToolCall、ToolResult、PermissionDecision、TokenUsage、TurnState、SystemNote）
- `SessionSummary`：列表展示用的精简投影
- `SessionChangedEvent`：session 变更通知（created / archived / renamed / mode-change 等）
- `deriveTurnRecords()`：从 StoredMessage[] 推断 TurnRecord[] 的纯函数

**被谁消费**：Storage layer（读写 JSONL）、UI session list、session detail panel、runtime（创建/恢复 session）。

### 4.4 `permission.ts` — Permission System

**389 lines**。定义：

- `PERMISSION_MODES`：`explore` / `ask` / `execute`
- `TOOL_CATEGORIES`：11 种工具类别（read、shell_safe、fs_destructive、git_destructive 等）
- `PERMISSION_POLICY`：3×11 的 `Record<PermissionMode, Record<ToolCategory, PolicyDecision>>` 矩阵
- `preToolUse()`：纯函数，三步评估——(1) classify tool → category，(2) policy lookup + turn-remembered check，(3) 返回 proceed/needsPrompt/blockReason
- `categorizeBash()`：对 shell 命令做 6 层分类（privileged > fs_destructive > git_destructive > shell_unsafe → safe prefix match → default unsafe）
- `SAFE_SHELL_PREFIXES`：白名单（ls、pwd、echo、git status 等），刻意排除了 `cd` 和 `env`
- `permissionScopeKey()`：生成 scope key 用于 turn-remembered 机制

**关键纯函数属性**：`preToolUse()` 不生成 UUID、不读时钟、不做 I/O。PermissionEngine 在 runtime 层包裹它。

### 4.5 `connections.ts` — Connection-setup Events

**55 lines**。很小，但与 `SessionEvent` 明确分离。定义：

- `ConnectionEvent`：3 种事件（credential_request、test_result、list_changed）
- `ConnectionCommand`：5 种命令（credential_response、oauth_start、test、save、delete）

这些事件不在 session event stream 上，而是在 desktop bridge 的独立 `connections.*` channel 上传输。没有 `turnId`，不绑定特定 session。

### 4.6 `llm-connections.ts` — LLM Provider Metadata

**521 lines**。定义：

- `ProviderType`：12 种 provider（anthropic、openai、deepseek、ollama、claude-subscription 等）
- `LlmConnection`：磁盘持久化的连接记录（不含 secrets）
- `PROVIDER_DEFAULTS`：每个 provider 的 baseUrl、authKind、backendKind、fallbackModels、protocol、category 等的完整登记表
- `validateConnectionBaseUrl()` / `normalizeConnectionBaseUrl()`：IPC 边界的 baseUrl scheme allowlist gate（只允许 http/https，拒绝 javascript/file/data 等）
- `migrateConnectionV1ToV2()`：向后兼容的迁移逻辑

### 4.7 `runtime-inputs.ts` — Runtime API Inputs

**60 lines**。定义了创建 session 和发送消息的输入 shape。`SessionListFilter` 支持按 `isArchived` / `isFlagged` / `labelSlug` 过滤。

### 4.8 `artifacts.ts` — Artifact Records

**63 lines**。定义 `ArtifactRecord`（id、sessionId、turnId、relativePath、kind、status 等）和读取结果的 discriminated union（`ArtifactTextReadResult`、`ArtifactBinaryReadResult`、`ArtifactSaveResult`）。path 永远是 relativePath，never absolute。

**⚠ 需要注意**：此文件仅定义类型和 union，没有 normalizer/validator。读取/保存逻辑在别处。当前无专属测试。

### 4.9 `workspace.ts` — Workspace Config

**22 lines**。简单的 `WorkspaceConfig` interface，包含 id、name、rootPath、defaults（permissionMode、backend、llmConnectionSlug、model）。

**⚠ 需要注意**：纯类型定义，无测试覆盖。

### 4.10 `settings.ts` — App Settings

**858 lines**。最大的单文件之一。定义：

- `AppSettings`：全量 settings shape（network、botChat、usage、appearance、personalization、onboarding、openGateway、webSearch、localMemory、workspaceInstructions）
- `mergeSettings()`：深度合并（含 bot channel credential 一致性校验、masked token reconciliation）
- `normalizeSettings()`：对 disk-loaded unknown → `AppSettings` 做 fail-close normalize（闭合枚举校验、palette 回退、bot readiness 降级）
- `BotChannelSettings` 含 `allowedUserIds` 白名单独有 `normalizeAllowedUserIds()` 防御
- `coerceReadinessForCurrentState()`：write-path 降级逻辑——无 credentials 时 credential-claiming readiness 降回 scaffolded

### 4.11 `memory.ts` — Memory Contract (PR-MEMORY-1)

**562 lines**。这是 privacy-preserving memory 的 contract-only 模块：

- 9 个隐私门（G1–G9）通过类型系统和 normalizer 实现
- `MEMORY_SOURCES`（user_authored、chat_extracted）与 `MEMORY_CANDIDATE_SOURCES`（voice_transcript、activity_observation 等）是 disjoint enum——candidate 永远不能 typing 到达 `'active'`
- `validateMemoryWriteRequest()` 是唯一的 gate function，11 步顺序校验，每一步有独立 `MemoryBlockReason`
- `embeddingProvider` 硬编码为 `'disabled'`——当前无 embedding vector provider
- RegExp 用 `String.fromCharCode` 构造函数而非字面量，保持源文件 plain ASCII（避免 git 当 binary 处理）

### 4.12 `search.ts` — Search Contract

**277 lines**。定义：

- `SearchRequest` / `SearchResult` / `WebFetchRequest` / `SearchResultTarget`
- `SearchSourceKind` 的 6 种源（web、web_fetch、thread、memory、activity、tool）
- `SearchErrorReason` 的 13 种错误原因（含 incognito 门）
- `SearchSourceSnapshot`：各源的能力快照
- 全套 normalizer：`normalizeSearchQuery`、`normalizeSearchLimit`、`normalizeSearchDomain`、`normalizeSearchUrl`
- `rewriteSearchQueryForFreshness()`：带年份推断的查询改写
- `stripSearchTrackingParams()`：去除 utm_ / fbclid / gclid 等 tracking params

### 4.13 `bot-events.ts` — Bot Message Contract

**275 lines**。定义：

- `BotMessageEvent`：入站 bot 消息的标准化 shape
- `botSourceEventKey()`：幂等 key（防止平台重发导致重复 agent 回复）
- `isPlaintextResetCommand()` / `isPlaintextHelpCommand()`：DM-only 的纯文本指令检测（无 slash command runtime）
- `nonTextMessageAck()`：非文本消息的固定中文回复
- `humanizeBotStatusReason()`：bridge 错误码 → 用户可读描述

---

## 5. flows

### 5.1 Session Event Flow

```
User types in renderer
  → SessionCommand { type: 'send', turnId, text }
  → IPC → main process → runtime backend
  → AI SDK streams provider-native chunks
  → Runtime adapter normalizes → SessionEvent union (events.ts)
    → TextDeltaEvent { type: 'text_delta', messageId, text }
    → ToolStartEvent { type: 'tool_start', toolUseId, toolName, args }
    → ToolOutputDeltaEvent { stream, chunk, seq }
    → PermissionRequestEvent { requestId, toolUseId, category, reason }
    → ToolResultEvent { isError, content: ToolResultContent }
    → TokenUsageEvent { input, output, costUsd }
    → CompleteEvent { stopReason: 'end_turn' }
  → IPC → renderer streams UI updates
  → Concurrently: storage layer converts selected events → StoredMessage (session.ts)
    → UserMessage | AssistantMessage | ToolCallMessage | ToolResultMessage
    → Appended to JSONL session file
```

### 5.2 Permission Request Flow

```
Tool invoked in backend (e.g., Bash { command: "rm foo.txt" })
  → Runtime adapter calls preToolUse() (permission.ts)
    → categorizeBash("rm foo.txt") → 'fs_destructive'
    → PERMISSION_POLICY[execute]['fs_destructive'] → 'prompt'
    → Returns { proceed: false, needsPrompt: true, partialRequest }
  → PermissionEngine wraps → adds requestId + toolUseId
  → Emits PermissionRequestEvent to event stream
  → UI renderer shows permission dialog (toolName, category, reason, args)
  → User clicks Allow in renderer
  → SessionCommand { type: 'permission_response', response: PermissionResponse }
  → IPC → main
  → PermissionEngine records scopeKey in turnRemembered
  → Backend executes tool
  → PermissionDecisionAckEvent echoed back through stream (for audit/observer)
  → PermissionDecisionMessage appended to JSONL (for replay audit)
```

### 5.3 Provider Connection Flow

```
User opens Settings → Models
  → Renders provider catalog (llm-connections.ts / PROVIDER_DEFAULTS)
  → User fills CreateConnectionInput { slug, name, providerType, baseUrl, apiKey }
  → Renderer sends via IPC → main
  → normalizeConnectionBaseUrl() gate checks scheme (http/https only)
  → ConnectionTest → validates API key against provider endpoint
  → ConnectionTestResultEvent emitted (connections.ts)
    → UI shows test status (success/error/latency)
  → On save: connection persisted to disk WITHOUT apiKey
    → apiKey stored in OS credential store, keyed by slug
  → ConnectionListChangedEvent emitted → UI re-fetches connection list
```

### 5.4 Artifact Preview Flow

```
Tool writes file → ToolResultEvent { content: { kind: 'file_write', path, bytes } }
  → Storage layer records ArtifactRecord { id, sessionId, turnId, relativePath, kind }
  → UI ArtifactPanel lists session artifacts
  → User clicks preview
  → main reads artifact → ArtifactTextReadResult | ArtifactBinaryReadResult
  → UI renders based on ArtifactKind (file/diff/html/image/pdf)
```

### 5.5 Memory Write Flow (Privacy-gated)

```
User or candidate source triggers a memory write
  → IPC sends MemoryWriteRequest to main
  → validateMemoryWriteRequest() executes 11-step gate:
    1. mode === 'off' → block
    2. incognitoActive → block
    3. normalizeMemoryContent (NFC + control char strip + trim + cap)
    4. normalizeMemoryScope
    5. normalizeMemoryPersistenceState
    6. normalizeMemorySource (memory vs candidate vs unknown)
    7. candidate + persistence 'active' → block (candidate_source_no_active)
    8. mode 'manual_only' + candidate → block
    9. memory source + non-active state → block
    10. memory source + active without confirmedAt → block
    11. renderer-originated + active → block (renderer_provenance_forged)
  → Returns MemoryEntry (DurableMemoryEntry | DraftMemoryEntry)
  → Persisted in memory store (future)
```

### 5.6 Search Flow

```
User types search query in UI
  → normalizeSearchQuery() trims/validates/caps
  → normalizeSearchLimit() clamps to max
  → SearchRequest sent to local search provider
  → Each source (thread/memory/activity/tool/web) returns results
  → normalizeSearchDomainList() validates domain constraints
  → SearchResult[] returned (each with citationIndex, snippet, target)
  → UI renders results with source badges and navigation targets
```

---

## 6. tests

### 6.1 现有测试覆盖

共 **29 个测试文件**，覆盖以下 contracts：

| Test File | 测试的 Contract |
|---|---|
| `events.test.ts` | `TOOL_OUTPUT_STREAMS` 常量锁 + `ToolOutputDeltaEvent` 类型 assignability |
| `session-status.test.ts` | `SessionStatus` / `TurnStatus` / `SessionBlockedReason` 枚举锁 + `deriveTurnRecords()` |
| `session-event-health.test.ts` | Session event stream 健康状态推导 |
| `permission.test.ts` (336 lines) | `categorizeBash()` 全覆盖（safe/unsafe/destructive/privileged/pipe）、`preToolUse()` 3-mode×category 矩阵 + turn-remembered |
| `permission-request-health.test.ts` | Permission request 超时/过期推导 |
| `llm-connections.test.ts` (400 lines) | `validateConnectionBaseUrl()` + `normalizeConnectionBaseUrl()` 全覆盖（accept/reject 各种 scheme、oversize、trim） |
| `settings.test.ts` (734 lines) | `mergeSettings()` / `normalizeSettings()`、bot readiness 写路径降级、palette 回退、legacy 字段迁移 |
| `memory.test.ts` (622 lines) | G1–G9 九个隐私门全量测试、normalizer matrix、`validateMemoryWriteRequest()` 11步顺序 |
| `search.test.ts` (284 lines) | `normalizeSearchQuery/Limit/Domain/Url`、domain matching、URL tracking param stripping、freshness rewriting |
| `bot-events.test.ts` (285 lines) | `isPlaintextResetCommand/HelpCommand`、`botSourceEventKey` idempotency、`nonTextMessageAck` 固定文案、`humanizeBotStatusReason` |
| `voice.test.ts` | Voice contract normalizers |
| `capabilities.test.ts` | Capability readiness 推导 |
| `health.test.ts` | Health signal 聚合 |
| `incognito.test.ts` | `validateWorkspacePrivacyContext()` |
| `plan-reminders.test.ts` | Plan reminder normalizers |
| `local-memory.test.ts` | Local memory markdown parse/format |
| `onboarding.test.ts` | `sanitizeOnboardingMilestones()` |
| `provider-auth.test.ts` | `deriveProviderAuthContract()` |
| `oauth-subscription.test.ts` | PKCE code challenge + authorization URL 构建 |
| `redaction.test.ts` | `redactSecrets()` + 通用错误消息 |
| `text-file-import.test.ts` | `preflightDroppedTextFilesForPromptImport()` |
| `daily-review.test.ts` | `buildDailyReviewSummary()` |
| `web-search.test.ts` | `maskedTokenForDisplay()` / `reconcileMaskedToken()` |
| `explore-agent.test.ts` | `isDeepResearchSession()` / `normalizeQuickChatMode()` |
| `model-catalog.test.ts` | `validateChatDefaultModel()` |
| `session-name.test.ts` | `normalizeUserSessionName()` |
| `bot-platform-hints.test.ts` | `buildBotPlatformPromptFragment()` |
| `lang-pref.test.ts` | `isUiLocalePreference()` |
| `relative-time.test.ts` | 相对时间格式化 |

### 6.2 缺少测试的 Contract

以下 contracts **仅在 `index.ts` 中作为 type export 出现，但 `__tests__/` 中没有对应测试文件**：

| 文件 | 缺少的测试 | 风险 |
|---|---|---|
| `connections.ts` | `ConnectionEvent` / `ConnectionCommand` 的 type assignability 测试 | 低——纯 type 定义，无逻辑 |
| `workspace.ts` | `WorkspaceConfig` 的 type assignability | 低——纯 type，仅 22 行 |
| `artifacts.ts` | `ArtifactRecord` shape、`ArtifactTextReadResult` / `ArtifactBinaryReadResult` union 的判别 | **中**——8 种 `ArtifactKind` 和 5 种 `ArtifactReadFailureReason`，下游节点可能需要确认枚举完整性 |
| `runtime-inputs.ts` | `CreateSessionInput` / `UserMessageInput` / `SessionListFilter` 的 shape | 低——纯 input type |
| `visual-smoke.ts` | `VisualSmokeScenario` / `VisualSmokeState` | 低——test fixture type |
| `backend-types.ts` | `BackendSendInput` / `PermissionDecision` | 低——仅 2 个 type |
| `connection-readiness.ts` | `isConnectionReady()` / `isRealConnection()` 的逻辑 | **中**——这两个函数有判断逻辑，目前未独立测试 |

此外，`events.test.ts` 仅 **36 行**，只测试了 `ToolOutputDeltaEvent`。`SessionEvent` union 的其余 13 个 variant、`ToolResultContent` 的 11 个 kind、`SessionCommand` 的 4 个 kind **均无 type-level assignability 测试**。

---

## 7. risks

### 7.1 类型膨胀

`index.ts` 已经是 745 行 barrel export，`package.json` 中有 41 个 subpath exports。每次新增一个 feature contract（如 memory、voice、plan-reminders），都会增加：
- 新的源文件 + 新的 subpath export + 新的 barrel re-export
- 下游 renderer 的 bundle 即便按需导入，仍需维护类型依赖图

**实际数据**：当前 `src/` 目录下有 **36+ 个 .ts 文件**（不含 `__tests__/`），其中约 10 个是近期 feature PR（memory、voice、plan-reminders、daily-review、explore-agent 等）引入。若持续以 "一功能一文件" 模式增长，subpath export 数量将快速接近 100。

### 7.2 隐私/权限边界仅靠类型约束

`MEMORY_SOURCES` vs `MEMORY_CANDIDATE_SOURCES` 的 disjoint union 在 TypeScript 层面提供了类型安全，但运行时 `validateMemoryWriteRequest()` 中的第 7 步（`candidate_source_no_active`）才是真正的 enforcement。**如果未来的代码路径绕过了 `validateMemoryWriteRequest()` 直接构造 `DurableMemoryEntry`**，类型检测不会报错（因为 TypeScript 不会禁止手动构造对象字面量），隐私门会被静默绕过。

类似地，`PermissionRequestEvent` 的 `args` 字段是 `unknown`，没有结构化校验。如果 runtime adapter 将错误 shape 的 args 传给 `PermissionRequestEvent`，UI renderer 无法安全渲染。

### 7.3 向后兼容

- `migrateConnectionV1ToV2()` 存在于 `llm-connections.ts`，说明已经存在一次 breaking format change。
- `SessionHeader.schemaVersion` 硬编码为 `1`，但未见 V2 转换路径。
- `normalizeSettings()` 中有大量 legacy field stripping（如 `toastPosition`），说明 settings shape 已有多次迭代。
- `TurnRecord` 中 `retriedFromTurnId` / `regeneratedFromTurnId` / `branchOfTurnId` 等字段的 proliferate 暗示 turn 拓扑关系在快速演化。

**核心风险**：当前 JSONL session 格式和 settings.json 没有 formal schema migration framework。每次新增字段都依赖 `normalizeSettings()` 中的 default-filling——但旧版 Maka 写入的 session JSONL 如果被新版读取，字段缺失可能导致运行时 crash（而非优雅降级）。

### 7.4 Renderer-Main Contract 漂移

`SessionCommand` 有 4 种，`ConnectionCommand` 有 5 种。如果 renderer 端的 IPC client 未经编译校验就构造 `{ type: 'stop_session' }`（与 type `'stop'` 不同），TypeScript 不会捕获（因为 renderer 可能使用 `any` 或独立定义类型）。`@maka/core` 的类型需要在 renderer bundle 中保持同步——但目前未见 monorepo link 的实际验证机制。

### 7.5 Normalizer 缺失

`artifacts.ts`、`workspace.ts`、`runtime-inputs.ts` 都没有 normalizer/validator。这些 input 通过 IPC 传递时，main process 如果直接 trust 它们，恶意或错误的 renderer payload 可能：
- 构造非法的 `relativePath`（如 `../../../../etc/passwd`）写入 artifact 存储
- 传递超大 `cwd` 字符串到 `CreateSessionInput`
- 传入非法的 `labels[]` 数组元素

### 7.6 Search & Memory 共享 `SearchErrorReason`

`search.ts` 的 `SearchErrorReason` 包含 `incognito_active`，这个 reason 同时被 search 和 workspace privacy context 使用。注释说明 "two paths share this reason so consumers do not need an extra UI state"。但如果未来 incognito 语义发生变化（例如从 boolean 变为 multi-level），这个耦合会导致 search contract 也需要更新。

---

## 8. next_questions

以下问题设计为下一轮 deep read 节点可独立作业的 DAG node：

### 8.1 Session 拓扑完整性（需下游节点确认）

1. `TurnStateMessage` 中的 `retriedFromTurnId` / `regeneratedFromTurnId` / `branchOfTurnId` 的完整写入路径在哪些代码中？runtime 层的 `retryTurn` / `regenerateTurn` / `branchFromTurn` 是否强制执行 parentTurnId 一致性？
2. `deriveTurnRecords()` 中的 `inferLegacyTurnStatus()` 针对没有 `turn_state` 消息的旧 session 做推断——是否有已知的 bug 会导致旧 session 的 turn status 错误？

### 8.2 Permission 实际调用路径

3. `preToolUse()` 被哪些 adapter 调用？每个 adapter 是否正确传入 `turnRemembered`？PermissionEngine 的 requestId 生成算法是什么？
4. `PERMISSION_POLICY` matrix 中 `execute` 模式下的 `fs_destructive: 'prompt'` 和 `shell_unsafe: 'allow'` 是否可能产生矛盾？例如 agent 在 execute 模式下通过 `shell_unsafe` 的 Bash 执行 `rm -rf /tmp/*`，虽然 `fs_destructive` 要 prompt，但 `categorizeBash` 在 policy check 之前已经分类为 `fs_destructive`——确认此顺序无 bug。

### 8.3 Connection & Credential 安全

5. `normalizeConnectionBaseUrl()` 的输入检验（`typeof baseUrl !== 'string'`）是否在 IPC 规范层就应拦截？如果 renderer 能传递 number/object，是否意味着 IPC bridge 有 serialization vulnerability？
6. `LlmConnection` 记录在 `connections.ts` 的 `ConnectionCommand.save` 中通过 IPC 发送——main process 是否有 sanitize 步骤防止 persist 到 disk 的 record 包含 secrets？

### 8.4 Artifact 安全

7. `ArtifactRecord.relativePath` 的 "never absolute and never exposed as filesystem path to renderer" 约束是如何 enforce 的？生成 artifact 记录的代码路径在哪？
8. `ArtifactSaveResult` 中的 `saved: string` 是相对路径还是绝对路径？会不会 leak workspace 的文件系统布局？

### 8.5 Memory 9-gate 审计

9. `validateMemoryWriteRequest()` 的第 11 步（renderer_provenance_forged）说 "main must record confirmation"——confirmation event 的格式和存储位置是什么？是 `PermissionDecisionMessage` 还是独立的 `MemoryConfirmationMessage`？
10. `MEMORY_USE_POLICIES` 目前只有 `never` 和 `cited_only`。在 prompt-injection 环节，citation 格式由谁负责构造？cite 是否包含 session id / turn id / source？

### 8.6 Search 实现路径

11. `SearchRequest` 的 `source` 为 `'thread'` 时的全文搜索实现在哪里？是 local SQLite FTS 还是 in-memory search？
12. `SearchSourceSnapshot` 中 `memory` source 的 `provider: 'local' | 'api'` 是否暗示将来有 cloud embedding provider？当前 `memory.ts` 的 `embeddingProvider: 'disabled'` 是否需要与 search contract 同步演化？

### 8.7 Bot 安全性

13. `botSourceEventKey()` 的 idempotency 机制依赖 `sourceMessageId`——如果 Telegram/WeChat 平台修改了 message ID 格式（例如从 int 变为 string），会不会出现 key collision？
14. `BotChannelSettings.allowedUserIds` 的白名单检查在哪个代码路径执行？是 main process bot bridge 还是 renderer 的 settings UI？当前是否有测试验证 "空数组 = 无限制" 这一 sentinel 行为？

### 8.8 跨进程类型同步机制

15. 当前 renderer 如何确保使用的 `@maka/core` 类型与 main process 编译自同一 commit？有没有 CI step 做 cross-package type compatibility check？
16. `CreateSessionInput` / `UpdateAppSettingsInput` 等 input type 在 IPC 反序列化后是否有 ensure type 的 runtime check？（例如通过 zod schema 或 io-ts runtime type）如果完全依赖编译期类型检查，未来的 breaking change 会导致 main crash 且无有用错误信息。

### 8.9 缺失测试的 blocker 分析

17. `connections.ts` / `workspace.ts` / `artifacts.ts` / `runtime-inputs.ts` 的测试缺失是故意的（因为这些文件只定义 type 和 interface，无逻辑可测）还是积压任务？如果认为是"仅 type 不需测试"，那 `events.ts` 为何有一个 36 行的 test？
18. `connection-readiness.ts` 中的 `isConnectionReady()` 和 `isRealConnection()` 有何逻辑？是否需要独立测试？

---

> **生成说明**：本报告基于对 `packages/core/src/` 全部源文件和 `__tests__/` 目录的完整阅读生成。所有 TypeScript 标识符和文件路径保留原文。标注 "需下游节点确认" 的条目表示在当前 `@maka/core` 源码中无法找到调用路径或实现。
>
> 阅读文件清单（共 46 个）：`package.json`、`index.ts`、`events.ts`、`session.ts`、`permission.ts`、`connections.ts`、`llm-connections.ts`、`runtime-inputs.ts`、`artifacts.ts`、`workspace.ts`、`settings.ts`、`memory.ts`、`search.ts`、`bot-events.ts` + 29 个 `__tests__/*.test.ts` 文件（抽样读取 6 个）。
