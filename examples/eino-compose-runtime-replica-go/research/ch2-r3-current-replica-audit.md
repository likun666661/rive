# R3 审计：当前 Go 复刻版集成面分析

> 审计目标：扫描 `examples/eino-compose-runtime-replica-go/compose/*.go` 当前代码，识别 Graph/Runner/DAG/Pregel 的实际结构，定位 FieldMapping、Workflow、Chain 三层抽象的精确集成点。

## 1. 当前复刻版架构总览

### 1.1 文件清单与核心职责

| 文件 | 大小 | 核心职责 |
|------|------|----------|
| `types.go` | 47行 | 哨兵错误、`NodeTriggerMode`、`ComponentType`、`START/END` 常量 |
| `runnable.go` | 72行 | `Runnable[I,O]` 接口、`composableRunnable`、`Lambda` (仅 `Invoke`) |
| `graph.go` | 362行 | 核心 `graph` 结构、`AddNode/AddEdge/AddControlEdge/AddBranch`、`compile()` 主流程、Kahn 环检测 |
| `generic_graph.go` | 129行 | `Graph[I,O]` 泛型包装、`NewGraph/Compile/GetGraphInfo`、类型转换 `graphRunnable` |
| `graph_node.go` | 25行 | `graphNode` 结构、子图递归编译 `compileIfNeeded` |
| `graph_compile.go` | 60行 | 编译选项：`WithGraphName/WithNodeTriggerMode/WithMaxRunSteps/WithEagerExecutionDisabled` |
| `graph_run.go` | 171行 | `runner` 结构、`run()` 主循环、`createTasks/resolveCompletedTasks/routeInputToStartNodes` |
| `graph_manager.go` | 179行 | `channel` 接口、`channelManager`、`taskManager` (goroutine 池) |
| `dag.go` | 146行 | `dagChannel`：AllPredecessor 状态机（control + data 前驱）、merge config、skip 传播 |
| `pregel.go` | 51行 | `pregelChannel`：AnyPredecessor 简陋 Values map、先到先得语义 |
| `branch.go` | 23行 | `GraphBranch`：条件函数 + branch map（无 endNodes / noDataFlow 字段） |
| `introspect.go` | 50行 | `GraphInfo/GraphNodeInfo/GraphEdgeInfo`：编译时拓扑导出 |
| `event_log.go` | 95行 | `EventLog`：线程安全执行事件记录 |
| `utils.go` | 7行 | `fmtTypeError` 辅助函数 |
| `graph_test.go` | 2857行 | 70+ 测试用例，覆盖 DAG/Pregel/边界/EventLog |

### 1.2 `graph` 结构体语义（`graph.go:8-26`）

```
graph {
    nodes        map[string]*graphNode    // 普通 Lambda 节点或嵌套子图
    controlEdges map[string][]string      // 控制边（只传递执行依赖，不传递数据）
    dataEdges    map[string][]string      // 数据边（同时传递数据和执行依赖）
    branches     map[string][]*GraphBranch // 条件分支

    // 编译后填充
    chanSubscribeTo     map[string]*chanCall
    dataPredecessors    map[string][]string
    controlPredecessors map[string][]string
    successors          map[string][]string
    startNodes          []string
    endNodes            []string
}
```

关键观察：当前的边模型是"数据边 + 控制边"二元结构，每一条边同时承载执行依赖和数据传递语义。这与 Eino 手册第2章指出的核心设计问题完全一致——"执行依赖与数据映射是两回事"（manual:02-17），但当前复刻版尚未实现这个解耦。

### 1.3 `graphNode` 与子图递归（`graph_node.go:5-10`）

```go
type graphNode struct {
    name string
    cr   *composableRunnable  // 已编译的可运行单元（Lambda 或子图结果）
    g    *graph               // 非空时表示嵌套子图（Workflow/Chain 的底层图）
    info *GraphNodeInfo
}
```

