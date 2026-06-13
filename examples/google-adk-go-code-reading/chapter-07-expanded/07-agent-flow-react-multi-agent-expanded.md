# Chapter 07 - Agent Flow / ReAct / Multi-Agent 深度讲解

> 本章对应教学大纲 Chapter 07：Agent Flow / ReAct / Multi-Agent。
> 代码基线是本机 `rive-adk-go` 复刻版。原版 ADK 的概念会在必要处对照，但讲解以当前代码可验证行为为准。

---

## 0. 本章一句话

Chapter 07 回到 agent runtime 里最有味道的部分：

> ReAct 不是魔法，而是 `model -> function call -> tool result -> model` 的事件循环；Multi-Agent 也不是神秘调度，而是 transfer tool、event actions、runner history routing、agent tree config 共同构成的控制权转移机制。

如果把前几章拼起来，主线是：

```text
Runner
  -> active Agent
  -> Flow.Run loop
     -> model request
     -> model event
     -> tool events
     -> maybe transfer to another agent
     -> maybe policy plugin changes behavior
  -> events persisted to Session
  -> next user turn can resume the active agent from history
```

本章真正要讲清楚四件事：

1. ReAct loop 怎么靠 `Flow.Run` 和 `Event.IsFinalResponse()` 停下来。
2. 为什么同一次 invocation 里的 `priorEvents` 必须回灌给下一轮 model request。
3. `transfer_to_agent` 如何从"一个工具调用"变成"执行另一个 agent"。
4. `ExitLoop` / `RetryReflect` / `FunctionModifier` / JSON config 如何把前面的 hook、tool、workflow 机制串起来。

---

## 1. 为什么最后一章重要

前几章分别讲了：

```text
Chapter 01: Runner / Agent / Flow / Event / Session
Chapter 02: State / Memory / Artifact
Chapter 03: Tool System
Chapter 04: Callback / Plugin / Instruction
Chapter 05: Workflow / AgentTool / Remote A2A
Chapter 06: Entrypoint / Deploy / Telemetry
```

Chapter 07 的价值，是把这些拼成完整 agent workflow：

```text
LLM 先想一步
  -> 如果需要工具，就输出 FunctionCall
  -> runtime 执行工具
  -> tool result 回到 history
  -> LLM 看到结果后继续想
  -> 如果该交给 specialist，就 transfer
  -> 如果该停，就 ExitLoop / final response
  -> 如果工具失败，就 RetryReflect 给模型反思材料
  -> 如果有内部参数，就 FunctionModifier 控制 schema 和 state
  -> 如果要工程化，就 JSON config 构建 agent tree
```

这就是 agent workflow 的骨架。

不是"prompt 写得聪明"。

不是"多塞几个 agent"。

而是：

> 事件、工具、状态、插件、路由共同形成可解释的控制流。

---

## 2. 初学者桥：ReAct 到底是什么

### 2.1 ReAct 不是一个单独类型

当前复刻版里没有一个叫 `ReActAgent` 的核心类型。

ReAct 是 `Flow.Run` 的行为：

```text
for step := 1; ; step++ {
  runOneStep
  if final response:
    stop
  else:
    continue
}
```

也就是说，ReAct 是一个循环语义，不是魔法对象。

### 2.2 一次典型 ReAct

用户问：

```text
Tokyo 天气怎么样？
```

第一轮 model：

```text
FunctionCall(get_weather, {"city":"Tokyo"})
```

Flow 执行 tool：

```text
FunctionResponse(get_weather, {"temperature":22})
```

第二轮 model：

```text
Tokyo is 22°C and sunny.
```

事件序列是：

```text
user event
model event with FunctionCall
tool event with FunctionResponse
model event with final text
```

`cmd/demo/main.go` 的 `demoReActLoop` 正是这个例子。

### 2.3 为什么它能继续第二轮

关键不是 tool 执行本身，而是下一轮 model request 的 history 里必须包含：

```text
user question
model function call
tool function response
```

否则真实模型第二轮看不到工具结果，只能胡编。

当前复刻版用 `priorEvents` 解决这个问题：

```go
history := append([]*event.Event{}, ctx.Session().Events()...)
history = append(history, priorEvents...)
req := &model.LLMRequest{
    Contents: model.ContentsFromEvents(history),
}
```

