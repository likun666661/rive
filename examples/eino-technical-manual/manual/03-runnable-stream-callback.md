# 03 — Runnable / Stream / Callback 运行时模式

## 1. 面临的问题

Eino 图编译的产出是 `Runnable[I, O]` —— 一个统一了四种数据流模式的单一接口：

```go
// compose/runnable.go:32-37
type Runnable[I, O any] interface {
    Invoke(ctx context.Context, input I, opts ...Option) (output O, err error)
    Stream(ctx context.Context, input I, opts ...Option) (output *schema.StreamReader[O], err error)
    Collect(ctx context.Context, input *schema.StreamReader[I], opts ...Option) (output O, err error)
    Transform(ctx context.Context, input *schema.StreamReader[I], opts ...Option) (output *schema.StreamReader[O], err error)
}
```

Eino 图中的每个节点 —— 无论是 ChatModel、Retriever、Lambda 还是嵌套子图 —— 都被编译为 `composableRunnable`，并通过这四种模式之一执行。运行时必须：

1. 接受只实现了这四种方法子集的组件（例如只暴露 `Generate` 和 `Stream` 的模型），并自动推导出缺失的方法。
2. 在正确的生命周期节点触发回调（callback），而无需组件作者手动编写回调分发逻辑。
3. 正确处理流的复制（stream copying）—— 将单个流扇出到 N 个并发消费者（下游节点 + 回调处理器），不会出现死锁或 goroutine 泄漏。
4. 将组件级的抽象（`BaseChatModel`、`Retriever`、`ChatTemplate` 等）转换为统一的图节点，使图调度器无需了解组件特定的内部细节。

以上任何一点如果处理不当，图执行要么静默丢弃流数据，要么泄漏 goroutine，要么使用过期的上下文触发回调，要么因类型不匹配而 panic。

## 2. 为什么这么难

**多模式降级组合爆炸。** 四种方法（I、S、C、T）以及组件可能实现的任意子集，意味着运行时需要 `4 * 3 = 12` 个降级函数。优先级顺序很重要：通过 Transform 调用比通过 Stream 调用更廉价（一次流转换 vs 两次），通过 Stream 调用比通过 Collect 调用更廉价（一次 concat vs 两次）。`newRunnablePacker`（`compose/runnable.go:336-400`）中的分配顺序是经过精心选择的，但除了代码本身没有任何文档记录。

**流复制具有微妙的所属权语义。** `schema.StreamReader.Copy`（`schema/stream.go:261-275`）使用基于链表结构的共享缓冲区（`parentStreamReader` + `childStreamReader`，第 784-898 行）将单个源扇出为 N 个独立的子 reader。每个子 reader 在消费后**必须**调用 `Close()`。如果任何子 reader 未关闭，父 reader 就永远不会关闭原始 reader，从而导致底层 channel goroutine 泄漏。回调处理器通过 `OnStartWithStreamInputHandle` / `OnEndWithStreamOutputHandle`（`internal/callbacks/inject.go:163-193`）自动接收流副本 —— 如果处理器忘记关闭自己的副本，整个管线就会泄漏。

**回调上下文链是处理器隔离的，而非全局有序的。** 每个处理器接收到的是**同一个**处理器的上一个阶段返回的上下文，但上下文不会从一个处理器流向另一个处理器（`callbacks/interface.go:67-71`）。如果某个处理器通过 `context.WithValue` 修改上下文并假设下一个处理器能看到它，那它一定会感到意外。

**组件到节点的转换必须协调两种运行时模型。** 像 `BaseChatModel`（`components/model/interface.go`）这样的组件拥有 `Generate` + `Stream`，而图引擎通过 `composableRunnable` 使用 `Invoke`/`Stream`/`Collect`/`Transform`。转换还必须设置 `executorMeta` —— 包括组件类别、实现类型，以及组件是否自行上报回调 —— 以便图运行时正确决定何时触发 `OnStart`/`OnEnd`，以及何时跳过。

## 3. 设计思路

Eino 将此问题拆分为四个协作层：

**第一层：Runnable 接口 + packer。** `compose/runnable.go` 定义了 `Runnable[I,O]` 和 `composableRunnable`，即每个图节点编译后的内部包装。`runnablePacker` 接收组件的原始函数指针（`Invoke`、`Stream`、`Collect`、`Transform`），并通过自动降级填充缺失的方法。

