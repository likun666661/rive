# Chapter 04 - Checkpoint / Interrupt / Resume 深度讲解

面向读者：假设你已经读过前三章，知道图如何编译成 `Runnable`，也知道节点执行时会带着 `context.Context`、地址、callback 等运行时信息。

这一章要回答的问题是：

```text
一个图执行到一半需要人工审批时，怎么停下来？
停下来之后，怎么记住自己停在哪里？
用户审批后，怎么把恢复数据送回正确节点？
为什么不能简单地从头重跑？
```

参考代码位置：

- 手册：`examples/eino-technical-manual/manual/04-checkpoint-interrupt-resume.md`
- 复刻版：`examples/eino-compose-runtime-replica-go`
- 本章重点源码：
  - `compose/address.go`
  - `compose/interrupt.go`
  - `compose/resume.go`
  - `compose/checkpoint.go`
  - `compose/graph_run.go`
  - `compose/graph_manager.go`
  - `compose/checkpoint_test.go`

说明：原始手册里描述了原版 Eino 更完整的 checkpoint 模型，包括 channel 状态、task 输入、skip 标记、子图 checkpoint、stream 物化、状态迁移等。当前 Go 复刻版是教学版简化实现：主要保存原始输入、interrupt ID 到 address/state 的映射，并在恢复时重放原始输入，让节点通过地址匹配拿回中断状态和恢复数据。本文以当前复刻版为准。

## 1. 为什么需要中断和恢复

很多 LLM 应用不是一次函数调用就能结束。

典型场景：

- 模型想调用高风险工具，需要人工审批。
- agent 要发邮件、下单、转账，需要用户确认。
- 工具遇到速率限制，想暂停后继续。
- 多个并行任务中有一部分等待外部输入。
- 长流程中间某一步失败，不希望从头重新跑。

如果没有中断恢复，最朴素的做法是：

```text
执行失败或暂停 -> 记录一下 -> 用户确认后从头再跑
```

这很危险。

为什么？

第一，重新运行可能重复副作用。比如之前已经调用过 API、写过数据库、发过消息，再跑一遍可能造成重复行为。

第二，重新运行不一定走同一路径。LLM 输出有随机性，上一次选择工具 A，这一次可能选择工具 B。

第三，无法把恢复数据送到正确位置。用户说“批准 tool call #2”，runtime 必须知道这个批准对应哪个节点、哪个工具、哪个调用 ID。

所以需要一个正式机制：

```text
节点主动 Interrupt
runtime 保存 checkpoint
用户拿到 interrupt ID
用户用 interrupt ID 提交 resume data
runtime 恢复 checkpoint
节点在同一地址重新拿到 state + resume data
继续执行
```

## 2. 当前复刻版的核心模型

当前复刻版的中断恢复由四个概念组成：

```text
Address      标识“我是谁，我在图的哪里”
Interrupt    节点主动暂停，并带出用户可见信息和持久状态
CheckPoint   保存原始输入 + interrupt 地址/状态映射
Resume       用户用 interrupt ID 提供恢复数据
```

文件对应关系：

```text
address.go    -> Address / AppendAddressSegment / resume data 路由
interrupt.go  -> Interrupt / StatefulInterrupt / CompositeInterrupt
checkpoint.go -> CheckPoint / CheckPointStore / save/restore
resume.go     -> ResumeWithData / BatchResumeWithData / GetInterruptState / GetResumeContext
graph_run.go  -> runner 何时 restore、何时 save checkpoint
```

## 3. 一句话串起执行流程

首次执行：

```text
runner.run
  -> restoreCheckPointContext 找不到 checkpoint，正常跑
  -> node 执行时 AppendAddressSegment 得到当前地址
  -> node 调 StatefulInterrupt(ctx, info, state)
  -> runner 捕获 InterruptError
  -> saveInterruptCheckPoint 保存 input + interrupt id/address/state
  -> 调用方拿到 InterruptInfo
```

