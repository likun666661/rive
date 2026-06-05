# Eino Compose Runtime Replica (Go MVP)

受 Eino (CloudWeGo) 启发的第二/三/四/五章骨架示例与验证项目,覆盖核心编译边界与 DAG/Pregel 执行引擎,并实现三层编排抽象 (FieldMapping / Workflow / Chain / Parallel / Branch) + Runnable Stream/Callback 教学示例 + ChatModel/Retriever 组件接口与 Bridge Adapter 模式 + Checkpoint/Interrupt/Resume 教学子集 + PromptTemplate/Tool/ToolsNode 桥接。本项目为学习与研究用途的章节级骨架,非 Eino 的完整产品复刻。

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

第四章: Checkpoint / Interrupt / Resume 教学子集
  ├── Address / AddressSegment 结构化执行点身份
  ├── InterruptSignal 树 + 扁平 InterruptContext 视图
  ├── context 型 CheckPointStore / CheckPointID 恢复入口
  ├── GetInterruptState / GetResumeContext 定向恢复
  └── PipeStreamReader 物化与恢复示例

第五章: PromptTemplate / Tool / ToolsNode 组件桥接
  ├── MessageTemplate (ChatTemplate 接口 + {{variable}} 替换)
  ├── BridgeTool 领域接口 (Name + Execute)
  ├── ToolsNode (ToolCall 解析 → 工具分发 → 结果组装)
  ├── Workflow.AsPromptTemplateNode / AsToolsNode 便捷方法
  └── Graph/Workflow/Chain 三种编排演示
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

---

## 第四章功能 (Checkpoint / Interrupt / Resume)

### 核心问题

图运行时不是单个函数调用,而是可嵌套、可并发、可流式的执行网络。某个节点或工具需要暂停时,运行时必须保存“到底卡在哪个结构化执行点”,并在恢复时把数据送回同一个地址,否则会重复执行已完成副作用或把用户输入路由到错误节点。

### 教学子集设计

- **Address**: `AddressSegment{Type, ID, SubID}` 组成稳定分层地址,例如 `runnable:root;node:approval;tool:lookup:call_1`。
- **InterruptSignal**: 支持树状 `Subs`,便于表达批量工具/子图里的多个 root cause；面向用户暴露扁平 `InterruptContext`。
- **Checkpoint**: `CheckPointStore` 保存原始输入与 `interruptID -> Address/State` 映射；当前示例使用 `InMemoryCheckPointStore`。
- **Resume**: `ResumeWithData` / `BatchResumeWithData` 注入恢复数据；`GetResumeContext` 区分“我是直接目标”和“我是通往后代目标的 conduit”。
- **Graph runner 集成**: 节点执行前自动追加 `node:<key>` 地址段；节点返回 interrupt 时 runner 保存 checkpoint,调用方可用同一个 checkpoint ID 恢复。
- **Stream materialization**: `MaterializeStream` / `RestoreStream` 演示在 checkpoint 边界把一次性 `PipeStreamReader` 转成持久值再还原。

```go
store := compose.NewInMemoryCheckPointStore()
ctx := compose.WithCheckPoint(context.Background(), "cp1", store)

_, err := graph.Invoke(ctx, "draft")
info, _ := compose.ExtractInterruptInfo(err)
id := info.InterruptContexts[0].ID

resumeCtx := compose.ResumeWithData(
    compose.WithCheckPoint(context.Background(), "cp1", store),
    id,
    "approved",
)
out, err := graph.Invoke(resumeCtx, "")
```

### 边界

这不是完整 Eino checkpoint 实现。当前教育子集不持久化 channel manager 全量状态、不做子图 checkpoint 转发、不做序列化注册/迁移、不支持工具 rerun skip handler。它重点复刻地址、信号树、checkpoint store 和定向 resume 这四个核心模式。

---

## I3 Bridge Adapter 模式: 领域组件与通用图运行时桥接 (ChatModel + Retriever)

> **本章讲解 bridge adapter 设计模式,展示如何让 Retriever / ChatModel 等非图原生组件参与通用图运行时,并以 RAG pipeline 为教学示例。**

### 核心问题

