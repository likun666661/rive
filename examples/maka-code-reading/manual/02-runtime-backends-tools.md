# Maka Runtime / Backends / Tools 粗读报告

> 基线: `335220a` | 包: `@maka/runtime` | 深度: architecture

---

## 1. problem — runtime 层的定位

`@maka/runtime` 是 Maka 的**核心编排层**，它不能放在 Electron main 或 UI 渲染进程中，原因有三：

1. **平台无关的会话生命周期管理**：`SessionManager` 需要持久化 JSONL 消息流、管理 `ActiveSession` 状态机、处理中断恢复。这些逻辑与 Electron IPC、React 组件树无关，是一个纯 Node.js 可运行的领域模型。
2. **后端抽象与多 Provider 切换**：`AiSdkBackend` 包裹 Vercel AI SDK，需要通过 `BackendRegistry` 在运行时按 `BackendKind`（`'ai-sdk'` / `'fake'`）动态构建 Agent 实例。Electron main 进程只负责把 `SessionStore` 传入，具体的 stream → event → JSONL 流转由 runtime 完成。
3. **权限引擎 + 工具执行的安全边界**：`PermissionEngine` 控制 Bash/Write/Edit 等危险工具的 allow/block/prompt 三部曲，工具实现 `builtin-tools.ts` 强制执行 `cwd` sandbox。这些安全逻辑必须与 UI 线程隔离，防止渲染进程被注入后绕过沙箱。

简言之，runtime 是 **protocol truth** 的持有者——消息写入 JSONL 即事实，UI 只是这个 truth 的消费方。

---

## 2. why_hard — 复杂度来源

```
┌──────────────────────────────────────────────────────────────────┐
│  Provider         AI SDK Stream      Maka Event Stream            │
│  (Anthropic/      (fullStream)       (SessionEvent)               │
│   OpenAI/          │                  │                           │
│   Google/          ▼                  ▼                           │
│   DeepSeek/    ┌────────┐        ┌──────────┐                    │
│   Ollama...)   │ chunk  │───────▶│ text_    │                    │
│                │ normal │        │ delta    │                    │
│                │ izer   │        │ thinking │                    │
│                └────────┘        │ tool_*   │                    │
│                                  └──────────┘                    │
│  ┌──────────┐  ┌───────────┐     ┌──────────┐                   │
│  │Permission│◀─│wrapTool   │────▶│JSONL     │  Append-only      │
│  │Engine    │  │Execute()  │     │Store     │  truth            │
│  │park/resume│ │           │     └──────────┘                   │
│  └──────────┘  └───────────┘                                    │
│       ▲              │                                           │
│       │ pause/       │ tool impl (Bash, Write, Read, etc.)       │
│       │ resume        ▼                                          │
│  ┌─────────┐     ┌──────────┐     ┌───────────┐                 │
│  │Stream   │     │Artifact  │     │Telemetry  │                 │
│  │Watchdog │     │Derivation│     │Recorder   │                 │
│  └─────────┘     └──────────┘     └───────────┘                 │
│                                                                  │
│  ┌──────────────────────────────────────────────────────┐        │
│  │ Bots / Network: proxy dispatcher, bot bridges         │        │
│  │ (Telegram/Feishu/Discord/DingTalk/QQ/WeChat)          │        │
│  └──────────────────────────────────────────────────────┘        │
└──────────────────────────────────────────────────────────────────┘
```

六大维度叠加后的难点：

