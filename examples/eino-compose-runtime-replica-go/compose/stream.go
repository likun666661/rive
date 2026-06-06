package compose

import (
	"errors"
	"reflect"
	"sync"
)

var ErrStreamClosed = errors.New("stream closed")

type PipeStreamReader[T any] interface {
	Recv() (T, bool)
	Close()
}

type PipeStreamWriter[T any] interface {
	Send(T) error
	Close()
}

type stream[T any] struct {
	ch   chan T
	done chan struct{}

	mu     sync.Mutex
	closed bool
}

func newStream[T any](cap int) *stream[T] {
	return &stream[T]{
		ch:   make(chan T, cap),
		done: make(chan struct{}),
	}
}

func (s *stream[T]) closeStream() {
	s.mu.Lock()
	if s.closed {
		s.mu.Unlock()
		return
	}
	s.closed = true
	s.mu.Unlock()
	close(s.done)
}

type pipeReader[T any] struct {
	s *stream[T]
}

func (r *pipeReader[T]) Recv() (T, bool) {
	select {
	case val, ok := <-r.s.ch:
		return val, ok
	case <-r.s.done:
		select {
		case val, ok := <-r.s.ch:
			return val, ok
		default:
			var zero T
			return zero, false
		}
	}
}

func (r *pipeReader[T]) Close() {
	r.s.closeStream()
}

type pipeWriter[T any] struct {
	s *stream[T]
}

func (w *pipeWriter[T]) Send(val T) error {
	select {
	case <-w.s.done:
		return ErrStreamClosed
	case w.s.ch <- val:
		return nil
	}
}

func (w *pipeWriter[T]) Close() {
	w.s.closeStream()
}

func NewPipe[T any](cap int) (PipeStreamReader[T], PipeStreamWriter[T]) {
	s := newStream[T](cap)
	return &pipeReader[T]{s: s}, &pipeWriter[T]{s: s}
}

func PipeStreamReaderFromSlice[T any](items []T) PipeStreamReader[T] {
	s := newStream[T](len(items))
	for _, item := range items {
		s.ch <- item
	}
	close(s.ch)
	return &pipeReader[T]{s: s}
}

func PipeStreamReaderFromValue[T any](item T) PipeStreamReader[T] {
	return PipeStreamReaderFromSlice([]T{item})
}

func drainAll[T any](r PipeStreamReader[T]) []T {
	var items []T
	for {
		item, ok := r.Recv()
		if !ok {
			break
		}
		items = append(items, item)
	}
	return items
}

func Copy[T any](parent PipeStreamReader[T], n int) []PipeStreamReader[T] {
	items := drainAll(parent)
	children := make([]PipeStreamReader[T], n)
	for i := 0; i < n; i++ {
		childItems := make([]T, len(items))
		copy(childItems, items)
		children[i] = PipeStreamReaderFromSlice(childItems)
	}
	return children
}

func Merge[T any](readers ...PipeStreamReader[T]) PipeStreamReader[T] {
	sr, sw := NewPipe[T](0)
	go func() {
		defer sw.Close()
		var wg sync.WaitGroup
		for _, r := range readers {
			wg.Add(1)
			go func(r PipeStreamReader[T]) {
				defer wg.Done()
				for {
					item, ok := r.Recv()
					if !ok {
						return
					}
					if err := sw.Send(item); err != nil {
						return
					}
				}
			}(r)
		}
		wg.Wait()
	}()
	return sr
}

var concatFns sync.Map

func RegisterConcatFunc[T any](fn func([]T) T) {
	var zero T
	concatFns.Store(reflect.TypeOf(zero), fn)
}

func Concat[T any](readers ...PipeStreamReader[T]) PipeStreamReader[T] {
	sr, sw := NewPipe[T](1)
	go func() {
		defer sw.Close()
		var allItems []T
		for _, r := range readers {
			items := drainAll(r)
			allItems = append(allItems, items...)
		}
		if len(allItems) == 0 {
			return
		}
		var zero T
		t := reflect.TypeOf(zero)
		if fn, ok := concatFns.Load(t); ok {
			result := reflect.ValueOf(fn).Call([]reflect.Value{reflect.ValueOf(allItems)})
			sw.Send(result[0].Interface().(T))
			return
		}
		if fn, ok := concatFuncRegistry.Load(t); ok {
			results := reflect.ValueOf(fn).Call([]reflect.Value{reflect.ValueOf(allItems)})
			result := results[0].Interface().(T)
			var err error
			if !results[1].IsNil() {
				err = results[1].Interface().(error)
			}
			if err == nil {
				sw.Send(result)
			}
			return
		}
		sw.Send(allItems[len(allItems)-1])
	}()
	return sr
}
