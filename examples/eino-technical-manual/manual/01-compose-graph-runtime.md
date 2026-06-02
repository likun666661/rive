# Chapter 1: Compose Graph — 编译与运行模型

## 1. 面临的问题

Eino 要解决的核心问题是 **LLM 应用里"可组合执行图"的编译与运行分离**。开发者需要把 prompt、ChatModel、tool、retriever、indexer、lambda、子图等不同形态的组件组合成一个可运行单元，并且同一个单元要同时支持 Invoke（普通调用）、Stream（流式输出）、Collect（流式输入）、Transform（流式转换）四种执行形态。

如果只用普通函数调用串联所有组件，几个问题会同时出现：

1. **执行顺序散落在业务代码里**。每个开发者都会在各自应用里手写"先调 A，再调 B，有分支走 C"的控制流，无法统一检查依赖关系、类型兼容性和拓扑正确性。

2. **组件能力不一致**。ChatModel 可能支持 Invoke + Stream，Retriever 可能只支持 Invoke，Lambda 可能只支持 Transform。上层统一调用时，需要自动发现组件能力并做降级/转接。

3. **多节点应用需要复杂的拓扑表达**。包括字段映射（前一个节点的输出字段如何映射到后一个节点的输入字段）、并行执行、条件分支、嵌套子图，而不仅仅是线性 pipeline。

4. **运行时需要统一的基础设施**。callback 注入、checkpoint 持久化、interrupt/resume（中断与恢复）、状态共享，这些横切关注点不可能让每个组件自己实现。

## 2. 为什么这么难

LLM 应用的组件不是普通 DAG 里的纯函数，它们具有多项特殊性：

### 2.1 类型适配的复杂性

节点之间的类型必须兼容，但开发者经常添加 passthrough（透传）节点，其输入输出类型需要在编译期通过拓扑上下文推断。例如：

```
nodeA (output: string) -> passthrough -> nodeB (input: string)
```

如果 passthrough 没有显式声明类型，编译期必须沿着图拓扑链式推断出它是 `passthrough[string]`。Eino 在 `graph.go:65-68` 的 `toValidateMap` 中追踪所有待推断的节点，并在 `updateToValidateMap`（`graph.go:561`）中通过 BFS 方式沿边链式解析。

### 2.2 执行触发模式的差异

不同场景需要不同的触发策略：
- **AnyPredecessor（任一前驱完成即触发）**：适合 agentic 循环图，一个节点可能被多个前驱的任意一个触发。对应 Pregel 运行时。
- **AllPredecessor（所有前驱完成才触发）**：适合确定性 DAG，一个节点需要收集所有前驱的输出后才能执行。对应 DAG 运行时。

同一个图可能既包含需要 AllPredecessor 的子图，又包含需要 AnyPredecessor 的区域，编译期必须正确推断并配置每个节点的 channel 行为。

### 2.3 多执行形态的交叉组合

四个执行形态（Invoke / Stream / Collect / Transform）互相之间存在复杂的降级和转换关系。如果某个组件只实现了 `Invoke`，但上层调用 `Stream`，运行时需要通过 `runnablePacker` 把 Invoke 的结果包装成 stream reader 返回。反之亦然。`compose/runnable.go:194-334` 实现了全部 4×4 组合的自动转换。

### 2.4 嵌套图的地址空间

子图的 callback、checkpoint、interrupt 都需要挂接到父图的地址空间里。Eino 使用 `AddressSegment`（路径段）链式拼接来构建全局唯一的节点路径，例如 `parent_graph/node_key/sub_graph/sub_node`。中断发生时，`interruptError` 从最深层子图向上冒泡，直到被某个父图的 `runner` 捕获并转换为该父图的 checkpoint。

### 2.5 状态、回调和检查点的正确传播

共享状态（local state）通过 `context.Context` 传递，子图可以访问父图的状态。当子图被中断并恢复时，状态必须是完整且一致的。这要求 checkpoint 序列化不仅要保存所有 channel 的状态（值、依赖、跳过标记），还要保存每个节点的输入输出数据。

