# 第二章：Workflow / Chain / 字段映射模式

## 1. 问题

Eino 提供三种编排抽象 — `Graph`、`Workflow`、`Chain` — 开发者需要理解它们的区别和适用场景才能在正确的场景使用正确的工具。核心问题是：

1. **"传递什么数据"与"按什么顺序执行"是两种正交的关心点**。在简单的 DAG 中，一条边同时表达了数据流和依赖关系，但一旦出现嵌套路径、跨分支数据访问或多元数据合并，简单边就不够用了。
2. **线性 pipeline 很常见**，但每次都手动构建 Graph（添加节点、添加边）太繁琐。开发者想要 builder 风格的链式 API：`chain.AppendX().AppendY().AppendZ()`。
3. **并行和分支是 LLM 应用的常见需求** — 同时调两个模型、根据意图路由 — 这些在底层图结构中需要特殊的编排逻辑。
4. **同一种结构需要支持四种执行形态**（Invoke / Stream / Collect / Transform），编排抽象需要在编译时就验证类型兼容性。

如果缺少这三层抽象，用户要么在 Graph 里写大量样板代码，要么丢失字段映射的类型安全。

## 2. 为什么难

### 2.1 执行依赖与数据映射是两回事

在单边图中，`A -> B` 自然表达了"A 执行完 B 才能执行"且"A 的输出作为 B 的输入"。但考虑这个场景：

```
START -> SetupNode -> ModelNode -> END
                  \-> AuditNode --/
```

`AuditNode` 需要 `Start` 的数据（user_id）但不应该被 `SetupNode` 阻塞。如果只用普通的边，每一个 `AddEdge` 都同时创建了执行依赖和数据传递。这迫使开发者要么创建一个冗余的边破坏了执行顺序，要么把数据用别的方式手动传入。

### 2.2 字段映射需要编译时类型检查

LLM 应用中的节点输入输出通常是结构体或 map，字段映射需要：
- 在编译时检查源字段是否存在、目标字段类型是否兼容
- 处理嵌套路径（`user.profile.name`）
- 处理 interface 类型需要推迟到请求时检查
- 处理 map key 缺失需要根据配置决定是跳过还是报错

`compose/field_mapping.go` 的 `validateFieldMapping` 函数（L645）承担了所有这些检查逻辑，包括对 interface 中间类型的 deferred check。

### 2.3 Chain builder 需要处理分支汇聚

Chain 的 builder 模式看似简单，但一旦加入 Parallel 或 Branch，前驱节点的概念就变得复杂：Parallel 之后是多个节点同时成为"前驱"，Branch 之后不确定哪个分支会被激活。Chain 必须隐式追踪 `preNodeKeys` 并正确处理 START/END 的自动连接。

### 2.4 Workflow 的分支语义与 Graph 不同

在 Graph 中，分支的输入数据会自动传递给分支选中的节点。在 Workflow 中，分支**不传递数据**：`WorkflowBranch` (workflow.go:408) 要求分支内部节点显式定义自己的字段映射（workflow.go:415-418）。

## 3. 设计思路

Eino 的三层抽象按照"控制力 vs 便利性"的坐标排列：

```
控制力高 ←→ 便利性高
Graph ────── Workflow ────── Chain
```

### 3.1 Graph：完全控制

`Graph[I,O]` (`compose/generic_graph.go`) 是最底层的显式有向图。用户通过 `AddEdge` 手动建立边，控制触发模式（`AnyPredecessor` 或 `AllPredecessor`），适合表达复杂的、非常规的拓扑结构（如带环的 Pregel 图）。

### 3.2 Workflow：声明式依赖 + 字段映射

`Workflow[I,O]` (`compose/workflow.go:45`) 包装了 Graph，把"添加边"的思维转变为"声明依赖关系"。核心假设是：节点之间的数据依赖比执行依赖更重要。Workflow 使用 `AllPredecessor` 触发模式，不能用出环。

Workflow 不通过 `AddEdge`，而是通过 `WorkflowNode.AddInput` 来同时建立**数据映射**和**执行依赖**。这允许精细控制：
- 默认：`AddInput` 创建数据流 + 执行依赖
- `WithNoDirectDependency`：只有数据流，执行依赖通过其他节点间接保证
- `AddDependency`：只有执行依赖，没有数据流

