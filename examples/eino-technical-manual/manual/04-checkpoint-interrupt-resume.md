# 04 — Checkpoint / 中断 / 恢复

## 1. 面临的问题

Eino 的执行图可以任意嵌套深度：`Graph` 嵌套子图，`Agent` 包裹 `Graph`，`ToolsNode` 扇出多个并行工具调用，而 `Lambda` 节点可能启动一个完整的独立 `Runnable`。当这个深度嵌套网络中的任意组件决定暂停 —— 因为需要人工输入、触发了速率限制或工具调用需要审批 —— 运行时必须：

- **保存精确的执行状态**，以便图可以从精确的中断点重新启动，而非从头开始。
- **在整个调用树中唯一标识每一个中断点**，即使同一个工具以不同的调用 ID 被调用了两次（`tool:my_tool:call_1` 与 `tool:my_tool:call_2`）。
- **防止父图吞掉子中断** —— 子图的中断必须向上传播到根调用方。
- **在检查点保存之前物化流数据**，以便恢复时具有确定性的、非临时的输入。
- **支持定向恢复** —— 用户可以选择仅恢复若干并行中断点中的一个，而让其他中断点保持暂停。

临时方案（保存调用栈、从顶部重新运行）之所以失败，是因为：组件调用具有副作用（LLM API 调用、数据库写入），重新运行会重复执行已完成的工作，且如果没有完全相同的检查点状态，无法保证会走同一条执行路径。

## 2. 为什么困难

难点不在于保存状态 —— 而在于在**分层、并发、异构**的运行时中，以**正确的身份**在**正确的时机**保存**正确的**状态。

### 2.1 分层身份

执行点不是扁平的函数调用，它们形成一棵树：

```text
runnable:root;node:sub_graph_a;node:sub_graph_b;node:tools;tool:interrupt_tool:tool_call_123
                                            ^^^^^^^^^^^^^^^  ^^^^^^^  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                                            graph node         tools    工具名 : 工具调用 ID
```

如果没有稳定的、分层式的地址系统，运行时无法：
- 区分在同一个 `ToolsNode` 上并行运行的工具调用 `#1` 和工具调用 `#2`。
- 当用户说“继续工具调用 `#3`”时，将恢复数据路由到正确的叶子节点。
- 让包裹了独立 `Graph` 的 `Lambda` 节点将其自身的地址段正确地前置到内部图的地址之前。

### 2.2 并发性

Eino 使用类 Pregel（或 DAG 风格）的执行模型，多个节点可以同时运行。当 `ToolsNode` 并行运行 3 个工具且其中 2 个中断时，检查点必须：
- 记录 1 个已完成工具的输出。
- 记录 2 个暂停工具的中断信号。
- 为同样发生中断的子图节点存储子图检查点。

### 2.3 流物化

Eino 将流（`Stream`、`Transform`、`Collect`）作为一等执行模式支持。`StreamReader` 是一个临时的、一次性消费者 —— 一旦消费，它就消失了。在创建检查点之前，通道和输入中的所有流值必须物化为具体值（`compose/checkpoint.go:272` 中的 `convertCheckPoint`）。恢复时，这些具体值必须重新包装为 `StreamReader` 实例（`compose/checkpoint.go:291` 中的 `restoreCheckPoint`），以便下游的流模式节点接收到正确的类型。

### 2.4 遗留互操作性

Eino 原始的中断机制（`InterruptAndRerun`、`NewInterruptAndRerunErr`）是一个没有地址或唯一 ID 的扁平哨兵错误。现代系统（`Interrupt`、`StatefulInterrupt`、`CompositeInterrupt`）必须通过 `WrapInterruptAndRerunIfNeeded`（`compose/interrupt.go:78`）以及经过 `CompositeInterrupt` 的废弃路径（`compose/interrupt.go:181-213`）与遗留代码共存。

### 2.5 复合组件的双重性

