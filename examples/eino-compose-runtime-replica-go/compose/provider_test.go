package compose

import (
	"context"
	"encoding/json"
	"testing"
)

func TestNewTextContentBlock(t *testing.T) {
	b := NewTextContentBlock("hello")
	if b.Type != ContentBlockTypeUserInputText {
		t.Fatalf("expected ContentBlockTypeUserInputText, got %q", b.Type)
	}
	if b.UserInputText == nil || *b.UserInputText != "hello" {
		t.Fatal("unexpected UserInputText")
	}
}

func TestNewAssistantTextContentBlock(t *testing.T) {
	b := NewAssistantTextContentBlock("hi")
	if b.Type != ContentBlockTypeAssistantGenText {
		t.Fatalf("expected ContentBlockTypeAssistantGenText, got %q", b.Type)
	}
	if b.AssistantGenText == nil || b.AssistantGenText.Content != "hi" {
		t.Fatal("unexpected AssistantGenText")
	}
}

func TestNewToolCallContentBlock(t *testing.T) {
	b := NewToolCallContentBlock("call_1", "search", `{"q":"test"}`)
	if b.Type != ContentBlockTypeFunctionToolCall {
		t.Fatalf("expected ContentBlockTypeFunctionToolCall, got %q", b.Type)
	}
	if b.FunctionToolCall == nil || b.FunctionToolCall.Name != "search" {
		t.Fatal("unexpected FunctionToolCall")
	}
}

func TestNewToolResultContentBlock(t *testing.T) {
	b := NewToolResultContentBlock("call_1", "done")
	if b.Type != ContentBlockTypeFunctionToolResult {
		t.Fatalf("expected ContentBlockTypeFunctionToolResult, got %q", b.Type)
	}
	if b.ToolResult == nil || b.ToolResult.Output != "done" {
		t.Fatal("unexpected ToolResult")
	}
}

func TestNewImageContentBlock(t *testing.T) {
	b := NewImageContentBlock("https://example.com/img.png")
	if b.Type != ContentBlockTypeUserInputImage {
		t.Fatalf("expected ContentBlockTypeUserInputImage, got %q", b.Type)
	}
	if b.UserInputImage == nil || b.UserInputImage.URL != "https://example.com/img.png" {
		t.Fatal("unexpected UserInputImage")
	}
}

func TestAgenticMessageFirstText(t *testing.T) {
	am := &AgenticMessage{Role: AgenticRoleUser, ContentBlocks: []*ContentBlock{
		NewTextContentBlock("Hello"), NewTextContentBlock("World"),
	}}
	if AgenticMessageFirstText(am) != "Hello" {
		t.Fatalf("expected 'Hello', got %q", AgenticMessageFirstText(am))
	}
}

func TestAgenticMessageToolCalls(t *testing.T) {
	am := &AgenticMessage{Role: AgenticRoleAssistant, ContentBlocks: []*ContentBlock{
		NewAssistantTextContentBlock("hi"),
		NewToolCallContentBlock("c1", "search", `{}`),
		NewToolCallContentBlock("c2", "calc", `{}`),
	}}
	calls := AgenticMessageToolCalls(am)
	if len(calls) != 2 || calls[0].Name != "search" || calls[1].Name != "calc" {
		t.Fatalf("unexpected tool calls: %v", calls)
	}
}

func TestOpenAIToCanonicalMessages(t *testing.T) {
	req := &OpenAIChatRequest{Model: "gpt-4", Messages: []*OpenAIMessage{
		{Role: "system", Content: "You are helpful."},
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi!"},
	}}
	msgs := ToCanonicalMessages(req)
	if len(msgs) != 3 || msgs[0].Role != System || msgs[1].Role != User || msgs[2].Role != Assistant {
		t.Fatalf("unexpected conversion result")
	}
}

