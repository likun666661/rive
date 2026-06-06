package compose

import "fmt"

// ToolCall describes a model-requested tool invocation.
// In streaming mode Index identifies deltas belonging to the same logical call.
type ToolCall struct {
	Index    *int             `json:"-"`
	ID       string           `json:"id"`
	Type     string           `json:"type"`
	Function ToolCallFunction `json:"function"`
	Extra    map[string]any   `json:"-"`
}

type ToolCallFunction struct {
	Name      string `json:"name"`
	Arguments string `json:"arguments"`
}

// ToolInfo describes tool metadata for model registration.
type ToolInfo struct {
	Name        string
	Desc        string
	ParamsOneOf *ParamsOneOf
	Extra       map[string]any
}

// ParamsOneOf is a two-mode parameter schema: lightweight param tree or full JSON Schema.
type ParamsOneOf struct {
	params     map[string]*ParameterInfo
	jsonSchema any
}

// NewParamsOneOfByParams creates a lightweight parameter schema.
func NewParamsOneOfByParams(params map[string]*ParameterInfo) *ParamsOneOf {
	return &ParamsOneOf{params: params}
}

// NewParamsOneOfByJSONSchema creates a schema from a full JSON Schema value.
func NewParamsOneOfByJSONSchema(schema any) *ParamsOneOf {
	return &ParamsOneOf{jsonSchema: schema}
}

// ToJSONSchema normalises both modes into a map representation.
// For the params mode it renders the ParameterInfo tree into a map.
// For the jsonSchema mode it returns the stored schema as-is.
func (p *ParamsOneOf) ToJSONSchema() (any, error) {
	if p == nil {
		return nil, nil
	}
	if p.params != nil {
		return paramsToMap(p.params), nil
	}
	return p.jsonSchema, nil
}

func paramsToMap(params map[string]*ParameterInfo) map[string]any {
	out := map[string]any{
		"type":       "object",
		"properties": map[string]any{},
	}
	props := out["properties"].(map[string]any)
	required := make([]string, 0)
	for name, pi := range params {
		props[name] = paramInfoToMap(pi)
		if pi.Required {
			required = append(required, name)
		}
	}
	if len(required) > 0 {
		out["required"] = required
	}
	return out
}

func paramInfoToMap(pi *ParameterInfo) map[string]any {
	m := map[string]any{}
	if pi.Desc != "" {
		m["description"] = pi.Desc
	}
	if pi.Type != "" {
		m["type"] = string(pi.Type)
	}
	if len(pi.Enum) > 0 {
		m["enum"] = toAnySlice(pi.Enum)
	}
	if pi.ElemInfo != nil {
		m["items"] = paramInfoToMap(pi.ElemInfo)
	}
	if len(pi.SubParams) > 0 {
		sub := map[string]any{
			"type":       "object",
			"properties": map[string]any{},
		}
		subProps := sub["properties"].(map[string]any)
		subRequired := make([]string, 0)
		for spName, spi := range pi.SubParams {
			subProps[spName] = paramInfoToMap(spi)
			if spi.Required {
				subRequired = append(subRequired, spName)
			}
		}
		if len(subRequired) > 0 {
			sub["required"] = subRequired
		}
		return sub
	}
	return m
}

func toAnySlice(ss []string) []any {
	out := make([]any, len(ss))
	for i, s := range ss {
		out[i] = s
	}
	return out
}

// ParameterInfo describes a parameter in the lightweight schema mode.
type ParameterInfo struct {
	Type      DataType
	Desc      string
	Required  bool
	Enum      []string
	SubParams map[string]*ParameterInfo
	ElemInfo  *ParameterInfo
}

// ToolResult stores the output of a single tool execution.
type ToolResult struct {
	Text   string
	Images []*ImageContent
	Audio  []*AudioContent
	Video  []*VideoContent
	Files  []*FileContent
}

// ImageContent represents an image returned by a tool or model.
type ImageContent struct {
	URL    string
	Data   []byte
	Format string
}

// AudioContent represents audio returned by a tool or model.
type AudioContent struct {
	URL    string
	Data   []byte
	Format string
}

// VideoContent represents video returned by a tool or model.
type VideoContent struct {
	URL    string
	Data   []byte
	Format string
}

// FileContent represents a file returned by a tool or model.
type FileContent struct {
	URL  string
	Data []byte
	Name string
	Type string
}

// Document is the canonical retrieved/indexed document type.
type Document struct {
	ID        string
	Content   string
	Metadata  map[string]string
	Meta      map[string]any
	Embedding []float64
	Score     float64
}

// String returns a human-readable representation for debugging.
func (tc ToolCall) String() string {
	idx := "<nil>"
	if tc.Index != nil {
		idx = fmt.Sprintf("%d", *tc.Index)
	}
	return fmt.Sprintf("ToolCall{Index=%s, ID=%s, Type=%s, Name=%s}", idx, tc.ID, tc.Type, tc.Function.Name)
}
