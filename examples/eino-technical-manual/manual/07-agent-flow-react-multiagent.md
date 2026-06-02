# Chapter 7: Agent Flow — ReAct / Multi-Agent Host

## 1. 面临的问题

Eino 的 `compose.Graph` 提供了底层图编排能力（chapter 1），但 LLM 应用中最常见的 pattern——agent——还需要一层更高层的封装。agent 的核心问题可以描述为：**让模型自主决定是否调用工具、调用哪个工具、工具结果如何反馈给模型继续思考，直到模型给出最终答案**。

具体来说，agent flow 需要解决以下问题：

1. **Model ↔ Tools 循环**。模型输出 tool call → 执行工具 → 工具结果回到模型 → 模型可能再次 tool call → …… → 最终输出纯文本答案。这个循环的终止条件不是预设步数，而是模型"自己觉得不需要再调工具了"。
2. **Streaming 下 tool call 检测**。不同 provider 的 streaming 行为不同：OpenAI 在第一块 stream chunk 中就给出 tool call，Claude 可能先输出文本再给 tool call。默认的 `firstChunkStreamToolCallChecker` 对 Claude 完全不可用。
3. **工具返回直接终止**。有些工具的输出本身就是最终答案（如搜索工具返回了精确结果），agent 应该立即返回而非继续循环。
4. **多 agent 协作**。当任务需要多个专家分工时，需要一个 "host" agent 将用户意图路由到合适的 specialist，并在多意图场景下聚合结果。
5. **Agent 作为嵌套子图**。同一个 ReAct agent 可能被直接调用，也可能作为另一个更大 graph 的子节点。callback、message future 等横切关注点必须在这两种场景下都正确工作。

## 2. 为什么这么难

### 2.1 Provider 行为差异

ReAct 的关键控制点是"当前轮模型输出是否包含 tool call"。在 invoke 模式（非流式）下，这个判断是 trivial 的——直接检查返回的 `*schema.Message` 的 `ToolCalls` 字段即可。但在 streaming 模式下，消息是分块到达的，不同模型提供商采用了完全不同的输出策略：

- **OpenAI 风格**：第一个 chunk 就携带 `ToolCalls` 字段（可能 delta 形式）。
- **Claude 风格**：先输出文本内容（`text_delta`），然后输出 `tool_use` block。也就是说，`Content` 非空之后才出现 `ToolCalls`。
- **Gemini 风格**：可能将 tool call 和文本交替输出。

Eino 在 `react/react.go:218-240` 的 `firstChunkStreamToolCallChecker` 采用启发式策略：读取 stream chunk，看到空 chunk（`len(msg.Content) == 0`）就 continue，看到 tool call 就 return true，看到非空 Content 就 return false。这对 OpenAI 有效，对 Claude 会错误地将"先有文本、后有 tool call"的流判为无 tool call，导致循环提前终止。

### 2.2 状态累计与修改的并发安全

ReAct 循环的每一步都需要把新消息追加到 message history 中。如果直接把这些消息拼成一个 `[]*schema.Message` 在节点间传递，会出现两个问题：

1. **无法在 pre-handler 中修改历史**。MessageRewriter / MessageModifier 需要访问完整历史才能做压缩或改写，但标准 graph 的节点输入只是前驱节点的输出。
2. **多个版本的 state 可能冲突**。如果 agent 作为子图被嵌入（例如一个 tool 内部又调用了另一个 agent），子 agent 的 message history 不应该污染父 agent 的 state。

### 2.3 嵌套 Graph 的 Callback 隔离

`WithMessageFuture`（`react/option.go:151`）通过 graph callback 收集 agent 执行过程中的所有中间消息。当 agent 直接使用时，callback 触发是明确的。但当 agent 作为子图嵌入另一个 graph 时：

- 外层 graph 可能也有自己的 callback。
- 同一个 graph name（如默认的 `"ReActAgent"`）可能在同一个父图中被多个子 agent 实例共享。

