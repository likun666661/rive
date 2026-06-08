# 第三部分：工具、函数调用、MCP、Skills 与确认机制

> 基于 `google/adk-go` 仓库只读代码分析，不修改仓库。

---

## 1. 面临的问题是什么

ADK Go 的工具系统需要将以下**异构工具来源**统一成一套接口与执行模型：

| 工具来源 | 本质 | 示例 |
|----------|------|------|
| **Go function** (`functiontool/`) | 本地 Go 函数包装为 tool | `functiontool.New(cfg, handler)` |
| **MCP** (`mcptoolset/`) | 远程 MCP 服务器上的 tool | `mcptoolset.New(cfg)` → 连接 → `mcp.Client.ListTools/CallTool` |
| **Skills** (`skilltoolset/`) | 文件系统中的技能指令 | `list_skills / load_skill / load_skill_resource` |
| **Gemini built-ins** (`geminitool/`) | Gemini 原生服务端工具 | `GoogleSearch{}`, `genai.Tool{Retrieval: ...}` |
| **Memory/Artifact** (`loadmemorytool/`, `preloadmemorytool/`, `loadartifactstool/`) | Session 上下文数据加载 | `load_memory`, `load_artifacts` |
| **Agent-as-tool** (`agenttool/`) | 子 Agent 作为 tool | `agenttool.New(subAgent, cfg)` |
| **Exit loop** (`exitlooptool/`) | 控制流 tool | `exit_loop` → `agent.Actions().Escalate = true` |
| **Few-shot example** (`exampletool/`) | 示例注入 tool | `exampletool.New(ExampleToolConfig{})` |

### 统一形态：`tool.Tool` + 三个扩展接口

```
tool.Tool (public interface)
    ├── Name() string
    ├── Description() string
    └── IsLongRunning() bool

toolinternal.FunctionTool (internal interface)
    ├── extends tool.Tool
    ├── Declaration() *genai.FunctionDeclaration
    └── Run(ctx ToolContext, args any) (map[string]any, error)

toolinternal.StreamingFunctionTool (internal interface)
    ├── extends tool.Tool
    ├── Declaration() *genai.FunctionDeclaration
    └── RunStream(ctx ToolContext, args any) iter.Seq2[string, error]

toolinternal.RequestProcessor (internal interface)
    └── ProcessRequest(ctx ToolContext, req *model.LLMRequest) error

tool.Toolset (public interface)
    ├── Name() string
    └── Tools(ctx ReadonlyContext) ([]Tool, error)
```

**核心思想**：`Tool` 是最小公约数（身份标识），两个内部接口 + 一个 RequestProcessor 分别覆盖"可被 LLM 调用执行"和"可修改 LLM 请求"两种能力。不同工具按需实现不同接口组合。例如：

- `functionTool` 实现 `Tool` + `FunctionTool` + `RequestProcessor`
- `geminiTool` / `GoogleSearch` 只实现 `Tool` + `RequestProcessor`（不本地执行）
- `preloadMemoryTool` / `exampleTool` 只实现 `Tool` + `RequestProcessor`（自动注入，非 LLM 调用）

参考：`tool/tool.go:37-46`, `internal/toolinternal/tool.go:28-42`

---

## 2. 为什么这是问题：核心复杂点

### 2.1 Schema Generation

`functiontool` 需要从泛型 Go 类型自动推断 JSON Schema：

```go
// functiontool/function.go:77-120
func New[TArgs, TResults any](cfg Config, handler Func[TArgs, TResults]) (tool.Tool, error) {
    // 1. 反射校验 TArgs 必须是 struct 或 map
    argsType := reflect.TypeOf(zeroArgs)
    // 2. 自动推断 InputSchema / OutputSchema
    ischema, _ := resolvedSchema[TArgs](cfg.InputSchema)    // functiontool/function.go:267-277
    oschema, _ := resolvedSchema[TResults](cfg.OutputSchema)
    // 3. 转为 genai.FunctionDeclaration
    decl := &genai.FunctionDeclaration{
        ParametersJsonSchema: ischema.Schema(),
        ResponseJsonSchema:   oschema.Schema(),
    }
}
```

