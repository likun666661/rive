# ADK-Go 工具系统精读报告

**阅读基线**: `81a63d8feb7d713b1731f0c740d95574eb64dafa`
**阅读深度**: `implementation`
**生成时间**: 2026-06-08

---

## 1. Problem — 工具来源统一

ADK Go 的 agent 在运行时需要与多种"工具"交互，但这些工具的来源截然不同：

| 来源 | 示例 | 协议/形态 |
|------|------|-----------|
| **Go 函数** (`tool/functiontool`) | 业务逻辑函数，由开发者直接编写 | Go 泛型函数 `Func[TArgs, TResults]` |
| **Streaming Go 函数** (`tool/functiontool`) | 流式返回结果的函数 | Go 泛型函数 `StreamingFunc[TArgs]`，返回 `iter.Seq2[string, error]` |
| **MCP 服务器** (`tool/mcptoolset`) | 外部进程通过 Model Context Protocol 暴露工具 | JSON-RPC over stdio/SSE |
| **Gemini 原生工具** (`tool/geminitool`) | GoogleSearch、Retrieval 等 | Gemini API `genai.Tool` 结构体 |
| **子 Agent** (`tool/agenttool`) | 将一个 Agent 作为另一个 Agent 的工具调用 | 内部 runner + session |
| **Skill** (`tool/skilltoolset`) | 基于文件系统的可复用技能包 | SKILL.md + 资源文件 |
| **内置基础设施工具** | `load_artifacts`, `load_memory`, `preload_memory`, `exit_loop` | 固定的 FunctionDeclaration |

这些来源的差异巨大：
- **Schema 来源不同**：Go 函数通过 `jsonschema-go` 从泛型类型推断；MCP 工具从服务端 `ListTools` 响应获取；Gemini 工具直接使用 `genai.Tool` 结构体。
- **调用方式不同**：本地函数调用 vs RPC 调用 vs Gemini API 自身闭环。
- **生命周期不同**：MCP 需要连接管理 (connect/reconnect)；Skill 需要文件系统访问；本地函数直接调用。
- **确认逻辑不同**：每类工具都需要独立支持 Human-in-the-Loop (HITL) 确认。

**核心问题**：如何用一套统一的 `Tool` / `Toolset` 抽象，让 LLM Agent 在请求-响应循环中无差别地使用所有上述工具，同时让每个工具来源都能以最小代价接入？

---

## 2. Why Hard — 复杂性来源

### 2.1 Schema 生成与推断

- **Go 泛型 → JSON Schema**：`functiontool` 使用 `github.com/google/jsonschema-go` 从泛型类型 `TArgs` / `TResults` 自动推断 schema（`tool/functiontool/function.go:267-277`）。边界情况包括：基本类型返回值需包装为 `{"result": value}`（`function.go:245`）、`map` 输入类型支持、空 struct 输入（如 `exitlooptool` 的 `struct{}`）。
- **Schema 覆写**：开发者可通过 `Config.InputSchema` / `Config.OutputSchema` 提供自定义 schema，与推断 schema 的合并逻辑尚未完成（`function.go:268` 注释 "TODO: check if override schema is compatible with T"）。
- **MCP 的 nil schema 陷阱**：`mcptoolset/tool.go:45-56` 注释详细说明了 `*jsonschema.Schema` 作为 `interface{}` 赋值时的 typed nil 问题 —— 需要显式的 `if t.InputSchema != nil` 检查以避免向 LLM 发送 `"responseJsonSchema": null`。

### 2.2 Args/Result 编码

- **LLM → Go 类型转换**：模型返回的 function call args 是 `map[string]any`，需要通过 `typeutil.ConvertToWithJSONSchema` 转换为强类型 `TArgs`（`function.go:197`）。
- **Go 类型 → LLM 返回**：执行结果 `TResults` 需通过 `typeutil.ConvertToWithJSONSchema[TResults, map[string]any]` 转回 `map[string]any`（`function.go:231`）。
- **基本类型包装**：当输出 schema 推断失败且结果不是 map 时，自动包装为 `{"result": value}`（`function.go:240-246`）。
- **MCP 响应编码**：MCP 工具将 `mcp.CallToolResult` 的多态 Content （`TextContent`, `ImageContent` 等）转换为统一的 `map[string]any{"output": ...}`（`mcptoolset/tool.go:149-174`）。

### 2.3 Streaming Function

- **接口差异**：`StreamingFunctionTool`（`internal/toolinternal/tool.go:34-38`）返回 `iter.Seq2[string, error]` 而非普通 `(map[string]any, error)`。
- **Live Session 分派**：在 live (Bidi Streaming) 模式下，`handleFunctionCalls` 将 streaming tool 注册到 `liveSessionImpl` 中，异步执行并实时推送 chunk（`base_flow.go:1066-1107`）。
- **非 Live 模式**：在非 live 模式下，阻塞式收集所有 chunk 并合并为 `map[string]any{"result": concatenated}`（`base_flow.go:1109-1120`）。
- **取消机制**：支持 `stop_streaming` 伪函数调用，通过 `liveSessionImpl.CancelAllStreamingTools()` 中断流式工具（`base_flow.go:1048-1055`）。

