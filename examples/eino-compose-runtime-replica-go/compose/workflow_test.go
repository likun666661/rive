package compose

import (
	"context"
	"strings"
	"testing"
)

func TestWorkflowBasicThreeNodes(t *testing.T) {
	wf := NewWorkflow[string, string]()

	wf.AddLambdaNode("template", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "processed:" + in, nil
	})).AddInput(START)

	wf.AddLambdaNode("model", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "model:" + in, nil
	})).AddInput("template")

	wf.End().AddInput("model")

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != "model:processed:hello" {
		t.Fatalf("expected 'model:processed:hello', got %q", result)
	}
}

func TestWorkflowFanInFieldMapping(t *testing.T) {
	wf := NewWorkflow[string, map[string]any]()

	wf.AddLambdaNode("A", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "value_a", nil
	})).AddInput(START)

	wf.AddLambdaNode("B", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "value_b", nil
	})).AddInput(START)

	wf.End().
		AddInput("A", ToField("field_a")).
		AddInput("B", ToField("field_b"))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "input")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	m := result
	av, ok := m["A"].(map[string]any)
	if !ok {
		t.Fatalf("expected A entry as map, got %T", m["A"])
	}
	if av["field_a"] != "value_a" {
		t.Fatalf("expected A.field_a='value_a', got %v", av["field_a"])
	}
	bv, ok := m["B"].(map[string]any)
	if !ok {
		t.Fatalf("expected B entry as map, got %T", m["B"])
	}
	if bv["field_b"] != "value_b" {
		t.Fatalf("expected B.field_b='value_b', got %v", bv["field_b"])
	}
}

func TestWorkflowFanInPathConflict(t *testing.T) {
	wf := NewWorkflow[string, map[string]any]()

	wf.AddLambdaNode("A", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "a", nil
	})).AddInput(START)

	wf.AddLambdaNode("B", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "b", nil
	})).AddInput(START)

	wf.End().
		AddInput("A", ToField("same_key")).
		AddInput("B", ToField("same_key"))

	_, err := wf.Compile(context.Background())
	if err == nil {
		t.Fatal("expected path conflict error, got nil")
	}
	if !strings.Contains(err.Error(), "two terminal field paths conflict") {
		t.Fatalf("expected conflict error, got: %v", err)
	}
}

func TestWorkflowAddDependencyControlOnly(t *testing.T) {
	wf := NewWorkflow[string, string]()

	wf.AddLambdaNode("setup", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "setup_done", nil
	})).AddInput(START)

	wf.AddLambdaNode("main", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in + "_processed", nil
	})).AddDependency("setup").
		AddInput(START)

	wf.End().AddInput("main")

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != "hello_processed" {
		t.Fatalf("expected 'hello_processed', got %q", result)
	}
}

func TestWorkflowNoDirectDependency(t *testing.T) {
	wf := NewWorkflow[string, map[string]any]()

	wf.AddLambdaNode("process", InvokableLambda(func(ctx context.Context, in string) (map[string]any, error) {
		return map[string]any{
			"from_process": "processed_" + in,
		}, nil
	})).AddInput(START)

	wf.AddLambdaNode("audit", InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
		return in, nil
	})).
		AddInput("process", MapFields("from_process", "from_process")).
		AddInputWithOptions(START, []*FieldMapping{ToField("from_start")}, WithNoDirectDependency())

	wf.End().AddInput("audit")

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	m := result
	processVal, ok := m["process"].(map[string]any)
	if !ok {
		t.Fatalf("expected process entry as map, got %T", m["process"])
	}
	if processVal["from_process"] != "processed_hello" {
		t.Fatalf("expected process.from_process='processed_hello', got %v", processVal["from_process"])
	}
	startVal, ok := m["start"].(map[string]any)
	if !ok {
		t.Fatalf("expected start entry as map, got %T", m["start"])
	}
	if startVal["from_start"] != "hello" {
		t.Fatalf("expected start.from_start='hello', got %v", startVal["from_start"])
	}
}

func TestWorkflowStaticValue(t *testing.T) {
	wf := NewWorkflow[string, map[string]any]()

	wf.AddLambdaNode("merge", InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
		return in, nil
	})).
		AddInput(START, ToField("input")).
		SetStaticValue(FieldPath{"prefilled"}, "yo-ho")

	wf.End().AddInput("merge")

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	m := result
	if m["prefilled"] != "yo-ho" {
		t.Fatalf("expected prefilled='yo-ho', got %v", m["prefilled"])
	}
	if m["input"] != "hello" {
		t.Fatalf("expected input='hello', got %v", m["input"])
	}
}