Graph/Workflow/Chain 运行时的基本单位是 `Lambda` (`composableRunnable`),但领域组件有其自身的接口约定:
- `Retriever` 关心 `Retrieve(ctx, query) ([]Document, error)`
- `ChatModel` 关心 `Generate(ctx, messages) (string, error)`

直接将这些组件硬编码进图运行时会破坏接口隔离,让组件与 graph runtime 相互耦合。

### 解决方案: Bridge Adapter 模式

```
领域层 (Domain)             桥接层 (Bridge)          运行时 (Runtime)
┌──────────────┐          ┌──────────────┐          ┌──────────────────┐
│ Retriever    │──bridge──│ toLambda()   │──Lambda──│ Graph[I,O]       │
│ .Retrieve()  │          │              │          │  .AddLambdaNode  │
└──────────────┘          └──────────────┘          │  .AddEdge        │
                                                     │  .Compile()      │
┌──────────────┐          ┌──────────────┐          │  .Invoke()       │
│ ChatModel    │──bridge──│ toLambda()   │──Lambda──│                  │
│ .Generate()  │          │              │          │  Workflow[I,O]   │
└──────────────┘          └──────────────┘          │  .AsRetrieverNode│
                                                     │  .AsChatModelNode│
┌──────────────┐          ┌──────────────┐          │  .AddInput()     │
│ Tool         │──bridge──│ toLambda()   │──Lambda──│                  │
│ .Execute()   │          │              │          │  Chain[I,O]      │
└──────────────┘          └──────────────┘          │  .AppendLambda   │
                                                     └──────────────────┘
```

### 桥接适配原理

1. **统一合约 (Lambda)**: Graph/Workflow/Chain 只认 Lambda 作为可执行单元。Bridge 将任何一个实现领域接口的结构体包装成 Lambda,无须修改图运行时。

2. **接口隔离**: 领域组件定义自己的接口,实现者只需关心领域逻辑,不依赖 graph/compose 包的类型系统。

3. **零侵入**: bridge 函数是纯适配逻辑,不修改组件自身,不污染图运行时。新增领域组件类型只需添加一个 bridge + 接口。

4. **FieldMapping 衔接**: 不同组件输入输出类型不同 (`string → []*Document → []*Message → string`)。FieldMapping 在 bridge 节点之间做字段提取、转换、注入。

5. **三重抽象复用**: 同一套 Bridge Lambda 可用于 Graph / Workflow / Chain 三种编排抽象。

### RAG Pipeline 示例

```go
// 定义领域组件 (仅实现接口,无须感知图运行时)
type MyRetriever struct{}
func (r *MyRetriever) Retrieve(ctx context.Context, query string) ([]*compose.Document, error) {
    return fetchDocs(query)
}

type MyChatModel struct{}
func (m *MyChatModel) Generate(ctx context.Context, msgs []*compose.Message) (string, error) {
    return callLLM(msgs)
}

// 用 Workflow + Bridge 编排 RAG pipeline
wf := compose.NewWorkflow[*RAGInput, map[string]any]()

wf.AsRetrieverNode("retriever", &MyRetriever{}).
    AddInput(compose.START, compose.FromField("Query"))

wf.AsPromptAssemblerNode("assemble", systemPrompt).
    AddInput(compose.START, compose.FromField("Query")).
    AddInput("retriever", compose.ToField("documents"))

wf.AsChatModelNode("model", &MyChatModel{}).
    AddInput("assemble")

wf.End().
    AddInput("model", compose.ToField("answer")).
    AddInput(compose.START, compose.MapFields("Query", "original_query"))

r, _ := wf.Compile(ctx)
result, _ := r.Invoke(ctx, &RAGInput{Query: "What is Rive?"})
// result["answer"] contains the generated response
// result["original_query"] retains the user query
```

### 便捷方法对照

