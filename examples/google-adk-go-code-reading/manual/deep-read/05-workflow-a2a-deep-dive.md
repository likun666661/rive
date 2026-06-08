# 精读报告：Workflow Agents / AgentTool / Remote A2A

## 1. problem

`google/adk-go` 的多智能体编排层解决以下核心问题：

| 问题域 | 模块 | 解决的编排问题 |
|--------|------|---------------|
| **顺序编排** | `workflowagents/sequentialagent` | 多个子 agent 按固定顺序依次执行，前置 agent 的输出作为后置 agent 的上下文，适合 pipeline 模式（如代码生成→审查→修复） |
| **并行编排** | `workflowagents/parallelagent` | 多个子 agent 并发执行并各自产出事件流，适合多视角分析、多算法对比、并行搜索后汇总 |
| **循环编排** | `workflowagents/loopagent` | 子 agent 集合反复迭代执行，直到达到指定次数或触发 escalate 终止条件，适合迭代优化场景（如代码修复循环） |
| **Agent as Tool** | `tool/agenttool` | 将任意 agent 封装为普通 tool，使得 LLM agent 可以在 function call 中调用子 agent，实现细粒度的 "当需要某能力时按需委派" |
| **远程 A2A** | `agent/remoteagent/v2` | 将运行在其他进程/主机上的 agent 透明地作为本地子 agent 使用，通过 Agent-to-Agent 协议通信，支持流式响应 |

所有这些机制的共同目标：**将 agent 的 `Run(ctx) -> iter.Seq2[*session.Event, error]` 统一为可组合的迭代器**，使得编排层可以对事件流进行转发、聚合、拆分或转换。

## 2. why_hard

### 2.1 状态共享与隔离

- **Sequential agent**：子 agent 共享同一个 session，前置 agent 的 `StateDelta` 可以被后置 agent 感知。但如果子 agent 内部修改了 session，错误的状态传播可能导致非确定性行为。
- **Parallel agent**：每个子 agent 运行在独立 goroutine 中，理论上应该隔离 — 但当前实现共享同一个 session（`icontext.NewInvocationContext` 传入同一个 Session）。这意味着并行写入 session 存在竞态条件（见 `parallelagent/agent_test.go:239-291` 中的 `TestParallelAgentWithTools`）。
- **AgentTool**：为被调用的子 agent 创建**全新的独立 session**（`agent_tool.go:168`），并将父 session 的非内部状态复制到子 session。这提供了合理的隔离，但状态复制可能遗漏 runtime 状态。
- **Loop agent**：子 agent 在同一 session 上多次执行，状态在迭代间累积。需要确保上一次迭代的状态不会被下一迭代意外污染。

### 2.2 事件流聚合

- **Parallel agent 的核心难点**：多个 goroutine 并发产出事件，如何保证 runner 能按序处理？如何防止子 agent 产出事件时 runner 尚未处理完上一个事件？
- **解决方案**：通过 `resultsChan` + `ackChan` 机制（见后文源码分析）。
- **Remote A2A**：服务端以流式 `TaskArtifactUpdateEvent` 返回，客户端需要对 partial events 做聚合，并在 terminal event 到达时 flush 所有累积的 artifact。

### 2.3 错误传播

- 每个编排层都需要处理：子 agent 返回错误时，是否停止其他子 agent？如何通知调用的 runner？
- **Sequential**：遇到错误立即 `return`，停止后续子 agent。
- **Parallel**：通过 `errgroup` 收集所有 goroutine 的错误；一旦某个子 agent 出错，`errGroup.Wait()` 返回错误并通过 `resultsChan` 传播。其他 goroutine 通过 `doneChan` 接受取消信号。
- **Loop**：子 agent 出错时 `yield(event, err)`，由 runner 决定是否停止循环。
- **Remote A2A**：网络错误、转换错误、协议错误需要分别处理，且需要 `RemoteTaskCleanupCallback` 机制来清理远端任务。

### 2.4 Parallel Backpressure

这是 parallel agent 最复杂的部分。如果 runner 消费事件的速度慢于子 agent 生产事件的速度，需要背压机制。实现通过 `ackChan` 确保 runner 处理完当前事件后，子 agent 才能产下一个事件。

### 2.5 协议兼容（Remote A2A）

- A2A 协议有 v0（legacy）和 v1 两个版本。`agent/remoteagent/a2a_agent.go`（deprecated）通过 `a2av0.ToV1AgentCard`、`a2av0.FromV1Event` 等双向转换函数桥接两个版本。
- 服务端 `server/adka2a/executor.go`（deprecated）同样做双向转换。
- `v2` 包是纯净的 v1 协议实现，legacy 包所有逻辑最终都委托给 v2。

### 2.6 Remote Streaming 复杂性

- A2A 协议的 streaming 模式下，服务端返回 `iter.Seq2[a2a.Event, error]`。客户端需要处理多种 event 类型（`Task`、`Message`、`TaskArtifactUpdateEvent`、`TaskStatusUpdateEvent`），并且需要正确关联 `Append`/`LastChunk` 标志来做 partial aggregation。
- 在服务端侧，ADK 的 `session.Event` 需要转换为 A2A event，反之亦然。期间需要处理 `Partial` 标志、`LongRunningToolIDs`、`CitationMetadata`、`GroundingMetadata` 等丰富的 GenAI 元数据。

## 3. design_approach

### 3.1 分层架构

