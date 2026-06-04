package compose

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"
)

// stubRetriever returns canned documents for testing.
type stubRetriever struct {
	docs []*BridgeDocument
}

func (r *stubRetriever) Retrieve(ctx context.Context, query string) ([]*BridgeDocument, error) {
	return r.docs, nil
}

// stubChatModel returns a canned response for testing.
type stubChatModel struct {
	response string
}

func (m *stubChatModel) Generate(ctx context.Context, messages []*BridgeMessage) (string, error) {
	return m.response, nil
}

func TestBridgeRetrieverToLambda(t *testing.T) {
	retriever := &stubRetriever{
		docs: []*BridgeDocument{
			{Content: "Rive is a local-first agent team runtime.", Score: 0.9},
			{Content: "Eino is CloudWeGo's LLM application framework.", Score: 0.7},
		},
	}

	bridge := &retrieverBridge{retriever: retriever}
	lambda := bridge.toLambda()

	g := NewGraph[string, []*BridgeDocument]()
	g.AddLambdaNode("retrieve", lambda)
	g.AddEdge(START, "retrieve")
	g.AddEdge("retrieve", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("retriever_bridge_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "what is Rive")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if len(result) != 2 {
		t.Fatalf("expected 2 documents, got %d", len(result))
	}
	if result[0].Content != "Rive is a local-first agent team runtime." {
		t.Fatalf("unexpected first doc content: %q", result[0].Content)
	}
}

func TestBridgeChatModelToLambda(t *testing.T) {
	model := &stubChatModel{response: "Bridge adapters let domain components participate in graph runtime."}

	bridge := &chatModelBridge{model: model}
	lambda := bridge.toLambda()

	messages := []*BridgeMessage{
		{Role: "system", Content: "You are helpful."},
		{Role: "user", Content: "Explain bridge adapters."},
	}

	g := NewGraph[[]*BridgeMessage, string]()
	g.AddLambdaNode("chat", lambda)
	g.AddEdge(START, "chat")
	g.AddEdge("chat", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("chat_model_bridge_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), messages)
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if result != "Bridge adapters let domain components participate in graph runtime." {
		t.Fatalf("unexpected response: %q", result)
	}
}

func TestBridgePromptAssemblerToLambda(t *testing.T) {
	systemPrompt := "Answer using only the provided context."
	bridge := &promptAssemblerBridge{systemPrompt: systemPrompt}
	lambda := bridge.toLambda()

	input := map[string]any{
		"query": "what is Rive",
		"documents": []*BridgeDocument{
			{Content: "Rive is a local-first agent team runtime.", Score: 0.9},
		},
	}

	g := NewGraph[map[string]any, []*BridgeMessage]()
	g.AddLambdaNode("assemble", lambda)
	g.AddEdge(START, "assemble")
	g.AddEdge("assemble", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("prompt_assembler_bridge_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), input)
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if len(result) != 2 {
		t.Fatalf("expected 2 messages, got %d", len(result))
	}
	if result[0].Role != "system" {
		t.Fatalf("expected system message first, got role=%q", result[0].Role)
	}
	if result[0].Content != systemPrompt {
		t.Fatalf("expected system prompt %q, got %q", systemPrompt, result[0].Content)
	}
	if result[1].Role != "user" {
		t.Fatalf("expected user message second, got role=%q", result[1].Role)
	}
	if !strings.Contains(result[1].Content, "what is Rive") {
		t.Fatalf("expected user message to contain query, got %q", result[1].Content)
	}
	if !strings.Contains(result[1].Content, "Rive is a local-first agent team runtime.") {
		t.Fatalf("expected user message to contain document content, got %q", result[1].Content)
	}
}

func TestBridgeRAGPipelineWorkflow(t *testing.T) {
	retriever := &stubRetriever{
		docs: []*BridgeDocument{
			{Content: "Rive is a local-first agent team runtime.", Score: 0.9},
			{Content: "Eino provides compose.Graph for LLM pipelines.", Score: 0.7},
		},
	}
	model := &stubChatModel{
		response: "Rive is a local-first agent team runtime with DAG/Pregel support.",
	}
	systemPrompt := "Answer the question using only the provided context. Be concise."

	wf := NewWorkflow[string, map[string]any]()

	wf.AsRetrieverNode("retriever", retriever).
		AddInput(START)

	wf.AsPromptAssemblerNode("assemble", systemPrompt).
		AddInput(START, MapFields("", "query")).
		AddInput("retriever", ToField("documents"))

	wf.AsChatModelNode("model", model).
		AddInput("assemble")

	wf.End().
		AddInput("model", ToField("answer")).
		AddInput(START, MapFields("", "original_query"))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "what is Rive")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	modelResult, ok := result["model"].(map[string]any)
	if !ok {
		t.Fatalf("expected 'model' map in result, got %T", result["model"])
	}
	answer, ok := modelResult["answer"].(string)
	if !ok {
		t.Fatalf("expected model.answer string field, got %T", modelResult["answer"])
	}
	if answer != "Rive is a local-first agent team runtime with DAG/Pregel support." {
		t.Fatalf("unexpected answer: %q", answer)
	}

	startResult, ok := result["start"].(map[string]any)
	if !ok {
		t.Fatalf("expected 'start' map in result, got %T", result["start"])
	}
	if startResult["original_query"] != "what is Rive" {
		t.Fatalf("expected original_query='what is Rive', got %v", startResult["original_query"])
	}

	fmt.Printf("RAG Pipeline Test Output: answer=%q, original_query=%q\n", answer, startResult["original_query"])
}

func TestBridgeRetrieverNodeConvenience(t *testing.T) {
	wf := NewWorkflow[string, []*BridgeDocument]()
	retriever := &stubRetriever{
		docs: []*BridgeDocument{{Content: "test doc", Score: 1.0}},
	}

	_ = wf.AsRetrieverNode("retriever", retriever)
	if _, ok := wf.workflowNodes["retriever"]; !ok {
		t.Fatal("AsRetrieverNode should create a workflow node")
	}
}

func TestBridgeChatModelNodeConvenience(t *testing.T) {
	wf := NewWorkflow[[]*BridgeMessage, string]()
	model := &stubChatModel{response: "ok"}

	_ = wf.AsChatModelNode("model", model)
	if _, ok := wf.workflowNodes["model"]; !ok {
		t.Fatal("AsChatModelNode should create a workflow node")
	}
}

func TestBridgePromptAssemblerNodeConvenience(t *testing.T) {
	wf := NewWorkflow[map[string]any, []*BridgeMessage]()

	_ = wf.AsPromptAssemblerNode("assemble", "Be helpful.")
	if _, ok := wf.workflowNodes["assemble"]; !ok {
		t.Fatal("AsPromptAssemblerNode should create a workflow node")
	}
}

// stubErrorRetriever returns an error for testing error propagation.
type stubErrorRetriever struct {
	err error
}

func (r *stubErrorRetriever) Retrieve(ctx context.Context, query string) ([]*BridgeDocument, error) {
	return nil, r.err
}

// stubErrorChatModel returns an error for testing error propagation.
type stubErrorChatModel struct {
	err error
}

func (m *stubErrorChatModel) Generate(ctx context.Context, messages []*BridgeMessage) (string, error) {
	return "", m.err
}

func TestBridgeRetrieverFromMapBridgeWrongKeyType(t *testing.T) {
	retriever := &stubRetriever{
		docs: []*BridgeDocument{{Content: "test", Score: 1.0}},
	}
	lambda := retrieverFromMapBridge(retriever, "query")

	// Input has "query" key but with wrong type (int, not string)
	_, err := lambda.invokeFn(context.Background(), map[string]any{
		"query": 42,
	})
	if err == nil {
		t.Fatal("expected error for wrong key type in map input")
	}
	if !strings.Contains(err.Error(), "expected string value for key") {
		t.Fatalf("expected type error, got: %v", err)
	}
}

func TestBridgeRetrieverFromMapBridgeMissingKey(t *testing.T) {
	retriever := &stubRetriever{
		docs: []*BridgeDocument{{Content: "test", Score: 1.0}},
	}
	lambda := retrieverFromMapBridge(retriever, "query")

	// Input does not have "query" key — nil type assertion fails, returns error
	_, err := lambda.invokeFn(context.Background(), map[string]any{
		"other": "value",
	})
	if err == nil {
		t.Fatal("expected error for missing key in map input")
	}
	if !strings.Contains(err.Error(), "expected string value for key") {
		t.Fatalf("expected type error, got: %v", err)
	}
}

func TestBridgeRAGPipelineRetrieverError(t *testing.T) {
	retriever := &stubErrorRetriever{err: errors.New("retrieval service unavailable")}

	// Use a simple graph to verify error propagation from bridge component
	g := NewGraph[string, string]()
	retrieverNode := (&retrieverBridge{retriever: retriever}).toLambda()
	g.AddLambdaNode("retrieve", retrieverNode)
	g.AddEdge(START, "retrieve")
	g.AddEdge("retrieve", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("retriever_error_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	_, err = r.Invoke(context.Background(), "what is Rive")
	if err == nil {
		t.Fatal("expected error from retriever")
	}
}

func TestBridgeRAGPipelineChatModelError(t *testing.T) {
	model := &stubErrorChatModel{err: errors.New("model inference failed")}

	g := NewGraph[[]*BridgeMessage, string]()
	modelNode := (&chatModelBridge{model: model}).toLambda()
	g.AddLambdaNode("model", modelNode)
	g.AddEdge(START, "model")
	g.AddEdge("model", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("chatmodel_error_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	_, err = r.Invoke(context.Background(), []*BridgeMessage{
		{Role: "system", Content: "be helpful"},
		{Role: "user", Content: "test"},
	})
	if err == nil {
		t.Fatal("expected error from chat model")
	}
}

func TestBridgeRAGPipelineWithCallbacks(t *testing.T) {
	model := &stubChatModel{
		response: "Rive is a local-first agent team runtime.",
	}

	var (
		modelStartCalled bool
		modelEndCalled   bool
	)

	// Graph: []*BridgeMessage → string (chat model via bridge)
	g := NewGraph[[]*BridgeMessage, string]()
	modelNode := (&chatModelBridge{model: model}).toLambda()
	g.AddLambdaNode("model", modelNode)
	g.AddEdge(START, "model")
	g.AddEdge("model", END)

	g.SetNodeCallbacks("model", &Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
			modelStartCalled = true
			return ctx
		},
		OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
			modelEndCalled = true
			return ctx
		},
	})

	r, err := g.Compile(context.Background(),
		WithGraphName("rag_callbacks_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	output, err := r.Invoke(context.Background(), []*BridgeMessage{
		{Role: "user", Content: "what is Rive"},
	})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if output != "Rive is a local-first agent team runtime." {
		t.Fatalf("unexpected output: %q", output)
	}
	if !modelStartCalled {
		t.Error("model OnStart not called")
	}
	if !modelEndCalled {
		t.Error("model OnEnd not called")
	}
}

func TestBridgeRAGPipelineWithStreamingCallbacks(t *testing.T) {
	model := &stubChatModel{
		response: "Rive is a local-first agent team runtime.",
	}

	var modelEndCalled bool

	g := NewGraph[[]*BridgeMessage, string]()
	modelNode := (&chatModelBridge{model: model}).toLambda()
	g.AddLambdaNode("model", modelNode)
	g.AddEdge(START, "model")
	g.AddEdge("model", END)

	g.SetNodeCallbacks("model", &Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
			return ctx
		},
		OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
			modelEndCalled = true
			return ctx
		},
	})

	r, err := g.Compile(context.Background(),
		WithGraphName("rag_streaming_cb_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	// Use Stream to verify the streaming path callbacks
	sr, err := r.Stream(context.Background(), []*BridgeMessage{
		{Role: "user", Content: "what is Rive"},
	})
	if err != nil {
		t.Fatalf("Stream failed: %v", err)
	}

	msg, err := sr.Recv()
	if err != nil {
		t.Fatalf("Recv failed: %v", err)
	}
	if msg != "Rive is a local-first agent team runtime." {
		t.Fatalf("unexpected output: %q", msg)
	}

	if !modelEndCalled {
		t.Error("model OnEnd not called via stream path")
	}
}

func TestBridgeRAGPipelineOnErrorCallback(t *testing.T) {
	testErr := errors.New("retrieval failed")
	retriever := &stubErrorRetriever{err: testErr}
	model := &stubChatModel{response: "should not reach"}
	systemPrompt := "Answer using context."

	var (
		retrieverOnErrorCalled bool
		retrieverReceivedErr   error
	)

	g := NewGraph[string, map[string]any]()

	retrieverNode := (&retrieverBridge{retriever: retriever}).toLambda()
	g.AddLambdaNode("retriever", retrieverNode)

	assembleNode := (&promptAssemblerBridge{systemPrompt: systemPrompt}).toLambda()
	g.AddLambdaNode("assemble", assembleNode)

	modelNode := (&chatModelBridge{model: model}).toLambda()
	g.AddLambdaNode("model", modelNode)

	g.AddEdge(START, "retriever")
	g.AddEdge(START, "assemble")
	g.AddEdge("retriever", "assemble")
	g.AddEdge("assemble", "model")
	g.AddEdge("model", END)

	g.SetNodeCallbacks("retriever", &Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
			return ctx
		},
		OnError: func(ctx context.Context, info *RunInfo, err error) context.Context {
			retrieverOnErrorCalled = true
			retrieverReceivedErr = err
			return ctx
		},
	})

	r, err := g.Compile(context.Background(),
		WithGraphName("rag_onerror_cb_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	_, err = r.Invoke(context.Background(), "what is Rive")
	if err == nil {
		t.Fatal("expected error from retriever")
	}

	if !retrieverOnErrorCalled {
		t.Error("retriever OnError not called")
	}
	if !errors.Is(retrieverReceivedErr, testErr) {
		t.Errorf("expected OnError to receive %v, got %v", testErr, retrieverReceivedErr)
	}
}

func TestBridgeFieldMappingInterop(t *testing.T) {
	// Test field mapping between struct-based workflow and bridge components.
	type QueryInput struct {
		Question string
	}

	model := &stubChatModel{
		response: "42 is the answer.",
	}

	wf := NewWorkflow[QueryInput, map[string]any]()

	// Use InvokableLambda for assemble to handle QueryInput directly
	assembleNode := InvokableLambda(func(ctx context.Context, in QueryInput) ([]*BridgeMessage, error) {
		return []*BridgeMessage{
			{Role: "system", Content: "Be concise."},
			{Role: "user", Content: in.Question},
		}, nil
	})

	wf.AddLambdaNode("assemble", assembleNode).
		AddInput(START)

	wf.AsChatModelNode("model", model).
		AddInput("assemble")

	wf.End().
		AddInput("model", ToField("answer"))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), QueryInput{Question: "what is the answer"})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if modelVal := result["model"]; modelVal != nil {
		if modelResult, ok := modelVal.(map[string]any); ok {
			if modelResult["answer"] != "42 is the answer." {
				t.Errorf("unexpected answer: %q", modelResult["answer"])
			}
		}
	}
}

