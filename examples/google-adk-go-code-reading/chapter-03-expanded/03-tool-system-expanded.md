# Chapter 03 - Tool System: Declaration / Execution / Streaming / Confirmation 深度讲解

> 本章对应教学大纲 Chapter 03：Tool System: Declaration / Execution / Streaming / Confirmation。
> 代码基线是本机 `rive-adk-go` 复刻版。原版 ADK 的概念会在必要处对照，但讲解以当前代码可验证行为为准。

---

## 0. 本章一句话

Tool System 解决的是一个非常具体的问题：

> 模型只能输出文本或结构化 JSON 意图，真正的业务动作必须由本地代码执行。Tool System 就是把"模型想调用某个能力"安全、可描述、可执行、可回灌地接进 agent workflow。

如果 Chapter 01 讲的是：

```text
Runner -> Agent -> Flow -> Model -> Event -> Session
```

那么 Chapter 03 讲的是中间这段：

```text
Model 产出 FunctionCall
  -> Flow 找到本地 Tool
  -> Tool 执行业务代码
  -> Flow 生成 FunctionResponse event
  -> 下一轮 Model 看到 tool result 后继续回答
```

这不是"给模型一个 Go 函数"这么简单。模型不会直接执行 Go 代码，它只能看到 tool declaration，输出 tool name + JSON args。运行时负责把这份 intent 找到对应函数、执行、封装结果、再塞回上下文。

---

## 1. 为什么 Tool System 难

先从一个业务例子开始：

```text
用户：帮我查一下 Tokyo 天气，然后告诉我能不能出门跑步。
```

模型本身不知道实时天气。它应该做的是：

```json
{
  "name": "get_weather",
  "arguments": {
    "city": "Tokyo"
  }
}
```

但这只是模型输出的结构化意图。要真的查天气，还需要运行时做五件事：

1. 在请求模型前，把 `get_weather` 的名字、描述、参数 schema 告诉模型。
2. 模型返回 function call 后，把 JSON args 解成运行时的参数 map。
3. 根据 tool name 找到本地注册的 Go function。
4. 执行 Go function，拿到结构化 result 或 error。
5. 把 result 变成 `FunctionResponse`，在下一轮 model request 中作为 tool message 回灌给模型。

这就是本章的核心链路。

如果少了 declaration，模型不知道可以调用什么。

如果少了 execution，模型只是在"许愿"，业务动作不会发生。

如果少了 response 回灌，模型看不到工具结果，只能胡编最终答案。

如果少了 confirmation/filtering，危险工具可能被模型误调用。

如果少了 provider bridge，OpenAI-compatible / DeepSeek 这类真实模型根本不理解 Go struct，只认 JSON schema、tool_calls 和 role=`tool` message。

---

## 2. 初学者桥：模型输出怎么变成本地函数执行

这一步一定要先讲清楚，否则后面的接口会显得很抽象。

### 2.1 模型看到的是 declaration，不是 Go function

本地 Go 代码里可以这样注册工具：

```go
weatherTool := tool.NewFunctionToolWithDeclaration(
    "get_weather",
    "Get deterministic current weather for one city.",
    tool.NewDeclaration(
        "get_weather",
        "Get deterministic current weather for one city.",
        map[string]any{
            "type": "object",
            "properties": map[string]any{
                "city": map[string]any{"type": "string"},
            },
            "required": []any{"city"},
        },
        nil,
    ),
    func(args map[string]any) (map[string]any, error) {
        city, _ := args["city"].(string)
        return map[string]any{"city": city, "temperature": 22}, nil
    },
)
```

这里有两层东西：

- `Declaration`：给模型看的说明书。
- `run func`：给本地 runtime 执行的 Go 代码。

模型不会看到 Go function body。它只看到类似：

```json
{
  "name": "get_weather",
  "description": "Get deterministic current weather for one city.",
  "parameters": {
    "type": "object",
    "properties": {
      "city": { "type": "string" }
    },
    "required": ["city"]
  }
}
```

所以 declaration 写得不好，模型就不知道怎么调用工具。

### 2.2 模型返回的是 FunctionCall

在复刻版里，模型响应会变成 `event.FunctionCall`：

```go
type FunctionCall struct {
    ID   string
    Name string
    Args map[string]any
}
```

这代表：

```text
call id: fc1
tool name: get_weather
args: {"city": "Tokyo"}
```

注意，这仍然没有执行任何业务动作。它只是一个 event part：

```go
event.Part{FunctionCall: &event.FunctionCall{...}}
```

### 2.3 Flow 根据 name 找到本地 Tool

`flow.Flow.executeToolCall` 做 lookup：

```go
t := f.lookupTool(fc.Name)
```

然后按 tool 类型执行：

```go
switch executable := t.(type) {
case tool.StreamingFunctionTool:
    cr = tool.ExecuteStream(fc.ID, fc.Name, args, executable)
case tool.ContextFunctionTool:
    ttctx := tool.NewToolContext(ctx, fc.ID, actions, nil)
    cr = tool.ContextExecute(ttctx, fc.ID, fc.Name, args, executable)
case tool.FunctionTool:
    cr = tool.Execute(fc.ID, fc.Name, args, executable)
default:
    ...
}
```

