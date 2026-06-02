# Go Replica Module/API Plan — Eino Compose Runtime (R1)

> Based on: CloudWeGo Eino technical manual (chapters 01–07), Rive repo structure, R1 goals.
> This is a design document. No implementation code.

---

## 1. R1 Scope — What to Replicate vs Interface-Only

Eino's `compose/` + supporting modules (`schema/`, `components/`, `callbacks/`) form a ~6000+ line Go codebase. R1 targets the **core compose runtime** — the engine that compiles graphs into runnable units and executes them — plus the **observability and resilience** layers. Provider-specific code, agent-level patterns, and the Pregel runtime are deferred to R2+.

### 1.1 Fully Replicate (implement)

| Capability | Eino Source Files | Rationale |
|---|---|---|
| `Runnable[I,O]` interface + auto-degrade packer | `compose/runnable.go` (334 lines) | Central abstraction; every component executes through it |
| Graph construction & compile pipeline | `compose/graph.go` (1219 lines), `compose/generic_graph.go` (158 lines) | Core topology → execution boundary |
| Graph runtime engine (DAG mode) | `compose/graph_run.go` (1055 lines), `compose/graph_manager.go` (556 lines), `compose/dag.go` (195 lines) | DAG execution: task submit, channel management, completion resolution |
| Stream primitives | `schema/stream.go`, `compose/stream_reader.go`, `compose/stream_concat.go` | Copy (fan-out), Merge (fan-in), Concat (fold), plus required close semantics |
| Callback system (5 stages) | `callbacks/interface.go`, `callbacks/handler_builder.go`, `internal/callbacks/inject.go` | OnStart/OnEnd/OnError/OnStartWithStreamInput/OnEndWithStreamOutput |
| Checkpoint / Interrupt / Resume | `compose/checkpoint.go` (373 lines), `compose/interrupt.go`, `compose/resume.go`, `internal/core/address.go`, `internal/core/interrupt.go` | Full address system, InterruptSignal tree, stateful interrupt, directed resume |
| Local state (`WithGenLocalState`) | `compose/graph.go:37-44`, `compose/graph_run.go:842-854` | Per-graph-instance shared state |
| Field mapping system | `compose/field_mapping.go`, `compose/workflow.go` | 6 mapping constructors, compile-time type validation |
| Conditional branching (`GraphBranch`) | `compose/branch.go` | Runtime condition evaluation |
| Component-to-node bridge | `compose/component_to_graph_node.go` | Component → `graphNode` conversion |
| Type inference (`toValidateMap`) | `compose/graph.go:561-668` | BFS-based deferred type resolution |
| `Chain` builder | `compose/chain.go` | Fluent sequential composition |
| `Workflow` declarative builder | `compose/workflow.go` | Declarative deps + field mapping |
| Graph introspection (`GraphInfo`, `GraphCompileCallback`) | `compose/introspect.go` | Compile-time observability export |
| Lambda (arbitrary node function) | `compose/runnable.go` — `InvokableLambda`, `StreamableLambda`, `CollectableLambda`, `TransformableLambda` | User-defined inline logic |
| Concat dispatch (`RegisterStreamChunkConcatFunc`) | `compose/stream_concat.go:44` | Type-specific stream aggregation |
| Gob-based serialization | `schema/register.go` | `RegisterName[T]` for checkpoint persistence |
| `ToolsNode` (parallel tool execution) | `compose/tool_node.go` | Tool call dispatch incl. interrupt-and-rerun |

### 1.2 Interface-Only (define types, no implementation)

| Capability | Reason for deferral |
|---|---|
| `BaseChatModel` / `ChatModel` / `AgenticModel` interfaces | External provider-dependent; define contracts in `components/model/` |
| `BaseTool` / `InvokableTool` / `StreamableTool` interfaces | External provider-dependent; define contracts in `components/tool/` |
| `ChatTemplate` (FString / GoTemplate / Jinja2) | Rendering engines are complex; define `ChatTemplate.Format` interface, defer engines |
| `Embedder` / `Indexer` / `Retriever` interfaces | RAG components; define contracts, defer implementations |
| Schema types: `Message`, `AgenticMessage` | Define struct definitions + getter/setter; defer provider extensions (openai/claude/gemini specific fields) |
| `ToolInfo` / params types | Define structs; defer JSON Schema generation |
| Provider adapters (eino-ext) | Entirely external repo concern |
| `StreamToolCallChecker` implementations | Provider-specific stream detection (OpenAI vs Claude) |
| Agent-level abstractions (ReAct, Host Multi-Agent) | Depend on fully implemented compose + component layers; R2 |
| Pregel runtime (`compose/pregel.go`) | Additional complexity; DAG mode is R1 priority; Pregel is R2 |
| `MessageRewriter` / `MessageModifier` | Agent-level concern |
| `WithMessageFuture` | Agent-level concern |
| Checkpoint Store implementations (Redis, etc.) | Define `CheckPointStore` interface; ship in-memory store for tests; R2 for backends |
| `WithGraphInterrupt` (external cancellation) | R2 |