| 维度 | 难点 |
|------|------|
| **Streaming 模型/后端** | Vercel AI SDK 的 `fullStream` 产出多种 chunk type（`text-delta`、`reasoning`、`tool-call`、`tool-result`），Maka 需要将这些规范化为自己的 `SessionEvent` 联合类型。不同 Provider 的 chunk 格式不同（如 Anthropic 的 `reasoning-delta` vs OpenAI 的 `text-delta`），`handleStreamChunk` 需要用防御性的字段回退（`chunk.text ?? chunk.textDelta ?? chunk.delta`）来适应 SDK 演进。 |
| **Tool Calls + Permission** | 工具调用本身是异步的（Bash 可能跑 2 分钟），而 AI SDK 的 `streamText` 是一个连续循环。Maka 在 `wrapToolExecute` 中插入 permission 门控——`prompt` 时需要 `await parked`（挂起整个 stream 等用户决策），同时必须 `pause/resume` StreamWatchdog，否则 idle timeout 会错误触发。 |
| **Permissions** | 三层判断：① `@maka/core` 的纯函数 `preToolUse()` 给出 allow/block/prompt；② `PermissionEngine` 管理 turn-scoped `remembered` 集合和 `parked` Promise 注册表；③ `AiSdkBackend.wrapToolExecute` 根据决策合成 tool_result 或等待用户输入。`permissionRequired: false` 的工具（Read/Glob/Grep）跳过整个引擎。 |
| **Telemetry / Cost** | LLM 调用和工具调用的遥测记录是 fire-and-forget（`queueMicrotask`），但成本计算需要先有 token usage。`AiSdkBackend` 在 stream 结束后 `await result.usage` 再调用 `recordLlmCall`，而 `recordLlmCall` 内部再次调用 `computeCost` 基于 `builtin-pricing.ts` 的硬编码定价表。 |
| **Bots / Network** | Bot 桥接（Telegram/Feishu/Discord 等）通过 `proxiedFetch` 统一走代理层。代理可以是 HTTP/HTTPS 或 SOCKS5，通过 `undici` 的 `ProxyAgent` 或自定义 `Agent`（SOCKS5）实现。`bypass-matcher.ts` 支持 CIDR、通配符域名 bypass。 |
| **交互正交性** | 上述维度彼此独立但共享同一 session：一个 LLM stream 可能触发 N 个 tool call，每个 tool call 可能被 permission 挂起，期间 bot bridge 可能同时收到新消息并创建新 turn。`ActiveSession.activeStreams` 计数器 + `activeTurnIds` Set 保证并发安全。 |

---

## 3. design_approach — 协作架构

### 3.1 核心数据流

```
Electron Main (caller)
  │
  ├─ SessionStore (JSONL, @maka/storage)
  │
  ├─ BackendRegistry ──▶ factory(kind, ctx) ──▶ AgentBackend
  │     ├─ 'ai-sdk'  → AiSdkBackend
  │     └─ 'fake'    → FakeBackend
  │
  └─ SessionManager
       ├─ sendMessage()   → AsyncIterable<SessionEvent>
       ├─ stopSession()   → backend.stop('user_stop')
       ├─ respondToPermission() → backend.respondToPermission(decision)
       ├─ retryTurn() / regenerateTurn() / branchFromTurn()
       └─ recoverInterruptedSessions()
```

### 3.2 AiSdkBackend 内部架构

```
send(input)
  ├─ lazy import('ai') → { streamText, stepCountIs }
  ├─ modelFactory({ connection, apiKey, modelId }) → LanguageModelV2
  ├─ tools: MakaTool[] → wrapToolExecute → ai-sdk tools dict
  │     └─ each tool.execute(args, ctx):
  │          1. write ToolCallMessage → JSONL
  │          2. emit ToolStartEvent → queue
  │          3. PermissionEngine.evaluate(...)
  │             ├─ allow: run impl → write ToolResultMessage → emit ToolResultEvent
  │             ├─ block: write synthetic error ToolResultMessage
  │             └─ prompt: emit PermissionRequestEvent → await parked
  │                    ├─ allow: run impl (as above)
  │                    └─ deny: write synthetic "用户已拒绝" ToolResultMessage
  │          4. recordToolArtifactsSafely (fire-and-forget)
  │          5. recordToolInvocation (telemetry)
  │
  ├─ background pump: streamText({...}) → fullStream → handleStreamChunk → queue
  │     ├─ StreamWatchdog: connect 30s / idle 120s, pause on permission wait
  │     ├─ text-delta → TextDeltaEvent
  │     ├─ reasoning-delta → ThinkingDeltaEvent
  │     ├─ finish → TextCompleteEvent (when pump sees accumulated text)
  │     └─ usage → TokenUsageEvent + persist TokenUsageMessage → JSONL
  │
  └─ yield from queue (AsyncEventQueue<SessionEvent>)
```

### 3.3 关键设计决策

1. **JSONL 是唯一 truth**：`tool_call` 消息**先于**permission 写入 JSONL（`ai-sdk-backend.ts:548-558`）。即使进程崩溃，`materializer.ts` 的 `toolActivityFromPair` 也能正确将 orphan tool_call 显示为 `status: 'interrupted'`。

2. **PermissionEngine 是 turn-scoped**：每个 turn 有独立的 `remembered` Set（`permission-engine.ts:42`），`endTurn()` 会 reject 所有未决的 parked Promise。

