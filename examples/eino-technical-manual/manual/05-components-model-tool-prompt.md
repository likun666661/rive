# 第五章：组件 — Model / Tool / Prompt 契约

## 1. 问题

Eino 是一个将 LLM 应用编排为图的框架。图运行时（`compose` 包）需要调用模型、执行工具调用、格式化提示词、嵌入文本、索引文档以及检索相关片段——这一切都无需了解每个操作背后是哪个 provider 或后端。如果图必须理解 OpenAI 与 Anthropic API 的差异，或者 FAISS 索引器与 Redis 检索器的区别，那么它将无法做到通用。

组件层通过为**每个能力定义唯一的最小接口**来解决这个问题。一个配置了 `BaseChatModel` 的图节点可以调用 `Generate` 和 `Stream`，无论其实现是 `openai.ChatModel`、`anthropic.ChatModel` 还是本地的 Ollama 封装。接口即是契约。

## 2. 为什么这么难

抽象_深度_的拿捏是难点所在：

**接口粒度。** 如果每个子能力都拥有自己的接口，实现者将淹没在样板代码中。如果接口过于粗糙（一个包含 30 个方法的巨型 `Component` 接口），大多数后端只能实现一个子集，而类型系统无法表达具体是哪一子集——你只能将校验推迟到运行时。Eino 采取了折中方案：`BaseModel` 恰好只有两个方法（`Generate` + `Stream`），但通过_接口组合_（`ToolCallingChatModel`）来表达诸如工具绑定之类的额外能力。

**Provider 特定选项不得泄露。** 一个 OpenAI 模型需要 `OpenAI-specific-user`，一个 Anthropic 模型需要 `Anthropic-specific-cache`，一个 Redis 检索器需要 `Redis-specific-index-name`。如果公共接口为选项接受 `map[string]any`，则调用方失去类型安全。如果接口只接受公共选项，则 provider 失去表达能力。Eino 通过在 `Option` 结构体中使用双桶设计解决了这一问题：公共选项通过 `GetCommonOptions` 应用，实现特定的选项通过 `GetImplSpecificOptions` 应用。

**工具绑定中的并发性。** 许多 LLM 框架允许通过 `BindTools(tools)` 来修改模型实例。在一个并发服务器中，goroutine A 绑定搜索工具，而 goroutine B 在同一个共享模型实例上绑定计算器工具——即时产生竞态。Eino 弃用了 `BindTools`，转而采用 `WithTools`，后者返回一个_新_的实例，使契约在设计上就是并发安全的。

**工具结果的保真度。** 某些工具返回纯文本（`"42"`），另一些返回图片、音频或视频（多模态结果）。如果组件接口仅支持 `string` 类型的工具输出，多模态工具就不得不将富媒体序列化为有损的字符串表示。Eino 通过一个“增强”工具层级来解决该问题，该层级携带 `schema.ToolResult`——一个包含文本、图片、音频、视频和文件内容的结构化容器。

## 3. 设计思路

Eino 的组件契约遵循五种模式：

### 3.1 接口极简设计

每个组件类别暴露一到两个方法。没有“init”，没有“close”，没有生命周期——接口就是带有选项的**函数调用**：

| 组件 | 接口 | 方法 |
|-----------|-----------|---------|
| ChatModel | `BaseChatModel = BaseModel[*schema.Message]` | `Generate`、`Stream` |
| Tool | `BaseTool` → `InvokableTool` / `StreamableTool` | `Info`、`InvokableRun` / `StreamableRun` |
| Prompt | `ChatTemplate` | `Format` |
| Embedder | `Embedder` | `EmbedStrings` |
| Indexer | `Indexer` | `Store` |
| Retriever | `Retriever` | `Retrieve` |

`BaseModel[M]` 泛型（定义在 `components/model/interface.go:36`）是核心模型契约。它通过消息类型 `M` 参数化，经由类型约束 `messageType`（第 27 行）封闭为仅允许 `*schema.Message` 和 `*schema.AgenticMessage`：

