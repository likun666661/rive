# Chapter 03 — Runnable / Stream / Callback 运行时模式教学细纲

---

## 1. 本章讲解目标

学完本章，听众应能用自己的语言解释以下问题：

1. **Runnable 四种执行模式分别解决什么输入/输出组合？** Invoke、Stream、Collect、Transform 各自对应什么调用场景？什么时候应该用哪种？
2. **为什么组件只实现 1~2 种模式就够了，但调用方可以任意选模式？** 自动降级矩阵是怎么工作的，优先级是什么，边界情况在哪里？
3. **流（StreamReader）的生命周期管理为什么是 LLM 应用正确性的关键？** 流的 Copy / Merge / Concat 什么时候用，什么时候必须 Close，不 Close 会怎样？
4. **回调（Callback）如何在不污染业务代码的前提下实现可观测性？** 五种时序（OnStart/OnEnd/OnError/OnStartWithStreamInput/OnEndWithStreamOutput）各自的触发时机、上下文隔离规则、流副本机制。
5. **EventLog 与 Callback 的职责如何分工？** 二者都记录执行过程，但面向的消费者不同——怎么选择？

**前置知识要求：**

- 了解 Eino 的 `Graph -> Compile -> Runnable` 编译边界（来自 Chapter 01）
- 理解 Go 的 `context.Context`、channel、goroutine 基本语义
- 知道什么是 LLM 的流式输出（token-by-token）

---

## 2. 这个问题在 LLM 应用 / Agent Runtime 中为什么会出现

### 2.1 LLM 应用的三种经典数据流模式

考虑一个典型的 RAG 问答链：`Retriever -> ChatModel -> FormatOutput`

```
用户问 "什么是 Eino？"
  → Retriever 检索文档（一次调用，返回文档列表）
  → ChatModel 基于文档生成回答（一次性生成 或 逐 token 流式输出）
  → FormatOutput 格式化最终文本
```

三种问题立刻浮现：

**问题 1：能力不对称。** Retriever 只有 `Retrieve(ctx, query) → []Document`（非流式），ChatModel 有 `Generate`（非流式）和 `Stream`（流式）。如果用户希望整个链 `Invoke("什么是 Eino?")` 返回完整文本，可以；但如果用户希望 `Stream("什么是 Eino?")` 得到逐 token 输出，链中的每个节点都必须知道怎么处理流——但 Retriever 不支持流式。

**问题 2：上下游模式不匹配。** 如果上游 ChatModel A 输出流，下游 ChatModel B 需要一个完整输入（非流式），谁来"把流折叠成单值"？如果下游又是流式输出，谁来"把单值摊平成流"？调用方不应该感知这些转换细节。

**问题 3：可观测性需要侵入式代码。** 如果每次都想记录"这个节点输入了什么，输出了什么，耗时多少，有没有报错"，传统做法是在每个组件内部加 `log.Printf`。但这样做会让日志逻辑散落各处，换一个组件就要重新写一遍。而且流式输出的 token 记录需要消耗流数据——如果日志处理器把流读完了，真正的下游消费者就收不到数据了。

### 2.2 如果不解决这些问题的后果

| 问题 | 如果不管 | 实际后果 |
|---|---|---|
| 能力不对称 | 调用方必须 if-else 判断每个节点支持什么模式 | 图编译失败、运行时 panic |
| 模式不匹配 | 需要手动写 convert 函数串联节点 | 代码膨胀，类型系统崩溃 |
| 观测性侵入 | 日志散落各组件 | 换一个组件重写一遍；流数据被日志消费后下游读不到 |
| 流生命周期管理 | 谁都不 Close stream reader | goroutine 泄漏，服务内存持续增长 |

---

## 3. Eino 的解决思路："问题 → 抽象 → 运行时行为"

### 3.1 第一层抽象：Runnable 四模式统一接口

**问题：** 组件能力不同，调用方需要一种方式表达"我想要哪种模式"，且运行时能自动桥接。

**Eino 的答案：** 定义一个四模式接口，让所有图节点都实现它。

```go
// compose/runnable.go:15-20
type Runnable[I, O any] interface {
    Invoke    (ctx context.Context, input I) (output O, err error)
    Stream    (ctx context.Context, input I) (output StreamReader[O], err error)
    Collect   (ctx context.Context, input StreamReader[I]) (output O, err error)
    Transform (ctx context.Context, input StreamReader[I]) (output StreamReader[O], err error)
}
```

四种模式的语义：

```
                输入是单值(I)        输入是流(StreamReader[I])
输出是单值(O)   Invoke               Collect
输出是流(...O)  Stream               Transform
```

**运行时行为：** `composableRunnable` 内部存储最多 4 个 `func(any) → any` 的函数指针（`i, s, c, t`），其中 nil 表示该模式"未原生实现"。调用任一方法时，按优先级在四种实现间寻找降级路径。

### 3.2 第二层抽象：4×4 降级矩阵

**问题：** 组件可能只实现了 {Invoke} 或 {Invoke, Stream}，但调用方可能调用任意模式。需要所有 12 种降级路径（`4 种目标 × 3 种备用源`）都正确。

**Eino 的答案：** 在 `composableRunnable` 的四个方法中硬编码降级优先级，通过少量基础原语（`recvAll` 将流转数组、`streamFromItems` 将数组转流、`collected` 将数组折叠为单值）桥接。

**复刻版降级矩阵（`compose/runnable.go:110-296`）：**

| 目标模式 | 优先级 1（最优） | 优先级 2 | 优先级 3 | 优先级 4（最后手段） |
|---|---|---|---|---|
| **Invoke** | 原生 i | `invokeByStream`: S → recvAll → collected | `invokeByCollect`: 包裹输入为流 → C | `invokeByTransform`: 包裹输入为流 → T → recvAll → collected |
| **Stream** | 原生 s | `streamByTransform`: 包裹输入为流 → T | `streamByInvoke`: I → streamFromItems | `streamByCollect`: 包裹输入为流 → C → streamFromItems |
| **Collect** | 原生 c | `collectByTransform`: T → recvAll → collected | `collectByInvoke`: recvAll → collected → I | `collectByStream`: recvAll → collected → S → recvAll → collected |
| **Transform** | 原生 t | `transformByStream`: recvAll → 逐元素 S → 合并流 | `transformByCollect`: C → streamFromItems | `transformByInvoke`: recvAll → collected → I → streamFromItems |

