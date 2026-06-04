package compose

import (
	"context"
	"errors"
	"fmt"
	"testing"
)

func TestRunInfo(t *testing.T) {
	info := &RunInfo{
		Name:      "test-node",
		Type:      "Lambda",
		Component: ComponentOfLambda,
	}
	if info.Name != "test-node" {
		t.Errorf("expected Name='test-node', got %q", info.Name)
	}
	if info.Type != "Lambda" {
		t.Errorf("expected Type='Lambda', got %q", info.Type)
	}
	if info.Component != ComponentOfLambda {
		t.Errorf("expected ComponentOfLambda, got %q", info.Component)
	}
}

func TestCallbackTimingString(t *testing.T) {
	if TimingOnStart.String() != "OnStart" {
		t.Errorf("expected 'OnStart', got %q", TimingOnStart.String())
	}
	if TimingOnEnd.String() != "OnEnd" {
		t.Errorf("expected 'OnEnd', got %q", TimingOnEnd.String())
	}
	if TimingOnError.String() != "OnError" {
		t.Errorf("expected 'OnError', got %q", TimingOnError.String())
	}
	if TimingOnStartWithStreamInput.String() != "OnStartWithStreamInput" {
		t.Errorf("expected 'OnStartWithStreamInput', got %q", TimingOnStartWithStreamInput.String())
	}
	if TimingOnEndWithStreamOutput.String() != "OnEndWithStreamOutput" {
		t.Errorf("expected 'OnEndWithStreamOutput', got %q", TimingOnEndWithStreamOutput.String())
	}
	if CallbackTiming(99).String() != "Unknown" {
		t.Errorf("expected 'Unknown' for unknown timing, got %q", CallbackTiming(99).String())
	}
}

func TestHandlerNeededTimingsEmpty(t *testing.T) {
	h := &Handler{}
	if h.neededTimings() != 0 {
		t.Errorf("empty handler should need 0 timings, got %d", h.neededTimings())
	}
}

func TestHandlerNeededTimingsFull(t *testing.T) {
	h := &Handler{
		OnStart:                func(ctx context.Context, info *RunInfo, input any) context.Context { return ctx },
		OnEnd:                  func(ctx context.Context, info *RunInfo, output any) context.Context { return ctx },
		OnError:                func(ctx context.Context, info *RunInfo, err error) context.Context { return ctx },
		OnStartWithStreamInput: func(ctx context.Context, info *RunInfo, input *CbStreamReader) context.Context { return ctx },
		OnEndWithStreamOutput:  func(ctx context.Context, info *RunInfo, output *CbStreamReader) context.Context { return ctx },
	}
	needed := h.neededTimings()
	expected := TimingOnStart | TimingOnEnd | TimingOnError | TimingOnStartWithStreamInput | TimingOnEndWithStreamOutput
	if needed != expected {
		t.Errorf("expected all timings (%d), got %d", expected, needed)
	}
}

func TestHandlerBuilderTimingCheckerEmpty(t *testing.T) {
	hb := NewHandlerBuilder()
	checker := hb.TimingChecker()
	if checker(TimingOnStart) {
		t.Error("empty builder should not need OnStart")
	}
	if checker(TimingOnEnd) {
		t.Error("empty builder should not need OnEnd")
	}
	if checker(TimingOnStartWithStreamInput) {
		t.Error("empty builder should not need stream input timing")
	}
	if checker(TimingOnEndWithStreamOutput) {
		t.Error("empty builder should not need stream output timing")
	}
}

func TestHandlerBuilderTimingCheckerPartial(t *testing.T) {
	hb := NewHandlerBuilder()
	hb.AddHandler(&Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context { return ctx },
		OnEnd:   func(ctx context.Context, info *RunInfo, output any) context.Context { return ctx },
	})
	checker := hb.TimingChecker()
	if !checker(TimingOnStart) {
		t.Error("should need OnStart")
	}
	if !checker(TimingOnEnd) {
		t.Error("should need OnEnd")
	}
	if checker(TimingOnStartWithStreamInput) {
		t.Error("should not need stream input timing when no handler registers it")
	}
	if checker(TimingOnEndWithStreamOutput) {
		t.Error("should not need stream output timing when no handler registers it")
	}
	if checker(TimingOnError) {
		t.Error("should not need OnError when no handler registers it")
	}
}