恢复执行：

```text
调用方 ResumeWithData(ctx, interruptID, data)
  -> runner.run
  -> restoreCheckPointContext 读取 checkpoint
  -> populateInterruptState 把 id/address/state 放入 context
  -> 使用 checkpoint.Input 替代本次传入 input
  -> 图重新跑到同一个 node address
  -> AppendAddressSegment 匹配 interruptID/address
  -> node 调 GetInterruptState 拿回 state
  -> node 调 GetResumeContext 拿到 resume data
  -> node 继续执行并返回结果
```

这就是当前复刻版的主干。

## 4. Address：为什么必须有稳定地址

如果图里只有一个节点，中断恢复很简单。

但真实图会有嵌套：

```text
runnable:root
  node:tools
    tool:lookup:call_1
    tool:lookup:call_2
```

两个工具调用都叫 `lookup`，但它们不是同一个中断点。

所以需要地址。

当前复刻版在 `compose/address.go` 里定义：

```go
type AddressSegmentType string

const (
    AddressSegmentNode     AddressSegmentType = "node"
    AddressSegmentTool     AddressSegmentType = "tool"
    AddressSegmentRunnable AddressSegmentType = "runnable"
)

type AddressSegment struct {
    ID    string
    Type  AddressSegmentType
    SubID string
}

type Address []AddressSegment
```

地址字符串格式：

```go
func (a Address) String() string {
    part := fmt.Sprintf("%s:%s", seg.Type, seg.ID)
    if seg.SubID != "" {
        part += ":" + seg.SubID
    }
    return strings.Join(parts, ";")
}
```

例如测试 `TestAddressStringAndSegmentSubID` 期望：

```text
runnable:root;node:tools;tool:lookup:call_1
```

这就是一个稳定、可持久化、可比较的执行位置。

## 5. AppendAddressSegment：进入一个新的执行作用域

`AppendAddressSegment` 是地址系统的核心：

```go
func AppendAddressSegment(ctx context.Context, typ AddressSegmentType, id string, opts ...AddressOption) context.Context
```

它做三件事。

第一，基于父地址追加一个 segment：

```go
addr := append(parent, AddressSegment{Type: typ, ID: id, SubID: o.subID})
```

第二，读取全局 resume info：

```go
gri := getGlobalResumeInfo(ctx)
```

第三，判断当前地址是否匹配历史中断地址或恢复目标：

```go
for interruptID, interruptAddr := range gri.idToAddress {
    if interruptAddr.equal(addr) {
        // exact match
        next.interruptState = ...
        if data exists {
            next.isResumeTarget = true
            next.hasResumeData = true
            next.resumeData = data
        }
        continue
    }

    if resumeData exists && interruptAddr.hasPrefix(addr) && len(interruptAddr) > len(addr) {
        // descendant target
        next.isResumeTarget = true
    }
}
```

这里有一个很重要的设计：不仅精确地址会被标记为 resume target，某个恢复目标的祖先地址也会被标记为 resume target，但没有数据。

为什么？

因为复合节点需要知道“我的后代要恢复”。它自己不是最终目标，但它要继续往下执行，不能把恢复信号吞掉。

测试：`TestResumeContextMarksAncestorAsConduit`。

## 6. Graph runner 什么时候追加地址

在 `graph_run.go` 开头：

```go
if r.graphName != "" {
    ctx = AppendAddressSegment(ctx, AddressSegmentRunnable, r.graphName)
}
```

如果编译时用了：

```go
WithGraphName("checkpoint_demo")
```

那么图地址会先有：

```text
runnable:checkpoint_demo
```

节点 task 创建时：

```go
t := &task{
    nodeKey: nodeKey,
    ctx: AppendAddressSegment(ctx, AddressSegmentNode, nodeKey),
}
```

所以节点 `approval` 的地址是：

```text
runnable:checkpoint_demo;node:approval
```

