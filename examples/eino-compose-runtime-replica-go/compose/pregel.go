package compose

type pregelChannel struct {
	values        map[string]any
	mergeValuesFn func(map[string]any) (any, error)
}

func newPregelChannel() *pregelChannel {
	return &pregelChannel{
		values: make(map[string]any),
	}
}

func (pc *pregelChannel) reportValues(nodeKey string, value any) {
	pc.values[nodeKey] = value
}

func (pc *pregelChannel) reportDependency(nodeKey string) {
}

func (pc *pregelChannel) reportSkip(nodeKey string) bool {
	return false
}

func (pc *pregelChannel) get() (any, bool, error) {
	if len(pc.values) == 0 {
		return nil, false, nil
	}

	if pc.mergeValuesFn != nil && len(pc.values) > 1 {
		merged, err := pc.mergeValuesFn(pc.values)
		pc.values = make(map[string]any)
		if err != nil {
			return nil, false, err
		}
		return merged, true, nil
	}

	var result any
	for _, v := range pc.values {
		result = v
		break
	}

	pc.values = make(map[string]any)
	return result, true, nil
}

func (pc *pregelChannel) setMergeConfig(fn func(map[string]any) (any, error)) {
	pc.mergeValuesFn = fn
}