复杂点：
- Go 泛型类型 → `jsonschema.Schema` → `genai.FunctionDeclaration` 的转换链依赖 `github.com/google/jsonschema-go`
- MCP 工具的 schema 直接来自 `mcp.Tool.InputSchema/OutputSchema`（`*jsonschema.Schema` 指针），需要小心处理 nil 接口问题（`mcptoolset/tool.go:50-56`）
- 用户可手动覆盖 schema（`Config.InputSchema` / `Config.OutputSchema`），需要验证兼容性
- `agenttool` 的 schema 来自子 Agent 的 `InputSchema`（`agenttool/agent_tool.go:86-116`），如果不存在回退为 `{"request": "STRING"}`

### 2.2 Tool Call Args/Result

入参和返回值经历了多次类型转换：

```
LLM 发送 JSON → map[string]any (原始 args)
    → typeutil.ConvertToWithJSONSchema[TArgs] → 强类型 TArgs
    → handler(ctx, TArgs) → TResults
    → typeutil.ConvertToWithJSONSchema[TResults, map[string]any] → map[string]any
    → genai.Part{FunctionResponse} → 送回 LLM
```

参考：`functiontool/function.go:185-247`

MCP 工具的参数量换更简单（不经过 Go 泛型），直接透传给 MCP server：
```go
// mcptoolset/tool.go:121-127
res, err := t.mcpClient.CallTool(ctx, &mcp.CallToolParams{
    Name: t.name, Arguments: args,
})
```

流式函数的返回值完全不同——用 Go 1.23 iterator：
```go
// functiontool/streaming_function.go:35
type StreamingFunc[TArgs any] func(agent.ToolContext, TArgs) iter.Seq2[string, error]
```

### 2.3 IsLongRunning

长期运行工具需要抑制 LLM 重复调用：
```go
// functiontool/function.go:172-179
if f.cfg.IsLongRunning {
    instruction := "NOTE: This is a long-running operation. Do not call this tool again..."
    decl.Description += "\n\n" + instruction
}
```
MCP 工具目前不支持 long-running（`mcptoolset/tool.go:82-84` 硬编码返回 `false`）。

### 2.4 Human-in-the-Loop Confirmation

确认逻辑在 **三个地方重复实现**，代码几乎完全相同：

| 位置 | 实现类 |
|------|---------|
| `tool/tool.go:203-229` | `confirmationTool` |
| `functiontool/function.go:202-225` | `functionTool.Run` |
| `functiontool/streaming_function.go:149-173` | `streamingFunctionTool.RunStream` |
| `mcptoolset/tool.go:94-118` | `mcpTool.Run` |

确认流程分为三个层级：
1. **Context 已有确认结果**：检查 `ctx.ToolConfirmation().Confirmed`，拒绝则返回 `ErrConfirmationRejected`
2. **静态配置**：检查 `requireConfirmation bool` 字段
3. **动态 Provider**：调用 `requireConfirmationProvider(toolInput)` （functiontool）或 `ConfirmationProvider(toolName, toolInput)` （mcptoolset）

如果确认需要但未发生，则：
```go
ctx.RequestConfirmation("Please approve or reject...", nil)
ctx.Actions().SkipSummarization = true
return nil, fmt.Errorf("%w", tool.ErrConfirmationRequired)
```

`toolconfirmation.ToolConfirmation` 结构（`toolconfirmation/tool_confirmation.go:50-64`）与 `adk_request_confirmation` function call 名称（`toolconfirmation/tool_confirmation.go:46`）一起形成了确认协议的完整定义。

---

## 3. 解决思路：分层接口 + 装饰器模式

### 3.1 三层设计

