package compose

import (
	"context"
	"fmt"
	"io"
	"sync"
)

type RoleType string

const (
	System    RoleType = "system"
	Human     RoleType = "human"
	Assistant RoleType = "assistant"
	Tool      RoleType = "tool"
)

// Message is the canonical chat message type.
//
// Role-driven model: Assistant carries ToolCalls, Tool carries ToolCallID.
// Multi-modal content goes in UserInputMultiContent / AssistantGenMultiContent.
// Provider-specific metadata lives in ResponseMeta with typed extension slots.
type Message struct {
	Role                     RoleType
	Content                  string
	ToolCalls                []ToolCall
	ToolCallID               string
	Name                     string
	ToolName                 string
	UserInputMultiContent    []MessageInputPart
	AssistantGenMultiContent []MessageOutputPart
	ResponseMeta             *ResponseMeta
	ReasoningContent         string
	Extra                    map[string]any
}

// ResponseMeta carries completion metadata and typed provider extension slots.
type ResponseMeta struct {
	ID              string
	Model           string
	FinishReason    string
	Usage           *TokenUsage
	LogProbs        *LogProbs
	OpenAIExtension *OpenAIRespMetaExtension
	GeminiExtension *GeminiRespMetaExtension
	ClaudeExtension *ClaudeRespMetaExtension
	Extension       any
}

// TokenUsage records token consumption for a single completion.
type TokenUsage struct {
	PromptTokens     int
	CompletionTokens int
	TotalTokens      int
	ReasoningTokens  int
}

// LogProbs gathers per-token log probabilities.
type LogProbs struct {
	Content []*LogProbInfo
}

type LogProbInfo struct {
	Token       string
	LogProb     float64
	Bytes       []int32
	TopLogProbs map[string]float64
}

// MessageInputPart is a tagged union for user/external multi-modal input parts.
type MessageInputPart struct {
	Type             ChatMessagePartType
	Text             *string
	Image            *MessageInputImage
	Audio            *MessageInputAudio
	Video            *MessageInputVideo
	File             *MessageInputFile
	ToolSearchResult *ToolSearchResult
}

type MessageInputImage struct {
	URL    string
	Detail string
}

type MessageInputAudio struct {
	URL string
}

type MessageInputVideo struct {
	URL string
}

type MessageInputFile struct {
	URL string
}

// MessageOutputPart is a tagged union for model multi-modal output parts.
type MessageOutputPart struct {
	Type      ChatMessagePartType
	Text      *string
	Image     *MessageOutputImage
	Audio     *MessageOutputAudio
	Video     *MessageOutputVideo
	Reasoning *string
}

type MessageOutputImage struct {
	URL    string
	Data   []byte
	Format string
}

type MessageOutputAudio struct {
	URL    string
	Data   []byte
	Format string
}

type MessageOutputVideo struct {
	URL    string
	Data   []byte
	Format string
}

// ToolSearchResult represents a tool-match from a search request.
type ToolSearchResult struct {
	ToolName string
	Score    float64
}

// Provider extension stubs for ResponseMeta.

type OpenAIRespMetaExtension struct {
	ID                   string
	Status               string
	PreviousResponseID   string
	IncompleteDetails    *OpenAIIncompleteDetails
	ServiceTier          string
	CreatedAt            int64
	PromptCacheRetention string
}

type OpenAIIncompleteDetails struct {
	Reason string
}

type ClaudeRespMetaExtension struct {
	ID          string
	StopReason  string
	StopDetails *ClaudeStopDetails
}

type ClaudeStopDetails struct {
	Category    string
	Explanation string
}

type GeminiRespMetaExtension struct {
	ID            string
	FinishReason  string
	GroundingMeta *GeminiGroundingMetadata
}

type GeminiGroundingMetadata struct {
	GroundingChunks   []*GeminiGroundingChunk
	GroundingSupports []*GeminiGroundingSupport
	SearchEntryPoint  *GeminiSearchEntryPoint
	WebSearchQueries  []string
}

type GeminiGroundingChunk struct {
	Web *GeminiWebSource
}

type GeminiWebSource struct {
	Title  string
	URI    string
	Domain string
}

type GeminiGroundingSupport struct {
	Segment               string
	ConfidenceScores      []float64
	GroundingChunkIndices []int32
}

type GeminiSearchEntryPoint struct {
	RenderedContent string
	SDKBlob         string
}

