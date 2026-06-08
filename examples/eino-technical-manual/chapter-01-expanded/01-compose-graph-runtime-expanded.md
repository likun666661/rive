# Chapter 01 - Compose Graph Runtime 深度讲解

面向读者：假设你刚开始学习这个 repo，不要求你已经熟悉 Eino、LangChain、DAG 调度器或 agent runtime。本文按“先讲问题，再讲解法，再走读代码”的方式展开。

参考代码位置：

- 手册：`examples/eino-technical-manual/manual/01-compose-graph-runtime.md`
- 复刻版：`examples/eino-compose-runtime-replica-go`
- 本章重点源码：
  - `compose/generic_graph.go`
  - `compose/graph.go`
  - `compose/graph_run.go`
  - `compose/graph_manager.go`
  - `compose/dag.go`
  - `compose/pregel.go`
  - `compose/runnable.go`
  - `compose/graph_test.go`

说明：手册中有一些原版 Eino 风格的描述，例如完整类型推断、复杂 checkpoint、更多 handler manager 等。当前 repo 的 Go 复刻版是简化实现。学习这份代码时，应以当前复刻版源码为准，把手册中更复杂的部分理解为“原版 Eino 的设计方向”。

## 1. 这一章到底在解决什么问题

如果你写一个最简单的 LLM 应用，可能就是这样：

```go
prompt := buildPrompt(userInput)
answer := callModel(prompt)
return answer
```

这没什么问题。但真实 LLM 应用很快会变复杂：

```text
用户输入
  -> prompt template
  -> chat model
  -> 判断是否需要工具
  -> tool call
  -> tool result 回填
  -> chat model 再回答
  -> parser
  -> 最终输出
```

再复杂一点，还会出现：

- 并行：同时查多个 retriever，然后合并结果。
- 分支：如果问题需要外部工具就走 tool node，否则直接回答。
- 循环：agent 不断 “think -> act -> observe -> think”，直到满足停止条件。
- 流式：model 输出 token stream，不是一次性返回完整字符串。
- 回调：记录每个节点开始、结束、耗时、错误。
- checkpoint：中断后可以恢复。
- 类型转换：一个节点输出 `Message`，下一个节点可能要的是 `string` 或 struct。

如果所有这些都靠业务代码手写 `if/for/go func/channel`，会有几个问题。

第一，控制流会散落在各处。每个业务都自己写“先调 A，再调 B，如果条件成立走 C”，最后无法统一做拓扑检查、可视化、调试和复用。

第二，节点能力不一致。一个节点可能只支持普通调用 `Invoke`，另一个节点支持流式 `Stream`，还有节点可能接收流式输入。上层不应该每次都手写适配逻辑。

第三，依赖关系变得难维护。比如 fan-in 节点要等两个前驱都完成才能执行；agent loop 节点则可能收到任意一个前驱消息就能继续执行。这两类触发语义完全不同。

第四，横切能力不好插入。callback、event log、checkpoint、interrupt 这些能力如果塞进每个业务节点，会污染节点本身。

所以 Compose Graph Runtime 的核心问题是：

```text
如何让开发者先声明一张“组件执行图”，再由统一运行时安全、可观测、可复用地执行这张图？
```

## 2. Eino 的基本解法：构建图、编译图、执行 Runnable

本章最重要的抽象分界是：

```text
Graph construction -> Compile -> Runnable execution
```

翻译成更直白的话：

- `Graph` 是你搭积木的阶段。
- `Compile` 是把积木图冻结、检查、转换成运行器的阶段。
- `Runnable` 是真正能被调用的执行单元。

这和普通函数调用的区别很大。普通函数调用是“写到哪里执行到哪里”；Graph Runtime 是“先描述结构，再交给引擎执行”。

你可以把它类比为 SQL：

```text
SQL 文本      -> 查询计划编译        -> 查询引擎执行
Graph builder -> Graph.Compile() -> runner.run()
```

开发者写的是结构；运行时关心的是依赖、调度、并发、错误和结果收集。

## 3. 初学者先记住的 6 个概念

