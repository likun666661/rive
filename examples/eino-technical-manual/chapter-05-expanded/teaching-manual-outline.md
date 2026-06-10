# Eino Go 复刻版教学手册细纲

> 合成日期：2026-06-07  
> 输入 artifact：7 份 chapter validator artifact + 上轮总纲 `final-eino-replica-design-samuel-reviewed.md`  
> 复刻工程：`examples/eino-compose-runtime-replica-go/`  
> **定位：教学材料，不是生产级 Eino 替代品。文中多处标注简化边界。**

---

## 0. 教学路线图（90–120 分钟）

| 段 | 章节 | 主题 | 建议时长 | 累积 | 转场逻辑 |
|----|------|------|----------|------|----------|
| 1 | Ch01 | Compose Graph Runtime | 18 min | 18 | 开场：建立编译边界核心概念，通过 3 个业务故事立 flag |
| 2 | Ch02 | Workflow / Chain / FieldMapping | 18 min | 36 | 从 Graph 到声明式编排——"既然 Graph 要手写边，怎么更声明式地组装业务？" |
| 3 | Ch03 | Runnable / Stream / Callback | 18 min | 54 | 从编译器到运行器——"编译完的图怎么执行？怎么观测？怎么处理流？" |
| 4 | Ch04 | Checkpoint / Interrupt / Resume | 14 min | 68 | 从一次性执行到可恢复执行——"Agent 跑一半用户需要点确认怎么办？" |
| 5 | Ch05 | Components / Model / Tool / Prompt | 14 min | 82 | 从通用图到领域组件——"ChatModel 不是 Lambda，怎么让它进 Graph？" |
| 6 | Ch06 | Schema / Provider Adapter | 12 min | 94 | 从一个 Provider 到多个——"OpenAI 和 Claude 的消息格式不一样，Graph 怎么不感知？" |
| 7 | Ch07 | Agent Flow / ReAct / MultiAgent | 12 min | 106 | 收官——"前面 6 章全为了这一章：ReAct 不是魔法，是组合" |
| — | Q&A | 自由问答 + 总结 thesis | 14 min | 120 | 开放讨论，回顾 6 条关键 thesis |

**主线串联**：Graph → Workflow/FieldMapping → Runnable/Stream/Callback → Checkpoint/Resume → Components → Schema/Provider → ReAct

**每个转场只需一句话**，例如：
- Ch01→Ch02："有了 Graph，下一步是让声明式 Workflow 和链式 Chain 也同样能编译成 Runnable。"
- Ch02→Ch03："编译完了，图在运行时怎么执行？不同组件能力不同怎么自动适配？"
- Ch03→Ch04："执行过程中，模型可能说'需要人类确认'——怎么暂停、恢复？"
- Ch04→Ch05："前四章都在说 Graph 本身，现在把真实的 ChatModel、Retriever 放进来。"
- Ch05→Ch06："ChatModel 桥接了，但 OpenAI 和 Claude 的消息格式不一样——用 Schema 做防火墙。"
- Ch06→Ch07："最后一步：把 ChatModel + Tools + Branch + State 组合起来，就是 ReAct Agent。"

### 如果现场只有 30 分钟怎么压缩

1. **砍掉 Live Coding，只做白板 + 投屏走读**（省 15 min）。
2. **砍掉练习题演示，口述题目即可**（省 10 min）。
3. **Ch02 和 Ch05 各压缩到 5 分钟**——只讲三层编排对比表（Ch02）和 `toLambda()` 桥接模式一句话（Ch05）。
4. **Ch04 压缩到 5 分钟**——只讲 `AddressSegment` 概念 + `InterruptError → ResumeWithData` 的一个故事。
5. **Ch06 压缩到 3 分钟**——只展示 `Message` vs `AgenticMessage` 两种规范的对比表。
6. **Ch07 压缩到 2 分钟**——一句话："ReAct = Graph(ChatModel + Tools + Branch + State)"，交给课后自学。

30 分钟不可求全，核心是在听众脑中植入 **"编译边界 / 桥接模式 / ReAct 是组合"** 三个概念。剩下的章节在附录里提供代码索引和练习题，自学即可。

---

## Chapter 01 — Compose Graph Runtime（18 min）

> 复刻代码目录：`compose/` | 核心文件：`graph.go`, `graph_run.go`, `dag.go`, `pregel.go`  
> 原版手册对应：`manual/01-compose-graph-runtime.md`

### 讲解目标

学完本章，听众应能：
1. 口头解释为什么 LLM 应用需要"图编译 + 运行时分离"，而不是直接用函数调用串联。
2. 画出一张 DAG 图并说明 `dagChannel` 和 `pregelChannel` 的触发条件差异。
3. 写出最简 `NewGraph → AddLambdaNode → AddEdge → Compile → Invoke` 代码。
4. 判断"编译后改图"和"DAG 模式下写环"两个反例会报什么错。

### 问题背景

**三条业务故事**：
- **RAG Pipeline**：用户问题 → Prompt Template → ChatModel → 输出。加 Retriever 后控制流分叉，手写在业务代码里改动成本高。
- **ReAct Agent**：模型输出 → 检查 ToolCall → 调工具 → 回模型 → 再判断，循环次数不定。
- **多 Agent 协作**：Host Model 决策调用哪个 Specialist → 执行 → 汇总 → 返回，需要嵌套子图。

**不用框架的四个痛点**：拓扑无校验、运行模式散落、横切关注点缺失、组件接口不统一。

### 为什么难

- 同一个图既要支持 DAG（确定性线性拓扑），又要支持 Pregel（循环/迭代），而且两种语义统一在同一套 channel 接口多态之下。
- 编译边界的设计需要预计算所有依赖关系（`chanSubscribeTo`、`dataPredecessors`、`controlPredecessors`），同时保持编译后的不可变性。

### 核心抽象

```
Graph ──Compile()──→ Runner ──Invoke/Stream──→ Runnable
(可修改拓扑)         (预计算 channel 依赖)        (只暴露执行)
```

- **编译不是优化，是冻结**：所有不确定的（谁依赖谁、谁在谁后面、合并配置）都在编译期一次性算完，运行时只做机械执行。
- **DAG vs Pregel 区别**：
  - `dagChannel`（`dag.go:24`）：所有 control 前置 Ready + 所有 data 前置上报 → barrier 触发。get() 后重置 data 追踪，保留 control 状态。
  - `pregelChannel`（`pregel.go:3`）：任一 data 前置上报 → 立即触发。get() 后清空值。
- **三层编排最终收敛到 Runnable**：`Graph` / `Chain` / `Workflow` 编译后都产出 `Runnable[I,O]`。

### 复刻版代码走读

推荐阅读顺序：
1. `types.go` — START/END、NodeTriggerMode、defaultMaxSteps
2. `graph_node.go` — graphNode + compileIfNeeded
3. `graph_compile.go` — CompileOption 函数式选项
4. `graph.go` — graph struct (L8)、compile() (L205)、checkDAGCycles() (L408)
5. `generic_graph.go` — NewGraph、Compile、graphRunnable
6. `graph_manager.go` — channel 接口、chanCall、channelManager、taskManager
7. `graph_run.go` — runner.run() 主循环 (L28)、resolveCompletedTasks (L186)
8. `dag.go` — dagChannel.get() barrier (L75)
9. `pregel.go` — pregelChannel.get() fire-on-any (L25)

**最小可运行示例**（已验证）：
```go
g := compose.NewGraph[string, string]()
g.AddLambdaNode("upper", compose.InvokableLambda(
    func(ctx context.Context, s string) (string, error) {
        return strings.ToUpper(s), nil
    }))
g.AddLambdaNode("prefix", compose.InvokableLambda(
    func(ctx context.Context, s string) (string, error) {
        return "[PREFIX] " + s, nil
    }))
g.AddEdge(compose.START, "upper")
g.AddEdge("upper", "prefix")
g.AddEdge("prefix", compose.END)
r, _ := g.Compile(ctx, compose.WithGraphName("demo"))
out, _ := r.Invoke(ctx, "hello")  // "[PREFIX] HELLO"
```

### 演示建议

1. Live coding 从零写 3 节点 Graph，编译运行（3 min）。
2. 加 Branch（条件分支），展示 `branch.condition` 路由（2 min）。
3. 切换到 `WithNodeTriggerMode(compose.AllPredecessor)`，对比行为（1 min）。
4. 故意在 DAG 模式下写环 `A→B, B→A`，看 `ErrDAGHasCycle`（1 min）。

### 容易误解点

1. **"编译就是预计算依赖"** → 不止，还做了递归编译子图、DAG 无环校验、模式决策、设置编译锁。
2. **"DAG 和 Pregel 只是配置不同"** → 核心差异在 channel 实现。`dagChannel` 维护 control 状态机，`pregelChannel` 几乎无状态。
3. **"Control Edge 和 Data Edge 是同一回事"** → `AddEdge` 同时建 data + 隐含 control；`AddControlEdge` 只控制顺序不传数据。
4. **"编译后的 Runnable 还有 Graph 信息"** → 接口只暴露 Invoke/Stream/Collect/Transform，拓扑快照需通过 `GraphInfo` 获取。
5. **"dagChannel.get() 后保留值"** → **错误**。get() 后两个 channel 都清空值。dagChannel 的区别在于保留 controlPredecessors 在 Ready 状态。
6. **"Eager Execution 只影响性能"** → 有语义影响：goroutine 并发下 state 修改顺序不确定。
7. **"子图编译后可以独立运行"** → 复刻版简化了 address/checkpoint 传递，子图的 callback/state 需在父图 context 中工作。

### 练习题

