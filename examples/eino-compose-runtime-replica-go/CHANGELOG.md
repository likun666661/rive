# CHANGELOG — 第二/三/四/五/六章 & I3 Bridge Adapter 示例与文档更新

本文档记录对 `examples/eino-compose-runtime-replica-go` 的第二章 (FieldMapping / Workflow / Chain / Parallel / Branch)、第三章 (Runnable Stream / Collect / Transform / Callback)、第四章 (ChatModel + Retriever 组件接口)、第五章 (PromptTemplate / Tool / ToolsNode)、第六章 (Canonical Schema / Stream Concat / Provider Adapters)、I3 Bridge Adapter 与桥接审计 (R1/R2) 的示例补全与文档更新。

---

## Ch6: Canonical Schema / Stream Concat / Provider Adapter 教学子集

参考 Eino 技术手册第六章,将前文的 ChatModel、Retriever、Tool 和 Provider 消息格式收敛到规范 Schema 层,补齐流式 chunk concat 注册表和 OpenAI / Claude / Gemini provider adapter 骨架。

### 变更范围

| 文件 | 变更类型 | 说明 |
|------|---------|------|
| `compose/types.go` | 更新 | 新增 `User` 规范角色、`DataType`、`ChatMessagePartType` |
| `compose/schema.go` | 更新 | 扩展 `ToolCall.Index/Extra`、`ToolInfo.Extra`、`ParamsOneOf` 双模式 JSON Schema 输出、`ToolResult` 多模态字段,并将 `Document` 作为规范类型 |
| `compose/chatmodel.go` | 更新 | `Message` 扩展 `ToolName`、多模态 content part、`ResponseMeta`、`ReasoningContent`、`Extra`;新增 provider extension metadata 类型 |
| `compose/concat.go` | 新增 | `RegisterStreamChunkConcatFunc` / `ConcatItems` 注册式合并,实现 `ConcatMessages`、`ConcatMessageArray`、`ConcatToolResults` |
| `compose/stream.go` | 更新 | `Concat` 支持 Chapter 6 concat 注册表,用于流式 Message chunk 折叠 |
| `compose/provider.go` | 新增 | `ContentBlock` / `AgenticMessage` 教育子集和 provider interface 定义 |
| `compose/provider_openai.go` | 新增 | OpenAI chat message 与规范 `Message` 双向转换 |
| `compose/provider_claude.go` | 新增 | Claude content blocks 与规范 `AgenticMessage` 双向转换 |
| `compose/provider_gemini.go` | 新增 | Gemini parts/functionCall/functionResponse 与 `AgenticMessage`、`Message` 双路径转换 |
| `compose/schema_test.go` / `compose/concat_test.go` / `compose/provider_test.go` | 新增 | 覆盖 schema API、stream concat 行为、provider adapter round trip 和跨组件消费模式 |
| `research/ch6-*.md` | 新增 | 第六章差距审计、provider schema 契约、实现契约和验证记录 |
| `README.md` / `FINAL_SUMMARY.md` | 更新 | 记录第六章具体模式、边界和运行验证 |

### 设计要点

- **经典消息模型**: `Message` 保持 role-driven chat 语义,assistant 携带 `ToolCalls`,tool 消息通过 `ToolCallID` 回指。
- **AgenticMessage 模型**: `ContentBlock` 表示 Claude/Gemini 风格的 text/image/tool_use/tool_result 内容块,工具调用与结果不依赖单独的 `tool` 角色。
- **Provider 扩展槽位**: `ResponseMeta` 使用 `OpenAIExtension`、`ClaudeExtension`、`GeminiExtension` 类型化指针保存 provider 特定元数据。
- **Stream concat**: `ToolCall.Index` 用于合并同一工具调用的流式 delta,参数字符串按到达顺序拼接,索引冲突返回错误。
- **教育边界**: Adapter 是本地 fake/skeleton,不调用外部 Provider SDK;重点展示原生线格式与规范类型之间的转换模式。

---

## Ch4B: Checkpoint / Interrupt / Resume 教学子集

参考 Eino 技术手册 `04-checkpoint-interrupt-resume.md`,实现可运行的教育子集: 结构化执行地址、InterruptSignal 树、checkpoint store、resume data 路由和 graph runner interrupt 保存/恢复。

### 变更范围

