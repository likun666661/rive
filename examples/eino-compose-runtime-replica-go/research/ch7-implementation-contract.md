# D1 Chapter 7 Implementation Contract: ReAct / Host Multi-Agent

> **Contract ID**: ch7-impl-contract-d1
> **Input**: Chapter manual (`07-agent-flow-react-multiagent.md`) + Research contract (`ch7-agent-flow-contract.md`) + existing `compose/` module audit
> **Purpose**: Assign concrete file ownership, API names, test specifications for Chapter 7 — ReAct agent core (I1) and Host Multi-Agent skeleton (I2), with tests/docs/examples (T1).
> **Constraint**: This is a contract document only. Do not implement production code from this node.
> **Date**: 2026-06-07

> **Final-scope note**: this file records the worker implementation proposal used during the Rive DAG run. The merged educational subset is intentionally narrower than several proposal sections: it keeps ReAct in `agent/react.go`, keeps Host Multi-Agent in `compose/multiagent.go`, removes the unused `agent/react_option.go` / `agent/react_callback.go` surfaces, and treats `WithMessageFuture` as an excluded production feature. See `README.md` and `FINAL_SUMMARY.md` for the final public contract.

---

## 1. Overall Scope

Chapter 7 demonstrates how to encode LLM agent patterns as `compose.Graph` builders without a special runtime. Two graph builders are delivered:

| Builder | Description | Owner |
|---------|-------------|-------|
| **ReAct Agent** | `START → ChatModel → (toolCall? Tools → ChatModel : END)` loop with local state, rewriter/modifier, direct return, stream checker | I1 |
| **Host Multi-Agent** | `START → Host → (toolCall? Specialist[...] → Collect → (multi? Summarize : pass) : END)` with specialist-as-tool routing, single/multi-intent branching | I2 |
| **Tests / Docs / Examples** | Integration tests, README updates, example `cmd/` program | T1 |

---

## 1.1 Dependency Map

```
compose/ (existing)
  ├── graph.go            Graph compilation, edges, branches
  ├── generic_graph.go    Graph[I,O], Compile, graphRunnable
  ├── graph_run.go        runner.run(), Pregel loop
  ├── graph_manager.go    chanCall, channelManager, task execution
  ├── graph_node.go       graphNode, compileIfNeeded
  ├── graph_compile.go    CompileOption, WithMaxRunSteps, WithGraphName
  ├── branch.go           GraphBranch, NewGraphBranch, BranchCondition
  ├── address.go          Address, GetCurrentAddress, AppendAddressSegment
  ├── pregel.go           pregelChannel, AnyPredecessor
  ├── callbacks.go        Handler, OnStartFn, OnEndFn, RunInfo, CallbackWrapper
  ├── chatmodel.go        Message, ToolCall, ToolInfo, FakeChatModel, SystemMessage etc.
  ├── schema.go           ToolCall, ToolCallFunction, ToolInfo, ParamsOneOf
  ├── types.go            NodeTriggerMode, ComponentType, RoleType, START, END
  ├── stream.go           StreamReader, stream conversion
  ├── runnable.go         Runnable[I,O], Invoke/Stream/Collect/Transform
  └── event_log.go        EventLog

compose/ (new for Ch7 — state infrastructure, shared by I1+I2)
  └── state.go            WithGenLocalState, ProcessState, graphStateKey (NEW)

agent/ (new package, owned by I1+I2)
  ├── types.go            AgentConfig, state, MultiAgentConfig, Specialist, Host (shared)
  ├── react.go            NewAgent, modelPreHandle, toolsNodePreHandle, buildReturnDirectly (I1)
  ├── react_option.go     WithTools, WithMessageFuture, WithStreamToolCallChecker (I1)
  ├── react_callback.go   cbHandler, address isolation (I1)
  ├── host.go             NewMultiAgent, addSpecialistAgent, addHostAgent, routing (I2)
  ├── host_option.go      Host option functions (I2)
  ├── react_test.go       ReAct agent tests (T1, after I1)
  ├── host_test.go        Host multi-agent tests (T1, after I2)
  └── README.md           Agent layer documentation (T1)
```

---

## 2. STATE INFRASTRUCTURE (`compose/state.go` — NEW)

Both I1 and I2 depend on this. I1 implements it as part of the ReAct foundation.

### 2.1 `compose/state.go` — Graph Local State (I1)

**Owner**: I1
**Estimated LOC**: 50

```go
package compose

import "context"

// stateContextKey is unexported — only compose.WithGenLocalState / ProcessState touch it.
type stateContextKey struct{}

// WithGenLocalState returns a CompileOption that injects a per-run state factory.
// The factory is called once at graph start; the returned state is stored in context.
// All state pre-handlers and ProcessState calls operate on this same state instance.
func WithGenLocalState[T any](fn func(ctx context.Context) *T) CompileOption {
    return &genLocalStateOption[T]{factory: fn}
}

// ProcessState reads and applies a mutation to the graph-local state of type T.
// Call this from within tool implementations or pre-handlers.
// Panics if no state of type T is found in context (implementation error).
func ProcessState[T any](ctx context.Context, fn func(ctx context.Context, s *T) error) error

// GetState retrieves the graph-local state without modifying it.
func GetState[T any](ctx context.Context) (*T, bool)

// SetToolCallID stores the current tool call ID in context for SetReturnDirectly.
func SetToolCallID(ctx context.Context, callID string) context.Context

// GetToolCallID retrieves the current tool call ID from context.
func GetToolCallID(ctx context.Context) string

// WithNodePreHandler returns a CompileOption that registers an input-transforming
// pre-handler on the last-added node. The handler receives (ctx, nodeInput) and
// returns (transformedInput, error). It has access to graph-local state via
// ProcessState/GetState. Multiple registrations stack LIFO.
func WithNodePreHandler(fn func(ctx context.Context, input any) (any, error)) CompileOption
```

