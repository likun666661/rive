# Provider 一致性测试计划

> 阅读基线：`f1c35a3`
> 上游产物：`05-tool-call-parsing-matrix.md`
> 生成时间：2026-06-13

---

## 1. executive_summary

### 现状

`pie-ai` (`crates/ai`) 目前有 **11 个 Provider**（Anthropic、OpenAI Responses、OpenAI Completions、OpenAI Codex Responses、Azure OpenAI Responses、Google Gemini、Google Vertex、Amazon Bedrock、Mistral、Cloudflare、Faux），它们共享 **同一套 `AssistantMessageEvent` 输出契约**（`crates/ai/src/types.rs:468-523`），但各自使用 **完全不同的流式解析路径**、**不同的 tool call delta 累积策略**、**不同的 JSON 解析时机**。

### 已有测试覆盖

| 测试类别 | 已覆盖 | 缺口 |
|----------|--------|------|
| Anthropic E2E (SSE mock) | 6 个 case：text 流、tool use、error、retry、abort×2 | 仅 Anthropic 一个 provider |
| 各 Provider 请求体构建单元测试 | Anthropic 5、OpenAI Responses 7、Completions 4、Google 3、Bedrock 3、json_parse 5 | 均为 body-only，不测试流式解析 |
| 跨 Provider tool call 一致性 | **0** | 完全缺失 |
| Partial JSON 边界测试 | `json_parse.rs` 仅 5 个基础 case | 嵌套、Unicode、非法输入未覆盖 |
| 并行 tool call | **0** | OpenAI Responses `rposition()` 不可靠但无测试 |

### 目标

建立一套 **fixture-based provider 一致性测试体系**，覆盖所有 provider 的 tool call 解析、input_json_delta 累积、reasoning 输出、usage 统计、cache 行为、abort/retry 语义。

---

## 2. conformance_matrix

### 维度说明

| 维度 | 含义 | 涉及 Provider |
|------|------|-------------|
| **tool_call_start** | Tool call 宣告（ID/name/content_index 正确性） | 全部 |
| **input_json_delta** | 流式 JSON 片段累积 → 最终 arguments 解析正确性 | Anthropic, Bedrock (标准 `input_json_delta`); OpenAI Responses, Completions, Mistral (各自 delta 字段) |
| **reasoning** | thinking/thought/reasoning 内容的事件流正确性 | Anthropic (thinking), OpenAI Completions (reasoning_content), Google Gemini (thought), Bedrock (reasoningContent) |
| **usage** | `message.usage.input/output` 提取正确性 | 全部（Anthropic/OpenAI/Google/Bedrock 各自不同 usage 字段） |
| **cache** | prompt cache / long retention / citation 的 HTTP header & body 字段 | Anthropic (`cache_control`), OpenAI Responses (`text.format.budge`), Google Gemini (`context_cache`) |
| **abort** | CancellationToken 取消时的行为：retry sleep 中断 + SSE drain 中断 + error 事件正确 | 全部（共用 SSE/AWS eventstream 层，但需逐 provider 测试 abort 注入点） |
| **retry** | HTTP 429/503/5xx 时的指数退避重试行为 | 全部（共用 HTTP 层） |
| **error** | Provider 错误事件（safety filter/content filter/overloaded/network error）→ `Error` 事件 | 全部 |
| **parallel_tool_call** | 多个 tool call 同时流式传输时的 index 区分和 arguments 各自累积 | Anthropic, OpenAI Completions, Bedrock（OpenAI Responses 用 `rposition` 不支持） |
| **tool_call_id** | ID 格式、跨 provider 切换时的 ID 映射、空 ID 合成 | Google Gemini（合成 ID）、transform_messages（重映射） |

### 矩阵表

