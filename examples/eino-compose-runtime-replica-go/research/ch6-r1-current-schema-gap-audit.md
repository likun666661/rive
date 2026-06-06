# R1 Current Replica Schema Gap Audit: Chapter 06 (Schema / Provider Adapters)

> Audit target: Compare Eino technical manual Chapter 06 (`06-schema-provider-adapters.md`, 506 lines) against the current Go replica (`compose/` + existing `research/` docs), identify schema/stream/concat/serialisation gaps in the replica, and propose file-level implementation opportunities with exact test requirements.
> This is a research/audit node. Do not implement production code.

---

## 1. What Problem Chapter 06 Solves

### 1.1 The Core Problem

Eino is a multi-provider LLM application framework. Users compose a Graph that may use OpenAI for chat completion, Claude for reasoning, Gemini for embedding — all in the same pipeline. Each provider uses a different wire format: different Message structures, different streaming protocols, different response metadata.

If every graph node knows which provider it talks to, switching providers requires rewriting every node. If `compose/` (the orchestration engine) branches on provider name, the engine is no longer generic. **The core problem: how do components from different providers interoperate in a single pipeline without any component knowing about other components?**

### 1.2 The Solution Strategy (Three-Layer Separation)

Eino separates concerns into three layers (manual §3):

```
Provider Adapters (eino-ext)              Convert native types → canonical types
  │ implements
Component Interfaces (components/)        Generic contracts (BaseModel[M])
  │ uses types
Canonical Schema (schema/)                Types (Message, AgenticMessage, StreamReader, ToolInfo)
  │ contains
Provider Extensions (schema/openai,       Optional typed slots on canonical types
 schema/claude, schema/gemini)
```

Five key design decisions power this:

1. **Two Message models, not one.** `Message` (classic text + ToolCalls) for `BaseChatModel`; `AgenticMessage` (ContentBlock-based) for `AgenticModel`. Both share the `messageType` Go union constraint.
2. **Provider extensions are data types, not implementations.** Each provider directory defines structs embedded via typed pointer fields — never `map[string]any`. Components that don't care simply ignore nil pointers.
3. **Generic interfaces enforce type safety.** `BaseModel[M messageType]` accepts only `*Message` or `*AgenticMessage`. You cannot pass raw maps or arbitrary structs through the framework.
4. **StreamReader[T] is a universal streaming primitive.** Five internal backends (channel, array, multi-stream, with-convert, child) hide implementation differences behind a single polymorphic interface.
5. **Registration-based Concat dispatch.** `RegisterStreamChunkConcatFunc[T]` builds a `reflect.Type → func` dispatch table. When `compose/` needs to merge a stream, it calls `ConcatItems[T]` which dispatches to the registered concat function without knowing what `T` is.

### 1.3 Summary of Key Sub-Problems

| Sub-problem | Why it's hard | Eino's solution |
|---|---|---|
| Messages are provider-invented, no standard | OpenAI uses `tool_calls[]`, Claude embeds `tool_use` in content, Gemini uses `functionCall` in `parts[]` | `schema.Message` + `schema.AgenticMessage` canonical models |
| Tool parameter schemas differ by provider | Some accept flat `properties`, others require full JSON Schema with `anyOf`/`oneOf`/`$defs` | `ParamsOneOf` dual-mode (lightweight `ParameterInfo` + full `*jsonschema.Schema`) |
| Stream chunks merge differently | Text = string concat, ToolCalls = index-grouped JSON concat, Reasoning = cumulative overlay, Image = non-mergeable | Registration-based type-specific concat functions |
| Provider extensions must not leak to generic code | OpenAI has annotations, Claude has 4 location-type citations, Gemini has Grounding metadata | Typed extension slots (`OpenAIExtension *openai.Thing`) — nil = absent |
| Serialisation must survive graph suspend/resume | Graph checkpoint → persist → resume requires `encoding/gob` round-trip of all state types | `RegisterName[T]("_name")` in `init()`; custom `GobEncode`/`GobDecode` for complex types |

---

## 2. Current Replica State: What Exists Today

### 2.1 File Inventory

| File | Contents | Role |
|---|---|---|
| `compose/schema.go` (33 lines) | `ToolCall`, `ToolCallFunction`, `ToolInfo`, `ParamsOneOf`, `ParameterInfo`, `ToolResult` | Minimal tool data model |
| `compose/chatmodel.go` (164 lines) | `RoleType`, `Message`, `ChatModel` interface, `FakeChatModel` | Chat model + message |
| `compose/retriever.go` (75 lines) | `Document`, `Query`, `Retriever` interface, `FakeRetriever` | Retriever component |
| `compose/prompt_tool_bridge.go` (143 lines) | `BridgeTool` interface, `toolsNodeBridge`, `promptTemplateBridge` | Tool bridge adapters |
| `compose/prompt.go` | `ChatTemplate`, `MessageTemplate` | Prompt template |
| `compose/stream.go` | `PipeStreamReader`, `PipeStreamWriter`, `Copy`, `Merge`, `Concat` | Basic streaming |
| `compose/callbacks.go` | `CallbackWrapper`, `RunInfo`, `Handler` | Callback infrastructure |
| `compose/runnable.go` | `Runnable[I,O]`, `composableRunnable`, `Lambda` | Runnable abstraction |
| `compose/types.go` | `NodeTriggerMode`, `ComponentType`, sentinel errors | Core types |

### 2.2 Schema Types: Current vs Chapter 06 Target

#### Message (`compose/chatmodel.go:19-25`)

| Field | Current replica | Chapter 06 `schema.Message` | Status |
|---|---|---|---|
| `Role` | `RoleType` (system/human/assistant/tool) | `RoleType` (Assistant/User/System/Tool) | Present but naming differs |
| `Content` | `string` | `string` | Present |
| `ToolCalls` | `[]ToolCall` | `[]ToolCall` | Present |
| `ToolCallID` | `string` | `string` | Present |
| `Name` | `string` | Not in Eino core `Message` but used by some providers | Present |
| `UserInputMultiContent` | **Missing** | `[]MessageInputPart` (type union: Text, ImageURL, AudioURL, VideoURL, FileURL, ToolSearchResult) | **Gap** |
| `AssistantGenMultiContent` | **Missing** | `[]MessageOutputPart` (type union: Text, Image, Audio, Video, Reasoning) | **Gap** |
| `ResponseMeta` | **Missing** | `*ResponseMeta` (finish_reason, usage, logprobs) | **Gap** |
| `ReasoningContent` | **Missing** | `string` | **Gap** |
| `Extra` | **Missing** | `map[string]any` legacy extension bag | **Gap** |

