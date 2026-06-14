# AI SDK Backend / ModelAdapter / ToolRuntime / RunTrace 现状分析

> 基线: `05ca5a3` → `4dd1bf1`（Extract runtime tool, model, and trace layers）
> 分析日期: 2026-06-14
> 深度档位: maintainer

---

## scope

| 文件 | 行数 | 关键函数/类 | 分析角度 |
|------|------|-------------|----------|
| `ai-sdk-backend.ts` | 655 | `AiSdkBackend.send()`, `wrapToolExecute()`, `materializePriorMessages()`, `cleanupAfterTurn()`, `repairMakaToolCall()` | 主干编排与残留职责 |
| `model-adapter.ts` | 272 | `ModelAdapter.resolveModel()`, `startStream()`, `handleStreamChunk()`, `normalizeAiSdkUsage()` | provider seam 是否稳定 |
| `model-factory.ts` | 146 | `getAIModel()`, `buildProviderOptions()` | 工厂是否独立于 adapter |
| `tool-runtime.ts` | 608 | `ToolRuntime.wrapToolExecute()`, `executeTool()`, `awaitPermissionDecision()`, `deriveToolResultStatus()` | 不变量是否集中 |
| `run-trace.ts` | 152 | `RunTrace.emit()`, `trace phases/event types` | observability 骨架 |
| `permission-engine.ts` | 264 | `PermissionEngine.evaluate()`, `recordResponse()`, `expireRequest()` | 权限引擎与 runtime 解耦 |
| `builtin-tools.ts` | 278 | `buildBuiltinTools()` (Bash/Read/Write/Edit/Glob/Grep) | 工具定义与 runtime 隔离 |
| `ai-sdk-backend.test.ts` | 1340 | 22 tests | 测试覆盖范围 |
| `model-adapter.test.ts` | 143 | 3 tests | adapter 单元测试 |
| `tool-runtime-extraction-contract.test.ts` | 160 | 6 tests | 抽取契约验证 |

---

## problem

AI SDK backend 容易失控的原因:

1. **历史上 `AiSdkBackend` 是一个"大函数"**：`send()` 方法超过 750 行，包含 streamText 调用、chunk type 的 switch-case、tool execute 权限回路、subagent 并发控制、Bash 终端失败处理、usage 标准化、错误分类——所有逻辑耦合在一个类里。

2. **没有 provider seam**：ai-sdk 的 `LanguageModelV2` 类型、`streamText` API、chunk 格式直接散布在 backend 中。切换 provider 或升级 ai-sdk 版本时，变更影响面巨大且不可测试。

3. **tool execution 逻辑分散**：权限评估、watchdog pause/resume、telemetry 记录、结果分类、终端失败处理各自散落在 backend 的不同 private 方法中，很容易出现"某条路径忘记写 tool_result"或"某条路径忘记 resume watchdog"。

4. **observability 无结构**：没有统一的 trace 抽象，debug 只能靠 console.log 和 SessionEvent 推断。

---

## current_design

```
SessionManager
    │
    ├── send(text, attachments, context)
    │
    └── AiSdkBackend (AgentBackend)
            │
            ├── ModelAdapter ─────────────────────
            │   ├── resolveModel()                 │  provider seam
            │   ├── startStream() → streamText()   │  ai-sdk 封装
            │   ├── handleStreamChunk()            │  chunk → SessionEvent
            │   ├── makeErrorEvent()               │  错误标准化
            │   ├── mapFinishReason()              │  finish→stopReason
            │   └── normalizeAiSdkUsage()          │  token 归一化
            │
            ├── ToolRuntime ──────────────────────
            │   ├── wrapToolExecute()              │  permission gating seam
            │   ├── executeTool()                  │  完整执行生命周期
            │   │   ├── append ToolCallMessage     │  §6.2: tool_call 先写
            │   │   ├── emit ToolStartEvent        │
            │   │   ├── PermissionEngine.evaluate()│
            │   │   │   ├── allow → 运行 impl
            │   │   │   ├── block → 合成错误结果
            │   │   │   └── prompt → 暂停 watchdog
            │   │   │       ├── 等待用户决定
            │   │   │       ├── allow → 运行 impl
            │   │   │       └── deny → 合成"用户拒绝"
            │   │   ├── subagent 并发槽位
            │   │   ├── telemetry 记录
            │   │   ├── artifact 记录
            │   │   └── trace 发射
            │   └── writeSyntheticToolResult()
            │
            ├── RunTrace ─────────────────────────
            │   ├── turn_started / model_resolved
            │   ├── model_stream_started / completed / failed
            │   ├── usage_recorded
            │   └── abort_requested
            │   (tool/permission 相位由 ToolRuntime 发射)
            │
            ├── PermissionEngine ─────────────────  纯策略引擎
            ├── StreamWatchdog ────────────────────  流超时守护
            ├── AsyncEventQueue ───────────────────  事件队列 SPSC
            └── ModelFactory (model-factory.ts) ───  provider 实例化
```

