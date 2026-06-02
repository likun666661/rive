package compose

type GraphNodeInfo struct {
	Name        string        `json:"name"`
	Component   ComponentType `json:"component"`
	InputType   string        `json:"input_type,omitempty"`
	OutputType  string        `json:"output_type,omitempty"`
	HasSubGraph bool          `json:"has_sub_graph,omitempty"`
}

type GraphEdgeInfo struct {
	From string `json:"from"`
	To   string `json:"to"`
}

type GraphInfo struct {
	Name        string          `json:"name"`
	InputType   string          `json:"input_type"`
	OutputType  string          `json:"output_type"`
	Nodes       []GraphNodeInfo `json:"nodes"`
	Edges       []GraphEdgeInfo `json:"edges"`
	TriggerMode NodeTriggerMode `json:"trigger_mode"`
	DAGMode     bool            `json:"dag_mode"`
	PregelMode  bool            `json:"pregel_mode"`
	MaxSteps    int             `json:"max_steps,omitempty"`
	NumNodes    int             `json:"num_nodes"`
	NumEdges    int             `json:"num_edges"`
}

func newGraphInfo(name string, triggerMode NodeTriggerMode, maxSteps int) *GraphInfo {
	return &GraphInfo{
		Name:        name,
		Nodes:       make([]GraphNodeInfo, 0),
		Edges:       make([]GraphEdgeInfo, 0),
		TriggerMode: triggerMode,
		DAGMode:     triggerMode == AllPredecessor,
		PregelMode:  triggerMode == AnyPredecessor,
		MaxSteps:    maxSteps,
	}
}

func (gi *GraphInfo) addNode(name string, component ComponentType) {
	gi.Nodes = append(gi.Nodes, GraphNodeInfo{Name: name, Component: component})
	gi.NumNodes++
}

func (gi *GraphInfo) addEdge(from, to string) {
	gi.Edges = append(gi.Edges, GraphEdgeInfo{From: from, To: to})
	gi.NumEdges++
}
