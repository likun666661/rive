# Chapter 01 - Runtime Flow: Runner -> Agent -> Flow -> Model/Tool -> Event -> Session 深度讲解

> 复刻工程：`/Users/likun/Desktop/workspace-for-google-adk-go/rive-adk-go/`  
> 教学细纲：`examples/google-adk-go-code-reading/manual/teaching-manual-outline.md`  
> 核心文件：`runner/runner.go`、`agent/agent.go`、`llmagent/llmagent.go`、`flow/flow.go`、`model/model.go`、`model/contents.go`、`event/event.go`、`session/session.go`、`cmd/demo/main.go`  
> 建议讲解时长：16 分钟；建议自学阅读时长：60-90 分钟

这一章讲 Google ADK Go 复刻版的运行主链路。它不是先讲某个 API 怎么调用，而是先回答一个更底层的问题：

**用户发来一句话以后，这句话如何穿过 Runner、Agent、Flow、Model、Tool、Event、Session，最终变成可返回、可持久化、可继续下一轮对话的事件流？**

如果只看业务表层，天气查询 demo 很简单：

```text
User: What's the weather in Tokyo?
Model: 我需要调用 get_weather
Tool: Tokyo 22°C sunny
Model: The weather in Tokyo is 22°C and sunny.
```

但 runtime 真正要处理的是一条更长的链路：

```text
Runner.Run
  -> get/create Session
  -> append user event
  -> choose active Agent
  -> build InvocationContext
  -> Agent.Execute
      -> before callbacks
      -> Flow.Run
          -> build model request from session + prior step events
          -> call model
          -> yield model event
          -> execute function calls
          -> yield tool event
          -> loop until final response
      -> after callbacks
  -> persist non-partial events
  -> return session + produced events
```

Chapter 01 的目标就是把这条链路讲清楚。只要这条链路建立起来，后面几章的 State、Tool、Callback、Workflow、Entrypoint、ReAct 都是在这条主干上加能力。

---

## 1. 为什么第一章必须从 Runtime Flow 开始

很多 agent 框架的入门文档会从"创建一个 agent"开始。但如果想读懂 ADK Go 这种 runtime，直接从 agent API 开始容易漏掉关键边界。

一个真实 agent 调用不只是"函数入参 -> 函数出参"，而是涉及：

- 用户消息要先进入 Session，成为历史的一部分。
- Runner 要判断这次应该由 root agent 还是上次 transfer 后的 specialist agent 接着处理。
- Agent 要执行 before/after 生命周期回调。
- Flow 要运行 model/tool 多步循环。
- Model 可能输出普通文本，也可能输出 function call。
- Tool 执行结果要变成 function response event。
- 同一次 invocation 内的 model event 和 tool event 要回灌给下一轮 model。
- Partial streaming event 可以 yield 给调用方，但不能持久化。
- 最终事件要写回 Session，成为下一次请求的 history。

所以第一章不追求"马上写复杂业务"，而是先建立这套运行时坐标系：

```text
Runner 是外壳
Agent 是生命周期对象
Flow 是模型/工具循环
Model 和 Tool 是执行能力
Event 是运行时事实
Session 是事实存储
```

这六个词后面会反复出现。

---

## 2. 先看最小天气查询 Demo

复刻工程的 `cmd/demo/main.go` 里，`runChapter01()` 是第一章的最小入口。它搭了一个天气工具、一个 FakeModel、一个 Flow、一个 LLM agent、一个 Runner，然后调用 `Runner.Run`。

核心结构是：

```go
weatherTool := tool.NewFunctionTool("get_weather", "Get current weather for a city",
    func(args map[string]any) (map[string]any, error) {
        city, _ := args["city"].(string)
        return map[string]any{
            "city":        city,
            "temperature": 22,
            "condition":   "sunny",
            "humidity":    "45%",
        }, nil
    },
)
```

这个工具只是一个 Go 函数。它的名字是 `get_weather`，输入是 `map[string]any`，输出也是 `map[string]any`。

接着构造 FakeModel：

```go
fakeModel := model.NewFakeModel("demo-model",
    model.FunctionCallResponse("Let me check the weather.",
        event.FunctionCall{
            ID:   "fc-1",
            Name: "get_weather",
            Args: map[string]any{"city": "Tokyo"},
        },
    ),
    model.TextResponse("The weather in Tokyo is 22°C and sunny with 45% humidity."),
)
```

FakeModel 的响应队列有两个响应：

1. 第一轮返回 function call：调用 `get_weather(city=Tokyo)`。
2. 第二轮返回最终文本答案。

再把 Model 和 Tool 放进 Flow：

```go
f := &flow.Flow{
    Model: fakeModel,
    Tools: map[string]tool.FunctionTool{
        "get_weather": weatherTool,
    },
}
```

然后用 `llmagent.New` 把 Flow 包成一个 Agent：

```go
ag, err := llmagent.New("weather_bot", "A bot that answers weather questions.", f)
```

最后创建 Runner：

```go
r, err := runner.New(runner.Config{
    AppName:        "weather_app",
    Agent:          execAgent,
    SessionService: sessionSvc,
})
```

