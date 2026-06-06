package compose

import "fmt"

type ClaudeContentBlock struct {
	Type      string             `json:"type"`
	Text      string             `json:"text,omitempty"`
	Source    *ClaudeImageSource `json:"source,omitempty"`
	ID        string             `json:"id,omitempty"`
	Name      string             `json:"name,omitempty"`
	Input     interface{}        `json:"input,omitempty"`
	Content   interface{}        `json:"content,omitempty"`
	ToolUseID string             `json:"tool_use_id,omitempty"`
}

type ClaudeImageSource struct {
	Type      string `json:"type"`
	MediaType string `json:"media_type"`
	Data      string `json:"data"`
}

type ClaudeMessage struct {
	Role    string                `json:"role"`
	Content []*ClaudeContentBlock `json:"content"`
}

type ClaudeChatRequest struct {
	Model    string           `json:"model"`
	Messages []*ClaudeMessage `json:"messages"`
}

func claudeRoleToAgentic(role string) AgenticRoleType {
	switch role {
	case "user":
		return AgenticRoleUser
	case "assistant":
		return AgenticRoleAssistant
	default:
		return AgenticRoleType(role)
	}
}

func agenticRoleToClaude(role AgenticRoleType) string {
	switch role {
	case AgenticRoleSystem:
		return "user"
	case AgenticRoleUser:
		return "user"
	case AgenticRoleAssistant:
		return "assistant"
	default:
		return "user"
	}
}

func ToCanonicalAgenticMessages(req *ClaudeChatRequest) []*AgenticMessage {
	if req == nil {
		return nil
	}
	msgs := make([]*AgenticMessage, 0, len(req.Messages))
	for _, cm := range req.Messages {
		role := claudeRoleToAgentic(cm.Role)
		blocks := make([]*ContentBlock, 0, len(cm.Content))
		for _, cb := range cm.Content {
			blocks = append(blocks, claudeBlockToCanonical(cb))
		}
		msgs = append(msgs, &AgenticMessage{Role: role, ContentBlocks: blocks})
	}
	return msgs
}

func claudeBlockToCanonical(cb *ClaudeContentBlock) *ContentBlock {
	switch cb.Type {
	case "text":
		return NewTextContentBlock(cb.Text)
	case "image":
		if cb.Source != nil {
			return NewImageContentBlock(cb.Source.Data)
		}
		return NewTextContentBlock("")
	case "tool_use":
		inputStr := ""
		if cb.Input != nil {
			inputStr = fmt.Sprintf("%v", cb.Input)
		}
		return NewToolCallContentBlock(cb.ID, cb.Name, inputStr)
	case "tool_result":
		contentStr := ""
		if cb.Content != nil {
			contentStr = fmt.Sprintf("%v", cb.Content)
		}
		return NewToolResultContentBlock(cb.ToolUseID, contentStr)
	default:
		return NewTextContentBlock(fmt.Sprintf("%v", cb))
	}
}

func FromCanonicalAgenticMessages(msgs []*AgenticMessage, model string) *ClaudeChatRequest {
	cmsgs := make([]*ClaudeMessage, 0, len(msgs))
	for _, am := range msgs {
		blocks := make([]*ClaudeContentBlock, 0, len(am.ContentBlocks))
		for _, cb := range am.ContentBlocks {
			blocks = append(blocks, canonicalBlockToClaude(cb))
		}
		cmsgs = append(cmsgs, &ClaudeMessage{
			Role:    agenticRoleToClaude(am.Role),
			Content: blocks,
		})
	}
	return &ClaudeChatRequest{Model: model, Messages: cmsgs}
}

func canonicalBlockToClaude(cb *ContentBlock) *ClaudeContentBlock {
	switch {
	case cb.UserInputText != nil:
		return &ClaudeContentBlock{Type: "text", Text: *cb.UserInputText}
	case cb.AssistantGenText != nil:
		return &ClaudeContentBlock{Type: "text", Text: cb.AssistantGenText.Content}
	case cb.UserInputImage != nil:
		return &ClaudeContentBlock{Type: "image", Source: &ClaudeImageSource{Type: "url", MediaType: "image/png", Data: cb.UserInputImage.URL}}
	case cb.FunctionToolCall != nil:
		return &ClaudeContentBlock{Type: "tool_use", ID: cb.FunctionToolCall.CallID, Name: cb.FunctionToolCall.Name, Input: cb.FunctionToolCall.Arguments}
	case cb.ToolResult != nil:
		return &ClaudeContentBlock{Type: "tool_result", ToolUseID: cb.ToolResult.CallID, Content: cb.ToolResult.Output}
	default:
		return &ClaudeContentBlock{Type: "text", Text: ""}
	}
}

type FakeClaudeProvider struct{}

func (p *FakeClaudeProvider) Name() string { return "claude" }

func (p *FakeClaudeProvider) ToCanonicalAgenticMessages(req *ClaudeChatRequest) ([]*AgenticMessage, error) {
	if req == nil {
		return nil, fmt.Errorf("claude: nil request")
	}
	return ToCanonicalAgenticMessages(req), nil
}

func (p *FakeClaudeProvider) FromCanonicalAgenticMessages(msgs []*AgenticMessage) (*ClaudeChatRequest, error) {
	if msgs == nil {
		return nil, fmt.Errorf("claude: nil messages")
	}
	return FromCanonicalAgenticMessages(msgs, "claude-3-opus"), nil
}

var _ ProviderClaude = (*FakeClaudeProvider)(nil)