**为什么优先级这么重要？** 以 Stream 为例：

- `streamByTransform` 比 `streamByInvoke` 更好：Transform 本身就是"流转流"，一次转换即可；而 Invoke 是先变单值再包成流，丢失了流式语义。
- `streamByTransform` 比 `streamByCollect` 更好：Collect 要把整个流消费完才返回单值，然后再包回流，完全破坏了流式低延迟特性（必须先等上游所有数据到齐）。

### 3.3 第三层抽象：Stream 原语（Pipe / Copy / Merge / Concat）

**问题：** 降级矩阵中频繁出现"流→数组→流"的转换，需要一个轻量级的流抽象。

**Eino 的答案：** `StreamReader`/`StreamWriter` 基于 goroutine channel，提供：

| 原语 | 用途 | 在复刻版中的位置 |
|---|---|---|
| `NewPipe` | 创建带缓冲的 channel 流 | `compose/stream.go:87-90` |
| `PipeStreamReaderFromSlice` | 将切片转为已关闭的流（用于降级：数组→流） | `compose/stream.go:92-98` |
| `drainAll` | 消费整个流到切片（用于降级：流→数组） | `compose/stream.go:105-115` |
| `Copy(parent, n)` | 扇出：将流复制为 n 份独立副本 | `compose/stream.go:117-126` |
| `Merge(readers...)` | 扇入：将多个流并发合并为一个 | `compose/stream.go:128-151` |
| `Concat(readers...)` | 串联：将多个流顺序拼接，并调用用户注册的 concat 函数折叠为单值 | `compose/stream.go:160-194` |

**关键设计决策：复刻版的 Copy 是"先 drain，再拷贝切片"。** 这与 Eino 原版的"共享缓冲区 lazy copy"不同。复刻版更简单，适合教学，但代价是必须等所有数据到齐才能 Copy。这直接影响了回调的流副本语义——如果原始流是无限的（如持续输出的 LLM），复刻版的 Copy 会永远阻塞。

### 3.4 第四层抽象：Callback 被动观测

**问题：** 观测逻辑（日志、metrics、tracing）需要观测四种模式的所有生命周期事件，但不希望污染业务代码。

**Eino 的答案：** 定义五个时序的 `Handler` 接口，通过 `CallbackWrapper` 以装饰器模式包裹原始函数。

```
Handler {
    OnStart                (ctx, info, input)  → ctx   // 非流式输入就绪
    OnEnd                  (ctx, info, output) → ctx   // 非流式输出就绪
    OnError                (ctx, info, err)    → ctx   // 任一阶段报错
    OnStartWithStreamInput  (ctx, info, input)  → ctx  // 流式输入就绪（收到流副本）
    OnEndWithStreamOutput   (ctx, info, output) → ctx  // 流式输出就绪（收到流副本）
}
```

**核心设计原则：单个处理器的上下文链是隔离的。** 参见 `compose/callbacks.go:97-109` 和测试 `TestPerHandlerContextChainNotCrossHandlerGlobalOrdering`（`compose/callbacks_test.go:588-652`）。处理器 A 的 `OnStart` 返回的 ctx 只传给 A 的 `OnEnd`/`OnError`，不会传给处理器 B。这意味着不能用 context 在处理器之间传递状态。

**流副本机制：** 当处理器需要流式输入或输出时，`CallbackWrapper` 会调用 `input.Copy(n)` 创建 n 份副本（n = 需要此阶段的处理器数量）。每个处理器收到独立副本，消费者（实际节点）收到原始流。复刻版中的 Copy 是"先 drain 再拷贝"，所以**副本是完全独立的**——消费者读完不会影响处理器，反之亦然。

### 3.5 第五层抽象：EventLog 结构化事件日志

**问题：** Callback 是面向"单个节点执行"的被动钩子，但图运行时还需要一个**全图级别**的结构化事件日志——记录哪个节点何时开始、何时结束、有没有报错，并且能持久化到 JSONL 文件。

**Eino 的答案：** `EventLog` + `EventSink` 接口。

```go
type EventLog struct {
    Events []Event          // 内存中的事件列表
    sinks  []EventSink      // 持久化输出目标
}
```

`EventLog` 知道图执行的"宏观"事件（graph_start、node_start、node_end、node_error、channel_ready、checkpoint），而 Callback 关注的是"微观"的组件执行过程。二者不冲突——Callback 可以读取 `Input/Output` 细节做 metrics，EventLog 记录执行拓扑做调试。

**复刻版中的集成点：** `graph_run.go` 持有 `*EventLog`，在每个节点执行前后调用 `el.LogNodeStart` / `el.LogNodeEnd`。`graph_manager.go:171` 在每个 task 执行时通过 `CallbackWrapper` 包裹函数。

---

## 4. 复刻版对应的最小实现路径：按文件 / 函数 / 测试组织

以下是建议的"最小实现路径"——如果让听众自己实现一遍，按这个顺序写：

### Phase 1: Runnable 接口 + 降级矩阵（`compose/runnable.go`）