func TestHandlerBuilderTimingCheckerAllSet(t *testing.T) {
	hb := NewHandlerBuilder()
	hb.AddHandler(&Handler{
		OnStart:                func(ctx context.Context, info *RunInfo, input any) context.Context { return ctx },
		OnStartWithStreamInput: func(ctx context.Context, info *RunInfo, input *CbStreamReader) context.Context { return ctx },
	})
	hb.AddHandler(&Handler{
		OnEnd:                 func(ctx context.Context, info *RunInfo, output any) context.Context { return ctx },
		OnEndWithStreamOutput: func(ctx context.Context, info *RunInfo, output *CbStreamReader) context.Context { return ctx },
	})
	hb.AddHandler(&Handler{
		OnError: func(ctx context.Context, info *RunInfo, err error) context.Context { return ctx },
	})
	checker := hb.TimingChecker()
	expectedTimings := []CallbackTiming{
		TimingOnStart, TimingOnEnd, TimingOnError,
		TimingOnStartWithStreamInput, TimingOnEndWithStreamOutput,
	}
	for _, timing := range expectedTimings {
		if !checker(timing) {
			t.Errorf("should need %s", timing.String())
		}
	}
}

func TestCallbackWrapperInvokeSuccess(t *testing.T) {
	var (
		startCalled bool
		endCalled   bool
		startInput  any
		endOutput   any
	)

	info := &RunInfo{Name: "test", Type: "Lambda", Component: ComponentOfLambda}
	handler := &Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
			startCalled = true
			startInput = input
			return ctx
		},
		OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
			endCalled = true
			endOutput = output
			return ctx
		},
	}

	cw := NewCallbackWrapper(info, []*Handler{handler})

	baseFn := func(ctx context.Context, input any) (any, error) {
		return fmt.Sprintf("result:%v", input), nil
	}

	wrapped := cw.Invoke(baseFn)
	output, err := wrapped(context.Background(), "hello")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if output != "result:hello" {
		t.Errorf("expected 'result:hello', got %q", output)
	}
	if !startCalled {
		t.Error("OnStart was not called")
	}
	if startInput != "hello" {
		t.Errorf("OnStart input mismatch: got %v", startInput)
	}
	if !endCalled {
		t.Error("OnEnd was not called")
	}
	if endOutput != "result:hello" {
		t.Errorf("OnEnd output mismatch: got %v", endOutput)
	}
}

func TestCallbackWrapperInvokeError(t *testing.T) {
	var (
		startCalled bool
		endCalled   bool
		errorCalled bool
		receivedErr error
	)

	info := &RunInfo{Name: "test", Type: "Lambda", Component: ComponentOfLambda}
	handler := &Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
			startCalled = true
			return ctx
		},
		OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
			endCalled = true
			return ctx
		},
		OnError: func(ctx context.Context, info *RunInfo, err error) context.Context {
			errorCalled = true
			receivedErr = err
			return ctx
		},
	}

	cw := NewCallbackWrapper(info, []*Handler{handler})

	testErr := errors.New("test error")
	baseFn := func(ctx context.Context, input any) (any, error) {
		return nil, testErr
	}

	wrapped := cw.Invoke(baseFn)
	_, err := wrapped(context.Background(), "hello")

	if !errors.Is(err, testErr) {
		t.Errorf("expected testErr, got %v", err)
	}
	if !startCalled {
		t.Error("OnStart was not called")
	}
	if endCalled {
		t.Error("OnEnd should not be called on error")
	}
	if !errorCalled {
		t.Error("OnError was not called")
	}
	if !errors.Is(receivedErr, testErr) {
		t.Errorf("OnError received wrong error: %v", receivedErr)
	}
}

