package compose

import (
	"encoding/json"
	"fmt"
)

type GeminiPart struct {
	Text             string                  `json:"text,omitempty"`
	InlineData       *GeminiInlineData       `json:"inlineData,omitempty"`
	FunctionCall     *GeminiFunctionCall     `json:"functionCall,omitempty"`
	FunctionResponse *GeminiFunctionResponse `json:"functionResponse,omitempty"`
}

type GeminiInlineData struct {
	MimeType string `json:"mimeType"`
	Data     string `json:"data"`
}

type GeminiFunctionCall struct {
	Name string         `json:"name"`
	Args map[string]any `json:"args"`
}

type GeminiFunctionResponse struct {
	Name     string         `json:"name"`
	Response map[string]any `json:"response"`
}

type GeminiContent struct {
	Role  string        `json:"role"`
	Parts []*GeminiPart `json:"parts"`
}

type GeminiChatRequest struct {
	Contents []*GeminiContent `json:"contents"`
}

func geminiRoleToAgentic(role string) AgenticRoleType {
	switch role {
	case "user":
		return AgenticRoleUser
	case "model":
		return AgenticRoleAssistant
	case "function":
		return AgenticRoleUser
	default:
		return AgenticRoleType(role)
	}
}

func agenticRoleToGemini(role AgenticRoleType) string {
	switch role {
	case AgenticRoleSystem:
		return "user"
	case AgenticRoleUser:
		return "user"
	case AgenticRoleAssistant:
		return "model"
	default:
		return "user"
	}
}

func geminiRoleToMessage(role string) RoleType {
	switch role {
	case "user":
		return User
	case "model":
		return Assistant
	case "function":
		return Tool
	default:
		return RoleType(role)
	}
}

func messageRoleToGemini(role RoleType) string {
	switch role {
	case System:
		return "user"
	case Human, User:
		return "user"
	case Assistant:
		return "model"
	case Tool:
		return "function"
	default:
		return "user"
	}
}

func ToCanonicalAgenticMessagesFromGemini(req *GeminiChatRequest) []*AgenticMessage {
	if req == nil {
		return nil
	}
	msgs := make([]*AgenticMessage, 0, len(req.Contents))
	for _, gc := range req.Contents {
		role := geminiRoleToAgentic(gc.Role)
		blocks := make([]*ContentBlock, 0, len(gc.Parts))
		for _, p := range gc.Parts {
			blocks = append(blocks, geminiPartToCanonical(p))
		}
		msgs = append(msgs, &AgenticMessage{Role: role, ContentBlocks: blocks})
	}
	return msgs
}

func geminiPartToCanonical(p *GeminiPart) *ContentBlock {
	if p.InlineData != nil {
		return NewImageContentBlock("")
	}
	if p.FunctionCall != nil {
		argsJSON, _ := json.Marshal(p.FunctionCall.Args)
		return NewToolCallContentBlock(
			fmt.Sprintf("call_%s", p.FunctionCall.Name),
			p.FunctionCall.Name,
			string(argsJSON),
		)
	}
	if p.FunctionResponse != nil {
		respJSON, _ := json.Marshal(p.FunctionResponse.Response)
		return NewToolResultContentBlock(p.FunctionResponse.Name, string(respJSON))
	}
	return NewTextContentBlock(p.Text)
}

func FromCanonicalAgenticMessagesToGemini(msgs []*AgenticMessage) *GeminiChatRequest {
	contents := make([]*GeminiContent, 0, len(msgs))
	for _, am := range msgs {
		parts := make([]*GeminiPart, 0, len(am.ContentBlocks))
		for _, cb := range am.ContentBlocks {
			parts = append(parts, canonicalBlockToGeminiPart(cb))
		}
		contents = append(contents, &GeminiContent{
			Role:  agenticRoleToGemini(am.Role),
			Parts: parts,
		})
	}
	return &GeminiChatRequest{Contents: contents}
}

func canonicalBlockToGeminiPart(cb *ContentBlock) *GeminiPart {
	switch {
	case cb.UserInputText != nil:
		return &GeminiPart{Text: *cb.UserInputText}
	case cb.AssistantGenText != nil:
		return &GeminiPart{Text: cb.AssistantGenText.Content}
	case cb.UserInputImage != nil:
		return &GeminiPart{InlineData: &GeminiInlineData{MimeType: "image/png", Data: cb.UserInputImage.URL}}
	case cb.FunctionToolCall != nil:
		var args map[string]any
		_ = json.Unmarshal([]byte(cb.FunctionToolCall.Arguments), &args)
		return &GeminiPart{FunctionCall: &GeminiFunctionCall{Name: cb.FunctionToolCall.Name, Args: args}}
	case cb.ToolResult != nil:
		var resp map[string]any
		_ = json.Unmarshal([]byte(cb.ToolResult.Output), &resp)
		return &GeminiPart{FunctionResponse: &GeminiFunctionResponse{Name: cb.ToolResult.CallID, Response: resp}}
	default:
		return &GeminiPart{Text: ""}
	}
}

