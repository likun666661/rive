package compose

type ToolCall struct {
	ID       string           `json:"id"`
	Type     string           `json:"type"`
	Function ToolCallFunction `json:"function"`
}

type ToolCallFunction struct {
	Name      string `json:"name"`
	Arguments string `json:"arguments"`
}

type ToolInfo struct {
	Name        string
	Desc        string
	ParamsOneOf *ParamsOneOf
}

type ParamsOneOf struct {
	Params map[string]*ParameterInfo
}

type ParameterInfo struct {
	Type     string
	Desc     string
	Required bool
	Enum     []string
}

type ToolResult struct {
	Text string
}