### 3.1 Node

Node 是图里的业务节点。当前复刻版中最常见的是 Lambda node：

```go
g.AddLambdaNode("upper", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
    return strings.ToUpper(in), nil
}))
```

你可以先把 node 理解成“带名字的函数”。名字是 `"upper"`，函数是 `InvokableLambda(...)`。

### 3.2 Edge

Edge 是节点之间的连线：

```go
g.AddEdge(compose.START, "upper")
g.AddEdge("upper", "reverse")
g.AddEdge("reverse", compose.END)
```

这表示：

```text
START -> upper -> reverse -> END
```

当前复刻版里的 `AddEdge` 是数据边。一个节点执行完后，它的输出会沿着数据边写给后继节点。

### 3.3 START 和 END

`START` 和 `END` 是虚拟节点，不是业务函数。

- `START`：外部输入从这里进入图。
- `END`：最终输出从这里返回调用者。

没有 `START` 到某个节点的边，图就不知道先执行谁；没有某个节点到 `END` 的边，图就不知道最终结果在哪里。

### 3.4 Compile

`Compile` 把可修改的图变成可执行的 runtime。

编译后，图会被锁住。你再 `AddEdge` 或 `AddLambdaNode` 会得到 `ErrGraphCompiled`。

这个锁很重要。否则运行器执行时拓扑还能被改，调度逻辑会非常难保证正确。

### 3.5 Runnable

`Runnable[I,O]` 是最终执行接口，定义在 `compose/runnable.go`：

```go
type Runnable[I, O any] interface {
    Invoke(ctx context.Context, input I) (output O, err error)
    Stream(ctx context.Context, input I) (output StreamReader[O], err error)
    Collect(ctx context.Context, input StreamReader[I]) (output O, err error)
    Transform(ctx context.Context, input StreamReader[I]) (output StreamReader[O], err error)
}
```

Chapter 01 主要看 `Invoke` 就够了。后面章节会更细讲 stream。

### 3.6 Channel

Channel 是 runtime 内部用于判断“节点是否 ready”的结构。

你不要把这里的 channel 简单理解成 Go 原生 `chan`。当前复刻版里它是一个接口：

```go
type channel interface {
    reportValues(nodeKey string, value any)
    reportDependency(nodeKey string)
    reportSkip(nodeKey string) bool
    get() (any, bool, error)
    setMergeConfig(fn func(map[string]any) (any, error))
}
```

每个节点都有一个 channel。前驱节点执行完成后，会向后继节点的 channel 上报值和依赖状态。后继节点的 channel 决定自己是否 ready。

## 4. 第一段最小例子：线性 DAG

看 `cmd/example/main.go` 的 `example1_DAGBasic` 或 `graph_test.go` 的 `TestDAGLinearExecution`。

简化后是这样：

```go
g := compose.NewGraph[string, string]()

g.AddLambdaNode("upper", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
    return strings.ToUpper(in), nil
}))

g.AddLambdaNode("reverse", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
    // 反转字符串
    return reversed, nil
}))

g.AddEdge(compose.START, "upper")
g.AddEdge("upper", "reverse")
g.AddEdge("reverse", compose.END)

r, err := g.Compile(context.Background(),
    compose.WithGraphName("dag_linear"),
    compose.WithNodeTriggerMode(compose.AllPredecessor),
)

result, err := r.Invoke(context.Background(), "abc")
```

执行过程是：

```text
外部 input = "abc"
START 把 "abc" 写给 upper
upper ready，执行，输出 "ABC"
upper 把 "ABC" 写给 reverse
reverse ready，执行，输出 "CBA"
reverse 把 "CBA" 写给 END
runner 从 END 取到结果并返回
```

这里的关键不是大写或反转，而是 Graph Runtime 完成了“按边调度节点”的工作。

## 5. 代码走读一：Graph 是如何保存拓扑的

先看 `compose/generic_graph.go`：

```go
type Graph[I, O any] struct {
    g      *graph
    input  I
    output O
}
```