### 2.4 Long-Running Tool

- **声明级提示**：`IsLongRunning=true` 时，`Declaration()` 会在 description 中追加 "NOTE: This is a long-running operation..." 提示 LLM 不要重复调用（`function.go:172-179`）。
- **LongRunningToolIDs 追踪**：function call event 的 `LongRunningToolIDs` 字段记录长运行工具 ID，用于后续响应匹配。
- **初始返回**：长运行工具首次返回中间状态（如 `{"status": "pending"}`），后续通过 function response 继续传递结果。测试验证了 function 只被调用一次（`long_running_function_test.go:82-88`），但可接收多次 function response（`long_running_function_test.go:187-217`）。

### 2.5 MCP 协议适配

- **连接生命周期**：`connectionRefresher`（`mcptoolset/client.go:39-45`）管理 MCP session 的懒加载、Ping 检测、自动重连。`refreshableErrors` 列表包括 `ErrConnectionClosed`, `ErrSessionMissing`, `io.ErrClosedPipe`, `io.EOF`。
- **分页**：`ListTools` 支持 cursor-based 分页，并处理重连后 cursor 失效的情况（重新从第一页开始，`client.go:92-98`）。
- **错误透传**：MCP 工具执行错误通过 `res.IsError` 和 `TextContent` 提取，构造 `errors.New(errMsg)` 返回（`mcptoolset/tool.go:129-147`）。
- **重试机制**：`withRetry` 泛型函数实现 "执行 → 失败 → refreshConnection → 重试一次" 的模式（`client.go:114-135`）。

### 2.6 HITL Confirmation（人机协同确认）

这是整个工具系统中最复杂的横切关注点，涉及三层确认路径：

1. **静态 `RequireConfirmation`**：Config 级别的布尔标志，所有调用都需要确认。
2. **动态 `RequireConfirmationProvider`**：Go 函数 `func(TArgs) bool`（functiontool）或 `func(toolName string, toolInput any) bool`（MCP/tool 级别），根据运行时参数决定是否需要确认。
3. **`tool.WithConfirmation` 包装**：`tool/tool.go:143-149` 提供 Toolset 级别的 `confirmationToolset`，将任何包含 `runnableTool` 接口的工具自动包装上确认逻辑。

**确认流程**（详见第 5 节链路三）：
- Tool Run 时先检查 `ctx.ToolConfirmation()`，若已确认则直接执行。
- 若需确认，调用 `ctx.RequestConfirmation(hint, payload)`，将 `RequestedToolConfirmations` 写入 `EventActions`。
- LLM flow 中 `generateRequestConfirmationEvent`（`functions.go:32-93`）生成 `adk_request_confirmation` function call event。
- `RequestConfirmationRequestProcessor`（`request_confirmation_processor.go:37-172`）在下一次请求中反向搜索历史 events，匹配用户确认/拒绝的 function response，并调用 `handleFunctionCalls` 重新执行原工具。
- 确认 payload 反序列化支持两种格式：`{"response": "{\"confirmed\": true}"}` （web client）和直接的 `{"confirmed": true}`（`request_confirmation_processor.go:72-97`）。

### 2.7 Toolset Filtering

- **`tool.FilterToolset`**：通过 `Predicate func(ctx agent.ReadonlyContext, tool Tool) bool` 在 Toolset 级别过滤（`tool/tool.go:89-101`）。
- **`AllowedToolsPredicate`**：基于白名单的工具名匹配（`tool/tool.go:76-85`）。
- **MCP ToolFilter**：MCP Toolset 也有内置的 `ToolFilter` 字段（deprecated，建议使用外层 `FilterToolset`）。

---

## 3. Design Approach — 分层架构

```
┌─────────────────────────────────────────────────┐
│              tool.Tool (公共接口)                  │
│  Name() / Description() / IsLongRunning()       │
├─────────────────────────────────────────────────┤
│         internal/toolinternal (内部接口)           │
│  FunctionTool        / StreamingFunctionTool     │
│  Declaration() + Run / Declaration() + RunStream │
│  RequestProcessor: ProcessRequest(req)           │
├─────────────────────────────────────────────────┤
│  functiontool  │ mcptoolset │ agenttool │ ...   │  ← 具体实现
│  泛型 Go 函数   │ MCP 协议    │ 子 Agent   │       │
├─────────────────────────────────────────────────┤
│              tool.Toolset (集合接口)               │
│  Tools(ctx ReadonlyContext) ([]Tool, error)      │
│  → FilterToolset / WithConfirmation (装饰器)      │
├─────────────────────────────────────────────────┤
│       internal/llminternal (LLM 引擎层)           │
│  Flow.RequestProcessors:                         │
│    toolProcessor → 收集 Tools/Toolsets           │
│    RequestConfirmationRequestProcessor → HITL     │
│  Flow.handleFunctionCalls → 统一调度执行          │
└─────────────────────────────────────────────────┘
```

