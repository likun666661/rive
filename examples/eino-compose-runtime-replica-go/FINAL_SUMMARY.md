# Eino Compose Runtime Replica — 最终验证摘要 (第二/三/四/五章 + I3 Bridge Adapter + Checkpoint,教学子集,非完整产品复刻)

## 验证状态

| 命令 | 结果 |
|------|------|
| `gofmt -w .` | ✅ 所有 Go 文件已格式化,无变更 |
| `go build ./...` | ✅ 零编译错误零警告 |
| `go test ./... -count=1` | ✅ compose 包 + agent 包全部 PASS |
| `go vet ./...` | ✅ 静态分析无问题 |
| `go run ./cmd/example` | ✅ 23 个示例全部正常运行 |
| `git diff --check` | ✅ 无空白字符问题 |

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

### 四、第四章: ChatModel + Retriever Component Interfaces (ch4)

> **本章实现独立的 `compose.ChatModel` 和 `compose.Retriever` 接口 (参考 Eino 组件模型第五/六章)、Fake 实现、组件 Lambda 桥接与回调集成、测试覆盖及 I3 桥接审计 (R1/R2)。**

#### 23. ChatModel 接口与组件 (chatmodel.go)
- `ChatModel` 接口: `Generate(ctx, []*Message) (*Message, error)` + `Stream(ctx, []*Message) (StreamReader[*Message], error)`
- `Message{Role, Content}` / `RoleType` (System/Human/Assistant/Tool)
- `FakeChatModel`: 选项模式,默认 echo 行为
- `ChatModelComponent`: `GetRunnable()` 返回 `composableRunnable{i, s}`

#### 24. Retriever 接口与组件 (retriever.go)
- `Retriever` 接口: `Retrieve(ctx, *Query) ([]*Document, error)`
- `Document{Content, Metadata}` / `Query{Text, K}`
- `FakeRetriever`: 支持自定义 `RetrieveFn` + 错误注入
- `NewRetrieverLambda(cfg *RetrieverConfig)`: 包装 Retriever → composableRunnable + CallbackWrapper 集成
- 组件常量: `ComponentOfRetriever = "Retriever"`, `ComponentOfChatModel = "ChatModel"`

#### 25. 四模式降级测试覆盖 (retriever_test.go / chatmodel_test.go)
- ChatModel: 19 个测试 — Invoke/Stream/Collect/Transform 四模式 + 回调 OnStart/OnEnd/OnError/Stream
- Retriever: 17 个测试 — 默认/自定义/错误 Fake + Lambda Invoke/Stream/Collect/Transform + 多 handler + 回调上下文隔离

#### 26. R1 研究提案 (ch4-r1-chatmodel-retriever-contract.md)
- Eino 组件契约分析: ChatModel/Retriever/Message/Document/Tool 类型系统
- Bridge Adapter 三层职责: 方法签名适配 / 组件元数据提取 / 类型安全校验
- Sync vs Stream 语义与 runnablePacker 12 降级函数矩阵
- 回调边界: Typer/Checker 接口、组件级 CallbackInput/Output
- 最小可行实现 (MVP) 路径与 Phase 1-5 优先级

#### 27. R2 桥接审计 (ch4-r2-replica-bridge-audit.md)
- I1 插入点 (Graph ↔ FieldMapping/Workflow/Callback): 14 个桥接点,4 个关键缺口
- I2 插入点 (FieldMapping): 12 个桥接点,4 个关键缺口
- I3 插入点 (Workflow): 14 个桥接点,4 个关键缺口
- **三个关键缺口**:
  1. `validateFieldMapping` 未被 `graph.compile()` 调用 — 类型错误推迟到运行时
  2. GraphBranch 运行时路由缺失 — Workflow 分支不可用 (Chain 通过内联绕过)
  3. `reportSkip` 调用链缺失 — 未选中分支节点永久阻塞
- 线程安全约束全部通过 (无锁安全 / sync.Mutex / sync.Map)
- Chain 层 subGraph 接口已定义,支持嵌套 Chain;Workflow 尚未实现

---

### 五、第四章: Checkpoint / Interrupt / Resume 教学子集

