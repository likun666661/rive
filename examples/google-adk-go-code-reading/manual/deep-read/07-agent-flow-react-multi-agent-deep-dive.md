# 精读报告：Agent Flow / ReAct / Multi-Agent

> 基于 ADK Go 源码的深度分析报告，覆盖 Agent 执行流程、ReAct 循环、多 Agent 路由、策略插件、声明式配置五大主题。目标读者是需要在 `rive-adk-go` 中复现第七章 Agent Flow 的实现者。

---

## 1. problem

ADK Go 要解决的核心问题是：**如何构建一个可组合、可扩展的 LLM Agent 执行引擎，使单个 Agent 能够完成 ReAct 循环（推理-行动-观察），同时多个 Agent 能够在树形拓扑中相互路由和转移控制权？**

这一问题可拆解为六个子问题：

### 1.1 ReAct 循环的自动化执行

LLM Agent 需要自主完成以下循环，直到产生最终答案或主动终止：

```
用户输入 → 构造模型请求 → 调用 LLM → 解析响应（文本/函数调用）→ 执行工具 → 反馈工具结果 → 循环直至终止
```

框架必须处理：请求构造、流式响应聚合、并行工具调用、会话历史重建、回调短路语义、工具未找到的容错。

### 1.2 多 Agent 间的控制权转移

在多 Agent 系统中，单个 LLM 驱动的 Agent 需要能够将任务委托给树状层次结构中的其他 Agent。这包含：

- **LLM 层面的转移声明**：模型通过 `transfer_to_agent` 函数调用选择目标 Agent
- **Runner 层面的 Agent 选择**：当新用户消息到达时，Runner 必须确定树中的"当前活跃 Agent"
- **路由约束**：父→子、子→父、平级→平级的转移各有许可规则

### 1.3 策略层的可插拔扩展

裸 ReAct 循环是脆弱的。实际应用需要：

- **错误恢复**：工具失败时，Agent 应"反思"并调整策略，而非死循环
- **工具塑形**：在工具调用前后注入/剥离额外参数（如 `user_id`），不污染 LLM 的认知空间
- **技能注入**：长篇指令、参考文档、脚本按需加载到系统提示词中
- **记忆/制品预加载**：历史对话和会话制品自动纳入上下文
- **循环控制**：`exit_loop` 等信号机制让 Agent 优雅退出循环

### 1.4 声明式 Agent 构造

除编程式 API 外，还需支持 YAML 配置文件描述 Agent 拓扑，通过工厂注册表将配置文件解析为运行时的 Agent 实例。这使得非 Go 开发者也能编排 Agent 工作流。

### 1.5 统一的 Agent 抽象

单个 LLM Agent、工作流 Agent（Sequential/Parallel/Loop）、远程 Agent（A2A 协议）、Agent-as-Tool —— 这些形态各异的实体必须共享同一个 `agent.Agent` 接口，实现统一的组合与嵌套。

### 1.6 示例驱动的教学模式

框架需要提供一套最小化、可教学的示例代码，覆盖单 Agent、多 Agent、工作流编排、管道传参、Web 服务等典型模式，让使用者无需 fork 整个框架即可构建自己的 ReAct/多 Agent 系统。

---

## 2. why_hard

### 2.1 流式响应的增量聚合

LLM 的流式输出以片段形式到达——文本片段、增量函数调用 JSON 补丁（`PartialArgs`）、思考签名等。框架必须在分派函数调用之前将这些片段缓冲并组装为完整部件。排序错误（如在音频转录完成前解析函数调用）会直接破坏模型行为。ADK Go 在 `internal/llminternal/stream_aggregator.go` 中维护了一个复杂的状态机：文本按思考/非思考边界合并，函数调用的增量 JSON Path 被累积到 `currentFunctionArgs` map 中，`WillContinue=false` 时才刷新到输出序列。

### 2.2 并行函数调用与结果合并

单个模型响应可能请求多个工具。这些调用应并发执行（goroutine），但结果必须合并为单个响应事件。合并逻辑需处理重叠的状态增量（state delta）、转移动作（transfer actions）、跳过摘要标志（skip-summarization flags），且不能丢失数据。ADK Go 通过 `sync.WaitGroup` 编排并发，`mergeParallelFunctionResponseEvents` 负责合并——这比串行执行复杂数个量级。

### 2.3 会话历史重建的复杂性

在每轮 LLM 调用之间，框架必须回溯会话事件，将函数响应与其原始调用配对（可能跨越长时间异步间隙），按正确的时间顺序重排，过滤内部簿记调用（EUC 凭证请求、工具确认），并将外来 Agent 的事件转换为当前 Agent 的用户角色上下文。`contents_processor.go` 是这一复杂性的集中体现——它实现了分支范围过滤、异步调用/响应对齐、外来事件格式转换、转录聚合等多个子步骤。

### 2.4 回调链的短路语义

每一个生命周期阶段（模型调用前后、工具调用前后、错误发生）都有一条回调链。任意回调返回非 nil 即可短路后续行为。插件复制了这些钩子。协调插件和 Agent 本地回调之间的短路优先级，同时保持错误传播的正确性，是零 Bug 实现的核心挑战。

### 2.5 动态工具构造与分布式授权模型

`transfer_to_agent` 工具的模式是在运行时从实际的 Agent 子树动态构建的——其 `agent_name` 枚举取决于 Agent 在树中的位置。决策分散在四个层面：LLM 模型选择目标、`TransferToAgentTool.Run` 设置动作、`Flow.runOneStep` 执行转移、`Runner.findAgentToRun` 为下一用户回合选择 Agent。每一层必须对同一组允许目标达成一致。

### 2.6 隐式编排 vs 显式 DAG

ADK Go 没有中央"编排器"来调度 Agent。Agent 接口本身就是 `Run(ctx) -> iter.Seq2[*Event, error]`。多 Agent 流从工作流 Agent（Sequential/Parallel/Loop）和 Agent-as-Tool 包装（`agenttool.New`）中涌现。理解"编排 = Agent 嵌套 Agent"这一范式对新人极不直观。

### 2.7 工具类型的碎片化

