# Chapter 04 - Callback / Plugin / Instruction 深度讲解

> 本章对应教学大纲 Chapter 04：Callback / Plugin / Instruction。
> 代码基线是本机 `rive-adk-go` 复刻版。原版 ADK 的概念会在必要处对照，但讲解以当前代码可验证行为为准。

---

## 0. 本章一句话

Chapter 04 讲的是 agent workflow 的"可插拔控制层"：

> 不改 `Flow` 主循环，也不改业务 `Tool` 和底层 `Model`，仍然可以在关键节点插入日志、缓存、指令拼装、状态注入、审计、错误恢复和结果改写。

如果前几章的主线是：

```text
Runner -> Agent -> Flow -> Model -> Tool -> Event -> Session
```

那么本章关心的是这些插入点：

```text
Runner / Agent lifecycle
  -> before agent hooks
Flow preprocess
  -> RequestProcessors
  -> Instruction processor
Model call
  -> Plugin BeforeModel
  -> direct BeforeModel callbacks
  -> real Model.GenerateContent
  -> Plugin OnModelError / AfterModel
  -> direct AfterModel callbacks
Tool call
  -> Plugin BeforeTool
  -> direct BeforeTool callbacks
  -> real Tool execution
  -> Plugin OnToolError / AfterTool
  -> direct AfterTool callbacks
Flow postprocess
  -> ResponseProcessors
Runner / Agent lifecycle
  -> after agent hooks
```

这不是为了把系统做复杂，而是为了把横切逻辑从核心 workflow 里拿出来：

- 缓存不应该写死在每个模型调用里。
- 日志不应该散落在每个工具函数里。
- 动态 system instruction 不应该让业务 tool 自己拼 prompt。
- 审计、限流、状态写入、错误恢复不应该污染 `Flow` 的主执行逻辑。

本章的重点不是背接口，而是理解三层扩展点各自适合放什么：

| 层 | 解决的问题 | 典型业务动作 |
| --- | --- | --- |
| `Instruction` | 模型调用前怎么构造 system instruction | 从用户画像、会话状态、全局规则拼 prompt |
| `Callback` | 当前 flow/agent 的本地钩子 | 某个 agent 专属缓存、状态标记、工具参数拦截 |
| `Plugin` | 可复用、可注册、跨 agent 的钩子集合 | 通用日志、审计、模型缓存、工具错误恢复 |

---

## 1. 为什么需要 Callback / Plugin / Instruction

先看一个业务场景：

```text
客服 agent 收到用户问题：
1. 根据用户等级、语言偏好和当前工单类型生成 system instruction。
2. 调模型前先查缓存，如果同一个 session 刚问过同类问题，直接返回缓存答案。
3. 调工具前记录审计日志。
4. 工具失败时把底层错误转成用户可读的 fallback result。
5. 每次工具执行后把 "last_tool_name" 写入 session state。
6. 最后把模型响应做一次安全改写或统计埋点。
```

这些逻辑如果都写进 `Flow.Run`，代码会很快变成这样：

```text
Flow:
  if customer support then inject prompt
  if cache enabled then check cache
  if audit enabled then record audit
  if tool failed then recover maybe
  if state tracking enabled then update state
  if safety enabled then rewrite response
  ...
```

这会带来三个问题：

1. `Flow` 主循环看不出真正的 agent 执行链路。
2. 每个项目都会往 `Flow` 里加自己的业务开关。
3. 同一套日志、缓存、审计逻辑无法在多个 agent 之间复用。

所以 ADK Go 复刻版把这些横切逻辑拆成三层：

```text
Instruction Layer
  负责 prompt / system instruction 拼装

Callback Layer
  负责当前 Flow 里的直接钩子

Plugin Layer
  负责可复用、可注册、有名字的钩子集合
```

从 workflow 开发者视角，可以把它们理解成：

- 想影响"模型看到什么规则"：优先看 `instruction`。
- 想给"这个 agent"加一个小钩子：优先看 direct callback。
- 想把一套能力做成"多个 agent 都能用"：优先看 plugin。

---

## 2. 初学者桥：三个概念先分清

### 2.1 Instruction 是模型调用前的 prompt 构造器

`instruction` 不执行模型，也不执行工具。它只是一个 `RequestProcessor`，在模型请求发出前改写 `model.LLMRequest`：

```text
ReadonlyContext + state
  -> Instruction / InstructionProvider
  -> GlobalInstruction / GlobalInstructionProvider
  -> InjectSessionState
  -> req.SystemInstruction
```

它适合做：

- 把 app 级规则加入 system instruction。
- 把 agent 专属角色说明加入 system instruction。
- 根据当前用户输入动态生成 instruction。
- 从 session/app/user/temp state 里插入变量。

例如：

```text
你是一个金融研究助手。

当前用户等级：{user:tier}
当前任务：{current_task}
缺失可选信息：{optional_hint?}
```

