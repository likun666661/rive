# Chapter 06 - Schema / Provider Adapter 深度讲解

面向读者：假设你已经读过前五章，知道 Graph / Workflow / Chain 只调度统一的 `Runnable`，也知道 ChatModel、Prompt、Tool、Retriever 这些领域组件可以通过 Bridge / Component 包装进入图。

这一章要回答的问题是：

```text
OpenAI、Claude、Gemini 的消息格式完全不一样，为什么 Graph 不需要知道这些差异？
工具调用在 OpenAI 里是 Message.ToolCalls，在 Claude 里是 content block，在 Gemini 里是 part，框架如何统一？
为什么需要 Message 和 AgenticMessage 两套 canonical schema？
Provider Adapter 的职责是什么，职责边界又在哪里？
为什么 ResponseMeta 要用类型化扩展槽位，而不是一个 map[string]any？
```

参考代码位置：

- 大纲：`examples/eino-technical-manual/manual/teaching-manual-outline.md`
- 复刻版：`examples/eino-compose-runtime-replica-go`
- 本章重点源码：
  - `compose/schema.go`
  - `compose/chatmodel.go`
  - `compose/provider.go`
  - `compose/provider_openai.go`
  - `compose/provider_claude.go`
  - `compose/provider_gemini.go`
  - `compose/concat.go`
  - `compose/schema_test.go`
  - `compose/chatmodel_test.go`
  - `compose/provider_test.go`
  - `compose/concat_test.go`

先说明边界：仓库当前没有单独的 `manual/06-schema-provider-adapter.md` 文件，Chapter 06 的教学主线写在 `teaching-manual-outline.md` 里。本章扩展以 outline 的 Chapter 06 为提纲，以当前 Go 复刻版源码和测试为事实依据。原始 Eino 的 schema/provider 层更完整，当前复刻版是教学实现：provider 都是 fake adapter，没有真实 SDK 请求，也没有完整 provider option、复杂流协议和生产级错误模型。

## 1. 为什么需要 Canonical Schema

第五章讲的是组件如何进入 Graph。

但组件进入图以后，还有一个更隐蔽的问题：

```text
组件之间传什么数据？
```

如果你只接一个 OpenAI provider，业务里直接使用 OpenAI 的结构也能跑：

```text
OpenAI messages[]
  -> OpenAI ChatModel
  -> OpenAI response
```

但一旦你要支持多个 provider，问题马上出现。

OpenAI 的格式大致是：

```text
messages[].role = system / user / assistant / tool
assistant message 上有 tool_calls
tool message 上有 tool_call_id
```

Claude 的格式大致是：

```text
messages[].role = user / assistant
content 是 block 数组
block.type = text / image / tool_use / tool_result
```

Gemini 的格式大致是：

```text
contents[].role = user / model / function
parts 是联合体
part 可能是 text / inlineData / functionCall / functionResponse
```

如果 Graph 直接感知这些格式，节点会变成这样：

```go
if provider == "openai" {
    // parse OpenAI messages
} else if provider == "claude" {
    // parse Claude blocks
} else if provider == "gemini" {
    // parse Gemini parts
}
```

这会把 provider 差异扩散到所有层：

```text
Prompt 要知道 provider
Tool 要知道 provider
ChatModel 要知道 provider
Agent 要知道 provider
Graph 也可能间接知道 provider
```

结果就是：

```text
切换模型 = 重写半条业务链
```

Canonical Schema 的目标正好相反：

```text
Provider 边界外：使用 provider 原生格式
Provider 边界内：尽快转成框架规范格式
Graph / Workflow / Agent：只看规范格式
返回 provider 前：再从规范格式转回 provider 原生格式
```

可以画成这样：

```text
OpenAI request  ─┐
Claude request  ─┼─> Provider Adapter -> Canonical Schema -> Graph / Component / Agent
Gemini request  ─┘

Graph / Component / Agent -> Canonical Schema -> Provider Adapter -> provider request
```

这就是本章标题里的 Provider Adapter。

它不是调度器，也不是模型，也不是工具执行器。它的职责很窄：

```text
provider native type <-> canonical type
```

## 2. 本章的核心文件

当前复刻版把 schema 和 provider adapter 都放在 `compose/` 包里：

```text
compose/
  schema.go           共享 schema：ToolCall / ToolInfo / ParamsOneOf / ToolResult / Document
  chatmodel.go        Message / ResponseMeta / provider metadata extension
  provider.go         AgenticMessage / ContentBlock / Provider interfaces
  provider_openai.go  OpenAI native type <-> Message
  provider_claude.go  Claude native type <-> AgenticMessage
  provider_gemini.go  Gemini native type <-> Message 或 AgenticMessage
  concat.go           流式 chunk 合并，尤其是 Message / ToolCall / ToolResult
```

不要被文件名迷惑。

`schema.go` 不是唯一的 schema 文件。当前复刻版里有三类 schema：

```text
schema.go       工具、文档、参数 schema
chatmodel.go    经典聊天消息 Message + ResponseMeta
provider.go     agent 级消息 AgenticMessage + ContentBlock
```

为什么拆成这样？

因为这三类数据服务不同场景。

`schema.go` 是横向共享基础类型：

```text
ToolCall
ToolInfo
ParamsOneOf
ParameterInfo
ToolResult
Document
```

`chatmodel.go` 是经典 Chat Completion 路径：

```text
Message
RoleType
ResponseMeta
TokenUsage
provider extension slots
```

`provider.go` 是 Agentic / content block 路径：

```text
ContentBlock
AgenticMessage
ProviderOpenAI
ProviderClaude
ProviderGemini
```

这也是本章最重要的概念之一：当前复刻版不只有一个消息模型。

## 3. 两条规范路径：Message 与 AgenticMessage

当前复刻版有两条 canonical message 路径：

```text
Message          经典 Chat Completion 路径
AgenticMessage   Agent / ContentBlock 路径
```

### 3.1 Message：经典聊天消息

`compose/chatmodel.go` 里定义：

```go
type RoleType string

const (
    System    RoleType = "system"
    Human     RoleType = "human"
    Assistant RoleType = "assistant"
    Tool      RoleType = "tool"
)
```

`Message` 是：

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

这套模型适合 OpenAI 风格的 chat completion。

工具调用放在顶层字段：

```text
Assistant Message
  ToolCalls []ToolCall

Tool Message
  ToolCallID string
  Content    tool result
```

所以一轮工具调用可以表示为：

```text
User:      Weather?
Assistant: ToolCalls[0] = get_weather({"city":"Paris"})
Tool:      ToolCallID = call_1, Content = Sunny
Assistant: The weather is sunny.
```

这和 Chapter 05 的 ToolsNode 直接衔接。

### 3.2 AgenticMessage：内容块消息

`compose/provider.go` 里定义另一套模型：

```go
type AgenticRoleType string

const (
    AgenticRoleAssistant AgenticRoleType = "assistant"
    AgenticRoleUser      AgenticRoleType = "user"
    AgenticRoleSystem    AgenticRoleType = "system"
)

type AgenticMessage struct {
    Role          AgenticRoleType
    ContentBlocks []*ContentBlock
}
```

