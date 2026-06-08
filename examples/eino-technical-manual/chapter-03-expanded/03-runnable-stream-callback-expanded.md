# Chapter 03 - Runnable / Stream / Callback 深度讲解

面向读者：假设你已经读过 Chapter 01 和 Chapter 02，知道 `Graph / Workflow / Chain` 最后都会 `Compile` 成 `Runnable`，也知道底层执行由 `runner.run` 根据图拓扑调度节点。

这一章要回答的问题是：

```text
编译出来的 Runnable 为什么有四种执行形态？
一个组件只实现 Invoke，为什么还能被 Stream/Collect/Transform 调用？
Stream 在当前复刻版里到底是什么？
Callback 是如何在节点执行前后插入的？
```

参考代码位置：

- 手册：`examples/eino-technical-manual/manual/03-runnable-stream-callback.md`
- 复刻版：`examples/eino-compose-runtime-replica-go`
- 本章重点源码：
  - `compose/runnable.go`
  - `compose/stream.go`
  - `compose/callbacks.go`
  - `compose/graph_manager.go`
  - `compose/chatmodel.go`
  - `compose/runnable_test.go`
  - `compose/stream_test.go`
  - `compose/callbacks_test.go`
  - `compose/chatmodel_test.go`

说明：原始手册里包含不少原版 Eino 的复杂设计，例如更完整的 stream reader copy 语义、内部 callback manager、组件级 callback checker、路径级 callback 注入等。当前 Go 复刻版更轻量，本文以当前复刻版源码为准，并在必要处明确标注“当前复刻版的边界”。

## 1. 从前两章接上：Runnable 是所有编排的终点

Chapter 01 讲过：

```text
Graph construction -> Compile -> Runnable execution
```

Chapter 02 讲过：

```text
Graph / Workflow / Chain -> Compile -> Runnable
```

所以 `Runnable` 是 compose runtime 的统一执行出口。

无论你用的是：

- 手动 `Graph`
- 声明式 `Workflow`
- builder 风格 `Chain`
- 一个 `Lambda`
- 一个 `ChatModelComponent`
- 一个 `Retriever`

最终都要被包装成某种 runnable，然后由 runtime 用统一方式调用。

当前复刻版里的公开接口在 `compose/runnable.go`：

```go
type Runnable[I, O any] interface {
    Invoke(ctx context.Context, input I) (output O, err error)
    Stream(ctx context.Context, input I) (output StreamReader[O], err error)
    Collect(ctx context.Context, input StreamReader[I]) (output O, err error)
    Transform(ctx context.Context, input StreamReader[I]) (output StreamReader[O], err error)
}
```

这就是本章的中心。

## 2. 四种执行形态到底是什么

先别急着看代码。先把四个词讲清楚。

### 2.1 Invoke：普通输入，普通输出

```text
I -> O
```

这是最常见的函数调用。

例子：

```go
func(ctx context.Context, input string) (string, error) {
    return strings.ToUpper(input), nil
}
```

输入 `"hello"`，输出 `"HELLO"`。

### 2.2 Stream：普通输入，流式输出

```text
I -> Stream[O]
```

这适合模型 token-by-token 输出。

例子：

```text
input:  "hello"
output: "Hel" -> "lo" -> EOF
```

调用方可以边读边处理，而不是等完整结果。

### 2.3 Collect：流式输入，普通输出

```text
Stream[I] -> O
```

这适合把多个 chunk 收集成一个结果。

例子：

```text
input stream:  "a" -> "b" -> "c"
output:        "abc"
```

### 2.4 Transform：流式输入，流式输出

```text
Stream[I] -> Stream[O]
```

这适合流式转换。

例子：

```text
input stream:  "a" -> "b" -> "c"
output stream: "A" -> "B" -> "C"
```

这四种形态组合起来，就能覆盖 LLM 应用里的多数数据流形式。

## 3. 为什么需要 fallback

真实组件不一定四种模式都实现。

比如：

- 普通 lambda 只实现 Invoke。
- ChatModel 可能实现 Invoke + Stream。
- 一个 parser 可能只实现 Collect。
- 一个 token filter 可能只实现 Transform。

但图 runtime 希望上层统一调用：

```go
r.Invoke(...)
r.Stream(...)
r.Collect(...)
r.Transform(...)
```

问题来了：