#### 28. 结构化执行地址 (address.go)
- `AddressSegment{Type, ID, SubID}` 表达 runnable / node / tool 等执行层级。
- `Address.String()` 输出稳定表示: `runnable:root;node:approval;tool:lookup:call_1`。
- `AppendAddressSegment(ctx, typ, id, opts...)` 在进入新执行 scope 时扩展地址,并注入 checkpoint 恢复状态。

#### 29. InterruptSignal 树 (interrupt.go)
- `Interrupt` / `StatefulInterrupt` / `CompositeInterrupt` 覆盖简单中断、带状态中断和复合中断。
- `InterruptSignal.Subs` 保留树形结构,`InterruptContext` 提供面向用户的扁平 root cause 视图。
- `SignalToPersistenceMaps` 将信号树落成 `interruptID -> Address/State` map,供 checkpoint 持久化。

#### 30. Resume + CheckPointStore (resume.go / checkpoint.go)
- `WithCheckPoint(ctx, id, store)` 将 checkpoint store/id 注入运行上下文。
- `ResumeWithData` / `BatchResumeWithData` 将恢复数据定向到 interrupt ID。
- `GetInterruptState[T]` 读取同一地址上一次中断保存的状态。
- `GetResumeContext[T]` 区分直接目标 (`hasData=true`) 与通往后代目标的 conduit (`hasData=false`)。

#### 31. Runner 集成 (graph_run.go / graph_manager.go)
- graph runner 为 graph 和 node 自动追加 runnable/node 地址段。
- 节点返回 interrupt error 时,runner 保存 checkpoint 并原样返回 interrupt。
- task manager 使用每个节点自己的 context,确保地址/resume 信息能进入 lambda。

#### 32. Stream 物化示例
- `MaterializeStream[T]` 将一次性 `PipeStreamReader[T]` drain 成 `MaterializedStream[T]`。
- `RestoreStream[T]` 将持久化 chunk 重新包装成 reader。

#### 33. 明确不包含
- 完整 Eino channel checkpoint 复制/恢复。
- 嵌套子图 checkpoint 转发和迁移。
- 序列化类型注册与 checkpoint state migration。
- ToolsNode rerun skip handler。

---

### 六、第五章: PromptTemplate / Tool / ToolsNode 组件桥接

> **本章扩展 I3 Bridge Adapter 模式,实现 PromptTemplate 渲染、Tool 领域接口与 ToolsNode 工具执行节点,支持 Workflow/Chain/Graph 三种编排方式,构建完整的确定性 Tool Calling Pipeline。**

#### 34. ToolCall Schema 数据模型 (schema.go)
- `ToolCall{Index, ID, Type, Function, Extra}` / `ToolCallFunction{Name, Arguments}`: 表示模型发出的工具调用请求,其中 `Index` 支持流式 delta 合并
- `ToolInfo{Name, Desc, ParamsOneOf, Extra}`: 工具元信息 (用于注册和校验)
- `NewParamsOneOfByParams(params)` / `NewParamsOneOfByJSONSchema(schema)`: 工具参数 Schema 的两种构造路径,通过 `ToJSONSchema()` 统一输出
- `ToolResult{Text, Images, Audio, Video, Files}`: 工具执行结果封装
- `Message.ToolCalls` 字段扩展: 在 `chatmodel.go` 的 `Message` 结构体中新增 `ToolCalls []ToolCall`

#### 35. ChatTemplate 接口与 MessageTemplate (prompt.go)
- `ChatTemplate` 接口: `Format(ctx, vs map[string]any) ([]*Message, error)` — 统一提示词渲染合约
- `MessageTemplate`: `NewMessageTemplate(tpl)` + `.WithSystemTemplate(tpl)`,支持 `{{variable}}` 占位符替换 (使用 `regexp.MustCompile`)
- `FakeChatTemplate`: 测试用 mock
- `ChatTemplateComponent`: 将 ChatTemplate 包装为 `composableRunnable`,组件类型 `ComponentOfPrompt = "Prompt"`

#### 36. BridgeTool 领域接口 (prompt_tool_bridge.go)
- `BridgeTool` 接口: `Name() string` + `Execute(ctx, args map[string]any) (string, error)`
- `BridgeToolFunc` / `NewBridgeTool(name, fn)`: 将普通函数包装为 BridgeTool
- 工具通过 `Name()` 唯一标识,在 ToolsNode 中按名匹配

