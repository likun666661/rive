# 跨 Provider Tool Call 解析一致性分析

> 阅读基线：`f1c35a3`
> 深度档位：`maintainer`
> 生成时间：2026-06-13

---

## 1. problem：不同 Provider Tool Call Arguments 解析要解决什么问题

`pie` 是一个多 Provider AI 客户端运行时，支持 Anthropic、OpenAI Responses、OpenAI Chat Completions、Google Gemini、Amazon Bedrock 五个主流协议。各 Provider 的 tool call 在 **事件结构、参数传输方式、ID/name 字段语义、JSON 完整性时机** 四个维度上完全不同。

`pie` 需要解决的核心问题是：

- **将五种异构的 streaming tool call 协议统一映射为同一种 `AssistantMessageEvent` 事件流**，让上层 Agent 运行时无需感知 Provider 差异。
- **处理 streaming 传输下的 partial JSON 累积与解析**。除 Google Gemini 外，其余 Provider 均以纯文本 JSON 片段（流式增量）传输 tool call arguments，必须在流结束时或 delta 累积到有效 JSON 后才能解析为结构化 `arguments`。
- **保证 tool call ID 的跨 Provider 可追溯性**，支持 Agent 环中 tool result → tool call 的精确匹配，即使在 ID 格式不兼容的 Provider 间切换 session。

---

## 2. why_hard：Streaming Delta、Partial JSON、Provider-specific Event Shape、Tool id/name/schema 差异

### 2.1 Streaming Delta

tool call arguments 不是一次性送达的完整 JSON 对象，而是以 **逐 token 流式增量** 的方式传输。例如 Anthropic 将 `{"city":"sf"}` 拆为 `{"city":"` 和 `"sf"}"` 两个 SSE delta。这要求每个 Provider 的实现维护一个 **per-tool-call 字符串缓冲区**，将 delta 拼接到完整 JSON 字符串后再解析。

### 2.2 Partial JSON

在流式传输的任何中间时刻，累积的 JSON 字符串几乎总是 **不完整** 的——可能缺少闭合花括号、引号未闭合、有尾部逗号。标准的 `serde_json::from_str` 对这种输入直接报错。`pie` 必须实现一个 **容错 parser**（`parse_partial_json`），能够自动补全未闭合的结构。

### 2.3 Provider-specific Event Shape

| 维度 | Anthropic | OpenAI Responses | OpenAI Completions | Google Gemini | Amazon Bedrock |
|------|-----------|-----------------|-------------------|---------------|----------------|
| 工具声明字段 | `tool_use` | `function_call` | `tool_calls` | `functionCall` | `toolUse` |
| 参数累积方式 | 流式 `input_json_delta` | 流式 `function_call_arguments.delta` | 流式 `tool_calls[N].function.arguments` | 原子 `args` 对象 | 流式 `toolUse.input` |
| 解析时机 | `content_block_stop` 时 | `function_call_arguments.done` 时 | 流结束后统一解析 | 收到 `functionCall` 时即完整 | `contentBlockStop` 时 |
| ID 字段 | `id`（服务端提供） | `call_id`（服务端提供） | `tool_calls[N].id`（服务端提供） | `id`（可能为空） | `toolUseId`（服务端提供） |
| 传输协议 | SSE | SSE | SSE（`[DONE]` sentinel） | SSE | AWS binary eventstream |
| 内容索引方式 | `index` 字段 | 无显式索引，按出现顺序 | `index` 字段（BTreeMap） | 无索引，原子对象 | `contentBlockIndex` 字段 |

### 2.4 Tool id/name/schema 差异

- **Anthropic / OpenAI Responses / OpenAI Completions / Bedrock**：服务端在 tool call 开始时提供唯一 ID，无需客户端合成。
- **Google Gemini**：`functionCall` 的 `id` 字段可能为空字符串。此时 `pie` 需要**合成一个 ID**，格式为 `{name}_{timestamp}_{counter}`。
- **OpenAI Responses**：tool call 使用 `call_id`（而非 `id`），且 item-level output 结构需要 `output_item.added` 事件先行通知。
- **OpenAI Completions**：tool call 嵌套在 `delta.tool_calls` 数组中，支持**并行多个 tool call 同时流式传输**，每个有独立的 `index` 和 `function.name/arguments`。

