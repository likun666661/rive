package compose

import (
	"context"
	"errors"
	"strings"
	"testing"
)

type approvalState struct {
	Original string
}

func TestAddressStringAndSegmentSubID(t *testing.T) {
	ctx := context.Background()
	ctx = AppendAddressSegment(ctx, AddressSegmentRunnable, "root")
	ctx = AppendAddressSegment(ctx, AddressSegmentNode, "tools")
	ctx = AppendAddressSegment(ctx, AddressSegmentTool, "lookup", WithAddressSubID("call_1"))

	addr := GetCurrentAddress(ctx).String()
	want := "runnable:root;node:tools;tool:lookup:call_1"
	if addr != want {
		t.Fatalf("expected %q, got %q", want, addr)
	}
}

func TestGraphCheckpointInterruptResume(t *testing.T) {
	store := NewInMemoryCheckPointStore()

	g := NewGraph[string, string]()
	err := g.AddLambdaNode("approval", InvokableLambda(func(ctx context.Context, input string) (string, error) {
		wasInterrupted, hasState, state := GetInterruptState[approvalState](ctx)
		if !wasInterrupted {
			return "", StatefulInterrupt(ctx, "need approval", approvalState{Original: input})
		}
		if !hasState || state.Original != "draft" {
			t.Fatalf("expected persisted original input, got interrupted=%v hasState=%v state=%+v", wasInterrupted, hasState, state)
		}
		isResume, hasData, decision := GetResumeContext[string](ctx)
		if !isResume || !hasData {
			return "", StatefulInterrupt(ctx, "still waiting for direct resume data", state)
		}
		return state.Original + ":" + decision, nil
	}))
	if err != nil {
		t.Fatal(err)
	}
	if err := g.AddEdge(START, "approval"); err != nil {
		t.Fatal(err)
	}
	if err := g.AddEdge("approval", END); err != nil {
		t.Fatal(err)
	}
	r, err := g.Compile(context.Background(), WithGraphName("checkpoint_demo"), WithNodeTriggerMode(AllPredecessor))
	if err != nil {
		t.Fatal(err)
	}

	ctx := WithCheckPoint(context.Background(), "cp-1", store)
	_, err = r.Invoke(ctx, "draft")
	info, ok := ExtractInterruptInfo(err)
	if !ok {
		t.Fatalf("expected interrupt error, got %v", err)
	}
	if len(info.InterruptContexts) != 1 {
		t.Fatalf("expected one interrupt context, got %d", len(info.InterruptContexts))
	}
	if got := info.InterruptContexts[0].Address.String(); got != "runnable:checkpoint_demo;node:approval" {
		t.Fatalf("unexpected interrupt address: %s", got)
	}

	resumeCtx := ResumeWithData(WithCheckPoint(context.Background(), "cp-1", store), info.InterruptContexts[0].ID, "approved")
	out, err := r.Invoke(resumeCtx, "")
	if err != nil {
		t.Fatalf("resume failed: %v", err)
	}
	if out != "draft:approved" {
		t.Fatalf("unexpected resume output: %q", out)
	}
}

func TestCompositeInterruptFlattensRootCauses(t *testing.T) {
	ctx := AppendAddressSegment(context.Background(), AddressSegmentNode, "batch")

	childA := func() error {
		sub := AppendAddressSegment(ctx, AddressSegmentTool, "lookup", WithAddressSubID("a"))
		return StatefulInterrupt(sub, "need a", approvalState{Original: "a"})
	}()
	childB := func() error {
		sub := AppendAddressSegment(ctx, AddressSegmentTool, "lookup", WithAddressSubID("b"))
		return Interrupt(sub, "need b")
	}()

	err := CompositeInterrupt(ctx, "batch paused", map[string]bool{"a": false, "b": false}, childA, childB)
	info, ok := ExtractInterruptInfo(err)
	if !ok {
		t.Fatalf("expected composite interrupt, got %v", err)
	}
	if len(info.InterruptContexts) != 2 {
		t.Fatalf("expected 2 root causes, got %d", len(info.InterruptContexts))
	}
	got := []string{info.InterruptContexts[0].Address.String(), info.InterruptContexts[1].Address.String()}
	joined := strings.Join(got, "\n")
	if !strings.Contains(joined, "tool:lookup:a") || !strings.Contains(joined, "tool:lookup:b") {
		t.Fatalf("expected distinct child addresses, got:\n%s", joined)
	}
	for _, ctx := range info.InterruptContexts {
		if ctx.Parent == nil {
			t.Fatalf("expected flattened root cause %s to retain parent context", ctx.ID)
		}
	}
}

func TestResumeContextMarksAncestorAsConduit(t *testing.T) {
	childCtx := AppendAddressSegment(context.Background(), AddressSegmentNode, "parent")
	childCtx = AppendAddressSegment(childCtx, AddressSegmentTool, "tool", WithAddressSubID("call"))
	err := Interrupt(childCtx, "pause child")
	info, ok := ExtractInterruptInfo(err)
	if !ok {
		t.Fatal("expected interrupt")
	}
	idToAddr, idToState := SignalToPersistenceMaps(info.Signal)

	parentCtx := ResumeWithData(context.Background(), info.InterruptContexts[0].ID, "resume child")
	parentCtx = populateInterruptState(parentCtx, idToAddr, idToState)
	parentCtx = AppendAddressSegment(parentCtx, AddressSegmentNode, "parent")

	isResumeTarget, hasData, _ := GetResumeContext[string](parentCtx)
	if !isResumeTarget {
		t.Fatal("expected parent address to act as resume conduit for descendant")
	}
	if hasData {
		t.Fatal("parent conduit should not receive child resume data")
	}

	exactCtx := AppendAddressSegment(parentCtx, AddressSegmentTool, "tool", WithAddressSubID("call"))
	isResumeTarget, hasData, data := GetResumeContext[string](exactCtx)
	if !isResumeTarget || !hasData || data != "resume child" {
		t.Fatalf("expected exact child target data, got target=%v hasData=%v data=%q", isResumeTarget, hasData, data)
	}
}

func TestMaterializeAndRestoreStream(t *testing.T) {
	stream := PipeStreamReaderFromSlice([]string{"a", "b", "c"})
	materialized := MaterializeStream(stream)
	if len(materialized.Items) != 3 {
		t.Fatalf("expected 3 materialized chunks, got %d", len(materialized.Items))
	}
	restored := RestoreStream(materialized)
	got := drainAll(restored)
	if strings.Join(got, "") != "abc" {
		t.Fatalf("unexpected restored stream: %v", got)
	}
}

func TestCheckpointStoreMissing(t *testing.T) {
	store := NewInMemoryCheckPointStore()
	_, err := store.Get(context.Background(), "missing")
	if !errors.Is(err, ErrCheckPointNotFound) {
		t.Fatalf("expected ErrCheckPointNotFound, got %v", err)
	}
}
