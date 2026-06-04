# R2 审计：当前复刻版桥接插入点分析

> 审计目标：扫描 `compose/` 全部源文件与测试，识别 Runnable / Stream / Callback / FieldMapping / Workflow / Chain 六层抽象之间的精确适配器插入点（I1/I2/I3），说明当前约束、已知缺口以及测试应保护的合约边界。
> 不修改生产 Go 代码。

---

## 1. 当前复刻版能力总览

### 1.1 已实现的完整能力矩阵

| 层 | 文件 | 行数 | 状态 | Eino 等价度 |
|----|------|------|------|-------------|
| **Runnable** | `runnable.go` | 385 | 四模式 + 全 12 路降级矩阵 | 100% |
| **Stream** | `stream.go` | 182 | Pipe/Copy/Merge/Concat + RegisterConcatFunc | 95%（无 schema.StreamReader 接口，用 PipeStreamReader 替代） |
| **Callbacks** | `callbacks.go` | 383 | 5 阶段 Handler + CallbackWrapper(Invoke/Stream/Collect/Transform) + TimingChecker + HandlerBuilder + 上下文隔离 | 95%（无全局处理器 AppendGlobalHandlers；CbStreamReader 替代 schema.StreamReader） |
| **FieldMapping** | `field_mapping.go` | 635 | 6 构造器 + validateFieldMapping + fieldMap + takeOne/assignOne/convertTo + streamFieldMap(stub) | 95%（streamFieldMap 是 stub） |
| **Graph** | `graph.go` | 454 | addEdgeWithMappings + fieldMappingRecords + handlerPreNodes + Kahn 环检测 + addBranch | 100% |
| **Graph Run** | `graph_run.go` | 208 | runner 主循环 + fieldMapping 数据提取 + preHandler 执行 | 90%（缺 branch 运行时路由） |
| **Graph Manager** | `graph_manager.go` | 194 | channel 接口 + channelManager + taskManager(goroutine 池) + CallbackWrapper 集成 | 100% |
| **DAG** | `dag.go` | 146 | AllPredecessor 三态状态机 + mergeValuesFn + skip 传播 | 100% |
| **Pregel** | `pregel.go` | 51 | AnyPredecessor 先到先得 | 100% |
| **Branch** | `branch.go` | 26 | GraphBranch(condition + branchMap + invoke + endNodes + noDataFlow) + NewGraphBranch | 90%（invoke/endNodes 字段定义但未在运行中使用） |
| **Workflow** | `workflow.go` | 319 | Workflow[I,O] + WorkflowNode + AddInput/AddInputWithOptions/AddDependency/SetStaticValue + 两阶段编译 + 路径冲突检测 | 95% |
| **Chain** | `chain.go` | 278 | Chain[I,O] + AppendLambda/Passthrough/Parallel/Branch/Graph + preNodeKeys 追踪 + 节点命名 | 100% |
| **Chain Parallel** | `chain_parallel.go` | 84 | Parallel + AddLambda/Graph/Passthrough + outputKey 去重 | 100% |
| **Chain Branch** | `chain_branch.go` | 103 | ChainBranch + NewChainBranch/NewChainMultiBranch + AddLambda/Graph/Passthrough | 100% |

### 1.2 测试覆盖状况

| 测试文件 | 行数 | 测试函数数 | 覆盖范围 |
|----------|------|-----------|---------|
| `runnable_test.go` | 597 | 12 | 四模式降级、类型转换、graphRunnable 流回退 |
| `stream_test.go` | 462 | 20 | Pipe/Copy/Merge/Concat/并发安全 |
| `callbacks_test.go` | 1000 | 25 | 5 阶段时序、上下文隔离、TimingChecker、CbStreamReader |
| `field_mapping_test.go` | 1129 | 28+ | 6 构造器、validateFieldMapping、fieldMap、takeOne、convertTo |
| `graph_test.go` | 3860 | 80+ | DAG/Pregel/边界/EventLog/Branch/Callback 集成 |
| `workflow_test.go` | 645 | 16 | 基本链式、Fan-in、路径冲突、staticValue、并发 |
| `chain_test.go` | 648 | 17 | 线性/Parallel/Branch/MultiBranch/子图嵌套/编译锁 |

