# pie-ai Provider / Model / Streaming 粗读报告

> 阅读基线: `f1c35a3`
> 深度档位: `architecture`
> 阅读范围: `crates/ai/src/`

---

## 1. problem — 这一层要解决什么问题

`pie-ai` 是 `pie` CLI 的 LLM 接入层，需要把**五花八门的模型供应商**（OpenAI、Anthropic、Google Gemini、Amazon Bedrock、Cloudflare、Mistral、本地 DS4 等）统一成一组**与协议无关的上层接口**，让 Agent 循环和 CLI 不关心底层是 Responses API 还是 Messages API、是 SSE 还是 AWS binary eventstream。

具体来说，它要解决以下统一问题：

1. **多供应商 / 多协议注册** — 每种 wire protocol（`openai-responses`、`anthropic-messages`、`google-generative-ai`、`bedrock-converse-stream` 等）注册为一个 `ApiProvider` trait 实现，上层通过 `Model.api` 字段查表分发（`api_registry.rs:19-40`）。

2. **统一 Streaming 事件流** — 无论底层是 SSE、AWS binary eventstream 还是 direct JSON，最终都产出 `AssistantMessageEvent` 序列：`Start → (TextStart→TextDelta→TextEnd) | (ThinkingStart→ThinkingDelta→ThinkingEnd) | (ToolCallStart→ToolCallDelta→ToolCallEnd) → Done | Error`（`types.rs:468-545`）。

3. **Unified Options** — 上层通过 `SimpleStreamOptions`（含 `reasoning` 级别 + `thinking_budgets`）表达意图，每个 provider 自行翻译成 provider-specific 的 knobs（如 Anthropic 的 `thinking: { type: enabled, budget_tokens: 8192 }`）。

4. **Reasoning / Thinking** — 统一 `ThinkingLevel` (Minimal/Low/Medium/High/Xhigh) 映射到各 provider 的具体参数（`types.rs:87-122`）。

5. **Usage / Cost Accounting** — 各 provider 的 token 统计字段不同（Anthropic `input_tokens + cache_read_input_tokens`、OpenAI `input_tokens + input_tokens_details.cached_tokens`、Google `promptTokenCount - cachedContentTokenCount`），统一收敛到 `Usage { input, output, cache_read, cache_write, total_tokens }`（`types.rs:311-322`）。

6. **Tool Call** — 不同协议的 tool call 表示不同（Anthropic `tool_use`、OpenAI Responses `function_call`、Google `functionCall`），统一为 `ContentBlock::ToolCall { id, name, arguments }`。

7. **Context Overflow 检测** — 30+ 正则模式覆盖所有 provider 的溢出错误消息形态（`utils/overflow.rs`），统一判断是否需要触发 compaction。

8. **Transparent Retry** — 统一的 HTTP 重试层（408/409/425/429/5xx），包括 DS4 的 409 Conflict（`docs/ds4.md` 第 53-61 行）。

---

## 2. why_hard — 为什么难

### 2.1 Provider 差异

- **Wire format 差异**：OpenAI Responses 是 JSON-in-SSE；Anthropic Messages 是 SSE with `event:` 字段；Google Gemini 是 SSE（chunk 里嵌套 `/candidates/0/content/parts`）；Bedrock 是 AWS binary eventstream（`vnd.amazon.eventstream`），需要手写 CRC32 + 二进制帧解码（`event_stream.rs:76-131`）。

- **消息格式不兼容**：每个 provider 的 message 序列化格式完全不同。OpenAI Responses 用 `{ role, content: [{ type: "input_text" }] }`；Anthropic 用 `{ role, content: [{ type: "text" }] }`；Google 用 `{ role, parts: [{ text }] }`；Bedrock 用 `{ role, content: [{ "text" }] }`。每个 provider 都有独立的 `convert_messages` 函数。

- **Content block 类型差异**：OpenAI Responses 没有显式的 "thinking" content block，reasoning 通过 `response.output_item.added` 事件中的 `"reasoning"` 类型出现；Anthropic 有明确的 `thinking` 和 `redacted_thinking` 类型；Google 通过 `part["thought"] == true` 标记；Bedrock 有 `reasoningContent` delta。

- **Tool Call 差异**：Anthropic 流式返回 `input_json_delta`；OpenAI Responses 返回 `function_call_arguments.delta`；Google 一次性返回完整的 `functionCall` 对象（不流式）；Bedrock 返回 `toolUse.input` delta。

### 2.2 Stream Event 标准化

- **SSE 解析**：手写了一个最小 SSE 解析器（`utils/sse.rs`），只支持 `event:` 和 `data:` 字段。这是因为 TS 端依赖 `eventsource-parser` 第三方包，而 Rust 端避免引入不必要依赖。