`priorEvents` 是同一次 invocation 内刚产生、但还没持久化进 session 的 model/tool events。

`TestFlowFeedsPriorStepEventsIntoNextModelRequest` 验证第二次 model request 里有：

```text
user
prior model function call
prior tool function response
```

这条测试是 ReAct 能接真实 LLM 的关键保护。

---

## 3. Flow.Run 主循环

### 3.1 主循环结构

`Flow.Run` 的核心：

```text
allEvents = []
for step := 1; ; step++:
  if ctx.Ended():
    return allEvents

  stepEvents = runOneStep(ctx, step, allEvents)
  allEvents += stepEvents

  if any event.Actions.EndInvocation:
    ctx.EndInvocation()
    return allEvents

  modelEvent = stepEvents[0]
  if modelEvent.IsFinalResponse():
    return allEvents

  if modelEvent.Partial:
    return error

  if any later event has TransferToAgent:
    return allEvents
```

这里有四类停止条件：

| 停止条件 | 来源 |
| --- | --- |
| `ctx.Ended()` | 外层 invocation 被结束 |
| `Actions.EndInvocation` | tool/callback/plugin 发出结束动作 |
| `IsFinalResponse()` | model 输出最终文本，不再有 function call |
| `TransferToAgent` | 当前 agent 已把控制权交给目标 agent |

### 3.2 IsFinalResponse

`Event.IsFinalResponse()` 返回 true 的条件：

```text
not nil
not partial
not interrupted
no error
no TransferToAgent
content parts contain no FunctionCall
```

所以一个普通文本回答是 final。

但下面这些都不是 final：

```text
partial streaming chunk
model event with function call
event with error
event with transfer action
interrupted event
```

### 3.3 Partial 在当前 Flow 里的边界

如果 model event 是 partial：

```text
runOneStep returns partial model event
Flow.Run returns error: streaming limit reached
```

当前复刻版支持 streaming tool collection 的非 live 模式，但 Flow 的主 loop 不是真正的 live streaming ReAct。

这也解释了大纲里提到的 `StreamToolCallChecker`：

> 原版 ADK Go 需要处理 provider-specific streaming tool-call 判断；当前本机复刻版没有暴露 `StreamToolCallChecker` 抽象，主要用非 live `LLMResponse` / event slice 模型教学。

---

## 4. runOneStep：一个 ReAct step 的六段

`runOneStep` 是本章最应该精读的函数。

### 4.1 构造 request history

```go
history := append([]*event.Event{}, ctx.Session().Events()...)
history = append(history, priorEvents...)
req := &model.LLMRequest{
    Model:    f.Model.Name(),
    Contents: model.ContentsFromEvents(history),
}
```

这里把两段 history 合起来：

```text
session persisted events
current invocation prior events
```

这就是工具结果回灌的核心。

### 4.2 preprocess

RequestProcessors 可以：

- 修改 request。
- 直接返回 event short-circuit。
- 返回 error。

Instruction layer 就是在这一层接入。

### 4.3 inject tools 和 transfer tool

普通工具声明：

```go
f.injectToolDeclarations(req)
```

transfer 工具：

```go
f.injectTransferTool(currentAgent, req)
```

如果当前 agent 有可转移目标，`transfer_to_agent` 会作为工具声明注入 request。

### 4.4 callModel + postprocess

`callModel` 顺序仍然是 Chapter 04 的 hook 链：

```text
Plugin BeforeModel
direct BeforeModel ctx
direct BeforeModel legacy
Model.GenerateContent
Plugin OnModelError
Plugin AfterModel
direct AfterModel ctx
direct AfterModel legacy
```

然后 `postprocess` 跑 ResponseProcessors。

### 4.5 finalize model event

`finalizeModelResponseEvent` 把 `model.LLMResponse` 变成 `event.Event`：

```text
ID = <invocation>-step-<n>
Author = ctx.Agent().Name()
Role = model
Branch = ctx.Branch()
Content = resp.Content
Actions = modelActions
```

### 4.6 handle function calls and transfer

如果 model event 有 function calls：

```text
handleFunctionCalls
  -> execute tool calls in goroutines
  -> merge results into one tool event
  -> merge state delta into session state
```

如果 tool event 里有：

```text
Actions.TransferToAgent = "math_bot"
```

