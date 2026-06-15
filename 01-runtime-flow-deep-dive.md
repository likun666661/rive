# 精读报告：Runner / Agent / LLM Flow 主循环

> 阅读基线：`81a63d8feb7d713b1731f0c740d95574eb64dafa`
> 阅读范围：`google/adk-go`（ADK Go）
> 深度档位：`implementation`

---

## 1. problem — 这一层面临的问题

ADK Go 的 Runner / Agent / Flow 主循环要解决的核心问题是：**如何将一次用户自然语言请求，自动转换为多轮 LLM 调用、工具调用、Agent 转移、事件输出的可观测、可持久化的执行链路**。

具体来说，一条用户消息进入系统后，需要经历以下转换链：

```
User Message
  → Runner.Run (会话管理、Agent 路由)
    → Agent.Run (before/after callbacks、telemetry)
      → llmAgent.run (初始化 Flow)
        → Flow.Run (多 step 循环)
          → runOneStep (单 step 循环)
            → preprocess (processor 管道)
            → callLLM (模型调用 + model callbacks)
            → postprocess (response processor 管道)
            → finalizeModelResponseEvent (事件构造)
            → handleFunctionCalls (工具调用)
            → agent transfer (跨 agent 转移)
          → Event (yield 给 Runner)
            → AppendEvent (session 持久化)
              → yield to caller (客户端消费)
```

其中每一步都必须处理：
- **Streaming (SSE) / Live (Bidi) 两种模式**的差异
- **多 Agent 树**中如何路由到正确 Agent、如何控制 transfer 方向
- **Tool Call** 的并行执行、错误处理、回调链、confirmation 流程
- **State Delta** 的累积与合并
- **Session Event** 的持久化时机（partial vs final）

---

## 2. why_hard — 为什么这是问题

### 2.1 多 Agent + Agent Transfer 的复杂性

ADK 支持 Agent 树（`agent/agent.go:52-53` — `SubAgents() []Agent` / `FindAgent(name string) Agent`）。当 LLM 调用 `transfer_to_agent` 工具时，Flow 需要：

- 找到目标 agent（`base_flow.go:642`）
- 在**同一 invocation 内**启动目标 agent 的 `Run`（`base_flow.go:647`）
- 完成后返回原 agent 的步骤循环（如果 transfer 不终结 invocation）

以及 Runner 层面的 `findAgentToRun`（`runner/runner.go:592`）需要在跨 invocation 时基于 session history 找到最后活跃的 agent，并验证 transfer 链是否允许（`isTransferableAcrossAgentTree`，`runner/runner.go:653`）。

### 2.2 Streaming / Live 两种模式

| 模式 | 文件 | 特征 |
|------|------|------|
| SSE Streaming | `runner/runner.go:131` Run | 单向流，partial events 直接 yield，不持久化 |
| Live (Bidi) | `runner/runner.go:328` RunLive + `base_flow.go:251` RunLive | 双向 WebSocket，独立的 goroutine send/recv 循环，断线重连 |

Live 模式下还有**时序缓冲**（`runner/runner.go:459-508`）：如果 transcription 还在进行中就收到了 tool call/response，需要先缓冲 tool events，等 transcription 完成后再按正确时序写出。

### 2.3 Tool Call 的复杂性

- **并行执行**：多个 function call 通过 `sync.WaitGroup` 并行执行（`base_flow.go:1025-1174`），结果通过 `mergeParallelFunctionResponseEvents` 合并
- **回调链**：BeforeTool → Plugin → 实际执行 → AfterTool → Plugin，错误时还有 OnToolError 回调
- **Long Running Tool**：标记为 `IsLongRunning()` 的 tool 不会阻塞 step 循环（`base_flow.go:946`）
- **Streaming Tool**：Live 模式下支持流式工具，结果异步推送回模型（`base_flow.go:1066-1107`）
- **HITL Confirmation**：工具可以 `RequestConfirmation`，生成 `adk_request_confirmation` 事件（`functions.go:32-93`）

### 2.4 State Persistence 的复杂性

- 每次 `runOneStep` 积累 `stateDelta` map（`base_flow.go:555`）
- 工具调用产生的 state delta 需要 **deep merge**（`base_flow.go:1345-1359` `deepMergeMap`）
- OutputKey 机制：llmAgent 将 output 写入 `event.Actions.StateDelta`（`llmagent/llmagent.go:441-474`）
- 只有 **non-partial** 事件才持久化到 session service（`runner/runner.go:256-261`）

### 2.5 Contents 管理的复杂性

