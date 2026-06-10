# Chapter 07 - Agent Flow / ReAct / MultiAgent 深度讲解

> 复刻代码目录：`examples/eino-compose-runtime-replica-go/agent/` 与 `examples/eino-compose-runtime-replica-go/compose/multiagent.go`  
> 核心文件：`agent/types.go`、`agent/react.go`、`compose/multiagent.go`  
> 建议讲解时长：12 分钟；建议自学阅读时长：45-70 分钟

这一章讲 Agent Flow。它看起来像是从 Graph 运行时之上突然跳到一个更"应用层"的概念：ReAct、Tool Calling、Multi-Agent、Host、Specialist。但复刻代码想表达的核心非常朴素：

**Agent 不是另一套运行时。Agent 是一张被预先搭好的 `compose.Graph`。**

如果前六章已经讲清楚 Graph、Branch、Runnable、State、ToolsNode 和 ChatModel，这一章要做的事情就是把这些零件重新组合起来：

- ChatModel 负责决定是否调用工具。
- ToolsNode 负责执行工具调用。
- Branch 负责根据模型输出选择下一跳。
- Graph local state 负责保存跨轮消息历史和直接返回标记。
- MultiAgent 把 specialist 暴露成 Host 可选择的"工具"。

所以 Chapter 07 的读法不是"学习一个全新框架"，而是"确认前面章节的抽象是否真的能拼出 Agent"。

---

## 1. 从手写 agent loop 的问题开始

很多人第一次实现 ReAct，都会写出类似这样的循环：

```go
msgs := userMsgs
for step := 0; step < maxStep; step++ {
    msg, err := model.Generate(ctx, msgs)
    if err != nil {
        return nil, err
    }
    if len(msg.ToolCalls) == 0 {
        return msg, nil
    }
    toolMsgs := runTools(ctx, msg.ToolCalls)
    msgs = append(msgs, msg)
    msgs = append(msgs, toolMsgs...)
}
return nil, ErrTooManySteps
```

这段代码足够解释 ReAct 的直觉，但它不适合进入可组合的 runtime。问题至少有四类。

第一，终止条件是内容驱动的。正常结束不是靠固定 step，而是靠模型这一轮不再输出 `ToolCalls`。也就是说，循环结构和分支判断必须能由节点输出动态决定。

第二，消息历史不是普通节点输出。每一轮 ChatModel 都需要看到完整 history，但 ToolsNode 的输出只是工具结果。如果只靠前驱节点输出传递数据，ChatModel 下一轮只能看到 ToolsNode 的 `[]*Message`，看不到用户原始消息和上一轮 assistant tool call。复刻版因此把 history 放进 graph local state。

第三，工具结果可能直接就是最终答案。搜索、数据库查询、检索类工具有时已经返回精确答案，此时再让模型"总结一下"反而可能引入幻觉。Agent 需要在 ToolsNode 之后插入一个额外分支：继续回到 ChatModel，还是直接返回某个 tool result。

第四，流式输出下不能只看最终消息。不同 provider 的 tool call chunk 到达顺序不同。OpenAI 风格常常第一批有效 chunk 就包含 tool call；Claude 风格可能先出现文本、再出现 tool use。判断"是否有 tool call"的逻辑必须可插拔。

复刻代码用 Graph 来承接这些问题。Graph 不关心"Agent"这个词，它只提供节点、边、分支、状态和编译后的 Runnable。ReAct 只是把这些机制按固定拓扑装配好。

---

## 2. 本章代码地图

本章核心代码不多，建议按下面顺序读：

| 文件 | 重点 | 为什么先读 |
| --- | --- | --- |
| `agent/types.go` | `AgentConfig`、`MessageRewriter`、`MessageModifier`、`StreamToolCallChecker`、`reactState`、`Agent` | 先看可插拔点和状态形状 |
| `agent/react.go` | `NewAgent`、`modelPreHandle`、`toolsNodePreHandle`、`modelPostBranchCondition`、`buildReturnDirectly`、两个 checker、`SetReturnDirectly` | ReAct Graph 的实际装配过程 |
| `compose/multiagent.go` | `Specialist`、`MultiAgentConfig`、`NewMultiAgent`、`executeMultiAgent`、`invokeSpecialist`、summary 逻辑 | Host Multi-Agent 的路由和 specialist 调用 |
| `agent/react_test.go` | 单轮、多轮、直接返回、rewriter/modifier、checker、state isolation | 用测试确认每个控制点 |
| `compose/multiagent_test.go` | 单 specialist、多 specialist、原始消息入参、三种 specialist 能力、错误处理 | 用测试确认 MultiAgent 语义 |

先给一句总括：

`NewAgent` 返回的是：

```go
type Agent struct {
    Runnable compose.Runnable[[]*compose.Message, *compose.Message]
    Graph    *compose.Graph[[]*compose.Message, *compose.Message]
}
```

这意味着 ReAct agent 的输入是用户消息列表，输出是一个 assistant/tool message。`Generate` 只是调用 `Runnable.Invoke`：

```go
func (a *Agent) Generate(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
    return a.Runnable.Invoke(ctx, input)
}
```

所以 Agent 的执行能力完全来自 Graph 编译后的 Runnable。

---

## 3. ReAct 的图结构

复刻版 `agent/react.go` 在文件开头直接写出了拓扑：

```text
START -> ChatModel
ChatModel --(has tool call)--> Tools -> ChatModel (loop)
ChatModel --(no tool call)--> END
Tools --(return directly)--> direct_return lambda -> END
```

换成更适合课堂讲解的图：

```text
              ┌──────────────────────┐
              │      ChatModel       │
              └──────────┬───────────┘
                         │
              has tool?  │  no
                  yes    │
                         v
        ┌────────────── Tools ───────────────┐
        │                │                    │
        │ returnDirect?  │ no                 │ yes
        │                v                    v
        │          ChatModel loop       direct_return
        │                                      │
        └──────────────────────────────────────┘
                                               v
                                              END
```

图里最重要的不是节点数量，而是两个分支：