| Provider | tool_call_start | input_json_delta | reasoning | usage | cache | abort | retry | error | parallel_tool_call | tool_call_id |
|----------|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| **Anthropic** | P0 | P0 | P0 | P0 | P1 | P0 | P1 | P1 | P1 | P1 |
| **OpenAI Responses** | P0 | P0 | P1 (reasoning_summary) | P0 | P1 | P0 | P1 | P1 | **P1** ⚠️ | P1 |
| **OpenAI Completions** | P0 | P0 | P1 (reasoning_content) | P0 | P2 | P0 | P1 | P1 | P1 (BTreeMap) | P1 |
| **Google Gemini** | P0 | N/A (原子 args) | P1 (thought) | P1 | P2 | P0 | P1 | P1 | P2 (原子) | P0 (ID 合成) |
| **Amazon Bedrock** | P0 | P0 | P2 (reasoningContent) | P1 | P2 | P0 | P1 | P1 | P1 | P1 |
| **Mistral** | P1 | P1 | P2 | P2 | P2 | P1 | P2 | P2 | P2 (单 tool) | P2 |
| **Azure OpenAI Responses** | P2 | P2 | P2 | P2 | P2 | P2 | P2 | P2 | P2 | P2 |
| **OpenAI Codex Responses** | P2 | P2 | P2 | P2 | P2 | P2 | P2 | P2 | P2 | P2 |
| **Google Vertex** | P2 | N/A | P2 | P2 | P2 | P2 | P2 | P2 | P2 | P2 |
| **Cloudflare** | P2 | P2 | P2 | P2 | P2 | P2 | P2 | P2 | P2 | P2 |
| **Faux** | P2 | N/A (预构建) | P2 | P2 | P2 | P2 | P2 | P2 | P2 | P2 |

**优先级定义**：
- **P0**：阻塞发布，涉及核心 tool call 一致性，必须有 mock 测试
- **P1**：重要功能，建议有 mock 测试
- **P2**：可延后或跟随上游 provider 实现

---

## 3. mock_server_plan

### 3.1 现有 Mock 模式（扩展）

当前 `crates/ai/tests/anthropic_sse_e2e.rs` 已有 `serve_once()` 工具函数：

```rust
async fn serve_once(body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = socket.read(&mut buf).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(), body
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
    });
    format!("http://{addr}")
}
```

### 3.2 推荐扩展：`MockStreamServer`

创建 `crates/ai/tests/mock_server.rs`，提供可复用的 mock HTTP/SSE 服务器：

```rust
// MockStreamServer 结构设计
pub struct MockStreamServer {
    bind_addr: String,
}

impl MockStreamServer {
    /// 绑定随机端口并启动 mock 服务器
    pub async fn start() -> Self { ... }

    /// 预设一次请求的 HTTP 响应（一次性）
    pub async fn serve_once(status: u16, headers: &[(&str, &str)], body: &str) { ... }

    /// 预设多次请求的序列响应（用于重试测试）
    pub async fn serve_sequence(responses: Vec<MockHttpResponse>) { ... }

    /// 预设分块发送的 SSE 响应（用于 abort 测试和逐帧验证）
    pub async fn serve_chunked(chunks: Vec<(Duration, Vec<u8>)>) { ... }
}
```

### 3.3 SSE Fixture 格式

使用内联字符串或函数构造标准 SSE fixture body：

```rust
/// 构造标准 Anthropic tool call SSE
pub fn anthropic_tool_use_sse(
    message_id: &str,
    tool_id: &str,
    tool_name: &str,
    json_chunks: &[&str],
    stop_reason: &str,
) -> String { ... }

/// 构造标准 OpenAI Responses tool call SSE
pub fn openai_responses_tool_call_sse(
    call_id: &str,
    fn_name: &str,
    arguments: &str,
) -> String { ... }

/// 构造标准 OpenAI Completions tool call SSE
pub fn openai_completions_tool_call_sse(
    tool_calls: &[(u64, &str, &str, &str)], // (index, id, name, args_chunks)
    finish_reason: &str,
) -> String { ... }

/// 构造标准 Google Gemini tool call SSE
pub fn gemini_tool_call_sse(
    fn_name: &str,
    fn_id: Option<&str>,
    args: &serde_json::Value,
    finish_reason: &str,
) -> String { ... }

/// 构造标准 Bedrock tool call eventstream（二进制帧）
pub fn bedrock_tool_use_eventstream(
    tool_use_id: &str,
    tool_name: &str,
    json_chunks: &[&str],
    stop_reason: &str,
) -> Vec<u8> { ... }
```