---

## 2. Module Layout

### 2.1 Directory Tree

The replica lives at `examples/eino-compose-runtime-replica-go/` as a standalone Go module.

```
examples/eino-compose-runtime-replica-go/
│
├── go.mod                                  # module github.com/your-org/eino-compose-runtime-replica
├── go.sum
│
├── compose/                                # Core compose layer
│   ├── types.go                            # Component type constants (ComponentOfGraph, ComponentOfLambda, ...),
│   │                                       #   NodeTriggerMode enum, Sentinel errors
│   ├── runnable.go                         # Runnable[I,O] interface, composableRunnable,
│   │                                       #   runnablePacker (auto-degrade), InvokableLambda, etc.
│   ├── graph.go                            # graph struct, NewGraph[I,O], AddNode/AddEdge/AddBranch,
│   │                                       #   compile(), toValidateMap, fieldMapping conflict check,
│   │                                       #   START/END constants
│   ├── generic_graph.go                    # Graph[I,O] generic wrapper, Compile(), compileAnyGraph(),
│   │                                       #   WithGenLocalState
│   ├── graph_node.go                       # graphNode, executorMeta, nodeInfo, compileIfNeeded()
│   ├── graph_compile.go                    # Compile option types, GraphCompileOption, WithNodeTriggerMode,
│   │                                       #   WithGraphName, WithMaxRunSteps, WithEagerDisabled
│   ├── graph_run.go                        # runner struct, chanCall, run() main loop, calculateNextTasks,
│   │                                       #   handleInterrupt, resolveCompletedTasks, createTasks
│   ├── graph_manager.go                    # channel interface, channelManager, taskManager, handler managers
│   ├── dag.go                              # dagChannel (AllPredecessor), ControlPredecessors,
│   │                                       #   DataPredecessors, Skipped, get(), reportValues()
│   ├── pregel.go                           # pregelChannel (AnyPredecessor) — stub in R1, implement in R2
│   ├── chain.go                            # Chain[I,O] fluent builder, Parallel, Branch, auto preNodeKey
│   ├── workflow.go                         # Workflow[I,O], AddInput, noDirectDependency, branchDependency
│   ├── branch.go                           # GraphBranch, condition function, branch map
│   ├── field_mapping.go                    # FieldMapping, MapFields, FromField, ToField,
│   │                                       #   validateFieldMapping (compile-time type check)
│   ├── stream_reader.go                    # Internal streamReader interface (copy, merge, withKey, close...),
│   │                                       #   packStreamReader/unpackStreamReader
│   ├── stream_concat.go                    # concatStreamReader[T], RegisterStreamChunkConcatFunc
│   ├── checkpoint.go                       # checkpoint struct, WithCheckPointStore, WithCheckPointID,
│   │                                       #   WithForceNewRun, WithStateModifier, convertCheckPoint,
│   │                                       #   restoreCheckPoint, forwardCheckPoint, MigrateCheckpointState
│   ├── interrupt.go                        # Interrupt, StatefulInterrupt, CompositeInterrupt,
│   │                                       #   WrapInterruptAndRerunIfNeeded, ExtractInterruptInfo,
│   │                                       #   AddressSegment type constants
│   ├── resume.go                           # GetInterruptState, GetResumeContext, Resume, ResumeWithData,
│   │                                       #   BatchResumeWithData, AppendAddressSegment, GetCurrentAddress
│   ├── component_to_graph_node.go          # toComponentNode, toChatModelNode, toToolsNode,
│   │                                       #   toRetrieverNode, toPassthroughNode, parseExecutorInfo
│   ├── introspect.go                       # GraphInfo, GraphNodeInfo, GraphCompileCallback
│   ├── utils.go                            # runWithCallbacks, invokeWithCallbacks, streamWithCallbacks,
│   │                                       #   collectWithCallbacks, transformWithCallbacks,
│   │                                       #   initGraphCallbacks, initNodeCallbacks
│   ├── graph_add_node_options.go           # GraphAddNodeOpt, WithStatePreHandler, WithStatePostHandler,
│   │                                       #   WithNodeName, WithOutputKey, WithInputKey
│   ├── graph_call_options.go               # InvokeOption types, WithCallbacks, WithNodePath
│   └── graph_add_node_options_test.go      # Tests for node options
│
├── callbacks/                              # Public callback API
│   ├── interface.go                        # Handler interface (5 stages), RunInfo, TimingChecker
│   ├── handler_builder.go                  # HandlerBuilder, OnStartFn, OnEndFn...
│   └── inject.go                           # Callback context helper functions (ConvCallbackInput, etc.)
│
├── internal/                               # Internal machinery
│   ├── callbacks/
│   │   ├── manager.go                      # Manager (global + per-invocation handlers), AppendGlobalHandlers
│   │   └── inject.go                       # On[T] dispatch, Handle[T], OnWithStreamHandle,
│   │                                       #   stream copy for callbacks
│   ├── core/
│   │   ├── address.go                      # Address type, AddressSegment, AppendAddressSegment,
│   │   │                                   #   PopulateInterruptState, GetNextResumptionPoints
│   │   ├── interrupt.go                    # InterruptSignal tree, core.Interrupt,
│   │   │                                   #   SignalToPersistenceMaps, ToInterruptContexts,
│   │   │                                   #   FromInterruptContexts, CheckPointStore interface
│   │   └── resume.go                       # GetInterruptState, GetResumeContext implementations,
│   │                                       #   globalResumeInfo, getRunCtx
│   ├── concat.go                           # ConcatItems dispatch (registered concat functions)
│   └── serialization/
│       └── gob.go                          # Gob encoder/decoder registration helpers
│
├── schema/                                 # Canonical data types
│   ├── message.go                          # Message struct (Role, Content, ToolCalls, ResponseMeta,
│   │                                       #   ReasoningContent), constructors
│   ├── agentic_message.go                  # AgenticMessage struct, ContentBlock tagged union (20+ variants)
│   ├── message_convert.go                  # Message ↔ AgenticMessage conversion
│   ├── tool.go                             # ToolInfo, ParamsOneOf ByParams / ByJsonSchema,
│   │                                       #   emptyToolCall, FunctionToolCall
│   ├── stream.go                           # StreamReader[T], StreamWriter[T], Copy, MergeStreamReaders,
│   │                                       #   MergeNamedStreamReaders, StreamReaderWithConvert,
│   │                                       #   Pipe, StreamReaderFromArray
│   ├── register.go                         # RegisterName[T], Register[T] for serialization
│   └── types.go                            # Role constants (User, Assistant, System, Tool)
│
├── components/                             # Component interface contracts
│   ├── types.go                            # Component type constants & Typer/Checker interfaces
│   ├── model/
│   │   └── interface.go                    # BaseChatModel, ChatModel, ToolCallingChatModel, AgenticModel,
│   │                                       #   Option (dual-bucket), WithTools
│   ├── tool/
│   │   └── interface.go                    # BaseTool, InvokableTool, StreamableTool,
│   │                                       #   EnhancedInvokableTool, EnhancedStreamableTool
│   ├── prompt/
│   │   └── interface.go                    # ChatTemplate interface (Format, MessagesTemplate)
│   ├── embedding/
│   │   └── interface.go                    # Embedder interface (EmbedStrings)
│   ├── indexer/
│   │   └── interface.go                    # Indexer interface (Store)
│   ├── retriever/
│   │   └── interface.go                    # Retriever interface (Retrieve)
│   └── callback.go                         # CallbackInput, CallbackOutput per-component extra types,
│                                           #   ConvCallbackInput/Output helpers
│
├── research/                               # Design & analysis documents
│   └── go-replica-plan.md                  # This file
│
└── tests/                                  # Integration / behavior tests
    ├── compose/
    │   ├── graph_test.go                   # Graph construction + compile + invoke
    │   ├── graph_run_test.go               # Runtime execution: DAG, eager, interrupt/resume
    │   ├── graph_compile_test.go           # Compile validation: type inference, field mapping
    │   ├── stream_test.go                  # Stream Reader: copy, merge, concat lifecycle
    │   ├── callback_test.go                # Callback: 5 stages, TimingChecker, global + per-invoke
    │   ├── checkpoint_test.go              # Checkpoint: save/load, stream conversion, migration
    │   ├── field_mapping_test.go           # Field mapping: 6 constructors, validation edge cases
    │   ├── workflow_test.go                # Workflow: deps, field mapping, noDirectDependency
    │   ├── chain_test.go                   # Chain: sequential, parallel, branch
    │   ├── branch_test.go                  # GraphBranch: condition routing
    │   ├── tool_node_test.go               # ToolsNode: sequential/parallel, interrupt-and-rerun
    │   └── lambda_test.go                  # Lambda: all 4 modes, auto-degrade
    ├── schema/
    │   ├── message_test.go                 # Message struct, constructors, conversion
    │   ├── stream_test.go                  # StreamReader: Copy, Merge, Pipe, close semantics
    │   └── register_test.go                # Serialization round-trip
    └── components/
        └── interface_test.go               # Interface compliance (no-op implementations)
```