3. **Backend 实例与 session header 绑定**：`updateSession` 检测 `backend`/`llmConnectionSlug`/`model` 变更后调用 `disposeBackend()`，下次 turn 重新构建（`session-manager.ts:191-205`）。

4. **FakeBackend 是确定性测试桩**：`fake-backend.ts` 产生固定的文本 + 9 字符分片 + 45ms 延迟，证明整个 JSONL → stream → renderer 链路畅通。

5. **Subagent 扇出限制**：`MAX_ACTIVE_SUBAGENT_TOOLS_PER_TURN = 5`，超过限制返回合成错误（`ai-sdk-backend.ts:154`）。

---

## 4. code_walkthrough — 关键类/函数/文件

### 4.1 `session-manager.ts` — SessionManager

- **`sendMessage()`** (line 299-431)：入口方法，异步生成器 `AsyncIterable<SessionEvent>`。执行顺序：
  1. 读 header → 写 UserMessage 到 JSONL → 写 turn_state(running)
  2. 锁 `connectionLocked` → 构建/复用 `AgentBackend`
  3. `active.activeStreams += 1` → `for await (ev of backend.send(...))` → yield ev
  4. finally：`activeStreams -= 1` → 更新 `lastMessageAt`/`hasUnread`/status
- **`ensureActive()`** (line 535-561)：懒构建 ActiveSession，缓存 header，注册 backend。
- **`recoverInterruptedSessions()`** (line 155-184)：从 JSONL 恢复 running 状态的 turn 为 failed。
- **`stopSession()`** (line 433-459)：调用 `backend.stop('user_stop')`，将所有 `activeTurnIds` 加入 `stoppedTurnIds`，写 abort SystemNote。

### 4.2 `ai-sdk-backend.ts` — AiSdkBackend

- **`send()`** (line 264-528)：整个 Agent 循环的入口。关键步骤：
  - 解析 model（调用 `modelFactory`），失败则产出 `ErrorEvent + CompleteEvent(stopReason: 'error')`
  - 动态 `import('ai')` 获取 `streamText` + `stepCountIs`
  - 包装所有 `MakaTool` 为 ai-sdk 格式（`wrapToolExecute` 注入 permission gating）
  - 后台 pump：`streamText()` → `for await (chunk of result.fullStream)` → `handleStreamChunk()`
  - 结束后写入 `AssistantMessage`、`TokenUsageMessage`，产出 `CompleteEvent`
  - fire `recordLlmCall()` 遥测
- **`wrapToolExecute()`** (line 534-796)：permission gating 的 seam。
  - `permissionRequired === false` → 直接运行 impl（Read/Glob/Grep）
  - `PermissionEngine.evaluate()` → allow/block/prompt 分支
  - prompt 分支：`watchdog.pause()` → `await parked` → `watchdog.resume()` → 处理决策
  - 运行 impl → `coerceResultContent()` → 写 JSONL + emit 事件
  - `recordToolInvocation()` + `recordToolArtifactsSafely()`
- **`handleStreamChunk()`** (line 802-868)：SDK chunk → SessionEvent 的规范化层。处理 `text-delta`、`reasoning`/`reasoning-delta`、`error` 等类型，忽略 `tool-call`/`tool-result`（因为 `wrapToolExecute` 已经 emit 过了）。
- **`repairMakaToolCall()`** (line 1143-1165)：当模型请求的工具名不在可用工具集中时，尝试大小写修复；失败则路由到 `INVALID_TOOL_NAME` 工具。

### 4.3 `permission-engine.ts` — PermissionEngine

- **`evaluate()`** (line 127-187)：调用 `preToolUse()`（core 层纯函数），分别处理 allow / block / prompt 三种结果。prompt 时创建 parked Promise 并注册到 `state.parked` Map。
- **`recordResponse()`** (line 197-222)：从 `parked` Map 中取出请求，resolve Promise，如果 `rememberForTurn` 则把 `scopeKey` 加入 `remembered` Set。
- **`endTurn()`** (line 111-120)：reject 所有未决的 parked Promise。

### 4.4 `builtin-tools.ts` — 内置工具

| 工具 | 权限 | 沙箱措施 |
|------|------|---------|
| `Bash` | permissionRequired: true | `resolveWritableInsideCwd` + 10MB 输出硬上限 + timeout + AbortSignal |
| `Read` | permissionRequired: false | `resolveExistingInsideCwd` + 必须相对路径 |
| `Write` | permissionRequired: true | `resolveWritableInsideCwd` + 必须相对路径 |
| `Edit` | permissionRequired: true | `resolveExistingInsideCwd` + old_string 唯一性检查 |
| `Glob` | permissionRequired: false | 上限 200 文件 + 禁止绝对路径/`..` |
| `Grep` | permissionRequired: false | 通过 `rg` 子进程 + `resolveExistingInsideCwd` |

