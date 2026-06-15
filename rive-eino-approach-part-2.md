# Eino 复刻版设计分析：第三章 Runnable / Stream / Callback 与第四章 Checkpoint / Interrupt / Resume

> 分析目标：解释为什么只会 Invoke 不够、为什么需要 streaming/callback 观测面、以及 checkpoint/interrupt/resume 怎样把运行过程变成可恢复过程。
> 语言：中文

---

## 第三章：Runnable / Stream / Callback — 为什么只会 Invoke 不够

### 1. 面临的问题

Eino 图编译的产出是 `Runnable[I, O]` —— 一个统一了四种数据流模式的单一接口：

```go
Invoke(ctx, I) → (O, error)          // 值进值出
Stream(ctx, I) → (*StreamReader[O])   // 值进流出
Collect(ctx, *StreamReader[I]) → (O)  // 流进值出
Transform(ctx, *StreamReader[I]) → (*StreamReader[O]) // 流进流出
```

图中每个节点——无论是 ChatModel、Retriever、Lambda 还是嵌套子图——都被编译为 `composableRunnable`，并通过这四种模式之一执行。**如果只有 Invoke，以下场景全部不可行：**

| 场景 | 仅有 Invoke 时的失败模式 |
|------|------------------------|
| 用户想看 LLM 逐 token 输出 | 必须等 Generate 返回完整结果，失去了流式体验 |
| 上游节点产生流式输出，下游需要消费并继续流式处理 | 流被折叠为单值再展开，增加一次 concat + 一次展开开销 |
| Retriever 只实现了 `Retrieve`，但用户对它调用了 `.Stream()` | 直接报错 "Stream not supported" |
| 需要对每个节点的执行注入可观测性（计时、日志、追踪） | 没有回调钩子，只能修改节点源码 |

**核心问题：图运行时需要一种统一的、能自动适配所有执行模式的抽象，同时提供一个不与业务逻辑耦合的观测面。**

---

### 2. 为什么这么难

#### 2.1 多模式降级组合爆炸

四种方法（I、S、C、T），组件可能只实现任意子集。运行时需要 **12 个降级函数** 来填充缺失的方法：

```
Invoke 降级链:   Native I → invokeByStream(I→S→concat) → invokeByCollect → invokeByTransform
Stream 降级链:   Native S → streamByTransform → streamByInvoke(I→wrap as stream) → streamByCollect
Collect 降级链:  Native C → collectByTransform → collectByInvoke → collectByStream
Transform 降级链: Native T → transformByStream → transformByCollect → transformByInvoke
```

优先级顺序不是任意的：通过 Transform 调用比通过 Stream 调用更廉价（一次流转换 vs 两次），通过 Stream 调用比通过 Collect 调用更廉价（一次 concat vs 两次）。这个顺序是经过精心选择的，但除了代码本身没有文档记录。

#### 2.2 流复制具有微妙的所属权语义

`schema.StreamReader.Copy` 使用共享缓冲区（`parentStreamReader` + `childStreamReader` 链表）将单个源扇出为 N 个独立子 reader。每个子 reader 在消费后**必须**调用 `Close()`。如果任何子 reader 未关闭，父 reader 永远不关闭原始 reader，底层 channel goroutine 永久泄漏。

回调引擎通过 `OnStartWithStreamInputHandle` / `OnEndWithStreamOutputHandle` 自动接收流副本——创建 **N（处理器数量）+ 1（实际消费者）** 份副本。如果回调处理器忘记关闭自己的副本，整个管线泄漏。

#### 2.3 回调上下文链是处理器隔离的，而非全局有序的

每个处理器接收到的是**同一个**处理器的上一个阶段返回的上下文，但上下文不会从一个处理器流向另一个处理器。如果某个处理器通过 `context.WithValue` 修改上下文并假设下一个处理器能看到它，那它一定会感到意外。

#### 2.4 组件到节点的转换必须协调两种运行时模型

ChatModel 拥有 `Generate` + `Stream`，而图引擎通过 `composableRunnable` 使用 `Invoke`/`Stream`/`Collect`/`Transform`。转换还必须设置 `executorMeta`——包括组件类别、实现类型，以及组件是否自行上报回调——以便图运行时正确决定何时触发 OnStart/OnEnd，以及何时跳过以防止回调重复触发。

---