### 边界判断

| 边界 | 判断 |
|------|------|
| `AiSdkBackend ↔ ModelAdapter` | **真实 abstraction**。AiSdkBackend 不再直接 import `ai` 包，不再写 `switch (chunk.type)`，不再做 `finiteToken` 归一化。ModelAdapter 是单一职责的 provider seam。 |
| `AiSdkBackend ↔ ToolRuntime` | **真实 abstraction**。`wrapToolExecute()` 在 AiSdkBackend 中是 3 行 shim。所有 permission/watchdog/telemetry/artifact/分类逻辑都在 ToolRuntime 中。 |
| `AiSdkBackend ↔ RunTrace` | **真实 abstraction 但耦合仍存在**。RunTrace 是独立的类，但 AiSdkBackend 通过 `currentRunTrace` 字段持有它，并在 send() 中显式调用 `trace.turnStarted()` 等。ToolRuntime 通过 `getRunTrace` 回调获取 trace 进行 tool/permission 相位记录。耦合可以接受，因为 trace 本来就是 backend 的诊断层。 |
| `ModelAdapter ↔ ai-sdk` | **半抽象**。`await import('ai')` 是动态的，但 `ModelAdapterStreamInput.tools` 仍是 `Record<string, unknown>`，chunk 类型 `AiSdkStreamChunk` 仍在 adapter 文件中定义。如果 ai-sdk v5 改变 chunk 格式，需要修改 adapter 但不影响 backend。 |
| `ToolRuntime ↔ PermissionEngine` | **清晰边界**。ToolRuntime 只调用 `evaluate()`，根据返回的 `kind` 走不同分支。引擎是纯函数包一层状态管理。 |

---

## source_evidence

