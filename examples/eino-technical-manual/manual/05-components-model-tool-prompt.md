# Chapter 5: Components — Model / Tool / Prompt Contracts

## 1. Problem

Eino is a framework for composing LLM applications as graphs. The graph runtime
(`compose` package) needs to invoke models, execute tool calls, format prompts,
embed text, index documents, and retrieve relevant chunks — all without knowing
which provider or backend sits behind each operation. If the graph has to
understand OpenAI vs Anthropic API differences, or the distinction between a
FAISS indexer and a Redis retriever, it cannot be generic.

The components layer solves this by defining **one minimal interface per
capability**. A graph node configured with a `BaseChatModel` can call `Generate`
and `Stream` regardless of whether the implementation is `openai.ChatModel`,
`anthropic.ChatModel`, or a local Ollama wrapper. The interface IS the contract.

## 2. Why It Is Hard

Getting the abstraction _depth_ right is the hard part:

**Interface granularity.** If every sub-capability gets its own interface,
implementors drown in boilerplate. If the interface is too coarse (one giant
`Component` interface with 30 methods), most backends can only implement a
subset, and the type system can't express which subset — you push validation to
runtime. Eino splits the difference: a `BaseModel` has exactly two methods
(`Generate` + `Stream`), but additional capabilities like tool binding are
expressed through _interface composition_ (`ToolCallingChatModel`).

**Provider-specific options must not leak.** An OpenAI model needs
`OpenAI-specific-user`, an Anthropic model needs `Anthropic-specific-cache`,
and a Redis retriever needs `Redis-specific-index-name`. If the common interface
accepts `map[string]any` for options, callers lose type safety. If the interface
accepts only common options, providers lose expressiveness. Eino solves this
with a dual bucket in the `Option` struct: common options apply through
`GetCommonOptions`, impl-specific options through `GetImplSpecificOptions`.

**Concurrency in tool binding.** Many LLM frameworks allow mutating a model
instance with `BindTools(tools)`. In a concurrent server, goroutine A binds
search tools and goroutine B binds calculator tools on the same shared model
instance — instant race. Eino deprecates `BindTools` in favour of
`WithTools` which returns a _new_ instance, making the contract
concurrency-safe by design.

**Tool result fidelity.** Some tools return plain text (`"42"`), others return
images, audio, or video (multimodal results). If the component interface only
supports `string` tool output, multimodal tools have to serialize rich media
into a lossy string representation. Eino addresses this with an "enhanced"
tool tier that carries `schema.ToolResult` — a structured container for
text, images, audio, video, and file content.

## 3. Design Idea

Eino's component contracts follow five patterns:

### 3.1 Interface Minimalism

Every component category exposes one or two methods. There is no "init", no
"close", no lifecycle — the interface is a **function invocation** with
options:

| Component | Interface | Methods |
|-----------|-----------|---------|
| ChatModel | `BaseChatModel = BaseModel[*schema.Message]` | `Generate`, `Stream` |
| Tool | `BaseTool` → `InvokableTool` / `StreamableTool` | `Info`, `InvokableRun` / `StreamableRun` |
| Prompt | `ChatTemplate` | `Format` |
| Embedder | `Embedder` | `EmbedStrings` |
| Indexer | `Indexer` | `Store` |
| Retriever | `Retriever` | `Retrieve` |

The `BaseModel[M]` generic (defined in `components/model/interface.go:36`) is
the core model contract. It's parameterized by message type `M`, sealed to only
allow `*schema.Message` and `*schema.AgenticMessage` through a type constraint
`messageType` (line 27):

```go
type messageType interface {
    *schema.Message | *schema.AgenticMessage
}

type BaseModel[M messageType] interface {
    Generate(ctx context.Context, input []M, opts ...Option) (M, error)
    Stream(ctx context.Context, input []M, opts ...Option) (*schema.StreamReader[M], error)
}
```

This gives us two concrete type aliases:
- `BaseChatModel = BaseModel[*schema.Message]` (standard chat)
- `AgenticModel = BaseModel[*schema.AgenticMessage]` (agentic chat)

### 3.2 Dual-Bucket Options

The `Option` struct in `components/model/option.go:64` carries TWO setters, never
both populated by the same `Option`:

```go
type Option struct {
    apply            func(opts *Options)   // common options setter
    implSpecificOptFn any                  // provider-specific setter
}
```