### 3. Eino 的设计思路

Eino 将此问题拆分为四个协作层：

**第一层：Runnable 接口 + packer。** `compose/runnable.go` 定义 `Runnable[I,O]` 和 `composableRunnable`。`runnablePacker` 接收组件的原始函数指针（最多 4 个，部分可为 nil），通过自动降级填充全部 4 个方法。

**第二层：流原语。** `schema/stream.go` 提供 `StreamReader`/`StreamWriter`（基于 goroutine channel）、扇出 `Copy`、扇入 `MergeStreamReaders`、类型转换 `StreamReaderWithConvert`。内部 `streamReader` 接口增加 `close()`、`copy(n)`、`merge()`、`mergeWithNames()`、`withKey()` 等操作。

**第三层：回调引擎。** `callbacks/` 暴露包含五个阶段的 `Handler` 接口。`internal/callbacks/` 管理每次调用的处理器列表，分发各个阶段，并强制执行 `TimingChecker` 跳过不必要的流复制。四种执行模式各有唯一的回调包装路径。

**第四层：组件到节点的桥接。** `compose/component_to_graph_node.go` 为每种组件类型提供转换函数。每个函数通过 `toComponentNode` 构建 `executorMeta`（检查 `components.Typer` 和 `callbacks.Checker`），并通过 `runnableLambda` 将组件方法包装为 `composableRunnable`。

**核心洞察：图调度器从不直接接触组件——它只接触 `composableRunnable`。** 这使得调度器对所有组件类型保持通用，回调/恢复逻辑因此可以在整个系统中复用。

---

### 4. 复刻版如何落地

#### 4.1 Runnable 四模式与降级矩阵 (`compose/runnable.go`)

复刻版实现了完整的教学版 `composableRunnable`，携带四个函数指针：

```go
type composableRunnable struct {
    i func(ctx context.Context, input any) (output any, err error)
    s func(ctx context.Context, input any) (output any, err error)
    c func(ctx context.Context, input any) (output any, err error)
    t func(ctx context.Context, input any) (output any, err error)
}
```

每个模式的 fallback 优先级与 Eino 一致：

- **Invoke**: Native I → Stream + concat → Collect + stream input → Transform + concat
- **Stream**: Native S → Transform + stream input → Invoke + wrap → Collect + stream input + wrap
- **Collect**: Native C → Transform + concat → concat input + Invoke → concat input + Stream + concat
- **Transform**: Native T → concat input + Stream → Collect + wrap → concat input + Invoke + wrap

降级原语是 `recvAll`（消费整个流）和 `collected`（单元素直接返回、多元素作为切片返回）。

#### 4.2 Pipe Stream (`compose/stream.go`)

复刻版用 `PipeStreamReader[T]` / `PipeStreamWriter[T]` 模拟 Eino 的流式读写抽象，提供：

- `NewPipe(cap)`：创建带缓冲的 goroutine channel 流
- `PipeStreamReaderFromSlice` / `PipeStreamReaderFromValue`：从已有数据构造
- `Copy(parent, n)`：教学版扇出——先 drain 全部元素，再复制 N 份
- `Merge(readers...)`：教学版扇入——goroutine 并发消费并写入合并流
- `Concat(readers...)`：教学版流折叠——支持通过 `RegisterConcatFunc` 注册自定义拼接逻辑
- `drainAll`：消费全部元素，用于将流转换为切片

**与 Eino 的关键差异**：Eino 的 `Copy` 使用链表共享缓冲区（惰性消费），复刻版是先 drain 再复制（急切消费）。这避免了链表共享的复杂性，但失去了惰性消费的内存效率。

#### 4.3 CallbackWrapper (`compose/callbacks.go`)

复刻版实现了完整的教学版回调引擎：

- **RunInfo** 结构体（Name / Type / Component）——提供稳定的执行点标识
- **五个回调阶段**：OnStart / OnEnd / OnError / OnStartWithStreamInput / OnEndWithStreamOutput
- **HandlerBuilder**：声明式构建处理器，自动计算 needs timing
- **TimingChecker**：跳过不需要的阶段，避免不必要的流复制
- **CallbackWrapper**：四种模式特定包装器（Invoke / Stream / Collect / Transform）
- **上下文隔离**：每个处理器的 OnStart → OnEnd 链是独立的（独立 `handlerCtxs` 切片）
- **CbStreamReader**：流回调副本机制——`Copy(n)` 为每个处理器创建独立副本

