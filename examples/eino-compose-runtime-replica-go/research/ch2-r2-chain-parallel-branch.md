# R2 Chain·Parallel·Branch 研究笔记

> 基于 Eino 源码 `compose/chain.go`, `compose/chain_parallel.go`, `compose/chain_branch.go`, `compose/branch.go` 及对应测试文件。

---

## 1. Chain / Parallel / Branch 解决什么问题

### 1.1 问题场景

LLM 应用中常见的控制流编排场景：

| 场景 | 控制流 | 示例 |
|------|--------|------|
| 线性 pipeline | 顺序执行 A → B → C | prompt → chat model → 提取结果 |
| 并发调用 | 同时调多个模型/工具，合并结果 | 同时调 GPT-4o 和 Claude，对比输出 |
| 条件路由 | 根据中间结果走不同处理路径 | 意图识别 → 闲聊 / 知识问答 / 工具调用 |

这三个场景如果用底层 Graph API（`AddNode` + `AddEdge`）手写，会产生大量样板代码：

```go
// 如果用 Graph 手写：
g := NewGraph[I, O]()
g.AddLambdaNode("n0", ...)
g.AddLambdaNode("n1", ...)
g.AddEdge(START, "n0")
g.AddEdge("n0", "n1")
g.AddEdge("n1", END)
// 每个 Append 对应 1-3 行 Graph 操作
```

Chain 的目标是**用一个 Fluent Builder 消除这套样板**，同时内置 Parallel 和 Branch 的支持。

### 1.2 三类编排物的定位

| 维度 | Graph | Chain | Workflow |
|------|-------|-------|----------|
| 控制力 | 最高（手动管理一切） | 中低 | 中高 |
| 便利性 | 最低 | 最高 | 中 |
| 并行/分支 | 手工建多入边 / `AddBranch` | `AppendParallel` / `AppendBranch` 内建 | 通过多 `AddInput` / `AddBranch` |
| 字段映射 | 通过边 + FieldMapping | 自动（类型匹配即传递） | 内置在 `AddInput` 中 |
| 适合场景 | 复杂拓扑、Pregel 循环 | 线性/条件/并发 pipeline | 声明式数据流 + 字段映射 |

---

## 2. 为什么 Builder 尾部追踪和 Branch / Parallel 汇聚很困难

### 2.1 尾部追踪 (`preNodeKeys`) 的问题

Chain 的核心假设是**线性推进**：每个 `AppendX` 自动把新节点接到上一个（或上一组）节点后面。这在 `addNode`（`chain.go:560-600`）中实现：

```go
// 伪代码
func (c *Chain) addNode(node, options) {
    if len(c.preNodeKeys) == 0 {
        c.preNodeKeys = append(c.preNodeKeys, START) // 第一个节点连到 START
    }
    for _, preNodeKey := range c.preNodeKeys {
        c.gg.AddEdge(preNodeKey, nodeKey)  // 从所有 preNodeKeys 建边
    }
    c.preNodeKeys = []string{nodeKey}  // 新节点成为唯一尾部
}
```

**难点 1：单一尾部 vs 多重尾部**

- 线性 Append 后，`preNodeKeys` 只有一个元素（`node_0 → node_1 → node_2`）。
- `AppendParallel` 之后，`preNodeKeys` 变成 N 个元素（所有并行节点都是尾部）。
- `AppendBranch` 之后，`preNodeKeys` 变成 M 个元素（所有分支节点都是尾部）。

下一个 `AppendX` 必须处理**当前尾部是 1 个还是多个**的差异：

```go
// AppendParallel (chain.go:476-483)
var startNode string
if len(c.preNodeKeys) == 0 {
    startNode = START
} else if len(c.preNodeKeys) == 1 {
    startNode = c.preNodeKeys[0]
} else {
    c.reportError(...) // 多前驱时拒绝追加 Parallel
}
```

这揭示了一个隐含约束：**不能在 Parallel/Branch 之后再追加 Parallel/Branch**（除非用 `AppendPassthrough` 汇聚），因为 Parallel 要求单一前驱节点作为起点。

**难点 2：自动 END 连接**

