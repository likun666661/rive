# pie 架构总览

> 阅读基线: `f1c35a3` | 深度档位: `architecture` | 日期: 2026-06-13
> 上游产物: 01-ai-provider-streaming, 02-agent-core-runtime, 03-coding-cli-tools, 04-automation-loops-triggers, 05-mcp-and-fefe-hub, 06-roadmap-docs-issues

---

## 1. executive_summary

1. **pie 是一个本地优先、可长期运行的 AI coding agent runtime**，由 Rust 2024 workspace 构建，分三层 `crates/ai`（LLM 流式客户端）、`crates/agent`（agent 运行时）、`crates/coding-agent`（CLI/TUI/工具壳），外加独立的 `crates/mcp`（MCP 客户端库）。

2. **pie-ai 统一了 8+ 模型供应商**（OpenAI Responses、Anthropic Messages、Google Gemini、Amazon Bedrock、Mistral、Codex、Azure、Faux 测试双精度），核心链路为 `stream()` → trait object 注册表分派 → provider-specific SSE/binary eventstream 解码 → 统一 `AssistantMessageEvent` 流。

3. **AgentHarness 是运行时的装配层**，将 `Agent`（纯状态机、IO-free）、`SessionStorage`（append-only JSONL + memory）、`Compaction`（自动摘要压缩）、`CostTracker`（token/cost 累加）、`TriggerRuntime`（事件去重/循环抑制）组装为完整的 agent 生命周期。

4. **编码工具链通过 `coding-agent` 层暴露**：11 个核心工具（read/write/edit/bash/ls/grep/find/web_fetch/web_search/git/memory） + MCP 适配工具 + Skill/Task/Trigger/Cron 高级工具，经 `AgentTool` trait + 双层权限审核接入 agent loop。

5. **自动化引擎由三层组成**：Cron（定时触发，支持 stateful loop 模式 + 纯文本 `<loop-state>/<inbox>` 协议）、Dynamic Trigger（自然语言规则 → 周期性 sub-agent 评估）、MCP Notification Hook（双向通知映射为 Trigger 信封），三者共享统一的 dedup + cycle suppression 运行时。

6. **Compaction 是长会话的核心机制**：当 context tokens 超过 context_window 的 80% 自动触发，在 turn-boundary 安全的切点对旧消息做 LLM 摘要，摘要结果以 `Compaction` session entry 持久化，resume 时跳过被压缩的消息。

7. **会话持久化采用 append-only JSONL + tree DAG**：每个 entry 有 `parent_id` 形成分支树，支持 `--resume`/`/undo`/`move_to` 等操作；全局 inbox (`~/.pie/inbox.jsonl`) 做 loop 产物的 triage 收集；session export 为 `.piesession` tar 归档。

8. **MCP 客户端支持 stdio 和 Streamable HTTP 两种传输**，通过 `tools/list → tools/call` 路径将外部工具暴露为 `McpAgentTool`；通知路径通过 `McpNotificationHook` 映射为 Trigger，进入统一的 trigger 运行时管线。

9. **fefe hub 公共跨 agent 服务已明确移除**（2026-06-10），唯一保留的网络表面是 Web Relay（`/web-connect` → Cloudflare Worker DO → capability URL → 浏览器观看 + 远程 prompt + remote approval）。

10. **本地模型 (DS4) 优化是独特的工程亮点**：通过 byte-exact reasoning 重放、HTTP 409 透明重试、cache_write token 报告三个修复，确保 DS4 的 KV prefix cache 在长会话中持续命中。

11. **测试覆盖扎实但不均衡**：整体约 225+ 测试，trigger runtime 有 21+ 集成测试，危险 bash 有 25 个变体；但 SSE 解析器无独立单元测试，`utils/sse.rs` 依赖各 provider 间接验证，DS4 集成测试需手动启动真实服务器。

12. **最值得继续精读的部分**：`AgentHarness::handle_trigger` 完整状态机、`cron_harness_listener` 的 tag 提取 + inbox 写入链路、JSONL session 的多 leaf 分支表示、LSP supervisor 的实际集成深度、provider 层面 `input_json_delta` 碎片组装的跨 provider 一致性。

---

