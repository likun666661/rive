# MCP / fefe-hub Integration 粗读报告

> 仓库: `pie` | 阅读基线: `f1c35a3` | 深度档位: `architecture` | 日期: 2026-06-13

---

## 1. problem — MCP 与 fefe hub 要解决什么问题

### 1.1 MCP 客户端: 本地工具协议的可插拔扩展

MCP (Model Context Protocol, 协议版本 `2025-03-26`) 是生态中事实上的可插拔工具协议。pie 需要成为一个 **MCP 客户端**, 使任意 MCP 服务器(filesystem, git, GitHub, Slack, 自定义)无需修改代码即可接入 pie 的工具链。

MCP 在 pie 中解决三个层次的问题:

1. **工具暴露 (Tools):** 外部 MCP 服务器通过 stdio 子进程或 HTTP 暴露工具列表, pie 在启动时通过 `initialize → tools/list` 握手获取工具目录, 并以 `McpAgentTool` 适配器将 MCP 工具包装为 `AgentTool`, 使 LLM 可以像调用内置工具一样调用外部工具。

2. **通知/Trigger 管道 (Notifications):** MCP 服务器可以向客户端推送无 `id` 的 JSON-RPC 通知帧。pie 的 `McpClient` read pump 将这些帧路由到 `mpsc` 频道, 由 `McpNotificationHook` 消费并转换为 `Trigger` 信封, 驱动 pie 的 trigger 运行时(可触发子 agent 推理、直接注入聊天摘要、或触发一个 model turn)。

3. **协议传输抽象 (Transports):** 支持两种传输:
   - **stdio**: 通过子进程 stdin/stdout 进行 newline-delimited JSON 通信
   - **Streamable HTTP**: HTTP POST + SSE 长连接, 支持 Bearer token 认证和指数退避重连

### 1.2 fefe hub / Web Relay: 远程会话共享与 relay

fefe hub 的原始设计已被 **de-scope** (2026-06-10)。原计划是一个公共的跨 agent MCP 服务中心, 使用 Cloudflare Worker + D1 + Durable Object 实现用户注册、agent 间消息、MCP ingress、通知推送等功能。这一整套设计(`docs/issues/18-rfc-fefe-mcp-hub.md`, `docs/issues/19-fefe-client-onboard.md`)已被标记为 `Archived / removed from shipped product surface`。

**当前保留的唯一公共网络表面是 Web Relay** (`docs/issues/22-web-relay.md`):

- `/web-connect` 命令在运行中的 pie 会话中生成一个 capability URL (如 `https://pie.0xfefe.me/session/<160-bit随机token>`)
- 任何持有该 URL 的人可以在浏览器中观看对话 **并发送 prompt 给 agent**
- 不涉及账户、无持久化、无跨 agent 消息 — 纯粹的点对点 relay
- `/web-disconnect` 或退出 pie 使 URL 永久 404

### 1.3 endpoint 管理 (已废弃)

`docs/endpoints.md` 已被归档。原始 public webhook relay 依赖已删除的 fefe hub 服务。当前替代方案是使用本地 command hooks 或显式配置的 MCP 服务器。

---

## 2. why_hard — 为什么难

### 2.1 协议边界与跨语言通信

- **JSON-RPC 2.0 解析的健壮性**: 需要处理 `id` 存在(matched request/response)、`id` 不存在(notification)、`method` 缺失(非法帧)三种情况。`McpClient` 的 read pump (`client.rs:90-161`) 使用 `parking_lot::Mutex<HashMap<u64, oneshot::Sender>>` 实现请求-响应匹配, 需处理:
  - 超时/取消导致的 inflight entry 泄漏 (由 `InflightGuard` RAII 解决)
  - 取消后迟到的 server 响应 (由 `inflight.remove()` 后 pump 无法匹配, 静默丢弃)
  - 伪造/残缺的 JSON 帧 (malformed frames 被跳过)

- **stdio 子进程管理**: 需要管理子进程生命周期(`StdioTransport::spawn` + `Child::kill`), 处理 stdin/stdout 分离读写的异步安全(使用 `AsyncMutex<ChildStdin>` + `mpsc channel` for stdout), 以及 stderr 诊断信息采集。

### 2.2 Streaming / HTTP 的复杂性