编译时 `addEndIfNeeded`（`chain.go:98-121`）遍历 `preNodeKeys` 把所有尾部节点连到 END。如果用户在 Parallel 后没有添加汇聚节点（`AppendPassthrough` 或接受 `map[string]any` 的节点），所有并行节点都会被直接连到 END，可能产生非预期的行为。

### 2.2 Branch 汇聚的难点

**难点 3：分支节点 key 到 Graph 节点 key 的映射**

`ChainBranch` 内部使用分支 key（如 `"b1"`, `"chatPath"`）作为标识符，但 Graph 需要全局唯一的节点 key。`AppendBranch`（`chain.go:373-445`）做了三层映射：

```
分支 key "b1"  →  Graph 节点 key "node_1_branch_b1"
条件函数返回 "b1"  →  运行时查找 key2NodeKey["b1"]  →  路由到 "node_1_branch_b1"
```

**难点 4：条件函数的注入包装**

`GraphBranch.invoke` / `GraphBranch.collect` 被重新包装（`chain.go:397-433`），将分支 key 翻译为 Graph 节点 key：

```go
invokeCon := func(ctx context.Context, in any) (endNode []string, err error) {
    ends, err := b.internalBranch.invoke(ctx, in)
    // ends = ["b1"] 或 ["b2"]
    nodeKeyEnds := make([]string, 0, len(ends))
    for _, end := range ends {
        nodeKey, ok := key2NodeKey[end] // "b1" → "node_1_branch_b1"
        ...
        nodeKeyEnds = append(nodeKeyEnds, nodeKey)
    }
    return nodeKeyEnds, nil
}
```

**难点 5：多分支单分支**

Eino 支持两种分支模式：
- **单路径分支**（`NewChainBranch`）：条件返回一个 `endNode`，每次只选一条路径。
- **多路径分支**（`NewChainMultiBranch`）：条件返回 `map[string]bool`，可同时激活多条路径。

Branch 的 `endNodes` 校验（`NewGraphMultiBranch`, `branch.go:96-99`）会拒绝条件返回的 key 不在 `endNodes` 白名单中的情况：

```go
for end := range ends {
    if !endNodes[end] {
        return nil, fmt.Errorf("branch invocation returns unintended end node: %s", end)
    }
}
```

### 2.3 Parallel 汇聚的难点

**难点 6：输出 key 冲突检测**

`Parallel.addNode`（`chain_parallel.go:235-262`）在添加时检测 key 冲突：

```go
if _, ok := p.outputKeys[outputKey]; ok {
    p.err = fmt.Errorf("parallel add node err, duplicate output key= %s", outputKey)
    return p
}
```

**难点 7：所有并行节点共享同一个数据源**

在 `AppendParallel`（`chain.go:503`）中，所有并行节点都从 `startNode`（即前一个节点）获得数据：

```go
for i := range p.nodes {
    c.gg.addNode(nodeKey, ...)
    c.gg.AddEdge(startNode, nodeKey)  // 同一个 startNode → 每个并行节点
}
```

这意味着**并行节点的输入都相同**，输出通过各自的 `outputKey` 区分，下游通过 `map[string]any` 获取。

---

## 3. Eino 的解决方案模式与关键源码机制

### 3.1 Chain 的三段式生命周期

```
构建阶段 → 编译阶段 → 运行阶段
  │           │           │
  │      addEndIfNeeded   │
  │      展开子图          │
AppendX → gg.compile() → Runnable[I,O].Invoke()
```

### 3.2 Chain 核心结构 (`chain.go:72-82`)

```go
type Chain[I, O any] struct {
    err         error
    gg          *Graph[I, O]   // 内部包装一个 Graph
    nodeIdx     int            // 自动命名计数器
    preNodeKeys []string       // 当前尾部节点集合
    hasEnd      bool           // END 连接标志
}
```

关键设计：
- `gg *Graph[I,O]`：Chain 只是一个 Graph 的 **Builder 外观**，内部操作都委托给 Graph。
- `preNodeKeys`：自动追踪"当前链尾"，省去用户手工管理边的负担。
- `hasEnd`：确保 `addEndIfNeeded` 只执行一次（编译时调用）。