### 2.2 Go Module Identity

```
module github.com/your-org/eino-compose-runtime-replica

go 1.22

require (
    // Minimal dependencies. In R1, aim for zero third-party dependencies
    // beyond the Go standard library.
)
```

### 2.3 Package Dependency Graph

```
                ┌──────────┐
                │  tests/  │ (external test packages)
                └────┬─────┘
                     │ imports
    ┌────────────────┼────────────────┐
    │                │                │
    v                v                v
┌────────┐    ┌──────────┐    ┌────────────┐
│ schema │    │ compose  │    │ components │
└───┬────┘    └────┬─────┘    └─────┬──────┘
    │              │                │
    │     ┌────────┴────────┐       │
    │     v                 v       │
    │ ┌──────────┐   ┌──────────┐   │
    │ │ callbacks│   │ internal │   │
    │ └──────────┘   └──────────┘   │
    │                                │
    └────────────────────────────────┘
              (leaf packages)

Dependency rules:
  - `schema/`                — NO internal dependencies (leaf)
  - `callbacks/`             — NO internal dependencies (leaf)
  - `components/`            — depends on `schema/` only
  - `internal/core/`         — depends on `schema/` only
  - `internal/callbacks/`    — depends on `callbacks/`, `internal/concat/`, `schema/`
  - `internal/concat/`       — NO internal dependencies (leaf)
  - `internal/serialization/`— NO internal dependencies (leaf)
  - `compose/`               — depends on ALL above (orchestration package)
  - `tests/`                 — depends on `compose/`, `schema/`, `components/`
```