它没有 `Tool` role。

工具调用和工具结果都进入 `ContentBlocks`：

```go
type ContentBlock struct {
    Type             ContentBlockType
    UserInputText    *string
    UserInputImage   *MessageInputImage
    UserInputAudio   *MessageInputAudio
    UserInputVideo   *MessageInputVideo
    UserInputFile    *MessageInputFile
    AssistantGenText *AssistantGenTextBlock
    Reasoning        *string
    FunctionToolCall *FunctionToolCallBlock
    ServerToolCall   *ServerToolCallBlock
    ToolResult       *ToolResultBlock
    ToolSearchResult *ToolSearchResult
}
```

这更接近 Claude / Gemini 的 block / part 模型。

一个 agentic 消息可以同时包含：

```text
text block
image block
tool call block
tool result block
reasoning block
tool search result block
```

例如 Claude 的 assistant 消息：

```text
assistant:
  text: "Checking..."
  tool_use: get_weather({"city":"Paris"})
```

在 canonical agentic 路径里就是：

```text
AgenticMessage{
  Role: assistant,
  ContentBlocks: [
    AssistantGenText("Checking..."),
    FunctionToolCall(callID, name, args),
  ],
}
```

### 3.3 为什么不只用一种 Message

初学者容易问：

```text
为什么不把所有 provider 都转成 Message？
```

答案是：可以转一部分，但不是所有语义都自然。

`Message` 的设计偏 OpenAI 经典聊天：

```text
role 是消息级别
工具调用在顶层 ToolCalls
工具结果是一条 Tool role 消息
```

`AgenticMessage` 的设计偏内容块：

```text
role 是消息级别
消息内部有多种 block
工具调用和工具结果都是 block
多模态输入输出天然是 block
```

Claude 的 content 本来就是 block 数组，如果硬塞进 `Message.Content`，你要么丢结构，要么把 block 序列化成字符串。

Gemini 更特殊：它既可以走 Message 路径，也可以走 AgenticMessage 路径，因为当前复刻版同时实现了：

```go
ToCanonicalAgenticMessagesFromGemini
FromCanonicalAgenticMessagesToGemini
ToCanonicalMessagesFromGemini
FromCanonicalMessagesToGemini
```

这不是重复劳动，而是在展示同一个 provider 可以被适配到两条不同的 canonical 路径。

## 4. schema.go：工具和参数的共享防火墙

`compose/schema.go` 定义的是工具、参数、文档这类跨组件共享结构。

### 4.1 ToolCall：模型请求调用工具

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

它表达一件事：

```text
模型希望调用 Function.Name，参数是 Function.Arguments。
```

`Arguments` 是字符串，因为 provider 通常把函数参数作为 JSON 字符串传递。

`Index` 只在内存中用于流式合并：

```text
Index=0 的多个 chunk 属于同一个工具调用
Index=1 的多个 chunk 属于另一个工具调用
```

但它有 `json:"-"` 标签。

这意味着 JSON 往返后，`Index` 会丢失。测试 `TestToolCall_JsonRoundTrip` 专门验证了这一点。

这不是 bug，而是当前复刻版的设计边界：`Index` 是流式拼接时的临时信息，不是持久 JSON 协议字段。

`Extra` 也是 `json:"-"`，所以 provider 私有信息如果放这里，也不会 JSON 往返保留。

### 4.2 ToolInfo：给模型看的工具说明

```go
type ToolInfo struct {
    Name        string
    Desc        string
    ParamsOneOf *ParamsOneOf
    Extra       map[string]any
}
```

`ToolInfo` 不是工具执行结果。

它是注册给模型看的工具元信息：

```text
Name        工具名
Desc        工具说明
ParamsOneOf 参数 schema
Extra       额外信息
```

模型需要它来生成合法的 ToolCall。

真实 provider 会把它转成各自的 tool schema：

```text
OpenAI tools[]
Claude tools[]
Gemini function declarations
```

当前复刻版没有完整实现这些 provider tool schema 输出，但 `ToolInfo` 已经是这个方向的规范结构。

### 4.3 ParamsOneOf：双模式参数 schema

`ParamsOneOf` 很关键：

```go
type ParamsOneOf struct {
    params     map[string]*ParameterInfo
    jsonSchema any
}
```

它有两个构造函数：

```go
NewParamsOneOfByParams(params map[string]*ParameterInfo)
NewParamsOneOfByJSONSchema(schema any)
```

第一种适合简单工具：

```go
NewParamsOneOfByParams(map[string]*ParameterInfo{
    "city": {
        Type:     DataTypeString,
        Desc:     "City name",
        Required: true,
    },
})
```

第二种适合复杂 JSON Schema：

```go
NewParamsOneOfByJSONSchema(map[string]any{
    "type": "object",
    "properties": map[string]any{
        "query": map[string]any{"type": "string"},
    },
})
```

`ToJSONSchema()` 的优先级是：

```go
if p == nil {
    return nil, nil
}
if p.params != nil {
    return paramsToMap(p.params), nil
}
return p.jsonSchema, nil
```

也就是说：

```text
params 非 nil -> 忽略 jsonSchema
params nil    -> 返回 jsonSchema
```

这解释了 outline 里的“ParamsOneOf 双重设置陷阱”。

如果有人手动构造：

```go
p := &ParamsOneOf{
    params:     someParams,
    jsonSchema: someSchema,
}
```

那么 `jsonSchema` 会被静默忽略。

由于字段是小写非导出，正常包外用户无法这样做；但在同包内测试或扩展代码中仍然要知道这个规则。最稳妥的方式是只用构造函数。

### 4.4 ParameterInfo 如何变成 JSON Schema

轻量参数树由 `ParameterInfo` 表示：

```go
type ParameterInfo struct {
    Type      DataType
    Desc      string
    Required  bool
    Enum      []string
    SubParams map[string]*ParameterInfo
    ElemInfo  *ParameterInfo
}
```

它可以表达：

```text
primitive: string / integer / boolean / number
enum
object 子字段
array 元素类型
required 字段
description
```

`paramsToMap` 会生成：

```text
{
  "type": "object",
  "properties": {...},
  "required": [...]
}
```

`paramInfoToMap` 负责递归展开单个参数。

两个细节要注意。

第一，如果 `SubParams` 非空，它会返回一个 object schema，并覆盖原本累积的 `m`：

```go
if len(pi.SubParams) > 0 {
    sub := map[string]any{
        "type":       "object",
        "properties": map[string]any{},
    }
    ...
    return sub
}
```

所以对象型参数的描述、枚举等字段不会继续保留在返回值里。这是当前教学实现的简化。

第二，required 是当前层字段名列表。

顶层 `paramsToMap` 收集顶层 required；嵌套 object 再收集自己的 subRequired。

### 4.5 ToolResult 与 Document

`ToolResult` 是多模态工具结果容器：

```go
type ToolResult struct {
    Text   string
    Images []*ImageContent
    Audio  []*AudioContent
    Video  []*VideoContent
    Files  []*FileContent
}
```