### 3.3 节点自动命名规则 (`chain.go:544-548`)

```go
func (c *Chain[I, O]) nextNodeKey() string {
    idx := c.nodeIdx
    c.nodeIdx++
    return fmt.Sprintf("node_%d", idx)
}
```

| 场景 | 生成的 key | 说明 |
|------|-----------|------|
| 普通节点 | `node_0`, `node_1`, `node_2` | 顺序递增 |
| Parallel 节点 | `node_0_parallel_0`, `node_0_parallel_1` | `{prefix}_parallel_{index}` |
| Branch 节点 | `node_1_branch_b1`, `node_1_branch_b2` | `{prefix}_branch_{branchKey}` |

用户可以通过 `WithNodeKey("customKey")` 覆盖自动命名。

### 3.4 错误延迟报告 (`chain.go:552-556`)

Chain 使用**先存后报**的错误模式，允许链式调用不因中间错误而 panic：

```go
func (c *Chain[I, O]) reportError(err error) {
    if c.err == nil {
        c.err = err  // 只保留第一个错误
    }
}
```

所有 `AppendX` 方法在开头检查 `c.err`，如果已有错误则直接返回（无效操作）。编译时 `addEndIfNeeded` 会返回该错误。

### 3.5 Parallel 核心结构 (`chain_parallel.go:49-53`)

```go
type Parallel struct {
    nodes      []nodeOptionsPair       // 节点列表
    outputKeys map[string]bool         // 输出 key 集合（去重校验）
    err        error
}
```

Parallel 的执行语义由底层 Graph 的多入边 + AllPredecessor 触发模式保证：
- 所有并行节点从前一个节点接收相同输入
- 所有并行节点完成后，下游节点被触发（AllPredecessor）
- 并行节点输出通过 `outputKey` 标注，下游接收 `map[string]any`

### 3.6 ChainBranch 核心结构 (`chain_branch.go:38-42`)

```go
type ChainBranch struct {
    internalBranch *GraphBranch                 // 底层分支对象
    key2BranchNode map[string]nodeOptionsPair   // 分支 key → (graphNode, options)
    err            error
}
```

四种构造函数：

| 函数 | 分支模式 | 输入模式 |
|------|---------|---------|
| `NewChainBranch[T](cond)` | 单路径 | Invoke |
| `NewChainMultiBranch[T](cond)` | 多路径 | Invoke |
| `NewStreamChainBranch[T](cond)` | 单路径 | Stream (collect) |
| `NewStreamChainMultiBranch[T](cond)` | 多路径 | Stream (collect) |

`NewChainBranch` 是对 `NewChainMultiBranch` 的包装（`chain_branch.go:100-108`）：

```go
func NewChainBranch[T any](cond GraphBranchCondition[T]) *ChainBranch {
    return NewChainMultiBranch(func(ctx context.Context, in T) (endNode map[string]bool, err error) {
        ret, err := cond(ctx, in)
        return map[string]bool{ret: true}, nil
    })
}
```

### 3.7 GraphBranch 核心结构 (`branch.go:42-50`)

```go
type GraphBranch struct {
    invoke    func(ctx context.Context, input any) (output []string, err error)
    collect   func(ctx context.Context, input streamReader) (output []string, err error)
    inputType reflect.Type
    *genericHelper
    endNodes   map[string]bool    // 合法出口白名单
    idx        int                // 并行分支索引
    noDataFlow bool               // Workflow 分支标记
}
```

`invoke`/`collect` 双函数设计支持 Invoke 和 Stream 两种执行模式。`endNodes` 白名单在校验和 ChainBuilder 包装中起关键作用。

### 3.8 AppendBranch 完整流程 (`chain.go:342-447`)

1. **校验**：Branch 非 nil、内部错误为空、至少 2 个分支节点
2. **确定起点**：从 `preNodeKeys` 获取单一起点（拒绝多前驱）
3. **注册节点**：为每个分支节点生成 Graph key 命名空间（加前缀避免冲突）
4. **包装条件函数**：将分支 key 映射到 Graph 节点 key
5. **设置 endNodes**：`gslice.ToMap(...)` 构建白名单
6. **添加分支到 Graph**：`c.gg.AddBranch(startNode, &gBranch)`
7. **更新尾部**：`c.preNodeKeys = gmap.Values(key2NodeKey)`

