package agent

import (
	"context"
	"errors"
	"io"
	"strings"
	"testing"

	"github.com/rive/eino-compose-runtime-replica-go/compose"
)

func newScriptedChatModel(responses []*compose.Message) *compose.FakeChatModel {
	callCount := 0
	return compose.NewFakeChatModel(
		compose.WithChatGenerateFunc(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
			if callCount >= len(responses) {
				return &compose.Message{Role: compose.Assistant, Content: "done"}, nil
			}
			resp := responses[callCount]
			callCount++
			return resp, nil
		}),
	)
}

func newCannedTool(name, desc, result string, directReturn bool) compose.InvokableTool {
	return &cannedTool{name: name, desc: desc, result: result, directReturn: directReturn}
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
		if err := SetReturnDirectly(ctx); err != nil {
			return "", err
		}
	}
	return t.result, nil
}

func streamFromMessages(msgs ...*compose.Message) compose.StreamReader[*compose.Message] {
	return &messageSliceStream{msgs: msgs}
}

type messageSliceStream struct {
	msgs []*compose.Message
	pos  int
}

func (s *messageSliceStream) Recv() (*compose.Message, error) {
	if s.pos >= len(s.msgs) {
		return nil, io.EOF
	}
	m := s.msgs[s.pos]
	s.pos++
	return m, nil
}

func TestReAct_NoTools_ReturnsModelOutput(t *testing.T) {
	ctx := context.Background()
	model := compose.NewFakeChatModel(
		compose.WithChatGenerateFunc(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
			return &compose.Message{Role: compose.Assistant, Content: "hello"}, nil
		}),
	)

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		MaxStep:   10,
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	result, err := agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "hi"},
	})
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if result.Content != "hello" {
		t.Fatalf("expected 'hello', got %q", result.Content)
	}
}

func TestReAct_SingleToolCall(t *testing.T) {
	ctx := context.Background()
	tool := newCannedTool("search", "search tool", "search result: 42", false)

	model := newScriptedChatModel([]*compose.Message{
		{
			Role:      compose.Assistant,
			ToolCalls: []compose.ToolCall{{ID: "1", Function: compose.ToolCallFunction{Name: "search", Arguments: `{"q":"hello"}`}}},
		},
		{Role: compose.Assistant, Content: "The answer is 42."},
	})

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		ToolsConfig: compose.ToolsNodeConfig{
			Tools: []compose.InvokableTool{tool},
		},
		MaxStep: 20,
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	result, err := agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "search for 42"},
	})
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if result.Content != "The answer is 42." {
		t.Fatalf("expected 'The answer is 42.', got %q", result.Content)
	}
}

func TestReAct_MultiRoundToolCall(t *testing.T) {
	ctx := context.Background()
	tool1 := newCannedTool("search", "search tool", "found it", false)
	tool2 := newCannedTool("calc", "calc tool", "calc result", false)

	model := newScriptedChatModel([]*compose.Message{
		{
			Role:      compose.Assistant,
			ToolCalls: []compose.ToolCall{{ID: "1", Function: compose.ToolCallFunction{Name: "search", Arguments: `{}`}}},
		},
		{
			Role:      compose.Assistant,
			ToolCalls: []compose.ToolCall{{ID: "2", Function: compose.ToolCallFunction{Name: "calc", Arguments: `{}`}}},
		},
		{Role: compose.Assistant, Content: "final answer"},
	})

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		ToolsConfig: compose.ToolsNodeConfig{
			Tools: []compose.InvokableTool{tool1, tool2},
		},
		MaxStep: 20,
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	result, err := agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "multi"},
	})
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if result.Content != "final answer" {
		t.Fatalf("expected 'final answer', got %q", result.Content)
	}
}

func TestReAct_MaxStepEnforced(t *testing.T) {
	ctx := context.Background()
	tool := newCannedTool("loop", "looping tool", "result", false)

	model := compose.NewFakeChatModel(
		compose.WithChatGenerateFunc(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
			return &compose.Message{
				Role:      compose.Assistant,
				ToolCalls: []compose.ToolCall{{ID: "x", Function: compose.ToolCallFunction{Name: "loop", Arguments: `{}`}}},
			}, nil
		}),
	)

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		ToolsConfig: compose.ToolsNodeConfig{
			Tools: []compose.InvokableTool{tool},
		},
		MaxStep: 3,
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	_, err = agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "loop forever"},
	})
	if err == nil {
		t.Fatal("expected error due to max steps, got nil")
	}
	if !errors.Is(err, compose.ErrExceedMaxSteps) {
		t.Fatalf("expected ErrExceedMaxSteps, got %v", err)
	}
}