| 步骤 | 写什么 | 对应测试 |
|---|---|---|
| 1.1 | 定义 `Runnable[I,O]` 接口（4 个方法） | 先不写测试，编译通过即可 |
| 1.2 | 定义内部类型 `composableRunnable`（4 个 `func(any)→(any,error)`） | — |
| 1.3 | 实现基础原语：`recvAll`、`streamFromItems`、`collected`、`typedStreamWrapper`、`untypedStreamWrapper` | — |
| 1.4 | 实现 `composableRunnable.invoke()` 降级逻辑 | `TestInvokeOnlyStreamFallback`（`runnable_test.go:29`） |
| 1.5 | 实现 `composableRunnable.stream()` 降级逻辑 | `TestStreamOnlyInvokeFallbackWithConcat`（`runnable_test.go:58`） |
| 1.6 | 实现 `composableRunnable.collect()` 降级逻辑 | `TestCollectableLambdaNative`（`runnable_test.go:512`） |
| 1.7 | 实现 `composableRunnable.transform()` 降级逻辑 | `TestTransformFallbackToInvoke`（`runnable_test.go:82`） |
| 1.8 | 实现 `Lambda` 四种工厂函数 | `TestInvokeFallbackPriority` 系列 |
| 1.9 | 验证所有降级优先级顺序 | `TestInvokeFallbackPriority` / `TestStreamFallbackPriority` / `TestCollectFallbackPriority` / `TestTransformFallbackPriority` |
| 1.10 | 验证图编译产物也支持四模式降级 | `TestGraphRunnableStreamFallback`（`runnable_test.go:538`） |

### Phase 2: Stream 原语（`compose/stream.go`）

| 步骤 | 写什么 | 对应测试 |
|---|---|---|
| 2.1 | 实现 `NewPipe`（goroutine channel 流） | `TestPipeSendRecv`（`stream_test.go:9`） |
| 2.2 | 实现 `PipeStreamReaderFromSlice` / `PipeStreamReaderFromValue` | `TestPipeStreamReaderFromSlice`（`stream_test.go:93`） |
| 2.3 | 实现 `drainAll` | `TestDrainAll`（`stream_test.go:210`） |
| 2.4 | 实现 `Copy(parent, n)`（先 drain 再拷贝） | `TestCopySameData` / `TestCopyIndependentChildren`（`stream_test.go:150, 173`） |
| 2.5 | 实现 `Merge(readers...)`（多 goroutine 并发扇入） | `TestMerge`（`stream_test.go:219`） |
| 2.6 | 实现 `Concat(readers...)` + `RegisterConcatFunc` | `TestConcatFallbackLastChunk` / `TestConcatRegisteredFunction`（`stream_test.go:282, 300`） |

### Phase 3: Callback 观测（`compose/callbacks.go`）

| 步骤 | 写什么 | 对应测试 |
|---|---|---|
| 3.1 | 定义 `RunInfo`、`CallbackTiming`、`TimingChecker` | `TestRunInfo` / `TestCallbackTimingString`（`callbacks_test.go:10, 27`） |
| 3.2 | 定义 `Handler`（五种回调函数指针）及 `neededTimings()` | `TestHandlerNeededTimingsEmpty` / `TestHandlerNeededTimingsFull`（`callbacks_test.go:48, 55`） |
| 3.3 | 实现 `HandlerBuilder` + `TimingChecker` | `TestHandlerBuilderTimingCheckerPartial`（`callbacks_test.go:87`） |
| 3.4 | 实现 `CbStreamReader`（回调专用的可 Copy 流） | `TestCbStreamReaderCopy` / `TestCbStreamReaderNextAndAll`（`callbacks_test.go:239, 931`） |
| 3.5 | 实现 `CallbackWrapper.Invoke()` | `TestCallbackWrapperInvokeSuccess` / `TestCallbackWrapperInvokeError`（`callbacks_test.go:136, 187`） |
| 3.6 | 实现 `CallbackWrapper.Stream()`（含流副本分发） | `TestCallbackWrapperStreamOnEndWithStreamOutput`（`callbacks_test.go:321`） |
| 3.7 | 实现 `CallbackWrapper.Collect()`（含流副本分发） | `TestCallbackWrapperCollect`（`callbacks_test.go:379`） |
| 3.8 | 实现 `CallbackWrapper.Transform()`（含双流副本分发） | `TestCallbackWrapperTransform`（`callbacks_test.go:479`） |
| 3.9 | 验证上下文链隔离规则 | `TestPerHandlerContextChainNotCrossHandlerGlobalOrdering`（`callbacks_test.go:588`） |
| 3.10 | 验证 TimingChecker 跳过流复制 | `TestTimingCheckerSkipsStreamCopy`（`callbacks_test.go:549`） |

### Phase 4: EventLog 事件日志（`compose/event_log.go`）

| 步骤 | 写什么 | 对应测试 |
|---|---|---|
| 4.1 | 定义 `Event`、`EventType` 枚举 | `TestEventLogLifecycle`（`graph_test.go:551`） |
| 4.2 | 实现 `EventLog`（内存事件列表 + 多 sink） | `TestEventLogEmpty`（`graph_test.go:715`） |
| 4.3 | 实现 `JSONLEventSink`（写入 JSONL 文件） | `TestEventLogJSONLSinkWritesImmediately`（`graph_test.go:584`） |
| 4.4 | 实现各个 `LogNodeStart` / `LogNodeEnd` 等便利方法 | `TestEventLogAllEventTypes`（`graph_test.go:1888`） |
| 4.5 | 验证线程安全 | `TestEventLogThreadSafety`（`graph_test.go:695`） |
| 4.6 | 验证图级集成 | `TestEventLogIntegrationWithRunner`（`graph_test.go:2718`） |

---

## 5. 课堂讲解顺序（建议 25-30 分钟）

### 第一部分：问题热身（0-5 分钟）

**讲解内容：** 用 PPT 或白板画出 RAG 链的三种数据流模式。问听众："如果 Retriever 只支持非流式，上游 ChatModel 输出流式结果，你怎么把这两者连起来？"

**关键概念：**
- 四种输入输出组合 = 一张 2×2 矩阵
- 不是所有组件都实现全部四种模式
- 调用方不应该关心实现细节

**可现场演示的代码片段（5 行以内）：**

```go
// 一个只实现了 Invoke 的 Lambda
l := compose.InvokableLambda(func(ctx context.Context, input string) (string, error) {
    return "hello " + input, nil
})
// 但用户可以调用 Stream —— 框架自动降级
sr, _ := l.GetRunnable().stream(ctx, "world")
```