### 3.3 Chain：Builder 模式

`Chain[I,O]` (`compose/chain.go:72`) 提供线性 builder API。每个 `AppendX` 自动将新节点与前一个节点（或前一组节点如 Parallel）连接。内部通过 `preNodeKeys` (`compose/chain.go:79`) 追踪当前"尾部"节点集合。

Chain 内置 Parallel（并发）和 Branch（条件分支）支持，但汇聚逻辑需要用 `AppendPassthrough` 或接受 `map[string]any` 的节点来处理。

### 3.4 统一编译目标

三者最终都编译为 `Runnable[I,O]` (`compose/runnable.go`)，共享同样的运行时执行、callback、state 机制。Workflow 和 Chain 在 `Compile` 阶段被展开为底层 Graph，因此它们的性能没有额外开销。

## 4. 源码走读

### 4.1 Workflow 核心结构

`compose/workflow.go`

```go
// L45-50: Workflow 包装 Graph，维护节点映射和依赖关系表
type Workflow[I, O any] struct {
    g                *graph
    workflowNodes    map[string]*WorkflowNode
    workflowBranches []*WorkflowBranch
    dependencies     map[string]map[string]dependencyType
}
```

```go
// L34-41: WorkflowNode 持有 Graph 内部节点的引用，维护映射路径和静态值
type WorkflowNode struct {
    g                *graph
    key              string
    addInputs        []func() error        // 延迟执行的 AddInput 闭包
    staticValues     map[string]any        // 编译时静态值
    dependencySetter func(fromNodeKey string, typ dependencyType)
    mappedFieldPath  map[string]any        // 已映射的字段路径（防冲突）
}
```

**依赖类型** (`workflow.go:52-58`)：

```go
const (
    normalDependency    dependencyType = iota  // 数据 + 执行
    noDirectDependency                        // 只有数据（通过间接路径保证执行）
    branchDependency                          // 分支依赖
)
```

### 4.2 AddInput 的内部机制

`WorkflowNode.AddInput` (`workflow.go:197-199`) 是所有数据依赖的入口。实际逻辑在 `addDependencyRelation` (L316-367)，它根据配置创建三种形态的闭包：

1. **默认模式** (`else` 分支, L348-363)：
   - 验证字段路径不冲突（`checkAndAddMappedPath`）
   - 调用 `g.addEdgeWithMappings(fromNodeKey, n.key, false, false, inputs...)`
   - 设置 `normalDependency`

2. **NoDirectDependency 模式** (L321-336)：
   - 同样验证路径和添加映射边
   - 将 `noDirectDependency` 作为第三个参数传入 `addEdgeWithMappings(..., true, false, ...)`
   - 设置 `noDirectDependency`

3. **DependencyWithoutInput 模式** (L337-347)：
   - 拒接带 input 的调用
   - 调用 `g.addEdgeWithMappings(fromNodeKey, n.key, false, true)`
   - 设置 `normalDependency`

关键：`addInputs` 是闭包数组而不是立即执行，因为 Workflow 的编译顺序是"先收集所有分支信息，再统一添加边"。在 `compile` (`workflow.go:440-512`) 中，先处理所有 `workflowBranches`，再统一执行所有 `addInputs`。

### 4.3 FieldMapping 体系

`compose/field_mapping.go`

FieldMapping 是一个字段级别的数据传递描述符：

```go
// L31-37: FieldMapping 包含源节点 key、源字段路径、目标字段路径、自定义提取器
type FieldMapping struct {
    fromNodeKey string
    from        string          // 内部使用 \x1F 分隔的嵌套路径
    to          string
    customExtractor func(input any) (any, error)
}
```

六个构造函数提供不同的映射粒度：

| 函数 | 含义 | 示例 |
|------|------|------|
| `MapFields(from, to)` | 从源字段到目标字段 | `MapFields("name", "userName")` |
| `FromField(from)` | 从源字段到全部输入 | `FromField("Field1")` |
| `ToField(to)` | 从全部输出到目标字段 | `ToField("query")` |
| `MapFieldPaths(fromPath, toPath)` | 从嵌套路径到嵌套路径 | `MapFieldPaths(FieldPath{"user","profile","name"}, FieldPath{"name"})` |
| `FromFieldPath(fp)` | 从嵌套路径到全部输入 | `FromFieldPath(FieldPath{"data","result"})` |
| `ToFieldPath(fp)` | 从全部输出到嵌套路径 | `ToFieldPath(FieldPath{"response","data"})` |