## 3. 设计思路

### 3.1 三层编排抽象

Eino 在 `compose/` 下提供三层抽象，三者最终都编译成统一的 `Runnable[I, O]` 接口：

| 抽象层 | 文件 | 特点 | 运行时 |
|---|---|---|---|
| `Graph[I,O]` | `generic_graph.go:93` | 显式有向图，手动添加节点和边，支持循环 | Pregel (默认) |
| `Chain[I,O]` | `chain.go:37` | Fluent Builder 风格，顺序连接 | DAG |
| `Workflow[I,O]` | `workflow.go:61` | 声明式依赖和字段映射包装 | DAG |

### 3.2 Graph vs Compiled Runnable — 编译边界分离

这是 Eino 最核心的设计决策：**将拓扑构建（Graph construction）与运行时执行（Runnable execution）彻底分离**。

- **构建阶段**：用户在 `Graph/Chain/Workflow` 上添加节点、边、分支、字段映射、回调钩子。此时图处于"未编译"状态，用户仍可自由修改拓扑。
- **编译阶段**（`Graph.Compile()` → `compileAnyGraph()` → `graph.compile()`）：
  1. 完成所有待推断的类型（`toValidateMap` 必须为空，`graph.go:708-713`）
  2. 校验字段映射无冲突（不允许同一节点的两个 mapping 目标到同一字段，`graph.go:715-727`）
  3. 递归编译所有子图节点（`graph_node.go:121-149` 的 `compileIfNeeded`）
  4. 收集每个节点的数据边和控制边，构建 `chanCall` 对象（`graph.go:729-757`）
  5. 反转边关系构建 `dataPredecessors` 和 `controlPredecessors`（`graph.go:759-797`）
  6. 创建 `runner` 结构体（`graph.go:814-834`）
  7. 如果是 DAG 模式，执行 Kahn 算法校验无环（`graph.go:856-862`）
  8. 创建 `checkPointer` 对接 checkpoint store（`graph.go:864-878`）
  9. 将 `runner` 包装为 `composableRunnable`，再通过 `toGenericRunnable` 包装为 `Runnable[I,O]`（`graph.go:891`）
- **运行时阶段**：编译产出的 `Runnable[I,O]` 不再暴露拓扑信息，只提供四个执行方法。执行时由 `runner.run()` 主循环驱动。

编译边界带来的关键收益：

1. **安全性**：`graph.compiled` 标志位（`graph.go:84`）阻止编译后的拓扑修改，`ErrGraphCompiled` 错误（`graph.go:160`）在每次写操作时检查。
2. **可观测性**：编译完成的回调（`GraphCompileCallback`，`introspect.go:54`）让外部系统可以获取完整的 `GraphInfo`（包括所有节点、边、分支、类型信息），用于可视化、监控和审计。
3. **可复用性**：同一个 `Graph` 可以多次编译（每次可传入不同的 `GraphCompileOption`），产生多个独立的 `Runnable` 实例。

### 3.3 NodeTriggerMode — 节点触发模式

`NodeTriggerMode`（`types.go:39-46`）是一个编译选项，控制图中节点的触发行为：

```go
type NodeTriggerMode string
const (
    AnyPredecessor NodeTriggerMode = "any_predecessor"  // 任一前驱完成即触发
    AllPredecessor NodeTriggerMode = "all_predecessor"  // 所有前驱完成才触发
)
```

编译期在 `graph.go:680-690` 中根据 Graph 类型和编译选项决定运行模式：
- 默认 Graph → `runTypePregel`，使用 `pregelChannelBuilder`
- Chain / Workflow 或显式指定 `AllPredecessor` → `runTypeDAG`，使用 `dagChannelBuilder`

**eager 执行**也与触发模式相关（`graph.go:693-699`）：
- Workflow 或 DAG 模式默认开启 `eager`（只要能执行就立即执行，不等本轮所有 ready task 一起 submit）
- 可通过 `WithEagerExecutionDisabled()` 关闭