- **AWS EventStream**：AWS 的 `vnd.amazon.eventstream` 二进制协议是一个完整的自定义帧协议（12 字节 prelude + 变长 headers + payload + 4 字节 message CRC）。需要手写 IEEE 802.3 CRC32 查找表实现（`event_stream.rs:213-236`），因为没有引入专门的 CRC crate。

- **Event 生命周期管理**：每个 provider 都需要精确管理 content block 的 start/delta/end 事件状态。比如 Anthropic 通过 content block index 追踪；OpenAI Responses 通过 `response.output_item.added` + `response.output_text.delta` + `response.output_text.done` 系列事件；Google 使用 `open: u8` 状态机切换 text/thinking 模式。

### 2.3 OAuth / API Key

- **多 key 源**：API key 可以来自 `options.api_key`（显式传入）、环境变量（`env_api_keys.rs` 映射了 18+ 个 provider 到对应的环境变量名）、Bearer token（Bedrock 用 `AWS_BEARER_TOKEN_BEDROCK`）、Google Vertex ADC（`GOOGLE_APPLICATION_CREDENTIALS`）。

- **OpenAI-compatible key 回退**：在 `openai_responses.rs:768-780` 中，`resolve_openai_compatible_api_key` 先尝试 provider-specific key，再回退到 `OPENAI_API_KEY`，实现了 OpenAI 生态（OpenRouter、Groq、Together 等）的 key 兼容。

### 2.4 DS4 Prefix Cache 维护

`docs/ds4.md` 记录了 DS4（DeepSeek V4 本地推理）特有的 prefix cache 优化，这是 `pie-ai` 的一个核心难点：

- **Byte-exact cache mismatch**：DS4 的 KV prefix cache 依赖于请求历史与采样的 token 流 byte-identical。如果客户端回放时丢掉了 reasoning 内容，缓存即失效（第 22-27 行）。

- **修复 1 — Reasoning 重放**：在 `openai_responses.rs:692-699`，当 model descriptor 设了 `requiresReasoningContentOnAssistantMessages: true` 时，将 assistant 的 thinking content 作为 `{"type":"reasoning"}` input item 重放到 assistant message 之前（DS4 会将 reasoning item merge 到紧随其后的 message）。

- **修复 2 — 409 重试**：`utils/retry.rs:139-143` 将 HTTP 409 加入可重试状态集。DS4 在 live continuation state 被驱逐后返回 409，要求 client 重放完整历史重建 KV checkpoint——pie 发送的就是完整历史，直接重试即可。

- **修复 3 — cache_write_tokens**：`openai_responses.rs:557-562` 读取非标准字段 `input_tokens_details.cache_write_tokens` 并填入 `Usage.cache_write`。

### 2.5 Retry / Overflow

- **Retry 策略**：指数退避 + jitter，默认最多 2 次重试，支持 `Retry-After` 头，最大延迟 60s（`utils/retry.rs:17-19`）。

- **Overflow 识别**：30+ 正则表达式 + 两种静默溢出检测（usage 超窗口、length-stop + 零 output），覆盖 Anthropic/OpenAI/Google/Bedrock/Mistral/Groq/Ollama/llama.cpp/LM Studio 等（`utils/overflow.rs:13-41`）。

### 2.6 JSON 解析和 Token/Cost Accounting

- **Partial JSON**：流式 tool call arguments 以 JSON 片段到达，`parse_partial_json` 关闭未闭合的括号/引号后交给 `serde_json`（`utils/json_parse.rs:11-24`）。

- **Usage 差异**：
  - Anthropic: `input_tokens + cache_read_input_tokens + cache_creation_input_tokens`（`anthropic.rs:556-573`）
  - OpenAI: `input_tokens + input_tokens_details.cached_tokens + input_tokens_details.cache_write_tokens`（`openai_responses.rs:542-564`）
  - Google: `promptTokenCount - cachedContentTokenCount` for input（`google.rs:391-415`）
  - Bedrock: `inputTokens + outputTokens + cacheReadInputTokens + cacheWriteInputTokens`（`amazon_bedrock.rs:348-360`）

- **Cost 计算**：`Model.cost` 字段存储各 provider 的 USD/1M tokens 价格，但当前 Rust 端口未实现 cost 乘以 usage 的计算（TS 端在调用侧做）。`UsageCost` 结构体已定义但未在各 provider 中填充。

---

## 3. design_approach — pie-ai 的解决思路

### 3.1 文本流程图