### 3.9 AppendParallel 完整流程 (`chain.go:459-514`)

1. **校验**：Parallel 非 nil、内部错误为空、至少 2 个节点
2. **确定起点**：从 `preNodeKeys` 获取单一起点
3. **注册所有节点**：使用 `{prefix}_parallel_{i}` 命名
4. **从起点到每个节点建边**：`c.gg.AddEdge(startNode, nodeKey)`
5. **更新尾部**：`c.preNodeKeys = nodeKeys`（多个并行节点成为新尾部）

### 3.10 AppendPassthrough (`chain.go:533-537`)

Passthrough 是一个透传节点，解决 Branch/Parallel 后的**汇聚**问题：

```go
chain.AppendBranch(cb).AppendPassthrough().AppendParallel(p)
```

Passthrough 不需要用户指定输入输出类型——它在编译时通过 `toValidateMap` BFS 推断机制自动推导出类型为 `passthrough[T]`。

### 3.11 AppendGraph — 子 Chain 嵌套 (`chain.go:522-526`)

```go
func (c *Chain[I, O]) AppendGraph(node AnyGraph, opts ...GraphAddNodeOpt) *Chain[I, O] {
    gNode, options := toAnyGraphNode(node, opts...)
    c.addNode(gNode, options)
    return c
}
```

通过 `AnyGraph` 接口实现子 Chain/Graph 的递归嵌套，编译时 `compileIfNeeded` 递归编译子图。

---

## 4. Go Replica 应实现什么、明确跳过什么

### 4.1 应该实现（Implement）

| 组件 | 优先级 | 理由 |
|------|--------|------|
| `Chain[I, O]` 结构 + Builder API | P0 | Chain 是最高频使用的 API，消除样板代码 |
| `addNode` + `preNodeKeys` 自动追踪 | P0 | Chain 的核心机制 |
| `nextNodeKey` 自动命名 | P0 | 减少用户心智负担 |
| `addEndIfNeeded` 自动 END 连接 | P0 | 编译时完成的隐式操作 |
| `reportError` 错误延迟报告 | P0 | 链式调用友好 |
| `Compile` → `Runnable[I,O]` | P0 | 统一编译目标 |
| `Parallel` 结构 + Add* 方法 | P0 | 并发调用是 LLM 应用常见场景 |
| `ChainBranch` + 四种构造函数 | P0 | 条件路由是核心功能 |
| `GraphBranch` 基本结构 | P0 | Branch 底层实现 |
| `AppendPassthrough` | P0 | Branch/Parallel 汇聚的必需品 |
| `AppendGraph` 子图嵌套 | P0 | 子 Chain 嵌套复用 |
| `AppendLambda` | P0 | 自定义逻辑实现 |
| `WithNodeKey` / `WithOutputKey` | P1 | 节点命名和并行输出标记 |
| `AppendChatTemplate` / `AppendChatModel` | P1 | 通过 component bridge 映射 |

### 4.2 应该跳过（Skip / Defer）

| 项目 | 理由 | 目标版本 |
|------|------|---------|
| `AppendAgenticModel` / `AppendAgenticChatTemplate` | 依赖 AgenticModel / AgenticChatTemplate 接口，R1 component 接口可能不完整 | R2 |
| `AppendAgenticToolsNode` | 同上 | R2 |
| `AppendEmbedding` / `AppendRetriever` / `AppendLoader` / `AppendIndexer` / `AppendDocumentTransformer` | RAG 组件，R1 仅定义接口 | R3 |
| Stream ChainBranch 的完整 collect 实现 | StreamReader 机制需要 schema 完整实现 | R1 做 Stub |
| MultiBranch 的 `WithOutputKey` + map 结果合并 | 依赖 StreamReader Merge 机制 | R1 基础路径 |
| Component 特定的 `toChatModelNode` / `toToolsNode` 等 | R1 component bridge 可能简化 | R1 |
| 编译时的类型推断（`toValidateMap` / `updateToValidateMap`） | R1 TODO，先要求显式类型 | R2 |
| 编译时 FieldMapping 验证 | R1 TODO | R2 |
| Workflow 抽象 | ch2 另有笔记 | R2 |

