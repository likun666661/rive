package compose

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"
)

// helper to create a fake chat model that returns a canned message
func newCannedChatModel(content string, toolCalls []ToolCall) *FakeChatModel {
	return NewFakeChatModel(
		WithChatGenerateFunc(func(ctx context.Context, input []*Message) (*Message, error) {
			return &Message{
				Role:      Assistant,
				Content:   content,
				ToolCalls: toolCalls,
			}, nil
		}),
	)
}

// helper to create a fake chat model that records its input for inspection
func newRecordingChatModel(record *[]*Message, content string) *FakeChatModel {
	return NewFakeChatModel(
		WithChatGenerateFunc(func(ctx context.Context, input []*Message) (*Message, error) {
			*record = append([]*Message{}, input...)
			return &Message{Role: Assistant, Content: content}, nil
		}),
	)
}

func TestMultiAgent_SingleSpecialist_SingleIntent(t *testing.T) {
	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "math_expert", Arguments: `{"reason":"solve"}`}},
	})

	mathExpert := newCannedChatModel("The answer is 42.", nil)

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{Name: "math_expert", IntendedUse: "solves math problems", ChatModel: mathExpert},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	result, err := agent.Invoke(context.Background(), []*Message{
		{Role: User, Content: "What is 6 * 7?"},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if result.Content != "The answer is 42." {
		t.Fatalf("expected 'The answer is 42.', got %q", result.Content)
	}
}

func TestMultiAgent_MultiSpecialist_MultiIntent(t *testing.T) {
	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "math_expert", Arguments: `{"reason":"math"}`}},
		{ID: "2", Function: ToolCallFunction{Name: "code_expert", Arguments: `{"reason":"code"}`}},
	})

	mathExpert := newCannedChatModel("Answer: 42", nil)
	codeExpert := newCannedChatModel("print(42)", nil)

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{Name: "math_expert", IntendedUse: "solves math", ChatModel: mathExpert},
			{Name: "code_expert", IntendedUse: "writes code", ChatModel: codeExpert},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	result, err := agent.Invoke(context.Background(), []*Message{
		{Role: User, Content: "Solve and code 6*7"},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if !strings.Contains(result.Content, "math_expert") || !strings.Contains(result.Content, "code_expert") {
		t.Fatalf("expected default summarizer output containing both specialist names, got %q", result.Content)
	}
	if !strings.Contains(result.Content, "42") {
		t.Fatalf("expected output containing '42', got %q", result.Content)
	}
}

func TestMultiAgent_NoSpecialist_DirectAnswer(t *testing.T) {
	host := newCannedChatModel("I can help with that directly.", nil)

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{Name: "unused", IntendedUse: "unused", ChatModel: newCannedChatModel("should not be called", nil)},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	result, err := agent.Invoke(context.Background(), []*Message{
		{Role: User, Content: "Hello"},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if result.Content != "I can help with that directly." {
		t.Fatalf("expected direct host answer, got %q", result.Content)
	}
}

func TestMultiAgent_Specialist_ChatModel(t *testing.T) {
	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "chat_expert", Arguments: `{}`}},
	})

	chatExpert := newCannedChatModel("chat model answer", nil)

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{Name: "chat_expert", IntendedUse: "answers questions", ChatModel: chatExpert},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	result, err := agent.Invoke(context.Background(), []*Message{
		{Role: User, Content: "Ask the expert"},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if result.Content != "chat model answer" {
		t.Fatalf("expected 'chat model answer', got %q", result.Content)
	}
}

func TestMultiAgent_Specialist_Invokable(t *testing.T) {
	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "fn_expert", Arguments: `{}`}},
	})

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{
				Name:        "fn_expert",
				IntendedUse: "invokes a function",
				Invokable: func(ctx context.Context, input []*Message) (*Message, error) {
					return &Message{Role: Assistant, Content: "invokable result"}, nil
				},
			},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	result, err := agent.Invoke(context.Background(), []*Message{
		{Role: User, Content: "Call the function"},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if result.Content != "invokable result" {
		t.Fatalf("expected 'invokable result', got %q", result.Content)
	}
}

func TestMultiAgent_Specialist_Streamable(t *testing.T) {
	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "stream_expert", Arguments: `{}`}},
	})

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{
				Name:        "stream_expert",
				IntendedUse: "streams a response",
				Streamable: func(ctx context.Context, input []*Message) (StreamReader[*Message], error) {
					return chatMessageStreamFromSlice(
						&Message{Role: Assistant, Content: "streaming result"},
					), nil
				},
			},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	result, err := agent.Invoke(context.Background(), []*Message{
		{Role: User, Content: "Stream a response"},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if result.Content != "streaming result" {
		t.Fatalf("expected 'streaming result', got %q", result.Content)
	}
}

func TestMultiAgent_PreHandler_InputReplacement(t *testing.T) {
	var specialistInput []*Message

	specialist := NewFakeChatModel(
		WithChatGenerateFunc(func(ctx context.Context, input []*Message) (*Message, error) {
			specialistInput = append([]*Message{}, input...)
			return &Message{Role: Assistant, Content: "received"}, nil
		}),
	)

	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "expert", Arguments: `{"reason":"delegate"}`}},
	})

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{Name: "expert", IntendedUse: "expert", ChatModel: specialist},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	originalMsgs := []*Message{
		{Role: User, Content: "What is the weather?"},
		{Role: User, Content: "In Tokyo"},
	}
	_, err = agent.Invoke(context.Background(), originalMsgs)
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if len(specialistInput) != 2 {
		t.Fatalf("expected specialist to receive 2 messages (original user history), got %d", len(specialistInput))
	}
	if specialistInput[0].Content != "What is the weather?" {
		t.Fatalf("expected first message 'What is the weather?', got %q", specialistInput[0].Content)
	}
	if specialistInput[1].Content != "In Tokyo" {
		t.Fatalf("expected second message 'In Tokyo', got %q", specialistInput[1].Content)
	}
}

