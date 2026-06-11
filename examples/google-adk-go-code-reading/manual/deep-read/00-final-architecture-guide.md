# ADK Go 精读技术手册：最终架构总纲

> 阅读基线：`81a63d8feb7d713b1731f0c740d95574eb64dafa`
> 仓库：`google/adk-go`
> 生成方式：汇总 6 份精读报告 + 粗读总纲

---

## 1. executive_summary

### 一句话

ADK Go 是一个基于 **Go 1.23 push-iterator（`iter.Seq2`）** 构建的 Agent 开发框架，通过分层架构（Runner → Agent → Flow → Model/Tool）和统一的回调 / Plugin / Processor 管道，将多 Agent 编排、工具调用、状态管理、流式响应、人机协同确认、远程 A2A 通信及多云部署整合为单一可组合运行时。

### 三句话

1. **执行核心**：`Runner.Run` → `Agent.Run` → `llmAgent.run` → `Flow.Run` → `runOneStep` 形成主调用链。每次 `runOneStep` 执行 `preprocess（RequestProcessor 管道 13 个处理器）→ callLLM（Before/After/OnError 回调）→ postprocess（ResponseProcessor 管道）→ handleFunctionCalls（并行 goroutine + WaitGroup）→ agent transfer` 的完整循环，直到 LLM 返回 `IsFinalResponse() == true`。

2. **状态体系**：Session（对话历史 + KV State，支持 app:/user:/temp: 三级前缀）、Memory（跨 session 长期相关性/语义检索）、Artifact（版本化文件，支持 user: 命名空间）三者独立为 `Service` 接口，通过 `InvocationContext` 注入 agent 运行时；StateDelta 采用 **写穿（write-through）** 策略——状态修改同时写入 delta 和真实 state，无事务回滚。

3. **扩展与编排**：Callback/Plugin 双层扩展（Plugin 先于 Callback，均 early-exit）覆盖 Agent/Model/Tool 三个生命周期；Workflow Agents（Sequential/Parallel/Loop）通过 `iter.Seq2` 组合实现编排与背压；AgentTool 将子 Agent 封装为 Tool 在沙箱 session 中运行；Remote A2A 通过 A2A 协议实现跨进程 Agent 通信；入口层通过 `launcher.Config` + sublauncher 体系统一控制台、REST、A2A、Agent Engine、Cloud Run 等多种部署形态。

### 十句话

1. **统一 Agent 接口**（`agent/agent.go:43-52`）：所有 Agent 实现 `Run(InvocationContext) iter.Seq2[*session.Event, error]`，返回 Go 1.23 惰性迭代器，Runner 层按需拉取 event，天然支持流式。

2. **Runner 生命周期**（`runner/runner.go:131-268`）：Session 管理（get-or-create）→ `findAgentToRun`（基于 session history 路由到活跃 Agent，含 transfer chain 验证）→ 注入 InvocationContext → 迭代 Agent events → 持久化 non-partial events → 回调 Plugin.OnEvent。

3. **Flow 主循环**（`internal/llminternal/base_flow.go:101-654`）：`runOneStep` 是执行原子单元。`preprocess` 按固定顺序执行 13 个 RequestProcessor；`callLLM` 封装 BeforeModel + 真实 LLM 调用 + AfterModel + OnModelError；`handleFunctionCalls` 用 goroutine + WaitGroup 并行执行 tool，结果通过 `mergeParallelFunctionResponseEvents` 合并。

4. **Contents 构建**（`internal/llminternal/contents_processor.go:37-187`）：从 session events 提取对话历史作为 LLM Contents，进行 Branch 过滤（`strings.HasPrefix`）、外来 Agent 事件转换（`[agent_x] said:` 格式）、Transcription 聚合、Function Call/Response 重排——但目录中标注了 `TODO` 与 Python 版本不完全一致。

5. **State 三层作用域**（`session/session.go:163-176`）：`app:`（跨用户/会话共享）、`user:`（同用户跨会话）、`temp:`（仅当前 invocation 有效，持久化前 trim）。StateDelta 在同一 invocation 内通过 `maps.Copy` 覆盖合并，无原子 compare-and-swap 保证。

6. **工具系统统一抽象**（`tool/tool.go` + `internal/toolinternal/tool.go`）：`Tool` 是最小公共接口（Name/Description/IsLongRunning）；`FunctionTool` / `StreamingFunctionTool` 提供执行能力；`RequestProcessor.ProcessRequest` 提供声明注入点。七类工具来源（Go 函数、MCP、Gemini、AgentTool、Skill、内置基础设施、子 Agent）通过此分层接入。

7. **HITL 确认**（`internal/llminternal/functions.go:32-93` + `request_confirmation_processor.go:37-172`）：Tool 通过 `ctx.RequestConfirmation()` 生成 `adk_request_confirmation` 事件；`RequestConfirmationRequestProcessor` 在下轮请求中反向扫描 session events 找到用户确认/拒绝的 function response，调用 `handleFunctionCalls` 重新执行原 tool——**确认逻辑在 functiontool、mcptoolset、confirmationTool 三处几乎完全重复**（每处约 24 行）。

8. **Workflow Agent 编排**（`agent/workflowagents/`）：Sequential 按序迭代子 agent 的 iter.Seq2（11 行核心）；Parallel 用 errgroup + resultsChan + ackChan 实现并发 + 背压（runner 处理完才释放子 agent 产下一个 event）；Loop 支持无限循环 + `Actions.Escalate` 终止。

9. **A2A 远程通信**（`agent/remoteagent/v2/a2a_agent.go` + `server/adka2a/v2/executor.go`）：客户端解析 AgentCard、创建 A2A Client、增量同步 session（`toMissingRemoteSessionParts`）、通过 `SendStreamingMessage` 获取流式事件；服务端通过 Executor 将 A2A 消息转为 genai.Content、创建 Runner、将 session.Event 转为 A2A artifact/status event。Legacy v0 → v1 适配层已标记 Deprecated。

10. **入口层产品化**（`cmd/launcher/` + `server/adkrest/` + `server/adka2a/` + `server/agentengine/`）：`launcher.Config` 统一注入 Session/Artifact/Memory/AgentLoader/Plugin/Telemetry；Sublauncher 组合模式（console、web api、web a2a、web agentengine、web webui、pubsub、eventarc）；`adkgo deploy` 通过 go build + Dockerfile + gcloud 部署到 Cloud Run 或 Agent Engine。

---

## 2. architecture_map

### 2.1 总体架构

