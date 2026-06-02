# 04 — Checkpoint / Interrupt / Resume

## 1. Facing Problem

Eino's execution graphs can be arbitrarily deep: `Graph` nests sub-graphs, `Agent` wraps `Graph`, `ToolsNode` fans out multiple parallel tool calls, and a `Lambda` node may spin up an entire standalone `Runnable`. When any component in this deeply nested mesh decides to pause — because it needs human input, hits a rate limit, or a tool call requires approval — the runtime must:

- **Save exact execution state** so the graph can be restarted from the precise interruption point, not from scratch.
- **Uniquely identify every interrupt point** across the entire call tree, even when the same tool is called twice with different call IDs (`tool:my_tool:call_1` vs `tool:my_tool:call_2`).
- **Prevent parent graphs from swallowing child interrupts** — a sub-graph interrupt must propagate up to the root caller.
- **Materialize stream data** before checkpointing so that resumption has deterministic, non-ephemeral inputs.
- **Support targeted resume** — the user may choose to resume only one of several parallel interrupt points, leaving others paused.

An ad-hoc approach (save a stack trace, re-run from the top) fails because: component invocations are side-effectful (LLM API calls, database writes), re-running would re-execute already-completed work, and there is no guarantee the same execution path would be taken again without the exact same checkpoint state.

## 2. Why It Is Hard

The difficulty is not saving state — it is saving the **right** state at the **right** identity in a **hierarchical, concurrent, heterogeneous** runtime.

### 2.1 Hierarchical Identity

Execution points are not flat function calls. They form a tree:

```text
runnable:root;node:sub_graph_a;node:sub_graph_b;node:tools;tool:interrupt_tool:tool_call_123
                                            ^^^^^^^^^^^^^^^  ^^^^^^^  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                                            graph node        tools    tool name : tool call id
```

Without a stable, hierarchical address system, the runtime cannot:
- Distinguish tool call `#1` from tool call `#2` running in parallel on the same `ToolsNode`.
- Route resume data to the correct leaf node when the user says "continue tool call `#3`".
- Let a `Lambda` node that wraps a standalone `Graph` correctly prepend its own address segments to the inner graph's addresses.

### 2.2 Concurrency

Eino uses a Pregel-style (or DAG-style) execution model where multiple nodes can run simultaneously. When a `ToolsNode` runs 3 tools in parallel and 2 of them interrupt, the checkpoint must:
- Record the outputs of the 1 completed tool.
- Record the interrupt signals of the 2 paused tools.
- Store sub-graph checkpoints for any sub-graph nodes that also interrupted.

### 2.3 Stream Materialization

Eino supports streaming (`Stream`, `Transform`, `Collect`) as first-class execution modes. A `StreamReader` is an ephemeral, one-shot consumer — once consumed, it is gone. Before checkpointing, all stream values in channels and inputs must be materialized into concrete values (`convertCheckPoint` in `compose/checkpoint.go:272`). On resumption, those concrete values must be re-wrapped into `StreamReader` instances (`restoreCheckPoint` in `compose/checkpoint.go:291`) so that downstream stream-mode nodes receive the correct types.

### 2.4 Legacy Interoperability

Eino's original interrupt mechanism (`InterruptAndRerun`, `NewInterruptAndRerunErr`) was a flat sentinel error with no address or unique ID. The modern system (`Interrupt`, `StatefulInterrupt`, `CompositeInterrupt`) must coexist with legacy code via `WrapInterruptAndRerunIfNeeded` (`compose/interrupt.go:78`) and the deprecated path through `CompositeInterrupt` (`compose/interrupt.go:181-213`).

### 2.5 Composite Component Duality

A `ToolsNode` or a `Lambda` that wraps a sub-`Graph` must serve two roles simultaneously:
1. **Self-targeting**: The composite node itself might be the resume target (e.g., to modify its own internal state).
2. **Conduit**: If the resume target is a descendant, the composite node must re-execute its children to let the resume context flow down — it must not consume the signal.

This duality is encoded in `GetResumeContext`'s return value `isResumeTarget` (`compose/resume.go:77`): `true` with `hasData = false` means "a descendant is the target, propagate"; `true` with `hasData = true` means "you are the direct target."

## 3. Design Idea

Eino's design rests on four pillars:

### 3.1 Address System (`internal/core/address.go`)

Every execution context carries a hierarchical `Address` — a `[]AddressSegment` — stored in the Go context under `addrCtxKey{}`. Each `AddressSegment` has:

```go
// internal/core/address.go:69-77
type AddressSegment struct {
    ID    string              // node key, tool name, runnable name
    Type  AddressSegmentType  // "node", "tool", "runnable"
    SubID string              // tool call ID for disambiguation
}
```

Three segment types are defined (`compose/interrupt.go:275-286`):
- `AddressSegmentNode` (`"node"`) — graph nodes added via `AddLambdaNode`, `AddGraphNode`, etc.
- `AddressSegmentTool` (`"tool"`) — tool invocations within a `ToolsNode`.
- `AddressSegmentRunnable` (`"runnable"`) — standalone `Graph`/`Workflow`/`Chain` instances (created by `WithGraphName`).

The `String()` method (`internal/core/address.go:35-53`) produces a stable, joinable representation: `runnable:root;node:sub_a;tool:my_tool:call_1`.

`AppendAddressSegment` (`internal/core/address.go:118-187`) is called whenever the runtime enters a new execution scope (graph node, tool call, sub-runnable). It:
1. Builds the new hierarchical address by extending the parent's address.
2. Checks `globalResumeInfo` (the global resume target map) to see if this new address matches a stored interrupt state or resume data.
3. Sets `interruptState`, `isResumeTarget`, and `resumeData` on the new `addrCtx` accordingly.

A critical design choice: `isResumeTarget` is set to `true` not only when the address **exactly** matches a resume target, but also when a resume target exists that is a **descendant** of the current address (`internal/core/address.go:175-183`). This is what enables composite components to act as conduits — they know a child needs resuming.

### 3.2 InterruptSignal Tree (`internal/core/interrupt.go`)

At the heart of the interrupt mechanism is `InterruptSignal` (`internal/core/interrupt.go:43-49`):

```go
type InterruptSignal struct {
    ID             string               // UUID
    Address        Address              // hierarchical address
    InterruptInfo  InterruptInfo        // { Info any, IsRootCause bool }
    InterruptState InterruptState       // { State any, LayerSpecificPayload any }
    Subs           []*InterruptSignal   // child signals (for composite/virtual nodes)
}
```

The `Subs` field makes this a tree, not a flat list. A `CompositeInterrupt` (e.g., from a `ToolsNode` with 3 parallel tool calls where 2 interrupt) produces a parent signal with `Subs: [signal_tool_1, signal_tool_2]`. This tree can then be serialized into the checkpoint and reconstructed on resume.

Key conversion functions:
- `SignalToPersistenceMaps` (`internal/core/interrupt.go:327-349`): Flattens the tree into two maps (`id2addr`, `id2state`) for checkpoint storage.
- `ToInterruptContexts` (`internal/core/interrupt.go:254-294`): Converts the tree into a flat list of user-facing `InterruptCtx` objects (root causes only), each with a `Parent` pointer for tree traversal.
- `FromInterruptContexts` (`internal/core/interrupt.go:198-243`): Reconstructs the tree from a flat list of `InterruptCtx` objects — used when bridging across execution environments (e.g., ADK agent tools).

### 3.3 Checkpoint Persistence (`compose/checkpoint.go`)

The `checkpoint` struct (`compose/checkpoint.go:106-117`) captures the full execution snapshot:

```go
type checkpoint struct {
    Channels       map[string]channel          // in-flight channel values
    Inputs         map[string]any              // pending task inputs
    State          any                         // graph-level state
    SkipPreHandler map[string]bool             // pre-handler skip flags
    RerunNodes     []string                    // nodes that need re-run
    SubGraphs      map[string]*checkpoint      // nested sub-graph checkpoints
    InterruptID2Addr  map[string]Address       // flattened interrupt ID → address
    InterruptID2State map[string]core.InterruptState // flattened interrupt ID → state
}
```