1. ChatModel 后面的分支：模型是否输出了 tool call。
2. Tools 后面的分支：这次工具结果是否应该直接作为最终答案。

这两个分支把"手写 for 循环里的 if"变成了 Graph 的结构。

### 3.1 `NewAgent` 的装配顺序

`NewAgent` 的主流程可以拆成八步：

```go
func NewAgent(ctx context.Context, config *AgentConfig) (*Agent, error) {
    validate config
    default MaxStep

    g := compose.NewGraph[[]*compose.Message, *compose.Message]()

    add ChatModel node with modelPreHandle
    add START -> chat_model

    add Tools node with toolsNodePreHandle
    add chat_model -> END
    add chat_model -> tools
    add branch on chat_model

    buildReturnDirectly(g, config)

    compile with:
      graph name = ReActAgent
      max run steps = config.MaxStep
      trigger mode = AnyPredecessor
      local state factory = *reactState
}
```

注意 `chat_model -> END` 和 `chat_model -> tools` 两条边都存在。真正决定走哪条边的是 `AddBranch`。边定义了"可能到哪里"，branch condition 定义了"本轮实际去哪里"。

### 3.2 为什么要 `AnyPredecessor`

ReAct 是循环图。`chat_model` 的前驱有两个：

- `START`，用于第一轮。
- `tools`，用于工具执行后的下一轮。

如果使用"所有前驱都到齐再触发"的语义，循环节点会被卡死，因为第一轮不可能同时等到 START 和 tools。复刻版编译时使用：

```go
compose.WithNodeTriggerMode(compose.AnyPredecessor)
```

含义是：任一前驱产生输入就可以触发节点。这正好适合循环图。

这也是 Chapter 01 的 Pregel 运行时在本章的落点：Agent 并没有绕开 Pregel，它只是选择了适合循环图的触发模式。

### 3.3 `MaxStep` 是保险丝，不是正常终止条件

`AgentConfig.MaxStep` 默认是 20：

```go
if config.MaxStep <= 0 {
    config.MaxStep = 20
}
```

它被传给：

```go
compose.WithMaxRunSteps(config.MaxStep)
```

这不是 ReAct 的正常结束机制。正常结束靠 `modelPostBranchCondition` 判断模型输出没有 tool call，然后走 END。`MaxStep` 只是在模型一直请求工具、图一直绕圈时防止无限循环。

测试 `TestReAct_MaxStepEnforced` 就是在验证这个保险丝：模型每轮都发 tool call，最终应该得到 `compose.ErrExceedMaxSteps`。

---

## 4. `AgentConfig`：把 Agent 的可变部分放在配置里

`agent/types.go` 中的 `AgentConfig` 是本章最值得先背下来的结构：

```go
type AgentConfig struct {
    ChatModel             compose.ChatModel
    ToolsConfig           compose.ToolsNodeConfig
    MaxStep               int
    MessageRewriter       MessageRewriter
    MessageModifier       MessageModifier
    StreamToolCallChecker StreamToolCallChecker
    ToolReturnDirectly    map[string]bool
}
```

每个字段都对应一个 Agent 控制点：

| 字段 | 控制点 | 没有它会怎样 |
| --- | --- | --- |
| `ChatModel` | 决策和最终回答 | Agent 没有脑子，无法决定是否调用工具 |
| `ToolsConfig` | 工具执行能力 | 模型可以发 tool call，但 runtime 无法执行 |
| `MaxStep` | 循环上限 | 模型反复调工具时可能无限跑 |
| `MessageRewriter` | 持久改写 history | 无法做压缩、裁剪、长期记忆整理 |
| `MessageModifier` | 当前轮临时改写输入 | 无法临时加 system prompt、注入运行时约束 |
| `StreamToolCallChecker` | 流式 tool call 检测策略 | 不同 provider 下可能误判是否需要走 Tools |
| `ToolReturnDirectly` | 配置级直接返回 | 无法声明某些工具结果无需模型二次总结 |

这里有个教学细节：`StreamToolCallChecker` 在复刻版类型中保留为可插拔抽象，并提供默认与扫描全流两个实现；当前 `NewAgent` 的非流式主分支直接检查 `msg.ToolCalls`，没有在 `modelPostBranchCondition` 中真正消费 stream。也就是说，复刻版把 checker 的语义和测试留出来了，但主图路径是简化实现。讲课时不要把它说成完整原版流式分支，应该说它是"复刻版保留的 provider 差异教学边界"。

---

## 5. Graph Local State：ReAct 的消息历史放在哪里

ReAct 需要跨多轮保存完整消息历史。复刻版状态是：

```go
type reactState struct {
    Messages                 []*compose.Message
    ReturnDirectlyToolCallID string
}
```

两个字段分别服务两个问题：

- `Messages`：保存用户消息、assistant tool call 消息、tool result 消息，供下一轮 ChatModel 使用。
- `ReturnDirectlyToolCallID`：记录哪个 tool call 的结果应该被直接返回。

状态通过编译选项创建：

```go
compose.WithGenLocalState(func(ctx context.Context) *reactState {
    return &reactState{
        Messages: make([]*compose.Message, 0),
    }
})
```

这里要强调"local"。每次 Runnable 调用都有自己的 state，不同运行之间不会共享消息历史。测试里的 state isolation 就是为了确认这一点：同一个 Agent 被多次调用，第二次不应该看到第一次的消息。

---

## 6. `modelPreHandle`：Rewriter 和 Modifier 的关键分界线

`modelPreHandle` 是本章最容易误解的函数。它不是简单地把输入转成 ChatModel 需要的格式，而是负责维护 ReAct message history。

简化成伪代码：

```go
func modelPreHandle(config *AgentConfig) preHandler {
    return func(ctx context.Context, input any) (any, error) {
        s := compose.GetState[reactState](ctx)

        switch input := input.(type) {
        case []*compose.Message:
            s.Messages = append(s.Messages, input...)
        case *compose.Message:
            s.Messages = append(s.Messages, input)
        default:
            return error
        }

        if config.MessageRewriter != nil {
            s.Messages = config.MessageRewriter(ctx, s.Messages)
        }

        modified := copySlice(s.Messages)

        if config.MessageModifier != nil {
            modified = config.MessageModifier(ctx, modified)
        }

        return modified, nil
    }
}
```

