# R1 研究笔记：Workflow 与 FieldMapping 机制

> 基于 Eino 源码：`compose/workflow.go`、`compose/field_mapping.go`、`compose/workflow_test.go`、`compose/values_merge.go` 及技术手册第二章

---

## 一、Workflow / FieldMapping 解决什么问题

在一个由多个异构算力单元（ChatModel、Tool、Retriever、Lambda、子图）组成的执行图中，存在两个正交的关心点：

1. **数据流**：前一个算力单元的输出（或其中某些字段）如何传入下一个算力单元的输入（或其中某些字段）。
2. **执行依赖**：下一个算力单元必须等待哪些前驱算力单元完成之后才可以执行。

在简单的 DAG 图里，每条边 `A → B` 自然地同时表达了这两个语义。但以下场景无法用简单边解决：

- **多前驱汇聚（fan-in）**：B 需要从 A1 取 `field_a`、从 A2 取 `field_b`，两者都完成才执行。如果为每个前驱建全量数据边，会在 B 的输入上产生字段冲突 —— 每个前驱的输出都想独占 B 的整个输入类型。
- **执行依赖与数据流分离**：C 需要在 A 执行完毕后执行（控制依赖），但不需要 A 的任何数据（或只通过间接路径从其他节点获取数据）。反之，C 需要 A 的输出数据，但执行顺序应该由另一条 `pathB → C` 的间接路径保证。
- **跨分支数据访问**：D 在分支的一侧需要原始 START 输入的数据（如 `user_id`），但 D 的执行由分支条件决定，不应该被 START 直接阻塞。
- **嵌套字段提取与注入**：前驱输出是 `structB{F1: *structA{F1: "hello"}}`，后继只需要 `"hello"` 这个终端值，且需要写入后继的 `response.data.userName` 位置。

`Workflow` 和 `FieldMapping` 的本质目标，就是将这三种关心点（数据字段映射、执行前置等待、分支调度）从"隐式嵌入到边"提升为"显式声明式配置"。

---

## 二、为什么这个问题在 graph 编排中是真实的

### 2.1 Fan-in 场景下的字段冲突

假设 END 节点需要从 node1 取 `Field1`、从 node2 取 `Field2`。如果用 `AddEdge(node1, END)` 和 `AddEdge(node2, END)`，底层图会把 node1 和 node2 的整个输出都送到 END。如果 END 的类型是 `Output{Field1, Field2}`，编译器无法知道 node1 的输出应该映射到 `Field1` 还是 `Field2`，最终导致 `mergeValues` 试图将两个不同来源的整个 object 合并，结果不可预测。

**Eino 的解决方案**：
```go
w.End().AddInput("A", MapFields("Field1", "Field1")).
        AddInput("B", MapFields("Field2", "Field2"))
```

每个 `AddInput` 声明了精确的字段映射，`checkAndAddMappedPath`（`workflow.go:369-404`）在编译时检测目标路径冲突。当两个 mapping 同时声称要写入终端的同一路径时，直接报错：

```go
// workflow.go:390
return fmt.Errorf("two terminal field paths conflict for node %s: %v, %v", n.key, traversed, targetPath)
```

### 2.2 执行依赖与数据流是真正交的

`AddDependency`（`workflow.go:300-302`）只创建控制边（无数据传递）：
```go
func (n *WorkflowNode) AddDependency(fromNodeKey string) *WorkflowNode {
    return n.addDependencyRelation(fromNodeKey, nil, &workflowAddInputOpts{dependencyWithoutInput: true})
}
```
内部调用 `g.addEdgeWithMappings(fromNodeKey, n.key, false, true)` —— 第三个参数 `isControl=true`（`workflow.go:342`）。

`WithNoDirectDependency`（`workflow.go:257-261`）则相反：只创建数据边，不创建直接执行依赖。执行顺序通过其他节点的间接路径保证。内部调用 `g.addEdgeWithMappings(fromNodeKey, n.key, true, false)`（`workflow.go:331`）。

这在跨分支场景下至关重要。例如 node1 的数据需要传给分支后的 node2，但 node2 的执行由分支逻辑决定。若用直接边，node2 会越过分支控制，在 node1 完成时就触发（错误）。用 `WithNoDirectDependency` 后，数据可以安全传递，但执行排序留给分支本身处理。

### 2.3 编译时 vs 请求时的类型检查

