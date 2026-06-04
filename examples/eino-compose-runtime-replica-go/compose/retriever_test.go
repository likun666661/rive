package compose

import (
	"context"
	"errors"
	"fmt"
	"io"
	"testing"
)

func TestFakeRetrieverDefaults(t *testing.T) {
	r := &FakeRetriever{
		Docs: []*Document{
			{Content: "doc1", Metadata: map[string]string{"source": "A"}},
			{Content: "doc2", Metadata: map[string]string{"source": "B"}},
		},
	}

	docs, err := r.Retrieve(context.Background(), &Query{Text: "test", K: 5})
	if err != nil {
		t.Fatal(err)
	}
	if len(docs) != 2 {
		t.Fatalf("expected 2 docs, got %d", len(docs))
	}
	if docs[0].Content != "doc1" {
		t.Errorf("doc[0].Content: expected 'doc1', got %q", docs[0].Content)
	}
	if docs[1].Metadata["source"] != "B" {
		t.Errorf("doc[1].Metadata[source]: expected 'B', got %q", docs[1].Metadata["source"])
	}
}

func TestFakeRetrieverCustomFn(t *testing.T) {
	r := &FakeRetriever{
		RetrieveFn: func(ctx context.Context, query *Query) ([]*Document, error) {
			if query.K > 10 {
				return nil, errors.New("k too large")
			}
			return []*Document{
				{Content: query.Text, Metadata: map[string]string{"k": fmt.Sprintf("%d", query.K)}},
			}, nil
		},
	}

	docs, err := r.Retrieve(context.Background(), &Query{Text: "hello", K: 3})
	if err != nil {
		t.Fatal(err)
	}
	if len(docs) != 1 {
		t.Fatalf("expected 1 doc, got %d", len(docs))
	}
	if docs[0].Content != "hello" {
		t.Errorf("expected Content='hello', got %q", docs[0].Content)
	}

	_, err = r.Retrieve(context.Background(), &Query{Text: "fail", K: 20})
	if err == nil || err.Error() != "k too large" {
		t.Errorf("expected 'k too large', got %v", err)
	}
}

func TestFakeRetrieverError(t *testing.T) {
	r := &FakeRetriever{Err: errors.New("connection refused")}

	_, err := r.Retrieve(context.Background(), &Query{Text: "test", K: 1})
	if err == nil || err.Error() != "connection refused" {
		t.Errorf("expected 'connection refused', got %v", err)
	}
}

func TestNewRetrieverLambdaInvoke(t *testing.T) {
	r := &FakeRetriever{
		Docs: []*Document{
			{Content: "result1"},
			{Content: "result2"},
		},
	}

	l := NewRetrieverLambda(&RetrieverConfig{Retriever: r})
	cr := l.GetRunnable()

	out, err := cr.invoke(context.Background(), &Query{Text: "query", K: 5})
	if err != nil {
		t.Fatal(err)
	}

	docs, ok := out.([]*Document)
	if !ok {
		t.Fatalf("expected []*Document, got %T", out)
	}
	if len(docs) != 2 {
		t.Fatalf("expected 2 docs, got %d", len(docs))
	}
	if docs[0].Content != "result1" {
		t.Errorf("expected 'result1', got %q", docs[0].Content)
	}
	if docs[1].Content != "result2" {
		t.Errorf("expected 'result2', got %q", docs[1].Content)
	}
}

func TestNewRetrieverLambdaWrongInput(t *testing.T) {
	r := &FakeRetriever{}
	l := NewRetrieverLambda(&RetrieverConfig{Retriever: r})
	cr := l.GetRunnable()

	_, err := cr.invoke(context.Background(), "not a query")
	if err == nil {
		t.Fatal("expected error for wrong input type")
	}
}

func TestNewRetrieverLambdaStreamFallback(t *testing.T) {
	r := &FakeRetriever{
		Docs: []*Document{
			{Content: "docA"},
			{Content: "docB"},
		},
	}

	l := NewRetrieverLambda(&RetrieverConfig{Retriever: r})
	cr := l.GetRunnable()

	srRaw, err := cr.stream(context.Background(), &Query{Text: "q", K: 5})
	if err != nil {
		t.Fatal(err)
	}
	wr, ok := srRaw.(streamReader)
	if !ok {
		t.Fatalf("expected streamReader, got %T", srRaw)
	}

	v, err := wr.Recv()
	if err != nil {
		t.Fatal(err)
	}
	if v != nil {
		t.Logf("stream fallback item: %v", v)
	}

	_, err = wr.Recv()
	if err != io.EOF {
		t.Fatalf("expected EOF, got %v", err)
	}
}

