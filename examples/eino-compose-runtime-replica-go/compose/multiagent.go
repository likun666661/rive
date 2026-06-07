package compose

import (
	"context"
	"fmt"
	"strings"
)

// Specialist describes a domain expert that the Host Multi-Agent can route to.
type Specialist struct {
	Name         string
	IntendedUse  string
	ChatModel    ChatModel
	SystemPrompt string
	Invokable    func(ctx context.Context, input []*Message) (*Message, error)
	Streamable   func(ctx context.Context, input []*Message) (StreamReader[*Message], error)
}

// Summarizer aggregates multiple specialist answers in multi-intent mode.
type Summarizer struct {
	ChatModel    ChatModel
	SystemPrompt string
}

// MultiAgentConfig configures a Host Multi-Agent.
type MultiAgentConfig struct {
	Host        ChatModel
	Specialists []*Specialist
	Summarizer  *Summarizer
	MaxStep     int
}

// multiAgentResult holds the output of the host routing + specialist execution phase.
type multiAgentResult struct {
	HostMsg      *Message
	DirectAnswer bool
	Answers      []specialistAnswer
}

type specialistAnswer struct {
	name    string
	content string
}

// NewMultiAgent builds a Host Multi-Agent as a compose.Graph.
//
// Graph topology (logical):
//
//	START → Host (ChatModel)
//	Host ──(no tool call)──→ END
//	Host ──(has tool calls)──→ SpecialistExecutor
//	SpecialistExecutor ──(single intent)──→ END
//	SpecialistExecutor ──(multi intent)──→ Summarize → END
//
// The host's ChatModel decides which specialist(s) to invoke via tool calls.
// Each specialist receives the original user message history (not the tool-call arguments).
// Single-intent calls return the specialist answer directly.
// Multi-intent calls collect all specialist answers and summarize them.
func NewMultiAgent(ctx context.Context, config *MultiAgentConfig) (Runnable[[]*Message, *Message], error) {
	if err := validateMultiAgentConfig(config); err != nil {
		return nil, err
	}

	maxStep := config.MaxStep
	if maxStep <= 0 {
		maxStep = 20
	}

	g := NewGraph[[]*Message, *Message]()

	agentLambda := InvokableLambda(func(ctx context.Context, msgs []*Message) (*Message, error) {
		return executeMultiAgent(ctx, config, msgs)
	})

	if err := g.AddLambdaNode("multi_agent_core", agentLambda); err != nil {
		return nil, err
	}
	if err := g.AddEdge(START, "multi_agent_core"); err != nil {
		return nil, err
	}
	if err := g.AddEdge("multi_agent_core", END); err != nil {
		return nil, err
	}

	return g.Compile(ctx, WithGraphName("HostMultiAgent"), WithMaxRunSteps(maxStep))
}

func validateMultiAgentConfig(config *MultiAgentConfig) error {
	if config == nil {
		return fmt.Errorf("MultiAgentConfig is nil")
	}
	if config.Host == nil {
		return fmt.Errorf("MultiAgentConfig.Host ChatModel is nil")
	}
	if len(config.Specialists) == 0 {
		return fmt.Errorf("MultiAgentConfig.Specialists is empty: at least one specialist is required")
	}
	nameSet := make(map[string]bool)
	for i, spec := range config.Specialists {
		if spec == nil {
			return fmt.Errorf("MultiAgentConfig.Specialists[%d] is nil", i)
		}
		if spec.Name == "" {
			return fmt.Errorf("MultiAgentConfig.Specialists[%d].Name is empty", i)
		}
		if nameSet[spec.Name] {
			return fmt.Errorf("duplicate specialist name %q", spec.Name)
		}
		nameSet[spec.Name] = true
	}
	return nil
}

func executeMultiAgent(ctx context.Context, config *MultiAgentConfig, msgs []*Message) (*Message, error) {
	hostMsg, err := config.Host.Generate(ctx, msgs)
	if err != nil {
		return nil, fmt.Errorf("host model error: %w", err)
	}

	if len(hostMsg.ToolCalls) == 0 {
		return hostMsg, nil
	}

	specSet := buildSpecialistSet(config.Specialists)

	answers := make([]specialistAnswer, 0, len(hostMsg.ToolCalls))
	for _, tc := range hostMsg.ToolCalls {
		spec, ok := specSet[tc.Function.Name]
		if !ok {
			return nil, fmt.Errorf("no specialist registered for tool name %q", tc.Function.Name)
		}

		answer, err := invokeSpecialist(ctx, spec, msgs)
		if err != nil {
			return nil, fmt.Errorf("specialist %q error: %w", spec.Name, err)
		}
		answers = append(answers, specialistAnswer{name: spec.Name, content: answer})
	}

	if len(answers) == 1 {
		return &Message{Role: Assistant, Content: answers[0].content}, nil
	}

	return summarizeAnswers(ctx, config.Summarizer, answers)
}

func buildSpecialistSet(specialists []*Specialist) map[string]*Specialist {
	m := make(map[string]*Specialist, len(specialists))
	for _, s := range specialists {
		m[s.Name] = s
	}
	return m
}

func invokeSpecialist(ctx context.Context, spec *Specialist, originalMsgs []*Message) (string, error) {
	input := originalMsgs

	if spec.ChatModel != nil {
		if spec.SystemPrompt != "" {
			input = append([]*Message{SystemMessage(spec.SystemPrompt)}, originalMsgs...)
		}
		msg, err := spec.ChatModel.Generate(ctx, input)
		if err != nil {
			return "", err
		}
		return msg.Content, nil
	}

	if spec.Invokable != nil {
		msg, err := spec.Invokable(ctx, input)
		if err != nil {
			return "", err
		}
		return msg.Content, nil
	}

	if spec.Streamable != nil {
		sr, err := spec.Streamable(ctx, input)
		if err != nil {
			return "", err
		}
		msgs, err := chatMessageStreamCollect(sr)
		if err != nil {
			return "", err
		}
		if len(msgs) == 0 {
			return "", fmt.Errorf("specialist %q streamable returned no messages", spec.Name)
		}
		return msgs[len(msgs)-1].Content, nil
	}

	return "", fmt.Errorf("specialist %q has no invocable capability (ChatModel, Invokable, or Streamable must be set)", spec.Name)
}

func summarizeAnswers(ctx context.Context, summarizer *Summarizer, answers []specialistAnswer) (*Message, error) {
	if summarizer != nil && summarizer.ChatModel != nil {
		return customSummarize(ctx, summarizer, answers)
	}
	return defaultSummarize(answers), nil
}

func defaultSummarize(answers []specialistAnswer) *Message {
	var parts []string
	for _, a := range answers {
		parts = append(parts, fmt.Sprintf("[%s]: %s", a.name, a.content))
	}
	return &Message{
		Role:    Assistant,
		Content: strings.Join(parts, "\n\n"),
	}
}

func customSummarize(ctx context.Context, summarizer *Summarizer, answers []specialistAnswer) (*Message, error) {
	var parts []string
	for _, a := range answers {
		parts = append(parts, fmt.Sprintf("Expert %s says: %s", a.name, a.content))
	}
	combined := strings.Join(parts, "\n---\n")

	msgs := []*Message{
		UserMessage(combined),
	}
	if summarizer.SystemPrompt != "" {
		msgs = append([]*Message{SystemMessage(summarizer.SystemPrompt)}, msgs...)
	}

	return summarizer.ChatModel.Generate(ctx, msgs)
}
