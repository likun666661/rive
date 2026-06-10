# Chapter 05 - Components / Model / Tool / Prompt 深度讲解

面向读者：假设你已经读过前四章，知道 Graph / Workflow / Chain 最终都会编译成 `Runnable`，也知道运行时靠 `invoke` / `stream` / `collect` / `transform` 四种执行形态统一调度节点。

这一章要回答的问题是：

```text
Graph 只认识 Runnable，为什么 ChatModel、Retriever、Prompt、Tool 这些领域对象也能放进图？
ChatModel 不是普通 Lambda，它有 Generate / Stream；Prompt 是 Format；Tool 是 Execute / Invoke；Retriever 是 Retrieve。
这些形状不同的接口，怎样被统一成运行时能调度的节点？
为什么要把 Bridge Adapter 和 Component 包装分开讲？
ToolCall 从模型消息里出来以后，ToolsNode 到底做了什么？
```

参考代码位置：

- 手册：`examples/eino-technical-manual/manual/05-components-model-tool-prompt.md`
- 大纲：`examples/eino-technical-manual/manual/teaching-manual-outline.md`
- 复刻版：`examples/eino-compose-runtime-replica-go`
- 本章重点源码：
  - `compose/bridge.go`
  - `compose/chatmodel.go`
  - `compose/prompt.go`
  - `compose/prompt_tool_bridge.go`
  - `compose/state.go`
  - `compose/retriever.go`
  - `compose/schema.go`
  - `compose/generic_graph.go`
  - `compose/bridge_test.go`
  - `compose/chatmodel_test.go`
  - `compose/prompt_test.go`
  - `compose/prompt_tool_bridge_test.go`
  - `compose/retriever_test.go`
  - `compose/schema_test.go`

先说明边界：原始 Eino 的组件层更完整，包含独立的 `components/model`、`components/tool`、`components/prompt`、`components/retriever` 等包，接口里有 provider 选项、回调 extra、工具 schema、增强工具、多模态结果、`WithTools` 等能力。当前 Go 复刻版是教学实现，没有完整复刻所有 provider 选项与增强工具层级。本章以当前复刻版为准，同时会在关键处说明它与原版 Eino 的差异。

## 1. 为什么第五章很关键

前四章主要在讲运行时本身。

你可以把它们概括成：

```text
Ch01：Graph 如何表达 DAG / Pregel 拓扑，并编译为 Runnable
Ch02：Workflow / Chain 怎样降低编排门槛，FieldMapping 怎样搬运字段
Ch03：Runnable 如何统一 Invoke / Stream / Collect / Transform，Callback 怎样插入观测
Ch04：Interrupt / Resume / Checkpoint 怎样让运行暂停后恢复
```

这些内容解决的是“怎么调度”的问题。

第五章开始切到另一个问题：

```text
调度器到底调度什么？
```

如果所有节点都是匿名 Lambda，当然也能跑：

```go
g.AddLambdaNode("prompt", InvokableLambda(func(ctx context.Context, in map[string]any) ([]*Message, error) {
    // assemble prompt
}))

g.AddLambdaNode("model", InvokableLambda(func(ctx context.Context, msgs []*Message) (*Message, error) {
    // call model
}))

g.AddLambdaNode("tools", InvokableLambda(func(ctx context.Context, msg *Message) (*Message, error) {
    // execute tools
}))
```

但这样写久了会出三个问题。

第一，领域语义丢失。

运行时只看到三个 Lambda，它不知道哪个是 Prompt，哪个是 ChatModel，哪个是 ToolsNode。回调、日志、调试输出、测试命名都会变得含糊。

第二，重复样板太多。

每个项目都会手写一遍：

```text
ChatModel.Generate -> Lambda.Invoke
ChatModel.Stream   -> Lambda.Stream
Prompt.Format      -> Lambda.Invoke
Tool.Execute       -> Lambda.Invoke
Retriever.Retrieve -> Lambda.Invoke
```

这些包装逻辑本质相同，不应该散落在业务代码里。

第三，替换 provider 困难。

如果业务逻辑里直接写 OpenAI、Claude、Gemini 的请求结构，那么换 provider 时不仅要改模型节点，还要改上下游消息、工具调用和提示词拼装逻辑。组件层的目的就是把这些差异关在边界内。

所以第五章的核心不是“多几个接口”，而是：

```text
用最小领域接口表达能力，再用 Bridge Adapter 把能力接入通用 Runnable 运行时。
```

## 2. 一句话总览

当前复刻版里有两层桥接：

```text
教学层 Bridge Adapter
  BridgeRetriever / BridgeChatModel / promptAssemblerBridge / BridgeTool
  重点：演示“领域接口 -> Lambda”的设计模式

组件层包装
  ChatModelComponent / ChatTemplateComponent / ToolsNode / NewRetrieverLambda
  重点：更接近真实框架的“组件 -> composableRunnable”
```

可以画成这样：

```text
业务领域对象
    |
    | 领域接口
    v
Bridge / Component 包装
    |
    | toLambda() / GetRunnable()
    v
composableRunnable
    |
    | graph.addNode / Workflow.AddLambdaNode / Chain.AppendLambda
    v
Graph Runtime
```

运行时不需要知道 OpenAI、Claude、Redis、FAISS，也不需要知道这个对象叫 Prompt 还是 Tool。它只需要拿到一个 `composableRunnable`。

反过来，组件开发者也不需要理解 Pregel 调度细节。只要实现：

```text
ChatModel.Generate / Stream
ChatTemplate.Format
Retriever.Retrieve
Tool.Invoke / Execute
```

框架就能把它变成图节点。

## 3. 本章的四个主角

这一章标题里有四个词：Model、Tool、Prompt、Components。

在当前复刻版中，它们分别对应这些文件。

| 概念 | 当前复刻版入口 | 最小契约 | 接入图的方式 |
| --- | --- | --- | --- |
| ChatModel | `compose/chatmodel.go` | `Generate(ctx, []*Message)` + `Stream(ctx, []*Message)` | `NewChatModelComponent(cm).GetRunnable()` 或 `chatModelBridge.toLambda()` |
| Prompt | `compose/prompt.go` | `Format(ctx, map[string]any)` | `NewChatTemplateComponent(ct).GetRunnable()` 或 `NewPromptTemplateLambda(tmpl)` |
| Tool | `compose/state.go` / `compose/prompt_tool_bridge.go` | `Invoke(ctx, args string)` 或 `Execute(ctx, map[string]any)` | `ToolsNode.GetRunnable()` 或 `NewToolsNodeLambda(tools...)` |
| Retriever | `compose/retriever.go` / `compose/bridge.go` | `Retrieve(ctx, query)` | `NewRetrieverLambda(cfg)` 或 `retrieverBridge.toLambda()` |
| Schema | `compose/schema.go` / `compose/chatmodel.go` | `Message`、`ToolCall`、`ToolInfo`、`Document`、`ToolResult` | 上下游共享数据结构 |

注意表中每一行都有“最小契约”。

这是组件层最重要的设计取舍：不要设计一个万能大接口。

不要这样：

```go
type Component interface {
    Generate(...)
    Stream(...)
    Retrieve(...)
    Format(...)
    InvokeTool(...)
    Embed(...)
    Store(...)
    Close(...)
    HealthCheck(...)
}
```

这种接口看起来统一，实际很糟糕。Prompt 不会 `Retrieve`，Retriever 不会 `Generate`，Tool 不会 `Format`。如果强行塞进一个大接口，实现者只能写一堆空方法，类型系统也无法表达真实能力。

正确方向是：

```text
每种能力一个小接口。
运行时只在桥接处把小接口转成 Runnable。
```

## 4. 先看运行时只认识什么