### 3.4 AWS EventStream Mock

Bedrock 使用 `vnd.amazon.eventstream` 二进制协议，需要专用的 mock 工具：

```rust
/// 构造单个 AWS eventstream 帧
fn aws_eventstream_frame(message: &[u8]) -> Vec<u8> {
    // Prelude: total_len(u32) + headers_len(u32) + prelude_crc(u32)
    // Headers: 空
    // Payload: message bytes
    // Message CRC: u32
    let total_len = 16 + message.len() as u32;
    let mut out = Vec::new();
    out.extend_from_slice(&total_len.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // headers_len = 0
    let prelude_crc = crc32(&out);
    out.extend_from_slice(&prelude_crc.to_be_bytes());
    out.extend_from_slice(message);
    let msg_crc = crc32(message);
    out.extend_from_slice(&msg_crc.to_be_bytes());
    out
}
```

### 3.5 Mock 验证工具

```rust
/// 从 stream 中收集所有事件并分类
pub async fn collect_events(
    stream: impl Stream<Item = AssistantMessageEvent>
) -> CollectedEvents { ... }

#[derive(Debug, Default)]
pub struct CollectedEvents {
    pub start: bool,
    pub tool_starts: Vec<(usize, String, String)>,    // (content_index, id, name)
    pub tool_deltas: Vec<(usize, String)>,              // (content_index, delta)
    pub tool_ends: Vec<(usize, String, serde_json::Map<String, Value>)>, // (content_index, id, args)
    pub text: String,
    pub done_reason: Option<DoneReason>,
    pub error_reason: Option<ErrorReason>,
    pub error_message: Option<String>,
    pub usage_input: u64,
    pub usage_output: u64,
}
```

---

## 4. test_cases

### P0 — 阻塞发布

#### TC-P0-01: 单一 tool call 跨 Provider arguments 一致

**场景**：所有 provider 解析相同的 tool call arguments（`{"city": "San Francisco", "unit": "celsius"}`）产生完全一致的 `ToolCall.arguments`。

**实现方式**：
```rust
// 每个 provider 的 fixture 构造相同的逻辑 arguments，但以各自的 wire 格式表达
fn identical_args_fixture() -> serde_json::Map<String, Value> {
    serde_json::json!({"city": "San Francisco", "unit": "celsius"}).as_object().unwrap().clone()
}

// 对 Anthropic: input_json_delta 拆为 ["{\"city\":\"San ", "Francisco\",\"unit\":\"celsius\"}"]
// 对 OpenAI Responses: function_call_arguments.delta 拆为 ["{\"city\":\"San ", "Francisco\",\"unit\":\"celsius\"}"]
// 对 OpenAI Completions: tool_calls[0].function.arguments 拆为 ["{\"city\":\"San ", "Francisco\",\"unit\":\"celsius\"}"]
// 对 Google Gemini: functionCall.args 直接传入完整对象
// 对 Bedrock: toolUse.input 拆为 ["{\"city\":\"San ", "Francisco\",\"unit\":\"celsius\"}"]
```

**验证面**：
- `ToolCallStart` 的 `name` 正确
- `ToolCallEnd` 的 `arguments` 跨 provider 完全一致
- `Done.reason` 为 `ToolUse`

---

#### TC-P0-02: Partial JSON 边界 — 嵌套对象未闭合

