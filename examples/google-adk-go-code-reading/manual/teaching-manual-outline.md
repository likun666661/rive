# Google ADK Go 复刻版教学手册细纲

> 合成日期：2026-06-11
> 输入 artifact：7 份 chapter section artifact
> 复刻工程：`/Users/likun/Desktop/workspace-for-google-adk-go/rive-adk-go/`
> 精读报告：`examples/google-adk-go-code-reading/manual/deep-read/*.md`
> **定位：教学材料，不是生产级 ADK Go 替代品。文中多处标注简化边界。**

---

## 0. 教学路线图（90–120 分钟）

| 段 | 章节 | 主题 | 建议时长 | 累积 | 转场逻辑 |
|----|------|------|----------|------|----------|
| 1 | Ch01 | Runtime Flow: Runner → Agent → Flow → Model/Tool → Event → Session | 16 min | 16 | 开场：画完整调用链，用天气查询 demo 立 flag |
| 2 | Ch02 | State Lifecycle: Session / Memory / Artifact | 14 min | 30 | 从持久化到状态——"Events 存了，但跨 session 的记忆和文件呢？" |
| 3 | Ch03 | Tool System: Declaration / Execution / Streaming / Confirmation | 14 min | 44 | 从状态到工具——"模型调用了 function call，谁来执行？" |
| 4 | Ch04 | Callback / Plugin / Instruction | 14 min | 58 | 从核心到扩展——"日志、缓存、指令注入不能侵入 Flow 循环" |
| 5 | Ch05 | Workflow / AgentTool / Remote A2A | 14 min | 72 | 从单 agent 到多 agent——"一个 agent 不够，怎么组合多个？" |
| 6 | Ch06 | Entrypoint / Deploy / Telemetry | 10 min | 82 | 从开发到产品——"写好了 agent，怎么暴露出去？" |
| 7 | Ch07 | Agent Flow / ReAct / Multi-Agent | 14 min | 96 | 收官——"前面 6 章全为了这一章：ReAct 是组合，不是魔法" |
| — | Q&A | 自由问答 + 总结 thesis | 14 min | 110 | 开放讨论，回顾 6 条关键 thesis |

**主线串联**：Runner → Agent → Flow → Tool/State → Multi-Agent

**每个转场只需一句话**：
- Ch01→Ch02："Runner 跑完一次调用，events 持久化到 session——但不同 session 之间的状态和数据怎么共享？"
- Ch02→Ch03："Flow 的核心是 model → tool 的循环，模型调了 function call，工具怎么声明、怎么执行？"
- Ch03→Ch04："工具跑起来了，但日志、缓存、指令注入这些横切关注点不能塞进 tool 本身。"
- Ch04→Ch05："单 agent 搞定了，现在需要多个 agent 串联、并联、按需委派。"
- Ch05→Ch06："agent 组合好了，怎么通过 console、REST、部署到云上让人用？"
- Ch06→Ch07："最后一步：把所有概念（Model + Tool + Plugin + Workflow + Config）组合成完整的 ReAct agent 和可配置的 agent tree。"

### 如果现场只有 30 分钟怎么压缩

1. **砍掉 Live Coding，只做白板 + 投屏走读**（省 15 min）。
2. **砍掉练习题演示，口述题目即可**（省 10 min）。
3. **Ch02 压缩到 5 分钟**——只讲四层 state 作用域对比表 + write-through 一句话。
4. **Ch05 压缩到 5 分钟**——只讲 Sequential/Parallel/Loop 三种 workflow 的对比表 + AgentTool 一句话。
5. **Ch06 压缩到 3 分钟**——只展示 `launcher.Config` 稳定协议 + telemetry span 四类。
6. **Ch07 压缩到 2 分钟**——一句话："ReAct = Flow.Run 的 for 循环 + transfer_to_agent + 策略插件"，交给课后自学。

30 分钟不可求全，核心是在听众脑中植入 **"四层分离 / state 四作用域 / ReAct 是 Flow 循环"** 三个概念。

---

## Chapter 01 — Runtime Flow: Runner → Agent → Flow → Model/Tool → Event → Session（16 min）

> 复刻代码目录：`rive-adk-go/` | 核心文件：`runner/runner.go`, `agent/agent.go`, `flow/flow.go`, `model/model.go`, `event/event.go`, `session/session.go`, `llmagent/llmagent.go`, `cmd/demo/main.go`
> 精读报告对应：`manual/deep-read/01-runtime-flow-deep-dive.md`

### 讲解目标

学完本章，听众应能：
1. 画出完整运行时调用链：`Runner.Run → Agent.Execute → Flow.Run → runOneStep(preprocess → callModel → postprocess → handleFunctionCalls → handleTransfer) → Event → Session`。
2. 解释为什么需要四层分离（Runner / Agent / Flow / Model）而不是一个函数解决所有问题。
3. 用 FakeModel + Flow 写最简 tool-calling demo，理解三步循环：model → function call → tool → model。
4. 理解 Event 的 `Partial` 标记如何影响 Session 持久化：partial 事件被 yield 但不被保存。
5. 识别 Runner 的 agent routing 逻辑：通过 session history 反向查找最后活跃的可 transfer agent。

### 问题背景

用户发一条消息 `"What's the weather in Tokyo?"`，背后涉及：
- **多轮交互**：model 先返回 function call → 工具执行 → 再调 model 生成最终回复。
- **流式 vs 非流式**：partial chunks 需要实时 yield 给客户端，但不能提前写入 session（否则中断恢复时数据不一致）。
- **多 Agent 路由**：specialist agent 被 `transfer_to_agent` 调用后，后续消息应路由回 specialist 而非 root。
- **可观测性**：需要 before/after 钩子注入日志、缓存、状态修改，但不侵入核心循环。

### 为什么难

四层职责必须分离但必须无缝协作：

| 层 | 职责 | 难点 |
|----|------|------|
| Runner | 会话管理、agent 路由、事件持久化 | 跨 invocation 的 agent 路由需反向扫描 session history，判断 transfer 链是否可允许 |
| Agent | 生命周期回调（before/after） | 回调可能 early-exit（返回 content 跳过 run），也可能只产生 state delta |
| Flow | 多 step 循环（model call + tool exec） | 终止条件不是固定次数，而是 `IsFinalResponse()` 的语义判定 |
| Model | LLM 调用抽象 | FakeModel 用预定义响应队列实现确定性测试 |

### 核心抽象

整体架构：

```
Runner.Run
  → session.Get/Create → findAgentToRun → appendMessageToSession
  → create InvocationContext → agentToRun.Execute
      → beforeAgentCallbacks (可 early-exit)
      → llmAgent.run → Flow.Run
          → for { runOneStep → if IsFinalResponse() → return }
          → runOneStep:
              1. preprocess (request processors)
              2. callModel (BeforeCallbacks → GenerateContent → AfterCallbacks)
              3. postprocess (response processors)
              4. finalizeModelResponseEvent → yield event
              5. handleFunctionCalls (并发 goroutine 执行 tools → mergeResultsToEvent)
              6. handleTransfer (如 TransferToAgent → 查找目标 agent → Execute)
      → afterAgentCallbacks
  → persist non-partial events → yield all events to caller
```

### 复刻版代码走读

推荐阅读顺序（从外层到内层）：

1. `cmd/demo/main.go` — `runChapter01()` (L121-218)：最简可运行示例，展示完整链路。
2. `runner/runner.go` — `Runner.Run` (L124-187)：顶层编排器入口。三步：get/create session → findAgentToRun → Execute agent → persist non-partial events。
3. `runner/runner.go` — `findAgentToRun` (L192-215)：反向扫描 events，跳过 user，找最后活跃且可 transfer 的 agent。
4. `agent/agent.go` — `Agent` 接口 (L41-49)、`baseAgent.Execute` (L144-173)：before callbacks → a.run → after callbacks。
5. `llmagent/llmagent.go` — `New` (L29-48)：Agent→Flow 胶水层，将 `agent.InvocationContext` 断言为 `context.InvocationContext`，然后调 `f.Run(ic)`。
6. `flow/flow.go` — `Flow` 结构体 (L79-100)、`Flow.Run` (L103-147)、`runOneStep` (L151-223)：核心多步循环。
7. `flow/flow.go` — `callModel` 回调顺序 (L241-313)：**Plugin → Ctx callback → Legacy callback → model → Plugin(OnError+After) → Ctx callback → Legacy callback**。
8. `flow/flow.go` — `handleFunctionCalls` (L368)：并发 goroutine 通过 `sync.WaitGroup` 并行执行工具。
9. `flow/flow.go` — `executeTransfer` (L767-831)：递归调用目标 agent 的 Execute，支持链式 transfer（A→B→C），深度限制 10 层。
10. `model/model.go` — `LLM` 接口 (L47-50)、`FakeModel` (L55-81)：**教学复刻精髓**——无需真实 LLM 即可测试整个 Flow 循环。
11. `event/event.go` — `Event` 结构体 (L105-146)、`IsFinalResponse` (L166-187)：循环终止判定。
12. `session/session.go` — `Session` 接口 (L108-116)、`inMemorySession.AppendEvent` (L170-193)。

### 演示建议