### 4.3 实现原则

1. **从 `map[string]any` 开始**：R1 的 Chain 不必支持泛型推断，第一版用 `map[string]any` 作为输入输出类型，简化类型系统。
2. **保留 Chain 的 Graph 委托结构**：`Chain.gg *Graph[I,O]` 作为底层，所有操作最终落到 Graph 上。
3. **错误处理采用先存后报**：与 Eino 一致，避免 panic。
4. **节点命名空间自动管理**：前缀隔离避免 Parallel / Branch 嵌套时的 key 冲突。
5. **编译边界不可变性**：`Compile()` 后设置 `hasEnd = true`，所有 AppendX 检查之。

---

## 5. 具体 API 草稿与测试用例

### 5.1 API 草稿

```go
// ========== Chain ==========

// NewChain creates a new Chain builder.
// In R1, I and O are constrained to map[string]any for simplicity.
func NewChain[I, O any]() *Chain[I, O]

func (c *Chain[I, O]) AppendLambda(lambda *Lambda, opts ...GraphAddNodeOpt) *Chain[I, O]
func (c *Chain[I, O]) AppendGraph(graph AnyGraph, opts ...GraphAddNodeOpt) *Chain[I, O]
func (c *Chain[I, O]) AppendPassthrough(opts ...GraphAddNodeOpt) *Chain[I, O]
func (c *Chain[I, O]) AppendParallel(p *Parallel) *Chain[I, O]
func (c *Chain[I, O]) AppendBranch(b *ChainBranch) *Chain[I, O]
func (c *Chain[I, O]) Compile(ctx context.Context, opts ...GraphCompileOption) (Runnable[I, O], error)


// ========== Parallel ==========

func NewParallel() *Parallel

func (p *Parallel) AddLambda(outputKey string, node *Lambda, opts ...GraphAddNodeOpt) *Parallel
func (p *Parallel) AddGraph(outputKey string, node AnyGraph, opts ...GraphAddNodeOpt) *Parallel
func (p *Parallel) AddPassthrough(outputKey string, opts ...GraphAddNodeOpt) *Parallel


// ========== ChainBranch ==========

type GraphBranchCondition[T any] func(ctx context.Context, in T) (endNode string, err error)
type GraphMultiBranchCondition[T any] func(ctx context.Context, in T) (endNode map[string]bool, err error)

func NewChainBranch[T any](cond GraphBranchCondition[T]) *ChainBranch
func NewChainMultiBranch[T any](cond GraphMultiBranchCondition[T]) *ChainBranch

func (cb *ChainBranch) AddLambda(key string, node *Lambda, opts ...GraphAddNodeOpt) *ChainBranch
func (cb *ChainBranch) AddGraph(key string, node AnyGraph, opts ...GraphAddNodeOpt) *ChainBranch
func (cb *ChainBranch) AddPassthrough(key string, opts ...GraphAddNodeOpt) *ChainBranch
```

### 5.2 测试用例

#### TC-01: 基础线性 Chain

```go
func TestChainLinear(t *testing.T) {
    chain := NewChain[string, string]()

    chain.
        AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return strings.ToUpper(in), nil
        })).
        AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return "[" + in + "]", nil
        }))

    r, err := chain.Compile(context.Background())
    require.NoError(t, err)

    out, err := r.Invoke(context.Background(), "hello")
    require.NoError(t, err)
    assert.Equal(t, "[HELLO]", out)
}
```

#### TC-02: 带 Parallel 的 Chain

```go
func TestChainParallel(t *testing.T) {
    chain := NewChain[string, map[string]any]()

    parallel := NewParallel()
    parallel.
        AddLambda("upper", InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return strings.ToUpper(in), nil
        })).
        AddLambda("lower", InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return strings.ToLower(in), nil
        }))

    chain.
        AppendPassthrough().
        AppendParallel(parallel)

    r, err := chain.Compile(context.Background())
    require.NoError(t, err)

    out, err := r.Invoke(context.Background(), "Hello")
    require.NoError(t, err)
    // out = {"upper": "HELLO", "lower": "hello"}
    assert.Equal(t, "HELLO", out["upper"])
    assert.Equal(t, "hello", out["lower"])
}
```

