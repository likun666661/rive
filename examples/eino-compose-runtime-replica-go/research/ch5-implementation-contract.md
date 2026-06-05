# Chapter 05 实现契约：Model / Tool / Prompt 组件接口与图桥接

> 基于 R1 组件差距审计 + 当前 `compose/` 源文件扫描
> 目标读者：实施工人 I1（桥接层）/ I2（Schema+Prompt）/ I3（Tool+ToolsNode）
> 语言：中文

---

## 目录

1. [总体目标与范围](#1-总体目标与范围)
2. [消息体 / ToolCall Schema 扩展](#2-消息体--toolcall-schema-扩展)
3. [PromptTemplate 渲染](#3-prompttemplate-渲染)
4. [Tool 与 ToolsNode 执行](#4-tool-与-toolsnode-执行)
5. [Workflow / Graph / Chain 桥接演示](#5-workflow--graph--chain-桥接演示)
6. [文件归属分配与冲突避免](#6-文件归属分配与冲突避免)
7. [测试矩阵](#7-测试矩阵)
8. [明确排除的非目标](#8-明确排除的非目标)
9. [集成风险与约束](#9-集成风险与约束)

---

## 1. 总体目标与范围

### 1.1 本章目标

在当前 `compose/` 复刻版（已实现图编译/运行时、FieldMapping、Workflow/Chain、Stream/Callback、Checkpoint/Interrupt/Resume）之上，补齐第五章定义的四项**组件契约**能力：

| # | 能力 | 当前状态 | 目标 |
|---|------|---------|------|
| 1 | Message 扩展（ToolCalls 字段） | `Message` 只有 `Role` + `Content` | 新增 `ToolCalls`、`ToolCallID`、`Name` 字段 |
| 2 | ToolCall schema 类型 | 不存在 | 新增 `ToolCall`、`FunctionCall`、`ToolInfo`、`ToolResult` 类型 |
| 3 | PromptTemplate 渲染 | 无 `ChatTemplate` 接口；仅 `promptAssemblerBridge` 硬编码 | 新增 `ChatTemplate` 接口 + `Format` 方法 + 变量替换实现 |
| 4 | Tool / ToolsNode 执行 | 无任何工具接口、无 ToolsNode | 新增 `BaseTool` / `InvokableTool` + `ToolsNode`（顺序 Invoke 模式） |
| 5 | Workflow/Graph/Chain 桥接 | Bridge 层已预留位置 | 新增 Tool bridge + Prompt bridge + `AsToolNode`/`AsPromptNode` 便捷方法 |

### 1.2 定位声明

本章依然是**教育子集**，目标是用最小可工作例程演示 Chapter 05 的核心契约模式。完整选项双桶、增强工具接口、流式 ToolsNode 等推迟到后续。

### 1.3 实现规模估算

| 组件 | 文件 | 预估行数 |
|------|------|---------|
| Schema 扩展（Message ToolCalls + ToolCall 类型） | `compose/chatmodel.go`（修改）+ 抽取 `compose/schema.go`（新建） | ~120 行 |
| PromptTemplate | `compose/prompt.go`（新建） | ~130 行 |
| Tool + ToolsNode | `compose/tool.go` + `compose/tool_node.go`（新建） | ~300 行 |
| 桥接适配器 | `compose/bridge.go`（修改） | ~80 行 |
| 组件类型常量 | `compose/types.go`（修改） | ~10 行 |
| 测试文件 | `compose/prompt_test.go` + `compose/tool_node_test.go` + `compose/bridge_test.go`（扩展） | ~500 行 |

---

## 2. 消息体 / ToolCall Schema 扩展

### 2.1 现有 Message 结构（`compose/chatmodel.go:19-22`）

```go
type Message struct {
    Role    RoleType
    Content string
}
```

### 2.2 扩展后的 Message 结构

**位置**：`compose/chatmodel.go` — 在现有 `Message` 中新增字段（向后兼容）。

```go
type Message struct {
    Role       RoleType
    Content    string
    ToolCalls  []ToolCall   // assistant 消息中的工具调用请求
    ToolCallID string       // tool 消息中关联的原始工具调用 ID
    Name       string       // 发送者名称（可选）
}
```

零成本兼容：现有代码不读取 `ToolCalls`/`ToolCallID`/`Name` 字段，保持正常运行。

### 2.3 新增 ToolCall / FunctionCall 类型

**位置**：新建 `compose/schema.go`

```go
package compose

// ToolCall 描述模型生成的一个工具调用请求。
// 当 Assistant 消息的 ToolCalls 非空时，ToolsNode 解析此结构并执行对应工具。
type ToolCall struct {
    ID       string           // 唯一标识，用于 Tool 消息回指
    Type     string           // 固定为 "function"
    Function ToolCallFunction
}

type ToolCallFunction struct {
    Name      string // 工具名
    Arguments string // JSON 格式的参数
}

// ToolInfo 描述一个工具的元数据，用于向 ChatModel 注册可用工具。
type ToolInfo struct {
    Name        string
    Desc        string
    ParamsOneOf *ParamsOneOf
}

// ParamsOneOf 轻量级参数模式（仅支持扁平参数，不支持 JSON Schema 完整模式）。
type ParamsOneOf struct {
    Params map[string]*ParameterInfo
}

type ParameterInfo struct {
    Type     string // "string" | "number" | "boolean" | "object" | "array"
    Desc     string
    Required bool
    Enum     []string
}

// ToolResult 表示一个工具的字符串输出（教育子集仅支持文本结果）。
type ToolResult struct {
    Text string
}
```

### 2.3 向后兼容保证

- `Message` 新增三个字段，零值（nil / empty string）不影响现有行为。
- 所有现有 `Message{...}` 字面量继续编译。
- 所有 `Message.Role` / `Message.Content` 读取维持原样。

---

## 3. PromptTemplate 渲染

### 3.1 接口定义

**位置**：新建 `compose/prompt.go`

```go
package compose

import "context"

// ChatTemplate 是提示词模板的统一接口。
// Format 接收变量映射，渲染为消息列表。
// 教育子集仅支持系统 + 用户两条消息的简单模板。
type ChatTemplate interface {
    Format(ctx context.Context, vs map[string]any) ([]*Message, error)
}
```

### 3.2 MessageTemplate 实现

在 `compose/prompt.go` 中：

```go
// MessageTemplate 是一个简单的提示词模板实现。
// 使用 {{variable}} 语法进行变量替换。
// 支持系统提示词（可选）和用户提示词（必需）。
type MessageTemplate struct {
    systemTemplate *string  // 系统级模板，可选
    userTemplate   string   // 用户消息模板，必需
}

// NewMessageTemplate 创建用户消息模板。
func NewMessageTemplate(tpl string) *MessageTemplate

// WithSystemTemplate 附加可选的系统提示词模板。
func (mt *MessageTemplate) WithSystemTemplate(tpl string) *MessageTemplate

// Format 执行变量替换并返回消息列表。
// 缺失变量：将变量名保留在输出中（不报错，教育子集行为）。
func (mt *MessageTemplate) Format(ctx context.Context, vs map[string]any) ([]*Message, error)
```

### 3.3 Format 变量替换规则

- 识别 `{{variableName}}` 语法
- 查找 `vs["variableName"]`，通过 `fmt.Sprint` 转为字符串
- 若变量在 `vs` 中不存在，保留 `{{variableName}}` 原文
- 字符串级别替换（非 AST 级别），不支持循环/条件

### 3.4 Prompt → Graph 节点适配

在 `compose/prompt.go` 中：

```go
// ChatTemplateComponent 将 ChatTemplate 包装为 composableRunnable，
// 使其可以作为图节点使用。
type ChatTemplateComponent struct {
    ct ChatTemplate
}

func NewChatTemplateComponent(ct ChatTemplate) *ChatTemplateComponent

// GetRunnable 返回 composableRunnable，Invoke 模式调用 Format。
func (c *ChatTemplateComponent) GetRunnable() *composableRunnable

// GetComponentType 返回 ComponentOfPrompt。
func (c *ChatTemplateComponent) GetComponentType() ComponentType
```

### 3.5 FakeChatTemplate 测试辅助

在 `compose/prompt.go` 中：

```go
// FakeChatTemplate 提供可编程的测试替身。
type FakeChatTemplate struct {
    FormatFn func(ctx context.Context, vs map[string]any) ([]*Message, error)
}

func NewFakeChatTemplate(fn func(ctx context.Context, vs map[string]any) ([]*Message, error)) *ChatTemplate
func (f *FakeChatTemplate) Format(ctx context.Context, vs map[string]any) ([]*Message, error)
```

---

## 4. Tool 与 ToolsNode 执行

### 4.1 Tool 接口层次

**位置**：新建 `compose/tool.go`

```go
package compose

// BaseTool 提供工具元数据。
// 每个工具必须能够描述自身的名称、描述和参数 Schema。
type BaseTool interface {
    Info(ctx context.Context) (*ToolInfo, error)
}

// InvokableTool 是可被 ToolsNode 调用的工具。
// 教育子集仅支持字符串输入 → 字符串输出的同步调用。
type InvokableTool interface {
    BaseTool
    InvokableRun(ctx context.Context, argumentsInJSON string) (string, error)
}
```

### 4.2 FakeTool 测试辅助

在 `compose/tool.go` 中：

```go
// FakeTool 是一个可编程的测试工具替身。
type FakeTool struct {
    name  string
    desc  string
    RunFn func(ctx context.Context, argumentsInJSON string) (string, error)
}

func NewFakeTool(name, desc string, runFn func(ctx context.Context, argumentsInJSON string) (string, error)) *FakeTool
func (ft *FakeTool) Info(ctx context.Context) (*ToolInfo, error)
func (ft *FakeTool) InvokableRun(ctx context.Context, argumentsInJSON string) (string, error)
```

### 4.3 ToolsNode

**位置**：新建 `compose/tool_node.go`

```go
package compose

// convTools 将工具列表按优先级排序：优先使用 EnhancedInvokableTool，
// 其次 InvokableTool，最后 BaseTool（仅元数据，无法执行）。
// 教育子集仅处理 InvokableTool，遇到仅 BaseTool 返回错误。
func convTools(tools []InvokableTool) []InvokableTool

// ToolsNode 执行消息中的工具调用。
// 教育子集实现：顺序执行、仅 Invoke 模式、无中断/重运行。
type ToolsNode struct {
    toolsByName map[string]InvokableTool
}

// NewToolsNode 从工具列表创建 ToolsNode。
// 每个工具的 Info().Name 作为查找键。
func NewToolsNode(tools []InvokableTool) (*ToolsNode, error)

// Invoke 处理输入消息中的 ToolCalls，顺序执行每个调用。
// 输入：*Message（必须包含 ToolCalls）
// 输出：[]*Message（每个工具调用返回一个 Tool 角色消息）
func (tn *ToolsNode) Invoke(ctx context.Context, msg *Message) ([]*Message, error)

// ToolsNodeComponent 将 ToolsNode 包装为 composableRunnable 用于图编排。
type ToolsNodeComponent struct {
    tn *ToolsNode
}

func NewToolsNodeComponent(tn *ToolsNode) *ToolsNodeComponent
func (c *ToolsNodeComponent) GetRunnable() *composableRunnable
func (c *ToolsNodeComponent) GetComponentType() ComponentType
```

### 4.4 ToolsNode.Invoke 行为规范

1. 接收 `*Message`，检查 `msg.ToolCalls` 是否非空。若为空，返回 `([]*Message{}, nil)`（空 slice，非错误）。
2. 按顺序遍历 `ToolCalls`：
   - 在 `toolsByName` 中查找 `tc.Function.Name`。
   - 未找到 → 返回 `nil, fmt.Errorf("tool %q not found in ToolsNode", name)`。
   - 找到 → 调用 `InvokableRun(ctx, tc.Function.Arguments)`。
   - 构建 `*Message{Role: Tool, Content: result, ToolCallID: tc.ID}`。
3. 按顺序返回所有 Tool 消息。

### 4.5 已知未实现（文档标注）

- 并行工具执行：当前为顺序执行
- `StreamableTool` / `EnhancedInvokableTool`：仅支持 `InvokableTool`
- 工具执行中断/重运行：依赖 Checkpoint 系统
- 未知工具处理器（`UnknownToolsHandler`）

---

## 5. Workflow / Graph / Chain 桥接演示

### 5.1 Tool Bridge

**位置**：修改 `compose/bridge.go`

```go
// BridgeTool 是工具桥接的领域接口。
type BridgeTool interface {
    Info() (name, desc string)
    Run(ctx context.Context, input string) (string, error)
}

// toolBridge 将 BridgeTool 包装为 InvokableTool。
type toolBridge struct {
    tool BridgeTool
}

func (b *toolBridge) Info(ctx context.Context) (*ToolInfo, error)
func (b *toolBridge) InvokableRun(ctx context.Context, argumentsInJSON string) (string, error)

// Workflow 新增方法：
func (wf *Workflow[I, O]) AsToolNode(key string, tool BridgeTool) *WorkflowNode
// 内部：创建 ToolsNode（含一个工具）→ NewToolsNodeComponent → AddLambdaNode
```

### 5.2 Prompt Bridge

**位置**：修改 `compose/bridge.go`

```go
// BridgePromptTemplate 是提示词模板桥接的领域接口。
type BridgePromptTemplate interface {
    Format(ctx context.Context, vs map[string]any) ([]*BridgeMessage, error)
}

// Workflow 新增方法：
func (wf *Workflow[I, O]) AsPromptNode(key string, pt BridgePromptTemplate) *WorkflowNode
// 内部：创建 ChatTemplateComponent → AddLambdaNode
```

### 5.3 BridgeMessage → Message 转换

桥接适配器中需要双向转换：

```go
func bridgeMessagesToMessages(in []*BridgeMessage) []*Message
func messagesToBridgeMessages(in []*Message) []*BridgeMessage
```

### 5.4 Workflow 端到端演示

新增桥接方法后，Workflow 支持以下端到端模式：

```
START → (query) → Retriever(AsRetrieverNode)
                 → Prompt(AsPromptNode) → ChatModel(AsChatModelNode) → Tool(AsToolNode) → END
```

通过 `AddInput` + FieldMapping 连接。

### 5.5 Chain 桥接演示

**位置**：修改 `compose/chain.go`（可选，低优先级）

```go
func (c *Chain[I, O]) AppendTool(name string, tool InvokableTool) *Chain[I, O]
func (c *Chain[I, O]) AppendPrompt(tpl ChatTemplate) *Chain[I, O]
```

### 5.6 ComponentType 常量扩展

**位置**：修改 `compose/types.go`

```go
const (
    ComponentOfPrompt ComponentType = "Prompt"
    ComponentOfTool   ComponentType = "Tool"
)
```

### 5.7 管道式 chat 示例（Graph 直接集成）

在 `compose/bridge.go` 中新增辅助函数：

```go
// NewChatPipeline 演示工具函数：将 ChatModel + ToolsNode 组合为一个图节点，
// 一个节点内完成"模型生成→工具调用→结果返回"循环（单轮）。
type ChatPipeline struct {
    model ChatModel
    tools *ToolsNode
}

func NewChatPipeline(model ChatModel, tools *ToolsNode) *ChatPipeline
func (p *ChatPipeline) GetRunnable() *composableRunnable
func (p *ChatPipeline) GetComponentType() ComponentType
```

**内部流程**：接收 `[]*Message` → `model.Generate` → 检查 `ToolCalls` → `tools.Invoke` → 追加 Tool 消息 → 返回 `[]*Message`。

---

## 6. 文件归属分配与冲突避免

### 6.1 工人角色定义

```
I1: 桥接层工人
    - 修改 compose/bridge.go（Tool/Prompt bridge 适配器 + Workflow 便捷方法）
    - 修改 compose/types.go（ComponentOfPrompt / ComponentOfTool 常量）
    - 修改 compose/chain.go（可选：AppendTool / AppendPrompt）
    - 新建 compose/chat_pipeline.go（ChatPipeline 组合节点）
    - 新增/修改测试：compose/bridge_test.go

I2: Schema + Prompt 工人
    - 新建 compose/schema.go（ToolCall / FunctionCall / ToolInfo / ToolResult 类型）
    - 修改 compose/chatmodel.go（Message 新增 3 个字段）
    - 新建 compose/prompt.go（ChatTemplate 接口 + MessageTemplate + ChatTemplateComponent + FakeChatTemplate）
    - 新增测试：compose/prompt_test.go

I3: Tool + ToolsNode 工人
    - 新建 compose/tool.go（BaseTool / InvokableTool 接口 + FakeTool）
    - 新建 compose/tool_node.go（ToolsNode + ToolsNodeComponent + convTools）
    - 新增测试：compose/tool_node_test.go
```

### 6.2 文件归属矩阵

| 文件 | I1 | I2 | I3 | 操作类型 |
|------|----|----|----|---------|
| `compose/schema.go` | | **新建** | | I2 独自拥有 |
| `compose/chatmodel.go` | | **修改** | | I2 在 `Message` 追加字段；尽量在文件末尾追加，不干扰 `ChatModel` 接口 |
| `compose/prompt.go` | | **新建** | | I2 独自拥有 |
| `compose/tool.go` | | | **新建** | I3 独自拥有 |
| `compose/tool_node.go` | | | **新建** | I3 独自拥有 |
| `compose/bridge.go` | **修改** | **修改** | | I1 新增 Tool/Prompt bridge；I2 如需 BridgeMessage↔Message 转换函数，在底部追加 |
| `compose/types.go` | **修改** | | | I1 追加 `ComponentOfPrompt` / `ComponentOfTool` 常量（末尾追加） |
| `compose/chain.go` | **修改**（可选） | | | I1 在末尾追加 `AppendTool` / `AppendPrompt` |
| `compose/chat_pipeline.go` | **新建** | | | I1 独自拥有 |

### 6.3 冲突避免规则

| 规则 | 说明 |
|------|------|
| **R1：优先新建文件** | 任何全新的接口/类型/实现，必须放在新文件中。不得在现有文件中混入不相关的新类型。 |
| **R2：仅行尾追加修改** | 对 `chatmodel.go` / `bridge.go` / `types.go` / `chain.go` 的修改仅允许在文件末尾追加新类型/方法。禁止修改现有函数体或插入中间行。 |
| **R3：Message 字段追加** | 在 `chatmodel.go` 的 `Message` 结构体末尾追加 `ToolCalls` / `ToolCallID` / `Name`。不修改 `Role` / `Content` 的位置或语义。 |
| **R4：测试文件隔离** | 每个新组件使用独立的测试文件（`prompt_test.go`、`tool_node_test.go`）。现有 `bridge_test.go` 扩展由 I1 在末尾追加。 |
| **R5：非侵入式桥接** | 桥接适配器不修改领域接口本身。`BridgeTool` / `BridgePromptTemplate` 是独立的领域接口，不侵入 `InvokableTool` / `ChatTemplate` 定义。 |
| **R6：I1 先完成 bridge.go 修改，I2/I3 再引用** | I2 需要 I1 的 `BridgePromptTemplate` 类型；I3 需要 I1 的 `BridgeTool` 类型。I1 应最先完成 bridge.go 修改（占位类型定义），I2/I3 再引用。或者 I2/I3 先定义 schema.go 中的类型，I1 最后做适配。**推荐顺序**：I2(I3) 先定义 schema 与组件类型 → I1 最后通过桥接层统一接入。 |
| **R7：不修改现存在用接口签名** | `ChatModel.Generate` / `ChatModel.Stream` / `Retriever.Retrieve` 签名保持不变。即使 R1 建议加 `...Option`，也不在本次变更中做。 |

### 6.4 不依赖 Rive 特有机制的约束

- 所有新类型使用 Go 标准库，不引入外部依赖。
- 所有新类型放在 `compose` 包中，不创建子包。
- 所有新接口保持最小设计：恰好就是教学需要的 1-2 个方法。

---

## 7. 测试矩阵

### 7.1 Schema 扩展测试（`compose/chatmodel_test.go` 扩展 / `compose/schema_test.go`）

| # | 测试 | 说明 |
|---|------|------|
| S-01 | Message 新增字段零值兼容 | 现有 `Message{Role, Content}` 字面量仍然编译，零值 `ToolCalls==nil` |
| S-02 | ToolCall 序列化/反序列化 | JSON marshal/unmarshal ToolCall 字段 |
| S-03 | ToolInfo 创建与访问 | ToolInfo Name/Desc/ParamsOneOf 字段正确设置 |
| S-04 | ParameterInfo 类型枚举 | Type 值为合法集合（string/number/boolean/object/array） |
| S-05 | ToolResult Text 访问 | 简单文本读写 |

### 7.2 PromptTemplate 测试（`compose/prompt_test.go`）

| # | 测试 | 说明 |
|---|------|------|
| P-01 | MessageTemplate 基本 Format | `userTemplate="{{.name}}"` + `vs={"name":"Alice"}` → `[Message{Role:User, Content:"Alice"}]` |
| P-02 | MessageTemplate 带系统提示词 | `systemTemplate="You are {{.role}}"` + `userTemplate="{{.query}}"` → 两条消息 |
| P-03 | 缺失变量处理 | `{{missing}}` 保留原文，不报错 |
| P-04 | 变量含特殊字符 | `{{x}}` 替换 `x` 含 `<>` 等字符保持不变 |
| P-05 | 空变量映射 | `vs=nil` or `vs={}` → 所有 `{{var}}` 保留原文 |
| P-06 | 多次 Format 隔离 | 同一 `MessageTemplate` 多次 Format，互不影响 |
| P-07 | ChatTemplateComponent 图集成 | `ChatTemplateComponent.GetRunnable().invoke(ctx, map[string]any{})` 成功调用 Format |
| P-08 | FakeChatTemplate | FormatFn 被正确调用并返回预期结果 |
| P-09 | WithSystemTemplate 链式调用 | `NewMessageTemplate("{{.q}}").WithSystemTemplate("role: {{.r}}").Format(...)` 正确 |

### 7.3 Tool 与 ToolsNode 测试（`compose/tool_node_test.go`）

| # | 测试 | 说明 |
|---|------|------|
| T-01 | FakeTool Info | 返回正确 name/desc |
| T-02 | FakeTool Run | InvokableRun 被调用，返回预期字符串 |
| T-03 | ToolsNode 基本调用 | `NewToolsNode([tool])` → `Invoke(msg{ToolCalls:[{ID:"1",Function:{Name:"tool1",Arguments:"{}"}}]})` → `[Message{Role:Tool, Content:"result", ToolCallID:"1"}]` |
| T-04 | ToolsNode 无 ToolCalls | 输入消息无 ToolCalls → 返回空 `[]*Message`，非错误 |
| T-05 | ToolsNode 未知工具名 | `tc.Function.Name` 不在 toolsByName → 返回错误 `"tool X not found in ToolsNode"` |
| T-06 | ToolsNode 多工具顺序执行 | 3 个 ToolCalls → 按顺序执行，返回 3 个 Tool 消息 |
| T-07 | ToolsNode 参数传递 | Arguments JSON 原样传递给 InvokableRun |
| T-08 | convTools 类型断言 | 仅 InvokableTool 列表传入，convTools 保持顺序 |
| T-09 | ToolsNodeComponent 图集成 | `NewToolsNodeComponent(tn).GetRunnable().invoke(ctx, msg)` 正确调用 ToolsNode |
| T-10 | NewToolsNode 空工具列表 | `NewToolsNode(nil)` → 错误（至少需要一个工具） |
| T-11 | NewToolsNode 重复工具名 | 两个工具同名 → 错误 |

### 7.4 桥接层测试（`compose/bridge_test.go` 扩展）

| # | 测试 | 说明 |
|---|------|------|
| B-01 | BridgeTool → InvokableTool 适配 | `toolBridge{tool}` → `Info()` 返回 *ToolInfo，`InvokableRun` 返回结果 |
| B-02 | BridgePromptTemplate → ChatTemplate 适配 | `bridgePromptTemplate` → `Format(ctx, vs)` 返回 `[]*Message` |
| B-03 | Workflow.AsToolNode | Workflow 添加工具节点，端到端 Invoke |
| B-04 | Workflow.AsPromptNode | Workflow 添加提示词节点，端到端 Invoke |
| B-05 | BridgeMessage ↔ Message 转换 | 双向转换正确（Role 映射 + Content） |
| B-06 | ChatPipeline 基本流程 | ChatModel + ToolsNode → GetRunnable → Invoke → 正确调用 Generate + Tool 执行 |
| B-07 | ChatPipeline 无工具调用 | 模型不返回 ToolCalls → 仅返回 Generate 结果，不调用 ToolsNode |

### 7.5 集成测试（`compose/graph_test.go` 扩展）

| # | 测试 | 说明 |
|---|------|------|
| I-01 | Graph 中 ChatModel + ToolsNode | 两节点：ChatModel → ToolsNode，消息流正确 |
| I-02 | Workflow 端到端：Retriever → Prompt → ChatModel | 完整 RAG 链 |
| I-03 | Workflow 端到端：Prompt → ChatModel → Tool | 工具调用链 |
| I-04 | Chain AppendPrompt | Chain 中添加提示词节点 |
| I-05 | Chain AppendTool | Chain 中添加工具节点 |
| I-06 | callback 观察工具执行 | ChatPipeline 或 ToolsNode 图中的 OnStart/OnEnd 事件记录正确 |

---

## 8. 明确排除的非目标

| 排除项 | 理由 | 替代路径 |
|--------|------|---------|
| **双桶选项系统**（`Option.apply` + `implSpecificOptFn`） | 教学价值有限，增加复杂度 | 使用配置结构体（如 `FakeChatModel` 的 `ChatModelOption`）演示选项模式 |
| **ChatModel 签名加 `...Option`** | 会破坏现有 `FakeChatModel` 实现和所有测试 | 保持当前 `Generate(ctx, []*Message) (*Message, error)` 不变 |
| **StreamableTool / EnhancedInvokableTool** | 需要 Stream 完整支持和多模态工具结果 | 仅实现 `InvokableTool`，教育子集足够 |
| **AgenticChatTemplate / AgenticMessage** | 依赖 `AgenticMessage` 类型系统 | 仅标准 `ChatTemplate` + `Message` |
| **ToolsNode 并行执行** | 需要 goroutine 池和并发结果合并 | 顺序执行展示核心编排模式即可 |
| **ToolsNode.Stream** | 仅 Invoke 模式已经覆盖教学目标 | 现有 `composableRunnable.s` 字段已预留 |
| **工具中断/重运行**（InterruptRerunError） | 依赖第四章 Checkpoint/Resume 完整机制 | 保持中断与工具执行关注点分离 |
| **UnknownToolsHandler** | 额外处理路径非教学核心 | 未找到工具时直接返回错误 |
| **ToolAliasConfig 参数别名重映射** | 高级特性 | 直接使用工具定义中的参数名 |
| **服务端工具搜索**（DeferredTools / ToolSearchTool） | Eino 高级特性 | 教育子集不需要 |
| **回调扩展**（`CallbackInput`/`CallbackOutput` 每组件） | 需要完整的 `callbacks/` 回调引擎 | 现有 `CallbackWrapper` 支持 OnStart/OnEnd 观察 |
| **`Typer.GetType()` / `Checker.IsCallbacksEnabled()`** | 回调引擎未深入集成 | 当前 `RunInfo.Type/Component` 字段已就绪 |
| **JSON Schema 完整 ParamsOneOf** | 仅轻量级 `ParameterInfo` 模式 | 足以演示工具参数 Schema 概念 |
| **Provider 特定实现**（OpenAI / Anthropic 等） | 教学复刻版不绑定具体 provider | `FakeChatModel` / `FakeTool` / `FakeChatTemplate` 作为教学替身 |
| **多模态工具结果**（图片/音频/视频输出） | `ToolResult` 多模态增加 schema 复杂度 | 仅返回 `Text string` |

---

## 9. 集成风险与约束

### 9.1 向后兼容风险

| 风险 | 影响 | 缓解 |
|------|------|------|
| `Message` 新增字段后，基于结构体比较的测试可能失效（`reflect.DeepEqual` 中新增的 `ToolCalls` 零值 `nil` 与期望的 `nil` 匹配） | 低 — 零值 nil == nil | 现有 `Message{A, B}` 比较不受影响；新增字段是 nil-safe |
| `chanMessage` 或其他内部消息包装类可能需要对 `ToolCalls` 做深拷贝 | 低 — 当前 channel 传递 `*Message` 指针 | 已有事件日志等使用指针传递，ToolCalls slice 共享引用；仅在需要副本时通过 `*msg == *other` 比较 |

### 9.2 实现顺序建议

```
Phase 1（I2: Schema 先行）:
  compose/schema.go              → ToolCall / FunctionCall / ToolInfo / ToolResult 类型
  compose/chatmodel.go（修改）   → Message 新增 3 个字段
  compose/prompt.go              → ChatTemplate / MessageTemplate / ChatTemplateComponent

Phase 2（I3: Tool 与 ToolsNode）:
  compose/tool.go                → BaseTool / InvokableTool / FakeTool
  compose/tool_node.go           → ToolsNode / ToolsNodeComponent / convTools

Phase 3（I1: 桥接层统一接入）:
  compose/types.go（修改）       → ComponentOfPrompt / ComponentOfTool 常量
  compose/bridge.go（修改）      → BridgeTool / BridgePromptTemplate + Workflow 便捷方法
  compose/chat_pipeline.go       → ChatPipeline 组合节点（可选）
  compose/chain.go（修改）       → AppendTool / AppendPrompt（可选）
```

Phase 1 和 Phase 2 可并行执行（I2 的 `schema.go` 工具类型定义完成后，I3 可立即开始）。
Phase 3 依赖 Phase 1 的 `ChatTemplate` + Phase 2 的 `InvokableTool` / `ToolsNode`。

### 9.3 与先前章节的集成

| 章节 | 集成点 | 影响 |
|------|--------|------|
| Ch2 Workflow | `AsToolNode` / `AsPromptNode` 新增桥接方法 | 仅追加方法，不影响现有 Workflow 功能 |
| Ch2 Chain | `AppendTool` / `AppendPrompt`（可选） | 仅追加方法，不影响现有 Chain 功能 |
| Ch3 Callback | ToolsNode 执行被 `CallbackWrapper` 观察 | 现有 `graph_manager.go` 的 callback 路由不变 |
| Ch4 Checkpoint | 不集成 — ToolsNode 不支持中断/恢复 | 保持关注点分离 |
| Bridge 模式 | 新增 Tool/Prompt bridge 完善 Bridge 模式演示 | 现有 `retrieverBridge` / `chatModelBridge` 模式一致 |

### 9.4 已识别的内部未完成项

| 项目 | 原因 | 后续处理 |
|------|------|---------|
| `promptAssemblerBridge` 重构 | 当前是硬编码 RAG 提示词构造器 | 可逐步迁移为使用 `MessageTemplate`，但非本次强制要求 |
| `BridgeMessage` 类型重复 | `BridgeMessage`（bridge.go）与 `Message`（chatmodel.go）功能重叠 | 桥接适配器中新增 `bridgeMessagesToMessages` 转换函数即可 |
| `FakeChatModel` 的 `ChatModelOption` 与 Eino 选项模式不同 | 当前是 `func(*FakeChatModel)` 选项函数 | 保持不动，教育子集可演示不同的选项风格 |

---

## 附录 A：实现检查清单

### I1（桥接层）检查清单

- [ ] `compose/types.go`：追加 `ComponentOfPrompt` / `ComponentOfTool` 常量（文件末尾）
- [ ] `compose/bridge.go`：定义 `BridgeTool` 领域接口 + `toolBridge` 适配器
- [ ] `compose/bridge.go`：定义 `BridgePromptTemplate` 领域接口 + 适配器
- [ ] `compose/bridge.go`：实现 `bridgeMessagesToMessages` / `messagesToBridgeMessages` 转换
- [ ] `compose/bridge.go`：`Workflow.AsToolNode` 便捷方法
- [ ] `compose/bridge.go`：`Workflow.AsPromptNode` 便捷方法
- [ ] `compose/chat_pipeline.go`：`ChatPipeline` 组合节点（可选）
- [ ] `compose/chain.go`：`AppendTool` / `AppendPrompt` 方法（可选）
- [ ] `compose/bridge_test.go`：≥ 7 个桥接测试（B-01 ~ B-07）

### I2（Schema + Prompt）检查清单

- [ ] `compose/schema.go`：`ToolCall` / `ToolCallFunction` / `ToolInfo` / `ParamsOneOf` / `ParameterInfo` / `ToolResult` 类型
- [ ] `compose/chatmodel.go`：`Message` 追加 `ToolCalls []ToolCall`、`ToolCallID string`、`Name string` 字段（结构体末尾）
- [ ] `compose/prompt.go`：`ChatTemplate` 接口
- [ ] `compose/prompt.go`：`MessageTemplate` 结构 + `NewMessageTemplate` / `WithSystemTemplate` / `Format`
- [ ] `compose/prompt.go`：`{{variable}}` 替换实现
- [ ] `compose/prompt.go`：`ChatTemplateComponent` + `GetRunnable` + `GetComponentType`
- [ ] `compose/prompt.go`：`FakeChatTemplate` 测试替身
- [ ] `compose/prompt_test.go`：≥ 9 个测试（P-01 ~ P-09）

### I3（Tool + ToolsNode）检查清单

- [ ] `compose/tool.go`：`BaseTool` / `InvokableTool` 接口
- [ ] `compose/tool.go`：`FakeTool` 结构 + `NewFakeTool` + `Info` / `InvokableRun`
- [ ] `compose/tool_node.go`：`convTools` 工具排序
- [ ] `compose/tool_node.go`：`ToolsNode` 结构 + `NewToolsNode` + `Invoke`
- [ ] `compose/tool_node.go`：`ToolsNodeComponent` + `GetRunnable` + `GetComponentType`
- [ ] `compose/tool_node_test.go`：≥ 11 个测试（T-01 ~ T-11）