#### ToolCall (`compose/schema.go:3-7`)

| Field | Current replica | Chapter 06 `schema.ToolCall` | Status |
|---|---|---|---|
| `ID` | `string` | `string` | Present |
| `Type` | `string` | `string` | Present |
| `Function` | `ToolCallFunction{Name, Arguments}` | `FunctionCall{Name, Arguments}` | Present |
| `Index` | **Missing** | `*int` (streaming: identifies which tool call delta chunks belong to) | **Critical Gap** |
| `Extra` | **Missing** | `map[string]any` | Gap |

#### ToolInfo (`compose/schema.go:14-18`)

| Field | Current replica | Chapter 06 `schema.ToolInfo` | Status |
|---|---|---|---|
| `Name` | `string` | `string` | Present |
| `Desc` | `string` | `string` | Present |
| `ParamsOneOf` | `*ParamsOneOf` | `*ParamsOneOf` | Present |
| `Extra` | **Missing** | `map[string]any` | Gap |

#### ParamsOneOf (`compose/schema.go:20-22`)

| Field | Current replica | Chapter 06 `schema.ParamsOneOf` | Status |
|---|---|---|---|
| `Params` | `map[string]*ParameterInfo` | `*paramsOneOf` with dual-mode | Present but incomplete |
| **JSON Schema branch** | **Missing** | `*jsonschema.Schema` (full JSON Schema 2020-12 support) | **Critical Gap** |
| `ToJSONSchema()` method | **Missing** | Converts both modes to `*jsonschema.Schema` for provider API calls | **Critical Gap** |

#### ParameterInfo (`compose/schema.go:24-29`)

| Field | Current replica | Chapter 06 `schema.ParameterInfo` | Status |
|---|---|---|---|
| `Type` | `string` | `DataType` (string enum with constants: String, Integer, Boolean, Number, Object, Array) | Present but untyped |
| `Desc` | `string` | `string` | Present |
| `Required` | `bool` | `bool` | Present |
| `Enum` | `[]string` | `[]string` | Present |
| `SubParams` | **Missing** | `map[string]*ParameterInfo` (recursive for nested objects) | **Gap** |
| `ElemInfo` | **Missing** | `*ParameterInfo` (for array element type) | **Gap** |

#### ToolResult (`compose/schema.go:31-33`)

| Field | Current replica | Chapter 06 `schema.ToolResult` | Status |
|---|---|---|---|
| `Text` | `string` | `string` | Present |
| `Images` | **Missing** | `[]*ImageContent` | **Gap** |
| `Audio` | **Missing** | `[]*AudioContent` | **Gap** |
| `Video` | **Missing** | `[]*VideoContent` | **Gap** |
| `Files` | **Missing** | `[]*FileContent` | **Gap** |

#### Missing Entirely: AgenticMessage & ContentBlock System

Chapter 06 defines a second message model (`AgenticMessage`, manual §4.2) with ~20 `ContentBlock` variants:

- **Input blocks**: `UserInputText`, `UserInputImage`, `UserInputAudio`, `UserInputVideo`, `UserInputFile`, `ToolSearchResult`
- **Output blocks**: `AssistantGenText`, `AssistantGenImage`, `AssistantGenAudio`, `AssistantGenVideo`, `Reasoning`
- **Tool call blocks**: `FunctionToolCall`, `ServerToolCall`, `MCPToolCall`
- **Tool result blocks**: `FunctionToolResult`, `ServerToolResult`, `MCPToolResult`
- **MCP protocol blocks**: `MCPListToolsResult`, `MCPToolApprovalRequest`, `MCPToolApprovalResponse`

The current replica has **zero** AgenticMessage support. This is a full gap.

### 2.3 StreamReader: Current vs Chapter 06 Target

| Feature | Current replica (`compose/stream.go`) | Chapter 06 `schema.StreamReader[T]` | Status |
|---|---|---|---|
| Channel backend (Pipe) | `PipeStreamReader` + `PipeStreamWriter` | `Pipe[T](cap)` → `StreamReader` + `StreamWriter` | Present |
| Array backend | **Missing** | `StreamReaderFromArray[T](arr)` — zero-overhead slice reader | **Gap** |
| Multi-stream merge | `Merge` (basic) | `MergeStreamReaders[T]` — arbitrary number of sources interleaved | Partial |
| Named merge | **Missing** | `MergeNamedStreamReaders[T]` — source-identified fan-in with `SourceEOF` | **Gap** |
| Type-safe convert | **Missing** | `StreamReaderWithConvert[T,D](sr, convert)` — element-wise transformation | **Gap** |
| Copy (fan-out) | `Copy` (basic) | `Copy(n)` — linked-list shared buffer, `n` independent child readers | Partial |
| Polymorphic backend selection | All channel-based | 5 internal backends selected based on source | **Critical Gap** |
| `Close()` semantics | Present | Present | Present |
| `SetAutomaticClose()` | **Missing** | Auto-close via GC (non-deterministic cleanup) | **Gap** |

### 2.4 Stream Concat: Current vs Chapter 06 Target

| Feature | Current replica | Chapter 06 | Status |
|---|---|---|---|
| `ConcatMessages` | **Missing** | Content string concat, ReasoningContent concat, ToolCalls merge, MultiContent merge, ResponseMeta propagation | **Critical Gap** |
| `ConcatAgenticMessages` | **Missing** | ContentBlock grouping by StreamingMeta.Index, type-specific concat per block type, provider extension merge | **Critical Gap** |
| `ConcatToolResults` | **Missing** | Tool result merge for multi-modal outputs | **Gap** |
| Registration dispatch | **Missing** | `RegisterStreamChunkConcatFunc[T]` → `ConcatItems[T]` reflect.Type-based dispatch | **Critical Gap** |
| `ConcatMessageArray` | **Missing** | Array-level concat (single element per stream position) | **Gap** |