```
┌──────────────────────────────────────────────┐
│           Runner (runner/runner.go)          │  ← 统一编排入口
├──────────────────────────────────────────────┤
│  Workflow Agents  │  AgentTool  │ Remote A2A │  ← 编排变体
│  (sequential/     │  (agent as  │ (remote    │
│   parallel/loop)  │   tool)     │  client)   │
├──────────────────────────────────────────────┤
│         Base Agent (agent/agent.go)          │  ← 统一 agent 接口
├──────────────────────────────────────────────┤
│         Session / Model / Tool / Memory      │  ← 基础设施
└──────────────────────────────────────────────┘
```

### 3.2 Workflow Agent Variants 设计

所有 workflow agent 都遵循同一模式：

1. 调用 `agent.New(cfg.AgentConfig)` 创建基础 agent。
2. 将 `cfg.AgentConfig.Run` 替换为自定义的编排逻辑（禁止用户提供自定义 `Run`）。
3. 通过 `agentinternal.Reveal` 获取内部 state，设置 `AgentType`（`TypeSequentialAgent`/`TypeParallelAgent`/`TypeLoopAgent`）。
4. 自定义 `Run` 函数遍历 `ctx.Agent().SubAgents()` 并编排执行。

**核心约定**：`Run(ctx)` 返回 `iter.Seq2[*session.Event, error]`，这是一个惰性迭代器。编排层只需委托、转发或合并子 agent 的迭代器。

### 3.3 AgentTool Sandbox 设计

AgentTool 将 agent 封装为 tool 的三个关键决策：

1. **独立 session**：调用 `session.InMemoryService().Create()` 创建全新的 session，复制父 session 的非内部状态（过滤 `_adk` 前缀的 key）。
2. **独立 runner**：创建新的 `runner.Runner` 实例，使用独立的 `ArtifactService` 和 `MemoryService`。
3. **类型化输入/输出**：如果子 agent 是 LLM agent 且声明了 `InputSchema`/`OutputSchema`，则在调用前后进行 JSON Schema 校验。
4. **SkipSummarization 控制**：配置项允许跳过父 agent 的 summarization，直接透传结果。

### 3.4 Remote Agent Client / Processor 分层

```
┌─────────────────────────────────────────────┐
│  agent/remoteagent/v2/a2a_agent.go          │ ← NewA2A(), a2aAgent.run()
│  - AgentCard 解析（URL/文件/静态）          │
│  - A2AClientProvider 创建 client            │
│  - 消息构造（session 增量同步）              │
│  - 流式/非流式模式分发                       │
│  - cleanupRemoteTask 终止清理                │
├─────────────────────────────────────────────┤
│  a2a_agent_run_processor.go                 │ ← runProcessor
│  - BeforeRequestCallbacks 调用               │
│  - convertToSessionEvent (A2A→Session)      │
│  - aggregatePartial (partial 事件聚合)       │
│  - AfterRequestCallbacks 调用                │
│  - updateCustomMetadata                     │
├─────────────────────────────────────────────┤
│  client.go                                  │ ← A2AClient interface
│  - SendMessage / SendStreamingMessage       │
│  - CancelTask / Destroy                     │
├─────────────────────────────────────────────┤
│  utils.go                                   │ ← 消息构造工具
│  - getUserFunctionCallAt (long-running tool)│
│  - toMissingRemoteSessionParts (增量同步)   │
│  - presentAsUserMessage (多 agent 对话历史)  │
└─────────────────────────────────────────────┘
```

### 3.5 A2A Server Executor 分层

```
┌─────────────────────────────────────────────┐
│  server/adka2a/v2/executor.go               │ ← Executor.Execute()
│  - 消息解析 (toGenAIContent)                 │
│  - RunnerProvider 创建 runner                │
│  - HandleInputRequired 处理                  │
│  - Session 准备 (get-or-create)              │
│  - 事件循环：r.Run() → processor.process()   │
│  - writeFinalTaskStatus                     │
├─────────────────────────────────────────────┤
│  processor.go                               │ ← eventProcessor
│  - ADK event → A2A event 转换                │
│  - Part 转换 (GenAI ↔ A2A)                  │
│  - 失败检测 (ErrorCode/ErrorMessage)         │
│  - makeFinalStatusUpdate                    │
├─────────────────────────────────────────────┤
│  events.go                                  │ ← ToSessionEvent 等
│  - A2A event → ADK event 反向转换            │
│  - TaskArtifactUpdateEvent → Event           │
│  - TaskStatusUpdateEvent → Event             │
│  - Task → Event                              │
│  - Message → Event                           │
├─────────────────────────────────────────────┤
│  parts.go / metadata.go / task_artifact.go  │ ← 底层转换工具
└─────────────────────────────────────────────┘
```

## 4. code_walkthrough

### 4.1 Sequential Agent (`agent/workflowagents/sequentialagent/agent.go`)

**核心结构**（:34-38）：

```go
type seqAgent struct {
    agent.Agent          // 嵌入基础 agent
    *agentinternal.State // 内部状态（AgentType 等）
    impl *sequentialAgent // 实际的 Run 实现
}
```

**`Run` 方法**（:78-89）：

```go
func (a *sequentialAgent) Run(ctx agent.InvocationContext) iter.Seq2[*session.Event, error] {
    return func(yield func(*session.Event, error) bool) {
        for _, subAgent := range ctx.Agent().SubAgents() {
            for event, err := range subAgent.Run(ctx) {
                if !yield(event, err) {
                    return  // consumer 停止消费
                }
            }
        }
    }
}
```

关键语义：遍历 `SubAgents()`，每个子 agent 的迭代器完全消费后，才进入下一个。这是最简洁的编排 — 仅 11 行。

**`RunLive` 方法**（:125-204）：