#### 37. ToolsNode 工具执行节点 (prompt_tool_bridge.go)
- `promptTemplateBridge`: 将 `MessageTemplate` 包装为 Lambda (输入 `map[string]any` → 输出 `[]*Message`)
- `toolsNodeBridge`: 输入 `*Message` → 解析 `ToolCalls` → 匹配工具 → 执行 → 返回结果 `*Message`
- `NewPromptTemplateLambda(tmpl)`: 导出构造函数
- `NewToolsNodeLambda(tools...)`: 导出构造函数,从变长参数构建 tools map
- `NewToolsNodeLambdaFromMap(toolMap)`: 从预构建 map 创建
- `Workflow.AsPromptTemplateNode(key, tmpl)`: Workflow 便捷方法
- `Workflow.AsToolsNode(key, tools...)`: Workflow 便捷方法

#### 38. 三编排演示 (cmd/example/main.go)
- **Example 19: Tool Calling Pipeline (Workflow)** — `START → prompt → model1 → tools → model2 → END`,使用 `AsPromptTemplateNode` / `AsToolsNode` 便捷方法
- **Example 20: Tool Calling Pipeline (Chain / Graph)** — Chain 版本 `AppendLambda` 自动连接;Graph 版本 `AddEdge` 手动拓扑,展示最大灵活性
- 全程确定性: 使用 `FakeChatModel` 返回固定 `ToolCall`,使用 `BridgeTool` 返回固定结果

#### 39. 测试覆盖 (prompt_test.go / prompt_tool_bridge_test.go)
- `prompt_test.go`: 6 个测试 — plain/system/missing vars/missing map/FakeChatTemplate/ChatTemplateComponent
- `prompt_tool_bridge_test.go`: 15 个测试 — BridgeToolFunc 包装、PromptTemplate Bridge Lambda、ToolsNode 单工具/多工具/未找到/错误/无效参数、Workflow/Chain/Graph 端到端 pipeline

#### 40. 明确不包含
- Eino 完整 `InvokableTool` / `ToolCallingChatModel` 分层接口
- Streaming tool call (模型边生成边返回 ToolCall)
- Tool rerun skip handler (中断恢复后跳过已执行工具)
- 工具级 Callback 集成
- Provider 特定的工具绑定选项

---

### 七、第六章: Canonical Schema / Stream Concat / Provider Adapters

> **本章将 ChatModel、Retriever、Tool 与 Provider wire format 统一到规范 Schema 层,通过 `Message` / `AgenticMessage` 两种模型展示多 Provider 互操作,并补齐流式 chunk concat 行为。**

#### 41. Canonical Schema 扩展
- `types.go`: 新增 `User` 规范角色、`DataType`、`ChatMessagePartType`。
- `schema.go`: `ToolCall` 增加 `Index` 和 `Extra`;`ToolInfo` 增加 `Extra`;`ParamsOneOf` 支持轻量参数树与完整 JSON Schema;`ToolResult` 支持 text/images/audio/video/files;`Document` 扩展 ID/meta/embedding/score。
- `chatmodel.go`: `Message` 增加 `ToolName`、多模态输入/输出 part、`ResponseMeta`、`ReasoningContent`、`Extra`;`ResponseMeta` 包含 usage/logprobs 和 OpenAI/Claude/Gemini 类型化扩展槽位。

#### 42. Stream concat 注册表
- `concat.go`: `RegisterStreamChunkConcatFunc[T]` 建立类型到 concat 函数的注册表,`ConcatItems` 提供直接合并入口。
- `ConcatMessages`: 拼接内容和 reasoning,按 `ToolCall.Index` 分组合并流式 tool call delta,冲突时报错,保留最后一个非空 `ResponseMeta`。
- `ConcatToolResults`: 拼接文本并追加多模态工具结果。
- `stream.go`: `Concat` 在旧式 `RegisterConcatFunc` 后接入 Chapter 6 concat 注册表,使 `*Message` stream chunk 能折叠成完整消息。