### 2.5 Provider Extensions: Current vs Chapter 06 Target

The current replica has **zero** provider extension types. Chapter 06 defines:

#### OpenAI (`schema/openai/extension.go` — manual §4.6)

| Extension type | Fields | Status |
|---|---|---|
| `ResponseMetaExtension` | ID, Status, PreviousResponseID, Error, IncompleteDetails, Reasoning, ServiceTier, CreatedAt, PromptCacheRetention | **Missing** |
| `AssistantGenTextExtension` | Refusal (content filter), Annotations (FileCitation, URLCitation, FilePath, ContainerFileCitation) | **Missing** |
| `ConcatResponseMetaExtensions` | Concatenation logic | **Missing** |
| `ConcatAssistantGenTextExtensions` | Annotation dedup by Index | **Missing** |

#### Claude (`schema/claude/extension.go` — manual §4.6)

| Extension type | Fields | Status |
|---|---|---|
| `ResponseMetaExtension` | ID, StopReason, StopSequence, StopDetails | **Missing** |
| `AssistantGenTextExtension` | Citations (CitationCharLocation, CitationPageLocation, CitationContentBlockLocation, CitationWebSearchResultLocation) | **Missing** |
| `ConcatResponseMetaExtensions` | Concatenation logic | **Missing** |
| `ConcatAssistantGenTextExtensions` | Simple append | **Missing** |

#### Gemini (`schema/gemini/extension.go` — manual §4.6)

| Extension type | Fields | Status |
|---|---|---|
| `ResponseMetaExtension` | ID, FinishReason, GroundingMeta | **Missing** |
| `GroundingMetadata` | GroundingChunks (Web source: domain, title, URI), GroundingSupports (confidence, passage info), SearchEntryPoint, WebSearchQueries | **Missing** |
| `ConcatResponseMetaExtensions` | Concatenation logic | **Missing** |

### 2.6 Serialisation: Current vs Chapter 06 Target

| Feature | Current replica | Chapter 06 (`schema/serialization.go`) | Status |
|---|---|---|---|
| `RegisterName[T](name)` | **Missing** | `gob.RegisterName` + `serialization.GenericRegister[T]` | **Critical Gap** |
| Type registration in `init()` | **Missing** | ~20 types registered: `*Message`, `*AgenticMessage`, `ToolCall`, `ResponseMeta`, `TokenUsage`, etc. | **Critical Gap** |
| Custom gob codecs for complex types | **Missing** | `ToolInfo.GobEncode`/`GobDecode` for `ParamsOneOf` union | **Gap** |
| `serialization.GenericRegister` | **Missing** | Builds `reflect.Type → name` mapping for checkpoint decode | **Gap** |

### 2.7 Additional Schema Types From Chapter 06 (Fully Missing)

| Type | Description | Status |
|---|---|---|
| `ResponseMeta` | finish_reason, usage, logprobs metadata | **Missing** |
| `TokenUsage` | Prompt, Completion, Total token counts | **Missing** |
| `MessageInputPart` | Type discriminator union for multi-modal user input | **Missing** |
| `MessageOutputPart` | Type discriminator union for multi-modal model output | **Missing** |
| `MessagePartCommon` | URL, base64 data container for multi-modal parts | **Missing** |
| `ChatMessagePartType` | Enum: Text, ImageURL, AudioURL, VideoURL, FileURL, ToolSearchResult | **Missing** |
| `RoleType` | Assistant/User/System/Tool (schema-level, not compose-level) | Partially present in `compose/chatmodel.go` but uses different naming (Human vs User) |
| `StreamReader[T]` generic type | The universal streaming primitive with polymorphic backends | Partially present as non-generic `PipeStreamReader` |
| `StreamWriter[T]` | Paired writer from `Pipe[T]` | Present as `PipeStreamWriter` |
| `AgenticRoleType` | system/user/assistant (no "tool" role) | **Missing** |
| `ContentBlockType` | ~20 content block type constants | **Missing** |
| `StreamingMeta` | `{Index int}` for ContentBlock stream grouping | **Missing** |
| `AgenticResponseMeta` | TokenUsage + typed provider extension slots | **Missing** |

---

## 3. Gap Severity Summary

### Critical Gaps (Blocking for stream concat, checkpoint, multi-provider)

| # | Gap | Impact |
|---|---|---|
| GAP-C6-1 | `ToolCall.Index` field missing | Streaming tool calls cannot be merged (all deltas collapse to one call) |
| GAP-C6-2 | `ParamsOneOf` JSON Schema branch + `ToJSONSchema()` | Cannot pass complex tool schemas (`anyOf`/`oneOf`/`$defs`) to model APIs |
| GAP-C6-3 | `ConcatMessages` function | Multi-chunk streams cannot be merged into complete Messages |
| GAP-C6-4 | `ConcatAgenticMessages` function | Agentic multi-chunk streams cannot be merged |
| GAP-C6-5 | `RegisterStreamChunkConcatFunc` + `ConcatItems` dispatch | No extensible concat mechanism for graph stream merge paths |
| GAP-C6-6 | `RegisterName[T]` serialisation | Checkpoint persistence impossible without gob registration of state types |
| GAP-C6-7 | No `AgenticMessage` / `ContentBlock` system | Cannot represent tool-call-within-content models, MCP tools, server-side tools |
| GAP-C6-8 | `StreamReader[T]` polymorphic backends | All streaming goes through channel; no zero-overhead array path, no merge, no convert |
| GAP-C6-9 | No provider extension types | Cannot carry provider-specific metadata (annotations, citations, grounding) |

### High Gaps (Needed for full Eino parity)