func ToCanonicalMessagesFromGemini(req *GeminiChatRequest) []*Message {
	if req == nil {
		return nil
	}
	msgs := make([]*Message, 0, len(req.Contents))
	for _, gc := range req.Contents {
		role := geminiRoleToMessage(gc.Role)
		var textContent string
		var toolCalls []ToolCall
		var toolCallID string
		for i, p := range gc.Parts {
			if p.InlineData != nil {
				textContent += fmt.Sprintf("[image: %s]", p.InlineData.MimeType)
				continue
			}
			if p.FunctionCall != nil {
				argsJSON, _ := json.Marshal(p.FunctionCall.Args)
				toolCalls = append(toolCalls, ToolCall{
					ID:   fmt.Sprintf("call_%s_%d", p.FunctionCall.Name, i),
					Type: "function",
					Function: ToolCallFunction{
						Name:      p.FunctionCall.Name,
						Arguments: string(argsJSON),
					},
				})
				continue
			}
			if p.FunctionResponse != nil {
				respJSON, _ := json.Marshal(p.FunctionResponse.Response)
				textContent += string(respJSON)
				toolCallID = p.FunctionResponse.Name
				continue
			}
			textContent += p.Text
		}
		msgs = append(msgs, &Message{
			Role:       role,
			Content:    textContent,
			ToolCalls:  toolCalls,
			ToolCallID: toolCallID,
		})
	}
	return msgs
}

func FromCanonicalMessagesToGemini(msgs []*Message) *GeminiChatRequest {
	contents := make([]*GeminiContent, 0, len(msgs))
	for _, m := range msgs {
		geminiRole := messageRoleToGemini(m.Role)
		var parts []*GeminiPart
		if m.Content != "" {
			parts = append(parts, &GeminiPart{Text: m.Content})
		}
		for _, tc := range m.ToolCalls {
			var args map[string]any
			_ = json.Unmarshal([]byte(tc.Function.Arguments), &args)
			parts = append(parts, &GeminiPart{
				FunctionCall: &GeminiFunctionCall{Name: tc.Function.Name, Args: args},
			})
		}
		if m.Role == Tool && m.ToolCallID != "" {
			var resp map[string]any
			_ = json.Unmarshal([]byte(m.Content), &resp)
			parts = append(parts, &GeminiPart{
				FunctionResponse: &GeminiFunctionResponse{Name: m.ToolCallID, Response: resp},
			})
		}
		if len(parts) == 0 {
			parts = append(parts, &GeminiPart{Text: ""})
		}
		contents = append(contents, &GeminiContent{Role: geminiRole, Parts: parts})
	}
	return &GeminiChatRequest{Contents: contents}
}

type FakeGeminiProvider struct{}

func (p *FakeGeminiProvider) Name() string { return "gemini" }

func (p *FakeGeminiProvider) ToCanonicalAgenticMessages(req *GeminiChatRequest) ([]*AgenticMessage, error) {
	if req == nil {
		return nil, fmt.Errorf("gemini: nil request")
	}
	return ToCanonicalAgenticMessagesFromGemini(req), nil
}

func (p *FakeGeminiProvider) FromCanonicalAgenticMessages(msgs []*AgenticMessage) (*GeminiChatRequest, error) {
	if msgs == nil {
		return nil, fmt.Errorf("gemini: nil messages")
	}
	return FromCanonicalAgenticMessagesToGemini(msgs), nil
}

func (p *FakeGeminiProvider) ToCanonicalMessages(req *GeminiChatRequest) ([]*Message, error) {
	if req == nil {
		return nil, fmt.Errorf("gemini: nil request")
	}
	return ToCanonicalMessagesFromGemini(req), nil
}

func (p *FakeGeminiProvider) FromCanonicalMessages(msgs []*Message) (*GeminiChatRequest, error) {
	if msgs == nil {
		return nil, fmt.Errorf("gemini: nil messages")
	}
	return FromCanonicalMessagesToGemini(msgs), nil
}

var _ ProviderGemini = (*FakeGeminiProvider)(nil)
