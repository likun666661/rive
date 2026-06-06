# D1 Chapter 6 Implementation Contract

> **Contract ID**: ch6-impl-contract-d1
> **Input**: R1 (current schema gap audit) + R2 (provider schema contract research)
> **Purpose**: Assign concrete file ownership, API names, test specifications, and acceptance criteria for Chapter 6 schema/concat/provider-adapter implementation.
> **Constraint**: This is a contract document only. Do not implement production code from this node.
> **Date**: 2026-06-06

---

## 1. Contract Scope Summary

Chapter 6 introduces the canonical schema layer, stream concat registry, and provider adapter skeletons that enable multi-provider LLM interoperability. Based on the three-layer architecture (Provider Adapters → Component Interfaces → Canonical Schema), this contract covers:

| Area | Files | Priority |
|---|---|---|
| **Schema Canonical Types** (Phase 1) | `compose/types.go` (modify), `compose/schema.go` (modify), `compose/chatmodel.go` (modify) | CRITICAL |
| **Concat/Merge Registry** (Phase 1) | `compose/concat.go` (new), `compose/stream.go` (modify init) | CRITICAL |
| **Provider Adapter Skeletons** (Phase 1) | `adapter/openai/adapter.go`, `adapter/claude/adapter.go`, `adapter/gemini/adapter.go` (new) | HIGH |
| **Tests** (Phase 1) | `compose/concat_test.go`, `compose/schema_test.go`, `compose/chatmodel_test.go` (new/modify) | CRITICAL |
| **AgenticMessage System** (Phase 2) | `compose/agentic_message.go` (new) | HIGH |
| **Provider Extensions** (Phase 2) | `compose/openai/ext.go`, `compose/claude/ext.go`, `compose/gemini/ext.go` (new) | HIGH |
| **Serialisation** (Phase 2) | `compose/serialization.go` (new) | HIGH |
| **README/Example Updates** (Phase 3) | `README.md` (modify), `cmd/example/main.go` (modify) | MEDIUM |

---

## 2. File Ownership & API Names — Schema Canonical Types

### 2.1 `compose/types.go` — New Shared Types (MODIFY)

**Owner**: Phase 1 Worker
**Estimated LOC**: +30

```go
// ADD to compose/types.go — shared enums and base types

// RoleType — moved/aliased from chatmodel.go; add User alias
const (
    User      RoleType = "user"  // NEW — alias for Human, Eino canonical name
)

// DataType constants — NEW
type DataType string
const (
    DataTypeString  DataType = "string"
    DataTypeInteger DataType = "integer"
    DataTypeBoolean DataType = "boolean"
    DataTypeNumber  DataType = "number"
    DataTypeObject  DataType = "object"
    DataTypeArray   DataType = "array"
)

// ChatMessagePartType — NEW
type ChatMessagePartType string
const (
    ChatMessagePartTypeText            ChatMessagePartType = "text"
    ChatMessagePartTypeImageURL        ChatMessagePartType = "image_url"
    ChatMessagePartTypeAudioURL        ChatMessagePartType = "audio_url"
    ChatMessagePartTypeVideoURL        ChatMessagePartType = "video_url"
    ChatMessagePartTypeFileURL         ChatMessagePartType = "file_url"
    ChatMessagePartTypeToolSearchResult ChatMessagePartType = "tool_search_result"
)
```

**API Contract**:
| Symbol | Kind | Location |
|---|---|---|
| `RoleType("user")` | Constant alias | `compose/types.go` |
| `DataType` + 6 constants | Type + enum | `compose/types.go` |
| `ChatMessagePartType` + 6 constants | Type + enum | `compose/types.go` |

---

### 2.2 `compose/schema.go` — Extended Tool Schema Types (MODIFY)

**Owner**: Phase 1 Worker
**Estimated LOC**: +180 (from 33 → ~213)

```go
// MODIFY existing types; ADD new types

// ToolCall — ADD Index field
type ToolCall struct {
    Index    *int             `json:"-"`     // NEW — streaming: identifies which logical call deltas belong to
    ID       string           `json:"id"`
    Type     string           `json:"type"`
    Function ToolCallFunction `json:"function"`
    Extra    map[string]any   `json:"-"`     // NEW — legacy extension bag
}

// ToolInfo — ADD Extra field
type ToolInfo struct {
    Name        string
    Desc        string
    ParamsOneOf *ParamsOneOf
    Extra       map[string]any // NEW
}

// ParamsOneOf — ADD jsonSchema branch + ToJSONSchema method
type ParamsOneOf struct {
    params     map[string]*ParameterInfo    // mode 1: lightweight (existing)
    jsonSchema any                          // mode 2: *JSONSchema (NEW — minimised; use interface{} to avoid external dep)
}

// Constructors — NEW
func NewParamsOneOfByParams(params map[string]*ParameterInfo) *ParamsOneOf
func NewParamsOneOfByJSONSchema(schema any) *ParamsOneOf

// ToJSONSchema — NEW
// Normalises both modes. For params mode, renders ParameterInfo tree → map representation.
// For jsonSchema mode, returns the stored schema as-is.
func (p *ParamsOneOf) ToJSONSchema() (any, error)

// ParameterInfo — ADD SubParams + ElemInfo, Type → DataType
type ParameterInfo struct {
    Type      DataType                    // CHANGED: was string, now DataType
    Desc      string
    Required  bool
    Enum      []string
    SubParams map[string]*ParameterInfo   // NEW — nested object fields
    ElemInfo  *ParameterInfo              // NEW — array element type
}

// ToolResult — ADD multi-modal fields
type ToolResult struct {
    Text   string          // existing
    Images []*ImageContent // NEW
    Audio  []*AudioContent // NEW
    Video  []*VideoContent // NEW
    Files  []*FileContent  // NEW
}

// ——————————————————— NEW types ——————————————————

// ImageContent — NEW
type ImageContent struct {
    URL    string
    Data   []byte
    Format string // "png", "jpeg", etc.
}

// AudioContent — NEW
type AudioContent struct {
    URL    string
    Data   []byte
    Format string
}

// VideoContent — NEW
type VideoContent struct {
    URL    string
    Data   []byte
    Format string
}

// FileContent — NEW
type FileContent struct {
    URL  string
    Data []byte
    Name string
    Type string
}

// ——————————————————— NEW document type ——————————————————

// Document — NEW
type Document struct {
    ID        string
    Content   string
    Meta      map[string]any
    Embedding []float64
    Score     float64
}
```

**API Contract — Exported Symbols**:
| Symbol | Kind | Location |
|---|---|---|
| `ToolCall.Index` | Field `*int` | `compose/schema.go` |
| `ToolCall.Extra` | Field `map[string]any` | `compose/schema.go` |
| `ToolInfo.Extra` | Field `map[string]any` | `compose/schema.go` |
| `NewParamsOneOfByParams` | Constructor | `compose/schema.go` |
| `NewParamsOneOfByJSONSchema` | Constructor | `compose/schema.go` |
| `ParamsOneOf.ToJSONSchema()` | Method | `compose/schema.go` |
| `ParameterInfo.Type` | Field `DataType` | `compose/schema.go` |
| `ParameterInfo.SubParams` | Field | `compose/schema.go` |
| `ParameterInfo.ElemInfo` | Field | `compose/schema.go` |
| `ToolResult.Images/Audio/Video/Files` | Fields | `compose/schema.go` |
| `ImageContent` | Type | `compose/schema.go` |
| `AudioContent` | Type | `compose/schema.go` |
| `VideoContent` | Type | `compose/schema.go` |
| `FileContent` | Type | `compose/schema.go` |
| `Document` | Type | `compose/schema.go` |

---

### 2.3 `compose/chatmodel.go` — Extended Message + ResponseMeta (MODIFY)

**Owner**: Phase 1 Worker
**Estimated LOC**: +100 (from 164 → ~264)