则：

```text
executeTransfer(ctx, step, "math_bot")
```

把目标 agent 事件追加到当前 step events。

---

## 5. priorEvents：真实 LLM tool-calling 的关键

### 5.1 session history 不够

Runner 只在 agent 执行结束后才把非 partial events append 到 session。

但 ReAct 一次 invocation 内可能有多轮 step：

```text
step 1:
  model function call
  tool result

step 2:
  model final answer
```

step 2 发生时，step 1 的 events 还没有由 Runner 统一持久化进 session。

如果只读 `ctx.Session().Events()`，step 2 看不到 step 1 的 tool result。

### 5.2 allEvents 传回 runOneStep

`Flow.Run` 维护：

```go
var allEvents []*event.Event
...
stepEvents, err := f.runOneStep(ctx, step, allEvents)
allEvents = append(allEvents, stepEvents...)
```

下一次 step 会把 `allEvents` 当 `priorEvents` 加入 history。

这就是同一次 invocation 内的短期事件回灌。

### 5.3 ContentsFromEvents 必须保留结构

`model.ContentsFromEvents` 不是简单拼文本。

它会保留：

- `Text`
- `FunctionCall`
- `FunctionResponse`

并 clone args/result map，避免后续 mutation 污染 history。

`TestContentsFromEventsPreservesToolLoop` 和 `TestFlowFeedsPriorStepEventsIntoNextModelRequest` 保护这个行为。

---

## 6. FunctionCall 到 FunctionResponse 的 ReAct 动作

### 6.1 handleFunctionCalls 并发执行

model event 里可能有多个 function calls。

当前 Flow：

```text
for each fnCall:
  go executeToolCall
wait all
mergeResultsToEvent in original index order
```

执行并发，但结果数组按 function call index 写回，所以 merge 后顺序稳定。

`TestFlowMultipleToolCallsDeterministic` 验证了这个行为。

### 6.2 mergeResultsToEvent

每个 tool result 变成：

```text
event.Part{
  FunctionResponse: {
    ID: callID,
    Name: toolName,
    Result: result,
    Error: optional error,
  }
}
```

所有 tool responses 合并成一个 tool event：

```text
<invocation>-step-<n>-toolresults
```

如果 result 里有：

```text
"state_delta": map[string]any{...}
```

会转成：

```text
ev.Actions.StateDelta
```

然后 Flow 立即：

```go
session.MergeStateDelta(ctx.Session().State(), merged.Actions.StateDelta)
```

这保证同一次 Flow 后续 step 能读到 tool 写入的 state。

---

## 7. transfer_to_agent：工具调用如何变成控制权转移

### 7.1 transfer 不是特殊 LLM 能力

模型看到的仍然只是一个工具：

```text
transfer_to_agent(agent_name)
```

`tool/transfer/tool.go` 里的 declaration schema：

```text
agent_name: string enum [allowed agent names]
```

模型输出：

```json
{
  "name": "transfer_to_agent",
  "arguments": {
    "agent_name": "math_bot"
  }
}
```

### 7.2 ComputeTransferTargets

当前 agent 的可转移目标来自：

1. 所有 sub-agents。
2. parent，除非 `DisallowTransferToParent=true`。
3. peers，除非 `DisallowTransferToPeers=true`，且 parent 允许 peer transfer。

所以目标不是固定写死的。

它取决于当前 agent 在 agent tree 里的位置。

### 7.3 InjectTransferTool

`InjectTransferTool(currentAgent, req)` 做两件事：

1. 把 `transfer_to_agent` declaration append 到 `req.ToolDeclarations`。
2. 把 transfer instructions append 到 `req.SystemInstruction`。

如果没有 transfer targets：

```text
return nil
```

此时模型即使输出 `transfer_to_agent`，Flow 也会把它当普通 tool not found。

`TestFlowTransferWithoutSubAgentsHasEmptyTargets` 验证了这个边界。

### 7.4 RunWithContext 设置 action

`TransferToAgentTool.RunWithContext` 在验证 agent name 后：

```go
tc.Actions().TransferToAgent = agentName
return {"transferred_to": agentName}
```

这意味着 transfer 的真正信号在 tool event actions 里：

```text
tool event Actions.TransferToAgent = "math_bot"
```