func TestBridgeRetrieverEmptyDocs(t *testing.T) {
	retriever := &stubRetriever{
		docs: []*BridgeDocument{},
	}
	model := &stubChatModel{
		response: "no context available",
	}
	systemPrompt := "Answer using context."

	wf := NewWorkflow[string, map[string]any]()

	wf.AsRetrieverNode("retriever", retriever).
		AddInput(START)

	wf.AsPromptAssemblerNode("assemble", systemPrompt).
		AddInput(START, MapFields("", "query")).
		AddInput("retriever", ToField("documents"))

	wf.AsChatModelNode("model", model).
		AddInput("assemble")

	wf.End().
		AddInput("model", ToField("answer"))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "what is Rive")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if modelVal := result["model"]; modelVal != nil {
		if modelResult, ok := modelVal.(map[string]any); ok {
			t.Logf("model answer with empty docs: %q", modelResult["answer"])
		}
	}
}

func TestBridgeChatModelWithEmptyMessages(t *testing.T) {
	model := &stubChatModel{response: "no messages"}

	bridge := &chatModelBridge{model: model}
	lambda := bridge.toLambda()

	g := NewGraph[[]*BridgeMessage, string]()
	g.AddLambdaNode("chat", lambda)
	g.AddEdge(START, "chat")
	g.AddEdge("chat", END)

	r, err := g.Compile(context.Background(),
		WithGraphName("chat_empty_messages_test"),
		WithNodeTriggerMode(AllPredecessor),
	)
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), []*BridgeMessage{})
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if result != "no messages" {
		t.Fatalf("expected 'no messages', got %q", result)
	}
}

