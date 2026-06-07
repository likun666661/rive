package compose

import (
	"context"
	"fmt"
)

type runner struct {
	chanSubscribeTo     map[string]*chanCall
	successors          map[string][]string
	dataPredecessors    map[string][]string
	controlPredecessors map[string][]string
	inputChannels       *chanCall
	startNodes          []string
	endNodes            []string
	dag                 bool
	pregel              bool
	eager               bool
	maxSteps            int
	graphName           string
	graphInfo           *GraphInfo
	eventLog            *EventLog
	runStepCount        int
	genLocalState       *genLocalStateEntry
	branches            map[string][]*GraphBranch
}

func (r *runner) run(ctx context.Context, input any) (any, error) {
	r.runStepCount = 0
	var err error
	ctx, input, err = restoreCheckPointContext(ctx, input)
	if err != nil {
		return nil, err
	}
	if r.graphName != "" {
		ctx = AppendAddressSegment(ctx, AddressSegmentRunnable, r.graphName)
	}

	if r.genLocalState != nil {
		state := r.genLocalState.factory(ctx)
		ctx = context.WithValue(ctx, r.genLocalState.key, state)
	}

	cm := newChannelManager()
	r.initChannels(cm)

	tm := newTaskManager(r.eventLog)
	if r.eventLog != nil {
		r.eventLog.LogGraphStart(r.graphName)
	}

	r.routeInputToStartNodes(cm, input)

	var lastEndValue any
	var lastEndReady bool
	hasResult := false

	for {
		r.runStepCount++

		if r.pregel && r.runStepCount > r.maxSteps {
			if r.eventLog != nil {
				r.eventLog.LogMaxStepsHit(r.graphName, r.runStepCount)
			}
			return nil, fmt.Errorf("%w: step %d exceeds max %d", ErrExceedMaxSteps, r.runStepCount, r.maxSteps)
		}

		if r.eventLog != nil && r.runStepCount%10 == 0 {
			_ = r.eventLog
		}

		readyNodes := cm.getReadyChannels("")
		if len(readyNodes) == 0 {
			if endVal, ok := cm.getEndChannel(); ok {
				lastEndValue = endVal
				lastEndReady = true
				hasResult = true
			}
			break
		}

		tasks := r.createTasks(ctx, readyNodes)
		if len(tasks) == 0 {
			break
		}

		tm.setStep(r.runStepCount)
		tm.submit(ctx, tasks)

		completedTasks := tm.wait()

		if err := r.resolveCompletedTasks(ctx, cm, completedTasks); err != nil {
			if info, ok := ExtractInterruptInfo(err); ok {
				_ = saveInterruptCheckPoint(ctx, input, info)
			}
			return nil, err
		}

		if endVal, ok := cm.getEndChannel(); ok {
			lastEndValue = endVal
			lastEndReady = true
			hasResult = true
		}
	}

	if hasResult && lastEndReady {
		return lastEndValue, nil
	}

	if !hasResult {
		endVal, ok := cm.getEndChannel()
		if ok {
			return endVal, nil
		}
	}

	return nil, fmt.Errorf("graph %q: no result produced", r.graphName)
}

func (r *runner) initChannels(cm *channelManager) {
	for nodeKey := range r.chanSubscribeTo {
		if nodeKey == START {
			continue
		}

		controlPreds := r.controlPredecessors[nodeKey]
		dataPreds := r.dataPredecessors[nodeKey]

		if r.dag {
			dc := newDAGChannel(
				append([]string{}, controlPreds...),
				append([]string{}, dataPreds...),
			)
			cm.addChannel(nodeKey, dc)
		} else {
			pc := newPregelChannel()
			cm.addChannel(nodeKey, pc)
		}
	}
}

func (r *runner) routeInputToStartNodes(cm *channelManager, input any) {
	startCall := r.chanSubscribeTo[START]
	for _, startNode := range r.startNodes {
		if startCall != nil {
			if fms, ok := startCall.fieldMappings[startNode]; ok && len(fms) > 0 {
				val, err := fieldMap(fms, false, nil)(input)
				if err != nil {
					continue
				}
				cm.updateValues(START, val, map[string]bool{startNode: true})
				cm.updateDependencies(START, map[string]bool{startNode: true})
				continue
			}
		}
		cm.updateValues(START, input, map[string]bool{startNode: true})
		cm.updateDependencies(START, map[string]bool{startNode: true})
	}
}

func (r *runner) createTasks(ctx context.Context, readyNodes map[string]any) []*task {
	tasks := make([]*task, 0, len(readyNodes))

	for nodeKey, input := range readyNodes {
		if nodeKey == END || nodeKey == START {
			continue
		}

		cc := r.chanSubscribeTo[nodeKey]
		if cc == nil {
			continue
		}

		t := &task{
			nodeKey: nodeKey,
			call:    cc,
			input:   input,
			ctx:     AppendAddressSegment(ctx, AddressSegmentNode, nodeKey),
		}
		tasks = append(tasks, t)
	}

	return tasks
}

func (r *runner) resolveCompletedTasks(ctx context.Context, cm *channelManager, completedTasks []*task) error {
	var taskErrs []error
	for _, t := range completedTasks {
		if t.err != nil {
			taskErrs = append(taskErrs, t.err)
		}
	}
	if len(taskErrs) == 1 {
		return taskErrs[0]
	}
	if len(taskErrs) > 1 {
		allInterrupts := true
		for _, err := range taskErrs {
			if _, ok := ExtractInterruptInfo(err); !ok {
				allInterrupts = false
				break
			}
		}
		if allInterrupts {
			return CompositeInterrupt(ctx, "multiple graph nodes interrupted", nil, taskErrs...)
		}
		return taskErrs[0]
	}

	for _, t := range completedTasks {

		cc := t.call
		if cc == nil {
			continue
		}

		output := t.output

		for _, h := range cc.preHandlers {
			var err error
			output, err = h.invoke(output)
			if err != nil {
				continue
			}
		}

		writeTo := make(map[string]bool)
		skipped := make(map[string]bool)
		for k, v := range cc.writeTo {
			writeTo[k] = v
		}

		if branches, ok := r.branches[t.nodeKey]; ok && len(branches) > 0 {
			branch := branches[len(branches)-1]
			matchedTarget, err := branch.condition(ctx, output)
			if err != nil {
				return fmt.Errorf("branch %s condition: %w", t.nodeKey, err)
			}
			if matchedTarget != "" {
				if !branch.branchMap[matchedTarget] {
					return fmt.Errorf("branch %s returned unknown target %q", t.nodeKey, matchedTarget)
				}
				for target := range writeTo {
					if target != matchedTarget {
						skipped[target] = true
					}
				}
				writeTo = map[string]bool{matchedTarget: true}
			}
		}

		if len(skipped) > 0 {
			cm.reportSkip(t.nodeKey, skipped)
		}

		if len(cc.fieldMappings) > 0 {
			for target := range writeTo {
				var val any = output
				if fms, ok := cc.fieldMappings[target]; ok && len(fms) > 0 {
					var err error
					val, err = fieldMap(fms, false, nil)(output)
					if err != nil {
						continue
					}
				}
				cm.updateValues(t.nodeKey, val, map[string]bool{target: true})
			}
		} else {
			cm.updateValues(t.nodeKey, output, writeTo)
		}

		cm.updateDependencies(t.nodeKey, cc.controls)

		for branchTarget := range writeTo {
			_ = branchTarget
		}
	}
	return nil
}