### 4.5 `materializer.ts` — 消息物化

- **`materializeSession()`** (line 70-136)：两遍遍历 JSONL，第一遍建立 `resultsByToolUseId` / `decisionsByToolUseId` 索引，第二遍产出 `ChatItem[]`。Orphan `tool_call`（无 `tool_result`）→ status `interrupted`。
- **`applyAppendedMessage()`** (line 186-248)：增量更新 ChatItem[]。`tool_result` 消息按 `toolUseId` 定位并原地更新 status。
- **`setToolStatus()`** (line 259-268)：流式更新工具状态（如 `running` → `completed`）。

### 4.6 `stream-watchdog.ts` — StreamWatchdog

- 两阶段超时：`connect`（首个事件到达前，默认 30s）和 `idle`（事件间隔，默认 120s）。
- `pause()/resume()` 用于 permission 等待期间，避免 idle timeout 错误触发。
- 内部使用可注入的 `setTimer/clearTimer`，方便测试。

### 4.7 `tool-output-delta.ts` — 工具输出增量

- `createToolOutputDeltaEmitter()` 维护 `stdout`/`stderr` 两个缓冲区。
- 按行（`\n`）分片推送 `ToolOutputDeltaEvent`。
- 跨写入缓冲敏感信息（如 secret key 被分成两个 chunk 到达），最终输出时仍被 `redactSecrets` 统一脱敏。
- 每个 chunk 有单调递增的 `seq` 编号。

### 4.8 `tool-artifacts.ts` — 工件派生

- `deriveToolArtifactCandidates()` 从 `Write`/`Edit`/`Bash` 的输出自动派生 `ArtifactCandidate`。
- `Bash` 工具通过解析命令中的 `>` 重定向符号来推断输出文件路径。
- `recordToolArtifactsSafely()` 是 fire-and-forget，失败通过 `onWarning` 回调通知。

### 4.9 `model-factory.ts` — 模型工厂

- `getAIModel()` 根据 `LlmConnection.providerType` dispatch 到不同的 AI SDK provider：
  - `anthropic` / `kimi-coding-plan` / `claude-subscription` → `createAnthropic().chat()`
  - `codex-subscription` → `createOpenAI().responses()`
  - `openai` → gpt-5 用 `responses()`，其他用 `chat()`
  - `google` → `createGoogleGenerativeAI().chat()`
  - `deepseek` / `moonshot` / `zai-coding-plan` / `ollama` / `openai-compatible` → `createOpenAICompatible().chatModel()`
- subscriber 特有 header：`claude-subscription` 注入 `x-app: cli`、`User-Agent: claude-cli/2.1.88`、beta features flag。

### 4.10 `model-fetcher.ts` — 模型列表拉取

- `fetchProviderModels()` 根据 provider 协议调用相应 API：
  - Anthropic → `/v1/models`
  - OpenAI → `/models`
  - Google → `/v1beta/models?key=...`
  - Ollama → `/api/tags`
- 全部通过 `proxiedFetch()` 走代理层。

### 4.11 `telemetry/` — 遥测与成本

- **`builtin-pricing.ts`**：硬编码 20 款模型的 `inputUsdPer1M` / `outputUsdPer1M` / `cacheReadUsdPer1M` / `cacheWriteUsdPer1M`。
- **`cost.ts`**：`computeCost()` 按 token / 1M × 单价 计算输入、输出、缓存读写四项成本。
- **`pricing.ts`**：`buildPricingLookup()` 合并 builtin 定价与用户覆盖。
- **`record-llm-call.ts`** / **`record-tool-invocation.ts`**：`queueMicrotask` 包装的持久化记录，失败只 log。

### 4.12 `network/` — 网络代理

- **`proxy-env.ts`**：`getEnvWithProxy()` 设置 `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` 环境变量。
- **`proxy-dispatcher.ts`**：`buildProxyDispatcher()` HTTP/HTTPS 用 `ProxyAgent`，SOCKS5 用自定义 `Agent` + `SocksClient` + TLS 升级。
- **`proxy-parser.ts`**：`parseProxyConfig()` 从持久化配置解析 ProxySettings。
- **`bypass-matcher.ts`**：`matchesBypassList()` 支持通配符 `*`、`*.domain`、CIDR。
- **`active-proxy-state.ts`**：单例 `setActiveProxy()/resolveActiveProxy()`。
- **`proxy-test.ts`**：`testProxyConnection()` 通过代理请求 `icanhazip.com` 验证连通性。