### 7.5 executeTransfer inline 执行目标 agent

Flow 看到 transfer action 后：

```text
rootAgent.FindAgent(targetName)
targetAgent must implement Execute
depth guard
targetCtx := transferContext(...)
targetAgent.Execute(targetCtx)
append target events
```

`transferContext` 改写：

```go
Agent()     -> targetAgent
AgentName() -> targetAgent.Name()
Branch()    -> targetName
```

所以目标 agent 产生的 events author 是 specialist，而不是 host。

`TestFlowTransferToSubAgent` 验证：

```text
model event with transfer function call
tool event with TransferToAgent
last event authored by math_bot
```

### 7.6 invalid target 是结构化 tool error

如果 root tree 找不到 target：

```text
executeTransfer returns an event with FunctionResponse error
```

不会直接 panic。

`TestFlowTransferInvalidTarget` 验证 tool event 里有 `transfer_to_agent` 的 error response。

### 7.7 transfer loop guard

如果 target events 里继续带 `Actions.TransferToAgent`，Flow 会递归 executeTransfer。

为避免无限转移：

```go
const maxTransferDepth = 10
```

超过后报：

```text
transfer loop detected
```

`TestFlowTransferLoopDetection` 验证这个行为。

---

## 8. Runner：下一轮如何恢复 active agent

### 8.1 Transfer 不是只影响当前 invocation

一次用户消息里 host transfer 到 math_bot 后，下一次用户消息应该继续由哪个 agent 处理？

当前 Runner 的答案是：

```text
扫描 session history，找最近的非 user event author。
如果这个 author 对应一个 transferable agent，就让它处理下一轮。
```

### 8.2 findAgentToRun

`Runner.findAgentToRun`：

```text
events := sess.Events()
for i := len(events)-1; i >= 0; i--:
  ev := events[i]
  if ev == nil or ev.Author == "user":
    continue

  candidate := root.FindAgent(ev.Author)
  if candidate == nil:
    continue

  if isTransferableAcrossAgentTree(candidate):
    return candidate

return root
```

所以它不读一个专门的 `active_agent` state key。

它从已持久化 event author history 里恢复 active agent。

### 8.3 isTransferableAcrossAgentTree

一个 candidate 是否可继续接管，取决于它和所有祖先：

```text
for cur := candidate; cur != nil; cur = cur.Parent():
  if cur.DisallowTransferToParent():
    return false
return true
```

任意祖先禁止 transfer-to-parent，candidate 就不可作为下一轮 active agent。

测试：

- `TestRunnerIsTransferableAcrossAgentTree`
- `TestRunnerIsTransferableWhenAllowed`

### 8.4 第二轮路由

`TestRunnerSecondRunRoutesToActiveAgent` 验证：

1. 第一轮 root transfer 到 `math_bot`。
2. session 持久化了 `math_bot` authored event。
3. `findAgentToRun(sess)` 返回 `math_bot`。
4. 第二轮用户消息由 `math_bot` 处理。

这就是 Multi-Agent 对话能延续 specialist 上下文的原因。

---

## 9. ExitLoop：用 tool action 结束 invocation

### 9.1 ExitLoopTool

`tool/exitloop/exitloop.go` 很小：

```go
func (e *ExitLoopTool) RunWithContext(tc tool.ToolContext, args map[string]any) (map[string]any, error) {
    if actions := tc.Actions(); actions != nil {
        actions.EndInvocation = true
    }
    return map[string]any{"ended": true}, nil
}
```

它是一个工具。

模型调用：

```text
exit_loop()
```

tool event actions：

```text
EndInvocation = true
```

Flow 主循环看到后：

```text
ctx.EndInvocation()
return allEvents
```

### 9.2 ExitLoop 和 LoopAgent Escalate 不是一回事

ExitLoopTool 设置：

```text
Actions.EndInvocation
```

LoopAgent 的合作式终止看：

```text
Actions.Escalate
```

消费者不同。

不要把两者混成一个概念。

### 9.3 测试行为

`TestFlowExitLoopStopsMultiStep` 验证：

```text
model fc exit_loop
tool event EndInvocation=true
Flow stops
```

`TestFlowExitLoopAfterToolCall` 验证：

```text
step1 normal tool
step2 exit_loop
step3 queued response not consumed
```