`compileIfNeeded` 已在 `graph_node.go:12-25` 中支持子图递归编译：如果 `gn.cr != nil` 直接返回，否则如果 `gn.g != nil` 就递归 `gn.g.compile(ctx)`。这意味着 `graphNode` 已经预留了嵌套子图的入口，具备了将 Workflow/Chain 展开后挂载为子图的基本能力。

### 1.4 Runner 主循环（`graph_run.go:26-98`）

```
run(ctx, input):
  1. 初始化 channelManager
  2. 初始化所有 channel（DAG 或 Pregel）
  3. routeInputToStartNodes: START → 所有 startNodes
  4. loop:
     a. runStepCount++ 并检查 maxSteps
     b. cm.getReadyChannels() 获取就绪节点
     c. 如果无就绪节点：检查 END channel，break
     d. createTasks → taskManager.submit → taskManager.wait
     e. resolveCompletedTasks: 将输出写入下游 channel，更新依赖
     f. 检查 END channel 是否已有结果
  5. 返回 END channel 中的值
```

当前 runner 不感知分支（`GraphBranch` 在 `graph.go:104-109` 只是附加到 `g.branches` map 中，编译和运行阶段目前未消费分支信息）。分支路由逻辑完全缺失。

### 1.5 Channel 体系

**`channel` 接口**（`graph_manager.go:9-15`）:
- `reportValues(nodeKey, value)` — 上游节点报告数据
- `reportDependency(nodeKey)` — 上游节点报告控制依赖完成
- `reportSkip(nodeKey) bool` — DAG 专有：跳过语义
- `get() (any, bool, error)` — 轮询是否就绪
- `setMergeConfig(fn)` — 多输入合并函数

**DAGChannel**（`dag.go:24-31`）:
- `controlPredecessors map[string]dependencyState` — 三态：Waiting → Ready / Skipped
- `dataPredecessors map[string]bool` — 是否已上报数据
- `mergeValuesFn` — 自定义合并逻辑
- 就绪条件：所有 control 非 Waiting **且** 所有 data 已上报
- 多值自动打包为 `map[string]any`（`dag.go:129-138`）

**PregelChannel**（`pregel.go:3-6`）:
- `values map[string]any` — 简单累积
- 就绪条件：任意 values 非空
- 多值取第一个（先到先得），语义粗糙

### 1.6 Branch

`GraphBranch`（`branch.go:7-10`）当前只包含 `condition func(ctx context.Context, input any) (string, error)` 和 `branchMap map[string]bool`。与 Eino 完整 `GraphBranch`（manual:244-254）相比缺少：
- `invoke` / `collect` 双评估函数
- `endNodes map[string]bool` — 分支的合法终止节点集合
- `noDataFlow bool` — Workflow 分支专用标记

更重要的是，`graph.compile()` 和 `runner.run()` **未集成分支评估**。`graph.AddBranch` 只是将 branch 存入 `g.branches[key]` map，编译和运行时均不消费。

### 1.7 Runnable 抽象

当前只有 `Invoke` 执行形态（`runnable.go:8-10`）。Eino 完整 Runnable 支持 4 种形态：Invoke / Stream / Collect / Transform。

`composableRunnable`（`runnable.go:12-15`）已预留 `s` 字段（Stream 函数指针），`stream()` 方法存在但使用 `invoke` 作为 fallback（`runnable.go:24-36`）。这为后续添加 Stream/Collect/Transform 提供了扩展点。

### 1.8 编译流程（`graph.go:112-248`）

```
compile(ctx):
  1. 填充 GraphInfo 拓扑信息
  2. 为每个节点创建 chanCall（编译子图如需要）
  3. 从 dataEdges / controlEdges 推导 dataPredecessors / controlPredecessors / successors
  4. 计算 startNodes（START 指向的节点）和 endNodes（指向 END 的节点）
  5. DAG 模式执行 Kahn 环检测
  6. 构造 runner 并标记 compiled = true
```

编译 lock 机制（`graph.go:43-48`）保证编译后无法通过 `AddNode / AddEdge / AddControlEdge / AddBranch` 修改图。