```go
type messageType interface {
    *schema.Message | *schema.AgenticMessage
}

type BaseModel[M messageType] interface {
    Generate(ctx context.Context, input []M, opts ...Option) (M, error)
    Stream(ctx context.Context, input []M, opts ...Option) (*schema.StreamReader[M], error)
}
```

这为我们提供了两个具体的类型别名：
- `BaseChatModel = BaseModel[*schema.Message]`（标准聊天）
- `AgenticModel = BaseModel[*schema.AgenticMessage]`（智能体聊天）

### 3.2 双桶选项

`components/model/option.go:64` 中的 `Option` 结构体携带两个 setter，同一个 `Option` 永远不会同时填充两者：

```go
type Option struct {
    apply            func(opts *Options)   // 公共选项 setter
    implSpecificOptFn any                  // provider 特定 setter
}
```

诸如 `WithTemperature`、`WithModel`、`WithTools` 等公共选项填充 `apply`。Provider 特定的选项（如 `openai.WithUser`）使用 `WrapImplSpecificOptFn`（`option.go:196`）来填充 `implSpecificOptFn`。实现者依次调用两者：

```go
common := model.GetCommonOptions(nil, opts...)
myOpts := model.GetImplSpecificOptions(&MyOpts{}, opts...)
```

工具包在 `components/tool/option.go:22` 中以自己的 `Option` 结构体和 `WrapImplSpecificOptFn` 镜像了这一模式。

### 3.3 不可变工具绑定

旧的 `ChatModel` 接口（`components/model/interface.go:80`）暴露了会修改接收者的 `BindTools`：

```go
// 已弃用: BindTools 会修改实例 —— 非并发安全。
type ChatModel interface {
    BaseChatModel
    BindTools(tools []*schema.ToolInfo) error
}
```

替代接口 `ToolCallingChatModel`（`interface.go:99`）返回一个新实例，可在并发中安全使用：

```go
type ToolCallingChatModel interface {
    BaseChatModel
    WithTools(tools []*schema.ToolInfo) (ToolCallingChatModel, error)
}
```

使用模式：
```go
base, _ := openai.NewChatModel(ctx, cfg)         // 共享实例，无工具
withSearch, _ := base.WithTools([]*schema.ToolInfo{searchTool})
// base 保持无工具；withSearch 是绑定了搜索工具的新实例
```

### 3.4 分层工具接口

Eino 中的工具通过一个栈式接口层次结构（`components/tool/interface.go`）来定义：

```
BaseTool (Info)                                    ← 仅元数据
  ├── InvokableTool (BaseTool + InvokableRun)      ← 字符串输入，字符串输出
  ├── StreamableTool (BaseTool + StreamableRun)    ← 字符串输入，流式输出
  ├── EnhancedInvokableTool (BaseTool + InvokableRun with ToolResult)  ← 结构化输入/输出
  └── EnhancedStreamableTool (BaseTool + StreamableRun with ToolResult)
```

`BaseTool` 单独就足以通过 `WithTools` 将其工具 schema 传递给 ChatModel——模型仅需要工具的 JSON schema 来生成工具调用。但要让 `ToolsNode` _执行_一个工具，实现还必须至少满足 `InvokableTool` 或 `StreamableTool`（或其增强变体）之一。

当某个工具同时实现了标准和增强接口时，`ToolsNode` 会优先使用增强接口（`compose/tool_node.go:830-838`）。

### 3.5 每种组件类型的回调附加信息

每个组件包都定义了 `CallbackInput` 和 `CallbackOutput` 结构体以及 `ConvCallbackInput`/`ConvCallbackOutput` 辅助函数，使观察者能够检查类型化的输入和输出。示例：

- `components/model/callback_extra.go:66-80` —— `CallbackInput{Messages, Config}` 以及 `CallbackOutput{Message, Config, TokenUsage}`
- `components/tool/callback_extra.go:25-33` —— `CallbackInput{ArgumentsInJSON, Config}` 以及 `CallbackOutput{Response}`
- `components/prompt/callback_extra.go:25-35` —— `CallbackInput{Variables, Templates}` 以及 `CallbackOutput{Result}`
- `components/retriever/callback_extra.go:25-41` —— `CallbackInput{Query, Options}` 以及 `CallbackOutput{Docs}`

