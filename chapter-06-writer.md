# Chapter 06: Schema Provider Adapters — 教学细纲

---

## 1. 本章讲解目标

学完本章后，听众应能用自己的话解释以下内容：

1. 为什么一个多 Provider LLM 框架必须在消息格式、工具参数 Schema、流式协议上建立**规范数据模型（Canonical Schema）**，而不是“谁调谁转换”。
2. 规范 `Message` 和 `AgenticMessage` 两种消息模型的区别、设计动机、适用场景。
3. Provider 适配器的职责边界：**双向转换**（原生类型 ↔ 规范类型），以及为什么它不参与 Graph 调度。
4. `ParamsOneOf` 双模式参数 Schema 的语义——什么情况下用轻量级 `params` 树，什么情况下用完整 JSON Schema `anyOf`。
5. 类型化 Provider 扩展槽位（`OpenAIExtension`、`ClaudeExtension`、`GeminiExtension`）为什么优于 `map[string]any`。
6. 如何在同一个 Graph 中混合使用 OpenAI / Claude / Gemini 的请求，而不让任何组件“认识”具体的 Provider。

---

## 2. 这个问题在 LLM 应用 / Agent Runtime 中为什么会出现

### 2.1 多 Provider 场景

假设你在构建一个 RAG + 工具调用的 Agent：

- 用户问 “今天北京天气如何？”
- 流水线先用 **OpenAI** 做意图识别 → 生成 `get_weather` 工具调用
- 工具返回 `{temperature: 22, condition: "sunny"}`
- 再把工具结果发给 **Claude** 做推理总结
- 最后把结果用 **Gemini** 做 Grounding 验证

如果每个节点都直接构造 Provider 的原生请求格式（OpenAI 的 `{role, content, tool_calls}`，Claude 的 `{role, content[{type, text}, {type, tool_use}]}`，Gemini 的 `{role, parts[{text}, {functionCall}]}`），那么：

- 切换 Provider 就需要改写每个节点
- Graph 编辑器需要知道所有 Provider 的 API 细节
- 无法抽象出 `ChatModel` 这个通用概念

### 2.2 消息格式的差异矩阵

| 维度 | OpenAI | Claude | Gemini |
|------|--------|--------|--------|
| 角色名称 | `assistant` | `assistant` | `model` |
| 多模态载体 | `content: [{type, text/image_url}]` | `content: [{type, text/image/source}]` | `parts: [{text}, {inlineData}]` |
| 工具调用位置 | `tool_calls[]`（顶层数组） | `content` 中的 `tool_use` 块 | `parts` 中的 `functionCall` |
| 工具结果位置 | 单独 `role: "tool"` 的 Message | `user` Message 中的 `tool_result` 块 | `function` 角色中的 `functionResponse` |
| 流式增量 | 基于 Index 的 delta 块 | 基于 index 的 content block start/delta | 候选方案中的 chunk |

### 2.3 为什么不能“选一个”做内部格式

如果你选 OpenAI 格式做内标：

- Claude 的工具调用需要映射回 `tool_calls[]`（丢失语义）
- Gemini 的 Grounding 元数据无处存放
- 以后接入新 Provider 就需要为它设计整个映射层

如果选 `map[string]any` 做通用格式：

- 失去编译期类型检查
- 流式合并（同 Index 的 JSON 片段拼接）无法正确实现
- 任何代码都可以随意修改数据，框架无法保证语义

### 2.4 问题的本质

> **如何让来自不同 Provider 的组件在同一个流水线中互操作，同时没有任何组件知道其他组件的具体类型？**

答案：在所有 Provider 与 Graph 运行时之间插入一层**规范 Schema** + **Provider 适配器**。

---

## 3. Eino 的解决思路：问题 → 抽象 → 运行时行为

### 3.1 三层体系结构

```
┌──────────────────────────────────────────────┐
│  Graph 运行时 (compose/)                       │
│  只操作规范类型: Message, AgenticMessage       │
│  不 import 任何 Provider 包                    │
├──────────────────────────────────────────────┤
│  规范 Schema (compose/schema 内联)             │
│  Message, ToolCall, ToolInfo, ParamsOneOf      │
│  ContentBlock, AgenticMessage                  │
│  ResponseMeta, Provider 扩展槽位                │
├──────────────────────────────────────────────┤
│  Provider 适配器 (compose/provider_*.go)       │
│  OpenAI ↔ Message                              │
│  Claude ↔ AgenticMessage                       │
│  Gemini ↔ Message / AgenticMessage（双路径）     │
│  每个适配器实现 Provider 接口                    │
└──────────────────────────────────────────────┘
```

关键设计决策：

1. **Graph 只 import `compose` 包**，不知道任何 Provider。
2. **Schema 类型定义在 `compose/` 包内**，`Message`、`ToolCall` 等是框架的基础类型。
3. **Provider 适配器也定义在 `compose/` 包内**（复刻版做了内联简化；原版在 `eino-ext` 外部仓库），它们 import 规范类型，实现转换函数。
4. **依赖方向永远向下**：Graph → Schema；Provider 适配器 → Schema。Schema 永远不 import Provider。

### 3.2 两种 Message 模型

#### 经典 `Message`（`chatmodel.go:24`）

适用于基于 Chat Completion 的经典应用：

```go
type Message struct {
    Role                     RoleType          // System | Human | Assistant | Tool
    Content                  string            // 纯文本内容
    ToolCalls                []ToolCall        // assistant 的工具调用请求
    ToolCallID               string            // tool 角色的响应 ID
    UserInputMultiContent    []MessageInputPart   // 多模态输入
    AssistantGenMultiContent []MessageOutputPart  // 多模态输出
    ResponseMeta             *ResponseMeta     // finish_reason, usage, provider 扩展
    ReasoningContent         string            // 推理链文本
    Extra                    map[string]any    // 遗留扩展袋
}
```