func TestReAct_ReturnDirectly_Config(t *testing.T) {
	ctx := context.Background()
	tool := newCannedTool("search", "search tool", "direct result", false)

	model := newScriptedChatModel([]*compose.Message{
		{
			Role:      compose.Assistant,
			ToolCalls: []compose.ToolCall{{ID: "1", Function: compose.ToolCallFunction{Name: "search", Arguments: `{}`}}},
		},
		{Role: compose.Assistant, Content: "should not see this"},
	})

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		ToolsConfig: compose.ToolsNodeConfig{
			Tools: []compose.InvokableTool{tool},
		},
		MaxStep:            20,
		ToolReturnDirectly: map[string]bool{"search": true},
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	result, err := agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "search"},
	})
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if !strings.Contains(result.Content, "direct result") {
		t.Fatalf("expected tool result 'direct result', got %q", result.Content)
	}
}

func TestReAct_ReturnDirectly_Runtime(t *testing.T) {
	ctx := context.Background()
	tool := newCannedTool("search", "search tool", "runtime direct", true)

	model := newScriptedChatModel([]*compose.Message{
		{
			Role:      compose.Assistant,
			ToolCalls: []compose.ToolCall{{ID: "1", Function: compose.ToolCallFunction{Name: "search", Arguments: `{}`}}},
		},
		{Role: compose.Assistant, Content: "should not see this"},
	})

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		ToolsConfig: compose.ToolsNodeConfig{
			Tools: []compose.InvokableTool{tool},
		},
		MaxStep: 20,
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	result, err := agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "search"},
	})
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if !strings.Contains(result.Content, "runtime direct") {
		t.Fatalf("expected tool result 'runtime direct', got %q", result.Content)
	}
}

func TestReAct_MessageModifier_Persistent(t *testing.T) {
	ctx := context.Background()
	tool := newCannedTool("search", "search tool", "result", false)

	modifierCallCount := 0
	model := newScriptedChatModel([]*compose.Message{
		{
			Role:      compose.Assistant,
			ToolCalls: []compose.ToolCall{{ID: "1", Function: compose.ToolCallFunction{Name: "search", Arguments: `{}`}}},
		},
		{Role: compose.Assistant, Content: "final"},
	})

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		ToolsConfig: compose.ToolsNodeConfig{
			Tools: []compose.InvokableTool{tool},
		},
		MaxStep: 20,
		MessageModifier: func(ctx context.Context, msgs []*compose.Message) []*compose.Message {
			modifierCallCount++
			sysMsg := compose.SystemMessage("you are a helpful assistant")
			return append([]*compose.Message{sysMsg}, msgs...)
		},
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	_, err = agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "hello"},
	})
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if modifierCallCount != 2 {
		t.Fatalf("expected 2 modifier calls (one per model round), got %d", modifierCallCount)
	}
}

func TestReAct_MessageRewriter_Compression(t *testing.T) {
	ctx := context.Background()
	tool := newCannedTool("search", "search tool", "result", false)

	var stateLenAfterRewriter int
	model := newScriptedChatModel([]*compose.Message{
		{
			Role:      compose.Assistant,
			ToolCalls: []compose.ToolCall{{ID: "1", Function: compose.ToolCallFunction{Name: "search", Arguments: `{}`}}},
		},
		{Role: compose.Assistant, Content: "final"},
	})

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		ToolsConfig: compose.ToolsNodeConfig{
			Tools: []compose.InvokableTool{tool},
		},
		MaxStep: 20,
		MessageRewriter: func(ctx context.Context, msgs []*compose.Message) []*compose.Message {
			if len(msgs) > 3 {
				return msgs[len(msgs)-3:]
			}
			return msgs
		},
		MessageModifier: func(ctx context.Context, msgs []*compose.Message) []*compose.Message {
			stateLenAfterRewriter = len(msgs)
			return msgs
		},
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	_, err = agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "hello"},
	})
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if stateLenAfterRewriter > 3 {
		t.Fatalf("expected <=3 messages after rewriter, got %d", stateLenAfterRewriter)
	}
}

func TestReAct_MessageRewriter_Ordering(t *testing.T) {
	ctx := context.Background()

	var modifierSawSystem bool
	model := compose.NewFakeChatModel(
		compose.WithChatGenerateFunc(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
			return &compose.Message{Role: compose.Assistant, Content: "ok"}, nil
		}),
	)

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		MaxStep:   20,
		MessageRewriter: func(ctx context.Context, msgs []*compose.Message) []*compose.Message {
			return append([]*compose.Message{compose.SystemMessage("rewrote")}, msgs...)
		},
		MessageModifier: func(ctx context.Context, msgs []*compose.Message) []*compose.Message {
			for _, m := range msgs {
				if m.Content == "rewrote" {
					modifierSawSystem = true
				}
			}
			return msgs
		},
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	_, err = agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "test"},
	})
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if !modifierSawSystem {
		t.Fatal("MessageModifier should see rewriter's changes (rewriter runs first)")
	}
}

