# Chapter 03 — Runnable / Stream / Callback 运行时模式教学细纲（Reviewed）

---

## 1. 校验结论：needs_attention

### 通过项
- 覆盖了全部 5 个学习目标，回答问题清晰。
- 四层抽象（Runnable 接口 → 降级矩阵 → Stream 原语 → Callback → EventLog）讲解顺序合理，由问题到抽象再到实现。
- 码走读路线图完整，行号引用经逐行比对，**95% 以上准确**。
- 容易误解点 6 条中的 4 条有教学价值（Copy 后 parent 已消费、context 隔离、Merge 不保序、Concat 不等于列数组）。
- 练习题覆盖基础、进阶、设计三层，数量和质量都合格。

### 需修正项（已纳入修订版）

| # | 问题 | 严重度 | 说明 |
|---|---|---|---|
| **A** | 代码片段 `l.GetRunnable().stream(ctx, "world")` 无效 | 高 | `composableRunnable.stream` 是小写非导出方法，仅 `compose` 包内可用。课堂演示代码若在外部包写会编译失败。 |
| **B** | `collectByStream` 的 concat fallback 数据丢失警告不适用于复刻版 | 高 | 复刻版 `composableRunnable.collect()` 中 `collectByStream` 使用 `collected(items2)` 返回完整数组，不调用 concat 函数。concat fallback 是 Eino 原版行为。 |
| **C** | 遗漏 `compose/concat.go`（202 行） | 中 | `compose/concat.go` 包含 `concatFuncRegistry`、`RegisterStreamChunkConcatFunc`、`ConcatItems`、`ConcatMessages`、`ConcatToolResults`。`Concat` 在 `stream.go:179` 也检查此 registry，但 writer 只提了 `concatFns`。 |
| **D** | `RegisterConcatFunc` vs `RegisterStreamChunkConcatFunc` 未区分 | 中 | 前者在 `stream.go:155` 写入 `concatFns`（`func([]T) T`），后者在 `concat.go:22` 写入 `concatFuncRegistry`（`func([]T) (T, error)`）。二者签名不同、用途不同，且 `Concat` 函数双检。 |
| **E** | writer 示例代码中 `l.GetRunnable().stream()` 用于演示"自动降级"混淆了包内外边界 | 低 | 建议改为展示 `graphRunnable.Stream()` 的包外用法，或明确标注"仅 compose 包内可用的内部路径"。 |
| **F** | EventLog 时间线缺少 `checkpoint` 事件类型说明 | 低 | `event_log.go` 定义 `EventCheckpoint` 但 writer 的 EventLog 对比表中未提及，可能误导读者以为 EventLog 不包括恢复事件。 |

---

## 2. 修订版教学细纲

以下是对 writer 细纲的修订版，保留其结构，修正上述问题。

---

### 1. 本章讲解目标

学完本章，听众应能用自己的语言解释以下问题：

1. **Runnable 四种执行模式分别解决什么输入/输出组合？** Invoke、Stream、Collect、Transform 各自对应什么调用场景？
2. **为什么组件只实现 1~2 种模式就够了，但调用方可以任意选模式？** 自动降级矩阵怎么工作，优先级是什么？
3. **流（`StreamReader` / `PipeStreamReader`）的生命周期管理为什么重要？** Copy / Merge / Concat 什么时候用，什么时候必须 Close？
4. **Callback 如何在不污染业务代码的前提下实现可观测性？** 五种时序各自的触发时机、上下文隔离规则、流副本机制。
5. **EventLog 与 Callback 的职责如何分工？** 二者都记录执行过程，但面向的消费者不同。

**前置知识：**

- 了解 Eino 的 `Graph -> Compile -> Runnable` 编译边界（Chapter 01）
- 理解 Go 的 `context.Context`、channel、goroutine
- 知道什么是 LLM 的流式输出（token-by-token）

---

### 2. 问题背景：LLM 应用中的三种数据流模式

考虑典型 RAG 链：`Retriever → ChatModel → FormatOutput`

三种问题：

**问题 1：能力不对称。** Retriever 只有 `Retrieve(query) → []Document`（非流式），ChatModel 同时有 `Generate`（非流式）和 `Stream`（流式）。用户希望整个链 `Invoke("query")` 得到完整文本可以；但希望 `Stream("query")` 得到逐 token 输出时，每个节点都必须能处理流——Retriever 不支持。

**问题 2：上下游模式不匹配。** 上游 ChatModel A 输出流，下游 ChatModel B 需要完整输入。谁来"把流折叠成单值"？调用方不应感知这些转换。

**问题 3：可观测性需要侵入式代码。** 想记录"节点输入了什么，输出了什么，耗时多少"，但不能在每个组件里加 `log.Printf`。而且流式 token 记录需要消耗流数据——日志处理器读完流，消费者还读什么？