- **SSE 解析 (`http.rs:408-466`):** 需要手写 SSE parser(`SseParser`), 处理分片(chunked)数据、跨 `\n\n` 边界的缓冲、`id:`/`data:` 字段提取、心跳/注释行(`:`)的忽略。不允许引入外部 SSE 库。
- **SSE 长连接 + POST 复用**: 同一 endpoint 上 POST 返回 JSON 或 SSE 响应, GET 返回长期 SSE 流。`enqueue_response_body()` 根据 `Content-Type` 区分处理, JSON 直接入队, SSE spawn 异步 reader。
- **重连与幂等性**: `spawn_sse_loop()` 使用指数退避 (`saturating_mul(2)`), 维护 `last-event-id` header 断点续传, 并在 agent socket 重连时使用 TOFU (trust on first use) 的 agent_key 校验。

### 2.3 认证与安全

- **HTTP MCP Bearer Token**: 通过 `AuthStore` 管理, token 在 `Debug` 输出中自动 redact, 错误诊断信息中不泄露 `token_keychain_ref`。
- **Web Relay 的 capability URL**: 160-bit 随机 token (base32) 既是认证也是授权。agent 侧通过独立的 `agent_key` (WEB Socket 握手 header) 进行 TOFU 验证, 防止 view-token 持有者冒充 agent。
- **Remote approval 的安全决策** (2026-06-11): capability URL 被升级为 **watch + prompt + abort + approve** 的全权限, 因为远程审批行为与本地 TUI 审批完全相同。这意味着泄露的 URL = 完全控制权。

### 2.4 Cloudflare Worker / D1 的特殊约束

- **Durable Object 的内存模型**: `SessionRelay` 是每个 view token 一个 DO 实例, 所有状态(agent WS, 最新 snapshot, viewers SSE 集合)在内存中, 无持久化。DO eviction 后状态丢失 (v1 接受)。
- **WebSocketPair**: Workers 环境不提供直接 Socket 创建, 必须使用 `new WebSocketPair()` 分别获取 client/server 端。
- **HTML 嵌入**: viewer HTML 通过 `include_str!` 编译时嵌入 worker bundle (`workers/shared/viewer_html.ts` → `viewer_html.generated.js`)。

### 2.5 Notification / Privacy

- **去重与命名空间 (`mcp_notification_hook.rs:11-43`):** 两层命名空间 — `mcp:{server_name}:` 前缀防止不同 server 的同名 notification 冲突; `custom:` 段防止用户自定义 key 与内置 key 碰撞。runtime 拥有去重窗口(adapter 不去重)。
- **Payload 可见性**: `payload_visibility = Local` — 完整 `params` blob 在持久化前丢弃, 只有 `_meta.pie_summary` 保留。未提供 summary 的自定义 notification 摘要退化为 method name。

### 2.6 Client Onboarding (已废弃)

原始 public cross-agent service 的 client onboarding (`docs/issues/19-fefe-client-onboard.md`) 包含账户注册、身份验证、信任文件、首次接触卡片等复杂流程, 已被完全移除。当前要求: 清洁 profile 不自动配置 public service endpoint; stale credentials 被 generic MCP loader 忽略。

---

## 3. design_approach — 设计思路

### 3.1 架构分层

```
┌──────────────────────────────────────────────────────────────┐
│                   pie-coding-agent (CLI)                      │
│  mcp_loader.rs  ──→  mcp_adapter.rs  ──→  triggers/          │
│  (配置读取)          (工具适配)            (通知管线)          │
├──────────────────────────────────────────────────────────────┤
│                    pie-mcp crate                              │
│  protocol.rs  ←→  client.rs  ←→  transport.rs                │
│  (类型/JSON-RPC)   (请求/响应)     (抽象 trait)               │
│  stdio.rs   http.rs                                           │
├──────────────────────────────────────────────────────────────┤
│                 pie-agent-core                                │
│  AgentTool, NotificationHook, Trigger, TriggerSink            │
└──────────────────────────────────────────────────────────────┘
```

### 3.2 两条主要流程

#### 流程 A: MCP Client → Server Tool Call

