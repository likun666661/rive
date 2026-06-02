# Eino Compose 运行时研究：问题、难点、思路与落地

## 一、问题是什么

Eino Compose 要解决的核心问题是 **LLM 应用中"可组合执行图"的编译与运行分离**。

### 1.1 原始问题

开发者需要将 prompt、ChatModel、tool、retriever、indexer、lambda、子图等不同形态的组件，组合成一个可运行单元。这个单元需要同时支持四种执行形态（`Invoke` / `Stream` / `Collect` / `Transform`）。

如果用普通函数调用串联所有组件，会同时出现以下四个问题：

1. **执行顺序散落在业务代码中**：每个开发者手写"先调 A，再调 B，有分支走 C"的控制流，无法统一检查依赖关系、类型兼容性和拓扑正确性。
2. **组件能力不一致**：ChatModel 可能支持 Invoke + Stream，Retriever 可能只支持 Invoke，Lambda 可能只支持 Transform。上层统一调用时需要自动发现组件能力并做降级/转接。
3. **多节点应用需要复杂的拓扑表达**：包括字段映射（前一个节点的输出字段如何映射到后一个节点的输入字段）、并行执行、条件分支、嵌套子图，而不仅仅是线性 pipeline。
4. **运行时需要统一的基础设施**：callback 注入、checkpoint 持久化、interrupt/resume（中断与恢复）、状态共享，这些横切关注点不可能让每个组件自己实现。

### 1.2 核心设计决策

Eino 最核心的设计决策是：**将拓扑构建（Graph construction）与运行时执行（Runnable execution）彻底分离**。由此派生出一个编译边界（Compile Boundary）：

- 构建阶段：用户在 `Graph/Chain/Workflow` 上自由添加节点、边、分支、字段映射、回调钩子。
- 编译阶段：`Graph.Compile()` 进行类型推断、拓扑校验、子图递归编译、channel/runtime 构造。
- 运行时阶段：编译产出的 `Runnable[I, O]` 只暴露四种执行方法，不暴露拓扑信息。

---

## 二、为什么这么难

### 2.1 类型适配的复杂性

LLM 应用的组件不是普通 DAG 中的纯函数。节点之间的类型必须兼容，但开发者经常添加 passthrough（透传）节点，其输入输出类型需要在编译期通过拓扑上下文推断。

```
nodeA (output: string) → passthrough → nodeB (input: string)
```

如果 passthrough 没有显式声明类型，编译期必须沿着图拓扑链式推断出它是 `passthrough[string]`。

**源码实现**：`graph.go:65-68` 的 `toValidateMap` 追踪所有待推断的节点，`updateToValidateMap`（`graph.go:561`）通过 BFS 方式沿边链式解析。具体机制如下：

- 每次添加数据边时调用 `addToValidateMap`（`graph.go:552-557`）记录待验证的 (startNode, endNode, mappings) 三元组。
- `updateToValidateMap` 在循环中反复检查 `toValidateMap`：当某条边的 startNode 和 endNode 中有任一方的类型是确定的，就把类型"传染"给另一方（passthrough 节点）。
- 如果起点类型和终点类型都已知，则调用 `checkAssignable` 做类型兼容性检查，若需要运行时类型转换则注册 `edgeHandlerManager`（`graph.go:597-601`）。

详情见：`graph.go:561-637`

### 2.2 执行触发模式的差异

不同场景需要不同的触发策略：

| 模式 | 语义 | 适用场景 | 对应运行时 |
|------|------|---------|-----------|
| `AnyPredecessor` | 任一前驱完成即触发 | agentic 循环图，一个节点可能被多个前驱的任意一个触发 | Pregel |
| `AllPredecessor` | 所有前驱完成才触发 | 确定性 DAG，节点需要收集所有前驱的输出后才执行 | DAG |

同一个图可能既需要 AllPredecessor 语义的子图，又需要 AnyPredecessor 区域。编译期必须正确推断并配置每个节点的 channel 行为。

**源码实现**：`graph.go:680-690` 根据 Graph 类型和编译选项决定运行模式：
- 默认 Graph（`ComponentOfGraph`）→ `runTypePregel`，使用 `pregelChannelBuilder`
- Chain / Workflow 或显式指定 `AllPredecessor` → `runTypeDAG`，使用 `dagChannelBuilder`