---

## 3. design_approach：pie 的统一 Event/Content 模型

`pie` 定义了一套**与 Provider 无关的流式事件抽象**，定义在 `crates/ai/src/types.rs:468-523`：

```rust
pub enum AssistantMessageEvent {
    Start { partial: AssistantMessage },
    TextStart { content_index: usize, partial: AssistantMessage },
    TextDelta { content_index: usize, delta: String, partial: AssistantMessage },
    TextEnd { content_index: usize, content: String, partial: AssistantMessage },
    ThinkingStart { content_index: usize, partial: AssistantMessage },
    ThinkingDelta { content_index: usize, delta: String, partial: AssistantMessage },
    ThinkingEnd { content_index: usize, content: String, partial: AssistantMessage },
    ToolCallStart { content_index: usize, partial: AssistantMessage },
    ToolCallDelta { content_index: usize, delta: String, partial: AssistantMessage },
    ToolCallEnd { content_index: usize, tool_call: ToolCall, partial: AssistantMessage },
    Done { reason: DoneReason, message: AssistantMessage },
    Error { reason: ErrorReason, error: AssistantMessage },
}
```

### 核心设计原则

**1. Content Block 统一模型**（`types.rs:255-266`）：
```rust
pub enum ContentBlock {
    Text(TextContent),
    Thinking(ThinkingContent),
    Image(ImageContent),
    ToolCall(ToolCall),
}
```

**2. 累积式 Partial Message**：每个 Provider 维护一个 `partial: AssistantMessage`，随着事件逐步填充 `content: Vec<ContentBlock>`。每次事件推送时附带 `partial.clone()` 作为快照，消费者可以随时获取当前累积状态。

**3. 位置索引 `content_index`**：每个 `ToolCallStart`/`ToolCallDelta`/`ToolCallEnd` 携带 `content_index: usize`，指向 `partial.content` 中的 `ContentBlock::ToolCall` 位置，确保消费者能区分多个并行 tool call。

**4. Delta 透传**：`ToolCallDelta` 的 `delta: String` 是**原始 JSON 片段**，不做任何改写，让消费者可以自行做增量渲染。

**5. 统一终止流**：所有 Provider 在流结束时都必须发出 `Done { reason, message }` 或 `Error { reason, error }`，此后不应再有事件。

---

## 4. code_walkthrough：逐 Provider 路径走读

### 4.1 Anthropic（`anthropic.rs`）

**整体流程**：`stream()` → `tokio::spawn(run())` → HTTP POST → `SseStream` → `handle_sse()` → 按 `type` 字段分发。

**Tool Call 路径**：

1. **Start**（行 419-436）：收到 `content_block_start`，type 为 `tool_use`。从 block 中提取 `id` 和 `name`，创建 `ContentBlock::ToolCall`，推送 `ToolCallStart`。
2. **Delta**（行 472-480）：收到 `content_block_delta`，type 为 `input_json_delta`。提取 `partial_json` 字段，追加到 `tool_arg_buffers: HashMap<usize, String>`（key 为 `index`）。**注意：此时不解析 JSON**，仅推送原始 delta。
3. **Stop**（行 493-511）：收到 `content_block_stop`。从 `tool_arg_buffers` 中取出完整累积字符串，调用 `parse_partial_json()` 解析为 `serde_json::Map`，写入 `tc.arguments`，推送 `ToolCallEnd`。

**关键特征**：
- 用 `HashMap<usize, String>` 管理每个 tool call 的参数缓冲区
- 只在 `content_block_stop` 时解析 arguments
- `ensure_block()` 按 index 保证 content vec 长度足够

### 4.2 OpenAI Responses（`openai_responses.rs`）

**Tool Call 路径**：