#### 43. Provider Adapter Skeletons
- `provider.go`: 定义 `ContentBlock`、`AgenticMessage`、OpenAI/Claude/Gemini provider interface 和常用内容块构造器。
- `provider_openai.go`: OpenAI role/message/tool_calls/tool_call_id ↔ 规范 `Message`。
- `provider_claude.go`: Claude `text` / `image` / `tool_use` / `tool_result` content blocks ↔ `AgenticMessage`。
- `provider_gemini.go`: Gemini `parts` / `functionCall` / `functionResponse` ↔ `AgenticMessage`,并提供经典 `Message` 路径。
- `FakeOpenAIProvider` / `FakeClaudeProvider` / `FakeGeminiProvider`: 教育桩实现,用于证明下游 ChatModel/Retriever/Tool 消费规范类型而不感知 Provider 来源。

#### 44. 测试覆盖
- `schema_test.go`: schema 字段、JSON Schema 输出、多模态 ToolResult、Document、Message metadata。
- `concat_test.go`: concat 注册表、Message 内容/reasoning/tool call delta、ToolResult 多模态合并、`Concat` stream pipe 集成。
- `provider_test.go`: OpenAI/Claude/Gemini 双向转换、round trip、fake provider 接口、跨 Provider 到 ChatModel/Retriever/Tool 的消费示例。

#### 45. 明确不包含
- 外部 Provider SDK 调用、鉴权、真实 HTTP 请求。
- 完整 Eino `schema/openai` / `schema/claude` / `schema/gemini` 子包拆分。
- 完整 AgenticMessage block 全量枚举、序列化注册和 provider-specific streaming protocol。

---

### 八、第七章: Agent Flow — ReAct / Host Multi-Agent

> **本章将可复用的 LLM 应用 pattern 编码为 compose.Graph 构建器,包括 ReAct 推理-行动循环 (agent/react.go) 和 Host Multi-Agent 多专家路由 (compose/multiagent.go)。不引入独立的 "agent runtime"。**

#### 46. ReAct Agent Graph Builder (agent/react.go)
- `NewAgent(config)` 构建 `START → ChatModel → (ToolCall? Tools → ChatModel : END)` 循环图
- 通过 `AnyPredecessor` + Pregel 实现循环,`MaxStep` 作为安全上限
- `reactState{Messages, ReturnDirectlyToolCallID}` 通过 `WithGenLocalState` 管理
- `modelPreHandle`: 追加本轮输入 → MessageRewriter (持久化) → MessageModifier (临时) → 返回处理后的消息给 ChatModel
- `toolsNodePreHandle`: 追加 model 的 tool call → 判断 return directly
- `modelPostBranchCondition`: 检查 `msg.ToolCalls` → 路由到 Tools 或 END
- `buildReturnDirectly`: Tools 后分支 → 如果标记 return directly → direct_return lambda → END
- `DefaultStreamToolCallChecker`: OpenAI 风格的 "first chunk" 启发式
- `ScanAllStreamToolCallChecker`: text-before-tool-call provider 的完整扫描示例
- `SetReturnDirectly(ctx)`: 运行时标注当前工具调用直接返回

#### 47. MessageRewriter vs MessageModifier (agent/react.go)
- `MessageRewriter`: 修改 `state.Messages` 本身 (持久化),用于上下文压缩
- `MessageModifier`: 修改 `state.Messages` 的副本 (临时),用于注入 system prompt
- 执行顺序: Rewriter 先执行,Modifier 后执行
- 测试验证: `TestReAct_MessageRewriter_Ordering` 验证了 Modifier 能看到 Rewriter 的更改

#### 48. Tool Return Directly 双机制 (agent/react.go)
- 配置级: `AgentConfig.ToolReturnDirectly` map 声明哪些工具直接返回
- 运行时: 工具实现中调用 `SetReturnDirectly(ctx)` 标注
- 双机制共存: `TestReAct_SetReturnDirectly_Priority` 验证运行时优先

#### 49. Host Multi-Agent Graph Builder (compose/multiagent.go)
- `NewMultiAgent(config)` 构建多专家路由图
- 验证: nil config / nil host / empty specialists / duplicate specialists / empty specialist name / nil specialist in list
- Host ChatModel 通过 ToolCall 选择 specialist(s)
- 每个 specialist 接收完整用户消息历史 (`state.Msgs`),而非 ToolCall 参数
- 复刻版把 Host 路由、specialist 执行和聚合封装在一个 `multi_agent_core` graph node 内,保留逻辑拓扑而不复制生产版所有内部 branch 节点