```
mcp.toml → mcp_loader.rs → connect_one()
  ├─ Stdio:  spawn cmd → StdioTransport → McpClient::new()
  └─ HTTP:   HttpMcpTransport::connect() → McpClient::new()
        │
        ▼
  initialize("pie-coding-agent")
        │ JSON-RPC: {"method":"initialize","id":1,...}
        ▼
  tools_list()
        │ JSON-RPC: {"method":"tools/list","id":2,...}
        ▼
  为每个 tool 创建 McpAgentTool(client, McpTool)
        │
        ▼
  当 LLM 调用工具时:
  McpAgentTool::execute()
        │ tools_call(name, arguments, cancel_token)
        ▼
  McpClient::tools_call()
        │ JSON-RPC: {"method":"tools/call","id":3,...}
        ▼
  McpToolCallResult { content: [ToolContent], is_error }
        │ adapter 转换为 UserContentBlock
        ▼
  AgentToolResult
```

#### 流程 B: fefe hub → endpoint/relay

```
pie TUI / --web 模式
        │ /web-connect
        ▼
  本地生成 view_token + agent_key (各 160-bit random)
        │ 发起 WebSocket to /relay/agent?token=<view_token>
        ▼
  Cloudflare Worker SessionRelay DO
        │ agent 发送 {"type":"hello","agent_key":"..."}
        │ TOFU: 首次连接 pin agent_key
        │ agent 持续推送 {"type":"snapshot","data":<WebSnapshot>}
        ▼
  Browser 访问 /session/<token>/
        │ 返回共享 viewer HTML
        │ GET /session/<token>/events (SSE)
        ▼
  实时 snapshot + status 推送
        │ 浏览器发送 POST /session/<token>/prompt {"text":"..."}
        ▼
  DO 通过 agent WS 转发到本地 pie → 注入 run queue
```

### 3.3 配置驱动

配置位于 `~/.pie/mcp.toml` (全局) 和 `<cwd>/.pie/mcp.toml` (项目, 覆盖同名条目):

```toml
[[server]]
name = "weather"
kind = "streamable_http"  # 或省略默认为 "stdio"
command = "python3"        # stdio 模式
args = ["./weather-server.py"]
endpoint = "https://mcp.example.com/mcp"  # HTTP 模式
auth = { kind = "bearer", token_keychain_ref = "mcp-example:default" }
request_timeout_ms = 30000
sse_idle_timeout_ms = 60000
body_cap_bytes = 1048576
reconnect = { initial_ms = 500, max_ms = 30000, max_attempts = 10 }
inject_summary = true      # 推送直接注入聊天(不触发 sub-agent)
inject_and_run = true       # 注入后额外运行一个 model turn
```

### 3.4 关键设计决策

- **失败非致命**: 某个 MCP 服务器启动失败仅记录诊断信息, 不影响其他服务器和 agent 运行。
- **Transport trait 抽象**: `Transport` 是 `Send + Sync + async_trait`, 所有 I/O 通过 line-oriented JSON 抽象, 以便后续添加 WebSocket 或 in-process transport。
- **单消费者通知**: `take_notifications()` 返回 `Some(rx)` 恰好一次; 后续调用返回 `None`。这保证了 `McpNotificationHook` 独占通知流。
- **Remote approval = first-class** (2026-06-11 owner decision): `POST /session/<token>/control-plane/resolve {approve: bool}` 的行为与本地 TUI 审批完全等价。

---

## 4. code_walkthrough — 关键 Rust/TypeScript 文件、类型、函数

### 4.1 `crates/mcp/` — 核心 MCP 客户端库

