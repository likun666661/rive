# 第一部分：核心 Agent、Runtime 与 Model Loop

---

## 1. 面临的问题是什么：ADK Go 在 agent/runtime/model loop 这层要解决什么问题？

ADK Go 的 agent/runtime/model loop 层要解决的核心问题是：**在单次用户输入触发的一次 Invocation 中，如何协调 LLM 模型、工具调用、Agent 间转移、流式事件输出与会话状态持久化，形成可复用、可扩展、可观测的执行主干。**

具体而言，它需要统一处理以下场景：

1. **单 Agent LLM 对话**：用户发消息 → LLM 生成回复（可能多次 function call → 新 LLM 调用循环）→ 最终输出。
2. **多 Agent 编排**：Agent A 通过 `transfer_to_agent` 将控制权转移给 Agent B（子 agent / 父 agent / 同级 agent），形成一个 agent 树中的递归执行。
3. **流式（SSE）模式**：LLM 逐 token 产出，部分事件只向前端透传而不写入 session。
4. **Live 双向流**：音频/视频实时双向通信，需要会话恢复（Session Resumption）、转录事件与 tool call 时序对齐。
5. **Human-in-the-Loop (HITL)**：工具调用需要用户确认（`toolconfirmation`），确认后继续或拒绝。
6. **插件系统**：在 run / event / model call / tool call 各阶段注入外部行为，不修改核心代码。
7. **可观测性**：OpenTelemetry span + trace，覆盖 invocation → model call → tool call 全链路。

**参考文件**：
- `agent/agent.go:28-60` — `Agent` 接口定义
- `runner/runner.go:131-268` — 核心 `Run` 入口
- `internal/llminternal/base_flow.go:101-127` — `Flow.Run` 的 step loop

---

## 2. 为什么这是问题：agent execution、LLM events、tool calls、streaming/live run 为什么容易复杂？

### 2.1 状态模型的膨胀

一次简单的 "用户提问 → LLM 回答" 在真实场景下会展开为：

```
用户消息 → [组装 history (N个历史Event)] → [注入 instruction/global instruction]
→ [挂载 tool declaration] → [agent transfer tool 注册] → [BeforeModelCallback]
→ [LLM 调用 (可能 stream 多个 partial event)] → [AfterModelCallback]
→ [解析 function_call → 并行执行 tools → BeforeToolCallback → Tool.Run → AfterToolCallback]
→ [合并 function_response → 新一轮 LLM 调用] → [检查 IsFinalResponse]
→ [如果有 transfer_to_agent → 递归调用子 agent] → [最终输出]

在 Live 模式下，还要叠加：
- 音频 → 转录（InputTranscription partial events）
- 转录未完成时 tool call 的 buffering（避免时序错乱）
- 会话断线重连（Session Resumption handle）
```

每一步都可能失败、被 callback 拦截、或产生 state delta。如果不分层抽象，复杂度会指数增长。

### 2.2 多 Agent 带来的上下文问题

- Agent A 的 function response 对 Agent B 不可见（除非通过 `ConvertForeignEvent` 转为 user content）
- Agent 树中的分支（branch）标识决定了哪些历史 event 属于当前 agent 的上下文（`eventBelongsToBranch`）
- Agent 转移方向受 `DisallowTransferToParent` / `DisallowTransferToPeers` 控制，需要在整个 agent 链路上判断

### 2.3 流式与非流式模式的统一

- 同一个 `model.LLM.GenerateContent(ctx, req, stream bool)` 接口必须同时承载同步调用和 SSE 流
- `StreamingResponseAggregator` 需要处理：文本拼接、流式 function call arg 渐进组装（`PartialArgs` + JSON path）、thought signature 传递
- 部分事件（`Partial: true`）只向前端透传，不持久化到 session service

### 2.4 Event 持久化的边界