| 问题 | 如果不管 | 实际后果 |
|---|---|---|
| 能力不对称 | 调用方必须 if-else 判断每个节点支持什么模式 | 图编译失败、运行时 panic |
| 模式不匹配 | 需要手动写 convert 函数 | 类型系统崩溃 |
| 观测性侵入 | 日志散落各组件 | 换组件重写一遍；流数据被日志消费后下游读不到 |
| 流生命周期管理 | 谁都不 Close stream | goroutine 泄漏，服务内存增长 |

---

### 3. 解决思路：四层抽象

#### 3.1 第一层：Runnable 四模式统一接口

**问题**：组件能力不同，调用方需要一种方式表达"我要哪种模式"。

**Eino 的答案**：定义四模式接口。所有图节点编译后都实现它。

```go
// compose/runnable.go:14-20
type Runnable[I, O any] interface {
    Invoke(ctx context.Context, input I) (output O, err error)
    Stream(ctx context.Context, input I) (output StreamReader[O], err error)
    Collect(ctx context.Context, input StreamReader[I]) (output O, err error)
    Transform(ctx context.Context, input StreamReader[I]) (output StreamReader[O], err error)
}
```

**注意**：复刻版中，`Runnable` 接口由 `graphRunnable[I,O]`（`compose/generic_graph.go:120`）实现。内部的 `composableRunnable` 使用小写方法 `invoke/stream/collect/transform` 供包内使用。用户代码通过 `Graph.Compile() → Runnable` 或 `Lambda.GetRunnable()`（包内）获得执行能力。

```
                 输入是单值(I)        输入是流(StreamReader[I])
输出是单值(O)   Invoke               Collect
输出是流(...O)  Stream               Transform
```

#### 3.2 第二层：4×4 降级矩阵

**问题**：组件可能只实现了 {Invoke} 或 {Invoke, Stream}，但调用方可能调用任意模式。

**Eino 的答案**：`composableRunnable` 存储 4 个 `func(any) → any` 指针（`i, s, c, t`），nil 表示未实现。调用任一方法时按优先级寻找降级路径。

**复刻版降级矩阵（已验证）**：

| 目标模式 | 优先级 1 | 优先级 2 | 优先级 3 | 优先级 4 |
|---|---|---|---|---|
| **Invoke** | 原生 i | invokeByStream: s → recvAll → collected | invokeByCollect: streamFromItems(input) → c | invokeByTransform: streamFromItems(input) → t → recvAll → collected |
| **Stream** | 原生 s | streamByTransform: streamFromItems(input) → t | streamByInvoke: i → streamFromItems | streamByCollect: streamFromItems(input) → c → streamFromItems |
| **Collect** | 原生 c | collectByTransform: t → recvAll → collected | collectByInvoke: recvAll → collected → i | collectByStream: recvAll → collected → s → recvAll → collected |
| **Transform** | 原生 t | transformByStream: recvAll → 逐元素 s → 合并流 | transformByCollect: c → streamFromItems | transformByInvoke: recvAll → collected → i → streamFromItems |

**为什么优先级重要？** 以 Stream 为例：`streamByTransform` 是"流转流"，低延迟；`streamByInvoke` 先变单值再包成流，丢失流式语义；`streamByCollect` 消费全量输入后才返回流，延迟最高。

**课堂提问**："如果只实现了 Transform，调用 Stream 会走哪条路径？为什么不是 Invoke？"（答案：`streamByTransform`，因为 T 是"流转流"，最低延迟。）

#### 3.3 第三层：Stream 原语

**问题**：降级矩阵中频繁出现"流→数组→流"转换，需要一个轻量级流抽象。

**复刻版的两种流抽象**：

| 抽象 | 用途 | 来源 |
|---|---|---|
| `streamReader`（内部接口） | 降级矩阵中的流消费，`Recv() (any, error)` | `compose/runnable.go:22-25` |
| `PipeStreamReader[T]` | goroutine channel 流，`Recv() (T, bool)`，支持 Close | `compose/stream.go:11-14` |
| `CbStreamReader` | 回调专用内存流，支持 Copy / Next / All | `compose/callbacks.go:43-89` |

**核心原语**：

| 原语 | 位置 | 用途 |
|---|---|---|
| `NewPipe[T]` | `stream.go:87-90` | 创建 goroutine channel 流（带关闭信号） |
| `PipeStreamReaderFromSlice` | `stream.go:92-98` | 切片→已关闭流 |
| `drainAll` | `stream.go:105-115` | 消费完整流到切片 |
| `Copy[T]` | `stream.go:117-126` | **先 drain 再切片拷贝**（eager copy） |
| `Merge[T]` | `stream.go:128-151` | 多 goroutine 并发扇入（顺序不确定） |
| `Concat[T]` | `stream.go:160-194` | 顺序拼接所有 reader 后调用 concat 函数 |
| `RegisterConcatFunc[T]` | `stream.go:155-158` | 注册 `func([]T) T` 型 concat（写入 `concatFns`） |
| `RegisterStreamChunkConcatFunc[T]` | `concat.go:22-24` | 注册 `func([]T) (T, error)` 型 concat（写入 `concatFuncRegistry`） |
| `ConcatItems[T]` | `concat.go:28-50` | 对类型注册表分发 concat |
| `ConcatMessages` | `concat.go:61-105` | Message 的流式拼接（content 累加、toolcalls 按 Index 合并） |