Key design decisions:
- **Nested SubGraphs**: Each sub-graph node's checkpoint is stored in `SubGraphs[nodeKey]`, allowing recursive nesting. On resume, `forwardCheckPoint` (`compose/checkpoint.go:157-168`) plucks the nested checkpoint and injects it into the child context, deleting it from the parent to "forward only once."
- **Stream Conversion**: `convertCheckPoint` and `restoreCheckPoint` (`compose/checkpoint.go:272-307`) handle the bidirectional conversion between stream and non-stream values via `streamConverter`, backed by registered `streamConvertPair` entries (`compose/checkpoint.go:309-373`).
- **State Modification**: `WithStateModifier` (`compose/checkpoint.go:100`) allows injecting a `StateModifier` callback invoked during checkpoint read/write for migration or runtime augmentation.
- **Checkpoint Migration**: `MigrateCheckpointState` (`compose/checkpoint.go:231-244`) is an advanced utility for upgrading checkpoint schemas across framework versions.

### 3.4 Resume Context Injection

The resume data flow has three stages:

**Stage 1 — User provides resume targets**: `Resume(ctx, id)` / `ResumeWithData(ctx, id, data)` / `BatchResumeWithData(ctx, map)` (`compose/resume.go:94-121`) inject a `globalResumeInfo` into the context, mapping interrupt IDs to resume data.

**Stage 2 — Checkpoint restores interrupt state**: When the graph loads a checkpoint on resume, `setCheckPointToCtx` (`compose/checkpoint.go:145-148`) calls `core.PopulateInterruptState` (`internal/core/address.go:271-321`) to merge the checkpoint's `InterruptID2Addr` and `InterruptID2State` maps into the existing `globalResumeInfo`. This is how components learn they were interrupted in a prior run.

**Stage 3 — Address matching distributes state**: As the graph creates tasks, `AppendAddressSegment` (step 3.1 above) matches the new address against the `globalResumeInfo` maps, setting `interruptState` and `isResumeTarget` on each leaf's `addrCtx`.

Components read this state using two public APIs (`compose/resume.go:32-78`):
- `GetInterruptState[T](ctx)` — "Was I interrupted before? Here's my saved state."
- `GetResumeContext[T](ctx)` — "Am I the resume target? Here's the resume data."

## 4. Source Walkthrough

### 4.1 Key Files and Their Roles

| File | Role |
|------|------|
| `compose/checkpoint.go` | `checkpoint` struct definition, serialization, stream conversion, `MigrateCheckpointState`, `WithCheckPointStore`, `WithCheckPointID`, `WithForceNewRun` |
| `compose/interrupt.go` | Public interrupt APIs: `Interrupt`, `StatefulInterrupt`, `CompositeInterrupt`, `WrapInterruptAndRerunIfNeeded`; `InterruptInfo` struct; `subGraphInterruptError`; `ExtractInterruptInfo` |
| `compose/resume.go` | Public resume APIs: `GetInterruptState`, `GetResumeContext`, `Resume`, `ResumeWithData`, `BatchResumeWithData`, `AppendAddressSegment`, `GetCurrentAddress` |
| `internal/core/address.go` | `Address` / `AddressSegment` types, `AppendAddressSegment` (the core context builder), `PopulateInterruptState`, `BatchResumeWithData`, `GetNextResumptionPoints` |
| `internal/core/interrupt.go` | `InterruptSignal` tree, `core.Interrupt`, `SignalToPersistenceMaps`, `ToInterruptContexts`, `FromInterruptContexts`, `CheckPointStore` interface |
| `internal/core/resume.go` | `GetInterruptState`, `GetResumeContext` implementations, `getRunCtx` |
| `compose/graph_run.go` | `handleInterrupt` (line 502), `handleInterruptWithSubGraphAndRerunNodes` (line 598), `resolveInterruptCompletedTasks` (line 457), `restoreCheckPointState` (line 382), `restoreTasks` (line 777), `createTasks` (line 735) |
| `compose/graph_call_options.go` | `WithGraphInterrupt` (external cancellation, line 72) |
| `compose/tool_node.go` | `ToolsNode` composite interrupt handling for parallel tool calls |

### 4.2 Execution Flow: Interrupt

