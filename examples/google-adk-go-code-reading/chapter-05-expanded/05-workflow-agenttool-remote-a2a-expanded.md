# Chapter 05 - Workflow / AgentTool / Remote A2A 深度讲解

> 本章对应教学大纲 Chapter 05：Workflow / AgentTool / Remote A2A。
> 代码基线是本机 `rive-adk-go` 复刻版。原版 ADK 的概念会在必要处对照，但讲解以当前代码可验证行为为准。

---

## 0. 本章一句话

Chapter 05 讲的是 agent workflow 从"一个 agent 自己做事"升级到"多个 agent 组合做事"：

> Workflow Agent 把多个本地 agent 组合成顺序、并发、循环流程；AgentTool 把一个完整 agent 包装成父 agent 可调用的 tool；RemoteAgent 把另一个进程或服务里的 agent 流转换成本地 event 流。

如果前几章讲的是单个 LLM agent 的内部循环：

```text
Runner -> Agent -> Flow -> Model -> Tool -> Event -> Session
```

那么本章讲的是三种更大的组合方式：

```text
Workflow Agent:
  Agent -> child Agent -> child Agent -> child Agent

AgentTool:
  parent Model -> FunctionCall(child_agent_tool) -> child Agent -> FunctionResponse

Remote A2A:
  local Agent -> remote A2A stream -> Converter -> Aggregator -> local Event
```

它们的共同目标是：

> 让复杂业务不必塞进一个超大 agent，而是拆成多个可独立理解、独立测试、独立复用的 agent 单元。

但它们的 session 语义完全不同：

| 组合方式 | Session 语义 | 适合场景 |
| --- | --- | --- |
| Workflow Agent | 子 agent 共享同一个本地 session | 本地流水线、并行审查、循环修复 |
| AgentTool | 子 agent 用隔离 child session，父状态单向拷贝 | 父 agent 临时委派专家 agent |
| Remote A2A | 远程 session 对本地透明，本地只接收转换后的 events | 跨进程/跨服务调用远程 agent |

这个差异是本章最重要的教学点。

---

## 1. 为什么需要多 agent composition

先看一个代码审查业务。

用户说：

```text
请生成一个 Go HTTP handler，然后做安全审查、性能审查，最后修复问题。
```

如果只用一个 agent，可以写一个很长的 prompt：

```text
你先写代码，再从安全角度审查，再从性能角度审查，再修复，再总结。
```

这能跑，但有三个问题：

1. 每一步的职责混在一个模型上下文里，难测试。
2. 安全、性能、风格三个视角没有真正并发。
3. 想复用"安全审查 agent"到别的 workflow 很困难。

更自然的拆法是：

```text
coder agent
  -> reviewer agent
  -> fixer agent
```

或者：

```text
coder agent
  -> parallel(
       security reviewer,
       performance reviewer,
       style reviewer
     )
  -> fixer loop
```

这就是 Workflow Agent 的价值。

再看另一个场景：

```text
主 agent 是客服，但用户突然问一个复杂数学问题。
```

你不想把数学专家的完整能力塞进客服 agent，也不想让客服 agent 直接切换成另一个 agent。你希望模型像调用工具一样调用：

```json
{
  "name": "math_agent",
  "arguments": {
    "request": "what is 6*7"
  }
}
```

这就是 AgentTool 的价值：把完整 agent 包成 tool。

再看远程场景：

```text
本地 agent 需要调用另一个进程上的知识库 agent。
远程 agent 会通过 A2A protocol streaming 返回多个 chunk。
```

本地 runtime 不应该关心远程内部 session 怎么存，也不应该直接把 remote chunk 当成本地最终 event。它需要：

```text
remote stream event
  -> convert to local Event
  -> aggregate partial chunks
  -> produce local Event
```

这就是 RemoteAgent 的价值。

---

## 2. 初学者桥：三种"多 agent"不是一回事

### 2.1 Workflow Agent：agent-as-composition

Workflow Agent 自己也是 agent。

它实现的是：

```text
agent.Agent
runner.ExecutableAgent
```

所以 Runner 看到它时，不需要特殊分支：

```text
Runner.Run
  -> Execute(workflow agent)
  -> workflow agent calls child.Execute(...)
  -> returns []*event.Event
  -> Runner persists events
```

这叫 agent-as-composition：

> Sequential / Parallel / Loop 不是外部调度器，它们本身就是 agent，只是内部组合了子 agent 的 `Execute()` 事件流。

### 2.2 AgentTool：agent-as-tool

AgentTool 不是 workflow agent。

它把一个 agent 包装成 `tool.FunctionTool` / `tool.ContextFunctionTool`：

```text
parent model outputs FunctionCall("math_agent")
  -> Flow.executeToolCall
  -> agenttool.RunWithContext
  -> create child runner/session
  -> run child agent synchronously
  -> return {"result": lastText}
  -> parent model sees FunctionResponse
```

父 agent 视角里，子 agent 就像一个 tool。

但实现上，它确实跑了一个完整 agent。

### 2.3 RemoteAgent：remote stream as local agent