| # | Gap | Impact |
|---|---|---|
| GAP-C6-10 | `Message.ResponseMeta` + `TokenUsage` | Cannot surface finish reason, token counts to callers |
| GAP-C6-11 | `Message.UserInputMultiContent` / `AssistantGenMultiContent` | Cannot represent multi-modal input/output (images, audio, video) |
| GAP-C6-12 | `Message.ReasoningContent` | Cannot capture chain-of-thought / thinking content |
| GAP-C6-13 | `ParameterInfo.SubParams` + `ElemInfo` | Cannot define nested or array-typed tool parameters |
| GAP-C6-14 | `Schema.ToolResult` multi-modal fields (Images/Audio/Video/Files) | Enhanced tools returning rich media cannot pass results |
| GAP-C6-15 | `MergeNamedStreamReaders` + `SourceEOF` | Cannot track completion per-source in fan-in scenarios |

### Medium Gaps (Enhancements)

| # | Gap | Impact |
|---|---|---|
| GAP-C6-16 | `StreamReader.SetAutomaticClose()` | Stream goroutine leak protection is non-deterministic |
| GAP-C6-17 | `StreamReaderWithConvert` | Type-safe stream element transformation requires manual loops |
| GAP-C6-18 | Custom gob codecs for `ToolInfo`/union types | Complex schema types fail gob encoding without custom codecs |
| GAP-C6-19 | OpenAI/Claude/Gemini concat extension functions | Provider metadata lost during stream merge |
| GAP-C6-20 | Schema `DataType` typed constants (String, Integer, etc.) | String-typed `ParameterInfo.Type` loses compile-time safety |

---

## 4. File-Level Implementation Opportunities

### 4.1 New File: `schema/message.go` (Priority: CRITICAL)

**What to implement:**
- `RoleType` enum: `Assistant`, `User`, `System`, `Tool`
- `Message` struct with all fields: Role, Content, ToolCalls, ToolCallID, ToolName, UserInputMultiContent, AssistantGenMultiContent, ResponseMeta, ReasoningContent, Extra
- `ToolCall` struct: Index (*int), ID, Type, Function{Name, Arguments}, Extra
- `MessageInputPart`: type discriminator + fields for Text, ImageURL, AudioURL, VideoURL, FileURL, ToolSearchResult
- `MessageOutputPart`: type discriminator + fields for Text, Image, Audio, Video, Reasoning
- `ResponseMeta`: FinishReason, Usage (*TokenUsage), LogProbs
- `TokenUsage`: PromptTokens, CompletionTokens, TotalTokens
- `ChatMessagePartType` constants
- Constructors: `SystemMessage`, `UserMessage`, `AssistantMessage`, `ToolMessage`
- `ConcatMessages(chunks []*Message) (*Message, error)`: content concat, reasoning concat, tool calls merge (by Index), multi-content merge, ResponseMeta propagation
- `concatToolCalls`: group by Index, validate ID/Type/Name consistency, concat Arguments JSON fragments
- `concatAssistantMultiContent` / `concatUserMultiContent`: type-specific merge logic
- `init()`: call `internal.RegisterStreamChunkConcatFunc(ConcatMessages)`

**Estimated lines:** ~400-500

### 4.2 New File: `schema/agentic_message.go` (Priority: CRITICAL)

**What to implement:**
- `AgenticRoleType`: `system`, `user`, `assistant`
- `AgenticMessage` struct: Role, ContentBlocks, ResponseMeta, Extra
- `ContentBlock` struct: ~20 variant fields as nullable pointers, Type discriminator
- `ContentBlockType` constants (~20 values)
- Input block structs: `UserInputText`, `UserInputImage`, `UserInputAudio`, `UserInputVideo`, `UserInputFile`, `ToolSearchResult`
- Output block structs: `AssistantGenText`, `AssistantGenImage`, `AssistantGenAudio`, `AssistantGenVideo`, `Reasoning`
- Tool call block structs: `FunctionToolCall`, `ServerToolCall`, `MCPToolCall`
- Tool result block structs: `FunctionToolResult`, `ServerToolResult`, `MCPToolResult`
- MCP block structs: `MCPListToolsResult`, `MCPToolApprovalRequest`, `MCPToolApprovalResponse`
- `StreamingMeta`: `{Index int}` for per-block stream grouping
- `AgenticResponseMeta`: TokenUsage + typed extension slots (OpenAI, Gemini, Claude, Extension any)
- `ConcatAgenticMessages(chunks []*AgenticMessage) (*AgenticMessage, error)`: ContentBlock grouping by Index, type-specific concat per block type
- Type-specific concat functions: `concatAssistantGenTexts`, `concatFunctionToolCalls`, etc.
- `concatAgenticResponseMeta`: merge token usage, delegate to per-provider extension concat functions
- `init()`: call `internal.RegisterStreamChunkConcatFunc(ConcatAgenticMessages)`

**Estimated lines:** ~600-800

### 4.3 New File: `schema/stream.go` (Priority: CRITICAL)

**What to implement:**
- `StreamReader[T]` generic struct with polymorphic backend selection
- Five internal backends:
  - `readerTypeStream` — channel-based (Pipe)
  - `readerTypeArray` — slice-based (StreamReaderFromArray)
  - `readerTypeMultiStream` — fan-in (MergeStreamReaders)
  - `readerTypeWithConvert` — element transformation (StreamReaderWithConvert)
  - `readerTypeChild` — fan-out (Copy)
- Public API:
  - `Recv() (T, error)` + `Close()`
  - `Pipe[T](cap int) (*StreamReader[T], *StreamWriter[T])`
  - `StreamReaderFromArray[T](arr []T) *StreamReader[T]`
  - `Copy(n int) []*StreamReader[T]` — linked-list shared buffer fan-out
  - `SetAutomaticClose()`
  - `MergeStreamReaders[T](srs []*StreamReader[T]) *StreamReader[T]`
  - `MergeNamedStreamReaders[T](srs, names) *StreamReader[map[string]T]`
  - `StreamReaderWithConvert[T,D](sr, func(T)(D,error)) *StreamReader[D]`
- `StreamWriter[T]`: `Send(chunk T, err error)`, `Close()`
- Error types: `ErrNoValue` (filter element in convert)

**Estimated lines:** ~500-700

### 4.4 New File: `schema/tool.go` (Priority: CRITICAL)

