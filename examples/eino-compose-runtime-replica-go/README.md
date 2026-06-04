# Eino Compose Runtime Replica (Go MVP)

受 Eino (CloudWeGo) 启发的第二章骨架示例与验证项目,覆盖核心编译边界与 DAG/Pregel 执行引擎,并实现三层编排抽象 (FieldMapping / Workflow / Chain / Parallel / Branch)。本项目为学习与研究用途的章节级骨架,非 Eino 的完整产品复刻。

## 架构总览

本复刻版实现 Eino 最核心的设计决策：**图拓扑构建与运行时执行分离**。

```
Graph Builder  ──>  Compile  ──>  Runnable[I, O]
  (可变)           (编译锁)       (不可变执行体)

第一层: Graph (最灵活)
  ├── FieldMapping (字段级数据映射)
  │
第二层: Workflow (声明式编排)
  ├── AddInput + FieldMapping
  ├── AddDependency (控制依赖)
  └── SetStaticValue (静态注入)
  │
第三层: Chain (Builder 风格)
  ├── AppendLambda / AppendGraph
  ├── AppendParallel (内建并行)
  └── AppendBranch (内建分支)
  │
第三章: Runnable Stream / Collect / Transform / Callback 教学示例
  ├── composableRunnable 的 stream 回退机制
  ├── StreamReader 生产-收集模式演示
  ├── Transform 流式变换管道演示
  └── Callback 生命周期计时演示
```

### 三层抽象对比

| 维度 | Graph | Workflow | Chain |
|------|-------|----------|-------|
| 控制力 | 最高 (手动 AddEdge) | 中等 (声明式 AddInput) | 最低 (自动 AppendX) |
| 便利性 | 最低 | 中等 | 最高 |
| 字段映射 | 通过 addEdgeWithMappings (手动) | 内置在 AddInput | 自动 (类型匹配即传) |
| 并行/分支 | 手工多入边 / AddBranch | 多 AddInput / AddBranch | AppendParallel / AppendBranch 内建 |
| 适合场景 | 复杂拓扑、Pregel 循环 | 声明式数据流 + 字段映射 | 线性/条件/并发 pipeline |

---

## 第一章功能 (Graph / DAG / Pregel)

### DAG vs Pregel

| 维度 | DAG (AllPredecessor) | Pregel (AnyPredecessor) |
|---|---|---|
| 触发条件 | 所有控制 + 数据前驱就绪 | 任一数据前驱上报 |
| 环检测 | 编译时 Kahn 拓扑排序拒绝环 | 允许环,maxSteps 安全上限 |
| Skip 传播 | 支持 | 不支持 |

### 关键特性

- **Graph Builder**: 添加节点 (Lambda)、数据边、控制边、分支
- **编译锁**: `Compile()` 后 Graph 锁定,修改返回 `ErrGraphCompiled`
- **Runnable[I,O]**: 统一执行接口 `Invoke(ctx, input) (output, err)`
- **NodeTriggerMode**: `AllPredecessor` (DAG) 和 `AnyPredecessor` (Pregel)
- **Channel 抽象**: `dagChannel` 实现 AllPredecessor 语义; `pregelChannel` 实现 AnyPredecessor 语义
- **maxSteps**: Pregel 模式步数上限,防止无限循环
- **GraphInfo**: 编译时拓扑信息导出
- **Event Log**: 线程安全的执行事件记录

---

## 第二章功能 (FieldMapping / Workflow / Chain / Parallel / Branch)

### FieldMapping — 字段级数据映射

**解决的问题**:
在 Eino 图编排中,相邻节点的输入/输出类型往往不匹配——前驱输出是大结构体但后继只需一个字段,或多个前驱的不同字段需汇聚到一个后继。传统 `AddEdge` 只能传递整个输出值,无法做字段级裁剪。

**设计方案**:
- **六个构造函数**: `MapFields`、`FromField`、`ToField`、`MapFieldPaths`、`FromFieldPath`、`ToFieldPath`
- **自定义提取器**: `WithCustomExtractor` 支持任意数据源提取
- **路径分隔符**: 使用 `\x1F` (Unit Separator) 编码嵌套路径,与 Eino 源码一致
- **编译时校验**: `validateFieldMapping` 在编译阶段检查字段存在性、导出性、类型赋值兼容性
- **请求时执行**: `fieldMap` 按 mapping 规则提取字段,通过 `convertTo` 转换为目标类型