**API Contract — Exported Symbols**:

| Symbol | Kind | Location |
|--------|------|----------|
| `WithGenLocalState[T any](fn)` | Generic function → CompileOption | `compose/state.go` |
| `ProcessState[T any](ctx, fn)` | Generic function | `compose/state.go` |
| `GetState[T any](ctx) (*T, bool)` | Generic function | `compose/state.go` |
| `SetToolCallID(ctx, callID) context.Context` | Function | `compose/state.go` |
| `GetToolCallID(ctx) string` | Function | `compose/state.go` |
| `WithNodePreHandler(fn) CompileOption` | Function | `compose/state.go` |

**Implementation Notes**:
- `WithGenLocalState` stores the factory on a compile option. During `runner.run()`, if the option is set, the factory is called and the state is stored in ctx via `context.WithValue`.
- `WithNodePreHandler` adds to an internal set of pre-handlers; these are registered on `graphNode` during `AddNode`. During graph compilation, they're attached to the `chanCall.preHandlers` for the node.
- The existing `handlerPreNodes` (graph.go line 261-267) is executed AFTER task completion as output-post-handler. The new `WithNodePreHandler` runs BEFORE the task action, transforming input. Both must coexist.
- In `graph_run.go:resolveCompletedTasks`, the existing loop at line 212-218 processes output post-handlers. We need to add input pre-handler processing in `taskManager.submit` before calling `actionFn`.

**New fields on compile/graph**:
- `graphCompileOptions.genLocalStateFn` — the state factory
- `graphCompileOptions.nodePreHandlers` — deferred pre-handler registrations (cleared after each `Add*Node`)
- `graphNode.preHandlers []handlerPair` — pre-handlers for this node
- `chanCall.preHandlers` already exists; both input-pre and output-post handlers share this slice. We distinguish them by registration order: input-pre registered first during `AddNode`, output-post registered later during compile.

---

## 3. SHARED TYPES (`agent/types.go` — NEW)

**Owner**: I1 (for ReAct types), extended by I2 (for Host types)
**Estimated LOC**: 80

```go
package agent

import "github.com/rive/eino-compose-runtime-replica-go/compose"

// ——————————————————— ReAct Types ——————————————————

// AgentConfig configures a ReAct agent graph builder.
type AgentConfig struct {
    ChatModel             compose.ChatModel
    ToolsConfig           compose.ToolsNodeConfig
    MaxStep               int
    MessageRewriter       MessageRewriter
    MessageModifier       MessageModifier
    StreamToolCallChecker StreamToolCallChecker
    ToolReturnDirectly    map[string]bool
}

// MessageRewriter modifies state.Messages in-place (persistent across rounds).
type MessageRewriter func(ctx context.Context, msgs []*compose.Message) []*compose.Message

// MessageModifier receives a copy of state.Messages and returns modified messages
// for the current round only (non-persistent).
type MessageModifier func(ctx context.Context, msgs []*compose.Message) []*compose.Message

// StreamToolCallChecker reads the entire stream and returns whether ANY chunk contains a tool call.
type StreamToolCallChecker func(ctx context.Context, sr compose.StreamReader[*compose.Message]) (bool, error)

// reactState is the per-run graph-local state for a ReAct agent.
type reactState struct {
    Messages                 []*compose.Message
    ReturnDirectlyToolCallID string
    MaxStep                  int
    StepCount                int
}

// Agent is the compiled ReAct agent (wraps the graph Runnable).
type Agent struct {
    Runnable compose.Runnable[[]*compose.Message, *compose.Message]
    Graph    *compose.Graph[[]*compose.Message, *compose.Message]
}

// ——————————————————— Host Multi-Agent Types (I2) ——————————————————

// MultiAgentConfig configures a Host Multi-Agent graph builder.
type MultiAgentConfig struct {
    Host        Host
    Specialists []*Specialist
    Summarizer  *Summarizer
}

// Host describes the routing ChatModel.
type Host struct {
    ChatModel    compose.ChatModel
    SystemPrompt string
}

// Specialist describes a domain expert.
type Specialist struct {
    Name         string
    IntendedUse  string
    ChatModel    compose.ChatModel
    SystemPrompt string
    Invokable    func(ctx context.Context, input []*compose.Message) (*compose.Message, error)
    Streamable   func(ctx context.Context, input []*compose.Message) (compose.StreamReader[*compose.Message], error)
}

// Summarizer aggregates multiple specialist answers in multi-intent mode.
type Summarizer struct {
    ChatModel    compose.ChatModel
    SystemPrompt string
}

// hostState is the per-run graph-local state for a Host Multi-Agent.
type hostState struct {
    Msgs             []*compose.Message // original user messages
    IsMultipleIntents bool
}
```

**API Contract — Exported Symbols**:

| Symbol | Kind | Location (owner) |
|--------|------|-------------------|
| `AgentConfig` | Type | `agent/types.go` (I1) |
| `MessageRewriter` | Function type | `agent/types.go` (I1) |
| `MessageModifier` | Function type | `agent/types.go` (I1) |
| `StreamToolCallChecker` | Function type | `agent/types.go` (I1) |
| `reactState` | Type (unexported) | `agent/types.go` (I1) |
| `Agent` | Type | `agent/types.go` (I1) |
| `MultiAgentConfig` | Type | `agent/types.go` (I2) |
| `Host` | Type | `agent/types.go` (I2) |
| `Specialist` | Type | `agent/types.go` (I2) |
| `Summarizer` | Type | `agent/types.go` (I2) |
| `hostState` | Type (unexported) | `agent/types.go` (I2) |

---

## 4. REACT AGENT CORE (I1: `agent/react.go`, `agent/react_option.go`, `agent/react_callback.go`)

### 4.1 `agent/react.go` — Graph Builder (I1)