这就是"模型意图"变成"本地函数调用"的关键桥。

### 2.4 Tool result 变成 FunctionResponse

工具执行结果不是直接发给用户，而是先变成 tool event：

```go
event.Part{
    FunctionResponse: &event.FunctionResponse{
        ID:     r.CallID,
        Name:   r.Name,
        Result: r.Result,
    },
}
```

这条 event 的 role 是 `tool`。下一轮 model request 会把它作为上下文交给模型，让模型基于真实结果继续推理。

完整链路是：

```text
User asks
  -> Model event: FunctionCall(get_weather, {"city":"Tokyo"})
  -> Tool event: FunctionResponse(get_weather, {"temperature":22})
  -> Model event: "Tokyo is 22°C and sunny."
```

`TestFlowOneToolCallThenFinalResponse` 正是在验证这件事：三条 events，第一条是 model function call，第二条是 tool response，第三条是 final model text。

---

## 3. 本章代码地图

按教学大纲，本章建议按这个顺序读源码：

| 文件 | 重点 | 为什么读 |
| --- | --- | --- |
| `event/event.go` | `FunctionCall`、`FunctionResponse`、`EventActions.RequestedToolConfirmations` | tool-call 在 event 层的结构 |
| `tool/tool.go` | `Tool`、`FunctionTool`、`Declaration`、`FuncTool`、`Execute` | tool 最小接口、声明、执行 |
| `tool/tool.go` | `Toolset`、`StaticToolset`、`FilterToolset`、`InjectDeclarations` | tool collection 和 declaration 注入 |
| `tool/tool.go` | `WithConfirmation`、`ConfirmationControl`、`ContextExecute` | HITL 确认和 context-aware execution |
| `tool/context.go` | `ToolContext`、`RequestConfirmation` | 工具执行时如何访问 invocation、actions、确认请求 |
| `tool/streaming_tool.go` | `StreamingFunctionTool`、`StreamChunk`、`ExecuteStream` | streaming tool 在 non-live 模式下如何收集 |
| `flow/flow.go` | `handleFunctionCalls`、`executeToolCall`、`mergeResultsToEvent`、`injectToolDeclarations` | Flow 如何执行 function calls 并生成 tool event |
| `model/openai.go` | `openAIToolsFromDeclarations`、`llmResponseFromOpenAIChoice`、`openAIToolMessagesFromContent` | OpenAI-compatible provider bridge |
| `cmd/realllm/main.go` | `get_weather` real LLM smoke | declaration/schema 对真实模型的重要性 |
| `cmd/demo/main.go` | Chapter 03 demos | filtered tools、confirmation、streaming、long-running 的可运行演示 |

---

## 4. 三层接口：Tool / DeclarationProvider / FunctionTool

### 4.1 Tool 是最小身份接口

`tool.Tool` 很小：

```go
type Tool interface {
    Name() string
    Description() string
    IsLongRunning() bool
}
```

它回答三个问题：

- 这个工具叫什么？
- 它给人/模型的描述是什么？
- 它是不是 long-running？

只实现 `Tool` 不代表它能被本地执行。它只是 tool 的公共身份。

### 4.2 FunctionTool 才有本地执行能力

`FunctionTool` 扩展了 `Tool`：

```go
type FunctionTool interface {
    Tool
    Run(args map[string]any) (map[string]any, error)
}
```

这里的 `args` 是 `map[string]any`。这是复刻版的简化点：它没有原版那种更强的泛型 args/result 转换，也没有 `typeutil.ConvertToWithJSONSchema`。所以工具实现者要自己做类型断言：

```go
city, _ := args["city"].(string)
```

这件事很重要。真实 LLM 输出的是 JSON，进入运行时之后就是动态结构。你不能假设 `"city"` 一定存在，也不能假设它一定是 string，除非 schema 和 provider 行为都可靠。

### 4.3 DeclarationProvider 给模型看

`Declaration` 是 tool 给 LLM 的说明书：

```go
type Declaration struct {
    Name         string
    Description  string
    InputSchema  map[string]any
    OutputSchema map[string]any
}
```

`DeclarationProvider` 是可选接口：

```go
type DeclarationProvider interface {
    Declaration() Declaration
}
```

为什么它是可选的？

因为有些本地工具只在测试里直接通过 `FunctionCall` 调用，不需要暴露给真实模型。但真实 provider 需要 declaration，否则模型不知道工具存在。

`TestDeclarationNotCollectedWhenEmpty` 验证：没有有效 declaration name 的工具不会被注入到 LLM request。

### 4.4 Declaration 和 execution 必须分开

这是一条主线：

```text
Declaration: before model call
Execution: after model returns FunctionCall
```

时序完全不同。

在请求模型前：

```text
Flow.injectToolDeclarations
  -> tool.InjectDeclarations
  -> req.ToolDeclarations
```

模型返回后：