```
CLI / Agent
    │
    ├─ 调用 stream(model, context, options)   [stream.rs:19]
    │         │
    │         ├─ resolve(model) → 查 api_registry   [stream.rs:13]
    │         │         │
    │         │         ├─ register_builtins::ensure()   [register_builtins.rs:12]
    │         │         ├─ get_api_provider(&model.api)  [api_registry.rs:77]
    │         │         └─ RegisteredHandle { Arc<dyn ApiProvider> }
    │         │
    │         └─ handle.stream(model, context, options) → AssistantMessageEventStream
    │                   │
    │                   ├─ [AnthropicProvider]  → SSE → AssistantMessageEvent
    │                   │     anthropic.rs:110-124
    │                   │     ├─ resolve api_key (options / env)
    │                   │     ├─ resolve_compat (Fireworks/Cloudflare overrides)
    │                   │     ├─ build_request_body (cache_control, thinking, tools)
    │                   │     ├─ POST /v1/messages with SSE Accept
    │                   │     ├─ send_with_retry (utils/retry.rs)
    │                   │     ├─ consume SSE: SseStream + handle_sse
    │                   │     └─ push AssistantMessageEvent to sender
    │                   │
    │                   ├─ [OpenAIResponsesProvider] → SSE → AssistantMessageEvent
    │                   │     openai_responses.rs:79-93
    │                   │     ├─ resolve_openai_compatible_api_key
    │                   │     ├─ resolve_compat (DS4 replay_reasoning_content etc.)
    │                   │     ├─ build_request_body (reasoning, prompt_cache, tools)
    │                   │     ├─ POST /v1/responses with SSE Accept
    │                   │     ├─ send_with_retry
    │                   │     ├─ consume_responses_sse (shared with Azure)
    │                   │     └─ push AssistantMessageEvent to sender
    │                   │
    │                   ├─ [GoogleProvider] → Gemini SSE → AssistantMessageEvent
    │                   │     google.rs:43-56
    │                   │     ├─ POST :streamGenerateContent?alt=sse
    │                   │     ├─ consume_gemini_sse (shared with Vertex)
    │                   │     └─ state machine: open text/thinking/toolCall
    │                   │
    │                   ├─ [AmazonBedrockProvider] → AWS EventStream → AssistantMessageEvent
    │                   │     amazon_bedrock.rs:36-49
    │                   │     ├─ Bearer token auth (no SigV4 yet)
    │                   │     ├─ POST /model/{id}/converse-stream
    │                   │     ├─ AwsEventStream framed decoding
    │                   │     └─ contentBlockStart/Delta/Stop → events
    │                   │
    │                   ├─ [MistralProvider, FauxProvider, etc.]
    │                   │
    │                   └─ Error path: error_stream()  [api_registry.rs:147-169]
    │                             │
    │                             └─ AssistantMessageEvent::Error
    │
    └─ Consumer iterates / awaits .result()
              │
              ├─ AgentHarness → compaction / next turn
              ├─ CLI /cost → Usage display
              └─ Session storage
```

### 3.2 设计要点

1. **Trait Object 注册表**：`ApiProvider` 是一个 trait（`async_trait`），每个 provider 实现它并注册到全局 `HashMap<String, RegisteredProvider>`。查询返回 `RegisteredHandle`，持有 `Arc<dyn ApiProvider>`，使 in-flight 流不因 unregister 而中断（`api_registry.rs:106-108`，已通过测试验证 `crates/ai/src/api_registry.rs:271-292`）。

2. **Feature Gating**：不同 provider 通过 Cargo features 编译，避免冷启动时加载不需要的 provider（`providers/mod.rs:11-70`）。`register_builtins` 的 `ensure()` 函数使用 `OnceLock` 保证仅注册一次。

3. **Sender/Receiver 分离**：`AssistantMessageEventStream::new()` 返回 `(stream, sender)` 对（`utils/event_stream.rs:71-83`）。provider 在 `tokio::spawn` 的 task 里持有 sender，consumer 持有 stream。Terminal event 同时 resolve 一个 `oneshot` 供 `result()` 使用。

4. **`SimpleStreamOptions` 翻译层**：上层用 `ThinkingLevel` + `ThinkingBudgets` 表达意图，每个 provider 自行翻译为 provider-specific knobs（`simple_options.rs:10-12`）。

5. **Cross-provider 消息变换**：`transform_messages.rs` 处理 provider 切换时的历史重写（downgrade images → placeholder、thinking → text、tool call id 规范化、orphaned tool call 合成）。

---

## 4. code_walkthrough — 源码走读

### 4.1 核心类型层

| 文件 | 关键类型/函数 | 在链路中的位置 |
|------|-------------|-------------|
| `types.rs` | `Api`, `KnownApi`, `Provider`, `Model`, `Message`, `ContentBlock`, `AssistantMessage`, `AssistantMessageEvent`, `StreamOptions`, `SimpleStreamOptions`, `Usage`, `ThinkingLevel`, `Tool`, `Context` | 全链路共享的类型宇宙 |
| `models.rs` | `get_model()`, `list_models()`, `register_custom_model()` | Model 查询入口；组合了静态 `BUILTIN_MODELS` + dynamic `custom_registry` |
| `models_generated.rs` | `BUILTIN_MODELS: Lazy<Vec<Model>>` | 来自 `models.generated.json`（`include_str!`），包含 ~500 个模型的静态描述 |