1. **Start**（行 355-369）：收到 `response.output_item.added`，type 为 `function_call`。提取 `call_id` 和 `name`，创建 `ContentBlock::ToolCall`，推送 `ToolCallStart`。**注意：无 `index` 字段，直接 append 到 `partial.content` 末尾。**
2. **Delta**（行 477-494）：收到 `response.function_call_arguments.delta`。通过 `content.iter().rposition()` 找到最后一个 `ToolCall` 的位置，推送 `ToolCallDelta`。**注意：此 provider 的 delta 不更新 partial 本身**——delta 事件仅通知消费者，真正的 arguments 累积由 provider 外部完成。
3. **Done**（行 496-521）：收到 `response.function_call_arguments.done`。从 `payload["arguments"]` 获取完整的 arguments 字符串，调用 `parse_partial_json()` 解析，写入 `tc.arguments`，推送 `ToolCallEnd`。

**关键特征**：
- 使用 `rposition()` 查找最后一个 `ToolCall`（不支持并行 tool call 的精确索引）
- `function_call_arguments.done` 事件携带完整的最终 arguments（也可能是 partial JSON）
- 共享 SSE consumer（`consume_responses_sse`）被 Azure 复用

### 4.3 OpenAI Chat Completions（`openai_completions.rs`）

**Tool Call 路径**（最复杂的并行处理）：

1. **Accumulation**（行 314-364）：收到 `delta.tool_calls` 数组。使用 `BTreeMap<u64, ToolAccum>` 按 index 聚合。
   - 每个工具的第一个 delta 创建 `ToolAccum`（含 `content_index`、`id`、`name`、`args`），同时创建 `ContentBlock::ToolCall` 并推送到 partial，发送 `ToolCallStart`。
   - 后续 delta 追加到 `ToolAccum.args`，发送 `ToolCallDelta`。
2. **Finalization**（行 386-400）：流结束后，遍历所有 `tools` 条目。对每个条目调用 `parse_partial_json()` 解析累积的 args，更新 `partial.content` 中的 `ToolCall`，发送 `ToolCallEnd`。

**关键特征**：
- 使用 `BTreeMap<u64, ToolAccum>` 支持**多个并行 tool call** 的流式传输
- 使用 `is_new` 标志（id/name/args 全空）判断首次出现，触发 `ToolCallStart`
- 支持 `reasoning_content`/`reasoning`/`reasoning_text` 三种不同的 reasoning 字段名
- 流结束时统一解析所有 tool call，而非每个 `index` 独立收束

### 4.4 Google Gemini（`google.rs` + `google_shared.rs`）

**Tool Call 路径**（最特殊——**非流式，原子对象**）：

1. **Complete Object**（行 267-317）：在 `candidates[0].content.parts` 中收到 `functionCall` 对象。该对象已包含完整的 `name`、`args`（Map）、可选的 `id` 和 `thoughtSignature`。
2. **ID 合成**（行 276-284）：如果 `id` 为空或不存在，合成格式为 `{name}_{timestamp_ms}_{counter}`。使用 `tool_counter: u64` 跟踪序号。
3. **一次性推送**（行 299-316）：同时推送 `ToolCallStart`、`ToolCallDelta`（含序列化后的 arguments）、`ToolCallEnd`。因为 arguments 已经是完整的 Map，不需要 `parse_partial_json`。

**关键特征**：
- Gemini 是唯一 **arguments 不需要 partial JSON 解析** 的 provider——它们以完整的 `args` Map 形式送达
- 没有流式 delta，tool call 在单个 chunk 中一次性出现
- 需要合成 tool call ID 以保持跨 Provider 一致性
- 通过 `open: u8` 变量追踪当前开放的 content block 类型（0=none, 1=text, 2=thinking）
- `convert_messages` 在 `google_shared.rs` 中处理 message 的双向转换

### 4.5 Amazon Bedrock（`amazon_bedrock.rs`）

**Tool Call 路径**：

