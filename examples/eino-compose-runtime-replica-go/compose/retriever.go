package compose

import (
	"context"
	"fmt"
)

const ComponentOfRetriever ComponentType = "Retriever"

type Document struct {
	Content  string
	Metadata map[string]string
}

type Query struct {
	Text string
	K    int
}

type Retriever interface {
	Retrieve(ctx context.Context, query *Query) ([]*Document, error)
}

type FakeRetriever struct {
	Docs       []*Document
	Err        error
	RetrieveFn func(ctx context.Context, query *Query) ([]*Document, error)
}

func (f *FakeRetriever) Retrieve(ctx context.Context, query *Query) ([]*Document, error) {
	if f.RetrieveFn != nil {
		return f.RetrieveFn(ctx, query)
	}
	if f.Err != nil {
		return nil, f.Err
	}
	return f.Docs, nil
}

type RetrieverConfig struct {
	Retriever Retriever
	Info      *RunInfo
	Handlers  []*Handler
}

func NewRetrieverLambda(cfg *RetrieverConfig) *Lambda {
	if cfg.Retriever == nil {
		panic("RetrieverConfig.Retriever must not be nil")
	}

	info := cfg.Info
	if info == nil {
		info = &RunInfo{
			Name:      "Retriever",
			Type:      "Retriever",
			Component: ComponentOfRetriever,
		}
	}

	invokeFn := func(ctx context.Context, input any) (any, error) {
		query, ok := input.(*Query)
		if !ok {
			return nil, fmt.Errorf("Retriever: expected *Query input, got %T", input)
		}
		return cfg.Retriever.Retrieve(ctx, query)
	}

	if len(cfg.Handlers) > 0 {
		cw := NewCallbackWrapper(info, cfg.Handlers)
		invokeFn = cw.Invoke(invokeFn)
	}

	cr := &composableRunnable{i: invokeFn}
	return &Lambda{invokeFn: invokeFn, cr: cr, kind: "RetrieverLambda"}
}
