package compose

import (
	"context"
	"strings"
	"testing"
)

func TestChainLinear(t *testing.T) {
	chain := NewChain[string, string]()

	chain.
		AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return strings.ToUpper(in), nil
		})).
		AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return "[" + in + "]", nil
		}))

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if out != "[HELLO]" {
		t.Fatalf("expected [HELLO], got %q", out)
	}
}

func TestChainParallel(t *testing.T) {
	chain := NewChain[string, map[string]any]()

	parallel := NewParallel()
	parallel.
		AddLambda("upper", InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return strings.ToUpper(in), nil
		})).
		AddLambda("lower", InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return strings.ToLower(in), nil
		}))

	chain.AppendParallel(parallel)

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "Hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if out["upper"] != "HELLO" {
		t.Fatalf("expected upper=HELLO, got %v", out["upper"])
	}
	if out["lower"] != "hello" {
		t.Fatalf("expected lower=hello, got %v", out["lower"])
	}
}

func TestChainParallelDuplicateKey(t *testing.T) {
	p := NewParallel()
	twice := InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in + in, nil
	})

	p.AddLambda("dup", twice)
	p.AddLambda("dup", twice)

	if p.Error() == nil {
		t.Fatal("expected error for duplicate output key")
	}
	if !strings.Contains(p.Error().Error(), "duplicate") {
		t.Fatalf("expected duplicate key error, got: %v", p.Error())
	}
}

func TestChainBranch(t *testing.T) {
	chain := NewChain[string, string]()

	branchCond := func(ctx context.Context, in string) (string, error) {
		if len(in) > 5 {
			return "long", nil
		}
		return "short", nil
	}

	chain.
		AppendBranch(NewChainBranch(branchCond).
			AddLambda("long", InvokableLambda(func(ctx context.Context, in string) (string, error) {
				return "LONG:" + in, nil
			})).
			AddLambda("short", InvokableLambda(func(ctx context.Context, in string) (string, error) {
				return "SHORT:" + in, nil
			})),
		)

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "hello-world")
	if err != nil {
		t.Fatalf("Invoke for long input failed: %v", err)
	}
	if out != "LONG:hello-world" {
		t.Fatalf("expected LONG:hello-world, got %q", out)
	}

	out, err = r.Invoke(context.Background(), "hi")
	if err != nil {
		t.Fatalf("Invoke for short input failed: %v", err)
	}
	if out != "SHORT:hi" {
		t.Fatalf("expected SHORT:hi, got %q", out)
	}
}

func TestChainBranchWithPassthrough(t *testing.T) {
	chain := NewChain[string, string]()

	branchCond := func(ctx context.Context, in string) (string, error) {
		if len(in) > 5 {
			return "long", nil
		}
		return "short", nil
	}

	chain.
		AppendBranch(NewChainBranch(branchCond).
			AddLambda("long", InvokableLambda(func(ctx context.Context, in string) (string, error) {
				return "LONG:" + in, nil
			})).
			AddLambda("short", InvokableLambda(func(ctx context.Context, in string) (string, error) {
				return "SHORT:" + in, nil
			})),
		).
		AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return "result=" + in, nil
		}))

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "hello-world")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if out != "result=LONG:hello-world" {
		t.Fatalf("expected result=LONG:hello-world, got %q", out)
	}
}

func TestChainMultiBranch(t *testing.T) {
	chain := NewChain[string, map[string]any]()

	cond := func(ctx context.Context, in string) (map[string]bool, error) {
		return map[string]bool{"path_a": true, "path_b": true}, nil
	}

	chain.
		AppendBranch(NewChainMultiBranch(cond).
			AddLambda("path_a", InvokableLambda(func(ctx context.Context, in string) (string, error) {
				return "A:" + in, nil
			})).
			AddLambda("path_b", InvokableLambda(func(ctx context.Context, in string) (string, error) {
				return "B:" + in, nil
			})).
			AddLambda("path_c", InvokableLambda(func(ctx context.Context, in string) (string, error) {
				return "C:" + in, nil
			})),
		)

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if out["path_a"] != "A:hello" {
		t.Fatalf("expected path_a=A:hello, got %v", out["path_a"])
	}
	if out["path_b"] != "B:hello" {
		t.Fatalf("expected path_b=B:hello, got %v", out["path_b"])
	}
	if _, ok := out["path_c"]; ok {
		t.Fatalf("path_c should not be in output, got %v", out["path_c"])
	}
}