`InjectSessionState` 会把模板里的变量替换成 state 里的值。必填变量缺失会报错，可选变量缺失会变成空字符串。

### 2.2 Callback 是 Flow 上的直接钩子

Callback 是挂在 `flow.Flow` 字段上的函数列表，例如：

```go
BeforeModelCallbacks
AfterModelCallbacks
BeforeToolCallbacks
AfterToolCallbacks
BeforeModelCallbacksCtx
AfterModelCallbacksCtx
BeforeToolCallbacksCtx
AfterToolCallbacksCtx
```

它适合做"这个 agent/flow 自己的本地控制"：

- 只给这个 agent 加一次模型缓存。
- 某个工具调用前检查参数。
- 某个模型响应后写 session state。
- 某个工具执行后保存 artifact。

direct callback 的特点是：代码离 `Flow` 很近，适合局部行为，不适合做成平台能力。

### 2.3 Plugin 是可复用的钩子包

`plugin.Plugin` 是一组命名 hook：

```go
plugin.New(plugin.Config{
    Name: "audit",
    BeforeModel: ...,
    AfterModel: ...,
    BeforeTool: ...,
    AfterTool: ...,
})
```

`plugin.Manager` 按注册顺序执行这些 plugin：

```text
Register(plugin-a)
Register(plugin-b)
Register(plugin-c)

RunBeforeModel:
  plugin-a.BeforeModel
  plugin-b.BeforeModel
  plugin-c.BeforeModel
```

它适合做可复用能力：

- 通用日志插件。
- 通用模型缓存插件。
- 通用工具审计插件。
- 通用错误恢复插件。
- 通用成本统计插件。

### 2.4 Context 是 hook 能看到和能改的东西

本章会频繁看到三个 context：

| Context | 权限 | 典型使用 |
| --- | --- | --- |
| `ReadonlyContext` | 只读身份和状态 | instruction provider 读取用户、session、state |
| `CallbackContext` | 只读身份 + 可写 state + artifact/memory service | model/agent callback 写状态、保存 artifact |
| `ToolContext` | `CallbackContext` + tool call id + tool actions + memory search | tool callback 请求确认、搜索 memory、写 tool state |

这三个 context 的区别很重要。不要把所有 hook 都理解成"随便改运行时"。

---

## 3. 本章代码地图

按教学大纲，本章建议按这个顺序读源码：

| 文件 | 重点 | 为什么读 |
| --- | --- | --- |
| `callbackctx/callbackctx.go` | `ReadonlyContext`、`CallbackContext`、`ToolContext` | hook 能看到什么、能写什么 |
| `context/callback_context.go` | context 实现、state write-through、artifact tracking | callback 写状态如何落到 event actions 和 session |
| `plugin/plugin.go` | `Plugin`、`Config`、hook 类型 | plugin 是什么 |
| `plugin/manager.go` | register order、early exit、error propagation | plugin 执行顺序和短路规则 |
| `instruction/instruction.go` | `InjectSessionState`、`RequestProcessor`、`MergeStateView` | instruction 拼装和 state 注入 |
| `instruction/adapter.go` | `ToRequestProcessor` | instruction 如何接入 `flow.RequestProcessor` |
| `flow/flow.go` | preprocess、model hook、tool hook、postprocess | 三层扩展点如何进入主循环 |
| `runner/chapter04_test.go` | 端到端行为验证 | 用测试确认教学行为 |
| `cmd/demo/main.go` | `runChapter04` demos | 课堂演示入口 |

一个大图：

```text
Runner.Run
  -> InvocationContext
  -> Agent.RunWithCallbackContext
       Plugin BeforeAgent
       direct BeforeAgent
       Agent.Run
         Flow.Run
           preprocess(RequestProcessors)
             instruction processor
           callModel
             Plugin BeforeModel
             direct BeforeModel
             Model.GenerateContent
             Plugin OnModelError
             Plugin AfterModel
             direct AfterModel
           executeToolCall
             Plugin BeforeTool
             direct BeforeTool
             Tool.Execute
             Plugin OnToolError
             Plugin AfterTool
             direct AfterTool
           postprocess(ResponseProcessors)
       Plugin AfterAgent
       direct AfterAgent
```

---

## 4. CallbackContext：hook 的权限边界

### 4.1 ReadonlyContext：只读身份和只读 state

`callbackctx.ReadonlyContext` 暴露的是 identity 和 read-only state：

```go
type ReadonlyContext interface {
    UserContent() *event.Content
    InvocationID() string
    AgentName() string
    ReadonlyState() session.State
    UserID() string
    AppName() string
    SessionID() string
    Branch() string
}
```

它适合给 instruction provider 用，因为 provider 只需要知道：

- 当前用户输入是什么。
- 当前 app/user/session 是谁。
- 当前 state 里有什么。
- 当前 agent 是谁。