- **Q1**：写一个 3 节点 Graph，A 是 InvokableLambda，B 是 StreamableLambda，C 是 CollectableLambda。编译为 DAG 并 Invoke。
- **Q2**：同一 Graph 编译两次（一次 DAG，一次 Pregel），r1 和 r2 是同一个实例吗？
- **Q3**：编译后添加节点会报什么错误？（`ErrGraphCompiled`）
- **Q4**：在 DAG 模式下，`resolveCompletedTasks` 如何处理 branch 输出？
- **Q5**：Data Edge vs Control Edge 在 Pregel 下行为差异？（A 中 B 的输入来自 A；B 中 B 的输入不来自 A）
- **Q6（思考）**：原版 Eino 的 `toValidateMap` 链式类型推断，复刻版没有。什么场景会暴露？
- **Q7（设计）**：如果要在 runner 主循环加入 "interrupt after node X"，需要在哪里加检查？

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 对应章节 | 建议讲解点 | 测试文件 |
|------|-------------|------|----------|-----------|---------|
| `compose/types.go` | `NodeTriggerMode`, `START`, `END`, `defaultMaxSteps`, `ErrGraphCompiled`, `ErrDAGHasCycle` | 34,62,67,71,74 | Ch01 | 全局常量/枚举定义 | `graph_test.go` |
| `compose/graph.go` | `graph`, `checkCompiled()`, `AddEdge()`, `AddControlEdge()`, `AddBranch()`, `compile()`, `checkDAGCycles()` | 8,49,76,93,110,205,408 | Ch01 | **编译边界入口** + Kahn 算法 | `graph_test.go` |
| `compose/graph_node.go` | `graphNode`, `compileIfNeeded()` | 5,13 | Ch01 | 节点三形态：cr/g/info | `graph_test.go` |
| `compose/graph_compile.go` | `graphCompileOptions`, `WithGraphName`, `WithNodeTriggerMode`, `WithMaxRunSteps` | 8,25-58 | Ch01 | 函数式编译选项 | `graph_test.go` |
| `compose/generic_graph.go` | `Graph[I,O]`, `NewGraph()`, `Compile()`, `graphRunnable` | 8,14,74,120 | Ch01 | 泛型公开 API | `graph_test.go` |
| `compose/graph_manager.go` | `channel` 接口, `chanCall`, `channelManager`, `taskManager` | 9,17,29,92,102 | Ch01 | DAG/Pregel 多态基础 | `graph_test.go` |
| `compose/graph_run.go` | `runner` struct, `run()` 主循环, `resolveCompletedTasks` | 8,28,186 | Ch01 | **运行时主循环** | `graph_test.go` |
| `compose/dag.go` | `dagChannel`, `reportValues()`, `reportDependency()`, `reportSkip()`, `get()` | 3,24,48,55,61,75 | Ch01 | **AllPredecessor barrier** | `graph_test.go` |
| `compose/pregel.go` | `pregelChannel`, `reportValues()`, `get()` | 3,14,25 | Ch01 | **AnyPredecessor fire-on-any** | `graph_test.go` |
| `compose/runnable.go` | `Runnable[I,O]`, `composableRunnable`, `Lambda` | 15,103,302 | Ch01/Ch03 | 四模式接口定义 + 降级矩阵 | `runnable_test.go` |
| `compose/branch.go` | `GraphBranch`, `NewGraphBranch()` | 7,15 | Ch01/Ch02 | 条件分支结构 | `graph_test.go` |
| `compose/introspect.go` | `GraphInfo`, `GraphNodeInfo` | 3,16 | Ch01 | 编译后拓扑快照 | `graph_test.go` |

**关键测试**：`TestDAGLinearExecution`(1804), `TestDAGFanIn`(288), `TestDAGCycleRejection`(392), `TestPregelCycleAllowed`(443), `TestMaxStepsExceeded`(469), `TestCompileLockAddEdge`(137), `TestGraphBranch`(965), `TestRecompileWithDifferentOptions`(816), `TestConcurrentGraphInvokes`(1430).

---

## Chapter 02 — Workflow / Chain / FieldMapping（18 min）

> 复刻代码目录：`compose/` | 核心文件：`workflow.go`, `chain.go`, `field_mapping.go`  
> 原版手册对应：`manual/02-workflow-chain-field-mapping.md`

### 讲解目标

1. 理解三层编排抽象（Graph / Workflow / Chain）的控制力 vs 便利性坐标。
2. 区分执行依赖和数据映射两种正交概念：`AddInput` vs `AddDependency` vs `WithNoDirectDependency`。
3. 理解 `FieldMapping` 的编译时类型检查（`checkAssignable` 三级判等）和运行时字段提取（`fieldMap`）。
4. 理解 `Chain` 的 `preNodeKeys` 算法如何自动追踪尾部节点。

### 问题背景

**四个场景**：
1. **类型不兼容**：ChatModel 输出 `*Message`，下游 Lambda 输入 `struct{Content string}` → 需要字段映射。
2. **多前驱汇聚**：A 提供 `modelName`，B 提供 `userProfile`，下游需要同时取两个字段 → 后到的覆盖先来。
3. **控制依赖 ≠ 数据依赖**：AuditNode 需要 START 的 `user_id` + 等待 SetupNode 完成 → 用普通 `AddEdge` 无法分离。
4. **样板代码**：3 步 Pipeline 每次都要 `NewGraph + AddNode + AddEdge`。

### 为什么难

- 来自四重设计约束：
  - **接口粒度**：声明式 Workflow 和 Builder Chain 需要在"便利性"和"控制力"之间找平衡点。
  - **依赖语义**：数据流和执行顺序是正交的，混为一体会导致"有数据但没到执行时间"或"到了执行时间但没有数据"。
  - **类型桥接**：`FieldMapping` 的 `checkAndExtractFieldType` 遇到 `interface{}` 中间类型只能"推迟到请求时检查"——编译通过不意味着运行安全。
  - **延迟求值**：Workflow 的分支需要先注册，`addInputs` 不能立即执行，否则引用的目标节点可能尚未注册。

### 核心抽象

```
控制力 ←─── Graph ───── Workflow ───── Chain ───→ 便利性
(手动 AddEdge)  (AddInput + FieldMapping)   (AppendX builder)
```

**三者编译后都是 `Runnable[I,O]`**，运行时无区别。

- **Workflow**：三种依赖类型 `normalDependency(1)` / `noDirectDependency(2)` / `branchDependency(3)`，通过 `addDependencyRelation` 延迟闭包统一处理。
- **FieldMapping**：六种构造函数 + `\x1F` 分隔的嵌套路径；`checkAssignable` 三级判等（Must/May/MustNot）；运行时 `fieldMap` 按 struct/map/interface 三种分叉提取字段。
- **Chain**：`preNodeKeys` 追踪尾部节点，线性自动连、Parallel 后自动 merge、Branch 后自动 routing。`addEndIfNeeded` 自动连接 END。

### 复刻版代码走读

1. `field_mapping.go` — `FieldPath` (`\x1F` 分隔符 L10)、`FieldMapping`(L30)、六种构造函数(L39-75)、`checkAndExtractFieldType`(L161)、`checkAssignable`(L197)、`validateFieldMapping`(L259)、`fieldMap`(L364)
2. `workflow.go` — `dependencyType`(L8)、`WorkflowNode`/`WorkflowBranch`(L16-28)、`AddInput`/`AddDependency`/`WithNoDirectDependency`(L120-151)、`addDependencyRelation`(L158)、`compile` 四阶段(L254)
3. `chain.go` — `Chain[I,O]`(L13)、`addNodeEdges`(L236)、`AppendParallel`(L60)、`AppendBranch`(L133)
4. `chain_parallel.go` — `Parallel` 结构(L13)、outputKey 防重复(L25)
5. `chain_branch.go` — `ChainBranch`(L12)、单分支 vs 多分支输出差异

### 演示建议

1. 现场写 4 个场景的反例代码，展示痛点（5 min）。
2. 画三层坐标轴，一句话对比表（3 min）。
3. 走读 `workflow.go` 的三种依赖声明，白板画 DAG 标注每条依赖（5 min）。
4. 走读 `chain.go` 的 `preNodeKeys` 算法，演示线性/Parallel/Branch 三种模式（5 min）。

### 容易误解点

1. **`AddInput` ≠ `AddEdge`** — AddInput 同时建 data edge + control edge + 记录 dependencies 表。
2. **`NoDirectDependency` 需要间接路径** — 必须有至少一条通过其他节点的间接执行路径，否则顺序不确定。
3. **FieldMapping 路径冲突** — 两个 `AddInput` 都不带映射 → 第二个报错（整个输入被第一个占用）。
4. **Workflow Branch 不自动传数据** — `noDataFlow: true`，分支节点必须显式 `AddInput`。
5. **Chain Branch 后继输出类型不同** — 单分支收到分支节点输出；多分支收到 `map[string]any`。
6. **interface 字段映射编译通过 ≠ 安全** — 推迟到运行时检查，实际类型不是 struct/map 时报错。
7. **Parallel outputKey 不可重复** — `chain_parallel.go` L25-40 检查并设置 `p.err`。

### 练习题

- **Q1**：写 Workflow，从 START 输入 `{Query, ModelName}` 分别提取字段到两个下游。
- **Q2**：Chain 中实现并行 ToUpper + ToLower → 以 `upper`/`lower` key 输出到 map。
- **Q3**：判断 FieldMapping 是否合法：`MapFields("Name","Name")` 但 Input=`struct{A int}` → ❌ 字段不存在。
- **Q4**：设计 Workflow 拓扑：节点 C 需要 A 的数据 + B 的数据，B→C 间接保证 A 顺序。
- **Q5**：Chain 编译后有几条控制边？（思考中间边 + START→chain 首节点 + 尾节点→END）
- **Q6**：Chain Branch 简化版（Lambda 封装）与原版（拓扑层）的区别？什么场景出问题？
- **Q7（设计）**：如果没有 FieldMapping，为 RAG pipeline（2 template + 2 model + aggregator）估算中介 Lambda 数量。

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 对应章节 | 建议讲解点 | 测试文件 |
|------|-------------|------|----------|-----------|---------|
| `compose/field_mapping.go` | `fieldPathSeparator`, `FieldPath`, `FieldMapping`, `MapFields`/`FromField`/`ToField`, `checkAndExtractFieldType`, `checkAssignable`, `validateFieldMapping`, `fieldMap` | 10,14,30,39-75,161,197,259,364 | Ch02 | 编译时类型检查 + 运行时字段提取 | `field_mapping_test.go` |
| `compose/workflow.go` | `dependencyType`, `WorkflowNode`, `WorkflowBranch`, `Workflow[I,O]`, `AddInput`/`AddDependency`/`WithNoDirectDependency`, `addDependencyRelation`, `compile` | 8,16,25,30,120-151,158,254 | Ch02 | 声明式依赖三种模式 + 延迟闭包 | `workflow_test.go` |
| `compose/chain.go` | `Chain[I,O]`, `addNodeEdges`, `AppendParallel`, `AppendBranch`, `Compile`, `addEndIfNeeded` | 13,236,60,133,226,249 | Ch02 | preNodeKeys 自动追踪算法 | `chain_test.go` |
| `compose/chain_parallel.go` | `Parallel` | 13 | Ch02 | outputKey 防重复 | `chain_test.go` |
| `compose/chain_branch.go` | `ChainBranch`, `NewChainBranch`, `NewChainMultiBranch` | 12,19,39 | Ch02 | 单分支 vs 多分支输出差异 | `chain_test.go` |
| `compose/branch.go` | `GraphBranch` | 7 | Ch02 | noDataFlow 字段 | `graph_test.go` |

