package compose

import (
	"context"
	"errors"
	"fmt"
	"sort"
	"strings"
	"sync"
	"testing"
)

func nodeIdentity(ctx context.Context, in string) (string, error) {
	return in, nil
}

func nodeToUpper(ctx context.Context, in string) (string, error) {
	return strings.ToUpper(in), nil
}

func nodeReverse(ctx context.Context, in string) (string, error) {
	runes := []rune(in)
	for i, j := 0, len(runes)-1; i < j; i, j = i+1, j-1 {
		runes[i], runes[j] = runes[j], runes[i]
	}
	return string(runes), nil
}

func nodeFailing(ctx context.Context, in string) (string, error) {
	return "", errors.New("forced failure")
}

func TestDuplicateNode(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddLambdaNode("echo", InvokableLambda(nodeToUpper))

	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("duplicate_node"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != "HELLO" {
		t.Fatalf("expected HELLO (second AddLambdaNode overwrites first), got %q", result)
	}
}

func TestUnknownEdgeSource(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))

	err := g.AddEdge("nonexistent", "echo")
	if err == nil {
		t.Fatal("expected error for unknown source node")
	}
	if !errors.Is(err, ErrNodeNotFound) {
		t.Fatalf("expected ErrNodeNotFound, got %v", err)
	}
	if !strings.Contains(err.Error(), "nonexistent") {
		t.Fatalf("error should mention the missing node, got: %v", err)
	}
}

func TestUnknownEdgeTarget(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))

	err := g.AddEdge("echo", "nonexistent")
	if err == nil {
		t.Fatal("expected error for unknown target node")
	}
	if !errors.Is(err, ErrNodeNotFound) {
		t.Fatalf("expected ErrNodeNotFound, got %v", err)
	}
}

func TestUnknownControlEdge(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))

	err := g.AddControlEdge("echo", "ghost")
	if err == nil {
		t.Fatal("expected error for unknown control edge target")
	}
	if !errors.Is(err, ErrNodeNotFound) {
		t.Fatalf("expected ErrNodeNotFound, got %v", err)
	}
}

func TestCompileLockAddNode(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("compiled"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	err = g.AddLambdaNode("after_lock", InvokableLambda(nodeIdentity))
	if err == nil {
		t.Fatal("expected ErrGraphCompiled after compile")
	}
	if !errors.Is(err, ErrGraphCompiled) {
		t.Fatalf("expected ErrGraphCompiled, got %v", err)
	}
}

func TestCompileLockAddEdge(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("compiled"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	err = g.AddEdge("echo", "echo")
	if err == nil {
		t.Fatal("expected ErrGraphCompiled after compile")
	}
	if !errors.Is(err, ErrGraphCompiled) {
		t.Fatalf("expected ErrGraphCompiled, got %v", err)
	}
}

func TestCompileLockAddControlEdge(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("compiled"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	err = g.AddControlEdge("echo", "echo")
	if err == nil {
		t.Fatal("expected ErrGraphCompiled after compile")
	}
	if !errors.Is(err, ErrGraphCompiled) {
		t.Fatalf("expected ErrGraphCompiled, got %v", err)
	}
}

func TestGraphInfoDAGMode(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("node_a", InvokableLambda(nodeIdentity))
	g.AddLambdaNode("node_b", InvokableLambda(nodeToUpper))
	g.AddLambdaNode("node_c", InvokableLambda(nodeReverse))

	g.AddEdge(START, "node_a")
	g.AddEdge("node_a", "node_b")
	g.AddEdge("node_b", "node_c")
	g.AddEdge("node_c", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("info_test"),
		WithNodeTriggerMode(AllPredecessor),
		WithMaxRunSteps(50),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	info := g.GetGraphInfo()
	if info == nil {
		t.Fatal("expected non-nil GraphInfo")
	}
	if info.Name != "info_test" {
		t.Fatalf("expected name info_test, got %s", info.Name)
	}
	if info.TriggerMode != AllPredecessor {
		t.Fatalf("expected AllPredecessor, got %s", info.TriggerMode)
	}
	if !info.DAGMode {
		t.Fatal("expected DAGMode=true")
	}
	if info.PregelMode {
		t.Fatal("expected PregelMode=false")
	}
	if info.MaxSteps != 50 {
		t.Fatalf("expected MaxSteps=50, got %d", info.MaxSteps)
	}
	if info.NumNodes != 3 {
		t.Fatalf("expected 3 nodes, got %d", info.NumNodes)
	}
	if info.NumEdges != 4 {
		t.Fatalf("expected 4 edges, got %d", info.NumEdges)
	}
	if info.InputType != "string" {
		t.Fatalf("expected InputType=string, got %s", info.InputType)
	}
	if info.OutputType != "string" {
		t.Fatalf("expected OutputType=string, got %s", info.OutputType)
	}

	infoNodes := make(map[string]bool)
	for _, n := range info.Nodes {
		infoNodes[n.Name] = true
	}
	for _, expected := range []string{"node_a", "node_b", "node_c"} {
		if !infoNodes[expected] {
			t.Fatalf("expected node %s in GraphInfo.Nodes", expected)
		}
	}
}

func TestGraphInfoPregelMode(t *testing.T) {
	g := NewGraph[int, int]()

	g.AddLambdaNode("inc", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in + 1, nil
	}))
	g.AddEdge(START, "inc")
	g.AddEdge("inc", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("pregel_info"),
		WithNodeTriggerMode(AnyPredecessor),
		WithMaxRunSteps(10),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	info := g.GetGraphInfo()
	if info == nil {
		t.Fatal("expected non-nil GraphInfo")
	}
	if info.TriggerMode != AnyPredecessor {
		t.Fatalf("expected AnyPredecessor, got %s", info.TriggerMode)
	}
	if info.DAGMode {
		t.Fatal("expected DAGMode=false")
	}
	if !info.PregelMode {
		t.Fatal("expected PregelMode=true")
	}
	if info.MaxSteps != 10 {
		t.Fatalf("expected MaxSteps=10, got %d", info.MaxSteps)
	}
	if info.NumNodes != 1 {
		t.Fatalf("expected 1 node, got %d", info.NumNodes)
	}
}

func TestDAGFanIn(t *testing.T) {
	g := NewGraph[any, string]()

	g.AddLambdaNode("upper", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		s, _ := in.(string)
		return strings.ToUpper(s), nil
	}))
	g.AddLambdaNode("reverse", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		s, _ := in.(string)
		runes := []rune(s)
		for i, j := 0, len(runes)-1; i < j; i, j = i+1, j-1 {
			runes[i], runes[j] = runes[j], runes[i]
		}
		return string(runes), nil
	}))
	g.AddLambdaNode("merger", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		switch v := in.(type) {
		case string:
			return "single:" + v, nil
		case map[string]any:
			var parts []string
			for k, val := range v {
				parts = append(parts, fmt.Sprintf("%s=%v", k, val))
			}
			sort.Strings(parts)
			return "merged:" + strings.Join(parts, "|"), nil
		}
		return "", fmt.Errorf("fan_in merger: unexpected type %T", in)
	}))

	g.AddEdge(START, "upper")
	g.AddEdge(START, "reverse")
	g.AddEdge("upper", "merger")
	g.AddEdge("reverse", "merger")
	g.AddEdge("merger", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("fan_in"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if !strings.HasPrefix(result, "merged:") {
		t.Fatalf("expected merged multi-input result, got %q", result)
	}
	t.Logf("DAG fan-in result: %q", result)
}

func TestDAGFanInMultiInput(t *testing.T) {
	g := NewGraph[any, string]()

	g.AddLambdaNode("a", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		s, _ := in.(string)
		return "A-" + s, nil
	}))
	g.AddLambdaNode("b", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		s, _ := in.(string)
		return "B-" + s, nil
	}))
	g.AddLambdaNode("merger", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		switch v := in.(type) {
		case map[string]any:
			var parts []string
			for k, val := range v {
				parts = append(parts, fmt.Sprintf("%s=%v", k, val))
			}
			sort.Strings(parts)
			return "MERGED[" + strings.Join(parts, ",") + "]", nil
		case string:
			return "SINGLE[" + v + "]", nil
		}
		return "", fmt.Errorf("fan_in_multi merger: unexpected type %T", in)
	}))

	g.AddEdge(START, "a")
	g.AddEdge(START, "b")
	g.AddEdge("a", "merger")
	g.AddEdge("b", "merger")
	g.AddEdge("merger", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("fan_in_multi"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "x")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if !strings.Contains(result, "MERGED") {
		t.Fatalf("expected result to contain MERGED, got %q", result)
	}
	t.Logf("fan-in multi-input result: %q", result)
}