测试 `TestGraphCheckpointInterruptResume` 就检查了这个地址。

## 7. Interrupt：节点如何主动暂停

节点如果想暂停，可以返回 interrupt error。

最简单：

```go
return "", Interrupt(ctx, "need approval")
```

带状态：

```go
return "", StatefulInterrupt(ctx, "need approval", approvalState{Original: input})
```

`StatefulInterrupt` 做的事：

```go
sig := newInterruptSignal(ctx, info, state, true)
return &InterruptError{Info: newInterruptInfo(sig)}
```

`newInterruptSignal` 会记录当前地址：

```go
addr := GetCurrentAddress(ctx)
```

生成信号：

```go
type InterruptSignal struct {
    ID             string
    Address        Address
    InterruptInfo  InterruptPayload
    InterruptState InterruptState
    Subs           []*InterruptSignal
}
```

`ID` 是恢复时用的唯一标识。

`Address` 是节点位置。

`InterruptInfo.Info` 是给调用方看的暂停原因。

`InterruptState.State` 是节点恢复时要拿回的内部状态。

## 8. InterruptInfo：为什么用户看到的是扁平 root cause

`InterruptInfo`：

```go
type InterruptInfo struct {
    Signal            *InterruptSignal
    InterruptContexts []*InterruptContext
}
```

`Signal` 是完整树。

`InterruptContexts` 是扁平化之后给用户看的根因列表。

为什么要扁平化？

比如一个 batch 节点内部有两个工具都暂停：

```text
batch
  lookup:a paused
  lookup:b paused
```

调用方最关心的是两个真正需要处理的 root cause：

```text
interrupt_1 -> tool:lookup:a
interrupt_2 -> tool:lookup:b
```

`ToInterruptContexts` 会只把 `IsRootCause=true` 的 signal 放进用户列表，同时保留 `Parent` 指针。

测试：`TestCompositeInterruptFlattensRootCauses`。

## 9. CompositeInterrupt：多个子中断如何合并

`CompositeInterrupt` 用于把多个中断包装成一个父中断：

```go
func CompositeInterrupt(ctx context.Context, info any, state any, errs ...error) error
```

它会遍历子错误：

```go
if child, ok := ExtractInterruptInfo(err); ok && child.Signal != nil {
    subs = append(subs, child.Signal)
}
```

然后创建父 signal：

```go
sig := newInterruptSignal(ctx, info, state, len(subs) == 0)
sig.Subs = subs
```

如果有子中断，父 signal 通常不是 root cause；真正的 root cause 是子 signal。

这适合：

- 并行工具调用。
- batch 子任务。
- 复合节点内部多个等待点。

## 10. CheckPoint 保存什么

当前复刻版的 `CheckPoint` 很轻量：

```go
type CheckPoint struct {
    Input                 any
    InterruptID2Addr      map[string]Address
    InterruptID2State     map[string]InterruptState
    LayerSpecificSnapshot map[string]any
}
```

核心字段：

- `Input`：首次执行的原始 input。
- `InterruptID2Addr`：interrupt ID 对应哪个地址。
- `InterruptID2State`：interrupt ID 对应保存的状态。
- `LayerSpecificSnapshot`：预留给层特定快照，当前主线里用得很少。

对比原版 Eino，当前复刻版没有保存完整 channel/task 状态。它的恢复策略更像：

```text
保存原始输入和中断状态；
恢复时从头重放原始输入；
跑到同一地址后注入 state/resume data。
```

这对教学非常清晰，但它不是完整的“从精确 task/channel 状态继续执行”模型。

## 11. CheckPointStore

接口：

```go
type CheckPointStore interface {
    Get(ctx context.Context, id string) (*CheckPoint, error)
    Set(ctx context.Context, id string, cp *CheckPoint) error
}
```

当前提供内存实现：

```go
NewInMemoryCheckPointStore()
```

`Get` 和 `Set` 都会 clone checkpoint，避免调用方直接修改 store 里的对象。

