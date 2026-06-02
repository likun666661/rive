# CHANGELOG — 测试补充与审阅结论

本文档记录对 `examples/eino-compose-runtime-replica-go` Go 副本的测试补全与审阅结果，说明各测试覆盖点与 Eino（CloudWeGo）设计的对应关系。

## 1. 重复节点 (Duplicate Node)

**Eino 设计**: 同一个 `nodeKey` 在一个 graph 内只能定义一次；重复添加应覆盖前一次定义（或报错）。本实现采用“后添加覆盖先添加”策略。

**已有测试**:
- `TestDuplicateNode` — DAG 模式 (`AllPredecessor`) 下重复添加同名节点，验证后者生效。

**本次补充**:
- `TestDuplicateNodePregel` — Pregel 模式 (`AnyPredecessor`) 下重复添加同名节点，验证后者覆盖前者，保证 DAG / Pregel 行为一致。

## 2. 未知边 (Unknown Edge)

**Eino 设计**: `AddEdge` / `AddControlEdge` 的 `from` 或 `to` 为不存在的节点（包括非 `START`/`END` 的虚拟节点）时应返回错误。

**已有测试**:
- `TestUnknownEdgeSource` — 未知源节点
- `TestUnknownEdgeTarget` — 未知目标节点
- `TestUnknownControlEdge` — 控制边未知目标
- `TestUnknownEdgeTargetEnd` — 目标非 `END` 的未知节点

**本次补充**:
- `TestUnknownEdgeFromStart` — 从 `START` 出发指向不存在的目标，验证 `ErrNodeNotFound`
- `TestUnknownControlEdgeFromStart` — 控制边从 `START` 出发指向不存在的目标，验证 `ErrNodeNotFound`

## 3. 编译锁 (Compile Lock)

**Eino 设计**: `Compile()` 调用后，graph 进入“已编译”状态，禁止后续结构变更（`AddNode`、`AddEdge`、`AddControlEdge`、`AddBranch` 等），返回 `ErrGraphCompiled`。

**已有测试**:
- `TestCompileLockAddNode`
- `TestCompileLockAddEdge`
- `TestCompileLockAddControlEdge`
- `TestCompileLockAddBranch`

**本次补充**:
- `TestCompileLockMutationsAfterCompile` — 将 AddNode / AddEdge / AddControlEdge 三类锁操作统一验证，确保编译后所有变更入口均返回 `ErrGraphCompiled`。

## 4. GraphInfo / 内省 (Introspection)

**Eino 设计**: `GraphInfo` 提供 graph 的运行时元信息，包括节点列表、边列表、触发模式 (`TriggerMode`)、`MaxSteps`、`DAGMode` / `PregelMode` 等。

**已有测试**:
- `TestGraphInfoDAGMode` — DAG 模式下的完整 `GraphInfo` 校验（名称、模式、节点/边数量、输入/输出类型、节点名列表）
- `TestGraphInfoPregelMode` — Pregel 模式下的 `GraphInfo` 校验
- `TestGraphInfoNodeDetails` — 校验 Node 的 `Component` 类型
- `TestGraphInfoEdgeDetails` — 校验边数量
- `TestNewGraphInfoDefaults` — `newGraphInfo` 的默认值校验
- `TestGraphInfoAddNodeEdgeCounts` — `addNode` / `addEdge` 计数方法校验

**本次补充**:
- `TestGraphInfoWithoutName` — 无 `WithGraphName` 选项时 GraphInfo 仍正确填充类型、节点、边信息
- `TestGraphInfoEdgesWithStartEnd` — 校验 `START`/`END` 边是否正确记录在 GraphInfo 中

## 5. DAG 扇入 / AllPredecessor

**Eino 设计**: `AllPredecessor` 模式下，节点等所有数据前驱都上报值后才触发（扇入语义）。单前驱时直接传递值，多前驱时合并为 `map[string]any`。

**已有测试**:
- `TestDAGFanIn` — 两个分支汇聚到 merger 节点
- `TestDAGFanInMultiInput` — 多输入扇入
- `TestDAGSimpleFanOut` — 扇出（一个节点广播到多个下游）
- `TestMultipleStartNodes` — 多条 `START` 边同时触发

**本次补充**:
- `TestDAGFanInSinglePredecessor` — 单前驱节点在 DAG 模式下直接接收值（不包 `map`）
- `TestDAGFanInWithMergeConfig` — DAG Channel 的自定义合并函数 (`setMergeConfig`) 校验

## 6. DAG 环检测 (DAG Cycle Rejection)

**Eino 设计**: `AllPredecessor` 模式下编译时必须使用 Kahn 拓扑排序检测环。检测到环返回 `ErrDAGHasCycle`。

**已有测试**:
- `TestDAGCycleRejection` — 三节点环 (a→b→c→a)
- `TestDAGThreeNodeCycle` — 两节点环 (y↔z)
- `TestDAGControlCycle` — 仅由控制边构成的环

