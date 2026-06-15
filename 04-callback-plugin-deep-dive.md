# Callback / Plugin / Instruction 扩展机制精读报告

> 阅读基线：`81a63d8feb7d713b1731f0c740d95574eb64dafa`
> 阅读深度：`implementation`
> 仓库：`google/adk-go`

---

## 1. `problem` — 为什么 agent runtime 需要横切扩展点

ADK-Go 的执行链路包含多个关键节点：用户消息到达 → agent 启动 → LLM 请求构造 → LLM 调用 → 响应解析 → tool 调用 → 循环迭代。在不同节点上，开发者需要：

1. **可观测性**：记录请求/响应、token 用量、tool 调用参数和结果（logging plugin 场景）。
2. **流量控制**：在 LLM 调用前拦截请求，返回缓存或 mock 响应，跳过真实调用（replay plugin、caching 场景）。
3. **请求/响应改写**：修改 tool 声明（FunctionCallModifier）、修改 system instruction、注入额外 context。
4. **错误恢复**：tool 调用失败时自动重试并注入反思提示（RetryAndReflect plugin）。
5. **状态管理**：在 agent 生命周期中读写 session state、artifact delta。
6. **用户审批**：tool 执行前请求用户确认（HITL RequestConfirmation）。
7. **指令注入**：根据 session state 动态渲染 instruction 模板中的 `{placeholder}`。

如果不提供统一的扩展点，每个功能都需要侵入 agent core 逻辑，导致代码重复、顺序冲突、难以组合。

---

## 2. `why_hard` — 插入横切逻辑的难点

### 2.1 状态污染风险

Callbacks 可以调用 `ctx.State().Set()` 修改 session state。如果多个 callback/plugin 同时修改同一个 key，会出现竞态。当前实现通过 **state delta** 机制隔离：callback 的写操作记录在 `EventActions.StateDelta` 中，只在该 step 的 Event 产出时合并到真实 session state。但同时，callback 对真实 session state 的 `Set()` 调用（`callbackContextState.Set` at `agent/callback_context.go:230-235`）**同时写入 delta 和真实 state**，这意味着 callback 的 state 修改是立即可见的，而非事务性的。

### 2.2 控制流改变

多个 callback 类型可以改变控制流：

| Callback 类型 | 返回 nil | 返回非 nil Content | 返回 error |
|---|---|---|---|
| `BeforeAgentCallback` | 继续 agent run | **终止 agent run**，创建新 Event | 终止，返回 error |
| `BeforeModelCallback` | 继续调用 LLM | **跳过 LLM**，返回该响应 | 终止，返回 error |
| `BeforeToolCallback` | 继续调用 tool | **跳过 tool**，返回该结果 | 终止，返回 error |
| `AfterModelCallback` | 使用原始响应 | **替换 LLM 响应** | 终止，返回 error |
| `AfterToolCallback` | 使用原始结果 | **替换 tool 结果** | 终止，返回 error |
| `OnModelErrorCallback` | 透传原始 error | **替换为成功响应** | 终止，返回新 error |
| `OnToolErrorCallback` | 透传原始 error | **替换为成功结果** | 终止，返回新 error |

关键行为：**Once any callback returns a non-nil result, the remaining callbacks in the list are skipped**（early-exit 模式）。这意味着 callback 的顺序至关重要。

### 2.3 组合冲突

同一个 hook 点可能有多个 plugin 注册。`PluginManager` 采用 **顺序执行 + early-exit** 策略（`internal/plugininternal/plugin_manager.go:76-90`）。这导致：

- 如果 Plugin A 的 `BeforeModelCallback` 返回了 mock 响应，Plugin B 的 `BeforeModelCallback` 根本不会执行。
- plugin 的优先级完全由注册顺序决定，无法在运行时动态调整。
- 没有 plugin 间的依赖声明或 combinator 模式（如 middleware chain）。

---

## 3. `design_approach` — 核心设计决策

### 3.1 三层扩展架构

```
┌─────────────────────────────────────────────────────────────┐
│                       Plugin Layer                          │
│  plugin.Plugin (包装器, 聚合所有回调类型)                    │
│  PluginManager (顺序执行 + early-exit)                       │
│  例: LoggingPlugin, RetryAndReflect, FunctionCallModifier   │
├─────────────────────────────────────────────────────────────┤
│                      Callback Layer                         │
│  agent.BeforeAgentCallback / AfterAgentCallback              │
│  llmagent.BeforeModelCallback / AfterModelCallback / ...    │
│  llmagent.BeforeToolCallback / AfterToolCallback / ...      │
│  (直接注册在 Agent Config 中)                                │
├─────────────────────────────────────────────────────────────┤
│                   Instruction/Processor Layer               │
│  InstructionProvider / GlobalInstructionProvider             │
│  InjectSessionState (模板变量注入)                           │
│  RequestProcessor pipeline (basic, tools, instruction, ...) │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 Callback Context 设计

`CallbackContext` (`agent/context.go:125`) 是所有 callback 的统一上下文，提供：
- **只读接口** (`ReadonlyContext`): `AgentName()`, `InvocationID()`, `SessionID()`, `UserID()`, `Branch()`, `ReadonlyState()`, `UserContent()`
- **读写接口**: `State()` (返回 `session.State`), `Artifacts()`
- `ToolContext` 在此基础上增加了: `FunctionCallID()`, `Actions()` (返回 `*session.EventActions`), `SearchMemory()`, `ToolConfirmation()`, `RequestConfirmation()`

核心实现细节 (`agent/callback_context.go:101`):
- `callbackContext` 是 **单个具体类型**，同时实现 `CallbackContext` 和 `ToolContext`
- `callbackContext.State()` 返回 `callbackContextState`，其 `Get()` 方法先查 delta 再查真实 state，`Set()` 同时写入 delta 和真实 state
- `trackedArtifacts` 装饰器自动追踪 `Artifacts().Save()` 到 `EventActions.ArtifactDelta`

### 3.3 Plugin Manager 设计

`PluginManager` (`internal/plugininternal/plugin_manager.go:38`) 持有 `[]*plugin.Plugin` 有序列表。

**执行策略（贯穿所有 hook 方法）**：
```
for each plugin (in registration order):
    callback = plugin.SomeCallback()
    if callback == nil: continue
    result, err = callback(ctx, ...)
    if err != nil: return nil, err    // 立即失败
    if result != nil: return result   // early-exit