**第二层：流原语。** `schema/stream.go` 提供了 `StreamReader`/`StreamWriter`（基于 goroutine channel）、用于扇出的 `Copy`、用于扇入的 `MergeStreamReaders`，以及用于类型转换的 `StreamReaderWithConvert`。`compose/stream_reader.go` 将其包装为内部的 `streamReader` 接口供 `composableRunnable` 使用 —— 增加了 `close()`、`copy(n)`、`merge()`、`mergeWithNames()`、`withKey()` 和 `toAnyStreamReader()`。

**第三层：回调引擎。** `callbacks/` 暴露了包含五个阶段的公开 `Handler` 接口。`internal/callbacks/` 管理每次调用的处理器列表，分发各个阶段，并强制执行 `TimingChecker` 以跳过不必要的流复制。其机制位于 `compose/utils.go`（`invokeWithCallbacks`、`streamWithCallbacks` 等），每个执行模式都通过 `runWithCallbacks` 进行包装。

**第四层：组件到节点的桥接。** `compose/component_to_graph_node.go` 为每种组件类型提供了一个函数（`toChatModelNode`、`toRetrieverNode`、`toToolsNode` 等）。每个函数调用 `toComponentNode`，后者通过 `parseExecutorInfoFromComponent` 构建 `executorMeta`（检查 `components.Typer` 和 `callbacks.Checker`），并通过 `runnableLambda` 将组件的方法包装为 `composableRunnable`。

核心洞察：**图调度器从不直接接触组件 —— 它只接触 `composableRunnable`**。这使得调度器对所有组件类型保持通用，回调/恢复逻辑因此可以在整个系统中复用。

## 4. 源码走读

### 4.1 Runnable 与自动降级（`compose/runnable.go`）

核心函数是 `newRunnablePacker`（第 336 行）。它最多接收四个函数指针（有些可能为 nil），并填充全部四个方法。每个方法的优先级级联如下：

- **Invoke**：优先使用原生 I，否则 `invokeByStream`（第 194 行：调用 Stream，合并 stream reader），否则 `invokeByCollect`（第 205 行：将输入包装为数组流，调用 Collect），否则 `invokeByTransform`（第 213 行：数组流 → Transform → 合并输出流）。
- **Stream**：优先使用原生 S，否则 `streamByTransform`（第 226 行：数组流输入 → Transform），否则 `streamByInvoke`（第 234 行：Invoke → 将输出包装为数组流），否则 `streamByCollect`（第 245 行：数组流输入 → Collect → 将输出包装为数组流）。
- **Collect**：优先使用原生 C，否则 `collectByTransform`（Transform → 合并输出），否则 `collectByInvoke`（合并输入 → Invoke），否则 `collectByStream`（合并输入 → Stream → 合并输出）。
- **Transform**：优先使用原生 T，否则 `transformByStream`（合并输入 → Stream），否则 `transformByCollect`（Collect → 将输出包装为数组流），否则 `transformByInvoke`（合并输入 → Invoke → 将输出包装为数组流）。

每个降级函数仅通过名称进行文档说明 —— 函数签名之外没有任何额外注释。其正确性依赖于 `concatStreamReader`（`compose/stream_concat.go:50-88`），该函数消费 `StreamReader` 中的所有数据块，通过 `internal.ConcatItems` 将它们拼接起来（后者分发到由 `RegisterStreamChunkConcatFunc` 注册的类型特定的 concat 函数，第 44 行），并**始终关闭 reader**（第 51 行：`defer sr.Close()`）。

**重要细节**：如果组件只实现了 `Stream()` 但用户调用了 `Invoke()`，运行时会调用 `invokeByStream`，后者先调用 `Stream`，再调用 `concatStreamReader`。如果该流类型没有注册 concat 函数，`internal.ConcatItems` 会回退到返回最后一个数据块 —— 这对于像 `schema.Message` 这样的"增量更新"类型是有效的（每个数据块都是覆盖前一个的完整消息）。

`composableRunnable`（第 46 行）只存储了两个内部执行函数（`i invoke` 和 `t transform`），外加元数据（`inputType`、`outputType`、`optionType`、`isPassthrough`、`meta`、`nodeInfo`）。另外两种模式（Stream、Collect）在图运行时通过 `toGenericRunnable`（第 402 行）从这两个函数派生。