```
Layer 1: tool.Tool (身份)
    所有工具都实现：Name(), Description(), IsLongRunning()

Layer 2: 内部接口 (能力)
    toolinternal.FunctionTool          → Declaration() + Run()
    toolinternal.StreamingFunctionTool → Declaration() + RunStream()
    toolinternal.RequestProcessor      → ProcessRequest()

Layer 3: 装饰器 (横切关注点)
    FilterToolset      → 按 Predicate 过滤工具
    WithConfirmation   → 为 runnableTool 注入确认逻辑
```

### 3.2 工具集（Toolset）

`Toolset` 是工具的组织容器，提供动态工具发现：
```go
// tool/tool.go:57-64
type Toolset interface {
    Name() string
    Tools(ctx agent.ReadonlyContext) ([]Tool, error)
}
```

`mcptoolset.set.Tools()` 懒连接 MCP 服务器并动态列出工具（`mcptoolset/set.go:108-129`），而 `SkillToolset.Tools()` 返回预创建的三个固定工具（`skilltoolset/toolset.go:102`）。

装饰器链：
```go
FilterToolset(toolset, predicate)  → 过滤工具
WithConfirmation(toolset, bool, provider) → 注入 HITL
```

### 3.3 确认合约（Confirmation Contract）

完整流程：
```
1. LLM 调用 tool → agent 创建 ToolContext
2. Tool.Run() 检查 ctx.ToolConfirmation()
   a. 如果没有确认结果 + requireConfirmation=true
      → ctx.RequestConfirmation(hint, nil)
      → 返回 ErrConfirmationRequired
   b. Agent 检测到此错误 → 生成 adk_request_confirmation FunctionCall
   c. 前端收到 adk_request_confirmation → 展示给用户
   d. 用户确认 → 前端发送 adk_request_confirmation FunctionResponse (confirmed=true)
   e. Agent 重新调用 Tool.Run()，此时 ctx.ToolConfirmation().Confirmed = true
      → 继续执行
```

参考：`tool/tool.go:203-229`, `toolconfirmation/tool_confirmation.go:27-63`

### 3.4 FunctionTool Adapter 模式

`functiontool.New` 是核心适配器，将任意 Go 函数适配为 `tool.Tool`：

```go
func New[TArgs, TResults any](cfg Config, handler Func[TArgs, TResults]) (tool.Tool, error)
```

所有需要自定义行为的工具都可以通过 `functiontool.New` 构建：
- `exitlooptool.New()` → `functiontool.New(...)` + `agent.Actions().Escalate = true`
- `skilltool.ListSkills(source)` → `functiontool.New(...)` + `source.ListFrontmatters()`
- `skilltool.LoadSkill(source)` → `functiontool.New(...)` + `source.LoadInstructions()`

### 3.5 PackTool：工具声明合并

多个 `functionTool` 的声明会被合并到同一个 `genai.Tool{
FunctionDeclarations: [...]}` 中，避免创建过多的 genai.Tool 对象：

```go
// internal/toolinternal/toolutils/toolutils.go:35-69
func PackTool(req *model.LLMRequest, tool Tool) error {
    // 找到现有的 genai.Tool with FunctionDeclarations
    // 追加新的 Declaration
    // 或创建新的 genai.Tool
}
```

---

## 4. adk-go 代码落地

### 4.1 关键类型/函数/文件索引