Eino 通过地址（`compose.Address`）机制解决：`cbHandler` 在第一次 `onGraphStart` 时记录自己的完整 graph address（`react/option.go:278-285`），之后所有 callback 都通过 `isOwnGraph`（`react/option.go:267-272`）比对当前地址与记录的地址是否一致，不一致则直接返回，不做任何处理。

### 2.4 Multi-Agent 路由的正确性

Host multi-agent 将 specialist agent 包装成 tools。Host model 通过 tool calling 选择 specialist。这里有一个微妙的正确性问题：

- Specialist agent 的入参不是 tool call argument，而是完整的用户 message history。所以当 host 决定 route 到一个 specialist 时，实际传给 specialist 的不是 `{"reason": "..."}`，而是 `state.msgs`。
- 这个"入参替换"通过 pre-handler 实现（`host/compose.go:160-162`）：`return state.msgs, nil`，直接丢弃 tool call message，代之以 graph state 中保存的原始用户消息。

## 3. 设计思路

### 3.1 ReAct Agent 就是一张 Graph

Eino 的核心设计哲学：**ReAct agent 不是一个特殊 runtime，它就是一个 `compose.Graph[[]*schema.Message, *schema.Message]`**。

```
START → ChatModel
ChatModel ──(has tool call)──→ Tools → ChatModel
ChatModel ──(no tool call)───→ END
```

从 `NewAgent`（`react/react.go:284`）的代码可以清晰看到 graph 构建过程：

1. `graph.AddChatModelNode(nodeKeyModel, chatModel, compose.WithStatePreHandler(modelPreHandle))` — 添加 model 节点，附 pre-handler。
2. `graph.AddEdge(compose.START, nodeKeyModel)` — START → ChatModel。
3. `graph.AddToolsNode(nodeKeyTools, toolsNode, compose.WithStatePreHandler(toolsNodePreHandle))` — 添加 tools 节点。
4. `graph.AddBranch(nodeKeyModel, compose.NewStreamGraphBranch(...))` — ChatModel 的分支：有 tool call 走 Tools，否则走 END。
5. `buildReturnDirectly(graph)` — 在 Tools 节点后插入 direct_return 分支和 lambda。
6. `graph.Compile(ctx, compileOpts...)` — 编译为标准 Runnable。

由于 ReAct agent 输出的是 `compose.Runnable`，它可以被 `ExportGraph()` 导出并嵌入到更大的 graph 中（`react/react.go:490-492`）。

### 3.2 Graph Local State 保存 Message History

ReAct 循环中的 message 累计不是靠节点输出拼接，而是靠 graph 的 **local state**（`react/react.go:56-59`）：

```go
type state struct {
    Messages                 []*schema.Message
    ReturnDirectlyToolCallID string
}
```

Graph 通过 `compose.WithGenLocalState(func(ctx context.Context) *state { ... })` 在每个运行上下文中创建一个新的 state 实例（`react/react.go:329-331`）。State 中 `Messages` 的追加逻辑全部在 pre-handler 中完成：

- **`modelPreHandle`**（`react/react.go:333-347`）：将当前轮输入 messages 追加到 state，执行 `MessageRewriter`（如果有），再执行 `MessageModifier`（如果有），最后返回处理过的 messages 给 ChatModel。
- **`toolsNodePreHandle`**（`react/react.go:357-364`）：将 model 的输出 message（包含 tool calls）追加到 state，判断是否有 tool 需要直接返回。

注意 `MessageRewriter` 和 `MessageModifier` 的区别（`react/react.go:151-156`）：
- `MessageRewriter`：修改 state 中的 `state.Messages`，修改会持久化到 state，影响后续所有轮次。
- `MessageModifier`：接收 state 中 messages 的一份 **copy**（`react/react.go:344-346`），修改 copy 后返回。不影响 state 中的 messages。适合在每轮调用前临时添加 system prompt 等。

### 3.3 两种 Return Directly 机制

Eino 提供了两种让 agent 在 tool execution 后立即返回的机制：