**类型检查**在 `validateFieldMapping` (L645-774) 中进行：

1. 实例检查：不允许 FromAll + ToAll（这是普通边）
2. 结构检查：ToAll 要求 successor 是 struct/map；FromAll（FromField 不设置映射）要求 predecessor 是 struct/map
3. 逐字段检查（L664-740）：遍历每个 mapping，用 `checkAndExtractFieldType` 沿着 source/target 路径提取类型，判断 `assignableTypeMust` / `assignableTypeMay` / `assignableTypeMustNot`
4. 对于包含 interface 中间类型的路径或 `assignableTypeMay` 的字段，构建 `handlerPair` 推迟到请求时检查

**实际映射执行**在 `fieldMap` (L484-566) 中：
- 遍历每个 mapping
- 调用 `takeOne` 从 source value 中按路径提取值（处理 struct field 和 map key 两种情况）
- 遇到 nil interface / nil map 报错，遇到 map key not found 根据 `allowMapKeyNotFound` 决定是否跳过

### 4.4 Chain 的 Builder 模式

`compose/chain.go`

Chain 的核心是 `addNode` (L560-600)：

```go
// 伪代码
func (c *Chain) addNode(node, options) {
    // 1. 如果 preNodeKeys 为空（chain 刚开始），自动连接 START
    if len(c.preNodeKeys) == 0 {
        c.preNodeKeys = append(c.preNodeKeys, START)
    }
    // 2. 从所有 preNodeKeys 到新节点建边
    for _, preNodeKey := range c.preNodeKeys {
        c.gg.AddEdge(preNodeKey, nodeKey)
    }
    // 3. 新节点成为唯一的 preNodeKey
    c.preNodeKeys = []string{nodeKey}
}
```

这个设计使得：
- 线性 Append：每次 preNodeKeys 只有一个元素，链自然形成
- Parallel 之后：`AppendParallel` (L459-514) 将 preNodeKeys 设置为多个并行节点的 key，下一个 `addNode` 会从所有并行节点连接到新节点
- Branch 之后：`AppendBranch` (L342-447) 将 preNodeKeys 设置为所有分支节点的 key，下一个节点自动从所有分支汇聚

**自动 END 连接**：`addEndIfNeeded` (L98-121) 在编译时将所有 `preNodeKeys` 连接到 END。

**节点命名规则** (`nextNodeKey`, L544-548)：
- 普通节点：`node_0`, `node_1`, ...
- 并行节点：`node_0_parallel_0`, `node_0_parallel_1`
- 分支节点：`node_1_branch_customkey`

### 4.5 Parallel 和 ChainBranch 的内部结构

`compose/chain_parallel.go`

`Parallel` (L49-53) 是"输出 key 到节点"的集合，要求至少 2 个节点。每个节点用 `WithOutputKey` 标记，最终输出为 `map[string]any`。

```go
parallel := NewParallel()
parallel.AddChatModel("gpt4", model1)   // => {"gpt4": *schema.Message{}}
parallel.AddChatModel("claude", model2)  // => {"claude": *schema.Message{}}
```

`compose/chain_branch.go`

`ChainBranch` (L38-42) 封装 `GraphBranch` + 节点映射表：

```go
type ChainBranch struct {
    internalBranch *GraphBranch
    key2BranchNode map[string]nodeOptionsPair
    err            error
}
```

`NewChainBranch` (L100-108) 包装单路径条件函数为多路径，`NewChainMultiBranch` (L46-63) 直接接受多路径条件。

当 `chain.AppendBranch(cb)` 被调用时 (chain.go:342-447)：
1. 每个分支节点被注册到 graph 中，key 加前缀避免冲突
2. `GraphBranch.invoke`/`collect` 被重新包装以将分支 key 映射到 graph 节点 key
3. 分支被添加到 graph 中

### 4.6 GraphBranch 条件评估

`compose/branch.go`