```text
Flow.handleFunctionCalls
  -> executeToolCall
  -> tool.Execute / ExecuteStream / ContextExecute
```

所以不能把 declaration 当成 execution，也不能以为注册了 Go function，模型就自然知道怎么调用。

### 4.5 FuncTool 是本地函数包装器

复刻版用 `FuncTool` 把普通函数包装成 tool：

```go
type FuncTool struct {
    name        string
    description string
    decl        Declaration
    run         func(args map[string]any) (map[string]any, error)
    longRunning bool
}
```

常见构造器：

```go
tool.NewFunctionTool(...)
tool.NewFunctionToolWithDeclaration(...)
tool.NewLongRunningFunctionTool(...)
```

`TestFunctionTool` 验证最小包装器能返回 name、description，并能执行 `Run`。

`TestFunctionToolStableDeclaration` 验证 declaration 会被 clone，调用方修改原始 schema 或返回的 schema，不会污染工具内部稳定 declaration。

---

## 5. Tool declaration 怎么进入 model request

### 5.1 CollectDeclarations 只收 DeclarationProvider

`tool.CollectDeclarations` 做三件事：

```go
for _, t := range tools {
    if dp, ok := t.(DeclarationProvider); ok {
        d := cloneDeclaration(dp.Declaration())
        if d.Name != "" {
            decls = append(decls, d)
        }
    }
}
sort.Slice(decls, func(i, j int) bool {
    return decls[i].Name < decls[j].Name
})
```

要点：

- 只有实现 `DeclarationProvider` 的 tool 会贡献 declaration。
- declaration name 为空会被跳过。
- 结果按 name 排序，保证 deterministic。

`TestInjectDeclarationsDeterministicAndOrdered` 用 `calculator`、`get_weather`、`search` 验证顺序稳定。

### 5.2 InjectDeclarations 写入 LLMRequest

`InjectDeclarations` 把 declarations 放进：

```go
req.ToolDeclarations
```

`model.LLMRequest` 的字段是：

```go
type LLMRequest struct {
    Model             string
    SystemInstruction string
    Contents          []LLMContent
    ToolDeclarations  []any
}
```

这里用 `[]any` 是为了给 provider adapter 留空间：复刻版内部用 `tool.Declaration`，OpenAI-compatible bridge 也能接收 map 形式的 declaration。

### 5.3 Toolset 是动态工具集合

`Toolset`：

```go
type Toolset interface {
    Name() string
    Tools() ([]Tool, error)
}
```

它的意义不是执行，而是决定这一次 invocation 有哪些工具可见。

`StaticToolset` 是固定集合。

`FilterToolset` 是包装集合：

```go
filtered := tool.NewFilterToolset(
    "safe_tools",
    fullToolset,
    tool.AllowedToolsPredicate("get_weather"),
)
```

这个例子很适合业务开发者理解：你可以在系统里注册 `get_weather` 和 `delete_data`，但在某个 agent / session / risk policy 下只暴露 `get_weather` 给模型。

`TestFilterToolsetByName` 验证 `search` 会被过滤掉。

### 5.4 过滤工具和确认工具不是一回事

这两个概念容易混：

| 机制 | 发生时机 | 解决什么 |
| --- | --- | --- |
| `FilterToolset` | model call 前 | 模型能不能看见这个工具 |
| `WithConfirmation` | tool execution 时 | 这次调用能不能真正执行 |

如果工具被过滤，模型通常不会调用它。

如果工具需要确认，模型可以提出调用，但运行时先返回 confirmation-required，不直接执行危险动作。

安全策略通常两者都要用：

- 默认不要把危险工具暴露给不需要的 agent。
- 必须暴露时，再对高风险参数做 confirmation。

---

## 6. FunctionCall 的执行生命周期

### 6.1 Flow 找到所有 function calls

`handleFunctionCalls` 先从 model event 里取 function calls：

```go
fnCalls := modelEvent.FunctionCalls()
if len(fnCalls) == 0 {
    return nil, nil
}
```

`event.Event.FunctionCalls()` 会遍历 `Content.Parts`，找出所有 `Part.FunctionCall != nil`。

### 6.2 多个 function calls 并发执行

复刻版并行执行：

```go
results := make([]tool.CallResult, len(fnCalls))
var wg sync.WaitGroup

for i, fnCall := range fnCalls {
    wg.Add(1)
    go func(idx int, fc *event.FunctionCall) {
        defer wg.Done()
        results[idx] = f.executeToolCall(ctx, fc)
    }(i, fnCall)
}
wg.Wait()
```

注意这里既并行，又保持结果 slice 的 index 稳定。也就是说执行顺序可能不确定，但合并 event 时的结果顺序由原 function call 顺序决定。

`TestFlowMultipleToolCallsDeterministic` 验证一个 model event 里有 `get_weather` 和 `search` 两个 calls，tool result event 里会有两个 function responses。

### 6.3 executeToolCall 的核心分派

核心逻辑是：