```go
// 六个构造函数示例
MapFields("Query", "question")           // 单字段 → 单字段
FromField("Query")                       // 提取一个字段作为后继整个输入
ToField("Result")                        // 整个输出 → 指定字段
MapFieldPaths(                           // 嵌套路径 → 嵌套路径
    FieldPath{"data", "title"},
    FieldPath{"result", "title"},
)
FromFieldPath(FieldPath{"user", "name"}) // 嵌套路径 → 整个输入
ToFieldPath(FieldPath{"output", "text"}) // 整个输出 → 嵌套路径
```

### Workflow — 声明式数据流编排

**解决的问题**:
原始 Graph API 需要手动调用 `AddEdge` / `AddControlEdge`,当图变大时边的声明分散,字段映射需额外配置,控制依赖与数据依赖混在一起。

**设计方案**:
- **AddInput(fromNodeKey, mappings...)**: 一次声明从哪些前驱取数据及字段映射规则
- **AddDependency(fromNodeKey)**: 纯执行依赖,不传递数据
- **SetStaticValue(path, value)**: 编译时注入常量值
- **End()**: 终端的声明式输入
- **三态依赖**: `normalDependency` (数据+控制) / `noDirectDependency` (仅数据) / `branchDependency` (分支)
- **编译时展开**: 延迟闭包数组在 `Compile()` 时统一执行,展开为底层 Graph

```go
wf := compose.NewWorkflow[*Input, *Output]()
wf.AddLambdaNode("process", ...).AddInput(START, FromField("Query"))
wf.End().AddInput("process", ToField("Result"))
```

### Chain — Builder 风格线性管道

**解决的问题**:
很多场景下处理流程是简单的 A → B → C 线性管道,用 Graph 需手动 AddEdge 过于繁琐。

**设计方案**:
- **AppendLambda / AppendGraph / AppendPassthrough**: 追加节点
- **AppendParallel**: 嵌入 Parallel 并行组
- **AppendBranch**: 嵌入 ChainBranch 条件分支
- **自动命名**: 节点自动命名 (`node_0`, `node_1`, ...),无需手动指定 key
- **自动连接**: 编译时自动连接 START/END,`preNodeKeys` 追踪尾部节点

```go
chain := compose.NewChain[string, string]()
chain.
    AppendLambda(transformFn).
    AppendLambda(formatFn).
    Compile(ctx)
```

### Parallel — 内建并行执行

**解决的问题**:
对同一输入执行多个独立操作(如同时大写+小写转换),Graph API 需手动创建扇出拓扑。

**设计方案**:
- 节点共享同一前驱输入,通过 `outputKey` 标注输出来源
- 下游节点接收 `map[string]any`,通过 key 区分来源
- 运行时 goroutine 并发执行,taskManager 管理同步

```go
parallel := compose.NewParallel()
parallel.AddLambda("upper", upperFn).AddLambda("lower", lowerFn)
chain.AppendParallel(parallel)
```

### ChainBranch — 条件分支

**解决的问题**:
根据输入内容选择不同处理路径(如长文本走摘要,短文本直出),普通图只能静态连接所有节点。

**设计方案**:
- **单路径分支** (`NewChainBranch`): 条件函数返回单个 key
- **多路径分支** (`NewChainMultiBranch`): 条件函数返回 key 集合
- 每个分支节点通过 `AddLambda` 注册
- 编译时转换为内部 GraphBranch 路由

```go
branch := compose.NewChainBranch(func(ctx context.Context, in string) (string, error) {
    if len(in) > 5 { return "long", nil }
    return "short", nil
}).AddLambda("long", longHandler).AddLambda("short", shortHandler)

chain.AppendBranch(branch)
```

---

## 第三章功能 (Runnable Stream / Collect / Transform / Callback 教学示例)

> **注意**: 本章实现了 Runnable 四模式、基础 Pipe stream、Collect/Transform 降级和 CallbackWrapper 教学路径。**组件桥接、图级流式执行、stream field mapping 和流式分支不在当前范围内。**

### composableRunnable 四字段设计

**核心设计**:
`composableRunnable` 维护四组执行函数,支持 `invoke`、`stream`、`collect`、`transform` 四种执行路径:

```go
type composableRunnable struct {
    i func(ctx context.Context, input any) (output any, err error)  // invoke
    s func(ctx context.Context, input any) (output any, err error)  // stream
    c func(ctx context.Context, input any) (output any, err error)  // collect
    t func(ctx context.Context, input any) (output any, err error)  // transform
}
```