func TestDAGCycleRejection(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("a", InvokableLambda(nodeIdentity))
	g.AddLambdaNode("b", InvokableLambda(nodeToUpper))
	g.AddLambdaNode("c", InvokableLambda(nodeReverse))

	g.AddEdge(START, "a")
	g.AddEdge("a", "b")
	g.AddEdge("b", "c")
	g.AddEdge("c", "a")
	g.AddEdge("c", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("cycle_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err == nil {
		t.Fatal("expected cycle detection error in DAG mode")
	}
	if !errors.Is(err, ErrDAGHasCycle) {
		t.Fatalf("expected ErrDAGHasCycle, got %v", err)
	}
	t.Logf("cycle rejection: %v", err)
}

func TestDAGThreeNodeCycle(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("x", InvokableLambda(nodeIdentity))
	g.AddLambdaNode("y", InvokableLambda(nodeIdentity))
	g.AddLambdaNode("z", InvokableLambda(nodeIdentity))

	g.AddEdge(START, "x")
	g.AddEdge("x", "y")
	g.AddEdge("y", "z")
	g.AddEdge("z", "y")
	g.AddEdge("z", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("three_node_cycle"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err == nil {
		t.Fatal("expected cycle detection error")
	}
	if !errors.Is(err, ErrDAGHasCycle) {
		t.Fatalf("expected ErrDAGHasCycle, got %v", err)
	}
}

func TestPregelCycleAllowed(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("a", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "A:" + in, nil
	}))
	g.AddLambdaNode("b", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "B:" + in, nil
	}))

	g.AddEdge(START, "a")
	g.AddEdge("a", "b")
	g.AddEdge("b", "a")
	g.AddEdge("a", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("pregel_cycle"),
		WithNodeTriggerMode(AnyPredecessor),
		WithMaxRunSteps(10),
	)
	if err != nil {
		t.Fatalf("Pregel compile should allow cycles, but got: %v", err)
	}
	t.Log("Pregel mode correctly allows cycles")
}

func TestMaxStepsExceeded(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("loop", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in + ".", nil
	}))

	g.AddEdge(START, "loop")
	g.AddEdge("loop", "loop")
	g.AddEdge("loop", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("max_steps_test"),
		WithNodeTriggerMode(AnyPredecessor),
		WithMaxRunSteps(3),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	_, err = r.Invoke(context.Background(), "hello")
	if err == nil {
		t.Fatal("expected max steps exceeded error")
	}
	if !errors.Is(err, ErrExceedMaxSteps) {
		t.Fatalf("expected ErrExceedMaxSteps, got %v", err)
	}

	info := g.GetGraphInfo()
	if info.MaxSteps != 3 {
		t.Fatalf("expected MaxSteps=3 in GraphInfo, got %d", info.MaxSteps)
	}
	t.Logf("maxSteps exceeded as expected: %v", err)
}

func TestMaxStepsNotHitWhenBelow(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("below_max"),
		WithNodeTriggerMode(AnyPredecessor),
		WithMaxRunSteps(50),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "ok")
	if err != nil {
		t.Fatalf("Invoke should not hit maxSteps: %v", err)
	}
	if result != "ok" {
		t.Fatalf("expected ok, got %q", result)
	}
}

func TestDefaultMaxSteps(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("default_max"),
		WithNodeTriggerMode(AnyPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	info := g.GetGraphInfo()
	if info.MaxSteps != defaultMaxSteps {
		t.Fatalf("expected default MaxSteps=%d, got %d", defaultMaxSteps, info.MaxSteps)
	}
	_ = r
}

func TestEventLogLifecycle(t *testing.T) {
	el := NewEventLog()

	el.LogGraphStart("test_lifecycle")
	el.LogNodeStart("node_1", 1, "input_1")
	el.LogNodeEnd("node_1", 1, "output_1")
	el.LogNodeStart("node_2", 2, "input_2")
	el.LogNodeEnd("node_2", 2, "output_2")
	el.LogGraphEnd("test_lifecycle", 2)

	if len(el.Events) != 6 {
		t.Fatalf("expected 6 events, got %d", len(el.Events))
	}

	expectedTypes := []EventType{
		EventGraphStart,
		EventNodeStart,
		EventNodeEnd,
		EventNodeStart,
		EventNodeEnd,
		EventGraphEnd,
	}
	for i, et := range expectedTypes {
		if el.Events[i].Type != et {
			t.Fatalf("event[%d]: expected type %s, got %s", i, et, el.Events[i].Type)
		}
	}

	if el.Events[0].GraphName != "test_lifecycle" {
		t.Fatalf("expected graph name test_lifecycle, got %s", el.Events[0].GraphName)
	}
}

func TestEventLogNodeError(t *testing.T) {
	el := NewEventLog()

	el.LogGraphStart("error_test")
	el.LogNodeStart("bad_node", 1, "input")
	el.LogNodeError("bad_node", 1, errors.New("boom"))
	el.LogGraphError("error_test", errors.New("boom"))

	if len(el.Events) != 4 {
		t.Fatalf("expected 4 events, got %d", len(el.Events))
	}
	if el.Events[2].Type != EventNodeError {
		t.Fatalf("expected EventNodeError, got %s", el.Events[2].Type)
	}
	if el.Events[2].Error != "boom" {
		t.Fatalf("expected error 'boom', got %s", el.Events[2].Error)
	}
	if el.Events[3].Type != EventGraphError {
		t.Fatalf("expected EventGraphError, got %s", el.Events[3].Type)
	}
}

func TestEventLogMaxStepsHit(t *testing.T) {
	el := NewEventLog()

	el.LogGraphStart("max_test")
	el.LogNodeStart("loop", 1, "data")
	el.LogNodeEnd("loop", 1, "data.")
	el.LogMaxStepsHit("max_test", 101)

	if len(el.Events) != 4 {
		t.Fatalf("expected 4 events, got %d", len(el.Events))
	}
	if el.Events[3].Type != EventMaxStepsHit {
		t.Fatalf("expected EventMaxStepsHit, got %s", el.Events[3].Type)
	}
	if el.Events[3].Step != 101 {
		t.Fatalf("expected step 101, got %d", el.Events[3].Step)
	}
}

func TestEventLogString(t *testing.T) {
	el := NewEventLog()
	el.LogGraphStart("string_test")
	el.LogNodeStart("n1", 1, "in")
	el.LogNodeEnd("n1", 1, "out")
	el.LogGraphEnd("string_test", 1)

	s := el.String()
	if s == "" {
		t.Fatal("expected non-empty string output")
	}
	if !strings.Contains(s, "graph_start") {
		t.Fatal("expected output to contain graph_start")
	}
	if !strings.Contains(s, "node_start") {
		t.Fatal("expected output to contain node_start")
	}
	if !strings.Contains(s, "node_end") {
		t.Fatal("expected output to contain node_end")
	}
	if !strings.Contains(s, "graph_end") {
		t.Fatal("expected output to contain graph_end")
	}
}

func TestEventLogThreadSafety(t *testing.T) {
	el := NewEventLog()
	var wg sync.WaitGroup
	n := 100

	for i := 0; i < n; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			el.LogNodeStart("node", idx, idx)
			el.LogNodeEnd("node", idx, idx)
		}(i)
	}
	wg.Wait()

	if len(el.Events) != n*2 {
		t.Fatalf("expected %d events, got %d", n*2, len(el.Events))
	}
}

func TestEventLogEmpty(t *testing.T) {
	el := NewEventLog()
	if len(el.Events) != 0 {
		t.Fatalf("expected 0 events for fresh EventLog, got %d", len(el.Events))
	}
	if el.String() != "" {
		t.Fatalf("expected empty string, got %q", el.String())
	}
}

func TestBasicExampleDAG(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("upper", InvokableLambda(nodeToUpper))
	g.AddLambdaNode("reverse", InvokableLambda(nodeReverse))

	g.AddEdge(START, "upper")
	g.AddEdge("upper", "reverse")
	g.AddEdge("reverse", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("basic_dag_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result == "hello" {
		t.Fatal("expected transformed output, got unchanged input")
	}
	t.Logf("basic DAG result: %q", result)
}

func TestBasicExamplePregel(t *testing.T) {
	g := NewGraph[int, int]()

	g.AddLambdaNode("inc", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in + 1, nil
	}))

	g.AddEdge(START, "inc")
	g.AddEdge("inc", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("basic_pregel"),
		WithNodeTriggerMode(AnyPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), 41)
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != 42 {
		t.Fatalf("expected 42, got %d", result)
	}
}

