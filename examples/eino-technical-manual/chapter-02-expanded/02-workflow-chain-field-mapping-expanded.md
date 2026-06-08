# Chapter 02 - Workflow / Chain / FieldMapping 深度讲解

面向读者：假设你已经读过 Chapter 01，知道 `Graph -> Compile -> Runnable` 这条主线，但还不熟悉 Eino 复刻版里 `Workflow`、`Chain` 和 `FieldMapping` 的设计。

这一章要回答的问题是：

```text
既然底层 Graph 已经能表达任意节点和边，为什么还要再做 Workflow 和 Chain？
既然边能传数据，为什么还要 FieldMapping？
```

参考代码位置：

- 手册：`examples/eino-technical-manual/manual/02-workflow-chain-field-mapping.md`
- 复刻版：`examples/eino-compose-runtime-replica-go`
- 本章重点源码：
  - `compose/workflow.go`
  - `compose/chain.go`
  - `compose/chain_parallel.go`
  - `compose/chain_branch.go`
  - `compose/field_mapping.go`
  - `compose/workflow_test.go`
  - `compose/chain_test.go`
  - `compose/field_mapping_test.go`

说明：本文以当前 Go 复刻版为准。原版 Eino 的 Chain Branch、stream field mapping、类型转换等能力更复杂；当前复刻版保留了核心教学骨架，足够理解三层编排抽象的设计。

## 1. 从 Chapter 01 接上：Graph 解决了什么，还没解决什么

Chapter 01 已经建立了一个核心模型：

```text
Graph construction -> Compile -> Runnable execution
```

你可以用 `Graph` 手动写出这样的拓扑：

```text
START -> prompt -> model -> parser -> END
```

代码大概是：

```go
g := compose.NewGraph[string, string]()
g.AddLambdaNode("prompt", promptNode)
g.AddLambdaNode("model", modelNode)
g.AddLambdaNode("parser", parserNode)
g.AddEdge(compose.START, "prompt")
g.AddEdge("prompt", "model")
g.AddEdge("model", "parser")
g.AddEdge("parser", compose.END)
```

这很清楚，也很底层。但业务代码一多，Graph 会暴露三个痛点。

第一，简单线性流程也要写很多 `AddEdge`。绝大多数 pipeline 其实就是“一步接一步”，每次都手写节点 key 和边很烦。

第二，一条边默认同时表达两件事：

```text
A 的输出作为 B 的输入
A 完成后 B 才能执行
```

也就是“数据流”和“控制依赖”绑在一起了。但真实业务里这两者不总是同一回事。

第三，节点之间不一定传“整个输出”。例如一个节点输出：

```go
type QueryContext struct {
    Query   string
    UserID  string
    TraceID string
}
```

下游 prompt 节点可能只需要 `Query`，audit 节点只需要 `UserID` 和 `TraceID`。如果只用普通边，你要么让下游节点接受完整结构体，要么写很多中间 lambda 拆字段。

所以第二章的目标是：在不放弃 Graph runtime 的前提下，给开发者更方便、更声明式的编排方式。

## 2. 三层编排抽象：Graph / Workflow / Chain

先记一张坐标图：

```text
控制力强                                          便利性强
Graph ---------------- Workflow ---------------- Chain
手动节点/边           声明依赖和字段映射          Append builder
```

这三者不是三个 runtime。它们最后都会编译成 `Runnable[I,O]`。

- `Graph`：你亲手添加节点和边，控制力最高。
- `Workflow`：你声明“这个节点需要哪些输入”，它帮你转成底层 graph 边和 field mapping。
- `Chain`：你用 `AppendX` 串起来，它帮你自动连接前后节点。

换句话说：

```text
Graph 是底层执行图。
Workflow 是声明式图构建器。
Chain 是顺序/并行/分支 pipeline builder。
```

这章不是在学三个互相替代的东西，而是在学三个不同粒度的 API 如何收敛到底层 Graph。

## 3. 为什么 FieldMapping 是这一章的中心

没有 FieldMapping 时，边只能表达：

```text
把 A 的整个输出交给 B
```

有了 FieldMapping，边可以表达：

```text
把 A 输出里的某个字段交给 B 输入里的某个字段
```

例如：

```go
wf.End().AddInput("model", compose.ToField("answer"))
```

这句话的意思是：把 `model` 节点的完整输出写到 END 输入的 `answer` 字段。

再比如：

```go
wf.AddLambdaNode("prompt", promptNode).
    AddInput(compose.START, compose.MapFields("Query", "query"))
```

意思是：从 START 输入中取 `Query` 字段，写入 prompt 节点输入的 `query` 字段。

这对 LLM 应用很关键。因为业务上下文常常是一个大对象，而每个组件只关心其中一部分。