1. **配置级 `ToolReturnDirectly`**（`react/react.go:164`）：在 `AgentConfig` 中静态声明哪些 tool 的调用应该导致立即返回。在 `toolsNodePreHandle` 中通过 `getReturnDirectlyToolCallID`（`react/react.go:465-477`）匹配 tool name，将匹配到的 tool call ID 写入 `state.ReturnDirectlyToolCallID`。

2. **运行时 `SetReturnDirectly`**（`react/react.go:254-258`）：工具在执行过程中动态决定"我的结果就是最终答案"。通过 `compose.ProcessState` 修改 state 中的 `ReturnDirectlyToolCallID`。这个函数具有比配置级更高的优先级（`public.go:252-253` 注释明确说明）。

两种机制都通过 `buildReturnDirectly`（`react/react.go:399-449`）实现：在 Tools 节点后添加一个 branch（检查 `state.ReturnDirectlyToolCallID` 是否非空），如果非空则走到 `direct_return` lambda 节点，从 Tools 的输出中挑出匹配的 tool result message 作为最终输出直接走向 END。

### 3.4 Host Multi-Agent: 将 Specialist 当作 Tool

Host multi-agent 的设计思路是把"选择 specialist"当作一个 tool calling 问题：

1. 用户在 `MultiAgentConfig.Specialists` 中注册 specialist，每个 specialist 有 `Name` 和 `IntendedUse`。
2. `NewMultiAgent` 为每个 specialist 生成一个 `schema.ToolInfo`（`host/compose.go:89-98`），以 `Name` 为 tool name，以 `IntendedUse` 为 description。
3. Host chat model bind 这些 tool infos（`host/compose.go:107`），其 system prompt 引导它"decide which tool is best for the task and call only the best tool"（`host/compose.go:31`）。
4. Host 的输出通过 branch 分发：如果有 tool call，走 multi-branch 路由到相应 specialist(s)；如果没有 tool call，视为 host 直接回答，走 END。

Specialist 可以是三种形式之一（`host/types.go:159-164`）：
- `ChatModel`（纯模型，会附带 `SystemPrompt`）。
- `Invokable` / `Streamable`（可以是任意实现了对应接口的 fn，也可以是 `react.Agent` 的 `Generate` / `Stream` 方法）。
- 两者都有（`AnyLambda`）。

### 3.5 单意图 vs 多意图分支

Host multi-agent 有一个关键设计：区分"单意图"和"多意图"。

- **单意图**：host 只调用了一个 specialist。结果直接取该 specialist 的输出 message 返回（`host/compose.go:246-261`）。
- **多意图**：host 调用了多个 specialist。所有 specialist 的结果通过 passthrough 节点 `specialist_answers_collect` 收集，然后根据 `state.isMultipleIntents` 决定走 summarize 分支还是直接取单个答案（`host/compose.go:263-284`）。

Summarizer 在 `host/compose.go:286-335` 中实现：将 `map[string]any`（各 specialist 的输出）转为 `[]*schema.Message` 列表（`map2List`），然后如果有自定义 `Summarizer` 就用其 ChatModel 做总结；否则用默认的 lambda 简单拼接所有 message.Content。

## 4. 源码走读

### 4.1 `flow/agent/agent_option.go` — Agent 通用 Option 层

```
flow/agent/agent_option.go:22-28  AgentOption 结构体
flow/agent/agent_option.go:41-45  WithComposeOptions
flow/agent/agent_option.go:48-52  WrapImplSpecificOptFn
flow/agent/agent_option.go:55-71  GetImplSpecificOptions
```

`AgentOption` 是一个双通道 option 载体：
- `composeOptions`：透传给底层的 `compose.Option`，用于 graph 编译和 runnable 调用。
- `implSpecificOptFn`：通过泛型 `WrapImplSpecificOptFn[T]` 将实现特有的 option（如 `host.options`）注入到 option 管道中，在具体的 generate/stream 方法中通过 `GetImplSpecificOptions` 提取。

这种设计使得 agent 的 option 体系可以支持多态——ReAct agent 的 `WithTools`、host 的 `WithAgentCallbacks` 都通过这个管道传递各自需要的配置。

### 4.2 `flow/agent/react/react.go` — ReAct Agent 核心