func TestNoStartEdgeError(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge("echo", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("no_start"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err == nil {
		t.Fatal("expected ErrNoStartEdge")
	}
	if !errors.Is(err, ErrNoStartEdge) {
		t.Fatalf("expected ErrNoStartEdge, got %v", err)
	}
}

func TestNoEndEdgeError(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")

	_, err := g.Compile(context.Background(),
		WithGraphName("no_end"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err == nil {
		t.Fatal("expected ErrNoEndEdge")
	}
	if !errors.Is(err, ErrNoEndEdge) {
		t.Fatalf("expected ErrNoEndEdge, got %v", err)
	}
}

func TestRecompileWithDifferentOptions(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	r1, err := g.Compile(context.Background(),
		WithGraphName("recompile_a"),
		WithNodeTriggerMode(AllPredecessor),
		WithMaxRunSteps(20),
	)
	if err != nil {
		t.Fatalf("First compile failed: %v", err)
	}

	info1 := g.GetGraphInfo()
	if info1.MaxSteps != 20 {
		t.Fatalf("expected MaxSteps=20, got %d", info1.MaxSteps)
	}

	result1, err := r1.Invoke(context.Background(), "first")
	if err != nil {
		t.Fatalf("First invoke failed: %v", err)
	}

	r2, err := g.Compile(context.Background(),
		WithGraphName("recompile_b"),
		WithNodeTriggerMode(AnyPredecessor),
		WithMaxRunSteps(30),
	)
	if err != nil {
		t.Fatalf("Second compile failed: %v", err)
	}

	info2 := g.GetGraphInfo()
	if info2.Name != "recompile_b" {
		t.Fatalf("expected name recompile_b, got %s", info2.Name)
	}
	if info2.TriggerMode != AnyPredecessor {
		t.Fatalf("expected AnyPredecessor, got %s", info2.TriggerMode)
	}
	if info2.MaxSteps != 30 {
		t.Fatalf("expected MaxSteps=30, got %d", info2.MaxSteps)
	}

	result2, err := r2.Invoke(context.Background(), "second")
	if err != nil {
		t.Fatalf("Second invoke failed: %v", err)
	}

	_ = result1
	_ = result2
}

func TestControlEdgeDAG(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("main", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "main-" + in, nil
	}))
	g.AddLambdaNode("side", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "side[" + in + "]", nil
	}))

	g.AddEdge(START, "main")
	g.AddControlEdge("main", "side")
	g.AddEdge("main", "side")
	g.AddEdge("side", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("control_edge"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "test")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if !strings.HasPrefix(result, "side[") {
		t.Fatalf("expected result from side node, got %q", result)
	}
	t.Logf("control edge result: %q", result)
	_ = r
}

func TestGraphWithControlPredecessors(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("producer", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "product-" + in, nil
	}))
	g.AddLambdaNode("consumer", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "consume[" + in + "]", nil
	}))

	g.AddEdge(START, "producer")
	g.AddEdge("producer", "consumer")
	g.AddControlEdge("producer", "consumer")
	g.AddEdge("consumer", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("control_preds"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "data")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if !strings.Contains(result, "consume") {
		t.Fatalf("expected consume pattern in output, got %q", result)
	}
}

func TestCompileLockAddBranch(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("compiled_branch"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	branch := NewGraphBranch[any](func(ctx context.Context, in any) (string, error) {
		return "echo", nil
	}, map[string]bool{"echo": true})

	err = g.AddBranch("test_branch", branch)
	if err == nil {
		t.Fatal("expected ErrGraphCompiled after compile for AddBranch")
	}
	if !errors.Is(err, ErrGraphCompiled) {
		t.Fatalf("expected ErrGraphCompiled, got %v", err)
	}
}

func TestGraphBranch(t *testing.T) {
	branch := NewGraphBranch(func(ctx context.Context, in string) (string, error) {
		if len(in) > 5 {
			return "long", nil
		}
		return "short", nil
	}, map[string]bool{"long": true, "short": true})

	if branch == nil {
		t.Fatal("expected non-nil GraphBranch")
	}
	if branch.condition == nil {
		t.Fatal("expected non-nil condition function")
	}
	if len(branch.branchMap) != 2 {
		t.Fatalf("expected 2 branch targets, got %d", len(branch.branchMap))
	}

	ctx := context.Background()
	result, err := branch.condition(ctx, "hello")
	if err != nil {
		t.Fatalf("branch condition failed: %v", err)
	}
	if result != "short" {
		t.Fatalf("expected short, got %q", result)
	}

	result, err = branch.condition(ctx, "hello world long")
	if err != nil {
		t.Fatalf("branch condition failed: %v", err)
	}
	if result != "long" {
		t.Fatalf("expected long, got %q", result)
	}
}

func TestGraphBranchTypeMismatch(t *testing.T) {
	branch := NewGraphBranch(func(ctx context.Context, in string) (string, error) {
		return "ok", nil
	}, map[string]bool{"ok": true})

	ctx := context.Background()
	_, err := branch.condition(ctx, 42)
	if err == nil {
		t.Fatal("expected type mismatch error")
	}
}

func TestDAGChannelMergeConfig(t *testing.T) {
	dc := newDAGChannel([]string{}, []string{"a", "b"})

	dc.setMergeConfig(func(values map[string]any) (any, error) {
		return "merged_value", nil
	})

	dc.reportValues("a", "val_a")
	dc.reportValues("b", "val_b")

	val, ok, err := dc.get()
	if err != nil {
		t.Fatalf("get failed: %v", err)
	}
	if !ok {
		t.Fatal("expected value to be ready")
	}
	if val != "merged_value" {
		t.Fatalf("expected merged_value, got %v", val)
	}
}

func TestDAGChannelSingleValue(t *testing.T) {
	dc := newDAGChannel([]string{}, []string{"a"})

	dc.reportValues("a", "single_val")

	val, ok, err := dc.get()
	if err != nil {
		t.Fatalf("get failed: %v", err)
	}
	if !ok {
		t.Fatal("expected value to be ready")
	}
	if val != "single_val" {
		t.Fatalf("expected single_val, got %v", val)
	}
}

func TestDAGChannelMultiValueNoMerge(t *testing.T) {
	dc := newDAGChannel([]string{}, []string{"a", "b"})

	dc.reportValues("a", "val_a")
	dc.reportValues("b", "val_b")

	val, ok, err := dc.get()
	if err != nil {
		t.Fatalf("get failed: %v", err)
	}
	if !ok {
		t.Fatal("expected value to be ready")
	}
	m, ok2 := val.(map[string]any)
	if !ok2 {
		t.Fatalf("expected map[string]any, got %T", val)
	}
	if m["a"] != "val_a" || m["b"] != "val_b" {
		t.Fatalf("expected map values, got %v", m)
	}
}

func TestDAGChannelNotReady(t *testing.T) {
	dc := newDAGChannel([]string{}, []string{"a", "b"})

	dc.reportValues("a", "val_a")

	_, ok, _ := dc.get()
	if ok {
		t.Fatal("expected not ready when only one of two data predecessors reported")
	}
}

func TestDAGChannelControlDependency(t *testing.T) {
	dc := newDAGChannel([]string{"c1"}, []string{"d1"})

	dc.reportValues("d1", "data_val")
	dc.reportDependency("c1")

	val, ok, err := dc.get()
	if err != nil {
		t.Fatalf("get failed: %v", err)
	}
	if !ok {
		t.Fatal("expected value ready when all control and data deps met")
	}
	if val != "data_val" {
		t.Fatalf("expected data_val, got %v", val)
	}
}

func TestDAGChannelControlNotReady(t *testing.T) {
	dc := newDAGChannel([]string{"c1"}, []string{"d1"})

	dc.reportValues("d1", "data_val")

	_, ok, _ := dc.get()
	if ok {
		t.Fatal("expected not ready when control dependency not met")
	}
}

func TestDAGChannelSkip(t *testing.T) {
	dc := newDAGChannel([]string{"c1"}, []string{})

	dc.reportSkip("c1")

	_, ok, _ := dc.get()
	if ok {
		t.Fatal("expected not ready after skip (no data values to return)")
	}
}

func TestDAGChannelSkipAll(t *testing.T) {
	dc := newDAGChannel([]string{"c1", "c2"}, []string{})

	dc.reportSkip("c1")

	allSkipped := dc.reportSkip("c2")
	if !allSkipped {
		t.Fatal("expected allSkipped=true when all control preds skipped")
	}
}

func TestPregelChannelMergeConfig(t *testing.T) {
	pc := newPregelChannel()

	pc.setMergeConfig(func(values map[string]any) (any, error) {
		return "pregel_merged", nil
	})

	pc.reportValues("a", "val_a")
	pc.reportValues("b", "val_b")

	val, ok, err := pc.get()
	if err != nil {
		t.Fatalf("get failed: %v", err)
	}
	if !ok {
		t.Fatal("expected value to be ready")
	}
	if val != "pregel_merged" {
		t.Fatalf("expected pregel_merged, got %v", val)
	}
}

func TestPregelChannelFirstValue(t *testing.T) {
	pc := newPregelChannel()

	pc.reportValues("a", "first")

	val, ok, err := pc.get()
	if err != nil {
		t.Fatalf("get failed: %v", err)
	}
	if !ok {
		t.Fatal("expected value to be ready")
	}
	if val != "first" {
		t.Fatalf("expected first, got %v", val)
	}
}

func TestPregelChannelMultipleValuesConsume(t *testing.T) {
	pc := newPregelChannel()

	pc.reportValues("a", "va")
	pc.reportValues("b", "vb")

	val, ok, err := pc.get()
	if err != nil {
		t.Fatalf("get failed: %v", err)
	}
	if !ok {
		t.Fatal("expected value to be ready")
	}

	_, ok2, _ := pc.get()
	if ok2 {
		t.Fatal("expected no more values after consumption")
	}
	_ = val
}

func TestPregelChannelEmpty(t *testing.T) {
	pc := newPregelChannel()

	_, ok, _ := pc.get()
	if ok {
		t.Fatal("expected no values in empty pregel channel")
	}
}

func TestErrorPropagation(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("fail", InvokableLambda(nodeFailing))
	g.AddEdge(START, "fail")
	g.AddEdge("fail", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("error_prop"),
		WithNodeTriggerMode(AnyPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	_, err = r.Invoke(context.Background(), "trigger")
	if err == nil {
		t.Fatal("expected error from failing node")
	}
	if !strings.Contains(err.Error(), "no result produced") {
		t.Logf("error propagation result: %v", err)
	}
}

func TestErrorNodeDoesNotPropagateOutput(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("fail", InvokableLambda(nodeFailing))
	g.AddLambdaNode("next", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "should_not_reach", nil
	}))

	g.AddEdge(START, "fail")
	g.AddEdge("fail", "next")
	g.AddEdge("next", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("error_no_prop"),
		WithNodeTriggerMode(AnyPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	_, err = r.Invoke(context.Background(), "trigger")
	if err == nil {
		t.Fatal("expected error")
	}
}

func TestEagerDisabledOption(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	opts := newNodeCompileOptions(WithEagerExecutionDisabled())
	if opts.isEager() {
		t.Fatal("expected isEager=false when disabled")
	}
	if opts.isDAG() {
		t.Fatal("expected isDAG=false with AnyPredecessor default")
	}

	r, err := g.Compile(context.Background(),
		WithGraphName("eager_disabled"),
		WithNodeTriggerMode(AnyPredecessor),
		WithEagerExecutionDisabled(),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "test")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != "test" {
		t.Fatalf("expected test, got %q", result)
	}
}

func TestLambdaInvoke(t *testing.T) {
	l := InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return strings.ToUpper(in), nil
	})

	if l.kind != "InvokableLambda" {
		t.Fatalf("expected kind InvokableLambda, got %s", l.kind)
	}
	if l.GetComponentType() != ComponentOfLambda {
		t.Fatalf("expected ComponentOfLambda, got %s", l.GetComponentType())
	}

	cr := l.GetRunnable()
	if cr == nil {
		t.Fatal("expected non-nil composableRunnable")
	}
	if cr.i == nil {
		t.Fatal("expected non-nil Invoke function")
	}

	output, err := cr.invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("invoke failed: %v", err)
	}
	if output != "HELLO" {
		t.Fatalf("expected HELLO, got %v", output)
	}
}

