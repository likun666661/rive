# R1 组件契约差距审计：Chapter 05 (Model / Tool / Prompt) vs 当前 Go 复刻版

> 审计目标：对照 Eino 技术手册第五章（`05-components-model-tool-prompt.md`）定义的组件契约，扫描 `compose/` 下所有源文件，识别当前复刻版在 ChatModel / Tool / ToolsNode / PromptTemplate / Message schema 五个维度的能力差距，为教育子集的实现边界提供建议。
> 不修改 Go 生产代码。

---

## 1. Chapter 05 解决的核心问题

Eino 将 LLM 应用编排为图。图的运行时（`compose` 包）需要调用模型、执行工具调用、格式化提示词、嵌入文本、索引文档以及检索相关片段——这一切都无需了解每个操作背后是哪个 provider 或后端。

**组件层通过为每个能力定义唯一的最小接口来解决这个问题。** 一个配置了 `BaseChatModel` 的图节点可以调用 `Generate` 和 `Stream`，无论其实现是 `openai.ChatModel`、`anthropic.ChatModel` 还是本地的 Ollama 封装。接口即是契约。

关键设计目标：
1. **Provider 无关性** —— 图不感知底层 API 差异
2. **接口封闭** —— 通过 Go 类型约束 `messageType` 限制参数类型（仅 `*schema.Message` / `*schema.AgenticMessage`）
3. **双桶选项** —— 公共选项（`Temperature`, `Model`, `TopP`, `MaxTokens`, `Stop`, `Tools`）与 provider 特定选项通过 `Option{apply, implSpecificOptFn}` 结构共存

---

## 2. 为什么组件契约很难

Chapter 05 指出了四个核心难点：

### 2.1 接口粒度

如果每个子能力都拥有自己的接口，实现者将淹没在样板代码中。如果接口过于粗糙（一个包含 30 个方法的巨型 `Component` 接口），大多数后端只能实现一个子集，而类型系统无法表达具体是哪一子集——你只能将校验推迟到运行时。Eino 采取了折中方案：`BaseModel` 恰好只有两个方法（`Generate` + `Stream`），但通过**接口组合**（`ToolCallingChatModel`）来表达诸如工具绑定之类的额外能力。

### 2.2 Provider 特定选项不得泄露

一个 OpenAI 模型需要 `openai.WithUser`，一个 Anthropic 模型需要 `anthropic.WithCache`，一个 Redis 检索器需要 `redis.WithIndexName`。如果公共接口为选项接受 `map[string]any`，则调用方失去类型安全。如果接口只接受公共选项，则 provider 失去表达能力。Eino 通过在 `Option` 结构体中使用双桶设计解决了这一问题。

### 2.3 工具绑定中的并发性

许多 LLM 框架允许通过 `BindTools(tools)` 来修改模型实例。在一个并发服务器中，goroutine A 绑定搜索工具，而 goroutine B 在同一个共享模型实例上绑定计算器工具——即时产生竞态。Eino 弃用了 `BindTools`，转而采用 `WithTools`，后者返回一个**新**的实例，使契约在设计上就是并发安全的。

### 2.4 工具结果的保真度

某些工具返回纯文本（`"42"`），另一些返回图片、音频或视频（多模态结果）。如果组件接口仅支持 `string` 类型的工具输出，多模态工具就不得不将富媒体序列化为有损的字符串表示。Eino 通过一个"增强"工具层级来解决该问题，该层级携带 `schema.ToolResult`——一个包含文本、图片、音频、视频和文件内容的结构化容器。

---

## 3. 当前复刻版已有的能力

### 3.1 ChatModel 接口（`chatmodel.go:24-27`）

```go
type ChatModel interface {
    Generate(ctx context.Context, input []*Message) (*Message, error)
    Stream(ctx context.Context, input []*Message) (StreamReader[*Message], error)
}
```