如果没有 checkpoint：

```go
ErrCheckPointNotFound
```

测试：`TestCheckpointStoreMissing`。

## 12. 如何把 store 和 checkpoint ID 放进 context

`checkpoint.go` 提供：

```go
WithCheckPointStore(ctx, store)
WithCheckPointID(ctx, id)
WithCheckPoint(ctx, id, store)
```

通常直接用：

```go
ctx := WithCheckPoint(context.Background(), "cp-1", store)
```

这会把 checkpoint id 和 store 都放进 context。

runner 通过：

```go
checkpointConfig(ctx)
```

判断当前调用是否启用了 checkpoint。

## 13. 首次执行时如何保存 checkpoint

在 `runner.run` 里：

```go
if err := r.resolveCompletedTasks(ctx, cm, completedTasks); err != nil {
    if info, ok := ExtractInterruptInfo(err); ok {
        _ = saveInterruptCheckPoint(ctx, input, info)
    }
    return nil, err
}
```

节点返回 interrupt error 后：

1. `resolveCompletedTasks` 返回错误。
2. runner 用 `ExtractInterruptInfo` 判断是不是 interrupt。
3. 如果是，调用 `saveInterruptCheckPoint`。
4. 然后把 interrupt error 返回给调用方。

`saveInterruptCheckPoint`：

```go
idToAddr, idToState := SignalToPersistenceMaps(info.Signal)
return store.Set(ctx, id, &CheckPoint{
    Input:             input,
    InterruptID2Addr:  idToAddr,
    InterruptID2State: idToState,
})
```

这里保存的是：

- 当前 run 的 input。
- interrupt tree 扁平化后的 id -> address。
- interrupt tree 扁平化后的 id -> state。

## 14. 恢复时如何读取 checkpoint

`runner.run` 一开始就调用：

```go
ctx, input, err = restoreCheckPointContext(ctx, input)
```

`restoreCheckPointContext`：

```go
cp, err := store.Get(ctx, id)
if ErrCheckPointNotFound {
    return ctx, input, nil
}
ctx = populateInterruptState(ctx, cp.InterruptID2Addr, cp.InterruptID2State)
return ctx, cp.Input, nil
```

两个重点：

第一，恢复时会把 checkpoint 里的 interrupt address/state 放回 context 的 global resume info。

第二，恢复时会用 `cp.Input` 替代本次传入的 input。

所以测试里恢复时传的是空字符串：

```go
out, err := r.Invoke(resumeCtx, "")
```

最终仍然能得到：

```text
draft:approved
```

因为真实输入来自 checkpoint 里保存的 `"draft"`。

## 15. ResumeWithData：恢复数据如何进入 context

`ResumeWithData`：

```go
func ResumeWithData(ctx context.Context, interruptID string, data any) context.Context {
    gri := cloneGlobalResumeInfo(getGlobalResumeInfo(ctx))
    gri.resumeData[interruptID] = data
    return context.WithValue(ctx, globalResumeInfoKey{}, gri)
}
```

它只做一件事：

```text
interruptID -> resume data
```

放进 context。

如果一次恢复多个中断：

```go
BatchResumeWithData(ctx, map[string]any{
    idA: dataA,
    idB: dataB,
})
```

当前测试主要覆盖 `ResumeWithData`，但 API 已经支持 batch。

## 16. GetInterruptState 和 GetResumeContext

节点恢复时通常要调用两个函数。

### 16.1 GetInterruptState

```go
wasInterrupted, hasState, state := GetInterruptState[approvalState](ctx)
```

含义：

- `wasInterrupted`：当前地址之前是否中断过。
- `hasState`：是否有类型匹配的 state。
- `state`：保存的 state。

如果泛型类型不匹配，`hasState=false`。

### 16.2 GetResumeContext

```go
isResume, hasData, decision := GetResumeContext[string](ctx)
```

含义：

