# Chapter 6: Schema / Provider Adapter Interop

## 1. Problem

Eino is a multi-provider LLM application framework. A user composes a graph
that might use OpenAI for chat completion, Claude for reasoning, and Gemini
for embedding — all in the same pipeline. Each provider speaks a different wire
format, has a different message structure, different streaming protocol, and
different response metadata.

If every graph node knows which provider it talks to, switching providers
requires rewriting every node. If `compose/` (the orchestration engine)
branches on provider names, the engine is not generic. The core problem: **how
do you make components from different providers interoperate in a single
pipeline without any component knowing about the others?**

The schema layer (`schema/`) solves this by defining a canonical data model.
Provider adapters (in the external `eino-ext` repo) convert their native SDK
types into canonical types. The composition engine and ADK (`compose/`,
`adk/`) operate exclusively on canonical types. The critical boundary is the
component interface — `BaseModel[M]` is parameterized by message type `M`,
which is sealed to `*schema.Message` and `*schema.AgenticMessage`. The Go
type system catches mismatches at compile time.

## 2. Why It Is Hard

### 2.1 Messages Are Provider-Invented, Not Standardised

| Dimension | OpenAI | Claude | Gemini |
|-----------|--------|--------|--------|
| **Role names** | `"assistant"` | `"assistant"` | `"model"` |
| **Multimodal parts** | `content: [{type:"text", text:"..."}, {type:"image_url", image_url:{...}}]` | `content: [{type:"text", text:"..."}, {type:"image", source:{...}}]` | `parts: [{text:"..."}, {inlineData:{...}}]` |
| **Tool calls** | `tool_calls[]` with index-based streaming delta chunks | `tool_use` content blocks inside message content | `functionCall` inside `parts[]` |
| **Tool results** | Role `"tool"` message with `tool_call_id` | `tool_result` content block in a `user` message | `functionResponse` part in a `user` role |
| **Reasoning** | `reasoning_tokens` in usage details | Thinking content block | `thought` part |
| **Response ID** | `response.id` | `message.id` | `candidates[0].content.parts` union |

Naively picking one provider's format as the internal schema creates lock-in.
A canonical schema must accommodate all of these without preferring any.

### 2.2 Tool Parameter Schemas Vary by Provider

Some models accept flat `properties` objects. Others require full JSON Schema
with `anyOf`, `oneOf`, `$defs`. Some providers use server-side tool search
where tools are discovered dynamically, not pre-defined. A single parameter
schema representation must be translatable to every model API's expected format.

### 2.3 Streaming Chunks Merge Differently

Text chunks: simple string concatenation. Tool call chunks: merge by index,
concatenate JSON fragment arguments. Reasoning chunks: cumulative
accumulation. Image/audio/video chunks: non-mergeable (each is a standalone
artifact). The framework must register type-specific concat functions and
dispatch to them via Go generics at runtime.

### 2.4 Provider Extensions Must Not Leak Into Generic Code

Provider A has annotations (OpenAI). Provider B has citations with four
location types (Claude). Provider C has grounding metadata with search entry
points (Gemini). A generic graph node should remain unaware of these. Yet a
specialized component (like a RAG evaluator) must access them. The schema must
carry provider data without forcing every consumer to type-assert.

### 2.5 Serialization Must Survive Graph Interrupt/Resume

When a graph checkpoints (suspends) and resumes, the intermediate state —
messages, tool calls, multimodal parts, extension metadata — must survive a
round-trip through `encoding/gob`. Every canonical type used in state must be
pre-registered. Types with interface fields or recursive structures need custom
`GobEncode`/`GobDecode`.

## 3. Design Idea

Eino separates concerns into three layers:

```
Provider Adapters (eino-ext)          CONVERT native → canonical
    │ implements
Component Interfaces (components/)     GENERIC contracts (BaseModel[M])
    │ uses types from
Canonical Schema (schema/)             TYPES (Message, AgenticMessage, StreamReader, ToolInfo)
    │ includes
Provider Extensions (schema/openai,    OPTIONAL typed slots on canonical types
 schema/claude, schema/gemini)
```

Key design decisions:

1. **Two message models, not one.**
   - `Message` (`schema/message.go:497`): Classic text + `ToolCalls[]` model.
     Backward-compatible, channels-based multimodal input/output. Used by
     `BaseChatModel`.
   - `AgenticMessage` (`schema/agentic_message.go:71`): ContentBlock-based
     model with richer type system. Distinguishes FunctionToolCall,
     ServerToolCall, MCPToolCall, tool search, approval flows. Has typed
     provider extension slots. Used by `AgenticModel`.