func TestCbStreamReaderCopy(t *testing.T) {
	sr := CbStreamReaderFromSlice([]any{"a", "b", "c"})

	copies := sr.Copy(2)
	if len(copies) != 2 {
		t.Fatalf("expected 2 copies, got %d", len(copies))
	}

	for i, cp := range copies {
		items := cp.All()
		if len(items) != 3 {
			t.Errorf("copy %d: expected 3 items, got %d", i, len(items))
		}
		if items[0] != "a" || items[1] != "b" || items[2] != "c" {
			t.Errorf("copy %d: content mismatch: %v", i, items)
		}
	}
}

func TestCbStreamReaderCopyIndependentReads(t *testing.T) {
	sr := CbStreamReaderFromSlice([]any{1, 2, 3, 4, 5})

	copies := sr.Copy(2)

	cp0Items := []any{}
	cp1Items := []any{}

	for {
		item, ok := copies[0].Next()
		if !ok {
			break
		}
		cp0Items = append(cp0Items, item)
	}

	for {
		item, ok := copies[1].Next()
		if !ok {
			break
		}
		cp1Items = append(cp1Items, item)
	}

	if len(cp0Items) != 5 {
		t.Errorf("copy 0: expected 5 items, got %d", len(cp0Items))
	}
	if len(cp1Items) != 5 {
		t.Errorf("copy 1: expected 5 items, got %d", len(cp1Items))
	}

	for i, v := range []any{1, 2, 3, 4, 5} {
		if cp0Items[i] != v {
			t.Errorf("copy 0[%d]: expected %v, got %v", i, v, cp0Items[i])
		}
		if cp1Items[i] != v {
			t.Errorf("copy 1[%d]: expected %v, got %v", i, v, cp1Items[i])
		}
	}
}

func TestCbStreamReaderOriginalReadsAfterCopy(t *testing.T) {
	sr := CbStreamReaderFromSlice([]any{"x", "y"})

	copies := sr.Copy(1)

	cpItems := copies[0].All()
	if len(cpItems) != 2 {
		t.Fatalf("copy: expected 2 items, got %d", len(cpItems))
	}
	if cpItems[0] != "x" || cpItems[1] != "y" {
		t.Errorf("copy content mismatch: %v", cpItems)
	}

	origItems := sr.All()
	if len(origItems) != 2 {
		t.Fatalf("original: expected 2 items, got %d", len(origItems))
	}
	if origItems[0] != "x" || origItems[1] != "y" {
		t.Errorf("original content mismatch: %v", origItems)
	}
}

func TestCallbackWrapperStreamOnEndWithStreamOutput(t *testing.T) {
	var handlerReadItems []any

	info := &RunInfo{Name: "stream", Type: "Lambda", Component: ComponentOfLambda}
	handler := &Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
			return ctx
		},
		OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
			return ctx
		},
		OnEndWithStreamOutput: func(ctx context.Context, info *RunInfo, output *CbStreamReader) context.Context {
			for {
				item, ok := output.Next()
				if !ok {
					break
				}
				handlerReadItems = append(handlerReadItems, item)
			}
			return ctx
		},
	}

	cw := NewCallbackWrapper(info, []*Handler{handler})

	streamOutput := CbStreamReaderFromSlice([]any{"r1", "r2", "r3"})

	baseFn := func(ctx context.Context, input any) (*CbStreamReader, error) {
		return streamOutput, nil
	}

	wrapped := cw.Stream(baseFn)
	output, err := wrapped(context.Background(), "hello")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	consumerItems := output.All()
	if len(consumerItems) != 3 {
		t.Errorf("consumer: expected 3 items, got %d (%v)", len(consumerItems), consumerItems)
	}
	for i, expected := range []any{"r1", "r2", "r3"} {
		if consumerItems[i] != expected {
			t.Errorf("consumer[%d]: expected %v, got %v", i, expected, consumerItems[i])
		}
	}

	if len(handlerReadItems) != 3 {
		t.Errorf("handler: expected 3 items, got %d (%v)", len(handlerReadItems), handlerReadItems)
	}
	for i, expected := range []any{"r1", "r2", "r3"} {
		if handlerReadItems[i] != expected {
			t.Errorf("handler[%d]: expected %v, got %v", i, expected, handlerReadItems[i])
		}
	}
}