当前工具执行路径主要返回 string，但 schema 已经预留了多模态结果。

`Document` 是检索/索引用规范文档：

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

这里也有两个 metadata 容器：

```text
Metadata map[string]string
Meta     map[string]any
```

前者适合简单字符串元数据，后者适合更复杂结构。

## 5. chatmodel.go：Message 与 ResponseMeta

`Message` 是经典路径的 canonical 消息。上一章已经讲过它的角色和 ToolCall，这一章重点看 provider 扩展。

### 5.1 ResponseMeta

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

它分成三层。

第一层是通用元信息：

```text
ID
Model
FinishReason
Usage
LogProbs
```

这些概念大多数 provider 都有类似字段。

第二层是类型化 provider 扩展槽：

```text
OpenAIExtension
GeminiExtension
ClaudeExtension
```

第三层是兜底扩展：

```text
Extension any
```

为什么不用一个 `map[string]any`？

因为 map 在合并、序列化、跨 provider 共存时很容易出问题。

例如 OpenAI 和 Claude 都有 `id`，但语义不完全一样。如果都放进：

```go
Extra["id"] = ...
```

后写入的 provider 可能覆盖先写入的 provider。

类型化扩展槽位则不会：

```go
meta.OpenAIExtension.ID
meta.ClaudeExtension.ID
meta.GeminiExtension.ID
```

三个槽位可以同时存在，各自保留语义。

### 5.2 TokenUsage 和 ReasoningTokens

`TokenUsage` 是：

```go
type TokenUsage struct {
    PromptTokens     int
    CompletionTokens int
    TotalTokens      int
    ReasoningTokens  int
}
```

`ReasoningTokens` 是面向推理模型的扩展字段。

测试 `TestMessage_ResponseMeta_Usage` 和 `TestTokenUsage_ReasoningTokens` 验证它们能被保存。

注意当前复刻版不会自动计算 `TotalTokens`。

测试里只是构造：

```go
PromptTokens:     50
CompletionTokens: 100
TotalTokens:      150
ReasoningTokens:  30
```

然后检查字段值。真实 provider adapter 如果要填 usage，需要从 provider 响应里明确映射。

### 5.3 三个 provider 扩展槽

OpenAI 扩展：

```go
type OpenAIRespMetaExtension struct {
    ID                   string
    Status               string
    PreviousResponseID   string
    IncompleteDetails    *OpenAIIncompleteDetails
    ServiceTier          string
    CreatedAt            int64
    PromptCacheRetention string
}
```

Claude 扩展：

```go
type ClaudeRespMetaExtension struct {
    ID          string
    StopReason  string
    StopDetails *ClaudeStopDetails
}
```

Gemini 扩展：

```go
type GeminiRespMetaExtension struct {
    ID            string
    FinishReason  string
    GroundingMeta *GeminiGroundingMetadata
}
```

Gemini 的 grounding metadata 又包含：

```text
GroundingChunks
GroundingSupports
SearchEntryPoint
WebSearchQueries
```

这很好地说明了类型化扩展的价值。

如果把这些字段硬塞进公共 `ResponseMeta`，公共 schema 会被 Gemini 的 grounding 细节污染；如果全部塞进 map，使用者又要做大量类型断言。

类型化扩展槽位是中间路线：

```text
公共层保持稳定
provider 特有层保持类型
```

## 6. provider.go：ContentBlock 与 Provider 接口

`provider.go` 先定义 ContentBlockType：

```go
const (
    ContentBlockTypeUserInputText      ContentBlockType = "user_input_text"
    ContentBlockTypeUserInputImage     ContentBlockType = "user_input_image"
    ContentBlockTypeUserInputAudio     ContentBlockType = "user_input_audio"
    ContentBlockTypeUserInputVideo     ContentBlockType = "user_input_video"
    ContentBlockTypeUserInputFile      ContentBlockType = "user_input_file"
    ContentBlockTypeAssistantGenText   ContentBlockType = "assistant_gen_text"
    ContentBlockTypeAssistantGenImage  ContentBlockType = "assistant_gen_image"
    ContentBlockTypeReasoning          ContentBlockType = "reasoning"
    ContentBlockTypeFunctionToolCall   ContentBlockType = "function_tool_call"
    ContentBlockTypeServerToolCall     ContentBlockType = "server_tool_call"
    ContentBlockTypeFunctionToolResult ContentBlockType = "function_tool_result"
    ContentBlockTypeServerToolResult   ContentBlockType = "server_tool_result"
    ContentBlockTypeToolSearchResult   ContentBlockType = "tool_search_result"
)
```

这是一个手写 tagged union。

Go 没有内建 sum type，所以当前复刻版用：

```go
Type 字段 + 多个指针字段
```

来表达“这个 block 实际是哪一种”。

例如：

```go
func NewToolCallContentBlock(callID, name, args string) *ContentBlock {
    return &ContentBlock{
        Type:             ContentBlockTypeFunctionToolCall,
        FunctionToolCall: &FunctionToolCallBlock{CallID: callID, Name: name, Arguments: args},
    }
}
```

构造函数同时设置：

```text
Type = function_tool_call
FunctionToolCall = 非 nil
```

这就是识别依据。

### 6.1 AgenticMessageFirstText

```go
func AgenticMessageFirstText(am *AgenticMessage) string {
    for _, b := range am.ContentBlocks {
        if b.UserInputText != nil {
            return *b.UserInputText
        }
        if b.AssistantGenText != nil {
            return b.AssistantGenText.Content
        }
    }
    return ""
}
```

它从 content blocks 中取第一段文本。

注意它只看两种字段：

```text
UserInputText
AssistantGenText
```

不会把 tool call、tool result、image 等转成文本。

### 6.2 AgenticMessageToolCalls

```go
func AgenticMessageToolCalls(am *AgenticMessage) []*FunctionToolCallBlock {
    var calls []*FunctionToolCallBlock
    for _, b := range am.ContentBlocks {
        if b.FunctionToolCall != nil {
            calls = append(calls, b.FunctionToolCall)
        }
    }
    return calls
}
```

它从 blocks 中提取所有 function tool call。

这解释了为什么 AgenticMessage 不需要顶层 `ToolCalls` 字段：工具调用就是 content block 的一种。

### 6.3 三个 Provider 接口

当前复刻版没有定义一个泛型 `Provider` 大接口，而是分了三个：

```go
type ProviderOpenAI interface {
    Name() string
    ToCanonicalMessages(req *OpenAIChatRequest) ([]*Message, error)
    FromCanonicalMessages(msgs []*Message) (*OpenAIChatRequest, error)
}

type ProviderClaude interface {
    Name() string
    ToCanonicalAgenticMessages(req *ClaudeChatRequest) ([]*AgenticMessage, error)
    FromCanonicalAgenticMessages(msgs []*AgenticMessage) (*ClaudeChatRequest, error)
}

type ProviderGemini interface {
    Name() string
    ToCanonicalAgenticMessages(req *GeminiChatRequest) ([]*AgenticMessage, error)
    FromCanonicalAgenticMessages(msgs []*AgenticMessage) (*GeminiChatRequest, error)
    ToCanonicalMessages(req *GeminiChatRequest) ([]*Message, error)
    FromCanonicalMessages(msgs []*Message) (*GeminiChatRequest, error)
}
```

