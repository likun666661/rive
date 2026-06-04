package compose

import (
	"context"
	"fmt"
	"strings"
)

// AddressSegmentType identifies one execution scope in a nested runnable tree.
type AddressSegmentType string

const (
	AddressSegmentNode     AddressSegmentType = "node"
	AddressSegmentTool     AddressSegmentType = "tool"
	AddressSegmentRunnable AddressSegmentType = "runnable"
)

// AddressSegment is one stable step in an execution address.
type AddressSegment struct {
	ID    string
	Type  AddressSegmentType
	SubID string
}

// Address is a structural execution identity such as
// "runnable:root;node:tools;tool:lookup:call_1".
type Address []AddressSegment

func (a Address) String() string {
	if len(a) == 0 {
		return ""
	}
	parts := make([]string, 0, len(a))
	for _, seg := range a {
		part := fmt.Sprintf("%s:%s", seg.Type, seg.ID)
		if seg.SubID != "" {
			part += ":" + seg.SubID
		}
		parts = append(parts, part)
	}
	return strings.Join(parts, ";")
}

func (a Address) clone() Address {
	if len(a) == 0 {
		return nil
	}
	cp := make(Address, len(a))
	copy(cp, a)
	return cp
}

func (a Address) equal(other Address) bool {
	if len(a) != len(other) {
		return false
	}
	for i := range a {
		if a[i] != other[i] {
			return false
		}
	}
	return true
}

func (a Address) hasPrefix(prefix Address) bool {
	if len(prefix) > len(a) {
		return false
	}
	for i := range prefix {
		if a[i] != prefix[i] {
			return false
		}
	}
	return true
}

type addressOption struct {
	subID string
}

// AddressOption customizes an appended execution address segment.
type AddressOption func(*addressOption)

// WithAddressSubID disambiguates repeated execution scopes such as tool calls.
func WithAddressSubID(subID string) AddressOption {
	return func(o *addressOption) {
		o.subID = subID
	}
}

type addressCtxKey struct{}
type globalResumeInfoKey struct{}

type addressContext struct {
	address          Address
	interruptState   *InterruptState
	isResumeTarget   bool
	hasResumeData    bool
	resumeData       any
	globalResumeInfo *globalResumeInfo
}

type globalResumeInfo struct {
	idToAddress map[string]Address
	idToState   map[string]InterruptState
	resumeData  map[string]any
}

func emptyGlobalResumeInfo() *globalResumeInfo {
	return &globalResumeInfo{
		idToAddress: make(map[string]Address),
		idToState:   make(map[string]InterruptState),
		resumeData:  make(map[string]any),
	}
}

func cloneGlobalResumeInfo(src *globalResumeInfo) *globalResumeInfo {
	dst := emptyGlobalResumeInfo()
	if src == nil {
		return dst
	}
	for id, addr := range src.idToAddress {
		dst.idToAddress[id] = addr.clone()
	}
	for id, state := range src.idToState {
		dst.idToState[id] = state
	}
	for id, data := range src.resumeData {
		dst.resumeData[id] = data
	}
	return dst
}

func getAddressContext(ctx context.Context) *addressContext {
	if ac, ok := ctx.Value(addressCtxKey{}).(*addressContext); ok && ac != nil {
		return ac
	}
	return nil
}

func getGlobalResumeInfo(ctx context.Context) *globalResumeInfo {
	if gri, ok := ctx.Value(globalResumeInfoKey{}).(*globalResumeInfo); ok && gri != nil {
		return gri
	}
	if ac := getAddressContext(ctx); ac != nil && ac.globalResumeInfo != nil {
		return ac.globalResumeInfo
	}
	return emptyGlobalResumeInfo()
}

// GetCurrentAddress returns the structural address of the current execution scope.
func GetCurrentAddress(ctx context.Context) Address {
	if ac := getAddressContext(ctx); ac != nil {
		return ac.address.clone()
	}
	return nil
}

// AppendAddressSegment enters a child execution scope and routes checkpointed
// interrupt state/resume data to the matching address.
func AppendAddressSegment(ctx context.Context, typ AddressSegmentType, id string, opts ...AddressOption) context.Context {
	o := &addressOption{}
	for _, opt := range opts {
		opt(o)
	}

	var parent Address
	if ac := getAddressContext(ctx); ac != nil {
		parent = ac.address.clone()
	}
	addr := append(parent, AddressSegment{Type: typ, ID: id, SubID: o.subID})
	gri := getGlobalResumeInfo(ctx)

	next := &addressContext{
		address:          addr,
		globalResumeInfo: gri,
	}

	for interruptID, interruptAddr := range gri.idToAddress {
		if interruptAddr.equal(addr) {
			if state, ok := gri.idToState[interruptID]; ok {
				st := state
				next.interruptState = &st
			}
			if data, ok := gri.resumeData[interruptID]; ok {
				next.isResumeTarget = true
				next.hasResumeData = true
				next.resumeData = data
			}
			continue
		}
		if _, ok := gri.resumeData[interruptID]; ok && interruptAddr.hasPrefix(addr) && len(interruptAddr) > len(addr) {
			next.isResumeTarget = true
		}
	}

	ctx = context.WithValue(ctx, globalResumeInfoKey{}, gri)
	return context.WithValue(ctx, addressCtxKey{}, next)
}

func populateInterruptState(ctx context.Context, idToAddress map[string]Address, idToState map[string]InterruptState) context.Context {
	gri := cloneGlobalResumeInfo(getGlobalResumeInfo(ctx))
	for id, addr := range idToAddress {
		gri.idToAddress[id] = addr.clone()
	}
	for id, state := range idToState {
		gri.idToState[id] = state
	}
	return context.WithValue(ctx, globalResumeInfoKey{}, gri)
}