- **角色驱动**：`Assistant` 角色携带 `ToolCalls`，`Tool` 角色携带 `ToolCallID`
- **工具结果**是独立的 `Role: Tool` 消息
- 由 `ChatModel` 接口使用（`Generate` / `Stream`）

#### Agentic `AgenticMessage`（`provider.go:67`）

适用于需要 MCP 工具、服务端工具、结构化多模态输出的 Agent 应用：

```go
type AgenticMessage struct {
    Role          AgenticRoleType   // system | user | assistant（没有 "tool" 角色）
    ContentBlocks []*ContentBlock   // 类型化块的有序列表
}
```

- **基于 ContentBlock 列表**：文本、图像、工具调用、工具结果、推理全在同一消息的 ContentBlocks 中
- **没有单独的 "tool" 角色**：工具结果是 `AgenticRoleUser` 消息中的 `FunctionToolResult` 块
- 每个 `ContentBlock` 有一个 `Type` 判别符和对应字段的指针

**如何选择**：
- 传统 Function Calling 应用 → `Message` + `ChatModel`
- 需要 MCP 工具 / 服务端工具 / 工具搜索 / 审批流的 Agent → `AgenticMessage` + 后续 `AgenticModel`
- 两者不能混用——Go 泛型约束 `BaseModel[M]` 确保编译期捕获

### 3.3 ParamsOneOf：双模式参数 Schema（`schema.go:29`）

```go
type ParamsOneOf struct {
    params     map[string]*ParameterInfo   // 轻量级路径
    jsonSchema any                          // 完整 JSON Schema 路径
}
```

两种构造方式：

1. **`NewParamsOneOfByParams(map[string]*ParameterInfo)`**
   - 适合简单工具：扁平字段、基础类型、嵌套对象/数组
   - 通过 `paramsToMap()` 自动渲染为 `{"type":"object","properties":{...}}`
   - 示例：`{city: String, unit: String enum[Celsius,Fahrenheit]}`

2. **`NewParamsOneOfByJSONSchema(schema)`**
   - 适合复杂 Schema：`anyOf`、`oneOf`、`$defs`、递归引用
   - 直接存储原始 JSON Schema 值
   - 通过 `utils.InferTool` 从 Go 结构体标签自动生成

统一出口：`ToJSONSchema()` 方法。先检查 `p.params != nil`（第 48 行），再 fallback 到 `p.jsonSchema`。

> **容易误解**：如果先用 `NewParamsOneOfByParams` 构造，再用 `NewParamsOneOfByJSONSchema` 覆盖 `ParamsOneOf` 字段，两个指针都存在于结构体中，但 `ToJSONSchema()` 只检查 `params != nil`，JSON Schema 分支会被**静默忽略**。

### 3.4 Provider 扩展：类型化槽位优于通用 Map（`chatmodel.go:39`）

```go
type ResponseMeta struct {
    ID              string
    Model           string
    FinishReason    string
    Usage           *TokenUsage
    OpenAIExtension *OpenAIRespMetaExtension    // nil = 不存在
    GeminiExtension *GeminiRespMetaExtension
    ClaudeExtension *ClaudeRespMetaExtension
    Extension       any                         // 未知 provider 回退
}
```

为什么这样设计：

1. **nil 指针表示“不适用”**：不关心 Provider 数据的组件直接忽略
2. **编译期类型安全**：不能把 Claude 数据放进 `OpenAIExtension`
3. **合并有明确定义**：`ConcatResponseMeta` 知道如何合并每个扩展槽位
4. **IDE 自动补全**：具体字段名，不需要魔法字符串

OpenAI 扩展：`ResponseMetaExtension{ID, Status, ServiceTier, Reasoning, IncompleteDetails}`  
Claude 扩展：`ResponseMetaExtension{ID, StopReason, StopDetails}`  
Gemini 扩展：`ResponseMetaExtension{ID, FinishReason, GroundingMeta}`

### 3.5 运行时行为

```
用户代码创建 Provider 原生请求
        │
        ├─→ provider.ToCanonicalMessages(req) → []*Message
        │   或
        └─→ provider.ToCanonicalAgenticMessages(req) → []*AgenticMessage
                │
                ▼
        Graph 编译 / 执行（只操作规范类型）
                │
                ├─→ ChatModel.Generate(ctx, []*Message) → *Message
                └─→ Tool.Execute(ctx, args) → result
                        │
                        ▼
        provider.FromCanonicalMessages(msgs, model) → 原生请求
        发给 Provider SDK 调用真实 API（复刻版用 Fake 实现）
```

对于 Gemini 这样的 Provider，同时支持 `Message` 和 `AgenticMessage` 双路径。FakeGeminiProvider 实现了 `ProviderGemini` 接口的全部 4 个方法。

---

## 4. 复刻版对应的最小实现路径

按文件 / 函数 / 测试组织：

### 4.1 规范 Schema 类型（`compose/schema.go`）

| 函数/类型 | 作用 | 对应测试 |
|-----------|------|---------|
| `ToolCall` (第 7 行) | Index 标记流式 delta 归属 | `TestToolCall_Index`, `TestToolCall_IndexNil`, `TestToolCall_JsonRoundTrip` |
| `ToolInfo` (第 21 行) | 工具元信息：名称、描述、参数 Schema | `TestToolInfo_Extra` |
| `ParamsOneOf` (第 29 行) | 双模式参数 Schema | `TestParamsOneOf_ByParams`, `TestParamsOneOf_ByJSONSchema`, `TestParamsOneOf_Empty`, `TestParamsOneOf_Nil`, `TestParamsOneOf_RequiredField` |
| `NewParamsOneOfByParams` (第 35 行) | 轻量级构造 | 同上 |
| `NewParamsOneOfByJSONSchema` (第 40 行) | JSON Schema 构造 | `TestParamsOneOf_ByJSONSchema` |
| `ToJSONSchema` (第 47 行) | 统一出口 | 所有 ParamsOneOf 测试 |
| `paramsToMap` / `paramInfoToMap` (第 57/76 行) | 渲染为 JSON Schema map | `TestParameterInfo_Nested`, `TestParameterInfo_ArrayElem`, `TestParameterInfo_DataType`, `TestParameterInfo_Enum` |
| `ParameterInfo` (第 120 行) | 参数描述：类型、描述、必填、枚举、子参数、数组元素类型 | 同上 |
| `ToolResult` (第 130 行) | 工具执行输出 | `TestToolResult_MultiModal` |
| `Document` (第 168 行) | 规范检索文档 | `TestDocument_AllFields`, `TestDocument_Embedding` |