**Owner**: I1
**Estimated LOC**: 350

```go
package agent

import (
    "context"
    "github.com/rive/eino-compose-runtime-replica-go/compose"
)

// NewAgent builds a ReAct agent as a compose.Graph.
//
// Graph topology:
//   START → ChatModel
//   ChatModel ──(has tool call)──→ Tools → ChatModel (loop)
//   ChatModel ──(no tool call)───→ END
//   Tools ──(return directly)──→ direct_return lambda → END
//
func NewAgent(ctx context.Context, config *AgentConfig) (*Agent, error)

// reactAgentInternal holds the graph building constants and helpers.
const (
    nodeKeyModel                     = "chat_model"
    nodeKeyTools                     = "tools"
    nodeKeyDirectReturn              = "direct_return"
    branchKeyModelPost               = "model_post_branch"
    branchKeyReturnDirectly          = "return_directly_branch"
    nodeKeyToolsDirectReturn         = "tools_direct_return" // tools node in the direct-return path
    defaultGraphName                 = "ReActAgent"
)

// modelPreHandle is the pre-handler for the ChatModel node.
// It appends the current input to state.Messages, runs MessageRewriter (persistent),
// then copies state.Messages and runs MessageModifier (temporary) for the model input.
func modelPreHandle(config *AgentConfig) func(ctx context.Context, input any) (any, error)

// toolsNodePreHandle is the pre-handler for the Tools node.
// It appends the model's tool call message to state.Messages and checks
// for return-directly tool calls.
func toolsNodePreHandle(config *AgentConfig) func(ctx context.Context, input any) (any, error)

// modelPostBranchCondition returns the branch condition for ChatModel → Tools or END.
// In invoke mode: checks message.ToolCalls. In stream mode: uses StreamToolCallChecker.
func modelPostBranchCondition(config *AgentConfig) compose.BranchCondition[*compose.Message]

// buildReturnDirectly adds the post-Tools branch that checks if any tool
// is marked for direct return, and if so routes to the direct_return lambda node.
func buildReturnDirectly(g *compose.Graph[[]*compose.Message, *compose.Message], config *AgentConfig)

// defaultStreamToolCallChecker implements the "first chunk" heuristic.
// Reads stream chunks: empty Content → continue; has ToolCalls → true; non-empty Content → false.
// Works for OpenAI-style providers; override via AgentConfig.StreamToolCallChecker.
func defaultStreamToolCallChecker(ctx context.Context, sr compose.StreamReader[*compose.Message]) (bool, error)

// SetReturnDirectly marks the current tool call for direct return.
// Called from within a tool implementation. Requires SetToolCallID to have been called
// by the ToolsNode before tool execution.
func SetReturnDirectly(ctx context.Context, callID string) error
```

**API Contract — Exported Symbols**:

| Symbol | Kind | Location |
|--------|------|----------|
| `NewAgent(ctx, config) (*Agent, error)` | Function | `agent/react.go` |
| `defaultStreamToolCallChecker` | Function (exported for test reuse) | `agent/react.go` |
| `SetReturnDirectly(ctx, callID) error` | Function | `agent/react.go` |

**Internal helper signatures**:

| Symbol | Kind | Location |
|--------|------|----------|
| `modelPreHandle(config) func(ctx, any) (any, error)` | Closure factory | `agent/react.go` |
| `toolsNodePreHandle(config) func(ctx, any) (any, error)` | Closure factory | `agent/react.go` |
| `modelPostBranchCondition(config) BranchCondition[*Message]` | Closure factory | `agent/react.go` |
| `buildReturnDirectly(g, config)` | Method | `agent/react.go` |

**Graph construction sequence** (inside `NewAgent`):
1. `g := compose.NewGraph[[]*compose.Message, *compose.Message]()`
2. Create ChatModelComponent from `config.ChatModel`
3. `g.AddChatModelNode(nodeKeyModel, cmc, compose.WithNodePreHandler(modelPreHandle(config)))`
4. `g.AddEdge(compose.START, nodeKeyModel)`
5. Create ToolsNode from `config.ToolsConfig`
6. `g.AddToolsNode(nodeKeyTools, tn, compose.WithNodePreHandler(toolsNodePreHandle(config)))`
7. `g.AddBranch(nodeKeyModel, compose.NewGraphBranch(modelPostBranchCondition(config), map[string]bool{nodeKeyTools: true, compose.END: true}))`
8. `buildReturnDirectly(g, config)`
9. `compileOpts := []compose.CompileOption{`
   `    compose.WithGraphName(defaultGraphName),`
   `    compose.WithMaxRunSteps(config.MaxStep),`
   `    compose.WithNodeTriggerMode(compose.AnyPredecessor),`
   `    compose.WithGenLocalState(func(ctx context.Context) *reactState { ... }),`
   `}`
10. `runnable, err := g.Compile(ctx, compileOpts...)`
11. Return `&Agent{Runnable: runnable, Graph: g}`

---

### 4.2 `agent/react_option.go` — Runtime Options (I1)

**Owner**: I1
**Estimated LOC**: 160

```go
package agent

// WithTools creates a runtime option that binds tool infos AND tool implementations.
// Must be used together for the same Generate/Stream call.
// Without it: ChatModel won't know about tools, ToolsNode won't know how to execute.
func WithTools(toolInfos []*compose.ToolInfo, tools []compose.InvokableTool) AgentOption

// WithStreamToolCallChecker overrides the default first-chunk checker.
func WithStreamToolCallChecker(checker StreamToolCallChecker) AgentOption

// WithMessageFuture returns a callback option and a MessageCollector that receives
// all intermediate messages produced during agent execution.
func WithMessageFuture() (AgentOption, *MessageCollector)

// WithChatModelOptions forwards extra options to the ChatModel.
func WithChatModelOptions(opts ...compose.Option) AgentOption

// AgentOption is passed to Agent.Generate / Agent.Stream.
type AgentOption func(*agentOptionState)

// MessageCollector receives streaming intermediate messages.
type MessageCollector struct {
    ch chan *compose.Message
}

func (mc *MessageCollector) Messages() <-chan *compose.Message
```