#### TC-03: 带 Branch 的 Chain（汇聚到 Passthrough）

```go
func TestChainBranch(t *testing.T) {
    chain := NewChain[string, string]()

    branchCond := func(ctx context.Context, in string) (string, error) {
        if len(in) > 5 {
            return "long", nil
        }
        return "short", nil
    }

    chain.
        AppendBranch(NewChainBranch(branchCond).
            AddLambda("long", InvokableLambda(func(ctx context.Context, in string) (string, error) {
                return "LONG:" + in, nil
            })).
            AddLambda("short", InvokableLambda(func(ctx context.Context, in string) (string, error) {
                return "SHORT:" + in, nil
            })),
        ).
        AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return "result=" + in, nil
        }))

    r, err := chain.Compile(context.Background())
    require.NoError(t, err)

    out, err := r.Invoke(context.Background(), "hello-world")
    require.NoError(t, err)
    assert.Equal(t, "result=LONG:hello-world", out)

    out, err = r.Invoke(context.Background(), "hi")
    require.NoError(t, err)
    assert.Equal(t, "result=SHORT:hi", out)
}
```

#### TC-04: MultiBranch（多路径同时激活）

```go
func TestChainMultiBranch(t *testing.T) {
    chain := NewChain[string, map[string]any]()

    cond := func(ctx context.Context, in string) (map[string]bool, error) {
        // 同时路由到 path_a 和 path_b
        return map[string]bool{"path_a": true, "path_b": true}, nil
    }

    chain.
        AppendBranch(NewChainMultiBranch(cond).
            AddLambda("path_a", InvokableLambda(func(ctx context.Context, in string) (string, error) {
                return "A:" + in, nil
            }), WithOutputKey("path_a")).
            AddLambda("path_b", InvokableLambda(func(ctx context.Context, in string) (string, error) {
                return "B:" + in, nil
            }), WithOutputKey("path_b")).
            AddLambda("path_c", InvokableLambda(func(ctx context.Context, in string) (string, error) {
                return "C:" + in, nil
            }), WithOutputKey("path_c")),
        )

    r, err := chain.Compile(context.Background())
    require.NoError(t, err)

    out, err := r.Invoke(context.Background(), "hello")
    require.NoError(t, err)
    // path_c 未被激活，不应出现
    assert.Equal(t, "A:hello", out["path_a"])
    assert.Equal(t, "B:hello", out["path_b"])
    assert.NotContains(t, out, "path_c")
}
```

#### TC-05: Branch → Parallel → Lambda 组合

```go
func TestChainBranchThenParallel(t *testing.T) {
    chain := NewChain[map[string]any, map[string]any]()

    // 意图识别 → 分支路由
    branchCond := func(ctx context.Context, in map[string]any) (string, error) {
        intent, ok := in["intent"].(string)
        if !ok {
            return "default", nil
        }
        return intent, nil
    }

    // chat 分支：并行调两个模型
    chatParallel := NewParallel()
    chatParallel.
        AddLambda("model_a", InvokableLambda(func(ctx context.Context, in map[string]any) (string, error) {
            return "chat_response_from_A", nil
        })).
        AddLambda("model_b", InvokableLambda(func(ctx context.Context, in map[string]any) (string, error) {
            return "chat_response_from_B", nil
        }))

    chatSubChain := NewChain[map[string]any, map[string]any]()
    chatSubChain.AppendParallel(chatParallel)

    chain.
        AppendLambda(InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
            // 预处理
            return in, nil
        })).
        AppendBranch(NewChainBranch(branchCond).
            AddGraph("chat", chatSubChain).
            AddLambda("default", InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
                in["route"] = "default"
                return in, nil
            })),
        ).
        AppendPassthrough().
        AppendLambda(InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
            // 后处理：统一输出
            return in, nil
        }))

    r, err := chain.Compile(context.Background())
    require.NoError(t, err)

    out, err := r.Invoke(context.Background(), map[string]any{"intent": "chat", "query": "hello"})
    require.NoError(t, err)
    assert.Equal(t, "chat_response_from_A", out["model_a"])
    assert.Equal(t, "chat_response_from_B", out["model_b"])
}
```