## 2. architecture_map

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                              pie-coding-agent (CLI/TUI/Web)                           │
│                                                                                       │
│  ┌──────────┐  ┌──────────────┐  ┌──────────┐  ┌──────────────┐  ┌───────────────┐  │
│  │ main.rs  │  │ commands.rs  │  │ ui/       │  │ tools/       │  │ triggers/     │  │
│  │ CLI args │  │ 28 slash     │  │ ratatui   │  │ read,write,  │  │ cron, dynamic,│  │
│  │ → config │  │ commands     │  │ TUI + Web │  │ edit,bash,ls,│  │ mcp_notify    │  │
│  │ → model  │  │ Tab completion│  │ + Headless│  │ grep,find,   │  │ inbox, hooks, │  │
│  │ → session│  │              │  │           │  │ web_fetch,   │  │ otlp, debug   │  │
│  └────┬─────┘  └──────┬───────┘  └─────┬─────┘  │ git,memory,  │  └───────┬───────┘  │
│       │               │                │        │ task,skill,  │          │          │
│       │               │                │        │ mcp_adapter  │          │          │
│       │               │                │        └──────┬───────┘          │          │
│       │               │                │               │                  │          │
│       ▼               ▼                ▼               ▼                  ▼          │
│  ┌──────────────────────────────────────────────────────────────────────────────┐    │
│  │                         AgentHarness (装配层)                                  │    │
│  │                                                                               │    │
│  │  ┌──────────┐  ┌──────────────┐  ┌───────────┐  ┌──────────────┐            │    │
│  │  │ Session  │  │ Compaction   │  │ Cost      │  │ Trigger      │            │    │
│  │  │ 管理     │  │ 自动摘要压缩 │  │ Tracker   │  │ Runtime      │            │    │
│  │  │ JSONL    │  │ 80% thresh  │  │ budget cap │  │ dedup+cycle  │            │    │
│  │  │ tree DAG │  │ turn-bound  │  │ saturating │  │ 5min / 5hop  │            │    │
│  │  └────┬─────┘  └──────┬───────┘  └─────┬─────┘  └──────┬───────┘            │    │
│  │       │               │                │               │                     │    │
│  │       ▼               ▼                ▼               ▼                     │    │
│  │  ┌────────────────────────────────────────────────────────────────────────┐  │    │
│  │  │                         Agent (纯状态机, IO-free)                       │  │    │
│  │  │                                                                         │  │    │
│  │  │  drive_loop:                                                            │  │    │
│  │  │    call_llm(model, context, tools) → stream_fn → AssistantMessageEvent  │  │    │
│  │  │    execute_tools(assistant) → permission → before/after hooks           │  │    │
│  │  │    should_stop / prepare_next_turn / steering queue drain               │  │    │
│  │  │                                                                         │  │    │
│  │  │  Hooks: BeforeToolCall, AfterToolCall, ShouldStop, PrepareNextTurn,     │  │    │
│  │  │         OnControlPlanePrompt, OnTurnEnd                                 │  │    │
│  │  └──────────────────────────────────┬─────────────────────────────────────┘  │    │
│  └─────────────────────────────────────┼────────────────────────────────────────┘    │
│                                        │                                              │
│                                        ▼ stream_fn (pie_ai::stream_simple)            │
│  ┌─────────────────────────────────────────────────────────────────────────────┐     │
│  │                            pie-ai (LLM 流式客户端)                            │     │
│  │                                                                              │     │
│  │  stream.rs: resolve(model) → api_registry → RegisteredHandle                │     │
│  │                                                                              │     │
│  │  ┌──────────────────────────────────────────────────────────────────────┐   │     │
│  │  │  ApiProvider trait 注册表                                              │   │     │
│  │  │                                                                        │   │     │
│  │  │  AnthropicProvider → SSE → Messages API                                │   │     │
│  │  │  OpenAIResponsesProvider → SSE → Responses API                         │   │     │
│  │  │  GoogleProvider → SSE → Gemini (shared with Vertex)                    │   │     │
│  │  │  AmazonBedrockProvider → AWS EventStream → Converse API                │   │     │
│  │  │  MistralProvider / OpenAICompletionsProvider / FauxProvider / ...      │   │     │
│  │  │                                                                        │   │     │
│  │  │  utils/: sse, aws_eventstream, event_stream, retry, abort, overflow,   │   │     │
│  │  │          json_parse, node_http_proxy, hash, env_api_keys               │   │     │
│  │  └──────────────────────────────────────────────────────────────────────┘   │     │
│  │                                                                              │     │
│  │  types.rs: Api, Model, Message, ContentBlock, AssistantMessageEvent,         │     │
│  │             Usage, ThinkingLevel, StreamOptions, Context                      │     │
│  │  models.rs / models_generated.rs: ~500 static models + custom registry       │     │
│  └──────────────────────────────────────────────────────────────────────────────┘     │
│                                                                                       │
│  ┌──────────────────────────────────────────────────────────────────────────────┐     │
│  │                     pie-mcp (MCP 客户端, 独立 crate)                           │     │
│  │                                                                                │     │
│  │  protocol.rs: JSON-RPC 2.0, McpTool, ToolsListResult, McpToolCallResult       │     │
│  │  transport.rs: trait Transport (newline-delimited JSON)                       │     │
│  │  client.rs: McpClient, inflight management, cancel, read pump                 │     │
│  │  stdio.rs: child process stdin/stdout communication                          │     │
│  │  http.rs: Streamable HTTP (POST + SSE), Bearer auth, reconnect               │     │
│  └──────────────────────────────────────────────────────────────────────────────┘     │
│                                                                                       │
│  ┌──────────────────────────────────────────────────────────────────────────────┐     │
│  │  workers/fefe-hub/ (Cloudflare Worker, 仅保留 Web Relay)                       │     │
│  │                                                                                │     │
│  │  SessionRelay DO: TOFU agent key pinning, snapshot broadcast, viewer SSE,     │     │
│  │                    prompt forwarding, control-plane resolve, abort             │     │
│  │  Legacy paths → 410 Gone                                                      │     │
│  └──────────────────────────────────────────────────────────────────────────────┘     │
└─────────────────────────────────────────────────────────────────────────────────────┘