它不应该直接结束 invocation，不应该执行工具，也不应该写状态。

### 4.2 CallbackContext：可以写 state、保存 artifact、访问 memory service

`callbackctx.CallbackContext` 嵌入 `ReadonlyContext`，并增加：

```go
ArtifactService() artifact.Service
MemoryService() memory.Service
State() session.State
```

这让 model/agent callback 可以做横切动作：

```go
ctx.State().Set("last_model_callback", "before")
ctx.ArtifactService().Save(...)
ctx.MemoryService()...
```

注意这里有一个关键点：`State()` 是可写的。

### 4.3 ToolContext：工具调用现场的专用上下文

`callbackctx.ToolContext` 又在 `CallbackContext` 上增加：

```go
FunctionCallID() string
Actions() *event.EventActions
SearchMemory(ctx context.Context, query string) ([]memory.Memory, error)
```

它适合 tool callback 和 context-aware tool 使用：

- 知道当前 function call id。
- 往当前 tool event 的 actions 里写确认请求、状态变更等。
- 调用 memory service 搜索长期记忆。

这也是为什么工具相关 hook 用 `ToolContext`，模型相关 hook 用 `CallbackContext`。

### 4.4 State write-through：不是事务

`context/callback_context.go` 里的 `callbackContextState.Set` 做了两件事：

```text
1. 写 event actions.StateDelta
2. 立刻写 durable session state
```

所以 callback 里：

```go
ctx.State().Set("model_callback", "ran")
```

会同时让当前 event 带上：

```text
Actions.StateDelta["model_callback"] = "ran"
```

并且让 session 里马上能读到这个值。

`context/callback_context_test.go` 里的测试覆盖了这些行为：

- `TestCallbackStateWriteThrough`：写 callback state 后 session state 也更新。
- `TestCallbackStateGetPriorityDeltaFirst`：同一步内 delta 优先于原 session state。
- `TestCallbackStateIntraStepDeltaVisible`：同一步内后续读取能看到前面写入。
- `TestCallbackStateDeltaAcrossCallbacks`：多个 callback 之间能看到彼此写的 state delta。
- `TestCallbackStateDelete`：删除会写 tombstone 并删除 durable state。

这对业务很有用，但也有风险：

```text
BeforeModel callback 写了 state
  -> 后续 callback 报错
  -> 已经写入 session 的 state 不会自动回滚
```

所以本章要强调：

> callback state write-through 是立即生效的，不是事务提交，不带自动 rollback。

### 4.5 Artifact tracking：保存 artifact 会反映到 EventActions

`NewCallbackContextWithArtifactTracking` 会把 artifact service 包一层 tracking wrapper。

在 callback 里保存 artifact：

```go
ctx.ArtifactService().Save(ctx, &artifact.SaveRequest{
    FileName: "model-note.txt",
    Part: &artifact.ArtifactPart{Text: "model callback"},
})
```

当前 event 的 actions 会记录：

```text
Actions.ArtifactDelta["model-note.txt"] = version
```

`TestArtifactSaveTracking` 和 `TestArtifactSaveTrackingMultiple` 验证了这个行为。`runner/chapter04_test.go` 里的 context-aware model/tool callback 测试也验证了 artifact delta 会出现在对应 event 上。

---

## 5. Plugin：可复用 hook 集合

### 5.1 Plugin 的结构

`plugin.Plugin` 的核心是一个名字加一组可选 hook：

```go
type Config struct {
    Name string

    BeforeAgent ...
    AfterAgent ...

    BeforeModel ...
    AfterModel ...
    OnModelError ...

    BeforeTool ...
    AfterTool ...
    OnToolError ...
}
```

所有 hook 都是可选的。一个 plugin 可以只关心模型，也可以只关心工具。

例如日志 plugin：

```go
logPlugin := plugin.New(plugin.Config{
    Name: "logger",
    BeforeModel: func(ctx callbackctx.CallbackContext, req *model.LLMRequest) (*model.LLMResponse, error) {
        log.Printf("before model: %s", ctx.AgentName())
        return nil, nil
    },
    AfterModel: func(ctx callbackctx.CallbackContext, req *model.LLMRequest, resp *model.LLMResponse, runErr error) (*model.LLMResponse, error) {
        log.Printf("after model: err=%v", runErr)
        return nil, nil
    },
})
```

这里返回 `nil, nil` 的意思是：我只是观察，不改写、不短路。

### 5.2 Manager 按注册顺序执行

`plugin.Manager.Register` 只是 append：

```text
Register(plugin-a)
Register(plugin-b)
Register(plugin-c)
```

执行时就是：

```text
plugin-a
plugin-b
plugin-c
```

`TestManagerRegistrationOrder` 验证了 BeforeModel 按注册顺序调用。`TestManagerSkipsNilHooks` 验证了没有实现对应 hook 的 plugin 会被跳过。