这很有教学意义。

OpenAI 只实现 Message 路径。

Claude 只实现 AgenticMessage 路径。

Gemini 两条路径都实现。

这不是说真实世界里 OpenAI 不能有 content block，也不是说 Claude 不能转经典 Message。它只是当前复刻版为了突出差异，给每个 provider 选择了最能体现其原生结构的路径。

## 7. OpenAI Adapter：最接近 Message 的 provider

`provider_openai.go` 是最容易读的 adapter。

### 7.1 原生结构

```go
type OpenAIMessage struct {
    Role       string     `json:"role"`
    Content    string     `json:"content,omitempty"`
    ToolCalls  []ToolCall `json:"tool_calls,omitempty"`
    ToolCallID string     `json:"tool_call_id,omitempty"`
    Name       string     `json:"name,omitempty"`
}

type OpenAIChatRequest struct {
    Model    string           `json:"model"`
    Messages []*OpenAIMessage `json:"messages"`
}
```

这个结构几乎和 `Message` 一一对应。

所以 OpenAI adapter 是最简单的：

```text
OpenAIMessage.Role       -> Message.Role
OpenAIMessage.Content    -> Message.Content
OpenAIMessage.ToolCalls  -> Message.ToolCalls
OpenAIMessage.ToolCallID -> Message.ToolCallID
OpenAIMessage.Name       -> Message.Name
```

### 7.2 角色映射

OpenAI 到 canonical：

```go
func openAIRoleToCanonical(role string) RoleType {
    switch role {
    case "system":
        return System
    case "user":
        return User
    case "assistant":
        return Assistant
    case "tool":
        return Tool
    default:
        return RoleType(role)
    }
}
```

canonical 到 OpenAI：

```go
func canonicalRoleToOpenAI(role RoleType) string {
    switch role {
    case System:
        return "system"
    case Human, User:
        return "user"
    case Assistant:
        return "assistant"
    case Tool:
        return "tool"
    default:
        return string(role)
    }
}
```

这里有一个容易忽略的点：`Human` 和 `User` 都映射成 `"user"`。

当前代码里：

```go
const (
    Human RoleType = "human"
)
```

而 `types.go` 里还有：

```go
const (
    User RoleType = "user"
)
```

也就是说：

```text
Human 的字符串值是 "human"
User  的字符串值是 "user"
```

它们不是同一个常量。

但 OpenAI adapter 输出时把二者都转成 `"user"`。

这就是 outline 里说的“Human 和 User 角色常量冗余”。

### 7.3 双向转换

OpenAI -> Message：

```go
func ToCanonicalMessages(req *OpenAIChatRequest) []*Message {
    if req == nil {
        return nil
    }
    msgs := make([]*Message, 0, len(req.Messages))
    for _, om := range req.Messages {
        msgs = append(msgs, &Message{
            Role:       openAIRoleToCanonical(om.Role),
            Content:    om.Content,
            ToolCalls:  om.ToolCalls,
            ToolCallID: om.ToolCallID,
            Name:       om.Name,
        })
    }
    return msgs
}
```

Message -> OpenAI：

```go
func FromCanonicalMessages(msgs []*Message, model string) *OpenAIChatRequest {
    omsgs := make([]*OpenAIMessage, 0, len(msgs))
    for _, m := range msgs {
        omsgs = append(omsgs, &OpenAIMessage{
            Role:       canonicalRoleToOpenAI(m.Role),
            Content:    m.Content,
            ToolCalls:  m.ToolCalls,
            ToolCallID: m.ToolCallID,
            Name:       m.Name,
        })
    }
    return &OpenAIChatRequest{Model: model, Messages: omsgs}
}
```

注意：`ToCanonicalMessages(nil)` 返回 nil，不报错。

但 `FakeOpenAIProvider.ToCanonicalMessages(nil)` 会返回 error：

```go
if req == nil {
    return nil, fmt.Errorf("openai: nil request")
}
```

这是两层 API 的差别：

```text
纯转换函数：nil in -> nil out
provider 接口方法：nil 是调用错误
```

### 7.4 OpenAI RoundTrip

测试 `TestOpenAIRoundTrip` 走的是：

```text
OpenAIChatRequest
  -> ToCanonicalMessages
  -> FromCanonicalMessages
  -> OpenAIChatRequest
```

它验证：

```text
messages 数量保留
tool_call_id 保留
tool_calls 保留
role 映射可往返
```

这就是 provider adapter 最基本的质量门槛。

## 8. Claude Adapter：content blocks 是主角

Claude adapter 只走 AgenticMessage 路径。

### 8.1 原生结构

```go
type ClaudeContentBlock struct {
    Type      string             `json:"type"`
    Text      string             `json:"text,omitempty"`
    Source    *ClaudeImageSource `json:"source,omitempty"`
    ID        string             `json:"id,omitempty"`
    Name      string             `json:"name,omitempty"`
    Input     interface{}        `json:"input,omitempty"`
    Content   interface{}        `json:"content,omitempty"`
    ToolUseID string             `json:"tool_use_id,omitempty"`
}

type ClaudeMessage struct {
    Role    string                `json:"role"`
    Content []*ClaudeContentBlock `json:"content"`
}
```

Claude 消息的核心不是 `Content string`，而是 `Content []*ClaudeContentBlock`。

所以它天然适合映射到：

```text
AgenticMessage.ContentBlocks
```

### 8.2 角色映射

Claude 原生 role：

```text
user
assistant
```

adapter 映射：

```go
func claudeRoleToAgentic(role string) AgenticRoleType {
    switch role {
    case "user":
        return AgenticRoleUser
    case "assistant":
        return AgenticRoleAssistant
    default:
        return AgenticRoleType(role)
    }
}
```

反向映射：

```go
func agenticRoleToClaude(role AgenticRoleType) string {
    switch role {
    case AgenticRoleSystem:
        return "user"
    case AgenticRoleUser:
        return "user"
    case AgenticRoleAssistant:
        return "assistant"
    default:
        return "user"
    }
}
```

注意 `system` 被映射为 `"user"`。

这不是说 Claude 没有系统提示能力，而是当前教学版没有建模 Claude 顶层 system 字段或更完整的 messages API，只把 `AgenticRoleSystem` 降级到 user role。

### 8.3 Claude block -> ContentBlock

```go
func claudeBlockToCanonical(cb *ClaudeContentBlock) *ContentBlock {
    switch cb.Type {
    case "text":
        return NewTextContentBlock(cb.Text)
    case "image":
        if cb.Source != nil {
            return NewImageContentBlock(cb.Source.Data)
        }
        return NewTextContentBlock("")
    case "tool_use":
        inputStr := ""
        if cb.Input != nil {
            inputStr = fmt.Sprintf("%v", cb.Input)
        }
        return NewToolCallContentBlock(cb.ID, cb.Name, inputStr)
    case "tool_result":
        contentStr := ""
        if cb.Content != nil {
            contentStr = fmt.Sprintf("%v", cb.Content)
        }
        return NewToolResultContentBlock(cb.ToolUseID, contentStr)
    default:
        return NewTextContentBlock(fmt.Sprintf("%v", cb))
    }
}
```