`TestFlowExitLoopSkipsRemainingQueue` 验证剩余 fake model queue 不会继续执行。

---

## 10. RetryReflect：工具失败后给模型反思材料

### 10.1 OnToolError 恢复错误

`plugin/retryreflect` 注册了：

```text
OnToolError
AfterTool
```

当 tool 失败：

```text
failure count +1
result["error"] = original error
if count <= maxRetries:
  result["reflection"] = "Tool failed... consider alternative..."
else:
  result["reflection_exceeded"] = "Stop using this tool..."
return result, nil
```

这会把工具错误转成结构化 `FunctionResponse`，让模型下一轮能看到：

```text
error
reflection
```

而不是让整个 Flow 直接失败。

### 10.2 成功后重置计数

`AfterTool` 在成功且 result 没有 error 时：

```text
delete failure count for toolName
```

`TestFlowRetryReflectThenResolve` 验证第一次失败有 reflection，第二次成功后 failure count reset。

### 10.3 原始错误不隐藏

`TestFlowRetryReflectPreservesOriginalError` 验证：

```text
Result["error"] contains original database error
Result["reflection"] exists
```

这点很重要：RetryReflect 不是掩盖错误，而是把错误变成模型可见的策略材料。

---

## 11. FunctionModifier：Hidden Args 的当前语义

### 11.1 BeforeModel 修改 tool declaration

`plugin/functionmodifier` 在 BeforeModel 中遍历 `req.ToolDeclarations`：

```text
if declaration matches Predicate:
  add HiddenArgs keys into InputSchema.properties
  add keys into required list
```

这影响的是当前 request 的 tool declaration。

`req` 是每轮新建的，所以不是永久修改 tool 本体。

### 11.2 AfterModel 剥离 function call args

AfterModel 中：

```text
for matching FunctionCall:
  if fc.Args contains hidden arg:
    copy to stripped
    delete from fc.Args
    state.Set("hidden/<callID>/<argName>", value)
```

也就是说，当前复刻版 Hidden Args 的重点是：

> 允许模型输出某些内部参数，但在工具执行前把它们从 args 里剥离，并写入 callback state。

不是把 hidden value 自动注入给工具执行。

### 11.3 工具看不到 hidden args

`TestFlowFunctionCallModifierHiddenArgs` 验证 tool 捕获到的 args 里没有：

```text
user_id
internal
```

只保留模型原本传的普通参数：

```text
query = hello
```

所以讲课时不要说"hidden args 会被注入到工具参数里执行"。

更准确的说法是：

```text
BeforeModel: 修改 schema，让某些参数出现在可调用声明里。
AfterModel: 从模型 function call 中剥离这些参数，写入 state，避免直接进入 tool execution args。
```

### 11.4 Predicate 限定作用范围

`Predicate(toolName)` 决定哪些 tool declaration / function call 会被处理。

`TestFlowFunctionCallModifierOnlyMatchesPredicate` 验证非匹配 tool 不会注入/剥离 hidden args。

---

## 12. JSON agentconfig：把 agent tree 工程化

### 12.1 支持的类型

`agent/agentconfig/config.go` 支持：

```text
llm_agent
sequential
parallel
loop
```

这正好把前几章连起来：

- `llm_agent` -> Flow/ReAct/tool agent
- `sequential` / `parallel` / `loop` -> Chapter 05 workflow agents
- `tools` -> Chapter 03 tool registry
- transfer flags -> Chapter 07 routing约束

### 12.2 Build 主流程

`Build(cfg, registry)`：

```text
1. cfg.Name required
2. validateNoDuplicateNames
3. validateToolRefs
4. buildNode
5. wireParents
```

构建出来的是标准 `agent.Agent`。

不是另一套 runtime。

### 12.3 buildLLMAgent

`llm_agent`：

```text
resolve tools from registry
build sub agents
create Flow{Model: FakeModel(modelName), Tools: toolsMap}
llmagent.New(name, description, flow)
set parent/sub-agents
apply transfer constraints
```

当前复刻版的 config loader 用 FakeModel，适合结构教学，不负责真实 model provider 配置。

### 12.4 workflow agent

`sequential`、`parallel`、`loop` 会：

```text
build sub nodes
assert each sub implements workflow.SubAgent
call workflow.NewSequentialAgent / NewParallelAgent / NewLoopAgent
apply transfer constraints
```