#### 50. Specialist 三种形式 (compose/multiagent.go)
- **ChatModel**: `spec.ChatModel.Generate()` + 可选的 `SystemPrompt` (通过 pre-handler 注入)
- **Invokable**: `spec.Invokable(ctx, input)` 直接调用
- **Streamable**: `spec.Streamable(ctx, input)` 流式 → 收集后返回最后一条消息的 Content
- 优先级: ChatModel → Invokable → Streamable

#### 51. Summarizer 两种模式 (compose/multiagent.go)
- **默认**: 将多个 specialist 回答拼接为 `[name]: content`
- **自定义**: 通过 `Summarizer{ChatModel, SystemPrompt}` 用 ChatModel 做总结
- 测试验证: `TestMultiAgent_CustomSummarizerWithSystemPrompt` 验证 system prompt 注入

#### 52. State 基础设施 (compose/state.go)
- `WithGenLocalState[T]`: per-run state 工厂,graph 启动时调用
- `ProcessState[T](ctx, fn)`: 在 pre-handler / 工具中读/写 state
- `GetState[T](ctx)`: 只读检索 state
- `WithNodePreHandler(fn)`: 注册节点输入预处理器
- `SetToolCallID / GetToolCallID`: context 级工具调用 ID 传递
- `ToolsNode` / `ToolsNodeConfig` / `InvokableTool`: 工具执行节点

#### 53. 测试覆盖 (agent/react_test.go + compose/multiagent_test.go)
- `agent/react_test.go`: 23 个测试 — NoTools / SingleToolCall / MultiRoundToolCall / MaxStepEnforced / ReturnDirectly_Config / ReturnDirectly_Runtime / MessageModifier_Persistent / MessageRewriter_Compression / MessageRewriter_Ordering / StreamToolCallChecker_Default / StreamToolCallChecker_ClaudeStyle / StreamToolCallChecker_ScanAllNoToolCall / EmptyInput / NilConfig / NilChatModel / NoToolsConfig / StateIsolation / SetReturnDirectly_Priority / StreamMode_Basic / LargeMultiRound / ToolCallWithMultipleTools
- `compose/multiagent_test.go`: 25 个测试 — SingleSpecialist_SingleIntent / MultiSpecialist_MultiIntent / NoSpecialist_DirectAnswer / Specialist_ChatModel / Specialist_Invokable / Specialist_Streamable / PreHandler_InputReplacement / DefaultSummarization / CustomSummarizer / InvalidSpecialistName / EmptySpecialists / NilHostChatModel / NilConfig / DuplicateSpecialistNames / SpecialistWithSystemPrompt / StateIsolation / MultipleToolCallsSameSpecialist / SpecialistEmptyName / HostModelError / SpecialistError / CustomSummarizerWithSystemPrompt / NilSpecialistInList / Stream / LargeMultiIntent / AgentAsSpecialist
- `compose/state_test.go`: 11 个 State 基础设施测试

#### 54. 明确不包含
- 生产级 `ToolCallingModel` 接口 (模型级 tool binding)
- Claude/Gemini 专用完整 `StreamToolCallChecker`
- `HandOff` callback (Host → Specialist 事件)
- Agent 级 Interrupt/Resume
- `ExportGraph` / 动态图修改
- Agent Option 双通道多态
- `WithMessageFuture` agent 级 callback future
- Streaming ToolsNode (工具仅 Invoke 模式)
- 增强型多模态 ToolResult

---

## 关键文件导览