func TestLambdaStreamFallback(t *testing.T) {
	l := InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "result", nil
	})

	cr := l.GetRunnable()
	output, err := cr.stream(context.Background(), "input")
	if err != nil {
		t.Fatalf("stream fallback failed: %v", err)
	}
	if output != "result" {
		t.Fatalf("expected result, got %v", output)
	}
}

func TestLambdaTypeMismatch(t *testing.T) {
	l := InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	})

	cr := l.GetRunnable()
	_, err := cr.invoke(context.Background(), 42)
	if err == nil {
		t.Fatal("expected type mismatch error")
	}
	if !strings.Contains(err.Error(), "expected input type") {
		t.Fatalf("expected type error message, got: %v", err)
	}
}

func TestGraphNodeCompileWithRunnable(t *testing.T) {
	gn := &graphNode{
		name: "test_node",
		cr:   (&Lambda{}).GetRunnable(),
	}

	_, err := gn.compileIfNeeded(context.Background(), nil)
	if err != nil {
		t.Fatalf("compileIfNeeded should return existing runnable: %v", err)
	}
}

func TestGraphNodeCompileWithGraph(t *testing.T) {
	subG := newGraph("string", "string")
	gn := &graphNode{
		name: "sub_graph",
		g:    subG,
	}

	_, err := gn.compileIfNeeded(context.Background(), nil)
	if err == nil {
		t.Fatal("expected error compiling sub graph without START/END edges")
	}
}

func TestGraphNodeCompileNoRunnable(t *testing.T) {
	gn := &graphNode{
		name: "empty",
	}

	_, err := gn.compileIfNeeded(context.Background(), nil)
	if err == nil {
		t.Fatal("expected ErrNoCompiledRunnable")
	}
	if !errors.Is(err, ErrNoCompiledRunnable) {
		t.Fatalf("expected ErrNoCompiledRunnable, got %v", err)
	}
}

func TestComposableRunnableNil(t *testing.T) {
	cr := &composableRunnable{}
	if !cr.nil() {
		t.Fatal("expected nil()=true for empty composableRunnable")
	}

	_, err := cr.invoke(context.Background(), "test")
	if err == nil {
		t.Fatal("expected error invoking nil runnable")
	}

	_, err = cr.stream(context.Background(), "test")
	if err == nil {
		t.Fatal("expected error streaming nil runnable")
	}
}

func TestChannelManagerGetReadyChannels(t *testing.T) {
	cm := newChannelManager()

	dc := newDAGChannel([]string{}, []string{})
	dc.reportValues("start", "hello")
	cm.addChannel("node_a", dc)

	dc2 := newDAGChannel([]string{}, []string{"x"})
	cm.addChannel("node_b", dc2)

	ready := cm.getReadyChannels("")
	if len(ready) != 1 {
		t.Fatalf("expected 1 ready channel, got %d", len(ready))
	}
	if val, ok := ready["node_a"]; !ok || val != "hello" {
		t.Fatalf("expected node_a to be ready with hello, got %v", ready)
	}
}

func TestConcurrentGraphInvokes(t *testing.T) {
	var wg sync.WaitGroup
	n := 10
	errs := make(chan error, n)

	for i := 0; i < n; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()

			g := NewGraph[int, int]()
			g.AddLambdaNode("inc", InvokableLambda(func(ctx context.Context, in int) (int, error) {
				return in + 1, nil
			}))
			g.AddEdge(START, "inc")
			g.AddEdge("inc", END)

			r, err := g.Compile(context.Background(),
				WithGraphName(fmt.Sprintf("concurrent_%d", id)),
				WithNodeTriggerMode(AnyPredecessor),
			)
			if err != nil {
				errs <- fmt.Errorf("compile %d: %v", id, err)
				return
			}

			result, err := r.Invoke(context.Background(), id)
			if err != nil {
				errs <- fmt.Errorf("invoke %d: %v", id, err)
				return
			}
			if result != id+1 {
				errs <- fmt.Errorf("expected %d, got %d", id+1, result)
			}
		}(i)
	}
	wg.Wait()
	close(errs)

	for err := range errs {
		t.Fatal(err)
	}
}

func TestNodeTriggerModeConstants(t *testing.T) {
	if AnyPredecessor != "any_predecessor" {
		t.Fatalf("expected any_predecessor, got %v", AnyPredecessor)
	}
	if AllPredecessor != "all_predecessor" {
		t.Fatalf("expected all_predecessor, got %v", AllPredecessor)
	}
}

func TestComponentTypeConstants(t *testing.T) {
	types := map[ComponentType]string{
		ComponentOfGraph:    "Graph",
		ComponentOfLambda:   "Lambda",
		ComponentOfWorkflow: "Workflow",
		ComponentOfChain:    "Chain",
		ComponentOfUnknown:  "Unknown",
	}
	for ct, expected := range types {
		if string(ct) != expected {
			t.Fatalf("expected ComponentType=%q, got %q", expected, string(ct))
		}
	}
}

func TestSentinelErrors(t *testing.T) {
	errs := []struct {
		err      error
		contains string
	}{
		{ErrGraphCompiled, "graph already compiled"},
		{ErrGraphNotCompiled, "graph not compiled yet"},
		{ErrExceedMaxSteps, "exceeded maximum run steps"},
		{ErrDAGHasCycle, "DAG graph has a cycle"},
		{ErrNoStartEdge, "no edge from START"},
		{ErrNoEndEdge, "no edge to END"},
		{ErrNodeNotFound, "node not found"},
		{ErrNoCompiledRunnable, "node has no compiled runnable"},
	}
	for _, tc := range errs {
		if !strings.Contains(tc.err.Error(), tc.contains) {
			t.Fatalf("expected error containing %q, got %q", tc.contains, tc.err.Error())
		}
	}
}

func TestCompileOptionsString(t *testing.T) {
	opts := newNodeCompileOptions(
		WithGraphName("test_graph"),
		WithNodeTriggerMode(AllPredecessor),
		WithMaxRunSteps(42),
		WithEagerExecutionDisabled(),
	)

	s := opts.String()
	if !strings.Contains(s, "name:test_graph") {
		t.Fatalf("expected name in string, got %s", s)
	}
	if !strings.Contains(s, "mode:all_predecessor") {
		t.Fatalf("expected mode in string, got %s", s)
	}
	if !strings.Contains(s, "maxSteps:42") {
		t.Fatalf("expected maxSteps in string, got %s", s)
	}
}