ADK 的 `session.Event` 是一个"胖事件"——它既承载 LLM 响应内容，又携带 `EventActions`（state delta、artifact delta、transfer target、tool confirmations）。Runner 需要精确判断：哪些 event 持久化、哪些只 yield、插件是否需要修改 event、何时触发 agent transfer 的副作用。

**参考文件**：
- `session/session.go:92-160` — `Event` 的 full struct
- `internal/llminternal/base_flow.go:528-653` — `runOneStep` 完整流程

---

## 3. 解决思路是什么：它用什么核心抽象、接口、事件流、runner loop 来拆问题？

### 3.1 三层架构

```
┌─────────────────────────────────────────────────┐
│  Runner 层                                       │
│  runner.Run() — 会话生命周期管理                   │
│  • 获取/创建 Session                              │
│  • findAgentToRun() — 基于 history 路由到正确 Agent │
│  • 组装 InvocationContext（artifacts, memory, ...）│
│  • 用户消息持久化 + 插件 callback                   │
│  • 消费 agent.Run() 的事件迭代器                    │
│  • 非 partial event 写入 Session Service          │
│  • RunLive() — 双向流 + 时序缓冲                    │
└──────────────┬──────────────────────────────────┘
               │ ctx (InvocationContext)
┌──────────────▼──────────────────────────────────┐
│  Agent 层                                        │
│  agent.Run() — 单次 agent 调用的生命周期            │
│  • BeforeAgentCallbacks (插件 → 用户)             │
│  • 用户 Run 函数 (llmagent → Flow.Run)            │
│  • AfterAgentCallbacks (插件 → 用户)              │
│  • EndInvocation 提前终止                         │
└──────────────┬──────────────────────────────────┘
               │ Flow.Run() — step loop
┌──────────────▼──────────────────────────────────┐
│  Flow 层 (llminternal)                           │
│  Flow.Run() — 模型调用的 step-based loop          │
│  • runOneStep()                                  │
│    ├─ preprocess (RequestProcessor pipeline)     │
│    ├─ callLLM (before/after model callbacks)     │
│    ├─ postprocess (ResponseProcessor pipeline)    │
│    ├─ finalizeModelResponseEvent                 │
│    ├─ handleFunctionCalls (并行 tool 执行)        │
│    ├─ generateRequestConfirmationEvent (HITL)    │
│    └─ agent transfer (递归 runOneStep)            │
│  • RunLive() — 双向 WebSocket 管理                │
└──────────────┬──────────────────────────────────┘
               │ model.LLM interface
┌──────────────▼──────────────────────────────────┐
│  Model 层                                        │
│  model.LLM.GenerateContent() — iter.Seq2[*LLMResponse, error]  │
│  • gemini.geminiModel — Gemini API 实现           │
│  • Gemini Streaming → StreamingResponseAggregator │
│  • apigee.apigeeModel — Apigee 代理               │
└─────────────────────────────────────────────────┘
```

### 3.2 核心接口

| 接口 | 位置 | 职责 |
|------|------|------|
| `agent.Agent` | `agent/agent.go:28` | `Name()`, `Description()`, `Run(InvocationContext) iter.Seq2[*session.Event, error]`, `SubAgents()`, `FindAgent()` |
| `agent.InvocationContext` | `agent/context.go` | 扩展 `context.Context`，提供 Agent、Session、Artifacts、Memory、RunConfig、EndInvocation |
| `agent.CallbackContext` | `agent/context.go` | ReadonlyContext + State（callback 中可修改 state delta） |
| `agent.ToolContext` | `agent/context.go` | CallbackContext + FunctionCallID + HITL 确认 |
| `model.LLM` | `model/llm.go:26` | `Name()`, `GenerateContent(ctx, *LLMRequest, stream bool) iter.Seq2[*LLMResponse, error]` |
| `session.Session` | `session/session.go:32` | `ID()`, `AppName()`, `UserID()`, `State()`, `Events()` |
| `session.Event` | `session/session.go:92` | 嵌入 `model.LLMResponse` + `EventActions`（StateDelta, TransferToAgent, 等） |
| `agent.LiveSession` | `agent/live.go` | `Send(LiveRequest) error`, `Close() error` — 双向流通信 |