```
┌──────────────────────────────────────────────────────────────────────────┐
│                          ENTRYPOINT LAYER                                  │
│  cmd/launcher/    cmd/adkgo/    server/adkrest/    server/adka2a/        │
│  console / web    deploy        REST / SSE / WS    A2A JSON-RPC          │
│  (Launcher + SubLauncher 组合)   (Agent Engine)    (Executor)             │
├──────────────────────────────────────────────────────────────────────────┤
│                          RUNNER LAYER                                     │
│  runner/runner.go                                                         │
│  - Session 管理 (Create/Get/AppendEvent)                                  │
│  - findAgentToRun (session history routing + transfer chain 验证)         │
│  - PluginManager (BeforeRun/AfterRun/OnUserMessage/OnEvent)              │
│  - appendMessageToSession (blob→artifact 转换)                            │
│  - 事件持久化 (!Partial → sessionService.AppendEvent)                     │
├──────────────────────────────────────────────────────────────────────────┤
│                          AGENT LAYER                                      │
│  agent/agent.go        agent/llmagent/llmagent.go                        │
│  - Agent 接口: Run(InvocationContext) iter.Seq2[*Event, error]            │
│  - base agent: BeforeAgentCallbacks → a.run() → AfterAgentCallbacks      │
│  - llmAgent: 创建 Flow{Model, RequestProcessors, callbacks}              │
│  - OutputKey 机制 (maybeSaveOutputToState)                                │
│  - Agent Transfer (register transfer_to_agent tool)                      │
├──────────────────────────────────────────────────────────────────────────┤
│                          FLOW LAYER                                       │
│  internal/llminternal/base_flow.go                                        │
│  - Flow.Run: for { runOneStep; if IsFinalResponse() → return }            │
│  - runOneStep: preprocess → callLLM → postprocess →                      │
│                 finalizeModelResponseEvent → handleFunctionCalls          │
│                 → agent transfer                                          │
│  - callLLM: BeforeModelCallbacks → generateContent →                     │
│              OnModelErrorCallbacks → AfterModelCallbacks                  │
│  - handleFunctionCalls: goroutine + WaitGroup 并行执行 tool               │
├──────────────────────────────────────────────────────────────────────────┤
│                     PROCESSOR / CALLBACK LAYER                             │
│  RequestProcessors (13 个, 按序执行):                                     │
│    basic → tool → auth → confirmation → instruction → identity →        │
│    contents → nlPlanning → codeExecution → outputSchema →               │
│    agentTransfer → removeDisplayName                                      │
│  Callbacks (Plugin 先于直接注册, early-exit):                             │
│    Agent: Before/After | Model: Before/After/OnError |                   │
│    Tool: Before/After/OnError                                             │
├──────────────────────────────────────────────────────────────────────────┤
│                          MODEL / TOOL LAYER                               │
│  model/          tool/              tool/functiontool/                    │
│  LLM interface   Tool + Toolset     泛型 Go 函数                          │
│                  tool/mcptoolset/   tool/agenttool/                      │
│                  MCP 协议           Agent as Tool                        │
│                  tool/geminitool/   tool/skilltoolset/                   │
│                  Gemini 原生工具     SKILL.md 技能包                      │
├──────────────────────────────────────────────────────────────────────────┤
│                          STATE LAYER                                      │
│  session/          memory/           artifact/                            │
│  Service: Create   Service:          Service: Save/Load/                  │
│  Get/List/Delete/  AddSessionToMemory Delete/List/Versions                │
│  AppendEvent       SearchMemory                                           │
│  inmemory/database/ inmemory/vertexai  inmemory/gcsartifact               │
│  vertexai                                                                 │
├──────────────────────────────────────────────────────────────────────────┤
│                     WORKFLOW / REMOTE LAYER                               │
│  workflowagents/sequentialagent     agent/remoteagent/v2/                 │
│  workflowagents/parallelagent       A2A Client + runProcessor             │
│  workflowagents/loopagent           server/adka2a/v2/                     │
│  tool/agenttool/                    A2A Server Executor                   │
├──────────────────────────────────────────────────────────────────────────┤
│                          TELEMETRY LAYER                                  │
│  telemetry/          internal/telemetry/                                  │
│  OTel/GCP exporter    Span semantics: invoke agent / generate content /   │
│  setup + provider     execute tool / token usage / tool args/response     │
└──────────────────────────────────────────────────────────────────────────┘
```

### 2.2 关键关系

| 关系 | 说明 |
|---|--|
| **Runner → Agent** | Runner 通过 `agentToRun.Run(ctx)` 迭代 events，负责持久化 non-partial events。Agent 不感知 Runner 的存在。 |
| **Agent → Flow** | llmAgent 在 `run()` 中创建 `Flow` 实例，注入 Model、13 个 RequestProcessors、回调链。Flow 是纯内部实现，不暴露给用户。 |
| **Flow → Model** | 通过 `model.LLM` 接口调用 `GenerateContent`。BeforeModel/AfterModel/OnModelError 回调可拦截 / 改写 / 恢复 LLM 调用。 |
| **Flow → Tool** | `toolProcessor` 收集 Tools/Toolsets → `handleFunctionCalls` 通过 `toolsDict` 查找并分发到 `FunctionTool.Run()` 或 `StreamingFunctionTool.RunStream()`。 |
| **Agent → State** | 通过 `InvocationContext.Session()` 访问 Session/State/Memory。StateDelta 在 event 产出时通过 `AppendEvent` 合并到 Session。 |
| **Plugin ↔ Callback** | Plugin 在 Runner/Agent/Model/Tool 各层都有钩子，**总是先于直接注册的 Callback 执行**。都是 early-exit 模式。 |
| **Workflow ↔ Agent** | Workflow Agent（Sequential/Parallel/Loop）通过组合子 Agent 的 `Run(ctx)` 迭代器实现编排。AgentTool 将 Agent 封装为 Tool，在独立 Session 中运行。 |
| **Remote A2A ↔ Agent** | A2A Client 将远程 Agent 包装为本地 Agent。A2A Server Executor 接收 A2A 协议消息，创建 Runner 执行本地 Agent，将 session.Event 转为 A2A event。 |
| **Server ↔ Runner** | REST/A2A/Agent Engine Server 通过 `launcher.Config` 获取服务依赖，创建 `runner.Runner`，将协议消息转为 runner 参数，events 转为协议响应。 |

---

## 3. deep_read_index

### 3.1 运行时主循环（01-runtime-flow-deep-dive.md）

- **解决的问题**：一次用户请求如何自动转换为多轮 LLM 调用、工具调用、Agent 转移、事件输出的完整可观测执行链路。
- **为什么难**：多 Agent Tree + Transfer 的路由复杂性；Streaming/Live 两种模式的差异（Live 需时序缓冲）；Tool Call 并行执行 + 回调链 + Long Running Tool + HITL Confirmation；Contents 构建需 Branch 过滤 + 外来 Agent 事件转换 + Function Call/Response 重排 + Transcription 聚合。
- **核心设计**：`Runner → Agent → Flow` 三层迭代器模型；`runOneStep` 的 6 阶段流水线（preprocess → callLLM → postprocess → finalize → handleFunctionCalls → agent transfer）；13 个 RequestProcessor 管道；多层回调链（Plugin → User Callback，early-exit）。
- **关键文件**：`runner/runner.go:131-666`，`agent/agent.go:162-360`，`agent/llmagent/llmagent.go:361-474`，`internal/llminternal/base_flow.go:62-1376`，`internal/llminternal/contents_processor.go:37-187`，`internal/llminternal/agent_transfer.go:69-344`
- **最值得继续读的点**：Parallel Agent 的 branch 分配与 event 合并机制；Streaming Response Aggregator 的 Partial → 聚合 state machine；与 Python ADK Contents 处理的对齐进度。

### 3.2 状态生命周期（02-state-lifecycle-deep-dive.md）