**本次补充**:
- `TestDAGCycleRejectionSelfLoop` — 直接自环 (loop→loop)，验证 DAG 模式拒绝
- `TestDAGCycleRejectionMixedEdges` — 数据边 + 控制边混合构成的环

## 7. Pregel 允许环 (Pregel Cycle Allowed)

**Eino 设计**: `AnyPredecessor` (Pregel) 模式下，graph 不做环检测，允许迭代 / 循环图结构。

**已有测试**:
- `TestPregelCycleAllowed` — 两节点互指 (a↔b)，编译成功

**本次补充**:
- `TestPregelCycleAllowedSelfLoop` — 自环图在 Pregel 模式下编译成功（DAG 模式会拒绝）；由于自环无限触发，运行时达到 maxSteps 退出
- `TestPregelCycleAllowedMultiNode` — 多节点环在 Pregel 下编译成功，验证非自环循环场景

## 8. maxSteps 超限 (MaxSteps Exceeded)

**Eino 设计**: Pregel 模式依赖 `maxSteps` 作为安全上限，防止循环图无限执行。到达上限返回 `ErrExceedMaxSteps`。默认值 100。

**已有测试**:
- `TestMaxStepsExceeded` — 自环 + maxSteps=3，验证 `ErrExceedMaxSteps`
- `TestMaxStepsNotHitWhenBelow` — 线性图，maxSteps=50，正常完成
- `TestDefaultMaxSteps` — 未指定 maxSteps 时默认 100

**本次补充**:
- `TestMaxStepsExceededSelfLoop` — 自环 + maxSteps=2，验证低阈值时也能正确超限报错

## 9. 事件日志 (Event Log)

**Eino 设计**: `EventLog` 提供线程安全的 graph 执行事件记录，包括 `node_start`、`node_end`、`node_error`、`node_skipped`、`graph_start`、`graph_end`、`graph_error`、`max_steps_hit`、`channel_ready`、`checkpoint` 十种事件。

**已有测试**:
- `TestEventLogLifecycle` — 完整生命周期事件顺序
- `TestEventLogNodeError` — 节点/图错误事件
- `TestEventLogMaxStepsHit` — maxSteps 触发事件
- `TestEventLogString` — String() 格式化输出
- `TestEventLogThreadSafety` — 100 协程并发写入
- `TestEventLogEmpty` — 空 EventLog 行为
- `TestEventLogAllEventTypes` — 所有 8 种主要事件类型的覆盖

**本次补充**:
- `TestEventLogIntegrationWithRunner` — 将 EventLog 挂载到 runner，通过实际 graph 执行验证 `graph_start`、`node_start`、`node_end` 事件的产生
- `TestEventLogNilSafety` — 验证 Pregel Channel 的 `reportDependency` / `reportSkip` 空操作行为

## 10. 其他补充

**本次补充**:
- `TestChannelManagerGetEndChannel` — 验证 `channelManager.getEndChannel()` 取值与消费
- `TestChannelManagerGetReadySkipPregel` — 验证 Pregel Channel 在 `channelManager` 中的就绪行为
- `TestPregelChannelSetMergeConfigThenGet` — Pregel Channel 合并函数校验
- `TestCompiledGraphRecompileWithModeSwitch` — DAG → Pregel 模式切换后 GraphInfo 正确更新

## 审阅结论

### 与 Eino 设计对照

| Eino 特性 | Go Replica 实现 | 测试覆盖 |
|---|---|---|
| Graph 泛型组合 (Graph[I,O]) | `generic_graph.go` | 覆盖 string / int / bool / any / struct 类型 |
| 触发模式 (NodeTriggerMode) | `AnyPredecessor` / `AllPredecessor` | DAG 与 Pregel 模式全覆盖 |
| 数据边 + 控制边 | `dataEdges` + `controlEdges` | 独立边、混合边、扇入/扇出均覆盖 |
| 分支 (Branch) | `GraphBranch` + `AddBranch` | 类型断言、分支映射、编译锁均覆盖 |
| Graph 编译锁 | `ErrGraphCompiled` | AddNode/Edge/Control/Branch 四入口覆盖 |
| DAG 环检测 (Kahn) | `checkDAGCycles()` | 自环、三节环、混合边环全覆盖 |
| Pregel 迭代 | 允许循环 + maxSteps 上限 | 多节环、自环 + 超限退出 |
| EventLog 事件系统 | 10 种事件 + 线程安全 | 生命周期、错误、并发、格式化全覆盖 |
| GraphInfo 内省 | 节点/边/模式/类型信息 | DAG/Pregel 模式、类型、节点详情全覆盖 |
| Lambda 封装 | `InvokableLambda` + 泛型 | Invoke / Stream / 类型错误全覆盖 |

### 状态

- 所有测试通过 (`go test ./...`)
- 代码格式化通过 (`gofmt -w .`)
- 本次补测共新增 17 个测试函数，强化了各类别的边界条件覆盖
