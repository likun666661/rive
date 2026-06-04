# Eino Compose Runtime Replica — 最终验证摘要 (第二章 + 第三章骨架,教学子集,非完整产品复刻)

## 验证状态

- **gofmt**: 通过 (所有 Go 文件均已格式化)
- **go test ./...**: 通过 (compose 包全部测试 PASS,cmd/example 无测试文件)
- **go run ./cmd/example**: 通过 (15 个示例全部正常运行)

---

## 实现的 Eino 核心机制

### 一、第一章: Graph 核心运行时

#### 1. Graph 泛型组合 (Graph[I, O])
- `generic_graph.go`: 泛型 `Graph[I, O]` 包装内部 `graph`,支持 string / int / bool / struct 等多种输入输出类型。
- `NewGraph[I, O]()` 创建图,`Compile(ctx, opts...)` 生成 `Runnable[I, O]`。

#### 2. 编译锁 (Compile Boundary)
- `Compile()` 后将 `graph.compiled` 置为 true,阻止后续 `AddLambdaNode` / `AddEdge` / `AddControlEdge` / `AddBranch` 操作,返回 `ErrGraphCompiled`。
- 同一 graph 可用不同选项多次编译 (DAG ↔ Pregel 切换),重新生成新的 runner。

#### 3. 触发模式 (NodeTriggerMode)
- `AllPredecessor`: 等所有数据+控制前驱就绪后触发 → DAG 模式。
- `AnyPredecessor`: 任一数据前驱上报即触发 → Pregel 模式。
- 默认: `AnyPredecessor` (Pregel)。

#### 4. 数据边 + 控制边
- `AddEdge(from, to)`: 数据边,传递数据值。
- `AddControlEdge(from, to)`: 控制边,仅传递依赖信号 (不传数据)。

#### 5. Channel 抽象 (DAG Channel / Pregel Channel)
- `dag.go`: `dagChannel` 对控制前驱 (dependencyState 状态机) 和数据前驱进行计数。
- `pregel.go`: `pregelChannel` 任一前驱上报即返回。
- 共同实现 `channel` 接口,支持自定义合并函数。

#### 6. 执行引擎 (runner)
- `graph_run.go`: 主循环按 step 逐轮调度,并发 golang goroutine 执行。

#### 7. 环检测 (Kahn 算法)
- DAG 模式编译时使用 Kahn 拓扑排序检测环,返回 `ErrDAGHasCycle`。

#### 8. maxSteps 安全上限
- 默认 `defaultMaxSteps = 100`,Pregel 模式下超步返回 `ErrExceedMaxSteps`。

#### 9. 分支 (GraphBranch)
- `branch.go`: `NewGraphBranch[I](condition, branchMap)` 创建条件路由分支。

#### 10. EventLog 事件系统
- 10 种事件类型,线程安全 (`sync.Mutex`),支持并发写入。

#### 11. GraphInfo 内省
- 编译后导出完整拓扑信息: 节点列表、边列表、触发模式等。

#### 12. Lambda 可组合函数
- `InvokableLambda[I, O](fn)` 泛型构造函数,内部自动类型断言。

---

### 二、第二章: 编排抽象

#### 13. FieldMapping 字段映射 (field_mapping.go)
- **六个构造函数**: `MapFields`、`FromField`、`ToField`、`MapFieldPaths`、`FromFieldPath`、`ToFieldPath`
- **自定义提取器**: `WithCustomExtractor` 支持任意数据源
- **路径分隔符**: `\x1F` (Unit Separator),与 Eino 源码一致
- **编译时校验**: `validateFieldMapping` 检查字段存在性、导出性、类型赋值兼容性
- **请求时执行**: `fieldMap` 按 mapping 规则提取字段,输出 `map[string]any`
- **类型转换**: `convertTo` 将 `map[string]any` 转换为目标 Go 类型
- **字段提取原语**: `takeOne` (struct field / map key 单值提取)、`assignOne` (单值写入)

#### 14. Workflow 声明式编排 (workflow.go)
- `Workflow[I,O]`: 泛型声明式图构建器,内部使用 AllPredecessor 模式
- `WorkflowNode.AddInput(fromNodeKey, mappings...)`: 一次声明数据来源与字段映射
- `WorkflowNode.AddDependency(fromNodeKey)`: 纯执行依赖 (无数据传递)
- `WorkflowNode.SetStaticValue(path, value)`: 编译时注入常量
- `WorkflowNode.AddInputWithOptions(opts...)`: 支持 `WithNoDirectDependency()`
- `Workflow.AddBranch(fromNodeKey, branch)`: Workflow 分支 (noDataFlow=true)
- `WorkflowCompile`: 两阶段编译 (分支收集 → addInputs 闭包 → 静态值注入 → graph.compile)
- **三态依赖**: `normalDependency` / `noDirectDependency` / `branchDependency`
- **路径冲突检测**: `checkAndAddMappedPath` 使用 trie 防止路径冲突