**场景**：`parse_partial_json` 对嵌套 JSON 的容错：
```
输入: {"a": {"b": {"c": "x
预期: {"a": {"b": {"c": "x"}}}
```

**实现方式**：
```rust
#[test]
fn tc_p0_02_nested_unclosed_object() {
    // 在 json_parse.rs 或新的 partial_json_tests.rs 中
    let v = parse_partial_json(r#"{"a": {"b": {"c": "x"#).unwrap();
    assert_eq!(v["a"]["b"]["c"], "x");
}

#[test]
fn tc_p0_02_nested_unclosed_array() {
    let v = parse_partial_json(r#"{"a": [1, [2, 3"#).unwrap();
    assert_eq!(v["a"][0], 1);
    assert_eq!(v["a"][1][0], 2);
    assert_eq!(v["a"][1][1], 3);
}
```

---

#### TC-P0-03: input_json_delta 跨 chunk 边界逐帧累积

**场景**：Anthropic/Bedrock 的 `input_json_delta` 在多个 SSE chunk 间正确累积为完整 JSON。

**Anthropic 实现**：
```rust
#[tokio::test]
async fn tc_p0_03_anthropic_multi_chunk_json_accumulation() {
    // 5 个 delta chunk，模拟逐 token 切割
    let json_chunks = [
        r#"{"na"#,
        r#"me":"#,
        r#"test","pa"#,
        r#"rams":{"x"#,
        r#":1}}"#,
    ];
    let body = build_anthropic_tool_use_sse("msg_1", "toolu_1", "my_tool", &json_chunks, "tool_use");
    let base = serve_once(&body).await;
    // ... stream 并验证解析结果
    // 预期: tool_call.name == "my_tool"
    //       tool_call.arguments == {"name": "test", "params": {"x": 1}}
}
```

---

#### TC-P0-04: Provider 错误事件 → Error event

**场景**：所有 5 个核心 provider 的错误事件正确转为 `AssistantMessageEvent::Error`。

**覆盖**：
- Anthropic: `event: error` + `error.message`
- OpenAI Responses: `response.failed` + `error.message`
- OpenAI Completions: `finish_reason = "content_filter"` / `network_error`
- Google Gemini: `finishReason = "SAFETY"` / `"RECITATION"`
- Amazon Bedrock: `exception_type` header

```rust
// 每个 provider 一个 test，使用 mock server 返回错误响应
#[tokio::test]
async fn tc_p0_04a_anthropic_error_event() { ... }
#[tokio::test]
async fn tc_p0_04b_openai_responses_error_event() { ... }
// ...
```

---

#### TC-P0-05: Google Gemini 空 ID 合成

**场景**：Gemini 返回 `functionCall.id == ""` 时，自动合成 ID 格式为 `{name}_{timestamp}_{counter}`。

```rust
#[tokio::test]
async fn tc_p0_05_gemini_synthesizes_id_when_empty() {
    let body = gemini_tool_call_sse(
        "get_weather",
        Some(""),  // 空 ID
        &serde_json::json!({"city": "sf"}),
        "STOP",
    );
    let base = serve_once(&body).await;
    // ... stream
    // 验证 tool_call.id 非空，且格式为 "get_weather_{timestamp}_{counter}"
    assert!(!tool_call.id.is_empty());
    assert!(tool_call.id.starts_with("get_weather_"));
}
```

---

### P1 — 重要功能

#### TC-P1-01: 并行 tool call（Anthropic & OpenAI Completions & Bedrock）

**场景**：两个 tool call 交替收到 delta，各自 arguments 不混淆。

