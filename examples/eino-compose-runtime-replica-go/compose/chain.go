package compose

import (
	"context"
	"fmt"
)

type subGraph interface {
	innerGraph() *graph
	finalizeSubGraph(ctx context.Context) error
}

type Chain[I, O any] struct {
	err         error
	gg          *Graph[I, O]
	nodeIdx     int
	preNodeKeys []string
	hasEnd      bool
}

func NewChain[I, O any]() *Chain[I, O] {
	return &Chain[I, O]{
		gg: NewGraph[I, O](),
	}
}

func (c *Chain[I, O]) AppendLambda(lambda *Lambda) *Chain[I, O] {
	if c.err != nil {
		return c
	}
	if lambda == nil {
		c.reportError(fmt.Errorf("chain add node invalid, node is nil"))
		return c
	}
	key := c.nextNodeKey()
	if err := c.gg.AddLambdaNode(key, lambda); err != nil {
		c.reportError(err)
		return c
	}
	c.addNodeEdges(key)
	return c
}

func (c *Chain[I, O]) AppendPassthrough() *Chain[I, O] {
	if c.err != nil {
		return c
	}
	key := c.nextNodeKey()
	passthroughLambda := InvokableLambda(func(ctx context.Context, in any) (any, error) {
		return in, nil
	})
	if err := c.gg.AddLambdaNode(key, passthroughLambda); err != nil {
		c.reportError(err)
		return c
	}
	c.addNodeEdges(key)
	return c
}

func (c *Chain[I, O]) AppendParallel(p *Parallel) *Chain[I, O] {
	if c.err != nil {
		return c
	}
	if p == nil {
		c.reportError(fmt.Errorf("append parallel invalid, parallel is nil"))
		return c
	}
	if p.err != nil {
		c.reportError(fmt.Errorf("append parallel invalid, parallel error: %w", p.err))
		return c
	}
	if len(p.nodes) <= 1 {
		c.reportError(fmt.Errorf("append parallel invalid, not enough nodes, count = %d", len(p.nodes)))
		return c
	}

	var startNode string
	if len(c.preNodeKeys) == 0 {
		startNode = START
	} else if len(c.preNodeKeys) == 1 {
		startNode = c.preNodeKeys[0]
	} else {
		c.reportError(fmt.Errorf("append parallel invalid, multiple previous nodes: %v", c.preNodeKeys))
		return c
	}

	prefix := c.nextNodeKey()
	var nodeKeys []string

	outputKeyMap := make(map[string]string, len(p.nodes))
	for i := range p.nodes {
		pn := &p.nodes[i]
		nodeKey := fmt.Sprintf("%s_parallel_%d", prefix, i)
		outputKeyMap[nodeKey] = pn.outputKey
		if err := c.gg.AddLambdaNode(nodeKey, pn.lambda); err != nil {
			c.reportError(fmt.Errorf("add parallel node to chain failed, key=%s, err: %w", nodeKey, err))
			return c
		}
		if err := c.gg.AddEdge(startNode, nodeKey); err != nil {
			c.reportError(fmt.Errorf("add parallel edge failed, from=%s, to=%s, err: %w", startNode, nodeKey, err))
			return c
		}
		nodeKeys = append(nodeKeys, nodeKey)
	}

	mergeKey := prefix + "_merge"
	mergeLambda := InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
		out := make(map[string]any, len(in))
		for k, v := range in {
			if outputKey, ok := outputKeyMap[k]; ok {
				out[outputKey] = v
			} else {
				out[k] = v
			}
		}
		return out, nil
	})
	if err := c.gg.AddLambdaNode(mergeKey, mergeLambda); err != nil {
		c.reportError(fmt.Errorf("add parallel merge node failed, key=%s, err: %w", mergeKey, err))
		return c
	}
	for _, nodeKey := range nodeKeys {
		if err := c.gg.AddEdge(nodeKey, mergeKey); err != nil {
			c.reportError(fmt.Errorf("add parallel merge edge failed, from=%s, to=%s, err: %w", nodeKey, mergeKey, err))
			return c
		}
	}

	c.preNodeKeys = []string{mergeKey}
	return c
}