func TestNewRetrieverLambdaCollectFallback(t *testing.T) {
	r := &FakeRetriever{
		Docs: []*Document{
			{Content: "hello"},
		},
	}

	l := NewRetrieverLambda(&RetrieverConfig{Retriever: r})
	cr := l.GetRunnable()

	out, err := cr.collect(context.Background(), streamFromItems(&Query{Text: "test", K: 1}))
	if err != nil {
		t.Fatal(err)
	}

	docs, ok := out.([]*Document)
	if !ok {
		t.Fatalf("expected []*Document, got %T", out)
	}
	if len(docs) != 1 {
		t.Fatalf("expected 1 doc, got %d", len(docs))
	}
	if docs[0].Content != "hello" {
		t.Errorf("expected 'hello', got %q", docs[0].Content)
	}
}

func TestNewRetrieverLambdaTransformFallback(t *testing.T) {
	r := &FakeRetriever{
		Docs: []*Document{
			{Content: "world"},
		},
	}

	l := NewRetrieverLambda(&RetrieverConfig{Retriever: r})
	cr := l.GetRunnable()

	srRaw, err := cr.transform(context.Background(), streamFromItems(&Query{Text: "q", K: 1}))
	if err != nil {
		t.Fatal(err)
	}
	wr, ok := srRaw.(streamReader)
	if !ok {
		t.Fatalf("expected streamReader, got %T", srRaw)
	}

	v, err := wr.Recv()
	if err != nil {
		t.Fatal(err)
	}
	if v != nil {
		t.Logf("transform fallback item: %v", v)
	}
}

func TestNewRetrieverLambdaCallbacksOnStartOnEnd(t *testing.T) {
	var (
		startCalled bool
		endCalled   bool
		startInput  any
		endOutput   any
	)

	r := &FakeRetriever{
		Docs: []*Document{
			{Content: "callback-doc"},
		},
	}

	handlers := []*Handler{
		{
			OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
				startCalled = true
				startInput = input
				return ctx
			},
			OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
				endCalled = true
				endOutput = output
				return ctx
			},
		},
	}

	l := NewRetrieverLambda(&RetrieverConfig{
		Retriever: r,
		Info: &RunInfo{
			Name:      "test-retriever",
			Type:      "Retriever",
			Component: ComponentOfRetriever,
		},
		Handlers: handlers,
	})
	cr := l.GetRunnable()

	query := &Query{Text: "callback test", K: 3}
	out, err := cr.invoke(context.Background(), query)
	if err != nil {
		t.Fatal(err)
	}

	docs := out.([]*Document)
	if len(docs) != 1 || docs[0].Content != "callback-doc" {
		t.Fatalf("unexpected output: %v", out)
	}

	if !startCalled {
		t.Error("OnStart was not called")
	}
	if startInput != query {
		t.Errorf("OnStart input mismatch: got %v", startInput)
	}
	if !endCalled {
		t.Error("OnEnd was not called")
	}
	if endOutput == nil {
		t.Error("OnEnd output is nil")
	}
}

func TestNewRetrieverLambdaCallbacksOnError(t *testing.T) {
	var (
		startCalled bool
		endCalled   bool
		errorCalled bool
		receivedErr error
	)

	testErr := errors.New("retrieval failed")

	r := &FakeRetriever{Err: testErr}

	handlers := []*Handler{
		{
			OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
				startCalled = true
				return ctx
			},
			OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
				endCalled = true
				return ctx
			},
			OnError: func(ctx context.Context, info *RunInfo, err error) context.Context {
				errorCalled = true
				receivedErr = err
				return ctx
			},
		},
	}

	l := NewRetrieverLambda(&RetrieverConfig{
		Retriever: r,
		Handlers:  handlers,
	})
	cr := l.GetRunnable()

	_, err := cr.invoke(context.Background(), &Query{Text: "error", K: 1})
	if err == nil {
		t.Fatal("expected error")
	}

	if !startCalled {
		t.Error("OnStart was not called")
	}
	if endCalled {
		t.Error("OnEnd should not be called on error")
	}
	if !errorCalled {
		t.Error("OnError was not called")
	}
	if !errors.Is(receivedErr, testErr) {
		t.Errorf("OnError received wrong error: %v", receivedErr)
	}
}

func TestNewRetrieverLambdaNoCallbacks(t *testing.T) {
	r := &FakeRetriever{
		Docs: []*Document{
			{Content: "no-cb-doc"},
		},
	}

	l := NewRetrieverLambda(&RetrieverConfig{
		Retriever: r,
		Handlers:  nil,
	})
	cr := l.GetRunnable()

	out, err := cr.invoke(context.Background(), &Query{Text: "nocb", K: 1})
	if err != nil {
		t.Fatal(err)
	}

	docs := out.([]*Document)
	if len(docs) != 1 || docs[0].Content != "no-cb-doc" {
		t.Fatalf("unexpected output: %v", out)
	}
}

func TestNewRetrieverLambdaDefaultInfo(t *testing.T) {
	r := &FakeRetriever{
		Docs: []*Document{
			{Content: "default-info-doc"},
		},
	}

	l := NewRetrieverLambda(&RetrieverConfig{Retriever: r})
	if l.kind != "RetrieverLambda" {
		t.Errorf("expected kind 'RetrieverLambda', got %q", l.kind)
	}
	if l.GetComponentType() != ComponentOfLambda {
		t.Errorf("expected ComponentOfLambda, got %q", l.GetComponentType())
	}
}