### 4.13 `bots/` — 机器人桥接

- **`bot-registry.ts`**：`BotRegistry` 管理多平台桥接生命周期，串行化 `applySettings` 操作。
- **`base-adapter.ts`**：`BaseBotAdapter` 抽象类，统一 `start/stop/isRunning/getStatus/emitIncomingMessage`。
- **`simple-bridge.ts`**：`SimpleBotBridge` 实现了 Telegram（长轮询 `getUpdates`）和 Feishu（tenant_access_token 验证）。Telegram 发送支持 UTF-16 分片、429 重试、ephemeral TTL 自删除、typing indicator。
- **`bot-test.ts`**：`testBotChannel()` 分平台验证凭据。

---

## 5. flows — 关键调用链

### 5.1 send message（发送消息链路）

```
用户输入
  │
  ▼
SessionManager.sendMessage(sessionId, { turnId, text })
  │
  ├─ 1. store.readHeader(sessionId)       → 获取 backend kind + permissionMode
  ├─ 2. store.appendMessage(userMsg)      → JSONL 写 UserMessage
  ├─ 3. store.updateHeader({connectionLocked: true})
  ├─ 4. ensureActive(sessionId, header)   → BackendRegistry.build(kind, ctx) → AgentBackend
  ├─ 5. for await (ev of backend.send({turnId, text, context}))
  │     └─ yield ev                       → 逐事件 yield 给 caller
  └─ 6. [finally] store.updateHeader({lastMessageAt, hasUnread, status})
```

### 5.2 model stream（模型流式链路）

```
AiSdkBackend.send({ turnId, text, context })
  │
  ├─ modelFactory({ connection, apiKey, modelId }) → LanguageModelV2
  ├─ import('ai') → { streamText, stepCountIs }
  ├─ build aiSdkTools dict (wrapToolExecute per tool)
  ├─ materializePriorMessages(context) → ai-sdk messages[]
  │
  ├─ background pump:
  │   streamText({ model, messages, tools, stopWhen: stepCountIs(50), ... })
  │     └─ for await (chunk of result.fullStream)
  │          └─ handleStreamChunk(chunk) → queue.push(SessionEvent)
  │            ├─ 'text-delta'     → TextDeltaEvent
  │            ├─ 'reasoning'      → ThinkingDeltaEvent
  │            ├─ 'tool-call'      → IGNORE (emitted by wrapToolExecute)
  │            ├─ 'tool-result'    → IGNORE (emitted by wrapToolExecute)
  │            └─ 'error'          → ErrorEvent
  │   after stream ends:
  │     ├─ write AssistantMessage → JSONL
  │     ├─ emit TextCompleteEvent
  │     ├─ await result.usage → write TokenUsageMessage → JSONL → emit TokenUsageEvent
  │     └─ emit CompleteEvent(stopReason)
  │
  └─ yield from queue (AsyncEventQueue)
```

### 5.3 tool call + permission（工具调用 + 权限链路）

```
streamText 自动调用 tool.execute(args, ctx)
  │
  ▼
wrapToolExecute(tool, turnId, queue)(args, ctx)
  │
  ├─ 1. write ToolCallMessage → JSONL
  ├─ 2. emit ToolStartEvent → queue
  │
  ├─ 3. if permissionRequired === false: 跳过权限，直接走步骤 4
  │
  ├─ 3a. PermissionEngine.evaluate({ toolName, args, mode })
  │     ├─ kind='allow'    → 继续步骤 4
  │     ├─ kind='block'    → write synthetic error ToolResultMessage → return errorReturn(reason)
  │     └─ kind='prompt'   → emit PermissionRequestEvent → queue
  │                           watchdog.pause()
  │                           await parked  ←── 挂起，等待用户响应
  │                           watchdog.resume()
  │                           ├─ decision='allow' → 继续步骤 4
  │                           └─ decision='deny'  → write synthetic ToolResultMessage → return errorReturn
  │
  ├─ 4. run tool.impl(args, { cwd, abortSignal, emitOutput })
  │     ├─ 成功 → coerceResultContent() → write ToolResultMessage → JSONL → emit ToolResultEvent
  │     └─ 失败 → write synthetic or terminal-failure ToolResultMessage → JSONL → emit ToolResultEvent
  │
  ├─ 5. recordToolArtifactsSafely(deriveToolArtifactCandidates) → recorder (fire-and-forget)
  └─ 6. recordToolInvocation({ status, durationMs, bytesIn, bytesOut }) → telemetry
  │
  └─ return result (back to ai-sdk, which may trigger next tool call or text completion)
```