**关键设计决策**：

1. **`Tool` 是最小公共接口**：只需 Name/Description/IsLongRunning。具体能力通过内部接口 `FunctionTool`（非流式运行）、`StreamingFunctionTool`（流式运行）、`RequestProcessor`（向 LLM 请求注入声明）表达。

2. **`Toolset` 提供动态工具集合**：`Tools()` 方法接受 `ReadonlyContext`，允许根据当前 invocation 状态动态决定提供哪些工具。MCP、Skill 均通过 `Toolset` 接入。

3. **`ProcessRequest` 是声明注入点**：每个工具/工具集通过 `ProcessRequest` 向 `LLMRequest` 中注入 function declaration（`functiontool`/`mcptoolset`/`agenttool` 调用 `toolutils.PackTool`）或 system instruction（`skilltoolset`/`loadmemorytool`/`loadartifactstool`）。

4. **`handleFunctionCalls` 是统一执行入口**：LLM flow 在收到 function call 后，通过 `handleFunctionCalls` 查找工具、执行、合并结果。区分了 `FunctionTool`、`StreamingFunctionTool`、`stop_streaming` 伪工具。

5. **Confirmation 是横切层**：确认逻辑在 `functionTool.Run`、`mcpTool.Run`、`confirmationTool.Run` 中分别重复实现（见第 7 节风险），通过 `ToolContext.ToolConfirmation()` 和 `ToolContext.RequestConfirmation()` 与 LLM flow 交互。

---

## 4. Code Walkthrough — 逐层走读

### 4.1 接口层：`tool/tool.go`

```go
// Tool: 最小公共接口
type Tool interface {
    Name() string
    Description() string
    IsLongRunning() bool
}

// Toolset: 动态工具集合
type Toolset interface {
    Name() string
    Tools(ctx agent.ReadonlyContext) ([]Tool, error)
}
```

预留了两类 sentinel error：
- `ErrConfirmationRequired`：工具需要确认
- `ErrConfirmationRejected`：确认被用户拒绝

`FilterToolset` 和 `WithConfirmation` 是两个装饰器模式的 Toolset 包装器。`Predicate` 定义了工具过滤函数。

### 4.2 内部接口层：`internal/toolinternal/tool.go`

```go
// FunctionTool: 非流式可运行工具
type FunctionTool interface {
    Tool
    Declaration() *genai.FunctionDeclaration
    Run(ctx agent.ToolContext, args any) (map[string]any, error)
}

// StreamingFunctionTool: 流式可运行工具
type StreamingFunctionTool interface {
    Tool
    Declaration() *genai.FunctionDeclaration
    RunStream(ctx agent.ToolContext, args any) iter.Seq2[string, error]
}

// RequestProcessor: 工具/工具集的 LLM 请求处理能力
type RequestProcessor interface {
    ProcessRequest(ctx agent.ToolContext, req *model.LLMRequest) error
}
```

这三个接口定义了工具系统与 LLM 引擎的两个连接点：
- **声明注入**：`RequestProcessor.ProcessRequest` 在 LLM 请求构建阶段调用
- **执行调用**：`FunctionTool.Run` / `StreamingFunctionTool.RunStream` 在 LLM 返回 function call 后调用

### 4.3 声明打包：`internal/toolinternal/toolutils/toolutils.go`

`PackTool(req, tool)` 是将 `FunctionDeclaration` 注入 `LLMRequest` 的核心工具函数：

```go
func PackTool(req *model.LLMRequest, tool Tool) error {
    // 1. 注册到 req.Tools map（防重名检查）
    // 2. 确保 req.Config 存在
    // 3. 找到或创建包含 FunctionDeclarations 的 genai.Tool
    // 4. 追加 function declaration
}
```

设计要点：多个 function tool 的 declaration 被合并到同一个 `genai.Tool` 的 `FunctionDeclarations` 列表中，而非各自创建独立的 `genai.Tool`。这在 `function_test.go:197-258` 中有专门的测试验证。

### 4.4 泛型 Function Tool：`tool/functiontool/function.go`

**创建流程** (`New[TArgs, TResults]`):
1. 通过 `reflect` 验证 `TArgs` 是 struct 或 map 类型
2. 通过 `resolvedSchema` 获取 input/output schema（自定义 > 推断）
3. 类型断言 `RequireConfirmationProvider` 为 `func(TArgs) bool`
4. 返回 `*functionTool[TArgs, TResults]`

**Declaration 生成** (`Declaration()`):
- 基础 name/description
- 设置 `ParametersJsonSchema` / `ResponseJsonSchema`
- 若 `IsLongRunning=true`，追加 NOTE 提示

**Run 执行** (`Run(ctx, args)`):
1. `defer recover()` 捕获 panic
2. `args.(map[string]any)` → `typeutil.ConvertToWithJSONSchema` → `TArgs`
3. 确认检查（三段式：已有确认 → 动态/静态判断 → RequestConfirmation）
4. 调用 `handler(ctx, input)` → `TResults`
5. `typeutil.ConvertToWithJSONSchema` → `map[string]any`
6. 若非 map 且无 output schema，包装为 `{"result": value}`

