# R1 研究笔记：ChatModel / Retriever 组件契约与复刻版桥接需求

> 基于 Eino 技术手册第五章（05-components-model-tool-prompt.md）与第六章（06-schema-provider-adapters.md）
> 目标读者：实施工人（实现 ChatModel / Retriever 组件接口 + 桥接适配器）
> 语言：中文
> 状态：研究提案 — 不修改生产 Go 代码

---

## 目录

1. [ChatModel / Retriever 解决什么问题](#1-chatmodel--retriever-解决什么问题)
2. [为什么通用图组件需要桥接适配器](#2-为什么通用图组件需要桥接适配器)
3. [Sync vs Stream 语义](#3-sync-vs-stream-语义)
4. [Message / Document 类型契约](#4-message--document-类型契约)
5. [Callback 边界](#5-callback-边界)
6. [错误语义](#6-错误语义)
7. [本复刻版的具体实现需求](#7-本复刻版的具体实现需求)

---

## 1. ChatModel / Retriever 解决什么问题

### 1.1 原始问题

Eino 是一个将 LLM 应用编排为图的框架。图运行时（`compose` 包）需要调用模型生成回复、执行工具调用、格式化提示词、嵌入文本、索引文档、检索相关片段——这一切都无需了解每个操作背后是哪个 provider 或后端。

如果没有统一的组件接口，以下场景均不可行：

| 场景 | 无接口时的问题 |
|------|--------------|
| 在图中使用 OpenAI 模型 | 图节点必须导入 `openai` 包，与 provider 耦合 |
| 将 OpenAI 替换为 Anthropic | 需要重写图节点代码 |
| 在同一流水线中混用多个 provider | 图中的每个 ChatModel 节点都需要独立的 provider 特定逻辑 |
| 对模型调用添加回调观测 | 每个 provider 的调用方式不同，无法统一注入回调 |
| 工具调用 + 模型生成协同 | 工具定义格式因 provider 而异，无法在图中通用 |

### 1.2 ChatModel 契约的设计意图

`BaseChatModel`（即 `BaseModel[*schema.Message]`）定义了两个方法：

```go
Generate(ctx context.Context, input []*schema.Message, opts ...Option) (*schema.Message, error)
Stream(ctx context.Context, input []*schema.Message, opts ...Option) (*schema.StreamReader[*schema.Message], error)
```

这为图运行时提供了三个保证：

1. **类型安全**：输入输出都是 `*schema.Message`（规范消息类型），无需类型断言。
2. **Provider 透明**：图运行时只看到 `BaseChatModel` 接口——无论是 OpenAI、Anthropic 还是本地 Ollama，调用方式完全一致。
3. **选项可扩展**：通过 `Option` 双桶模式，公共选项（`WithTemperature`、`WithModel`）和 provider 特定选项（`openai.WithUser`）可以无缝组合。

在此之上，`ToolCallingChatModel` 增加了不可变工具绑定：

```go
type ToolCallingChatModel interface {
    BaseChatModel
    WithTools(tools []*schema.ToolInfo) (ToolCallingChatModel, error)
}
```

关键设计决策：`WithTools` 返回**新实例**，而非修改自身——这从类型系统层面消除了并发竞态。两个 goroutine 可以同时在同一个 `BaseChatModel` 实例上调用 `WithTools` 各自获得独立的绑定实例，互不干扰。

### 1.3 Retriever 契约的设计意图

`Retriever` 只定义一个方法：

```go
Retrieve(ctx context.Context, query string, opts ...Option) ([]*schema.Document, error)
```

与 ChatModel 类似，它提供 provider 透明性：无论是 FAISS 向量索引、Redis 检索器还是 Elasticsearch 全文搜索，调用方式一致。关键约束：Indexer 存储时使用的 Embedder 与 Retriever 检索时使用的 Embedder 必须是**同一模型**——这通过 `retriever.Options.Embedding` 字段显式传递 embedder 实例来保证，而非依赖隐式约定。

### 1.4 与当前复刻版的关系

当前复刻版（`eino-compose-runtime-replica-go`）实现了图编译与运行时引擎（第一章）以及 Chain/Workflow/FieldMapping（第二章），但**没有** ChatModel 或 Retriever 接口。图节点仅能通过 `AddLambdaNode` + `InvokableLambda` 添加纯函数。引入 ChatModel / Retriever 组件接口将使图能够原生编排 LLM 调用和文档检索，而无需为每个 provider 编写 Lambda 包装。

---

## 2. 为什么通用图组件需要桥接适配器

### 2.1 类型鸿沟：组件接口 ≠ 图节点接口

图运行时只认识 `Runnable[I, O]`（或其内部表示 `composableRunnable`）。它不知道 ChatModel、Retriever、Indexer 或 Tool 是什么。具体来说：

```
图运行时的世界          组件世界
─────────────          ─────────
Runnable[I,O]          BaseChatModel = BaseModel[*Message]
  ├── Invoke(I)→O        ├── Generate([]*Message) → *Message
  └── Stream(I)→Stream   └── Stream([]*Message) → StreamReader[*Message]

composableRunnable      Retriever
  ├── i (invoke)          └── Retrieve(string) → []*Document
  └── t (transform)
                       Tool
                         ├── Info() → *ToolInfo
                         └── InvokableRun(string) → string
```

图运行时的方法签名是 `Invoke(ctx, input) → (output, error)` 和 `Stream(ctx, input) → (*StreamReader, error)`。ChatModel 的方法签名是 `Generate(ctx, []*Message, ...Option) → (*Message, error)`。Retriever 的方法签名是 `Retrieve(ctx, string, ...Option) → ([]*Document, error)`。

如果不做桥接，ChatModel 和 Retriever 无法直接作为图节点使用。

### 2.2 桥接适配器承担的三项职责

Eino 通过 `compose/component_to_graph_node.go` 中的 `to*Node` 函数解决此问题。每个桥接适配器做三件事：

#### 职责 1：方法签名适配

将组件方法包装为 `composableRunnable` 所需的 `i` 和 `t`（或 `s`）函数指针：

| 组件 | 适配函数 | i (invoke) 的实现 | s/t (stream/transform) 的实现 |
|------|---------|-------------------|-------------------------------|
| ChatModel | `toChatModelNode` | 调用 `Generate` | 调用 `Stream` |
| Retriever | `toRetrieverNode` | 调用 `Retrieve` | 无（仅 sync） |
| ToolsNode | `toToolsNode` | 调用 `Invoke` | 调用 `Stream` |
| ChatTemplate | `toChatTemplateNode` | 调用 `Format` | 无（仅 sync） |
| Indexer | `toIndexerNode` | 调用 `Store` | 无（仅 sync） |
| Embedder | `toEmbeddingNode` | 调用 `EmbedStrings` | 无（仅 sync） |

以 ChatModel 为例：

```go
// 在 toChatModelNode 内部 (伪代码):
i = func(ctx context.Context, input any) (any, error) {
    msgs := input.([]*schema.Message)
    opts := extractOptions(ctx)  // 从 context/graph 提取 Option
    return model.Generate(ctx, msgs, opts...)
}
t = func(ctx context.Context, input *schema.StreamReader[any]) (*schema.StreamReader[any], error) {
    // 对仅支持 Generate 的模型，通过 streamByInvoke fallback 自动派生
}
```

#### 职责 2：组件元数据提取

通过 `parseExecutorInfoFromComponent` 检查组件是否实现可选接口：

- `components.Typer` → `GetType() string`：返回实现类型名（如 `"OpenAI"`）。填充 `RunInfo.Type` 字段。
- `callbacks.Checker` → `IsCallbacksEnabled() bool`：当返回 `true` 时，图运行时不包装 `runWithCallbacks`，因为组件自己在内部触发了回调。这防止回调重复触发。

```go
// executorMeta 结构 (概念):
type executorMeta struct {
    component                string  // "ChatModel" / "Retriever" / "Lambda"
    componentImplType        string  // "OpenAI" / "RedisRetriever" / ""
    isComponentCallbackEnabled bool  // true → compose 层不再包装回调
}
```

#### 职责 3：类型安全校验

编译时通过泛型约束校验组件输入输出类型与图节点声明的一致。例如，`toChatModelNode` 的输入类型必须是 `[]*schema.Message`，输出类型必须是 `*schema.Message`。如果用户试图将 ChatModel 接入一个输入类型为 `string` 的图位置，编译期报错。

### 2.3 统一入口：toComponentNode

Eino 的整体流程：

```
toComponentNode(component, options)
  ├── parseExecutorInfoFromComponent(component)
  │     ├── 检查 Typer → executorMeta.componentImplType
  │     └── 检查 Checker → executorMeta.isComponentCallbackEnabled
  ├── 根据 component 具体类型路由到 toChatModelNode / toRetrieverNode / ...
  └── 返回 composableRunnable + executorMeta
        ├── composableRunnable → 存入 graphNode
        └── executorMeta → 运行时用于构建 RunInfo
```

### 2.4 对复刻版的影响

当前复刻版的 `AddLambdaNode` 直接接收 `InvokableLambda[I, O]`（纯函数），避开了桥接问题。要支持原生 ChatModel / Retriever 节点，需要：

1. 定义组件接口（`BaseChatModel`、`Retriever` 等）
2. 实现 `toChatModelNode` / `toRetrieverNode` 等适配器
3. 在 `graph` 上增加 `AddChatModelNode` / `AddRetrieverNode` 方法
4. 扩展 `executorMeta`（或等价结构）以携带组件元数据

---

## 3. Sync vs Stream 语义

### 3.1 Eino 的四种执行模式

Eino 定义四种执行模式（`Runnable` 接口完整形态）：

```
Invoke    (I) → O                      同步：输入单值，输出单值
Stream    (I) → StreamReader[O]        异步：输入单值，输出流
Collect   (StreamReader[I]) → O        异步：输入流，输出单值
Transform (StreamReader[I]) → Stream   异步：输入流，输出流
```

ChatModel 原生支持两种：

| 方法 | 模式 | 语义 |
|------|------|------|
| `Generate` | Invoke | 发送全部消息，等待完整回复 |
| `Stream` | Stream | 发送消息，逐 token 流式返回 |

Retriever 仅原生支持一种：

| 方法 | 模式 | 语义 |
|------|------|------|
| `Retrieve` | Invoke | 发送查询，返回文档列表 |

当图运行时调用某个组件不具备的模式时，通过 `runnablePacker` 的 12 个降级函数自动派生。以 Retriever 为例：

```
用户调用 Stream(Retriever):
  streamByInvoke →
    调用 Retrieve → 获得 []*Document →
    包装为 StreamReaderFromArray → 返回 StreamReader[*[]*Document]
```

### 3.2 ChatModel 的 Sync 路径：Generate

```
Input:  []*schema.Message            ← 历史消息列表 (user/assistant/system)
Output: *schema.Message              ← 模型回复 (assistant 角色)

流程:
  1. 提取公共选项 (Temperature, MaxTokens, Model, Tools)
  2. 提取实现特定选项 (如 OpenAI 的 user 字段)
  3. 转换为 provider 原生格式
  4. 调用 provider API (同步)
  5. 转换为 *schema.Message 返回
```

关键约束：`Generate` 返回的 `*Message` 包含完整的 `Content`、`ToolCalls[]` 和 `ResponseMeta`（finish_reason、token_usage）。这在语义上是"一次调用产生全部结果"。

### 3.3 ChatModel 的 Stream 路径：Stream

```
Input:  []*schema.Message
Output: *schema.StreamReader[*schema.Message]

流程:
  1. 提取选项 (同上)
  2. 创建 Pipe(sr, sw)
  3. 启动 goroutine:
       for each provider chunk:
         转换为 *schema.Message (partial delta)
         sw.Send(partial, nil)
       sw.Close()
  4. 立即返回 sr (不等待 goroutine 完成)
```

关键约束：

- **每个流块都是合法的 `*schema.Message`**：例如，流中的第一个块 `Content="Hello"`，第二个块 `Content=" world"`。调用方最终通过 `ConcatMessages`（在 `internal.RegisterStreamChunkConcatFunc` 中注册）将 chunks 拼接为完整消息。
- **ToolCall 的 Index 必须在流中保持稳定**：同一工具调用的所有 delta 块共享相同的 `Index`，`ConcatMessages` 按 Index 分组并拼接 Arguments JSON 片段。
- **所有者规则**：`Stream` 返回的 `StreamReader` 的所有权转移给调用方。调用方**必须**调用 `Close()`，否则底层 goroutine 泄漏。

### 3.4 自动降级机制（runnablePacker）

`runnablePacker` 的核心价值：组件实现者只需提供任何子集的能力方法，运行时自动补齐其余模式。降级优先级如下：

```
Invoke 降级链 (从高到低):
  Native Invoke → invokeByStream → invokeByCollect → invokeByTransform

Stream 降级链:
  Native Stream → streamByTransform → streamByInvoke → streamByCollect
```

以 `streamByInvoke` 为例（这是 Retriever 最常见的降级路径）：

```go
// 伪代码:
func streamByInvoke(ctx, input) (*StreamReader, error) {
    output, err := i(ctx, input)          // 调用 Retrieve
    if err != nil { return nil, err }
    return StreamReaderFromArray([]O{output}), nil  // 包装为单元素流
}
```

### 3.5 对复刻版的影响

当前复刻版已实现 `composableRunnable` 的 `i`（invoke）和 `s`（stream fallback），但未实现完整的 12 降级函数矩阵。对于 ChatModel / Retriever 桥接，需要：

1. **最小实现**：ChatModel 提供 `i`（→ Generate）和 `s`（→ Stream）。Retriever 提供 `i`（→ Retrieve），`s` 通过 `streamByInvoke` 自动派生。
2. **可选增强**：实现 `runnablePacker` 的完整降级矩阵，使组件在其他模式（Collect、Transform）下也能正确降级。
3. **Stream 基础设施**：当前复刻版已有基础 Pipe stream（`compose/stream.go`），但 `StreamReader` 缺少 `Copy`（扇出）和 `Concat` 原语，这两者在多回调处理器和流拼接场景中是必需的。

---

## 4. Message / Document 类型契约

### 4.1 两种 Message 模型与泛型约束

Eino 定义了两个 Message 类型，通过 Go 泛型约束封闭为恰好可供 `BaseModel` 使用：

```go
type messageType interface {
    *schema.Message | *schema.AgenticMessage
}

type BaseModel[M messageType] interface {
    Generate(ctx context.Context, input []M, opts ...Option) (M, error)
    Stream(ctx context.Context, input []M, opts ...Option) (*schema.StreamReader[M], error)
}
```

具体类型别名：

```go
BaseChatModel = BaseModel[*schema.Message]      // 标准聊天
AgenticModel  = BaseModel[*schema.AgenticMessage] // 智能体聊天
```

### 4.2 `*schema.Message` — 经典消息模型

```go
type Message struct {
    Role             RoleType              // Assistant | User | System | Tool
    Content          string                // 纯文本
    UserInputMultiContent   []MessageInputPart   // 用户多模态输入
    AssistantGenMultiContent []MessageOutputPart // 模型多模态输出
    ToolCalls        []ToolCall            // assistant: 请求的工具调用
    ToolCallID       string                // tool: 此响应对应哪个调用 ID
    ToolName         string                // tool: 响应工具的名称
    ResponseMeta     *ResponseMeta         // finish_reason, usage, logprobs
    ReasoningContent string                // 思考/推理内容
    Extra            map[string]any        // 遗留扩展 (不推荐使用)
}

type ToolCall struct {
    Index    *int                          // 流式处理: 标识同一调用的 delta 块
    ID       string
    Type     string                        // "function"
    Function FunctionCall
    Extra    map[string]any
}

type FunctionCall struct {
    Name      string
    Arguments string                       // JSON 字符串
}
```

**角色语义**：

| Role | 发送方 | 包含内容 |
|------|--------|---------|
| `User` | 用户/Retriever | 问题、文档内容（注入到提示词中） |
| `Assistant` | ChatModel | 文本回复 和/或 ToolCalls[] |
| `System` | 开发者 | 系统级指令 |
| `Tool` | ToolsNode | 工具执行结果 (`ToolCallID` + `Content`) |

**多模态消息部分**（`MessageInputPart`）的类型判别：

```
ChatMessagePartTypeText         → 纯文本
ChatMessagePartTypeImageURL     → 图片 URL
ChatMessagePartTypeAudioURL     → 音频 URL
ChatMessagePartTypeVideoURL     → 视频 URL
ChatMessagePartTypeFileURL      → 文件 URL
ChatMessagePartTypeToolSearchResult → 工具搜索结果
```

### 4.3 `*schema.Document` — 检索文档

```go
type Document struct {
    ID        string                 // 后端分配的唯一标识
    Content   string                 // 文档文本内容
    Meta      map[string]any         // 元数据 (来源 URL, 作者, 时间戳等)
    Embedding []float64              // 向量 (可选，由 Embedder 生成)
    Score     float64                // 检索相关性分数
}
```

契约关键点：

- `ID` 由 Indexer 的 `Store` 返回，Retriever 的 `Retrieve` 返回的文档带上此 ID。
- `Embedding` 在存储时由 `indexer.Options.Embedding` 指定的 Embedder 生成；检索时 `retriever.Options.Embedding` 必须是同一 Embedder，否则向量空间不匹配。
- `Score` 由 Retriever 后端计算；`ScoreThreshold` 选项过滤低分文档。
- `Meta` 是唯一携带 Provider 特定元数据的位置，但它是 `map[string]any`，不享受类型安全。

### 4.4 Tool 的类型层次

```
BaseTool (Info)                                    ← 仅元数据: ToolInfo(name, desc, params)
  ├── InvokableTool (BaseTool + InvokableRun)      ← 字符串输入 → 字符串输出
  ├── StreamableTool (BaseTool + StreamableRun)    ← 字符串输入 → 流式字符串输出
  ├── EnhancedInvokableTool (BaseTool + InvokableRun with ToolArgument/ToolResult)
  └── EnhancedStreamableTool (BaseTool + StreamableRun with ToolArgument/ToolResult)
```

`BaseTool` 仅返回 `*schema.ToolInfo`（工具的 JSON Schema 描述），足以传给 ChatModel 的 `WithTools`。但要被 `ToolsNode` 执行，还需至少实现 `InvokableTool`。当同时实现标准和增强接口时，`ToolsNode` 优先使用增强端点（支持多模态工具输出）。

`*schema.ToolInfo`:

```go
type ToolInfo struct {
    Name        string
    Desc        string
    ParamsOneOf *ParamsOneOf           // 两种模式之一
    Extra       map[string]any
}
```

`ParamsOneOf` 支持两种互斥模式：

1. **`NewParamsOneOfByParams(map[string]*ParameterInfo)`**：轻量级，扁平参数描述（Type、Desc、Required、Enum、嵌套 SubParams）
2. **`NewParamsOneOfByJSONSchema(*jsonschema.Schema)`**：完整 JSON Schema 2020-12（支持 anyOf、oneOf、$defs）

### 4.5 对复刻版的影响

当前复刻版**没有** `schema/` 包。要支持组件接口，需要定义以下最小 schema 类型：

| 必须定义 | 类型 | 用途 |
|---------|------|------|
| `Message` | struct | ChatModel 输入/输出 |
| `RoleType` | string enum | User/Assistant/System/Tool |
| `ToolCall` | struct | 函数调用请求 |
| `ToolInfo` | struct | 工具 Schema 描述 |
| `Document` | struct | Retriever 输入/输出 |
| `StreamReader[T]` | generic | 已有 `compose/stream.go` 中的 Pipe 实现可复用 |

不需要在当前教育复刻版中实现 `AgenticMessage`（那需要完整的 `ContentBlock` 类型系统）、Provider 扩展槽位、或序列化注册——这些超出了教学范围。

---

## 5. Callback 边界

### 5.1 组件特定的回调数据结构

每个组件包定义自己的 `CallbackInput`/`CallbackOutput` 结构体，以及 `ConvCallbackInput`/`ConvCallbackOutput` 类型转换函数：

| 组件 | CallbackInput | CallbackOutput | 转换函数文件 |
|------|--------------|----------------|-------------|
| ChatModel | `{Messages []*Message, Config *Config}` | `{Message *Message, Config *Config, TokenUsage *TokenUsage}` | `components/model/callback_extra.go` |
| Tool | `{ArgumentsInJSON string, Config *Config}` | `{Response string}` | `components/tool/callback_extra.go` |
| Prompt | `{Variables map[string]any, Templates []string}` | `{Result []*Message}` | `components/prompt/callback_extra.go` |
| Retriever | `{Query string, Options *Options}` | `{Docs []*Document}` | `components/retriever/callback_extra.go` |

`ConvCallbackInput`/`ConvCallbackOutput` 执行安全的类型断言：

```go
func ConvCallbackInput(input callbacks.CallbackInput) *CallbackInput {
    switch v := input.(type) {
    case *CallbackInput:
        return v
    default:
        return nil
    }
}
```

这使全局回调处理器能够优雅地处理混合组件类型的图：

```go
handler.OnStartFn(func(ctx context.Context, info *RunInfo, input CallbackInput) context.Context {
    if mi := model.ConvCallbackInput(input); mi != nil {
        // 这是 ChatModel 调用 → 访问 mi.Messages
    }
    if ri := retriever.ConvCallbackInput(input); ri != nil {
        // 这是 Retriever 调用 → 访问 ri.Query
    }
    return ctx
})
```

### 5.2 Typer 和 Checker 接口

两个可选接口为回调引擎提供运行时类型信息：

```go
// components/types.go
type Typer interface {
    GetType() string  // 返回实现名，如 "OpenAIChatModel", "RedisRetriever"
}

type Checker interface {
    IsCallbacksEnabled() bool  // 返回 true → 组件自行触发回调，框架不再包装
}
```

**`Typer` 的使用场景**：
- 图运行时使用它填充 `RunInfo.Type`（格式 `"{GetType()}{ComponentKind}"`）
- 工具使用它设置显示名称
- 调试输出中标识组件来源

**`Checker` 的使用场景**：
- 流式 ChatModel 自行在流传输过程中触发 `OnEndWithStreamOutput` 回调。如果 `IsCallbacksEnabled()` 返回 `true`，图运行时**不**再用 `runWithCallbacks` 包装，防止回调重复触发。
- 当前复刻版的 `CallbackWrapper` 没有这个防护机制——如果组件和框架都触发回调，会看到重复的 OnEnd 事件。

### 5.3 RunInfo 的构建路径

```
图运行时 → initNodeCallbacks(ctx, nodeKey, executorMeta)
  ├── Name      ← nodeKey (图节点键)
  ├── Type      ← executorMeta.componentImplType (来自 Typer.GetType())
  └── Component ← executorMeta.component (ComponentOfChatModel / ComponentOfRetriever)
```

关键行为：

- 未经 `initNodeCallbacks` 的独立组件：`Name` 为空，`Type` 为反射推导的类型名
- 图级调用（`graphRunnable.Invoke`）：`Component` 固定为 `"Graph"`
- 每个处理器的 `OnStart → OnEnd` 链是独立的，不在处理器间共享 context

### 5.4 对复刻版的影响

当前复刻版有 `CallbackWrapper`（`compose/callbacks.go`），支持 OnStart / OnEnd / OnError / OnStartWithStreamInput / OnEndWithStreamOutput，但缺少：

1. **组件特定的 CallbackInput/CallbackOutput**：当前回调 handler 接收 `any` 类型的输入/输出，无法区分是 ChatModel 还是 Retriever。
2. **Typer / Checker 接口**：无法获取组件实现类型名，也无法防止回调重复触发。
3. **RunInfo 的 Type 和 Component 字段**：当前 `RunInfo`（复刻版）仅有 `Name` 字段。

要实现组件级回调，需要：

- 为 ChatModel / Retriever 定义各自的 `CallbackInput`/`CallbackOutput`
- 在桥接适配器中构造类型化的回调数据
- 扩展 `RunInfo` 添加 `Type` 和 `Component` 字段
- 在 `parseExecutorInfoFromComponent` 中检查 `Typer`/`Checker`

---

## 6. 错误语义

### 6.1 同步路径（Invoke / Generate / Retrieve）

所有组件方法遵循标准 Go 错误返回惯例：

```go
func (m *MyModel) Generate(ctx context.Context, input []*Message, opts ...Option) (*Message, error) {
    resp, err := callProviderAPI(ctx, input)
    if err != nil {
        return nil, fmt.Errorf("openai generate: %w", err)  // 包装 provider 错误
    }
    if resp.Error != nil {
        return nil, &ModelError{Code: resp.Error.Code, Message: resp.Error.Message}
    }
    return convertResponse(resp), nil
}
```

**关键规则**：
- Provider 原始错误应该用 `%w` 包装，保留错误链
- Provider 返回的 API 级错误（如 rate limit、content filter）应该转换为框架级错误类型
- 工具调用中未知工具名称 → `ToolsNode` 检查 `UnknownToolsHandler`，若未配置则报错 `"tool X not found in tools"`

### 6.2 流路径（Stream）

流错误传播有两层：

**第一层：Stream 调用本身失败**

```go
sr, err := model.Stream(ctx, msgs)
if err != nil {
    // 连接失败、认证失败等 → 立即返回 error
    return nil, err
}
```

**第二层：流消费过程中出错**

```go
for {
    chunk, err := sr.Recv()
    if errors.Is(err, io.EOF) {
        break                     // 正常结束
    }
    if err != nil {
        // 流中断: provider 返回的超时、服务器错误等
        return nil, err
    }
    // 处理 chunk
}
```

`StreamReader.Recv()` 返回的错误是通过 `StreamWriter.Send(chunk, err)` 从发送方 goroutine 传递的。当发送方 goroutine 中发生 panic 时，`StreamWriter` 的 recover 机制将 panic 信息包装为 error 通过 channel 传递。

### 6.3 中断与重运行

`ToolsNode` 支持一种特殊的错误语义——`InterruptRerunError`：

```
当工具返回 InterruptRerunError:
  ToolsNode 保存已执行结果 → 返回复合中断 → Graph Checkpoint
  重运行时:
    跳过已执行的工具 → 重新执行失败的工具
```

这不是组件接口级别的错误语义，而是 `ToolsNode` 执行层的特性。ChatModel 和 Retriever 本身不产生此错误类型。

### 6.4 对复刻版的影响

当前复刻版的错误处理是基本的：`composableRunnable.invoke()` 返回 `(any, error)`，图运行时在 `taskManager.execute()` 中通过 `task.err` 字段捕获。`EventLog` 记录 `EventNodeError`。

要实现组件级错误语义：

1. **基础错误传递**：已有，无需改动
2. **错误包装链**：建议定义 `ComponentError` 类型，包含 `Component`（"ChatModel"）、`Provider`（"OpenAI"）、`Original` 字段
3. **流错误传播**：当前复刻版 `PipeStreamWriter.Send(data, err)` 已支持错误传递，无需额外工作
4. **InterruptRerunError**：不在当前教育复刻版范围（需要完整的 Checkpoint/Recovery 机制）

---

## 7. 本复刻版的具体实现需求

### 7.1 实现清单（优先级排序）

以下清单按依赖顺序排列，标注了"必须"（MUST）和"可选"（SHOULD）：

#### Phase 1：Schema 类型（必须先于所有其他工作）

| # | 需求 | 优先级 | 文件位置 |
|---|------|--------|---------|
| 1 | 定义 `Message` struct（含 Role、Content、ToolCalls） | MUST | 新建 `compose/schema.go` 或 `schema/message.go` |
| 2 | 定义 `RoleType` 常量（User、Assistant、System、Tool） | MUST | 同上 |
| 3 | 定义 `ToolCall` struct（Index、ID、Type、Function） | MUST | 同上 |
| 4 | 定义 `Document` struct（ID、Content、Meta、Score） | MUST | 同上 |
| 5 | 定义 `ToolInfo` struct（Name、Desc、ParamsOneOf） | MUST | 同上 |
| 6 | 定义 `ParameterInfo`（用于轻量级参数 Schema） | SHOULD | 同上 |

#### Phase 2：组件接口

| # | 需求 | 优先级 | 细节 |
|---|------|--------|------|
| 7 | 定义 `BaseChatModel` 接口 | MUST | `Generate(ctx, []*Message, ...Option) (*Message, error)` + `Stream(...)` |
| 8 | 定义 `ChatModelOption`（双桶 Option 模式） | SHOULD | 公共 Options（Temperature, MaxTokens, Model） + implSpecificOptFn |
| 9 | 定义 `ToolCallingChatModel` 接口 | SHOULD | `WithTools(tools)` 返回新实例 |
| 10 | 定义 `Retriever` 接口 | MUST | `Retrieve(ctx, query, ...Option) ([]*Document, error)` |
| 11 | 定义 `RetrieverOption`（TopK, ScoreThreshold, Embedding） | SHOULD | — |
| 12 | 定义 `BaseTool` 接口 | SHOULD | `Info(ctx) (*ToolInfo, error)` |
| 13 | 定义 `InvokableTool` 接口 | SHOULD | `InvokableRun(ctx, argsJSON, ...Option) (string, error)` |

#### Phase 3：桥接适配器

| # | 需求 | 优先级 | 细节 |
|---|------|--------|------|
| 14 | 实现 `toChatModelNode(model BaseChatModel) composableRunnable` | MUST | 将 Generate → i，Stream → s |
| 15 | 实现 `toRetrieverNode(retriever Retriever) composableRunnable` | MUST | 将 Retrieve → i |
| 16 | 在 `GenericGraph` 上增加 `AddChatModelNode(key, model)` | MUST | 内部调用 toChatModelNode + addNode |
| 17 | 在 `GenericGraph` 上增加 `AddRetrieverNode(key, retriever)` | MUST | 内部调用 toRetrieverNode + addNode |
| 18 | 实现 `parseExecutorInfoFromComponent` | SHOULD | 提取 Typer / Checker 元数据 |

#### Phase 4：回调集成

| # | 需求 | 优先级 | 细节 |
|---|------|--------|------|
| 19 | 定义 ChatModel `CallbackInput`/`CallbackOutput` | SHOULD | `{Messages}` / `{Message}` |
| 20 | 定义 Retriever `CallbackInput`/`CallbackOutput` | SHOULD | `{Query}` / `{Docs}` |
| 21 | 定义 `ConvCallbackInput`/`ConvCallbackOutput` 转换函数 | SHOULD | 安全的类型断言 |
| 22 | 定义 `Typer` / `Checker` 接口 | SHOULD | `GetType()` / `IsCallbacksEnabled()` |
| 23 | 扩展 `RunInfo` 添加 `Type` / `Component` 字段 | SHOULD | 当前仅有 `Name` |
| 24 | 扩展 `CallbackWrapper` 以传递 RunInfo.Type / RunInfo.Component | SHOULD | — |

#### Phase 5：Chain / Workflow 扩展（低优先级）

| # | 需求 | 优先级 | 细节 |
|---|------|--------|------|
| 25 | `Chain.AppendChatModel(model)` | SHOULD | Builder 风格的 ChatModel 节点追加 |
| 26 | `Chain.AppendRetriever(retriever)` | SHOULD | Builder 风格的 Retriever 节点追加 |
| 27 | `WorkflowNode.AddChatModelNode(key, model)` | SHOULD | Workflow 风格的 ChatModel 节点附加 |

### 7.2 最小可行实现（MVP）路径

如果仅实现最核心的 bridge 以使图能够编排 ChatModel 和 Retriever，以下是最小子集：

```
必须实现:
  ├── compose/schema.go          ← Message, Document, ToolInfo
  ├── compose/model.go           ← BaseChatModel 接口
  ├── compose/retriever.go       ← Retriever 接口
  ├── compose/component_to_graph_node.go  ← toChatModelNode, toRetrieverNode
  └── compose/generic_graph.go (修改) ← AddChatModelNode, AddRetrieverNode

可选延后:
  ├── ToolCallingChatModel (WithTools)
  ├── Tool 体系 (BaseTool, InvokableTool, ToolsNode)
  ├── 组件回调 (CallbackInput/Output, Typer/Checker)
  ├── Chain/Workflow 扩展
  └── AgenticMessage
```

### 7.3 与现有代码的集成点

当前复刻版已有以下基础设施可复用：

| 现有能力 | 文件 | 如何复用 |
|---------|------|---------|
| `composableRunnable` | `compose/runnable.go` | `i` 函数指针接收 Generate/Retrieve 闭包；`s` 函数指针接收 Stream 闭包 |
| `graph.addNode` | `compose/graph.go` | 接收 `composableRunnable`，无需改动 |
| `InvokableLambda` | `compose/runnable.go` | 用户仍可用 Lambda 包装组件，但失去类型化桥接的好处 |
| `CallbackWrapper` | `compose/callbacks.go` | 在桥接适配器中包一层 CallbackWrapper（Phase 4） |
| `PipeStreamReader` | `compose/stream.go` | ChatModel.Stream 返回 PipeStreamReader[*Message] |
| `EventLog` | `compose/event_log.go` | 在桥接适配器中触发 EventNodeStart/End/Error |

### 7.4 实现约束

1. **不改变现有接口签名**：`Runnable[I, O]` 接口保持不变。新组件通过桥接适配器适配到现有 `composableRunnable` 内部表示。
2. **不破坏现有测试**：所有新增类型和接口在独立的 `.go` 文件中，不修改现有文件的核心逻辑。
3. **纯 Go 标准库**：Schema 类型和组件接口不导入任何外部库。
4. **零依赖运行时注入**：用户在构造时注入 ChatModel/Retriever 实现，而非通过全局注册表。
5. **与 Eino 设计意图一致**：
   - 使用 `WithTools` 返回新实例模式，不使用 `BindTools`
   - 选项使用双桶模式（虽然简化为单层）
   - 流所有权由调用方负责

### 7.5 与 Eino 完整实现的差异（不含入）

以下 Eino 能力**不在**本次教育复刻版的组件契约范围内：

| 排除项 | 理由 |
|--------|------|
| `AgenticMessage` + `AgenticModel` | 需要完整的 ContentBlock 类型系统 (~20 种块类型) + Provider 扩展槽位 |
| `*AgenticMessage` 的流式 concat 注册 | 依赖 Provider 特定的扩展合并函数 |
| `Indexer` / `Embedder` 组件 | 需要向量存储后端集成 |
| `ToolsNode` 完整实现（并行执行 + 增强工具输出） | 执行层面的复杂逻辑，可延后 |
| Provider 扩展槽位（OpenAIExtension 等） | 教学复刻版不绑定具体 provider |
| `ParamsOneOf` 双模式（轻量级 + JSON Schema） | 简化为仅轻量级 ParameterInfo 模式 |
| 序列化注册（`RegisterName[T]`） | 无 Checkpoint/Recovery，无需 gob 注册 |
| `ToolsNode.InterruptRerunError` | 需要完整的中断/恢复机制 |
| `ToolResult.ToMessageInputParts()` 多模态工具输出 | 需要 MessageInputPart 多模态类型系统 |
| `ServerToolCall` / `MCPToolCall` | 服务端工具搜索 / MCP 协议不在范围内 |
| 全局回调处理器 `AppendGlobalHandlers` | 当前复刻版无此机制 |
| 路径范围限定回调 `WithCallbacks(WithNodePath(...))` | 同上 |
| `isComponentCallbackEnabled` 防止重复触发 | 当仅有一层回调包装时不会出现重复 |

---

## 附录 A：关键 Eino 源码位置

| 概念 | Eino 源文件 | 行号 |
|------|-----------|------|
| `BaseModel[M]` 泛型接口 | `components/model/interface.go` | 36-71 |
| `BaseChatModel` 类型别名 | `components/model/interface.go` | 74 |
| `ChatModel` (已弃用) | `components/model/interface.go` | 80-85 |
| `ToolCallingChatModel` | `components/model/interface.go` | 99-103 |
| `AgenticModel` | `components/model/interface.go` | 109 |
| `Option` 双桶结构 | `components/model/option.go` | 64 |
| `WithTools` 选项函数 | `components/model/option.go` | 116 |
| `Retriever` 接口 | `components/retriever/interface.go` | 48 |
| `BaseTool` 接口 | `components/tool/interface.go` | 32 |
| `InvokableTool` 接口 | `components/tool/interface.go` | 42 |
| `EnhancedInvokableTool` | `components/tool/interface.go` | 67 |
| `ToolInfo` 结构 | `schema/tool.go` | 128 |
| `Message` 结构 | `schema/message.go` | 497 |
| `Document` 结构 | `schema/indexer.go` | — |
| `toChatModelNode` | `compose/component_to_graph_node.go` | 93 |
| `toRetrieverNode` | `compose/component_to_graph_node.go` | 151 |
| `toComponentNode` | `compose/component_to_graph_node.go` | 29 |
| `parseExecutorInfoFromComponent` | `compose/component_to_graph_node.go` | 151-163 |
| `Typer` / `Checker` 接口 | `components/types.go` | 29-52 |
| `ComponentOfChatModel` 常量 | `components/types.go` | 64-87 |
| ChatModel `CallbackInput/Output` | `components/model/callback_extra.go` | 66-80 |
| Retriever `CallbackInput/Output` | `components/retriever/callback_extra.go` | 25-41 |
| `ToolsNode` 接口优先级 | `compose/tool_node.go` | 830-838 |
| `ConcatMessages` | `schema/message.go` | 1643 |
| `ConcatAgenticMessages` | `schema/agentic_message.go` | 897 |
| `RegisterStreamChunkConcatFunc` | `internal/concat.go` | 71 |

## 附录 B：复刻版现有文件与新增文件对照

```
现有文件                             新增文件 (建议)
─────────                            ─────────
compose/runnable.go                  compose/schema.go            ← Message, Document, ToolInfo
compose/stream.go                    compose/model.go             ← BaseChatModel, Option
compose/callbacks.go                 compose/retriever.go         ← Retriever, RetrieverOption
compose/graph.go                     compose/tool.go              ← BaseTool, InvokableTool (可选)
compose/generic_graph.go (修改)      compose/component_to_graph_node.go  ← 桥接适配器
compose/graph_run.go                 compose/model_callback.go    ← ChatModel CallbackInput/Output (可选)
compose/types.go (修改)              compose/retriever_callback.go ← Retriever CallbackInput/Output (可选)
```

## 附录 C：简化版接口签名速查

```go
// === compose/schema.go ===

type RoleType string
const (
    User      RoleType = "user"
    Assistant RoleType = "assistant"
    System    RoleType = "system"
    Tool      RoleType = "tool"
)

type Message struct {
    Role      RoleType
    Content   string
    ToolCalls []ToolCall
    ToolCallID string
    ToolName   string
}

type ToolCall struct {
    Index    *int
    ID       string
    Type     string
    Function FunctionCall
}

type FunctionCall struct {
    Name      string
    Arguments string  // JSON string
}

type Document struct {
    ID      string
    Content string
    Score   float64
    Meta    map[string]any
}

type ToolInfo struct {
    Name        string
    Desc        string
    ParamsOneOf *ParamsOneOf
}

type ParamsOneOf struct {
    Params map[string]*ParameterInfo  // 简化: 仅轻量级模式
}

type ParameterInfo struct {
    Type     string                  // "string" | "number" | "boolean" | "object" | "array"
    Desc     string
    Required bool
    Enum     []string
    SubParams map[string]*ParameterInfo
}

// === compose/model.go ===

type BaseChatModel interface {
    Generate(ctx context.Context, input []*Message, opts ...ModelOption) (*Message, error)
    Stream(ctx context.Context, input []*Message, opts ...ModelOption) (*StreamReader[*Message], error)
}

type ModelOptions struct {
    Temperature *float32
    MaxTokens   *int
    Model       *string
    TopP        *float32
    Stop        []string
    Tools       []*ToolInfo
}

type ModelOption struct {
    apply            func(*ModelOptions)     // 公共选项
    implSpecificOptFn any                    // provider 特定 (双桶模式)
}

// === compose/retriever.go ===

type Retriever interface {
    Retrieve(ctx context.Context, query string, opts ...RetrieverOption) ([]*Document, error)
}

type RetrieverOptions struct {
    TopK          *int
    ScoreThreshold *float64
}

type RetrieverOption func(*RetrieverOptions)

// === compose/tool.go (可选) ===

type BaseTool interface {
    Info(ctx context.Context) (*ToolInfo, error)
}

type InvokableTool interface {
    BaseTool
    InvokableRun(ctx context.Context, argumentsInJSON string, opts ...ToolOption) (string, error)
}
```
