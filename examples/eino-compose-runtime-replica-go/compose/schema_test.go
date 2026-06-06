package compose

import (
	"encoding/json"
	"reflect"
	"testing"
)

func TestToolCall_Index(t *testing.T) {
	idx := 3
	tc := ToolCall{ID: "call_1", Type: "function", Function: ToolCallFunction{Name: "search", Arguments: `{"q":"test"}`}, Index: &idx}
	if tc.Index == nil || *tc.Index != 3 {
		t.Fatalf("expected Index=3, got %v", tc.Index)
	}
}

func TestToolCall_IndexNil(t *testing.T) {
	tc := ToolCall{ID: "call_1", Type: "function", Function: ToolCallFunction{Name: "search", Arguments: `{}`}}
	if tc.Index != nil {
		t.Fatalf("expected nil Index, got %v", tc.Index)
	}
}

func TestToolCall_Extra(t *testing.T) {
	tc := ToolCall{
		ID:       "call_1",
		Type:     "function",
		Function: ToolCallFunction{Name: "search", Arguments: `{}`},
		Extra:    map[string]any{"provider": "openai", "priority": 1},
	}
	if tc.Extra["provider"] != "openai" {
		t.Fatalf("expected Extra[provider]=openai, got %v", tc.Extra["provider"])
	}
}

func TestToolCall_JsonRoundTrip(t *testing.T) {
	idx := 0
	tc := ToolCall{
		Index:    &idx,
		ID:       "call_abc",
		Type:     "function",
		Function: ToolCallFunction{Name: "get_weather", Arguments: `{"city":"Paris"}`},
		Extra:    map[string]any{"x": 1},
	}
	data, err := json.Marshal(tc)
	if err != nil {
		t.Fatal(err)
	}
	var restored ToolCall
	if err := json.Unmarshal(data, &restored); err != nil {
		t.Fatal(err)
	}
	// Index and Extra are json:"-" so not serialised
	if restored.ID != "call_abc" {
		t.Fatalf("expected ID=call_abc, got %s", restored.ID)
	}
	if restored.Function.Name != "get_weather" {
		t.Fatalf("expected Name=get_weather, got %s", restored.Function.Name)
	}
	if restored.Index != nil {
		t.Fatal("Index should be nil after JSON round-trip (json:\"-\")")
	}
	if restored.Extra != nil {
		t.Fatal("Extra should be nil after JSON round-trip (json:\"-\")")
	}
}

func TestParamsOneOf_ByParams(t *testing.T) {
	params := map[string]*ParameterInfo{
		"city": {Type: DataTypeString, Desc: "City name", Required: true},
	}
	p := NewParamsOneOfByParams(params)
	schema, err := p.ToJSONSchema()
	if err != nil {
		t.Fatal(err)
	}
	m, ok := schema.(map[string]any)
	if !ok {
		t.Fatalf("expected map[string]any, got %T", schema)
	}
	if m["type"] != "object" {
		t.Fatalf("expected type=object, got %v", m["type"])
	}
	props, ok := m["properties"].(map[string]any)
	if !ok {
		t.Fatal("expected properties map")
	}
	if props["city"] == nil {
		t.Fatal("expected city in properties")
	}
}

func TestParamsOneOf_ByJSONSchema(t *testing.T) {
	custom := map[string]any{"type": "object", "properties": map[string]any{"x": map[string]any{"type": "string"}}}
	p := NewParamsOneOfByJSONSchema(custom)
	schema, err := p.ToJSONSchema()
	if err != nil {
		t.Fatal(err)
	}
	m, ok := schema.(map[string]any)
	if !ok {
		t.Fatalf("expected map, got %T", schema)
	}
	if m["type"] != "object" {
		t.Fatalf("expected type=object, got %v", m["type"])
	}
}

func TestParamsOneOf_Empty(t *testing.T) {
	p := NewParamsOneOfByParams(map[string]*ParameterInfo{})
	schema, err := p.ToJSONSchema()
	if err != nil {
		t.Fatal(err)
	}
	m, ok := schema.(map[string]any)
	if !ok {
		t.Fatalf("expected map, got %T", schema)
	}
	if m["type"] != "object" {
		t.Fatalf("expected type=object, got %v", m["type"])
	}
}