func TestReAct_StreamToolCallChecker_Default(t *testing.T) {
	ctx := context.Background()
	checker := DefaultStreamToolCallChecker

	sr := streamFromMessages(
		&compose.Message{Content: "", ToolCalls: nil},
		&compose.Message{Role: compose.Assistant, ToolCalls: []compose.ToolCall{{ID: "1"}}},
	)
	hasToolCall, err := checker(ctx, sr)
	if err != nil {
		t.Fatalf("checker error: %v", err)
	}
	if !hasToolCall {
		t.Fatal("expected hasToolCall=true for OpenAI-style stream")
	}
}

func TestReAct_StreamToolCallChecker_ClaudeStyle(t *testing.T) {
	ctx := context.Background()

	sr := streamFromMessages(
		&compose.Message{Content: "I think I should search...", ToolCalls: nil},
		&compose.Message{ToolCalls: []compose.ToolCall{{ID: "1", Function: compose.ToolCallFunction{Name: "search"}}}},
	)
	hasToolCall, err := ScanAllStreamToolCallChecker(ctx, sr)
	if err != nil {
		t.Fatalf("checker error: %v", err)
	}
	if !hasToolCall {
		t.Fatal("expected hasToolCall=true for Claude-style stream")
	}
}

func TestReAct_StreamToolCallChecker_ScanAllNoToolCall(t *testing.T) {
	ctx := context.Background()

	sr := streamFromMessages(
		&compose.Message{Content: "hello", ToolCalls: nil},
		&compose.Message{Content: " world", ToolCalls: nil},
	)
	hasToolCall, err := ScanAllStreamToolCallChecker(ctx, sr)
	if err != nil {
		t.Fatalf("checker error: %v", err)
	}
	if hasToolCall {
		t.Fatal("expected hasToolCall=false for text-only stream")
	}
}

func TestReAct_EmptyInput(t *testing.T) {
	ctx := context.Background()
	model := compose.NewFakeChatModel()

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		MaxStep:   10,
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	result, err := agent.Generate(ctx, []*compose.Message{})
	if err != nil {
		t.Fatalf("Generate with empty input: %v", err)
	}
	if result == nil {
		t.Fatal("expected non-nil result")
	}
}

func TestReAct_NilConfig(t *testing.T) {
	ctx := context.Background()
	_, err := NewAgent(ctx, nil)
	if err == nil {
		t.Fatal("expected error for nil config")
	}
}

func TestReAct_NilChatModel(t *testing.T) {
	ctx := context.Background()
	_, err := NewAgent(ctx, &AgentConfig{
		MaxStep: 10,
	})
	if err == nil {
		t.Fatal("expected error for nil ChatModel")
	}
}

func TestReAct_NoToolsConfig(t *testing.T) {
	ctx := context.Background()
	model := compose.NewFakeChatModel(
		compose.WithChatGenerateFunc(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
			return &compose.Message{Role: compose.Assistant, Content: "answer"}, nil
		}),
	)

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		MaxStep:   10,
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	result, err := agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "question"},
	})
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if result.Content != "answer" {
		t.Fatalf("expected 'answer', got %q", result.Content)
	}
}

func TestReAct_StateIsolation(t *testing.T) {
	ctx := context.Background()

	model1 := compose.NewFakeChatModel(
		compose.WithChatGenerateFunc(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
			return &compose.Message{Role: compose.Assistant, Content: "agent1"}, nil
		}),
	)
	model2 := compose.NewFakeChatModel(
		compose.WithChatGenerateFunc(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
			return &compose.Message{Role: compose.Assistant, Content: "agent2"}, nil
		}),
	)

	agent1, err := NewAgent(ctx, &AgentConfig{ChatModel: model1, MaxStep: 10})
	if err != nil {
		t.Fatalf("NewAgent1: %v", err)
	}
	agent2, err := NewAgent(ctx, &AgentConfig{ChatModel: model2, MaxStep: 10})
	if err != nil {
		t.Fatalf("NewAgent2: %v", err)
	}

	r1, _ := agent1.Generate(ctx, []*compose.Message{{Role: compose.User, Content: "a"}})
	r2, _ := agent2.Generate(ctx, []*compose.Message{{Role: compose.User, Content: "b"}})

	if r1.Content != "agent1" {
		t.Fatalf("agent1: %q", r1.Content)
	}
	if r2.Content != "agent2" {
		t.Fatalf("agent2: %q", r2.Content)
	}
}