### 3.3 事件流（Event Flow）

```
用户输入 (genai.Content)
    │
    ▼
Runner.Run()
    │
    ├─ sessionService.Get/Create → Session
    ├─ findAgentToRun() → 路由到正确的 Agent
    ├─ NewInvocationContext() → 组装上下文
    ├─ appendMessageToSession() → 用户消息持久化
    ├─ plugin.RunBeforeRunCallback() → 可能 early exit
    │
    ▼
agentToRun.Run(ctx) → iter.Seq2[*session.Event, error]
    │
    ├─ BeforeAgentCallbacks → 可能跳过 agent run
    │
    ▼
llmAgent.run() → Flow.Run()
    │
    ├─ for each step:
    │   ├─ preprocess (13个 RequestProcessor, pipeline 顺序执行)
    │   │   ├─ basicRequestProcessor → 拷贝 GenerateContentConfig
    │   │   ├─ toolProcessor → 展开 Tools + Toolsets
    │   │   ├─ authPreprocessor → (TODO)
    │   │   ├─ RequestConfirmationRequestProcessor → HITL
    │   │   ├─ instructionsRequestProcessor → 注入 instruction + global instruction
    │   │   ├─ identityRequestProcessor → 身份注入
    │   │   ├─ ContentsRequestProcessor → 拼接 conversation history
    │   │   ├─ nlPlanningRequestProcessor → (TODO)
    │   │   ├─ codeExecutionRequestProcessor → (TODO)
    │   │   ├─ outputSchemaRequestProcessor → 结构化输出
    │   │   ├─ AgentTransferRequestProcessor → 注册 transfer_to_agent tool
    │   │   └─ removeDisplayNameIfExists → 清理 display name
    │   ├─ callLLM()
    │   │   ├─ plugin.RunBeforeModelCallback()
    │   │   ├─ BeforeModelCallbacks (用户) → 可拦截跳过 LLM 调用
    │   │   ├─ generateContent() → 带 telemetry span
    │   │   │   └─ model.LLM.GenerateContent(ctx, req, useStream)
    │   │   │       └─ (如果 stream) → StreamingResponseAggregator
    │   │   │           ├─ ProcessResponse → aggregate + yield partial(event)
    │   │   │           └─ Close() → yield final(event)
    │   │   └─ AfterModelCallbacks (插件 + 用户) → 可替换响应
    │   ├─ postprocess (ResponseProcessor pipeline)
    │   ├─ finalizeModelResponseEvent → 创建 session.Event + 设置 Author/Branch/LongRunningToolIDs
    │   ├─ yield modelResponseEvent (Partial 或 non-Partial)
    │   ├─ handleFunctionCalls (并行 goroutine 执行)
    │   │   ├─ BeforeToolCallbacks → 可拦截
    │   │   ├─ tool.Run() → 实际工具执行
    │   │   ├─ AfterToolCallbacks → 可替换结果
    │   │   └─ mergeParallelFunctionResponseEvents → 合并为单个 event
    │   ├─ generateRequestConfirmationEvent → HITL 确认事件
    │   ├─ yield functionResponseEvent
    │   ├─ yield toolConfirmationEvent
    │   ├─ outputSchemaResponse check
    │   └─ 如果 TransferToAgent != ""
    │       └─ nextAgent.Run(ctx) → 递归进入子 agent
    │
    └─ 如果 IsFinalResponse() → break step loop
    │   否则 → 下一轮 step (将 function_response 作为新的 context)
    │
    ▼
Runner.iterator:
    ├─ plugin.RunOnEventCallback() → 可修改/丢弃 event
    ├─ 非 Partial event → sessionService.AppendEvent() 持久化
    └─ yield event 给调用方
```

### 3.4 Runner Loop 的设计要点

