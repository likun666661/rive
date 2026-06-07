package agent

import (
	"context"

	"github.com/rive/eino-compose-runtime-replica-go/compose"
)

// AgentConfig configures a ReAct agent graph builder.
type AgentConfig struct {
	ChatModel             compose.ChatModel
	ToolsConfig           compose.ToolsNodeConfig
	MaxStep               int
	MessageRewriter       MessageRewriter
	MessageModifier       MessageModifier
	StreamToolCallChecker StreamToolCallChecker
	ToolReturnDirectly    map[string]bool
}

// MessageRewriter modifies state.Messages in-place (persistent across rounds).
type MessageRewriter func(ctx context.Context, msgs []*compose.Message) []*compose.Message

// MessageModifier receives a copy of state.Messages and returns modified messages
// for the current round only (non-persistent).
type MessageModifier func(ctx context.Context, msgs []*compose.Message) []*compose.Message

// StreamToolCallChecker reads the entire stream and returns whether ANY chunk contains a tool call.
type StreamToolCallChecker func(ctx context.Context, sr compose.StreamReader[*compose.Message]) (bool, error)

// reactState is the per-run graph-local state for a ReAct agent.
type reactState struct {
	Messages                 []*compose.Message
	ReturnDirectlyToolCallID string
}

// Agent is the compiled ReAct agent (wraps the graph Runnable).
type Agent struct {
	Runnable compose.Runnable[[]*compose.Message, *compose.Message]
	Graph    *compose.Graph[[]*compose.Message, *compose.Message]
}

// Generate is a convenience method for invoking the agent.
func (a *Agent) Generate(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
	return a.Runnable.Invoke(ctx, input)
}