| 文件 | 函数/位置 | 证据 | 判断 |
|------|-----------|------|------|
| `ai-sdk-backend.ts:218-232` | `constructor` 创建 `ModelAdapter` + `ToolRuntime` | Backend 在构造时组装两个子系统，不内联逻辑 | 抽取成功 |
| `ai-sdk-backend.ts:272` | `this.modelAdapter.resolveModel()` | 不再包含 provider 实例化代码 | 职责移交 |
| `ai-sdk-backend.ts:332-348` | `this.modelAdapter.startStream(...)` | 不再包含 `streamText()` 调用 | 职责移交 |
| `ai-sdk-backend.ts:350-358` | `this.modelAdapter.handleStreamChunk(...)` | 不再包含 `switch (chunk.type)` | 职责移交 |
| `ai-sdk-backend.ts:517-523` | `private wrapToolExecute()` 仅 3 行 | 纯 shim，无业务逻辑 | 抽取成功 |
| `ai-sdk-backend.ts:549-556` | `private writeSyntheticToolResult()` 仅 3 行 | 纯 shim，委托 ToolRuntime | 抽取成功 |
| `ai-sdk-backend.ts:558-561` | `private mapFinishReason()` 仅 3 行 | 纯 shim，委托 ModelAdapter | 抽取成功 |
| `ai-sdk-backend.ts:563-565` | `private makeErrorEvent()` 仅 3 行 | 纯 shim，委托 ModelAdapter | 抽取成功 |
| `ai-sdk-backend.ts:606-614` | `cleanupAfterTurn()` 调用 `this.toolRuntime.resetTurnState()` | per-turn 状态重置在 ToolRuntime | 职责移交 |
| `model-adapter.ts:68-186` | 整个 `ModelAdapter` 类 | 独立文件，可单独测试 | 隔离成功 |
| `model-adapter.ts:239-268` | `normalizeAiSdkUsage()` | 独立导出函数，处理 6 种 token 字段变体 | 归一化集中 |
| `tool-runtime.ts:84-504` | 整个 `ToolRuntime` 类 | 独立文件，权限/watchdog/telemetry 全部在此 | 不变量集中 |
| `tool-runtime.ts:132-460` | `executeTool()` | 完整工具执行生命周期，12 个步骤 | 不变量集中 |
| `tool-runtime.ts:462-487` | `awaitPermissionDecision()` | watchdog pause/resume + timeout + Promise.race | 不变量集中 |
| `tool-runtime.ts:489-499` | `reserveSubagentSlot/releaseSubagentSlot` | subagent 并发控制在 ToolRuntime | 不变量集中 |
| `run-trace.ts:45-133` | 整个 `RunTrace` 类 | 6 个 phase，12 个 event type，独立文件 | 骨架搭建完成 |
| `run-trace.ts:64-68` | `emit()` 中 `try { record(event) } catch {}` | trace 错误不影响主流程 | 诊断隔离 |
| `ai-sdk-backend.test.ts:331-403` | RunTrace 事件顺序测试 | 验证 turn_started → model_resolved → stream_started → usage → completed | 契约锁住 |
| `ai-sdk-backend.test.ts:405-463` | trace recorder 失败测试 | 验证 recorder 抛异常不影响 SessionEvent | 隔离锁住 |
| `tool-runtime-extraction-contract.test.ts:22-45` | `AiSdkBackend keeps only the ai-sdk loop` | 用 `assert.doesNotMatch` 验证 backend 不包含 coerceResultContent, coerceTerminalFailure 等 8 个符号 | 抽取契约锁住 |
| `tool-runtime-extraction-contract.test.ts:92-108` | `ModelAdapter owns provider stream` | 验证 ai-sdk import, streamText, switch chunk.type 都在 adapter 中 | 抽取契约锁住 |

---

## call_flow

以下是用户发送消息到一个完整 turn 结束的逐步链路：

### 1. 用户消息进入

```
SessionManager.send(text, attachments, context)
  → AiSdkBackend.send({ turnId, text, attachments, context })
```

### 2. Turn 初始化

```
AiSdkBackend.send():
  ├─ permissionEngine.beginTurn(turnId)
  ├─ new AbortController()
  ├─ new AsyncEventQueue<SessionEvent>()          ← currentQueue
  ├─ new RunTrace({ sessionId, turnId, ... })     ← currentRunTrace
  └─ trace.turnStarted()
```

### 3. 模型解析

```
  ├─ modelAdapter.resolveModel()
  │   ├─ 检查 API key
  │   └─ modelFactory({ connection, apiKey, modelId })
  │       → getAIModel() 根据 providerType 返回 createAnthropic().chat(modelId) 等
  └─ trace.modelResolved()
```

若解析失败 → `queue.push(errorEvent)` + `queue.push(completeEvent)` + `queue.close()` + `yield* drain(queue)` → 结束。

### 4. 构建工具字典

```
  ├─ 遍历 this.input.tools + buildInvalidMakaTool()
  └─ aiSdkTools[t.name] = {
       description, inputSchema,
       execute: toolRuntime.wrapToolExecute(t, turnId, queue)
     }
```

### 5. 构建消息上下文

```
  ├─ materializePriorMessages(context)  → [{ role: 'user'|'assistant', content }]
  │   (跳过 tool_call/tool_result/permission_decision/token_usage/system_note)
  └─ push({ role: 'user', content: buildUserContent(text, attachments) })
```

### 6. 启动后台 pump