| Bridge 方法 | 等效 Graph 调用 | 说明 |
|---|---|---|---|
| `wf.AsRetrieverNode(key, retriever)` | `wf.AddLambdaNode(key, bridge.toLambda())` | 将 Retriever 桥接为 Lambda 节点 |
| `wf.AsChatModelNode(key, model)` | `wf.AddLambdaNode(key, bridge.toLambda())` | 将 ChatModel 桥接为 Lambda 节点 |
| `wf.AsPromptAssemblerNode(key, prompt)` | `wf.AddLambdaNode(key, bridge.toLambda())` | 创建提示词组装 Lambda 节点 |
| `wf.AsPromptTemplateNode(key, tmpl)` | `wf.AddLambdaNode(key, NewPromptTemplateLambda(tmpl))` | 将 MessageTemplate 桥接为 Lambda 节点 (输出 []*Message) |
| `wf.AsToolsNode(key, tools...)` | `wf.AddLambdaNode(key, NewToolsNodeLambda(tools...))` | 将 BridgeTool 集合桥接为 ToolsNode Lambda |

### 扩展清单 (本教育子集未实现)

- **StreamChatModel bridge**: `GenerateStream()` → `StreamableLambda`
- **Embedding bridge**: `Embedder.Embed()` → Lambda
- **完整的错误传递与重试语义** (callback + state 集成)

> **Tool bridge 已实现**: `compose/prompt_tool_bridge.go` 提供 `NewToolsNodeLambda` / `Workflow.AsToolsNode` 等方法,支持将 `BridgeTool` 包装为 Lambda,解析 `ToolCall` 并分发执行。详见下方"第五章: PromptTemplate / Tool / ToolsNode"章节。

---

## 第五章功能 (PromptTemplate / Tool / ToolsNode 组件桥接)

> **本章扩展 I3 Bridge Adapter 模式,增加 PromptTemplate 渲染、Tool 接口与 ToolsNode 工具执行节点,实现完整的 Tool Calling Pipeline。所有示例均确定性 (无外部模型调用),适合教学演示。**

### 核心概念

```
PromptTemplate → ChatModel (返回 ToolCall) → ToolsNode → ChatModel (生成回答)
```

**Tool Calling Pipeline** 是 LLM 应用的标准模式:
1. **PromptTemplate**: 将用户输入与系统提示词组装为 `[]*Message`
2. **ChatModel**: 根据提示词决定调用哪个工具,返回带 `ToolCalls` 的 `Message`
3. **ToolsNode**: 解析 `ToolCalls`,匹配已注册的 `BridgeTool`,执行工具并返回结果
4. **ChatModel**: 基于工具结果生成最终回答

### 数据模型 (schema.go)

```go
type ToolCall struct {
    ID       string
    Type     string
    Function ToolCallFunction    // {Name, Arguments}
}

type ToolInfo struct {
    Name, Desc string
    ParamsOneOf *ParamsOneOf     // 参数 Schema
}

type ToolResult struct {
    Text string
}
```

`Message` 结构体 (`chatmodel.go`) 扩展了 `ToolCalls []ToolCall` 字段,用于在模型节点与工具节点之间传递工具调用意图。

### PromptTemplate — ChatTemplate 接口与 MessageTemplate

**解决的问题**:
ChatModel 的输入是 `[]*Message`,但用户输入通常是原始文本或结构体。每次手动构造 `Message` 对象繁琐且容易出错。

**设计方案**:
- `ChatTemplate` 接口: `Format(ctx, vs map[string]any) ([]*Message, error)`
- `MessageTemplate`: 支持 `{{variable}}` 占位符替换,可选的系统提示词模板
- `ChatTemplateComponent`: 将 ChatTemplate 包装为 `composableRunnable`,用于图运行时

```go
tmpl := compose.NewMessageTemplate("{{query}}").
    WithSystemTemplate("You are a helpful assistant.")

msgs, _ := tmpl.Format(ctx, map[string]any{"query": "What is the weather?"})
// 输出: [System("You are a helpful assistant."), Human("What is the weather?")]
```

### Tool — BridgeTool 领域接口

**解决的问题**:
工具 (如天气查询、计算器) 有自己的领域逻辑和接口,不能直接作为图运行时节点。

**设计方案**:
- `BridgeTool` 接口: `Name() string` + `Execute(ctx, args map[string]any) (string, error)`
- `BridgeToolFunc` / `NewBridgeTool`: 将普通函数包装为 `BridgeTool`
- 工具注册后由 `toolsNodeBridge` 按名称分发调用