这段代码有两个教学点。

第一，Claude 的 `tool_use` 映射为 `FunctionToolCall` block：

```text
ID    -> CallID
Name  -> Name
Input -> Arguments string
```

第二，当前复刻版对 `Input` 使用 `fmt.Sprintf("%v", cb.Input)`。

如果 `Input` 是 map，输出可能是：

```text
map[city:Paris]
```

这不是 JSON。

这说明当前 fake adapter 只演示结构映射，不保证生产级 JSON 保真。真实 adapter 应该用 `json.Marshal` 或 provider SDK 给出的原始 JSON。

### 8.4 ContentBlock -> Claude block

反向转换：

```go
func canonicalBlockToClaude(cb *ContentBlock) *ClaudeContentBlock {
    switch {
    case cb.UserInputText != nil:
        return &ClaudeContentBlock{Type: "text", Text: *cb.UserInputText}
    case cb.AssistantGenText != nil:
        return &ClaudeContentBlock{Type: "text", Text: cb.AssistantGenText.Content}
    case cb.UserInputImage != nil:
        return &ClaudeContentBlock{Type: "image", Source: &ClaudeImageSource{Type: "url", MediaType: "image/png", Data: cb.UserInputImage.URL}}
    case cb.FunctionToolCall != nil:
        return &ClaudeContentBlock{Type: "tool_use", ID: cb.FunctionToolCall.CallID, Name: cb.FunctionToolCall.Name, Input: cb.FunctionToolCall.Arguments}
    case cb.ToolResult != nil:
        return &ClaudeContentBlock{Type: "tool_result", ToolUseID: cb.ToolResult.CallID, Content: cb.ToolResult.Output}
    default:
        return &ClaudeContentBlock{Type: "text", Text: ""}
    }
}
```

注意它不是根据 `cb.Type` switch，而是根据哪个指针字段非 nil switch。

这也是 tagged union 常见风险：

```text
如果 Type 和指针字段不一致，实际转换以指针字段为准。
```

所以创建 ContentBlock 时最好使用构造函数，不要手写结构体。

## 9. Gemini Adapter：最容易混淆的双路径

Gemini adapter 最复杂，因为它同时实现了 Message 路径和 AgenticMessage 路径。

### 9.1 原生结构

```go
type GeminiPart struct {
    Text             string
    InlineData       *GeminiInlineData
    FunctionCall     *GeminiFunctionCall
    FunctionResponse *GeminiFunctionResponse
}

type GeminiContent struct {
    Role  string
    Parts []*GeminiPart
}

type GeminiChatRequest struct {
    Contents []*GeminiContent
}
```

Gemini 的核心是：

```text
Content = role + parts[]
Part = text / inlineData / functionCall / functionResponse
```

### 9.2 双路径角色映射

Agentic 路径：

```go
func geminiRoleToAgentic(role string) AgenticRoleType {
    switch role {
    case "user":
        return AgenticRoleUser
    case "model":
        return AgenticRoleAssistant
    case "function":
        return AgenticRoleUser
    default:
        return AgenticRoleType(role)
    }
}
```

Message 路径：

```go
func geminiRoleToMessage(role string) RoleType {
    switch role {
    case "user":
        return User
    case "model":
        return Assistant
    case "function":
        return Tool
    default:
        return RoleType(role)
    }
}
```

同一个 Gemini role `"function"`：

```text
Agentic 路径 -> AgenticRoleUser
Message 路径 -> Tool
```

这是本章最容易错的点。

为什么会这样？

因为两条 canonical schema 的表达方式不同。

AgenticMessage 没有 Tool role，工具结果是一个 `ToolResult` block，通常放在 user role 消息里回给模型。

Message 有 Tool role，工具结果可以表示为：

```text
Message{Role: Tool, ToolCallID: ..., Content: ...}
```

所以 `"function"` 在两条路径里有不同语义落点。

### 9.3 Gemini -> AgenticMessage

```go
func ToCanonicalAgenticMessagesFromGemini(req *GeminiChatRequest) []*AgenticMessage {
    if req == nil {
        return nil
    }
    msgs := make([]*AgenticMessage, 0, len(req.Contents))
    for _, gc := range req.Contents {
        role := geminiRoleToAgentic(gc.Role)
        blocks := make([]*ContentBlock, 0, len(gc.Parts))
        for _, p := range gc.Parts {
            blocks = append(blocks, geminiPartToCanonical(p))
        }
        msgs = append(msgs, &AgenticMessage{Role: role, ContentBlocks: blocks})
    }
    return msgs
}
```

Part 转 block：

```go
func geminiPartToCanonical(p *GeminiPart) *ContentBlock {
    if p.InlineData != nil {
        return NewImageContentBlock("")
    }
    if p.FunctionCall != nil {
        argsJSON, _ := json.Marshal(p.FunctionCall.Args)
        return NewToolCallContentBlock(
            fmt.Sprintf("call_%s", p.FunctionCall.Name),
            p.FunctionCall.Name,
            string(argsJSON),
        )
    }
    if p.FunctionResponse != nil {
        respJSON, _ := json.Marshal(p.FunctionResponse.Response)
        return NewToolResultContentBlock(p.FunctionResponse.Name, string(respJSON))
    }
    return NewTextContentBlock(p.Text)
}
```

几个简化边界：

```text
InlineData -> NewImageContentBlock("")，没有保留 data
FunctionCall.CallID 人工生成为 call_<name>
FunctionResponse.Name 被用作 ToolResult.CallID
json.Marshal error 被忽略
```

这些都说明它是 fake adapter，不是生产 SDK adapter。

### 9.4 Gemini -> Message

Message 路径更像经典 Chat Completion：

```go
func ToCanonicalMessagesFromGemini(req *GeminiChatRequest) []*Message {
    if req == nil {
        return nil
    }
    msgs := make([]*Message, 0, len(req.Contents))
    for _, gc := range req.Contents {
        role := geminiRoleToMessage(gc.Role)
        var textContent string
        var toolCalls []ToolCall
        var toolCallID string
        for i, p := range gc.Parts {
            ...
        }
        msgs = append(msgs, &Message{
            Role:       role,
            Content:    textContent,
            ToolCalls:  toolCalls,
            ToolCallID: toolCallID,
        })
    }
    return msgs
}
```

Part 处理规则：

```text
InlineData        -> Content 追加 "[image: mime]"
FunctionCall      -> 追加 ToolCall
FunctionResponse  -> Content 追加 JSON 字符串，ToolCallID = response name
Text              -> Content 追加 text
```

这条路径会把部分 block 结构压平成 `Message.Content` 和 `Message.ToolCalls`。

### 9.5 Message -> Gemini