Runner 的 run loop（`runner/runner.go:234-266`）是一个 Go 1.23 `iter.Seq2` 消费器：

```go
for event, err := range agentToRun.Run(ctx) {
    // 1. Error → yield and continue (不停止 loop)
    // 2. Plugin OnEventCallback → 可修改/替换/丢弃 event
    // 3. 非 Partial → 持久化到 Session Service
    // 4. yield event 给调用方 (外部可随时停止消费)
}
```

**关键设计决策**：
- **Pull 模型**（`iter.Seq2`）：调用方控制消费节奏，适合前端逐 event 渲染
- **只在 Runner 层持久化**：Agent 层和 Flow 层不关心 session storage，只产出 event
- **Partial event 不持久化**：避免 session 被单 token event 填满
- **Agent transfer 的处理**：在 `runOneStep` 的后处理阶段（`base_flow.go:647`），直接调用 `nextAgent.Run(ctx)` 产生递归子树事件，而非返回给 Runner 重新路由

---

## 4. adk-go 代码怎么落地：关键类型/函数/文件、调用链、测试覆盖、未读风险

### 4.1 目录速查

```
agent/
├── agent.go              — Agent 接口 + Config + New() + 具体 agent struct
├── context.go            — InvocationContext, ReadonlyContext, CallbackContext, ToolContext
├── callback_context.go   — callbackContext 实现 + state delta 追踪
├── live.go               — LiveSession 接口 + LiveRequest/LiveRunConfig
├── run_config.go         — StreamingMode + RunConfig
├── loader.go             — Loader 接口 (root/多 agent 加载)
├── doc.go                — 包文档
├── agent_test.go         — Agent 回调、EndInvocation、WithContext、FindAgent 测试
├── loader_test.go        — Loader 去重测试
└── llmagent/
    ├── llmagent.go       — Config + New() + llmAgent.run() + RunLive() + maybeSaveOutputToState()
    ├── llmagent_test.go  — LLMAgent 测试
    ├── dynamic_events_test.go
    ├── state_agent_test.go
    └── testdata/         — HTTP replay 测试数据

runner/
├── runner.go             — Config + New() + Runner.Run() + Runner.RunLive() + findAgentToRun()
├── runner_test.go        — findAgentToRun / isTransferable / SaveInputBlobs 测试
└── live_runner_test.go   — RunLive Callbacks/EarlyExit/ChronologicalBuffering 测试

model/
├── llm.go                — LLM 接口 + LLMRequest/LLMResponse
├── llm_test.go
├── gemini/gemini.go      — Gemini 实现 + 同步/流式调用 + StreamAggregator
└── apigee/apigee.go      — Apigee 代理模型

internal/llminternal/
├── base_flow.go          — Flow struct + Run()/RunLive()/runOneStep()/callLLM()/handleFunctionCalls()
├── agent.go              — llmAgent 内部 State (Model + Tools + Instruction)
├── basic_processor.go    — basicRequestProcessor → 拷贝 GenerateContentConfig
├── contents_processor.go — ContentsRequestProcessor → 组装 conversation history
├── instruction_processor.go — instructionsRequestProcessor → 注入 instruction + global instruction
├── tools_processor.go    — toolProcessor → 展开 Tools + Toolsets
├── agent_transfer.go     — AgentTransferRequestProcessor → 注入 transfer_to_agent tool
├── stream_aggregator.go  — StreamingResponseAggregator → 流式响应聚合
├── functions.go          — generateRequestConfirmationEvent → HITL 确认
├── other_processors.go   — nlPlanning/codeExecution/auth (TODO stubs)
├── converters/converters.go — Genai2LLMResponse 转换
└── googlellm/            — LiveConnection + variant 检测

internal/context/
├── invocation_context.go — InvocationContext 实现
├── callback_context.go   — CallbackContext + ToolContext 实现
├── readonly_context.go   — ReadonlyContext 实现
└── context_test.go

session/
├── session.go            — Session/State/Event/EventActions 接口与类型
├── service.go            — Service 接口
├── inmemory.go           — InMemory 实现
└── doc.go
```