```rust
#[tokio::test]
async fn tc_p1_01a_anthropic_parallel_tool_calls() {
    // Anthropic SSE: index 0 和 index 1 的 tool_use 交替
    // content_block_start index=0 toolu_A get_weather
    // content_block_start index=1 toolu_B get_time
    // content_block_delta index=0 {"city":"sf"}
    // content_block_delta index=1 {"tz":"PT"}
    // content_block_delta index=0
    // content_block_stop index=0
    // content_block_delta index=1
    // content_block_stop index=1
    // 验证: tool call 0 的 arguments 为 {"city": "sf"}
    //       tool call 1 的 arguments 为 {"tz": "PT"}
}

#[tokio::test]
async fn tc_p1_01b_openai_completions_parallel_tool_calls() {
    // 两个不同 index 的 tool_calls 交替出现
    // 验证 BTreeMap 正确聚合
}
```

---

#### TC-P1-02: OpenAI Responses 并行 tool call（已知 rposition 不可靠）

**场景**：如果 Responses API 实际支持并行 tool call，当前 `rposition()` 实现会出错。

**第一步**：验证 Responses API 是否实际支持并行 tool call（查阅 API 文档或实际测试）。

**第二步**：如果是，标记为 **P0 blocker** 需要实现 index-based 映射。

```rust
#[tokio::test]
async fn tc_p1_02_responses_parallel_tool_call_behavior() {
    // 构造两个 output_item.added (function_call) + 交替的 function_call_arguments.delta
    // 验证当前行为（预期可能失败），记录为 known issue 或触发重构
}
```

---

#### TC-P1-03: Reasoning/Thinking 内容流

**场景**：各 provider 的 thinking 内容正确以 `ThinkingStart → ThinkingDelta → ThinkingEnd` 流式输出。

| Provider | 事件/字段 |
|----------|----------|
| Anthropic | `content_block.type = "thinking"` + `thinking_delta` |
| OpenAI Completions | `delta.reasoning_content` / `delta.reasoning` / `delta.reasoning_text` |
| Google Gemini | `is_thinking_part()` 判断 `part.thought` |

```rust
#[tokio::test]
async fn tc_p1_03_anthropic_thinking_stream() { ... }
#[tokio::test]
async fn tc_p1_03_openai_completions_reasoning() { ... }
#[tokio::test]
async fn tc_p1_03_gemini_thought() { ... }
```

---

#### TC-P1-04: Usage 统计提取

**场景**：各 provider 的 token usage 正确填充到最终的 `AssistantMessage.usage`。

| Provider | Usage 源字段 |
|----------|-------------|
| Anthropic | `message_start.message.usage` + `message_delta.usage` |
| OpenAI Responses | `response.usage.input_tokens/output_tokens` |
| OpenAI Completions | `x-usage` SSE 帧 或 `usage` chunk 字段 |
| Google Gemini | `usageMetadata.totalTokenCount` |
| Bedrock | `metadata.usage` |

```rust
#[tokio::test]
async fn tc_p1_04_anthropic_usage() { assert_eq!(msg.usage.input, 10); assert_eq!(msg.usage.output, 2); }
// 已验证于现有 anthropic_sse_e2e::text_stream_produces_ordered_events
```

---

#### TC-P1-05: Anthropic cache_control 插入

**场景**：带 `cache_control` 的工具/消息模板正确写入 request body。

**已部分覆盖**：`anthropic.rs` 单元测试 `cache_control_placed_on_last_block`。

---

#### TC-P1-06: Abort 取消 SSE drain

**场景**：所有 provider 在收到 abort 信号后停止解析并返回 `Error(Aborted)`。

**已覆盖**：Anthropic（`abort_cancels_pending_sse_drain`）。需扩展到其他 provider。

```rust
#[tokio::test]
async fn tc_p1_06_openai_responses_abort_during_sse() { ... }
#[tokio::test]
async fn tc_p1_06_bedrock_abort_during_eventstream() { ... }
```

---

#### TC-P1-07: Retry 指数退避

**场景**：HTTP 503 + Retry-After header 时触发重试。

**已部分覆盖**：Anthropic（`retries_on_503_then_succeeds`）。需扩展。

---

### P2 — 可延后

#### TC-P2-01: transform_messages 跨 provider tool call ID 重映射