**What to implement:**
- `ToolInfo` struct: Name, Desc, ParamsOneOf, Extra
- `ParamsOneOf` dual-mode:
  - `NewParamsOneOfByParams(map[string]*ParameterInfo)` — lightweight mode
  - `NewParamsOneOfByJSONSchema(*jsonschema.Schema)` — full JSON Schema mode
- `ToJSONSchema() (*jsonschema.Schema, error)`: normalises both modes
- `ParameterInfo`: Type (DataType enum), Desc, Required, Enum, SubParams (recursive), ElemInfo (array elements)
- `DataType` constants: String, Integer, Boolean, Number, Object, Array
- `ToolResult`: Text, Images, Audio, Video, Files
- `ConcatToolResults([]*ToolResult) (*ToolResult, error)`
- Custom gob encode/decode for `ParamsOneOf` union type
- `init()`: call `internal.RegisterStreamChunkConcatFunc(ConcatToolResults)`

**Estimated lines:** ~300-400

### 4.5 New File: `schema/serialization.go` (Priority: HIGH)

**What to implement:**
- `RegisterName[T any](name string)`: calls `gob.RegisterName(name, zeroValue)` + `serialization.GenericRegister[T](name)`
- `GenericRegister[T]`: builds `reflect.Type → name` mapping in a package-level map
- `init()`: register all canonical types
  - `*Message`, `*AgenticMessage`
  - `ToolCall`, `ResponseMeta`, `TokenUsage`
  - `MessageInputPart`, `MessageOutputPart`
  - `ToolInfo`, `ToolResult`
  - etc. (~20 types)

**Estimated lines:** ~100-150

### 4.6 New File: `schema/types.go` (Priority: MEDIUM)

**What to implement:**
- `RoleType` constants if not in `message.go`
- `AgenticRoleType` constants
- `ContentBlockType` constants
- `ChatMessagePartType` constants
- `DataType` constants (if not in `tool.go`)
- Any shared sentinel errors

**Estimated lines:** ~60-80

### 4.7 New File: `internal/concat.go` (Priority: CRITICAL)

**What to implement:**
- `RegisterStreamChunkConcatFunc[T any](fn func([]T) (T, error))`: registers concat function keyed by `reflect.TypeOf(zeroValue)`
- `ConcatItems[T any](items []T) (T, error)`: looks up registered function for `T`, dispatches; if none registered, returns `ConcatNotSupportedError`
- `ConcatNotSupportedError` sentinel

**Estimated lines:** ~60-80

### 4.8 New Files: Provider Extension Packages (Priority: HIGH)

#### `schema/openai/extension.go`

- `ResponseMetaExtension`: ID, Status, PreviousResponseID, Error, IncompleteDetails, Reasoning, ServiceTier, CreatedAt, PromptCacheRetention
- `AssistantGenTextExtension`: Refusal, Annotations ([]*TextAnnotation)
- `TextAnnotation`: Index, type-specific location fields (FileCitation, URLCitation, FilePath, ContainerFileCitation)
- `ConcatResponseMetaExtensions([]*ResponseMetaExtension) *ResponseMetaExtension`
- `ConcatAssistantGenTextExtensions([]*AssistantGenTextExtension) *AssistantGenTextExtension`

**Estimated lines:** ~200-250

#### `schema/claude/extension.go`

- `ResponseMetaExtension`: ID, StopReason, StopSequence, StopDetails
- `AssistantGenTextExtension`: Citations ([]*TextCitation)
- `TextCitation`: CitationCharLocation, CitationPageLocation, CitationContentBlockLocation, CitationWebSearchResultLocation — each with CitedText, DocumentTitle, DocumentIndex
- `ConcatResponseMetaExtensions`, `ConcatAssistantGenTextExtensions`

**Estimated lines:** ~200-250

#### `schema/gemini/extension.go`

- `ResponseMetaExtension`: ID, FinishReason, GroundingMeta
- `GroundingMetadata`: GroundingChunks ([]*GroundingChunk with Web{Title, URI, Domain}), GroundingSupports, SearchEntryPoint, WebSearchQueries
- `ConcatResponseMetaExtensions`

**Estimated lines:** ~150-200

### 4.9 Modify Existing File: `compose/schema.go`

**Changes:**
- Deprecate existing `ToolCall`, `ToolInfo`, `ParamsOneOf`, `ParameterInfo`, `ToolResult` — these move to `schema/tool.go`
- Replace with type aliases or re-exports pointing to `schema/` package
- Or delete entirely and update all imports

**Estimated lines:** ~30 (rewrite)

### 4.10 Modify Existing File: `compose/chatmodel.go`

**Changes:**
- Update `Message` to include new fields (UserInputMultiContent, AssistantGenMultiContent, ResponseMeta, ReasoningContent) or re-export from `schema/`
- Update `ChatModel` interface to return `*schema.Message` if schema moves to separate package
- Add `ToolCall.Index` field

**Estimated lines:** ~20-40 (modification)

### 4.11 Modify Existing File: `compose/stream.go`

**Changes:**
- Deprecate `PipeStreamReader`/`PipeStreamWriter` in favor of `schema.StreamReader[T]`/`schema.StreamWriter[T]`
- Or keep as internal implementations and have `schema/stream.go` delegate to them

**Estimated lines:** ~20 (modification)

---

## 5. Total Implementation Size Estimate

| File | Estimated LOC | Priority |
|---|---|---|
| `schema/message.go` | 400–500 | CRITICAL |
| `schema/agentic_message.go` | 600–800 | CRITICAL |
| `schema/stream.go` | 500–700 | CRITICAL |
| `schema/tool.go` | 300–400 | CRITICAL |
| `schema/serialization.go` | 100–150 | HIGH |
| `schema/types.go` | 60–80 | MEDIUM |
| `internal/concat.go` | 60–80 | CRITICAL |
| `schema/openai/extension.go` | 200–250 | HIGH |
| `schema/claude/extension.go` | 200–250 | HIGH |
| `schema/gemini/extension.go` | 150–200 | HIGH |
| `compose/schema.go` (modify) | ~30 | — |
| `compose/chatmodel.go` (modify) | ~20–40 | — |
| `compose/stream.go` (modify) | ~20 | — |
| **Total** | **2,610–3,480** | — |