- `isResume`：当前地址是恢复目标，或是某个后代恢复目标的祖先。
- `hasData`：当前地址是否精确匹配并拿到了数据。
- `data`：恢复数据。

祖先 conduit 的情况：

```text
isResume=true
hasData=false
```

精确目标的情况：

```text
isResume=true
hasData=true
```

这两个状态要分清。

## 17. 完整例子：审批节点

测试 `TestGraphCheckpointInterruptResume` 是最好的入门例子。

节点逻辑：

```go
g.AddLambdaNode("approval", InvokableLambda(func(ctx context.Context, input string) (string, error) {
    wasInterrupted, hasState, state := GetInterruptState[approvalState](ctx)
    if !wasInterrupted {
        return "", StatefulInterrupt(ctx, "need approval", approvalState{Original: input})
    }
    if !hasState || state.Original != "draft" {
        t.Fatalf("expected persisted original input")
    }
    isResume, hasData, decision := GetResumeContext[string](ctx)
    if !isResume || !hasData {
        return "", StatefulInterrupt(ctx, "still waiting for direct resume data", state)
    }
    return state.Original + ":" + decision, nil
}))
```

第一次运行：

```go
ctx := WithCheckPoint(context.Background(), "cp-1", store)
_, err := r.Invoke(ctx, "draft")
info, ok := ExtractInterruptInfo(err)
```

拿到 interrupt：

```text
Address = runnable:checkpoint_demo;node:approval
Info    = need approval
State   = approvalState{Original:"draft"}
```

恢复：

```go
resumeCtx := ResumeWithData(
    WithCheckPoint(context.Background(), "cp-1", store),
    info.InterruptContexts[0].ID,
    "approved",
)
out, err := r.Invoke(resumeCtx, "")
```

输出：

```text
draft:approved
```

这说明三件事：

1. 原始 input `"draft"` 从 checkpoint 恢复了。
2. `approvalState{Original:"draft"}` 被保存并恢复了。
3. resume data `"approved"` 被送回了正确节点。

## 18. 多中断：CompositeInterrupt

看测试 `TestCompositeInterruptFlattensRootCauses`。

它构造一个父地址：

```go
ctx := AppendAddressSegment(context.Background(), AddressSegmentNode, "batch")
```

两个子工具地址：

```go
sub := AppendAddressSegment(ctx, AddressSegmentTool, "lookup", WithAddressSubID("a"))
sub := AppendAddressSegment(ctx, AddressSegmentTool, "lookup", WithAddressSubID("b"))
```

子 A：

```go
StatefulInterrupt(sub, "need a", approvalState{Original: "a"})
```

子 B：

```go
Interrupt(sub, "need b")
```

父节点合并：

```go
CompositeInterrupt(ctx, "batch paused", map[string]bool{"a": false, "b": false}, childA, childB)
```

用户看到两个 root cause：

```text
tool:lookup:a
tool:lookup:b
```

并且每个 `InterruptContext` 都保留 `Parent`。

这就是“复合节点把多个中断打包，但用户仍能分别处理根因”的设计。

## 19. 祖先 conduit：为什么 parent 也会 isResumeTarget

测试 `TestResumeContextMarksAncestorAsConduit` 讲了这个机制。

中断发生在：

```text
node:parent;tool:tool:call
```

恢复时用户提供的是 child interrupt ID 的数据：

```go
ResumeWithData(ctx, childID, "resume child")
```

当执行来到：

```text
node:parent
```

`GetResumeContext` 返回：

```go
isResumeTarget = true
hasData = false
```

这表示：

```text
你不是最终恢复目标，但你的后代是，所以你要继续往下走。
```

当执行来到精确地址：

```text
node:parent;tool:tool:call
```

`GetResumeContext` 返回：

```go
isResumeTarget = true
hasData = true
data = "resume child"
```

这就是恢复数据真正到达的地方。

## 20. Stream materialize / restore

流是一次性数据。读完就没了。