- **解决的问题**：Session（对话历史 + KV State）、Memory（跨 Session 长期知识）、Artifact（版本化文件）三者的独立生命周期管理，以及 app:/user:/temp: 三级 State 作用域。
- **为什么难**：StateDelta 合并在多轮调用中需正确处理 app/user/temp 三种作用域；Artifact 版本号在 GCS 后端有已知竞态；Database 后端的 stale session 检测用微秒时间戳比较；代码重复——`localSession` + `state` + `events` + `trimTempDeltaState` + `updateSessionState` 在 inmemory/database/vertexai 三个包中完全重复。
- **核心设计**：独立 Service 接口 + 至少一种 in-memory 实现 + 云端后端（GORM/GCS/Vertex AI）；`InvocationContext` 统一注入三种状态服务；StateDelta 写穿策略（同时写入 delta 和真实 state）；`ExtractStateDeltas` / `MergeStates` 公共 helper。
- **关键文件**：`session/session.go:32-179`，`session/inmemory.go:39-472`，`session/database/service.go:71-527`，`session/vertexai/vertexai.go`，`memory/inmemory.go:54-141`，`artifact/inmemory.go:64-286`，`artifact/gcsartifact/service.go:94-293`，`internal/sessionutils/utils.go:58-74`
- **最值得继续读的点**：三处重复的 session 实现何时统一到 `sessioninternal` 包；Database stale session 检测在分布式部署下是否可靠；GCS artifact 竞态修复方案；Memory 何时引入 embedding/向量搜索。

### 3.3 工具系统（03-tool-system-deep-dive.md）

- **解决的问题**：将 Go 函数、MCP 服务、Gemini 原生 API、子 Agent、Skill 包、内置基础设施工具等七种不同来源的工具，统一为一套 `Tool` / `Toolset` 抽象，让 LLM Agent 无差别使用。
- **为什么难**：Schema 推断（泛型 → JSON Schema vs MCP ListTools vs Gemini 原生 Tool）差异巨大；Args/Result 编码需处理 typed nil 陷阱与基本类型包装；Streaming/Long-Running Tool 接口和生命周期不同；HITL Confirmation 是横切关注点，确认逻辑在三处几乎完全重复（各约 24 行）；MCP 需连接管理 + 自动重连 + 分页 + 重试；AgentTool 需独立 sandbox session。
- **核心设计**：`Tool`（最小公共接口）→ `FunctionTool` / `StreamingFunctionTool`（执行能力）+ `RequestProcessor`（声明注入）；`Toolset` 支持动态工具集合（依赖 `ReadonlyContext`）；`HandleFunctionCalls` 通过 `toolsDict` 分派到不同执行路径；`toolutils.PackTool` 统一声明注入。
- **关键文件**：`tool/tool.go:76-149`，`internal/toolinternal/tool.go:34-178`，`tool/functiontool/function.go:202-267`，`tool/mcptoolset/set.go` + `client.go:39-135` + `tool.go:45-174`，`tool/agenttool/agent_tool.go:86-251`，`internal/llminternal/base_flow.go:1012-1180`，`internal/llminternal/functions.go:32-93`，`internal/llminternal/request_confirmation_processor.go:37-172`
- **最值得继续读的点**：`confirmationToolset` 是否会和内建确认逻辑双重触发；Long Running Tool 的 TODO 实现方案；`toolProcessor` 中 `f.Tools` 的缓存失效；`RequireConfirmationProvider` 两种签名不一致的统一。

### 3.4 回调与插件（04-callback-plugin-deep-dive.md）

- **解决的问题**：在 Agent 运行时的多个关键节点（用户消息到达、Agent 启动/结束、LLM 调用前后、Tool 调用前后、错误发生）提供标准扩展点，支持可观测性、流量控制、请求/响应改写、错误恢复、状态管理、HITL 确认、指令注入。
- **为什么难**：State 写穿策略（立即持久化）导致无事务回滚；多个 Callback/Plugin 在同一 hook 点执行，优先级完全由注册顺序决定，无法显式声明依赖；Early-exit 机制意味着第一个返回非 nil 的 callback 胜出，后续被跳过；并行 Tool 执行时 StateDelta 合并静默覆盖同名 key；Plugin 和直接注册的 Callback 类型大量重复但生命周期不同。
- **核心设计**：三层权限模型（`InvocationContext` → `CallbackContext` → `ReadonlyContext`）实现最小权限原则；PluginManager 顺序执行 + early-exit 策略（Plugin 先于 Callback）；`CallbackContextState` 的 delta-prioritized 读取 + write-through 写入；`trackedArtifacts` 装饰器自动追踪 Save 版本号到 ArtifactDelta；Instruction 模板系统支持 `{state_var}`、`{artifact.name}`、`app:`/`user:`/`temp:` 前缀。
- **关键文件**：`agent/context.go:62-189`，`agent/callback_context.go:33-261`，`plugin/plugin.go:78-167`，`internal/plugininternal/plugin_manager.go:38-288`，`internal/llminternal/instruction_processor.go:72-231`，`plugin/loggingplugin/logging_plugin.go:44-63`，`plugin/functioncallmodifier/plugin.go:53-119`，`plugin/retryandreflect/plugin.go:147-181`
- **最值得继续读的点**：State().Set() 写穿行为是设计意图还是实现简化；Plugin 和 Callback 是否应统一；Configurable 层如何支持 Plugin；并行 Tool StateDelta 合并是否应有冲突检测。

### 3.5 多 Agent 编排与 A2A（05-workflow-a2a-deep-dive.md）

- **解决的问题**：多个 Agent 的顺序执行、并行执行、循环执行编排；将 Agent 封装为 Tool（AgentTool）；通过 A2A 协议实现跨进程远程 Agent 调用。
- **为什么难**：Parallel Agent 的 session 共享 + 背压（resultsChan/ackChan 双向同步） + 错误传播（errgroup 取消）；Sequential Live 模式需注入 task_completed tool + 动态路由切换；A2A 客户端需处理多种 event 类型（Task/TaskArtifactUpdateEvent/TaskStatusUpdateEvent） + partial 聚合 + 增量 session 同步；A2A 服务端需 GenAI ↔ A2A part 双向转换 + finalStatus/error state machine；Legacy v0 兼容层全程代理转换。
- **核心设计**：所有 Workflow Agent 遵循 `agent.New` + 替换 `Run` + 设置 `AgentType` 的统一模式；Parallel 通过 errgroup + resultsChan + ackChan 实现并发 + 背压；AgentTool 通过独立 InMemorySession + Runner 实现沙箱隔离；A2A Client 通过 `toMissingRemoteSessionParts` 增量同步 + `aggregatePartial` 处理 streaming chunks；A2A Server 通过 Executor 整体流程控制 + eventProcessor 做好转换。
- **关键文件**：`agent/workflowagents/sequentialagent/agent.go:78-204`，`agent/workflowagents/parallelagent/agent.go:67-164`，`agent/workflowagents/loopagent/agent.go:75-104`，`tool/agenttool/agent_tool.go:86-251`，`agent/remoteagent/v2/a2a_agent.go:156-344`，`agent/remoteagent/v2/a2a_agent_run_processor.go:62-173`，`agent/remoteagent/v2/utils.go`，`server/adka2a/v2/executor.go:161-371`，`server/adka2a/v2/events.go`
- **最值得继续读的点**：Parallel Agent 替换为外部 session service 时的并发安全；Loop Agent 的 Escalate + SkipSummarization 交互规则；AgentTool 的 artifact 转发（Python 版本已实现）；A2A protocol version negotiation。