**关键发现**：`Concat` 函数双检两个注册表——先测 `concatFns`（`RegisterConcatFunc`），再测 `concatFuncRegistry`（`RegisterStreamChunkConcatFunc`）。课堂应展示 `compose/concat.go` 作为流拼接的子模块，说明为什么需要带 error 返回的 concat 函数（如 ToolCall ID 不匹配需要报错）。

#### 3.4 第四层：Callback 被动观测

**问题**：观测逻辑需要覆盖四种模式的所有生命周期事件，但不污染业务代码。

**Eino 的答案**：定义五个时序的 `Handler` 接口，通过 `CallbackWrapper` 以装饰器模式包裹原始函数。

```
Handler {
    OnStart                 (ctx, info, input) → ctx     // 非流式输入就绪
    OnEnd                   (ctx, info, output) → ctx    // 非流式输出就绪
    OnError                 (ctx, info, err) → ctx       // 报错
    OnStartWithStreamInput  (ctx, info, input)  → ctx   // 流式输入就绪（接收副本）
    OnEndWithStreamOutput   (ctx, info, output) → ctx   // 流式输出就绪（接收副本）
}
```

**核心设计原则**：单个处理器的 context 链隔离（`callbacks.go:97-109`）。每个 handler 在 `Invoke` 中独立存储 `handlerCtxs[idx]`（`callbacks.go:183-191`），处理器 A 的 OnStart 返回的 ctx 只传给 A 的 OnEnd/OnError，不会传给处理器 B。

**流副本机制**：当需要流阶段时，`dispatchOnStartWithStreamInput`（`callbacks.go:308-332`）会调用 `input.Copy(n)` 创建 n 份副本（n = 需要此阶段的处理器数），每个处理器收到独立副本。由于复刻版 Copy 是"先 drain 再切片拷贝"，副本完全独立。

**TimingChecker 优化**：`CallbackWrapper.TimingChecker()` 汇总所有 handler 的 `neededTimings()` 位掩码。不需要流阶段的 handler 不会触发 Copy（零开销）。

#### 3.5 第五层：EventLog 结构化事件

**问题**：Callback 关注单个节点执行，但图运行时还需要全图级别的结构化事件日志。

| 维度 | Callback | EventLog |
|---|---|---|
| 抽象层级 | 单个组件/节点 | 全图/全局 |
| 触发时机 | 组件执行前后（微观） | 节点调度、graph_start/end、channel_ready、checkpoint（宏观） |
| 数据内容 | Input/Output 完整值 | 节点 key、step、graph name（结构化元数据） |
| 消费者 | 应用代码（metrics、tracing） | 运维工具（JSONL 文件、调试面板） |

`EventLog` 包含 10 种事件类型（`event_log.go:13-26`），支持多 `EventSink` 并行写入（`event_log.go:149-162`）。每个 sink 的错误记录在 `sinkErrors` 中，不阻断其他 sink。`JSONLEventSink` 支持实时写入 JSONL 文件（`event_log.go:56-98`）。

---

### 4. 最小实现路径（修订版）

#### Phase 1: Runnable 接口 + 降级矩阵（`compose/runnable.go`）

| 步骤 | 写什么 | 对应测试 |
|---|---|---|
| 1.1 | `Runnable[I,O]` 接口定义 | 编译通过即可 |
| 1.2 | `composableRunnable`（4 个 func(any)→(any,error)） | — |
| 1.3 | `recvAll` / `streamFromItems` / `collected` / 类型适配器 | — |
| 1.4 | `composableRunnable.invoke()` 降级 | `TestInvokeOnlyStreamFallback`（行 29） |
| 1.5 | `composableRunnable.stream()` 降级 | `TestStreamOnlyInvokeFallbackWithConcat`（行 58） |
| 1.6 | `composableRunnable.collect()` 降级 | `TestCollectableLambdaNative`（行 512） |
| 1.7 | `composableRunnable.transform()` 降级 | `TestTransformFallbackToInvoke`（行 82） |
| 1.8 | Lambda 四种工厂：`InvokableLambda` / `StreamableLambda` / `CollectableLambda` / `TransformableLambda` | 优先级系列测试 |
| 1.9 | 验证所有降级优先级 | `TestInvokeFallbackPriority` / `TestStreamFallbackPriority` / `TestCollectFallbackPriority` / `TestTransformFallbackPriority` |
| 1.10 | 验证 `graphRunnable` 也支持降级 | `TestGraphRunnableStreamFallback`（行 538） |

#### Phase 2: Stream 原语（`compose/stream.go` + `compose/concat.go`）