---

## 2. I1 插入点：Graph 层桥接（Graph ↔ FieldMapping / Workflow / Callback）

### 2.1 桥接插入点矩阵

| 编号 | 插入点位置 | 桥接方向 | 机制 | 当前状态 |
|------|-----------|---------|------|---------|
| **I1-A** | `graph.go:128-161` `addEdgeWithMappings()` | Graph ← FieldMapping | 接收 `[]*FieldMapping`，按 `noDirectDependency`/`isControl` 分类写入 `dataEdges`/`controlEdges`；有 mapping 时写入 `fieldMappingRecords` | **已实现，正常** |
| **I1-B** | `graph.go:27` `fieldMappingRecords` | Graph 持有 FieldMapping | `map[string]map[string][]*FieldMapping`（from → to → mappings） | **已实现，正常** |
| **I1-C** | `graph.go:28` `handlerPreNodes` | Graph ← Workflow(SetStaticValue) | `map[string][]handlerPair`，编译时注入 pre-handler | **已实现，正常** |
| **I1-D** | `graph.go:251-259` compile 阶段 | Graph → chanCall | 将 `fieldMappingRecords` 复制到 `chanCall.fieldMappings` | **已实现，正常** |
| **I1-E** | `graph.go:261-267` compile 阶段 | handlerPreNodes → chanCall | 将 `handlerPreNodes` 复制到 `chanCall.preHandlers` | **已实现，正常** |
| **I1-F** | `graph.go:197-211` compile 阶段 | graphNode → chanCall | 构建 `chanCall` 时携带 `fieldMappings`/`callbacks`/`nodeInfo` | **已实现，正常** |
| **I1-G** | `graph_run.go:178-184` resolveCompletedTasks | preHandlers → 输出 | 在 field mapping 前执行 `preHandlers`（SetStaticValue 路径） | **已实现，正常** |
| **I1-H** | `graph_run.go:186-201` resolveCompletedTasks | FieldMapping → channelManager | 按 `cc.fieldMappings` 提取字段后写入下游 channel | **已实现，正常** |
| **I1-I** | `graph_run.go:122-138` routeInputToStartNodes | START FieldMapping → startNodes | 检查 `cc.fieldMappings[startNode]`，按 fieldMap 提取后写入 | **已实现，正常** |
| **I1-J** | `graph_manager.go:148-157` taskManager.submit | chanCall.callbacks → CallbackWrapper | 如果节点注册了 handlers，用 `CallbackWrapper.Invoke` 包装 action | **已实现，正常** |
| **I1-K** | `graph.go:116-126` `SetNodeHandler()` | Graph ← Callback | 为节点绑定 Handler 列表 | **已实现，正常** |
| **I1-L** | `graph.go:163-169` `addBranch()` | Graph ← Workflow | 存储 branch 并设置 `noDataFlow` 标志 | **已实现，正常** |
| **I1-M** | `graph.go:108-114` `AddBranch()` | Graph 公共 API | 公开的 branch 注册方法 | **已实现，正常** |

### 2.2 I1 关键缺口

