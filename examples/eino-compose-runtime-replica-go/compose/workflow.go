package compose

import (
	"context"
	"fmt"
)

type dependencyType int

const (
	normalDependency   dependencyType = 1
	noDirectDependency dependencyType = 2
	branchDependency   dependencyType = 3
)

type WorkflowNode struct {
	g                *graph
	key              string
	addInputs        []func() error
	staticValues     map[string]any
	dependencySetter func(fromNodeKey string, typ dependencyType)
	mappedFieldPath  map[string]any
}

type WorkflowBranch struct {
	fromNodeKey string
	*GraphBranch
}

type Workflow[I, O any] struct {
	g                *graph
	workflowNodes    map[string]*WorkflowNode
	workflowBranches []*WorkflowBranch
	dependencies     map[string]map[string]dependencyType
}

func NewWorkflow[I, O any]() *Workflow[I, O] {
	g := newGraph(fmtType(*new(I)), fmtType(*new(O)))
	g.triggerMode = AllPredecessor

	wf := &Workflow[I, O]{
		g:             g,
		workflowNodes: make(map[string]*WorkflowNode),
		dependencies:  make(map[string]map[string]dependencyType),
	}

	return wf
}

func (wf *Workflow[I, O]) initNode(key string) *WorkflowNode {
	n := &WorkflowNode{
		g:            wf.g,
		key:          key,
		staticValues: make(map[string]any),
		dependencySetter: func(fromNodeKey string, typ dependencyType) {
			if _, ok := wf.dependencies[key]; !ok {
				wf.dependencies[key] = make(map[string]dependencyType)
			}
			wf.dependencies[key][fromNodeKey] = typ
		},
		mappedFieldPath: make(map[string]any),
	}
	wf.workflowNodes[key] = n
	return n
}

func (wf *Workflow[I, O]) AddLambdaNode(key string, lambda *Lambda) *WorkflowNode {
	_ = wf.g.AddLambdaNode(key, lambda)
	return wf.initNode(key)
}

func (wf *Workflow[I, O]) AddGraphNode(key string, subGraph *Graph[any, any]) *WorkflowNode {
	subG := subGraph.g
	gn := &graphNode{
		name: key,
		g:    subG,
		info: &GraphNodeInfo{Name: key, Component: ComponentOfGraph},
	}
	_ = wf.g.AddNode(key, gn)
	return wf.initNode(key)
}

func (wf *Workflow[I, O]) AddPassthroughNode(key string) *WorkflowNode {
	lambda := InvokableLambda(func(ctx context.Context, input any) (any, error) {
		return input, nil
	})
	_ = wf.g.AddLambdaNode(key, lambda)
	return wf.initNode(key)
}

func (wf *Workflow[I, O]) End() *WorkflowNode {
	if node, ok := wf.workflowNodes[END]; ok {
		return node
	}
	n := &WorkflowNode{
		g:            wf.g,
		key:          END,
		staticValues: make(map[string]any),
		dependencySetter: func(fromNodeKey string, typ dependencyType) {
			if _, ok := wf.dependencies[END]; !ok {
				wf.dependencies[END] = make(map[string]dependencyType)
			}
			wf.dependencies[END][fromNodeKey] = typ
		},
		mappedFieldPath: make(map[string]any),
	}
	wf.workflowNodes[END] = n
	return n
}

func (wf *Workflow[I, O]) AddBranch(fromNodeKey string, branch *GraphBranch) *WorkflowBranch {
	wb := &WorkflowBranch{
		fromNodeKey: fromNodeKey,
		GraphBranch: branch,
	}
	wf.workflowBranches = append(wf.workflowBranches, wb)
	return wb
}

func (n *WorkflowNode) AddInput(fromNodeKey string, inputs ...*FieldMapping) *WorkflowNode {
	return n.addDependencyRelation(fromNodeKey, inputs, &workflowAddInputOpts{})
}

type workflowAddInputOpts struct {
	noDirectDependency     bool
	dependencyWithoutInput bool
}

type WorkflowAddInputOpt func(*workflowAddInputOpts)

func WithNoDirectDependency() WorkflowAddInputOpt {
	return func(opt *workflowAddInputOpts) {
		opt.noDirectDependency = true
	}
}

func getAddInputOpts(opts []WorkflowAddInputOpt) *workflowAddInputOpts {
	opt := &workflowAddInputOpts{}
	for _, o := range opts {
		o(opt)
	}
	return opt
}

func (n *WorkflowNode) AddInputWithOptions(fromNodeKey string, inputs []*FieldMapping, opts ...WorkflowAddInputOpt) *WorkflowNode {
	return n.addDependencyRelation(fromNodeKey, inputs, getAddInputOpts(opts))
}

func (n *WorkflowNode) AddDependency(fromNodeKey string) *WorkflowNode {
	return n.addDependencyRelation(fromNodeKey, nil, &workflowAddInputOpts{dependencyWithoutInput: true})
}

func (n *WorkflowNode) SetStaticValue(path FieldPath, value any) *WorkflowNode {
	n.staticValues[path.join()] = value
	return n
}