#### 4.4 EventLog — 复刻版独有的被动观测机制

除了 CallbackWrapper 的拦截式回调，复刻版还有一个独立的 EventLog 系统，提供 10 种事件类型的被动记录（node_start / node_end / node_error / node_skipped / graph_start / graph_end / graph_error 等）。与 Eino 回调引擎的区别在于：
- EventLog 是**被动记录**（记录发生了什么）而非**拦截式回调**（在发生前后插入逻辑）
- EventLog 不修改 context，不修改输入/输出
- EventLog 通过 `sync.Mutex` 保证线程安全

---

### 5. 我们做的学习性取舍

| 取舍 | Eino 做法 | 复刻版做法 | 理由 |
|------|----------|-----------|------|
| **流扇出语义** | 链表共享缓冲区（惰性 Copy） | 先 drain 再复制（急切 Copy） | 避免链表所有权的教学负担；演示概念而非追求生产性能 |
| **类型擦除** | 泛型 `runnablePacker[T, TOption]` | `any` 类型断言 | 降低类型参数复杂度，聚焦降级逻辑本身 |
| **Component Bridge** | 完整的 `toChatModelNode` / `toRetrieverNode` + `executorMeta` | Lambda 抽象 + 手动类型断言 | 减少概念跳跃，让读者先理解 Runnable 后再扩展到组件桥接 |
| **全局处理器 `AppendGlobalHandlers`** | 支持 | 不支持 | 教学子集不需要全局横切关注点 |
| **路径范围限定回调** | `WithCallbacks(WithNodePath(...))` | 不支持 | 教学子集用不同节点独立注册 handler 覆盖了等价场景 |
| **`isComponentCallbackEnabled` 防重复触发** | 通过 `Checker` 接口防止 | 不实现 | 无组件桥接层时不会出现回调重复 |
| **schema.StreamReader 接口族** | 完整 schema 包 | PipeStreamReader 简化版 | 仅覆盖教学所需的最小流抽象 |
| **`concatStreamReader` + `RegisterStreamChunkConcatFunc`** | 复杂注册机制 | `Concat` + `RegisterConcatFunc` 简化版 | 流式拼接的核心语义保留，去除类型注册的复杂性 |

---

## 第四章：Checkpoint / Interrupt / Resume — 如何把运行过程变成可恢复过程

### 1. 面临的问题

Eino 的执行图可以任意嵌套深度：Graph 嵌套子图，Agent 包裹 Graph，ToolsNode 扇出多个并行工具调用，Lambda 可能启动完整独立 Runnable。当这个深度嵌套网络中的任意组件决定暂停——因为需要人工输入、触发速率限制或工具调用需要审批——运行时必须：

- **保存精确的执行状态**，以便图可以从精确的中断点重新启动，而非从头开始
- **在整个调用树中唯一标识每一个中断点**，即使同一个工具以不同的调用 ID 被调用了两次（`tool:my_tool:call_1` vs `tool:my_tool:call_2`）
- **防止父图吞掉子中断** —— 子图的中断必须向上传播到根调用方
- **在检查点保存之前物化流数据**，以便恢复时具有确定性的、非临时的输入
- **支持定向恢复** —— 用户可以选择仅恢复若干并行中断点中的一个

临时方案（保存调用栈、从顶部重新运行）之所以失败：组件调用具有副作用（LLM API 调用、数据库写入），重新运行会重复执行已完成的工作，且如果没有相同的检查点状态，无法保证走同一条执行路径。

---

### 2. 为什么困难

难点不在于保存状态——而在于在**分层、并发、异构**的运行时中，以**正确的身份**在**正确的时机**保存**正确的**状态。

#### 2.1 分层身份

执行点不是扁平的函数调用，它们形成一棵树：

```
runnable:root;node:sub_graph_a;node:tools;tool:interrupt_tool:tool_call_123
```

如果没有稳定的、分层式的地址系统，运行时无法：
- 区分在同一个 ToolsNode 上并行运行的工具调用 #1 和工具调用 #2
- 当用户说"继续工具调用 #3"时，将恢复数据路由到正确的叶子节点
- 让包裹了独立 Graph 的 Lambda 节点将其自身的地址段正确前置