| 类型/函数 | 文件 : 行号 | 用途 |
|-----------|------------|------|
| `tool.Tool` | `tool/tool.go:38` | 公共工具接口 |
| `tool.Toolset` | `tool/tool.go:57` | 工具集接口 |
| `tool.WithConfirmation` | `tool/tool.go:143` | HITL 装饰器 |
| `tool.FilterToolset` | `tool/tool.go:89` | 过滤装饰器 |
| `tool.AllowedToolsPredicate` | `tool/tool.go:76` | 工具名称白名单 |
| `tool.ErrConfirmationRequired` | `tool/tool.go:32` | HITL 需确认错误 |
| `toolinternal.FunctionTool` | `internal/toolinternal/tool.go:28` | 可执行工具内部接口 |
| `toolinternal.RequestProcessor` | `internal/toolinternal/tool.go:40` | 请求处理器接口 |
| `functiontool.New` | `tool/functiontool/function.go:79` | Go 函数 → Tool 适配器 |
| `functiontool.Func` | `tool/functiontool/function.go:72` | 工具函数签名 |
| `functiontool.Config` | `tool/functiontool/function.go:37` | 工具配置 |
| `functiontool.NewStreaming` | `tool/functiontool/streaming_function.go:38` | 流式函数 → Tool |
| `mcptoolset.New` | `tool/mcptoolset/set.go:49` | MCP 工具集工厂 |
| `mcptoolset.convertTool` | `tool/mcptoolset/tool.go:32` | MCP Tool → ADK Tool |
| `geminitool.New` | `tool/geminitool/tool.go:43` | Gemini 原生工具工厂 |
| `geminitool.GoogleSearch` | `tool/geminitool/google_search.go:28` | Google Search 工具 |
| `toolconfirmation.ToolConfirmation` | `tool/toolconfirmation/tool_confirmation.go:50` | 确认数据结构 |
| `toolconfirmation.OriginalCallFrom` | `tool/toolconfirmation/tool_confirmation.go:86` | 解析原始 tool call |
| `agenttool.New` | `tool/agenttool/agent_tool.go:54` | 子 Agent → Tool |
| `exitlooptool.New` | `tool/exitlooptool/tool.go:33` | 退出循环工具 |
| `exampletool.New` | `tool/exampletool/tool.go:43` | 示例注入工具 |
| `loadartifactstool.New` | `tool/loadartifactstool/load_artifacts_tool.go:42` | 加载 artifact |
| `loadmemorytool.New` | `tool/loadmemorytool/tool.go:42` | 加载记忆 |
| `preloadmemorytool.New` | `tool/preloadmemorytool/tool.go:50` | 预加载记忆 |
| `skilltoolset.New` | `tool/skilltoolset/toolset.go:65` | Skill 工具集工厂 |
| `skill.Source` | `tool/skilltoolset/skill/source.go:41` | Skill 数据源接口 |
| `toolutils.PackTool` | `internal/toolinternal/toolutils/toolutils.go:35` | 工具声明合并 |

### 4.2 典型调用链

#### 链 1：Go Function Tool 的完整生命周期

```
1. 定义:
   functiontool.New(cfg, handler)
   → 反射推断 schema  → 返回 *functionTool[TArgs, TResults]

2. 注册到 LLM 请求:
   Agent.Run() → 遍历 tools → ProcessRequest(ctx, req)
   → toolutils.PackTool(req, tool)
   → 找到/创建 genai.Tool{FunctionDeclarations: [...]}
   → 追加 Declaration()

3. LLM 调用:
   模型返回 FunctionCall(toolName="my_tool", args={...})

4. 执行:
   agent 构造 ToolContext(confirmation=nil)
   → functionTool.Run(ctx, args)
   → 检查 ctx.ToolConfirmation() (nil → 首次调用)
   → 检查 requireConfirmation / provider
   → 如需要: ctx.RequestConfirmation() → 返回 ErrConfirmationRequired
   → 代理循环重试: agent 收到 FunctionResponse(adk_request_confirmation, confirmed=true)
   → agent 构造 ToolContext(confirmation=&ToolConfirmation{Confirmed: true})
   → functionTool.Run(ctx, args) → 跳过确认 → handler(ctx, typedArgs) → 返回 map[string]any
```

#### 链 2：MCP Tool 的连接与执行

```
1. 工厂:
   mcptoolset.New(cfg)
   → 创建 connectionRefresher(client, transport)
   → 返回 *set（实现 Toolset）

2. 首次 LLM 请求:
   agent.Run() → set.Tools(ctx) → mcpClient.ListTools(ctx)
   → getSession() → client.Connect(ctx, transport) → 懒连接
   → ListTools with pagination + cursor → 转换 mcp.Tool → mcpTool

3. LLM 调用:
   与 functionTool 流程相同，但 mcpTool.Run() → mcpClient.CallTool(params)
   → 处理 StructuredContent / TextContent
   → 如果连接断开 → 自动重连 → 重试

4. 错误处理:
   连接断开 → shouldRefreshConnection(err) → refreshConnection()
   → Ping 验证 → Close 旧 session → Connect 新 session → 重试
```