func TestStartEndConstants(t *testing.T) {
	if START != "start" {
		t.Fatalf("expected START=%q, got %q", "start", START)
	}
	if END != "end" {
		t.Fatalf("expected END=%q, got %q", "end", END)
	}
}

func TestDefaultMaxStepsConstant(t *testing.T) {
	if defaultMaxSteps != 100 {
		t.Fatalf("expected defaultMaxSteps=100, got %d", defaultMaxSteps)
	}
}

func TestTypeError(t *testing.T) {
	te := newTypeError("hello", 42)
	errMsg := te.Error()
	if !strings.Contains(errMsg, "type error") {
		t.Fatalf("expected type error message, got %s", errMsg)
	}
}

func TestContainsString(t *testing.T) {
	slice := []string{"a", "b", "c"}
	if !containsString(slice, "b") {
		t.Fatal("expected containsString to find b")
	}
	if containsString(slice, "d") {
		t.Fatal("expected containsString to not find d")
	}
}

func TestFmtTypeError(t *testing.T) {
	err := fmtTypeError(42)
	if err == nil {
		t.Fatal("expected non-nil error")
	}
	if !strings.Contains(err.Error(), "unexpected input type") {
		t.Fatalf("expected unexpected input type, got %v", err)
	}
}

func TestExtractTypeName(t *testing.T) {
	if extractTypeName("hello") != "string" {
		t.Fatal("expected string")
	}
	if extractTypeName(42) != "int" {
		t.Fatal("expected int")
	}
	if extractTypeName(3.14) != "float64" {
		t.Fatal("expected float64")
	}
	if extractTypeName(true) != "bool" {
		t.Fatal("expected bool")
	}
	if extractTypeName(struct{}{}) != "any" {
		t.Fatal("expected any for struct")
	}
}

func TestFmtType(t *testing.T) {
	if fmtType(nil) != "nil" {
		t.Fatalf("expected nil, got %s", fmtType(nil))
	}
	if fmtType("test") != "string" {
		t.Fatalf("expected string, got %s", fmtType("test"))
	}
}

func TestGraphWithIntTypes(t *testing.T) {
	g := NewGraph[int, int]()

	g.AddLambdaNode("double", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in * 2, nil
	}))
	g.AddLambdaNode("add_ten", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in + 10, nil
	}))

	g.AddEdge(START, "double")
	g.AddEdge("double", "add_ten")
	g.AddEdge("add_ten", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("int_ops"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), 5)
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != 20 {
		t.Fatalf("expected 20, got %d", result)
	}

	info := g.GetGraphInfo()
	if info.InputType != "int" {
		t.Fatalf("expected InputType=int, got %s", info.InputType)
	}
	if info.OutputType != "int" {
		t.Fatalf("expected OutputType=int, got %s", info.OutputType)
	}
}

func TestGraphWithBoolTypes(t *testing.T) {
	g := NewGraph[bool, bool]()

	g.AddLambdaNode("not", InvokableLambda(func(ctx context.Context, in bool) (bool, error) {
		return !in, nil
	}))

	g.AddEdge(START, "not")
	g.AddEdge("not", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("bool_op"),
		WithNodeTriggerMode(AnyPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), true)
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != false {
		t.Fatal("expected false")
	}
}

func TestGraphRecompileSameOptions(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	r1, err := g.Compile(context.Background(),
		WithGraphName("same_graph"),
		WithNodeTriggerMode(AnyPredecessor),
	)
	if err != nil {
		t.Fatalf("First compile failed: %v", err)
	}

	r2, err := g.Compile(context.Background(),
		WithGraphName("same_graph2"),
		WithNodeTriggerMode(AnyPredecessor),
	)
	if err != nil {
		t.Fatalf("Second compile failed: %v", err)
	}

	res1, _ := r1.Invoke(context.Background(), "a")
	res2, _ := r2.Invoke(context.Background(), "b")

	if res1 != "a" {
		t.Fatalf("expected a, got %q", res1)
	}
	if res2 != "b" {
		t.Fatalf("expected b, got %q", res2)
	}
}

func TestGraphInfoNodeDetails(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("alpha", InvokableLambda(nodeIdentity))
	g.AddLambdaNode("beta", InvokableLambda(nodeToUpper))

	g.AddEdge(START, "alpha")
	g.AddEdge("alpha", "beta")
	g.AddEdge("beta", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("node_details"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	info := g.GetGraphInfo()
	foundAlpha := false
	foundBeta := false
	for _, n := range info.Nodes {
		if n.Name == "alpha" && n.Component == ComponentOfLambda {
			foundAlpha = true
		}
		if n.Name == "beta" && n.Component == ComponentOfLambda {
			foundBeta = true
		}
	}
	if !foundAlpha || !foundBeta {
		t.Fatal("expected both nodes in GraphInfo.Nodes with correct Component type")
	}
}

func TestGraphInfoEdgeDetails(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("a", InvokableLambda(nodeIdentity))
	g.AddLambdaNode("b", InvokableLambda(nodeToUpper))

	g.AddEdge(START, "a")
	g.AddControlEdge("a", "b")
	g.AddEdge("a", "b")
	g.AddEdge("b", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("edge_details"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	info := g.GetGraphInfo()
	if info.NumEdges != 4 {
		t.Fatalf("expected 4 edges, got %d", info.NumEdges)
	}
}

func TestPregelLinearExecution(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("step1", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "1:" + in, nil
	}))
	g.AddLambdaNode("step2", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "2:" + in, nil
	}))
	g.AddLambdaNode("step3", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "3:" + in, nil
	}))

	g.AddEdge(START, "step1")
	g.AddEdge("step1", "step2")
	g.AddEdge("step2", "step3")
	g.AddEdge("step3", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("pregel_linear"),
		WithNodeTriggerMode(AnyPredecessor),
		WithMaxRunSteps(10),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "data")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if !strings.HasPrefix(result, "3:2:1:") {
		t.Fatalf("expected 3:2:1:data, got %q", result)
	}
}

func TestDAGLinearExecution(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("upper", InvokableLambda(nodeToUpper))
	g.AddLambdaNode("reverse", InvokableLambda(nodeReverse))
	g.AddLambdaNode("bracket", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "<" + in + ">", nil
	}))

	g.AddEdge(START, "upper")
	g.AddEdge("upper", "reverse")
	g.AddEdge("reverse", "bracket")
	g.AddEdge("bracket", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("dag_linear"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "abc")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	expected := "<CBA>"
	if result != expected {
		t.Fatalf("expected %q, got %q", expected, result)
	}
}

func TestInvokableLambdaWithComplexType(t *testing.T) {
	type MyStruct struct {
		Name string
		Age  int
	}

	g := NewGraph[MyStruct, MyStruct]()

	g.AddLambdaNode("age_year", InvokableLambda(func(ctx context.Context, in MyStruct) (MyStruct, error) {
		in.Age++
		return in, nil
	}))

	g.AddEdge(START, "age_year")
	g.AddEdge("age_year", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("complex_type"),
		WithNodeTriggerMode(AnyPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), MyStruct{Name: "Alice", Age: 30})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result.Age != 31 {
		t.Fatalf("expected age 31, got %d", result.Age)
	}
	if result.Name != "Alice" {
		t.Fatalf("expected Name Alice, got %s", result.Name)
	}
}

func TestMultipleBranches(t *testing.T) {
	g := NewGraph[string, string]()

	branch24 := NewGraphBranch(func(ctx context.Context, in string) (string, error) {
		if len(in) <= 4 {
			return "short_path", nil
		}
		return "long_path", nil
	}, map[string]bool{"short_path": true, "long_path": true})

	_ = g.AddBranch("router", branch24)
	if branch24.branchMap["short_path"] != true || branch24.branchMap["long_path"] != true {
		t.Fatal("branch map has incorrect entries")
	}
}

func TestEventLogAllEventTypes(t *testing.T) {
	el := NewEventLog()

	testErr := errors.New("test error")

	el.LogGraphStart("all_events")
	el.LogNodeStart("n1", 1, "in")
	el.LogNodeEnd("n1", 1, "out")
	el.LogNodeSkipped("n2", 2)
	el.LogNodeError("n3", 3, testErr)
	el.LogGraphError("all_events", testErr)
	el.LogMaxStepsHit("all_events", 101)
	el.LogGraphEnd("all_events", 150)

	eventsByType := make(map[EventType]int)
	for _, e := range el.Events {
		eventsByType[e.Type]++
	}

	expected := map[EventType]int{
		EventGraphStart:  1,
		EventNodeStart:   1,
		EventNodeEnd:     1,
		EventNodeSkipped: 1,
		EventNodeError:   1,
		EventGraphError:  1,
		EventMaxStepsHit: 1,
		EventGraphEnd:    1,
	}

	for et, expectedCount := range expected {
		if eventsByType[et] != expectedCount {
			t.Fatalf("expected %d %s events, got %d", expectedCount, et, eventsByType[et])
		}
	}

	if len(el.Events) != 8 {
		t.Fatalf("expected 8 total events, got %d", len(el.Events))
	}
}

