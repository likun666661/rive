package compose

import (
	"context"
	"fmt"
	"sync"
)

type channel interface {
	reportValues(nodeKey string, value any)
	reportDependency(nodeKey string)
	reportSkip(nodeKey string) bool
	get() (any, bool, error)
	setMergeConfig(fn func(map[string]any) (any, error))
}

type chanCall struct {
	nodeKey       string
	action        *composableRunnable
	writeTo       map[string]bool
	controls      map[string]bool
	fieldMappings map[string][]*FieldMapping
	preHandlers   []handlerPair
}

type channelManager struct {
	channels map[string]channel
}

func newChannelManager() *channelManager {
	return &channelManager{
		channels: make(map[string]channel),
	}
}

func (cm *channelManager) addChannel(nodeKey string, ch channel) {
	cm.channels[nodeKey] = ch
}

func (cm *channelManager) updateValues(completedNodeKey string, value any, targets map[string]bool) {
	for target := range targets {
		if ch, ok := cm.channels[target]; ok {
			ch.reportValues(completedNodeKey, value)
		}
	}
}

func (cm *channelManager) updateDependencies(completedNodeKey string, targets map[string]bool) {
	for target := range targets {
		if ch, ok := cm.channels[target]; ok {
			ch.reportDependency(completedNodeKey)
		}
	}
}

func (cm *channelManager) reportSkip(nodeKey string, targets map[string]bool) {
	for target := range targets {
		if ch, ok := cm.channels[target]; ok {
			ch.reportSkip(nodeKey)
		}
	}
}

func (cm *channelManager) getReadyChannels(exceptNode string) map[string]any {
	ready := make(map[string]any)
	for nodeKey, ch := range cm.channels {
		if nodeKey == exceptNode || nodeKey == END {
			continue
		}
		val, ok, err := ch.get()
		if err != nil {
			continue
		}
		if ok {
			ready[nodeKey] = val
		}
	}
	return ready
}

func (cm *channelManager) getEndChannel() (any, bool) {
	if ch, ok := cm.channels[END]; ok {
		val, ready, _ := ch.get()
		return val, ready
	}
	return nil, false
}

type task struct {
	nodeKey string
	call    *chanCall
	input   any
	ctx     context.Context
	output  any
	done    chan struct{}
	err     error
}

type taskManager struct {
	mu           sync.Mutex
	doneTasks    []*task
	runningTasks map[string]*task
	eventLog     *EventLog
	step         int
}

func newTaskManager(eventLog *EventLog) *taskManager {
	return &taskManager{
		doneTasks:    make([]*task, 0),
		runningTasks: make(map[string]*task),
		eventLog:     eventLog,
	}
}

func (tm *taskManager) setStep(step int) {
	tm.step = step
}

func (tm *taskManager) submit(ctx context.Context, tasks []*task) {
	var wg sync.WaitGroup
	for _, t := range tasks {
		t.ctx = ctx
		t.done = make(chan struct{}, 1)

		tm.mu.Lock()
		tm.runningTasks[t.nodeKey] = t
		tm.mu.Unlock()

		if tm.eventLog != nil {
			tm.eventLog.LogNodeStart(t.nodeKey, tm.step, t.input)
		}

		wg.Add(1)
		go func(tt *task) {
			defer wg.Done()
			defer close(tt.done)

			if tt.call == nil || tt.call.action == nil {
				tt.err = fmt.Errorf("task %s: no runnable action", tt.nodeKey)
				if tm.eventLog != nil {
					tm.eventLog.LogNodeError(tt.nodeKey, tm.step, tt.err)
				}
				return
			}

			output, err := tt.call.action.invoke(ctx, tt.input)
			tt.output = output
			tt.err = err

			if err != nil {
				if tm.eventLog != nil {
					tm.eventLog.LogNodeError(tt.nodeKey, tm.step, err)
				}
			} else {
				if tm.eventLog != nil {
					tm.eventLog.LogNodeEnd(tt.nodeKey, tm.step, output)
				}
			}

			tm.mu.Lock()
			tm.doneTasks = append(tm.doneTasks, tt)
			delete(tm.runningTasks, tt.nodeKey)
			tm.mu.Unlock()
		}(t)
	}
	wg.Wait()
}

func (tm *taskManager) wait() []*task {
	tm.mu.Lock()
	done := tm.doneTasks
	tm.doneTasks = make([]*task, 0)
	tm.mu.Unlock()
	return done
}

func (tm *taskManager) hasRunningTasks() bool {
	tm.mu.Lock()
	defer tm.mu.Unlock()
	return len(tm.runningTasks) > 0
}