2. **Provider extensions are data types, not implementations.**
   Each provider directory in `schema/` defines structs that slot into
   canonical types through typed pointer fields — never `map[string]any`.
   A component that does not care about provider data simply ignores the
   nil pointer. A component that does can type-assert.

3. **Generic interfaces enforce type safety.**
   `BaseModel[M messageType]` (`components/model/interface.go:36`) accepts
   only `*Message` and `*AgenticMessage`. You cannot pass a raw `map` or an
   arbitrary struct through the framework. The Go compiler enforces this.

4. **StreamReader[T] as universal streaming primitive.**
   `schema/stream.go:168` — not a simple channel wrapper. Supports array
   backing (zero-overhead from `StreamReaderFromArray`), fan-out via
   `Copy(n)`, fan-in via `MergeStreamReaders`, and type-safe conversion
   via `StreamReaderWithConvert`. Provider adapters convert their native
   SDK streams into `StreamReader[*Message]` or `StreamReader[*AgenticMessage]`.

5. **Registered concat dispatch.**
   `internal.RegisterStreamChunkConcatFunc[T]` (`internal/concat.go:71`)
   builds a type-indexed dispatch table. When `compose/` needs to merge a
   stream, it calls `internal.ConcatItems[T]`, which looks up the concat
   function registered for `T`. This is how `ConcatMessages` and
   `ConcatAgenticMessages` (which themselves call provider-specific
   extension concat logic) get wired into the generic stream merge path.

## 4. Source Walkthrough

### 4.1 `Message` — The Classic Model (`schema/message.go`)

```go
// schema/message.go:497
type Message struct {
    Role             RoleType              // Assistant | User | System | Tool
    Content          string                // plain text
    UserInputMultiContent []MessageInputPart   // multimodal input from user
    AssistantGenMultiContent []MessageOutputPart // multimodal output from model
    ToolCalls        []ToolCall            // assistant: tool calls requested
    ToolCallID       string                // tool: which call this responds to
    ToolName         string                // tool: name of the responding tool
    ResponseMeta     *ResponseMeta         // finish_reason, usage, logprobs
    ReasoningContent string                // thinking content
    Extra            map[string]any        // legacy provider-specific bag
}
```

**ToolCall** (`schema/message.go:132`): `{Index int, ID, Type, Function{Name,
Arguments string}, Extra}`. `Index` is critical for streaming — delta chunks
with the same `Index` belong to the same tool call. `Arguments` accumulate as
JSON fragments across chunks.

**MessageInputPart** (`schema/message.go:207`): typed union via a `Type`
discriminant — `Text`, `ImageURL`, `AudioURL`, `VideoURL`, `FileURL`,
`ToolSearchResult`.

**MessageOutputPart** (`schema/message.go:268`): analogous for model outputs —
`Text`, `Image`, `Audio`, `Video`, `Reasoning`.

### 4.2 `AgenticMessage` — The ContentBlock Model (`schema/agentic_message.go`)

```go
// schema/agentic_message.go:71
type AgenticMessage struct {
    Role         AgenticRoleType             // system | user | assistant (no "tool" role)
    ContentBlocks []*ContentBlock            // ordered list of typed blocks
    ResponseMeta *AgenticResponseMeta        // token usage + provider extensions
    Extra        map[string]any
}
```

**ContentBlock** (`schema/agentic_message.go:102`): tagged union with ~20
variants, each stored as a nullable pointer. The key innovation is that there
is no separate "tool result" role — tool calls and results are both content
blocks within the same message:

- Input blocks: `UserInputText`, `UserInputImage`, `UserInputAudio`,
  `UserInputVideo`, `UserInputFile`, `ToolSearchResult`
- Output blocks: `AssistantGenText`, `AssistantGenImage`, `AssistantGenAudio`,
  `AssistantGenVideo`, `Reasoning`
- Tool call blocks: `FunctionToolCall`, `ServerToolCall`, `MCPToolCall`
- Tool result blocks: `FunctionToolResult`, `ServerToolResult`, `MCPToolResult`
- MCP protocol blocks: `MCPListToolsResult`, `MCPToolApprovalRequest`,
  `MCPToolApprovalResponse`

**StreamingMeta** (`schema/agentic_message.go:174`): `{Index int}`. Each
streaming chunk carries an `Index` in its `ContentBlock.StreamingMeta`. When
concatenating (in `ConcatAgenticMessages`, line 897), blocks are grouped by
index and merged via type-specific functions.