func TestParamsOneOf_Nil(t *testing.T) {
	var p *ParamsOneOf
	schema, err := p.ToJSONSchema()
	if err != nil {
		t.Fatal(err)
	}
	if schema != nil {
		t.Fatal("expected nil schema")
	}
}

func TestParameterInfo_Nested(t *testing.T) {
	pi := &ParameterInfo{
		Type: DataTypeObject,
		Desc: "Address",
		SubParams: map[string]*ParameterInfo{
			"street": {Type: DataTypeString, Required: true},
		},
	}
	m := paramInfoToMap(pi)
	if m["type"] != "object" {
		t.Fatalf("expected type=object, got %v", m["type"])
	}
	props, ok := m["properties"].(map[string]any)
	if !ok {
		t.Fatal("expected properties map")
	}
	if props["street"] == nil {
		t.Fatal("expected street in nested properties")
	}
}

func TestParameterInfo_ArrayElem(t *testing.T) {
	pi := &ParameterInfo{
		Type:     DataTypeArray,
		Desc:     "List of names",
		ElemInfo: &ParameterInfo{Type: DataTypeString},
	}
	m := paramInfoToMap(pi)
	if m["type"] != "array" {
		t.Fatalf("expected type=array, got %v", m["type"])
	}
	items, ok := m["items"].(map[string]any)
	if !ok {
		t.Fatal("expected items map")
	}
	if items["type"] != "string" {
		t.Fatalf("expected items.type=string, got %v", items["type"])
	}
}

func TestParameterInfo_DataType(t *testing.T) {
	pi := &ParameterInfo{Type: DataTypeInteger}
	m := paramInfoToMap(pi)
	if m["type"] != "integer" {
		t.Fatalf("expected type=integer, got %v", m["type"])
	}
}

func TestParameterInfo_Enum(t *testing.T) {
	pi := &ParameterInfo{
		Type: DataTypeString,
		Enum: []string{"red", "green", "blue"},
	}
	m := paramInfoToMap(pi)
	en, ok := m["enum"].([]any)
	if !ok {
		t.Fatal("expected enum array")
	}
	if len(en) != 3 {
		t.Fatalf("expected 3 enum values, got %d", len(en))
	}
}

func TestToolResult_MultiModal(t *testing.T) {
	tr := &ToolResult{
		Text: "done",
		Images: []*ImageContent{
			{URL: "http://example.com/img.png", Format: "png"},
		},
		Audio: []*AudioContent{
			{URL: "http://example.com/audio.wav", Format: "wav"},
		},
		Video: []*VideoContent{
			{URL: "http://example.com/video.mp4", Format: "mp4"},
		},
		Files: []*FileContent{
			{URL: "http://example.com/doc.pdf", Name: "report", Type: "application/pdf"},
		},
	}
	if tr.Text != "done" {
		t.Fatalf("expected Text=done, got %s", tr.Text)
	}
	if len(tr.Images) != 1 || tr.Images[0].Format != "png" {
		t.Fatal("Images not preserved")
	}
	if len(tr.Audio) != 1 || tr.Audio[0].Format != "wav" {
		t.Fatal("Audio not preserved")
	}
	if len(tr.Video) != 1 || tr.Video[0].Format != "mp4" {
		t.Fatal("Video not preserved")
	}
	if len(tr.Files) != 1 || tr.Files[0].Name != "report" {
		t.Fatal("Files not preserved")
	}
}