### 5.4 artifact materialization（工件物化链路）

```
ToolResultMessage
  │
  ▼
deriveToolArtifactCandidates({ toolName, args, result, cwd })
  │
  ├─ Write tool → 取 result.path 或 args.path → ArtifactCandidate(kindForPath)
  ├─ Edit tool  → 取 args.path + old_string + new_string → ArtifactCandidate(kind='diff')
  └─ Bash tool  → 解析 command 中的 '>' stdout 重定向 → ArtifactCandidate
  │
  ▼
recordToolArtifactsSafely(input, recorder, onWarning)
  │
  └─ recorder({ candidates }) → 由 desktop main 层实现文件持久化
```

### 5.5 bot bridge（机器人桥接链路）

```
BotRegistry.applySettings(settings)
  │
  ├─ reconcileOne(platform, channelSettings)
  │     ├─ !enabled → stop existing bridge
  │     ├─ !isImplemented → scaffoldStatus
  │     └─ implemented → stop old, build new, wire listeners, start()
  │
  ▼
SimpleBotBridge.start() [Telegram]
  │
  ├─ telegramApi(getMe) → 验证 token, 填充 identity
  ├─ running=true, readiness='credentials_valid'
  └─ pollTelegram() loop:
       ├─ telegramApi(getUpdates, { offset, timeout: 15s })
       ├─ 通过 allowedUserIds 白名单过滤
       └─ emitIncomingMessage({ platform, userId, chatId, text, ... })
          └─ → onIncomingMessage callback (desktop main)
              └─ → SessionManager.sendMessage(sessionId, { text })
                  └─ → AiSdkBackend.send() → yield SessionEvent
                      └─ → callback → BotRegistry.sendMessage(platform, chatId, text)
                          └─ → SimpleBotBridge.sendMessage()
                              ├─ splitForTelegram (UTF-16 分片)
                              ├─ telegramApi(sendMessage) per chunk
                              ├─ 429 retry (exactly 1 retry, clamped to [1s, 30s])
                              └─ ephemeralTtlMs → setTimeout deleteMessage
```

### 5.6 telemetry / cost（遥测/成本链路）

```
AiSdkBackend.send() — pump finally 块
  │
  ├─ recordLlmCall({
  │     sessionId, turnId, connectionSlug, providerId, modelId,
  │     inputTokens, outputTokens, totalTokens, latencyMs, status
  │   })
  │     └─ queueMicrotask:
  │          ├─ computeCost({ inputTokens, outputTokens }, pricing)
  │          │     └─ builtinPricing[providerId:modelId] 定价表
  │          └─ repo.insertLlmCall({ ..., costUsd, date, ts })
  │
  ▼
AiSdkBackend.wrapToolExecute() — after impl succeeds or fails
  │
  └─ recordToolInvocation({
        sessionId, turnId, toolCallId, toolName, providerId, modelId,
        durationMs, status, argsSummary, bytesIn, bytesOut
      })
        └─ queueMicrotask:
             └─ repo.insertToolInvocation({ ..., date, ts })
```

---

## 6. tests — 已有覆盖与缺口

### 6.1 已有测试（12 个文件）

| 测试文件 | 覆盖范围 |
|----------|---------|
| `session-manager.test.ts` (787 行) | permission mode 变更、active stream 并发、retry/regenerate/branch、stop、recoverInterruptedSessions、headerToSummary |
| `ai-sdk-backend.test.ts` | tool execution、stream normalization、repairMakaToolCall、合成错误处理 |
| `permission-engine.test.ts` (367 行) | allow/block/prompt 三种路径、rememberForTurn、endTurn reject、idempotent beginTurn |
| `builtin-tools.test.ts` | Bash/Read/Write/Edit/Glob/Grep 的 cwd sandbox + 边界条件 |
| `stream-watchdog.test.ts` (139 行) | connect timeout、idle timeout after activity、pause/resume、stop cancels timer |
| `tool-output-delta.test.ts` (131 行) | seq 单调递增、stream 标签、secret redaction、跨 chunk 脱敏、chunk 边界强制、flush |
| `tool-artifacts.test.ts` | Write/Edit/Bash artifact derivation、extractStdoutRedirectPath |
| `materializer.test.ts` | materializeSession、applyAppendedMessage、orphan tool_call → interrupted、setToolStatus |
| `model-fetcher.test.ts` | 各 provider 模型列表获取 |
| `async-queue.test.ts` | AsyncEventQueue 的 push/close/drain 语义 |
| `claude-subscription-runtime.test.ts` | Claude subscription 的 model factory 和 headers |
| `pi-agent-backend.test.ts` | PI agent backend 适配（另一个 backend 实现） |