### 4.2 规范 Message 类型（`compose/chatmodel.go`）

| 函数/类型 | 作用 | 对应测试 |
|-----------|------|---------|
| `RoleType` + 常量 (第 10-17 行) | 角色枚举 | 角色转换测试 (provider_test.go) |
| `Message` (第 24 行) | 经典消息模型 | 所有 provider 测试 |
| `ResponseMeta` (第 39 行) | 带 Provider 扩展槽位的响应元数据 | (provider_test 中的跨 Provider 测试) |
| `SystemMessage` / `HumanMessage` / `AssistantMessage` / `ToolMessage` (第 317-331 行) | 便利构造函数 | (在 graph_test / chain_test 中使用) |
| `ChatModel` 接口 (第 194 行) | 消息级 Chat 模型接口 | `TestCanonicalMessageFromOpenAIChatModel` |
| `FakeChatModel` (第 199 行) | 可注入的 fake 实现 | 同上 |

### 4.3 AgenticMessage 和 ContentBlock（`compose/provider.go`）

| 函数/类型 | 作用 | 对应测试 |
|-----------|------|---------|
| `ContentBlockType` + 常量 (第 5-21 行) | 14 种内容块类型判别符 | `TestNewTextContentBlock` 等 |
| `ContentBlock` (第 23 行) | 类型化内容块——标记联合体 | 同上 |
| `AgenticMessage` (第 67 行) | Agent 级消息模型 | `TestAgenticMessageFirstText`, `TestAgenticMessageToolCalls` |
| `NewTextContentBlock` 等构造函数 (第 72-119 行) | 便捷工厂 | 构造测试 |
| `AgenticMessageFirstText` (第 121 行) | 提取第一条文本 | `TestAgenticMessageFirstText` |
| `AgenticMessageToolCalls` (第 133 行) | 提取所有工具调用 | `TestAgenticMessageToolCalls` |
| `ProviderOpenAI` / `ProviderClaude` / `ProviderGemini` 接口 (第 143-161 行) | Provider 适配器合约 | `TestProviderInterfaces` |

### 4.4 OpenAI Provider 适配器（`compose/provider_openai.go`）

| 函数/类型 | 作用 | 对应测试 |
|-----------|------|---------|
| `OpenAIMessage` (第 5 行) | OpenAI 原生消息格式 | `TestOpenAIRoundTrip` |
| `OpenAIChatRequest` (第 13 行) | OpenAI 原生请求格式 | 同上 |
| `openAIRoleToCanonical` (第 18 行) | 角色映射：`assistant` → `Assistant` | `TestOpenAIRoleRoundTrip` |
| `canonicalRoleToOpenAI` (第 33 行) | 反向角色映射 | `TestOpenAIRoleRoundTrip` |
| `ToCanonicalMessages` (第 48 行) | OpenAI → `[]*Message` | `TestOpenAIToCanonicalMessages`, `TestOpenAIToCanonicalMessagesWithToolCalls`, `TestOpenAIToCanonicalMessagesNil`, `TestOpenAIToCanonicalMessagesEmpty` |
| `FromCanonicalMessages` (第 65 行) | `[]*Message` → OpenAI | `TestOpenAIFromCanonicalMessages` |
| `FakeOpenAIProvider` (第 79 行) | 接口实现（在复刻版中不调用真实 SDK） | `TestFakeOpenAIProvider` |

### 4.5 Claude Provider 适配器（`compose/provider_claude.go`）

| 函数/类型 | 作用 | 对应测试 |
|-----------|------|---------|
| `ClaudeContentBlock` / `ClaudeMessage` / `ClaudeChatRequest` (第 5-30 行) | Claude 原生格式 | `TestClaudeRoundTrip` |
| `claudeRoleToAgentic` / `agenticRoleToClaude` (第 32-54 行) | 角色映射 | 内嵌在转换测试中 |
| `ToCanonicalAgenticMessages` (第 56 行) | Claude → `[]*AgenticMessage` | `TestClaudeToCanonicalAgenticMessages`, `TestClaudeToolUseConversion`, `TestClaudeNilInput`, `TestClaudeToCanonicalMessagesEmpty` |
| `claudeBlockToCanonical` (第 72 行) | 单个 block 转换：`text`/`image`/`tool_use`/`tool_result` | `TestClaudeToolUseConversion` |
| `FromCanonicalAgenticMessages` (第 98 行) | `[]*AgenticMessage` → Claude | `TestClaudeFromCanonicalAgenticMessages` |
| `canonicalBlockToClaude` (第 113 行) | 单个 block 反向转换 | `TestClaudeRoundTrip` |
| `FakeClaudeProvider` (第 130 行) | 接口实现 | `TestFakeClaudeProvider` |

### 4.6 Gemini Provider 适配器（`compose/provider_gemini.go`）

