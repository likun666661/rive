package compose

import (
	"errors"
	"reflect"
	"sort"
	"sync"
)

// ErrConcatNotSupported is returned when no concat function is registered for a type.
var ErrConcatNotSupported = errors.New("compose: concat not supported for type")

var concatFuncRegistry sync.Map

func init() {
	RegisterStreamChunkConcatFunc(ConcatMessages)
	RegisterStreamChunkConcatFunc(ConcatMessageArray)
	RegisterStreamChunkConcatFunc(ConcatToolResults)
}

// RegisterStreamChunkConcatFunc registers a concat function for type T.
func RegisterStreamChunkConcatFunc[T any](fn func([]T) (T, error)) {
	var zero T
	concatFuncRegistry.Store(reflect.TypeOf(zero), fn)
}

// ConcatItems dispatches to the registered concat function for type T.
func ConcatItems[T any](items []T) (T, error) {
	if len(items) == 0 {
		var zero T
		return zero, nil
	}
	if len(items) == 1 {
		return items[0], nil
	}
	var zero T
	t := reflect.TypeOf(zero)
	fn, ok := concatFuncRegistry.Load(t)
	if !ok {
		var zero T
		return zero, ErrConcatNotSupported
	}
	results := reflect.ValueOf(fn).Call([]reflect.Value{reflect.ValueOf(items)})
	result := results[0].Interface().(T)
	var err error
	if !results[1].IsNil() {
		err = results[1].Interface().(error)
	}
	return result, err
}

// ConcatMessages merges streaming Message chunks into a complete Message.
//
// Merge rules:
//   - Content: string concatenation
//   - ReasoningContent: string concatenation
//   - ToolCalls: group by Index, validate consistency, concat Arguments
//   - MultiContent: append slices
//   - ResponseMeta: keep last non-nil
//   - Role: keep first non-zero
func ConcatMessages(chunks []*Message) (*Message, error) {
	if len(chunks) == 0 {
		return nil, nil
	}
	if len(chunks) == 1 {
		return chunks[0], nil
	}

	result := &Message{}
	firstRoleSet := false

	for _, chunk := range chunks {
		if chunk == nil {
			continue
		}
		if !firstRoleSet && chunk.Role != "" {
			result.Role = chunk.Role
			firstRoleSet = true
		}
		result.Content += chunk.Content
		result.ReasoningContent += chunk.ReasoningContent
		if chunk.ResponseMeta != nil {
			result.ResponseMeta = chunk.ResponseMeta
		}
		result.ToolCalls = append(result.ToolCalls, chunk.ToolCalls...)
		result.UserInputMultiContent = append(result.UserInputMultiContent, chunk.UserInputMultiContent...)
		result.AssistantGenMultiContent = append(result.AssistantGenMultiContent, chunk.AssistantGenMultiContent...)
		if chunk.Extra != nil {
			if result.Extra == nil {
				result.Extra = make(map[string]any)
			}
			for k, v := range chunk.Extra {
				result.Extra[k] = v
			}
		}
	}

	var err error
	result.ToolCalls, err = concatToolCalls(result.ToolCalls)
	if err != nil {
		return nil, err
	}

	return result, nil
}

// ConcatMessageArray concats an array of Messages (one per stream position).
func ConcatMessageArray(chunks []*Message) (*Message, error) {
	return ConcatMessages(chunks)
}

// ConcatToolResults merges multiple ToolResults.
func ConcatToolResults(results []*ToolResult) (*ToolResult, error) {
	if len(results) == 0 {
		return nil, nil
	}
	merged := &ToolResult{}
	for _, r := range results {
		if r == nil {
			continue
		}
		merged.Text += r.Text
		merged.Images = append(merged.Images, r.Images...)
		merged.Audio = append(merged.Audio, r.Audio...)
		merged.Video = append(merged.Video, r.Video...)
		merged.Files = append(merged.Files, r.Files...)
	}
	return merged, nil
}

func concatToolCalls(toolCalls []ToolCall) ([]ToolCall, error) {
	if len(toolCalls) == 0 {
		return nil, nil
	}

	groups := make(map[int][]ToolCall)
	unindexed := make([]ToolCall, 0)

	for _, tc := range toolCalls {
		if tc.Index == nil {
			unindexed = append(unindexed, tc)
		} else {
			groups[*tc.Index] = append(groups[*tc.Index], tc)
		}
	}

	result := make([]ToolCall, 0, len(unindexed)+len(groups))
	result = append(result, unindexed...)

	for index, group := range groups {
		merged, err := mergeToolCallGroup(group)
		if err != nil {
			return nil, err
		}
		idx := index
		merged.Index = &idx
		result = append(result, *merged)
	}

	sortToolCallsByIndex(result)
	return result, nil
}

func mergeToolCallGroup(group []ToolCall) (*ToolCall, error) {
	if len(group) == 0 {
		return nil, nil
	}
	merged := &ToolCall{
		ID:       group[0].ID,
		Type:     group[0].Type,
		Function: ToolCallFunction{Name: group[0].Function.Name},
	}
	for _, tc := range group {
		if tc.ID != merged.ID {
			return nil, errors.New("compose: ToolCall ID mismatch in same index group")
		}
		if tc.Type != merged.Type {
			return nil, errors.New("compose: ToolCall Type mismatch in same index group")
		}
		if tc.Function.Name != merged.Function.Name {
			return nil, errors.New("compose: ToolCall Function.Name mismatch in same index group")
		}
		merged.Function.Arguments += tc.Function.Arguments
	}
	return merged, nil
}

func sortToolCallsByIndex(tcs []ToolCall) {
	sort.Slice(tcs, func(i, j int) bool {
		ai, bi := tcs[i].Index, tcs[j].Index
		if ai == nil && bi == nil {
			return false
		}
		if ai == nil {
			return false
		}
		if bi == nil {
			return true
		}
		return *ai < *bi
	})
}