### 3.6 入口层与部署（06-entrypoint-deploy-deep-dive.md）

- **解决的问题**：将同一 agent 运行时暴露到七类入口（本地 console、Web UI、REST API、A2A、Agent Engine、Cloud Run 部署、Agent Engine 部署）并保持语义一致；提供可观测性（OTel/GCP telemetry）；通过 CI 维持质量基线。
- **为什么难**：入口不是简单 adapter，而是多个生命周期和协议模型叠在一起；Cloud Run 部署依赖外部 gcloud 命令且缺乏 fake 集成测试；Agent Engine 部署需 Source Archive + ClassMethods 自省 + MemoryBank 配置；Web UI bundle 从 upstream main 拉取不可复现；Telemetry content capture 在生产中有 PII/secret 泄漏风险；Examples 角色易被误解为生产级样例。
- **核心设计**：`launcher.Config` 统一注入所有服务依赖；Sublauncher 组合模式（universal 路由 → 按 keyword 分发）；`adkgo deploy` 编译本地 entry point 为 linux/amd64 binary + distroless Dockerfile + gcloud 部署；Telemetry 分 public config（GCP/OTLP export）和 internal semantics（span 语义）两层。
- **关键文件**：`cmd/launcher/launcher.go`，`cmd/launcher/universal/universal.go`，`cmd/launcher/console/console.go`，`cmd/launcher/web/web.go`，`server/adkrest/controllers/runtime.go`，`server/adka2a/v2/executor.go`，`server/agentengine/`，`cmd/adkgo/internal/deploy/cloudrun/cloudrun.go`，`cmd/adkgo/internal/deploy/agentengine/agentengine.go`，`telemetry/config.go` + `telemetry/setup_otel.go`，`internal/telemetry/`
- **最值得继续读的点**：Cloud Run deploy 的 fake gcloud 集成测试；Agent Engine deploy 的 `.adkignore` 机制；Web UI vendored bundle 的可复现性；REST/A2A/Agent Engine 三套 streaming model 的统一 event schema。

---

## 4. cross_module_flows

### 4.1 用户请求到 LLM event 完整链路

```
User Message
  │
  ▼
runner.Runner.Run (runner/runner.go:131)
  ├─ sessionService.Get / Create → storedSession
  ├─ findAgentToRun(storedSession, msg) → agentToRun
  │   ├─ msg 是 function response → 找到原始 function call 的 agent
  │   ├─ 否则反向遍历 events → 获取最后活跃 agent → isTransferableAcrossAgentTree
  │   └─ 默认 fallback root agent
  ├─ parentmap/runconfig/plugininternal.ToContext
  ├─ icontext.NewInvocationContext (Session + Memory + Artifacts)
  ├─ appendMessageToSession
  │   ├─ pluginManager.RunOnUserMessageCallback
  │   ├─ 如 SaveInputBlobsAsArtifacts → blob→artifact 转换 + placeholder 替换
  │   └─ sessionService.AppendEvent(user event)
  ├─ pluginManager.RunBeforeRunCallback → 如有 early exit → return
  │
  └─ for event, err := range agentToRun.Run(ctx):
       ├─ pluginManager.RunOnEventCallback → 可修改 event
       ├─ if !event.Partial → sessionService.AppendEvent (持久化)
       └─ yield(event, err)

agent.agent.Run (agent/agent.go:162)
  ├─ telemetry.StartInvokeAgentSpan
  ├─ runBeforeAgentCallbacks
  │   ├─ PluginManager.RunBeforeAgentCallback
  │   └─ agent.beforeAgentCallbacks (逐个, early-exit)
  │   └─ 任一返回 Content → 创建 Event, EndInvocation, return
  ├─ a.run(ctx) → llmAgent.run

      agent/llmagent/llmagent.go:361 llmAgent.run
        ├─ icontext.NewInvocationContext (内部包装)
        ├─ 创建 Flow{Model, DefaultRequestProcessors, callbacks}
        └─ for event, err := range f.Run(ctx):
             ├─ maybeSaveOutputToState → 如 OutputKey ≠ "", 写入 StateDelta
             └─ yield(event, err)

          internal/llminternal/base_flow.go:101 Flow.Run
            └─ for lastEvent.IsFinalResponse() == false:
                 for event, err := range f.runOneStep(ctx):
                   └─ yield(event, err)

              runOneStep (base_flow.go:528)
                ├─ preprocess: 13 个 RequestProcessor 按序执行
                │   ├─ basicRequestProcessor → 克隆 GenerateContentConfig
                │   ├─ toolProcessor → 收集 Tools/Toolsets 到 f.Tools
                │   ├─ authPreprocessor → 认证预处理
                │   ├─ RequestConfirmationRequestProcessor → HITL 确认恢复
                │   ├─ instructionsRequestProcessor → 注入 SystemInstruction + GlobalInstruction
                │   │   ├─ InstructionProvider → 动态指令
                │   │   ├─ InjectSessionState → {state_var} / {artifact.name} / {app:key} 替换
                │   │   └─ 可选变量 {var?} → 不存在返回 ""
                │   ├─ identityRequestProcessor → Agent 身份
                │   ├─ ContentsRequestProcessor → 构建 LLM Contents
                │   │   ├─ 过滤: 跳过无 content/role/parts 事件
                │   │   ├─ Branch 过滤: strings.HasPrefix(invocationBranch, event.Branch+".")
                │   │   ├─ 排除: adk_request_credential / adk_request_confirmation
                │   │   ├─ 外来 agent 事件: "[agent_x] said:" 格式转换
                │   │   ├─ Transcription 聚合: 连续 partial → 单个 text content
                │   │   └─ Function Call/Response 重排
                │   ├─ nlPlanningRequestProcessor → NL Planning
                │   ├─ codeExecutionRequestProcessor → 代码执行
                │   ├─ outputSchemaRequestProcessor → Output Schema 工具
                │   ├─ AgentTransferRequestProcessor → 注册 transfer_to_agent tool
                │   └─ removeDisplayNameIfExists → 清理
                │
                ├─ callLLM (base_flow.go:722)
                │   ├─ PluginManager.RunBeforeModelCallback
                │   ├─ f.BeforeModelCallbacks (逐个, early-exit → 跳过 LLM)
                │   ├─ generateContent(ctx, f.Model, req, useStream)
                │   │   └─ m.GenerateContent → LLM response(s)
                │   │       ├─ streaming: partial responses trace-only
                │   │       └─ final response logged
                │   ├─ if error → PluginManager.RunOnModelErrorCallback
                │   │           → f.OnModelErrorCallbacks (可恢复为成功响应)
                │   ├─ PluginManager.RunAfterModelCallback
                │   └─ f.AfterModelCallbacks (逐个, early-exit → 替换响应)
                │
                ├─ postprocess: ResponseProcessors 按序执行
                ├─ finalizeModelResponseEvent → 构造 session.Event
                ├─ yield modelResponseEvent (Runner 层持久化 non-partial)
                │
                ├─ handleFunctionCalls (base_flow.go:1012)
                │   ├─ 提取 FunctionCalls
                │   ├─ goroutine + sync.WaitGroup 并行执行
                │   │   ├─ stop_streaming → 取消 Live 流式工具
                │   │   ├─ tool not found → newToolNotFoundError → OnToolErrorCallbacks
                │   │   ├─ StreamingFunctionTool (Live) → 注册异步执行
                │   │   ├─ StreamingFunctionTool (非 Live) → 阻塞收集 chunk → 合并
                │   │   └─ FunctionTool → callTool
                │   │       ├─ PluginManager.RunBeforeToolCallback
                │   │       ├─ f.BeforeToolCallbacks → tool.Run(ctx, args)
                │   │       │   ├─ typeutil.ConvertTo(mapArgs → TArgs)
                │   │       │   ├─ 确认检查 (RequireConfirmation/Provider)
                │   │       │   ├─ handler(ctx, TArgs) → TResults
                │   │       │   └─ typeutil.ConvertTo(TResults → map[string]any)
                │   │       ├─ if error → OnToolErrorCallbacks + PluginManager
                │   │       ├─ PluginManager.RunAfterToolCallback
                │   │       └─ f.AfterToolCallbacks
                │   └─ mergeParallelFunctionResponseEvents (深度合并 StateDelta)
                │
                ├─ generateRequestConfirmationEvent (如有 HITL 请求)
                └─ agent transfer: if TransferToAgent ≠ "" → 查找 → agent.Run(ctx)
```