| 文件 | 变更类型 | 说明 |
|------|---------|------|
| `compose/address.go` | 新增 | `Address` / `AddressSegment` / `AppendAddressSegment`,支持 runnable/node/tool 分层地址和 SubID |
| `compose/interrupt.go` | 新增 | `Interrupt` / `StatefulInterrupt` / `CompositeInterrupt`,`InterruptSignal` 树和 `InterruptContext` 扁平视图 |
| `compose/resume.go` | 新增 | `ResumeWithData` / `BatchResumeWithData` / `GetInterruptState` / `GetResumeContext` |
| `compose/checkpoint.go` | 新增 | `CheckPointStore`、`InMemoryCheckPointStore`、`WithCheckPoint`、stream materialization helper |
| `compose/graph_run.go` | 更新 | runner 在 graph/node scope 追加地址,interrupt 时保存 checkpoint 并返回 interrupt error |
| `compose/graph_manager.go` | 更新 | task 执行使用每个节点自己的 context,让地址/resume 信息进入 lambda |
| `compose/checkpoint_test.go` | 新增 | 覆盖地址字符串、graph interrupt/resume、复合中断、conduit resume、stream 物化 |
| `cmd/example/main.go` | 更新 | 新增 Example 18: Checkpoint / Interrupt / Resume |
| `research/ch4-checkpoint-interrupt-resume-contract.md` | 新增 | 研究笔记: Chapter 04 问题、设计思路、复刻版边界 |
| `README.md` / `FINAL_SUMMARY.md` | 更新 | 新增第四章 checkpoint 教学说明 |

### 教学边界

当前实现刻意保留为教育子集: 不复制 Eino 完整 channel checkpoint、子图 checkpoint 转发、序列化注册、状态迁移和工具 rerun skip handler。它解决的是复刻框架里最关键的四个模式: 结构化地址、树状中断信号、持久 checkpoint、定向恢复数据分发。

---

## Ch4: Chapter 4 — ChatModel + Retriever Component Interfaces

参考 Eino 组件模型第五章 (ChatModel) 和第六章 (Retriever),实现独立的 `compose.ChatModel` 和 `compose.Retriever` 接口、Fake 实现、组件 Lambda 桥接与回调集成。

### 变更范围

| 文件 | 变更类型 | 说明 |
|------|---------|------|
| `compose/chatmodel.go` | 新增 | ChatModel 接口 (Generate/Stream)、Message/RoleType 类型、FakeChatModel、ChatModelComponent |
| `compose/chatmodel_test.go` | 新增 | 19 个单元测试: 消息构造、FakeChatModel 默认/自定义/流式、ChatModelComponent 四模式降级、回调集成 |
| `compose/retriever.go` | 新增 | Retriever 接口 (Retrieve)、Document/Query 类型、FakeRetriever、RetrieverConfig、NewRetrieverLambda |
| `compose/retriever_test.go` | 新增 | 17 个单元测试: FakeRetriever 默认/自定义/错误、NewRetrieverLambda 四模式降级、回调集成、多 handler |
| `compose/bridge.go` | 已有 | Bridge Adapter 模式 (BridgeDocument/BridgeMessage/BridgeRetriever/BridgeChatModel + Workflow 便捷方法) |
| `compose/bridge_test.go` | 已有 | 7 个单元测试: RAG pipeline 端到端 + FieldMapping 聚合验证 |
| `compose/runnable_test.go` | 新增 | 12 个测试: 四模式降级矩阵、类型转换、graphRunnable 流回退 |
| `compose/stream_test.go` | 新增 | 20 个测试: Pipe/Copy/Merge/Concat 并发安全 |
| `compose/callbacks_test.go` | 新增 | 25 个测试: 5 阶段时序、上下文隔离、TimingChecker、CbStreamReader |
| `compose/workflow_test.go` | 新增 | 16 个测试: 链式/Fan-in/路径冲突/staticValue/并发 |
| `research/ch4-r1-chatmodel-retriever-contract.md` | 新增 | R1 研究笔记: ChatModel/Retriever 组件契约与复刻版桥接需求 |
| `research/ch4-r2-replica-bridge-audit.md` | 新增 | R2 审计: 当前复刻版 I1/I2/I3 桥接插入点分析、关键缺口、修复优先级 |

### ChatModel 组件设计

- **接口**: `ChatModel.Generate(ctx, []*Message) (*Message, error)` + `Stream(ctx, []*Message) (StreamReader[*Message], error)`
- **类型**: `Message{Role, Content}`, `RoleType` (system/human/assistant/tool)
- **FakeChatModel**: 选项模式 (WithChatGenerateFunc/WithChatStreamFunc),默认 echo 行为
- **ChatModelComponent**: `GetRunnable()` 返回 `composableRunnable{i, s}`,支持 Invoke/Stream 双模式
- **组件常量**: `ComponentOfChatModel = "ChatModel"`