```
  pumpDone = (async () => {
    ├─ new StreamWatchdog({ connectTimeoutMs, idleTimeoutMs })
    ├─ watchdog.start()
    ├─ trace.modelStreamStarted(activeTools)
    ├─ result = await modelAdapter.startStream({
    │     model, messages, tools: aiSdkTools, activeTools,
    │     repairToolCall: repairMakaToolCall,
    │     system, abortSignal
    │   })
    │   → await import('ai') → streamText({ model, messages, tools, ... })
    │
    ├─ for await (chunk of result.fullStream):
    │   ├─ watchdog.markActivity()
    │   └─ modelAdapter.handleStreamChunk(chunk, turnId, assistantMsgId, queue, callbacks)
    │       ├─ 'text-delta' → onText(chunk.text) + queue.push(TextDeltaEvent)
    │       ├─ 'reasoning'/'reasoning-delta' → onThinking(chunk.text) + queue.push(ThinkingDeltaEvent)
    │       ├─ 'tool-call'/'tool-result' → (ai-sdk 内部处理, 不产生 SessionEvent)
    │       ├─ 'error' → queue.push(ErrorEvent)
    │       └─ 其他 → 忽略
```

### 7. Tool call 触发（ai-sdk 内部调用 execute）

当模型决定调用工具时，ai-sdk 在 `streamText` 内部调用 `tools[toolName].execute(args, ctx)`：

```
  toolRuntime.wrapToolExecute(tool, turnId, queue)(args, { toolCallId, abortSignal })
    → toolRuntime.executeTool(tool, turnId, queue, args, ctx):
```

#### 7a. 写入 tool_call

```
      ├─ appendMessage(ToolCallMessage)     ← JSONL 持久化
      └─ queue.push(ToolStartEvent)
```

#### 7b. 权限评估

```
      ├─ if permissionRequired !== false:
      │   └─ verdict = permissionEngine.evaluate({
      │        sessionId, turnId, toolUseId, toolName, args, categoryHint, mode
      │      })
      │
      ├─ verdict.kind === 'block':
      │   ├─ trace permission_failed
      │   ├─ writeSyntheticToolResult(reason)  ← isError: true
      │   ├─ trace tool_failed
      │   └─ return { error: reason }
      │
      ├─ verdict.kind === 'prompt':
      │   ├─ queue.push(PermissionRequestEvent)    ← 渲染层弹出对话框
      │   ├─ trace permission_requested
      │   ├─ response = await awaitPermissionDecision(verdict, turnId)
      │   │   ├─ watchdog.pause()              ← 用户思考不算 idle
      │   │   ├─ Promise.race([verdict.parked, timeout])
      │   │   └─ watchdog.resume()             ← 无论结果
      │   ├─ appendMessage(PermissionDecisionMessage)
      │   ├─ queue.push(PermissionDecisionAckEvent)
      │   ├─ trace permission_decided
      │   │
      │   ├─ if decision === 'deny':
      │   │   ├─ writeSyntheticToolResult('用户已拒绝权限请求')
      │   │   └─ return { error: '...' }
      │   │
      │   └─ (allow 继续往下)
      │
      └─ verdict.kind === 'allow':
          └─ trace permission_decided(allow)
```

#### 7c. Subagent 槽位检查

```
      ├─ reserveSubagentSlot(tool)
      │   └─ 若 categoryHint === 'subagent' 且 >= MAX_ACTIVE_SUBAGENT_TOOLS_PER_TURN(5):
      │       ├─ writeSyntheticToolResult(SUBAGENT_TOOL_LIMIT_MESSAGE)
      │       └─ return { error: '...' }
```

#### 7d. 执行工具实现

```
      ├─ output = createToolOutputDeltaEmitter()  ← streaming stdout/stderr
      ├─ result = await tool.impl(args, {
      │     sessionId, turnId, cwd, toolCallId, abortSignal,
      │     emitOutput: output.emit
      │   })
      ├─ output.flush()
      ├─ content = coerceResultContent(result)     ← string/object → ToolResultContent
      ├─ status = deriveToolResultStatus(content)  ← explore_agent/office_document/rive_workflow → error/aborted
```

#### 7e. 写入 tool_result

```
      ├─ appendMessage(ToolResultMessage{ isError: status !== 'success', content, durationMs })
      ├─ queue.push(ToolResultEvent)
      ├─ recordToolInvocation({ status, durationMs, bytesIn/Out, ... })
      ├─ trace tool_completed
      ├─ recordToolArtifactsSafely(...)           ← 派生 artifact 候选
      └─ return result                             ← 返回给 ai-sdk
```

#### 7f. 错误路径

