# 当前 Bot / OpenGateway / Session Abuse 回归现状分析

> 基线 `4dd1bf1`，对比起点 `05ca5a3`
> 深度档位: `maintainer`

## scope

| 文件 | 路径 | 行数 |
|------|------|------|
| `open-gateway.ts` | `apps/desktop/src/main/open-gateway.ts` | 1479 |
| `main.ts` (bot handler) | `apps/desktop/src/main/main.ts` | ~800 (bot 段落) |
| `project-context.ts` | `apps/desktop/src/main/project-context.ts` | 72 |
| `bot-events.ts` | `packages/core/src/bot-events.ts` | 275 |
| `bot-platform-hints.ts` | `packages/core/src/bot-platform-hints.ts` | 127 |
| `session-manager.ts` | `packages/runtime/src/session-manager.ts` | 819 |
| `chat-readiness.ts` | `apps/desktop/src/main/chat-readiness.ts` | 247 |
| `bot-incoming-idempotency-contract.test.ts` | `apps/desktop/src/main/__tests__/bot-incoming-idempotency-contract.test.ts` | 131 |
| `open-gateway-sse-abuse-contract.test.ts` | `apps/desktop/src/main/__tests__/open-gateway-sse-abuse-contract.test.ts` | 50 |
| `open-gateway.test.ts` | `apps/desktop/src/main/__tests__/open-gateway.test.ts` | 1130 |

## problem

Bot 入口与 OpenGateway 入口通过 **同一个 `SessionManager`** 创建和复用 session，共享 `ai-sdk` backend、同一套模型连接 (`connectionStore`)、同一组 API key (`credentialStore`)，但两者的 abuse perimeter 完全不同：

1. **Bot 入口**来自公网 Telegram / 飞书 / Discord 等平台，攻击面是任何人都可以向 token 配置的 bot 发消息。如果没有正确的 idempotency / rate limit / session cap，一条被重放的 platform redelivery 就能产生重复 agent 回复，一轮 flood 就能耗尽用户配额。
2. **OpenGateway 入口**是本机 `127.0.0.1` 的 HTTP 服务，但通过 SSE event stream (`/v1/sessions/{id}/events`) 暴露了 session 后备的实时事件。如果 SSE 连接数不受控，多个 event stream 同时推送到一个 session，可能造成：主进程资源耗尽、renderer IPC 被 `sessions:event:{id}` 消息打爆、gateway 的 `recentEvents` buffer 被无效事件撑满。
3. **Session 复用**是两条攻击链的交汇点：bot 通过 `botConversationKey` 绑定 session，gateway 通过 `/v1/sessions/{id}/messages` 发消息送同一个 session。如果 permission mode 被意外切换（permission_mode_not_explore），bot 可能绕过 explore-only 约束执行 side effect。

## current_design

### 外部入口

#### Bot 入口 (`main.ts`)

1. `BotRegistry.onIncomingMessage` → `handleBotIncomingMessage()` (`main.ts:2851`)
2. 第一层：**Idempotency** — `rememberBotSourceEvent()` (`main.ts:2882`) 基于 `${platform}:${chatId}:${sourceMessageId}` 去重。TTL=60min，hard cap=1000。在 `message.text.trim()` 之前执行。
3. 第二层：**Conversation serialization** — 同一 `botConversationKey` 的消息排队串行处理 (`botConversationQueues` Map)，防止同一对话的并发 turn。
4. 第三层：**Rate limit** — `consumeBotConversationToken()` (`main.ts:2904`) 基于 token bucket（burst=8，refill=1 per 5s），TTL=60min，hard cap=1000 buckets。
5. 第四层：**Session cap** — `BOT_CONVERSATION_SESSION_LIMIT = 500`，超过后拒绝创建新 session。
6. 第五层：**Permission enforcement** — 新 bot session 创建时强制 `permissionMode: 'explore'`；已有 session 通过 `ensureBotSessionExploreMode()` (`main.ts:3119`) 在 send 前校验。
7. 第六层：**Non-text ack** — 照片/语音/贴纸消息返回 kind-aware 提示，不触发 send。

#### Gateway 入口 (`open-gateway.ts`)