**复刻版与 Eino 原版差异**：Chain Branch 在复刻版中是 Lambda 封装（原版是拓扑层 branchRouter）；Stream 模式下的 FieldMapping 未实现（仅 Invoke 模式）；Parse 类型转换未实现。

---

## Chapter 03 — Runnable / Stream / Callback（18 min）

> 复刻代码目录：`compose/` | 核心文件：`runnable.go`, `stream.go`, `concat.go`, `callbacks.go`, `event_log.go`  
> 原版手册对应：`manual/03-runnable-stream-callback.md`

### 讲解目标

1. 解释 `Runnable` 四种模式（Invoke/Stream/Collect/Transform）分别对应什么调用场景。
2. 理解 4×4 降级矩阵的优先级逻辑：为什么 `streamByTransform` 优于 `streamByInvoke`？
3. 掌握 `PipeStreamReader` 的生命周期：Copy / Merge / Concat 的语义和时机。
4. 理解 `Callback` 的装饰器模式和 context 隔离规则。
5. 区分 `Callback`（单节点微观）+ `EventLog`（全图宏观）的职责分工。

### 问题背景

**RAG 链中三种数据流模式**：
1. **能力不对称**：Retriever 只有 Invoke，ChatModel 同时有 Invoke 和 Stream。调用方希望 `Stream("query")` 时，Retriever 不支持流。
2. **上下游模式不匹配**：上游输出流，下游需要完整单值 → 谁来做"流折叠成单值"？
3. **可观测性侵入**：想记录节点输入/输出/耗时，但不能在每个组件写 `log.Printf`。而且流式 token 记录需要消耗流 → 日志读完了流，消费者读什么？

### 为什么难

- 降级矩阵有 4×4=16 种路径，但每种目标模式有 4 种优先级。错误地选择 `streamByInvoke` 而不是 `streamByTransform`，会丢失流式低延迟语义——不只是性能问题。
- Stream Copy 需要保证副本独立且 parent 被正确消费。复刻版使用 eager copy（先 drain 再切片拷贝），语义简单但无法处理无限流。
- Callback 的 context 隔离要求每个处理器独立 `handlerCtxs[idx]`，不能串联。流副本需要 `input.Copy(n)` 创建 n 份独立副本。
- `Concat` 需要双注册表：`RegisterConcatFunc`（简单无错拼接）和 `RegisterStreamChunkConcatFunc`（带 error 的复杂拼接，如 ToolCall ID 校验）。

### 核心抽象

**五层结构**：
```
Runnable 四接口 (Invoke/Stream/Collect/Transform)
    ↓
4×4 降级矩阵 (composableRunnable.i/s/c/t 优先级链)
    ↓
Stream 原语 (PipeStreamReader, Copy, Merge, Concat)
    ↓
Callback 装饰器 (Handler 五种时序 + context 隔离 + TimingChecker)
    ↓
EventLog (全图级结构化事件 + JSONL Sink)
```

- **降级矩阵优先级**（已验证）：
  - Invoke: i → S → C → T
  - Stream: s → T → I → C
  - Collect: c → T → I → S
  - Transform: t → S → C → I
- **`Copy` 是 eager**：先 `drainAll` → 切片拷贝 N 份 → parent 已消费。保留数据需 `Copy(parent, N+1)`。
- **`Callback` context 隔离**：处理器的 OnStart 各自收到原始 ctx，修改不影响其他处理器。
- **EventLog 10 种事件类型**：含 `EventCheckpoint`（为 Ch04 铺垫）。

### 复刻版代码走读

1. `compose/runnable.go` — `Runnable`(L14), `composableRunnable`(L103), `invoke()`(L110), `stream()`(L158), `collect()`(L186), `transform()`(L242), `Lambda` 工厂(L311-373)
2. `compose/stream.go` — `NewPipe`(L87), `Copy`(L117), `Merge`(L128), `Concat`(L160), `RegisterConcatFunc`(L155)
3. `compose/concat.go` — `RegisterStreamChunkConcatFunc`(L22), `ConcatItems`(L28), `ConcatMessages`(L61)
4. `compose/callbacks.go` — `Handler`(L97), `CallbackWrapper.Invoke()`(L180), `dispatchOnStartWithStreamInput`(L308), `TimingChecker`(L41)
5. `compose/event_log.go` — `EventType`(L13), `EventLog`(L100), `JSONLEventSink`(L49)

### 演示建议

1. 投屏展示降级表格，走读 `runnable.go:110` 的 4 层 if-else（3 min）。
2. 演示 `Copy(parent, 3)` → 3 个独立 reader，关闭 reader 0 不影响 reader 1（2 min）。
3. 演示 `CallbackWrapper.Invoke()` 装饰器流程：OnStart → 执行 → OnEnd（2 min）。
4. 对比 Callback 和 EventLog 的职责分工表（1 min）。

### 容易误解点

1. **Copy 后 parent 已消费** → eager copy。想保留数据需多 Copy 一份。
2. **回调处理器 context 不是全局串联** → 每个 handler 独立 `handlerCtxs[idx]`。
3. **降级优先级不只是性能问题** → `streamByInvoke` 完全失去流式低延迟。
4. **复刻版 `collectByStream` 无 concat fallback 数据丢失风险** → 使用 `collected()` 返回完整数组（原版 Eino 用 `concatStreamReader`）。
5. **Merge 不保证输入顺序** → 并发扇入，谁先到谁先写。
6. **Concat 不是列出所有数据** → 调用 concat 函数折叠为单个值。
7. **`RegisterConcatFunc` 和 `RegisterStreamChunkConcatFunc` 不是同一回事** → 签名不同（有无 error），写入不同注册表。

### 练习题

- **Q1**：组件只实现 Stream，用户调 Invoke → 画完整调用链。
- **Q2**：`NewPipe[int](2)` → Send(1), Send(2), Close → `drainAll` → len 是多少？
- **Q3**：只实现 Collect vs 只实现 Transform，上游输出流，用户调 Invoke 分别走哪条路径？哪种更高效？
- **Q4**：如何让 handler2 读到 handler1 设置的值？→ 不能通过 context，需要通过共享安全变量如 `sync.Map`。
- **Q5**：处理器 OnEndWithStreamOutput 只读第一个 token，复刻版会泄漏吗？→ 不会（`CbStreamReader` 是内存 slice）。
- **Q6（设计）**：如何实现 lazy copy？新风险？
- **Q7（设计）**：为什么 `Concat` 需要两个注册表？何时用哪个？

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 对应章节 | 建议讲解点 | 测试文件 |
|------|-------------|------|----------|-----------|---------|
| `compose/runnable.go` | `Runnable[I,O]`, `streamReader`, `composableRunnable`, `invoke()`, `stream()`, `collect()`, `transform()`, `recvAll`, `collected`, `InvokableLambda`/`StreamableLambda`/`CollectableLambda`/`TransformableLambda` | 14,22,103,110,158,186,242,74,93,311-373 | Ch03 | **降级矩阵核心** | `runnable_test.go` |
| `compose/stream.go` | `PipeStreamReader`, `NewPipe`, `Copy`, `Merge`, `Concat`, `RegisterConcatFunc` | 11,87,117,128,160,155 | Ch03 | 流原语层 | `stream_test.go` |
| `compose/concat.go` | `concatFuncRegistry`, `RegisterStreamChunkConcatFunc`, `ConcatItems`, `ConcatMessages` | 13,22,28,61 | Ch03 | 双注册表 + 消息级 concat | `stream_test.go` |
| `compose/callbacks.go` | `RunInfo`, `Handler`, `CallbackWrapper`, `TimingChecker`, `CbStreamReader`, `dispatchOnStartWithStreamInput` | 8,97,158,41,43,308 | Ch03 | 观测层核心：装饰器模式 | `callbacks_test.go` |
| `compose/event_log.go` | `EventType`, `Event`, `EventLog`, `JSONLEventSink`, `LogNodeStart`/`LogNodeEnd` | 13,28,100,49,164 | Ch03 | 全图级结构化事件 | `graph_test.go` |
| `compose/generic_graph.go` | `graphRunnable.Invoke()`/`Stream()` | 125 | Ch03 | 公开接口桥接内部 composableRunnable | `graph_test.go` |

**复刻版与原版差异**：Stream Copy 是 eager（原版 lazy）；`collectByStream` 用 `collected`（原版 `concatStreamReader`）；`chatMessageStreamReader` 是 slice-based fake（原版 5 后端）。

---

## Chapter 04 — Checkpoint / Interrupt / Resume（14 min）

> 复刻代码目录：`compose/` | 核心文件：`checkpoint.go`, `interrupt.go`, `address.go`, `state.go`  
> 原版手册对应：`manual/04-checkpoint-interrupt-resume.md`

### 讲解目标