```go
getWeather := compose.NewBridgeTool("get_weather",
    func(ctx context.Context, args map[string]any) (string, error) {
        loc, _ := args["location"].(string)
        return fmt.Sprintf("Sunny, 22°C in %s", loc), nil
    },
)
```

### ToolsNode — 工具执行节点

**解决的问题**:
ChatModel 返回的 `ToolCalls` 需要被解析、分配到正确的工具、收集结果,并返回给模型。这个过程需要与图运行时集成。

**设计方案**:
- `toolsNodeBridge`: 内部维护 `tools map[string]BridgeTool`
- 输入 `*Message` → 解析 `ToolCalls` → 匹配工具 → 执行 → 组装结果 `*Message`
- `NewToolsNodeLambda(tools...)`: 导出构造函数,将工具集包装为 Lambda
- `Workflow.AsToolsNode(key, tools...)`: Workflow 便捷方法

```go
tools := compose.NewToolsNodeLambda(getWeather, calcTool)
// 输入: *Message{ToolCalls: [{Function: {Name: "get_weather", Arguments: '{"location":"Paris"}'}}]}
// 输出: *Message{Content: "Tool results:\n- get_weather({location:Paris}): Sunny, 22°C in Paris\n"}
```

### 三种编排演示

以下是用 Workflow、Chain、Graph 三种抽象编排 Tool Calling Pipeline 的示例:

#### Workflow 版本 (声明式,最简洁)

```go
wf := compose.NewWorkflow[map[string]any, *compose.Message]()
wf.AsPromptTemplateNode("prompt", tmpl).AddInput(compose.START)
wf.AddLambdaNode("model1", model1Fn).AddInput("prompt")
wf.AsToolsNode("tools", getWeather).AddInput("model1")
wf.AddLambdaNode("model2", model2Fn).AddInput("tools")
wf.End().AddInput("model2")
r, _ := wf.Compile(ctx)
result, _ := r.Invoke(ctx, map[string]any{"query": "What is the weather in Paris?"})
// result.Content: "Final answer based on tool results:\nTool results:\n- get_weather(...): Sunny, 22°C in Paris\n"
```

#### Chain 版本 (Builder 风格,自动连接)

```go
chain := compose.NewChain[[]*compose.Message, *compose.Message]()
chain.
    AppendLambda(model1Fn).
    AppendLambda(compose.NewToolsNodeLambda(calcTool)).
    AppendLambda(model2Fn)
r, _ := chain.Compile(ctx)
result, _ := r.Invoke(ctx, []*compose.Message{compose.HumanMessage("What is 2+2?")})
// result.Content: "Answer: The tool computed → Computed result for '2+2' = 42"
```

#### Graph 版本 (最大灵活性,手动拓扑)

```go
g := compose.NewGraph[[]*compose.Message, *compose.Message]()
g.AddLambdaNode("model1", model1Fn)
g.AddLambdaNode("tools", compose.NewToolsNodeLambda(weatherTool))
g.AddLambdaNode("model2", model2Fn)
g.AddEdge(compose.START, "model1")
g.AddEdge("model1", "tools")
g.AddEdge("tools", "model2")
g.AddEdge("model2", compose.END)
r, _ := g.Compile(ctx, compose.WithNodeTriggerMode(compose.AllPredecessor))
result, _ := r.Invoke(ctx, []*compose.Message{compose.HumanMessage("Weather in Tokyo?")})
// result.Content: "Summary: Tool results:\n- get_weather(...): Cloudy, 18°C in Tokyo\n"
```

### 便捷方法对照

| Bridge 方法 | 等效 Graph 调用 | 说明 |
|---|---|---|
| `wf.AsPromptTemplateNode(key, tmpl)` | `wf.AddLambdaNode(key, NewPromptTemplateLambda(tmpl))` | 将 MessageTemplate 桥接为 Lambda (map → []*Message) |
| `wf.AsToolsNode(key, tools...)` | `wf.AddLambdaNode(key, NewToolsNodeLambda(tools...))` | 将 BridgeTool 集合桥接为 ToolsNode Lambda |

### 边界