func TestDocument_AllFields(t *testing.T) {
	doc := &Document{
		ID:        "doc_1",
		Content:   "Hello World",
		Metadata:  map[string]string{"source": "wiki"},
		Meta:      map[string]any{"score_float": 0.95},
		Embedding: []float64{0.1, 0.2, 0.3},
		Score:     0.95,
	}
	if doc.ID != "doc_1" {
		t.Fatalf("expected ID=doc_1, got %s", doc.ID)
	}
	if doc.Content != "Hello World" {
		t.Fatalf("expected Content=Hello World, got %s", doc.Content)
	}
	if doc.Metadata["source"] != "wiki" {
		t.Fatalf("expected Metadata[source]=wiki, got %s", doc.Metadata["source"])
	}
	if doc.Meta["score_float"] != 0.95 {
		t.Fatalf("expected Meta[score_float]=0.95, got %v", doc.Meta["score_float"])
	}
	if len(doc.Embedding) != 3 || doc.Embedding[0] != 0.1 {
		t.Fatal("Embedding not preserved")
	}
}

func TestDocument_Embedding(t *testing.T) {
	doc := &Document{
		ID:        "doc_2",
		Content:   "Vector search",
		Embedding: []float64{0.5, -0.3, 0.8},
	}
	if len(doc.Embedding) != 3 {
		t.Fatalf("expected embedding length 3, got %d", len(doc.Embedding))
	}
}

func TestToolInfo_Extra(t *testing.T) {
	ti := &ToolInfo{
		Name:  "search",
		Desc:  "Search the web",
		Extra: map[string]any{"timeout": 30},
	}
	if ti.Extra["timeout"] != 30 {
		t.Fatalf("expected Extra[timeout]=30, got %v", ti.Extra["timeout"])
	}
}

func TestImageContent(t *testing.T) {
	img := &ImageContent{URL: "http://example.com/photo.png", Format: "png", Data: []byte{1, 2, 3}}
	if img.URL != "http://example.com/photo.png" {
		t.Fatalf("URL not preserved")
	}
	if len(img.Data) != 3 {
		t.Fatal("Data not preserved")
	}
}

func TestDataTypeConstants(t *testing.T) {
	if DataTypeString != "string" {
		t.Fatalf("expected DataTypeString=string, got %s", DataTypeString)
	}
	if DataTypeInteger != "integer" {
		t.Fatalf("expected DataTypeInteger=integer, got %s", DataTypeInteger)
	}
	if DataTypeBoolean != "boolean" {
		t.Fatalf("expected DataTypeBoolean=boolean, got %s", DataTypeBoolean)
	}
	if DataTypeNumber != "number" {
		t.Fatalf("expected DataTypeNumber=number, got %s", DataTypeNumber)
	}
	if DataTypeObject != "object" {
		t.Fatalf("expected DataTypeObject=object, got %s", DataTypeObject)
	}
	if DataTypeArray != "array" {
		t.Fatalf("expected DataTypeArray=array, got %s", DataTypeArray)
	}
}

func TestParamsOneOf_RequiredField(t *testing.T) {
	params := map[string]*ParameterInfo{
		"query": {Type: DataTypeString, Desc: "Search query", Required: true},
		"limit": {Type: DataTypeInteger, Desc: "Limit", Required: false},
	}
	p := NewParamsOneOfByParams(params)
	schema, err := p.ToJSONSchema()
	if err != nil {
		t.Fatal(err)
	}
	m, ok := schema.(map[string]any)
	if !ok {
		t.Fatalf("expected map, got %T", schema)
	}
	required, ok := m["required"].([]string)
	if !ok {
		t.Fatal("expected required array")
	}
	if len(required) != 1 || required[0] != "query" {
		t.Fatalf("expected [query], got %v", required)
	}
}

func TestToolCall_String(t *testing.T) {
	idx := 1
	tc := ToolCall{
		Index:    &idx,
		ID:       "call_x",
		Type:     "function",
		Function: ToolCallFunction{Name: "echo", Arguments: `{}`},
	}
	s := tc.String()
	if s != "ToolCall{Index=1, ID=call_x, Type=function, Name=echo}" {
		t.Fatalf("unexpected String: %s", s)
	}
}

func TestParamsOneOf_SupportsConcreteType(t *testing.T) {
	pi := &ParameterInfo{Type: DataTypeInteger}
	if reflect.TypeOf(pi.Type) != reflect.TypeOf(DataTypeString) {
		t.Fatal("ParameterInfo.Type should be DataType")
	}
}