### 3.4 DAG vs Pregel 运行时

两种运行时共享同一个主循环结构（`runner.run()`，`graph_run.go:109`），但底层 channel 实现不同：

| 维度 | DAG Channel (`dag.go:50`) | Pregel Channel (`pregel.go:25`) |
|---|---|---|
| 触发条件 | 所有 control 前置都 Ready + 所有 data 前置都上报 | 任一 data 前置上报值 |
| 依赖跟踪 | 维护 `ControlPredecessors`（Waiting/Ready/Skipped）和 `DataPredecessors`（bool） | 只维护 `Values` map |
| Skip 传播 | 支持：若所有 control 前置被 skip，节点自身也被 skip | 不支持 |
| Fan-in merge | 收集所有 data 前置的值后统一 merge | 每次 get 后清空 Values |
| 循环支持 | 不支持（编译期 Kahn 算法拒绝环） | 支持「如果拓扑检测到环，DAG 编译会直接失败（`graph.go:856-862`）」 |

DAG channel 的核心判断在 `dag.go:get()` 方法（约 line 113-165）：
1. 检查 `Skipped` 标志
2. 检查所有 `ControlPredecessors` 是否都不是 Waiting 且至少一个是 Ready
3. 如果所有 control 前置都是 Skipped，则标记自身 Skipped 并返回 false
4. 检查所有 `DataPredecessors` 是否都已上报

Pregel channel 的判定在 `pregel.go:get()` （line 55-93）：
1. 若 `Values` 为空则返回 false
2. 遍历所有 value，通过 `edgeHandlerManager.handle()` 做类型转换
3. 若只有一个 value 则直接返回，否则 merge
4. 清空 `Values`

### 3.5 嵌套图与状态传递

当一个节点本身是一个子图时，`graphNode.g` 字段（`graph_node.go:63`）持有 `AnyGraph` 接口。编译时递归调用 `gn.g.compile(ctx, compileOption)`（`graph_node.go:123-124`），产生一个 `composableRunnable`。

子图与其父图的关键集成点：

- **Callback 地址**：通过 `AddressSegment` 拼接形成路径，例如 `parent/parent/child/0_LAMBDA`。
- **Local State**：通过 `WithGenLocalState[S]`（`generic_graph.go:37-44`）注册状态生成器，编译后的 `runner` 在 `runCtx` 闭包（`graph.go:842-854`）中创建 `internalState` 并注入 `context.Context`。子图通过 state 的 `parent` 指针形成链式查找。
- **Checkpoint 嵌套**：子图的 `runner` 在中断时创建 `subGraphInterruptError`，沿回调链向上冒泡到父图的 `runner.run()` 主循环。父图的 `handleInterruptWithSubGraphAndRerunNodes`（`graph_run.go:288-301`）将子图的中断信息保存到检查点中。

### 3.6 Local State 机制

Local state 是图编译时注册的、按运行实例共享的状态对象。声明方式：

```go
type MyState struct {
    Collected []string
}

graph := compose.NewGraph[string, string](
    compose.WithGenLocalState(func(ctx context.Context) *MyState {
        return &MyState{Collected: make([]string)}
    }),
)
```

节点可以通过 `WithStatePreHandler` / `WithStatePostHandler`（在 `graph_add_node_options.go` 中定义）访问和修改状态：

```go
graph.AddNode("node1", someNode,
    compose.WithStatePreHandler(func(ctx context.Context, in string, state *MyState) (string, error) {
        state.Collected = append(state.Collected, in)
        return in, nil
    }),
)
```

状态通过 `context.Context` 中存储的 `internalState` 结构体传递（`graph.go:849-853`）。`internalState` 包含 `parent` 指针，使得嵌套子图可以向上查找父图的状态。

## 4. 源码走读

### 4.1 核心数据结构总览

以下按文件组织，展示关键类型和函数的关系：

#### `types.go` — 类型常量与触发模式 (47 lines)