### 4.2 关键类型索引

| 类型 | 文件:行 | 说明 |
|------|---------|------|
| `agent.Agent` | `agent/agent.go:28` | 所有 agent 的基础接口 |
| `agent.InvocationContext` | `agent/context.go` | 一次 invocation 的上下文 |
| `agent.Config` | `agent/agent.go:62` | 通过 `agent.New()` 创建自定义 agent |
| `agent.RunConfig` | `agent/run_config.go:34` | StreamingMode + SaveInputBlobsAsArtifacts |
| `model.LLM` | `model/llm.go:26` | `GenerateContent(ctx, *LLMRequest, stream) iter.Seq2` |
| `model.LLMRequest` | `model/llm.go:32` | Contents, Config, Tools |
| `model.LLMResponse` | `model/llm.go:42` | Content, Partial, TurnComplete, ErrorCode... |
| `session.Event` | `session/session.go:92` | 嵌入 LLMResponse + EventActions |
| `session.EventActions` | `session/session.go:143` | StateDelta, TransferToAgent, SkipSummarization... |
| `runner.Runner` | `runner/runner.go:116` | 运行时容器：rootAgent, sessionService, pluginManager |
| `llminternal.Flow` | `internal/llminternal/base_flow.go:62` | 请求/响应处理器 + Model + 回调链 |
| `llminternal.State` | `internal/llminternal/agent.go:30` | LLMAgent 内部状态（Model, Tools, Instruction...） |
| `llmagent.Config` | `agent/llmagent/llmagent.go:130` | LLMAgent 创建配置 |
| `llmagent.llmAgent` | `agent/llmagent/llmagent.go:340` | 嵌入 `agent.Agent` + `llminternal.State` |

### 4.3 主执行调用链

```
Runner.New(cfg)
  → parentmap.New(rootAgent)                  // 构建 agent 父子关系图
  → plugininternal.NewPluginManager()

Runner.Run(ctx, userID, sessionID, msg, runCfg)
  → sessionService.Get/Create                 // 获取或创建 Session
  → r.findAgentToRun()                        // 基于 history 选 agent
    → handleUserFunctionCallResponse()         // function_response 路由
    → 倒序扫描 session events                  // 找最近的 transferable agent
  → runconfig.ToContext() / parentmap.ToContext()
  → icontext.NewInvocationContext()           // 组装 InvocationContext
  → r.appendMessageToSession()                // 用户消息持久化
  → pluginManager.RunBeforeRunCallback()      // 可能 early exit
  → agentToRun.Run(ctx)                       // 核心执行
    → llmAgent.run(ctx)                       // llmagent/llmagent.go:361
      → NewInvocationContext (per-agent 重建)
      → Flow.Run(ctx)                         // internal/llminternal/base_flow.go:101
        → for (step loop):
          → runOneStep(ctx)                   // line 528
            → preprocess (pipeline)           // line 656
              → basicRequestProcessor         // GCConfig 克隆
              → toolProcessor                 // 展开 Tools + Toolsets
              → ... (auth, HITL, instructions, identity, contents, nlplanning, codeexec, outputschema, agenttransfer)
            → callLLM(ctx, req, ...)          // line 722
              → plugin.BeforeModelCallback
              → BeforeModelCallbacks (user)
              → generateContent(ctx, model, req, useStream)
                → telemetry.StartGenerateContentSpan
                → model.GenerateContent(ctx, req, useStream)
                  → (stream) StreamingResponseAggregator
                → telemetry.TraceGenerateContentResult
              → AfterModelCallbacks
            → postprocess (ResponseProcessor pipeline)  // line 901
            → finalizeModelResponseEvent()             // line 925
            → yield modelResponseEvent
            → handleFunctionCalls()                    // line 1012
              → 并行 goroutine:
                → BeforeToolCallbacks
                → tool.Run(toolCtx, args)
                → AfterToolCallbacks
              → mergeParallelFunctionResponseEvents
            → generateRequestConfirmationEvent (HITL)
            → yield functionResponseEvent
            → if TransferToAgent != ""
              → nextAgent.Run(ctx)          // 递归 agent transfer
          → if IsFinalResponse() → break
  → for event in iterator:
    → plugin.RunOnEventCallback()
    → 非 Partial → sessionService.AppendEvent()
    → yield to caller
```