RemoteAgent 实现 `agent.Agent`，也能作为 workflow 子 agent 使用。

但它内部不是调用本地 child agent，而是：

```text
Create A2A client
Build SendMessageRequest
SendStreamingMessage
for each remote event:
  Converter(remote) -> local events
  Aggregator(remote, local events) -> emitted events
Destroy client
```

本地只看到转换后的 `[]*event.Event`。

远程 session、远程状态、远程 tool loop 都在远程服务里，本地不直接管理。

### 2.4 三个 session 语义先记住

这张表必须先讲，否则后面细节很容易混：

| 问题 | Workflow Agent | AgentTool | RemoteAgent |
| --- | --- | --- | --- |
| 子任务是否共享父 session | 是 | 否 | 否，本地不可见 |
| 子任务状态是否回写父 session | 是，因为本来就是同一个 session | 否 | 只通过转换后的 event/action 体现 |
| 子任务能否并发 | ParallelAgent 可以 | 单个 tool call 同步阻塞 | 远程 streaming，但本地 Execute 收集返回 |
| 输出是什么 | 子 agent events 直接拼接 | tool result map | converted local events |
| 适合什么 | 明确编排好的本地流程 | 父 LLM 临时委派专家 | 跨服务 agent 调用 |

---

## 3. 本章代码地图

按教学大纲，本章核心目录是：

```text
workflow/
tool/agenttool/
agent/remoteagent/
```

重点文件：

| 文件 | 重点 | 为什么读 |
| --- | --- | --- |
| `workflow/workflow.go` | `SequentialAgent`、`ParallelAgent`、`LoopAgent`、`subCtx` | 本地 agent 组合的核心 |
| `tool/agenttool/agent_tool.go` | `agentTool`、`Declaration`、`RunWithContext`、child session | agent 如何变成 tool |
| `agent/remoteagent/remote_agent.go` | `RemoteAgent.Execute`、client lifecycle、cleanup | 远程 A2A 流如何变成本地执行 |
| `agent/remoteagent/convert.go` | `DefaultConvertToSessionEvent`、`ConvertSessionEventToRemote` | remote event 与 local event 双向转换 |
| `agent/remoteagent/aggregate.go` | `aggregator.process`、`flush`、`terminalFlush` | partial chunk 如何聚合成完整 event |
| `agent/remoteagent/fake_client.go` | `FakeA2AClient` | 测试和 demo 用的无网络 A2A client |
| `workflow/workflow_test.go` | unit tests | 顺序、并发、循环、状态共享、错误传播 |
| `workflow/workflow_e2e_test.go` | runner e2e tests | Runner -> Workflow -> Session 的完整链路 |
| `tool/agenttool/agent_tool_test.go` | sandbox tests | AgentTool session isolation、state copy、skip summarization |
| `agent/remoteagent/remoteagent_test.go` | converter/aggregator/remote tests | A2A conversion、partial aggregation、cleanup |
| `cmd/demo/main.go` | `runChapter05` demos | 课堂演示 |

大图：

```text
Workflow:
  Runner.Run
    -> Sequential/Parallel/Loop.Execute
       -> child.Execute(subCtx)
       -> collect child events
    -> session.AppendEvent(child events)

AgentTool:
  parent Flow.handleFunctionCalls
    -> executeToolCall
       -> agentTool.RunWithContext
          -> create child runner/session
          -> copy non-_adk parent state
          -> child runner.Run
          -> return last child text as tool result

RemoteAgent:
  Runner.Run
    -> RemoteAgent.Execute
       -> client.SendStreamingMessage
       -> DefaultConvertToSessionEvent
       -> aggregator.process
       -> local events
```

---

## 4. Workflow Agent：把 agent 组合成流程

### 4.1 SubAgent 接口

`workflow.SubAgent` 定义很小：

```go
type SubAgent interface {
    agent.Agent
    Execute(ctx agent.InvocationContext) ([]*event.Event, error)
}
```

这说明 workflow 不关心子 agent 是 LLM agent、base agent、remote agent，还是另一个 workflow agent。

只要它能：

```text
Name()
Description()
FindAgent()
Execute(ctx)
```

就能被组合。

### 4.2 subCtx：让子 agent 看到自己

workflow 执行子 agent 时不会直接把原始 invocation context 传进去，而是包一层：

```go
sc := &subCtx{InvocationContext: ic, a: sub}
events, err := sub.Execute(sc)
```

`subCtx.Agent()` 返回的是当前子 agent：

```go
func (c *subCtx) Agent() agent.Agent { return c.a }
```

这让子 agent 内部看到：

```text
ctx.Agent() == child agent
```

而不是 workflow parent。

### 4.3 subCtx 的 EndInvocation 是隔离的

`subCtx` 自己维护 `ended` flag：

```go
func (c *subCtx) EndInvocation() {
    c.ended = true
}
```

它不会立刻直接结束父 context。

SequentialAgent 会在子 agent 结束后检查：

```text
if sc.Ended():
  ctx.EndInvocation()
  break
```