### 4.2 流 reader 内部实现（`compose/stream_reader.go`）

内部的 `streamReader` 接口（第 26 行）包装了 `schema.StreamReader`，增加了 compose 运行时所需的操作：

```go
type streamReader interface {
    copy(n int) []streamReader
    getType() reflect.Type
    getChunkType() reflect.Type
    merge([]streamReader) streamReader
    withKey(string) streamReader
    close()
    toAnyStreamReader() *schema.StreamReader[any]
    mergeWithNames([]streamReader, []string) streamReader
}
```

`packStreamReader` / `unpackStreamReader`（第 111-128 行）使用泛型包装 `streamReaderPacker[T]` 在带类型的 `*schema.StreamReader[T]` 和不带类型的 `streamReader` 之间转换。`copy` 方法（第 45 行）委托给 `sr.Copy(n)` 并重新包装每个副本。`merge` 方法（第 79 行）调用 `schema.MergeStreamReaders`。`withKey` 方法（第 95 行）使用 `schema.StreamReaderWithConvert` 将每个数据块 `T` 映射为 `map[string]any{key: T}`。

`unpackStreamReader` 函数对接口类型有特殊处理（第 121-127 行）：如果 `T` 是接口，它先将流转换为 `*StreamReader[any]`，然后用带类型的转换包装回 `T`。这使得当图中存在动态类型输出的节点时，运行时类型擦除能够正确工作。

### 4.3 流拼接（`compose/stream_concat.go`）

`concatStreamReader[T]`（第 50 行）是每次需要将流折叠为单个值的降级操作的核心。它执行以下步骤：

1. 延迟执行 `sr.Close()`（第 51 行）—— 对资源清理至关重要。
2. 循环调用 `sr.Recv()` 直到 `io.EOF`。
3. 跳过来自合并流的 `SourceEOF` 错误（第 62 行：`schema.GetSourceName(err)` —— 当多流合并发出每个源的完成信号时很有用）。
4. 将所有数据块累积到一个切片中。
5. 如果切片为空，返回 `emptyStreamConcatErr`。
6. 如果恰好只有一个数据块，直接返回（无 concat 开销）。
7. 否则，调用 `internal.ConcatItems(items)` 构建最终值。

公开的 `RegisterStreamChunkConcatFunc[T]`（第 44 行）允许用户为其类型注册自定义的 concat 逻辑。注册不是线程安全的，必须在程序初始化时完成。文档示例展示了如何通过取一个字段的最新值并对另一个字段求和来拼接结构体 —— 这是流式 token 聚合中的典型模式。

### 4.4 回调接口（`callbacks/interface.go` + `internal/callbacks/interface.go`）

**公开的 `RunInfo`**（`callbacks/interface.go:41`）：三个字段：
- `Name`：来自 `compose.WithNodeName` 的图节点名称，未调用 `InitCallbacks` 的独立组件则为空。
- `Type`：来自 `components.Typer` 的实现标识（如 `"OpenAI"`）；回退为反射推导的类型名。
- `Component`：来自 `components.Component` 的类别常量（如 `ComponentOfChatModel`、`ComponentOfRetriever`）。对于图级调用固定为 `"Graph"`/`"Chain"`/`"Workflow"`，Lambda 为 `"Lambda"`。

**五个回调阶段**（`callbacks/interface.go:114-134`）：
| 常量 | 时机 | 输入/输出 |
|---|---|---|
| `TimingOnStart` | 组件运行前 | `CallbackInput`（值） |
| `TimingOnEnd` | 组件成功后 | `CallbackOutput`（值） |
| `TimingOnError` | 组件返回错误 | `error` |
| `TimingOnStartWithStreamInput` | 组件接收流输入（Collect/Transform） | `*StreamReader[CallbackInput]`（副本） |
| `TimingOnEndWithStreamOutput` | 组件产生流输出（Stream/Transform） | `*StreamReader[CallbackOutput]`（副本） |

**`TimingChecker`**（`callbacks/interface.go:136-145`）：处理器上的可选接口。框架在每个阶段前调用 `Needed(ctx, info, timing)` 来决定是否需要分配流副本和 goroutine。通过 `HandlerBuilder`（`callbacks/handler_builder.go`）构建的处理器会自动实现 `TimingChecker` —— `Needed` 只在用户显式设置函数时才返回 `true`。