### 4.5 Streaming Function Tool：`tool/functiontool/streaming_function.go`

与 `functionTool` 高度对称，关键差异：
- Handler 类型：`func(agent.ToolContext, TArgs) iter.Seq2[string, error]`
- `RunStream` 返回 `iter.Seq2[string, error]` 而非 `(map[string]any, error)`
- 无 `OutputSchema`（流式工具只产生文本 chunk）
- 确认逻辑和在 `RunStream` 内部通过 `yield` 报告错误

### 4.6 MCP Toolset：`tool/mcptoolset/`

**Set 层** (`set.go`):
- `set.Tools(ctx)`：调用 `mcpClient.ListTools(ctx)` → `convertTool()` → 过滤 → 返回 `[]tool.Tool`
- 支持 `RequireConfirmation` / `RequireConfirmationProvider`

**Client 层** (`client.go`):
- `connectionRefresher` 实现 `MCPClient` 接口
- 懒加载 session（`getSession`）
- 自动重连（`refreshConnection`）：
  - 先 Ping 确认连接是否真的断开（防止竞态）
  - 关闭旧 session → 重新 `client.Connect`
- `withRetry` 泛型函数：执行 → 失败 → 重连 → 再执行一次

**Tool 层** (`tool.go`):
- `mcpTool` 实现 `FunctionTool` + `RequestProcessor`
- `Run` 中的确认逻辑与 `functionTool` 几乎重复（见第 7 节风险）
- 调用 `mcpClient.CallTool(ctx, params)` → 将 `mcp.CallToolResult` 转换为 `map[string]any{"output": ...}`

### 4.7 Gemini 原生工具：`tool/geminitool/`

独特的工具类型，**不实现 `FunctionTool`**（无 `Run` 方法）：
- `GoogleSearch`（`google_search.go`）：`ProcessRequest` 将 `genai.Tool{GoogleSearch: &genai.GoogleSearch{}}` 直接追加到 `req.Config.Tools`
- `New(name, description, genaiTool)`（`tool.go`）：通用的 Gemini 工具包装器

这些工具的执行完全由 Gemini API 自身闭环 —— ADK 不介入中间过程。

### 4.8 Agent Tool：`tool/agenttool/agent_tool.go`

将一个 Agent 包装为另一个 Agent 的可调用工具：
- `Declaration()` 从子 Agent 的 `InputSchema` 生成 parameters，若不存在则提供默认 `{"request": "STRING"}`
- `Run` 创建独立的 `runner.Runner` + `InMemoryService`，运行子 Agent 并返回最终文本
- 支持 `SkipSummarization` 配置

### 4.9 Skill Toolset：`tool/skilltoolset/`

**工具层** (`tool/skilltoolset/toolset.go`):
- `SkillToolset` 实现 `Toolset` + `RequestProcessor`
- `ProcessRequest` 将 Skill 列表以 XML 格式注入 system instruction
- 提供三个标准工具：`list_skills`, `load_skill`, `load_skill_resource`

**数据源** (`tool/skilltoolset/skill/`):
- `Source` 接口定义标准的数据访问层（`ListFrontmatters`, `LoadFrontmatter`, `LoadInstructions`, `LoadResource`, `ListResources`）
- `FilesystemSource`：基于本地文件系统
- `Frontmatter`：SKILL.md 的 YAML 元数据解析和验证
- `MergedSource`：多个 Source 的合并视图
- `Parse` / `ParseBytes` / `Build`：SKILL.md 的读写工具

**内部工具实现** (`internal/skilltool/`):
- 三个工具均通过 `functiontool.New()` 创建，复用泛型工具框架
- `load_skill_resource` 有 10 MiB 大小限制（`maxResourceSize`）

### 4.10 内置基础设施工具

| 工具 | 文件 | 类型 | `FunctionTool` | `RequestProcessor` |
|------|------|------|:---:|:---:|
| `load_artifacts` | `loadartifactstool/load_artifacts_tool.go` | 内置 | ✅ | ✅ |
| `load_memory` | `loadmemorytool/tool.go` | 内置 | ✅ | ✅ |
| `preload_memory` | `preloadmemorytool/tool.go` | 预处理器 | ❌ | ✅ |
| `exit_loop` | `exitlooptool/tool.go` | 基于 functiontool | ✅ | ✅ |

**`preload_memory` 的特殊性**: 不暴露给 LLM 调用（无 `Declaration()`），只在 `ProcessRequest` 中自动搜索 memory 并注入 system instruction。这是一种透明的工具模式。

### 4.11 Confirmation 系统：`tool/toolconfirmation/tool_confirmation.go`

```go
const FunctionCallName = "adk_request_confirmation"

type ToolConfirmation struct {
    Hint      string `json:"hint"`
    Confirmed bool   `json:"confirmed"`
    Payload   any    `json:"payload"`
}
```