type ChatModel interface {
	Generate(ctx context.Context, input []*Message) (*Message, error)
	Stream(ctx context.Context, input []*Message) (StreamReader[*Message], error)
}

type FakeChatModel struct {
	mu         sync.Mutex
	generateFn func(ctx context.Context, input []*Message) (*Message, error)
	streamFn   func(ctx context.Context, input []*Message) (StreamReader[*Message], error)
}

type ChatModelOption func(*FakeChatModel)

func NewFakeChatModel(opts ...ChatModelOption) *FakeChatModel {
	m := &FakeChatModel{}
	for _, opt := range opts {
		opt(m)
	}
	if m.generateFn == nil {
		m.generateFn = func(ctx context.Context, input []*Message) (*Message, error) {
			if len(input) == 0 {
				return &Message{Role: Assistant, Content: "no input"}, nil
			}
			last := input[len(input)-1]
			return &Message{Role: Assistant, Content: "echo: " + last.Content}, nil
		}
	}
	if m.streamFn == nil {
		m.streamFn = func(ctx context.Context, input []*Message) (StreamReader[*Message], error) {
			msg, err := m.Generate(ctx, input)
			if err != nil {
				return nil, err
			}
			return &chatMessageStreamReader{msgs: []*Message{msg}}, nil
		}
	}
	return m
}

func WithChatGenerateFunc(fn func(ctx context.Context, input []*Message) (*Message, error)) ChatModelOption {
	return func(m *FakeChatModel) { m.generateFn = fn }
}

func WithChatStreamFunc(fn func(ctx context.Context, input []*Message) (StreamReader[*Message], error)) ChatModelOption {
	return func(m *FakeChatModel) { m.streamFn = fn }
}

func (m *FakeChatModel) Generate(ctx context.Context, input []*Message) (*Message, error) {
	return m.generateFn(ctx, input)
}

func (m *FakeChatModel) Stream(ctx context.Context, input []*Message) (StreamReader[*Message], error) {
	return m.streamFn(ctx, input)
}

type chatMessageStreamReader struct {
	msgs []*Message
	pos  int
}

func (r *chatMessageStreamReader) Recv() (*Message, error) {
	if r.pos >= len(r.msgs) {
		return nil, io.EOF
	}
	m := r.msgs[r.pos]
	r.pos++
	return m, nil
}

func chatMessageStreamFromSlice(msgs ...*Message) StreamReader[*Message] {
	return &chatMessageStreamReader{msgs: msgs}
}

func chatMessageStreamCollect(r StreamReader[*Message]) ([]*Message, error) {
	var msgs []*Message
	for {
		m, err := r.Recv()
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, err
		}
		msgs = append(msgs, m)
	}
	return msgs, nil
}

type ChatModelComponent struct {
	cm ChatModel
}

func NewChatModelComponent(cm ChatModel) *ChatModelComponent {
	return &ChatModelComponent{cm: cm}
}

func (c *ChatModelComponent) GetRunnable() *composableRunnable {
	return &composableRunnable{
		i: func(ctx context.Context, input any) (any, error) {
			msgs, ok := input.([]*Message)
			if !ok {
				return nil, fmt.Errorf("ChatModelComponent.Invoke: expected []*Message input, got %T", input)
			}
			return c.cm.Generate(ctx, msgs)
		},
		s: func(ctx context.Context, input any) (any, error) {
			msgs, ok := input.([]*Message)
			if !ok {
				return nil, fmt.Errorf("ChatModelComponent.Stream: expected []*Message input, got %T", input)
			}
			sr, err := c.cm.Stream(ctx, msgs)
			if err != nil {
				return nil, err
			}
			return &typedStreamWrapper[*Message]{inner: sr}, nil
		},
	}
}

func (c *ChatModelComponent) GetComponentType() ComponentType {
	return ComponentOfChatModel
}

func SystemMessage(content string) *Message {
	return &Message{Role: System, Content: content}
}

func HumanMessage(content string) *Message {
	return &Message{Role: Human, Content: content}
}

func AssistantMessage(content string) *Message {
	return &Message{Role: Assistant, Content: content}
}

func ToolMessage(content string, toolCallID string) *Message {
	return &Message{Role: Tool, Content: content, ToolCallID: toolCallID}
}

// UserMessage creates a Message with the canonical User role.
func UserMessage(content string) *Message {
	return &Message{Role: User, Content: content}
}