```
flow/agent/react/react.go:56-59   state 结构体
flow/agent/react/react.go:136-190 AgentConfig 结构体
flow/agent/react/react.go:273-277 Agent 结构体
flow/agent/react/react.go:284-397 NewAgent — graph 构建主函数
flow/agent/react/react.go:333-347 modelPreHandle
flow/agent/react/react.go:357-364 toolsNodePreHandle
flow/agent/react/react.go:369-376 modelPostBranchCondition
flow/agent/react/react.go:399-449 buildReturnDirectly
flow/agent/react/react.go:254-258 SetReturnDirectly
flow/agent/react/react.go:465-477 getReturnDirectlyToolCallID
flow/agent/react/react.go:218-240 firstChunkStreamToolCallChecker
```

关键执行流程：

1. **`NewAgent`** 入口。首先通过 `agent.ChatModelWithTools`（`react/react.go:316`）将 tool infos 绑定到 chat model（如果提供了 `ToolCallingModel` 则优先使用；否则尝试将 `Model` 转为 `ToolCallingChatModel`）。
2. 创建一个新的 `compose.Graph`，通过 `WithGenLocalState` 注入 state 工厂。
3. 添加 ChatModel 节点和 Tools 节点，每个节点通过 `compose.WithStatePreHandler` 注册 pre-handler。
4. 添加 ChatModel → Tools/END 的 branch，其条件函数 `modelPostBranchCondition` 使用 `StreamToolCallChecker`（默认 `firstChunkStreamToolCallChecker`）判断是否包含 tool call。
5. `buildReturnDirectly`：在 Tools 节点后添加两层逻辑——先 branch 判断是否需要 direct return，如果是则走 `direct_return` lambda，否则回到 ChatModel。
6. 编译 graph，使用 `AnyPredecessor` 触发模式（适合循环图），设置 `MaxStep` 防止无限循环。

**`modelPreHandle` 细节**（`react/react.go:333-347`）：
```
state.Messages = append(state.Messages, input...)    // 追加本轮输入
→ MessageRewriter(ctx, state.Messages)                // 改写 state 中的消息
→ copy(modifiedInput, state.Messages)                 // 复制一份
→ messageModifier(ctx, modifiedInput)                 // 修改副本后返回给 ChatModel
```
这个设计保证了 `MessageRewriter` 的修改是持久的（影响后续轮次），而 `MessageModifier` 的修改是临时的（仅影响当前轮）。

**`SetReturnDirectly`**（`react/react.go:254-258`）：
```go
func SetReturnDirectly(ctx context.Context) error {
    return compose.ProcessState(ctx, func(ctx context.Context, s *state) error {
        s.ReturnDirectlyToolCallID = compose.GetToolCallID(ctx)
        return nil
    })
}
```
通过 `compose.GetToolCallID(ctx)` 获取当前工具调用的 call ID（这个 ID 由 ToolsNode 在执行前注入 context），写入 state。之后 `buildReturnDirectly` 中的 branch 检查 state 时就能匹配到对应的 tool result。

### 4.3 `flow/agent/react/option.go` — ReAct 运行时选项

```
flow/agent/react/option.go:32-35  WithToolOptions
flow/agent/react/option.go:37-40  WithChatModelOptions
flow/agent/react/option.go:92-107 WithTools
flow/agent/react/option.go:151-228 WithMessageFuture
```

**`WithTools`**（`react/option.go:92-107`）的关键价值在于它同时做两件事：
1. `model.WithTools(toolInfos)` — 让 chat model 知道有哪些工具可用（tool schema）。
2. `compose.WithToolList(tools...)` — 让 ToolsNode 知道如何执行这些工具（tool implementation）。

如果只用 `WithToolList`，model 不知道有没有工具、有哪些工具；如果只用 `model.WithTools`，ToolsNode 不知道如何执行，两个 side 必须同时配置。