## 2. FieldMapping / Workflow / Chain 集成点分析

### 2.1 FieldMapping 集成点

**Eino 的 FieldMapping 位于 `compose/field_mapping.go`**（manual:140-174），包含：
- `FieldMapping` 结构体（fromNodeKey, from, to, customExtractor）
- 六个构造函数（`MapFields`, `FromField`, `ToField`, `MapFieldPaths`, `FromFieldPath`, `ToFieldPath`）
- `validateFieldMapping` 类型检查（手动结构体/map 遍历，interface 延迟检查）
- `fieldMap` 实际提取与赋值（`takeOne` 路径提取）
- `checkAndExtractFieldType` 沿字段路径的类型推导

**当前复刻版中 FieldMapping 完全不存在**。需要在以下位置集成：

| 集成点 | 当前状态 | 需要新增 |
|--------|----------|----------|
| 新文件 `field_mapping.go` | 不存在 | FieldMapping 类型、构造函数、validateFieldMapping、fieldMap |
| `graph.go` — `addEdgeWithMappings` | 不存在 | 带 FieldMapping 的边注册方法（替代或扩展 AddEdge） |
| `graph.go` — `compile()` 中的类型验证 | 不存在 | 在编译阶段调用 `validateFieldMapping` |
| `graph_run.go` — `routeInputToStartNodes` | 只做全值透传 | 支持 FieldMapping 驱动的字段提取 |
| `dag.go` / `pregel.go` — `get()` | 支持 `mergeValuesFn` | 通过 `mergeValuesFn` 整合字段映射合并 |
| `types.go` | 无 FieldMapping 相关 | 允许 map key not found 的错误语义 |

### 2.2 Workflow 集成点

**Eino 的 Workflow 位于 `compose/workflow.go`**（manual:81-134），核心特征：
- `Workflow[I,O]` 包装 `*graph`
- `workflowNodes map[string]*WorkflowNode`
- `dependencies map[string]map[string]dependencyType`
- `WorkflowNode.AddInput(from, ...fieldMappings)` 建立数据+执行依赖
- `AddInputWithOptions(...WithNoDirectDependency())` 仅数据依赖
- `AddDependency(from)` 仅执行依赖
- `AddBranch` 在 Workflow 层面，分支不传递数据
- `SetStaticValue` 编译时静态值注入
- `compile` 方法：先处理所有 branch → 统一执行 `addInputs` 闭包 → 调用底层 `g.compile()`

**当前复刻版中 Workflow 完全不存在**。需要在以下位置集成：

| 集成点 | 当前状态 | 需要新增 |
|--------|----------|----------|
| 新文件 `workflow.go` | 不存在 | Workflow 类型、WorkflowNode、依赖追踪表、compile 方法 |
| `types.go` | `ComponentOfWorkflow` 已定义但未使用 | dependencyType 常量（normalDependency / noDirectDependency / branchDependency） |
| `graph.go` — `addEdgeWithMappings` | 不存在 | 接收 FieldMapping 参数的边添加方法，支持 noDirectDependency 标记 |
| `graph.go` — `AddBranch` | 仅存入 map，不消费 | 需要在 compile 中消费 branch 信息，并在 runner 中执行分支路由 |
| `branch.go` — `GraphBranch` | 缺少 `endNodes` / `noDataFlow` 字段 | 扩展 GraphBranch 结构以支持 Workflow 分支语义 |
| `graph_run.go` | `resolveCompletedTasks` 不处理 branch | 在节点完成后评估 branch，根据条件路由到分支目标 |
| `graph_node.go` — `graphNode` | 无静态值字段 | 可能需要 `staticValues` 或 handler 注入机制 |

**最关键的架构决策**：Workflow 的 `addInputs` 使用 `[]func() error` 延迟闭包模式（manual:134行），即"先声明所有节点/分支，后统一建图"。这要求在 `graph.go` 层面支持"延迟边注册"或在 Workflow 内部缓存所有 relationship，在 compile 时统一写入底层 graph。