## 4. 一个具体问题：为什么手写 Graph 会变啰嗦

假设你想做一个 RAG 前处理流程：

```text
输入: {Query, UserID, Locale}

retriever 只需要 Query
audit     需要 UserID 和 Query
prompt    需要 Query + retrieved docs + Locale
model     需要 prompt
```

如果不用 FieldMapping，你通常会写很多 adapter：

```text
input -> extractQuery -> retriever
input -> extractAuditFields -> audit
input + retrieverOutput -> assemblePrompt -> model
```

这些 adapter 不是真正的业务节点，只是搬字段。节点越多，图越脏。

FieldMapping 的价值就是把这些“搬字段”的样板逻辑收进 runtime：

```text
声明字段从哪里来到哪里去，而不是写一个 lambda 手动搬。
```

## 5. Workflow 的核心：声明依赖，而不是手动 AddEdge

看当前复刻版的 `compose/workflow.go`。

```go
type Workflow[I, O any] struct {
    g                *graph
    workflowNodes    map[string]*WorkflowNode
    workflowBranches []*WorkflowBranch
    dependencies     map[string]map[string]dependencyType
}
```

它里面仍然有一个底层 `*graph`。

这说明 Workflow 本质上不是新 runtime，而是 Graph 的包装器。它把用户的声明式 API 翻译成 Graph 的节点和边。

创建 Workflow 时：

```go
func NewWorkflow[I, O any]() *Workflow[I, O] {
    g := newGraph(fmtType(*new(I)), fmtType(*new(O)))
    g.triggerMode = AllPredecessor
    ...
}
```

这里有一个重要点：Workflow 默认使用 `AllPredecessor`，也就是 DAG 模式。

为什么？

因为 Workflow 表达的是确定性依赖关系。一个节点声明了多个输入，就应该等这些输入都准备好再执行。这更像普通业务流程，不是 agent loop。

## 6. WorkflowNode：节点外面再包一层声明信息

`WorkflowNode` 是 Workflow 的关键结构：

```go
type WorkflowNode struct {
    g                *graph
    key              string
    addInputs        []func() error
    staticValues     map[string]any
    dependencySetter func(fromNodeKey string, typ dependencyType)
    mappedFieldPath  map[string]any
}
```

字段解释：

- `g`：底层 graph。
- `key`：节点名。
- `addInputs`：延迟执行的输入声明。
- `staticValues`：编译时要注入的静态值。
- `dependencySetter`：记录依赖关系。
- `mappedFieldPath`：记录目标字段是否已经被占用，防止冲突。

最容易忽略的是 `addInputs []func() error`。

为什么 AddInput 不立即执行，而是存一个闭包？

因为 Workflow 允许你先声明节点和依赖，最后在 compile 时统一展开。尤其分支和 END 依赖需要在所有节点都注册后再处理。延迟执行可以避免“引用的节点还没准备好”的顺序问题。

## 7. Workflow 的三种依赖

`workflow.go` 里有三种 `dependencyType`：

```go
const (
    normalDependency   dependencyType = 1
    noDirectDependency dependencyType = 2
    branchDependency   dependencyType = 3
)
```

初学者可以这样理解。

### 7.1 normalDependency：数据 + 控制

默认 `AddInput` 是 normal dependency：

```go
node.AddInput("A", compose.MapFields("Name", "DisplayName"))
```

它表达两件事：

```text
A 的数据会传给 node
A 完成后 node 才能执行
```

底层会调用：

```go
n.g.addEdgeWithMappings(fromNodeKey, n.key, false, false, inputs...)
```

这里 `noDirectDependency=false`，`isControl=false`。

在当前 `graph.addEdgeWithMappings` 中，如果不是 `noDirectDependency`，添加数据边时也会添加控制边：

```go
g.dataEdges[fromNodeKey] = append(g.dataEdges[fromNodeKey], toNodeKey)
if !noDirectDependency {
    g.controlEdges[fromNodeKey] = append(g.controlEdges[fromNodeKey], toNodeKey)
}
```

所以 normal dependency 同时有数据流和控制依赖。

### 7.2 noDirectDependency：只有数据边，不加直接控制边

用法：

```go
node.AddInputWithOptions(
    compose.START,
    []*compose.FieldMapping{compose.ToField("from_start")},
    compose.WithNoDirectDependency(),
)
```

它表达：

```text
我需要 START 的数据，但不要因为这条边额外建立 START -> node 的控制依赖。
```

底层调用：

```go
n.g.addEdgeWithMappings(fromNodeKey, n.key, true, false, inputs...)
```

这里 `noDirectDependency=true`。

这适合什么场景？

看 `TestWorkflowNoDirectDependency`：