### 4.4 已实现 / TODO / 风险

| 状态 | 模块 | 说明 |
|------|------|------|
| ✅ 已实现 | Gemini sync/stream 调用 | `model/gemini/gemini.go` |
| ✅ 已实现 | StreamingResponseAggregator | `internal/llminternal/stream_aggregator.go` |
| ✅ 已实现 | 并行 Tool 执行 | `base_flow.go:1012-1179` (`handleFunctionCalls`) |
| ✅ 已实现 | Agent Transfer (via `transfer_to_agent`) | `internal/llminternal/agent_transfer.go` |
| ✅ 已实现 | Agent 上下文路由 (findAgentToRun + isTransferable) | `runner/runner.go:592-666` |
| ✅ 已实现 | Live 双向流 + 会话恢复 | `base_flow.go:251-526` (`RunLive`) |
| ✅ 已实现 | 时序 buffering (transcription vs tool calls) | `runner/runner.go:459-505` |
| ✅ 已实现 | HITL Tool Confirmation | `internal/llminternal/request_confirmation_processor.go` |
| ✅ 已实现 | OpenTelemetry Tracing | `internal/telemetry/` — GenerateContent + Tool spans |
| ✅ 已实现 | Plugin 系统 (Before/After Run/Model/Tool) | `internal/plugininternal/` |
| ✅ 已实现 | Artifact + Memory 服务 | `runner/runner.go:178-196` |
| ⚠️ TODO   | NL Planning 处理器 | `other_processors.go:25-28` — stub |
| ⚠️ TODO   | Code Execution 处理器 | `other_processors.go:30-33` — stub |
| ⚠️ TODO   | Auth Preprocessor | `other_processors.go:35-38` — stub |
| ⚠️ TODO   | Handle Partial response in model level | `base_flow.go:119-123` — TODO comment |
| ⚠️ TODO   | Output Schema 验证 + Unmarshal | `llmagent/llmagent.go:458` — TODO comment |

### 4.5 测试覆盖

| 测试文件 | 覆盖范围 |
|----------|----------|
| `agent/agent_test.go` | Agent 回调、EndInvocation、FindAgent 树查找 |
| `agent/loader_test.go` | 多 Agent 去重 |
| `agent/llmagent/llmagent_test.go` | LLMAgent 完整流程（HTTP replay） |
| `agent/llmagent/dynamic_events_test.go` | 动态 event 生成 |
| `agent/llmagent/state_agent_test.go` | 状态管理 |
| `agent/llmagent/llmagent_saveoutput_test.go` | OutputKey 保存 |
| `runner/runner_test.go` | findAgentToRun、isTransferable、SaveInputBlobsAsArtifacts、AutoCreateSession |
| `runner/live_runner_test.go` | Live Callbacks、EarlyExit、ChronologicalBuffering |
| `model/gemini/gemini_test.go` | Gemini sync/stream 调用（HTTP replay） |
| `internal/llminternal/base_flow_test.go` | Flow 单步执行 |
| `internal/llminternal/contents_processor_test.go` | 对话历史重组（含 async function response） |
| `internal/llminternal/parallel_function_call_test.go` | 并行 function call |
| `internal/llminternal/handle_function_calls_async_test.go` | 异步 function call |

---

## 5. 主要执行序列（Main Execution Sequence）

### 场景 A：单 Agent，无 Tool，流式响应