`functiontool`、`agenttool`、`geminitool`、`mcptoolset`、`skilltoolset`、`loadartifactstool`、`exitlooptool`——这些工具形态共享 `tool.Tool` 接口，但 `Toolset` 还额外实现了 `RequestProcessor` 以在每轮模型调用前注入指令和工具声明。理解 `Tool` vs `Toolset` 的区别及其在请求流水线中的位置是一个陡峭的学习曲线。

---

## 3. design_approach

### 3.1 三层架构

ADK Go 将 Agent 执行路径建模为围绕一个紧凑的 ReAct 循环的**无状态处理器管道**：

```
┌─────────────────────────────────────────────────────────────────┐
│  Runner（runner/runner.go）                                      │
│  · 会话生命周期 · Agent 树路由 · 插件生命周期（before/after run）│
│  · 外层壳，不感知工具和模型调用                                   │
├─────────────────────────────────────────────────────────────────┤
│  Flow（internal/llminternal/base_flow.go）                       │
│  · ReAct 循环核心                                                │
│  · 请求处理器（填充 LLMRequest）→ 模型调用（含回调链）→ 响应处理器│
│  · 函数调用处理（并行执行 + before/after/error 回调）             │
│  · 转移执行（调用 nextAgent.Run(ctx)）                            │
├─────────────────────────────────────────────────────────────────┤
│  LLMAgent（agent/llmagent/llmagent.go）                          │
│  · 用户可见的 API 边界                                            │
│  · 将配置（模型、工具、指令、回调）注入 Flow 并委托 Run()         │
└─────────────────────────────────────────────────────────────────┘
```

### 3.2 迭代器统一语义

每一步都返回 `iter.Seq2[*session.Event, error]`。这一统一返回类型使得处理器、模型调用和主循环都自然可组合。处理器可以产生事件（如工具确认提示）而不破坏迭代模型。

### 3.3 处理器即函数中间件

**请求处理器** (`func(ctx, *LLMRequest, *Flow) iter.Seq2[*session.Event, error]`) 形成有序列表 `DefaultRequestProcessors`，每个处理器丰富请求或产生副作用事件。**响应处理器** (`func(ctx, *LLMRequest, *LLMResponse) error`) 在模型返回后修改 LLMResponse。预留的存根处理器（NL Planning、Code Execution、Auth）保持管道槽位，便于未来实现。

默认请求处理器顺序及职责：

| # | 处理器 | 文件 | 职责 | 状态 |
|---|--------|------|------|------|
| 1 | `basicRequestProcessor` | `basic_processor.go:31` | 复制 GenerateContentConfig | DONE |
| 2 | `toolProcessor` | `tools_processor.go:29` | 提取 Tool/Toolset 列表 | DONE |
| 3 | `authPreprocessor` | `other_processors.go:35` | 凭证注入（存根） | STUB |
| 4 | `RequestConfirmationRequestProcessor` | `request_confirmation_processor.go:37` | HITL 确认恢复 | DONE |
| 5 | `instructionsRequestProcessor` | `instruction_processor.go:41` | 指令注入+占位符解析 | DONE |
| 6 | `identityRequestProcessor` | `identity_request_processor.go:29` | 注入 Agent 身份提示 | DONE |
| 7 | `ContentsRequestProcessor` | `contents_processor.go:37` | 会话历史编译 | DONE |
| 8 | `nlPlanningRequestProcessor` | `other_processors.go:25` | NL 规划（存根） | STUB |
| 9 | `codeExecutionRequestProcessor` | `other_processors.go:30` | 代码执行（存根） | STUB |
| 10 | `outputSchemaRequestProcessor` | `outputschema_processor.go:41` | 结构化输出工具注入 | DONE |
| 11 | `AgentTransferRequestProcessor` | `agent_transfer.go:69` | 注入 transfer_to_agent 工具 | DONE |
| 12 | `removeDisplayNameIfExists` | `file_uploads_processor.go:28` | Gemini 兼容性处理 | DONE |

### 3.4 带短路的回调链

每个回调钩子（BeforeModel、AfterModel、BeforeTool、AfterTool、OnError）遵循：**先运行插件回调 → 再运行 Agent 本地回调 → 第一个非 nil 返回值胜出**。如果 `BeforeModelCallback` 返回 `LLMResponse`，则完全跳过实际模型调用——支持缓存、模拟、护栏等能力。

### 3.5 Agent 即统一原语

```go
type Agent interface {
    Name() string
    Description() string
    Run(InvocationContext) iter.Seq2[*session.Event, error]
    SubAgents() []Agent
}
```

- **LLMAgent**：包装模型 + 指令 + 工具
- **工作流 Agent**（SequentialAgent、ParallelAgent、LoopAgent）：编排 SubAgents 的 Run 调用
- **远程 Agent**（`remoteagent.A2A`）：将 A2A 客户端包装为 Agent
- **agenttool.New(agent, config)**：将任意 Agent 包装为 `tool.Tool`，使 Agent 可作为另一个 Agent 的函数调用目标

### 3.6 树形拓扑与双向转移

Agent 树支持四个方向的转移：

```
           RootAgent (LLMAgent)
          /         \
   SubAgent1    SubAgent2 (LLMAgent)
   (LLMAgent)      |
              SubAgent2_1 (SingleFlow)
```

1. **父→子**：始终允许（若 Agent 有子 Agent）
2. **子→父**：默认允许，除非 `DisallowTransferToParent=true`
3. **平级→平级**：仅当 `DisallowTransferToPeers=false` 且父 Agent 也是 AutoFlow Agent
4. **非 LLMAgent 父**：若父为 Workflow Agent 或自定义 Agent，不添加为转移目标

### 3.7 SingleFlow vs AutoFlow

Agent 的 `shouldUseAutoFlow` 判定：有子 Agent 或未禁用父/平级转移 → AutoFlow（可转移）。无子 Agent 且同时禁用父/平级转移 → SingleFlow（不可转移）。SingleFlow Agent 的会话在下一用户回合会"反弹"到最近的可转移祖先。

### 3.8 声明式配置

```yaml
agent_class: LlmAgent
name: root
model: gemini-2.0-flash
instruction: "You are helpful."
sub_agents:
  - config_path: "./sub_agent.yaml"
tools:
  - name: "google_search"
```

工厂注册表通过 `agent_class` 分发到 `newLLMAgent`、`newLoopAgent`、`newParallelAgent`、`newSequentialAgent` 等构造函数。独立的工具注册表和回调注册表支持按名称引用组件。