执行顺序是：

```text
追加本轮输入 -> MessageRewriter 改 state -> copy state.Messages -> MessageModifier 改 copy -> 传给 ChatModel
```

这个顺序不能随便换。

### 6.1 `MessageRewriter` 是持久改写

类型定义：

```go
type MessageRewriter func(ctx context.Context, msgs []*compose.Message) []*compose.Message
```

它接收的是 `state.Messages`，返回值也会写回 `state.Messages`：

```go
s.Messages = config.MessageRewriter(ctx, s.Messages)
```

因此它适合做会影响后续轮次的事：

- 历史压缩。
- 只保留最近 N 条消息。
- 把长工具结果改写成摘要。
- 把多轮上下文整理成更短的系统上下文。

测试 `TestReAct_MessageRewriter_Compression` 的核心就是：rewriter 把消息历史限制在最近三条，后续模型轮次看到的是压缩后的历史。

### 6.2 `MessageModifier` 是当前轮临时改写

类型定义：

```go
type MessageModifier func(ctx context.Context, msgs []*compose.Message) []*compose.Message
```

它和 Rewriter 形状相似，但输入不是 state 原件，而是 shallow copy：

```go
modified := make([]*compose.Message, len(s.Messages))
copy(modified, s.Messages)

if config.MessageModifier != nil {
    modified = config.MessageModifier(ctx, modified)
}
```

因此 modifier 的修改只影响当前这一轮传给 ChatModel 的输入，不会持久写回 `state.Messages`。

适合放在 modifier 里的逻辑：

- 临时插入 system prompt。
- 当前请求级别的安全约束。
- 当前租户、语言、输出格式等运行时提示。
- 不想污染长期历史的上下文补丁。

一个典型误用是用 modifier 做历史压缩。它看似能让当前轮模型少看消息，但下一轮 state 里还是保留原始历史，压缩不会持续生效。

### 6.3 为什么先 Rewriter 再 Modifier

如果 modifier 先执行，rewriter 后执行，会产生两个问题：

第一，临时消息可能被持久化。比如 modifier 插入 system prompt，如果后续 rewriter 把它写进 state，临时提示就变成了长期历史。

第二，modifier 看不到 rewriter 的结果。比如 rewriter 已经把历史压缩成摘要，modifier 想基于压缩后的历史添加提示，就必须在 rewriter 之后运行。

复刻测试 `TestReAct_MessageRewriter_Ordering` 验证的正是这个顺序：modifier 看到的是 rewriter 处理后的消息列表。

---

## 7. `toolsNodePreHandle`：保存 assistant tool call，设置直接返回标记

ChatModel 输出 tool call 后，下一跳进入 ToolsNode。ToolsNode 执行之前，`toolsNodePreHandle` 会先处理 assistant 消息：

```go
if msg, ok := input.(*compose.Message); ok {
    s.Messages = append(s.Messages, msg)

    if config.ToolReturnDirectly != nil && len(msg.ToolCalls) > 0 {
        for _, tc := range msg.ToolCalls {
            if config.ToolReturnDirectly[tc.Function.Name] {
                s.ReturnDirectlyToolCallID = tc.ID
                break
            }
        }
    }
}
```

它做两件事。

第一，把 assistant 的 tool call 消息写入 history。没有这一步，下一轮 ChatModel 只能看到 tool result，却看不到自己上一轮请求了哪个工具。对大多数 provider 来说，这会破坏 tool call protocol。

第二，检查配置级 `ToolReturnDirectly`。如果这次 tool call 的函数名出现在 map 里，就把对应 tool call ID 写入 state。

注意直接返回标记保存的是 **tool call ID**，不是工具名。原因很简单：同一轮 assistant 可能调用多个工具，甚至可能多次调用同一个工具名。最终 direct_return 必须知道应该返回哪一个 tool result。

---

## 8. `modelPostBranchCondition`：ReAct 正常结束点

ChatModel 节点之后的分支条件非常直接：

```go
func modelPostBranchCondition(config *AgentConfig) compose.BranchCondition[*compose.Message] {
    return func(ctx context.Context, msg *compose.Message) (string, error) {
        if msg == nil {
            return compose.END, nil
        }
        if len(msg.ToolCalls) > 0 {
            return nodeKeyTools, nil
        }
        return compose.END, nil
    }
}
```

三种情况：

- `msg == nil`：没有可继续处理的输出，结束。
- 有 `ToolCalls`：走 ToolsNode。
- 无 `ToolCalls`：模型已经给出最终答案，走 END。

这就是 ReAct 的正常终止条件。不是 step 数，不是文本里出现某个关键词，也不是工具返回了某种特殊字符串，而是模型结构化输出里没有 tool call。

---

## 9. ToolsNode 后的 `buildReturnDirectly`

配置级和运行时直接返回最后都汇聚到 `buildReturnDirectly`。

它给图加了一个 lambda 节点：

```go
drLambda := compose.InvokableLambda(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
    s, ok := compose.GetState[reactState](ctx)
    if !ok || s.ReturnDirectlyToolCallID == "" {
        if len(input) > 0 {
            return input[len(input)-1], nil
        }
        return nil, fmt.Errorf("direct_return: no tool result found")
    }
    for _, msg := range input {
        if msg.ToolCallID == s.ReturnDirectlyToolCallID {
            return msg, nil
        }
    }
    if len(input) > 0 {
        return input[len(input)-1], nil
    }
    return nil, fmt.Errorf("direct_return: no matching tool result for call ID %s", s.ReturnDirectlyToolCallID)
})
```

然后给 ToolsNode 后面加两条可能的边：

```text
tools -> chat_model
tools -> direct_return -> END
```

再用 branch 判断：

```go
if !ok || s.ReturnDirectlyToolCallID == "" {
    return nodeKeyModel, nil
}
return nodeKeyDirectReturn, nil
```