**`WithMessageFuture`**（`react/option.go:151-228`）的实现细节：
- 创建 `cbHandler` 实例，包含 graph name 和消息 channel。
- 通过 `OnStartFn` / `OnStartWithStreamInputFn` 注册 graph callback：在 agent graph 启动时，将 `toolResultSenders` 注入 context（通过 `setToolResultSendersToCtx`），这些 sender 在 `newToolResultCollectorMiddleware`（`react/react.go:65-125`）中触发。
- 通过 model callback 的 `OnEnd` / `OnEndWithStreamOutput` 收集 ChatModel 的输出消息。
- `cbHandler.claimOwnership` / `isOwnGraph` 确保嵌套 graph 场景下不会发生消息泄漏。

Message future 支持四种 tool result 场景：普通 string result、streaming string result、enhanced result（多模态）、enhanced streaming result。

### 4.4 `flow/agent/react/callback.go` — 简化的 Callback Builder

```
flow/agent/react/callback.go:31-33 BuildAgentCallback
```

这是 `WithMessageFuture` 的简化版 helper，仅负责将 user 提供的 `ModelCallbackHandler` 和 `ToolCallbackHandler` 合并成一个 `callbacks.Handler`。内部调用顺序：ChatModel callback → Tool callback（`template.NewHandlerHelper().ChatModel(modelHandler).Tool(toolHandler).Handler()`）。

### 4.5 `flow/agent/multiagent/host/types.go` — Host Multi-Agent 类型定义

```
flow/agent/multiagent/host/types.go:32-39  MultiAgent 结构体
flow/agent/multiagent/host/types.go:76-102 MultiAgentConfig
flow/agent/multiagent/host/types.go:131-134 AgentMeta
flow/agent/multiagent/host/types.go:148-156 Host
flow/agent/multiagent/host/types.go:165-173 Specialist
flow/agent/multiagent/host/types.go:177-180 Summarizer
flow/agent/multiagent/host/types.go:104-128 validate()
```

核心类型的含义：
- `MultiAgent`：与 React `Agent` 同构——包装一个 `Runnable`、一个 `Graph`、一组 `GraphAddNodeOpt`，同样支持 `Generate` / `Stream` / `ExportGraph`。
- `Specialist`：可以是 `ChatModel`（简单模型）、`Invokable`（如 `react.Agent.Generate`）、`Streamable`（如 `react.Agent.Stream`）。通过 `compose.AnyLambda` 统一包装为 lambda（`host/compose.go:156`）。
- `Host`：当前仅支持 `ChatModel`，system prompt 作为路由决策的引导语。
- `Summarizer`：多意图场景下的聚合器，有一个 `ChatModel` 和自定义 `SystemPrompt`。

### 4.6 `flow/agent/multiagent/host/compose.go` — Host Graph 构建

```
flow/agent/multiagent/host/compose.go:49-152  NewMultiAgent
flow/agent/multiagent/host/compose.go:154-185 addSpecialistAgent
flow/agent/multiagent/host/compose.go:187-203 addHostAgent
flow/agent/multiagent/host/compose.go:205-220 addDirectAnswerBranch
flow/agent/multiagent/host/compose.go:222-244 addMultiSpecialistsBranch
flow/agent/multiagent/host/compose.go:246-261 addSingleIntentAnswerNode
flow/agent/multiagent/host/compose.go:263-284 addAfterSpecialistsBranch
flow/agent/multiagent/host/compose.go:286-335 addMultiIntentsSummarizeNode
```

Graph 拓扑结构：

```
START → Host (ChatModel)
Host ──(direct answer, no tool call)──→ END
Host ──(has tool calls)──→ msg2MsgList (converter)
msg2MsgList ──(multi-branch)──→ Specialist_A, Specialist_B, ...
Specialist_A → SpecialistsAnswersCollector (passthrough)
Specialist_B → SpecialistsAnswersCollector (passthrough)
SpecialistsAnswersCollector ──(branch: single vs multi intent)──→
    ├── SingleIntentAnswer → END
    └── map_to_list → MultiIntentSummarize → END
```

关键实现点：

1. **`addSpecialistAgent`**（`host/compose.go:154-185`）：根据 specialist 的能力类型（ChatModel / Invokable-Streamable / both）采用不同的节点添加方式。`ChatModel` 模式通过 pre-handler 注入 `SystemPrompt`。所有 specialist 的输出都通过 `compose.WithOutputKey(specialist.Name)` 标记，以便后续在 passthrough 节点中通过 key 区分。