```text
如果组件只实现 Invoke，但用户调用 Stream，怎么办？
如果组件只实现 Stream，但用户调用 Invoke，怎么办？
```

答案是 fallback，也就是自动降级/桥接。

当前复刻版的 `composableRunnable` 保存四个可选函数：

```go
type composableRunnable struct {
    i func(ctx context.Context, input any) (output any, err error)
    s func(ctx context.Context, input any) (output any, err error)
    c func(ctx context.Context, input any) (output any, err error)
    t func(ctx context.Context, input any) (output any, err error)
}
```

如果某个函数不存在，就尝试用其他函数模拟。

## 4. 当前复刻版的 fallback 矩阵

看 `compose/runnable.go` 里的四个方法：`invoke`、`stream`、`collect`、`transform`。

### 4.1 invoke 的优先级

```go
func (cr *composableRunnable) invoke(ctx context.Context, input any) (any, error) {
    if cr.i != nil { ... }
    if cr.s != nil { ... }
    if cr.c != nil { ... }
    if cr.t != nil { ... }
    return nil, fmt.Errorf("runnable: Invoke not supported")
}
```

优先级：

```text
Invoke native
-> Stream fallback
-> Collect fallback
-> Transform fallback
```

含义：

- 有原生 Invoke，直接调用。
- 只有 Stream，就先 Stream，再把 stream 读完收集成普通值。
- 只有 Collect，就把普通 input 包成单元素 stream，再 Collect。
- 只有 Transform，就把普通 input 包成单元素 stream，Transform 后读完。

测试：

- `TestStreamOnlyInvokeFallbackWithConcat`
- `TestTransformFallbackToInvoke`
- `TestInvokeFallbackPriority`

### 4.2 stream 的优先级

```go
func (cr *composableRunnable) stream(ctx context.Context, input any) (any, error) {
    if cr.s != nil { ... }
    if cr.t != nil { ... }
    if cr.i != nil { ... }
    if cr.c != nil { ... }
    return nil, fmt.Errorf("runnable: Stream not supported")
}
```

优先级：

```text
Stream native
-> Transform fallback
-> Invoke fallback
-> Collect fallback
```

最常见的是 Invoke fallback：

```go
out, err := cr.i(ctx, input)
return streamFromItems(out), nil
```

也就是把普通输出包成一个只有一个元素的 stream。

测试：`TestInvokeOnlyStreamFallback`。

这很重要：只实现 Invoke 的组件也可以被 Stream 调用，但它不是真正的 token stream，只是“单元素流”。

### 4.3 collect 的优先级

```text
Collect native
-> Transform fallback
-> Invoke fallback
-> Stream fallback
```

如果只有 Invoke：

```go
items, err := recvAll(wr)
return cr.i(ctx, collected(items))
```

也就是先把输入流读完，再把收集后的值交给 Invoke。

如果输入流只有一个元素，`collected(items)` 返回这个元素。

如果输入流有多个元素，`collected(items)` 返回 `[]any`。

### 4.4 transform 的优先级

```text
Transform native
-> Stream fallback
-> Collect fallback
-> Invoke fallback
```

如果只有 Invoke：

```go
items, err := recvAll(wr)
out, err := cr.i(ctx, collected(items))
return streamFromItems(out), nil
```

也就是：

```text
输入流 -> 读完 -> Invoke -> 单元素输出流
```

## 5. collected 的小规则

`collected` 是一个容易被忽略的小函数：

```go
func collected(items []any) any {
    if len(items) == 0 {
        return nil
    }
    if len(items) == 1 {
        return items[0]
    }
    return items
}
```

这意味着 fallback 后的输入/输出类型可能有三种：

- 空流 -> `nil`
- 单元素流 -> 元素本身
- 多元素流 -> `[]any`

这也是为什么有些测试里期望 `[]any`，例如 `TestStreamOnlyInvokeFallbackWithConcat`。

初学时最容易误解的是：Stream fallback 的“收集”不一定会拼成字符串；当前复刻版默认只是把多个 chunk 变成 `[]any`，除非你用 pipe stream 的 `Concat` 或 message concat 相关工具。

## 6. Lambda 四种构造器

当前复刻版提供四种 Lambda 构造：

### 6.1 InvokableLambda

```go
InvokableLambda(func(ctx context.Context, input I) (O, error) { ... })
```