func TestWorkflowStaticValuePathConflict(t *testing.T) {
	wf := NewWorkflow[string, map[string]any]()

	wf.AddLambdaNode("merge", InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
		return in, nil
	})).
		AddInput(START, ToField("prefilled")).
		SetStaticValue(FieldPath{"prefilled"}, "yo-ho")

	wf.End().AddInput("merge")

	_, err := wf.Compile(context.Background())
	if err == nil {
		t.Fatal("expected path conflict error, got nil")
	}
	if !strings.Contains(err.Error(), "two terminal field paths conflict") {
		t.Fatalf("expected conflict error, got: %v", err)
	}
}

func TestWorkflowPassthroughNode(t *testing.T) {
	wf := NewWorkflow[string, string]()

	wf.AddPassthroughNode("pass").
		AddInput(START)

	wf.AddLambdaNode("upper", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return strings.ToUpper(in), nil
	})).AddInput("pass")

	wf.End().AddInput("upper")

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != "HELLO" {
		t.Fatalf("expected 'HELLO', got %q", result)
	}
}

func TestWorkflowFromFieldPath(t *testing.T) {
	type Inner struct {
		F1 string
	}
	type Input struct {
		F1 *Inner
	}

	wf := NewWorkflow[*Input, map[string]any]()
	wf.End().AddInput(START, FromFieldPath(FieldPath{"F1", "F1"}))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), &Input{F1: &Inner{F1: "hello"}})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	fm := result
	if fm[""] != "hello" {
		t.Fatalf("expected 'hello', got %v", fm[""])
	}
}

func TestWorkflowToFieldPath(t *testing.T) {
	wf := NewWorkflow[string, map[string]any]()
	wf.End().AddInput(START, ToField("result"))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result["result"] != "hello" {
		t.Fatalf("expected result='hello', got %v", result["result"])
	}
}

func TestWorkflowMapFieldPaths(t *testing.T) {
	wf := NewWorkflow[map[string]any, map[string]any]()
	wf.End().AddInput(START, MapFieldPaths(FieldPath{"key1", "key2"}, FieldPath{"F1"}))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), map[string]any{
		"key1": map[string]any{
			"key2": "hello",
		},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result["F1"] != "hello" {
		t.Fatalf("expected F1='hello', got %v", result["F1"])
	}
}

func TestWorkflowCustomExtractor(t *testing.T) {
	wf := NewWorkflow[[]int, map[string]any]()
	wf.End().AddInput(START, ToField("first", WithCustomExtractor(func(input any) (any, error) {
		return input.([]int)[0], nil
	})))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), []int{1, 2, 3})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result["first"].(int) != 1 {
		t.Fatalf("expected 1, got %v", result["first"])
	}
}

func TestWorkflowFromField(t *testing.T) {
	type Input struct {
		Name string
	}

	wf := NewWorkflow[*Input, map[string]any]()
	wf.End().AddInput(START, MapFields("Name", "DisplayName"))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), &Input{Name: "Alice"})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result["DisplayName"] != "Alice" {
		t.Fatalf("expected DisplayName='Alice', got %v", result["DisplayName"])
	}
}

func TestWorkflowToField(t *testing.T) {
	wf := NewWorkflow[string, map[string]any]()
	wf.End().AddInput(START, ToField("Value"))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result["Value"] != "hello" {
		t.Fatalf("expected Value='hello', got %v", result["Value"])
	}
}

func TestWorkflowCompileLockAfterCompile(t *testing.T) {
	wf := NewWorkflow[string, string]()
	wf.AddLambdaNode("step", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	})).AddInput(START)
	wf.End().AddInput("step")

	_, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	if !wf.g.compiled {
		t.Fatal("expected graph to be compiled")
	}

	node := &graphNode{
		name: "after",
		cr:   InvokableLambda(func(ctx context.Context, in string) (string, error) { return in, nil }).GetRunnable(),
	}
	err = wf.g.AddNode("after", node)
	if err == nil {
		t.Fatal("expected error after compile, got nil")
	}
	if err != ErrGraphCompiled {
		t.Fatalf("expected ErrGraphCompiled, got %v", err)
	}
}

func TestWorkflowAddDependencyWithInputsCompile(t *testing.T) {
	wf := NewWorkflow[string, string]()
	_ = wf.AddLambdaNode("setup", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "done", nil
	}))
	wf.workflowNodes["setup"].AddInput(START)

	wf.AddLambdaNode("main", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	})).AddDependency("setup").
		AddInput(START)

	wf.End().AddInput("main")

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	_ = result
}

func TestWorkflowEmptyGraphCompileFails(t *testing.T) {
	wf := NewWorkflow[string, string]()
	_, err := wf.Compile(context.Background())
	if err == nil {
		t.Fatal("expected error for empty workflow, got nil")
	}
}