调用：

```go
sess, events, err := r.Run(
    context.Background(),
    "user-1",
    "sess-1",
    "What's the weather in Tokyo?",
)
```

这段 demo 的教学价值不在天气查询本身，而在它完整穿过了：

```text
Runner -> Agent -> Flow -> FakeModel -> FunctionTool -> Event -> Session
```

这就是本章最小闭环。

---

## 3. 运行时主链路总图

可以把一次 `Runner.Run` 画成三层。

第一层是外层会话与持久化：

```text
Runner.Run
  -> SessionService.Get/Create
  -> append user event
  -> findAgentToRun
  -> build InvocationContext
  -> agent.Execute
  -> append non-partial produced events
```

第二层是 Agent 生命周期：

```text
agent.Execute
  -> beforeAgentCallbacks
  -> a.run(ctx)
  -> afterAgentCallbacks
```

第三层是 LLM Flow 循环：

```text
Flow.Run
  -> for step := 1; ; step++ {
        runOneStep
        if modelEvent.IsFinalResponse() return
     }
```

把三层合起来：

```text
User message
  |
  v
Runner.Run
  |
  +-- Session: get/create + append user event
  |
  +-- Agent routing: find active agent from session history
  |
  +-- InvocationContext: app/user/session/agent/services
  |
  v
Agent.Execute
  |
  +-- before callbacks
  |
  v
LLMAgent.run
  |
  v
Flow.Run
  |
  +-- build LLMRequest from session events + prior step events
  +-- preprocess
  +-- inject tool declarations
  +-- inject transfer tool
  +-- call model
  +-- postprocess
  +-- finalize model event
  +-- execute function calls in parallel
  +-- merge tool results into tool event
  +-- loop or stop
  |
  v
Agent.Execute after callbacks
  |
  v
Runner persists non-partial events
  |
  v
Session history for next invocation
```

这个图要反复讲。后面任何机制都能放回图中定位：

- State delta 在 EventActions 上。
- Tool confirmation 在 tool event 上。
- TransferToAgent 在 EventActions 上。
- Partial event 在 Runner 持久化前被过滤。
- Plugin/callback 在 Agent、Model、Tool 阶段插入。

---

## 4. Runner：一次用户请求的外壳

`runner/runner.go` 的 `Runner` 结构体很小：

```go
type Runner struct {
    appName         string
    agent           ExecutableAgent
    sessionService  SessionService
    memoryService   memory.Service
    artifactService artifact.Service
}
```

它把四类东西放在一起：

- root agent。
- session service。
- memory service。
- artifact service。

Chapter 01 主要用 session service，memory/artifact 是 Chapter 02 的主题。但它们已经在 Runner 配置里出现，说明 ADK runtime 一开始就把"会话、长期记忆、文件产物"当成运行上下文的一部分。

### 4.1 `Runner.Run` 的七步

`Runner.Run` 的注释已经把流程写得很清楚：

```go
// 1. Get or create the session
// 2. Find the active agent from session history
// 3. Create a user event and append it to the session
// 4. Build an InvocationContext
// 5. Execute the active agent
// 6. Persist non-partial events to the session
// 7. Return the session and all events
```

实际代码里顺序有一个细节：它先 append user event，再 `findAgentToRun(sess)`。这是安全的，因为 `findAgentToRun` 会跳过 `Author == "user"` 的事件。

这也解释了为什么测试里要覆盖 `TestRunnerFindAgentToRunSkipsUser`：如果不跳过 user event，Runner 可能会误以为最后活跃 agent 是 user，导致路由失败。

### 4.2 获取或创建 Session

入口：

```go
sess, err := r.sessionService.Get(ctx, r.appName, userID, sessionID)
if err != nil {
    sess, err = r.sessionService.Create(ctx, r.appName, userID, sessionID)
}
```

这意味着调用方可以直接传 `(userID, sessionID)`，Runner 会帮你补齐 session 生命周期。

测试 `TestRunnerAutoCreateSession` 验证的是：session 不存在时，Runner 会自动创建。

### 4.3 用户消息先变成 Event

Runner 不会把字符串直接传给模型。它先构造 user event：

```go
userEvent := event.NewEvent(
    fmt.Sprintf("%s-user-%d", sess.ID(), nextEventOrdinal),
    "user",
    event.RoleUser,
)
userEvent.Branch = r.agent.Name()
userEvent.Content = &event.Content{
    Role: event.RoleUser,
    Parts: []event.Part{
        {Text: message},
    },
}
```

几个点要注意：

- `Author` 是 `"user"`。
- `Role` 是 `event.RoleUser`。
- `Content.Parts` 里放文本。
- `Branch` 初始设为 root agent 名。

然后立刻：

```go
sess.AppendEvent(userEvent)
```

这一步非常重要。Flow 后面构造 LLM request 时，会从 session events 里读历史。如果用户消息不先进入 session，模型第一轮就看不到用户输入。

### 4.4 Agent routing：为什么不是永远跑 root agent