用户看到的是泛型 `Graph[I,O]`，但真正工作的是内部 `*graph`。

再看 `compose/graph.go`：

```go
type graph struct {
    nodes        map[string]*graphNode
    controlEdges map[string][]string
    dataEdges    map[string][]string
    branches     map[string][]*GraphBranch
    graphName    string
    compiled     bool

    chanSubscribeTo     map[string]*chanCall
    dataPredecessors    map[string][]string
    controlPredecessors map[string][]string
    successors          map[string][]string
    startNodes          []string
    endNodes            []string
}
```

你可以分两组理解。

构建期字段：

- `nodes`：有哪些节点。
- `dataEdges`：数据边，从谁到谁。
- `controlEdges`：控制边，从谁触发谁。
- `branches`：条件分支。
- `compiled`：是否已经编译。

运行期准备字段：

- `chanSubscribeTo`：每个节点对应的运行时调用包装。
- `dataPredecessors`：每个节点有哪些数据前驱。
- `controlPredecessors`：每个节点有哪些控制前驱。
- `successors`：每个节点有哪些后继。
- `startNodes` / `endNodes`：图入口和出口。

### AddNode 做了什么

`AddLambdaNode` 本质上是把 lambda 包装成 `graphNode` 放进 `nodes`：

```go
g.nodes[key] = &graphNode{
    name: key,
    cr:   lambda.GetRunnable(),
    info: &GraphNodeInfo{Name: key, Component: lambda.GetComponentType()},
}
```

这一步还没有执行 lambda，只是登记它。

### AddEdge 做了什么

`AddEdge(from, to)` 先检查图是否已编译，再检查节点是否存在，最后把边写入 `dataEdges`：

```go
g.dataEdges[from] = append(g.dataEdges[from], to)
```

这一步也没有执行任何节点。它只是记录拓扑。

## 6. 代码走读二：Compile 做了什么

`Graph.Compile` 在 `generic_graph.go`，核心流程是：

```go
o := newNodeCompileOptions(opts...)
gg.g.graphName = o.graphName
gg.g.genLocalState = o.genLocalState
gg.g.graphInfo = newGraphInfo(...)

r, err := gg.g.compile(ctx)
...
return &graphRunnable[I, O]{cr: cr, runner: r}, nil
```

也就是说：

1. 解析编译选项。
2. 准备 `GraphInfo`。
3. 调内部 `g.compile(ctx)` 得到 `runner`。
4. 把 runner 包装成泛型 `Runnable[I,O]`。

内部 `g.compile(ctx)` 更关键。

### 6.1 选择运行模式

在 `graph.go` 里：

```go
runT := runTypePregel
if g.triggerMode == AllPredecessor {
    runT = runTypeDAG
}
if g.graphInfo != nil && g.graphInfo.TriggerMode == AllPredecessor {
    runT = runTypeDAG
}
if compileOpts != nil && compileOpts.isDAG() {
    runT = runTypeDAG
}
```

默认是 Pregel，也就是 `AnyPredecessor`。如果指定 `AllPredecessor`，就用 DAG。

初学者可以先这样记：

- 普通无环流程：用 `AllPredecessor`。
- agent loop 或循环图：用 `AnyPredecessor`。

### 6.2 把节点编译成 chanCall

每个节点都会变成一个 `chanCall`：

```go
g.chanSubscribeTo[key] = &chanCall{
    nodeKey:       key,
    action:        cr,
    writeTo:       make(map[string]bool),
    controls:      make(map[string]bool),
    fieldMappings: make(map[string][]*FieldMapping),
    callbacks:     node.handlers,
    nodeInfo:      node.info,
}
```

`chanCall` 是运行时真正需要的节点包装。它包含：

- `action`：实际要执行的 runnable。
- `writeTo`：执行完成后，输出要写给哪些后继。
- `controls`：执行完成后，要通知哪些控制依赖。
- `fieldMappings`：输出写给后继前是否要映射字段。
- `callbacks`：节点级回调。

也就是说，Compile 会把“用户视角的 node”翻译成“运行器视角的 chanCall”。