这让 workflow 能控制：

- 子 agent 可以表达"我想结束"。
- workflow 决定如何把这个信号传播给父 invocation。

### 4.4 Workflow 共享同一个 session

`subCtx` 嵌入的是同一个 `context.InvocationContext`。

这意味着：

```text
writer child writes session state
reader child reads same session state
```

`TestSequentialAgentStateSharing` 和 `TestWorkflowE2E_Sequential_StateSharing_ThroughRunner` 验证了这一点。

这正是 workflow 和 AgentTool 的根本区别之一。

---

## 5. SequentialAgent：声明顺序就是执行顺序

### 5.1 核心执行逻辑

`SequentialAgent.Execute` 的逻辑可以简化成：

```text
allEvents = []
for sub in subAgents:
  sc = subCtx(parentContext, sub)
  events, err = sub.Execute(sc)
  if err:
    return allEvents, wrapped error
  allEvents += events
  if sc.Ended():
    parent.EndInvocation()
    break
return allEvents
```

这就是顺序流水线。

### 5.2 适合什么业务

SequentialAgent 适合有明确依赖关系的任务：

```text
需求理解 -> 代码生成 -> 代码审查 -> 修复建议
```

或者：

```text
检索 -> 证据整理 -> 答案生成 -> 风险检查
```

前一个 agent 的输出、事件、状态都可以给后一个 agent 使用。

### 5.3 错误会停止后续 agent

`TestSequentialAgentErrorStopsChain` 验证：

```text
ok agent runs
failing agent returns error
later agent does not run
events from ok agent are still returned
```

这很像一个 build pipeline：

```text
compile ok
test failed
deploy skipped
```

### 5.4 EndInvocation 会停止链

`TestSequentialAgentEndInvocationStopsChain` 验证：

```text
first child calls ctx.EndInvocation()
workflow stops
second child does not run
```

注意是 `subCtx` 先记录 ended，然后 SequentialAgent 再把它传播到父 context。

---

## 6. ParallelAgent：并发执行，按声明顺序输出

### 6.1 核心执行逻辑

`ParallelAgent.Execute` 对每个子 agent 开 goroutine：

```text
for i, sub in subAgents:
  go:
    branch = parent.child
    events, err = sub.Execute(subCtx)
    tag each event.Branch
    send pResult{index, events, err}
```

然后收集结果：

```text
ordered[index] = result
for result in ordered:
  append events
  record first error
```

所以它有两个特点：

1. 执行是并发的。
2. 输出是按声明顺序稳定排列的。

`TestParallelAgentBranchAndEventAggregation` 和 `TestWorkflowE2E_Parallel_WithBranchLabels` 都验证了这个行为。

### 6.2 Branch label：区分并发来源

ParallelAgent 会给每个 event 注入 branch：

```text
parent.child
```

例如：

```text
review-team.analyst
review-team.critic
review-team.planner
```

如果 child event 已经有自己的 branch，会扩展：

```text
outer.inner.step1
```

这不是状态隔离，只是事件分组/来源标记。

教学时要明确：

> Branch label 用来区分事件来源，不等于 session state 隔离。

### 6.3 ParallelAgent 共享 session，状态写入可能非确定

`workflow/workflow.go` 顶部注释写得很直接：

```text
Sub-agents share the same underlying session.
For parallel workflows, concurrent writes to session state are protected by mutex,
but merge order is non-deterministic.
```

所以 ParallelAgent 适合并发读、并发生成独立 events。

如果多个 parallel child 同时写同一个 state key：

```text
child A: status = "a"
child B: status = "b"
```

最终值取决于谁最后 merge，不应该依赖。

### 6.4 错误传播：所有子 agent 都跑完，返回第一个错误

`TestParallelAgentErrorPropagation` 验证：

```text
ok-1 runs
err runs and returns error
ok-2 still runs
returned events include successful agents
returned error is first error
```

这和 SequentialAgent 不一样。

Sequential 是遇错停止。

Parallel 是并发任务都结束后再汇总第一个错误。

---

## 7. LoopAgent：合作式循环

### 7.1 核心执行逻辑

`LoopAgent.Execute` 是：

```text
count = maxIterations
for:
  for sub in subAgents:
    events, err = sub.Execute(subCtx)
    if err:
      return allEvents, error
    allEvents += events
    if any event.Actions.Escalate:
      return allEvents, nil

  if maxIterations > 0:
    count--
    if count == 0:
      return allEvents, nil
```

两个终止条件：

1. 达到 `maxIterations`。
2. 某个 event 的 `Actions.Escalate == true`。

还有一个失败条件：

```text
sub-agent returns error
```

### 7.2 maxIterations=0 表示无限循环

`NewLoopAgent(..., maxIterations=0)` 的语义是：

```text
run indefinitely until Escalate or error
```

`TestLoopAgentZeroMaxIterations` 用一个第五次设置 `Escalate` 的子 agent 防止测试无限跑。

教学时要强调：

> 如果 maxIterations=0，而且子 agent 永远不设置 Escalate，也不返回 error，这个 loop 不会自动停。