### 第二部分：Runnable 接口与降级矩阵（5-10 分钟）

**讲解顺序：**

1. 展示 `Runnable[I,O]` 接口定义（1 分钟）
2. 画出降级矩阵的 4×4 表格（2 分钟）—— 重点解释**为什么优先级有讲究**
3. 现场走读 `composableRunnable.invoke()` 的代码（`compose/runnable.go:110-156`），关注 4 层 if-else 的优先级顺序（2 分钟）

**课堂演示：** 运行 `TestInvokeFallbackPriority`，展示同一个 `Invoke("test")` 在三个不同 Lambda 上的不同降级路径。

**提问点：** "如果只实现了 Transform，调用 Stream 会走哪条路径？为什么不是 Invoke？"

### 第三部分：Stream 原语（10-15 分钟）

**讲解顺序：**

1. 讲 Pipe（1 分钟）：goroutine + channel 实现的生产者-消费者流
2. 讲 Copy（2 分钟）：展示 `Copy(parent, 3)` 后三个子 reader 独立工作
3. 讲 Merge（1 分钟）：多个流并发扇入到同一个输出流
4. 讲 Concat + 注册式 concat 函数（2 分钟）：字符串拼接 vs 取最后 chunk 的语义差异

**课堂演示：** 运行 `TestCopyIndependentChildren`——展示关闭子 reader 1 不影响子 reader 2。

**容易混淆点：** Merge 和 Concat 的语义区别。Merge 是并发归并（顺序不确定），Concat 是顺序拼接（顺序确定，且最终调用 concat 函数）。

### 第四部分：Callback 装饰器（15-20 分钟）

**讲解顺序：**

1. 展示 `Handler` 结构体和五种时序（1 分钟）
2. 用白板画 `CallbackWrapper.Invoke()` 的执行流程：OnStart → 实际执行 → OnEnd（如果成功）| OnError（如果失败）（2 分钟）
3. 重点讲流副本分发：`dispatchOnStartWithStreamInput` 和 `dispatchOnEndWithStreamOutput` 的内部逻辑（`compose/callbacks.go:308-352`）（3 分钟）
4. 讲 TimingChecker 的优化作用（2 分钟）

**课堂演示：**

```go
// 现场写一个带回调的 Invoke 调用
handler := &compose.Handler{
    OnStart: func(ctx context.Context, info *compose.RunInfo, input any) context.Context {
        fmt.Printf("[%s] OnStart\n", info.Name)
        return ctx
    },
    OnEnd: func(ctx context.Context, info *compose.RunInfo, output any) context.Context {
        fmt.Printf("[%s] OnEnd: %v\n", info.Name, output)
        return ctx
    },
}
cw := compose.NewCallbackWrapper(info, []*compose.Handler{handler})
wrapped := cw.Invoke(myFn)
wrapped(ctx, "hello")
```

运行后展示输出顺序。

**提问点：** "如果处理器 B 在 OnStart 中通过 `context.WithValue` 设置了 key，处理器 A 的 OnEnd 能看到吗？"（答案：不能，因为每个处理器收到的都是 base ctx）

### 第五部分：EventLog（20-25 分钟）

**讲解内容：** 与 Callback 对比，EventLog 记录的是什么？

| 维度 | Callback | EventLog |
|---|---|---|
| 抽象层级 | 单个组件/节点 | 全图 / 全局 |
| 触发时机 | 组件执行前后（微观） | 节点调度、channel 就绪、checkpoint（宏观） |
| 数据内容 | Input/Output 的完整值 | 节点 key、step、graph name（结构化元数据） |
| 消费者 | 应用代码（metrics、tracing） | 运维工具（JSONL 文件、调试面板） |

**课堂演示：** 展示 `TestEventLogLifecycle` 的输出——从 `graph_start` 到 `node_end` 到 `graph_end` 的完整时间线。

### 第六部分：回顾与练习（25-30 分钟）

快速回顾 5 个关键点，发布练习题。

---

## 6. 代码走读脚本

以下是建议的"代码走读路线"——按这个顺序带着听众看代码：

### 走读路线图

```
1. compose/runnable.go
   ├── 15-20: Runnable 接口定义
   ├── 28-40: internalStreamReader (降级用的内存流)
   ├── 43-72: typedStreamWrapper / untypedStreamWrapper (类型适配)
   ├── 74-101: recvAll / streamFromItems / collected (基础原语)
   ├── 110-156: composableRunnable.invoke() ← 最核心的降级逻辑
   ├── 158-184: composableRunnable.stream()
   ├── 186-240: composableRunnable.collect()
   ├── 242-296: composableRunnable.transform()
   └── 311-385: Lambda 四种工厂函数

2. compose/stream.go
   ├── 87-90: NewPipe
   ├── 92-98: PipeStreamReaderFromSlice
   ├── 105-115: drainAll
   ├── 117-126: Copy(parent, n)  ← 扇出核心
   ├── 128-151: Merge(readers...) ← 扇入核心
   └── 160-194: Concat(readers...) ← 流折叠核心

3. compose/callbacks.go
   ├── 8-22: RunInfo / CallbackTiming / TimingChecker
   ├── 43-89: CbStreamReader 及其 Copy 方法
   ├── 97-109: Handler 结构体
   ├── 111-129: neededTimings()  ← TimingChecker 的基础
   ├── 131-156: HandlerBuilder
   ├── 158-210: CallbackWrapper.Invoke()  ← 最核心：装饰器模式
   ├── 212-243: CallbackWrapper.Stream()
   ├── 245-274: CallbackWrapper.Collect()
   ├── 276-306: CallbackWrapper.Transform()
   └── 308-362: dispatchOnStartWithStreamInput / dispatchOnEndWithStreamOutput ← 流副本分发

4. compose/event_log.go
   ├── 13-26: EventType 枚举
   ├── 28-37: Event 结构体
   ├── 107-109: NewEventLog
   ├── 149-162: Log()  ← 同时写入内存和 sinks
   └── 164-194: LogNodeStart / LogNodeEnd 等便利方法

5. 测试文件（走读顺序）
   ├── compose/runnable_test.go
   │   ├── TestInvokeOnlyStreamFallback       (line 29)  ← 第一站：Invoke→Stream 降级
   │   ├── TestStreamOnlyInvokeFallbackWithConcat (line 58) ← Stream→Invoke 降级
   │   ├── TestTransformFallbackToInvoke      (line 82)  ← Transform→Invoke 降级
   │   ├── TestAllFourModesNative             (line 170) ← 全部原生四种
   │   └── TestGraphRunnableStreamFallback    (line 538) ← Graph 的 Runnable 也支持降级
   │
   ├── compose/stream_test.go
   │   ├── TestPipeSendRecv                   (line 9)    ← 基础 Pipe
   │   ├── TestCopySameData                   (line 150)  ← Copy 扇出
   │   ├── TestCopyIndependentChildren        (line 173)  ← 子 reader 独立性
   │   ├── TestMerge                          (line 219)  ← Merge 扇入
   │   └── TestConcatRegisteredFunction       (line 300)  ← 注册 concat 函数
   │
   └── compose/callbacks_test.go
       ├── TestCallbackWrapperInvokeSuccess   (line 136)  ← 基本 Invoke 回调
       ├── TestCallbackWrapperStreamOnEndWithStreamOutput (line 321) ← 流输出回调
       ├── TestCallbackWrapperTransform       (line 479)  ← Transform 双流回调
       ├── TestPerHandlerContextChainNotCrossHandlerGlobalOrdering (line 588) ← 上下文隔离
       └── TestCallbackWrapperInvokeError     (line 187)  ← 错误回调
```