```text
FunctionCall
  -> args map
  -> ToolContext
  -> before tool callbacks / plugins
  -> lookupTool(name)
  -> ExecuteStream / ContextExecute / Execute
  -> after tool callbacks / plugins
  -> CallResult
```

查不到工具时，不会 panic，而是生成 error result：

```go
tool "missing" not found
```

`TestExecuteToolNotFound` 和 `TestFlowToolErrorBecomesEvent` 都验证错误会进入 result/event，而不是被吞掉。

### 6.4 Tool result 合并成 tool event

`mergeResultsToEvent` 创建一个 role=`tool` event：

```go
ev := event.NewEvent(
    fmt.Sprintf("%s-step-%d-toolresults", ctx.InvocationID(), step),
    ctx.Agent().Name(),
    event.RoleTool,
)
```

每个 `CallResult` 变成一个 `FunctionResponse` part：

```go
event.Part{
    FunctionResponse: &event.FunctionResponse{
        ID:     r.CallID,
        Name:   r.Name,
        Result: r.Result,
    },
}
```

如果工具返回 error，`FunctionResponse.Error` 会设置，event 的 `ErrorMessage` 也会汇总。

### 6.5 Tool result 也可以产生 StateDelta

`mergeResultsToEvent` 还有一个很关键的约定：

```go
if sd, ok := r.Result["state_delta"]; ok {
    if sdMap, ok := sd.(map[string]any); ok {
        ...
        stateDelta[k] = v
    }
}
```

也就是说工具可以通过 result 里的 `state_delta` 影响 session state。

例如：

```go
return map[string]any{
    "status": "ok",
    "state_delta": map[string]any{
        "weather.last": "sunny",
    },
}, nil
```

`TestFlowStateDeltaMerge` 验证工具返回 `state_delta` 后，`ctx.Session().State()` 能读到 `weather.last = "sunny"`。

这和 Chapter 02 的 `StateDelta` 连接起来了：Tool 不只是返回给模型看的 data，也可能修改运行时 state。

---

## 7. ToolContext：工具执行时能看到什么

普通 `FunctionTool.Run(args)` 只能看到 args。

但很多真实工具需要更多上下文：

- 当前 invocation ID。
- 当前 session。
- 当前 function call ID。
- event actions。
- confirmation request。

所以有 `ToolContext`：

```go
type ToolContext interface {
    InvocationContext() context.InvocationContext
    FunctionCallID() string
    ToolConfirmation() *event.ToolConfirmation
    RequestConfirmation(hint string, payload any) error
    Actions() *event.EventActions
}
```

### 7.1 RequestConfirmation 会写 EventActions

`RequestConfirmation` 做的是：

```go
t.actions.RequestedToolConfirmations[t.functionCallID] = event.ToolConfirmation{
    Hint:      hint,
    Confirmed: false,
    Payload:   payload,
}
t.actions.SkipSummarization = true
```

它不是直接弹 UI，而是在 event actions 上记录 pending confirmation。外层系统可以看到这条 action，然后让用户 approve/reject。

`TestToolContextRequestConfirmation` 验证：

- `SkipSummarization` 会变成 true。
- `RequestedToolConfirmations["fc-003"]` 会存在。
- 初始 `Confirmed` 是 false。

### 7.2 ContextFunctionTool 优先走 RunWithContext

如果工具实现 `ContextFunctionTool`，Flow 会用：

```go
tool.ContextExecute(ttctx, fc.ID, fc.Name, args, executable)
```

否则用普通 `tool.Execute`。

这给需要 session/confirmation/actions 的工具留了接口，同时不增加普通工具的负担。

---

## 8. Confirmation：工具调用前的人类确认

### 8.1 为什么需要 confirmation

模型可能会调用危险动作：

```text
delete_user_data
deploy_to_prod
drop_table
send_wire_transfer
```

这些不能只靠 prompt 约束。运行时必须在执行前拦截。

复刻版提供的是 `WithConfirmation`：

```go
confirmedTool := tool.WithConfirmation(inner, true, nil)
```

### 8.2 WithConfirmation 的三段式状态机

`confirmationTool.Run` 的逻辑可以读成：

```text
if previously approved:
    execute inner tool
else if previously rejected:
    return confirmation_rejected
else if confirmation is needed:
    return confirmation_required
else:
    execute inner tool
```

对应代码：

```go
if c.confirmedCall {
    c.confirmedCall = false
    return c.inner.Run(args)
}

if c.confirmed {
    return c.rejectedResult(), fmt.Errorf("tool %q %w", c.Name(), ErrConfirmationRejected)
}

needsConfirmation := c.requireConfirmation
if c.provider != nil {
    needsConfirmation = c.provider(c.Name(), args)
}

if !needsConfirmation {
    return c.inner.Run(args)
}

return c.confirmationRequiredResult(), fmt.Errorf("tool %q %w", c.Name(), ErrConfirmationRequired)
```

`SetConfirmed(true)` 让下一次 call 执行。

`SetConfirmed(false)` 让下一次 call 返回 rejected。

### 8.3 静态确认和动态确认

静态确认：

