# 第六章：Schema / Provider 适配器互操作

## 1. 问题

Eino 是一个多 Provider 的 LLM 应用框架。用户组合一个 Graph，可能在同一个流水线中使用 OpenAI 进行 Chat Completion，使用 Claude 进行推理，使用 Gemini 进行 Embedding。每个 Provider 使用不同的线格式，有不同的 Message 结构、不同的流式协议和不同的响应元数据。

如果每个 Graph 节点都知道自己与哪个 Provider 通信，那么切换 Provider 就需要重写每个节点。如果 `compose/`（编排引擎）根据 Provider 名称进行分支，那么引擎就不再是通用的了。核心问题是：**如何让来自不同 Provider 的组件在同一个流水线中互操作，而没有任何组件知道其他组件的存在？**

Schema 层（`schema/`）通过定义一组规范数据模型来解决这个问题。Provider 适配器（在外部仓库 `eino-ext` 中）将其原生 SDK 类型转换为规范类型。组合引擎和 ADK（`compose/`、`adk/`）仅操作规范类型。关键的边界是组件接口——`BaseModel[M]` 通过 Message 类型 `M` 进行参数化，而 `M` 被封闭为 `*schema.Message` 和 `*schema.AgenticMessage`。Go 的类型系统在编译期捕获不匹配的情况。

## 2. 为什么困难

### 2.1 Message 是 Provider 各自发明的，没有标准

| 维度 | OpenAI | Claude | Gemini |
|-----------|--------|--------|--------|
| **角色名称** | `"assistant"` | `"assistant"` | `"model"` |
| **多模态部件** | `content: [{type:"text", text:"..."}, {type:"image_url", image_url:{...}}]` | `content: [{type:"text", text:"..."}, {type:"image", source:{...}}]` | `parts: [{text:"..."}, {inlineData:{...}}]` |
| **工具调用** | `tool_calls[]`，带有基于索引的流式 delta 块 | `tool_use` 内容块，内嵌在 Message 内容中 | `functionCall`，内嵌在 `parts[]` 中 |
| **工具结果** | 角色为 `"tool"` 的 Message，带有 `tool_call_id` | `user` Message 中的 `tool_result` 内容块 | `user` 角色中的 `functionResponse` 部件 |
| **推理** | 用量详情中的 `reasoning_tokens` | Thinking 内容块 | `thought` 部件 |
| **响应 ID** | `response.id` | `message.id` | `candidates[0].content.parts` 联合体 |

朴素地将某个 Provider 的格式选作内部 Schema 会造成锁定。一个规范 Schema 必须能容纳所有这些格式，而不偏向任何一种。

### 2.2 工具参数 Schema 因 Provider 而异

有些模型接受扁平的 `properties` 对象。另一些则需要完整的 JSON Schema，包含 `anyOf`、`oneOf`、`$defs`。有些 Provider 使用服务端工具搜索，工具是动态发现的，而不是预先定义的。一个单一的参数 Schema 表示必须能够转换为每个模型 API 所期望的格式。

### 2.3 流式块以不同方式合并

文本块：简单的字符串拼接。工具调用块：按索引合并，拼接 JSON 片段参数。推理块：累积叠加。图像/音频/视频块：不可合并（每个都是独立的产物）。框架必须注册类型特定的 concat 函数，并在运行时通过 Go 泛型派发到这些函数。

### 2.4 Provider 扩展不得泄露到通用代码中

Provider A 有注解（OpenAI）。Provider B 有四种位置类型的引用（Claude）。Provider C 有包含搜索入口点的 Grounding 元数据（Gemini）。通用的 Graph 节点应该对这些一无所知。然而，专门的组件（如 RAG 评估器）必须能访问它们。Schema 必须携带 Provider 数据，而不强制每个消费者进行类型断言。

### 2.5 序列化必须在 Graph 中断/恢复时存活

当 Graph Checkpoint（挂起）并恢复时，中间状态——Message、工具调用、多模态部件、扩展元数据——必须经受住通过 `encoding/gob` 的往返。每个在状态中使用的规范类型都必须预先注册。具有 interface 字段或递归结构的类型需要自定义 `GobEncode`/`GobDecode`。