**内部引擎**（`internal/callbacks/inject.go`）：函数 `On[T]`（第 74 行）是所有阶段的统一分发入口。它：
1. 从上下文中拉取 `manager`。
2. 收集需要此阶段的处理器（通过 `TimingChecker` 过滤）。
3. 使用处理器列表调用 `Handle[T]` 函数。
4. 返回更新后的上下文和（可能已复制的）值。

流处理器（`OnStartWithStreamInputHandle`、`OnEndWithStreamOutputHandle`，第 163-193 行）使用 `OnWithStreamHandle`（第 143 行），后者调用 `cpy(len(handlers) + 1)` 创建 N+1 份输入流的副本 —— N 份给处理器，1 份给实际消费者。**每个处理器接收到自己的私有副本**，并且必须关闭它。

**全局 vs 每次调用的处理器**（`internal/callbacks/manager.go`）：`manager` 同时持有 `globalHandlers`（程序初始化时通过 `AppendGlobalHandlers` 一次性设置）和 `handlers`（每次图调用时通过 `compose.WithCallbacks` 设置）。全局处理器优先执行（对于必须观测一切的仪表化功能具有更高优先级）。

### 4.5 compose 中的回调包装（`compose/utils.go`）

`runWithCallbacks`（第 100 行）是通用的包装器：

```go
func runWithCallbacks[I, O, TOption any](r func(...) (O, error),
    onStart on[I], onEnd on[O], onError on[error]) func(...) (O, error) {
    return func(ctx context.Context, input I, opts ...TOption) (output O, err error) {
        ctx, input = onStart(ctx, input)
        output, err = r(ctx, input, opts...)
        if err != nil {
            ctx, err = onError(ctx, err)
            return output, err
        }
        ctx, output = onEnd(ctx, output)
        return output, nil
    }
}
```

四种模式特定的包装器为：

- `invokeWithCallbacks`：onStart（值）→ Invoke → onEnd（值）
- `streamWithCallbacks`：onStart（值）→ Stream → onEndWithStreamOutput（流）
- `collectWithCallbacks`：onStartWithStreamInput（流）→ Collect → onEnd（值）
- `transformWithCallbacks`：onStartWithStreamInput（流）→ Transform → onEndWithStreamOutput（流）

`initGraphCallbacks` / `initNodeCallbacks`（第 152-207 行）在每个节点执行前在上下文中设置 `RunInfo`。它们检查 `executorMeta.component` 获取组件类别，检查 `executorMeta.componentImplType` 获取实现类型。每个节点的回调可以通过带有 `NodePath` 过滤的 `compose.WithCallbacks` 进行范围限定（第 188-200 行）。

### 4.6 组件到图节点的转换（`compose/component_to_graph_node.go`）

每种组件类型都有专用的转换函数。例如 `toChatModelNode`（第 93 行）：

```go
func toChatModelNode(node model.BaseChatModel, opts ...GraphAddNodeOpt) (*graphNode, *graphAddNodeOpts) {
    return toComponentNode(
        node,
        components.ComponentOfChatModel,
        node.Generate,  // Invoke
        node.Stream,    // Stream
        nil,            // Collect（未实现）
        nil,            // Transform（未实现）
        opts...)
}
```

`toComponentNode`（第 29 行）做三件事：
1. 调用 `parseExecutorInfoFromComponent`（`compose/graph_node.go:151-163`）构建 `executorMeta` —— 设置 `component`、`componentImplType`（来自 `components.GetType`）和 `isComponentCallbackEnabled`（来自 `components.IsCallbacksEnabled`）。
2. 使用四个方法指针调用 `runnableLambda`，后者调用 `newRunnablePacker` —— 通过自动降级填充缺失的 Collect/Transform（Collect 通过 `collectByStream`，Transform 通过 `transformByStream`）。
3. 调用 `toNode` 将所有内容包装为 `graphNode` 结构体。

**关于 `isComponentCallbackEnabled` 的关键细节**：`runnableLambda` 调用将 `!meta.isComponentCallbackEnabled` 作为 `enableCallback` 标志传入（第 41 行）。该标志是取反的 —— 如果组件说"我自己处理回调"（通过 `callbacks.Checker`），compose 层就**不**会用 `runWithCallbacks` 包装组件的方法。否则，compose 会包装它们。这防止了回调的重复触发。