### 2.3 两种 Channel 的对比

| 维度 | DAG Channel (`dag.go`) | Pregel Channel (`pregel.go`) |
|------|------------------------|------------------------------|
| 触发条件 | 所有 control 前置都 Ready + 所有 data 前置都上报 | 任一 data 前置上报值 |
| 依赖跟踪 | 维护 `ControlPredecessors`（Waiting/Ready/Skipped）和 `DataPredecessors`（bool） | 只维护 `Values` map |
| Skip 传播 | 支持：若所有 control 前置被 skip，节点自身也被 skip | 不支持（`reportSkip` 返回 false） |
| Fan-in merge | 收集所有 data 前置的值后统一 merge | 每次 `get()` 后清空 Values |
| 循环支持 | 不支持（编译期 Kahn 算法拒绝环，`graph.go:856-862`） | 支持，但有 `maxSteps` 限制防死循环 |
| 核心判断 | `dag.go:128-191` — `get()` 方法检查 Waiting/Ready/Skipped 状态机器 | `pregel.go:55-88` — `get()` 方法简单判断 Values 非空 |

### 2.4 多执行形态的交叉组合（4×4 自动转换）

四个执行形态（Invoke / Stream / Collect / Transform）互相之间存在复杂的降级和转换关系。如果某组件只实现了 `Invoke`，但上层调用 `Stream`，运行时需把 Invoke 的结果包装成 stream reader 返回。

**源码实现**：`runnable.go:194-334` 实现了 `runnablePacker` 的全部 4×4 组合自动转换：

- `invokeByStream`：调用 Stream，然后 concat stream reader 得到单个结果
- `invokeByCollect`：将输入包装为 stream reader，调用 Collect
- `invokeByTransform`：输入包装为 stream → Transform → concat 输出
- `streamByInvoke`：调用 Invoke，将结果包装为 `StreamReaderFromArray`
- `streamByTransform`：输入包装为 stream → Transform
- `streamByCollect`：输入包装为 stream → Collect → 结果包装为 stream
- `collectByTransform`：Transform → concat 输出 stream
- `collectByInvoke`：concat 输入 stream → Invoke
- `collectByStream`：concat 输入 stream → Stream → concat 输出 stream
- `transformByStream`：concat 输入 stream → Stream
- `transformByCollect`：Collect → 结果包装为 stream
- `transformByInvoke`：concat 输入 stream → Invoke → 结果包装为 stream

### 2.5 嵌套图的地址空间

子图的 callback、checkpoint、interrupt 都需要挂接到父图的地址空间。Eino 使用 `AddressSegment` 路径段链式拼接构建全局唯一的节点路径：

```
parent_graph/node_key/sub_graph/sub_node
```

**源码实现**：
- 通过 `AppendAddressSegment(ctx, AddressSegmentRunnable, graphName)` 在每层包装 runnable 时追加路径段（`generic_graph.go:149`）
- 创建 task 时通过 `AppendAddressSegment(ctx, AddressSegmentNode, nodeKey)` 追加节点段（`graph_run.go:748`）
- 中断发生时，`interruptError` 从最深层子图向上冒泡，直到被父图的 `runner` 捕获并转换为该父图的 checkpoint（`graph_run.go:288-301`）

### 2.6 状态、回调和检查点的正确传播

共享状态（local state）通过 `context.Context` 传递，子图可访问父图的状态。当子图被中断并恢复时，状态必须完整且一致。

**源码实现**：`graph.go:842-853` 的 `runCtx` 闭包：
```go
func(ctx context.Context) context.Context {
    var parent *internalState
    if p, ok := ctx.Value(stateKey{}).(*internalState); ok {
        parent = p
    }
    return context.WithValue(ctx, stateKey{}, &internalState{
        state:  g.stateGenerator(ctx),
        parent: parent,  // 链式查找父图状态
    })
}
```

---

## 三、通用设计思路

### 3.1 三层编排抽象

Eino 提供三层编排抽象，三者最终都编译成统一的 `Runnable[I, O]` 接口：

