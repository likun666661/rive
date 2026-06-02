package compose

import (
	"context"
	"fmt"
)

type Runnable[I, O any] interface {
	Invoke(ctx context.Context, input I) (output O, err error)
}

type composableRunnable struct {
	i func(ctx context.Context, input any) (output any, err error)
	s func(ctx context.Context, input any) (output any, err error)
}

func (cr *composableRunnable) invoke(ctx context.Context, input any) (any, error) {
	if cr.i == nil {
		return nil, fmt.Errorf("runnable: Invoke not supported")
	}
	return cr.i(ctx, input)
}

func (cr *composableRunnable) stream(ctx context.Context, input any) (any, error) {
	if cr.s != nil {
		return cr.s(ctx, input)
	}
	if cr.i != nil {
		out, err := cr.i(ctx, input)
		if err != nil {
			return nil, err
		}
		return out, nil
	}
	return nil, fmt.Errorf("runnable: Stream not supported")
}

func (cr *composableRunnable) nil() bool {
	return cr.i == nil && cr.s == nil
}

type Lambda struct {
	invokeFn func(ctx context.Context, input any) (output any, err error)
	cr       *composableRunnable
	kind     string
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

func (l *Lambda) GetRunnable() *composableRunnable {
	if l.cr != nil {
		return l.cr
	}
	l.cr = &composableRunnable{i: l.invokeFn}
	return l.cr
}

func (l *Lambda) GetComponentType() ComponentType {
	return ComponentOfLambda
}