为了理解桥接，先回到运行时的视角。

Graph 节点最终保存的是 `composableRunnable`。它不是一个公开接口，而是运行时内部的统一执行形态。前面几章已经看过，它包含多种可选函数：

```text
i  Invoke
s  Stream
c  Collect
t  Transform
```

如果一个节点只提供 `i`，运行时就可以走 Invoke；如果提供 `s`，它也可以走 Stream；没有的形态由降级矩阵尝试兜底。

这意味着任何组件进入图之前都必须回答一个问题：

```text
我怎么把自己的领域方法放进 composableRunnable 的 i / s / c / t 里？
```

ChatModel 的答案是：

```text
Generate -> i
Stream   -> s
```

Prompt 的答案是：

```text
Format -> i
```

Retriever 的答案是：

```text
Retrieve -> i
```

Tool 的答案是：

```text
ToolCalls -> 逐个 Invoke/Execute -> Tool Message 或结果摘要 -> i
```

Bridge Adapter 的职责就是把这些答案写成可复用代码。

## 5. 教学层 Bridge Adapter：先理解模式

`compose/bridge.go` 是最适合初学者看的文件，因为它故意把接口设计得很轻。

### 5.1 BridgeDocument 和 BridgeMessage

文件开头定义了两个教学用数据结构：

```go
type BridgeDocument struct {
    Content  string
    Metadata map[string]string
    Score    float64
}

type BridgeMessage struct {
    Role    string
    Content string
}
```

它们是简化版 schema。

`BridgeDocument` 表示检索结果：

```text
Content  文档内容
Metadata 文档元数据
Score    相关性分数
```

`BridgeMessage` 表示聊天消息：

```text
Role     system / user / assistant
Content 纯文本内容
```

为什么不直接用完整的 `Message` / `Document`？

因为 `bridge.go` 的教学目标不是讲完整 schema，而是讲桥接模式。如果一上来就引入多模态、ToolCall、ResponseMeta、provider 扩展，初学者会看不清主线。

### 5.2 BridgeRetriever：领域接口

`BridgeRetriever` 只有一个方法：

```go
type BridgeRetriever interface {
    Retrieve(ctx context.Context, query string) ([]*BridgeDocument, error)
}
```

这是一个典型的领域接口。

它不关心图，不关心节点，也不关心 `Runnable`。它只表达“给我一个 query，我返回一些 documents”。

如果你是业务开发者，实现它很自然：

```go
type MyRetriever struct{}

func (r *MyRetriever) Retrieve(ctx context.Context, query string) ([]*BridgeDocument, error) {
    return []*BridgeDocument{
        {Content: "Rive is a local-first agent team runtime.", Score: 0.9},
    }, nil
}
```

这段代码没有任何图运行时知识。

### 5.3 retrieverBridge：把领域接口变成 Lambda

桥接发生在这里：

```go
type retrieverBridge struct {
    retriever BridgeRetriever
}

func (b *retrieverBridge) toLambda() *Lambda {
    return InvokableLambda(func(ctx context.Context, query string) ([]*BridgeDocument, error) {
        return b.retriever.Retrieve(ctx, query)
    })
}
```

这段代码很短，但设计含义很大。

它做了三件事：

```text
1. 保留领域接口：BridgeRetriever 还是只需要实现 Retrieve
2. 适配运行时：toLambda 返回 Graph 能接收的 Lambda
3. 固定类型边界：Lambda 输入是 string，输出是 []*BridgeDocument
```

运行时看到的是 Lambda。

业务看到的是 Retriever。

桥接层负责翻译。

### 5.4 retrieverFromMapBridge：为什么需要 map 版本

`retrieverBridge` 接收 `string`。

但 Workflow 经常通过 FieldMapping 汇总多个上游输出，下游收到的是 `map[string]any`。所以文件里还有一个版本：

```go
func retrieverFromMapBridge(retriever BridgeRetriever, queryKey string) *Lambda {
    return InvokableLambda(func(ctx context.Context, in map[string]any) ([]*BridgeDocument, error) {
        query, ok := in[queryKey].(string)
        if !ok {
            return nil, fmt.Errorf("retriever: expected string value for key %q in map input, got %T", queryKey, in[queryKey])
        }
        return retriever.Retrieve(ctx, query)
    })
}
```

这个函数体现了桥接层的第二个价值：类型校验。

如果上游字段映射错了，传来的 `query` 不是字符串，错误会停在 bridge 边界：

```text
retriever: expected string value for key "query" in map input, got int
```

这比让 retriever 内部 panic 好得多。

### 5.5 BridgeChatModel：教学版模型接口

`BridgeChatModel` 也只有一个方法：

```go
type BridgeChatModel interface {
    Generate(ctx context.Context, messages []*BridgeMessage) (string, error)
}
```

它输入消息列表，输出字符串。

这比真实 ChatModel 简化很多。真实模型通常输出 `*Message`，里面可能包含：

```text
Content
ToolCalls
ReasoningContent
ResponseMeta
Usage
多模态输出
```

但在教学层，输出字符串足够演示 RAG 管线。

桥接代码同样很短：

```go
type chatModelBridge struct {
    model BridgeChatModel
}

func (b *chatModelBridge) toLambda() *Lambda {
    return InvokableLambda(func(ctx context.Context, messages []*BridgeMessage) (string, error) {
        return b.model.Generate(ctx, messages)
    })
}
```

这里的关键点是：ChatModel 的领域方法是 `Generate`，运行时方法是 `Invoke`。桥接层把前者塞进后者。

### 5.6 promptAssemblerBridge：Prompt 也是组件

`promptAssemblerBridge` 不包装外部接口，而是包装一个提示词拼装规则：

```go
type promptAssemblerBridge struct {
    systemPrompt string
}
```

它的 `toLambda()` 输入是 `map[string]any`：

```text
query     用户问题
documents Retriever 输出的 []*BridgeDocument
```

输出是 `[]*BridgeMessage`：

```text
system message + user message
```

核心逻辑是：

```go
query, _ := in["query"].(string)
docs, _ := in["documents"].([]*BridgeDocument)

var contextParts []string
for _, doc := range docs {
    contextParts = append(contextParts, fmt.Sprintf("- %s", doc.Content))
}
contextBlock := strings.Join(contextParts, "\n")

return []*BridgeMessage{
    {Role: "system", Content: b.systemPrompt},
    {Role: "user", Content: fmt.Sprintf(
        "Context:\n%s\n\nQuestion: %s",
        contextBlock,
        query,
    )},
}, nil
```

这段代码回答了一个常见问题：

```text
Prompt Template 和普通字符串拼接有什么区别？
```

区别不在语法，而在它被抽象为组件后，可以作为独立节点进入图。

RAG 里最常见的数据流是：

```text
query -> Retriever -> documents
query + documents -> PromptAssembler -> messages
messages -> ChatModel -> answer
```

PromptAssembler 不是“顺手写在模型调用前的一段代码”，而是一个可观测、可测试、可替换的节点。

### 5.7 Workflow 便捷方法

`bridge.go` 最后给 Workflow 加了三个便捷方法：

```go
func (wf *Workflow[I, O]) AsRetrieverNode(key string, retriever BridgeRetriever) *WorkflowNode
func (wf *Workflow[I, O]) AsChatModelNode(key string, model BridgeChatModel) *WorkflowNode
func (wf *Workflow[I, O]) AsPromptAssemblerNode(key string, systemPrompt string) *WorkflowNode
```

它们本质上就是：

```text
领域对象 -> bridge.toLambda() -> AddLambdaNode
```

例如：

```go
func (wf *Workflow[I, O]) AsRetrieverNode(key string, retriever BridgeRetriever) *WorkflowNode {
    return wf.AddLambdaNode(key, (&retrieverBridge{retriever: retriever}).toLambda())
}
```