字段映射涉及 Go 反射的类型检查。大部分路径可以在编译时验证（struct field 是否导出的、map key 类型是否为 string）。但当路径中间遇到 `interface{}` 或 `any` 类型时，中间类型无法在编译时确定：

```go
// field_mapping.go:472-473
if extracted.Kind() == reflect.Interface {
    return extracted, paths[i:], nil  // 返回剩余的未检查路径给请求时处理
}
```

`validateFieldMapping`（`field_mapping.go:645-774`）为此类情况构建 `handlerPair`（L714-738）推迟到请求时校验。这是经典的 **"编译期尽最大努力验证，请求期兜底"** 模式。

### 2.4 Workflow 分支与 Graph 分支的语义差异

这是 Eino 设计的一个关键点（`workflow.go:412-418`）：

- **Graph 分支**：分支输入数据自动传递给被选中的节点。
- **Workflow 分支**：分支不传递数据。分支的 end node 必须显式声明自己的 `AddInput`。

原因是：Graph 分支位于"控制能力强，声明少"的层级；Workflow 分支位于"声明式，字段映射可追踪"的层级。在 Workflow 中让分支自动传数据会破坏字段映射的可追溯性。

---

## 三、Eino 的解决方案模式与关键源码机制

### 3.1 Workflow 核心结构

```go
// workflow.go:45-50
type Workflow[I, O any] struct {
    g                *graph                                    // 底层图
    workflowNodes    map[string]*WorkflowNode                  // 所有节点
    workflowBranches []*WorkflowBranch                        // 分支列表
    dependencies     map[string]map[string]dependencyType      // 依赖表
}
```

```go
// workflow.go:34-41
type WorkflowNode struct {
    g                *graph
    key              string
    addInputs        []func() error       // 延迟闭包数组
    staticValues     map[string]any       // 编译时静态值
    dependencySetter func(fromNodeKey string, typ dependencyType)
    mappedFieldPath  map[string]any       // 已映射路径（防止冲突）
}
```

### 3.2 三种依赖类型

```go
// workflow.go:52-58
const (
    normalDependency    dependencyType = iota  // 数据 + 执行
    noDirectDependency                        // 只有数据
    branchDependency                          // 分支依赖
)
```

`normalDependency` 是 `AddInput` 的默认行为；`noDirectDependency` 被 `AddInputWithOptions(..., WithNoDirectDependency())` 设置；`branchDependency` 在 `compile` 阶段由分支路由逻辑设置。

### 3.3 `addDependencyRelation` — 统一的依赖声明入口

`workflow.go:316-367` 是所有依赖声明的核心。根据 `options` 不同构造三种闭包：

1. **默认模式**（`else` 分支, L348-363）：
   - `checkAndAddMappedPath(paths)` — 校验目标路径不冲突
   - `g.addEdgeWithMappings(fromNodeKey, n.key, false, false, inputs...)` — 创建数据+控制边
   - `n.dependencySetter(fromNodeKey, normalDependency)` — 写入依赖表

2. **NoDirectDependency 模式**（L321-336）：
   - 同上，但 `addEdgeWithMappings` 第三个参数为 `true`（不创建直接控制边）
   - `n.dependencySetter(fromNodeKey, noDirectDependency)`

3. **DependencyWithoutInput 模式**（L337-347）：
   - 校验 `inputs` 必须为空
   - `g.addEdgeWithMappings(fromNodeKey, n.key, false, true)` — 仅控制边（第四个参数 `isControl=true`）
   - `n.dependencySetter(fromNodeKey, normalDependency)`

关键设计：`addInputs` 是 `[]func() error` 闭包数组，不是立即执行。因为 Workflow 的 `compile` 方法先处理所有分支，再统一执行这些闭包——这是 **"声明-编译"两阶段模式**。

### 3.4 `compile` — 两阶段编译

`workflow.go:440-512` 的 `compile` 方法：

1. **阶段 1：收集分支信息**（L445-458）
   - 遍历所有 `workflowBranches`，将分支依赖注入到 `dependencies` 表
   - 调用 `g.addBranch(wb.fromNodeKey, wb.GraphBranch, true)` — 第三个参数 `noDataFlow=true`（Workflow 分支语义）

2. **阶段 2：执行所有延迟的 addInputs**（L460-467）
   - 遍历所有 `workflowNodes`，逐个执行 `addInputs` 闭包
   - 执行后清空 `addInputs`（`n.addInputs = nil`）