## 3. 设计思想

Eino 将关注点分离为三个层次：

```
Provider 适配器 (eino-ext)          将原生类型转换为规范类型
    │ 实现
组件接口 (components/)              通用合约 (BaseModel[M])
    │ 使用类型
规范 Schema (schema/)                类型 (Message, AgenticMessage, StreamReader, ToolInfo)
    │ 包含
Provider 扩展 (schema/openai,        规范类型上的可选类型化槽位
 schema/claude, schema/gemini)
```

关键设计决策：

1. **两种 Message 模型，而非一种。**
   - `Message` (`schema/message.go:497`)：经典文本 + `ToolCalls[]` 模型。向后兼容，基于 Channel 的多模态输入/输出。由 `BaseChatModel` 使用。
   - `AgenticMessage` (`schema/agentic_message.go:71`)：基于 ContentBlock 的模型，具有更丰富的类型系统。区分 FunctionToolCall、ServerToolCall、MCPToolCall、工具搜索、审批流。具有类型化的 Provider 扩展槽位。由 `AgenticModel` 使用。

2. **Provider 扩展是数据类型，不是实现。**
   `schema/` 下的每个 Provider 目录定义结构体，通过类型化指针字段嵌入规范类型——绝不使用 `map[string]any`。不关心 Provider 数据的组件只需忽略 nil 指针。关心 Provider 数据的组件可以进行类型断言。

3. **泛型接口强制类型安全。**
   `BaseModel[M messageType]` (`components/model/interface.go:36`) 只接受 `*Message` 和 `*AgenticMessage`。你不能通过框架传递原始的 `map` 或任意结构体。Go 编译器强制执行这一点。

4. **StreamReader[T] 作为通用流式基元。**
   `schema/stream.go:168`——不是一个简单的 Channel 包装器。支持数组后端（`StreamReaderFromArray` 零开销）、通过 `Copy(n)` 扇出、通过 `MergeStreamReaders` 扇入，以及通过 `StreamReaderWithConvert` 进行类型安全转换。Provider 适配器将其原生 SDK 流转换为 `StreamReader[*Message]` 或 `StreamReader[*AgenticMessage]`。

5. **注册式 Concat 派发。**
   `internal.RegisterStreamChunkConcatFunc[T]` (`internal/concat.go:71`) 构建一个按类型索引的派发表。当 `compose/` 需要合并一个流时，它调用 `internal.ConcatItems[T]`，后者查找为 `T` 注册的 concat 函数。这就是 `ConcatMessages` 和 `ConcatAgenticMessages`（它们自身调用 Provider 特定的扩展合并逻辑）如何接入通用流合并路径的。

## 4. 源码走读

### 4.1 `Message`——经典模型 (`schema/message.go`)

```go
// schema/message.go:497
type Message struct {
    Role             RoleType              // Assistant | User | System | Tool
    Content          string                // 纯文本
    UserInputMultiContent []MessageInputPart   // 来自用户的多模态输入
    AssistantGenMultiContent []MessageOutputPart // 来自模型的多模态输出
    ToolCalls        []ToolCall            // assistant：请求的工具调用
    ToolCallID       string                // tool：此响应对应哪个调用
    ToolName         string                // tool：响应工具的名称
    ResponseMeta     *ResponseMeta         // finish_reason, usage, logprobs
    ReasoningContent string                // 思考内容
    Extra            map[string]any        // 遗留的 Provider 特定数据袋
}
```

**ToolCall** (`schema/message.go:132`)：`{Index int, ID, Type, Function{Name, Arguments string}, Extra}`。`Index` 对流式处理至关重要——具有相同 `Index` 的 delta 块属于同一个工具调用。`Arguments` 跨块累积 JSON 片段。

**MessageInputPart** (`schema/message.go:207`)：通过 `Type` 判别符实现的类型联合——`Text`、`ImageURL`、`AudioURL`、`VideoURL`、`FileURL`、`ToolSearchResult`。

