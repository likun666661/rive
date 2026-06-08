# ADK Go 粗读总纲

> 基于 [`google/adk-go`](https://github.com/google/adk-go) 仓库六个粗读分册的合成报告。
> 只读分析，不修改仓库。

---

## 1. ADK Go 总体架构地图

ADK Go 的架构遵循 **"Runner 调度 → Agent 执行 → Flow 循环 → Model/Tool 调用"** 的四层主干，
覆盖 **状态服务 (session / memory / artifact)**、**插件/回调 (callbacks / plugins)**、
**多智能体编排 (workflow agents / agenttool / remote A2A)**、
**部署入口 (CLI / REST / A2A Server / Agent Engine)** 和 **可观测性 (OpenTelemetry)** 六大领域。

```
                          ┌─────────────────────────────────────────────┐
                          │         入口层 (Entrypoints / Deploy)          │
                          │                                             │
                          │  cmd/launcher/        cmd/adkgo/             │
                          │  full | prod | agentengine | console          │
                          │  web (webui,a2a,api,pubsub,eventarc)         │
                          │                                             │
                          │  server/adkrest/      server/adka2a/v2/      │
                          │  server/agentengine/   telemetry/            │
                          └──────────────┬──────────────────────────────┘
                                         │
                          ┌──────────────▼──────────────────────────────┐
                          │         Runner 层 (runner/runner.go)         │
                          │                                             │
                          │  • findAgentToRun() — 基于 history 路由       │
                          │  • InvocationContext 组装                     │
                          │  • sessionService.AppendEvent() 持久化        │
                          │  • pluginManager 回调编排                     │
                          │  • consumer iter.Seq2[*session.Event,error]  │
                          └───┬───────────┬───────────┬─────────────────┘
                              │           │           │
              ┌───────────────▼──┐ ┌──────▼──────┐ ┌──▼──────────────┐
              │  Session Service │ │  Artifact   │ │    Memory       │
              │  inmemory /      │ │  inmemory / │ │  inmemory /     │
              │  database(GORM) /│ │  gcs        │ │  vertexai       │
              │  vertexai        │ │             │ │                 │
              └──────────────────┘ └─────────────┘ └─────────────────┘
                              │
              ┌───────────────▼──────────────────────────────────────┐
              │          Agent 层 (agent/agent.go)                    │
              │                                                      │
              │  llmagent.run()        sequentialagent / parallel    │
              │  Before/AfterAgentCallbacks                          │
              │  remoteagent/v2 (A2A client)                         │
              └───────────────┬──────────────────────────────────────┘
                              │
              ┌───────────────▼──────────────────────────────────────┐
              │         Flow 层 (internal/llminternal/base_flow.go)   │
              │                                                      │
              │  runOneStep() = preprocess → callLLM → postprocess   │
              │                 → handleFunctionCalls                │
              │                 → agent transfer (递归)               │
              │  RunLive()  — 双向 WebSocket 时序缓冲                 │
              └───┬──────────────────────────┬───────────────────────┘
                  │                          │
      ┌───────────▼──────┐     ┌─────────────▼──────────────────┐
      │   Model 层       │     │        Tool 层                  │
      │   model/llm.go   │     │                                │
      │   gemini/        │     │  functiontool/  (Go func→tool)  │
      │   apigee/        │     │  mcptoolset/    (MCP protocol)  │
      │   LLM interface  │     │  skilltoolset/  (skill tools)   │
      │                  │     │  agenttool/     (agent as tool) │
      │                  │     │  geminitool/    (built-in)      │
      │                  │     │  exitlooptool / exampletool     │
      └──────────────────┘     └────────────────────────────────┘
                              │
              ┌───────────────▼──────────────────────────────────────┐
              │      插件/回调 层 (plugin/ + agent/callback_context)  │
              │                                                      │
              │  PluginManager: 14 个 hook 点, early-exit 语义        │
              │  plugin/functioncallmodifier/  — schema 改写          │
              │  plugin/retryandreflect/       — 工具失败自愈         │
              │  plugin/loggingplugin/         — 全链路调试日志       │
              │  CallbackContext — StateDelta + ArtifactDelta 沙箱  │
              └──────────────────────────────────────────────────────┘
```

**六大领域与对应分册：**

| 领域 | 分册 | 核心文件 |
|------|------|----------|
| Core Runtime (Agent/Runner/Flow/Model) | Part A | `agent/agent.go`, `runner/runner.go`, `internal/llminternal/base_flow.go` |
| State Services (Session/Memory/Artifact) | Part B | `session/service.go`, `artifact/service.go`, `memory/service.go` |
| Tools (Function Calling / MCP / HITL) | Part C | `tool/tool.go`, `tool/functiontool/function.go`, `tool/mcptoolset/set.go` |
| Callbacks & Plugins    | Part D | `plugin/plugin.go`, `agent/callback_context.go`, `internal/plugininternal/` |
| Workflow / Multi-Agent / A2A | Part E | `agent/workflowagents/`, `agent/remoteagent/v2/`, `server/adka2a/v2/` |
| Entrypoints / Server / Telemetry / Examples | Part F | `cmd/launcher/`, `server/adkrest/`, `telemetry/` |

---

## 2. 每个 Part 的粗读大纲摘要

### 第一部分：核心 Agent、Runtime 与 Model Loop

| 维度 | 内容 |
|------|------|
| **问题** | 如何在一次 Invocation 中协调 LLM、Tool、Agent 转移、流式输出、持久化？ |
| **为什么是问题** | 状态模型膨胀（history 重组 + tool call + agent tree 递归）、多 Agent 上下文隔离（branch 前缀匹配）、流式/非流式统一（StreamingResponseAggregator）、Event 持久化边界（Partial 不持久化） |
| **解决思路** | 三层架构：Runner（会话生命周期 + 持久化）→ Agent（回调 + 执行入口）→ Flow（preprocess → callLLM → postprocess → handleFunctionCalls step loop）。13 个 RequestProcessor 形成预处理管道。 |
| **代码落地** | `runner/runner.go:131-268` — `Run()`；`internal/llminternal/base_flow.go:528-653` — `runOneStep()`；`model/gemini/gemini.go` — Gemini sync/stream 实现；`agent/llmagent/llmagent.go:361` — `llmAgent.run()` |
| **待确认** | NL Planning / Code Execution / Auth 三个处理器为 TODO stub；`EndInvocation` 在 agent transfer 中的影响不明确；`ContentsRequestProcessor` 的 branch 前缀匹配健壮性 |

### 第二部分：Session、Memory 与 Artifact 状态服务

| 维度 | 内容 |
|------|------|
| **问题** | 多轮对话历史（Session）、版本化文件产物（Artifact）、跨 session 长期记忆（Memory）三者如何分层管理？ |
| **为什么是问题** | State 三种作用域（app:/user:/temp: 前缀）的拆分合并逻辑在多处重复实现；Artifact 版本号存在 GCS 竞态；Memory Search 在不同后端（关键词 vs 向量）行为差异大 |
| **解决思路** | Service 接口统一抽象 → 多实现（InMemory / Database(GORM) / VertexAI）→ `invocationContext` 注入给 agent。StateDelta 通过 `extractStateDeltas` 按前缀拆分，`trimTempDeltaState` 清理 temp 前缀 |
| **代码落地** | `session/service.go` — Service 接口；`session/inmemory.go:197-254` — AppendEvent 并发保护；`artifact/gcsartifact/service.go:102-113` — 竞态 TODO；`memory/vertexai/vertexai.go:37-41` — 增量更新 |
| **待确认** | `trimTempDeltaState` 三处重复需统一到 `sessioninternal`；Database 乐观锁微秒精度可能不足；VertexAI AppendEvent 未提取 StateDelta；GCS Artifact 版本号竞态 |

### 第三部分：工具、函数调用、MCP 与人工确认

| 维度 | 内容 |
|------|------|
| **问题** | 如何统一 8 种异构工具来源（Go func / MCP / Skills / Gemini builtins / Memory / Agent-as-tool / Exit Loop / Examples）为一套接口？ |
| **为什么是问题** | Schema 推断依赖泛型反射 + `jsonschema-go`；HITL 确认逻辑在 4 个位置重复实现；MCP 不支持 LongRunning；确认 Provider 接口在不同工具间不一致 |
| **解决思路** | `tool.Tool`（身份）→ `toolinternal.FunctionTool` / `StreamingFunctionTool` / `RequestProcessor`（能力）→ `FilterToolset` / `WithConfirmation`（装饰器）。`PackTool` 合并多个 declaration 到一个 `genai.Tool` |
| **代码落地** | `tool/functiontool/function.go:79` — `New[TArgs,TResults]` 泛型适配器；`tool/mcptoolset/set.go:108-129` — 懒连接 + ListTools；`tool/tool.go:203-229` — HITL 确认流程 |
| **待确认** | HITL 逻辑重复需提取 middleware；`streamingFunctionTool` 缺少 `Run()` 方法；`agenttool` 未继承 HITL；MCP 工具列表无缓存；Schema 推断不支持简单类型 |

### 第四部分：回调、插件与指令工具

| 维度 | 内容 |
|------|------|
| **问题** | 如何在不修改核心 agent loop 的情况下，在 agent/model/tool/event 各阶段插入自定义逻辑？ |
| **为什么是问题** | 随意插入会导致状态污染、结果覆盖、错误级联、并发冲突、可组合性差。需要 state delta 隔离 + early-exit 语义 + 有序执行 |
| **解决思路** | `callbackContext` 提供写时感知沙箱（StateDelta + ArtifactDelta）；PluginManager 按注册序遍历 15 个 hook 点，early-exit（第一个非 nil 胜出）；`functioncallmodifier` Before/After schema 改写；`retryandreflect` 工具失败自愈 |
| **代码落地** | `agent/callback_context.go:101` — `callbackContext` 实现；`internal/plugininternal/plugin_manager.go:38` — PluginManager；`plugin/retryandreflect/plugin.go:96` — 自愈插件 |
| **待确认** | Plugin close 顺序未考虑依赖关系；`functioncallmodifier` state key 碰撞风险；`InjectSessionState` 类型断言脆弱；`replayplugin` 使用 sleep + FIXME |

### 第五部分：Workflow Agents、多智能体编排与远程 A2A

| 维度 | 内容 |
|------|------|
| **问题** | 当需要多个 agent 协同（顺序/并行/循环）或跨进程/跨主机通信时，如何编排和通信？ |
| **为什么是问题** | 子 agent 间状态共享隔离策略不一致（sequential 共享 context vs parallel 复制 vs agenttool 新 session）；事件流多路复用（parallel ackChan 反向压力）；A2A v0→v2 双版本兼容维护成本高 |
| **解决思路** | Workflow agents 通过 `agent.Config.Run` 注入实现 → 统一返回 `agent.Agent`。Remote A2A 分层设计：`a2a_agent.go` (生命周期) → `client.go` (传输抽象) → `a2a_agent_run_processor.go` (事件管道) → `utils.go` (协议辅助) |
| **代码落地** | `agent/workflowagents/sequentialagent/agent.go:46`；`agent/workflowagents/parallelagent/agent.go:67-128` — ackChan 反向压力；`server/adka2a/v2/executor.go:161-239` — 8 步执行流程；`tool/agenttool/agent_tool.go:121-246` — 沙箱化子 agent |
| **待确认** | workflow agent error consistency TODO；`agenttool` agent loop termination TODO；parallel state sync race；A2A v0 legacy deprecation 时间线；LoopAgent / ParallelAgent 无 `RunLive` |

### 第六部分：CLI、Server、部署、Telemetry 与 Examples

| 维度 | 内容 |
|------|------|
| **问题** | Library 各组件如何暴露为可运行的 agent 服务？如何部署到 Google Cloud？如何实现可观测性？ |
| **为什么是问题** | 多协议端点 (REST/SSE/WebSocket/A2A/AgentEngine) 需要不同语义；Cloud Run 与 Agent Engine 部署差异大；OTel 初始化需要 GCP 认证和 resource 属性 |
| **解决思路** | Launcher 组合模式：`universal.NewLauncher(subLauncher...)` 按 keyword 路由；`web.NewLauncher()` 注册多种 sub-handler 到同一 HTTP mux；Server 层只做协议翻译，返回 `http.Handler` |
| **代码落地** | `cmd/launcher/full/full.go:31-33` — 全功能 launcher；`server/adkrest/handler.go:80-84` — REST Server；`telemetry/telemetry.go:118-124` — OTel 初始化 |
| **待确认** | Cloud Run `--no-allow-unauthenticated` 公网访问限制；Agent Engine `GOOGLE_API_KEY` secret 无自动创建；OTel LoggerProvider GCP export 未实现；adka2a v0.3 legacy 移除时间线；examples 缺集成测试 |

---

## 3. 跨模块主链路

### 3.1 完整请求链路

```
用户请求 (HTTP POST /api/run 或 console stdin)
    │
    ▼
server/adkrest/controllers/runtime.go    ← REST 协议解析
cmd/launcher/console/console.go          ← REPL/CLI 输入
    │
    ▼
runner.Runner.Run(ctx, userID, sessionID, msg, runCfg)  [runner/runner.go:131]
    │
    ├── 1. sessionService.Get/Create(session)           ← 获取/创建 Session
    │      └── session/inmemory.go 或 database/service.go 或 vertexai/
    │      └── state 合并: extractStateDeltas + MergeStates
    │
    ├── 2. findAgentToRun(session, msg)                 ← 基于 history 路由
    │      └── 倒序扫描 events → 找最近的 transferable agent
    │      └── runner/runner.go:592-666
    │
    ├── 3. NewInvocationContext(session, artifacts, memory, agent)
    │      └── internal/context/invocation_context.go
    │      └── 注入: agent.Artifacts, agent.Memory, session.State
    │
    ├── 4. appendMessageToSession()                     ← 用户消息持久化
    │      └── sessionService.AppendEvent()
    │
    ├── 5. pluginManager.RunBeforeRunCallback()         ← 可能 early exit
    │      └── internal/plugininternal/plugin_manager.go
    │
    ▼
agentToRun.Run(ctx)  ← iter.Seq2[*session.Event, error]
    │
    ├── BeforeAgentCallbacks (插件 → 用户)
    │      └── agent/agent.go:228-240
    │
    ▼
llmAgent.run(ctx)  [agent/llmagent/llmagent.go:361]
    │
    ▼
Flow.Run(ctx)  [internal/llminternal/base_flow.go:101]
    │
    └── for each step:
        │
        ├── runOneStep(ctx)  [base_flow.go:528]
        │   │
        │   ├── preprocess (13 个 RequestProcessor pipeline)
        │   │   ├── basicRequestProcessor         → GCConfig 克隆
        │   │   ├── toolProcessor                 → Tools + Toolsets 展开
        │   │   │   └── toolutils.PackTool()      → 合并 function declarations
        │   │   ├── AgentTransferRequestProcessor → 注入 transfer_to_agent tool
        │   │   ├── instructionsRequestProcessor  → instruction 模板注入
        │   │   │   └── instructionutil.InjectSessionState()  ← 解析 {key} 占位符
        │   │   ├── ContentsRequestProcessor       → 拼接 conversation history
        │   │   │   └── eventBelongsToBranch()     ← 分支匹配过滤
        │   │   └── outputSchemaRequestProcessor / HITL / auth / nlplanning / codeexec
        │   │
        │   ├── callLLM()  [base_flow.go:722]
        │   │   ├── plugin.RunBeforeModelCallback()
        │   │   ├── BeforeModelCallbacks (用户)
        │   │   ├── generateContent() ← telemetry span
        │   │   │   └── model.LLM.GenerateContent(ctx, req, stream)
        │   │   │       ├── gemini/gemini.go 或 apigee/apigee.go
        │   │   │       └── (stream) StreamingResponseAggregator
        │   │   │           └── internal/llminternal/stream_aggregator.go
        │   │   ├── AfterModelCallbacks (插件 + 用户)
        │   │   │   └── plugin/functioncallmodifier/  — 剥离额外 args → session state
        │   │
        │   ├── finalizeModelResponseEvent()
        │   │   └── yield modelResponseEvent (Partial 或 non-Partial)
        │   │
        │   ├── handleFunctionCalls()  [base_flow.go:1012]
        │   │   ├── 并行 goroutine processing:
        │   │   │   ├── BeforeToolCallbacks
        │   │   │   │   └── callbackContext.StateDelta / ArtifactDelta 追踪
        │   │   │   ├── tool.Run(toolCtx, args)
        │   │   │   │   ├── functiontool → handler(ctx, typedArgs)
        │   │   │   │   ├── mcptoolset  → mcpClient.CallTool(params)
        │   │   │   │   │   └── connectionRefresher (重连 + 重试)
        │   │   │   │   ├── agenttool   → sub runner.Run() → extract result
        │   │   │   │   ├── HITL check → ctx.RequestConfirmation()
        │   │   │   │   │   └── ErrConfirmationRequired → 循环重试
        │   │   │   │   └── exitlooptool → Actions().Escalate = true
        │   │   │   ├── AfterToolCallbacks
        │   │   │   │   └── plugin/retryandreflect/ → 失败 count → reflection prompt
        │   │   │   └── mergeParallelFunctionResponseEvents()
        │   │   ├── yield functionResponseEvent (含 EventActions.StateDelta)
        │   │
        │   ├── if ev.Actions.TransferToAgent != ""
        │   │   └── nextAgent.Run(ctx)  ← 递归 agent transfer
        │   │       └── (同 Flow.Run 流程)
        │   │
        │   └── IsFinalResponse()? → break 或 下一轮 step
        │
        ▼
Runner for loop (consumer):
    │
    ├── pluginManager.RunOnEventCallback()  ← 可修改/丢弃 event
    ├── event.Actions.StateDelta
    │   └── extractStateDeltas → update app/user/session state
    ├── event.Actions.ArtifactDelta → 记录 artifact 版本变更
    ├── 非 Partial → sessionService.AppendEvent() 持久化
    │   └── trimTempDeltaState() → persist to DB/Memory/VertexAI
    ├── (可选) memoryService.AddSessionToMemory(session)
    └── yield event to caller → SSE/WebSocket/console output
```

### 3.2 关键数据流

```
用户消息 UserMessage → session.Event (Author=user, Partial=false)
    ├── AppendEvent → extractStateDeltas → update app/user/session state
    │
    ▼
LLM 请求 LLMRequest
    ├── Contents: 历史 events → ContentsRequestProcessor 重组
    ├── Config: basicRequestProcessor 克隆 GCConfig
    ├── Tools: toolProcessor 展开 function decls
    └── Instruction: instructionProcessor 注入模板变量
    │
    ▼
LLM 响应 LLMResponse
    ├── 流式: Partial events (不持久化, 只 yield)
    ├── 最终: non-Partial event (持久化)
    └── FunctionCall(s) → handleFunctionCalls
    │
    ▼
Tool 执行 → functionResponse
    ├── StateDelta → session state 写入
    ├── ArtifactDelta → artifact 版本追踪
    ├── TransferToAgent → agent tree 递归
    └── LongRunningToolIDs → IsFinalResponse → step loop break
    │
    ▼
最终输出 → session.Event (Author=agent_name, Branch=agent.path)
    ├── StateDelta 合并 → scoped state (app/user/session)
    ├── AppendEvent → GORM/InMemory/VertexAI 持久化
    └── (可选) Memory service 提取事件 → 建索引
```

---

## 4. 代码阅读路线（建议三层阅读顺序）

### 第一层：理解主干 - Runner → Agent → Flow 执行链路

**目标**：看懂一次用户请求从 HTTP 到 LLM 再到 tool call 最后返回的完整路径。

| 顺序 | 文件 | 着重阅读 | 说明 |
|------|------|----------|------|
| 1 | `agent/agent.go` | `Agent` 接口 (L28-60)、`Config` (L62)、`New()` (L70) | 理解 Agent 是什么 |
| 2 | `agent/context.go` | `InvocationContext` (L70-123)、`CallbackContext` (L125)、`ToolContext` (L136) | 上下文接口全貌 |
| 3 | `model/llm.go` | `LLM` 接口 (L26)、`LLMRequest` (L32)、`LLMResponse` (L42) | 模型抽象 |
| 4 | `session/session.go` | `Session` (L32)、`Event` (L92)、`EventActions` (L143) | 事件与状态载体 |
| 5 | `runner/runner.go` | `Run()` (L131-268)、`findAgentToRun()` (L592-666) | Runner 主循环 |
| 6 | `agent/llmagent/llmagent.go` | `llmAgent.run()` (L361)、Config (L130) | Agent 入口 |
| 7 | `internal/llminternal/base_flow.go` | `Run()` (L101-127)、`runOneStep()` (L528-653)、`callLLM()` (L722)、`handleFunctionCalls()` (L1012) | 核心 step loop |
| 8 | `internal/llminternal/stream_aggregator.go` | `ProcessResponse()` (L57)、`Close()` (L304) | 流式聚合 |

### 第二层：理解横切 - Tools / Plugins / State Services

**目标**：理解 agent 如何调用工具、如何注入自定义行为、如何处理状态。

| 顺序 | 文件 | 着重阅读 | 说明 |
|------|------|----------|------|
| 9 | `tool/tool.go` | `Tool` 接口 (L38)、`Toolset` (L57)、`WithConfirmation()` (L143)、`FilterToolset()` (L89) | 工具抽象 |
| 10 | `internal/toolinternal/tool.go` | `FunctionTool` (L28)、`RequestProcessor` (L40) | 内部工具接口 |
| 11 | `tool/functiontool/function.go` | `New[TArgs,TResults]()` (L79)、`Run()` (L185-247) | Go 函数 → Tool 适配器 |
| 12 | `tool/mcptoolset/set.go` | `New()` (L49)、`Tools()` (L108-129) | MCP 工具集 |
| 13 | `internal/toolinternal/toolutils/toolutils.go` | `PackTool()` (L35) | 工具声明合并 |
| 14 | `session/service.go` | `Service` 接口 | Session CRUD |
| 15 | `session/inmemory.go` | `AppendEvent()` (L197-254) | 状态持久化 |
| 16 | `internal/sessionutils/utils.go` | `ExtractStateDeltas()` (L31)、`MergeStates()` (L58) | 状态作用域拆分 |
| 17 | `agent/callback_context.go` | `callbackContext` (L101)、`callbackContextState` (L217)、`trackedArtifacts` (L243) | 回调沙箱 |
| 18 | `plugin/plugin.go` | `Plugin` struct (L78)、15 个 callback 字段 | 插件定义 |
| 19 | `internal/plugininternal/plugin_manager.go` | `RunBeforeModelCallback()` (L222)、其他 hook 方法 | 插件编排 |
| 20 | `plugin/retryandreflect/plugin.go` | `New()` (L96)、`OnToolError` / `AfterTool` | 自愈机制 |

### 第三层：理解编排 - Multi-Agent / A2A / Deploy

**目标**：理解多 agent 协作、跨进程通信和生产部署。

| 顺序 | 文件 | 着重阅读 | 说明 |
|------|------|----------|------|
| 21 | `agent/workflowagents/sequentialagent/agent.go` | `New()` (L46)、`Run()` (L78) | 顺序编排 |
| 22 | `agent/workflowagents/parallelagent/agent.go` | `run()` (L67)、`runSubAgent()` (L130) | 并发编排 + 反向压力 |
| 23 | `agent/workflowagents/loopagent/agent.go` | `New()` (L45)、`Run()` (L75) | 循环编排 |
| 24 | `tool/agenttool/agent_tool.go` | `Run()` (L121-246) | Agent 作为 Tool |
| 25 | `agent/remoteagent/v2/a2a_agent.go` | `NewA2A()` (L156)、`run()` (L199-303) | A2A 客户端 |
| 26 | `server/adka2a/v2/executor.go` | `Execute()` (L161-239) | A2A 服务端 |
| 27 | `server/adka2a/v2/events.go` | `ToSessionEventWithParts()`、`EventToMessage()` | A2A 事件转换 |
| 28 | `cmd/launcher/full/full.go` | `NewLauncher()` (L31) | Launcher 组合入口 |
| 29 | `server/adkrest/handler.go` | `Server` struct (L80)、路由注册 (L48) | REST Server |
| 30 | `telemetry/telemetry.go` | `New()` (L118)、`Providers` | OTel 集成 |
| 31 | `cmd/adkgo/internal/deploy/cloudrun/` | Deploy 流程 (L280-310) | Cloud Run 部署 |
| 32 | `internal/llminternal/contents_processor.go` | `process()` (L160+) | 历史事件 → LLM Contents |
| 33 | `internal/llminternal/agent_transfer.go` | `transferToAgent.Run()` | Agent 转移 tool 实现 |

---

## 5. 风险 / 不确定点

### 5.1 代码层面

| # | 风险 | 位置 | 影响 |
|---|------|------|------|
| 1 | `trimTempDeltaState` 三处重复实现 | `inmemory.go`, `database/session.go`, `vertexai/session.go` | 维护一致性风险，已有 TODO 建议移入 `sessioninternal` |
| 2 | GCS Artifact 版本号竞态 | `artifact/gcsartifact/service.go:102-113` | 多客户端并发 Save 同一文件版本号可能重复 |
| 3 | HITL 确认逻辑四处重复 | `functiontool/function.go`, `streaming_function.go`, `mcptoolset/tool.go`, `tool/tool.go` | 修改需同步四处，极易遗漏 |
| 4 | `streamingFunctionTool` 缺少 `Run()` | `tool/functiontool/streaming_function.go` | 无法被 HITL `confirmationTool.Run()` 包装 |
| 5 | Database 乐观锁微秒精度 | `session/database/service.go:374-382` 使用 `>` 而非 `>=` | 同一微秒内两次 AppendEvent 不会被检测为 stale |
| 6 | VertexAI AppendEvent 未提取 StateDelta | `session/vertexai/vertexai.go:129-146` | app/user 级别 state 更新可能丢失 |
| 7 | A2A v0→v2 双版本代码 | `agent/remoteagent/a2a_agent.go` (341 行) + `server/adka2a/executor.go` (408 行) | 维护成本高，无明确的 deprecation 时间线 |
| 8 | `replayplugin` 使用 `time.Sleep` + FIXME | `replay_plugin.go:385-386` | 并发回放确定性保证不完善 |
| 9 | `ContentsRequestProcessor` branch 前缀匹配 | `internal/llminternal/contents_processor.go` `strings.HasPrefix` + `"."` 后缀 | 深层 agent 树中的潜在误匹配 |
| 10 | OTel LoggerProvider GCP export 未实现 | `telemetry/setup_otel.go` TODO(#479) | Logs 无法导出到 GCP Cloud Logging |

### 5.2 设计层面

| # | 问题 | 说明 |
|---|------|------|
| 11 | 三种状态隔离模型不一致 | `sequentialagent` 共享 context，`parallelagent` 复制 context，`agenttool` 创建新 session + 复制 state — 三种策略适用场景和风险不同 |
| 12 | `EndInvocation` 在 agent transfer 中的语义 | 子 agent 调用后是否影响父 agent callback 的执行？ |
| 13 | Agent transfer 缺少 "自动回弹" | Python ADK 中下一条用户消息自动路由回父 agent，Go 版 `runner.go:654-666` 仅检查 parent chain 禁止转移 |
| 14 | LoopAgent / ParallelAgent 无 `RunLive` | 双向流场景下无法使用这两种 workflow agent |
| 15 | `agenttool` 无 agent loop 终止保护 | `agent_tool.go:200` TODO — 子 agent 可能无限循环 |
| 16 | Plugin 执行顺序未文档化 | plugin 回调与 agent 原生 callback 的执行顺序、early-exit 语义在 15 个 hook 点上是否一致？ |
| 17 | Agent Engine `GOOGLE_API_KEY` secret | 部署时假设 secret 已存在，无自动创建逻辑 |

---

## 6. 下一轮精读 DAG 建议

建议将下一轮深度阅读拆为 **5 个独立工作节点**（可按序或并行执行）：

```
work_001: 核心 loop 精读
├── 深入 base_flow.go runOneStep 的每个分支
├── StreamingResponseAggregator 错误处理全路径
├── ContentsRequestProcessor branch 匹配的边界 case
├── handleFunctionCalls 的并行 error 传播
└── 产出: step loop 的状态机图 + 边界条件表

work_002: 状态服务一致性审计
├── 统一 trimTempDeltaState → sessioninternal 包（验证 3 处重复）
├── GCS Artifact 竞态的最小修复
├── Database 乐观锁的精度问题
├── VertexAI AppendEvent StateDelta 缺失的修复
└── 产出: 状态一致性审计报告 + PR diff 建议

work_003: 工具系统重构
├── HITL 逻辑提取为 middleware（消除 4 处重复）
├── streamingFunctionTool 补全 Run() 方法
├── MCP LongRunning 支持分析
├── agenttool HITL 集成
├── MCP 工具列表缓存策略
└── 产出: 工具系统重构建议 + 设计文档

work_004: A2A / Multi-Agent 深度分析
├── v0→v2 deprecation 的迁移路径
├── 三种状态隔离模型的统一分析
├── LoopAgent / ParallelAgent RunLive 可行性
├── agenttool agent loop 终止保护
├── parallel state sync race 验证
└── 产出: Multi-Agent 架构改进建议 + 迁移方案

work_005: 部署与可观测性评估
├── OTel LoggerProvider 替代方案评估
├── Agent Engine secret 生命周期管理
├── A2A v0.3 移除影响分析
├── examples 集成测试覆盖
├── Cloud Run 部署的认证配置审计
└── 产出: 生产可部署性评估报告
```

---

## 附录：Artifact 路径清单

- `/tmp/rive-google-adk-go-read-20260608/part-a-core-agent-runtime.md`
- `/tmp/rive-google-adk-go-read-20260608/part-b-state-services.md`
- `/tmp/rive-google-adk-go-read-20260608/part-c-tools.md`
- `/tmp/rive-google-adk-go-read-20260608/part-d-callbacks-plugins.md`
- `/tmp/rive-google-adk-go-read-20260608/part-e-workflow-multiagent-a2a.md`
- `/tmp/rive-google-adk-go-read-20260608/part-f-entrypoints-server-telemetry-examples.md`