func TestChainEmptyCompile(t *testing.T) {
	chain := NewChain[string, string]()
	_, err := chain.Compile(context.Background())
	if err == nil {
		t.Fatal("expected error from empty chain compile")
	}
	if !strings.Contains(err.Error(), "pre node keys not set") {
		t.Fatalf("expected 'pre node keys not set' error, got: %v", err)
	}
}

func TestChainEmptyParallel(t *testing.T) {
	chain := NewChain[string, string]()
	parallel := NewParallel()
	chain.AppendParallel(parallel)
	_, err := chain.Compile(context.Background())
	if err == nil {
		t.Fatal("expected error from empty parallel append")
	}
	if !strings.Contains(err.Error(), "not enough nodes") {
		t.Fatalf("expected 'not enough nodes' error, got: %v", err)
	}
}

func TestChainSingleNodeBranch(t *testing.T) {
	chain := NewChain[string, string]()

	cond := func(ctx context.Context, in string) (string, error) {
		return "only", nil
	}

	chain.AppendBranch(NewChainBranch(cond).
		AddLambda("only", InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return in, nil
		})))

	_, err := chain.Compile(context.Background())
	if err == nil {
		t.Fatal("expected error from branch with single node")
	}
	if !strings.Contains(err.Error(), "nodeList length") {
		t.Fatalf("expected 'nodeList length' error, got: %v", err)
	}
}

func TestChainNilBranchCondition(t *testing.T) {
	cb := NewChainBranch[string](nil)
	if cb.Error() == nil {
		t.Fatal("expected error from nil branch condition")
	}
	if !strings.Contains(cb.Error().Error(), "condition is nil") {
		t.Fatalf("expected 'condition is nil' error, got: %v", cb.Error())
	}

	cb2 := NewChainMultiBranch[string](nil)
	if cb2.Error() == nil {
		t.Fatal("expected error from nil multi-branch condition")
	}
	if !strings.Contains(cb2.Error().Error(), "condition is nil") {
		t.Fatalf("expected 'condition is nil' error, got: %v", cb2.Error())
	}
}

func TestChainAppendGraph(t *testing.T) {
	subChain := NewChain[string, string]()
	subChain.
		AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return "sub:" + in, nil
		}))

	parentChain := NewChain[string, string]()
	parentChain.
		AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return "pre:" + in, nil
		})).
		AppendGraph(subChain)

	r, err := parentChain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if out != "sub:pre:hello" {
		t.Fatalf("expected sub:pre:hello, got %q", out)
	}
}

func TestChainPreNodeKeysTracking(t *testing.T) {
	chain := NewChain[string, string]()

	if len(chain.preNodeKeys) != 0 {
		t.Fatalf("initial preNodeKeys should be empty, got %v", chain.preNodeKeys)
	}

	chain.AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return strings.ToUpper(in), nil
	}))

	if len(chain.preNodeKeys) != 1 || !strings.HasPrefix(chain.preNodeKeys[0], "node_") {
		t.Fatalf("after first Append, preNodeKeys should be [node_0], got %v", chain.preNodeKeys)
	}

	chain.AppendPassthrough()
	if len(chain.preNodeKeys) != 1 || !strings.HasPrefix(chain.preNodeKeys[0], "node_") {
		t.Fatalf("after passthrough, preNodeKeys should be single node, got %v", chain.preNodeKeys)
	}

	parallel := NewParallel()
	parallel.
		AddLambda("a", InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return in, nil
		})).
		AddLambda("b", InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return in, nil
		}))
	chain.AppendParallel(parallel)

	if len(chain.preNodeKeys) != 1 {
		t.Fatalf("after parallel, preNodeKeys should have 1 merge node, got %d", len(chain.preNodeKeys))
	}
	if !strings.Contains(chain.preNodeKeys[0], "_merge") {
		t.Fatalf("expected merge node key after parallel, got %s", chain.preNodeKeys[0])
	}
}