1. 理解 Eino"类 Pregel 异步消息传递"模型中执行状态的分布性——为什么只保存"哪个节点停了"不够。
2. 画出 `AddressSegment` 的三种段类型（Runnable / Node / Tool）和地址匹配逻辑。
3. 解释 `InterruptSignal` 树结构 → `InterruptContext` 扁平化的设计意图。
4. 走通 `graph.Invoke → InterruptError → CheckPoint → ResumeWithData → Invoke` 完整生命周期。

### 问题背景

LLM 应用中的典型场景：
- **人工审批**：模型生成工具调用后，需人类确认才能执行。
- **多步中断**：ChatModel → Tool1 → Interrupt（需确认）→ 确认后 → Tool2。
- **嵌套中断**：子图执行到一半需等待外部输入才能继续。

Eino 的运行时是**类 Pregel 的异步消息传递模型**。一个中断发生时，执行状态分布在 channel、pending tasks、嵌套子图的 runner 实例中。如果只保存"哪个节点停了"，恢复时图行为不可预测。

### 为什么难

- 中断点需要在嵌套图/工具/stream 中有**稳定身份**——`AddressSegment` 就是这套命名系统。
- 恢复时 `globalResumeInfo` 经历四个阶段的演变（空 → 加 data → 加 addr → 匹配分发），这是本章最难讲透的点。
- `saveInterruptCheckPoint` 只在 ctx 中注入了 store + id 时才保存——否则静默跳过。学生容易困惑"为什么有时保存有时不保存"。
- 复刻版不保存 channel 状态、不做 stream materialization——这是有意为之的教学简化，但需明确标注。

### 核心抽象

**生命周期三阶段**：
```
中断:  graph.Invoke → lambda 调用 StatefulInterrupt
        → InterruptError → ExtractInterruptInfo
        → saveInterruptCheckPoint (仅当 ctx 中有 store + id)
        → InterruptError 向上传播

持久:  CheckPoint{
    Input: input,
    InterruptID2Addr: {"int_1": [r:my_graph, n:approval]},
    InterruptID2State: {"int_1": InterruptState{State: approvalState{...}}}
}

恢复:  ResumeWithData(ctx, "int_1", "approved")
        → store.Get("cp-1") → populateInterruptState
        → AppendAddressSegment 精确/前缀匹配
        → GetInterruptState / GetResumeContext 在 lambda 中读取
```

- **AddressSegment**：分层身份——Runnable / Node / Tool 三种段类型。`AppendAddressSegment` 在 `createTasks`（`graph_run.go:178`）为每个任务注入地址段。
- **前缀匹配 vs 精确匹配**：前缀匹配仅在对应 interruptID 的 `resumeData` 有数据时才设置 `isResumeTarget = true`（管道模式），数据留给精确匹配的后代。
- **InterruptSignal 树 → InterruptContext 扁平**：只取根因节点（`IsRootCause`），非根因不出现在输出但子信号继续递归。

### 复刻版代码走读

1. `checkpoint.go` — `CheckPoint`(L106), `saveInterruptCheckPoint`(L118), `restoreCheckPointContext`(L131), `checkpointConfig`(L112)
2. `interrupt.go` — `InterruptState`/`InterruptSignal`/`InterruptContext`/`InterruptError`(L12-50), `ToInterruptContexts`(L136), `SignalToPersistenceMaps`(L161)
3. `address.go` — `AddressSegment`(L18), `Address`(L27), `AppendAddressSegment`(L179), `GetCurrentAddress`(L162)
4. `state.go` — `WithGenLocalState`, `ProcessState`, `GetState`, `SetToolCallID`/`GetToolCallID`
5. `graph_run.go` — `restoreCheckPointContext`(L31), `AppendAddressSegment`(L35), `ExtractInterruptInfo`(L93), `saveInterruptCheckPoint`(L94)

### 演示建议

1. 讲一个故事：用户问"帮我订机票"→ ChatModel 说"需要确认：北京→上海，明早 8 点，对吗？"→ 中断 → 用户输入"对"→ 恢复 → 执行 Tool（3 min）。
2. 画黑板：`globalResumeInfo` 四阶段演变表（2 min）。
3. 走读 `address.go:179` 的精确匹配 vs 前缀匹配逻辑（2 min）。
4. 演示"忘记 `WithCheckPoint`"→ InterruptError 返回了但 store 是空的（1 min）。

### 容易误解点

1. **"Address 就是字符串"** → 不是，是 `[][]AddressSegment` 的结构化分层身份。
2. **"所有中断都会被保存"** → 否。`saveInterruptCheckPoint` 在 `checkpointConfig(ctx)` 返回 false 时静默跳过。
3. **"ResumeWithData 后状态自动恢复"** → 需 `Graph.Invoke` 重新调用，`restoreCheckPointContext` 加载 `CheckPoint`。
4. **"CheckPoint 保存了完整运行时状态"** → 复刻版不保存 channel 状态、pending tasks、stream 物化。这是教学简化。
5. **"前缀匹配等于精确匹配"** → 前缀匹配设置 `isResumeTarget=true` 但不注入恢复数据，留给精确匹配的后代。
6. **"两个中断可以同时用同一个 resumeData"** → 否。每个 interruptID 独享自己的 `resumeData`，通过 `idToAddress` / `idToState` 独立匹配。
7. **"未注入 CheckPoint 配置时 `saveInterruptCheckPoint` 会报错"** → 不会，静默跳过。

### 练习题

- **Q1**：画出 `graph.Invoke → interrupt → save → resume → Invoke` 的完整消息序列图。
- **Q2**：`StatefulInterruptAndWait` 和 `StatefulInterrupt` 的区别？
- **Q3**：Address `[r:agent, n:chat, t:tool:calc]` 的 `AppendAddressSegment` 到 `[r:agent, n:chat]` 会怎么匹配？（前缀匹配，管道模式）
- **Q4**：`globalResumeInfo` 中 `resumeData={"int_1": "approved"}`，`AppendAddressSegment` 到父节点地址 → `isResumeTarget` 为 true 或 false？
- **Q5**：如何让 `toolsNode` 在中断后保留 stream 的半消费状态？（原版用 `MaterializeStream`，复刻版未实现）
- **Q6（设计）**：如果要在 `CheckPoint` 中加入 channel 状态快照，需要在 `dagChannel` 和 `pregelChannel` 中暴露什么序列化接口？

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 对应章节 | 建议讲解点 | 测试文件 |
|------|-------------|------|----------|-----------|---------|
| `compose/checkpoint.go` | `CheckPoint`, `saveInterruptCheckPoint`, `restoreCheckPointContext`, `WithCheckPoint` | 106,118,131,112 | Ch04 | **检查点生命周期** | `graph_test.go` |
| `compose/interrupt.go` | `InterruptState`, `InterruptPayload`, `InterruptSignal`, `InterruptContext`, `InterruptError`, `ToInterruptContexts`, `SignalToPersistenceMaps` | 12,18,24,33,42,136,161 | Ch04 | 中断信号树→扁平化 | `interrupt_test.go` |
| `compose/address.go` | `AddressSegment`, `Address`, `AppendAddressSegment`, `GetCurrentAddress` | 18,27,179,162 | Ch04 | **结构化地址系统** | `state_test.go` |
| `compose/state.go` | `WithGenLocalState`, `ProcessState`, `GetState` | — | Ch04 | Per-run state 隔离 | `state_test.go` |

**复刻版简化**：不保存 channel 状态；不保存 pending tasks；不实现 `MaterializeStream` / `RestoreStream`；`saveInterruptCheckPoint` 不调用 stream 物化。

---

## Chapter 05 — Components / Model / Tool / Prompt（14 min）

> 复刻代码目录：`compose/` | 核心文件：`chatmodel.go`, `prompt.go`, `prompt_tool_bridge.go`, `bridge.go`, `retriever.go`, `schema.go`  
> 原版手册对应：`manual/05-components-model-tool-prompt.md`

### 讲解目标

1. 理解"Bridge Adapter 模式"：领域组件通过 `toLambda()` / `GetRunnable()` 进入 Graph——Graph 只认识 Runnable，不认识具体组件。
2. 识别 `ChatModel` / `Retriever` / `Tool` / `Prompt` 四种领域接口各自的"最小契约"。
3. 理解教学层 Bridge（`BridgeChatModel` / `BridgeRetriever`）与生产层 Component（`ChatModelComponent` / `NewRetrieverLambda`）的差异。
4. 走通一条 RAG 管线：`Retriever → PromptAssembler → ChatModel → END`，通过 FieldMapping 编排数据流。

### 问题背景

手写 OpenAI 调用的五个问题：
1. **Provider 锁定**：换 Anthropic 需改所有调用点。
2. **不可编排**：无法放入 Graph/Workflow，失去编译期校验。
3. **不可观测**：无法挂载 Callback。
4. **无类型安全**：输入类型不匹配运行时才报错。
5. **工具并发不安全**：直接在 model 实例上 `BindTools` → goroutine 竞态。

### 为什么难

**四重困难**：
1. **接口粒度**：太细产生样板，太粗推迟校验到运行时。Eino 折中：`BaseModel` 恰好两方法 + 组合扩展。
2. **Provider 选项泄漏**：OpenAI 需 `user` 字段、Anthropic 需 `cache` 配置——公共接口不能直接暴露，但也不能完全丢失。Eino 用双桶 Option + `ResponseMeta` 扩展槽位。
3. **工具绑定的并发安全**：Eino 弃用 `BindTools`，改为 `WithTools()` 返回新实例。复刻版进一步简化：工具在管线上的独立 `toolsNodeBridge` 执行。
4. **工具结果保真度**：多模态工具返回图片/音频——`ToolResult` 多模态容器。

### 核心抽象

```
领域层               桥接层                 编排层
ChatModel ─────── GetRunnable() ────────→ compose.Graph
Retriever ─────── NewRetrieverLambda ──→
BridgeTool ─────── toolsNodeBridge ─────→
ChatTemplate ───── promptTemplateBridge →        │
                                                ▼
                                        compose.Runnable
```