```go
// Message — ADD fields
type Message struct {
    Role                      RoleType
    Content                   string
    ToolCalls                 []ToolCall
    ToolCallID                string
    ToolName                  string              // RENAMED from Name (backward compat: keep Name as well, or just ToolName)
    UserInputMultiContent     []MessageInputPart  // NEW — multi-modal user input
    AssistantGenMultiContent  []MessageOutputPart // NEW — multi-modal model output
    ResponseMeta              *ResponseMeta       // NEW
    ReasoningContent          string              // NEW
    Extra                     map[string]any      // NEW
}

// ResponseMeta — NEW
type ResponseMeta struct {
    ID              string
    Model           string
    FinishReason    string       // "stop" | "length" | "tool_calls" | "content_filter" | "function_call"
    Usage           *TokenUsage
    LogProbs        *LogProbs
    OpenAIExtension *OpenAIRespMetaExtension  // NEW — nil = not OpenAI
    GeminiExtension *GeminiRespMetaExtension  // NEW — nil = not Gemini
    ClaudeExtension *ClaudeRespMetaExtension  // NEW — nil = not Claude
    Extension       any                       // NEW — unknown provider fallback
}

// TokenUsage — NEW
type TokenUsage struct {
    PromptTokens     int
    CompletionTokens int
    TotalTokens      int
    ReasoningTokens  int // NEW — o-series reasoning tokens
}

// LogProbs — NEW
type LogProbs struct {
    Content []*LogProbInfo
}

type LogProbInfo struct {
    Token       string
    LogProb     float64
    Bytes       []int32
    TopLogProbs map[string]float64
}

// MessageInputPart — NEW
type MessageInputPart struct {
    Type ChatMessagePartType
    // Exactly one of the following is non-nil:
    Text             *string
    Image            *MessageInputImage
    Audio            *MessageInputAudio
    Video            *MessageInputVideo
    File             *MessageInputFile
    ToolSearchResult *ToolSearchResult
}

type MessageInputImage struct {
    URL    string
    Detail string // "low" | "high" | "auto"
}

type MessageInputAudio struct {
    URL string
}

type MessageInputVideo struct {
    URL string
}

type MessageInputFile struct {
    URL string
}

// MessageOutputPart — NEW
type MessageOutputPart struct {
    Type ChatMessagePartType
    // Exactly one of the following is non-nil:
    Text      *string
    Image     *MessageOutputImage
    Audio     *MessageOutputAudio
    Video     *MessageOutputVideo
    Reasoning *string
}

type MessageOutputImage struct {
    URL    string
    Data   []byte
    Format string
}

type MessageOutputAudio struct {
    URL    string
    Data   []byte
    Format string
}

type MessageOutputVideo struct {
    URL    string
    Data   []byte
    Format string
}

// ToolSearchResult — NEW
type ToolSearchResult struct {
    ToolName string
    Score    float64
}

// ——————————————————— PROVIDER EXTENSION STUBS (inline to avoid import cycles) ——————————————————

// OpenAIRespMetaExtension — NEW (stub, expanded in compose/openai/ext.go)
type OpenAIRespMetaExtension struct {
    ID                   string
    Status               string // "completed" | "incomplete" | "in_progress"
    PreviousResponseID   string
    IncompleteDetails    *OpenAIIncompleteDetails
    ServiceTier          string // "scale" | "default"
    CreatedAt            int64
    PromptCacheRetention string
}

type OpenAIIncompleteDetails struct {
    Reason string
}

// ClaudeRespMetaExtension — NEW (stub)
type ClaudeRespMetaExtension struct {
    ID          string
    StopReason  string
    StopDetails *ClaudeStopDetails
}

type ClaudeStopDetails struct {
    Category    string
    Explanation string
}

// GeminiRespMetaExtension — NEW (stub)
type GeminiRespMetaExtension struct {
    ID           string
    FinishReason string
    GroundingMeta *GeminiGroundingMetadata
}

type GeminiGroundingMetadata struct {
    GroundingChunks    []*GeminiGroundingChunk
    GroundingSupports  []*GeminiGroundingSupport
    SearchEntryPoint   *GeminiSearchEntryPoint
    WebSearchQueries   []string
}

type GeminiGroundingChunk struct {
    Web *GeminiWebSource
}

type GeminiWebSource struct {
    Title  string
    URI    string
    Domain string
}

type GeminiGroundingSupport struct {
    Segment              string
    ConfidenceScores     []float64
    GroundingChunkIndices []int32
}

type GeminiSearchEntryPoint struct {
    RenderedContent string
    SDKBlob         string
}

// ——————————————————— CONSTRUCTORS ——————————————————

func UserMessage(content string) *Message   // NEW — alias for HumanMessage
```

**API Contract — Exported Symbols**:
| Symbol | Kind | Location |
|---|---|---|
| `Message.ResponseMeta` | Field `*ResponseMeta` | `compose/chatmodel.go` |
| `Message.ReasoningContent` | Field `string` | `compose/chatmodel.go` |
| `Message.Extra` | Field `map[string]any` | `compose/chatmodel.go` |
| `Message.UserInputMultiContent` | Field | `compose/chatmodel.go` |
| `Message.AssistantGenMultiContent` | Field | `compose/chatmodel.go` |
| `Message.ToolName` | Field `string` | `compose/chatmodel.go` |
| `ResponseMeta` | Type | `compose/chatmodel.go` |
| `TokenUsage` | Type | `compose/chatmodel.go` |
| `LogProbs` / `LogProbInfo` | Types | `compose/chatmodel.go` |
| `MessageInputPart` | Type | `compose/chatmodel.go` |
| `MessageOutputPart` | Type | `compose/chatmodel.go` |
| `OpenAIRespMetaExtension` | Type (stub) | `compose/chatmodel.go` |
| `ClaudeRespMetaExtension` | Type (stub) | `compose/chatmodel.go` |
| `GeminiRespMetaExtension` | Type (stub) | `compose/chatmodel.go` |
| `UserMessage()` | Constructor | `compose/chatmodel.go` |

---

## 3. File Ownership & API Names — Concat/Merge Registry

### 3.1 `compose/concat.go` — Registration-based Concat Dispatch (NEW)

**Owner**: Phase 1 Worker
**Estimated LOC**: 120–150