### 用"一个字符串拼接链"串起来讲

建议用下面这个例子贯穿整个走读：

```go
// Step 1: 创建一个只有 Invoke 的节点（模拟 Retriever）
retriever := compose.InvokableLambda(func(ctx context.Context, input string) (string, error) {
    return "docs: [" + input + "]", nil
})

// Step 2: 创建一个只有 Stream 的节点（模拟 ChatModel 流式输出）
chatModel := compose.StreamableLambda(func(ctx context.Context, input string) (compose.StreamReader[string], error) {
    // 模拟逐 token 输出
    return sliceStreamReader("t1:"+input, "t2:"+input, "t3:"+input), nil
})

// Step 3: 调用 retriever 的 Stream——它会自动降级
cr1 := retriever.GetRunnable()
sr, _ := cr1.stream(ctx, "query")
// 验证：sr 是一个包含单个元素的流

// Step 4: 调用 chatModel 的 Invoke——它会自动降级
cr2 := chatModel.GetRunnable()
out, _ := cr2.invoke(ctx, "some_input")
// 验证：out 是收集后的流结果

// Step 5: 用 CallbackWrapper 包裹 chatModel
info := &compose.RunInfo{Name: "chat", Type: "ChatModel", Component: compose.ComponentOfLambda}
handler := &compose.Handler{
    OnEndWithStreamOutput: func(ctx context.Context, info *compose.RunInfo, output *compose.CbStreamReader) context.Context {
        count := 0
        for {
            _, ok := output.Next()
            if !ok { break }
            count++
        }
        fmt.Printf("[%s] streamed %d tokens\n", info.Name, count)
        return ctx
    },
}
cw := compose.NewCallbackWrapper(info, []*compose.Handler{handler})
// ... 用 cw.Stream 包裹后调用
```

---

## 7. 容易误解点和反例

### 误解点 1：Copy 后还能用原始 reader 吗？

**错误理解：** `Copy(parent, 3)` 后可以用 parent 继续读取，同时三个 child 也可以读。

**真相：** 复刻版的 `Copy` 实现是先 `drainAll(parent)` 再创建 N 份切片拷贝。调用 `Copy` 后，parent 已经被消费完了。如果你想保留一份原始数据，应该 `Copy(parent, N+1)`，自己持有其中一份。参见 `compose/stream.go:117-126`。

**反例：**

```go
sr := PipeStreamReaderFromSlice([]int{1, 2, 3})
children := Copy(sr, 2)
// 此时 sr 已经被 drain 完了！
_, ok := sr.Recv()  // ok == false !
```

### 误解点 2：回调处理器的 context 是全局串联的

**错误理解：** 处理器 A 的 OnStart 返回的 ctx 会传给处理器 B。

**真相：** `CallbackWrapper` 为每个 handler 独立存储 `handlerCtxs[idx]`。每个 handler 的 OnStart 收到的都是 base ctx（即调用方传入的原始 ctx），不是上一个 handler 修改后的。参见 `compose/callbacks.go:183-191` 和测试 `TestPerHandlerContextChainNotCrossHandlerGlobalOrdering`。

**反例：**

```go
handler1 := &Handler{
    OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
        return context.WithValue(ctx, "key", "value-from-h1")  // h1 设置了 key
    },
}
handler2 := &Handler{
    OnStart: func(ctx context.Context, info *RunInfo, input any) context.Context {
        val := ctx.Value("key")  // 期望："value-from-h1"
        // 实际：nil！因为 h2 收到的也是 base ctx
        return ctx
    },
}
```

### 误解点 3：降级矩阵的优先级只影响性能，不影响正确性

**错误理解：** 只要四种降级路径最终都能产生结果，用哪条无所谓。

**真相：** 不同降级路径的语义可能不同。例如，`streamByInvoke` 会等到 Invoke 完成才返回流（单元素流），而 `streamByTransform` 可以逐元素输出（真流式）。如果你用一个只有 Invoke 的节点做 Stream，你会得到一个立即返回的单元素流——虽然"正确"，但完全失去了流式低延迟的优势。

**更危险的例子：** `collectByStream` vs `collectByInvoke`。如果 Stream 输出多个数据块，`collectByStream` 会调用 concat 函数折叠它们；如果没有注册 concat 函数，回退到取最后一个数据块——对于"增量"类型（每次输出完整消息）没问题，对于"累加"类型（每次输出一个增量）会丢数据。