`ContentsRequestProcessor`（`contents_processor.go:37-63`）负责将 session event history 转换为 LLM 请求的 `Contents` 数组。这涉及：

- Branch 过滤（`eventBelongsToBranch`）：只取当前 agent 分支可见的事件
- 外来 agent 事件的转换（`ConvertForeignEvent`）：将其他 agent 的输出包装为 `"For context: [agent_x] said: ..."` 格式
- Function Call/Response 重排（`rearrangeEventsForLatestFunctionResponse` / `rearrangeEventsForFunctionResponsesInHistory`）：确保 function call 后紧跟其 response
- Transcription 文本聚合：将多个 partial transcription 事件合并为单个 text content

---

## 3. design_approach — ADK Go 的解决思路

### 3.1 整体架构

```
┌──────────────────────────────────────────────────────────────────┐
│                         Runner.Run                                │
│  session.Get/Create → findAgentToRun → appendMessageToSession    │
│  → agentToRun.Run(ctx) → for each event:                         │
│      plugin.onEvent → session.AppendEvent (if !Partial) → yield  │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│                     agent.Run (base agent)                        │
│  beforeAgentCallbacks → a.run(ctx) → afterAgentCallbacks          │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│                    llmAgent.run                                  │
│  创建 Flow{Model, RequestProcessors, ResponseProcessors,         │
│            BeforeModel/AfterModel/BeforeTool/AfterTool callbacks} │
│  → Flow.Run(ctx) → maybeSaveOutputToState                        │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│                      Flow.Run                                    │
│  for {                                                            │
│    runOneStep → if lastEvent.IsFinalResponse() → return           │
│  }                                                                │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│                    Flow.runOneStep                                │
│  1. preprocess (request processors pipeline)                     │
│  2. callLLM (BeforeCallbacks → model.GenerateContent →            │
│              AfterCallbacks → stream_aggregator)                  │
│  3. postprocess (response processors)                            │
│  4. finalizeModelResponseEvent                                    │
│  5. handleFunctionCalls (tool execution, parallel goroutines)     │
│  6. agent transfer (if ev.Actions.TransferToAgent != "")          │
└──────────────────────────────────────────────────────────────────┘
```

### 3.2 Processor Pipeline 设计

ADK 将 LLM Request 的构建拆分为一组按序执行的 **RequestProcessor**，每个 processor 负责填充 `LLMRequest` 的一个方面：

```
basicRequestProcessor          → 克隆 GenerateContentConfig
toolProcessor                  → 填充 Flow.Tools (从 Toolsets 展开)
authPreprocessor               → 认证预处理
RequestConfirmationRequestProcessor → HITL 确认
instructionsRequestProcessor   → 注入 SystemInstruction + GlobalInstruction
identityRequestProcessor       → Agent 身份信息
ContentsRequestProcessor       → 填充 Contents (session history)
nlPlanningRequestProcessor     → NL Planning
codeExecutionRequestProcessor  → 代码执行
outputSchemaRequestProcessor   → Output Schema 工具
AgentTransferRequestProcessor  → 注册 transfer_to_agent tool
removeDisplayNameIfExists      → 清理
```

这种管道设计使得：
- 每个 processor 可以独立测试
- 可以通过替换 `DefaultRequestProcessors` 来自定义行为
- 某些 processor 可以 yield event（如 confirmation），实现中途拦截

### 3.3 Callback 链设计

ADK 提供了多层回调，每层都遵循 **"先 plugin，后 user callback"** 的顺序：

| 层次 | 回调点 | 位置 |
|------|--------|------|
| Agent | BeforeAgent / AfterAgent | `agent/agent.go:186-214` |
| Model | BeforeModel / AfterModel / OnModelError | `base_flow.go:722-799` |
| Tool | BeforeTool / AfterTool / OnToolError | `base_flow.go:1193-1238` |
| Runner | BeforeRun / AfterRun / OnUserMessage / OnEvent | `runner/runner.go:218-253` |

---

## 4. code_walkthrough — 源码走读

### 4.1 `agent/agent.go` — Agent 接口与基础实现

**关键类型**：
- `Agent` 接口 (L43-52)：所有 agent 必须实现的统一接口。核心方法是 `Run(InvocationContext) iter.Seq2[*session.Event, error]`，返回 Go 1.23 的 push iterator。
- `Config` (L77-107)：自定义 agent 的配置，包含 `Name`、`SubAgents`、`BeforeAgentCallbacks`、`Run` 函数、`AfterAgentCallbacks`。
- `agent` (L139-148)：私有实现，嵌入 `agentinternal.State`。