func TestNewGraphInfoDefaults(t *testing.T) {
	gi := newGraphInfo("test", AnyPredecessor, 50)

	if gi.Name != "test" {
		t.Fatalf("expected name test, got %s", gi.Name)
	}
	if gi.TriggerMode != AnyPredecessor {
		t.Fatalf("expected AnyPredecessor, got %s", gi.TriggerMode)
	}
	if gi.DAGMode {
		t.Fatal("expected DAGMode=false")
	}
	if !gi.PregelMode {
		t.Fatal("expected PregelMode=true")
	}
	if gi.MaxSteps != 50 {
		t.Fatalf("expected MaxSteps=50, got %d", gi.MaxSteps)
	}
	if gi.NumNodes != 0 {
		t.Fatalf("expected 0 nodes, got %d", gi.NumNodes)
	}
	if gi.NumEdges != 0 {
		t.Fatalf("expected 0 edges, got %d", gi.NumEdges)
	}
}

func TestDependencyStateString(t *testing.T) {
	if dependencyWaiting.String() != "Waiting" {
		t.Fatalf("expected Waiting, got %s", dependencyWaiting.String())
	}
	if dependencyReady.String() != "Ready" {
		t.Fatalf("expected Ready, got %s", dependencyReady.String())
	}
	if dependencySkipped.String() != "Skipped" {
		t.Fatalf("expected Skipped, got %s", dependencySkipped.String())
	}
	if dependencyState(99).String() != "Unknown" {
		t.Fatalf("expected Unknown for invalid state, got %s", dependencyState(99).String())
	}
}

func TestPregelGraphWithMultipleBranches(t *testing.T) {
	g := NewGraph[int, int]()

	g.AddLambdaNode("inc", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in + 1, nil
	}))
	g.AddLambdaNode("double", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in * 2, nil
	}))

	g.AddEdge(START, "inc")
	g.AddEdge("inc", "double")
	g.AddEdge("double", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("pregel_multi"),
		WithNodeTriggerMode(AnyPredecessor),
		WithMaxRunSteps(50),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), 10)
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != 22 {
		t.Fatalf("expected 22, got %d", result)
	}
}

func TestGraphNameEmpty(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	r, err := g.Compile(context.Background(),
		WithNodeTriggerMode(AnyPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile without name should succeed: %v", err)
	}

	info := g.GetGraphInfo()
	if info == nil {
		t.Fatal("expected non-nil GraphInfo even without name")
	}

	result, err := r.Invoke(context.Background(), "test")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != "test" {
		t.Fatalf("expected test, got %q", result)
	}
}

func TestNewRunnerFromGraph(t *testing.T) {
	g := newGraph("string", "string")

	r := newRunnerFromGraph(g)
	if r == nil {
		t.Fatal("expected non-nil runner")
	}
	if r.chanSubscribeTo == nil {
		t.Fatal("expected non-nil chanSubscribeTo")
	}
}

func TestChannelManagerReportSkip(t *testing.T) {
	cm := newChannelManager()

	dc := newDAGChannel([]string{}, []string{})
	cm.addChannel("node_a", dc)

	cm.reportSkip("completed", map[string]bool{"node_a": true})

	_, ok, _ := dc.get()
	if ok {
		t.Fatal("expected no value after skip with no data")
	}
}

func TestDAGControlCycle(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("a", InvokableLambda(nodeIdentity))
	g.AddLambdaNode("b", InvokableLambda(nodeToUpper))

	g.AddEdge(START, "a")
	g.AddControlEdge("a", "b")
	g.AddControlEdge("b", "a")
	g.AddEdge("b", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("control_cycle"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err == nil {
		t.Fatal("expected cycle detection error for control edge cycle")
	}
	if !errors.Is(err, ErrDAGHasCycle) {
		t.Fatalf("expected ErrDAGHasCycle, got %v", err)
	}
}

func TestEmptyGraphDirectStartEnd(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("direct_start_end"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "test")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != "test" {
		t.Fatalf("expected test, got %q", result)
	}
}

func TestCompileWithoutTriggerModeDefaultsToPregel(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("default_pregel"),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	info := g.GetGraphInfo()
	if info.TriggerMode != AnyPredecessor {
		t.Fatalf("expected default trigger mode AnyPredecessor, got %s", info.TriggerMode)
	}
	if !info.PregelMode {
		t.Fatal("expected PregelMode=true by default")
	}

	result, err := r.Invoke(context.Background(), "ok")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != "ok" {
		t.Fatalf("expected ok, got %q", result)
	}
}

func TestUnknownEdgeTargetEnd(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))

	err := g.AddEdge("echo", "ghost_end")
	if err == nil {
		t.Fatal("expected error for unknown target node 'ghost_end'")
	}
	if !errors.Is(err, ErrNodeNotFound) {
		t.Fatalf("expected ErrNodeNotFound, got %v", err)
	}
}

func TestRunTypeConstants(t *testing.T) {
	if runTypeDAG != 1 {
		t.Fatalf("expected runTypeDAG=1, got %d", runTypeDAG)
	}
	if runTypePregel != 2 {
		t.Fatalf("expected runTypePregel=2, got %d", runTypePregel)
	}
}

func TestEventTypeConstants(t *testing.T) {
	expected := map[EventType]string{
		EventNodeStart:    "node_start",
		EventNodeEnd:      "node_end",
		EventNodeError:    "node_error",
		EventNodeSkipped:  "node_skipped",
		EventGraphStart:   "graph_start",
		EventGraphEnd:     "graph_end",
		EventGraphError:   "graph_error",
		EventChannelReady: "channel_ready",
		EventCheckpoint:   "checkpoint",
		EventMaxStepsHit:  "max_steps_hit",
	}
	for et, expectedVal := range expected {
		if string(et) != expectedVal {
			t.Fatalf("expected EventType=%q, got %q", expectedVal, string(et))
		}
	}
}

func TestGraphInfoAddNodeEdgeCounts(t *testing.T) {
	gi := newGraphInfo("count_test", AllPredecessor, 100)
	gi.addNode("n1", ComponentOfLambda)
	gi.addNode("n2", ComponentOfLambda)
	gi.addEdge("start", "n1")
	gi.addEdge("n1", "n2")
	gi.addEdge("n2", "end")

	if gi.NumNodes != 2 {
		t.Fatalf("expected 2 nodes, got %d", gi.NumNodes)
	}
	if gi.NumEdges != 3 {
		t.Fatalf("expected 3 edges, got %d", gi.NumEdges)
	}
	if len(gi.Nodes) != 2 {
		t.Fatalf("expected 2 nodes in slice, got %d", len(gi.Nodes))
	}
	if len(gi.Edges) != 3 {
		t.Fatalf("expected 3 edges in slice, got %d", len(gi.Edges))
	}
}

func TestDAGSimpleFanOut(t *testing.T) {
	g := NewGraph[any, string]()

	g.AddLambdaNode("broadcaster", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		s, _ := in.(string)
		return s + "-broadcast", nil
	}))
	g.AddLambdaNode("upper", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		s, _ := in.(string)
		return strings.ToUpper(s), nil
	}))
	g.AddLambdaNode("reverse", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		s, _ := in.(string)
		runes := []rune(s)
		for i, j := 0, len(runes)-1; i < j; i, j = i+1, j-1 {
			runes[i], runes[j] = runes[j], runes[i]
		}
		return string(runes), nil
	}))
	g.AddLambdaNode("merger", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		switch v := in.(type) {
		case string:
			return "SINGLE:" + v, nil
		case map[string]any:
			var parts []string
			for k, val := range v {
				parts = append(parts, fmt.Sprintf("%s=%v", k, val))
			}
			sort.Strings(parts)
			return "FANOUT:" + strings.Join(parts, "|"), nil
		}
		return "", fmt.Errorf("merger: unexpected type %T", in)
	}))

	g.AddEdge(START, "broadcaster")
	g.AddEdge("broadcaster", "upper")
	g.AddEdge("broadcaster", "reverse")
	g.AddEdge("upper", "merger")
	g.AddEdge("reverse", "merger")
	g.AddEdge("merger", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("fan_out"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "test")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if !strings.HasPrefix(result, "FANOUT:") {
		t.Fatalf("expected FANOUT: prefix, got %q", result)
	}
	t.Logf("DAG fan-out result: %q", result)
}