```
      catch (err):
        ├─ if Bash terminal failure (err.code is number):
        │   └─ coerceTerminalFailure → { kind: 'terminal', cwd, cmd, exitCode, stdout, stderr }
        ├─ else:
        │   └─ writeSyntheticToolResult(formatSyntheticToolErrorText(err))
        ├─ recordToolInvocation({ status: 'error', errorClass: classifyError(err) })
        ├─ trace tool_failed
        └─ return { error: message }
```

### 8. Stream 结束

```
    ├─ 若 finishReason === 'tool-calls' 且 assistantText 为空:
    │   └─ 注入"step cap reached"提示文本
    │
    ├─ 若有 assistantText:
    │   ├─ appendMessage(AssistantMessage{ text, thinking?, modelId })
    │   └─ queue.push(TextCompleteEvent)
    │
    ├─ tokenUsage = normalizeAiSdkUsage(await result.usage)
    │   ├─ trace usageRecorded
    │   ├─ appendMessage(TokenUsageMessage{ input, output, cacheRead, cacheCreation })
    │   └─ queue.push(TokenUsageEvent)
    │
    ├─ finishReason → mapFinishReason → stopReason
    ├─ trace modelStreamCompleted(stopReason)
    ├─ queue.push(CompleteEvent{ stopReason })
```

### 9. 错误/中止路径

```
    catch (err):
      ├─ if aborted:
      │   ├─ queue.push(AbortEvent)
      │   └─ queue.push(CompleteEvent{ stopReason: 'user_stop' })
      └─ else:
          ├─ classifyError → streamErrorClass
          ├─ queue.push(ErrorEvent)
          └─ queue.push(CompleteEvent{ stopReason: 'error' })
    finally:
      ├─ watchdog.stop()
      ├─ recordLlmCall({ sessionId, turnId, providerId, modelId, tokens, latencyMs, status, errorClass })
      └─ queue.close()
```

### 10. 消费与清理

```
  yield* drain(queue)       ← SessionManager 逐事件消费
  pumpDone.catch(() => {})  ← 确保 pump 完成
  cleanupAfterTurn():
    ├─ permissionEngine.endTurn(turnId, 'completed'|'aborted')
    ├─ abortController = null
    ├─ currentQueue = null
    ├─ currentTurnId = null
    ├─ currentRunTrace = null
    ├─ toolRuntime.resetTurnState()    ← activeSubagentToolCount = 0
    └─ aborted = false
```

---

## tests

### 现有测试锁住的行为

| 测试组 | 测试数 | 锁住的行为 |
|--------|--------|------------|
| `AiSdkBackend error surfaces` | 4 | secrets 在 ErrorEvent/synthetic tool error/terminal stdout 中必须 redact；model setup 401 被 generalize 为 "Authentication failed"；超时 error 携带 `reason: 'timeout'` |
| `AiSdkBackend stop` | 1 | `stop()` 必须 reject 所有 parked permission requests，pendingCount 归零 |
| `AiSdkBackend usage telemetry` | 2 | `normalizeAiSdkUsage` 正确处理 `inputTokenDetails.cacheReadTokens/cacheWriteTokens/reasoningTokens`；MockLanguageModelV3 的 Anthropic-format usage → 消息/事件/telemetry 三路输出一致性 |
| `AiSdkBackend RunTrace` | 4 | trace 事件顺序 (turn→model→stream→usage→complete)；phase 正确性；recorder 失败不影响 SessionEvent；permission deny 的 trace (tool_started→permission_requested→permission_decided→tool_failed)；abort trace |
| `AiSdkBackend tool permission category hints` | 8 | permissionRequired=false fast path 产生 tool_call→tool_result 顺序；permission prompt timeout 暂停/恢复 watchdog + 写入 error result；permission deny 记录 decision ack + rememberForTurn；tool failure telemetry 分类 Auth error 并 redact secrets；output delta 在 success/failure 结果之前 flush；categoryHint='subagent' + explore mode 自动 allow；subagent 并发上限 5；explore_agent/OfficeDocument 状态映射到 telemetry status |
| `AiSdkBackend tool-call repair` | 3 | 大小写修复 (`bash` → `Bash`)；不可修复工具路由到 `INVALID_TOOL_NAME` + secrets redact；INVALID_TOOL_NAME 本身不被递归修复 |
| `ModelAdapter stream and error normalization` | 3 | chunk → SessionEvent 映射 (text-delta/textDelta 兼容, reasoning/reasoning-delta, tool-call/tool-result 忽略, error 分类 rate_limit)；classifyError/makeErrorEvent/mapFinishReason 单独测试；`normalizeAiSdkUsage` 支持 `promptTokens/completionTokens` fallback |
| `ToolRuntime extraction contract` | 2 | AiSdkBackend 不包含 `coerceResultContent/coerceTerminalFailure/awaitPermissionDecision/activeSubagentToolCount` 等 8 个符号；ToolRuntime 包含所有 permission/watchdog/telemetry/artifact 调用 |
| `ModelAdapter extraction contract` | 2 | AiSdkBackend 不包含 `await import('ai')/streamText/stepCountIs/switch chunk.type` 等 5 个符号；ModelAdapter 独占这些 |
| `RunTrace extraction contract` | 3 | AiSdkBackend 导入 RunTrace 并持有 `currentRunTrace`；ToolRuntime 只 trace tool/permission 不 trace model；RunTrace 不扩展 SessionEvent，不在 core/events 或 desktop 层出现 |

