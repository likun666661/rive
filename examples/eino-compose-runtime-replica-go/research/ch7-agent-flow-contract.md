# Chapter 7 Agent Flow Contract: ReAct / Multi-Agent Host

> **基于**：Eino 技术手册第 7 章 (`07-agent-flow-react-multiagent.md`) + 当前 Go 复刻版源码审计
> **目标读者**：实施工人（I1/I2/I3/I4）及后续验证者
> **定位**：教育子集契约 —— 定义 Go 复刻版中 Agent Flow 层的"要什么、为什么、不要什么"
> **语言**：中文

---

## 目录

1. [scope — 范围定义](#1-scope--范围定义)
2. [contract — 核心契约](#2-contract--核心契约)
    - 2.1 [问题域：ReAct Agent 解决了什么](#21-问题域react-agent-解决了什么)
    - 2.2 [Provider 差异与 StreamToolCallChecker](#22-provider-差异与-streamtoolcallchecker)
    - 2.3 [Local State 保存 Message History](#23-local-state-保存-message-history)
    - 2.4 [MessageRewriter vs MessageModifier 持久化语义](#24-messagerewriter-vs-messagemodifier-持久化语义)
    - 2.5 [嵌套 Graph Callback 的 Address 隔离](#25-嵌套-graph-callback-的-address-隔离)
    - 2.6 [Host Multi-Agent：Specialist 作为 Tool + 单/多意图分支](#26-host-multi-agentspecialist-作为-tool--单多意图分支)
3. [cuts — 明确排除的生产特性](#3-cuts--明确排除的生产特性)
4. [evidence — 证据](#4-evidence--证据)

---

## 1. scope — 范围定义

### 1.1 本章在教育复刻版中的位置

Chapter 7 位于 Eino 能力栈的**顶层**——它建立在 Chapter 1（Graph + GraphBranch + Pregel）、Chapter 2（Workflow/Chain/Parallel）、Chapter 3（Stream/Callback/Runnable 降级）、Chapter 4（Checkpoint/Interrupt/Resume）、Chapter 5（Model/Tool/Prompt/ToolsNode）和 Chapter 6（Schema/Concat/Provider Adapter）之上。

Agent Flow 层的本质是**将可复用的 LLM 应用 pattern 编码为 Graph Builder**，而非独立 runtime。

### 1.2 教育子集目标

在我们的教育复刻版中，Chapter 7 的目标是用 **最小可工作例程** 演示以下核心概念：

| 概念 | 在复刻版中的实现方式 | 优先级 |
|------|-------------------|--------|
| ReAct Agent 作为 Graph Builder | `NewAgent` 构建 `START → ChatModel → Tools → ChatModel → ... → END` 循环图 | CRITICAL |
| Local State 消息历史 | `compose.WithGenLocalState` 在 graph 上下文中管理 `[]*schema.Message` | CRITICAL |
| StreamToolCallChecker 可插拔 | 默认 checker + 自定义注入点 | HIGH |
| MessageRewriter / MessageModifier | Pre-handler 中的 copy-vs-in-place 语义 | HIGH |
| Tool Return Directly | 配置级 + 运行时 `SetReturnDirectly` | MEDIUM |
| Host Multi-Agent 路由 | Specialist 作为 ToolInfo → Host Model → Branch → Specialist → Collect/Summarize | MEDIUM |
| Address 隔离 | 复用已有的 `compose/address.go` 机制 | LOW (继承) |

### 1.3 不在教育子集范围内的完整生产特性

参见 [3. cuts](#3-cuts--明确排除的生产特性)。

---

## 2. contract — 核心契约

### 2.1 问题域：ReAct Agent 解决了什么

**核心问题**：让模型**自主决定**是否调用工具、调用哪个工具、工具结果如何反馈给模型继续思考、直到模型给出最终答案。

传统的"预设步数循环"（for i := 0; i < N; i++）无法解决这个问题，因为：

1. **终止条件不是步数**。Agent 循环的终止条件是"模型觉得不需要再调工具了"，这由模型的 output 决定 —— 当模型输出纯文本而非 tool call 时，循环结束。这个判断是内容驱动的，不是计数器驱动的。
2. **工具结果可能直接是最终答案**。某些工具（如精确搜索）的输出本身就足以回答用户，不需要再回到模型做一轮"总结"。Eino 通过 `ToolReturnDirectly` 和 `SetReturnDirectly` 支持这种短路路径。
3. **迭代轮次不可预测**。同样的 prompt 在不同模型上可能需要不同轮数，甚至在同一个模型的不同采样下也不同。

**Eino 的解法**：把 ReAct 实现为一个 `compose.Graph`，其拓扑为：

```
START → ChatModel
ChatModel ──(has tool call)──→ Tools → ChatModel
ChatModel ──(no tool call)───→ END
```

关键点：
- ReAct agent **不是一个特殊 runtime**，它就是一张图。所有 graph 的基础设施（Streaming、Callback、Checkpoint、Interrupt/Resume）自动继承。
- 循环通过 Pregel 运行时 + `AnyPredecessor` 触发模式实现（ChatModel 和 Tools 交替触发）。
- `MaxStep` 作为安全上限防止无限循环，但这只是最后一道防线，正常终止靠模型输出不含 tool call。

**复刻版需要实现**：
- `NewAgent(config)` 作为 graph builder 函数
- `AgentConfig` 结构体，包含 `ChatModel`、`ToolsConfig`、`MaxStep`、`MessageRewriter`、`MessageModifier`、`StreamToolCallChecker`
- `state` 结构体，通过 `WithGenLocalState` 在每个运行上下文中创建

### 2.2 Provider 差异与 StreamToolCallChecker

**为什么 Provider 行为差异是 Agent Flow 的核心难题**：

ReAct 的循环控制点是一个关键判断：**当前轮模型输出是否包含 tool call**。在 Invoke（非流式）模式下，这个判断是 trivial 的——直接检查返回的 `*schema.Message` 的 `ToolCalls` 字段。但在 Streaming 模式下，消息分块到达，不同模型提供商采用了完全不同的输出策略：

| Provider | Streaming 行为 | 对 Eino 的影响 |
|----------|---------------|----------------|
| **OpenAI** | 第一个 chunk 就携带 `ToolCalls`（delta 形式） | `firstChunkStreamToolCallChecker` 有效 |
| **Claude** | 先输出文本 `text_delta`，然后输出 `tool_use` block | 默认 checker 在第一个非空 Content 处返回 false，**漏判 tool call** |
| **Gemini** | 可能将 tool call 和文本交替输出 | 默认 checker 甚至可能漏判部分 tool call |

**Eino `firstChunkStreamToolCallChecker` 的逻辑**（参考 `react/react.go:218-240`）：
```
读取 stream chunk →
  空 chunk (len(msg.Content)==0) → continue
  有 ToolCalls → return true
  有非空 Content → return false   ← 对 Claude 致命
```

这个启发式策略对 OpenAI 工作正确。但对 Claude：
1. Chunk 1: `Content: "Sure, let me search for that."` → checker 返回 false → 循环终止
2. Chunk 2: `ToolCalls: [{name: "search", ...}]` → agent 已退出，tool call 被忽略

**Eino 的解法**：`StreamToolCallChecker` 是一个**可插拔的函数注入点**（`AgentConfig.StreamToolCallChecker`）。默认值覆盖 OpenAI 场景；Claude/Gemini 用户提供自定义实现（遍历完整个 stream 再判断）。

**复刻版需要实现**：
- `StreamToolCallChecker` 函数类型定义
- 默认 `firstChunkStreamToolCallChecker` 实现
- 在 `AgentConfig` 中暴露为可选字段

### 2.3 Local State 保存 Message History

**为什么消息历史不能作为节点间数据流传递**：

ReAct 循环的每一步都需要把新消息追加到 message history 中。如果直接把消息拼成一个 `[]*schema.Message` 在节点间传递，会产生两个严重问题：

1. **无法在 pre-handler 中修改历史**。MessageRewriter / MessageModifier 需要访问完整的 message history 才能做压缩或改写。但标准 graph 的节点输入只是**前驱节点的输出**（对于 ChatModel 节点是 `Tools → ChatModel` 的边输出），不包含完整历史。如果 ChatModel 只有一个前驱（Tools），它的输入只是 Tools 节点的输出，不是完整对话历史。

2. **嵌套场景下的 state 污染**。如果 agent 作为子图被嵌入（例如一个 tool 内部又调用了另一个 agent），子 agent 的 message history 必须与父 agent 隔离。每个 agent 实例需要自己的 state 作用域。

**Eino 的解法**：Graph **Local State**（`react/react.go:56-59`）：

```go
type state struct {
    Messages                 []*schema.Message
    ReturnDirectlyToolCallID string
}
```

通过 `compose.WithGenLocalState(func(ctx context.Context) *state { ... })` 在每个 graph 运行上下文中创建一个新的 state 实例。State 与 graph 实例生命周期绑定，不同 agent 实例的 state 完全隔离。

消息累计（append）全部在 **pre-handler** 中完成：
- `modelPreHandle`：追加本轮输入 → 执行 MessageRewriter → 执行 MessageModifier → 返回处理后的 messages 给 ChatModel
- `toolsNodePreHandle`：追加 model 的 tool call message → 判断 return directly

**复刻版需要实现**：
- ReAct agent 的 `state` 结构体
- 通过 `compose.WithGenLocalState` 注入 state 工厂
- `modelPreHandle` 和 `toolsNodePreHandle` 两个 pre-handler

### 2.4 MessageRewriter vs MessageModifier 持久化语义

**为什么需要两种不同的消息修改机制**：

两者都在 model 调用前修改消息，但**持久化语义不同**：

| 特性 | MessageRewriter | MessageModifier |
|------|----------------|-----------------|
| **作用对象** | `state.Messages`（直接修改） | `state.Messages` 的 **copy**（`react/react.go:344-346`） |
| **持久性** | 修改影响后续**所有轮次** | 修改仅影响**当前轮** |
| **典型用途** | 上下文压缩（删除旧消息、截断历史） | 注入 system prompt、添加格式化指令 |
| **执行顺序** | 先执行，修改 state | 后执行，修改 copy 后返回给 ChatModel |

**关键代码路径**（`react/react.go:333-347`）：

```
state.Messages = append(state.Messages, input...)    // 1. 追加本轮输入
→ MessageRewriter(ctx, state.Messages)                // 2. 改写 state 中的消息 ← 持久
→ copy(modifiedInput, state.Messages)                 // 3. 复制一份
→ messageModifier(ctx, modifiedInput)                 // 4. 修改副本后返回 ← 临时
→ 返回 modifiedInput 给 ChatModel                     // 5. ChatModel 看到修改后的消息
```

**设计意图**：
- `MessageRewriter` 是"真正的改写"：上下文压缩需要从 state 中**删除**消息，否则 state 无限增长会 OOM。因此改写必须持久化到 state。
- `MessageModifier` 是"临时的装饰"：每次调用前加一句 system prompt，但不应污染 state。否则第二轮调用会有两个 system prompt，第三轮三个...

**复刻版需要实现**：
- `AgentConfig.MessageRewriter` 和 `AgentConfig.MessageModifier` 字段
- 在 `modelPreHandle` 中按正确顺序调用，确保 copy-vs-in-place 语义

### 2.5 嵌套 Graph Callback 的 Address 隔离

**问题场景**：

当一个 ReAct agent 作为子图嵌入另一个更大的 graph 时：

```
RootGraph
├── PreprocessNode
├── ReActAgent (子图, graphName="ReActAgent")   ← 有自己的 WithMessageFuture
│   ├── ChatModel  (node, callback 触发)
│   └── ToolsNode   (node, callback 触发)
├── ReActAgent (另一个实例, 同 graphName)       ← 共享 graph name
└── PostprocessNode
```

`WithMessageFuture`（`react/option.go:151`）通过 graph callback 收集 agent 执行过程中的所有中间消息。但：
- RootGraph 可能也有自己的 callback，监听 `OnStart`/`OnEnd` 事件。
- 同一个 graph name（默认 `"ReActAgent"`）可能在同一个父图中被多个子 agent 实例共享。
- 如果不加隔离，Callback Handler 会收到**所有** node 的事件，包括其他 agent 实例的、外层 graph 的。

**Eino 的解法**：Address 机制（`react/option.go:259-285`）

1. **`cbHandler.claimOwnership`**：在 `onGraphStart` callback 触发时，通过 `GetCurrentAddress(ctx)` 获取当前 graph 的完整运行地址（如 `runnable:RootGraph;runnable:ReActAgent#1`）。
2. **`cbHandler.isOwnGraph`**：后续所有 callback 事件中，对比当前事件地址与记录的地址：
   - 地址不一致 → 这是别人的事件，直接 return（不做任何处理）
   - 地址一致 → 这才是我的事件，正常收集消息

**已有基础设施**：

复刻版已实现完整的 Address 体系（`compose/address.go`）：
- `AddressSegment` / `Address` 类型
- `AppendAddressSegment(ctx, typ, id)` —— 在 context 中进入子 scope
- `GetCurrentAddress(ctx)` —— 获取当前执行地址
- `Address.equal(other)` / `Address.hasPrefix(prefix)` —— 地址比对

这意味着 Chapter 7 的 address 隔离**不需要新建 address 机制**，直接复用即可。

**复刻版需要实现**：
- `cbHandler` 结构体，在 `OnStart` 时记录 address
- `isOwnGraph` 检查逻辑
- 将 `cbHandler` 注册为 graph callback

### 2.6 Host Multi-Agent：Specialist 作为 Tool + 单/多意图分支

**核心设计思路**：把"选择 specialist"当作一个 tool calling 问题。

**架构**：

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

**关键实现机制**：

#### 2.6.1 Specialist 包装为 Tool

1. 用户在 `MultiAgentConfig.Specialists` 中注册 specialist，每个有 `Name` 和 `IntendedUse`。
2. `NewMultiAgent` 为每个 specialist 生成一个 `schema.ToolInfo`，以 `Name` 为 tool name，以 `IntendedUse` 为 description。
3. Host ChatModel bind 这些 tool infos，其 system prompt 引导它"decide which tool is best for the task and call only the best tool"。
4. Host 的输出通过 branch 分发：如果有 tool call → 路由到 specialist；如果没有 → Host 自己直接回答。

#### 2.6.2 Specialist 入参替换

一个微妙但关键的设计：Specialist 的输入不是 Host 发出的 tool call argument（如 `{"reason": "..."}`），而是 **完整的 state.msgs**（用户原始消息历史）。

这通过 pre-handler 实现（`host/compose.go:160-162`）：
```go
// pre-handler: 丢弃 tool call input，用 state 中的原始消息替换
return state.msgs, nil
```

原因：Specialist 需要看到完整的用户上下文才能做出好的回答，而非仅看到 Host 的路由参数。

#### 2.6.3 单意图 vs 多意图分支

Host Multi-Agent 区分两种场景：

- **单意图**：Host 只调用了一个 specialist。结果直接取该 specialist 的输出 message 返回（`SingleIntentAnswer` 节点）。
- **多意图**：Host 调用了多个 specialist。结果通过 passthrough 节点 `SpecialistsAnswersCollector` 收集，然后：
  - 如果提供了自定义 `Summarizer`，用其 ChatModel 做总结
  - 否则用默认 lambda 简单拼接所有 message.Content

多意图判断：在 `addMultiSpecialistsBranch` 中，如果 Host 的 tool calls 数量 > 1，设置 `state.isMultipleIntents = true`。

#### 2.6.4 Specialist 的三种形式

`Specialist` 可以是以下之一：
- **ChatModel**：纯模型，附带 `SystemPrompt`（通过 pre-handler 注入）
- **Invokable / Streamable**：可以是任意实现了对应接口的 fn，也可以是 `react.Agent` 的 `Generate` / `Stream` 方法
- **两者都有**：`AnyLambda` 统一包装

**复刻版需要实现**：
- `MultiAgentConfig` / `Specialist` / `Host` 类型定义
- `NewMultiAgent` graph builder 函数
- `addSpecialistAgent` —— specialist 节点添加
- `addMultiSpecialistsBranch` —— mult-branch 分发
- `addAfterSpecialistsBranch` —— 单/多意图判断
- `addMultiIntentsSummarizeNode` —— 多意图聚合

---

## 3. cuts — 明确排除的生产特性

以下 Eino 生产特性在本次教育子集中**明确不实现**。其作用是在实施时明确边界，防止 scope creep。

### 3.1 Agent Option 双通道多态设计

**生产特性**：`flow/agent/agent_option.go` 的 `AgentOption` 双通道设计（`composeOptions` + `implSpecificOptFn`），通过泛型 `WrapImplSpecificOptFn[T]` 支持 ReAct agent 的 `WithTools` 和 host 的 `WithAgentCallbacks` 通过同一管道传递。

**复刻版处理**：直接暴露显式的 option 函数（如 `react.WithTools`、`react.WithMessageFuture`），不构建泛型双通道。

**原因**：双通道设计是为"多 agent 实现共享同一套 option 体系"服务的，教育子集只有两个 agent builder（ReAct + Host），不需要这种抽象层。显式 option 函数更易理解。

### 3.2 Host Multi-Agent 的 HandOff Callback

**生产特性**：`MultiAgentCallback` 接口 + `OnHandOff` 事件 + `ConvertCallbackHandlers` → 当 Host 将任务交给 specialist 时触发专门的回调。

**复刻版处理**：不实现 HandOff 专用回调。仅复用 graph 层的通用 callback 机制。

**原因**：HandOff callback 是面向可观测性/监控的运营特性。教育子集不需要独立的 handoff tracing。

### 3.3 Streaming ToolsNode 和 Enhanced Tool Result

**生产特性**：
- ToolsNode 支持 Streaming 执行（工具边执行边产出结果）
- 增强的 tool result（多模态结果：图片、音频、搜索片段等）
- `WithMessageFuture` 中的四种 tool result 场景（普通 string、streaming string、enhanced result、enhanced streaming result）

**复刻版处理**：ToolsNode 仅支持 Invoke 模式（顺序执行，等待完整结果）。Tool result 仅支持 string 类型。

**原因**：
- Streaming ToolsNode 需要工具接口支持 `StreamableTool`，education scope 的 Chapter 5 只定义了 `InvokableTool`
- 增强 tool result 需要多模态 Message 体系，education scope 的消息只有文本

### 3.4 WithMessageFuture 的完整实现

**生产特性**：
- `WithMessageFuture` 返回 `future.GetMessages()` 提供异步消息流
- 四种 tool result sender（普通、streaming、enhanced、enhanced streaming）
- `toolResultSenders` 注入 context 供工具中间件使用

**复刻版处理**：实现简化版 —— 通过 graph callback 收集 ChatModel 输出，但不支持异步 future 模式。消息收集直接通过 callback handler 内部 channel 实现。

**原因**：异步 future 模式的价值在多 agent 协作和长耗时任务中体现，简单 ReAct 循环可以直接在 callback 中同步收集。

### 3.5 ExportGraph 与 Graph 动态修改

**生产特性**：
- `Agent.ExportGraph()` 导出底层 graph，允许用户添加自定义节点和边
- 在已编译的 graph 上追加 pre/post 节点
- `WithGraphAddNodeOpts` 允许在 agent 的 graph 上注入自定义节点

**复刻版处理**：不暴露 `ExportGraph`。Agent builder 返回 `Agent{Graph, Runnable}` 结构，但 graph 为内部字段。

**原因**：动态 graph 修改引入了编译锁重入、节点注入顺序、option 传递等复杂问题。教育子集的 agent 是"一次性构建、不再修改"的。

### 3.6 生产级 ChatModel 与 ToolCallingModel 集成

**生产特性**：
- `ToolCallingModel` 接口（自动将 tool infos 绑定到 model）
- 多 provider tool calling 格式适配（OpenAI function calling vs Claude tool_use vs Gemini functionDeclarations）
- Model 级别的 `WithTools` option

**复刻版处理**：ChatModel 使用教育子集的 mock 实现。Tool calling 通过 `schema.ToolInfo` → graph branch condition 的显式路由模拟。不实现真实的 LLM tool calling 协议。

**原因**：教育子集的目标是演示 graph 层面的 ReAct 循环逻辑，而非集成真实的 LLM provider。真实的 tool calling 需要 provider adapter 层（Ch6）的完整实现。

### 3.7 Claude/Gemini Provider-Specific 的完整适配

**生产特性**：
- `ClaudeStreamToolCallChecker`（遍历完整 stream）
- `GeminiStreamToolCallChecker`（交替文本/tool call）
- Provider 原生的 content block 格式适配

**复刻版处理**：仅实现默认 `firstChunkStreamToolCallChecker`。在 AgentConfig 中暴露 `StreamToolCallChecker` 的可插拔注入点作为设计示范。

**原因**：Provider 适配是 Chapter 6 的职责。Chapter 7 关注的是 graph 拓扑层面，不涉及具体 provider 的消息格式。

### 3.8 多意图 Summarizer 的 ChatModel 集成

**生产特性**：
- 自定义 `Summarizer` ChatModel + 自定义 `SystemPrompt`
- Summarizer 的 pre-handler 注入 system prompt + state.msgs + 各 specialist 输出
- `map2List` 将 `map[string]any` 转为 `[]*schema.Message` 列表

**复刻版处理**：多意图聚合使用默认 lambda（简单拼接所有 message.Content）。不集成 ChatModel 做总结。

**原因**：依赖 ChatModel 的实现。简单拼接已经能演示多意图聚合的 graph 结构。

### 3.9 Interrupt/Resume 在 Agent Loop 中的应用

**生产特性**：
- Agent 循环每步都可 interrupt（在执行 tool 前/后挂起）
- Resume 时恢复 state 和 step 计数
- Checkpoint 包含 state.Messages 和 state.ReturnDirectlyToolCallID

**复刻版处理**：不实现 agent 级别的 interrupt/resume。graph 层的 checkpoint/interrupt 机制已在 Ch4 实现，但 agent 不与 checkpoint 深度集成。

**原因**：Agent + Interrupt/Resume 的组合引入了复杂的状态恢复逻辑（需要恢复 Graph 的 step count、Pregel channel 状态、local state 序列化），超出教育子集范围。

### 3.10 ReAct Agent 的 Model Callback 收集

**生产特性**：
- `BuildAgentCallback`（`react/callback.go`）：将 `ModelCallbackHandler` 和 `ToolCallbackHandler` 合并为一个 `callbacks.Handler`
- 内部调用顺序：ChatModel callback → Tool callback
- `initAgentCallbacks` 使用 `agent.WithComposeOptions` 注册到 compose 层

**复刻版处理**：不实现专用 callback builder。复用 graph 层的通用 callback 机制（`WithCallbacks` + `Handler`）。

**原因**：Chapter 7 的 callback 模式与 Chapter 3 的通用 callback 机制本质相同，不需要重复实现。

---

## 4. evidence — 证据

### 4.1 已有基础设施（可被 Chapter 7 复用的组件）

以下组件已在复刻版中存在，Chapter 7 直接依赖它们：

| 组件 | 文件 | 关键类型/函数 | Ch7 如何使用 |
|------|------|-------------|-------------|
| Graph + Compile | `graph.go`, `generic_graph.go`, `graph_compile.go` | `Graph[I,O]`, `Compile()`, `AnyPredecessor` | Agent 构建为 `Graph[[]*Message, *Message]` |
| Branch | `branch.go` | `GraphBranch`, `NewGraphBranch[I]` | ChatModel→Tools/END 分支、ReturnDirectly 分支 |
| Pregel Runtime | `pregel.go`, `graph_run.go` | `pregelChannel`, `runner.run()` | Agent 循环通过 AnyPredecessor + Pregel 实现 |
| Address | `address.go` | `Address`, `GetCurrentAddress`, `AppendAddressSegment` | 嵌套 cbHandler 隔离 |
| Callback | `callbacks.go` | `Handler`, `OnStartFn`, `OnEndFn` | `WithMessageFuture` 的消息收集 |
| Schema | `schema.go`, `chatmodel.go` | `Message`, `ToolCall`, `ToolInfo` | Agent 的消息和 tool call 数据模型 |
| MaxStep | `graph_compile.go` | `WithMaxRunSteps(steps)` | 防止 agent 无限循环 |
| Runnable | `runnable.go` | `Runnable[I,O]`, `Generate` / `Stream` | Agent 编译后输出为 Runnable |
| EventLog | `event_log.go` | `EventLog` | Agent 步数事件的日志记录 |

### 4.2 需要新建的文件

| 文件 | 内容 | 预估行数 |
|------|------|---------|
| `agent/react.go` | `state`、`AgentConfig`、`NewAgent`、pre-handler、branch conditions | ~400 |
| `agent/option.go` | `WithTools`、`WithMessageFuture`、`WithStreamToolCallChecker` | ~200 |
| `agent/callback.go` | `cbHandler`、address 隔离 | ~150 |
| `agent/host.go` | `MultiAgentConfig`、`Specialist`、`Host`、`NewMultiAgent` | ~450 |
| `agent/host_option.go` | Host option 函数 | ~80 |
| `agent/react_test.go` | ReAct agent 测试 | ~250 |
| `agent/host_test.go` | Host multi-agent 测试 | ~250 |

### 4.3 复刻版当前状态验证

```
$ go test ./... -count=1
ok  	github.com/rive/eino-compose-runtime-replica-go/compose	0.639s

$ go vet ./...
(no errors)

$ go build ./...
(no errors)
```

所有现有基础设施（Graph、Branch、Pregel、Callback、Address、Stream、Checkpoint）均可正常工作和测试。

### 4.4 Agent 层的 Graph 拓扑关系

ReAct Agent 的 graph 拓扑：

```
[!start] → ChatModel
ChatModel → ┬─ branch:toolCall=true → Tools → ┬─ branch:returnDirectly=true → direct_return lambda → [!end]
            │                                  │
            │                                  └─ branch:returnDirectly=false → ChatModel (loop back)
            │
            └─ branch:toolCall=false → [!end]
```

关键编译选项：
- **触发模式**：`AnyPredecessor`（Pregel），因为 ChatModel 和 Tools 不能并发，必须交替执行
- **MaxStep**：用户配置（默认 20-100），作为安全上限
- **Graph Name**：默认 `"ReActAgent"`，可自定义

Host Multi-Agent 的 graph 拓扑：

```
[!start] → Host
Host → ┬─ branch:toolCall=false → [!end]
       │
       └─ branch:toolCall=true → msg2MsgList
                                  msg2MsgList → ┬─ → Specialist_A ─┐
                                                 ├─ → Specialist_B ─┤
                                                 └─ → ...           └→ SpecialistsAnswersCollector
                                                                      → ┬─ singleIntent → [!end]
                                                                        └─ multiIntent → map2List → Summarize → [!end]
```

两个 graph 最终都编译为 `Runnable[[]*schema.Message, *schema.Message]`，可以：
- 直接调用 `Generate(ctx, messages)`
- 作为子图嵌入另一个 graph（通过 `AddLambdaNode`）
- 作为 Host Multi-Agent 的 Specialist

### 4.5 关键类型依赖图

```
Chapter 7 (Agent Flow)
│
├─ 依赖 Chapter 5 (Model/Tool/Prompt)
│   ├── Message {Role, Content, ToolCalls, ToolCallID}
│   ├── ToolCall, ToolInfo
│   └── ToolsNode (Invoke 模式)
│
├─ 依赖 Chapter 6 (Schema/Provider)
│   ├── 不使用 provider adapter（不集成真实 LLM）
│   └── 使用 schema 层的 ToolInfo 类型
│
├─ 依赖 Chapter 3 (Stream/Callback)
│   ├── StreamToolCallChecker 使用 StreamReader
│   ├── WithMessageFuture 通过 callback 收集
│   └── Runnable 降级（Stream → Invoke）
│
├─ 依赖 Chapter 2 (编排)
│   └── [不直接使用 Workflow/Chain，Agent 是独立的 Graph Builder]
│
└─ 依赖 Chapter 1 (Graph Runtime)
    ├── Graph[I,O] / Compile / AnyPredecessor
    ├── GraphBranch / Pre-handler
    ├── PregelChannel / runner
    ├── Address / Context 传递
    └── MaxStep / EventLog
```