- **ChatModel**：最小两方法 `Generate` + `Stream`（`chatmodel.go:194`）。Fake 实现默认回显，支持 `WithChatGenerateFunc` / `WithChatStreamFunc` 注入。
- **Message**：Role 驱动模型（System/Human/Assistant/Tool），`ToolCalls` 在顶层字段，`ResponseMeta` 含 Provider 扩展槽位。
- **Tool**：`BridgeTool` 接口仅 `Execute(ctx, args string) (string, error)`。`toolsNodeBridge.toLambda()` 将多个工具注册为管线节点。
- **Bridge 两层的回调注入方式不同**：
  - 教学层（`chatModelBridge` / `retrieverBridge`）：Graph `SetNodeCallbacks`，bridge 内部不自带。
  - 生产层（`NewRetrieverLambda`）：`RetrieverConfig.Handlers` 内置 CallbackWrapper。

### 复刻版代码走读

学习路径按从简单到复杂：
1. `retriever.go` — `Retriever`(L15)、`FakeRetriever`(L19)、`NewRetrieverLambda`(L41，内置回调注入)
2. `bridge.go` (L1-86) — `BridgeRetriever`(L41)、`retrieverBridge.toLambda()`(L53)、`chatModelBridge.toLambda()`(L76)、`promptAssemblerBridge`(L91)
3. `chatmodel.go` — `Message`(L24)、`ResponseMeta`(L39)、`ChatModel`(L194)、`FakeChatModel`(L199)、`ChatModelComponent`(L282)
4. `prompt.go` — `ChatTemplate`(L11)、`MessageTemplate`(L15，`{{var}}` 语法)
5. `prompt_tool_bridge.go` — `BridgeTool`(L24)、`toolsNodeBridge`(L67)、管线 Workflow 便捷方法(L130)
6. `bridge.go` (L88-134) + `bridge_test.go:150-209` — RAG Workflow 大轴子

### 演示建议

1. 展示手写 OpenAI 调用的反例代码 → 指出 5 个问题（2 min）。
2. 走读 `retrieverBridge.toLambda()` → 一行 `InvokableLambda(func...)`（1 min）。
3. 走读 RAG 管线（`TestBridgeRAGPipelineWorkflow`）：retriever → assemble → model（3 min）。
4. 对比教学层 vs 生产层三种维度（接口复杂度、回调注入方式、便捷方法）（2 min）。

### 容易误解点

1. **"Component 就是 Node"** → Component 是 `GetRunnable()` 的生产者，Node 是 Graph 中的执行节点。
2. **"Bridge 模式就是多一层包装而已"** → 桥接还带来类型校验、观测锚点、可替换性。
3. **"Message 的 Role 只是标签"** → 是分派逻辑的驱动：`Tool` role 消息通过 `ToolCallID` 关联回上游 ToolCall。
4. **"FakeChatModel 和真实 ChatModel 行为一样"** → Fake 是教学简化：无 token 计数、无多模态、无 provider 网络调用。
5. **"ToolsNode 和 Tool 是一回事"** → ToolsNode 是桥接层节点，包含多个 `BridgeTool` 的注册和分发。
6. **"ChatModelComponent 内置回调注入"** → 否。只有 `NewRetrieverLambda` 内置回调。ChatModelComponent 的回调通过 Graph `SetNodeCallbacks` 或外部 `NewCallbackWrapper`。
7. **"StreamReader 都不需要 Close"** → `chatMessageStreamReader` 是 slice-based fake 不需要 Close，但真实 stream（goroutine 读 HTTP body）需要 Close。
8. **"AgenticModel 只是 ChatModel 的变体"** → 有本质不对称：`AgenticModel` 无 `WithTools` 方法，工具通过请求时选项传入。

### 练习题

- **Q1**：写一个 `FakeChatModel`，返回固定的 Message。
- **Q2**：实现 `BridgeTool`，调用 `ChatModel.Generate` 来总结结果。
- **Q3**：实现 `StubTranslationTool`，用 `NewToolsNodeLambda` 创建多工具节点。
- **Q4**：`ChatModelComponent.GetRunnable()` 同时返回 `cr.i` 和 `cr.s`。没有 Stream 能力的模型如何处理？（降级矩阵自动 bridge）
- **Q5**：`NewRetrieverLambda` 内置回调，但 `ChatModelComponent` 不内置。为什么？适合什么场景？
- **Q6**：如何在 `toolsNodeBridge` 中支持并行执行多个工具？
- **Q7**：`BridgeDocument.Score` 是余弦距离 [0,1]，需要欧氏距离怎么办？在桥接层做转换。
- **Q8**：为什么 Eino 需要 `AgenticModel`？对 toolsNodeBridge 设计有什么影响？

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 对应章节 | 建议讲解点 | 测试文件 |
|------|-------------|------|----------|-----------|---------|
| `compose/chatmodel.go` | `RoleType`, `Message`, `ResponseMeta`, `ChatModel`, `FakeChatModel`, `ChatModelComponent`, `SystemMessage`/`HumanMessage`/`AssistantMessage`/`ToolMessage` | 10,24,39,194,199,282,317-336 | Ch05 | 经典消息模型 + Component 桥接 | `chatmodel_test.go` |
| `compose/retriever.go` | `Retriever`, `FakeRetriever`, `NewRetrieverLambda` | 15,19,41 | Ch05 | 最简领域契约 + 内置回调注入 | `retriever_test.go` |
| `compose/prompt.go` | `ChatTemplate`, `MessageTemplate`, `ChatTemplateComponent` | 11,15,50 | Ch05 | 模板引擎：`{{var}}` 正则替换 | `prompt_test.go` |
| `compose/prompt_tool_bridge.go` | `BridgeTool`, `promptTemplateBridge`, `toolsNodeBridge` | 24,45,67 | Ch05 | Tool 管线：Prompt→Model→Tools→Model | `prompt_tool_bridge_test.go` |
| `compose/bridge.go` | `BridgeChatModel`, `BridgeRetriever`, `retrieverBridge`, `chatModelBridge`, `promptAssemblerBridge` | 46,41,53,76,91 | Ch05 | **教学层 Bridge**：一行 toLambda() | `bridge_test.go` |
| `compose/schema.go` | `ToolCall`, `ToolInfo`, `Document`, `ToolResult` | 7,21,168,130 | Ch05/Ch06 | 共享数据结构 | `schema_test.go` |

---

## Chapter 06 — Schema / Provider Adapter（12 min）

> 复刻代码目录：`compose/` | 核心文件：`provider_openai.go`, `provider_claude.go`, `provider_gemini.go`, `provider.go`, `schema.go`  
> 原版手册对应：`manual/06-schema-provider-adapter.md`

### 讲解目标

1. 解释为什么多 Provider 框架必须在消息格式、工具参数 Schema、流式协议上建立**规范数据模型（Canonical Schema）**。
2. 区分 `Message`（经典 Chat Completion）和 `AgenticMessage`（Agent 级对话）的设计动机和适用场景。
3. 理解 Provider 适配器的职责边界：**双向转换**（原生类型 ↔ 规范类型），不参与 Graph 调度。
4. 掌握 `ParamsOneOf` 双模式：轻量级 `params` 树 vs 完整 JSON Schema `anyOf`。
5. 理解类型化 Provider 扩展槽位为什么优于 `map[string]any`。

### 问题背景

OpenAI / Claude / Gemini 三家的消息格式：
- OpenAI：`messages[].role` = user/assistant/system/tool
- Claude：`messages[].role` = user/assistant，content 是 `[{type: "text"|"tool_use"|"tool_result"}]` 数组
- Gemini：`contents[].role` = user/model/function，parts 是联合体

如果用三个独立 format → Graph 需要感知 provider → 切换模型等于重写 Graph。

### 为什么难

1. **角色映射不一致**：Gemini 的 `"function"` → Agentic 路径映射为 `AgenticRoleUser`，Message 路径映射为 `Tool`。同一输入有两种语义。
2. **工具调用在两种模型中的位置不同**：`Message.ToolCalls`（顶层字段）vs `AgenticMessage.ContentBlocks[].FunctionToolCall`（内容块）。
3. **Provider 扩展信息容易丢失**：如果用 `map[string]any`，OpenAI 的 `id` 和 Claude 的 `stop_reason` 在合并时后到的会覆盖先到的。
4. **复刻版中 `AgenticMessage` 无 `ResponseMeta`**——通过 Agentic 路径无法携带 Provider 扩展元数据（教学简化边界）。

### 核心抽象

**三层架构（复刻版单包内联）**：
```
compose/ 包（单包内联）
  ├── schema.go           ← 规范 Schema: ToolCall, ToolInfo, ParamsOneOf
  ├── chatmodel.go        ← Message + ResponseMeta + Provider 扩展槽位
  ├── provider.go         ← AgenticMessage + ContentBlock + Provider 接口
  ├── provider_openai.go  ← FakeOpenAIProvider + 双向转换
  ├── provider_claude.go  ← FakeClaudeProvider + 双向转换
  └── provider_gemini.go  ← FakeGeminiProvider + 双路径转换
```

- **`Message` vs `AgenticMessage`**：前者 4 角色 + `ToolCalls` 顶层字段，后者 3 角色（无 Tool 角色）+ `ContentBlocks[]` 统一承载。
- **`ParamsOneOf`**：`params` 非 nil → 走 `paramsToMap()`；否则走 `jsonSchema`。使用构造函数而非手动设置字段避免"双重设置"陷阱。
- **Provider 扩展槽位**：类型化 nil 指针（`OpenAIExtension` / `ClaudeExtension` / `GeminiExtension`），合并时各自处理自己的槽位。
- **Fake Provider 的目的**：验证 Schema 边界的正确性，不验证网络层。

### 复刻版代码走读

1. `schema.go` — `ToolCall`(L7), `ToolInfo`(L21), `ParamsOneOf`(L29), `ToJSONSchema`(L47), `ParameterInfo`(L120)
2. `chatmodel.go` — `Message`(L24), `ResponseMeta`(L39), 三个扩展类型(L135,149,160)
3. `provider.go` — `ContentBlockType`(L5), `ContentBlock`(L23), `AgenticMessage`(L67), 三个 Provider 接口(L143-161)
4. `provider_openai.go` — `ToCanonicalMessages`(L48), `FromCanonicalMessages`(L65), 角色映射(L18,33)
5. `provider_gemini.go` — 双路径四套角色映射：`geminiRoleToAgentic`(L39) vs `geminiRoleToMessage`(L65)
6. 跨 Provider 集成测试：`TestCanonicalMessageFromOpenAIChatModel`(provider_test.go:379), `TestGeminiFullPipeline`(428)