1. HTTP server bind `127.0.0.1`，**Bearer token** 鉴权 (`isAuthorized()` → `Authorization: Bearer <token>`)，`/health` 无鉴权。
2. Body size limit `OPEN_GATEWAY_MAX_BODY_BYTES = 16KB`，text max `8_000` chars。
3. 每个请求带 `X-Maka-Request-Id` header，记录到 `recentRequests`（最多 50 条），不含 headers / query / payloads。
4. Send message 路由 → `deps.sendMessage()` → `runtime.sendMessage()`，通过 `ensureSessionCanSend()` 校验连接就绪。
5. Send 在 gateway 内部不做 bot 级的 permission mode/rate limit 检查（bot 的控制仅在 `main.ts:processBotIncomingMessage` 中实现）。

### Idempotency

- `botSourceEventKey()` in `bot-events.ts:105` 生成 `${platform}:${chatId}:${sourceMessageId}`。
- `rememberBotSourceEvent()` 在 handler 入口第一行调用，在 `text.trim()` 之前，保证 non-text ack 也是幂等的。
- Bot bridge 的 redelivery 会被直接 drop，不产生 session 创建 / send / 通知。

### SSE 防护

`open-gateway.ts` 的 SSE event stream 有如下硬限制：

| 常量 | 值 | 作用 |
|------|-----|------|
| `OPEN_GATEWAY_EVENT_STREAM_TOTAL_LIMIT` | 10 | 全局 SSE 连接上限 |
| `OPEN_GATEWAY_EVENT_STREAM_PER_SESSION_LIMIT` | 3 | 单 session SSE 上限 |
| `OPEN_GATEWAY_EVENT_HEARTBEAT_MS` | 15s | 心跳间隔 |
| `OPEN_GATEWAY_EVENT_IDLE_TIMEOUT_MS` | 5min | 无真实事件后的空闲超时 |
| `OPEN_GATEWAY_EVENT_REPLAY_LIMIT` | 100 | 重放 buffer 大小 |
| `OPEN_GATEWAY_EVENT_RECENT_LIMIT` | 50 | recent events 端点返回上限 |

- 429 拒绝在 `text/event-stream` header 提交之前执行，防止连接级 DDOS。
- 心跳 (`: heartbeat`) 不重置 idle timer，只有 `client.write(chunk)` 调用 `resetIdleTimer()`。
- Token 轮换时 `closeEventClients()` 清除所有 SSE 连接。

### Session Reuse

- Bot 通过 `botConversationSessions: Map<conversationKey, sessionId>` 维护 `platform:chatId → sessionId` 绑定。同一对话多次消息复用同一个 session。
- Gateway 通过 `POST /v1/sessions/{sessionId}/messages` 向任意 session 发送消息。
- 两条路径的 **cross-contamination 风险**：如果 gateway 向一个 bot-bound session 发送了消息，且该 session 的 `permissionMode` 被 gateway 侧修改为非 explore，bot 下一次 message 会触发 `ensureBotSessionExploreMode()` 将其切回 explore — 但如果 bot updateSession 抛错，turn 被拒绝。
- `ensureSessionCanSend()` 确保 bot session 没有 `connectionLocked: false` 的 stale FakeBackend 风险。

### Permission Mode

- Bot 创建 session 时 `permissionMode: 'explore'`（只读/web-read）。
- `ensureBotSessionExploreMode()` 在每次 bot 发送前检查：如果不是 `explore`，尝试 `runtime.updateSession(permissionMode: 'explore')`；如果失败（如正在 running），返回 false 并发送 transient notice。
- `SessionManager.setPermissionMode()` 在 `activeStreams > 0` 或 `status === 'waiting_for_user'` 时拒绝切换，但不区分调用者（renderer 或 bot handler）。

## source_evidence