| 函数/类型 | 作用 | 对应测试 |
|-----------|------|---------|
| `GeminiPart` / `GeminiContent` / `GeminiChatRequest` (第 8-37 行) | Gemini 原生格式 | `TestGeminiRoundTrip` |
| `geminiRoleToAgentic` / `agenticRoleToGemini` (第 39-63 行) | 角色映射（注意 `model` ↔ `Assistant`） | `TestGeminiMessageRoleRoundTrip` |
| `ToCanonicalAgenticMessagesFromGemini` (第 93 行) | Gemini → `[]*AgenticMessage` | `TestGeminiToCanonicalAgenticMessages`, `TestGeminiFunctionCallConversion`, `TestGeminiAgenticRoundTrip`, `TestGeminiNilInput`, `TestGeminiToCanonicalMessagesEmpty` |
| `FromCanonicalAgenticMessagesToGemini` (第 128 行) | `[]*AgenticMessage` → Gemini | `TestGeminiAgenticRoundTrip` |
| `ToCanonicalMessagesFromGemini` (第 164 行) | Gemini → `[]*Message`（经典路径） | `TestGeminiToCanonicalMessages`, `TestGeminiToCanonicalMessagesWithFunctionCall` |
| `FromCanonicalMessagesToGemini` (第 209 行) | `[]*Message` → Gemini（经典路径） | `TestGeminiMessageRoundTrip` |
| `FakeGeminiProvider` (第 239 行) | 接口实现（同时支持 Message 和 AgenticMessage） | `TestFakeGeminiProvider` |

### 4.7 跨 Provider 集成测试

| 测试 | 作用 |
|------|------|
| `TestCanonicalMessageFromOpenAIChatModel` | OpenAI 原生请求 → 规范 Message → FakeChatModel → 规范 Message |
| `TestCanonicalAgenticMessageFromClaudeWithTool` | Claude 原生请求 → 规范 AgenticMessage → 提取工具调用 → BridgeTool 执行 |
| `TestGeminiMessageWithRetriever` | Gemini 原生请求 → 规范 Message → Retriever 检索 |
| `TestGeminiFullPipeline` | Gemini 原生请求 → AgenticMessage → 提取工具调用 → 执行 → 构造工具结果 → 回转为 Gemini 原生请求 |
| `TestProviderInterfaces` | 验证三个 Provider 接口的可赋值性 |

---

## 5. 课堂讲解顺序（建议 20-30 分钟）

### 时间分配

| 时段 | 内容 | 时长 |
|------|------|------|
| 0-3 分钟 | **问题导入**：展示三个 Provider 的消息格式差异 | 3 min |
| 3-8 分钟 | **核心抽象**：规范 `Message` vs `AgenticMessage` 的区别和选择 | 5 min |
| 8-12 分钟 | **适配器模式**：Provider 接口合约 + 双向转换的代码演示（用 OpenAI 做例子） | 4 min |
| 12-17 分钟 | **双模式参数 Schema**：`ParamsOneOf` 的两种路径 + 容易踩的坑 | 5 min |
| 17-22 分钟 | **运行时行为**：展示一个完整的跨 Provider 流水线（Gemini → Message → ChatModel → Claude → AgenticMessage） | 5 min |
| 22-25 分钟 | **Provider 扩展槽位**：为什么用类型化字段而不是 `map[string]any` | 3 min |
| 25-28 分钟 | **容易误解点和反例**：角色映射陷阱、流式 Index 缺失、ParamsOneOf 覆盖 | 3 min |
| 28-30 分钟 | **练习题布置 + Q&A** | 2 min |

### 第 1 段：问题导入（3 分钟）

**目标**：让听众看到问题，产生 "需要一个规范层" 的直觉。

板书/幻灯片展示三列并排：

```
OpenAI                      Claude                      Gemini
──────                      ──────                      ──────
{role:"assistant",          {role:"assistant",           {role:"model",
 content:"Hi",               content:[{                  parts:[{text:"Hi"},
 tool_calls:[{               type:"text",                         {functionCall:
  id:"c1",                    text:"Hi"},                         {name:"get_weather",
  function:{                 {type:"tool_use",                     args:{city:"Paris"}}}]}
  name:"get_weather",         id:"toolu_01",
  arguments:'{"city":"Paris"}'}]} name:"get_weather",
                              input:{city:"Paris"}}]}
```

**关键问题**：如果你的 Graph 节点直接消费这些格式，当你想把 "意图识别" 从 OpenAI 切换到 Claude 时，需要改多少代码？

**答案**：每个下游节点都要改。这就是为什么需要中间一层。

### 第 2 段：核心抽象（5 分钟）

**讲解 `Message`**（`chatmodel.go:24`）：

- 角色驱动：`Assistant` → 带 `ToolCalls`，`Tool` → 带 `ToolCallID`
- 工具结果是独立的 `Tool` 角色 Message
- 经典 Chat Completion 场景足够用

```go
// 伪代码：构造一次完整的工具调用 + 结果
msgs := []*Message{
    HumanMessage("今天北京天气如何？"),          // role: Human
    {Role: Assistant, Content: "", ToolCalls: []ToolCall{   // role: Assistant
        {ID: "c1", Function: {Name: "get_weather", Arguments: `{"city":"Beijing"}`}},
    }},
    ToolMessage(`{"temp":22, "condition":"sunny"}`, "c1"), // role: Tool
}
```

**讲解 `AgenticMessage`**（`provider.go:67`）：

- ContentBlock 列表：文本、工具调用、工具结果全在一起
- 没有单独的 Tool 角色——工具结果嵌在 `User` 消息中
- 适合 MCP 工具、服务端工具、结构化多模态

```go
// 伪代码：同样的场景用 AgenticMessage
ams := []*AgenticMessage{
    {Role: AgenticRoleUser, ContentBlocks: []*ContentBlock{
        NewTextContentBlock("今天北京天气如何？"),
    }},
    {Role: AgenticRoleAssistant, ContentBlocks: []*ContentBlock{
        NewToolCallContentBlock("c1", "get_weather", `{"city":"Beijing"}`),
    }},
    {Role: AgenticRoleUser, ContentBlocks: []*ContentBlock{    // 注意：仍是 User 角色
        NewToolResultContentBlock("c1", `{"temp":22}`),
    }},
}
```