func TestCallbackWrapperCollect(t *testing.T) {
	var (
		streamInputCalled bool
		handlerStreamData []any
	)

	info := &RunInfo{Name: "collect", Type: "Lambda", Component: ComponentOfLambda}
	handler := &Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
			return ctx
		},
		OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
			return ctx
		},
		OnStartWithStreamInput: func(ctx context.Context, info *RunInfo, input *CbStreamReader) context.Context {
			streamInputCalled = true
			for {
				item, ok := input.Next()
				if !ok {
					break
				}
				handlerStreamData = append(handlerStreamData, item)
			}
			return ctx
		},
	}

	cw := NewCallbackWrapper(info, []*Handler{handler})

	streamInput := CbStreamReaderFromSlice([]any{10, 20, 30})

	baseFn := func(ctx context.Context, input *CbStreamReader) (any, error) {
		sum := 0
		for {
			item, ok := input.Next()
			if !ok {
				break
			}
			sum += item.(int)
		}
		return sum, nil
	}

	wrapped := cw.Collect(baseFn)
	output, err := wrapped(context.Background(), streamInput)

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if output != 60 {
		t.Errorf("expected 60, got %v", output)
	}
	if !streamInputCalled {
		t.Error("OnStartWithStreamInput was not called")
	}
	if len(handlerStreamData) != 3 {
		t.Errorf("handler: expected 3 stream items, got %d", len(handlerStreamData))
	}
}

func TestCallbackWrapperCollectHandlerReceivesCopiedReader(t *testing.T) {
	var handlerData []any

	info := &RunInfo{Name: "collect", Type: "Lambda", Component: ComponentOfLambda}
	handler := &Handler{
		OnStartWithStreamInput: func(ctx context.Context, info *RunInfo, input *CbStreamReader) context.Context {
			for {
				item, ok := input.Next()
				if !ok {
					break
				}
				handlerData = append(handlerData, item)
			}
			return ctx
		},
	}

	cw := NewCallbackWrapper(info, []*Handler{handler})

	streamInput := CbStreamReaderFromSlice([]any{"a1", "a2", "a3"})

	baseFn := func(ctx context.Context, input *CbStreamReader) (any, error) {
		items := input.All()
		return len(items), nil
	}

	wrapped := cw.Collect(baseFn)
	output, err := wrapped(context.Background(), streamInput)

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if output != 3 {
		t.Errorf("consumer expected 3 items processed, got %v", output)
	}
	if len(handlerData) != 3 {
		t.Errorf("handler expected 3 items, got %d", len(handlerData))
	}
}