包裹子 `Graph` 的 `ToolsNode` 或 `Lambda` 必须同时扮演两种角色：
1. **自指目标**：复合节点本身可能是恢复目标（例如，修改自身的内部状态）。
2. **管道**：如果恢复目标是后代节点，复合节点必须重新执行其子节点以让恢复上下文向下流动 —— 它不能消费该信号。

这种双重性编码在 `GetResumeContext` 的返回值 `isResumeTarget` 中（`compose/resume.go:77`）：`true` 且 `hasData = false` 表示“某后代是目标，向下传播”；`true` 且 `hasData = true` 表示“你本人是直接目标”。

## 3. 设计思路

Eino 的设计建立在四个支柱之上：

### 3.1 地址系统（`internal/core/address.go`）

每个执行上下文携带一个分层 `Address` —— 即 `[]AddressSegment` —— 存储在 Go context 的 `addrCtxKey{}` 键下。每个 `AddressSegment` 包含：

```go
// internal/core/address.go:69-77
type AddressSegment struct {
    ID    string              // 节点键、工具名、runnable 名称
    Type  AddressSegmentType  // "node"、"tool"、"runnable"
    SubID string              // 工具调用 ID，用于消歧义
}
```

定义了三种段类型（`compose/interrupt.go:275-286`）：
- `AddressSegmentNode`（`"node"`）—— 通过 `AddLambdaNode`、`AddGraphNode` 等添加的图节点。
- `AddressSegmentTool`（`"tool"`）—— `ToolsNode` 内部的工具调用。
- `AddressSegmentRunnable`（`"runnable"`）—— 独立的 `Graph` / `Workflow` / `Chain` 实例（由 `WithGraphName` 创建）。

`String()` 方法（`internal/core/address.go:35-53`）产生一个稳定的、可连接的表示形式：`runnable:root;node:sub_a;tool:my_tool:call_1`。

`AppendAddressSegment`（`internal/core/address.go:118-187`）在运行时进入新的执行作用域（图节点、工具调用、子 runnable）时被调用。它：
1. 通过扩展父地址来构建新的分层地址。
2. 检查 `globalResumeInfo`（全局恢复目标映射），判断新地址是否匹配已存储的中断状态或恢复数据。
3. 相应地在新的 `addrCtx` 上设置 `interruptState`、`isResumeTarget` 和 `resumeData`。

一个关键的设计选择：`isResumeTarget` 不仅在地址**精确**匹配恢复目标时设为 `true`，也会在存在一个恢复目标是当前地址的**后代**时设为 `true`（`internal/core/address.go:175-183`）。这正是让复合组件能够充当管道的机制 —— 它们知道有子节点需要恢复。

### 3.2 InterruptSignal 树（`internal/core/interrupt.go`）

中断机制的核心是 `InterruptSignal`（`internal/core/interrupt.go:43-49`）：

```go
type InterruptSignal struct {
    ID             string               // UUID
    Address        Address              // 分层地址
    InterruptInfo  InterruptInfo        // { Info any, IsRootCause bool }
    InterruptState InterruptState       // { State any, LayerSpecificPayload any }
    Subs           []*InterruptSignal   // 子信号（用于复合/虚拟节点）
}
```

`Subs` 字段使其成为一棵树而非扁平的列表。`CompositeInterrupt`（例如，来自一个并行运行 3 个工具调用且其中 2 个中断的 `ToolsNode`）产生一个带有 `Subs: [signal_tool_1, signal_tool_2]` 的父信号。这棵树随后可被序列化到检查点中，并在恢复时重建。

关键转换函数：
- `SignalToPersistenceMaps`（`internal/core/interrupt.go:327-349`）：将树扁平化为两个映射（`id2addr`、`id2state`），用于检查点存储。
- `ToInterruptContexts`（`internal/core/interrupt.go:254-294`）：将树转换为面向用户的扁平 `InterruptCtx` 对象列表（仅根因），每个对象带有一个 `Parent` 指针用于树遍历。
- `FromInterruptContexts`（`internal/core/interrupt.go:198-243`）：从扁平的 `InterruptCtx` 对象列表重建树 —— 用于跨执行环境桥接时（例如 ADK agent 工具）。