Common options like `WithTemperature`, `WithModel`, `WithTools` populate `apply`.
Provider-specific options like `openai.WithUser` use `WrapImplSpecificOptFn`
(`option.go:196`) to populate `implSpecificOptFn`. The implementor calls both:

```go
common := model.GetCommonOptions(nil, opts...)
myOpts := model.GetImplSpecificOptions(&MyOpts{}, opts...)
```

The tool package mirrors this pattern in `components/tool/option.go:22` with
its own `Option` struct and `WrapImplSpecificOptFn`.

### 3.3 Immutable Tool Binding

The old `ChatModel` interface (`components/model/interface.go:80`) exposed
`BindTools` which mutated the receiver:

```go
// Deprecated: BindTools mutates the instance — not concurrency-safe.
type ChatModel interface {
    BaseChatModel
    BindTools(tools []*schema.ToolInfo) error
}
```

The replacement `ToolCallingChatModel` (`interface.go:99`) returns a new
instance, safe for concurrent use:

```go
type ToolCallingChatModel interface {
    BaseChatModel
    WithTools(tools []*schema.ToolInfo) (ToolCallingChatModel, error)
}
```

Usage pattern:
```go
base, _ := openai.NewChatModel(ctx, cfg)         // shared, no tools
withSearch, _ := base.WithTools([]*schema.ToolInfo{searchTool})
// base remains tool-free; withSearch is a new instance with search tool bound
```

### 3.4 Layered Tool Interface

Tools in Eino are defined through a stacking interface hierarchy
(`components/tool/interface.go`):

```
BaseTool (Info)                                    ← metadata only
  ├── InvokableTool (BaseTool + InvokableRun)      ← string in, string out
  ├── StreamableTool (BaseTool + StreamableRun)    ← string in, stream out
  ├── EnhancedInvokableTool (BaseTool + InvokableRun with ToolResult)  ← structured in/out
  └── EnhancedStreamableTool (BaseTool + StreamableRun with ToolResult)
```

`BaseTool` alone is sufficient for passing tool schemas to a ChatModel via
`WithTools` — the model only needs the tool's JSON schema to generate tool
calls. But for `ToolsNode` to _execute_ a tool, the implementation must also
satisfy at least one of `InvokableTool` or `StreamableTool` (or their enhanced
variants).

When a tool implements both standard and enhanced interfaces, `ToolsNode`
prioritizes the enhanced interface (`compose/tool_node.go:830-838`).

### 3.5 Callback Extras per Component Kind

Each component package defines a `CallbackInput` and `CallbackOutput` struct
and `ConvCallbackInput`/`ConvCallbackOutput` helpers so observers can inspect
typed inputs and outputs. Examples:

- `components/model/callback_extra.go:66-80` — `CallbackInput{Messages, Config}` and
  `CallbackOutput{Message, Config, TokenUsage}`
- `components/tool/callback_extra.go:25-33` — `CallbackInput{ArgumentsInJSON, Config}` and
  `CallbackOutput{Response}`
- `components/prompt/callback_extra.go:25-35` — `CallbackInput{Variables, Templates}` and
  `CallbackOutput{Result}`
- `components/retriever/callback_extra.go:25-41` — `CallbackInput{Query, Options}` and
  `CallbackOutput{Docs}`

The `ConvCallbackInput`/`ConvCallbackOutput` functions perform a safe type
switch: if the raw callback value doesn't match the expected type, they return
`nil`. This lets a global callback handler gracefully ignore components it
doesn't care about.

## 4. Source Walkthrough

### 4.1 Component Identity: `components/types.go`

Two optional interfaces (`types.go:29-52`) let the runtime inspect a component:

- `Typer` → `GetType() string`: returns an implementation name like
  `"OpenAIChatModel"`. Tools use this to set their display name; the graph
  runtime uses it for debug output (format `"{GetType()}{ComponentKind}"`).

- `Checker` → `IsCallbacksEnabled() bool`: when the implementation returns
  `true`, the framework skips its default `OnStart`/`OnEnd` wrapping and trusts
  the component to fire callbacks itself. This is essential for streaming
  models that need to fire callbacks mid-stream, not just at completion.

`Component` constants (`types.go:64-87`) identify the category:
`ComponentOfChatModel`, `ComponentOfTool`, `ComponentOfPrompt`, etc. These
flow into `callbacks.RunInfo.Component` so observers can branch on kind.

### 4.2 ChatModel and ToolCallingChatModel: `components/model/interface.go`