**场景**：`ToolCallIdNormalizer` 回调正确将 Anthropic `toolu_XXX` ID 映射为目标 provider 格式。

#### TC-P2-02: Google Gemini thoughtSignature 跨 session 保留/丢弃

#### TC-P2-03: Unicode 跨 SSE chunk 边界截断（4 字节 emoji）

#### TC-P2-04: 空 tool call（无 name、无 args）的容错

#### TC-P2-05: Tool Choice（auto/any/none/specific）各 provider 请求体

#### TC-P2-06: 共享 SSE consumer 的 Azure/Codex Responses 变体

#### TC-P2-07: Bedrock 跨 text/reasoning/toolUse 类型的 contentBlockIndex 映射

---

### 测试文件组织

```
crates/ai/tests/
├── anthropic_sse_e2e.rs         # 已有，保留并扩展
├── mock_server.rs               # 新增：通用 mock 工具库
├── fixture_builder.rs           # 新增：各 provider SSE fixture 构造函数
├── provider_conformance.rs      # 新增：P0 跨 provider 一致性测试
├── partial_json_boundary.rs     # 新增：P0-P1 partial JSON 边界用例
├── parallel_tool_calls.rs       # 新增：P1 并行 tool call 测试
├── reasoning_stream.rs          # 新增：P1 reasoning/thinking 测试
├── abort_retry.rs               # 新增：P1 abort & retry 测试（扩展已有）
└── provider_specific.rs         # 新增：P2 各 provider 特殊行为
```

---

## 5. risks

### 5.1 测试不稳定性

| 风险 | 等级 | 描述 | 缓解措施 |
|------|------|------|---------|
| **flaky 测试因超时** | 中 | mock TCP server 的 `tokio::spawn` 与 `accept()` 之间的竞态。如果测试流在 server accept 之前就发送请求，会触发重试/错误。 | 在 `serve_once` 中加入 `listener.ready()` 或 `sleep(50ms)` 前置等待。 |
| **端口耗尽** | 低 | 大量并行 mock server 绑定随机端口，可能在 CI 高并发时耗尽 ephemeral port。 | 使用 `SO_REUSEPORT`；限制并行 mock server 数量（建议每个测试文件串行）。 |
| **AWS EventStream binary frame CRC 计算错误** | 中 | Bedrock mock 需要手动计算 CRC32，实现错误会导致 false positive/negative。 | 使用已知正确的 AWS SDK 输出做 cross-validation。 |

### 5.2 真实网络依赖

| 风险 | 等级 | 描述 | 缓解措施 |
|------|------|------|---------|
| **无真实网络测试的覆盖盲区** | 高 | mock 测试只能覆盖已知的协议格式，无法发现 provider 的实际行为变更或边缘协议行为。 | 分两阶段：mock 测试作为 CI 必须通过，真实网络测试作为定期 smoke test（如 weekly）。需要提供 `--ignored` 的 live test。 |
| **Provider schema 漂移** | 高 | 各 provider 的 API 响应格式可能不兼容变更（新增字段、字段重命名、事件类型变更）。 | 建立 provider changelog 监控；在 `ANTHROPIC_API_KEY` 等环境变量存在时运行 live conformance test。 |

### 5.3 Provider Schema 漂移

| Provider | 已知漂移风险 |
|----------|------------|
| **Anthropic** | `thinking_delta` 字段从 `thinking` 变为 `text` 的演变历史；`input_json_delta` 字段稳定 |
| **OpenAI Responses** | Responses API 仍在 beta，`output_item.added` / `output_item.done` 结构可能变化 |
| **OpenAI Completions** | Chat Completions 是稳定 API，但 `tool_calls` 的 `index` 可能从 `u64` 变为 `string` |
| **Google Gemini** | `functionCall` → `function_call` 命名可能有 camelCase 变化 |
| **Amazon Bedrock** | `contentBlockStart` / `contentBlockStop` 字段与 Anthropic 不同但语义对应；Converse Stream → Anthropic Messages 的 `Converter` 映射可能滞后 |