func TestCallbackWrapperTransform(t *testing.T) {
	var (
		streamInputCalled  bool
		streamOutputCalled bool
		handlerInputData   []any
		handlerOutputData  []any
	)

	info := &RunInfo{Name: "transform", Type: "Lambda", Component: ComponentOfLambda}
	handler := &Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
			return ctx
		},
		OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
			return ctx
		},
		OnStartWithStreamInput: func(ctx context.Context, info *RunInfo, input *CbStreamReader) context.Context {
			streamInputCalled = true
			handlerInputData = input.All()
			return ctx
		},
		OnEndWithStreamOutput: func(ctx context.Context, info *RunInfo, output *CbStreamReader) context.Context {
			streamOutputCalled = true
			handlerOutputData = output.All()
			return ctx
		},
	}

	cw := NewCallbackWrapper(info, []*Handler{handler})

	streamInput := CbStreamReaderFromSlice([]any{1, 2, 3})

	baseFn := func(ctx context.Context, input *CbStreamReader) (*CbStreamReader, error) {
		items := input.All()
		result := make([]any, len(items))
		for i, v := range items {
			result[i] = v.(int) * 10
		}
		return CbStreamReaderFromSlice(result), nil
	}

	wrapped := cw.Transform(baseFn)
	output, err := wrapped(context.Background(), streamInput)

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	consumerData := output.All()
	if len(consumerData) != 3 {
		t.Errorf("consumer: expected 3 items, got %d", len(consumerData))
	}
	if consumerData[0] != 10 || consumerData[1] != 20 || consumerData[2] != 30 {
		t.Errorf("consumer data mismatch: %v", consumerData)
	}

	if !streamInputCalled {
		t.Error("OnStartWithStreamInput was not called")
	}
	if !streamOutputCalled {
		t.Error("OnEndWithStreamOutput was not called")
	}
	if len(handlerInputData) != 3 {
		t.Errorf("handler input: expected 3 items, got %d", len(handlerInputData))
	}
	if len(handlerOutputData) != 3 {
		t.Errorf("handler output: expected 3 items, got %d", len(handlerOutputData))
	}
}

func TestTimingCheckerSkipsStreamCopy(t *testing.T) {
	info := &RunInfo{Name: "test", Type: "Lambda", Component: ComponentOfLambda}

	handler := &Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context { return ctx },
		OnEnd:   func(ctx context.Context, info *RunInfo, output any) context.Context { return ctx },
	}

	cw := NewCallbackWrapper(info, []*Handler{handler})
	checker := cw.TimingChecker()

	if checker(TimingOnStartWithStreamInput) {
		t.Error("TimingChecker should report false for stream input when no handler needs it")
	}
	if checker(TimingOnEndWithStreamOutput) {
		t.Error("TimingChecker should report false for stream output when no handler needs it")
	}
}

func TestTimingCheckerSignalsStreamCopyNeeded(t *testing.T) {
	info := &RunInfo{Name: "test", Type: "Lambda", Component: ComponentOfLambda}

	handler := &Handler{
		OnStart:                func(ctx context.Context, info *RunInfo, input any) context.Context { return ctx },
		OnStartWithStreamInput: func(ctx context.Context, info *RunInfo, input *CbStreamReader) context.Context { return ctx },
		OnEndWithStreamOutput:  func(ctx context.Context, info *RunInfo, output *CbStreamReader) context.Context { return ctx },
	}

	cw := NewCallbackWrapper(info, []*Handler{handler})
	checker := cw.TimingChecker()

	if !checker(TimingOnStartWithStreamInput) {
		t.Error("TimingChecker should report true for stream input when handler needs it")
	}
	if !checker(TimingOnEndWithStreamOutput) {
		t.Error("TimingChecker should report true for stream output when handler needs it")
	}
}

