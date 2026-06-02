# 03 — Runnable / Stream / Callback Runtime Patterns

## 1. Problem

Eino's graph compilation output is `Runnable[I, O]` — a single interface that unifies four data flow patterns:

```go
// compose/runnable.go:32-37
type Runnable[I, O any] interface {
    Invoke(ctx context.Context, input I, opts ...Option) (output O, err error)
    Stream(ctx context.Context, input I, opts ...Option) (output *schema.StreamReader[O], err error)
    Collect(ctx context.Context, input *schema.StreamReader[I], opts ...Option) (output O, err error)
    Transform(ctx context.Context, input *schema.StreamReader[I], opts ...Option) (output *schema.StreamReader[O], err error)
}
```

Every node in an Eino graph — whether it is a ChatModel, a Retriever, a Lambda, or a nested sub-Graph — is compiled into `composableRunnable` and executed through one of these four modes. The runtime must:

1. Accept components that implement only a subset of these four methods (e.g., a model that only exposes `Generate` and `Stream`) and automatically derive the missing methods.
2. Fire callbacks at the right lifecycle moments, without the component author writing callback dispatch manually.
3. Handle stream copying correctly — fanning out a single stream to N concurrent consumers (downstream nodes + callback handlers) without deadlocks or goroutine leaks.
4. Convert component-level abstractions (`BaseChatModel`, `Retriever`, `ChatTemplate`, etc.) into uniform graph nodes so the graph scheduler does not need to know component-specific internals.

If any of these is done wrong, the graph either silently drops stream data, leaks goroutines, fires callbacks with stale context, or panics on a type mismatch.

## 2. Why It Is Hard

**Multi-mode downgrade is combinatorially complex.** With four methods (I, S, C, T) and arbitrary subsets that a component may implement, the runtime needs `4 * 3 = 12` downgrade functions. Getting the priority right matters: invoking via Transform is cheaper than invoking via Stream (one stream conversion vs. two), and invoking via Stream is cheaper than invoking via Collect (one concat vs. two). The allocation order in `newRunnablePacker` (`compose/runnable.go:336-400`) is carefully chosen and documented nowhere except in code.

**Stream copying has subtle ownership semantics.** `schema.StreamReader.Copy` (`schema/stream.go:261-275`) fans out a single source into N independent child readers using a linked-list-based shared buffer (`parentStreamReader` + `childStreamReader`, lines 784-898). Each child MUST `Close()` its reader after consuming. If any child fails to close, the parent never closes the original reader, leaking the underlying channel goroutine. Callback handlers automatically receive copies via `OnStartWithStreamInputHandle` / `OnEndWithStreamOutputHandle` (`internal/callbacks/inject.go:163-193`) — if the handler forgets to close its copy, the entire pipeline leaks.

**Callback context chains are handler-isolated, not globally ordered.** Each handler receives the context returned by the previous timing of the *same* handler, but context does NOT flow from one handler to the next (`callbacks/interface.go:67-71`). Handlers that mutate context with `context.WithValue` and assume the next handler sees it will be surprised.

**Component-to-node conversion must reconcile two runtime models.** Components like `BaseChatModel` (`components/model/interface.go`) have `Generate` + `Stream`, while the graph engine speaks `Invoke`/`Stream`/`Collect`/`Transform` via `composableRunnable`. The conversion must also set up `executorMeta` — component category, implementation type, and whether the component self-reports callbacks — so the graph runtime can correctly decide when to fire `OnStart`/`OnEnd` and when to skip.

## 3. Design Idea

Eino splits this into four cooperating layers:

**Layer 1: Runnable interface + packer.** `compose/runnable.go` defines `Runnable[I,O]` and `composableRunnable`, the internal wrapper that every graph node is compiled into. `runnablePacker` takes the raw function pointers (`Invoke`, `Stream`, `Collect`, `Transform`) from a component and fills in missing methods via automatic downgrade.