### Retriever 组件设计

- **接口**: `Retriever.Retrieve(ctx, *Query) ([]*Document, error)`
- **类型**: `Document{Content, Metadata}`, `Query{Text, K}`
- **FakeRetriever**: 支持 Retriever 接口 + `RetrieveFn` 自定义函数 + `Err` 错误注入
- **NewRetrieverLambda**: 将 Retriever 包装为 `composableRunnable{i}`,支持 CallbackWrapper
- **组件常量**: `ComponentOfRetriever = "Retriever"`

### 与 bridge.go I3 Bridge Adapter 的关系

| 文件 | 接口/类型 | 用途 |
|------|----------|------|
| `bridge.go` | `BridgeRetriever (string query)`, `BridgeChatModel (string output)`, `BridgeDocument`, `BridgeMessage` | I3 教学用简化 Bridge,专为 Workflow `As*Node` 便捷方法设计 |
| `retriever.go` | `Retriever (*Query query)`, `Document`, `Query` | 接近 Eino 正式接口,支持 Callback、Lambda 包装 |
| `chatmodel.go` | `ChatModel (*Message + Stream)`, `Message`, `RoleType` | 接近 Eino 正式接口,支持 Invoke/Stream 双模式 |

两套实现互补: bridge.go 展示 Workflow 层的声明式桥接,RAG pipeline 示例 (example16) 通过 `AsRetrieverNode/AsChatModelNode/AsPromptAssemblerNode` 串联; retriever.go/chatmodel.go 展示 Eino 正式组件体系与运行时集成模式。

### Ch4 桥接审计 (R2) 关键发现

六层抽象 (Runnable/Stream/Callbacks/FieldMapping/Graph/Workflow/Chain) 均已实现至 90%+。三个关键桥接缺口:

1. **validateFieldMapping 编译时调用缺失**: `field_mapping.go` 中 `validateFieldMapping()` 已完整实现,但 `graph.compile()` 未调用它。类型错误推迟到运行时才发现。
2. **GraphBranch 运行时路由缺失**: Workflow 的分支依赖关系和 `noDataFlow=true` 语义在运行时不执行。Chain 通过内联分支评估绕过。
3. **reportSkip 调用链缺失**: 多分支未选中节点会被永久阻塞而不被 skip。

详见 `research/ch4-r2-replica-bridge-audit.md`。

---

## I3: Bridge Adapter — ChatModel + Retriever RAG Pipeline 演示

### 变更范围

| 文件 | 变更类型 | 说明 |
|------|---------|------|
| `compose/bridge.go` | 新增 | Bridge Adapter 模式: Retriever/ChatModel/Message/Document 领域接口 + bridge 适配器 + Workflow 便捷方法 |
| `compose/bridge_test.go` | 新增 | 7 个单元测试: retriever/chatmodel/promptAssembler Lambda 测试 + RAG pipeline Workflow 端到端测试 |
| `cmd/example/main.go` | 更新 | 新增 2 个 I3 示例 (example16/17),示例总数增至 17 |
| `README.md` | 更新 | 新增 I3 Bridge Adapter 模式章节,含架构图、RAG pipeline 示例、便捷方法对照表 |
| `CHANGELOG.md` | 更新 | 本文件 |
| `FINAL_SUMMARY.md` | 更新 | 新增 I3 Bridge Adapter 摘要 |

### 新增示例说明

#### Example 16: RAG Pipeline (Retriever → Prompt Assembly → ChatModel)
- 完整 RAG 流水线: 用户提问 → 文档检索 → 提示词组装 → 模型生成
- 拓扑: `START → retriever → assemble → model → END`
- 展示 FieldMapping 在异质节点间的字段级数据聚合 (query + documents → prompt messages)
- mockRetriever/mockChatModel 实现 compose.Retriever/compose.ChatModel 接口
- AsRetrieverNode / AsChatModelNode / AsPromptAssemblerNode 便捷桥接方法

#### Example 17: Bridge Adapter 模式说明
- 三层架构图: 领域层 → 桥接层 → 运行时
- 五个桥接原理: 统一合约 (Lambda) / 接口隔离 / 零侵入 / FieldMapping 衔接 / 三重抽象复用
- 扩展清单: Tool bridge / StreamChatModel bridge / Embedding bridge

### Bridge Adapter 设计要点