`RunLive` 支持双向实时通信。实现要点：
1. 检查子 agent 是否实现 `RunLive` 方法。
2. 注入 `task_completed` function tool 到 LLM 子 agent，通过 instruction suffix 告诉 LLM 何时调用。
3. 创建 `sequentialLiveSession`，随着子 agent 切换，动态改变 `activeSess`。
4. 每个子 agent 完成后关闭其 session。

**`sequentialLiveSession`**（:91-123）：

使用 `sync.Mutex` 保护并发访问。`Send` 方法将请求路由到当前活跃的子 session。仅在子 agent 切换期间存在竞态窗口 — 在旧 session 关闭和新 session 设置之间。

### 4.2 Parallel Agent (`agent/workflowagents/parallelagent/agent.go`)

**`run` 函数**（:67-128）是整个 ADK Go 中最复杂的编排实现。逐段分析：

**第一阶段：启动 goroutine**（:70-100）：

```go
errGroup, errGroupCtx = errgroup.WithContext(ctx)
doneChan = make(chan bool)
resultsChan = make(chan result)

for _, sa := range ctx.Agent().SubAgents() {
    branch := fmt.Sprintf("%s.%s", curAgent.Name(), sa.Name())
    subAgent := sa
    errGroup.Go(func() error {
        subCtx := icontext.NewInvocationContext(errGroupCtx, ...)  // 每个子 agent 获得独立 branch
        return runSubAgent(subCtx, subAgent, resultsChan, doneChan)
    })
}
```

要点：
- 使用 `golang.org/x/sync/errgroup` 管理 goroutine 生命周期。
- 每个子 agent 获得一个唯一的 `branch` 标识（`parent.child`），用于 session 事件隔离。
- `errGroupCtx` 用于：任意子 agent 出错时，取消所有其他子 agent。

**第二阶段：错误收集 goroutine**（:102-110）：

```go
go func() {
    if err := errGroup.Wait(); err != nil {
        select {
        case resultsChan <- result{err: err}:
        case <-doneChan:
        }
    }
    close(resultsChan)
}()
```

`errGroup.Wait()` 在所有 goroutine 完成后（无论成功或失败）返回。如果返回 error，尝试写入 `resultsChan`；如果 `doneChan` 已关闭（runner 停止消费），则忽略。

**第三阶段：背压迭代器**（:112-128）：

```go
return func(yield func(*session.Event, error) bool) {
    defer close(doneChan)
    for res := range resultsChan {
        shouldContinue := yield(res.event, res.err)
        if res.ackChan != nil {
            close(res.ackChan)  // 信号：runner 已完成处理
        }
        if !shouldContinue {
            break
        }
    }
}
```

每次从 `resultsChan` 读取一个结果，yield 给 runner 后，关闭 `ackChan` 通知子 agent 可以继续。

**`runSubAgent`**（:130-158）：

```go
func runSubAgent(ctx agent.InvocationContext, agent agent.Agent, results chan<- result, done <-chan bool) error {
    for event, err := range agent.Run(ctx) {
        if err != nil { return err }
        ackChan := make(chan struct{})
        select {
        case <-done:           return nil     // runner 停止，优雅退出
        case <-ctx.Done():     return ctx.Err() // 上下文取消
        case results <- result{event: event, ackChan: ackChan}:
            select {
            case <-ackChan:    // runner 已处理，继续循环
            case <-done:       return nil
            case <-ctx.Done(): return ctx.Err()
            }
        }
    }
    return nil
}
```

**背压机制关键**：`results <- result{ackChan}` 写入后，**阻塞等待 `ackChan` 关闭**。只有在 runner 处理完当前事件并 `close(res.ackChan)` 后，`runSubAgent` 才会进入下一次循环继续生产事件。

**`result` 结构体**（:160-164）：

```go
type result struct {
    event   *session.Event
    err     error
    ackChan chan struct{}  // nil 表示错误事件，无需 ack
}
```

**Backpressure 流程总结**：

```
子 Agent goroutine                resultsChan           Runner
      │                              │                    │
      ├── event1 + ackChan1 ────────►│                    │
      │   (阻塞等待 ack)              ├── event1 ────────►│
      │                              │                    │── 处理 event1
      │                              │◄── close(ackChan1)─│
      │◄── ackChan1 关闭             │                    │
      ├── event2 + ackChan2 ────────►│                    │
      │   ...                        │                    │
```

### 4.3 Loop Agent (`agent/workflowagents/loopagent/agent.go`)

**核心逻辑**（:75-104）：

```go
func (a *loopAgent) Run(ctx agent.InvocationContext) iter.Seq2[*session.Event, error] {
    count := a.maxIterations
    return func(yield func(*session.Event, error) bool) {
        for {
            shouldExit := false
            for _, subAgent := range ctx.Agent().SubAgents() {
                for event, err := range subAgent.Run(ctx) {
                    if !yield(event, err) { return }
                    if event != nil && event.Actions.Escalate {
                        shouldExit = true  // 子 agent 通过 escalate 信号终止循环
                    }
                }
                if shouldExit { return }
            }
            if count > 0 {
                count--
                if count == 0 { return }
            }
        }
    }
}
```

关键语义：
- **无限循环**：`MaxIterations == 0` 时无限执行（`count` 不减）。
- **Escalate 终止**：子 agent 可以通过 `event.Actions.Escalate = true` 主动终止循环（通过 function tool 设置，如 `loopagent/agent_test.go:309-312`）。
- **无 RunLive 支持**：Loop agent 不实现 `RunLive`。

### 4.4 Agent as Tool (`tool/agenttool/agent_tool.go`)