**Layer 2: Stream primitives.** `schema/stream.go` provides `StreamReader`/`StreamWriter` (based on goroutine channels), `Copy` for fan-out, `MergeStreamReaders` for fan-in, and `StreamReaderWithConvert` for type conversion on the fly. `compose/stream_reader.go` wraps this with an internal `streamReader` interface that `composableRunnable` works with — adding `close()`, `copy(n)`, `merge()`, `mergeWithNames()`, `withKey()`, and `toAnyStreamReader()`.

**Layer 3: Callback engine.** `callbacks/` exposes the public `Handler` interface with five timings. `internal/callbacks/` manages per-invocation handler lists, dispatches timings, and enforces `TimingChecker` to skip unnecessary stream copies. The machinery lives in `compose/utils.go` (`invokeWithCallbacks`, `streamWithCallbacks`, etc.) which wraps each execution mode with `runWithCallbacks`.

**Layer 4: Component-to-node bridge.** `compose/component_to_graph_node.go` provides one function per component type (`toChatModelNode`, `toRetrieverNode`, `toToolsNode`, etc.). Each calls `toComponentNode`, which builds `executorMeta` via `parseExecutorInfoFromComponent` (checking for `components.Typer` and `callbacks.Checker`) and wraps the component's methods into `composableRunnable` via `runnableLambda`.

The key insight: **the graph scheduler never sees components — it only sees `composableRunnable`**. This makes the scheduler generic over all component types, and makes callback/recovery logic reusable across the entire system.

## 4. Source Walkthrough

### 4.1 Runnable and automatic downgrade (`compose/runnable.go`)

The core function is `newRunnablePacker` (line 336). It takes at most four function pointers (some may be nil) and fills in all four methods. The priority cascade for each method:

- **Invoke**: prefer native I, else `invokeByStream` (line 194: call Stream, concat stream reader), else `invokeByCollect` (line 205: wrap input as array stream, call Collect), else `invokeByTransform` (line 213: array stream → Transform → concat output stream).
- **Stream**: prefer native S, else `streamByTransform` (line 226: array stream input → Transform), else `streamByInvoke` (line 234: Invoke → wrap output as array stream), else `streamByCollect` (line 245: array stream input → Collect → wrap output as array stream).
- **Collect**: prefer native C, else `collectByTransform` (Transform → concat output), else `collectByInvoke` (concat input → Invoke), else `collectByStream` (concat input → Stream → concat output).
- **Transform**: prefer native T, else `transformByStream` (concat input → Stream), else `transformByCollect` (Collect → wrap output as array stream), else `transformByInvoke` (concat input → Invoke → wrap output as array stream).

Each downgrade function is documented by name alone — there is no additional commentary beyond the function signature. The correctness relies on `concatStreamReader` (`compose/stream_concat.go:50-88`), which consumes all chunks from a `StreamReader`, concatenates them via `internal.ConcatItems` (which dispatches to type-specific concat funcs registered by `RegisterStreamChunkConcatFunc`, line 44), and **always closes the reader** (line 51: `defer sr.Close()`).

**Important detail**: if a component only implements `Stream()` but the user calls `Invoke()`, the runtime calls `invokeByStream` which calls `Stream` then `concatStreamReader`. If the stream type has no registered concat function, `internal.ConcatItems` falls back to returning the last chunk — which works for "incremental update" types like `schema.Message` (each chunk is a complete message that supersedes the previous one).

`composableRunnable` (line 46) stores only two internal execution functions (`i invoke` and `t transform`), plus metadata (`inputType`, `outputType`, `optionType`, `isPassthrough`, `meta`, `nodeInfo`). The other two modes (Stream, Collect) are derived from these two at graph runtime via `toGenericRunnable` (line 402).

### 4.2 Stream reader internals (`compose/stream_reader.go`)

The internal `streamReader` interface (line 26) wraps `schema.StreamReader` to add operations needed by the compose runtime:

```go
type streamReader interface {
    copy(n int) []streamReader
    getType() reflect.Type
    getChunkType() reflect.Type
    merge([]streamReader) streamReader
    withKey(string) streamReader
    close()
    toAnyStreamReader() *schema.StreamReader[any]
    mergeWithNames([]streamReader, []string) streamReader
}
```

`packStreamReader` / `unpackStreamReader` (lines 111-128) convert between the typed `*schema.StreamReader[T]` and the untyped `streamReader` using a generic wrapper `streamReaderPacker[T]`. The `copy` method (line 45) delegates to `sr.Copy(n)` and re-wraps each copy. The `merge` method (line 79) calls `schema.MergeStreamReaders`. The `withKey` method (line 95) uses `schema.StreamReaderWithConvert` to map each chunk `T` into `map[string]any{key: T}`.

The `unpackStreamReader` function has special handling for interface types (line 121-127): if `T` is an interface, it converts the stream to `*StreamReader[any]` first, then wraps it with a typed conversion back to `T`. This allows runtime type erasure to work correctly when the graph has nodes with dynamically-typed outputs.

### 4.3 Stream concatenation (`compose/stream_concat.go`)

`concatStreamReader[T]` (line 50) is the workhorse behind every downgrade that needs to collapse a stream into a single value. It:

1. Defers `sr.Close()` (line 51) — critical for resource cleanup.
2. Loops with `sr.Recv()` until `io.EOF`.
3. Skips `SourceEOF` errors from merged streams (line 62: `schema.GetSourceName(err)` — useful when a multi-stream merge signals per-source completion).
4. Accumulates all chunks into a slice.
5. If the slice is empty, returns `emptyStreamConcatErr`.
6. If there is exactly one chunk, returns it directly (no concat overhead).
7. Otherwise, calls `internal.ConcatItems(items)` to build the final value.

The public `RegisterStreamChunkConcatFunc[T]` (line 44) lets users register custom concat logic for their types. Registration is not thread-safe and must be done at program init. The doc example shows concatenating a struct by taking the latest value of one field and summing another — a pattern typical of streaming token aggregation.

### 4.4 Callback interface (`callbacks/interface.go` + `internal/callbacks/interface.go`)

**Public `RunInfo`** (`callbacks/interface.go:41`): three fields:
- `Name`: graph node name from `compose.WithNodeName`, or empty for standalone components that haven't called `InitCallbacks`.
- `Type`: implementation identity (e.g. `"OpenAI"`) from `components.Typer`; falls back to reflection-derived type name.
- `Component`: category constant from `components.Component` (e.g. `ComponentOfChatModel`, `ComponentOfRetriever`). Fixed to `"Graph"`/`"Chain"`/`"Workflow"` for graph-level invocations, `"Lambda"` for lambdas.

**Five callback timings** (`callbacks/interface.go:114-134`):
| Constant | When | Input/Output |
|---|---|---|
| `TimingOnStart` | Before component runs | `CallbackInput` (value) |
| `TimingOnEnd` | After component succeeds | `CallbackOutput` (value) |
| `TimingOnError` | Component returns error | `error` |
| `TimingOnStartWithStreamInput` | Component receives stream input (Collect/Transform) | `*StreamReader[CallbackInput]` (copy) |
| `TimingOnEndWithStreamOutput` | Component produces stream output (Stream/Transform) | `*StreamReader[CallbackOutput]` (copy) |

**`TimingChecker`** (`callbacks/interface.go:136-145`): an optional interface on handlers. The framework calls `Needed(ctx, info, timing)` before each timing to decide whether to allocate stream copies and goroutines. Handlers built with `HandlerBuilder` (`callbacks/handler_builder.go`) automatically implement `TimingChecker` — `Needed` returns `true` only for the functions the user explicitly set.