| 抽象层 | 文件 | 特点 | 运行时 |
|--------|------|------|--------|
| `Graph[I, O]` | `generic_graph.go:93` | 显式有向图，手动添加节点和边，支持循环 | Pregel（默认） |
| `Chain[I, O]` | `chain.go` | Fluent Builder 风格，顺序连接 | DAG |
| `Workflow[I, O]` | `workflow.go` | 声明式依赖 + 字段映射包装 | DAG |

其中 `Graph` 是 `*graph` 的泛型包装（`generic_graph.go:93-95`），通过嵌入的方式获得所有图操作方法。

### 3.2 编译边界（Compile Boundary）

这是 Eino 架构最核心的设计模式。编译边界带来的关键收益：

1. **安全性**：`graph.compiled` 标志位（`graph.go:84`）阻止编译后的拓扑修改。所有修改操作开头检查 `ErrGraphCompiled`（`graph.go:162-168`）。
2. **可观测性**：编译完成的回调（`GraphCompileCallback`，`introspect.go:54-57`）让外部系统获取完整的 `GraphInfo`，用于可视化、监控和审计。
3. **可复用性**：同一个 Graph 可多次编译（每次传入不同的 `GraphCompileOption`），产生多个独立的 Runnable 实例。

### 3.3 编译流程详解

```
Graph.Compile(ctx, opts) [generic_graph.go:123]
  └── compileAnyGraph[I,O](ctx, g, opts...) [generic_graph.go:127]
        ├── g.compile(ctx, option) [graph.go:674]
        │     ├── 确定 runType: Pregel 还是 DAG [graph.go:680-690]
        │     ├── 确定 eager 模式 [graph.go:693-699]
        │     ├── 校验 toValidateMap 为空 [graph.go:708-713]
        │     ├── 校验 fieldMapping 无冲突 [graph.go:715-727]
        │     ├── 注册 beforeChildGraphsCompile 回调 [graph.go:729-732]
        │     ├── 对每个节点调用 compileIfNeeded [graph.go:734-737]
        │     │     ├── 如果是子图：递归 g.compile(ctx, compileOption)
        │     │     └── 如果是 runnable：直接返回
        │     ├── 构建 chanSubscribeTo / predecessors / runner [graph.go:739-834]
        │     ├── DAG 无环校验 (Kahn 算法) [graph.go:856-862]
        │     ├── 创建 checkPointer [graph.go:864-878]
        │     └── r.toComposableRunnable() [graph.go:891]
        └── toGenericRunnable[I,O] → runnablePacker → Runnable[I,O]
```

### 3.4 运行时执行流程

```
Runnable.Invoke(ctx, input)
  └── cr.i(ctx, input) → runner.invoke(ctx, input)
        └── runner.run(ctx, isStream=false, input) [graph_run.go:109]
              ├── 初始化 channelManager (每个节点创建 dag/pregel channel)
              ├── 初始化 taskManager (goroutine pool)
              ├── 检查点恢复 或 从 START 初始化
              └── 主循环:
                    ├── tm.submit(nextTasks)
                    │     ├── 同步执行 pre-handler
                    │     └── 并发 goroutine 执行 task
                    ├── tm.wait()
                    │     └── runPostHandler on success
                    ├── calculateNextTasks(completedTasks)
                    │     ├── resolveCompletedTasks
                    │     │     ├── writeChannelValues (路由输出到目标 channel)
                    │     │     └── calculateBranch (条件分支决策)
                    │     ├── cm.updateAndGet (上报值/依赖, 获取就绪 channel)
                    │     └── createTasks (创建新 task)
                    └── 检查 isEnd → 返回 result
```

### 3.5 Channel 抽象 — 触发器多态

Eino 在同一个运行时框架中统一 `AnyPredecessor` 和 `AllPredecessor`，仅通过 channel 实现的多态来区分：

**channel 接口**（`graph_manager.go:29-38`）定义了 5 个方法：
```go
type channel interface {
    reportValues(map[string]any) error
    reportDependencies([]string)
    reportSkip([]string) bool
    get(bool, string, *edgeHandlerManager) (any, bool, error)
    convertValues(fn func(map[string]any) error) error
    load(channel) error
    setMergeConfig(FanInMergeConfig)
}
```