**Declaration 生成**（:86-116）：

```go
func (t *agentTool) Declaration() *genai.FunctionDeclaration {
    decl := &genai.FunctionDeclaration{
        Name: t.Name(), Description: t.Description(),
    }
    if agentInputSchema != nil {
        decl.Parameters = agentInputSchema   // 使用 LLM agent 的 InputSchema
    } else {
        decl.Parameters = &genai.Schema{     // 默认 "request" string 参数
            Type: "OBJECT",
            Properties: map[string]*genai.Schema{"request": {Type: "STRING"}},
            Required: []string{"request"},
        }
    }
    return decl
}
```

这样 LLM 就可以在 function call 中调用其他 agent。

**Run 方法**（:121-251）：

1. **参数校验**（:122-126）：断言 args 为 `map[string]any`。
2. **输入校验**（:146-149）：如果 agent 有 `InputSchema`，进行 JSON schema 校验。
3. **Sandbox 创建**（:168-177）：
   ```go
   sessionService := session.InMemoryService()
   r, err := runner.New(runner.Config{
       AppName:         t.agent.Name(),
       Agent:           t.agent,
       SessionService:  sessionService,
       ArtifactService: artifact.InMemoryService(),
       MemoryService:   memory.InMemoryService(),
   })
   ```
4. **状态注入**（:182-198）：从父 `ToolContext.State().All()` 中过滤内部 key（`_adk` 前缀），复制到子 session。
5. **执行并收集**（:201-216）：遍历 `r.Run()` 的 event 流，记录最后一个有 `Content` 的事件。
6. **输出校验**（:234-248）：如果 agent 有 `OutputSchema`，对最后一个 event 的文本输出进行 JSON Schema 校验。

### 4.5 Remote A2A Client (`agent/remoteagent/v2/a2a_agent.go`)

**`NewA2A`**（:156-193）：

```go
func NewA2A(cfg A2AConfig) (agent.Agent, error) {
    if cfg.ClientProvider == nil {
        cfg.ClientProvider = NewA2AClientProvider(a2aclient.NewFactory())
    }
    remoteAgent := &a2aAgent{
        serverConfig: &iremoteagent.A2AServerConfig{
            AgentCard: cfg.AgentCard, AgentCardProvider: cfg.AgentCardProvider,
            ClientProvider: cfg.ClientProvider,
        },
    }
    agent, err := agent.New(agent.Config{
        Name: cfg.Name, Description: cfg.Description,
        Run: func(ic agent.InvocationContext) iter.Seq2[*session.Event, error] {
            return remoteAgent.run(ic, cfg)
        },
    })
    ...
}
```

与传统 workflow agent 不同，remote agent 直接将 `run` 方法作为 agent 的 `Run` 函数，不依赖 `SubAgents()`。

**`a2aAgent.run`**（:199-303）：

执行流程：

1. **解析 AgentCard**（:201-205）：解析远程 agent 的 capability 描述。
2. **创建 A2A Client**（:207-212）：通过 `ClientProvider` 创建通信客户端。
3. **构造消息**（:214-218）：
   - 如果最后一个 session event 是 user function response（long-running tool），直接使用该 event 的 parts。
   - 否则，调用 `toMissingRemoteSessionParts` 进行增量同步：从后向前遍历 session events，找到最近的 remote agent 响应，将所有后续的 events 序列化为 A2A parts。非 user/remote_agent 的事件会被 `presentAsUserMessage` 转换为带 `[author] said:` 前缀的文本。
4. **BeforeRequestCallbacks**（:223-230）：支持缓存、请求修改等 hook。
5. **流式/非流式模式**（:292-302）：
   - 如果是 `StreamingModeNone`，调用 `SendMessage`（单次请求-响应）。
   - 否则，调用 `SendStreamingMessage` 并通过 `processEvent` 处理每个 event。
6. **cleanupRemoteTask**（:249-255, :306-344）：通过 defer 确保即使 Run 提前退出，也会清理远程 task。清理策略：
   - 如果 lastEvent 为 terminal 状态：跳过。
   - 如果状态为 `TaskStateInputRequired` 且无 cause（正常等待输入）：跳过。
   - 否则：调用 `CancelTask` RPC（默认 5s timeout）。

**`processEvent` 闭包**（:257-290）：

```go
processEvent := func(a2aEvent a2a.Event, a2aErr error) bool {
    if a2aEvent != nil { lastEvent = a2aEvent }
    var event *session.Event
    if cfg.Converter != nil {
        event, err = cfg.Converter(ctx, req, a2aEvent, a2aErr)
    } else {
        event, err = processor.convertToSessionEvent(ctx, a2aEvent, a2aErr)
    }
    // AfterRequestCallbacks
    if cbResp, cbErr := processor.runAfterA2ARequestCallbacks(ctx, event, err); ... { ... }
    // aggregatePartial: 聚合 partial events
    for _, toEmit := range processor.aggregatePartial(ctx, a2aEvent, event) {
        if !yield(toEmit, nil) { return false }
    }
    return true
}
```

### 4.6 A2A Server Executor (`server/adka2a/v2/executor.go`)

**`Executor.Execute`**（:161-240）：

