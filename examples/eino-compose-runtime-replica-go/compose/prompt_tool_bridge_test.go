package compose

import (
	"context"
	"errors"
	"strings"
	"testing"
)

// =============================================================================
// Stub tools for deterministic testing
// =============================================================================

type stubWeatherTool struct{}

func (t *stubWeatherTool) Name() string { return "get_weather" }

func (t *stubWeatherTool) Execute(ctx context.Context, args map[string]any) (string, error) {
	loc, _ := args["location"].(string)
	return "Sunny, 22°C in " + loc, nil
}

type stubCalcTool struct{}

func (t *stubCalcTool) Name() string { return "calculator" }

func (t *stubCalcTool) Execute(ctx context.Context, args map[string]any) (string, error) {
	expr, _ := args["expression"].(string)
	if expr == "" {
		return "", errors.New("calculator: expression required")
	}
	return "Result: 42", nil
}

type stubErrorTool struct {
	err error
}

func (t *stubErrorTool) Name() string { return "failing_tool" }

func (t *stubErrorTool) Execute(ctx context.Context, args map[string]any) (string, error) {
	return "", t.err
}

// =============================================================================
// BridgeTool tests
// =============================================================================

func TestBridgeToolFunc(t *testing.T) {
	tool := NewBridgeTool("echo", func(ctx context.Context, args map[string]any) (string, error) {
		return args["msg"].(string), nil
	})

	if tool.Name() != "echo" {
		t.Fatalf("expected name 'echo', got %q", tool.Name())
	}

	result, err := tool.Execute(context.Background(), map[string]any{"msg": "hello"})
	if err != nil {
		t.Fatal(err)
	}
	if result != "hello" {
		t.Fatalf("expected 'hello', got %q", result)
	}
}

// =============================================================================
// PromptTemplate bridge tests
// =============================================================================