```go
package compose

import (
    "errors"
    "reflect"
    "sync"
)

// ——————————————————— ERROR TYPES ——————————————————

var ErrConcatNotSupported = errors.New("compose: concat not supported for type")

// ——————————————————— REGISTRY ——————————————————

var concatFuncRegistry sync.Map // reflect.Type → func([]T) (T, error)  (replaces existing concatFns)

// RegisterStreamChunkConcatFunc registers a concat function for type T.
// Should be called in init().
func RegisterStreamChunkConcatFunc[T any](fn func([]T) (T, error)) {
    var zero T
    concatFuncRegistry.Store(reflect.TypeOf(zero), fn)
}

// ConcatItems dispatches to the registered concat function for type T.
// Returns ErrConcatNotSupported if no function is registered for T.
func ConcatItems[T any](items []T) (T, error) {
    if len(items) == 0 {
        var zero T
        return zero, nil
    }
    if len(items) == 1 {
        return items[0], nil
    }
    var zero T
    t := reflect.TypeOf(zero)
    fn, ok := concatFuncRegistry.Load(t)
    if !ok {
        var zero T
        return zero, ErrConcatNotSupported
    }
    // fn has type func([]T) (T, error) — call via reflection
    results := reflect.ValueOf(fn).Call([]reflect.Value{reflect.ValueOf(items)})
    result := results[0].Interface().(T)
    var err error
    if !results[1].IsNil() {
        err = results[1].Interface().(error)
    }
    return result, err
}

// ——————————————————— ConcatMessages ——————————————————

// ConcatMessages merges streaming Message chunks into a complete Message.
//
// Merge rules:
//   - Content: string concatenation
//   - ReasoningContent: string concatenation
//   - ToolCalls: group by Index, validate ID/Type/Function.Name consistency, concat Arguments JSON
//   - UserInputMultiContent / AssistantGenMultiContent: type-specific merge
//   - ResponseMeta: keep last non-nil
//   - Role: keep first non-zero
func ConcatMessages(chunks []*Message) (*Message, error) {
    if len(chunks) == 0 {
        return nil, nil
    }
    if len(chunks) == 1 {
        return chunks[0], nil
    }

    result := &Message{}
    firstRoleSet := false

    for _, chunk := range chunks {
        if chunk == nil {
            continue
        }
        if !firstRoleSet && chunk.Role != "" {
            result.Role = chunk.Role
            firstRoleSet = true
        }
        result.Content += chunk.Content
        result.ReasoningContent += chunk.ReasoningContent
        if chunk.ResponseMeta != nil {
            result.ResponseMeta = chunk.ResponseMeta
        }
        // accumulate ToolCalls for later index-based merge
        result.ToolCalls = append(result.ToolCalls, chunk.ToolCalls...)
        result.UserInputMultiContent = append(result.UserInputMultiContent, chunk.UserInputMultiContent...)
        result.AssistantGenMultiContent = append(result.AssistantGenMultiContent, chunk.AssistantGenMultiContent...)
    }

    // concat ToolCalls by Index
    var err error
    result.ToolCalls, err = concatToolCalls(result.ToolCalls)
    if err != nil {
        return nil, err
    }

    return result, nil
}

// concatToolCalls groups ToolCalls by Index, validates consistency, concats Arguments.
func concatToolCalls(toolCalls []ToolCall) ([]ToolCall, error) {
    if len(toolCalls) == 0 {
        return nil, nil
    }

    // Group by Index. nil Index means each ToolCall is a complete call.
    groups := make(map[int][]ToolCall)
    unindexed := make([]ToolCall, 0)

    for _, tc := range toolCalls {
        if tc.Index == nil {
            unindexed = append(unindexed, tc)
        } else {
            groups[*tc.Index] = append(groups[*tc.Index], tc)
        }
    }

    result := make([]ToolCall, 0, len(unindexed)+len(groups))

    // Unindexed ToolCalls pass through as-is
    result = append(result, unindexed...)

    // Merge indexed groups
    for index, group := range groups {
        merged, err := mergeToolCallGroup(group)
        if err != nil {
            return nil, err
        }
        idx := index
        merged.Index = &idx
        result = append(result, *merged)
    }

    // Sort by index ascending
    sortToolCallsByIndex(result)

    return result, nil
}

func mergeToolCallGroup(group []ToolCall) (*ToolCall, error) {
    if len(group) == 0 {
        return nil, nil
    }

    merged := &ToolCall{
        ID:       group[0].ID,
        Type:     group[0].Type,
        Function: ToolCallFunction{Name: group[0].Function.Name},
    }

    for _, tc := range group {
        if tc.ID != merged.ID {
            return nil, errors.New("compose: ToolCall ID mismatch in same index group")
        }
        if tc.Type != merged.Type {
            return nil, errors.New("compose: ToolCall Type mismatch in same index group")
        }
        if tc.Function.Name != merged.Function.Name {
            return nil, errors.New("compose: ToolCall Function.Name mismatch in same index group")
        }
        merged.Function.Arguments += tc.Function.Arguments
    }

    return merged, nil
}

func sortToolCallsByIndex(tcs []ToolCall) {
    // sort by Index ascending, nil Index at end
    // use sort.Slice
}

// ——————————————————— ConcatMessageArray ——————————————————

// ConcatMessageArray concats an array of Messages (one per stream position).
func ConcatMessageArray(chunks []*Message) (*Message, error) {
    return ConcatMessages(chunks)
}

// ——————————————————— ConcatToolResults ——————————————————

// ConcatToolResults merges multiple ToolResults.
func ConcatToolResults(results []*ToolResult) (*ToolResult, error) {
    if len(results) == 0 {
        return nil, nil
    }
    merged := &ToolResult{}
    for _, r := range results {
        if r == nil {
            continue
        }
        merged.Text += r.Text
        merged.Images = append(merged.Images, r.Images...)
        merged.Audio = append(merged.Audio, r.Audio...)
        merged.Video = append(merged.Video, r.Video...)
        merged.Files = append(merged.Files, r.Files...)
    }
    return merged, nil
}
```

**API Contract — Exported Symbols**:
| Symbol | Kind | Location |
|---|---|---|
| `RegisterStreamChunkConcatFunc[T any](fn)` | Generic function | `compose/concat.go` |
| `ConcatItems[T any](items []T) (T, error)` | Generic function | `compose/concat.go` |
| `ConcatMessages(chunks []*Message) (*Message, error)` | Function | `compose/concat.go` |
| `ConcatMessageArray(chunks []*Message) (*Message, error)` | Function | `compose/concat.go` |
| `ConcatToolResults(results []*ToolResult) (*ToolResult, error)` | Function | `compose/concat.go` |
| `ErrConcatNotSupported` | Sentinel error | `compose/concat.go` |

---

### 3.2 `compose/stream.go` — Concat Registration in init() (MODIFY)

**Owner**: Phase 1 Worker
**Estimated LOC**: +5

```go
// ADD to existing file — wrap init section

func init() {
    RegisterStreamChunkConcatFunc(ConcatMessages)
    RegisterStreamChunkConcatFunc(ConcatMessageArray)
    RegisterStreamChunkConcatFunc(ConcatToolResults)
}
```

Also update the existing `RegisterConcatFunc` (line 155) to delegate to `RegisterStreamChunkConcatFunc` or deprecate it.

---

## 4. File Ownership & API Names — Provider Adapter Skeletons

**Design constraint**: Adapter skeletons define only conversion function signatures (interface + type stubs) without any SDK imports or network calls. They serve as documentation of the expected adapter contract.

### 4.1 `adapter/openai/adapter.go` — OpenAI Adapter Skeleton (NEW)

**Owner**: Phase 1 Worker
**Estimated LOC**: 80–100

```go
// Package openai defines the OpenAI-to-canonical adapter contract.
//
// This is a skeleton only. It defines the expected type shapes and conversion
// function signatures. No SDK is imported; no API calls are made.
//
// When implemented, the adapter would wrap an OpenAI SDK client and implement
// the compose.ChatModel interface.

package openai

import compose "github.com/rive/eino-compose-runtime-replica-go/compose"

// ——————————————————— ADAPTER TYPE ——————————————————

// ChatModel wraps an OpenAI API client as a compose.ChatModel.
// SKELETON: no actual SDK client.
type ChatModel struct {
    model   string
    options Options
}

type Options struct {
    Temperature *float32
    MaxTokens   *int
    TopP        *float32
    Stop        []string
    Tools       []*compose.ToolInfo
}

// ——————————————————— CHATMODEL INTERFACE ——————————————————

// Generate converts compose Messages → OpenAI API call → compose Message.
// SKELETON: not implemented; documentation only.
func (m *ChatModel) Generate(ctx context.Context, messages []*compose.Message, opts ...compose.ChatModelOption) (*compose.Message, error)

// Stream converts compose Messages → OpenAI streaming call → StreamReader[*compose.Message].
// SKELETON: not implemented; documentation only.
func (m *ChatModel) Stream(ctx context.Context, messages []*compose.Message, opts ...compose.ChatModelOption) (compose.StreamReader[*compose.Message], error)

// ——————————————————— CONVERSION FUNCTION SIGNATURES ——————————————————

// ConvertMessages canonical Messages → OpenAI ChatCompletion message format.
// Role mapping: Assistant→"assistant", User→"user", System→"system", Tool→"tool".
// Multi-modal input → content parts array.
// ToolCalls → tool_calls array.
// Tool role messages → role:"tool" with tool_call_id.
// SKELETON: signature only.
func ConvertMessages(messages []*compose.Message) []OpenAIMessageParam

// ConvertResponse OpenAI ChatCompletion response → canonical Message.
// Extracts: Content, ToolCalls, FinishReason, Usage, ReasoningContent.
// Populates ResponseMeta.OpenAIExtension with OpenAI-specific fields.
// SKELETON: signature only.
func ConvertResponse(resp *OpenAIChatCompletionResponse) *compose.Message

// ConvertChunk OpenAI stream delta → partial canonical Message.
// Sets ToolCall.Index from delta.tool_calls[].index.
// Includes ResponseMeta on final chunk with finish_reason and usage.
// SKELETON: signature only.
func ConvertChunk(chunk *OpenAIStreamChunk) *compose.Message

// ConvertTools canonical ToolInfo → OpenAI tool format.
// Calls ParamsOneOf.ToJSONSchema() for each tool.
// SKELETON: signature only.
func ConvertTools(tools []*compose.ToolInfo) []OpenAIToolDef

// ——————————————————— MOCK TYPES (represent OpenAI shapes) ——————————————————

type OpenAIMessageParam struct {
    Role    string
    Content any // string or []OpenAIContentPart
}

type OpenAIContentPart struct {
    Type     string
    Text     string
    ImageURL *OpenAIImageURL
}

type OpenAIImageURL struct {
    URL    string
    Detail string
}

type OpenAIChatCompletionResponse struct {
    ID       string
    Model    string
    Object   string
    Created  int64
    Choices  []OpenAIChoice
    Usage    OpenAIUsage
}

type OpenAIChoice struct {
    Index        int
    Message      OpenAIMessageParam
    FinishReason string
}

type OpenAIUsage struct {
    PromptTokens     int
    CompletionTokens int
    TotalTokens      int
}

type OpenAIStreamChunk struct {
    ID      string
    Object  string
    Created int64
    Model   string
    Choices []OpenAIStreamChoice
    Usage   *OpenAIUsage
}

type OpenAIStreamChoice struct {
    Index        int
    Delta        OpenAIMessageParam
    FinishReason string
}

type OpenAIToolDef struct {
    Type     string
    Function *OpenAIFunctionDef
}

type OpenAIFunctionDef struct {
    Name        string
    Description string
    Parameters  any // JSON Schema
}
```