```
1. Runner.Run(ctx, userID, sessionID, msg)                    [runner/runner.go:131]
2. r.findAgentToRun(session, msg) → rootAgent                 [runner/runner.go:166]
3. icontext.NewInvocationContext(...)                          [runner/runner.go:198]
4. r.appendMessageToSession(...) → 用户 msg 持久化               [runner/runner.go:206]
5. agentToRun.Run(ctx)                                        [runner/runner.go:234]
  5a. llmAgent.run(ctx)                                       [llmagent/llmagent.go:361]
    5a1. Flow.Run(ctx)                                        [base_flow.go:101]
      5a1a. runOneStep(ctx)                                   [base_flow.go:528]
        • preprocess: basicRequest + instruction + contents   [base_flow.go:656]
        • callLLM: before callbacks + generateContent(stream) [base_flow.go:722]
          • model.GenerateContent(ctx, req, true)             [model/llm.go:28]
            → StreamingResponseAggregator                    [stream_aggregator.go:57]
              → yield partial event (token 1)
              → yield partial event (token 2)
              → ...
              → yield partial event (token N)
              → Close() → yield final event                  [stream_aggregator.go:304]
        • finalizeModelResponseEvent → session.Event          [base_flow.go:925]
        • yield partial events...final event                  [base_flow.go:588-595]
        • handleFunctionCalls → nil (no calls)               [base_flow.go:599]
        • IsFinalResponse() → true → break                    [base_flow.go:116]
6. Runner for-loop:
   每个 partial event → yield (不持久化)                       [runner/runner.go:256]
   最终 non-partial event → sessionService.AppendEvent → yield [runner/runner.go:256-258]
```

### 场景 B：Agent + Tool + Transfer

```
1. Runner.Run() → agentToRun.Run(ctx)
2. Flow.runOneStep(ctx)
   preprocess:
     • AgentTransferRequestProcessor → 注入 transfer_to_agent tool [agent_transfer.go:69]
   callLLM → LLM returns [function_call: transfer_to_agent, args: {agent_name: "child_agent"}]
   handleFunctionCalls:
     • transfer_to_agent.Run() → set ctx.Actions().TransferToAgent = "child_agent"
   yield functionResponseEvent (with TransferToAgent set)
   检测 ev.Actions.TransferToAgent != ""                       [base_flow.go:639]
   → nextAgent := f.agentToRun(ctx, "child_agent")            [base_flow.go:642]
   → nextAgent.Run(ctx)                                       [base_flow.go:647]
     → 递归进入子 agent 的 Flow.Run()
     → 子 agent 的所有 event 透传给 Runner
```

---

## 6. 深入探索问题（5-10 Deeper Follow-up Questions）

1. **`invocationContext` 的 `EndInvocation` 机制**：`EndInvocation()` 在 `agent/agent.go` 中被调用后，AfterAgentCallbacks 被跳过。但它是全局的——如果在 agent transfer 子链中 `EndInvocation`，父 agent 的 cleanup 如何保证？当前实现在 `agent.go:228-240` 直接 return，这会不会导致父 agent 的 AfterCallbacks 也被跳过？

2. **Agent Transfer 的"反转"逻辑**：Python ADK 的 Runner 在 `_find_agent_to_run` 中有 "自动回弹"逻辑（子 agent 完成后下一条用户消息自动路由回父 agent）。Go 版 `runner.go:654-666` 的 `isTransferableAcrossAgentTree` 仅检查 parent chain 是否禁止转移，但没有实现下一条消息的自动回弹。是否需要补全？

3. **`StreamingResponseAggregator` 的错误处理**：当 `ProcessResponse` 中 `genai2LLMResponse` 返回错误的候选（如 FINISH_REASON_SAFETY），aggregator 的 `aggregateResponse` 如何行为？当前代码 `stream_aggregator.go:89-91` 只处理 `FinishReason != ""` 的情况，但 SAFETY/MAX_TOKENS 等 finish_reason 在 partial token 阶段如何处理？