### 4.2 Tool Call 到 Function Response

```
LLM 返回 FunctionCall {Name: "get_weather", Args: {"city": "Tokyo"}}
  │
  ▼
Flow.runOneStep → handleFunctionCalls(fnCalls, tools, resp, confirmations)
  │
  ├─ 1. 提取 fnCalls = resp.LLMResponse.Content.Parts[*FunctionCall]
  ├─ 2. createMergedSpan (如多个 call)
  │
  └─ 3. 对每个 fnCall 启动 goroutine:
       │
       ├─ 特殊 case: name == "stop_streaming" → liveSession.CancelAllStreamingTools()
       │
       ├─ curTool = toolsDict[fnCall.Name]
       │   ├─ 不存在 → newToolNotFoundError → runOnToolErrorCallbacks
       │   └─ 存在 →
       │       │
       │       ├─ StreamingFunctionTool? + LiveSession?
       │       │   ├─ Yes: 注册到 liveSession, 异步运行 + 实时推送 chunk
       │       │   │   └─ 结果通过 LiveSession.Send 送回模型
       │       │   └─ No (StreamingFunctionTool 但非 Live): 阻塞收集所有 chunk
       │       │       └─ 合并为 map[string]any{"result": concatenated}
       │       │
       │       └─ FunctionTool?
       │           └─ callTool(toolCtx, funcTool, fArgs):
       │               │
       │               ├─ toolCtx = callback_context.NewToolContext(ic, fnCallID, actions, confirmations)
       │               │
       │               ├─ PluginManager.RunBeforeToolCallback(toolCtx, curTool, fArgs)
       │               │   └─ 返回 non-nil → 跳过工具执行
       │               │
       │               ├─ f.BeforeToolCallbacks (逐个, early-exit)
       │               │
       │               ├─ curTool.Run(toolCtx, fArgs)
       │               │   │
       │               │   ├─ [functiontool]:
       │               │   │   ├─ ConvertTo(mapArgs → TArgs)
       │               │   │   ├─ 检查 ctx.ToolConfirmation()
       │               │   │   │   ├─ Confirmed=true → 跳过确认
       │               │   │   │   └─ Confirmed=false → ErrConfirmationRejected
       │               │   │   ├─ 无确认 → requireConfirmation? (静态 || Provider)
       │               │   │   │   └─ Yes → ctx.RequestConfirmation(hint, payload)
       │               │   │   │       → 写入 actions.RequestedToolConfirmations[fnCallID]
       │               │   │   │       → actions.SkipSummarization = true
       │               │   │   │       → return ErrConfirmationRequired
       │               │   │   └─ No → handler(ctx, TArgs) → TResults
       │               │   │
       │               │   ├─ [mcptoolset]:
       │               │   │   ├─ 同上确认逻辑 (重复实现)
       │               │   │   ├─ mcpClient.CallTool(ctx, params)
       │               │   │   │   └─ withRetry: session.CallTool → 失败 → refreshConnection → 重试
       │               │   │   └─ convert CallToolResult → map[string]any{"output": ...}
       │               │   │
       │               │   └─ [agenttool]:
       │               │       ├─ 输入/输出 Schema 校验
       │               │       ├─ 创建独立 InMemorySession + Runner
       │               │       ├─ 复制父 session 非内部状态
       │               │       ├─ subRunner.Run → 收集 lastEvent.Content
       │               │       └─ 返回 map[string]any{"result": text}
       │               │
       │               ├─ if error → PluginManager.RunOnToolErrorCallback
       │               │           → f.OnToolErrorCallbacks (可返回 result 恢复)
       │               │           → [RetryAndReflect Plugin]: 注入反思 prompt
       │               │
       │               ├─ PluginManager.RunAfterToolCallback
       │               └─ f.AfterToolCallbacks
       │                   └─ callbacks 写入 state delta 到 callbackContext
       │
       └─ 同步写入 functionResponseEvent (channel)

  ▼
mergeParallelFunctionResponseEvents
  ├─ SkipSummarization: OR
  ├─ TransferToAgent: last non-empty wins
  ├─ StateDelta: deepMergeMap (recursive map[string]any)
  └─ RequestedToolConfirmations: maps.Copy

  ▼
yield functionResponseEvent → Runner → AppendEvent → yield to caller
```

### 4.3 Session State / Artifact / Memory 完整生命周期