**AgenticResponseMeta** (`schema/agentic_message.go:85`): carries `TokenUsage`
plus typed provider extension slots:

```go
OpenAIExtension *openai.ResponseMetaExtension
GeminiExtension *gemini.ResponseMetaExtension
ClaudeExtension  *claude.ResponseMetaExtension
Extension       any  // fallback for unknown/custom providers
```

**AssistantGenText** (`schema/agentic_message.go:234`): also has
`OpenAIExtension *openai.AssistantGenTextExtension` and `ClaudeExtension
*claude.AssistantGenTextExtension` for per-text-block annotations/citations.

### 4.3 `ToolInfo` — Dual-Mode Parameter Schema (`schema/tool.go`)

`ToolInfo` (`schema/tool.go:128`) describes a tool to the model. The
critical field is `*ParamsOneOf`, which is exactly one of two modes:

1. **`NewParamsOneOfByParams(map[string]*ParameterInfo)`** (line 283):
   Lightweight. A flat map of `ParameterInfo{Type, ElemInfo, SubParams, Desc,
   Enum, Required}`. Supports recursive `ParameterInfo` for nested
   objects and arrays.

2. **`NewParamsOneOfByJSONSchema(*jsonschema.Schema)`** (line 290): Full JSON
   Schema 2020-12. Used by `utils.InferTool` which auto-generates schema from
   Go struct tags. Required for `anyOf`, `oneOf`, `$defs`.

Conversion: `ParamsOneOf.ToJSONSchema()` (`tool.go:297`) normalizes both
modes to `*jsonschema.Schema` for passing to model APIs. Most provider
adapters call this before marshalling tool schemas for their native API.

### 4.4 `StreamReader[T]` — Universal Streaming (`schema/stream.go`)

`StreamReader[T]` (`schema/stream.go:168`) is a polymorphic reader with
five internal backends:

```
readerTypeStream   → channel-based (Pipe)
readerTypeArray    → slice-based (StreamReaderFromArray)
readerTypeMultiStream  → fan-in (MergeStreamReaders)
readerTypeWithConvert  → element-wise transform (StreamReaderWithConvert)
readerTypeChild   → fan-out (Copy)
```

Key operations:

- **`Pipe[T](cap)`** (line 99): Creates a paired `StreamReader` +
  `StreamWriter`. One sender goroutine calls `sw.Send(chunk, err)`,
  one receiver calls `sr.Recv()`. Close signals `io.EOF`.

- **`StreamReaderFromArray[T](arr)`** (line 461): Zero-overhead reader backed
  by a slice. `Recv()` returns elements in order, then `io.EOF`.

- **`Copy(n int)`** (line 261): Fan-out — creates `n` independent child
  readers using a linked-list shared buffer. Each child must be closed
  independently. Used when a stream feeds multiple consumers (callback
  handlers + downstream nodes).

- **`MergeStreamReaders[T](srs)`** (line 912): Fan-in — interleaves multiple
  streams into one. Chunks from all sources arrive in arrival order, not
  source order.

- **`MergeNamedStreamReaders[T](srs, names)`** (line 990): Fan-in with
  source identification. Emits `SourceEOF` errors with the source name
  when individual streams end, so consumers can track per-source completion.

- **`StreamReaderWithConvert[T,D](sr, convert)`** (line 691): Element-wise
  transformation. The `convert` function maps `T → (D, error)`. Return
  `ErrNoValue` to filter an element out of the stream.

**Why polymorphism matters:** If `StreamReader` were always channel-backed,
`StreamReaderFromArray` would require a goroutine to pump elements. With
multiple backends, the runtime picks the optimal path. The compose layer
(`compose/stream_reader.go`) wraps this with an internal `streamReader`
interface that adds `copy`, `merge`, `mergeWithNames`, `withKey`, and
`toAnyStreamReader`.

### 4.5 Stream Concatenation (`schema/message.go`)

When a stream yields partial chunks, the framework must merge them into
complete messages. This is registered once per type:

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

`ConcatMessages` (`schema/message.go:1643`):
- Concatenates `Content` string (plain text accumulation).
- Concatenates `ReasoningContent`.
- Merges `ToolCalls` via `concatToolCalls` (line 1283): groups chunks by
  `Index`, validates consistent ID/Type/Name within each group, concatenates
  `Arguments` JSON fragments, sorts final calls by index.