3. **阶段 3：注入静态值**（L469-507）
   - 为有静态值的节点构建 `handlerPair`，通过 `handlerPreNode` 注入
   - 内部使用 `mergeValues` 合并静态值和动态输入

4. **最终委托给底层 graph.compile**（L511）

### 3.5 FieldMapping 体系

```go
// field_mapping.go:31-37
type FieldMapping struct {
    fromNodeKey    string
    from           string                    // 内部用 \x1F (Unit Separator) 分隔的嵌套路径
    to             string
    customExtractor func(input any) (any, error)
}
```

六个构造函数提供不同精度的映射：

| 构造器 | 语义 | 示例 |
|--------|------|------|
| `MapFields(from, to)` | 源字段 → 目标字段 | `MapFields("name", "userName")` |
| `FromField(from)` | 源字段 → 后继全部输入 | `FromField("Field1")` |
| `ToField(to)` | 前驱全部输出 → 目标字段 | `ToField("query")` |
| `MapFieldPaths(fromPath, toPath)` | 嵌套路径 → 嵌套路径 | `MapFieldPaths(FieldPath{"user","profile","name"}, FieldPath{"response","name"})` |
| `FromFieldPath(fp)` | 嵌套源路径 → 全部输入 | `FromFieldPath(FieldPath{"data","result"})` |
| `ToFieldPath(fp)` | 全部输出 → 嵌套目标路径 | `ToFieldPath(FieldPath{"response","data","userName"})` |

**嵌套路径分隔符**：使用 `\x1F`（Unit Separator）作为内部路径分隔符（`field_mapping.go:142`），因为该字符极不可能出现在用户字段名中。

### 3.6 `validateFieldMapping` — 编译时类型检验

`field_mapping.go:645-774` 执行三阶段检验：

1. **实例校验**（L652-660）：
   - 不允许 `FromAll + ToAll`（这是普通边）
   - `ToField` 要求 successor 类型是 struct 或 map
   - `FromField` 要求 predecessor 类型是 struct 或 map

2. **逐字段路径检验**（L664-740）：
   - `checkAndExtractFieldType` 沿路径逐段提取类型（穿透指针、进入 struct field、进入 map value）
   - 遇到 interface 类型时返回 `remainingPaths`（推迟到请求时）
   - 对可完整路径提取的字段，执行 `checkAssignable`（`assignableTypeMust` / `assignableTypeMustNot` / `assignableTypeMay`）

3. **构建 deferred checker**（L743-773）：
   - 对 `assignableTypeMay` 和 `interface 中间路径` 构建 `handlerPair`
   - 请求时通过 `checker(invoke fn)` 做最后一次类型断言

### 3.7 `fieldMap` — 请求时映射执行

`field_mapping.go:484-566` 是实际的数据提取执行函数：

- 遍历每个 mapping
- 使用 `takeOne`（L574-601）从源值中按路径提取：struct 用 `FieldByName`，map 用 `MapIndex`
- 遇到 nil interface/nil map 报错
- map key not found 的错误根据 `allowMapKeyNotFound` 配置决定跳过还是报错

### 3.8 `checkAndAddMappedPath` — 字段路径冲突检测

`workflow.go:369-404` 使用嵌套 map 作 trie 结构记录已映射路径：

```go
// "" key 的值为 struct{} 表示整个输入已被一个 mapping 独占
// "" key 的值为 map[string]any 表示部分字段已映射
```

当两个 mapping 的终端路径重叠时返回 `"two terminal field paths conflict"` 错误。

### 3.9 `values_merge.go` — Fan-in 值合并

`values_merge.go:39-82` 的 `mergeValues` 处理多前驱值的合并：

- 通过注册的 `RegisterValuesMergeFunc[T]` 分发特定类型的合并逻辑
- StreamReader 的合并支持 `streamMergeWithSourceEOF` 标记（用于 `FanInMergeConfig`）
- 默认只支持注册了 merge 函数的类型

### 3.10 Workflow 中的 Graph 底层交互

关键函数 `addEdgeWithMappings`（`graph.go:232-294`）接收四个核心参数：

```
addEdgeWithMappings(fromNodeKey, toNodeKey string, noDirectDependency bool, isControl bool, mappings ...*FieldMapping)
```

- `noDirectDependency=true`：仅数据边，不建立控制依赖
- `isControl=true`：仅控制边，不建立数据依赖
- 两个都为 `false`：标准数据+控制边
- 两个都为 `true`：无实际操作（代码中不存在此路径）

---

## 四、Go Replica 应该实现什么、明确跳过什么