**MessageOutputPart** (`schema/message.go:268`)：模型输出的类比——`Text`、`Image`、`Audio`、`Video`、`Reasoning`。

### 4.2 `AgenticMessage`——ContentBlock 模型 (`schema/agentic_message.go`)

```go
// schema/agentic_message.go:71
type AgenticMessage struct {
    Role         AgenticRoleType             // system | user | assistant（没有 "tool" 角色）
    ContentBlocks []*ContentBlock            // 类型化块的有序列表
    ResponseMeta *AgenticResponseMeta        // token 用量 + provider 扩展
    Extra        map[string]any
}
```

**ContentBlock** (`schema/agentic_message.go:102`)：标记联合体，包含约 20 种变体，每种存储为可空指针。关键创新是没有单独的 "tool result" 角色——工具调用和工具结果都是同一 Message 中的内容块：

- 输入块：`UserInputText`、`UserInputImage`、`UserInputAudio`、`UserInputVideo`、`UserInputFile`、`ToolSearchResult`
- 输出块：`AssistantGenText`、`AssistantGenImage`、`AssistantGenAudio`、`AssistantGenVideo`、`Reasoning`
- 工具调用块：`FunctionToolCall`、`ServerToolCall`、`MCPToolCall`
- 工具结果块：`FunctionToolResult`、`ServerToolResult`、`MCPToolResult`
- MCP 协议块：`MCPListToolsResult`、`MCPToolApprovalRequest`、`MCPToolApprovalResponse`

**StreamingMeta** (`schema/agentic_message.go:174`)：`{Index int}`。每个流式块在其 `ContentBlock.StreamingMeta` 中携带一个 `Index`。拼接时（在 `ConcatAgenticMessages` 中，第 897 行），块按索引分组，并通过类型特定的函数合并。

**AgenticResponseMeta** (`schema/agentic_message.go:85`)：携带 `TokenUsage` 以及类型化的 Provider 扩展槽位：

```go
OpenAIExtension *openai.ResponseMetaExtension
GeminiExtension *gemini.ResponseMetaExtension
ClaudeExtension  *claude.ResponseMetaExtension
Extension       any  // 未知/自定义 provider 的回退
```

**AssistantGenText** (`schema/agentic_message.go:234`)：也具有 `OpenAIExtension *openai.AssistantGenTextExtension` 和 `ClaudeExtension *claude.AssistantGenTextExtension`，用于每个文本块的注解/引用。

### 4.3 `ToolInfo`——双模式参数 Schema (`schema/tool.go`)

`ToolInfo` (`schema/tool.go:128`) 向模型描述一个工具。关键字段是 `*ParamsOneOf`，它恰好是以下两种模式之一：

1. **`NewParamsOneOfByParams(map[string]*ParameterInfo)`** (第 283 行)：轻量级。一个扁平的 `ParameterInfo{Type, ElemInfo, SubParams, Desc, Enum, Required}` 映射。支持递归的 `ParameterInfo` 用于嵌套对象和数组。

2. **`NewParamsOneOfByJSONSchema(*jsonschema.Schema)`** (第 290 行)：完整的 JSON Schema 2020-12。由 `utils.InferTool` 使用，该函数从 Go 结构体标签自动生成 Schema。对于 `anyOf`、`oneOf`、`$defs` 是必需的。

转换：`ParamsOneOf.ToJSONSchema()` (`tool.go:297`) 将两种模式标准化为 `*jsonschema.Schema`，以便传递给模型 API。大多数 Provider 适配器在为其原生 API 编组工具 Schema 之前调用此方法。

### 4.4 `StreamReader[T]`——通用流式 (`schema/stream.go`)

`StreamReader[T]` (`schema/stream.go:168`) 是一个多态读取器，具有五种内部后端：

```
readerTypeStream   → 基于 Channel (Pipe)
readerTypeArray    → 基于切片 (StreamReaderFromArray)
readerTypeMultiStream  → 扇入 (MergeStreamReaders)
readerTypeWithConvert  → 逐元素转换 (StreamReaderWithConvert)
readerTypeChild   → 扇出 (Copy)
```