存储布局 (~/.pie/):
├── sessions/<cwd-hash>/<uuid>.jsonl          # 会话历史 (append-only JSONL, tree DAG)
├── sessions/<cwd-hash>/<uuid>.triggers.json  # session-scoped 动态触发规则
├── sessions/<cwd-hash>/<uuid>.cron.toml      # session-scoped cron jobs
├── sessions/<cwd-hash>/<uuid>.loop-<id>.md   # stateful loop 状态文件
├── inbox.jsonl                               # 全局 triage inbox
├── auth.json (mode 0600)                      # 存储的 API keys
├── models.json                                # 自定义模型定义
├── mcp.toml                                   # MCP 服务器配置
├── hooks.toml                                 # CLI hooks 配置
├── config.toml                                # 用户配置
├── logs/<session>.log                         # 结构化日志
├── skills/<name>/SKILL.md                     # 用户全局技能
└── history                                    # 输入历史 (容量 1000)
```

---

## 3. deep_read_index

| 序号 | 主题 | 建议阅读文件 | 为什么值得深读 | 预期产物 |
|------|------|-------------|---------------|----------|
| 1 | AgentHarness::handle_trigger 完整状态机 | `crates/agent/src/harness/agent_harness.rs` (trigger 处理段)、`crates/agent/src/harness/trigger.rs` | Trigger 从 Receive → Accept → Running → Completed/Failed 的每一步状态转换都在此实现，是自动化引擎的心脏 | 状态机时序图 + 每个状态的副作用清单 |
| 2 | Loop 标签提取 + Inbox 写入链路 | `crates/coding-agent/src/triggers/cron.rs` (`cron_harness_listener`, `extract_tag_block`)、`crates/coding-agent/src/inbox.rs` | stateful cron 的核心收尾逻辑：从模型输出中解析 `<loop-state>`/`<inbox>` 并持久化，是整个 Loop Engineering 模式的闭环 | 标签提取的边界 case 分析 + inbox 并发安全性评估 |
| 3 | JSONL Session 的多 leaf 分支表示 | `crates/agent/src/harness/session/session.rs`、`crates/agent/src/harness/session/jsonl_repo.rs` | 当前 session 是线性的 append-only JSONL，但 #17 多 leaf session 设计需要 `active_leaf_id` 和 `branch_from`——这两个机制的实现细节决定 resume/undo 的可靠性 | 分支语义的源码级描述 + 与 `parent_id` DAG 的映射关系 |
| 4 | LSP Supervisor 实际集成深度 | `crates/coding-agent/src/lsp_supervisor.rs`、`crates/coding-agent/src/main.rs:781-786` | 上游报告提及 after_tool_call hook 注入 diagnostics，但支持的语言、启动延迟处理、诊断与 edit 的交互均未深入 | 支持的 LSP 清单 + 诊断注入时序 + 性能影响评估 |
| 5 | input_json_delta 碎片组装的跨 provider 一致性 | `providers/anthropic.rs`、`providers/openai_responses.rs`、`providers/amazon_bedrock.rs` | 三个 provider 对流式 tool call arguments 的处理方式不同（Anthropic 流式 delta、OpenAI 流式 delta、Google 一次性返回、Bedrock 流式 delta），需验证 `parse_partial_json` 在所有路径的行为 | 跨 provider tool call 解析差异表 + 潜在 bug 清单 |
| 6 | `/goal` evaluator prompt 与 TurnEnd 循环 | `crates/agent/src/harness/agent_harness.rs` (`run_evaluator`)、`crates/agent/src/types.rs` (`OnTurnEndHook`) | evaluator 是无 tool 的裸 Agent，其 prompt 策略、bounded transcript 裁剪方式、false positive/negative 缓解是 goal 模式可靠性的关键 | evaluator prompt 模板 + 循环终止条件分析 |
| 7 | Provider Handoff + transform_messages 触发路径 | `providers/transform_messages.rs`、`crates/agent/src/agent_loop.rs` (`convert_to_llm`) | 跨 provider 消息重写（images downgrade、thinking→text、tool call id 规范化）何时触发？在 compaction/resume 场景中是否引入一致性问题？ | 触发场景清单 + transform 前后消息对比 |
| 8 | Streaming tool call 的 partial JSON 安全性 | `utils/json_parse.rs`、`utils/overflow.rs` | `parse_partial_json` 关闭未闭合括号的策略在 Anthropic/OpenAI/Bedrock 三种流式 tool call 格式下是否普遍安全？Google 的一次性 `functionCall` 走不同路径 | 各 provider 的 tool call 解析路径对比 + fuzz 建议 |
| 9 | Inbox JSONL 跨进程一致性 | `crates/coding-agent/src/inbox.rs` | 文档声称"多进程 append 可容忍，status 更新 last-writer-wins"——实现是否依赖文件锁？是否有竞态导致 claim/dismiss 丢失？ | 并发安全性分析 + 修复建议（如需要） |
| 10 | Anthropic + OpenAI 的 cache_control 策略对比 | `providers/anthropic.rs` (build_request_body), `providers/openai_responses.rs` (prompt_cache) | Anthropic 用显式 `cache_control` 断点，OpenAI 用 `prompt_cache_key`——两种缓存的预算策略、作用范围、与 compaction 的交互截然不同 | 缓存策略对比表 + 长对话最优缓存策略 |

---

## 4. cross_module_flows

### 4.1 普通 Coding Turn（用户输入 → 工具执行 → 回复）

```
CLI (main.rs)
  │ 用户输入 "fix the bug in src/main.rs"
  ▼