**API Contract — Exported Symbols**:

| Symbol | Kind | Location |
|--------|------|----------|
| `WithTools(toolInfos, tools) AgentOption` | Function | `agent/react_option.go` |
| `WithStreamToolCallChecker(checker) AgentOption` | Function | `agent/react_option.go` |
| `WithMessageFuture() (AgentOption, *MessageCollector)` | Function | `agent/react_option.go` |
| `WithChatModelOptions(opts...) AgentOption` | Function | `agent/react_option.go` |
| `AgentOption` | Type | `agent/react_option.go` |
| `MessageCollector` | Type | `agent/react_option.go` |
| `(*MessageCollector).Messages() <-chan *compose.Message` | Method | `agent/react_option.go` |

---

### 4.3 `agent/react_callback.go` — Callback Handler with Address Isolation (I1)

**Owner**: I1
**Estimated LOC**: 120

```go
package agent

// callbackHandler collects intermediate messages during agent execution.
// It uses compose.Address for isolation: on first graph OnStart, it records its
// address; subsequent callbacks check isOwnGraph before processing.
type callbackHandler struct {
    graphName    string
    ownAddress   compose.Address
    msgCollector *MessageCollector
}

// claimOwnership is called on graph OnStart. Records the current address.
func (h *callbackHandler) claimOwnership(ctx context.Context) context.Context

// isOwnGraph checks whether the current execution address matches the recorded address.
func (h *callbackHandler) isOwnGraph(ctx context.Context) bool

// onChatModelEnd handles the model's OnEnd callback, collecting the output message.
func (h *callbackHandler) onChatModelEnd(ctx context.Context, info *compose.RunInfo, output any) context.Context

// registerCallbacks attaches the callbackHandler's OnStart and model OnEnd
// to the agent graph's compile options.
func (h *callbackHandler) registerCallbacks() []compose.CompileOption

// newCallbackHandler creates a handler for the given graph name and collector.
func newCallbackHandler(graphName string, collector *MessageCollector) *callbackHandler
```

**API Contract**: All symbols in this file are unexported (implementation details). I1 owns the file. The `WithMessageFuture` exported function in `react_option.go` uses `callbackHandler` internally.

**Address isolation logic**:
1. Graph starts → `OnStart` callback fires → `claimOwnership` records `GetCurrentAddress(ctx)`
2. Each model output → `OnEnd` callback fires → `isOwnGraph` compares `GetCurrentAddress(ctx)` prefix with recorded address
3. Match → collect message into `MessageCollector.ch`. No match → return immediately (no action).
4. Channels close when graph execution completes (via `OnGraphEnd` callback).

---

## 5. HOST MULTI-AGENT SKELETON (I2: `agent/host.go`, `agent/host_option.go`)

### 5.1 `agent/host.go` — Graph Builder (I2)

**Owner**: I2
**Estimated LOC**: 400

```go
package agent

import (
    "context"
    "github.com/rive/eino-compose-runtime-replica-go/compose"
)

// NewMultiAgent builds a Host Multi-Agent as a compose.Graph.
//
// Graph topology:
//   START → Host (ChatModel)
//   Host ──(no tool call)──→ END
//   Host ──(has tool calls)──→ msg2MsgList
//   msg2MsgList ──(multi-branch)──→ Specialist_A, Specialist_B, ...
//   Specialist_A ─┐
//   Specialist_B ─┤→ SpecialistsAnswersCollector (passthrough)
//   ...           ─┘
//   SpecialistsAnswersCollector ──(branch: single vs multi)──→
//       ├── SingleIntentAnswer → END
//       └── MapToList → MultiIntentSummarize → END
//
func NewMultiAgent(ctx context.Context, config *MultiAgentConfig) (*Agent, error)

// ——————————————————— Internal constants ——————————————————
const (
    hostKeyNodeKey                 = "host"
    hostKeyMsg2MsgList             = "msg_to_msg_list"
    hostKeyAnswersCollector        = "specialist_answers_collector"
    hostKeySingleIntentAnswer      = "single_intent_answer"
    hostKeyMapToList               = "map_to_list"
    hostKeyMultiIntentSummarize    = "multi_intent_summarize"
    hostKeyBranchAfterHost         = "branch_after_host"
    hostKeyBranchAfterCollector    = "branch_after_collector"
    hostDefaultGraphName           = "HostMultiAgent"
)

// ——————————————————— Internal helpers ——————————————————

// addHostAgent adds the Host ChatModel node with its system prompt pre-handler.
func addHostAgent(g *compose.Graph[[]*compose.Message, *compose.Message], cfg *MultiAgentConfig)

// addSpecialistAgent adds a single specialist node.
// If ChatModel is set, uses ChatModelComponent with SystemPrompt pre-handler.
// If Invokable/Streamable is set, wraps as AnyLambda via Lambda component.
// All specialist nodes use compose.WithOutputKey(specialist.Name) for downstream routing.
func addSpecialistAgent(g *compose.Graph[[]*compose.Message, *compose.Message], spec *Specialist, index int) error

// addDirectAnswerBranch adds the branch after Host: toolCall → msg2MsgList, else → END.
func addDirectAnswerBranch(g *compose.Graph[[]*compose.Message, *compose.Message], cfg *MultiAgentConfig)

// addMultiSpecialistsBranch adds the msg2MsgList converter and multi-branch to specialists.
// If Host's output has >1 tool calls, sets state.IsMultipleIntents = true.
func addMultiSpecialistsBranch(g *compose.Graph[[]*compose.Message, *compose.Message], specialists []*Specialist)

// addAfterSpecialistsBranch adds the branch after collector: single intent → SingleIntentAnswer,
// multiple intents → MapToList → Summarize.
func addAfterSpecialistsBranch(g *compose.Graph[[]*compose.Message, *compose.Message], cfg *MultiAgentConfig)

// addMultiIntentsSummarizeNode adds the summarization node.
// If cfg.Summarizer.ChatModel is set, uses it with SystemPrompt pre-handler.
// Otherwise uses a default lambda that concatenates all specialist Content fields.
func addMultiIntentsSummarizeNode(g *compose.Graph[[]*compose.Message, *compose.Message], cfg *MultiAgentConfig) error

// toolCallChecker reads the host's output and returns whether it contains tool calls.
func toolCallChecker(ctx context.Context, msg *compose.Message) (bool, error)
```