1. **Live coding 30 行天气查询 demo**（3 min）：`NewFunctionTool` → `NewFakeModel` → `Flow` → `llmagent.New` → `Runner.Run`，展示 3 个 events 输出。
2. **Partial 事件不持久化**（2 min）：产生一个 partial + 一个 final event，验证 session 里的 events 只有 user + final。
3. **Agent Transfer 路由**（2 min）：两次 invocation，第一次触发 transfer_to_agent，第二次直接命中 specialist。

### 容易误解点

1. **"Flow.Run 的终止条件只是没有 function call"** → `IsFinalResponse()` 同时检查 `Partial`、`Interrupted`、`ErrorCode`、`TransferToAgent`。
2. **"Runner 每次都从 root agent 开始执行"** → `findAgentToRun` 反向扫描 events，如果上一次调用以 specialist 结尾，下次会直接路由到 specialist。
3. **"Partial 事件不持久化 = 丢掉"** → partial 事件被 yield 给调用方，只是不写入 session history。
4. **"FakeModel 的响应队列用完了会怎样"** → 返回 `"no more queued responses"` 错误。
5. **"每个 step 都调 GenerateContent"** → 如果 `BeforeModelCallback` 返回了非 nil response，实际 model 调用被跳过。

### 练习题

- **Q1**：写出 Runner → Agent → Flow → Model → Event → Session 的完整调用栈。
- **Q2**：一个 tool-calling step 产生了 3 个 function call，它们在 `handleFunctionCalls` 中如何执行？结果顺序如何？
- **Q3**：什么条件下 `Flow.Run` 会跳过 `callModel` 直接返回 event？
- **Q4**：Runner 配置了 MemoryService 和 ArtifactService，但 agent 和 flow 中都没有使用。这些服务在 Ch01 运行时中起作用吗？
- **Q5**：如果 `findAgentToRun` 的实现中没有跳过 user 事件，会发生什么？

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 建议讲解点 | 测试文件 |
|------|-------------|------|-----------|---------|
| `runner/runner.go` | `Runner`, `Run`, `findAgentToRun`, `isTransferableAcrossAgentTree` | 77,124,192,220 | **顶层编排入口** + agent routing 算法 | `runner_test.go` |
| `agent/agent.go` | `Agent` interface, `baseAgent`, `Execute`, `New` | 41,95,144,176 | Agent 生命周期 | `agent_test.go` |
| `llmagent/llmagent.go` | `New` | 29 | Agent→Flow 胶水层 | `llmagent_test.go` |
| `flow/flow.go` | `Flow`, `Run`, `runOneStep`, `callModel`, `handleFunctionCalls`, `executeTransfer` | 79,103,151,241,368,767 | **核心多步循环** + 回调链顺序 | `flow_test.go` |
| `model/model.go` | `LLM` interface, `FakeModel`, `LLMRequest`, `LLMResponse` | 15,37,47,55 | 最小化 LLM 抽象 + 预定义响应队列 | `model_test.go` |
| `event/event.go` | `Event`, `EventActions`, `IsFinalResponse`, `NewEvent` | 68,100,166,148 | 核心数据单元 + 循环终止判定 | `event_test.go` |
| `session/session.go` | `Session` interface, `inMemorySession`, `Service`, `AppendEvent` | 107,142,363,446 | 会话状态 | `session_test.go` |
| `cmd/demo/main.go` | `runChapter01` | 121 | **最简完整链路演示** | — |

**关键测试**：`TestRunnerSimpleTextRun`, `TestRunnerToolCallAndFinalResponse`, `TestRunnerPartialEventsNotPersisted`, `TestRunnerAutoCreateSession`, `TestRunnerSessionReuse`.

---

## Chapter 02 — State Lifecycle: Session / Memory / Artifact（14 min）

> 复刻代码目录：`session/`, `memory/`, `artifact/`, `context/`, `runner/`
> 核心文件：`session/session.go`, `memory/service.go`, `memory/inmemory.go`, `artifact/service.go`, `artifact/inmemory.go`, `context/context.go`, `context/callback_context.go`
> 精读报告对应：`manual/deep-read/02-state-lifecycle-deep-dive.md`

### 讲解目标

1. 解释为什么 Session / Memory / Artifact 三个存储不能混成一个。
2. 画出 state 写入后经过 `StateDelta` → `trimTempDeltaState` → `ExtractStateDeltas` → `updateAppState/updateUserState/maps.Copy` 的完整路由。
3. 指出 `app:` / `user:` / `(无前缀)` / `temp:` 四种作用域各自的可见性范围和生命周期。
4. 写出 artifact Save → 版本自增 → Load by version / Load latest 的调用代码。
5. 写出 memory AddSessionToMemory → SearchMemory 的调用代码。
6. 判断为什么 CallbackContext 的 `State()` 是 write-through 模式。

### 问题背景

Agent 对话过程中需要管理三种性质完全不同的数据：

| 存储 | 示例 | 关键特征 |
|------|------|----------|
| Session | 用户查天气，模型返回 function call，工具返回结果，模型回复 | 一次对话线程内的短期 KV + 事件流 |
| Memory | Session 1 说"我喜欢 Python"，Session 2 应回忆起 Python 偏好 | 跨 Session 的长期知识，关键词搜索 |
| Artifact | Agent 生成了图表 PNG，需持久化并返回给用户 | 按文件独立存在、支持版本演进 |

### 为什么难

四种维度的差异不可调和：生命周期（session 短期 vs memory 长期 vs artifact 按文件）、数据模型（有序事件+KV vs 关键词索引 vs 文件 blob+版本）、查询模式（Key 查询 vs 关键词搜索 vs 文件名+版本）、作用域（app+user+session vs app+user vs app+user+session/user:）。

### 核心抽象

**四层作用域**：

| 前缀 | 作用域 | 生命周期 | 存储位置 |
|------|--------|----------|----------|
| `app:` | 同 app 所有 user/session 共享 | App 生命周期 | `Service.appState[appName]` |
| `user:` | 同 app 同 user 所有 session 共享 | User 生命周期 | `Service.userState[appName][userID]` |
| *(无前缀)* | 仅当前 session | Session 生命周期 | `session.state.data` |
| `temp:` | 仅当前 invocation 可见 | Invocation 结束即丢弃 | 运行时存在，不持久化 |

**状态写入路径**：

```
CallbackContext.State().Set("app:env", "prod")
    → actions.StateDelta["app:env"] = "prod"  (delta 记录)
    → session.state.Set("app:env", "prod")     (write-through)

AppendEvent:
    → applyStateDelta: 所有 key 写入 session state
    → trimTempDeltaState: 从 delta 中移除 temp: 前缀 key
    → ExtractStateDeltas: 拆分 app:/user:/session delta
    → updateAppState / updateUserState / maps.Copy(sessionState, sessionDelta)
```

### 复刻版代码走读

1. `session/session.go` — `State` 接口 (L38-44)、前缀常量 `KeyPrefixApp/KeyPrefixUser/KeyPrefixTemp` (L28-32)、`Session` 接口 (L108-116)、`MergeStates` (L280-308)、`ExtractStateDeltas` (L252-271)、`trimTempDeltaState` (L330-353)、`Service` 结构体 (L363-368)。
2. `session/session.go` — `Service.AppendEvent` (L446-482)：状态路由核心。校验→applyStateDelta→trimTempDeltaState→removeTempKeysFromState→追加 event。
3. `session/session.go` — `applyStateDelta` (L495-505)：所有 key 写入 session state → ExtractStateDeltas → updateAppState/updateUserState/maps.Copy。
4. `artifact/service.go` — `Service` interface (L23-30), `SaveRequest`/`LoadRequest` (L46, L85)。
5. `artifact/inmemory.go` — `Save` (L51-69): `maxVersion + 1`；`Load` (L72-98): `req.Version > 0` 精确查找；`resolveIdentity` (L39-49): `user:` 前缀跨 session 共享。
6. `memory/service.go` — `Service` interface (L19)：`AddSessionToMemory` + `SearchMemory`。
7. `memory/inmemory.go` — `AddSessionToMemory` (L40-91): 遍历 events，分词后按 (app, user, sessionID) 存储；`SearchMemory` (L93-128): query 分词后 `wordsIntersect`。
8. `context/callback_context.go` — `callbackContextState` (L152-187): `Get` 先查 delta → 回退 durable; `Set` 同时写 delta + durable; `trackedArtifacts` (L195-235): decorate `Save` 自动记录版本到 `ArtifactDelta`。

### 演示建议

1. **三 session 状态隔离演示**（3 min）：sess-1 设置 `app:env=production`, `user:theme=dark`, `topic=session1`；sess-2 通过 `GetMergedState` 看到前两个但看不到 `topic` 和 `temp:` 前缀 key。
2. **白板画 state 写入流程**（2 min）：CallbackContext.Set → AppendEvent → applyStateDelta → trimTemp → removeTemp → ExtractStateDeltas → updateApp/User。
3. **Artifact 版本管理**（2 min）：Save v1 → Save v2（版本自增）→ Load latest → Load Version=1。
4. **Memory 跨 session 搜索**（2 min）：AddSessionToMemory(sess1) + AddSessionToMemory(sess2) → SearchMemory("state config") → 不同 user 搜索返回 0 条。

### 容易误解点