Runner 会调用：

```go
agentToRun := r.findAgentToRun(sess)
```

这个函数反向扫描 session history：

```go
for i := len(events) - 1; i >= 0; i-- {
    ev := events[i]
    if ev == nil {
        continue
    }
    if ev.Author == "user" {
        continue
    }

    candidate := r.agent.FindAgent(ev.Author)
    if candidate == nil {
        continue
    }

    if r.isTransferableAcrossAgentTree(candidate) {
        return candidate
    }
}
return r.agent
```

这解决的是多 agent continuation 问题。

假设第一轮对话中 root agent transfer 到 `math_bot`，并且最后一个非 user event 是 `math_bot` 产出的。第二轮用户继续问 "那再算一下 2+3"，Runner 不应该强行从 root agent 开始，而应该把请求路由给上次活跃的 `math_bot`。

这就是 `findAgentToRun` 的意义：Session history 不只是记录，它还影响下一次运行入口。

Chapter 01 只需要理解这个机制的存在；真正的 transfer-to-agent 会在后续 Agent Flow / Multi-Agent 章节展开。

### 4.5 InvocationContext：把一次运行需要的东西装起来

Runner 创建上下文：

```go
ic := invctx.NewInvocationContext(invctx.Params{
    Ctx:          ctx,
    Agent:        agentToRun,
    RootAgent:    r.agent,
    Session:      sess,
    Memory:       r.memoryService,
    Artifact:     r.artifactService,
    InvocationID: invocationID,
    Branch:       agentToRun.Name(),
    UserContent:  message,
})
```

InvocationContext 是传给 Agent/Flow/Tool/Callback 的运行上下文。它里面至少要有：

- 当前 agent。
- root agent。
- 当前 session。
- memory/artifact services。
- invocation ID。
- branch。
- 原始 user content。

这就是为什么 ADK runtime 不能简单写成 `model.Generate(message)`。模型调用只是其中一步，真正运行时需要携带大量上下文。

### 4.6 Runner 只持久化 non-partial event

Agent 执行结束后：

```go
for _, ev := range sessEvents {
    if ev.Partial {
        continue
    }
    sess.AppendEvent(ev)
}
```

这条规则很关键：

- Partial event 可以返回给调用方，用于流式展示。
- Partial event 不写入 session，避免 session history 被 token chunk 污染。

测试 `TestRunnerPartialEventsNotPersisted` 覆盖的就是这个边界。

---

## 5. Agent：生命周期对象，不是模型本身

`agent/agent.go` 定义的 `Agent` 接口：

```go
type Agent interface {
    Name() string
    Description() string
    SubAgents() []Agent
    FindAgent(name string) Agent
    Parent() Agent
    DisallowTransferToParent() bool
    DisallowTransferToPeers() bool
}
```

这看起来不像一个"能回答问题"的接口。它更像 agent tree 里的元数据接口：名字、描述、子 agent、父 agent、transfer 限制。

真正能执行的是 Runner 侧定义的 `ExecutableAgent`：

```go
type ExecutableAgent interface {
    agent.Agent
    Execute(ctx agent.InvocationContext) ([]*event.Event, error)
}
```

也就是说，在这个复刻版里：

- `agent.Agent` 表达身份和树结构。
- `ExecutableAgent` 才表达可执行能力。

### 5.1 `baseAgent.Execute` 的生命周期

核心代码：

```go
func (a *baseAgent) Execute(ctx InvocationContext) ([]*event.Event, error) {
    var allEvents []*event.Event

    ev, err := runBeforeCallbacks(ctx, a.beforeAgentCallbacks)
    if err != nil {
        return nil, err
    }
    if ev != nil {
        return []*event.Event{ev}, nil
    }

    runEvents, err := a.run(ctx)
    if err != nil {
        return nil, err
    }
    allEvents = append(allEvents, runEvents...)

    ev, err = runAfterCallbacks(ctx, runEvents, a.afterAgentCallbacks)
    if err != nil {
        return nil, err
    }
    if ev != nil {
        allEvents = append(allEvents, ev)
        if ev.Actions.EndInvocation {
            ctx.EndInvocation()
        }
    }

    return allEvents, nil
}
```

一句话：

```text
before callbacks -> run -> after callbacks
```

before callback 可以 early exit：只要返回 event，就跳过 agent 的 `run`。after callback 可以追加事件，并且如果 event 上有 `EndInvocation`，就结束 invocation。

这说明 Agent 不是模型本身。Agent 是一个生命周期容器，它可以把任意 run 函数包起来，并在前后加扩展点。

### 5.2 `llmagent.New`：把 Flow 包成 Agent

`llmagent/llmagent.go` 是 Agent 和 Flow 的胶水：

```go
func New(name, description string, f *flow.Flow) (agent.Agent, error) {
    if f == nil {
        return nil, fmt.Errorf("llmagent: flow is required")
    }
    a, err := agent.New(agent.Config{
        Name:        name,
        Description: description,
        Run: func(ctx agent.InvocationContext) ([]*event.Event, error) {
            ic, ok := ctx.(invctx.InvocationContext)
            if !ok {
                return nil, fmt.Errorf("llmagent: expected context.InvocationContext, got %T", ctx)
            }
            return f.Run(ic)
        },
    })
    return a, err
}
```