**API Contract — Exported Symbols**:

| Symbol | Kind | Location |
|--------|------|----------|
| `NewMultiAgent(ctx, config) (*Agent, error)` | Function | `agent/host.go` |

**Graph construction sequence** (inside `NewMultiAgent`):
1. Validate config (host must have ChatModel, at least one specialist)
2. Generate ToolInfo for each specialist: `Name=specialist.Name`, `Desc=specialist.IntendedUse`
3. Create graph, host node, specialist nodes, branching, collector
4. Compile with `WithGenLocalState[hostState]`, `WithGraphName`, `WithMaxRunSteps`, `AnyPredecessor`
5. Return `&Agent{Runnable: runnable, Graph: g}`

**Specialist node construction** (inside `addSpecialistAgent`):
- Priority: `ChatModel` → `Invokable` → `Streamable` → error
- ChatModel mode: `ChatModelComponent.ChatModel = spec.ChatModel` + `WithNodePreHandler` injects `SystemPrompt`
- Invokable mode: `LambdaRunnable.Invoke = spec.Invokable`
- Streamable mode: `LambdaRunnable.Stream = spec.Streamable`
- Output key: `compose.WithOutputKey(spec.Name)` on the node

**Pre-handler for Specialist (ChatModel mode)**: wraps input with `SystemPrompt` → prepends `compose.SystemMessage(prompt)` to input `[]*compose.Message`

**Pre-handler for Specialist (Invokable/Streamable mode)**: `return state.msgs, nil` — discards tool call input, replaces with full message history from `hostState.Msgs`

---

### 5.2 `agent/host_option.go` — Host Runtime Options (I2)

**Owner**: I2
**Estimated LOC**: 60

```go
package agent

// WithHostCallbacks registers callbacks specifically on the Host ChatModel node.
func WithHostCallbacks(handlers ...*compose.Handler) AgentOption

// WithSpecialistCallbacks registers callbacks on all specialist nodes.
func WithSpecialistCallbacks(handlers ...*compose.Handler) AgentOption
```

**API Contract — Exported Symbols**:

| Symbol | Kind | Location |
|--------|------|----------|
| `WithHostCallbacks(handlers...) AgentOption` | Function | `agent/host_option.go` |
| `WithSpecialistCallbacks(handlers...) AgentOption` | Function | `agent/host_option.go` |

---

## 6. FAKE MODEL / TOOL PATTERNS (for testing)

Both I1 and I2 tests need deterministic, controllable fakes. These patterns are defined here so T1 can write tests against a known contract.

### 6.1 FakeChatModel for ReAct Tests

```go
// Pattern: FakeChatModel with a script of responses.
// Each call to Generate consumes the next scripted message.

func NewScriptedChatModel(responses []*compose.Message) *compose.FakeChatModel {
    callCount := 0
    return compose.NewFakeChatModel(
        compose.WithChatGenerateFunc(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
            if callCount >= len(responses) {
                // Default: no tool calls → terminate loop
                return &compose.Message{Role: compose.Assistant, Content: "done"}, nil
            }
            resp := responses[callCount]
            callCount++
            return resp, nil
        }),
    )
}

// Usage:
//   model := NewScriptedChatModel([]*compose.Message{
//       {Role: compose.Assistant, Content: "", ToolCalls: []compose.ToolCall{{ID: "1", Function: compose.ToolCallFunction{Name: "search", Arguments: `{"q":"hello"}`}}}},
//       {Role: compose.Assistant, Content: "The answer is 42."},
//   })
//   // First call → tool call. Second call → final answer.
```

### 6.2 FakeTool for ReAct Tests

```go
// Pattern: A tool that returns a canned response and optionally calls SetReturnDirectly.

func NewCannedTool(name, desc string, result string, directReturn bool) compose.InvokableTool {
    return &cannedTool{
        name:        name,
        desc:        desc,
        result:      result,
        directReturn: directReturn,
    }
}

type cannedTool struct {
    name, desc, result string
    directReturn       bool
}

func (t *cannedTool) Info(ctx context.Context) (*compose.ToolInfo, error) {
    return &compose.ToolInfo{Name: t.name, Desc: t.desc}, nil
}

func (t *cannedTool) Invoke(ctx context.Context, args string) (string, error) {
    if t.directReturn {
        if callID := compose.GetToolCallID(ctx); callID != "" {
            agent.SetReturnDirectly(ctx, callID)
        }
    }
    return t.result, nil
}
```

### 6.3 FakeChatModel for Host Multi-Agent Tests

```go
// Host routing fake: output contains tool calls naming which specialists to invoke.
func NewRoutingFakeModel(toolCalls []compose.ToolCall) *compose.FakeChatModel {
    return compose.NewFakeChatModel(
        compose.WithChatGenerateFunc(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
            return &compose.Message{
                Role:      compose.Assistant,
                ToolCalls: toolCalls,
            }, nil
        }),
    )
}

// Specialist fake: returns a canned answer.
func NewSpecialistFakeModel(answer string) *compose.FakeChatModel {
    return compose.NewFakeChatModel(
        compose.WithChatGenerateFunc(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
            return &compose.Message{Role: compose.Assistant, Content: answer}, nil
        }),
    )
}
```

