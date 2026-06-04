package compose

import (
	"strings"
	"sync"
	"testing"
)

func TestPipeSendRecv(t *testing.T) {
	sr, sw := NewPipe[int](0)

	go func() {
		sw.Send(1)
		sw.Send(2)
		sw.Send(3)
		sw.Close()
	}()

	var got []int
	for {
		v, ok := sr.Recv()
		if !ok {
			break
		}
		got = append(got, v)
	}

	if len(got) != 3 || got[0] != 1 || got[1] != 2 || got[2] != 3 {
		t.Fatalf("expected [1 2 3], got %v", got)
	}
}

func TestPipeSendAfterClose(t *testing.T) {
	_, sw := NewPipe[int](0)
	sw.Close()

	err := sw.Send(1)
	if err != ErrStreamClosed {
		t.Fatalf("expected ErrStreamClosed, got %v", err)
	}
}

func TestPipeRecvAfterClose(t *testing.T) {
	sr, sw := NewPipe[int](1)
	sw.Send(42)
	sw.Close()

	v, ok := sr.Recv()
	if !ok || v != 42 {
		t.Fatalf("expected (42, true), got (%v, %v)", v, ok)
	}

	_, ok = sr.Recv()
	if ok {
		t.Fatal("expected false after drain")
	}
}

func TestPipeDoubleClose(t *testing.T) {
	_, sw := NewPipe[int](0)
	sw.Close()
	sw.Close()
}

func TestPipeReaderDoubleClose(t *testing.T) {
	sr, _ := NewPipe[int](0)
	sr.Close()
	sr.Close()
}

func TestPipeBuffered(t *testing.T) {
	sr, sw := NewPipe[int](3)

	sw.Send(1)
	sw.Send(2)
	sw.Send(3)
	sw.Close()

	var got []int
	for {
		v, ok := sr.Recv()
		if !ok {
			break
		}
		got = append(got, v)
	}

	if len(got) != 3 {
		t.Fatalf("expected 3 items, got %d: %v", len(got), got)
	}
}

func TestPipeStreamReaderFromSlice(t *testing.T) {
	sr := PipeStreamReaderFromSlice([]string{"a", "b", "c"})

	var got []string
	for {
		v, ok := sr.Recv()
		if !ok {
			break
		}
		got = append(got, v)
	}

	if len(got) != 3 || got[0] != "a" || got[1] != "b" || got[2] != "c" {
		t.Fatalf("expected [a b c], got %v", got)
	}
}

func TestPipeStreamReaderFromSliceEmpty(t *testing.T) {
	sr := PipeStreamReaderFromSlice[int](nil)

	_, ok := sr.Recv()
	if ok {
		t.Fatal("expected false from empty slice stream")
	}
}

func TestPipeStreamReaderFromValue(t *testing.T) {
	sr := PipeStreamReaderFromValue("hello")

	v, ok := sr.Recv()
	if !ok || v != "hello" {
		t.Fatalf("expected (hello, true), got (%v, %v)", v, ok)
	}

	_, ok = sr.Recv()
	if ok {
		t.Fatal("expected false after singleton")
	}
}

func TestPipeStreamReaderFromValueDoubleClose(t *testing.T) {
	sr := PipeStreamReaderFromValue(42)

	v, ok := sr.Recv()
	if !ok || v != 42 {
		t.Fatalf("expected (42, true), got (%v, %v)", v, ok)
	}

	sr.Close()
	sr.Close()

	_, ok = sr.Recv()
	if ok {
		t.Fatal("expected false after drain and close")
	}
}

func TestCopySameData(t *testing.T) {
	sr := PipeStreamReaderFromSlice([]int{10, 20, 30})
	children := Copy(sr, 3)

	if len(children) != 3 {
		t.Fatalf("expected 3 children, got %d", len(children))
	}

	for i, child := range children {
		var got []int
		for {
			v, ok := child.Recv()
			if !ok {
				break
			}
			got = append(got, v)
		}
		if len(got) != 3 || got[0] != 10 || got[1] != 20 || got[2] != 30 {
			t.Fatalf("child %d: expected [10 20 30], got %v", i, got)
		}
	}
}

