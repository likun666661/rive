# 第五部分：Workflow Agents、多智能体编排与远程 A2A

## Workflow 模式地图

```
┌──────────────────────────────────────────────────────────────────────┐
│                    ADK Multi-Agent Orchestration                      │
├──────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌─────────────────────────┐    ┌──────────────────────────┐        │
│  │   IN-PROCESS WORKFLOW   │    │   CROSS-PROCESS REMOTE   │        │
│  │        AGENTS           │    │        A2A AGENTS        │        │
│  ├─────────────────────────┤    ├──────────────────────────┤        │
│  │                         │    │                          │        │
│  │  SequentialAgent        │    │  remoteagent/v2          │        │
│  │  ├─ SubAgents[0].Run()  │    │  ├─ AgentCardProvider    │        │
│  │  ├─ SubAgents[1].Run()  │◄───│  ├─ A2AClient (HTTP/RPC)│        │
│  │  └─ SubAgents[N].Run()  │    │  └─ EventConverter       │        │
│  │                         │    │                          │        │
│  │  ParallelAgent          │    │  adka2a/v2 (Server)      │        │
│  │  ├─ SubAgents[0].Run() ─┤    │  ├─ Executor             │        │
│  │  ├─ SubAgents[1].Run() ─┤    │  ├─ EventProcessor       │        │
│  │  └─ errgroup.Wait()     │    │  └─ PartConverters       │        │
│  │                         │    │                          │        │
│  │  LoopAgent              │    │  adka2a (Legacy)         │        │
│  │  ├─ for i < MaxIter:    │    │  └─ v0→v1 compat shims   │        │
│  │  │   for sa in Subs:    │    │                          │        │
│  │  │     sa.Run()         │    │                          │        │
│  │  └─ exit if Escalate    │    │                          │        │
│  │                         │    │                          │        │
│  └─────────────────────────┘    └──────────────────────────┘        │
│                                                                      │
│  ┌──────────────────────────────────────────────────────┐            │
│  │               AGENT-AS-TOOL BRIDGE                    │            │
│  ├──────────────────────────────────────────────────────┤            │
│  │  agenttool.New(agent, config)                        │            │
│  │  ├─ tool.Tool interface (Name, Desc, IsLongRunning)  │            │
│  │  ├─ Declaration() → FunctionDeclaration(InputSchema) │            │
│  │  ├─ ProcessRequest() → PackTool(req, t)              │            │
│  │  └─ Run() → runner.New() → subAgent.Run() → result   │            │
│  └──────────────────────────────────────────────────────┘            │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

---

## 1. 面临的问题是什么

### 1.1 Sequential/Parallel/Loop Workflow Agents 解决的多智能体编排问题

**问题：** 当单个 LLM agent 无法完成复杂任务时，需要将多个 agent 组合成管道 (pipeline)，并控制执行顺序、并发和迭代。

| Agent 类型 | 解决的问题 | 源文件 |
|---|---|---|
| `sequentialagent` | **顺序管道编排**：多个 LLM agent 需要按固定顺序执行，前一个 agent 的输出作为下一个 agent 的输入。典型场景：代码生成 → 代码审查 → 代码重构。 | `agent/workflowagents/sequentialagent/agent.go:46` |
| `parallelagent` | **并发编排**：多个 agent 需要同时从不同角度解决同一问题，如多算法并发、多候选回答生成 + 评估。 | `agent/workflowagents/parallelagent/agent.go:44` |
| `loopagent` | **迭代编排**：重复运行 agent 直到达到终止条件（最大迭代次数或子 agent 发出 escalation 信号）。场景：逐步细化代码。 | `agent/workflowagents/loopagent/agent.go:45` |

**核心设计决策：** 所有 workflow agent 通过 `agent.Config.Run` 字段注入自定义 `Run` 函数，但**禁止用户同时设置 `Run` 和 workflow agent**（`agent/workflowagents/sequentialagent/agent.go:47-49`）。这保证了编排逻辑的统一性。

### 1.2 Agent-as-Tool 解决的多智能体编排问题

**问题：** 不同 agent 可能使用不同的 LLM 模型或工具集；`genai` 框架限制某些工具无法共存于同一个 LLM agent 中。需要一种机制让主 agent 像调用函数工具一样调用子 agent。

`agenttool` 将任意 `agent.Agent` 包装为 `tool.Tool`（`tool/agenttool/agent_tool.go:54`），使得：
- 主 agent 通过 LLM 的 function calling 机制触发子 agent
- 子 agent 的输入/输出 schema 被提取为 function declaration 的参数/返回类型
- 子 agent 在独立的 runner/session/artifact/memory 中沙箱化执行

### 1.3 Remote A2A 解决的多智能体编排问题

**问题：** Agent 运行在不同的进程或机器上；网络边界使得内存共享不可行，需要标准化的远程通信协议。

`remoteagent` 实现 A2A (Agent-to-Agent) 协议客户端（`agent/remoteagent/v2/a2a_agent.go:154-193`），`adka2a` 实现服务端（`server/adka2a/v2/executor.go:140-159`），共同解决：
- **跨进程/跨主机通信**：通过 JSON-RPC over HTTP 透明地调用远程 agent
- **协议兼容**：session.Event ↔ A2A Event/Messaage/Artifact 的双向转换
- **状态追踪**：通过 A2A Task 生命周期管理远程调用的提交、进行中、等待输入、完成、失败
- **次级 agent 代理**：server 端通过递归查找 remote sub-agents (`server/adka2a/v2/utils.go:38`) 支持多跳 A2A 级联

---

## 2. 为什么这是问题

### 2.1 子 Agent 之间状态管理的复杂性

子 agent 之间需要共享状态但又要保持隔离，这涉及多个层面：

**Session 状态传播：**

| 组件 | 策略 | 源文件 |
|---|---|---|
| `parallelagent` | 每个子 agent 获得**独立的 `InvocationContext`**（复制 artifacts/memory/session），但共享同一 session ID | `agent/workflowagents/parallelagent/agent.go:83-92` |
| `sequentialagent` | 子 agent 共享同一个 `InvocationContext`，session 事件是**累积**的 | `agent/workflowagents/sequentialagent/agent.go:80-87` |
| `agenttool` | 父 session 的 state 被过滤（排除 `_adk` 前缀的 key）后**复制**到全新的子 session | `tool/agenttool/agent_tool.go:182-198` |
| `remoteagent` | 远端 agent 通过 A2A `ContextID` 关联 session；远端 agent 看不到的 session events 被 `toMissingRemoteSessionParts` 收集并发送 | `agent/remoteagent/v2/a2a_agent.go:346-364` |

**Artifact 共享的风险：**
- `parallelagent` 通过 `icontext.NewInvocationContext` 复制 artifacts，但多 agent 并发写同一个 artifact 仍可能产生冲突（`agent/workflowagents/parallelagent/agent.go:83-84`）
- `adka2a` executor 提供两种 artifact 模式：`OutputArtifactPerRun`（单 artifact）和 `OutputArtifactPerEvent`（每事件一 artifact），后者通过按 author 追踪 partial artifacts 来隔离输出（`server/adka2a/v2/task_artifact.go:26-68`）

### 2.2 事件流的复杂性

**多源事件合并：**
- `parallelagent` 最复杂：所有子 agent 的事件通过 `resultsChan` 多路复用，使用 `ackChan` 实现**反向压力**——每 yield 一个事件后等待 runner 确认，防止通道爆炸（`agent/workflowagents/parallelagent/agent.go:73,143-146`）
- `sequentialagent` 最简单的：直接遍历子 agent 的 iterator，依次 yield（`agent/workflowagents/sequentialagent/agent.go:81-86`）

**事件转换：**
- `adka2a` executor 的 `eventProcessor.process()` 将 `session.Event` 转换为 `a2a.TaskArtifactUpdateEvent`（`server/adka2a/v2/processor.go:72`）
- 5 种事件类型：正常 → `TaskStateCompleted`；LLM 错误 → `TaskStateFailed`；长耗时工具 → `TaskStateInputRequired`；text → artifact parts；function calls/responses → `adk_type` metadata
- Partial 事件在两种输出模式下处理方式不同（`server/adka2a/v2/task_artifact.go:70-115` vs `26-68`）

### 2.3 错误处理的复杂性

**错误传播链：**
1. `loopagent`：子 agent 错误直接 yield 给调用方；escalation 是唯一内置的循环退出条件（`agent/workflowagents/loopagent/agent.go:88-90`）
2. `parallelagent`：`errgroup` 收集所有 goroutine 的 error，通过 `resultsChan` 传播（`agent/workflowagents/parallelagent/agent.go:101-110`）
3. `sequentialagent`：错误 yield 后调用方可停止迭代（`agent/workflowagents/sequentialagent/agent.go:82-84`）
4. `remoteagent`：A2A 错误转为 `session.Event.ErrorMessage`；`cleanupRemoteTask` 负责取消远程 task（`agent/remoteagent/v2/a2a_agent.go:306-343`）
5. `agenttool`：三重错误检查——iterator error、event 中的 ErrorCode/ErrorMessage、output validation error（`tool/agenttool/agent_tool.go:206-246`）

**未观测的风险：**
- loopagent 的 TODO 注释："ensure consistency -- if there's an error, return and close iterator"（`agent/workflowagents/loopagent/agent.go:83`）表明错误一致性尚未完全保证
- agenttool 的 TODO："verify agent loop termination"（`tool/agenttool/agent_tool.go:200`）——agent 无限循环没有保护
- sequentialagent 有相同的 TODO 注释（`agent/workflowagents/sequentialagent/agent.go:82`）

### 2.4 协议兼容的复杂性

**A2A v0 → v2 迁移：**
- 存在两套 API：顶层 `adka2a` 包（v0 兼容层）和 `adka2a/v2` 包（当前版本）
- `agent/remoteagent/a2a_agent.go`（341 行）是 deprecated v0 兼容层，将所有 v0 类型转换后委托给 v2（`agent/remoteagent/a2a_agent.go:128-290`）
- 转换成本：`BeforeA2ARequestCallback` (v0) 使用 `*a2a.MessageSendParams`，v2 使用 `*a2a.SendMessageRequest`
- `server/adka2a/executor.go`（408 行）是同样模式的 legacy v0 executor wrapper

**Part 转换链：**
- A2A DataPart → GenAI Part 的映射：`function_call` / `function_response` / `code_execution_result` / `executable_code` 通过 `adk_type` metadata 标识（`server/adka2a/v2/parts.go:37-40`）
- 未知 type 的 DataPart 被包裹为 XML-like JSON 标签 `<a2a_datapart_json>` 以保持 roundtrip 安全（`server/adka2a/v2/parts.go:63-75`）
- 自定义 part 转换器（`A2APartConverter` / `GenAIPartConverter`）允许返回 `nil` 来表示**选择性删除**部分（`server/adka2a/v2/executor.go:46-54`）

**Metadata 双层前缀机制：**
- ADK → A2A: `"adk_"` 前缀（`server/adka2a/v2/metadata.go:44`）
- A2A → ADK: `"a2a:"` 前缀（`server/adka2a/v2/metadata.go:49`）
- 防止命名空间冲突。metadata 携带：citations, grounding, usage metadata, custom metadata, escalate flag, transfer-to-agent flag, partial flag, is-error-message flag（`server/adka2a/v2/metadata.go:29-40`）

---

## 3. 解决思路是什么

### 3.1 Workflow Agent Variants 的分层设计

```
┌──────────────────────────────────────────────────────┐
│                agent.Config                          │
│  Name, Description, SubAgents[], Run, ...           │
│  (统一入口)                                          │
├───────────────┬──────────────┬───────────────────────┤
│ Sequential    │  Parallel    │  Loop                 │
│ iterate subs  │  errgroup +  │  for{} iterate subs   │
│ in order      │  ackChan     │  + MaxIter + Escalate │
├───────────────┴──────────────┴───────────────────────┤
│            agent.New(cfg) → agent.Agent              │
│            (统一返回类型)                             │
└──────────────────────────────────────────────────────┘
```

**设计原则：**
1. **统一接口**：所有 workflow agent 返回 `agent.Agent`，调用方无需区分类型
2. **`Run` 函数注入**：workflow agent 的 `Run` 函数替换默认 LLM agent 的 `Run`，通过 `cfg.AgentConfig.Run = impl.Run` 注入（`agent/workflowagents/sequentialagent/agent.go:52`）
3. **类型标记**：每个 workflow agent 设置 `state.AgentType`（`TypeSequentialAgent`, `TypeParallelAgent`, `TypeLoopAgent`），允许后续逻辑按类型分发
4. **嵌套支持**：子 agents 可以是任意 agent 类型，包括其他 workflow agent（测试验证了嵌套 sequential，`sequentialagent/agent_test.go`）

### 3.2 Remote Agent Clients/Processors 的分层设计

```
┌──────────────────────────────────────────────────┐
│         remoteagent/v2/a2a_agent.go              │
│  A2AConfig → agent.Config → agent.Agent          │
│  (用户接口)                                       │
├──────────────────────────────────────────────────┤
│         remoteagent/v2/client.go                 │
│  A2AClient interface, A2AClientProvider          │
│  (传输抽象)                                       │
├──────────────────────────────────────────────────┤
│  a2a_agent_run_processor.go                      │
│  event conversion, partial aggregation,          │
│  callback execution                              │
│  (事件处理管道)                                    │
├──────────────────────────────────────────────────┤
│         remoteagent/v2/utils.go                  │
│  message building, part conversion,              │
│  session delta computation                       │
│  (协议辅助)                                       │
└──────────────────────────────────────────────────┘
```

**分层职责：**
1. **a2a_agent.go**：暴露 `NewA2A(cfg A2AConfig)`，管理 agent 生命周期：card 解析 → client 创建 → message 构建 → 远程调用 → 事件处理 → 清理
2. **client.go**：`A2AClient` 接口抽象 `SendMessage` / `SendStreamingMessage` / `CancelTask` / `Destroy`，`A2AClientProvider` 工厂允许注入自定义传输
3. **a2a_agent_run_processor.go**：`aggregatePartial` 处理 A2A 的 streaming artifact update 聚合逻辑——合并 adjacent text blocks by thought type，处理 append/lastChunk 标记，在 `TaskArtifactUpdateEvent` 到达时重置聚合（`agent/remoteagent/v2/a2a_agent_run_processor.go:62-117`）
4. **utils.go**：`toMissingRemoteSessionParts` 计算远端 agent 尚未看到的 session events——从后往前扫描到最后一个 remote agent response，将非用户事件重述为 user context message（`agent/remoteagent/v2/utils.go:128-161`）

### 3.3 A2A Server/Tool Bridge 的分层设计

```
┌──────────────────────────────────────────────────┐
│          adka2a/v2/executor.go                    │
│  Executor wraps adka2a.AgentExecutor              │
│  Execute / Cancel / Cleanup                       │
│  (核心调度)                                       │
├──────────────────────────────────────────────────┤
│  processor.go       executor_context.go           │
│  event → artifact    context w/ session access    │
│  eventProcessor      executorPlugin               │
│  (事件管道)          (会话注入)                    │
├──────────────────────────────────────────────────┤
│  events.go          parts.go        metadata.go   │
│  A2A ↔ ADK event    A2A ↔ GenAI     key prefix    │
│  (双协议转换)        part converter   管理         │
├──────────────────────────────────────────────────┤
│  input_required.go     task_artifact.go           │
│  long-running tool      artifact strategy          │
│  state management       per-run / per-event        │
│  (状态机)               (产物策略)                  │
└──────────────────────────────────────────────────┘
```

**Executor 执行流程** (`server/adka2a/v2/executor.go:161-239`):
1. A2A Message → GenAI Content (via `toGenAIContent`)
2. 创建 `executorPlugin`（注入 `BeforeRunCallback` 以捕获 session）
3. 通过 `RunnerProvider` 创建 runner
4. `BeforeExecuteCallback` hook
5. `HandleInputRequired`——如果上次是 `TaskStateInputRequired`，检查新消息是否提供所需输入
6. 如果是全新 task，yield `SubmittedTask`
7. `prepareSession`——get or create session
8. Yield `TaskStateWorking`
9. 根据 `OutputMode` 选择 `artifactMaker` 或 `legacyArtifactMaker`
10. `process()` 遍历 runner events，每 event 经 `eventProcessor.process()` 转换，可选 `AfterEventCallback`
11. `writeFinalTaskStatus`——调用 `AfterExecuteCallback`，yield final artifact reset + status update

**Agent-as-Tool Bridge** (`tool/agenttool/agent_tool.go`):
1. `Declaration()` 提取 LLM agent 的 `InputSchema` 作为 function declaration 的参数 schema
2. `ProcessRequest()` 将 agent-tool 的 function declaration 打包进 LLM request
3. `Run()` 创建独立的 runner/session/artifact/memory，复制父 session state，运行子 agent，提取最终文本
4. Output validation：如果子 agent 有 `OutputSchema`，JSON-parse 输出文本并验证
5. 返回 `map[string]any`——有 schema 时返回验证后的 map，否则返回 `{"result": text}`

---

## 4. adk-go 代码怎么落地

### 4.1 关键类型/函数/文件

#### Workflow Agents：工作流 Agent

| 文件 | 关键类型/函数 | 作用 |
|---|---|---|
| `agent/workflowagents/sequentialagent/agent.go:46` | `New(cfg Config) (agent.Agent, error)` | 构造函数 |
| `agent/workflowagents/sequentialagent/agent.go:78` | `(a *sequentialAgent) Run(ctx)` | 顺序执行子 agents |
| `agent/workflowagents/sequentialagent/agent.go:125` | `(a *sequentialAgent) RunLive(ctx)` | 多轮 Live 模式（注入 `task_completed` tool） |
| `agent/workflowagents/sequentialagent/agent.go:91` | `sequentialLiveSession` | Live session 代理（mutex 保护 active session 切换） |
| `agent/workflowagents/parallelagent/agent.go:44` | `New(cfg Config) (agent.Agent, error)` | 构造函数 |
| `agent/workflowagents/parallelagent/agent.go:67` | `run(ctx)` | errgroup + ackChan 并发执行 |
| `agent/workflowagents/parallelagent/agent.go:130` | `runSubAgent(ctx, agent, ch, done)` | 单个子 agent runner（反向压力 ack） |
| `agent/workflowagents/loopagent/agent.go:45` | `New(cfg Config) (agent.Agent, error)` | 构造函数 |
| `agent/workflowagents/loopagent/agent.go:75` | `(a *loopAgent) Run(ctx)` | 双循环：外层 for{}，内层遍历 sub-agents |

#### Remote A2A Client：远程 A2A 客户端

| 文件 | 关键类型/函数 | 作用 |
|---|---|---|
| `agent/remoteagent/v2/a2a_agent.go:156` | `NewA2A(cfg A2AConfig) (agent.Agent, error)` | 远程 agent 构造函数 |
| `agent/remoteagent/v2/a2a_agent.go:199` | `(a *a2aAgent) run(ctx, cfg)` | 核心运行循环：card→client→message→call→process |
| `agent/remoteagent/v2/a2a_agent.go:306` | `cleanupRemoteTask(ctx, cfg, card, client, lastEvent, cause)` | Task 清理（输入等待 / 取消 RPC） |
| `agent/remoteagent/v2/a2a_agent.go:62` | `NewAgentCardProvider(source)` | 从 URL 或文件路径解析 AgentCard |
| `agent/remoteagent/v2/a2a_agent_run_processor.go:40` | `a2aAgentRunProcessor` | 事件处理管道：转换 + 聚合 + 回调 |
| `agent/remoteagent/v2/a2a_agent_run_processor.go:62` | `aggregatePartial(ctx, a2aEvent, event)` | 流式 artifact 聚合 |
| `agent/remoteagent/v2/utils.go:93` | `toMissingRemoteSessionParts(ctx, events, cfg)` | 计算远端未见的 session delta |
| `agent/remoteagent/v2/utils.go:128` | `presentAsUserMessage(ctx, agentEvent)` | 将 agent 事件重述为用户上下文 |

#### A2A Server：A2A 服务端

| 文件 | 关键类型/函数 | 作用 |
|---|---|---|
| `server/adka2a/v2/executor.go:149` | `Executor` struct | A2A AgentExecutor 实现 |
| `server/adka2a/v2/executor.go:154` | `NewExecutor(config) *Executor` | 构造函数 |
| `server/adka2a/v2/executor.go:161` | `Execute(ctx, execCtx)` | 主执行方法（8 步流程） |
| `server/adka2a/v2/executor.go:242` | `Cancel(ctx, execCtx)` | 取消执行 |
| `server/adka2a/v2/executor.go:249` | `Cleanup(ctx, execCtx, result, cause)` | 执行后清理 |
| `server/adka2a/v2/processor.go:35` | `eventProcessor` | 事件→artifact 处理管道 |
| `server/adka2a/v2/processor.go:72` | `(p *eventProcessor) process(ctx, event)` | 单事件转换 |
| `server/adka2a/v2/events.go:80` | `ToSessionEventWithParts(ctx, event, converter)` | A2A Event → session.Event |
| `server/adka2a/v2/events.go:38` | `EventToMessage(event)` | session.Event → A2A Message |
| `server/adka2a/v2/parts.go:87` | `ToA2AParts(parts, longRunningToolIDs)` | GenAI Parts → A2A Parts |
| `server/adka2a/v2/parts.go:236` | `ToGenAIParts(parts)` | A2A Parts → GenAI Parts |
| `server/adka2a/v2/input_required.go:152` | `HandleInputRequired(reqCtx, content)` | 检查输入响应完整性 |
| `server/adka2a/v2/task_artifact.go:26` | `artifactMaker` (OutputArtifactPerEvent) | 新 artifact 策略 |
| `server/adka2a/v2/task_artifact.go:70` | `legacyArtifactMaker` (OutputArtifactPerRun) | Legacy artifact 策略 |
| `server/adka2a/v2/agent_card.go:33` | `BuildAgentSkills(agent)` | 生成 A2A AgentCard skills |
| `server/adka2a/v2/utils.go:38` | `findRemoteSubagents(root)` | 递归查找 remote sub-agents |
| `server/adka2a/v2/metadata.go:44` | `ToA2AMetaKey(key)` | ADK → A2A metadata key prefix |
| `server/adka2a/v2/metadata.go:49` | `ToADKMetaKey(key)` | A2A → ADK metadata key prefix |

#### Agent-as-Tool：Agent 作为工具

| 文件 | 关键类型/函数 | 作用 |
|---|---|---|
| `tool/agenttool/agent_tool.go:40` | `agentTool` struct | 包装 agent.Agent 为 tool.Tool |
| `tool/agenttool/agent_tool.go:54` | `New(agent, cfg) tool.Tool` | 构造函数 |
| `tool/agenttool/agent_tool.go:86` | `Declaration() *FunctionDeclaration` | 提取 InputSchema 作为 function schema |
| `tool/agenttool/agent_tool.go:121` | `Run(toolCtx, args)` | 沙箱化执行子 agent |
| `tool/agenttool/agent_tool.go:254` | `ProcessRequest(ctx, req)` | 将 agent-tool 注册到 LLM request |

### 4.2 典型执行流

#### 流程 1：顺序代码流水线

```
User Query → SequentialAgent.Run()
  → CodeWriterAgent.Run()  → 输出写入 state["generated_code"]
  → CodeReviewerAgent.Run() → 读取 {generated_code}，输出写入 state["temp:review_comments"]
  → CodeRefactorerAgent.Run() → 读取 {generated_code} + {temp:review_comments}，输出写入 state["refactored_code"]