### 6.4 Fake StreamToolCallChecker

```go
// Pattern: A stream checker with a controlled return value.
func NewFakeStreamChecker(hasToolCall bool) StreamToolCallChecker {
    return func(ctx context.Context, sr compose.StreamReader[*compose.Message]) (bool, error) {
        // Drain the stream for realism, but ignore content
        for {
            _, err := sr.Recv()
            if err == io.EOF {
                break
            }
            if err != nil {
                return false, err
            }
        }
        return hasToolCall, nil
    }
}
```

### 6.5 Stream Messages Helper

```go
// Pattern: Create a StreamReader from message slices for stream-mode testing.
func StreamFromMessages(msgs ...*compose.Message) compose.StreamReader[*compose.Message] {
    return compose.StreamReaderFromSlice(msgs)
}
```

---

## 7. EXACT TESTS REQUIRED (T1)

### 7.1 `agent/react_test.go` — ReAct Agent Tests (T1, after I1 completes)

**Estimated LOC**: 350

#### CRITICAL Tests

| Test Name | Description | Input | Expected |
|-----------|-------------|-------|----------|
| `TestReAct_NoTools_ReturnsModelOutput` | Agent with model that always returns text → loop terminates immediately | Model returns `{Content:"hello"}` | Output Content = "hello", 1 model call |
| `TestReAct_SingleToolCall` | Agent calls tool once, then model returns final answer | Model round 1: tool call. Round 2: final answer. | Output Content = final answer, tool was called |
| `TestReAct_MultiRoundToolCall` | Agent calls tool three times before final answer | Model: TC → TC → TC → text | Final answer correct, 3 tool executions |
| `TestReAct_MaxStepEnforced` | Agent exceeds MaxStep → error | MaxStep=2, model always emits tool call | ErrExceedMaxSteps |
| `TestReAct_ReturnDirectly_Config` | Tool in ToolReturnDirectly map → agent returns tool result directly | Config marks "search" as direct return | Output = tool result message, not model output |
| `TestReAct_ReturnDirectly_Runtime` | Tool calls SetReturnDirectly → agent returns tool result | Tool implementation calls SetReturnDirectly | Output = tool result message |
| `TestReAct_MessageModifier_Persistent` | MessageModifier appends system prompt each round → does NOT accumulate across rounds | Two rounds, modifier adds system prompt | state.Messages does not contain duplicate system prompts |
| `TestReAct_MessageRewriter_Compression` | MessageRewriter truncates state → truncation persists | Rewriter keeps last 3 of 10 messages | state.Messages has 3 messages after two rounds |
| `TestReAct_MessageRewriter_Ordering` | MessageRewriter runs BEFORE MessageModifier | Both set, rewriter removes msg[0] | Modifier receives truncated list |
| `TestReAct_StreamToolCallChecker_Default` | Default checker with OpenAI-style stream | Stream: empty chunk, then chunk with ToolCalls | Returns true |
| `TestReAct_StreamToolCallChecker_ClaudeStyle` | Custom checker handles text-before-toolcall | Stream: text chunk, then tool call chunk | Returns true (custom checker drains full stream) |
| `TestReAct_WithMessageFuture` | WithMessageFuture collects intermediate messages | Agent does 2 rounds | Collector receives all model output messages |

#### HIGH Tests

| Test Name | Description |
|-----------|-------------|
| `TestReAct_EmptyInput` | Agent with empty input messages → handles gracefully |
| `TestReAct_NilConfig` | NewAgent(nil) / NewAgent with nil ChatModel → error |
| `TestReAct_NoToolsConfig` | ToolsConfig empty → agent just passes model output through |
| `TestReAct_StateIsolation` | Two agents run sequentially → states don't leak |
| `TestReAct_StreamMode_Basic` | agent.Stream() produces same result as agent.Generate() |
| `TestReAct_CallbackHandler_AddressIsolation` | Nested agents → callback only collects own messages |
| `TestReAct_SetReturnDirectly_Priority` | Both config and SetReturnDirectly set for different tools → runtime wins |

---

### 7.2 `agent/host_test.go` — Host Multi-Agent Tests (T1, after I2 completes)

**Estimated LOC**: 350

#### CRITICAL Tests

| Test Name | Description | Input | Expected |
|-----------|-------------|-------|----------|
| `TestHost_SingleSpecialist_SingleIntent` | Host routes to one specialist, returns its answer | Host emits tool call for "code_expert" | Output = code_expert's answer |
| `TestHost_MultiSpecialist_MultiIntent` | Host routes to two specialists, returns summarized answer | Host emits two tool calls | Output = concatenated/aggregated result |
| `TestHost_NoSpecialist_DirectAnswer` | Host model outputs text (no tool call) → returned directly | Host model outputs text | Output = host's text, no specialists invoked |
| `TestHost_Specialist_ChatModel` | Specialist is ChatModel with SystemPrompt | Specialist has ChatModel + SystemPrompt | Specialist sees system prompt in its input |
| `TestHost_Specialist_Invokable` | Specialist is Invokable function | Specialist has Invokable (e.g., agent.Generate) | Invokable called with state.msgs |
| `TestHost_Specialist_Streamable` | Specialist is Streamable function | Specialist has Streamable (e.g., agent.Stream) | Streamable produces streaming output |
| `TestHost_PreHandler_InputReplacement` | Specialist pre-handler replaces tool call input with state.msgs | Host sends tool call args; state has original msgs | Specialist receives state.msgs, NOT tool call args |
| `TestHost_DefaultSummarization` | Multi-intent with no custom Summarizer | Two specialists return text | Output concatenates both Content fields |
| `TestHost_CustomSummarizer` | Multi-intent with custom Summarizer ChatModel | Two specialists return text | Summarizer ChatModel called with all specialist outputs |
| `TestHost_MaxStepEnforced` | Host loop exceeds MaxStep | Host always emits tool call | ErrExceedMaxSteps |

