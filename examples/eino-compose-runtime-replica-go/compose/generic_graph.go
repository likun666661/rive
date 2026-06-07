package compose

import (
	"context"
	"fmt"
)

type Graph[I, O any] struct {
	g      *graph
	input  I
	output O
}

func NewGraph[I, O any]() *Graph[I, O] {
	var zeroI I
	var zeroO O
	g := newGraph(fmtType(zeroI), fmtType(zeroO))
	return &Graph[I, O]{g: g}
}

func (gg *Graph[I, O]) AddLambdaNode(key string, lambda *Lambda) error {
	return gg.g.AddLambdaNode(key, lambda)
}

func (gg *Graph[I, O]) AddEdge(from, to string) error {
	return gg.g.AddEdge(from, to)
}

func (gg *Graph[I, O]) AddControlEdge(from, to string) error {
	return gg.g.AddControlEdge(from, to)
}

func (gg *Graph[I, O]) AddBranch(key string, branch *GraphBranch) error {
	return gg.g.AddBranch(key, branch)
}

func (gg *Graph[I, O]) SetNodeCallbacks(key string, handlers ...*Handler) error {
	return gg.g.SetNodeHandler(key, handlers...)
}

func (gg *Graph[I, O]) SetNodeInputPreHandler(key string, fn func(ctx context.Context, input any) (any, error)) error {
	gg.g.setNodeInputPreHandler(key, fn)
	return nil
}

func (gg *Graph[I, O]) AddChatModelNode(key string, cmc *ChatModelComponent, opts ...NodeOption) error {
	if err := gg.g.addChatModelNode(key, cmc); err != nil {
		return err
	}
	ns := &nodeOptionState{}
	for _, opt := range opts {
		opt(ns)
	}
	for _, h := range ns.inputPreHandlers {
		gg.g.setNodeInputPreHandler(key, h)
	}
	return nil
}

func (gg *Graph[I, O]) AddToolsNode(key string, tn *ToolsNode, opts ...NodeOption) error {
	if err := gg.g.addLambdaNode(key, tn.GetRunnable()); err != nil {
		return err
	}
	ns := &nodeOptionState{}
	for _, opt := range opts {
		opt(ns)
	}
	for _, h := range ns.inputPreHandlers {
		gg.g.setNodeInputPreHandler(key, h)
	}
	return nil
}

func (gg *Graph[I, O]) Compile(ctx context.Context, opts ...CompileOption) (Runnable[I, O], error) {
	o := newNodeCompileOptions(opts...)
	gg.g.graphName = o.graphName
	gg.g.genLocalState = o.genLocalState

	if o.nodeTriggerMode == AllPredecessor || o.nodeTriggerMode == AnyPredecessor {
		gi := newGraphInfo(o.graphName, o.nodeTriggerMode, o.maxSteps)
		gi.InputType = fmtType(gg.input)
		gi.OutputType = fmtType(gg.output)
		gg.g.graphInfo = gi
	} else {
		gi := newGraphInfo(o.graphName, AnyPredecessor, o.maxSteps)
		gi.InputType = fmtType(gg.input)
		gi.OutputType = fmtType(gg.output)
		gg.g.graphInfo = gi
	}

	r, err := gg.g.compile(ctx)
	if err != nil {
		return nil, err
	}
	if o.eventLog != nil {
		r.eventLog = o.eventLog
	} else if len(o.eventSinks) > 0 {
		r.eventLog = NewEventLog(o.eventSinks...)
	}

	if o.nodeTriggerMode == AllPredecessor {
		r.dag = true
		r.pregel = false
	} else {
		r.dag = false
		r.pregel = true
	}
	r.maxSteps = o.maxSteps
	r.eager = o.isEager()

	cr := r.toComposableRunnable()

	return &graphRunnable[I, O]{cr: cr, runner: r}, nil
}

func (gg *Graph[I, O]) GetGraphInfo() *GraphInfo {
	return gg.g.graphInfo
}

type graphRunnable[I, O any] struct {
	cr     *composableRunnable
	runner *runner
}

func (gr *graphRunnable[I, O]) Invoke(ctx context.Context, input I) (O, error) {
	output, err := gr.cr.invoke(ctx, input)
	if err != nil {
		var zero O
		return zero, err
	}
	typedOutput, ok := output.(O)
	if !ok {
		var zero O
		return zero, newTypeError(output, zero)
	}
	if gr.runner != nil && gr.runner.eventLog != nil {
		gr.runner.eventLog.LogGraphEnd(gr.runner.graphName, gr.runner.runStepCount)
	}
	return typedOutput, nil
}

func (gr *graphRunnable[I, O]) Stream(ctx context.Context, input I) (StreamReader[O], error) {
	sr, err := gr.cr.stream(ctx, input)
	if err != nil {
		return nil, err
	}
	wr, ok := sr.(streamReader)
	if !ok {
		return nil, fmt.Errorf("graph Stream: unexpected stream type %T", sr)
	}
	return &untypedStreamWrapper[O]{inner: wr}, nil
}

func (gr *graphRunnable[I, O]) Collect(ctx context.Context, input StreamReader[I]) (O, error) {
	wrapped := &typedStreamWrapper[I]{inner: input}
	output, err := gr.cr.collect(ctx, wrapped)
	if err != nil {
		var zero O
		return zero, err
	}
	typedOutput, ok := output.(O)
	if !ok {
		var zero O
		return zero, newTypeError(output, zero)
	}
	return typedOutput, nil
}

func (gr *graphRunnable[I, O]) Transform(ctx context.Context, input StreamReader[I]) (StreamReader[O], error) {
	wrapped := &typedStreamWrapper[I]{inner: input}
	sr, err := gr.cr.transform(ctx, wrapped)
	if err != nil {
		return nil, err
	}
	wr, ok := sr.(streamReader)
	if !ok {
		return nil, fmt.Errorf("graph Transform: unexpected stream type %T", sr)
	}
	return &untypedStreamWrapper[O]{inner: wr}, nil
}

func fmtType(v any) string {
	if v == nil {
		return "nil"
	}
	return extractTypeName(v)
}

func extractTypeName(v any) string {
	switch v.(type) {
	case string:
		return "string"
	case int:
		return "int"
	case float64:
		return "float64"
	case bool:
		return "bool"
	default:
		return "any"
	}
}

func newTypeError(got, want any) error {
	return &typeError{got: extractTypeName(got), want: extractTypeName(want)}
}

type typeError struct {
	got  string
	want string
}

func (e *typeError) Error() string {
	return "type error: got " + e.got + ", want " + e.want
}