func TestOpenAIToCanonicalMessagesWithToolCalls(t *testing.T) {
	req := &OpenAIChatRequest{Model: "gpt-4", Messages: []*OpenAIMessage{
		{Role: "user", Content: "Weather?"},
		{Role: "assistant", ToolCalls: []ToolCall{{ID: "c1", Type: "function", Function: ToolCallFunction{Name: "get_weather", Arguments: `{"city":"Paris"}`}}}},
		{Role: "tool", Content: "Sunny", ToolCallID: "c1"},
	}}
	msgs := ToCanonicalMessages(req)
	if len(msgs) != 3 || len(msgs[1].ToolCalls) != 1 || msgs[1].ToolCalls[0].Function.Name != "get_weather" {
		t.Fatalf("unexpected tool call conversion")
	}
	if msgs[2].Role != Tool || msgs[2].ToolCallID != "c1" {
		t.Fatalf("unexpected tool message")
	}
}

func TestOpenAIFromCanonicalMessages(t *testing.T) {
	msgs := []*Message{
		{Role: System, Content: "You are helpful."},
		{Role: User, Content: "Hello"},
	}
	req := FromCanonicalMessages(msgs, "gpt-4")
	if req.Model != "gpt-4" || len(req.Messages) != 2 {
		t.Fatalf("unexpected conversion")
	}
}

func TestOpenAIRoundTrip(t *testing.T) {
	original := &OpenAIChatRequest{Model: "gpt-4", Messages: []*OpenAIMessage{
		{Role: "system", Content: "Be helpful."},
		{Role: "user", Content: "Hi"},
		{Role: "assistant", ToolCalls: []ToolCall{{ID: "c1", Type: "function", Function: ToolCallFunction{Name: "search", Arguments: `{"q":"test"}`}}}},
		{Role: "tool", Content: "result", ToolCallID: "c1"},
	}}
	canonical := ToCanonicalMessages(original)
	rt := FromCanonicalMessages(canonical, "gpt-4")
	if len(rt.Messages) != 4 || rt.Messages[3].ToolCallID != "c1" {
		t.Fatalf("round trip failed")
	}
}

func TestOpenAIToCanonicalMessagesNil(t *testing.T) {
	if ToCanonicalMessages(nil) != nil {
		t.Fatal("expected nil")
	}
}

func TestFakeOpenAIProvider(t *testing.T) {
	p := &FakeOpenAIProvider{}
	if p.Name() != "openai" {
		t.Fatalf("expected 'openai', got %q", p.Name())
	}
	req := &OpenAIChatRequest{Model: "gpt-4", Messages: []*OpenAIMessage{{Role: "user", Content: "Hello"}}}
	msgs, err := p.ToCanonicalMessages(req)
	if err != nil || len(msgs) != 1 {
		t.Fatal("unexpected error")
	}
	_, err = p.ToCanonicalMessages(nil)
	if err == nil {
		t.Fatal("expected error for nil")
	}
	_, err = p.FromCanonicalMessages(nil)
	if err == nil {
		t.Fatal("expected error for nil")
	}
}

func TestClaudeToCanonicalAgenticMessages(t *testing.T) {
	req := &ClaudeChatRequest{Model: "claude-3", Messages: []*ClaudeMessage{
		{Role: "user", Content: []*ClaudeContentBlock{{Type: "text", Text: "Hello"}}},
		{Role: "assistant", Content: []*ClaudeContentBlock{{Type: "text", Text: "Hi!"}}},
	}}
	msgs := ToCanonicalAgenticMessages(req)
	if len(msgs) != 2 || msgs[0].Role != AgenticRoleUser || msgs[1].Role != AgenticRoleAssistant {
		t.Fatalf("unexpected conversion")
	}
}