### 6.3 从正向边推导反向依赖

用户声明的是：

```text
A -> B
```

但运行时判断 B 是否 ready 时，更关心：

```text
B 有哪些前驱？
```

所以 compile 会把 `dataEdges` 反向整理成 `dataPredecessors`：

```go
for from, targets := range g.dataEdges {
    for _, to := range targets {
        fromCall.writeTo[to] = true
        g.dataPredecessors[to] = append(g.dataPredecessors[to], from)
        g.successors[from] = append(g.successors[from], to)
    }
}
```

这一步很重要。很多调度器都是这样：用户声明正向边，运行器内部同时维护反向依赖。

### 6.4 找入口和出口

所有从 `START` 指向的节点都是入口：

```go
for startTarget := range g.chanSubscribeTo[START].writeTo {
    g.startNodes = append(g.startNodes, startTarget)
}
```

所有指向 `END` 的节点都是出口：

```go
if _, ok := call.writeTo[END]; ok {
    g.endNodes = append(g.endNodes, nodeKey)
}
```

如果没有入口或出口，编译失败。

### 6.5 DAG 模式检查环

如果是 DAG：

```go
if runT == runTypeDAG {
    if err := g.checkDAGCycles(); err != nil {
        return nil, err
    }
}
```

`checkDAGCycles` 用的是 Kahn 算法。直觉上是：

1. 计算每个节点入度。
2. 入度为 0 的节点先进队列。
3. 不断移除节点和它的出边。
4. 如果最后还有节点没被移除，说明有环。

DAG 的 D 就是 directed，A 就是 acyclic，无环。如果有环，就不应该叫 DAG。

### 6.6 创建 runner 并锁图

最后创建 runner：

```go
r := &runner{
    chanSubscribeTo:     g.chanSubscribeTo,
    successors:          g.successors,
    dataPredecessors:    g.dataPredecessors,
    controlPredecessors: g.controlPredecessors,
    inputChannels:       g.chanSubscribeTo[START],
    startNodes:          g.startNodes,
    endNodes:            g.endNodes,
    dag:                 isDAG,
    pregel:              !isDAG,
    eager:               isEager,
    maxSteps:            maxSt,
    graphName:           g.graphName,
    graphInfo:           g.graphInfo,
}

g.compiled = true
```

这里就是编译边界。

编译前，你拥有的是可修改的 Graph。

编译后，你得到的是可执行的 runner/Runnable。

## 7. 代码走读三：runner.run 如何执行图

`runner.run` 在 `compose/graph_run.go`。它是 Chapter 01 的运行时核心。

### 7.1 初始化 channel

```go
cm := newChannelManager()
r.initChannels(cm)
```

`initChannels` 会给每个节点创建 channel：

```go
if r.dag {
    dc := newDAGChannel(controlPreds, dataPreds)
    cm.addChannel(nodeKey, dc)
} else {
    pc := newPregelChannel()
    cm.addChannel(nodeKey, pc)
}
```

注意：同一套 runner 主循环，底层 channel 不同，行为就不同。

这是一个很漂亮的设计点：DAG 和 Pregel 的区别被封装在 channel 实现里，而不是把主循环复制两份。

### 7.2 把输入路由到 start nodes

```go
r.routeInputToStartNodes(cm, input)
```

如果你的图是：

```text
START -> upper
```

那么外部 input 会写入 `upper` 的 channel。

### 7.3 主循环：找 ready 节点

```go
readyNodes := cm.getReadyChannels("")
```

`channelManager` 会遍历每个节点的 channel，调用 `ch.get()`。

如果某个 channel 返回 `(value, true, nil)`，说明这个节点 ready，可以执行。

### 7.4 创建 task 并执行

```go
tasks := r.createTasks(ctx, readyNodes)
tm.submit(ctx, tasks)
completedTasks := tm.wait()
```

`taskManager.submit` 会启动 goroutine 执行每个 task：

```go
output, err := actionFn(tt.ctx, input)
tt.output = output
tt.err = err
```