→ Final Output
```

源文件：`examples/workflowagents/sequentialCode/main.go:123-133`

#### 流程 2：带反压的并行 Agent

```
ParallelAgent.run()
  → errgroup.Go(subAgent1)  ←── goroutine 1
  │   runSubAgent → agent.Run() → resultsChan ← event → wait ackChan
  │
  → errgroup.Go(subAgent2)  ←── goroutine 2
  │   runSubAgent → agent.Run() → resultsChan ← event → wait ackChan
  │
  → yield loop: for res := range resultsChan:
      yield(res.event)
      close(res.ackChan)  ←── signals goroutine to proceed
```

源文件：`agent/workflowagents/parallelagent/agent.go:67-128`

#### 流程 3：A2A 远程 Agent 流式执行

```
remoteagent/v2/a2a_agent.go:run()
  1. ResolveAgentCard → 获取远端 agent 能力
  2. ClientProvider → 创建 A2AClient
  3. newMessage → 构造 A2A Message（session delta）
  4-1. StreamingModeNone:
       SendMessage(req) → processEvent(result) → yield
  4-2. Streaming:
       for event := range SendStreamingMessage(req):
         processEvent:
           convertToSessionEvent → AfterCallbacks → aggregatePartial → yield
  5. Defer cleanupRemoteTask:
       如果非终端状态 && 非 Message → CancelTask RPC (5s timeout)