func TestClaudeToolUseConversion(t *testing.T) {
	req := &ClaudeChatRequest{Model: "claude-3", Messages: []*ClaudeMessage{
		{Role: "assistant", Content: []*ClaudeContentBlock{
			{Type: "text", Text: "Checking..."},
			{Type: "tool_use", ID: "toolu_01", Name: "get_weather", Input: map[string]string{"city": "Paris"}},
		}},
		{Role: "user", Content: []*ClaudeContentBlock{
			{Type: "tool_result", ToolUseID: "toolu_01", Content: "Sunny"},
		}},
	}}
	msgs := ToCanonicalAgenticMessages(req)
	if len(msgs) != 2 || len(msgs[0].ContentBlocks) != 2 {
		t.Fatalf("unexpected block count")
	}
	if msgs[0].ContentBlocks[1].Type != ContentBlockTypeFunctionToolCall {
		t.Fatalf("expected function_tool_call block")
	}
	if msgs[0].ContentBlocks[1].FunctionToolCall.CallID != "toolu_01" {
		t.Fatalf("expected toolu_01")
	}
	if msgs[1].ContentBlocks[0].Type != ContentBlockTypeFunctionToolResult {
		t.Fatalf("expected function_tool_result block")
	}
}

func TestClaudeFromCanonicalAgenticMessages(t *testing.T) {
	msgs := []*AgenticMessage{
		{Role: AgenticRoleUser, ContentBlocks: []*ContentBlock{NewTextContentBlock("Hi")}},
		{Role: AgenticRoleAssistant, ContentBlocks: []*ContentBlock{NewAssistantTextContentBlock("Hello!")}},
	}
	req := FromCanonicalAgenticMessages(msgs, "claude-3")
	if req.Model != "claude-3" || len(req.Messages) != 2 {
		t.Fatalf("unexpected conversion")
	}
}

func TestClaudeRoundTrip(t *testing.T) {
	original := &ClaudeChatRequest{Model: "claude-3", Messages: []*ClaudeMessage{
		{Role: "user", Content: []*ClaudeContentBlock{{Type: "text", Text: "Weather?"}}},
		{Role: "assistant", Content: []*ClaudeContentBlock{
			{Type: "text", Text: "Checking..."},
			{Type: "tool_use", ID: "toolu_01", Name: "get_weather", Input: map[string]string{"city": "Paris"}},
		}},
	}}
	rt := FromCanonicalAgenticMessages(ToCanonicalAgenticMessages(original), "claude-3")
	if len(rt.Messages) != 2 || rt.Messages[1].Content[1].Type != "tool_use" {
		t.Fatalf("round trip failed")
	}
}

func TestClaudeNilInput(t *testing.T) {
	if ToCanonicalAgenticMessages(nil) != nil {
		t.Fatal("expected nil")
	}
}

func TestFakeClaudeProvider(t *testing.T) {
	p := &FakeClaudeProvider{}
	if p.Name() != "claude" {
		t.Fatalf("expected 'claude'")
	}
	req := &ClaudeChatRequest{Messages: []*ClaudeMessage{{Role: "user", Content: []*ClaudeContentBlock{{Type: "text", Text: "Hi"}}}}}
	msgs, err := p.ToCanonicalAgenticMessages(req)
	if err != nil || len(msgs) != 1 {
		t.Fatal("unexpected error")
	}
	_, err = p.ToCanonicalAgenticMessages(nil)
	if err == nil {
		t.Fatal("expected nil error")
	}
	_, err = p.FromCanonicalAgenticMessages(nil)
	if err == nil {
		t.Fatal("expected nil error")
	}
}

func TestGeminiToCanonicalAgenticMessages(t *testing.T) {
	req := &GeminiChatRequest{Contents: []*GeminiContent{
		{Role: "user", Parts: []*GeminiPart{{Text: "Hello"}}},
		{Role: "model", Parts: []*GeminiPart{{Text: "Hi!"}}},
	}}
	msgs := ToCanonicalAgenticMessagesFromGemini(req)
	if len(msgs) != 2 || msgs[0].Role != AgenticRoleUser || msgs[1].Role != AgenticRoleAssistant {
		t.Fatalf("unexpected conversion")
	}
}