return nil, nil                        // 所有 plugin 都返回 nil
```

这意味着：
1. **第一个返回非 nil 值的 plugin 胜出**
2. **任何 plugin 的 error 都会中断整个链**
3. **返回 nil,nil 表示"我不处理，继续下一个"**

### 3.4 Hook Ordering

在 `Flow` 执行中的时序（`internal/llminternal/base_flow.go`）：

```
Flow.Run (主循环)
  └─ Flow.runOneStep
       ├─ preprocess (RequestProcessors pipeline)
       │    ├─ basicRequestProcessor
       │    ├─ toolProcessor
       │    ├─ authPreprocessor
       │    ├─ RequestConfirmationRequestProcessor  ← 处理 HITL
       │    ├─ instructionsRequestProcessor  ← 注入 instruction/global instruction
       │    ├─ identityRequestProcessor
       │    ├─ ContentsRequestProcessor
       │    ├─ nlPlanningRequestProcessor
       │    ├─ codeExecutionRequestProcessor
       │    ├─ outputSchemaRequestProcessor
       │    ├─ AgentTransferRequestProcessor
       │    └─ removeDisplayNameIfExists
       │
       ├─ callLLM
       │    ├─ PluginManager.RunBeforeModelCallback  ← plugin hook
       │    ├─ f.BeforeModelCallbacks (逐个, early-exit)
       │    ├─ generateContent (真实 LLM 调用)
       │    ├─ if error: OnModelErrorCallbacks
       │    ├─ PluginManager.RunAfterModelCallback
       │    └─ f.AfterModelCallbacks
       │
       ├─ postprocess (ResponseProcessors pipeline)
       │    ├─ nlPlanningResponseProcessor
       │    └─ codeExecutionResponseProcessor
       │
       └─ handleFunctionCalls
            └─ callTool (per tool, in parallel goroutines)
                 ├─ PluginManager.RunBeforeToolCallback
                 ├─ f.BeforeToolCallbacks → tool.Run → OnToolErrorCallbacks
                 ├─ PluginManager.RunOnToolErrorCallback (if error)
                 ├─ PluginManager.RunAfterToolCallback
                 └─ f.AfterToolCallbacks

Agent.Run (agent 层)
  ├─ PluginManager.RunBeforeAgentCallback  ← plugin hook
  ├─ agent 注册的 BeforeAgentCallbacks (逐个, early-exit)
  ├─ agent.run() (LLM loop)
  └─ PluginManager.RunAfterAgentCallback
     └─ agent 注册的 AfterAgentCallbacks (逐个, early-exit)
```

关键时序：
- **Plugin hooks 总是先于直接注册的 callbacks 执行**
- **BeforeModel/BroreTool callbacks 先于真实 LLM/tool 调用**
- **AfterModel/AfterTool callbacks 后于真实 LLM/tool 调用**，可以改写结果
- **OnError callbacks 只在错误发生时调用**，可以恢复或替换错误

### 3.5 State / Artifact Delta 机制

```
callbackContextState (agent/callback_context.go:217)
  ├─ Get(key):
  │    1. 先查 actions.StateDelta[key] (当前 step 内其他 callback 的写入)
  │    2. 再查 invocationContext.Session().State() (持久化 state)
  │
  └─ Set(key, val):
       1. 写入 actions.StateDelta[key] (当前 Event 的 delta)
       2. 同时写入 invocationContext.Session().State() (立即持久化!)

trackedArtifacts (agent/callback_context.go:243)
  └─ Save(ctx, name, data):
       1. 调用真实 Artifacts.Save()
       2. 成功后将版本号记录到 actions.ArtifactDelta[name]
```

注意：**State().Set() 同时写入 delta 和真实 state**，这意味着即使在 early-exit 场景下（后续 callback 被跳过），已执行的 callback 的 state 修改已持久化。这是一种 **写穿(write-through)** 而非 **写回(write-back)** 策略。

---

## 4. `code_walkthrough` — 逐文件分析

### 4.1 `agent/callback_context.go` — CallbackContext 实现

**文件**: `agent/callback_context.go`（261 行）

**关键类型**：

| 类型 | 行号 | 说明 |
|---|---|---|
| `callbackContext` | :101-110 | 唯一具体实现，同时满足 `CallbackContext` 和 `ToolContext` |
| `callbackContextState` | :217-219 | session.State 装饰器，实现 delta-prioritized 读取 |
| `trackedArtifacts` | :243-246 | Artifacts 装饰器，自动追踪 Save 版本号 |

**构造函数**：

- `NewCallbackContext(ic, actions)` (:33): 基础构造，不含 artifact 追踪
- `NewCallbackContextWithArtifactTracking(ic, actions)` (:48): 含 artifact 追踪
- `NewToolContext(ic, functionCallID, actions, confirmation)` (:67): 构造 ToolContext

`prepareEventActions` (:82-94): 确保 `StateDelta` 和 `ArtifactDelta` 都是非 nil map。

**ToolContext 扩展方法**：

- `RequestConfirmation(hint, payload)` (:196-213): 将确认信息写入 `actions.RequestedToolConfirmations`，并设置 `SkipSummarization = true` 以暂停 agent loop。

### 4.2 `agent/context.go` — 接口定义

**文件**: `agent/context.go`（190 行）

定义了四个核心接口：

```go
InvocationContext  // 完整调用上下文 (agent/context.go:62-105)
  ├─ Agent(), Artifacts(), Memory(), Session()
  ├─ InvocationID(), Branch(), UserContent(), RunConfig()
  └─ EndInvocation(), Ended(), WithContext()

ReadonlyContext    // 只读上下文 (agent/context.go:108-122)
  ├─ UserContent(), InvocationID(), AgentName()
  ├─ ReadonlyState(), UserID(), AppName(), SessionID()
  └─ Branch()

CallbackContext    // Callback 上下文 (agent/context.go:125-130)
  ├─ ReadonlyContext (嵌入)
  ├─ Artifacts()
  └─ State() session.State  ← 可写

ToolContext        // Tool 执行的上下文 (agent/context.go:136-189)
  ├─ CallbackContext (嵌入)
  ├─ FunctionCallID(), Actions()
  ├─ SearchMemory(), ToolConfirmation()
  └─ RequestConfirmation(hint, payload)
```

**设计亮点**：`CallbackContext` 嵌入 `ReadonlyContext` 而非 `InvocationContext`，防止 callback 调用 `EndInvocation()` 或访问 `RunConfig()`，实现了最小权限原则。

### 4.3 `agent/agent.go` — Agent 级 Callback

**文件**: `agent/agent.go`（437 行）

**核心类型**：
- `BeforeAgentCallback func(CallbackContext) (*genai.Content, error)` (:129)
- `AfterAgentCallback func(CallbackContext) (*genai.Content, error)` (:137)

**执行流程** (`agent.Run()` :162-215):
```
1. runBeforeAgentCallbacks(ctx)
   ├─ PluginManager.RunBeforeAgentCallback  ← plugin hook
   └─ agent.beforeAgentCallbacks (逐个)
   └─ 如果任一返回 Content → 创建新 Event, EndInvocation()

2. agent.run(ctx)  ← 实际 agent 逻辑 (LLM loop)