App::run() → kernel.run_prompt(prompt)
  │ 通过 feed channel 将 user message 推入 TUI
  ▼
AgentHarness::prompt(text)
  ├─ check_budget_cap() → Ok
  ├─ run_auto_compaction() → tokens < 80% window, skip
  └─ run_turn_with_continuation(user_msg)
       │
       ▼
     Agent::prompt(AgentMessage)
       ├─ emit AgentStart
       ├─ append user msg → emit MessageStart/End
       └─ drive_loop:
            │
            ├─ [1] call_llm:
            │     agent_loop.rs → transform_context → convert_to_llm
            │     → stream_fn (pie_ai::stream_simple)
            │       → api_registry::get_api_provider()
            │       → OpenAIResponsesProvider::stream()
            │         → build_request_body (messages + tools + reasoning)
            │         → POST /v1/responses (SSE)
            │         → consume_responses_sse → AssistantMessageEvent stream
            │     → emit MessageStart/TextDelta/ThinkingDelta/.../MessageEnd
            │
            ├─ [2] execute_tools:
            │     assistant.content → [ToolCall { name: "edit", args: {...} }]
            │     → permission_classification → Allow
            │     → before_tool_call hook (LSP supervisor, PermissionPolicy)
            │     → EditTool::execute(filePath, oldString, newString)
            │       → read file → match oldString → replace → write
            │     → after_tool_call hook (LSP diagnostics)
            │     → emit ToolExecutionStart/Update/End
            │
            ├─ [3] should_stop_after_turn: stop_reason=ToolUse → 继续循环
            │     → 回到 [1] call_llm (LLM 看到 tool result, 生成最终回复)
            │     → stop_reason=Stop → 退出 loop
            │
            └─ finalize: emit AgentEnd
                  │
                  ▼
            CostTracker 监听 MessageEnd → 累加 Usage tokens + cost
            Session listener → append MessageEnd entries to JSONL
```

**跨模块边界**: CLI → agent-core (AgentHarness/Agent) → pie-ai (stream) → agent-core (execute_tools) → coding-agent (tool impl) → agent-core (persist)

---

### 4.2 Session Resume / Compaction 恢复

```
pie --resume
  │
  ▼
main.rs: resume(repo, id)
  │ session::find_session_path(repo, files, id) → UUIDv7 前缀匹配
  ▼
JsonlSessionRepo::open(session_path) → Session facade
  │
  ▼
AgentHarness::new(opts)  # opts 包含 session
  │
  ▼
harness.rehydrate_from_session()
  │
  ▼
session.build_context()
  │ session.branch(None) → get_path_to_root (沿 parent_id 链)
  │
  ▼
build_session_context(path_entries)
  ├─ 扫描 ThinkingLevelChange / ModelChange → 恢复 thinking_level, model
  ├─ 遇到 Compaction { summary, first_kept_entry_id }:
  │     → 取 summary 作为 Custom message 注入 agent state
  │     → 跳过 parent_id ≤ first_kept_entry_id 的所有后续 entries
  └─ 收集未被压缩的 Message + CustomMessage → agent.state.messages
  │
  ▼