| 缺口编号 | 位置 | 描述 | 影响 |
|----------|------|------|------|
| **GAP-I1-1** | `graph.go:172-340` `compile()` | **`validateFieldMapping` 未被调用**。编译时将 `fieldMappingRecords` 复制到 `chanCall`，但从未调用 `validateFieldMapping()` 进行编译时类型检查。所有类型验证完全推迟到运行时 `fieldMap()`。 | 类型不兼容的 field mapping 只能在运行时发现，而非编译时就报错。 |
| **GAP-I1-2** | `graph_run.go:165-208` `resolveCompletedTasks()` | **GraphBranch 运行时路由完全缺失**。节点完成后不检查 `g.branches` 来决定数据路由。Chain 层通过在 `AppendBranch` 中将分支评估内联到单个 Lambda 节点来绕过此限制（`chain.go:133-190`），但 Workflow 层的 `AddBranch` 无法工作，因为底层 runner 不执行 branch condition。 | Workflow 的分支语义（`noDataFlow=true`、`endNodes` 白名单）完全无法在运行时执行。 |
| **GAP-I1-3** | `graph_run.go` / `graph_manager.go` | **`reportSkip` 调用路径缺失**。`dagChannel.reportSkip()` 和 `channelManager.reportSkip()` 已实现，但没有任何运行时代码调用它们。Branch 未选中的节点无法被标记为 skip。 | 多分支场景下未选中的分支节点会永久阻塞 runner 主循环（因为其 channel 永远等不到所有前驱完成）。 |
| **GAP-I1-4** | `graph_run.go:204-206` | **空操作代码**。`resolveCompletedTasks` 中存在一个无用的 `for branchTarget := range cc.writeTo { _ = branchTarget }` 循环，疑似为 branch 路由预留但未实现。 | 无功能影响，但暗示 branch 实现被推迟。 |

### 2.3 I1 应保护的测试

| 测试 | 文件 | 行号 | 保护内容 |
|------|------|------|---------|
| `TestCompileLockAddBranch` | `graph_test.go` | 883 | 编译后 AddBranch 报 `ErrGraphCompiled` |
| `TestGraphBranch` | `graph_test.go` | 911 | `GraphBranch.condition` 正确执行 |
| `TestGraphBranchTypeMismatch` | `graph_test.go` | 947 | branch condition 输入类型不匹配报错 |
| `TestMultipleBranches` | `graph_test.go` | 1818 | 多个分支节点注册 |
| `TestGraphNodeCallbackOnStartOnEndInvoke` | `graph_test.go` | 3407 | 节点 callback 在 Invoke 中正确触发 |
| `TestGraphNodeCallbackOnError` | `graph_test.go` | 3454 | 节点错误时 OnError 触发，OnEnd 不触发 |
| `TestGraphMultiNodeCallbackOrder` | `graph_test.go` | 3511 | 多节点 callback 执行顺序 |
| `TestSetNodeCallbacksUnknownNode` | `graph_test.go` | 3726 | 为不存在节点设置 callback 报错 |
| `TestSetNodeCallbacksAfterCompile` | `graph_test.go` | 3743 | 编译后设置 callback 报 `ErrGraphCompiled` |

### 2.4 I1 需新增的测试（当前缺失）

- `TestGraphBranchRuntimeRouting` — 验证 branch condition 在运行时被评估并正确路由
- `TestGraphBranchSkipPropagationForUnselectedNodes` — 未选中分支节点被 skip
- `TestGraphFieldMappingCompileTimeValidation` — 编译时 `validateFieldMapping` 拦截类型错误
- `TestGraphNoDataFlowBranch` — Workflow 分支的 `noDataFlow=true` 语义

---

## 3. I2 插入点：FieldMapping 层桥接

### 3.1 桥接插入点矩阵