func TestGeminiFunctionCallConversion(t *testing.T) {
	req := &GeminiChatRequest{Contents: []*GeminiContent{
		{Role: "user", Parts: []*GeminiPart{{Text: "Weather?"}}},
		{Role: "model", Parts: []*GeminiPart{
			{Text: "Checking..."},
			{FunctionCall: &GeminiFunctionCall{Name: "get_weather", Args: map[string]any{"city": "Paris"}}},
		}},
		{Role: "function", Parts: []*GeminiPart{
			{FunctionResponse: &GeminiFunctionResponse{Name: "get_weather", Response: map[string]any{"temp": 22}}},
		}},
	}}
	msgs := ToCanonicalAgenticMessagesFromGemini(req)
	if len(msgs) != 3 || len(msgs[1].ContentBlocks) != 2 {
		t.Fatalf("unexpected message count")
	}
	if msgs[1].ContentBlocks[1].Type != ContentBlockTypeFunctionToolCall {
		t.Fatalf("expected function_tool_call")
	}
	if msgs[1].ContentBlocks[1].FunctionToolCall.Name != "get_weather" {
		t.Fatalf("expected 'get_weather'")
	}
	if msgs[2].ContentBlocks[0].Type != ContentBlockTypeFunctionToolResult {
		t.Fatalf("expected function_tool_result")
	}
}

func TestGeminiAgenticRoundTrip(t *testing.T) {
	original := &GeminiChatRequest{Contents: []*GeminiContent{
		{Role: "user", Parts: []*GeminiPart{{Text: "Weather?"}}},
		{Role: "model", Parts: []*GeminiPart{
			{FunctionCall: &GeminiFunctionCall{Name: "get_weather", Args: map[string]any{"city": "Paris"}}},
		}},
	}}
	rt := FromCanonicalAgenticMessagesToGemini(ToCanonicalAgenticMessagesFromGemini(original))
	if len(rt.Contents) != 2 || rt.Contents[1].Role != "model" {
		t.Fatalf("round trip failed")
	}
}

func TestGeminiToCanonicalMessages(t *testing.T) {
	req := &GeminiChatRequest{Contents: []*GeminiContent{
		{Role: "user", Parts: []*GeminiPart{{Text: "Hello"}}},
		{Role: "model", Parts: []*GeminiPart{{Text: "Hi!"}}},
	}}
	msgs := ToCanonicalMessagesFromGemini(req)
	if len(msgs) != 2 || msgs[0].Role != User || msgs[1].Role != Assistant {
		t.Fatalf("unexpected conversion")
	}
}

func TestGeminiToCanonicalMessagesWithFunctionCall(t *testing.T) {
	req := &GeminiChatRequest{Contents: []*GeminiContent{
		{Role: "user", Parts: []*GeminiPart{{Text: "Weather?"}}},
		{Role: "model", Parts: []*GeminiPart{
			{FunctionCall: &GeminiFunctionCall{Name: "get_weather", Args: map[string]any{"city": "Paris"}}},
		}},
		{Role: "function", Parts: []*GeminiPart{
			{FunctionResponse: &GeminiFunctionResponse{Name: "get_weather", Response: map[string]any{"temp": 22}}},
		}},
	}}
	msgs := ToCanonicalMessagesFromGemini(req)
	if len(msgs) != 3 || len(msgs[1].ToolCalls) != 1 || msgs[1].ToolCalls[0].Function.Name != "get_weather" {
		t.Fatalf("unexpected tool call conversion")
	}
	if msgs[2].Role != Tool {
		t.Fatalf("expected Tool role")
	}
}