Loop 默认：

```text
if maxIterations == 0:
  maxIterations = 10
```

这和 Chapter 05 直接调用 `NewLoopAgent(..., 0)` 表示无限循环不同。

在 config loader 里，0 被当成"未配置"，默认 10。

### 12.5 验证点

测试覆盖：

- `TestFromJSONBasic`
- `TestBuildValidTree`
- `TestBuildDuplicateNames`
- `TestBuildUnknownTool`
- `TestBuildUnknownToolListsAvailableToolsDeterministically`
- `TestBuildSequentialAgent`
- `TestBuildParallelAgent`
- `TestBuildLoopAgent`
- `TestBuildNestedTreeWithParentLinks`
- `TestBuildWorkflowAgentParentLinksAndTransferConstraints`

---

## 13. 课堂演示

`cmd/demo/main.go` 的 `runChapter07` 有四组 demo。

### 13.1 demoReActLoop

演示：

```text
weather_bot
  model -> get_weather function call
  tool -> weather result
  model -> final answer
```

重点讲：

- ReAct 是 Flow loop。
- 第二次 model request 依赖 priorEvents history 回灌。
- 输出 events 是 model/tool/model。

### 13.2 demoAgentTransfer

演示：

```text
host_agent
  -> transfer_to_agent(math_agent)
  -> math_agent executes calculator
  -> math_agent returns final text
```

重点讲：

- transfer tool 是动态注入的。
- tool event actions 承载 `TransferToAgent`。
- `transferContext` 让 specialist events 的 author 是 `math_agent`。

### 13.3 demoPolicyExtensions

三个子 demo：

```text
ExitLoop:
  exit_loop tool -> EndInvocation

RetryReflect:
  failing tool -> reflection in FunctionResponse

Hidden Args:
  FunctionModifier before/after model controls schema and args stripping
```

重点讲：

- 这些不是改 Flow 主循环，而是通过 tool/plugin hook 注入策略。
- 策略插件是 Chapter 04 的落地用法。

### 13.4 demoConfigurableConstruction

演示：

```text
JSON -> AgentConfig -> Build -> agent tree
```

重点讲：

- duplicate name error
- unknown tool error
- unknown type error
- parent chain wiring

---

## 14. 容易误解点

### 14.1 误区：ReAct 是无限循环

不是。

当前 Flow 每轮都检查：

```text
ctx.Ended
EndInvocation
IsFinalResponse
Partial error
TransferToAgent
```

不过当前 replica 没有显式 `maxSteps`，生产环境仍应加上限或 context timeout。

### 14.2 误区：只要 session history 就够了

不够。

同一次 invocation 内刚产生的 model/tool events 尚未由 Runner 持久化，必须通过 `priorEvents` 加入下一轮 request。

### 14.3 误区：transfer_to_agent 是普通业务工具

外观看是工具，但语义不普通。

它的 result 会带：

```text
Actions.TransferToAgent
```

Flow 会消费这个 action 并 inline 执行目标 agent。

### 14.4 误区：transfer 后 event author 仍是 host

不是。

`transferContext` 改写当前 agent，所以 specialist event author 是 specialist name。

### 14.5 误区：所有 agent 都能作为下一轮 active agent

不是。

Runner 会检查 candidate 和祖先链的 `DisallowTransferToParent`。

不满足时回退 root。

### 14.6 误区：ExitLoop 和 LoopAgent Escalate 是同一个信号

不是。

ExitLoop 是 `EndInvocation`，由 Flow 主循环消费。

LoopAgent 的 early stop 是 `Escalate`，由 workflow loop 消费。

### 14.7 误区：FunctionModifier 会把 hidden value 注入工具执行

当前复刻版不是。

它修改 declaration，并从 model function call args 中剥离 hidden args写入 state，工具执行时看不到这些 hidden args。

### 14.8 误区：JSON config 支持所有 runtime 能力

不支持。

当前 config loader 不支持：

- YAML
- plugin config
- instruction strings
- callback registration
- deployment config
- real provider config

它是 agent tree 构造教学版。

### 14.9 误区：StreamToolCallChecker 已在当前 replica 实现

没有。

大纲提到的是原版/完整 runtime 的 streaming provider 问题。当前 replica 主要用非 live response/event slice 模型教学。