### 误解点 4：Merge 保证输入顺序

**错误理解：** `Merge(sr1, sr2, sr3)` 会先输出 sr1 的所有数据，再输出 sr2 的数据。

**真相：** Merge 是并发扇入——每个 reader 在一个独立的 goroutine 中读取，谁先到谁先写入输出 channel。顺序不确定。参见 `compose/stream.go:128-151`。

### 误解点 5：Concat 等价于"把所有 reader 的数据列出"

**错误理解：** `Concat` 就是把多个 reader 的数据拼成一个数组。

**真相：** `Concat` 收集所有 reader 的所有数据块后，调用注册的 concat 函数将其折叠为**单个值**。如果没有注册 concat 函数，回退到取最后一块。参见 `compose/stream.go:160-194`。

### 误解点 6：回调的 OnStartWithStreamInput 会消费流数据，影响实际业务

**错误理解：** 如果处理器在 OnStartWithStreamInput 中把流读完了，实际业务函数就收不到数据了。

**真相：** `CallbackWrapper` 会先调用 `input.Copy(n)` 创建 n 份副本，n = 需要此阶段的处理器数量。每个处理器收到独立副本。业务函数收到的仍然是原始流（的 Copy）。但由于 Copy 是"先 drain 再拷贝"，所以不会互相影响。参见 `compose/callbacks.go:308-332` 和测试 `TestCallbackWrapperCollectHandlerReceivesCopiedReader`。

---

## 8. 练习题 / 思考题

### 基础题

**Q1:** 一个组件只实现了 `Stream(ctx, input)` 方法。用户调用了 `Invoke(ctx, input)`。请画出完整的调用链（从 `composableRunnable.invoke()` 开始，经过 `recvAll` → `collected`，返回最终结果）。

**Q2:** 以下代码会产生什么输出？为什么？

```go
sr, sw := compose.NewPipe[int](2)
sw.Send(1)
sw.Send(2)
sw.Close()
items := compose.drainAll(sr)
fmt.Println(len(items))  // ？
```

### 进阶题

**Q3:** 有两种方法实现"将流式输出转为非流式输出"的节点：
- 方法 A: 只实现 `Collect`（消费输入流，返回单值），不实现任何其他模式
- 方法 B: 只实现 `Transform`（消费输入流，返回输出流），不实现任何其他模式

当上游节点输出流、用户调用 `Invoke` 时，这两种节点分别走哪条降级路径？哪种更高效？为什么？

**Q4:** 在 `TestPerHandlerContextChainNotCrossHandlerGlobalOrdering` 中，handler1 的 OnStart 设置了 context key 为 `"from-h1-onstart"`。如果你想让 handler2 能读取到这个 key，需要怎么修改？这种修改会引入什么问题？

**Q5:** 以下回调处理器有什么问题？

```go
handler := &compose.Handler{
    OnEndWithStreamOutput: func(ctx context.Context, info *compose.RunInfo, output *compose.CbStreamReader) context.Context {
        // 只读了第一个 token
        first, _ := output.Next()
        fmt.Println("first token:", first)
        // 没有读完剩余数据
        return ctx
    },
}
```

### 设计题

**Q6:** 假设你要为这个复刻版实现"真正的 lazy copy"（即 Copy 后不 drain 原始流，而是所有子 reader 共享同一个底层 channel），你需要怎么改 `stream.go` 中的 `Copy` 函数？这种实现有什么新的风险？

**Q7:** 如果你要在 `CallbackWrapper` 中增加一个 `OnBeforeRetry` 回调阶段，它应该放在 `OnError` 之后，仅在图执行重试时触发。请画出该阶段的触发时序图，并说明它与现有 `OnError` 的区别。

**Q8:** 当 `EventLog` 同时有 3 个 sinks（2 个 JSONL 文件 + 1 个内存 buffer），如果其中一个 sink 的 `WriteEvent` 返回了错误，其他 sink 会受影响吗？`EventLog` 如何处理这个错误？

---

## 9. 附录：代码索引

### A. 核心源文件

| 文件 | 核心内容 | 行数 | 为什么要看 |
|---|---|---|---|
| `compose/runnable.go` | `Runnable[I,O]` 接口定义 + `composableRunnable` 四模式降级实现 + `Lambda` 工厂函数 | 385 | **整个章节的基础**。这是所有图节点编译后的统一包装。理解了降级矩阵就理解了为什么 Graph 能无缝组合不同能力的组件。 |
| `compose/stream.go` | `PipeStreamReader` / `PipeStreamWriter` + `Copy` / `Merge` / `Concat` + `RegisterConcatFunc` | 194 | **流原语层**。降级矩阵中的所有"流转数组/数组转流"操作都依赖这些原语。Copy 是回调流副本分发的底层实现。 |
| `compose/callbacks.go` | `RunInfo` / `Handler` / `CallbackWrapper` / `HandlerBuilder` + `TimingChecker` | 383 | **观测层核心**。装饰器模式的典范。理解五个时序的触发时机和上下文隔离规则是写出正确观测代码的前提。 |
| `compose/event_log.go` | `Event` / `EventLog` / `EventSink` / `JSONLEventSink` | 229 | **全图级结构化日志**。与 Callback 互补——Callback 关注微观执行，EventLog 记录宏观拓扑。 |

### B. 核心函数 / 类型