The full hierarchy (all in `interface.go`):

- `BaseChatModel = BaseModel[*schema.Message]` — core two-method contract
  (`Generate` + `Stream`), lines 36-71.
- `ChatModel` (deprecated, line 80) — adds mutating `BindTools`.
- `ToolCallingChatModel` (line 99) — adds immutable `WithTools`.
- `AgenticModel = BaseModel[*schema.AgenticMessage]` (line 109) — agentic
  variant; tools passed via `model.WithTools` option instead of interface method.

The key design decision: `AgenticModel` does NOT have a `WithTools` method.
For agentic models, tools are passed at request-time via the `model.WithTools`
option (defined in `model/option.go:116`). This is a deliberate asymmetry —
agentic models treat tools as a per-request concern, while chat models treat
them as (immutable) configuration.

### 4.3 Model Options: `components/model/option.go`

`Options` struct (line 22) carries `Temperature`, `Model`, `TopP`, `MaxTokens`,
`Stop`, `Tools`, `DeferredTools`, `ToolSearchTool`, `ToolChoice`,
`AllowedToolNames`, and `AgenticToolChoice`.

The `WithTools` option function (line 116) normalizes `nil` to an empty slice
to avoid nil pointer issues downstream.

`ToolSearchTool` and `DeferredTools` (lines 127-152) support server-side
tool search: tools are registered with `defer_loading=true`, and a special
"tool search tool" discovers and loads them on demand. This is the pattern
behind server-side tool calling where the model API handles tool search
internally, not the Eino framework.

### 4.4 Tool Interface: `components/tool/interface.go`

`BaseTool` (line 32) returns `*schema.ToolInfo` — name, description,
parameter JSON schema. That's the only method.

`InvokableTool` (line 42) adds `InvokableRun(ctx, argumentsInJSON string, opts ...Option) (string, error)`.
The arguments arrive as a JSON string — the framework does NOT parse them. The
caller (`ToolsNode`) passes the raw JSON from the model's tool call.

`EnhancedInvokableTool` (line 67) uses `*schema.ToolArgument` instead of a
raw string, and returns `*schema.ToolResult` instead of a string.
`schema.ToolResult` carries multimodal content (text, images, audio, video,
files). The interface priority rule in `compose/tool_node.go:830`: if enhanced
endpoints exist, `ToolsNode` uses them; otherwise it falls back to standard
endpoints.

### 4.5 ToolsNode Execution: `compose/tool_node.go`

`ToolsNode` (line 79) is the graph node that executes tool calls. Its
signature:

```go
Invoke(ctx, *schema.Message, ...ToolsNodeOption) ([]*schema.Message, error)
Stream(ctx, *schema.Message, ...ToolsNodeOption) (*schema.StreamReader[[]*schema.Message], error)
```

Input: **one** Assistant `Message` containing `ToolCalls`. Output: **one**
Tool `Message` per tool call.

Key execution details:

**Task generation (`genToolCallTasks`, line 777).** Iterates over
`input.ToolCalls`, looks up each tool name in `tuple.indexes`, and builds a
`toolCallTask` with the appropriate endpoint (enhanced vs standard). Unknown
tool names are dispatched to `UnknownToolsHandler` if configured; otherwise
they error.

**Argument alias remapping (`remapArgs`, line 334).** When `ToolAliasConfig`
is configured, the tool's call arguments JSON is deserialized, keys are
remapped from aliases to canonical names, and the JSON is re-serialized
before execution.

**Sequential vs parallel execution.** By default (`executeSequentially = false`),
tool calls run concurrently via `parallelRunToolCall` (line 985). The first
tool runs on the calling goroutine, the rest in `go` routines joined by a
`sync.WaitGroup`. Panic recovery wraps each goroutine. When
`executeSequentially = true`, calls run in order via `sequentialRunToolCall`
(line 973).

**Interrupt-and-rerun.** `ToolsNode` supports checkpointing: if a tool returns
an `InterruptRerunError`, the node saves executed results in
`ToolsInterruptAndRerunExtra` (line 287) and returns a composite interrupt.
On rerun, previously executed tools are skipped (their results are reused).

**Enhanced tool output conversion.** For enhanced tools, the `ToolResult` is
converted to `Message` via `ToolResult.ToMessageInputParts()` (line 1129),
which populates `UserInputMultiContent` with multimodal parts.

### 4.6 Prompt Template: `components/prompt/interface.go`