- `component`（类型别名，`types.go:23`）：所有组件的统一类型标签，包括 `ComponentOfGraph`、`ComponentOfWorkflow`、`ComponentOfChain`、`ComponentOfLambda` 等（`types.go:27-36`）
- `NodeTriggerMode`（`types.go:39-46`）：`AnyPredecessor` 和 `AllPredecessor` 两个枚举值

#### `graph.go` — 核心图结构 (1219 lines)

核心结构 `graph`（`graph.go:57-89`）包含以下关键字段：

| 字段 | 类型 | 用途 |
|---|---|---|
| `nodes` | `map[string]*graphNode` | 所有节点 |
| `controlEdges` | `map[string][]string` | 控制依赖边（执行顺序） |
| `dataEdges` | `map[string][]string` | 数据流边 |
| `branches` | `map[string][]*GraphBranch` | 条件分支 |
| `toValidateMap` | `map[string][]struct{...}` | 待类型推断的边 |
| `compiled` | `bool` | 编译锁标志 |
| `handlerOnEdges` | `map[string]map[string][]handlerPair` | 边上的类型转换处理器 |
| `handlerPreNode` | `map[string][]handlerPair` | 节点前置处理器（字段映射等） |
| `handlerPreBranch` | `map[string][][]handlerPair` | 分支前置处理器 |

编译方法 `graph.compile()`（`graph.go:674-892`）是核心编排入口，详细流程见 3.2 节。

常量：`START = "start"`（`graph.go:37`）和 `END = "end"`（`graph.go:40`）作为图的虚拟起止节点。

#### `generic_graph.go` — 泛型公共 API (158 lines)

- `Graph[I, O any]`（`generic_graph.go:93-95`）：嵌入 `*graph` 指针的泛型包装
- `NewGraph[I, O]`（`generic_graph.go:72-88`）：创建泛型图，接受 `WithGenLocalState[S]` 选项
- `Graph.Compile()`（`generic_graph.go:123-124`）：委托给 `compileAnyGraph[I, O]()`
- `compileAnyGraph[I, O]()`（`generic_graph.go:127-158`）：调用 `g.compile()` 获得 `composableRunnable`，设置 meta/回调/context wrapper，通过 `toGenericRunnable` 包装为 `Runnable[I,O]`

#### `graph_node.go` — 节点表示 (177 lines)

- `graphNode`（`graph_node.go:60-70`）：聚合了 `cr`（预编译 runnable）、`g`（子图）、`nodeInfo`（名称/前置后置处理器/compileOption）、`executorMeta`（组件类型/回调能力）
- `executorMeta`（`graph_node.go:29-43`）：组件的元信息，包括 `component`（类型标签）、`isComponentCallbackEnabled`（是否原生支持回调）、`componentImplType`（实现类名）
- `nodeInfo`（`graph_node.go:45-57`）：节点的展示名、输入输出 key、state pre/post 处理器、子图编译选项
- `compileIfNeeded()`（`graph_node.go:121-149`）：如果节点是子图则递归编译，否则返回已有的 `composableRunnable`；编译后处理 inputKey/outputKey 的包装

#### `graph_run.go` — 运行时引擎 (1055 lines)

- `runner`（`graph_run.go:43-84`）：编译后的运行时引擎
  - `chanSubscribeTo`：所有节点的 `chanCall` 映射
  - `successors`：后继节点映射（用于 skip 传播）
  - `dataPredecessors` / `controlPredecessors`：前驱依赖映射
  - `inputChannels`：START 虚拟节点的 `chanCall`
  - `chanBuilder`：channel 工厂（`pregelChannelBuilder` 或 `dagChannelBuilder`）
  - `eager` / `dag`：执行模式标志
  - `checkPointer`、`interruptBeforeNodes`、`interruptAfterNodes`：中断/检查点支持
  - `mergeConfigs`：fan-in merge 配置

- `chanCall`（`graph_run.go:31-39`）：节点的执行包装
  - `action`：`*composableRunnable`，实际可执行对象
  - `writeTo`：数据边目标节点列表
  - `writeToBranches`：条件分支目标
  - `controls`：控制边目标
  - `preProcessor` / `postProcessor`：state 处理器的 runnable