`OriginalCallFrom(functionCall)` 从确认 wrapper 中提取原始 function call，支持两种输入格式：
1. `*genai.FunctionCall` 直接传入
2. `map[string]any` 通过 `converters.FromMapStructure` 反序列化

### 4.12 LLM 层连接点：`internal/llminternal/`

#### `tools_processor.go` — 工具收集
`toolProcessor` 是 `Flow.RequestProcessors` 的第 2 个处理器，负责：
1. 从 `llmAgent` 获取 `Tools` 列表
2. 遍历 `Toolsets`，调用 `toolSet.Tools(readonlyCtx)` 收集动态工具
3. 合并到 `f.Tools`，供后续 `handleFunctionCalls` 使用

```go
func toolProcessor(ctx agent.InvocationContext, req *model.LLMRequest, f *Flow) iter.Seq2[*session.Event, error] {
    if f.Tools != nil { return }  // 已收集，跳过
    tools := Reveal(llmAgent).Tools
    for _, toolSet := range Reveal(llmAgent).Toolsets {
        tsTools, err := toolSet.Tools(readonlyCtx)
        tools = append(tools, tsTools...)
    }
    f.Tools = tools
}
```

#### `base_flow.go` — 工具调度执行

**`runOneStep`**（`base_flow.go:528-653`）:
1. `preprocess` → 遍历所有 `RequestProcessors`（包括 `toolProcessor`、`RequestConfirmationRequestProcessor` 等）
2. `callLLM` → 向模型发送请求
3. `postprocess` → 执行 ResponseProcessors
4. `handleFunctionCalls(ctx, tools, resp.LLMResponse, nil, nil)` → 执行工具
5. `generateRequestConfirmationEvent` → 若工具请求确认，生成确认 event
6. yield function response event → yield confirmation event

**`handleFunctionCalls`**（`base_flow.go:1012-1180`）:
1. 提取 `FunctionCalls`
2. 并发 goroutine 执行每个 function call（`sync.WaitGroup`）
3. 每种工具类型的分派：
   - `stop_streaming` → 取消流式工具
   - `StreamingFunctionTool` + live session → 注册异步执行
   - `StreamingFunctionTool` 无 live → 阻塞收集 chunk
   - `FunctionTool` → `callTool(toolCtx, funcTool, fArgs)`
   - 未知 → `runOnToolErrorCallbacks`
4. `callTool` 执行链：`BeforeToolCallbacks` → `tool.Run` → `OnToolErrorCallbacks` → `AfterToolCallbacks`
5. 合并多个 function response event 为一个（`mergeParallelFunctionResponseEvents`）

#### `functions.go` — 确认事件生成
`generateRequestConfirmationEvent` 从 function response event 的 `Actions.RequestedToolConfirmations` 中提取需要确认的工具，为每个生成 `adk_request_confirmation` function call event。

#### `request_confirmation_processor.go` — 确认响应处理
`RequestConfirmationRequestProcessor` 在每次新的 LLM 请求前执行：
1. 反向搜索 session events，找到用户的确认/拒绝 function response
2. 匹配对应的 `adk_request_confirmation` function call
3. 通过 `toolconfirmation.OriginalCallFrom` 提取原始工具调用
4. 调用 `handleFunctionCalls` 重新执行原始工具（携带 `toolConfirmations` 参数）
5. 删除已处理的确认，避免重复执行

---

## 5. Tool Lifecycle — 三条关键链路

### 链路一：Go 函数 → Schema → Model Function Call → Run → Response

```
开发者调用 functiontool.New[TArgs, TResults](cfg, handler)
  │
  ├─ 1. schema 推断: jsonschema.For[TArgs] → *jsonschema.Resolved
  ├─ 2. 创建 functionTool[TArgs, TResults]
  │
  ▼
LLM Agent 启动 Flow.runOneStep(ctx)
  │
  ├─ 3. preprocess → toolProcessor → 收集 Tools + Toolsets
  │
  ├─ 4. 遍历 Tools, 调用 ProcessRequest
  │     functionTool.ProcessRequest → toolutils.PackTool
  │     → req.Config.Tools[0].FunctionDeclarations = [...]
  │
  ├─ 5. callLLM(ctx, req) → 模型返回 FunctionCall
  │     {name: "sum", args: {a: 1, b: 2}}
  │
  ├─ 6. handleFunctionCalls(fnCalls)
  │     ├─ toolsDict["sum"] = functionTool
  │     ├─ funcTool, ok := curTool.(FunctionTool) → true
  │     │
  │     └─ callTool(toolCtx, funcTool, fArgs)
  │         ├─ BeforeToolCallbacks(toolCtx, tool, fArgs)
  │         ├─ tool.Run(toolCtx, fArgs)
  │         │   ├─ typeutil.ConvertTo(mapArgs → TArgs)
  │         │   ├─ 确认检查 (RequireConfirmation/Provider)
  │         │   ├─ handler(ctx, TArgs) → (TResults, error)
  │         │   └─ typeutil.ConvertTo(TResults → map[string]any)
  │         ├─ AfterToolCallbacks(toolCtx, tool, fArgs, result, err)
  │         └─ 返回 map[string]any{"sum": 3}
  │
  └─ 7. 构造 FunctionResponse event → yield 给调用方
       LLM 下一轮可以基于结果继续推理
```

