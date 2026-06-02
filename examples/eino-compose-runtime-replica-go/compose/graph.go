package compose

import (
	"context"
	"fmt"
)

type graph struct {
	nodes        map[string]*graphNode
	controlEdges map[string][]string
	dataEdges    map[string][]string
	branches     map[string][]*GraphBranch
	graphName    string
	compiled     bool
	inputType    string
	outputType   string
	state        any
	graphInfo    *GraphInfo

	chanSubscribeTo     map[string]*chanCall
	dataPredecessors    map[string][]string
	controlPredecessors map[string][]string
	successors          map[string][]string
	startNodes          []string
	endNodes            []string
}

func newGraph(inputType, outputType string) *graph {
	return &graph{
		nodes:               make(map[string]*graphNode),
		controlEdges:        make(map[string][]string),
		dataEdges:           make(map[string][]string),
		branches:            make(map[string][]*GraphBranch),
		inputType:           inputType,
		outputType:          outputType,
		chanSubscribeTo:     make(map[string]*chanCall),
		dataPredecessors:    make(map[string][]string),
		controlPredecessors: make(map[string][]string),
		successors:          make(map[string][]string),
	}
}

func (g *graph) checkCompiled() error {
	if g.compiled {
		return ErrGraphCompiled
	}
	return nil
}

func (g *graph) AddNode(key string, node *graphNode) error {
	if err := g.checkCompiled(); err != nil {
		return err
	}
	g.nodes[key] = node
	return nil
}

func (g *graph) AddLambdaNode(key string, lambda *Lambda) error {
	if err := g.checkCompiled(); err != nil {
		return err
	}
	g.nodes[key] = &graphNode{
		name: key,
		cr:   lambda.GetRunnable(),
		info: &GraphNodeInfo{Name: key, Component: lambda.GetComponentType()},
	}
	return nil
}

func (g *graph) AddEdge(from, to string) error {
	if err := g.checkCompiled(); err != nil {
		return err
	}

	if _, ok := g.nodes[from]; !ok && from != START {
		return fmt.Errorf("%w: %s", ErrNodeNotFound, from)
	}
	if _, ok := g.nodes[to]; !ok && to != END {
		return fmt.Errorf("%w: %s", ErrNodeNotFound, to)
	}

	g.dataEdges[from] = append(g.dataEdges[from], to)

	return nil
}

func (g *graph) AddControlEdge(from, to string) error {
	if err := g.checkCompiled(); err != nil {
		return err
	}

	if _, ok := g.nodes[from]; !ok && from != START {
		return fmt.Errorf("%w: %s", ErrNodeNotFound, from)
	}
	if _, ok := g.nodes[to]; !ok && to != END {
		return fmt.Errorf("%w: %s", ErrNodeNotFound, to)
	}

	g.controlEdges[from] = append(g.controlEdges[from], to)

	return nil
}

func (g *graph) AddBranch(key string, branch *GraphBranch) error {
	if err := g.checkCompiled(); err != nil {
		return err
	}
	g.branches[key] = append(g.branches[key], branch)
	return nil
}