func TestPerHandlerContextChainNotCrossHandlerGlobalOrdering(t *testing.T) {
	// Each handler's OnStart receives the base context (not the previous
	// handler's modified context). Context chaining is per handler:
	// h.OnStart(ctx_base) -> returns ctx_h
	// h.OnEnd(ctx_h) for the same handler
	// Handlers do not influence each other's context chain.
	info := &RunInfo{Name: "chain-test", Type: "Lambda", Component: ComponentOfLambda}

	type ctxKey string

	var h1StartCtxVal string
	var h1EndCtxVal string
	var h2StartCtxVal string
	var h2EndCtxVal string

	handler1 := &Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
			h1StartCtxVal, _ = ctx.Value(ctxKey("key")).(string)
			return context.WithValue(ctx, ctxKey("key"), "from-h1-onstart")
		},
		OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
			h1EndCtxVal, _ = ctx.Value(ctxKey("key")).(string)
			return ctx
		},
	}

	handler2 := &Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
			h2StartCtxVal, _ = ctx.Value(ctxKey("key")).(string)
			return context.WithValue(ctx, ctxKey("key"), "from-h2-onstart")
		},
		OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
			h2EndCtxVal, _ = ctx.Value(ctxKey("key")).(string)
			return ctx
		},
	}

	cw := NewCallbackWrapper(info, []*Handler{handler1, handler2})

	baseFn := func(ctx context.Context, input any) (any, error) { return input, nil }

	wrapped := cw.Invoke(baseFn)
	rootCtx := context.WithValue(context.Background(), ctxKey("key"), "root")
	_, err := wrapped(rootCtx, "test")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// h1.OnStart receives the base context ("root")
	if h1StartCtxVal != "root" {
		t.Errorf("handler1 OnStart: expected 'root', got %q", h1StartCtxVal)
	}
	// h1.OnEnd receives h1's own OnStart return ("from-h1-onstart")
	if h1EndCtxVal != "from-h1-onstart" {
		t.Errorf("handler1 OnEnd: expected 'from-h1-onstart', got %q", h1EndCtxVal)
	}
	// h2.OnStart receives the base context ("root"), NOT h1's modified context
	if h2StartCtxVal != "root" {
		t.Errorf("handler2 OnStart: expected 'root' (base context, not chained from h1), got %q", h2StartCtxVal)
	}
	// h2.OnEnd receives h2's own OnStart return ("from-h2-onstart")
	if h2EndCtxVal != "from-h2-onstart" {
		t.Errorf("handler2 OnEnd: expected 'from-h2-onstart' (h2's own context), got %q", h2EndCtxVal)
	}
}

func TestInitCallbackInvoke(t *testing.T) {
	var startCalled bool
	var endCalled bool

	info := &RunInfo{Name: "init", Type: "Lambda", Component: ComponentOfLambda}
	handlers := []*Handler{
		{
			OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
				startCalled = true
				return ctx
			},
			OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
				endCalled = true
				return ctx
			},
		},
	}

	fn := func(ctx context.Context, input any) (any, error) {
		return fmt.Sprintf("out:%v", input), nil
	}

	wrapped := InitCallbackInvoke(info, handlers, fn)
	output, err := wrapped(context.Background(), "data")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if output != "out:data" {
		t.Errorf("expected 'out:data', got %q", output)
	}
	if !startCalled || !endCalled {
		t.Errorf("callbacks not called: start=%v end=%v", startCalled, endCalled)
	}
}

func TestInitCallbackStream(t *testing.T) {
	var streamOutputCalled bool

	info := &RunInfo{Name: "init-stream", Type: "Lambda", Component: ComponentOfLambda}
	handlers := []*Handler{
		{
			OnEndWithStreamOutput: func(ctx context.Context, info *RunInfo, output *CbStreamReader) context.Context {
				streamOutputCalled = true
				return ctx
			},
		},
	}

	fn := func(ctx context.Context, input any) (*CbStreamReader, error) {
		return CbStreamReaderFromSlice([]any{"x"}), nil
	}

	wrapped := InitCallbackStream(info, handlers, fn)
	output, err := wrapped(context.Background(), "data")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if output == nil {
		t.Fatal("expected non-nil output")
	}
	if !streamOutputCalled {
		t.Error("OnEndWithStreamOutput was not called")
	}
}

func TestInitCallbackCollect(t *testing.T) {
	var streamInputCalled bool

	info := &RunInfo{Name: "init-collect", Type: "Lambda", Component: ComponentOfLambda}
	handlers := []*Handler{
		{
			OnStartWithStreamInput: func(ctx context.Context, info *RunInfo, input *CbStreamReader) context.Context {
				streamInputCalled = true
				return ctx
			},
		},
	}

	fn := func(ctx context.Context, input *CbStreamReader) (any, error) {
		return 42, nil
	}

	wrapped := InitCallbackCollect(info, handlers, fn)
	output, err := wrapped(context.Background(), CbStreamReaderFromSlice([]any{1}))

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if output != 42 {
		t.Errorf("expected 42, got %v", output)
	}
	if !streamInputCalled {
		t.Error("OnStartWithStreamInput was not called")
	}
}

