package compose

import (
	"testing"
)

type testConcatType struct {
	value string
}

func TestConcatItems_Registered(t *testing.T) {
	RegisterStreamChunkConcatFunc(func(chunks []testConcatType) (testConcatType, error) {
		var result string
		for _, c := range chunks {
			result += c.value
		}
		return testConcatType{value: result}, nil
	})
	result, err := ConcatItems([]testConcatType{
		{value: "a"}, {value: "b"}, {value: "c"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if result.value != "abc" {
		t.Fatalf("expected 'abc', got %q", result.value)
	}
}

func TestConcatItems_Unregistered(t *testing.T) {
	_, err := ConcatItems([]int{1, 2, 3})
	if err != ErrConcatNotSupported {
		t.Fatalf("expected ErrConcatNotSupported, got %v", err)
	}
}

func TestConcatItems_SingleElement(t *testing.T) {
	RegisterStreamChunkConcatFunc(func(chunks []testConcatType) (testConcatType, error) {
		var result string
		for _, c := range chunks {
			result += c.value
		}
		return testConcatType{value: result}, nil
	})
	result, err := ConcatItems([]testConcatType{{value: "hello"}})
	if err != nil {
		t.Fatal(err)
	}
	if result.value != "hello" {
		t.Fatalf("expected 'hello', got %q", result.value)
	}
}

func TestConcatItems_EmptySlice(t *testing.T) {
	result, err := ConcatItems([]string{})
	if err != nil {
		t.Fatal(err)
	}
	if result != "" {
		t.Fatalf("expected empty string, got %q", result)
	}
}

func TestConcatMessages_TextOnly(t *testing.T) {
	chunks := []*Message{
		{Content: "Hello"},
		{Content: " World"},
	}
	result, err := ConcatMessages(chunks)
	if err != nil {
		t.Fatal(err)
	}
	if result.Content != "Hello World" {
		t.Fatalf("expected 'Hello World', got %q", result.Content)
	}
}

func TestConcatMessages_ReasoningContent(t *testing.T) {
	chunks := []*Message{
		{ReasoningContent: "I think"},
		{ReasoningContent: " therefore..."},
	}
	result, err := ConcatMessages(chunks)
	if err != nil {
		t.Fatal(err)
	}
	if result.ReasoningContent != "I think therefore..." {
		t.Fatalf("expected 'I think therefore...', got %q", result.ReasoningContent)
	}
}

func TestConcatMessages_ToolCalls(t *testing.T) {
	idx0 := 0
	chunks := []*Message{
		{
			ToolCalls: []ToolCall{
				{Index: &idx0, ID: "call_1", Type: "function", Function: ToolCallFunction{Name: "get_weather", Arguments: `{"loc`}},
			},
		},
		{
			ToolCalls: []ToolCall{
				{Index: &idx0, ID: "call_1", Type: "function", Function: ToolCallFunction{Name: "get_weather", Arguments: `ation":"NYC"}`}},
			},
		},
	}
	result, err := ConcatMessages(chunks)
	if err != nil {
		t.Fatal(err)
	}
	if len(result.ToolCalls) != 1 {
		t.Fatalf("expected 1 merged ToolCall, got %d", len(result.ToolCalls))
	}
	tc := result.ToolCalls[0]
	if tc.Function.Arguments != `{"location":"NYC"}` {
		t.Fatalf("expected merged arguments, got %q", tc.Function.Arguments)
	}
	if tc.Index == nil || *tc.Index != 0 {
		t.Fatalf("expected Index=0, got %v", tc.Index)
	}
}

func TestConcatMessages_ToolCallIndexConflict(t *testing.T) {
	idx0 := 0
	chunks := []*Message{
		{
			ToolCalls: []ToolCall{
				{Index: &idx0, ID: "call_1", Type: "function", Function: ToolCallFunction{Name: "get_weather", Arguments: "a"}},
			},
		},
		{
			ToolCalls: []ToolCall{
				{Index: &idx0, ID: "call_2", Type: "function", Function: ToolCallFunction{Name: "get_weather", Arguments: "b"}},
			},
		},
	}
	_, err := ConcatMessages(chunks)
	if err == nil {
		t.Fatal("expected error for ToolCall ID mismatch")
	}
}

func TestConcatMessages_ToolCallOrdering(t *testing.T) {
	idx2 := 2
	idx0 := 0
	idx1 := 1
	chunks := []*Message{
		{
			ToolCalls: []ToolCall{
				{Index: &idx2, ID: "c3", Type: "function", Function: ToolCallFunction{Name: "f3", Arguments: "c3"}},
			},
		},
		{
			ToolCalls: []ToolCall{
				{Index: &idx0, ID: "c1", Type: "function", Function: ToolCallFunction{Name: "f1", Arguments: "c1"}},
			},
		},
	}
	chunks2 := []*Message{
		{
			ToolCalls: []ToolCall{
				{Index: &idx1, ID: "c2", Type: "function", Function: ToolCallFunction{Name: "f2", Arguments: "c2"}},
			},
		},
	}
	allChunks := append(chunks, chunks2...)
	result, err := ConcatMessages(allChunks)
	if err != nil {
		t.Fatal(err)
	}
	if len(result.ToolCalls) != 3 {
		t.Fatalf("expected 3 ToolCalls, got %d", len(result.ToolCalls))
	}
	if result.ToolCalls[0].ID != "c1" {
		t.Fatalf("expected first ToolCall ID 'c1', got %q", result.ToolCalls[0].ID)
	}
	if result.ToolCalls[1].ID != "c2" {
		t.Fatalf("expected second ToolCall ID 'c2', got %q", result.ToolCalls[1].ID)
	}
	if result.ToolCalls[2].ID != "c3" {
		t.Fatalf("expected third ToolCall ID 'c3', got %q", result.ToolCalls[2].ID)
	}
}

func TestConcatMessages_UnindexedToolCalls(t *testing.T) {
	chunks := []*Message{
		{
			ToolCalls: []ToolCall{
				{ID: "call_a", Type: "function", Function: ToolCallFunction{Name: "fn_a", Arguments: "a"}},
			},
		},
		{
			ToolCalls: []ToolCall{
				{ID: "call_b", Type: "function", Function: ToolCallFunction{Name: "fn_b", Arguments: "b"}},
			},
		},
	}
	result, err := ConcatMessages(chunks)
	if err != nil {
		t.Fatal(err)
	}
	if len(result.ToolCalls) != 2 {
		t.Fatalf("expected 2 unindexed ToolCalls, got %d", len(result.ToolCalls))
	}
	if result.ToolCalls[0].ID != "call_a" {
		t.Fatalf("expected call_a, got %q", result.ToolCalls[0].ID)
	}
	if result.ToolCalls[1].ID != "call_b" {
		t.Fatalf("expected call_b, got %q", result.ToolCalls[1].ID)
	}
}

func TestConcatMessages_ResponseMeta(t *testing.T) {
	meta := &ResponseMeta{ID: "resp-2", Model: "gpt-4"}
	chunks := []*Message{
		{Content: "first"},
		{Content: "second", ResponseMeta: meta},
	}
	result, err := ConcatMessages(chunks)
	if err != nil {
		t.Fatal(err)
	}
	if result.ResponseMeta == nil {
		t.Fatal("expected ResponseMeta, got nil")
	}
	if result.ResponseMeta.ID != "resp-2" {
		t.Fatalf("expected resp-2, got %q", result.ResponseMeta.ID)
	}
}

func TestConcatMessages_Role(t *testing.T) {
	chunks := []*Message{
		{Role: Assistant, Content: "I am"},
		{Role: "", Content: " a bot"},
	}
	result, err := ConcatMessages(chunks)
	if err != nil {
		t.Fatal(err)
	}
	if result.Role != Assistant {
		t.Fatalf("expected Assistant role, got %q", result.Role)
	}
}

func TestConcatMessageArray(t *testing.T) {
	chunks := []*Message{
		{Content: "Hello"},
		{Content: " World"},
	}
	result, err := ConcatMessageArray(chunks)
	if err != nil {
		t.Fatal(err)
	}
	if result.Content != "Hello World" {
		t.Fatalf("expected 'Hello World', got %q", result.Content)
	}
}

func TestConcatMessages_NilChunk(t *testing.T) {
	chunks := []*Message{
		{Content: "Hello"},
		nil,
		{Content: " World"},
	}
	result, err := ConcatMessages(chunks)
	if err != nil {
		t.Fatal(err)
	}
	if result.Content != "Hello World" {
		t.Fatalf("expected 'Hello World', got %q", result.Content)
	}
}

func TestConcatMessages_AllNil(t *testing.T) {
	result, err := ConcatMessages(nil)
	if err != nil {
		t.Fatal(err)
	}
	if result != nil {
		t.Fatalf("expected nil, got %v", result)
	}
}

func TestConcatMessages_MultiProviderMeta(t *testing.T) {
	openAIExt := &OpenAIRespMetaExtension{ID: "openai-resp-id", Status: "completed"}
	claudeExt := &ClaudeRespMetaExtension{ID: "claude-resp-id", StopReason: "end_turn"}
	geminiExt := &GeminiRespMetaExtension{ID: "gemini-resp-id", FinishReason: "STOP"}

	chunks := []*Message{
		{
			Content: "first",
			ResponseMeta: &ResponseMeta{
				ID:              "resp-1",
				OpenAIExtension: openAIExt,
			},
		},
		{
			Content: "second",
			ResponseMeta: &ResponseMeta{
				ID:              "resp-2",
				ClaudeExtension: claudeExt,
				GeminiExtension: geminiExt,
			},
		},
	}
	result, err := ConcatMessages(chunks)
	if err != nil {
		t.Fatal(err)
	}
	if result.ResponseMeta == nil {
		t.Fatal("expected ResponseMeta, got nil")
	}
	if result.ResponseMeta.ID != "resp-2" {
		t.Fatalf("expected last ResponseMeta ID 'resp-2', got %q", result.ResponseMeta.ID)
	}
	if result.ResponseMeta.OpenAIExtension != nil {
		t.Fatal("expected nil OpenAIExtension since last ResponseMeta wins")
	}
	if result.ResponseMeta.ClaudeExtension == nil {
		t.Fatal("expected ClaudeExtension from last chunk")
	}
	if result.ResponseMeta.ClaudeExtension.ID != "claude-resp-id" {
		t.Fatalf("expected claude-resp-id, got %q", result.ResponseMeta.ClaudeExtension.ID)
	}
	if result.ResponseMeta.GeminiExtension == nil {
		t.Fatal("expected GeminiExtension from last chunk")
	}
}

func TestConcatToolResults(t *testing.T) {
	results := []*ToolResult{
		{Text: "result1 "},
		{Text: "result2"},
	}
	merged, err := ConcatToolResults(results)
	if err != nil {
		t.Fatal(err)
	}
	if merged.Text != "result1 result2" {
		t.Fatalf("expected 'result1 result2', got %q", merged.Text)
	}
}

func TestConcatToolResultsEmpty(t *testing.T) {
	merged, err := ConcatToolResults(nil)
	if err != nil {
		t.Fatal(err)
	}
	if merged != nil {
		t.Fatalf("expected nil, got %v", merged)
	}
}

func TestConcatToolResults_MultiModal(t *testing.T) {
	img1 := &ImageContent{URL: "https://example.com/a.png", Format: "png"}
	img2 := &ImageContent{URL: "https://example.com/b.png", Format: "png"}
	aud := &AudioContent{URL: "https://example.com/sound.mp3", Format: "mp3"}
	vid := &VideoContent{URL: "https://example.com/video.mp4", Format: "mp4"}
	f := &FileContent{Name: "doc.txt", Type: "text/plain"}

	results := []*ToolResult{
		{Text: "part1", Images: []*ImageContent{img1}, Audio: []*AudioContent{aud}},
		{Text: "part2", Images: []*ImageContent{img2}, Video: []*VideoContent{vid}, Files: []*FileContent{f}},
	}
	merged, err := ConcatToolResults(results)
	if err != nil {
		t.Fatal(err)
	}
	if merged.Text != "part1part2" {
		t.Fatalf("expected 'part1part2', got %q", merged.Text)
	}
	if len(merged.Images) != 2 {
		t.Fatalf("expected 2 images, got %d", len(merged.Images))
	}
	if len(merged.Audio) != 1 {
		t.Fatalf("expected 1 audio, got %d", len(merged.Audio))
	}
	if len(merged.Video) != 1 {
		t.Fatalf("expected 1 video, got %d", len(merged.Video))
	}
	if len(merged.Files) != 1 {
		t.Fatalf("expected 1 file, got %d", len(merged.Files))
	}
}

func TestConcatToolResults_NilChunk(t *testing.T) {
	results := []*ToolResult{
		{Text: "hello"},
		nil,
		{Text: " world"},
	}
	merged, err := ConcatToolResults(results)
	if err != nil {
		t.Fatal(err)
	}
	if merged.Text != "hello world" {
		t.Fatalf("expected 'hello world', got %q", merged.Text)
	}
}

func TestEndToEnd_StreamConcat(t *testing.T) {
	chunks := []*Message{
		{Role: Assistant, Content: "Hello"},
		{Role: Assistant, Content: " World"},
		{Role: Assistant, Content: "!"},
	}
	result, err := ConcatMessages(chunks)
	if err != nil {
		t.Fatal(err)
	}
	if result.Content != "Hello World!" {
		t.Fatalf("expected 'Hello World!', got %q", result.Content)
	}
	if result.Role != Assistant {
		t.Fatalf("expected Assistant role, got %q", result.Role)
	}
}

func TestEndToEnd_ToolCallStream(t *testing.T) {
	idx0 := 0
	idx1 := 1
	chunks := []*Message{
		{
			Role: Assistant,
			ToolCalls: []ToolCall{
				{Index: &idx0, ID: "tc1", Type: "function", Function: ToolCallFunction{Name: "search", Arguments: `{"q":`}},
				{Index: &idx1, ID: "tc2", Type: "function", Function: ToolCallFunction{Name: "calc", Arguments: `{"expr":`}},
			},
		},
		{
			Role: Assistant,
			ToolCalls: []ToolCall{
				{Index: &idx0, ID: "tc1", Type: "function", Function: ToolCallFunction{Name: "search", Arguments: `"golang"}`}},
				{Index: &idx1, ID: "tc2", Type: "function", Function: ToolCallFunction{Name: "calc", Arguments: `"2+2"}`}},
			},
		},
	}
	result, err := ConcatMessages(chunks)
	if err != nil {
		t.Fatal(err)
	}
	if len(result.ToolCalls) != 2 {
		t.Fatalf("expected 2 merged ToolCalls, got %d: %v", len(result.ToolCalls), result.ToolCalls)
	}
	tc0 := result.ToolCalls[0]
	if tc0.ID != "tc1" {
		t.Fatalf("expected first tc ID 'tc1', got %q", tc0.ID)
	}
	if tc0.Function.Arguments != `{"q":"golang"}` {
		t.Fatalf("expected merged search args, got %q", tc0.Function.Arguments)
	}
	tc1 := result.ToolCalls[1]
	if tc1.ID != "tc2" {
		t.Fatalf("expected second tc ID 'tc2', got %q", tc1.ID)
	}
	if tc1.Function.Arguments != `{"expr":"2+2"}` {
		t.Fatalf("expected merged calc args, got %q", tc1.Function.Arguments)
	}
}

func TestConcatMessages_ExtraField(t *testing.T) {
	chunks := []*Message{
		{Content: "a", Extra: map[string]any{"key1": "val1"}},
		{Content: "b", Extra: map[string]any{"key2": "val2"}},
	}
	result, err := ConcatMessages(chunks)
	if err != nil {
		t.Fatal(err)
	}
	if result.Extra == nil {
		t.Fatal("expected Extra map, got nil")
	}
	if result.Extra["key1"] != "val1" {
		t.Fatalf("expected key1=val1, got %v", result.Extra["key1"])
	}
	if result.Extra["key2"] != "val2" {
		t.Fatalf("expected key2=val2, got %v", result.Extra["key2"])
	}
}

func TestConcatMessages_StreamPipeIntegration(t *testing.T) {
	sr1 := PipeStreamReaderFromSlice([]*Message{
		{Role: Assistant, Content: "Hello"},
	})
	sr2 := PipeStreamReaderFromSlice([]*Message{
		{Role: Assistant, Content: " World"},
	})

	result := Concat(sr1, sr2)

	v, ok := result.Recv()
	if !ok {
		t.Fatal("expected value from concat pipe")
	}
	if v.Content != "Hello World" {
		t.Fatalf("expected 'Hello World', got %q", v.Content)
	}
	_, ok = result.Recv()
	if ok {
		t.Fatal("expected false after concat pipe drain")
	}
}