func TestGeminiMessageRoundTrip(t *testing.T) {
	original := &GeminiChatRequest{Contents: []*GeminiContent{
		{Role: "user", Parts: []*GeminiPart{{Text: "Hi"}}},
		{Role: "model", Parts: []*GeminiPart{
			{FunctionCall: &GeminiFunctionCall{Name: "search", Args: map[string]any{"q": "test"}}},
		}},
	}}
	rt := FromCanonicalMessagesToGemini(ToCanonicalMessagesFromGemini(original))
	if len(rt.Contents) != 2 || rt.Contents[1].Role != "model" {
		t.Fatalf("round trip failed")
	}
}

func TestGeminiNilInput(t *testing.T) {
	if ToCanonicalAgenticMessagesFromGemini(nil) != nil {
		t.Fatal("expected nil")
	}
	if ToCanonicalMessagesFromGemini(nil) != nil {
		t.Fatal("expected nil")
	}
}

func TestFakeGeminiProvider(t *testing.T) {
	p := &FakeGeminiProvider{}
	if p.Name() != "gemini" {
		t.Fatalf("expected 'gemini'")
	}
	req := &GeminiChatRequest{Contents: []*GeminiContent{{Role: "user", Parts: []*GeminiPart{{Text: "Hello"}}}}}
	ams, err := p.ToCanonicalAgenticMessages(req)
	if err != nil || len(ams) != 1 {
		t.Fatal("agentic path failed")
	}
	ms, err := p.ToCanonicalMessages(req)
	if err != nil || len(ms) != 1 || ms[0].Content != "Hello" {
		t.Fatal("message path failed")
	}
	_, err = p.ToCanonicalAgenticMessages(nil)
	if err == nil {
		t.Fatal("expected nil error")
	}
	_, err = p.ToCanonicalMessages(nil)
	if err == nil {
		t.Fatal("expected nil error")
	}
	_, err = p.FromCanonicalAgenticMessages(nil)
	if err == nil {
		t.Fatal("expected nil error")
	}
	_, err = p.FromCanonicalMessages(nil)
	if err == nil {
		t.Fatal("expected nil error")
	}
}

func TestCanonicalMessageFromOpenAIChatModel(t *testing.T) {
	req := &OpenAIChatRequest{Model: "gpt-4", Messages: []*OpenAIMessage{
		{Role: "system", Content: "Be helpful."},
		{Role: "user", Content: "What is Rive?"},
	}}
	cm := NewFakeChatModel(WithChatGenerateFunc(func(ctx context.Context, input []*Message) (*Message, error) {
		return AssistantMessage("Rive is an agent team runtime."), nil
	}))
	resp, err := cm.Generate(context.Background(), ToCanonicalMessages(req))
	if err != nil || resp.Content != "Rive is an agent team runtime." {
		t.Fatalf("cross-provider chat model failed: %v", err)
	}
}

func TestCanonicalAgenticMessageFromClaudeWithTool(t *testing.T) {
	req := &ClaudeChatRequest{Messages: []*ClaudeMessage{
		{Role: "user", Content: []*ClaudeContentBlock{{Type: "text", Text: "Weather?"}}},
		{Role: "assistant", Content: []*ClaudeContentBlock{
			{Type: "text", Text: "Checking..."},
			{Type: "tool_use", ID: "toolu_01", Name: "get_weather", Input: map[string]string{"city": "Paris"}},
		}},
	}}
	ams := ToCanonicalAgenticMessages(req)
	calls := AgenticMessageToolCalls(ams[1])
	if len(calls) != 1 || calls[0].Name != "get_weather" {
		t.Fatalf("failed to extract tool calls")
	}
	tool := NewBridgeTool("get_weather", func(ctx context.Context, args map[string]any) (string, error) {
		return "Sunny, 22C", nil
	})
	result, err := tool.Execute(context.Background(), map[string]any{"city": "Paris"})
	if err != nil || result != "Sunny, 22C" {
		t.Fatalf("tool execution failed")
	}
}