| 证据项 | 文件:行号 | 限制/边界 | 测试覆盖 |
|--------|----------|----------|---------|
| Bot 消息去重 | `main.ts:2882-2895` | `${platform}:${chatId}:${sourceMessageId}`; TTL=60min; cap=1000 | `bot-incoming-idempotency-contract.test.ts` |
| Bot rate limit | `main.ts:2904-2928` | bucket burst=8, refill=1/5s, TTL=60min, cap=1000 | `bot-incoming-idempotency-contract.test.ts` (rate-limit/session-cap) |
| Bot session cap | `main.ts:2998` | `BOT_CONVERSATION_SESSION_LIMIT = 500` | `bot-incoming-idempotency-contract.test.ts` |
| Bot permission enforcement | `main.ts:3022,3119-3138` | 强制 explore，拒绝非 explore | `bot-incoming-idempotency-contract.test.ts` (forces explore before send) |
| Gateway auth | `open-gateway.ts:178-182,473-476` | Bearer token via `Authorization` header | `open-gateway.test.ts:67-171` |
| Gateway body limit | `open-gateway.ts:686,1442-1466` | 16KB body; text `≤8000` chars | `open-gateway.test.ts:937-965` |
| Gateway SSE total limit | `open-gateway.ts:691,514-519` | 10 global | `open-gateway-sse-abuse-contract.test.ts` + `open-gateway.test.ts:565-601` |
| Gateway SSE per-session limit | `open-gateway.ts:692,514-519` | 3 per session | `open-gateway-sse-abuse-contract.test.ts` + `open-gateway.test.ts:565-601` |
| Gateway SSE idle timeout | `open-gateway.ts:690,531-548` | 5min idle; heartbeat doesn't reset | `open-gateway-sse-abuse-contract.test.ts:31-49` |
| Gateway token rotation closes SSE | `open-gateway.ts:86-88,574-578` | 所有 event clients 关闭 | `open-gateway.test.ts:905-935` |
| Gateway CORS | `open-gateway.ts:159-162` | `Access-Control-Allow-Origin: http://127.0.0.1` | 无显式单元测试 |
| Gateway error redaction | `open-gateway.ts:103-106` | 500 → `internal_error` + Chinese message + requestId | `open-gateway.test.ts:29-46` |
| Gateway request tracking | `open-gateway.ts:482-506` | 最近 50 请求，不含 headers/query/payloads | `open-gateway.test.ts:215-246` |
| Gateway replay buffer | `open-gateway.ts:591-598` | 100 events per session in-memory | `open-gateway.test.ts:623-681` |
| Gateway redaction | `open-gateway.ts` 多处 | `redactSecrets()` 在 id/path/message 上 | `open-gateway.test.ts` 多处 |
| SessionManager active session | `session-manager.ts:129,535-561` | `active: Map<string, ActiveSession>`; `activeStreams` 记数 | 无专门 abuse 测试 |
| SessionManager connection lock | `session-manager.ts:331-333` | `connectionLocked` 在首次 send 后置 true | 无专门 abuse 测试 |
| SessionManager dispose | `session-manager.ts:563-572` | archive/remove/backend config change 时 dispose | 间接覆盖 |

## abuse_flow

### 1. 重复 Incoming 攻击链

```
Bot Platform (Telegram)
  │ redelivery of sourceMessageId=X
  ▼
BotRegistry.onIncomingMessage
  ▼
handleBotIncomingMessage(message)         [main.ts:2851]
  │ rememberBotSourceEvent(message) → true → return  ← IDEMPOTENCY GATE
  │ [never reaches session lookup, send, or ack]
  ▼
Dropped silently
```

**防护状态**: 完全覆盖。TTL=60min 确保 bridge reconnect 的重放能去重；hard cap=1000 防止内存膨胀。

**残余风险**: TTL 超过 60min 的 redelivery 不会被去重（如 Telegram 重启后补推老消息）。当前设计认为 60min 后的重复 agent 回复是可接受的。

### 2. SSE Connection Storm 攻击链

```
Attacker (localhost, has bearer token)
  │
  ├─ for i in 0..100:
  │     GET /v1/sessions/s1/events
  │     Authorization: Bearer <token>
  │
  ▼
OpenGatewayService.openSessionEventStream()  [open-gateway.ts:508]
  │ counter >= 10 (total) → 429               ← TOTAL LIMIT GATE
  │ per-session >= 3 → 429                    ← PER-SESSION GATE
  │ [before text/event-stream header written]
  ▼
Rejected with {"ok":false,"error":"too_many_event_streams"}
```

**防护状态**: 两层硬限制都在 SSE header 提交前执行，429 不会建立 stream。

**残余风险**:
- 3 个合法 SSE + 7 个其他 session 的 SSE 仍然可以同时存在。每个 SSE 的 heartbeat 每 15s 写一次 `res`，但心跳不重置 idle timer。如果 10 个 SSE 全部连接到活跃 session，gateway 的 `publishSessionEvent()` 会向每个 client 写数据——`O(total_clients)` 的同步风扇开。
- `publishSessionEvent()` 用 `for...of` 同步遍历 `clients` Set (`open-gateway.ts:58-60`)，不是异步限流。10 个 client × 高频 text_delta event → 主进程 event loop 阻塞。
- `recentEvents: Map<string, SessionEvent[]>` 没有 per-key 写入保护（`open-gateway.ts:591-598`），大量 publish 可能撑满 buffer（hard cap 100）。