这段代码告诉我们：

**LLM agent 的本质就是一个 baseAgent，它的 run 函数调用 `Flow.Run`。**

所以主链路继续展开：

```text
Runner.Run
  -> ExecutableAgent.Execute
      -> baseAgent.Execute
          -> llmagent run
              -> flow.Flow.Run
```

---

## 6. Flow：模型/工具多步循环

`flow/flow.go` 是 Chapter 01 的核心。`Flow` 结构体包含：

```go
type Flow struct {
    Model                model.LLM
    Tools                map[string]tool.FunctionTool
    Toolsets             []tool.Toolset
    PluginManager        *plugin.Manager
    RequestProcessors    []RequestProcessor
    ResponseProcessors   []ResponseProcessor
    BeforeModelCallbacks []BeforeModelCallback
    AfterModelCallbacks  []AfterModelCallback
    BeforeToolCallbacks  []BeforeToolCallback
    AfterToolCallbacks   []AfterToolCallback

    BeforeModelCallbacksCtx []BeforeModelCallbackCtx
    AfterModelCallbacksCtx  []AfterModelCallbackCtx
    BeforeToolCallbacksCtx  []BeforeToolCallbackCtx
    AfterToolCallbacksCtx   []AfterToolCallbackCtx
}
```

第一章只关注前两项：

- `Model`
- `Tools`

其余 callbacks、plugins、processors 是后续章节的扩展点。现在先知道它们都挂在 Flow 上即可。

### 6.1 `Flow.Run` 的循环

核心逻辑：

```go
func (f *Flow) Run(ctx context.InvocationContext) ([]*event.Event, error) {
    if f.Model == nil {
        return nil, fmt.Errorf("flow: model not configured for agent %q", ctx.Agent().Name())
    }

    var allEvents []*event.Event

    for step := 1; ; step++ {
        if ctx.Ended() {
            return allEvents, nil
        }

        stepEvents, err := f.runOneStep(ctx, step, allEvents)
        if err != nil {
            return allEvents, err
        }
        if len(stepEvents) == 0 {
            return allEvents, nil
        }
        allEvents = append(allEvents, stepEvents...)

        for _, ev := range stepEvents {
            if ev != nil && ev.Actions.EndInvocation {
                ctx.EndInvocation()
                return allEvents, nil
            }
        }

        modelEvent := stepEvents[0]
        if modelEvent.IsFinalResponse() {
            return allEvents, nil
        }
        if modelEvent.Partial {
            return allEvents, fmt.Errorf("flow: model event is partial (streaming limit reached)")
        }

        if len(stepEvents) > 1 {
            for _, ev := range stepEvents[1:] {
                if ev != nil && ev.Actions.TransferToAgent != "" {
                    return allEvents, nil
                }
            }
        }
    }
}
```

这段代码回答三个问题。

第一，Flow 是多 step 循环，不是单次 model call。

第二，正常终止靠：

```go
modelEvent.IsFinalResponse()
```

第三，每个 step 可能产生不止一个 event：

- 第一个是 model event。
- 后面可能是 tool event。
- 如果 transfer，后面还可能接 transfer target agent 的 events。

### 6.2 为什么 `runOneStep` 要接收 `priorEvents`

`runOneStep` 的开头：

```go
history := append([]*event.Event{}, ctx.Session().Events()...)
history = append(history, priorEvents...)
req := &model.LLMRequest{
    Model:    f.Model.Name(),
    Contents: model.ContentsFromEvents(history),
}
```

这是本章最关键的实现细节之一。

Session 里已经有历史事件，包括这次 invocation 开始时写入的 user event。但同一次 invocation 内，第一轮 model event 和 tool event 还没被 Runner 持久化到 session，因为 Runner 要等 Agent.Execute 返回后才统一持久化。

如果第二轮 model request 只看 `ctx.Session().Events()`，它就看不到刚刚发生的：

```text
model function call
tool function response
```

真实 LLM 第二轮必须看到这些，否则它不知道工具已经执行过，也不知道工具结果是什么。

所以 Flow 把 `priorEvents` 也拼进去：

```text
model request history = persisted session events + events produced earlier in same invocation
```

测试 `TestFlowFeedsPriorStepEventsIntoNextModelRequest` 就是为了锁住这个行为。真实 OpenAI-compatible/DeepSeek smoke 也依赖这点。

### 6.3 `runOneStep` 的六步

`runOneStep` 可以拆成：

```text
1. 从 session events + priorEvents 构造 LLMRequest
2. preprocess
3. inject tool declarations + transfer tool
4. call model
5. postprocess + finalize model event
6. handle function calls + optional transfer
```

代码结构：