2. **`addDirectAnswerBranch`**（`host/compose.go:205-220`）：在 Host 节点后添加 branch。条件函数 `toolCallChecker` 判断 Host 的输出是否包含 tool call——包含则走到 `msg2MsgList`（后续路由到 specialist），否则直接到 END（Host 直接回答）。

3. **`addMultiSpecialistsBranch`**（`host/compose.go:222-244`）：`msg2MsgList` 节点将 Host 的单条 message 转为 list，然后通过 `NewGraphMultiBranch` 根据消息中的 tool calls 分发到各个 specialist。如果 tool calls 数量 > 1，设置 `state.isMultipleIntents = true`。

4. **`addAfterSpecialistsBranch`**（`host/compose.go:263-284`）：所有 specialist 输出到达 passthrough 后，检查 `state.isMultipleIntents` 决定后续路径。

5. **`addMultiIntentsSummarizeNode`**（`host/compose.go:286-335`）：如果提供了自定义 `Summarizer`，用其 ChatModel 做总结（pre-handler 注入 system prompt + state.msgs + 各 specialist 的输出）；否则用默认 lambda 简单拼接。

### 4.7 `flow/agent/multiagent/host/callback.go` — Host Callback

```
flow/agent/multiagent/host/callback.go:31-33  MultiAgentCallback 接口
flow/agent/multiagent/host/callback.go:36-39  HandOffInfo
flow/agent/multiagent/host/callback.go:42-89  ConvertCallbackHandlers
flow/agent/multiagent/host/callback.go:92-99  convertCallbacks
```

Host multi-agent 特有的 callback 概念是 `OnHandOff`——当 Host model 决定将任务交给某个 specialist 时触发。`ConvertCallbackHandlers` 将 `MultiAgentCallback` 注册为 ChatModel callback 的 `OnEnd` 处理函数——当 Host model 输出 assistant message（且包含 tool calls）时，遍历所有 tool calls，为每个 tool call 触发 `OnHandOff`。

`convertCallbacks` 通过 `GetImplSpecificOptions` 从 option 管道中提取 `multiagent.host.options`，将其 `agentCallbacks` 转换为 `callbacks.Handler` 并挂载到 Host 节点上（`host/types.go:47` 的 `DesignateNode(ma.HostNodeKey())`）。

## 5. 模式与示例

### 5.1 基础 ReAct Agent

```go
agent, err := react.NewAgent(ctx, &react.AgentConfig{
    ToolCallingModel: model,
    ToolsConfig: compose.ToolsNodeConfig{
        Tools: []tool.BaseTool{searchTool, calcTool},
    },
    MaxStep: 20,
})

msg, err := agent.Generate(ctx, []*schema.Message{
    {Role: schema.User, Content: "search for the latest Go version and calculate 2^10"},
})
```

### 5.2 带 MessageModifier 的 Agent

MessageModifier 用于在每次 model 调用前插入 system prompt，但不污染 state 中的 messages：

```go
agent, err := react.NewAgent(ctx, &react.AgentConfig{
    ToolCallingModel: model,
    ToolsConfig:      toolsConfig,
    MessageModifier: func(ctx context.Context, input []*schema.Message) []*schema.Message {
        return append([]*schema.Message{schema.SystemMessage(
            "You are an expert Go programmer. Always provide code examples.",
        )}, input...)
    },
})
```

### 5.3 带 MessageRewriter 的 Agent（压缩历史）

MessageRewriter 用于在每次 model 调用前压缩 state 中的 messages，防止上下文溢出：

```go
agent, err := react.NewAgent(ctx, &react.AgentConfig{
    ToolCallingModel: model,
    ToolsConfig:      toolsConfig,
    MessageRewriter: func(ctx context.Context, msgs []*schema.Message) []*schema.Message {
        if len(msgs) > 30 {
            // keep system message + last 20 messages
            return append(msgs[:1], msgs[len(msgs)-20:]...)
        }
        return msgs
    },
})
```