func TestDocumentMetadata(t *testing.T) {
	doc := &Document{
		Content: "test content",
		Metadata: map[string]string{
			"source": "file.txt",
			"score":  "0.95",
		},
	}
	if doc.Content != "test content" {
		t.Errorf("Content mismatch")
	}
	if doc.Metadata["source"] != "file.txt" {
		t.Errorf("Metadata source mismatch")
	}
	if doc.Metadata["score"] != "0.95" {
		t.Errorf("Metadata score mismatch")
	}
}

func TestEmptyQueryDefaults(t *testing.T) {
	q := &Query{}
	if q.Text != "" {
		t.Errorf("empty Query Text should be ''")
	}
	if q.K != 0 {
		t.Errorf("empty Query K should be 0")
	}
}

func TestNewRetrieverLambdaPanicsOnNilRetriever(t *testing.T) {
	defer func() {
		if r := recover(); r == nil {
			t.Error("expected panic for nil Retriever")
		}
	}()
	NewRetrieverLambda(&RetrieverConfig{})
}

func TestFakeRetrieverEmptyDocs(t *testing.T) {
	r := &FakeRetriever{}
	docs, err := r.Retrieve(context.Background(), &Query{Text: "any", K: 5})
	if err != nil {
		t.Fatal(err)
	}
	if len(docs) != 0 {
		t.Errorf("expected 0 docs, got %d", len(docs))
	}
}

func TestComponentOfRetriever(t *testing.T) {
	if string(ComponentOfRetriever) != "Retriever" {
		t.Errorf("expected 'Retriever', got %q", string(ComponentOfRetriever))
	}
}

func TestNewRetrieverLambdaMultipleHandlers(t *testing.T) {
	var (
		h1StartCalled bool
		h2StartCalled bool
		h1EndCalled   bool
		h2EndCalled   bool
	)

	r := &FakeRetriever{
		Docs: []*Document{
			{Content: "multi-handler-doc"},
		},
	}

	handlers := []*Handler{
		{
			OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
				h1StartCalled = true
				return ctx
			},
			OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
				h1EndCalled = true
				return ctx
			},
		},
		{
			OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
				h2StartCalled = true
				return ctx
			},
			OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
				h2EndCalled = true
				return ctx
			},
		},
	}

	l := NewRetrieverLambda(&RetrieverConfig{
		Retriever: r,
		Handlers:  handlers,
	})
	cr := l.GetRunnable()

	_, err := cr.invoke(context.Background(), &Query{Text: "multi", K: 1})
	if err != nil {
		t.Fatal(err)
	}

	if !h1StartCalled {
		t.Error("handler 1 OnStart not called")
	}
	if !h2StartCalled {
		t.Error("handler 2 OnStart not called")
	}
	if !h1EndCalled {
		t.Error("handler 1 OnEnd not called")
	}
	if !h2EndCalled {
		t.Error("handler 2 OnEnd not called")
	}
}

func TestFakeRetrieverFnNilReturnsDocs(t *testing.T) {
	r := &FakeRetriever{
		Docs:       []*Document{{Content: "fallback"}},
		RetrieveFn: nil,
		Err:        nil,
	}

	docs, err := r.Retrieve(context.Background(), &Query{Text: "test", K: 1})
	if err != nil {
		t.Fatal(err)
	}
	if len(docs) != 1 || docs[0].Content != "fallback" {
		t.Fatalf("expected fallback doc, got %v", docs)
	}
}

func TestNewRetrieverLambdaPerHandlerContextChain(t *testing.T) {
	type ctxKey string

	var h1EndCtxVal string
	var h2EndCtxVal string

	r := &FakeRetriever{
		Docs: []*Document{{Content: "ctx-chain-doc"}},
	}

	handlers := []*Handler{
		{
			OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
				return context.WithValue(ctx, ctxKey("key"), "from-h1")
			},
			OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
				h1EndCtxVal, _ = ctx.Value(ctxKey("key")).(string)
				return ctx
			},
		},
		{
			OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
				return context.WithValue(ctx, ctxKey("key"), "from-h2")
			},
			OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
				h2EndCtxVal, _ = ctx.Value(ctxKey("key")).(string)
				return ctx
			},
		},
	}

	l := NewRetrieverLambda(&RetrieverConfig{
		Retriever: r,
		Handlers:  handlers,
	})
	cr := l.GetRunnable()

	rootCtx := context.WithValue(context.Background(), ctxKey("key"), "root")
	_, err := cr.invoke(rootCtx, &Query{Text: "ctx", K: 1})
	if err != nil {
		t.Fatal(err)
	}

	if h1EndCtxVal != "from-h1" {
		t.Errorf("h1 OnEnd expected 'from-h1', got %q", h1EndCtxVal)
	}
	if h2EndCtxVal != "from-h2" {
		t.Errorf("h2 OnEnd expected 'from-h2', got %q", h2EndCtxVal)
	}
}