agent.state 恢复: messages, thinking_level, model
  │
  ▼
app.replay(ctx.messages) → 在 TUI feed 中重放历史消息
  │
  ▼
REPL 正常启动，用户可以继续对话
```

**跨模块边界**: coding-agent (session CRUD) → agent-core (SessionStorage trait) → agent-core (Compaction entry 语义) → coding-agent (TUI replay)

---

### 4.3 Stateful Cron Loop（完整生命周期）

```
用户: /cron add --stateful "0 9 * * *" check GitHub issues
  │
  ▼
NewCronJobTool::execute()
  │ cron_registry.add_job_full(schedule, action, stateful=true)
  │ → CronJob { stateful: true, ... } 写入 <session>.cron.toml
  │
  ▼
CronNotificationHook::run() (每 30s 扫描)
  │ registry.due_jobs(last_scan, now) → 到期 job
  │ → Trigger { idempotency_key: "cron:<job_id>:<due_at>", ... }
  │ → sink.send(trigger)
  │
  ▼
TriggerRuntime::evaluate(trigger)
  ├─ dedup check: 同 key 在 5min 窗口内 → Accept (首次)
  └─ cycle check: trace_id hop 计数 < 5 → Accept
  │
  ▼
AgentHarness::handle_trigger(trigger)
  ├─ BeforeTriggerHook → Allow
  │
  ▼
cron_action_hook (BeforeTriggerActionHook):
  │ job.stateful == true
  │ → compose_stateful_prompt(action, loop_state)
  │   → 注入 <loop-state>上次状态</loop-state> + output protocol
  │ → TriggerAction { delivery: SubAgent, prompt }
  │
  ▼
spawn_trigger_action(trigger)
  │ tokio::spawn → 独立 sub-harness (MemorySessionStorage)
  │ → sub-harness.prompt(action.prompt)
  │   → Agent::prompt → drive_loop → call_llm + execute_tools
  │     → 模型输出:
  │       <loop-state>checked issues #1-#10, new: none</loop-state>
  │       <inbox>PR #42 open 2 weeks without review</inbox>
  │
  ▼
cron_harness_listener 监听 HarnessEvent::TriggerCompleted
  ├─ extract_tag_block(summary, "loop-state")
  │   → write_loop_state(path, state)  (≤2000 chars)
  └─ extract_tag_all(summary, "inbox", 16)
      → inbox::append(inbox_path, ...) → ~/.pie/inbox.jsonl
  │
  ▼
用户下次打开 pie:
  /inbox → 看到 finding #1
  /inbox claim 1 → 主对话收到 "Address this finding: PR #42..."
```

**跨模块边界**: coding-agent (slash command / cron registry) → agent-core (Trigger/TiggerRuntime) → agent-core (sub-agent) → pie-ai (LLM call) → coding-agent (tag extract + inbox append)

---

### 4.4 MCP Notification → Trigger Pipeline

```
MCP Server (filesystem) 推送: notifications/resources/updated { uri: "file:///src/main.rs" }
  │
  ▼
McpClient read pump (crates/mcp/src/client.rs)
  │ JSON-RPC 帧无 id → 路由到 notification channel
  │ → tx.send(McpServerNotification { method, params })
  │
  ▼
McpNotificationHook::run() (crates/coding-agent/src/triggers/mcp_notification_hook.rs)
  │ rx.recv() → map_notification("filesystem", notification)
  │   method = "notifications/resources/updated"
  │   → idempotency_key = "mcp:filesystem:resources:file:///src/main.rs"
  │   → replacement_policy = LatestReplaces
  │ → payload_visibility = Local (params 丢弃, 仅保留 _meta.pie_summary)
  │ → Trigger { source: Mcp { server_name, method }, ... }
  │ → sink.send(trigger)
  │
  ▼
TriggerRuntime::evaluate(trigger)
  ├─ dedup: 同 URI 在窗口内 → LatestReplaces: 替换前一个 pending trigger
  └─ cycle check → Accept
  │
  ▼
AgentHarness::handle_trigger(trigger)
  ├─ 审计: 写入 SessionTreeEntry::Custom { custom_type: "trigger" }
  │
  ▼
BeforeTriggerActionHook → direct_inject_action_hook (如果 server ∈ inject_and_run_servers)
  │ → TriggerDelivery::InjectAndRun → 将 summary 注入主对话, LLM 立即反应
  │ 或
  │ → TriggerDelivery::InjectSummary → 仅注入 summary, 不运行模型
  │
  ▼