| 编号 | 插入点位置 | 桥接方向 | 机制 | 当前状态 |
|------|-----------|---------|------|---------|
| **I2-A** | `field_mapping.go:30-37` `FieldMapping` 结构 | 数据定义 | fromNodeKey、from、to、customExtractor | **已实现** |
| **I2-B** | `field_mapping.go:40-75` 6 个构造器 | API 入口 | MapFields/FromField/ToField/MapFieldPaths/FromFieldPath/ToFieldPath | **已实现** |
| **I2-C** | `field_mapping.go:87-93` 访问器 | API 查询 | FromNodeKey/FromPath/ToPath/TargetPath/IsFromAll/IsToAll/HasCustomExtractor | **已实现** |
| **I2-D** | `field_mapping.go:259-360` `validateFieldMapping()` | FieldMapping → 类型系统 | 编译时校验字段存在性、导出性、类型赋值兼容性。返回 `handlerPair`（推迟检查）+ `uncheckedSourcePath` | **已实现，但未被 Graph 调用（GAP）** |
| **I2-E** | `field_mapping.go:364-446` `fieldMap()` | FieldMapping → 运行时 | 按 mapping 从输入提取字段，组包为 `map[string]any`。支持 `allowMapKeyNotFound` | **已实现，被 runner 调用** |
| **I2-F** | `field_mapping.go:448-454` `streamFieldMap()` | FieldMapping → Stream | **Stub** — `panic("not implemented")` | **Stub** |
| **I2-G** | `field_mapping.go:480-503` `takeOne()` | 单值提取原语 | 从 struct/map 中按字段名/key 提取值 | **已实现** |
| **I2-H** | `field_mapping.go:506-623` `assignOne()` | 单值写入原语 | 将值写入目标 struct/map 的字段路径 | **已实现** |
| **I2-I** | `field_mapping.go:626-635` `convertTo()` | map → Go 类型 | 将 `map[string]any` 中间结果转为目标类型 | **已实现** |
| **I2-J** | `field_mapping.go:96-98` `handlerPair` | 推迟检查三元组 | invoke checker 在请求时执行最终类型验证 | **已实现** |
| **I2-K** | `field_mapping.go:100-106` `assignableType` | 类型兼容性枚举 | Must / MustNot / May | **已实现** |
| **I2-L** | `field_mapping.go:108-126` 哨兵错误 | 错误协议 | `errMapKeyNotFound`、`errInterfaceNotValidForFieldMapping` | **已实现** |

### 3.2 I2 关键缺口

| 缺口编号 | 位置 | 描述 | 影响 |
|----------|------|------|------|
| **GAP-I2-1** | `field_mapping.go:259` | `validateFieldMapping` 定义但**未被 Graph.compile() 调用**。编译时类型检查完全缺失。 | 字段名拼写错误、类型不兼容等错误只能推迟到运行时才发现。 |
| **GAP-I2-2** | `field_mapping.go:448-454` | `streamFieldMap` 是完整 stub，调用即 panic。 | 流式 FieldMapping（Stream/Transform 模式下的字段提取）完全不可用。 |
| **GAP-I2-3** | `field_mapping.go:364` | `fieldMap` 返回的 `allowMapKeyNotFound` 参数在 runner 中始终为 `false`。没有路径将其设为 `true`。 | 无法支持"可选字段映射"语义。 |
| **GAP-I2-4** | 全局 | `FieldMapping.fromNodeKey` 由 Workflow 在 `addDependencyRelation` 中设置（`workflow.go:160`），但**无验证逻辑**确保它被设置后再使用。如果直接通过 Graph 的 `addEdgeWithMappings` 使用 FieldMapping，`fromNodeKey` 可能为空。 | 潜在的空 fromNodeKey 不会在编译时被检测。 |

### 3.3 I2 应保护的测试

| 测试 | 文件 | 行号 | 保护内容 |
|------|------|------|---------|
| `TestValidateFieldMappingSimplePass` | `field_mapping_test.go` | 174 | 简单字段映射通过编译时检查 |
| `TestValidateFieldMappingFieldNotFound` | `field_mapping_test.go` | 230 | 字段不存在时报错 |
| `TestValidateFieldMappingUnexportedField` | `field_mapping_test.go` | 253 | 未导出字段报错 |
| `TestValidateFieldMappingTypeNotAssignable` | `field_mapping_test.go` | 276 | 类型不兼容报错 |
| `TestValidateFieldMappingFromAllToAllConflict` | `field_mapping_test.go` | 299 | FromAll+ToAll 冲突报错 |
| `TestValidateFieldMappingInterfacePath` | `field_mapping_test.go` | 321 | interface 中间路径推迟检查 |
| `TestFieldMapStructExtraction` | `field_mapping_test.go` | 458 | struct 字段提取正确性 |
| `TestFieldMapMapExtraction` | `field_mapping_test.go` | 575 | map key 提取正确性 |
| `TestFieldMapKeyNotFoundError` | `field_mapping_test.go` | 616 | allow=false 时缺失 key 报错 |
| `TestFieldMapKeyNotFoundAllow` | `field_mapping_test.go` | 627 | allow=true 时缺失 key 跳过 |
| `TestFieldMapCustomExtractor` | `field_mapping_test.go` | 658 | 自定义提取函数 |
| `TestFieldMapNilMap` | `field_mapping_test.go` | 645 | nil 中间 map 报错 |
| `TestStreamFieldMapNotImplemented` | `field_mapping_test.go` | 928 | streamFieldMap panic |
| `TestConvertToStruct` | `field_mapping_test.go` | 750 | map→struct 转换 |
| `TestConvertToNestedStruct` | `field_mapping_test.go` | 809 | 嵌套路径 struct 转换 |