**关键函数 `agent.Run`** (L162-214)：
```
1. 创建 telemetry span (StartInvokeAgentSpan)
2. 包装 yield 为 WrapYield (telemetry)
3. 复制 invocationContext
4. runBeforeAgentCallbacks → 有结果就 yield 并返回
5. a.run(ctx) → 迭代 yield event，自动设置 Author
6. runAfterAgentCallbacks → yield 结果
```

**`runBeforeAgentCallbacks`** (L247-302)：先调用 pluginManger 的回调，再依次调用 user callbacks。如果任何 callback 返回非 nil content，则构造 event、调用 `EndInvocation()`、返回。此外支持纯 state delta（无 content）也生成 event。

### 4.2 `agent/context.go` — 上下文体系

**关键类型**：
- `InvocationContext` (L62-105)：Agent 执行的完整上下文。内嵌 `context.Context`，提供 Agent、Session、Artifacts、Memory 等访问。关键方法 `EndInvocation()` 用于终止整个 invocation。
- `ReadonlyContext` (L108-122)：只读上下文，用于 InstructionProvider 等场景。
- `CallbackContext` (L125-130)：继承 ReadonlyContext，增加 Artifacts 和 State 的写访问。
- `ToolContext` (L136-189)：工具执行上下文，额外提供 `FunctionCallID()`、`Actions()`、`SearchMemory()`、`ToolConfirmation()`、`RequestConfirmation()`。

文档注释中给出了清晰的生命周期层级 (L28-60)：
```
invocation > agent_call > step > [call_llm, call_tool]
```

### 4.3 `agent/live.go` — Live 会话支持

定义了 `LiveSession` 接口（L22-25）：`Send(LiveRequest)` + `Close()`。`LiveRequest` 支持 `RealtimeInput`（音频 blob、activity start/end）和常规 `Content`。

`LiveRunConfig` (L38-49) 包含了双向流所需的所有配置：ResponseModalities、SpeechConfig、音频转录、SessionResumption 等。

### 4.4 `agent/run_config.go` — 运行时配置

```go
type RunConfig struct {
    StreamingMode            StreamingMode // "none" | "sse"
    SaveInputBlobsAsArtifacts bool
}
```

`StreamingMode` 在两个层级产生作用：
1. Runner 写入 `runconfig.StreamingModeSSE` 到 context（`runner/runner.go:173-175`）
2. Flow 的 `callLLM` 中读取该值决定是否 `useStream`（`base_flow.go:748`）

### 4.5 `agent/llmagent/llmagent.go` — LLM Agent 实现

**关键类型**：
- `llmAgent` (L340-357)：嵌入 `agent.Agent`、`llminternal.State`、`agentState`。包含 model、instruction、所有回调链。
- `Config` (L130-283)：最复杂的配置结构，包含 Model、Instructions、Tools、Toolsets、SubAgents、回调、OutputKey、InputSchema/OutputSchema 等。

**`llmAgent.run`** (L361-394)：
```go
func (a *llmAgent) run(ctx agent.InvocationContext) iter.Seq2[...] {
    ctx = icontext.NewInvocationContext(...)  // 创建内部 invocation context
    f := &llminternal.Flow{
        Model:              a.model,
        RequestProcessors:  llminternal.DefaultRequestProcessors,
        ResponseProcessors: llminternal.DefaultResponseProcessors,
        // ... 所有回调链
    }
    return func(yield ...) {
        for ev, err := range f.Run(ctx) {
            a.maybeSaveOutputToState(ev)  // 处理 OutputKey
            yield(ev, err)
        }
    }
}
```

**`llmAgent.RunLive`** (L396-437)：与 `run` 类似，但调用 `f.RunLive(ctx)` 获取双向 `LiveSession` + event iterator。

**`maybeSaveOutputToState`** (L441-474)：当 `OutputKey != ""` 且 event 是 non-partial 且有 content 时，将 text parts 拼接为字符串，写入 `event.Actions.StateDelta[OutputKey]`。

### 4.6 `runner/runner.go` — Runner 主循环

**`Runner.Run`** (L131-268) — 完整执行链路：

```
1. 解析 RunOptions (stateDelta)
2. sessionService.Get → 不存在则 Create (autoCreateSession)
3. findAgentToRun(storedSession, msg) → 确定目标 agent
4. parentmap.ToContext / runconfig.ToContext / plugininternal.ToContext
5. 初始化 Artifacts / Memory
6. icontext.NewInvocationContext → 创建 InvocationContext
7. appendMessageToSession → 将 user message 写入 session (含 blob→artifact 转换)
8. pluginManager.RunBeforeRunCallback → 如有 early exit，直接返回
9. agentToRun.Run(ctx) → 迭代所有 event:
   a. pluginManager.RunOnEventCallback → 可修改 event
   b. if !event.Partial → sessionService.AppendEvent (持久化)
   c. yield(event)
```