func TestMultiAgent_DefaultSummarization(t *testing.T) {
	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "expert_a", Arguments: `{}`}},
		{ID: "2", Function: ToolCallFunction{Name: "expert_b", Arguments: `{}`}},
	})

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{Name: "expert_a", IntendedUse: "expert a", ChatModel: newCannedChatModel("Answer from A", nil)},
			{Name: "expert_b", IntendedUse: "expert b", ChatModel: newCannedChatModel("Answer from B", nil)},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	result, err := agent.Invoke(context.Background(), []*Message{
		{Role: User, Content: "Ask both"},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if !strings.Contains(result.Content, "[expert_a]") {
		t.Fatalf("expected default summary to label expert_a, got %q", result.Content)
	}
	if !strings.Contains(result.Content, "[expert_b]") {
		t.Fatalf("expected default summary to label expert_b, got %q", result.Content)
	}
	if !strings.Contains(result.Content, "Answer from A") {
		t.Fatalf("expected summary to contain Answer from A, got %q", result.Content)
	}
	if !strings.Contains(result.Content, "Answer from B") {
		t.Fatalf("expected summary to contain Answer from B, got %q", result.Content)
	}
}

func TestMultiAgent_CustomSummarizer(t *testing.T) {
	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "expert_a", Arguments: `{}`}},
		{ID: "2", Function: ToolCallFunction{Name: "expert_b", Arguments: `{}`}},
	})

	summaryModel := newCannedChatModel("Synthesized summary by custom model", nil)

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{Name: "expert_a", IntendedUse: "expert a", ChatModel: newCannedChatModel("Answer from A", nil)},
			{Name: "expert_b", IntendedUse: "expert b", ChatModel: newCannedChatModel("Answer from B", nil)},
		},
		Summarizer: &Summarizer{
			ChatModel:    summaryModel,
			SystemPrompt: "Synthesize the following expert answers concisely.",
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	result, err := agent.Invoke(context.Background(), []*Message{
		{Role: User, Content: "Ask both"},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if result.Content != "Synthesized summary by custom model" {
		t.Fatalf("expected custom summarizer output, got %q", result.Content)
	}
}

func TestMultiAgent_InvalidSpecialistName(t *testing.T) {
	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "nonexistent", Arguments: `{}`}},
	})

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{Name: "real_expert", IntendedUse: "real", ChatModel: newCannedChatModel("answer", nil)},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	_, err = agent.Invoke(context.Background(), []*Message{
		{Role: User, Content: "Call nonexistent"},
	})
	if err == nil {
		t.Fatal("expected error for invalid specialist name")
	}
	if !strings.Contains(err.Error(), "nonexistent") {
		t.Fatalf("expected error to mention 'nonexistent', got %v", err)
	}
	if !strings.Contains(err.Error(), "no specialist") {
		t.Fatalf("expected error to say 'no specialist', got %v", err)
	}
}