```
┌── INVOCATION START ───────────────────────────────────────────────────┐
│                                                                        │
│  runner.Runner.Run                                                     │
│    ├─ sessionService.Get(appName, userID, sessionID, NumRecentEvents)  │
│    │   ├─ [inmemory]: session from omap.Map + MergeStates(app,user,session)
│    │   ├─ [database]: GORM Query storageSession + storageEvent + mergeStates
│    │   └─ [vertexai]: errgroup 并发 GetSession + GetEvents → 组装     │
│    │                                                                   │
│    └─ icontext.NewInvocationContext                                    │
│        └─ 注入 Session, Memory, Artifact, Branch, Agent, InvocationID  │
│                                                                        │
│  ┌── AGENT RUN ─────────────────────────────────────────────────────┐ │
│  │                                                                   │ │
│  │  agent.Run(ctx)                                                   │ │
│  │    ├─ Session.State().Get(key)        ← 读取持久化 state          │ │
│  │    ├─ callbackContext.State().Set(k,v)  ← 调用方：(如 callback)  │ │
│  │    │   ├─ action.StateDelta[k] = v                                │ │
│  │    │   └─ session.State().Set(k,v)     ← 立即持久化 (write-through)│ │
│  │    │                                                                 │
│  │    ├─ callbackContext.Artifacts().Save(ctx, name, data)            │ │
│  │    │   ├─ artifact.Service.Save → version++                       │ │
│  │    │   └─ actions.ArtifactDelta[name] = version                  │ │
│  │    │                                                                 │
│  │    ├─ Flow.Run (LLM loop)                                         │ │
│  │    │   ├─ instructionsRequestProcessor                            │ │
│  │    │   │   └─ InjectSessionState:                                 │ │
│  │    │   │       ├─ {state_var} → Session.State().Get()             │ │
│  │    │   │       ├─ {artifact.name} → Artifact.Load() → Part.Text  │ │
│  │    │   │       ├─ {app:key} / {user:key} / {temp:key}            │ │
│  │    │   │       └─ {var?} → 可选，不存在返回 ""                    │ │
│  │    │   │                                                          │ │
│  │    │   ├─ tool 执行中:                                            │ │
│  │    │   │   ├─ ToolContext.SearchMemory(query)                     │ │
│  │    │   │   │   └─ memory.Service.SearchMemory                    │ │
│  │    │   │   │       ├─ [inmemory]: 分词 → checkMapsIntersect       │ │
│  │    │   │   │       └─ [vertexai]: MemoryBank.search              │ │
│  │    │   │   │                                                      │ │
│  │    │   │   └─ ToolContext.RequestConfirmation(hint, payload)      │ │
│  │    │   │       └─ actions.RequestedToolConfirmations[id] = ...   │ │
│  │    │   │                                                          │ │
│  │    │   └─ llmAgent.maybeSaveOutputToState                         │ │
│  │    │       └─ 如 OutputKey ≠ "" → actions.StateDelta[key] = text │ │
│  │    │                                                              │ │
│  │    └─ 产出 Event{Actions.StateDelta, Actions.ArtifactDelta}     │ │
│  │                                                                   │ │
│  └─────────────────────────────────────────────────────────────────┘ │
│                                                                        │
│  runner 层事件处理:                                                    │
│    if !event.Partial:                                                  │
│      sessionService.AppendEvent(sess, event)                           │
│        ├─ sess.appendEvent(event): updateSessionState + trimTempDelta │
│        │   ├─ maps.Copy(sess.state, event.Actions.StateDelta)  ← 含 temp
│        │   └─ trimTempDeltaState: 从 event.StateDelta 移除 temp: 前缀 │
│        ├─ ExtractStateDeltas → appDelta/userDelta/sessionDelta       │
│        ├─ updateAppState / updateUserState / maps.Copy(session.state) │
│        └─ 持久化 event + state 到存储后端                             │
│                                                                        │
│  MEMORY 生命周期:                                                      │
│    AddSessionToMemory(session)                                        │
│      ├─ 遍历 Session.Events()                                          │
│      ├─ 分词 (空格 split, 转小写)                                     │
│      └─ 按 (appName, userID, sessionID) 写入 store [覆盖写]          │
│    SearchMemory(query)                                                 │
│      └─ 分词 query → checkMapsIntersect → 返回匹配 []Entry            │
│                                                                        │
│  ARTIFACT 生命周期:                                                    │
│    Save(app,user,session,fileName,part)                                │
│      ├─ user: 前缀 → sessionID 替换为 "user" (跨 session)              │
│      ├─ find 最大版本号 → +1 → set                                    │
│      └─ [GCS]: listVersions → max+1 (已知竞态)                        │
│    Load(app,user,session,fileName,version)                             │
│      ├─ version=0 → find 最新                                         │
│      └─ version>0 → get 精确版本 → LoadResponse{Part}                 │
│    Delete / List / Versions                                            │
│                                                                        │
└── INVOCATION END ────────────────────────────────────────────────────┘
```

### 4.4 Callback / Plugin 改写 Model/Tool 行为

```
┌── FLOW.callLLM ──────────────────────────────────────────────────────────┐
│                                                                           │
│  1. PluginManager.RunBeforeModelCallback(ctx, req)                        │
│     │                                                                     │
│     ├─ [FunctionCallModifier Plugin]                                      │
│     │   └─ 遍历 req.Config.Tools[].FunctionDeclarations                  │
│     │       └─ 匹配 Predicate → maps.Copy 注入额外参数 schema            │
│     │       └─ 可选: 修改 description                                    │
│     │                                                                     │
│     ├─ [Logging Plugin]                                                   │
│     │   └─ 打印 model, system instruction, tools → 返回 nil,nil          │
│     │                                                                     │
│     └─ [Replay Plugin]                                                    │
│         └─ 匹配 req → 返回录制的 LLMResponse (跳过真实 LLM)              │
│                                                                           │
│  2. f.BeforeModelCallbacks (逐个, early-exit)                             │
│     └─ 任一返回 LLMResponse → 跳过 generateContent, 直接作为模型响应     │
│                                                                           │
│  3. generateContent(ctx, f.Model, req, useStream)                         │
│     └─ 真实 LLM 调用                                                      │
│                                                                           │
│  4. 如果 LLM 返回 error:                                                  │
│     ├─ PluginManager.RunOnModelErrorCallback(ctx, req, err)              │
│     └─ f.OnModelErrorCallbacks (逐个)                                     │
│         └─ 返回 LLMResponse → 错误恢复, 继续执行                          │
│                                                                           │
│  5. PluginManager.RunAfterModelCallback(ctx, resp, err)                  │
│     │                                                                     │
│     ├─ [FunctionCallModifier Plugin]                                      │
│     │   └─ 遍历 LLMResponse.Content.Parts 中的 FunctionCall              │
│     │       └─ 匹配 Predicate → 剥离添加的参数                           │
│     │       └─ 将参数值存入 ctx.State() (key: {fnCallID}/{argName})     │
│     │                                                                     │
│     ├─ [Logging Plugin]                                                   │
│     │   └─ 打印 content, token usage → 返回 nil,nil                      │
│     │                                                                     │
│     └─ [Replay Plugin]                                                    │
│         └─ cmp.Diff 验证响应与录制一致                                    │
│                                                                           │
│  6. f.AfterModelCallbacks (逐个)                                          │
│     └─ 返回 LLMResponse → 替换原始 LLM 响应                              │
│                                                                           │
└───────────────────────────────────────────────────────────────────────────┘

┌── FLOW.callTool ──────────────────────────────────────────────────────────┐
│                                                                           │
│  1. PluginManager.RunBeforeToolCallback(toolCtx, tool, args)              │
│     ├─ [Logging Plugin]: 打印 tool 名称、参数 → 返回 nil,nil             │
│     └─ [Replay Plugin]: 匹配 tool 调用 → 返回录制的 result               │
│     └─ 返回非 nil → 跳过 tool.Run()                                       │
│                                                                           │
│  2. f.BeforeToolCallbacks (逐个, early-exit)                              │
│     └─ 任一返回 result → 跳过 tool.Run(), 使用该 result                  │
│                                                                           │
│  3. tool.Run(toolCtx, fArgs)  ← 实际执行                                  │
│     └─ 见 4.2 Tool Call 到 Function Response                             │
│                                                                           │
│  4. 如果 tool 返回 error:                                                 │
│     ├─ PluginManager.RunOnToolErrorCallback(toolCtx, tool, args, err)    │
│     │   │                                                                 │
│     │   └─ [RetryAndReflect Plugin]                                       │
│     │       ├─ err == ErrConfirmationRequired/Rejected → 跳过 (不干预 HITL)│
│     │       ├─ currentRetries = counter[toolName] + 1                     │
│     │       ├─ currentRetries <= maxRetries →                             │
│     │       │   └─ 返回 createToolReflectionResponse()                    │
│     │       │       └─ 包含错误详情 + 当前参数 + 反思 prompt              │
│     │       └─ currentRetries > maxRetries →                              │
│     │           └─ 返回 createToolRetryExceedMsg()                        │
│     │           └─ 或 透传 error (errorIfRetryExceeded=true)              │
│     │                                                                     │
│     └─ f.OnToolErrorCallbacks (逐个)                                      │
│         └─ 返回 result → 错误恢复, 继续执行                               │
│                                                                           │
│  5. PluginManager.RunAfterToolCallback → f.AfterToolCallbacks            │
│     ├─ [Logging Plugin]: 打印 tool 结果 → 返回 nil,nil                   │
│     ├─ [RetryAndReflect Plugin]:                                          │
│     │   ├─ 成功且非反思响应 → 重置该 tool 的失败计数                      │
│     │   └─ 反思响应 → 保持失败计数                                        │
│     └─ 返回 result → 替换 tool 执行结果                                   │
│                                                                           │
└───────────────────────────────────────────────────────────────────────────┘

┌── AGENT.cmd/launcher ─────────────────────────────────────────────────────┐
│                                                                           │
│  Agent.Run:                                                               │
│    1. PluginManager.RunBeforeAgentCallback(ctx)                           │
│       └─ 返回 Content → 终止 agent run                                    │
│    2. agent.BeforeAgentCallbacks (逐个)                                   │
│       └─ 同上                                                             │
│       └─ 如果只有 StateDelta 无 Content → 也会产出 Event                 │
│    3. agent.run()                                                         │
│    4. PluginManager.RunAfterAgentCallback(ctx)                            │
│       └─ 返回 Content → 追加后置 Event                                    │
│    5. agent.AfterAgentCallbacks (逐个)                                    │
│       └─ 同上                                                             │
│                                                                           │
│  Runner 层:                                                               │
│    ├─ pluginManager.RunOnUserMessageCallback(ctx, *Content)              │
│    │   └─ 返回 *Content → 替换用户消息                                    │
│    ├─ pluginManager.RunBeforeRunCallback(ctx)                             │
│    │   └─ 返回 *Content → 终止 invocation，返回该 Event                   │
│    ├─ event 循环中: pluginManager.RunOnEventCallback(ctx, *Event)        │
│    │   └─ 返回 *Event → 替换 Event (修改/过滤)                           │
│    └─ 完成后: pluginManager.RunAfterRunCallback(ctx)                     │
│                                                                           │
└───────────────────────────────────────────────────────────────────────────┘
```