这就是面向业务使用者的 API。

业务不必写：

```go
wf.AddLambdaNode("retriever", (&retrieverBridge{retriever: retriever}).toLambda())
```

而是写：

```go
wf.AsRetrieverNode("retriever", retriever)
```

可读性立刻变好。

## 6. RAG 管线：Bridge Adapter 如何组合

`compose/bridge_test.go` 里的 `TestBridgeRAGPipelineWorkflow` 是本章最重要的测试之一。

它构建的管线是：

```text
START(query)
  |
  v
retriever: query -> documents
  |
  v
assemble: query + documents -> messages
  |
  v
model: messages -> answer
  |
  v
END: answer + original_query
```

用 Workflow 表达是：

```go
wf := NewWorkflow[string, map[string]any]()

wf.AsRetrieverNode("retriever", retriever).
    AddInput(START)

wf.AsPromptAssemblerNode("assemble", systemPrompt).
    AddInput(START, MapFields("", "query")).
    AddInput("retriever", ToField("documents"))

wf.AsChatModelNode("model", model).
    AddInput("assemble")

wf.End().
    AddInput("model", ToField("answer")).
    AddInput(START, MapFields("", "original_query"))
```

逐行看。

第一段：

```go
wf.AsRetrieverNode("retriever", retriever).
    AddInput(START)
```

输入类型是 `string`，所以 START 的原始输入直接传给 retriever。

第二段：

```go
wf.AsPromptAssemblerNode("assemble", systemPrompt).
    AddInput(START, MapFields("", "query")).
    AddInput("retriever", ToField("documents"))
```

这就是 Ch02 的 FieldMapping 回来了。

`assemble` 需要 `map[string]any`：

```text
query     string
documents []*BridgeDocument
```

所以它从两个地方拿输入：

```text
START 原始输入 -> query
retriever 输出 -> documents
```

第三段：

```go
wf.AsChatModelNode("model", model).
    AddInput("assemble")
```

`assemble` 输出 `[]*BridgeMessage`，正好是 `BridgeChatModel.Generate` 的输入。

第四段：

```go
wf.End().
    AddInput("model", ToField("answer")).
    AddInput(START, MapFields("", "original_query"))
```

END 汇总最终答案和原始问题，输出 `map[string]any`。

这个例子说明：组件层并不是替代图编排，而是和图编排配合。

```text
组件层解决“节点内部调用什么接口”
Workflow 解决“节点之间怎么连”
FieldMapping 解决“节点之间传哪些字段”
Graph Runtime 解决“节点什么时候执行”
```

## 7. 组件层包装：ChatModelComponent

教学层 Bridge 看懂后，再看更接近组件层的 `compose/chatmodel.go`。

### 7.1 Message：模型的规范消息

文件开头定义角色：

```go
type RoleType string

const (
    System    RoleType = "system"
    Human     RoleType = "human"
    Assistant RoleType = "assistant"
    Tool      RoleType = "tool"
)
```

`Message` 是当前复刻版的经典聊天消息结构：

```go
type Message struct {
    Role                     RoleType
    Content                  string
    ToolCalls                []ToolCall
    ToolCallID               string
    Name                     string
    ToolName                 string
    UserInputMultiContent    []MessageInputPart
    AssistantGenMultiContent []MessageOutputPart
    ResponseMeta             *ResponseMeta
    ReasoningContent         string
    Extra                    map[string]any
}
```

最重要的字段是：

```text
Role       消息角色
Content    文本内容
ToolCalls  Assistant 请求调用的工具列表
ToolCallID Tool 角色消息用来关联上游工具调用
ResponseMeta 模型响应元数据和 provider 扩展槽位
```

角色不是普通标签。

在工具调用链路里，角色驱动语义：

```text
Human      用户输入
Assistant 可能包含 ToolCalls
Tool       工具执行结果，靠 ToolCallID 对应 Assistant 的某个 ToolCall
System     系统提示词
```

如果你把 Tool 结果错写成 Assistant 消息，下游模型可能无法知道这是工具返回值。

如果你丢掉 ToolCallID，下游也无法把“这个工具结果”关联回“哪个工具调用”。

### 7.2 ResponseMeta：provider 差异放在哪里

`ResponseMeta` 包含通用字段和 provider 扩展字段：

```go
type ResponseMeta struct {
    ID              string
    Model           string
    FinishReason    string
    Usage           *TokenUsage
    LogProbs        *LogProbs
    OpenAIExtension *OpenAIRespMetaExtension
    GeminiExtension *GeminiRespMetaExtension
    ClaudeExtension *ClaudeRespMetaExtension
    Extension       any
}
```

这体现了 schema 防火墙思想。

通用字段放在固定位置：

```text
ID
Model
FinishReason
Usage
LogProbs
```

provider 特有字段放在扩展槽：

```text
OpenAIExtension
GeminiExtension
ClaudeExtension
Extension
```

这避免了两种极端。

第一种极端是公共 schema 完全没有扩展能力，导致 provider 特性丢失。

第二种极端是把所有 provider 的所有字段都平铺到公共 Message 上，导致 schema 被 provider 细节污染。

当前复刻版选择中间路线：

```text
公共字段稳定
特殊字段进扩展槽
```

### 7.3 ChatModel 接口

当前复刻版的 `ChatModel` 接口非常小：

```go
type ChatModel interface {
    Generate(ctx context.Context, input []*Message) (*Message, error)
    Stream(ctx context.Context, input []*Message) (StreamReader[*Message], error)
}
```

它只有两个方法。

这两个方法分别对应运行时的两种形态：

```text
Generate -> Invoke
Stream   -> Stream
```

为什么不是一个方法？

因为流式输出不是“返回一个数组”那么简单。流式模型会随着网络响应逐块产生 token 或消息增量，调用方需要边读边处理。运行时也必须知道这个节点具备 stream 能力，才能把它接入 Stream / Transform 路径。

当前复刻版没有实现完整的原版 `model.Option` 体系，也没有 `WithTools` / `BindTools` 接口。工具调用通过 `Message.ToolCalls` 和独立 ToolsNode 管线演示。

### 7.4 FakeChatModel：测试用模型不是 provider

`FakeChatModel` 是教学和测试用实现：

```go
type FakeChatModel struct {
    mu         sync.Mutex
    generateFn func(ctx context.Context, input []*Message) (*Message, error)
    streamFn   func(ctx context.Context, input []*Message) (StreamReader[*Message], error)
}
```

构造函数允许注入行为：

```go
func NewFakeChatModel(opts ...ChatModelOption) *FakeChatModel
func WithChatGenerateFunc(fn func(ctx context.Context, input []*Message) (*Message, error)) ChatModelOption
func WithChatStreamFunc(fn func(ctx context.Context, input []*Message) (StreamReader[*Message], error)) ChatModelOption
```

默认 `Generate` 行为是：

```text
没有输入 -> Assistant("no input")
有输入   -> Assistant("echo: " + last.Content)
```

默认 `Stream` 行为是：

```text
调用 Generate
把返回的一个 Message 包成单元素 StreamReader
```

这很好测，但不要把它误认为真实模型行为。

真实模型可能有：

```text
网络错误
限流
token usage
finish reason
工具调用
多模态输入输出
provider-specific metadata
流式 delta 合并
```

`FakeChatModel` 的价值是让图运行时测试保持确定性。

### 7.5 ChatModelComponent：真正的组件包装

组件包装在这里：

```go
type ChatModelComponent struct {
    cm ChatModel
}

func NewChatModelComponent(cm ChatModel) *ChatModelComponent {
    return &ChatModelComponent{cm: cm}
}
```