func TestChainParallelWithPassthrough(t *testing.T) {
	chain := NewChain[string, map[string]any]()

	parallel := NewParallel()
	parallel.
		AddLambda("upper", InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return strings.ToUpper(in), nil
		})).
		AddPassthrough("identity")

	chain.AppendPassthrough().AppendParallel(parallel)

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "Test")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if len(out) != 2 {
		t.Fatalf("expected 2 outputs from parallel, got %d: %v", len(out), out)
	}
	if out["upper"] != "TEST" {
		t.Fatalf("expected upper=TEST, got %v", out["upper"])
	}
	if out["identity"] != "Test" {
		t.Fatalf("expected identity=Test, got %v", out["identity"])
	}
}

func TestChainMultipleAppend(t *testing.T) {
	chain := NewChain[string, string]()

	chain.
		AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return strings.ToUpper(in), nil
		})).
		AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return "[" + in + "]", nil
		})).
		AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return "{" + in + "}", nil
		}))

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if out != "{[HELLO]}" {
		t.Fatalf("expected {[HELLO]}, got %q", out)
	}
}

func TestChainParallelAddGraph(t *testing.T) {
	subChain := NewChain[string, string]()
	subChain.
		AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return "sub_" + strings.ToUpper(in), nil
		}))

	chain := NewChain[string, map[string]any]()

	parallel := NewParallel()
	parallel.
		AddLambda("direct", InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return strings.ToLower(in), nil
		})).
		AddGraph("wrapped", subChain)

	chain.AppendParallel(parallel)

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "Hello")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}

	if len(out) != 2 {
		t.Fatalf("expected 2 outputs from parallel, got %d: %v", len(out), out)
	}
	if out["direct"] != "hello" {
		t.Fatalf("expected direct=hello, got %v", out["direct"])
	}
	if out["wrapped"] != "sub_HELLO" {
		t.Fatalf("expected wrapped=sub_HELLO, got %v", out["wrapped"])
	}
}

func TestChainBranchAddGraph(t *testing.T) {
	subChain := NewChain[string, string]()
	subChain.
		AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return "SUB:" + strings.ToUpper(in), nil
		}))

	chain := NewChain[string, string]()

	cond := func(ctx context.Context, in string) (string, error) {
		if len(in) > 5 {
			return "long", nil
		}
		return "short", nil
	}

	chain.
		AppendBranch(NewChainBranch(cond).
			AddGraph("long", subChain).
			AddLambda("short", InvokableLambda(func(ctx context.Context, in string) (string, error) {
				return "SHORT:" + in, nil
			})),
		)

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "hello-world")
	if err != nil {
		t.Fatalf("Invoke for long input failed: %v", err)
	}
	if out != "SUB:HELLO-WORLD" {
		t.Fatalf("expected SUB:HELLO-WORLD, got %q", out)
	}

	out, err = r.Invoke(context.Background(), "hi")
	if err != nil {
		t.Fatalf("Invoke for short input failed: %v", err)
	}
	if out != "SHORT:hi" {
		t.Fatalf("expected SHORT:hi, got %q", out)
	}
}

func TestChainBranchAddPassthrough(t *testing.T) {
	chain := NewChain[string, string]()

	cond := func(ctx context.Context, in string) (string, error) {
		if len(in) > 5 {
			return "long", nil
		}
		return "short", nil
	}

	chain.
		AppendBranch(NewChainBranch(cond).
			AddPassthrough("long").
			AddPassthrough("short"),
		)

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "hello-world")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if out != "hello-world" {
		t.Fatalf("expected passthrough output hello-world, got %q", out)
	}
}