**API Contract — Exported Symbols**:
| Symbol | Kind | Location |
|---|---|---|
| `ChatModel` | Type | `adapter/openai/adapter.go` |
| `Options` | Type | `adapter/openai/adapter.go` |
| `ConvertMessages` | Function signature | `adapter/openai/adapter.go` |
| `ConvertResponse` | Function signature | `adapter/openai/adapter.go` |
| `ConvertChunk` | Function signature | `adapter/openai/adapter.go` |
| `ConvertTools` | Function signature | `adapter/openai/adapter.go` |
| `OpenAIMessageParam` / `OpenAIChatCompletionResponse` / etc. | Mock types | `adapter/openai/adapter.go` |

---

### 4.2 `adapter/claude/adapter.go` — Claude Adapter Skeleton (NEW)

**Owner**: Phase 1 Worker
**Estimated LOC**: 70–90

```go
// Package claude defines the Claude-to-canonical adapter contract.
//
// SKELETON only. No SDK imports. Defines expected conversion function signatures.

package claude

import compose "github.com/rive/eino-compose-runtime-replica-go/compose"

type ChatModel struct {
    model   string
    options Options
}

type Options struct {
    Temperature *float32
    MaxTokens   int    // Claude: max_tokens is REQUIRED
    TopP        *float32
    Tools       []*compose.ToolInfo
}

// Generate — SKELETON
func (m *ChatModel) Generate(ctx context.Context, messages []*compose.Message, opts ...compose.ChatModelOption) (*compose.Message, error)

// Stream — SKELETON
func (m *ChatModel) Stream(ctx context.Context, messages []*compose.Message, opts ...compose.ChatModelOption) (compose.StreamReader[*compose.Message], error)

// ConvertMessagesToClaude canonical Messages → Anthropic Messages format.
// Key differences from OpenAI:
//   - System messages → top-level "system" parameter (not in message list)
//   - Tool role messages → user messages with tool_result content blocks
//   - Claude requires user/assistant alternation → consecutive same-role messages merged
//   - ToolCalls → assistant messages with tool_use content blocks
// SKELETON: signature only.
func ConvertMessagesToClaude(messages []*compose.Message) ([]ClaudeMessageParam, string)

// ConvertClaudeResponse Anthropic Message response → canonical Message.
// Handles text blocks (Content), tool_use blocks (ToolCalls), thinking blocks (ReasoningContent).
// Populates ResponseMeta.ClaudeExtension.
// SKELETON: signature only.
func ConvertClaudeResponse(resp *ClaudeMessageResponse) *compose.Message

// ConvertToolsToClaude canonical ToolInfo → Anthropic tool format.
// SKELETON: signature only.
func ConvertToolsToClaude(tools []*compose.ToolInfo) []ClaudeToolParam

// ——————————————————— MOCK TYPES ——————————————————

type ClaudeMessageParam struct {
    Role    string
    Content []ClaudeContentBlock
}

type ClaudeContentBlock struct {
    Type   string
    Text   string
    ID     string
    Name   string
    Input  any // JSON object for tool_use
}

type ClaudeMessageResponse struct {
    ID         string
    Model      string
    StopReason string
    Content    []ClaudeContentBlock
    Usage      ClaudeUsage
}

type ClaudeUsage struct {
    InputTokens  int
    OutputTokens int
}

type ClaudeToolParam struct {
    Name         string
    Description  string
    InputSchema  any
}
```

**API Contract — Exported Symbols**:
| Symbol | Kind | Location |
|---|---|---|
| `ChatModel` | Type | `adapter/claude/adapter.go` |
| `ConvertMessagesToClaude` | Function signature | `adapter/claude/adapter.go` |
| `ConvertClaudeResponse` | Function signature | `adapter/claude/adapter.go` |
| `ConvertToolsToClaude` | Function signature | `adapter/claude/adapter.go` |
| `ClaudeMessageParam` / `ClaudeMessageResponse` / etc. | Mock types | `adapter/claude/adapter.go` |

---

### 4.3 `adapter/gemini/adapter.go` — Gemini Adapter Skeleton (NEW)

**Owner**: Phase 1 Worker
**Estimated LOC**: 70–90

```go
// Package gemini defines the Gemini-to-canonical adapter contract.
//
// SKELETON only. No SDK imports. Defines expected conversion function signatures.

package gemini

import compose "github.com/rive/eino-compose-runtime-replica-go/compose"

type ChatModel struct {
    model   string
    options Options
}

type Options struct {
    Temperature *float32
    MaxTokens   *int32
    TopP        *float32
    Tools       []*compose.ToolInfo
}

// Generate — SKELETON
func (m *ChatModel) Generate(ctx context.Context, messages []*compose.Message, opts ...compose.ChatModelOption) (*compose.Message, error)

// Stream — SKELETON
func (m *ChatModel) Stream(ctx context.Context, messages []*compose.Message, opts ...compose.ChatModelOption) (compose.StreamReader[*compose.Message], error)

// ConvertMessagesToGemini canonical Messages → Gemini Content format.
// Key differences:
//   - Assistant → "model" role
//   - System role → SystemInstruction (first content before user/model alternation)
//   - Gemini requires user/model alternation → consecutive same-role messages merged
//   - ToolCalls → model Content with functionCall parts
//   - Tool role messages → user Content with functionResponse parts
// SKELETON: signature only.
func ConvertMessagesToGemini(messages []*compose.Message) ([]GeminiContent, *GeminiSystemInstruction)

// ConvertGeminiResponse Gemini GenerateContentResponse → canonical Message.
// Handles text parts (Content), functionCall parts (ToolCalls), thought parts (ReasoningContent).
// Handles InlineData parts (multi-modal output).
// Populates ResponseMeta.GeminiExtension with grounding metadata.
// SKELETON: signature only.
func ConvertGeminiResponse(resp *GeminiGenerateContentResponse) *compose.Message

// ConvertGeminiTools canonical ToolInfo → Gemini tool declaration format.
// SKELETON: signature only.
func ConvertGeminiTools(tools []*compose.ToolInfo) []GeminiToolDef

// ConvertGroundingMetadata raw Gemini grounding → canonical grounding.
// SKELETON: signature only.
func ConvertGroundingMetadata(gm *GeminiGroundingMeta) *compose.GeminiGroundingMetadata

// ——————————————————— MOCK TYPES ——————————————————

type GeminiContent struct {
    Role  string
    Parts []GeminiPart
}

type GeminiPart struct {
    Text           string
    FunctionCall   *GeminiFunctionCall
    FunctionResponse *GeminiFunctionResponse
    Thought        string
    InlineData     *GeminiInlineData
}

type GeminiFunctionCall struct {
    ID   string
    Name string
    Args string
}

type GeminiFunctionResponse struct {
    ID     string
    Output string
}

type GeminiInlineData struct {
    MIMEType string
    Data     []byte
}

type GeminiGenerateContentResponse struct {
    Candidates     []GeminiCandidate
    ResponseID     string
    ModelVersion   string
    UsageMetadata  *GeminiUsageMetadata
}

type GeminiCandidate struct {
    Content          GeminiContent
    FinishReason     string
    GroundingMetadata *GeminiGroundingMeta
}

type GeminiGroundingMeta struct {
    GroundingChunks    []GeminiGroundingChunk
    GroundingSupports  []GeminiGroundingSupport
    SearchEntryPoint   *GeminiSearchEntryPoint
    WebSearchQueries   []string
}

type GeminiGroundingChunk struct {
    Web *GeminiWebSource
}

type GeminiWebSource struct {
    Title  string
    URI    string
    Domain string
}

type GeminiGroundingSupport struct {
    Segment              string
    ConfidenceScores     []float64
    GroundingChunkIndices []int32
}

type GeminiSearchEntryPoint struct {
    RenderedContent string
    SDKBlob         string
}

type GeminiUsageMetadata struct {
    PromptTokenCount     int
    CandidatesTokenCount int
    TotalTokenCount      int
}

type GeminiSystemInstruction struct {
    Parts []GeminiPart
}

type GeminiToolDef struct {
    FunctionDeclarations []GeminiFunctionDeclaration
}

type GeminiFunctionDeclaration struct {
    Name        string
    Description string
    Parameters  any
}
```