| 步骤 | 写什么 | 对应测试 |
|---|---|---|
| 2.1 | `NewPipe`（goroutine channel 流） | `TestPipeSendRecv`（行 9） |
| 2.2 | `PipeStreamReaderFromSlice` / `PipeStreamReaderFromValue` | `TestPipeStreamReaderFromSlice`（行 93）/ `TestPipeStreamReaderFromValue`（行 119） |
| 2.3 | `drainAll` | `TestDrainAll`（行 210） |
| 2.4 | `Copy(parent, n)`（eager copy） | `TestCopySameData`（行 150）/ `TestCopyIndependentChildren`（行 173） |
| 2.5 | `Merge`（多 goroutine 并发扇入） | `TestMerge`（行 219） |
| 2.6 | `Concat` + `RegisterConcatFunc` | `TestConcatFallbackLastChunk`（行 282）/ `TestConcatRegisteredFunction`（行 300） |
| 2.7 | `RegisterStreamChunkConcatFunc` + `ConcatItems` | 展示 `concat.go` 中的双注册表机制和 `ConcatMessages` 作为示例 |

**新增 2.7**：`compose/concat.go` 是流拼接的子模块，定义 `RegisterStreamChunkConcatFunc`（带 error 的 concat）和 `ConcatMessages`（消息级拼接）。`Concat` 函数双检两个注册表——课堂上展示为什么需要两个注册表（简单类型如 string 用 `RegisterConcatFunc`，复杂类型如 Message/ToolCall 用 `RegisterStreamChunkConcatFunc`）。

#### Phase 3: Callback 观测（`compose/callbacks.go`）

| 步骤 | 写什么 | 对应测试 |
|---|---|---|
| 3.1 | `RunInfo` / `CallbackTiming` / `TimingChecker` | `TestRunInfo`（行 10）/ `TestCallbackTimingString`（行 27） |
| 3.2 | `Handler` + `neededTimings()` | `TestHandlerNeededTimingsEmpty`（行 48）/ `TestHandlerNeededTimingsFull`（行 55） |
| 3.3 | `HandlerBuilder` + `TimingChecker` | `TestHandlerBuilderTimingCheckerPartial`（行 87） |
| 3.4 | `CbStreamReader` + `Copy` / `Next` / `All` | `TestCbStreamReaderCopy`（行 239）/ `TestCbStreamReaderNextAndAll`（行 931） |
| 3.5 | `CallbackWrapper.Invoke()` | `TestCallbackWrapperInvokeSuccess`（行 136）/ `TestCallbackWrapperInvokeError`（行 187） |
| 3.6 | `CallbackWrapper.Stream()` + 流副本分发 | `TestCallbackWrapperStreamOnEndWithStreamOutput`（行 321） |
| 3.7 | `CallbackWrapper.Collect()` + 流副本分发 | `TestCallbackWrapperCollect`（行 379） |
| 3.8 | `CallbackWrapper.Transform()` + 双流副本 | `TestCallbackWrapperTransform`（行 479） |
| 3.9 | context 隔离 | `TestPerHandlerContextChainNotCrossHandlerGlobalOrdering`（行 588） |
| 3.10 | TimingChecker 跳过流复制 | `TestTimingCheckerSkipsStreamCopy`（行 549） |

#### Phase 4: EventLog（`compose/event_log.go`）

| 步骤 | 写什么 | 对应测试 |
|---|---|---|
| 4.1 | `EventType` / `Event` 枚举 | `TestEventLogLifecycle`（`graph_test.go:551`） |
| 4.2 | `EventLog` 创建（内存 + 多 sink） | `TestEventLogEmpty`（`graph_test.go:715`） |
| 4.3 | `JSONLEventSink` 实现 | `TestEventLogJSONLSinkWritesImmediately`（`graph_test.go:584`） |
| 4.4 | `LogNodeStart` / `LogNodeEnd` 等便利方法 | `TestEventLogAllEventTypes`（`graph_test.go:1888`） |
| 4.5 | 线程安全 | `TestEventLogThreadSafety`（`graph_test.go:695`） |
| 4.6 | 图级集成 | `TestEventLogIntegrationWithRunner`（`graph_test.go:2718`） |

---

### 5. 课堂讲解顺序（建议 30-35 分钟）

#### 第一部分：问题热身（0-5 分钟）

画出 RAG 链的三种数据流模式，问听众："如果 Retriever 只支持非流式，上游 ChatModel 输出流式结果，你怎么连？"

**可演示代码（修订版 — 展示图编译后用法）**：

```go
// 一个只实现了 Invoke 的 Lambda 编译为图节点
g := compose.NewGraph[string, string]()
g.AddLambdaNode("step1", compose.InvokableLambda(func(ctx context.Context, input string) (string, error) {
    return "hello " + input, nil
}))
g.AddEdge(compose.START, "step1")
g.AddEdge("step1", compose.END)
r, _ := g.Compile(ctx)  // r 是 Runnable[string,string]
// 用户调用 Stream —— 框架自动降级
sr, _ := r.Stream(ctx, "world")
```

#### 第二部分：Runnable 接口与降级矩阵（5-10 分钟）

1. 展示 `Runnable[I,O]` 接口定义（1 分钟）
2. 画出 4×4 降级表格，解释优先级原因（2 分钟）
3. 走读 `composableRunnable.invoke()`（`runnable.go:110-156`），关注 4 层 if-else 优先级（2 分钟）