3. runAfterAgentCallbacks(ctx)
   ├─ PluginManager.RunAfterAgentCallback  ← plugin hook
   └─ agent.afterAgentCallbacks (逐个)
   └─ 如果任一返回 Content → 创建新 Event
```

`runBeforeAgentCallbacks` (:247-302) 和 `runAfterAgentCallbacks` (:306-360) 的关键行为：
1. **Plugin 先于直接注册的 callbacks 执行**
2. 所有 callback 共享同一个 `EventActions` (含 `StateDelta`)，即使某个 callback 提前返回非 nil Content，前面 callback 的 state delta 也会被写入 Event
3. 如果只有 state delta 没有 Content，也会产出 Event

### 4.4 `agent/llmagent/llmagent.go` — LLMAgent 与 Callback 类型

**文件**: `agent/llmagent/llmagent.go`（490 行）

定义了六种 callback 类型：

```go
// model 维度
BeforeModelCallback  func(CallbackContext, *LLMRequest) (*LLMResponse, error)  // :289
AfterModelCallback   func(CallbackContext, *LLMResponse, error) (*LLMResponse, error)  // :295
OnModelErrorCallback func(CallbackContext, *LLMRequest, error) (*LLMResponse, error)  // :301

// tool 维度
BeforeToolCallback   func(ToolContext, Tool, map[string]any) (map[string]any, error)  // :313
AfterToolCallback    func(ToolContext, Tool, args, result map[string]any, error) (map[string]any, error)  // :322
OnToolErrorCallback  func(ToolContext, Tool, map[string]any, error) (map[string]any, error)  // :328
```

**注释规范**（见每个类型的 godoc）：
- `Before*` callbacks 返回 non-nil → **跳过实际调用**
- `After*` callbacks 返回 non-nil → **替换实际结果**
- `On*Error` callbacks 返回 non-nil → **替换错误为成功/新错误**

**Config 结构** (:130-283):
```go
type Config struct {
    BeforeAgentCallbacks []agent.BeforeAgentCallback
    AfterAgentCallbacks  []agent.AfterAgentCallback
    BeforeModelCallbacks []BeforeModelCallback
    AfterModelCallbacks  []AfterModelCallback
    OnModelErrorCallbacks []OnModelErrorCallback
    BeforeToolCallbacks  []BeforeToolCallback
    AfterToolCallbacks   []AfterToolCallback
    OnToolErrorCallbacks []OnToolErrorCallback
    Instruction          string                    ← 模板字符串
    InstructionProvider  InstructionProvider       ← 动态指令
    GlobalInstruction    string
    GlobalInstructionProvider InstructionProvider
    // ...
}
```

构造函数 `New` (:34-127) 将用户提供的 `[]BeforeModelCallback` 转换为内部的 `[]llminternal.BeforeModelCallback`（类型别名），并在 `llmAgent.run()` 中注入 `Flow`。

`InstructionProvider func(ReadonlyContext) (string, error)` (:490)：以 `ReadonlyContext` 为参数（而非 `CallbackContext`），进一步限制权限——指令生成不应修改 state。

### 4.5 `internal/llminternal/base_flow.go` — Flow 执行引擎

**文件**: `internal/llminternal/base_flow.go`（1376 行）

**`Flow` 结构体** (:62-74):
```go
type Flow struct {
    Model                 model.LLM
    Tools                 []tool.Tool
    RequestProcessors     []func(InvocationContext, *LLMRequest, *Flow) iter.Seq2[*Event, error]
    ResponseProcessors    []func(InvocationContext, *LLMRequest, *LLMResponse) error
    BeforeModelCallbacks  []BeforeModelCallback
    AfterModelCallbacks   []AfterModelCallback
    OnModelErrorCallbacks []OnModelErrorCallback
    BeforeToolCallbacks   []BeforeToolCallback
    AfterToolCallbacks    []AfterToolCallback
    OnToolErrorCallbacks  []OnToolErrorCallback
}
```

**关键方法**:

| 方法 | 行号 | 职责 |
|---|---|---|
| `Run()` | :101-127 | 主循环：重复 `runOneStep` 直到 final response |
| `runOneStep()` | :528-654 | 单步：preprocess → callLLM → postprocess → handleFunctionCalls |
| `preprocess()` | :656-680 | 按序执行 RequestProcessors + tool/toolset preprocess |
| `callLLM()` | :722-800 | 执行 before/after/onError callbacks 封装 LLM 调用 |
| `handleFunctionCalls()` | :1012-1180 | 并行执行 tool 调用 + merge responses |
| `callTool()` | :1193-1238 | 执行 before/after/onError callbacks 封装单个 tool 调用 |

**`callLLM` 执行顺序** (:722-800):
```
1. PluginManager.RunBeforeModelCallback
2. f.BeforeModelCallbacks (逐个, early-exit)
3. generateContent (真实 LLM 调用)
4. if error → PluginManager.RunOnModelErrorCallback → f.OnModelErrorCallbacks
5. PluginManager.RunAfterModelCallback
6. f.AfterModelCallbacks (逐个, early-exit)
```

**`callTool` 执行顺序** (:1193-1238):
```
1. PluginManager.RunBeforeToolCallback
2. f.BeforeToolCallbacks → 如果任一返回 non-nil, 跳过 tool.Run()
3. tool.Run() (如果未被跳过)
4. if error → PluginManager.RunOnToolErrorCallback → f.OnToolErrorCallbacks
5. PluginManager.RunAfterToolCallback
6. f.AfterToolCallbacks
```

**重要设计**: `handleFunctionCalls` (:1029-1173) 使用 goroutine + `sync.WaitGroup` 并行执行多个 tool 调用。每个 goroutine 创建独立的 `ToolContext`（含独立 `StateDelta`），最终通过 `mergeParallelFunctionResponseEvents` (:1287-1313) 合并。

**合并策略** (`mergeEventActions` :1315-1343):
- `SkipSummarization`: 任一为 true 则最终为 true
- `TransferToAgent`: 取最后一个非空值
- `StateDelta`: 深度合并 (recursive `map[string]any`)
- `RequestedToolConfirmations`: `maps.Copy`

### 4.6 `plugin/plugin.go` — Plugin 定义

**文件**: `plugin/plugin.go`（167 行）

`Plugin` 结构体 (`plugin/plugin.go:78-99`):

```go
type Plugin struct {
    name string
    // Runner-level hooks
    onUserMessageCallback, beforeRunCallback, afterRunCallback, onEventCallback
    // Agent-level hooks (复用 agent 包类型)
    beforeAgentCallback, afterAgentCallback
    // Model-level hooks (复用 llmagent 包类型)
    beforeModelCallback, afterModelCallback, onModelErrorCallback
    // Tool-level hooks (复用 llmagent 包类型)
    beforeToolCallback, afterToolCallback, onToolErrorCallback
    // 生命周期
    closeFunc func() error
}
```

**Plugin 级别 hook 类型** (不同于 agent/llmagent callback):

```go
// 定义在 plugin/plugin.go:161-167
type OnUserMessageCallback func(InvocationContext, *genai.Content) (*genai.Content, error)
type BeforeRunCallback func(InvocationContext) (*genai.Content, error)
type AfterRunCallback func(InvocationContext)
type OnEventCallback func(InvocationContext, *session.Event) (*session.Event, error)
```

注意：这 4 个 hook 使用 `InvocationContext`（全权限），因为它们操作的是 runner 级别的生命周期。

**`Config` 到 `Plugin` 的映射** (`plugin/plugin.go:50-76`):
- 所有字段一对一映射
- `CloseFunc` 保证不为 nil（提供空实现兜底）

### 4.7 `internal/plugininternal/plugin_manager.go` — PluginManager

**文件**: `internal/plugininternal/plugin_manager.go`（288 行）

**核心方法**：每个 hook 类型都有对应的 `Run*` 方法，遵循统一的顺序执行 + early-exit 模式。

例如 `RunBeforeModelCallback` (:222-236):
```go
func (pm *PluginManager) RunBeforeModelCallback(cctx agent.CallbackContext, llmRequest *model.LLMRequest) (*model.LLMResponse, error) {
    for _, plugin := range pm.plugins {
        callback := plugin.BeforeModelCallback()
        if callback != nil {                    // 跳过未注册的 hook
            newResponse, err := callback(cctx, llmRequest)
            if err != nil { return nil, err }    // error → 中断
            if newResponse != nil { return newResponse, nil } // non-nil → early exit
        }
    }
    return nil, nil  // 所有 plugin 都返回 nil
}
```

**示例：不同 hook 的参数语义**：

| 方法 | 参数 | 返回值含义 (non-nil) |
|---|---|---|
| `RunOnUserMessageCallback` | `(InvocationContext, *Content)` | 替换用户消息 |
| `RunBeforeRunCallback` | `(InvocationContext)` | 终止 invocation |
| `RunAfterRunCallback` | `(InvocationContext)` | 无返回值 |
| `RunOnEventCallback` | `(InvocationContext, *Event)` | 替换 Event |
| `RunBeforeAgentCallback` | `(CallbackContext)` | 终止 agent run |
| `RunAfterAgentCallback` | `(CallbackContext)` | 添加后置 Event |
| `RunBeforeModelCallback` | `(CallbackContext, *LLMRequest)` | 跳过 LLM 调用 |
| `RunAfterModelCallback` | `(CallbackContext, *LLMResponse, error)` | 替换 LLM 响应 |
| `RunOnModelErrorCallback` | `(CallbackContext, *LLMRequest, error)` | 替换错误为响应 |
| `RunBeforeToolCallback` | `(ToolContext, Tool, map[string]any)` | 跳过 tool 调用 |
| `RunAfterToolCallback` | `(ToolContext, Tool, args, result, error)` | 替换 tool 结果 |
| `RunOnToolErrorCallback` | `(ToolContext, Tool, args, error)` | 替换错误为结果 |

**`Close()` (:273-284)**: 按序调用所有 plugin 的 `Close()`，收集所有 error 并返回合并的 error。注意：即使某个 plugin 的 Close 失败，仍会继续关闭后续 plugin。

**`ToContext()` (:286-288)**: 通过 `context.WithValue` 将 `PluginManager` 注入 `context.Context`，传递 key 定义在 `plugincontext.PluginManagerCtxKey`。

### 4.8 `plugin/functioncallmodifier/plugin.go` — FunctionCallModifier 插件

**文件**: `plugin/functioncallmodifier/plugin.go`（120 行）

**功能**：修改 LLM 请求中的 tool 声明（添加额外参数），并在 LLM 响应中移除这些额外参数，将其值保存到 session state。

**配置** (`FunctionCallModifierConfig` :29-33):
```go
type FunctionCallModifierConfig struct {
    Predicate           func(toolName string) bool           // 匹配需要修改的 tool
    Args                map[string]*genai.Schema             // 要添加的参数 schema
    OverrideDescription func(originalDescription string) string  // 可选：修改描述
}
```

**双阶段流水线**：

1. **BeforeModel** (`beforeModelCallback` :53-89)：遍历 `req.Config.Tools[].FunctionDeclarations`，对匹配 `Predicate` 的声明，用 `maps.Copy` 注入 `cfg.Args` 到 `decl.Parameters.Properties`，可选修改描述。

2. **AfterModel** (`afterModelCallback` :91-119)：遍历 `llmResponse.Content.Parts` 中的 `FunctionCall`，对匹配 `Predicate` 的调用：
   - 删除所有 `cfg.Args` 中的 key
   - 将参数值以 key `{functionCallID}/{argName}` 存入 `ctx.State()`

**设计意义**：这个插件展示了如何在 Before/After 对中实现请求/响应的对称修改——BeforeModel 给 tool 添加额外参数让 LLM 可以填写，AfterModel 在 LLM 填写后将参数剥离并存入 state，防止 tool 实现收到不认识的参数。

### 4.9 `plugin/loggingplugin/logging_plugin.go` — 日志插件

**文件**: `plugin/loggingplugin/logging_plugin.go`（312 行）

**功能**：注册了所有可用的 hook 类型，全面展示插件的能力边界。

**注册的 hook 一览** (New :44-63):

| Hook | 用途 | 返回策略 |
|---|---|---|
| `OnUserMessageCallback` | 打印用户消息、session 信息 | 返回 `nil, nil`（不修改） |
| `BeforeRunCallback` | 打印 invocation 开始 | 返回 `nil, nil` |
| `OnEventCallback` | 打印每个 Event 的详情 | 返回 `nil, nil` |
| `AfterRunCallback` | 打印 invocation 完成 | 无返回值 (void) |
| `BeforeAgentCallback` | 打印 agent 开始 | 返回 `nil, nil` |
| `AfterAgentCallback` | 打印 agent 完成 | 返回 `nil, nil` |
| `BeforeModelCallback` | 打印 LLM 请求（model, system instruction, tools） | 返回 `nil, nil` |
| `AfterModelCallback` | 打印 LLM 响应（content, token usage） | 返回 `nil, nil` |
| `OnModelErrorCallback` | 打印 LLM 错误 | 返回 `nil, nil`（透传 error） |
| `BeforeToolCallback` | 打印 tool 名称、参数 | 返回 `nil, nil` |
| `AfterToolCallback` | 打印 tool 结果 | 返回 `nil, nil` |
| `OnToolErrorCallback` | 打印 tool 错误 | 返回 `nil, nil` |

关键特征：**所有 hook 都返回 `nil, nil`**，即 logging plugin 是**纯观察者模式**，不改变任何执行流。

在 `afterModel` (:243-273) 中，即使传入 error，也返回 `nil, nil` 以透传原始错误——体现了观察者不干预的原则。

### 4.10 `plugin/retryandreflect/plugin.go` — 重试反思插件

**文件**: `plugin/retryandreflect/plugin.go`（274 行）

**功能**：当 tool 调用失败时，自动生成反思提示注入 LLM 上下文，引导 LLM 调整调用参数重试。超过最大重试次数后生成终止提示。

**配置** (`PluginOption`):
```go
WithMaxRetries(3)                    // 最大重试次数 (默认 3)
WithErrorIfRetryExceeded(true)      // 超限后是报 error 还是生成终止提示
WithTrackingScope(Invocation|Global) // 失败计数范围
```

**核心逻辑** (`handleToolError` :147-181):

```
if err == tool.ErrConfirmationRequired || ErrConfirmationRejected:
    return nil, nil  // 不干预 HITL 流程