---

## 3. Public API Design

### 3.1 Core Type: `Runnable[I, O any]` (in `compose/`)

```go
// compose/runnable.go
type Runnable[I, O any] interface {
    Invoke(ctx context.Context, input I, opts ...Option) (output O, err error)
    Stream(ctx context.Context, input I, opts ...Option) (output *schema.StreamReader[O], err error)
    Collect(ctx context.Context, input *schema.StreamReader[I], opts ...Option) (output O, err error)
    Transform(ctx context.Context, input *schema.StreamReader[I], opts ...Option) (output *schema.StreamReader[O], err error)
}
```

This is the **single entry point** for all graph execution. Every Graph/Chain/Workflow compiles to a `Runnable[I,O]`.

### 3.2 Graph Construction API (in `compose/`)

```go
// compose/generic_graph.go
func NewGraph[I, O any](opts ...GraphOption) *Graph[I, O]

// Node additions
func (g *Graph[I, O]) AddLambdaNode(key string, node *Lambda, opts ...GraphAddNodeOpt) error
func (g *Graph[I, O]) AddGraphNode(key string, subGraph AnyGraph, opts ...GraphAddNodeOpt) error
// More specializations for ChatModel, Tool, Retriever — deferred to component bridge

// Edge operations
func (g *Graph[I, O]) AddEdge(from, to string) error
func (g *Graph[I, O]) AddBranch(key string, branch *GraphBranch) error

// Compile
func (g *Graph[I, O]) Compile(ctx context.Context, opts ...GraphCompileOption) (Runnable[I, O], error)
```

### 3.3 Lambda (user-defined node logic)

```go
// compose/runnable.go
func InvokableLambda[I, O any](fn func(context.Context, I) (O, error)) *Lambda
func StreamableLambda[I, O any](fn func(context.Context, I) (*schema.StreamReader[O], error)) *Lambda
func CollectableLambda[I, O any](fn func(context.Context, *schema.StreamReader[I]) (O, error)) *Lambda
func TransformableLambda[I, O any](fn func(context.Context, *schema.StreamReader[I]) (*schema.StreamReader[O], error)) *Lambda
```

### 3.4 Chain Builder API

```go
// compose/chain.go
func NewChain[I, O any]() *Chain[I, O]

func (c *Chain[I, O]) AppendGraph(graph AnyGraph, opts ...GraphAddNodeOpt) *Chain[I, O]
func (c *Chain[I, O]) AppendLambda(lambda *Lambda, opts ...GraphAddNodeOpt) *Chain[I, O]
func (c *Chain[I, O]) Parallel() *Chain[I, O]
func (c *Chain[I, O]) AppendBranch(condition GraphBranchCondition[O]) *Chain[I, O]

func (c *Chain[I, O]) Compile(ctx context.Context, opts ...GraphCompileOption) (Runnable[I, O], error)
```

### 3.5 Workflow API

```go
// compose/workflow.go
func NewWorkflow[I, O any](opts ...GraphOption) *Workflow[I, O]

func (wf *Workflow[I, O]) AddInput(dependentNode string, dependencyNodes ...string) *Workflow[I, O]
func (wf *Workflow[I, O]) AddInputWithOptions(dep string, fm *FieldMapping, dependencies ...string) *Workflow[I, O]

func (wf *Workflow[I, O]) Compile(ctx context.Context, opts ...GraphCompileOption) (Runnable[I, O], error)
```