所以多个 ready 节点可以并发执行。比如：

```text
START -> A
START -> B
A -> merger
B -> merger
```

A 和 B 都收到 START 输入后，可以同一轮并发执行。

### 7.5 完成后写给后继节点

节点执行完成后，`resolveCompletedTasks` 会把输出写给后继：

```go
cm.updateValues(t.nodeKey, output, writeTo)
cm.updateDependencies(t.nodeKey, cc.controls)
```

如果当前节点是 `upper`，后继是 `reverse`，那就等于：

```text
upper 的输出写入 reverse 的 channel
```

下一轮循环里，`reverse` 的 channel 可能就 ready 了。

### 7.6 从 END 取结果

每轮结束后，runner 会检查 END channel：

```go
if endVal, ok := cm.getEndChannel(); ok {
    lastEndValue = endVal
    lastEndReady = true
    hasResult = true
}
```

没有 ready 节点后，如果 END 有值，就返回最终结果。

## 8. DAG Channel：AllPredecessor 到底是什么意思

`AllPredecessor` 的意思是：所有前驱都满足条件后，节点才能执行。

看 `compose/dag.go`：

```go
type dagChannel struct {
    values              map[string]any
    controlPredecessors map[string]dependencyState
    dataPredecessors    map[string]bool
}
```

它维护两类前驱状态：

- `dataPredecessors`：哪些前驱必须提供值。
- `controlPredecessors`：哪些前驱必须完成控制依赖。

`get()` 的核心判断：

```go
if !allControlReady || !allDataReported {
    return nil, false, nil
}
```

所以 DAG fan-in 可以工作：

```text
START -> upper   -> merger
START -> reverse -> merger
```

`merger` 的 data predecessors 是 `upper` 和 `reverse`。

只有当两个都 report value 后，`merger` 才 ready。

如果只有 `upper` 完成，`merger` 还不能执行。

### DAG 的输出合并

如果 fan-in 有多个输入，当前复刻版默认会把多个前驱值组成 map：

```go
if len(dc.values) > 1 {
    result := make(map[string]any)
    for k, v := range dc.values {
        result[k] = v
    }
    return result, true, nil
}
```

所以 merger 节点收到的可能是：

```go
map[string]any{
    "upper": "HELLO",
    "reverse": "olleh",
}
```

这也是一个常见误解点：fan-in 节点不一定收到单个值。多个前驱时，输入可能是 map，除非你配置了 merge function 或 field mapping。

## 9. Pregel Channel：AnyPredecessor 到底是什么意思

`AnyPredecessor` 的意思是：任意前驱给了值，节点就可以执行。

看 `compose/pregel.go`：

```go
type pregelChannel struct {
    values map[string]any
}
```

它比 DAG channel 简单很多。`get()` 里：

```go
if len(pc.values) == 0 {
    return nil, false, nil
}
...
pc.values = make(map[string]any)
return result, true, nil
```

只要有一个 value，节点就 ready。

这适合循环图：

```text
START -> agent
agent -> tool
tool -> agent
agent -> END
```

agent 收到 tool 的结果后可以继续跑；tool 收到 agent 的 tool call 后可以跑。这里不是严格的一次性 DAG，而是消息推动的循环。

Pregel 模式允许环，所以需要 `maxSteps`：

```go
if r.pregel && r.runStepCount > r.maxSteps {
    return nil, fmt.Errorf("%w: step %d exceeds max %d", ErrExceedMaxSteps, r.runStepCount, r.maxSteps)
}
```

否则一个自环节点可能永远运行。

## 10. DAG 和 Pregel 怎么选

先用这个规则：

```text
如果你的图是确定流程、无环、要等全部依赖完成，选 AllPredecessor / DAG。
如果你的图是循环、agent loop、消息驱动，选 AnyPredecessor / Pregel。
```

对比表：