1. **"temp: 前缀的 key 会持久化"** → 不会。`AppendEvent` 的 `trimTempDeltaState` + `removeTempKeysFromState` 彻底清理。
2. **"app: 和 user: key 只存在特定存储中"** → 也写入了 `session.state`。`GetMergedState` 返回三层合并视图。
3. **Artifact `List` 返回文件内容** → 只返回文件名列表。
4. **Memory 搜索是语义搜索** → 只是简单关键词交集 `wordsIntersect`，非向量搜索。
5. **"Memory 的 AddSessionToMemory 是增量追加"** → 每次调用覆盖整个 session 的所有 events（幂等但不增量）。

### 练习题

- **Q1**：创建两个 session（同一 app 同一 user），验证 `GetMergedState` 能看到/看不到哪些 key。
- **Q2**：写出 artifact Save v1 → Save v2 → Load latest → Load version 1 的调用代码。
- **Q3**：判断正误：A) `app:` key 不写入 session.state；B) temp key 在 AppendEvent 后被移除但 invocation 内可见；C) Memory 使用向量语义匹配；D) Artifact version 从 0 开始。
- **Q4**：说明 CallbackContext write-through 策略的优缺点。
- **Q5**：如何在 Memory 中添加向量语义搜索？需要改动哪些接口？

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 建议讲解点 | 测试文件 |
|------|-------------|------|-----------|---------|
| `session/session.go` | `State`, `Session`, `Service`, `MergeStates`, `ExtractStateDeltas`, `trimTempDeltaState`, `applyStateDelta` | 38,108,363,280,252,330,495 | **作用域路由核心** | `session_test.go` |
| `event/event.go` | `EventActions.StateDelta`, `EventActions.ArtifactDelta` | 68,70,75 | Delta 数据结构 | `event_test.go` |
| `artifact/service.go` | `Service`, `SaveRequest`, `LoadRequest` | 23,46,85 | Artifact 接口和验证 | `inmemory_test.go` |
| `artifact/inmemory.go` | `Save`, `Load`, `resolveIdentity`, `List` | 51,72,39,129 | 版本自增 + user namespace | `inmemory_test.go` |
| `memory/service.go` | `Service` | 19 | Memory 接口 | `inmemory_test.go` |
| `memory/inmemory.go` | `AddSessionToMemory`, `SearchMemory`, `wordsIntersect` | 40,93,130 | **关键词交集搜索** | `inmemory_test.go` |
| `context/callback_context.go` | `callbackContextState`, `trackedArtifacts`, `RunWithCallbackContext` | 152,195,255 | **write-through state + artifact tracking** | `callback_context_test.go` |
| `runner/runner.go` | `Runner.Run` | 124 | **入口主循环** | `runner_test.go` |

---

## Chapter 03 — Tool System: Declaration / Execution / Streaming / Confirmation（14 min）

> 复刻代码目录：`tool/` + `flow/flow.go` 的 tool 调度逻辑
> 核心文件：`tool/tool.go`, `tool/streaming_tool.go`, `tool/context.go`, `flow/flow.go`
> 精读报告对应：`manual/deep-read/03-tool-system-deep-dive.md`

### 讲解目标

1. 解释 declaration 与 runtime execution 分离的动机。
2. 画出 FunctionCall → ToolContext → FunctionResponse 完整生命周期。
3. 写出三种工具接入模式：`NewFunctionTool`、`WithConfirmation`、`NewStreamingFunctionTool`。
4. 对比 Tool vs Toolset vs FilterToolset vs WithConfirmation 四个抽象层。
5. 识别 replica 简化边界：MCP、Gemini 原生工具、AgentTool 等。

### 问题背景

LLM Agent 面临的工具来源极其多样：Go 函数（泛型 TArgs→JSON Schema 推断）、MCP 服务器（JSON-RPC over stdio/SSE）、Gemini 原生工具（genai.Tool）、子 Agent（InputSchema）、Skill 文件系统。

核心问题：**如何让 Flow 在主循环中无差别地调度这些工具，同时每种来源能以最小代价接入？**

### 为什么难

- **Schema 生成与转换**：Go 泛型→JSON Schema 需反射，边界情况多（基本类型需包装、空 struct 特殊处理）。
- **Args/Result 编码**：LLM 返回 `map[string]any`，需 `ConvertToWithJSONSchema` 转为强类型 TArgs，两层转换有三处边界。
- **Streaming 双轨模型**：Live 模式 chunks 异步推送，Non-Live 模式 `CollectStreamChunks` 合并为单条结果。
- **Confirmation 三层路径**：静态 flag → 动态 Provider → WithConfirmation 装饰器，确认逻辑在四处以几乎相同的代码复制。
- **Long-Running 声明注入**：通过修改 declaration 的 description 提示 LLM，框架不强制约束。

### 核心抽象

三层接口 + 两套执行协议：

```
Tool (最小公共接口: Name / Description / IsLongRunning)
    │
    ├── DeclarationProvider (给 LLM 的声明: Declaration())
    └── FunctionTool (本地执行: Run(args))
            │
            └── StreamingFunctionTool (流式: RunStream(args))
```

**为什么 declaration 和 execution 必须分离？**
1. **时序不同**：declaration 在 LLM 请求**之前**被注入，execution 在 LLM 返回 FunctionCall **之后**才触发。
2. **协议插口不同**：declaration 面向 LLM（FunctionDeclaration），execution 面向工具实现者（Run）。
3. **来源可能合一也可能分离**：本地 Go 函数的 declaration 和 execution 在同一个 struct；Gemini 原生工具由 Gemini API 闭环。

### 复刻版代码走读

1. `tool/tool.go` — `Tool` 接口 (L34-78), `Declaration` 结构体, `DeclarationProvider`, `FunctionTool`, `FuncTool`, `NewFunctionTool`, `NewLongRunningFunctionTool`。
2. `tool/tool.go` — `Toolset` (L190-258): `StaticToolset`, `FilterToolset`, `NewFilterToolset`, `AllowedToolsPredicate`。
3. `tool/tool.go` — `WithConfirmation` (L360-444): 三段式 Run——检查确认状态→判断是否需要确认→执行/返回确认错误。
4. `tool/streaming_tool.go` — `StreamingFunctionTool`, `StreamChunk`, `CollectStreamChunks` (全文件 66 行)。
5. `tool/context.go` — `ToolContext` 接口 (全文件 76 行): `InvocationContext()`, `RequestConfirmation()`, `Actions()`。
6. `flow/flow.go` — FunctionCall 生命周期 (L368-496): preprocess → injectToolDeclarations → callModel → handleFunctionCalls → executeToolCall → mergeResultsToEvent。
7. `flow/flow.go` — `lookupTool` 分派 (L475-492): type switch 区分 `StreamingFunctionTool` / `ContextFunctionTool` / `FunctionTool`。

### 演示建议

1. **Filtered Tools**（2 min）：一个 agent 同时有 `get_weather` 和 `delete_data`，通过 `FilterToolset` + `AllowedToolsPredicate` 让 LLM 只能看到前者。
2. **Confirmed Tool Call**（2 min）：`deploy_app` 需审批，第一次调用返回 `ErrConfirmationRequired`，`SetConfirmed(true)` 后第二次执行真实逻辑。
3. **Rejected Confirmation**（1 min）：`SetConfirmed(false)` → `ErrConfirmationRejected`。
4. **Streaming Tool Non-Live**（2 min）：`generate_report` 返回三个 chunks，`CollectStreamChunks` 合并为单条结果。
5. **Long-Running Tool**（1 min）：`NewLongRunningFunctionTool` → declaration description 包含 "Do not call again" 注解。

### 容易误解点

1. **Confirmation 逻辑四重复制**：`functionTool.Run`, `streamingFunctionTool.RunStream`, `mcpTool.Run`, `confirmationTool.Run` 四处包含几乎相同的 24-27 行确认逻辑。
2. **Declaration 与 execution 不匹配**：`resolvedSchema` 中的 `TODO: check if override schema is compatible with T` 意味着自定义 schema 与泛型类型不一致时不会在创建时报错。
3. **Long-Running 的"LLM 依赖"**：框架不阻止 LLM 重复调用——它依赖于 LLM 的遵循能力。
4. **Streaming Non-Live 丢失增量语义**：`CollectStreamChunks` 将所有 chunk 合并为单条结果，完全失去流式低延迟语义。
5. **`nil` 作为 interface 的 typed nil 陷阱**：`*jsonschema.Schema` 赋值给 `interface{}` 时，nil 指针不会转成 nil interface。
6. **`ConfirmationProvider` 签名不一致**：`functiontool` 的 Provider 签名（类型安全，编译时检查）vs `tool` 包级别签名（运行时类型检查）。

### 练习题

