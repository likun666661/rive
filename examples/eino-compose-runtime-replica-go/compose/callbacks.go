package compose

import (
	"context"
	"sync"
)

type RunInfo struct {
	Name      string
	Type      string
	Component ComponentType
}

type CallbackTiming int

const (
	TimingOnStart                CallbackTiming = 1 << 0
	TimingOnEnd                  CallbackTiming = 1 << 1
	TimingOnError                CallbackTiming = 1 << 2
	TimingOnStartWithStreamInput CallbackTiming = 1 << 3
	TimingOnEndWithStreamOutput  CallbackTiming = 1 << 4
)

func (t CallbackTiming) String() string {
	switch t {
	case TimingOnStart:
		return "OnStart"
	case TimingOnEnd:
		return "OnEnd"
	case TimingOnError:
		return "OnError"
	case TimingOnStartWithStreamInput:
		return "OnStartWithStreamInput"
	case TimingOnEndWithStreamOutput:
		return "OnEndWithStreamOutput"
	default:
		return "Unknown"
	}
}

type TimingChecker func(timing CallbackTiming) bool

type CbStreamReader struct {
	mu   sync.Mutex
	data []any
	pos  int
}

func CbStreamReaderFromSlice(items []any) *CbStreamReader {
	return &CbStreamReader{data: items}
}

func (sr *CbStreamReader) Next() (any, bool) {
	sr.mu.Lock()
	defer sr.mu.Unlock()
	if sr.pos >= len(sr.data) {
		return nil, false
	}
	item := sr.data[sr.pos]
	sr.pos++
	return item, true
}

func (sr *CbStreamReader) All() []any {
	sr.mu.Lock()
	defer sr.mu.Unlock()
	rest := make([]any, len(sr.data)-sr.pos)
	copy(rest, sr.data[sr.pos:])
	sr.pos = len(sr.data)
	return rest
}

func (sr *CbStreamReader) Remaining() int {
	sr.mu.Lock()
	defer sr.mu.Unlock()
	return len(sr.data) - sr.pos
}

func (sr *CbStreamReader) Copy(n int) []*CbStreamReader {
	sr.mu.Lock()
	defer sr.mu.Unlock()
	copies := make([]*CbStreamReader, n)
	for i := 0; i < n; i++ {
		d := make([]any, len(sr.data))
		copy(d, sr.data)
		copies[i] = &CbStreamReader{data: d}
	}
	return copies
}

type OnStartFn func(ctx context.Context, info *RunInfo, input any) context.Context
type OnEndFn func(ctx context.Context, info *RunInfo, output any) context.Context
type OnErrorFn func(ctx context.Context, info *RunInfo, err error) context.Context
type OnStartWithStreamInputFn func(ctx context.Context, info *RunInfo, input *CbStreamReader) context.Context
type OnEndWithStreamOutputFn func(ctx context.Context, info *RunInfo, output *CbStreamReader) context.Context

// Handler groups lifecycle callbacks for a Runnable execution.
// Context chaining is per handler: each handler's OnStart receives the base
// context (not the previous handler's modified context), and the context it
// returns is passed to that same handler's OnEnd/OnError. Handlers do not
// influence each other's context chain. This is intentional: there is no
// cross-handler global ordering or priority.
type Handler struct {
	OnStart                OnStartFn
	OnEnd                  OnEndFn
	OnError                OnErrorFn
	OnStartWithStreamInput OnStartWithStreamInputFn
	OnEndWithStreamOutput  OnEndWithStreamOutputFn
}

func (h *Handler) neededTimings() CallbackTiming {
	var t CallbackTiming
	if h.OnStart != nil {
		t |= TimingOnStart
	}
	if h.OnEnd != nil {
		t |= TimingOnEnd
	}
	if h.OnError != nil {
		t |= TimingOnError
	}
	if h.OnStartWithStreamInput != nil {
		t |= TimingOnStartWithStreamInput
	}
	if h.OnEndWithStreamOutput != nil {
		t |= TimingOnEndWithStreamOutput
	}
	return t
}

type HandlerBuilder struct {
	handlers []*Handler
}