**`findAgentToRun`** (L592-623)：
1. 先检查 user message 是否是 function response → 找到发出该 function call 的 agent
2. 否则反向遍历 session events，找到最后一个可 transfer 的 agent（`isTransferableAcrossAgentTree`）
3. 默认 fallback 到 root agent

**`isTransferableAcrossAgentTree`** (L653-666)：从当前 agent 向上遍历 parent 链，任一 agent 标记 `DisallowTransferToParent` 则不可 transfer。

**`appendMessageToSession`** (L533-588)：
1. pluginManager.RunOnUserMessageCallback → 可修改 msg
2. 如果 `SaveInputBlobsAsArtifacts`，将 InlineData parts 保存为 artifact，替换为文本 placeholder
3. 构造 user event 并 AppendEvent

**`Runner.RunLive`** (L328-531)：
1. 与 Run 类似的前置处理（session、agent 查找、context 注入）
2. 类型断言 `agentToRun.(liveAgent)`，调用 `lAgent.RunLive(iCtx)`
3. 包装 event iterator：实现 transcription 时序缓冲逻辑（L459-508）
4. 返回 `runnerLiveSession`（包装了 `Send` 方法，自动将 user content 持久化到 session）

### 4.7 `internal/llminternal/base_flow.go` — Flow 核心循环

**`Flow` 结构体** (L62-74)：包含 Model、Tools、RequestProcessors、ResponseProcessors 和所有回调链。

**`Flow.Run`** (L101-127) — Agent 调用级循环：
```go
func (f *Flow) Run(ctx agent.InvocationContext) iter.Seq2[*session.Event, error] {
    for {
        lastEvent := ... 
        for ev, err := range f.runOneStep(ctx) { yield(ev) }
        if lastEvent == nil || lastEvent.IsFinalResponse() { return }
        if lastEvent.LLMResponse.Partial { /* error */ return }
    }
}
```
关键：循环直到 `IsFinalResponse()` 为 true（没有 function call、没有 transfer、无 code execution 等需要继续的信号）。

**`Flow.runOneStep`** (L528-654) — 单步执行：
```
1. preprocess → 运行所有 RequestProcessors
2. callLLM → BeforeCallbacks → generateContent → AfterCallbacks → OnModelError
3. postprocess → 运行 ResponseProcessors
4. 跳过空响应（Content == nil && ErrorCode == "" && !Interrupted）
5. finalizeModelResponseEvent → 构建 event
6. yield model response event
7. handleFunctionCalls → 并行执行 tools
8. 生成 tool confirmation event（如有）
9. 如 TransferToAgent != "" → 查找目标 agent → agent.Run(ctx)
```

**`Flow.callLLM`** (L722-799)：
```
1. pluginManager.RunBeforeModelCallback
2. user BeforeModelCallbacks（链式，第一个非 nil 结果生效）
3. generateContent(ctx, f.Model, req, useStream) → 实际 LLM 调用
4. 如果 LLM 返回 error → runOnModelErrorCallbacks
5. runAfterModelCallbacks → 链式
```

**`generateContent`** (L809-855)：
- 创建 telemetry span
- 调用 `m.GenerateContent(ctx, req, useStream)`
- 每个 response 包装为 `responseWithEventID`（分配 UUID）
- streaming 模式下：partial responses 只 trace 不 log；final response log

**`handleFunctionCalls`** (L1012-1180)：
- 提取所有 FunctionCalls
- 多个 call 时创建 merged trace span
- 对每个 call 启动 goroutine（`sync.WaitGroup` 等待）：
  - 特殊处理 `stop_streaming`（Live 模式取消流式工具）
  - tool not found → `newToolNotFoundError` → OnToolErrorCallbacks
  - streaming tool (Live) → 异步运行，结果通过 `LiveSession.Send` 送回模型
  - streaming tool (非 Live) → 顺序收集 chunk，合并为单个 result
  - 普通 tool → `callTool` → Before/After callbacks + 实际执行
- `mergeParallelFunctionResponseEvents` → 合并多个 tool 结果为单个 event

### 4.8 `internal/llminternal/basic_processor.go` — 基础配置填充

`basicRequestProcessor` (L31-55)：将 `llmAgent.State.GenerateContentConfig` 深拷贝到 `req.Config`。如果设置了 OutputSchema 且不需要 tool-based workaround，直接设置 `ResponseSchema` 和 `ResponseMIMEType`。