1. **Start**（行 188-206）：收到 `contentBlockStart`，检查 `start.toolUse` 存在。提取 `toolUseId` 和 `name`，创建 `ContentBlock::ToolCall`。使用 `index_map: HashMap<u64, usize>` 将 Bedrock 的 `contentBlockIndex` 映射到 `partial.content` 的位置索引。
2. **Delta**（行 260-269）：收到 `contentBlockDelta`，检查 `delta.toolUse.input`。追加到 `tool_args: HashMap<u64, String>`（key 为 `contentBlockIndex`），推送 `ToolCallDelta`。
3. **Stop**（行 272-314）：收到 `contentBlockStop`。从 `tool_args` 取出累积字符串，调用 `parse_partial_json()` 解析 arguments，更新 partial，推送 `ToolCallEnd`。同时处理 text 和 reasoningContent 的 stop。

**关键特征**：
- 使用 `index_map` 做 Bedrock 的 block 索引 → content vec 位置的**双向映射**
- 对 text、reasoning、toolUse 三种 delta 类型在一个 `contentBlockDelta` 处理器中统一分发
- 使用 AWS binary eventstream（`AwsEventStream`）而非标准 SSE
- 支持 `exception_type` 错误帧

### 4.6 `parse_partial_json`（`json_parse.rs`）

这是所有 Provider（除 Google Gemini）的**公共容错 JSON 解析器**：

```rust
pub fn parse_partial_json(input: &str) -> Result<Value, serde_json::Error> {
    let trimmed = input.trim();
    if trimmed.is_empty() { return Ok(Value::Null); }
    // Fast path — well-formed input.
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) { return Ok(v); }
    // Close partial structures and retry.
    let closed = close_partial(trimmed);
    serde_json::from_str(&closed)
}
```

**`close_partial` 策略**（行 29-71）：
- 逐字符扫描，维护 `stack: Vec<char>` 记录未闭合的 `{`/`[`
- 追踪字符串状态（`in_string`/`escape`），确保未闭合的字符串能被正确收尾
- 去除尾部逗号后，按 stack 顺序追加闭合符号
- 如果处于未闭合字符串中，追加 `"`

**已知限制**：
- 不支持数组/对象内的裸值解析（如 `[1, 2,` → 丢失 `3`）
- 不支持深层嵌套的不完整结构中的语法错误（如 `{"a": {"b": "x}` 不匹配引号会出错）
- 不验证 JSON key 的合法性

### 4.7 `transform_messages`（`transform_messages.rs`）

跨 Provider session 切换时，`transform_messages` 负责重写历史消息：
- 将 tool call ID 通过 `ToolCallIdNormalizer` 回调重新规范化
- 丢失签名和 redacted thinking
- 为非 vision 模型降级图像为占位文本
- 为孤儿 tool call（无对应 toolResult）合成空的 error toolResult

---

## 5. provider_matrix：跨 Provider 对比表

### 5.1 输入事件

| 阶段 | Anthropic | OpenAI Responses | OpenAI Completions | Google Gemini | Amazon Bedrock |
|------|-----------|-----------------|-------------------|---------------|----------------|
| **Tool Call 宣告** | `content_block_start` + `content_block.type = "tool_use"` | `response.output_item.added` + `item.type = "function_call"` | `delta.tool_calls[N].index` 首次出现 | `candidates[0].content.parts[N].functionCall` | `contentBlockStart` + `start.toolUse` |
| **ID 字段** | `content_block.id` | `item.call_id` | `tool_calls[N].id` | `functionCall.id`（可为空） | `start.toolUse.toolUseId` |
| **Name 字段** | `content_block.name` | `item.name` | `tool_calls[N].function.name` | `functionCall.name` | `start.toolUse.name` |
| **Arguments Delta** | `content_block_delta` + `delta.type = "input_json_delta"` + `delta.partial_json` | `response.function_call_arguments.delta` + `delta` | `delta.tool_calls[N].function.arguments` | 无 delta，原子对象 | `contentBlockDelta` + `delta.toolUse.input` |
| **完成信号** | `content_block_stop` | `response.function_call_arguments.done` (`arguments` 字段) | `finish_reason` 出现后 EOS/`[DONE]` | `functionCall` 即完成 | `contentBlockStop` |