---

## 4. code_walkthrough

### 4.1 入口：Runner.Run

**文件**：`runner/runner.go:131`

```
Run(ctx, userID, sessionID, msg, cfg) -> iter.Seq2[*session.Event, error]
```

执行步骤：
1. 通过 `SessionService` 加载或自动创建会话
2. `findAgentToRun`（`:592`）——反向扫描会话历史，找到最后一个可转移的非用户事件作者；若为用户函数回调响应，匹配原始调用事件并路由到对应 Agent；回退到根 Agent
3. 构建 `InvocationContext`（制品、记忆、会话、Agent、用户内容、运行配置、调用 ID）
4. 通过 `PluginManager.RunOnUserMessageCallback` 转换用户消息，追加为会话事件
5. 运行插件 `BeforeRunCallback`；若提前退出，产生合成事件并返回
6. 调用 `agentToRun.Run(ctx)`
7. 对每个产生的事件：运行 `PluginManager.RunOnEventCallback`（修改或替换），向会话历史追加非 partial 事件，向调用者产生事件
8. 延迟 `PluginManager.RunAfterRunCallback`

### 4.2 Agent 构造：LLMAgent.New

**文件**：`agent/llmagent/llmagent.go:34`

1. 将用户可见的回调类型转换为内部 `llminternal.*Callback` 类型
2. 将配置（模型、工具、工具集、指令、模式、转移策略）打包为 `llminternal.State`
3. 通过 `agent.New(...)` 创建基 `agent.Agent`，将 `Run` 连接到 `llmAgent.run`
4. 暴露内部状态供 before-agent-callback 访问

### 4.3 Agent 执行：llmAgent.run

**文件**：`agent/llmagent/llmagent.go:361`

1. 用 LLMAgent 的特定状态包装调用上下文
2. 构造 `Flow`（模型、默认处理器列表、回调链）
3. 迭代 `Flow.Run(ctx)`，通过 `maybeSaveOutputToState` 处理每个事件（若 `OutputKey` 已设置，保存文本输出到会话状态）

### 4.4 核心 ReAct 循环：Flow.Run

**文件**：`internal/llminternal/base_flow.go:101`

```go
for {
    lastEvent := ...
    for ev, err := range f.runOneStep(ctx) {
        yield ev
        lastEvent = ev
    }
    if lastEvent == nil || lastEvent.IsFinalResponse() { return }
    if lastEvent.LLMResponse.Partial { error }
}
```

每次 `runOneStep` 是一次 ReAct 迭代。循环持续执行，只要最后一个事件包含需要执行的函数调用。

**停止条件**：
- `lastEvent == nil`：未产生模型响应事件（边界情况）
- `IsFinalResponse()`：事件标记为最终（无待定函数调用 / 显式停止 / 已转移）
- Partial 响应未完成：流式传输达到 token 限制但未完成
- `ev.Actions.TransferToAgent != ""`：Agent 委托给另一 Agent，当前循环结束
- `ctx.Ended()`：上下文已取消或会话已结束

### 4.5 单步执行：Flow.runOneStep

**文件**：`internal/llminternal/base_flow.go:528`

四个子阶段：

#### a. 预处理（`:656`）

按顺序运行每个请求处理器。然后运行 `toolPreprocess`（每个工具的 `ProcessRequest` 向 `req.Tools` 添加函数声明）和 `toolsetPreprocess`（每个工具集的 `ProcessRequest`）。若 `ctx.Ended()` 返回 true，中止。

#### b. 调用 LLM（`:722`）

1. 运行 `PluginManager.RunBeforeModelCallback`，然后 Agent 的 `BeforeModelCallbacks`。第一个非 nil 响应短路模型调用
2. `generateContent`（`:809`）包装模型调用，包含遥测 span 创建、请求/响应日志记录、span 结束
3. 模型的 `GenerateContent(ctx, req, stream)` 返回 `LLMResponse` 分块的迭代器。流式模式下，每个 partial 分块立即产生；最终分块是 `streamingResponseAggregator.Close()` 的聚合结果
4. 模型错误：运行 `OnModelErrorCallbacks`（先插件后 Agent）。若回调返回响应，用作恢复
5. 运行 `AfterModelCallbacks`（先插件后 Agent）。回调响应若非 nil，替换模型响应
6. 填充客户端函数调用 ID（Genai API 使其可选）

#### c. 后处理（`:901`）

运行 `ResponseProcessors`（当前为 `nlPlanningResponseProcessor` 和 `codeExecutionResponseProcessor`，均为存根）。

#### d. 处理函数调用和最终化

1. `finalizeModelResponseEvent`（`:925`）——创建包含作者、分支、LLMResponse、状态增量的 `session.Event`，产生事件
2. 跳过 partial 响应的函数调用处理
3. `handleFunctionCalls`（`:1009`）——执行工具调用。若 nil，无函数调用，返回（循环结束）
4. 若工具确认被请求，生成并产生 `requestConfirmationEvent`
5. 若使用结构化输出（`set_model_response` 工具），提取并产生最终模型响应
6. 若 `event.Actions.TransferToAgent` 已设置，递归调用 `nextAgent.Run(ctx)`，内联产生其事件

### 4.6 函数调用处理：Flow.handleFunctionCalls

**文件**：`internal/llminternal/base_flow.go:1009`

1. 从响应内容提取 `FunctionCall` 部件
2. 若多个函数调用，创建合并遥测 span
3. 对每个函数调用启动 goroutine：
   - `stop_streaming`：取消所有活跃流式工具实例
   - **工具未找到**：创建描述性错误，传递到 `runOnToolErrorCallbacks`
   - **流式工具**：若实时会话，注册流并异步发送块；否则收集所有块为单个结果
   - **普通工具**：`callTool`
4. 使用结果构建 `FunctionResponse` 部件，包装为 `session.Event`
5. 运行 `sync.WaitGroup` 等待所有 goroutine
6. `mergeParallelFunctionResponseEvents`：合并所有响应事件为一个合并事件

### 4.7 工具执行链：Flow.callTool

**文件**：`internal/llminternal/base_flow.go:1193`

```
callTool(ctx, tool, args) -> result
```