### 3.3 检查点持久化（`compose/checkpoint.go`）

`checkpoint` 结构体（`compose/checkpoint.go:106-117`）捕获完整的执行快照：

```go
type checkpoint struct {
    Channels       map[string]channel          // 在途通道值
    Inputs         map[string]any              // 待处理任务输入
    State          any                         // 图级状态
    SkipPreHandler map[string]bool             // 前置处理器跳过标记
    RerunNodes     []string                    // 需要重新运行的节点
    SubGraphs      map[string]*checkpoint      // 嵌套子图检查点
    InterruptID2Addr  map[string]Address       // 扁平化中断 ID → 地址
    InterruptID2State map[string]core.InterruptState // 扁平化中断 ID → 状态
}
```

关键设计决策：
- **嵌套子图**：每个子图节点的检查点存储在 `SubGraphs[nodeKey]` 中，允许递归嵌套。恢复时，`forwardCheckPoint`（`compose/checkpoint.go:157-168`）取出嵌套检查点并将其注入子图上下文中，同时将其从父图中删除以“仅转发一次”。
- **流转换**：`convertCheckPoint` 和 `restoreCheckPoint`（`compose/checkpoint.go:272-307`）通过 `streamConverter` 处理流与非流值之间的双向转换，该转换器由注册的 `streamConvertPair` 条目支持（`compose/checkpoint.go:309-373`）。
- **状态修改**：`WithStateModifier`（`compose/checkpoint.go:100`）允许注入一个 `StateModifier` 回调，在检查点读取/写入时调用，用于迁移或运行时增强。
- **检查点迁移**：`MigrateCheckpointState`（`compose/checkpoint.go:231-244`）是一个高级工具，用于跨框架版本升级检查点模式。

### 3.4 恢复上下文注入

恢复数据流有三个阶段：

**阶段 1 —— 用户提供恢复目标**：`Resume(ctx, id)` / `ResumeWithData(ctx, id, data)` / `BatchResumeWithData(ctx, map)`（`compose/resume.go:94-121`）将 `globalResumeInfo` 注入 context，将中断 ID 映射到恢复数据。

**阶段 2 —— 检查点恢复中断状态**：当图在恢复时加载检查点，`setCheckPointToCtx`（`compose/checkpoint.go:145-148`）调用 `core.PopulateInterruptState`（`internal/core/address.go:271-321`）将检查点的 `InterruptID2Addr` 和 `InterruptID2State` 映射合并到现有的 `globalResumeInfo` 中。这就是组件如何得知自己在之前的运行中曾被中断。

**阶段 3 —— 地址匹配分发状态**：当图创建任务时，`AppendAddressSegment`（上述步骤 3.1）将新地址与 `globalResumeInfo` 映射进行匹配，在每个叶子节点的 `addrCtx` 上设置 `interruptState` 和 `isResumeTarget`。

组件通过两个公开 API 读取此状态（`compose/resume.go:32-78`）：
- `GetInterruptState[T](ctx)` —— “我之前被中断过吗？这是我保存的状态。”
- `GetResumeContext[T](ctx)` —— “我是恢复目标吗？这是恢复数据。”

## 4. 源码走读

### 4.1 关键文件及其角色