```text
START -> process -> audit -> END
START -----------data--------> audit
```

`audit` 既需要 `process` 的输出，也需要 START 的原始输入。执行顺序由 `process -> audit` 保证；START 的原始输入只是补充数据，不需要再表达控制顺序。

### 7.3 AddDependency：只有控制依赖，没有数据

用法：

```go
node.AddDependency("setup").AddInput(compose.START)
```

它表达：

```text
setup 必须先完成，但 setup 的输出不作为 node 的输入。
```

底层走控制边：

```go
n.g.addEdgeWithMappings(fromNodeKey, n.key, false, true)
```

这里 `isControl=true`。

看 `TestWorkflowAddDependencyControlOnly`：

```text
setup 接收 START，输出 setup_done
main 接收 START，同时依赖 setup 完成
```

`main` 最终处理的还是原始 input，但它会等 `setup` 完成。

这就是“执行依赖”和“数据映射”分离的价值。

## 8. Workflow 的编译流程

`Workflow.Compile` 最终调用 `wf.compile(ctx)`：

```go
func (wf *Workflow[I, O]) compile(ctx context.Context) (Runnable[I, O], error) {
    ...
    r, err := wf.g.compile(ctx)
    ...
    r.dag = true
    r.pregel = false
    return &graphRunnable[I, O]{cr: cr, runner: r}, nil
}
```

可以分成四步看。

### 8.1 先处理分支依赖

```go
for _, wb := range wf.workflowBranches {
    for endNode := range wb.endNodes {
        ...
    }
    _ = wf.g.addBranch(wb.fromNodeKey, wb.GraphBranch, true)
}
```

注意最后一个参数 `true`。这表示 Workflow 分支使用 `noDataFlow` 语义。

含义是：分支只负责路由控制，不自动把数据传给分支目标。目标节点如果需要数据，要自己显式 `AddInput`。

这和 Graph branch 容易混淆。初学时记住一句话：

```text
Workflow Branch 主要表达“走哪条路”，不负责“把什么数据送过去”。
```

### 8.2 再执行所有 AddInput 闭包

```go
for _, n := range wf.workflowNodes {
    for _, addInput := range n.addInputs {
        if err := addInput(); err != nil {
            return nil, err
        }
    }
    n.addInputs = nil
}
```

这一步才真正往底层 graph 里加边和 mapping。

### 8.3 再处理静态值

```go
if len(n.staticValues) > 0 {
    ...
    wf.g.handlerPreNodes[n.key] = append(wf.g.handlerPreNodes[n.key], pair)
}
```

`SetStaticValue` 不来自任何前驱节点，它是在节点执行前往输入里补字段。

例如：

```go
wf.AddLambdaNode("merge", mergeNode).
    AddInput(START, ToField("input")).
    SetStaticValue(FieldPath{"prefilled"}, "yo-ho")
```

最终 `merge` 收到：

```go
map[string]any{
    "input": "hello",
    "prefilled": "yo-ho",
}
```

### 8.4 最后编译底层 Graph

```go
r, err := wf.g.compile(ctx)
```

所以 Workflow 的本质就是：

```text
Workflow declarations -> Graph nodes/edges/mappings -> Graph compile -> Runnable
```

## 9. FieldMapping 的数据结构

看 `compose/field_mapping.go`：

```go
type FieldMapping struct {
    fromNodeKey     string
    from            string
    to              string
    customExtractor func(input any) (any, error)
}
```

字段含义：

- `fromNodeKey`：源节点是谁，Workflow 的 `AddInput(fromNodeKey, ...)` 会填它。
- `from`：从源输出的哪个字段取。
- `to`：写到目标输入的哪个字段。
- `customExtractor`：不用普通字段路径，自定义提取逻辑。

内部字段路径用 `\x1F` 分隔：

```go
const fieldPathSeparator = "\x1F"
```

为什么不用 `"."`？

因为 map key 或 struct field 可能本身包含点。内部用一个极少出现在普通字段名里的分隔符，更稳。

## 10. FieldMapping 的六种常用构造

### 10.1 MapFields(from, to)

```go
MapFields("Name", "DisplayName")
```

含义：

```text
source.Name -> target.DisplayName
```

适合 struct/map 到 struct/map 的字段改名。

### 10.2 FromField(from)

```go
FromField("Name")
```

含义：

```text
source.Name -> target whole input
```

也就是把源字段抽出来，作为目标节点的整个输入。

例如源是：

```go
Input{Name: "Alice"}
```

目标节点直接收到：

```go
"Alice"
```

### 10.3 ToField(to)

```go
ToField("query")
```

含义：

```text
source whole output -> target.query
```

如果 START 输入是字符串 `"hello"`，目标节点收到：