链式回调：
1. `PluginManager.RunBeforeToolCallback` → Agent `invokeBeforeToolCallbacks`。第一个非 nil 结果短路
2. `tool.Run(ctx, args)`——实际工具执行
3. 错误时：`PluginManager.RunOnToolErrorCallback` → Agent `invokeOnToolErrorCallbacks`——允许重写错误结果
4. `PluginManager.RunAfterToolCallback` → Agent `invokeAfterToolCallbacks`——允许后处理结果/错误

**这正是策略插件（RetryAndReflect、FunctionCallModifier）注入其行为的关键位置。**

### 4.8 工具接口体系

**文件**：`tool/tool.go:38`

```go
type Tool interface {
    Name() string
    Description() string
    IsLongRunning() bool
}
```

额外接口（`internal/toolinternal`）：
- `FunctionTool`：提供 `Declaration() *genai.FunctionDeclaration`——JSON Schema
- `RequestProcessor`：`ProcessRequest(ctx, req) error`——在请求构造阶段注入工具声明

`functiontool.New[TArgs, TResults](cfg, handler)` 使用 Go 泛型从 `TArgs` 和 `TResults` 推断 JSON Schema。支持显式 Schema 覆盖、HITL 确认、panic 恢复。

### 4.9 会话历史编译器：ContentsRequestProcessor

**文件**：`internal/llminternal/contents_processor.go:37`

这是最复杂的处理器之一。其子步骤：

1. **过滤**（`buildContentsDefault`）：跳过无内容/角色的会话事件、不在当前分支中的事件、内部函数调用事件（`adk_request_credential`、`adk_request_confirmation`）
2. **转换外来事件**：Agent A 的输出变为 Agent B 的用户角色上下文（`"[agent_A] said: ..."`）
3. **聚合转录**：连续的输入/输出转录 partial 合并为单个文本部件
4. **重排最新函数响应**：若最后一个事件是函数响应且未紧接其原始调用，回溯搜索调用事件，移除中间事件，合并响应
5. **重排异步响应**：对长时间运行的工具，合并拆分的调用/响应对
6. **清理**：移除空部件，剔除客户端函数调用 ID

### 4.10 代理转移机制

**文件**：`internal/llminternal/agent_transfer.go`

**转移工具的创建**（`AgentTransferRequestProcessor:69`）：
1. 检查 `shouldUseAutoFlow`——SingleFlow 或非 LLMAgent 返回空
2. 调用 `transferTargets(agent, parent)` 计算允许目标：
   - `slices.Clone(agent.SubAgents())`——直接子级始终包含
   - 若不禁用且父为 LLMAgent，添加父
   - 若不禁用且父为 AutoFlow，添加平级
3. 创建 `TransferToAgentTool`，其 `Declaration()` 返回包含 `agent_name` 枚举的 `genai.FunctionDeclaration`
4. 将转移指令和工具声明注入请求

**转移执行**（`TransferToAgentTool.Run:167`）：
```go
ctx.Actions().TransferToAgent = agent
```

**转移动作在 Flow 中**（`base_flow.go:639-651`）：
```go
nextAgent := f.agentToRun(ctx, ev.Actions.TransferToAgent)
for ev, err := range nextAgent.Run(ctx) {
    yield(ev, err)
}
```

转移是**即时且同步的**——目标 Agent 在同一迭代中运行，使用相同的调用上下文（相同会话、相同调用 ID），但 `ctx.Agent()` 返回新 Agent。所有来自目标 Agent 的事件以目标名称作为 Author。

### 4.11 多回合 Agent 选择：Runner.findAgentToRun

**文件**：`runner/runner.go:592`

```
用户发送消息
    ↓
[1] 检查函数响应匹配 → 返回调用作者 Agent
    ↓ (无匹配)
[2] 反向扫描会话事件
    ↓
    跳过用户作者事件
    ↓
    通过 event.Author 查找 Agent
    ↓
    检查 isTransferableAcrossAgentTree
    ↓ (是)
    返回该 Agent
    ↓ (无符合条件的事件)
[3] 返回根 Agent
```

**可转移性检查**（`runner.go:653-666`）：
```go
func isTransferableAcrossAgentTree(agentToRun agent.Agent) bool {
    for curAgent := agentToRun; curAgent != nil; curAgent = parents[curAgent.Name()] {
        if !ok || DisallowTransferToParent { return false }
    }
    return true
}
```

沿父链向上遍历，要求 Agent 及所有祖先均实现 `llminternal.Agent` 且 `DisallowTransferToParent == false`。若任一祖先阻断转移，则该 Agent 不可转移，扫描继续。

### 4.12 插件系统

**文件**：`plugin/plugin.go`、`internal/plugininternal/plugin_manager.go`

12 个钩子点，覆盖完整生命周期：

| 钩子 | 语义 | 位置 (plugin_manager.go) |
|------|------|--------------------------|
| `OnUserMessage` | 转换用户消息 | `:76` |
| `BeforeRun` | 提前退出（合成事件） | `:93` |
| `AfterRun` | 清理，不产生事件 | `:110` |
| `OnEvent` | 转换任意事件 | `:120` |
| `BeforeAgent` | Agent 回调链前运行 | `:137` |
| `AfterAgent` | Agent 回调链后运行 | `:153` |
| `BeforeModel` | 修改请求/短路模型调用 | `:222` |
| `AfterModel` | 修改响应 | `:239` |
| `OnModelError` | 模型错误恢复 | `:256` |
| `BeforeTool` | 修改工具参数/短路工具执行 | `:171` |
| `AfterTool` | 转换工具结果 | `:188` |
| `OnToolError` | 工具错误恢复 | `:205` |

**RetryAndReflect 插件**（`plugin/retryandreflect/plugin.go`）实现了最复杂的策略：

1. 通过 `OnToolErrorCallback` 拦截工具错误
2. 维护按工具名分组的失败计数器（支持 Invocation 和 Global 两种作用域）
3. 若当前重试次数 ≤ 最大重试次数 → 生成反射指导（"工具 X 调用失败，分析错误，考虑替代方法"）
4. 若超出最大重试次数 → 生成超限消息（"停止使用工具 X，制定新策略"）
5. 工具成功时重置计数器（`afterTool`）
6. 模板文件（`reflection.md`、`exceeded.md`）通过 `text/template` 渲染，填充工具名、错误详情、参数摘要、重试次数