| 文件 | 职责 |
|---|---|
| `types.go` | 类型常量 (NodeTriggerMode, ComponentType, runType)、START/END 哨兵、sentinel errors |
| `runnable.go` | Runnable[I,O] 接口、composableRunnable、Lambda、InvokableLambda 泛型构造函数 |
| `runnable_test.go` | 12 测试: 四模式降级矩阵、类型转换、graphRunnable 流回退 |
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
| `field_mapping_test.go` | 28+ 测试: 6 构造器、validateFieldMapping、fieldMap、takeOne、convertTo |
| `workflow.go` | Workflow[I,O] / WorkflowNode / WorkflowBranch / AddInput / AddDependency / SetStaticValue / compile |
| `workflow_test.go` | 16 测试: 基本链式、Fan-in、路径冲突、staticValue、并发 |
| `chain.go` | Chain[I,O] Builder / AppendLambda / AppendParallel / AppendBranch / addNode / preNodeKeys |
| `chain_parallel.go` | Parallel / AddLambda / outputKey 冲突检测 |
| `chain_branch.go` | ChainBranch / NewChainBranch / NewChainMultiBranch / AddLambda |
| `chain_test.go` | 17 测试: 线性/Parallel/Branch/MultiBranch/子图嵌套/编译锁 |
| `introspect.go` | GraphInfo、GraphNodeInfo、GraphEdgeInfo (编译时拓扑导出) |
| `event_log.go` | EventLog、10 种事件类型、线程安全记录与格式化 |
| `utils.go` | 辅助工具函数 |
| `bridge.go` | Bridge Adapter: Retriever/ChatModel 领域接口 + toLambda() 桥接函数 + Workflow 便捷方法 |
| `bridge_test.go` | Bridge Adapter 测试: 7 个测试,覆盖独立 Lambda + RAG pipeline 端到端 |
| `retriever.go` | Retriever 接口 (Retrieve)、Document/Query 类型、FakeRetriever、RetrieverConfig、NewRetrieverLambda |
| `retriever_test.go` | 17 测试: FakeRetriever 三模式、Lambda 四模式降级、回调集成、多 handler |
| `chatmodel.go` | ChatModel 接口 (Generate/Stream)、Message/RoleType、FakeChatModel、ChatModelComponent |
| `chatmodel_test.go` | 19 测试: 消息构造、FakeChatModel 四模式、ChatModelComponent 四模式降级、回调集成 |
| `stream.go` | PipeStreamReader/PipeStreamWriter、Copy、Merge、Concat |
| `stream_test.go` | 20 测试: Pipe/Copy/Merge/Concat 并发安全 |
| `callbacks.go` | RunInfo、Handler、HandlerBuilder、CallbackWrapper、流输入/输出副本 |
| `schema.go` | ToolCall / ToolInfo / ParamsOneOf / ToolResult / Document 规范数据模型 |
| `schema_test.go` | Chapter 6 schema 字段、JSON Schema、多模态结果测试 |
| `concat.go` | Chapter 6 stream chunk concat 注册表和 Message/ToolResult 合并规则 |
| `concat_test.go` | Chapter 6 concat 注册表、tool call delta、stream pipe 集成测试 |
| `provider.go` | ContentBlock / AgenticMessage 规范类型和 provider interface |
| `provider_openai.go` | OpenAI message/request ↔ 规范 Message adapter |
| `provider_claude.go` | Claude content blocks ↔ 规范 AgenticMessage adapter |
| `provider_gemini.go` | Gemini parts/functionCall/functionResponse ↔ 规范类型 adapter |
| `provider_test.go` | Provider adapter round trip 与跨组件消费测试 |
| `prompt.go` | ChatTemplate 接口、MessageTemplate ({{variable}} 替换)、ChatTemplateComponent |
| `prompt_test.go` | 6 测试: MessageTemplate / FakeChatTemplate / ChatTemplateComponent |
| `prompt_tool_bridge.go` | BridgeTool 接口、promptTemplateBridge / toolsNodeBridge、NewToolsNodeLambda、Workflow 便捷方法 |
| `prompt_tool_bridge_test.go` | 15 测试: BridgeToolFunc / PromptTemplate / ToolsNode / 三编排端到端 pipeline |
| `callbacks_test.go` | 25 测试: 5 阶段时序、上下文隔离、TimingChecker、CbStreamReader |
| `graph_test.go` | 80+ 测试: DAG/Pregel/边界/EventLog/Branch/Callback 集成 |
| `cmd/example/main.go` | 综合示例程序 (23 个场景,覆盖 Graph/DAG/Pregel/FieldMapping/Workflow/Chain/Parallel/Branch/Stream/Collect/Transform/Callback/Bridge/RAG/ToolCallingPipeline/Checkpoint/Schema/Provider/ReAct/Host) |
| `agent/react.go` | ReAct Agent Graph Builder (NewAgent, pre-handler, branch, DefaultStreamToolCallChecker) |
| `agent/types.go` | AgentConfig, MessageRewriter, MessageModifier, StreamToolCallChecker, reactState |
| `agent/react_test.go` | 23 个 ReAct Agent 测试 |
| `compose/multiagent.go` | Host Multi-Agent Graph Builder (NewMultiAgent, Specialist, Summarizer) |
| `compose/multiagent_test.go` | 25 个 Host Multi-Agent 测试 |
| `compose/state.go` | State 基础设施 (WithGenLocalState, ProcessState, GetState, WithNodePreHandler, ToolsNode) |
| `compose/state_test.go` | 11 个 State 基础设施测试 |
| `research/` | 研究文档: ch2/ch3/ch4/ch5/ch6/ch7 实现契约、差距审计、验证记录 |

