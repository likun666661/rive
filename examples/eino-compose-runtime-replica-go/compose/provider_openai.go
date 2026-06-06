package compose

import "fmt"

type OpenAIMessage struct {
	Role       string     `json:"role"`
	Content    string     `json:"content,omitempty"`
	ToolCalls  []ToolCall `json:"tool_calls,omitempty"`
	ToolCallID string     `json:"tool_call_id,omitempty"`
	Name       string     `json:"name,omitempty"`
}

type OpenAIChatRequest struct {
	Model    string           `json:"model"`
	Messages []*OpenAIMessage `json:"messages"`
}

func openAIRoleToCanonical(role string) RoleType {
	switch role {
	case "system":
		return System
	case "user":
		return User
	case "assistant":
		return Assistant
	case "tool":
		return Tool
	default:
		return RoleType(role)
	}
}

func canonicalRoleToOpenAI(role RoleType) string {
	switch role {
	case System:
		return "system"
	case Human, User:
		return "user"
	case Assistant:
		return "assistant"
	case Tool:
		return "tool"
	default:
		return string(role)
	}
}

func ToCanonicalMessages(req *OpenAIChatRequest) []*Message {
	if req == nil {
		return nil
	}
	msgs := make([]*Message, 0, len(req.Messages))
	for _, om := range req.Messages {
		msgs = append(msgs, &Message{
			Role:       openAIRoleToCanonical(om.Role),
			Content:    om.Content,
			ToolCalls:  om.ToolCalls,
			ToolCallID: om.ToolCallID,
			Name:       om.Name,
		})
	}
	return msgs
}

func FromCanonicalMessages(msgs []*Message, model string) *OpenAIChatRequest {
	omsgs := make([]*OpenAIMessage, 0, len(msgs))
	for _, m := range msgs {
		omsgs = append(omsgs, &OpenAIMessage{
			Role:       canonicalRoleToOpenAI(m.Role),
			Content:    m.Content,
			ToolCalls:  m.ToolCalls,
			ToolCallID: m.ToolCallID,
			Name:       m.Name,
		})
	}
	return &OpenAIChatRequest{Model: model, Messages: omsgs}
}

type FakeOpenAIProvider struct{}

func (p *FakeOpenAIProvider) Name() string { return "openai" }

func (p *FakeOpenAIProvider) ToCanonicalMessages(req *OpenAIChatRequest) ([]*Message, error) {
	if req == nil {
		return nil, fmt.Errorf("openai: nil request")
	}
	return ToCanonicalMessages(req), nil
}

func (p *FakeOpenAIProvider) FromCanonicalMessages(msgs []*Message) (*OpenAIChatRequest, error) {
	if msgs == nil {
		return nil, fmt.Errorf("openai: nil messages")
	}
	return FromCanonicalMessages(msgs, "gpt-4"), nil
}

var _ ProviderOpenAI = (*FakeOpenAIProvider)(nil)