### 3. Gateway-to-Session Cross-Contamination 链路

```
Gateway Client (has token)
  │ POST /v1/sessions/{bot-session-id}/messages
  │ body: {"text": "rm -rf /"}
  ▼
OpenGatewayService.handle() [open-gateway.ts:388-404]
  │ deps.sendMessage(sessionId, {text})
  │   → ensureSessionCanSend(sessionId)  ← 仅校验连接就绪
  │   → runtime.sendMessage(sessionId, ...)
  │       [permissionMode 未校验 — 网关侧无 bot guard]
  ▼
AiSdkBackend.send() → tools execute → side effects
```

**防护状态**: 跨层防护不全。Gateway 的 `sendMessage` 不做任何 permission mode 检查。如果 bot session 的 permission mode 因为之前的内存状态或其他原因不是 `explore`，gateway 可以绕过 bot 的保护直接触发 tool 执行。

**但实际上**：
- Gateway 依赖的 `ensureSessionCanSend()` 只看连接就绪，不看 permission。
- Bot handler 的 `ensureBotSessionExploreMode()` 只在 `processBotIncomingMessage()` 中执行，不在 gateway 路径上。
- 然而 gateway 传入的 sessionId 是外部提供的（URL path），外部调用者需要事先知道 bot session ID。这不是一个 trivial 的 bypass，但仍然是 architectural gap。

## tests

### 已有测试覆盖

| 测试 | 文件 | 覆盖内容 |
|------|------|---------|
| Bot incoming idempotency contract | `bot-incoming-idempotency-contract.test.ts` | 去重调用顺序、TTL 过期清理、rate limit + session cap 执行顺序、permission mode 强制 |
| SSE abuse hardening | `open-gateway-sse-abuse-contract.test.ts` | 429 在 header 前提交、心跳不重置 idle timer、cleanup 清 timer |
| OpenGateway 功能测试 | `open-gateway.test.ts` | auth、health、capabilities、session CRUD、SSE stream、replay、cursor miss、token rotation、state payloads redaction、incidents、body validation、pagination、recent requests |

### 测试缺口

| 缺口 | 严重度 | 说明 |
|------|--------|------|
| Gateway sendMessage 不校验 permission mode | P1 | 网关可以向 bot session 发消息而不触发 explore 强制 — 无测试覆盖该 bypass 路径 |
| SSE 高频 publish 下主进程阻塞 | P2 | 没有对 `publishSessionEvent()` 在 10 client + 100 event/s 场景下的性能/阻塞测试 |
| `botConversationQueues` 串行化饥饿 | P2 | 如果单个 conversation 持续高频发消息，其他 conversation 被阻塞在队列后面 — 无测试 |
| Gateway CORS 配置安全性 | P2 | `Access-Control-Allow-Origin: http://127.0.0.1` 仅允许无端口 localhost，但没有测试验证 |
| `recentEvents` 写入无锁保护 | P3 | 高并发 publish 可能导致 buffer 内容竞争 — 无测试 |
| SessionManager `active` map 未设硬上限 | P3 | `active` map 按 session 数增长，没有清理策略 — 无测试 |
| Bot `connectionLocked` 的跨入口一致性 | P2 | Bot 和 gateway 共享 session，connectionLocked 在 sendMessage 中自动设置但 gateway 进入前没有 barrier |

## risks

### P0 — 可导致 user-visible 攻击

无当前已知 P0 风险。Idempotency、rate limit、session cap 三条主要 bot abuse 防线均已实现并有 contract 测试。

### P1 — 可导致绕过或意外 side effect

1. **Gateway 路径缺少 permission mode guard** (`open-gateway.ts:388-404`)
   - Gateway `sendMessage` 不验证 session 的 permission mode。
   - 如果 bot session 的 permission mode 因为任何原因不是 `explore`（例如用户在桌面端切到了 `ask`），gateway 发出的消息会以非 explore 模式执行。
   - **缓解**：外部调用者需要知道 bot session 的 UUID（不可猜测），且 gateway 只在 `127.0.0.1` 监听、需要 bearer token。但这个 architectural gap 应该在 gateway 层直接防御。

2. **Bot session cap 500 在并发场景下可能触达**
   - 500 个活跃 bot 会话在理论上是足够大的缓冲区，但如果 bot 在不清理绑定的情况下累积（用户不主动重置），500 会慢慢填满。
   - **缓解**：reset 命令清除绑定。无自动过期机制（botConversationSessions 没有 TTL）。