### 6.2 缺口

1. **AiSdkBackend 端到端流测试**：测试缺少真实 `streamText` mock 下的完整 send → stream → tool call → permission → resume 流程。现有 `ai-sdk-backend.test.ts` 主要测 `handleStreamChunk` 和 `wrapToolExecute` 独立行为。
2. **StreamWatchdog 与真实 backend 的集成测试**：独立测试完善，但缺少 watchdog 在 `send()` 的 background pump 中被 pause/resume 的异步竞态测试。
3. **FakeBackend 与 SessionManager 的行为一致性测试**：`fake-backend.ts` 没有独立测试文件，仅在 `session-manager.test.ts` 中通过 `TestBackend` 间接测试。
4. **Bot bridge 端到端测试**：`simple-bridge.ts` 的 poll loop、429 重试、UTF-16 分片有良好的单元可测性（`__TEST__` export），但没有 `bot-registry.test.ts` 验证 settings apply → reconcile → stop → start 的串行化行为。
5. **Network proxy 集成测试**：`proxy-dispatcher.ts`、`bypass-matcher.ts` 缺少测试文件。
6. **Telemetry 端到端**：`recordLlmCall` 和 `recordToolInvocation` 缺少单元测试（仅靠 runtime 中 fire-and-forget 调用 + console.error 兜底）。
7. **Permission engine 与 real backend 的异步竞态**：`parked.resolve()` 后 `wrapToolExecute` 恢复执行的路径没有覆盖。

---

## 7. risks — 风险矩阵

| 风险 | 严重度 | 详情 |
|------|--------|------|
| **Provider 抽象泄漏** | 🔴 高 | `model-factory.ts` 中 `codex-subscription` 走 `openai.responses()` 而非 `chat()`，`claude-subscription` 注入特殊 headers + beta flags。新增 Provider 需要理解 AI SDK 的内部 API 语义，skill 层无法完全屏蔽 Provider 差异。`handleStreamChunk` 用 `chunk.text ?? chunk.textDelta ?? chunk.delta` 回退链防御 SDK chunk 格式变化，但新 Provider 可能引入未知 chunk type。 |
| **Tool 安全** | 🔴 高 | `builtin-tools.ts` 的 cwd sandbox 依赖 `fs.realpath` 解析符号链接，但 `resolveWritableInsideCwd` 只检查最终路径是否在 cwd 内——如果路径经过 `..` 跳出再进入，仍可能创建文件。此外，`Grep` 工具通过子进程 `rg` 执行，目标路径虽经 `resolveExistingInsideCwd` 检查，但 `rg` 的 glob 参数可能绕过限制。 |
| **Proxy/Network** | 🟡 中 | `proxiedFetch` 用 `undici` 的 dispatcher 机制，SOCKS5 自定义 Agent 手动处理 TLS 升级（`proxy-dispatcher.ts:49-66`）。如果 `undici` 或 `socks` 库更新 API，可能导致连接失败。`bypass-matcher.ts` 的 CIDR 匹配只支持 IPv4。 |
| **FakeBackend 行为漂移** | 🟡 中 | FakeBackend 不是 AiSdkBackend 的 thin wrapper，而是完全独立的实现（`fake-backend.ts` 仅 55 行）。它不经过 PermissionEngine、不写 ToolCallMessage/TokenUsageMessage、不触发 telemetry。如果 SessionManager 或 UI 层依赖了 AiSdkBackend 独有的事件类型（如 `tool_start`、`token_usage`），FakeBackend 不会产生它们。 |
| **Bot side effect** | 🟡 中 | Bot 桥接在 `emitIncomingMessage` → `SessionManager.sendMessage` → `AiSdkBackend.send` → 生成回复 → `BotRegistry.sendMessage` 这条链路中，如果 agent 回复触发 side-effect 工具（Write/Edit/Bash），错误会同步影响用户工作区。Bot 模式下没有 permission UI（用户不在桌面端），只能依赖 `explore` 或 `execute` 模式的自动决策。 |
| **Permission 挂起超时** | 🟡 中 | `StreamWatchdog.pause()` 在 permission 等待期间暂停 idle timeout，但如果用户永远不响应（如关闭电脑），parked Promise 永远不会 resolve。`endTurn('aborted')` 会 reject 所有 parked Promise，但 reactive 的端（如 bot bridge）可能没有 abort 信号源。 |
| **成本计算硬编码** | 🟢 低 | `builtin-pricing.ts` 的定价表是 2026-05-20 快照，新增模型需要手动更新。`computeCost` 假设 pricing 为 null 时返回 0（`cost.ts:19-20`），意味着未定价模型不会报错。 |
| **JSONL 写入失败丢失事件** | 🟢 低 | `sendMessage` 在 `appendMessage` 失败后仍继续 yield 事件（`session-manager.ts:415-418` 吞掉 header 更新失败）。ui 看到正确的事件流，但 JSONL 历史不完整。下次 reload 时 materializer 会显示 `interrupted` 状态。 |