`clone` 函数 (L60-139)：基于 `reflect` 的泛型深拷贝，不含未导出字段（会 panic）。

### 4.9 `internal/llminternal/contents_processor.go` — Contents 构建

`ContentsRequestProcessor` (L37-63)：收集 session events，调用 `buildContentsDefault` 或 `buildContentsCurrentTurnContextOnly`。

**`buildContentsDefault`** (L67-174)：
1. 过滤：跳过无 content/role/parts 且无 transcription 的事件
2. Branch 过滤：`eventBelongsToBranch` — 仅保留当前 branch 可见的事件
3. 排除：`adk_request_credential` 和 `adk_request_confirmation` 调用/响应不进入 LLM context
4. 外来 agent 事件的转换（`ConvertForeignEvent`）：将其他 agent 的输出包装为用户内容
5. Transcription 聚合：将连续的 partial transcription 合并为单个 text content
6. Function Call/Response 重排：
   - `rearrangeEventsForLatestFunctionResponse`：处理最后一个 event 是 function response 的情况
   - `rearrangeEventsForFunctionResponsesInHistory`：确保每个 function call 后紧跟 merged response
7. 清理空 parts、删除 client-side function call ID

**`eventBelongsToBranch`** (L176-187)：使用 `strings.HasPrefix(invocationBranch, event.Branch+".")` 匹配，避免 agent_1 意外匹配 agent_10。

### 4.10 `internal/llminternal/instruction_processor.go` — Instructions 注入

`instructionsRequestProcessor` (L41-67)：
1. 从 parentmap 找到 root agent（用于获取 GlobalInstruction）
2. `appendGlobalInstructions` → 先 `InstructionProvider` 后模板渲染
3. `appendInstructions` → 同上

**`InjectSessionState`** (L204-231)：使用正则 `{+[^{}]*}+` 匹配模板占位符，调用 `replaceMatch` 替换：
- `{artifact.name}` → 加载 artifact 的 text
- `{state_var}` → `Session().State().Get(varName)`（支持 app:/user:/temp: 前缀）
- `{var?}` → 可选，不存在不报错

### 4.11 `internal/llminternal/tools_processor.go` — Tools 注册

`toolProcessor` (L29-51)：从 `llmAgent.State.Tools` 和 `State.Toolsets`（通过 `toolSet.Tools(ctx)` 展开）收集所有 tool，写入 `f.Tools`。

### 4.12 `internal/llminternal/stream_aggregator.go` — Streaming 响应聚合

`streamingResponseAggregator` (L33-50) 处理 Gemini steaming 响应：
- 聚合 text chunks 为完整的 text part
- 聚合 PartialArgs 为完整的 FunctionCall（使用 JSONPath 深路径设置 `setValueByJSONPath`）
- 区分 thought 和普通 text
- `Close()` 方法生成最终的聚合响应

### 4.13 `internal/llminternal/agent_transfer.go` — Agent Transfer 机制

**核心思想**：Agent Transfer 通过**注册特殊 tool `transfer_to_agent`** 来实现。当 LLM 调用此 tool 时，`Run` 方法将目标 agent name 写入 `ctx.Actions().TransferToAgent`。

**`AgentTransferRequestProcessor`** (L69-97)：
- 仅在 `shouldUseAutoFlow(agent)` 时生效
- 调用 `transferTargets` 计算可 transfer 的 agent 列表
- 创建 `TransferToAgentTool`，将其 instructions 注入 LLM request，并注册为 tool

**`transferTargets`** (L185-211)：
```
targets = agent.SubAgents()  // 子 agent 始终可选
if !DisallowTransferToParent → 添加 parent
if !DisallowTransferToPeers && parent is AutoFlow → 添加 peer agents
```

**Agent Transfer Prompt** (`agentTransferInstructionTemplate`, L324-344)：使用 Go template 生成 prompt，列出所有可用 agent 的名称和描述。

### 4.14 `internal/llminternal/functions.go` — Tool Confirmation 事件

`generateRequestConfirmationEvent` (L32-93)：当 tool 返回 `RequestedToolConfirmations` 时，为每个需要确认的 function call 生成一个 `adk_request_confirmation` function call 事件。

### 4.15 `internal/context/invocation_context.go` — 内部 InvocationContext

`InvocationContext` 结构体 (L52-56) 是 `agent.InvocationContext` 接口的内部实现，通过 `InvocationContextParams` 注入所有参数。额外提供了 `LiveSessionResumptionHandle` 的 getter/setter（用于 Live 重连）。

