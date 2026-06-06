package compose

import "encoding/base64"

type ContentBlockType string

const (
	ContentBlockTypeUserInputText      ContentBlockType = "user_input_text"
	ContentBlockTypeUserInputImage     ContentBlockType = "user_input_image"
	ContentBlockTypeUserInputAudio     ContentBlockType = "user_input_audio"
	ContentBlockTypeUserInputVideo     ContentBlockType = "user_input_video"
	ContentBlockTypeUserInputFile      ContentBlockType = "user_input_file"
	ContentBlockTypeAssistantGenText   ContentBlockType = "assistant_gen_text"
	ContentBlockTypeAssistantGenImage  ContentBlockType = "assistant_gen_image"
	ContentBlockTypeReasoning          ContentBlockType = "reasoning"
	ContentBlockTypeFunctionToolCall   ContentBlockType = "function_tool_call"
	ContentBlockTypeServerToolCall     ContentBlockType = "server_tool_call"
	ContentBlockTypeFunctionToolResult ContentBlockType = "function_tool_result"
	ContentBlockTypeServerToolResult   ContentBlockType = "server_tool_result"
	ContentBlockTypeToolSearchResult   ContentBlockType = "tool_search_result"
)

type ContentBlock struct {
	Type             ContentBlockType
	UserInputText    *string
	UserInputImage   *MessageInputImage
	UserInputAudio   *MessageInputAudio
	UserInputVideo   *MessageInputVideo
	UserInputFile    *MessageInputFile
	AssistantGenText *AssistantGenTextBlock
	Reasoning        *string
	FunctionToolCall *FunctionToolCallBlock
	ServerToolCall   *ServerToolCallBlock
	ToolResult       *ToolResultBlock
	ToolSearchResult *ToolSearchResult
}

type AssistantGenTextBlock struct {
	Content string
}

type FunctionToolCallBlock struct {
	CallID    string
	Name      string
	Arguments string
}

type ServerToolCallBlock struct {
	CallID string
	Name   string
	Args   map[string]any
}

type ToolResultBlock struct {
	CallID string
	Output string
}

type AgenticRoleType string

const (
	AgenticRoleAssistant AgenticRoleType = "assistant"
	AgenticRoleUser      AgenticRoleType = "user"
	AgenticRoleSystem    AgenticRoleType = "system"
)

type AgenticMessage struct {
	Role          AgenticRoleType
	ContentBlocks []*ContentBlock
}

func NewTextContentBlock(text string) *ContentBlock {
	s := text
	return &ContentBlock{Type: ContentBlockTypeUserInputText, UserInputText: &s}
}

func NewAssistantTextContentBlock(text string) *ContentBlock {
	return &ContentBlock{
		Type:             ContentBlockTypeAssistantGenText,
		AssistantGenText: &AssistantGenTextBlock{Content: text},
	}
}

func NewToolCallContentBlock(callID, name, args string) *ContentBlock {
	return &ContentBlock{
		Type:             ContentBlockTypeFunctionToolCall,
		FunctionToolCall: &FunctionToolCallBlock{CallID: callID, Name: name, Arguments: args},
	}
}

func NewToolResultContentBlock(callID, output string) *ContentBlock {
	return &ContentBlock{
		Type:       ContentBlockTypeFunctionToolResult,
		ToolResult: &ToolResultBlock{CallID: callID, Output: output},
	}
}

func NewImageContentBlock(url string) *ContentBlock {
	return &ContentBlock{
		Type:           ContentBlockTypeUserInputImage,
		UserInputImage: &MessageInputImage{URL: url},
	}
}

func NewImageContentBlockFromData(data []byte, mimeType string) *ContentBlock {
	return &ContentBlock{
		Type: ContentBlockTypeUserInputImage,
		UserInputImage: &MessageInputImage{
			URL: "data:" + mimeType + ";base64," + base64.StdEncoding.EncodeToString(data),
		},
	}
}

func NewSearchResultContentBlock(toolName string, score float64) *ContentBlock {
	return &ContentBlock{
		Type:             ContentBlockTypeToolSearchResult,
		ToolSearchResult: &ToolSearchResult{ToolName: toolName, Score: score},
	}
}

func AgenticMessageFirstText(am *AgenticMessage) string {
	for _, b := range am.ContentBlocks {
		if b.UserInputText != nil {
			return *b.UserInputText
		}
		if b.AssistantGenText != nil {
			return b.AssistantGenText.Content
		}
	}
	return ""
}

func AgenticMessageToolCalls(am *AgenticMessage) []*FunctionToolCallBlock {
	var calls []*FunctionToolCallBlock
	for _, b := range am.ContentBlocks {
		if b.FunctionToolCall != nil {
			calls = append(calls, b.FunctionToolCall)
		}
	}
	return calls
}

type ProviderOpenAI interface {
	Name() string
	ToCanonicalMessages(req *OpenAIChatRequest) ([]*Message, error)
	FromCanonicalMessages(msgs []*Message) (*OpenAIChatRequest, error)
}

type ProviderClaude interface {
	Name() string
	ToCanonicalAgenticMessages(req *ClaudeChatRequest) ([]*AgenticMessage, error)
	FromCanonicalAgenticMessages(msgs []*AgenticMessage) (*ClaudeChatRequest, error)
}

type ProviderGemini interface {
	Name() string
	ToCanonicalAgenticMessages(req *GeminiChatRequest) ([]*AgenticMessage, error)
	FromCanonicalAgenticMessages(msgs []*AgenticMessage) (*GeminiChatRequest, error)
	ToCanonicalMessages(req *GeminiChatRequest) ([]*Message, error)
	FromCanonicalMessages(msgs []*Message) (*GeminiChatRequest, error)
}