### 演示建议

1. 三列并排板书：OpenAI / Claude / Gemini 的消息格式对比（2 min）。
2. 走读 `provider_openai.go:48` `ToCanonicalMessages`：字段级 1:1 映射（1 min）。
3. 走读 `provider_gemini.go:93` vs `:164`：双路径对比（1 min）。
4. 黑板对比 `map[string]any` vs 类型化扩展槽位在流式合并中的行为差异（1 min）。

### 容易误解点

1. **混淆两种 Message 模型** — `Message` 有 ToolCall 顶层字段，`AgenticMessage` 有 ContentBlock。复刻版中二者之间无桥接函数。
2. **依赖 `Extra` 而非扩展槽位** — `map[string]any` 在跨 Provider 流式合并中会丢失另一个 Provider 的元数据。
3. **流式工具调用丢失 `Index`** — 复刻版 `ToolCall.Index` 定义存在但未被 concat 逻辑消费。
4. **混淆 Gemini 角色映射** — `"function"` → Agentic 路径为 `User`，Message 路径为 `Tool`。
5. **ParamsOneOf 双重设置** — 手动设置 `p.params`（非 nil）后再设置 `p.jsonSchema` → jsonSchema 被静默忽略。
6. **`Human` 和 `User` 角色常量冗余** — `chatmodel.go` 定义 `Human`，`types.go` 定义 `User`，两者都映射为 `"user"`，但字符串比较时要注意。
7. **复刻版 StreamReader 是简化版** — 原版有 5 种后端（Channel/Array/MultiStream/WithConvert/Child），复刻版只有 slice-based。

### 练习题

- **Q1**：写出 OpenAI → 规范 Message → FakeChatModel → 规范 Message → OpenAI 往返的 6 个步骤。
- **Q2**：为什么 `ToCanonicalMessagesFromGemini` 和 `ToCanonicalAgenticMessagesFromGemini` 是两个独立函数？
- **Q3**：`ParamsOneOf` 两个字段都非 nil → `ToJSONSchema()` 返回什么？→ params 优先。
- **Q4.5**：`TestToolCall_JsonRoundTrip` 中 `Index` 丢失的原因？（`json:"-"` 标签）对流式工具合并意味着什么？
- **Q5**：为 Gemini 写 `default` 分支的角色映射，保证未来新增角色不 panic。
- **Q6**：`FakeClaudeProvider` 为什么不需要 `ToCanonicalMessages` 方法？
- **Q8**：设计 `ServerToolCallBlock`，使其能在 `ContentBlock` 联合体中被正确识别。

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 对应章节 | 建议讲解点 | 测试文件 |
|------|-------------|------|----------|-----------|---------|
| `compose/schema.go` | `ToolCall`, `ToolInfo`, `ParamsOneOf`, `NewParamsOneOfByParams`, `NewParamsOneOfByJSONSchema`, `ToJSONSchema`, `ParameterInfo` | 7,21,29,35,40,47,120 | Ch06 | **双模式参数 Schema** | `schema_test.go` |
| `compose/chatmodel.go` | `Message`, `ResponseMeta`, `OpenAIRespMetaExtension`, `ClaudeRespMetaExtension`, `GeminiRespMetaExtension`, `ChatModel`, `FakeChatModel` | 24,39,135,149,160,194,199 | Ch06 | 规范消息 + 扩展槽位 | `chatmodel_test.go` |
| `compose/provider.go` | `ContentBlockType`, `ContentBlock`, `AgenticMessage`, `ProviderOpenAI`, `ProviderClaude`, `ProviderGemini` | 5,23,67,143,149,155 | Ch06 | ContentBlock 联合体 + 三个 `Provider*` 接口 | `provider_test.go` |
| `compose/provider_openai.go` | `OpenAIMessage`, `OpenAIChatRequest`, `openAIRoleToCanonical`/`canonicalRoleToOpenAI`, `ToCanonicalMessages`/`FromCanonicalMessages`, `FakeOpenAIProvider` | 5,13,18,33,48,65,79 | Ch06 | **最简单的 Provider 适配器** | `provider_test.go` |
| `compose/provider_claude.go` | `ClaudeContentBlock`, `ClaudeMessage`, `ClaudeChatRequest`, `claudeRoleToAgentic`/`agenticRoleToClaude`, `ToCanonicalAgenticMessages`/`FromCanonicalAgenticMessages`, `FakeClaudeProvider` | 5,22,27,32,43,56,98,130 | Ch06 | Claude 的 content 数组提取 | `provider_test.go` |
| `compose/provider_gemini.go` | `GeminiPart`, `GeminiContent`, `GeminiChatRequest`, `geminiRoleToAgentic`/`geminiRoleToMessage`, `ToCanonicalAgenticMessagesFromGemini`/`ToCanonicalMessagesFromGemini`, `FakeGeminiProvider` | 8,30,35,39,65,93,164,239 | Ch06 | **最复杂 Provider：双路径四套角色映射** | `provider_test.go` |

**复刻版简化**：Schema 类型内联在 `compose/` 包（原版独立 `schema/` 包）；StreamReader 仅 slice-based（原版 5 后端）；无 `ConcatMessages` 流式合并；`AgenticMessage` 无 `ResponseMeta`；Provider 适配器都是 Fake（无真实 SDK）。

---

## Chapter 07 — Agent Flow / ReAct / MultiAgent（12 min）

> 复刻代码目录：`agent/` + `compose/multiagent.go` | 核心文件：`agent/react.go`, `agent/types.go`, `compose/multiagent.go`  
> 原版手册对应：`manual/07-agent-flow-react-multiagent.md`  
> 本章基于 Chapter 07 writer/reviewer artifact 合成，重点保留 ReAct/MultiAgent 的图结构、状态语义和教学边界。

### 讲解目标

1. **ReAct agent 不是独立 runtime，而是一张 `compose.Graph`**：循环通过 `AnyPredecessor` + Pregel 实现。
2. **ChatModel + Tools + Branch + State** 四个概念组合就是 Agent。缺任何一个，ReAct 的哪一块会塌。
3. **`StreamToolCallChecker` 为什么必须可插拔**：OpenAI 第一个 chunk 有 tool call，Claude 先文本再 tool call → 默认 checker 对 Claude 误判。
4. **`MessageRewriter`（持久）vs `MessageModifier`（临时）** 的语义区别和 copy-vs-in-place 执行顺序。
5. **Host Multi-Agent 把 specialist 当 tool**：Specialist 收到完整用户消息历史（非 ToolCall 参数）。

### 问题背景

手写 agent loop 的四个缺陷：
1. **终止条件是内容驱动的** — "模型自己觉得不需要再调工具"不可预测。
2. **Message history 无法嵌套隔离** — 子 agent 的 messages 污染父 agent。
3. **Streaming 下无法判断 tool call** — 流式分块到达，不同 provider 输出顺序不同。
4. **工具结果可能直接是最终答案** — 搜索工具返回精确结果，agent 应立即返回，不需要模型"总结"。

### 为什么难

- Agent 的真正复杂度不在循环本身，而在**可插拔的决策点**：
  - `StreamToolCallChecker`：不同 provider 的流式输出顺序不同，判断"有无 tool call"的时机需要可注入。
  - `MessageRewriter` vs `MessageModifier`：作用对象不同（state vs copy）、持久性不同、执行顺序有要求。
  - `ToolReturnDirectly` 两种优先级（配置级 vs 运行时）的覆盖关系。
- **Host Multi-Agent 的 specialist 入参替代**：Host 输出 ToolCall，但 specialist 收到的是完整用户消息历史——原因是 specialist 需要完整上下文。
- **Agent 输出是 `Runnable[[]*Message, *Message]`**：可以独立调用、嵌套为子图、或作为另一个 Multi-Agent 的 Specialist。

### 核心抽象

**ReAct Graph 拓扑**：
```
START → ChatModel ──(ToolCall?)──→ Tools → ChatModel (loop, Pregel 驱动)
                 └──(no ToolCall)──→ END
                 Tools ──(returnDirectly?)──→ direct_return → END
```

- **Graph Local State** 保存消息历史（`reactState: Messages + ReturnDirectlyToolCallID`），不在节点间传递。
- **`modelPreHandle`**：追加 → Rewriter（持久改 state）→ copy → Modifier（临时改 copy）→ 返回给 ChatModel。
- **`buildReturnDirectly`**：在 Tools 节点后添加 branch，检查 `state.ReturnDirectlyToolCallID` → 匹配 tool result → 直接走 END。

**Host Multi-Agent**：
```
START → Host(ChatModel) ──(no tool call)──→ END
                        ──(has tool calls)→ SpecialistExecutor
                           SpecialistExecutor──单意图→ END
                           SpecialistExecutor──多意图→ Summarize → END
```

- Specialist 被包装成 `ToolInfo`，Host ChatModel bind 这些 ToolInfo。
- Specialist 三种形式：ChatModel / Invokable / Streamable（优先级链）。
- 复刻版 MultiAgent 简化为单节点 `InvokableLambda` 封装（原版是完整多节点图）。

### 复刻版代码走读

1. `agent/types.go` — `AgentConfig`(L10), `reactState`(L31), `Agent`(L37), `MessageRewriter`/`MessageModifier`/`StreamToolCallChecker` 类型定义
2. `agent/react.go` — `NewAgent`(L28), `modelPreHandle`(L88), `toolsNodePreHandle`(L119), `modelPostBranchCondition`(L143), `buildReturnDirectly`(L155), `DefaultStreamToolCallChecker`(L206), `ScanAllStreamToolCallChecker`(L233), `SetReturnDirectly`(L250)
3. `agent/react_test.go` — `TestReAct_SingleToolCall`(94), `TestReAct_MessageRewriter_Compression`(311), `TestReAct_StreamToolCallChecker_ClaudeStyle`(413)
4. `compose/multiagent.go` — `Specialist`(L10), `NewMultiAgent`(L59), `executeMultiAgent`(L114), `invokeSpecialist`(L155)