**关键澄清**：讲解 `Invoke` → `Stream` 降级时，展示 `runnable.go:110-130`：
- 原生 i → `invokeByStream`: s → recvAll → collected
- 说明 `collected` 的三种行为：0 元素 → nil，1 元素 → 该值，>1 元素 → `[]any`

**提问**："如果只实现了 Transform，调用 Stream 走哪条路径？"（答案：`streamByTransform`）

#### 第三部分：Stream 原语（10-15 分钟）

1. `NewPipe`：goroutine + channel 实现（1 分钟）
2. `Copy`：展示 `Copy(parent, 3)` → 3 个独立 reader（2 分钟）
3. `Merge` vs `Concat` 语义对比（2 分钟）
4. **新增**：展示 `concat.go` 中的 `RegisterStreamChunkConcatFunc` 和 `ConcatMessages`（2 分钟）

**课堂演示**：`TestCopyIndependentChildren` — 关闭子 reader 0 不影响子 reader 1。

**新增观察点**：`Copy` 是 eager copy（先 drain 再切片拷贝），调用后 parent 已消费完毕。如果想保留数据，应 `Copy(parent, N+1)` 自己持有一份。

#### 第四部分：Callback 装饰器（15-20 分钟）

1. `Handler` 结构体五种时序（1 分钟）
2. `CallbackWrapper.Invoke()` 流程：OnStart → 执行 → OnEnd|OnError（2 分钟）
3. 流副本分发：`dispatchOnStartWithStreamInput` / `dispatchOnEndWithStreamOutput`（3 分钟）
4. TimingChecker 优化（1 分钟）

**提问**："处理器 B 在 OnStart 中设置 context key，处理器 A 的 OnEnd 能看到吗？"（不能，context 隔离）

#### 第五部分：EventLog（20-25 分钟）

与 Callback 对比，EventLog 记录全图级事件。展示 `TestEventLogLifecycle` 输出时间线。

**新增要点**：展示 `checkpoint` 事件类型作为恢复/中断的纽带（为 Chapter 04 铺垫）。

#### 第六部分：回顾与练习（25-35 分钟）

---

### 6. 代码走读脚本（修订版）

```
1. compose/runnable.go（385 行）
   ├── 14-20:   Runnable 接口定义
   ├── 22-25:   streamReader 内部接口（降级用）
   ├── 28-40:   internalStreamReader（内存 slice 流）
   ├── 43-53:   typedStreamWrapper（T → any 适配）
   ├── 55-72:   untypedStreamWrapper（any → T 适配）
   ├── 74-87:   recvAll（流→数组）
   ├── 89-91:   streamFromItems（数组→流）
   ├── 93-101:  collected（数组折叠为单值）
   ├── 103-108: composableRunnable 结构体
   ├── 110-156: composableRunnable.invoke()  ← 最核心降级
   ├── 158-184: composableRunnable.stream()
   ├── 186-240: composableRunnable.collect()  ← 注意 collectByStream 不使用 concat，只用 collected
   ├── 242-296: composableRunnable.transform()
   └── 311-385: Lambda 四种工厂 + GetRunnable

2. compose/stream.go（194 行）
   ├── 87-90:   NewPipe（goroutine channel 流）
   ├── 92-98:   PipeStreamReaderFromSlice（切片→已关闭流）
   ├── 105-115: drainAll（消费完整流）
   ├── 117-126: Copy（eager copy：先 drain 再切片拷贝）
   ├── 128-151: Merge（多 goroutine 并发扇入）
   ├── 155-158: RegisterConcatFunc（写入 concatFns）
   └── 160-194: Concat（双检 concatFns + concatFuncRegistry）

3. compose/concat.go（202 行）  ← 新增！
   ├── 13:      concatFuncRegistry（全局 concat 注册表）
   ├── 22-24:   RegisterStreamChunkConcatFunc（带 error 的 concat 注册）
   ├── 28-50:   ConcatItems（类型分发 concat）
   └── 61-105:  ConcatMessages（消息流拼接示例）

4. compose/callbacks.go（383 行）
   ├── 8-12:    RunInfo 结构体
   ├── 14-22:   CallbackTiming（五位掩码）
   ├── 41:      TimingChecker 类型
   ├── 43-89:   CbStreamReader（回调专用流）
   ├── 97-109:  Handler 结构体（五种回调函数指针）
   ├── 111-129: Handler.neededTimings()
   ├── 131-156: HandlerBuilder
   ├── 158-168: CallbackWrapper 结构体
   ├── 180-210: CallbackWrapper.Invoke()  ← 核心装饰器
   ├── 212-243: CallbackWrapper.Stream()
   ├── 245-274: CallbackWrapper.Collect()
   ├── 276-306: CallbackWrapper.Transform()
   ├── 308-332: dispatchOnStartWithStreamInput()  ← 流副本分发
   └── 334-352: dispatchOnEndWithStreamOutput()

5. compose/event_log.go（229 行）
   ├── 13-26:   EventType（10 种枚举，含 EventCheckpoint）
   ├── 28-37:   Event 结构体
   ├── 40-42:   EventSink 接口
   ├── 49-98:   JSONLEventSink 实现
   ├── 100-105: EventLog 结构体
   ├── 107-109: NewEventLog（接收可选 sinks）
   ├── 149-162: Log()（写入内存 + 所有 sinks）
   └── 164-194: LogNodeStart / LogNodeEnd / LogNodeError / LogGraphStart 等

6. compose/generic_graph.go  ← 新增关键入口
   ├── 74-114:  Graph.Compile() → 返回 graphRunnable[I,O]
   └── 120-140: graphRunnable.Invoke()（大写，公开接口）
```