这意味着 plugin 之间的优先级不是单独配置出来的，而是注册顺序。

如果一个项目需要：

```text
auth -> rate limit -> cache -> telemetry
```

就必须按这个顺序注册。

### 5.3 Before* early exit：第一个非 nil 结果短路

`RunBeforeModelCallback` 的规则是：

```text
for plugin in plugins:
  if plugin.BeforeModel == nil:
    continue
  resp, err := plugin.BeforeModel(...)
  if err != nil:
    return nil, err
  if resp != nil:
    return resp, nil
return nil, nil
```

也就是说：

- 返回 `nil, nil`：继续后面的 plugin/hook。
- 返回 `resp, nil`：短路，使用这个 fake/cached response。
- 返回 `nil, err`：失败，停止当前链路。

工具也是一样：

- `BeforeTool` 返回 `map[string]any` 会短路真实工具执行。
- `BeforeAgent` 返回 `*event.Event` 会短路真实 agent run。

测试：

- `TestManagerEarlyExitBeforeAgent`
- `TestManagerEarlyExitBeforeModel`
- `TestManagerEarlyExitBeforeTool`

### 5.4 OnError：错误恢复点

Plugin 还有两个 error hook：

```go
OnModelError
OnToolError
```

它们的意义不是"记录错误"这么简单，而是可以恢复错误。

模型调用失败时：

```text
Model.GenerateContent returns error
  -> Plugin OnModelError
  -> if plugin returns response, recover
```

工具执行失败时：

```text
Tool.Execute returns error
  -> Plugin OnToolError
  -> if plugin returns result, recover
```

测试：

- `TestManagerOnModelErrorRecovery`
- `TestManagerOnToolErrorRecovery`
- `TestManagerOnModelErrorNoError`
- `TestManagerOnToolErrorNoError`

这适合做：

- fallback model。
- tool result 降级。
- 把底层错误转成结构化业务 result。
- 对特定错误类型自动重试或替换。

---

## 6. Flow 里的真实 hook 顺序

### 6.1 preprocess：RequestProcessor 先于模型调用

`flow.preprocess` 会按顺序运行：

```go
for _, processor := range f.RequestProcessors {
    ev, err := processor.ProcessRequest(ctx, req)
    if ev != nil || err != nil {
        return ev, err
    }
}
```

这说明 `RequestProcessor` 可以：

- 修改 request。
- 生成一个 event 直接短路当前 step。
- 返回 error 终止当前 step。

`instruction.ToRequestProcessor` 就是通过这里接入 `Flow` 的。

### 6.2 model path：plugin 在 direct callback 之前

当前复刻版 `flow.callModel` 的顺序是：

```text
create CallbackContext

PluginManager.RunBeforeModelCallback
BeforeModelCallbacksCtx
BeforeModelCallbacks

Model.GenerateContent

if model error:
  PluginManager.RunOnModelErrorCallback

PluginManager.RunAfterModelCallback
AfterModelCallbacksCtx
AfterModelCallbacks
```

`runner/chapter04_test.go` 里的 `TestRunnerPluginOrdering` 验证了顺序：

```text
plugin:beforeModel
direct:beforeModel
plugin:afterModel
direct:afterModel
```

所以教学时要把这句话讲清楚：

> 同一个 hook 点上，Plugin 先于 direct callback。

### 6.3 重要差异：当前复刻版 BeforeModel early-exit 会跳过 AfterModel

教学大纲里提醒了一个容易错的点：

```text
BeforeModel 返回非 nil 时跳过真实 LLM，但后续 AfterModel 仍会在 fake response 上执行。
```

这是一个重要概念，但当前本机 `rive-adk-go` 复刻版源码不是这个行为。

在当前 `flow.callModel` 中：

```text
Plugin BeforeModel returns non-nil response
  -> callModel immediately returns that response
  -> direct BeforeModel callbacks are skipped
  -> real Model.GenerateContent is skipped
  -> AfterModel plugin/callbacks are skipped
```

direct `BeforeModel` 返回非 nil 时也是同样逻辑：

```text
BeforeModelCallbacksCtx returns response
  -> immediate return
  -> later direct callbacks and AfterModel callbacks are skipped
```

`TestRunnerPluginBeforeModelEarlyExit` 也体现了这个行为：plugin before model 返回 cached response 后，真实 model 没有被调用，最终只有一条模型 event。

所以本章按"当前代码可验证行为"教学：

> 在当前复刻版里，BeforeModel early-exit 是真正的 immediate return，不会继续跑 AfterModel。

如果未来要对齐原版 ADK 或大纲预期，需要改 `flow.callModel` 的控制流，让 fake response 继续进入 after hooks。

### 6.4 tool path：BeforeTool 可以短路真实工具

`flow.executeToolCall` 的工具链路是：