**failure path**:
- Schema 推断/转换失败 → `nil, error` → event 中 `{"error": err.Error()}`
- Handler panic → `defer recover()` → `fmt.Errorf("panic in tool %q: %v\nstack: %s", ...)`
- 工具名未找到 → `runOnToolErrorCallbacks` → `{"error": "Tool <name> not found..."}`
- 重复工具名 → `toolutils.PackTool` 返回 error（`base_flow.go:42-44`）

### 链路二：MCP List / Connect / Call

```
mcptoolset.New(Config{Transport: clientTransport})
  │
  ▼
LLM Agent Flow.preprocess → toolProcessor
  │
  ├─ 调用 mcpToolset.Tools(readonlyCtx)
  │   │
  │   ├─ 1. connectionRefresher.ListTools(ctx)
  │   │   ├─ getSession(ctx)
  │   │   │   ├─ session == nil → client.Connect(ctx, transport)
  │   │   │   └─ session != nil → 返回已有 session
  │   │   ├─ session.ListTools(ctx, params)
  │   │   │   └─ 支持 cursor 分页循环
  │   │   └─ 失败 → shouldRefreshConnection?
  │   │       ├─ Yes → refreshConnection → 重试 ListTools
  │   │       └─ No → 返回 error
  │   │
  │   ├─ 2. convertTool(mcpTool, client)
  │   │   └─ mcp.Tool → mcpTool (实现 FunctionTool + RequestProcessor)
  │   │
  │   └─ 3. ToolFilter 过滤（若配置）
  │
  ▼
Model 返回 FunctionCall → handleFunctionCalls
  │
  ├─ mcpTool.Run(toolCtx, args)
  │   ├─ 确认检查
  │   ├─ mcpClient.CallTool(ctx, &mcp.CallToolParams{Name, Arguments})
  │   │   └─ connectionRefresher.CallTool
  │   │       └─ withRetry: session.CallTool → 失败 → 重连 → 重试
  │   ├─ 处理 CallToolResult:
  │   │   ├─ StructuredContent != nil → {"output": structured}
  │   │   └─ Content (TextContent) → {"output": text}
  │   └─ IsError → error 消息
  │
  └─ 构造 FunctionResponse event
```

**failure path**:
- ListTools 失败 → `toolProcessor` 返回 error
- 某 MCP 工具转换失败 → 跳过或返回 error
- CallTool 失败 → 重连 + 重试一次 → 仍失败则 error
- MCP `IsError=true` → 从 `TextContent` 提取错误详情
- 无 text content 且无 structured content → `errors.New("no text content in tool response")`

### 链路三：Confirmation Request → User Confirm/Reject → Tool Execution

```
工具 Run(ctx, args)
  │
  ├─ 检查 ctx.ToolConfirmation()
  │   ├─ confirmation != nil
  │   │   ├─ confirmation.Confirmed == true → 跳过确认,直接执行 handler
  │   │   └─ confirmation.Confirmed == false → ErrConfirmationRejected
  │   │
  │   └─ confirmation == nil
  │       ├─ 判断 requireConfirmation (静态 || 动态 Provider)
  │       ├─ false → 直接执行 handler
  │       └─ true →
  │           ├─ ctx.RequestConfirmation(hint, nil)
  │           │   → ctx.Actions().RequestedToolConfirmations[fnCallID] = ToolConfirmation
  │           ├─ ctx.Actions().SkipSummarization = true
  │           └─ 返回 ErrConfirmationRequired
  │
  ▼
LLM Flow 检测到 error 返回
  │
  ├─ handleFunctionCalls → function response event 包含 {"error": "requires confirmation..."}
  │   和 event.Actions.RequestedToolConfirmations
  │
  ├─ generateRequestConfirmationEvent(ctx, fnCallEvent, fnRespEvent)
  │   ├─ 遍历 RequestedToolConfirmations
  │   ├─ 为每个待确认工具生成 adk_request_confirmation FunctionCall
  │   │   args: {originalFunctionCall, toolConfirmation}
  │   └─ 返回包含 FunctionCall 的 Event
  │
  ├─ yield functionResponseEvent (先于确认事件)
  ├─ yield toolConfirmationEvent
  │
  ▼
用户返回 FunctionResponse:
  {
    name: "adk_request_confirmation",
    response: {"confirmed": true/false}
  }
  │
  ▼
下一轮 Flow.preprocess → RequestConfirmationRequestProcessor
  │
  ├─ 1. 反向扫描 session events
  │   ├─ 找到最新的 user-authored event
  │   ├─ 提取 FunctionResponse(name == "adk_request_confirmation")
  │   └─ 解析 ToolConfirmation (支持两种 JSON 格式)
  │
  ├─ 2. 向前搜索对应的 adk_request_confirmation FunctionCall
  │   ├─ 提取 originalFunctionCall
  │   └─ 去重 (删除已处理的)
  │
  ├─ 3. handleFunctionCalls(ctx, toolsmap, {Content: originalCall},
  │      toolConfirmations: {fnCallID: ToolConfirmation})
  │   └─ 每个 goroutine 中 toolCtx 携带 confirmation
  │       └─ Tool.Run → ctx.ToolConfirmation() != nil
  │           ├─ Confirmed=true → 执行 handler
  │           └─ Confirmed=false → ErrConfirmationRejected
  │
  └─ 4. yield function response event
```