func TestPromptTemplateBridgeToLambda(t *testing.T) {
	tmpl := NewMessageTemplate("{{query}}")
	bridge := &promptTemplateBridge{tmpl: tmpl}
	lambda := bridge.toLambda()

	g := NewGraph[map[string]any, []*Message]()
	g.AddLambdaNode("prompt", lambda)
	g.AddEdge(START, "prompt")
	g.AddEdge("prompt", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("prompt_template_bridge_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), map[string]any{"query": "hello world"})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if len(result) != 1 {
		t.Fatalf("expected 1 message, got %d", len(result))
	}
	if result[0].Role != Human {
		t.Fatalf("expected Human role, got %q", result[0].Role)
	}
	if result[0].Content != "hello world" {
		t.Fatalf("expected 'hello world', got %q", result[0].Content)
	}
}

func TestPromptTemplateBridgeWithSystem(t *testing.T) {
	tmpl := NewMessageTemplate("{{query}}").
		WithSystemTemplate("You are {{role}}.")
	bridge := &promptTemplateBridge{tmpl: tmpl}
	lambda := bridge.toLambda()

	g := NewGraph[map[string]any, []*Message]()
	g.AddLambdaNode("prompt", lambda)
	g.AddEdge(START, "prompt")
	g.AddEdge("prompt", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("prompt_template_system_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), map[string]any{
		"role":  "assistant",
		"query": "What is Go?",
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if len(result) != 2 {
		t.Fatalf("expected 2 messages, got %d", len(result))
	}
	if result[0].Role != System || result[0].Content != "You are assistant." {
		t.Fatalf("system message mismatch: %+v", result[0])
	}
	if result[1].Role != Human || result[1].Content != "What is Go?" {
		t.Fatalf("user message mismatch: %+v", result[1])
	}
}

// =============================================================================
// ToolsNode bridge tests
// =============================================================================

func TestToolsNodeBridgeNoToolCalls(t *testing.T) {
	tools := &toolsNodeBridge{tools: map[string]BridgeTool{}}
	lambda := tools.toLambda()

	msg := &Message{Role: Assistant, Content: "hello"}
	g := NewGraph[*Message, *Message]()
	g.AddLambdaNode("tools", lambda)
	g.AddEdge(START, "tools")
	g.AddEdge("tools", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("tools_noop_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), msg)
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if result.Content != "hello" {
		t.Fatalf("expected content preserved, got %q", result.Content)
	}
}

func TestToolsNodeBridgeExecutesTool(t *testing.T) {
	tools := &toolsNodeBridge{
		tools: map[string]BridgeTool{
			"get_weather": &stubWeatherTool{},
		},
	}
	lambda := tools.toLambda()

	msg := &Message{
		Role:    Assistant,
		Content: "",
		ToolCalls: []ToolCall{
			{
				ID:   "call_1",
				Type: "function",
				Function: ToolCallFunction{
					Name:      "get_weather",
					Arguments: `{"location":"Paris"}`,
				},
			},
		},
	}

	g := NewGraph[*Message, *Message]()
	g.AddLambdaNode("tools", lambda)
	g.AddEdge(START, "tools")
	g.AddEdge("tools", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("tools_exec_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), msg)
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if !strings.Contains(result.Content, "Sunny, 22°C in Paris") {
		t.Fatalf("expected tool result in content, got: %q", result.Content)
	}
}

func TestToolsNodeBridgeMultipleToolCalls(t *testing.T) {
	tools := &toolsNodeBridge{
		tools: map[string]BridgeTool{
			"get_weather": &stubWeatherTool{},
			"calculator":  &stubCalcTool{},
		},
	}
	lambda := tools.toLambda()

	msg := &Message{
		Role:    Assistant,
		Content: "",
		ToolCalls: []ToolCall{
			{
				ID:   "call_1",
				Type: "function",
				Function: ToolCallFunction{
					Name:      "get_weather",
					Arguments: `{"location":"Tokyo"}`,
				},
			},
			{
				ID:   "call_2",
				Type: "function",
				Function: ToolCallFunction{
					Name:      "calculator",
					Arguments: `{"expression":"2+2"}`,
				},
			},
		},
	}

	_, err := lambda.invokeFn(context.Background(), msg)
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
}

func TestToolsNodeBridgeToolNotFound(t *testing.T) {
	tools := &toolsNodeBridge{
		tools: map[string]BridgeTool{
			"get_weather": &stubWeatherTool{},
		},
	}
	lambda := tools.toLambda()

	msg := &Message{
		Role:    Assistant,
		Content: "",
		ToolCalls: []ToolCall{
			{
				ID:   "call_1",
				Type: "function",
				Function: ToolCallFunction{
					Name:      "nonexistent_tool",
					Arguments: `{}`,
				},
			},
		},
	}

	_, err := lambda.invokeFn(context.Background(), msg)
	if err == nil {
		t.Fatal("expected error for unknown tool")
	}
	if !strings.Contains(err.Error(), "tool not found") {
		t.Fatalf("expected 'tool not found' error, got: %v", err)
	}
}

func TestToolsNodeBridgeToolError(t *testing.T) {
	testErr := errors.New("tool execution failed")
	tools := &toolsNodeBridge{
		tools: map[string]BridgeTool{
			"failing_tool": &stubErrorTool{err: testErr},
		},
	}
	lambda := tools.toLambda()

	msg := &Message{
		Role:    Assistant,
		Content: "",
		ToolCalls: []ToolCall{
			{
				ID:   "call_1",
				Type: "function",
				Function: ToolCallFunction{
					Name:      "failing_tool",
					Arguments: `{}`,
				},
			},
		},
	}

	_, err := lambda.invokeFn(context.Background(), msg)
	if err == nil {
		t.Fatal("expected error from tool")
	}
}

// =============================================================================
// Full pipeline tests (Graph / Workflow / Chain)
// =============================================================================

func buildToolCallModel() *FakeChatModel {
	return NewFakeChatModel(WithChatGenerateFunc(
		func(ctx context.Context, input []*Message) (*Message, error) {
			return &Message{
				Role:    Assistant,
				Content: "",
				ToolCalls: []ToolCall{
					{
						ID:   "call_weather",
						Type: "function",
						Function: ToolCallFunction{
							Name:      "get_weather",
							Arguments: `{"location":"Paris"}`,
						},
					},
				},
			}, nil
		},
	))
}

func buildFinalModel() *FakeChatModel {
	return NewFakeChatModel(WithChatGenerateFunc(
		func(ctx context.Context, input []*Message) (*Message, error) {
			if len(input) == 0 {
				return &Message{Role: Assistant, Content: "no input"}, nil
			}
			last := input[len(input)-1]
			return &Message{
				Role:    Assistant,
				Content: "Final answer based on: " + last.Content,
			}, nil
		},
	))
}

func TestToolCallingPipelineGraph(t *testing.T) {
	toolCallModel := buildToolCallModel()
	finalModel := buildFinalModel()

	toolsNode := (&toolsNodeBridge{
		tools: map[string]BridgeTool{
			"get_weather": &stubWeatherTool{},
		},
	}).toLambda()

	g := NewGraph[[]*Message, *Message]()

	chatModelLambda := InvokableLambda(func(ctx context.Context, msgs []*Message) (*Message, error) {
		return toolCallModel.Generate(ctx, msgs)
	})
	g.AddLambdaNode("model1", chatModelLambda)
	g.AddLambdaNode("tools", toolsNode)

	finalLambda := InvokableLambda(func(ctx context.Context, msg *Message) (*Message, error) {
		return finalModel.Generate(ctx, []*Message{msg})
	})
	g.AddLambdaNode("model2", finalLambda)

	g.AddEdge(START, "model1")
	g.AddEdge("model1", "tools")
	g.AddEdge("tools", "model2")
	g.AddEdge("model2", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("tool_calling_pipeline"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), []*Message{
		{Role: Human, Content: "What is the weather in Paris?"},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if !strings.Contains(result.Content, "Final answer") {
		t.Fatalf("expected final answer, got: %q", result.Content)
	}
}

func TestToolCallingPipelineWorkflow(t *testing.T) {
	tmpl := NewMessageTemplate("{{query}}").
		WithSystemTemplate("You are a helpful assistant with tools.")

	toolCallModel := buildToolCallModel()
	finalModel := buildFinalModel()

	wf := NewWorkflow[map[string]any, *Message]()

	wf.AsPromptTemplateNode("prompt", tmpl).
		AddInput(START)

	wf.AddLambdaNode("model1", InvokableLambda(func(ctx context.Context, msgs []*Message) (*Message, error) {
		return toolCallModel.Generate(ctx, msgs)
	})).AddInput("prompt")

	wf.AsToolsNode("tools", &stubWeatherTool{}).
		AddInput("model1")

	wf.AddLambdaNode("model2", InvokableLambda(func(ctx context.Context, msg *Message) (*Message, error) {
		return finalModel.Generate(ctx, []*Message{msg})
	})).AddInput("tools")

	wf.End().AddInput("model2")

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), map[string]any{
		"query": "What is the weather in Paris?",
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if !strings.Contains(result.Content, "Final answer") {
		t.Fatalf("expected final answer in result, got: %q", result.Content)
	}
	if !strings.Contains(result.Content, "Sunny, 22°C in Paris") {
		t.Fatalf("expected weather tool result in final content, got: %q", result.Content)
	}
}

func TestToolCallingPipelineChain(t *testing.T) {
	chatModelLambda := InvokableLambda(func(ctx context.Context, msgs []*Message) (*Message, error) {
		return buildToolCallModel().Generate(ctx, msgs)
	})

	toolsLambda := (&toolsNodeBridge{
		tools: map[string]BridgeTool{
			"get_weather": &stubWeatherTool{},
		},
	}).toLambda()

	finalLambda := InvokableLambda(func(ctx context.Context, msg *Message) (*Message, error) {
		return buildFinalModel().Generate(ctx, []*Message{msg})
	})

	c := NewChain[[]*Message, *Message]()
	c.AppendLambda(chatModelLambda)
	c.AppendLambda(toolsLambda)
	c.AppendLambda(finalLambda)

	r, err := c.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), []*Message{
		{Role: Human, Content: "What is the weather in Paris?"},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if !strings.Contains(result.Content, "Final answer") {
		t.Fatalf("expected final answer, got: %q", result.Content)
	}
}

// =============================================================================
// Workflow convenience method tests
// =============================================================================

func TestAsPromptTemplateNodeConvenience(t *testing.T) {
	wf := NewWorkflow[map[string]any, []*Message]()
	tmpl := NewMessageTemplate("test")
	_ = wf.AsPromptTemplateNode("prompt", tmpl)
	if _, ok := wf.workflowNodes["prompt"]; !ok {
		t.Fatal("AsPromptTemplateNode should create a workflow node")
	}
}

func TestAsToolsNodeConvenience(t *testing.T) {
	wf := NewWorkflow[*Message, *Message]()
	_ = wf.AsToolsNode("tools", &stubWeatherTool{})
	if _, ok := wf.workflowNodes["tools"]; !ok {
		t.Fatal("AsToolsNode should create a workflow node")
	}
}

// =============================================================================
// Edge case tests
// =============================================================================

func TestToolsNodeBridgeInvalidArgs(t *testing.T) {
	tools := &toolsNodeBridge{
		tools: map[string]BridgeTool{
			"get_weather": &stubWeatherTool{},
		},
	}
	lambda := tools.toLambda()

	msg := &Message{
		Role:    Assistant,
		Content: "",
		ToolCalls: []ToolCall{
			{
				ID:   "call_1",
				Type: "function",
				Function: ToolCallFunction{
					Name:      "get_weather",
					Arguments: `invalid json`,
				},
			},
		},
	}

	_, err := lambda.invokeFn(context.Background(), msg)
	if err == nil {
		t.Fatal("expected error for invalid JSON arguments")
	}
}

func TestToolsNodeBridgeNilMessage(t *testing.T) {
	tools := &toolsNodeBridge{
		tools: map[string]BridgeTool{
			"get_weather": &stubWeatherTool{},
		},
	}
	lambda := tools.toLambda()

	_, err := lambda.invokeFn(context.Background(), nil)
	if err == nil {
		t.Fatal("expected error for nil input")
	}
}