| 文件 | 关键类型/函数 | 说明 |
|------|-------------|------|
| `protocol.rs` | `PROTOCOL_VERSION = "2025-03-26"`, `McpTool`, `ToolContent`, `ToolsListResult`, `McpToolCallResult`, `RpcRequest`, `RpcError`, `RpcNotification`, `make_request()`, `make_notification()`, `CancelledNotificationParams` | JSON-RPC 2.0 协议类型和工厂函数。仅覆盖 initialize + tools 子集。 |
| `transport.rs` | `trait Transport: Send + Sync` | 双向 newline-delimited JSON 通道抽象: `send_line()`, `recv_line()`, `close()` |
| `client.rs` | `McpClient`, `McpServerNotification`, `InflightGuard` (RAII), `CANCEL_NOTIFY_SEND_BUDGET = 200ms` | MCP 客户端核心: 管理 inflight HashMap, spawn read pump task, 实现 initialize/tools_list/tools_call。`request()` 使用 `tokio::select! biased;` 优先返回已完成的响应。 |
| `stdio.rs` | `StdioTransport`, `StdioTransport::spawn()` | 子进程 stdin/stdout 通信, stderr tail (最多 200 行) 用于诊断。 |
| `http.rs` | `HttpMcpTransport`, `HttpMcpTransportOptions`, `HttpMcpAuth` (Debug redact), `ReconnectPolicy`, `SseParser`, `spawn_sse_loop()` | Streamable HTTP transport。POST 发请求, GET 接收 SSE, 指数退避重连。仅允许 `https` (除了 `127.0.0.1` 测试)。 |
| `errors.rs` | `McpError` | Transport / Protocol / ServerError / Timeout / NotInitialized / Cancelled / Other |
| `lib.rs` | re-export: `McpClient`, `Transport`, `StdioTransport`, `HttpMcpTransport`, `HttpMcpTransportOptions`, `HttpMcpAuth`, `ReconnectPolicy`, `McpTool`, `McpToolCallResult`, `InitializeResult`, `ServerInfo` | |

### 4.2 `crates/coding-agent/src/mcp_loader.rs` — 配置加载与连接编排

| 类型/函数 | 说明 |
|----------|------|
| `McpConfig` (TOML 反序列化) | `server: Vec<ServerConfig>` |
| `ServerConfig` | `name`, `kind: ServerKind` (Stdio/StreamableHttp), `command`, `args`, `endpoint`, `auth`, `request_timeout_ms`, `sse_idle_timeout_ms`, `body_cap_bytes`, `reconnect`, `inject_summary`, `inject_and_run` |
| `LoadedMcp` | 输出: `tools`, `diagnostics`, `client_count` (成功连接数), `notification_hooks`, `inject_summary_servers`, `inject_and_run_servers` |
| `load_all()` | 读取用户 + 项目配置, 合并(项目覆盖), 调用 `connect_all()`, 返回 `LoadedMcp` |
| `connect_one()` | 根据 `ServerKind` 选择 stdio/HTTP transport → `initialize()` → `take_notifications()` → `tools_list()` → 创建 `McpAgentTool` × N + `McpNotificationHook` × 1 |
| `resolve_http_auth()` | 从 `AuthStore` 解析 Bearer token; 诊断信息中不泄露 token_ref |

### 4.3 `crates/coding-agent/src/tools/mcp_adapter.rs` — 工具适配器

| 类型/函数 | 说明 |
|----------|------|
| `McpAgentTool` | 包装 `Arc<McpClient>` + `Tool` definition |
| `AgentTool::execute()` | `tools_call()` → `Cancelled` → `AgentToolError::Message("cancelled")`; Text → `UserContentBlock::Text`; Image → `UserContentBlock::Image`; Resource → JSON wrapped as `<resource>` text |
| `execution_mode()` | 返回 `Parallel` — MCP 工具调用可并行执行 |

### 4.4 `crates/coding-agent/src/triggers/mcp_notification_hook.rs` — 通知管线

| 类型/函数 | 说明 |
|----------|------|
| `McpNotificationHook` | 核心字段: `label` (如 `"mcp:weather"`), `server_name`, `rx: Mutex<Option<UnboundedReceiver<McpServerNotification>>>`, `status` |
| `NotificationHook::run()` | drain notifications → 映射为 `Trigger` 信封 → 推送到 `TriggerSink` |
| 去重映射规则 | `notifications/tools/listChanged` → `mcp:{server}:tools` (LatestReplaces); `notifications/resources/updated` → `mcp:{server}:resources:{hashed-uri}` (LatestReplaces); 自定义 → `mcp:{server}:custom:{key}` (Drop) |
| Privacy | `payload_visibility = Local` — raw params 丢弃; `_meta.pie_summary` 保留 → 摘要被 capped + redact; 不安全 URI 被 hash |

### 4.5 `workers/fefe-hub/` — Cloudflare Worker