```text
create ToolContext

PluginManager.RunBeforeToolCallback
BeforeToolCallbacksCtx
BeforeToolCallbacks

lookup tool
execute real tool

if tool error:
  PluginManager.RunOnToolErrorCallback

PluginManager.RunAfterToolCallback
AfterToolCallbacksCtx
AfterToolCallbacks
```

如果 `BeforeTool` 返回 result：

```text
Plugin BeforeTool returns {"status": "mocked"}
  -> real tool is skipped
  -> FunctionResponse uses mocked result
```

`TestRunnerPluginBeforeToolEarlyExit` 验证了这个行为。

### 6.5 tool error recovery 后仍可进入 after hooks

工具执行出错后，plugin 的 `OnToolError` 可以返回一个 recovery result。

然后 `applyAfterToolPluginAndCallbacks` 会继续把 result 交给 after hooks：

```text
tool run error
  -> Plugin OnToolError recovers with result
  -> Plugin AfterTool
  -> direct AfterTool callbacks
  -> FunctionResponse
```

这很适合做业务 fallback：

```text
inventory_tool failed because upstream timeout
  -> OnToolError returns {"available": false, "source": "fallback"}
  -> AfterTool records degraded mode
```

### 6.6 postprocess：ResponseProcessor 最后处理 model response

`flow.postprocess` 会按顺序运行 ResponseProcessor：

```text
model response event
  -> ResponseProcessors
  -> final event
```

它适合做：

- 最终响应改写。
- 安全过滤。
- 结构化输出校验。
- 统一 telemetry。

和 `AfterModel` 的区别是：`AfterModel` 仍在模型调用语义里；`ResponseProcessor` 是 Flow 生成 event 前后的处理层。

---

## 7. Instruction：把业务状态变成 SystemInstruction

### 7.1 Instruction 本质上是 RequestProcessor

`instruction.NewRequestProcessor` 返回的是一个只读 processor。再通过：

```go
instruction.ToRequestProcessor(processor)
```

适配到 `flow.RequestProcessor`。

这意味着 instruction 发生在模型调用之前：

```text
Flow.preprocess
  -> Instruction processor mutates req.SystemInstruction
Flow.callModel
  -> model sees req.SystemInstruction
```

所以它适合做 prompt 拼装，而不是做模型响应改写或工具拦截。

### 7.2 四类 instruction 会合并

`instruction.Config` 里有四种来源：

```go
GlobalInstruction
GlobalInstructionProvider
Instruction
InstructionProvider
```

处理顺序是：

```text
1. GlobalInstruction              root agent only
2. GlobalInstructionProvider      root agent only
3. Instruction
4. InstructionProvider
```

它们不是互斥关系。非空内容会按顺序 join，中间用空行分隔。

`TestNewRequestProcessorGlobalInstruction` 验证 root agent 会包含 global + agent instruction。

`TestNewRequestProcessorGlobalInstructionNotRoot` 验证非 root agent 不会包含 global instruction。

### 7.3 Provider 可以读取 readonly context

静态 instruction 是固定字符串：

```go
Instruction: "You are a helpful assistant."
```

provider 是动态函数：

```go
InstructionProvider: func(ctx callbackctx.ReadonlyContext) (string, error) {
    return "User asks: " + firstText(ctx.UserContent()), nil
}
```

这适合根据当前输入或状态拼 instruction：

```text
如果用户问的是退款，使用 refund policy。
如果用户问的是技术问题，使用 troubleshooting policy。
如果用户等级是 enterprise，加入 SLA 说明。
```

`TestNewRequestProcessorDynamicProvider` 和 `TestRunnerDynamicInstructionProvider` 都验证 provider 能看到当前 user content。

### 7.4 InjectSessionState：模板变量替换

`InjectSessionState` 支持这几类 placeholder：

| 模板 | 含义 |
| --- | --- |
| `{varName}` | session/default scope 的必填变量 |
| `{varName?}` | session/default scope 的可选变量 |
| `{app:key}` | app scope 变量 |
| `{user:key}` | user scope 变量 |
| `{temp:key}` | temp scope 变量 |

例子：

```text
User tier: {user:tier}
Current task: {current_task}
Optional hint: {optional_hint?}
App policy: {app:policy}
```

如果 `{current_task}` 不存在，会返回 error。

如果 `{optional_hint?}` 不存在，会替换成空字符串。

`TestInjectSessionState` 覆盖了：

- 普通 session 变量。
- app scoped 变量。
- user scoped 变量。
- optional exists / absent。
- multiple placeholders。
- required missing error。

### 7.5 MergeStateView：把 app/user/session state 合成模板可读视图

`MergeStateView` 会把不同 scope 合成一个 flat map：

```text
app:policy      -> state["app:policy"]
user:tier       -> state["user:tier"]
session key     -> state["current_task"]
```