| 维度 | 当前复刻版 | Eino 第五章 |
|------|-----------|-------------|
| 方法签名 | `Generate(ctx, []*Message) (*Message, error)` | `Generate(ctx, []*Message/M, opts ...Option) (Message/M, error)` |
| Stream | `Stream(ctx, []*Message) (StreamReader[*Message], error)` | `Stream(ctx, []*Message/M, opts ...Option) (*schema.StreamReader[M], error)` |
| Options | **无** | `model.WithTemperature`, `model.WithModel`, `model.WithTools`, `model.WithTopP`, `model.WithMaxTokens`, `model.WithStop`, `model.WithToolChoice` 等 |
| 并发安全 | 通过 `FakeChatModel.mu sync.Mutex` 演示 | 接口级设计：`WithTools` 返回新实例 |
| 工具绑定 | **无** | `ToolCallingChatModel.WithTools(tools) (ToolCallingChatModel, error)` |
| 类型参数 | 硬编码 `*Message` | `BaseModel[M messageType]` 泛型约束为 `*schema.Message` / `*schema.AgenticMessage` |
| 回调支持 | **无** | `CallbackInput{Messages, Config}`, `CallbackOutput{Message, Config, TokenUsage}` |
| Fake 实现 | `NewFakeChatModel` 支持 `WithChatGenerateFunc`/`WithChatStreamFunc` | 复杂 mock 层 |

### 3.2 Retriever 接口（`retriever.go:20-22`）

```go
type Retriever interface {
    Retrieve(ctx context.Context, query *Query) ([]*Document, error)
}
```

| 维度 | 当前复刻版 | Eino 第五章 |
|------|-----------|-------------|
| 方法签名 | `Retrieve(ctx, *Query) ([]*Document, error)` | `Retrieve(ctx, query string, opts ...Option) ([]*schema.Document, error)` |
| Options | **无** | `retriever.WithTopK`, `retriever.WithScoreThreshold`, `retriever.WithEmbedding` |
| Query 类型 | 自定义 `*Query{Text, K}` | 纯 `string`，参数通过 options 传递 |
| 回调支持 | `NewRetrieverLambda` 支持 `Handler` | `CallbackInput{Query, Options}`, `CallbackOutput{Docs}` |

### 3.3 Bridge Adapter 模式（`bridge.go`）

当前复刻版已实现：
- `BridgeRetriever` / `BridgeChatModel` 领域接口
- `retrieverBridge` / `chatModelBridge` / `promptAssemblerBridge` 适配器
- `Workflow.AsRetrieverNode` / `AsChatModelNode` / `AsPromptAssemblerNode` 便捷方法

但 README 明确列出未实现：
> - **Tool bridge**: `Tool.Execute()` → Lambda
> - **StreamChatModel bridge**: `GenerateStream()` → `StreamableLambda`
> - **Embedding bridge**: `Embedder.Embed()` → Lambda

### 3.4 已有的组件图节点封装

| 组件 | 封装 | 位置 |
|------|------|------|
| ChatModel | `ChatModelComponent` → `composableRunnable` | `chatmodel.go:112-145` |
| Retriever | `NewRetrieverLambda` → `Lambda` | `retriever.go:46-74` |

两者均在 `GetRunnable()` / `cr` 层面处理类型断言，无 `Option` 传递路径。

---

## 4. 缺失的能力：逐项差距分析

### 4.1 PromptTemplate（完整缺失）

**当前复刻版：无 `ChatTemplate` 接口，无 `Format` 方法。**
`promptAssemblerBridge`（`bridge.go:91-115`）是一个硬编码的 RAG 提示词构造器，不是通用模板系统。

**Eino 第五章定义：**
```go
type ChatTemplate interface {
    Format(ctx context.Context, vs map[string]any, opts ...Option) ([]*schema.Message, error)
}
```

**差距明细：**

| 子能力 | 是否存在 | 备注 |
|--------|---------|------|
| `ChatTemplate` 接口 | ❌ | 完全缺失 |
| `Format(ctx, map[string]any) ([]*Message, error)` | ❌ | 完全缺失 |
| 变量替换语法（FString / GoTemplate / Jinja2） | ❌ | 完全缺失 |
| `AgenticChatTemplate`（返回 `[]*AgenticMessage`） | ❌ | 完全缺失 |
| 回调支持 | ❌ | `CallbackInput{Variables, Templates}`, `CallbackOutput{Result}` |
| Prompt → graph 节点适配 | ❌ | Eino 有 `toChatTemplateNode` |
| 缺失变量运行时错误 | ❌ | 无模板引擎 |