---

## 6. Exact Tests / Examples Needed

### 6.1 Schema Message Tests (`schema/message_test.go`)

| Test | Description | Priority |
|---|---|---|
| `TestMessageConstruction` | Construct a full Message with all fields, verify zero-value safety | HIGH |
| `TestSystemMessage` | `SystemMessage("you are helpful")` returns correct Role+Content | HIGH |
| `TestUserMessage` | `UserMessage("hello")` returns correct Role+Content | HIGH |
| `TestAssistantMessage` | `AssistantMessage("hello")` returns correct Role+Content | HIGH |
| `TestToolMessage` | `ToolMessage("result", "call_1")` returns correct Role+Content+ToolCallID | HIGH |
| `TestConcatMessages_TextOnly` | Two messages with Content="Hello" and " World" → Content="Hello World" | CRITICAL |
| `TestConcatMessages_Reasoning` | Two messages with ReasoningContent fragments → concatenated | CRITICAL |
| `TestConcatMessages_ToolCalls` | Messages with indexed ToolCall deltas → merged by index, name/id consistency validated, arguments JSON concatenated | CRITICAL |
| `TestConcatMessages_ToolCallIndexConflict` | Two deltas with same index but different ID → error | CRITICAL |
| `TestConcatMessages_MultiContent` | Messages with UserInputMultiContent parts → type-specific merge | HIGH |
| `TestConcatMessages_ResponseMeta` | Last non-nil ResponseMeta wins | HIGH |
| `TestConcatMessages_ToolCallOrdering` | Merged ToolCalls sorted by index ascending | MEDIUM |
| `TestConcatMessageArray` | Array of Messages (one per position) → correct merge | MEDIUM |
| `TestMessageRoundTrip_Empty` | Message with all zero values passes through concat | LOW |

### 6.2 Schema AgenticMessage Tests (`schema/agentic_message_test.go`)

| Test | Description | Priority |
|---|---|---|
| `TestAgenticMessageConstruction` | Build AgenticMessage with mixed ContentBlock types | HIGH |
| `TestContentBlock_InputTypes` | Each input ContentBlock type (Text, Image, Audio, Video, File, ToolSearchResult) constructs correctly | HIGH |
| `TestContentBlock_OutputTypes` | Each output ContentBlock type (Text, Image, Audio, Video, Reasoning) constructs correctly | HIGH |
| `TestContentBlock_ToolCallTypes` | FunctionToolCall, ServerToolCall, MCPToolCall construct correctly | HIGH |
| `TestContentBlock_ToolResultTypes` | FunctionToolResult, ServerToolResult, MCPToolResult construct correctly | HIGH |
| `TestConcatAgenticMessages_TextBlocks` | AssistantGenText blocks with StreamingMeta.Index → merged by index | CRITICAL |
| `TestConcatAgenticMessages_ToolCalls` | FunctionToolCall blocks with index → merged, arguments concatenated | CRITICAL |
| `TestConcatAgenticMessages_MixedBlocks` | Mixed block types in same stream → each type merged independently | CRITICAL |
| `TestConcatAgenticMessages_ResponseMeta` | AgenticResponseMeta with token usage and provider extensions → merged correctly | HIGH |
| `TestConcatAgenticMessages_OpenAIExtension` | OpenAI extension slots on ResponseMeta + AssistantGenText → merged via openai.ConcatXxxExtensions | HIGH |
| `TestConcatAgenticMessages_ClaudeExtension` | Claude extension slots on ResponseMeta + AssistantGenText → correct merge | HIGH |
| `TestConcatAgenticMessages_GeminiExtension` | Gemini GroundingMetadata → correct merge | HIGH |
| `TestConcatAgenticMessages_ExtensionFallback` | Unknown extension in `Extension any` field → ConcatSliceValue append | MEDIUM |
| `TestAgenticMessage_NoToolRole` | Verify tool results are User role ContentBlocks, not separate Tool role Messages | HIGH |

### 6.3 Schema Tool Tests (`schema/tool_test.go`)

| Test | Description | Priority |
|---|---|---|
| `TestToolInfo_ParamsMode` | Construct ToolInfo with `NewParamsOneOfByParams` | HIGH |
| `TestToolInfo_JSONSchemaMode` | Construct ToolInfo with `NewParamsOneOfByJSONSchema` | HIGH |
| `TestParamsOneOf_ToJSONSchema_Params` | Convert ParamsOneOf with params → *jsonschema.Schema | CRITICAL |
| `TestParamsOneOf_ToJSONSchema_JSONSchema` | Convert ParamsOneOf with json schema → same *jsonschema.Schema | CRITICAL |
| `TestParamsOneOf_ToJSONSchema_Empty` | Convert empty ParamsOneOf → valid empty schema | MEDIUM |
| `TestParameterInfo_Nested` | ParameterInfo with SubParams (recursive objects) | HIGH |
| `TestParameterInfo_ArrayElem` | ParameterInfo with ElemInfo (array element type) | HIGH |
| `TestParameterInfo_Enum` | ParameterInfo with Enum values | MEDIUM |
| `TestToolResult_TextOnly` | ToolResult with just Text | HIGH |
| `TestToolResult_MultiModal` | ToolResult with Images/Audio/Video/Files | HIGH |
| `TestConcatToolResults` | Multiple ToolResults merged correctly | HIGH |
| `TestToolInfo_GobEncodeDecode` | ToolInfo round-trip through gob (ParamsOneOf union) | CRITICAL |
| `TestParamsOneOf_MutualExclusion` | Cannot have both params and jsonSchema set (or existing behavior documented) | MEDIUM |

### 6.4 Schema Stream Tests (`schema/stream_test.go`)