func TestInitCallbackTransform(t *testing.T) {
	var streamInputCalled bool
	var streamOutputCalled bool

	info := &RunInfo{Name: "init-transform", Type: "Lambda", Component: ComponentOfLambda}
	handlers := []*Handler{
		{
			OnStartWithStreamInput: func(ctx context.Context, info *RunInfo, input *CbStreamReader) context.Context {
				streamInputCalled = true
				return ctx
			},
			OnEndWithStreamOutput: func(ctx context.Context, info *RunInfo, output *CbStreamReader) context.Context {
				streamOutputCalled = true
				return ctx
			},
		},
	}

	fn := func(ctx context.Context, input *CbStreamReader) (*CbStreamReader, error) {
		return CbStreamReaderFromSlice([]any{"transformed"}), nil
	}

	wrapped := InitCallbackTransform(info, handlers, fn)
	output, err := wrapped(context.Background(), CbStreamReaderFromSlice([]any{"a"}))

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if output == nil {
		t.Fatal("expected non-nil output")
	}
	if !streamInputCalled {
		t.Error("OnStartWithStreamInput was not called")
	}
	if !streamOutputCalled {
		t.Error("OnEndWithStreamOutput was not called")
	}
}

func TestCallbackWrapperStreamError(t *testing.T) {
	var errorCalled bool
	var endCalled bool
	var receivedErr error

	info := &RunInfo{Name: "stream-err", Type: "Lambda", Component: ComponentOfLambda}
	handler := &Handler{
		OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context { return ctx },
		OnError: func(ctx context.Context, info *RunInfo, err error) context.Context {
			errorCalled = true
			receivedErr = err
			return ctx
		},
		OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context {
			endCalled = true
			return ctx
		},
	}

	cw := NewCallbackWrapper(info, []*Handler{handler})

	testErr := errors.New("stream failure")
	baseFn := func(ctx context.Context, input any) (*CbStreamReader, error) {
		return nil, testErr
	}

	wrapped := cw.Stream(baseFn)
	_, err := wrapped(context.Background(), "hello")

	if !errors.Is(err, testErr) {
		t.Errorf("expected testErr, got %v", err)
	}
	if !errorCalled {
		t.Error("OnError was not called")
	}
	if !errors.Is(receivedErr, testErr) {
		t.Errorf("OnError received wrong error: %v", receivedErr)
	}
	if endCalled {
		t.Error("OnEnd should not be called on error")
	}
}

func TestCallbackWrapperCollectError(t *testing.T) {
	var errorCalled bool

	info := &RunInfo{Name: "collect-err", Type: "Lambda", Component: ComponentOfLambda}
	handler := &Handler{
		OnError: func(ctx context.Context, info *RunInfo, err error) context.Context {
			errorCalled = true
			return ctx
		},
	}

	cw := NewCallbackWrapper(info, []*Handler{handler})

	testErr := errors.New("collect failure")
	baseFn := func(ctx context.Context, input *CbStreamReader) (any, error) {
		return nil, testErr
	}

	wrapped := cw.Collect(baseFn)
	_, err := wrapped(context.Background(), CbStreamReaderFromSlice([]any{1}))

	if err == nil {
		t.Fatal("expected error")
	}
	if !errorCalled {
		t.Error("OnError was not called")
	}
}

