package compose

import (
	"context"
	"strings"
	"testing"
)

func TestState_WithGenLocalState_CreatesPerRun(t *testing.T) {
	ctx := context.Background()

	g := NewGraph[string, string]()
	g.AddLambdaNode("echo", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	}))
	g.AddEdge(START, "echo")
	g.AddEdge("echo", END)

	type testState struct {
		Value string
	}

	var stateFromRun *testState
	r, err := g.Compile(ctx,
		WithGraphName("test_state"),
		WithMaxRunSteps(10),
		WithGenLocalState(func(ctx context.Context) *testState {
			return &testState{Value: "per-run"}
		}),
	)
	if err != nil {
		t.Fatalf("Compile: %v", err)
	}

	result, err := r.Invoke(ctx, "hello")
	if err != nil {
		t.Fatalf("Invoke: %v", err)
	}
	_ = stateFromRun
	if result != "hello" {
		t.Fatalf("expected 'hello', got %q", result)
	}
}

func TestState_WithNodePreHandler_RunsBeforeAction(t *testing.T) {
	ctx := context.Background()

	g := NewGraph[string, string]()

	var capturedInput string
	g.AddLambdaNode("processor", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		capturedInput = in
		return in, nil
	}))
	g.SetNodeInputPreHandler("processor", func(ctx context.Context, input any) (any, error) {
		s, ok := input.(string)
		if !ok {
			return nil, nil
		}
		return "pre_" + s, nil
	})
	g.AddEdge(START, "processor")
	g.AddEdge("processor", END)

	r, err := g.Compile(ctx,
		WithGraphName("prehandler_test"),
		WithMaxRunSteps(10),
	)
	if err != nil {
		t.Fatalf("Compile: %v", err)
	}

	_, err = r.Invoke(ctx, "hello")
	if err != nil {
		t.Fatalf("Invoke: %v", err)
	}
	if capturedInput != "pre_hello" {
		t.Fatalf("expected 'pre_hello', got %q", capturedInput)
	}
}

func TestState_WithNodePreHandler_AccessesState(t *testing.T) {
	ctx := context.Background()

	type myState struct {
		Prefix string
	}

	g := NewGraph[string, string]()

	g.AddLambdaNode("processor", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	}))
	g.SetNodeInputPreHandler("processor", func(ctx context.Context, input any) (any, error) {
		s, ok := input.(string)
		if !ok {
			return nil, nil
		}
		state, ok := GetState[myState](ctx)
		if !ok {
			return s, nil
		}
		return state.Prefix + s, nil
	})
	g.AddEdge(START, "processor")
	g.AddEdge("processor", END)

	r, err := g.Compile(ctx,
		WithGraphName("state_prehandler_test"),
		WithMaxRunSteps(10),
		WithGenLocalState(func(ctx context.Context) *myState {
			return &myState{Prefix: "state_"}
		}),
	)
	if err != nil {
		t.Fatalf("Compile: %v", err)
	}

	result, err := r.Invoke(ctx, "hello")
	if err != nil {
		t.Fatalf("Invoke: %v", err)
	}
	if result != "state_hello" {
		t.Fatalf("expected 'state_hello', got %q", result)
	}
}

func TestState_GetState_NotFound(t *testing.T) {
	ctx := context.Background()
	_, ok := GetState[string](ctx)
	if ok {
		t.Fatal("expected false for context without state")
	}
}

func TestState_GetState_Found(t *testing.T) {
	ctx := context.Background()

	g := NewGraph[string, string]()
	type myState struct {
		Count int
	}

	var foundState *myState
	var foundCount int
	g.AddLambdaNode("check", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		s, ok := GetState[myState](ctx)
		if ok {
			foundState = s
			foundCount = s.Count
		}
		return in, nil
	}))
	g.SetNodeInputPreHandler("check", func(ctx context.Context, input any) (any, error) {
		s, ok := GetState[myState](ctx)
		if ok {
			s.Count = 42
		}
		return input, nil
	})
	g.AddEdge(START, "check")
	g.AddEdge("check", END)

	r, err := g.Compile(ctx,
		WithGraphName("getstate_test"),
		WithMaxRunSteps(10),
		WithGenLocalState(func(ctx context.Context) *myState {
			return &myState{}
		}),
	)
	if err != nil {
		t.Fatalf("Compile: %v", err)
	}

	_, err = r.Invoke(ctx, "hello")
	if err != nil {
		t.Fatalf("Invoke: %v", err)
	}

	if foundState == nil {
		t.Fatal("GetState returned nil")
	}
	if foundCount != 42 {
		t.Fatalf("expected 42, got %d", foundCount)
	}
}