**FunctionCallModifier 插件**（`plugin/functioncallmodifier/plugin.go`）实现两阶段工具塑形：

- **Before Model**：遍历所有函数声明，对匹配 Predicate 的工具注入额外 `Args`
- **After Model**：从函数调用中剥离注入的参数，存入会话状态 `{callID}/{argName}`，防止敏感参数到达实际工具

### 4.13 技能工具集

**文件**：`tool/skilltoolset/`

架构层次：
```
tool/skilltoolset/toolset.go          ← 顶层工具集
  ├── internal/skilltool/
  │   ├── list_skills.go              ← tool.Tool: list_skills
  │   ├── load_skill.go               ← tool.Tool: load_skill
  │   └── load_skill_resource.go      ← tool.Tool: load_skill_resource
  └── skill/
      ├── source.go                   ← Source 接口
      ├── frontmatter.go              ← YAML frontmatter 解析/验证
      ├── filesystem_source.go        ← fs.FS 支持的 Source
      ├── merged_source.go            ← 多源聚合
      ├── frontmatter_preload.go      ← 预加载 frontmatter
      └── complete_preload.go         ← 全量内存预加载
```

技能格式（`SKILL.md`）：
```yaml
---
name: my-skill
description: Does something useful
allowed-tools: [tool_a, tool_b]
---
# Markdown instructions here
```

暴露给 LLM 的三个工具：`list_skills`（返回 `<available_skills>` XML）、`load_skill`（加载指令+frontmatter）、`load_skill_resource`（加载资源文件，上限 10MB）。工具集的 `ProcessRequest` 在每轮 LLM 请求中注入技能使用指南和 XML 技能列表。

### 4.14 记忆与制品预加载

- **PreloadMemoryTool**（`tool/preloadmemorytool/tool.go`）：自动工具，实现 `ProcessRequest`；使用用户当前查询文本搜索记忆，注入 `<PAST_CONVERSATIONS>` XML。模型不可调用。
- **LoadMemoryTool**（`tool/loadmemorytool/tool.go`）：显式工具，模型可调用 `load_memory(query)` 查询匹配的记忆条目。
- **LoadArtifactsTool**（`tool/loadartifactstool/`）：两阶段模式——第一轮模型调用 `load_artifacts(["foo"])`，第二轮工具预加载实际制品内容。

### 4.15 流式响应聚合

**文件**：`internal/llminternal/stream_aggregator.go`

`streamingResponseAggregator` 的状态机：
- **文本**：连接合并到缓冲区；在思考/非思考边界或不同部件类型到达时刷新
- **函数调用**：`PartialArgs`（JSON Path 增量更新）累积到 `currentFunctionArgs` map；`WillContinue=false` 时，组装好的函数调用刷新到序列
- **思考签名**：按思考块和函数调用跟踪
- **Close()**：刷新剩余缓冲区，返回最终聚合的 `LLMResponse`（含元数据：Usage、Grounding、Citations）

### 4.16 模型抽象

**文件**：`model/llm.go:26`

```go
type LLM interface {
    Name() string
    GenerateContent(ctx, req *LLMRequest, stream bool) iter.Seq2[*LLMResponse, error]
}
```

Gemini 适配器（`model/gemini/gemini.go`）：
1. 可能附加 user-role 内容以保证 API 兼容性
2. 构造 `GenerateContentConfig`（工具、系统指令、安全设置）
3. 流式模式：`genai.Models.GenerateContentStream` → `streamingResponseAggregator.ProcessResponse` → 同时产生 partial 分块和 `aggregator.Close()` 聚合结果
4. 非流式模式：`genai.Models.GenerateContent` → 产生单个响应
5. 使用 `converters.Genai2LLMResponse` 映射类型

### 4.17 声明式 Agent 构造

**文件**：`internal/configurable/configurable.go`

- `FromConfig(path)`：读取 YAML → 检查 `agent_class` → 分发到注册工厂 → 反序列化为具体配置 → 构建 Agent
- `ResolveAgentReference`：解析相对路径子 Agent 配置，带缓存（`agentRegistry` map）
- 工具注册表（`configurable_utils.go:51-239`）：预制注册了 `exit_loop`、`google_search`、`url_context`、`google_maps_grounding`、`AgentTool`、`LongRunningFunctionTool`、`ExampleTool`、`McpToolset`
- 尚未注册但已有 Go 实现：`skill_toolset`、`preload_memory`、`load_memory`、`load_artifacts`、`retry_and_reflect_plugin`、`function_call_modifier_plugin`

### 4.18 示例代码全景

| 示例 | 文件 | 模式 |
|------|------|------|
| quickstart | `examples/quickstart/main.go:34-65` | 单个 LLM Agent + 工具 — 最小可用 Agent |
| web | `examples/web/main.go:63-114` | 多 Agent Loader — 兄弟 Agent 可选 |
| multipletools | `examples/tools/multipletools/main.go:42-121` | Agent-as-Tool — 动态 Agent 路由 |
| sequential | `examples/workflowagents/sequential/main.go:55-95` | 静态顺序编排 |
| sequentialCode | `examples/workflowagents/sequentialCode/main.go:44-149` | 管道传参（OutputKey + 占位符） |
| skills | `examples/skills/main.go:47-87` | 声明式技能加载 |
| mcp | `examples/mcp/main.go:88-135` | MCP 工具集成 |
| toolconfirmation | `examples/toolconfirmation/main.go:128-200` | 人工确认（HITL） |

---

## 5. replica_plan

以教学清晰度为优先，在 `rive-adk-go` 中实现 Agent Flow 功能。以下是分阶段计划。

### 5.1 Phase 1：核心类型与 ReAct 循环（250-350 LOC）

**实现 ArcReAct 的 Flow 等价物**：

1. **`Tool` 接口**：`Name()`、`Description()`、`IsLongRunning()`、`Declaration()` → JSON Schema、`Run(ctx, args) -> result`
2. **`LLM` 接口**：`GenerateContent(ctx, request, stream) -> Iterator<Response>`
3. **`LLMRequest`**：Contents（历史）、Config（温度、安全）、Tools（名称→实例映射）
4. **`LLMResponse`**：Content（部件列表，含 Text/FunctionCall/FunctionResponse）、Partial 标志、FinishReason、Metadata
5. **`SessionEvent`**：Author、Branch、Content、Actions（StateDelta、TransferToAgent、ToolConfirmations）、Partial、LongRunningToolIDs
6. **`Flow`** 核心循环：
   ```go
   for {
       events, err := runOneStep(ctx)
       lastEvent = events.Last()
       if lastEvent == nil || lastEvent.IsFinalResponse() { return }
   }
   ```