### 5.2 累积策略

| Provider | 缓冲区类型 | Key | 解析时机 | 并行支持 |
|----------|-----------|-----|---------|---------|
| Anthropic | `HashMap<usize, String>` | SSE 中的 `index` | `content_block_stop` | ✅ 天然支持（index 区分） |
| OpenAI Responses | 无显式缓冲区 | `rposition()` 查找最后 ToolCall | `function_call_arguments.done` | ⚠️ 仅支持单 tool call（rposition 不可靠） |
| OpenAI Completions | `BTreeMap<u64, ToolAccum>` | `tool_calls[N].index` | 流结束后统一解析 | ✅ 天然支持（BTreeMap 按 index） |
| Google Gemini | 无缓冲区 | 无索引 | 收到 `functionCall` 时即解析 | ✅ 每个 part 独立 |
| Amazon Bedrock | `HashMap<u64, String>` | `contentBlockIndex`（跨 text/thinking/tool） | `contentBlockStop` | ✅ 支持（contentBlockIndex 区分） |

### 5.3 完成时机与输出事件

| Provider | `ToolCallStart` 时机 | `ToolCallDelta` 时机 | `ToolCallEnd` 时机 | `Done.reason` 判定 |
|----------|---------------------|---------------------|-------------------|-------------------|
| Anthropic | `content_block_start` (tool_use) | 每次 `input_json_delta` | `content_block_stop` | `message_delta.stop_reason == "tool_use"` |
| OpenAI Responses | `response.output_item.added` (function_call) | 每次 `function_call_arguments.delta` | `function_call_arguments.done` | `response.output` 中包含 function_call |
| OpenAI Completions | 首次出现 `tool_calls[N]` | 每次 `function.arguments`（非空） | 流结束后解析并存 | `finish_reason == "tool_calls"` |
| Google Gemini | `functionCall` part 出现时 | **同 ToolCallEnd 一并推送** | **同 ToolCallStart 一并推送** | `candidates[0].finishReason` 或 content 中有 ToolCall |
| Amazon Bedrock | `contentBlockStart` (toolUse) | 每次 `toolUse.input` | `contentBlockStop` | `messageStop.stopReason == "tool_use"` |

### 5.4 错误处理

| Provider | 错误来源 | 错误事件类型 | 错误信息提取路径 |
|----------|---------|-------------|----------------|
| Anthropic | SSE `error` event | `Error { reason: ErrorReason::Error }` | `error.message` |
| OpenAI Responses | `response.failed` / `response.error` / SSE `error` | `Error { reason: ErrorReason::Error }` | `error.message` 或 `response.error.message` |
| OpenAI Completions | `finish_reason = "content_filter" / "network_error"` | `Error { reason: ErrorReason::Error }` | Provider finish_reason 字符串 |
| Google Gemini | `finishReason = "SAFETY"/"RECITATION"/"BLOCKLIST"/"PROHIBITED_CONTENT"` | `Error { reason: ErrorReason::Error }` | `"google error"` 默认文本 |
| Amazon Bedrock | `exception_type` header | `Error { reason: ErrorReason::Error }` | `{exception_type}: {payload_body}` |
| **所有** | HTTP 4xx/5xx / SSE 解析错误 / Abort 信号 | 对应 Error 或 Aborted | 各处统一 |

---

## 6. fuzz_cases：建议 Fuzz/Fixture Case

### 6.1 Partial JSON 边界

