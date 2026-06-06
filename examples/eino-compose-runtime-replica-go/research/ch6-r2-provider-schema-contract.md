# R2 研究笔记：Provider Schema 契约与多适配器互操作设计

> 基于 Eino 技术手册第六章（06-schema-provider-adapters.md）与当前 Go 复刻版源码审计
> 目标读者：实施工人（负责 Schema 层 / Provider 适配器互操作）
> 语言：中文
> 状态：研究提案 — 定义教育最小契约，不修改生产 Go 代码

---

## 目录

1. [问题域与设计目标](#1-问题域与设计目标)
2. [规范消息类型：Message vs AgenticMessage](#2-规范消息类型message-vs-agenticmessage)
3. [ContentBlock 类型联合体](#3-contentblock-类型联合体)
4. [ToolCall / ToolResult / ResponseMeta / Document](#4-toolcall--toolresult--responsemeta--document)
5. [Provider 扩展元数据槽位](#5-provider-扩展元数据槽位)
6. [Stream Concat 注册表与 Message Concat 行为](#6-stream-concat-注册表与-message-concat-行为)
7. [OpenAI 适配器骨架](#7-openai-适配器骨架)
8. [Claude 适配器骨架](#8-claude-适配器骨架)
9. [Gemini 适配器骨架](#9-gemini-适配器骨架)
10. [与复刻版现状的差距分析](#10-与复刻版现状的差距分析)
11. [教育子集实现路径](#11-教育子集实现路径)
12. [附录：关键源码对照](#附录关键源码对照)

---

## 1. 问题域与设计目标

### 1.1 核心问题

Eino 是一个多 Provider 的 LLM 应用框架。用户组合一个 Graph，可能在同一个流水线中使用 OpenAI 进行 Chat Completion，使用 Claude 进行推理，使用 Gemini 进行 Embedding。每个 Provider 使用不同的线格式（wire format），有不同的 Message 结构、不同的流式协议和不同的响应元数据。

如果每个 Graph 节点都知道自己与哪个 Provider 通信，那么切换 Provider 就需要重写每个节点。如果 `compose/`（编排引擎）根据 Provider 名称进行分支，那么引擎就不再是通用的。

**核心命题**：如何让来自不同 Provider 的组件在同一个流水线中互操作，而没有任何组件知道其他组件的存在？

### 1.2 三层架构解

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

| # | 决策 | 说明 |
|---|------|------|
| 1 | 两种 Message 模型 | `Message`（经典 Chat）+ `AgenticMessage`（ContentBlock 体系） |
| 2 | Provider 扩展是类型化结构体字段 | 不是 `map[string]any`，而是 `*openai.ResponseMetaExtension` 等 |
| 3 | 泛型接口封闭为两种类型 | `BaseModel[M messageType]` 只接受 `*Message` 和 `*AgenticMessage` |
| 4 | StreamReader[T] 作为通用流式基元 | 支持 Channel/数组/扇入/扇出/类型转换五种后端 |
| 5 | 注册式 Concat 派发 | `RegisterStreamChunkConcatFunc[T]` 构建 `reflect.Type → func` 映射 |

### 1.3 Provider 格式差异一览

| 维度 | OpenAI | Claude | Gemini |
|------|--------|--------|--------|
| **角色名称** | `"assistant"` | `"assistant"` | `"model"` |
| **多模态部件** | `content: [{type:"text", text:"..."}, {type:"image_url", image_url:{...}}]` | `content: [{type:"text", text:"..."}, {type:"image", source:{...}}]` | `parts: [{text:"..."}, {inlineData:{...}}]` |
| **工具调用** | `tool_calls[]`，基于索引的流式 delta | `tool_use` 内容块，内嵌在 Message 内容中 | `functionCall`，内嵌在 `parts[]` 中 |
| **工具结果** | 角色为 `"tool"` 的 Message，带 `tool_call_id` | `user` Message 中的 `tool_result` 内容块 | `user` 角色中的 `functionResponse` 部件 |
| **推理** | 用量详情中的 `reasoning_tokens` | Thinking 内容块 | `thought` 部件 |
| **响应 ID** | `response.id` | `message.id` | `candidates[0].content.parts` 联合体 |

---

## 2. 规范消息类型：Message vs AgenticMessage

### 2.1 为什么需要两种模型

`Message` 和 `AgenticMessage` 服务于不同的语义层：

- **`Message`**：经典 ChatMessage 模型。角色驱动（User/Assistant/System/Tool），工具调用通过 `ToolCalls[]` 字段，工具结果通过 `Tool` 角色消息 + `ToolCallID` 回指。向后兼容。
- **`AgenticMessage`**：基于 ContentBlock 的模型。工具调用和工具结果都是同一消息中的内容块（无需单独的 `Tool` 角色）。MCP 工具、服务端工具搜索、审批流等高级语义由专用块类型承载。

两种模型通过 Go 泛型约束共存：

```go
type messageType interface {
    *schema.Message | *schema.AgenticMessage
}

type BaseModel[M messageType] interface {
    Generate(ctx context.Context, input []M, opts ...Option) (M, error)
    Stream(ctx context.Context, input []M, opts ...Option) (*schema.StreamReader[M], error)
}
```

### 2.2 `Message` — 经典消息模型（教育最小契约）

```go
// schema/message.go

// RoleType 角色枚举
type RoleType string

const (
    Assistant RoleType = "assistant" // 模型回复
    User      RoleType = "user"      // 用户或外部输入
    System    RoleType = "system"    // 系统指令
    Tool      RoleType = "tool"      // 工具执行结果
)

// Message 经典消息模型。
// 角色驱动：assistant 携带 ToolCalls，tool 携带 ToolCallID 回指。
type Message struct {
    Role             RoleType              // 发送方角色
    Content          string                // 纯文本内容
    UserInputMultiContent   []MessageInputPart   // 用户多模态输入（可选）
    AssistantGenMultiContent []MessageOutputPart  // 模型多模态输出（可选）
    ToolCalls        []ToolCall            // assistant：请求的工具调用
    ToolCallID       string                // tool：回指原始工具调用 ID
    ToolName         string                // tool：响应工具的名称
    ResponseMeta     *ResponseMeta         // finish_reason、usage、logprobs
    ReasoningContent string                // 思考/推理内容
    Extra            map[string]any        // 遗留扩展（不推荐，优先使用 Provider 扩展槽位）
}
```

**角色语义表**：

| Role | 发送方 | 包含内容 |
|------|--------|---------|
| `User` | 用户 / Retriever / ToolsNode 结果合并 | 问题文本、文档内容、多模态输入 |
| `Assistant` | ChatModel | 文本回复、ToolCalls[]、多模态输出 |
| `System` | 开发者 | 系统级指令 |
| `Tool` | ToolsNode | 工具执行结果 (`ToolCallID` + `Content`) |

**多模态部件**：

```go
// MessageInputPart 用户/外部输入的多模态联合体
type MessageInputPart struct {
    Type ChatMessagePartType
    // 以下指针——同一时间只有一个非 nil
    Text             *string
    Image            *MessageInputImage   // ImageURL
    Audio            *MessageInputAudio   // AudioURL
    Video            *MessageInputVideo   // VideoURL
    File             *MessageInputFile    // FileURL
    ToolSearchResult *ToolSearchResult
}

type MessageInputImage struct {
    MessagePartCommon    // URL
    Detail     string    // "low" | "high" | "auto" (OpenAI)
}

// MessageOutputPart 模型输出的多模态联合体
type MessageOutputPart struct {
    Type ChatMessagePartType
    Text      *string
    Image     *MessageOutputImage   // base64 或 URL
    Audio     *MessageOutputAudio
    Video     *MessageOutputVideo
    Reasoning *string               // 思考内容
}
```

### 2.3 `AgenticMessage` — ContentBlock 模型（教育最小契约）

```go
// AgenticRoleType 角色枚举 — 与 Message 的关键不同：没有 "tool" 角色
type AgenticRoleType string

const (
    AgenticRoleAssistant AgenticRoleType = "assistant"
    AgenticRoleUser      AgenticRoleType = "user"
    AgenticRoleSystem    AgenticRoleType = "system"
)

// AgenticMessage 基于 ContentBlock 的消息模型。
// 工具调用和工具结果都是 user/assistant 消息内的内容块，无需单独的 tool 角色。
type AgenticMessage struct {
    Role         AgenticRoleType
    ContentBlocks []*ContentBlock            // 类型化块的有序列表
    ResponseMeta *AgenticResponseMeta        // token 用量 + provider 扩展槽位
    Extra        map[string]any
}

// AgenticResponseMeta 携带 provider 特定的类型化扩展槽位
type AgenticResponseMeta struct {
    TokenUsage      *TokenUsage
    OpenAIExtension *openai.ResponseMetaExtension    // nil = 非 OpenAI provider
    GeminiExtension *gemini.ResponseMetaExtension    // nil = 非 Gemini provider
    ClaudeExtension  *claude.ResponseMetaExtension   // nil = 非 Claude provider
    Extension       any                                // 未知/自定义 provider 的回退
}

// TokenUsage 通用用量统计
type TokenUsage struct {
    PromptTokens     int
    CompletionTokens int
    TotalTokens      int
    ReasoningTokens  int  // 思考 token（OpenAI o-series 等）
}
```

**AgenticMessage 的关键创新**：
- 没有单独的 `Tool` 角色消息 — 工具结果是 `user` 角色中的内容块
- 这更自然地映射到 Claude 和 Gemini（工具结果是 user 回合的内容）以及 OpenAI Responses API（工具结果是内联的）
- Provider 扩展槽位是**类型化指针**：未被使用的 provider → nil；被使用的 provider → 具体值。组件代码或 `Extra map[string]any` 需要检查 nil

---

## 3. ContentBlock 类型联合体

### 3.1 设计原则

ContentBlock 是 `AgenticMessage` 的核心构建块。它是一个**标记联合体**（tagged union）：每种变体都是一个可空的指针字段，仅有一个字段在同一时间非 nil。

```go
type ContentBlockType string

const (
    // 用户输入块
    ContentBlockTypeUserInputText   ContentBlockType = "user_input_text"
    ContentBlockTypeUserInputImage  ContentBlockType = "user_input_image"
    ContentBlockTypeUserInputAudio  ContentBlockType = "user_input_audio"
    ContentBlockTypeUserInputVideo  ContentBlockType = "user_input_video"
    ContentBlockTypeUserInputFile   ContentBlockType = "user_input_file"

    // 模型输出块
    ContentBlockTypeAssistantGenText  ContentBlockType = "assistant_gen_text"
    ContentBlockTypeAssistantGenImage ContentBlockType = "assistant_gen_image"
    ContentBlockTypeAssistantGenAudio ContentBlockType = "assistant_gen_audio"
    ContentBlockTypeAssistantGenVideo ContentBlockType = "assistant_gen_video"

    // 推理块
    ContentBlockTypeReasoning ContentBlockType = "reasoning"

    // 工具调用块（三种粒度）
    ContentBlockTypeFunctionToolCall ContentBlockType = "function_tool_call"
    ContentBlockTypeServerToolCall   ContentBlockType = "server_tool_call"
    ContentBlockTypeMCPToolCall      ContentBlockType = "mcp_tool_call"

    // 工具结果块
    ContentBlockTypeFunctionToolResult ContentBlockType = "function_tool_result"
    ContentBlockTypeServerToolResult   ContentBlockType = "server_tool_result"
    ContentBlockTypeMCPToolResult      ContentBlockType = "mcp_tool_result"

    // MCP 协议块
    ContentBlockTypeMCPListToolsResult     ContentBlockType = "mcp_list_tools_result"
    ContentBlockTypeMCPToolApprovalRequest  ContentBlockType = "mcp_tool_approval_request"
    ContentBlockTypeMCPToolApprovalResponse ContentBlockType = "mcp_tool_approval_response"

    // 工具搜索
    ContentBlockTypeToolSearchResult ContentBlockType = "tool_search_result"
)
```

### 3.2 ContentBlock 结构（教育最小子集）

```go
// ContentBlock 标记联合体 — 恰好一个变体指针非 nil
type ContentBlock struct {
    Type ContentBlockType
    // 以下指针中只有一个在同一时间为非 nil
    UserInputText         *string
    UserInputImage        *MessageInputImage
    UserInputAudio        *MessageInputAudio
    UserInputVideo        *MessageInputVideo
    UserInputFile         *MessageInputFile
    AssistantGenText      *AssistantGenText      // 含 Provider 扩展槽位
    AssistantGenImage     *MessageOutputImage
    AssistantGenAudio     *MessageOutputAudio
    AssistantGenVideo     *MessageOutputVideo
    Reasoning             *string
    FunctionToolCall      *FunctionToolCall
    ServerToolCall        *ServerToolCall
    MCPToolCall           *MCPToolCall
    FunctionToolResult    *FunctionToolResult
    ServerToolResult      *ServerToolResult
    MCPToolResult         *MCPToolResult
    MCPListToolsResult    *MCPListToolsResult
    MCPToolApprovalRequest  *MCPToolApprovalRequest
    MCPToolApprovalResponse *MCPToolApprovalResponse
    ToolSearchResult      *ToolSearchResult
    // 流式控制信息
    StreamingMeta         *StreamingMeta     // {Index int} — 标识属于哪个逻辑调用
}

// StreamingMeta 流式拼接控制。
// 每个流式 chunk 的 ContentBlock 携带一个 Index。
// ConcatAgenticMessages 按 Index 分组 chunk，并通过类型特定的函数合并。
type StreamingMeta struct {
    Index int
}

// AssistantGenText 文本输出块，携带 Provider 扩展
type AssistantGenText struct {
    Content         string
    OpenAIExtension *openai.AssistantGenTextExtension   // 注解/引用
    ClaudeExtension  *claude.AssistantGenTextExtension   // 引用
}
```

### 3.3 ContentBlock 变体设计意图

| 块类别 | 变体 | 用途 |
|--------|------|------|
| **用户输入** | UserInputText/Image/Audio/Video/File | 多模态输入，来源为用户或上游节点 |
| **模型输出** | AssistantGenText/Image/Audio/Video | 模型的生成结果 |
| **推理** | Reasoning | 模型思考过程（"chain of thought"） |
| **工具调用** | FunctionToolCall | 标准函数调用（OpenAI function calling） |
| | ServerToolCall | 服务端搜索工具（动态发现） |
| | MCPToolCall | Model Context Protocol 工具调用 |
| **工具结果** | FunctionToolResult | 标准工具执行结果 |
| | ServerToolResult | 服务端工具结果 |
| | MCPToolResult | MCP 工具结果 |
| **MCP 协议** | MCPListToolsResult | 列出可用 MCP 工具 |
| | MCPToolApprovalRequest/Response | 工具调用审批流 |
| **搜索** | ToolSearchResult | 工具搜索匹配结果 |

### 3.4 工具调用/结果块的结构（教育子集）

```go
// FunctionToolCall 标准函数工具调用
type FunctionToolCall struct {
    CallID    string // 唯一标识，用于匹配工具结果
    Name      string // 工具名
    Arguments string // JSON 格式参数
}

// FunctionToolResult 标准函数工具结果
type FunctionToolResult struct {
    CallID string // 回指 FunctionToolCall.CallID
    Output string // 工具输出文本
}

// ServerToolCall 服务端工具调用（工具在服务端动态发现）
type ServerToolCall struct {
    CallID string
    Name   string
    Args   map[string]any  // 结构化参数，非 JSON 字符串
}

// MCPToolCall MCP 协议工具调用
type MCPToolCall struct {
    CallID     string
    ServerName string
    ToolName   string
    Arguments  string
}
```

---

## 4. ToolCall / ToolResult / ResponseMeta / Document

### 4.1 ToolCall（Message 模型中的工具调用）

```go
// ToolCall 描述模型请求的一个工具调用。
// 在流式模式下，Index 标识 deltas 属于同一个逻辑调用。
type ToolCall struct {
    Index    *int           // 流式控制：同一 Index 的 chunk 属于同一调用
    ID       string         // 唯一标识
    Type     string         // 固定为 "function"（标准工具调用）
    Function ToolCallFunction
    Extra    map[string]any // 遗留扩展
}

type ToolCallFunction struct {
    Name      string // 工具名
    Arguments string // JSON 字符串参数
}
```

**Index 在流式中的关键作用**：
- Provider 适配器在流式模式下发出部分 chunk
- 同一工具调用的所有 chunk 共享相同的 `Index`
- `ConcatMessages` → `concatToolCalls` 按 `Index` 分组
- 每组内验证 ID / Type / Name 一致，拼接 `Arguments` JSON 片段
- 如果 Provider 适配器将每个 chunk 的 `Index` 都设为 `0`，则所有 chunk 被错误合并为一个调用

### 4.2 ToolResult / ResponseMeta

```go
// ToolResult 工具执行结果（教育子集：仅文本输出）
type ToolResult struct {
    Text string // 工具输出纯文本
}

// ResponseMeta 同步/流式调用结束时的响应元数据
type ResponseMeta struct {
    ID           string      // 提供商响应 ID
    Model        string      // 使用的模型名称
    FinishReason string      // "stop" | "length" | "tool_calls" | "content_filter" | "function_call"
    Usage        *TokenUsage // 用量统计
    LogProbs     *LogProbs   // 对数概率（如模型支持）
    // Provider 扩展槽位 — 类型化 nil 指针模式
    OpenAIExtension *openai.ResponseMetaExtension
    GeminiExtension *gemini.ResponseMetaExtension
    ClaudeExtension  *claude.ResponseMetaExtension
    Extension       any  // 未知 provider 的回退
}

// LogProbs token 级对数概率
type LogProbs struct {
    Content []*LogProbInfo
}

type LogProbInfo struct {
    Token       string
    LogProb     float64
    Bytes       []int32
    TopLogProbs map[string]float64
}
```

### 4.3 Document（检索/索引文档）

```go
// Document 检索结果文档
type Document struct {
    ID        string         // 后端分配的唯一标识
    Content   string         // 文档文本内容
    Meta      map[string]any // 元数据（来源 URL、作者、时间戳等）
    Embedding []float64      // 向量嵌入（可选，由 Embedder 生成）
    Score     float64        // 检索相关性分数
}
```

**Document 契约关键点**：
- `ID` 由 Indexer 的 `Store` 返回，Retriever 的 `Retrieve` 返回的文档带上此 ID
- `Embedding` 在存储时由 `indexer.Options.Embedding` 指定的 Embedder 生成；检索时 `retriever.Options.Embedding` 必须是同一 Embedder，否则向量空间不匹配
- `Score` 由 retriever 后端计算；`ScoreThreshold` 选项过滤低分文档
- `Meta` 是唯一携带 Provider 特定元数据的位置（`map[string]any`，不享受类型安全）

### 4.4 ToolInfo — 双模式参数 Schema

```go
// ToolInfo 描述工具的元数据，用于向模型注册可用工具
type ToolInfo struct {
    Name        string
    Desc        string
    ParamsOneOf *ParamsOneOf  // 两种模式恰好选其一
    Extra       map[string]any
}

// ParamsOneOf 两选一：轻量级 Params 或完整 JSON Schema
type ParamsOneOf struct {
    params      map[string]*ParameterInfo   // 模式 1：轻量级
    jsonSchema  *jsonschema.Schema          // 模式 2：JSON Schema 2020-12
}

// NewParamsOneOfByParams 创建轻量级参数模式（用于简单工具）
func NewParamsOneOfByParams(params map[string]*ParameterInfo) *ParamsOneOf

// NewParamsOneOfByJSONSchema 创建完整 JSON Schema 模式（用于 anyOf/oneOf/$defs）
func NewParamsOneOfByJSONSchema(s *jsonschema.Schema) *ParamsOneOf

// ToJSONSchema 将两种模式标准化为 *jsonschema.Schema（Provider 适配器调用此方法）
func (p *ParamsOneOf) ToJSONSchema() *jsonschema.Schema

// ParameterInfo 轻量级参数描述
type ParameterInfo struct {
    Type      string                   // "string" | "number" | "boolean" | "object" | "array"
    ElemInfo  *ParameterInfo           // 数组元素类型
    SubParams map[string]*ParameterInfo // 嵌套对象字段
    Desc      string
    Enum      []string
    Required  bool
}
```

**关键约束**：
- `ParamsOneOf` 只能有一种模式 — `params` 或 `jsonschema`，不能两者兼有
- 若先 `NewParamsOneOfByParams` 再 `NewParamsOneOfByJSONSchema` 覆盖，两者都存在于结构体中，但 `ToJSONSchema()` 先检查 `p.params != nil`，JSON Schema 分支被静默忽略
- 大多数 Provider 适配器在为其原生 API 编组工具 Schema 前调用 `ToJSONSchema()`

---

## 5. Provider 扩展元数据槽位

### 5.1 设计原则

Provider 扩展是**类型化结构体字段**，嵌在规范类型的可选指针字段中：

```go
type ResponseMeta struct {
    // ...
    OpenAIExtension *openai.ResponseMetaExtension   // nil = 非 OpenAI
    GeminiExtension *gemini.ResponseMetaExtension   // nil = 非 Gemini
    ClaudeExtension  *claude.ResponseMetaExtension  // nil = 非 Claude
    Extension       any                              // 未知 provider 回退
}
```

**为什么用类型化字段而非 `map[string]any`**：
1. **Concat 函数有类型化合约** — `ConcatMessages` 可以调用 `openai.ConcatResponseMetaExtensions()`，知道确切的数据结构
2. **编译器可检查** — 类型错误在编译期捕获，而非运行时
3. **IDE 自动补全** — 具体字段可见、可导航
4. **零成本存在/不存在检查** — `nil` 指针直接表明"此 provider 未提供此数据"

### 5.2 OpenAI 扩展

```go
// schema/openai/extension.go

// ResponseMetaExtension OpenAI 特定的响应元数据
type ResponseMetaExtension struct {
    ID                     string
    Status                 string                 // "completed" | "incomplete" | "in_progress"
    PreviousResponseID     string                 // Responses API 的上一次响应
    Error                  *ResponseError
    IncompleteDetails      *IncompleteDetails     // 内容过滤截断原因
    Reasoning              *Reasoning             // o-series 推理配置
    ServiceTier            ServiceTier            // "scale" | "default"
    CreatedAt              int64                  // 时间戳
    PromptCacheRetention   PromptCacheRetention   // 缓存保留策略
}

type ResponseError struct {
    Code    string
    Message string
}

type IncompleteDetails struct {
    Reason string  // "content_filter" | "max_tokens" | ...
}

type Reasoning struct {
    Effort  string  // "low" | "medium" | "high"
    Summary string  // 推理总结
}

// AssistantGenTextExtension 每个文本块的注解/引用
type AssistantGenTextExtension struct {
    Refusal     *OutputRefusal
    Annotations []*TextAnnotation
}

type OutputRefusal struct {
    Refusal string // 内容过滤器拒绝原因
}

// TextAnnotation 文档引用，四种位置类型
type TextAnnotation struct {
    Index   int
    Type    string   // "file_citation" | "url_citation" | "file_path" | "container_file_citation"
    // 以下指针中只有一个非 nil：
    FileCitation          *FileCitation
    URLCitation           *URLCitation
    FilePath              *FilePath
    ContainerFileCitation *ContainerFileCitation
}

type FileCitation struct {
    FileID string
}

type URLCitation struct {
    URL    string
    Title  string
}

type FilePath struct {
    FileID string
}

type ContainerFileCitation struct {
    FileID         string
    CharOffsetStart int
    CharOffsetEnd  int
}

// ConcatAssistantGenTextExtensions 合并流式注解
func ConcatAssistantGenTextExtensions(chunks []*AssistantGenTextExtension) *AssistantGenTextExtension
```

### 5.3 Claude 扩展

```go
// schema/claude/extension.go

// ResponseMetaExtension Claude 特定的响应元数据
type ResponseMetaExtension struct {
    ID           string
    StopReason   string  // "end_turn" | "max_tokens" | "stop_sequence" | "tool_use"
    StopSequence string  // 触发的停止序列
    StopDetails  *StopDetails
}

type StopDetails struct {
    Category    string
    Explanation string
}

// AssistantGenTextExtension 每个文本块的引用
type AssistantGenTextExtension struct {
    Citations []*TextCitation
}

// TextCitation Claude 引用 — 四种引用类型的联合体
type TextCitation struct {
    // 恰好一个位置类型非 nil：
    CharLocation          *CitationCharLocation
    PageLocation          *CitationPageLocation
    ContentBlockLocation  *CitationContentBlockLocation
    WebSearchResultLocation *CitationWebSearchResultLocation
}

type CitationCharLocation struct {
    CitedText     string
    DocumentTitle string
    DocumentIndex int
    CharStart     int
    CharEnd       int
}

type CitationPageLocation struct {
    CitedText     string
    DocumentTitle string
    DocumentIndex int
    PageStart     int
    PageEnd       int
}

type CitationContentBlockLocation struct {
    CitedText       string
    DocumentTitle   string
    DocumentIndex   int
    BlockIndexStart int
    BlockIndexEnd   int
}

type CitationWebSearchResultLocation struct {
    CitedText     string
    SearchResultIndex int
}

// ConcatAssistantGenTextExtensions 追加所有引用（引用通常出现在最终块中）
func ConcatAssistantGenTextExtensions(chunks []*AssistantGenTextExtension) *AssistantGenTextExtension
```

### 5.4 Gemini 扩展

```go
// schema/gemini/extension.go

// ResponseMetaExtension Gemini 特定的响应元数据
type ResponseMetaExtension struct {
    ID           string
    FinishReason string
    GroundingMeta *GroundingMetadata   // 在线搜索结果
}

// GroundingMetadata 搜索基础信息
type GroundingMetadata struct {
    GroundingChunks   []*GroundingChunk   // 网页来源
    GroundingSupports []*GroundingSupport  // 置信度分数、段落信息
    SearchEntryPoint  *SearchEntryPoint    // 渲染内容、SDK blob
    WebSearchQueries []string              // 后续搜索建议
}

type GroundingChunk struct {
    Web *WebSource
}

type WebSource struct {
    Title string
    URI   string
    Domain string
}

type GroundingSupport struct {
    Segment          string
    ConfidenceScores []float64
    GroundingChunkIndices []int32
}

type SearchEntryPoint struct {
    RenderedContent string
    SDKBlob         string
}

// ConcatResponseMetaExtensions 合并多个 Gemini 响应元数据块
func ConcatResponseMetaExtensions(chunks []*ResponseMetaExtension) *ResponseMetaExtension
```

### 5.5 扩展合并合约

每个 Provider 目录（`schema/openai/`、`schema/claude/`、`schema/gemini/`）必须导出以下 concat 辅助函数：

```
ConcatResponseMetaExtensions(chunks []*ResponseMetaExtension) *ResponseMetaExtension
ConcatAssistantGenTextExtensions(chunks []*AssistantGenTextExtension) *AssistantGenTextExtension  // 如适用
```

这些函数被 `ConcatAgenticMessages` 调用（通过 `concatAgenticResponseMeta`），后者自身被 `internal.RegisterStreamChunkConcatFunc` 注册。这使得 Provider 扩展的合并逻辑对通用流合并路径完全集成。

---

## 6. Stream Concat 注册表与 Message Concat 行为

### 6.1 注册式 Concat 派发

框架使用一个集中的 concat 函数注册表，通过 Go 泛型 + `reflect.Type` 实现类型安全的派发：

```go
// internal/concat.go (概念代码)

// concatFuncRegistry 按类型索引的合并函数注册表
var concatFuncRegistry = map[reflect.Type]func([]any) (any, error){}

// RegisterStreamChunkConcatFunc[T] 注册类型 T 的流合并函数
// 在 init() 中调用一次。
func RegisterStreamChunkConcatFunc[T any](fn func([]T) (T, error)) {
    var zero T
    t := reflect.TypeOf(zero)
    // 包装为 any → any 的通用形式
    concatFuncRegistry[t] = func(chunks []any) (any, error) {
        typed := make([]T, len(chunks))
        for i, c := range chunks {
            typed[i] = c.(T)
        }
        return fn(typed)
    }
}

// ConcatItems[T] 合并类型为 T 的流数据块
// compose 层调用此方法，无需知道 T 的具体类型
func ConcatItems[T any](chunks []T) (T, error) { ... }
```

**init() 注册**（`schema/message.go`）：

```go
func init() {
    internal.RegisterStreamChunkConcatFunc(ConcatMessages)
    internal.RegisterStreamChunkConcatFunc(ConcatMessageArray)
    internal.RegisterStreamChunkConcatFunc(ConcatAgenticMessages)
    internal.RegisterStreamChunkConcatFunc(ConcatAgenticMessagesArray)
    internal.RegisterStreamChunkConcatFunc(ConcatToolResults)
}
```

### 6.2 ConcatMessages — 经典 Message 拼接

```go
// ConcatMessages 将流式 Message chunk 拼接为完整 Message
func ConcatMessages(chunks []*Message) (*Message, error)
```

**拼接规则**：

| 字段 | 拼接行为 | 说明 |
|------|---------|------|
| `Content` | 字符串拼接 | `"Hello"` + `" world"` → `"Hello world"` |
| `ReasoningContent` | 字符串拼接 | 思考内容累积 |
| `ToolCalls` | 按 `Index` 分组 → 验证 ID/Type/Name → 拼接 `Arguments` JSON 片段 → 按 Index 排序 | `concatToolCalls` |
| `AssistantGenMultiContent` | 按类型合并多模态部件 | `concatAssistantMultiContent` |
| `UserInputMultiContent` | 按类型合并多模态部件 | `concatUserMultiContent` |
| `ResponseMeta` | 保留最后一个非 nil | finish_reason、usage 在流末尾到达 |
| `Role` | 保留第一个非零值 | 角色不应在流中变化 |

**concatToolCalls 行为细则**：
1. 遍历 chunk，对非 nil `ToolCall` 按 `Index` 分组
2. 每组内验证所有 `ID` / `Type` / `Function.Name` 一致（不一致则错误）
3. 拼接每个 `Function.Arguments` 字符串（JSON 片段累积）
4. 按 `Index` 升序排序，返回合并后的 `[]ToolCall`

### 6.3 ConcatAgenticMessages — ContentBlock 拼接

```go
// ConcatAgenticMessages 将流式 AgenticMessage chunk 拼接为完整 AgenticMessage
func ConcatAgenticMessages(chunks []*AgenticMessage) (*AgenticMessage, error)
```

**拼接规则**：

1. **按 StreamingMeta.Index 分组 ContentBlock**：同一 Index 的块属于同一个逻辑块
2. **每组内按类型分派到特定拼接函数**：
   - `AssistantGenText` → `concatAssistantGenTexts`（拼接 Content + 合并 Provider 扩展）
   - `FunctionToolCall` → `concatFunctionToolCalls`（拼接 Arguments JSON）
   - `Reasoning` → 字符串拼接
   - `FunctionToolResult` → 合并输出文本
   - 图像/音频/视频 → **不可合并**（每个是独立产物），追加到列表
3. **ResponseMeta 拼接**通过 `concatAgenticResponseMeta`：
   - OpenAI 分支：调用 `openai.ConcatResponseMetaExtensions()`
   - Claude 分支：调用 `claude.ConcatResponseMetaExtensions()`
   - Gemini 分支：调用 `gemini.ConcatResponseMetaExtensions()`
   - `Extension any` 回退：使用 `internal.ConcatSliceValue`（运行时类型断言 + append）

### 6.4 流所有权与并发安全

关键约束：
- `StreamReader.Recv()` 是线程安全的主消费方法
- `StreamReader.Copy(n)` 创建 n 个独立子读取器，**每个必须调用 Close()**
- 若有子读取器泄漏，底层 goroutine 永远不会终止 ← Eino 中最常见的 goroutine 泄漏
- `SetAutomaticClose()` 依赖垃圾回收，不提供确定性清理

### 6.5 StreamReader 多态后端

`StreamReader[T]` 有五种内部后端：

```
readerTypeStream   → 基于 Channel (Pipe)
readerTypeArray    → 基于切片 (StreamReaderFromArray)
readerTypeMultiStream → 扇入 (MergeStreamReaders)
readerTypeWithConvert → 逐元素转换 (StreamReaderWithConvert)
readerTypeChild    → 扇出 (Copy)
```

关键操作：

| 操作 | 函数 | 用途 |
|------|------|------|
| 创建管道 | `Pipe[T](cap)` | 创建配对的 StreamReader + StreamWriter |
| 零开销数组 | `StreamReaderFromArray[T](arr)` | 由切片支持的只读流 |
| 扇出 | `Copy(n int)` | 创建 n 个独立子读取器（回调处理器 + 下游节点） |
| 扇入 | `MergeStreamReaders[T](srs)` | 多个流交错合并，按到达顺序 |
| 命名扇入 | `MergeNamedStreamReaders[T](srs, names)` | 扇入并跟踪每个源完成状态 |
| 类型转换 | `StreamReaderWithConvert[T, D](sr, convert)` | 逐元素转换，返回 `ErrNoValue` 以过滤 |

---

## 7. OpenAI 适配器骨架

### 7.1 适配器职责

OpenAI 适配器将 OpenAI Chat Completion API（或 Responses API）的原生格式转换为规范类型，反之亦然。以下骨架定义最小接口和转换函数。

```go
// adapter/openai/adapter.go

package openai

// OpenAIChatModel 将 OpenAI SDK 包装为 BaseChatModel 的适配器
type OpenAIChatModel struct {
    client     *openai.Client
    model      string
    options    *OpenAIOptions
}

type OpenAIOptions struct {
    Temperature *float32
    MaxTokens   *int
    TopP        *float32
    Stop        []string
    User        string     // OpenAI 特定的 user 标识
    Tools       []*schema.ToolInfo
}

// === 实现 BaseChatModel ===

// Generate 同步生成：OpenAI Chat Completion → *schema.Message
func (m *OpenAIChatModel) Generate(
    ctx context.Context,
    messages []*schema.Message,
    opts ...schema.ModelOption,
) (*schema.Message, error) {
    // 1. 合并选项（公共 + Provider 特定）
    // 2. ConvertMessages(messages) → []openai.ChatCompletionMessage
    // 3. client.CreateChatCompletion(ctx, req)
    // 4. ConvertResponse(resp) → *schema.Message
    // 5. 返回（含 ToolCalls、ResponseMeta、OpenAIExtension）
}

// Stream 流式生成：OpenAI Chat Completion Stream → StreamReader[*schema.Message]
func (m *OpenAIChatModel) Stream(
    ctx context.Context,
    messages []*schema.Message,
    opts ...schema.ModelOption,
) (*schema.StreamReader[*schema.Message], error) {
    // 1. 创建 Pipe[*schema.Message]
    // 2. 启动 goroutine：
    //    - client.CreateChatCompletionStream(ctx, req)
    //    - for each chunk → ConvertChunk(chunk) → sw.Send(partialMessage)
    //    - sw.Close()
    // 3. 立即返回 StreamReader
}

// === 转换函数 ===

// ConvertMessages 规范消息 → OpenAI API 格式
func ConvertMessages(messages []*schema.Message) []openai.ChatCompletionMessage {
    var result []openai.ChatCompletionMessage
    for _, msg := range messages {
        openaiMsg := openai.ChatCompletionMessage{
            Role: convertRole(msg.Role),  // Assistant→"assistant", Tool→"tool", etc.
        }
        // 处理 Content 字段
        if len(msg.UserInputMultiContent) > 0 {
            // 多模态内容 → []openai.ChatMessagePart
            openaiMsg.MultiContent = convertInputParts(msg.UserInputMultiContent)
        } else {
            openaiMsg.Content = msg.Content
        }
        // 处理 ToolCalls
        if len(msg.ToolCalls) > 0 {
            openaiMsg.ToolCalls = convertToolCalls(msg.ToolCalls)
        }
        // 处理 Tool 角色
        if msg.Role == schema.Tool {
            openaiMsg.ToolCallID = msg.ToolCallID
        }
        result = append(result, openaiMsg)
    }
    return result
}

// ConvertResponse OpenAI API 响应 → 规范 Message
func ConvertResponse(resp *openai.ChatCompletionResponse) *schema.Message {
    choice := resp.Choices[0]
    msg := &schema.Message{
        Role:    schema.Assistant,
        Content: choice.Message.Content,
        ResponseMeta: &schema.ResponseMeta{
            ID:           resp.ID,
            Model:        resp.Model,
            FinishReason: string(choice.FinishReason),
            Usage:        convertUsage(&resp.Usage),
            OpenAIExtension: &schema.RespMetaOpenAI{
                ID:        resp.ID,
                Status:    resp.Status,
                CreatedAt: resp.Created,
                ServiceTier: convertServiceTier(resp.ServiceTier),
            },
        },
    }
    // 转换工具调用
    if len(choice.Message.ToolCalls) > 0 {
        msg.ToolCalls = convertToolCallsFromOpenAI(choice.Message.ToolCalls)
    }
    // 转换推理内容
    if hasReasoningContent(choice) {
        msg.ReasoningContent = extractReasoningContent(choice)
    }
    return msg
}

// ConvertChunk 流式 OpenAI chunk → 部分 Message
func ConvertChunk(chunk *openai.ChatCompletionStreamResponse) *schema.Message {
    delta := chunk.Choices[0].Delta
    msg := &schema.Message{
        Role:    schema.Assistant,
        Content: delta.Content,
    }
    // 工具调用 deltas：按 Index 分组，携带 JSON 片段
    if len(delta.ToolCalls) > 0 {
        msg.ToolCalls = convertToolCallDeltas(delta.ToolCalls)  // 设置 Index
    }
    // 最终 chunk 携带 finish_reason 和 usage
    if chunk.Choices[0].FinishReason != "" {
        msg.ResponseMeta = &schema.ResponseMeta{
            FinishReason: string(chunk.Choices[0].FinishReason),
            Usage:        convertUsage(chunk.Usage),
        }
    }
    return msg
}

// ConvertTools 规范工具 → OpenAI 工具格式
func ConvertTools(tools []*schema.ToolInfo) []openai.Tool {
    result := make([]openai.Tool, len(tools))
    for i, t := range tools {
        jsonSchema := t.ParamsOneOf.ToJSONSchema() // 标准化为 JSON Schema
        result[i] = openai.Tool{
            Type: "function",
            Function: &openai.FunctionDefinition{
                Name:        t.Name,
                Description: t.Desc,
                Parameters:  jsonSchema,
            },
        }
    }
    return result
}
```

### 7.2 OpenAI 适配器关键行为

| 行为 | 规则 |
|------|------|
| **角色映射** | `Assistant`→`"assistant"`, `User`→`"user"`, `System`→`"system"`, `Tool`→`"tool"` |
| **多模态内容** | `MessageInputPart` 联合体 → OpenAI content parts (text / image_url / audio / video / file) |
| **工具调用 index** | 流式 tool_calls delta → 用 `index` 字段设置 `ToolCall.Index`，同一 index 的 delta 累积 Arguments 片段 |
| **推理 token** | `reasoning_tokens` → `TokenUsage.ReasoningTokens` |
| **内容过滤** | `IncompleteDetails` → `ResponseMeta.OpenAIExtension.IncompleteDetails` |
| **注解** | `annotations[]` → `AssistantGenTextExtension.Annotations`，按 `Index` 去重 |
| **服务等级** | `service_tier` → `ResponseMeta.OpenAIExtension.ServiceTier` |

---

## 8. Claude 适配器骨架

```go
// adapter/claude/adapter.go

package claude

// ClaudeChatModel 将 Anthropic SDK 包装为 BaseChatModel 的适配器
type ClaudeChatModel struct {
    client  *anthropic.Client
    model   string
    options *ClaudeOptions
}

type ClaudeOptions struct {
    Temperature *float32
    MaxTokens   int              // Claude 的 max_tokens 是**必需的**
    TopP        *float32
    Tools       []*schema.ToolInfo
}

// === 实现 BaseChatModel ===

// Generate 同步生成：Anthropic Messages API → *schema.Message
func (m *ClaudeChatModel) Generate(ctx context.Context, msgs []*schema.Message, opts ...schema.ModelOption) (*schema.Message, error) {
    // 1. ConvertMessages(msgs) → anthropic.MessageParam slice
    //    — 合并连续的 user 消息（Claude 要求 user/assistant 交替）
    //    — 提取 system 消息作为顶级 system 参数
    //    — 将 ToolCalls 转换为 assistant 消息中的 tool_use 内容块
    //    — 将 Tool 角色消息（ToolCallID）转换为 user 消息中的 tool_result 内容块
    // 2. ConvertTools(opts.Tools) → anthropic.ToolParam slice
    // 3. client.Messages.Create(ctx, req)
    // 4. ConvertResponse(resp) → *schema.Message
}

// Stream 流式生成：Anthropic Messages Stream → StreamReader[*schema.Message]
func (m *ClaudeChatModel) Stream(ctx context.Context, msgs []*schema.Message, opts ...schema.ModelOption) (*schema.StreamReader[*schema.Message], error) {
    // 1. 创建 Pipe[*schema.Message]
    // 2. 启动 goroutine：处理 stream events → ConvertEvent → sw.Send
    // 3. 返回 StreamReader
}

// === 转换函数 (关键差异) ===

// ConvertMessagesToClaude 规范消息 → Anthropic API 格式
func ConvertMessagesToClaude(messages []*schema.Message) []anthropic.MessageParam {
    // Claude 的严格约束：
    // - 消息必须 user/assistant 交替
    // - 连续的 user 消息需要合并
    // - system 消息作为顶级 system 参数，不出现在消息列表中
    // - ToolCall → assistant 消息中的 tool_use 内容块
    // - Tool 角色消息 → user 消息中的 tool_result 内容块
}

// ConvertClaudeResponse Anthropic 响应 → 规范 Message
func ConvertClaudeResponse(resp *anthropic.MessageResponse) *schema.Message {
    msg := &schema.Message{
        Role: schema.Assistant,
        ResponseMeta: &schema.ResponseMeta{
            ID:           resp.ID,
            Model:        resp.Model,
            FinishReason: resp.StopReason,
            ClaudeExtension: &schema.RespMetaClaude{
                ID:         resp.ID,
                StopReason: resp.StopReason,
            },
        },
    }
    // 遍历 resp.Content 块：
    for _, block := range resp.Content {
        switch block.Type {
        case "text":
            msg.Content += block.Text
            // 提取引用：block.Text 中可能有 <citation> 或使用 Citations 字段
        case "tool_use":
            msg.ToolCalls = append(msg.ToolCalls, &schema.ToolCall{
                ID:   block.ID,
                Type: "function",
                Function: schema.ToolCallFunction{
                    Name:      block.Name,
                    Arguments: string(block.Input), // JSON 对象
                },
            })
        }
    }
    // 处理 Thinking 内容块（Claude 扩展思考）
    if hasThinkingBlock(resp) {
        msg.ReasoningContent = extractThinking(resp)
    }
    return msg
}

// ConvertToolsToClaude 规范工具 → Anthropic 工具格式
func ConvertToolsToClaude(tools []*schema.ToolInfo) []anthropic.ToolParam {
    result := make([]anthropic.ToolParam, len(tools))
    for i, t := range tools {
        jsonSchema := t.ParamsOneOf.ToJSONSchema()
        // Anthropic 工具格式：
        result[i] = anthropic.ToolParam{
            Name:        t.Name,
            Description: t.Desc,
            InputSchema: jsonSchema,
        }
    }
    return result
}
```

### 8.1 Claude 适配器关键行为

| 行为 | 规则 |
|------|------|
| **角色映射** | Claude 只有 user/assistant 角色。System → 顶级 `system` 参数 |
| **消息合并** | 连续的 user 消息合并为一条（Claude 要求 alternation） |
| **System 处理** | 从消息列表中提取所有 system 消息，作为 API 的顶级 `system` 参数 |
| **Tool 角色** | `Tool` 角色 + `ToolCallID` → user 消息中的 `tool_result` 内容块 |
| **ToolCalls** | `ToolCalls[]` → assistant 消息中的 `tool_use` 内容块 |
| **Thinking** | Claude thinking 内容块 → `Message.ReasoningContent` |
| **引用** | `CitationCharLocation`/`CitationPageLocation`/`CitationContentBlockLocation` → `ClaudeExtension.Citations` |
| **停止原因** | `end_turn`/`max_tokens`/`stop_sequence`/`tool_use` → `ResponseMeta.FinishReason` |

---

## 9. Gemini 适配器骨架

```go
// adapter/gemini/adapter.go

package gemini

// GeminiChatModel 将 Google GenAI SDK 包装为 BaseChatModel 的适配器
type GeminiChatModel struct {
    client  *genai.Client
    model   string
    options *GeminiOptions
}

type GeminiOptions struct {
    Temperature  *float32
    MaxTokens    *int32
    TopP         *float32
    Tools        []*schema.ToolInfo
}

// === 实现 BaseChatModel ===

// Generate 同步生成：Gemini GenerateContent → *schema.Message
func (m *GeminiChatModel) Generate(ctx context.Context, msgs []*schema.Message, opts ...schema.ModelOption) (*schema.Message, error) {
    // 1. ConvertMessages(msgs) → []genai.Content
    //    — "model" 角色用 Assistant；"user" 角色用 User
    //    — 合并连续的相同角色消息（Gemini 要求 alternation）
    //    — ToolCalls → Assistant Content 中的 functionCall parts
    //    — Tool 角色消息 → User Content 中的 functionResponse parts
    // 2. ConvertSynGeminiTools(opts.Tools) → []genai.Tool
    // 3. client.Models.GenerateContent(ctx, model, contents)
    // 4. ConvertGeminiResponse(resp) → *schema.Message
}

// Stream 流式生成：Gemini GenerateContentStream → StreamReader[*schema.Message]
func (m *GeminiChatModel) Stream(ctx context.Context, msgs []*schema.Message, opts ...schema.ModelOption) (*schema.StreamReader[*schema.Message], error) {
    // 1. 创建 Pipe[*schema.Message]
    // 2. 启动 goroutine：for each resp in stream → ConvertStreamChunk → sw.Send
    // 3. 返回 StreamReader
}

// === 转换函数 (关键差异) ===

// ConvertMessagesToGemini 规范消息 → Google AI Content 格式
func ConvertMessagesToGemini(messages []*schema.Message) []genai.Content {
    // Gemini 的特殊约束：
    // - "assistant" → "model" 角色
    // - 消息必须是 user/model 交替，连续的相同角色合并
    // - System 消息 → 放在第一个 Content 前，通过 SystemInstruction 传递
    //   或作为第一个 user Content 的 text part（如果使用 GenerateContentConfig.SystemInstruction）
    // - ToolCalls → model Content 中的 functionCall parts
    // - Tool 角色（ToolCallID）→ user Content 中的 functionResponse parts
}

// ConvertGeminiResponse Gemini 响应 → 规范 Message
func ConvertGeminiResponse(resp *genai.GenerateContentResponse) *schema.Message {
    if len(resp.Candidates) == 0 {
        return nil
    }
    candidate := resp.Candidates[0]
    msg := &schema.Message{
        Role: schema.Assistant,
    }

    // 遍历 Parts：
    for _, part := range candidate.Content.Parts {
        switch {
        case part.Text != "":
            msg.Content += part.Text
        case part.FunctionCall != nil:
            msg.ToolCalls = append(msg.ToolCalls, &schema.ToolCall{
                ID:   part.FunctionCall.ID,
                Type: "function",
                Function: schema.ToolCallFunction{
                    Name:      part.FunctionCall.Name,
                    Arguments: part.FunctionCall.Args, // JSON 字符串
                },
            })
        case part.Thought != nil:
            msg.ReasoningContent += part.Thought // 或通过 Thought 字段累积
        case part.InlineData != nil:
            // 多模态输出（图像/音频等）→ 填充 AssistantGenMultiContent
        }
    }

    // 响应元数据
    msg.ResponseMeta = &schema.ResponseMeta{
        ID:           resp.ResponseID,
        Model:        resp.ModelVersion,
        FinishReason: string(candidate.FinishReason),
        Usage:        convertGeminiUsage(resp.UsageMetadata),
        GeminiExtension: &schema.RespMetaGemini{
            ID:           resp.ResponseID,
            FinishReason: string(candidate.FinishReason),
            GroundingMeta: convertGroundingMetadata(candidate.GroundingMetadata),
        },
    }

    return msg
}

// ConvertGroundingMetadata Gemini Grounding → 规范 GroundingMetadata
func ConvertGroundingMetadata(gm *genai.GroundingMetadata) *schema.GeminiGroundingMetadata {
    if gm == nil {
        return nil
    }
    result := &schema.GeminiGroundingMetadata{}
    // GroundingChunks：网页来源
    for _, chunk := range gm.GroundingChunks {
        if chunk.Web != nil {
            result.GroundingChunks = append(result.GroundingChunks, &schema.GeminiGroundingChunk{
                Web: &schema.GeminiWebSource{
                    Title:  chunk.Web.Title,
                    URI:    chunk.Web.URI,
                    Domain: chunk.Web.Domain,
                },
            })
        }
    }
    // GroundingSupports：置信度分数
    for _, gs := range gm.GroundingSupports {
        result.GroundingSupports = append(result.GroundingSupports, &schema.GeminiGroundingSupport{
            Segment:              gs.Segment.Text,
            ConfidenceScores:     gs.ConfidenceScores,
            GroundingChunkIndices: gs.GroundingChunkIndices,
        })
    }
    // SearchEntryPoint
    if gm.SearchEntryPoint != nil {
        result.SearchEntryPoint = &schema.GeminiSearchEntryPoint{
            RenderedContent: gm.SearchEntryPoint.RenderedContent,
            SDKBlob:         gm.SearchEntryPoint.SdkBlob,
        }
    }
    result.WebSearchQueries = gm.WebSearchQueries
    return result
}

// ConvertToolsToGemini 规范工具 → Gemini 工具声明格式
func ConvertToolsToGemini(tools []*schema.ToolInfo) []*genai.Tool {
    funcDecls := make([]*genai.FunctionDeclaration, len(tools))
    for i, t := range tools {
        jsonSchema := t.ParamsOneOf.ToJSONSchema()
        funcDecls[i] = &genai.FunctionDeclaration{
            Name:        t.Name,
            Description: t.Desc,
            Parameters:  jsonSchema,
        }
    }
    return []*genai.Tool{{FunctionDeclarations: funcDecls}}
}
```

### 9.1 Gemini 适配器关键行为

| 行为 | 规则 |
|------|------|
| **角色映射** | `Assistant`→`"model"`, `User`→`"user"`, `System`→通过 SystemInstruction 传递 |
| **消息合并** | 连续的相同角色合并（Gemini 要求 user/model 交替） |
| **System 处理** | System 角色消息 → `GenerateContentConfig.SystemInstruction`（第一个 `Content` 之前） |
| **Tool 角色** | `Tool` 角色 + `ToolCallID` → user Content 中的 `functionResponse` part |
| **ToolCalls** | `ToolCalls[]` → model Content 中的 `functionCall` parts |
| **Thought** | `part.Thought` → `Message.ReasoningContent` |
| **多模态输出** | `part.InlineData` → `Message.AssistantGenMultiContent` (Image/Audio/Video) |
| **Grounding** | `GroundingMetadata` → `GeminiExtension.GroundingMeta` (Chunks/Supports/SearchEntryPoint) |
| **响应 ID** | `response_id`（非 `candidates[0].content`）→ `ResponseMeta.ID` |

---

## 10. 与复刻版现状的差距分析

### 10.1 当前复刻版已有的类型

| 类型 | 文件 | 状态 |
|------|------|------|
| `Message` | `compose/chatmodel.go:19` | ✅ 已有（Role/Content/ToolCalls/ToolCallID/Name） |
| `RoleType` | `compose/chatmodel.go:10` | ✅ 已有（System/Human/Assistant/Tool）* |
| `ToolCall` | `compose/schema.go:3` | ✅ 已有（ID/Type/Function） |
| `ToolCallFunction` | `compose/schema.go:9` | ✅ 已有（Name/Arguments） |
| `ToolInfo` | `compose/schema.go:14` | ✅ 已有（Name/Desc/ParamsOneOf） |
| `ParamsOneOf` | `compose/schema.go:20` | ✅ 已有（仅轻量级 Params） |
| `ParameterInfo` | `compose/schema.go:24` | ✅ 已有（Type/Desc/Required/Enum） |
| `ToolResult` | `compose/schema.go:31` | ✅ 已有（Text） |
| `ChatModel` 接口 | `compose/chatmodel.go:27` | ✅ 已有（Generate/Stream） |
| `StreamReader` | `compose/stream.go` | ✅ 已有（基础 Pipe） |

> *注意：当前复刻版使用 `Human` 而非 Eino 规范的 `User`。这是需要对齐的关键差异。

### 10.2 缺失的核心类型（教育子集需要补齐）

| # | 缺失类型 | 所属层级 | 教育子集是否需要 |
|---|---------|---------|:---:|
| 1 | `ResponseMeta` | Schema | ✅ 必须 |
| 2 | `TokenUsage` | Schema | ✅ 必须 |
| 3 | `Document` | Schema | ✅ 必须 |
| 4 | `AgenticMessage` | Schema | ⚪ 可选（高级教学） |
| 5 | `ContentBlock` 类型系统 | Schema | ⚪ 可选（高级教学） |
| 6 | `AgenticRoleType` | Schema | ⚪ 可选 |
| 7 | `AssistantGenText` (+ Provider 扩展槽位) | Schema | ⚪ 可选 |
| 8 | `MessageInputPart / MessageOutputPart` | Schema | ⚪ 可选（多模态教学） |
| 9 | `ToolCall.Index` 字段 | Schema | ✅ 必须（流式工具调用） |
| 10 | `ParamsOneOf.jsonSchema` 分支 | Schema | ⚪ 可选（轻量级模式已够） |
| 11 | `schema/openai/` Provider 扩展结构体 | Provider 扩展 | ⚪ 可选 |
| 12 | `schema/claude/` Provider 扩展结构体 | Provider 扩展 | ⚪ 可选 |
| 13 | `schema/gemini/` Provider 扩展结构体 | Provider 扩展 | ⚪ 可选 |
| 14 | `internal.RegisterStreamChunkConcatFunc` | Concat 注册表 | ✅ 必须 |
| 15 | `ConcatMessages` | Concat | ✅ 必须 |
| 16 | `ConcatAgenticMessages` | Concat | ⚪ 可选 |
| 17 | `StreamReader` 多态后端（Copy/Merge/WithConvert） | Stream | ⚪ 可选（已有 Pipe） |
| 18 | Provider 适配器（OpenAI/Claude/Gemini） | Adapter | ⚪ 可选（骨架文档已提供） |
| 19 | `BaseModel[M]` 泛型接口 + `messageType` 约束 | 组件接口 | ⚪ 可选 |
| 20 | `components.Typer` / `components.Checker` | 组件接口 | ⚪ 可选 |

### 10.3 RoleType 命名差异

| 当前复刻版 | Eino 规范 | 对齐方向 |
|-----------|----------|---------|
| `Human` | `User` | 应改为 `User`（或两者共存，`const User = Human`） |
| `System` | `System` | ✅ 一致 |
| `Assistant` | `Assistant` | ✅ 一致 |
| `Tool` | `Tool` | ✅ 一致 |

---

## 11. 教育子集实现路径

### 11.1 必须实现的 MVP 子集

以下是最小可行教育实现，覆盖本节点的核心教学目标：

```
Phase 1 — Schema 扩展 (当前 compose/schema.go + compose/chatmodel.go):
├── Message 新增字段: ResponseMeta, ReasoningContent, Extra
├── 新增 ResponseMeta 类型 (ID, Model, FinishReason, Usage)
├── 新增 TokenUsage 类型
├── 新增 ToolCall.Index 字段 (流式控制)
├── 新增 Document 类型 (ID, Content, Score, Meta)
├── 对齐 RoleType: Human → User (或增加 User = Human 别名)

Phase 2 — Concat 注册表 (新建 compose/concat.go):
├── concatFuncRegistry: map[reflect.Type]func
├── RegisterStreamChunkConcatFunc[T] 泛型注册函数
├── ConcatMessages: 按 Index 合并 ToolCalls, 拼接 Content/ReasoningContent
├── init(): 注册 ConcatMessages 到注册表

Phase 3 — Provider 适配器骨架 (仅文档/接口，非运行代码):
├── adapter/ 包骨架定义 (适配器接口)
├── 每种 Provider 的 ConvertMessages / ConvertResponse 签名
```

### 11.2 可选增强（非本节点范围）

```
Phase 4 — AgenticMessage (高级教学):
├── ContentBlock 类型联合体 (教育子集: 仅 5 种核心块)
├── AgenticMessage 类型 + ConcatAgenticMessages

Phase 5 — Provider 扩展槽位:
├── schema/openai/extension.go (ResponseMetaExtension + AssistantGenTextExtension)
├── schema/claude/extension.go
├── schema/gemini/extension.go

Phase 6 — 多态 StreamReader:
├── Copy(n) 扇出
├── MergeStreamReaders 扇入
├── StreamReaderWithConvert 类型转换
```

### 11.3 文件归属规划

| 文件 | 内容 | 优先级 |
|------|------|:---:|
| `compose/schema.go` (修改) | 追加 `ResponseMeta`、`TokenUsage`、`Document`、`ToolCall.Index` | MUST |
| `compose/chatmodel.go` (修改) | `Message` 追加 `ResponseMeta`、`ReasoningContent`、`Extra`；RoleType 增加 `User` 别名 | MUST |
| `compose/concat.go` (新建) | 注册表 + `RegisterStreamChunkConcatFunc` + `ConcatMessages` | MUST |
| `compose/concat_test.go` (新建) | Concat 行为测试 (Content 拼接、ToolCalls 按 Index 合并) | MUST |
| `adapter/openai/adapter.go` (新建) | OpenAI 适配器骨架 (仅转换签名，不实现) | SHOULD |
| `adapter/claude/adapter.go` (新建) | Claude 适配器骨架 (仅转换签名) | SHOULD |
| `adapter/gemini/adapter.go` (新建) | Gemini 适配器骨架 (仅转换签名) | SHOULD |

### 11.4 实现约束

1. **不改变现有接口签名** — `ChatModel.Generate` / `ChatModel.Stream` 保持不变
2. **不破坏现有测试** — 所有新增字段为零值兼容（nil/empty string）
3. **纯 Go 标准库** — 不引入外部依赖
4. **最小接口设计** — 恰好是教学需要的 1-2 个方法
5. **适配器骨架不执行网络调用** — 仅定义转换函数签名和注释说明

---

## 12. 附录：关键源码对照

### 12.1 Eino 手册第六章对应章节

| 概念 | 手册章节 | 行号参考 |
|------|---------|---------|
| 问题描述 (Provider 格式差异) | §1 | 3-8 |
| Provider 差异对比表 | §2.1 | 15-23 |
| 三层架构图 | §3 | 45-55 |
| Message 结构 | §4.1 | 79-92 |
| AgenticMessage 结构 | §4.2 | 103-120 |
| ContentBlock 类型系统 | §4.2 | 113-120 |
| ToolInfo / ParamsOneOf | §4.3 | 134-142 |
| StreamReader 多态后端 | §4.4 | 145-170 |
| 流拼接 init() 注册 | §4.5 | 176-194 |
| OpenAi 扩展 | §4.6 | 199-218 |
| Claude 扩展 | §4.6 | 219-234 |
| Gemini 扩展 | §4.6 | 236-250 |
| 序列化 | §4.7 | 252-270 |
| 组件 Schema 桥接 | §4.8 | 272-277 |
| 示例代码 | §5.1-5.7 | 280-450 |
| 常见陷阱 | §6.1-6.7 | 452-481 |
| Rive 借鉴要点 | §7.1-7.6 | 483-506 |

### 12.2 复刻版现有文件对应

| 概念 | 复刻版文件 | 行号 |
|------|-----------|------|
| `Message` | `compose/chatmodel.go` | 19-25 |
| `RoleType` | `compose/chatmodel.go` | 10-17 |
| `ToolCall` | `compose/schema.go` | 3-7 |
| `ToolInfo` / `ParamsOneOf` | `compose/schema.go` | 14-22 |
| `ParameterInfo` | `compose/schema.go` | 24-29 |
| `ToolResult` | `compose/schema.go` | 31-33 |
| `ChatModel` 接口 | `compose/chatmodel.go` | 27-30 |
| `ComponentType` 常量 | `compose/types.go` | 21-29 |
| `PipeStreamReader` | `compose/stream.go` | 11-14 |
| `PipeStreamWriter` | `compose/stream.go` | 16-19 |

### 12.3 Provider 适配器转换函数速查

| Provider | 适配器关键转换 | 输入 | 输出 |
|----------|:---|------|------|
| OpenAI | `ConvertMessages` | `[]*schema.Message` | `[]openai.ChatCompletionMessage` |
| | `ConvertResponse` | `*openai.ChatCompletionResponse` | `*schema.Message` |
| | `ConvertChunk` | `*openai.ChatCompletionStreamResponse` | `*schema.Message` (partial) |
| | `ConvertTools` | `[]*schema.ToolInfo` | `[]openai.Tool` |
| Claude | `ConvertMessagesToClaude` | `[]*schema.Message` | `[]anthropic.MessageParam` + `system` |
| | `ConvertClaudeResponse` | `*anthropic.MessageResponse` | `*schema.Message` |
| | `ConvertToolsToClaude` | `[]*schema.ToolInfo` | `[]anthropic.ToolParam` |
| Gemini | `ConvertMessagesToGemini` | `[]*schema.Message` | `[]genai.Content` + `SystemInstruction` |
| | `ConvertGeminiResponse` | `*genai.GenerateContentResponse` | `*schema.Message` |
| | `ConvertGeminiTools` | `[]*schema.ToolInfo` | `[]*genai.Tool` |
| | `ConvertGroundingMetadata` | `*genai.GroundingMetadata` | `*schema.GeminiGroundingMetadata` |

### 12.4 Concat 行为速查表

| 数据类型 | 字段 | 拼接规则 |
|---------|------|---------|
| `Message.Content` | 字符串 | 直接拼接 |
| `Message.ReasoningContent` | 字符串 | 直接拼接 |
| `Message.ToolCalls` | 切片 | 按 `Index` 分组 → 验证 → 拼接 Arguments → 排序 |
| `Message.ResponseMeta` | 结构体 | 保留最后一个非 nil |
| `AgenticMessage.ContentBlocks` | 切片 | 按 `StreamingMeta.Index` 分组 → 类型特定拼接 |
| `AssistantGenText.Content` | 字符串 | 直接拼接 |
| `AssistantGenText.OpenAIExtension.Annotations` | 切片 | 按 Index 去重后追加 |
| `AssistantGenText.ClaudeExtension.Citations` | 切片 | 直接追加 |
| `AgenticResponseMeta.OpenAIExtension` | 结构体 | `openai.ConcatResponseMetaExtensions` |
| `AgenticResponseMeta.ClaudeExtension` | 结构体 | `claude.ConcatResponseMetaExtensions` |
| `AgenticResponseMeta.GeminiExtension` | 结构体 | `gemini.ConcatResponseMetaExtensions` |
| 图像/音频/视频块 | 不可合并 | 追加到列表（每个是独立产物） |