### 4.1 应该实现（Full Replicate）

| 能力 | 优先级 | 理由 |
|------|--------|------|
| `FieldMapping` 结构体 + 全部 6 个构造器 | P0 | 是整个字段映射的根；所有其他功能依赖它 |
| `FieldPath` 类型 + `splitFieldPath`/`join` | P0 | 嵌套路径的基础设施 |
| `checkAndExtractFieldType` — 编译时路径类型提取 | P0 | `validateFieldMapping` 的核心依赖，决定哪些路径能在编译时校验 |
| `validateFieldMapping` — 编译时静态校验 | P0 | 防止用户在 mapping 中使用非法字段/类型；生成 `handlerPair` 做 deferred check |
| `fieldMap` + `streamFieldMap` — 请求时映射执行 | P0 | 运行时提取源字段值并组包为目标 `map[string]any` |
| `takeOne` — 从 struct/map 中提取单个字段 | P0 | `fieldMap` 的核心原语 |
| `assignOne` — 将单个值写入 struct/map 的目标路径 | P0 | `convertTo` 的核心原语 |
| `convertTo` — 将中间 `map[string]any` 转换为后继的实际类型 | P0 | 映射的终点：从 map 转换为目标 Go 类型 |
| `FieldMappingOptions` + `WithCustomExtractor` | P1 | 允许用户提供自定义提取函数（map 和 struct 之外的来源，如 array element） |
| `Workflow[I,O]` 泛型构造 + `Compile` 入口 | P1 | 提供声明式 API |
| `WorkflowNode.AddInput` / `AddInputWithOptions` | P1 | 核心声明方式 |
| `WorkflowNode.AddDependency` — 纯执行依赖 | P1 | 执行依赖与数据流解耦 |
| `WorkflowNode.SetStaticValue` — 编译时静态值 | P1 | 通过 `handlerPreNode` merge 静态值到节点输入 |
| `checkAndAddMappedPath` — 路径冲突检测 | P1 | 防止两个 mapping 写入同一终端路径 |
| `Workflow.AddBranch` — 分支声明 | P1 | 依赖 Graph 的 Branch 支持 |
| `Workflow.compile` 两阶段编译 | P1 | 先收集分支，后执行延迟的 addInputs |
| `Workflow.initNode` — 节点初始化 | P1 | 创建 WorkflowNode 并注册到依赖表 |

### 4.2 明确跳过

| 能力 | 理由 |
|------|------|
| 组件类型桥接（`AddChatModelNode` / `AddToolsNode` / `AddRetrieverNode` 等） | 这些方法是 Graph 节点操作的语法糖。Replica 已经拥有 graph 层，Workflow 可以通过 `AddGraphNode` + `AddLambdaNode` 完成等价功能。组件桥接在 R2 自然获得。 |
| `AddAgenticModelNode` / `AddAgenticChatTemplateNode` | 依赖 AgenticMessage schema，R2 完成。 |
| `workflowAddInputOpts` 的全部 option 函数导出（`WithNoDirectDependency`、`WithDependencyWithoutInput` 除外） | 当前 Workflow 测试表明 `noDirectDependency` 和 `dependencyWithoutInput` 是两个最常用的选项，其余内部细节暂不暴露。 |
| Workflow 分支内的 `noDataFlow` 标记 | 依赖 Graph 的 `addBranch` 支持第三个参数 `noDataFlow`。当前 replica 的 `branch.go` 可能尚未实现此标记，可在 R1 内做 stub（始终为 `false`）或直接默认不传数据。 |
| `values_merge.go` 中 StreamReader 的 `mergeWithNames` 和 `SourceEOF` 逻辑 | 依赖完整的 stream reader 合并抽象。R1 replica 的 stream 层可能尚未支持命名流。将其标记为 R2。 |
| `indirectEdges` 合法性校验（`workflow.go:509` 的 TODO 注释） | Eino 源码本身标记为 TODO。R1 replica 暂不实现。 |

### 4.3 简化建议