---

### 7. 容易误解点（修订版）

#### 误解 1：Copy 后还能用原始 reader 吗？
**不能。** 复刻版 `Copy` 是 eager：先 `drainAll(parent)` → 切片拷贝 N 份。`Copy` 后 parent 已被消费完毕。参见 `stream.go:117-126`。

#### 误解 2：回调处理器的 context 是全局串联的
**不是。** 每个 handler 的 OnStart 收到的都是原始 `ctx`（调用方传入的 base ctx），不是上一个 handler 修改后的。参见 `callbacks.go:183-191` 和 `TestPerHandlerContextChainNotCrossHandlerGlobalOrdering`。

#### 误解 3：降级优先级只影响性能
**不只是性能。** `streamByInvoke`（先执行完 Invoke 再返回单元素流）完全失去流式低延迟特性。`streamByTransform` 是真正的"流转流"。语义差异不只是快慢问题。

#### 误解 4：复刻版 `collectByStream` 有 concat fallback 数据丢失风险
**没有。** 复刻版 `composableRunnable.collect()` 的 `collectByStream` 路径使用 `collected(items2)` 返回完整数组，不调用 concat 函数。这个风险属于 Eino 原版（原版通过 `concatStreamReader` 折叠流）。复刻版中，concat fallback 只影响 `compose/stream.go` 的 `Concat` 函数（直接用户调用）和 `ConcatItems`（`concat.go`）。

#### 误解 5：Merge 保证输入顺序
**不保证。** Merge 是并发扇入——每个 reader 在独立 goroutine 中读取，谁先到谁先写入。`stream.go:128-151`。

#### 误解 6：Concat 等价于列出所有数据
**不是。** Concat 调用用户注册的 concat 函数折叠为**单个值**。无注册函数时 fallback 为最后一块。`stream.go:160-194`。

#### 误解 7（新增）：`RegisterConcatFunc` 和 `RegisterStreamChunkConcatFunc` 是同一个东西
**不是。** 前者签名 `func([]T) T`，写入 `concatFns`；后者签名 `func([]T) (T, error)`，写入 `concatFuncRegistry`。`Concat` 函数双检两个表。简单类型用前者（如 string join），复杂类型用后者（如 ToolCall 需校验 ID 一致性）。

---

### 8. 练习题（修订版）

#### 基础题

**Q1**：一个组件只实现了 `Stream`，用户调用了 `Invoke`。画完整调用链。（答案：`invoke` → `invokeByStream`: s → recvAll → collected）

**Q2**：以下代码输出什么？
```go
sr, sw := compose.NewPipe[int](2)
sw.Send(1); sw.Send(2); sw.Close()
items := compose.drainAll(sr)
fmt.Println(len(items))  // 2
```

#### 进阶题

**Q3**：只实现 `Collect` 的节点 vs 只实现 `Transform` 的节点，上游输出流时，用户调用 `Invoke` 分别走哪条降级路径？
- 方法 A（只 Collect）: `invoke` → `invokeByCollect`: streamFromItems(input) → c
- 方法 B（只 Transform）: `invoke` → `invokeByTransform`: streamFromItems(input) → t → recvAll → collected
哪种更高效？（方法 A 更高效：1 次转换 vs 方法 B 的 2 次转换）

**Q4**：在 `TestPerHandlerContextChainNotCrossHandlerGlobalOrdering` 中，如何让 handler2 读取到 handler1 设置的值？（答案：通过共享安全变量，如 `sync.Map` 或闭包，而非 context）

**Q5**：以下回调处理器有什么问题？
```go
handler := &compose.Handler{
    OnEndWithStreamOutput: func(ctx context.Context, info *compose.RunInfo, output *compose.CbStreamReader) context.Context {
        first, _ := output.Next()
        fmt.Println("first token:", first)
        return ctx  // 没读完剩余，但复刻版 CbStreamReader 无 Close 语义，所以实际无泄漏。原版 Eino 需要 defer Close
    },
}
```
（复刻版：无泄漏，因为 `CbStreamReader` 是纯内存 slice，无底层 channel。原版：必须 `defer output.Close()`）

#### 设计题

**Q6**：如何实现 lazy copy？（共享底层 channel + 引用计数）新风险？（一个 reader 不 Close 导致所有 reader 泄漏）

**Q7**：`EventLog` 中一个 sink 报错，其他 sink 受影响吗？（不受影响，`Log()` 中每个 sink 独立处理，错误记录在 `sinkErrors`）