如果要把流放进 checkpoint，不能保存一个正在读的 channel 或 reader 对象。必须把流物化成普通数据。

当前复刻版提供：

```go
type MaterializedStream[T any] struct {
    Items []T
}

func MaterializeStream[T any](r PipeStreamReader[T]) *MaterializedStream[T] {
    return &MaterializedStream[T]{Items: drainAll(r)}
}

func RestoreStream[T any](m *MaterializedStream[T]) PipeStreamReader[T] {
    return PipeStreamReaderFromSlice(m.Items)
}
```

测试 `TestMaterializeAndRestoreStream`：

```go
stream := PipeStreamReaderFromSlice([]string{"a", "b", "c"})
materialized := MaterializeStream(stream)
restored := RestoreStream(materialized)
got := drainAll(restored)
```

最终得到：

```text
abc
```

注意：当前 checkpoint 主线并没有自动遍历图中所有 stream channel 去物化。这里提供的是教学版工具函数，用来展示“流 checkpoint 必须先物化”的基本思想。

## 21. 当前复刻版与原版完整模型的差异

这是本章最容易误解的地方。

手册里描述的原版模型包括：

- 保存 channel 状态。
- 保存 pending task input。
- 保存 skip pre-handler 标记。
- 保存 rerun nodes。
- 保存 subgraph checkpoint。
- 保存图级 state。
- 递归恢复子图。
- 更完整的 stream materialize/restore。

当前复刻版实际 `CheckPoint` 只有：

```go
Input
InterruptID2Addr
InterruptID2State
LayerSpecificSnapshot
```

因此当前复刻版更适合理解概念主线：

```text
中断信号如何产生；
地址如何标识中断点；
checkpoint 如何保存 id/address/state；
resume data 如何通过 context 路由回节点。
```

但它不是完整生产级 checkpoint runtime。

## 22. 常见误解点

### 误解 1：Interrupt 会自动保存 checkpoint

不完全对。

节点只是返回 `InterruptError`。真正保存 checkpoint 的地方是 `runner.run`：

```go
if info, ok := ExtractInterruptInfo(err); ok {
    _ = saveInterruptCheckPoint(ctx, input, info)
}
```

如果 context 里没有 checkpoint store 和 checkpoint id，就不会保存。

### 误解 2：有 checkpoint 就一定从精确位置继续

当前复刻版不是。

它恢复原始 input，然后重新跑图，跑到同一 address 时注入 state/data。

这不是完整的 channel/task 级别恢复。

### 误解 3：恢复时传给 Invoke 的 input 会被使用

如果 checkpoint 存在，`restoreCheckPointContext` 会返回 `cp.Input`。

所以恢复调用里传入的 input 通常会被 checkpoint input 替代。

### 误解 4：Interrupt info 和 Interrupt state 是一回事

不是。

`info` 是给调用方看的，例如：

```text
need approval
```

`state` 是恢复时给同一地址节点看的，例如：

```go
approvalState{Original:"draft"}
```

### 误解 5：resume data 会广播给所有节点

不会。

`ResumeWithData` 绑定的是 interrupt ID。只有地址匹配该 ID 的节点能拿到数据。

祖先节点只会看到 `isResumeTarget=true, hasData=false`。

### 误解 6：没有 state 就不能 resume

可以。

`Interrupt(ctx, info)` 没有 state，但仍有 interrupt ID 和 address。恢复数据仍然可以通过 `ResumeWithData` 送到该地址。

### 误解 7：CompositeInterrupt 会让用户看到父中断

如果有子中断，父 signal 通常不是 root cause。用户看到的是被扁平化后的 root cause 子中断。

### 误解 8：Address 只需要 node name

不够。

同一个工具可以并行调用多次，所以需要 `SubID` 区分：

```text
tool:lookup:a
tool:lookup:b
```

### 误解 9：MaterializeStream 不会消费原 stream

会消费。