---

## 15. 源码行为对照表

| 行为 | 当前复刻版结论 | 证据 |
| --- | --- | --- |
| ReAct loop | `Flow.Run` 多 step，直到 final/end/transfer | `flow/flow.go` |
| 同 invocation history | `priorEvents` 加入下一轮 request | `TestFlowFeedsPriorStepEventsIntoNextModelRequest` |
| FunctionCall final 判定 | 有 FunctionCall 时 `IsFinalResponse=false` | `event/event.go`、`event_test.go` |
| 多工具执行 | goroutine 并发执行，结果按 index merge | `TestFlowMultipleToolCallsDeterministic` |
| state_delta merge | tool result 的 `state_delta` 合入 event actions/session | `mergeResultsToEvent`、`TestFlowStateDeltaMerge` |
| transfer tool injection | 有 targets 时注入 declaration + instructions | `transfer.InjectTransferTool` |
| transfer target rules | sub-agents、parent、peers，受 disallow flags 控制 | `ComputeTransferTargets` |
| transfer action | `RunWithContext` 设置 `Actions.TransferToAgent` | `transfer/tool.go` |
| inline transfer | Flow 查 root tree，执行 target agent | `executeTransfer`、`TestFlowTransferToSubAgent` |
| invalid transfer target | 返回 structured tool error event | `TestFlowTransferInvalidTarget` |
| transfer loop guard | max depth 10 | `TestFlowTransferLoopDetection` |
| next-turn active agent | Runner 反向扫描 session event author | `findAgentToRun` |
| active agent constraints | candidate 和祖先不能 disallow transfer-to-parent | `isTransferableAcrossAgentTree` |
| second run routing | transfer 后下一轮路由到 specialist | `TestRunnerSecondRunRoutesToActiveAgent` |
| ExitLoop | tool 设置 `EndInvocation`，Flow 停止 | `TestFlowExitLoopStopsMultiStep` |
| RetryReflect | tool error 转 reflection result | `TestFlowRetryReflectPlugin` |
| RetryReflect success reset | 成功后 failure count 清零 | `TestFlowRetryReflectThenResolve` |
| FunctionModifier | declaration 修改 + hidden args 剥离到 state | `plugin/functionmodifier`、react policy tests |
| JSON Build | validate -> buildNode -> wireParents | `agentconfig/config.go` |
| JSON duplicate/tool/type errors | deterministic errors | `agentconfig/config_test.go` |

---

## 16. 建议课堂脚本

### 16.1 先从 ReAct event sequence 开场

画：

```text
user
model(FunctionCall)
tool(FunctionResponse)
model(final)
```

然后问：

```text
第二个 model 怎么知道 tool result？
```

引出 `priorEvents`。

### 16.2 再读 Flow.Run

重点不是每行代码，而是五个控制点：

```text
allEvents
runOneStep
EndInvocation
IsFinalResponse
TransferToAgent
```

### 16.3 接着讲 transfer

分四步：

```text
InjectTransferTool
LLM FunctionCall
TransferToAgent action
executeTransfer
```

然后补充下一轮：

```text
Runner.findAgentToRun scans history
```

### 16.4 再讲策略插件

用三个问题：

```text
怎么让模型主动结束？ -> ExitLoop
工具失败怎么给模型反思材料？ -> RetryReflect
内部参数怎么不直接进 tool args？ -> FunctionModifier
```

### 16.5 最后讲 JSON config

把它定位成：

> agent tree 的工程化入口，不是完整生产配置系统。

---

## 17. 练习题

### 17.1 练习一：画完整 ReAct loop

目标：

```text
user -> model fc -> tool result -> model final
```

要求：

- 标出哪些 events 已经在 session。
- 标出哪些 events 只是 `priorEvents`。
- 解释第二次 model request 的 `Contents` 应该有几条。

参考：

- `TestFlowFeedsPriorStepEventsIntoNextModelRequest`

### 17.2 练习二：transfer 到二级 specialist

目标：

```text
root
  -> planner
      -> math_bot
```

要求：

- root 或 planner 输出 `transfer_to_agent("math_bot")`。
- 验证最终 event author 是 `math_bot`。
- 验证 transfer target enum 包含正确 agent names。

讨论：