---

## 如何运行

```bash
# 工作目录
cd examples/eino-compose-runtime-replica-go

# 格式化
gofmt -w .

# 编译 + 静态分析
go build ./...
go vet ./...

# 运行测试 (禁用缓存)
go test ./... -count=1

# 运行综合示例 (20 个场景)
go run ./cmd/example/

# 检查空白字符 (仓库根目录)
git diff --check
```

---

## 明确未实现的边界

**本复刻版是教育子集 (educational subset)。ChatModel/Retriever 组件接口已实现 (Chapter 4),Bridge Adapter 模式已演示 (I3),PromptTemplate/Tool/ToolsNode 桥接已实现 (Chapter 5),Canonical Schema / Stream Concat / Provider Adapter 已实现 (Chapter 6),ReAct Agent + Host Multi-Agent 已实现 (Chapter 7)。以下为明确未实现的部分:**

本复刻版聚焦于 Eino Compose Runtime 的核心图编译与执行引擎。以下为明确未实现的部分:

### 运行时不支持
- **组件桥接 (ChatModel/Tool/Retriever)**: Bridge Adapter 模式 (bridge.go) 已展示 Workflow 声明式桥接。ChatModel/Retriever 独立接口 (retriever.go/chatmodel.go) 已实现。Tool bridge 已实现 (prompt_tool_bridge.go)。Embedding bridge 未实现。
- **图级 Stream 执行管线**: Runnable 四模式已经实现,但 graph runner 主路径仍以 Invoke 为主
- **streamFieldMap 流式映射**: 依赖图级 stream channel,当前未接入 (见 `field_mapping.go:448` stub)
- **Stream ChainBranch**: 流式分支暂未接入 Chain Builder
- **validateFieldMapping 编译时调用**: `validateFieldMapping()` 已完整实现但 `graph.compile()` 未调用 — 类型错误推迟到运行时 (GAP-I1-1)
- **GraphBranch 运行时路由**: Workflow 分支不可用 (GAP-I1-2)。Chain 通过内联分支评估绕过。
- **State 传递 (graph.state)**: 字段已定义但未使用
- **持久化/分布式 Checkpoint Store**: Checkpoint / Interrupt / Resume 已有内存教学实现,但未接持久化后端或分布式恢复
- **Fan-in 智能合并 (Merge 配置)**: 当前默认 map[string]any 合并

### 周边工具未实现
- **可视化 / DOT 导出**: 无 graph 拓扑可视化
- **JSON Schema 校验**: 无编译时 node 输入输出类型的 schema 校验
- **DevOps 工具**: 无 tracing / metrics / profiling 集成
- **组件级 Callback 桥接**: CallbackWrapper 已实现并在 `NewRetrieverLambda` 中集成。ChatModel/Tool 的 Typer/Checker 接口与组件级 CallbackInput/Output 未实现。

### 类型系统局限
- `fmtType()` 仅覆盖 `string/int/float64/bool` 四种基础类型,其余返回 `"any"`

### 第七章 Agent Flow 明确排除
- **Agent Option 双通道多态**: 不实现 `composeOptions` + `implSpecificOptFn` 双通道,仅暴露显式 option 函数
- **HandOff Callback**: 不实现 Host ↔ Specialist 专用回调,复用 graph 级通用 callback
- **Agent 级 Interrupt/Resume**: Ch4 的 checkpoint/interrupt 在 graph 层已实现,但不与 agent loop 深度集成
- **Claude/Gemini 专用 StreamToolCallChecker**: 仅提供默认 OpenAI 风格 checker + 注入点
- **Streaming ToolsNode**: 工具仅 Invoke 模式 (匹配 Ch5 教育子集范围)
- **增强型 ToolResult**: tool result 仅 string 类型
- **ExportGraph / 动态图修改**: agent 图一次性构建不可修改