### 3.6 Field Mapping API

```go
// compose/field_mapping.go
type FieldMapping struct { ... }

func MapFields(fromNode, fromField, toNode, toField string) *FieldMapping
func FromField(fromNode, fromField string) *FieldMapping
func ToField(toNode, toField string) *FieldMapping
func MapFieldPaths(fromPath, toPath []string) *FieldMapping
func FromFieldPath(path []string) *FieldMapping
func ToFieldPath(path []string) *FieldMapping
```

### 3.7 Callback API (in `callbacks/`)

```go
// callbacks/interface.go
type Handler interface {
    OnStart(ctx context.Context, info *RunInfo, input CallbackInput) context.Context
    OnEnd(ctx context.Context, info *RunInfo, output CallbackOutput) context.Context
    OnError(ctx context.Context, info *RunInfo, err error) context.Context
    OnStartWithStreamInput(ctx context.Context, info *RunInfo, input *schema.StreamReader[CallbackInput]) context.Context
    OnEndWithStreamOutput(ctx context.Context, info *RunInfo, output *schema.StreamReader[CallbackOutput]) context.Context
}

type TimingChecker interface {
    Needed(ctx context.Context, info *RunInfo, timing CallbackTiming) bool
}

// callbacks/handler_builder.go
func NewHandlerBuilder() *HandlerBuilder
func (hb *HandlerBuilder) OnStartFn(fn OnStartFn) *HandlerBuilder
// ... OnEndFn, OnErrorFn, OnStartWithStreamInputFn, OnEndWithStreamOutputFn
func (hb *HandlerBuilder) Build() Handler

func AppendGlobalHandlers(handlers ...Handler)
```

### 3.8 Checkpoint / Interrupt / Resume API (in `compose/`)

```go
// compose/interrupt.go
func Interrupt(ctx context.Context, info any) error
func StatefulInterrupt[T any](ctx context.Context, info any, state T) error
func CompositeInterrupt(ctx context.Context, info any, state any, errs ...error) error
func ExtractInterruptInfo(err error) (*InterruptInfo, bool)

// compose/resume.go
func GetInterruptState[T any](ctx context.Context) (wasInterrupted bool, hasState bool, state T)
func GetResumeContext[T any](ctx context.Context) (isResumeTarget bool, hasData bool, data T)
func Resume(ctx context.Context, interruptID string) context.Context
func ResumeWithData(ctx context.Context, interruptID string, data any) context.Context
func BatchResumeWithData(ctx context.Context, resumeMap map[string]any) context.Context
func AppendAddressSegment(ctx context.Context, segType AddressSegmentType, id string) context.Context

// compose/checkpoint.go
func WithCheckPointID(id string) GraphCompileOption
func WithCheckPointStore(store CheckPointStore) GraphCompileOption
func WithForceNewRun() GraphCompileOption
func WithStateModifier(modifier StateModifier) GraphCompileOption
func MigrateCheckpointState(data []byte, serializer Serializer, migrateFn func(state any) (any, bool, error)) ([]byte, error)
```

### 3.9 Stream API (in `schema/`)

```go
// schema/stream.go
type StreamReader[T any] struct { ... }

func (sr *StreamReader[T]) Recv() (T, error)
func (sr *StreamReader[T]) Close()
func (sr *StreamReader[T]) Copy(n int) []*StreamReader[T]

func StreamReaderFromArray[T any](items []T) *StreamReader[T]
func Pipe[T any](capacity int) (*StreamReader[T], *StreamWriter[T])

func MergeStreamReaders[T any](readers []*StreamReader[T]) *StreamReader[T]
func MergeNamedStreamReaders[T any](readers []*StreamReader[T], names []string) *StreamReader[map[string]T]
func StreamReaderWithConvert[T, U any](sr *StreamReader[T], convertFn func(T) (U, error)) *StreamReader[U]
```

### 3.10 Component Interfaces (in `components/`)

```go
// components/model/interface.go
type Message any // will be replaced by *schema.Message in actual code
type BaseChatModel interface {
    Generate(ctx context.Context, input []Message, opts ...model.Option) (*schema.Message, error)
    Stream(ctx context.Context, input []Message, opts ...model.Option) (*schema.StreamReader[*schema.Message], error)
}
type ToolCallingChatModel interface {
    BaseChatModel
    WithTools(tools []*schema.ToolInfo) ToolCallingChatModel
}

// components/tool/interface.go
type BaseTool interface {
    Info(ctx context.Context) (*schema.ToolInfo, error)
}
type InvokableTool interface {
    BaseTool
    InvokableRun(ctx context.Context, argumentsInJSON string, opts ...tool.Option) (string, error)
}
type StreamableTool interface {
    BaseTool
    StreamableRun(ctx context.Context, argumentsInJSON string, opts ...tool.Option) (*schema.StreamReader[string], error)
}
```

