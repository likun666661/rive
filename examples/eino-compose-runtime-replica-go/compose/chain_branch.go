package compose

import (
	"context"
	"fmt"
)

type GraphBranchCondition[T any] func(ctx context.Context, in T) (endNode string, err error)

type GraphMultiBranchCondition[T any] func(ctx context.Context, in T) (endNode map[string]bool, err error)

type ChainBranch struct {
	condition      func(ctx context.Context, input any) (string, error)
	multiCondition func(ctx context.Context, input any) (map[string]bool, error)
	lambdas        map[string]*Lambda
	err            error
}

func NewChainBranch[T any](cond GraphBranchCondition[T]) *ChainBranch {
	if cond == nil {
		return &ChainBranch{
			lambdas: make(map[string]*Lambda),
			err:     fmt.Errorf("chain branch condition is nil"),
		}
	}
	return &ChainBranch{
		lambdas: make(map[string]*Lambda),
		condition: func(ctx context.Context, input any) (string, error) {
			typed, ok := input.(T)
			if !ok {
				var zero T
				return "", fmt.Errorf("chain branch: expected %T, got %T", zero, input)
			}
			return cond(ctx, typed)
		},
	}
}

func NewChainMultiBranch[T any](cond GraphMultiBranchCondition[T]) *ChainBranch {
	if cond == nil {
		return &ChainBranch{
			lambdas: make(map[string]*Lambda),
			err:     fmt.Errorf("chain multi-branch condition is nil"),
		}
	}
	return &ChainBranch{
		lambdas: make(map[string]*Lambda),
		multiCondition: func(ctx context.Context, input any) (map[string]bool, error) {
			typed, ok := input.(T)
			if !ok {
				var zero T
				return nil, fmt.Errorf("chain multi-branch: expected %T, got %T", zero, input)
			}
			return cond(ctx, typed)
		},
	}
}

func (cb *ChainBranch) AddLambda(key string, node *Lambda) *ChainBranch {
	if cb.err != nil {
		return cb
	}
	if node == nil {
		cb.err = fmt.Errorf("chain branch add node err, lambda is nil for key %s", key)
		return cb
	}
	cb.lambdas[key] = node
	return cb
}

func (cb *ChainBranch) AddGraph(key string, sub subGraph) *ChainBranch {
	if cb.err != nil {
		return cb
	}
	wrapper := InvokableLambda(func(ctx context.Context, in any) (any, error) {
		if err := sub.finalizeSubGraph(ctx); err != nil {
			return nil, err
		}
		gn := &graphNode{g: sub.innerGraph()}
		cr, err := gn.compileIfNeeded(ctx, newNodeCompileOptions())
		if err != nil {
			return nil, err
		}
		return cr.invoke(ctx, in)
	})
	cb.lambdas[key] = wrapper
	return cb
}

func (cb *ChainBranch) AddPassthrough(key string) *ChainBranch {
	if cb.err != nil {
		return cb
	}
	identity := InvokableLambda(func(ctx context.Context, in any) (any, error) {
		return in, nil
	})
	cb.lambdas[key] = identity
	return cb
}

func (cb *ChainBranch) Error() error {
	return cb.err
}