#### 15. Chain Builder 线性管道 (chain.go)
- `Chain[I,O]`: Builder 风格图构建,内部包装 `Graph[I,O]`
- `AppendLambda / AppendGraph / AppendPassthrough`: 追加节点
- `AppendParallel`: 嵌入 Parallel 并行组 (自动生成 merge 节点)
- `AppendBranch`: 嵌入 ChainBranch 条件分支
- **自动命名**: `nextNodeKey()` 生成 `node_0`, `node_1`, ...
- **自动追踪**: `preNodeKeys` 追踪尾部节点集
- **自动 END 连接**: `addEndIfNeeded` 编译时自动连接 END

#### 16. Parallel 并行节点组 (chain_parallel.go)
- `Parallel`: 并行节点集合,所有节点共享同一前驱输入
- `AddLambda / AddGraph / AddPassthrough`: 注册并行节点
- `outputKey`: 标注每个并行节点的输出来源
- **outputKey 冲突检测**: `outputKeys` map 确保 key 唯一

#### 17. ChainBranch 条件分支 (chain_branch.go)
- `ChainBranch`: 封装 GraphBranch + 分支节点映射表
- `NewChainBranch[T]`: 单路径分支 (条件函数返回单个 key)
- `NewChainMultiBranch[T]`: 多路径分支 (条件函数返回 key 集合)
- `AddLambda / AddGraph / AddPassthrough`: 注册分支节点

---

### 三、第三章: Runnable Stream / Collect / Transform / Callback 教学示例

> **本章实现 Runnable 四模式、基础 Pipe stream、Collect/Transform 降级和 CallbackWrapper 教学路径。组件桥接、图级流式执行、stream field mapping 和流式分支不在当前范围内。**

#### 18. composableRunnable 四字段设计 (runnable.go)
- `i`: invoke 执行函数体
- `s`: stream 执行函数体
- `c`: collect 执行函数体
- `t`: transform 执行函数体
- **四模式降级矩阵**: invoke/stream/collect/transform 都能在缺原生函数时按规则 fallback

#### 19. Pipe stream 教学版实现 (stream.go)
- `PipeStreamReader[T]` / `PipeStreamWriter[T]`: 模拟 Eino 的流式读写抽象
- `NewPipe` / `PipeStreamReaderFromSlice` / `PipeStreamReaderFromValue`: 常用构造路径
- `Copy`: 教学版流扇出
- `Merge` / `Concat`: 教学版流扇入和折叠

#### 20. Stream Collect 收集模式
- 流式分块按序收集为完整结果
- `StreamReader → Recv(token_i) → Concat → 完整结果`
- Eino 完整版支持多种 merge 策略 (append/concat/mergeMap)

#### 21. Stream Transform 变换模式
- 流式处理管道: `生产 → Transform(fn) → Collect`
- 三种变换: 逐 chunk 变换 / 带状态变换 / 批量变换
- Eino 中由 compose.Transform 实现

#### 22. CallbackWrapper 回调计时 (callbacks.go)
- 回调生命周期: `OnStart → Execute → OnEnd/OnError`
- 支持流输入/流输出回调副本: `OnStartWithStreamInput` / `OnEndWithStreamOutput`
- HandlerBuilder 可根据注册 handler 计算需要的 timing
- EventLog 在 graph 级别提供等效可观测性

---

## 关键文件导览