### 4.16 `internal/context/readonly_context.go` — ReadonlyContext

`ReadonlyContext` (L33-36) 包装 `agent.InvocationContext`，只暴露只读方法（`AppName`、`UserID`、`SessionID` 等），用于 InstructionProvider 和 Toolset 的 `Tools()` 方法。

---

## 5. execution_trace — 典型执行链路

以下跟踪一条典型的用户请求 `"What's the weather in Tokyo?"` 在具有 weather tool 的 LLM Agent 中的完整执行链路：

### Step 1: `Runner.Run`

```
runner/runner.go:131  Run(ctx, userID, sessionID, msg, cfg)
  → L142: sessionService.Get → 获取已有 session
  → L166: findAgentToRun(session, msg)
       → L592: root agent (首次调用，无 history)
  → L172: parentmap.ToContext → 注入 agent 树关系
  → L173: runconfig.ToContext → 注入 StreamingMode
  → L198: icontext.NewInvocationContext → 创建 invocation context
  → L206: appendMessageToSession(storedSession, msg)
       → L574: 创建 user event、设置 Author="user"
       → L584: sessionService.AppendEvent → 持久化 user message
  → L234: agentToRun.Run(ctx) → 进入 Agent 层
```

### Step 2: `agent.Run` (base agent)

```
agent/agent.go:162  agent.Run
  → L164: telemetry.StartInvokeAgentSpan
  → L186: runBeforeAgentCallbacks → 无 before callback，返回 nil
  → L197: a.run(ctx) → 进入 llmAgent.run
```

### Step 3: `llmAgent.run`

```
agent/llmagent/llmagent.go:361  llmAgent.run
  → L363: icontext.NewInvocationContext → 包装内部 context
  → L374: 创建 Flow{Model, DefaultRequestProcessors, DefaultResponseProcessors, callbacks}
  → L387: f.Run(ctx) → 进入 Flow 层
```

### Step 4: `Flow.Run` → `runOneStep` (Step 1)

```
internal/llminternal/base_flow.go:101  Flow.Run
  → L105: f.runOneStep(ctx)
       → L530: 检查 Model != nil
       → L535: 创建 LLMRequest{Model: f.Model.Name()}
       → L540: f.preprocess(ctx, req)
```

### Step 5: `preprocess` — RequestProcessor Pipeline

```
base_flow.go:656  preprocess
  → basicRequestProcessor → 克隆 GenerateContentConfig
  → toolProcessor → 从 State.Tools/Toolsets 填充 f.Tools (weather_tool)
  → instructionsRequestProcessor → 注入 system instruction
  → ContentsRequestProcessor → 构建 LLM Contents (当前只有 user message)
  → AgentTransferRequestProcessor → 注册 transfer_to_agent tool (如有 subagents)
```

### Step 6: `callLLM`

```
base_flow.go:722  callLLM
  → BeforeModelCallbacks → Plugin → User callbacks (均通过)
  → L748: useStream = StreamingModeSSE ？
  → L750: generateContent(ctx, f.Model, req, useStream)
       → model/gemini/gemini.go: 调用 genai.Models.GenerateContent
       → 返回 LLMResponse{Content: {Parts: [{FunctionCall: {Name: "get_weather", Args: {"city": "Tokyo"}}}]}}
  → AfterModelCallbacks → Plugin → User callbacks
  → yield(responseWithEventID)
```

### Step 7: `postprocess` + `finalizeModelResponseEvent`

```
base_flow.go:563  postprocess
  → nlPlanningResponseProcessor → nl planning 后处理
  → codeExecutionResponseProcessor → 代码执行后处理
base_flow.go:588  finalizeModelResponseEvent
  → 构建 session.Event
  → Author = ctx.Agent().Name()
  → PopulateClientFunctionCallID → 确保 function call ID 非空
  → yield modelResponseEvent → Runner 层：!Partial → AppendEvent
```

### Step 8: `handleFunctionCalls`

```
base_flow.go:599  handleFunctionCalls
  → 提取 fnCalls = [{Name: "get_weather", Args: {"city": "Tokyo"}}]
  → L1017: len(fnCalls) == 1 → 不创建 merged span
  → 对每个 fnCall 启动 goroutine:
       → 查找 toolsDict["get_weather"] → 找到 weather tool
       → callTool: BeforeToolCallbacks → tool.Run(ctx, args) → result = {"temp": 22, "condition": "sunny"}
       → 构造 FunctionResponse event
  → mergeParallelFunctionResponseEvents → 合并为单个 event
  → yield function response event → Runner 层持久化
```

### Step 9: 返回 `Flow.Run` 循环