`ChatTemplate` (line 43) has one method:

```go
Format(ctx context.Context, vs map[string]any, opts ...Option) ([]*schema.Message, error)
```

Variable substitution syntax (FString, GoTemplate, Jinja2) is chosen at
construction time. Missing variables produce a runtime error — there is no
compile-time safety for prompt templates.

`AgenticChatTemplate` (line 48) returns `[]*schema.AgenticMessage` for
agentic model consumption.

### 4.7 RAG Components: Embedding, Indexer, Retriever

**Embedder** (`components/embedding/interface.go:37`):
```go
EmbedStrings(ctx context.Context, texts []string, opts ...Option) ([][]float64, error)
```
Returns one vector per input text, same order. Dimension is fixed by the
underlying model.

**Indexer** (`components/indexer/interface.go:38`):
```go
Store(ctx context.Context, docs []*schema.Document, opts ...Option) ([]string, error)
```
Stores documents (optionally generating vectors if `Options.Embedding` is set)
and returns backend-assigned IDs.

**Retriever** (`components/retriever/interface.go:48`):
```go
Retrieve(ctx context.Context, query string, opts ...Option) ([]*schema.Document, error)
```
Returns matching documents ordered by relevance. `ScoreThreshold` filters
low-scoring docs; `TopK` caps result count.

The critical contract: Indexer and Retriever must use the **same embedder
model**. Mismatched dimensions or model families break semantic similarity.
Both `indexer.Options.Embedding` and `retriever.Options.Embedding` fields
carry the embedder reference.

### 4.8 Component-to-Graph-Node Bridge: `compose/component_to_graph_node.go`

Each component kind has a `to*Node` adapter (lines 49-167):

| Function | Input | Graph Node |
|----------|-------|------------|
| `toChatModelNode` | `BaseChatModel` | Invoke=Generate, Stream=Stream |
| `toToolsNode` | `*ToolsNode` | Invoke/Stream from `ToolsNode` methods |
| `toChatTemplateNode` | `ChatTemplate` | Invoke=Format |
| `toRetrieverNode` | `Retriever` | Invoke=Retrieve |
| `toIndexerNode` | `Indexer` | Invoke=Store |
| `toEmbeddingNode` | `Embedder` | Invoke=EmbedStrings |

The core `toComponentNode` function (line 29) wraps each component with
`parseExecutorInfoFromComponent` to extract `Typer` and `Checker` metadata,
then builds a `composableRunnable` that handles callback injection.

## 5. Patterns and Examples

### 5.1 Minimal ChatModel Implementation

```go
type MyModel struct {
    defaultTemp float32
}

func (m *MyModel) Generate(ctx context.Context, input []*schema.Message, opts ...model.Option) (*schema.Message, error) {
    common := model.GetCommonOptions(&model.Options{Temperature: &m.defaultTemp}, opts...)
    myOpts := model.GetImplSpecificOptions(&MyOptions{}, opts...)
    // use common.Temperature, common.Tools, myOpts.MyParam, etc.
    return &schema.Message{Role: schema.Assistant, Content: "...response..."}, nil
}

func (m *MyModel) Stream(ctx context.Context, input []*schema.Message, opts ...model.Option) (*schema.StreamReader[*schema.Message], error) {
    // ...
}

func (m *MyModel) GetType() string { return "MyChatModel" } // optional, for Typer
```

### 5.2 ToolCallingChatModel Implementation

```go
type MyModel struct {
    baseConfig *Config
    tools      []*schema.ToolInfo
}

func (m *MyModel) WithTools(tools []*schema.ToolInfo) (model.ToolCallingChatModel, error) {
    newM := *m          // shallow copy
    newM.tools = tools   // no mutation of original
    return &newM, nil
}
```

### 5.3 Minimal InvokableTool

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

### 5.4 Standalone Retriever Usage

```go
retriever, _ := redis.NewRetriever(ctx, cfg)
docs, err := retriever.Retrieve(ctx, "what is eino?",
    retriever.WithTopK(5),
    retriever.WithScoreThreshold(0.7),
)
```

### 5.5 Graph Integration Pattern

```go
graph := compose.NewGraph[string, *schema.Message]()
graph.AddChatModelNode("llm", baseModel)       // component → node
graph.AddToolsNode("tools", toolsNode)           // component → node
graph.AddRetrieverNode("retriever", retriever)   // component → node
// compose knows how to invoke each because of their interfaces
```