生产代码里应该给外层 context timeout/cancel。

### 7.3 Escalate 是合作式终止，不是强杀

LoopAgent 不会中断正在运行的子 agent。

它是在子 agent 返回 events 后检查：

```text
for _, ev := range events:
  if ev.Actions.Escalate:
    stop
```

所以 Escalate 是合作式信号：

- 子 agent 自己决定何时发。
- workflow 在一个子 agent 执行完后观察到它。
- 不是抢占式 cancellation。

`TestLoopAgentEarlyStopOnEscalate` 和 `TestWorkflowE2E_Loop_EarlyStop_ThroughRunner` 验证了这个行为。

---

## 8. Workflow Agent 的边界和简化

### 8.1 当前复刻版没有 per-event backpressure

原版 ADK Go 的并发 workflow 更强调流式事件和背压，比如每个 event yield 后等待 runner ack。

当前复刻版简化为：

```text
child.Execute returns []*event.Event
workflow collects slice
workflow returns combined []*event.Event
```

这意味着：

- 子 agent 不是边产 event 边被外层消费。
- ParallelAgent 不是 per-event 交错输出。
- 没有 `ackChan` 这种背压机制。

教学时可以说：

> 当前 replica 是 slice-collection 模型，足够讲 composition 语义，但不是完整原版 streaming/backpressure 模型。

### 8.2 Workflow 不是 task scheduler

Sequential / Parallel / Loop 没有外部队列，也没有分布式调度。

它们就是普通 agent：

```text
Runner calls Execute()
Execute() calls child Execute()
```

所以不要把它理解成 Kubernetes job、Celery task、Airflow DAG。

它是 agent runtime 内的组合模式。

---

## 9. AgentTool：把完整 agent 包成 tool

### 9.1 Declaration：模型看到的是一个 tool

`agentTool.Declaration` 生成：

```go
tool.Declaration{
    Name: t.agent.Name(),
    Description: t.agent.Description(),
    InputSchema: map[string]any{
        "type": "object",
        "properties": map[string]any{
            "request": map[string]any{"type": "string"},
        },
        "required": []any{"request"},
    },
}
```

模型看到的是：

```text
tool name = math_agent
argument = request string
```

模型不知道这个 tool 背后其实是一个完整 agent。

### 9.2 RunWithContext：同步执行 child agent

AgentTool 的核心路径：

```text
RunWithContext(tc, args)
  -> if SkipSummarization, mark tc.Actions.SkipSummarization
  -> runWithContext(tc, args)
```

`runWithContext` 做：

```text
1. 读取 args["request"]
2. 检查 wrapped agent 是否实现 runner.ExecutableAgent
3. 创建新的 InMemorySessionService / ArtifactService / MemoryService
4. 创建 child runner
5. 创建 child session
6. 从父 session 拷贝非 _adk state 到 child session
7. child runner.Run(inputText)
8. 遍历 child events，取最后一个文本
9. 返回 {"result": lastText}
```

这是同步阻塞的。

父 Flow 必须等 child agent 完整执行完，才能拿到 tool result。

### 9.3 child session 是隔离的

AgentTool 不复用父 session。

它每次创建：

```go
sessionService := runner.NewInMemorySessionService()
artifactService := artifact.InMemoryService()
memoryService := memory.InMemoryService()
```

然后创建 child session：

```go
childSessionID := fmt.Sprintf("%s-agenttool-%d", t.agent.Name(), time.Now().UnixNano())
sess, err := sessionService.Create(...)
```

这意味着：

```text
child writes state
  -> parent session does not see it
```

`TestAgentTool_Run_SessionIsolation` 验证了这个行为。

### 9.4 父 state 是单向拷贝

AgentTool 会把父 session 的 state 拷贝到 child session：

```go
for k, v := range parentSession.State().All() {
    if !strings.HasPrefix(k, internalStatePrefix) {
        sess.State().Set(k, v)
    }
}
```

其中：

```go
const internalStatePrefix = "_adk"
```

所以：

```text
parent shared_key -> child can read
parent _adk_internal_key -> child cannot read
child child_key -> parent cannot read
```

测试：

- `TestAgentTool_Run_ParentStateCopied`
- `TestAgentTool_Run_InternalStateNotCopied`
- `TestAgentTool_Run_SessionIsolation`

这就是"单行道"：

```text
parent state -> child initial state
child state -X-> parent state
```

### 9.5 AgentTool 返回最后一个文本

AgentTool 会遍历 child events，找最后一个有文本内容的 event：

```text
lastText = last non-empty text joined by "\n"
return {"result": lastText}
```

如果没有文本：

```text
return map[string]any{}
```

如果 child event 带 error：

```text
return error
```

这意味着 AgentTool 不是把 child events 原样交给父 agent。

父 agent 只看到一个 tool result。

`TestAgentTool_Run_ChildOutput` 和 `TestAgentTool_Run_EmptyOutput` 验证了这两个分支。

### 9.6 SkipSummarization

AgentTool 支持配置：