**关键时序约束**: function response event（含 error）必须在 confirmation event **之前** yield，确保消费端先看到工具的执行状态再看到确认请求（`base_flow.go:611-621`）。

---

## 6. Tests — 测试覆盖矩阵

| 测试文件 | 覆盖内容 | 覆盖层级 |
|----------|---------|---------|
| `tool/functiontool/function_test.go` | 基本 function tool 创建与执行、自定义 schema、基本类型返回、map 输入、多 tool declaration 合并、panic 恢复、`RequireConfirmationProvider` 类型验证、无效输入类型 | Unit + Integration (含 Gemini) |
| `tool/functiontool/long_running_function_test.go` | 长运行工具创建、声明注入、LongRunningToolIDs 追踪、多轮 function response 交互 | Unit + Integration |
| `tool/mcptoolset/set_test.go` | MCP 端到端（in-memory transport + Gemini）、ToolFilter、`ListTools` 重连、`CallTool` 重连、确认流程（静态/动态 Provider、confirm/reject） | Integration (含 Gemini) |
| `tool/skilltoolset/toolset_test.go` | `ProcessRequest` system instruction 注入、无 skill 边界、缺失 source 错误、Tools 列表验证 | Unit |
| `tool/skilltoolset/internal/skilltool/tools_test.go` | `list_skills`/`load_skill`/`load_skill_resource` 与 mockSource 的完整交互 | Unit |
| `tool/geminitool/tool_test.go` | `ProcessRequest` 的 genai.Tool 注入、追加到已有 Tools、nil request 错误 | Unit |
| `tool/loadartifactstool/load_artifacts_tool_test.go` | artifacts tool 基本功能 | Unit |
| `tool/loadmemorytool/tool_test.go` | memory search 基本功能 | Unit |
| `tool/preloadmemorytool/tool_test.go` | preload memory 基本功能 | Unit |
| `tool/exitlooptool/tool_test.go` | exit loop 基本功能 | Unit |
| `internal/llminternal/streaming_tool_test.go` | `handleFunctionCalls` 的流式/非流式工具分派 | Unit |

**确认流程测试特别覆盖**:
- functiontool 确认（`function_test.go:560-867`）：10 个场景（无确认、需确认、确认通过、确认拒绝、条件确认不需、条件确认需要、条件确认通过、条件确认拒绝等）
- MCP 确认（`set_test.go:366-742`）：10 个场景，含 `RequireConfirmation` + `RequireConfirmationProvider`
- functiontool 确认时序（`function_test.go:869-917`）：验证 function response 先于 confirmation event

**缺失覆盖**:
- `confirmationToolset`（`tool.WithConfirmation` 包装）的独立测试
- `agenttool` 的端到端测试
- streaming confirmation 场景（streamingFunctionTool + HITL）
- MCP 重连的并发安全测试

---

## 7. Risks — 风险与边界

### 7.1 重复确认逻辑（代码重复）

确认逻辑在三处**几乎完全重复**（仅泛型参数不同）：

| 位置 | 行数 |
|------|------|
| `functionTool.Run` (`function.go:202-225`) | 24 行 |
| `streamingFunctionTool.RunStream` (`streaming_function.go:149-173`) | 25 行 |
| `mcpTool.Run` (`mcptoolset/tool.go:95-118`) | 24 行 |
| `confirmationTool.Run` (`tool/tool.go:203-230`) | 27 行 |

每处都包含相同的三段式确认检查、相同的 hint 消息模板、相同的 `SkipSummarization = true`。任何确认逻辑的变更需要在四处同步修改。

### 7.2 Schema 推断边界

- `resolvedSchema` 中的 `TODO: check if override schema is compatible with T`（`function.go:268`）意味着自定义 schema 与泛型类型不一致时不会在创建时报错，而是在运行时反序列化失败。
- 基本类型返回值（非 struct/map）通过 `{"result": value}` 包装，但带 output schema 时走不同的分支（`function.go:240-246`），两种路径的行为不完全一致。
- `map[string]int` 类型输入的 schema 推断与 `struct` 类型可能存在差异。

### 7.3 MCP 生命周期

- 懒加载 session 意味着首次 `ListTools` 时才会 connect，连接失败的反馈时机较晚。
- 分页 + 重连的交互：重连后 cursor 失效，代码通过 `hasReconnected` 标志防止无限循环，但重复重连（同一次调用中连接再次断开）会返回错误（`client.go:92`）。
- `refreshConnection` 中的 Ping 检查和 session 关闭在持锁状态下进行，可能造成阻塞。