### 测试覆盖盲区

| 盲区 | 风险 |
|------|------|
| 真实 provider 的 fullStream 行为 | 当前测试全用 `MockLanguageModelV3`，未验证 Anthropic/OpenAI/Google 实际 chunk 形状差异 |
| tool-call repair 在 streamText 中的集成 | `repairMakaToolCall` 单独测试了，但未测试 `experimental_repairToolCall` 在真实 ai-sdk 循环中的行为 |
| 多步 tool calling (maxSteps 触发) | 未测试 ai-sdk 在一次 `send()` 中调用多个 tool 的完整链路 |
| Trace 持久化 | `recordRunTrace` 只是一个回调，没有测试实际写入 JSONL 的行为 |
| permission mode='accept_all' 或 'explore' | 只有 `ask` 和 `explore` 模式被覆盖 |

---

## risks

### 仍可能失败的边界

1. **Provider stream quirks**
   - 不同 provider 的 chunk type 命名可能不一致（已经在 `handleStreamChunk` 中做了 `chunk.text ?? chunk.textDelta ?? chunk.delta` 的兼容，但新 provider 可能引入新的 chunk type）
   - `case 'tool-call'` 和 `case 'tool-result'` 被显式忽略——假设 ai-sdk 内部处理这些。如果 ai-sdk 未来不处理，会导致静默丢失数据
   - **风险等级**: 中。已有兼容逻辑但依赖 ai-sdk 契约。

2. **Tool call ordering**
   - `appendMessage(ToolCallMessage)` 在 tool 执行**之前**写入（§6.2 设计），`appendMessage(ToolResultMessage)` 在**之后**写入
   - 如果 ai-sdk 并行调用多个 tool，消息写入顺序可能与实际执行顺序不一致
   - **风险等级**: 低。ai-sdk 默认串行调用 tool，且有 `MAX_ACTIVE_SUBAGENT_TOOLS_PER_TURN=5` 限制。

3. **Trace write loss**
   - `RunTrace.emit()` 的 `catch {}` 保证不影响主流程，但也意味着如果 `recordRunTrace` 回调本身有问题（如 JSONL 写入失败），trace 会静默丢失
   - 没有 trace 重放或 fallback 机制
   - **风险等级**: 低-中。trace 是诊断层，丢失不致命，但会损害 debuggability。

4. **Cache/reasoning token 损失**
   - `normalizeAiSdkUsage()` 处理了 6 种 token 字段变体，但各 provider 的 `LanguageModelUsage` 格式持续演进
   - 如果新 provider 报告 `reasoningTokens` 在不同路径，可能被 normalizer 遗漏
   - 已在 `TokenUsageMessage` 中做了 `cacheRead/cacheCreation` 的条件写入（`> 0` 才包含），receiver 端可能缺少向后兼容
   - **风险等级**: 中。usage 数据是成本核算的关键输入。

5. **`stopWhen: stepCountIs(maxSteps)` 的边界**
   - 当达到 maxSteps 时，ai-sdk 返回 `finishReason: 'tool-calls'` 且可能没有 assistant text
   - 代码通过注入中文"step cap reached"文本来处理，但这个文本是硬编码的
   - 多语言支持缺失；文本格式假设用户理解"继续"的含义
   - **风险等级**: 低。功能正确，但体验有改进空间。