只填 `cr.i`。

适合普通同步节点。

### 6.2 StreamableLambda

```go
StreamableLambda(func(ctx context.Context, input I) (StreamReader[O], error) { ... })
```

只填 `cr.s`。

适合普通输入，流式输出。

### 6.3 CollectableLambda

```go
CollectableLambda(func(ctx context.Context, input StreamReader[I]) (O, error) { ... })
```

只填 `cr.c`。

适合把流式输入收集成一个输出。

### 6.4 TransformableLambda

```go
TransformableLambda(func(ctx context.Context, input StreamReader[I]) (StreamReader[O], error) { ... })
```

只填 `cr.t`。

适合流式转换。

这些构造器内部都会做类型断言。例如 `InvokableLambda`：

```go
typedInput, ok := input.(I)
if !ok {
    var zero I
    return zero, fmt.Errorf("InvokableLambda: expected input type %T, got %T", zero, input)
}
```

所以 fallback 很方便，但不代表类型可以随便混。

## 7. StreamReader：公开接口和内部接口

当前复刻版里有两套 stream 概念，要分清。

### 7.1 Runnable 使用的 StreamReader

`runnable.go` 里：

```go
type StreamReader[T any] interface {
    Recv() (T, error)
}
```

这是泛型公开接口。读不到数据时通常返回 `io.EOF`。

### 7.2 composableRunnable 内部的 streamReader

```go
type streamReader interface {
    Recv() (any, error)
}
```

这是非泛型内部接口，用于 fallback 时统一处理。

泛型到非泛型靠：

```go
typedStreamWrapper[T]
```

非泛型到泛型靠：

```go
untypedStreamWrapper[T]
```

这两个 wrapper 的作用是类型擦除和类型恢复。

## 8. streamFromItems 和 internalStreamReader

fallback 经常需要把普通值包装成 stream：

```go
func streamFromItems(items ...any) streamReader {
    return &internalStreamReader{items: items}
}
```

`internalStreamReader` 就是一个很简单的 slice reader：

```go
type internalStreamReader struct {
    items []any
    pos   int
}
```

`Recv()` 每次返回一个元素，读完返回 `io.EOF`。

所以当前复刻版的 fallback stream 是非常轻量的，不是复杂异步 token channel。

## 9. Pipe stream：当前复刻版的流原语

`compose/stream.go` 另外提供了 pipe 风格的流：

```go
type PipeStreamReader[T any] interface {
    Recv() (T, bool)
    Close()
}

type PipeStreamWriter[T any] interface {
    Send(T) error
    Close()
}
```

它用 channel 实现：

```go
type stream[T any] struct {
    ch   chan T
    done chan struct{}
    mu     sync.Mutex
    closed bool
}
```

创建：

```go
sr, sw := NewPipe[int](0)
```

写：

```go
sw.Send(1)
sw.Send(2)
sw.Close()
```

读：

```go
for {
    v, ok := sr.Recv()
    if !ok { break }
}
```

注意这套 `PipeStreamReader` 的 `Recv` 返回 `(T, bool)`，和 `Runnable` 的 `StreamReader[T]` 返回 `(T, error)` 不一样。

它更像当前复刻版里用于测试和内部工具的流原语。

## 10. Copy / Merge / Concat

`stream.go` 提供三个重要工具。

### 10.1 Copy

```go
func Copy[T any](parent PipeStreamReader[T], n int) []PipeStreamReader[T]
```

当前复刻版的 Copy 很直接：

1. 先 `drainAll(parent)` 把 parent 读完。
2. 为每个 child 复制一份 slice。
3. 每个 child 都从自己的 slice 读。

这和原版 Eino 的懒复制共享 buffer 不同。

优点：简单，容易理解。

代价：必须先把 parent 全部读完，不适合真正大流式场景。

测试：

- `TestCopySameData`
- `TestCopyIndependentChildren`
- `TestCopyZeroChildren`

### 10.2 Merge

```go
func Merge[T any](readers ...PipeStreamReader[T]) PipeStreamReader[T]
```

它会启动 goroutine，把多个 reader 的数据写到同一个 writer。

注意：多个 reader 并发写入，顺序不保证稳定。测试 `TestMerge` 也只是检查元素都出现了。

### 10.3 Concat

