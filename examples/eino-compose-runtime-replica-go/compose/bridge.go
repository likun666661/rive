package compose

import (
	"context"
	"fmt"
	"strings"
)

// =============================================================================
// I3 Bridge Adapters — 让领域组件参与通用图运行时
//
// 核心问题:
// Graph/Workflow/Chain 运行时的基本单位是 Lambda (composableRunnable),
// 但领域组件 (Retriever, ChatModel) 有其自身的接口约定。
//
// Bridge Adapter 模式:
// 为每种领域组件定义一个轻量接口 + 一个适配函数 (toLambda),
// 将领域语义包装为 Lambda,既不侵入组件自身,也不侵入图运行时。
// 组件开发者按领域接口实现,通过 Bridge 即可参加图编排。
//
// 对比 Eino:
// Eino (CloudWeGo) 的 components 包里内置了 ChatModel/Tool/Retriever
// 接口及对应的 Component 包装。本复刻版教的是 bridge 设计模式,
// 展示如何在不修改 graph runtime 的前提下接入任何领域组件。
// =============================================================================

// BridgeDocument represents a retrieved document in a RAG pipeline.
type BridgeDocument struct {
	Content  string
	Metadata map[string]string
	Score    float64
}

// BridgeMessage represents a chat message.
type BridgeMessage struct {
	Role    string // "system", "user", "assistant"
	Content string
}

// BridgeRetriever is the domain interface for document retrieval.
type BridgeRetriever interface {
	Retrieve(ctx context.Context, query string) ([]*BridgeDocument, error)
}

// BridgeChatModel is the domain interface for chat model generation.
type BridgeChatModel interface {
	Generate(ctx context.Context, messages []*BridgeMessage) (string, error)
}

// retrieverBridge wraps a domain BridgeRetriever as a Lambda.
// The Lambda accepts a query string (from direct AddInput without field mappings)
// and outputs []*BridgeDocument.
type retrieverBridge struct {
	retriever BridgeRetriever
}

func (b *retrieverBridge) toLambda() *Lambda {
	return InvokableLambda(func(ctx context.Context, query string) ([]*BridgeDocument, error) {
		return b.retriever.Retrieve(ctx, query)
	})
}

// retrieverFromMapBridge wraps a domain BridgeRetriever as a Lambda that accepts
// map[string]any (from FieldMapping outputs), extracting the query value.
// Use this variant when the retriever is connected via FieldMapping.
func retrieverFromMapBridge(retriever BridgeRetriever, queryKey string) *Lambda {
	return InvokableLambda(func(ctx context.Context, in map[string]any) ([]*BridgeDocument, error) {
		query, ok := in[queryKey].(string)
		if !ok {
			return nil, fmt.Errorf("retriever: expected string value for key %q in map input, got %T", queryKey, in[queryKey])
		}
		return retriever.Retrieve(ctx, query)
	})
}

// chatModelBridge wraps a domain BridgeChatModel as a Lambda.
// The Lambda expects input as []*BridgeMessage, and outputs a string.
type chatModelBridge struct {
	model BridgeChatModel
}

func (b *chatModelBridge) toLambda() *Lambda {
	return InvokableLambda(func(ctx context.Context, messages []*BridgeMessage) (string, error) {
		return b.model.Generate(ctx, messages)
	})
}

// promptAssemblerBridge wraps a prompt assembly function as a Lambda.
// Input is map[string]any with "query" (string) and "documents" ([]*BridgeDocument) keys.
// Output is []*BridgeMessage with system prompt + context-augmented user message.
type promptAssemblerBridge struct {
	systemPrompt string
}

func (b *promptAssemblerBridge) toLambda() *Lambda {
	return InvokableLambda(func(ctx context.Context, in map[string]any) ([]*BridgeMessage, error) {
		query, _ := in["query"].(string)
		docs, _ := in["documents"].([]*BridgeDocument)

		var contextParts []string
		for _, doc := range docs {
			contextParts = append(contextParts, fmt.Sprintf("- %s", doc.Content))
		}
		contextBlock := strings.Join(contextParts, "\n")

		return []*BridgeMessage{
			{Role: "system", Content: b.systemPrompt},
			{Role: "user", Content: fmt.Sprintf(
				"Context:\n%s\n\nQuestion: %s",
				contextBlock,
				query,
			)},
		}, nil
	})
}

// AsRetrieverNode creates a WorkflowNode from a domain BridgeRetriever using a bridge adapter.
// The node expects input as a string (query), suitable for AddInput without field mappings.
func (wf *Workflow[I, O]) AsRetrieverNode(key string, retriever BridgeRetriever) *WorkflowNode {
	return wf.AddLambdaNode(key, (&retrieverBridge{retriever: retriever}).toLambda())
}

// AsChatModelNode creates a WorkflowNode from a domain BridgeChatModel using a bridge adapter.
// The node expects input as []*BridgeMessage and outputs a string.
func (wf *Workflow[I, O]) AsChatModelNode(key string, model BridgeChatModel) *WorkflowNode {
	return wf.AddLambdaNode(key, (&chatModelBridge{model: model}).toLambda())
}

// AsPromptAssemblerNode creates a WorkflowNode that assembles prompt messages
// from retriever output and the user query. Input is map[string]any with
// "query" and "documents" keys (from FieldMapping fan-in). Output is []*BridgeMessage.
func (wf *Workflow[I, O]) AsPromptAssemblerNode(key string, systemPrompt string) *WorkflowNode {
	return wf.AddLambdaNode(key, (&promptAssemblerBridge{systemPrompt: systemPrompt}).toLambda())
}