| 编号 | 输入片段序列 | 预期行为 |
|------|------------|---------|
| F-01 | `{"a": 1}` + `} `（闭合后继续收到右括号） | 应正确容错，不因多余 `}` 崩溃 |
| F-02 | `{"key": "val` （字符串未闭合即收到 `contentBlockStop`） | `parse_partial_json` 应自动补全引号 |
| F-03 | `{"a": 1,` （尾部逗号） | 应自动移除逗号后补全 `}` |
| F-04 | `{"a": [1, 2,` （未闭合数组） | 应正确补全 `]}` |
| F-05 | `{"nested": {"deep": {"very": "x` （三层嵌套，最内层字符串未闭合） | 应补全三层引号和花括号 |
| F-06 | 空 delta 序列（tool call 开始但无任何 args delta） | `parse_partial_json("")` → `Value::Null`，arguments 应为空 Map |
| F-07 | `[]` （arguments 是数组而非对象） | `parse_partial_json` 返回 `Value::Array`，不匹配 `Value::Object(map)`，arguments 保持空 Map |

### 6.2 Unicode

| 编号 | 输入 | 预期行为 |
|------|------|---------|
| F-08 | `{"城市": "北京"}` | CJK 字符作为 key/value 应正确解析 |
| F-09 | `{"emoji": "🎉🔥"}` | 4 字节 emoji 在 streaming delta 中被切断（跨 SSE chunk 边界） |
| F-10 | `{"text": "\\n\\t\\"quote\\""}` | 转义序列应正确处理 |
| F-11 | `{"bytes": "\u0000"}` | null 字符在 JSON 字符串中 |

### 6.3 并行 Tool Call

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| F-12 | 两个 tool call 交替到达 delta（index 0 arg → index 1 arg → index 0 arg） | 每个 tool call 的 args 应正确各自累积 |
| F-13 | 三个 tool call，仅两个有 `ToolCallStart`，第三个无 | 不应 panic，缺失的 tool call 应有空 arguments |
| F-14 | tool call index 乱序（先 index 2，后 index 0） | OpenAI Completions 用 BTreeMap 应自动排序；Anthropic/Bedrock HashMap 不保证顺序但位置正确 |

### 6.4 跨 Provider ID 边界

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| F-15 | Google Gemini `functionCall.id` 为空字符串 `""` | 应合成 ID |
| F-16 | Anthropic `tool_use.id` 格式 `toolu_XXXX` → Gemini → Bedrock 切换 | `transform_messages` 应通过 `ToolCallIdNormalizer` 重映射 |
| F-17 | tool call ID 包含特殊字符（`/`、`.`、`:`） | 不应被破坏或截断 |

### 6.5 异常流

| 编号 | 场景 | 预期行为 |
|------|------|---------|
| F-18 | `content_block_stop` 在 `content_block_start` 之前到达 | 不应 panic，应优雅处理 |
| F-19 | tool call 的 index 大于 `content_block_start` 时的声明 | Anthropic `ensure_block` 会填充，但可能丢失 type 信息 |
| F-20 | `function_call_arguments.done` 的参数为非法 JSON | `parse_partial_json` 失败，arguments 保持空 Map（不崩溃） |

---

## 7. tests：已有测试和缺口

### 7.1 已有测试

**单元测试**（各 Provider 的 `#[cfg(test)] mod tests`）：

| Provider | 测试数量 | 覆盖内容 |
|----------|---------|---------|
| Anthropic | 5 | cache_control、long_retention、thinking+temperature、tools_cc、fireworks_compat |
| OpenAI Responses | 7 | URL 规范化、system_prompt、long_retention、reasoning_effort、thinking replay/ discard、usage、tool_call 序列化 |
| OpenAI Completions | 4 | URL 规范化、body 结构、assistant tool_calls 序列化、finish_reason 映射 |
| Google | 3 | body 结构、thinking_budget、tools 转换 |
| Amazon Bedrock | 3 | body 结构、tool_result 转换、stop_reason 映射 |
| json_parse | 5 | full_object、unclosed_object、unclosed_string、trailing_comma、empty |
| transform_messages | 4 | thinking→text、errored 跳过、孤儿 tool_call、image 降级 |

**端到端测试**（`crates/ai/tests/anthropic_sse_e2e.rs`）：