```go
func (e *Executor) Execute(ctx context.Context, execCtx *a2asrv.ExecutorContext) iter.Seq2[a2a.Event, error] {
    return func(yield func(a2a.Event, error) bool) {
        // 1. 消息转换：A2A message → genai.Content
        content, err := toGenAIContent(ctx, msg, e.config.A2APartConverter)
        // 2. 创建 executorPlugin（注入 ExecutorContext 到 plugin registry）
        executorPlugin, err := newExecutorPlugin()
        // 3. 通过 RunnerProvider 创建 runner
        cfg, r, err := e.config.RunnerProvider(ctx, execCtx, executorPlugin.plugin)
        // 4. BeforeExecuteCallback
        // 5. HandleInputRequired：处理长运行工具的输入等待
        // 6. 如果无 StoredTask，emit TaskStateSubmitted
        // 7. 准备 session（get-or-create）
        // 8. emit TaskStateWorking
        // 9. 创建 artifactTransform（OutputArtifactPerRun vs OutputArtifactPerEvent）
        // 10. 创建 eventProcessor
        // 11. process() 事件循环
    }
}
```

**`process` 方法**（:342-371）：

```go
func (e *Executor) process(ctx ExecutorContext, r Runner, processor *eventProcessor, yield func(a2a.Event, error) bool) {
    for adkEvent, adkErr := range r.Run(ctx, meta.userID, meta.sessionID, ctx.UserContent(), e.config.RunConfig) {
        if adkErr != nil {
            event := processor.makeTaskFailedEvent(ctx, ...)
            e.writeFinalTaskStatus(ctx, yield, processor.makeFinalArtifactUpdate(), event, adkErr)
            return
        }
        a2aEvent, pErr := processor.process(ctx, adkEvent)
        if pErr == nil && a2aEvent != nil && e.config.AfterEventCallback != nil {
            pErr = e.config.AfterEventCallback(ctx, adkEvent, a2aEvent)
        }
        if pErr != nil {
            event := processor.makeTaskFailedEvent(ctx, ...)
            e.writeFinalTaskStatus(ctx, yield, processor.makeFinalArtifactUpdate(), event, pErr)
            return
        }
        if a2aEvent != nil {
            if !yield(a2aEvent, nil) { return }
        }
    }
    // runner 迭代器结束
    finalStatus := processor.makeFinalStatusUpdate()
    e.writeFinalTaskStatus(ctx, yield, processor.makeFinalArtifactUpdate(), finalStatus, nil)
}
```

**职责边界**：
- **Executor**：整体流程控制（session 管理、runner 创建、状态转换）。
- **eventProcessor**：单个 event 的转换逻辑（GenAI parts → A2A parts、failed/input-required 检测）。
- **eventToArtifactTransform**：控制 artifact 如何构建（`OutputArtifactPerRun` vs `OutputArtifactPerEvent`）。

### 4.7 Part Aggregation in Remote Client (`a2a_agent_run_processor.go`)

**`aggregatePartial`**（:62-117）：

这是客户端处理服务端 streaming response 的核心。A2A 服务端可能以 partial chunks 发送响应（`Append=true`, `LastChunk=false`）。客户端需要将这些 chunks 聚合为非 partial 的完整事件。

聚合策略：

| A2A Event 类型 | 聚合行为 |
|---------------|---------|
| `TaskStatusUpdateEvent` (terminal) | 将所有累积的 aggregation 作为非 partial event 发射，再发射 terminal event |
| `Task` (snapshot) | 重置所有 aggregation（服务端已提供完整数据） |
| `TaskArtifactUpdateEvent` (非 Append) | 移除该 artifact 的 aggregation 并重置 |
| `TaskArtifactUpdateEvent` (Append, 非 LastChunk) | 追加到 aggregation，继续累积 |
| `TaskArtifactUpdateEvent` (Append, LastChunk) | 追加到 aggregation，移除并发射完整的非 partial 事件 |

**`updateAggregation`**（:130-173）：

在聚合过程中：
- 合并连续的 text parts（同类型 `Thought` 标记的文本合并）。
- 合并 `CitationMetadata`、`GroundingMetadata`。
- 合并 `UsageMetadata`（后出现的覆盖前者，因为是 cumulative）。
- 合并 `CustomMetadata`。

## 5. orchestration_flows

### 5.1 Sequential Workflow

```
User Input
    │
    ▼
SequentialAgent.Run(ctx)
    │
    ├── subAgent[0].Run(ctx)
    │     ├── event_0_0 ──► yield ──► Runner
    │     ├── event_0_1 ──► yield ──► Runner
    │     └── (iterator exhausted)
    │
    ├── subAgent[1].Run(ctx)
    │     ├── event_1_0 ──► yield ──► Runner
    │     └── (iterator exhausted)
    │
    └── subAgent[N].Run(ctx)
          └── event_N_0 ──► yield ──► Runner
```

**语义**：严格有序，前一个 agent 完成后（事件流消耗完毕）才开始下一个。共享 session，状态自然传递。

### 5.2 Parallel Workflow

```
SequentialAgent.Run(ctx)
    │
    ├── errgroup.Go(goroutine 0)
    │     subAgent[0].Run(subCtx_0)
    │       ├── event_0_0 + ackChan_0 ──► resultsChan
    │       │   (阻塞等待 ackChan_0)
    │       │
    │       ├── (ackChan_0 关闭) ──► Runner 已处理
    │       ├── event_0_1 + ackChan_1 ──► resultsChan
    │       │   ...
    │       └── (iterator exhausted)
    │
    ├── errgroup.Go(goroutine 1)        ← 并发
    │     subAgent[1].Run(subCtx_1)
    │       ├── event_1_0 + ackChan_2 ──► resultsChan
    │       │   ...
    │       └── (iterator exhausted)
    │
    ├── errgroup.Go(goroutine N)
    │     ...
    │
    └── (main goroutine) ── resultsChan ──► yield ──► Runner
         for res := range resultsChan {
             yield(res.event, res.err)
             close(res.ackChan)   // 背压释放
         }
```