#### 2.2 并发性

多个节点可以同时运行。当 ToolsNode 并行运行 3 个工具且其中 2 个中断时，检查点必须记录 1 个已完成工具的输出、2 个暂停工具的中断信号、以及子图节点的子图检查点。

#### 2.3 流物化

StreamReader 是一个临时的一次性消费者——一旦消费就消失了。在创建检查点之前，通道和输入中的所有流值必须物化为具体值。恢复时，这些具体值必须重新包装为 StreamReader 实例。

#### 2.4 复合组件的双重性

包裹子 Graph 的 ToolsNode 或 Lambda 必须同时扮演两种角色：
1. **自指目标**：复合节点本身可能是恢复目标
2. **管道**：如果恢复目标是后代节点，复合节点必须重新执行其子节点以让恢复上下文向下流动

这种双重性编码在 `isResumeTarget` 中：`true` + `hasData = false` 表示"某后代是目标，向下传播"；`true` + `hasData = true` 表示"你本人是直接目标"。

---

### 3. Eino 的设计思路

Eino 的设计建立在四个支柱之上：

#### 3.1 地址系统 — 结构化的执行点身份

每个执行上下文携带一个分层 `Address`（`[]AddressSegment`），存储在 Go context 中。每个 `AddressSegment` 包含：

```go
type AddressSegment struct {
    ID    string              // 节点键、工具名、runnable 名称
    Type  AddressSegmentType  // "node"、"tool"、"runnable"
    SubID string              // 工具调用 ID，用于消歧义
}
```

三种段类型：`AddressSegmentNode`（图节点）、`AddressSegmentTool`（工具调用）、`AddressSegmentRunnable`（独立 Graph/Workflow/Chain）。

一个关键设计选择：`isResumeTarget` 不仅在地址**精确**匹配恢复目标时设为 `true`，也会在存在一个恢复目标是当前地址的**后代**时设为 `true`。这正是让复合组件能够充当管道的机制。

#### 3.2 InterruptSignal 树

中断机制的核心是 `InterruptSignal` 树：

```go
type InterruptSignal struct {
    ID             string
    Address        Address
    InterruptInfo  InterruptInfo
    InterruptState InterruptState
    Subs           []*InterruptSignal   // 子信号（复合/虚拟节点）
}
```

关键转换：
- `SignalToPersistenceMaps`：将树扁平化为 `id2addr`、`id2state` 两个映射
- `ToInterruptContexts`：将树转换为面向用户的扁平 `InterruptCtx` 对象列表（仅根因）
- `FromInterruptContexts`：从扁平列表重建树

#### 3.3 Checkpoint 持久化

`checkpoint` 结构体捕获完整执行快照：Channels（在途值）、Inputs（待处理任务）、State（图级状态）、SubGraphs（嵌套子图）、InterruptID2Addr、InterruptID2State。嵌套子图通过 `SubGraphs[nodeKey]` 存储，恢复时通过 `forwardCheckPoint` 取出并注入子图上下文。

流转换通过 `convertCheckPoint` / `restoreCheckPoint` 处理流与非流之间的双向转换。

#### 3.4 恢复上下文注入 — 三个阶段

1. **用户提供恢复目标**：`Resume(ctx, id)` / `ResumeWithData(ctx, id, data)`
2. **检查点恢复中断状态**：`setCheckPointToCtx` 调用 `PopulateInterruptState` 合并检查点的中断映射
3. **地址匹配分发状态**：`AppendAddressSegment` 将新地址与全局恢复信息进行匹配

组件通过两个公开 API 读取状态：
- `GetInterruptState[T](ctx)` — "我之前被中断过吗？这是我保存的状态。"
- `GetResumeContext[T](ctx)` — "我是恢复目标吗？这是恢复数据。"

**关键区分**：`GetInterruptState` 在**任何**恢复运行中返回 true（无论该组件是否是本次的恢复目标），而 `GetResumeContext` 仅在**该特定地址被显式定向**时才返回 true。一个被中断过但不是当前恢复目标的组件**必须重新中断**。

---

### 4. 复刻版如何落地

#### 4.1 结构化执行地址 (`compose/address.go`)

```go
type AddressSegment struct {
    ID    string
    Type  AddressSegmentType  // "node" | "tool" | "runnable"
    SubID string               // 消歧义，如 "call_1"
}
type Address []AddressSegment
```