如 server ∈ inject_summary_servers:
  │ → TriggerDelivery::InjectSummary → 仅注入摘要文本
```

**跨模块边界**: pie-mcp (protocol decode) → coding-agent (notification→trigger mapping) → agent-core (dedup + audit) → agent-core (inject into main context)

---

### 4.5 DS4 Local Model Prefix Cache 优化链路

```
用户启动 DS4 服务器 (本地推理)
  │ ./ds4-server --ctx 100000 --kv-disk-dir /tmp/ds4-kv
  │
  ▼
pie 配置: models.json 声明 deepseek-v4-flash
  │ compat: { requiresReasoningContentOnAssistantMessages: true }
  │
  ▼
每轮对话:
  │
  ▼
OpenAIResponsesProvider::run() (openai_responses.rs)
  │
  ├─ resolve_compat(model):
  │   读取 compat.requiresReasoningContentOnAssistantMessages = true
  │   → replay_reasoning_content = true
  │
  ├─ convert_messages() (openai_responses.rs:662-735):
  │   对每个 AssistantMessage:
  │     - Thinking content block → {"type":"reasoning", "summary": [...]}
  │       作为独立 input item 插入到 assistant message **之前**
  │       (顺序是 load-bearing: DS4 的 merge 规则要求 reasoning 在 message 之前)
  │     - 其他 content block 正常转换
  │
  ├─ POST http://127.0.0.1:8000/v1/responses
  │
  ├─ send_with_retry (utils/retry.rs):
  │   HTTP 409 → retryable status (DS4 live continuation state 被驱逐)
  │   → pie 的请求始终包含完整 history → 直接重试即可重建 KV checkpoint
  │
  └─ consume_responses_sse:
      usage 处理:
        - 读取 input_tokens_details.cached_tokens → Usage.cache_read
        - 读取 input_tokens_details.cache_write_tokens (DS4 非标准字段) → Usage.cache_write
  │
  ▼
/cost 显示: cache_read >> input (证明前缀缓存持续命中)
```

**跨模块边界**: pie-ai (model compat → reasoning replay) → pie-ai (retry 409) → pie-ai (custom usage fields) → CLI (/cost display)

---

## 5. risks

### 5.1 架构 / 工程风险

| 风险 | 来源 | 严重度 | 说明 |
|------|------|--------|------|
| **单人维护者 (bus factor = 1)** | 06-roadmap | 高 | 大部分代码由 AI 编写，单个维护者承载所有设计决策和安全审计 |
| **bin crate 导致测试引用脆弱** | 03-coding | 高 | 集成测试通过 `#[path]` 直接引用 `bin` 源码，新模块依赖需在测试中复制模块树 |
| **!Send future 限制并发模型** | 03-coding | 中 | `TurnFut` 因 `parking_lot` guard 跨 `.await` 导致 `!Send`，无法 `tokio::spawn`，限制了并行 subagent 的可能 |
| **DS4 紧耦合风险** | 06-roadmap | 中 | 项目大量优化围绕 DS4，如果 DS4 演进方向改变或用户切换推理引擎，投入可能无法迁移 |
| **Bedrock SigV4 签名未实现** | 01-ai | 中 | 当前仅支持 Bearer token auth，AWS 标准 SigV4 路径未接入 |
| **CRC32 在 aws_eventstream.rs 中不验证** | 01-ai | 中 | Bedrock stream 的 CRC 校验被跳过，网络传输损坏可能导致 corrupt JSON |
| **SSE 解析器无独立单元测试** | 01-ai | 中 | `utils/sse.rs` 依赖各 provider 集成测试间接覆盖 |
| **MCP tools_list 不处理分页** | 05-mcp | 中 | `next_cursor` 字段存在但未处理，大工具集 MCP server 会丢失工具 |
| **JSONL session 无 compaction/vacuum 机制** | 02-agent | 低 | 持续 append 会无限增长，无自动压缩或清理 |
| **Web UI 与 TUI 代码重复** | 03-coding | 中 | axum SSE 端点和 ratatui event loop 是两套独立的 push/poll 模型，feature parity 维护成本高 |

### 5.2 安全 / 边界风险

