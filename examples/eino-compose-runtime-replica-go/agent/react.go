package agent

import (
	"context"
	"fmt"
	"io"

	"github.com/rive/eino-compose-runtime-replica-go/compose"
)

const (
	nodeKeyModel          = "chat_model"
	nodeKeyTools          = "tools"
	nodeKeyDirectReturn   = "direct_return"
	branchKeyModelPost    = "model_post_branch"
	branchKeyReturnDirect = "return_directly_branch"
	defaultGraphName      = "ReActAgent"
)

// NewAgent builds a ReAct agent as a compose.Graph.
//
// Graph topology:
//
//	START → ChatModel
//	ChatModel ──(has tool call)──→ Tools → ChatModel (loop)
//	ChatModel ──(no tool call)───→ END
//	Tools ──(return directly)──→ direct_return lambda → END
func NewAgent(ctx context.Context, config *AgentConfig) (*Agent, error) {
	if config == nil {
		return nil, fmt.Errorf("NewAgent: config is nil")
	}
	if config.ChatModel == nil {
		return nil, fmt.Errorf("NewAgent: ChatModel is nil")
	}
	if config.MaxStep <= 0 {
		config.MaxStep = 20
	}

	g := compose.NewGraph[[]*compose.Message, *compose.Message]()

	cmc := compose.NewChatModelComponent(config.ChatModel)
	if err := g.AddChatModelNode(nodeKeyModel, cmc, compose.WithNodePreHandler(modelPreHandle(config))); err != nil {
		return nil, fmt.Errorf("NewAgent: add chat model node: %w", err)
	}

	if err := g.AddEdge(compose.START, nodeKeyModel); err != nil {
		return nil, err
	}

	tn := compose.NewToolsNode(config.ToolsConfig)
	if err := g.AddToolsNode(nodeKeyTools, tn, compose.WithNodePreHandler(toolsNodePreHandle(config))); err != nil {
		return nil, fmt.Errorf("NewAgent: add tools node: %w", err)
	}

	g.AddEdge(nodeKeyModel, compose.END)
	g.AddEdge(nodeKeyModel, nodeKeyTools)

	if err := g.AddBranch(nodeKeyModel, compose.NewGraphBranch(
		modelPostBranchCondition(config),
		map[string]bool{nodeKeyTools: true, compose.END: true},
	)); err != nil {
		return nil, fmt.Errorf("NewAgent: add model branch: %w", err)
	}

	if err := buildReturnDirectly(g, config); err != nil {
		return nil, err
	}

	compileOpts := []compose.CompileOption{
		compose.WithGraphName(defaultGraphName),
		compose.WithMaxRunSteps(config.MaxStep),
		compose.WithNodeTriggerMode(compose.AnyPredecessor),
		compose.WithGenLocalState(func(ctx context.Context) *reactState {
			return &reactState{
				Messages: make([]*compose.Message, 0),
			}
		}),
	}

	runnable, err := g.Compile(ctx, compileOpts...)
	if err != nil {
		return nil, fmt.Errorf("NewAgent: compile: %w", err)
	}

	return &Agent{Runnable: runnable, Graph: g}, nil
}

func modelPreHandle(config *AgentConfig) func(ctx context.Context, input any) (any, error) {
	return func(ctx context.Context, input any) (any, error) {
		s, ok := compose.GetState[reactState](ctx)
		if !ok {
			return nil, fmt.Errorf("modelPreHandle: state not found")
		}

		switch in := input.(type) {
		case []*compose.Message:
			s.Messages = append(s.Messages, in...)
		case *compose.Message:
			s.Messages = append(s.Messages, in)
		default:
			return nil, fmt.Errorf("modelPreHandle: unexpected input type %T", input)
		}

		if config.MessageRewriter != nil {
			s.Messages = config.MessageRewriter(ctx, s.Messages)
		}

		modified := make([]*compose.Message, len(s.Messages))
		copy(modified, s.Messages)

		if config.MessageModifier != nil {
			modified = config.MessageModifier(ctx, modified)
		}

		return modified, nil
	}
}