### 7.4 Streaming / Long-Running 不一致

- Long-running 使用 `IsLongRunning() bool` + description 注入提示词控制（让 LLM 不要重复调用），而 streaming 通过 `StreamingFunctionTool` 接口完全由框架控制生命周期。
- Long-running tool 的 `handleFunctionCalls` 中仍有 `// TODO: handle long-running tool.` 注释（`base_flow.go:1132`），表明实现不完整。
- Streaming tool 的非 live 模式将所有 chunk 合并为单个 `{"result": concat}`，丢失了流式的增量特性（这对于非 live 模式可能是不得已的）。

### 7.5 Confirmation Provider 类型不一致

- `functiontool` 的 Provider 签名：`func(TArgs) bool`（类型安全，编译时检查）
- `tool` 包级别（MCP 等）的 Provider 签名：`func(toolName string, toolInput any) bool`（运行时类型检查）
- 这两者的设计不一致，且 `functiontool` 在创建时会做运行时类型断言（`function.go:105-108`），但编译时无法捕获不匹配。

### 7.6 Toolset 动态性与缓存

`toolProcessor` 中的 `if f.Tools != nil { return }`（`tools_processor.go:31-33`）意味着 Tools 仅在首次请求时收集。如果 Toolset 的 `Tools()` 结果依赖于 invocation context 的变化（如 session state），则可能无法反映后续变化。

### 7.7 Gemini 工具的"黑洞"

`geminitool` 实现 `RequestProcessor` 但不实现 `FunctionTool`。这意味着在 `handleFunctionCalls` 中，如果 Gemini API 返回了 GoogleSearch 的 function call，框架会因找不到工具而调用 `runOnToolErrorCallbacks`。实际上 Gemini 在服务端自行处理这些工具，不会返回 function call，但这一依赖并未在代码中显式约束。

### 7.8 Agent Tool 的隔离性

`agenttool.Run` 为每次调用创建全新的 `runner.Runner` + `session.InMemoryService`。这意味着：
- 子 agent 无法访问父 agent 的 session 历史（只复制了非内部 state）
- 每次调用创建新的 runner 可能有性能开销
- 子 agent 的 artifacts/memory 通过独立的 service 实例管理，可能造成数据孤岛

---

## 8. Next Questions — 下一轮追问

1. `confirmationToolset`（`tool.WithConfirmation`）的确认逻辑与 functiontool/mcptoolset 内部确认逻辑是否会**双重触发**？如果一个 functiontool 自身 `RequireConfirmation=true`，又被 `WithConfirmation` 包装，确认会发生两次吗？

2. Long-running tool 的 `// TODO: handle long-running tool.`（`base_flow.go:1132`）计划如何实现？是否采用类似 `StreamingFunctionTool` 的 goroutine + 异步注册模式？

3. `toolProcessor` 中的 `f.Tools` 缓存是否需要失效机制？当 Toolset 通过 `ReadonlyContext.State()` 动态决定工具列表时，state 变化是否需要刷新 `f.Tools`？

4. Streaming tool 在 live 模式下的 `cancelCtx` 生命周期管理：在多轮会话中，cancel context 是否可能因为 goroutine 未及时退出而泄漏？

5. MCP `connectionRefresher` 中的 `sync.Mutex` 粒度是否合理？`refreshConnection` 在持锁时关闭旧 session（可能阻塞），是否应使用 `sync.RWMutex`？

6. `RequireConfirmationProvider` 的两种不同签名（`func(TArgs) bool` vs `func(string, any) bool`）是否需要统一？当前的不一致会增加维护成本和集成错误。

7. `agenttool` 不实现 `toolinternal.FunctionTool` 的 `Declaration()` 返回值何时为 nil？当子 agent 不是 `llminternal.Agent` 类型时会发生什么？

8. `preloadmemorytool` 的 `ProcessRequest` 在每次 LLM 请求时搜索 memory，但搜索关键词来自 `UserContent().Parts[0].Text`。如果多轮对话中最新 user content 只是确认性回复（"yes"），preload 的效果如何？

9. 当 `handleFunctionCalls` 并发执行多个工具，其中部分请求确认、部分失败时，`mergeParallelFunctionResponseEvents` 合并后的 `EventActions.RequestedToolConfirmations` 行为是否正确？

10. `skilltoolset` 的 `ProcessRequest` 将 skill 列表注入 system instruction，但无 `OutputSchema`。如果 future 需要支持结构化 skill 选择输出，如何兼容？

11. `set.go:66` 将 MCP ToolFilter 标记为 `Deprecated` 推荐使用 `tool.FilterToolset`，但两者的过滤时机不同（MCP 侧为 convert 后过滤，外层为任意 Toolset 过滤），性能影响如何？

12. Gemini 工具（GoogleSearch 等）和 MCP 工具是否可以同时在一个 agent 中工作？`req.Config.Tools` 的结构是否会因此产生冲突？
