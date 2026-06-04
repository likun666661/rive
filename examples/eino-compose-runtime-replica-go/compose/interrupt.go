package compose

import (
	"context"
	"errors"
	"fmt"
	"sync/atomic"
)

// InterruptState stores state that should be visible to the same execution
// address when a checkpointed graph is resumed.
type InterruptState struct {
	State                any
	LayerSpecificPayload any
}

// InterruptPayload is user-facing information attached to an interrupt signal.
type InterruptPayload struct {
	Info        any
	IsRootCause bool
}

// InterruptSignal is the persistent tree form of an interrupt.
type InterruptSignal struct {
	ID             string
	Address        Address
	InterruptInfo  InterruptPayload
	InterruptState InterruptState
	Subs           []*InterruptSignal
}

// InterruptContext is the flattened user-facing form of an interrupt root cause.
type InterruptContext struct {
	ID      string
	Address Address
	Info    any
	State   InterruptState
	Parent  *InterruptContext
}

// InterruptInfo groups the full signal tree and flattened root-cause contexts.
type InterruptInfo struct {
	Signal            *InterruptSignal
	InterruptContexts []*InterruptContext
}

// InterruptError is returned by nodes that intentionally pause execution.
type InterruptError struct {
	Info *InterruptInfo
}

func (e *InterruptError) Error() string {
	if e == nil || e.Info == nil {
		return "interrupt"
	}
	return fmt.Sprintf("interrupt: %d root cause(s)", len(e.Info.InterruptContexts))
}

var interruptCounter uint64

func nextInterruptID() string {
	return fmt.Sprintf("interrupt_%d", atomic.AddUint64(&interruptCounter, 1))
}

func newInterruptSignal(ctx context.Context, info any, state any, rootCause bool) *InterruptSignal {
	addr := GetCurrentAddress(ctx)
	return &InterruptSignal{
		ID:      nextInterruptID(),
		Address: addr,
		InterruptInfo: InterruptPayload{
			Info:        info,
			IsRootCause: rootCause,
		},
		InterruptState: InterruptState{State: state},
	}
}

// Interrupt pauses the current execution scope without custom resume state.
func Interrupt(ctx context.Context, info any) error {
	sig := newInterruptSignal(ctx, info, nil, true)
	return &InterruptError{Info: newInterruptInfo(sig)}
}

// StatefulInterrupt pauses the current execution scope and persists state for resume.
func StatefulInterrupt(ctx context.Context, info any, state any) error {
	sig := newInterruptSignal(ctx, info, state, true)
	return &InterruptError{Info: newInterruptInfo(sig)}
}

// CompositeInterrupt wraps multiple child interrupts under the current scope.
func CompositeInterrupt(ctx context.Context, info any, state any, errs ...error) error {
	var subs []*InterruptSignal
	for _, err := range errs {
		if err == nil {
			continue
		}
		if child, ok := ExtractInterruptInfo(err); ok && child.Signal != nil {
			subs = append(subs, child.Signal)
			continue
		}
		subs = append(subs, &InterruptSignal{
			ID:      nextInterruptID(),
			Address: GetCurrentAddress(ctx),
			InterruptInfo: InterruptPayload{
				Info:        err.Error(),
				IsRootCause: true,
			},
		})
	}

	sig := newInterruptSignal(ctx, info, state, len(subs) == 0)
	sig.Subs = subs
	return &InterruptError{Info: newInterruptInfo(sig)}
}

// ExtractInterruptInfo unwraps an interrupt error.
func ExtractInterruptInfo(err error) (*InterruptInfo, bool) {
	if err == nil {
		return nil, false
	}
	var ie *InterruptError
	if errors.As(err, &ie) && ie != nil && ie.Info != nil {
		return ie.Info, true
	}
	return nil, false
}

func newInterruptInfo(sig *InterruptSignal) *InterruptInfo {
	return &InterruptInfo{
		Signal:            sig,
		InterruptContexts: ToInterruptContexts(sig),
	}
}

// ToInterruptContexts flattens only root-cause interrupt signals for users.
func ToInterruptContexts(sig *InterruptSignal) []*InterruptContext {
	var out []*InterruptContext
	var walk func(s *InterruptSignal, parent *InterruptContext)
	walk = func(s *InterruptSignal, parent *InterruptContext) {
		if s == nil {
			return
		}
		ctx := &InterruptContext{
			ID:      s.ID,
			Address: s.Address.clone(),
			Info:    s.InterruptInfo.Info,
			State:   s.InterruptState,
			Parent:  parent,
		}
		if s.InterruptInfo.IsRootCause {
			out = append(out, ctx)
		}
		for _, sub := range s.Subs {
			walk(sub, ctx)
		}
	}
	walk(sig, nil)
	return out
}

// SignalToPersistenceMaps converts an interrupt tree into checkpoint maps.
func SignalToPersistenceMaps(sig *InterruptSignal) (map[string]Address, map[string]InterruptState) {
	idToAddress := make(map[string]Address)
	idToState := make(map[string]InterruptState)
	var walk func(*InterruptSignal)
	walk = func(s *InterruptSignal) {
		if s == nil {
			return
		}
		idToAddress[s.ID] = s.Address.clone()
		idToState[s.ID] = s.InterruptState
		for _, sub := range s.Subs {
			walk(sub)
		}
	}
	walk(sig)
	return idToAddress, idToState
}