| 维度 | DAG / AllPredecessor | Pregel / AnyPredecessor |
|---|---|---|
| 节点触发 | 所有前驱满足后触发 | 任一前驱有值就触发 |
| 是否允许环 | 不允许 | 允许 |
| 适合场景 | workflow、chain、fan-in | agent loop、状态机、多轮推理 |
| 风险 | 环会编译失败 | 可能无限循环 |
| 保护机制 | Kahn 环检测 | maxSteps |
| fan-in 行为 | 等所有输入 | 有输入就动 |

初学阶段，建议大部分例子都先用 `AllPredecessor`。只有你明确需要循环时，再用 `AnyPredecessor`。

## 11. Runnable 适配：为什么有 Invoke/Stream/Collect/Transform

这节手册也提到四种执行形态。当前复刻版定义在 `compose/runnable.go`：

- `Invoke`：普通输入 -> 普通输出。
- `Stream`：普通输入 -> 流式输出。
- `Collect`：流式输入 -> 普通输出。
- `Transform`：流式输入 -> 流式输出。

为什么需要这么多？因为 LLM 应用天然会遇到流：

- 模型 token-by-token 输出：`Stream`
- 把多个 chunk 收集成完整结果：`Collect`
- 对流式 token 做实时转换：`Transform`

当前复刻版里的 `composableRunnable` 会做一些 fallback。例如：

```go
func (cr *composableRunnable) stream(ctx context.Context, input any) (any, error) {
    if cr.s != nil {
        return cr.s(ctx, input)
    }
    if cr.t != nil {
        return cr.t(ctx, streamFromItems(input))
    }
    if cr.i != nil {
        out, err := cr.i(ctx, input)
        return streamFromItems(out), nil
    }
    ...
}
```

意思是：如果一个组件没有原生 `Stream`，但有 `Invoke`，runtime 可以先 `Invoke`，再把结果包装成只有一个元素的 stream。

这让上层调用更统一。你不需要每次都问“这个节点到底支不支持 stream”，runtime 会尽量适配。

## 12. GraphInfo 和可观测性

`GraphInfo` 在 `compose/introspect.go`：

```go
type GraphInfo struct {
    Name        string
    InputType   string
    OutputType  string
    Nodes       []GraphNodeInfo
    Edges       []GraphEdgeInfo
    TriggerMode NodeTriggerMode
    DAGMode     bool
    PregelMode  bool
    MaxSteps    int
    NumNodes    int
    NumEdges    int
}
```

它是编译后给外界看的图信息。比如：

```go
info := g.GetGraphInfo()
fmt.Println(info.Name)
fmt.Println(info.Nodes)
fmt.Println(info.Edges)
```

为什么这重要？

因为 LLM 应用一旦复杂，你需要知道：

- 图里有哪些节点？
- 哪些节点连到哪些节点？
- 当前图是 DAG 还是 Pregel？
- 最大运行步数是多少？

这对调试、可视化、审计都很关键。

## 13. 容易误解点详解

### 误解 1：AddEdge 时节点就会执行

不会。

`AddEdge` 只是记录拓扑：

```go
g.dataEdges[from] = append(g.dataEdges[from], to)
```

真正执行发生在：

```go
r.Invoke(ctx, input)
```

更准确地说，发生在 `runner.run` 的 task submit 阶段。

### 误解 2：Graph 就是 Runnable

不是。

`Graph` 是构建器，`Runnable` 是编译产物。

你不能直接执行一个还没 compile 的 graph。必须：

```go
r, err := g.Compile(ctx)
result, err := r.Invoke(ctx, input)
```

### 误解 3：Compile 只是类型检查

不只是。

当前复刻版的 `Compile` 至少做了这些事：

- 创建 GraphInfo。
- 选择 DAG/Pregel 模式。
- 把 node 变成 chanCall。
- 构造 START/END 虚拟节点的 chanCall。
- 整理 writeTo、controls、predecessors、successors。
- 找 startNodes/endNodes。
- DAG 模式下环检测。
- 创建 runner。
- 设置 compiled lock。

类型检查只是其中一部分，而且当前复刻版没有完整实现原版 Eino 的复杂类型推断。

### 误解 4：DAG 只是“顺序执行”

不是。

DAG 可以并行。