### 4.2 Provider 注册与分发层

| 文件 | 关键类型/函数 | 在链路中的位置 |
|------|-------------|-------------|
| `api_registry.rs` | `ApiProvider` trait, `register_api_provider()`, `get_api_provider()`, `RegisteredHandle`, `error_stream()` | Provider 注册和查询中枢 |
| `register_builtins.rs` | `ensure()` | 延迟一次性注册所有启用的 provider |
| `stream.rs` | `stream()`, `stream_simple()`, `complete()`, `complete_simple()`, `resolve()` | 对外暴露的顶层 API 入口 |
| `simple_options.rs` | `translate_base()` | `SimpleStreamOptions` → `StreamOptions` 的基类翻译 |

### 4.3 Provider 实现层

| 文件 | 关键结构/函数 | 特点 |
|------|-------------|------|
| `providers/anthropic.rs` | `AnthropicProvider`, `run()`, `handle_sse()`, `build_request_body()`, `resolve_compat()` | 完整实现（936 行）：SSE 解析、cache_control、thinking budget、Fireworks compat overrides、temperature 与 thinking 互斥 |
| `providers/openai_responses.rs` | `OpenAIResponsesProvider`, `run()`, `consume_responses_sse()`, `handle_event()`, `build_request_body()`, `convert_messages()` | 完整实现（1061 行）：Responses API、reasoning effort/summary、prompt_cache、DS4 reasoning replay、DS4 cache_write_tokens |
| `providers/google.rs` | `GoogleProvider`, `run()`, `consume_gemini_sse()`, `build_request_body()`, `translate_simple()` | 完整实现（539 行）：Gemini SSE、thinking_part 状态机、functionCall 完整对象处理、tool id 自动生成 |
| `providers/google_shared.rs` | `convert_messages()`, `convert_tools()`, `map_stop_reason()`, `is_thinking_part()` | Google / Vertex 共享的 message 转换和 thinking 检测 |
| `providers/google_vertex.rs` | `GoogleVertexProvider` | 复用 `google.rs` 的 SSE consumer，仅替换 auth 方式（ADC OAuth）和 URL |
| `providers/amazon_bedrock.rs` | `AmazonBedrockProvider`, `run()`, `consume()`, `build_request_body()` | 完整实现（546 行）：Bearer token auth、AWS binary eventstream 解码、contentBlockIndex 映射 |
| `providers/cloudflare.rs` | `resolve_cloudflare_base_url()`, `is_cloudflare_provider()` | 仅 URL placeholder 解析，非独立 provider |
| `providers/faux.rs` | `FauxProvider`, `set_faux_responses()`, `replay()` | 测试双精度：queue 预制的 `AssistantMessage` 并 replay 为 streaming events |
| `providers/mistral.rs` | `MistralProvider` | Mistral Conversations API |
| `providers/openai_completions.rs` | `OpenAICompletionsProvider` | Chat Completions API（老协议） |
| `providers/openai_codex_responses.rs` | `OpenAICodexResponsesProvider` | Codex-specific Responses |
| `providers/azure_openai_responses.rs` | `AzureOpenAIResponsesProvider` | Azure Responses，复用 `consume_responses_sse()` |
| `providers/transform_messages.rs` | `transform_messages()` | Cross-provider 历史消息重写 |
| `providers/openai_responses_shared.rs` | `placeholder()` | 占位符，待实现 |
| `providers/openai_prompt_cache.rs` | `placeholder()` | 占位符，待实现 |

### 4.4 传输/流处理层

| 文件 | 关键类型/函数 | 说明 |
|------|-------------|------|
| `utils/sse.rs` | `SseStream<S>`, `SseEvent` | 最小 SSE 解析器：`event:` 和 `data:` 字段 |
| `utils/aws_eventstream.rs` | `AwsEventStream<S>`, `EventStreamMessage` | AWS binary eventstream 解码器（轻量版，不验证 CRC） |
| `event_stream.rs` | `parse_message()`, `EventMessage`, `HeaderValue`, `crc32()` | AWS eventstream 完整解析器（含 CRC32 校验），用于 `sigv4` 模块 |
| `utils/retry.rs` | `send_with_retry()`, `is_retryable_status()` | HTTP 重试：408/409/425/429/5xx，指数退避 + jitter |
| `utils/abort.rs` | `send_or_abort()`, `next_or_abort()`, `sleep_or_abort()`, `push_aborted()` | CancellationToken 集成到所有 HTTP send / SSE drain / sleep 路径 |
| `utils/event_stream.rs` | `AssistantMessageEventSender`, `AssistantMessageEventStream`, `Stream` impl | Provider ↔ Consumer 的通道抽象 |
| `utils/node_http_proxy.rs` | `build_client()` | HTTP client 构建（含 proxy 支持） |