### 3.4 I2 需新增的测试

- `TestValidateFieldMappingWithGraphIntegration` — 通过 Graph API 添加带 FieldMapping 的边后，在 compile 时被正确校验
- `TestFieldMapWithUncheckedSourcePath` — 验证 interface 中间路径的推迟检查在运行时正确执行
- `TestFieldMapAllowMapKeyNotFoundTrue` — 验证 allow=true 时的跳过行为（当前只在 allow=false 时测试）

---

## 4. I3 插入点：Workflow 层桥接

### 4.1 桥接插入点矩阵

| 编号 | 插入点位置 | 桥接方向 | 机制 | 当前状态 |
|------|-----------|---------|------|---------|
| **I3-A** | `workflow.go:37-48` `NewWorkflow()` | Workflow → Graph | 创建内部 `graph`，强制 `AllPredecessor` 模式 | **已实现** |
| **I3-B** | `workflow.go:50-65` `initNode()` | Workflow → WorkflowNode | 创建 `WorkflowNode` 并初始化 `dependencySetter` 闭包 + `mappedFieldPath` trie | **已实现** |
| **I3-C** | `workflow.go:67-70` `AddLambdaNode()` | Workflow → Graph | 委托 `g.AddLambdaNode()` + `initNode()` | **已实现** |
| **I3-D** | `workflow.go:72-81` `AddGraphNode()` | Workflow → Graph(子图) | 创建包装子图 `graph` 的 `graphNode`，调用 `g.AddNode()` | **已实现** |
| **I3-E** | `workflow.go:83-89` `AddPassthroughNode()` | Workflow → Lambda | 创建 identity Lambda + `g.AddLambdaNode()` | **已实现** |
| **I3-F** | `workflow.go:91-109` `End()` | Workflow → END | 返回 END 的 WorkflowNode 供链式 AddInput | **已实现** |
| **I3-G** | `workflow.go:120-122` `AddInput()` | WorkflowNode → Graph | 调用 `addDependencyRelation`（默认模式：数据+执行依赖） | **已实现** |
| **I3-H** | `workflow.go:145-147` `AddInputWithOptions()` | WorkflowNode → Graph | 支持 `WithNoDirectDependency()` 选项 | **已实现** |
| **I3-I** | `workflow.go:149-151` `AddDependency()` | WorkflowNode → Graph(控制边) | 调用 `addDependencyRelation(dependencyWithoutInput=true)`，通过 `g.addEdgeWithMappings(isControl=true)` 建立纯控制边 | **已实现** |
| **I3-J** | `workflow.go:153-156` `SetStaticValue()` | WorkflowNode → 静态值 | 存储到 `staticValues map[string]any` | **已实现** |
| **I3-K** | `workflow.go:158-208` `addDependencyRelation()` | WorkflowNode → graph(核心) | 三种路径的差异处理：默认/NoDirectDependency/DependencyWithoutInput。将 `FieldMapping.fromNodeKey` 设置为 `fromNodeKey`。通过延迟闭包 (`addInputs`) 调用 `g.addEdgeWithMappings()`。 | **已实现** |
| **I3-L** | `workflow.go:211-248` `checkAndAddMappedPath()` | WorkflowNode → 冲突检测 | 使用嵌套 `map[string]any` trie 检测终端路径冲突 | **已实现** |
| **I3-M** | `workflow.go:254-318` `compile()` | Workflow → Graph.compile | **两阶段编译**：① 处理 branch → 注入 branchDependency + 调用 `g.addBranch(noDataFlow=true)`；② 执行所有 `addInputs` 延迟闭包；③ 注入 `staticValues` 到 `handlerPreNodes`；④ 委托 `g.compile()` | **已实现** |
| **I3-N** | `workflow.go:31-35` `Workflow[I,O]` 结构 | 状态管理 | 持有 `*graph`、`workflowNodes`、`workflowBranches`、`dependencies` | **已实现** |