```go
func Concat[T any](readers ...PipeStreamReader[T]) PipeStreamReader[T]
```

它会把多个 reader 读完，收集所有 chunks，然后输出一个最终值。

如果注册了 concat 函数：

```go
RegisterConcatFunc(func(chunks []string) string {
    return strings.Join(chunks, "")
})
```

就用注册函数合并。

如果没有注册函数，fallback 是最后一个 chunk：

```go
sw.Send(allItems[len(allItems)-1])
```

测试：

- `TestConcatFallbackLastChunk`
- `TestConcatRegisteredFunction`
- `TestRegisterConcatFuncOverwrite`

## 11. Graph Runnable 的一个重要边界

这是 Chapter 03 在当前复刻版里最需要讲清楚的一点。

`runner.toComposableRunnable()` 只设置了 `i`：

```go
func (r *runner) toComposableRunnable() *composableRunnable {
    return &composableRunnable{
        i: func(ctx context.Context, input any) (any, error) {
            return r.run(ctx, input)
        },
    }
}
```

也就是说，编译后的 Graph 原生只实现 Invoke。

那为什么 `graphRunnable` 还有 `Stream/Collect/Transform`？

因为 `graphRunnable.Stream` 调的是：

```go
sr, err := gr.cr.stream(ctx, input)
```

而 `cr.stream` 如果没有原生 `s`，会 fallback 到 `Invoke`：

```go
if cr.i != nil {
    out, err := cr.i(ctx, input)
    return streamFromItems(out), nil
}
```

所以当前复刻版的 Graph Stream 是：

```text
先完整执行整张图 Invoke
再把最终输出包装成单元素 stream
```

这不是逐节点、token-by-token 的真正图级流式调度。

这个边界很重要。学习当前 repo 时不要误以为 Graph runtime 已经完整支持节点级 streaming propagation。

## 12. Callback 解决什么问题

Callback 是横切逻辑。

你不希望每个节点都手写：

```go
logStart()
out, err := realWork()
if err != nil { logError(err) }
logEnd(out)
```

这会污染业务逻辑。

Callback 的目标是：

```text
业务节点只写业务；
运行时统一在节点执行前后触发观测逻辑。
```

典型用途：

- tracing
- metrics
- logging
- debug
- token 统计
- error capture

## 13. Callback 的核心类型

看 `compose/callbacks.go`。

### 13.1 RunInfo

```go
type RunInfo struct {
    Name      string
    Type      string
    Component ComponentType
}
```

在当前复刻版里，节点执行时会构造：

```go
ri := &RunInfo{
    Name:      tt.call.nodeInfo.Name,
    Type:      string(tt.call.nodeInfo.Component),
    Component: tt.call.nodeInfo.Component,
}
```

它告诉 callback：

- 当前节点名是什么。
- 当前组件类型是什么。
- 当前 component 分类是什么。

### 13.2 Timing

```go
const (
    TimingOnStart
    TimingOnEnd
    TimingOnError
    TimingOnStartWithStreamInput
    TimingOnEndWithStreamOutput
)
```

这五个 timing 分别表示：

- 普通输入开始前。
- 普通输出成功后。
- 执行错误后。
- 流式输入开始前。
- 流式输出成功后。

### 13.3 Handler

```go
type Handler struct {
    OnStart                OnStartFn
    OnEnd                  OnEndFn
    OnError                OnErrorFn
    OnStartWithStreamInput OnStartWithStreamInputFn
    OnEndWithStreamOutput  OnEndWithStreamOutputFn
}
```

你只填自己关心的函数即可。

## 14. 当前 Graph 节点 callback 是怎么触发的

在 `compose/graph_manager.go` 的 `taskManager.submit` 中：

```go
actionFn := tt.call.action.invoke
if len(tt.call.callbacks) > 0 && tt.call.nodeInfo != nil {
    ri := &RunInfo{...}
    cw := NewCallbackWrapper(ri, tt.call.callbacks)
    actionFn = cw.Invoke(actionFn)
}

output, err := actionFn(tt.ctx, input)
```

这说明当前图节点执行 callback 的主线是：

```text
节点 ready
-> 创建 task
-> 如果节点配置了 callbacks，用 CallbackWrapper 包住 Invoke
-> 执行业务 action
-> wrapper 触发 OnStart / OnEnd / OnError
```