`GraphBranch` (L42-50) 的核心是两个评估函数：

```go
type GraphBranch struct {
    invoke    func(ctx context.Context, input any) (output []string, err error)
    collect   func(ctx context.Context, input streamReader) (output []string, err error)
    ...
    endNodes   map[string]bool
    noDataFlow bool  // Workflow 的分支使用此标记
}
```

`NewGraphBranch` (L145-153) 实现单路径分支，`NewGraphMultiBranch` (L89-107) 实现多路径分支（一个输入可以路由到多个目标节点）。stream 变体 `NewStreamGraphBranch` / `NewStreamGraphMultiBranch` 对 StreamReader 做条件评估。

## 5. 模式与示例

### 5.1 模式：带字段映射的简单 Workflow

从 START 接收含有 `query` 字段的结构体，经过模板渲染和模型调用后输出。

```go
type Input struct {
    Query  string
    UserID string
}

type Output struct {
    Reply string
}

w := NewWorkflow[*Input, *Output]()

// STEP 1: 从 START 提取 query 字段传给模板节点
w.AddChatTemplateNode("template", chatTemplate).
    AddInput(START, MapFields("Query", "query"))

// STEP 2: 模板输出 -> 模型调用
w.AddChatModelNode("model", chatModel).
    AddInput("template")

// STEP 3: 模型输出 -> 提取 Content 字段到 END 的 Reply
w.End().AddInput("model", MapFields("Content", "Reply"))

r, _ := w.Compile(ctx)
result, _ := r.Invoke(ctx, &Input{Query: "你好", UserID: "u1"})
```

### 5.2 模式：执行依赖与数据流分离

`SetupNode` 负责初始化，`MainNode` 需要 `SetupNode` 完成但不需要其数据，`MainNode` 也同时需要原始输入的 `userID`。

```go
w := NewWorkflow[*Input, *Output]()

setupNode := w.AddLambdaNode("setup", setupLambda)
mainNode := w.AddLambdaNode("main", mainLambda)

// mainNode 需要 setupNode 执行完成，但不需要其输出数据
mainNode.AddDependency("setup")

// mainNode 需要 START 的 userID 字段
mainNode.AddInput(START, MapFields("UserID", "userID"))
```

### 5.3 模式：NoDirectDependency — 跨路径数据访问

`AuditNode` 需要原始输入的数据但不应该在 `ProcessNode` 的直接依赖链上。它的执行由 `ProcessNode` 间接保证（因为存在另一条路径）。

```go
processNode := w.AddLambdaNode("process", processLambda).
    AddInput(START)
auditNode := w.AddLambdaNode("audit", auditLambda)

// auditNode 需要 START 的数据，但不直接依赖 START 完成
// 它的执行顺序由 processNode -> auditNode 的间接路径保证
auditNode.AddInputWithOptions(START, nil, WithNoDirectDependency())

// 同时 auditNode 等待 processNode 的数据
auditNode.AddInput("process")
```

### 5.4 模式：带分支的 Chain

根据意图路由到不同的处理路径。

```go
chain := NewChain[map[string]any, string]()

// 意图识别
branchCond := func(ctx context.Context, in map[string]any) (string, error) {
    if in["intent"] == "chat" {
        return "chatPath", nil
    }
    return "qaPath", nil
}

chatPath := InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
    in["role"] = "friendly bot"
    return in, nil
})
qaPath := InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
    in["role"] = "knowledge expert"
    return in, nil
})

chain.
    AppendLambda(intentLambda).
    AppendBranch(NewChainBranch[map[string]any](branchCond).
        AddLambda("chatPath", chatPath).
        AddLambda("qaPath", qaPath),
    ).
    AppendPassthrough().  // 汇聚两个分支
    AppendChatTemplate(chatTemplate).
    AppendChatModel(model).
    AppendLambda(extractContentLambda)

r, _ := chain.Compile(ctx)
```

### 5.5 模式：带并行的 Chain

并行调用两个模型，然后对比结果。