### 2.3 Chain 集成点

**Eino 的 Chain 位于 `compose/chain.go`**（manual:177-204），核心特征：
- `Chain[I,O]` 包装 `*graph`
- `preNodeKeys []string` 追踪链尾节点
- `AppendLambda/AppendChatTemplate/AppendChatModel/...` builder 方法
- `AppendParallel` — 并行节点组
- `AppendBranch` — 条件分支（`ChainBranch` 封装 `GraphBranch` + 节点映射）
- `AppendPassthrough` / `AppendGraph` — 汇聚 / 子图嵌套
- `addNode` 核心：从所有 `preNodeKeys` 建边到新节点，新节点成为唯一 preNodeKey
- `addEndIfNeeded`：编译前将所有 `preNodeKeys` 连接 END
- `nextNodeKey`：自动节点命名（`node_0`, `node_0_parallel_0`, `node_1_branch_customkey`）

**当前复刻版中 Chain 完全不存在**。需要在以下位置集成：

| 集成点 | 当前状态 | 需要新增 |
|--------|----------|----------|
| 新文件 `chain.go` | 不存在 | Chain 类型、preNodeKeys 跟踪、addNode/addEndIfNeeded/nextNodeKey |
| 新文件 `chain_parallel.go` | 不存在 | Parallel 类型、AddChatModel/AddLambda/WithOutputKey |
| 新文件 `chain_branch.go` | 不存在 | ChainBranch 类型、NewChainBranch/NewChainMultiBranch |
| `generic_graph.go` — `Graph[I,O]` | 无 `AddGraphNode` | 需要 `AddGraphNode` 方法支持子图/子 Chain/子 Workflow 挂载 |
| `graph_node.go` | 已支持子图递归 | `graphNode.g` 已预留，但需要暴露 `AddGraphNode` API |
| `branch.go` | `GraphBranch` 结构简单 | 需要扩展以支持 `invoke/collect` 双评估函数（Chain/Invoke vs Chain/Stream） |

### 2.4 当前 Graph 层缺失的关键扩展点

以下是底层 graph 需要新增或修改以支撑上层三抽象的能力：

```
graph 层需新增:
  ✗ addEdgeWithMappings(from, to, noDirectDependency, noDataFlow, ...fieldMappings)
     — 当前只有 AddEdge(from, to)，无字段映射参数
  ✗ handlerPreNode 机制
     — Eino 用于在编译阶段注入 mergeValues handler（静态值、字段映射）
  ✗ chanCall 扩展
     — 当前 writeTo map[string]bool 无法区分数据边 vs 字段映射边
  ✗ channelManager 分支路由支持
     — 当前 getReadyChannels 不处理条件分支激活/跳过

graphNode 需新增:
  ✗ graphNode 的结构体字段不足以表达 WorkflowNode 的元数据
     — 缺少 staticValues、mappedFieldPath、dependencySetter

GraphBranch 需新增:
  ✗ invoke / collect 双评估函数
  ✗ endNodes map[string]bool
  ✗ noDataFlow bool

graph 结构体需新增:
  ✗ mergeConfigs / handlerPreNode 存储
  ✗ compiled 后 immutable runner 的缓存机制（当前 recompile 每次都新编译）
```

### 2.5 与 Eino 源手册的差异对照