1. **路径分隔符**：保持使用 `\x1F`（与 Eino 一致），不做额外抽象。
2. **`customExtractor`**：保留字段但 R1 仅需在 `fieldMap` 中判断 `!= nil` 时调用即可。
3. **`allowMapKeyNotFound`**：`fieldMap` 的第二参数。在 Workflow 上下文中始终为 `false`（缺失时报错）；在 Stream 上下文中 `streamFieldMap` 传 `true`。保持与 Eino 一致。
4. **`WorkflowNode` 不持有 `*graph` 引用**：可以通过持有 func 闭包的方式（`addInputs` 闭包中直接调用 `n.g.addEdgeWithMappings`）实现，与 Eino 一致地持有底层 graph 指针。或者通过 interface 解耦。
5. **类型校验的依赖**：`validateFieldMapping` 需要知道 predecessor 的输出类型和 successor 的输入类型。这些信息需要从 Graph 层获取。在 replica 中，可以通过 `runnable` 包装的 `getInputType`/`getOutputType` 方法获取。

---

## 五、具体 API 草案

### 5.1 FieldMapping API

```go
// compose/field_mapping.go

package compose

// FieldMapping 描述一个字段级别的数据传递。
// from 和 to 字段内部使用 \x1F 字符作为嵌套路径分隔符。
type FieldMapping struct {
    fromNodeKey    string
    from           string   // 空表示整个前驱输出
    to             string   // 空表示后继的整个输入
    customExtractor func(input any) (any, error)
}

// FieldPath 表示嵌套字段路径。每个元素是 struct 字段名或 map key。
type FieldPath []string

// 构造器
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
func (m *FieldMapping) FromPath() FieldPath
func (m *FieldMapping) ToPath() FieldPath
func (m *FieldMapping) TargetPath() FieldPath

// 内部函数
func splitFieldPath(path string) FieldPath
func (fp FieldPath) join() string
func validateFieldMapping(predecessorType, successorType reflect.Type, mappings []*FieldMapping) (
    typeHandler *handlerPair,
    uncheckedSourcePath map[string]FieldPath,
    err error,
)
func fieldMap(mappings []*FieldMapping, allowMapKeyNotFound bool, uncheckedSourcePaths map[string]FieldPath) func(any) (map[string]any, error)
func streamFieldMap(mappings []*FieldMapping, uncheckedSourcePaths map[string]FieldPath) func(streamReader) streamReader
func checkAndExtractFieldType(paths []string, typ reflect.Type) (extracted reflect.Type, remaining FieldPath, err error)
func takeOne(inputValue reflect.Value, inputType reflect.Type, from string) (taken any, takenType reflect.Type, err error)
func assignOne(destValue reflect.Value, taken any, to string) reflect.Value
func convertTo(mappings map[string]any, typ reflect.Type) any
```

### 5.2 Workflow API

```go
// compose/workflow.go

package compose

// dependencyType 表示依赖的类型。
type dependencyType int

const (
    normalDependency    dependencyType = 1   // 数据 + 执行依赖
    noDirectDependency  dependencyType = 2   // 仅数据依赖（不建立直接执行依赖）
    branchDependency    dependencyType = 3   // 分支依赖
)

// WorkflowNode 代表 Workflow 中的一个节点。
type WorkflowNode struct {
    g                *graph
    key              string
    addInputs        []func() error
    staticValues     map[string]any
    dependencySetter func(fromNodeKey string, typ dependencyType)
    mappedFieldPath  map[string]any
}

// Workflow 是对 graph 的声明式包装，用 AddInput 和 FieldMapping 替代 AddEdge。
type Workflow[I, O any] struct {
    g                *graph
    workflowNodes    map[string]*WorkflowNode
    workflowBranches []*WorkflowBranch
    dependencies     map[string]map[string]dependencyType
}

// WorkflowBranch 表示 Workflow 中的一个分支。
type WorkflowBranch struct {
    fromNodeKey string
    *GraphBranch
}

// 构造
func NewWorkflow[I, O any](opts ...NewGraphOption) *Workflow[I, O]

// 核心 API
func (n *WorkflowNode) AddInput(fromNodeKey string, inputs ...*FieldMapping) *WorkflowNode
func (n *WorkflowNode) AddInputWithOptions(fromNodeKey string, inputs []*FieldMapping, opts ...WorkflowAddInputOpt) *WorkflowNode
func (n *WorkflowNode) AddDependency(fromNodeKey string) *WorkflowNode
func (n *WorkflowNode) SetStaticValue(path FieldPath, value any) *WorkflowNode

// 选项
type WorkflowAddInputOpt func(*workflowAddInputOpts)
func WithNoDirectDependency() WorkflowAddInputOpt

// 添加节点（返回 WorkflowNode 供链式操作）
func (wf *Workflow[I, O]) AddLambdaNode(key string, lambda *Lambda, opts ...GraphAddNodeOpt) *WorkflowNode
func (wf *Workflow[I, O]) AddGraphNode(key string, graph AnyGraph, opts ...GraphAddNodeOpt) *WorkflowNode
func (wf *Workflow[I, O]) AddPassthroughNode(key string, opts ...GraphAddNodeOpt) *WorkflowNode

// 分支
func (wf *Workflow[I, O]) AddBranch(fromNodeKey string, branch *GraphBranch) *WorkflowBranch

// 终端
func (wf *Workflow[I, O]) End() *WorkflowNode

// 编译
func (wf *Workflow[I, O]) Compile(ctx context.Context, opts ...GraphCompileOption) (Runnable[I, O], error)
```