- `runner.run()`（`graph_run.go:109-359`）：主执行循环
  1. 初始化 `channelManager` 和 `taskManager`（`graph_run.go:129-130`）
  2. 检查点恢复或从头初始化（`graph_run.go:155-235`）
  3. 主循环：submit tasks → wait completion → calculate next tasks（`graph_run.go:241-359`）
  4. 每步都检查中断条件（`interruptBeforeNodes` / `interruptAfterNodes`）
  5. 在 Pregel 模式下检查 `maxSteps` 防止无限循环（`graph_run.go:249-251`）

- `calculateNextTasks()`（`graph_run.go:710`）：完成的任务经过 `resolveCompletedTasks` 写入 channel，然后 `cm.updateAndGet` 获取就绪节点，创建新的 task

#### `graph_manager.go` — 通道与任务管理 (556 lines)

- `channel` 接口（`graph_manager.go:29-38`）：抽象了节点间的通信机制
  - `reportValues()`：向上报值
  - `reportDependencies()`：上报控制依赖
  - `reportSkip()`：跳过传播
  - `get()`：获取就绪数据

- `channelManager`（`graph_manager.go:115-125`）：管理所有 channels，提供 `updateAndGet()` 统一操作
  - `updateValues()`（`graph_manager.go:138-164`）：将完成节点的输出路由到目标 channel
  - `updateDependencies()`（`graph_manager.go:167-186`）：将控制依赖通知目标 channel
  - `getFromReadyChannels()`（`graph_manager.go:189-205`）：遍历所有 channel，获取 ready 节点的值并运行 preNode handlers

- `taskManager`（`graph_manager.go:269-283`）：管理并发 goroutine pool
  - `submit()`（`graph_manager.go:300-351`）：同步执行 pre-handler，异步启动 goroutine 执行任务
  - `wait()`（`graph_manager.go:353-378`）：等待任务完成，处理取消/超时
  - 智能优化：当只有一个任务且 `needAll` 模式时，在 submit 的 goroutine 内部同步执行，避免不必要的 goroutine 切换

- Handler managers（`graph_manager.go:40-113`）：三个类型转换管理器
  - `edgeHandlerManager`（`graph_manager.go:40-65`）：边上的流/非流类型转换
  - `preNodeHandlerManager`（`graph_manager.go:67-89`）：节点前置处理（字段映射等）
  - `preBranchHandlerManager`（`graph_manager.go:91-113`）：分支条件的前置类型转换

#### `dag.go` — DAG Channel (195 lines)

`dagChannel`（`dag.go:50-60`）实现了 AllPredecessor 语义：
- `ControlPredecessors`：每个控制前置的状态（Waiting/Ready/Skipped）
- `DataPredecessors`：每个数据前置是否上报
- `Skipped`：节点自身是否被跳过
- `mergeConfig`：支持 `StreamMergeWithSourceEOF` 模式

#### `pregel.go` — Pregel Channel (95 lines)

`pregelChannel`（`pregel.go:25-29`）实现了 AnyPredecessor 语义：
- 只维护 `Values` map，任何前置上报值就 ready
- 每次 `get()` 后清空 `Values`（`pregel.go:60`）
- 支持多值 merge（`pregel.go:74-93`）

#### `introspect.go` — 自省与可观测性 (57 lines)

- `GraphInfo`（`introspect.go:41-52`）：编译完成后导出的完整图信息，包括所有节点、边、分支、类型、状态生成器
- `GraphNodeInfo`（`introspect.go:27-36`）：单个节点的详细信息
- `GraphCompileCallback`（`introspect.go:54-57`）：编译完成后的回调接口，供外部系统做可视化/审计

### 4.2 编译流程详解

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
        └── toGenericRunnable[I,O] → runnablePacker → Runnable[I,O] [generic_graph.go:138-155]
