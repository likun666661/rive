package compose

import "errors"

type NodeTriggerMode string

const (
	AnyPredecessor NodeTriggerMode = "any_predecessor"
	AllPredecessor NodeTriggerMode = "all_predecessor"
)

type runType int

const (
	runTypeDAG    runType = 1
	runTypePregel runType = 2
)

type ComponentType string

const (
	ComponentOfGraph     ComponentType = "Graph"
	ComponentOfLambda    ComponentType = "Lambda"
	ComponentOfWorkflow  ComponentType = "Workflow"
	ComponentOfChain     ComponentType = "Chain"
	ComponentOfChatModel ComponentType = "ChatModel"
	ComponentOfPrompt    ComponentType = "Prompt"
	ComponentOfTool      ComponentType = "Tool"
	ComponentOfUnknown   ComponentType = "Unknown"
)

const (
	START = "start"
	END   = "end"
)

const (
	defaultMaxSteps = 100
)

var (
	ErrGraphCompiled      = errors.New("graph already compiled, cannot be modified")
	ErrGraphNotCompiled   = errors.New("graph not compiled yet")
	ErrExceedMaxSteps     = errors.New("exceeded maximum run steps")
	ErrDAGHasCycle        = errors.New("DAG graph has a cycle, cannot compile in AllPredecessor mode")
	ErrNoStartEdge        = errors.New("no edge from START")
	ErrNoEndEdge          = errors.New("no edge to END")
	ErrNodeNotFound       = errors.New("node not found")
	ErrNoCompiledRunnable = errors.New("node has no compiled runnable")
)