| 文件 | 角色 |
|------|------|
| `compose/checkpoint.go` | `checkpoint` 结构体定义、序列化、流转换、`MigrateCheckpointState`、`WithCheckPointStore`、`WithCheckPointID`、`WithForceNewRun` |
| `compose/interrupt.go` | 公开中断 API：`Interrupt`、`StatefulInterrupt`、`CompositeInterrupt`、`WrapInterruptAndRerunIfNeeded`；`InterruptInfo` 结构体；`subGraphInterruptError`；`ExtractInterruptInfo` |
| `compose/resume.go` | 公开恢复 API：`GetInterruptState`、`GetResumeContext`、`Resume`、`ResumeWithData`、`BatchResumeWithData`、`AppendAddressSegment`、`GetCurrentAddress` |
| `internal/core/address.go` | `Address` / `AddressSegment` 类型，`AppendAddressSegment`（核心上下文构建器），`PopulateInterruptState`，`BatchResumeWithData`，`GetNextResumptionPoints` |
| `internal/core/interrupt.go` | `InterruptSignal` 树，`core.Interrupt`，`SignalToPersistenceMaps`，`ToInterruptContexts`，`FromInterruptContexts`，`CheckPointStore` 接口 |
| `internal/core/resume.go` | `GetInterruptState`、`GetResumeContext` 实现，`getRunCtx` |
| `compose/graph_run.go` | `handleInterrupt`（第 502 行），`handleInterruptWithSubGraphAndRerunNodes`（第 598 行），`resolveInterruptCompletedTasks`（第 457 行），`restoreCheckPointState`（第 382 行），`restoreTasks`（第 777 行），`createTasks`（第 735 行） |
| `compose/graph_call_options.go` | `WithGraphInterrupt`（外部取消，第 72 行） |
| `compose/tool_node.go` | 并行工具调用的 `ToolsNode` 复合中断处理 |

### 4.2 执行流程：中断

```
1. 节点返回 InterruptSignal（通过 Interrupt / StatefulInterrupt / CompositeInterrupt）
2. graph_run.resolveInterruptCompletedTasks（第 457 行）检测到信号
   - SubGraphInterruptError → 存入 tempInfo.subGraphInterrupts[nodeKey]
   - 带有 IsRootCause 的 InterruptSignal → 收集到 tempInfo.signals 中
   - Rerun 节点中断 → 存入 tempInfo.interruptRerunNodes
3. graph_run.handleInterrupt（第 502 行）或 handleInterruptWithSubGraphAndRerunNodes（第 598 行）：
   a. 构建包含 Channels、Inputs、State、SkipPreHandler、RerunNodes、SubGraphs 的检查点
   b. 调用 core.Interrupt(ctx, info, nil, tempInfo.signals) → 构建 InterruptSignal 树
   c. 调用 SignalToPersistenceMaps → 填充 cp.InterruptID2Addr、cp.InterruptID2State
   d. 调用 convertCheckPoint → 物化流数据
   e. 如果是子图：返回 subGraphInterruptError{CheckPoint: cp, Info: intInfo}
   f. 如果是根图：将 cp 持久化到 CheckPointStore，返回 interruptError{Info: intInfo}
```

### 4.3 执行流程：恢复

```
1. 用户调用 ResumeWithData(ctx, id, data) → 将 globalResumeInfo 注入 ctx
2. Graph.Invoke(ctx, input, WithCheckPointID(id)) → 从存储加载检查点
3. setCheckPointToCtx（checkpoint.go:145）：
   a. 调用 PopulateInterruptState → 将检查点的中断映射合并到 globalResumeInfo 中
   b. 将检查点放入 ctx 的 checkPointKey{} 键下
4. graph_run.restoreCheckPointState（第 382 行）：
   a. 读取 runCtx.isResumeTarget 和 runCtx.resumeData
   b. 如果是有数据的定向目标，则覆盖检查点状态
   c. 调用 convertCheckPoint → 物化检查点中的流数据
5. graph_run.run()（第 109 行）：从检查点恢复任务，重新执行图
6. createTasks（第 735 行）：对于每个新任务：
   a. 对子图节点调用 forwardCheckPoint
   b. 调用 AppendAddressSegment → 构建分层地址 → 匹配恢复数据
7. 组件接收在其 addrCtx 上设置了 interruptState + resumeData 的 ctx
8. 组件调用 GetInterruptState → 看到 wasInterrupted=true，获取到状态
9. 组件调用 GetResumeContext → 看到 isResumeTarget=true，获取到数据
```