| 测试 | 覆盖内容 |
|------|---------|
| `text_stream_produces_ordered_events` | 文本流事件顺序（Start → TextStart → TextDelta → TextEnd → Done） |
| `tool_use_sets_tooluse_stop_reason` | ✅ **Tool call 参数解析**（`{"city":"` + `"sf"}"` → `city: "sf"`） |
| `http_error_becomes_error_event` | SSE error 事件 |
| `retries_on_503_then_succeeds` | HTTP 重试 |
| `abort_cancels_retry_sleep_before_second_request` | Abort 取消重试等待 |
| `abort_cancels_pending_sse_drain` | Abort 取消 SSE 排空 |

### 7.2 测试缺口（按严重程度排序）

1. **跨 Provider tool call 解析一致性测试**（严重）：
   - 无测试验证 Anthropic / OpenAI Responses / Completions / Bedrock 对 **相同 tool call JSON 输入** 产生相同的 `ToolCall.arguments` 输出。
   - 无 fixture 驱动的跨 Provider 对比测试。

2. **Partial JSON 边界测试**（严重）：
   - `parse_partial_json` 仅覆盖基本场景（unclosed object/string/comma）。缺少：
     - 嵌套 object 未闭合（`{"a": {"b": "x"}` → 不匹配引号）
     - 嵌套数组未闭合（`{"a": [1, [2,` → 多层补全）
     - 非法字符（如裸 `}` 或 `]` 先于 `{` 出现）
   - 各 Provider 的 tool call 解析路径没有 partial JSON 注入测试。

3. **OpenAI Responses 并行 tool call**（中等）：
   - 当前实现用 `rposition()` 查找最后 ToolCall，不支持并行 tool call。
   - 无测试覆盖多个 function_call 同时存在的场景。

4. **Google Gemini ID 合成**（中等）：
   - 无测试验证合成 ID 的格式、唯一性、跨 chunk 一致性。

5. **OpenAI Completions 并行 tool call**（中等）：
   - 有 BTreeMap 结构，但无实际测试覆盖多个 tool call 交替流式到达的 scenario。

6. **Bedrock 跨块类型索引**（低）：
   - `contentBlockIndex` 在 text / reasoning / toolUse 间共享，无测试验证交叉接收时的正确性。

7. **transform_messages tool call ID 重映射**（低）：
   - 无测试覆盖 `ToolCallIdNormalizer` 回调的实际行为。
   - 无端到端测试覆盖跨 Provider session 切换。

8. **Unicode 在 streaming delta 中**（低）：
   - 无测试覆盖多字节 UTF-8 字符在 SSE 边界被截断的情况。

---

## 8. risks：跨 Provider 一致性风险

### 8.1 高风险

| 风险 | 描述 | 影响范围 |
|------|------|---------|
| **Arguments 解析差异** | Anthropic/Bedrock 在 `contentBlockStop` 时解析（累积后解析），Google Gemini 直接使用 `args: Map`（无需解析），OpenAI Responses 和 Completions 各自使用 `parse_partial_json`。理论上对相同输入产生相同输出，但无实际对比测试验证。 | 所有 Provider |
| **Partial JSON 容错不一致** | `parse_partial_json` 仅做栈平衡补全，不修复语法错误。不同 Provider 的 delta 可能产生不同形状的 partial JSON（如 key 值 cut 在中间 `{"na`），解析结果可能因 `close_partial` 的行为而不同。 | Anthropic, OpenAI (×2), Bedrock |
| **Tool Call ID 格式不一致** | Anthropic 用 `toolu_` 前缀，OpenAI 用 `call_` 前缀，Google 可能为空需合成，Bedrock 可能用 UUID。跨 Provider session 切换时 tool call ID 的映射错误会导致 tool result 匹配失败。 | 跨 Provider 切换 |
| **并行 Tool Call 正确性** | OpenAI Completions 正确实现 BTreeMap 并行聚合，Anthropic/Bedrock 有 HashMap 索引但缺乏实际测试验证，OpenAI Responses 用 `rposition` 不支持并行。 | OpenAI Responses 最弱 |

### 8.2 中等风险