- Merges multimodal content via `concatAssistantMultiContent` /
  `concatUserMultiContent`.
- Keeps the last non-nil `ResponseMeta` (finish reason, usage arrive at end).

`ConcatAgenticMessages` (`schema/agentic_message.go:897`): Groups
`ContentBlock`s by `StreamingMeta.Index`. Each group is concatenated by
type-specific functions (`concatAssistantGenTexts`,
`concatFunctionToolCalls`, etc.). Provider extension slots are merged via
`concatAgenticResponseMeta` (line 1002), which calls per-provider helpers:
`openai.ConcatResponseMetaExtensions`, `claude.ConcatResponseMetaExtensions`,
`gemini.ConcatResponseMetaExtensions`. For the `Extension any` fallback, it
uses `internal.ConcatSliceValue` (runtime type assertion + append).

### 4.6 Provider Extensions (`schema/openai/`, `schema/claude/`, `schema/gemini/`)

**OpenAI (`schema/openai/extension.go`):**

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
    Refusal     *OutputRefusal      // content filter refusal reason
    Annotations []*TextAnnotation    // file citations, URL citations
}
```

`ConcatAssistantGenTextExtensions` (line 116): merges annotations by
de-duplicating on `Index`, concatenates refusal reasons. `TextAnnotation`
(line 59) has four location types: `FileCitation`, `URLCitation`, `FilePath`
(with file ID), and `ContainerFileCitation` (with character offsets).

**Claude (`schema/claude/extension.go`):**

```go
type ResponseMetaExtension struct {
    ID, StopReason, StopSequence string
    StopDetails  *StopDetails     // category, explanation
}

type AssistantGenTextExtension struct {
    Citations []*TextCitation
}
```

`TextCitation` (line 39): typed union of `CitationCharLocation`,
`CitationPageLocation`, `CitationContentBlockLocation`,
`CitationWebSearchResultLocation`. Each carries `CitedText`,
`DocumentTitle`, `DocumentIndex`. Cited position is expressed as character
offsets, page numbers, content block indices, or web search result indices.

`ConcatAssistantGenTextExtensions` (line 88): simply appends citations from
all chunks — citations typically appear in the final chunk, not per-delta.

**Gemini (`schema/gemini/extension.go`):**

```go
type ResponseMetaExtension struct {
    ID, FinishReason string
    GroundingMeta    *GroundingMetadata
}

type GroundingMetadata struct {
    GroundingChunks   []*GroundingChunk   // web sources (domain, title, URI)
    GroundingSupports []*GroundingSupport  // confidence scores, segment info
    SearchEntryPoint  *SearchEntryPoint    // rendered content, SDK blob
    WebSearchQueries []string              // follow-up search queries
}
```

### 4.7 Serialization (`schema/serialization.go`)

Graph checkpoint persistence requires every type used in intermediate state
to be pre-registered with both `encoding/gob` and a custom serializer:

```go
// schema/serialization.go:27-56
func init() {
    RegisterName[*Message]("_eino_message")
    RegisterName[*AgenticMessage]("_eino_agentic_message")
    RegisterName[ToolCall]("_eino_tool_call")
    RegisterName[ResponseMeta]("_eino_response_meta")
    RegisterName[TokenUsage]("_eino_token_usage")
    // ... ~20 more type registrations
}
```

`RegisterName[T](name)` (`schema/serialization.go:83`): calls
`gob.RegisterName` and `serialization.GenericRegister[T]`. The `GenericRegister`
builds a `reflect.Type → name` mapping used by the checkpoint store to
decode serialized state back into the correct concrete type.

Types with complex internal structures implement custom gob codecs. For
example, `ToolInfo` (`tool.go:194`): its `ParamsOneOf` union (either a
`map[string]*ParameterInfo` or a `*jsonschema.Schema`) is serialized via
`toolInfoForGob` which JSON-encodes the schema branch into a string field.

### 4.8 Component-to-Schema Bridge

The `compose/component_to_graph_node.go` converts component interfaces into
graph nodes. Each `to*Node` function wraps the component's methods into a
`composableRunnable`. The key function `parseExecutorInfoFromComponent`
(approximately line 50): checks if the component implements `Typer`
(`GetType()`) and `Checker` (`IsCallbacksEnabled()`), extracting
metadata that the graph runtime uses to decide when to fire callbacks.

The graph scheduler never sees component types directly — it sees
`composableRunnable` with `inputType` and `outputType` fields typed to
`*schema.Message` or `*schema.AgenticMessage`. This abstraction is why adding
a new provider model requires only implementing `BaseModel[M]` — no graph
changes.

## 5. Patterns and Examples

### 5.1 Building a Multimodal User Message

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

### 5.2 Building Agentic Tool Messages

```go
// Assistant message requesting a tool call
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