**语义**：所有子 agent 在独立 goroutine 中并发运行。事件通过 `resultsChan` 汇聚到主 goroutine，由主 goroutine 按到达顺序 yield 给 runner。`ackChan` 机制确保 runner 消费完一个事件后，对应的子 agent 才能产下一个事件。

### 5.3 Loop Workflow

```
LoopAgent.Run(ctx)
    │
    ├── iteration 1
    │     ├── subAgent[0].Run(ctx)
    │     │     ├── event ──► yield ──► Runner
    │     │     └── (Actions.Escalate? → shouldExit=true)
    │     ├── subAgent[1].Run(ctx)
    │     │     └── ...
    │     └── (shouldExit? → return)
    │
    ├── iteration 2 (if count > 0 and !shouldExit)
    │     └── ...
    │
    └── (count reaches 0 or shouldExit) → return
```

**语义**：子 agent 按顺序迭代执行。每次迭代后检查 `shouldExit`（由子 agent 的 `Actions.Escalate` 设置）或 `count` 计数器。支持无限循环（`MaxIterations=0`）。

### 5.4 Agent-as-Tool

```
LLM Agent.Run(ctx)
    │
    ├── LLMRequest ──► Model
    ├── LLMResponse (contains FunctionCall to "math_agent")
    │
    ├── ToolRunner 调度
    │     └── agentTool.Run(toolCtx, args)
    │           ├── 1. 校验 args 对 InputSchema
    │           ├── 2. 创建独立 InMemorySession
    │           ├── 3. 复制父 session 状态（过滤 _adk）
    │           ├── 4. 创建独立 Runner
    │           ├── 5. subRunner.Run(toolCtx, userID, sessionID, content, ...)
    │           │     ├── subAGent.Run(subCtx)
    │           │     │     └── event ──► 收集
    │           │     └── (iterator exhausted)
    │           ├── 6. 取 lastEvent.Content 的文本
    │           ├── 7. 如果 OutputSchema，校验
    │           └── 8. 返回 map[string]any{"result": "..."} 或解析后的 map
    │
    └── LLM 收到 FunctionResponse，继续推理
```

**语义**：agent 作为 tool 被同步调用（阻塞等待结果）。子 agent 拥有完全独立的运行上下文，与父 agent 通过输入/输出 schema 接口契约通信。

### 5.5 Remote A2A (Client → Server Executor → Runner)

```
┌─── Client Side ────────────────────────────────────────────────────┐
│                                                                    │
│  a2aAgent.run(ctx, cfg)                                            │
│    │                                                               │
│    ├── 1. ResolveAgentCard (URL/文件/静态)                         │
│    ├── 2. A2AClientProvider.CreateClient(card)                     │
│    ├── 3. newMessage(ctx, cfg)                                     │
│    │     ├── 如果有 user function response → 直接使用              │
│    │     └── 否则 toMissingRemoteSessionParts (增量同步)           │
│    ├── 4. 创建 runProcessor                                        │
│    ├── 5. runBeforeA2ARequestCallbacks                             │
│    ├── 6. sender.SendStreamingMessage(ctx, req)                    │
│    │     │                              │                          │
│    │     │   ──── HTTP/JSON-RPC ──────► │                          │
└────┼─────┼──────────────────────────────┼──────────────────────────┘
     │     │                              │
     │     │   ┌─── Server Side ──────────▼──────────────────────────┐
     │     │   │                                                      │
     │     │   │  a2asrv.RequestHandler                               │
     │     │   │    │                                                 │
     │     │   │    └── Executor.Execute(ctx, execCtx)                │
     │     │   │          │                                           │
     │     │   │          ├── 1. toGenAIContent(A2A msg → genai.Content)
     │     │   │          ├── 2. newExecutorPlugin()                 │
     │     │   │          ├── 3. RunnerProvider(ctx, execCtx, plugin) │
     │     │   │          ├── 4. HandleInputRequired                 │
     │     │   │          ├── 5. emit TaskStateSubmitted             │
     │     │   │          ├── 6. prepareSession (get-or-create)      │
     │     │   │          ├── 7. emit TaskStateWorking               │
     │     │   │          ├── 8. eventProcessor                      │
     │     │   │          └── 9. process()                           │
     │     │   │               │                                      │
     │     │   │               └── r.Run(ctx, userID, sessionID, ...) │
     │     │   │                     │                                │
     │     │   │                     ├── agent.Run(ctx)               │
     │     │   │                     │     ├── LLMRequest ──► Model   │
     │     │   │                     │     ├── FunctionCall ──► Tool  │
     │     │   │                     │     └── event ──► processor    │
     │     │   │                     │           │                    │
     │     │   │                     │           ├── updateTerminalActions
     │     │   │                     │           ├── inputRequiredProcessor
     │     │   │                     │           ├── convertParts     │
     │     │   │                     │           │   (GenAI → A2A)    │
     │     │   │                     │           ├── eventToArtifact. │
     │     │   │                     │           │   transform        │
     │     │   │                     │           └── yield a2aEvent   │
     │     │   │                     │                 │              │
     │     │   │  ◄── a2aEvent ──────┼─────────────────┘              │
     │     │   │  (TaskArtifactUpdateEvent / TaskStatusUpdateEvent)   │
     │     │   │                                                      │
     │     │   └──────────────────────────────────────────────────────┘
     │     │
     │     ├── for a2aEvent, a2aErr := range sender.SendStreamingMessage:
     │     │     ├── convertToSessionEvent(a2aEvent → session.Event)
     │     │     ├── runAfterA2ARequestCallbacks
     │     │     └── aggregatePartial (合并 partial chunks)
     │     │           └── yield(session.Event, nil)
     │     │
     │     └── defer: cleanupRemoteTask (如果非 terminal, CancelTask)
```