关键操作：

- **`Pipe[T](cap)`** (第 99 行)：创建一对配对的 `StreamReader` + `StreamWriter`。一个发送者 goroutine 调用 `sw.Send(chunk, err)`，一个接收者调用 `sr.Recv()`。Close 发出 `io.EOF` 信号。

- **`StreamReaderFromArray[T](arr)`** (第 461 行)：由切片支持的零开销读取器。`Recv()` 按顺序返回元素，然后返回 `io.EOF`。

- **`Copy(n int)`** (第 261 行)：扇出——使用链表共享缓冲区创建 `n` 个独立的子读取器。每个子读取器必须独立关闭。当流馈送给多个消费者（回调处理器 + 下游节点）时使用。

- **`MergeStreamReaders[T](srs)`** (第 912 行)：扇入——将多个流交错合并为一个。来自所有源的数据块按到达顺序到达，而不是源顺序。

- **`MergeNamedStreamReaders[T](srs, names)`** (第 990 行)：带有源标识的扇入。当单个流结束时，发出带有源名称的 `SourceEOF` 错误，以便消费者跟踪每个源的完成情况。

- **`StreamReaderWithConvert[T,D](sr, convert)`** (第 691 行)：逐元素转换。`convert` 函数将 `T → (D, error)` 映射。返回 `ErrNoValue` 以从流中过滤掉某个元素。

**为什么多态重要：**如果 `StreamReader` 总是基于 Channel 的，那么 `StreamReaderFromArray` 将需要一个 goroutine 来推送元素。有了多个后端，运行时会选择最优路径。组合层（`compose/stream_reader.go`）用内部的 `streamReader` 接口包装了这一点，该接口添加了 `copy`、`merge`、`mergeWithNames`、`withKey` 和 `toAnyStreamReader`。

### 4.5 流拼接 (`schema/message.go`)

当流产出部分块时，框架必须将它们合并成完整的 Message。这是按类型注册一次的：

```go
// schema/message.go:39-46
func init() {
    internal.RegisterStreamChunkConcatFunc(ConcatMessages)
    internal.RegisterStreamChunkConcatFunc(ConcatMessageArray)
    internal.RegisterStreamChunkConcatFunc(ConcatAgenticMessages)
    internal.RegisterStreamChunkConcatFunc(ConcatAgenticMessagesArray)
    internal.RegisterStreamChunkConcatFunc(ConcatToolResults)
}
```

`ConcatMessages` (`schema/message.go:1643`)：
- 拼接 `Content` 字符串（纯文本累积）。
- 拼接 `ReasoningContent`。
- 通过 `concatToolCalls`（第 1283 行）合并 `ToolCalls`：按 `Index` 分组块，验证每个组内一致的 ID/Type/Name，拼接 `Arguments` JSON 片段，按索引排序最终调用。
- 通过 `concatAssistantMultiContent` / `concatUserMultiContent` 合并多模态内容。
- 保留最后一个非 nil 的 `ResponseMeta`（finish reason、usage 在最后到达）。

`ConcatAgenticMessages` (`schema/agentic_message.go:897`)：按 `StreamingMeta.Index` 对 `ContentBlock` 分组。每组通过类型特定的函数拼接（`concatAssistantGenTexts`、`concatFunctionToolCalls` 等）。Provider 扩展槽位通过 `concatAgenticResponseMeta`（第 1002 行）合并，后者调用每个 Provider 的辅助函数：`openai.ConcatResponseMetaExtensions`、`claude.ConcatResponseMetaExtensions`、`gemini.ConcatResponseMetaExtensions`。对于 `Extension any` 回退，它使用 `internal.ConcatSliceValue`（运行时类型断言 + append）。

### 4.6 Provider 扩展 (`schema/openai/`、`schema/claude/`、`schema/gemini/`)

**OpenAI (`schema/openai/extension.go`)：**