```

源文件：`agent/remoteagent/v2/a2a_agent.go:199-303`

#### 流程 4：A2A 服务端执行器

```
adka2a/v2/executor.go:Execute()
  1. A2A Message → GenAI Content (toGenAIContent)
  2. New executorPlugin (BeforeRunCallback captures session)
  3. RunnerProvider → runner.New()
  4. BeforeExecuteCallback hook (if configured)
  5. HandleInputRequired: 如果有 pending 长耗时 function call，验证新消息是否提供响应
  6. If new task → yield SubmittedTask
  7. prepareSession → get or create session
  8. Yield TaskStateWorking
  9. Choose artifactMaker / legacyArtifactMaker
  10. process():
      for event := range runner.Run():
        processor.process(event):
          convert parts → input-required handling → transform → yield TaskArtifactUpdateEvent
          Optional: AfterEventCallback
  11. writeFinalTaskStatus:
        Optional: AfterExecuteCallback
        Yield final artifact reset (LastChunk=true)
        Yield TaskStateCompleted / Failed / InputRequired
```

源文件：`server/adka2a/v2/executor.go:161-371`

### 4.3 测试覆盖

| 测试区域 | 测试文件 | 行数 | 覆盖的关键场景 |
|---|---|---|---|
| Sequential | `sequentialagent/agent_test.go` | 567 | 基本顺序执行、嵌套顺序、重复名称错误、RunLive tool 注入、Live session 路由 |
| Parallel | `parallelagent/agent_test.go` | 537 | 并发完成、context cancellation、agent error 传播、Gemini 集成 + tools、state sync |
| Loop | `loopagent/loop_test.go` | 378 | 无限循环、有界循环、多子 agent 循环、escalation 退出、escalation + skip summarization |
| Remote Agent | `v2/a2a_agent_test.go` | 1437 | ADK↔ADK、ADK↔A2A、callbacks、payload 构造、card 解析、error 处理、清理回调、part 转换 |
| Remote Agent Processor | `v2/a2a_agent_run_processor_test.go` | 328 | Partial 聚合、Task snapshot 重置、append/lastChunk 组合、artifact 排序 |
| Remote Agent Utils | `v2/utils_test.go` | 317 | Function call 匹配、Session parts 收集、Agent→User 消息重述 |
| Remote Agent E2E | `v2/a2a_e2e_test.go` | 1276 | Input-required、多跳 input-required、清理传播、单跳 final response、Gemini streaming、多跳 cancellation、结构化 error 传播 |
| Remote Agent Compat | `a2a_agent_compat_test.go` | 810 | v0 legacy executor、v0 callbacks、v0 cleanup propagation、auth context propagation |
| A2A Executor | `v2/executor_test.go` | 1025 | 无 message、malformed data、session setup failure、new/existing task、LLM error、cancel、session reuse、所有 callbacks、part converters、artifact-per-event 模式、RunnerProvider |
| A2A Processor | `v2/processor_test.go` | 570 | 15 种 process 场景 + artifact 更新链 + partial events 丢弃 |
| A2A Events | `v2/events_test.go` | 742 | 17 种事件转换场景 + nil result filtering |
| A2A Agent Card | `v2/agent_card_test.go` | 397 | LLM agent, workflow agents, nested sub-agents, pronoun replacement |
| A2A Parts | `v2/parts_test.go` | 201 | 8 种部分类型双向转换 + arbitrary DataPart roundtrip |
| A2A Metadata | `v2/metadata_test.go` | 132 | 6 种 metadata 场景 roundtrip |
| AgentTool | `agent_tool_test.go` | 367 | Declaration、无 schema declaration、输入验证、输出验证、成功调用、无 schema 调用、空响应、skip summarization |

**测试特点：**
- `httprr`（HTTP Record & Replay）用于 Gemini real-model 集成测试
- 大量使用 mock/fake：`FakeLLM`、`testutil.MockModel`、`spyAgent`、`testSessionService`、`testRunner`
- `parallelagent` 测试使用 `loopagent` 作为子 agent 来精确控制迭代次数
- E2E 测试覆盖了多跳 A2A 场景（两个 server 级联）

### 4.4 未读风险

1. **Error consistency across workflow agents**：两处 TODO 注释（`loopagent/agent.go:83` 和 `sequentialagent/agent.go:82`）表明在错误发生后 iterator 的关闭行为需要统一审核
2. **Agent loop termination**：`agenttool/agent_tool.go:200` 的 TODO 提示子 agent 可能无限循环而没有超时保护
3. **Parallel state sync race**：`parallelagent` 的 `runSubAgent` 通过 `ackChan` 实现反向压力，但 state 写入没有分布式锁，多个 agent 同时写入相同 state key 可能产生竞争
4. **v0→v2 compat deprecation**：`agent/remoteagent/a2a_agent.go` (341 lines) 和 `server/adka2a/executor.go` (408 lines) 是 legacy 代码，维护成本高，需要制定迁移时间表
5. **A2A cleanup completeness**：`cleanupRemoteTask` 跳过 `InputRequired` + `cause == nil` 的情况（`agent/remoteagent/v2/a2a_agent.go:335-337`），但 context cancellation 期间 `cause` 可能为 nil 而 context 已取消，是否应也取消 task？
6. **OutputKey state pollution**：`sequentialCode` 示例中 agent 使用 `OutputKey: "generated_code"` 写入 session state，但如果多个 agent 使用相同 key，后执行的 agent 会覆盖前一个 agent 的输出（没有命名空间隔离）
7. **Missing `RunLive` for loop/parallel agents**：LoopAgent 和 ParallelAgent 没有 `RunLive` 实现，意味着这些 workflow agent 无法用于双向流式场景
8. **Part converter nil semantics**：`A2APartConverter` 和 `GenAIPartConverter` 的 `nil` 返回值表示"intentionally dropped"，但错误返回值也允许继续处理——两者的语义在代码中未明确区分
9. **Artifact mode inconsistency**：`OutputArtifactPerRun`（legacy）使用两个 artifact ID 管理 partial/non-partial，而 `OutputArtifactPerEvent` 按 author 追踪——两者的行为差异可能导致迁移时的兼容性问题
10. **No automatic retry in remote agent**：`remoteagent` 对 A2A 调用没有内置重试机制；远端错误直接转换为 session error event 并 yield 给调用方，需要调用方自行处理重试逻辑

---

## 5. 五个深入追问

1. **State isolation model inconsistency**: `sequentialagent` 共享 `InvocationContext`（session events 累积），而 `parallelagent` 使用 `icontext.NewInvocationContext` 复制上下文（隔离），`agenttool` 创建全新 session 但复制 state。这三种不同的隔离模型各自的适用场景和风险是什么？adk-go 是否有计划统一隔离策略？

2. **LiveRun extensibility**: 目前只有 `sequentialagent` 实现了 `RunLive`。对于需要双向流式交互的多 agent pipeline（如多 agent 辩论、联合推理），LoopAgent 或 ParallelAgent 的 Live 模式如何设计？是否需要 `task_completed` tool 注入模式的泛化？

3. **Cross-workflow composition**: 当前示例仅展示了同类型 workflow agent 的嵌套（如嵌套 sequential），但实际场景可能需要 sequential → parallel → loop 的任意组合。adk-go 的类型系统（`agent.Agent` 接口）在理论上支持这种组合，但状态传递（session events/state/artifacts）在这种混合嵌套中是否仍然正确？

4. **A2A multi-hop event attribution**: 在多跳 A2A 场景中，每个 hop 的 `server/adka2a` executor 会创建新的 invocation metadata。如果 3-hop chain 中的第 2 个 agent 失败，调用者如何定位到具体是哪个 hop 的 agent 产生了错误？当前的 `eventMeta` 只包含当前 hop 的 `invocation_id` 和 `branch`，缺少完整的 hop 链路追踪。

5. **Agent-as-Tool vs Workflow Agent tradeoff**: `agenttool` 和 `sequentialagent` 都可以实现 agent 的顺序执行，但前者通过 LLM function calling 触发（更灵活但依赖 LLM 正确选择调用时机），后者通过确定性框架控制（更可靠但缺乏动态决策能力）。在实际生产环境中，如何在这两种模式间取舍？它们是否可以混合使用（如 sequential agent 中包含 agenttool）？

---

> Generated from reading ~30 files across 6 directories.
> Repository: google/adk-go
> Scope: agent/workflowagents/**, agent/remoteagent/**, tool/agenttool/**, server/adka2a/**, examples/workflowagents/**, examples/a2a/**