---

## 8. next_questions — 下一轮精读节点

按可并行维度列出：

### 8.1 AiSdkBackend 深度（节点 A）
- 追踪一次完整的 `streamText` 调用：`model` 如何从 `LanguageModelV2` 变成 ai-sdk 内部格式，`fullStream` 的 chunk 类型的完整枚举。
- `repairMakaToolCall` 被调用的时机——ai-sdk 何时触发 `experimental_repairToolCall`？
- `maxSteps=50` 的 `stepCountIs` 如何与 tool loop 交互，reach cap 后的 grace message 注入逻辑。

### 8.2 Permission Engine vs Core Policy 对照（节点 B）
- 精读 `@maka/core/permission` 中的 `preToolUse()` 策略矩阵，与 `PermissionEngine.evaluate()` 逐行对照。
- `rememberForTurn` 的 scopeKey 生成逻辑（core 层），验证不同 args 的相同 tool 是否会意外共享白名单。
- `explore` 模式下的自动批准规则，与 `ask`/`execute` 的差异。

### 8.3 Bot Bridge 端到端（节点 C）
- 完整追踪一条 Telegram 消息从 `pollTelegram` 到 agent 回复的 `sendMessage` 全链路。
- `BotRegistry.applySettings` 的串行化队列是否正确处理 parallel `applySettings` + `stopAll` 竞态。
- Discord/DingTalk/QQ 桥接的 gateway 实现状态。

### 8.4 工具安全审计（节点 D）
- `builtin-tools.ts` 中 `resolveWritableInsideCwd` 对符号链接 + `..` 路径的 bypass 测试。
- `Bash` 工具的 `emitOutput` 流式输出与 `wrapToolExecute` 的 `AsyncEventQueue` 交互是否存在背压问题。
- `Grep` 的 `rg` 子进程参数注入风险。

### 8.5 Telemetry 完整性（节点 E）
- `LlmCallRecord` 和 `ToolInvocationRecord` 的持久化后端（`TelemetryRepoLite`）的实际实现。
- 成本计算是否包含 reasoning tokens（Claude thinking / OpenAI o-series）的计费。
- `computeCost` 的 `cacheReadCost` / `cacheWriteCost` 是否从 SDK 获取到正确的缓存 token 数。

### 8.6 Network / Proxy 集成（节点 F）
- `proxiedFetch` 在真实 SOCKS5 代理下的端到端验证。
- `bypass-matcher.ts` 的 IPv6 CIDR 支持缺口。
- `active-proxy-state.ts` 单例在 Electron 多窗口场景下的线程安全。

### 8.7 中断恢复与状态一致性（节点 G）
- `recoverInterruptedSessions` 的 `interruptedTurnRecoveries` 逻辑覆盖 `app_restarted` 和 `completed-without-assistant` 两种场景。
- `turn_state` 消息在 JSONL 中的写入时序——是否可能在 crash 时丢失最后一条 turn_state。
- `stoppedTurnIds` Set 在 `sendMessage().finally` 中的清理是否保证不会丢失并发 stop。

---

*报告生成时间: 2026-06-13 | 阅读文件: 约 35 个源文件 + 12 个测试文件*
