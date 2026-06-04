package compose

import (
	"context"
	"errors"
	"sync"
)

var ErrCheckPointNotFound = errors.New("checkpoint not found")

// CheckPoint captures the educational subset needed to resume an interrupted
// graph: original input plus interrupt address/state maps.
type CheckPoint struct {
	Input                 any
	InterruptID2Addr      map[string]Address
	InterruptID2State     map[string]InterruptState
	LayerSpecificSnapshot map[string]any
}

func (cp *CheckPoint) clone() *CheckPoint {
	if cp == nil {
		return nil
	}
	out := &CheckPoint{
		Input:                 cp.Input,
		InterruptID2Addr:      make(map[string]Address),
		InterruptID2State:     make(map[string]InterruptState),
		LayerSpecificSnapshot: make(map[string]any),
	}
	for id, addr := range cp.InterruptID2Addr {
		out.InterruptID2Addr[id] = addr.clone()
	}
	for id, state := range cp.InterruptID2State {
		out.InterruptID2State[id] = state
	}
	for k, v := range cp.LayerSpecificSnapshot {
		out.LayerSpecificSnapshot[k] = v
	}
	return out
}

// CheckPointStore persists checkpoints by caller-chosen IDs.
type CheckPointStore interface {
	Get(ctx context.Context, id string) (*CheckPoint, error)
	Set(ctx context.Context, id string, cp *CheckPoint) error
}

// InMemoryCheckPointStore is a deterministic test/teaching store.
type InMemoryCheckPointStore struct {
	mu    sync.Mutex
	items map[string]*CheckPoint
}

func NewInMemoryCheckPointStore() *InMemoryCheckPointStore {
	return &InMemoryCheckPointStore{items: make(map[string]*CheckPoint)}
}

func (s *InMemoryCheckPointStore) Get(ctx context.Context, id string) (*CheckPoint, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	cp, ok := s.items[id]
	if !ok {
		return nil, ErrCheckPointNotFound
	}
	return cp.clone(), nil
}

func (s *InMemoryCheckPointStore) Set(ctx context.Context, id string, cp *CheckPoint) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.items[id] = cp.clone()
	return nil
}

type checkPointStoreCtxKey struct{}
type checkPointIDCtxKey struct{}

func WithCheckPointStore(ctx context.Context, store CheckPointStore) context.Context {
	return context.WithValue(ctx, checkPointStoreCtxKey{}, store)
}

func WithCheckPointID(ctx context.Context, id string) context.Context {
	return context.WithValue(ctx, checkPointIDCtxKey{}, id)
}

func WithCheckPoint(ctx context.Context, id string, store CheckPointStore) context.Context {
	return WithCheckPointID(WithCheckPointStore(ctx, store), id)
}

func checkpointConfig(ctx context.Context) (string, CheckPointStore, bool) {
	id, ok := ctx.Value(checkPointIDCtxKey{}).(string)
	if !ok || id == "" {
		return "", nil, false
	}
	store, ok := ctx.Value(checkPointStoreCtxKey{}).(CheckPointStore)
	if !ok || store == nil {
		return "", nil, false
	}
	return id, store, true
}

func restoreCheckPointContext(ctx context.Context, input any) (context.Context, any, error) {
	id, store, ok := checkpointConfig(ctx)
	if !ok {
		return ctx, input, nil
	}
	cp, err := store.Get(ctx, id)
	if errors.Is(err, ErrCheckPointNotFound) {
		return ctx, input, nil
	}
	if err != nil {
		return ctx, input, err
	}
	ctx = populateInterruptState(ctx, cp.InterruptID2Addr, cp.InterruptID2State)
	return ctx, cp.Input, nil
}

func saveInterruptCheckPoint(ctx context.Context, input any, info *InterruptInfo) error {
	id, store, ok := checkpointConfig(ctx)
	if !ok || info == nil || info.Signal == nil {
		return nil
	}
	idToAddr, idToState := SignalToPersistenceMaps(info.Signal)
	return store.Set(ctx, id, &CheckPoint{
		Input:             input,
		InterruptID2Addr:  idToAddr,
		InterruptID2State: idToState,
	})
}

// MaterializedStream stores a one-shot stream as deterministic values.
type MaterializedStream[T any] struct {
	Items []T
}

// MaterializeStream drains a stream into a checkpoint-safe value.
func MaterializeStream[T any](r PipeStreamReader[T]) *MaterializedStream[T] {
	if r == nil {
		return &MaterializedStream[T]{}
	}
	return &MaterializedStream[T]{Items: drainAll(r)}
}

// RestoreStream turns a materialized stream back into a reader.
func RestoreStream[T any](m *MaterializedStream[T]) PipeStreamReader[T] {
	if m == nil {
		return PipeStreamReaderFromSlice([]T{})
	}
	return PipeStreamReaderFromSlice(m.Items)
}