```
1. Node returns InterruptSignal (via Interrupt/StatefulInterrupt/CompositeInterrupt)
2. graph_run.resolveInterruptCompletedTasks (line 457) detects the signal
   - SubGraphInterruptError → stored in tempInfo.subGraphInterrupts[nodeKey]
   - InterruptSignal with IsRootCause → collected in tempInfo.signals
   - Rerun node interrupt → stored in tempInfo.interruptRerunNodes
3. graph_run.handleInterrupt (line 502) or handleInterruptWithSubGraphAndRerunNodes (line 598):
   a. Constructs checkpoint with Channels, Inputs, State, SkipPreHandler, RerunNodes, SubGraphs
   b. Calls core.Interrupt(ctx, info, nil, tempInfo.signals) → builds InterruptSignal tree
   c. Calls SignalToPersistenceMaps → populates cp.InterruptID2Addr, cp.InterruptID2State
   d. Calls convertCheckPoint → materializes streams
   e. If subgraph: returns subGraphInterruptError{CheckPoint: cp, Info: intInfo}
   f. If root: persists cp to CheckPointStore, returns interruptError{Info: intInfo}
```

### 4.3 Execution Flow: Resume

```
1. User calls ResumeWithData(ctx, id, data) → injects globalResumeInfo into ctx
2. Graph.Invoke(ctx, input, WithCheckPointID(id)) → loads checkpoint from store
3. setCheckPointToCtx (checkpoint.go:145):
   a. Calls PopulateInterruptState → merges checkpoint interrupt maps into globalResumeInfo
   b. Puts checkpoint into ctx under checkPointKey{}
4. graph_run.restoreCheckPointState (line 382):
   a. Reads runCtx.isResumeTarget and runCtx.resumeData
   b. If targeted with data, overwrites checkpoint state
   c. Calls convertCheckPoint → materializes streams in checkpoint
5. graph_run.run() (line 109): restores tasks from checkpoint, re-executes graph
6. createTasks (line 735): for each new task:
   a. Calls forwardCheckPoint for sub-graph nodes
   b. Calls AppendAddressSegment → builds hierarchical address → matches resume data
7. Component receives ctx with interruptState + resumeData set on its addrCtx
8. Component calls GetInterruptState → sees wasInterrupted=true, retrieves state
9. Component calls GetResumeContext → sees isResumeTarget=true, retrieves data
```

### 4.4 Sub-graph Interrupt Propagation

```
Sub-graph Node → InterruptSignal
    ↓
Sub-graph runner.handleInterrupt
    → returns subGraphInterruptError{CheckPoint, Info, signal}
    ↓
Parent graph.resolveInterruptCompletedTasks
    → detects via isSubGraphInterrupt (interrupt.go:329)
    → stores in tempInfo.subGraphInterrupts[nodeKey]
    → collects signal into tempInfo.signals
    ↓
Parent graph.handleInterruptWithSubGraphAndRerunNodes
    → cp.SubGraphs[nodeKey] = subGraphInterruptError.CheckPoint
    → intInfo.SubGraphs[nodeKey] = subGraphInterruptError.Info
    → calls core.Interrupt with accumulated tempInfo.signals (including sub's)
    → builds unified InterruptSignal tree with correct parent-child relationships
```

On resume, the nested checkpoint is forwarded via `forwardCheckPoint` (`checkpoint.go:157-168`):
```go
func forwardCheckPoint(ctx context.Context, nodeKey string) context.Context {
    cp := getCheckPointFromCtx(ctx)
    if subCP, ok := cp.SubGraphs[nodeKey]; ok {
        delete(cp.SubGraphs, nodeKey) // only forward once
        return context.WithValue(ctx, checkPointKey{}, subCP)
    }
    return context.WithValue(ctx, checkPointKey{}, (*checkpoint)(nil))
}
```

## 5. Patterns and Examples

### 5.1 Simple Interrupt and Resume

A lambda node that interrupts with typed state and resumes with data:

```go
type myState struct{ OriginalInput string }
type myData struct{ Message string }

lambda := InvokableLambda(func(ctx context.Context, input string) (string, error) {
    wasInterrupted, hasState, state := GetInterruptState[*myState](ctx)
    if !wasInterrupted {
        return "", StatefulInterrupt(ctx,
            map[string]any{"reason": "need approval"},
            &myState{OriginalInput: input},
        )
    }
    // Resumed
    isResume, hasData, data := GetResumeContext[*myData](ctx)
    if isResume && hasData {
        return "resumed: " + data.Message, nil
    }
    return "", StatefulInterrupt(ctx, "still waiting", state)
})
```

Caller extracts interrupt info and resumes:

```go
// First run → interrupts
out, err := graph.Invoke(ctx, "input", WithCheckPointID("cp1"))
info, _ := ExtractInterruptInfo(err)
id := info.InterruptContexts[0].ID

// Resume
ctx2 := ResumeWithData(context.Background(), id, &myData{Message: "go ahead"})
out, err = graph.Invoke(ctx2, "", WithCheckPointID("cp1"))
```

### 5.2 Composite Component with Multiple Sub-processes

A "batch" lambda that fans out N parallel sub-processes, each with its own address segment and independent interrupt/resume cycle:

```go
const PathProcess AddressSegmentType = "process"
processIDs := []string{"p0", "p1", "p2"}

batchLambda := InvokableLambda(func(ctx context.Context, _ string) (map[string]string, error) {
    _, _, batchState := GetInterruptState[*batchState](ctx)
    var errs []error
    for _, id := range processIDs {
        if _, done := batchState.Results[id]; done { continue }
        subCtx := AppendAddressSegment(ctx, PathProcess, id)
        res, err := runSubProcess(subCtx, id)
        if err != nil { errs = append(errs, err) }
        else { batchState.Results[id] = res }
    }
    if len(errs) > 0 {
        return nil, CompositeInterrupt(ctx, nil, batchState, errs...)
    }
    return batchState.Results, nil
})
```

The key pattern: each sub-process gets its own address segment via `AppendAddressSegment`, its own interrupt state, and the parent uses `CompositeInterrupt` to bundle all sub-errors into a tree. The caller sees 3 flat `InterruptCtx`s (root causes) with a shared parent.

### 5.3 Graph-within-Lambda Interrupt Propagation

When a `Lambda` node wraps a standalone compiled `Graph`, the inner graph's `runnable` address segment is automatically prepended (`compose/resume.go:131-133`). The lambda acts as a composite node:

```go
compositeLambda := InvokableLambda(func(ctx context.Context, input string) (string, error) {
    output, err := compiledInnerGraph.Invoke(ctx, input, WithCheckPointID("inner-cp"))
    if err != nil {
        if _, isInterrupt := ExtractInterruptInfo(err); isInterrupt {
            // Pass the inner graph's interrupt up, with the lambda's own address
            return "", CompositeInterrupt(ctx, "composite interrupt from lambda", nil, err)
        }
        return "", err
    }
    return output, nil
})
```

The resulting address: `runnable:root;node:composite_lambda;runnable:inner;node:inner_lambda`

### 5.4 ReAct-style Re-entry with Tool Interrupts

A common pattern where a `ChatModel` → `ToolsNode` loop interrupts on tool calls, resumes, and the model may call the same tool again — the re-entrant call must have a fresh context (not marked as interrupted). Tests demonstrate this in `compose/resume_test.go:628` (`TestReentryForResumedTools`): on the second invocation, `call_1` is resumed (wasInterrupted=true, isResumeTarget=true), `call_2` re-interrupts (wasInterrupted=true, isResumeTarget=false), and on the third invocation the model creates a new `call_3` (wasInterrupted=false, isResumeTarget=false).

### 5.5 Checkpoint Migration

When graph state types change, use `MigrateCheckpointState` to transform old checkpoints:

```go
newBytes, err := MigrateCheckpointState(oldBytes, serializer, func(state any) (any, bool, error) {
    if old, ok := state.(*OldStateType); ok {
        return old.ToNewType(), true, nil
    }
    return state, false, nil
})
```

The migrate function is applied recursively to `checkpoint.State` and all `SubGraphs`' states (`compose/checkpoint.go:247-269`).

## 6. Common Pitfalls

### 6.1 Forgetting to Call `WrapInterruptAndRerunIfNeeded` for Legacy Errors

When using the deprecated `InterruptAndRerun` or `NewInterruptAndRerunErr` inside a composite component, the errors must be wrapped with `WrapInterruptAndRerunIfNeeded` (`compose/interrupt.go:78`) before being passed to `CompositeInterrupt`. Without wrapping, the errors lack address context and the interrupt point's address will be empty.

### 6.2 Not Re-interrupting When `isResumeTarget` is False

In an explicit targeted resume scenario, if `GetResumeContext` returns `isResumeTarget = false`, the component **must** re-interrupt. Otherwise, the state is lost and the graph continues as if no interruption occurred — potentially producing incorrect results or failing silently. The pattern is:

```go
isResume, _, _ := GetResumeContext[myData](ctx)
if !isResume {
    return "", StatefulInterrupt(ctx, "still waiting", state) // re-interrupt
}
```

### 6.3 Stream Leak in Callback Handlers

When writing callbacks for streaming contexts, the `OnStartWithStreamInput` and `OnEndWithStreamOutput` handlers receive `StreamReader` copies. These copies **must** be closed, otherwise goroutine/memory leaks occur. The pattern is:

```go
func (h *myHandler) OnStartWithStreamInput(ctx context.Context, info *callbacks.RunInfo,
    input *schema.StreamReader[callbacks.CallbackInput]) context.Context {
    input.Close()
    return ctx
}
```

### 6.4 Assuming Sequential Resumption

Parallel interrupts (e.g., 3 tool calls all interrupt) can be resumed independently and in any order. Components should not assume that the first resume processes all interrupt points — each must be targeted explicitly, and the batch node must track which sub-processes have completed. See `compose/resume_test.go:375` (`TestMultipleInterruptsAndResumes`) for the correct pattern with `batchState.Results` tracking.

### 6.5 Sub-graph State Not Being Updated

When a sub-graph node interrupts, its state is inside `info.SubGraphs[nodeKey].State`, not `info.State`. Callers that only check `info.State` will miss sub-graph state changes. Similarly, when resuming, the sub-graph's state is in `cp.SubGraphs[nodeKey].State` and is forwarded via `forwardCheckPoint` — the parent must not overwrite it.

### 6.6 Not Registering Types for Serialization

Custom types used in interrupt state or resume data must be registered with `schema.RegisterName[T](name)` or `schema.Register[T]()`, otherwise checkpoint serialization will fail. Examples in `compose/checkpoint_test.go` use `schema.Register[testStruct]()` and `schema.Register[*testPersistRerunInputState]()`.

### 6.7 Confusing `GetInterruptState` with `GetResumeContext`

`GetInterruptState` answers "was I interrupted before / what was my state?" — it returns true during any resumed run, regardless of whether this specific component is the resume target. `GetResumeContext` answers "am I the explicit resume target / what data was sent to me?" — it returns true only when this specific address was targeted. A component that was interrupted but is not the current resume target must re-interrupt.

## 7. What Rive Can Learn

### 7.1 Execution Point Identity Needs to Be Structural, Not Descriptive

Eino's address is a deterministic chain of typed, ID'd segments built from the graph topology (`compose/resume.go:123-140`). It is not a natural language description or a stack trace. For Rive's dispatch/resume model, this means: a resume point should be identified by `run_id + node_id + dispatch_id + subdispatch_id`, not "the worker that got stuck on the token limit."

### 7.2 Composite Dispatch-as-Conduit Pattern

Eino's `isResumeTarget` with `hasData = false` for descendants is a clean pattern for composite components. Rive dispatches that fan out sub-dispatches (equivalent to `ToolsNode` or batch `Lambda`) need the same duality: the dispatch itself can be a resume target, and it must transparently forward resume signals to its children.

### 7.3 Flat User-Facing Context with Tree State

Eino's `ToInterruptContexts` produces a flat list of root causes (the leaves the user cares about), while `InterruptSignal.Subs` retains the tree structure for correct state persistence and reconstruction. Rive should similarly expose a flat "things to resume" view to humans while keeping the parent-child tree internally for correct state propagation.

### 7.4 Stream/State Duality Is a Migration Concern

Eino's `convertCheckPoint` / `restoreCheckPoint` pattern for stream materialization is specific to in-process streaming, but the principle generalizes: any ephemeral, one-shot data (streams, connection pools, in-flight RPCs) must be materialized before checkpointing and reattached on resume. Rive dispatches that involve streaming transport should identify analogous ephemeral state that cannot survive a checkpoint boundary.

### 7.5 Checkpoint Migration as a Framework-Level Concern

Eino's `MigrateCheckpointState` (`compose/checkpoint.go:231-244`) provides a recursive state migrator that upgrades checkpoint schemas without user code changes. For Rive, long-lived dispatches that persist state (e.g., minutes-to-hours) need the same capability: a framework-level hook that transforms old state shapes into new ones, applied automatically on resume.