7. **`runOneStep`** 四阶段：Preprocess → callLLM → Postprocess → handleFunctionCalls
8. 停止条件：无函数调用、FinalResponse、TransferToAgent、ctx.Ended

### 5.2 Phase 2：请求处理器管道（200-300 LOC）

实现有序处理器链，每个处理器签名为 `func(ctx, req, flow) iter.Seq2[*Event, error]`：

1. **BasicConfigProcessor**：复制模型配置（temperature、safety、max_tokens）
2. **ToolLoaderProcessor**：从 Agent 配置提取工具列表，注入请求
3. **InstructionsProcessor**：追加系统指令（支持 `{var_name}`、`{artifact.name}` 占位符解析）
4. **IdentityProcessor**：注入 `"You are an agent. Your internal name is X."`
5. **ContentsProcessor**（关键组件）：
   - 按分支过滤事件
   - 将外来 Agent 事件转换为 user-role 上下文
   - 异步函数调用/响应对齐
   - `IncludeContents` 模式（Full/CurrentTurnOnly）
6. **AgentTransferProcessor**（见 Phase 6）
7. **存根槽位**：NlPlanningProcessor、CodeExecutionProcessor、AuthProcessor（均实现为空操作 + TODO 注释）

### 5.3 Phase 3：函数调用处理器（200-300 LOC）

1. **工具查找**：通过名称匹配，未找到时生成 `"Tool 'X' not found. Available tools: [list]"` 错误
2. **回调链**：`beforeCallbacks` → `tool.Run` → `onErrorCallbacks`（若失败）→ `afterCallbacks`
3. **并行执行**：goroutine + `sync.WaitGroup`
4. **结果合并**：将并行结果合并为单个函数响应事件（合并部件、状态增量、转移动作）
5. **HITL 确认流程**：可延迟，用存根实现

### 5.4 Phase 4：模型适配器（150-250 LOC）

1. **Gemini 适配器**：
   - 包装 genai SDK
   - 流式响应的增量聚合（文本合并、函数调用 JSON 累积）
   - 非流式路径的简单包装
2. **OpenAI 适配器**（可选扩展点，展示多模型支持）

### 5.5 Phase 5：策略插件层（200-300 LOC）

1. **Plugin 结构体**：包含 12 个可选钩子函数字段
2. **钩子执行顺序**：注册顺序，先插件后 Agent 本地回调，第一个非 nil 短路
3. **RetryAndReflect**（必须实现）：
   - 按工具名维护失败计数器（Invoation 作用域和 Global 作用域）
   - 嵌入 `reflection.md` 和 `exceeded.md` 模板
   - `OnToolError` 逻辑：≤maxRetries → 反射指导；>maxRetries → 超限消息
   - `AfterTool` 逻辑：成功后重置计数器
   - `sync.Mutex` 保护并发访问
4. **FunctionCallModifier**（推荐实现）：
   - BeforeModel 注入额外 Args
   - AfterModel 剥离并存入会话状态
5. **LoggingPlugin**（教学参考）：
   - 连接所有 12 个钩子
   - 格式化日志输出

### 5.6 Phase 6：Agent 转移与多 Agent 路由（350-500 LOC）

1. **Agent 接口**：`Name()`、`Description()`、`SubAgents()`、`Run(ctx)`
2. **Parent Map**：初始化时遍历 Agent 树，构建 `map[name]parent`；验证无重复名称、无多重父级
3. **TransferToAgentTool**（动态工具）：
   - `Declaration()` 返回包含目标 Agent 名称枚举的 Schema
   - `Run()` 设置 `ctx.Actions().TransferToAgent = targetName`
4. **TransferTargets 计算**：
   - 所有子 Agent
   - 父 Agent（`!DisallowTransferToParent` 且父为 LLMAgent）
   - 平级 Agent（`!DisallowTransferToPeers` 且父为 AutoFlow）
5. **转移执行**：在 `runOneStep` 的工具处理后，检查 `actions.TransferToAgent`，调用 `target.Run(ctx)` 并内联产生事件
6. **FindAgentToRun**：
   - 反向扫描会话事件
   - 跳过用户事件
   - 找到第一个可转移的非用户事件 Author
   - 沿父链检查 `DisallowTransferToParent`（任一祖先阻断 → 跳过）
   - 回退到根 Agent
7. **函数响应路由**：若用户消息包含函数响应，匹配原始调用事件并路由到该作者
8. **SingleFlow 检测**：`len(SubAgents)==0 && DisallowTransferToParent && DisallowTransferToPeers` → 不注入转移工具
9. **事件身份**：`Author = agent.Name()`；用户事件 `Author = "user"`
10. **状态隔离**：Agent 仅保存自己撰写的输出（`Author == self.Name()`）

### 5.7 Phase 7：工作流 Agent（150-200 LOC）

1. **SequentialAgent**：
   - 按 `SubAgents` 顺序调用 `agent.Run(ctx)`
   - 收集并产生所有事件
   - Session State（`{key}` 模板解析）在阶段间传递数据
2. **ParallelAgent**：
   - goroutine 并发调用 `SubAgents`
   - 使用 channel 或 `errgroup` 收集结果
3. **LoopAgent**：
   - 重复调用 `SubAgent[0]`，最多 `MaxIterations` 次
   - 支持通过 `exit_loop` 工具（`Escalate=true`）提前退出

### 5.8 Phase 8：配置加载与 Web 演示（250-350 LOC）

1. **Config Loader**（最小化 `configurable.FromConfig`）：
   ```yaml
   type: react
   name: root
   model: gemini-2.0-flash
   instruction: "You are helpful."
   sub_agents:
     - config_path: "./sub_agent.yaml"
   tools:
     - type: agent
       name: search
   ```
   支持三种配置值类型：`react`（→ ReActAgent）、`sequential`（→ SequentialAgent）、`parallel`（→ ParallelAgent）
2. **Web 服务器**（仅 `net/http`）：
   - `POST /chat`：接收 `{message}`，返回 Agent 响应
   - SSE 流式端点（可选）
   - 最小 HTML 聊天 UI