- `Address.String()` 产生稳定表示：`runnable:root;node:tools;tool:lookup:call_1`
- `AppendAddressSegment(ctx, typ, id, opts...)` 在进入新执行 scope 时扩展地址，并通过 `globalResumeInfo` 匹配注入的恢复状态
- `equal()` / `hasPrefix()` 用于精确匹配和前缀匹配

#### 4.2 地址匹配与恢复路由 (`compose/address.go:159-210`)

`AppendAddressSegment` 在每次进入新作用域时执行：
1. 扩展父地址构建新地址
2. 遍历 `globalResumeInfo` 中的所有中断 ID
3. 如果地址**精确匹配**：注入 `interruptState`，如果用户提供了恢复数据则注入 `resumeData`，设置 `isResumeTarget = true, hasResumeData = true`
4. 如果地址是恢复目标的**前缀**且恢复目标在更深层：设置 `isResumeTarget = true, hasResumeData = false`——这是"conduit"模式

#### 4.3 中断信号树 (`compose/interrupt.go`)

- `Interrupt(ctx, info)`：简单中断
- `StatefulInterrupt(ctx, info, state)`：带状态中断
- `CompositeInterrupt(ctx, info, state, errs...)`：复合中断——从子错误中提取信号树，挂到当前作用域下
- `InterruptError` 包装 `InterruptInfo{Sig nal, InterruptContexts}`
- `ToInterruptContexts`：将树扁平化为仅根因的列表，保留 Parent 指针
- `SignalToPersistenceMaps`：将信号树转为 `interruptID → Address` 和 `interruptID → InterruptState` 两组映射

#### 4.4 CheckPoint 存储 (`compose/checkpoint.go`)

```go
type CheckPoint struct {
    Input                 any
    InterruptID2Addr      map[string]Address
    InterruptID2State     map[string]InterruptState
    LayerSpecificSnapshot map[string]any
}
```

- `CheckPointStore` 接口：`Get(ctx, id)` / `Set(ctx, id, cp)`
- `InMemoryCheckPointStore`：教学用确定性内存存储（带 `sync.Mutex` 线程安全）
- `WithCheckPoint(ctx, id, store)` / `WithCheckPointStore` / `WithCheckPointID`：注入到 context
- `restoreCheckPointContext`：恢复时从 store 加载检查点并合并到全局恢复信息
- `saveInterruptCheckPoint`：中断时保存当前输入和中断信号映射

#### 4.5 恢复 API (`compose/resume.go`)

- `ResumeWithData(ctx, interruptID, data)`：将恢复数据定向到指定中断 ID
- `BatchResumeWithData(ctx, map[interruptID]data)`：批量恢复
- `GetInterruptState[T](ctx)`：返回 `(wasInterrupted, hasState, typedState)`
- `GetResumeContext[T](ctx)`：返回 `(isResumeTarget, hasData, typedData)`——区分直接目标和 conduit

#### 4.6 Runner 集成 (`compose/graph_run.go`)

- `runner.run()` 入口自动追加 `runnable:<graphName>` 地址段
- `createTasks()` 为每个任务自动追加 `node:<nodeKey>` 地址段
- `resolveCompletedTasks()` 在检测到中断错误时调用 `saveInterruptCheckPoint`
- CheckPoint 保存后，`InterruptError` 原样向上传播，由调用方提取 `InterruptContexts` 并决定恢复策略

#### 4.7 Stream 物化示例 (`compose/checkpoint.go:131-150`)

```go
MaterializeStream[T](PipeStreamReader[T]) → MaterializedStream[T]{Items: []T}
RestoreStream[T](MaterializedStream[T]) → PipeStreamReader[T]
```

演示如何在 checkpoint 边界将一次性 `PipeStreamReader` 物化为可持久化值，并在恢复时重新包装。

---

### 5. 我们做的学习性取舍