6. **StreamWatchdog 和 permission 的交互**
   - watchdog 在 permission wait 期间被 `pause()`，但如果 `pauseTarget` 为 null（backend 未设置 `currentWatchdog`），pause/resume 是 no-op
   - 代码中 `getPermissionPauseTarget` 回调返回 `this.currentWatchdog`，存在时序窗口：如果在 `currentWatchdog` 赋值前 tool call 到来，watchdog 不会被暂停
   - **风险等级**: 低。tool call 只能在 streamText 开始后触发，而 watchdog 在 streamText 之前就已创建。

7. **Bash 终端失败的特殊处理**
   - `coerceTerminalFailure` 硬编码检查 `tool.name !== 'Bash'`——如果未来有其他类似 Bash 的工具（如 Docker exec），不会被正确处理
   - `exitCode` 和 `stdout/stderr` 只在 `err.code` 是 number 时才返回 terminal 结构
   - **风险等级**: 低。当前只有 Bash 需要这种处理。

---

## next_actions

### 维护者判断：真实降低复杂度 vs 搬家

| 抽象 | 判断 |
|------|------|
| **ModelAdapter** | ✅ **真实降低复杂度**。将 ai-sdk 的 import/streamText/chunk 处理/usage 归一化集中到 272 行文件，backend 从 750+ 行降到 655 行，且所有 `switch (chunk.type)` 逻辑消失。可独立测试。 |
| **ToolRuntime** | ✅ **真实降低复杂度**。将 12 步 tool 执行生命周期集中到 608 行文件。permission/watchdog/telemetry/artifact/trace 五个关注点统一管理。之前 scattered 的 `activeSubagentToolCount`/`coerceTerminalFailure`/`awaitPermissionDecision` 现在都有单一归宿。 |
| **RunTrace** | ⚠️ **部分搬家，但有价值**。152 行的 trace 类是轻量抽象。虽然本质上是结构化 log，但它建立了 phase/event 类型体系，为后续 observability/replay 提供了类型安全的骨架。当前集成点偏多（AiSdkBackend + ToolRuntime 都有调用），可以接受。 |
| **wrapToolExecute shim** | ⚠️ **搬家**。`AiSdkBackend.wrapToolExecute()` 和 `writeSyntheticToolResult()` 各 3 行的 shim 是纯粹委托。如果 ToolRuntime 直接暴露 `wrapToolExecute` 且 backend 在 build tools dict 时直接引用，可以消除这两个 shim。 |
| **mapFinishReason/makeErrorEvent shim** | ⚠️ **搬家**。同样是 3 行委托。可以等 ModelAdapter 被更多调用者使用时再消除。 |

### 建议的下一步 DAG

```
Phase A: 加固 provider seam
├── A1: 为 google/deepseek/openai 提供商的真实 chunk 添加集成测试
├── A2: 测试 `experimental_repairToolCall` 在 streamText 集成中的行为
└── A3: 验证 model-factory.ts 的 `buildProviderOptions` 透传正确

Phase B: 扩展 RunTrace 为可用的 observability 层
├── B1: 添加 RunTrace JSONL sink（将 trace event 写入 session 目录）
├── B2: 实现 trace replay reader（用于 cost diagnosis）
├── B3: 为每轮 turn 添加 trace summary（total tokens, tool count, latency）
└── B4: 在 desktop UI 添加 trace viewer（可选，Phase B 完成后再做）

Phase C: 消除 shim 和硬编码
├── C1: 移除 AiSdkBackend 的 wrapToolExecute/writeSyntheticToolResult/mapFinishReason/makeErrorEvent shim
├── C2: 将 "step cap reached" 文本移到 i18n 系统
└── C3: 将 Bash 终端失败的 `coerceTerminalFailure` 改为基于工具返回接口而非 `tool.name` 硬编码

Phase D: 多步 tool calling 测试
├── D1: 添加 `send()` 中包含 2+ tool calls 的集成测试（使用真实 MockLanguageModelV3）
├── D2: 验证 tool_call→tool_result 消息顺序在多步场景下正确
└── D3: 验证 maxSteps 边界：刚好达到上限时不丢最后一个 tool result
```