4. **`ContentsRequestProcessor` 中的 Branch 前缀匹配**：`eventBelongsToBranch` 使用字符串前缀匹配（`strings.HasPrefix`），但分支名用 `.` 分隔时可能误匹配（该函数已经用 `+ "."` 来避免）。在深层 agent 树（超过 3 层）中，这个实现是否足够健壮？是否需要改用 `[]string` 类型的分支？

5. **并行 Tool 调用中的 Error 传播**：`handleFunctionCalls` 用 `sync.WaitGroup` 管理并行 tool 调用，但各个 goroutine 的 error 只被记录到每个 event 的 content 中（`result["error"]`），不返回 Go error。如果多个 tool 都 fail，`mergeParallelFunctionResponseEvents` 里的 `mergeEventActions` 如何合并多个 error？外层 Flow 是否应该感知到部分失败？

6. **`RunLive` 的 Reconnection 机制细节**：`base_flow.go:497-501` 中，当 `errChan` 收到可恢复错误（broken pipe, EOF, GoAway...），设置 `reconnect = true` 并 `break` 出 select 重新连接。但 `outputConn` 和 `eventsChan` 的 goroutine 如何确保已正确关闭？在 race condition 下，新旧连接的事件会不会混合？

7. **`PluginManager` 与 User Callback 的执行顺序**：`callLLM` 中 plugin 回调先于用户回调执行（`base_flow.go:724-742`）。如果 plugin 返回非 nil response 短路了用户回调，用户如何知晓？是否存在 debug flag 或日志策略来追踪这类短路？

8. **Session Event 的 `LongRunningToolIDs`**：`IsFinalResponse` 中检测 `len(e.LongRunningToolIDs) > 0` 会直接返回 true（`session/session.go:125`），这意味着一旦启用长运行工具，step loop 立即终止。但后续的 function response 通过 Runner 层的 `handleUserFunctionCallResponse` 重新路由。这个"断开式"设计在 LLM 视角下是否意味着失去上下文？是否有设计文档说明？

9. **`Clone` 函数对反射的依赖**：`basic_processor.go:60-138` 的 `clone[M any]` 使用反射做深拷贝，且要求所有字段 exported。如果 `genai.GenerateContentConfig` 的未来版本引入 unexported 字段，这个深度拷贝会 panic。是否需要改用 proto clone 或 JSON round-trip？

10. **`LiveRun` 中的 `closedLiveSession`**：当 `BeforeRunCallback` 触发 early exit 时返回 `closedLiveSession`（`runner/runner.go:318-326`），其 `Send` 直接返回错误。但 `RunLive` 的调用方可能在获得 `closedLiveSession` 和迭代器之前就已经持有对原 `LiveSession` 的引用。这种 "替换 session" 的模式是否安全？调用方的并发安全如何保证？

---

## 附录：文件创建/修改时间

| 核心文件 | 行数 | 角色 |
|----------|------|------|
| `agent/agent.go` | ~300 | Agent 接口 + 基础实现 |
| `agent/context.go` | ~100 | InvocationContext 等接口定义 |
| `agent/llmagent/llmagent.go` | 490 | LLMAgent 创建 + run() |
| `runner/runner.go` | 678 | Runner — 会话 + 执行循环 |
| `model/llm.go` | 68 | LLM 接口 + 请求/响应 |
| `model/gemini/gemini.go` | ~350 | Gemini 模型实现 |
| `session/session.go` | 212 | Session/Event 类型 |
| `internal/llminternal/base_flow.go` | 1376 | **最大的单文件** — Flow + callLLM + handleFunctionCalls + RunLive |
| `internal/llminternal/contents_processor.go` | 625 | 历史事件 → LLM Contents |
| `internal/llminternal/agent_transfer.go` | 344 | Agent 间转移 tool |
| `internal/llminternal/stream_aggregator.go` | 329 | 流式响应聚合 |
| `internal/llminternal/instruction_processor.go` | 231 | Instruction 模板注入 |
| `internal/llminternal/agent.go` | 58 | LLMAgent 内部 State |