```

### 4.3 运行时执行流程

```
Runnable.Invoke(ctx, input)
  └── cr.i(ctx, input) → runner.invoke(ctx, input)
        └── runner.run(ctx, isStream=false, input)
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

## 5. 模式与示例

### 5.1 基本图构建与编译

```go
// 创建泛型图
graph := compose.NewGraph[string, *schema.Message]()

// 添加节点
_ = graph.AddChatTemplateNode("prompt", prompt.FromMessages(schema.Jinja2, ...))
_ = graph.AddChatModelNode("model", chatModel)
_ = graph.AddLambdaNode("transform", compose.InvokableLambda(func(ctx context.Context, msg *schema.Message) (string, error) {
    return msg.Content, nil
}))

// 添加边
_ = graph.AddEdge(compose.START, "prompt")
_ = graph.AddEdge("prompt", "model")
_ = graph.AddEdge("model", "transform")
_ = graph.AddEdge("transform", compose.END)

// 编译
runnable, err := graph.Compile(ctx,
    compose.WithGraphName("my_graph"),
    compose.WithNodeTriggerMode(compose.AllPredecessor),
)

// 执行
result, err := runnable.Invoke(ctx, "hello world")
```

### 5.2 条件分支

```go
// 分支条件：根据输入决定走向哪个节点
condition := func(ctx context.Context, in string) (string, error) {
    if len(in) > 10 {
        return "long_handler", nil
    }
    return "short_handler", nil
}

branch := compose.NewGraphBranch(condition, map[string]bool{
    "long_handler":  true,
    "short_handler": true,
})
_ = graph.AddBranch("router_node", branch)
```

### 5.3 嵌套子图

```go
subGraph := compose.NewGraph[string, string]()
// ... 添加子图节点和边 ...

parentGraph := compose.NewGraph[string, string]()
_ = parentGraph.AddGraphNode("sub_graph_key", subGraph)
_ = parentGraph.AddEdge(compose.START, "sub_graph_key")
_ = parentGraph.AddEdge("sub_graph_key", compose.END)

runnable, _ := parentGraph.Compile(ctx,
    compose.WithGraphName("parent_of_sub"),
)
```

### 5.4 Local State 共享

```go
type state struct {
    Logs []string
}

genState := func(ctx context.Context) *state {
    return &state{Logs: make([]string)}
}

graph := compose.NewGraph[string, string](compose.WithGenLocalState(genState))

graph.AddLambdaNode("node_a", compose.InvokableLambda(
    func(ctx context.Context, in string) (string, error) {
        // 可以通过 context 获取 state, 但更推荐用 PreHandler/PostHandler
        return in, nil
    },
), compose.WithStatePreHandler(func(ctx context.Context, in string, state *state) (string, error) {
    state.Logs = append(state.Logs, "node_a received: "+in)
    return in, nil
}))
```

### 5.5 Workflow 声明式字段映射

```go
wf := compose.NewWorkflow[string, *WorkflowResult]()

wf.AddChatModelNode("model", chatModel)
wf.AddLambdaNode("parser", resultParser)

// 声明式依赖 + 字段映射
wf.AddInput("parser", "model") // parser 依赖 model 的输出

runnable, _ := wf.Compile(ctx)
```

## 6. 常见陷阱

### 6.1 编译后修改图

调用 `Compile()` 后，`graph.compiled` 被设为 `true`（`graph.go:887`）。后续任何对图拓扑的修改（`AddEdge`、`AddNode` 等）都会返回 `ErrGraphCompiled` 错误（`graph.go:160`）。需要修改拓扑时，必须先创建新图。

### 6.2 DAG 模式下的循环依赖

当使用 `AllPredecessor` 触发模式（或 Workflow）时，编译期使用 Kahn 算法检测环（`graph.go:856-862`）。如果存在环，编译会直接失败并返回 `DAGInvalidLoopErr`（`graph.go:1129`）。需要使用 `AnyPredecessor` 模式（默认 Pregel）来支持循环图。

### 6.3 passThrough 节点的类型推断链断裂