```
base_flow.go:116  lastEvent.IsFinalResponse() == false (有 function call)
  → 继续下一轮 step
```

### Step 10: 第二轮 `runOneStep`

- `ContentsRequestProcessor` 现在会将上一轮的 function call + function response 加入 history
- `callLLM` 发送带有 conversation history 的请求
- LLM 返回最终文本响应 `"The weather in Tokyo is 22°C and sunny."`
- `lastEvent.IsFinalResponse()` == true → Flow.Run 退出
- `llmagent.run`: `maybeSaveOutputToState` → 如果有 OutputKey，写入 state
- `agent.Run`: `runAfterAgentCallbacks` → 无回调
- 返回 Runner 层：最后 yield、持久化

---

## 6. tests — 支撑这些判断的测试

| 测试文件 | 测试意图 |
|----------|----------|
| `runner/runner_test.go:34` `TestRunner_findAgentToRun` | 验证 session history 中的 agent routing 逻辑：包含 function response 匹配、transfer 链检查、fallback to root |
| `runner/runner_test.go:125` `Test_isTransferrableAcrossAgentTree` | 验证 `DisallowTransferToParent` 和 non-LLM agent 的 transfer 限制 |
| `runner/runner_test.go:171` `TestRunner_SaveInputBlobsAsArtifacts` | 验证 blob→artifact 转换和 placeholder 替换 |
| `runner/runner_test.go:350` `TestRunner_AutoCreateSession` | 验证 autoCreateSession 四种场景（存在/不存在 × 自动/不自动） |
| `runner/live_runner_test.go:44` `TestRunner_RunLive_Callbacks` | 验证 Live 模式下 BeforeRun/AfterRun callbacks 的调用时序 |
| `runner/live_runner_test.go:123` `TestRunner_RunLive_EarlyExit` | 验证 BeforeRunCallback 返回 content 时的 early exit 路径及 closedLiveSession |
| `runner/live_runner_test.go:204` `TestRunner_RunLive_ChronologicalBuffering` | 验证 transcription 时序缓冲：partial transcription + 中间 tool call + final transcription 的正确排序 |

> 注：`internal/llminternal/` 下的各 processor 文件没有在该仓库中发现独立的 `_test.go` 文件（仅通过集成测试覆盖）。

---

## 7. risks — 未读风险、TODO、可能的边界 bug

### 7.1 源码中标注的 TODO

| 位置 | TODO 内容 | 风险 |
|------|-----------|------|
| `agent/agent.go:172` | `"TODO: verify&update the setup here. Should we branch etc."` | invocation context 的构造逻辑可能不完善 |
| `agent/agent.go:346` | `// TODO set context invocation ended` | AfterAgentCallback 返回后未设置 `endInvocation`，可能导致 agent 继续执行 |
| `agent/llmagent/llmagent.go:108-126` | `"TODO: remove this in favor of the state reveal below"` / `"TODO: temporary hack"` | llmAgent 的类型信息需要在多处修补，存在不一致风险 |
| `base_flow.go:121-123` | `"TODO: handle Partial response in model level"` | streaming 模式下达到 max token limit 时直接返回 error，未优雅处理 |
| `base_flow.go:638` | `"TODO(hakim): figure out why this isn't handled by the runner"` | agent transfer 的触发在 Flow 层而非 Runner 层，设计意图不清 |
| `base_flow.go:744` | `"TODO: Set _ADK_AGENT_NAME_LABEL_KEY in req.GenerateConfig.Labels"` | 无法按 agent 维度拆分计费 |
| `contents_processor.go:39` | `"TODO: implement (adk-python ...) - extract function call results, etc."` | Contents 处理不完整，与 Python 版本可能不一致 |
| `agent_transfer.go:66` | `"TODO: implement it in the runners package and update this doc"` | transfer 逻辑分散在 Flow 和 Runner 两层，职责不清 |
| `base_flow.go:1132` | `"TODO: handle long-running tool"` | Long Running Tool 的完整生命周期管理未实现 |

### 7.2 潜在边界 bug

1. **Branch 匹配** (`contents_processor.go:176-187`)：使用 `strings.HasPrefix(invocationBranch, event.Branch+".")` 匹配是合理的，但 `invocationBranch` 为空时直接返回 `true`，可能遗漏 edge case。

2. **`eventBelongsToBranch`** 空 branch 时返回 true，这意味着没有 branch 信息的旧事件会对所有 agent 可见，可能导致并行 agent 场景下信息泄漏。