### 3.11 Compile Options (in `compose/graph_compile.go`)

```go
type GraphCompileOption func(*graphCompileOptions)

func WithGraphName(name string) GraphCompileOption
func WithNodeTriggerMode(mode NodeTriggerMode) GraphCompileOption
func WithMaxRunSteps(maxSteps int) GraphCompileOption
func WithEagerExecutionDisabled() GraphCompileOption
```

---

## 4. Key Design Consistency Rules

These rules must hold across all R1 code to match Eino's actual behavior:

### 4.1 Compile Boundary Immutability

- After `Compile()`, set `graph.compiled = true`.
- Every mutating operation (`AddNode`, `AddEdge`, `AddBranch`) checks `graph.compiled` and returns `ErrGraphCompiled` if true.
- The same `Graph` / `Chain` / `Workflow` instance can be compiled multiple times with different options, producing independent `Runnable` instances.

### 4.2 Runnable Auto-Degrade

- The `runnablePacker` fills the 4-method `Runnable[I,O]` interface from any subset.
- Priority order: Transform → Stream → Collect → Invoke (most capable to least).
- Each degrade function closes stream readers with `defer sr.Close()`.

### 4.3 Stream Reader Close Semantics

- `Copy(n)` must be called before the first `Recv()` on any child.
- Every consumer (node, callback handler) must close its own reader copy.
- `MergeStreamReaders` closes individual readers after consumption.
- `Pipe` returns a paired writer/reader; closing the writer signals EOF to the reader.

### 4.4 Callback Context Isolation

- Context does NOT flow between different handlers.
- Each handler gets the previous stage's context from the SAME handler, not from other handlers.
- Global handlers execute before per-invocation handlers.

### 4.5 Address System

- Addresses are `[]AddressSegment`, not flat strings.
- `AddressSegment.Type` is one of: `node`, `tool`, `runnable`.
- `AddressSegment.SubID` disambiguates parallel tool calls.
- `AppendAddressSegment` checks `globalResumeInfo` on every new scope entry.

### 4.6 Interrupt Propagation

- Sub-graph interrupts must bubble up through `subGraphInterruptError`.
- Parent graphs collect sub-signals into a unified `InterruptSignal` tree.
- `forwardCheckPoint` uses delete-once pattern to prevent double-consumption.

### 4.7 Channel Abstraction

- All node communication goes through the `channel` interface (`reportValues`, `reportDependencies`, `reportSkip`, `get`).
- DAG channel enforces `AllPredecessor`: waits for all control + data predecessors.
- DAG channel supports skip propagation: if all control predecessors are skipped, the node itself is skipped.

---

## 5. Test Strategy

### 5.1 Test Organization

```
tests/
├── compose/
│   ├── graph_test.go              # Graph build → compile → invoke: basic, branching, nested
│   ├── graph_run_test.go          # Runtime: dag execution order, eager vs non-eager,
│   │                              #   interrupt/resume lifecycle, maxSteps enforcement
│   ├── graph_compile_test.go      # Compile validation: ErrGraphCompiled, type inference,
│   │                              #   field mapping conflicts, DAG cycle detection
│   ├── stream_test.go             # StreamReader lifecycles: copy fan-out, merge fan-in,
│   │                              #   concat, close leak detection, Pipe
│   ├── callback_test.go           # 5 stages fire correctly, TimingChecker, global priority,
│   │                              #   context isolation, stream copy for callbacks
│   ├── checkpoint_test.go         # Checkpoint save/load round-trip, stream conversion,
│   │                              #   sub-graph checkpoint, migration, StateModifier
│   ├── field_mapping_test.go      # All 6 constructors, compile-time validation, edge cases
│   ├── workflow_test.go           # Dependency resolution, field mapping, NoDirectDependency
│   ├── chain_test.go              # Sequential append, Parallel fan-out, Branch routing
│   ├── branch_test.go             # Condition evaluation, branch key validation
│   ├── tool_node_test.go          # Sequential/parallel execution, argument aliasing,
│   │                              #   interrupt-and-rerun, enhanced tool conversion
│   └── lambda_test.go             # All 4 modes individually, auto-degrade combinations
├── schema/
│   ├── message_test.go            # Message/AccessoryMessage constructors, Role validation
│   ├── stream_test.go             # Copy before Recv, merge ordering, close propagation
│   └── register_test.go           # RegisterName round-trip through gob
└── components/
    └── interface_test.go          # Compile-time interface satisfaction checks (no-op)
```

### 5.2 Test Principles

