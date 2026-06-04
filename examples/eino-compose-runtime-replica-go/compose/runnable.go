package compose

import (
	"context"
	"fmt"
	"io"
)

// StreamReader is the interface for reading streaming data.
type StreamReader[T any] interface {
	Recv() (T, error)
}

// Runnable is the core execution interface with four modes.
type Runnable[I, O any] interface {
	Invoke(ctx context.Context, input I) (output O, err error)
	Stream(ctx context.Context, input I) (output StreamReader[O], err error)
	Collect(ctx context.Context, input StreamReader[I]) (output O, err error)
	Transform(ctx context.Context, input StreamReader[I]) (output StreamReader[O], err error)
}

// streamReader is the internal, non-generic stream interface used for fallback chains.
type streamReader interface {
	Recv() (any, error)
}

// internalStreamReader is a concrete stream reader used for fallback implementation.
type internalStreamReader struct {
	items []any
	pos   int
}

func (r *internalStreamReader) Recv() (any, error) {
	if r.pos >= len(r.items) {
		return nil, io.EOF
	}
	v := r.items[r.pos]
	r.pos++
	return v, nil
}

// typedStreamWrapper adapts a typed StreamReader[T] to the internal streamReader interface.
type typedStreamWrapper[T any] struct {
	inner StreamReader[T]
}

func (w *typedStreamWrapper[T]) Recv() (any, error) {
	v, err := w.inner.Recv()
	if err != nil {
		return nil, err
	}
	return v, nil
}

// untypedStreamWrapper adapts an internal streamReader to a typed StreamReader[T].
type untypedStreamWrapper[T any] struct {
	inner streamReader
}

func (w *untypedStreamWrapper[T]) Recv() (T, error) {
	v, err := w.inner.Recv()
	if err != nil {
		var zero T
		return zero, err
	}
	typed, ok := v.(T)
	if !ok {
		var zero T
		return zero, fmt.Errorf("untypedStreamWrapper: expected %T, got %T", zero, v)
	}
	return typed, nil
}

func recvAll(sr streamReader) ([]any, error) {
	var items []any
	for {
		v, err := sr.Recv()
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, err
		}
		items = append(items, v)
	}
	return items, nil
}

func streamFromItems(items ...any) streamReader {
	return &internalStreamReader{items: items}
}

func collected(items []any) any {
	if len(items) == 0 {
		return nil
	}
	if len(items) == 1 {
		return items[0]
	}
	return items
}

type composableRunnable struct {
	i func(ctx context.Context, input any) (output any, err error)
	s func(ctx context.Context, input any) (output any, err error)
	c func(ctx context.Context, input any) (output any, err error)
	t func(ctx context.Context, input any) (output any, err error)
}

func (cr *composableRunnable) invoke(ctx context.Context, input any) (any, error) {
	// 1. native Invoke
	if cr.i != nil {
		return cr.i(ctx, input)
	}
	// 2. by Stream
	if cr.s != nil {
		sr, err := cr.s(ctx, input)
		if err != nil {
			return nil, err
		}
		wr, ok := sr.(streamReader)
		if !ok {
			return nil, fmt.Errorf("invoke by stream: output is not a stream reader, got %T", sr)
		}
		items, err := recvAll(wr)
		if err != nil {
			return nil, err
		}
		return collected(items), nil
	}
	// 3. by Collect
	if cr.c != nil {
		out, err := cr.c(ctx, streamFromItems(input))
		if err != nil {
			return nil, err
		}
		return out, nil
	}
	// 4. by Transform
	if cr.t != nil {
		sr, err := cr.t(ctx, streamFromItems(input))
		if err != nil {
			return nil, err
		}
		wr, ok := sr.(streamReader)
		if !ok {
			return nil, fmt.Errorf("invoke by transform: output is not a stream reader, got %T", sr)
		}
		items, err := recvAll(wr)
		if err != nil {
			return nil, err
		}
		return collected(items), nil
	}
	return nil, fmt.Errorf("runnable: Invoke not supported")
}

func (cr *composableRunnable) stream(ctx context.Context, input any) (any, error) {
	// 1. native Stream
	if cr.s != nil {
		return cr.s(ctx, input)
	}
	// 2. by Transform
	if cr.t != nil {
		return cr.t(ctx, streamFromItems(input))
	}
	// 3. by Invoke
	if cr.i != nil {
		out, err := cr.i(ctx, input)
		if err != nil {
			return nil, err
		}
		return streamFromItems(out), nil
	}
	// 4. by Collect
	if cr.c != nil {
		out, err := cr.c(ctx, streamFromItems(input))
		if err != nil {
			return nil, err
		}
		return streamFromItems(out), nil
	}
	return nil, fmt.Errorf("runnable: Stream not supported")
}

func (cr *composableRunnable) collect(ctx context.Context, input any) (any, error) {
	// 1. native Collect
	if cr.c != nil {
		return cr.c(ctx, input)
	}
	wr, ok := input.(streamReader)
	if !ok {
		return nil, fmt.Errorf("collect: expected stream reader input, got %T", input)
	}
	// 2. by Transform
	if cr.t != nil {
		sr, err := cr.t(ctx, input)
		if err != nil {
			return nil, err
		}
		tr, ok := sr.(streamReader)
		if !ok {
			return nil, fmt.Errorf("collect by transform: output is not a stream reader, got %T", sr)
		}
		items, err := recvAll(tr)
		if err != nil {
			return nil, err
		}
		return collected(items), nil
	}
	// 3. by Invoke
	if cr.i != nil {
		items, err := recvAll(wr)
		if err != nil {
			return nil, err
		}
		return cr.i(ctx, collected(items))
	}
	// 4. by Stream
	if cr.s != nil {
		items, err := recvAll(wr)
		if err != nil {
			return nil, err
		}
		sr, err := cr.s(ctx, collected(items))
		if err != nil {
			return nil, err
		}
		tr, ok := sr.(streamReader)
		if !ok {
			return nil, fmt.Errorf("collect by stream: output is not a stream reader, got %T", sr)
		}
		items2, err := recvAll(tr)
		if err != nil {
			return nil, err
		}
		return collected(items2), nil
	}
	return nil, fmt.Errorf("runnable: Collect not supported")
}