func TestChainNextNodeKey(t *testing.T) {
	chain := NewChain[string, string]()

	k1 := chain.nextNodeKey()
	if k1 != "node_0" {
		t.Fatalf("expected node_0, got %s", k1)
	}

	k2 := chain.nextNodeKey()
	if k2 != "node_1" {
		t.Fatalf("expected node_1, got %s", k2)
	}

	k3 := chain.nextNodeKey()
	if k3 != "node_2" {
		t.Fatalf("expected node_2, got %s", k3)
	}
}

func TestChainCompiledLock(t *testing.T) {
	chain := NewChain[string, string]()
	chain.
		AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return in, nil
		}))

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "test")
	if err != nil {
		t.Fatalf("Invoke failed: %v", err)
	}
	if out != "test" {
		t.Fatalf("expected test, got %q", out)
	}

	if !chain.hasEnd {
		t.Fatal("chain should have hasEnd=true after compile")
	}
}

func TestChainBranchParallelCombination(t *testing.T) {
	chain := NewChain[string, map[string]any]()

	branchCond := func(ctx context.Context, in string) (string, error) {
		if len(in) > 5 {
			return "process", nil
		}
		return "skip", nil
	}

	parallel := NewParallel()
	parallel.
		AddLambda("upper", InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return strings.ToUpper(in), nil
		})).
		AddLambda("length", InvokableLambda(func(ctx context.Context, in string) (int, error) {
			return len(in), nil
		})).
		AddLambda("prefix", InvokableLambda(func(ctx context.Context, in string) (string, error) {
			return "p:" + in, nil
		}))

	chain.
		AppendBranch(NewChainBranch(branchCond).
			AddLambda("process", InvokableLambda(func(ctx context.Context, in string) (string, error) {
				return in, nil
			})).
			AddLambda("skip", InvokableLambda(func(ctx context.Context, in string) (string, error) {
				return "skipped", nil
			})),
		).
		AppendParallel(parallel)

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	out, err := r.Invoke(context.Background(), "hello-world")
	if err != nil {
		t.Fatalf("Invoke for process branch failed: %v", err)
	}
	if out["upper"] != "HELLO-WORLD" {
		t.Fatalf("expected upper=HELLO-WORLD, got %v", out["upper"])
	}
	if out["length"] != 11 {
		t.Fatalf("expected length=11, got %v", out["length"])
	}
	if out["prefix"] != "p:hello-world" {
		t.Fatalf("expected prefix=p:hello-world, got %v", out["prefix"])
	}

	out, err = r.Invoke(context.Background(), "hi")
	if err != nil {
		t.Fatalf("Invoke for skip branch failed: %v", err)
	}
	if out["upper"] != "SKIPPED" {
		t.Fatalf("expected upper=SKIPPED, got %v", out["upper"])
	}
	if out["length"] != 7 {
		t.Fatalf("expected length=7, got %v", out["length"])
	}
	if out["prefix"] != "p:skipped" {
		t.Fatalf("expected prefix=p:skipped, got %v", out["prefix"])
	}
}

func TestChainBranchErrorPropagation(t *testing.T) {
	chain := NewChain[string, string]()

	cond := func(ctx context.Context, in string) (string, error) {
		if in == "error" {
			return "", context.DeadlineExceeded
		}
		return "ok", nil
	}

	chain.
		AppendBranch(NewChainBranch(cond).
			AddLambda("ok", InvokableLambda(func(ctx context.Context, in string) (string, error) {
				return "OK:" + in, nil
			})).
			AddLambda("fail", InvokableLambda(func(ctx context.Context, in string) (string, error) {
				return "FAIL", nil
			})),
		)

	r, err := chain.Compile(context.Background())
	if err != nil {
		t.Fatalf("Compile failed: %v", err)
	}

	_, err = r.Invoke(context.Background(), "error")
	if err == nil {
		t.Fatal("expected error from branch condition, got nil")
	}
}