### 4.5 Workflow Agent / A2A 编排

详见 `05-workflow-a2a-deep-dive.md` 第 5 节 `orchestration_flows`，其中包含 5 条完整链路：

1. **Sequential Workflow**：主 goroutine 依次迭代每个子 agent 的 event iterator，共享同一 session，state 自然传递。
2. **Parallel Workflow**：errgroup 管理并发 goroutine → resultsChan + ackChan 背压机制 → 主 goroutine 按到达顺序 yield。
3. **Loop Workflow**：子 agent 按序迭代执行 → 每轮结束后检查 `Actions.Escalate` 或 count 计数器 → 支持无限循环。
4. **Agent-as-Tool**：独立 InMemorySession + Runner 沙箱 → 输入/输出 Schema 校验 → 阻塞返回 lastEvent text → LLM 收到 FunctionResponse。
5. **Remote A2A**：Client 侧解析 AgentCard → 增量 session 同步 → SendStreamingMessage → aggregatePartial (处理 Task/TaskArtifactUpdateEvent/TaskStatusUpdateEvent) → 转换为 session.Event。Server 侧 toGenAIContent → RunnerProvider → process (ADK event → A2A event) → writeFinalTaskStatus。

---

## 5. maintainer_questions

如果要维护或复刻 ADK Go，必须先回答以下关键问题清单：

### 5.1 架构设计决策

1. **为什么选择 Go 1.23 `iter.Seq2` 而非 channel？** iter.Seq2 是惰性求值，支持 consumer-driven backpressure，但它的 compose/delegate 模式在 parallel agent 中引入了复杂的 ackChan 同步。是否评估过 channel-based 方案？

2. **Plugin 和 Callback 的边界设计意图是什么？** 两者在所有 hook 点类型高度重复（BeforeModel、AfterTool 等），但 Plugin 多 Runner-level hooks 和 Close 生命周期。可否将 Callback 统一为 Plugin 的特例，减少双重维护？

3. **State 的 write-through 策略是设计意图还是实现简化？** Python ADK 中 StateDelta 采用延迟提交（回写）策略，Go 版本选择在 `Set()` 时立即持久化——这导致 callback 间无事务回滚能力。权衡原因是什么？

4. **Agent Transfer 逻辑为何分散在 Flow 和 Runner 两层？** `base_flow.go:638` 的 TODO 注明了设计意图不清晰。应该由谁持有 transfer 决策权？

5. **Tool 系统的 `RequireConfirmationProvider` 为什么有两种不同签名？** `func(TArgs) bool`（functiontool）vs `func(toolName string, toolInput any) bool`（MCP/tool 包级别）——是否应统一？

### 5.2 代码质量与重复

6. **三处重复的 session 实现何时统一？** `session/session.go:session`、`database/session.go:localSession`、`vertexai/session.go:localSession` 以及 `trimTempDeltaState`、`updateSessionState` 完全重复。所有文件都标注了 `TODO ... Move to sessioninternal`。

7. **确认逻辑在四处的重复如何解决？** `functionTool.Run`、`streamingFunctionTool.RunStream`、`mcpTool.Run`、`confirmationTool.Run` 中确认逻辑各约 24 行几乎完全相同。能否提取为公共函数？

8. **Database 后端的 `extractStateDeltas` 和 `mergeStates` 与 `sessionutils` 包独立维护**——是否有回归测试保证行为同步？

### 5.3 未完成的 TODO

9. **Long Running Tool 的完整生命周期**（`base_flow.go:1132` 的 TODO）计划如何实现？是否采用类似 Streaming Tool 的 goroutine + 异步注册模式？

10. **Contents 处理与 Python ADK 的差异**（`contents_processor.go:39` 的 TODO）——哪些 Python 功能已对齐，哪些是关键缺口？这对多语言 ADK 生态的互操作性有多大影响？

11. **Agent Transfer 职责混乱**（`agent_transfer.go:66` 的 TODO "implement it in the runners package"）——当前的 split-brain 设计带来了什么后果？

### 5.4 并发安全

12. **Parallel Agent 更换为外部 session service 时并发安全如何保证？** `session.InMemoryService` 用全局 RWMutex 保护 AppendEvent，但 database 后端用 GORM 事务 + 时间戳 stale 检测——两者语义不完全一致。

13. **Stale session 检测在分布式部署下是否可靠？** 用 `UnixMicro()` 比较时间戳，时钟不同步会导致误判。是否需要乐观锁版本号？

14. **并行 Tool Call 的 StateDelta 合并没有冲突检测**——两个 tool 同时修改同一 key 时静默覆盖。是否需要告警或策略选择（last-write-wins / merge / reject）？

### 5.5 维护和演进

15. **Legacy A2A v0 adapter 的移除计划是什么？** 所有调用委托给 v2 但增加了事件转换开销，何时可以安全移除？

16. **Examples 的角色和 taxonomy 是否需要重组？** 当前 examples 是 minimal testing examples 但被用户视为学习入口，需要按能力矩阵重组并标注非目标场景。