### 5.6 Callback Observation

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

## 6. Common Pitfalls

### 6.1 BindTools Race on Shared Model Instances

Using the deprecated `ChatModel.BindTools` on a model shared across goroutines
causes a data race: one request's `BindTools` can overwrite another's tool list
before `Generate` executes. **Always use `ToolCallingChatModel.WithTools`** or
pass tools via `model.WithTools()` option for `AgenticModel`.

### 6.2 Nil vs Empty Slice for Options

`model.WithTools(nil)` normalizes to an empty slice (`option.go:117-118`),
but not all option functions do this. Passing `nil` to a function that
dereferences the pointer without normalization causes a panic. Always check
for nil when writing option getters.

### 6.3 Tool Implements BaseTool Only

Defining a tool struct that satisfies `BaseTool` but not `InvokableTool` or
`StreamableTool` compiles fine, but `ToolsNode` will fail at construction with
`"tool X is not invokable, streamable, enhanced invokable or enhanced streamable"`
(`tool_node.go:541`). This is a runtime error, not a compile-time error, because
`ToolsNodeConfig.Tools` accepts `[]tool.BaseTool`.

### 6.4 StreamReader Ownership

When `BaseModel.Stream` returns a `*schema.StreamReader`, the **caller** is
responsible for `Close()`. If the caller does not close, the underlying
goroutine that feeds the stream leaks. Similarly, `ToolsNode.Stream` returns a
merged stream reader — the consumer must close it.

### 6.5 Mismatched Embedder Between Indexer and Retriever

Passing `indexer.WithEmbedding(adaEmbedder)` at store time but
`retriever.WithEmbedding(bgeEmbedder)` at retrieve time silently produces
meaningless results because vectors from different models live in different
semantic spaces. Always pair the same embedder instance.

### 6.6 Callback Handler Not Closing Stream Copy

When multiple handlers register for stream timings, the stream is copied N+1
times. If any handler's copy is not closed, the original stream cannot be freed,
leaking both goroutines and memory for the entire pipeline.

### 6.7 Passing Tools Twice

For `AgenticModel`, tools are passed via the `model.WithTools()` option at
request time. If you also mistakenly call `WithTools` (which doesn't exist on
`AgenticModel`) or attempt to replicate a `ToolCallingChatModel` pattern,
you may end up with tools attached to the _option_ but not to the _model_ —
the model ignores them.

## 7. What Rive Can Learn

### 7.1 Interface Sealing Through Type Constraints

Eino uses a Go type constraint `messageType` to seal `BaseModel[M]` to exactly
two concrete types (`*schema.Message` and `*schema.AgenticMessage`). This is
cleaner than a `type-assert-everywhere` approach. Rive could use a similar
pattern for its own generic interfaces, preventing arbitrary type parameters
from leaking through the abstraction boundary.

### 7.2 Dual-Bucket Options for Extensibility

The `Option{apply, implSpecificOptFn}` pattern is a practical compromise
between a sealed common API and provider extensibility. It lets each provider
ship its own `WithFoo` functions that compose seamlessly with common
`WithTemperature` etc. Rive's plugin system could adopt this pattern to let
plugins define custom options without polluting the core option type.

### 7.3 Deprecation with Clear Migration Path

The `ChatModel.BindTools` → `ToolCallingChatModel.WithTools` migration is
well-documented via doc comments (mentioning the race condition), type
deprecation annotation, and a clear code example in the interface comment. Rive
should adopt the same practice: when deprecating an API, explain _why_ the old
API is unsafe and show the new canonical pattern.

### 7.4 Layered Interface Hierarchy for Progressive Capability

The `BaseTool → InvokableTool → EnhancedInvokableTool` hierarchy lets simple
tools implement just what they need, while `ToolsNode` discovers capabilities
at construction time via type assertions. Rive can use this for its own
extensible interfaces — define a base, and let the runtime discover extended
capabilities without requiring every implementation to satisfy them all.

### 7.5 Runtime Interface Discovery for Tool Execution

`ToolsNode` (`convTools`, line 489) uses type assertions to discover which
interfaces a tool satisfies, then promotes standard endpoints to enhanced
and vice versa through automatic conversion functions
(`invokableToStreamable`, `streamableToInvokable`, etc). This means a tool
author only needs to implement ONE execution method, and `ToolsNode` makes it
work in both Invoke and Stream contexts. Rive's node execution engine could
benefit from similar automatic capability bridging.