```go
func FromCanonicalMessagesToGemini(msgs []*Message) *GeminiChatRequest {
    contents := make([]*GeminiContent, 0, len(msgs))
    for _, m := range msgs {
        geminiRole := messageRoleToGemini(m.Role)
        var parts []*GeminiPart
        if m.Content != "" {
            parts = append(parts, &GeminiPart{Text: m.Content})
        }
        for _, tc := range m.ToolCalls {
            var args map[string]any
            _ = json.Unmarshal([]byte(tc.Function.Arguments), &args)
            parts = append(parts, &GeminiPart{
                FunctionCall: &GeminiFunctionCall{Name: tc.Function.Name, Args: args},
            })
        }
        if m.Role == Tool && m.ToolCallID != "" {
            var resp map[string]any
            _ = json.Unmarshal([]byte(m.Content), &resp)
            parts = append(parts, &GeminiPart{
                FunctionResponse: &GeminiFunctionResponse{Name: m.ToolCallID, Response: resp},
            })
        }
        if len(parts) == 0 {
            parts = append(parts, &GeminiPart{Text: ""})
        }
        contents = append(contents, &GeminiContent{Role: geminiRole, Parts: parts})
    }
    return &GeminiChatRequest{Contents: contents}
}
```

注意如果 `m.Content` 是非 JSON 字符串，但 `m.Role == Tool`，这里：

```go
_ = json.Unmarshal([]byte(m.Content), &resp)
```

失败会被忽略，`resp` 可能是 nil。

当前复刻版测试用的是简单可控数据，没有覆盖所有错误场景。生产实现应显式处理 parse error。

## 10. concat.go：流式合并和 Provider Metadata

虽然 outline 里说“复刻版无 ConcatMessages 流式合并”，但当前代码里已经有 `concat.go`，并实现了 `ConcatMessages`、`ConcatToolResults` 等。这里要按当前源码为准。

### 10.1 注册表

```go
var concatFuncRegistry sync.Map

func init() {
    RegisterStreamChunkConcatFunc(ConcatMessages)
    RegisterStreamChunkConcatFunc(ConcatMessageArray)
    RegisterStreamChunkConcatFunc(ConcatToolResults)
}
```

`ConcatItems[T]` 通过 `reflect.TypeOf(zero)` 找到注册的 concat 函数。

这和 Chapter 03 的 stream collect/concat 有关联。

### 10.2 Message 合并规则

`ConcatMessages` 的注释写得很清楚：

```text
Content: string concatenation
ReasoningContent: string concatenation
ToolCalls: group by Index, validate consistency, concat Arguments
MultiContent: append slices
ResponseMeta: keep last non-nil
Role: keep first non-zero
```

源码对应：

```go
if !firstRoleSet && chunk.Role != "" {
    result.Role = chunk.Role
    firstRoleSet = true
}
result.Content += chunk.Content
result.ReasoningContent += chunk.ReasoningContent
if chunk.ResponseMeta != nil {
    result.ResponseMeta = chunk.ResponseMeta
}
result.ToolCalls = append(result.ToolCalls, chunk.ToolCalls...)
...
```

这里有一个和 provider metadata 有关的重点：

```text
ResponseMeta: keep last non-nil
```

如果流式 chunk 每个都带不同 provider metadata，当前复刻版会保留最后一个非 nil `ResponseMeta`，不是深度合并每个扩展槽。

但是 `Extra` 是按 key 合并：

```go
for k, v := range chunk.Extra {
    result.Extra[k] = v
}
```

如果多个 chunk 的 `Extra` 有相同 key，后者覆盖前者。

这正好说明为什么类型化扩展槽位比 `map[string]any` 更安全：如果你要保留多个 provider 的不同元信息，应该放到不同类型槽，而不是共用一个扁平 key。

### 10.3 ToolCall 合并

工具调用合并走：

```go
result.ToolCalls, err = concatToolCalls(result.ToolCalls)
```

`concatToolCalls` 会分两类：

```text
Index == nil  -> unindexed，直接保留
Index != nil  -> 按 Index 分组，组内 merge
```

组内 merge 会校验：

```text
ID 相同
Type 相同
Function.Name 相同
```

然后拼接：

```go
merged.Function.Arguments += tc.Function.Arguments
```

所以流式 function calling 的典型情况是：

```text
chunk1: Index=0, Arguments="{\"loc"
chunk2: Index=0, Arguments="ation\":\"NYC\"}"
merge:  Arguments="{\"location\":\"NYC\"}"
```

测试 `TestConcatMessages_ToolCalls` 覆盖了这个场景。

### 10.4 Index 的双重语义

`Index` 在内存中非常重要。

没有它，就无法知道两个 tool call delta 是同一个工具调用的两段，还是两个不同工具调用。

但 `Index` 又不会 JSON 序列化。

所以要记住：

```text
Index 是流式合并的运行时辅助字段，不是跨 JSON 持久化协议。
```

如果你要把流式中间态持久化，当前 `ToolCall` 的 JSON 标签还不够，需要额外设计。

## 11. Provider Adapter 的职责边界

Provider Adapter 容易被误解成“模型调用层”。

但当前复刻版里它不是。

它不负责：

```text
发 HTTP 请求
管理 API key
重试
限流
流式网络读取
Graph 调度
Tool 执行
Prompt 格式化
Callback 注入
```

它只负责：

```text
原生请求/消息结构 -> canonical schema
canonical schema -> 原生请求/消息结构
```

Fake provider 也只验证 schema 边界：

```go
type FakeOpenAIProvider struct{}
type FakeClaudeProvider struct{}
type FakeGeminiProvider struct{}
```

例如：

```go
func (p *FakeOpenAIProvider) ToCanonicalMessages(req *OpenAIChatRequest) ([]*Message, error)
func (p *FakeOpenAIProvider) FromCanonicalMessages(msgs []*Message) (*OpenAIChatRequest, error)
```

它不会调用 OpenAI API。

这和 `FakeChatModel` 类似：都是为了测试架构边界，不是为了模拟真实网络 provider。

## 12. 三个 provider 的对比表

| Provider | 原生消息单位 | 原生内容单位 | 当前 canonical 路径 | 工具调用位置 | 工具结果位置 |
| --- | --- | --- | --- | --- | --- |
| OpenAI | `OpenAIMessage` | `Content string` + `ToolCalls` | `Message` | `Message.ToolCalls` | `Tool` role message |
| Claude | `ClaudeMessage` | `ClaudeContentBlock[]` | `AgenticMessage` | `ContentBlock.FunctionToolCall` | `ContentBlock.ToolResult` |
| Gemini | `GeminiContent` | `GeminiPart[]` | `Message` 和 `AgenticMessage` | Message: `ToolCalls`; Agentic: block | Message: `Tool` role; Agentic: block |

角色映射对比：

| Provider role | Message 路径 | AgenticMessage 路径 |
| --- | --- | --- |
| OpenAI `system` | `System` | 当前复刻版未实现 |
| OpenAI `user` | `User` | 当前复刻版未实现 |
| OpenAI `assistant` | `Assistant` | 当前复刻版未实现 |
| OpenAI `tool` | `Tool` | 当前复刻版未实现 |
| Claude `user` | 当前复刻版未实现 | `AgenticRoleUser` |
| Claude `assistant` | 当前复刻版未实现 | `AgenticRoleAssistant` |
| Gemini `user` | `User` | `AgenticRoleUser` |
| Gemini `model` | `Assistant` | `AgenticRoleAssistant` |
| Gemini `function` | `Tool` | `AgenticRoleUser` |