```go
tool.WithConfirmation(inner, true, nil)
```

每次都需要确认。

动态确认：

```go
provider := func(toolName string, toolInput map[string]any) bool {
    risk, _ := toolInput["risk"].(string)
    return risk == "high"
}
tool.WithConfirmation(inner, false, provider)
```

只有高风险参数需要确认。

`TestConfirmationWithDynamicProvider` 验证 `risk=low` 直接执行，`risk=high` 返回 confirmation required。

### 8.4 Confirmation 的教学边界

当前复刻版的 confirmation 是简化版。它展示了确认机制的核心，但不是完整生产工作流。

重要边界：

- 没有完整的 `RequestConfirmationRequestProcessor`。
- 没有跨请求自动匹配 approve/reject event 的完整 loop。
- `WithConfirmation` 使用 wrapper 内部状态，适合教学和 demo，不适合多用户并发生产复用。
- `ToolContext.RequestConfirmation` 已经能把 pending confirmation 写进 actions，但外层 UI/审批系统不在本章实现。

所以讲课时要说清楚：本章展示的是 HITL 的两种入口形态。

| 形态 | 当前代码 | 教学意义 |
| --- | --- | --- |
| Wrapper confirmation | `WithConfirmation` | 最直观地展示 approve/reject/execution gate |
| Event action confirmation | `ToolContext.RequestConfirmation` | 更接近 ADK runtime 的事件驱动审批模型 |

---

## 9. Streaming Tool：流式工具和 non-live fallback

### 9.1 StreamingFunctionTool

流式工具接口：

```go
type StreamingFunctionTool interface {
    Tool
    Declaration() Declaration
    RunStream(args map[string]any) ([]StreamChunk, error)
}
```

`StreamChunk`：

```go
type StreamChunk struct {
    Text  string
    Error string
    Final bool
}
```

这表达的是：工具可能分块产生文本。

### 9.2 Non-live 模式会收集成单条 result

复刻版没有实现真正 live bidi streaming。当前 `ExecuteStream` 会调用：

```go
chunks, err := t.RunStream(args)
result, err := CollectStreamChunks(chunks)
```

`CollectStreamChunks` 把所有 chunks 的 `Text` 拼起来：

```go
text += c.Text
```

返回：

```go
map[string]any{"result": text}
```

如果某个 chunk 有 Error：

```go
map[string]any{"result": text, "error": errMsg}
```

`TestStreamingCollection` 验证 `"Hello" + " " + "World"` 会变成 `"Hello World"`。

`TestStreamingError` 验证有 error chunk 时，保留 partial text，同时返回 error。

### 9.3 这里会丢失低延迟语义

这就是教学大纲里的容易误解点：

> Streaming Non-Live 丢失增量语义。

也就是说，工具内部虽然是 chunks，但 Flow 最终给 model 的仍然是一条完整 function response。用户不会实时看到每个 chunk。

如果要做真正 live streaming，还需要：

- 边产生 chunk 边 yield partial event。
- 保持 function call / response 的关联。
- 处理 partial event 不持久化。
- 处理模型和工具双向流式协议。

本章只讲 non-live fallback。

---

## 10. Long-Running Tool：声明里的行为提示

`NewLongRunningFunctionTool` 会做两件事：

1. 设置 `longRunning = true`。
2. 在 declaration description 上追加提示：

```text
NOTE: This is a long-running operation. Do not call this tool again if it has already returned some intermediate or pending status.
```

这不是强制机制。框架不会阻止模型重复调用。

它只是通过 declaration description 告诉模型：

```text
这个工具可能返回 pending/job_id，不要重复调用。
```

`TestLongRunningToolDeclaration` 验证：

- `IsLongRunning()` 为 true。
- declaration description 包含 `long-running operation`。
- description 包含 `Do not call this tool again`。

真实业务里，long-running tool 通常返回：

```go
map[string]any{
    "job_id": "job-12345",
    "status": "pending",
}
```

`TestLongRunningToolResultMetadata` 验证这种 pending metadata。

---

## 11. OpenAI-compatible provider bridge

这是本章最容易被忽略、但对真实模型最关键的一节。

### 11.1 真实 provider 不认识 Go Tool

OpenAI-compatible API 只认识：

- `messages`
- `tools`
- assistant `tool_calls`
- role=`tool` messages

所以 `model/openai.go` 必须做三段转换。

### 11.2 Declaration -> tools

`GenerateContent` 构造 payload：

```go
payload := openAIChatRequest{
    Model:    m.name,
    Messages: openAIMessagesFromRequest(req),
    Tools:    openAIToolsFromDeclarations(req.ToolDeclarations),
}
```

`openAIToolsFromDeclarations` 把内部 declaration 转成 OpenAI-compatible tools。

对于 `get_weather`，最终类似：

```json
{
  "type": "function",
  "function": {
    "name": "get_weather",
    "description": "Get weather.",
    "parameters": {
      "type": "object",
      "properties": {
        "city": { "type": "string" }
      }
    }
  }
}
```