**当前替代方案：** `promptAssemblerBridge` 硬编码 system prompt + context 组装，非通用。

### 4.2 Tool / ToolsNode（完整缺失）

**当前复刻版：无任何工具接口，无 `ToolsNode`，无 `ToolInfo`/`ToolCall` 类型。**
桥接层（`bridge.go`）预留了 "Tool bridge" 占位符但未实现。

**Eino 第五章定义的工具接口层次（`components/tool/interface.go`）：**

```
BaseTool (Info)                                           ← 仅元数据
  ├── InvokableTool (BaseTool + InvokableRun)             ← 字符串输入，字符串输出
  ├── StreamableTool (BaseTool + StreamableRun)           ← 字符串输入，流式输出
  ├── EnhancedInvokableTool (BaseTool + InvokableRun with ToolResult)  ← 结构化输入/输出
  └── EnhancedStreamableTool (BaseTool + StreamableRun with ToolResult)
```

**ToolsNode（`compose/tool_node.go`）关键能力：**
| 能力 | 是否存在 | 备注 |
|------|---------|------|
| `ToolsNode.Invoke(ctx, *Message) ([]*Message, error)` | ❌ | 完全缺失 |
| `ToolsNode.Stream` | ❌ | 完全缺失 |
| 并行工具执行 | ❌ | 当前复刻版无 goroutine 池用于工具 |
| 顺序工具执行 | ❌ | 当前复刻版无工具执行 |
| 中断并重运行 | ❌ | 需要 `ToolsInterruptAndRerunExtra` |
| 参数别名重映射 | ❌ | 需要 `ToolAliasConfig` |
| 增强 vs 标准接口优先级 | ❌ | `convTools` 类型断言模式 |
| `UnknownToolsHandler` | ❌ | 未知工具名处理 |

**`schema.ToolInfo` 结构：**
```go
type ToolInfo struct {
    Name        string
    Desc        string
    ParamsOneOf *ParamsOneOf  // JSON Schema 参数定义
}
```

### 4.3 Message / ToolCall Schema（完整缺失）

**当前复刻版：`Message` 仅含 `Role` + `Content`。**

```go
type Message struct {
    Role    RoleType
    Content string
}
```

**Eino 第五章的 `schema.Message`：**

| 字段 | 当前复刻版 | Eino `schema.Message` |
|------|-----------|----------------------|
| `Role` | ✅ `RoleType`（system/human/assistant/tool） | ✅ `Role` |
| `Content` | ✅ `string` | ✅ `string` |
| `ToolCalls` | ❌ | `[]ToolCall`（ID + Function{Name, Arguments}） |
| `ToolCallID` | ❌ | `string`（回复工具结果时引用） |
| `MultiContent` | ❌ | 多模态内容（text/image/audio/video/file） |
| `Name` | ❌ | 可选的发送者名称 |

**Eino 的 `schema.ToolCall`：**
```go
type ToolCall struct {
    ID       string
    Function ToolCallFunction
}

type ToolCallFunction struct {
    Name      string
    Arguments string  // JSON
}
```

**Eino 的 `schema.ToolResult`：**
```go
type ToolResult struct {
    Text   string
    Images []*ImageContent
    Audio  []*AudioContent
    Video  []*VideoContent
    Files  []*FileContent
}
```

**当前复刻版缺少的 ToolCall 相关模式：**
1. 模型输出 `ToolCalls` → Assistant 消息携带工具调用意图
2. `ToolsNode` 读取 `ToolCalls` → 执行工具 → 生成 Tool 消息
3. Tool 消息的 `ToolCallID` 关联回原始工具调用
4. 多模态工具结果 → `Message.Parts`/`MultiContent`

### 4.4 Options 系统（完整缺失）

**当前复刻版：无 `Option` 类型，无双桶设计。**