1. **领域接口**: Retriever.Retrieve / ChatModel.Generate — 与 graph 运行时无关
2. **桥接函数**: toLambda() 将领域接口包装为 composableRunnable → AddLambdaNode
3. **Workflow 便捷方法**: AsRetrieverNode / AsChatModelNode / AsPromptAssemblerNode 在 Workflow 上提供声明式桥接
4. **FieldMapping 衔接**: MapFields("", "query") (FromAll) + ToField("documents") 实现异质类型节点间的数据聚合
5. **零侵入**: 组件开发者仅实现领域接口,包外的 bridge 适配器完成图运行时接入

---

## Ch5: Chapter 5 — PromptTemplate / Tool / ToolsNode 组件桥接

参考 Eino 技术手册第五章 (`05-components-model-tool-prompt.md`),扩展 I3 Bridge Adapter 模式,增加 PromptTemplate 渲染、Tool 领域接口与 ToolsNode 工具执行节点,实现完整的确定性 Tool Calling Pipeline。

### 变更范围

| 文件 | 变更类型 | 说明 |
|------|---------|------|
| `compose/schema.go` | 新增 | `ToolCall` / `ToolCallFunction` / `ToolInfo` / `ParamsOneOf` / `ParameterInfo` / `ToolResult` 数据模型 |
| `compose/prompt.go` | 新增 | `ChatTemplate` 接口 (`Format`)、`MessageTemplate` (带 `WithSystemTemplate` 和 `{{variable}}` 替换)、`ChatTemplateComponent`、`FakeChatTemplate` |
| `compose/prompt_test.go` | 新增 | 6 个测试: MessageTemplate 简单/系统/缺失变量/缺失map/FakeChatTemplate/ChatTemplateComponent |
| `compose/prompt_tool_bridge.go` | 新增 | `BridgeTool` 接口、`BridgeToolFunc` / `NewBridgeTool`、`promptTemplateBridge` / `toolsNodeBridge` 适配器、`NewPromptTemplateLambda` / `NewToolsNodeLambda` / `NewToolsNodeLambdaFromMap`、`Workflow.AsPromptTemplateNode` / `Workflow.AsToolsNode` 便捷方法 |
| `compose/prompt_tool_bridge_test.go` | 新增 | 15 个测试: BridgeToolFunc / PromptTemplate / ToolsNode 单工具/多工具/未找到/错误/无效参数,Workflow/Chain/Graph 三编排端到端 |
| `cmd/example/main.go` | 更新 | 新增 Example 19 (Workflow) 和 Example 20 (Chain/Graph) Tool Calling Pipeline,示例总数增至 20 |
| `README.md` | 更新 | 新增第五章功能章节,架构总览增加 PromptTemplate/Tool/ToolsNode,更新便捷方法表、扩展清单、未实现边界、包结构、示例计数 |
| `CHANGELOG.md` | 更新 | 本文件 |
| `FINAL_SUMMARY.md` | 更新 | 新增第五章功能摘要,更新验证状态 |
| `research/ch5-implementation-contract.md` | 新增 | 研究笔记: Chapter 05 组件契约与实现计划 |
| `research/ch5-r1-component-gap-audit.md` | 新增 | 差距审计: 当前复刻版 vs Eino Chapter 05 组件契约 |

### 新增示例说明

#### Example 19: Tool Calling Pipeline (Workflow)
- 完整 pipeline: `PromptTemplate → FakeChatModel (ToolCall) → ToolsNode → FakeChatModel (final answer)`
- 使用 `wf.AsPromptTemplateNode` / `wf.AsToolsNode` 便捷方法
- 拓扑: `START → prompt → model1 → tools → model2 → END`
- 模拟 `get_weather` 工具,返回确定性结果

#### Example 20: Tool Calling Pipeline (Chain / Graph)
- **Chain 版本**: `AppendLambda(model1).AppendLambda(tools).AppendLambda(model2)`,自动连接
- **Graph 版本**: `AddEdge` 手动拓扑,展示最大灵活性
- 模拟 `calculator` 和 `get_weather` 工具

### PromptTemplate 组件设计

- **ChatTemplate 接口**: `Format(ctx, vs map[string]any) ([]*Message, error)` — 统一提示词渲染合约
- **MessageTemplate**: 支持 `{{variable}}` 占位符替换 + 可选的系统提示词模板
- **ChatTemplateComponent**: `GetRunnable()` 返回 `composableRunnable{i}`,支持图运行时集成
- **FakeChatTemplate**: 测试用 mock 实现
- **组件常量**: `ComponentOfPrompt = "Prompt"`

### Tool / ToolsNode 组件设计

