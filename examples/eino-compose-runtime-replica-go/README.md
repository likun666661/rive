# Eino Compose Runtime Replica (Go MVP)

Go implementation of the Eino Compose Graph runtime, replicating the core compile boundary and DAG/Pregel execution engines.

## Architecture

The replica implements the critical design decision from Eino: **separating graph topology construction from runtime execution**.

```
Graph Builder  ──>  Compile  ──>  Runnable[I, O]
 (mutable)       (compile lock)   (immutable exec)
```

### Key Features

- **Graph Builder**: Add nodes (Lambda), edges (data + control), and branches
- **Compile Boundary**: After `Compile()`, the graph is locked; modifications return `ErrGraphCompiled`
- **Runnable[I,O]**: Single execution interface with `Invoke(ctx, input) (output, err)`
- **NodeTriggerMode**: `AllPredecessor` (DAG) and `AnyPredecessor` (Pregel)
- **Channel Abstraction**: `dagChannel` enforces all-predecessors-ready; `pregelChannel` fires on any-predecessor
- **maxSteps**: Pregel mode has step limit to prevent infinite loops
- **GraphInfo**: Compile-time introspection exporting full topology
- **Event Log**: Thread-safe execution event recording

### DAG vs Pregel

| Dimension | DAG (AllPredecessor) | Pregel (AnyPredecessor) |
|---|---|---|
| Trigger | All control + data predecessors ready | Any data predecessor reports |
| Cycles | Rejected at compile time (Kahn) | Allowed with maxSteps guard |
| Skip propagation | Supported | Not supported |

## Quick Start

```go
package main

import (
    "context"
    "fmt"
    compose "github.com/rive/eino-compose-runtime-replica-go/compose"
)

func main() {
    g := compose.NewGraph[string, string]()

    g.AddLambdaNode("upper", compose.InvokableLambda(
        func(ctx context.Context, in string) (string, error) {
            return strings.ToUpper(in), nil
        },
    ))

    g.AddEdge(compose.START, "upper")
    g.AddEdge("upper", compose.END)

    r, _ := g.Compile(context.Background(),
        compose.WithGraphName("my_graph"),
        compose.WithNodeTriggerMode(compose.AllPredecessor),
    )

    result, _ := r.Invoke(context.Background(), "hello")
    fmt.Println(result) // "HELLO"
}
```

## Run Example

```bash
cd examples/eino-compose-runtime-replica-go
go run ./cmd/example/
```

## Package Structure

```
compose/
├── types.go           # NodeTriggerMode, ComponentType, sentinel errors, START/END
├── runnable.go        # Runnable[I,O], composableRunnable, Lambda, InvokableLambda
├── graph.go           # graph struct, AddNode, AddEdge, compile(), Kahn cycle detection
├── generic_graph.go   # Graph[I,O] wrapper, NewGraph, Compile(), GraphInfo access
├── graph_node.go      # graphNode, compileIfNeeded (sub-graph recursion)
├── graph_compile.go   # CompileOption types: WithNodeTriggerMode, WithMaxRunSteps, etc.
├── graph_run.go       # runner struct, run() main loop, createTasks, resolveCompleted
├── graph_manager.go   # channel interface, channelManager, taskManager (goroutine pool)
├── dag.go             # dagChannel (AllPredecessor with ControlPredecessors state machine)
├── pregel.go          # pregelChannel (AnyPredecessor with simple Values map)
├── branch.go          # GraphBranch (conditional routing)
├── introspect.go      # GraphInfo, GraphNodeInfo (compile-time topology export)
├── event_log.go       # EventLog (thread-safe execution event recorder)
└── utils.go           # Helper utilities
```

## Design Decisions

1. **Zero external dependencies**: Only Go standard library
2. **Compile-lock pattern**: `graph.compiled` flag prevents mutations; same graph can be compiled multiple times with different options
3. **Channel polymorphism**: Both DAG and Pregel share `channel` interface; only implementation differs
4. **Kahn's algorithm**: DAG mode uses topological sort for cycle detection
5. **Goroutine pool**: taskManager runs nodes concurrently with WaitGroup synchronization