### 5.4 运行时 Tool Return Directly

在工具实现中使用 `react.SetReturnDirectly`：

```go
func mySearchTool(ctx context.Context, query string) (string, error) {
    result := doSearch(query)
    if isDefinitive(result) {
        react.SetReturnDirectly(ctx) // signal: stop here, this is the answer
    }
    return result, nil
}
```

### 5.5 Host Multi-Agent 基础用法

```go
hostAgent, err := host.NewMultiAgent(ctx, &host.MultiAgentConfig{
    Host: host.Host{
        ChatModel:    routingModel,
        SystemPrompt: "You are a router. Decide which expert to call.",
    },
    Specialists: []*host.Specialist{
        {
            AgentMeta:   host.AgentMeta{Name: "code_expert", IntendedUse: "answers programming questions"},
            ChatModel:   codeModel,
            SystemPrompt: "You are a programming expert.",
        },
        {
            AgentMeta:  host.AgentMeta{Name: "math_expert", IntendedUse: "solves math problems"},
            Streamable: mathAgent.Stream, // a react.Agent
        },
    },
    Summarizer: &host.Summarizer{
        ChatModel:    summaryModel,
        SystemPrompt: "Synthesize the following expert answers into one response.",
    },
})
```

### 5.6 自定义 StreamToolCallChecker（Claude）

```go
agent, err := react.NewAgent(ctx, &react.AgentConfig{
    ToolCallingModel: claudeModel,
    ToolsConfig:      toolsConfig,
    StreamToolCallChecker: func(ctx context.Context, sr *schema.StreamReader[*schema.Message]) (bool, error) {
        defer sr.Close()
        for {
            msg, err := sr.Recv()
            if err == io.EOF {
                return false, nil
            }
            if err != nil {
                return false, err
            }
            // Claude may output text before tool calls
            if len(msg.ToolCalls) > 0 {
                return true, nil
            }
        }
    },
})
```

### 5.7 使用 WithMessageFuture 获取中间消息

```go
opt, future := react.WithMessageFuture()
go func() {
    msgs := future.GetMessages()
    for {
        msg, ok, err := msgs.Next()
        if !ok {
            break
        }
        fmt.Printf("Intermediate message: %s\n", msg.Content)
    }
}()
result, err := agent.Generate(ctx, input, opt)
```

## 6. 常见陷阱

### 6.1 Claude 模型 + Stream 模式

**问题**：使用 Claude 模型调用 `agent.Stream()` 时，默认的 `firstChunkStreamToolCallChecker` 无法正确识别 tool call，agent 可能在模型输出文本后直接结束，而不执行工具。

**原因**：`firstChunkStreamToolCallChecker`（`react/react.go:218-240`）在遇到第一个非空 Content 时立即返回 false，而 Claude 的输出顺序是文本先于 tool call。

**解决**：提供自定义 `StreamToolCallChecker` 遍历完整个 stream（见 5.6）。

### 6.2 ToolReturnDirectly 与多人调用的竞争

**问题**：`AgentConfig` 中配置了 `ToolReturnDirectly`，希望某个 tool 执行后 agent 直接返回。但同时该 tool 可能被多个 tool call 并行调用，只有第一个匹配到的会生效（`react/react.go:163` 注释）。

**原因**：`getReturnDirectlyToolCallID` 遍历 `input.ToolCalls`，返回第一个匹配的 tool call ID。`buildReturnDirectly` 中的 `direct_return` lambda 也只用第一个匹配的 tool call ID 去查找结果。

### 6.3 WithTools 必须一次传给 Generate/Stream

**问题**：通过 `react.WithTools(ctx, tool1, tool2)` 生成的两个 option 必须同时传给 `agent.Generate()` 或 `agent.Stream()`。如果只传其中一个，要么 model 不知道有工具（不会产生 tool call），要么 ToolsNode 不知道如何执行工具（执行时报错）。

### 6.4 Host Multi-Agent 中 Specialist 的入参