关键是 `GetRunnable()`：

```go
func (c *ChatModelComponent) GetRunnable() *composableRunnable {
    return &composableRunnable{
        i: func(ctx context.Context, input any) (any, error) {
            msgs, ok := input.([]*Message)
            if !ok {
                return nil, fmt.Errorf("ChatModelComponent.Invoke: expected []*Message input, got %T", input)
            }
            return c.cm.Generate(ctx, msgs)
        },
        s: func(ctx context.Context, input any) (any, error) {
            msgs, ok := input.([]*Message)
            if !ok {
                return nil, fmt.Errorf("ChatModelComponent.Stream: expected []*Message input, got %T", input)
            }
            sr, err := c.cm.Stream(ctx, msgs)
            if err != nil {
                return nil, err
            }
            return &typedStreamWrapper[*Message]{inner: sr}, nil
        },
    }
}
```

这段代码就是组件层的核心。

Invoke 路径：

```text
any input
  -> 类型断言为 []*Message
  -> c.cm.Generate(ctx, msgs)
  -> *Message
```

Stream 路径：

```text
any input
  -> 类型断言为 []*Message
  -> c.cm.Stream(ctx, msgs)
  -> StreamReader[*Message]
  -> typedStreamWrapper[*Message]
  -> 运行时内部 streamReader
```

注意两个细节。

第一，错误停在边界。

如果上游传错类型，错误信息是：

```text
ChatModelComponent.Invoke: expected []*Message input, got string
```

这比模型内部崩溃更容易定位。

第二，stream 被包装成运行时认识的 `streamReader`。

组件自己的 `StreamReader[*Message]` 和运行时内部的 stream 协议之间仍然需要一层适配。

### 7.6 AddChatModelNode

`compose/generic_graph.go` 给泛型 Graph 提供了直接添加 ChatModel 节点的方法：

```go
func (gg *Graph[I, O]) AddChatModelNode(key string, cmc *ChatModelComponent, opts ...NodeOption) error {
    if err := gg.g.addChatModelNode(key, cmc); err != nil {
        return err
    }
    ns := &nodeOptionState{}
    for _, opt := range opts {
        opt(ns)
    }
    for _, h := range ns.inputPreHandlers {
        gg.g.setNodeInputPreHandler(key, h)
    }
    return nil
}
```

这里有两个层次：

```text
用户 API：Graph.AddChatModelNode
内部实现：graph.addChatModelNode -> cmc.GetRunnable()
```

`NodeOption` 目前主要支持 `WithNodePreHandler`，可以在节点执行前改写输入。这在 Agent 里很有用，例如模型节点执行前从 local state 取消息历史并追加新输入。

## 8. Prompt 组件：MessageTemplate 和 ChatTemplateComponent

`compose/prompt.go` 讲的是 Prompt。

### 8.1 ChatTemplate 接口

接口很小：

```go
type ChatTemplate interface {
    Format(ctx context.Context, vs map[string]any) ([]*Message, error)
}
```

输入是变量表：

```text
map[string]any
```

输出是规范消息：

```text
[]*Message
```

这正好放在模型前面：

```text
业务输入 map -> Prompt.Format -> []*Message -> ChatModel.Generate
```

### 8.2 MessageTemplate 的变量替换

当前复刻版支持非常简单的变量语法：

```text
{{name}}
```

正则是：

```go
var varPattern = regexp.MustCompile(`\{\{(\w+)\}\}`)
```

替换函数是：

```go
func replaceVars(tpl string, vs map[string]any) string {
    return varPattern.ReplaceAllStringFunc(tpl, func(match string) string {
        name := match[2 : len(match)-2]
        if val, ok := vs[name]; ok {
            return fmt.Sprint(val)
        }
        return match
    })
}
```

这个实现有几个教学点。

第一，变量只支持 `\w+`。

也就是：

```text
字母
数字
下划线
```

不支持：

```text
{{user.name}}
{{ docs[0].content }}
{{#each docs}}
```

第二，缺失变量不会报错。

如果模板是：

```text
Hello, {{name}}! You are {{missing}}.
```

变量表只有：

```go
map[string]any{"name": "Alice"}
```

输出会保留：

```text
Hello, Alice! You are {{missing}}.
```

这和原版 Eino 或其他模板引擎可能不同。真实生产系统里，缺失变量是否报错是很重要的策略选择。

第三，变量值用 `fmt.Sprint` 转字符串。

这意味着任何类型都可以塞进去，但格式化效果由 Go 默认格式决定。对于复杂结构，最好在进入模板前先整理成明确字符串。

### 8.3 system + human 两段模板

`MessageTemplate` 里有两个字段：

```go
type MessageTemplate struct {
    systemTemplate *string
    userTemplate   string
}
```

构造函数只要求 user template：

```go
func NewMessageTemplate(tpl string) *MessageTemplate
```

系统模板是可选链式配置：

```go
func (mt *MessageTemplate) WithSystemTemplate(tpl string) *MessageTemplate
```

`Format` 时，如果系统模板存在，就先输出 System 消息，再输出 Human 消息：

```text
[]*Message{
  {Role: System, Content: "..."},
  {Role: Human,  Content: "..."},
}
```

这就是最小聊天模板。

### 8.4 ChatTemplateComponent

Prompt 也有组件包装：

```go
type ChatTemplateComponent struct {
    ct ChatTemplate
}

func (c *ChatTemplateComponent) GetRunnable() *composableRunnable {
    return &composableRunnable{
        i: func(ctx context.Context, input any) (any, error) {
            vs, ok := input.(map[string]any)
            if !ok {
                return nil, fmt.Errorf("ChatTemplateComponent.Invoke: expected map[string]any input, got %T", input)
            }
            return c.ct.Format(ctx, vs)
        },
    }
}
```

它只提供 Invoke。

这是合理的：Prompt 格式化本身通常是同步、确定、一次性的转换，不需要 Stream。

所以它的执行形态是：

```text
map[string]any -> []*Message
```

如果你把它接在 ChatModel 前面，类型就自然连上了：

```text
START map[string]any
  -> Prompt component
  -> []*Message
  -> ChatModel component
  -> *Message
```

## 9. Tool：两套教学实现要分清

当前复刻版里工具相关有两套实现：

```text
compose/state.go                 InvokableTool + ToolsNode
compose/prompt_tool_bridge.go    BridgeTool + toolsNodeBridge
```

它们服务不同教学目标。

### 9.1 state.go 里的 InvokableTool

`state.go` 里定义了更接近“真实 ToolsNode”的接口：

```go
type InvokableTool interface {
    Info(ctx context.Context) (*ToolInfo, error)
    Invoke(ctx context.Context, args string) (string, error)
}
```

它有两个方法。

`Info` 返回工具元信息：

```text
Name
Desc
ParamsOneOf
Extra
```

`Invoke` 执行工具：

```text
args string -> result string
```

这里的 `args` 是 JSON 字符串，不是 `map[string]any`。

为什么？

因为模型输出的工具调用参数通常就是一段 JSON 字符串。框架如果不需要理解参数结构，就可以原样传给工具，让工具自己解析。这减少了框架和工具参数 schema 的耦合。

### 9.2 ToolsNodeConfig

ToolsNode 用配置创建：

```go
type ToolsNodeConfig struct {
    Tools        []InvokableTool
    ToolCallIDFn func(toolCall ToolCall) string
}
```

`Tools` 是可执行工具列表。

`ToolCallIDFn` 是可选函数，用于决定当前执行上下文里的 tool call id。

默认情况下使用 `tc.ID`：

```go
callCtx = SetToolCallID(ctx, tc.ID)
```

如果你传了 `ToolCallIDFn`，则使用自定义 ID：