`ConvCallbackInput`/`ConvCallbackOutput` 函数执行安全的类型分支：如果原始回调值不匹配期望的类型，它们会返回 `nil`。这使全局回调处理器能够优雅地忽略它不关心的组件。

## 4. 源码走读

### 4.1 组件身份：`components/types.go`

两个可选接口（`types.go:29-52`）允许运行时检查组件：

- `Typer` → `GetType() string`：返回实现名称，如 `"OpenAIChatModel"`。工具使用它来设置其显示名称；图运行时使用它来生成调试输出（格式 `"{GetType()}{ComponentKind}"`）。

- `Checker` → `IsCallbacksEnabled() bool`：当实现返回 `true` 时，框架跳过其默认的 `OnStart`/`OnEnd` 包装，信任组件自行触发回调。这对于需要在流式传输过程中触发回调（而非仅在完成时）的流式模型至关重要。

`Component` 常量（`types.go:64-87`）标识类别：`ComponentOfChatModel`、`ComponentOfTool`、`ComponentOfPrompt` 等。它们流入 `callbacks.RunInfo.Component`，以便观察者根据种类进行分支。

### 4.2 ChatModel 和 ToolCallingChatModel：`components/model/interface.go`

完整的层次结构（均在 `interface.go` 中）：

- `BaseChatModel = BaseModel[*schema.Message]` —— 核心两方法契约（`Generate` + `Stream`），第 36-71 行。
- `ChatModel`（已弃用，第 80 行）—— 增加了会修改接收者的 `BindTools`。
- `ToolCallingChatModel`（第 99 行）—— 增加了不可变的 `WithTools`。
- `AgenticModel = BaseModel[*schema.AgenticMessage]`（第 109 行）—— 智能体变体；工具通过 `model.WithTools` 选项传入，而非接口方法。

关键设计决策：`AgenticModel` **没有** `WithTools` 方法。对于智能体模型，工具是在请求时通过 `model.WithTools` 选项（定义在 `model/option.go:116`）传入的。这是一种刻意的非对称设计——智能体模型将工具视为每次请求的关注点，而聊天模型将工具视为（不可变的）配置。

### 4.3 Model 选项：`components/model/option.go`

`Options` 结构体（第 22 行）携带 `Temperature`、`Model`、`TopP`、`MaxTokens`、`Stop`、`Tools`、`DeferredTools`、`ToolSearchTool`、`ToolChoice`、`AllowedToolNames` 和 `AgenticToolChoice`。

`WithTools` 选项函数（第 116 行）将 `nil` 规范化为空切片，以避免下游 nil 指针问题。

`ToolSearchTool` 和 `DeferredTools`（第 127-152 行）支持服务端工具搜索：工具以 `defer_loading=true` 的方式注册，一个特殊的“工具搜索工具”按需发现并加载它们。这是服务端工具调用的背后模式，其中模型 API 在内部处理工具搜索，而非由 Eino 框架处理。

### 4.4 Tool 接口：`components/tool/interface.go`

`BaseTool`（第 32 行）返回 `*schema.ToolInfo`——名称、描述、参数 JSON schema。这是唯一的方法。

`InvokableTool`（第 42 行）增加 `InvokableRun(ctx, argumentsInJSON string, opts ...Option) (string, error)`。参数以 JSON 字符串形式传入——框架**不解析**它们。调用方（`ToolsNode`）直接传递来自模型工具调用的原始 JSON。

`EnhancedInvokableTool`（第 67 行）使用 `*schema.ToolArgument` 替代原始字符串，并返回 `*schema.ToolResult` 替代字符串。`schema.ToolResult` 携带多模态内容（文本、图片、音频、视频、文件）。`compose/tool_node.go:830` 中的接口优先级规则：如果存在增强端点，`ToolsNode` 使用它们；否则回退到标准端点。

### 4.5 ToolsNode 执行：`compose/tool_node.go`

`ToolsNode`（第 79 行）是执行工具调用的图节点。其签名为：

```go
Invoke(ctx, *schema.Message, ...ToolsNodeOption) ([]*schema.Message, error)
Stream(ctx, *schema.Message, ...ToolsNodeOption) (*schema.StreamReader[[]*schema.Message], error)
```