```go
agenttool.Config{SkipSummarization: true}
```

如果通过 `RunWithContext` 执行，它会设置：

```go
tc.Actions().SkipSummarization = true
```

`TestAgentTool_Run_SkipSummarization` 和 `TestWorkflowE2E_AgentTool_SkipSummarization_ThroughRunner` 验证了这个行为。

教学时可以把它理解成：

> 子 agent 的结果已经足够结构化或足够最终，不一定需要父 agent 再总结一次。

### 9.7 AgentTool 的典型链路

`TestWorkflowE2E_AgentTool_Delegation_ThroughRunner` 展示完整链路：

```text
parent user asks: What is 6*7?
parent model outputs FunctionCall:
  name = "math_agent"
  args = {"request": "what is 6*7"}
Flow executes agenttool
child math_agent returns "42"
Flow creates FunctionResponse:
  name = "math_agent"
  result = {"result": "42"}
parent model sees tool result
parent model returns final answer
```

这和普通 tool call 的外观一致，但 tool 内部是 child agent。

---

## 10. RemoteAgent：把远程 A2A 流接回本地

### 10.1 RemoteAgent 的定位

`RemoteAgent` 是一个本地代理对象：

```text
local RemoteAgent object
  -> talks to remote service through A2AClient
  -> emits local event.Event
```

它实现 `agent.Agent`，也能作为 workflow sub-agent 使用。

但它的真实执行发生在远程服务。

### 10.2 Execute 主流程

`RemoteAgent.Execute` 的核心步骤：

```text
1. clientProvider(agentCard) -> client
2. requestPartsFromContext(ctx) -> request parts
3. client.SendStreamingMessage(req) -> stream
4. choose converter
5. for each StreamEvent:
     if error: stop
     converted = converter(remoteEvent)
     toEmit = aggregator.process(remoteEvent, converted)
     allEvents += toEmit
6. cleanup callbacks
7. client.Destroy()
8. maybe flush aggregator
9. return allEvents
```

这就是远程流桥接成本地事件流。

### 10.3 requestPartsFromContext：本地用户输入变远程 request

`requestPartsFromContext` 从 invocation context 里取：

```text
UserContent()
InvocationID()
```

构造一个本地 user event：

```go
event.NewEvent(invocationID+"-user-request", "user", event.RoleUser)
```

再用：

```go
ConvertSessionEventToRemote(ev)
```

变成远程 request parts。

如果 context 不是本机 `context.InvocationContext`，或者没有 user content，就返回 nil。

### 10.4 DefaultConvertToSessionEvent：remote event 变 local event

`DefaultConvertToSessionEvent` 处理三类 remote event：

| RemoteEvent type | 本地转换 |
| --- | --- |
| `TaskStatusUpdate` | 生成 model event，把 task state/id 写入 `Actions.StateDelta` |
| `TaskArtifactUpdate` | 生成 content event，parts 1:1 转换 |
| `Message` | 生成 content event，parts 1:1 转换 |

status update 会写：

```text
Actions.StateDelta["remote_task_state"] = state
Actions.StateDelta["remote_task_id"] = taskID
```

终态：

```text
completed / failed / cancelled
```

会生成非 partial event。

非终态会生成 partial event。

### 10.5 FunctionCall / FunctionResponse 也能转换

remote part 里可以带：

```text
RemoteFunctionCall
RemoteFunctionResponse
```

converter 会转成：

```text
event.FunctionCall
event.FunctionResponse
```

测试：

- `TestConvertToSessionEvent_MessageWithFunctionCall`
- `TestConvertSessionEventToRemote_FunctionCall`

这说明 RemoteAgent 不只是文本桥接，也保留 tool-call 结构。

---

## 11. Aggregator：partial chunks 变完整 event

### 11.1 为什么 converter 之后还需要 aggregator

converter 只是"格式转换"。

但 remote streaming 经常是：

```text
Append chunk: "Hello "
Append chunk: "World"
Append LastChunk: "!"
TaskStatusUpdate completed
```

本地最终不应该保存三条 partial text events，而应该得到：

```text
"Hello World!"
```

这就是 aggregator 的职责。

### 11.2 process 规则

`aggregator.process(remote, converted)` 的规则：

| remote type | 条件 | 行为 |
| --- | --- | --- |
| `TaskStatusUpdate` | terminal | flush pending，再 append status event |
| `TaskStatusUpdate` | non-terminal | 直接 emit converted |
| `TaskArtifactUpdate` | `!Append` | flush old pending，reset，emit converted |
| `TaskArtifactUpdate` | `Append && !LastChunk` | accumulate，emit nothing |
| `TaskArtifactUpdate` | `Append && LastChunk` | accumulate，flush |
| `Message` | `Append && !LastChunk` | accumulate，emit nothing |
| `Message` | `Append && LastChunk` | accumulate，flush |
| `Message` | `!Append` | flush old pending，reset，emit converted |

### 11.3 Append chunks suppress emission until flush

`TestAggregator_AppendChunksThenLastChunk` 验证：