- `dagChannel`（`dag.go:50`）实现 AllPredecessor 语义：维护 ControlPredecessors 的状态机（Waiting→Ready→Skipped）和 DataPredecessors 的布尔标记。
- `pregelChannel`（`pregel.go:25`）实现 AnyPredecessor 语义：仅维护 Values map，任何前置上报值就 ready；`reportDependencies` 和 `reportSkip` 为空操作。

### 3.6 Task 管理与并发控制

**taskManager**（`graph_manager.go:269-283`）是运行时并发的核心控制器：

- **submit**（`graph_manager.go:300-351`）：同步执行 pre-handler，然后异步启动 goroutine 执行任务。智能优化：当只有一个任务且 `needAll` 模式时，在 submit 的 goroutine 内部同步执行，避免不必要的 goroutine 切换。
- **wait**（`graph_manager.go:353-378`）：等待任务完成。`needAll` 模式（DAG）下调用 `waitAll` 收集所有完成的任务；`!needAll`（eager）模式下调用 `waitOne` 逐个收集。
- **cancel 机制**（`graph_manager.go:434-525`）：`receiveWithListening` 支持通过 cancel channel 中断执行，包含超时控制（`receiveWithDeadline`）。

### 3.7 核心数据结构

| 数据结构 | 文件:行号 | 用途 |
|---------|----------|------|
| `graph` | `graph.go:57-89` | 图的核心结构：nodes、edges、branches、toValidateMap、编译锁 |
| `runner` | `graph_run.go:43-84` | 编译后的运行时引擎：channels、predecessors、checkPointer、中断配置 |
| `chanCall` | `graph_run.go:31-39` | 节点的执行包装：action、writeTo、controls、pre/post processor |
| `task` | `graph_manager.go:257-267` | 单个执行任务：context、nodeKey、call、input/output、error |
| `channelManager` | `graph_manager.go:115-125` | 管理所有 channels，提供 updateAndGet 统一操作 |
| `taskManager` | `graph_manager.go:269-283` | 并发 goroutine pool：submit、wait、cancel |
| `dagChannel` | `dag.go:50-60` | AllPredecessor 语义的 channel |
| `pregelChannel` | `pregel.go:25-29` | AnyPredecessor 语义的 channel |
| `GraphInfo` | `introspect.go:41-52` | 编译完成后的自省信息（节点、边、分支等） |
| `runnablePacker` | `runnable.go:72-77` | 4×4 形态自动转换的泛型包装 |

### 3.8 Local State 机制

Local state 是图编译时注册的、按运行实例共享的状态对象。

声明方式（`generic_graph.go:37-44`）：
```go
type MyState struct { Collected []string }
graph := compose.NewGraph[string, string](
    compose.WithGenLocalState(func(ctx context.Context) *MyState {
        return &MyState{Collected: make([]string)}
    }),
)
```

节点通过 `WithStatePreHandler` / `WithStatePostHandler` 访问和修改状态。状态通过 `context.Context` 中存储的 `internalState` 传递（`graph.go:849-853`），内部包含 `parent` 指针支持嵌套子图的链式查找。

---

## 四、Eino 如何落地（关键源码索引）

### 4.1 编译层（Build → Compile）

| 关键函数 | 文件:行号 | 作用 |
|---------|----------|------|
| `NewGraph[I, O]` | `generic_graph.go:72-88` | 创建泛型图，接受 `WithGenLocalState` 选项 |
| `graph.addNode` | `graph.go:162-230` | 添加节点，校验 state、pre/post handler 类型 |
| `graph.addEdgeWithMappings` | `graph.go:232-294` | 添加边，区分 control edge 和 data edge，触发类型推断 |
| `graph.updateToValidateMap` | `graph.go:561-637` | BFS 链式类型推断，注册 edgeHandler 做运行时类型转换 |
| `graph.compile` | `graph.go:674-892` | 核心编译：类型校验 → 子图编译 → runner 构造 → 编译锁 |
| `compileAnyGraph` | `generic_graph.go:127-158` | 包装 compile 结果：设置 meta/回调/context wrapper |

### 4.2 运行时层（Execute）