### 演示建议

1. 开场问："如果我让你手写 agent loop，怎么写？"→ 展示手写 for 循环的 4 个缺陷（2 min）。
2. 黑板画 ReAct 图拓扑：ChatModel → (ToolCall?) → Tools → ChatModel loop（2 min）。
3. 走读 `modelPreHandle`：Rewriter 持久 vs Modifier 临时的 copy-vs-in-place（2 min）。
4. 投屏 `agent/react.go:28` `NewAgent` 函数 → 证明"Agent = Graph"（2 min）。
5. 演示 Claude 用 `DefaultStreamToolCallChecker` 的错误行为 → 切换到 `ScanAllStreamToolCallChecker`（1 min）。

### 容易误解点

1. **"Agent 是一个特殊执行引擎"** → ReAct agent 就是一张 `compose.Graph`，和 `NewChain`、`NewWorkflow` 同一层抽象。
2. **"MessageModifier 修改 state.Messages"** → Modifier 只修改 copy，不影响 state。用 Rewriter 注入 system prompt 会导致累积。
3. **"Claude 直接用默认 Checker 就行"** → 默认 checker 看到文本 chunk 先到 → 返回 false → agent 认为无 tool call → 工具不执行。
4. **"Specialist 的入参是 Host 的 ToolCall 参数"** → 实际上是完整用户消息历史（`input := originalMsgs`）。
5. **"ToolReturnDirectly 配置级和运行时共存"** → 运行时 `SetReturnDirectly` 覆盖配置级，只有一个 call ID 生效。
6. **"WithTools 分两次传也行"** → `WithTools` 同时做两件事：让 ChatModel 知道 schema + 让 ToolsNode 知道如何执行。必须同一次调用传入。
7. **"Host Multi-Agent 的图结构和 ReAct 一样复杂"** → 复刻版简化为单节点 Lambda 封装（教学简化）。

### 练习题

- **Q1**：画出 ReAct agent 的完整图拓扑，标注每个分支触发条件。
- **Q2**：`modelPreHandle` 中步骤填空：追加 → ? → copy → ? → return。
- **Q3**：判断对错：MessageRewriter 和 MessageModifier 可以合并？（不能，持久性语义不同）；MaxStep 是主要终止机制？（不是，正常终止靠 `modelPostBranchCondition`）；Specialist 必须实现 ChatModel？（不，支持 Invokable 和 Streamable）。
- **Q4**：为 Gemini 实现 `StreamToolCallChecker`，与 OpenAI/Claude 有什么区别？
- **Q5**：如果 `buildReturnDirectly` 不存在，搜索工具返回精确结果 + 模型总结加入幻觉 → 怎么办？
- **Q6**：Host Multi-Agent 中 specialist 自身是 ReAct agent（嵌套）→ `invokeSpecialist` 的 `input := originalMsgs` 有问题吗？
- **Q7（设计）**：如果要把 ReAct agent 的 message history 持久化（checkpoint/restore），需要改哪些地方？

### 代码附录

| 文件 | 核心类型/函数 | 行号 | 对应章节 | 建议讲解点 | 测试文件 |
|------|-------------|------|----------|-----------|---------|
| `agent/types.go` | `AgentConfig`, `MessageRewriter`, `MessageModifier`, `StreamToolCallChecker`, `reactState`, `Agent` | 10,21,25,28,31,37 | Ch07 | 所有类型定义 + 可插拔注入点 | `agent/react_test.go` |
| `agent/react.go` | `NewAgent`, `modelPreHandle`, `toolsNodePreHandle`, `modelPostBranchCondition`, `buildReturnDirectly`, `DefaultStreamToolCallChecker`, `ScanAllStreamToolCallChecker`, `SetReturnDirectly` | 28,88,119,143,155,206,233,250 | Ch07 | **ReAct Graph Builder 核心** | `agent/react_test.go` |
| `compose/multiagent.go` | `Specialist`, `MultiAgentConfig`, `NewMultiAgent`, `executeMultiAgent`, `invokeSpecialist`, `defaultSummarize`, `customSummarize` | 10,26,59,114,155,202,213 | Ch07 | Host Multi-Agent 路由 + Specialist 调用 | `compose/multiagent_test.go` |
| — | `cmd/example/main.go` example22/23 | 1601-1876 | Ch07 | 完整可运行示例 | — |

**依赖的基础设施（跨章交叉引用）**：
- Ch01: `Graph[I,O]`, `Compile()`, `Pregel` (`AnyPredecessor`), `MaxStep`
- Ch02: `GraphBranch`, `BranchCondition`
- Ch03: `Runnable[I,O]`, `Invoke`, `Stream`, `WithGenLocalState`, `ProcessState`, `GetState`
- Ch04: `Address`, `GetCurrentAddress`（嵌套 Callback 隔离）
- Ch05: `ChatModel`, `FakeChatModel`, `Message`, `ToolCall`, `ToolInfo`, `BridgeTool`

---

## 附录 A：全手册文件索引

| 文件 | 核心类型/函数 | 对应章节 | 建议讲解点 | 测试文件 |
|------|-------------|----------|-----------|---------|
| `compose/types.go` | `NodeTriggerMode`, `START`, `END`, `defaultMaxSteps`, `ErrGraphCompiled`, `ErrDAGHasCycle`, `RoleType`(User) | Ch01, Ch06 | 全局常量和错误枚举 | `graph_test.go` |
| `compose/graph.go` | `graph`, `checkCompiled`, `AddEdge`, `AddControlEdge`, `AddBranch`, `compile`, `checkDAGCycles` | Ch01 | **编译边界入口 + Kahn 算法** | `graph_test.go` |
| `compose/graph_node.go` | `graphNode`, `compileIfNeeded` | Ch01 | 节点三形态：cr / g / info | `graph_test.go` |
| `compose/graph_compile.go` | `graphCompileOptions`, `WithGraphName`, `WithNodeTriggerMode`, `WithMaxRunSteps` | Ch01, Ch07 | 函数式编译选项 | `graph_test.go` |
| `compose/generic_graph.go` | `Graph[I,O]`, `NewGraph`, `Compile`, `graphRunnable` | Ch01, Ch03 | 泛型公开 API + Runnable 接口实现 | `graph_test.go` |
| `compose/graph_manager.go` | `channel` 接口, `chanCall`, `channelManager`, `taskManager` | Ch01 | DAG/Pregel 多态基础 | `graph_test.go` |
| `compose/graph_run.go` | `runner`, `run`, `resolveCompletedTasks`, `initChannels`, `routeInputToStartNodes`, `createTasks` | Ch01, Ch04 | **运行时主循环** | `graph_test.go` |
| `compose/dag.go` | `dependencyState`, `dagChannel`, `reportValues`, `reportDependency`, `reportSkip`, `get` | Ch01 | **AllPredecessor barrier 判断** | `graph_test.go` |
| `compose/pregel.go` | `pregelChannel`, `reportValues`, `get` | Ch01 | **AnyPredecessor fire-on-any** | `graph_test.go` |
| `compose/field_mapping.go` | `fieldPathSeparator`, `FieldPath`, `FieldMapping`, `MapFields`/`FromField`/`ToField`, `checkAndExtractFieldType`, `checkAssignable`, `validateFieldMapping`, `fieldMap` | Ch02 | 编译时类型检查 + 运行时字段提取 | `field_mapping_test.go` |
| `compose/workflow.go` | `dependencyType`, `WorkflowNode`, `WorkflowBranch`, `Workflow[I,O]`, `AddInput`/`AddDependency`/`WithNoDirectDependency`, `addDependencyRelation`, `compile` | Ch02 | 声明式依赖三种模式 + 延迟闭包 | `workflow_test.go` |
| `compose/chain.go` | `Chain[I,O]`, `addNodeEdges`, `AppendParallel`, `AppendBranch`, `AppendGraph`, `Compile` | Ch02 | preNodeKeys 自动追踪算法 | `chain_test.go` |
| `compose/chain_parallel.go` | `Parallel`, `AddLambda` | Ch02 | outputKey 防重复 | `chain_test.go` |
| `compose/chain_branch.go` | `ChainBranch`, `NewChainBranch`, `NewChainMultiBranch` | Ch02 | 单分支 vs 多分支输出差异 | `chain_test.go` |
| `compose/branch.go` | `GraphBranch`, `NewGraphBranch` | Ch01, Ch02, Ch07 | 条件分支：noDataFlow + condition | `graph_test.go` |
| `compose/introspect.go` | `GraphInfo`, `GraphNodeInfo` | Ch01 | 编译后可导出拓扑快照 | `graph_test.go` |
| `compose/runnable.go` | `Runnable[I,O]`, `streamReader`, `composableRunnable`, `invoke`, `stream`, `collect`, `transform`, `recvAll`, `collected`, `InvokableLambda`/`StreamableLambda`/`CollectableLambda`/`TransformableLambda` | Ch03, Ch07 | **4×4 降级矩阵的核心** | `runnable_test.go` |
| `compose/stream.go` | `PipeStreamReader`, `NewPipe`, `Copy`, `Merge`, `Concat`, `RegisterConcatFunc` | Ch03 | 流原语：goroutine channel 流 | `stream_test.go` |
| `compose/concat.go` | `concatFuncRegistry`, `RegisterStreamChunkConcatFunc`, `ConcatItems`, `ConcatMessages` | Ch03 | 双注册表 + 消息级 concat | `stream_test.go` |
| `compose/callbacks.go` | `RunInfo`, `Handler`, `CallbackWrapper`, `TimingChecker`, `CbStreamReader`, `dispatchOnStartWithStreamInput` | Ch03 | 观测层核心：装饰器模式 | `callbacks_test.go` |
| `compose/event_log.go` | `EventType`, `Event`, `EventLog`, `JSONLEventSink` | Ch03 | 全图级结构化事件（含 EventCheckpoint） | `graph_test.go` |
| `compose/checkpoint.go` | `CheckPoint`, `saveInterruptCheckPoint`, `restoreCheckPointContext`, `WithCheckPoint` | Ch04 | **检查点生命周期** | `graph_test.go` |
| `compose/interrupt.go` | `InterruptState`, `InterruptSignal`, `InterruptContext`, `InterruptError`, `ToInterruptContexts`, `SignalToPersistenceMaps` | Ch04 | 中断信号树 → 扁平化 | `interrupt_test.go` |
| `compose/address.go` | `AddressSegment`, `Address`, `AppendAddressSegment`, `GetCurrentAddress` | Ch04 | **结构化地址系统** | `state_test.go` |
| `compose/state.go` | `WithGenLocalState`, `ProcessState`, `GetState`, `SetToolCallID`, `GetToolCallID` | Ch04, Ch07 | Per-run state 隔离 | `state_test.go` |
| `compose/chatmodel.go` | `RoleType`, `Message`, `ResponseMeta`, `ChatModel`, `FakeChatModel`, `ChatModelComponent`, `SystemMessage`/`HumanMessage`/`AssistantMessage`/`ToolMessage` | Ch05, Ch06 | 经典消息模型 + 三种 Provider 扩展槽位 | `chatmodel_test.go` |
| `compose/retriever.go` | `Retriever`, `FakeRetriever`, `NewRetrieverLambda` | Ch05 | 最简领域契约 + 内置回调注入 | `retriever_test.go` |
| `compose/prompt.go` | `ChatTemplate`, `MessageTemplate`, `ChatTemplateComponent` | Ch05 | 模板引擎 | `prompt_test.go` |
| `compose/prompt_tool_bridge.go` | `BridgeTool`, `promptTemplateBridge`, `toolsNodeBridge` | Ch05 | Tool 管线：Prompt→Model→Tools→Model | `prompt_tool_bridge_test.go` |
| `compose/bridge.go` | `BridgeChatModel`, `BridgeRetriever`, `retrieverBridge`, `chatModelBridge`, `promptAssemblerBridge` | Ch05 | **教学层 Bridge** | `bridge_test.go` |
| `compose/schema.go` | `ToolCall`, `ToolInfo`, `ParamsOneOf`, `ParameterInfo`, `ToolResult`, `Document` | Ch05, Ch06 | 共享规范数据结构 | `schema_test.go` |
| `compose/provider.go` | `ContentBlockType`, `ContentBlock`, `AgenticMessage`, `ProviderOpenAI`, `ProviderClaude`, `ProviderGemini` | Ch06 | ContentBlock 联合体 + Provider 合约 | `provider_test.go` |
| `compose/provider_openai.go` | `OpenAIMessage`, `OpenAIChatRequest`, `ToCanonicalMessages`, `FromCanonicalMessages`, `FakeOpenAIProvider` | Ch06 | **最简单 Provider 适配器** | `provider_test.go` |
| `compose/provider_claude.go` | `ClaudeContentBlock`, `ClaudeMessage`, `ClaudeChatRequest`, `ToCanonicalAgenticMessages`, `FakeClaudeProvider` | Ch06 | Claude content 数组提取 | `provider_test.go` |
| `compose/provider_gemini.go` | `GeminiPart`, `GeminiContent`, `GeminiChatRequest`, `ToCanonicalAgenticMessagesFromGemini`, `ToCanonicalMessagesFromGemini`, `FakeGeminiProvider` | Ch06 | **最复杂 Provider：双路径四套角色映射** | `provider_test.go` |
| `agent/types.go` | `AgentConfig`, `MessageRewriter`, `MessageModifier`, `StreamToolCallChecker`, `reactState`, `Agent` | Ch07 | Agent 类型定义 + 可插拔注入点 | `agent/react_test.go` |
| `agent/react.go` | `NewAgent`, `modelPreHandle`, `toolsNodePreHandle`, `modelPostBranchCondition`, `buildReturnDirectly`, `DefaultStreamToolCallChecker`, `ScanAllStreamToolCallChecker`, `SetReturnDirectly` | Ch07 | **ReAct Graph Builder 核心** | `agent/react_test.go` |
| `compose/multiagent.go` | `Specialist`, `MultiAgentConfig`, `NewMultiAgent`, `executeMultiAgent`, `invokeSpecialist` | Ch07 | Host Multi-Agent 路由 | `compose/multiagent_test.go` |
| `cmd/example/main.go` | `example22_ReActAgent`, `example23_HostMultiAgent` | Ch07 | 完整可运行示例 | — |