- parent/peer transfer flags 如何影响 targets？

### 17.3 练习三：ExitLoop 替代方案

目标：

不用 `exit_loop` tool，让 flow 提前停止。

给出两种方案：

- BeforeModel callback 返回 final response。
- Tool/Callback 设置 `Actions.EndInvocation`。

讨论：

- 这两种方案和 ExitLoopTool 的差别是什么？

### 17.4 练习四：RetryReflect 超限

目标：

让同一个 tool 连续失败超过 `MaxRetries`。

要求：

- 前几次 result 有 `reflection`。
- 超限后 result 有 `reflection_exceeded`。
- 成功后 count reset。

### 17.5 练习五：FunctionModifier state 检查

目标：

让模型输出：

```text
FunctionCall(search, {"query":"hello", "user_id":"u1"})
```

要求：

- tool 执行时 args 没有 `user_id`。
- callback/session state 里存在 `hidden/<callID>/user_id`。
- final model request 不再直接暴露 hidden arg 给 tool。

### 17.6 练习六：JSON config 构建 workflow

目标：

写 JSON：

```text
sequential pipeline
  generator
  loop fix_loop(max_iterations=3)
    fixer
```

要求：

- `Build` 后 parent links 正确。
- duplicate agent name 报错。
- unknown tool ref 报错，并列出 sorted available tools。

---

## 18. 自测题

1. 当前复刻版里 ReAct 是哪个函数实现的？
2. 为什么 `priorEvents` 对真实 LLM tool-calling 必不可少？
3. `IsFinalResponse()` 为什么遇到 FunctionCall 会返回 false？
4. 多个 function calls 是串行执行还是并发执行？输出顺序如何保证？
5. `transfer_to_agent` 的 allowed enum 来自哪里？
6. `TransferToAgent` action 是在哪一层设置的？
7. `executeTransfer` 如何保证 target event author 是 specialist？
8. 下一轮用户消息如何恢复到上一次 transfer 后的 active agent？
9. `ExitLoopTool` 设置的是 `EndInvocation` 还是 `Escalate`？
10. `RetryReflect` 是否隐藏原始错误？
11. 当前 FunctionModifier 是否把 hidden value 注入 tool execution args？
12. JSON config 的 loop `max_iterations=0` 在 builder 里是什么意思？

参考答案：

1. `flow.Flow.Run` 的多 step loop。
2. 因为同一次 invocation 里的 model/tool events 尚未持久化到 session，第二轮 request 必须通过 `priorEvents` 看到它们。
3. FunctionCall 表示还有 tool 要执行，不是最终回答。
4. 并发执行，结果写入 index 对应位置后按原顺序 merge。
5. `ComputeTransferTargets(currentAgent)`：sub-agents、parent、peers，受 transfer flags 约束。
6. `TransferToAgentTool.RunWithContext` 在 tool context actions 上设置。
7. 用 `transferContext` 覆盖 `Agent()` / `AgentName()` / `Branch()`。
8. Runner 反向扫描 session events，跳过 user author，找最近 transferable agent。
9. `EndInvocation`。LoopAgent 用的是 `Escalate`。
10. 不隐藏。`Result["error"]` 保留原始错误，另加 `reflection` 或 `reflection_exceeded`。
11. 不会。当前实现会剥离 hidden args 并写入 state，tool args 看不到它们。
12. 在 config builder 里表示未配置，默认成 10；这不同于直接构造 `workflow.NewLoopAgent(..., 0)` 的无限循环语义。

---

## 19. 本章收束

Chapter 07 的核心是把整套 ADK Go 复刻版串起来：

```text
ReAct:
  Flow.Run + Event + Tool + priorEvents

Transfer:
  dynamic transfer_to_agent tool + EventActions + inline target execution + runner history routing

Policy:
  ExitLoop / RetryReflect / FunctionModifier 通过 tool/plugin 改变行为，不污染 Flow 主循环

Config:
  JSON -> standard agent tree，把 llm_agent/workflow/tool refs/transfer flags 工程化
```

真正要带走的是：

> agent workflow 的复杂性不在"让模型多想几步"，而在于每一步都能被事件记录、被工具执行、被历史回灌、被策略插件改写、被 transfer 路由，并在下一轮恢复到正确 agent。

这也是这套代码最值得精读的地方。