本教育子集实现的 Tool Calling Pipeline 不包含:
- Eino 完整 `InvokableTool` / `ToolCallingChatModel` 分层接口
- Streaming tool call (模型边生成边返回 ToolCall)
- Tool rerun skip handler (中断恢复后跳过已执行工具)
- 工具级 Callback 集成
- Provider 特定的工具绑定选项

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
go test ./... -count=1
```

## 格式化 + 静态分析

```bash
cd examples/eino-compose-runtime-replica-go
gofmt -w .
go vet ./...
```

## 空白字符检查

```bash
# 在仓库根目录执行
git diff --check
```

## 包结构

```
examples/eino-compose-runtime-replica-go/
├── cmd/example/main.go          # 综合示例 (20 个场景,覆盖 Chapter 1/2/3/4/5)
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
│   ├── schema.go                # ToolCall / ToolCallFunction / ToolInfo / ToolResult 数据模型
│   ├── prompt.go                # ChatTemplate 接口, MessageTemplate ({{variable}} 替换), ChatTemplateComponent
│   ├── prompt_tool_bridge.go    # I3 Tool Bridge: BridgeTool 接口 + promptTemplateBridge + toolsNodeBridge + Workflow 便捷方法
│   ├── bridge.go                # Bridge Adapter: BridgeRetriever/BridgeChatModel + Workflow 便捷方法
│   ├── retriever.go             # Retriever 接口, Document/Query 类型, FakeRetriever, NewRetrieverLambda
│   ├── chatmodel.go             # ChatModel 接口, Message/RoleType, FakeChatModel, ChatModelComponent
│   └── utils.go                 # 辅助函数
├── research/
│   ├── ch2-implementation-contract.md  # 第二章实现契约
│   ├── ch2-verification.md             # 第二章完整验证记录
│   ├── ch3-runtime-contract.md         # 第三章 Runnable/Stream/Callback 契约
│   ├── ch4-r1-chatmodel-retriever-contract.md  # 第四章组件契约研究
│   ├── ch4-r2-replica-bridge-audit.md  # 第四章桥接审计
│   ├── ch5-implementation-contract.md  # 第五章实现契约: Model/Tool/Prompt 组件接口与图桥接
│   └── ch5-r1-component-gap-audit.md   # 第五章组件差距审计
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

**本复刻版是教育子集 (educational subset)。ChatModel/Retriever 组件接口已实现 (Chapter 4),Bridge Adapter 模式已演示 (I3),PromptTemplate/Tool/ToolsNode 桥接已实现 (Chapter 5)。完整图流式执行、stream field mapping、Workflow 分支运行时路由仍为已知缺口。**

本复刻版聚焦于 Eino Compose Runtime 的核心图编译与执行引擎,以下为明确未实现的部分:

### 运行时不支持
- **组件桥接 (ChatModel/Tool/Retriever/Prompt)**: ChatModel/Retriever 接口已实现 (retriever.go/chatmodel.go)。Tool bridge 已实现 (prompt_tool_bridge.go)。Prompt bridge 已实现 (prompt.go)。Bridge Adapter 模式已展示 Workflow 声明式桥接。
- **图级 Stream 执行管线**: Runnable 四模式已经实现,但 graph runner 主路径仍以 Invoke 为主
- **streamFieldMap 流式映射**: 依赖图级 stream channel,当前未接入
- **Stream ChainBranch**: 流式分支暂未接入 Chain Builder
- **validateFieldMapping 编译时调用缺失 (GAP-I1-1)**: `validateFieldMapping()` 已完整实现但 `graph.compile()` 未调用
- **GraphBranch 运行时路由缺失 (GAP-I1-2)**: Workflow 分支不可用;Chain 通过内联绕过
- **State 传递 (graph.state)**: 字段已定义但未使用
- **持久化/分布式 Checkpoint Store**: Checkpoint / Interrupt / Resume 已有内存教学实现,但未接持久化后端或分布式恢复
- **Fan-in 智能合并**: 当前 DAG Fan-in 默认输出 map[string]any 或单值直传

### 周边工具未实现
- **可视化 / DOT 导出**: 无 graph 拓扑可视化
- **JSON Schema 校验**: 无编译时 node 输入输出类型的 schema 校验
- **DevOps 工具**: 无 tracing / metrics / profiling 集成

### 类型系统局限
- `fmtType()` 仅覆盖 `string/int/float64/bool` 四种基础类型,其余返回 `"any"`