这个设计让直接返回成为图结构的一部分，而不是 ToolsNode 内部偷偷提前结束。好处是清楚：

- ToolsNode 只负责执行工具。
- Branch 负责决定下一跳。
- direct_return lambda 负责从多个 tool result 中挑出目标结果。
- END 仍然是统一的图结束节点。

### 9.1 配置级直接返回

配置级直接返回来自：

```go
ToolReturnDirectly map[string]bool
```

如果配置里有：

```go
ToolReturnDirectly: map[string]bool{
    "search": true,
}
```

那么任意名为 `search` 的 tool call 都会被标记为可直接返回。测试 `TestReAct_ReturnDirectly_Config` 验证了这个场景：模型第一次输出 search tool call，工具执行后 agent 直接返回 tool result，不再调用第二次模型。

### 9.2 运行时直接返回

运行时直接返回来自工具内部调用：

```go
func SetReturnDirectly(ctx context.Context) error {
    callID := compose.GetToolCallID(ctx)
    return compose.ProcessState[reactState](ctx, func(ctx context.Context, s *reactState) error {
        s.ReturnDirectlyToolCallID = callID
        return nil
    })
}
```

这里有两个跨章知识点。

第一，`compose.GetToolCallID(ctx)` 是 ToolsNode 在执行每个工具前写入 context 的。没有 Chapter 05 的 ToolsNode 调用上下文，这里拿不到当前 tool call ID。

第二，`compose.ProcessState` 是 Chapter 03 的 graph local state 访问方式。工具不是 graph 节点本身，但工具执行时处在 graph run context 内，因此可以修改当前 ReAct 的 local state。

测试 `TestReAct_ReturnDirectly_Runtime` 验证的是：工具在运行时决定"我的结果就是最终答案"，调用 `SetReturnDirectly` 后，ToolsNode 结束分支会走 `direct_return`。

### 9.3 多工具调用下为什么匹配 ID

一个 assistant message 可以包含多个 tool calls。复刻测试 `TestReAct_ToolCallWithMultipleTools` 就覆盖了单轮多个工具调用。

ToolsNode 会返回多个 tool result message。direct_return 不能简单返回第一个或最后一个，因为配置级或运行时标记可能对应其中任意一个 tool call。用 `ToolCallID` 匹配是唯一可靠的方式。

---

## 10. StreamToolCallChecker：Provider 差异为什么必须显式化

非流式调用中，判断是否有 tool call 很简单：

```go
len(msg.ToolCalls) > 0
```

流式调用中，模型输出是一串 chunk。问题是不同 provider 的 chunk 顺序不同。

### 10.1 默认 checker：适合 OpenAI 风格

`DefaultStreamToolCallChecker` 的策略是：

```text
循环读取 chunk
  nil 或空 chunk: continue
  有 ToolCalls: return true
  有 Content: return false
EOF: return false
```

它假设第一个有效 chunk 足够代表模型意图：

```go
if len(msg.ToolCalls) > 0 {
    return true, nil
}
if len(msg.Content) > 0 {
    return false, nil
}
```

这适合 OpenAI 风格：如果模型要调工具，tool call 很早出现；如果先出现正常文本，就可以认为本轮不调工具。

### 10.2 ScanAll checker：适合先文本后 tool 的 provider

`ScanAllStreamToolCallChecker` 更保守：

```go
for {
    msg, err := sr.Recv()
    if err == io.EOF {
        return false, nil
    }
    if err != nil {
        return false, err
    }
    if msg != nil && len(msg.ToolCalls) > 0 {
        return true, nil
    }
}
```

它会扫完整个 stream，只要任意 chunk 出现 tool call 就返回 true。

这适合 Claude 风格：可能先输出一些文本或 reasoning，再输出 tool use。如果用默认 checker，看到第一段文本就返回 false，Agent 会误以为没有 tool call，从而提前结束。

### 10.3 复刻版的教学边界

这里必须讲清楚复刻版和完整原版的差异。

复刻版提供了 checker 类型和两个实现，也有相应测试：

- `TestReAct_StreamToolCallChecker_Default`
- `TestReAct_StreamToolCallChecker_ClaudeStyle`
- `TestReAct_StreamToolCallChecker_ScanAllNoToolCall`

但当前 `NewAgent` 的主图分支 `modelPostBranchCondition` 是基于完整 `*Message` 判断 `ToolCalls`，没有把 stream checker 接入图分支。也就是说，checker 在复刻版里承担的是"抽象与 provider 差异的教学展示"，不是完整 streaming ReAct runtime。

讲课时可以这样说：

> 在生产级 Eino 中，stream branch 必须消费 stream 并用 checker 决定下一跳；在复刻版里，我们把这一控制点独立保留下来，用测试解释为什么它必须可插拔。

这样既不会夸大复刻版能力，也能保留教学重点。

---

## 11. ReAct 的测试应该怎么读

`agent/react_test.go` 是本章最好的行为规格。建议按四组读。

### 11.1 基础循环

`TestReAct_NoTools_ReturnsModelOutput`：

- 模型直接返回普通 assistant message。
- 没有 tool call。
- 分支走 END。

这证明 ReAct agent 可以退化成普通 ChatModel wrapper。

`TestReAct_SingleToolCall`：

- 第一轮模型输出 search tool call。
- ToolsNode 执行 search。
- 第二轮模型看到工具结果后输出最终答案。
- 分支走 END。

这是最标准的一轮 ReAct。

`TestReAct_MultiRoundToolCall`：

- 第一轮 search。
- 第二轮 calc。
- 第三轮最终回答。

这证明 `tools -> chat_model` 的循环边和 state history 都在工作。

### 11.2 安全阀和直接返回

`TestReAct_MaxStepEnforced`：

- 模型永远输出 tool call。
- 图超过最大步数后返回 `compose.ErrExceedMaxSteps`。

`TestReAct_ReturnDirectly_Config`：

- `ToolReturnDirectly["search"] = true`。
- search 工具结果直接作为最终输出。
- 后续模型响应不会被消费。