if maxRetries == 0:
    return createToolRetryExceedMsg() 或 透传 error

currentRetries = counter[toolName] + 1
if currentRetries <= maxRetries:
    return createToolReflectionResponse()  // 注入反思 prompt
else:
    return createToolRetryExceedMsg() 或 透传 error
```

**反思 prompt 模板**：

`reflection.md`（go:embed 内嵌）: 告诉 LLM "你的 tool 调用失败了，这是错误详情和当前参数，请分析原因并重试"。

`exceeded.md`（go:embed 内嵌）: 告诉 LLM "这个 tool 已经失败多次，请停止使用它，尝试其他方式完成任务"。

**返回类型标记** (`reflectAndRetryResponseType` :47): 所有反思响应都带有 `"response_type": "ERROR_HANDLED_BY_REFLECT_AND_RETRY_PLUGIN"` 标记，防止 `afterTool` 清理 failure counter。

**失败重置** (`afterTool` :128-141):
- 成功且不是反思响应 → 重置该 tool 的失败计数
- 反思响应 → 保持失败计数（因为这不是真正的成功）

**并发安全**：使用 `sync.Mutex` 保护 `scopedFailureCounters` map。

### 4.11 `internal/configurable/` — YAML 可配置层

#### 4.11.1 `configurable.go` — Agent 工厂

**文件**: `internal/configurable/configurable.go`（282 行）

定义了四种 agent 的 YAML 配置映射：

```go
type llmAgentYAMLConfig struct {
    baseAgentConfig `yaml:",inline"`     // 公共字段: name, sub_agents, callbacks...
    Model           string
    Instruction     string
    Tools           []ToolConfig
    DisallowTransferToPeers  bool
    DisallowTransferToParent bool
    GenerateContentConfig    *genai.GenerateContentConfig
}
```

`baseAgentConfig` (:66-94) 包含 `BeforeAgentCallbacks` 和 `AfterAgentCallbacks`，类型为 `[]codeConfig`。

`codeConfig` (:36-42):
```go
type codeConfig struct {
    Name   string         `yaml:"name"`           // 注册名称
    Params map[string]any `yaml:"params,omitempty"` // 可选参数
}
```

`resolveCallbacks[T]` (:266-282): 泛型函数，从 `callbackRegistry` 查找已注册的 callback，做类型断言。

#### 4.11.2 `configurable_utils.go` — 注册与查找

**文件**: `internal/configurable/configurable_utils.go`（499 行）

全局注册表：
```go
var (
    registry         = make(map[string]AgentFactory)    // agent_factory
    agentRegistry    = make(map[string]agent.Agent)     // agent instance cache
    toolRegistry     = make(map[string]any)             // tool/toolset factories
    callbackRegistry = make(map[string]any)             // callback functions
)
```

`RegisterCallback(name, callback)` (:274-282): 将任意类型的 callback 注册到全局 map。

**资源解析流程** (`FromConfig` → `newLLMAgent` → `toLLMAgentConfig`):
1. 读取 YAML 文件
2. 解析 `agent_class` 字段，查找 `registry` 中的 `AgentFactory`
3. 调用 `toLLMAgentConfig` 组装 `llmagent.Config`
4. `resolveCallbacks` 从 `callbackRegistry` 查找并恢复 callback 函数

#### 4.11.3 `conformance/callbacks.go` — 示例 Callback

**文件**: `internal/configurable/conformance/callbacks.go`（106 行）

展示了三种典型的 agent callback 模式：

1. **顺序状态修改** (`beforeAgentCallback1` + `beforeAgentCallback2`): callback1 写入 state key，callback2 读取并追加。验证了同一 `CallbackContext` 的 state delta 在 callback 间可见。
2. **快捷返回** (`shortcutAgentExecution`): 检查 state，如果 `conversation_limit_reached` 为 "True"，则返回 skip Content 跳过 agent run。
3. **后置状态修改** (`afterAgentCallback1` + `afterAgentCallback2`): 与 before 对称，在 agent 完成后修改 state。

#### 4.11.4 `conformance/replayplugin/replay_plugin.go` — 重放插件

**文件**: `conformance/replayplugin/replay_plugin.go`（621 行）

**功能**：在测试中重放预录制的 LLM 和 tool 交互，确保 agent 行为确定性。

**注册的 hook**：
- `BeforeRunCallback`: 从 `state["_adk_replay_config"]` 加载录制数据
- `BeforeModelCallback`: 匹配 LLM 请求，返回录制的响应（跳过真实 LLM 调用）
- `BeforeToolCallback`: 匹配 tool 调用，返回录制的响应（跳过真实 tool 调用）
- `AfterRunCallback`: 清理 invocation state

**关键机制**：
- 使用 `cmp.Diff` 做严格断言，验证请求与录制一致
- `invocationReplayState` 跟踪每个 agent 的 replay index，使用 `sync.Cond` 确保并行 agent 的录制按正确顺序消费
- 路径安全：验证录制目录在 `allowedBaseDir` 内，防止路径遍历攻击

### 4.12 Instruction 扩展机制

#### 4.12.1 `internal/llminternal/instruction_processor.go` — 模板变量注入

**文件**: `internal/llminternal/instruction_processor.go`（231 行）

**注册为 RequestProcessor** (`DefaultRequestProcessors` 中第 5 个): `instructionsRequestProcessor` 在每次 LLM 请求前执行。

**执行流程** (`appendInstructions` :72-94):
1. 如果 `InstructionProvider != nil`: 调用 `InstructionProvider(ctx)` 获取动态指令字符串（**不做模板替换**）
2. 如果 `Instruction != ""`: 调用 `InjectSessionState(ctx, instruction)` → 替换 `{var}`, `{artifact.name}` 占位符
3. 通过 `utils.AppendInstructions` 追加到 `req.Config.SystemInstruction`

**`InjectSessionState` (:204-231):**
- 用正则 `{+[^{}]*}+` 匹配所有占位符
- 对每个占位符调用 `replaceMatch`
- 支持 `{var?}` 可选变量（不存在时返回空字符串而非 error）

**`replaceMatch` (:121-164):**
```
{artifact.filename}  → Artifacts.Load() → 返回 Part.Text
{varName}            → Session.State().Get(varName) → 返回 fmt.Sprintf("%v", value)
{app:key}            → 支持 namespace 前缀 (app:, user:, temp:)
{var?}               → 可选变量, 不存在时返回 ""
```

**`isValidStateName` (:187-201):**
- 无前缀: 必须是合法标识符 `^[a-zA-Z_][a-zA-Z0-9_]*$`
- 有前缀: prefix 必须是 `app:`, `user:`, `temp:` 之一

#### 4.12.2 `global instruction`

与 agent instruction 并行处理 (`appendGlobalInstructions` :96-118):
- **只有 root agent 的 **`GlobalInstruction` / `GlobalInstructionProvider` 生效
- 执行顺序: global instruction 先于 agent instruction

#### 4.12.3 `util/instructionutil/instruction.go` — 外部 helper

**文件**: `util/instructionutil/instruction.go`（47 行）

`InjectSessionState(ctx agent.ReadonlyContext, template string) (string, error)`: 
- 将 `ReadonlyContext` 转换为底层 `*icontext.ReadonlyContext`
- 委托给 `llminternal.InjectSessionState(ictx.InvocationContext, template)`
- 用途：在 `InstructionProvider` 中手动调用模板替换

#### 4.12.4 `internal/llminternal/request_confirmation_processor.go` — HITL 确认

**文件**: `internal/llminternal/request_confirmation_processor.go`（172 行）

**注册为 RequestProcessor** (第 4 个): 在 instructions 之前执行。

**功能**：扫描 session events，寻找：
1. 用户回复的 tool confirmation（`function_responses` 中 name 为 `adk_request_confirmation`）
2. 对应的原始 function call（触发确认的 tool）
3. 对已确认的 tool，调用 `f.handleFunctionCalls` 重新执行

**设计要点**：这是一个**请求前置处理器**——在实际 LLM 调用前，先检查是否有待确认的 tool，如果有就恢复执行，这样可以避免 LLM 在等待用户响应期间继续产生不必要的内容。

---

## 5. `extension_points` — 扩展点地图

### 5.1 Lifecycle 全景

```
┌─────────────────────────────────────────────────────────────────────┐
│                       RUNNER LIFECYCLE                               │
│  [OnUserMessage] ──> [BeforeRun] ──> [Agent Loop] ──> [AfterRun]    │
└──────────────────────────────────┬──────────────────────────────────┘
                                   │
            ┌──────────────────────┴──────────────────────┐
            │              AGENT LIFECYCLE                  │
            │  [BeforeAgent] ──> [agent.Run()] ──> [AfterAgent]
            └────────┬──────────────────────────────────────┘
                     │
       ┌─────────────┴──────────────┐
       │         LLM FLOW           │
       │  ┌─ RequestProcessors ─┐   │
       │  │  instruction inject │   │
       │  │  confirmation       │   │
       │  │  contents           │   │
       │  │  tools              │   │
       │  └─────────────────────┘   │
       │  ┌─ callLLM ───────────┐   │
       │  │ BeforeModel (xN)    │   │  ← 修改/跳过
       │  │ LLM Call            │   │
       │  │ OnModelError (xN)   │   │  ← 错误可恢复
       │  │ AfterModel (xN)     │   │  ← 观察/改写
       │  └─────────────────────┘   │
       │  ┌─ ResponseProcessors ─┐  │
       │  └──────────────────────┘  │
       │  ┌─ handleFunctionCalls ─┐ │
       │  │ BeforeTool (xN)      │  │  ← 修改/跳过
       │  │ Tool.Run()           │  │
       │  │ OnToolError (xN)     │  │  ← 错误可恢复
       │  │ AfterTool (xN)       │  │  ← 观察/改写
       │  └───────────────────────┘ │
       └───────────────────────────┘
              │
       [OnEvent] ← 每个 Event 产出时