- **BridgeTool 接口**: `Name() string` + `Execute(ctx, args map[string]any) (string, error)`
- **BridgeToolFunc**: 将普通函数包装为 BridgeTool
- **toolsNodeBridge**: 内部维护 `tools map[string]BridgeTool`;输入 `*Message` → 解析 `ToolCalls` → 匹配工具 → 执行 → 结果组装
- **工具匹配**: 根据 `ToolCall.Function.Name` 在 tools map 中查找
- **JSON 参数解析**: `json.Unmarshal` 反序列化 `ToolCall.Function.Arguments`
- **三种构造路径**: `NewToolsNodeLambda(tools...)` / `NewToolsNodeLambdaFromMap(toolMap)` / `Workflow.AsToolsNode`

### 教学边界

当前实现刻意保留为教育子集: 不实现 Eino 完整 `InvokableTool` / `ToolCallingChatModel` 分层接口、streaming tool call、tool rerun skip handler、工具级 Callback 集成和 provider 特定的工具绑定选项。

---

## M1: Final merge docs — 第三章教学示例与文档补全

### 变更范围

| 文件 | 变更类型 | 说明 |
|------|---------|------|
| `cmd/example/main.go` | 新增 | 新增 4 个第三章教学示例 (example12-15),示例总数增至 15 |
| `compose/runnable.go` | 更新 | Runnable 扩展为 Invoke/Stream/Collect/Transform 四模式与 fallback 矩阵 |
| `compose/stream.go` | 新增 | 基础 Pipe stream、Copy、Merge、Concat |
| `compose/callbacks.go` | 新增 | RunInfo、HandlerBuilder、CallbackWrapper、流输入/输出回调副本 |
| `README.md` | 更新 | 新增第三章功能章节,更新架构总览、包结构说明 |
| `CHANGELOG.md` | 更新 | 本文件,记录第三章变更 |
| `FINAL_SUMMARY.md` | 更新 | 新增第三章功能摘要,明确教学子集边界 |

### 新增示例说明

#### Example 12: Runnable Stream 概念演示
- 展示 `composableRunnable` 四字段设计 (`i` / `s` / `c` / `t`)
- 说明 Invoke/Stream/Collect/Transform fallback 矩阵
- 通过 Graph + InvokableLambda 演示 Runnable[I,O].Invoke 公开 API
- 源码追踪: `compose/runnable.go` 的四模式降级逻辑

#### Example 13: Stream Collect 模式
- 基础 Pipe stream 实现: `NewPipe` / `Recv` / `Send` / `Copy` / `Merge` / `Concat`
- 模拟流式 Lambda 输出 5 个 token
- Collect 按序收集所有分块为完整结果
- 说明 Eino 完整版的 merge 策略 (append/concat/mergeMap)

#### Example 14: Stream Transform 模式
- 流式管道: `生产 → Transform(ToUpper) → Collect`
- 三种变换模式说明: 逐 chunk 变换 / 带状态变换 / 批量变换
- 教学演示,完整图流式执行不在范围内

#### Example 15: Callback 计时模式
- 回调生命周期: `OnStart → Execute → OnEnd/OnError`
- 计时 trace 实现: 记录开始时间、计算耗时
- CallbackWrapper 覆盖 Invoke/Stream/Collect/Transform 包装与流回调副本
- EventLog 在 graph 级别的等效可观测性演示

### 文档更新说明

README.md 第三章新增内容:
1. composableRunnable 四字段设计与四模式 fallback 矩阵
2. 基础 Pipe stream、Copy、Merge、Concat 实现说明
3. Collect / Transform / CallbackWrapper 教学模式
4. 明确组件桥接、图级流式执行、stream field mapping 和流式分支不在当前范围内

### 状态

- 所有测试通过 (`go test ./...`) — compose 包 130+ 测试 PASS
- 代码编译通过 (`go build ./...`)、`go vet ./...` 无问题
- 代码格式化通过 (`gofmt -w .`)
- 示例程序运行通过 (`go run ./cmd/example`)
- 20 个示例覆盖 Chapter 1 (Graph/DAG/Pregel/Info/EventLog) + Chapter 2 (FieldMapping/Workflow/Chain/Parallel/Branch) + Chapter 3 (Stream/Collect/Transform/Callback) + Chapter 4 Bridge (RAG Pipeline + Bridge Pattern) + Chapter 5 (Tool Calling Pipeline Workflow/Chain/Graph)
- 已知缺口已在 `research/ch4-r2-replica-bridge-audit.md` 完整记录