如果 passthrough 节点的前驱类型也是未知的（例如两个 passthrough 串联），类型推断链会在编译期报错。确保至少有一个节点的类型是确定的，或者显式指定 passthrough 的泛型类型。

### 6.4 Pregel 模式下的 maxSteps 限制

Pregel 模式支持循环，为避免无限循环，Eino 默认设置 `maxRunSteps = len(nodes) + 10`（`graph.go:884`）。如果图包含长链或多轮循环，需要显式通过 `WithMaxRunSteps(n)` 提高限制。超过限制会返回 `ErrExceedMaxSteps`。

### 6.5 Field mapping 目标冲突

不允许同一个节点的两个不同 field mapping 映射到同一个目标字段（`graph.go:715-727`）。例如同时将 `a.field_x` 和 `b.field_y` 都映射到 `c.result` 会导致编译失败。

### 6.6 子图中断的正确传播

当子图发生 interrupt，`interruptError` 只向上冒泡到直接父图。如果有多层嵌套，每一层都需要正确处理 `subGraphInterruptError`。频繁的检查点 I/O 可能成为性能瓶颈，需要合理选择 `CheckPointStore` 的实现。

### 6.7 Eager execution 与并发安全

DAG 模式下默认启用 eager execution（`graph.go:694`），意味着节点一旦就绪就立即执行，不需要等同一个 step 内所有就绪节点一起 submit。这提高了吞吐，但 local state 的并发修改需要用户自行保证安全性。

## 7. Rive 可以学到什么

Eino 的 Compose Graph 设计与 Rive 的 Work DAG 在理念上有显著共鸣，但实现路径不同：

### 7.1 Topology 与 Execution 的分离

两者都将拓扑描述与执行引擎分开：
- **Eino**：`Graph`（构建时）→ `Compile`（编译边界）→ `Runnable`（运行时）。编译锁（`graph.compiled`）阻止拓扑修改。
- **Rive**：Work DAG（声明时）→ Dispatch（投影边界）→ Execution（agent 执行）。

Rive 可以借鉴 Eino 的编译边界概念，在 Dispatch 创建时做一次彻底的**拓扑校验**（如类型检查、依赖完整性验证），而不是在执行时才发现问题。Eino 的 `toValidateMap` 机制（编译期延迟类型推断直到所有信息齐备）可以作为 Rive 中 task 输入输出类型检查的参考。

### 7.2 两种触发模式的统一

Eino 在同一个运行时框架中统一了 `AnyPredecessor` 和 `AllPredecessor` 两种截然不同的触发模式，仅通过 channel 实现的多态（`dagChannel` vs `pregelChannel`）来区分行为。Rive 目前 Work DAG 的触发是确定的（所有依赖完成），但未来如果引入 stream 或 event-driven 触发，可以参考这种 channel 抽象来统一不同的触发策略。

### 7.3 嵌套图的地址与检查点

Eino 的嵌套图通过 `AddressSegment` 路径链构建全局唯一的节点标识，检查点从子图冒泡到父图的机制清晰且完整。Rive 如果未来支持 sub-dispatch（在一个 dispatch 内部启动另一个 dispatch），可以借鉴这种地址空间设计来追踪跨 dispatch 的检查点和中断恢复。

### 7.4 编译回调与可观测性

Eino 的 `GraphCompileCallback`（`introspect.go:54`）让外部系统在编译完成时获取完整的 `GraphInfo`。Rive 可以在 Dispatch 投影完成时提供类似的回调钩子，将完整的 DAG 拓扑信息导出给监控和可视化系统，而不必等到运行时。

### 7.5 错误处理的统一模式

Eino 在多个层级提供错误处理钩子：`onGraphStart`、`onGraphError`、`onGraphEnd`（`graph_run.go:111-119` 的 defer）以及节点级别的 pre/post handler。Rive 的 Work DAG 调度器可以考虑类似的层级化错误处理：在 Dispatch 级别、节点级别、transition 级别分别提供 hook 点，让用户注入横切逻辑而不侵入业务 agent。