这让 instruction 模板可以同时读取多种层级的状态。

`TestRunnerInstructionTemplateInjection` 手动构造 merged state，然后验证：

```text
Hi Ada, tier gold, task triage
```

进入了 `SystemInstruction`。

### 7.6 当前复刻版没有 artifact placeholder

当前 `instruction/instruction.go` 的 placeholder regex 支持的是：

```text
{varName}
{varName?}
{app:key}
{user:key}
{temp:key}
```

本机复刻版没有实现类似 `{artifact.name}` 的 instruction placeholder。讲课时不要把旧笔记或其他实现里的 artifact placeholder 当成当前代码行为。

---

## 8. Agent lifecycle：BeforeAgent / AfterAgent

除了模型和工具，plugin 也能挂 agent 生命周期。

`context/callback_context.go` 里的 agent 包装逻辑可以概括为：

```text
RunWithCallbackContext
  -> Plugin BeforeAgent
  -> context-aware direct BeforeAgent callbacks
  -> Agent.Run
  -> Plugin AfterAgent
  -> context-aware direct AfterAgent callbacks
```

`BeforeAgent` 可以返回 event 来提前结束 agent run。

`AfterAgent` 可以看到 agent run 产生的 events，并返回额外 event 或标记结束。

这适合做：

- agent 级权限检查。
- agent 级缓存。
- invocation 级审计。
- agent 运行前后的 telemetry。

但不要把它和 model/tool hook 混淆：

- `BeforeAgent` 是整个 agent run 前。
- `BeforeModel` 是某一次模型调用前。
- `BeforeTool` 是某一次工具调用前。

一个 agent run 可能有多轮 model/tool step，所以 model/tool hook 可能执行多次。

---

## 9. 课堂演示：从业务动作进入代码

`cmd/demo/main.go` 的 `runChapter04` 提供了四个演示方向。

### 9.1 demoPluginLogging：纯观察型 plugin

这个 demo 适合第一段展示：

```text
BeforeModel: 记录模型调用前信息
AfterModel: 记录模型调用后信息
BeforeTool: 记录工具调用前信息
AfterTool: 记录工具调用后信息
```

重点讲：

- hook 返回 `nil, nil` 表示只观察。
- plugin 可以覆盖 model 和 tool 两条链路。
- 日志逻辑不需要写进 tool function。

### 9.2 demoBeforeModelCache：模型缓存短路

这个 demo 展示：

```text
BeforeModel sees user content
  -> cache hit
  -> return cached LLMResponse
  -> skip real model
```

业务含义：

```text
FAQ / deterministic answer / high-cost model call
  -> before model cache
```

要强调当前复刻版行为：

- cache hit 后真实 model 不执行。
- `AfterModel` 也不会执行。
- 如果需要 cache hit 后仍做 telemetry，应在 cache plugin 内部记录，或调整 Flow 控制流。

### 9.3 demoInstructionInterpolation：状态驱动 instruction

这个 demo 展示：

```text
session state:
  user_name = Ada
  user_role = analyst
  current_task = incident triage

RequestProcessor:
  build SystemInstruction from state
```

课堂上可以把它讲成：

> 业务系统不用把所有上下文塞进用户消息，可以把稳定的角色、任务、偏好放在 state 里，然后由 instruction 层统一拼成 system instruction。

### 9.4 demoPluginOrdering：注册顺序和 direct callback 顺序

这个 demo 展示：

```text
plugin-a before
plugin-b before
direct before
real model
plugin-a after
plugin-b after
direct after
```

重点讲：

- plugin 内部按注册顺序。
- plugin 先于 direct callback。
- direct callback 适合当前 flow 本地逻辑。
- plugin 适合可复用逻辑。

---

## 10. 误区和边界

### 10.1 误区：Plugin 和 Callback 是同一种东西

它们确实都叫 hook，但生命周期和复用方式不同。

Callback：

```text
挂在 Flow 字段上
更靠近当前 agent
适合局部定制
```

Plugin：

```text
挂在 PluginManager 上
有 Name
可以注册多个
适合跨 agent 复用
```

如果只是给一个 demo flow 加一个小行为，callback 就够了。

如果要做"所有 agent 都要用的日志/审计/缓存"，用 plugin。

### 10.2 误区：BeforeModel 返回假响应后 AfterModel 一定会跑

在当前本机复刻版里，不会。

当前行为是：

```text
BeforeModel returns response
  -> immediate return
  -> skip real model
  -> skip after model hooks
```

这是源码可验证行为，也被 Chapter 04 runner 测试覆盖。

如果要讲原版 ADK 或大纲预期，需要明确标注那是另一个语义，不要混在当前代码讲解里。

### 10.3 误区：state delta 只是 event 上的临时值

不是。

callback state 写入会：

```text
write EventActions.StateDelta
write durable session state
```

它不是事务，也没有自动 rollback。

