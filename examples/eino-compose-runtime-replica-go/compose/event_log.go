package compose

import (
	"fmt"
	"sync"
	"time"
)

type EventType string

const (
	EventNodeStart    EventType = "node_start"
	EventNodeEnd      EventType = "node_end"
	EventNodeError    EventType = "node_error"
	EventNodeSkipped  EventType = "node_skipped"
	EventGraphStart   EventType = "graph_start"
	EventGraphEnd     EventType = "graph_end"
	EventGraphError   EventType = "graph_error"
	EventChannelReady EventType = "channel_ready"
	EventCheckpoint   EventType = "checkpoint"
	EventMaxStepsHit  EventType = "max_steps_hit"
)

type Event struct {
	Type      EventType `json:"type"`
	Timestamp time.Time `json:"timestamp"`
	NodeKey   string    `json:"node_key,omitempty"`
	GraphName string    `json:"graph_name,omitempty"`
	Step      int       `json:"step,omitempty"`
	Input     any       `json:"input,omitempty"`
	Output    any       `json:"output,omitempty"`
	Error     string    `json:"error,omitempty"`
}

type EventLog struct {
	mu     sync.Mutex
	Events []Event `json:"events"`
}

func NewEventLog() *EventLog {
	return &EventLog{Events: make([]Event, 0)}
}

func (el *EventLog) Log(event Event) {
	el.mu.Lock()
	defer el.mu.Unlock()
	event.Timestamp = time.Now()
	el.Events = append(el.Events, event)
}

func (el *EventLog) LogNodeStart(nodeKey string, step int, input any) {
	el.Log(Event{Type: EventNodeStart, NodeKey: nodeKey, Step: step, Input: input})
}

func (el *EventLog) LogNodeEnd(nodeKey string, step int, output any) {
	el.Log(Event{Type: EventNodeEnd, NodeKey: nodeKey, Step: step, Output: output})
}

func (el *EventLog) LogNodeError(nodeKey string, step int, err error) {
	el.Log(Event{Type: EventNodeError, NodeKey: nodeKey, Step: step, Error: err.Error()})
}

func (el *EventLog) LogNodeSkipped(nodeKey string, step int) {
	el.Log(Event{Type: EventNodeSkipped, NodeKey: nodeKey, Step: step})
}

func (el *EventLog) LogGraphStart(graphName string) {
	el.Log(Event{Type: EventGraphStart, GraphName: graphName})
}

func (el *EventLog) LogGraphEnd(graphName string, step int) {
	el.Log(Event{Type: EventGraphEnd, GraphName: graphName, Step: step})
}

func (el *EventLog) LogGraphError(graphName string, err error) {
	el.Log(Event{Type: EventGraphError, GraphName: graphName, Error: err.Error()})
}

func (el *EventLog) LogMaxStepsHit(graphName string, step int) {
	el.Log(Event{Type: EventMaxStepsHit, GraphName: graphName, Step: step})
}

func (el *EventLog) String() string {
	el.mu.Lock()
	defer el.mu.Unlock()
	var result string
	for i, e := range el.Events {
		result += fmt.Sprintf("[%d] %s %s node=%s step=%d", i, e.Timestamp.Format("15:04:05.000"), e.Type, e.NodeKey, e.Step)
		if e.Error != "" {
			result += fmt.Sprintf(" err=%s", e.Error)
		}
		result += "\n"
	}
	return result
}