输入：**一个**包含 `ToolCalls` 的 Assistant `Message`。输出：每个工具调用对应**一个** Tool `Message`。

关键执行细节：

**任务生成（`genToolCallTasks`，第 777 行）。** 遍历 `input.ToolCalls`，在 `tuple.indexes` 中查找每个工具名称，并构建一个带有合适端点（增强 vs 标准）的 `toolCallTask`。未知的工具名称会分派到 `UnknownToolsHandler`（如果已配置）；否则将报错。

**参数别名重映射（`remapArgs`，第 334 行）。** 当配置了 `ToolAliasConfig` 时，工具调用的参数 JSON 会被反序列化，键从别名重映射为规范名称，然后 JSON 在执行前被重新序列化。

**顺序执行 vs 并行执行。** 默认情况下（`executeSequentially = false`），工具调用通过 `parallelRunToolCall`（第 985 行）并发运行。第一个工具在调用 goroutine 上运行，其余在 `go` 协程中运行，通过 `sync.WaitGroup` 汇合。每个 goroutine 都有 panic 恢复包装。当 `executeSequentially = true` 时，调用通过 `sequentialRunToolCall`（第 973 行）按顺序执行。

**中断并重运行。** `ToolsNode` 支持检查点：如果某个工具返回 `InterruptRerunError`，节点会保存已执行的结果到 `ToolsInterruptAndRerunExtra`（第 287 行），并返回一个复合中断。重运行时，之前已执行的工具会被跳过（其结果可复用）。

**增强工具输出转换。** 对于增强工具，`ToolResult` 通过 `ToolResult.ToMessageInputParts()`（第 1129 行）转换为 `Message`，进而填充带有 `UserInputMultiContent` 多模态部分的字段。

### 4.6 Prompt 模板：`components/prompt/interface.go`

`ChatTemplate`（第 43 行）只有一个方法：

```go
Format(ctx context.Context, vs map[string]any, opts ...Option) ([]*schema.Message, error)
```

变量替换语法（FString、GoTemplate、Jinja2）在构造时选择。缺失的变量在运行时报错——提示词模板没有编译期安全。

`AgenticChatTemplate`（第 48 行）返回 `[]*schema.AgenticMessage`，用于智能体模型调用。

### 4.7 RAG 组件：Embedding、Indexer、Retriever

**Embedder**（`components/embedding/interface.go:37`）：
```go
EmbedStrings(ctx context.Context, texts []string, opts ...Option) ([][]float64, error)
```
每个输入文本返回一个向量，顺序一致。维度由底层模型确定。

**Indexer**（`components/indexer/interface.go:38`）：
```go
Store(ctx context.Context, docs []*schema.Document, opts ...Option) ([]string, error)
```
存储文档（若设置了 `Options.Embedding` 则可选地生成向量），并返回后端分配的 ID。

**Retriever**（`components/retriever/interface.go:48`）：
```go
Retrieve(ctx context.Context, query string, opts ...Option) ([]*schema.Document, error)
```
返回按相关性排序的匹配文档。`ScoreThreshold` 过滤低分文档；`TopK` 限制结果数量。

关键契约：Indexer 和 Retriever 必须使用**同一 embedder 模型**。维度或模型家族的错配将破坏语义相似性。`indexer.Options.Embedding` 和 `retriever.Options.Embedding` 字段均携带 embedder 引用。

### 4.8 组件到图节点桥接：`compose/component_to_graph_node.go`

每种组件类型都有一个 `to*Node` 适配器（第 49-167 行）：

| 函数 | 输入 | 图节点 |
|----------|-------|------------|
| `toChatModelNode` | `BaseChatModel` | Invoke=Generate，Stream=Stream |
| `toToolsNode` | `*ToolsNode` | Invoke/Stream 来自 `ToolsNode` 方法 |
| `toChatTemplateNode` | `ChatTemplate` | Invoke=Format |
| `toRetrieverNode` | `Retriever` | Invoke=Retrieve |
| `toIndexerNode` | `Indexer` | Invoke=Store |
| `toEmbeddingNode` | `Embedder` | Invoke=EmbedStrings |