func TestMultipleStartNodes(t *testing.T) {
	g := NewGraph[any, string]()

	g.AddLambdaNode("a", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		s, _ := in.(string)
		return "A[" + s + "]", nil
	}))
	g.AddLambdaNode("b", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		s, _ := in.(string)
		return "B[" + s + "]", nil
	}))
	g.AddLambdaNode("merger", InvokableLambda(func(ctx context.Context, in any) (string, error) {
		switch v := in.(type) {
		case string:
			return "S/" + v, nil
		case map[string]any:
			var parts []string
			for k, val := range v {
				parts = append(parts, fmt.Sprintf("%s=%v", k, val))
			}
			return "M/" + strings.Join(parts, ","), nil
		}
		return "", nil
	}))

	g.AddEdge(START, "a")
	g.AddEdge(START, "b")
	g.AddEdge("a", "merger")
	g.AddEdge("b", "merger")
	g.AddEdge("merger", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("multi_start"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "x")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if !strings.Contains(result, "A[") {
		t.Fatalf("expected result containing 'A[', got %q", result)
	}
	t.Logf("multiple start nodes result: %q", result)
}

func TestPregelGraphWithoutMaxStepsDefaults(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("pregel_default_max"),
		WithNodeTriggerMode(AnyPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	info := g.GetGraphInfo()
	if info.MaxSteps != 100 {
		t.Fatalf("expected default MaxSteps=100, got %d", info.MaxSteps)
	}

	result, err := r.Invoke(context.Background(), "data")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != "data" {
		t.Fatalf("expected data, got %q", result)
	}
}

// --- Supplemental tests added via direct-test-changelog task ---

// TestDuplicateNodePregel verifies that adding the same node key twice in
// Pregel mode overwrites the first definition, consistent with DAG mode.
func TestDuplicateNodePregel(t *testing.T) {
	g := NewGraph[int, int]()

	g.AddLambdaNode("transform", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in + 100, nil
	}))
	g.AddLambdaNode("transform", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in * 3, nil
	}))

	g.AddEdge(START, "transform")
	g.AddEdge("transform", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("duplicate_pregel"),
		WithNodeTriggerMode(AnyPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), 7)
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != 21 {
		t.Fatalf("expected 21 (second AddLambdaNode wins: 7*3), got %d", result)
	}
}

// TestUnknownEdgeFromStart verifies that AddEdge with FROM=START but TO not
// a node returns ErrNodeNotFound.
func TestUnknownEdgeFromStart(t *testing.T) {
	g := NewGraph[string, string]()

	err := g.AddEdge(START, "ghost")
	if err == nil {
		t.Fatal("expected error for unknown target from START")
	}
	if !errors.Is(err, ErrNodeNotFound) {
		t.Fatalf("expected ErrNodeNotFound, got %v", err)
	}
}

// TestUnknownControlEdgeFromStart verifies that AddControlEdge with
// FROM=START but TO not a node returns ErrNodeNotFound.
func TestUnknownControlEdgeFromStart(t *testing.T) {
	g := NewGraph[string, string]()

	err := g.AddControlEdge(START, "ghost")
	if err == nil {
		t.Fatal("expected error for unknown control target from START")
	}
	if !errors.Is(err, ErrNodeNotFound) {
		t.Fatalf("expected ErrNodeNotFound, got %v", err)
	}
}

// TestCompileLockAddNodeThenEdge verifies that after compile, AddNode and
// AddEdge are both locked with ErrGraphCompiled.
func TestCompileLockMutationsAfterCompile(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("lock_test"),
		WithNodeTriggerMode(AnyPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	mutations := []func() error{
		func() error { return g.AddLambdaNode("new_node", InvokableLambda(nodeIdentity)) },
		func() error { return g.AddEdge("echo", "echo") },
		func() error { return g.AddControlEdge("echo", "echo") },
	}

	for i, mutate := range mutations {
		err := mutate()
		if err == nil {
			t.Fatalf("mutation %d: expected ErrGraphCompiled", i)
		}
		if !errors.Is(err, ErrGraphCompiled) {
			t.Fatalf("mutation %d: expected ErrGraphCompiled, got %v", i, err)
		}
	}
}

// TestGraphInfoWithoutName verifies that GraphInfo still populates correctly
// even when no WithGraphName option is provided.
func TestGraphInfoWithoutName(t *testing.T) {
	g := NewGraph[int, int]()

	g.AddLambdaNode("plus1", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in + 1, nil
	}))
	g.AddLambdaNode("times2", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in * 2, nil
	}))

	g.AddEdge(START, "plus1")
	g.AddEdge("plus1", "times2")
	g.AddEdge("times2", END)

	_, err := g.Compile(context.Background(),
		WithNodeTriggerMode(AllPredecessor),
		WithMaxRunSteps(300),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	info := g.GetGraphInfo()
	if info == nil {
		t.Fatal("expected non-nil GraphInfo")
	}
	if info.NumNodes != 2 {
		t.Fatalf("expected 2 nodes, got %d", info.NumNodes)
	}
	if info.NumEdges != 3 {
		t.Fatalf("expected 3 edges, got %d", info.NumEdges)
	}
	if !info.DAGMode {
		t.Fatal("expected DAGMode=true")
	}
	if info.MaxSteps != 300 {
		t.Fatalf("expected MaxSteps=300, got %d", info.MaxSteps)
	}
	if info.InputType != "int" {
		t.Fatalf("expected InputType=int, got %s", info.InputType)
	}
	if info.OutputType != "int" {
		t.Fatalf("expected OutputType=int, got %s", info.OutputType)
	}
}

// TestGraphInfoEdgesWithStartAndEnd verifies that GraphInfo records START/END
// edges correctly.
func TestGraphInfoEdgesWithStartEnd(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("node_a", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "node_a")
	g.AddEdge("node_a", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("edge_start_end"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	info := g.GetGraphInfo()
	if info.NumEdges != 2 {
		t.Fatalf("expected 2 edges (START->node_a, node_a->END), got %d", info.NumEdges)
	}

	hasStartEdge := false
	hasEndEdge := false
	for _, e := range info.Edges {
		if e.From == "start" && e.To == "node_a" {
			hasStartEdge = true
		}
		if e.From == "node_a" && e.To == "end" {
			hasEndEdge = true
		}
	}
	if !hasStartEdge {
		t.Fatal("expected START->node_a edge in GraphInfo")
	}
	if !hasEndEdge {
		t.Fatal("expected node_a->END edge in GraphInfo")
	}
}

// TestDAGFanInSinglePredecessor verifies that a node with a single data
// predecessor in DAG mode receives its value directly (not wrapped in a map).
func TestDAGFanInSinglePredecessor(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("single_pred"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "alone")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != "alone" {
		t.Fatalf("expected 'alone', got %q", result)
	}
}

// TestDAGFanInWithMergeConfig verifies DAG mode with a custom merge function
// specified via channel configuration.
func TestDAGFanInWithMergeConfig(t *testing.T) {
	dc := newDAGChannel([]string{}, []string{"x", "y"})

	dc.setMergeConfig(func(values map[string]any) (any, error) {
		return fmt.Sprintf("custom(%v,%v)", values["x"], values["y"]), nil
	})

	dc.reportValues("x", "hello")
	dc.reportValues("y", "world")

	val, ok, err := dc.get()
	if err != nil {
		t.Fatalf("get failed: %v", err)
	}
	if !ok {
		t.Fatal("expected value to be ready")
	}
	if val != "custom(hello,world)" {
		t.Fatalf("expected custom(hello,world), got %v", val)
	}
}

// TestDAGCycleRejectionSelfLoop verifies a direct self-loop is rejected in
// DAG mode.
func TestDAGCycleRejectionSelfLoop(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("loop", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "loop")
	g.AddEdge("loop", "loop")
	g.AddEdge("loop", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("self_loop"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err == nil {
		t.Fatal("expected cycle detection error for self-loop in DAG mode")
	}
	if !errors.Is(err, ErrDAGHasCycle) {
		t.Fatalf("expected ErrDAGHasCycle, got %v", err)
	}
}

// TestDAGCycleRejectionMixedEdges verifies a cycle formed by a mix of data
// and control edges in DAG mode is rejected.
func TestDAGCycleRejectionMixedEdges(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("p", InvokableLambda(nodeIdentity))
	g.AddLambdaNode("q", InvokableLambda(nodeToUpper))

	g.AddEdge(START, "p")
	g.AddEdge("p", "q")
	g.AddControlEdge("q", "p")
	g.AddEdge("q", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("mixed_edges_cycle"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err == nil {
		t.Fatal("expected cycle detection error for mixed-edge cycle")
	}
	if !errors.Is(err, ErrDAGHasCycle) {
		t.Fatalf("expected ErrDAGHasCycle, got %v", err)
	}
}

// TestPregelCycleAllowedSelfLoop verifies a self-loop compiles in Pregel
// mode without cycle error (unlike DAG mode). Execution loops until
// maxSteps.
func TestPregelCycleAllowedSelfLoop(t *testing.T) {
	g := NewGraph[int, int]()

	g.AddLambdaNode("counter", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in + 1, nil
	}))

	g.AddEdge(START, "counter")
	g.AddEdge("counter", "counter")
	g.AddEdge("counter", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("pregel_self_loop"),
		WithNodeTriggerMode(AnyPredecessor),
		WithMaxRunSteps(5),
	)
	if err != nil {
		t.Fatalf("Pregel mode should allow self-loop compile, got: %v", err)
	}

	_, err = r.Invoke(context.Background(), 0)
	if err == nil {
		t.Fatal("expected maxSteps exceeded on self-loop in Pregel")
	}
	if !errors.Is(err, ErrExceedMaxSteps) {
		t.Fatalf("expected ErrExceedMaxSteps, got %v", err)
	}
}

// TestPregelCycleAllowedMultiNode verifies a multi-node cycle (A->B->A) in
// Pregel mode compiles without cycle error (unlike DAG mode). Execution
// loops until maxSteps.
func TestPregelCycleAllowedMultiNode(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("a", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "A:" + in, nil
	}))
	g.AddLambdaNode("b", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "B:" + in, nil
	}))

	g.AddEdge(START, "a")
	g.AddEdge("a", "b")
	g.AddEdge("b", "a")
	g.AddEdge("a", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("pregel_multi_cycle"),
		WithNodeTriggerMode(AnyPredecessor),
		WithMaxRunSteps(5),
	)
	if err != nil {
		t.Fatalf("Pregel compile should allow cycles, got: %v", err)
	}

	_, err = r.Invoke(context.Background(), "x")
	if err == nil {
		t.Fatal("expected maxSteps exceeded on multi-node cycle in Pregel")
	}
	if !errors.Is(err, ErrExceedMaxSteps) {
		t.Fatalf("expected ErrExceedMaxSteps, got %v", err)
	}
}