---

## 附录 B：讲给别人时的关键 Thesis

1. **编译边界是第一性原理。** 构建期可自由加节点/边；编译期完成拓扑、类型、字段映射、运行模式校验；运行期只暴露 `Runnable`。这个边界是后续所有能力（Callback/Checkpoint/Agent）的挂载基础。

2. **Runnable 是统一执行接口。** 不管底层是 Lambda、ChatModel、Retriever、Tool 还是子图，运行时都只调 `Runnable`。组件差异被 Bridge Adapter 收敛，调度器不需要知道领域细节。这是"编排层与领域层解耦"的落地。

3. **FieldMapping 是组件复用的关键。** 没有 FieldMapping，每个节点必须为具体上下文定制输入输出结构；有了 FieldMapping，同一个组件可以在不同 Workflow 中复用——只需声明不同的字段映射。

4. **横切面（Callback / EventLog / Checkpoint）属于运行时层，不是业务逻辑。** 观测、恢复、中断如果散在每个组件里，框架就失去统一性。复刻版把这些能力放在 Runtime 层，通过装饰器模式注入。

5. **Schema 是 Provider 与 Runtime 的防火墙。** Provider Adapter 负责把 OpenAI / Claude / Gemini 的原生格式转成 canonical type，上层组件只消费 canonical type。这样模型供应商差异不会污染 Graph。

6. **ReAct 是组合结果，不是魔法。** ReAct agent 就是一张 `compose.Graph(ChatModel + ToolsNode + Branch + State)`。它之所以简单，是因为它站在前面 6 章的肩膀上：Graph 提供编排、Runnable 提供执行、Callback 提供观测、State 提供记忆、Schema 提供语言。

---

## 附录 C：如果现场只有 30 分钟怎么压缩

| 时间段 | 内容 | 形式 |
|--------|------|------|
| 0:00–3:00 | **开场 thesis**：一句话讲 6 条 thesis，在黑板上写 "Graph → Compile → Runnable" 编译边界 | 口述 + 白板 |
| 3:00–8:00 | **Ch01 精华**：Graph vs DAG vs Pregel channel 三种触发语义（只讲概念对比，不走读代码） | 白板画图 |
| 8:00–13:00 | **Ch02 + Ch05 合并**：三层编排对比表（Graph/Workflow/Chain）+ Bridge Adapter 一句话讲 `toLambda()` | 投屏对比表 |
| 13:00–18:00 | **Ch03 精华**：Runnable 四模式 + 降级矩阵（只讲 Stream fallback 一条路径） + Callback 装饰器概念 | 口述 + 一张表 |
| 18:00–23:00 | **Ch04 故事**：`Interrupt → Resume` 一个完整故事（航空公司订票确认） | 口述 |
| 23:00–28:00 | **Ch06 + Ch07 合并**：`Message` vs `AgenticMessage` 对比 + "ReAct = Graph(ChatModel + Tools + Branch + State)" | 投屏两张表 |
| 28:00–30:00 | **总结**：回到 6 条 thesis，说明教学简化边界，布置自学路径 | 口述 |

**砍掉的内容**：
- 所有 Live Coding、代码走读、练习题演示
- Ch02 FieldMapping 细节（只留三层编排对比）
- Ch04 地址匹配细节（只留中断/恢复故事）
- Ch05 教学层 vs 生产层 Bridge 对比
- Ch06 三个 Provider 的详细代码路径
- Ch07 Multi-Agent、StreamToolCallChecker、MessageRewriter/Modifier 细节

**留下的概念**：编译边界、Bridge Adapter 模式、Runnable 降级矩阵、Interrupt/Resume 生命周期、Canonical Schema 隔离、ReAct 是组合。这 6 个概念构成 Eino 的架构骨架，自学附录可补细节。

---

## 教学简化边界声明

本手册所有内容基于 Go 复刻版 `examples/eino-compose-runtime-replica-go/compose/` 和 `agent/` 目录。以下能力在原版 Eino 中存在但复刻版有意简化：

1. **类型推断**：未实现 BFS `toValidateMap` 链式推断。
2. **Stream**：仅 slice-based fake stream，无多后端 Channel/Array/MultiStream/WithConvert/Child。
3. **Stream Copy**：eager copy（先 drain 再切片），无法处理无限流。
4. **FieldMapping（Stream 模式）**：仅支持 Invoke 模式的字段映射。
5. **Checkpoint**：不保存 channel 状态、pending tasks、stream materialization。
6. **Chain Branch**：Lambda 封装而非原版拓扑层 branchRouter，分支内部可观测性较原版弱。
7. **Provider Adapter**：全是 Fake/Skeleton（无真实 SDK 调用）。
8. **Multi-Agent**：简化为单节点 InvokableLambda 封装（原版是完整多节点图结构）。
9. **AgenticMessage**：无 `ResponseMeta` 字段，无法通过 Agentic 路径携带 Provider 扩展元数据。
10. **Concat 流式合并**：未实现 `ConcatMessages`、`concatToolCalls` 等流式合并逻辑。
11. **序列化/反序列化**：无 gob 注册、无持久化层的流与非流值双向转换。

这些简化不是缺陷，而是有意的教学边界：先抓架构主线（编译边界、桥接模式、ReAct 是组合），再在后续实践中补生产细节。

---
*由 Rive Dispatch `disp_2683c56ef16240c0bf8af1eec0155388` 的 Worker `agent_3811529452eb4040a0ba7b03b856f0cc`（Work Node `work_a3e5583df0d14b8cb6f9a38b47c09261`）合成。*