### 5.9 不实现（留作存根或后续）的组件

| 组件 | 理由 |
|------|------|
| Live 双向流式 | 复杂，基础循环运行后再添加 |
| NL Planning 处理器 | 依赖复杂提示工程；保留处理器槽位 + TODO |
| Code Execution 处理器 | 需沙箱环境；保留槽位 + TODO |
| Auth 预处理器 | 需身份提供商集成；存根 |
| 遥测/追踪 | 用 `log.Printf` 替代 OpenTelemetry |
| 会话持久化 | 初始使用内存存储 |
| Skill Toolset（完整版） | 用静态技能文件替代文件系统遍历 |
| MCP Toolset | 外部依赖太重 |
| A2A 协议集成 | 保留为远程 Agent 存根 |
| 声明式配置（完整版） | 用最小 YAML 加载器替代工厂注册表 |

### 5.10 关键教学简化

- 省略并行函数调用合并 → 串行执行工具
- 省略 `rearrangeEventsFor*` 复杂度 → 线性事件→内容转换
- 省略 `pluginManagerFromContext` 模式 → 直接在 Agent 配置注册回调
- 省略 Partial/PartialArgs 增量 → 仅处理完整函数调用

---

## 6. implementation_dag

以下是 `rive-adk-go` 中实现 Agent Flow 的 DAG 计划。每个节点是一个独立的实现任务，边表示依赖关系。

```
Phase 1: 核心类型
├── D01: 定义 core 类型（Tool, LLM, LLMRequest, LLMResponse, Event）
└── D02: 定义 Agent 接口（Agent, Run, SubAgents）

Phase 2: ReAct 循环
├── D03: 实现 Flow.Run（核心循环：runOneStep → 检查 Final → 重复）
│   └── 依赖: D01, D02
├── D04: 实现 Flow.runOneStep（Preprocess → callLLM → Postprocess → handleFCs）
│   └── 依赖: D03
└── D05: 实现 handleFunctionCalls（工具查找 → 回调链 → 串行执行 → 结果合并）
    └── 依赖: D04

Phase 3: 请求处理器管道
├── D06: 实现 BasicConfigProcessor
│   └── 依赖: D04
├── D07: 实现 ToolLoaderProcessor
│   └── 依赖: D04
├── D08: 实现 InstructionsProcessor（含 {var} 模板解析）
│   └── 依赖: D04
├── D09: 实现 IdentityProcessor
│   └── 依赖: D04
├── D10: 实现 ContentsProcessor（事件过滤 → 外来转换 → 异步对齐）
│   └── 依赖: D04
├── D11: 存根 NlPlanningProcessor（空实现 + TODO）
│   └── 依赖: D04
└── D12: 存根 CodeExecutionProcessor（空实现 + TODO）
    └── 依赖: D04

Phase 4: 模型适配器
├── D13: 实现 Gemini 适配器（包装 genai SDK，含流式聚合）
│   └── 依赖: D01
└── D14: 存根 OpenAI 适配器（展示多模型扩展点）
    └── 依赖: D01

Phase 5: 策略插件
├── D15: 实现 Plugin 结构体（12 个钩子字段 + 注册顺序执行）
│   └── 依赖: D03
├── D16: 实现 RetryAndReflect 插件（计数 map + 反射/超限模板）
│   └── 依赖: D15
├── D17: 实现 FunctionCallModifier 插件（BeforeModel 注入 + AfterModel 剥离）
│   └── 依赖: D15
└── D18: 实现 LoggingPlugin（参考实现，连接所有钩子）
    └── 依赖: D15

Phase 6: 多 Agent 路由
├── D19: 实现 ParentMap 构建与验证（无重复名称、无多重父级）
│   └── 依赖: D02
├── D20: 实现 TransferToAgentTool（动态 Declaration + 目标枚举）
│   └── 依赖: D19
├── D21: 实现 AgentTransferProcessor（注入转移工具 + 转移指令）
│   └── 依赖: D20
├── D22: 实现转移执行（runOneStep 中调用 target.Run(ctx)）
│   └── 依赖: D21, D04
├── D23: 实现 findAgentToRun（反向扫描 + isTransferableAcrossAgentTree）
│   └── 依赖: D19
└── D24: 实现 SingleFlow 检测 + 事件身份 + 状态隔离
    └── 依赖: D23

Phase 7: 工作流 Agent
├── D25: 实现 SequentialAgent
│   └── 依赖: D02
├── D26: 实现 ParallelAgent
│   └── 依赖: D02
└── D27: 实现 LoopAgent + ExitLoopTool
    └── 依赖: D02

Phase 8: 配置与演示
├── D28: 实现 YAML Config Loader（react/sequential/parallel 三种类型）
│   └── 依赖: D03, D25, D26
├── D29: 实现 Web Server（/chat + SSE + HTML UI）
│   └── 依赖: D03
└── D30: 编写集成示例（单 Agent、多 Agent、管道、Web 演示）
    └── 依赖: D03, D25, D26, D29

边汇总：
  D03 → D01, D02
  D04 → D03
  D05 → D04
  D06-D12 → D04
  D13-D14 → D01
  D15 → D03
  D16-D18 → D15
  D19 → D02
  D20 → D19
  D21 → D20
  D22 → D21, D04
  D23 → D19
  D24 → D23
  D25-D27 → D02
  D28 → D03, D25, D26
  D29 → D03
  D30 → D03, D25, D26, D29
```

### 关键路径分析

最长路径（必须串行完成的节点序列）：

```
D01 → D03 → D04 → D22 → D30
```

关键路径上的每个节点：
- **D01/D02**（类型定义）：所有节点的基础，约 2-3 小时
- **D03/D04**（ReAct 循环 + runOneStep）：核心引擎，约 4-6 小时
- **D22**（转移执行）：让转移端到端工作，约 2-3 小时
- **D30**（集成示例）：端到端验证，约 2-3 小时

估算关键路径总时长：10-15 小时（单人）。

### 并行机会

以下组可并行实现：

- **Phase 3**（D06-D12）：8 个请求处理器可独立开发，仅需 D04 完成
- **Phase 4**（D13-D14）：模型适配器与处理器并行
- **Phase 5**（D16-D18）：三个插件可独立实现，仅需 D15 完成
- **Phase 7**（D25-D27）：三个工作流 Agent 可独立实现

