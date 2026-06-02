package main

import (
	"context"
	"fmt"

	compose "github.com/rive/eino-compose-runtime-replica-go/compose"
)

func main() {
	fmt.Println("=== Eino Compose Runtime Replica MVP ===")
	fmt.Println()

	example1_DAGBasic()
	example2_PregelWithMaxSteps()
	example3_CompileBoundary()
	example4_GraphInfo()
	example5_EventLog()
}

func example1_DAGBasic() {
	fmt.Println("--- Example 1: Basic DAG Graph (AllPredecessor) ---")

	g := compose.NewGraph[string, string]()
	g.AddLambdaNode("upper", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return fmt.Sprintf("[UPPER:%s]", in), nil
	}))
	g.AddLambdaNode("reverse", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		runes := []rune(in)
		for i, j := 0, len(runes)-1; i < j; i, j = i+1, j-1 {
			runes[i], runes[j] = runes[j], runes[i]
		}
		return string(runes), nil
	}))

	g.AddEdge(compose.START, "upper")
	g.AddEdge("upper", "reverse")
	g.AddEdge("reverse", compose.END)

	r, err := g.Compile(context.Background(),
		compose.WithGraphName("basic_dag"),
		compose.WithNodeTriggerMode(compose.AllPredecessor),
	)
	if err != nil {
		fmt.Printf("Compile error: %v\n", err)
		return
	}

	result, err := r.Invoke(context.Background(), "hello world")
	if err != nil {
		fmt.Printf("Invoke error: %v\n", err)
		return
	}

	fmt.Printf("Input:  %q\n", "hello world")
	fmt.Printf("Output: %q\n", result)
	fmt.Printf("Expected: reversed upper-cased string\n\n")
}

func example2_PregelWithMaxSteps() {
	fmt.Println("--- Example 2: Pregel Graph (AnyPredecessor) with maxSteps ---")

	g := compose.NewGraph[int, int]()
	g.AddLambdaNode("increment", compose.InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in + 1, nil
	}))

	g.AddEdge(compose.START, "increment")
	g.AddEdge("increment", compose.END)

	r, err := g.Compile(context.Background(),
		compose.WithGraphName("pregel_example"),
		compose.WithNodeTriggerMode(compose.AnyPredecessor),
		compose.WithMaxRunSteps(50),
	)
	if err != nil {
		fmt.Printf("Compile error: %v\n", err)
		return
	}

	result, err := r.Invoke(context.Background(), 41)
	if err != nil {
		fmt.Printf("Invoke error: %v\n", err)
		return
	}

	fmt.Printf("Input:  %d\n", 41)
	fmt.Printf("Output: %d\n", result)
	fmt.Printf("Expected: 42\n\n")
}

func example3_CompileBoundary() {
	fmt.Println("--- Example 3: Compile Boundary ---")

	g := compose.NewGraph[string, string]()
	g.AddLambdaNode("echo", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	}))
	g.AddEdge(compose.START, "echo")
	g.AddEdge("echo", compose.END)

	r, err := g.Compile(context.Background(),
		compose.WithGraphName("compile_boundary"),
		compose.WithNodeTriggerMode(compose.AllPredecessor),
	)
	if err != nil {
		fmt.Printf("Compile error: %v\n", err)
		return
	}

	err = g.AddLambdaNode("new_node", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	}))
	if err != nil {
		fmt.Printf("Expected error after compile: %v\n", err)
	}

	_ = r
	result, err := r.Invoke(context.Background(), "boundary test")
	if err != nil {
		fmt.Printf("Invoke error: %v\n", err)
		return
	}
	fmt.Printf("Result: %q\n\n", result)
}

func example4_GraphInfo() {
	fmt.Println("--- Example 4: GraphInfo Introspection ---")

	g := compose.NewGraph[string, string]()
	g.AddLambdaNode("node_a", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	}))
	g.AddLambdaNode("node_b", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	}))
	g.AddEdge(compose.START, "node_a")
	g.AddEdge("node_a", "node_b")
	g.AddEdge("node_b", compose.END)

	r, err := g.Compile(context.Background(),
		compose.WithGraphName("introspect_graph"),
		compose.WithNodeTriggerMode(compose.AllPredecessor),
	)
	if err != nil {
		fmt.Printf("Compile error: %v\n", err)
		return
	}

	_ = r

	info := g.GetGraphInfo()
	fmt.Printf("Graph Name:     %s\n", info.Name)
	fmt.Printf("Trigger Mode:   %s\n", info.TriggerMode)
	fmt.Printf("DAG Mode:       %v\n", info.DAGMode)
	fmt.Printf("Pregel Mode:    %v\n", info.PregelMode)
	fmt.Printf("Max Steps:      %d\n", info.MaxSteps)
	fmt.Printf("Num Nodes:      %d\n", info.NumNodes)
	fmt.Printf("Num Edges:      %d\n", info.NumEdges)
	fmt.Printf("Input Type:     %s\n", info.InputType)
	fmt.Printf("Output Type:    %s\n", info.OutputType)
	fmt.Println()
}

func example5_EventLog() {
	fmt.Println("--- Example 5: Event Log ---")

	el := compose.NewEventLog()
	el.LogGraphStart("event_demo")
	el.LogNodeStart("node_1", 1, "input_data")
	el.LogNodeEnd("node_1", 1, "output_data")
	el.LogNodeStart("node_2", 2, "output_data")
	el.LogNodeEnd("node_2", 2, "final_result")
	el.LogGraphEnd("event_demo", 2)

	fmt.Println("Event Log:")
	fmt.Print(el.String())
	fmt.Println()
}