| 风险 | 来源 | 严重度 | 说明 |
|------|------|--------|------|
| **Web Relay capability URL 泄露 = 完全控制** | 05-mcp | 高 | Watch + prompt + abort + approve 全权限，`/web-disconnect` 是唯一补救 |
| **Inbox 并发写入竞争** | 04-auto | 中 | Status 更新是全文件重写 (last-writer-wins)，高并发时 claim/dismiss 可能丢失 |
| **Dedup 窗口基于内存** | 04-auto | 中 | 崩溃重启后 dedup map 丢失，可能导致重复触发 (MAX_DEDUP_WINDOW = 24h) |
| **Loop state 文件损坏** | 04-auto | 中 | 外部工具修改/截断 state 文件后，agent 可能基于错误基线工作 |
| **MCP inject_and_run 反馈循环** | 06-roadmap | 中 | 无运行时硬性防护，仅依赖"信任声明"——显式设为 `inject_and_run` 的服务器才允许此路径 |
| **Trigger approve 流程未完成** | 06-roadmap | 中 | `promote_requires_approval = true` 路径 fail-closed to pending，`/triggers approve` 命令未实现 |
| **TOFU agent_key 不防中间人重 pin** | 05-mcp | 低 | Worker DO 重启后接受首个 agent_key，存在短暂攻击窗口 |
| **Remote approval = full power** | 05-mcp | 中 | 是 owner 的有意设计决策 (phone-first workflows)，但意味着泄露 URL = 完全控制权 |

### 5.3 未完成 / 技术债