| 选项 | 当前复刻版 | Eino |
|------|-----------|------|
| `WithTemperature` | ❌ | `model/option.go` |
| `WithModel` | ❌ | `model/option.go` |
| `WithTools` | ❌ | `model/option.go:116`（nil→空切片规范化） |
| `WithTopP` / `WithMaxTokens` / `WithStop` | ❌ | `model/option.go` |
| `WithToolChoice` / `WithAllowedToolNames` | ❌ | `model/option.go` |
| `WithDeferredTools` / `WithToolSearchTool` | ❌ | 服务端工具搜索 |
| `WrapImplSpecificOptFn` | ❌ | `option.go:196` |
| `GetCommonOptions` / `GetImplSpecificOptions` | ❌ | 双桶提取 |

### 4.5 组件元数据 / 回调查询（完整缺失）

| 能力 | 当前复刻版 | Eino |
|------|-----------|------|
| `Typer.GetType() string` | ❌ | `components/types.go:29` |
| `Checker.IsCallbacksEnabled() bool` | ❌ | `components/types.go:44` |
| `ComponentOfChatModel` | ✅ `ComponentOfChatModel` | ✅ |
| `ComponentOfTool` | ❌ | ✅ |
| `ComponentOfPrompt` | ❌ | ✅ |
| 组件回调输入/输出类型 | ❌ | `callback_extra.go` 每组件包 |

---

## 5. 推荐的教育子集实现边界

### 5.1 总体原则

当前复刻版定位为**教育子集**，目标不是完整复刻 Eino，而是用最小可工作例程演示 Chapter 05 的核心契约模式。以下建议优先考虑**接口设计模式的示范价值**，而非功能完备性。

### 5.2 Recommended: 第一优先级（核心契约模式）

| 组件 | 建议实现 | 教学价值 |
|------|---------|---------|
| **`ChatTemplate` 接口** | `Format(ctx, map[string]any) ([]*Message, error)` | 展示"接口即函数"模式 |
| **简单变量替换** | `{{variable}}` 或 `{variable}` 替换，单文件实现 | 展示契约 vs 实现分离 |
| **`BaseTool` + `InvokableTool`** | `Info()` + `InvokableRun()`，2-3 个接口 | 展示分层接口层次结构 |
| **最小化 `ToolsNode`** | 仅 `Invoke` 模式，顺序执行 2-3 个工具 | 展示工具执行编排模式 |
| **`Message.ToolCalls` 字段** | 添加 `[]ToolCall` 到 `Message` | 展示消息上下文传递 |
| **`ToolCall{ID, Function{Name, Arguments}}`** | 最小 schema 类型 | 展示工具调用数据模型 |
| **组件元数据接口** | `Typer` + `ComponentOfTool`/`ComponentOfPrompt` 常量 | 展示运行时自省模式 |

### 5.3 Recommended: 第二优先级（强化现有能力）

| 组件 | 建议实现 | 教学价值 |
|------|---------|---------|
| `Option` 单桶系统 | 简单 `type Option func(*Options)`，仅公共选项 | 展示选项模式，暂不引入双桶复杂度 |
| `ChatModel` 选项支持 | 在 `Generate/Stream` 签名中添加 `...Option` | 展示公共选项传递 |
| Prompt → graph 节点适配 | `toChatTemplateNode` 或 `AsPromptNode` | 展示 bridge 模式的完整链条 |
| Tool → Lambda bridge | 桥接文档中已列出的 `Tool.Execute() → Lambda` | 完善 bridge 模式演示 |

### 5.4 Not Recommended: 推迟到后续章节

| 组件 | 推迟原因 | 替代教学路径 |
|------|---------|-------------|
| 双桶选项（`implSpecificOptFn`） | 增加复杂度但教育价值有限 | 用配置结构体替代 |
| `StreamableTool` / `EnhancedInvokableTool` | 需要 stream 完整支持 | 仅实现 `InvokableTool` |
| `AgenticChatTemplate` | 依赖 `AgenticMessage` 类型 | 推迟到智能体章节 |
| 工具并行执行 | 依赖 goroutine 池管理 | 顺序执行即可展示概念 |
| 中断并重运行 | 依赖 checkpoint 系统（第四章） | 保持 checkpoint 关注点分离 |
| 多模态工具结果 | `ToolResult` 多模态内容增加了 schema 复杂度 | 仅返回字符串结果 |
| `DeferredTools` / `ToolSearchTool` | 服务端工具搜索高级特性 | 教学子集不需要 |
| 完整的 `callback_extra.go` | 回调扩展属于可观测性层 | 当前 `CallbackWrapper` 足够 |