func (c *Chain[I, O]) AppendBranch(b *ChainBranch) *Chain[I, O] {
	if c.err != nil {
		return c
	}
	if b == nil {
		c.reportError(fmt.Errorf("append branch invalid, branch is nil"))
		return c
	}
	if b.err != nil {
		c.reportError(fmt.Errorf("append branch error: %w", b.err))
		return c
	}
	if len(b.lambdas) < 2 {
		c.reportError(fmt.Errorf("append branch invalid, nodeList length = %d, need at least 2", len(b.lambdas)))
		return c
	}

	var branchRouter *Lambda
	if b.multiCondition != nil {
		branchRouter = InvokableLambda(func(ctx context.Context, input any) (any, error) {
			selected, err := b.multiCondition(ctx, input)
			if err != nil {
				return nil, err
			}
			result := make(map[string]any)
			for key := range selected {
				if l, ok := b.lambdas[key]; ok {
					out, err := l.GetRunnable().invoke(ctx, input)
					if err != nil {
						return nil, fmt.Errorf("branch %s: %w", key, err)
					}
					result[key] = out
				}
			}
			return result, nil
		})
	} else {
		branchRouter = InvokableLambda(func(ctx context.Context, input any) (any, error) {
			selected, err := b.condition(ctx, input)
			if err != nil {
				return nil, err
			}
			l, ok := b.lambdas[selected]
			if !ok {
				return nil, fmt.Errorf("branch key not found: %s", selected)
			}
			return l.GetRunnable().invoke(ctx, input)
		})
	}

	key := c.nextNodeKey()
	if err := c.gg.AddLambdaNode(key, branchRouter); err != nil {
		c.reportError(err)
		return c
	}
	c.addNodeEdges(key)
	return c
}

func (c *Chain[I, O]) innerGraph() *graph {
	return c.gg.g
}

func (c *Chain[I, O]) finalizeSubGraph(ctx context.Context) error {
	return c.addEndIfNeeded()
}

func (c *Chain[I, O]) AppendGraph(sub subGraph) *Chain[I, O] {
	if c.err != nil {
		return c
	}
	if sub == nil {
		c.reportError(fmt.Errorf("chain add node invalid, sub chain is nil"))
		return c
	}
	if err := sub.finalizeSubGraph(context.Background()); err != nil {
		c.reportError(fmt.Errorf("chain append graph, finalize subgraph failed: %w", err))
		return c
	}
	key := c.nextNodeKey()
	gn := &graphNode{
		name: key,
		g:    sub.innerGraph(),
		info: &GraphNodeInfo{Name: key, Component: ComponentOfChain},
	}
	if err := c.gg.g.AddNode(key, gn); err != nil {
		c.reportError(err)
		return c
	}
	c.addNodeEdges(key)
	return c
}

func (c *Chain[I, O]) Compile(ctx context.Context) (Runnable[I, O], error) {
	if c.err != nil {
		return nil, c.err
	}
	if err := c.addEndIfNeeded(); err != nil {
		return nil, err
	}
	return c.gg.Compile(ctx, WithNodeTriggerMode(AllPredecessor))
}

func (c *Chain[I, O]) addNodeEdges(nodeKey string) {
	if len(c.preNodeKeys) == 0 {
		c.preNodeKeys = []string{START}
	}
	for _, preKey := range c.preNodeKeys {
		if err := c.gg.AddEdge(preKey, nodeKey); err != nil {
			c.reportError(err)
			return
		}
	}
	c.preNodeKeys = []string{nodeKey}
}

func (c *Chain[I, O]) addEndIfNeeded() error {
	if c.hasEnd {
		return nil
	}
	if c.err != nil {
		return c.err
	}
	if len(c.preNodeKeys) == 0 {
		return fmt.Errorf("pre node keys not set, number of nodes in chain= %d", 0)
	}
	for _, nodeKey := range c.preNodeKeys {
		if err := c.gg.AddEdge(nodeKey, END); err != nil {
			return err
		}
	}
	c.hasEnd = true
	return nil
}

func (c *Chain[I, O]) nextNodeKey() string {
	idx := c.nodeIdx
	c.nodeIdx++
	return fmt.Sprintf("node_%d", idx)
}

func (c *Chain[I, O]) reportError(err error) {
	if c.err == nil {
		c.err = err
	}
}