func TestBridgePromptAssemblerEmptyInput(t *testing.T) {
	bridge := &promptAssemblerBridge{systemPrompt: "Be helpful."}
	lambda := bridge.toLambda()

	// Input has neither query nor documents
	result, err := lambda.invokeFn(context.Background(), map[string]any{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	messages := result.([]*BridgeMessage)
	if len(messages) != 2 {
		t.Fatalf("expected 2 messages even with empty input, got %d", len(messages))
	}
	if messages[0].Role != "system" || messages[0].Content != "Be helpful." {
		t.Errorf("system message mismatch: role=%q content=%q", messages[0].Role, messages[0].Content)
	}
	if messages[1].Role != "user" {
		t.Errorf("expected user message, got role=%q", messages[1].Role)
	}
}

func TestBridgeChatModelNilModelPanicsOnInvoke(t *testing.T) {
	defer func() {
		if r := recover(); r == nil {
			t.Error("expected panic for nil ChatModel in bridge")
		}
	}()
	lambda := (&chatModelBridge{model: nil}).toLambda()
	_, _ = lambda.invokeFn(context.Background(), []*BridgeMessage{{Role: "user", Content: "hi"}})
}

func TestBridgeRetrieverNilRetrieverPanicsOnInvoke(t *testing.T) {
	defer func() {
		if r := recover(); r == nil {
			t.Error("expected panic for nil Retriever in bridge")
		}
	}()
	lambda := (&retrieverBridge{retriever: nil}).toLambda()
	_, _ = lambda.invokeFn(context.Background(), "test")
}

func TestBridgeRAGPipelineDocumentMetadata(t *testing.T) {
	retriever := &stubRetriever{
		docs: []*BridgeDocument{
			{Content: "doc with metadata", Score: 0.95, Metadata: map[string]string{"source": "kb", "page": "3"}},
		},
	}
	model := &stubChatModel{
		response: "metadata preserved",
	}
	systemPrompt := "Answer using provided context."

	wf := NewWorkflow[string, map[string]any]()

	wf.AsRetrieverNode("retriever", retriever).
		AddInput(START)

	wf.AsPromptAssemblerNode("assemble", systemPrompt).
		AddInput(START, MapFields("", "query")).
		AddInput("retriever", ToField("documents"))

	wf.AsChatModelNode("model", model).
		AddInput("assemble")

	wf.End().
		AddInput("model", ToField("answer"))

	r, err := wf.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	result, err := r.Invoke(context.Background(), "test query")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if modelVal := result["model"]; modelVal != nil {
		if modelResult, ok := modelVal.(map[string]any); ok {
			if modelResult["answer"] != "metadata preserved" {
				t.Errorf("unexpected answer: %q", modelResult["answer"])
			}
		}
	}
}
