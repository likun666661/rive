package compose

import "fmt"

type graphCompileOptions struct {
	graphName       string
	nodeTriggerMode NodeTriggerMode
	maxSteps        int
	eagerDisabled   bool
}

type CompileOption func(*graphCompileOptions)

func WithGraphName(name string) CompileOption {
	return func(o *graphCompileOptions) {
		o.graphName = name
	}
}

func WithNodeTriggerMode(mode NodeTriggerMode) CompileOption {
	return func(o *graphCompileOptions) {
		o.nodeTriggerMode = mode
	}
}

func WithMaxRunSteps(steps int) CompileOption {
	return func(o *graphCompileOptions) {
		o.maxSteps = steps
	}
}

func WithEagerExecutionDisabled() CompileOption {
	return func(o *graphCompileOptions) {
		o.eagerDisabled = true
	}
}

func newNodeCompileOptions(opts ...CompileOption) *graphCompileOptions {
	o := &graphCompileOptions{
		nodeTriggerMode: AnyPredecessor,
		maxSteps:        defaultMaxSteps,
	}
	for _, opt := range opts {
		opt(o)
	}
	return o
}

func (o *graphCompileOptions) isDAG() bool {
	return o.nodeTriggerMode == AllPredecessor
}

func (o *graphCompileOptions) isEager() bool {
	return !o.eagerDisabled
}

func (o *graphCompileOptions) String() string {
	return fmt.Sprintf("graphCompileOptions{name:%s mode:%s maxSteps:%d eager:%v}",
		o.graphName, o.nodeTriggerMode, o.maxSteps, o.isEager())
}