### 4.2 I3 关键缺口

| 缺口编号 | 位置 | 描述 | 影响 |
|----------|------|------|------|
| **GAP-I3-1** | `workflow.go:254-268` compile 阶段一 | Workflow 的 branch 处理调用了 `g.addBranch(wb.fromNodeKey, wb.GraphBranch, true)`，但**底层 runner 不消费 branch 信息**（GAP-I1-2）。Workflow branch 在编译层面正确注册了，但运行时无法执行。 | Workflow 的分支依赖关系和 `noDataFlow=true` 语义完全丢失。 |
| **GAP-I3-2** | `workflow.go:72-81` `AddGraphNode()` | 参数类型硬编码为 `*Graph[any, any]`，不支持 `Chain` 或 `Workflow` 类型作为子图。虽然 Chain 实现了 `subGraph` 接口（`chain.go:8-11`），但 Workflow 的 `AddGraphNode` 不接受此接口。 | Chain 和 Workflow 之间无法互相嵌套。 |
| **GAP-I3-3** | `workflow.go:254-318` compile() | **不调用 `validateFieldMapping`**。与 GAP-I1-1 一致，Workflow 的 `AddInput` 中的 FieldMapping 没有经过编译时类型验证。 | 同 GAP-I2-1。 |
| **GAP-I3-4** | `workflow.go:31-35` | `dependencies` map 在编译时被填充（通过 `dependencySetter` 闭包），但**编译后未被任何代码消费**。它只作为声明阶段的记录存在。 | 依赖类型信息（normal/noDirect/branch）在编译后不可用，无法用于调试/自省。 |

### 4.3 I3 应保护的测试

| 测试 | 文件 | 行号 | 保护内容 |
|------|------|------|---------|
| `TestWorkflowBasicThreeNodes` | `workflow_test.go` | 9 | 基本三节点 Workflow 正确运行 |
| `TestWorkflowFanInFieldMapping` | `workflow_test.go` | 36 | 多前驱 FieldMapping 正确提取 |
| `TestWorkflowFanInPathConflict` | `workflow_test.go` | 78 | 路径冲突检测报错 |
| `TestWorkflowAddDependencyControlOnly` | `workflow_test.go` | 102 | AddDependency 仅控制依赖 |
| `TestWorkflowNoDirectDependency` | `workflow_test.go` | 130 | WithNoDirectDependency 仅数据依赖 |
| `TestWorkflowStaticValue` | `workflow_test.go` | 174 | SetStaticValue 正确注入 |
| `TestWorkflowStaticValuePathConflict` | `workflow_test.go` | 204 | 静态值与 AddInput 路径冲突 |
| `TestWorkflowPassthroughNode` | `workflow_test.go` | 224 | 透传节点正确转发 |
| `TestWorkflowFromFieldPath` | `workflow_test.go` | 250 | 嵌套 FieldPath 提取 |
| `TestWorkflowToFieldPath` | `workflow_test.go` | 276 | 嵌套 FieldPath 写入 |
| `TestWorkflowMapFieldPaths` | `workflow_test.go` | 294 | MapFieldPaths 跨层映射 |
| `TestWorkflowCustomExtractor` | `workflow_test.go` | 316 | 自定义提取器 |
| `TestWorkflowCompileLockAfterCompile` | `workflow_test.go` | 376 | 编译后锁定 |
| `TestWorkflowDependencyWithoutInputError` | `workflow_test.go` | 587 | AddDependency 与 AddInput 混合验证 |
| `TestWorkflowConcurrentInvokes` | `workflow_test.go` | 539 | 并发执行不互相影响 |