```go
map[string]any{"query": "hello"}
```

### 10.4 MapFieldPaths(fromPath, toPath)

```go
MapFieldPaths(FieldPath{"user", "profile", "name"}, FieldPath{"name"})
```

含义：

```text
source.user.profile.name -> target.name
```

适合嵌套结构。

### 10.5 FromFieldPath(fromPath)

```go
FromFieldPath(FieldPath{"F1", "F1"})
```

含义：

```text
source.F1.F1 -> target whole input
```

### 10.6 ToFieldPath(toPath)

```go
ToFieldPath(FieldPath{"payload", "query"})
```

含义：

```text
source whole output -> target.payload.query
```

## 11. FieldMapping 编译时检查

FieldMapping 不只是运行时搬字段，它还会做静态检查。

入口函数是：

```go
func validateFieldMapping(predecessorType, successorType reflect.Type, mappings []*FieldMapping) (...)
```

它做几类检查。

### 11.1 禁止 from-all 到 to-all

```go
if isFromAll(mappings) && isToAll(mappings) {
    return nil, nil, fmt.Errorf("invalid field mappings: from all fields to all, use common edge instead")
}
```

如果你想把整个输入传给整个输出，不需要 FieldMapping，普通边就够了。

### 11.2 目标字段要求目标是 struct/map/any

```go
if !isToAll(mappings) && !validateStructOrMap(successorType) && successorType != anyType {
    return error
}
```

如果你要写 `target.field`，那 target 至少得是 struct 或 map。否则字段写到哪里？

### 11.3 源字段要求源是 struct/map

```go
if fromFields(mappings) && !validateStructOrMap(predecessorType) {
    return error
}
```

如果你要取 `source.Name`，source 也得是 struct 或 map。

### 11.4 字段必须存在且可导出

`checkAndExtractFieldType` 会沿路径检查：

- struct 是否有这个字段。
- 字段是否 exported。
- map key 是否可用。
- 中间类型是否还能继续取字段。

所以这个会失败：

```go
MapFields("NonExist", "Y")
```

测试：`TestValidateFieldMappingFieldNotFound`。

这个也会失败：

```go
type Input struct {
    inner string
}
MapFields("inner", "Y")
```

因为 `inner` 不是导出字段。测试：`TestValidateFieldMappingUnexportedField`。

### 11.5 类型必须可赋值

`checkAssignable` 做类型兼容判断：

```go
if from.AssignableTo(to) {
    return assignableTypeMust
}
if reflect.PtrTo(from).AssignableTo(to) {
    return assignableTypeMust
}
if to.Kind() == reflect.Interface {
    if from.Implements(to) || reflect.PtrTo(from).Implements(to) {
        return assignableTypeMay
    }
}
return assignableTypeMustNot
```

例如：

```go
type Input struct { Age int }
type Output struct { Name string }
MapFields("Age", "Name")
```

`int` 不能赋给 `string`，所以编译时报错。

测试：`TestValidateFieldMappingTypeNotAssignable`。

### 11.6 interface 中间路径会推迟到运行时检查

如果字段路径中间遇到 `any`：

```go
type Container struct {
    Data any
}

MapFieldPaths(FieldPath{"Data", "Value"}, FieldPath{"Result"})
```

编译期只能知道 `Data` 是 `any`，不知道运行时里面到底是 struct、map 还是别的类型。

所以复刻版会返回 `uncheckedSourcePath`，把检查推迟到实际请求时。

这就是 outline 里说的：

```text
interface{} 中间类型只能推迟到请求时检查，编译通过不意味着运行安全。
```

## 12. FieldMapping 运行时怎么搬字段

运行时入口是：

```go
func fieldMap(mappings []*FieldMapping, allowMapKeyNotFound bool, uncheckedSourcePaths map[string]FieldPath) func(any) (map[string]any, error)
```

它返回一个函数。这个函数输入是源节点输出，输出是：

```go
map[string]any
```

也就是“目标字段路径 -> 提取到的值”。

### 12.1 custom extractor 优先

```go
if mapping.customExtractor != nil {
    result[mapping.to], err = mapping.customExtractor(input)
    continue
}
```

例如：

```go
ToField("first", WithCustomExtractor(func(input any) (any, error) {
    return input.([]int)[0], nil
}))
```

测试：`TestWorkflowCustomExtractor`。

### 12.2 from 为空表示取整个输入

```go
if len(mapping.from) == 0 {
    result[mapping.to] = input
    continue
}
```

这就是 `ToField("query")` 的逻辑。

### 12.3 普通路径逐段 takeOne

```go
taken, pathInputType, err = takeOne(pathInputValue, pathInputType, path)
```

`takeOne` 支持：

- struct field
- map key