### 4.5 辅助工具层

| 文件 | 关键类型/函数 | 说明 |
|------|-------------|------|
| `utils/overflow.rs` | `is_context_overflow()`, `OVERFLOW_PATTERNS`, `NON_OVERFLOW_PATTERNS` | 30+ regex 模式 + 2 种静默检测 |
| `utils/json_parse.rs` | `parse_partial_json()`, `close_partial()` | Partial JSON 解析（关闭未闭合括号） |
| `utils/diagnostics.rs` | `AssistantMessageDiagnostic` | 结构化诊断信息（未在 provider 中实际使用） |
| `utils/validation.rs` | `validate()` | Tool input 校验（当前为 stub） |
| `env_api_keys.rs` | `get_env_api_key()`, `env_var_names()` | Provider → 环境变量映射（18+ entries） |
| `utils/hash.rs` | hash 工具 | |
| `utils/headers.rs` | header 工具 | |
| `utils/sanitize_unicode.rs` | unicode sanitization | |

---

## 5. flows — 典型流程

### 5.1 OpenAI Responses Streaming 流程

```
1. CLI/Agent 调用 stream(model, context, options)
   → stream.rs:resolve() 查找 registry
   → register_builtins::ensure() 确保已注册
   → get_api_provider("openai-responses") 返回 RegisteredHandle

2. handle.stream() → OpenAIResponsesProvider::stream() [openai_responses.rs:79-93]
   创建 (stream, sender) 对
   tokio::spawn async { run(model, context, options, sender) }

3. run() [openai_responses.rs:133-221]:
   a) resolve_openai_compatible_api_key()
      - 先查 options.api_key
      - 再查 env_api_keys::get_env_api_key(provider)
      - 最后回退 OPENAI_API_KEY
   b) resolve_compat(model)
      - 读 model.compat JSON 中的 sendSessionIdHeader / supportsLongCacheRetention /
        requiresReasoningContentOnAssistantMessages
   c) build_request_body()
      - convert_messages() → [{role, content: [{type: "input_text"}]}]
      - 如果 replay_reasoning → 插入 {"type":"reasoning"} 项到 assistant message 之前
      - 设置 prompt_cache_key = session_id, prompt_cache_retention (long→"24h")
      - 设置 reasoning.effort + reasoning.summary
      - 设置 tools → [{type: "function", name, description, parameters}]
   d) POST /v1/responses
      - headers: content-type json, accept text/event-stream, session_id, x-client-request-id
      - send_with_retry
   e) consume_responses_sse()
      - 初始化 SseStream
      - 循环 drain SSE events:
        - "response.created/.in_progress" → 记录 response_id
        - "response.output_item.added" → type=reasoning → ThinkingStart
                                    → type=function_call → ToolCallStart
        - "response.output_text.delta" → TextDelta
        - "response.output_text.done" → TextEnd
        - "response.reasoning_summary_text.delta" → ThinkingDelta
        - "response.reasoning_summary_text.done" → ThinkingEnd
        - "response.function_call_arguments.delta" → ToolCallDelta
        - "response.function_call_arguments.done" → parse_partial_json → ToolCallEnd
        - "response.completed" → update_usage + Done
        - "response.failed/.error/error" → Error

4. Consumer (Agent/Cli) 通过 Stream trait 迭代事件或 await result()
```

### 5.2 Anthropic Provider 流程

```
1. AnthropicProvider::stream() [anthropic.rs:110-124]
   → same pattern: spawn async run()

2. run() [anthropic.rs:176-301]:
   a) api_key: options.api_key || env ANTHROPIC_API_KEY
   b) resolve_compat(model):
      - 基于 provider 和 base_url 判断 Fireworks/Cloudflare 兼容模式
      - supportsLongCacheRetention, sendSessionAffinityHeaders,
        supportsCacheControlOnTools, supportsEagerToolInputStreaming
   c) build_request_body():
      - system → [{type: "text", text, cache_control}]
      - messages → [...], 最后一个 user/tool_result block 加 cache_control
      - tools → [{name, description, input_schema, cache_control on last tool}]
      - thinking.enabled 时 temperature 被移除（Anthropic API 要求）
   d) POST /v1/messages
      - headers: x-api-key, anthropic-version: 2023-06-01,
        anthropic-beta: prompt-caching-2024-07-31, interleaved-thinking-2025-05-14,
        fine-grained-tool-streaming-2025-05-14
      - send_with_retry
   e) consume SSE:
      - SseStream + handle_sse()
      - "message_start" → update_usage from /message/usage, 记录 message.id
      - "content_block_start" → type=text→TextStart, thinking→ThinkingStart,
                                redacted_thinking→ThinkingStart(redacted),
                                tool_use→ToolCallStart
      - "content_block_delta" → text_delta→TextDelta, thinking_delta→ThinkingDelta,
                                input_json_delta→ToolCallDelta (buffer),
                                signature_delta→累加到 thinking_signature
      - "content_block_stop" → ToolCall: parse_partial_json(buffer) → ToolCallEnd
                              Text/Thinking: 对应的 End event
      - "message_delta" → update stop_reason + usage
      - "message_stop" → Done
      - "error" → Error
```