**API Contract — Exported Symbols**:
| Symbol | Kind | Location |
|---|---|---|
| `ChatModel` | Type | `adapter/gemini/adapter.go` |
| `ConvertMessagesToGemini` | Function signature | `adapter/gemini/adapter.go` |
| `ConvertGeminiResponse` | Function signature | `adapter/gemini/adapter.go` |
| `ConvertGeminiTools` | Function signature | `adapter/gemini/adapter.go` |
| `ConvertGroundingMetadata` | Function signature | `adapter/gemini/adapter.go` |
| `GeminiContent` / `GeminiPart` / `GeminiGenerateContentResponse` / etc. | Mock types | `adapter/gemini/adapter.go` |

---

## 5. File Ownership & API Names — AgenticMessage System (Phase 2)

### 5.1 `compose/agentic_message.go` — AgenticMessage + ContentBlock System (NEW, Phase 2)

**Owner**: Phase 2 Worker
**Estimated LOC**: 400–500

```go
// AgenticRoleType — NEW
type AgenticRoleType string
const (
    AgenticRoleAssistant AgenticRoleType = "assistant"
    AgenticRoleUser      AgenticRoleType = "user"
    AgenticRoleSystem    AgenticRoleType = "system"
    // NOTE: no "tool" role — tool results are user-role ContentBlocks
)

// ContentBlockType — NEW
type ContentBlockType string
const (
    // User input blocks
    ContentBlockTypeUserInputText  ContentBlockType = "user_input_text"
    ContentBlockTypeUserInputImage ContentBlockType = "user_input_image"
    ContentBlockTypeUserInputAudio ContentBlockType = "user_input_audio"
    ContentBlockTypeUserInputVideo ContentBlockType = "user_input_video"
    ContentBlockTypeUserInputFile  ContentBlockType = "user_input_file"
    // Model output blocks
    ContentBlockTypeAssistantGenText  ContentBlockType = "assistant_gen_text"
    ContentBlockTypeAssistantGenImage ContentBlockType = "assistant_gen_image"
    ContentBlockTypeAssistantGenAudio ContentBlockType = "assistant_gen_audio"
    ContentBlockTypeAssistantGenVideo ContentBlockType = "assistant_gen_video"
    // Reasoning
    ContentBlockTypeReasoning ContentBlockType = "reasoning"
    // Tool calls
    ContentBlockTypeFunctionToolCall ContentBlockType = "function_tool_call"
    ContentBlockTypeServerToolCall   ContentBlockType = "server_tool_call"
    ContentBlockTypeMCPToolCall      ContentBlockType = "mcp_tool_call"
    // Tool results
    ContentBlockTypeFunctionToolResult ContentBlockType = "function_tool_result"
    ContentBlockTypeServerToolResult   ContentBlockType = "server_tool_result"
    ContentBlockTypeMCPToolResult      ContentBlockType = "mcp_tool_result"
    // MCP protocol
    ContentBlockTypeMCPListToolsResult     ContentBlockType = "mcp_list_tools_result"
    ContentBlockTypeMCPToolApprovalRequest  ContentBlockType = "mcp_tool_approval_request"
    ContentBlockTypeMCPToolApprovalResponse ContentBlockType = "mcp_tool_approval_response"
    // Search
    ContentBlockTypeToolSearchResult ContentBlockType = "tool_search_result"
)

// StreamingMeta — stream grouping control
type StreamingMeta struct {
    Index int
}

// ContentBlock — tagged union, exactly one variant non-nil
type ContentBlock struct {
    Type                    ContentBlockType
    UserInputText           *string
    UserInputImage          *MessageInputImage
    UserInputAudio          *MessageInputAudio
    UserInputVideo          *MessageInputVideo
    UserInputFile           *MessageInputFile
    AssistantGenText        *AssistantGenText
    AssistantGenImage       *MessageOutputImage
    AssistantGenAudio       *MessageOutputAudio
    AssistantGenVideo       *MessageOutputVideo
    Reasoning               *string
    FunctionToolCall        *FunctionToolCall
    ServerToolCall          *ServerToolCall
    MCPToolCall             *MCPToolCall
    FunctionToolResult      *FunctionToolResult
    ServerToolResult        *ServerToolResult
    MCPToolResult           *MCPToolResult
    MCPListToolsResult      *MCPListToolsResult
    MCPToolApprovalRequest   *MCPToolApprovalRequest
    MCPToolApprovalResponse  *MCPToolApprovalResponse
    ToolSearchResult        *ToolSearchResult
    StreamingMeta           *StreamingMeta
}

// AssistantGenText — model text output with provider extension slots
type AssistantGenText struct {
    Content         string
    OpenAIExtension *OpenAIGenTextExtension // NEW
    ClaudeExtension *ClaudeGenTextExtension // NEW
}

type OpenAIGenTextExtension struct {
    Refusal     string
    Annotations []*OpenAITextAnnotation
}

type OpenAITextAnnotation struct {
    Index int
    Type  string // "file_citation" | "url_citation" | "file_path" | "container_file_citation"
    FileCitation          *OpenAIFileCitation
    URLCitation           *OpenAIURLCitation
    FilePath              *OpenAIFilePath
    ContainerFileCitation *OpenAIContainerFileCitation
}

type ClaudeGenTextExtension struct {
    Citations []*ClaudeTextCitation
}

type ClaudeTextCitation struct {
    CharLocation          *ClaudeCitationCharLocation
    PageLocation          *ClaudeCitationPageLocation
    ContentBlockLocation  *ClaudeCitationContentBlockLocation
    WebSearchResultLocation *ClaudeCitationWebSearchResultLocation
}

// — FunctionToolCall, ServerToolCall, MCPToolCall — tool call block types
type FunctionToolCall struct {
    CallID    string
    Name      string
    Arguments string
}

type ServerToolCall struct {
    CallID string
    Name   string
    Args   map[string]any
}

type MCPToolCall struct {
    CallID     string
    ServerName string
    ToolName   string
    Arguments  string
}

// — FunctionToolResult, ServerToolResult, MCPToolResult — tool result block types
type FunctionToolResult struct {
    CallID string
    Output string
}

// — MCP protocol block types
type MCPListToolsResult struct{}

type MCPToolApprovalRequest struct{}
type MCPToolApprovalResponse struct{}

// AgenticMessage — NEW
type AgenticMessage struct {
    Role          AgenticRoleType
    ContentBlocks []*ContentBlock
    ResponseMeta  *AgenticResponseMeta
    Extra         map[string]any
}

type AgenticResponseMeta struct {
    TokenUsage      *TokenUsage
    OpenAIExtension *OpenAIRespMetaExtension
    GeminiExtension *GeminiRespMetaExtension
    ClaudeExtension  *ClaudeRespMetaExtension
    Extension       any
}

// ConcatAgenticMessages merges streaming AgenticMessage chunks.
// Groups ContentBlocks by StreamingMeta.Index, then delegates to type-specific concat.
func ConcatAgenticMessages(chunks []*AgenticMessage) (*AgenticMessage, error)
```