| 文件 | 类型 / 函数 | 行号 | 说明 |
|---|---|---|---|
| `compose/runnable.go` | `Runnable[I,O any]` | 15-20 | 四模式统一接口（公共 API） |
| `compose/runnable.go` | `composableRunnable` | 103-108 | 内部包装，持有 i/s/c/t 四个函数指针 |
| `compose/runnable.go` | `composableRunnable.invoke()` | 110-156 | Invoke 降级：i → S → C → T |
| `compose/runnable.go` | `composableRunnable.stream()` | 158-184 | Stream 降级：s → T → I → C |
| `compose/runnable.go` | `composableRunnable.collect()` | 186-240 | Collect 降级：c → T → I → S |
| `compose/runnable.go` | `composableRunnable.transform()` | 242-296 | Transform 降级：t → S → C → I |
| `compose/runnable.go` | `recvAll` | 74-87 | 将 `streamReader` 消费为 `[]any`（降级基础原语） |
| `compose/runnable.go` | `streamFromItems` | 89-91 | 将 `[]any` 包装为 `internalStreamReader`（降级基础原语） |
| `compose/runnable.go` | `collected` | 93-101 | 将 `[]any` 折叠为单值（0→nil, 1→第一个, >1→返回数组） |
| `compose/runnable.go` | `InvokableLambda[I,O any]` | 311-323 | 创建只有 Invoke 的 Lambda |
| `compose/runnable.go` | `StreamableLambda[I,O any]` | 325-341 | 创建只有 Stream 的 Lambda |
| `compose/runnable.go` | `CollectableLambda[I,O any]` | 343-355 | 创建只有 Collect 的 Lambda |
| `compose/runnable.go` | `TransformableLambda[I,O any]` | 357-373 | 创建只有 Transform 的 Lambda |
| `compose/stream.go` | `NewPipe[T any]` | 87-90 | 创建 goroutine channel 流程对 |
| `compose/stream.go` | `PipeStreamReaderFromSlice` | 92-98 | 将切片转为已关闭流 |
| `compose/stream.go` | `drainAll` | 105-115 | 消费整个流到切片 |
| `compose/stream.go` | `Copy[T any]` | 117-126 | 扇出：先 drain 再拷贝 N 份 |
| `compose/stream.go` | `Merge[T any]` | 128-151 | 扇入：多 goroutine 并发归并 |
| `compose/stream.go` | `Concat[T any]` | 160-194 | 串联折叠：收集所有块后调 concat 函数 |
| `compose/stream.go` | `RegisterConcatFunc` | 155-158 | 为类型注册自定义 concat 逻辑 |
| `compose/callbacks.go` | `RunInfo` | 8-12 | 节点执行标识（Name + Type + Component） |
| `compose/callbacks.go` | `CallbackTiming` | 14-22 | 五位掩码枚举：五个回调阶段 |
| `compose/callbacks.go` | `TimingChecker` | 41 | 检查某阶段是否需要的函数类型 |
| `compose/callbacks.go` | `Handler` | 97-109 | 五种回调函数指针的聚合体 |
| `compose/callbacks.go` | `Handler.neededTimings()` | 111-129 | 从 Handler 推断需要的 Timing |
| `compose/callbacks.go` | `HandlerBuilder` | 131-156 | Handler 构建器，自动生成 TimingChecker |
| `compose/callbacks.go` | `CallbackWrapper` | 158-168 | 装饰器：包裹四种执行模式添加回调 |
| `compose/callbacks.go` | `CallbackWrapper.Invoke()` | 180-210 | 包装 Invoke：OnStart → 执行 → OnEnd/OnError |
| `compose/callbacks.go` | `CallbackWrapper.Stream()` | 212-243 | 包装 Stream：OnStart → 执行 → 分发流副本 → OnEnd |
| `compose/callbacks.go` | `CallbackWrapper.Collect()` | 245-274 | 包装 Collect：分发流副本 → OnStart → 执行 → OnEnd |
| `compose/callbacks.go` | `CallbackWrapper.Transform()` | 276-306 | 包装 Transform：分发流副本 → OnStart → 执行 → 分发流副本 → OnEnd |
| `compose/callbacks.go` | `dispatchOnStartWithStreamInput` | 308-332 | 流输入副本分发逻辑（Copy + 逐个调用处理器） |
| `compose/callbacks.go` | `dispatchOnEndWithStreamOutput` | 334-352 | 流输出副本分发逻辑（Copy + 逐个调用处理器） |
| `compose/callbacks.go` | `CbStreamReader` | 43-89 | 回调专用流 reader，支持 Copy 和独立消费 |
| `compose/event_log.go` | `Event` | 28-37 | 事件结构体 |
| `compose/event_log.go` | `EventType` | 13-26 | 10 种事件类型枚举 |
| `compose/event_log.go` | `EventLog` | 100-105 | 事件日志（内存 + 多 sink） |
| `compose/event_log.go` | `EventLog.Log()` | 149-162 | 写入事件到内存和所有 sinks |
| `compose/event_log.go` | `EventSink` | 40-42 | sink 接口：接收持久化事件 |
| `compose/event_log.go` | `JSONLEventSink` | 49-98 | JSONL 文件 sink 实现 |

### C. 测试文件索引