`MaterializeStream` 内部调用 `drainAll`，原 stream 被读完。

### 误解 10：当前复刻版已经有完整 subgraph checkpoint

没有。

地址系统支持嵌套地址，CompositeInterrupt 可以表示树，但 checkpoint 结构没有完整保存子图 channel/task 状态。

## 23. 建议源码阅读顺序

第一遍看主线：

1. `checkpoint_test.go`
   - `TestGraphCheckpointInterruptResume`
   - `TestCompositeInterruptFlattensRootCauses`
   - `TestResumeContextMarksAncestorAsConduit`
   - `TestMaterializeAndRestoreStream`

2. `address.go`
   - `AddressSegment`
   - `Address.String`
   - `AppendAddressSegment`
   - `populateInterruptState`

3. `interrupt.go`
   - `InterruptState`
   - `InterruptSignal`
   - `Interrupt`
   - `StatefulInterrupt`
   - `CompositeInterrupt`
   - `ToInterruptContexts`
   - `SignalToPersistenceMaps`

4. `checkpoint.go`
   - `CheckPoint`
   - `CheckPointStore`
   - `WithCheckPoint`
   - `restoreCheckPointContext`
   - `saveInterruptCheckPoint`
   - `MaterializeStream`
   - `RestoreStream`

5. `resume.go`
   - `ResumeWithData`
   - `BatchResumeWithData`
   - `GetInterruptState`
   - `GetResumeContext`

6. `graph_run.go`
   - `restoreCheckPointContext`
   - `saveInterruptCheckPoint`
   - `AppendAddressSegment(ctx, AddressSegmentRunnable, r.graphName)`

7. `graph_manager.go`
   - task ctx 如何加 node address。

第二遍再看扩展：

1. 多节点并发中断如何通过 `CompositeInterrupt` 合并。
2. 祖先 conduit 机制如何支持复合节点。
3. stream materialize/restore 的限制。
4. 当前简化版和原版完整 checkpoint 模型的差异。

## 24. 练习题

### 练习 1：打印地址

目标：理解 AddressSegment。

要求：

1. 从 `context.Background()` 开始。
2. Append runnable `root`。
3. Append node `tools`。
4. Append tool `lookup`，SubID 为 `call_1`。
5. 打印 `GetCurrentAddress(ctx).String()`。

期望：

```text
runnable:root;node:tools;tool:lookup:call_1
```

### 练习 2：最小 Interrupt

目标：理解 InterruptError。

要求：

1. 创建一个 graph，节点永远返回 `Interrupt(ctx, "pause")`。
2. Invoke。
3. 用 `ExtractInterruptInfo(err)` 提取 info。
4. 打印 interrupt ID、address、info。

思考：

- 如果没有 `WithGraphName`，address 会少什么？
- 如果没有 checkpoint store，会不会保存 checkpoint？

### 练习 3：StatefulInterrupt + ResumeWithData

目标：复现 `TestGraphCheckpointInterruptResume`。

要求：

1. 节点第一次运行时保存 state：原始 input。
2. 恢复时读取 state。
3. 用 resume data 作为审批结果。
4. 输出 `original + ":" + decision`。

思考：

- 为什么恢复时传空 input 也能拿到 original？
- `GetInterruptState` 和 `GetResumeContext` 分别负责什么？

### 练习 4：不带 checkpoint 的恢复会怎样

目标：理解 checkpoint store 的必要性。

要求：

1. 第一次运行时不使用 `WithCheckPoint`。
2. 节点中断。
3. 尝试用 interrupt ID + ResumeWithData 恢复。

思考：

- 没有 checkpoint 时，state 从哪里来？
- 节点会认为自己 wasInterrupted 吗？

### 练习 5：CompositeInterrupt 两个子中断

目标：理解中断树和扁平 root cause。

要求：