**Internal engine** (`internal/callbacks/inject.go`): the function `On[T]` (line 74) is the single dispatch entry point for all timings. It:
1. Pulls the `manager` from context.
2. Collects handlers that need this timing (filtered via `TimingChecker`).
3. Calls the `Handle[T]` function with the handlers list.
4. Returns the updated context and (possibly copied) value.

Stream handlers (`OnStartWithStreamInputHandle`, `OnEndWithStreamOutputHandle`, lines 163-193) use `OnWithStreamHandle` (line 143), which calls `cpy(len(handlers) + 1)` to create N+1 copies of the input stream — N for the handlers and 1 for the actual consumer. **Each handler receives its own private copy** and must close it.

**Global vs. per-invocation handlers** (`internal/callbacks/manager.go`): `manager` holds both `globalHandlers` (set once at program init via `AppendGlobalHandlers`) and `handlers` (set per graph invocation via `compose.WithCallbacks`). Global handlers run first (higher priority for instrumentation that must observe everything).

### 4.5 Callback wrapping in compose (`compose/utils.go`)

`runWithCallbacks` (line 100) is the generic wrapper:

```go
func runWithCallbacks[I, O, TOption any](r func(...) (O, error),
    onStart on[I], onEnd on[O], onError on[error]) func(...) (O, error) {
    return func(ctx context.Context, input I, opts ...TOption) (output O, err error) {
        ctx, input = onStart(ctx, input)
        output, err = r(ctx, input, opts...)
        if err != nil {
            ctx, err = onError(ctx, err)
            return output, err
        }
        ctx, output = onEnd(ctx, output)
        return output, nil
    }
}
```

The four mode-specific wrappers are:

- `invokeWithCallbacks`: onStart (value) → Invoke → onEnd (value)
- `streamWithCallbacks`: onStart (value) → Stream → onEndWithStreamOutput (stream)
- `collectWithCallbacks`: onStartWithStreamInput (stream) → Collect → onEnd (value)
- `transformWithCallbacks`: onStartWithStreamInput (stream) → Transform → onEndWithStreamOutput (stream)

`initGraphCallbacks` / `initNodeCallbacks` (lines 152-207) set up `RunInfo` in context before each node executes. They check `executorMeta.component` for the component category and `executorMeta.componentImplType` for the implementation type. Per-node callbacks can be scoped via `compose.WithCallbacks` with `NodePath` filtering (lines 188-200).

### 4.6 Component-to-graph-node conversion (`compose/component_to_graph_node.go`)

Every component type has a dedicated conversion function. For example, `toChatModelNode` (line 93):

```go
func toChatModelNode(node model.BaseChatModel, opts ...GraphAddNodeOpt) (*graphNode, *graphAddNodeOpts) {
    return toComponentNode(
        node,
        components.ComponentOfChatModel,
        node.Generate,  // Invoke
        node.Stream,    // Stream
        nil,            // Collect (not implemented)
        nil,            // Transform (not implemented)
        opts...)
}
```

`toComponentNode` (line 29) does three things:
1. Calls `parseExecutorInfoFromComponent` (`compose/graph_node.go:151-163`) to build `executorMeta` — sets `component`, `componentImplType` (from `components.GetType`), and `isComponentCallbackEnabled` (from `components.IsCallbacksEnabled`).
2. Calls `runnableLambda` with the four method pointers, which invokes `newRunnablePacker` — filling in the missing Collect/Transform via automatic downgrade (Collect via `collectByStream`, Transform via `transformByStream`).
3. Calls `toNode` to wrap everything into a `graphNode` struct.

**Critical detail on `isComponentCallbackEnabled`**: the `runnableLambda` call passes `!meta.isComponentCallbackEnabled` as the `enableCallback` flag (line 41). This flag is inverted — if a component says "I handle my own callbacks" (via `callbacks.Checker`), the compose layer does NOT wrap the component's methods with `runWithCallbacks`. Otherwise, compose wraps them. This prevents double-firing of callbacks.