注意：图节点这里包装的是 `Invoke`。这和上面讲的 Graph 原生只挂 `runner.run` 到 Invoke 是一致的。

## 15. CallbackWrapper.Invoke 的执行顺序

`CallbackWrapper.Invoke`：

```go
for idx, h := range cw.handlers {
    if h.OnStart != nil {
        handlerCtxs[idx] = h.OnStart(ctx, cw.info, input)
    } else {
        handlerCtxs[idx] = ctx
    }
}

output, err = i(ctx, input)
if err != nil {
    for idx, h := range cw.handlers {
        if h.OnError != nil {
            h.OnError(handlerCtxs[idx], cw.info, err)
        }
    }
    return nil, err
}

for idx, h := range cw.handlers {
    if h.OnEnd != nil {
        h.OnEnd(handlerCtxs[idx], cw.info, output)
    }
}
```

顺序：

```text
handler1.OnStart
handler2.OnStart
...
real invoke
if error:
    handler1.OnError
    handler2.OnError
else:
    handler1.OnEnd
    handler2.OnEnd
```

每个 handler 有自己的 `handlerCtxs[idx]`。

这意味着 handler A 在 OnStart 返回的新 context，不会传给 handler B。

这是设计上的隔离。

## 16. CallbackWrapper 的 stream callback

当前复刻版也有 `Stream/Collect/Transform` 的 callback wrapper。

例如 `Stream`：

```go
output, err = s(ctx, input)
...
cw.dispatchOnEndWithStreamOutput(handlerCtxs, output)
```

`dispatchOnEndWithStreamOutput` 会：

```go
copies := output.Copy(countStreamHandlers(...))
...
h.OnEndWithStreamOutput(handlerCtxs[idx], cw.info, copies[copyIdx])
```

这里使用的是 `CbStreamReader`，不是 `Runnable` 的 `StreamReader[T]`。

`CbStreamReader.Copy` 的当前实现是复制 slice：

```go
d := make([]any, len(sr.data))
copy(d, sr.data)
```

所以 stream callback 读自己的副本，不会消耗原 stream。

同样，Collect/Transform 会对输入流做 `OnStartWithStreamInput`。

## 17. TimingChecker 的作用

`CallbackWrapper.TimingChecker` 会统计 handlers 需要哪些 timing：

```go
needed |= h.neededTimings()
```

如果没有任何 handler 关心 stream input：

```go
if !checker(TimingOnStartWithStreamInput) {
    return handlerCtxs
}
```

就不会复制 stream。

这对性能很重要。因为 stream copy 可能要把整个流读完或复制数据。

测试：

- `TestTimingCheckerSkipsStreamCopy`
- `TestTimingCheckerSignalsStreamCopyNeeded`

## 18. ChatModelComponent 的例子

`ChatModelComponent` 是理解 Runnable fallback 的好例子。

当前复刻版的 fake chat model 支持：

- `Generate`：普通生成。
- `Stream`：流式生成。

`ChatModelComponent` 会把它包装成 runnable，使它能支持：

- Invoke
- Stream
- Collect fallback
- Transform fallback

相关测试：

- `TestChatModelComponentStream`
- `TestChatModelComponentStreamToInvokeFallback`
- `TestChatModelComponentCollectFallback`
- `TestChatModelComponentTransformFallback`
- `TestChatModelComponentCallbackInvoke`
- `TestChatModelComponentCallbackError`
- `TestChatModelComponentCallbackStreamViaWrapper`

学习这部分时，重点不是 chat model 本身，而是看组件方法如何落到 Runnable 四模式上。

## 19. 常见误解点

### 误解 1：Runnable 四种模式都必须由组件原生实现

不需要。

组件可以只实现其中一种或几种，`composableRunnable` 会尽量 fallback。

但 fallback 不等于原生能力。例如 Invoke fallback 出来的 Stream 只是单元素 stream。

### 误解 2：Stream fallback 一定会拼接字符串

不会。

当前 `recvAll + collected` 的规则是：

- 0 个 chunk -> nil
- 1 个 chunk -> 该 chunk
- 多个 chunk -> `[]any`

如果你需要拼字符串，要用 `Concat` 或注册 concat 函数的相关机制。

### 误解 3：Graph.Stream 是真正的图级流式执行

当前复刻版不是。