`TestOpenAICompatibleModelToolCallResponse` 验证 captured request 里有一个 tool，name 是 `get_weather`。

### 11.3 tool_calls -> FunctionCall

真实 provider 返回：

```json
{
  "tool_calls": [{
    "id": "call_1",
    "type": "function",
    "function": {
      "name": "get_weather",
      "arguments": "{\"city\":\"Tokyo\"}"
    }
  }]
}
```

adapter 需要把 `arguments` JSON string decode 成：

```go
event.FunctionCall{
    ID: "call_1",
    Name: "get_weather",
    Args: map[string]any{"city": "Tokyo"},
}
```

同一个测试验证 `resp.Content.Parts[0].FunctionCall.Args["city"] == "Tokyo"`。

### 11.4 FunctionResponse -> role=tool message

当 tool event 进入下一轮 request，OpenAI-compatible provider 需要把它转成：

```json
{
  "role": "tool",
  "tool_call_id": "call_1",
  "name": "get_weather",
  "content": "{\"result\":{\"temperature\":22}}"
}
```

对应函数是 `openAIToolMessagesFromContent`。

这是为什么 Chapter 01 的 tool loop 不能只把 result 存在本地：真实模型下一轮必须收到 provider 协议认可的 tool message。

`TestContentsFromEventsPreservesToolLoop` 验证 event history 会保留 function call / function response，从而给 provider bridge 转换。

### 11.5 malformed JSON arguments 的边界

教学大纲问：

> 如果模型返回 malformed JSON arguments，应该在哪里观察错误？

在 OpenAI-compatible adapter 层，`arguments` 是 string，需要 decode 成 map。如果 JSON malformed，`GenerateContent` 应该返回 decode error。这个错误会发生在 model provider bridge，而不是 tool execution 层。

也就是说：

```text
provider response malformed
  -> model.GenerateContent error
  -> Flow callModel / callback error path
```

工具根本不会被执行，因为还没有形成合法 `FunctionCall.Args`。

---

## 12. FilterToolset / WithConfirmation / ToolContext 的安全边界

这三个东西经常被混在一起。

### 12.1 FilterToolset：能不能看见

`FilterToolset` 控制 declaration 注入。

如果一个工具没有 declaration 出现在 request 里，模型通常不会主动调用它。

适合：

- 根据 agent role 暴露不同工具。
- 根据用户权限隐藏工具。
- 根据环境隐藏生产危险工具。

### 12.2 WithConfirmation：看见了也不能马上执行

`WithConfirmation` 控制 execution gate。

适合：

- deploy。
- delete。
- transfer money。
- 发送外部消息。

### 12.3 ToolContext.RequestConfirmation：事件驱动审批

`RequestConfirmation` 控制 event action。

适合更完整的 runtime：

- tool 执行中发现参数高风险。
- 在 event 上记录 pending confirmation。
- UI / 人类回合 / processor 在后续请求中处理确认结果。

### 12.4 三者组合

一个更合理的生产策略是：

```text
普通用户:
  FilterToolset 只暴露 read-only tools

管理员:
  暴露 deploy_app
  deploy_app 使用 confirmation

高风险参数:
  ToolContext.RequestConfirmation 写 pending action
  等用户明确 approve
```

---

## 13. 容易误解点

### 13.1 "Tool declaration 就是 tool execution"

不对。Declaration 只是模型调用前的说明书。Execution 是模型返回 FunctionCall 之后，Flow 根据 name 找到本地 tool 并执行。

### 13.2 "模型能直接调用 Go 函数"

不对。模型只能输出 tool name + JSON args。Go 函数由本地 runtime 调用。

### 13.3 "有 FunctionTool 就一定会注入给模型"

不对。只有带有效 declaration 的 `DeclarationProvider` 才会被 `CollectDeclarations` 注入。空 declaration name 会跳过。

### 13.4 "FilterToolset 是审批机制"

不对。FilterToolset 控制可见性，不处理某次调用是否 approve。

### 13.5 "WithConfirmation 能代表完整生产审批"

不完整。当前 wrapper 是教学简化版。更完整的 ADK-style 审批会围绕 event actions、pending confirmations、后续用户响应和 request processor。

### 13.6 "StreamingFunctionTool 一定会实时流给用户"

复刻版不是。当前 non-live 模式会收集 chunks，生成单条 `FunctionResponse`。

### 13.7 "Long-running tool 会被框架强制去重"

不会。当前实现只是修改 declaration description，依赖模型遵循。

### 13.8 "Tool error 会让 Flow 直接失败"

通常不会。`tool.Execute` 会把 error 放进 `CallResult.Error` 和 result `error` 字段，Flow 生成带 error 的 tool event。这样模型还有机会基于错误继续回答。

### 13.9 "Tool result 给本地 runtime 看就够了"

不够。下一轮 model request 必须把 tool result 按 provider 协议回灌。OpenAI-compatible 模型需要 role=`tool` message。

### 13.10 "args map 可以随便信"

不能。真实模型输出 JSON，provider decode 后是动态 map。工具实现要校验必填字段和类型。Schema 能降低错误，但不能代替运行时校验。