#### 链 3：HITL 确认循环

```
工具 Run() 返回 ErrConfirmationRequired
→ agent 生成 adk_request_confirmation FunctionCall
→ 前端展示确认提示
→ 用户确认 → 前端发送 adk_request_confirmation FunctionResponse
→ agent 提取 ToolConfirmation{Confirmed: true}
→ agent 构造新 ToolContext(confirmation=confirmed)
→ 重新调用 tool.Run()
→ 工具检测 confirmed=true → 继续执行
→ 如 rejected → 返回 ErrConfirmationRejected → 错误传播到 LLM
```

### 4.3 模式清单 (Pattern Inventory)

| 模式 | 示例 | 说明 |
|------|------|------|
| **Adapter** | `functiontool.New` 将 Go func → `tool.Tool` | 泛型反射 + schema 推断 |
| **Adapter** | `mcptoolset.convertTool` 将 `mcp.Tool` → `mcpTool` | MCP protocol → ADK protocol |
| **Decorator** | `FilterToolset(predicate)` | 按条件过滤工具 |
| **Decorator** | `WithConfirmation(require, provider)` | 注入 HITL 逻辑 |
| **Factory** | `skilltoolset.New(ctx, cfg)` | 创建固定 3 个 tool 的 toolset |
| **Lazy Initialization** | `connectionRefresher.getSession()` | MCP 连接懒加载 |
| **Retry** | `withRetry[T]` | MCP 连接断开重试 |
| **Registry + Consolidation** | `toolutils.PackTool` | 多个 tool → 一个 genai.Tool |
| **Command** | `exitlooptool.exitLoop` 设置 `Actions().Escalate` | 工具作为控制流命令 |

### 4.4 测试覆盖

| 测试文件 | 覆盖内容 |
|----------|----------|
| `tool/tool_test.go` | `WithConfirmation` 核心 HITL 逻辑（7 个测试用例） |
| `tool/functiontool/function_test.go` | 函数工具创建/执行/错误处理 |
| `tool/functiontool/long_running_function_test.go` | 长期运行工具行为 |
| `tool/mcptoolset/set_test.go` + `testdata/*.httprr` | MCP 工具集连接与工具调用（HTTP replay） |
| `tool/geminitool/tool_test.go` | Gemini 工具 ProcessRequest |
| `tool/agenttool/agent_tool_test.go` | Agent-as-tool |
| `tool/exitlooptool/tool_test.go` | 退出循环 |
| `tool/exampletool/tool_test.go` | 示例注入 |
| `tool/loadartifactstool/load_artifacts_tool_test.go` | 加载 artifacts |
| `tool/loadmemorytool/tool_test.go` | 加载记忆 |
| `tool/preloadmemorytool/tool_test.go` | 预加载记忆 |
| `tool/toolconfirmation/tool_confirmation_test.go` | 确认数据解析 |
| `tool/skilltoolset/toolset_test.go` | Skill 工具集 |
| `tool/skilltoolset/internal/skilltool/tools_test.go` | Skill 内部工具 |
| `tool/skilltoolset/skill/*_test.go` | Skill 源码/预加载 |

### 4.5 未读风险 (Unread / Not-in-scope Risks)

1. **确认逻辑重复**：`functionTool.Run`、`streamingFunctionTool.RunStream`、`mcpTool.Run`、`confirmationTool.Run` 四个位置包含几乎相同的 HITL 逻辑（各 ~30 行）。任何修改需要同步四处，极易遗漏。

2. **MCP 不支持 long-running**：`mcpTool.IsLongRunning()` 硬编码返回 `false`（`mcptoolset/tool.go:83`），MCP 协议的异步能力未能映射。