```go
req := &model.LLMRequest{
    Model:    f.Model.Name(),
    Contents: model.ContentsFromEvents(history),
}

ev, err := f.preprocess(ctx, req)
if ev != nil { return []*event.Event{ev}, nil }

f.injectToolDeclarations(req)
f.injectTransferTool(currentAgent, req)

resp, err := f.callModel(ctx, req, modelActions)
f.postprocess(ctx, req, resp)

modelEvent := f.finalizeModelResponseEvent(ctx, step, resp, modelActions)
events := []*event.Event{modelEvent}

toolEvent, tt := f.handleFunctionCalls(ctx, step, modelEvent)
if toolEvent != nil { events = append(events, toolEvent) }
```

Chapter 01 可以先把 preprocess/postprocess 当作空扩展点。它们的作用在后面 Callback / Plugin / Instruction 章节展开。

### 6.4 `callModel` 的扩展顺序

`callModel` 不是直接调用：

```go
f.Model.GenerateContent(req)
```

它的顺序是：

```text
Plugin before model
Context-aware before model callbacks
Legacy before model callbacks
Model.GenerateContent
Plugin on model error
Plugin after model
Context-aware after model callbacks
Legacy after model callbacks
```

这说明 Model call 是 Flow 的中间点，不是不可拦截的黑盒。缓存、日志、限流、重试、prompt 注入、测试替身，都可以围绕这个点做。

第一章不需要展开每种 callback，只要讲清楚这条事实：

**Flow 是 runtime loop，Model 是 Flow 中被调用的一步。**

### 6.5 function call 如何变成 tool event

Model event 生成后，Flow 检查其中的 function calls：

```go
fnCalls := modelEvent.FunctionCalls()
if len(fnCalls) == 0 {
    return nil, nil
}
```

如果有 function calls，就并行执行：

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

并发执行但结果 slice 保持原始顺序，因为每个 goroutine 写回自己的 `idx`。

然后：

```go
merged := mergeResultsToEvent(ctx, step, results)
```

得到一个 tool event。这个 event 的 content parts 是 function responses，之后会被 `ContentsFromEvents` 转回 model request history。

这就是 tool-calling loop 的闭环：

```text
model event contains FunctionCall
  -> Flow executes tool
  -> tool event contains FunctionResponse
  -> next model request includes both FunctionCall and FunctionResponse
```

---

## 7. Event：运行时事实的统一载体

`event.Event` 是整个 runtime 的中心数据结构：

```go
type Event struct {
    ID        string
    Author    string
    Role      Role
    Content   *Content
    Actions   EventActions
    Partial   bool
    Timestamp time.Time
    Branch    string

    Error        error
    ErrorCode    string
    ErrorMessage string
    Interrupted  bool
    TurnComplete bool
}
```

它同时承载：

- 谁产生的：`Author`
- 角色是什么：`Role`
- 内容是什么：`Content`
- 有什么副作用：`Actions`
- 是否是流式片段：`Partial`
- 属于哪个 agent branch：`Branch`
- 是否错误/中断/turn complete

这就是为什么 ADK runtime 用 event 串起来，而不是用一堆临时变量串起来。Event 是 model/tool/agent/session 之间的共同语言。

### 7.1 Content 和 Part

`Content` 是：

```go
type Content struct {
    Role  Role
    Parts []Part
}
```

`Part` 可以是：

```go
type Part struct {
    Text             string
    FunctionCall     *FunctionCall
    FunctionResponse *FunctionResponse
    Thought          bool
}
```

也就是说，一个 event 可以表示：

- 普通文本。
- 模型请求工具调用。
- 工具返回结果。

这三种内容统一在 `Content.Parts` 里，Flow 才能用同一套 `ContentsFromEvents` 逻辑构造下一轮 model request。

### 7.2 `EventActions`

`EventActions` 是副作用通道：

```go
type EventActions struct {
    StateDelta                 map[string]any
    ArtifactDelta              map[string]int64
    TransferToAgent            string
    EndInvocation              bool
    Escalate                   bool
    SkipSummarization          bool
    RequestedToolConfirmations map[string]ToolConfirmation
}
```

Chapter 01 先记三项：

- `StateDelta`：后续 Chapter 02 讲 state 写入。
- `TransferToAgent`：后续 Chapter 07 讲 agent transfer。
- `EndInvocation`：Agent/Flow 可以提前结束 invocation。

这解释了为什么 Event 不只是 content。它还携带 runtime 应该执行的动作。

### 7.3 `IsFinalResponse`

Flow 停止循环依赖：

```go
func (e *Event) IsFinalResponse() bool {
    if e == nil || e.Partial {
        return false
    }
    if e.Interrupted {
        return false
    }
    if e.Error != nil || e.ErrorCode != "" {
        return false
    }
    if e.Actions.TransferToAgent != "" {
        return false
    }
    if c := e.Content; c != nil {
        for _, p := range c.Parts {
            if p.FunctionCall != nil {
                return false
            }
        }
    }
    return true
}
```

它不是简单地判断"有没有文本"。一个 event 要成为 final response，必须满足：

- 不是 partial。
- 没有 interrupted。
- 没有 error。
- 没有 transfer。
- content 中没有 function call。

这就是 Flow 的终止语义。