| 文件 | 关键类型/函数 | 说明 |
|------|-------------|------|
| `src/index.ts` | `HubApp`, `createTestApp()`, 路由表, `REMOVED_PATHS` | Worker 入口。`/relay/agent` → DO relay; `/session/<token>` → DO relay; legacy paths → 410 |
| `src/relay.ts` | `RelayCore` (pure state machine), `SessionRelay` (DO wrapper), `parseSessionPath()`, `isValidToken()`, `AgentFrame` | Relay 核心逻辑: TOFU key pinning, snapshot broadcast, shutdown, viewer SSE, prompt forwarding, control-plane resolve |
| `wrangler.toml` | Durable Object bindings: `MAILBOX` (AgentMailbox, 已 tombstone), `SESSION_RELAY` (SessionRelay); D1 binding: `DB` (pie_fefe_hub) | |

#### RelayCore 状态机 (`relay.ts:39-90`)

```
AgentConnected → helloSeen = false
    ↓ receive {"type":"hello","agent_key":"k"}
  first ever? → agentKey = k (TOFU)
  subsequent? → agentKey === k ? accept : reject(bad_key)
    ↓ accept → helloSeen = true
    ↓ receive {"type":"snapshot","data":...}
  → broadcast(snapshot) + latestSnapshot = snapshot
    ↓ receive {"type":"shutdown"}
  → closed = true, latestSnapshot = null, shutdown
```

### 4.6 示例 MCP 服务器

#### `examples/mcp-weather-python/` — 真实数据源 (wttr.in 天气)

- `weather-server.py`: stdio JSON-RPC server, 定期 poll wttr.in, 通过 `notifications/pie/weather/observation` 推送含 `_meta.pie_dedup_key` 和 `_meta.pie_summary` 的通知
- `mcp.toml`: 配置 `inject_summary = true`, 使天气更新直接注入聊天

#### `examples/mcp-notify-python/` — 心跳演示

- `notify-server.py`: stdio JSON-RPC server, 每 10s 发送 `notifications/pie/demo/heartbeat`, 带 `_meta.pie_dedup_key` 和 `_meta.pie_summary`
- 演示了 server-push notification 的完整模式

---

## 5. flows — 关键流程简述

### 5.1 stdio MCP Call

1. `mcp_loader::connect_stdio()` 读取 `command` + `args` from `mcp.toml`
2. `StdioTransport::spawn()` 启动子进程, 建立 stdin/stdout/stderr 通道
3. `McpClient::new(transport)` 创建 client, spawn read pump task (消费 stdout, 路由 response/notification)
4. `client.initialize("pie-coding-agent")` → 发送 `{"jsonrpc":"2.0","id":1,"method":"initialize"}` → 接收 `InitializeResult`
5. `client.tools_list()` → 获取工具目录
6. `McpAgentTool::new(client, tool)` 为每个工具创建适配器
7. 当 LLM 调用工具: `tool.execute()` → `client.tools_call(name, arguments, cancel_token)` → `tokio::select!` (response vs cancel vs timeout) → 返回 `McpToolCallResult`

### 5.2 HTTP MCP Call (Streamable HTTP)

1. `mcp_loader::connect_streamable_http()` 验证配置 (endpoint 必须是 https 或 127.0.0.1)
2. `HttpMcpTransport::connect(opts)` 创建 `reqwest::Client`, 设置 auth header
3. `spawn_sse_loop()` 启动后台 SSE 长连接 (GET with `Accept: text/event-stream`), 带 `last-event-id` 续传和指数退避重连
4. POST 每个请求 (Content-Type: application/json, Accept: application/json, text/event-stream)
5. `enqueue_response_body()` 根据响应 Content-Type:
   - JSON: 直接入队 mpsc channel
   - SSE: spawn async reader 持续读帧
6. `McpClient` 对 transport 的使用与 stdio 完全相同 (同一种 line-oriented 抽象)

### 5.3 Notification Hook

1. `connect_one()` 中 `client.take_notifications()` 获取 `UnboundedReceiver<McpServerNotification>`
2. 创建 `McpNotificationHook::new(server_name, rx)`
3. Harness 通过 `register_notification_hook()` 注册
4. hook 的 `run()` 在独立 task 中 drain 通知:
   - 提取 `method`, `params`
   - 根据 method 生成 idempotency key (带 `mcp:{server}:` 前缀)
   - 提取 `_meta.pie_dedup_key`, `_meta.pie_summary`
   - 构建 `Trigger` 信封 → 推送到 `TriggerSink`