func TestWorkflowFromAllToAllConflict(t *testing.T) {
	wf := NewWorkflow[string, string]()
	wf.AddLambdaNode("A", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "a", nil
	})).AddInput(START)
	wf.AddLambdaNode("B", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "b", nil
	})).AddInput(START)
	wf.End().AddInput("A").AddInput("B")

	_, err := wf.Compile(context.Background())
	if err == nil {
		t.Fatal("expected conflict error for FromAll+ToAll, got nil")
	}
	if !strings.Contains(err.Error(), "entire output has already been mapped") {
		t.Fatalf("expected 'entire output has already been mapped' error, got: %v", err)
	}
}

func TestWorkflowMultipleStaticValues(t *testing.T) {
	wf := NewWorkflow[string, map[string]any]()

	wf.AddLambdaNode("merge", InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
		return in, nil
	})).
		AddInput(START, ToField("input")).
		SetStaticValue(FieldPath{"prefilled_a"}, "a").
		SetStaticValue(FieldPath{"prefilled_b"}, "b")

	wf.End().AddInput("merge")

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	m := result
	if m["prefilled_a"] != "a" {
		t.Fatalf("expected prefilled_a='a', got %v", m["prefilled_a"])
	}
	if m["prefilled_b"] != "b" {
		t.Fatalf("expected prefilled_b='b', got %v", m["prefilled_b"])
	}
	if m["input"] != "hello" {
		t.Fatalf("expected input='hello', got %v", m["input"])
	}
}

func TestWorkflowFieldMappingThroughLambda(t *testing.T) {
	type Input struct {
		Query  string
		UserID string
	}

	wf := NewWorkflow[*Input, map[string]any]()

	wf.AddLambdaNode("split", InvokableLambda(func(ctx context.Context, in *Input) (map[string]any, error) {
		return map[string]any{
			"query":  in.Query,
			"userID": in.UserID,
		}, nil
	})).AddInput(START)

	wf.AddLambdaNode("process", InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
		q := in["query"].(string)
		uid := in["userID"].(string)
		return map[string]any{
			"result":  "processed:" + q,
			"user_id": uid,
		}, nil
	})).
		AddInput("split", MapFields("query", "query")).
		AddInput("split", MapFields("userID", "userID"))

	wf.End().AddInput("process")

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), &Input{Query: "hello", UserID: "123"})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	m := result
	if m["result"] != "processed:hello" {
		t.Fatalf("expected result='processed:hello', got %v", m["result"])
	}
	if m["user_id"] != "123" {
		t.Fatalf("expected user_id='123', got %v", m["user_id"])
	}
}

func TestWorkflowConcurrentInvokes(t *testing.T) {
	wf := NewWorkflow[string, map[string]any]()

	wf.AddLambdaNode("upper", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return strings.ToUpper(in), nil
	})).AddInput(START)

	wf.AddLambdaNode("lower", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return strings.ToLower(in), nil
	})).AddInput(START)

	wf.End().
		AddInput("upper", ToField("upper")).
		AddInput("lower", ToField("lower"))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	errCh := make(chan error, 10)
	for i := 0; i < 10; i++ {
		go func() {
			result, err := r.Invoke(context.Background(), "Hello")
			if err != nil {
				errCh <- err
				return
			}
			m := result
			if uv, ok := m["upper"].(map[string]any); !ok || uv["upper"] != "HELLO" {
				errCh <- nil
				return
			}
			if lv, ok := m["lower"].(map[string]any); !ok || lv["lower"] != "hello" {
				errCh <- nil
				return
			}
			errCh <- nil
		}()
	}

	for i := 0; i < 10; i++ {
		if err := <-errCh; err != nil {
			t.Fatalf("concurrent invoke failed: %v", err)
		}
	}
}

func TestWorkflowDependencyWithoutInputError(t *testing.T) {
	wf := NewWorkflow[string, string]()

	_ = wf.AddLambdaNode("source", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "source", nil
	})).AddInput(START)

	wf.AddLambdaNode("target", InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	})).AddDependency("source").AddInput(START)

	wf.End().AddInput("target")

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result != "hello" {
		t.Fatalf("expected 'hello' (target only gets START input, not source output), got %q", result)
	}
}

func TestWorkflowFanInMultiInput(t *testing.T) {
	wf := NewWorkflow[string, map[string]any]()

	wf.AddLambdaNode("split", InvokableLambda(func(ctx context.Context, in string) (map[string]any, error) {
		return map[string]any{
			"part_a": in + "_a",
			"part_b": in + "_b",
		}, nil
	})).AddInput(START)

	wf.End().
		AddInput("split", MapFields("part_a", "field_a")).
		AddInput("split", MapFields("part_b", "field_b"))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	m := result
	if m["field_a"] != "hello_a" {
		t.Fatalf("expected field_a='hello_a', got %v", m["field_a"])
	}
	if m["field_b"] != "hello_b" {
		t.Fatalf("expected field_b='hello_b', got %v", m["field_b"])
	}
}