1. 创建父 address：`node:batch`。
2. 创建两个子 address：`tool:lookup:a` 和 `tool:lookup:b`。
3. 子 A 返回 StatefulInterrupt。
4. 子 B 返回 Interrupt。
5. 父用 CompositeInterrupt 合并。
6. 检查 `InterruptContexts` 长度为 2。

思考：

- 父中断是否是 root cause？
- 每个子 context 的 Parent 是否存在？

### 练习 6：祖先 conduit

目标：理解 `isResumeTarget=true, hasData=false`。

要求：

1. 让 child address 中断。
2. 用 child interrupt ID 设置 ResumeWithData。
3. populateInterruptState。
4. 进入 parent address。
5. 调 GetResumeContext。
6. 再进入 child address。
7. 再调 GetResumeContext。

思考：

- parent 为什么没有 data？
- parent 看到 isResumeTarget 的意义是什么？

### 练习 7：BatchResumeWithData

目标：设计多中断恢复。

要求：

1. 构造两个 interrupt ID。
2. 用 `BatchResumeWithData` 同时传入两个数据。
3. 分别进入两个匹配地址。
4. 检查每个地址拿到自己的数据。

思考：

- 如果只恢复其中一个 ID，另一个地址会怎样？

### 练习 8：MaterializeStream

目标：理解流物化。

要求：

1. 创建 `PipeStreamReaderFromSlice([]string{"a","b","c"})`。
2. 调 `MaterializeStream`。
3. 调 `RestoreStream`。
4. drain restored stream。

思考：

- 原 stream 是否还能再读？
- 为什么 checkpoint 不能直接保存 reader？

### 练习 9：checkpoint clone

目标：理解 store 为什么 clone。

要求：

1. Set 一个 checkpoint。
2. Get 出来后修改 map。
3. 再 Get 一次。
4. 检查 store 内部数据有没有被污染。

思考：

- 如果不 clone，会有什么问题？

### 练习 10：设计题：审批工具调用

场景：

```text
agent 生成 tool call
tool 节点准备执行 delete_resource
需要人工审批
审批通过后继续执行 tool
```

请设计：

1. address 应该包含哪些 segment？
2. interrupt info 给用户看什么？
3. interrupt state 保存什么？
4. resume data 应该是什么类型？
5. tool 节点恢复时如何判断批准/拒绝？

## 25. 自测问题

读完后，你应该能回答：

1. 为什么不能简单从头重跑？
2. Address 的作用是什么？
3. `SubID` 解决什么问题？
4. `Interrupt` 和 `StatefulInterrupt` 的区别是什么？
5. `InterruptInfo.Info` 和 `InterruptState.State` 有什么区别？
6. `CompositeInterrupt` 为什么需要 `Subs`？
7. 用户看到的 `InterruptContexts` 是树还是扁平列表？
8. 当前 `CheckPoint` 保存哪些字段？
9. `WithCheckPoint` 做了什么？
10. runner 在哪里保存 checkpoint？
11. runner 在哪里恢复 checkpoint？
12. 恢复时为什么会使用 checkpoint.Input？
13. `ResumeWithData` 如何把数据送到正确节点？
14. 祖先 conduit 是什么意思？
15. `MaterializeStream` 为什么会消费原 stream？
16. 当前复刻版和原版完整 checkpoint 模型的差异是什么？

## 26. 一句话总结

Chapter 04 的核心是让暂停和继续变成可持久化、可定位、可恢复的运行时协议：

```text
Address 标识中断位置；
InterruptSignal 描述暂停原因和保存状态；
CheckPoint 持久化 input + interrupt id/address/state；
ResumeWithData 通过 interrupt ID 提供恢复数据；
AppendAddressSegment 在重新执行到匹配地址时注入 state/data；
节点用 GetInterruptState 和 GetResumeContext 决定继续还是再次暂停。
```

学习当前复刻版时，也要记住它的边界：

```text
它展示的是中断恢复的核心协议；
它不是完整保存 channel/task/subgraph 状态的生产级 checkpoint runtime。
```