### 4.4 I3 需新增的测试

- `TestWorkflowBranchRouting` — Workflow 分支在运行时被正确评估和路由（当前被 GAP-I1-2 阻塞）
- `TestWorkflowBranchNoDataFlow` — 分支节点的 `noDataFlow=true` 语义：分支节点需要显式 AddInput 获取数据
- `TestWorkflowNestedChain` — Workflow 内嵌 Chain 作为子图
- `TestWorkflowDependenciesIntactAfterCompile` — 验证 `dependencies` 表在编译后完整

---

## 5. Chain 层桥接（辅助分析，不在 I1/I2/I3 主分配中）

### 5.1 Chain 的 Branch 绕过策略

Chain 的 `AppendBranch`（`chain.go:133-190`）采用了与 Graph Branch 运行时路由完全不同的策略：它将分支条件评估和分支执行**内联到单个 Lambda 节点**中。分支路由节点内部：
1. 调用 `condition`/`multiCondition` 获取目标分支 key
2. 直接 `l.GetRunnable().invoke(ctx, input)` 执行选中分支的 Lambda
3. 对于 multi-branch，返回 `map[string]any` 汇聚结果

这一策略**完全绕过了** Graph 层的 branch 运行时路由需求。Chain 的 branch 功能独立工作，不依赖 GAP-I1-2 的修复。

### 5.2 Chain 的 `subGraph` 接口

Chain 通过 `subGraph` 接口（`chain.go:8-11`）支持子图嵌套：
```go
type subGraph interface {
    innerGraph() *graph
    finalizeSubGraph(ctx context.Context) error
}
```
`Chain` 实现了此接口（`chain.go:192-198`），因此 Chain 可以嵌套 Chain。但 Workflow 没有实现 `subGraph`，导致 Workflow 不能通过此接口被 Chain 嵌套。

---

## 6. 当前约束总表

### 6.1 架构级约束

| 约束编号 | 描述 | 影响的桥接点 | 严重度 |
|----------|------|-------------|--------|
| **C-1** | `validateFieldMapping` 未被 Graph.compile() 调用 | I1-D, I2-D, I3-K | **高** — 类型错误在运行时才发现 |
| **C-2** | GraphBranch 运行时路由缺失 | I1-M, GAP-I1-2, I3-M | **高** — Workflow branch 完全不可用 |
| **C-3** | `reportSkip` 调用路径缺失 | I1-H, GAP-I1-3 | **高** — 多分支未选中节点永久阻塞 |
| **C-4** | `streamFieldMap` 是 stub | I2-F | **中** — 流式字段映射不可用（预期行为） |
| **C-5** | Workflow.AddGraphNode 类型硬编码 | I3-D, GAP-I3-2 | **中** — Workflow 不能嵌套 Chain |
| **C-6** | `composableRunnable` 不携带 `reflect.Type` | I1-F | **中** — 编译时类型验证缺少类型信息 |
| **C-7** | `dependencies` 表编译后不消费 | I3-N, GAP-I3-4 | **低** — 影响调试/自省 |
| **C-8** | Chain Branch 内联绕过 Graph branch | chain.go:133 | **低** — 功能正常工作，但架构不统一 |

### 6.2 线程安全约束

| 约束 | 位置 | 状态 |
|------|------|------|
| `graph.compiled` 检查无锁 | `graph.go:47-52` | **安全** — 编译是单线程操作，编译后所有 mutation 立即失败 |
| `channelManager.channels` 无锁 | `graph_manager.go:28-30` | **安全** — 仅在 runner 主循环的单个 goroutine 中访问 |
| `dagChannel` 无锁 | `dag.go:24-31` | **安全** — 仅在 runner 的 `resolveCompletedTasks` 中顺序访问 |
| `taskManager` 有 `sync.Mutex` | `graph_manager.go:102` | **安全** — `doneTasks`/`runningTasks` 有锁保护 |
| `EventLog` 有 `sync.Mutex` | `event_log.go:36` | **安全** |
| `concatFns` 使用 `sync.Map` | `stream.go:153` | **安全** |
| Workflow 的 `mappedFieldPath` 无锁 | `workflow.go:22` | **安全** — 声明阶段单线程使用 |
| Workflow 的 `addInputs` 无锁 | `workflow.go:19` | **安全** — compile 阶段单线程执行 |