```go
type ResponseMetaExtension struct {
    ID, Status, PreviousResponseID string
    Error             *ResponseError
    IncompleteDetails *IncompleteDetails
    Reasoning         *Reasoning       // effort, summary
    ServiceTier       ServiceTier       // scale, default
    CreatedAt         int64
    PromptCacheRetention PromptCacheRetention
}

type AssistantGenTextExtension struct {
    Refusal     *OutputRefusal      // 内容过滤器拒绝原因
    Annotations []*TextAnnotation    // 文件引用、URL 引用
}
```

`ConcatAssistantGenTextExtensions` (第 116 行)：通过按 `Index` 去重合并注解，拼接拒绝原因。`TextAnnotation` (第 59 行) 有四种位置类型：`FileCitation`、`URLCitation`、`FilePath`（带有文件 ID）和 `ContainerFileCitation`（带有字符偏移量）。

**Claude (`schema/claude/extension.go`)：**

```go
type ResponseMetaExtension struct {
    ID, StopReason, StopSequence string
    StopDetails  *StopDetails     // category, explanation
}

type AssistantGenTextExtension struct {
    Citations []*TextCitation
}
```

`TextCitation` (第 39 行)：`CitationCharLocation`、`CitationPageLocation`、`CitationContentBlockLocation`、`CitationWebSearchResultLocation` 的类型化联合。每种都携带 `CitedText`、`DocumentTitle`、`DocumentIndex`。引用位置以字符偏移量、页码、内容块索引或网页搜索结果索引表示。

`ConcatAssistantGenTextExtensions` (第 88 行)：简单地追加来自所有块的引用——引用通常出现在最终块中，而不是每个 delta。

**Gemini (`schema/gemini/extension.go`)：**

```go
type ResponseMetaExtension struct {
    ID, FinishReason string
    GroundingMeta    *GroundingMetadata
}

type GroundingMetadata struct {
    GroundingChunks   []*GroundingChunk   // 网页来源 (domain, title, URI)
    GroundingSupports []*GroundingSupport  // 置信度分数、段落信息
    SearchEntryPoint  *SearchEntryPoint    // 渲染的内容、SDK blob
    WebSearchQueries []string              // 后续搜索查询
}
```

### 4.7 序列化 (`schema/serialization.go`)

Graph Checkpoint 持久化要求中间状态中使用的每种类型都通过 `encoding/gob` 和自定义序列化器预先注册：

```go
// schema/serialization.go:27-56
func init() {
    RegisterName[*Message]("_eino_message")
    RegisterName[*AgenticMessage]("_eino_agentic_message")
    RegisterName[ToolCall]("_eino_tool_call")
    RegisterName[ResponseMeta]("_eino_response_meta")
    RegisterName[TokenUsage]("_eino_token_usage")
    // ... 约 20 个其他类型注册
}
```

`RegisterName[T](name)` (`schema/serialization.go:83`)：调用 `gob.RegisterName` 和 `serialization.GenericRegister[T]`。`GenericRegister` 构建一个 `reflect.Type → name` 映射，Checkpoint 存储使用该映射将序列化的状态解码回正确的具体类型。

具有复杂内部结构的类型实现自定义 gob 编解码器。例如，`ToolInfo` (`tool.go:194`)：它的 `ParamsOneOf` 联合体（要么是 `map[string]*ParameterInfo` 要么是 `*jsonschema.Schema`）通过 `toolInfoForGob` 序列化，后者将 Schema 分支 JSON 编码为一个字符串字段。

### 4.8 组件到 Schema 的桥接

`compose/component_to_graph_node.go` 将组件接口转换为 Graph 节点。每个 `to*Node` 函数将组件的方法包装为 `composableRunnable`。关键函数 `parseExecutorInfoFromComponent`（大约第 50 行）：检查组件是否实现 `Typer`（`GetType()`）和 `Checker`（`IsCallbacksEnabled()`），提取 Graph 运行时用于决定何时触发回调的元数据。

Graph 调度器从不直接看到组件类型——它看到的是 `composableRunnable`，其 `inputType` 和 `outputType` 字段类型化为 `*schema.Message` 或 `*schema.AgenticMessage`。这种抽象就是为什么添加一个新的 Provider 模型只需要实现 `BaseModel[M]`——不需要更改 Graph。