核心函数 `toComponentNode`（第 29 行）使用 `parseExecutorInfoFromComponent` 提取 `Typer` 和 `Checker` 元数据，然后构建一个处理回调注入的 `composableRunnable`。

## 5. 模式与示例

### 5.1 最小化 ChatModel 实现

```go
type MyModel struct {
    defaultTemp float32
}

func (m *MyModel) Generate(ctx context.Context, input []*schema.Message, opts ...model.Option) (*schema.Message, error) {
    common := model.GetCommonOptions(&model.Options{Temperature: &m.defaultTemp}, opts...)
    myOpts := model.GetImplSpecificOptions(&MyOptions{}, opts...)
    // 使用 common.Temperature、common.Tools、myOpts.MyParam 等。
    return &schema.Message{Role: schema.Assistant, Content: "...response..."}, nil
}

func (m *MyModel) Stream(ctx context.Context, input []*schema.Message, opts ...model.Option) (*schema.StreamReader[*schema.Message], error) {
    // ...
}

func (m *MyModel) GetType() string { return "MyChatModel" } // 可选，用于 Typer
```

### 5.2 ToolCallingChatModel 实现

```go
type MyModel struct {
    baseConfig *Config
    tools      []*schema.ToolInfo
}

func (m *MyModel) WithTools(tools []*schema.ToolInfo) (model.ToolCallingChatModel, error) {
    newM := *m          // 浅拷贝
    newM.tools = tools   // 不修改原始实例
    return &newM, nil
}
```

### 5.3 最小化 InvokableTool

```go
type WeatherTool struct{}

func (t *WeatherTool) Info(ctx context.Context) (*schema.ToolInfo, error) {
    return &schema.ToolInfo{
        Name: "get_weather",
        Desc: "Get current weather for a city",
        ParamsOneOf: schema.NewParamsOneOfByParams(map[string]*schema.ParameterInfo{
            "city": {Type: schema.String, Desc: "City name", Required: true},
        }),
    }, nil
}

func (t *WeatherTool) InvokableRun(ctx context.Context, argsJSON string, opts ...tool.Option) (string, error) {
    var args struct{ City string }
    json.Unmarshal([]byte(argsJSON), &args)
    return fmt.Sprintf("Weather in %s: sunny, 22°C", args.City), nil
}
```

### 5.4 独立使用 Retriever

```go
retriever, _ := redis.NewRetriever(ctx, cfg)
docs, err := retriever.Retrieve(ctx, "what is eino?",
    retriever.WithTopK(5),
    retriever.WithScoreThreshold(0.7),
)
```

### 5.5 图集成模式

```go
graph := compose.NewGraph[string, *schema.Message]()
graph.AddChatModelNode("llm", baseModel)       // 组件 → 节点
graph.AddToolsNode("tools", toolsNode)           // 组件 → 节点
graph.AddRetrieverNode("retriever", retriever)   // 组件 → 节点
// compose 通过接口知道如何调用每一个组件
```

### 5.6 回调观察

```go
handler := callbacks.NewHandlerBuilder().
    OnStartFn(func(ctx context.Context, info *callbacks.RunInfo, input callbacks.CallbackInput) context.Context {
        if modelInput := model.ConvCallbackInput(input); modelInput != nil {
            log.Printf("[%s] model called with %d messages", info.Name, len(modelInput.Messages))
        }
        return ctx
    }).Build()

runnable.Invoke(ctx, input, compose.WithCallbacks(handler))
```

## 6. 常见陷阱

### 6.1 BindTools 在共享模型实例上的竞态

在跨 goroutine 共享的模型上使用已弃用的 `ChatModel.BindTools` 会导致数据竞态：一个请求的 `BindTools` 会在 `Generate` 执行前覆盖另一个请求的工具列表。**务必使用 `ToolCallingChatModel.WithTools`**，或对于 `AgenticModel` 通过 `model.WithTools()` 选项传入工具。

### 6.2 Options 中 nil 与空切片的区别