**问题**：以为 host 会将 tool call argument（如 `{"reason": "..."}`）传给 specialist。

**实际**：Specialist 的 pre-handler（`host/compose.go:160-162`）完全丢弃 tool call input，替换为 `state.msgs`（即最初发给 host 的 messages）。这意味着 specialist 看到的输入是完整的用户消息历史，而不是 host 的路由参数。

### 6.5 嵌套 Graph 中 WithMessageFuture 的行为

**问题**：将 agent 作为子图嵌入另一个 graph 后，`WithMessageFuture` 是否仍能正确收集消息？

**答案**：可以。`cbHandler` 通过 address 机制（`react/option.go:259-285`）精确匹配自己的 graph 实例。即使多个 agent 实例同 graph name、同 nesting layer，每个 `cbHandler` 记录的是 `compose.Address`（完整路径），不会互相干扰。

### 6.6 AnyPredecessor 触发模式下的状态一致性

ReAct agent 的 graph 使用 `AnyPredecessor` 模式编译（`react/react.go:386`）。这意味着 Tools 节点完成后，ChatModel 节点立即被触发（不需要等待其他前驱）。这种模式下，`state.Messages` 是唯一的共享状态，pre-handler 中的追加操作必须保持正确顺序。Eino 通过 Pregel runtime 的 channel 机制保证单节点单次执行，不会出现并发问题。

## 7. 对 Rive 的启示

### 7.1 将领域 Pattern 编码为 Graph Builder

Eino 没有为 ReAct 实现特殊 runtime，而是将 ReAct 编码为一个 graph builder。这带来了几个好处：
- Graph 的所有基础设施（callback、streaming、checkpoint、interrupt/resume）自动继承。
- 可以嵌套：ReAct agent 可以作为另一个 graph 的子节点，也可以作为 multi-agent host 的 specialist。
- 可扩展：用户可以在 ReAct agent 的基础上添加自己的 node（通过 `ExportGraph` 获取底层 graph 后添加边和节点）。

Rive 在设计 agentic 能力时，应该考虑将"agent pattern"实现为 graph 的 builder/converter，而非独立 runtime。

### 7.2 Provider 差异应该暴露为扩展点

Eino 的 `StreamToolCallChecker` 是一个很好的设计示范——它不是把特定 provider 的行为 hardcode 到 agent 里，而是暴露一个可插拔的判断函数。默认值覆盖常见情况（OpenAI），但复杂情况（Claude、Gemini）可以通过自定义实现处理。

启示：在设计 protocol/pattern 时，为 provider-specific 行为预留回调/注入点，不要假设所有 provider 行为一致。

### 7.3 Option 管道的多态设计

`AgentOption` 的"双通道"设计（`composeOptions` + `implSpecificOptFn`）兼顾了通用性和特异性：
- 通用 option（如 callback、streaming config）通过 `composeOptions` 统一传递。
- 实现特有的 option（如 host 的 `agentCallbacks`）通过 `WrapImplSpecificOptFn[T]` 泛型传递。

这种设计避免了为每个 agent 实现定义完全独立的 option 类型，同时又不丢失类型安全。

### 7.4 嵌套场景的 Callback 隔离

`cbHandler` 通过 `compose.Address` 实现嵌套 graph 的 callback 隔离是一个精巧的模式：在第一次 graph start 时记录完整地址，后续所有 callback 都通过地址比对决定是否处理。这使得同一个 graph name 的多个实例可以在同一个父图中独立运行，互不干扰。

Rive 在设计 callback 和 observable 系统时，应考虑类似的地址隔离机制。

### 7.5 State Pre/Post Handler 模式

ReAct agent 通过 pre-handler（而非 post-handler 或中间件）管理 message history 累计，这是一个值得借鉴的模式：
- 数据修改逻辑集中在节点入口，而非分散在图的边或运行时。
- Graph state 和节点输入是解耦的——pre-handler 可以替换节点输入（例如用 state 中的 messages 替换工具调用的 tool call argument）。
- MessageRewriter（持久改写）和 MessageModifier（临时修改）通过是否 copy 来区分语义，简洁明确。