`TestReAct_ReturnDirectly_Runtime`：

- 工具内部调用 `SetReturnDirectly(ctx)`。
- 运行时设置当前 tool call ID。
- Tools 后分支走 direct_return。

### 11.3 消息改写

`TestReAct_MessageRewriter_Compression`：

- rewriter 持久压缩 history。
- 后续模型看到压缩后的消息。

`TestReAct_MessageRewriter_Ordering`：

- rewriter 先执行。
- modifier 后执行。
- modifier 可以看到 rewriter 插入或改写后的消息。

这组测试是理解 `modelPreHandle` 的关键。

### 11.4 Provider checker

`TestReAct_StreamToolCallChecker_Default`：

- 空 chunk 后出现 tool call。
- 默认 checker 返回 true。

`TestReAct_StreamToolCallChecker_ClaudeStyle`：

- 先文本，后 tool call。
- `ScanAllStreamToolCallChecker` 返回 true。

这组测试强调：streaming tool call detection 不能写死成一种 provider 假设。

---

## 12. MultiAgent：Host 把 specialist 当作工具

ReAct 解决的是"一个模型如何循环调用工具"。MultiAgent 解决的是"一个 Host 如何把任务分派给多个 specialist"。

教学上可以先画逻辑拓扑：

```text
START -> Host(ChatModel)
Host --(no tool call)--> END
Host --(has tool calls)--> SpecialistExecutor
SpecialistExecutor --single intent--> END
SpecialistExecutor --multi intent--> Summarize -> END
```

但复刻代码的真实实现更简单：`NewMultiAgent` 创建一张 Graph，里面只有一个核心 lambda 节点：

```go
g := NewGraph[[]*Message, *Message]()

agentLambda := InvokableLambda(func(ctx context.Context, msgs []*Message) (*Message, error) {
    return executeMultiAgent(ctx, config, msgs)
})

g.AddLambdaNode("multi_agent_core", agentLambda)
g.AddEdge(START, "multi_agent_core")
g.AddEdge("multi_agent_core", END)
```

也就是说，复刻版 MultiAgent 的"Host -> Specialist -> Summarize"是 lambda 内部的逻辑流程，不是像 ReAct 那样展开成多个 Graph 节点。

这不是 bug，而是教学简化。它让读者聚焦 MultiAgent 的语义：

- Host 如何决定调用谁。
- Specialist 收到什么输入。
- 单意图和多意图如何返回。
- Specialist 有哪些实现形式。

---

## 13. `Specialist` 和 `MultiAgentConfig`

`compose/multiagent.go` 中的 specialist 定义是：

```go
type Specialist struct {
    Name         string
    IntendedUse  string
    ChatModel    ChatModel
    SystemPrompt string
    Invokable    func(ctx context.Context, input []*Message) (*Message, error)
    Streamable   func(ctx context.Context, input []*Message) (StreamReader[*Message], error)
}
```

几个字段的含义：

- `Name`：Host tool call 里的函数名必须匹配这个名字。
- `IntendedUse`：描述 specialist 适合处理什么任务；完整版本中它会成为 Host 可见的 tool description。
- `ChatModel`：最简单的 specialist 形式。
- `SystemPrompt`：仅在 ChatModel specialist 或 summarizer 中作为额外 system message 使用。
- `Invokable`：任意同步调用能力，可以包装一个 ReAct agent 的 `Generate`。
- `Streamable`：任意流式调用能力。

配置定义：

```go
type MultiAgentConfig struct {
    Host        ChatModel
    Specialists []*Specialist
    Summarizer  *Summarizer
    MaxStep     int
}
```

`validateMultiAgentConfig` 会检查：

- config 不能 nil。
- Host 不能 nil。
- Specialists 至少一个。
- specialist 不能 nil。
- specialist name 不能为空。
- specialist name 不能重复。

这些验证看似普通，但对 MultiAgent 非常重要：Host 输出 tool call 时只带函数名，如果 name 重复或为空，路由语义就不确定。

---

## 14. `executeMultiAgent`：MultiAgent 的主流程

核心流程是：

```go
func executeMultiAgent(ctx context.Context, config *MultiAgentConfig, msgs []*Message) (*Message, error) {
    hostMsg, err := config.Host.Generate(ctx, msgs)
    if err != nil {
        return nil, fmt.Errorf("host model error: %w", err)
    }

    if len(hostMsg.ToolCalls) == 0 {
        return hostMsg, nil
    }

    specSet := buildSpecialistSet(config.Specialists)

    answers := make([]specialistAnswer, 0, len(hostMsg.ToolCalls))
    for _, tc := range hostMsg.ToolCalls {
        spec, ok := specSet[tc.Function.Name]
        if !ok {
            return nil, fmt.Errorf("no specialist registered for tool name %q", tc.Function.Name)
        }

        answer, err := invokeSpecialist(ctx, spec, msgs)
        if err != nil {
            return nil, fmt.Errorf("specialist %q error: %w", spec.Name, err)
        }
        answers = append(answers, specialistAnswer{name: spec.Name, content: answer})
    }

    if len(answers) == 1 {
        return &Message{Role: Assistant, Content: answers[0].content}, nil
    }

    return summarizeAnswers(ctx, config.Summarizer, answers)
}
```

可以拆成五步：

1. Host 先看完整用户消息。
2. Host 如果不发 tool call，就直接回答。
3. Host 如果发 tool call，就按 tool call function name 找 specialist。
4. 每个 specialist 都用原始消息历史调用。
5. 一个答案直接返回，多个答案进入 summary。

这里最重要的语义是第 4 步：**specialist 收到的是 `msgs`，不是 tool call arguments。**

---

## 15. Specialist 为什么收到完整消息历史

很多人会误以为 Host 输出 tool call 后，specialist 应该收到 tool call 的参数：

```json
{
  "reason": "need finance analysis",
  "query": "compare AAPL and MSFT"
}
```

但复刻版不是这样。调用 specialist 时传的是：

```go
answer, err := invokeSpecialist(ctx, spec, msgs)
```