The passthrough node (`toPassthroughNode`, line 186) is a zero-cost identity node — its `Invoke` returns input unchanged and its `Transform` returns the input stream unchanged. It is used by graph compilation to insert synthetic nodes for routing.

### 4.7 Callback context lifecycle (`internal/callbacks/inject.go`)

The `On[T]` dispatch function (line 74) has a subtle `start` parameter:

- When `start = true` (first timing of an execution): the manager's `runInfo` is consumed and stored into context via `CtxRunInfoKey`. This prevents re-dispatch on nested calls.
- When `start = false` (subsequent timings): if the manager still has `runInfo` (because `EnsureRunInfo` was called by a sub-component), it is used; otherwise it is fetched from `CtxRunInfoKey` in context.

This mechanism ensures that when a sub-graph node calls `EnsureRunInfo` to provide its own `RunInfo`, subsequent callback timings inside that node use the sub-graph's `RunInfo`, not the parent's.

## 5. Patterns and Examples

### Pattern 1: Component with only Invoke

A Retriever implements only `Retrieve` (which maps to Invoke). Eino wraps it:

```
Invoke  → native Retrieve
Stream  → invokeByStream      // call Retrieve, wrap result as array stream
Collect → collectByInvoke      // concat input stream, call Retrieve
Transform → transformByInvoke  // concat input stream, call Retrieve, wrap as array stream
```

The graph user can call `.Stream()` on this node and it works, even though the retriever never implemented streaming.

### Pattern 2: ChatModel with Invoke + Stream

`BaseChatModel` has both `Generate` (Invoke) and `Stream`. The missing Collect and Transform are derived:

```
Collect → collectByStream   // concat input, Stream, concat output
Transform → transformByStream // concat input, Stream
```

This means a ChatModel node can receive stream input (from an upstream streaming prompt chain) and produce stream output. The concat in `collectByStream` is the price of bridging non-streaming input to streaming output.

### Pattern 3: HandlerBuilder for per-timing callbacks

```go
handler := callbacks.NewHandlerBuilder().
    OnStartFn(func(ctx context.Context, info *callbacks.RunInfo, input callbacks.CallbackInput) context.Context {
        if mi := model.ConvCallbackInput(input); mi != nil {
            log.Printf("[%s] messages: %d", info.Name, len(mi.Messages))
        }
        return ctx
    }).
    OnEndWithStreamOutputFn(func(ctx context.Context, info *callbacks.RunInfo, output *schema.StreamReader[callbacks.CallbackOutput]) context.Context {
        defer output.Close() // REQUIRED
        for {
            chunk, err := output.Recv()
            if errors.Is(err, io.EOF) { break }
            // accumulate token counts from stream
        }
        return ctx
    }).
    Build()
```

The `HandlerBuilder.Build()` call (`callbacks/handler_builder.go:161`) returns a `handlerImpl` that implements `TimingChecker` — `Needed` returns true only for `OnStartFn` and `OnEndWithStreamOutputFn` in this example, skipping the other three timings with zero overhead.

### Pattern 4: Global handlers for cross-cutting concerns

```go
func init() {
    callbacks.AppendGlobalHandlers(
        tracingHandler,   // always runs first
        metricsHandler,   // always runs second
    )
}
```

Global handlers are processed BEFORE per-invocation handlers in the `On` dispatch function (`internal/callbacks/inject.go:95`). This means distributed tracing sees every component invocation regardless of what handlers the application code passes.

### Pattern 5: Path-scoped callbacks

```go
r.Invoke(ctx, input,
    compose.WithCallbacks(
        compose.WithNodePath("tool_node", "calculator"),
    ).DesignateHandler(calcHandler),
)
```

The `initNodeCallbacks` function (`compose/utils.go:190-200`) matches the `NodePath` against the current node key. If the path matches, the handler is appended; otherwise the node reuses the existing handlers from context via `ReuseHandlers`.

## 6. Common Pitfalls