## 6. tests

### 6.1 Test Coverage Matrix

| 模块 | 测试文件 | 行数 | 覆盖要点 |
|------|---------|------|---------|
| `sequentialagent` | `agent_test.go` | 567 | 正常顺序执行、嵌套 sequential、名称冲突检测、重复 subagent 检测、`RunLive` 注入 `task_completed` tool、`RunLive` 会话路由切换 |
| `parallelagent` | `agent_test.go` | 537 | 正常并行、context 取消、agent 错误传播、工具使用竞态检测 (`FunctionCalls <= FunctionResponses`)、错误传播到 `errgroup` |
| `loopagent` | `agent_test.go` | 378 | 无限循环、max iterations、escalate 终止、escalate with SkipSummarization |
| `agenttool` | `agent_tool_test.go` | 367 | Declaration 生成、输入校验（extra_field/invalid_type/missing_required）、输出校验、正常运行（有/无 schema）、空模型响应、SkipSummarization 配置 |
| `remoteagent/v2` | `a2a_agent_test.go` | 1437 | 完整 HTTP mock server 测试、part 转换、Task/TaskArtifactUpdateEvent/TaskStatusUpdateEvent 转换、custom converter、card 解析、错误处理、message 构造 (`newMessage`) |
| `remoteagent/v2` | `utils_test.go` | 317 | `toMissingRemoteSessionParts`、`presentAsUserMessage`、`getUserFunctionCallAt` |
| `remoteagent/v2` | `a2a_e2e_test.go` | 1276 | 端到端测试：a2aclient → a2aserver → adka2a.Executor → llmagent。包括长运行工具（approval flow）、transfer to agent、sequential workflow via A2A |
| `remoteagent/v2` | `a2a_agent_run_processor_test.go` | - | Run processor 的聚合逻辑和 callback 链测试 |
| `remoteagent` (legacy) | `a2a_agent_compat_test.go` | 810 | v0↔v1 兼容性：旧 Executor 直连测试、旧 client + 新 server、新 client + 旧 server |
| `server/adka2a/v2` | `executor_test.go` | 1025 | Executor 各状态转换、错误传播、OutputMode、session 创建失败、BeforeExecuteCallback、AfterEventCallback、AfterExecuteCallback |
| `server/adka2a/v2` | `processor_test.go` | - | eventProcessor 的 part 转换、failed event 生成、input required 处理 |
| `server/adka2a/v2` | `events_test.go` | - | A2A event → session event 双向转换 |
| `server/adka2a/v2` | `parts_test.go` | - | GenAI part ↔ A2A part 转换 |
| `server/adka2a/v2` | `agent_card_test.go` | - | BuildAgentSkills |
| `server/adka2a` (legacy) | - | 通过 compat 测试间接覆盖 |

**测试模式特点**：
- 使用 `session.InMemoryService()` + `runner.New()` 进行集成级测试。
- Workflow agent 测试使用 mock agent（`customAgent`/`FakeLLM`）而非真实 LLM。
- A2A 测试使用 `httptest.Server` + mock `AgentExecutor`，无需真正的网络调用。
- E2E 测试使用 Gemini 录制/回放（`httprr`）机制。

### 6.2 关键测试场景

1. **`TestParallelAgentWithTools`**（`parallelagent/agent_test.go:239-291`）：
   验证并行 agent 中两个子 agent 都使用 function tool 时，不会出现竞态条件（`FunctionCalls > FunctionResponses` 即表示 agent 在 FunctionResponse 写入 session 之前就读取了 session）。

2. **`TestParallelAgent_PropagatesContextError`**（`parallelagent/agent_test.go:372-443`）：
   验证 context 取消后，错误正确传播到 runner。

3. **`TestParallelAgent_StateSync`**（`parallelagent/agent_test.go:464-537`）：
   验证子 agent 通过 `StateDelta` 设置的状态能被 `AfterAgentCallback` 访问。

## 7. risks

### 7.1 状态同步

- **Parallel agent session 共享**：当前实现中，多个子 agent goroutine 共享同一个 `session.Service`。虽然 `branch` 机制提供了命名空间隔离，但 `session.AppendEvent` 的并发写入需要内部加锁保护。测试 `TestParallelAgentWithTools` 已覆盖此类问题，但复杂场景（如多个 agent 同时写入 `StateDelta`）仍可能产生非预期行为。
- **Sequential agent 状态可见性**：子 agent 通过 session 隐式共享状态，但缺少显式的状态传递契约。如果后置 agent 依赖前置 agent 的某个 state key，而该 key 因 `_adk` 过滤被清除，会导致静默失败。

### 7.2 错误一致性

- **Sequential agent** 的 `TODO` 注释（`:82`）明确指出错误处理的不一致性：`// TODO: ensure consistency -- if there's an error, return and close iterator, verify everywhere in ADK.`
- **Parallel agent**：如果某个子 agent 返回错误，`errgroup` 会取消其他 goroutine 的 context，但已发送到 `resultsChan` 的事件可能已经被 runner 处理并写入 session。这导致 session 中存在部分执行的结果。
- **Loop agent**：escalate 机制依赖子 agent 正确设置 `Actions.Escalate`，但没有任何强制机制确保设置了 escalate 的 agent 也返回了有意义的结果。

### 7.3 RunLive 支持