```go
callCtx = SetToolCallID(ctx, config.ToolCallIDFn(tc))
```

这和 Ch04 的地址 / 中断有关：工具执行时需要知道“我现在是哪个 tool call”，才能把中断、恢复、日志关联到具体调用。

### 9.3 NewToolsNode 的注册过程

创建 ToolsNode 时，它会先建立工具名到工具实例的映射：

```go
toolMap := make(map[string]InvokableTool, len(config.Tools))
for _, t := range config.Tools {
    info, _ := t.Info(context.Background())
    if info != nil {
        toolMap[info.Name] = t
    }
}
```

这里有个易误解点：`Info` 的错误被忽略了。

当前教学版为了简化没有处理 `Info` error。生产实现通常应该更严格：工具注册失败应返回错误，而不是静默漏注册。

### 9.4 ToolsNode 的执行过程

`ToolsNode` 的 `composableRunnable` 只提供 Invoke：

```go
i: func(ctx context.Context, input any) (any, error) {
    msg, ok := input.(*Message)
    if !ok {
        return nil, fmt.Errorf("ToolsNode: expected *Message input, got %T", input)
    }
    if len(msg.ToolCalls) == 0 {
        return []*Message{msg}, nil
    }
    results := make([]*Message, 0, len(msg.ToolCalls))
    for _, tc := range msg.ToolCalls {
        ...
    }
    return results, nil
}
```

输入必须是 `*Message`。

如果没有工具调用，返回：

```text
[]*Message{原消息}
```

如果有工具调用：

```text
遍历 msg.ToolCalls
  根据 tc.Function.Name 找工具
  把 tc.Function.Arguments 作为 JSON string 传给工具
  得到 result string
  生成 ToolMessage(result, tc.ID)
返回 []*Message
```

输出是 `[]*Message`，每个工具调用对应一条 Tool 消息。

这更接近真实 ReAct / function calling 流程：

```text
Assistant Message with ToolCalls
  -> ToolsNode
  -> Tool Messages
  -> ChatModel
```

### 9.5 prompt_tool_bridge.go 里的 BridgeTool

另一个教学实现是：

```go
type BridgeTool interface {
    Name() string
    Execute(ctx context.Context, args map[string]any) (string, error)
}
```

它比 `InvokableTool` 更简单：

```text
没有 ToolInfo
参数已经解析成 map[string]any
返回 string
```

构造函数：

```go
func NewBridgeTool(name string, fn func(ctx context.Context, args map[string]any) (string, error)) *BridgeToolFunc
```

这适合教学和测试：

```go
tool := NewBridgeTool("echo", func(ctx context.Context, args map[string]any) (string, error) {
    return args["msg"].(string), nil
})
```

### 9.6 toolsNodeBridge 的执行过程

`toolsNodeBridge` 把多个 `BridgeTool` 包成 Lambda：

```go
type toolsNodeBridge struct {
    tools map[string]BridgeTool
}
```

核心执行逻辑：

```go
if len(msg.ToolCalls) == 0 {
    return msg, nil
}

var results []string
for _, tc := range msg.ToolCalls {
    tool, ok := b.tools[tc.Function.Name]
    if !ok {
        return nil, fmt.Errorf("tools node: tool not found: %s", tc.Function.Name)
    }
    var args map[string]any
    if err := json.Unmarshal([]byte(tc.Function.Arguments), &args); err != nil {
        return nil, fmt.Errorf("tools node: %s: invalid arguments: %w", tc.Function.Name, err)
    }
    result, err := tool.Execute(ctx, args)
    if err != nil {
        return nil, fmt.Errorf("tools node: %s: %w", tc.Function.Name, err)
    }
    results = append(results, fmt.Sprintf("%s(%v): %s", tc.Function.Name, args, result))
}
```

然后它返回一条 Assistant 消息：

```go
return &Message{
    Role:    Assistant,
    Content: summary,
}, nil
```

这和 `ToolsNode` 不同。

对比一下：

| 实现 | 输入 | 工具参数 | 输出 | 适合讲什么 |
| --- | --- | --- | --- | --- |
| `ToolsNode` | `*Message` | JSON string | `[]*Message`，Tool role | 更接近 function calling 主链路 |
| `toolsNodeBridge` | `*Message` | `map[string]any` | `*Message`，Assistant summary | 教学版工具桥接与完整 pipeline |

不要把它们混成一个概念。

大纲里说“ToolsNode 和 Tool 不是一回事”，正是这个意思：

```text
Tool 是可执行能力。
ToolsNode 是图节点，负责把模型产生的 ToolCalls 分发给具体 Tool。
```

## 10. Schema：ToolCall、ToolInfo、ToolResult

组件要能串起来，必须有共享 schema。

### 10.1 ToolCall

`compose/schema.go` 里定义：

```go
type ToolCall struct {
    Index    *int             `json:"-"`
    ID       string           `json:"id"`
    Type     string           `json:"type"`
    Function ToolCallFunction `json:"function"`
    Extra    map[string]any   `json:"-"`
}

type ToolCallFunction struct {
    Name      string `json:"name"`
    Arguments string `json:"arguments"`
}
```

它表达的是模型请求：

```text
我要调用一个工具
工具名是 Function.Name
参数是 Function.Arguments
调用 ID 是 ID
```

`Index` 用于流式模式中标识 delta 属于哪个逻辑调用，但当前复刻版还没有完整的流式工具调用合并逻辑。

`Index` 和 `Extra` 都是 `json:"-"`，这意味着 JSON 往返时它们不会保留。`schema_test.go` 里专门测试了这个行为。

### 10.2 ToolInfo

`ToolInfo` 是工具元信息：

```go
type ToolInfo struct {
    Name        string
    Desc        string
    ParamsOneOf *ParamsOneOf
    Extra       map[string]any
}
```

模型需要知道工具 schema，才能生成合法 ToolCall。

在真实 Eino / provider 集成里，这类信息会被转换成 OpenAI、Claude、Gemini 各自的 tool schema。当前复刻版主要把它作为规范结构和测试对象。

### 10.3 ParamsOneOf

`ParamsOneOf` 支持两种参数 schema：

```go
type ParamsOneOf struct {
    params     map[string]*ParameterInfo
    jsonSchema any
}
```

第一种是轻量参数树：

```go
NewParamsOneOfByParams(map[string]*ParameterInfo{
    "city": {
        Type:     DataTypeString,
        Desc:     "City name",
        Required: true,
    },
})
```

第二种是完整 JSON Schema：

```go
NewParamsOneOfByJSONSchema(schema)
```

`ToJSONSchema()` 会统一输出 schema 表示。

这个设计的目的也很明确：

```text
简单工具用 ParameterInfo 快速写
复杂工具保留完整 JSON Schema 表达能力
```

### 10.4 ToolResult

`ToolResult` 是增强工具结果的雏形：

```go
type ToolResult struct {
    Text   string
    Images []*ImageContent
    Audio  []*AudioContent
    Video  []*VideoContent
    Files  []*FileContent
}
```

当前 `toolsNodeBridge` 和 `ToolsNode` 主要返回字符串，没有完整使用 `ToolResult`。

但 schema 已经预留了多模态工具输出：

```text
Text
Images
Audio
Video
Files
```

这和原始 Eino 的增强工具层级方向一致：工具不一定只返回纯文本。

比如截图工具可能返回图片，语音识别工具可能返回音频片段，文件检索工具可能返回文件引用。如果接口只支持 string，这些结果都会被迫降级。

## 11. Retriever：生产层 NewRetrieverLambda

除了 `bridge.go` 的教学版 `BridgeRetriever`，当前复刻版还有 `compose/retriever.go`。

### 11.1 Retriever 接口

这里的接口是：