#### HIGH Tests

| Test Name | Description |
|-----------|-------------|
| `TestHost_SingleSpecialist_OutputKey` | Specialist output correctly keyed and collected by passthrough |
| `TestHost_EmptySpecialists` | Config with no specialists → validation error |
| `TestHost_NilHostChatModel` | Config with nil host ChatModel → validation error |
| `TestHost_LargeMultiIntent` | 5 specialists, all routed → collector receives all |
| `TestHost_StateIsolation` | Two host agents run sequentially → states don't leak |
| `TestHost_AgentAsSpecialist` | Specialist = ReAct agent (via Invokable) → works as sub-graph |

---

### 7.3 `compose/state_test.go` — State Infrastructure Tests (I1, as part of react implementation)

**Estimated LOC**: 150

#### CRITICAL Tests

| Test Name | Description | Input | Expected |
|-----------|-------------|-------|----------|
| `TestState_WithGenLocalState_CreatesPerRun` | Two graph runs → separate state instances | Graph with WithGenLocalState | Each run has independent state |
| `TestState_ProcessState_ReadWrite` | Tool implementation modifies state via ProcessState | ProcessState sets field | Subsequent ProcessState reads modified value |
| `TestState_GetState_NilContext` | GetState on context without state | Context without state | Returns false, nil |
| `TestState_WithNodePreHandler_RunsBeforeAction` | Pre-handler transforms input before node action | Pre-handler doubles input string | Node action receives doubled value |
| `TestState_WithNodePreHandler_AccessesState` | Pre-handler reads/writes state | Pre-handler uses ProcessState | State mutations visible to subsequent nodes |
| `TestState_SetToolCallID_GetToolCallID` | Set/Get round-trip | Set ID, retrieve | ID matches |

---

## 8. INTEGRATION / EXAMPLE (T1)

### 8.1 `cmd/example/main.go` — ADD Chapter 7 Section (T1)

**Estimated LOC**: +100

Add a new `exampleReAct()` function to the existing example program demonstrating:
1. Create a FakeChatModel that simulates a ReAct loop (search tool call → tool result → final answer)
2. Build ReAct agent with `NewAgent`
3. Demonstrate `MessageModifier` with system prompt injection
4. Demonstrate `MessageRewriter` with context compression
5. Demonstrate `SetReturnDirectly` in tool implementation
6. Demonstrate `WithMessageFuture` message collection

Add a new `exampleHostMultiAgent()` function demonstrating:
1. Create routing model + two specialist models
2. Build Host Multi-Agent with `NewMultiAgent`
3. Demonstrate single-intent routing
4. Demonstrate multi-intent routing with summarization

### 8.2 README Update (T1)

**Estimated LOC**: +40

Add Chapter 7 section after existing Chapter 6 section.

---

## 9. FILE OWNERSHIP SUMMARY

| File | Owner | Estimated LOC | Dependencies |
|------|-------|---------------|--------------|
| `compose/state.go` | I1 | 50 | none (new) |
| `compose/state_test.go` | I1 (delivered w/ react) | 150 | `compose/state.go` |
| `agent/types.go` | I1 (base) → I2 (extend) | 80 | `compose/` |
| `agent/react.go` | I1 | 350 | `compose/`, `agent/types.go` |
| `agent/react_option.go` | I1 | 160 | `compose/`, `agent/types.go` |
| `agent/react_callback.go` | I1 | 120 | `compose/`, `agent/types.go` |
| `agent/host.go` | I2 | 400 | `compose/`, `agent/types.go` |
| `agent/host_option.go` | I2 | 60 | `compose/`, `agent/types.go` |
| `agent/react_test.go` | T1 (after I1) | 350 | `agent/react.go` |
| `agent/host_test.go` | T1 (after I2) | 350 | `agent/host.go` |
| `cmd/example/main.go` (modify) | T1 | +100 | agent/ |
| `README.md` (modify) | T1 | +40 | — |

---

## 10. CUTS — Features Intentionally Not Copied

| # | Eino Feature | Reason for Exclusion |
|---|-------------|---------------------|
| 1 | Agent Option 双通道多态 (`AgentOption.composeOptions` + `implSpecificOptFn`) | Only 2 agent builders; explicit option functions suffice |
| 2 | Host `OnHandOff` callback (`MultiAgentCallback`, `ConvertCallbackHandlers`) | Observability feature; education scope uses generic callbacks |
| 3 | Streaming ToolsNode (streaming tool execution) | ToolsNode is invoke-only in education scope (matches Ch5) |
| 4 | Enhanced tool result (multi-modal: images, audio) | Tool result is string-only (matches Ch5) |
| 5 | `WithMessageFuture` four tool result sender types | Simplified to single channel-based collector |
| 6 | `ExportGraph` / dynamic graph modification | Agent graph is build-once, compile-once |
| 7 | Production ChatModel ↔ ToolCallingModel integration | Uses education-scope FakeChatModel |
| 8 | Claude/Gemini provider-specific `StreamToolCallChecker` | Only default checker + injection point; provider adaptation is Ch6 responsibility |
| 9 | Custom Summarizer ChatModel with full pre-handler pipeline | Multi-intent summarization uses default lambda (concatenation); custom Summarizer hook accepts ChatModel as optional |
| 10 | Interrupt/Resume in agent loop | Ch4 checkpoint/interrupt exists at graph layer; agent-specific interrupt not in scope |
| 11 | `BuildAgentCallback` (callback builder helper) | Reuses generic graph callback mechanism |
| 12 | `WithGraphAddNodeOpts` (custom node injection at compile time) | Dynamic graph modification excluded |
| 13 | `GetImplSpecificOptions` generic option extraction | No generic option pipe needed |