func (n *WorkflowNode) addDependencyRelation(fromNodeKey string, inputs []*FieldMapping, options *workflowAddInputOpts) *WorkflowNode {
	for _, input := range inputs {
		input.fromNodeKey = fromNodeKey
	}

	if options.noDirectDependency {
		n.addInputs = append(n.addInputs, func() error {
			var paths []FieldPath
			for _, input := range inputs {
				paths = append(paths, input.TargetPath())
			}
			if err := n.checkAndAddMappedPath(paths); err != nil {
				return err
			}

			if err := n.g.addEdgeWithMappings(fromNodeKey, n.key, true, false, inputs...); err != nil {
				return err
			}
			n.dependencySetter(fromNodeKey, noDirectDependency)
			return nil
		})
	} else if options.dependencyWithoutInput {
		n.addInputs = append(n.addInputs, func() error {
			if len(inputs) > 0 {
				return fmt.Errorf("dependency without input should not have inputs. node: %s, fromNode: %s, inputs: %v", n.key, fromNodeKey, inputs)
			}
			if err := n.g.addEdgeWithMappings(fromNodeKey, n.key, false, true); err != nil {
				return err
			}
			n.dependencySetter(fromNodeKey, normalDependency)
			return nil
		})
	} else {
		n.addInputs = append(n.addInputs, func() error {
			var paths []FieldPath
			for _, input := range inputs {
				paths = append(paths, input.TargetPath())
			}
			if err := n.checkAndAddMappedPath(paths); err != nil {
				return err
			}

			if err := n.g.addEdgeWithMappings(fromNodeKey, n.key, false, false, inputs...); err != nil {
				return err
			}
			n.dependencySetter(fromNodeKey, normalDependency)
			return nil
		})
	}

	return n
}

func (n *WorkflowNode) checkAndAddMappedPath(paths []FieldPath) error {
	if v, ok := n.mappedFieldPath[""]; ok {
		if _, ok = v.(struct{}); ok {
			return fmt.Errorf("entire output has already been mapped for node: %s", n.key)
		}
	} else {
		if len(paths) == 0 {
			n.mappedFieldPath[""] = struct{}{}
			return nil
		} else {
			n.mappedFieldPath[""] = map[string]any{}
		}
	}

	for _, targetPath := range paths {
		m := n.mappedFieldPath[""].(map[string]any)
		var traversed FieldPath
		for i, path := range targetPath {
			traversed = append(traversed, path)
			if v, ok := m[path]; ok {
				if _, ok = v.(struct{}); ok {
					return fmt.Errorf("two terminal field paths conflict for node %s: %v, %v", n.key, traversed, targetPath)
				}
			}

			if i < len(targetPath)-1 {
				if _, ok := m[path]; !ok {
					m[path] = make(map[string]any)
				}
				m = m[path].(map[string]any)
			} else {
				m[path] = struct{}{}
			}
		}
	}

	return nil
}

func (wf *Workflow[I, O]) Compile(ctx context.Context) (Runnable[I, O], error) {
	return wf.compile(ctx)
}

func (wf *Workflow[I, O]) compile(ctx context.Context) (Runnable[I, O], error) {
	for _, wb := range wf.workflowBranches {
		for endNode := range wb.endNodes {
			if endNode == END {
				if _, ok := wf.dependencies[END]; !ok {
					wf.dependencies[END] = make(map[string]dependencyType)
				}
				wf.dependencies[END][wb.fromNodeKey] = branchDependency
			} else {
				n := wf.workflowNodes[endNode]
				n.dependencySetter(wb.fromNodeKey, branchDependency)
			}
		}
		_ = wf.g.addBranch(wb.fromNodeKey, wb.GraphBranch, true)
	}

	for _, n := range wf.workflowNodes {
		for _, addInput := range n.addInputs {
			if err := addInput(); err != nil {
				return nil, err
			}
		}
		n.addInputs = nil
	}

	for _, n := range wf.workflowNodes {
		if len(n.staticValues) > 0 {
			var paths []FieldPath
			for path := range n.staticValues {
				paths = append(paths, splitFieldPath(path))
			}

			if err := n.checkAndAddMappedPath(paths); err != nil {
				return nil, err
			}

			staticCopy := make(map[string]any, len(n.staticValues))
			for k, v := range n.staticValues {
				staticCopy[k] = v
			}

			pair := handlerPair{
				invoke: func(in any) (any, error) {
					return applyStaticValues(in, staticCopy)
				},
			}

			if wf.g.handlerPreNodes == nil {
				wf.g.handlerPreNodes = make(map[string][]handlerPair)
			}
			wf.g.handlerPreNodes[n.key] = append(wf.g.handlerPreNodes[n.key], pair)
		}
	}

	r, err := wf.g.compile(ctx)
	if err != nil {
		return nil, err
	}

	r.dag = true
	r.pregel = false

	cr := r.toComposableRunnable()

	return &graphRunnable[I, O]{cr: cr, runner: r}, nil
}