`msgs` 是用户原始消息历史。`invokeSpecialist` 里也明确写了：

```go
func invokeSpecialist(ctx context.Context, spec *Specialist, originalMsgs []*Message) (string, error) {
    input := originalMsgs
    ...
}
```

为什么这么设计？

因为 Host 的 tool call 在这里主要表达"应该找哪个专家"，不是表达完整任务载荷。Specialist 如果只收到 Host 编出来的一小段 tool 参数，容易丢掉用户原始上下文：

- 用户原文的约束条件。
- 多轮对话里的前提。
- 文件、表格、代码片段等上下文。
- 用户要求的输出格式和语言。

测试 `TestMultiAgent_PreHandler_InputReplacement` 就是在验证这一点：specialist 看到的是原始用户 history，而不是 Host tool call args。

这也是 MultiAgent 和普通工具调用的最大语义差别。普通工具通常只需要结构化参数；specialist 更像另一个 agent，需要完整上下文。

---

## 16. Specialist 的三种能力形式

`invokeSpecialist` 的优先级是：

1. `ChatModel`
2. `Invokable`
3. `Streamable`

### 16.1 ChatModel specialist

如果 specialist 有 ChatModel：

```go
if spec.ChatModel != nil {
    if spec.SystemPrompt != "" {
        input = append([]*Message{SystemMessage(spec.SystemPrompt)}, originalMsgs...)
    }
    msg, err := spec.ChatModel.Generate(ctx, input)
    return msg.Content, nil
}
```

这适合简单专家：比如翻译专家、代码审查专家、法律条款解释专家。SystemPrompt 会被插到原始消息前面，只影响这次 specialist 调用。

测试 `TestMultiAgent_SpecialistWithSystemPrompt` 验证了 system prompt 会被 prepend。

### 16.2 Invokable specialist

如果没有 ChatModel，但有 Invokable：

```go
if spec.Invokable != nil {
    msg, err := spec.Invokable(ctx, input)
    return msg.Content, nil
}
```

这适合把任意能力包装成 specialist：

- 一个 ReAct agent 的 `Generate`。
- 一个工作流 Runnable 的 `Invoke`。
- 一个业务函数。
- 一个远程服务调用。

测试 `TestMultiAgent_AgentAsSpecialist` 用 invokable 模拟 agent-like specialist。这里的教学重点是：specialist 不必须是 ChatModel，它可以是任何符合签名的能力。

### 16.3 Streamable specialist

如果没有 ChatModel 和 Invokable，但有 Streamable：

```go
if spec.Streamable != nil {
    sr, err := spec.Streamable(ctx, input)
    msgs, err := chatMessageStreamCollect(sr)
    if len(msgs) == 0 {
        return "", fmt.Errorf("specialist %q streamable returned no messages", spec.Name)
    }
    return msgs[len(msgs)-1].Content, nil
}
```

复刻版把 stream 收集成消息列表，然后取最后一条内容作为 specialist answer。这是简化策略。真实产品中可能需要更细的流式拼接、事件转发、callback 和部分输出处理。

### 16.4 优先级的含义

如果一个 Specialist 同时设置了 ChatModel 和 Invokable，ChatModel 会先被使用。这是代码顺序决定的。

教学上要提醒：不要在同一个 specialist 上同时设置多个能力，除非你明确知道优先级。更清晰的做法是每个 specialist 只设置一种能力。

---

## 17. 单意图、多意图和 Summarizer

Host 输出的 tool call 数量决定返回路径。

### 17.1 单意图：直接返回 specialist answer

如果只有一个 specialist answer：

```go
if len(answers) == 1 {
    return &Message{Role: Assistant, Content: answers[0].content}, nil
}
```

这里不再让 Host 总结。原因和 ReAct direct return 类似：单个 specialist 已经给出答案，再过一层模型可能增加成本、延迟和失真。

### 17.2 多意图：默认 summary

如果多个 specialist 被调用，默认总结方式是简单拼接：

```go
func defaultSummarize(answers []specialistAnswer) *Message {
    var parts []string
    for _, a := range answers {
        parts = append(parts, fmt.Sprintf("[%s]: %s", a.name, a.content))
    }
    return &Message{
        Role:    Assistant,
        Content: strings.Join(parts, "\n\n"),
    }
}
```

默认 summary 不会重新解释内容，只是把多个专家结果按名字列出来。这很适合复刻版：确定性强、容易测试、不会引入额外模型行为。

### 17.3 自定义 Summarizer

如果配置了：

```go
type Summarizer struct {
    ChatModel    ChatModel
    SystemPrompt string
}
```

就走 `customSummarize`：

```go
combined := strings.Join(parts, "\n---\n")
msgs := []*Message{UserMessage(combined)}
if summarizer.SystemPrompt != "" {
    msgs = append([]*Message{SystemMessage(summarizer.SystemPrompt)}, msgs...)
}
return summarizer.ChatModel.Generate(ctx, msgs)
```

输入格式是：

```text
Expert finance says: ...
---
Expert legal says: ...
```

再交给 summarizer ChatModel 产出最终回答。

这说明 MultiAgent 里至少有两类模型角色：

- Host：决定找谁。
- Summarizer：决定如何整合多个 specialist 的回答。

Specialist 自己也可以是模型或 agent。

---

## 18. MultiAgent 测试应该怎么读

`compose/multiagent_test.go` 是 MultiAgent 的行为规格。建议按五组读。

### 18.1 Host 直接回答

`TestMultiAgent_NoSpecialistCall_DirectHostAnswer`：

- Host 没有输出 tool call。
- MultiAgent 返回 Host 的 message。

这证明 MultiAgent 不强制调用 specialist。

### 18.2 单 specialist

`TestMultiAgent_SingleSpecialistSingleIntent`：

- Host 输出一个 tool call。
- 路由到对应 specialist。
- 返回 specialist answer。

这是最小 MultiAgent 路由。

### 18.3 多 specialist

`TestMultiAgent_MultiSpecialistMultiIntent`：

- Host 输出多个 tool calls。
- 每个 specialist 都被调用。
- 默认 summary 包含 specialist 名字和答案。