| Test | Description | Priority |
|---|---|---|
| `TestPipe` | Pipe creates paired reader/writer, Send→Recv works | CRITICAL |
| `TestPipe_Close` | Close writer → Recv returns io.EOF | CRITICAL |
| `TestPipe_Error` | Send error → Recv returns error | HIGH |
| `TestStreamReaderFromArray` | Array-backed reader, zero goroutines | CRITICAL |
| `TestStreamReaderFromArray_Empty` | Empty array → immediate io.EOF | HIGH |
| `TestCopy_FanOut` | Copy(3) → 3 independent readers, each sees all elements | CRITICAL |
| `TestCopy_CloseLeak` | Unclosed child reader → leak detection (goroutine count) | CRITICAL |
| `TestCopy_BeforeFirstRecv` | Copy called after first Recv → error or documented behavior | CRITICAL |
| `TestMergeStreamReaders` | Multiple readers → merged stream, all elements received | CRITICAL |
| `TestMergeStreamReaders_Ordering` | Merge preserves per-source order but interleaves sources | HIGH |
| `TestMergeStreamReaders_EmptySources` | Merge with empty array readers → no panic, correct EOF | MEDIUM |
| `TestMergeNamedStreamReaders` | Named merge with SourceEOF per source | HIGH |
| `TestMergeNamedStreamReaders_SourceTracking` | Consumer can identify which source produced each EOF | MEDIUM |
| `TestStreamReaderWithConvert` | Type-safe element transformation | CRITICAL |
| `TestStreamReaderWithConvert_ErrNoValue` | Convert returns ErrNoValue → element filtered out | HIGH |
| `TestStreamReaderWithConvert_Error` | Convert returns error → error propagated | HIGH |
| `TestSetAutomaticClose` | SetAutomaticClose → reader auto-closes on GC (test with explicit GC) | MEDIUM |
| `TestBackendSelection` | Verify correct backend selected: Pipe→channel, FromArray→array, etc. | MEDIUM |

### 6.5 Serialisation Tests (`schema/serialization_test.go`)

| Test | Description | Priority |
|---|---|---|
| `TestRegisterName_RoundTrip` | Register a type, encode, decode → same value | CRITICAL |
| `TestRegisterName_Message` | Full Message gob round-trip with ToolCalls, ResponseMeta | CRITICAL |
| `TestRegisterName_AgenticMessage` | Full AgenticMessage gob round-trip with ContentBlocks | CRITICAL |
| `TestRegisterName_ToolInfo` | ToolInfo with ParamsOneOf (both modes) gob round-trip | CRITICAL |
| `TestRegisterName_DuplicateName` | Registering same name twice → panic or error | MEDIUM |
| `TestUnregisteredType_Error` | Encoding unregistered type → gob error | MEDIUM |

### 6.6 Internal Concat Tests (`internal/concat_test.go`)

| Test | Description | Priority |
|---|---|---|
| `TestConcatItems_Registered` | Register func for type T, call ConcatItems[T] → dispatches correctly | CRITICAL |
| `TestConcatItems_Unregistered` | Call ConcatItems for unregistered type → ConcatNotSupportedError | CRITICAL |
| `TestConcatItems_SingleElement` | Concat with 1 element → returns same element | MEDIUM |
| `TestConcatItems_EmptySlice` | Concat with empty slice → returns zero value or error | MEDIUM |
| `TestConcatItems_MultipleRegistrations` | Register for multiple types, verify correct dispatch for each | HIGH |

### 6.7 Provider Extension Tests

#### `schema/openai/extension_test.go`

| Test | Description | Priority |
|---|---|---|
| `TestOpenAI_ResponseMetaConcat` | Multiple ResponseMetaExtensions merged correctly | HIGH |
| `TestOpenAI_TextAnnotationDedup` | AssistantGenTextExtensions merged, annotations deduped by Index | HIGH |
| `TestOpenAI_TextAnnotation_FourLocationTypes` | Each annotation location type (FileCitation, URLCitation, FilePath, ContainerFileCitation) round-trips | MEDIUM |
| `TestOpenAI_RefusalConcat` | Refusal text concatenated across extensions | MEDIUM |

#### `schema/claude/extension_test.go`

| Test | Description | Priority |
|---|---|---|
| `TestClaude_ResponseMetaConcat` | Multiple ResponseMetaExtensions merged correctly | HIGH |
| `TestClaude_CitationsConcat` | AssistantGenTextExtensions merged, citations simply appended | HIGH |
| `TestClaude_TextCitation_FourLocationTypes` | Each citation location type round-trips | MEDIUM |

#### `schema/gemini/extension_test.go`

| Test | Description | Priority |
|---|---|---|
| `TestGemini_ResponseMetaConcat` | Multiple ResponseMetaExtensions merged correctly | HIGH |
| `TestGemini_GroundingMetadataConcat` | GroundingMetadata with chunks + supports + search entry → merged | HIGH |
| `TestGemini_GroundingChunk` | GroundingChunk with Web{Title, URI, Domain} round-trips | MEDIUM |

### 6.8 Integration / Composition Tests

| Test | Description | Priority |
|---|---|---|
| `TestEndToEnd_StreamConcat` | Pipe → Send chunks → ConcatMessages → complete Message | CRITICAL |
| `TestEndToEnd_AgenticStreamConcat` | Pipe → Send AgenticMessage chunks → ConcatAgenticMessages → complete AgenticMessage | CRITICAL |
| `TestEndToEnd_ToolCallStream` | Simulated streaming tool call: multiple ToolCall chunks with indices → concat → correct merged ToolCalls | CRITICAL |
| `TestEndToEnd_MultiProviderExtensions` | AgenticMessage with both OpenAI and Claude extensions → concat preserves both | MEDIUM |
| `TestEndToEnd_SerializationWithCheckpoint` | Message → gob encode → gob decode → same Message | HIGH |

---

## 7. Implementation Phasing Recommendation

### Phase 1: Foundation (Schema types + Concat) — Must be first

1. `internal/concat.go` — Registration-based concat dispatch (no dependencies)
2. `schema/types.go` — Shared constants and enums
3. `schema/message.go` — Message + ToolCall + ConcatMessages + register concat
4. `schema/stream.go` — StreamReader[T] polymorphic streaming
5. `schema/tool.go` — ToolInfo + ParamsOneOf + ToolResult + ConcatToolResults

**Verification gate:** All Message/Tool/Stream unit tests pass.

