package compose

import "context"

// ResumeWithData targets one interrupt ID with data to be delivered when the
// graph reaches the matching address during resume.
func ResumeWithData(ctx context.Context, interruptID string, data any) context.Context {
	gri := cloneGlobalResumeInfo(getGlobalResumeInfo(ctx))
	gri.resumeData[interruptID] = data
	return context.WithValue(ctx, globalResumeInfoKey{}, gri)
}

// BatchResumeWithData targets multiple interrupt IDs in one resume operation.
func BatchResumeWithData(ctx context.Context, data map[string]any) context.Context {
	gri := cloneGlobalResumeInfo(getGlobalResumeInfo(ctx))
	for id, value := range data {
		gri.resumeData[id] = value
	}
	return context.WithValue(ctx, globalResumeInfoKey{}, gri)
}

// GetInterruptState returns whether the current address was previously
// interrupted and, if possible, the typed state saved for that address.
func GetInterruptState[T any](ctx context.Context) (wasInterrupted bool, hasState bool, state T) {
	ac := getAddressContext(ctx)
	if ac == nil || ac.interruptState == nil {
		return false, false, state
	}
	wasInterrupted = true
	if ac.interruptState.State == nil {
		return wasInterrupted, false, state
	}
	typed, ok := ac.interruptState.State.(T)
	if !ok {
		return wasInterrupted, false, state
	}
	return wasInterrupted, true, typed
}

// GetResumeContext returns whether the current address is a resume target or a
// conduit to a descendant target. hasData is only true for exact target matches.
func GetResumeContext[T any](ctx context.Context) (isResumeTarget bool, hasData bool, data T) {
	ac := getAddressContext(ctx)
	if ac == nil || !ac.isResumeTarget {
		return false, false, data
	}
	if !ac.hasResumeData {
		return true, false, data
	}
	typed, ok := ac.resumeData.(T)
	if !ok {
		return true, false, data
	}
	return true, true, typed
}
