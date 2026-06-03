# 第二章实现契约：Workflow / Chain / FieldMapping / Parallel / Branch

> 基于 R1/R2/R3 研究笔记 + Eino 技术手册第二章
> 目标读者：实施工人 I1/I2/I3/I4
> 语言：中文

---

## 目录

1. [总体目标与范围](#1-总体目标与范围)
2. [API 契约：FieldMapping](#2-api-契约fieldmapping)
3. [API 契约：Workflow](#3-api-契约workflow)
4. [API 契约：Chain / Parallel / ChainBranch](#4-api-契约chain--parallel--chainbranch)
5. [运行时语义与跳过的非目标](#5-运行时语义与跳过的非目标)
6. [与现有 Graph / Runner / Channel 的集成点](#6-与现有-graph--runner--channel-的集成点)
7. [文件归属分配（I1/I2/I3/I4）](#7-文件归属分配-i1i2i3i4)
8. [测试矩阵](#8-测试矩阵)
9. [代码示例](#9-代码示例)
10. [已知风险与约束](#10-已知风险与约束)

---

## 1. 总体目标与范围

本章目标：在现有 `compose` 复刻版（已实现 Graph/DAG/Pregel/Runner/Channel/EventLog）之上，添加三层编排抽象及字段映射基础设施，使其具备与 Eino 等价的声明式图编排能力。

实现完成后，以下 API 应可正常编译、运行并通过测试：

```go
// FieldMapping: 字段级数据提取/注入
MapFields("Query", "query")
FromFieldPath(FieldPath{"user", "profile", "name"})

// Workflow: 声明式 AddInput 替代 AddEdge
wf.AddLambdaNode("step1", ...).AddInput(START, FromField("Query"))

// Chain: Builder 风格的线性构造
NewChain[string, string]().AppendLambda(...).AppendParallel(...).Compile(ctx)
```

### 1.1 三层抽象定位

| 维度 | Graph | Workflow | Chain |
|------|-------|----------|-------|
| 控制力 | 最高（手动 AddEdge） | 中等（声明式 AddInput） | 最低（自动 AppendX） |
| 便利性 | 最低 | 中等 | 最高 |
| 字段映射 | 通过边 + FieldMapping（手动） | 内置在 AddInput | 自动（类型匹配即传） |
| 并行/分支 | 手工多入边 / AddBranch | 多 AddInput / AddBranch | AppendParallel / AppendBranch 内建 |
| 适合场景 | 复杂拓扑、Pregel 循环 | 声明式数据流 + 字段映射 | 线性/条件/并发 pipeline |

---

## 2. API 契约：FieldMapping

### 2.1 新增文件

**文件**: `compose/field_mapping.go`

### 2.2 类型定义

```go
package compose

import "reflect"

// FieldPath 表示嵌套字段路径。每个元素是 struct 字段名或 map key。
type FieldPath []string

// FieldMapping 描述一个字段级别的数据传递。
// from 和 to 字段内部使用 \x1F (Unit Separator) 作为嵌套路径分隔符。
type FieldMapping struct {
    fromNodeKey    string
    from           string   // 空表示整个前驱输出（FromAll）
    to             string   // 空表示后继的整个输入（ToAll）
    customExtractor func(input any) (any, error)
}

// 六个构造器
func MapFields(from, to string) *FieldMapping
func FromField(from string) *FieldMapping
func ToField(to string, opts ...FieldMappingOption) *FieldMapping
func MapFieldPaths(fromPath, toPath FieldPath) *FieldMapping
func FromFieldPath(fromPath FieldPath) *FieldMapping
func ToFieldPath(toPath FieldPath, opts ...FieldMappingOption) *FieldMapping

// 选项
type FieldMappingOption func(*FieldMapping)
func WithCustomExtractor(extractor func(input any) (any, error)) FieldMappingOption

// 访问器
func (m *FieldMapping) FromNodeKey() string
func (m *FieldMapping) FromPath() FieldPath     // splitFieldPath(m.from)
func (m *FieldMapping) ToPath() FieldPath       // splitFieldPath(m.to)
func (m *FieldMapping) TargetPath() FieldPath   // 同 ToPath
func (m *FieldMapping) IsFromAll() bool         // m.from == ""
func (m *FieldMapping) IsToAll() bool           // m.to == ""
func (m *FieldMapping) HasCustomExtractor() bool // m.customExtractor != nil
```

### 2.3 内部函数（不导出，供 FieldMapping/Workflow 内部使用）

```go
// 路径分隔符常量
const fieldPathSeparator = "\x1F"

// FieldPath ↔ string 转换
func splitFieldPath(path string) FieldPath
func (fp FieldPath) join() string

// 逐段类型提取：沿路径穿透指针、进入 struct field、进入 map value
// 遇到 interface 类型时返回剩余的未检查路径（推迟到请求时处理）
func checkAndExtractFieldType(paths []string, typ reflect.Type) (
    extracted reflect.Type,
    remaining FieldPath,
    err error,
)

// 编译时验证：检查 6 个 mapping 的字段存在性、类型赋值兼容性
// 返回: typeHandler (推迟检查), uncheckedSourcePath (interface 中间路径), err
func validateFieldMapping(
    predecessorType, successorType reflect.Type,
    mappings []*FieldMapping,
) (
    typeHandler *handlerPair,
    uncheckedSourcePath map[string]FieldPath,
    err error,
)

// 请求时映射执行：从输入值中按 mapping 规则提取字段，组包为 map[string]any
// allowMapKeyNotFound: true 时 map key 不存在则跳过；false 时报错
func fieldMap(
    mappings []*FieldMapping,
    allowMapKeyNotFound bool,
    uncheckedSourcePaths map[string]FieldPath,
) func(input any) (map[string]any, error)

// 流式映射（本次标记为 TODO，暂不实现核心逻辑）
func streamFieldMap(
    mappings []*FieldMapping,
    uncheckedSourcePaths map[string]FieldPath,
) func(streamReader) streamReader

// 从 struct/map 中按路径提取单个值
func takeOne(inputValue reflect.Value, inputType reflect.Type, from string) (
    taken any, takenType reflect.Type, err error,
)

// 将单个值写入 dest struct/map 的目标路径
func assignOne(destValue reflect.Value, taken any, to string) reflect.Value

// 将 map[string]any 中间结果转换为实际目标类型（struct 或 map）
func convertTo(mappings map[string]any, typ reflect.Type) any
```

### 2.4 类型赋值分类（`validateFieldMapping` 内部）

```go
// 三个赋值等级：
// assignableTypeMust:    编译时必须可赋值，否则报错
// assignableTypeMustNot: 编译时必须不可赋值，否则报错
// assignableTypeMay:     编译时不强制，推迟到请求时检查（纳入 handlerPair）
```

### 2.5 内部错误定义（不导出）

```go
// 在 field_mapping.go 中定义两个哨兵错误：
var errMapKeyNotFound = errors.New("map key not found")
var errInterfaceNotValidForFieldMapping = errors.New("interface not valid for field mapping")

// 在 fieldMap 及其他函数中使用 errors.Is 进行类型断言。
```

### 2.6 handlerPair 扩展（需要新增字段）

在 `compose/utils.go` 或 `compose/types.go` 中新增 `handlerPair` 类型：

```go
// handlerPair 用于推迟到请求时的类型检查
type handlerPair struct {
    // checker 在请求时执行最终的类型验证
    checker func(invoke func(any) (any, error)) error
}
```

---

## 3. API 契约：Workflow

### 3.1 新增文件

**文件**: `compose/workflow.go`

### 3.2 类型定义

```go
package compose

import "context"

// dependencyType 表示 Workflow 节点间依赖类型
type dependencyType int

const (
    normalDependency   dependencyType = 1  // 数据 + 执行依赖
    noDirectDependency dependencyType = 2  // 仅数据依赖（不建立直接执行依赖）
    branchDependency   dependencyType = 3  // 分支依赖
)

// WorkflowNode 代表 Workflow 中的一个节点，支持链式声明
type WorkflowNode struct {
    g                *graph
    key              string
    addInputs        []func() error       // 延迟执行的 AddInput 闭包
    staticValues     map[string]any       // 编译时静态值
    dependencySetter func(fromNodeKey string, typ dependencyType)
    mappedFieldPath  map[string]any       // 已映射路径 trie（防冲突）
}

// WorkflowBranch 表示 Workflow 中的分支
type WorkflowBranch struct {
    fromNodeKey string
    *GraphBranch
}

// Workflow[I, O] 是 Graph 的声明式包装，通过 AddInput + FieldMapping 替代 AddEdge
type Workflow[I, O any] struct {
    g                *graph
    workflowNodes    map[string]*WorkflowNode
    workflowBranches []*WorkflowBranch
    dependencies     map[string]map[string]dependencyType
}
```

### 3.3 构造函数

```go
// NewWorkflow 创建新的 Workflow，底层图使用 AllPredecessor 触发模式
func NewWorkflow[I, O any]() *Workflow[I, O]
```

### 3.4 Workflow 核心方法

```go
// --- 添加节点（返回 *WorkflowNode 供链式操作）---

func (wf *Workflow[I, O]) AddLambdaNode(key string, lambda *Lambda) *WorkflowNode
func (wf *Workflow[I, O]) AddGraphNode(key string, graph AnyGraph) *WorkflowNode
func (wf *Workflow[I, O]) AddPassthroughNode(key string) *WorkflowNode

// --- 分支 ---

func (wf *Workflow[I, O]) AddBranch(fromNodeKey string, branch *GraphBranch) *WorkflowBranch

// --- 终端 ---

func (wf *Workflow[I, O]) End() *WorkflowNode

// --- 编译 ---

// Compile 编译 Workflow 为 Runnable[I, O]
// 编译阶段：先收集所有分支信息 → 统一执行 addInputs → 注入静态值 → 委托 graph.compile
func (wf *Workflow[I, O]) Compile(ctx context.Context) (Runnable[I, O], error)
```

### 3.5 WorkflowNode 核心方法

```go
// AddInput 声明从一个前驱节点获取数据 + 建立执行依赖（默认行为）
// inputs 为空时表示获取前驱全部输出
func (n *WorkflowNode) AddInput(fromNodeKey string, inputs ...*FieldMapping) *WorkflowNode

// AddInputWithOptions 带选项的输入声明
func (n *WorkflowNode) AddInputWithOptions(
    fromNodeKey string,
    inputs []*FieldMapping,
    opts ...WorkflowAddInputOpt,
) *WorkflowNode

// AddDependency 声明仅执行依赖，不传递数据
func (n *WorkflowNode) AddDependency(fromNodeKey string) *WorkflowNode

// SetStaticValue 设置在编译时注入到节点输入的静态值
func (n *WorkflowNode) SetStaticValue(path FieldPath, value any) *WorkflowNode
```

### 3.6 选项

```go
// WorkflowAddInputOpt 是 AddInputWithOptions 的选项函数类型
type WorkflowAddInputOpt func(*workflowAddInputOpts)

// WithNoDirectDependency 取消直接执行依赖，仅保留数据依赖
// 注意：调用者必须确保存在间接路径保证前驱在该节点之前完成
func WithNoDirectDependency() WorkflowAddInputOpt

// workflowAddInputOpts 内部选项聚合（不导出）
type workflowAddInputOpts struct {
    noDirectDependency     bool
    dependencyWithoutInput bool
}
```

### 3.7 `addDependencyRelation` 内部逻辑规范

`addDependencyRelation(fromNodeKey, inputs, opts)` 是 WorkflowNode 所有依赖声明的统一入口：

```
1. 若 dependencyWithoutInput = true（AddDependency 路径）：
   - 校验 inputs 必须为空
   - 调用 g.addEdgeWithMappings(fromNodeKey, n.key, false, true, nil)
     (noDirectDependency=false, isControl=true, 无数据)
   - 设置 normalDependency

2. 若 noDirectDependency = true（WithNoDirectDependency 路径）：
   - 校验字段路径不冲突（checkAndAddMappedPath）
   - 调用 g.addEdgeWithMappings(fromNodeKey, n.key, true, false, inputs)
     (noDirectDependency=true, isControl=false, 有数据)
   - 设置 noDirectDependency

3. 默认模式（AddInput 默认路径）：
   - 校验字段路径不冲突（checkAndAddMappedPath）
   - 调用 g.addEdgeWithMappings(fromNodeKey, n.key, false, false, inputs)
     (noDirectDependency=false, isControl=false, 有数据)
   - 设置 normalDependency
```

### 3.8 Workflow.compile 两阶段编译流程

```
compile(ctx):
  1. 阶段一：收集分支信息
     for each wb in workflowBranches:
       - 为每个分支目标注入 branchDependency 到 dependencies 表
       - 调用 g.addBranch(wb.fromNodeKey, wb.GraphBranch, true)
         (第三个参数 noDataFlow=true，Workflow 分支不传递数据)

  2. 阶段二：执行延迟的 addInputs 闭包
     for each node in workflowNodes:
       for each addInput in node.addInputs:
         err := addInput()  // 闭包内部调用 g.addEdgeWithMappings
       node.addInputs = nil

  3. 阶段三：注入静态值
     for each node with staticValues:
       - 构造 handlerPair，通过 handlerPreNode 注入
       - handler 内部用 mergeValues 合并静态值与动态输入

  4. 委托底层 g.compile(ctx)
```

### 3.9 `checkAndAddMappedPath` — 路径冲突检测

```go
// checkAndAddMappedPath 使用嵌套 map[string]any 作为 trie 记录已映射路径
// "" key 值为 struct{} → 整个输入已被独占
// "" key 值为 map[string]any → 部分字段已映射
// 返回 error 当且仅当两个 mapping 的终端路径重叠
func (n *WorkflowNode) checkAndAddMappedPath(paths []string) error
```

错误信息格式: `"two terminal field paths conflict for node %s: %v, %v"`

---

## 4. API 契约：Chain / Parallel / ChainBranch

### 4.1 新增文件

| 文件 | 说明 |
|------|------|
| `compose/chain.go` | Chain[I,O] Builder 主结构、addNode、preNodeKeys 追踪、addEndIfNeeded |
| `compose/chain_parallel.go` | Parallel 并行节点组 |
| `compose/chain_branch.go` | ChainBranch 分支封装、四种构造函数 |

### 4.2 Chain API

```go
// compose/chain.go

// Chain[I, O] 提供线性 Builder 风格的图构造，内部包装一个 Graph
type Chain[I, O any] struct {
    err         error
    gg          *Graph[I, O]    // 内部包装的泛型图
    nodeIdx     int             // 自动命名计数器
    preNodeKeys []string        // 当前尾部节点集合
    hasEnd      bool            // END 连接标志
}

func NewChain[I, O any]() *Chain[I, O]

// --- Append 系列 ---

func (c *Chain[I, O]) AppendLambda(lambda *Lambda, opts ...GraphAddNodeOpt) *Chain[I, O]
func (c *Chain[I, O]) AppendGraph(graph AnyGraph, opts ...GraphAddNodeOpt) *Chain[I, O]
func (c *Chain[I, O]) AppendPassthrough(opts ...GraphAddNodeOpt) *Chain[I, O]
func (c *Chain[I, O]) AppendParallel(p *Parallel) *Chain[I, O]
func (c *Chain[I, O]) AppendBranch(b *ChainBranch) *Chain[I, O]

// --- 编译 ---

func (c *Chain[I, O]) Compile(ctx context.Context) (Runnable[I, O], error)

// --- 内部 ---

// addNode 是链式构建核心：从所有 preNodeKeys 建边到新节点，新节点成为唯一尾部
func (c *Chain[I, O]) addNode(node *graphNode, opts []GraphAddNodeOpt)

// addEndIfNeeded 在编译前将所有 preNodeKeys 连接到 END
func (c *Chain[I, O]) addEndIfNeeded() error

// nextNodeKey 自动节点命名：node_0, node_1, node_0_parallel_0, node_1_branch_custom
func (c *Chain[I, O]) nextNodeKey() string

// reportError 先存后报错误，允许链式调用不因中间错误而 panic
func (c *Chain[I, O]) reportError(err error)
```

### 4.3 `addNode` 内部行为规范

```
addNode(node, options):
  1. 如 preNodeKeys 为空：preNodeKeys = [START]（第一个节点自动连接 START）
  2. for each preNodeKey in preNodeKeys:
       c.gg.AddEdge(preNodeKey, nodeKey)
  3. preNodeKeys = [nodeKey]（新节点成为唯一尾部）
  4. nodeIdx++
```

### 4.4 Parallel API

```go
// compose/chain_parallel.go

// Parallel 是一组并行执行的节点的集合
// 所有并行节点共享同一个前驱的数据，输出通过 outputKey 标注
type Parallel struct {
    nodes      []nodeOptionsPair    // 节点列表
    outputKeys map[string]bool      // 输出 key 集合（去重校验）
    err        error
}

func NewParallel() *Parallel

// 至少需要 2 个节点；outputKey 不可重复
func (p *Parallel) AddLambda(outputKey string, node *Lambda, opts ...GraphAddNodeOpt) *Parallel
func (p *Parallel) AddGraph(outputKey string, node AnyGraph, opts ...GraphAddNodeOpt) *Parallel
func (p *Parallel) AddPassthrough(outputKey string, opts ...GraphAddNodeOpt) *Parallel
func (p *Parallel) Error() error  // 返回 p.err
```

### 4.5 ChainBranch API

```go
// compose/chain_branch.go

// GraphBranchCondition / GraphMultiBranchCondition
type GraphBranchCondition[T any] func(ctx context.Context, in T) (endNode string, err error)
type GraphMultiBranchCondition[T any] func(ctx context.Context, in T) (endNode map[string]bool, err error)

// ChainBranch 封装 GraphBranch + 分支节点映射表
type ChainBranch struct {
    internalBranch *GraphBranch
    key2BranchNode map[string]nodeOptionsPair
    err            error
}

// NewChainBranch 单路径分支：条件函数返回单个 endNode key
func NewChainBranch[T any](cond GraphBranchCondition[T]) *ChainBranch

// NewChainMultiBranch 多路径分支：条件函数返回 map[key]bool，可同时激活多条路径
func NewChainMultiBranch[T any](cond GraphMultiBranchCondition[T]) *ChainBranch

// NewStreamChainBranch 单路径流式分支（本次做 Stub，内部复用 NewChainBranch）
func NewStreamChainBranch[T any](cond GraphBranchCondition[T]) *ChainBranch

// NewStreamChainMultiBranch 多路径流式分支（本次做 Stub，内部复用 NewChainMultiBranch）
func NewStreamChainMultiBranch[T any](cond GraphMultiBranchCondition[T]) *ChainBranch

// 添加分支节点
func (cb *ChainBranch) AddLambda(key string, node *Lambda, opts ...GraphAddNodeOpt) *ChainBranch
func (cb *ChainBranch) AddGraph(key string, node AnyGraph, opts ...GraphAddNodeOpt) *ChainBranch
func (cb *ChainBranch) AddPassthrough(key string, opts ...GraphAddNodeOpt) *ChainBranch
func (cb *ChainBranch) Error() error  // 返回 cb.err
```

### 4.6 `NewChainBranch` 实现规范

```go
// NewChainBranch 是对 NewChainMultiBranch 的包装
func NewChainBranch[T any](cond GraphBranchCondition[T]) *ChainBranch {
    return NewChainMultiBranch[T](func(ctx context.Context, in T) (map[string]bool, error) {
        end, err := cond(ctx, in)
        if err != nil {
            return nil, err
        }
        return map[string]bool{end: true}, nil
    })
}
```

### 4.7 AppendBranch 内部流程

```
AppendBranch(b *ChainBranch):
  1. 校验：Branch 非 nil，至少 2 个分支节点
  2. 确定起点：从 preNodeKeys 获取唯一起点（拒绝多前驱）
  3. 注册节点：为每个分支节点生成 Graph key（加前缀避免冲突）
     - 格式: "{nextNodeKey()}_branch_{branchKey}"
  4. 包装条件函数：将分支 key 映射到 Graph 节点 key
  5. 设置 endNodes 白名单
  6. 调用 c.gg.AddBranch(startNode, &gBranch)
  7. 更新 preNodeKeys = 所有分支节点 key 列表
```

### 4.8 AppendParallel 内部流程

```
AppendParallel(p *Parallel):
  1. 校验：Parallel 非 nil，至少 2 个节点
  2. 确定起点：从 preNodeKeys 获取唯一起点（拒绝多前驱）
  3. 注册所有节点：使用 "{nextNodeKey()}_parallel_{i}" 命名
  4. 从起点到每个节点建边：c.gg.AddEdge(startNode, nodeKey)
  5. 更新 preNodeKeys = 所有并行节点 key 列表
```

### 4.9 节点命名规则

| 场景 | 生成的 key | 说明 |
|------|-----------|------|
| 普通节点 | `node_0`, `node_1`, `node_2` | 顺序递增 |
| Parallel 节点 | `node_0_parallel_0`, `node_0_parallel_1` | `{prefix}_parallel_{index}` |
| Branch 节点 | `node_1_branch_b1`, `node_1_branch_b2` | `{prefix}_branch_{branchKey}` |

用户可通过 `WithNodeKey("customKey")` GraphAddNodeOpt 覆盖自动命名。

---

## 5. 运行时语义与跳过的非目标

### 5.1 运行时语义

#### FieldMapping

- **编译时**：`validateFieldMapping` 执行前置类型检查。对于 struct 字段：检查字段存在性、导出性、类型赋值兼容性。对于带 `interface{}` 中间类型的路径：推迟到请求时检查。
- **请求时**：`fieldMap` 按 mapping 规则从输入值提取字段，组包为 `map[string]any`，再通过 `convertTo` 转换为目标 Go 类型。
- **错误处理**：map key not found → 根据 `allowMapKeyNotFound` 决定跳过/报错。nil interface / nil map → 报错。类型不匹配 → 报错。

#### Workflow

- **触发模式**：固定使用 `AllPredecessor`（DAG 模式）。
- **依赖解析**：
  - `normalDependency`：前驱完成（control）+ 数据到达（data）= 节点就绪。
  - `noDirectDependency`：仅数据到达 = 节点就绪（执行顺序由间接路径保证）。
  - `AddDependency`：仅 control 完成 = 节点就绪（无数据传递）。
- **Workflow 分支语义**：与 Graph 分支不同，分支**不自动**传递数据（`noDataFlow=true`）。分支的 end node 必须显式声明自己的 `AddInput`。

#### Chain

- **运行时无额外开销**：Chain 编译后被展开为底层 Graph，运行时与直接构建的 Graph 等价。
- **错误报告**：构建错误采用"先存后报"模式（`reportError`），首个错误被保留，后续 `AppendX` 在开头检查并跳过。
- **编译后不可变**：`Compile()` 后设置 `hasEnd = true`，所有 `AppendX` 检查 `hasEnd` 拒绝追加。

#### Parallel

- 所有并行节点共享同一个前驱节点的输入。
- 并行节点完成后，下游节点被触发（AllPredecessor）。
- 并行节点输出通过各自的 `outputKey` 标注，下游接收 `map[string]any`。

#### ChainBranch

- 条件函数在运行时评估，决定路由到哪个（或哪些）分支节点。
- 未激活的分支节点通过 `reportSkip` 标记为已跳过。

### 5.2 明确跳过的非目标

| 项目 | 理由 |
|------|------|
| **组件类型桥接**：`AddChatModelNode` / `AddToolsNode` / `AddRetrieverNode` / `AppendChatTemplate` / `AppendChatModel` 等 | 依赖 ChatModel/Tool/Retriever 组件接口，当前仅有 `Lambda`。可通过 `AddLambdaNode` + `AddGraphNode` 完成等价功能 |
| **Stream 执行形态**：`Runnable.Stream / Collect / Transform` | 当前复刻版仅有 `Invoke`。`composableRunnable.s` 字段已预留，本次不实现 |
| **`streamFieldMap` 流式映射** | 依赖完整的 stream reader 抽象，本次做 Stub（函数体返回 nil 或 panic("not implemented")） |
| **Stream ChainBranch 完整实现** | StreamReader 机制需要 schema 完整实现，`NewStreamChainBranch` / `NewStreamChainMultiBranch` 本次做 Stub |
| **Callback 机制**：OnStart/OnEnd/OnError | 当前不在范围内 |
| **State 传递**：`graph.state any` | 字段已定义但未使用，本次不实现 |
| **Checkpoint / Recovery** | 可恢复执行的中断-恢复机制，不在范围内 |
| **`indirectEdges` 合法性校验** | Eino 源码本身标记为 TODO（`workflow.go:509`），本次不做 |
| **`values_merge.go` 的 StreamReader merge** | StreamReader 抽象未完成 |
| **编译时类型推断**（`toValidateMap` / `updateToValidateMap`） | R1 先要求显式类型，推迟到后续版本 |
| **Component 特定的 `toChatModelNode` / `toToolsNode`** | R1 组件桥接简化 |

---

## 6. 与现有 Graph / Runner / Channel 的集成点

### 6.1 当前复刻版已有能力

基于 R3 审计，当前 `compose` 包已拥有：

| 模块 | 能力 | 文件 |
|------|------|------|
| `graph` | 节点添加（AddNode/AddLambdaNode）、数据边（AddEdge）、控制边（AddControlEdge）、分支注册（AddBranch，仅存储不消费）、编译 lock、Kahn 环检测、GraphInfo 导出 | `graph.go` |
| `Graph[I,O]` | 泛型包装、Compile → `Runnable[I,O]`、GetGraphInfo | `generic_graph.go` |
| `graphNode` | 子图递归编译（`compileIfNeeded`），`g *graph` 字段已预留嵌套子图 | `graph_node.go` |
| `runner` | 主循环（initChannels → routeInputToStartNodes → createTasks → resolveCompletedTasks）、maxSteps、EventLog | `graph_run.go` |
| `channelManager` | channel 管理、updateValues/updateDependencies/reportSkip/getReadyChannels | `graph_manager.go` |
| `dagChannel` | AllPredecessor 状态机（control 三态 + data 布尔）、mergeValuesFn、skip 传播、多值自动打包为 `map[string]any` | `dag.go` |
| `pregelChannel` | AnyPredecessor 先到先得语义 | `pregel.go` |
| `GraphBranch` | 基本结构（condition + branchMap） | `branch.go` |
| `Runnable[I,O]` | Invoke 接口 | `runnable.go` |
| `Lambda` | InvokableLambda 构造 | `runnable.go` |

### 6.2 必须在 Graph 层新增的集成点

#### 6.2.1 `addEdgeWithMappings` — 必须

在 `compose/graph.go` 中新增方法：

```go
// addEdgeWithMappings 带字段映射的边添加方法
// noDirectDependency: true 时不创建控制依赖（仅数据边）
// isControl: true 时不传递数据（仅控制边）
// mappings: 字段映射规则（isControl=true 时为空）
func (g *graph) addEdgeWithMappings(
    fromNodeKey, toNodeKey string,
    noDirectDependency bool,
    isControl bool,
    mappings []*FieldMapping,
) error
```

行为矩阵：

| noDirectDependency | isControl | 行为 |
|---|---|---|
| `false` | `false` | 标准数据+控制边。数据边写入 `dataEdges`，控制边写入 `controlEdges`。保存 mappings 到 `fieldMappingRecords` |
| `false` | `true` | 仅控制边（AddDependency 路径）。只写 `controlEdges` |
| `true` | `false` | 仅数据边（WithNoDirectDependency 路径）。只写 `dataEdges`，不写 `controlEdges` |
| `true` | `true` | 无实际操作（代码中不应出现此路径） |

#### 6.2.2 `fieldMappingRecords` — 必须

在 `graph` 结构体中新增字段：

```go
type graph struct {
    // ... 现有字段 ...
    fieldMappingRecords map[string]map[string][]*FieldMapping  // fromNodeKey → toNodeKey → mappings
    handlerPreNodes     map[string][]func(ctx context.Context, in any) (any, error)  // 编译时注入的 pre-handler
}
```

`handlerPreNodes` 用于 `SetStaticValue` 的编译时注入：在编译阶段，为有静态值的节点构建一个 merge handler，该 handler 在运行时节点执行之前被调用，将静态值合并到节点输入中。

#### 6.2.3 `compile()` 集成 FieldMapping 类型验证 — 必须

在 `graph.compile()` 方法（`graph.go:112`）的节点编译阶段之后，新增一个 **类型验证 pass**：

```
// 伪代码
for fromNodeKey, toMap := range g.fieldMappingRecords {
    for toNodeKey, mappings := range toMap {
        preType := g.getNodeOutputType(fromNodeKey)   // 从 chanCall.action 获取
        succType := g.getNodeInputType(toNodeKey)      // 从 chanCall.action 获取
        handler, unchecked, err := validateFieldMapping(preType, succType, mappings)
        if err != nil {
            return nil, err
        }
        // 存储 handlerPair 和 uncheckedSourcePath 供运行时使用
    }
}
```

#### 6.2.4 `chanCall` 扩展 — 必须

```go
type chanCall struct {
    nodeKey  string
    action   *composableRunnable
    writeTo  map[string]bool
    controls map[string]bool
    // 新增字段：
    fieldMappings    []*FieldMapping       // 本节点对下游的字段映射
    uncheckedPaths   map[string]FieldPath  // 未在编译时检查的路径
    preHandler       func(ctx context.Context, in any) (any, error)  // SetStaticValue 注入的 pre-handler
}
```

#### 6.2.5 `GraphBranch` 扩展 — 必须

在 `compose/branch.go` 中扩展 `GraphBranch` 结构：

```go
type GraphBranch struct {
    condition  func(ctx context.Context, input any) (string, error)
    branchMap  map[string]bool
    // 新增字段：
    invoke     func(ctx context.Context, input any) ([]string, error)    // Invoke 模式评估
    collect    func(ctx context.Context, input streamReader) ([]string, error)  // Stream 模式评估
    endNodes   map[string]bool    // 合法出口白名单
    noDataFlow bool               // Workflow 分支标记（不传递数据到分支节点）
}
```

新增构造函数：

```go
func NewGraphMultiBranch[T any](
    condition func(ctx context.Context, input T) (map[string]bool, error),
    endNodes map[string]bool,
) *GraphBranch

func NewGraphBranch[T any](
    condition func(ctx context.Context, input T) (string, error),
    endNodes map[string]bool,
) *GraphBranch
```

#### 6.2.6 Runner 集成 Branch 路由 — 必须

在 `graph_run.go` 的 `resolveCompletedTasks` 中集成 branch 评估：

```
resolveCompletedTasks(cm, completedTasks):
  for each task:
    // 检查该节点是否有 branch 注册
    if branches := g.branches[task.nodeKey]; len(branches) > 0:
      for each branch:
        selected := branch.invoke(ctx, task.output)
        for endNode in selected:
          // 数据路由到目标节点
          cm.updateValues(task.nodeKey, task.output, {endNode: true})
          cm.updateDependencies(task.nodeKey, {endNode: true})
        // 未选中的分支节点标记 skip
        for endNode in branch.endNodes - selected:
          cm.reportSkip(task.nodeKey, {endNode: true})
    else:
      // 原有正常路由
      cm.updateValues(task.nodeKey, task.output, cc.writeTo)
      cm.updateDependencies(task.nodeKey, cc.controls)
```

#### 6.2.7 `Graph[I,O]` 新增方法 — 必须

在 `compose/generic_graph.go` 中新增：

```go
// AddGraphNode 添加子图/子Chain/子Workflow 作为节点
func (gg *Graph[I, O]) AddGraphNode(key string, subRunnable AnyGraph) error

// addEdgeWithMappings (暴露给 Workflow 使用)
func (gg *Graph[I, O]) addEdgeWithMappings(
    fromNodeKey, toNodeKey string,
    noDirectDependency bool,
    isControl bool,
    mappings []*FieldMapping,
) error
```

#### 6.2.8 `AnyGraph` 接口 — 必须

在 `compose/runnable.go` 或 `compose/graph_node.go` 中新增：

```go
// AnyGraph 是图兼容类型的接口，支持 Graph/Chain/Workflow 统一挂载
type AnyGraph interface {
    // getGraph 返回底层 graph 结构（供编译展开）
    getGraph() *graph
    // getInputType 返回输入类型（供类型验证）
    getInputType() reflect.Type
    // getOutputType 返回输出类型（供类型验证）
    getOutputType() reflect.Type
}
```

`Graph[I,O]`、`Workflow[I,O]`、`Chain[I,O]` 均需实现此接口。

### 6.3 集成点变化总结

| 文件 | 改动类型 | 改动量估算 |
|------|---------|-----------|
| `compose/graph.go` | 扩展：addEdgeWithMappings / fieldMappingRecords / handlerPreNodes / compile 类型验证 pass | ~80 行 |
| `compose/branch.go` | 扩展：invoke/collect/endNodes/noDataFlow 字段，新增构造函数 | ~50 行 |
| `compose/graph_run.go` | 扩展：resolveCompletedTasks 集成 branch 评估 + fieldMapping 数据提取 | ~60 行 |
| `compose/generic_graph.go` | 扩展：AddGraphNode / addEdgeWithMappings | ~30 行 |
| `compose/graph_node.go` | 扩展：AnyGraph 接口 | ~15 行 |
| `compose/types.go` | 扩展：dependencyType 常量、GraphAddNodeOpt 选项 | ~20 行 |
| `compose/runnable.go` | 扩展：AnyGraph 接口定义 | ~10 行 |
| `compose/utils.go` | 扩展：handlerPair 类型 | ~10 行 |
| `compose/field_mapping.go` | **新增** | ~500 行 |
| `compose/workflow.go` | **新增** | ~500 行 |
| `compose/chain.go` | **新增** | ~300 行 |
| `compose/chain_parallel.go` | **新增** | ~100 行 |
| `compose/chain_branch.go` | **新增** | ~150 行 |

---

## 7. 文件归属分配（I1/I2/I3/I4）

### 7.1 Worker 角色定义

```
I1: Graph 层扩展工人
    负责 graph 层所有新增/修改，以及分支运行时路由

I2: FieldMapping 实现工人
    负责 field_mapping.go 全部实现 + type.go 扩展

I3: Workflow 实现工人
    负责 workflow.go 全部实现

I4: Chain / Parallel / ChainBranch 实现工人
    负责 chain.go / chain_parallel.go / chain_branch.go 全部实现
```

### 7.2 文件归属矩阵

| 文件 | I1 | I2 | I3 | I4 | 说明 |
|------|----|----|----|----|------|
| `compose/types.go` | **修改** | **修改** | | | I1: dependencyType 常量。I2: GraphAddNodeOpt / fieldMapping 相关 |
| `compose/utils.go` | **修改** | | | | handlerPair 类型定义 |
| `compose/runnable.go` | **修改** | | | | AnyGraph 接口定义 |
| `compose/branch.go` | **修改** | | | | GraphBranch 扩展：invoke/collect/endNodes/noDataFlow |
| `compose/graph.go` | **修改** | | | | addEdgeWithMappings / fieldMappingRecords / handlerPreNodes / compile 类型验证 pass |
| `compose/graph_node.go` | **修改** | | | | graphNode 可能扩展，AnyGraph 实现 |
| `compose/generic_graph.go` | **修改** | | | | AddGraphNode / addEdgeWithMappings 暴露 |
| `compose/graph_run.go` | **修改** | | | | resolveCompletedTasks 集成 branch 路由 + fieldMapping 数据提取 |
| `compose/graph_manager.go` | **不修改** | | | | channel 接口已足够 |
| `compose/dag.go` | **不修改** | | | | dagChannel 已支持 mergeValuesFn / reportSkip |
| `compose/pregel.go` | **不修改** | | | | 无变化 |
| `compose/event_log.go` | **不修改** | | | | 无变化 |
| `compose/introspect.go` | **不修改** | | | | 无变化 |
| `compose/field_mapping.go` | | **新增** | | | FieldMapping / FieldPath / validateFieldMapping / fieldMap / takeOne / assignOne / convertTo |
| `compose/workflow.go` | | | **新增** | | Workflow / WorkflowNode / WorkflowBranch / AddInput / AddDependency / SetStaticValue / compile |
| `compose/chain.go` | | | | **新增** | Chain Builder / addNode / preNodeKeys / addEndIfNeeded / nextNodeKey |
| `compose/chain_parallel.go` | | | | **新增** | Parallel / outputKey 冲突检测 |
| `compose/chain_branch.go` | | | | **新增** | ChainBranch / 四种构造函数 / Add* 方法 |
| `compose/graph_test.go` | **扩展** | | | | addEdgeWithMappings 测试、Branch 运行测试 |
| `compose/field_mapping_test.go` | | **新增** | | | FieldMapping 全量测试 |
| `compose/workflow_test.go` | | | **新增** | | Workflow 全量测试 |
| `compose/chain_test.go` | | | | **新增** | Chain / Parallel / ChainBranch 全量测试 |

### 7.3 交付顺序依赖

```
Phase 1（I1 + I2 并行启动）:
  I1: graph 层扩展（addEdgeWithMappings / branch 扩展 / runner 集成）
  I2: field_mapping.go 完整实现
  I2 的 validateFieldMapping 依赖 I1 的 addEdgeWithMappings 的接口定义
  → 先对齐接口签名，再各自实现

Phase 2（I3 依赖 I1+I2 完成）:
  I3: workflow.go 实现
  依赖: addEdgeWithMappings / validateFieldMapping / branch noDataFlow

Phase 3（I4 可与 I3 并行，也可等 I3 完成）:
  I4: chain.go / chain_parallel.go / chain_branch.go
  依赖: graph AddEdge / AddBranch 基础能力（I1 完成后即可开始）
  部分依赖: GraphBranch 扩展（I1）、AnyGraph 接口（I1）
```

---

## 8. 测试矩阵

### 8.1 FieldMapping 测试（`field_mapping_test.go`）

| 编号 | 测试用例 | 说明 |
|------|---------|------|
| FM-01 | `MapFields` 单字段到单字段 | struct 源 → struct 目标，类型匹配，编译通过 |
| FM-02 | `FromField` 源字段到全部输入 | 提取一个字段作为后继全量输入 |
| FM-03 | `ToField` 全部输出到目标字段 | 前驱全量输出填充到一个字段 |
| FM-04 | `MapFieldPaths` 嵌套到嵌套 | FieldPath{3 层} → FieldPath{2 层} |
| FM-05 | `FromFieldPath` 嵌套路径到全部 | 嵌套提取后赋值全量 |
| FM-06 | `ToFieldPath` 全部到嵌套路径 | 全量写入嵌套 struct 终端字段 |
| FM-07 | 编译时拒绝：源字段不存在 | field not found 错误 |
| FM-08 | 编译时拒绝：源字段未导出 | unexported field 错误 |
| FM-09 | 编译时拒绝：类型绝对不兼容 | assignableTypeMustNot 错误 |
| FM-10 | 编译时拒绝：FromAll + ToAll 冲突 | 两个前驱都试图写整个输入 |
| FM-11 | 编译时通过：assignableTypeMay | 推迟到请求时检查 |
| FM-12 | 编译时通过：interface 中间路径 | 路径中间遇到 interface{}，剩余路径推迟 |
| FM-13 | 请求时：struct → map 正确提取 | 运行时 takeOne 提取 |
| FM-14 | 请求时：map → struct 正确写入 | 运行时 assignOne + convertTo |
| FM-15 | 请求时：nil interface 报错 | errInterfaceNotValidForFieldMapping |
| FM-16 | 请求时：map key not found（allow=true 跳过） | fieldMap 第二参数为 true |
| FM-17 | 请求时：map key not found（allow=false 报错） | fieldMap 第二参数为 false |
| FM-18 | `WithCustomExtractor` | 自定义提取函数被正确调用 |
| FM-19 | FieldPath.join/splitFieldPath 往返 | join(split(s)) ≡ s |
| FM-20 | `FromNodeKey` / `FromPath` / `ToPath` 访问器 | 返回正确的内部值 |

### 8.2 Graph 层扩展测试（`graph_test.go` 扩展）

| 编号 | 测试用例 | 说明 |
|------|---------|------|
| GE-01 | `addEdgeWithMappings` 基本数据传播 | mappings 正确提取字段并传递 |
| GE-02 | `addEdgeWithMappings` noDirectDependency=true | 仅数据边，控制依赖不建立 |
| GE-03 | `addEdgeWithMappings` isControl=true | 仅控制边，数据不传递 |
| GE-04 | Graph Branch 单路径路由 | 条件返回一个 key，该分支节点被激活 |
| GE-05 | Graph Branch 多路径路由 | 条件返回多个 key，多分支同时激活 |
| GE-06 | Graph Branch 未激活节点 skip 传播 | 未选中的分支节点 channel 标记为 Skipped |
| GE-07 | Graph Branch 分支后汇聚 | 多分支结果汇聚到下游节点，通过 mergeValuesFn 合并 |
| GE-08 | Branch endNodes 验证 | 条件返回不在 endNodes 白名单的 key 时编译报错 |
| GE-09 | `AddGraphNode` 子图挂载 | 子图作为节点编译并执行 |
| GE-10 | `fieldMappingRecords` 编译类型指定 | 类型不兼容编译报错 |

### 8.3 Workflow 测试（`workflow_test.go`）

| 编号 | 测试用例 | 说明 |
|------|---------|------|
| WF-01 | 基本三节点 + FieldMapping | START → template(取 Query) → model → END |
| WF-02 | Fan-in 字段映射 | 从 A 取 field_a，从 B 取 field_b，汇聚到 END |
| WF-03 | Fan-in 路径冲突检测 | 两个 mapping 写入同一终端路径 → 编译报错 |
| WF-04 | `AddDependency` 仅控制依赖 | 前驱完成后触发，无数据传递 |
| WF-05 | `WithNoDirectDependency` 仅数据依赖 | 数据到达即触发，无直接控制依赖 |
| WF-06 | `WithNoDirectDependency` + 间接路径 | 通过其他节点间接保证执行顺序 |
| WF-06b | `WithNoDirectDependency` 无间接路径 → 数据丢失 | 缺少间接路径时前驱可能后执行 |
| WF-07 | `SetStaticValue` 预填充 | 编译时自静态值正确注入到节点输入 |
| WF-08 | `SetStaticValue` + AddInput 路径冲突检测 | 静态值路径与动态映射路径重叠 → 编译报错 |
| WF-09 | Workflow Branch 不传递数据 | noDataFlow=true，分支节点需显式 AddInput |
| WF-10 | Workflow Branch 分支节点显式 AddInput | 分支节点通过自己的 AddInput 获取数据 |
| WF-11 | 编译后锁定不修改 | Compile() 后再 AddInput → 报错 |
| WF-12 | addInputs 延迟闭包执行顺序 | 多个 AddInput 的正确闭包执行顺序 |
| WF-13 | AddPassthroughNode | 透传节点正确转发 |
| WF-14 | AddGraphNode 挂载子 Graph | 子图在 Workflow 中正确编译和运行 |

### 8.4 Chain 测试（`chain_test.go`）

| 编号 | 测试用例 | 说明 |
|------|---------|------|
| CH-01 | 线性 Append：3 个 Lambda | AppendLambda 三次，验证链式数据流 |
| CH-02 | AppendParallel | 并行调用两个 Lambda，汇聚到下游 |
| CH-03 | AppendBranch 单路径 + 汇聚 | 条件分叉 → AppendPassthrough → Lambda |
| CH-04 | AppendMultiBranch 多路径 | 同时路由到两个路径 |
| CH-05 | Branch → Parallel 组合 | 分叉后，一个分支内并行执行 |
| CH-06 | AppendGraph 子 Chain 嵌套 | 父 Chain 引用子 Chain |
| CH-07 | `addEndIfNeeded` 自动 END 连接 | 编译时自动将尾部连接到 END |
| CH-08 | `preNodeKeys` 跟踪正确 | 经 AppendX 后 preNodeKeys 变化验证 |
| CH-09 | 空 chain 编译失败 | 无节点时 Compile 报错 |
| CH-10 | 空 parallel 编译失败 | Parallel 无节点时 Append 报错 |
| CH-11 | 单节点 branch 编译失败 | Branch 只有 1 个分支节点时报错 |
| CH-12 | 重复 outputKey 检测 | Parallel 中 outputKey 重复时报错 |
| CH-13 | nil condition 在 branch 中 | NewChainBranch(nil) → Error() 非 nil |
| CH-14 | `nextNodeKey` 命名规则 | 验证 node_0, node_0_parallel_0, node_1_branch_xxx |

### 8.5 边界条件与安全测试（各测试文件共享）

| 编号 | 测试用例 | 说明 |
|------|---------|------|
| BC-01 | 编译 lock 对所有 mutation 生效 | graph / workflow / chain 编译后不可修改 |
| BC-02 | 并发执行多图不互相影响 | 多个 Workflow/Chain 并行执行，数据隔离 |
| BC-03 | EventLog 线程安全 | 并发 Write/Read 无 race |
| BC-04 | 图无 START/END 报错 | 编译时缺少边报错 |
| BC-05 | maxSteps 边界 | Pregel 模式超步报错 |
| BC-06 | 节点执行错误传播 | 节点运行时错误正确向外传播 |
| BC-07 | 嵌套图类型不匹配 | 父图输出类型与子图输出类型不兼容编译报错 |

### 8.6 测试规范

- **包声明**：`package compose`（白盒测试，可直接访问未导出函数）
- **命名规范**：`Test<Feature><Scenario>`，如 `TestFieldMappingFromField`、`TestWorkflowFanInConflict`
- **错误断言**：使用 `errors.Is(err, ExpectedError)` 或 `strings.Contains(err.Error(), "expected sub-string")`
- **setup 模式**：每个测试独立创建对象 → 调用 → 断言
- **辅助函数**：可复用现有 `nodeIdentity` / `nodeToUpper` / `nodeReverse` / `nodeFailing`

---

## 9. 代码示例

### 9.1 使用示例：FieldMapping + Workflow

```go
package main

import (
    "context"
    "fmt"
    "github.com/example/eino/compose"
)

type Input struct {
    Query  string
    UserID string
}
type Output struct {
    Reply string
}

func main() {
    wf := compose.NewWorkflow[*Input, *Output]()

    // STEP 1: 从 START 提取 Query 字段 → template 节点
    wf.AddLambdaNode("template", compose.InvokableLambda(
        func(ctx context.Context, query string) (string, error) {
            return "processed:" + query, nil
        },
    )).AddInput(compose.START, compose.FromField("Query"))

    // STEP 2: template 输出 → model 节点
    wf.AddLambdaNode("model", compose.InvokableLambda(
        func(ctx context.Context, prompt string) (string, error) {
            return "model:" + prompt, nil
        },
    )).AddInput("template")

    // STEP 3: model 输出 → END 的 Reply 字段
    wf.End().AddInput("model", compose.MapFields("Content", "Reply"))

    r, err := wf.Compile(context.Background())
    if err != nil {
        panic(err)
    }

    result, err := r.Invoke(context.Background(), &Input{Query: "hello"})
    fmt.Println(result) // &Output{Reply: "model:processed:hello"}
}
```

### 9.2 使用示例：Workflow 执行依赖与数据流分离

```go
// setupNode 负责初始化
setupNode := wf.AddLambdaNode("setup", setupLambda)
setupNode.AddInput(compose.START)

// mainNode 需要 setupNode 完成但不需要其数据；需要 START 的 userID
mainNode := wf.AddLambdaNode("main", mainLambda)
mainNode.AddDependency("setup")                                    // 仅执行依赖
mainNode.AddInput(compose.START, compose.MapFields("UserID", "userID"))  // 数据依赖
```

### 9.3 使用示例：Chain 线性 + Parallel

```go
chain := compose.NewChain[string, string]()

parallel := compose.NewParallel()
parallel.
    AddLambda("upper", compose.InvokableLambda(
        func(ctx context.Context, s string) (string, error) { return strings.ToUpper(s), nil },
    )).
    AddLambda("lower", compose.InvokableLambda(
        func(ctx context.Context, s string) (string, error) { return strings.ToLower(s), nil },
    ))

chain.
    AppendPassthrough().
    AppendParallel(parallel).
    AppendLambda(compose.InvokableLambda(
        func(ctx context.Context, m map[string]any) (string, error) {
            return m["upper"].(string) + " " + m["lower"].(string), nil
        },
    ))

r, _ := chain.Compile(context.Background())
result, _ := r.Invoke(context.Background(), "Hello")
// result = "HELLO hello"
```

### 9.4 使用示例：Chain Branch

```go
chain := compose.NewChain[string, string]()

branchCond := func(ctx context.Context, in string) (string, error) {
    if len(in) > 5 {
        return "long", nil
    }
    return "short", nil
}

chain.
    AppendBranch(compose.NewChainBranch(branchCond).
        AddLambda("long", compose.InvokableLambda(
            func(ctx context.Context, s string) (string, error) {
                return "LONG:" + s, nil
            },
        )).
        AddLambda("short", compose.InvokableLambda(
            func(ctx context.Context, s string) (string, error) {
                return "SHORT:" + s, nil
            },
        )),
    ).
    AppendPassthrough()

r, _ := chain.Compile(context.Background())
result, _ := r.Invoke(context.Background(), "hello-world")
// result = "LONG:hello-world"
result, _ = r.Invoke(context.Background(), "hi")
// result = "SHORT:hi"
```

---

## 10. 已知风险与约束

### 10.1 架构风险

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| **Graph 编译时类型系统不完整** | `validateFieldMapping` 需要获取 predecessor/successor 的类型。当前 `composableRunnable` 不携带 `reflect.Type` 信息，只能用 `fmtType()` 返回简单字符串。 | I2 实现时需要让 `graphNode` 携带 `inputType/outputType reflect.Type` 字段。或通过 `Lambda` 的泛型参数在构造时捕获类型。 |
| **Branch 路由与 Runner 主循环的竞态** | 分支激活/跳过与节点就绪判断之间可能存在并发时序问题。当前 `taskManager.wait()` 是同步的，但需确保 skip 传播在 `createTasks` 之前完成。 | 分支评估在 `resolveCompletedTasks` 中同步完成，然后在下一轮 `getReadyChannels` 中生效。 |
| **嵌套图 key 冲突** | Workflow/Chain 内的节点 key 可能与父 graph 的同名节点冲突。 | Chain 的 `nextNodeKey` 使用递增数字作为前缀避免冲突。Workflow 通过用户手动指定唯一 key。如果用户手动指定 key 重复，编译时 `graph.AddNode` 会静默覆盖旧节点。需要在 AddNode 时发出警告或报错。 |
| **延迟闭包模式的副作用** | `addInputs: []func() error` 在编译前已捕获对 `graph` 的指针引用，但如果闭包在编译前访问了还未初始化的数据结构，可能 panic。 | 严格遵循两阶段编译顺序：先收集所有分支信息，再执行 addInputs 闭包。 |
| **`mappedFieldPath` trie 线程非安全** | 当前 Eino 源码和 replica 均在单线程声明阶段使用，无锁。如未来支持并发添加节点，需加锁。 | 文档标明"不可在并发环境下调用 AddInput"。 |

### 10.2 实现约束

| 约束 | 说明 |
|------|------|
| **路径分隔符必须使用 `\x1F`** | 与 Eino 源码一致，不使用其他字符。因为该字符极不可能出现在用户字段名中。 |
| **`allowMapKeyNotFound` 默认 false** | 在 Workflow 上下文中，`fieldMap` 的第二参数始终为 `false`（缺失时报错）。仅在流式映射中可能为 `true`。 |
| **Workflow 不直接调用底层 Graph 的 AddEdge** | 混用手动 AddEdge 和 Workflow.AddInput 会导致依赖追踪表 `dependencies` 与实际图结构不一致。 |
| **Chain 的 `addEndIfNeeded` 只执行一次** | `hasEnd` 标记确保编译后 `appendEndIfNeeded` 不会被重复调用。 |
| **Branch 至少需要 2 个节点** | 1 个分支节点无意义：编译时 `AppendBranch` / `NewChainBranch` 应拒绝单个分支节点。 |
| **Parallel 至少需要 2 个节点** | 1 个并行节点等价于普通节点，无意义：`AppendParallel` 应拒绝。 |
| **`SetStaticValue` 的值必须是 JSON 可序列化的基础类型** | string, int, float, bool, 或嵌套 map/slice。不支持 channel / function / struct pointer 等。 |
| **`customExtractor` 仅对 struct/map 之外的来源有效** | 如从 slice 中取元素。对标准 struct/map 路径不调用 customExtractor。 |
| **当前不使用 Go 原生泛型反射** | `fmtType()` 只返回 `string/int/float64/bool/any` 六个字符串。`validateFieldMapping` 需要真实的 `reflect.Type` 时必须从 `Lambda` / `graphNode` 的运行时类型中获取，而不是从泛型参数推导。 |

### 10.3 已识别的 Bugs / 未完成项

| 项目 | 状态 | 处理方式 |
|------|------|---------|
| `graph.branches` 当前只存储不消费 | 已知，I1 修复 | I1 在 `graph_run.go` 中集成 branch 评估 |
| `graph_node` 无 `inputType/outputType` 反射类型 | 已知，I2/I1 修复 | I2 需要在 `graphNode` 中加入 `reflect.Type` 字段，由 `AddLambdaNode` 时填充 |
| `GraphBranch` 无 `invoke/collect` 双函数 | 已知，I1 修复 | I1 扩展 `GraphBranch` 结构 |
| `fmtType()` 返回简单字符串，无法支持复杂类型验证 | 已知 | 在 `Lambda` 构造时使用 `reflect.TypeOf` 捕获真实类型，存储到 `graphNode` 中 |
| `composableRunnable` 预留了 `s` 字段但未实现 Stream | 已知，本次不处理 | 跳过 |
| `graph.state any` 字段定义但未使用 | 已知，本次不处理 | 跳过 |
| `IndirectEdges` 合法性校验 | Eino 源码 TODO | 本次不实现 |

---

## 附录 A：实现检查清单

### I1（Graph 层扩展）检查清单

- [ ] `compose/graph.go`：新增 `fieldMappingRecords` / `handlerPreNodes` 字段
- [ ] `compose/graph.go`：实现 `addEdgeWithMappings(fromNodeKey, toNodeKey, noDirectDependency, isControl, mappings)`
- [ ] `compose/graph.go`：`compile()` 新增类型验证 pass（调用 `validateFieldMapping`）
- [ ] `compose/graph.go`：`compile()` 集成 `fieldMappingRecords` 的数据提取逻辑到 `chanCall`
- [ ] `compose/branch.go`：扩展 `GraphBranch`（invoke/collect/endNodes/noDataFlow）
- [ ] `compose/branch.go`：新增 `NewGraphMultiBranch` / `NewGraphBranch`（带 endNodes 参数）
- [ ] `compose/graph_run.go`：`resolveCompletedTasks` 集成 branch 评估
- [ ] `compose/graph_run.go`：`resolveCompletedTasks` 集成 fieldMapping 数据提取
- [ ] `compose/generic_graph.go`：新增 `AddGraphNode` / `addEdgeWithMappings`
- [ ] `compose/runnable.go`：新增 `AnyGraph` 接口定义
- [ ] `compose/utils.go`：新增 `handlerPair` 类型
- [ ] `compose/graph_node.go`：`graphNode` 新增 `inputType/outputType reflect.Type` 字段
- [ ] `compose/types.go`：新增 `dependencyType` 常量

### I2（FieldMapping）检查清单

- [ ] `compose/field_mapping.go`：`FieldMapping` 结构体 + 6 个构造器
- [ ] `compose/field_mapping.go`：`FieldPath` 类型 + `splitFieldPath` / `join` / `\x1F` 分隔符
- [ ] `compose/field_mapping.go`：`checkAndExtractFieldType` — 编译时路径类型提取
- [ ] `compose/field_mapping.go`：`validateFieldMapping` — 编译时静态校验（实例校验/逐字段/推迟检查）
- [ ] `compose/field_mapping.go`：`fieldMap` — 请求时映射执行
- [ ] `compose/field_mapping.go`：`takeOne` / `assignOne` — 字段提取/写入原语
- [ ] `compose/field_mapping.go`：`convertTo` — map → Go 类型转换
- [ ] `compose/field_mapping.go`：`FieldMappingOption` + `WithCustomExtractor`
- [ ] `compose/field_mapping.go`：内部错误 `errMapKeyNotFound` / `errInterfaceNotValidForFieldMapping`
- [ ] `compose/field_mapping.go`：`streamFieldMap` — Stub（返回 nil 或 panic("not implemented")）
- [ ] `compose/field_mapping_test.go`：≥ 20 个测试用例

### I3（Workflow）检查清单

- [ ] `compose/workflow.go`：`Workflow[I,O]` 结构 + `NewWorkflow`
- [ ] `compose/workflow.go`：`WorkflowNode` 结构 + 延迟闭包数组
- [ ] `compose/workflow.go`：`WorkflowBranch` 结构
- [ ] `compose/workflow.go`：`AddLambdaNode` / `AddGraphNode` / `AddPassthroughNode`
- [ ] `compose/workflow.go`：`End()` — 返回 END 的 WorkflowNode
- [ ] `compose/workflow.go`：`WorkflowNode.AddInput` / `AddInputWithOptions`
- [ ] `compose/workflow.go`：`WorkflowNode.AddDependency` — 纯执行依赖
- [ ] `compose/workflow.go`：`WorkflowNode.SetStaticValue` — 编译时静态值
- [ ] `compose/workflow.go`：`Workflow.AddBranch` — Workflow 分支声明
- [ ] `compose/workflow.go`：`addDependencyRelation` — 统一依赖入口
- [ ] `compose/workflow.go`：`checkAndAddMappedPath` — 路径冲突检测
- [ ] `compose/workflow.go`：`compile` — 两阶段编译（分支收集 → addInputs → 静态值注入 → graph.compile）
- [ ] `compose/workflow_test.go`：≥ 14 个测试用例

### I4（Chain / Parallel / ChainBranch）检查清单

- [ ] `compose/chain.go`：`Chain[I,O]` 结构 + `NewChain`
- [ ] `compose/chain.go`：`AppendLambda` / `AppendGraph` / `AppendPassthrough` / `AppendParallel` / `AppendBranch`
- [ ] `compose/chain.go`：`addNode` 核心（preNodeKeys 追踪）
- [ ] `compose/chain.go`：`addEndIfNeeded` — 自动 END 连接
- [ ] `compose/chain.go`：`nextNodeKey` / `reportError` — 辅助函数
- [ ] `compose/chain.go`：`Compile` — 展开底层图并调用 graph.compile
- [ ] `compose/chain_parallel.go`：`Parallel` 结构 + `NewParallel`
- [ ] `compose/chain_parallel.go`：`AddLambda` / `AddGraph` / `AddPassthrough`
- [ ] `compose/chain_parallel.go`：outputKey 冲突检测
- [ ] `compose/chain_branch.go`：`ChainBranch` 结构
- [ ] `compose/chain_branch.go`：`NewChainBranch` / `NewChainMultiBranch`
- [ ] `compose/chain_branch.go`：`NewStreamChainBranch` / `NewStreamChainMultiBranch`（Stub）
- [ ] `compose/chain_branch.go`：`AddLambda` / `AddGraph` / `AddPassthrough`
- [ ] `compose/chain_branch.go`：`AppendBranch` 中分支 key → Graph key 映射逻辑
- [ ] `compose/chain_test.go`：≥ 14 个测试用例

---

## 附录 B：关键源码参考（Eino 原始实现）

| 内容 | Eino 文件:行号 | 说明 |
|------|----------|------|
| Workflow 结构定义 | `workflow.go:45-50` | Workflow[I,O] / workflowNodes / workflowBranches / dependencies |
| WorkflowNode 结构 | `workflow.go:34-41` | addInputs 延迟闭包 / staticValues / mappedFieldPath |
| dependencyType 常量 | `workflow.go:52-58` | normalDependency / noDirectDependency / branchDependency |
| addDependencyRelation | `workflow.go:316-367` | 三种模式的依赖统一入口 |
| checkAndAddMappedPath | `workflow.go:369-404` | 路径冲突检测 trie |
| Workflow.compile | `workflow.go:440-512` | 两阶段编译 |
| FieldMapping 结构 | `field_mapping.go:31-37` | fromNodeKey / from / to / customExtractor |
| validateFieldMapping | `field_mapping.go:645-774` | 编译时三阶段检验 |
| fieldMap | `field_mapping.go:484-566` | 请求时映射执行 |
| checkAndExtractFieldType | `field_mapping.go:...` | 路径类型提取 |
| takeOne | `field_mapping.go:574-601` | 单值提取 |
| Chain 结构 | `chain.go:72-82` | err / gg / nodeIdx / preNodeKeys / hasEnd |
| addNode 核心 | `chain.go:560-600` | preNodeKeys → 新节点 → 更新尾部 |
| addEndIfNeeded | `chain.go:98-121` | 自动 END 连接 |
| AppendBranch 实现 | `chain.go:342-447` | 分支 key → Graph key 映射 |
| AppendParallel 实现 | `chain.go:459-514` | Parallel 解包到图 |
| Parallel 结构 | `chain_parallel.go:49-53` | nodes / outputKeys |
| ChainBranch 结构 | `chain_branch.go:38-42` | internalBranch / key2BranchNode |
| GraphBranch 结构 | `branch.go:42-50` | invoke / collect / endNodes / noDataFlow |

---

> **契约版本**: v1.0
> **编写日期**: 2026-06-03
> **编写依据**: R1/R2/R3 研究笔记 + Eino 技术手册第二章
> **适用范围**: I1 / I2 / I3 / I4 工人实现参考