| 关键函数 | 文件:行号 | 作用 |
|---------|----------|------|
| `runner.run` | `graph_run.go:109-359` | 主执行循环：提交任务 → 等待完成 → 计算下一批 → 检查终点 |
| `runner.calculateNextTasks` | `graph_run.go:710-733` | 解析完成的任务 → 路由值到 channel → 获取就绪节点 |
| `runner.createTasks` | `graph_run.go:735-756` | 为就绪节点创建 task 对象，附加 context 和 option |
| `runner.resolveCompletedTasks` | `graph_run.go:828-864` | 将完成任务的输出路由到目标 channel（writeChannelValues） |
| `runner.calculateBranch` | `graph_run.go:866-931` | 条件分支：执行 branch.invoke/collect → 确定下一跳节点 → 传播 skip |
| `runner.handleInterrupt` | `graph_run.go:502-569` | 中断处理：保存 channel 状态、inputs、state 到 checkpoint |
| `runner.handleInterruptWithSubGraphAndRerunNodes` | `graph_run.go:598-708` | 处理含子图中断和重跑节点的复杂中断场景 |
| `runner.restoreCheckPointState` | `graph_run.go:382-422` | 从 checkpoint 恢复状态：load channels → 恢复 state |
| `runner.restoreTasks` | `graph_run.go:777-826` | 从 checkpoint 的 inputs 恢复 task 列表 |

### 4.3 Channel 层

| 关键函数 | 文件:行号 | 作用 |
|---------|----------|------|
| `channelManager.updateValues` | `graph_manager.go:138-164` | 将完成节点的输出路由到目标 channel |
| `channelManager.updateDependencies` | `graph_manager.go:167-186` | 将控制依赖通知目标 channel |
| `channelManager.getFromReadyChannels` | `graph_manager.go:189-205` | 遍历所有 channel，获取 ready 节点并运行 preNode handlers |
| `channelManager.reportBranch` | `graph_manager.go:219-246` | 分支 skip 传播：skip 节点沿 successors 链状传播 |
| `dagChannel.get` | `dag.go:128-191` | DAG 就绪判定：control 非 Waiting 且 data 全部上报 |
| `dagChannel.reportSkip` | `dag.go:106-126` | DAG skip 传播：标记 control 为 Skipped，全部 skipped 则自身 skipped |
| `pregelChannel.get` | `pregel.go:55-88` | Pregel 就绪判定：Values 非空即 ready，get 后清空 |
| `pregelChannel.reportValues` | `pregel.go:48-53` | Pregel 值上报：直接写入 Values map |

### 4.4 Task 管理层

| 关键函数 | 文件:行号 | 作用 |
|---------|----------|------|
| `taskManager.submit` | `graph_manager.go:300-351` | 提交任务：pre-handler 同步执行 → goroutine 异步执行 |
| `taskManager.wait` | `graph_manager.go:353-378` | 等待完成：needAll 模式 waitAll，eager 模式 waitOne |
| `taskManager.waitAll` | `graph_manager.go:415-432` | 收集所有完成的任务（循环 waitOne） |
| `taskManager.execute` | `graph_manager.go:285-298` | goroutine 内执行任务：init callbacks → runWrapper → send to done |
| `runPreHandler` / `runPostHandler` | `graph_manager.go:527-556` | 节点的 state pre/post handler 执行 |

### 4.5 类型推断层

| 关键函数 | 文件:行号 | 作用 |
|---------|----------|------|
| `graph.addToValidateMap` | `graph.go:552-557` | 记录待推断的边 |
| `graph.updateToValidateMap` | `graph.go:561-637` | BFS 传播类型信息，注册运行时 handler |
| `graph.getNodeInputType` / `getNodeOutputType` | `graph.go:648-664` | 获取节点的类型（START/END 从 graph 获取，中间节点从 graphNode 获取） |
| `runnablePacker.toComposableRunnable` | `runnable.go:100-155` | 泛型 runnable 包装：类型安全的 invoce/transform 闭包 |

### 4.6 Callback 与观测层

| 关键函数 | 文件:行号 | 作用 |
|---------|----------|------|
| `graph.onCompileFinish` | `graph.go:1011-1025` | 编译完成时调用所有 `GraphCompileCallback.OnFinish` |
| `graph.toGraphInfo` | `graph.go:948-1009` | 收集完整的图信息（节点、边、分支、类型、状态） |
| `GraphCompileCallback.OnFinish` | `introspect.go:54-57` | 编译回调接口，`graphInfo` 包含全部拓扑和类型信息 |