func TestReAct_SetReturnDirectly_Priority(t *testing.T) {
	ctx := context.Background()
	tool := newCannedTool("search", "search tool", "runtime wins", true)

	model := newScriptedChatModel([]*compose.Message{
		{
			Role:      compose.Assistant,
			ToolCalls: []compose.ToolCall{{ID: "1", Function: compose.ToolCallFunction{Name: "search", Arguments: `{}`}}},
		},
		{Role: compose.Assistant, Content: "should not see"},
	})

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		ToolsConfig: compose.ToolsNodeConfig{
			Tools: []compose.InvokableTool{tool},
		},
		MaxStep:            20,
		ToolReturnDirectly: map[string]bool{"search": true},
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	result, err := agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "search"},
	})
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if !strings.Contains(result.Content, "runtime wins") {
		t.Fatalf("expected 'runtime wins', got %q", result.Content)
	}
}

func TestDefaultStreamToolCallChecker_FirstChunkEmpty(t *testing.T) {
	ctx := context.Background()
	sr := streamFromMessages(
		&compose.Message{Role: compose.Assistant, Content: "no tools"},
	)
	hasToolCall, err := DefaultStreamToolCallChecker(ctx, sr)
	if err != nil {
		t.Fatalf("checker error: %v", err)
	}
	if hasToolCall {
		t.Fatal("expected hasToolCall=false for text-only stream")
	}
}

func TestDefaultStreamToolCallChecker_FirstChunkToolCall(t *testing.T) {
	ctx := context.Background()
	sr := streamFromMessages(
		&compose.Message{Role: compose.Assistant, ToolCalls: []compose.ToolCall{{ID: "1"}}},
		&compose.Message{Content: "extra text"},
	)
	hasToolCall, err := DefaultStreamToolCallChecker(ctx, sr)
	if err != nil {
		t.Fatalf("checker error: %v", err)
	}
	if !hasToolCall {
		t.Fatal("expected hasToolCall=true")
	}
}

func TestReAct_StreamMode_Basic(t *testing.T) {
	ctx := context.Background()
	model := compose.NewFakeChatModel(
		compose.WithChatGenerateFunc(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
			return &compose.Message{Role: compose.Assistant, Content: "stream answer"}, nil
		}),
	)

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		MaxStep:   10,
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	sr, err := agent.Runnable.Stream(ctx, []*compose.Message{
		{Role: compose.User, Content: "hello"},
	})
	if err != nil {
		t.Fatalf("Stream: %v", err)
	}

	msg, err := sr.Recv()
	if err != nil {
		t.Fatalf("Recv: %v", err)
	}
	if msg.Content != "stream answer" {
		t.Fatalf("expected 'stream answer', got %q", msg.Content)
	}
}

func TestReAct_LargeMultiRound(t *testing.T) {
	ctx := context.Background()
	responses := make([]*compose.Message, 0, 10)
	for i := 0; i < 9; i++ {
		responses = append(responses, &compose.Message{
			Role:      compose.Assistant,
			ToolCalls: []compose.ToolCall{{ID: "x", Function: compose.ToolCallFunction{Name: "loop", Arguments: `{}`}}},
		})
	}
	responses = append(responses, &compose.Message{Role: compose.Assistant, Content: "final after 9 rounds"})

	tool := newCannedTool("loop", "looping", "result", false)
	model := newScriptedChatModel(responses)

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		ToolsConfig: compose.ToolsNodeConfig{
			Tools: []compose.InvokableTool{tool},
		},
		MaxStep: 50,
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	result, err := agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "loop"},
	})
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if result.Content != "final after 9 rounds" {
		t.Fatalf("expected 'final after 9 rounds', got %q", result.Content)
	}
}

func TestReAct_ToolCallWithMultipleTools(t *testing.T) {
	ctx := context.Background()
	tool1 := newCannedTool("search", "search tool", "search_result", false)
	tool2 := newCannedTool("calc", "calc tool", "calc_result", false)

	model := newScriptedChatModel([]*compose.Message{
		{
			Role: compose.Assistant,
			ToolCalls: []compose.ToolCall{
				{ID: "1", Function: compose.ToolCallFunction{Name: "search", Arguments: `{}`}},
				{ID: "2", Function: compose.ToolCallFunction{Name: "calc", Arguments: `{}`}},
			},
		},
		{Role: compose.Assistant, Content: "final"},
	})

	agent, err := NewAgent(ctx, &AgentConfig{
		ChatModel: model,
		ToolsConfig: compose.ToolsNodeConfig{
			Tools: []compose.InvokableTool{tool1, tool2},
		},
		MaxStep: 20,
	})
	if err != nil {
		t.Fatalf("NewAgent: %v", err)
	}

	result, err := agent.Generate(ctx, []*compose.Message{
		{Role: compose.User, Content: "do both"},
	})
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if result.Content != "final" {
		t.Fatalf("expected 'final', got %q", result.Content)
	}
}