**API Contract — Exported Symbols**:
| Symbol | Kind | Location |
|---|---|---|
| `AgenticRoleType` + 3 constants | Type + enum | `compose/agentic_message.go` |
| `ContentBlockType` + ~20 constants | Type + enum | `compose/agentic_message.go` |
| `ContentBlock` | Type | `compose/agentic_message.go` |
| `AgenticMessage` | Type | `compose/agentic_message.go` |
| `AgenticResponseMeta` | Type | `compose/agentic_message.go` |
| `StreamingMeta` | Type | `compose/agentic_message.go` |
| `AssistantGenText` | Type | `compose/agentic_message.go` |
| `FunctionToolCall` / `ServerToolCall` / `MCPToolCall` | Types | `compose/agentic_message.go` |
| `ConcatAgenticMessages` | Function | `compose/agentic_message.go` |

---

## 6. File Ownership & API Names — Provider Extensions (Phase 2)

### 6.1 `compose/openai/ext.go` — OpenAI Extension Concatenation (NEW, Phase 2)

**Owner**: Phase 2 Worker
**Estimated LOC**: 80–100

```go
package openai

import (
    compose "github.com/rive/eino-compose-runtime-replica-go/compose"
)

// ConcatResponseMetaExtensions merges OpenAI response meta chunks.
func ConcatResponseMetaExtensions(chunks []*compose.OpenAIRespMetaExtension) *compose.OpenAIRespMetaExtension

// ConcatAssistantGenTextExtensions merges OpenAI text block extensions.
// Annotations are deduped by Index.
func ConcatAssistantGenTextExtensions(chunks []*compose.OpenAIGenTextExtension) *compose.OpenAIGenTextExtension
```

### 6.2 `compose/claude/ext.go` — Claude Extension Concatenation (NEW, Phase 2)

**Owner**: Phase 2 Worker
**Estimated LOC**: 60–80

```go
package claude

import compose "github.com/rive/eino-compose-runtime-replica-go/compose"

func ConcatResponseMetaExtensions(chunks []*compose.ClaudeRespMetaExtension) *compose.ClaudeRespMetaExtension
func ConcatAssistantGenTextExtensions(chunks []*compose.ClaudeGenTextExtension) *compose.ClaudeGenTextExtension
```

### 6.3 `compose/gemini/ext.go` — Gemini Extension Concatenation (NEW, Phase 2)

**Owner**: Phase 2 Worker
**Estimated LOC**: 50–70

```go
package gemini

import compose "github.com/rive/eino-compose-runtime-replica-go/compose"

func ConcatResponseMetaExtensions(chunks []*compose.GeminiRespMetaExtension) *compose.GeminiRespMetaExtension
```

---

## 7. File Ownership & API Names — Serialisation (Phase 2)

### 7.1 `compose/serialization.go` — gob RegisterName (NEW, Phase 2)

**Owner**: Phase 2 Worker
**Estimated LOC**: 60–80

```go
package compose

import "encoding/gob"

// RegisterName registers a type name for gob serialization.
// Wraps gob.RegisterName with a _eino_ prefix convention.
func RegisterName(value any, name string) {
    gob.RegisterName("eino_"+name, value)
}

// init registers all canonical types for checkpoint serialization.
func init() {
    RegisterName(&Message{}, "Message")
    RegisterName(&ToolCall{}, "ToolCall")
    RegisterName(&ResponseMeta{}, "ResponseMeta")
    RegisterName(&TokenUsage{}, "TokenUsage")
    RegisterName(&ToolInfo{}, "ToolInfo")
    RegisterName(&ToolResult{}, "ToolResult")
    RegisterName(&Document{}, "Document")
    // Phase 2 additions:
    // RegisterName(&AgenticMessage{}, "AgenticMessage")
    // RegisterName(&ContentBlock{}, "ContentBlock")
}
```

Note: This file's `init()` must be separate from `concat.go`'s `init()`. The serialization `init()` calls `RegisterName` which wraps `gob.RegisterName`. Go allows multiple `init()` functions in the same package; they execute in file-name lexical order. Ensure `serialization.go` < `concat.go` alphabetically.

**API Contract**:
| Symbol | Kind | Location |
|---|---|---|
| `RegisterName(value any, name string)` | Function | `compose/serialization.go` |
| `init()` — all type registrations | init | `compose/serialization.go` |

---

## 8. Exact Tests Required

### 8.1 `compose/concat_test.go` — Concat Registry + Message Concat Tests (NEW, Phase 1)

**Owner**: Phase 1 Worker
**Estimated LOC**: 250–350

**CRITICAL Tests**:
| Test Name | Description | Input | Expected |
|---|---|---|---|
| `TestConcatItems_Registered` | Register func, call ConcatItems → dispatches | `RegisterStreamChunkConcatFunc[string](myConcat)`, then `ConcatItems([]string{"a","b"})` | `"ab"` from myConcat |
| `TestConcatItems_Unregistered` | ConcatItems for unregistered type → ErrConcatNotSupported | `ConcatItems([]int{1,2})` | error |
| `TestConcatItems_SingleElement` | Single-element slice → returns same element | `ConcatItems([]string{"a"})` | `"a"`, nil |
| `TestConcatItems_EmptySlice` | Empty slice → zero value | `ConcatItems([]string{})` | `""`, nil |
| `TestConcatMessages_TextOnly` | Two Messages with Content | `[&Message{Content:"Hello"}, &Message{Content:" World"}]` | Content="Hello World" |
| `TestConcatMessages_ReasoningContent` | Reasoning fragments concatenated | `[&Message{ReasoningContent:"I think"}, &Message{ReasoningContent:" therefore..."}]` | ReasoningContent="I think therefore..." |
| `TestConcatMessages_ToolCalls` | Indexed ToolCall deltas → merged by index | Messages with ToolCalls having same Index=0, different Arguments fragments | Single merged ToolCall with concatenated Arguments |
| `TestConcatMessages_ToolCallIndexConflict` | Same Index, different ID → error | Two ToolCalls with Index=0 but different ID | error |
| `TestConcatMessages_ToolCallOrdering` | Merged ToolCalls sorted by Index | Chunks arrive out of order | Result sorted ascending |
| `TestConcatMessages_UnindexedToolCalls` | ToolCalls without Index pass through | nil Index | Each preserved as-is |
| `TestConcatMessages_ResponseMeta` | Last non-nil ResponseMeta wins | Two Messages, second has ResponseMeta | ResponseMeta from second |
| `TestConcatMessages_Role` | First non-zero Role preserved | Two Messages, first has Role=Assistant | Role=Assistant |
| `TestConcatMessageArray` | Array-level concat | `[]*Message` slice | Same as ConcatMessages |
| `TestConcatToolResults` | Multiple ToolResults merged | Text concatenated, images/appended | Merged correctly |
| `TestConcatToolResults_MultiModal` | ToolResults with Images/Audio/Video | Multi-modal fields | All appended |

**HIGH Tests**:
| Test Name | Description |
|---|---|
| `TestConcatMessages_NilChunk` | Nil chunks in slice → ignored |
| `TestConcatMessages_AllNil` | All nil → nil, nil |
| `TestConcatMessages_MultiProviderMeta` | Messages with OpenAI/Claude/Gemini extensions → all preserved |

---

### 8.2 `compose/chatmodel_test.go` — Extended Message Tests (MODIFY, Phase 1)

**Owner**: Phase 1 Worker
**Estimated LOC**: +150

**CRITICAL Tests**:
| Test Name | Description |
|---|---|
| `TestMessage_NewFields_ZeroValueSafe` | Construct Message → all new fields zero-value safe |
| `TestMessage_ResponseMeta_Usage` | Set ResponseMeta with TokenUsage → round-trip |
| `TestMessage_ReasoningContent` | Message with ReasoningContent → persists |
| `TestMessage_MultiContent_Input` | UserInputMultiContent with Text+Image parts → correct |
| `TestMessage_MultiContent_Output` | AssistantGenMultiContent with Text+Reasoning → correct |
| `TestMessage_ToolName` | ToolName field distinct from Name → persists |
| `TestMessage_Extra` | Extra map → survives serialization |
| `TestUserMessage` | UserMessage("hello") → Role=User, Content="hello" |
| `TestSystemMessage_Role` | SystemMessage → Role=System |

