package compose

import (
	"context"
	"io"
	"strings"
	"testing"
)

func sliceStreamReader[T any](items ...T) StreamReader[T] {
	return &sliceStream[T]{items: items}
}

type sliceStream[T any] struct {
	items []T
	pos   int
}

func (s *sliceStream[T]) Recv() (T, error) {
	if s.pos >= len(s.items) {
		var zero T
		return zero, io.EOF
	}
	v := s.items[s.pos]
	s.pos++
	return v, nil
}

func TestInvokeOnlyStreamFallback(t *testing.T) {
	l := InvokableLambda(func(ctx context.Context, input string) (string, error) {
		return "hello " + input, nil
	})
	cr := l.GetRunnable()

	sr, err := cr.stream(context.Background(), "world")
	if err != nil {
		t.Fatal(err)
	}
	wr, ok := sr.(streamReader)
	if !ok {
		t.Fatalf("stream result should be streamReader, got %T", sr)
	}

	v, err := wr.Recv()
	if err != nil {
		t.Fatal(err)
	}
	if v != "hello world" {
		t.Fatalf("expected 'hello world', got %v", v)
	}

	_, err = wr.Recv()
	if err != io.EOF {
		t.Fatalf("expected EOF after single element, got %v", err)
	}
}

func TestStreamOnlyInvokeFallbackWithConcat(t *testing.T) {
	l := StreamableLambda(func(ctx context.Context, input string) (StreamReader[string], error) {
		parts := strings.Split(input, ",")
		return sliceStreamReader(parts...), nil
	})
	cr := l.GetRunnable()

	out, err := cr.invoke(context.Background(), "a,b,c")
	if err != nil {
		t.Fatal(err)
	}

	items, ok := out.([]any)
	if !ok {
		t.Fatalf("expected []any from collected stream, got %T", out)
	}
	if len(items) != 3 {
		t.Fatalf("expected 3 items, got %d", len(items))
	}
	if items[0] != "a" || items[1] != "b" || items[2] != "c" {
		t.Fatalf("expected [a b c], got %v", items)
	}
}

func TestTransformFallbackToInvoke(t *testing.T) {
	l := TransformableLambda(func(ctx context.Context, input StreamReader[string]) (StreamReader[string], error) {
		var results []string
		for {
			v, err := input.Recv()
			if err == io.EOF {
				break
			}
			if err != nil {
				return nil, err
			}
			results = append(results, "trans_"+v)
		}
		return sliceStreamReader(results...), nil
	})
	cr := l.GetRunnable()

	out, err := cr.invoke(context.Background(), "hello")
	if err != nil {
		t.Fatal(err)
	}
	if out != "trans_hello" {
		t.Fatalf("expected 'trans_hello', got %v", out)
	}
}

func TestTransformFallbackToStream(t *testing.T) {
	l := TransformableLambda(func(ctx context.Context, input StreamReader[string]) (StreamReader[string], error) {
		var results []string
		for {
			v, err := input.Recv()
			if err == io.EOF {
				break
			}
			if err != nil {
				return nil, err
			}
			results = append(results, "T:"+v)
		}
		return sliceStreamReader(results...), nil
	})
	cr := l.GetRunnable()

	srRaw, err := cr.stream(context.Background(), "hello")
	if err != nil {
		t.Fatal(err)
	}
	wr, ok := srRaw.(streamReader)
	if !ok {
		t.Fatalf("expected streamReader, got %T", srRaw)
	}

	v, err := wr.Recv()
	if err != nil {
		t.Fatal(err)
	}
	if v != "T:hello" {
		t.Fatalf("expected 'T:hello', got %v", v)
	}
}

func TestTransformFallbackToCollect(t *testing.T) {
	l := TransformableLambda(func(ctx context.Context, input StreamReader[string]) (StreamReader[int], error) {
		count := 0
		for {
			_, err := input.Recv()
			if err == io.EOF {
				break
			}
			if err != nil {
				return nil, err
			}
			count++
		}
		return sliceStreamReader(count), nil
	})
	cr := l.GetRunnable()

	inputSR := streamFromItems("a", "b", "c")
	out, err := cr.collect(context.Background(), inputSR)
	if err != nil {
		t.Fatal(err)
	}
	if out != 3 {
		t.Fatalf("expected 3, got %v", out)
	}
}