## 5. 模式和示例

### 5.1 构建多模态用户 Message

```go
msg := &schema.Message{
    Role: schema.User,
    UserInputMultiContent: []schema.MessageInputPart{
        {Type: schema.ChatMessagePartTypeText, Text: "Describe this image:"},
        {Type: schema.ChatMessagePartTypeImageURL,
         Image: &schema.MessageInputImage{
             MessagePartCommon: schema.MessagePartCommon{URL: "https://example.com/photo.png"},
         }},
    },
}
```

### 5.2 构建 Agentic 工具 Message

```go
// 请求工具调用的 Assistant Message
assistantMsg := &schema.AgenticMessage{
    Role: schema.AgenticRoleAssistant,
    ContentBlocks: []*schema.ContentBlock{{
        Type: schema.ContentBlockTypeFunctionToolCall,
        FunctionToolCall: &schema.FunctionToolCall{
            Name: "get_weather",
            Arguments: `{"city":"Beijing"}`,
        },
    }},
}

// 携带工具结果的 User Message（注意：没有单独的 "tool" 角色）
toolResultMsg := &schema.AgenticMessage{
    Role: schema.AgenticRoleUser,
    ContentBlocks: []*schema.ContentBlock{{
        Type: schema.ContentBlockTypeFunctionToolResult,
        FunctionToolResult: &schema.FunctionToolResult{
            CallID: assistantMsg.ContentBlocks[0].FunctionToolCall.CallID,
            Output: `{"temperature": 22, "condition": "sunny"}`,
        },
    }},
}
```

Agentic Message 与经典 `Message` 不同：工具结果是 `user` 角色 Message 中的内容块，而不是单独的 `tool` 角色 Message。这更自然地映射到像 Claude 和 Gemini 这样的 Provider（工具结果是 user 回合的内容）以及 OpenAI 的 Responses API（工具结果是内联的）。

### 5.3 定义具有嵌套参数的工具

```go
tool := &schema.ToolInfo{
    Name: "search_documents",
    Desc: "Search a document corpus",
    ParamsOneOf: schema.NewParamsOneOfByParams(map[string]*schema.ParameterInfo{
        "query": {Type: schema.String, Desc: "Search query", Required: true},
        "filters": {
            Type: schema.Object,
            SubParams: map[string]*schema.ParameterInfo{
                "date_range": {
                    Type: schema.Object,
                    SubParams: map[string]*schema.ParameterInfo{
                        "start": {Type: schema.String, Desc: "Start date (YYYY-MM-DD)"},
                        "end":   {Type: schema.String, Desc: "End date (YYYY-MM-DD)"},
                    },
                },
                "author": {Type: schema.String},
            },
        },
    }),
}
```

对于具有 `anyOf`/`oneOf` 的复杂 Schema，使用 `NewParamsOneOfByJSONSchema`：

```go
schema, _ := jsonschema.NewSchemaFromFile("search_schema.json")
tool := &schema.ToolInfo{
    Name: "search",
    Desc: "Advanced search",
    ParamsOneOf: schema.NewParamsOneOfByJSONSchema(schema),
}
```

### 5.4 将 Pipe 流转换为数组以进行测试

```go
sr, sw := schema.Pipe[*schema.Message](3)
go func() {
    defer sw.Close()
    sw.Send(&schema.Message{Role: schema.Assistant, Content: "Hello"}, nil)
    sw.Send(&schema.Message{Role: schema.Assistant, Content: " World"}, nil)
}()

// 将流拼接为完整的 Message
arraysr, _ := schema.StreamReaderWithConvert(sr,
    func(v *schema.Message) (*schema.Message, error) {
        return v, nil
    })
// 或收集所有块：
var chunks []*schema.Message
for {
    chunk, err := sr.Recv()
    if errors.Is(err, io.EOF) { break }
    chunks = append(chunks, chunk)
}
complete, _ := schema.ConcatMessages(chunks)
// complete.Content == "Hello World"
```