| 风险 | 描述 | 影响范围 |
|------|------|---------|
| **理论 vs 实际 arguments 解析** | Anthropic/Bedrock `parse_partial_json` 仅在完整累积后解析。但 Anthropic `content_block_stop` 的 payload 中是否有 `end` 标记携带完整 arguments？代码中未使用 `content_block_stop` 的 `content_block` 字段，仅依赖 `tool_arg_buffers`。 | Anthropic, Bedrock |
| **Google Gemini thoughtSignature 丢失** | Thought signature 与 tool call 绑定，但 `transform_messages` 跨 model 时会清除 `thought_signature`，可能影响 Gemini → Gemini 的 session 切换。 | Google Gemini ↔ Gemini |
| **Error 事件中的 arguments 一致性** | 当发生错误时，partial 中的 `ToolCall` 可能有未解析的 arguments（因为 `contentBlockStop` 未到达）。消费者可能看到空的 `arguments: Map`。 | 所有 Provider |

### 8.3 低风险

| 风险 | 描述 |
|------|------|
| **JSON Schema 兼容性** | `ToolParameters` 在各 Provider 间以不同字段名序列化（Anthropic: `input_schema`，OpenAI Completions: `function.parameters`，OpenAI Responses: 顶层 `parameters`，Bedrock: `toolSpec.inputSchema.json`，Google: 顶层 `parameters` → `functionDeclarations`）。理论上应等价，但 JSON Schema draft 版本可能有差。 |
| **Usage 统计缺失** | 部分 Provider（Google Gemini）的 `tokenUsage` 不区分 tool call 相关 token 消耗。

---

## 9. next_questions：下一轮问题

1. **`parse_partial_json` 是否需要更强的容错能力？** 考虑接入 `serde_json` 的 `StreamDeserializer` 或 `partial-json` crate 替代自研实现，以获得对深层嵌套、数组、错误恢复的更好支持。

2. **OpenAI Responses 是否实际出现并行 tool call？** 如果 OpenAI Responses API 不支持并行 function calling，当前 `rposition()` 实现是安全的。如果将来支持，需要改为 index-based 映射。

3. **Tool Call Delta 的 `delta` 字段应透传 raw JSON 还是已解析的键值对？** 当前 `ToolCallDelta { delta: String }` 透传原始 JSON 片段，但 Google Gemini 推送的是序列化后的完整 arguments。消费者需要统一处理这两种格式吗？

4. **跨 Provider 的 Tool Call ID 规范化策略是否应移到 Provider 层？** 当前 `ToolCallIdNormalizer` 是 `transform_messages` 的回调参数，但 ID 知识需要了解 Provider 具体格式。是否有更简洁的 Provider trait 扩展方式？

5. **`content_block_stop` 的 `content_block` 字段（Anthropic）是否包含完整 parsed arguments？** 如果是，可以直接使用而非依赖 `tool_arg_buffers`，减少累积字符串的内存开销和 partial JSON 风险。

6. **是否需要实现 Tool Choice 配置？** 当前代码中 Tool Choice（`auto`/`any`/`none`/specific）被标记为 TODO，这对 Agent loop 中的 forced tool use 模式很重要。

7. **Google Gemini 的 `thoughtSignature` 在连续 tool call 中如何流转？** 当前代码保留签名字段，但 cross-model 时丢弃。是否应该保留以支持 Gemini ↔ Gemini session 切换？

8. **Bedrock 的 SigV4 签名路径缺失**，当前仅支持 Bearer token。这是否是使用障碍？

9. **`consume_responses_sse` 被 Azure 复用**，Azure 的 tool call 解析是否有额外差异？是否需要类似 Compat 的覆盖？

10. **是否需要统一 `Done.reason` 的判定逻辑？** 当前各 Provider 在 `Done` 事件中设置 `reason` 的方式不同：Anthropic 依赖 `stop_reason`，OpenAI Responses 检查 output 数组，Completions 依赖 `finish_reason`，Gemini 既检查 finishReason 又检查 content 中是否有 ToolCall，Bedrock 直接从 stopReason 映射。有没有被遗漏的边缘 case（如 content 和 stop_reason 不一致）？