透传节点（`toPassthroughNode`，第 186 行）是一个零开销的恒等节点 —— 其 `Invoke` 原样返回输入，`Transform` 原样返回输入流。它被图编译用于插入合成的路由节点。

### 4.7 回调上下文生命周期（`internal/callbacks/inject.go`）

`On[T]` 分发函数（第 74 行）有一个微妙的 `start` 参数：

- 当 `start = true`（执行的第一个阶段）时：manager 的 `runInfo` 被消费并存储到上下文中（通过 `CtxRunInfoKey`）。这防止了在嵌套调用上的重复分发。
- 当 `start = false`（后续阶段）时：如果 manager 仍然有 `runInfo`（因为子组件调用了 `EnsureRunInfo`），则使用它；否则从上下文中的 `CtxRunInfoKey` 获取。

该机制确保当子图节点调用 `EnsureRunInfo` 提供自己的 `RunInfo` 时，该节点内部后续的回调阶段使用子图的 `RunInfo`，而非父图的。

## 5. 模式与示例

### 模式 1：只有 Invoke 的组件

Retriever 只实现了 `Retrieve`（映射到 Invoke）。Eino 将其包装为：

```
Invoke   → 原生 Retrieve
Stream   → invokeByStream       // 调用 Retrieve，将结果包装为数组流
Collect  → collectByInvoke      // 合并输入流，调用 Retrieve
Transform → transformByInvoke   // 合并输入流，调用 Retrieve，包装为数组流
```

图的用户可以对这样的节点调用 `.Stream()` 并且能正常工作，即使 retriever 从未实现流式功能。

### 模式 2：拥有 Invoke + Stream 的 ChatModel

`BaseChatModel` 同时拥有 `Generate`（Invoke）和 `Stream`。缺失的 Collect 和 Transform 被派生：

```
Collect   → collectByStream     // 合并输入，Stream，合并输出
Transform → transformByStream   // 合并输入，Stream
```

这意味着 ChatModel 节点可以接收流输入（来自上游的流式 prompt 链）并产生流输出。`collectByStream` 中的 concat 操作是桥接非流式输入到流式输出的代价。

### 模式 3：使用 HandlerBuilder 为每个阶段编写回调

```go
handler := callbacks.NewHandlerBuilder().
    OnStartFn(func(ctx context.Context, info *callbacks.RunInfo, input callbacks.CallbackInput) context.Context {
        if mi := model.ConvCallbackInput(input); mi != nil {
            log.Printf("[%s] messages: %d", info.Name, len(mi.Messages))
        }
        return ctx
    }).
    OnEndWithStreamOutputFn(func(ctx context.Context, info *callbacks.RunInfo, output *schema.StreamReader[callbacks.CallbackOutput]) context.Context {
        defer output.Close() // 必须执行
        for {
            chunk, err := output.Recv()
            if errors.Is(err, io.EOF) { break }
            // 从流中累积 token 计数
        }
        return ctx
    }).
    Build()
```

`HandlerBuilder.Build()` 调用（`callbacks/handler_builder.go:161`）返回一个 `handlerImpl`，它实现了 `TimingChecker` —— 在此示例中，`Needed` 只对 `OnStartFn` 和 `OnEndWithStreamOutputFn` 返回 true，另外三种阶段以零开销跳过。

### 模式 4：全局处理器处理横切关注点

```go
func init() {
    callbacks.AppendGlobalHandlers(
        tracingHandler,   // 总是首先执行
        metricsHandler,   // 总是第二执行
    )
}
```

全局处理器在 `On` 分发函数中**先于**每次调用的处理器执行（`internal/callbacks/inject.go:95`）。这意味着分布式追踪能看到每个组件的每次调用，无论应用代码传入了什么处理器。

### 模式 5：路径范围限定的回调

```go
r.Invoke(ctx, input,
    compose.WithCallbacks(
        compose.WithNodePath("tool_node", "calculator"),
    ).DesignateHandler(calcHandler),
)
```

`initNodeCallbacks` 函数（`compose/utils.go:190-200`）将 `NodePath` 与当前节点 key 进行匹配。如果路径匹配，则追加该处理器；否则节点通过 `ReuseHandlers` 复用上下文中已有的处理器。

## 6. 常见陷阱

### 陷阱 1：回调处理器中泄漏 stream reader