| 手册描述 | 复刻版现状 | 差距 |
|----------|-----------|------|
| Workflow 包装 Graph（manual:85-91） | Graph 独立存在，无 Workflow 层 | 缺整个 workflow.go |
| AddInput 通过闭包延迟执行（manual:134） | 无此机制 | graph.go 需支持 deferred edge registration |
| 三种依赖类型（normal/noDirect/branch） | 无 | types.go 无 dependencyType |
| FieldMapping 六种构造函数（manual:154-161） | 无 | 缺 field_mapping.go |
| validateFieldMapping — compile time type check（manual:163-168） | 无类型检查 | graph compile 需扩展 type check pass |
| SetStaticValue（manual:417-420） | 无 | 需 graphNode 扩展 + compile 注入 |
| Chain builder — preNodeKeys（manual:179-194） | 无 | 缺 chain.go |
| Parallel — WithOutputKey（manual:214-218） | 无 | 缺 chain_parallel.go |
| ChainBranch — 双评估函数（manual:221-238） | 无 | 缺 chain_branch.go |
| GraphBranch 的 noDataFlow 标记（manual:473-476） | 无 | branch.go 缺此字段 |
| Runnable 的 Stream/Collect/Transform | 仅有 Invoke 和 fallback Stream | runnable.go 需扩展 |
| 嵌套图 — AddGraphNode（manual:426-435） | graphNode.g 已预留但无公开 API | generic_graph.go 需 AddGraphNode 方法 |
| 统一编译为 Runnable[I,O]（manual:73-75） | graphRunnable 已实现 | 架构基础已具备 |

## 3. 现有约束与风险

### 3.1 图合并语义（Graph Merge）

- **风险**：Workflow/Chain 编译时会被展开为底层 graph，这意味着 Workflow 内的节点可能与外部节点 key 冲突。Eino 通过 prefix 机制（如 `chain.go:544-548` 的 `nextNodeKey`）和编译时命名空间隔离解决此问题。当前复刻版 graph 无前缀机制。
- **影响**：嵌套 Workflow/Chain 时，内部节点 key 可能覆盖父 graph 的同名节点。

### 3.2 Branch 状态管理

- **当前状态**：`GraphBranch` 仅存储于 `graph.branches` map，编译不消费，运行不评估。完全不可用。
- **风险**：即使添加了 branch 评估，当前 runner 的 step loop 无法区分"分支未激活"和"节点未就绪"两种状态。`dagChannel.reportSkip` 机制已预留但未连接到 branch 路由。
- **缺失**：`channelManager` 无 `reportSkip` 的调用路径。当 branch 决定跳过某个分支时，需要通知该分支内所有节点的 channel 标记为已 skip。

### 3.3 Runner 中的 Branch 路由集成缺失

当前 `resolveCompletedTasks`（`graph_run.go:153-171`）只做简单的前驱值传播，不检查 `g.branches`。需要完整重构为：
1. 节点完成后，检查该节点是否有 `GraphBranch`
2. 若有，调用 `branch.condition(ctx, output)` 获取目标分支 key
3. 将数据只路由到选中的目标 nodeKey，其他分支节点通过 `reportSkip` 标记

### 3.4 Test 矩阵缺口

当前 `graph_test.go` 有 70+ 测试（2857行），覆盖了较全面的 DAG/Pregel 基础场景。但以下关键测试缺失：

- **无 FieldMapping 测试**：字段提取、类型检查、映射冲突、interface 延迟检查
- **无 Workflow 测试**：AddInput、WithNoDirectDependency、AddDependency、SetStaticValue、Workflow Branch、分支不传递数据
- **无 Chain 测试**：线性 Append、AppendParallel、AppendBranch+汇聚、preNodeKeys 自动管理
- **无嵌套图测试**：Workflow 内嵌 Chain、Chain 内嵌 Workflow
- **无 Branch 运行时测试**：条件路由、分支跳过、多分支汇聚
- **无 Stream/Collect/Transform 测试**
- **无 error 恢复测试**：节点失败后的图状态、部分执行回滚

### 3.5 示例代码缺口

`cmd/example/main.go` 仅展示 5 个基础场景（DAG、Pregel+maxSteps、编译边界、GraphInfo、EventLog）。无法演示 Workflow/Chain/FieldMapping 的使用。

## 4. 推荐最小实现计划

### 4.1 实现优先级与依赖关系