func TestMultiAgent_EmptySpecialists(t *testing.T) {
	config := &MultiAgentConfig{
		Host:        newCannedChatModel("hello", nil),
		Specialists: []*Specialist{},
	}

	_, err := NewMultiAgent(context.Background(), config)
	if err == nil {
		t.Fatal("expected error for empty specialists")
	}
	if !strings.Contains(err.Error(), "empty") {
		t.Fatalf("expected error to mention empty, got %v", err)
	}
}

func TestMultiAgent_NilHostChatModel(t *testing.T) {
	config := &MultiAgentConfig{
		Host: nil,
		Specialists: []*Specialist{
			{Name: "expert", IntendedUse: "expert", ChatModel: newCannedChatModel("answer", nil)},
		},
	}

	_, err := NewMultiAgent(context.Background(), config)
	if err == nil {
		t.Fatal("expected error for nil host ChatModel")
	}
	if !strings.Contains(err.Error(), "Host") {
		t.Fatalf("expected error to mention Host, got %v", err)
	}
}

func TestMultiAgent_NilConfig(t *testing.T) {
	_, err := NewMultiAgent(context.Background(), nil)
	if err == nil {
		t.Fatal("expected error for nil config")
	}
}

func TestMultiAgent_DuplicateSpecialistNames(t *testing.T) {
	config := &MultiAgentConfig{
		Host: newCannedChatModel("", nil),
		Specialists: []*Specialist{
			{Name: "expert", IntendedUse: "first", ChatModel: newCannedChatModel("a", nil)},
			{Name: "expert", IntendedUse: "second", ChatModel: newCannedChatModel("b", nil)},
		},
	}

	_, err := NewMultiAgent(context.Background(), config)
	if err == nil {
		t.Fatal("expected error for duplicate specialist names")
	}
	if !strings.Contains(err.Error(), "duplicate") {
		t.Fatalf("expected error to mention duplicate, got %v", err)
	}
}

func TestMultiAgent_SpecialistWithSystemPrompt(t *testing.T) {
	var specialistInput []*Message

	specialist := NewFakeChatModel(
		WithChatGenerateFunc(func(ctx context.Context, input []*Message) (*Message, error) {
			specialistInput = append([]*Message{}, input...)
			return &Message{Role: Assistant, Content: "answered"}, nil
		}),
	)

	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "prompted_expert", Arguments: `{}`}},
	})

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{
				Name:         "prompted_expert",
				IntendedUse:  "expert with system prompt",
				ChatModel:    specialist,
				SystemPrompt: "You are a helpful assistant.",
			},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	_, err = agent.Invoke(context.Background(), []*Message{
		{Role: User, Content: "Hello"},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if len(specialistInput) < 1 {
		t.Fatal("expected specialist to receive at least 1 message")
	}
	if specialistInput[0].Role != System {
		t.Fatalf("expected first message to be system, got role %q", specialistInput[0].Role)
	}
	if specialistInput[0].Content != "You are a helpful assistant." {
		t.Fatalf("expected system prompt, got %q", specialistInput[0].Content)
	}
}

func TestMultiAgent_StateIsolation(t *testing.T) {
	host1 := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "expert", Arguments: `{}`}},
	})
	host2 := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "expert", Arguments: `{}`}},
	})

	expert1 := newCannedChatModel("answer from agent 1", nil)
	expert2 := newCannedChatModel("answer from agent 2", nil)

	config1 := &MultiAgentConfig{
		Host:        host1,
		Specialists: []*Specialist{{Name: "expert", IntendedUse: "e", ChatModel: expert1}},
	}
	config2 := &MultiAgentConfig{
		Host:        host2,
		Specialists: []*Specialist{{Name: "expert", IntendedUse: "e", ChatModel: expert2}},
	}

	agent1, err := NewMultiAgent(context.Background(), config1)
	if err != nil {
		t.Fatalf("NewMultiAgent 1 failed: %v", err)
	}
	agent2, err := NewMultiAgent(context.Background(), config2)
	if err != nil {
		t.Fatalf("NewMultiAgent 2 failed: %v", err)
	}

	r1, err := agent1.Invoke(context.Background(), []*Message{{Role: User, Content: "q1"}})
	if err != nil {
		t.Fatalf("Invoke 1 failed: %v", err)
	}
	r2, err := agent2.Invoke(context.Background(), []*Message{{Role: User, Content: "q2"}})
	if err != nil {
		t.Fatalf("Invoke 2 failed: %v", err)
	}

	if r1.Content != "answer from agent 1" {
		t.Fatalf("agent 1: expected 'answer from agent 1', got %q", r1.Content)
	}
	if r2.Content != "answer from agent 2" {
		t.Fatalf("agent 2: expected 'answer from agent 2', got %q", r2.Content)
	}
}

