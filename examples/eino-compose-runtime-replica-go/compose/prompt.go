package compose

import (
	"context"
	"fmt"
	"regexp"
)

var varPattern = regexp.MustCompile(`\{\{(\w+)\}\}`)

type ChatTemplate interface {
	Format(ctx context.Context, vs map[string]any) ([]*Message, error)
}

type MessageTemplate struct {
	systemTemplate *string
	userTemplate   string
}

func NewMessageTemplate(tpl string) *MessageTemplate {
	return &MessageTemplate{userTemplate: tpl}
}

func (mt *MessageTemplate) WithSystemTemplate(tpl string) *MessageTemplate {
	mt.systemTemplate = &tpl
	return mt
}

func (mt *MessageTemplate) Format(ctx context.Context, vs map[string]any) ([]*Message, error) {
	var msgs []*Message
	if mt.systemTemplate != nil {
		sysContent := replaceVars(*mt.systemTemplate, vs)
		msgs = append(msgs, &Message{Role: System, Content: sysContent})
	}
	userContent := replaceVars(mt.userTemplate, vs)
	msgs = append(msgs, &Message{Role: Human, Content: userContent})
	return msgs, nil
}

func replaceVars(tpl string, vs map[string]any) string {
	return varPattern.ReplaceAllStringFunc(tpl, func(match string) string {
		name := match[2 : len(match)-2]
		if val, ok := vs[name]; ok {
			return fmt.Sprint(val)
		}
		return match
	})
}

type ChatTemplateComponent struct {
	ct ChatTemplate
}

func NewChatTemplateComponent(ct ChatTemplate) *ChatTemplateComponent {
	return &ChatTemplateComponent{ct: ct}
}

func (c *ChatTemplateComponent) GetRunnable() *composableRunnable {
	return &composableRunnable{
		i: func(ctx context.Context, input any) (any, error) {
			vs, ok := input.(map[string]any)
			if !ok {
				return nil, fmt.Errorf("ChatTemplateComponent.Invoke: expected map[string]any input, got %T", input)
			}
			return c.ct.Format(ctx, vs)
		},
	}
}

func (c *ChatTemplateComponent) GetComponentType() ComponentType {
	return ComponentOfPrompt
}

type FakeChatTemplate struct {
	FormatFn func(ctx context.Context, vs map[string]any) ([]*Message, error)
}

func NewFakeChatTemplate(fn func(ctx context.Context, vs map[string]any) ([]*Message, error)) *FakeChatTemplate {
	return &FakeChatTemplate{FormatFn: fn}
}

func (f *FakeChatTemplate) Format(ctx context.Context, vs map[string]any) ([]*Message, error) {
	return f.FormatFn(ctx, vs)
}