### 5.3 DS4 / Local OpenAI-Compatible Path 流程

```
1. Model descriptor (models.generated.json):
   {
     "provider": "ds4",
     "api": "openai-responses",
     "baseUrl": "http://127.0.0.1:8000/v1",
     "compat": { "requiresReasoningContentOnAssistantMessages": true },
     ...
   }

2. 请求走 OpenAIResponsesProvider → openai_responses.rs

3. resolve_compat() 读取 compat.requiresReasoningContentOnAssistantMessages = true
   → replay_reasoning_content = true

4. convert_messages() [openai_responses.rs:662-735]:
   对于每个 AssistantMessage:
   - Thinking content block → 作为独立的 {"type":"reasoning", "summary": [...]} input item
     插入到 assistant message **之前**（DS4 的 merge 规则要求这个顺序）
   - 其他 content block 正常转换

5. POST /v1/responses
   - base_url = http://127.0.0.1:8000 → URL = "http://127.0.0.1:8000/v1/responses"
   - send_with_retry 处理可能的 409 Conflict

6. retry layer [utils/retry.rs:139-143]:
   - 409 被归类为 retryable status
   - DS4 返回 409 意味着 live continuation state 已驱逐
   - pie 的请求始终包含完整 history，因此直接重试即可重建 KV checkpoint

7. usage 处理 [openai_responses.rs:557-562]:
   - 读取非标准字段 input_tokens_details.cache_write_tokens
   - 填入 Usage.cache_write（DS4 的标准字段是 input_tokens_details.cached_tokens）

8. Google Vertex 复用 Google SSE consumer:
   google_vertex.rs 使用 GoogleVertexProvider，其 stream() 调用
   google::consume_gemini_sse() 处理相同的 Gemini SSE 格式，
   区别在于 auth（OAuth2 ADC token vs API key）和 URL（vertex API endpoint）
```

---

## 6. tests — 支撑判断的测试/示例文件

| 测试位置 | 测试意图 |
|---------|---------|
| `api_registry.rs:171-335` | Registry 正确性：handle 在 unregister/clear 后仍可用、mismatch api 返回 error stream、stream_simple 同样存活 |
| `event_stream.rs:238-310` | AWS eventstream 解析：CRC32 标准向量校验（`0xCBF43926`）、完整 frame round-trip、bad prelude CRC 拒绝、short buffer 报告 |
| `utils/event_stream.rs:110-163` | EventStream 通道：迭代到 Done、result() 在 drain 前 resolve |
| `utils/retry.rs:170-202` | Retry 状态码分类、409 可重试、退避增长和 cap |
| `utils/overflow.rs:101-174` | Overflow 检测：Anthropic/OpenAI/Gemini 错误消息、rate limit 排除、静默溢出 via usage / length-stop |
| `utils/json_parse.rs:73-107` | Partial JSON：完整对象、未闭合对象、未闭合字符串值、trailing comma、空输入 |
| `providers/anthropic.rs:780-936` | Anthropic：cache_control 作用于 system + last user、long retention 加 ttl、temperature 在 thinking 时移除、tools cache_control on last、Fireworks compat |
| `providers/openai_responses.rs:825-1061` | OpenAI Responses：URL 不双拼 /v1、system prompt 包含、long retention 设 24h + cache_key、reasoning effort 设 reasoning block、thinking replay（DS4）、thinking drop 无 compat flag、usage 读 cached_tokens + cache_write_tokens、tool_call 序列化为 function_call、thinking item 在 assistant message 之前 |
| `providers/google.rs:476-538` | Google：body 有 contents 和 systemInstruction、thinking budget 设 generationConfig、tools 转为 functionDeclarations |
| `providers/amazon_bedrock.rs:493-546` | Bedrock：converse body shape、tool result 转换、stop_reason 映射 |
| `providers/cloudflare.rs:62-98` | Cloudflare：URL placeholder passthrough / 缺失 env 报错 |
| `providers/faux.rs:250-323` | Faux：queue 文本 + tool_call replay、回退到 canned 消息 |
| `providers/transform_messages.rs:220-352` | Cross-provider transform：thinking → text、errored assistant 跳过、orphaned tool call 合成、images downgrade |
| `env_api_keys.rs:54-59` | DS4 使用专用 DS4_API_KEY |
| `utils/abort.rs:137-151` | abort 集成：next_or_abort 在 CancellationToken 被 cancel 时返回 Aborted |
| `utils/sse.rs` (内联测试无) | SSE 解析器无独立测试，通过各 provider 的 integration 测试间接覆盖 |
| `docs/ds4.md` | DS4 集成验证步骤：`/cost` 显示 cache_read 主导、重启 server 后 409-retry 恢复、cache_write 出现 |