func TestMultiAgent_MultipleToolCallsSameSpecialist(t *testing.T) {
	callCount := 0
	specialist := NewFakeChatModel(
		WithChatGenerateFunc(func(ctx context.Context, input []*Message) (*Message, error) {
			callCount++
			return &Message{Role: Assistant, Content: "result"}, nil
		}),
	)

	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "expert", Arguments: `{}`}},
		{ID: "2", Function: ToolCallFunction{Name: "expert", Arguments: `{}`}},
	})

	config := &MultiAgentConfig{
		Host:        host,
		Specialists: []*Specialist{{Name: "expert", IntendedUse: "expert", ChatModel: specialist}},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	result, err := agent.Invoke(context.Background(), []*Message{{Role: User, Content: "ask twice"}})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if callCount != 2 {
		t.Fatalf("expected specialist to be called 2 times, got %d", callCount)
	}
	if !strings.Contains(result.Content, "[expert]") {
		t.Fatalf("expected default summarizer to contain [expert], got %q", result.Content)
	}
}

func TestMultiAgent_SpecialistEmptyName(t *testing.T) {
	config := &MultiAgentConfig{
		Host: newCannedChatModel("", nil),
		Specialists: []*Specialist{
			{Name: "", IntendedUse: "no name", ChatModel: newCannedChatModel("a", nil)},
		},
	}

	_, err := NewMultiAgent(context.Background(), config)
	if err == nil {
		t.Fatal("expected error for empty specialist name")
	}
}

func TestMultiAgent_HostModelError(t *testing.T) {
	host := NewFakeChatModel(
		WithChatGenerateFunc(func(ctx context.Context, input []*Message) (*Message, error) {
			return nil, errors.New("host model failure")
		}),
	)

	config := &MultiAgentConfig{
		Host:        host,
		Specialists: []*Specialist{{Name: "expert", IntendedUse: "expert", ChatModel: newCannedChatModel("ok", nil)}},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	_, err = agent.Invoke(context.Background(), []*Message{{Role: User, Content: "test"}})
	if err == nil {
		t.Fatal("expected error from host model failure")
	}
	if !strings.Contains(err.Error(), "host model") {
		t.Fatalf("expected error to mention host model, got %v", err)
	}
}

func TestMultiAgent_SpecialistError(t *testing.T) {
	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "expert", Arguments: `{}`}},
	})

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{
				Name:        "expert",
				IntendedUse: "expert",
				Invokable: func(ctx context.Context, input []*Message) (*Message, error) {
					return nil, errors.New("specialist failure")
				},
			},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	_, err = agent.Invoke(context.Background(), []*Message{{Role: User, Content: "test"}})
	if err == nil {
		t.Fatal("expected error from specialist failure")
	}
	if !strings.Contains(err.Error(), "specialist") {
		t.Fatalf("expected error to mention specialist, got %v", err)
	}
}

func TestMultiAgent_CustomSummarizerWithSystemPrompt(t *testing.T) {
	var summarizerInput []*Message

	summaryModel := NewFakeChatModel(
		WithChatGenerateFunc(func(ctx context.Context, input []*Message) (*Message, error) {
			summarizerInput = append([]*Message{}, input...)
			return &Message{Role: Assistant, Content: "custom summary"}, nil
		}),
	)

	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "a", Arguments: `{}`}},
		{ID: "2", Function: ToolCallFunction{Name: "b", Arguments: `{}`}},
	})

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{Name: "a", IntendedUse: "a", ChatModel: newCannedChatModel("A answer", nil)},
			{Name: "b", IntendedUse: "b", ChatModel: newCannedChatModel("B answer", nil)},
		},
		Summarizer: &Summarizer{
			ChatModel:    summaryModel,
			SystemPrompt: "Synthesize concisely.",
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	_, err = agent.Invoke(context.Background(), []*Message{{Role: User, Content: "ask"}})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if len(summarizerInput) < 1 {
		t.Fatal("expected summarizer to receive at least 1 message")
	}
	if summarizerInput[0].Role != System {
		t.Fatalf("expected first summarizer message to be system, got role %q", summarizerInput[0].Role)
	}
	if summarizerInput[0].Content != "Synthesize concisely." {
		t.Fatalf("expected system prompt 'Synthesize concisely.', got %q", summarizerInput[0].Content)
	}
}