func TestAllFourModesNative(t *testing.T) {
	invokeFn := func(ctx context.Context, input string) (string, error) {
		return "invoke:" + input, nil
	}
	streamFn := func(ctx context.Context, input string) (StreamReader[string], error) {
		return sliceStreamReader("s1_"+input, "s2_"+input), nil
	}
	collectFn := func(ctx context.Context, input StreamReader[string]) (string, error) {
		var parts []string
		for {
			v, err := input.Recv()
			if err == io.EOF {
				break
			}
			if err != nil {
				return "", err
			}
			parts = append(parts, v)
		}
		return "collected:" + strings.Join(parts, "+"), nil
	}
	transformFn := func(ctx context.Context, input StreamReader[string]) (StreamReader[string], error) {
		var results []string
		for {
			v, err := input.Recv()
			if err == io.EOF {
				break
			}
			if err != nil {
				return nil, err
			}
			results = append(results, "t:"+v)
		}
		return sliceStreamReader(results...), nil
	}

	cr := &composableRunnable{
		i: func(ctx context.Context, input any) (any, error) {
			s, ok := input.(string)
			if !ok {
				return nil, nil
			}
			return invokeFn(ctx, s)
		},
		s: func(ctx context.Context, input any) (any, error) {
			s, ok := input.(string)
			if !ok {
				return nil, nil
			}
			sr, err := streamFn(ctx, s)
			if err != nil {
				return nil, err
			}
			return &typedStreamWrapper[string]{inner: sr}, nil
		},
		c: func(ctx context.Context, input any) (any, error) {
			wr, ok := input.(streamReader)
			if !ok {
				return nil, nil
			}
			typedSR := &untypedStreamWrapper[string]{inner: wr}
			return collectFn(ctx, typedSR)
		},
		t: func(ctx context.Context, input any) (any, error) {
			wr, ok := input.(streamReader)
			if !ok {
				return nil, nil
			}
			typedSR := &untypedStreamWrapper[string]{inner: wr}
			sr, err := transformFn(ctx, typedSR)
			if err != nil {
				return nil, err
			}
			return &typedStreamWrapper[string]{inner: sr}, nil
		},
	}

	out, err := cr.invoke(context.Background(), "test")
	if err != nil {
		t.Fatal("invoke:", err)
	}
	if out != "invoke:test" {
		t.Fatalf("invoke: expected 'invoke:test', got %v", out)
	}

	srRaw, err := cr.stream(context.Background(), "test")
	if err != nil {
		t.Fatal("stream:", err)
	}
	wr := srRaw.(streamReader)
	v1, _ := wr.Recv()
	v2, _ := wr.Recv()
	_, eofErr := wr.Recv()
	if v1 != "s1_test" || v2 != "s2_test" || eofErr != io.EOF {
		t.Fatalf("stream: expected s1_test, s2_test, EOF; got %v, %v, %v", v1, v2, eofErr)
	}

	inputSR := streamFromItems("x", "y", "z")
	collectedOut, err := cr.collect(context.Background(), inputSR)
	if err != nil {
		t.Fatal("collect:", err)
	}
	if collectedOut != "collected:x+y+z" {
		t.Fatalf("collect: expected 'collected:x+y+z', got %v", collectedOut)
	}

	transformInputSR := streamFromItems("x", "y", "z")
	transformSRRaw, err := cr.transform(context.Background(), transformInputSR)
	if err != nil {
		t.Fatal("transform:", err)
	}
	tr := transformSRRaw.(streamReader)
	tv1, _ := tr.Recv()
	tv2, _ := tr.Recv()
	tv3, _ := tr.Recv()
	_, tEOF := tr.Recv()
	if tv1 != "t:x" || tv2 != "t:y" || tv3 != "t:z" || tEOF != io.EOF {
		t.Fatalf("transform: expected t:x, t:y, t:z, EOF; got %v, %v, %v, %v", tv1, tv2, tv3, tEOF)
	}
}

func TestUnsupportedModeError(t *testing.T) {
	cr := &composableRunnable{}

	_, err := cr.invoke(context.Background(), "input")
	if err == nil {
		t.Fatal("expected error for unsupported Invoke")
	}

	_, err = cr.stream(context.Background(), "input")
	if err == nil {
		t.Fatal("expected error for unsupported Stream")
	}

	inputSR := streamFromItems("input")
	_, err = cr.collect(context.Background(), inputSR)
	if err == nil {
		t.Fatal("expected error for unsupported Collect")
	}

	_, err = cr.transform(context.Background(), inputSR)
	if err == nil {
		t.Fatal("expected error for unsupported Transform")
	}
}