---

## 7. risks — 未读风险、TODO、可能的边界 bug

### 7.1 已标注 TODO

- **Anthropic** (`anthropic.rs:14-20`)：adaptive thinking (`effort` knob)、interleaved-thinking beta toggle、OAuth bearer-token auth、redacted_thinking 处理、tool-choice 配置、fine-grained tool-streaming beta header 协商。当前 `redacted_thinking` 用硬编码 `"[Reasoning redacted]"` 文本代替（第 403-418 行），可能丢失原始 data 信息。

- **OpenAI Responses** (`openai_responses.rs:14-19`)：cross-provider transform_messages、GitHub Copilot 动态 headers、Cloudflare AI Gateway URL rewriting、tool-call id `call|item` 规范化、service_tier pricing multiplier、`output_text.done`/`function_call_arguments.done` final-state reconciliation。

- **Google / Google Vertex** (`google.rs:8-9`)：Gemini 3 / Gemma thinking-level selection、multimodal functionResponse parts、thoughtSignature replay correctness、tool-choice config。

- **Amazon Bedrock** (`amazon_bedrock.rs:12-16`)：**SigV4 request signing 未实现**（当前仅支持 Bearer token）、prompt caching (`cachePoint` blocks)、thinking budget / display modes、tool results 中的 image content blocks。

- **Cross-provider**：`openai_responses_shared.rs` 和 `openai_prompt_cache.rs` 为 placeholder（仅含 `placeholder()` 函数），对应 TS 端的重要共享逻辑尚未移植。

- **Validation** (`utils/validation.rs:20-27`)：`validate()` 为 stub，始终返回 `valid: true`。TS 端使用 typebox 运行时校验，Rust 端尚未接入 `jsonschema`。

### 7.2 潜在边界问题

- **Streaming body retry** (`utils/retry.rs:51-57`)：当 request body 不可 clone（streaming body）时，retry 降级为 single-shot。所有当前请求使用 JSON body，所以实际未触发此路径，但未来若支持 streaming upload 会有风险。

- **`Sender::is_closed` 竞态** (`utils/event_stream.rs:49-51`)：`is_closed()` 和 `push()` 之间存在 check-then-act 竞态窗口。虽然 `push()` 内部 swallow 了 send error（第 45 行），但 `is_closed()` 返回 false 后 consumer 可能立即 drop，导致 `push()` 失败但 provider 继续运行一段无意义的路径。

- **Google tool call 不流式** (`google.rs:267-317`)：Gemini 的 `functionCall` 是一次性完整返回的，但代码在 `ToolCallStart` 后立即发 `ToolCallDelta` + `ToolCallEnd`（第 307-316 行），消费者侧可能假设 tool call 参数是逐步到达的。

- **Google stop_reason 优先级** (`google.rs:321-333`)：finishReason 先被 `map_stop_reason` 处理，然后如果 content 中有 tool call 就强制覆盖为 `ToolUse`。这意味着一个同时有 text output 和 tool call 的 turn 会被标记为 ToolUse——这符合语义但可能掩盖某些边界情况（如 tool call 后紧接着 STOP reason 的 partial output）。

- **DS4 reasoning replay 顺序敏感** (`openai_responses.rs:692-699`)：reasoning item 被 `push` 到 `out` 在 assistant message 之前，但 code 中 reasoning 来自 assistant content block 的迭代。这是符合 DS4 协议的，但文档明确标注 "ordering is load-bearing"——如果未来改变消息处理顺序，DS4 cache 会静默失效。

- **Anthropic temperature 与 thinking 互斥** (`anthropic.rs:611-616`)：temperature 仅在 `thinking_enabled == false` 时设置。但如果 Anthropic 未来允许两者共存，此逻辑需要更新。

- **Bedrock 无流式 text start 事件** (`amazon_bedrock.rs:211-231`)：text delta 首次到来时根据 `is_first` 判断发 `TextStart`，但这不是标准的事件驱动方式（与 Anthropic/OpenAI 的显式 `content_block_start` 不同）。