### 5.3 与现有 Replica 的集成点

现有的 replica compose 层已实现：
- `graph` 结构体（`compose/graph.go`）
- `AddEdge` / `AddBranch` / `addEdgeWithMappings`
- `handlerPair` 类型
- `handlerPreNode` 映射
- `fieldMappingRecords` 映射

Workflow 需要以下新增集成点：

1. **`addEdgeWithMappings` 签名确认**：验证当前 replica 的实现是否支持 `noDirectDependency bool, isControl bool` 参数。如果不支持，需要在 `graph.go` 中扩展此方法。

2. **`handlerPreNode` 机制**：当前 replica 是否支持编译时为特定节点注入 pre-handler（用于 `SetStaticValue` 的 merge 逻辑）。

3. **`fieldMappingRecords`**：当前 replica 图结构中是否有此字段映射记录（用于自省 / `ToFieldPath` 注册）。

4. **Branch `noDataFlow` 支持**：`addBranch` 是否接受 `noDataFlow bool` 参数（Workflow 分支语义所需）。

---

## 六、测试用例草案

### 6.1 FieldMapping 编译时校验测试

```go
func TestValidateFieldMapping(t *testing.T) {
    type Input struct {
        Name  string
        Age   int
        inner string // 未导出
    }
    type Output struct {
        DisplayName string
        Years       int
    }

    t.Run("合法映射——单字段到单字段", func(t *testing.T) {
        // MapFields("Name", "DisplayName") → 应该编译通过
    })

    t.Run("合法映射——FromFieldPath 嵌套", func(t *testing.T) {
        // FromFieldPath([]string{"A", "F1", "F1"}) → 应该编译通过
    })

    t.Run("非法——前驱字段不存在", func(t *testing.T) {
        // FromField("NonExist") → 编译错误 "has no field[NonExist]"
    })

    t.Run("非法——前驱字段未导出", func(t *testing.T) {
        // FromField("inner") → 编译错误 "has an unexported field[inner]"
    })

    t.Run("非法——类型不匹配", func(t *testing.T) {
        // MapFields("Age", "DisplayName") → 编译错误 "is absolutely not assignable"
    })

    t.Run("非法——FromAll + ToAll", func(t *testing.T) {
        // AddInput("A") 和 AddInput("B") 都被无参数的 AddInput("C") 调用 → 编译错误
    })

    t.Run("延迟检查——interface 中间路径", func(t *testing.T) {
        // output 类型为 any + ToFieldPath → 编译通过（推迟到请求时）
    })
}
```

### 6.2 Workflow 基础行为测试