// User message carrying the tool result (note: no separate "tool" role)
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

Agentic messages differ from classic `Message`: tool results are content
blocks within a `user` role message, not separate `tool` role messages. This
maps more naturally to providers like Claude and Gemini where tool results
are user-turn content, and OpenAI's Responses API where tool results are
inline.

### 5.3 Defining a Tool With Nested Parameters

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

For complex schemas with `anyOf`/`oneOf`, use `NewParamsOneOfByJSONSchema`:

```go
schema, _ := jsonschema.NewSchemaFromFile("search_schema.json")
tool := &schema.ToolInfo{
    Name: "search",
    Desc: "Advanced search",
    ParamsOneOf: schema.NewParamsOneOfByJSONSchema(schema),
}
```

### 5.4 Converting a Pipe Stream to Array for Testing

```go
sr, sw := schema.Pipe[*schema.Message](3)
go func() {
    defer sw.Close()
    sw.Send(&schema.Message{Role: schema.Assistant, Content: "Hello"}, nil)
    sw.Send(&schema.Message{Role: schema.Assistant, Content: " World"}, nil)
}()

// Concatenate stream into a complete message
arraysr, _ := schema.StreamReaderWithConvert(sr,
    func(v *schema.Message) (*schema.Message, error) {
        return v, nil
    })
// Or collect all chunks:
var chunks []*schema.Message
for {
    chunk, err := sr.Recv()
    if errors.Is(err, io.EOF) { break }
    chunks = append(chunks, chunk)
}
complete, _ := schema.ConcatMessages(chunks)
// complete.Content == "Hello World"
```

### 5.5 Fan-Out with Copy for Callback Observation

```go
sr, _ := model.Stream(ctx, messages)  // single stream from model
children := sr.Copy(3)                // original + 2 callback copies
defer children[0].Close()
defer children[1].Close()
defer children[2].Close()

// children[0] → downstream graph node
// children[1] → timing callback handler
// children[2] → logging callback handler
```

### 5.6 Accessing Provider Extension Metadata

```go
resp, _ := agenticModel.Generate(ctx, msgs)

if resp.ResponseMeta != nil {
    // Check OpenAI-specific metadata
    if oe := resp.ResponseMeta.OpenAIExtension; oe != nil {
        fmt.Printf("OpenAI response ID: %s, Service tier: %v\n",
            oe.ID, oe.ServiceTier)
    }

    // Check Claude-specific metadata
    if ce := resp.ResponseMeta.ClaudeExtension; ce != nil {
        fmt.Printf("Claude stop reason: %s\n", ce.StopReason)
    }

    // Check Gemini grounding metadata
    if ge := resp.ResponseMeta.GeminiExtension; ge != nil {
        if gm := ge.GroundingMeta; gm != nil {
            for _, ch := range gm.GroundingChunks {
                fmt.Printf("Grounded on: %s (%s)\n", ch.Web.Title, ch.Web.URI)
            }
        }
    }
}

// Access per-text-block annotations (OpenAI)
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

### 5.7 Registering Custom Types for Checkpoint Persistence

```go
// In your component package init()
func init() {
    schema.RegisterName[*MyState]("_myapp_state")
    schema.RegisterName[MyCustomToolResult]("_myapp_tool_result")
}