这张表就是本章最该记住的东西。

## 13. 典型链路 1：OpenAI -> Message -> ChatModel -> OpenAI

测试 `TestCanonicalMessageFromOpenAIChatModel` 走的是：

```text
OpenAIChatRequest
  -> ToCanonicalMessages
  -> FakeChatModel.Generate
  -> Message
```

代码大意：

```go
req := &OpenAIChatRequest{Model: "gpt-4", Messages: []*OpenAIMessage{
    {Role: "system", Content: "Be helpful."},
    {Role: "user", Content: "What is Rive?"},
}}

cm := NewFakeChatModel(WithChatGenerateFunc(func(ctx context.Context, input []*Message) (*Message, error) {
    return AssistantMessage("Rive is an agent team runtime."), nil
}))

resp, err := cm.Generate(context.Background(), ToCanonicalMessages(req))
```

关键是 `FakeChatModel` 的输入是 `[]*Message`。

它不需要知道输入最初来自 OpenAI。

如果换成 Gemini Message 路径，只要也转成 `[]*Message`，模型节点仍然可以复用。

这就是 canonical schema 对 Graph 的价值。

## 14. 典型链路 2：Claude -> AgenticMessage -> Tool

测试 `TestCanonicalAgenticMessageFromClaudeWithTool` 走的是：

```text
ClaudeChatRequest
  -> ToCanonicalAgenticMessages
  -> AgenticMessageToolCalls
  -> BridgeTool.Execute
```

Claude 原生消息：

```text
assistant:
  text "Checking..."
  tool_use id=toolu_01 name=get_weather input={city: Paris}
```

转成 AgenticMessage 后，工具调用在：

```text
ams[1].ContentBlocks[1].FunctionToolCall
```

提取：

```go
calls := AgenticMessageToolCalls(ams[1])
```

然后执行工具：

```go
tool := NewBridgeTool("get_weather", func(ctx context.Context, args map[string]any) (string, error) {
    return "Sunny, 22C", nil
})
```

这条链路说明：Claude 不需要先变成 OpenAI 风格的 `Message.ToolCalls`，也能表达工具调用。

## 15. 典型链路 3：Gemini full pipeline

测试 `TestGeminiFullPipeline` 是最综合的。

输入：

```go
req := &GeminiChatRequest{Contents: []*GeminiContent{
    {Role: "user", Parts: []*GeminiPart{{Text: "Weather?"}}},
    {Role: "model", Parts: []*GeminiPart{
        {FunctionCall: &GeminiFunctionCall{Name: "get_weather", Args: map[string]any{"city": "Tokyo"}}},
    }},
}}
```

流程：

```text
Gemini native request
  -> ToCanonicalAgenticMessagesFromGemini
  -> AgenticMessageToolCalls
  -> BridgeTool.Execute
  -> NewToolResultContentBlock
  -> append AgenticMessage{Role: user, ToolResult block}
  -> FromCanonicalAgenticMessagesToGemini
```

最后断言：

```go
if len(rt.Contents) != 3 || rt.Contents[2].Role != "user" {
    t.Fatalf("full pipeline failed")
}
```

这体现了 Gemini Agentic 路径里的工具结果语义：

```text
function response 回给模型时，外层 role 是 user
```

这和 Message 路径中的 `Tool` role 不一样。

## 16. 容易误解点

### 16.1 “Provider Adapter 会调用真实模型”

不会。

当前 provider adapter 只做结构转换。

真实模型调用在 ChatModel 实现里，而当前复刻版只有 `FakeChatModel`。

### 16.2 “Canonical Schema 就是一个大一统 Message”

不是。

当前复刻版至少有两条 canonical message 路径：

```text
Message
AgenticMessage
```

不同 provider 可以选择最适合的路径。

### 16.3 “AgenticMessage 是 Message 的子集”

不是。

AgenticMessage 用 content blocks 表达结构，Message 用顶层字段表达工具调用。它们不是简单包含关系。

当前复刻版也没有提供 `Message <-> AgenticMessage` 的通用桥接函数。

### 16.4 “Gemini function role 总是 Tool”

不是。

在 Message 路径里：

```text
Gemini "function" -> Tool
```

在 AgenticMessage 路径里：

```text
Gemini "function" -> User
```

这是由目标 schema 决定的。

### 16.5 “ParamsOneOf 两种 schema 会合并”

不会。

`params != nil` 时，`jsonSchema` 被忽略。

用构造函数避免双重设置。

### 16.6 “ToolCall.Index 会被 JSON 保留”

不会。

`Index` 是 `json:"-"`。

它只适合内存中的流式 chunk 合并。

### 16.7 “ResponseMeta 会深度合并”

当前复刻版不会。

`ConcatMessages` 里是：

```text
keep last non-nil ResponseMeta
```

不是把 OpenAIExtension、ClaudeExtension、GeminiExtension 分别深度合并。

### 16.8 “Extra 能安全承载 provider metadata”

谨慎。

`Extra` 是 map，合并时同名 key 会覆盖。

Provider 特有响应元数据优先放类型化扩展槽位。

### 16.9 “Claude tool_use 的 Input 一定是 JSON”

当前复刻版里不是。

`claudeBlockToCanonical` 使用 `fmt.Sprintf` 转字符串，map 会变成 Go 的 `map[...]` 形式。

生产 adapter 应该保留 JSON。

### 16.10 “Type 字段一定决定 ContentBlock 类型”

当前转换函数多半看指针字段是否非 nil，而不只看 `Type`。

所以不要手写不一致的 ContentBlock：

```go
ContentBlock{
    Type: ContentBlockTypeFunctionToolCall,
    UserInputText: &text,
}
```

这会让转换语义变得混乱。

## 17. 源码阅读顺序

### 17.1 第一遍：schema.go

先读：

```text
ToolCall
ToolCallFunction
ToolInfo
ParamsOneOf
ParameterInfo
ToolResult
Document
```

同时读 `schema_test.go`。

目标：

```text
知道工具调用、工具信息、参数 schema、工具结果、文档如何被规范化。
```

### 17.2 第二遍：chatmodel.go 的 Message / ResponseMeta

读：

```text
RoleType
Message
ResponseMeta
TokenUsage
OpenAIRespMetaExtension
ClaudeRespMetaExtension
GeminiRespMetaExtension
```

同时读 `chatmodel_test.go` 后半部分。

目标：

```text
知道经典 Message 路径如何承载工具调用和 provider 响应元数据。
```

### 17.3 第三遍：provider.go

读：

```text
ContentBlockType
ContentBlock
AgenticMessage
NewTextContentBlock
NewToolCallContentBlock
AgenticMessageToolCalls
ProviderOpenAI / ProviderClaude / ProviderGemini
```

目标：

```text
理解 content block 联合体和三类 provider 接口。
```