func (cr *composableRunnable) transform(ctx context.Context, input any) (any, error) {
	// 1. native Transform
	if cr.t != nil {
		return cr.t(ctx, input)
	}
	wr, ok := input.(streamReader)
	if !ok {
		return nil, fmt.Errorf("transform: expected stream reader input, got %T", input)
	}
	// 2. by Stream
	if cr.s != nil {
		items, err := recvAll(wr)
		if err != nil {
			return nil, err
		}
		var allResults []any
		for _, item := range items {
			sr, err := cr.s(ctx, item)
			if err != nil {
				return nil, err
			}
			tr, ok := sr.(streamReader)
			if !ok {
				return nil, fmt.Errorf("transform by stream: stream output is not a stream reader, got %T", sr)
			}
			subItems, err := recvAll(tr)
			if err != nil {
				return nil, err
			}
			allResults = append(allResults, subItems...)
		}
		return streamFromItems(allResults...), nil
	}
	// 3. by Collect
	if cr.c != nil {
		out, err := cr.c(ctx, input)
		if err != nil {
			return nil, err
		}
		return streamFromItems(out), nil
	}
	// 4. by Invoke
	if cr.i != nil {
		items, err := recvAll(wr)
		if err != nil {
			return nil, err
		}
		out, err := cr.i(ctx, collected(items))
		if err != nil {
			return nil, err
		}
		return streamFromItems(out), nil
	}
	return nil, fmt.Errorf("runnable: Transform not supported")
}

func (cr *composableRunnable) nil() bool {
	return cr.i == nil && cr.s == nil && cr.c == nil && cr.t == nil
}

type Lambda struct {
	invokeFn    func(ctx context.Context, input any) (output any, err error)
	streamFn    func(ctx context.Context, input any) (output any, err error)
	collectFn   func(ctx context.Context, input any) (output any, err error)
	transformFn func(ctx context.Context, input any) (output any, err error)
	cr          *composableRunnable
	kind        string
}

func InvokableLambda[I, O any](fn func(ctx context.Context, input I) (O, error)) *Lambda {
	cr := &composableRunnable{
		i: func(ctx context.Context, input any) (any, error) {
			typedInput, ok := input.(I)
			if !ok {
				var zero I
				return zero, fmt.Errorf("InvokableLambda: expected input type %T, got %T", zero, input)
			}
			return fn(ctx, typedInput)
		},
	}
	return &Lambda{invokeFn: cr.i, cr: cr, kind: "InvokableLambda"}
}

func StreamableLambda[I, O any](fn func(ctx context.Context, input I) (StreamReader[O], error)) *Lambda {
	cr := &composableRunnable{
		s: func(ctx context.Context, input any) (any, error) {
			typedInput, ok := input.(I)
			if !ok {
				var zero I
				return nil, fmt.Errorf("StreamableLambda: expected input type %T, got %T", zero, input)
			}
			sr, err := fn(ctx, typedInput)
			if err != nil {
				return nil, err
			}
			return &typedStreamWrapper[O]{inner: sr}, nil
		},
	}
	return &Lambda{streamFn: cr.s, cr: cr, kind: "StreamableLambda"}
}

func CollectableLambda[I, O any](fn func(ctx context.Context, input StreamReader[I]) (O, error)) *Lambda {
	cr := &composableRunnable{
		c: func(ctx context.Context, input any) (any, error) {
			wr, ok := input.(streamReader)
			if !ok {
				return nil, fmt.Errorf("CollectableLambda: expected stream reader input, got %T", input)
			}
			typedSR := &untypedStreamWrapper[I]{inner: wr}
			return fn(ctx, typedSR)
		},
	}
	return &Lambda{collectFn: cr.c, cr: cr, kind: "CollectableLambda"}
}

func TransformableLambda[I, O any](fn func(ctx context.Context, input StreamReader[I]) (StreamReader[O], error)) *Lambda {
	cr := &composableRunnable{
		t: func(ctx context.Context, input any) (any, error) {
			wr, ok := input.(streamReader)
			if !ok {
				return nil, fmt.Errorf("TransformableLambda: expected stream reader input, got %T", input)
			}
			typedSR := &untypedStreamWrapper[I]{inner: wr}
			sr, err := fn(ctx, typedSR)
			if err != nil {
				return nil, err
			}
			return &typedStreamWrapper[O]{inner: sr}, nil
		},
	}
	return &Lambda{transformFn: cr.t, cr: cr, kind: "TransformableLambda"}
}

func (l *Lambda) GetRunnable() *composableRunnable {
	if l.cr != nil {
		return l.cr
	}
	l.cr = &composableRunnable{i: l.invokeFn, s: l.streamFn, c: l.collectFn, t: l.transformFn}
	return l.cr
}

func (l *Lambda) GetComponentType() ComponentType {
	return ComponentOfLambda
}