#### TC-06: 错误情况

```go
func TestChainErrors(t *testing.T) {
    // 空 chain 编译失败
    chain := NewChain[string, string]()
    _, err := chain.Compile(context.Background())
    assert.Error(t, err)

    // 空 parallel 追加失败
    chain = NewChain[string, string]()
    parallel := NewParallel()
    chain.AppendParallel(parallel)
    _, err = chain.Compile(context.Background())
    assert.Error(t, err) // "not enough nodes, count = 0"

    // 单节点 branch 失败
    chain = NewChain[string, string]()
    chain.AppendBranch(NewChainBranch[string](func(ctx context.Context, in string) (string, error) {
        return "only", nil
    }).AddLambda("only", InvokableLambda(func(ctx context.Context, in string) (string, error) {
        return in, nil
    })))
    _, err = chain.Compile(context.Background())
    assert.Error(t, err) // "nodeList length = 1"

    // 重复 output key 在 parallel 中
    p := NewParallel()
    p.AddLambda("dup", someLambda)
    p.AddLambda("dup", someLambda)
    assert.NotNil(t, p.err) // "duplicate output key= dup"

    // nil condition 在 branch 中
    chain = NewChain[string, string]()
    chain.AppendBranch(NewChainBranch[string](nil))
    assert.NotNil(t, chain.err)
}
```

#### TC-07: 子 Chain 嵌套

```go
func TestChainNestedGraph(t *testing.T) {
    // 构建一个子 Chain
    subChain := NewChain[string, string]()
    subChain.
        AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return "sub:" + in, nil
        }))

    // 父 Chain 引用子 Chain
    parentChain := NewChain[string, string]()
    parentChain.
        AppendLambda(InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return "pre:" + in, nil
        })).
        AppendGraph(subChain)

    r, err := parentChain.Compile(context.Background())
    require.NoError(t, err)

    out, err := r.Invoke(context.Background(), "hello")
    require.NoError(t, err)
    assert.Equal(t, "sub:pre:hello", out)
}
```

---

## 关键源码索引

| 内容 | 文件:行号 | 
|------|----------|
| Chain 结构定义 | `chain.go:72-82` |
| `addNode`（尾部追踪核心） | `chain.go:560-600` |
| `addEndIfNeeded`（自动 END） | `chain.go:98-121` |
| `reportError`（错误延迟） | `chain.go:552-556` |
| `nextNodeKey`（自动命名） | `chain.go:544-548` |
| `AppendBranch` 完整实现 | `chain.go:342-447` |
| `AppendParallel` 完整实现 | `chain.go:459-514` |
| `AppendPassthrough` | `chain.go:533-537` |
| `AppendGraph` | `chain.go:522-526` |
| `AppendLambda` | `chain.go:266-270` |
| `Compile` | `chain.go:157-163` |
| Parallel 结构 | `chain_parallel.go:49-53` |
| `Parallel.addNode`（key 冲突检测） | `chain_parallel.go:235-262` |
| ChainBranch 结构 | `chain_branch.go:38-42` |
| `NewChainBranch` | `chain_branch.go:100-108` |
| `NewChainMultiBranch` | `chain_branch.go:46-63` |
| `NewStreamChainBranch` | `chain_branch.go:123-131` |
| `NewStreamChainMultiBranch` | `chain_branch.go:67-83` |
| GraphBranch 结构 | `branch.go:42-50` |
| `NewGraphBranch` | `branch.go:145-153` |
| `NewGraphMultiBranch` | `branch.go:89-107` |
| `newGraphBranch`（invoke/collect 双函数） | `branch.go:57-85` |
| TestChain（完整例子） | `chain_test.go:37-110` |
| TestChainBranch（分支测试） | `chain_branch_test.go:35-274` |
| TestChainMultiBranch | `chain_branch_test.go:276-311` |