func TestGeminiMessageWithRetriever(t *testing.T) {
	req := &GeminiChatRequest{Contents: []*GeminiContent{
		{Role: "user", Parts: []*GeminiPart{{Text: "Tell me about Rive"}}},
	}}
	msgs := ToCanonicalMessagesFromGemini(req)
	query := &Query{Text: msgs[0].Content, K: 3}
	fr := &FakeRetriever{Docs: []*Document{{Content: "Rive docs"}}}
	docs, err := fr.Retrieve(context.Background(), query)
	if err != nil || len(docs) != 1 {
		t.Fatalf("retriever failed")
	}
}

func TestGeminiFullPipeline(t *testing.T) {
	req := &GeminiChatRequest{Contents: []*GeminiContent{
		{Role: "user", Parts: []*GeminiPart{{Text: "Weather?"}}},
		{Role: "model", Parts: []*GeminiPart{
			{FunctionCall: &GeminiFunctionCall{Name: "get_weather", Args: map[string]any{"city": "Tokyo"}}},
		}},
	}}
	ams := ToCanonicalAgenticMessagesFromGemini(req)
	calls := AgenticMessageToolCalls(ams[1])
	tool := NewBridgeTool("get_weather", func(ctx context.Context, args map[string]any) (string, error) {
		return "Cloudy, 18C", nil
	})
	var execArgs map[string]any
	_ = json.Unmarshal([]byte(calls[0].Arguments), &execArgs)
	result, _ := tool.Execute(context.Background(), execArgs)
	response := NewToolResultContentBlock("get_weather", result)
	allMsgs := append(ams, &AgenticMessage{Role: AgenticRoleUser, ContentBlocks: []*ContentBlock{response}})
	rt := FromCanonicalAgenticMessagesToGemini(allMsgs)
	if len(rt.Contents) != 3 || rt.Contents[2].Role != "user" {
		t.Fatalf("full pipeline failed")
	}
}

func TestOpenAIToCanonicalMessagesEmpty(t *testing.T) {
	req := &OpenAIChatRequest{Model: "gpt-4"}
	if len(ToCanonicalMessages(req)) != 0 {
		t.Fatal("expected empty")
	}
}

func TestClaudeToCanonicalMessagesEmpty(t *testing.T) {
	if len(ToCanonicalAgenticMessages(&ClaudeChatRequest{})) != 0 {
		t.Fatal("expected empty")
	}
}

func TestGeminiToCanonicalMessagesEmpty(t *testing.T) {
	if len(ToCanonicalAgenticMessagesFromGemini(&GeminiChatRequest{})) != 0 {
		t.Fatal("expected empty")
	}
}

func TestOpenAIRoleRoundTrip(t *testing.T) {
	for _, r := range []string{"system", "user", "assistant", "tool"} {
		if canonicalRoleToOpenAI(openAIRoleToCanonical(r)) != r {
			t.Fatalf("role round trip failed for %q", r)
		}
	}
}

func TestGeminiMessageRoleRoundTrip(t *testing.T) {
	for _, pair := range []struct{ gemini, canon string }{
		{"user", "user"}, {"model", "assistant"}, {"function", "tool"},
	} {
		mapped := string(messageRoleToGemini(geminiRoleToMessage(pair.gemini)))
		if mapped != pair.gemini {
			t.Fatalf("gemini role round trip failed: %q -> %q", pair.gemini, mapped)
		}
	}
}

func TestProviderInterfaces(t *testing.T) {
	var oai ProviderOpenAI = &FakeOpenAIProvider{}
	if oai.Name() != "openai" {
		t.Fatal("OpenAI interface")
	}
	var claude ProviderClaude = &FakeClaudeProvider{}
	if claude.Name() != "claude" {
		t.Fatal("Claude interface")
	}
	var gemini ProviderGemini = &FakeGeminiProvider{}
	if gemini.Name() != "gemini" {
		t.Fatal("Gemini interface")
	}
}