| 取舍 | Eino 做法 | 复刻版做法 | 理由 |
|------|----------|-----------|------|
| **检查点内容** | 完整 channel manager 状态（在途通道值、待处理任务） | 仅保存原始输入 + 中断 ID/地址/状态映射 | 教学子集不需要恢复中间执行状态，只需定向恢复 |
| **嵌套子图** | `SubGraphs[nodeKey]` + `forwardCheckPoint` 递归转发 | 不支持 | 需要在图编译时建立父子 runner 关系，超出了当前 runner 的简化模型 |
| **序列化类型注册** | `schema.RegisterName[T]` / `schema.Register[T]()` | 不支持 | 无持久化需求，context 内传引用足够 |
| **检查点迁移** | `MigrateCheckpointState` 递归状态转换 | 不支持 | 教学子集不涉及跨版本升级 |
| **分布式 CheckPointStore** | 支持任意后端 | 仅 InMemoryCheckPointStore | 内存存储足够证明概念 |
| **ToolsNode rerun skip handler** | 恢复时跟踪已执行工具，跳过重执行 | 不支持 | 需要更精细的工具调用 ID 追踪和中断恢复状态机 |
| **完整的 interrupt/rerun 遗留兼容** | `WrapInterruptAndRerunIfNeeded` 兼容老 API | 不支持 | 没有遗留 API 需要兼容 |

---

### 6. 两个章节的协同关系

第三章和第四章并非独立的两套机制，它们在同一运行时中紧密协作：

1. **流 + 检查点 = 物化边界**。Stream 模式的临时数据（`PipeStreamReader`）在检查点创建前必须物化（`MaterializeStream`），在恢复时重新挂载（`RestoreStream`）。这是 Eino 中 `convertCheckPoint` / `restoreCheckPoint` 在复刻版中的教学简化版。

2. **回调 + 中断 = 可观测的暂停点**。CallbackWrapper 的 OnStart / OnEnd / OnError 阶段使得运行时可以在中断发生时记录"哪个节点在做什么事"，EventLog 记录了中断的完整上下文。

3. **Runnable 四模式 + 恢复 = 模式感知的重入**。当一个组件通过 `GetInterruptState` 发现自己曾被中断时，它需要知道自己之前是通过哪种模式（Invoke / Stream / Collect / Transform）被调用的，以便在恢复时以相同模式继续执行。`composableRunnable` 的四字段设计（i / s / c / t）为此提供了基础。

4. **Conduit 模式 + Callback 上下文隔离**：复合组件的 `isResumeTarget = true, hasData = false` 的 conduit 模式与回调的处理器隔离上下文一脉相承——两者都是 Eino 对"谁是责任主体"的清晰界定。回调引擎说"每个处理器独立"，恢复引擎说"只有精确匹配的地址才是数据目标，祖先只是管道"。

---

### 7. Rive 可以借鉴的地方

#### 7.1 执行点身份应当是结构性的，而非描述性的

Eino 的地址是从图拓扑构建的类型化、带 ID 段组成的确定性链。对于 Rive 的 dispatch/resume 模型，这意味着恢复点应由 `run_id + node_id + dispatch_id + subdispatch_id` 来标识，而非"卡在 token 限制上的那个 worker"。

#### 7.2 复合 Dispatch 即管道模式

`isResumeTarget` 带 `hasData = false` 用于后代节点，对复合组件来说是一种干净的模式。Rive 中扇出子 dispatch 同样需要双重性：dispatch 本身可以是恢复目标，同时必须透明地将恢复信号向下转发。

#### 7.3 CallbackWrapper 的拦截式观测 vs EventLog 的被动记录

两种观测模式各有价值：CallbackWrapper 可以在事件发生前后插入逻辑（修改输入、拦截输出），EventLog 只记录而不介入。Rive 可以同时提供两种级别的可观测性。

#### 7.4 面向用户的扁平上下文与树状状态

`ToInterruptContexts` 产生扁平的根因列表（用户关心的叶子），而 `InterruptSignal.Subs` 保留树结构以便正确的状态持久化和重建。Rive 同样应向人类暴露扁平的视图，同时在内部保留父子树。

#### 7.5 流 / 状态双重性

任何临时的、一次性数据（流、连接池、在途 RPC）必须在创建检查点之前物化，在恢复时重新挂载。涉及流式传输的 Rive dispatch 应识别类似的临时状态。

#### 7.6 自动能力降级作为迁移策略

Runnable 的降级矩阵允许组件独立演化——一个组件可以添加 `Stream()` 支持而无需更新每一个调用方。Rive 的 worker 同样可以声明能力级别（`supports_streaming`、`supports_resume`），协议可以自动桥接不同的能力级别。