### P2 — 运维/资源风险

1. **SSE fan-out 阻塞主进程**
   - `publishSessionEvent()` 是同步 for-loop（`open-gateway.ts:58-60`），在高频事件 + 多 client 时每个 `client.write(chunk)` 在同一个 tick 中执行。
   - 如果 SSE client 的 TCP buffer 满了（客户端不消费），`res.write()` 可能 block event loop。
   - **缓解**：当前 max 10 global + 3 per-session，还算小。但无 backpressure 机制。

2. **`botConversationQueues` 串行化模型无超时**
   - 同一 `botConversationKey` 的消息在队列上串行处理（`main.ts:2872-2879`）。
   - 如果某个 turn 处理耗时过长（LLM 调用 10 分钟），该 conversation 的后续消息全部排队等待。
   - **缓解**：无。队列的 `.catch(() => {})` 只隔离 error 不加速。

3. **`recentEvents` buffer OOM**
   - `recentEvents: Map<string, SessionEvent[]>` 在系统有大量 session 活跃时每 session 最多存 100 events。
   - 1000 session × 100 events × 每个 event 的 JSON payload → 约 10-50MB 内存。
   - **缓解**：当前 500 bot session cap 限制了 session 数量上限。但没有对总 buffer 大小做上限。

4. **Gateway token 泄露窗口**
   - Gateway token 通过 `settingsStore.get()` 读取，token 以明文存储在 settings 文件中。
   - 任何能读 `~/.maka/workspaces/default/settings.json` 的进程都能拿到 gateway token。
   - **缓解**：Gateway 只监听 `127.0.0.1`，外部无法直接连接。但本地恶意进程（如 npm postinstall script）可以读 settings 文件 + 连接 `127.0.0.1:3939` → 获取所有 session 信息 + 发消息。

### P3 — 边界/edge case 风险

1. **SSL/TLS 缺失**
   - Gateway 是纯 HTTP，没有任何加密。如果用户修改 host 为非 localhost，所有请求明文传输。
   - **缓解**：`host` 默认 `127.0.0.1`，sync 时如果 host/port 改变会重建 server，但没有任何校验防止用户改 host。

2. **botConversationSessions 无 TTL 自动清理**
   - 如果 bot 被从平台移除或 token 失效，但在 Maka 侧仍然绑定 500 个 session，清理需要用户手动 reset 或 bot restart。

## next_actions

1. **P1: 为 gateway sendMessage 添加 permission mode guard**
   - 在 `open-gateway.ts` 的 `POST /v1/sessions/{id}/messages` handler 中，检查目标 session 的 permission mode，对齐 bot handler 的 explore-only 策略（至少对带有 `bot` label 的 session）。
   - 添加对应的 contract 测试。

2. **P2: 添加 SSE 高频 publish backpressure**
   - 在 `publishSessionEvent()` 中，对高频率 event 做合并/batch（如同类型 text_delta 在一个 tick 内合并）或对 client write 做 `setImmediate` 分片。
   - 添加 SSE storm 性能测试（模拟 10 client × 500 events/s）。

3. **P2: 添加 bot conversation queue timeout**
   - 在 `handleBotIncomingMessage()` 的队列逻辑中添加超时（如 5 分钟），超时后直接发送 fallback 错误通知。
   - 测试验证超时后 conversation 恢复。

4. **P2: 添加 `recentEvents` total size cap**
   - 在 `recordRecentEvent()` 中添加全局 buffer 大小上限（按总 events 数量或内存大小）。
   - 当超过上限时，从 oldest session 开始逐出。

5. **P3: gateway token 文件读取加固**
   - 在 gateway `sync()` 时对 token 做内存 crypto hash，不存明文。
   - 或者至少写入前对 settings.json 的 token 字段做加密。

6. **P3: 添加 bot conversation session TTL 自动过期**
   - 给 `botConversationSessions` Map value 添加 `createdAt` / `lastUsedAt` timestamp。
   - 定时器或惰性清理过期绑定（如 7 天未使用自动解绑）。

7. **测试补强**
   - 添加 `open-gateway.test.ts` 中对 gateway send → bot session permission bypass 的测试。
   - 添加 `session-manager.ts` 对 active session map 无上限的 contract 测试。
   - 添加对 gateway CORS header 精确值的测试。