func TestCopyIndependentChildren(t *testing.T) {
	sr := PipeStreamReaderFromSlice([]string{"x", "y"})
	children := Copy(sr, 2)

	children[0].Close()
	children[0].Close()

	v1, ok1 := children[1].Recv()
	if !ok1 || v1 != "x" {
		t.Fatalf("child 1 should still work after child 0 close, got (%v, %v)", v1, ok1)
	}

	v2, ok2 := children[1].Recv()
	if !ok2 || v2 != "y" {
		t.Fatalf("child 1 second Recv, got (%v, %v)", v2, ok2)
	}
}

func TestCopyDoubleCloseNoPanic(t *testing.T) {
	sr := PipeStreamReaderFromSlice([]int{1, 2, 3})
	children := Copy(sr, 2)

	for _, child := range children {
		child.Close()
		child.Close()
	}
}

func TestCopyZeroChildren(t *testing.T) {
	sr := PipeStreamReaderFromSlice([]int{1, 2, 3})
	children := Copy(sr, 0)

	if len(children) != 0 {
		t.Fatalf("expected 0 children, got %d", len(children))
	}
}

func TestDrainAll(t *testing.T) {
	sr := PipeStreamReaderFromSlice([]int{5, 10, 15})
	items := drainAll(sr)

	if len(items) != 3 || items[0] != 5 || items[1] != 10 || items[2] != 15 {
		t.Fatalf("expected [5 10 15], got %v", items)
	}
}

func TestMerge(t *testing.T) {
	sr1 := PipeStreamReaderFromSlice([]string{"a", "b"})
	sr2 := PipeStreamReaderFromSlice([]string{"c", "d"})
	sr3 := PipeStreamReaderFromSlice([]string{"e"})

	merged := Merge(sr1, sr2, sr3)

	seen := make(map[string]bool)
	count := 0
	for {
		v, ok := merged.Recv()
		if !ok {
			break
		}
		seen[v] = true
		count++
	}

	if count != 5 {
		t.Fatalf("expected 5 items, got %d", count)
	}
	for _, want := range []string{"a", "b", "c", "d", "e"} {
		if !seen[want] {
			t.Fatalf("missing item %q from merge result", want)
		}
	}
}

func TestMergeEmptyReaders(t *testing.T) {
	merged := Merge[int]()

	_, ok := merged.Recv()
	if ok {
		t.Fatal("expected false from empty merge")
	}
}

func TestMergeSingleReader(t *testing.T) {
	sr := PipeStreamReaderFromSlice([]int{1, 2, 3})
	merged := Merge(sr)

	var got []int
	for {
		v, ok := merged.Recv()
		if !ok {
			break
		}
		got = append(got, v)
	}

	if len(got) != 3 {
		t.Fatalf("expected 3 items, got %d: %v", len(got), got)
	}
}

func TestMergeCloseMerged(t *testing.T) {
	sr := PipeStreamReaderFromSlice([]int{1, 2, 3})
	merged := Merge(sr)
	merged.Close()

	merged.Close()
}

func TestConcatFallbackLastChunk(t *testing.T) {
	sr1 := PipeStreamReaderFromSlice([]int{1, 2})
	sr2 := PipeStreamReaderFromSlice([]int{3, 4})
	sr3 := PipeStreamReaderFromSlice([]int{5})

	result := Concat(sr1, sr2, sr3)

	v, ok := result.Recv()
	if !ok || v != 5 {
		t.Fatalf("fallback: expected last chunk 5, got (%v, %v)", v, ok)
	}

	_, ok = result.Recv()
	if ok {
		t.Fatal("expected false after last chunk")
	}
}