**四模式降级机制**: 当目标模式没有原生函数时,`composableRunnable` 会按 Eino 的语义做 fallback。比如 `stream()` 优先用原生 stream,然后依次尝试 transform、invoke、collect。

```go
func (cr *composableRunnable) stream(ctx context.Context, input any) (any, error) {
    if cr.s != nil { return cr.s(ctx, input) }
    if cr.t != nil { return cr.t(ctx, streamFromItems(input)) }
    if cr.i != nil { return streamFromItems(cr.i(ctx, input)) }
    if cr.c != nil { return streamFromItems(cr.c(ctx, streamFromItems(input))) }
    return nil, fmt.Errorf("runnable: Stream not supported")
}
```

### StreamReader — 流式数据接收器

**基础 Pipe stream** 模拟 Eino 的流式数据接收抽象:
- `NewPipe[T](cap)`: 创建 reader/writer
- `PipeStreamReader[T].Recv() (T, bool)`: 接收下一个分块
- `PipeStreamWriter[T].Send(T)`: 发送分块
- `Copy`: 教学版流扇出
- `Merge` / `Concat`: 教学版流扇入和折叠

### Collect — 流式收集模式

将流式分块按序收集为完整结果:
```
StreamReader → Recv(token_1) → Recv(token_2) → ... → Collect(完整结果)
```

Eino 完整版支持多种合并策略 (append/concat/mergeMap),本教育子集演示基础概念。

### Transform — 流式变换模式

对流中每个分块应用变换函数,构建处理管道:
```
StreamReader → Transform(fn) → Collect
```

支持三种变换模式:
- **逐 chunk 变换**: `StreamReader[T] → Transform(fn) → StreamReader[U]`
- **带状态变换**: 函数中维护计数器/滑动窗口/状态机
- **批量变换**: 收集 N 个 chunk 后一次性处理

### CallbackWrapper — 回调生命周期

演示 Eino 的回调生命周期 (OnStart → Execute → OnEnd/OnError),并提供轻量 CallbackWrapper:
- `OnStart`: 记录开始时间和输入
- `OnEnd`: 记录结束时间、输出、耗时
- `OnError`: 记录错误和耗时
- `OnStartWithStreamInput`: 为流输入回调提供副本
- `OnEndWithStreamOutput`: 为流输出回调提供副本

EventLog 在 graph 级别提供等效的可观测性。

---

## 快速示例

```go
package main

import (
    "context"
    "fmt"
    compose "github.com/rive/eino-compose-runtime-replica-go/compose"
)

func main() {
    // 方式一: 使用 Graph (最大灵活性)
    g := compose.NewGraph[string, string]()
    g.AddLambdaNode("upper", compose.InvokableLambda(
        func(ctx context.Context, in string) (string, error) {
            return strings.ToUpper(in), nil
        },
    ))
    g.AddEdge(compose.START, "upper")
    g.AddEdge("upper", compose.END)
    r, _ := g.Compile(context.Background(),
        compose.WithNodeTriggerMode(compose.AllPredecessor),
    )
    result, _ := r.Invoke(context.Background(), "hello")
    fmt.Println(result) // "HELLO"

    // 方式二: 使用 Chain (最便捷)
    chain := compose.NewChain[string, string]()
    chain.
        AppendLambda(compose.InvokableLambda(
            func(ctx context.Context, s string) (string, error) {
                return strings.ToUpper(s), nil
            },
        )).
        AppendLambda(compose.InvokableLambda(
            func(ctx context.Context, s string) (string, error) {
                return "[" + s + "]", nil
            },
        ))
    r2, _ := chain.Compile(context.Background())
    result2, _ := r2.Invoke(context.Background(), "hello")
    fmt.Println(result2) // "[HELLO]"
}
```

## 运行示例

```bash
cd examples/eino-compose-runtime-replica-go
go run ./cmd/example/
```

## 运行测试

```bash
cd examples/eino-compose-runtime-replica-go
go test ./...
```

## 格式化

```bash
cd examples/eino-compose-runtime-replica-go
gofmt -w .
```

## 包结构