这证明多意图会聚合。

### 18.4 原始消息入参

`TestMultiAgent_PreHandler_InputReplacement`：

- specialist 检查收到的 input。
- input 是用户原始消息历史。
- 不是 Host tool call 参数。

这是本章最重要的 MultiAgent 语义测试。

### 18.5 能力形式和错误处理

相关测试包括：

- ChatModel specialist。
- Invokable specialist。
- Streamable specialist。
- nil config。
- nil Host。
- 空 Specialists。
- duplicate specialist name。
- unknown specialist tool call。
- host error。
- specialist error。

这些测试说明 MultiAgent 的边界比 ReAct 简单，但输入验证和错误传播仍然要清楚。

---

## 19. ReAct 和 MultiAgent 的关系

ReAct 和 MultiAgent 不是竞争关系，而是两层组合方式。

ReAct 关注：

```text
一个模型如何循环调用工具
```

MultiAgent 关注：

```text
一个 Host 如何把任务分派给多个 specialist
```

specialist 可以是：

- 一个普通 ChatModel。
- 一个业务函数。
- 一个 Streamable。
- 一个完整的 ReAct agent。

所以可以得到这样的组合：

```text
User
  -> Host MultiAgent
      -> finance specialist (ReAct agent with market data tools)
      -> code specialist (ReAct agent with repo search tools)
      -> writing specialist (plain ChatModel)
  -> Summarizer
  -> Final Answer
```

这正是 Agent Flow 的核心价值：不是把所有能力塞进一个巨大 prompt，而是把不同能力包装成可组合单元。

---

## 20. 和前六章的交叉引用

Chapter 07 几乎没有新 runtime 能力，它是在复用前六章。

| 前置章节 | 本章落点 |
| --- | --- |
| Ch01 Graph / Pregel | ReAct agent 是 Graph；循环靠 `AnyPredecessor`；`MaxStep` 防无限循环 |
| Ch02 Branch | ChatModel 后分支决定 END 还是 Tools；Tools 后分支决定 loop 还是 direct_return |
| Ch03 Runnable / Stream / State | Agent 包装 Runnable；local state 保存 messages；`ProcessState` 支持工具修改 state |
| Ch04 Interrupt / Resume | Agent 作为子图时仍应保持运行上下文隔离；完整原版会涉及更复杂 callback/address |
| Ch05 Components / Tool / Prompt | ChatModel、ToolsNode、Message、ToolCall、ToolInfo 是 Agent 的基本材料 |
| Ch06 Schema / Adapter | Provider 行为差异解释了为什么 stream checker 必须可插拔 |

如果学生听到本章觉得"东西突然变多"，通常是前置抽象还没有内化。可以反过来问：

- Branch 在哪里？
- State 在哪里？
- Runnable 在哪里？
- ToolsNode 在哪里？
- ChatModel 在哪里？

只要这些点能标出来，Agent 就不神秘。

---

## 21. 常见误解

### 21.1 "Agent 是特殊执行引擎"

不是。复刻版 ReAct agent 是 `compose.Graph[[]*Message, *Message]` 编译出来的 Runnable。

### 21.2 "MessageModifier 会修改长期历史"

不会。modifier 接收的是 `state.Messages` 的 slice copy。它适合临时注入当前轮上下文。需要持久改写时用 `MessageRewriter`。

### 21.3 "MessageRewriter 和 MessageModifier 可以合并"

不建议。它们表达的是不同持久性语义。合并之后，调用者无法判断自己的修改会不会污染后续轮次。

### 21.4 "MaxStep 是 ReAct 的终止条件"

不是。正常终止靠 ChatModel 输出无 tool call。MaxStep 是防止无限循环的保险丝。

### 21.5 "ToolReturnDirectly 直接让 ToolsNode 返回 END"

不是。ToolsNode 仍然只执行工具。直接返回是 ToolsNode 后面的 branch 和 `direct_return` lambda 完成的。

### 21.6 "运行时 SetReturnDirectly 不需要 tool call ID"

需要。`SetReturnDirectly` 通过 `compose.GetToolCallID(ctx)` 获取当前 tool call ID。没有 ID，就无法在多个 tool result 中确定应该返回哪一条。

### 21.7 "默认 StreamToolCallChecker 适合所有 provider"

不适合。默认 checker 是 first-effective-chunk heuristic。对先文本后 tool call 的 provider，要用扫描全流或 provider-specific checker。

### 21.8 "MultiAgent 的 specialist 入参是 tool call args"

不是。复刻版 specialist 收到完整原始消息历史。Host tool call 主要用于选择 specialist。

### 21.9 "复刻版 MultiAgent 是完整多节点 host graph"

不是。复刻版 `NewMultiAgent` 是单个 `multi_agent_core` lambda 节点，内部模拟 Host -> Specialist -> Summarize 逻辑。教学图是逻辑拓扑，不是当前代码的一比一节点图。

### 21.10 "Specialist 必须是 ChatModel"

不是。复刻版支持 ChatModel、Invokable、Streamable，优先级按代码顺序执行。

---

## 22. 课堂讲解脚本

如果只讲 12 分钟，可以按这个节奏。

### 第 0-2 分钟：从手写 loop 出发

先写出简单 for loop，然后指出四个问题：

- 终止条件由模型输出决定。
- 消息历史不能只靠节点输出传递。
- 工具结果可能要直接返回。
- streaming provider 的 tool call chunk 顺序不同。

目标是让学生接受：Agent Flow 不是一个 for 循环那么简单。

### 第 2-4 分钟：画 ReAct Graph

画出：

```text
START -> ChatModel -> END
              |
              v
            Tools -> ChatModel
              |
              v
        direct_return -> END
```

强调两个 branch：

- ChatModel 后：has tool call?
- Tools 后：return directly?

### 第 4-6 分钟：讲 State 和 `modelPreHandle`

展示 `reactState`：

```go
Messages []*Message
ReturnDirectlyToolCallID string
```

然后讲：

```text
append -> Rewriter -> copy -> Modifier -> ChatModel
```