`model.WithTools(nil)` 会规范化为空切片（`option.go:117-118`），但并非所有选项函数都如此。将 `nil` 传给某个在未规范化的情况下解引用指针的函数会导致 panic。编写选项 getter 时务必检查 nil。

### 6.3 工具仅实现 BaseTool

定义一个满足 `BaseTool` 但不满足 `InvokableTool` 或 `StreamableTool` 的工具结构体可以编译通过，但 `ToolsNode` 在构造时会失败，报错 `"tool X is not invokable, streamable, enhanced invokable or enhanced streamable"`（`tool_node.go:541`）。这是一个运行时错误，而非编译期错误，因为 `ToolsNodeConfig.Tools` 接受 `[]tool.BaseTool`。

### 6.4 StreamReader 的所有权

当 `BaseModel.Stream` 返回 `*schema.StreamReader` 时，**调用方**负责 `Close()`。如果调用方没有关闭，为流提供数据的底层 goroutine 将泄漏。类似地，`ToolsNode.Stream` 返回一个合并的流读取器——调用方必须关闭它。

### 6.5 Indexer 与 Retriever 之间的 Embedder 不匹配

在存储时传入 `indexer.WithEmbedding(adaEmbedder)` 但在检索时传入 `retriever.WithEmbedding(bgeEmbedder)` 会默默产生无意义的结果，因为不同模型的向量存在于不同的语义空间中。务必配对使用同一个 embedder 实例。

### 6.6 回调处理器未关闭流副本

当多个处理器注册流计时时，流会被复制 N+1 次。如果任一处理器的副本未关闭，原始流将无法释放，导致整个管道的 goroutine 和内存泄漏。

### 6.7 重复传入工具

对于 `AgenticModel`，工具是在请求时通过 `model.WithTools()` 选项传入的。如果你还错误地调用了 `WithTools`（`AgenticModel` 上不存在该方法），或试图复制 `ToolCallingChatModel` 的模式，可能会将工具附加到了_选项_上，而不是_模型_上——模型将忽略它们。

## 7. Rive 可以学到什么

### 7.1 通过类型约束实现接口封闭

Eino 使用 Go 类型约束 `messageType` 将 `BaseModel[M]` 封闭为恰好两个具体类型（`*schema.Message` 和 `*schema.AgenticMessage`）。这比“到处做类型断言”的方式更干净。Rive 可以对其自身的泛型接口采用类似的模式，防止任意类型参数泄漏出抽象边界。

### 7.2 通过双桶选项实现可扩展性

`Option{apply, implSpecificOptFn}` 模式是封闭公共 API 与 provider 可扩展性之间的务实折中。它让每个 provider 可以发布自己的 `WithFoo` 函数，与公共的 `WithTemperature` 等无缝组合。Rive 的插件系统可以采用这一模式，让插件定义自定义选项而不会污染核心选项类型。

### 7.3 提供清晰迁移路径的弃用

从 `ChatModel.BindTools` 到 `ToolCallingChatModel.WithTools` 的迁移通过文档注释（提及竞态条件）、类型弃用注解以及接口注释中的清晰代码示例得到了良好的记录。Rive 应采用同样的实践：在弃用 API 时，解释_为什么_旧 API 不安全，并展示新的规范模式。

### 7.4 支持渐进能力的分层接口层次结构

`BaseTool → InvokableTool → EnhancedInvokableTool` 的层次结构让简单工具只需实现它们所需的内容，而 `ToolsNode` 在构造时通过类型断言发现能力。Rive 可以为其自身的可扩展接口使用这一模式——定义一个基础接口，让运行时发现扩展能力，而无需每个实现都全部满足。

### 7.5 工具执行中的运行时接口发现

`ToolsNode`（`convTools`，第 489 行）使用类型断言来发现一个工具满足了哪些接口，然后通过自动转换函数（`invokableToStreamable`、`streamableToInvokable` 等）将标准端点提升为增强端点，反之亦然。这意味着工具作者只需要实现**一个**执行方法，而 `ToolsNode` 使其在 Invoke 和 Stream 两种上下文中都能工作。Rive 的节点执行引擎可以从类似的自动能力桥接中受益。