Graph 编译后的 `composableRunnable` 只设置 `Invoke`。Graph.Stream 是 `Invoke` 完整跑完后包装单元素 stream。

### 误解 4：PipeStreamReader 和 Runnable StreamReader 是同一个接口

不是。

`Runnable` 使用：

```go
Recv() (T, error)
```

`PipeStreamReader` 使用：

```go
Recv() (T, bool)
```

两者在当前复刻版里用途不同。

### 误解 5：Copy 是懒复制、边读边分发

当前复刻版不是。

`Copy` 会先 drain parent，再给每个 child 复制 slice。

这很适合教学和测试，但不是大规模 token stream 的理想实现。

### 误解 6：多个 callback handler 的 context 会串起来

不会。

每个 handler 有自己的 context 链：

```text
handler[i].OnStart 返回的 ctx -> handler[i].OnEnd/OnError
```

handler A 的 context 不会传给 handler B。

### 误解 7：OnEndWithStreamOutput 可以直接消费原始输出流

当前复刻版给 handler 的是 `CbStreamReader` 副本。

handler 读副本，不影响真实 consumer。

### 误解 8：OnError 后还会 OnEnd

不会。

`CallbackWrapper.Invoke` 中，如果 real invoke 返回 error，只触发 `OnError`，然后直接返回。

### 误解 9：没有 callback 时也有很多包装成本

图节点执行时只有 `len(tt.call.callbacks) > 0` 才会创建 `CallbackWrapper` 包装 action。

没有 handler 时，基本走原 action。

### 误解 10：当前复刻版已经完整覆盖原版 Eino callback 能力

没有。

当前复刻版有清晰的 callback wrapper 和节点级 callback，但没有完整复刻原版的全局 manager、路径级 handler 注入、复杂 stream reader 生命周期等。

## 20. 建议源码阅读顺序

第一遍看主线：

1. `runnable.go`
   - `Runnable`
   - `composableRunnable`
   - `invoke/stream/collect/transform`
   - 四种 Lambda 构造器

2. `runnable_test.go`
   - `TestInvokeOnlyStreamFallback`
   - `TestStreamOnlyInvokeFallbackWithConcat`
   - `TestAllFourModesNative`
   - fallback priority tests

3. `stream.go`
   - `NewPipe`
   - `Copy`
   - `Merge`
   - `Concat`

4. `stream_test.go`
   - pipe send/recv
   - copy independent children
   - merge order
   - concat fallback and registered function

5. `callbacks.go`
   - `RunInfo`
   - `Handler`
   - `CallbackWrapper`
   - `Invoke/Stream/Collect/Transform`

6. `callbacks_test.go`
   - invoke success/error
   - stream output callback
   - collect/transform callback
   - timing checker

7. `graph_manager.go`
   - `taskManager.submit`
   - 看 Graph 节点 callback 如何接入 Invoke

第二遍再看组件：

1. `chatmodel.go`
2. `chatmodel_test.go`
3. `retriever.go`
4. `retriever_test.go`

## 21. 练习题

### 练习 1：只实现 Invoke，然后调用 Stream

目标：理解 Invoke -> Stream fallback。

要求：

1. 创建 `InvokableLambda[string,string]`。
2. `GetRunnable().stream(ctx, "world")`。
3. 读取 stream。
4. 期望只有一个 chunk。

思考：

- 这是真正 token stream 吗？
- 第二次 `Recv` 应该返回什么？

### 练习 2：只实现 Stream，然后调用 Invoke

目标：理解 Stream -> Invoke fallback。

要求：

1. 创建 `StreamableLambda`，输入 `"a,b,c"`，按逗号分成三个 chunk。
2. 调 `cr.invoke(ctx, "a,b,c")`。
3. 观察输出是单值还是 `[]any`。

思考：

- 为什么不是字符串 `"abc"`？
- 当前 `collected` 的规则是什么？

### 练习 3：实现一个 CollectableLambda

目标：理解流输入转普通输出。

要求：

1. 输入 stream 是多个 string。
2. CollectableLambda 把它们 join 成一个 string。
3. 调 native collect。
4. 再尝试调用 invoke，看 fallback 如何把普通 input 包成 stream。

### 练习 4：实现 TransformableLambda

目标：理解流输入转流输出。

要求：

