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
}

func (r *runner) run(ctx context.Context, input any) (any, error) {
	r.runStepCount = 0

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

		r.resolveCompletedTasks(cm, completedTasks)

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
	for _, startNode := range r.startNodes {
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
		}
		tasks = append(tasks, t)
	}

	return tasks
}

func (r *runner) resolveCompletedTasks(cm *channelManager, completedTasks []*task) {
	for _, t := range completedTasks {
		if t.err != nil {
			continue
		}

		cc := t.call
		if cc == nil {
			continue
		}

		cm.updateValues(t.nodeKey, t.output, cc.writeTo)
		cm.updateDependencies(t.nodeKey, cc.controls)

		for branchTarget := range cc.writeTo {
			_ = branchTarget
		}
	}
}