### 6.3 数据类型约束

| 约束 | 说明 |
|------|------|
| `fmtType()` 仅识别 6 种类型 | `generic_graph.go:150-163` — string/int/float64/bool/any/nil。不支持自定义 struct/map 的类型字符串。 |
| `validateFieldMapping` 需要 `reflect.Type` | 当前只能通过测试中的 `reflect.TypeOf()` 传入，Graph 层无类型信息传递机制。 |
| `FieldPath` 分隔符固定为 `\x1F` | `field_mapping.go:10` — 与 Eino 一致，不可修改。 |
| `Workflow` 固定使用 `AllPredecessor` | `workflow.go:38` — 不支持 Pregel 模式。 |

---

## 7. 修复优先级建议

### Phase 1（必须最先修复）
1. **GAP-I1-1 / C-1**：在 `graph.compile()` 中添加 `validateFieldMapping` 调用，连接 I1-D 与 I2-D
2. **GAP-I2-1**：确保 `graphNode` 携带 `inputType/outputType reflect.Type`，为 `validateFieldMapping` 提供类型信息

### Phase 2（依赖 Phase 1）
3. **GAP-I1-2 / C-2**：在 `resolveCompletedTasks` 中集成 GraphBranch 评估和运行时路由
4. **GAP-I1-3 / C-3**：实现 branch 未选中节点的 `reportSkip` 调用链

### Phase 3（可选增强）
5. **GAP-I3-2 / C-5**：扩展 `Workflow.AddGraphNode` 支持 `subGraph` 接口
6. **GAP-I2-2 / C-4**：实现 `streamFieldMap`（需要 Stream 模式的完整支持）

---

## 8. 文件归属总结

### 8.1 I1 负责文件

| 文件 | 操作 | 桥接角色 |
|------|------|---------|
| `graph.go` | 修改 | addEdgeWithMappings、fieldMappingRecords、handlerPreNodes、compile 类型验证 pass、branch 运行时路由 |
| `graph_run.go` | 修改 | resolveCompletedTasks 集成 branch 评估 + reportSkip |
| `graph_node.go` | 修改 | 可选：添加 inputType/outputType reflect.Type 字段 |
| `graph_manager.go` | 不修改 | channel 接口已足够 |
| `branch.go` | 不修改 | GraphBranch 结构已完整 |
| `dag.go` | 不修改 | reportSkip 已就绪 |

### 8.2 I2 负责文件

| 文件 | 操作 | 桥接角色 |
|------|------|---------|
| `field_mapping.go` | 不修改 | 所有 FieldMapping 逻辑已完整实现 |
| `graph.go` | 配合 I1 | compile() 中集成 validateFieldMapping 调用的接口 |

### 8.3 I3 负责文件

| 文件 | 操作 | 桥接角色 |
|------|------|---------|
| `workflow.go` | 可不修改 | 编译逻辑已完整；branch 执行依赖 I1 修复 |

---

*审计完成时间：2026-06-04*
*审计范围：compose/ 下 28 个源文件（15 源 + 7 测试 + 6 辅助）+ research/ 下 4 个研究文档*
*关键发现：六层抽象均已实现至 90%+ 完成度。三个关键桥接缺口为 (1) validateFieldMapping 未被 compile 调用，(2) GraphBranch 运行时路由缺失，(3) reportSkip 调用链缺失。Workflow 的分支语义因 (2) 和 (3) 而无法工作，但 Chain 通过内联分支评估绕过了这些限制。*