### 5.5 使用 Copy 进行扇出以用于回调观察

```go
sr, _ := model.Stream(ctx, messages)  // 来自模型的单一流
children := sr.Copy(3)                // 原始 + 2 个回调副本
defer children[0].Close()
defer children[1].Close()
defer children[2].Close()

// children[0] → 下游 Graph 节点
// children[1] → 计时回调处理器
// children[2] → 日志回调处理器
```

### 5.6 访问 Provider 扩展元数据

```go
resp, _ := agenticModel.Generate(ctx, msgs)

if resp.ResponseMeta != nil {
    // 检查 OpenAI 特定的元数据
    if oe := resp.ResponseMeta.OpenAIExtension; oe != nil {
        fmt.Printf("OpenAI response ID: %s, Service tier: %v\n",
            oe.ID, oe.ServiceTier)
    }

    // 检查 Claude 特定的元数据
    if ce := resp.ResponseMeta.ClaudeExtension; ce != nil {
        fmt.Printf("Claude stop reason: %s\n", ce.StopReason)
    }

    // 检查 Gemini Grounding 元数据
    if ge := resp.ResponseMeta.GeminiExtension; ge != nil {
        if gm := ge.GroundingMeta; gm != nil {
            for _, ch := range gm.GroundingChunks {
                fmt.Printf("Grounded on: %s (%s)\n", ch.Web.Title, ch.Web.URI)
            }
        }
    }
}

// 访问每个文本块的注解 (OpenAI)
for _, block := range resp.ContentBlocks {
    if text := block.AssistantGenText; text != nil {
        if oe := text.OpenAIExtension; oe != nil {
            for _, ann := range oe.Annotations {
                fmt.Printf("Annotation at index %d\n", ann.Index)
            }
        }
    }
}
```

### 5.7 为 Checkpoint 持久化注册自定义类型

```go
// 在你的组件包 init() 中
func init() {
    schema.RegisterName[*MyState]("_myapp_state")
    schema.RegisterName[MyCustomToolResult]("_myapp_tool_result")
}

// MyState 现在将在 Graph 中断/恢复后存活
```

## 6. 常见陷阱

### 6.1 混淆 Message 模型的选择

对于使用 Function Calling（工具）的经典 Chat 应用，使用 `Message` + `BaseChatModel`。对于需要 MCP 工具、服务端工具、工具搜索或结构化多模态输出的 Agent 应用，使用 `AgenticMessage` + `AgenticModel`。混合使用它们——将 `*Message` 传递给 `AgenticModel`——将因 `BaseModel[M]` 的类型约束而导致编译错误。

### 6.2 未关闭 Stream 副本

`StreamReader.Copy(n)` 创建 `n` 个由共享缓冲区支持的独立子读取器。每个子读取器在消费后必须调用 `Close()`。如果有一个子读取器泄漏，父级的基础 goroutine 永远不会终止。这是 Eino Graph 中最常见的 goroutine 泄漏。`SetAutomaticClose()`（第 279 行）有所帮助，但依赖垃圾回收，而非确定性清理。

### 6.3 假设流合并保持顺序

`MergeStreamReaders` 按到达顺序交错来自所有源的数据块。如果你需要按源排序（例如，先拼接源 A 的数据块，再拼接源 B 的数据块），请使用 `MergeNamedStreamReaders` 并跟踪 `SourceEOF` 错误，或分别收集每个源的数据并进行拼接。

### 6.4 依赖 `Extra` 而非扩展槽位

`Message.Extra` (`map[string]any`) 存在，但绕过了类型安全。如果你将 Provider 特定数据存储在 `Extra` 中，下游代码必须发现是哪个 Provider 生成的，并对每个值进行类型断言。请使用 `AgenticResponseMeta` 和 `AssistantGenText` 上的类型化扩展槽位（`OpenAIExtension`、`ClaudeExtension`、`GeminiExtension`）——框架的 concat 函数能够理解它们；而 `map[string]any` 的合并是最后写入胜出。