- **Q1**：画出 FunctionCall → ToolContext → FunctionResponse 的完整生命周期。
- **Q2**：用 `NewFunctionToolWithDeclaration` 创建带 JSON Schema 声明的工具。
- **Q3**：实现 `WithConfirmation` + `ConfirmationProvider`，当 `args["env"] == "prod"` 时才要求确认。
- **Q4**：对比 `FilterToolset` vs `WithConfirmation` 的安全职责。
- **Q5**：`StreamingFunctionTool` 在 Live 与 Non-Live 模式下的行为差异。
- **Q6**：如何避免 Confirmation 逻辑的四重复制？

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 对应小节 | 测试文件 |
|------|-------------|------|----------|---------|
| `tool/tool.go` | `Tool`, `Declaration`, `DeclarationProvider`, `FunctionTool`, `FuncTool`, `NewFunctionTool`, `NewLongRunningFunctionTool` | 34-135 | 3.1-3.3 | `tool_test.go` |
| `tool/tool.go` | `CallResult`, `Execute`, `MergeResults` | 137-184 | 3.6 | `tool_test.go` |
| `tool/tool.go` | `Toolset`, `StaticToolset`, `FilterToolset`, `AllowedToolsPredicate` | 190-258 | 3.4 | `tool_test.go` |
| `tool/tool.go` | `RequestProcessor`, `InjectDeclarations`, `CollectDeclarations` | 265-306 | 3.5 | `tool_test.go` |
| `tool/tool.go` | `ContextFunctionTool`, `WithConfirmation`, `ConfirmationControl`, `confirmationTool` | 318-497 | 3.3 | `tool_confirmation_streaming_long_running_test.go` |
| `tool/streaming_tool.go` | `StreamFuncTool`, `StreamingFunctionTool`, `StreamChunk`, `CollectStreamChunks`, `ExecuteStream` | 全文件 66 行 | 3.4 | `tool_confirmation_streaming_long_running_test.go` |
| `tool/context.go` | `ToolContext`, `toolContextImpl`, `NewToolContext`, `RequestConfirmation` | 全文件 76 行 | 3.6 | `tool_confirmation_streaming_long_running_test.go` |
| `flow/flow.go` | `handleFunctionCalls`, `executeToolCall`, `injectToolDeclarations`, `resolveToolsets`, `lookupTool` | 368-496 | 3.6 | `flow_test.go` |
| `flow/flow.go` | `mergeResultsToEvent` | 591-681 | 3.6 | — |
| `cmd/demo/main.go` | `demoFilteredTools`, `demoConfirmedToolCall`, `demoRejectedConfirmation`, `demoStreamingToolNonLive`, `demoLongRunningTool` | 559-807 | 3.1-3.5 | — |

**简化边界汇总**：无 MCP（连接管理/Ping/重连未实现）；无 `typeutil.ConvertToWithJSONSchema`（使用 `map[string]any`）；无 RequestConfirmationRequestProcessor（仅 `WithConfirmation` + `SetConfirmed`）；无 Live bidi streaming；无 Gemini 原生工具；无 AgentTool。

---

## Chapter 04 — Callback / Plugin / Instruction（14 min）

> 复刻代码目录：`callbackctx/`, `plugin/`, `instruction/`, `flow/`, `runner/`
> 核心文件：`callbackctx/callbackctx.go`, `plugin/plugin.go`, `plugin/manager.go`, `instruction/instruction.go`, `flow/flow.go`
> 精读报告对应：`manual/deep-read/04-callback-plugin-deep-dive.md`

### 讲解目标

1. 区分 Callback / Plugin / Instruction 三个层次。
2. 理解 early-exit 短路语义：Before* hook 返回非 nil 结果会跳过其后所有 hook 以及实际调用。
3. 解释 Plugin 优先于 Direct Callback 的执行顺序。
4. 写出 Instruction Provider + `InjectSessionState` 模板替换。
5. 在 `CallbackContext/ToolContext` 中操作 state delta，理解 write-through 机制。

### 问题背景

运行时中存在多个横切关注点（日志、缓存、错误重试、状态注入、权限校验），每种需求都侵入 `flow.Flow` 核心逻辑会导致：核心与非核心逻辑耦合、hook 执行顺序不可控、测试困难、缺少统一的 state 变更追踪。

### 为什么难

- **State 写穿策略无事务回滚**：`State().Set()` 同时写入 delta 和真实 session state，即使后面回调出错也无法回滚。
- **多个 Callback/Plugin 在同一 hook 点执行**：优先级完全由注册顺序决定，无法显式声明依赖。
- **Early-exit 语义细节**：`BeforeModel` 返回非 nil 不仅跳过后续 BeforeModel hook，还跳过了真实 LLM 调用。但后续 AfterModel 仍会在 fake response 上执行。
- **Plugin 和 Callback 类型大量重复但生命周期不同**：Plugin 跨 agent 复用，Callback 单 agent 定制。

### 核心抽象

**三层扩展架构**：

```
Plugin Layer (PluginManager, 顺序执行 + early-exit)
  ├─ BeforeAgent / AfterAgent
  ├─ BeforeModel / AfterModel / OnModelError
  ├─ BeforeTool / AfterTool / OnToolError

Callback Layer (Flow 结构体上的直接函数列表)
  ├─ RequestProcessors / ResponseProcessors
  ├─ BeforeModelCallbacks / AfterModelCallbacks
  ├─ BeforeToolCallbacks / AfterToolCallbacks

Instruction Layer (RequestProcessor 实现)
  ├─ GlobalInstruction / GlobalInstructionProvider (仅 root)
  ├─ Instruction / InstructionProvider (当前 agent)
  └─ InjectSessionState ({placeholder} 替换)
```

**Hook 排序矩阵**：

| Hook 类型 | 返回 (nil, nil) | 返回 (non-nil, nil) | 返回 (_, err) |
|-----------|----------------|---------------------|---------------|
| BeforeAgent | 继续 agent run | 终止 agent run, 创建 Event | 终止，返回 error |
| BeforeModel | 继续 LLM 调用 | 跳过 LLM 调用，用此响应 | 终止，返回 error |
| BeforeTool | 继续 tool 调用 | 跳过 tool，用此结果 | 终止，返回 error |
| AfterModel | 使用原始响应 | 替换 LLM 响应 | 终止，返回 error |
| OnModelError | 透传原始 error | 替换 error 为成功响应 | 终止，返回新 error |

### 复刻版代码走读

1. `callbackctx/callbackctx.go` — `ReadonlyContext` (L22), `CallbackContext` (L37), `ToolContext` (L48): 三层上下文接口 + 最小权限原则。
2. `plugin/plugin.go` — `Plugin` 结构体 (L23-65), `Config` (L40-55), `New` (L56): 每个 hook 字段可以为 nil。
3. `plugin/manager.go` — `Manager`, `Register` (L32), `RunBeforeModelCallback` (L93): **顺序执行 + early-exit**——第一个非 nil 胜出，任何 err 中断整个链。
4. `flow/flow.go` — `callModel` 时序 (L241-313): Plugin → Ctx callback → Legacy callback → model → Plugin(OnError+After) → Ctx callback → Legacy callback。
5. `flow/flow.go` — `callTool` 时序 (L498-589): Plugin → Callback → tool.Run → OnError → Plugin After Tool → Callback After Tool。
6. `instruction/instruction.go` — Instruction 四种来源 (L34-80): `InjectSessionState` 模板替换支持 `{varName}`, `{varName?}`, `{app:key}`, `{user:key}`, `{temp:key}`。
7. `flow/flow.go` — callback context 状态传播: `callbackContextState.Set` (同时写入 delta 和 durable state) 是 write-through 策略。

### 演示建议

1. **Logging Plugin（纯观察者）**（2 min）：所有 hook 返回 `nil, nil`，纯日志输出不干预。
2. **Before-Model Cache（控制流干预）**（2 min）：BeforeModel 返回非 nil `LLMResponse` → 跳过真实 LLM 调用。
3. **Instruction Interpolation**（3 min）：Instruction Processor 在 preprocess 中从 session state 读取 `user_name`、`user_role` 注入 system instruction。
4. **Plugin Ordering**（2 min）：注册 plugin-a 先于 plugin-b，验证执行顺序：`plugin-a:beforeModel → plugin-b:beforeModel → direct:beforeModel-1`。

### 容易误解点

1. **"Plugin 和 Callback 是一样的"** → Plugin 是自包含 hook 集合（有 name, 可复用），通过 PluginManager 注册；Callback 是直接挂在 Flow 上的函数列表。
2. **"Early-exit 只跳过后续同类型 hook"** → `BeforeModel` 返回非 nil 不仅跳过后续 BeforeModel hook，还**跳过了真实 LLM 调用**。但 AfterModel 会在 fake response 上执行。
3. **"`State().Set()` 在 early-exit 后会回滚"** → write-through 策略，**同时写入 delta 和真实 state**，即使 early-exit 已执行 Set() 已持久化。
4. **"Instruction Provider 和 Instruction 是互斥的"** → 两者拼接，顺序：GlobalInstruction → GlobalInstructionProvider → Instruction → InstructionProvider → InjectSessionState。
5. **"`CallbackContext` 可以调用 `EndInvocation()`"** → 嵌入的是 `ReadonlyContext`，`EndInvocation()` 不在其接口中。

### 练习题