```text
START -> A -> C
START -> B -> C
```

A 和 B 没有依赖关系，所以可以并发执行。C 等 A 和 B 都完成后执行。

DAG 的关键不是“顺序”，而是“依赖有向无环”。

### 误解 5：Pregel 比 DAG 更高级，所以都用 Pregel

不是。

Pregel 更灵活，但也更危险。因为任一前驱触发就能执行，所以循环图可能跑很多轮，甚至无限跑。

普通 workflow 用 Pregel，可能会得到和预期不同的触发行为。比如一个 fan-in 节点原本应该等两个输入都齐，但 Pregel 可能收到一个输入就开始执行。

所以不是“Pregel 更高级”，而是“适合不同问题”。

### 误解 6：fan-in 节点收到的一定是上一个节点输出类型

不一定。

如果只有一个前驱，通常收到单个值。

如果多个前驱，DAG channel 可能传给你 `map[string]any`。

所以 fan-in 的 merger 节点通常要写成能处理 map，或者使用 merge config / field mapping。

### 误解 7：START 和 END 是普通业务节点

不是。

它们没有业务 action。它们是 runtime 用来统一输入输出的虚拟节点。

`START` 的作用是把外部 input 写给真正的入口节点。

`END` 的作用是收集最终输出并返回。

### 误解 8：编译后还可以改图再运行

不能。

compile 后 `g.compiled = true`。再调用 `AddEdge/AddLambdaNode` 会报 `ErrGraphCompiled`。

如果你需要不同拓扑，创建一个新 graph。

### 误解 9：手册里的所有 Eino 能力当前复刻版都完整实现了

不是。

当前复刻版保留了核心骨架，适合学习 runtime 主线：

- Graph/Compile/Runnable
- DAG/Pregel
- runner 主循环
- channel manager
- task manager
- branch 基础结构
- runnable 四模式适配

但手册里提到的一些原版复杂能力，在当前代码里没有完全展开，比如完整的 passthrough 类型推断、复杂 checkpoint 嵌套处理、更多 callback 编译钩子。

学习时不要被这些细节干扰，先掌握主干。

## 14. 建议的源码阅读顺序

第一遍只看主干：

1. `cmd/example/main.go`
   - 看 `example1_DAGBasic`
   - 看 `example2_PregelWithMaxSteps`
   - 看 `example3_CompileBoundary`

2. `compose/generic_graph.go`
   - 看 `Graph[I,O]`
   - 看 `NewGraph`
   - 看 `Compile`
   - 看 `graphRunnable.Invoke`

3. `compose/graph.go`
   - 看 `graph` struct
   - 看 `AddLambdaNode`
   - 看 `AddEdge`
   - 看 `compile`
   - 看 `checkDAGCycles`

4. `compose/graph_run.go`
   - 看 `runner` struct
   - 看 `run`
   - 看 `initChannels`
   - 看 `routeInputToStartNodes`
   - 看 `createTasks`
   - 看 `resolveCompletedTasks`

5. `compose/dag.go` 和 `compose/pregel.go`
   - 对比两个 channel 的 `get()`

6. `compose/graph_test.go`
   - 看测试帮助你理解真实行为。

第二遍再看扩展：

1. `compose/runnable.go`
2. `compose/branch.go`
3. `compose/field_mapping.go`
4. `compose/callbacks.go`
5. `compose/checkpoint.go`
6. `compose/stream.go`

## 15. 练习题

### 练习 1：手写一个最小线性图

目标：理解 START、END、AddNode、AddEdge、Compile、Invoke 的最小闭环。

要求：

1. 创建 `NewGraph[string, string]()`。
2. 添加一个节点 `trim`，去掉字符串首尾空格。
3. 添加一个节点 `upper`，转大写。
4. 连线：`START -> trim -> upper -> END`。
5. 用 `AllPredecessor` 编译。
6. 输入 `"  hello  "`，期望输出 `"HELLO"`。

思考：

- 如果漏掉 `START -> trim` 会怎样？
- 如果漏掉 `upper -> END` 会怎样？
- 如果 compile 后再 AddEdge 会怎样？