func TestInvokeFallbackPriority(t *testing.T) {
	l := StreamableLambda(func(ctx context.Context, input string) (StreamReader[string], error) {
		return sliceStreamReader("from_stream:" + input), nil
	})
	cr := l.GetRunnable()
	out, err := cr.invoke(context.Background(), "test")
	if err != nil {
		t.Fatal(err)
	}
	if out != "from_stream:test" {
		t.Fatalf("expected 'from_stream:test' (by Stream), got %v", out)
	}

	l2 := CollectableLambda(func(ctx context.Context, input StreamReader[string]) (string, error) {
		v, err := input.Recv()
		if err != nil {
			return "", err
		}
		return "from_collect:" + v, nil
	})
	cr2 := l2.GetRunnable()
	out2, err := cr2.invoke(context.Background(), "test2")
	if err != nil {
		t.Fatal(err)
	}
	if out2 != "from_collect:test2" {
		t.Fatalf("expected 'from_collect:test2' (by Collect), got %v", out2)
	}

	l3 := TransformableLambda(func(ctx context.Context, input StreamReader[string]) (StreamReader[string], error) {
		v, err := input.Recv()
		if err != nil {
			return nil, err
		}
		return sliceStreamReader("from_transform:" + v), nil
	})
	cr3 := l3.GetRunnable()
	out3, err := cr3.invoke(context.Background(), "test3")
	if err != nil {
		t.Fatal(err)
	}
	if out3 != "from_transform:test3" {
		t.Fatalf("expected 'from_transform:test3' (by Transform), got %v", out3)
	}
}

func TestStreamFallbackPriority(t *testing.T) {
	l := TransformableLambda(func(ctx context.Context, input StreamReader[string]) (StreamReader[string], error) {
		v, err := input.Recv()
		if err != nil {
			return nil, err
		}
		return sliceStreamReader("t:" + v), nil
	})
	cr := l.GetRunnable()
	srRaw, err := cr.stream(context.Background(), "test")
	if err != nil {
		t.Fatal(err)
	}
	wr := srRaw.(streamReader)
	v, _ := wr.Recv()
	if v != "t:test" {
		t.Fatalf("expected 't:test' (by Transform), got %v", v)
	}

	l2 := InvokableLambda(func(ctx context.Context, input string) (string, error) {
		return "i:" + input, nil
	})
	cr2 := l2.GetRunnable()
	srRaw2, err := cr2.stream(context.Background(), "test2")
	if err != nil {
		t.Fatal(err)
	}
	wr2 := srRaw2.(streamReader)
	v2, _ := wr2.Recv()
	if v2 != "i:test2" {
		t.Fatalf("expected 'i:test2' (by Invoke), got %v", v2)
	}

	l3 := CollectableLambda(func(ctx context.Context, input StreamReader[string]) (string, error) {
		v, err := input.Recv()
		if err != nil {
			return "", err
		}
		return "c:" + v, nil
	})
	cr3 := l3.GetRunnable()
	srRaw3, err := cr3.stream(context.Background(), "test3")
	if err != nil {
		t.Fatal(err)
	}
	wr3 := srRaw3.(streamReader)
	v3, _ := wr3.Recv()
	if v3 != "c:test3" {
		t.Fatalf("expected 'c:test3' (by Collect), got %v", v3)
	}
}

func TestCollectFallbackPriority(t *testing.T) {
	l := TransformableLambda(func(ctx context.Context, input StreamReader[string]) (StreamReader[string], error) {
		v, err := input.Recv()
		if err != nil {
			return nil, err
		}
		return sliceStreamReader("c_t:" + v), nil
	})
	cr := l.GetRunnable()
	out, err := cr.collect(context.Background(), streamFromItems("input"))
	if err != nil {
		t.Fatal(err)
	}
	if out != "c_t:input" {
		t.Fatalf("expected 'c_t:input' (by Transform), got %v", out)
	}

	l2 := InvokableLambda(func(ctx context.Context, input string) (string, error) {
		return "c_i:" + input, nil
	})
	cr2 := l2.GetRunnable()
	out2, err := cr2.collect(context.Background(), streamFromItems("hello"))
	if err != nil {
		t.Fatal(err)
	}
	if out2 != "c_i:hello" {
		t.Fatalf("expected 'c_i:hello' (by Invoke), got %v", out2)
	}

	l3 := StreamableLambda(func(ctx context.Context, input string) (StreamReader[string], error) {
		return sliceStreamReader("c_s:" + input), nil
	})
	cr3 := l3.GetRunnable()
	out3, err := cr3.collect(context.Background(), streamFromItems("world"))
	if err != nil {
		t.Fatal(err)
	}
	if out3 != "c_s:world" {
		t.Fatalf("expected 'c_s:world' (by Stream), got %v", out3)
	}
}

