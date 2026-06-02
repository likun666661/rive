package compose

type dependencyState int

const (
	dependencyWaiting dependencyState = iota
	dependencyReady
	dependencySkipped
)

func (s dependencyState) String() string {
	switch s {
	case dependencyWaiting:
		return "Waiting"
	case dependencyReady:
		return "Ready"
	case dependencySkipped:
		return "Skipped"
	default:
		return "Unknown"
	}
}

type dagChannel struct {
	value               any
	values              map[string]any
	hasValue            bool
	controlPredecessors map[string]dependencyState
	dataPredecessors    map[string]bool
	mergeValuesFn       func(map[string]any) (any, error)
}

func newDAGChannel(controlPredecessors []string, dataPredecessors []string) *dagChannel {
	dc := &dagChannel{
		values:              make(map[string]any),
		controlPredecessors: make(map[string]dependencyState),
		dataPredecessors:    make(map[string]bool),
	}
	for _, cp := range controlPredecessors {
		dc.controlPredecessors[cp] = dependencyWaiting
	}
	for _, dp := range dataPredecessors {
		dc.dataPredecessors[dp] = false
	}
	return dc
}

func (dc *dagChannel) reportValues(nodeKey string, value any) {
	dc.values[nodeKey] = value
	if _, ok := dc.dataPredecessors[nodeKey]; ok {
		dc.dataPredecessors[nodeKey] = true
	}
}

func (dc *dagChannel) reportDependency(nodeKey string) {
	if _, ok := dc.controlPredecessors[nodeKey]; ok {
		dc.controlPredecessors[nodeKey] = dependencyReady
	}
}

func (dc *dagChannel) reportSkip(nodeKey string) bool {
	if _, ok := dc.controlPredecessors[nodeKey]; ok {
		dc.controlPredecessors[nodeKey] = dependencySkipped
	}
	allSkipped := true
	for _, state := range dc.controlPredecessors {
		if state != dependencySkipped {
			allSkipped = false
			break
		}
	}
	return allSkipped
}

func (dc *dagChannel) get() (any, bool, error) {
	if dc.hasValue {
		val := dc.value
		dc.value = nil
		dc.hasValue = false
		dc.values = make(map[string]any)
		for k := range dc.dataPredecessors {
			dc.dataPredecessors[k] = false
		}
		return val, true, nil
	}

	allControlReady := true
	for _, state := range dc.controlPredecessors {
		if state == dependencyWaiting {
			allControlReady = false
			break
		}
	}

	allDataReported := true
	for _, reported := range dc.dataPredecessors {
		if !reported {
			allDataReported = false
			break
		}
	}

	if !allControlReady || !allDataReported {
		return nil, false, nil
	}

	if dc.mergeValuesFn != nil && len(dc.values) > 0 {
		merged, err := dc.mergeValuesFn(dc.values)
		if err != nil {
			return nil, false, err
		}
		dc.values = make(map[string]any)
		for k := range dc.dataPredecessors {
			dc.dataPredecessors[k] = false
		}
		return merged, true, nil
	}

	if len(dc.values) == 1 {
		for _, v := range dc.values {
			dc.values = make(map[string]any)
			for k := range dc.dataPredecessors {
				dc.dataPredecessors[k] = false
			}
			return v, true, nil
		}
	}

	if len(dc.values) > 1 {
		result := make(map[string]any)
		for k, v := range dc.values {
			result[k] = v
		}
		dc.values = make(map[string]any)
		for k := range dc.dataPredecessors {
			dc.dataPredecessors[k] = false
		}
		return result, true, nil
	}

	return nil, false, nil
}

func (dc *dagChannel) setMergeConfig(fn func(map[string]any) (any, error)) {
	dc.mergeValuesFn = fn
}