### 10.4 误区：InstructionProvider 会替代 Instruction

不会。它们会累加。

顺序是：

```text
GlobalInstruction
GlobalInstructionProvider
Instruction
InstructionProvider
```

只不过 global 只在 root agent 生效。

### 10.5 误区：Plugin 有复杂优先级系统

当前没有。

plugin manager 用注册顺序作为优先级。

如果顺序重要，就在注册处显式管理顺序，并给 plugin 命名清楚。

### 10.6 误区：Instruction 模板可以读所有运行时对象

当前不能。

它只能读传入的 state map，并按 placeholder 规则替换。

如果你需要 artifact、memory、外部数据库等动态信息，应该通过 provider 自己读取，或先把需要的信息写入 state，再做 template injection。

### 10.7 误区：CallbackContext 可以随便控制 invocation

`CallbackContext` 有可写 state、artifact service、memory service，但不是完整 `InvocationContext`。

它没有直接暴露 `EndInvocation` 这样的全局控制能力。工具确认、tool actions 这类能力在 `ToolContext` / `EventActions` 里体现。

这也是权限边界：hook 能做横切增强，但不应该拿到整个 runtime 的所有控制权。

---

## 11. 源码行为对照表

| 行为 | 当前复刻版结论 | 证据 |
| --- | --- | --- |
| Plugin 注册顺序 | append 顺序执行 | `plugin/manager.go`、`TestManagerRegistrationOrder` |
| nil hook | 跳过 | `TestManagerSkipsNilHooks` |
| BeforeAgent early exit | 返回 event 后停止后续 before hook 和 agent run | `TestManagerEarlyExitBeforeAgent` |
| BeforeModel early exit | 返回 response 后 immediate return，跳过真实 model 和 after model hook | `flow/flow.go`、`TestRunnerPluginBeforeModelEarlyExit` |
| BeforeTool early exit | 返回 result 后跳过真实 tool | `TestManagerEarlyExitBeforeTool`、`TestRunnerPluginBeforeToolEarlyExit` |
| Plugin vs direct model callback | plugin 先于 direct callback | `TestRunnerPluginOrdering` |
| AfterModel 改写 response | plugin/direct after 可替换 response | `TestManagerAfterModelReplaceResponse`、`TestRunnerPluginAfterModelTransform` |
| OnModelError | 可以恢复成 response | `TestManagerOnModelErrorRecovery` |
| OnToolError | 可以恢复成 result | `TestManagerOnToolErrorRecovery` |
| Instruction root global | root agent 才拼 global instruction | `TestNewRequestProcessorGlobalInstruction`、`TestNewRequestProcessorGlobalInstructionNotRoot` |
| Instruction provider | 能读取 user content | `TestNewRequestProcessorDynamicProvider`、`TestRunnerDynamicInstructionProvider` |
| Template required missing | 返回 error | `TestInjectSessionState` |
| Template optional missing | 替换为空字符串 | `TestInjectSessionState` |
| State write-through | 写 delta 也写 session | `TestCallbackStateWriteThrough` |
| Delta priority | 同一步读取优先看 delta | `TestCallbackStateGetPriorityDeltaFirst` |
| Artifact tracking | Save 后写 ArtifactDelta | `TestArtifactSaveTracking` |

---

## 12. 建议课堂脚本

### 12.1 先用业务场景建立动机

不要一上来讲接口定义。先问：

```text
如果我要给所有模型调用加缓存，应该改哪里？
如果我要给某个工具调用前做审批，应该改哪里？
如果我要根据用户等级拼 system prompt，应该改哪里？
如果工具失败，我想返回 fallback result，应该改哪里？
```

然后给出三层答案：

```text
Instruction: prompt construction
Callback: local flow customization
Plugin: reusable hook package
```

### 12.2 再画执行链路

用这一张图：

```text
RequestProcessor / Instruction
  -> Plugin BeforeModel
  -> direct BeforeModel
  -> Model
  -> Plugin AfterModel
  -> direct AfterModel
  -> FunctionCall?
      -> Plugin BeforeTool
      -> direct BeforeTool
      -> Tool
      -> Plugin AfterTool
      -> direct AfterTool
```

然后补充当前复刻版 early-exit 特例：

```text
BeforeModel returns response
  -> current replica returns immediately
  -> AfterModel skipped
```

### 12.3 接着读源码

推荐顺序：

1. `callbackctx/callbackctx.go`：先看 hook 能拿到什么。
2. `plugin/plugin.go`：看 plugin 是一组可选 hook。
3. `plugin/manager.go`：看注册顺序、nil skip、early exit、error recovery。
4. `instruction/instruction.go`：看 instruction 合并和 placeholder 注入。
5. `flow/flow.go`：把三层 hook 放回主循环。
6. `context/callback_context.go`：看 state write-through 和 artifact tracking。

### 12.4 最后跑 demo 和测试