func TestTransformFallbackPriority(t *testing.T) {
	l := StreamableLambda(func(ctx context.Context, input string) (StreamReader[string], error) {
		return sliceStreamReader("t_s:" + input), nil
	})
	cr := l.GetRunnable()
	srRaw, err := cr.transform(context.Background(), streamFromItems("a", "b"))
	if err != nil {
		t.Fatal(err)
	}
	wr := srRaw.(streamReader)
	v1, _ := wr.Recv()
	v2, _ := wr.Recv()
	if v1 != "t_s:a" || v2 != "t_s:b" {
		t.Fatalf("expected ['t_s:a', 't_s:b'] (by Stream), got [%v, %v]", v1, v2)
	}

	l2 := CollectableLambda(func(ctx context.Context, input StreamReader[string]) (string, error) {
		var parts []string
		for {
			v, err := input.Recv()
			if err == io.EOF {
				break
			}
			if err != nil {
				return "", err
			}
			parts = append(parts, v)
		}
		return "joined:" + strings.Join(parts, ","), nil
	})
	cr2 := l2.GetRunnable()
	srRaw2, err := cr2.transform(context.Background(), streamFromItems("a", "b"))
	if err != nil {
		t.Fatal(err)
	}
	wr2 := srRaw2.(streamReader)
	v3, _ := wr2.Recv()
	if v3 != "joined:a,b" {
		t.Fatalf("expected 'joined:a,b' (by Collect), got %v", v3)
	}

	l3 := InvokableLambda(func(ctx context.Context, input string) (string, error) {
		return "t_i:" + input, nil
	})
	cr3 := l3.GetRunnable()
	srRaw3, err := cr3.transform(context.Background(), streamFromItems("x"))
	if err != nil {
		t.Fatal(err)
	}
	wr3 := srRaw3.(streamReader)
	v4, _ := wr3.Recv()
	if v4 != "t_i:x" {
		t.Fatalf("expected 't_i:x' (by Invoke), got %v", v4)
	}
}

func TestCollectableLambdaNative(t *testing.T) {
	l := CollectableLambda(func(ctx context.Context, input StreamReader[string]) (int, error) {
		count := 0
		for {
			_, err := input.Recv()
			if err == io.EOF {
				break
			}
			if err != nil {
				return 0, err
			}
			count++
		}
		return count, nil
	})
	cr := l.GetRunnable()

	out, err := cr.collect(context.Background(), streamFromItems("a", "b", "c", "d"))
	if err != nil {
		t.Fatal(err)
	}
	if out != 4 {
		t.Fatalf("expected 4, got %v", out)
	}
}

func TestGraphRunnableStreamFallback(t *testing.T) {
	g := NewGraph[string, string]()
	l := InvokableLambda(func(ctx context.Context, input string) (string, error) {
		return "graph:" + input, nil
	})
	if err := g.AddLambdaNode("step1", l); err != nil {
		t.Fatal(err)
	}
	if err := g.AddEdge(START, "step1"); err != nil {
		t.Fatal(err)
	}
	if err := g.AddEdge("step1", END); err != nil {
		t.Fatal(err)
	}

	r, err := g.Compile(context.Background())
	if err != nil {
		t.Fatal(err)
	}

	out, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatal(err)
	}
	if out != "graph:hello" {
		t.Fatalf("expected 'graph:hello', got %v", out)
	}

	sr, err := r.Stream(context.Background(), "world")
	if err != nil {
		t.Fatal(err)
	}
	v, err := sr.Recv()
	if err != nil {
		t.Fatal(err)
	}
	if v != "graph:world" {
		t.Fatalf("expected 'graph:world', got %v", v)
	}

	collectOut, err := r.Collect(context.Background(), sliceStreamReader("collect_test"))
	if err != nil {
		t.Fatal(err)
	}
	if collectOut != "graph:collect_test" {
		t.Fatalf("expected 'graph:collect_test', got %v", collectOut)
	}

	tr, err := r.Transform(context.Background(), sliceStreamReader("transform_test"))
	if err != nil {
		t.Fatal(err)
	}
	tv, err := tr.Recv()
	if err != nil {
		t.Fatal(err)
	}
	if tv != "graph:transform_test" {
		t.Fatalf("expected 'graph:transform_test', got %v", tv)
	}
}