```go
chain := NewChain[map[string]any, string]()

parallel := NewParallel()
parallel.
    AddChatModel("gpt4", gpt4Model).
    AddChatModel("claude", claudeModel)

chain.
    AppendChatTemplate(chatTemplate).
    AppendParallel(parallel).
    AppendLambda(InvokableLambda(func(ctx context.Context, in map[string]any) (string, error) {
        // in = {"gpt4": *schema.Message{}, "claude": *schema.Message{}}
        gpt4Msg := in["gpt4"].(*schema.Message)
        claudeMsg := in["claude"].(*schema.Message)
        return fmt.Sprintf("GPT4: %s\nClaude: %s", gpt4Msg.Content, claudeMsg.Content), nil
    }))

r, _ := chain.Compile(ctx)
```

### 5.6 模式：带分支的 Workflow（与 Graph Branch 的区别）

```go
w := NewWorkflow[*Input, *Output]()

classifier := w.AddLambdaNode("classifier", classifierLambda).
    AddInput(START)
pathA := w.AddLambdaNode("pathA", pathALambda)
pathB := w.AddLambdaNode("pathB", pathBLambda)

// Workflow Branch: 分支不传递数据
// 每个分支节点需要显式 AddInput
branch := NewGraphBranch(condFunc, map[string]bool{"pathA": true, "pathB": true})
w.AddBranch("classifier", branch)

// pathA 和 pathB 需要自己声明从哪里获取数据
pathA.AddInput("classifier", FromField("Field1"))
pathB.AddInput("classifier", FromField("Field2"))

w.End().AddInput("pathA", MapFields("Result", "Reply"))
w.End().AddInput("pathB", MapFields("Result", "Reply"))
```

### 5.7 模式：在编译时使用 Compile（静态值）

```go
node := w.AddChatTemplateNode("template", chatTemplate)

// 在编译时写入静态值，运行时不可变
node.SetStaticValue(FieldPath{"system_prompt"}, "You are a helpful assistant")
node.SetStaticValue(FieldPath{"settings", "temperature"}, 0.7)
node.AddInput(START, MapFields("UserQuery", "user_query"))
```

`SetStaticValue` 在 `compile` 阶段 (workflow.go:469-507) 通过 `handlerPreNode` 注入一个 mergeValues handler，将静态值合并到节点输入中。

### 5.8 模式：Workflow 中嵌套 Chain

```go
// 构建一个子 Chain
subChain := NewChain[string, *Output]().
    AppendChatTemplate(template).
    AppendChatModel(model)

// 在 Workflow 中使用
w.AddGraphNode("sub", subChain).
    AddInput(START, MapFields("Query", "Query"))
```

## 6. 常见陷阱

### 6.1 混合 Graph 边和 Workflow 映射

Workflow 不应该直接调用底层 Graph 的 `AddEdge`。Workflow 通过 `AddInput` 同时管理数据映射和执行依赖，如果混用手动边，可能导致 Workflow 的依赖追踪表 `dependencies` 与实际图结构不一致。

### 6.2 忘记 Branch 后汇聚

在 Chain 中使用 `AppendBranch` 后，所有分支的 end node 都成为 `preNodeKeys`。若不使用 `AppendPassthrough()` 或下一个节点接受 `map[string]any`，编译时 `addEndIfNeeded()` 会将所有分支节点接到 END，这可能产生意外的行为。

Chain test (`chain_test.go`) 中的正确做法：`AppendBranch(...).AppendPassthrough()`。

### 6.3 NoDirectDependency 缺少间接路径

`WithNoDirectDependency` 取消了直接执行依赖，但**必须**存在一条通过其他节点的间接路径保证 predecessor 在该节点之前完成。如果不存在，predecessor 可能在 successor 之后执行，导致数据丢失。

文档注释明确说明 (`workflow.go:250-252`)：
> There MUST be a path from the predecessor that eventually reaches the current node through other nodes with direct dependencies.

### 6.4 FieldMapping 的类型冲突

当对一个 successor 节点调用多次 `AddInput` 时，`checkAndAddMappedPath` (`workflow.go:369-404`) 会检查目标路径是否冲突。例如：

```go
// 错误：两次都映射到整个输入
node.AddInput("A")           // 整个 A 输出 -> 整个输入
node.AddInput("B")           // 整个 B 输出 -> 整个输入  ← 冲突！

// 正确：逐个字段映射
node.AddInput("A", MapFields("field1", "f1"))
node.AddInput("B", MapFields("field2", "f2"))
```