### 练习 2：观察编译锁

目标：理解 Graph 和 Runnable 的边界。

要求：

1. 创建一个最小图并 compile。
2. compile 后调用 `AddLambdaNode("new_node", ...)`。
3. 判断错误是否是 `ErrGraphCompiled`。

参考测试：`TestCompileLockAddEdge`。

思考：

- 为什么 runtime 不允许编译后修改图？
- 如果允许修改，会给并发执行带来什么问题？

### 练习 3：fan-out + fan-in

目标：理解 DAG 并发和 fan-in。

图结构：

```text
          -> upper   -
START ->              -> merger -> END
          -> reverse -
```

要求：

1. `upper` 返回大写。
2. `reverse` 返回反转字符串。
3. `merger` 接收 `any`。
4. 如果输入是 `map[string]any`，把两个结果拼成稳定字符串。
5. 用 `AllPredecessor` 编译。

思考：

- `merger` 为什么不是收到一个 string？
- 如果改成 Pregel 模式，行为会有什么变化？

### 练习 4：DAG 环检测

目标：理解 DAG 为什么不能有环。

图结构：

```text
START -> A -> B -> A
B -> END
```

要求：

1. 用 `AllPredecessor` 编译。
2. 观察是否返回 `ErrDAGHasCycle`。

思考：

- 为什么这个图无法按“所有前驱完成”语义稳定执行？
- Kahn 算法为什么能发现它？

### 练习 5：Pregel 自环和 maxSteps

目标：理解循环图和保护机制。

图结构：

```text
START -> loop
loop -> loop
loop -> END
```

要求：

1. `loop` 每次给字符串追加 `"."`。
2. 用 `AnyPredecessor` 编译。
3. 设置 `WithMaxRunSteps(3)`。
4. 调用 `Invoke`，观察是否得到 `ErrExceedMaxSteps`。

思考：

- 为什么 Pregel 允许这个图 compile？
- 为什么运行时必须限制 maxSteps？
- 如果没有 `loop -> END`，结果会怎样？

### 练习 6：GraphInfo 自省

目标：理解 compile 后的图信息。

要求：

1. 创建三节点图。
2. compile 后调用 `g.GetGraphInfo()`。
3. 打印 `Name`、`TriggerMode`、`NumNodes`、`NumEdges`、`Nodes`、`Edges`。

思考：

- GraphInfo 对调试有什么帮助？
- 如果要做一个图可视化页面，需要 GraphInfo 的哪些字段？

### 练习 7：Runnable fallback

目标：理解 `Invoke` 和 `Stream` 的适配。

要求：

1. 创建一个只支持 `Invoke` 的 Lambda。
2. 编译成 Runnable。
3. 调用 `Stream`。
4. 读取 stream，看是否只有一个输出。

思考：

- 为什么只支持 Invoke 的组件也能 Stream？
- 这种 fallback 和真正 token stream 有什么区别？

## 16. 自测问题

读完后，你应该能回答这些问题：

1. `Graph` 和 `Runnable` 有什么区别？
2. `Compile` 为什么是一个重要边界？
3. `START` 和 `END` 的作用是什么？
4. `AllPredecessor` 和 `AnyPredecessor` 的触发语义有什么区别？
5. 为什么 DAG 模式要拒绝环？
6. 为什么 Pregel 模式需要 `maxSteps`？
7. fan-in 节点为什么可能收到 `map[string]any`？
8. `runner.run` 的主循环大概做了哪几步？
9. `channel` 在 runtime 里负责什么？
10. 为什么普通业务节点不应该自己实现 callback/checkpoint 等横切逻辑？

## 17. 一句话总结

Chapter 01 要你建立的不是某个 API 的记忆，而是一种 runtime 思维：

```text
先把 LLM 应用声明成一张图，
再通过 Compile 把图冻结成运行计划，
最后由统一 runner 根据 channel 的 ready 语义调度节点执行。
```

这就是 Compose Graph Runtime 的核心。