- **Q1**：写出 logging plugin 的 `BeforeModel` hook 完整签名和返回策略。
- **Q2**：BeforeModel cache plugin 返回非 nil 响应后，后续 BeforeModel hook 和 AfterModel hook 分别还会执行吗？
- **Q3**：写一个 BeforeTool plugin，当 tool name 为 `"delete_user"` 时跳过真实调用。
- **Q4**：为什么 `InjectSessionState` 的 `ReadonlyState` 需要预先 merge app+user+session 三层 state？
- **Q5**：如果两个 plugin 都注册了 `BeforeModel`，P1 要求比 P2 先执行，能否保证？
- **Q6**：`Instruction` 的 `RequestProcessor` 为什么在 `preprocess` 阶段运行，而不是在 `callModel` 的 BeforeModel hook 中？

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 建议讲解点 | 测试文件 |
|------|-------------|------|-----------|---------|
| `callbackctx/callbackctx.go` | `ReadonlyContext`, `CallbackContext`, `ToolContext` | 22,37,48 | **三层上下文接口 + 最小权限原则** | — |
| `plugin/plugin.go` | `Plugin`, `Config`, `New` | 23,40,56 | Plugin 结构体 + 可选 hook 字段 | `plugin_test.go` |
| `plugin/manager.go` | `Manager`, `Register`, `RunBeforeModelCallback` | 11,32,93 | **顺序执行 + early-exit 策略** | `plugin_test.go` |
| `instruction/instruction.go` | `Provider`, `Config`, `NewRequestProcessor`, `InjectSessionState`, `MergeStateView` | 34,83,119,50,178 | Instruction 四种来源 + 模板替换 | `instruction_test.go` |
| `flow/flow.go` | `callModel`, `callTool`, `executeToolCall`, `preprocess` | 241,399,498,225 | **完整 hook 执行序列** | `flow_test.go` |
| `cmd/demo/main.go` | `runChapter04`, `demoPluginLogging`, `demoBeforeModelCache`, `demoInstructionInterpolation`, `demoPluginOrdering` | 813-1219 | 4 个课堂演示 | — |

**关键测试索引**：`TestRunnerInstructionProcessor`, `TestRunnerDynamicInstructionProvider`, `TestRunnerGlobalInstruction`, `TestRunnerInstructionTemplateInjection`, `TestRunnerPluginBeforeModelEarlyExit`, `TestRunnerPluginOrdering`, `TestRunnerPluginAfterModelTransform`, `TestRunnerFullChainInstructionPluginCallback`.

---

## Chapter 05 — Workflow / AgentTool / Remote A2A（14 min）

> 复刻代码目录：`workflow/` + `tool/agenttool/` + `agent/remoteagent/`
> 核心文件：`workflow/workflow.go`, `tool/agenttool/agent_tool.go`, `agent/remoteagent/remote_agent.go`, `agent/remoteagent/aggregate.go`
> 精读报告对应：`manual/deep-read/05-workflow-a2a-deep-dive.md`

### 讲解目标

1. 口头解释 Sequential/Parallel/Loop 是 **agent-as-composition** 模式——它们组合子 agent 的 `Execute()` 事件流，不是外部调度器。
2. 理解 **AgentTool** 的边界语义：把 Agent 塞进 Tool 接口，子 agent 运行在隔离 session 中。
3. 理解 **Remote A2A** 的桥接模式：把远程流转换成本地事件流，`aggregator` 把 partial chunks 合并为完整事件。
4. 区分三种组合维度的 session 语义：共享 session（workflow）、隔离 session（AgentTool）、透明 session（Remote A2A）。

### 问题背景

**三条故事线**：
1. **代码审查流水线**：生成→审查→修复，用 SequentialAgent 串起来。多视角并发审查（性能、安全、风格）用 ParallelAgent。反复修复用 LoopAgent。
2. **Agent 分身术**：主 agent 遇到数学问题需调数学专家。但专家不是"工具"——它是一个完整 agent。AgentTool 把 agent 包装成 tool 接口。
3. **远程 agent 透明化**：agent 需访问另一个进程上的知识库 agent。RemoteAgent 将远程 A2A 流转换成本地 event 流。

### 为什么难

- **状态共享与隔离**：Sequential 共享 session；Parallel 共享 session 有竞态；AgentTool 创建全新隔离 session，但状态复制是单行道。
- **背压机制**：原版 ADK Go 用 `ackChan` 确保 runner 处理完当前事件才释放子 agent 产下一个事件。**复刻版简化**为收集到 slice 后拼接，无背压。
- **Partial 聚合**：Remote A2A 的 streaming 模式中多个 chunks 需要 `aggregator` 按 Append/LastChunk 标志做 partial 到 full 的转换。
- **AgentTool 同步阻塞**：`Tool.Run()` 是同步方法，父 LLM 必须等待子 agent 完全执行完毕。

### 核心抽象

**三者对比**：

| 维度 | Workflow Agent (Seq/Par/Loop) | AgentTool (Agent as Tool) | Remote A2A |
|------|-------------------------------|---------------------------|------------|
| Session | 共享同一 session | 隔离 child session，拷贝非 `_adk` 父状态 | 远程 session（本地不可见）|
| 调用方式 | 同进程 `Execute()` 调用 | 同步 `Tool.Run()`，阻塞父 LLM | 网络 RPC 流，流式转换 |
| Event 模型 | 子 agent 事件流经父迭代器 | 子事件内部收集，返回最后文本 | A2A event → Converter → aggregator → session event |
| 状态共享 | 直接共享 session state | 拷贝父状态（写隔离），不回写 | 不适用 |

### 复刻版代码走读

1. `workflow/workflow.go` — `SequentialAgent.Execute` (L144): 11 行核心逻辑——遍历→Execute→append events→检查 EndInvocation。`subCtx` (L64) 包装 InvocationContext，子 agent 的 `EndInvocation()` 不意外终止整个 workflow。
2. `workflow/workflow.go` — `ParallelAgent.Execute` (L244): goroutine + channel + WaitGroup。`ev.Branch = "parent.child"` branch label 注入。按 index 排序保证确定性输出。
3. `workflow/workflow.go` — `LoopAgent.Execute` (L376): for 循环迭代子 agent，`Actions.Escalate` 是合作式终止信号。`maxIterations=0` 表示无限循环。
4. `tool/agenttool/agent_tool.go` — `runWithContext` (L73): 创建独立 InMemorySessionService → 拷贝父 session 的非 `_adk` 状态 → 独立 runner → Run → 遍历 events 找最后一个文本 → 返回 `{"result": lastText}`。
5. `agent/remoteagent/remote_agent.go` — `Execute` (L108): `A2AClientProvider.CreateClient` → `SendStreamingMessage` → for each StreamEvent → Converter → aggregator → append events → invokeCleanupCallbacks。
6. `agent/remoteagent/aggregate.go` — `process` (L45): 根据 RemoteEvent 类型（TaskStatusUpdate/TaskArtifactUpdate/Message）和 Append/LastChunk 标志决定 flush/buffer/accumulate。

### 演示建议

1. **Sequential Workflow**（2 min）：coder → reviewer 两个子 agent，事件顺序展示，共享 session 使 coder 的状态 reviewer 可见。
2. **Parallel Workflow**（2 min）：三个子 agent 并发执行，输出按声明顺序排列，`ev.Branch` 区分来源。
3. **Loop Workflow**（2 min）：fixer 在第 3 次迭代时设置 `Actions.Escalate=true` 终止循环。
4. **AgentTool Delegation**（2 min）：父 LLM 通过 function call 调用子 agent，展示 tool call → tool result 完整链路。
5. **Remote A2A Streaming**（2 min）：4 个 Append chunks → aggregator → 1 个完整非 partial event。

### 容易误解点

1. **"Sequential/Parallel/Loop 是外部调度器"** → 它们实现了 `runner.ExecutableAgent`，本质上是组合子 agent 的 Execute 事件流。没有外部调度器，只有"agent 组合 agent"。
2. **"ParallelAgent session 写隔离"** → 当前实现共享同一个 session，并发 `State().Set()` 行为非确定。**教学简化**。
3. **"AgentTool 子 agent 状态会回写父 session"** → 不回写。子 session 是全新创建的，状态变更不会传播。
4. **"AgentTool 可以异步委派"** → `Tool.Run()` 是同步方法，不支持"边流式传输边处理"。
5. **"Remote A2A converter 输出是最终事件"** → converter 输出可能带有 `Partial=true`，必须经过 `aggregator`。
6. **"Loop 终止是强制机制"** → `Escalate` 是合作式信号。如果子 agent 忘记设置，loop 会继续直到 maxIterations 耗尽。

### 练习题

- **Q1**：Sequential Agent 中 coder 设置 `code_language=go`，reviewer 读取。SequentialAgent 共享 session 下需要几步？
- **Q2**：ParallelAgent 注册顺序 [B, A, C]（A 慢、B 快、C 中），输出 event 顺序？
- **Q3**：AgentTool 子 agent 能否读到父 session 的 `api_key=sk-xxx`？哪些 key 读不到？
- **Q4**：RemoteA2A 收到 Event1 (Append, !LastChunk, "Hello ") + Event2 (TaskStatusUpdate, completed)，aggregator 输出几个 event？
- **Q5**：`maxIterations=0` 无限循环时，如果子 agent 永远不设置 Escalate，如何停止？
- **Q6**：AgentTool 和 SequentialAgent 的本质区别？什么场景用哪个？

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 讲解点 | 测试文件 |
|------|-------------|------|--------|---------|
| `workflow/workflow.go` | `SequentialAgent`, `ParallelAgent`, `LoopAgent`, `subCtx`, `pResult` | 51,91,184,318,64,237 | **组合模式核心** | `workflow_test.go` |
| `tool/agenttool/agent_tool.go` | `agentTool`, `New`, `Declaration`, `Run`, `runWithContext` | 23,34,46,60,73 | **sandbox 隔离** | `agent_tool_test.go` |
| `agent/remoteagent/remote_agent.go` | `RemoteAgent`, `NewRemoteAgent`, `Execute`, `invokeCleanupCallbacks` | 74,51,108,200,242 | **流式桥接** | `remoteagent_test.go` |
| `agent/remoteagent/aggregate.go` | `aggregator`, `process`, `flush`, `accumulate`, `terminalFlush` | 25,45,82,131,171 | **partial→full 聚合** | `remoteagent_test.go` |
| `agent/remoteagent/convert.go` | `DefaultConvertToSessionEvent`, `ConvertSessionEventToRemote` | 19,126 | **双向转换** | `remoteagent_test.go` |
| `agent/remoteagent/fake_client.go` | `FakeA2AClient`, `FakeA2AClientConfig` | — | **教学简化：无网络** | `remoteagent_test.go` |
| `cmd/demo/main.go` | `runChapter05`, `demoSequentialWorkflow`, `demoParallelWorkflow`, `demoLoopWorkflow`, `demoAgentToolDelegation`, `demoRemoteA2AStreaming` | 1743-2029 | 5 个完整示例 | — |

