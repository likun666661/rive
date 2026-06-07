package compose

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
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

// EventSink receives a durable copy of every EventLog event.
type EventSink interface {
	WriteEvent(event Event) error
}

// eventSinkCloser is implemented by sinks that own resources.
type eventSinkCloser interface {
	Close() error
}

// JSONLEventSink appends one JSON event per line and flushes each write.
type JSONLEventSink struct {
	mu   sync.Mutex
	file *os.File
	enc  *json.Encoder
}

func NewJSONLEventSink(path string) (*JSONLEventSink, error) {
	if dir := filepath.Dir(path); dir != "." && dir != "" {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return nil, err
		}
	}

	file, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	if err != nil {
		return nil, err
	}
	return &JSONLEventSink{file: file, enc: json.NewEncoder(file)}, nil
}

func (s *JSONLEventSink) WriteEvent(event Event) error {
	if s == nil {
		return nil
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.file == nil {
		return os.ErrClosed
	}
	if err := s.enc.Encode(sanitizeEventForJSON(event)); err != nil {
		return err
	}
	return s.file.Sync()
}

func (s *JSONLEventSink) Close() error {
	if s == nil {
		return nil
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.file == nil {
		return nil
	}
	err := s.file.Close()
	s.file = nil
	s.enc = nil
	return err
}

type EventLog struct {
	mu         sync.Mutex
	Events     []Event `json:"events"`
	sinks      []EventSink
	sinkErrors []error
}

func NewEventLog(sinks ...EventSink) *EventLog {
	return &EventLog{Events: make([]Event, 0), sinks: append([]EventSink(nil), sinks...)}
}

func (el *EventLog) AddSink(sink EventSink) {
	if el == nil || sink == nil {
		return
	}
	el.mu.Lock()
	defer el.mu.Unlock()
	el.sinks = append(el.sinks, sink)
}

func (el *EventLog) SinkErrors() []error {
	if el == nil {
		return nil
	}
	el.mu.Lock()
	defer el.mu.Unlock()
	return append([]error(nil), el.sinkErrors...)
}

func (el *EventLog) Close() error {
	if el == nil {
		return nil
	}
	el.mu.Lock()
	defer el.mu.Unlock()
	var errs []error
	for _, sink := range el.sinks {
		closer, ok := sink.(eventSinkCloser)
		if !ok {
			continue
		}
		if err := closer.Close(); err != nil {
			el.sinkErrors = append(el.sinkErrors, err)
			errs = append(errs, err)
		}
	}
	return errors.Join(errs...)
}

func (el *EventLog) Log(event Event) {
	el.mu.Lock()
	defer el.mu.Unlock()
	event.Timestamp = time.Now()
	el.Events = append(el.Events, event)
	for _, sink := range el.sinks {
		if sink == nil {
			continue
		}
		if err := sink.WriteEvent(event); err != nil {
			el.sinkErrors = append(el.sinkErrors, err)
		}
	}
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

func sanitizeEventForJSON(event Event) Event {
	event.Input = sanitizeJSONValue(event.Input)
	event.Output = sanitizeJSONValue(event.Output)
	return event
}

func sanitizeJSONValue(value any) any {
	if value == nil {
		return nil
	}
	data, err := json.Marshal(value)
	if err != nil {
		return fmt.Sprintf("%v", value)
	}
	var decoded any
	if err := json.Unmarshal(data, &decoded); err != nil {
		return fmt.Sprintf("%v", value)
	}
	return decoded
}