### 5.4 Runtime 测试隔离

| 风险 | 等级 | 描述 | 缓解措施 |
|------|------|------|---------|
| **Faux provider 全局状态污染** | 中 | `response_queue()` 是全局 `Mutex<VecDeque>`，并行测试可能互相干扰。 | 在测试前缀中清空队列；将并行测试标记为 `#[serial]` 或使用 `cargo test -- --test-threads=1`。 |
| **Provider registry 全局状态** | 低 | `register_builtins::ensure()` 使用 `OnceLock`，只注册一次。但如果测试修改 registry 状态可能影响后续测试。 | 不依赖 registry 状态；直接构造 `Model` 和 `StreamOptions` 以绕过 registry。 |

---

## 6. recommendations

### 6.1 立即执行（Short-term）

1. **创建 `crates/ai/tests/mock_server.rs` + `fixture_builder.rs`**
   - 将 `serve_once()` 扩展为通用 mock 工具
   - 实现各 provider 的 SSE/eventstream fixture 构造函数
   - 优先：Anthropic → OpenAI Responses → OpenAI Completions → Google Gemini → Bedrock

2. **实现 P0 测试用例（6 个）**
   - TC-P0-01: 跨 provider arguments 一致
   - TC-P0-02: Partial JSON 边界（新用例）
   - TC-P0-03: 多 chunk JSON 累积
   - TC-P0-04: 错误事件（5 个 provider × 1 test 各）
   - TC-P0-05: Gemini 空 ID 合成

3. **修复已识别的关键问题**
   - OpenAI Responses `rposition()` → index-based 映射（如果 Responses API 支持并行 tool call）
   - `parse_partial_json` 增强深层嵌套容错（当前三层的 `{"a":{"b":"x}` 可能不工作）

### 6.2 短期执行（Next Sprint）

4. **实现 P1 测试用例**
   - TC-P1-01: 并行 tool call（A/B/C 矩阵测试 An+B+C 三个 provider）
   - TC-P1-02: 评估 OpenAI Responses 并行支持
   - TC-P1-03: Reasoning/thinking 流
   - TC-P1-04: Usage 统计（扩展现有测试）
   - TC-P1-06: Abort 测试扩展到所有 provider
   - TC-P1-07: Retry 测试扩展到所有 provider

5. **建立 CI 工作流**
   ```yaml
   # .github/workflows/provider-conformance.yml
   - name: Mock conformance tests (all providers)
     run: cargo test --features all-providers --test provider_conformance
   - name: Partial JSON boundary tests
     run: cargo test --test partial_json_boundary
   - name: Live smoke tests (optional, gated)
     run: cargo test --features all-providers -- --ignored
     if: env.ANTHROPIC_API_KEY != '' || env.OPENAI_API_KEY != ''
   ```

### 6.3 中期执行

6. **Fuzzing 集成**
   - 引入 `proptest` 或 `libfuzzer` 对 `parse_partial_json` 进行结构化模糊测试
   - 对每种 provider 的 streaming 输入进行随机化 chunk 切割测试

7. **真实网络 Smoke Test**
   - 创建 `crates/ai/tests/live_conformance.rs`（所有 test 标记 `#[ignore]`）
   - 每个 provider 一个基础 test：发送 tool call 请求 → 验证返回的事件流 contract
   - 由 CI 在夜间/每周运行，依赖 API key 环境变量

### 6.4 长期执行

8. **Provider 协议监控**
   - 记录每个 provider API 的响应 schema snapshot（JSON Schema）
   - 在 CI 中对比 mock fixture 与 snapshot，检测未知字段

9. **跨 Runtime 一致性测试**
   - 同一 query 发送给两个不同 provider（如 Claude + GPT），验证 tool call arguments 的语义等价性（而非字符串等价）
   - 需要使用真实 API，标记为 experimental