func NewHandlerBuilder() *HandlerBuilder {
	return &HandlerBuilder{}
}

func (hb *HandlerBuilder) AddHandler(h *Handler) *HandlerBuilder {
	hb.handlers = append(hb.handlers, h)
	return hb
}

func (hb *HandlerBuilder) Handlers() []*Handler {
	return hb.handlers
}

func (hb *HandlerBuilder) TimingChecker() TimingChecker {
	needed := CallbackTiming(0)
	for _, h := range hb.handlers {
		needed |= h.neededTimings()
	}
	return func(timing CallbackTiming) bool {
		return needed&timing != 0
	}
}

type CallbackWrapper struct {
	handlers []*Handler
	info     *RunInfo
}

func NewCallbackWrapper(info *RunInfo, handlers []*Handler) *CallbackWrapper {
	if handlers == nil {
		handlers = []*Handler{}
	}
	return &CallbackWrapper{handlers: handlers, info: info}
}

func (cw *CallbackWrapper) TimingChecker() TimingChecker {
	needed := CallbackTiming(0)
	for _, h := range cw.handlers {
		needed |= h.neededTimings()
	}
	return func(timing CallbackTiming) bool {
		return needed&timing != 0
	}
}

func (cw *CallbackWrapper) Invoke(
	i func(ctx context.Context, input any) (output any, err error),
) func(ctx context.Context, input any) (output any, err error) {
	return func(ctx context.Context, input any) (output any, err error) {
		handlerCtxs := make([]context.Context, len(cw.handlers))
		for idx, h := range cw.handlers {
			if h.OnStart != nil {
				handlerCtxs[idx] = h.OnStart(ctx, cw.info, input)
			} else {
				handlerCtxs[idx] = ctx
			}
		}

		output, err = i(ctx, input)
		if err != nil {
			for idx, h := range cw.handlers {
				if h.OnError != nil {
					h.OnError(handlerCtxs[idx], cw.info, err)
				}
			}
			return nil, err
		}

		for idx, h := range cw.handlers {
			if h.OnEnd != nil {
				h.OnEnd(handlerCtxs[idx], cw.info, output)
			}
		}
		return output, nil
	}
}

func (cw *CallbackWrapper) Stream(
	s func(ctx context.Context, input any) (output *CbStreamReader, err error),
) func(ctx context.Context, input any) (output *CbStreamReader, err error) {
	return func(ctx context.Context, input any) (output *CbStreamReader, err error) {
		handlerCtxs := make([]context.Context, len(cw.handlers))
		for idx, h := range cw.handlers {
			if h.OnStart != nil {
				handlerCtxs[idx] = h.OnStart(ctx, cw.info, input)
			} else {
				handlerCtxs[idx] = ctx
			}
		}

		output, err = s(ctx, input)
		if err != nil {
			for idx, h := range cw.handlers {
				if h.OnError != nil {
					h.OnError(handlerCtxs[idx], cw.info, err)
				}
			}
			return nil, err
		}

		cw.dispatchOnEndWithStreamOutput(handlerCtxs, output)
		for idx, h := range cw.handlers {
			if h.OnEnd != nil {
				h.OnEnd(handlerCtxs[idx], cw.info, output)
			}
		}
		return output, nil
	}
}

func (cw *CallbackWrapper) Collect(
	c func(ctx context.Context, input *CbStreamReader) (output any, err error),
) func(ctx context.Context, input *CbStreamReader) (output any, err error) {
	return func(ctx context.Context, input *CbStreamReader) (output any, err error) {
		handlerCtxs := cw.dispatchOnStartWithStreamInput(ctx, input)

		for idx, h := range cw.handlers {
			if h.OnStart != nil {
				handlerCtxs[idx] = h.OnStart(handlerCtxs[idx], cw.info, input)
			}
		}

		output, err = c(ctx, input)
		if err != nil {
			for idx, h := range cw.handlers {
				if h.OnError != nil {
					h.OnError(handlerCtxs[idx], cw.info, err)
				}
			}
			return nil, err
		}

		for idx, h := range cw.handlers {
			if h.OnEnd != nil {
				h.OnEnd(handlerCtxs[idx], cw.info, output)
			}
		}
		return output, nil
	}
}