5. Runtime 的去重窗口处理重复通知; 匹配的 trigger 规则决定下一步动作

### 5.4 fefe Auth / Endpoint / Relay

**通知: 原始 fefe hub auth/endpoint 已移除。**

当前 Web Relay 流程:

1. **`/web-connect`**: pie 本地生成 `view_token` (160-bit random) + `agent_key` (160-bit random), 发起 WebSocket 到 `wss://pie.0xfefe.me/relay/agent?token=<view_token>`, header `x-pie-agent-key: <agent_key>`
2. **Agent Handshake**: `SessionRelay` DO 接收 WebSocket upgrade, 等待首个 agent frame `{"type":"hello","agent_key":"..."}` — 首次连接 pin key (TOFU), 重连需匹配
3. **Snapshot Push**: pie 在每个 turn 结束后推送 `{"type":"snapshot","data":<WebSnapshot>}`, DO 广播给所有 SSE viewers
4. **Viewer**: 浏览器访问 `/session/<token>/` → DO 返回 viewer HTML; `/session/<token>/events` → SSE stream (snapshots + relay_status)
5. **Remote Prompt**: 浏览器 POST `/session/<token>/prompt {"text":"..."}` → DO 通过 agent WS 转发到 pie → 注入串行 run queue
6. **Remote Approval**: POST `/session/<token>/control-plane/resolve {"approve": true|false}` → 行为与本地 TUI 审批等价
7. **Abort**: POST `/session/<token>/abort` → 转发到 agent
8. **`/web-disconnect`**: pie 发送 `{"type":"shutdown"}` → DO 关闭所有连接, token 永久 404

---

## 6. tests — 相关测试和测试意图

### 6.1 `crates/mcp/tests/client_fixture.rs` (7 个测试, 513 行)

使用 in-process `PipeTransport` (两个 `mpsc` channel 模拟 stdin/stdout) 测试:

| 测试 | 意图 |
|------|------|
| `handshake_list_and_call_round_trip` | Initialize → tools/list → tools/call 完整往返 |
| `tools_list_before_initialize_is_rejected` | 未 initialized 时调用 tools_list 应返回 `NotInitialized` |
| `server_push_notifications_reach_take_notifications_in_order` | 验证 read pump 正确路由 server-pushed notifications, malformed 帧被静默丢弃, `take_notifications` 单次消费 |
| `tools_call_cancel_during_wait_returns_cancelled_and_notifies_server` | Cancel 后返回 `McpError::Cancelled`, server 收到 `notifications/cancelled` 帧, `requestId` 匹配, 迟到响应被丢弃 |
| `tools_call_success_does_not_emit_cancelled_notification` | 正常成功调用不发 cancel 通知 (biased select! 正确) |
| `tools_call_without_cancel_token_keeps_pre_existing_behavior` | 无 cancel token 的调用行为不变 |
| `tools_call_request_timeout_still_returns_timeout_when_no_cancel` | 纯 timeout 路径正常工作 |

### 6.2 `crates/mcp/tests/http_fixture.rs` (3 个测试, 177 行)

使用 in-process `axum` HTTP server 测试:

| 测试 | 意图 |
|------|------|
| `streamable_http_posts_requests_and_receives_sse_notifications` | HTTP transport 完整流程: POST 发包, GET SSE 收通知, auth header 正确 |
| `streamable_http_error_body_is_redacted` | HTTP 4xx 响应体中不泄露内部 secret |
| `streamable_http_body_cap_rejects_oversize_response` | body_cap_bytes 拒绝超大响应 |

### 6.3 `crates/coding-agent/src/mcp_loader.rs` tests (5 个测试)