| 文件 | 测试函数 | 行号 | 验证什么 |
|---|---|---|---|
| `compose/runnable_test.go` | `TestInvokeOnlyStreamFallback` | 29 | Invoke→Stream 降级（Invoke 结果包成单元素流） |
| `compose/runnable_test.go` | `TestStreamOnlyInvokeFallbackWithConcat` | 58 | Stream→Invoke 降级（流被 drain 为数组） |
| `compose/runnable_test.go` | `TestTransformFallbackToInvoke` | 82 | Transform→Invoke 降级（单值→包裹为流→Transform→drain→返回） |
| `compose/runnable_test.go` | `TestTransformFallbackToStream` | 108 | Transform→Stream 降级（单值→包裹为流→Transform→返回流） |
| `compose/runnable_test.go` | `TestAllFourModesNative` | 170 | 四种模式全原生实现，验证每个模式都按预期工作 |
| `compose/runnable_test.go` | `TestUnsupportedModeError` | 291 | 空 composableRunnable 调用任意模式都报错 |
| `compose/runnable_test.go` | `TestInvokeFallbackPriority` | 316 | 验证 Invoke 降级优先级：S > C > T |
| `compose/runnable_test.go` | `TestStreamFallbackPriority` | 362 | 验证 Stream 降级优先级：T > I > C |
| `compose/runnable_test.go` | `TestCollectFallbackPriority` | 414 | 验证 Collect 降级优先级：T > I > S |
| `compose/runnable_test.go` | `TestTransformFallbackPriority` | 456 | 验证 Transform 降级优先级：S > C > I |
| `compose/runnable_test.go` | `TestGraphRunnableStreamFallback` | 538 | 图编译产物也支持四模式降级 |
| `compose/stream_test.go` | `TestPipeSendRecv` | 9 | 基础 Pipe 收发 |
| `compose/stream_test.go` | `TestPipeSendAfterClose` | 33 | Close 后 Send 返回 ErrStreamClosed |
| `compose/stream_test.go` | `TestPipeRecvAfterClose` | 43 | Close 后仍能读缓冲数据 |
| `compose/stream_test.go` | `TestCopySameData` | 150 | Copy 后所有子 reader 都包含相同数据 |
| `compose/stream_test.go` | `TestCopyIndependentChildren` | 173 | 关闭子 reader 0 不影响子 reader 1 |
| `compose/stream_test.go` | `TestCopyZeroChildren` | 201 | Copy 0 份返回空切片 |
| `compose/stream_test.go` | `TestDrainAll` | 210 | drainAll 消费完整流 |
| `compose/stream_test.go` | `TestMerge` | 219 | 多流并发归并后数据完整 |
| `compose/stream_test.go` | `TestMergeEmptyReaders` | 247 | Merge 空 reader 列表返回已关闭流 |
| `compose/stream_test.go` | `TestMergeSingleReader` | 256 | Merge 单个 reader 等价于原 reader |
| `compose/stream_test.go` | `TestConcatFallbackLastChunk` | 282 | 无注册函数时 Concat 返回最后一块 |
| `compose/stream_test.go` | `TestConcatRegisteredFunction` | 300 | 注册 concat 函数后正确折叠 |
| `compose/stream_test.go` | `TestConcatEmptyReaders` | 321 | Concat 空 reader 列表返回已关闭流 |
| `compose/stream_test.go` | `TestPipeConcurrentSendRecv` | 368 | 并发收发 100 个元素 |
| `compose/stream_test.go` | `TestCopyParentAlreadyConsumed` | 451 | 已消费的流 Copy 后子 reader 也为空 |
| `compose/callbacks_test.go` | `TestCallbackWrapperInvokeSuccess` | 136 | Invoke 成功时 OnStart→OnEnd 触发 |
| `compose/callbacks_test.go` | `TestCallbackWrapperInvokeError` | 187 | Invoke 失败时 OnStart→OnError 触发，OnEnd 不触发 |
| `compose/callbacks_test.go` | `TestCallbackWrapperStreamOnEndWithStreamOutput` | 321 | Stream 回调：handler 和 consumer 都拿到独立流副本 |
| `compose/callbacks_test.go` | `TestCallbackWrapperCollect` | 379 | Collect 回调：OnStartWithStreamInput 触发 |
| `compose/callbacks_test.go` | `TestCallbackWrapperCollectHandlerReceivesCopiedReader` | 439 | handler 的流副本独立于 consumer |
| `compose/callbacks_test.go` | `TestCallbackWrapperTransform` | 479 | Transform 双流回调（输入+输出）都触发 |
| `compose/callbacks_test.go` | `TestPerHandlerContextChainNotCrossHandlerGlobalOrdering` | 588 | 处理器上下文隔离规则验证 |
| `compose/callbacks_test.go` | `TestTimingCheckerSkipsStreamCopy` | 549 | 未注册流阶段的 handler 不会触发 Copy |
| `compose/callbacks_test.go` | `TestTimingCheckerSignalsStreamCopyNeeded` | 568 | 注册了流阶段的 handler 会触发 Copy |
| `compose/graph_test.go` | `TestEventLogLifecycle` | 551 | EventLog 全生命周期：start → node_end → graph_end |
| `compose/graph_test.go` | `TestEventLogJSONLSinkWritesImmediately` | 584 | JSONL sink 实时写入 |
| `compose/graph_test.go` | `TestEventLogThreadSafety` | 695 | EventLog 并发写入安全 |
| `compose/graph_test.go` | `TestEventLogIntegrationWithRunner` | 2718 | EventLog 与图执行的完整集成 |

### D. 关键设计决策速查

| 决策 | 复刻版做法 | 原版 Eino 做法 | 差异与影响 |
|---|---|---|---|
| Stream Copy 语义 | 先 drain 再拷贝切片（eager copy） | 共享缓冲区 lazy copy（parentStreamReader + childStreamReader） | 复刻版简单但无法处理无限流；原版更复杂但支持无限流 |
| 降级优先级 | 硬编码 if-else 链 | `newRunnablePacker` 中的优先级级联 | 思想一致，原版通过 packer 抽象更泛化 |
| 回调上下文链 | 每个 handler 独立存储 ctx，互不影响 | 同样隔离，但通过 `manager` + `CtxRunInfoKey` 实现嵌套图中的 RunInfo 覆盖 | 核心语义一致，原版有更复杂的嵌套图支持 |
| TimingChecker | `Handler.neededTimings()` 按位掩码 OR | `HandlerBuilder.Build()` 实现 `TimingChecker` 接口 | 思想一致——只在需要时才分配流副本 |
| EventLog 与 Callback 关系 | EventLog 在 `graph_run.go` 中独立调用；Callback 在 `graph_manager.go` 通过 `CallbackWrapper` 包裹 | EventLog 和 Callback 是两个独立横切面 | 职责区分一致 |
| Concat 回退策略 | 无注册函数时取最后一块 | 同样，通过 `internal.ConcatItems` 回退 | 一致 |

---

*本章细纲基于以下输入生成：*
- *Eino 技术手册 Chapter 03 原文（`manual/03-runnable-stream-callback.md`）*
- *Go 复刻工程源码（`compose/runnable.go`, `compose/stream.go`, `compose/callbacks.go`, `compose/event_log.go` 及其测试文件）*
- *上轮总纲（`final-eino-replica-design-samuel-reviewed.md`）*