### 17.4 第四遍：OpenAI adapter

读：

```text
provider_openai.go
provider_test.go: OpenAI tests
```

目标：

```text
看最简单的一一映射。
```

### 17.5 第五遍：Claude adapter

读：

```text
provider_claude.go
provider_test.go: Claude tests
```

目标：

```text
理解 block array 如何映射到 ContentBlock。
```

### 17.6 第六遍：Gemini adapter

读：

```text
provider_gemini.go
provider_test.go: Gemini tests
```

目标：

```text
重点掌握 Message 路径和 AgenticMessage 路径的不同角色映射。
```

### 17.7 第七遍：concat.go

读：

```text
ConcatMessages
concatToolCalls
mergeToolCallGroup
ConcatToolResults
concat_test.go
```

目标：

```text
理解流式 chunk 如何合并，以及哪些 metadata 会被保留或覆盖。
```

## 18. 练习题

### 练习 1：写 OpenAI 往返步骤

写出这条链的 6 个步骤：

```text
OpenAIChatRequest
  -> ToCanonicalMessages
  -> FakeChatModel.Generate
  -> Assistant Message
  -> append 到消息列表
  -> FromCanonicalMessages
  -> OpenAIChatRequest
```

要求说明每一步的数据类型。

### 练习 2：给天气工具写 ParamsOneOf

实现：

```text
city: string, required
unit: string, enum ["celsius", "fahrenheit"]
```

调用 `ToJSONSchema()`，写出输出结构。

### 练习 3：解释 Gemini 双路径

回答：

```text
为什么 Gemini "function" 在 Message 路径是 Tool，但在 AgenticMessage 路径是 User？
```

要求结合两种 canonical schema 的工具结果表达方式说明。

### 练习 4：修正 Claude tool_use 参数保真

当前：

```go
inputStr = fmt.Sprintf("%v", cb.Input)
```

改成更接近生产的：

```go
data, err := json.Marshal(cb.Input)
```

思考：

```text
err 怎么处理？
nil Input 怎么处理？
```

### 练习 5：实现 ServerToolCallBlock 构造函数

当前已有：

```go
type ServerToolCallBlock struct {
    CallID string
    Name   string
    Args   map[string]any
}
```

写一个：

```go
func NewServerToolCallContentBlock(callID, name string, args map[string]any) *ContentBlock
```

要求：

```text
Type = ContentBlockTypeServerToolCall
ServerToolCall 非 nil
```

### 练习 6：测试 ToolCall Index 的 JSON 丢失

构造：

```go
idx := 0
tc := ToolCall{Index: &idx, ...}
```

JSON marshal/unmarshal 后检查：

```text
restored.Index == nil
```

然后解释这对流式中间态持久化意味着什么。

### 练习 7：写一个 ResponseMeta 合并策略

当前 `ConcatMessages` 只保留最后一个非 nil `ResponseMeta`。

设计一个新函数：

```go
MergeResponseMeta(a, b *ResponseMeta) *ResponseMeta
```

要求：

```text
公共字段以后者为准
OpenAIExtension / ClaudeExtension / GeminiExtension 分别保留最新非 nil
Usage 合并或以后者为准，需要说明策略
```

### 练习 8：为 Gemini image 保留数据

当前：

```go
if p.InlineData != nil {
    return NewImageContentBlock("")
}
```

改为保留：

```text
mime type
data
```

思考当前 `MessageInputImage` 只有 `URL` 和 `Detail`，是否需要新增字段或用 data URL。

## 19. 自测问题

1. Provider Adapter 的核心职责是什么？
2. 当前复刻版有哪些 canonical message 模型？
3. `Message.ToolCalls` 和 `AgenticMessage.ContentBlocks[].FunctionToolCall` 的区别是什么？
4. OpenAI adapter 为什么最简单？
5. Claude adapter 为什么不实现 `ToCanonicalMessages`？
6. Gemini 为什么同时实现 Message 和 AgenticMessage 两条路径？
7. Gemini `"function"` 在两条路径里分别映射成什么？
8. `ParamsOneOf.ToJSONSchema()` 在 `params` 和 `jsonSchema` 都存在时优先返回哪一个？
9. `ToolCall.Index` 为什么 JSON 往返后会丢失？
10. `ResponseMeta` 为什么要有 OpenAI / Claude / Gemini 类型化扩展槽？
11. `ConcatMessages` 如何合并多个 ToolCall chunk？
12. `ConcatMessages` 如何处理多个非 nil ResponseMeta？
13. `AgenticMessageToolCalls` 从哪里提取工具调用？
14. `ContentBlock` 的 Type 字段和指针字段不一致时，当前转换函数更依赖哪一个？
15. 当前 fake provider 和真实 provider adapter 的最大区别是什么？

参考答案要点：

1. 原生 provider 类型与 canonical schema 的双向转换。
2. `Message` 和 `AgenticMessage`。
3. 前者是经典消息顶层字段，后者是 content block 联合体的一种 block。
4. OpenAI 原生结构和 `Message` 几乎一一对应。
5. 当前复刻版选择 Claude 走 AgenticMessage 路径，因为 Claude 原生 content 是 block 数组。
6. Gemini parts 既可压平成 Message，也可保留为 AgenticMessage blocks。
7. Message 路径为 `Tool`，AgenticMessage 路径为 `AgenticRoleUser`。
8. `params`。
9. `Index` 标记为 `json:"-"`。
10. 避免 provider 特有字段互相覆盖，同时保持类型安全。
11. 按 `Index` 分组，校验 ID/Type/Name 一致，拼接 `Arguments`。
12. 保留最后一个非 nil。
13. 遍历 `ContentBlocks`，收集 `FunctionToolCall != nil` 的 block。
14. 多数转换函数更依赖非 nil 指针字段。
15. fake provider 只做结构转换和测试，不做网络请求、鉴权、重试、真实流式解析。

## 20. 本章总结

Chapter 06 的核心可以压缩成一句话：

```text
Provider Adapter 把不同厂商的原生消息协议挡在边界外，Graph 和组件只面对 canonical schema。
```

你需要记住三层：

```text
schema.go
  工具、参数、文档等共享 schema

chatmodel.go
  Message + ResponseMeta，适合经典 Chat Completion 路径

provider.go
  AgenticMessage + ContentBlock，适合 block/part/agentic 路径
```

你还需要记住三家 provider 的差异：

```text
OpenAI   最接近 Message
Claude   最接近 AgenticMessage
Gemini   两条路径都能演示，角色映射最容易混淆
```

最后记住边界：

```text
当前复刻版 provider 都是 fake adapter。
它们验证的是 schema 防火墙和双向转换思路，不是生产级 SDK 集成。
生产实现需要补齐真实 JSON 保真、provider option、错误处理、流式 delta 解析、metadata 深度合并和鉴权网络层。
```

读完这一章，你应该能理解为什么下一章的 ReAct / MultiAgent 可以不关心 OpenAI、Claude、Gemini 的原生消息格式。因为进入 Agent 之前，这些差异已经被 Provider Adapter 收敛到了 `Message` 或 `AgenticMessage`。