### 第 3 段：适配器模式（4 分钟）

**展示三个 Provider 接口**（`provider.go:143-161`）：

```go
type ProviderOpenAI interface {
    Name() string
    ToCanonicalMessages(req *OpenAIChatRequest) ([]*Message, error)
    FromCanonicalMessages(msgs []*Message) (*OpenAIChatRequest, error)
}

type ProviderClaude interface {
    Name() string
    ToCanonicalAgenticMessages(req *ClaudeChatRequest) ([]*AgenticMessage, error)
    FromCanonicalAgenticMessages(msgs []*AgenticMessage) (*ClaudeChatRequest, error)
}

type ProviderGemini interface {
    Name() string
    ToCanonicalAgenticMessages(req *GeminiChatRequest) ([]*AgenticMessage, error)
    FromCanonicalAgenticMessages(msgs []*AgenticMessage) (*GeminiChatRequest, error)
    ToCanonicalMessages(req *GeminiChatRequest) ([]*Message, error)       // 双路径
    FromCanonicalMessages(msgs []*Message) (*GeminiChatRequest, error)
}
```

**关键观察**：
- OpenAI 映射到 `Message`（经典模型）
- Claude 映射到 `AgenticMessage`（ContentBlock 模型）
- Gemini 同时支持两条路径
- Name() 返回 Provider 标识（用于日志/观测）

**课堂演示**：OpenAI 往返

```go
original := &OpenAIChatRequest{Model: "gpt-4", Messages: []*OpenAIMessage{
    {Role: "user", Content: "Hello"},
}}
canonical := ToCanonicalMessages(original)
// → [{Role: User, Content: "Hello", ...}]
rt := FromCanonicalMessages(canonical, "gpt-4")
// rt.Messages[0].Role == "user"
```

**强调**：`openAIRoleToCanonical("system") → System`（`chatmodel.go:13 System = "system"`）是逐字段映射中最容易出错的环节。对应测试 `TestOpenAIRoleRoundTrip`。

### 第 4 段：双模式参数 Schema（5 分钟）

**展示 ParamsOneOf 的定义**（`schema.go:29`）：

```go
type ParamsOneOf struct {
    params     map[string]*ParameterInfo   // 走这条路，或
    jsonSchema any                          // 走这条路
}
```

**演示路径 1：轻量级参数树**

```go
tool := &ToolInfo{
    Name: "get_weather",
    Desc: "获取城市天气",
    ParamsOneOf: NewParamsOneOfByParams(map[string]*ParameterInfo{
        "city": {Type: DataTypeString, Desc: "城市名", Required: true},
        "unit": {Type: DataTypeString, Desc: "温度单位", Enum: []string{"Celsius", "Fahrenheit"}},
    }),
}
// ToJSONSchema() → {"type":"object","properties":{"city":{"type":"string","description":"城市名"},"unit":{"type":"string","enum":["Celsius","Fahrenheit"]}},"required":["city"]}
```

**演示路径 2：完整 JSON Schema**

```go
tool := &ToolInfo{
    Name: "advanced_search",
    Desc: "高级搜索",
    ParamsOneOf: NewParamsOneOfByJSONSchema(map[string]any{
        "type": "object",
        "properties": map[string]any{
            "filters": map[string]any{
                "anyOf": []map[string]any{
                    {"$ref": "#/$defs/DateRange"},
                    {"$ref": "#/$defs/Keyword"},
                },
            },
        },
        "$defs": map[string]any{
            "DateRange": map[string]any{"type": "object", "properties": map[string]any{"start": map[string]any{"type": "string"}, "end": map[string]any{"type": "string"}}},
            "Keyword":   map[string]any{"type": "object", "properties": map[string]any{"word": map[string]any{"type": "string"}}},
        },
    }),
}
```

**容易踩的坑**（在此重点讲）：`ToJSONSchema()` 实现（`schema.go:47`）中：

```go
func (p *ParamsOneOf) ToJSONSchema() (any, error) {
    if p.params != nil {        // ← 先检查 params
        return paramsToMap(p.params), nil
    }
    return p.jsonSchema, nil    // ← 只有 params 为 nil 时才到这里
}
```

如果你同时设置了 `params` 和 `jsonSchema`，JSON Schema 会被**静默忽略**。

### 第 5 段：运行时行为（5 分钟）

**展示完整的跨 Provider 流水线**——这是课堂的核心演示：

```go
// 场景：用户用 Gemini 格式发送请求，内部用 ChatModel 处理，最终返回 Claude 格式

// 步骤 1: Gemini 原生请求 → 规范 Message
geminiReq := &GeminiChatRequest{Contents: []*GeminiContent{
    {Role: "user", Parts: []*GeminiPart{{Text: "What is Rive?"}}},
}}
canonicalMsgs := ToCanonicalMessagesFromGemini(geminiReq)
// 结果: []*Message{{Role: User, Content: "What is Rive?"}}

// 步骤 2: 规范 Message → ChatModel 推理
cm := NewFakeChatModel(WithChatGenerateFunc(func(ctx context.Context, input []*Message) (*Message, error) {
    return AssistantMessage("Rive is an agent team runtime."), nil
}))
resp, _ := cm.Generate(context.Background(), canonicalMsgs)
// 结果: *Message{Role: Assistant, Content: "Rive is an agent team runtime."}

// 步骤 3: 规范 Message → Claude 原生请求
allMsgs := append(canonicalMsgs, resp)
claudeReq := FromCanonicalAgenticMessages(
    // 注意：Message → AgenticMessage 需要转换（当前复刻版未包含此转换）
    []*AgenticMessage{
        {Role: AgenticRoleUser, ContentBlocks: []*ContentBlock{NewTextContentBlock("What is Rive?")}},
        {Role: AgenticRoleAssistant, ContentBlocks: []*ContentBlock{NewAssistantTextContentBlock("Rive is an agent team runtime.")}},
    },
    "claude-3-opus",
)
// 结果: ClaudeChatRequest{Messages: [{Role:"user",Content:[{Type:"text",Text:"What is Rive?"}]},
//                                      {Role:"assistant",Content:[{Type:"text",Text:"Rive is an agent team runtime."}]}]}
```