### 4.7 关键常量与类型

| 常量/类型 | 文件:行号 | 值/含义 |
|----------|----------|--------|
| `START` | `graph.go:37` | `"start"` — 虚拟起始节点 |
| `END` | `graph.go:40` | `"end"` — 虚拟终止节点 |
| `runTypePregel` | `graph.go:47` | Pregel 运行模式（支持循环） |
| `runTypeDAG` | `graph.go:49` | DAG 运行模式（无环） |
| `NodeTriggerMode` | `types.go:39-46` | `AnyPredecessor` / `AllPredecessor` |
| `dependencyState` | `dag.go:42-48` | `Waiting` / `Ready` / `Skipped` |
| `ErrGraphCompiled` | `graph.go:160` | 编译后修改图错误 |
| `DAGInvalidLoopErr` | `graph.go:1129` | DAG 循环依赖错误 |
| `interruptError` | `graph_run.go:568` | 顶层中断错误（含 InterruptInfo） |
| `subGraphInterruptError` | `graph_run.go:555-559` | 子图中断错误（含 CheckPoint，向上冒泡） |

### 4.8 编译时校验清单

编译期 `graph.compile()` 依次执行以下校验：

1. **toValidateMap 为空**（`graph.go:708-713`）：所有 passthrough 节点类型必须被推断出来，否则编译失败。
2. **fieldMapping 无冲突**（`graph.go:715-727`）：不允许同一节点的两个 mapping 目标到同一个字段。
3. **startNodes 非空**（`graph.go:701-703`）：没有从 START 发出的边。
4. **endNodes 非空**（`graph.go:704-706`）：没有指向 END 的边。
5. **DAG 无环校验**（`graph.go:856-862`）：Kahn 算法检测环，发现环则返回 `DAGInvalidLoopErr`（包含具体环路信息）。
6. **组件能力降级**（`graph.go:597-601`）：若类型 "May" 兼容（非严格匹配但可转换），注册运行时 `edgeHandlerManager` 做类型转换。
7. **SubGraph 编译回调注册**（`graph.go:729-732`）：如果设置了 `GraphCompileCallback`，注册 `beforeChildGraphsCompile` 和 `onCompileFinish`。

---

## 五、与 Rive Work DAG 的对照

### 5.1 共性

| 维度 | Eino Compose | Rive Work DAG |
|------|-------------|--------------|
| 拓扑与执行分离 | Graph → Compile → Runnable | Work DAG → Dispatch → Execution |
| 编译锁 | `graph.compiled` 标志阻止修改 | Dispatch 创建后的投影不可变 |
| 自身图支持 | `GraphNode.g` → 递归编译 | 无（当前版本） |
| 中断/恢复 | CheckPoint + InterruptError 冒泡 | 未实现 |

### 5.2 可借鉴的设计

1. **编译边界概念**：在 Dispatch 创建时做一次完整的拓扑校验（类型检查、依赖完整性），而 Eino 的 `toValidateMap` 延迟推断机制可参考。
2. **Channel 抽象**：将"所有依赖完成触发"与"任一依赖触发"统一到同一个 channel 接口，仅通过实现多态区分。
3. **嵌套地址空间**：`AddressSegment` 路径链拼接，支持跨 dispatch 的 checkpoint 和中断恢复。
4. **编译回调与可观测性**：`GraphCompileCallback` 模式 — 在编译完成时导出完整拓扑信息。
5. **层级化错误处理**：在 Dispatch 级别、节点级别、transition 级别分别提供 hook 点。

### 5.3 差异

| 维度 | Eino | Rive |
|------|------|------|
| 触发模式 | AnyPredecessor + AllPredecessor 两种 | 当前仅 AllPredecessor |
| 循环支持 | Pregel 模式支持循环 + maxSteps 限制 | 不支持 |
| 执行单元 | goroutine pool（taskManager） | 独立 agent 进程 |
| 状态管理 | local state 通过 context 链式传递 | 通过工作空间文件传递 |
| 流式支持 | 内置 Invoke/Stream/Collect/Transform 4×4 转换 | 无（仅 Invoke 语义） |