func (g *graph) compile(ctx context.Context) (*runner, error) {
	if g.compiled {
		for _, call := range g.chanSubscribeTo {
			if call != nil && call.action != nil {
				return newRunnerFromGraph(g), nil
			}
		}
	}

	g.populateGraphInfo()

	var compileOpts *graphCompileOptions
	compileOpts = newNodeCompileOptions()

	runT := runTypePregel
	if g.graphInfo != nil && g.graphInfo.TriggerMode == AllPredecessor {
		runT = runTypeDAG
	}
	if compileOpts != nil && compileOpts.isDAG() {
		runT = runTypeDAG
	}

	for key, node := range g.nodes {
		cr, err := node.compileIfNeeded(ctx, compileOpts)
		if err != nil {
			return nil, fmt.Errorf("compile node %s: %w", key, err)
		}
		g.chanSubscribeTo[key] = &chanCall{
			nodeKey:  key,
			action:   cr,
			writeTo:  make(map[string]bool),
			controls: make(map[string]bool),
		}
	}

	g.chanSubscribeTo[START] = &chanCall{
		nodeKey:  START,
		writeTo:  make(map[string]bool),
		controls: make(map[string]bool),
	}

	g.chanSubscribeTo[END] = &chanCall{
		nodeKey:  END,
		writeTo:  make(map[string]bool),
		controls: make(map[string]bool),
	}

	for from, targets := range g.dataEdges {
		fromCall, ok := g.chanSubscribeTo[from]
		if !ok {
			continue
		}
		for _, to := range targets {
			fromCall.writeTo[to] = true
			g.dataPredecessors[to] = append(g.dataPredecessors[to], from)
			g.successors[from] = append(g.successors[from], to)
		}
	}

	for from, targets := range g.controlEdges {
		fromCall, ok := g.chanSubscribeTo[from]
		if !ok {
			continue
		}
		for _, to := range targets {
			fromCall.controls[to] = true
			g.controlPredecessors[to] = append(g.controlPredecessors[to], from)
			g.successors[from] = append(g.successors[from], to)
		}
	}

	for startTarget := range g.chanSubscribeTo[START].writeTo {
		g.startNodes = append(g.startNodes, startTarget)
	}
	for startTarget := range g.chanSubscribeTo[START].controls {
		if !containsString(g.startNodes, startTarget) {
			g.startNodes = append(g.startNodes, startTarget)
		}
	}

	for nodeKey := range g.chanSubscribeTo {
		if nodeKey == START || nodeKey == END {
			continue
		}
		call := g.chanSubscribeTo[nodeKey]
		if _, ok := call.writeTo[END]; ok {
			g.endNodes = append(g.endNodes, nodeKey)
		}
		if _, ok := call.controls[END]; ok {
			if !containsString(g.endNodes, nodeKey) {
				g.endNodes = append(g.endNodes, nodeKey)
			}
		}
	}

	if len(g.startNodes) == 0 {
		return nil, ErrNoStartEdge
	}
	if len(g.endNodes) == 0 {
		return nil, ErrNoEndEdge
	}

	if runT == runTypeDAG {
		if err := g.checkDAGCycles(); err != nil {
			return nil, err
		}
	}

	isDAG := runT == runTypeDAG
	maxSt := defaultMaxSteps
	if compileOpts != nil {
		maxSt = compileOpts.maxSteps
	}
	isEager := true
	if compileOpts != nil {
		isEager = compileOpts.isEager()
	}

	r := &runner{
		chanSubscribeTo:     g.chanSubscribeTo,
		successors:          g.successors,
		dataPredecessors:    g.dataPredecessors,
		controlPredecessors: g.controlPredecessors,
		inputChannels:       g.chanSubscribeTo[START],
		startNodes:          g.startNodes,
		endNodes:            g.endNodes,
		dag:                 isDAG,
		pregel:              !isDAG,
		eager:               isEager,
		maxSteps:            maxSt,
		graphName:           g.graphName,
		graphInfo:           g.graphInfo,
	}

	g.compiled = true
	return r, nil
}

func (r *runner) toComposableRunnable() *composableRunnable {
	return &composableRunnable{
		i: func(ctx context.Context, input any) (any, error) {
			return r.run(ctx, input)
		},
	}
}

func newRunnerFromGraph(g *graph) *runner {
	return &runner{
		chanSubscribeTo:     g.chanSubscribeTo,
		successors:          g.successors,
		dataPredecessors:    g.dataPredecessors,
		controlPredecessors: g.controlPredecessors,
		inputChannels:       g.chanSubscribeTo[START],
		startNodes:          g.startNodes,
		endNodes:            g.endNodes,
		dag:                 g.graphInfo != nil && g.graphInfo.DAGMode,
		pregel:              g.graphInfo != nil && g.graphInfo.PregelMode,
		maxSteps:            defaultMaxSteps,
		graphName:           g.graphName,
		graphInfo:           g.graphInfo,
	}
}

func (g *graph) checkDAGCycles() error {
	inDegree := make(map[string]int)
	for key := range g.chanSubscribeTo {
		inDegree[key] = 0
	}

	edges := make(map[string][]string)
	for from, targets := range g.dataEdges {
		for _, to := range targets {
			inDegree[to]++
			edges[from] = append(edges[from], to)
		}
	}
	for from, targets := range g.controlEdges {
		for _, to := range targets {
			inDegree[to]++
			edges[from] = append(edges[from], to)
		}
	}

	queue := make([]string, 0)
	for node, degree := range inDegree {
		if degree == 0 {
			queue = append(queue, node)
		}
	}

	visited := 0
	for len(queue) > 0 {
		node := queue[0]
		queue = queue[1:]
		visited++

		for _, neighbor := range edges[node] {
			inDegree[neighbor]--
			if inDegree[neighbor] == 0 {
				queue = append(queue, neighbor)
			}
		}
	}

	totalNodes := len(inDegree)
	if visited < totalNodes {
		cycleNodes := make([]string, 0)
		for node, degree := range inDegree {
			if degree > 0 {
				cycleNodes = append(cycleNodes, node)
			}
		}
		return fmt.Errorf("%w: %v", ErrDAGHasCycle, cycleNodes)
	}

	return nil
}

func (g *graph) populateGraphInfo() {
	if g.graphInfo == nil {
		return
	}
	for key, node := range g.nodes {
		var compType ComponentType
		if node.g != nil {
			compType = ComponentOfGraph
		} else {
			compType = ComponentOfLambda
		}
		g.graphInfo.addNode(key, compType)
	}
	for from, targets := range g.dataEdges {
		for _, to := range targets {
			g.graphInfo.addEdge(from, to)
		}
	}
	for from, targets := range g.controlEdges {
		for _, to := range targets {
			g.graphInfo.addEdge(from, to)
		}
	}
}

func containsString(slice []string, s string) bool {
	for _, item := range slice {
		if item == s {
			return true
		}
	}
	return false
}