**教学中要强调的边界**：
- 步骤 2 的 `FakeChatModel` 不知道输入来自 Gemini
- 步骤 3 的 `FromCanonicalAgenticMessages` 不知道上一步用了什么模型
- 每一个环节只知道规范类型

对应测试：`TestCanonicalMessageFromOpenAIChatModel`（`provider_test.go:379`）、`TestGeminiFullPipeline`（`provider_test.go:428`）

### 第 6 段：Provider 扩展槽位（3 分钟）

 **对比两种方式**：

```go
// ❌ 反例：用 map[string]any 存 Provider 数据
resp := &Message{
    Extra: map[string]any{
        "openai_response_id": "resp_abc123",
        "claude_stop_reason": "end_turn",
    },
}
// 问题：不知道哪个 Provider 产生的；合并时最后写入胜出；没有编译期检查

// ✅ 正确：类型化扩展槽位
resp := &Message{
    ResponseMeta: &ResponseMeta{
        FinishReason:    "stop",
        OpenAIExtension: &OpenAIRespMetaExtension{
            ID: "resp_abc123", ServiceTier: "default",
        },
        ClaudeExtension: nil,  // Claude 没生成此响应，nil 表示不适用
    },
}
```

**关键**：`nil` 指针让不关心的组件直接忽略；框架的 concat 函数知道如何合并每个扩展。

### 第 7 段：容易误解点和反例（3 分钟）

见下文第 7 节。

---

## 6. 代码走读脚本

### 推荐阅读顺序（25 分钟可以走完）

```
第 1 步 (5 min)    compose/schema.go              看 ToolCall, ParamsOneOf, ParameterInfo 的结构
第 2 步 (3 min)    compose/chatmodel.go:24-36     看 Message 和 ResponseMeta 的完整字段
第 3 步 (5 min)    compose/provider.go:67-161      看 AgenticMessage + ContentBlock + Provider 接口定义
第 4 步 (4 min)    compose/provider_openai.go:48-77 看 ToCanonicalMessages / FromCanonicalMessages
第 5 步 (4 min)    compose/provider_gemini.go:93-107,164-207 看 Gemini 的双路径转换
第 6 步 (5 min)    compose/provider_test.go:379-449 看跨 Provider 集成测试
```

### 配套讲解的代码片段

**片段 A：ParahmsOneOf 的统一出口**

文件：`compose/schema.go:47-55`

```go
func (p *ParamsOneOf) ToJSONSchema() (any, error) {
    if p == nil { return nil, nil }
    if p.params != nil { return paramsToMap(p.params), nil }  // 轻量级路径
    return p.jsonSchema, nil                                    // JSON Schema 路径
}
```

讲：两个分支只有一个会命中。不要同时设置。

**片段 B：开放 AI 角色映射**

文件：`compose/provider_openai.go:18-46`

```go
func openAIRoleToCanonical(role string) RoleType {
    switch role {
    case "system":    return System
    case "user":      return User
    case "assistant": return Assistant
    case "tool":      return Tool
    default:          return RoleType(role)
    }
}
```

讲：为什么 `default` branch 回退到原始字符串而不是报错？——为了不阻塞未知角色类型（extension 场景）。

**片段 C：Claude 的 content block 转换**

文件：`compose/provider_claude.go:72-96`

```go
func claudeBlockToCanonical(cb *ClaudeContentBlock) *ContentBlock {
    switch cb.Type {
    case "text":        return NewTextContentBlock(cb.Text)
    case "image":       return NewImageContentBlock(cb.Source.Data)
    case "tool_use":    return NewToolCallContentBlock(cb.ID, cb.Name, fmt.Sprintf("%v", cb.Input))
    case "tool_result": return NewToolResultContentBlock(cb.ToolUseID, fmt.Sprintf("%v", cb.Content))
    default:            return NewTextContentBlock(fmt.Sprintf("%v", cb))
    }
}
```

讲：Claude 的 `tool_use` 和 `tool_result` 都嵌在 `content` 数组中，不是一个顶层字段。适配器负责提取。

**片段 D：Gemini Full Pipeline 测试**

文件：`compose/provider_test.go:428-449`

```go
func TestGeminiFullPipeline(t *testing.T) {
    req := &GeminiChatRequest{Contents: []*GeminiContent{
        {Role: "user", Parts: []*GeminiPart{{Text: "Weather?"}}},
        {Role: "model", Parts: []*GeminiPart{
            {FunctionCall: &GeminiFunctionCall{Name: "get_weather", Args: map[string]any{"city": "Tokyo"}}},
        }},
    }}
    ams := ToCanonicalAgenticMessagesFromGemini(req)
    calls := AgenticMessageToolCalls(ams[1])
    tool := NewBridgeTool("get_weather", func(ctx context.Context, args map[string]any) (string, error) {
        return "Cloudy, 18C", nil
    })
    var execArgs map[string]any
    _ = json.Unmarshal([]byte(calls[0].Arguments), &execArgs)
    result, _ := tool.Execute(context.Background(), execArgs)
    response := NewToolResultContentBlock("get_weather", result)
    allMsgs := append(ams, &AgenticMessage{
        Role: AgenticRoleUser, ContentBlocks: []*ContentBlock{response},
    })
    rt := FromCanonicalAgenticMessagesToGemini(allMsgs)
    if len(rt.Contents) != 3 || rt.Contents[2].Role != "user" {
        t.Fatalf("full pipeline failed")
    }
}
```