**Q8**（新增）：为什么 `Concat` 需要两个注册表（`concatFns` + `concatFuncRegistry`）？何时用哪个？（`concatFns` 用于简单无错拼接，`concatFuncRegistry` 用于复杂类型需要错误反馈的场景。如 ToolCall 合并时 ID 不匹配需要报错。）

---

### 9. 附录：代码索引（修订版）

#### A. 核心源文件

| 文件 | 核心内容 | 行数 | 教学要点 |
|---|---|---|---|
| `compose/runnable.go` | `Runnable[I,O]` + `composableRunnable` 降级 + `Lambda` 工厂 | 385 | 整个章节基础。理解降级矩阵 = 理解 Graph 无缝组合 |
| `compose/stream.go` | `PipeStreamReader`/`PipeStreamWriter` + Copy/Merge/Concat + `RegisterConcatFunc` | 194 | 流原语层。降级和回调的流操作依赖 |
| `compose/concat.go` | `concatFuncRegistry` + `RegisterStreamChunkConcatFunc` + `ConcatItems` + `ConcatMessages` | 202 | 流拼接子模块，带 error 的 concat 注册 |
| `compose/callbacks.go` | `RunInfo` / `Handler` / `CallbackWrapper` / `HandlerBuilder` | 383 | 观测层核心。装饰器模式典范 |
| `compose/event_log.go` | `Event` / `EventLog` / `EventSink` / `JSONLEventSink` | 229 | 全图级结构化日志 |
| `compose/generic_graph.go` | `graphRunnable[I,O]` 实现 `Runnable` 接口 | 215 | Runnable→composableRunnable 的桥接 |

#### B. 核心函数/类型

| 文件 | 类型/函数 | 行号 | 说明 |
|---|---|---|---|
| `compose/runnable.go` | `Runnable[I,O any]` | 14-20 | 四模式统一接口（公共 API） |
| `compose/runnable.go` | `streamReader` | 22-25 | 内部流接口（降级用） |
| `compose/runnable.go` | `composableRunnable` | 103-108 | 内部包装，持有 i/s/c/t |
| `compose/runnable.go` | `invoke()` | 110-156 | Invoke 降级：i → S → C → T |
| `compose/runnable.go` | `stream()` | 158-184 | Stream 降级：s → T → I → C |
| `compose/runnable.go` | `collect()` | 186-240 | Collect 降级：c → T → I → S |
| `compose/runnable.go` | `transform()` | 242-296 | Transform 降级：t → S → C → I |
| `compose/runnable.go` | `recvAll` | 74-87 | 流→[]any |
| `compose/runnable.go` | `collected` | 93-101 | 数组折叠：0→nil, 1→v, >1→[]any |
| `compose/runnable.go` | `InvokableLambda` | 311-323 | 只有 Invoke 的 Lambda |
| `compose/runnable.go` | `StreamableLambda` | 325-341 | 只有 Stream 的 Lambda |
| `compose/runnable.go` | `CollectableLambda` | 343-355 | 只有 Collect 的 Lambda |
| `compose/runnable.go` | `TransformableLambda` | 357-373 | 只有 Transform 的 Lambda |
| `compose/stream.go` | `NewPipe[T]` | 87-90 | goroutine channel 流 |
| `compose/stream.go` | `Copy[T]` | 117-126 | eager copy：drain + 切片拷贝 N 份 |
| `compose/stream.go` | `Merge[T]` | 128-151 | 并发扇入 |
| `compose/stream.go` | `Concat[T]` | 160-194 | 串联折叠（双检注册表） |
| `compose/stream.go` | `RegisterConcatFunc` | 155-158 | 注册 func([]T) T |
| `compose/concat.go` | `RegisterStreamChunkConcatFunc` | 22-24 | 注册 func([]T) (T, error) |
| `compose/concat.go` | `ConcatItems[T]` | 28-50 | 类型分发 concat |
| `compose/concat.go` | `ConcatMessages` | 61-105 | Message 流式拼接示例 |
| `compose/callbacks.go` | `Handler` | 97-109 | 五种回调函数指针聚合 |
| `compose/callbacks.go` | `CallbackWrapper.Invoke()` | 180-210 | 装饰器：OnStart→执行→OnEnd/OnError |
| `compose/callbacks.go` | `dispatchOnStartWithStreamInput` | 308-332 | 流副本分发（Copy → 逐个处理器） |
| `compose/callbacks.go` | `CbStreamReader` | 43-89 | 回调专用内存流 |
| `compose/event_log.go` | `EventLog` | 100-105 | 事件日志（内存 + sinks） |
| `compose/event_log.go` | `JSONLEventSink` | 49-98 | JSONL 文件输出 |
| `compose/generic_graph.go` | `graphRunnable.Invoke()` | 125-140 | 大写接口 → composableRunnable.invoke() 桥接 |

#### C. 测试文件索引