**1. Behavior-spec, not implementation-unit tests.** Each test describes a graph scenario (e.g., "3-node DAG with branching") and asserts runtime behavior (execution order, channel values, interrupt points). Avoid testing private helper functions directly.

**2. Stream leak detection.** Every stream test must use a goroutine leak detector or channel-buffer counting pattern. Close semantics are the most error-prone part of the system.

**3. Deterministic execution order.** For DAG-mode tests, use a `taskObserver` pattern (or buffered channel) to record execution order and assert specific node sequences.

**4. Checkpoint round-trip.** Every feature that integrates with checkpoint (interrupt, sub-graph, stream mode) must have a save → force-new-run → load → resume → verify cycle test.

**5. Compile-phase validation tests.** Test that bad inputs fail at compile time, not runtime:
- Adding an edge after `Compile()` → `ErrGraphCompiled`
- Cyclic DAG → `DAGInvalidLoopErr`
- Conflicting field mappings → compile error
- Unresolved type inference chain → compile error

**6. Concurrency stress.** `ToolsNode` parallel tests and taskManager goroutine pool tests must run under `-race`.

### 5.3 Test Fixtures

All tests should use self-contained graphs built inline (no external dependencies):

```go
// Canonical test pattern:
func TestBasicDAG(t *testing.T) {
    g := compose.NewGraph[string, string]()
    g.AddLambdaNode("a", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
        return "from_a:" + in, nil
    }))
    g.AddLambdaNode("b", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
        return "from_b:" + in, nil
    }))
    g.AddEdge(compose.START, "a")
    g.AddEdge("a", "b")
    g.AddEdge("b", compose.END)

    r, err := g.Compile(context.Background(),
        compose.WithNodeTriggerMode(compose.AllPredecessor),
    )
    require.NoError(t, err)

    result, err := r.Invoke(context.Background(), "hello")
    require.NoError(t, err)
    assert.Equal(t, "from_b:from_a:hello", result)
}
```

### 5.4 Coverage Targets (R1)

| Area | Target |
|---|---|
| Graph compile (happy path + error paths) | 100% |
| DAG runtime (all edge cases: skip, merge, eager) | 100% |
| Stream (copy, merge, concat, close) | 100% (including leak-free) |
| Callback (5 stages, TimingChecker, context isolation) | 100% |
| Checkpoint/Interrupt/Resume (full lifecycle) | 100% |
| Field mapping (all 6 constructors, validation) | 100% |
| Component interfaces (satisfaction checks) | 80% (interface-only; deep tests in R2) |

---

## 6. Implementation Order (R1 Milestones)

Recommended build sequence within R1:

### M1: Foundation (schema + stream + internal)
- `schema/`: Message, StreamReader/Writer, registration
- `internal/core/`: Address, AddressSegment
- `internal/concat/`: Generic concat dispatch
- `internal/serialization/`: Gob helpers

**Verification**: `go test ./schema/... ./internal/...`

### M2: Runnable + Stream Concat
- `compose/runnable.go`: Runnable interface, composableRunnable, runnablePacker
- `compose/stream_reader.go`: Internal stream wrapper
- `compose/stream_concat.go`: concatStreamReader, RegisterStreamChunkConcatFunc

**Verification**: Lambda tests (all 4 modes individually, auto-degrade matrix)

### M3: Graph Construction + Compile
- `compose/types.go`: Constants, sentinel errors
- `compose/graph.go`: graph struct, AddNode/AddEdge/AddBranch, compile skeleton
- `compose/graph_node.go`: graphNode, executorMeta, compileIfNeeded
- `compose/generic_graph.go`: Graph[I,O] wrapper, Compile, WithGenLocalState
- `compose/graph_compile.go`: Compile options
- `compose/dag.go`: dagChannel
- `compose/field_mapping.go`: FieldMapping system
- `compose/branch.go`: GraphBranch
- `compose/introspect.go`: GraphInfo export

**Verification**: Graph compile tests (type inference, field mapping validation, cycle detection)

### M4: Runtime Engine
- `compose/graph_manager.go`: channel interface, channelManager, taskManager
- `compose/graph_run.go`: runner, main loop, calculateNextTasks, createTasks

**Verification**: Graph run tests (DAG execution order, eager, branching, nested sub-graphs)

### M5: Callbacks
- `callbacks/`: Handler interface, HandlerBuilder
- `internal/callbacks/`: Manager, On[T] dispatch, stream copy
- `compose/utils.go`: runWithCallbacks wrappers
- `compose/component_to_graph_node.go`: Component bridging

**Verification**: Callback tests (5 stages, TimingChecker, context isolation, global priority)