---

## 14. 测试如何证明本章语义

### 14.1 FunctionTool 和 Execute

`TestFunctionTool` 验证 `NewFunctionTool` 包装函数后可以执行。

`TestExecute` 验证 `Execute` 生成 `CallResult`，保留 call id、tool name、result。

`TestExecuteToolError` 验证工具返回 error 时，`CallResult.Error` 和 `Result["error"]` 都会有值。

### 14.2 Declaration

`TestFunctionToolStableDeclaration` 验证 declaration clone，不被外部 mutation 污染。

`TestDeclarationNotCollectedWhenEmpty` 验证空 declaration 不注入。

`TestInjectDeclarationsDeterministicAndOrdered` 验证 declaration 按 name 排序。

### 14.3 Toolset / filtering

`TestAllowedToolsPredicate` 验证 allow-list predicate。

`TestFilterToolsetByName` 验证只保留允许的工具。

`TestFilterToolsetAllBlocked` 验证可以过滤到空集合。

### 14.4 Flow tool loop

`TestFlowOneToolCallThenFinalResponse` 验证：

```text
model FunctionCall -> tool FunctionResponse -> final model text
```

`TestFlowMultipleToolCallsDeterministic` 验证多个 function calls 的并行执行和合并。

`TestFlowStateDeltaMerge` 验证 tool result 里的 `state_delta` 会进入 session state。

`TestFlowToolErrorBecomesEvent` 验证 tool error 会成为 event/function response 的 error。

### 14.5 Tool callbacks

`TestFlowBeforeToolCallbackOverride` 验证 before tool callback 可以短路真实工具，返回 cached result。

`TestFlowAfterToolCallbackTransform` 验证 after tool callback 可以改写 tool result。

这部分会在 Chapter 04 更完整展开，但在 Chapter 03 要知道 tool execution 不是裸奔，它也受 callback/plugin 管道影响。

### 14.6 Confirmation

`TestConfirmationRequired` 验证第一次调用危险工具返回 confirmation required。

`TestConfirmationApproved` 验证 `SetConfirmed(true)` 后下一次执行真实工具。

`TestConfirmationRejected` 验证 `SetConfirmed(false)` 后返回 rejected。

`TestConfirmationWithDynamicProvider` 验证 provider 可以基于 args 决定是否需要确认。

### 14.7 Streaming

`TestStreamingCollection` 验证 chunks 被收集成 `"Hello World"`。

`TestStreamingError` 验证 error chunk 会保留 partial result 并返回 error。

`TestCollectStreamChunks` 验证 helper 直接拼接 chunks。

### 14.8 OpenAI-compatible provider bridge

`TestOpenAICompatibleModelToolCallResponse` 验证：

- request 带 `tools`。
- provider 返回 `tool_calls`。
- adapter 解出 `FunctionCall{Name:"get_weather", Args:{"city":"Tokyo"}}`。

`TestContentsFromEventsPreservesToolLoop` 验证 function call / response 在 event history 到 model contents 转换中保留。

---

## 15. 课堂讲解脚本

### 第 0-2 分钟：从业务动作切入

抛问题：

```text
模型说 "I will deploy the app" 和系统真的 deploy，有什么区别？
```

答案：

```text
模型只能产生意图。
Tool System 才能执行代码动作。
```

画出：

```text
Declaration -> FunctionCall -> local Run -> FunctionResponse -> next model turn
```

### 第 2-5 分钟：讲 declaration 和 execution 分离

用 `get_weather`：

- Declaration 给模型看。
- Run 给本地 runtime 执行。
- `NewFunctionToolWithDeclaration` 把两者挂在同一个工具对象上。

强调：

```text
真实模型只看 JSON Schema，不看 Go struct。
```

### 第 5-8 分钟：讲 Flow 执行 function call

按源码走：

```text
modelEvent.FunctionCalls()
  -> executeToolCall
  -> lookupTool
  -> Execute / ExecuteStream / ContextExecute
  -> mergeResultsToEvent
```

用 `TestFlowOneToolCallThenFinalResponse` 收口。

### 第 8-10 分钟：讲 filtering 和 confirmation

对比：

```text
FilterToolset: 模型看不看得到
WithConfirmation: 看到了能不能执行
```

用 `delete_data` / `deploy_app` 举例。

### 第 10-12 分钟：讲 streaming 和 long-running

说明当前复刻版边界：

- streaming chunks 在 non-live 模式下被收集成一个 result。
- long-running 只是 declaration hint，不是 runtime 去重锁。

### 第 12-14 分钟：讲真实 provider bridge

画 OpenAI-compatible 三段桥：

```text
Declaration -> tools
tool_calls -> FunctionCall
FunctionResponse -> role=tool message
```

最后用 `cmd/realllm/main.go` 说明真实 LLM smoke 为什么必须写 JSON schema。

---

## 16. 实战阅读任务

### 任务 1：画完整 tool loop

基于 `TestFlowOneToolCallThenFinalResponse`，画出三条 events：

