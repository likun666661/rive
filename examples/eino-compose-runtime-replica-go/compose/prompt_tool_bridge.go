package compose

import (
	"context"
	"encoding/json"
	"fmt"
)

// =============================================================================
// I3 Tool Bridge Adapters — Tool Calling Pipeline
//
// Extends the Bridge Adapter pattern (bridge.go) with tool-calling support:
//
//   BridgeTool           — domain interface for executable tools
//   promptTemplateBridge — wraps MessageTemplate as a Lambda (map[string]any → []*Message)
//   toolsNodeBridge      — executes tools for incoming ToolCalls (*Message → *Message)
//
// Pipeline (deterministic, no external model calls):
//   PromptTemplate → FakeChatModel (returns ToolCall) → ToolsNode → optional final model
// =============================================================================

// BridgeTool is the domain interface for a tool that can be executed
// by the ToolsNode bridge adapter.
type BridgeTool interface {
	Name() string
	Execute(ctx context.Context, args map[string]any) (string, error)
}

// BridgeToolFunc wraps a function as a BridgeTool.
type BridgeToolFunc struct {
	name    string
	execute func(ctx context.Context, args map[string]any) (string, error)
}

func NewBridgeTool(name string, fn func(ctx context.Context, args map[string]any) (string, error)) *BridgeToolFunc {
	return &BridgeToolFunc{name: name, execute: fn}
}

func (t *BridgeToolFunc) Name() string { return t.name }

func (t *BridgeToolFunc) Execute(ctx context.Context, args map[string]any) (string, error) {
	return t.execute(ctx, args)
}

// promptTemplateBridge wraps a MessageTemplate as a Lambda.
// Input:  map[string]any — template variables.
// Output: []*Message — formatted messages (system + human).
type promptTemplateBridge struct {
	tmpl *MessageTemplate
}

func (b *promptTemplateBridge) toLambda() *Lambda {
	return InvokableLambda(func(ctx context.Context, vs map[string]any) ([]*Message, error) {
		return b.tmpl.Format(ctx, vs)
	})
}

// toolsNodeBridge wraps a set of BridgeTool values as a Lambda.
// Input:  *Message — incoming message that may contain ToolCalls.
// Output: *Message — original message with tool result messages appended
// to a ToolMessages slice embedded in Content, and a summary assembled
// from tool results.
//
// Each ToolCall in msg.ToolCalls is dispatched to the matching tool by name.
// Tool result messages (Role=Tool) are appended to the returned messages.
// The Content field of the result message summarises all tool outputs.
type toolsNodeBridge struct {
	tools map[string]BridgeTool
}

func (b *toolsNodeBridge) toLambda() *Lambda {
	return InvokableLambda(func(ctx context.Context, msg *Message) (*Message, error) {
		if len(msg.ToolCalls) == 0 {
			return msg, nil
		}

		var results []string
		for _, tc := range msg.ToolCalls {
			tool, ok := b.tools[tc.Function.Name]
			if !ok {
				return nil, fmt.Errorf("tools node: tool not found: %s", tc.Function.Name)
			}
			var args map[string]any
			if err := json.Unmarshal([]byte(tc.Function.Arguments), &args); err != nil {
				return nil, fmt.Errorf("tools node: %s: invalid arguments: %w", tc.Function.Name, err)
			}
			result, err := tool.Execute(ctx, args)
			if err != nil {
				return nil, fmt.Errorf("tools node: %s: %w", tc.Function.Name, err)
			}
			results = append(results, fmt.Sprintf("%s(%v): %s", tc.Function.Name, args, result))
		}

		summary := "Tool results:\n"
		for _, r := range results {
			summary += "- " + r + "\n"
		}

		return &Message{
			Role:    Assistant,
			Content: summary,
		}, nil
	})
}

// NewPromptTemplateLambda creates a Lambda from a MessageTemplate.
// Input:  map[string]any — template variables.
// Output: []*Message — formatted prompt messages.
func NewPromptTemplateLambda(tmpl *MessageTemplate) *Lambda {
	return (&promptTemplateBridge{tmpl: tmpl}).toLambda()
}

// NewToolsNodeLambda creates a Lambda that executes tools based on
// ToolCalls found in incoming *Message values.
// Input:  *Message (may contain ToolCalls).
// Output: *Message (tool results assembled into Content).
func NewToolsNodeLambda(tools ...BridgeTool) *Lambda {
	toolMap := make(map[string]BridgeTool, len(tools))
	for _, t := range tools {
		toolMap[t.Name()] = t
	}
	return (&toolsNodeBridge{tools: toolMap}).toLambda()
}

// NewToolsNodeLambdaFromMap creates a Lambda from a pre-built tool map.
func NewToolsNodeLambdaFromMap(tools map[string]BridgeTool) *Lambda {
	return (&toolsNodeBridge{tools: tools}).toLambda()
}

// AsPromptTemplateNode creates a WorkflowNode from a MessageTemplate.
// The node expects map[string]any input (template variables) and
// outputs []*Message (formatted prompt messages).
func (wf *Workflow[I, O]) AsPromptTemplateNode(key string, tmpl *MessageTemplate) *WorkflowNode {
	return wf.AddLambdaNode(key, NewPromptTemplateLambda(tmpl))
}

// AsToolsNode creates a WorkflowNode that executes tools based on
// ToolCalls found in incoming *Message values.
// Input:  *Message (may contain ToolCalls).
// Output: *Message (tool results assembled into Content).
func (wf *Workflow[I, O]) AsToolsNode(key string, tools ...BridgeTool) *WorkflowNode {
	return wf.AddLambdaNode(key, NewToolsNodeLambda(tools...))
}