### 6.5 Workflow Branch 不自动传递数据

与 Graph 的分支不同，Workflow 的 `AddBranch` 注释明确说明 (`workflow.go:415-418`)：

> End nodes of the branch are required to define their own field mappings.

忘记为分支节点添加 `AddInput` 是最常见的 Workflow 分支错误。

### 6.6 Parallel 输出 Key 冲突

`Parallel` 的 `addNode` (chain_parallel.go:235-262) 会检查输出 key 是否重复。两个节点用同一个 outputKey 会报错。

### 6.7 编译时 vs 请求时的类型检查边界

当字段映射路径中遇到 interface 类型时，`checkAndExtractFieldType` (`field_mapping.go:472-473`) 返回 error（需要请求时检查），此时 `validateFieldMapping` 将其放入 `uncheckedSourcePath`，推迟到请求时通过 `fieldMap` 的实际提取结果进行验证。这意味着带 interface 的字段映射在编译时不会报错，但请求时可能 panic。

## 7. Rive 可借鉴之处

### 7.1 将执行依赖与数据映射解耦

Eino 的 `WithNoDirectDependency` 和 `AddDependency` 是关键的架构观察：**在复杂的 Work DAG 中，数据依赖图和执行依赖图往往是两个不同的图**。Rive 目前把数据传递和执行顺序绑定在一个依赖中。参考 Eino 的设计，Rive 可以考虑：

- 让工作节点声明"我需要 X 的输出数据"(数据映射)
- 同时声明"我需要在 Y 之后执行"(执行依赖)
- 这两者不一定要来自同一个上游节点

### 7.2 Field Mapping 模式的适用性

Eino 的 `FieldMapping` 是在 in-process 执行图中做细粒度数据流转。Rive 的 Work DAG 节点通常通过 `output / input key` 传递整个输出，但有些场景（如跨节点合并多个输出、提取嵌套字段）可能需要类似的"字段路径提取"机制。不过这需要在 Rive 的序列化层 (protobuf) 上实现路径提取逻辑，不同于 Eino 的反射方式。

### 7.3 Builder 模式降低入门门槛

Eino 的 `Chain` builder 模式使简单的线性 pipeline 不需要写 Graph 样板代码。Rive 可以为常见的编排模式（如串行任务链、条件分支）提供类似的 Builder API，降低用户搭建 DAG 的心智负担。

### 7.4 统一编译目标

Eino 的 Graph / Workflow / Chain 三者编译后都是 `Runnable[I,O]`，这意味着它们可以任意嵌套。Rive 当前的 Work DAG 是平面结构，子图支持有限。参考 Eino 的嵌套图模式，Rive 可以让子 Work DAG 作为父 DAG 的一个节点，共享回调、状态和恢复机制。

### 7.5 延迟求值的闭包模式

Eino 的 Workflow 将 `addInputs` 设计为 `[]func() error` 延迟闭包 (`workflow.go:37`) 而不是立即执行，使得 Workflow 可以在收集完所有节点和分支信息后再统一构建图。这种"声明-编译"两阶段模式值得 Rive 在执行图构建时参考。

### 7.6 静态值注入

`SetStaticValue` 提供了一种在编译时固定值的机制，适合配置参数、系统 prompt 等在运行时不变的数据。Rive 的 dispatch 参数目前全部是动态传入的，增加编译时参数可以减少每次执行的计算开销。

## 总结

| 维度 | Graph | Workflow | Chain |
|------|-------|----------|-------|
| 声明方式 | 手动 AddEdge | AddInput + FieldMapping | AppendX builder |
| 触发模式 | AnyPredecessor / AllPredecessor | AllPredecessor (固定) | 自动管理 |
| 字段映射 | 通过边 + FieldMapping | 内置在 AddInput | 自动（类型匹配即可） |
| 分支行为 | 数据自动传递 | 数据需要手动声明 | 通过 ChainBranch 封装 |
| 并行支持 | 通过多入边 | 通过多入边 | AppendParallel |
| 嵌套支持 | AddGraphNode | AddGraphNode | AppendGraph |
| 适合场景 | 复杂拓扑、Pregel | 声明式数据流 + 字段映射 | 线性 pipeline |