```

### 5.2 扩展点分类

#### 5.2.1 修改请求类 (Mutate Request)

| 扩展点 | 位置 | 可修改内容 | 实现 |
|---|---|---|---|
| `OnUserMessageCallback` | Runner | 替换用户消息 | plugin hook |
| `BeforeModelCallback` | Flow.callLLM | LLM 请求 (system instruction, tools, messages) | plugin + direct callback |
| `BeforeToolCallback` | Flow.callTool | Tool 参数 | plugin + direct callback |
| `FunctionCallModifier.BeforeModel` | Plugin | Tool 声明 schema | plugin 示例 |
| `RequestProcessors[n]` | Flow.preprocess | LLM 请求 (全局) | pipeline function |
| `instructionsRequestProcessor` | Flow.preprocess | System instruction (模板注入) | pipeline function |

#### 5.2.2 观察/诊断类 (Observe / Diagnose)

| 扩展点 | 位置 | 观察内容 | 实现 |
|---|---|---|---|
| `OnEventCallback` | Runner | 每个产出 Event | plugin hook |
| `AfterModelCallback` | Flow.callLLM | LLM 响应 + token usage | plugin + direct callback |
| `AfterToolCallback` | Flow.callTool | Tool 结果 | plugin + direct callback |
| `LoggingPlugin` (全部 hook) | 各个节点 | 请求/响应/状态 | 纯观察者 |

#### 5.2.3 改变控制流类 (Control Flow)

| 扩展点 | 控制流影响 | 实现 |
|---|---|---|
| `BeforeAgentCallback` | 返回 Content → **终止 agent run**, 跳过 LLM loop | plugin + direct |
| `BeforeModelCallback` | 返回 LLMResponse → **跳过 LLM 调用** | plugin + direct |
| `BeforeToolCallback` | 返回 result → **跳过 tool 调用** | plugin + direct |
| `AfterAgentCallback` | 返回 Content → **追加新 Event** | plugin + direct |
| `AfterModelCallback` | 返回 LLMResponse → **替换 LLM 响应** | plugin + direct |
| `AfterToolCallback` | 返回 result → **替换 tool 结果** | plugin + direct |
| `OnModelErrorCallback` | 返回 LLMResponse → **错误恢复**, 继续执行 | plugin + direct |
| `OnToolErrorCallback` | 返回 result → **错误恢复**, 继续执行 | plugin + direct |
| `RequestConfirmation` | 暂停 agent loop (HITL) | ToolContext method |
| `RetryAndReflect.onToolError` | 注入反思 prompt, 引导 LLM 重试 | plugin |

### 5.3 Plugin vs Callback 边界

| 维度 | Callback | Plugin |
|---|---|---|
| 安装方式 | 直接写入 `llmagent.Config` 或 `agent.Config` | 通过 `PluginManager` 注册，通过 `context.Context` 传递 |
| 生命周期 | 与 agent 实例绑定 | 全局，可 attach 到整个 runner |
| Hook 覆盖 | Agent/Model/Tool level only | 额外覆盖 Runner level (OnUserMessage, BeforeRun, AfterRun, OnEvent) |
| 可组合性 | 静态列表 | 动态列表，但优先级由注册顺序决定 |
| 关闭清理 | 无 | `CloseFunc()` |
| 典型用途 | 单 agent 定制 | 跨 agent 横切关注点（logging, retry, replay） |

---

## 6. `tests` — 测试覆盖矩阵

### 6.1 单元测试覆盖

| 测试文件 | 覆盖内容 | 测试数量 |
|---|---|---|
| `plugin/functioncallmodifier/plugin_test.go` | BeforeModel (添加参数到声明), AfterModel (剥离参数到 state) | 2 函数, 多个 sub-tests |
| `plugin/functioncallmodifier/integration_test.go` | 端到端：Plugin + Runner + Gemini HTTP replay | 3 场景 |
| `plugin/retryandreflect/plugin_test.go` | 构造参数验证, 成功重置计数, 反思响应不重置, 最大重试, errorIfRetryExceeded, scope 隔离 | 6 个 Test 函数 |
| `internal/llminternal/instruction_processor_test.go` | InjectSessionState: state 变量, artifact, 可选变量, 缺失变量 error, 非法变量名, 前缀变量, nil 值 | 11 个 sub-tests |
| `internal/configurable/conformance/callbacks.go` | Before/After agent callback 的注册和状态读写 | (通过 conformance 测试运行) |
| `internal/configurable/conformance/replayplugin/replay_plugin_test.go` | replay plugin 的录制匹配逻辑 | - |
| `internal/configurable/conformance/replayplugin/replay_plugin_internal_test.go` | replay plugin 内部逻辑 | - |
| `agent/llmagent/llmagent_test.go` | LLMAgent 构造和基本行为 | - |
| `agent/llmagent/state_agent_test.go` | Agent 状态处理 | - |
| `agent/llmagent/llmagent_saveoutput_test.go` | OutputKey 保存逻辑 | - |
| `internal/llminternal/base_flow_test.go` | Flow 基本逻辑 | - |
| `internal/llminternal/handle_function_calls_async_test.go` | Tool 并行调用 | - |
| `internal/llminternal/request_confirmation_processor_test.go` | HITL 流程 | - |
| `internal/llminternal/functions_test.go` | Request confirmation event 生成 | - |

### 6.2 测试模式

1. **HTTP Replay 模式**: `functioncallmodifier/integration_test.go` 使用 `testutil.NewGeminiTestClientConfig` + `.httprr` 文件录制真实 Gemini 响应，确保测试可重复
2. **InMemory Session/Artifact**: 所有需要 session 的测试使用 `session.InMemoryService()` 和 `artifact.InMemoryService()`
3. **Mock Context**: `retryandreflect/plugin_test.go` 中自定义 `mockContext` 实现 `agent.ToolContext`

### 6.3 缺失测试

基于代码扫描，以下场景缺少专项测试：
- **callback 顺序与 early-exit 组合**的集成测试（如两个 BeforeModelCallback 互相影响）
- **PluginManager 与 direct callback 的执行顺序**验证测试
- **并行 tool 执行时 state delta merge**的竞态测试
- **Plugin.Close()** 失败处理的行为测试
- **Global vs Invocation scope** 的跨 invocation 状态隔离测试

---

## 7. `risks` — 风险分析

### 7.1 Hook 顺序隐式依赖

**风险等级: HIGH**

Plugin 和 direct callback 的执行顺序是**隐式的、不可配置的**：
- Plugin 总是先于 direct callback
- 同类 hook 按注册顺序执行
- 没有优先级声明或依赖图

**具体证据**：`callLLM` (:724-742) 中 plugin manager 的 `RunBeforeModelCallback` 先执行，然后才是 `f.BeforeModelCallbacks`。如果一个 plugin 在 BeforeModel 中修改了 `req`，后面的 direct callback 将看到修改后的 req——但这是无法显式声明的隐式契约。

**Same for Tools**：`callTool` (:1196-1206) 中 plugin `RunBeforeToolCallback` 先执行，如果它返回了 non-nil result，所有的 `f.BeforeToolCallbacks` 都会被跳过。

### 7.2 State 写穿导致的非事务性

**风险等级: MEDIUM-HIGH**

`callbackContextState.Set()` (`agent/callback_context.go:230-235`) 同时写入 delta 和真实 state。如果 callback A 写入 state，callback B 随后报错，callback A 的写入已经持久化，无法回滚。

```go
// agent/callback_context.go:230-235
func (c *callbackContextState) Set(key string, val any) error {
    if c.ctx.actions != nil && c.ctx.actions.StateDelta != nil {
        c.ctx.actions.StateDelta[key] = val   // delta
    }
    return c.ctx.invocationContext.Session().State().Set(key, val)  // 立即持久化
}
```

这不同于 Python ADK 中 state delta 先暂存、最后统一提交的模式。

### 7.3 Plugin 关闭失败静默风险

**风险等级: MEDIUM**

`PluginManager.Close()` (:273-284) 收集所有 plugin 的关闭错误并合并返回。但调用方可能忽略返回值。此外，没有超时控制——某个 plugin 的 `closeFunc` 可以永久阻塞。

`PluginConfig.CloseTimeout` 字段已定义但未在 Close 方法中使用。

### 7.4 Configurable 层类型安全

**风险等级: MEDIUM**

`resolveCallbacks[T]` (`configurable.go:266-282`) 使用泛型类型断言将 `any` 转为 `T`，但 `callbackRegistry` 存储的是 `any` 类型，编译时无法保证注册的 callback 类型与声明的类型一致。类型不匹配只能在运行时发现。

### 7.5 Replay Plugin 的时间敏感性

**风险等级: LOW-MEDIUM**

`replay_plugin.go:386` 有一行 `time.Sleep(time.Duration(expectedRecording.Index) * time.Millisecond * 10)`，注释标注 `FIXME`，说明这是临时方案，且可能在不同的硬件/网络环境下导致 replay 不准确。

### 7.6 模板注入安全隐患

**风险等级: LOW**

`InjectSessionState` (`instruction_processor.go:204-231`) 将 state 值通过 `fmt.Sprintf("%v", value)` 直接插入 instruction 字符串。如果 state 中包含恶意模板语法（如 `{other_var}`）或特殊字符，不会被二次处理（因为正则只匹配原模板中的占位符）。但写入 instruction 的值直接影响 LLM prompt，可能被利用进行 prompt injection——特别是当 state 来源不可信时（如用户可控的 state key）。

### 7.7 Parallel Tool Call State Delta Merge 竞态

**风险等级: LOW**

`handleFunctionCalls` (:1012-1180) 中每个 goroutine 使用独立的 `ToolContext` 和独立的 `StateDelta`，最终通过 `mergeEventActions`(:1315-1343) 的 `deepMergeMap` 合并。如果两个 tool 修改了同一个 state key 的深层嵌套 map，后合并者会覆盖先合并者的 nested map（因为 `deepMergeMap` 是浅合并）。虽然函数是并发的，但合并发生在 `wg.Wait()` 之后，所以没有数据竞态——但有语义歧义。

### 7.8 Callback Interface 语义不清晰

**风险等级: MEDIUM**

`AfterToolCallback` 的签名 `func(ctx ToolContext, tool Tool, args, result map[string]any, err error) (map[string]any, error)`:

- 如果 callback 返回 `(nil, nil)`, 使用原始 result/error
- 如果 callback 返回 `(newResult, nil)`, 替换 result
- 如果 callback 返回 `(nil, newErr)`, 替换 error
- 如果 callback 返回 `(newResult, newErr)`, 同时替换 result 和 error

但 `invokeAfterToolCallbacks` (:1255-1269) 的实现中，如果 intermediate callback 返回 `(nil, err)`，会立即返回 error 而不再执行后续 callback。这意味着 error recovery 必须在单个 callback 中完成，不能通过链式传递。

---

## 8. `next_questions` — 下一轮追问

1. **PluginManager 的执行顺序是否应该支持显式优先级？** 当前按注册顺序，优先级不可调节。是否应该引入类似 middleware chain 的显式组合机制？

2. **State().Set() 的写穿行为是设计意图还是实现简化？** Python ADK 中 state delta 是延迟提交的，Go 的实现为什么选择立即持久化？这会影响回滚语义。

3. **Plugin 和 Callback 是否应该统一？** 两者大量重复 same hook types (BeforeModel, AfterTool 等)。Plugin 的优势是多了 Runner-level hooks 和 Close 生命周期。能否将 Callback 实现为特殊的 "anonymous plugin"？

4. **Configurable 层如何支持 Plugin？** 当前 `llmAgentYAMLConfig` 和 `baseAgentConfig` 都没有 plugin 字段。如果要通过 YAML 配置 plugin（如 functioncallmodifier），需要扩展 schema。

5. **Early-exit 机制是否应该支持 `continue-on-error` 模式？** 当前任何 callback error 立即中断整个链。对于纯观察类 plugin（如 logging），error 不应阻止核心流程。

6. **`InvocationContext` vs `CallbackContext` vs `ReadonlyContext` 的三层权限模型是否完备？** `CallbackContext` 有写 state 权限但无 endInvocation 权限。ToolContext 有写 state + requestConfirmation 权限但同样无 endInvocation。这种分层是否覆盖了所有场景？

7. **Instruction 模板的 `{app:key}`, `{user:key}`, `{temp:key}` prefix 机制是否应该扩展到所有 state 访问？** 目前只在 instruction 模板中使用 namespace prefix，但 callback 中的 `State().Get()` 不支持自动加前缀。

8. **RetryAndReflect 的 reflection prompt 模板如何定制？** 当前通过 go:embed 内嵌反射模板，用户无法替换。如果用户想用自己的重试策略语言怎么办？

9. **并行 tool 执行中的 state delta 合并是否应该有冲突检测？** 两个 tool 同时修改同一个 state key 时静默覆盖，没有告警或策略选择（last-write-wins / merge / reject）。

10. **Replay Plugin 的 `time.Sleep` hack 何时解决？** FIXME 注释表明这应该通过 `onEvent` callback 的 `curIndex++` 和 `cond.Broadcast` 来解决，当前实现是临时性质的。

11. **`OutputKey` 机制能否通过 Plugin 实现？** `llmAgent.maybeSaveOutputToState` (:441-474) 是硬编码在 agent logic 中的。如果改为 AfterModel plugin hook，可以提高灵活性和可替代性。

12. **HITL RequestConfirmation 与 RetryAndReflect 的交互是否测试过？** RetryAndReflect 在 `handleToolError` 中跳过 `tool.ErrConfirmationRequired`，但如果用户确认后又失败的其他错误，retry 逻辑如何作用？

---

## 附录：关键文件索引

| 文件路径 | 层级 | 职责 |
|---|---|---|
| `agent/context.go` | Public API | CallbackContext, ToolContext, ReadonlyContext 接口定义 |
| `agent/callback_context.go` | Public API | callbackContext 具体实现, state delta, artifact tracking |
| `agent/agent.go` | Public API | Agent, Before/AfterAgentCallback, 执行框架 |
| `agent/llmagent/llmagent.go` | Public API | LLMAgent Config, Model/Tool callback 类型定义 |
| `internal/llminternal/base_flow.go` | Internal | Flow 引擎, callLLM, callTool, 并行 tool 执行 |
| `internal/llminternal/agent.go` | Internal | llminternal.State, InstructionProvider |
| `plugin/plugin.go` | Public API | Plugin 结构体, Runner-level hook 类型 |
| `internal/plugininternal/plugin_manager.go` | Internal | PluginManager (顺序执行 + early-exit) |
| `internal/plugininternal/plugincontext/context.go` | Internal | context key 定义 |
| `plugin/loggingplugin/logging_plugin.go` | 示例 | 全 hook 观察者实现 |
| `plugin/functioncallmodifier/plugin.go` | 功能 | Request/Response 双阶段 tool 声明改写 |
| `plugin/retryandreflect/plugin.go` | 功能 | Tool 错误重试 + LLM 反思引导 |
| `internal/configurable/configurable.go` | Internal | YAML → Agent Config 转换 |
| `internal/configurable/configurable_utils.go` | Internal | Agent/Tool/Callback 全局注册表 |
| `internal/configurable/conformance/callbacks.go` | 测试 | Before/After agent callback 示例 |
| `internal/configurable/conformance/replayplugin/replay_plugin.go` | 测试 | LLM/tool 调用重放 |
| `internal/llminternal/instruction_processor.go` | Internal | InjectSessionState, 模板变量替换 |
| `internal/llminternal/request_confirmation_processor.go` | Internal | HITL tool 确认恢复 |
| `util/instructionutil/instruction.go` | Public API | 外部可用的 InjectSessionState helper |
| `internal/context/callback_context.go` | Internal | NewCallbackContextWithDelta 工厂 |
| `internal/context/readonly_context.go` | Internal | ReadonlyContext 实现 |