func TestCallbackWrapperTransformError(t *testing.T) {
	var errorCalled bool

	info := &RunInfo{Name: "transform-err", Type: "Lambda", Component: ComponentOfLambda}
	handler := &Handler{
		OnError: func(ctx context.Context, info *RunInfo, err error) context.Context {
			errorCalled = true
			return ctx
		},
	}

	cw := NewCallbackWrapper(info, []*Handler{handler})

	testErr := errors.New("transform failure")
	baseFn := func(ctx context.Context, input *CbStreamReader) (*CbStreamReader, error) {
		return nil, testErr
	}

	wrapped := cw.Transform(baseFn)
	_, err := wrapped(context.Background(), CbStreamReaderFromSlice([]any{1}))

	if err == nil {
		t.Fatal("expected error")
	}
	if !errorCalled {
		t.Error("OnError was not called")
	}
}

func TestHandlerBuilderAddHandler(t *testing.T) {
	hb := NewHandlerBuilder()
	h1 := &Handler{OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context { return ctx }}
	h2 := &Handler{OnEnd: func(ctx context.Context, info *RunInfo, output any) context.Context { return ctx }}

	hb.AddHandler(h1).AddHandler(h2)

	handlers := hb.Handlers()
	if len(handlers) != 2 {
		t.Fatalf("expected 2 handlers, got %d", len(handlers))
	}
	if handlers[0] != h1 {
		t.Error("first handler mismatch")
	}
	if handlers[1] != h2 {
		t.Error("second handler mismatch")
	}
}

func TestCbStreamReaderRemaining(t *testing.T) {
	sr := CbStreamReaderFromSlice([]any{1, 2, 3, 4, 5})

	if sr.Remaining() != 5 {
		t.Errorf("expected 5 remaining, got %d", sr.Remaining())
	}

	_, _ = sr.Next()

	if sr.Remaining() != 4 {
		t.Errorf("expected 4 remaining after 1 read, got %d", sr.Remaining())
	}

	sr.All()

	if sr.Remaining() != 0 {
		t.Errorf("expected 0 remaining after All(), got %d", sr.Remaining())
	}
}

func TestCbStreamReaderNextAndAll(t *testing.T) {
	sr := CbStreamReaderFromSlice([]any{"a", "b", "c", "d"})

	item, ok := sr.Next()
	if !ok || item != "a" {
		t.Errorf("first Next: expected 'a', got %v (ok=%v)", item, ok)
	}

	item, ok = sr.Next()
	if !ok || item != "b" {
		t.Errorf("second Next: expected 'b', got %v (ok=%v)", item, ok)
	}

	rest := sr.All()
	if len(rest) != 2 {
		t.Errorf("All: expected 2 items, got %d", len(rest))
	}
	if rest[0] != "c" || rest[1] != "d" {
		t.Errorf("All content mismatch: %v", rest)
	}

	_, ok = sr.Next()
	if ok {
		t.Error("Next should return false after exhausted")
	}
}

func TestCbStreamReaderNext(t *testing.T) {
	sr := CbStreamReaderFromSlice([]any{"single"})

	item, ok := sr.Next()
	if !ok {
		t.Fatal("expected item")
	}
	if item != "single" {
		t.Errorf("expected 'single', got %q", item)
	}

	_, ok = sr.Next()
	if ok {
		t.Error("expected false for second Next")
	}
}

func TestCbStreamReaderFromNilSlice(t *testing.T) {
	sr := CbStreamReaderFromSlice(nil)
	if sr.Remaining() != 0 {
		t.Errorf("nil slice should have 0 remaining, got %d", sr.Remaining())
	}
	_, ok := sr.Next()
	if ok {
		t.Error("Next should return false for nil slice reader")
	}
}

func TestCbStreamReaderAll(t *testing.T) {
	sr := CbStreamReaderFromSlice([]any{10, 20, 30})

	items := sr.All()
	if len(items) != 3 {
		t.Fatalf("expected 3 items, got %d", len(items))
	}
	if items[0] != 10 || items[1] != 20 || items[2] != 30 {
		t.Errorf("content mismatch: %v", items)
	}

	if sr.Remaining() != 0 {
		t.Error("reader should be empty after All()")
	}
}