不支持的类型会报错。

### 12.4 最后 convertTo 写入目标类型

`fieldMap` 先得到 `map[string]any`。后续会通过 `convertTo` 把 map 写入目标类型：

```go
func convertTo(mappings map[string]any, typ reflect.Type) any {
    tValue := newInstanceByType(typ)
    for mapping, taken := range mappings {
        tValue = assignOne(tValue, taken, mapping)
    }
    return tValue.Interface()
}
```

`assignOne` 支持写 struct、map、指针和嵌套路径。

这就是为什么你可以写：

```go
ToFieldPath(FieldPath{"payload", "query"})
```

runtime 会按路径逐层创建 map/struct/ptr 需要的中间对象。

## 13. Chain 的核心：自动连边

`Chain` 的结构很小：

```go
type Chain[I, O any] struct {
    err         error
    gg          *Graph[I, O]
    nodeIdx     int
    preNodeKeys []string
    hasEnd      bool
}
```

它里面也包了底层 `Graph[I,O]`。

Chain 的核心字段是 `preNodeKeys`。

你可以把它理解为：

```text
当前链尾部有哪些节点？
下一个 Append 节点应该从这些尾部节点接过来。
```

## 14. Chain 的线性 Append

看 `addNodeEdges`：

```go
func (c *Chain[I, O]) addNodeEdges(nodeKey string) {
    if len(c.preNodeKeys) == 0 {
        c.preNodeKeys = []string{START}
    }
    for _, preKey := range c.preNodeKeys {
        c.gg.AddEdge(preKey, nodeKey)
    }
    c.preNodeKeys = []string{nodeKey}
}
```

第一次 append：

```text
preNodeKeys 为空 -> 设置成 START
START -> node_0
preNodeKeys = [node_0]
```

第二次 append：

```text
node_0 -> node_1
preNodeKeys = [node_1]
```

第三次 append：

```text
node_1 -> node_2
preNodeKeys = [node_2]
```

所以这段 Chain：

```go
chain.
    AppendLambda(upper).
    AppendLambda(bracket)
```

会生成：

```text
START -> node_0 -> node_1 -> END
```

测试：`TestChainLinear`。

## 15. Chain Compile 自动补 END

`Chain.Compile`：

```go
func (c *Chain[I, O]) Compile(ctx context.Context) (Runnable[I, O], error) {
    if c.err != nil {
        return nil, c.err
    }
    if err := c.addEndIfNeeded(); err != nil {
        return nil, err
    }
    return c.gg.Compile(ctx, WithNodeTriggerMode(AllPredecessor))
}
```

`addEndIfNeeded` 会把当前尾部节点连到 END：

```go
for _, nodeKey := range c.preNodeKeys {
    c.gg.AddEdge(nodeKey, END)
}
```

所以 Chain 用户不用手写 `END` 边。

空 Chain 会失败：

```text
pre node keys not set
```

测试：`TestChainEmptyCompile`。

## 16. Chain Parallel：并行执行，然后 merge key

用法：

```go
parallel := compose.NewParallel()
parallel.
    AddLambda("upper", upperLambda).
    AddLambda("lower", lowerLambda)

chain.AppendParallel(parallel)
```

当前复刻版的 `AppendParallel` 做了三件事。

### 16.1 从同一个 startNode 分叉到多个并行节点

```go
if len(c.preNodeKeys) == 0 {
    startNode = START
} else if len(c.preNodeKeys) == 1 {
    startNode = c.preNodeKeys[0]
}
```

然后：

```go
c.gg.AddEdge(startNode, nodeKey)
```

生成：

```text
START -> node_0_parallel_0
START -> node_0_parallel_1
```

### 16.2 创建 merge node

当前复刻版不是让下一个节点直接收到 graph fan-in 的原始 map，而是显式加了一个 merge lambda：

```go
mergeKey := prefix + "_merge"
mergeLambda := InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
    out := make(map[string]any, len(in))
    for k, v := range in {
        if outputKey, ok := outputKeyMap[k]; ok {
            out[outputKey] = v
        } else {
            out[k] = v
        }
    }
    return out, nil
})
```

Graph fan-in 的 key 是内部 node key，例如：

```text
node_0_parallel_0
node_0_parallel_1
```

merge node 会把它改成用户指定的输出 key：

```text
upper
lower
```

所以 `TestChainParallel` 里最终输出是：

```go
map[string]any{
    "upper": "HELLO",
    "lower": "hello",
}
```

### 16.3 merge node 成为新的链尾

```go
c.preNodeKeys = []string{mergeKey}
```

所以 Parallel 之后再 Append 一个节点时，不是从两个并行节点连过去，而是从 merge node 连过去。

当前复刻版的结构是：