用一句话总结：

> Rewriter 改长期历史，Modifier 改当前轮输入。

### 第 6-8 分钟：讲 direct return

配置级：

```go
ToolReturnDirectly["search"] = true
```

运行时：

```go
agent.SetReturnDirectly(ctx)
```

两者都写入 `ReturnDirectlyToolCallID`，最后由 `direct_return` lambda 挑出对应 tool result。

### 第 8-9 分钟：讲 stream checker

对比：

```text
OpenAI: tool call first -> default checker works
Claude: text first, tool call later -> scan-all checker needed
```

同时说明复刻版边界：checker 抽象和测试存在，主图分支仍是非流式简化。

### 第 9-12 分钟：讲 MultiAgent

画逻辑拓扑：

```text
Host -> Specialist(s) -> optional Summarizer
```

强调三件事：

- Host 用 tool call 选择 specialist。
- Specialist 收到完整原始消息历史，不是 tool call args。
- 复刻版实现是单 lambda，不是完整多节点图。

---

## 23. 实战阅读任务

给学生布置这组阅读任务，比单纯读文档更有效。

### 任务 1：标出 ReAct 图的边

打开 `agent/react.go`，标出以下边和分支：

- `START -> chat_model`
- `chat_model -> END`
- `chat_model -> tools`
- `chat_model` 上的 branch
- `tools -> chat_model`
- `tools -> direct_return`
- `direct_return -> END`
- `tools` 上的 branch

完成后回答：哪些边只是"可能路径"，哪些 branch 决定实际路径？

### 任务 2：追踪一次单工具调用

按 `TestReAct_SingleToolCall` 的行为手动追踪：

1. 用户消息进入 model pre-handler。
2. ChatModel 输出 search tool call。
3. model post branch 选择 Tools。
4. tools pre-handler 保存 assistant tool call。
5. ToolsNode 执行 search，返回 tool message。
6. tools branch 选择回到 ChatModel。
7. model pre-handler 把 tool message 追加到 history。
8. ChatModel 输出最终答案。
9. model post branch 选择 END。

### 任务 3：解释 Rewriter/Modifier 的差异

回答：

- 哪个会修改 `state.Messages`？
- 哪个只修改 copy？
- 为什么 modifier 适合 system prompt？
- 为什么 rewriter 适合历史压缩？

### 任务 4：设计一个 provider-specific checker

假设某 provider 会这样输出：

```text
chunk 1: reasoning text
chunk 2: normal text
chunk 3: tool call
chunk 4: tool call arguments delta
```

默认 checker 会返回什么？ScanAll checker 会返回什么？如果你要避免消费完整 stream 才判断，有什么 tradeoff？

### 任务 5：把 ReAct agent 当 specialist

设计一个 MultiAgent：

- Host 负责选择 `search_agent` 或 `math_agent`。
- `search_agent` 是一个 ReAct agent，内部有 search tool。
- `math_agent` 是一个 Invokable 函数。

回答：两个 specialist 收到的 input 是什么？Host 的 tool call args 会不会传进去？

---

## 24. 自测题

1. ReAct agent 在复刻版里为什么需要 `AnyPredecessor`？
2. `MaxStep` 和 "no tool call -> END" 哪个是正常终止机制？
3. `modelPreHandle` 的四步顺序是什么？
4. `MessageRewriter` 和 `MessageModifier` 的持久性差异是什么？
5. `ToolReturnDirectly` 为什么保存 tool call ID，而不是工具名？
6. `SetReturnDirectly` 为什么只能在工具执行上下文里可靠工作？
7. 默认 `DefaultStreamToolCallChecker` 对 Claude 风格输出可能出现什么误判？
8. 复刻版 `StreamToolCallChecker` 的实现边界是什么？
9. MultiAgent 中 Host 没有输出 tool call 时会发生什么？
10. MultiAgent specialist 收到的是 tool call arguments 还是原始消息历史？
11. Specialist 同时设置 ChatModel 和 Invokable 时，谁优先？
12. 多个 specialist answer 默认如何聚合？
13. 复刻版 MultiAgent 的逻辑拓扑和实际 Graph 节点结构有什么差异？
14. 如果 specialist name 重复，为什么必须报错？
15. ReAct 和 MultiAgent 如何组合？

参考答案：

1. 因为 `chat_model` 既可以由 START 触发，也可以由 tools 循环触发，循环图不能等待所有前驱。
2. "no tool call -> END" 是正常终止，`MaxStep` 是保险丝。
3. 追加输入、Rewriter 改 state、copy、Modifier 改 copy。
4. Rewriter 持久写回 state；Modifier 只影响当前轮传给 ChatModel 的 copy。
5. 同一轮可能有多个 tool call，甚至同名工具多次调用，必须用 ID 精确匹配结果。
6. 因为 ToolsNode 执行工具前把当前 tool call ID 写入 context，工具外通常没有这个上下文。
7. 先看到文本就返回 false，导致后续 tool call 被忽略。
8. 复刻版提供 checker 抽象和测试，但主图分支仍基于完整 message 的 `ToolCalls`。
9. 直接返回 Host 的回答。
10. 原始消息历史。
11. ChatModel 优先。
12. 默认按 `[name]: content` 拼接；配置 Summarizer 后用 summarizer ChatModel。
13. 逻辑上是 Host -> Specialist -> Summarize；代码上是单个 `multi_agent_core` lambda。
14. Host tool call 通过 function name 路由，重复 name 会导致路由不确定。
15. MultiAgent 的 specialist 可以是 ReAct agent 的 `Generate` 或 `Stream` 包装。

---

## 25. 本章一句话总结

ReAct 和 MultiAgent 都不是神秘的新 runtime：ReAct 是用 Graph、Branch、ToolsNode 和 local state 搭出的循环决策图；MultiAgent 是让 Host 用 tool call 选择 specialist，并把完整用户上下文交给 specialist 处理的组合模式。掌握本章之后，Agent 就从"会自己思考的黑盒"变成了"可以逐条边、逐个状态字段解释的图程序"。