所以"没有 function call"只是必要条件的一部分，不是完整条件。

---

## 8. Model：真实 LLM 之前先用 FakeModel 固定控制流

`model.LLM` 接口非常小：

```go
type LLM interface {
    Name() string
    GenerateContent(req *LLMRequest) (*LLMResponse, error)
}
```

`LLMRequest` 包含：

```go
type LLMRequest struct {
    Model             string
    SystemInstruction string
    Contents          []LLMContent
    ToolDeclarations  []any
}
```

Chapter 01 的核心是 `Contents`：它来自 session events + priorEvents。

### 8.1 FakeModel 的意义

FakeModel 是一个队列：

```go
type FakeModel struct {
    responses []*LLMResponse
    nextIdx   int
}
```

每次调用：

```go
resp := m.responses[m.nextIdx]
m.nextIdx++
return resp, nil
```

如果队列用完：

```go
return nil, fmt.Errorf("model %q: no more queued responses (called %d times)", m.name, m.nextIdx)
```

为什么要这样设计？

因为 Chapter 01 要测试 runtime 控制流，而不是测试 LLM 智能。FakeModel 让测试可以确定：

- 第一轮一定输出 function call。
- 工具一定被执行。
- 第二轮一定输出 final text。
- Session 最终一定有预期事件数量。

真实 LLM smoke 放到后面验证 provider adapter，而不是用来证明核心 runtime。

### 8.2 `ContentsFromEvents`

`model/contents.go` 把 event history 转成 model request contents：

```go
func ContentsFromEvents(events []*event.Event) []LLMContent {
    contents := make([]LLMContent, 0, len(events))
    for _, ev := range events {
        if ev == nil || ev.Content == nil || len(ev.Content.Parts) == 0 {
            continue
        }
        content := LLMContent{
            Role:  string(ev.Content.Role),
            Parts: make([]LLMPart, 0, len(ev.Content.Parts)),
        }
        for _, part := range ev.Content.Parts {
            content.Parts = append(content.Parts, LLMPart{
                Text:             part.Text,
                FunctionCall:     cloneFunctionCall(part.FunctionCall),
                FunctionResponse: cloneFunctionResponse(part.FunctionResponse),
            })
        }
        contents = append(contents, content)
    }
    return contents
}
```

注意它保留三类信息：

- Text。
- FunctionCall。
- FunctionResponse。

这对真实 LLM 非常重要。OpenAI-compatible tool calling 第二轮必须看到上一轮 assistant tool call 和 tool response，否则模型无法合规地继续生成最终答案。

测试 `TestContentsFromEventsPreservesToolLoop` 锁定了这个转换行为。

---

## 9. Session：运行结果如何留到下一轮

Runner 开始时把 user event 写入 session。Agent 执行结束后，把 non-partial events 写入 session。

天气 demo 最终 session 应该包含四类事件：

```text
1. user: What's the weather in Tokyo?
2. model: function call get_weather(city=Tokyo)
3. tool: function response get_weather -> weather data
4. model: final answer
```

这四个事件就是下一轮对话的历史。

为什么 tool event 也要持久化？因为下一轮或调试时需要知道模型当时为什么得出最终答案。只保存最终文本会丢失推理链路中的结构化事实。

为什么 partial event 不持久化？因为 partial 是流式过程片段，不是稳定事实。它可能是半句话、半个 JSON 参数、半个工具调用 delta。写入 session 反而会污染历史。

---

## 10. 一次天气查询的完整事件时间线

以 demo 为例，具体走一遍。

### Step 0：Runner 写入用户事件

调用：

```go
r.Run(ctx, "user-1", "sess-1", "What's the weather in Tokyo?")
```

Runner 创建：

```text
ID=sess-1-user-1
Author=user
Role=user
Content.Text="What's the weather in Tokyo?"
```

并 append 到 session。

### Step 1：Flow 第一轮模型调用

`runOneStep(step=1, priorEvents=[])` 构造 request：

```text
Contents:
  user: What's the weather in Tokyo?
```

FakeModel 返回：

```text
model text: Let me check the weather.
function_call: get_weather({city: Tokyo})
```

Flow 把它 finalize 成 model event：

```text
ID=sess-1-inv-2-step-1
Author=weather_bot
Role=model
Parts:
  Text("Let me check the weather.")
  FunctionCall(get_weather)
```

`IsFinalResponse()` 返回 false，因为 content 里有 function call。

### Step 1：Flow 执行工具

`handleFunctionCalls` 看到 `get_weather`，调用工具：

```go
weatherTool.Run(args)
```

得到：

```text
city=Tokyo
temperature=22
condition=sunny
humidity=45%
```

Flow 合并成 tool event：

```text
Role=tool
Parts:
  FunctionResponse(get_weather => weather data)
```

本 step 返回两个 event：

```text
[model function call event, tool response event]
```

### Step 2：Flow 第二轮模型调用

`Flow.Run` 继续循环。此时 `allEvents` 里已有 step 1 的两个 events。

`runOneStep(step=2, priorEvents=allEvents)` 构造 request：