3. **确认 Provider 接口不一致**：`functiontool` 的 provider 签名是 `func(TArgs) bool`（类型安全但绑定泛型参数），而 `mcptoolset` / `tool.WithConfirmation` 使用 `ConfirmationProvider = func(toolName string, toolInput any) bool`（统一但丢失类型安全）。

4. **`streamingFunctionTool` 缺少 `Run()` 方法**：只实现 `RunStream()`，不实现 `toolinternal.FunctionTool`，如果流式工具被 HITL 流程尝试用 `confirmationTool.Run()` 包装会失败。

5. **`agenttool` 未继承 HITL**：`agentTool.Run()` 没有确认逻辑，被包装在子 session 中的工具如果有确认需求，可能在子 Agent 上下文中处理不当。

6. **`runnableTool` 接口私有**：定义在 `tool/tool.go:189-193`，仅供 `WithConfirmation` 内部使用。外部无法自定义确认行为而不使用 `functiontool` / `mcptoolset`。

7. **`PackTool` 的 tool name 重复检测**：使用 `req.Tools[name]` map 检查重复，但只存 tool 引用（`toolutils/toolutils.go:42-44`），可能被错误地用于去重。

8. **`skill.Source` 接口并发安全要求**：注释标明必须并发安全（`skill/source.go:34-39`），但 `mergedSource` 和 `filesystem_source` 的并发行为未经过专门测试覆盖。

9. **Schema 推断的边界情况**：`functiontool/function.go:85-88` 只接受 struct 和 map，不支持简单类型（string, int）或数组作为输入。如果用户需要在 Go 层处理这些类型需要额外的包装开销。

10. **MCP 连接恢复与工具列表缓存**：`set.Tools()` 每次调用都重新 `ListTools`，如果 LLM 在多次 turn 中调用，MCP 服务器可能收到大量的 `ListTools` 请求。工具列表缓存策略缺失。

---

## 5. 深入追问

1. 为什么不将 HITL 确认逻辑提取为 `Run` 的 middleware/装饰器，避免四个位置的重复代码？

2. `streamingFunctionTool` 的确认逻辑会在 yield 之前执行，如果 HITL 循环发生，已 yield 的部分数据如何处理？框架是否支持 stream cancellation/resume？

3. `runnableTool` 是私有接口，但 `WithConfirmation` 是公开 API——如果用户实现了一个自定义 `FunctionTool` 但不暴露 `Run()` 方法就无法被 HITL 包装。是否应该公开 `runnableTool`？

4. MCP 的 `IsLongRunning` 一直返回 false。如果 MCP 服务器提供 long-running 语义（如 Job/Handle 模式），ADK 如何支持？

5. `agenttool` 在子 session 中创建新的 `runner.Runner`，但未转发父 session 的 memory、artifacts 服务（只做了 state forwarding）。这是故意设计还是待实现？

6. `toolutils.PackTool` 把多个 `FunctionTool.Declaration()` 合并到一个 `genai.Tool{FunctionDeclarations: [...]}`，但 Gemini 对 `FunctionDeclarations` 数组大小是否有上限？是否需要分片？

7. `geminitool` 的 `ProcessRequest` 通过 `setTool()` 追加到 `req.Config.Tools`。这些 Gemini 原生工具也需要和 `functionTool` 共享 token 预算吗？它们对 API 调用的影响是否有模型差异？

8. `preloadmemorytool` 在每个请求阶段都会 `SearchMemory`，若用户连续发送相同 query，会有重复搜索开销。是否需要基于 session 的缓存/去重？

9. Skill 的 `completePreloadSource` 预加载所有技能资源。对于大型技能集合（如 50+ 技能），内存占用和处理延迟是否可接受？延迟加载（按需）是否有性能优势？

10. `toolconfirmation.OriginalCallFrom` 仅支持 `*genai.FunctionCall` 和 `map[string]any` 两种格式。如果后续引入其他 RPC 格式（如 gRPC、Protobuf），解析路径如何扩展？