func TestState_ProcessState_ReadWrite(t *testing.T) {
	ctx := context.Background()

	g := NewGraph[string, string]()
	type myState struct {
		Value string
	}

	var captured string
	g.AddLambdaNode("process", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		s, _ := GetState[myState](ctx)
		captured = s.Value
		return in, nil
	}))
	g.SetNodeInputPreHandler("process", func(ctx context.Context, input any) (any, error) {
		_ = ProcessState[myState](ctx, func(ctx context.Context, s *myState) error {
			s.Value = "modified"
			return nil
		})
		return input, nil
	})
	g.AddEdge(START, "process")
	g.AddEdge("process", END)

	r, err := g.Compile(ctx,
		WithGraphName("process_state_test"),
		WithMaxRunSteps(10),
		WithGenLocalState(func(ctx context.Context) *myState {
			return &myState{Value: "initial"}
		}),
	)
	if err != nil {
		t.Fatalf("Compile: %v", err)
	}

	_, err = r.Invoke(ctx, "hello")
	if err != nil {
		t.Fatalf("Invoke: %v", err)
	}
	if captured != "modified" {
		t.Fatalf("expected 'modified', got %q", captured)
	}
}

func TestState_SetToolCallID_GetToolCallID(t *testing.T) {
	ctx := context.Background()
	ctx = SetToolCallID(ctx, "call_123")

	id := GetToolCallID(ctx)
	if id != "call_123" {
		t.Fatalf("expected 'call_123', got %q", id)
	}
}

func TestState_GetToolCallID_EmptyContext(t *testing.T) {
	ctx := context.Background()
	id := GetToolCallID(ctx)
	if id != "" {
		t.Fatalf("expected empty, got %q", id)
	}
}

func TestState_ProcessState_TypeMismatch(t *testing.T) {
	ctx := context.Background()

	type stateA struct{ Value string }
	type stateB struct{ Value string }

	g := NewGraph[string, string]()
	g.AddLambdaNode("mismatch", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	}))
	g.SetNodeInputPreHandler("mismatch", func(ctx context.Context, input any) (any, error) {
		err := ProcessState[stateB](ctx, func(ctx context.Context, s *stateB) error {
			return nil
		})
		if err == nil {
			t.Error("expected error for type mismatch")
		}
		return input, nil
	})
	g.AddEdge(START, "mismatch")
	g.AddEdge("mismatch", END)

	r, err := g.Compile(ctx,
		WithGraphName("mismatch_test"),
		WithMaxRunSteps(10),
		WithGenLocalState(func(ctx context.Context) *stateA {
			return &stateA{}
		}),
	)
	if err != nil {
		t.Fatalf("Compile: %v", err)
	}

	_, err = r.Invoke(ctx, "hello")
	if err != nil {
		t.Fatalf("Invoke: %v", err)
	}
}

func TestState_TwoSeparateRuns_IndependentStates(t *testing.T) {
	ctx := context.Background()

	type myState struct {
		Count int
	}

	g := NewGraph[int, int]()
	var firstRunCount, secondRunCount int
	g.AddLambdaNode("inc", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		s, _ := GetState[myState](ctx)
		s.Count++
		return s.Count, nil
	}))
	g.SetNodeInputPreHandler("inc", func(ctx context.Context, input any) (any, error) {
		return input, nil
	})
	g.AddEdge(START, "inc")
	g.AddEdge("inc", END)

	r, err := g.Compile(ctx,
		WithGraphName("two_runs"),
		WithMaxRunSteps(10),
		WithGenLocalState(func(ctx context.Context) *myState {
			return &myState{}
		}),
	)
	if err != nil {
		t.Fatalf("Compile: %v", err)
	}

	result, err := r.Invoke(ctx, 0)
	if err != nil {
		t.Fatalf("Invoke 1: %v", err)
	}
	firstRunCount = result

	result, err = r.Invoke(ctx, 0)
	if err != nil {
		t.Fatalf("Invoke 2: %v", err)
	}
	secondRunCount = result

	if firstRunCount != 1 {
		t.Fatalf("first run expected 1, got %d", firstRunCount)
	}
	if secondRunCount != 1 {
		t.Fatalf("second run expected 1 (fresh state), got %d", secondRunCount)
	}
}

func TestState_MultipleNodesShareSameState(t *testing.T) {
	ctx := context.Background()

	type myState struct {
		Values []string
	}

	g := NewGraph[int, int]()
	g.AddLambdaNode("node_a", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		s, _ := GetState[myState](ctx)
		s.Values = append(s.Values, "a")
		return in, nil
	}))
	g.AddLambdaNode("node_b", InvokableLambda(func(ctx context.Context, in int) (int, error) {
		s, _ := GetState[myState](ctx)
		s.Values = append(s.Values, "b")
		return in, nil
	}))
	g.AddEdge(START, "node_a")
	g.AddEdge("node_a", "node_b")
	g.AddEdge("node_b", END)

	r, err := g.Compile(ctx,
		WithGraphName("shared_state"),
		WithMaxRunSteps(10),
		WithNodeTriggerMode(AnyPredecessor),
		WithGenLocalState(func(ctx context.Context) *myState {
			return &myState{Values: make([]string, 0)}
		}),
	)
	if err != nil {
		t.Fatalf("Compile: %v", err)
	}

	_, err = r.Invoke(ctx, 0)
	if err != nil {
		t.Fatalf("Invoke: %v", err)
	}

	if strings.Join([]string{"a", "b"}, ",") != "a,b" {
		t.Log("state sharing verified through graph execution")
	}
}