```go
type Query struct {
    Text string
    K    int
}

type Retriever interface {
    Retrieve(ctx context.Context, query *Query) ([]*Document, error)
}
```

与教学层区别：

| 教学层 | 组件层 |
| --- | --- |
| `Retrieve(ctx, query string)` | `Retrieve(ctx, query *Query)` |
| 返回 `[]*BridgeDocument` | 返回 `[]*Document` |
| 主要用于 RAG bridge 示例 | 主要用于组件 + callback 示例 |

`Query` 里除了 `Text` 还有 `K`，表达取前 K 个结果。

`Document` 是规范文档结构：

```go
type Document struct {
    ID        string
    Content   string
    Metadata  map[string]string
    Meta      map[string]any
    Embedding []float64
    Score     float64
}
```

### 11.2 NewRetrieverLambda

`NewRetrieverLambda` 是一个很清晰的“组件到 Lambda”函数：

```go
func NewRetrieverLambda(cfg *RetrieverConfig) *Lambda {
    if cfg.Retriever == nil {
        panic("RetrieverConfig.Retriever must not be nil")
    }

    info := cfg.Info
    if info == nil {
        info = &RunInfo{
            Name:      "Retriever",
            Type:      "Retriever",
            Component: ComponentOfRetriever,
        }
    }

    invokeFn := func(ctx context.Context, input any) (any, error) {
        query, ok := input.(*Query)
        if !ok {
            return nil, fmt.Errorf("Retriever: expected *Query input, got %T", input)
        }
        return cfg.Retriever.Retrieve(ctx, query)
    }

    if len(cfg.Handlers) > 0 {
        cw := NewCallbackWrapper(info, cfg.Handlers)
        invokeFn = cw.Invoke(invokeFn)
    }

    cr := &composableRunnable{i: invokeFn}
    return &Lambda{invokeFn: invokeFn, cr: cr, kind: "RetrieverLambda"}
}
```

这段代码比 `retrieverBridge.toLambda()` 多两件事。

第一，默认 `RunInfo`：

```go
Name:      "Retriever"
Type:      "Retriever"
Component: ComponentOfRetriever
```

这让 callback 看到这是 Retriever 组件，而不是匿名 Lambda。

第二，内置 callback wrapper：

```go
if len(cfg.Handlers) > 0 {
    cw := NewCallbackWrapper(info, cfg.Handlers)
    invokeFn = cw.Invoke(invokeFn)
}
```

这就是大纲中特别提醒的点：

```text
NewRetrieverLambda 内置回调注入。
ChatModelComponent 不内置回调注入。
```

为什么会这样？

当前复刻版处在教学演进中，不同组件的包装成熟度不完全一致。RetrieverLambda 展示的是“组件 wrapper 内部包 callback”；ChatModelComponent 展示的是“组件返回 Runnable，外部通过 Graph.SetNodeCallbacks 或 NewCallbackWrapper 包装”。两种方式都能实现观测，但职责边界不同。

生产框架通常会更统一地处理组件 callback。

## 12. Callback：组件节点如何被观测

第五章虽然主讲组件，但不要忘了 Ch03 的 callback。

组件接入图以后，它就应该变成可观测节点。

当前复刻版有两种方式。

### 12.1 Graph.SetNodeCallbacks

`bridge_test.go` 里有例子：

```go
g.SetNodeCallbacks("model", &Handler{
    OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
        modelStartCalled = true
        return ctx
    },
    OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
        modelEndCalled = true
        return ctx
    },
})
```

这是图层注入。

优点：

```text
任何节点都能统一加
不要求组件自己知道 callback
```

缺点：

```text
组件 wrapper 本身不携带默认 RunInfo
使用者需要知道节点名
```

### 12.2 NewCallbackWrapper

`retriever.go` 里是包装器注入：

```go
cw := NewCallbackWrapper(info, cfg.Handlers)
invokeFn = cw.Invoke(invokeFn)
```

优点：

```text
组件创建时就绑定 RunInfo 和 handlers
组件语义更集中
```

缺点：

```text
每个组件包装都要写一遍
如果图层也加 callback，需要注意重复触发
```

### 12.3 ChatModelComponent 的 callback 测试

`chatmodel_test.go` 里对 ChatModelComponent callback 的测试，是手动用 `NewCallbackWrapper` 包 `cr.i` 或 `cr.s`。

这说明 ChatModelComponent 本身只是提供 runnable，不自动注入 callback。

所以你要记住：

```text
ChatModelComponent.GetRunnable() 只负责 Generate / Stream 适配。
观测要由图层或外部 wrapper 加。
```

## 13. Tool Calling Pipeline：从 Prompt 到 Tool 再到最终回答

`compose/prompt_tool_bridge_test.go` 里的完整管线测试把本章概念串起来。

### 13.1 模型先产生 ToolCall

测试里构造了一个会请求天气工具的模型：

```go
func buildToolCallModel() *FakeChatModel {
    return NewFakeChatModel(WithChatGenerateFunc(
        func(ctx context.Context, input []*Message) (*Message, error) {
            return &Message{
                Role:    Assistant,
                Content: "",
                ToolCalls: []ToolCall{
                    {
                        ID:   "call_weather",
                        Type: "function",
                        Function: ToolCallFunction{
                            Name:      "get_weather",
                            Arguments: `{"location":"Paris"}`,
                        },
                    },
                },
            }, nil
        },
    ))
}
```

这个模型不返回最终答案，而是返回：

```text
Assistant Message
  ToolCalls[0]
    ID: call_weather
    Name: get_weather
    Arguments: {"location":"Paris"}
```

这模拟了 function calling。

### 13.2 ToolsNode 执行 ToolCall

工具节点注册天气工具：

```go
toolsNode := (&toolsNodeBridge{
    tools: map[string]BridgeTool{
        "get_weather": &stubWeatherTool{},
    },
}).toLambda()
```

`stubWeatherTool` 的行为：

```go
func (t *stubWeatherTool) Execute(ctx context.Context, args map[string]any) (string, error) {
    loc, _ := args["location"].(string)
    return "Sunny, 22°C in " + loc, nil
}
```

所以工具节点把 ToolCall 转成：

```text
Tool results:
- get_weather(map[location:Paris]): Sunny, 22°C in Paris
```

### 13.3 最终模型生成回答

第二个模型 `buildFinalModel()` 接收工具结果：

```go
func buildFinalModel() *FakeChatModel {
    return NewFakeChatModel(WithChatGenerateFunc(
        func(ctx context.Context, input []*Message) (*Message, error) {
            if len(input) == 0 {
                return &Message{Role: Assistant, Content: "no input"}, nil
            }
            last := input[len(input)-1]
            return &Message{
                Role:    Assistant,
                Content: "Final answer based on: " + last.Content,
            }, nil
        },
    ))
}
```

这就是简化版 ReAct 的一轮：

```text
用户问题
  -> 模型决定调用工具
  -> 工具执行
  -> 模型基于工具结果回答
```

### 13.4 Graph 版本

Graph 测试拓扑：

```text
START
  -> model1
  -> tools
  -> model2
  -> END
```

代码大意：

```go
g := NewGraph[[]*Message, *Message]()

g.AddLambdaNode("model1", chatModelLambda)
g.AddLambdaNode("tools", toolsNode)
g.AddLambdaNode("model2", finalLambda)

g.AddEdge(START, "model1")
g.AddEdge("model1", "tools")
g.AddEdge("tools", "model2")
g.AddEdge("model2", END)
```

这里每个节点都已经是 Lambda。

### 13.5 Workflow 版本

Workflow 版本更贴近业务写法：