---

## 7. risks_and_open_questions

### 7.1 流式聚合的正确性风险

`streamingResponseAggregator` 的状态机复杂度高（文本缓冲、思考边界检测、增量 JSON 累积）。在 `rive-adk-go` 中若先实现非流式路径而后添加流式支持，需特别注意：
- PartialArgs 的 JSON Path 语义在不同 LLM 提供商（Gemini vs OpenAI vs Anthropic）间不一致
- 思考/非思考边界检测依赖模型特定行为
- **缓解**：先仅支持非流式模式，流式聚合作为 Phase 9 独立添加

### 7.2 会话历史重建的边界情况

`contents_processor.go` 的重排逻辑隐含大量边界情况：
- 长时间异步工具调用可能在事件流中跨越多轮用户消息
- 外来 Agent 事件的角色转换（Agent → user-role）可能与某些模型的 context window 策略冲突
- EUC 凭证请求和工具确认的内部过滤逻辑可能遗漏边缘情况
- **缓解**：使用简单线性事件→内容映射作为初始实现，渐进增强

### 7.3 并行函数调用的合并复杂度

`mergeParallelFunctionResponseEvents` 需同时合并 Parts、Actions（StateDelta、TransferToAgent、ArtifactDelta、SkipSummarization、Escalate）。多个工具可能设置冲突的 Actions 值。
- **缓解**：初始实现使用串行工具执行，专注于正确性而非并行性

### 7.4 回调短路与错误传播的交互

当 `BeforeModelCallback` 返回非 nil 响应短路模型调用时，`AfterModelCallbacks` 是否仍应运行？若 `BeforeToolCallback` 短路且返回的结果是错误的，`OnToolErrorCallbacks` 是否运行？
- ADK Go 的当前行为：Before 短路后，After 不运行。但错误回调（OnError）的行为因层而异
- **风险**：在 `rive-adk-go` 中不精确复现回调顺序可能导致插件（尤其是 RetryAndReflect）行为异常
- **缓解**：编写清晰的状态转换表文档化每个生命周期点的回调顺序

### 7.5 转移反转（bounce-back）的语义正确性

`findAgentToRun` + `isTransferableAcrossAgentTree` 的组合意味着：一旦 Agent 转移到一个 SingleFlow 叶子 Agent，下一个用户消息将"弹回"到可转移的祖先。但若祖先链中的中间 Agent 被删除或重命名，行为可能不正确。
- **缓解**：ParentMap 在初始化时构建且不可变，避免运行时的拓扑变化

### 7.6 NL Planning 和 Code Execution 的缺失

ADK Go 中这两个处理器均为空存根。ADK Python 有完整实现。在 `rive-adk-go` 复现中：
- NL Planning 需要设计多步推理的提示模板和规划解析器
- Code Execution 需要安全沙箱（子进程、Docker、gVisor）
- **决策**：保留处理器槽位，标记为 "TODO: implement NL planning using prompt template X"，展示扩展点设计模式但不实现完整功能

### 7.7 声明式配置的工厂注册表范围

`configurable/` 中的工具注册表未包含所有已实现的工具（如 `skill_toolset`、`preload_memory` 等）。这表明 ADK Go 的 configurable 子系统仍在向 Python ADK 的成熟度追赶。
- **风险**：在 `rive-adk-go` 中实现完整的工厂注册表可能过多投入 DX 功能
- **缓解**：使用最小化 YAML 加载器（`type: react/sequential/parallel`），不实现通用工厂注册表

### 7.8 状态隔离与 Agent 身份

当 `nextAgent.Run(ctx)` 在转移中被调用时，原始 Agent 的 `maybeSaveOutputToState` 需要检查 `event.Author != a.Name()` 以防止状态泄漏。若新事件没有正确的 Author 字段，可能静默地将数据写入错误的 Agent 状态键。
- **缓解**：在事件创建点（`finalizeModelResponseEvent`、`handleFunctionCalls`）强制设置 Author；编写测试验证

### 7.9 工具类型碎片化的简化策略

ADK Go 的 `Tool` vs `Toolset` 分离（`Toolset` 额外实现 `RequestProcessor`）对于理解工具如何在请求流水线中参与至关重要。简化：
- 将 `Toolset` 合并到 `Tool` 接口（作为可选的 `ProcessRequest(ctx, req) error` 方法）
- 或明确文档化两种类型的区别和使用场景

### 7.10 开放问题

1. **Agent-as-Tool vs 转移的区别**：何时使用 Agent-as-Tool（`agenttool.New`）而非 transfer_to_agent？前者在单个 ReAct 循环内将子 Agent 作为工具调用；后者移交控制权。两者的交互模式、事件 Authorship、状态继承方式不同。需要在文档中明确比较。

2. **Workflow Agent 的嵌套深度限制**：无限嵌套 SequentialAgent → LoopAgent → ParallelAgent → LLMAgent 可能导致状态爆炸。需要定义最佳实践和性能上限。

3. **跨 Agent 的记忆共享**：当前 `PreloadMemoryTool` 按用户查询文本搜索记忆；在转移场景中，目标 Agent 是否也能访问原始用户的记忆上下文？文档未明确。

4. **流式模式下的转移行为**：在 Live（双向流式）模式中，如果 Agent A 正在流式响应时发生转移，客户端接收的事件序列是什么？ADK Go 的处理是在 `RunLive` goroutine 中内联处理，但语义复杂。

5. **重放/记录一致性的测试策略**：ADK Go 的 conformance 插件（Replay/Record）依赖于确定性的事件序列。任何引入非确定性（goroutine 调度、map 迭代顺序）的变更都会破坏测试。`rive-adk-go` 需要类似的测试策略吗？

---

> **文档版本**：基于 ADK Go（截至 2026-06）源码分析合成
> **源报告**：
> - 01-llm-agent-react-loop.md（ReAct 循环与工具执行流程）
> - 02-transfer-multi-agent-routing.md（转移与多 Agent 路由）
> - 03-planner-reflection-skills.md（策略插件与技能系统）
> - 04-examples-configurable-patterns.md（示例与声明式配置模式）
> **目标仓库**：`rive-adk-go` Chapter 07 实现指南
