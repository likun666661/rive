package compose

import (
	"context"
	"fmt"
	"reflect"
)

type genLocalStateKey struct {
	typ reflect.Type
}

type toolCallIDCtxKey struct{}

// WithGenLocalState returns a CompileOption that injects a per-run state factory.
// The factory is called once at graph start; the returned state is stored in context.
func WithGenLocalState[T any](fn func(ctx context.Context) *T) CompileOption {
	return func(o *graphCompileOptions) {
		var zero T
		o.genLocalState = &genLocalStateEntry{
			factory: func(ctx context.Context) any {
				return fn(ctx)
			},
			key: genLocalStateKey{typ: reflect.TypeOf(zero)},
		}
	}
}

// ProcessState reads and applies a mutation to the graph-local state of type T.
// Panics if no state of type T is found in context.
func ProcessState[T any](ctx context.Context, fn func(ctx context.Context, s *T) error) error {
	var zero T
	key := genLocalStateKey{typ: reflect.TypeOf(zero)}
	s, ok := ctx.Value(key).(*T)
	if !ok {
		return fmt.Errorf("ProcessState: no state of type %T found in context", zero)
	}
	return fn(ctx, s)
}

// GetState retrieves the graph-local state without modifying it.
func GetState[T any](ctx context.Context) (*T, bool) {
	var zero T
	key := genLocalStateKey{typ: reflect.TypeOf(zero)}
	s, ok := ctx.Value(key).(*T)
	return s, ok
}

// SetToolCallID stores the current tool call ID in context.
func SetToolCallID(ctx context.Context, callID string) context.Context {
	return context.WithValue(ctx, toolCallIDCtxKey{}, callID)
}

// GetToolCallID retrieves the current tool call ID from context.
func GetToolCallID(ctx context.Context) string {
	if id, ok := ctx.Value(toolCallIDCtxKey{}).(string); ok {
		return id
	}
	return ""
}

// NodeOption configures a node added to a graph.
type NodeOption func(*nodeOptionState)

type nodeOptionState struct {
	inputPreHandlers []func(ctx context.Context, input any) (any, error)
}

// WithNodePreHandler returns a NodeOption that registers an input-transforming
// pre-handler on the node. The handler runs before the node action.
func WithNodePreHandler(fn func(ctx context.Context, input any) (any, error)) NodeOption {
	return func(o *nodeOptionState) {
		o.inputPreHandlers = append(o.inputPreHandlers, fn)
	}
}

// InvokableTool is the interface for tools that can be invoked by name with JSON args.
type InvokableTool interface {
	Info(ctx context.Context) (*ToolInfo, error)
	Invoke(ctx context.Context, args string) (string, error)
}

// ToolsNodeConfig configures a tools execution node.
type ToolsNodeConfig struct {
	Tools        []InvokableTool
	ToolCallIDFn func(toolCall ToolCall) string
}

// ToolsNode executes tool calls found in incoming messages.
type ToolsNode struct {
	config ToolsNodeConfig
	cr     *composableRunnable
}

// NewToolsNode creates a tools execution node.
func NewToolsNode(config ToolsNodeConfig) *ToolsNode {
	tn := &ToolsNode{config: config}
	toolMap := make(map[string]InvokableTool, len(config.Tools))
	for _, t := range config.Tools {
		info, _ := t.Info(context.Background())
		if info != nil {
			toolMap[info.Name] = t
		}
	}
	tn.cr = &composableRunnable{
		i: func(ctx context.Context, input any) (any, error) {
			msg, ok := input.(*Message)
			if !ok {
				return nil, fmt.Errorf("ToolsNode: expected *Message input, got %T", input)
			}
			if len(msg.ToolCalls) == 0 {
				return []*Message{msg}, nil
			}
			results := make([]*Message, 0, len(msg.ToolCalls))
			for _, tc := range msg.ToolCalls {
				t, ok := toolMap[tc.Function.Name]
				if !ok {
					return nil, fmt.Errorf("ToolsNode: tool not found: %s", tc.Function.Name)
				}
				callCtx := ctx
				if config.ToolCallIDFn != nil {
					callCtx = SetToolCallID(ctx, config.ToolCallIDFn(tc))
				} else {
					callCtx = SetToolCallID(ctx, tc.ID)
				}
				result, err := t.Invoke(callCtx, tc.Function.Arguments)
				if err != nil {
					return nil, fmt.Errorf("ToolsNode: %s: %w", tc.Function.Name, err)
				}
				results = append(results, ToolMessage(result, tc.ID))
			}
			return results, nil
		},
	}
	return tn
}

// GetRunnable returns the composable runnable for graph integration.
func (tn *ToolsNode) GetRunnable() *composableRunnable {
	return tn.cr
}