// MyState will now survive graph interrupt/resume
```

## 6. Common Pitfalls

### 6.1 Confusing Message Model Choice

Use `Message` + `BaseChatModel` for classic chat apps with function calling
(tools). Use `AgenticMessage` + `AgenticModel` for agent apps that need MCP
tools, server tools, tool search, or structured multimodal outputs. Mixing
them — passing `*Message` to an `AgenticModel` — is a compile error thanks
to the type constraint on `BaseModel[M]`.

### 6.2 Not Closing Stream Copies

`StreamReader.Copy(n)` creates `n` independent child readers backed by a
shared buffer. Every child MUST call `Close()` after consumption. If one child
leaks, the parent's underlying goroutine never terminates. This is the most
common goroutine leak in Eino graphs. `SetAutomaticClose()` (line 279) helps
but relies on garbage collection, not deterministic cleanup.

### 6.3 Assuming Stream Merging Preserves Order

`MergeStreamReaders` interleaves chunks from all sources in arrival order.
If you need per-source ordering (e.g., concatenate source A's chunks before
source B's), use `MergeNamedStreamReaders` and track `SourceEOF` errors, or
collect each source separately and concatenate.

### 6.4 Relying on `Extra` Instead of Extension Slots

`Message.Extra` (`map[string]any`) exists but bypasses type safety. If you
store provider-specific data in `Extra`, downstream code must discover which
provider produced it and type-assert each value. Use the typed extension
slots (`OpenAIExtension`, `ClaudeExtension`, `GeminiExtension`) on
`AgenticResponseMeta` and `AssistantGenText` — the framework's concat
functions understand them; `map[string]any` concat is last-write-wins.

### 6.5 Missing Serialization Registration

If your graph uses a custom type in state (e.g., a user-defined aggregator
struct), and you enable checkpointing without calling
`schema.RegisterName[T]("_name")` in an `init()`, the checkpoint store will
fail at encode/decode time with an opaque gob error. Every type that crosses
the interrupt/resume boundary must be pre-registered.

### 6.6 Mixing ParamsOneOf Modes

A `ToolInfo` can have exactly one `ParamsOneOf` mode — either `params` or
`jsonschema`, not both. If you construct with `NewParamsOneOfByParams` and
then overwrite the `ParamsOneOf` field with a `NewParamsOneOfByJSONSchema`,
both pointers live in the struct but `ToJSONSchema()` only checks
`p.params != nil` first (line 302), so the JSON Schema branch is silently
ignored.

### 6.7 Streaming Tool Calls Without Index

In streaming, `ToolCall.Index` identifies which tool call a delta chunk
belongs to. If a provider adapter sets `Index = 0` for every chunk of every
tool call, `concatToolCalls` merges all chunks into one (invalid) call.
Ensure each distinct tool call gets a unique `Index`, and increments
correctly across the stream.

## 7. What Rive Can Learn

### 7.1 Canonical Schema as Integration Surface

Eino's `schema/` package is the single source of truth for all data types
that flow between components. Provider packages in `eino-ext` depend on
`schema/` (not vice versa). This is the Dependency Inversion Principle
applied to data: high-level modules define the data types; low-level
implementations conform to them. Rive's plugin system should define a
canonical schema package that plugins import — not a per-plugin data
format.

### 7.2 Typed Extension Slots Over Generic Maps

The `ResponseMeta.OpenAIExtension *openai.ResponseMetaExtension` pattern
(with nil = absent) is strictly better than `Extra map[string]any`. It gives
the concat functions a typed contract (they know exactly how to merge), it
gives the compiler something to check, and it gives IDE autocomplete concrete
fields. Rive should favor typed optional struct fields over generic extension
bags for plugin-specific data.

### 7.3 Type-Constraint Sealing for Generics

`type messageType interface { *schema.Message | *schema.AgenticMessage }`
uses a Go 1.18+ union constraint to seal `BaseModel[M]` to exactly two
concrete types. This prevents a third-party from passing `BaseModel[MyType]`
through the framework, which would break graph compilation. Rive can use
similar union constraints to seal its own generic execution interfaces.

### 7.4 Registered Dispatch Tables for Extensibility

`internal.RegisterStreamChunkConcatFunc[T]` builds a `reflect.Type → func`
map. When the compose layer encounters a stream of type `T`, it calls
`internal.ConcatItems[T]`, which dispatches to the registered concat
function — without knowing what `T` is. This is the Go-generics equivalent
of a plugin registry. Rive can use this pattern for its own extensible
operations (serialization, validation, merging) where new types need to
register handlers without modifying the core engine.

### 7.5 Streaming Abstraction Polymorphism

`StreamReader[T]` switching between channel, array, multi-stream, and
convert backends is more sophisticated than a simple `chan`. The compose
layer's `streamReader` internal interface further adds copy/merge/withKey.
Rive's streaming primitives should similarly hide backend differences —
making `StreamReader` behave the same whether data comes from a live
goroutine, a pre-computed array, or a merge of multiple sources.

### 7.6 Bidirectional Message Model Compatibility

Having both `Message` and `AgenticMessage` creates a migration path: existing
graphs built on `BaseChatModel` continue to work; new graphs adopt
`AgenticModel` with richer semantics. The two models coexist because they
share the `messageType` constraint. Rive should design its data model
evolution with similar graceful coexistence — not a breaking migration.