### Pitfall 1: Leaking stream readers in callback handlers

The most dangerous bug. `OnEndWithStreamOutput` and `OnStartWithStreamInput` handlers receive a **private copy** of the stream. If the handler does not close this copy, `parentStreamReader.close()` (`schema/stream.go:868-881`) never increments its `closedNum`, so the original stream is never closed, and the goroutine in `toStream()` (line 747-778) leaks forever.

**Fix**: always `defer sr.Close()` in stream handlers. The `HandlerBuilder` docstrings (`callbacks/handler_builder.go:141, 153`) explicitly warn about this.

### Pitfall 2: Assuming callback handler ordering

Context does NOT flow between different handlers. Each handler's `OnStart` → `OnEnd` chain is independent. If handler A sets a value via `context.WithValue` and handler B expects to read it, handler B will get nil.

**Fix**: use a single handler for serial state, or use global state that is safe for concurrent access.

### Pitfall 3: Mutating callback input/output values

The `OnStart` and `OnEnd` handlers receive pointers to the same objects used by other nodes and handlers (`direct assignment, not a deep copy` — `callbacks/interface.go:82-84`). Mutating these values causes data races in concurrent graph execution.

### Pitfall 4: Forgetting to register stream concat functions

If a component only implements `Stream()` and the user calls `Invoke()`, the runtime uses `concatStreamReader` to collapse the stream. If no concat function was registered for the chunk type, `internal.ConcatItems` falls back to the last chunk. For types that do not follow the "last chunk is the final answer" semantics (e.g., a streaming numeric counter where each chunk is a delta), this produces garbage.

**Fix**: call `compose.RegisterStreamChunkConcatFunc` at init for custom chunk types.

### Pitfall 5: Double-firing callbacks through nested graph and component

If a ChatModel implementation self-implements callback dispatch via `callbacks.Checker`, but the compose layer also wraps it with `runWithCallbacks`, each timing fires twice. Eino prevents this via `isComponentCallbackEnabled` in `executorMeta` — but only if the component correctly implements `Checker`.

### Pitfall 6: Stream copy before Recv

`StreamReader.Copy()` **must** be called before the first `Recv()`. The original reader becomes unusable after Copy. The `copyStreamReaders` function (`schema/stream.go:792-821`) replaces the original reader with a `parentStreamReader` that lazily fetches from the source — but this replacement is irreversible. If the caller tries to `Recv()` on the original after Copy, it panics.

## 7. What Rive Can Learn

**Stable execution addresses, not function calls.** Eino's `RunInfo` (Name + Type + Component) forms a stable identity for every execution point in the graph. Rive could adopt a similar structured identity: `{dispatchId, workerId, executionAttempt}` rather than relying on natural language descriptions to identify nodes.

**Stream lifecycle = resource lifecycle.** Eino treats stream readers as resources with mandatory close semantics. The callback engine creates N+1 copies and assigns exactly one to each consumer. Rive's dispatch protocol could benefit from a similar model for streaming data between workers: explicit copy-on-fork, mandatory close acknowledgments, and automatic cleanup when a consumer fails to close.

**Automatic capability downgrade as a migration strategy.** Eino's downgrade matrix allows components to evolve independently — a component can add `Stream()` support without updating every caller. Rive workers could similarly advertise capability levels (`supports_streaming`, `supports_resume`) and the protocol could automatically bridge between different capability levels.

**TimingChecker as cost model.** Eino skips stream copies and allocations when a handler doesn't need a timing. Rive could apply the same idea to observability: workers declare which lifecycle events they want to observe, and the protocol omits instrumentation overhead for unsubscribed events.

**Component-agnostic scheduling.** Eino's scheduler never sees `ChatModel` or `Retriever` — it only sees `composableRunnable`. This allows new component types to be added without touching the scheduler. Rive could similarly define a uniform work-unit interface so the scheduler does not need to understand SQL queries, Python scripts, or API calls.