课堂 demo：

```bash
go run ./cmd/demo -chapter=4
```

行为验证：

```bash
go test ./plugin ./instruction ./context ./runner ./flow -run 'TestManager|TestInjectSessionState|TestNewRequestProcessor|TestCallbackState|TestArtifactSaveTracking|TestRunnerPlugin|TestRunnerInstruction|TestFlowBeforeModelCallbackShortCircuit|TestFlowBeforeToolCallbackOverride' -v
```

---

## 13. 练习题

### 13.1 练习一：写一个模型缓存 plugin

目标：

```text
如果用户输入命中 cache map，BeforeModel 返回 cached LLMResponse。
否则返回 nil，让真实 model 执行。
```

要求：

- cache key 至少包含 user text。
- cache hit 时写 `ctx.State().Set("cache_hit", true)`。
- 测试真实 model 没有被调用。
- 说明当前复刻版 cache hit 后 AfterModel 是否会执行。

参考测试：

- `TestRunnerPluginBeforeModelEarlyExit`

### 13.2 练习二：写一个工具审计 plugin

目标：

```text
BeforeTool 记录 toolName 和 args。
AfterTool 记录 result 或 error。
```

要求：

- 不改变工具执行结果。
- 用 state 记录 `last_tool_name`。
- 如果工具返回 error，用 `OnToolError` 返回 fallback result。

参考测试：

- `TestRunnerContextAwareToolCallbacksMergeActionsAndOrdering`
- `TestManagerOnToolErrorRecovery`

### 13.3 练习三：写一个动态 instruction provider

目标：

```text
根据 user tier 和 current task 拼 system instruction。
```

要求：

- 使用 `{user:tier}` 和 `{current_task}`。
- `{optional_hint?}` 缺失时不报错。
- root agent 时包含 global instruction。
- 非 root agent 时不包含 global instruction。

参考测试：

- `TestInjectSessionState`
- `TestNewRequestProcessorGlobalInstruction`
- `TestNewRequestProcessorGlobalInstructionNotRoot`
- `TestRunnerInstructionTemplateInjection`

### 13.4 练习四：验证 state write-through 风险

目标：

```text
BeforeModel callback 写 state。
后续 callback 返回 error。
观察 session state 是否已经被写入。
```

讨论：

- 为什么这不是事务？
- 如果业务需要 rollback，应放在哪里做？
- event actions 和 durable session state 分别有什么用？

参考测试：

- `TestCallbackStateWriteThrough`
- `TestCallbackStateDeltaAcrossCallbacks`

---

## 14. 自测题

1. `InstructionProvider` 和 `BeforeModel` callback 都在模型调用前发生，它们的职责有什么不同？
2. plugin manager 如何决定多个 plugin 的执行顺序？
3. `BeforeTool` 返回非 nil result 后，真实 tool 会不会执行？
4. 当前复刻版中，`BeforeModel` 返回 cached response 后，`AfterModel` 会不会执行？
5. 为什么 `CallbackContext.State().Set` 不是安全的事务写？
6. `{current_task}` 和 `{current_task?}` 在缺失时有什么区别？
7. global instruction 在非 root agent 上会不会生效？
8. direct callback 和 plugin 同时存在时，谁先执行？
9. `ToolContext` 比 `CallbackContext` 多了哪些能力？
10. 保存 artifact 后，event actions 里会出现什么变化？

参考答案：

1. `InstructionProvider` 负责构造 request 的 system instruction；`BeforeModel` 负责模型调用前的横切控制，可以短路或改写模型请求。
2. 按 `Register` append 顺序执行。
3. 不会。返回 result 会短路真实 tool execution。
4. 当前复刻版不会执行。`callModel` immediate return。
5. 因为它同时写 `EventActions.StateDelta` 和 durable session state，后续错误不会自动回滚。
6. `{current_task}` 缺失会报错；`{current_task?}` 缺失会替换为空字符串。
7. 不会。global instruction 只在 root agent 生效。
8. plugin 先执行，direct callback 后执行。
9. function call id、event actions、memory search 等工具调用现场能力。
10. `Actions.ArtifactDelta[fileName] = version`。

---

## 15. 本章收束

Chapter 04 的核心不是"多了几种 hook"，而是学会把 agent workflow 的横切逻辑放在正确层：

```text
Instruction:
  让模型在正确的规则和上下文里工作。

Callback:
  给当前 agent/flow 加局部控制点。

Plugin:
  把可复用的日志、缓存、审计、恢复能力做成可注册组件。

CallbackContext / ToolContext:
  在 hook 里提供受控权限，允许写 state、保存 artifact、搜索 memory、访问 tool actions。
```

最终目标是：

> 核心 Flow 保持干净，业务 workflow 仍然能被精细控制。

这就是 Callback / Plugin / Instruction 在 agent workflow 开发里的价值。