```text
chunk 1: Append, !LastChunk, "Hello " -> no emitted event
chunk 2: Append, !LastChunk, "World" -> no emitted event
chunk 3: Append, LastChunk, "!" -> one emitted aggregated event
```

聚合后的 event：

```text
Partial = false
TurnComplete = true
```

### 11.4 Terminal status 会 flush pending

`TestAggregator_TerminalFlush` 验证：

```text
Append partial text
TaskStatusUpdate completed
  -> first emit aggregated text
  -> then emit terminal status event
```

这解决了远程服务最后没有发 LastChunk，但发了 completed status 的情况。

### 11.5 Non-append message 会重置 pending

`TestAggregator_NonAppendResets` 验证：

```text
pending old partial
non-append new standalone message
  -> flush old
  -> emit new standalone
  -> reset pending
```

这避免旧 partial 泄漏到新的独立消息里。

---

## 12. 三种组合方式的事件模型

### 12.1 Workflow Agent：子事件直接流经父结果

Sequential / Parallel / Loop 返回的是子 agent events 的拼接。

例如 Sequential：

```text
coder event
reviewer event
fixer event
```

这些 events 会由 Runner 作为当前 run 的 agent events 保存进 session。

### 12.2 AgentTool：子事件内部消化，只暴露 tool result

AgentTool 的 child events 不直接进入父 session。

父 session 里看到的是：

```text
model event: FunctionCall(math_agent)
tool event: FunctionResponse(math_agent, {"result": "42"})
model event: final answer
```

child agent 的内部事件只用于提取最后文本。

这就是为什么 AgentTool 适合"委派专家并拿摘要结果"，不适合"把专家执行全过程展开给父 session"。

### 12.3 RemoteAgent：remote event 转成本地 session event

RemoteAgent 返回的是转换/聚合后的本地 events。

例如：

```text
Remote Append "The capital "
Remote Append "of France "
Remote LastChunk "is Paris."
Remote completed
```

本地可能得到：

```text
aggregated model event: "The capital of France is Paris."
status event: remote_task_state=completed
```

`TestRemoteAgent_StreamingAggregation_ThroughRunner` 验证了 Runner 里的完整链路。

---

## 13. 课堂演示

`cmd/demo/main.go` 的 `runChapter05` 有五个 demo。

### 13.1 demoSequentialWorkflow：生成 -> 审查

演示：

```text
coder
  -> reviewer
```

重点讲：

- 子 agent 按声明顺序执行。
- 每个子 agent 产生自己的 event。
- session 中保存 user event + child agent events。

### 13.2 demoParallelWorkflow：多视角并发

演示：

```text
analyst
critic
evaluator
```

重点讲：

- goroutine 并发执行。
- 输出按声明顺序排列。
- event.Branch 标出来源。

### 13.3 demoLoopWorkflow：修复循环

演示：

```text
fixer round 1
fixer round 2
fixer round 3 -> Escalate
stop
```

重点讲：

- `Actions.Escalate` 是 cooperative stop。
- `maxIterations` 是上限。
- `maxIterations=0` 需要额外 cancellation 保护。

### 13.4 demoAgentToolDelegation：父 agent 调专家 agent

演示：

```text
parent model calls math_agent tool
math_agent child runner returns 42
parent model receives FunctionResponse
```

重点讲：

- AgentTool 的 declaration 看起来像普通 tool。
- Tool.Run 同步阻塞。
- child session 隔离。
- child output 变成 `{"result": lastText}`。

### 13.5 demoRemoteA2AStreaming：远程 chunk 聚合

演示：

```text
"According "
"to the "
"latest data, "
the capital is Tokyo.
completed
```

重点讲：

- remote append chunks 不直接变成多个最终 event。
- aggregator 负责 partial -> full。
- status update 写入 `remote_task_state`。

---

## 14. 容易误解点

### 14.1 误区：Sequential/Parallel/Loop 是外部调度器

不是。

它们实现 agent 接口，本身就是 agent。

Runner 不知道它们内部是一个 agent 还是多个 agent。

### 14.2 误区：ParallelAgent 的 branch 是状态隔离

不是。

branch 是 event metadata。

ParallelAgent 的子 agent 仍共享同一个 session。并发写同一个 state key 的结果不要依赖。

### 14.3 误区：AgentTool 子 agent 状态会回写父 session

不会。

AgentTool 创建 child session，并只做父 state 到 child state 的单向拷贝。

child 写入不会回父 session。

### 14.4 误区：AgentTool 是异步委派

不是。

`Tool.Run()` / `RunWithContext()` 是同步方法。父 Flow 等 child runner 完成后才得到 tool result。

### 14.5 误区：Remote converter 输出就是最终输出

不一定。

converter 可能输出 partial event。RemoteAgent 还要经过 aggregator，根据 `Append` 和 `LastChunk` 决定是否 suppress、flush、reset。

### 14.6 误区：LoopAgent 会强制停止子 agent

不会。

LoopAgent 只在子 agent 返回 events 后检查 `Actions.Escalate`。