---

## Chapter 06 — Entrypoint / Deploy / Telemetry（10 min）

> 复刻代码目录：`cmd/launcher/`, `server/adkrest/`, `deploy/`, `telemetry/`
> 核心文件：`cmd/launcher/launcher.go`, `cmd/launcher/universal/universal.go`, `cmd/launcher/console/console.go`, `server/adkrest/server.go`, `deploy/cloudrun.go`, `deploy/agentengine.go`, `telemetry/telemetry.go`, `telemetry/instrumentation.go`
> 精读报告对应：`manual/deep-read/06-entrypoint-deploy-deep-dive.md`

### 讲解目标

1. 解释 `launcher.Config` 为什么是入口层和运行时层之间的稳定协议。
2. 画出 `Launcher → SubLauncher → universal 路由 → console/web` 的分层结构。
3. 理解 dry-run deploy plan 的教学价值：相同输入→相同输出，"部署即文档"。
4. 理解 telemetry 的分层设计：public config → instrumentation helpers → runner/model/tool 层不关心 exporter。
5. 区分 telemetry span 四种语义：`invoke_agent`, `generate_content`, `execute_tool`, `server <method> <path>`。

### 问题背景

前五章都在说 agent runtime 怎么跑。但 runtime 写好了，怎么暴露出来？本地调试需要 stdin/stdout console、测试需要 HTTP JSON/SSE、云上部署需要 Cloud Run / Agent Engine、线上观测需要 telemetry。如果每种入口写一套 agent 初始化代码，切换入口就等于重写 main.go。

### 为什么难

入口层天然会引入很多和核心 runtime 无关的差异：console 是阻塞式 stdin/stdout，REST 是 request/response，SSE 需要 flush 和 streaming 语义，Cloud Run / Agent Engine 关心镜像、端口、启动命令，telemetry 又要横跨 runner/model/tool/server。难点不是"写一个 main"，而是让这些入口共享同一个 agent 构造、同一套 session/memory/artifact/plugin 服务，并且不让部署和观测逻辑反向污染 Flow / Agent / Tool 的核心语义。

### 核心抽象：launcher.Config 作为稳定协议

```
launcher.Config (cmd/launcher/launcher.go:36)
  ├── AgentLoader → RootAgent()
  ├── SessionService → runner.SessionService
  ├── MemoryService → memory.Service
  ├── ArtifactService → artifact.Service
  └── PluginManager → *plugin.Manager
       ↓
  launcher.Launcher
       ↓
  universal.New(console, web)
       ↓
  no args → console.Run()
  "web"   → web.Run()
```

**"同一个 agent runtime 不感知自己在哪里被调用"**。入口层只做生命周期和协议转换，不修改运行时语义。

### 复刻版代码走读

1. `cmd/launcher/launcher.go` — `Config` (L36), `Launcher` (L47), `SubLauncher` (L55), `AgentLoader` (L61)。
2. `cmd/launcher/universal/universal.go` — `parse` 路由策略 (L44)，第一个 argv token 匹配 sublauncher keyword，不传参默认第一个 sublauncher。
3. `cmd/launcher/console/console.go` — `Run` (L91): 创建 session → 创建 runner → 逐行 `runner.Run`。
4. `server/adkrest/server.go` — `runHandler` (L64), `runSSEHandler` (L94), `runAgent` (L149): HTTP JSON + SSE 双协议，共享 runAgent 逻辑。
5. `deploy/cloudrun.go` — `PlanCloudRun` (L54), `cloudRunDockerfile` (L106): 生成 distroless Dockerfile, CMD 为 `/app/<exec> web -port <port> [api] [a2a] [webui]`。
6. `deploy/agentengine.go` — `PlanAgentEngine` (L62), `agentEngineDockerfile` (L109): multi-stage Dockerfile + class methods + stream query endpoint。
7. `telemetry/telemetry.go` — `Recorder` (L53): 内存 span/log 累加器。`Providers` (L159), `WithCaptureMessageContent` (L147)。
8. `telemetry/instrumentation.go` — 四类 span helper (L15-66): `StartInvokeAgentSpan`, `StartGenerateContentSpan`, `StartExecuteToolSpan`, `StartServerEventSpan`。

### 演示建议

1. **Launcher Config 解耦**（3 min）：同一 Config 被 console 和 web server 复用，展示入口切换不修改 agent 代码。
2. **Dry-Run Deploy Plan**（3 min）：生成 Cloud Run plan 并打印 Dockerfile，展示确定性输出。
3. **Telemetry Span 捕获**（2 min）：`StartInvokeAgentSpan` → `span.End("OK")` → `Recorder.Spans()` 输出。
4. **Console vs REST 对比**（2 min）：console 是 `runner.Run` 最薄包装，REST 做 JSON 编解码 + SSE flush/reconnect。

### 容易误解点

1. **"Deploy plan 是真正的部署工具"** → 是 dry-run 快照，不执行 gcloud/docker。真正部署需用户按 plan 手动执行。
2. **"launcher.Config 是全局单例"** → 是一个 struct，每个入口持有自己的副本。
3. **"console 和 web 必须分开 binary"** → universal launcher 在同一 binary 中通过 argv token 路由。
4. **"Telemetry Recorder 就是 OTel"** → 是 in-memory 教学替代，不依赖 OTel SDK 或 GCP exporter。
5. **"adkrest 是生产级 REST 框架"** → 复刻版教学简化：无认证、无 rate limit、无中间件链。

### 练习题

- **Q1**：写出 `launcher.Config` 的五个字段及其对应 runtime 服务。
- **Q2**：`universal.New(console, web)` 中 console 是默认入口，如何让 web 成为默认？
- **Q3**：`CloudRunPlan` 的 Dockerfile CMD 中 `"api", "-webui_address", "..."` 的 webui_address 用途？
- **Q4**：说明 `telemetry.StartExecuteToolSpan` 的 span 在 `Recorder.Spans()` 中的结构。
- **Q5（设计）**：增加 gRPC sublauncher（`grpc` 关键字），需要实现 SubLauncher 的哪些方法？

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 建议讲解点 |
|------|-------------|------|-----------|
| `cmd/launcher/launcher.go` | `Config`, `Launcher`, `SubLauncher`, `AgentLoader` | 30-61 | 入口层稳定协议 |
| `cmd/launcher/universal/universal.go` | `uniLauncher`, `New`, `Execute`, `parse` | 18-78 | argv token 路由 |
| `cmd/launcher/console/console.go` | `Console`, `Run`, `processLine` | 38-180 | 最薄 runner 包装 |
| `server/adkrest/server.go` | `Server`, `runHandler`, `runSSEHandler`, `runAgent` | 17-275 | HTTP JSON + SSE 双协议 |
| `deploy/cloudrun.go` | `CloudRunPlan`, `PlanCloudRun`, `cloudRunDockerfile` | 13-216 | 确定性 dry-run Cloud Run |
| `deploy/agentengine.go` | `AgentEnginePlan`, `PlanAgentEngine`, `agentEngineDockerfile` | 9-230 | 确定性 dry-run Agent Engine |
| `telemetry/telemetry.go` | `SpanRecord`, `LogRecord`, `Recorder`, `Providers` | 30-225 | 内存 span/log 累加器 |
| `telemetry/instrumentation.go` | `StartInvokeAgentSpan`, `StartGenerateContentSpan`, `StartExecuteToolSpan`, `StartServerEventSpan` | 15-66 | 四类 span helper |

---

## Chapter 07 — Agent Flow / ReAct / Multi-Agent（14 min）

> 复刻代码目录：`flow/`, `tool/transfer/`, `tool/exitloop/`, `plugin/retryreflect/`, `plugin/functionmodifier/`, `agent/agentconfig/`
> 核心文件：`flow/flow.go`, `tool/transfer/tool.go`, `tool/exitloop/exitloop.go`, `plugin/retryreflect/plugin.go`, `plugin/functionmodifier/plugin.go`, `agent/agentconfig/config.go`
> 精读报告对应：`manual/deep-read/07-agent-flow-react-multi-agent-deep-dive.md`

### 讲解目标