```text
model: FunctionCall
tool: FunctionResponse
model: final text
```

写出每条 event 的 role、content part 类型、是否 final。

### 任务 2：写一个带 declaration 的工具

创建：

```text
get_stock_price(symbol string) -> price
```

要求：

- declaration name 和 tool name 一致。
- input schema 有 `symbol` string。
- `required` 包含 `symbol`。
- Run 里校验 symbol 非空。

### 任务 3：实现安全过滤

注册三个工具：

```text
get_weather
search_docs
delete_data
```

用 `FilterToolset` 只暴露前两个。解释为什么这不是审批机制。

### 任务 4：实现动态确认

工具：

```text
deploy_app(env string)
```

规则：

- `env == "prod"` 需要确认。
- `env == "staging"` 不需要确认。

用 `ConfirmationProvider` 实现。

### 任务 5：解释 streaming non-live 的损失

给出三个 chunks：

```text
"Intro\n"
"Analysis\n"
"Conclusion\n"
```

说明 `CollectStreamChunks` 后模型看到什么，用户失去了什么实时能力。

### 任务 6：追踪 provider bridge

从 `tool.Declaration` 开始，追踪它如何进入 OpenAI-compatible request 的 `tools` 字段；再追踪 response `tool_calls` 如何变成 `event.FunctionCall`。

### 任务 7：解释 tool error 的传播

工具返回：

```go
return nil, errors.New("database connection refused")
```

写出：

- `CallResult.Error`
- `Result["error"]`
- `FunctionResponse.Error`
- `Event.ErrorMessage`

### 任务 8：比较 `state_delta` 和普通 result

工具返回：

```go
map[string]any{
    "message": "done",
    "state_delta": map[string]any{"task.status": "done"},
}
```

说明哪部分给模型看，哪部分会进入 session state。

---

## 17. 自测题

1. `Tool` 和 `FunctionTool` 的区别是什么？
2. `Declaration` 是给谁看的？`Run` 是谁调用的？
3. 为什么 declaration injection 发生在 model call 之前？
4. 模型返回 function call 后，Flow 如何找到本地工具？
5. 多个 function calls 是串行还是并行执行？结果顺序如何保持？
6. tool 返回 error 时，Flow 会直接失败吗？
7. `FilterToolset` 和 `WithConfirmation` 的安全职责分别是什么？
8. `ToolContext.RequestConfirmation` 会写入哪里？
9. streaming tool 在当前 non-live 模式下会如何返回结果？
10. long-running tool 的防重复调用是 runtime 强制还是 declaration hint？
11. OpenAI-compatible provider bridge 需要哪三段转换？
12. 为什么 tool result 必须以 role=`tool` message 回灌给模型？
13. `state_delta` 出现在 tool result 里时，Flow 会做什么？
14. 如果模型返回 malformed JSON arguments，错误应该发生在 provider adapter 层还是 tool execution 层？
15. 空 declaration name 的工具会被注入给模型吗？

参考答案：

1. `Tool` 只有 name/description/long-running 身份信息；`FunctionTool` 还有 `Run(args)`，能本地执行。
2. `Declaration` 给 LLM/provider 看；`Run` 由本地 Flow/runtime 调用。
3. 因为模型要先知道可用工具和参数 schema，才可能返回正确 FunctionCall。
4. `Flow.executeToolCall` 通过 `lookupTool(fc.Name)` 找工具，再按类型执行。
5. 并行执行；结果写入固定 index 的 slice，按原 function call 顺序合并。
6. 通常不会。error 会进入 `CallResult.Error`、result `error`、`FunctionResponse.Error` 和 tool event error message。
7. `FilterToolset` 控制模型能不能看见工具；`WithConfirmation` 控制一次调用能不能真正执行。
8. 写入 `EventActions.RequestedToolConfirmations`，并设置 `SkipSummarization`。
9. chunks 被 `CollectStreamChunks` 拼成单个 `result`，有 error chunk 时同时返回 `error`。
10. declaration hint。当前 runtime 不强制去重。
11. `Declaration -> tools`，`tool_calls -> FunctionCall`，`FunctionResponse -> role=tool message`。
12. 因为真实模型下一轮需要通过 provider 协议看到工具结果，否则无法基于真实结果回答。
13. `mergeResultsToEvent` 会合并到 event actions 的 `StateDelta`，并更新 session state。
14. provider adapter 层。JSON arguments 还没合法 decode 成 `FunctionCall.Args`，工具不会执行。
15. 不会。`CollectDeclarations` 会跳过 name 为空的 declaration。

---

## 18. 本章一句话总结

Chapter 03 的核心不是"怎么写一个函数"，而是把模型意图接到真实业务动作：declaration 让模型知道可调用能力，FunctionCall 表达模型的结构化调用意图，Flow 根据 name 执行本地 Tool，FunctionResponse 把真实结果回灌给模型；Toolset、Confirmation、Streaming、Long-running 和 provider bridge 则分别解决可见性、审批、流式、异步任务和真实 LLM 协议适配问题。