### 5.5 建议文件变更清单

| 操作 | 文件 | 建议内容 |
|------|------|---------|
| **新建** | `compose/prompt.go` | `ChatTemplate` 接口 + `FakeChatTemplate` + `MessageTemplate` 简单实现 + `Format` 方法 + `ChatTemplateComponent` → `composableRunnable` 适配 |
| **新建** | `compose/tool.go` | `BaseTool` / `InvokableTool` 接口 + `ToolInfo` 类型 + `ToolCall`/`ToolCallFunction`/`ToolResult` 类型 + `FakeWeatherTool` 示例 |
| **新建** | `compose/tool_node.go` | `ToolsNode` 结构（简化版 `Invoke` only）+ `genToolCallTasks`（无别名/无中断）+ `convTools` 类型断言 + `ToolsNodeComponent` → `composableRunnable` |
| **修改** | `compose/chatmodel.go` | 在 `ChatModel.Generate/Stream` 签名末尾添加 `...Option`（可选，兼容现有代码） |
| **修改** | `compose/chatmodel.go` | 在 `Message` 中新增 `ToolCalls []ToolCall` 字段（可选，最小新增） |
| **修改** | `compose/bridge.go` | 新增 `BridgeTool` 接口 + `toolBridge` 适配器 + `Workflow.AsToolNode` 便捷方法 |
| **新建** | `compose/option.go` | 简单 `type Option func(*Options)` + `Options{ Temperature, Model, TopP, MaxTokens, Stop }`（单桶，无 impl-specific） |
| **新建** | `compose/component_type.go` | 补全 `ComponentOfTool`, `ComponentOfPrompt` 常量 + `Typer`/`Checker` 接口定义 |

### 5.6 实现规模估算

| 组件 | 预估代码行数 | 复杂度 |
|------|-------------|--------|
| `prompt.go` | ~100-120 行 | 低 — 接口 + Format 实现 + 适配器 |
| `tool.go` | ~120-150 行 | 中 — 多层接口 + schema 类型 |
| `tool_node.go` | ~200-250 行 | 中 — 简化 ToolsNode 逻辑 |
| `option.go` | ~80-100 行 | 低 — 单桶选项模式 |
| `component_type.go` | ~30 行 | 低 — 常量和接口 |
| `bridge.go` 修改 | ~50 行新增 | 低 — Tool bridge adapter |
| `chatmodel.go` 修改 | ~10 行 | 低 — 签名扩展 |

**总计：~590-710 行新代码**，分布在 5-6 个新文件 + 2 个现有文件的微小修改。

### 5.7 与 Chapter 02/03/04 的集成

| 章节 | 集成点 |
|------|--------|
| Ch2 (Workflow/FieldMapping) | `AsPromptNode` / `AsToolNode` 添加到 workflow，FieldMapping 连接 prompt→model→tools |
| Ch3 (Runnable/Stream/Callback) | `ToolsNode.Stream` 模式（如果实现），Callback 观察 Tool 执行 |
| Ch4 (Bridge Adapter) | Tool bridge 完善 Bridge 模式演示 |

---

## 6. 差距总表