### 6.5 缺少序列化注册

如果你的 Graph 在状态中使用了自定义类型（例如，用户定义的聚合器结构体），并且你在启用 Checkpoint 时没有在 `init()` 中调用 `schema.RegisterName[T]("_name")`，Checkpoint 存储将在编码/解码时报出晦涩的 gob 错误。每个跨过中断/恢复边界的类型都必须预先注册。

### 6.6 混合 ParamsOneOf 模式

一个 `ToolInfo` 只能有一种 `ParamsOneOf` 模式——要么 `params` 要么 `jsonschema`，不能两者兼有。如果你用 `NewParamsOneOfByParams` 构造，然后用 `NewParamsOneOfByJSONSchema` 覆盖 `ParamsOneOf` 字段，两个指针都存在于结构体中，但 `ToJSONSchema()` 首先检查 `p.params != nil`（第 302 行），所以 JSON Schema 分支会被静默忽略。

### 6.7 流式工具调用缺少 Index

在流式处理中，`ToolCall.Index` 标识一个 delta 块属于哪个工具调用。如果 Provider 适配器为每个工具调用的每个块都设置 `Index = 0`，`concatToolCalls` 会将所有块合并为一个（无效的）调用。确保每个不同的工具调用获得唯一的 `Index`，并在整个流中正确递增。

## 7. Rive 可以借鉴的地方

### 7.1 规范 Schema 作为集成面

Eino 的 `schema/` 包是组件之间流动的所有数据类型的单一真实来源。`eino-ext` 中的 Provider 包依赖 `schema/`（而不是反过来）。这是应用于数据的依赖倒置原则：高层模块定义数据类型；低层实现遵循它们。Rive 的插件系统应该定义一个插件导入的规范 Schema 包——而不是每个插件一个数据格式。

### 7.2 类型化扩展槽位优于通用 Map

`ResponseMeta.OpenAIExtension *openai.ResponseMetaExtension` 模式（nil = 不存在）严格优于 `Extra map[string]any`。它给 concat 函数提供了一个类型化合约（它们确切知道如何合并），给编译器提供了可检查的内容，给 IDE 自动补全提供了具体字段。Rive 应该优先使用类型化可选结构体字段，而不是通用扩展袋来存储插件特定数据。

### 7.3 泛型的类型约束封闭

`type messageType interface { *schema.Message | *schema.AgenticMessage }` 使用 Go 1.18+ 的联合约束将 `BaseModel[M]` 封闭为恰好两种具体类型。这防止第三方通过框架传递 `BaseModel[MyType]`，后者会破坏 Graph 编译。Rive 可以使用类似的联合约束来封闭自己的泛型执行接口。

### 7.4 注册式派发表用于可扩展性

`internal.RegisterStreamChunkConcatFunc[T]` 构建一个 `reflect.Type → func` 映射。当组合层遇到类型为 `T` 的流时，它调用 `internal.ConcatItems[T]`，后者派发到已注册的 concat 函数——而无需知道 `T` 是什么。这是 Go 泛型中等价于插件注册表的模式。Rive 可以在其自身的可扩展操作（序列化、验证、合并）中使用此模式，这些操作需要新类型注册处理器而无需修改核心引擎。

### 7.5 流式抽象多态

`StreamReader[T]` 在 Channel、数组、多流和转换后端之间切换，比简单的 `chan` 更为复杂。组合层的内部 `streamReader` 接口进一步添加了 copy/merge/withKey。Rive 的流式基元应同样隐藏后端差异——使 `StreamReader` 无论是来自活跃的 goroutine、预计算的数组还是多个源的合并，行为都保持一致。

### 7.6 双向 Message 模型兼容性

同时拥有 `Message` 和 `AgenticMessage` 创建了一条迁移路径：基于 `BaseChatModel` 构建的现有 Graph 继续工作；新的 Graph 采用语义更丰富的 `AgenticModel`。两种模型共存是因为它们共享 `messageType` 约束。Rive 应该设计其数据模型演进时具有类似的优雅共存——而不是破坏性迁移。