```go
func TestWorkflow(t *testing.T) {
    ctx := context.Background()

    t.Run("简单三节点 workflow + field mapping", func(t *testing.T) {
        type Input struct {
            Query  string
            UserID string
        }
        type Output struct {
            Reply string
        }

        wf := NewWorkflow[*Input, *Output]()

        // 从 START 提取 Query 字段 → template 节点
        wf.AddLambdaNode("template", InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return "processed:" + in, nil
        })).AddInput(START, FromField("Query"))

        // template 输出 → model 节点
        wf.AddLambdaNode("model", InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return "model:" + in, nil
        })).AddInput("template")

        // model 输出 → END 的 Reply 字段
        wf.End().AddInput("model", MapFields("Content", "Reply"))

        r, err := wf.Compile(ctx)
        require.NoError(t, err)

        result, err := r.Invoke(ctx, &Input{Query: "hello"})
        require.NoError(t, err)
        // 预期结果取决于实际 Lambda 实现
    })

    t.Run("Fan-in 到同一个节点的字段映射", func(t *testing.T) {
        wf := NewWorkflow[string, map[string]any]()

        wf.AddLambdaNode("A", InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return "value_a", nil
        }), WithOutputKey("a")).AddInput(START)

        wf.AddLambdaNode("B", InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return "value_b", nil
        }), WithOutputKey("b")).AddInput(START)

        // fan-in 汇聚到 END：从 A 取 field_a，从 B 取 field_b
        wf.End().
            AddInput("A", MapFields("a", "field_a")).
            AddInput("B", MapFields("b", "field_b"))

        r, err := wf.Compile(ctx)
        require.NoError(t, err)

        result, err := r.Invoke(ctx, "input")
        require.NoError(t, err)
        assert.Contains(t, result, "field_a")
        assert.Contains(t, result, "field_b")
    })

    t.Run("Fan-in 字段路径冲突检测", func(t *testing.T) {
        wf := NewWorkflow[string, map[string]any]()
        wf.AddLambdaNode("A", InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return "a", nil
        }), WithOutputKey("a")).AddInput(START)
        wf.AddLambdaNode("B", InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return "b", nil
        }), WithOutputKey("a")).AddInput(START)

        // 两个节点都试图映射到 map key "a"
        wf.End().
            AddInput("A", MapFields("a", "a")).
            AddInput("B", MapFields("a", "a"))

        _, err := wf.Compile(ctx)
        require.Error(t, err)
        assert.Contains(t, err.Error(), "two terminal field paths conflict")
    })
}
```

### 6.3 执行依赖与数据流解耦测试

```go
func TestDependencySeparation(t *testing.T) {
    ctx := context.Background()

    t.Run("AddDependency——仅控制依赖，无数据传递", func(t *testing.T) {
        wf := NewWorkflow[string, string]()

        wf.AddLambdaNode("setup", InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return "setup_done", nil
        })).AddInput(START)

        wf.AddLambdaNode("main", InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return in + "_processed", nil
        })).AddDependency("setup").
            AddInputWithOptions(START, nil, WithNoDirectDependency())

        wf.End().AddInput("main")

        r, err := wf.Compile(ctx)
        require.NoError(t, err)

        result, err := r.Invoke(ctx, "hello")
        require.NoError(t, err)
        assert.Equal(t, "hello_processed", result)
        // main 节点没有收到 setup 的输出数据
    })

    t.Run("NoDirectDependency——数据传递但无直接执行依赖", func(t *testing.T) {
        wf := NewWorkflow[string, map[string]any]()

        wf.AddLambdaNode("process", InvokableLambda(func(ctx context.Context, in string) (string, error) {
            return "processed_" + in, nil
        })).AddInput(START)

        wf.AddLambdaNode("audit", InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
            return in, nil
        })).
            AddInput("process", ToField("from_process")).
            AddInputWithOptions(START, []*FieldMapping{ToField("from_start")}, WithNoDirectDependency())

        wf.End().AddInput("audit")

        r, err := wf.Compile(ctx)
        require.NoError(t, err)

        result, err := r.Invoke(ctx, "hello")
        require.NoError(t, err)
        assert.Equal(t, "processed_hello", result["from_process"])
        assert.Equal(t, "hello", result["from_start"])
    })
}
```

### 6.4 StaticValue 测试

```go
func TestStaticValue(t *testing.T) {
    ctx := context.Background()

    t.Run("预填充 map", func(t *testing.T) {
        wf := NewWorkflow[string, map[string]any]()

        wf.AddLambdaNode("0", InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
            return in, nil
        })).
            AddInput(START, ToField("input")).
            SetStaticValue(FieldPath{"prefilled"}, "yo-ho")

        wf.End().AddInput("0")

        r, err := wf.Compile(ctx)
        require.NoError(t, err)

        result, err := r.Invoke(ctx, "hello")
        require.NoError(t, err)
        assert.Equal(t, "yo-ho", result["prefilled"])
        assert.Equal(t, "hello", result["input"])
    })

    t.Run("静态值与动态映射冲突检测", func(t *testing.T) {
        wf := NewWorkflow[string, map[string]any]()

        wf.AddLambdaNode("0", InvokableLambda(func(ctx context.Context, in map[string]any) (map[string]any, error) {
            return in, nil
        })).
            AddInput(START, ToField("prefilled")).
            SetStaticValue(FieldPath{"prefilled"}, "yo-ho")

        wf.End().AddInput("0")

        _, err := wf.Compile(ctx)
        require.Error(t, err)
        assert.Contains(t, err.Error(), "two terminal field paths conflict")
    })
}
```