讲：这就是 Agent 循环的原型——模型生成工具调用 → 执行工具 → 追加结果 → 回轮。全链路使用规范 AgenticMessage，不涉及 Gemini 原生格式。

---

## 7. 容易误解点和反例

### 7.1 混淆两种 Message 模型的选择

| 场景 | 用 Message | 用 AgenticMessage |
|------|-----------|------------------|
| 简单 Chat + Function Calling | ✅ | 过于复杂 |
| MCP 工具 / 服务端工具搜索 | ❌ | ✅ |
| 多模态内容 + 工具混合在同一轮 | 可能够用 | ✅（ContentBlock 更自然） |
| 审批流 / 人工介入 | ❌ | ✅（MCPToolApprovalRequest 等块） |

**反例**：试图把 `*AgenticMessage` 传给需要 `*Message` 的 `ChatModel.Generate()`——Go 编译器不会让你过。

### 7.2 依赖 `Extra` 而非扩展槽位

```go
// ❌ 反例
msg.Extra = map[string]any{"openai_id": "resp_1"}  // 其他代码必须知道 key 名和 provider

// ✅ 正确
msg.ResponseMeta = &ResponseMeta{
    OpenAIExtension: &OpenAIRespMetaExtension{ID: "resp_1"},
}
// 不关心 Provider 的代码直接忽略 nil；关心的做类型断言
```

### 7.3 流式工具调用丢失 Index

在流式处理中，`ToolCall.Index` 标识 delta 块属于哪个工具调用。如果适配器给所有块都设置 `Index = 0`，合并时会把所有 delta 拼成一个调用。

```go
// ❌ 反例——每个块的 Index 都是 0
streams := []ToolCall{
    {Index: &zero, ID: "c1", Function: ToolCallFunction{Name: "search", Arguments: `{"q"`}},
    {Index: &zero, ID: "c2", Function: ToolCallFunction{Name: "calc", Arguments: `{"x":1}`}},
}
// concat 会以为它们属于同一个调用

// ✅ 正确——按工具调用递增 Index
call1idx, call2idx := 0, 1
streams := []ToolCall{
    {Index: &call1idx, ID: "c1", ...},
    {Index: &call2idx, ID: "c2", ...},
}
```

### 7.4 混淆 Gemini 角色映射

Gemini 用 `"model"` 而不是 `"assistant"` 表示 AI 输出，用 `"function"` 而不是 `"tool"` 表示函数返回值。

```go
// geminiRoleToMessage("model") → Assistant   ✓
// geminiRoleToMessage("function") → Tool      ✓
// geminiRoleToAgentic("model") → AgenticRoleAssistant  ✓
// geminiRoleToAgentic("function") → AgenticRoleUser     ✓ (注意!)
```

对应测试：`TestGeminiMessageRoleRoundTrip`（`provider_test.go:478`）

### 7.5 ParamsOneOf 的双重设置

```go
// ❌ 反例——两个路径都设了
t := &ToolInfo{
    ParamsOneOf: NewParamsOneOfByParams(map[string]*ParameterInfo{...}),
}
t.ParamsOneOf = NewParamsOneOfByJSONSchema(...)  // 覆盖了结构体，但...
// 如果 NewParamsOneOfByJSONSchema 只是替换了指针，而原对象中 params 非 nil，ToJSONSchema 仍然走 params 分支
```

### 7.6 未正确闭合流

在真实 Eino 中，`StreamReader.Copy(n)` 创建 `n` 个共享缓冲的子读取器。必须各自 `Close()`，否则底层 goroutine 泄漏。复刻版做了简化，但教学中要提到这点。

### 7.7 角色常量歧义

`compose/types.go:7` 定义了 `User RoleType = "user"`，而 `chatmodel.go:14` 定义了 `Human RoleType = "human"`。两者都是用户输入的有效角色。实际使用中 `"user"` 和 `"human"` 等价，但需注意两套常量的存在。

---

## 8. 练习题 / 思考题

### 基础题

1. **角色映射填空**：
   ```
   OpenAI "tool"        → 规范 _______ RoleType
   Gemini "model"       → 规范 _______ RoleType
   Gemini "function"    → 规范 _______ RoleType (经典 Message 路径)
   Gemini "function"    → 规范 _______ AgenticRoleType (Agentic 路径)
   ```
   答案：`Tool`、`Assistant`、`Tool`、`AgenticRoleUser`

2. **ParamsOneOf 判断**：下面哪种方式适合定义以下工具参数？

   a) `weather(city: string, unit: enum[Celsius, Fahrenheit])`  
   b) `search({filters: anyOf[DateRange, Keyword, AuthorName]})`

   答案：a) `NewParamsOneOfByParams`，b) `NewParamsOneOfByJSONSchema`

3. **找 bug**：下面代码有什么问题？
   ```go
   msg := &Message{
       Role: Tool,
       Content: "temperature: 22",
       ToolCallID: "call_abc",
       Extra: map[string]any{"openai_response_id": "resp_1"},
   }
   ```
   答案：`ToolCallID` 对应的是工具调用的 `ToolCall.ID`，而不是 OpenAI 的响应 ID。OpenAI 响应 ID 应放在 `ResponseMeta.OpenAIExtension.ID` 中。

### 进阶题

4. **新 Provider 适配器**：如果要添加一个新的 Provider "Mistral"，它的消息格式如下。设计 `MistralProvider` 接口和 `ToCanonicalMessages` / `FromCanonicalMessages` 函数。
   ```json
   {"role": "user|assistant|system|tool", "content": "...", "tool_calls": [...]}
   ```
   提示：Mistral 的格式与 OpenAI 几乎相同。是否可从 `ProviderOpenAI` 接口复刻？

5. **转换错误处理**：当 Claude 返回一个未知 `content_block.type`（如 `"thinking"`）时，`claudeBlockToCanonical` 应如何反应？当前的 `default` 分支做法安全吗？

