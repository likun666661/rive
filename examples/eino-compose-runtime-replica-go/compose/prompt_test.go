package compose

import (
	"context"
	"testing"
)

func TestMessageTemplateBasicFormat(t *testing.T) {
	mt := NewMessageTemplate("Hello, {{name}}!")
	msgs, err := mt.Format(context.Background(), map[string]any{"name": "Alice"})
	if err != nil {
		t.Fatal(err)
	}
	if len(msgs) != 1 {
		t.Fatalf("expected 1 message, got %d", len(msgs))
	}
	if msgs[0].Role != Human {
		t.Fatalf("expected Human role, got %q", msgs[0].Role)
	}
	if msgs[0].Content != "Hello, Alice!" {
		t.Fatalf("expected 'Hello, Alice!', got %q", msgs[0].Content)
	}
}

func TestMessageTemplateWithSystemTemplate(t *testing.T) {
	mt := NewMessageTemplate("{{query}}").
		WithSystemTemplate("You are {{role}}.")
	msgs, err := mt.Format(context.Background(), map[string]any{
		"role":  "assistant",
		"query": "What is Go?",
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(msgs) != 2 {
		t.Fatalf("expected 2 messages, got %d", len(msgs))
	}
	if msgs[0].Role != System {
		t.Fatalf("expected System role for first message, got %q", msgs[0].Role)
	}
	if msgs[0].Content != "You are assistant." {
		t.Fatalf("expected 'You are assistant.', got %q", msgs[0].Content)
	}
	if msgs[1].Role != Human {
		t.Fatalf("expected Human role for second message, got %q", msgs[1].Role)
	}
	if msgs[1].Content != "What is Go?" {
		t.Fatalf("expected 'What is Go?', got %q", msgs[1].Content)
	}
}

func TestMessageTemplateMissingVariable(t *testing.T) {
	mt := NewMessageTemplate("Hello, {{name}}! You are {{missing}}.")
	msgs, err := mt.Format(context.Background(), map[string]any{"name": "Alice"})
	if err != nil {
		t.Fatal(err)
	}
	if len(msgs) != 1 {
		t.Fatalf("expected 1 message, got %d", len(msgs))
	}
	expected := "Hello, Alice! You are {{missing}}."
	if msgs[0].Content != expected {
		t.Fatalf("expected %q, got %q", expected, msgs[0].Content)
	}
}

func TestMessageTemplateSpecialChars(t *testing.T) {
	mt := NewMessageTemplate("{{x}}")
	msgs, err := mt.Format(context.Background(), map[string]any{"x": "<tag> & \"quote\""})
	if err != nil {
		t.Fatal(err)
	}
	if msgs[0].Content != "<tag> & \"quote\"" {
		t.Fatalf("expected special chars preserved, got %q", msgs[0].Content)
	}
}

func TestMessageTemplateEmptyVarMap(t *testing.T) {
	mt := NewMessageTemplate("Hello, {{name}}!")
	msgs, err := mt.Format(context.Background(), nil)
	if err != nil {
		t.Fatal(err)
	}
	if msgs[0].Content != "Hello, {{name}}!" {
		t.Fatalf("expected 'Hello, {{name}}!', got %q", msgs[0].Content)
	}

	msgs, err = mt.Format(context.Background(), map[string]any{})
	if err != nil {
		t.Fatal(err)
	}
	if msgs[0].Content != "Hello, {{name}}!" {
		t.Fatalf("expected 'Hello, {{name}}!', got %q", msgs[0].Content)
	}
}

func TestMessageTemplateMultipleFormatIsolation(t *testing.T) {
	mt := NewMessageTemplate("Hello, {{name}}!")
	msgs1, err := mt.Format(context.Background(), map[string]any{"name": "Alice"})
	if err != nil {
		t.Fatal(err)
	}
	if msgs1[0].Content != "Hello, Alice!" {
		t.Fatalf("expected 'Hello, Alice!', got %q", msgs1[0].Content)
	}

	msgs2, err := mt.Format(context.Background(), map[string]any{"name": "Bob"})
	if err != nil {
		t.Fatal(err)
	}
	if msgs2[0].Content != "Hello, Bob!" {
		t.Fatalf("expected 'Hello, Bob!', got %q", msgs2[0].Content)
	}
}

func TestChatTemplateComponentGraphIntegration(t *testing.T) {
	mt := NewMessageTemplate("Echo: {{msg}}")
	comp := NewChatTemplateComponent(mt)
	cr := comp.GetRunnable()

	out, err := cr.invoke(context.Background(), map[string]any{"msg": "hello world"})
	if err != nil {
		t.Fatal(err)
	}

	msgs, ok := out.([]*Message)
	if !ok {
		t.Fatalf("expected []*Message output, got %T", out)
	}
	if len(msgs) != 1 {
		t.Fatalf("expected 1 message, got %d", len(msgs))
	}
	if msgs[0].Content != "Echo: hello world" {
		t.Fatalf("expected 'Echo: hello world', got %q", msgs[0].Content)
	}
}

func TestChatTemplateComponentWrongInput(t *testing.T) {
	mt := NewMessageTemplate("test")
	comp := NewChatTemplateComponent(mt)
	cr := comp.GetRunnable()

	_, err := cr.invoke(context.Background(), "bad input")
	if err == nil {
		t.Fatal("expected error for wrong input type")
	}
}

func TestFakeChatTemplate(t *testing.T) {
	fn := func(ctx context.Context, vs map[string]any) ([]*Message, error) {
		return []*Message{
			{Role: System, Content: "You are helpful."},
			{Role: Human, Content: "Hi"},
		}, nil
	}
	fct := NewFakeChatTemplate(fn)
	msgs, err := fct.Format(context.Background(), nil)
	if err != nil {
		t.Fatal(err)
	}
	if len(msgs) != 2 {
		t.Fatalf("expected 2 messages, got %d", len(msgs))
	}
	if msgs[0].Role != System || msgs[0].Content != "You are helpful." {
		t.Fatalf("unexpected first message: %+v", msgs[0])
	}
	if msgs[1].Role != Human || msgs[1].Content != "Hi" {
		t.Fatalf("unexpected second message: %+v", msgs[1])
	}
}

func TestMessageTemplateWithSystemTemplateChaining(t *testing.T) {
	mt := NewMessageTemplate("{{q}}").
		WithSystemTemplate("role: {{r}}")
	msgs, err := mt.Format(context.Background(), map[string]any{
		"r": "system",
		"q": "hello world",
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(msgs) != 2 {
		t.Fatalf("expected 2 messages, got %d", len(msgs))
	}
	if msgs[0].Role != System || msgs[0].Content != "role: system" {
		t.Fatalf("unexpected system message: %+v", msgs[0])
	}
	if msgs[1].Role != Human || msgs[1].Content != "hello world" {
		t.Fatalf("unexpected human message: %+v", msgs[1])
	}
}

func TestMessageTemplateRepeatedVariable(t *testing.T) {
	mt := NewMessageTemplate("{{name}} says hello to {{name}}!")
	msgs, err := mt.Format(context.Background(), map[string]any{"name": "Alice"})
	if err != nil {
		t.Fatal(err)
	}
	expected := "Alice says hello to Alice!"
	if msgs[0].Content != expected {
		t.Fatalf("expected %q, got %q", expected, msgs[0].Content)
	}
}

func TestChatTemplateComponentType(t *testing.T) {
	mt := NewMessageTemplate("test")
	comp := NewChatTemplateComponent(mt)
	if comp.GetComponentType() != ComponentOfPrompt {
		t.Fatalf("expected ComponentOfPrompt, got %q", comp.GetComponentType())
	}
}