```text
START -> parallel_0 -> merge -> next
      -> parallel_1 -/
```

## 17. Chain Branch：当前复刻版是 Lambda router

这是一个重要差异点。

outline 里提到原版 Eino 可能是拓扑层 branch router。但当前复刻版的 `AppendBranch` 不是把每个分支节点注册成 Graph 分支拓扑，而是创建一个 Lambda 作为 router。

看 `chain.go`：

```go
branchRouter = InvokableLambda(func(ctx context.Context, input any) (any, error) {
    selected, err := b.condition(ctx, input)
    ...
    l, ok := b.lambdas[selected]
    return l.GetRunnable().invoke(ctx, input)
})
```

也就是说，从底层 Graph 看，Chain Branch 是一个普通节点：

```text
START -> branchRouter -> END
```

branchRouter 内部自己判断走哪个 lambda。

### 17.1 单分支输出

`NewChainBranch` 返回一个 key：

```go
return "long", nil
```

router 执行对应 lambda，直接返回该 lambda 的输出。

测试：`TestChainBranch`。

输入 `"hello-world"` 输出：

```text
LONG:hello-world
```

输入 `"hi"` 输出：

```text
SHORT:hi
```

### 17.2 多分支输出

`NewChainMultiBranch` 返回多个 key：

```go
return map[string]bool{"path_a": true, "path_b": true}, nil
```

router 会执行选中的多个 lambda，把结果放进 map：

```go
map[string]any{
    "path_a": "A:hello",
    "path_b": "B:hello",
}
```

测试：`TestChainMultiBranch`。

### 17.3 这个设计的优缺点

优点：

- 实现简单。
- 对 Chain 用户很顺滑。
- 单分支后继续 append 节点很自然。

缺点：

- 分支内部不是 Graph 拓扑层节点，可观测性弱一些。
- runtime event log 看不到每个分支 lambda 是独立图节点。
- branch 内部无法像 Graph branch 那样参与更细粒度的调度。

所以文档里要清楚写明：当前复刻版的 Chain Branch 是教学版简化实现。

## 18. Workflow vs Chain：什么时候用谁

### 用 Chain 的场景

当你的流程自然是 pipeline：

```text
input -> normalize -> prompt -> model -> parser -> output
```

用 Chain 很舒服：

```go
chain.
    AppendLambda(normalize).
    AppendLambda(prompt).
    AppendLambda(model).
    AppendLambda(parser)
```

Chain 适合：

- 线性流程。
- 简单并行。
- 简单条件路由。
- 不想手写 node key 和 edge。

### 用 Workflow 的场景

当你的节点之间是字段级依赖：

```text
prompt 需要 START.Query
audit  需要 START.UserID + model.Answer
END    需要 model.Answer + audit.LogID
```

用 Workflow 更清楚：

```go
wf.AddLambdaNode("prompt", prompt).
    AddInput(START, MapFields("Query", "query"))

wf.AddLambdaNode("audit", audit).
    AddInput(START, MapFields("UserID", "user_id")).
    AddInput("model", MapFields("Answer", "answer"))
```

Workflow 适合：

- 多输入节点。
- 字段映射。
- 控制依赖和数据依赖要分离。
- 想保留比 Chain 更多的拓扑表达能力。

### 用 Graph 的场景

当你需要：

- Pregel / AnyPredecessor。
- 循环图。
- 更底层的 branch/control edge。
- 自己完全控制拓扑。

就直接用 Graph。

## 19. 易误解点详解

### 误解 1：Workflow 是另一套运行时

不是。

Workflow 内部有 `g *graph`，最后调用 `wf.g.compile(ctx)`。

它只是更声明式地生成底层 Graph。

### 误解 2：Chain 是链表执行

不是。

Chain 不是自己写一个 for loop 执行节点。它仍然是生成 Graph：

```text
AppendLambda -> AddLambdaNode + AddEdge
Compile      -> Graph.Compile
```

### 误解 3：AddInput 永远等价于 AddEdge

不完全是。

默认 AddInput 确实会生成数据边和控制边。

但：

- `WithNoDirectDependency`：数据边，不加直接控制边。
- `AddDependency`：控制边，不传数据。

这正是 Workflow 比手写 Graph 更适合业务依赖的地方。

### 误解 4：FieldMapping 只是 map[string]any 的小工具

不是。

FieldMapping 做了三层工作：

1. 描述 source field -> target field。
2. 编译期用 reflect 检查字段和类型。
3. 运行时提取字段并写入目标类型。

它是一个数据契约，不只是 map 操作。

### 误解 5：编译通过就一定运行安全

不一定。

如果路径中间有 `interface{}` / `any`，编译期无法知道运行时具体类型，只能推迟检查。