| 项目 | 状态 | 来源 |
|------|------|------|
| Maker/checker 验证 (Loops Phase 3) | 未实现 | 04-auto, 06-roadmap |
| Session export 不 bundle sidecars (loop state, skills, MCP config) | v1 限制 | 06-roadmap |
| Multi-leaf session (#17) round-trip 确定性 | 待落地 | 06-roadmap |
| Bedrock streaming + Vertex full ADC chain | 流式待做 | 01-ai, 06-roadmap |
| WASM 扩展宿主 (Skills Part B) | 仅设计预留 | 06-roadmap |
| Legacy bad `firstKeptEntryId` 残留数据 | 已修复但历史文件可能损坏 | 06-roadmap |
| UsageCost 未在各 provider 中填充 | 结构体已定义但未使用 | 01-ai |
| Windows 支持 | 已明确取消 | 06-roadmap |

---

## 6. next_dag

以下 DAG 按 **聚焦、可并行、带验收产物** 原则设计。节点间用 `→` 表示依赖，`∥` 表示可并行。

```
Phase 2: 精读 DAG

┌─────────────────────────────────────────────────────────────────────────┐
│  Node A: Trigger 状态机追踪                                              │
│  ─────────────────────────                                              │
│  输入: agent_harness.rs (handle_trigger 段), trigger.rs, trigger_runtime.rs│
│  任务: 追踪 Trigger 从 handle_trigger 进入到 TriggerCompleted/Failed     │
│        发出的每一步状态转换 + 副作用 (audit write, sub-agent spawn,      │
│        promotion)                                                        │
│  产物: trigger-state-machine.md (时序图 + 状态副作用表)                   │
│  工时: 粗读 1h                                                           │
├─────────────────────────────────────────────────────────────────────────┤
│  Node B: Loop 标签提取 + Inbox 闭环                                       │
│  ────────────────────────────────                                        │
│  输入: triggers/cron.rs (cron_harness_listener, extract_tag_block),      │
│        inbox.rs                                                          │
│  任务: 分析 <loop-state>/<inbox> 标签的解析鲁棒性、截断边界、             │
│        inbox JSONL 并发安全性                                             │
│  产物: loop-inbox-internals.md (含并发 race condition 分析)               │
│  工时: 粗读 1h                                                           │
├─────────────────────────────────────────────────────────────────────────┤
│  Node C: JSONL Session 多 leaf 分支表示                                   │
│  ─────────────────────────────────────                                   │
│  输入: session/session.rs, session/jsonl_repo.rs, SessionTreeEntry enum  │
│  任务: 理解 parent_id DAG、active_leaf_id、branch_from 的映射关系，      │
│        验证 Compaction 的 first_kept_entry_id 在分支场景下的正确性        │
│  产物: session-branch-model.md (含分支场景的 JSONL 示例)                  │
│  工时: 粗读 1h                                                           │
├─────────────────────────────────────────────────────────────────────────┤
│  Node D: LSP Supervisor 集成深度                                          │
│  ────────────────────────────                                            │
│  输入: lsp_supervisor.rs, main.rs:781-786, after_tool_call hook 注入点   │
│  任务: 确定支持的语言清单、启动延迟处理、诊断注入时序、                    │
│        与 edit/write 工具的交互                                           │
│  产物: lsp-integration-report.md (含性能影响评估)                         │
│  工时: 粗读 1h                                                           │
├─────────────────────────────────────────────────────────────────────────┤
│  Node E: 跨 Provider Tool Call 解析一致性                                 │
│  ────────────────────────────────────────                                │
│  输入: anthropic.rs (input_json_delta), openai_responses.rs              │
│        (function_call_arguments.delta), google.rs (functionCall),        │
│        amazon_bedrock.rs (toolUse.input), utils/json_parse.rs            │
│  任务: 对比四种 tool call arguments 流式/非流式路径，                     │
│        验证 parse_partial_json 在所有路径的安全性                         │
│  产物: tool-call-parsing-matrix.md (含差异表 + fuzz 建议)                 │
│  工时: 粗读 1h                                                           │
├─────────────────────────────────────────────────────────────────────────┤
│  Node F: /goal Evaluator + OnTurnEndHook 循环                             │
│  ──────────────────────────────────────────                              │
│  输入: agent_harness.rs (run_evaluator), types.rs (OnTurnEndHook),       │
│        commands.rs (/goal 实现)                                           │
│  任务: 揭示 evaluator prompt 策略、transcript bounding、                  │
│        goal achieved 判断逻辑、false positive/negative 缓解措施           │
│  产物: goal-evaluator-internals.md                                       │
│  工时: 粗读 1h                                                           │
└─────────────────────────────────────────────────────────────────────────┘

依赖关系:
  A ∥ B ∥ C ∥ D ∥ E ∥ F  (全并行)

Phase 3: 综合 DAG (依赖 Phase 2)

┌─────────────────────────────────────────────────────────────────────────┐
│  Node G: 自动化安全审计 (依赖 A + B)                                      │
│  ─────────────────────────────────────                                   │
│  任务: 综合 trigger 状态机 + loop inbox 闭环，评估自动化链路的安全边界    │
│  (dedup 丢失、inbox 竞态、loop state 损坏、MCP feedback loop)            │
│  产物: automation-security-audit.md                                      │
├─────────────────────────────────────────────────────────────────────────┤
│  Node H: Session 完整性审查 (依赖 C)                                      │
│  ────────────────────────────────                                        │
│  任务: 基于 session 分支模型，审查 compaction/resume/export 的            │
│        数据完整性保证 (legacy bad firstKeptEntryId, export 缺 sidecar)   │
│  产物: session-integrity-review.md                                       │
├─────────────────────────────────────────────────────────────────────────┤
│  Node I: 跨 Provider 一致性测试计划 (依赖 E)                              │
│  ─────────────────────────────────────────                               │
│  任务: 基于 tool call 解析矩阵，制定跨 provider 一致性测试策略            │
│        (需增加哪些测试、mock HTTP server 设计)                            │
│  产物: provider-conformance-test-plan.md                                 │
└─────────────────────────────────────────────────────────────────────────┘

依赖关系:
  G ← A, B
  H ← C
  I ← E
  G ∥ H ∥ I  (全并行)
```

---

## 附录 A: 上游产物完整性检查

| 上游文件 | 存在 | 行数 | 是否完整 |
|----------|------|------|----------|
| 01-ai-provider-streaming.md | ✓ | 465 | 完整 |
| 02-agent-core-runtime.md | ✓ | 374 | 完整 |
| 03-coding-cli-tools.md | ✓ | 479 | 完整 |
| 04-automation-loops-triggers.md | ✓ | 524 | 完整 |
| 05-mcp-and-fefe-hub.md | ✓ | 455 | 完整 |
| 06-roadmap-docs-issues.md | ✓ | 376 | 完整 |

**无缺失**。所有六个上游 reader 产物均已读取并综合。

## 附录 B: 跨报告矛盾标注

1. **UsageCost 计算位置** — 01-ai 报告 `UsageCost` 结构体已定义但未在各 provider 中填充；02-agent 报告 CostTracker 监听 MessageEnd 并累加 Usage。两者一致：token 计数在 pie-ai 层完成，但 USD cost 乘以 `Model.cost` 的计算逻辑不在 pie-ai 各 provider 中。**需人工复核**: 搜索 `UsageCost` 和 `cost.cents` 的赋值点，确认 USD 计算是在 agent-core 的 CostTracker 中还是在 coding-agent 的某个 display 层。

2. **Tool 数量统计差异** — 03-coding 报告 "11 个核心工具"；04-auto 报告提及 NewCronJobTool、NewTriggerTool、SkillBuilderTool 等额外工具。两者一致：11 个是 `default_tools()` 返回的核心工具集，其余是高级/control-plane 工具。

3. **fefe hub 状态** — 05-mcp 报告 fefe hub 已 de-scope；06-roadmap 确认 Tier 8 跨 agent 服务已移除，Web relay 是唯一保留。一致。

4. **Compaction 与 Trigger Custom Entry** — 02-agent 报告提到 #19 修复了 `build_session_context` 跳过 trigger Custom 条目；06-roadmap 提到 "legacy bad firstKeptEntryId"。需人工复核：`build_session_context` 中对 `SessionTreeEntry::Custom` 的具体 skip 逻辑，确认 trigger/trigger_result/trigger_promotion 等子类型是否都被正确处理。