**HIGH Tests**:
| Test Name | Description |
|---|---|
| `TestRoleType_UserAlias` | `User` == `"user"`, `Human` == `"human"` |
| `TestResponseMeta_OpenAIExtension` | OpenAIExtension field set → round-trip |
| `TestResponseMeta_GeminiExtension` | GeminiExtension with GroundingMeta → round-trip |
| `TestResponseMeta_ClaudeExtension` | ClaudeExtension with StopReason → round-trip |
| `TestTokenUsage_ReasoningTokens` | ReasoningTokens field → persists |

---

### 8.3 `compose/schema_test.go` — Extended Tool Schema Tests (NEW, Phase 1)

**Owner**: Phase 1 Worker
**Estimated LOC**: 200–250

**CRITICAL Tests**:
| Test Name | Description |
|---|---|
| `TestToolCall_Index` | ToolCall with Index set → round-trip |
| `TestToolCall_IndexNil` | ToolCall with nil Index → zero-value safe |
| `TestToolCall_Extra` | Extra map → persists |
| `TestParamsOneOf_ByParams` | NewParamsOneOfByParams → ToJSONSchema() returns valid schema |
| `TestParamsOneOf_ByJSONSchema` | NewParamsOneOfByJSONSchema → ToJSONSchema() returns same |
| `TestParamsOneOf_Empty` | Empty ParamsOneOf → ToJSONSchema() returns valid empty schema |
| `TestParameterInfo_Nested` | SubParams recursive object → round-trip |
| `TestParameterInfo_ArrayElem` | ElemInfo for array element → round-trip |
| `TestParameterInfo_DataType` | Type field is DataType enum, not raw string |
| `TestToolResult_MultiModal` | ToolResult with Images/Audio/Video/Files → all fields persist |
| `TestDocument` | Document with all fields → round-trip |
| `TestDocument_Embedding` | Embedding []float64 → persists |
| `TestToolInfo_Extra` | ToolInfo.Extra map → persists |

**HIGH Tests**:
| Test Name | Description |
|---|---|
| `TestToolCall_JsonMarshal` | ToolCall JSON round-trip (Extra excluded via `json:"-"`) |
| `TestParameterInfo_Enum` | Enum values → persist |
| `TestParamsOneOf_MutualExclusion` | Setting both params and jsonSchema → documented behavior |
| `TestImageContent` | ImageContent URL/Data/Format → round-trip |

---

### 8.4 Adapter Tests — Skeleton Compile-Time Checks Only (Phase 1)

**Owner**: Phase 1 Worker
**Estimated LOC**: 30–50 total

Each adapter package needs one test file verifying the skeleton compiles and types export correctly:

| File | Test | Description |
|---|---|---|
| `adapter/openai/adapter_test.go` | `TestOpenAISkeletonCompiles` | Verify ChatModel type exists, ConvertMessages/ConvertResponse/ConvertChunk/ConvertTools signatures compile |
| `adapter/claude/adapter_test.go` | `TestClaudeSkeletonCompiles` | Verify ChatModel type exists, conversion function signatures compile |
| `adapter/gemini/adapter_test.go` | `TestGeminiSkeletonCompiles` | Verify ChatModel type exists, conversion function signatures compile |

---

### 8.5 Integration Tests (Phase 1)

| Test Name | Description | Location |
|---|---|---|
| `TestEndToEnd_StreamConcat` | Pipe → Send Message chunks → ConcatMessages → complete Message | `compose/concat_test.go` |
| `TestEndToEnd_ToolCallStream` | Simulated streaming tool calls with Index → concat → correct merged ToolCalls | `compose/concat_test.go` |

---

## 9. README & Example Updates (Phase 3)

### 9.1 `README.md` — ADD Chapter 6 Section

**Owner**: Phase 3 Worker
**Estimated LOC**: +40–60

Add a new section after the Chapter 5 section:

```markdown
## 第六章: Schema / Provider Adapters (Chapter 6)

> 教育子集: 规范 Schema 类型 + Concat 注册表 + Provider 适配器骨架

### 核心设计: Provider 无关数据模型

Eino 是多 Provider LLM 框架。通过规范消息类型 (`Message`), 流合并注册表 (`ConcatMessages`) 和 Provider 适配器骨架实现跨 Provider 互操作。

#### 三层架构
- **Schema 层** (`compose/schema.go`, `compose/chatmodel.go`): 规范类型 (Message, ToolCall, ToolInfo, ResponseMeta, Document)
- **Concat 注册表** (`compose/concat.go`): `RegisterStreamChunkConcatFunc[T]` + `ConcatItems[T]` 反射派发
- **Provider 适配器** (`adapter/openai/`, `adapter/claude/`, `adapter/gemini/`): 骨架接口, 定义 ConvertXxx 签名

#### 关键 API

\```go
// ToolCall 流式控制
tc := &compose.ToolCall{Index: ptr(0), ID: "call_1"}

// 流合并
result, err := compose.ConcatMessages([]*compose.Message{chunk1, chunk2})

// Provider 扩展
msg.ResponseMeta = &compose.ResponseMeta{
    FinishReason: "stop",
    Usage: &compose.TokenUsage{PromptTokens: 10, CompletionTokens: 20, TotalTokens: 30},
    OpenAIExtension: &compose.OpenAIRespMetaExtension{ID: "resp_1"},
}
\```

### 边界
- 适配器骨架**不做网络调用**: 仅定义转换函数签名与类型
- Provider 扩展为**可选 nil 指针**: 不使用该 Provider 的代码忽略 nil
- StreamReader 多态后端 (Copy/Merge/WithConvert) 未在本阶段覆盖
```

### 9.2 `cmd/example/main.go` — ADD Chapter 6 Example

**Owner**: Phase 3 Worker
**Estimated LOC**: +40–60

Add a new example demonstrating:
1. Message construction with new fields (ResponseMeta, ReasoningContent)
2. Stream concat with indexed ToolCalls
3. Provider extension stub usage

---

## 10. Acceptance Criteria

### Phase 1 Gate (MUST pass before Phase 2 starts)

| # | Criterion | Verification |
|---|-----------|-------------|
| AC-1 | All new schema types compile: `ToolCall.Index`, `ParameterInfo.SubParams/ElemInfo`, `Document`, `ResponseMeta`, `TokenUsage`, `MessageInputPart`, `MessageOutputPart` | `go build ./...` succeeds |
| AC-2 | `ConcatMessages` correctly merges text, reasoning, indexed ToolCalls | `go test ./compose/ -run TestConcat -count=1` passes |
| AC-3 | `RegisterStreamChunkConcatFunc` + `ConcatItems` dispatch correctly for registered and unregistered types | `go test ./compose/ -run TestConcatItems -count=1` passes |
| AC-4 | `ConcatToolResults` merges multi-modal results | `go test ./compose/ -run TestConcatToolResults -count=1` passes |
| AC-5 | Provider adapter skeletons compile: `adapter/openai/`, `adapter/claude/`, `adapter/gemini/` | `go build ./adapter/...` succeeds |
| AC-6 | All existing tests continue to pass (backward compatibility) | `go test ./... -count=1` — no regressions |
| AC-7 | `go vet ./...` — no warnings | clean output |
| AC-8 | `RoleType("user")` alias exists alongside `RoleType("human")` | compilation + test |
| AC-9 | `ParamsOneOf.ToJSONSchema()` works for both params and jsonSchema modes | test assertion |
| AC-10 | New fields are zero-value safe — constructing `Message{}` does not panic | test assertion |

### Phase 2 Gate

| # | Criterion | Verification |
|---|-----------|-------------|
| AC-11 | `AgenticMessage` and ~20 `ContentBlockType` constants compile | build |
| AC-12 | `ConcatAgenticMessages` groups by StreamingMeta.Index and delegates per block type | test |
| AC-13 | Provider extension concat functions compile and have correct signatures | build |
| AC-14 | `RegisterName` registers all canonical types for gob | test: encode/decode round-trip |

### Phase 3 Gate

| # | Criterion | Verification |
|---|-----------|-------------|
| AC-15 | README.md has Chapter 6 section with API examples | visual review |
| AC-16 | `cmd/example/main.go` has Chapter 6 demonstration | `go run ./cmd/example/` includes Chapter 6 output |

---

## 11. Non-Goals (Explicitly Out of Scope)

The following are explicitly **not** part of this contract:

| # | Non-Goal | Reason |
|---|---|---|
| NG-1 | Full `StreamReader[T]` polymorphic backend (Copy/Merge/WithConvert/array) | Phase 4+; current `PipeStreamReader` suffices |
| NG-2 | Provider adapter implementation (actual SDK calls) | This node defines skeletons only; Rive dispatch nodes will implement adapters |
| NG-3 | `BaseModel[M messageType]` generic interface with `messageType` constraint | Component-level concern; belongs in a future component contract |
| NG-4 | `components.Typer` / `components.Checker` interfaces | Out of scope for schema layer |
| NG-5 | JSON Schema library integration (`any` type for `jsonSchema` in ParamsOneOf) | Zero-dependency constraint; use `interface{}` with runtime cast |
| NG-6 | `MergeNamedStreamReaders` + `SourceEOF` tracking | Advanced streaming; Phase 4+ |
| NG-7 | `SetAutomaticClose()` GC-based stream cleanup | Non-deterministic; low-priority |
| NG-8 | `StreamReaderWithConvert` typed element transformation | Phase 4+ |
| NG-9 | Schema-level stream types (`StreamReader[T]` as schema type) | Current `compose.PipeStreamReader` is sufficient |
| NG-10 | Provider-specific tool binding options (OpenAI `strict`, Gemini `functionDeclarations`) | Adapter implementation detail |
| NG-11 | Content filter / safety refusal handling beyond type definitions | Adapter implementation detail |
| NG-12 | Prompt cache retention / persistent caching features | Adapter implementation detail |
| NG-13 | Custom gob codecs for complex types (`ToolInfo.GobEncode`/`GobDecode`) | Not needed until `ParamsOneOf` uses real JSON Schema types |
| NG-14 | Tokenizer integration for token counting | Out of scope |
| NG-15 | LogProbsContent (advanced logprobs) | Rarely used; can be added later |

---

## 12. Implementation Phasing

```
Phase 1 (THIS NODE — Foundation)
├── compose/types.go            [MODIFY]  DataType, ChatMessagePartType, User alias
├── compose/schema.go           [MODIFY]  ToolCall.Index, ToolInfo.Extra, ParamsOneOf dual-mode,
│                                          ParameterInfo.SubParams/ElemInfo, ToolResult multi-modal,
│                                          ImageContent, AudioContent, VideoContent, FileContent, Document
├── compose/chatmodel.go        [MODIFY]  Message: +ResponseMeta, +ReasoningContent, +Extra,
│                                          +UserInputMultiContent, +AssistantGenMultiContent, +ToolName,
│                                          ResponseMeta, TokenUsage, LogProbs, MessageInputPart,
│                                          MessageOutputPart, provider extension stubs, UserMessage()
├── compose/concat.go           [NEW]     RegisterStreamChunkConcatFunc, ConcatItems,
│                                          ConcatMessages, ConcatMessageArray, ConcatToolResults,
│                                          concatToolCalls, sortToolCallsByIndex
├── compose/stream.go           [MODIFY]  init(): register concat functions
├── adapter/openai/adapter.go   [NEW]     ChatModel type, ConvertXxx signatures, mock types
├── adapter/claude/adapter.go   [NEW]     ChatModel type, ConvertXxx signatures, mock types
├── adapter/gemini/adapter.go   [NEW]     ChatModel type, ConvertXxx signatures, mock types
├── compose/concat_test.go      [NEW]     ConcatItems, ConcatMessages, ConcatToolResults tests
├── compose/schema_test.go      [NEW]     ToolCall.Index, ParameterInfo, ParamsOneOf, Document tests
├── compose/chatmodel_test.go   [MODIFY]  Extended Message, ResponseMeta, TokenUsage tests
│                                          (update existing tests for new fields)
├── adapter/*/adapter_test.go   [NEW]     Compile-time skeleton verification
├── go.mod                      [NOOP]    No new dependencies
│
Phase 2 (FUTURE NODE — Agentic + Provider Extensions)
├── compose/agentic_message.go  [NEW]     AgenticMessage, ContentBlock system, ConcatAgenticMessages
├── compose/openai/ext.go       [NEW]     ConcatResponseMetaExtensions, ConcatAssistantGenTextExtensions
├── compose/claude/ext.go       [NEW]     ConcatResponseMetaExtensions, ConcatAssistantGenTextExtensions
├── compose/gemini/ext.go       [NEW]     ConcatResponseMetaExtensions
├── compose/serialization.go    [NEW]     RegisterName + init() gob registrations
│
Phase 3 (FUTURE NODE — Docs)
├── README.md                   [MODIFY]  Chapter 6 section
├── cmd/example/main.go         [MODIFY]  Chapter 6 example
```

---

## 13. Risk Register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Breaking existing tests due to new Message fields | High | Medium | All new fields zero-value safe (nil/empty). Existing constructors (`HumanMessage`, `SystemMessage`) unchanged. Run full test suite before commit. |
| R2 | `compose/concat.go` references types from `compose/chatmodel.go` — same package, no cycle | Low | Low | Both in `package compose` — safe. Verify with `go build`. |
| R3 | `ParamsOneOf.jsonSchema` uses `any` → no compile-time type safety | Medium | Low | Acceptable for zero-dependency. Phase 2 can add concrete JSON Schema type if a pure-Go implementation is introduced. |
| R4 | Multiple `init()` functions in same package (concat.go + stream.go + serialization.go) | Low | Medium | Go supports multiple `init()` per package. Execution order is lexical by filename. Ensure `serialization.go` < `concat.go` alphabetically. |
| R5 | Provider extension stubs in `compose/chatmodel.go` pollute compose namespace | Medium | Low | Acceptable trade-off to avoid import cycles. Phase 2 sub-packages (`compose/openai/ext.go`) re-export cleaner wrappers. |
| R6 | `adapter/` packages reference `compose/*` types → need module path in go.mod | Low | Low | Already configured: `module github.com/rive/eino-compose-runtime-replica-go`. Internal import paths resolve correctly. |
| R7 | `ConcatItems` uses `reflect.ValueOf(fn).Call` — slower than direct call | Low | Low | Concat is called once per graph stream merge, not per chunk. Performance impact negligible. |
| R8 | `ToolCall.Index` is `*int` — nil dereference risk | Medium | Medium | All concat logic checks `tc.Index == nil` before dereference. Tests cover nil case explicitly. |

---

## 14. Implementation Order & Dependencies

```
compose/types.go  (no deps, add constants)
       ├── compose/schema.go     (depends: DataType from types.go)
       ├── compose/chatmodel.go  (depends: schema.go for ToolCall, types.go for ChatMessagePartType)
       └── compose/concat.go     (depends: chatmodel.go for Message, schema.go for ToolResult)
                │
       compose/stream.go (MODIFY: depends on concat.go for RegisterStreamChunkConcatFunc)
                │
       adapter/openai/adapter.go     (depends: compose/schema.go, compose/chatmodel.go)
       adapter/claude/adapter.go     (depends: compose/schema.go, compose/chatmodel.go)
       adapter/gemini/adapter.go     (depends: compose/schema.go, compose/chatmodel.go)
```

---

## 15. Delivery Checklist

Before reporting this node as `done`, verify:

- [ ] `ch6-implementation-contract.md` written to `examples/eino-compose-runtime-replica-go/research/`
- [ ] All API names assigned with exact Go signatures
- [ ] All file ownership assigned (`compose/types.go`, `compose/schema.go`, `compose/chatmodel.go`, `compose/concat.go`, `adapter/openai/adapter.go`, `adapter/claude/adapter.go`, `adapter/gemini/adapter.go`)
- [ ] All test files named with test case specifications
- [ ] Acceptance criteria enumerated (AC-1 through AC-16)
- [ ] Non-goals explicitly listed (NG-1 through NG-15)
- [ ] Risk register populated
- [ ] Phase 2 and Phase 3 file outlines included
- [ ] No production Go code modified (contract doc only)

---

*Contract finalized: 2026-06-06*
*Inputs: R1 (ch6-r1-current-schema-gap-audit.md, 703 lines) + R2 (ch6-r2-provider-schema-contract.md, 1480 lines)*
*Referenced Eino manual chapter: 06-schema-provider-adapters.md (506 lines)*