所以这类 mapping 可能 compile 成功，但 invoke 时失败。

### 误解 6：Parallel 后下游一定收到原始 fan-in map

在 Graph 里，fan-in 默认可能收到按前驱 node key 分组的 map。

但当前 Chain Parallel 会插入 merge node，把内部 node key 改成用户指定的 output key。

所以 Chain Parallel 的输出 key 更友好：

```go
AddLambda("upper", ...)
AddLambda("lower", ...)
```

下游看到的是：

```go
map[string]any{"upper": ..., "lower": ...}
```

### 误解 7：Chain Branch 是底层 GraphBranch

当前复刻版不是。

Chain Branch 是 Lambda router。它内部调用选中 lambda。

这和 Graph 的 `AddBranch`/`GraphBranch` 是不同实现。

### 误解 8：Workflow Branch 会自动把数据传给分支节点

当前 Workflow compile 时：

```go
wf.g.addBranch(wb.fromNodeKey, wb.GraphBranch, true)
```

`true` 表示 noDataFlow。

所以 Workflow branch 只负责路由；分支节点要自己声明 `AddInput`。

### 误解 9：多个 AddInput 映射到同一目标字段没问题

会冲突。

`WorkflowNode.checkAndAddMappedPath` 会记录目标字段路径。两个输入都写同一个 target path 时，compile 失败。

测试：`TestWorkflowFanInPathConflict`。

### 误解 10：Stream 模式下 FieldMapping 已完整实现

当前复刻版没有。

`streamFieldMap` 里直接：

```go
panic("streamFieldMap: not implemented")
```

所以学习当前 repo 时，把 FieldMapping 理解为 Invoke 主线能力即可。

## 20. 建议源码阅读顺序

第一遍看主线：

1. `workflow_test.go`
   - `TestWorkflowBasicThreeNodes`
   - `TestWorkflowFanInFieldMapping`
   - `TestWorkflowNoDirectDependency`
   - `TestWorkflowStaticValue`

2. `workflow.go`
   - `Workflow` struct
   - `WorkflowNode` struct
   - `AddInput`
   - `AddInputWithOptions`
   - `AddDependency`
   - `compile`

3. `field_mapping.go`
   - `FieldMapping`
   - 六个构造函数
   - `validateFieldMapping`
   - `fieldMap`
   - `convertTo`

4. `chain_test.go`
   - `TestChainLinear`
   - `TestChainParallel`
   - `TestChainBranch`
   - `TestChainMultiBranch`

5. `chain.go`
   - `Chain` struct
   - `AppendLambda`
   - `AppendParallel`
   - `AppendBranch`
   - `addNodeEdges`
   - `addEndIfNeeded`

6. `chain_parallel.go` / `chain_branch.go`
   - `NewParallel`
   - `AddLambda`
   - `NewChainBranch`
   - `NewChainMultiBranch`

第二遍再看边界能力：

1. Workflow branch。
2. static value 冲突。
3. field mapping interface deferred check。
4. Chain AppendGraph / Parallel AddGraph / Branch AddGraph。
5. stream field mapping 未实现的边界。

## 21. 练习题

### 练习 1：把 Graph 线性流程改成 Chain

目标：理解 Chain 自动连边。

要求：

1. 创建 `NewChain[string, string]()`。
2. Append 一个 trim lambda。
3. Append 一个 upper lambda。
4. Append 一个 bracket lambda。
5. 输入 `"  hello  "`，输出 `"[HELLO]"`。

思考：

- Chain 内部生成了几个节点？
- START 和 END 是什么时候被加上的？
- 如果不 Append 任何节点就 Compile 会怎样？

### 练习 2：实现 Chain Parallel

目标：理解 Parallel 和 merge node。

要求：

1. 创建 `NewParallel()`。
2. 添加 `upper` 和 `lower` 两个 lambda。
3. `chain.AppendParallel(parallel)`。
4. 输入 `"Hello"`。
5. 期望输出：

```go
map[string]any{
    "upper": "HELLO",
    "lower": "hello",
}
```

思考：

- 如果两个 parallel node 使用同一个 output key，会怎样？
- merge node 为什么要存在？

### 练习 3：实现 Chain Branch

目标：理解当前复刻版的 Lambda router。

要求：

1. 创建 `NewChainBranch`。
2. 字符串长度大于 5 走 `long`。
3. 否则走 `short`。
4. `long` 输出 `"LONG:"+in`。
5. `short` 输出 `"SHORT:"+in`。

思考：

- 这个 branch 在底层 Graph 里是几个节点？
- 为什么它不是 GraphBranch？
- 如果 condition 返回不存在的 key，会怎样？

### 练习 4：Workflow 基础三节点