如果子 agent 卡住，LoopAgent 不会凭空抢占它。

### 14.7 误区：AgentTool 会复制所有父状态

不会。

`_adk` 前缀的内部 state 不会复制。

这是为了避免把运行时内部状态泄漏给 child agent。

---

## 15. 源码行为对照表

| 行为 | 当前复刻版结论 | 证据 |
| --- | --- | --- |
| Workflow agent 类型 | Sequential/Parallel/Loop 都实现 agent + Execute | `workflow/workflow.go`、interface tests |
| Sequential 执行顺序 | 按声明顺序执行和输出 | `TestSequentialAgentOrder`、`TestWorkflowE2E_Sequential_ThroughRunner` |
| Sequential error | 遇到第一个 error 停止，保留前序 events | `TestSequentialAgentErrorStopsChain` |
| Sequential EndInvocation | child EndInvocation 后停止后续 child | `TestSequentialAgentEndInvocationStopsChain` |
| Sequential state sharing | 后续 child 可读前序 child 写入的 session state | `TestSequentialAgentStateSharing`、`TestWorkflowE2E_Sequential_StateSharing_ThroughRunner` |
| Parallel 执行 | goroutine 并发，输出按 index 排序 | `TestParallelAgentBranchAndEventAggregation` |
| Parallel branch | event branch 是 `parent.child` | `TestWorkflowE2E_Parallel_WithBranchLabels` |
| Parallel error | 所有 child 跑完，返回第一个 error 和成功 events | `TestParallelAgentErrorPropagation` |
| Loop max iterations | 达到上限停止 | `TestLoopAgentMaxIterations` |
| Loop Escalate | event.Actions.Escalate 触发 early stop | `TestLoopAgentEarlyStopOnEscalate` |
| Loop max=0 | 无限循环直到 Escalate/error | `TestLoopAgentZeroMaxIterations` |
| AgentTool declaration | 输入 schema 是 `{request: string}` | `TestAgentTool_Declaration` |
| AgentTool child output | 返回最后文本为 `{"result": text}` | `TestAgentTool_Run_ChildOutput` |
| AgentTool empty output | 返回空 map | `TestAgentTool_Run_EmptyOutput` |
| AgentTool child state | 不回写父 session | `TestAgentTool_Run_SessionIsolation` |
| AgentTool parent state | 非 `_adk` 父 state 会复制给 child | `TestAgentTool_Run_ParentStateCopied` |
| AgentTool internal state | `_adk` 前缀不复制 | `TestAgentTool_Run_InternalStateNotCopied` |
| AgentTool skip summary | 设置 `Actions.SkipSummarization` | `TestAgentTool_Run_SkipSummarization` |
| Remote conversion | remote Message/Status 转 local Event | `TestConvertToSessionEvent_*` |
| Remote function call | FunctionCall/FunctionResponse 结构可双向转换 | `TestConvertToSessionEvent_MessageWithFunctionCall`、`TestConvertSessionEventToRemote_FunctionCall` |
| Aggregator append chunks | Append chunks suppress until LastChunk/terminal flush | `TestAggregator_AppendChunksThenLastChunk`、`TestAggregator_TerminalFlush` |
| Remote e2e | streaming chunks 经 Runner 聚合成 local events | `TestRemoteAgent_StreamingAggregation_ThroughRunner` |
| Cleanup callbacks | stream/convert error 会触发 cleanup | `TestCleanupCallbacks_*`、`TestRemoteAgent_Cleanup_ThroughRunner` |

---

## 16. 建议课堂脚本

### 16.1 先讲三条故事线

用三个业务问题开场：

```text
1. 本地代码审查流水线怎么拆 agent？
2. 父 agent 怎么临时委派数学专家 agent？
3. 本地 agent 怎么调用远程知识库 agent？
```

分别对应：

```text
Workflow Agent
AgentTool
RemoteAgent
```

### 16.2 再讲 session 语义

先画这张表：

```text
Workflow: shared local session
AgentTool: isolated child session + parent state one-way copy
RemoteAgent: remote session transparent to local runtime
```

这一步比接口定义更重要。

### 16.3 然后读源码

建议顺序：

1. `workflow/workflow.go`：先理解 composition agent。
2. `workflow/workflow_test.go`：顺序、并发、循环的行为。
3. `tool/agenttool/agent_tool.go`：agent-as-tool 和 child session。
4. `tool/agenttool/agent_tool_test.go`：父状态拷贝/隔离边界。
5. `agent/remoteagent/remote_agent.go`：远程流主流程。
6. `agent/remoteagent/convert.go`：remote/local event 转换。
7. `agent/remoteagent/aggregate.go`：partial 聚合。

### 16.4 最后跑 demo 和测试

课堂 demo：

```bash
go run ./cmd/demo -chapter=5
```

行为验证：

```bash
go test ./workflow ./tool/agenttool ./agent/remoteagent -run 'TestSequential|TestParallel|TestLoop|TestWorkflowE2E|TestAgentTool|TestConvert|TestAggregator|TestRemoteAgent' -v
```

---

## 17. 练习题