func TestMultiAgent_NilSpecialistInList(t *testing.T) {
	config := &MultiAgentConfig{
		Host: newCannedChatModel("", nil),
		Specialists: []*Specialist{
			{Name: "ok", IntendedUse: "ok", ChatModel: newCannedChatModel("a", nil)},
			nil,
		},
	}

	_, err := NewMultiAgent(context.Background(), config)
	if err == nil {
		t.Fatal("expected error for nil specialist in list")
	}
}

func TestMultiAgent_Stream(t *testing.T) {
	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "expert", Arguments: `{}`}},
	})

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{Name: "expert", IntendedUse: "expert", ChatModel: newCannedChatModel("stream answer", nil)},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	sr, err := agent.Stream(context.Background(), []*Message{{Role: User, Content: "test"}})
	if err != nil {
		t.Fatalf("Stream failed: %v", err)
	}

	msg, err := sr.Recv()
	if err != nil {
		t.Fatalf("Recv failed: %v", err)
	}
	if msg.Content != "stream answer" {
		t.Fatalf("expected 'stream answer', got %q", msg.Content)
	}
}

func TestMultiAgent_LargeMultiIntent(t *testing.T) {
	specialistCount := 5
	toolCalls := make([]ToolCall, specialistCount)
	specialists := make([]*Specialist, specialistCount)
	for i := 0; i < specialistCount; i++ {
		name := fmt.Sprintf("expert_%d", i)
		toolCalls[i] = ToolCall{ID: fmt.Sprintf("id_%d", i), Function: ToolCallFunction{Name: name, Arguments: `{}`}}
		specialists[i] = &Specialist{Name: name, IntendedUse: fmt.Sprintf("expert %d", i), ChatModel: newCannedChatModel(fmt.Sprintf("Answer from expert_%d", i), nil)}
	}

	host := newCannedChatModel("", toolCalls)

	config := &MultiAgentConfig{
		Host:        host,
		Specialists: specialists,
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	result, err := agent.Invoke(context.Background(), []*Message{{Role: User, Content: "ask all"}})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	for i := 0; i < specialistCount; i++ {
		expected := fmt.Sprintf("Answer from expert_%d", i)
		if !strings.Contains(result.Content, expected) {
			t.Fatalf("expected result to contain %q, got %q", expected, result.Content)
		}
	}
}

func TestMultiAgent_AgentAsSpecialist(t *testing.T) {
	host := newCannedChatModel("", []ToolCall{
		{ID: "1", Function: ToolCallFunction{Name: "agent_expert", Arguments: `{}`}},
	})

	config := &MultiAgentConfig{
		Host: host,
		Specialists: []*Specialist{
			{
				Name:        "agent_expert",
				IntendedUse: "agent-based specialist",
				Invokable: func(ctx context.Context, input []*Message) (*Message, error) {
					return &Message{Role: Assistant, Content: "agent specialist answer"}, nil
				},
			},
		},
	}

	agent, err := NewMultiAgent(context.Background(), config)
	if err != nil {
		t.Fatalf("NewMultiAgent failed: %v", err)
	}

	result, err := agent.Invoke(context.Background(), []*Message{{Role: User, Content: "ask agent"}})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if result.Content != "agent specialist answer" {
		t.Fatalf("expected 'agent specialist answer', got %q", result.Content)
	}

	callCount := 0
	config2 := &MultiAgentConfig{
		Host: newCannedChatModel("", []ToolCall{
			{ID: "1", Function: ToolCallFunction{Name: "counter", Arguments: `{}`}},
		}),
		Specialists: []*Specialist{
			{
				Name:        "counter",
				IntendedUse: "counts invocations",
				Invokable: func(ctx context.Context, input []*Message) (*Message, error) {
					callCount++
					return &Message{Role: Assistant, Content: fmt.Sprintf("call %d", callCount)}, nil
				},
			},
		},
	}

	agent2, err := NewMultiAgent(context.Background(), config2)
	if err != nil {
		t.Fatalf("NewMultiAgent 2 failed: %v", err)
	}

	r1, _ := agent2.Invoke(context.Background(), []*Message{{Role: User, Content: "a"}})
	r2, _ := agent2.Invoke(context.Background(), []*Message{{Role: User, Content: "b"}})

	if callCount != 2 {
		t.Fatalf("expected specialist called 2 times, got %d", callCount)
	}
	if r1.Content != "call 1" {
		t.Fatalf("expected 'call 1', got %q", r1.Content)
	}
	if r2.Content != "call 2" {
		t.Fatalf("expected 'call 2', got %q", r2.Content)
	}
}