```
Phase 1: Graph 层扩展（必须先于 Phase 2/3）
  ├── graph.go: addEdgeWithMappings(from, to, noDirectDependency, noDataFlow, ...FieldMapping)
  ├── graph.go: compile() 集成 branch 消费 + channelManager 初始化
  ├── branch.go: 扩展 GraphBranch（endNodes, noDataFlow, invoke+collect 双函数）
  ├── graph_run.go: resolveCompletedTasks 中集成 branch 评估与 skip 传播
  ├── generic_graph.go: 添加 AddGraphNode(key, subRunnable) 方法
  └── types.go: 添加 dependencyType 常量

Phase 2: FieldMapping（Phase 1 的消费者）
  └── field_mapping.go: FieldMapping 类型 + 6个构造函数 + validateFieldMapping + fieldMap

Phase 3a: Workflow（依赖 Phase 1+2）
  ├── workflow.go: Workflow[I,O], WorkflowNode, AddInput/AddInputWithOptions,
  │   AddDependency, SetStaticValue, WorkflowBranch, compile()
  └── branch.go: 添加 noDataFlow 支持

Phase 3b: Chain（可独立于 Phase 3a）
  ├── chain.go: Chain[I,O], preNodeKeys, AppendLambda/..., addNode,
  │   addEndIfNeeded, nextNodeKey
  ├── chain_parallel.go: Parallel 类型, AddChatModel/AddLambda
  ├── chain_branch.go: ChainBranch, NewChainBranch/NewChainMultiBranch
  └── 依赖 graph_level_branch_routing (Phase 1 的 branch 评估)
```

### 4.2 推荐 MVP 范围（本章 R3 节点输出后）

**必须实现（R4 阶段）**：
1. `branch.go` 扩展 + `graph_run.go` branch 集成
2. `field_mapping.go` 完整实现
3. `graph.go: addEdgeWithMappings`
4. `types.go: dependencyType`

**建议在 R5+ 阶段实现**：
5. `workflow.go`
6. `chain.go` + `chain_parallel.go` + `chain_branch.go`

### 4.3 非目标（Skipped）

以下不在当前复刻版范围内：
- **Stream / Collect / Transform** 执行形态（Runnable 扩展）：Phase 1 仅保持 `Invoke`，Stream fallback 已预留
- **Callback** 机制：Eino 的 `OnStart/OnEnd/OnError` callback 体系
- **State** 传递：`graph.state any` 字段已定义但未在任何地方使用
- **Checkpoint / Recovery**：可恢复执行的中断-恢复机制
- **ChatModel / ChatTemplate / Retriever / Indexer** 组件节点：当前仅 `Lambda`
- **Golang 原生泛型反射**：当前 `addEdgeWithMappings` 不需要真实的类型推导，可以用 `any` + `validateFieldMapping` 替代

## 5. 文件归属与测试矩阵

### 5.1 实现 Worker 文件归属

```
Worker A: Graph 层扩展
  文件: compose/branch.go, compose/graph.go, compose/graph_run.go,
        compose/generic_graph.go, compose/types.go
  新增: compose/field_mapping.go
  改动: 约 250–350 行新增代码

Worker B: Workflow 层
  文件: compose/workflow.go（新增）
  改动: 约 400–550 行

Worker C: Chain 层
  文件: compose/chain.go, compose/chain_parallel.go, compose/chain_branch.go（新增）
  改动: 约 500–700 行

Worker D: 测试与示例
  文件: compose/graph_test.go（扩展）, compose/workflow_test.go（新增）,
        compose/chain_test.go（新增）, compose/field_mapping_test.go（新增）,
        cmd/example/main.go（扩展）
  改动: 约 800–1200 行测试代码
```

### 5.2 测试矩阵（实现完成后必须覆盖）