### 17.1 练习一：代码审查 Sequential workflow

目标：

```text
coder -> reviewer -> fixer
```

要求：

- coder 写入一个 state key，例如 `code_language=go`。
- reviewer 读取该 key。
- fixer 输出最终文本。
- 测试 events author 顺序是 coder、reviewer、fixer。

讨论：

- 为什么 reviewer 能读到 coder 写的 state？
- 如果 coder 返回 error，reviewer 是否执行？

### 17.2 练习二：多视角 Parallel workflow

目标：

```text
security reviewer
performance reviewer
style reviewer
```

要求：

- 三个子 agent 并发执行。
- 输出按声明顺序。
- 每个 event 都有 `review-team.child` branch。

讨论：

- 如果 performance reviewer 最快结束，为什么输出不一定排第一？
- 如果三个 reviewer 都写 `review_status`，最终 state 可靠吗？

### 17.3 练习三：修复 Loop workflow

目标：

```text
fixer repeats until tests pass
```

要求：

- 第 3 次输出 `Actions.Escalate=true`。
- `maxIterations=10`。
- 验证最终只产生 3 次 fixer events。

讨论：

- `Escalate` 是谁设置的？
- 如果永远不设置 `Escalate`，`maxIterations=0` 会怎样？

### 17.4 练习四：把专家 agent 包成 AgentTool

目标：

```text
parent orchestrator calls math_agent tool
math_agent returns "42"
parent receives FunctionResponse result=42
```

要求：

- AgentTool declaration 必须包含 `request` 参数。
- child agent 能读到父 session 的 `shared_key`。
- child agent 写的 `child_key` 不回写父 session。
- `_adk_internal_key` 不复制。

讨论：

- AgentTool 和 SequentialAgent 都能调用 child agent，什么时候用哪个？

### 17.5 练习五：Remote A2A partial aggregation

目标：

```text
Remote Append "Hello "
Remote Append "World"
Remote LastChunk "!"
Remote completed
```

要求：

- 前两个 chunks 不 emit final event。
- LastChunk flush 出非 partial aggregated event。
- completed status 写入 `remote_task_state=completed`。

讨论：

- 为什么 converter 后还需要 aggregator？
- 如果最后没有 LastChunk，但来了 completed status，应该怎么处理？

---

## 18. 自测题

1. 为什么说 SequentialAgent / ParallelAgent / LoopAgent 是 agent-as-composition，而不是外部调度器？
2. SequentialAgent 遇到子 agent error 时，会不会返回之前已经产生的 events？
3. ParallelAgent 为什么要给 event 写 `Branch`？
4. ParallelAgent 的 `Branch` 是否意味着 state 隔离？
5. LoopAgent 的 `Actions.Escalate` 是强制中断还是合作式终止？
6. `maxIterations=0` 是什么意思？
7. AgentTool 的 child session 和父 session 是什么关系？
8. AgentTool 会不会复制 `_adk` 前缀的 state？
9. AgentTool 返回给父 agent 的是什么？child events 会不会直接进入父 session？
10. RemoteAgent 中 converter 和 aggregator 的职责有什么区别？
11. Remote `Append && !LastChunk` 的 chunk 会立即 emit 吗？
12. Remote terminal status update 会对 pending chunks 做什么？

参考答案：

1. 因为它们实现 agent 接口，Runner 仍然只是调用 `Execute()`；组合逻辑在 agent 内部。
2. 会。它返回前序 events 和 wrapped error。
3. 为了标记并发事件来自哪个子 agent，格式是 `parent.child`。
4. 不意味着。Parallel child 仍共享同一个 session。
5. 合作式终止。LoopAgent 在 child 返回 events 后检查 `Escalate`。
6. 无限循环，直到 Escalate 或 error。
7. child session 是新建隔离 session；父 state 单向拷贝到 child，child 写入不回父 session。
8. 不会。`_adk` 前缀被过滤。
9. 返回 `map[string]any{"result": lastText}` 或空 map；child events 不直接进入父 session。
10. converter 做 remote/local event 格式转换；aggregator 根据 Append/LastChunk/terminal status 处理 partial 到 full。
11. 不会。会 accumulate 并 suppress emission。
12. flush pending chunks，再 emit terminal status event。

---

## 19. 本章收束

Chapter 05 的核心是多 agent 组合，但不是"越多 agent 越好"。

要根据 session 语义和调用方式选择：

```text
Sequential / Parallel / Loop:
  本地 agent 流程编排，子 agent 共享 session。

AgentTool:
  父 LLM 需要把某个完整 agent 当工具临时调用，child session 隔离。

RemoteAgent:
  本地需要调用远程 agent，把 A2A streaming event 转成本地 event。
```

一个实用判断：

```text
流程结构确定，用 Workflow。
模型动态决定是否委派专家，用 AgentTool。
专家在远程服务或另一个进程，用 RemoteAgent。
```

理解这三种组合方式，ADK Go 的 agent workflow 才真正从"单 agent loop"变成"可拆分、可复用、可部署的 agent 系统"。