```text
Contents:
  user: What's the weather in Tokyo?
  model: function_call get_weather
  tool: function_response get_weather -> weather data
```

FakeModel 返回最终文本：

```text
The weather in Tokyo is 22°C and sunny with 45% humidity.
```

Flow finalize 成 model event。`IsFinalResponse()` 返回 true，因为：

- 不是 partial。
- 没有 interrupted/error。
- 没有 transfer。
- 没有 function call。

Flow 结束。

### Step 3：Runner 持久化 produced events

Agent.Execute 返回三个 produced events：

```text
model function call
tool response
model final
```

Runner 遍历它们，跳过 partial，全部 append 到 session。

最终 session 里有：

```text
user
model(function call)
tool(function response)
model(final)
```

这就是完整闭环。

---

## 11. 测试如何证明这条链路

第一章可以重点看几组测试。

### 11.1 `TestRunnerSimpleTextRun`

场景：

- FakeModel 直接返回文本。
- 没有工具调用。
- Runner 返回一个 produced event。
- Session 里有 user event + model final event。

它证明最简单的对话路径可用。

### 11.2 `TestRunnerToolCallAndFinalResponse`

场景：

- FakeModel 第一轮返回 function call。
- Tool 执行。
- FakeModel 第二轮返回 final response。
- Session 持久化 user、model call、tool result、model final。

它证明 model/tool loop 可用。

### 11.3 `TestFlowOneToolCallThenFinalResponse`

它更靠近 Flow 层，验证 Flow 不依赖 Runner 也能完成：

```text
model call -> tool result -> final model response
```

如果这个测试失败，说明问题在 Flow loop；如果 Runner 测试失败但 Flow 测试过，问题可能在 session/runner 持久化。

### 11.4 `TestFlowFeedsPriorStepEventsIntoNextModelRequest`

这是最重要的真实 LLM 风险测试。

它验证第二轮 model request 里包含同一次 invocation 的 prior step events。没有这个行为，FakeModel 可能还能过，因为它不真的读 request；真实 LLM 会失败。

### 11.5 `TestRunnerPartialEventsNotPersisted`

它验证：

- Partial event 可以被 produced。
- Runner 不把 partial event append 到 session。

这是 streaming 语义的持久化边界。

### 11.6 `TestOpenAICompatibleModelToolCallResponse`

这个测试不直接属于 Runner 主链路，但它说明真实 provider adapter 能把 OpenAI-compatible response 解析成复刻 runtime 的 `FunctionCall` / `FunctionResponse` 结构。

教学上可以把它放在最后：FakeModel 证明控制流，OpenAI-compatible tests 证明真实模型格式能接上。

---

## 12. 容易误解的点

### 12.1 "Agent 就是 LLM"

不是。Agent 是生命周期对象和树结构节点；LLM 是 Flow 里的 `Model` 字段。`llmagent.New` 只是把 Flow 包成 Agent。

### 12.2 "Runner 每次都跑 root agent"

不是。Runner 会从 session history 反向扫描最后活跃的非 user agent，并检查 transfer 约束。这样 transfer 后的 specialist 可以接续处理下一轮。

### 12.3 "Session 只保存最终答案"

不是。Session 保存 user event、model function call event、tool response event、final model event。这样下一轮模型和调试工具都能看到完整结构化历史。

### 12.4 "Function call 后工具结果会直接返回给用户"

不是。工具结果先变成 tool event，再进入下一轮 model request。最终是否直接显示给用户，取决于模型下一轮是否生成 final response，或后续章节中的 direct-return/exit-loop 策略。

### 12.5 "Partial event 丢了"

不是。Partial event 可以返回给调用方用于流式展示，只是不写入 session。它是传输事件，不是稳定历史。

### 12.6 "FakeModel 通过就代表真实 LLM 一定通过"

不是。FakeModel 只验证 runtime 控制流。真实 LLM 还依赖 provider adapter、tool declaration、history 回灌、function response 格式。`cmd/realllm` 和 OpenAI-compatible tests 用来覆盖这部分。

### 12.7 "Flow 终止条件就是没有 function call"

不完整。`IsFinalResponse` 还检查 partial、interrupted、error、transfer。没有 function call 只是其中一项。

### 12.8 "Tool call 并发会打乱结果顺序"

工具是 goroutine 并发执行，但结果写回固定 index 的 slice。合并时仍按原始 function call 顺序。

---

## 13. 课堂讲解脚本

如果只有 16 分钟，可以这样讲。

### 第 0-2 分钟：用天气 demo 立 flag

展示：

```text
User -> model function call -> tool result -> model final answer
```

告诉听众：今天只讲这条链路怎么落地。

### 第 2-4 分钟：画六层主链路

写：

```text
Runner -> Agent -> Flow -> Model/Tool -> Event -> Session
```

一句话解释：

- Runner 管 session 和持久化。
- Agent 管生命周期。
- Flow 管循环。
- Model/Tool 管能力。
- Event 管事实。
- Session 管历史。

### 第 4-7 分钟：走 `Runner.Run`