10. **Tool Call ID 生命周期测试**
    - 跨 session 切换：Anthropic → OpenAI → Google → Anthropic
    - 验证 `transform_messages` 正确映射所有 tool call ID 和 tool result 的 `tool_call_id`

---

## 附录 A：Mock Fixture 示例

### Anthropic SSE — 单个 tool call

```
event: message_start
data: {"type":"message_start","message":{"id":"msg_001","usage":{"input_tokens":10,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_001","name":"get_weather"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"city\":\"San "}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"Francisco\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":8}}

event: message_stop
data: {"type":"message_stop"}
```

### OpenAI Responses SSE — 单个 function_call

```
event: response.created
data: {"type":"response.created","response":{"id":"resp_001","usage":null}}

event: response.output_item.added
data: {"type":"response.output_item.added","item":{"id":"call_001","type":"function_call","name":"get_weather"}}

event: response.function_call_arguments.delta
data: {"type":"response.function_call_arguments.delta","delta":"{\"city\":\"San "}

event: response.function_call_arguments.delta
data: {"type":"response.function_call_arguments.delta","delta":"Francisco\"}"}

event: response.function_call_arguments.done
data: {"type":"response.function_call_arguments.done","arguments":"{\"city\":\"San Francisco\"}"}

event: response.output_item.done
data: {"type":"response.output_item.done","item":{"id":"call_001","type":"function_call"}}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp_001","usage":{"input_tokens":10,"output_tokens":8}}}
```

### Google Gemini SSE — 单个 functionCall

```
data: {"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"get_weather","args":{"city":"San Francisco"}}}]},"finishReason":"STOP"}],"usageMetadata":{"totalTokenCount":18}}
```

### Bedrock ConverseStream EventStream — 单个 toolUse

二进制帧序列（共 4 帧）：
1. `contentBlockStart`（type=1, contentBlockIndex=0, start={toolUse:{toolUseId:"id_001",name:"get_weather"}}）
2. `contentBlockDelta`（type=2, contentBlockIndex=0, delta={toolUse:{input:"{\"city\":\"San "}}）
3. `contentBlockDelta`（type=3, contentBlockIndex=0, delta={toolUse:{input:"Francisco\"}"}}）
4. `contentBlockStop`（type=4, contentBlockIndex=0）
5. `messageStop`（type=5, stopReason="tool_use"）

---

## 附录 B：`parse_partial_json` 扩展测试用例

```rust
#[test]
fn nested_unclosed_object_three_deep() {
    let v = parse_partial_json(r#"{"a": {"b": {"c": "x"#).unwrap();
    assert_eq!(v["a"]["b"]["c"], "x");
}

#[test]
fn nested_unclosed_array() {
    let v = parse_partial_json(r#"{"a": [1, [2, 3"#).unwrap();
    assert_eq!(v["a"][0], 1);
    assert_eq!(v["a"][1][0], 2);
}

#[test]
fn unclosed_string_with_escape() {
    let v = parse_partial_json(r#"{"a": "hello\"world"#).unwrap();
    assert_eq!(v["a"], "hello\"world");
}

#[test]
fn array_instead_of_object() {
    let v = parse_partial_json(r#"[1, 2, 3]"#).unwrap();
    assert!(v.is_array());
}

#[test]
fn bare_closing_brace_before_opening() {
    let v = parse_partial_json(r#"}{"a": 1"#).unwrap();
    // 当前已知行为: close_partial 不处理前置多余 };
    // 记录为 known issue
}

#[test]
fn cjk_characters() {
    let v = parse_partial_json(r#"{"城市":"北京"}"#).unwrap();
    assert_eq!(v["城市"], "北京");
}

#[test]
fn emoji_in_string() {
    let v = parse_partial_json(r#"{"msg":"hello 🎉"}"#).unwrap();
    assert_eq!(v["msg"], "hello 🎉");
}
```