func TestConcatRegisteredFunction(t *testing.T) {
	RegisterConcatFunc(func(chunks []string) string {
		return strings.Join(chunks, "")
	})

	sr1 := PipeStreamReaderFromSlice([]string{"Hel", "lo"})
	sr2 := PipeStreamReaderFromSlice([]string{"Wor", "ld"})

	result := Concat(sr1, sr2)

	v, ok := result.Recv()
	if !ok || v != "HelloWorld" {
		t.Fatalf("expected HelloWorld, got (%v, %v)", v, ok)
	}

	_, ok = result.Recv()
	if ok {
		t.Fatal("expected false after concat")
	}
}

func TestConcatEmptyReaders(t *testing.T) {
	result := Concat[int]()

	_, ok := result.Recv()
	if ok {
		t.Fatal("expected false from empty concat")
	}
}

func TestConcatEmptyChunks(t *testing.T) {
	sr1 := PipeStreamReaderFromSlice[int](nil)
	sr2 := PipeStreamReaderFromSlice[int](nil)

	result := Concat(sr1, sr2)

	_, ok := result.Recv()
	if ok {
		t.Fatal("expected false when all chunks are empty")
	}
}

func TestConcatSingleReaderFallback(t *testing.T) {
	sr := PipeStreamReaderFromSlice([]float64{1.1, 2.2, 3.3})
	result := Concat(sr)

	v, ok := result.Recv()
	if !ok || v != 3.3 {
		t.Fatalf("fallback: expected last chunk 3.3, got (%v, %v)", v, ok)
	}
}

func TestRegisterConcatFuncOverwrite(t *testing.T) {
	RegisterConcatFunc(func(chunks []string) string {
		return strings.Join(chunks, ",")
	})

	sr1 := PipeStreamReaderFromSlice([]string{"a", "b"})
	sr2 := PipeStreamReaderFromSlice([]string{"c"})

	result := Concat(sr1, sr2)

	v, ok := result.Recv()
	if !ok || v != "a,b,c" {
		t.Fatalf("expected a,b,c, got (%v, %v)", v, ok)
	}
}

func TestPipeConcurrentSendRecv(t *testing.T) {
	sr, sw := NewPipe[int](0)

	var wg sync.WaitGroup
	const n = 100

	wg.Add(1)
	go func() {
		defer wg.Done()
		for i := 0; i < n; i++ {
			sw.Send(i)
		}
		sw.Close()
	}()

	var got []int
	for {
		v, ok := sr.Recv()
		if !ok {
			break
		}
		got = append(got, v)
	}

	wg.Wait()

	if len(got) != n {
		t.Fatalf("expected %d items, got %d", n, len(got))
	}
}

func TestMergeConcurrentSafety(t *testing.T) {
	readers := make([]PipeStreamReader[int], 5)
	for i := 0; i < 5; i++ {
		idx := i
		sr, sw := NewPipe[int](0)
		go func() {
			for j := 0; j < 10; j++ {
				sw.Send(idx*10 + j)
			}
			sw.Close()
		}()
		readers[i] = sr
	}

	merged := Merge(readers...)

	seen := 0
	for {
		_, ok := merged.Recv()
		if !ok {
			break
		}
		seen++
	}

	if seen != 50 {
		t.Fatalf("expected 50 items from merge, got %d", seen)
	}
}

func TestMergeParentCloseIndependent(t *testing.T) {
	sr1 := PipeStreamReaderFromSlice([]int{1, 2})
	sr2 := PipeStreamReaderFromSlice([]int{3, 4})
	merged := Merge(sr1, sr2)

	v, ok := merged.Recv()
	if !ok {
		t.Fatal("expected first item")
	}
	_ = v

	merged.Close()
	merged.Close()
}

func TestConcatCloseNoPanic(t *testing.T) {
	sr := PipeStreamReaderFromSlice([]int{1, 2})
	result := Concat(sr)
	result.Close()
	result.Close()
}

func TestCopyParentAlreadyConsumed(t *testing.T) {
	sr := PipeStreamReaderFromSlice([]int{1, 2, 3})
	drainAll(sr)
	children := Copy(sr, 2)

	for i, child := range children {
		_, ok := child.Recv()
		if ok {
			t.Fatalf("child %d: expected false from already-consumed parent, got true", i)
		}
	}
}