目标：理解 AddInput 默认数据 + 控制依赖。

要求：

1. 创建 `NewWorkflow[string, string]()`。
2. `template` 节点输入 START，输出 `"processed:"+in`。
3. `model` 节点输入 template，输出 `"model:"+in`。
4. END 输入 model。
5. 输入 `"hello"`，输出 `"model:processed:hello"`。

思考：

- AddInput 什么时候真正变成 graph edge？
- Workflow 为什么默认是 DAG？

### 练习 5：Workflow 字段映射 fan-in

目标：理解多个来源写入不同字段。

要求：

1. 节点 A 输出 `"value_a"`。
2. 节点 B 输出 `"value_b"`。
3. END 同时 AddInput A 和 B：

```go
wf.End().
    AddInput("A", ToField("field_a")).
    AddInput("B", ToField("field_b"))
```

思考：

- 如果两个 ToField 都写 `"same_key"`，会怎样？
- 当前 END 结果为什么可能按来源节点分组？

### 练习 6：Workflow 控制依赖

目标：理解 AddDependency。

要求：

1. setup 节点接收 START，输出 `"setup_done"`。
2. main 节点接收 START，但 `AddDependency("setup")`。
3. main 输出 `in+"_processed"`。

思考：

- main 为什么不接收 setup 的输出？
- 如果去掉 AddDependency，main 是否可以更早执行？

### 练习 7：Workflow NoDirectDependency

目标：理解数据边和控制边拆分。

要求：

1. process 从 START 生成 map：`from_process`。
2. audit 接收 process 的 `from_process`。
3. audit 同时通过 `WithNoDirectDependency` 接收 START 原始输入到 `from_start`。

思考：

- START 的数据为什么能进 audit？
- audit 的执行顺序是谁保证的？
- 如果只有 noDirectDependency，没有其他控制依赖，会发生什么？

### 练习 8：FieldMapping 静态检查

目标：理解 compile-time error。

判断下面哪些能通过：

1. `MapFields("Name", "DisplayName")`，源有 `Name string`，目标有 `DisplayName string`。
2. `MapFields("Age", "Name")`，源 `Age int`，目标 `Name string`。
3. `MapFields("inner", "Name")`，源字段 `inner string` 未导出。
4. `MapFields("Missing", "Name")`，源没有 `Missing` 字段。

解释原因。

### 练习 9：FieldPath 嵌套路径

目标：理解嵌套字段提取。

要求：

1. 输入结构体：

```go
type Inner struct { Value string }
type Input struct { Data *Inner }
```

2. 使用：

```go
FromFieldPath(FieldPath{"Data", "Value"})
```

3. 输入 `&Input{Data: &Inner{Value: "hello"}}`。
4. 目标节点收到 `"hello"`。

思考：

- 如果 `Data` 是 nil，会发生什么？
- 如果中间字段是 `any`，为什么要运行时检查？

### 练习 10：设计题：什么时候不用 Chain

给一个流程：

```text
START.Query -> retriever
START.Locale -> prompt
retriever.Docs + START.Query + START.Locale -> prompt
prompt -> model
model.Answer + START.UserID -> audit
model.Answer -> END
```

问题：

- 用 Chain 表达会不会别扭？
- 用 Workflow 怎么声明更清楚？
- 哪些地方需要 FieldMapping？

## 22. 自测问题

读完后，你应该能回答：

1. Workflow 和 Graph 的关系是什么？
2. Chain 和 Graph 的关系是什么？
3. `AddInput` 默认表达哪两种依赖？
4. `AddDependency` 和 `AddInput` 有什么区别？
5. `WithNoDirectDependency` 解决什么问题？
6. `FieldMapping` 的 `from` 和 `to` 分别是什么意思？
7. `MapFields`、`FromField`、`ToField` 怎么区分？
8. 为什么 FieldMapping 要做编译期类型检查？
9. 为什么 interface 中间路径只能延迟到运行时检查？
10. Chain 的 `preNodeKeys` 是什么？
11. Chain Parallel 为什么需要 merge node？
12. 当前复刻版 Chain Branch 和 GraphBranch 有什么区别？
13. Workflow Branch 为什么不自动传数据？
14. 什么时候应该用 Graph，而不是 Workflow 或 Chain？

## 23. 一句话总结

Chapter 02 的核心不是多背几个 API，而是理解三层编排抽象的分工：

```text
Graph 负责最底层的拓扑表达。
Workflow 负责声明式依赖和字段级数据契约。
Chain 负责常见 pipeline 的顺手构建。
FieldMapping 负责把“节点输出”变成“字段级输入”。
```

它们最终都收敛到同一个运行时：

```text
Graph / Workflow / Chain -> Compile -> Runnable -> runner.run
```