```
examples/eino-compose-runtime-replica-go/
├── cmd/example/main.go          # 综合示例 (15 个场景,覆盖 Chapter 1/2/3)
├── compose/
│   ├── types.go                 # NodeTriggerMode, ComponentType, 哨兵错误, START/END
│   ├── runnable.go              # Runnable[I,O], Lambda, composableRunnable
│   ├── graph.go                 # 内部 graph: AddNode, AddEdge, addEdgeWithMappings, compile, Kahn 环检测
│   ├── generic_graph.go         # Graph[I,O] 公开 API, NewGraph, Compile, graphRunnable
│   ├── graph_node.go            # graphNode, 子图递归编译
│   ├── graph_compile.go         # CompileOption: WithNodeTriggerMode, WithMaxRunSteps 等
│   ├── graph_run.go             # runner: 主循环, createTasks, resolveCompletedTasks
│   ├── graph_manager.go         # channel 接口, channelManager, taskManager
│   ├── dag.go                   # dagChannel: AllPredecessor 状态机
│   ├── pregel.go                # pregelChannel: AnyPredecessor 语义
│   ├── branch.go                # GraphBranch: 条件路由
│   ├── field_mapping.go         # FieldMapping, FieldPath, validateFieldMapping, fieldMap, takeOne, assignOne, convertTo
│   ├── workflow.go              # Workflow[I,O], WorkflowNode, WorkflowBranch, AddInput, AddDependency, SetStaticValue
│   ├── chain.go                 # Chain[I,O] Builder: Append*, addNode, preNodeKeys, addEndIfNeeded
│   ├── chain_parallel.go        # Parallel: 并行节点组, outputKey 冲突检测
│   ├── chain_branch.go          # ChainBranch: NewChainBranch, NewChainMultiBranch, AddLambda
│   ├── introspect.go            # GraphInfo, GraphNodeInfo 编译时拓扑导出
│   ├── event_log.go             # EventLog: 10 种事件类型, 线程安全
│   ├── stream.go                # PipeStreamReader/PipeStreamWriter, Copy, Merge, Concat
│   ├── callbacks.go             # RunInfo, Handler, CallbackWrapper, stream callback copies
│   └── utils.go                 # 辅助函数
├── research/
│   ├── ch2-implementation-contract.md  # 第二章实现契约
│   ├── ch2-verification.md             # 第二章完整验证记录
│   └── ch3-runtime-contract.md         # 第三章 Runnable/Stream/Callback 契约
├── README.md                    # 本文档
├── CHANGELOG.md                 # 变更日志
├── FINAL_SUMMARY.md             # 最终验证摘要
└── go.mod
```

## 设计决策

1. **零外部依赖**: 仅依赖 Go 标准库
2. **编译锁模式**: `graph.compiled` 标记阻止编译后变更;同一 graph 可用不同选项多次编译
3. **Channel 多态**: DAG 和 Pregel 共享 `channel` 接口,仅实现不同
4. **Kahn 算法**: DAG 模式使用拓扑排序检测环
5. **Goroutine 池**: taskManager 使用 WaitGroup 并发执行同一步骤内的 task
6. **三层抽象**: Graph → Workflow → Chain,控制力递减,便利性递增

## 明确未实现的边界

**本复刻版是教育子集 (educational subset)。组件桥接 (ChatModel/Tool/Retriever)、完整图流式执行、stream field mapping 和流式分支不在当前范围内。**

本复刻版聚焦于 Eino Compose Runtime 的核心图编译与执行引擎,以下为明确未实现的部分:

### 运行时不支持
- **组件桥接 (ChatModel/Tool/Retriever)**: 当前仅有 Lambda 抽象,可通过 AddLambdaNode 等价替代
- **图级 Stream 执行管线**: Runnable 四模式已经实现,但 graph runner 主路径仍以 Invoke 为主
- **streamFieldMap 流式映射**: 依赖图级 stream channel,当前未接入
- **Stream ChainBranch**: 流式分支暂未接入 Chain Builder
- **组件级 Callback 桥接**: CallbackWrapper 已实现,但未接 ChatModel/Tool 组件体系与图级初始化链
- **State 传递 (graph.state)**: 字段已定义但未使用
- **Checkpoint / Recovery**: 可恢复执行的中断-恢复机制不在范围内
- **Fan-in 智能合并**: 当前 DAG Fan-in 默认输出 map[string]any 或单值直传

### 周边工具未实现
- **可视化 / DOT 导出**: 无 graph 拓扑可视化
- **JSON Schema 校验**: 无编译时 node 输入输出类型的 schema 校验
- **DevOps 工具**: 无 tracing / metrics / profiling 集成

### 类型系统局限
- `fmtType()` 仅覆盖 `string/int/float64/bool` 四种基础类型,其余返回 `"any"`