func (cw *CallbackWrapper) Transform(
	t func(ctx context.Context, input *CbStreamReader) (output *CbStreamReader, err error),
) func(ctx context.Context, input *CbStreamReader) (output *CbStreamReader, err error) {
	return func(ctx context.Context, input *CbStreamReader) (output *CbStreamReader, err error) {
		handlerCtxs := cw.dispatchOnStartWithStreamInput(ctx, input)

		for idx, h := range cw.handlers {
			if h.OnStart != nil {
				handlerCtxs[idx] = h.OnStart(handlerCtxs[idx], cw.info, input)
			}
		}

		output, err = t(ctx, input)
		if err != nil {
			for idx, h := range cw.handlers {
				if h.OnError != nil {
					h.OnError(handlerCtxs[idx], cw.info, err)
				}
			}
			return nil, err
		}

		cw.dispatchOnEndWithStreamOutput(handlerCtxs, output)
		for idx, h := range cw.handlers {
			if h.OnEnd != nil {
				h.OnEnd(handlerCtxs[idx], cw.info, output)
			}
		}
		return output, nil
	}
}

func (cw *CallbackWrapper) dispatchOnStartWithStreamInput(
	baseCtx context.Context, input *CbStreamReader,
) []context.Context {
	handlerCtxs := make([]context.Context, len(cw.handlers))
	for idx := range cw.handlers {
		handlerCtxs[idx] = baseCtx
	}

	checker := cw.TimingChecker()
	if !checker(TimingOnStartWithStreamInput) {
		return handlerCtxs
	}

	copies := input.Copy(countStreamHandlers(cw.handlers, func(h *Handler) bool {
		return h.OnStartWithStreamInput != nil
	}))
	copyIdx := 0
	for idx, h := range cw.handlers {
		if h.OnStartWithStreamInput != nil {
			handlerCtxs[idx] = h.OnStartWithStreamInput(baseCtx, cw.info, copies[copyIdx])
			copyIdx++
		}
	}
	return handlerCtxs
}

func (cw *CallbackWrapper) dispatchOnEndWithStreamOutput(
	handlerCtxs []context.Context, output *CbStreamReader,
) {
	checker := cw.TimingChecker()
	if !checker(TimingOnEndWithStreamOutput) {
		return
	}

	copies := output.Copy(countStreamHandlers(cw.handlers, func(h *Handler) bool {
		return h.OnEndWithStreamOutput != nil
	}))
	copyIdx := 0
	for idx, h := range cw.handlers {
		if h.OnEndWithStreamOutput != nil {
			h.OnEndWithStreamOutput(handlerCtxs[idx], cw.info, copies[copyIdx])
			copyIdx++
		}
	}
}

func countStreamHandlers(handlers []*Handler, pred func(*Handler) bool) int {
	n := 0
	for _, h := range handlers {
		if pred(h) {
			n++
		}
	}
	return n
}

type InvokeFn func(ctx context.Context, input any) (output any, err error)
type StreamFn func(ctx context.Context, input any) (output *CbStreamReader, err error)
type CollectFn func(ctx context.Context, input *CbStreamReader) (output any, err error)
type TransformFn func(ctx context.Context, input *CbStreamReader) (output *CbStreamReader, err error)

func InitCallbackInvoke(info *RunInfo, handlers []*Handler, fn InvokeFn) InvokeFn {
	return NewCallbackWrapper(info, handlers).Invoke(fn)
}

func InitCallbackStream(info *RunInfo, handlers []*Handler, fn StreamFn) StreamFn {
	return NewCallbackWrapper(info, handlers).Stream(fn)
}

func InitCallbackCollect(info *RunInfo, handlers []*Handler, fn CollectFn) CollectFn {
	return NewCallbackWrapper(info, handlers).Collect(fn)
}

func InitCallbackTransform(info *RunInfo, handlers []*Handler, fn TransformFn) TransformFn {
	return NewCallbackWrapper(info, handlers).Transform(fn)
}