### M6: Checkpoint / Interrupt / Resume
- `internal/core/interrupt.go`: InterruptSignal tree
- `internal/core/resume.go`: globalResumeInfo logic
- `compose/interrupt.go`: Public interrupt API
- `compose/resume.go`: Public resume API
- `compose/checkpoint.go`: Checkpoint persistence, stream conversion, migration

**Verification**: Full checkpoint/interrupt/resume lifecycle tests

### M7: Chain + Workflow + ToolsNode
- `compose/chain.go`: Chain builder
- `compose/workflow.go`: Workflow + field mapping integration
- `compose/tool_node.go`: ToolsNode (sequential + parallel)
- `compose/graph_add_node_options.go`: State pre/post handlers
- `compose/graph_call_options.go`: Invoke options, WithCallbacks, WithNodePath

**Verification**: Chain/Workflow integration tests, ToolsNode interrupt-and-rerun tests

### M8: Components + Schema (interface contracts)
- `components/`: All interface definitions
- `schema/agentic_message.go`: AgenticMessage + ContentBlock

**Verification**: Compile-time interface satisfaction tests

---

## 7. What Eino Capabilities Are NOT Covered in This Plan (Explicit Deferrals)

| Capability | Reason | Target |
|---|---|---|
| Pregel runtime (AnyPredecessor, cyclic graphs) | Adds significant complexity; DAG covers 80% of use cases | R2 |
| Provider-specific schema extensions (OpenAI annotations, Claude citations, Gemini grounding) | Requires deep provider knowledge; no R1 runtime benefit | R2 |
| Provider adapters (eino-ext) | Separate repository concern | External |
| ReAct agent | Depends on fully working compose + chat model implementations | R2 |
| Host Multi-Agent | Depends on ReAct + ToolsNode maturity | R2 |
| MessageRewriter / MessageModifier | Agent-level concern | R2 |
| WithMessageFuture | Nested graph callback isolation pattern; R2 | R2 |
| ChatTemplate rendering engines (FString, GoTemplate, Jinja2) | Interface defined; engines are independent modules | R2 |
| Embedder / Indexer implementations | Provider-dependent | R3 |
| External CheckPointStore backends (Redis, S3) | Define interface; ship in-memory for tests | R2 |
| GraphCompileCallback integration with external systems | Define callback interface; integration is consumer concern | Post-R1 |
| WithGraphInterrupt (external graph cancellation) | R2 | R2 |
| maxSteps enforcement in DAG mode | DAG is acyclic; only needed for Pregel | R2 |
| passthrough node type chain inference beyond 2 levels | Complex edge case; BFS-based system handles 2 levels; extend in R2 | R2 |
| Stream EOF source name detection (`schema.GetSourceName`) | Merge-specific optimization | R2 |
| `schema.StreamReader` internal backends beyond `stream` and `array` | `multi-stream`, `with-convert`, `child` backends deferred | R2 |
| EnhancedTool output conversion (structured → message) | Requires full schema integration | R2 |

---

## 8. Risk Register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Stream goroutine leaks in callback handlers | High | High (production memory leak) | Dedicated leak detector in test harness; `defer sr.Close()` linter |
| Type inference edge cases not covered by test matrix | Medium | Medium (compile failures) | Property-based tests for type chain resolution |
| DAG skip propagation logic errors | Medium | High (silent node omission) | Exhaustive skip-propagation test table |
| Checkpoint migration breaking old checkpoints | Low | High (data loss) | Version-tagged checkpoint format; backward-compat test |
| Concurrent local state access with eager execution | Medium | Medium (race conditions) | Document state access pattern; `-race` in CI |
| Gob encoding failures for unregistered custom types | Medium | Medium (checkpoint failure) | Registration validation at graph compile time |

---

## 9. Success Criteria (R1 Done)

1. A 3-node DAG graph (Lambda → Lambda → Lambda) compiles and runs via `Invoke`, producing correct output.
2. Stream mode works end-to-end: `Stream` produces a readable `StreamReader`, `Collect` consumes one.
3. A graph with `Copy` (fan-out to 2 downstream nodes) runs without goroutine leaks.
4. All 5 callback stages fire at correct times; `TimingChecker` prevents unnecessary stream copies.
5. A graph interrupts mid-execution, checkpoints state, and resumes from exact point with data.
6. A sub-graph interrupt bubbles to parent; `forwardCheckPoint` works.
7. Field mapping validates at compile time and routes data correctly at runtime.
8. `Chain` and `Workflow` builders produce identical runtime behavior to equivalent `Graph`.
9. All tests pass with `-race` flag.
10. Package `go doc` output is self-documenting for all public API surfaces.

---

*Plan version: R1-draft-1. Generated for Rive dispatch `disp_f2d2541b70254c62a0544b50cd27e178`, work node `work_b1d651c0a6584c7883c5f9643ad5eaa0`.*