func toolsNodePreHandle(config *AgentConfig) func(ctx context.Context, input any) (any, error) {
	return func(ctx context.Context, input any) (any, error) {
		s, ok := compose.GetState[reactState](ctx)
		if !ok {
			return nil, fmt.Errorf("toolsNodePreHandle: state not found")
		}

		if msg, ok := input.(*compose.Message); ok {
			s.Messages = append(s.Messages, msg)

			if config.ToolReturnDirectly != nil && len(msg.ToolCalls) > 0 {
				for _, tc := range msg.ToolCalls {
					if config.ToolReturnDirectly[tc.Function.Name] {
						s.ReturnDirectlyToolCallID = tc.ID
						break
					}
				}
			}
		}

		return input, nil
	}
}

func modelPostBranchCondition(config *AgentConfig) compose.BranchCondition[*compose.Message] {
	return func(ctx context.Context, msg *compose.Message) (string, error) {
		if msg == nil {
			return compose.END, nil
		}
		if len(msg.ToolCalls) > 0 {
			return nodeKeyTools, nil
		}
		return compose.END, nil
	}
}

func buildReturnDirectly(g *compose.Graph[[]*compose.Message, *compose.Message], config *AgentConfig) error {
	drLambda := compose.InvokableLambda(func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
		s, ok := compose.GetState[reactState](ctx)
		if !ok || s.ReturnDirectlyToolCallID == "" {
			if len(input) > 0 {
				return input[len(input)-1], nil
			}
			return nil, fmt.Errorf("direct_return: no tool result found")
		}
		for _, msg := range input {
			if msg.ToolCallID == s.ReturnDirectlyToolCallID {
				return msg, nil
			}
		}
		if len(input) > 0 {
			return input[len(input)-1], nil
		}
		return nil, fmt.Errorf("direct_return: no matching tool result for call ID %s", s.ReturnDirectlyToolCallID)
	})

	if err := g.AddLambdaNode(nodeKeyDirectReturn, drLambda); err != nil {
		return fmt.Errorf("NewAgent: add direct return node: %w", err)
	}
	if err := g.AddEdge(nodeKeyTools, nodeKeyModel); err != nil {
		return err
	}
	if err := g.AddEdge(nodeKeyTools, nodeKeyDirectReturn); err != nil {
		return err
	}
	if err := g.AddEdge(nodeKeyDirectReturn, compose.END); err != nil {
		return err
	}

	if err := g.AddBranch(nodeKeyTools, compose.NewGraphBranch[[]*compose.Message](
		func(ctx context.Context, msgs []*compose.Message) (string, error) {
			s, ok := compose.GetState[reactState](ctx)
			if !ok || s.ReturnDirectlyToolCallID == "" {
				return nodeKeyModel, nil
			}
			return nodeKeyDirectReturn, nil
		},
		map[string]bool{nodeKeyModel: true, nodeKeyDirectReturn: true},
	)); err != nil {
		return fmt.Errorf("NewAgent: add direct return branch: %w", err)
	}
	return nil
}

// defaultStreamToolCallChecker implements the "first chunk" heuristic.
// Reads stream chunks: empty Content → continue; has ToolCalls → true; non-empty Content → false.
// Works for OpenAI-style providers.
func DefaultStreamToolCallChecker(ctx context.Context, sr compose.StreamReader[*compose.Message]) (bool, error) {
	for {
		msg, err := sr.Recv()
		if err == io.EOF {
			return false, nil
		}
		if err != nil {
			return false, err
		}
		if msg == nil {
			continue
		}
		if len(msg.Content) == 0 && len(msg.ToolCalls) == 0 {
			continue
		}
		if len(msg.ToolCalls) > 0 {
			return true, nil
		}
		if len(msg.Content) > 0 {
			return false, nil
		}
	}
}

// ScanAllStreamToolCallChecker scans the full stream before deciding whether a
// tool call exists. It is useful for providers that emit reasoning/text chunks
// before tool-call chunks.
func ScanAllStreamToolCallChecker(ctx context.Context, sr compose.StreamReader[*compose.Message]) (bool, error) {
	for {
		msg, err := sr.Recv()
		if err == io.EOF {
			return false, nil
		}
		if err != nil {
			return false, err
		}
		if msg != nil && len(msg.ToolCalls) > 0 {
			return true, nil
		}
	}
}

// SetReturnDirectly marks the current tool call for direct return.
// Called from within a tool implementation.
func SetReturnDirectly(ctx context.Context) error {
	callID := compose.GetToolCallID(ctx)
	return compose.ProcessState[reactState](ctx, func(ctx context.Context, s *reactState) error {
		s.ReturnDirectlyToolCallID = callID
		return nil
	})
}