6. **跨模型类型桥接**：当前 `FromCanonicalAgenticMessages` 和 `ToCanonicalMessages` 之间不能直接互转（一个是 `[]*AgenticMessage`，一个是 `[]*Message`）。设计一个 `AgenticToClassic` 转换函数。需要考虑：
   - `AgenticRoleSystem` → `System` 角色
   - `ContentBlock.FunctionToolCall` → `ToolCall[]`
   - `ContentBlock.FunctionToolResult` → `Tool` 角色 + `ToolCallID`
   - 多模态块如何映射？

### 设计题

7. **Provider 扩展的泛型化**：当前的 `ResponseMeta` 硬编码了三个 Provider 扩展槽位（`OpenAIExtension`、`ClaudeExtension`、`GeminiExtension`）。如果要支持任意数量的 Provider，如何在不使用 `map[string]any` 的情况下设计扩展机制？

   提示：考虑 Go 泛型 + 注册表模式的组合。

---

## 9. 附录代码索引

### 文件清单

| 文件 | 行数 | 角色 |
|------|------|------|
| `compose/schema.go` | 184 | ToolCall, ToolInfo, ParamsOneOf, ParameterInfo, ToolResult, Document |
| `compose/chatmodel.go` | 336 | RoleType, Message, ResponseMeta, Provider 扩展类型, ChatModel 接口, FakeChatModel |
| `compose/provider.go` | 161 | ContentBlockType, ContentBlock, AgenticMessage, Provider 接口定义 |
| `compose/provider_openai.go` | 97 | OpenAI 原生格式, 角色映射, ToCanonicalMessages, FromCanonicalMessages, FakeOpenAIProvider |
| `compose/provider_claude.go` | 148 | Claude 原生格式, 角色映射, ToCanonicalAgenticMessages, FromCanonicalAgenticMessages, FakeClaudeProvider |
| `compose/provider_gemini.go` | 271 | Gemini 原生格式, 角色映射, 双路径转换 (Message + AgenticMessage), FakeGeminiProvider |
| `compose/types.go` | 79 | DataType 常量, ChatMessagePartType 常量, ComponentType 常量 |
| `compose/schema_test.go` | 352 | ToolCall 测试, ParamsOneOf 测试, ParameterInfo 测试, ToolResult 测试, Document 测试 |
| `compose/provider_test.go` | 502 | ContentBlock 测试, AgenticMessage 测试, OpenAI/Claude/Gemini 转换测试, 跨 Provider 集成测试 |

### 函数 / 类型索引（按教学重要性排序）

| 优先级 | 函数/类型 | 文件:行号 | 为什么看 |
|--------|----------|-----------|---------|
| ⭐⭐⭐ | `Message` | `chatmodel.go:24` | 最重要：经典消息模型的完整字段，是所有 Provider 转换的目标 |
| ⭐⭐⭐ | `ToCanonicalMessages` | `provider_openai.go:48` | 适配器的核心逻辑——字段级 1:1 映射 |
| ⭐⭐⭐ | `AgenticMessage` | `provider.go:67` | ContentBlock 模型的入口，理解 Agent 数据模型的关键 |
| ⭐⭐⭐ | `ParamsOneOf` + `ToJSONSchema` | `schema.go:29,47` | 双模式 Schema 的统一出口——理解工具注册的关键 |
| ⭐⭐⭐ | `TestGeminiFullPipeline` | `provider_test.go:428` | 最完整的跨 Provider 演示：Gemini → AgenticMessage → Tool → 回合 |
| ⭐⭐ | `ResponseMeta` | `chatmodel.go:39` | Provider 扩展槽位的结构——理解类型化扩展优于 Extra |
| ⭐⭐ | `ToCanonicalAgenticMessagesFromGemini` | `provider_gemini.go:93` | Gemini 双路径之一：parts → ContentBlock 的逐字段转换 |
| ⭐⭐ | `claudeBlockToCanonical` | `provider_claude.go:72` | Claude 特有：content 数组中的 tool_use / tool_result 提取 |
| ⭐⭐ | `openAIRoleToCanonical` / `canonicalRoleToOpenAI` | `provider_openai.go:18,33` | 角色映射是跨 Provider 最容易出错的地方 |
| ⭐ | `NewParamsOneOfByParams` / `NewParamsOneOfByJSONSchema` | `schema.go:35,40` | 两种构造方式的选择 |
| ⭐ | `ProviderOpenAI` / `ProviderClaude` / `ProviderGemini` 接口 | `provider.go:143-161` | Provider 适配器的合约定义 |
| ⭐ | `FakeOpenAIProvider` / `FakeClaudeProvider` / `FakeGeminiProvider` | `provider_openai.go:79` / `provider_claude.go:130` / `provider_gemini.go:239` | 复刻版的 Fake 实现——不调用真实 SDK |
| ⭐ | `ContentBlock` + 构造函数 | `provider.go:23,72-119` | ContentBlock 标记联合体——理解 Agent 轮次结构 |
| ⭐ | `TestOpenAIRoundTrip` | `provider_test.go:118` | 最简单的往返测试 |
| ⭐ | `TestProviderInterfaces` | `provider_test.go:489` | 接口可赋值性验证 |

### 与总纲的关系

本章在总纲（`final-eino-replica-design-samuel-reviewed.md`）中的定位：

> **06 Schema / Provider Adapter**：OpenAI、Claude、Gemini 的消息格式、tool call、metadata 都不同。上层组件如果感知 provider，切换模型等于重写图。canonical `Message` / `AgenticMessage` / `ToolCall`；provider adapter 做双向转换；provider extension 只作为 typed metadata 被动携带。

本章为第 07 章 `Agent Flow / ReAct / MultiAgent` 提供数据语言——Agent 循环中模型生成的工具调用、工具执行结果、用户消息，全部通过规范 Schema 流转。