### 4.4 子图中断传播

```
子图节点 → InterruptSignal
    ↓
子图 runner.handleInterrupt
    → 返回 subGraphInterruptError{CheckPoint, Info, signal}
    ↓
父图 resolveInterruptCompletedTasks
    → 通过 isSubGraphInterrupt 检测（interrupt.go:329）
    → 存入 tempInfo.subGraphInterrupts[nodeKey]
    → 将 signal 收集到 tempInfo.signals 中
    ↓
父图 handleInterruptWithSubGraphAndRerunNodes
    → cp.SubGraphs[nodeKey] = subGraphInterruptError.CheckPoint
    → intInfo.SubGraphs[nodeKey] = subGraphInterruptError.Info
    → 使用累积的 tempInfo.signals（包含子图的）调用 core.Interrupt
    → 构建具有正确父子关系的统一 InterruptSignal 树
```

恢复时，嵌套检查点通过 `forwardCheckPoint` 转发（`checkpoint.go:157-168`）：

```go
func forwardCheckPoint(ctx context.Context, nodeKey string) context.Context {
    cp := getCheckPointFromCtx(ctx)
    if subCP, ok := cp.SubGraphs[nodeKey]; ok {
        delete(cp.SubGraphs, nodeKey) // 仅转发一次
        return context.WithValue(ctx, checkPointKey{}, subCP)
    }
    return context.WithValue(ctx, checkPointKey{}, (*checkpoint)(nil))
}
```

## 5. 模式与示例

### 5.1 简单中断与恢复

一个 lambda 节点，使用类型化状态中断，并使用数据恢复：

```go
type myState struct{ OriginalInput string }
type myData struct{ Message string }

lambda := InvokableLambda(func(ctx context.Context, input string) (string, error) {
    wasInterrupted, hasState, state := GetInterruptState[*myState](ctx)
    if !wasInterrupted {
        return "", StatefulInterrupt(ctx,
            map[string]any{"reason": "need approval"},
            &myState{OriginalInput: input},
        )
    }
    // 已恢复
    isResume, hasData, data := GetResumeContext[*myData](ctx)
    if isResume && hasData {
        return "resumed: " + data.Message, nil
    }
    return "", StatefulInterrupt(ctx, "still waiting", state)
})
```

调用方提取中断信息并恢复：

```go
// 首次运行 → 中断
out, err := graph.Invoke(ctx, "input", WithCheckPointID("cp1"))
info, _ := ExtractInterruptInfo(err)
id := info.InterruptContexts[0].ID

// 恢复
ctx2 := ResumeWithData(context.Background(), id, &myData{Message: "go ahead"})
out, err = graph.Invoke(ctx2, "", WithCheckPointID("cp1"))
```

### 5.2 多子进程的复合组件

一个“批量”lambda，扇出 N 个并行子进程，每个子进程拥有自己的地址段和独立的中断/恢复循环：

```go
const PathProcess AddressSegmentType = "process"
processIDs := []string{"p0", "p1", "p2"}

batchLambda := InvokableLambda(func(ctx context.Context, _ string) (map[string]string, error) {
    _, _, batchState := GetInterruptState[*batchState](ctx)
    var errs []error
    for _, id := range processIDs {
        if _, done := batchState.Results[id]; done { continue }
        subCtx := AppendAddressSegment(ctx, PathProcess, id)
        res, err := runSubProcess(subCtx, id)
        if err != nil { errs = append(errs, err) }
        else { batchState.Results[id] = res }
    }
    if len(errs) > 0 {
        return nil, CompositeInterrupt(ctx, nil, batchState, errs...)
    }
    return batchState.Results, nil
})
```