| 维度 | Chapter 05 定义 | 复刻版状态 | 差距等级 |
|------|----------------|-----------|---------|
| `ChatTemplate` 接口 + `Format` 方法 | ✅ `Format(ctx, vs map[string]any, opts ...Option) ([]*Message, error)` | ❌ 完全缺失 | **严重** |
| 变量替换语法 | ✅ FString / GoTemplate / Jinja2 | ❌ 完全缺失 | **严重** |
| `BaseTool` 接口 (Info) | ✅ `Info() (*ToolInfo, error)` | ❌ 完全缺失 | **严重** |
| `InvokableTool` 接口 (Run) | ✅ `InvokableRun(ctx, argumentsInJSON, opts) (string, error)` | ❌ 完全缺失 | **严重** |
| `ToolsNode` 执行 | ✅ `Invoke(ctx, *Message) ([]*Message, error)` | ❌ 完全缺失 | **严重** |
| `Message.ToolCalls` | ✅ `[]ToolCall{ID, Function{Name, Arguments}}` | ❌ 完全缺失 | **严重** |
| `ToolInfo` schema | ✅ `Name, Desc, ParamsOneOf` | ❌ 完全缺失 | **严重** |
| `Option` 双桶系统 | ✅ `Option{apply, implSpecificOptFn}` | ❌ 完全缺失 | **严重** |
| `ToolCallingChatModel.WithTools` | ✅ 返回新实例，并发安全 | ❌ 完全缺失 | **高** |
| ChatModel Options 参数 | ✅ `opts ...Option` | ❌ 签名无 options | **高** |
| `Typer.GetType()` / `Checker` | ✅ 运行时类型/元数据自省 | ❌ 完全缺失 | **中** |
| 增强工具接口 | ✅ `EnhancedInvokableTool` (ToolResult 多模态) | ❌ 完全缺失 | **中** |
| `ComponentOfTool` / `ComponentOfPrompt` | ✅ 组件种类常量 | ❌ 缺失 | **中** |
| Prompt/Tool 回调扩展 | ✅ `CallbackInput/CallbackOutput` + `ConvCallbackInput/ConvCallbackOutput` | ❌ 完全缺失 | **低** |
| 回调 TokenUsage 信息 | ✅ `TokenUsage{Prompt, Completion, Total}` | ❌ 完全缺失 | **低** |

---

## 7. 当前已存在但需要适配的代码

| 现有代码 | 位置 | 变化方向 |
|---------|------|---------|
| `Message` 结构体 | `chatmodel.go:19-22` | 新增 `ToolCalls []ToolCall` 字段 |
| `ChatModel` 接口 | `chatmodel.go:24-27` | 新增 `opts ...Option` 参数（向后兼容） |
| `ChatModelComponent` | `chatmodel.go:112-145` | 新增选项传递路径 |
| `NewRetrieverLambda` | `retriever.go:46-74` | 新增 `opts ...Option` 支持 |
| `promptAssemblerBridge` | `bridge.go:91-115` | 重构为使用 `ChatTemplate`（可选） |
| `bridge.go` 预留位置 | `bridge.go` | 新增 Tool bridge 适配器 |

---

## 8. 风险与注意事项

### 8.1 向后兼容

- `ChatModel` 接口的 `Generate`/`Stream` 签名修改会破坏现有实现。建议：如果保持教育子集定位，可以保留当前简化接口，在说明文档中标注完整签名的位置。
- 新增 `Message.ToolCalls` 字段是零成本的——现有代码不读取该字段即可保持兼容。

### 8.2 教学顺序建议

1. 先实现 `BaseTool` + `InvokableTool` + `ToolCall` schema（独立概念，最易理解）
2. 再实现 `ToolsNode`（依赖工具 schema，展示编排模式）
3. 再实现 `ChatTemplate`（展示接口即函数模式）
4. 最后扩展 `ChatModel` 签名和 Options（增量修改）

### 8.3 与非教育复刻版的区别

如果本项目目标是**产品级复刻**，上述"不推荐"列表都需要重新评估。特别是双桶选项和完整增强工具接口对于生产兼容性是必需的。当前建议基于"教学子集"的定位。

---

*审计完成时间：2026-06-05*
*审计范围：`05-components-model-tool-prompt.md`（385 行）+ `compose/` 下 6 个组件相关源文件 + `README.md`*
*关键发现：ChatModel 骨架已存在但缺 Options；Retriever 已存在但缺 Options；PromptTemplate / Tool / ToolsNode / ToolCall schema / Options 系统五项完整缺失；Bridge 模式已为 ChatModel 和 Retriever 建立，Tool bridge 留空。*