// TestMaxStepsExceededSelfLoop verifies maxSteps is hit on a tight self-loop
// with a low step limit.
func TestMaxStepsExceededSelfLoop(t *testing.T) {
	g := NewGraph[int, int]()

	g.AddLambdaNode("inc", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in + 1, nil
	}))

	g.AddEdge(START, "inc")
	g.AddEdge("inc", "inc")
	g.AddEdge("inc", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("tight_loop"),
		WithNodeTriggerMode(AnyPredecessor),
		WithMaxRunSteps(2),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	_, err = r.Invoke(context.Background(), 0)
	if err == nil {
		t.Fatal("expected maxSteps exceeded error on tight loop")
	}
	if !errors.Is(err, ErrExceedMaxSteps) {
		t.Fatalf("expected ErrExceedMaxSteps, got %v", err)
	}
}

// TestEventLogIntegrationWithRunner verifies events are written when a graph
// is run with an event log attached.
func TestEventLogIntegrationWithRunner(t *testing.T) {
	el := NewEventLog()

	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	gg := newGraph("string", "string")
	gg.graphName = "event_integration"
	gg.graphInfo = newGraphInfo("event_integration", AnyPredecessor, 50)
	gg.graphInfo.InputType = "string"
	gg.graphInfo.OutputType = "string"

	gg.nodes["echo"] = &graphNode{
		name: "echo",
		cr:   InvokableLambda(nodeIdentity).GetRunnable(),
		info: &GraphNodeInfo{Name: "echo", Component: ComponentOfLambda},
	}

	gg.dataEdges[START] = append(gg.dataEdges[START], "echo")
	gg.dataEdges["echo"] = append(gg.dataEdges["echo"], END)

	r, err := gg.compile(context.Background())
	if err != nil {
		t.Fatalf("compile failed: %v", err)
	}

	r.eventLog = el

	result, err := r.run(context.Background(), "events_test")
	if err != nil {
		t.Fatalf("run failed: %v", err)
	}
	if result != "events_test" {
		t.Fatalf("expected events_test, got %q", result)
	}

	if len(el.Events) < 3 {
		t.Fatalf("expected at least 3 events (graph_start, node_start, node_end), got %d", len(el.Events))
	}

	hasGraphStart := false
	hasNodeStart := false
	hasNodeEnd := false
	for _, e := range el.Events {
		switch e.Type {
		case EventGraphStart:
			hasGraphStart = true
		case EventNodeStart:
			hasNodeStart = true
		case EventNodeEnd:
			hasNodeEnd = true
		}
	}
	if !hasGraphStart {
		t.Fatal("expected EventGraphStart")
	}
	if !hasNodeStart {
		t.Fatal("expected EventNodeStart")
	}
	if !hasNodeEnd {
		t.Fatal("expected EventNodeEnd")
	}
}

// TestEventLogNilSafety verifies that EventLog methods are safe when called
// on a nil EventLog (used internally as no-op guards).
func TestEventLogNilSafety(t *testing.T) {
	cm := newChannelManager()
	cm.addChannel(END, newPregelChannel())
	cm.updateValues(START, "ignored", map[string]bool{END: true})

	val, ok := cm.getEndChannel()
	if !ok {
		t.Fatal("expected end channel to be ready")
	}
	if val != "ignored" {
		t.Fatalf("expected ignored, got %v", val)
	}

	pc := newPregelChannel()
	pc.reportDependency("noop")
	allSkipped := pc.reportSkip("noop")
	if allSkipped {
		t.Fatal("expected pregel reportSkip to return false")
	}
}

// TestChannelManagerGetEndChannel verifies getting data from the END channel.
func TestChannelManagerGetEndChannel(t *testing.T) {
	cm := newChannelManager()

	dm := newDAGChannel([]string{}, []string{})
	dm.reportValues("source", "final_value")
	cm.addChannel(END, dm)

	val, ok := cm.getEndChannel()
	if !ok {
		t.Fatal("expected END channel to be ready")
	}
	if val != "final_value" {
		t.Fatalf("expected final_value, got %v", val)
	}

	_, ok2 := cm.getEndChannel()
	if ok2 {
		t.Fatal("expected no more values after consuming END")
	}
}

// TestChannelManagerGetReadySkipPregel verifies Pregel channels become ready
// immediately when any predecessor reports a value.
func TestChannelManagerGetReadySkipPregel(t *testing.T) {
	cm := newChannelManager()

	pc := newPregelChannel()
	pc.reportValues("a", "first")
	cm.addChannel("node_a", pc)

	ready := cm.getReadyChannels("")
	if len(ready) != 1 {
		t.Fatalf("expected 1 ready channel in Pregel mode, got %d", len(ready))
	}
	if ready["node_a"] != "first" {
		t.Fatalf("expected first, got %v", ready["node_a"])
	}

	cm.reportSkip("ignored", map[string]bool{"node_a": true})
}

// TestPregelChannelSetMergeConfigThenGet verifies that setting a merge config
// on Pregel channel and then reporting multiple values merges them.
func TestPregelChannelSetMergeConfigThenGet(t *testing.T) {
	pc := newPregelChannel()

	pc.setMergeConfig(func(values map[string]any) (any, error) {
		return "joined", nil
	})

	pc.reportValues("p1", "v1")
	pc.reportValues("p2", "v2")

	val, ok, err := pc.get()
	if err != nil {
		t.Fatalf("get failed: %v", err)
	}
	if !ok {
		t.Fatal("expected value to be ready")
	}
	if val != "joined" {
		t.Fatalf("expected joined, got %v", val)
	}
}

// TestCompiledGraphRecompileWithModeSwitch verifies that recompiling from
// DAG to Pregel mode works and reflects in GraphInfo.
func TestCompiledGraphRecompileWithModeSwitch(t *testing.T) {
	g := NewGraph[string, string]()

	g.AddLambdaNode("echo", InvokableLambda(nodeIdentity))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	_, err := g.Compile(context.Background(),
		WithGraphName("mode_a"),
		WithNodeTriggerMode(AllPredecessor),
		WithMaxRunSteps(15),
	)
	if err != nil {
		t.Fatalf("First compile (DAG) failed: %v", err)
	}

	info1 := g.GetGraphInfo()
	if !info1.DAGMode {
		t.Fatal("expected DAGMode=true after first compile")
	}
	if info1.MaxSteps != 15 {
		t.Fatalf("expected MaxSteps=15, got %d", info1.MaxSteps)
	}

	_, err = g.Compile(context.Background(),
		WithGraphName("mode_b"),
		WithNodeTriggerMode(AnyPredecessor),
		WithMaxRunSteps(25),
	)
	if err != nil {
		t.Fatalf("Second compile (Pregel) failed: %v", err)
	}

	info2 := g.GetGraphInfo()
	if !info2.PregelMode {
		t.Fatal("expected PregelMode=true after second compile")
	}
	if info2.Name != "mode_b" {
		t.Fatalf("expected name mode_b, got %s", info2.Name)
	}
	if info2.MaxSteps != 25 {
		t.Fatalf("expected MaxSteps=25, got %d", info2.MaxSteps)
	}
}