| 文件 | 测试函数 | 行号 | 验证内容 |
|---|---|---|---|
| `runnable_test.go` | `TestInvokeOnlyStreamFallback` | 29 | Invoke→Stream 降级 |
| `runnable_test.go` | `TestStreamOnlyInvokeFallbackWithConcat` | 58 | Stream→Invoke 降级 |
| `runnable_test.go` | `TestTransformFallbackToInvoke` | 82 | Transform→Invoke 降级 |
| `runnable_test.go` | `TestTransformFallbackToStream` | 108 | Transform→Stream 降级 |
| `runnable_test.go` | `TestAllFourModesNative` | 170 | 四模式全原生实现 |
| `runnable_test.go` | `TestUnsupportedModeError` | 291 | 空 composableRunnable 报错 |
| `runnable_test.go` | `TestInvokeFallbackPriority` | 316 | Invoke 降级优先级：S > C > T |
| `runnable_test.go` | `TestStreamFallbackPriority` | 362 | Stream 降级优先级：T > I > C |
| `runnable_test.go` | `TestCollectFallbackPriority` | 414 | Collect 降级优先级：T > I > S |
| `runnable_test.go` | `TestTransformFallbackPriority` | 456 | Transform 降级优先级：S > C > I |
| `runnable_test.go` | `TestGraphRunnableStreamFallback` | 538 | Graph 编译产物也支持降级 |
| `stream_test.go` | `TestPipeSendRecv` | 9 | 基础 Pipe |
| `stream_test.go` | `TestCopySameData` | 150 | Copy 后子 reader 数据一致 |
| `stream_test.go` | `TestCopyIndependentChildren` | 173 | 子 reader 独立 |
| `stream_test.go` | `TestMerge` | 219 | 并发扇入 |
| `stream_test.go` | `TestConcatRegisteredFunction` | 300 | 注册 concat 函数 |
| `stream_test.go` | `TestCopyParentAlreadyConsumed` | 451 | 已消费流 Copy 后为空 |
| `callbacks_test.go` | `TestCallbackWrapperInvokeSuccess` | 136 | Invoke OnStart→OnEnd |
| `callbacks_test.go` | `TestCallbackWrapperInvokeError` | 187 | Invoke OnStart→OnError |
| `callbacks_test.go` | `TestCallbackWrapperStreamOnEndWithStreamOutput` | 321 | Stream 流副本分发 |
| `callbacks_test.go` | `TestCallbackWrapperCollect` | 379 | Collect 流副本分发 |
| `callbacks_test.go` | `TestCallbackWrapperTransform` | 479 | Transform 双流副本 |
| `callbacks_test.go` | `TestPerHandlerContextChainNotCrossHandlerGlobalOrdering` | 588 | context 隔离 |
| `callbacks_test.go` | `TestTimingCheckerSkipsStreamCopy` | 549 | TimingChecker 跳过机制 |
| `graph_test.go` | `TestEventLogLifecycle` | 551 | EventLog 全生命周期 |
| `graph_test.go` | `TestEventLogJSONLSinkWritesImmediately` | 584 | JSONL 实时写入 |
| `graph_test.go` | `TestEventLogThreadSafety` | 695 | 并发安全 |
| `graph_test.go` | `TestEventLogIntegrationWithRunner` | 2718 | 图级集成 |

#### D. 关键设计决策

| 决策 | 复刻版做法 | 原版 Eino 做法 | 教学影响 |
|---|---|---|---|
| Stream Copy 语义 | eager copy（先 drain 再切片拷贝） | lazy copy（parentStreamReader + childStreamReader） | 复刻版简单但无法处理无限流 |
| collectByStream | 用 `collected` 返回完整数组 | 用 `concatStreamReader` 调用 concat 函数 | 复刻版无 concat fallback 数据丢失风险 |
| 降级优先级 | 硬编码 if-else 链 | `newRunnablePacker` 优先级级联 | 思想一致 |
| 回调 context 隔离 | 每个 handler 独立 `handlerCtxs[idx]` | 同样隔离（manager + CtxRunInfoKey） | 核心语义一致 |
| TimingChecker | `neededTimings()` 位掩码 OR | `HandlerBuilder.Build()` 实现 TimingChecker 接口 | 思想一致 |
| EventLog/Callback 关系 | 独立横切面 | 独立横切面 | 职责区分一致 |
| Concat 注册 | 双注册表（`concatFns` + `concatFuncRegistry`） | 单注册表 + `internal.ConcatItems` 回退 | 复刻版分得更细，教学价值高 |

---

*本章细纲修订基于以下验证：*
- *原始 manual（`manual/03-runnable-stream-callback.md`）对照*
- *复刻版 6 个核心源文件逐行比对（`runnable.go`, `stream.go`, `concat.go`, `callbacks.go`, `event_log.go`, `generic_graph.go`）*
- *测试文件引用逐 Functions 验证（3 个测试文件，40+ 测试函数）*
- *上轮总纲（`final-eino-replica-design-samuel-reviewed.md`）对齐*
- *修正项：6 处（代码片段错误、语义混淆、遗漏文件、注册表区分、cp 边界说明、EventCheckpoint 类型补充）*