| 文件 | 职责 |
|---|---|
| `types.go` | 类型常量 (NodeTriggerMode, ComponentType, runType)、START/END 哨兵、sentinel errors |
| `runnable.go` | Runnable[I,O] 接口、composableRunnable、Lambda、InvokableLambda 泛型构造函数 |
| `graph.go` | graph 内部结构、AddNode/Edge/ControlEdge/Branch、addEdgeWithMappings、compile() 主流程、Kahn 环检测 |
| `generic_graph.go` | Graph[I,O] 公开 API、NewGraph、Compile、GetGraphInfo、graphRunnable |
| `graph_node.go` | graphNode、compileIfNeeded (子图递归) |
| `graph_compile.go` | graphCompileOptions、CompileOption 函数选项 (WithGraphName 等) |
| `graph_run.go` | runner 结构、主执行循环 run()、任务创建与结果分发、FieldMapping 集成 |
| `graph_manager.go` | channel 接口、channelManager、taskManager (goroutine 并发池) |
| `dag.go` | dagChannel: AllPredecessor 语义,带控制前驱状态机 |
| `pregel.go` | pregelChannel: AnyPredecessor 语义,简单取值即消费 |
| `branch.go` | GraphBranch、NewGraphBranch 泛型条件分支 |
| `field_mapping.go` | FieldMapping / FieldPath / validateFieldMapping / fieldMap / takeOne / assignOne / convertTo |
| `workflow.go` | Workflow[I,O] / WorkflowNode / WorkflowBranch / AddInput / AddDependency / SetStaticValue / compile |
| `chain.go` | Chain[I,O] Builder / AppendLambda / AppendParallel / AppendBranch / addNode / preNodeKeys |
| `chain_parallel.go` | Parallel / AddLambda / outputKey 冲突检测 |
| `chain_branch.go` | ChainBranch / NewChainBranch / NewChainMultiBranch / AddLambda |
| `introspect.go` | GraphInfo、GraphNodeInfo、GraphEdgeInfo (编译时拓扑导出) |
| `event_log.go` | EventLog、10 种事件类型、线程安全记录与格式化 |
| `utils.go` | 辅助工具函数 |
| `stream.go` | PipeStreamReader/PipeStreamWriter、Copy、Merge、Concat |
| `callbacks.go` | RunInfo、Handler、HandlerBuilder、CallbackWrapper、流输入/输出副本 |
| `cmd/example/main.go` | 综合示例程序 (15 个场景,覆盖 Graph/DAG/Pregel/FieldMapping/Workflow/Chain/Parallel/Branch/Stream/Collect/Transform/Callback) |

---

## 如何运行

### 运行测试
```bash
cd examples/eino-compose-runtime-replica-go
go test ./...
```

### 格式化代码
```bash
cd examples/eino-compose-runtime-replica-go
gofmt -w .
```

### 运行示例
```bash
cd examples/eino-compose-runtime-replica-go
go run ./cmd/example/
```

---

## 明确未实现的边界

**本复刻版是教育子集 (educational subset)。组件桥接 (ChatModel/Tool/Retriever)、完整图流式执行、stream field mapping 和流式分支不在当前范围内。**

本 MVP 复刻版聚焦于 Eino Compose Runtime 的核心图编译与执行引擎,以下为明确未实现的部分:

### 运行时不支持
- **组件桥接 (ChatModel/Tool/Retriever)**: 当前仅有 Lambda 抽象,可通过 AddLambdaNode 等价替代
- **图级 Stream 执行管线**: Runnable 四模式已经实现,但 graph runner 主路径仍以 Invoke 为主
- **streamFieldMap 流式映射**: 依赖图级 stream channel,当前未接入
- **Stream ChainBranch**: 流式分支暂未接入 Chain Builder
- **组件级 Callback 桥接**: CallbackWrapper 已实现,但未接 ChatModel/Tool 组件体系与图级初始化链
- **State 传递 (graph.state)**: 字段已定义但未使用
- **Checkpoint / Recovery**: 可恢复执行机制不在范围内
- **Fan-in 智能合并 (Merge 配置)**: 当前默认 map[string]any 合并

### 周边工具未实现
- **可视化 / DOT 导出**: 无 graph 拓扑可视化
- **JSON Schema 校验**: 无编译时 node 输入输出类型的 schema 校验
- **DevOps 工具**: 无 tracing / metrics / profiling 集成

### 类型系统局限
- `fmtType()` 仅覆盖 `string/int/float64/bool` 四种基础类型,其余返回 `"any"`

---

## 验证结论

Go Eino Compose Runtime Replica 成功实现了 Eino 的核心设计理念:

1. **编译边界分离**: 图构建 (可变) 与运行时执行 (不可变) 清晰分离
2. **双模式执行引擎**: DAG (AllPredecessor) 与 Pregel (AnyPredecessor) 通过 Channel 多态实现差异化调度
3. **三层编排抽象**: Graph → Workflow → Chain,控制力递减,便利性递增
4. **FieldMapping 基础设施**: 六个构造函数 + 自定义提取器,编译时校验 + 请求时执行
5. **声明式数据流**: Workflow 的 AddInput/AddDependency/SetStaticValue 替代手动 AddEdge
6. **Builder 风格管道**: Chain 的 Append* 系列,自动节点命名与拓扑连接
7. **内建并行与分支**: Parallel 并行节点组 + ChainBranch 条件路由
8. **Runnable 四模式**: composableRunnable 支持 Invoke/Stream/Collect/Transform 降级矩阵
9. **Stream 教学模式**: Pipe stream、Copy、Merge、Concat、Collect/Transform 概念演示
10. **CallbackWrapper**: OnStart/OnEnd/OnError 与流输入/输出回调副本
11. **零外部依赖**: 仅依赖 Go 标准库
12. **充分测试覆盖**: compose 包全量测试通过,demo 程序 15 个场景覆盖全部功能