### 6.5 嵌套字段路径测试

```go
func TestNestedFieldPaths(t *testing.T) {
    ctx := context.Background()

    t.Run("FromFieldPath 嵌套 struct", func(t *testing.T) {
        type Inner struct {
            F1 string
        }
        type Input struct {
            F1 *Inner
        }

        wf := NewWorkflow[*Input, string]()
        wf.End().AddInput(START, FromFieldPath(FieldPath{"F1", "F1"}))
        r, err := wf.Compile(ctx)
        require.NoError(t, err)

        result, err := r.Invoke(ctx, &Input{F1: &Inner{F1: "hello"}})
        require.NoError(t, err)
        assert.Equal(t, "hello", result)
    })

    t.Run("ToFieldPath 嵌套 struct", func(t *testing.T) {
        type Inner struct {
            F1 string
        }
        type Output struct {
            F1 *Inner
        }

        wf := NewWorkflow[string, *Output]()
        wf.End().AddInput(START, ToFieldPath(FieldPath{"F1", "F1"}))
        r, err := wf.Compile(ctx)
        require.NoError(t, err)

        result, err := r.Invoke(ctx, "hello")
        require.NoError(t, err)
        assert.Equal(t, &Output{F1: &Inner{F1: "hello"}}, result)
    })

    t.Run("MapFieldPaths——嵌套到嵌套", func(t *testing.T) {
        type Output struct {
            F1 string
        }

        wf := NewWorkflow[map[string]any, *Output]()
        wf.End().AddInput(START, MapFieldPaths(FieldPath{"key1", "key2"}, FieldPath{"F1"}))
        r, err := wf.Compile(ctx)
        require.NoError(t, err)

        result, err := r.Invoke(ctx, map[string]any{
            "key1": map[string]any{
                "key2": "hello",
            },
        })
        require.NoError(t, err)
        assert.Equal(t, &Output{F1: "hello"}, result)
    })
}
```

### 6.6 CustomExtractor 测试

```go
func TestCustomExtractor(t *testing.T) {
    ctx := context.Background()

    t.Run("从数组中提取元素", func(t *testing.T) {
        wf := NewWorkflow[[]int, map[string]int]()
        wf.End().AddInput(START, ToField("first", WithCustomExtractor(func(input any) (any, error) {
            return input.([]int)[0], nil
        })))

        r, err := wf.Compile(ctx)
        require.NoError(t, err)

        result, err := r.Invoke(ctx, []int{1, 2, 3})
        require.NoError(t, err)
        assert.Equal(t, map[string]int{"first": 1}, result)
    })
}
```

---

## 七、实现注意事项

1. **与 Graph 层的双向依赖**：Workflow 持有 `*graph`，通过 `addEdgeWithMappings` 和 `addBranch` 操作底层图。FieldMapping 的 `validateFieldMapping` 需要从 Graph 层获取 predecessor 和 successor 的类型信息（`getNodeOutputType` / `getNodeInputType`）。

2. **延迟闭包模式**：`addInputs: []func() error` 是两阶段编译的关键。必须在 `compile` 中先处理所有分支再执行这些闭包。不要试图在 `AddInput` 调用时立即操作图。

3. **`mappedFieldPath` trie 的线程安全**：Eino 源码中不在并发环境下访问此结构（所有 mapping 在编译前单线程声明），replica 也无需加锁。

4. **流式映射（streamFieldMap）**：如果 replica 尚未完全实现 stream reader 的 `toAnyStreamReader()` 和 `StreamReaderWithConvert`，可以先实现 `fieldMap`（非流版本），`streamFieldMap` 标记为 TODO。

5. **错误类型定义**：`errMapKeyNotFound` 和 `errInterfaceNotValidForFieldMapping` 是 field_mapping 内部的两个自定义错误类型，用于在不同层级区分错误恢复策略。在 `fieldMap` 中使用 `errors.As` 进行类型断言。

6. **`uncheckedSourcePaths`**：当 mapping 路径中包含 interface 类型时，`validateFieldMapping` 返回未检查的路径映射。`fieldMap` 在执行时用此信息决定是按编译期 panic 还是请求期 error 处理。

7. **测试覆盖目标**：FieldMapping 的 6 个构造器 × (struct/map/any interface/嵌套路径/nil值/类型不匹配) ≥ 30 个测试用例。Workflow 的核心路径（普通映射、Fan-in 冲突、依赖分离、分支后手动映射）≥ 15 个测试用例。