### Phase 2: Agentic Model + Serialisation

6. `schema/agentic_message.go` — AgenticMessage + ContentBlock + ConcatAgenticMessages + register concat
7. `schema/serialization.go` — RegisterName + init() registrations

**Verification gate:** All AgenticMessage and serialisation tests pass.

### Phase 3: Provider Extensions

8. `schema/openai/extension.go`
9. `schema/claude/extension.go`
10. `schema/gemini/extension.go`

**Verification gate:** All provider extension concat tests pass.

### Phase 4: Replica Integration

11. Update `compose/schema.go` — Deprecate old types, re-export from `schema/`
12. Update `compose/chatmodel.go` — Use `schema.Message` (re-export or migrate)
13. Update `compose/stream.go` — Use `schema.StreamReader[T]`
14. Update all existing tests to use new schema types

**Verification gate:** All existing replica tests still pass. Full integration test suite passes.

---

## 8. Risk Register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `compose/` import cycle with `schema/` | Medium | High (build failure) | `schema/` must be a leaf package with zero `compose/` imports. Use `internal/` for shared machinery. |
| Breaking existing replica tests | High | Medium (test failures) | Phase 4 migration should keep old types as deprecated aliases during transition. |
| Stream goroutine leaks in Copy/Merge | High | High | Dedicated leak detector tests. Each Copy/Merge test must verify goroutine count. |
| `ParamsOneOf` dual-mode confusion (both branches set) | Low | Medium | Document mutual exclusion clearly. `ToJSONSchema()` checks `params != nil` first. |
| Provider extension types growing unbounded | Medium | Low | Scope to OpenAI/Claude/Gemini only. Extensions for other providers are out of scope. |
| `RegisterName` name collisions with existing gob registrations | Low | Medium | Use `_eino_` prefix convention. Add collision detection in `RegisterName`. |
| Performance regression from polymorphic StreamReader backends | Low | Medium | Benchmark `StreamReaderFromArray` vs `Pipe` for small arrays. Measure overhead of backend selection. |

---

## 9. Relationship to Other Chapters

| Chapter | Integration point with Ch6 schema |
|---|---|
| Ch1 (Graph/DAG/Pregel) | Schema types flow through `chanCall.writeTo`, `task.input/output`. Pregel channel `Values map` holds `*Message` or `*AgenticMessage`. |
| Ch2 (FieldMapping/Workflow/Chain) | `FieldMapping.convertTo` must handle `*Message` ↔ `*AgenticMessage` conversions. Workflow nodes pass schema types between nodes. |
| Ch3 (Runnable/Stream/Callback) | `composableRunnable` wraps `StreamReader[*Message]`. Callback handlers receive schema-typed `CallbackInput`/`CallbackOutput`. |
| Ch4 (Checkpoint/Interrupt/Resume) | Checkpoint persists `*Message` state → depends on schema gob registration. Resume restores `StreamReader` → depends on StreamReader backend. |
| Ch5 (ChatModel/Tool/Prompt) | `BaseChatModel.Generate()` returns `*Message`. `ToolsNode` reads `ToolCalls`, produces `*Message`. `ChatTemplate.Format()` returns `[]*Message`. Tool schemas use `ToolInfo.ParamsOneOf`. |
| Ch6 (Schema/Provider Adapters) | **This chapter.** The canonical types defined here underpin all other chapters. |

---

## 10. Key Eino Source Locations for Implementation Reference

| Concept | Eino Source | Lines |
|---|---|---|
| `Message` struct | `schema/message.go` | 497– |
| `ToolCall` struct | `schema/message.go` | 132– |
| `MessageInputPart` | `schema/message.go` | 207– |
| `MessageOutputPart` | `schema/message.go` | 268– |
| `ConcatMessages` | `schema/message.go` | 1643– |
| `AgenticMessage` struct | `schema/agentic_message.go` | 71– |
| `ContentBlock` | `schema/agentic_message.go` | 102– |
| `ConcatAgenticMessages` | `schema/agentic_message.go` | 897– |
| `StreamReader[T]` | `schema/stream.go` | 168– |
| `Pipe[T]` | `schema/stream.go` | 99– |
| `Copy(n)` | `schema/stream.go` | 261– |
| `MergeStreamReaders[T]` | `schema/stream.go` | 912– |
| `StreamReaderWithConvert[T,D]` | `schema/stream.go` | 691– |
| `StreamReaderFromArray[T]` | `schema/stream.go` | 461– |
| `ToolInfo` struct | `schema/tool.go` | 128– |
| `ParamsOneOf.NewParamsOneOfByParams` | `schema/tool.go` | 283– |
| `ParamsOneOf.NewParamsOneOfByJSONSchema` | `schema/tool.go` | 290– |
| `ParamsOneOf.ToJSONSchema()` | `schema/tool.go` | 297– |
| `RegisterName[T]` | `schema/serialization.go` | 83– |
| `init()` registrations | `schema/serialization.go` | 27–56 |
| `RegisterStreamChunkConcatFunc[T]` | `internal/concat.go` | 71– |
| `ConcatItems[T]` | `internal/concat.go` | – |
| OpenAI `ResponseMetaExtension` | `schema/openai/extension.go` | – |
| Claude `TextCitation` | `schema/claude/extension.go` | 39– |
| Gemini `GroundingMetadata` | `schema/gemini/extension.go` | – |
| `init()` concat registrations | `schema/message.go` | 39–46 |

---

*Audit completed: 2026-06-06*
*Audit scope: `06-schema-provider-adapters.md` (506 lines) + `compose/*.go` (44 files) + research docs*
*Key finding: 20 gaps identified (9 critical, 6 high, 5 medium). Full AgenticMessage system, StreamReader polymorphic backends, concat dispatch, gob serialisation, and provider extension types are fully missing. Current schema types (ToolCall, ToolInfo, ParamsOneOf, ParameterInfo) exist as minimal stubs missing critical fields (Index, SubParams, ElemInfo, JSON Schema branch, ToJSONSchema). Implementation requires ~2,600–3,500 lines across 8 new files + 3 file modifications.*