- **Sequential agent** 实现了 `RunLive`，通过注入 `task_completed` tool 和 `sequentialLiveSession` 动态路由实现双向通信。
- **Parallel agent** 和 **Loop agent** 均未实现 `RunLive`。如果子 agent 需要双向实时通信，这两个编排器无法满足。
- `RunLive` 的 `sequentialLiveSession` 仅在 agent 切换瞬间存在短暂的竞态窗口（`:97-123`），但在高频切换场景下可能成为瓶颈。

### 7.4 Legacy A2A 版本

- `agent/remoteagent/a2a_agent.go` 和 `server/adka2a/executor.go` 均已标记 `Deprecated`。
- Legacy 实现通过 `a2av0` 兼容层将所有调用委托给 v2 实现。性能开销来自每个事件的 `ToV1Event`/`FromV1Event` 转换，在高频 streaming 场景下会增加内存和 CPU 负担。
- `compatClient`（`a2a_agent.go:292-341`）封装了 v0 client，将 v1 `SendMessageRequest` 转为 v0 格式、调用 v0 client、再将结果转回 v1。这增加了调用链路深度。

### 7.5 协议差异

- **自定义 Part 转换器**：`A2APartConverter` / `GenAIPartConverter` 提供了灵活性，但如果实现不当（例如返回 nil 表示丢弃 part），可能导致信息丢失且难以调试。
- **OutputMode 差异**：`OutputArtifactPerRun` 和 `OutputArtifactPerEvent` 在客户端侧的解析行为不同。如果服务端和客户端对 partial 标志的理解不一致，可能导致 artifact 聚合错误。
- **Metadata key 前缀**：`ToA2AMetaKey` / `ToADKMetaKey` 使用不同的前缀来区分 ADK 和 A2A 的元数据，但两者的 key 映射表可能不同步。

### 7.6 AgentTool 沙箱限制

- **独立 session 的局限**：子 agent 的状态变更不会反馈给父 agent。这在当前设计中是故意的隔离，但在某些编排场景下可能需要状态回传。
- **没有 artifact 转发**：代码中注释 `// TODO - use forwarding_artifact_service as in python`（`agent_tool.go:174`），表明子 agent 产生的 artifact 无法自动转发给父 agent。
- **同步阻塞**：AgentTool 的 `Run` 是同步方法，意味着父 LLM agent 必须等待子 agent 完全执行完毕才能继续。不支持异步或流式委派。

### 7.7 Streaming 模式

- 当 `StreamingModeNone` 时，remote agent 调用 `SendMessage`（单次请求-响应），结果作为单个 event 返回。这比分块 streaming 有更高的首字节延迟。
- A2A 的 `TaskArtifactUpdateEvent` 的 `Append`/`LastChunk` 语义与 ADK 的 `Partial` 标志不完全对等（见 `events.go:111-119` 的注释），可能导致客户端接收到意外的 partial 事件。

## 8. next_questions

1. **Parallel agent 的 session 并发安全**：`session.InMemoryService()` 内部如何保证 `AppendEvent` 的线程安全？如果替换为外部 session service（如数据库），并发写入是否会引入新的一致性问题？

2. **Parallel agent 的背压策略**：当前实现中 `ackChan` 是 per-event 的，意味着每个事件都需要一个 round trip。是否可以在批量确认（batch ack）和延迟之间做权衡？

3. **Loop agent 的终止语义**：`event.Actions.Escalate` 和 `event.Actions.SkipSummarization` 的交互规则是什么？escalate 后 runner 如何将结果汇总给用户？

4. **Sequential agent 的 RunLive 中 task_completed tool**：注入 `task_completed` 后，LLM 是否可能在未完成任务时提前调用？是否有 fallback 机制（如 timeout 或 max steps）？

5. **AgentTool 中 `output validation` 的具体实现**：`utils.ValidateOutputSchema` 使用的是哪种 JSON Schema 方言？是否支持 `$ref`、`oneOf`、`anyOf` 等高级特性？

6. **Remote A2A 的增量同步机制**：`toMissingRemoteSessionParts` 从后向前遍历 session events，找到最近的 remote agent event 后停止。如果 session 中存在多个 remote agent 的交错调用，算法是否正确？

7. **A2A Task 的生命周期管理**：`cleanupRemoteTask` 在什么确切条件下会触发 `CancelTask` RPC？如果客户端崩溃，服务端的 task 是否有超时清理机制？

8. **OutputArtifactPerEvent vs OutputArtifactPerRun**：两种模式下，客户端如何处理多 artifact 场景？`artifactAggregation` 在 `OutputArtifactPerRun` 模式下是否有重复 emit 的风险？

9. **Protocol version negotiation**：当 client 和 server 的 A2A 协议版本不匹配时，connection 建立阶段的 fallback 流程是怎样的？`a2aclient.NewFactory()` 是否自动协商？

10. **Error event 的 metadata 一致性问题**：在 `runProcessor.convertToSessionEvent`（`a2a_agent_run_processor.go:187-205`）中，转换失败时创建 error event 并设置 custom metadata。这个 error event 是否仍然需要走 `aggregatePartial`？如果 error 发生在 streaming 中途，已聚合的 partial data 如何处置？

11. **AgentTool 的 `IsLongRunning` 始终返回 false**：这意味着 agent-as-tool 在 A2A 场景下不会触发 `input-required` 状态。如果被包装的 agent 内部使用了长运行工具，会发生什么？

12. **A2A executor plugin 的作用范围**：`newExecutorPlugin` 创建的 plugin 用于将 `ExecutorContext` 注入到 agent callback 中。这个 plugin 的生命周期是什么？它是否会影响同一 runner 实例上的其他并发执行？