3. **Deep Copy 的未导出字段** (`basic_processor.go:95-98`)：`deepCopy` 函数在遇到未导出字段时会 `panic`，如果 `genai.GenerateContentConfig` 新增未导出字段，会导致运行时 panic。

4. **Streaming Response 的 Partial 处理**：`streamingResponseAggregator` 的 `Close()` 方法在 `FinishReason != Stop` 时保留 errorCode/errorMessage，但 `runOneStep` 中检查 `resp.Content == nil && resp.ErrorCode == ""` 来跳过空响应——这两个逻辑的一致性是脆弱的。

5. **并行 Tool Call 的 Action Merge** (`base_flow.go:1315-1343`)：`mergeEventActions` 中对于 `TransferToAgent` 和 `Escalate` 是覆盖而非合并，如果多个并行 tool 同时设置了不同的 `TransferToAgent`，只有最后一个生效。

6. **`FindAgent` vs `FindSubAgent`** (`agent/agent.go:221-235`)：`FindAgent` 先检查自身再检查子 agent（递归），但 `FindSubAgent` 只检查子 agent。在 llmAgent 中重写了 `FindAgent` (`llmagent/llmagent.go:477-482`) 调用 `a.Agent.FindSubAgent`，而 `a.Agent` 是 `agent.agent` 类型——这一层嵌套可能导致在复杂 agent 树中查找失败。

7. **Contents 重排的异步 function call** (`contents_processor.go:432`)：注释明确指出 `"Caveat: This implementation doesn't support a parallel function call event that contains async function calls of the same name"`——这意味着同名异步工具的并行调用会导致内容合并错误。

### 7.3 架构层面的风险

1. **单线程 Agent 执行**：`agent.Run` 是同步迭代器，同一时刻只有一个 agent 在运行。并行 agent（Parallel Agent）的实现细节不在本次精读范围，但其设计在现有架构中可能受限。

2. **Error 传播**：`runOneStep` 中多处 `yield(nil, err); return`——一旦 yield 返回 false（consumer 停止），err 和 event 可能被丢失。这在 Live 模式下尤其危险。

3. **Plugin 与 Callback 的重复**：Plugin 和 User Callback 在每个钩子点都是先后调用的，但 Plugin 的存在导致错误处理路径翻倍（`if pluginManager != nil { ... }`），增加了维护负担。

---

## 8. next_questions — 下一轮追问

1. **Parallel Agent 的实现**：多个 sub-agent 并行执行时，branch 如何分配？events 如何合并？与现有的单线程 `agent.Run` 迭代器模型如何集成？

2. **Session Service 的实现**：`session.InMemoryService()` 和 `session.Service` 接口的实际存储后端是什么？event 持久化是同步还是异步？有无写入失败的重试？

3. **Long Running Tool 的完整流程**：当 `IsLongRunning()` 返回 true 时，Flow 如何等待异步结果？如何将异步结果重新注入到 step 循环？跨 invocation 的 long running tool 如何恢复？

4. **HITL (Human-in-the-Loop) 的完整状态机**：`RequestConfirmation` 后如何挂起 agent？用户确认后如何恢复？状态如何跨 invocation 持久化？

5. **Memory Service 的集成**：`agent.Memory.SearchMemory` 在 Flow 的何处被调用？搜索结果如何注入到 LLM context？

6. **Code Execution 的实现**：`codeExecutionRequestProcessor` 和 `codeExecutionResponseProcessor` 的具体逻辑是什么？如何与外部代码执行环境交互？

7. **NL Planning 的实现**：`nlPlanningRequestProcessor` 和 `nlPlanningResponseProcessor` 如何标记/取消标记 planning thoughts？

8. **Error 处理与重试策略**：LLM 调用失败（如 429 rate limit）时的重试逻辑在哪里？`OnModelErrorCallbacks` 是否能返回非 nil response 来恢复？

9. **State 的完整生命周期**：`session.State` 的读写如何保证并发安全？`StateDelta` 的 merge 逻辑是否有原子性保证？跨 agent transfer 时 state 如何共享/隔离？

10. **Streaming 模式下的 Event 去重**：`streamingResponseAggregator` 产生的中间 partial events 和最终的 aggregated event 如何避免重复持久化？Runner 层的 `!event.Partial` 检查是否足够？

11. **Plugin 系统的设计意图**：Plugin 与 User Callback 的职责划分是什么？为什么在 agent、model、tool 三层都有 plugin 钩子？未来的 plugin 扩展方向？

12. **与 Python ADK 的差异**：本次精读发现多处 `TODO` 标注了与 Python 实现的差异（contents processor、agent transfer 位置等）。哪些 Python 功能已对齐，哪些是关键缺口？
