package compose

import (
	"context"
	"fmt"
)

type parallelNode struct {
	outputKey string
	lambda    *Lambda
}

type Parallel struct {
	nodes      []parallelNode
	outputKeys map[string]bool
	err        error
}

func NewParallel() *Parallel {
	return &Parallel{
		outputKeys: make(map[string]bool),
	}
}

func (p *Parallel) AddLambda(outputKey string, node *Lambda) *Parallel {
	if p.err != nil {
		return p
	}
	if node == nil {
		p.err = fmt.Errorf("parallel add node err, lambda is nil")
		return p
	}
	if _, ok := p.outputKeys[outputKey]; ok {
		p.err = fmt.Errorf("parallel add node err, duplicate output key= %s", outputKey)
		return p
	}
	p.outputKeys[outputKey] = true
	p.nodes = append(p.nodes, parallelNode{outputKey: outputKey, lambda: node})
	return p
}

func (p *Parallel) AddGraph(outputKey string, sub subGraph) *Parallel {
	if p.err != nil {
		return p
	}
	if _, ok := p.outputKeys[outputKey]; ok {
		p.err = fmt.Errorf("parallel add node err, duplicate output key= %s", outputKey)
		return p
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
	p.outputKeys[outputKey] = true
	p.nodes = append(p.nodes, parallelNode{outputKey: outputKey, lambda: wrapper})
	return p
}

func (p *Parallel) AddPassthrough(outputKey string) *Parallel {
	if p.err != nil {
		return p
	}
	if _, ok := p.outputKeys[outputKey]; ok {
		p.err = fmt.Errorf("parallel add node err, duplicate output key= %s", outputKey)
		return p
	}
	identity := InvokableLambda(func(ctx context.Context, in any) (any, error) {
		return in, nil
	})
	p.outputKeys[outputKey] = true
	p.nodes = append(p.nodes, parallelNode{outputKey: outputKey, lambda: identity})
	return p
}

func (p *Parallel) Error() error {
	return p.err
}