| 测试类别 | 测试文件 | 核心用例 |
|----------|----------|----------|
| FieldMapping 基础 | `field_mapping_test.go` | MapFields / FromField / ToField 正确提取；MapFieldPaths 嵌套提取；FromAll+ToAll 拒绝 |
| FieldMapping 类型 | `field_mapping_test.go` | struct 类型检查通过；map 类型检查通过；interface 中间类型延迟检查；assignableTypeMustNot 拒绝 |
| FieldMapping 执行 | `field_mapping_test.go` | takeOne 从 struct 提取；takeOne 从 map 提取；nil interface 报错；map key not found 跳过/报错 |
| Graph addEdgeWithMappings | `graph_test.go` | 字段映射边正确传播数据；noDirectDependency 标记正确存储；noDataFlow 标记阻止数据传递 |
| Graph Branch 运行时 | `graph_test.go` | 单分支路由正确激活；未选中分支节点 skip 传播；多分支汇聚 map 合并 |
| Graph Branch 编译 | `graph_test.go` | branch endNodes 验证；branch 未注册目标报错 |
| Workflow AddInput | `workflow_test.go` | 基本数据+执行依赖；FieldMapping 正确传播；多个 AddInput 路径冲突检测 |
| Workflow NoDirectDependency | `workflow_test.go` | 仅有数据依赖无直接执行依赖；缺少间接路径时数据丢失；间接路径存在时正常 |
| Workflow AddDependency | `workflow_test.go` | 纯执行依赖无数据传递；被依赖方完成后激活 |
| Workflow Branch | `workflow_test.go` | 分支不传递数据（noDataFlow=true）；分支节点显式 AddInput 获取数据 |
| Workflow SetStaticValue | `workflow_test.go` | 编译时静态值注入；运行时不可修改 |
| Workflow 编译 | `workflow_test.go` | addInputs 延迟闭包正确执行顺序；编译后锁定不可修改 |
| Chain 线性 Append | `chain_test.go` | AppendX 链式调用正确建边；preNodeKeys 跟踪正确 |
| Chain Parallel | `chain_test.go` | 并行节点输出 key 不冲突；OutputKey 映射正确；汇聚节点接收 map |
| Chain Branch | `chain_test.go` | ChainBranch 正确封装 GraphBranch；分支后 preNodeKeys 包含所有分支节点 |
| Chain 汇聚 | `chain_test.go` | AppendPassthrough 正确透传；addEndIfNeeded 正确自动连接 |
| Chain 嵌套 | `chain_test.go` | Chain 内嵌 Graph/Workflow 通过 AppendGraph |
| 边界条件 | `graph_test.go` | 编译锁对所有 mutation 生效；空分支条件；maxSteps 边界；图无 START/END 报错 |
| 并发安全 | 各测试文件 | 并行执行多图不互相影响；EventLog 线程安全 |
| 集成示例 | `cmd/example/main.go` | Workflow DAG 示例；Chain Parallel 示例；Chain Branch 示例 |

### 5.3 当前测试文件结构参考

当前 `graph_test.go`（2857行）包含的测试模式可供新测试复用：

- **Table-driven 不存在**：全部是独立 `TestXxx` 函数
- **命名规范**：`Test<Feature><Scenario>`，如 `TestDAGFanIn`、`TestGraphBranchTypeMismatch`
- **setup 模式**：每个测试独立创建 `NewGraph` → `AddLambdaNode` → `AddEdge` → `Compile` → `Invoke`
- **错误断言**：`errors.Is(err, ErrDAGHasCycle)` 检查哨兵错误
- **辅助函数**：包级 `nodeIdentity/nodeToUpper/nodeReverse/nodeFailing` 四个可复用测试节点
- **并发测试**：`TestConcurrentGraphInvokes` 使用 `sync.WaitGroup` + error channel

---

*审计完成时间：2026-06-03*
*审计范围：compose/ 下 15 个源文件 + 1 个测试文件 + cmd/example/main.go + README.md + manual chapter 2*
*关键发现：当前复刻版仅实现了 Graph 层（DAG+Pregel），完全缺少 FieldMapping / Workflow / Chain 三层抽象以及 Branch 运行时路由。graph_node.g 已预留子图扩展点，composableRunnable.s 已预留 Stream 扩展点。*