- **Cloudflare URL 解析** (`cloudflare.rs:30-59`)：手写的 `{VAR}` 替换只处理简单情况，不支持 `${VAR}` 或嵌套、不支持默认值。若 Cloudflare 的 URL 模板格式变更，此解析器会失败。

- **CRC32 在 aws_eventstream.rs 中不验证** (`utils/aws_eventstream.rs:8-9`)：注释明确说 "CRCs are not verified"。Bedrock stream 的 CRC 校验被跳过，若网络传输损坏可能导致 corrupt JSON 被错误解析。

- **UsageCost 未填充**：`types.rs:300-308` 定义了 `UsageCost` 结构体，但各 provider 的 `update_usage()` 都只填充了 token 计数，没有乘以 `Model.cost` 计算 USD 成本。

### 7.3 未读代码

- `sigv4.rs`（AWS SigV4 签名实现）未阅读
- `bedrock_anthropic.rs` 和 `bedrock_provider.rs`（Bedrock Anthropic passthrough）未阅读
- `vertex_adc.rs`（Google Vertex ADC OAuth）未阅读
- `oauth/` 目录（OAuth helpers）未阅读
- `session_resources.rs`（session 资源清理）未阅读
- `image_models.rs` / `images.rs` / `images_api_registry.rs`（图像模型）未阅读
- `cli.rs`（pie-ai 内 CLI helpers）未阅读
- `utils/oauth/` 子目录未阅读
- `providers/images/` 子目录未阅读

---

## 8. next_questions — 下一轮精读应继续追问

1. **SigV4 签名**：`sigv4.rs` 的实现完整度如何？Bedrock provider 的 TODO 标注了 SigV4 未接入，这是 AWS 标准 auth 路径，为什么选择先实现 Bearer token？SigV4 签名模块（`event_stream.rs` 的 CRC32 已经实现了，`sigv4.rs` 是否复用？）

2. **OAuth / ADC 流程**：Google Vertex 的 ADC token 刷新机制（`vertex_adc.rs`）如何处理 token 过期和 401 retry？整个 OAuth 链路（`oauth/` 目录）支持哪些 grant type？

3. **跨 Provider 会话迁移**：`transform_messages.rs` 在什么场景下被调用？Agent harness 是如何检测需要 provider handoff 的？tool call id 规范化（`ToolCallIdNormalizer` 回调）的调用侧在哪里？

4. **Compaction 触发链**：`is_context_overflow` 返回 true 后，Agent harness 如何触发 compaction？compaction 流程是否依赖 pie-ai 层提供任何接口？

5. **Cost accounting 完整链路**：`Usage.cost` 是在哪一层被填充的？Agent harness 还是 pie-ai 层？Model cost 数据（`ModelCost`）来自 catalog JSON，但乘以 usage 的计算逻辑在哪个文件中？

6. **OpenAI Completions 与 Responses 的差异**：`openai_completions.rs` 的实现完整度如何？Chat Completions API 和 Responses API 的 message 格式差异在哪里处理？

7. **GitHub Copilot header 注入**：`github_copilot_headers.rs` 的动态 header 生成逻辑是什么？如何在 OpenAI Responses 和 Anthropic Messages provider 中注入这些 headers？

8. **Session 管理**：`StreamOptions.session_id` 是如何传递到 provider 的？不同 provider 对 session/continuation 的支持程度不同（Anthropic session affinity、OpenAI prompt_cache_key、Google 无显式 session），pie-ai 如何统一？

9. **Image support**：图像 content block 在每个 provider 中的转换方式不同（Anthropic `source: { type: base64 }`、OpenAI `input_image`、Google `inlineData`、Bedrock `image: { format, source: { bytes } }`）。是否有统一的图像预处理/压缩/格式转换层？

10. **Error propagation vs panic**：`error_stream()` 契约要求 provider 把错误编码到返回的 stream 中而不是 panic/throw。是否有任何 provider 违反了这一契约（例如在同步代码路径上 panic）？Abort 路径（`push_aborted`）是否正确处理了所有 `tokio::select!` 分支？

11. **Streaming tool call 的 partial JSON 解析安全性**：`parse_partial_json` 关闭未闭合括号的策略是否在所有 provider 的 tool call 格式下都是安全的？Google 的 `functionCall` 是一次性完整返回的，但 Anthropic 和 OpenAI 是流式的——这两种模式是否有不同的解析路径？

12. **测试覆盖**：`utils/sse.rs` 没有独立单元测试。SSE 解析依赖各 provider 的集成测试间接验证——是否有专门的 mock HTTP server 测试（类似 TS 端的 vitest + nock）？`aws_eventstream.rs` 的 CRC 不验证在 `utils/` 版本中，而 `event_stream.rs` 版本有完整 CRC 验证——为什么有两套实现？能否合并？