关键模式：每个子进程通过 `AppendAddressSegment` 获得自己的地址段和中断状态，父节点使用 `CompositeInterrupt` 将所有子错误捆绑为一棵树。调用方看到的是 3 个扁平的 `InterruptCtx`（根因），这些根因共享一个父节点。

### 5.3 Lambda 内嵌 Graph 的中断传播

当 `Lambda` 节点包裹一个独立编译的 `Graph` 时，内部图的 `runnable` 地址段会自动前置（`compose/resume.go:131-133`）。lambda 充当复合节点：

```go
compositeLambda := InvokableLambda(func(ctx context.Context, input string) (string, error) {
    output, err := compiledInnerGraph.Invoke(ctx, input, WithCheckPointID("inner-cp"))
    if err != nil {
        if _, isInterrupt := ExtractInterruptInfo(err); isInterrupt {
            // 将内部图的中断向上传递，附带 lambda 自身的地址
            return "", CompositeInterrupt(ctx, "composite interrupt from lambda", nil, err)
        }
        return "", err
    }
    return output, nil
})
```

生成的地址：`runnable:root;node:composite_lambda;runnable:inner;node:inner_lambda`

### 5.4 带工具中断的 ReAct 风格重入

一种常见模式：`ChatModel` → `ToolsNode` 循环在工具调用时中断，恢复后模型可能再次调用同一工具 —— 重入调用必须拥有新的上下文（不标记为已中断）。测试在 `compose/resume_test.go:628`（`TestReentryForResumedTools`）中演示了这一点：在第二次调用时，`call_1` 被恢复（wasInterrupted=true, isResumeTarget=true），`call_2` 重新中断（wasInterrupted=true, isResumeTarget=false），而在第三次调用时，模型创建一个新的 `call_3`（wasInterrupted=false, isResumeTarget=false）。

### 5.5 检查点迁移

当图状态类型发生变化时，使用 `MigrateCheckpointState` 转换旧的检查点：

```go
newBytes, err := MigrateCheckpointState(oldBytes, serializer, func(state any) (any, bool, error) {
    if old, ok := state.(*OldStateType); ok {
        return old.ToNewType(), true, nil
    }
    return state, false, nil
})
```

迁移函数会递归应用于 `checkpoint.State` 和所有 `SubGraphs` 的状态（`compose/checkpoint.go:247-269`）。

## 6. 常见陷阱

### 6.1 忘记为遗留错误调用 `WrapInterruptAndRerunIfNeeded`

在复合组件内部使用已废弃的 `InterruptAndRerun` 或 `NewInterruptAndRerunErr` 时，错误必须在传递给 `CompositeInterrupt` 之前用 `WrapInterruptAndRerunIfNeeded`（`compose/interrupt.go:78`）进行包装。如果不包装，错误将缺少地址上下文，中断点的地址将为空。

### 6.2 `isResumeTarget` 为 false 时未重新中断

在显式定向恢复场景中，如果 `GetResumeContext` 返回 `isResumeTarget = false`，组件**必须**重新中断。否则，状态将丢失，图会像没有发生过中断一样继续运行 —— 可能导致错误结果或静默失败。正确的模式是：

```go
isResume, _, _ := GetResumeContext[myData](ctx)
if !isResume {
    return "", StatefulInterrupt(ctx, "still waiting", state) // 重新中断
}
```

### 6.3 回调处理器中的流泄漏

在编写流上下文的回调时，`OnStartWithStreamInput` 和 `OnEndWithStreamOutput` 处理器接收的是 `StreamReader` 的副本。这些副本**必须**关闭，否则会导致 goroutine / 内存泄漏。正确的模式是：

```go
func (h *myHandler) OnStartWithStreamInput(ctx context.Context, info *callbacks.RunInfo,
    input *schema.StreamReader[callbacks.CallbackInput]) context.Context {
    input.Close()
    return ctx
}
```