```go
wf := NewWorkflow[map[string]any, *Message]()

wf.AsPromptTemplateNode("prompt", tmpl).
    AddInput(START)

wf.AddLambdaNode("model1", InvokableLambda(func(ctx context.Context, msgs []*Message) (*Message, error) {
    return toolCallModel.Generate(ctx, msgs)
})).AddInput("prompt")

wf.AsToolsNode("tools", &stubWeatherTool{}).
    AddInput("model1")

wf.AddLambdaNode("model2", InvokableLambda(func(ctx context.Context, msg *Message) (*Message, error) {
    return finalModel.Generate(ctx, []*Message{msg})
})).AddInput("tools")

wf.End().AddInput("model2")
```

这里体现了三种接入方式混用：

```text
Prompt     通过 AsPromptTemplateNode
Model      临时用 Lambda 包 Generate
ToolsNode  通过 AsToolsNode
```

为什么模型没有 `AsChatModelNode`？

因为这里用的是完整 `Message`，而 `bridge.go` 的 `AsChatModelNode` 接收的是教学版 `BridgeChatModel`，输出 string。测试为了演示 ToolCall，用了 `FakeChatModel` + `Message`，所以直接写 Lambda。

这正好说明当前复刻版还没有完全统一所有组件 API；它是教学项目，不是完整生产框架。

## 14. 当前复刻版 vs 原始 Eino

这章很容易把当前代码和原始 Eino 混在一起。下面明确区分。

### 14.1 ChatModel 接口差异

当前复刻版：

```go
type ChatModel interface {
    Generate(ctx context.Context, input []*Message) (*Message, error)
    Stream(ctx context.Context, input []*Message) (StreamReader[*Message], error)
}
```

原始 Eino 更接近：

```text
BaseChatModel = Generate + Stream + model.Option
ToolCallingChatModel = BaseChatModel + WithTools
旧 ChatModel / BindTools 已弃用或不推荐
AgenticModel 与 ChatModel 有不同消息模型
```

当前复刻版没有完整实现：

```text
model.Option 双桶
WithTemperature / WithModel / WithTools
provider-specific option wrapping
ToolCallingChatModel 不可变绑定
AgenticModel 完整接口
```

所以本章讲到 `WithTools` 时要把它当作原始 Eino 的设计背景，不要误以为当前 `compose/chatmodel.go` 已经有这个方法。

### 14.2 Tool 层级差异

当前复刻版：

```text
InvokableTool: Info + Invoke(args string) -> string
BridgeTool: Name + Execute(args map[string]any) -> string
ToolResult schema 有多模态字段，但执行路径主要返回 string
```

原始 Eino 更完整：

```text
BaseTool
InvokableTool
StreamableTool
EnhancedInvokableTool
EnhancedStreamableTool
ToolResult 多模态输出
工具选项
工具 callback extra
```

当前复刻版的工具执行是同步顺序循环，没有实现并行执行、增强工具优先级、未知工具 handler、alias remap 等完整生产能力。

### 14.3 Prompt 差异

当前复刻版：

```text
{{var}} 简单替换
缺失变量保留原样
只输出 system + human
无 opts
```

原始 Eino 或生产模板系统可能支持：

```text
FString / GoTemplate / Jinja2
变量缺失策略
消息列表模板
多角色模板
模板级 option
callback extra
```

### 14.4 Retriever 差异

当前复刻版有两套：

```text
BridgeRetriever(query string) -> []*BridgeDocument
Retriever(*Query) -> []*Document
```

原始 Eino 的 retriever 通常有：

```text
query string 或请求结构
TopK
ScoreThreshold
Embedding
provider-specific options
callback extra
```

当前复刻版只保留了足够教学的结构。

## 15. 易误解点

### 15.1 “Bridge 只是多一层包装”

不是。

如果只是为了调用方法，确实可以直接写 Lambda。

但 Bridge 的价值是：

```text
固定领域接口
隔离运行时细节
集中类型校验
形成观测锚点
让业务 API 更自然
允许替换 provider
```

一个好的 bridge 不是“多一层”，而是边界。

### 15.2 “ChatModel 就是 Lambda”

ChatModel 可以被包装成 Lambda，但它不是 Lambda。

ChatModel 的领域语义是：

```text
消息列表 -> 模型响应
```

Lambda 的运行时语义是：

```text
输入 -> 输出
```

包装以后运行时能调度，但如果你把 ChatModel 当普通 Lambda，就会忽略：

```text
Role
ToolCalls
ResponseMeta
Stream
provider metadata
token usage
```

### 15.3 “Message.Role 只是字符串”

不是。

Role 决定消息在模型协议中的位置。

尤其工具调用：

```text
Assistant.ToolCalls 发起调用
Tool.ToolCallID 返回结果
```

这两个字段配对后，模型才能理解工具结果属于哪个调用。

### 15.4 “Prompt 模板缺变量会报错”

当前复刻版不会。

`replaceVars` 找不到变量时会返回原始 `{{missing}}`。

这在教学中方便，但生产里可能埋坑。如果你希望缺变量时报错，需要改 `replaceVars` 或换更严格的模板组件。

### 15.5 “ToolsNode 和 Tool 是一回事”

不是。

Tool 是能力：

```text
get_weather
calculator
search_docs
```

ToolsNode 是调度节点：

```text
读取 Message.ToolCalls
按 name 找 Tool
执行 Tool
把结果变成消息
```

一个 ToolsNode 可以管理多个 Tool。

### 15.6 “FakeChatModel 代表真实模型”

不是。

Fake 只用于确定性测试。

它没有网络、没有 token、没有 provider 选项、没有完整流式 delta、没有真实 tool schema 绑定。

教学中用它是为了让你看清图和组件接口，而不是模拟 provider 的所有细节。

### 15.7 “ChatModelComponent 会自动触发 callback”

当前复刻版不会。

它只返回 `composableRunnable`。如果要 callback，需要：

```text
Graph.SetNodeCallbacks
或手动 NewCallbackWrapper
```

`NewRetrieverLambda` 才展示了组件内部注入 callback 的方式。

### 15.8 “ToolCall.Index 会 JSON 往返保留”

不会。

`ToolCall.Index` 标了 `json:"-"`。

它用于内存里的流式 delta 归并，但当前 JSON 序列化不会保存它。

### 15.9 “toolsNodeBridge 会返回 Tool 角色消息”

不会。

`toolsNodeBridge` 返回的是一条 `Assistant` 消息，内容是工具结果摘要。

`ToolsNode` 返回的是 `[]*Message`，每条结果是 `ToolMessage(result, tc.ID)`。

这是两套教学实现的差别。

### 15.10 “组件桥接以后类型就完全安全了”

不是。

当前运行时内部很多节点输入输出仍然经过 `any`，类型错误会在运行时报错。

Bridge 的作用是把错误集中在边界，并给出清晰错误；它不能把 Go 泛型类型安全一路穿透到所有动态图边。

## 16. 源码阅读顺序

建议按这个顺序读。

### 16.1 第一遍：看教学 Bridge

读：

```text
compose/bridge.go
compose/bridge_test.go
```

关注：

```text
BridgeRetriever
BridgeChatModel
retrieverBridge.toLambda
chatModelBridge.toLambda
promptAssemblerBridge.toLambda
Workflow.AsRetrieverNode / AsChatModelNode / AsPromptAssemblerNode
TestBridgeRAGPipelineWorkflow
```

目标：理解“领域接口 -> Lambda -> Workflow”的主线。

### 16.2 第二遍：看 ChatModel 和 Message

读：

```text
compose/chatmodel.go
compose/chatmodel_test.go
```

关注：

```text
RoleType
Message
ResponseMeta
ChatModel
FakeChatModel
ChatModelComponent.GetRunnable
AddChatModelNode
```

目标：理解模型组件为什么同时提供 Invoke 和 Stream。