这是最危险的问题。`OnEndWithStreamOutput` 和 `OnStartWithStreamInput` 处理器接收流的**私有副本**。如果处理器不关闭该副本，`parentStreamReader.close()`（`schema/stream.go:868-881`）永远不会递增其 `closedNum`，因此原始流永远不会被关闭，`toStream()`（第 747-778 行）中的 goroutine 将永远泄漏。

**修复方法**：在流处理器中始终使用 `defer sr.Close()`。`HandlerBuilder` 的文档字符串（`callbacks/handler_builder.go:141, 153`）明确警告了这一点。

### 陷阱 2：假设回调处理器的执行顺序

上下文不**会**在不同处理器之间流动。每个处理器的 `OnStart` → `OnEnd` 链是独立的。如果处理器 A 通过 `context.WithValue` 设置了一个值，而处理器 B 期望能够读取它，处理器 B 会得到 nil。

**修复方法**：对串行状态使用单个处理器，或使用安全的全局并发状态。

### 陷阱 3：修改回调输入/输出值

`OnStart` 和 `OnEnd` 处理器接收的指针指向与其他节点和处理器使用的**同一个对象**（直接赋值，而非深拷贝 —— `callbacks/interface.go:82-84`）。修改这些值会在并发的图执行中导致数据竞争。

### 陷阱 4：忘记注册流 concat 函数

如果组件只实现了 `Stream()` 而用户调用了 `Invoke()`，运行时会使用 `concatStreamReader` 来折叠流。如果没有为数据块类型注册 concat 函数，`internal.ConcatItems` 会回退到最后一个数据块。对于不遵循"最后一个数据块即为最终答案"语义的类型（例如流式数字计数器，其中每个数据块是一个增量），这会产生错误的结果。

**修复方法**：在 init 时对自定义数据块类型调用 `compose.RegisterStreamChunkConcatFunc`。

### 陷阱 5：嵌套图和组件导致回调重复触发

如果 ChatModel 的实现通过 `callbacks.Checker` 自行实现了回调分发，但 compose 层又通过 `runWithCallbacks` 对其进行了包装，每个阶段都会触发两次。Eino 通过 `executorMeta` 中的 `isComponentCallbackEnabled` 来防止这一点 —— 但前提是组件正确实现了 `Checker`。

### 陷阱 6：在 Copy 之后进行 Recv

`StreamReader.Copy()` **必须**在第一次 `Recv()` 之前调用。原始 reader 在 Copy 之后就会变得不可用。`copyStreamReaders` 函数（`schema/stream.go:792-821`）将原始 reader 替换为一个惰性从源获取数据的 `parentStreamReader` —— 但此替换是不可逆的。如果调用方在 Copy 之后尝试对原始 reader 调用 `Recv()`，会直接 panic。

## 7. Rive 可以学到什么

**稳定的执行地址，而非函数调用。** Eino 的 `RunInfo`（Name + Type + Component）为图中的每个执行点构成了一个稳定的标识。Rive 可以采用类似的结构化标识：`{dispatchId, workerId, executionAttempt}`，而不是依赖自然语言描述来识别节点。

**流生命周期 = 资源生命周期。** Eino 将 stream reader 视为具有强制关闭语义的资源。回调引擎创建 N+1 个副本，并为每个消费者精确分配一个。Rive 的分发协议可以从类似的流式数据模型在 worker 之间传输中受益：显式的 fork 时复制、强制的关闭确认、以及消费者未能关闭时的自动清理。

**自动能力降级作为迁移策略。** Eino 的降级矩阵允许组件独立演化 —— 一个组件可以添加 `Stream()` 支持而无需更新每一个调用方。Rive 的 worker 同样可以声明能力级别（`supports_streaming`、`supports_resume`），协议可以自动桥接不同的能力级别。

**TimingChecker 作为成本模型。** Eino 在处理器不需要某个阶段时跳过流复制和分配。Rive 可以将同样的思路应用于可观测性：worker 声明它们想要观测哪些生命周期事件，协议对未订阅的事件省略仪表化开销。

**组件无关的调度。** Eino 的调度器从不接触 `ChatModel` 或 `Retriever` —— 它只接触 `composableRunnable`。这使得可以在不触及调度器的情况下添加新的组件类型。Rive 同样可以定义统一的工作单元接口，使调度器无需理解 SQL 查询、Python 脚本或 API 调用。