### 6.4 假设恢复是顺序进行的

并行中断（例如 3 个工具调用全部中断）可以独立且以任意顺序恢复。组件不应假定第一次恢复就处理了所有中断点 —— 每个中断点必须显式地被定向，且批量节点必须跟踪哪些子进程已完成。请参阅 `compose/resume_test.go:375`（`TestMultipleInterruptsAndResumes`）了解使用 `batchState.Results` 跟踪的正确模式。

### 6.5 子图状态未被更新

当子图节点中断时，其状态在 `info.SubGraphs[nodeKey].State` 中，而非 `info.State`。仅检查 `info.State` 的调用方会丢失子图的状态变更。同样，恢复时，子图的状态在 `cp.SubGraphs[nodeKey].State` 中，并通过 `forwardCheckPoint` 转发 —— 父节点不得覆盖它。

### 6.6 未注册序列化类型

中断状态或恢复数据中使用的自定义类型必须通过 `schema.RegisterName[T](name)` 或 `schema.Register[T]()` 进行注册，否则检查点序列化将失败。`compose/checkpoint_test.go` 中的示例使用了 `schema.Register[testStruct]()` 和 `schema.Register[*testPersistRerunInputState]()`。

### 6.7 混淆 `GetInterruptState` 与 `GetResumeContext`

`GetInterruptState` 回答的问题是“我之前被中断过吗 / 我的状态是什么？”—— 它在任何恢复运行中返回 true，无论该特定组件是否是恢复目标。`GetResumeContext` 回答的问题是“我是显式恢复目标吗 / 发送给我的数据是什么？”—— 仅当该特定地址被定向时才返回 true。一个被中断过但不是当前恢复目标的组件必须重新中断。

## 7. Rive 可以借鉴的地方

### 7.1 执行点身份应当是结构性的，而非描述性的

Eino 的地址是从图拓扑构建的类型化、带 ID 段组成的确定性链（`compose/resume.go:123-140`）。它不是自然语言描述或调用栈。对于 Rive 的 dispatch/resume 模型，这意味着：恢复点应由 `run_id + node_id + dispatch_id + subdispatch_id` 来标识，而非“卡在 token 限制上的那个 worker”。

### 7.2 复合 Dispatch 即管道（Conduit）模式

Eino 的 `isResumeTarget` 带 `hasData = false` 用于后代节点，对复合组件来说是一种干净的模式。Rive 中扇出子调度的 dispatch（相当于 `ToolsNode` 或批量 `Lambda`）需要同样的双重性：dispatch 本身可以是恢复目标，同时它必须透明地将恢复信号向下转发给其子节点。

### 7.3 面向用户的扁平上下文与树状状态

Eino 的 `ToInterruptContexts` 产生一个扁平的根因列表（用户关心的叶子节点），而 `InterruptSignal.Subs` 保留树结构以便正确的状态持久化和重建。Rive 同样应向人类暴露扁平的“需要恢复的事项”视图，同时在内部保留父子树以保证正确的状态传播。

### 7.4 流 / 状态双重性是迁移关注点

Eino 的 `convertCheckPoint` / `restoreCheckPoint` 模式用于流物化，它特定于进程内流式处理，但其中的原则具有普适性：任何临时的、一次性数据（流、连接池、在途 RPC）必须在创建检查点之前物化，并在恢复时重新挂载。涉及流式传输的 Rive dispatch 应识别无法跨越检查点边界的类似临时状态。

### 7.5 检查点迁移是框架级的关注点

Eino 的 `MigrateCheckpointState`（`compose/checkpoint.go:231-244`）提供了一个递归状态迁移器，可在无需用户修改代码的情况下升级检查点模式。对于 Rive，持久化状态的长时间运行的 dispatch（例如数分钟到数小时）需要同样的能力：一个框架级钩子，将旧的状态形状转换为新的，并在恢复时自动应用。