17. **Cloud Run / Agent Engine deploy 的 fake 集成测试何时添加？** 当前部署路径无法在不依赖真实 GCP 环境的情况下做回归测试。

18. **Web UI vendored bundle 的可复现性如何保证？** `update-adk-web.sh` 从 upstream main 拉取，不同时间生成不同 bundle。

19. **`toolProcessor` 中 `f.Tools` 缓存何时需要失效？** 当前仅在首次请求时收集 Tools，如果 Toolset 依赖 invocation state 动态变化，后续请求会遗漏新工具。

20. **是否需要一个正式的 "ADK Go SDK API compatibility policy"？** 当前 launcher keyword、接口签名等变更会直接影响下游 binary。

---

## 6. next_dag

建议下一轮 Rive DAG 包含以下节点和依赖关系：

```
Node 1: state-session-unification
  ├─ depends_on: []
  ├─ 目标: 统一 session/session.go、database/session.go、vertexai/session.go 的重复实现到 sessioninternal 包
  ├─ 产物:
  │   ├─ internal/sessioninternal/session.go (统一 localSession + state + events + trimTempDeltaState + updateSessionState)
  │   ├─ 三后端切换到共享实现
  │   └─ 验证三种后端 State() / AppendEvent 行为一致
  └─ 对应报告: 02-state-lifecycle-deep-dive.md §7.3

Node 2: confirmation-deduplication
  ├─ depends_on: []
  ├─ 目标: 提取重复的确认逻辑 (functiontool/mcptoolset/confirmationTool 三处) 为公共函数
  ├─ 产物:
  │   ├─ internal/toolinternal/confirmation.go (统一确认检查 + RequestConfirmation)
  │   ├─ 四处分发到统一实现
  │   └─ 验证 confirmationToolset 不会和内部确认双重触发
  └─ 对应报告: 03-tool-system-deep-dive.md §7.1

Node 3: state-delta-transactional
  ├─ depends_on: [state-session-unification]
  ├─ 目标: 评估/实现 StateDelta 从 write-through 切换到 write-back（延迟提交），支持事务性 callback 链
  ├─ 产物:
  │   ├─ design doc: write-back vs write-through 权衡
  │   ├─ 如可行: 在 Invocation 结束或 Event Append 时统一提交 Delta
  │   └─ 回归测试覆盖 parallel tool 合并场景
  └─ 对应报告: 04-callback-plugin-deep-dive.md §7.2, 02-state-lifecycle-deep-dive.md

Node 4: long-running-tool-lifecycle
  ├─ depends_on: []
  ├─ 目标: 完成 Long Running Tool 的完整生命周期管理 (base_flow.go:1132 TODO)
  ├─ 产物:
  │   ├─ design: 异步注册 + 结果重注入 step 循环
  │   ├─ 实现: handleFunctionCalls 中 LR Tool 的 goroutine + response matching
  │   └─ 测试: 跨 invocation 的 LR Tool 恢复
  └─ 对应报告: 01-runtime-flow-deep-dive.md §7.1, 03-tool-system-deep-dive.md §7.4

Node 5: contents-alignment-with-python
  ├─ depends_on: []
  ├─ 目标: 补齐 contents_processor.go TODO 中与 Python ADK 的差异
  ├─ 产物:
  │   ├─ 与 Python ADK 的 Contents 处理对比矩阵
  │   ├─ 补齐缺失功能 (如 function call results extraction)
  │   └─ 跨语言一致性集成测试
  └─ 对应报告: 01-runtime-flow-deep-dive.md §7.1

Node 6: parallel-agent-session-safety
  ├─ depends_on: [state-session-unification]
  ├─ 目标: 分析并加固 Parallel Agent 使用外部 session service 时的并发安全
  ├─ 产物:
  │   ├─ 并发模型分析: InMemory vs Database vs VertexAI session AppendEvent 语义差异
  │   ├─ StateDelta 合并冲突检测 (last-write-wins vs merge vs reject)
  │   └─ 测试: 多 goroutine 并发 AppendEvent + StateDelta 正确性
  └─ 对应报告: 05-workflow-a2a-deep-dive.md §7.1, 04-callback-plugin-deep-dive.md §7.7

Node 7: plugin-callback-unification
  ├─ depends_on: []
  ├─ 目标: 评估 Plugin 和 Callback 是否应统一为单一扩展机制
  ├─ 产物:
  │   ├─ design doc: 统一的 AgentDeveloperExtension 接口设计
  │   ├─ 如可行: 实现统一接口 + 迁移指南
  │   └─ 对比: 当前 Plugin+Callsack vs 统一 Extension 的复杂度
  └─ 对应报告: 04-callback-plugin-deep-dive.md §5.3, §7.3

Node 8: deploy-fake-integration-test
  ├─ depends_on: []
  ├─ 目标: 为 Cloud Run / Agent Engine deploy 路径添加不依赖真实 GCP 的 fake 集成测试
  ├─ 产物:
  │   ├─ fake gcloud command (编译 + Dockerfile 验证)
  │   ├─ fake ReasoningEngine client (archive + classMethods + memoryBank 验证)
  │   └─ CI 集成: 每次 PR 验证 deploy 路径不 drift
  └─ 对应报告: 06-entrypoint-deploy-deep-dive.md §tests

Node 9: examples-taxonomy
  ├─ depends_on: []
  ├─ 目标: 重组 examples 为能力矩阵 + 教学路线
  ├─ 产物:
  │   ├─ examples/README.md 按能力分类 (runtime basic / tools / workflow / streaming / web / cloud / telemetry)
  │   ├─ 每个 example 标注: 覆盖能力 / 非目标 / 生产注意事项
  │   └─ 自动检查: README 中列出的 launcher keyword 是否真实存在
  └─ 对应报告: 06-entrypoint-deep-deep-dive.md §risks.8

Node 10: tool-cache-invalidation
  ├─ depends_on: []
  ├─ 目标: 修复 toolProcessor 中 f.Tools 缓存不失效的问题
  ├─ 产物:
  │   ├─ 分析: 哪些 Toolset 需要动态刷新
  │   ├─ 实现: 缓存失效策略 (per-invocation / per-step / detector-based)
  │   └─ 测试: state 变化后 tool 列表正确更新
  └─ 对应报告: 03-tool-system-deep-dive.md §7.6
```

### DAG 视觉拓扑

```
state-session-unification ──┬── state-delta-transactional
                            └── parallel-agent-session-safety

confirmation-deduplication

long-running-tool-lifecycle

contents-alignment-with-python

plugin-callback-unification

deploy-fake-integration-test

examples-taxonomy

tool-cache-invalidation
```

10 个节点，其中 `state-session-unification` 被 `state-delta-transactional` 和 `parallel-agent-session-safety` 依赖（需要先把 session 实现统一，才能在上面做状态事务和并发安全的改动）。其余节点无相互依赖，可并行推进。

---

> 手册最终生成时间: 2026-06-08
> 输入来源: `01-runtime-flow-deep-dive.md`, `02-state-lifecycle-deep-dive.md`, `03-tool-system-deep-dive.md`, `04-callback-plugin-deep-dive.md`, `05-workflow-a2a-deep-dive.md`, `06-entrypoint-deploy-deep-dive.md`