---

## 11. RISKS

| Risk | Severity | Mitigation |
|------|----------|------------|
| **State infrastructure breaks existing compile/graph_run** | HIGH | `compose/state.go` adds new fields via options; existing code paths gated by nil checks. All existing tests must pass after addition. |
| **Pre-handler ordering (input-pre vs output-post) causes subtle bugs** | MEDIUM | Existing `handlerPreNodes` processes output after task completion. New `WithNodePreHandler` processes input before task execution. They must use separate storage (different slice) to avoid interleaving. Use `chanCall.inputPreHandlers` (NEW) separate from `chanCall.preHandlers` (existing, renamed to `outputPreHandlers`). |
| **`ProcessState` type safety: mismatched T between calls** | HIGH | Go generics guarantee compile-time type safety: `ProcessState[int](ctx, fn)` and `ProcessState[string](ctx, fn)` operate on different context keys. Each `WithGenLocalState[T]` uses a unique key derived from `reflect.TypeOf((*T)(nil))`. Panic if type doesn't match — this is a programmer error. |
| **Address isolation in callbackHandler doesn't work with nested graphs** | MEDIUM | The mechanism mirrors Eino: `claimOwnership` records `GetCurrentAddress(ctx)` at graph OnStart. `isOwnGraph` compares prefixes. Test with nested agents verifies correctness. |
| **Host multi-branch construction** | HIGH | The compose layer's `GraphBranch` currently only supports single-target branches. Multi-branch (one-to-many routing) requires extending `branch.go` with `NewGraphMultiBranch` or building the multi-branch via a converter node + individual edges. RECOMMENDATION: Use a converter node (`msg2MsgList` node outputs `[]*compose.Message` as a list) + separate edges to each specialist. This avoids modifying the branch subsystem. |
| **ToolsNode dependency** | MEDIUM | Chapter 5 implemented `ToolsNode` with invoke mode. Verify it supports the `compose.WithNodePreHandler` registration. If not, I1 can wrap tools execution in a lambda node instead. |
| **`compose.WithOutputKey` and output routing** | MEDIUM | Verify Ch5 implemented output key support. If not, I2 can use set-node-callbacks or post-handler node for output routing by specialist name. |

**Multi-branch design decision** (for I2):

The most practical approach for the education replica is to **NOT** implement `NewGraphMultiBranch`, and instead use a single converter node + individual edges:

```
Host → post-branch: if toolCall → msg2MsgList (single converter node)
msg2MsgList → edge → Specialist_A
msg2MsgList → edge → Specialist_B
```

The `msg2MsgList` node output is `[]*compose.Message` (full message list). Each specialist receives the same input via its own Pregel channel. This is correct because:
- Each specialist's pre-handler replaces the input with `state.msgs` anyway
- The pre-handler ignores the specific tool call argument

This avoids `NewGraphMultiBranch` entirely while preserving correctness.

---

## 12. VERIFICATION

After I1 and I2 complete their implementations:

```bash
cd examples/eino-compose-runtime-replica-go
go test ./compose/... -count=1  # Existing compose tests must still pass
go test ./agent/... -count=1    # New agent tests must pass
go vet ./...                    # No vet errors
go build ./...                  # No build errors
```

After T1 adds integration tests and examples:

```bash
go run cmd/example/main.go  # Chapter 7 examples run without error
```

---

## Appendix A: ToolsNode Interface Recap (from Ch5)

I1 and I2 need to reference these types from Chapter 5:

```go
// compose/tool_names.go (from Ch5)
type InvokableTool interface {
    Info(ctx context.Context) (*ToolInfo, error)
    Invoke(ctx context.Context, args string) (string, error)
}

type ToolsNodeConfig struct {
    Tools        []InvokableTool
    ToolCallIDFn func(toolCall ToolCall) string
}
```

### ToolsNode usage in ReAct graph

The ToolsNode must support `compose.WithNodePreHandler` so the `toolsNodePreHandle` can run before tool execution. If Ch5's ToolsNode doesn't support this, I1 wraps tools execution as:

```go
// Fallback: manual tools execution lambda if ToolsNode doesn't support pre-handlers
func makeToolsLambda(config *AgentConfig) *compose.Lambda {
    return compose.NewLambda(
        func(ctx context.Context, msg *compose.Message) ([]*compose.Message, error) {
            // Execute each tool call in msg.ToolCalls sequentially
            // Return tool result messages
        },
    )
}
```

This is a low-risk fallback since the education scope supports sequential invocation only.

## Appendix B: StreamToolCallChecker Registration

The `StreamToolCallChecker` is used in `modelPostBranchCondition`. For invoke mode (non-streaming), the branch condition simply checks `msg.ToolCalls != nil && len(msg.ToolCalls) > 0`. For stream mode, it delegates to the configured checker.

```go
func modelPostBranchCondition(config *AgentConfig) compose.BranchCondition[*compose.Message] {
    return func(ctx context.Context, msg *compose.Message) (string, error) {
        // If msg is from invoke (non-stream), check ToolCalls directly
        if msg.ToolCalls != nil && len(msg.ToolCalls) > 0 {
            return nodeKeyTools, nil
        }
        // If stream mode was used, the caller must have collected the stream
        // and passed the collected message — same logic applies
        return compose.END, nil
    }
}
```

**Stream mode handling**: In the education replica, stream mode for the agent means:
1. The ChatModel.Stream() returns a `StreamReader[*Message]`
2. The `StreamToolCallChecker` reads the entire stream to determine if tool calls exist
3. The stream chunks are concatenated into a single `*Message` (as if invoke)
4. The concatenated message is then processed by the branch condition

This avoids the complexity of branch-on-stream-chunks while still demonstrating the StreamToolCallChecker concept.