1. 口述 ReAct 不是魔法，是 model/tool/event loop：`Flow.Run` 的 `for { runOneStep → IsFinalResponse? }` 核心循环。
2. 解释 `transfer_to_agent` 如何把控制权交给子 agent：Host agent 输出 ToolCall → Flow 检测 `Actions.TransferToAgent` → `executeTransfer` 递归执行子 agent。
3. 讲清 runner 如何从历史事件恢复 active agent：`findAgentToRun` 反向扫描 session events → `isTransferableAcrossAgentTree`。
4. 区分三个策略插件：`ExitLoop`（主动终止）、`RetryReflect`（工具失败反思）、`FunctionModifier`（Hidden Args 注入与剥离）。
5. 理解 JSON configurable agent tree 如何把前面章节串起来。

### 问题背景

手写 agent loop 的缺陷：终止条件内容驱动不可预测、Message history 嵌套隔离困难、Streaming 下判断 tool call 困难、工具结果可能直接是最终答案。

### 为什么难

Agent 的真正复杂度不在循环本身，而在于**可插拔的决策点**：
- **StreamToolCallChecker**：不同 provider 流式输出顺序不同，判断"有无 tool call"时机需要可注入。
- **transfer_to_agent 动态工具构造**：`agent_name` 枚举取决于 agent 在树中的位置，决策分散在四层（LLM 选择 → Tool.Run 设置动作 → Flow.runOneStep 执行转移 → Runner.findAgentToRun 下一回合恢复）。
- **ExitLoop / RetryReflect / Hidden Args 三个策略插件**解决三个不同的生产问题，但都通过 Plugin 机制注入 Flow 的 callModel/handleFunctionCalls 流程。
- **JSON Config 构建**：Build→validateNoDuplicateNames→validateToolRefs→buildNode→wireParents，验证子树、parent chain、约束校验。

### 核心抽象

**ReAct Graph 拓扑**：

```
START → ChatModel ──(ToolCall?)──→ Tools → ChatModel (loop, Pregel 驱动)
                 └──(no ToolCall)──→ END
                 Tools ──(returnDirectly?)──→ direct_return → END
```

**Flow.Run 主循环** (flow.go:103)：
```go
for step := 1; ; step++ {
    if ctx.Ended() { return allEvents, nil }
    stepEvents, err := f.runOneStep(ctx, step)
    allEvents = append(allEvents, stepEvents...)
    for _, ev := range stepEvents {
        if ev != nil && ev.Actions.EndInvocation { return allEvents, nil }
    }
    modelEvent := stepEvents[0]
    if modelEvent.IsFinalResponse() { return allEvents, nil }
}
```

**Agent Transfer 流程**：
1. `InjectTransferTool` (transfer.go:197): 注入 `transfer_to_agent` 工具声明 + 系统指令。
2. LLM 输出 `FunctionCall{Name: "transfer_to_agent", Args: {agent_name: "..."}}`。
3. `executeTransfer` (flow.go:767): 递归调用目标 agent 的 Execute。
4. `findAgentToRun` (runner.go:192): 下一轮用户消息反向扫描 session events 定位到最近可转移的 agent。

### 复刻版代码走读

1. `flow/flow.go` — `Flow.Run` (L103): 核心 ReAct 循环。`runOneStep` (L151) 五个子阶段。`callModel` (L241) 回调链 PLUGIN → CALLBACK → MODEL → CALLBACK。
2. `agent/agent.go` — `baseAgent.Execute` (L144): beforeAgentCallbacks → a.run(ctx) → afterAgentCallbacks。
3. `tool/transfer/tool.go` — `ComputeTransferTargets` (L136), `InjectTransferTool` (L197): 四方向转移规则，`TransferInstructions` 系统指令注入。
4. `runner/runner.go` — `findAgentToRun` (L192): 反向扫描 events，跳过 user，沿父链检查可转移性。`isTransferableAcrossAgentTree` (L220): 所有祖先 DisallowTransferToParent 必须为 false。
5. `tool/exitloop/exitloop.go` — `RunWithContext` (L30): 设置 `actions.EndInvocation = true`。Flow.Run 主循环检查到后立即返回。
6. `plugin/retryreflect/plugin.go` — `OnToolError` (L35): 失败计数+反射指导；`AfterTool` (L59): 成功后重置计数。
7. `plugin/functionmodifier/plugin.go` — `BeforeModel` (L35): 注入额外参数声明；`AfterModel` (L80): 剥离匹配的参数存入 session state。
8. `agent/agentconfig/config.go` — `Build` (L67), `FromJSON` (L291), `buildNode` (L128): 按 `cfg.Type` 分发到 `buildLLMAgent`/`buildSequentialAgent`/`buildParallelAgent`/`buildLoopAgent`。三验证：重复名、工具引用、类型值。

### 演示建议

1. **ReAct Loop**（2 min）：完整 4 个事件序列——user → model(fc) → tool(result) → model(final)，跨越两个 runOneStep 迭代。
2. **Agent Transfer**（2 min）：Host agent "host_agent" + specialist "math_agent"，展示 `event.Actions.TransferToAgent` 在输出中可见。
3. **ExitLoop**（1 min）：ExitLoopTool → `EndInvocation=true` → Flow.Run 检测后立即返回。
4. **RetryReflect**（1 min）：AlwaysFailTool + RetryReflectPlugin → `OnToolError` 注入 reflection 字段。
5. **Hidden Args**（1 min）：FunctionCallModifierPlugin BeforeModel 注入 `user_id` → AfterModel 剥离并存入 state。
6. **Configurable Construction**（2 min）：JSON 定义 root + specialist → `agentconfig.FromJSON` → `Build` → 验证子树。

### 容易误解点

1. **"ReAct 循环是无限制的"** → 每次 `runOneStep` 检查 `IsFinalResponse()` 和 `ctx.Ended()`。生产环境应设置 `maxSteps`。
2. **"Transfer 后事件作者还是 host"** → `executeTransfer` 创建 `transferContext` 重写 `Agent()` → 子 agent 事件的 `Author` 是 specialist。
3. **"所有 agent 都可以被转移"** → `isTransferableAcrossAgentTree` 检查整个父链。任一祖先设置 `DisallowTransferToParent=true` 即不可转移。
4. **"ExitLoop 和 EndInvocation 是同一件事"** → ExitLoopTool 设置 `Actions.EndInvocation=true`。但对 LoopAgent，终止信号是 `Actions.Escalate`。消费者不同。
5. **"Hidden Args 在工具声明阶段永久修改 ToolDeclarations"** → `req` 是每轮新创建的 LLMRequest，修改只影响当前 step，不跨迭代持久化。
6. **"Config 构建的 agent 和代码构建的 agent 行为不同"** → 内部调用 `llmagent.New` 和 `workflow.New*Agent`，产出标准 `agent.Agent` 实例。

### 练习题

- **Q1**：画图描述 `Flow.Run` 一次完整迭代，标注 `runOneStep` 五个子阶段和对应 flow.go 行号。
- **Q2**：Host agent A 有子 agent B、C，B 有子 agent D。画 `transfer_to_agent("D")` 的调用链。
- **Q3**：修改 `demoReActLoop`，在 Flow 注册 AfterModelCallback，当模型连续两次输出 tool call 时打印警告。
- **Q4**：为 `demoAgentTransfer` 添加 `DisallowTransferToParent=true` 的 specialist，验证下一轮路由。
- **Q5**：如果不使用 ExitLoopTool，如何让 agent 提前终止 flow？给出两种替代方案。
- **Q6**：从 `tool/transfer/tool.go` 到 `flow/flow.go` 再到 `runner/runner.go`，画出 `transfer_to_agent` 完整信息流图。

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 讲解点 | 测试文件 |
|------|-------------|------|--------|---------|
| `flow/flow.go` | `Flow.Run`, `runOneStep`, `callModel`, `handleFunctionCalls`, `executeTransfer`, `mergeResultsToEvent` | 103,151,241,368,767,591 | **核心 ReAct 循环** + transfer 执行 | `flow_test.go` |
| `agent/agent.go` | `Agent` 接口, `baseAgent.Execute`, `New` | 41,144,176 | Agent 生命周期 | `agent_test.go` |
| `runner/runner.go` | `Runner.Run`, `findAgentToRun`, `isTransferableAcrossAgentTree` | 124,192,220 | 会话 + 路由 + 可转移性检查 | `runner_test.go` |
| `tool/transfer/tool.go` | `TransferToAgentTool`, `ComputeTransferTargets`, `InjectTransferTool`, `TransferInstructions` | 24,136,197,174 | **动态工具注入** | `tool_test.go` |
| `tool/exitloop/exitloop.go` | `ExitLoopTool.RunWithContext` | 30 | EndInvocation 信号 | `exitloop_test.go` |
| `plugin/retryreflect/plugin.go` | `New`, `OnToolError`, `AfterTool` | 23,35,59 | 失败计数 + 反射指导 | `plugin_test.go` |
| `plugin/functionmodifier/plugin.go` | `New`, `BeforeModel`, `AfterModel` | 26,35,80 | Hidden Args 注入/剥离 | `plugin_test.go` |
| `agent/agentconfig/config.go` | `Build`, `FromJSON`, `buildNode`, `buildLLMAgent`, `validateNoDuplicateNames`, `validateToolRefs` | 67,291,128,145,87,104 | **JSON config loader** | `config_test.go` |
| `cmd/demo/main.go` | `runChapter07`, `demoReActLoop`, `demoAgentTransfer`, `demoPolicyExtensions`, `demoConfigurableConstruction` | 1225-1727 | 四个教学 demo | N/A |