1. 输入 stream 是多个 string。
2. TransformableLambda 给每个 chunk 加前缀 `"T:"`。
3. 调 transform 并逐个读取输出。
4. 再调 invoke，观察 Transform -> Invoke fallback。

### 练习 5：Pipe stream 的 Copy

目标：理解当前复刻版 Copy 是 eager copy。

要求：

1. `PipeStreamReaderFromSlice([]int{1,2,3})`。
2. `Copy(sr, 2)`。
3. 关闭第一个 child。
4. 第二个 child 仍然能读到完整数据。

思考：

- parent 在 Copy 后还剩数据吗？
- 这种实现适不适合无限流？

### 练习 6：Merge 多个 reader

目标：理解 merge 的并发和顺序。

要求：

1. 创建三个 reader。
2. Merge 后读所有值。
3. 用 map 检查值都出现，而不是检查严格顺序。

思考：

- 为什么不应该依赖 merge 输出顺序？

### 练习 7：Concat 注册函数

目标：理解 concat fallback。

要求：

1. 先不注册 concat，Concat 多个 int reader，看输出是不是最后一个 chunk。
2. 注册 string concat 函数。
3. Concat string chunks，期望拼接字符串。

思考：

- 注册函数应在什么时候做？
- 多次注册同一类型会怎样？

### 练习 8：Callback Invoke 成功路径

目标：理解 OnStart/OnEnd。

要求：

1. 创建一个 Handler，OnStart 记录 input，OnEnd 记录 output。
2. 用 `NewCallbackWrapper` 包住一个 invoke 函数。
3. 执行成功。
4. 检查事件顺序。

### 练习 9：Callback Invoke 错误路径

目标：理解 OnError。

要求：

1. 业务 invoke 返回 error。
2. Handler 同时配置 OnEnd 和 OnError。
3. 检查只触发 OnStart + OnError，不触发 OnEnd。

### 练习 10：多个 handler 的 context 隔离

目标：理解 handler context 不串联。

要求：

1. handler A 在 OnStart 里 `context.WithValue`。
2. handler B 在 OnEnd 里尝试读取。
3. 验证 B 读不到 A 的值。

思考：

- 如果确实需要共享状态，应该怎么做？

### 练习 11：Graph.Stream 边界验证

目标：理解当前复刻版 Graph.Stream 是 fallback。

要求：

1. 建一个简单 Graph：START -> upper -> END。
2. Compile 后调用 `r.Stream(ctx, "hello")`。
3. 读取 stream。
4. 验证只有最终 `"HELLO"` 一个 chunk。

思考：

- 如果想做真正逐 token 流式图运行，当前 runner 还缺什么？

## 22. 自测问题

读完后，你应该能回答：

1. Runnable 四种执行形态分别是什么？
2. Invoke-only 组件被 Stream 调用时会发生什么？
3. Stream-only 组件被 Invoke 调用时会发生什么？
4. `collected` 对 0/1/N 个 chunk 分别返回什么？
5. `typedStreamWrapper` 和 `untypedStreamWrapper` 解决什么问题？
6. 当前 Graph.Stream 是否是真正图级流式执行？
7. `PipeStreamReader` 和 `Runnable.StreamReader` 有什么区别？
8. 当前 `Copy` 是懒复制还是 eager copy？
9. `Concat` 未注册函数时默认返回什么？
10. Callback 的 `RunInfo` 包含什么？
11. `OnStart`、`OnEnd`、`OnError` 的触发顺序是什么？
12. 多个 handler 的 context 是否会互相传递？
13. stream callback 为什么需要 copy？
14. 当前复刻版 callback 和原版 Eino callback 的主要差异是什么？

## 23. 一句话总结

Chapter 03 的核心是把“组件怎么执行”统一起来：

```text
不同组件可以只实现 Invoke / Stream / Collect / Transform 的一部分；
composableRunnable 通过 fallback 把它们补齐成统一 Runnable；
Stream 提供流式数据原语；
CallbackWrapper 在执行前后插入观测逻辑；
Graph / Workflow / Chain 最终都通过 Runnable 暴露统一执行入口。
```

但学习当前复刻版时一定要记住边界：

```text
当前 Graph 原生只执行 Invoke；
Graph.Stream / Collect / Transform 主要来自 fallback；
它不是完整的原版 Eino 图级流式调度实现。
```