投屏 `runner/runner.go`：

- Get/Create session。
- 创建 user event。
- append user event。
- findAgentToRun。
- NewInvocationContext。
- Execute agent。
- append non-partial events。

重点讲 partial 不持久化。

### 第 7-9 分钟：走 `llmagent.New` 和 `agent.Execute`

展示：

```text
before callbacks -> f.Run(ic) -> after callbacks
```

说明 Agent 是 Flow 的生命周期壳。

### 第 9-13 分钟：走 `Flow.Run/runOneStep`

画：

```text
build request -> call model -> model event -> tool event -> loop
```

重点讲 `priorEvents`：

```text
session history + same-invocation prior events
```

这是 tool-calling loop 能在真实 LLM 上闭环的关键。

### 第 13-15 分钟：讲 `Event.IsFinalResponse`

展示终止条件：

- not partial
- no interrupted
- no error
- no transfer
- no function call

### 第 15-16 分钟：用测试收口

推荐测试：

- `TestRunnerSimpleTextRun`
- `TestRunnerToolCallAndFinalResponse`
- `TestFlowFeedsPriorStepEventsIntoNextModelRequest`
- `TestRunnerPartialEventsNotPersisted`

---

## 14. 实战阅读任务

### 任务 1：手动追踪天气 demo

打开 `cmd/demo/main.go` 的 `runChapter01()`，把每个对象标到主链路上：

- `weatherTool` 属于 Tool。
- `fakeModel` 属于 Model。
- `flow.Flow` 属于 Flow。
- `llmagent.New` 生成 Agent。
- `runner.New` 生成 Runner。
- `r.Run` 产生 Event 并写 Session。

### 任务 2：画出 session 最终事件

运行 demo 后，写出 session 中四个事件：

```text
1. user text
2. model function call
3. tool function response
4. model final text
```

说明每个事件的 `Author`、`Role`、`Content.Parts`。

### 任务 3：解释 `priorEvents`

回答：

- 为什么 Runner 不在每个 step 后立刻持久化 event？
- 为什么 Flow 第二轮仍然必须看到第一轮 model/tool events？
- `ctx.Session().Events()` 和 `priorEvents` 分别代表什么？

### 任务 4：修改 FakeModel 队列

把 FakeModel 第二个 response 删除，只保留 function call response。预测会发生什么。

答案：第一轮工具执行后，Flow 进入第二轮 model call；FakeModel 队列用完，返回 `"no more queued responses"` 错误。

### 任务 5：构造 partial event

写一个 agent run 函数返回 partial event + final event。验证 Runner 只持久化 final event。

---

## 15. 自测题

1. `Runner.Run` 为什么要先把用户消息写成 user event？
2. `findAgentToRun` 为什么要跳过 `Author == "user"` 的事件？
3. `ExecutableAgent` 比 `agent.Agent` 多了什么能力？
4. `llmagent.New` 如何把 Flow 接到 Agent 上？
5. `Flow.Run` 的正常终止条件是什么？
6. `runOneStep` 为什么要把 `priorEvents` 拼到 session events 后面？
7. `Event.Content.Parts` 可以承载哪三类核心内容？
8. `Partial` event 为什么不持久化？
9. FakeModel 的响应队列用完会发生什么？
10. 多个 function call 并行执行时，结果顺序如何保持？
11. `IsFinalResponse` 除了没有 function call，还检查哪些条件？
12. 为什么真实 LLM smoke 不能被 FakeModel 测试完全替代？

参考答案：

1. 因为 Flow 构造 model request 时从 session events 读取 history；用户消息必须先进入 history。
2. 因为 user event 不是 agent 活跃状态，不能作为下一轮 agent routing 的依据。
3. 多了 `Execute(ctx) ([]*event.Event, error)`。
4. 它创建一个 baseAgent，把 `Run` 函数设置为类型断言 InvocationContext 后调用 `f.Run(ic)`。
5. `modelEvent.IsFinalResponse()` 返回 true，或 context ended / no step events / transfer 后收口等边界。
6. 同一次 invocation 内前面 step 的 model/tool events 尚未持久化，但下一轮 model 必须看到它们。
7. Text、FunctionCall、FunctionResponse。
8. Partial 是流式过程片段，不是稳定历史；写入 session 会污染后续 request。
9. 返回 `"no more queued responses"` 错误。
10. goroutine 并发执行，但每个结果写入原 function call index 对应的位置。
11. not partial、not interrupted、no error、no transfer。
12. FakeModel 不真实解析 request/history/tool declaration；真实 LLM 还要验证 provider adapter 和 tool-calling 格式。

---

## 16. 本章一句话总结

ADK Go 复刻版的第一章不是在讲"怎么问模型一句话"，而是在讲一次 invocation 的运行事实如何被组织起来：Runner 管会话入口和持久化，Agent 管生命周期，Flow 管 model/tool 循环，Event 统一承载文本、函数调用、工具结果和副作用，Session 保存稳定历史。理解这条主链路，后面的 State、Tool、Callback、Workflow、ReAct 才都有位置可放。