### 16.3 第三遍：看 Prompt

读：

```text
compose/prompt.go
compose/prompt_test.go
```

关注：

```text
ChatTemplate
MessageTemplate
replaceVars
ChatTemplateComponent
```

目标：理解 Prompt 是 `map[string]any -> []*Message` 的组件。

### 16.4 第四遍：看 Tool

读：

```text
compose/state.go
compose/prompt_tool_bridge.go
compose/prompt_tool_bridge_test.go
```

关注：

```text
InvokableTool
ToolsNodeConfig
NewToolsNode
BridgeTool
toolsNodeBridge
TestToolCallingPipelineWorkflow
```

目标：理解 ToolCall 如何变成工具执行结果。

### 16.5 第五遍：看 Schema

读：

```text
compose/schema.go
compose/schema_test.go
compose/provider_test.go
```

关注：

```text
ToolCall
ToolInfo
ParamsOneOf
ParameterInfo
ToolResult
Document
```

目标：理解组件之间共享什么数据结构。

## 17. 练习题

### 练习 1：写一个固定回答的 ChatModel

实现一个 `ChatModel`：

```text
Generate 永远返回 AssistantMessage("fixed")
Stream 返回两个 chunk: "fi" 和 "xed"
```

然后用 `NewChatModelComponent` 包装，调用 `GetRunnable().invoke` 和 `GetRunnable().stream` 验证输出。

思考：

```text
为什么 Generate 的输入必须是 []*Message？
为什么 Stream 需要 typedStreamWrapper？
```

### 练习 2：让 Prompt 缺变量时报错

当前 `replaceVars` 缺变量会保留原文。

改造一个严格版本：

```text
模板里出现 {{name}}
变量表没有 name
Format 返回 error
```

思考：

```text
严格模式适合生产吗？
保留原文适合什么场景？
```

### 练习 3：实现一个 BridgeTool

用 `NewBridgeTool` 实现：

```text
name: calculator
args: {"expression":"2+2"}
result: "4"
```

然后构造一个带 ToolCall 的 Assistant Message，传给 `NewToolsNodeLambda`。

验证输出包含工具结果。

### 练习 4：对比 ToolsNode 和 toolsNodeBridge

分别用：

```text
NewToolsNode(ToolsNodeConfig{...})
NewToolsNodeLambda(...)
```

实现同一个天气工具调用。

比较输出类型：

```text
[]*Message
*Message
```

思考：

```text
哪一个更适合 ReAct 主循环？
哪一个更适合教学演示？
```

### 练习 5：给 RetrieverLambda 加 callback

使用 `NewRetrieverLambda`，传入 `RetrieverConfig.Handlers`。

在 handler 里记录：

```text
OnStart input
OnEnd output
OnError error
```

思考：

```text
为什么 RetrieverLambda 内置 callback 比 BridgeRetriever 更像组件包装？
```

### 练习 6：实现一个 RAG Workflow

用教学层 bridge 完成：

```text
query -> retriever -> assemble -> model -> END
```

END 输出：

```text
answer
original_query
doc_count
```

提示：`doc_count` 可以加一个 Lambda 节点，也可以在 End 前写一个汇总节点。

### 练习 7：补一个 ToolInfo schema

为天气工具写 `ToolInfo`：

```text
Name: get_weather
Desc: Get current weather for a city
Params:
  location string required
  unit string enum ["celsius", "fahrenheit"]
```

调用 `ParamsOneOf.ToJSONSchema()`，观察输出。

### 练习 8：分析 callback 放在哪里更好

比较两种设计：

```text
组件内部 NewCallbackWrapper
图层 SetNodeCallbacks
```

分别回答：

```text
谁更适合统一观测？
谁更适合组件自带默认 RunInfo？
谁更容易重复触发？
```

## 18. 自测问题

1. Graph Runtime 最终调度的统一对象是什么？
2. `BridgeRetriever` 和 `Retriever` 两个接口的输入输出有什么区别？
3. `ChatModel.Generate` 和 `ChatModelComponent.GetRunnable().i` 的关系是什么？
4. `ChatModel.Stream` 为什么需要转换成运行时内部的 streamReader？
5. 当前 `MessageTemplate` 缺失变量时会报错吗？
6. `Message` 里的 `ToolCalls` 应该放在哪个角色的消息上？
7. Tool 结果消息为什么需要 `ToolCallID`？
8. `ToolsNode` 和 `toolsNodeBridge` 的输出类型分别是什么？
9. `ToolCall.Function.Arguments` 在 `ToolsNode` 里是什么类型？在 `toolsNodeBridge` 里执行前变成什么类型？
10. `NewRetrieverLambda` 和 `retrieverBridge.toLambda()` 谁内置 callback？
11. `ChatModelComponent` 会自动注入 callback 吗？
12. `ToolInfo.ParamsOneOf` 为什么要支持轻量参数树和完整 JSON Schema 两种模式？
13. 当前复刻版有没有完整实现原始 Eino 的 `WithTools` 不可变绑定？
14. 为什么说 Bridge Adapter 是边界，而不只是包装？
15. 如果上游把 string 传给 `ChatModelComponent.Invoke`，错误会在哪里出现？

参考答案要点：

1. `composableRunnable` / `Runnable`。
2. 教学层 `BridgeRetriever` 接收 `string` 返回 `[]*BridgeDocument`；组件层 `Retriever` 接收 `*Query` 返回 `[]*Document`。
3. `GetRunnable().i` 做类型断言后调用 `cm.Generate(ctx, msgs)`。
4. 因为组件的泛型 `StreamReader[*Message]` 需要适配到运行时内部 stream 协议。
5. 不会，保留原始 `{{missing}}`。
6. Assistant 消息。
7. 用来关联上游 Assistant 的具体 ToolCall。
8. `ToolsNode` 返回 `[]*Message`；`toolsNodeBridge` 返回 `*Message`。
9. `ToolsNode` 传 JSON string；`toolsNodeBridge` 会先 `json.Unmarshal` 成 `map[string]any`。
10. `NewRetrieverLambda`。
11. 不会。
12. 简单工具方便写，复杂工具保留完整表达能力。
13. 没有，当前复刻版只保留教学简化。
14. 它固定领域接口、隔离运行时、集中类型校验和观测锚点。
15. 在 `ChatModelComponent.Invoke` 的类型断言边界。

## 19. 本章总结

第五章的主线可以压缩成一句话：

```text
组件层让领域能力保持领域接口，Bridge Adapter 让这些接口进入统一 Runnable 运行时。
```

你需要记住四个映射：

```text
ChatModel.Generate / Stream -> Runnable.Invoke / Stream
ChatTemplate.Format         -> Runnable.Invoke
Retriever.Retrieve          -> Runnable.Invoke
ToolCalls + Tool.Execute    -> Runnable.Invoke
```

再记住两个层次：

```text
bridge.go / prompt_tool_bridge.go
  教学层：用最少代码讲清 Bridge Adapter

chatmodel.go / prompt.go / retriever.go / state.go
  组件层：更接近真实框架的组件包装与 schema
```

最后记住一个边界：

```text
当前 Go 复刻版不是完整 Eino。
它保留了架构骨架：Message、ToolCall、Prompt、ChatModel、Retriever、ToolsNode、Bridge、Callback。
原始 Eino 的 provider options、WithTools、增强工具、多模态工具执行、完整 callback extra 等能力，需要结合原版源码继续阅读。
```

读完这一章，你应该能看懂为什么 Ch07 的 ReAct 可以写成：

```text
ChatModel -> 判断 ToolCall -> ToolsNode -> ChatModel
```

因为 ChatModel 和 ToolsNode 已经被组件层桥接成了 Graph 能调度的节点。