---

### I3: Bridge Adapter — 领域组件参与通用图运行时

> **本章实现 Bridge Adapter 模式: 为 Retriever / ChatModel 定义领域接口,通过 bridge 适配器包装为 Lambda,使其能在 Workflow/Graph/Chain 三层编排中参与图运行时,并以 RAG pipeline 为教学示例。Chapter 4 (retriever.go/chatmodel.go) 提供接近 Eino 正式实现的独立组件接口与测试。**

#### 28. 领域接口定义 (bridge.go)
- `BridgeRetriever` 接口: `Retrieve(ctx, query string) ([]*BridgeDocument, error)`
- `BridgeChatModel` 接口: `Generate(ctx, messages []*BridgeMessage) (string, error)`
- `BridgeDocument` / `BridgeMessage`: 领域数据传输对象
- `retrieverBridge` / `chatModelBridge` / `promptAssemblerBridge`: bridge 适配结构体

#### 29. toLambda() 桥接函数
- 每个 bridge 实现 `toLambda()` 方法,将领域接口包装为 `InvokableLambda`
- 零侵入: 组件不需要依赖 compose 包,只需实现领域接口
- 零修改: 图运行时 (graph/runner) 不需要改动任何代码

#### 30. Workflow 便捷方法
- `AsRetrieverNode(key, retriever)`: 桥接 BridgeRetriever → Workflow Lambda 节点
- `AsChatModelNode(key, model)`: 桥接 BridgeChatModel → Workflow Lambda 节点
- `AsPromptAssemblerNode(key, systemPrompt)`: 创建提示词组装 Lambda 节点

#### 31. RAG Pipeline 端到端测试 (bridge_test.go)
- 7 个单元测试,覆盖:
  - retriever/chatModel/promptAssembler 独立 Lambda 测试
  - 完整 RAG 流程: `query → retriever → assemble(prompt) → model → END`
  - FieldMapping 在异质节点间的字段级聚合验证
  - 便捷方法 (AsRetrieverNode/AsChatModelNode) 创建验证

#### 32. RAG Pipeline Demo (cmd/example/main.go)
- Example 16: 可运行的 RAG 流水线,使用 mock Retriever + mock ChatModel
- Example 17: Bridge Adapter 模式架构图 + 五个核心设计原理说明
- 展示 FieldMapping 衔接异质类型节点 (string → []*BridgeDocument → []*BridgeMessage → string)

#### 33. Chapter 4 独立组件接口 (retriever.go/chatmodel.go)
- `ChatModel` 接口正式实现: `Generate` + `Stream` 双模式,`Message`/`RoleType` 类型
- `Retriever` 接口正式实现: `Retrieve`, `Document`/`Query` 类型
- 与 bridge.go 互补: bridge.go 展示 Workflow 声明式桥接,retriever.go/chatmodel.go 展示 Eino 正式组件体系

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
12. **Bridge Adapter (I3)**: 领域接口与图运行时之间的无侵入适配层,让 Retriever/ChatModel 参与图编排
13. **Chapter 4 Component Interfaces**: 独立的 ChatModel/Retriever 接口 + Fake 实现 + Lambda 桥接 + 四模式降级测试
14. **桥接审计 (R1/R2)**: 六层抽象 90%+ 完成度,3 个关键缺口已在 `research/ch4-r2-replica-bridge-audit.md` 记录
15. **Chapter 5 PromptTemplate / Tool / ToolsNode**: ChatTemplate 接口 + MessageTemplate 渲染 + BridgeTool 领域接口 + ToolsNode 工具执行 + Workflow/Chain/Graph 三编排,全程确定性无外部依赖
16. **Chapter 6 Canonical Schema / Stream Concat / Provider Adapters**: Message / AgenticMessage 双消息模型 + ToolCall/ToolResult 规范 + ConcatItems stream 合并 + OpenAI/Claude/Gemini Provider Adapter 骨架
17. **Chapter 7 Agent Flow**: ReAct 推理-行动循环 (Graph Builder 编码) + Host Multi-Agent 多专家路由 (Specialist-as-Tool) + State 基础设施 + MessageRewriter/Modifier 双语义 + Tool Return Directly 双机制 + Address 隔离 Callback Handler