---

## 跨章节总图

### Runtime Chain（Ch01 + Ch04 + Ch07）

```
                                Runner.Run
                                   │
                    ┌──────────────┼──────────────┐
                    │              │              │
              Session Mgt    findAgentToRun   Telemetry Span
                    │              │              │
                    └──────────────┼──────────────┘
                                   │
                            agent.Execute
                          (before/after callbacks + Plugin)
                                   │
                              Flow.Run
                          (for { runOneStep })
                                   │
                    ┌──────────────┼──────────────┐
                    │              │              │
              preprocess      callModel       postprocess
           (RequestProcessors,  (Plugin→CB→    (Response
            Instruction注入)     Model→CB)      Processors)
                    │              │              │
                    └──────────────┼──────────────┘
                                   │
                    ┌──────────────┼──────────────┐
                    │              │              │
          finalizeEvent    handleFunctionCalls   handleTransfer
          (yield event)    (并行 goroutine)    (递归 Execute)
                                   │
                              session.AppendEvent
                          (仅 non-partial 事件)
```

### State / Tool / Plugin / Control-Plane Chain（Ch02 + Ch03 + Ch04）

```
        Event Actions (StateDelta / ArtifactDelta / RequestedToolConfirmations)
               │
               ▼
     CallbackContext State (write-through)
               │
      ┌────────┴────────┐
      │                  │
  session.State     Service.State
  (temp + session)   (app + user + session merged)
      │                  │
      ▼                  ▼
  AppendEvent → ExtractStateDeltas → updateAppState / updateUserState
      │
      ▼
  trimTempDeltaState → removeTempKeysFromState

  ------------------------------------------

  Tool Declaration Chain (Ch03):
  Flow.preprocess
    → toolProcessor (resolveToolsets)
    → injectToolDeclarations
    → InjectDeclarations (CollectionProvider → LLMRequest.ToolDeclarations)
    → callModel → LLM 返回 FunctionCall
    → handleFunctionCalls
        → lookupTool → type switch
            StreamingFunctionTool → ExecuteStream
            ContextFunctionTool   → ContextExecute
            FunctionTool          → Execute
        → mergeResultsToEvent

  Plugin/Callback Chain (Ch04):
  PluginManager.RunBefore* (顺序 early-exit)
    → Flow.Before*CallbacksCtx (顺序 early-exit)
    → Flow.Before*Callbacks (顺序 early-exit)
    → actual call
    → PluginManager.RunAfter*
    → Flow.After*CallbacksCtx (替换 break)
    → Flow.After*Callbacks (替换 break)
```

### Multi-Agent Chain（Ch05 + Ch07）

```
        User Message
              │
              ▼
     Runner.Run → findAgentToRun (反向扫描 session history)
              │
        ┌─────┴─────┐
        │           │
   Root Agent   Specialist Agent (从历史恢复)
   (LLMAgent)   (transfer target)
        │           │
        ▼           ▼
   Flow.Run ──transfer_to_agent──→ executeTransfer
        │                              │
     SequentialAgent              AgentTool (隔离 session)
     ParallelAgent                RemoteA2A (透明桥接)
     LoopAgent
        │
        ▼
   Session 持久化 (仅 non-partial)
        │
        ▼
   yield events → 返回给调用方
```

---

## 讲师提示

### Replica 简化（教学时需明确标注）

| 简化项 | Replica 行为 | ADK Go 原版 |
|--------|-------------|-------------|
| Event 模型 | `[]*event.Event` 预收集 | `iter.Seq2[*session.Event, error]` 惰性迭代器 |
| Parallel 背压 | 无 per-event ackChan | `ackChan` per-event round trip |
| Stream Copy | eager copy（先 drain 再切片拷贝） | lazy copy |
| MCP 工具 | 未实现 | 完整连接管理 + Ping + 重连 |
| Gemini 原生工具 | 未实现 | `genai.Tool` 集成 |
| 流式工具 | 仅 non-live `CollectStreamChunks` | Live bidi streaming |
| Schema 类型推断 | 使用 `map[string]any` 直接传递 | `typeutil.ConvertToWithJSONSchema` |
| Remote A2A 传输 | `FakeA2AClient` (in-memory channel) | REST/gRPC 网络调用 |
| Telemetry | 内存 Recorder | OTel SDK + GCP exporter |
| Deploy plan | 确定性 dry-run 快照 | 真实 gcloud/docker 调用 |
| Confirmation | `WithConfirmation` + `SetConfirmed` 核心模式 | `RequestConfirmationRequestProcessor` event 搜索 |
| AgentTool artifact | 无 artifact 转发 | `forwarding_artifact_service` |
| AgentTool IsLongRunning | 始终 false | 支持长运行工具 |
| Config loader | 仅 JSON, 仅 FakeModel | YAML + 真实模型 + 回调配置 |

### 对应 ADK Go 真代码的说明

- **复刻版的核心抽象直接映射原版**：Runner → `runner/runner.go`, Flow → `internal/llminternal/base_flow.go`, Agent → `agent/agent.go`, Tool → `tool/tool.go`。
- **原版更复杂的关键差异**：
  - `ContentsRequestProcessor`（contents_processor.go:37-187）在每轮请求中从 session events 重建对话历史——**复刻版假设 contents 由外部构建**。
  - `StreamAggregator`（stream_aggregator.go）维护复杂状态机处理流式分块——**复刻版使用 slice-based fake**。
  - `handleFunctionCalls` 中函数调用/响应的重排，涉及并行 goroutine + ackChan——**复刻版用 sync.WaitGroup 简化**。
  - 入口层 `cmd/launcher/` 原版有 console/web/api/a2a/webui/pubsub/eventarc 7 种 sublauncher——**复刻版仅 console + web**。
- **推荐阅读原版路径**：`internal/llminternal/base_flow.go:62-654` 覆盖 Flow 核心循环；`runner/runner.go:131-268` 覆盖 Runner 生命周期；`tool/tool.go:76-149` 覆盖工具系统。

### 适合 Live Coding 的地方

1. **Ch01：最简天气查询 demo**（30 行，展示完整链路）——从 `NewFunctionTool` 到 `Runner.Run`。
2. **Ch03：`NewFunctionTool` + `WithConfirmation`**（20 行，展示确认模式）。
3. **Ch04：Logging Plugin + Cache Plugin**（30 行，展示纯观察 vs 控制流干预）。
4. **Ch05：SequentialAgent + ParallelAgent**（25 行，展示组合模式）。
5. **Ch07：`demoReActLoop` + `demoAgentTransfer`**（35 行，展示 ReAct 和 transfer）。

---

## 后续扩展 DAG 建议

```
                    ADK Go 复刻版教学手册
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
   Ch01 Runtime Flow   Ch05 Multi-Agent    Ch07 ReAct/Config
        │                  │                  │
        ▼                  ▼                  ▼
   ┌──────────┐      ┌──────────┐       ┌──────────┐
   │ 扩展 1：  │      │ 扩展 2：  │       │ 扩展 3：  │
   │ 真实 LLM  │      │ A2A v1   │       │ Live      │
   │ 集成      │      │ 协议实现  │       │ Streaming │
   └──────────┘      └──────────┘       └──────────┘
        │                  │                  │
        ▼                  ▼                  ▼
   ┌──────────┐      ┌──────────┐       ┌──────────┐
   │ 扩展 4：  │      │ 扩展 5：  │       │ 扩展 6：  │
   │ OTel     │      │ Config   │       │ 真实      │
   │ Telemetry│      │ YAML 支持│       │ 部署管道  │
   └──────────┘      └──────────┘       └──────────┘
```

**推荐扩展顺序**：
1. **真实 LLM 集成**：将 `FakeModel` 替换为 `openai.Client` 或 `genai.Client` 的 `GenerateContent` 调用——验证 `LLMRequest`/`LLMResponse` 契约是否能适配真实 Provider。
2. **A2A v1 协议实现**：将 `FakeA2AClient` 替换为真实 HTTP/gRPC 传输，实现 `A2AClient` 接口的 `SendStreamingMessage` 和 `CancelTask`。
3. **Live Streaming**：为 `StreamingFunctionTool` 增加真正的 goroutine-based 流式 push 模式，实现 bidi 通信。
4. **OTel Telemetry**：将 `telemetry.Recorder` 替换为 OpenTelemetry SDK 集成，导出到 stdout 或 GCP。
5. **Config YAML 支持**：在 `agent/agentconfig/` 中增加 YAML 解析器，支持 `agent_class`, `model`, `tools`, `sub_agents` 声明式配置。
6. **真实部署管道**：将 `deploy/cloudrun.go` 的 dry-run plan 与实际 `gcloud run deploy` 集成，增加 `adkgo deploy` CLI 子命令。

---

> 版本：2026-06-11
> 对应复刻代码：`/Users/likun/Desktop/workspace-for-google-adk-go/rive-adk-go/`
> 对应精读总纲：`examples/google-adk-go-code-reading/manual/deep-read/00-final-architecture-guide.md`
> 7 份章节 artifact：`/tmp/rive-adk-teaching-manual-20260611/01-runtime-flow-section.md` ~ `07-agent-flow-section.md`