| 测试 | 意图 |
|------|------|
| `client_count_reflects_successful_connections_not_attempts` | 2 个失败 server → `client_count = 0` (code-review item #9) |
| `empty_configs_reports_zero` | 空配置 → 全零返回 |
| `streamable_http_config_deserializes_with_bearer_ref` | TOML 反序列化正确 |
| `streamable_http_rejects_command_args` | HTTP mode 下设置 command/args 应报错 |
| `streamable_http_auth_resolves_from_auth_store_without_debug_leak` | Auth debug 输出 redact token |
| `streamable_http_missing_auth_diagnostic_does_not_echo_token_ref` | 缺少 credential 时的错误信息不泄漏 token_ref |

### 6.4 `crates/coding-agent/src/tools/mcp_adapter.rs` tests (1 个测试)

| 测试 | 意图 |
|------|------|
| `execute_propagates_cancel_token_to_mcp_client` | Cancel token 通过 adapter → McpClient → transport 全链路传播; server 收到 `notifications/cancelled` |

### 6.5 `crates/mcp/src/http.rs` tests (2 个测试)

| 测试 | 意图 |
|------|------|
| `auth_debug_redacts_token` | Debug 输出中 token 被替换为 `<redacted>` |
| `sse_parser_ignores_heartbeat_and_extracts_data` | SSE parser 正确忽略心跳行, 提取 data + id |

### 6.6 `workers/fefe-hub/tests/relay.test.mjs` (9 个测试)

| 测试 | 意图 |
|------|------|
| `relay core pins the agent key on first hello and rejects mismatches` | TOFU key pinning 正确 |
| `relay core requires hello before any other frame` | 非 hello 帧被拒绝 |
| `relay core stores and broadcasts snapshots, then forgets on shutdown` | Snapshot 存储/广播/shutdown 清除 |
| `relay core rejects oversized and malformed frames` | 过尺寸/非 JSON 帧拒绝 |
| `session path parsing accepts hex tokens and rejects junk` | Token 合法性校验 |
| `router redirects bare session URLs to trailing slash` | 无尾斜杠重定向 |
| `router forwards session subpaths to the durable object for the token` | 路由正确转发 |
| `router rejects invalid tokens without touching durable objects` | 无效 token 不进入 DO |
| `control-plane resolve validates input and reports agent_offline without a socket` | Control-plane 输入验证 |
| `legacy hub paths still return 410 with the relay enabled` | Legacy 路径返回 410 |
| `health reports the relay as enabled when configured` | /health endpoint 正确报告 relay 状态 |

### 6.7 `workers/fefe-hub/tests/hub.test.mjs` (4 个测试)

| 测试 | 意图 |
|------|------|
| `health reports disabled tombstone metadata` | /health 返回正确 tombstone 元数据 |
| `old hub entrypoints return bounded 410 tombstone responses` | 所有 legacy 端点返回 410, 不泄漏 secret |
| `mcp durable object tombstone does not open an event stream` | AgentMailbox DO 返回 410 而非 SSE |
| `unknown routes remain bounded not found` | 未知路由返回 404 |

---

## 7. risks — 安全、运维、协议兼容风险

### 7.1 安全风险

| 风险 | 严重度 | 描述 |
|------|--------|------|
| **Capability URL 泄露** | 高 | Web Relay 的 view token URL 一旦泄露即完全控制 (watch + prompt + abort + approve)。`/web-disconnect` 是唯一补救手段。 |
| **TOFU 的 agent_key 不防中间人** | 中 | agent_key 首次连接时 pin; 如果攻击者在 agent 首次连接前 intercept view token 并抢先连接, 则成为合法 agent。但对于 v1 内网 relay 场景可接受。 |
| **SSE 响应中的 token 泄露** | 低 | HTTP error 响应体被显式 redacted (`http.rs:294-297`), auth token 在 debug 输出中 redact |
| **Remote approval = full power** | 中 | `control-plane/resolve` 与本地审批等价, 包括 approve/deny 权限控制提示。这是 owner 的有意设计决策 (phone-first workflows)。 |
| **Snapshot 中包含 transcript** | 中 | Worker 中仅保留 rendered snapshots, 不传输 provider credentials/auth.json/session files。但 transcript 内容本身通过 capability URL 暴露。 |

### 7.2 运维风险

| 风险 | 严重度 | 描述 |
|------|--------|------|
| **DO eviction 导致状态丢失** | 中 | 无持久化 — DO restart 后需要 agent 重连。观众可能短暂看到 "agent offline"。v1 接受该行为。 |
| **SSE 重连风暴** | 中 | 多个 viewer 同时 SSE 重连可能造成 Worker CPU 压力。当前每个 viewer 独立 SSE stream。 |
| **D1 依赖保留** | 低 | `wrangler.toml` 中仍有 D1 binding, 但 relay 不使用。如果 D1 集群有问题可能影响 Worker 部署但非运行时。 |
| **Snapshot > 1 MiB 被丢弃** | 低 | `MAX_AGENT_FRAME_BYTES = 1_200_000`, 超过则 drop + 本地警告。Transcript 过长时 viewer 可能看不到完整内容。 |
| **stdio 子进程僵死** | 中 | `StdioTransport::close()` 使用 `child.kill()`, 但超时/信号处理在不同 OS 上行为可能不一致。 |

### 7.3 协议兼容风险

| 风险 | 严重度 | 描述 |
|------|--------|------|
| **MCP spec 演进** | 中 | 当前硬编码 `PROTOCOL_VERSION = "2025-03-26"`。如果 MCP spec 发布新版本(如 `2025-09-01`), 需更新 protocol types 和 client 行为。 |
| **JSON-RPC 错误码不完整** | 低 | `McpError::ServerError` 只捕获了 `code` 和 `message`, 丢弃了 `data` 字段。某些 MCP server 可能在 `data` 中携带额外诊断信息。 |
| **SSE 解析兼容性** | 低 | 手写 `SseParser` 可能不兼容某些边缘 SSE 格式(如多行 data 的 join 逻辑、含 comment 的 BOM)。 |
| **Streamable HTTP 的 `resumption` 机制** | 中 | 仅通过 `last-event-id` header 实现基本续传; 不支持 MCP spec 中完整的 `resumptionToken` 机制。 |
| **Tools list pagination** | 低 | `ToolsListResult.next_cursor` 字段存在但 `tools_list()` 不处理分页 — 如果 server 返回超出一条响应的 tools, 会缺失后续页的工具。 |

---

## 8. next_questions — 下一轮精读问题

### 8.1 MCP 协议深度

1. `tools_list()` 不处理 `next_cursor` 分页 — 当前是否有 MCP server 会返回超过默认大小的工具列表? 添加分页支持的优先级如何?
2. MCP spec 的 `sampling` 和 `roots` capabilities 是否在 pie 的路线图中 (`ClientCapabilitiesSpec` 已预留字段)?
3. `notifications/cancelled` 的 server 端行为: server 收到 cancel 后 SHOULD stop work, 但 pie 目前超时 200ms 后不再等待确认。是否有 MCP server 忽略 cancel 的场景?

### 8.2 Transport 与 Resilience

4. `HttpMcpTransport` 仅允许 `https` (除 `127.0.0.1` 外)。本地开发使用 `http://127.0.0.1` 的测试 fixture 是否会带来生产环境误用 non-https endpoint 的风险?
5. SSE 重连的 `max_attempts = None` (无限重试) — 是否需要添加全局重连次数限制或告警机制?
6. `StdioTransport::spawn()` 对跨平台兼容性(Windows process management difference)是否已有测试?

### 8.3 Web Relay 深度

7. `SessionRelay` 在 Cloudflare 中如何扩容? 当前不限制 viewer 数量, 是否需要 viewer cap 或 rate limiting?
8. `control-plane/resolve` 的语义: 远程 approve/deny 后, 审批结果如何体现在本地 TUI 的 approval UI 中? 如果远程 approve 和本地 approve 同时发生会怎样?
9. `shutdown` 帧发送后, 如果 agent WS 已断连但 browser SSE 仍在, viewers 的收尾体验是什么?

### 8.4 Notification / Trigger 深度

10. `McpNotificationHook` 的去重完全依赖 runtime 的去重窗口 — 这意味着如果在去重窗口内收到两条相同 key 的 notification, 第二条会静默丢弃。这个行为是否符合所有 notification provider 的期望?
11. `_meta.pie_summary` 的 capping/redact 逻辑在代码中如何实现? 摘要长度限制是多少?
12. `inject_and_run` 模式的 model turn 是否与普通用户 prompt 共享同一个 serialized run slot? 如果有用户 prompt 正在运行, trigger 的 inject_and_run 如何处理?

### 8.5 废弃功能清理

13. `workers/fefe-hub/wrangler.toml` 中仍保留 D1 binding 和 `AgentMailbox` DO binding — 这些是否可以完全移除(只保留 `SESSION_RELAY`)?
14. `docs/endpoints.md` 和 `docs/issues/18-rfc-fefe-mcp-hub.md` 中的设计是否还有可复用的部分(比如 MCP ingress 的设计思想)?
