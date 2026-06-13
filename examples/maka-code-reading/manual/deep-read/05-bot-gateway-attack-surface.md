# Bot Bridge / OpenGateway Attack Surface

## scope

- **Bot Bridge 层**：`packages/runtime/src/bots/` 下的 Telegram、Discord、DingTalk、QQ、WeChat（iLink + local bridge）六个平台适配器，以及 `BotRegistry`、`base-adapter`、`proxied-fetch` 基础设施。
- **OpenGateway 层**：`apps/desktop/src/main/open-gateway.ts` 中暴露的本地 HTTP API（Bearer Token 鉴权），含 SSE 事件流、session state、incident、request tracking、thread search 等端点。
- **集成调度**：`apps/desktop/src/main/main.ts` 中 BotRegistry → SessionManager 的入站消息处理链路，以及 OpenGateway 在 `streamEvents` / `applySettingsRuntimeEffects` 中的挂载点。
- **核心契约**：`packages/core/src/bot-events.ts`（消息模型、idempotency key、allowlist、plaintext 命令）和 `packages/core/src/bot-platform-hints.ts`（平台交付约束）。

## problem

Bot Bridge 与 OpenGateway 构成了 Maka 的两条外部边界：
1. **IM 入站**：不受控的第三方平台（Telegram/Discord/DingTalk/QQ/WeChat）可以向本地进程推送消息，自动创建 Session 并驱动 LLM 推理。
2. **本地 HTTP API**：OpenGateway 将 Session 状态、事件、消息内容暴露给 `127.0.0.1` 上的调用方，鉴权仅依赖于一个静态 Bearer Token。

两条边界在 `handleBotIncomingMessage` → `processBotIncomingMessage` → `runtime.sendMessage` 和 `openGateway.sync` → `handle` → `eventClients` 两条路径上交汇。本次精读的目标是识别每条路径上真实可被利用的攻击向量，区分"当前代码就存在的可利用风险"与"需要特定部署环境配合才能触发"的缺陷。

## source_evidence

### Bot Bridge 源码清单

| 文件 | 行数 | 职责 |
|------|------|------|
| `packages/runtime/src/bots/types.ts` | 85 | `BotBridge`、`BotStatus`、`SendCapable`、`BotTestResult` 接口 |
| `packages/runtime/src/bots/base-adapter.ts` | 76 | `BaseBotAdapter` 抽象基类、`botReadinessFromSettings` |
| `packages/runtime/src/bots/bot-registry.ts` | 198 | `BotRegistry`：桥生命周期管理、设置同步、入站/出站路由 |
| `packages/runtime/src/bots/simple-bridge.ts` | 530 | `SimpleBotBridge`：Telegram 长轮询 + Feishu credential probe |
| `packages/runtime/src/bots/discord-bridge.ts` | 598 | `DiscordBotBridge`：Gateway WebSocket、identify、heartbeat、dispatch |
| `packages/runtime/src/bots/dingtalk-bridge.ts` | 527 | `DingTalkBotBridge`：Stream WebSocket、access_token 缓存 |
| `packages/runtime/src/bots/qq-bridge.ts` | 714 | `QQBotBridge`：Gateway WebSocket、多 channel 类型 dispatch |
| `packages/runtime/src/bots/wechat-bridge.ts` | 632 | `WechatBridge`：本地 bridge SSE + iLink 长轮询 |
| `packages/runtime/src/bots/proxied-fetch.ts` | 70 | 通过系统代理发出的 undici fetch |
| `packages/runtime/src/bots/bot-test.ts` | 271 | 平台凭据探测（credentials smoke test） |

### OpenGateway 源码清单

| 文件 | 行数 | 职责 |
|------|------|------|
| `apps/desktop/src/main/open-gateway.ts` | 1451 | `OpenGatewayService`：HTTP server、SSE、state/sessions/events/incidents/requests API |
| `apps/desktop/src/main/main.ts:448–471` | 24 | `openGateway` DI 构造 + `sendMessage` / `searchThread` 委托 |
| `apps/desktop/src/main/main.ts:2724–2728` | 5 | `applySettingsRuntimeEffects` 中 `openGateway.sync` 调用 |
| `apps/desktop/src/main/main.ts:2744,2768` | 2 | `streamEvents` 中 `publishSessionEvent` 调用 |

### Bot 集成交互源码

| 文件 | 行数 | 职责 |
|------|------|------|
| `apps/desktop/src/main/main.ts:894–897` | 4 | `botConversationSessions` / `botConversationQueues` / `botRecentSourceEventKeys` |
| `apps/desktop/src/main/main.ts:2777–2806` | 30 | `handleBotIncomingMessage`：入站分派、attachmentKind ack |
| `apps/desktop/src/main/main.ts:2808–2819` | 12 | `rememberBotSourceEvent`：sourceMessageId 幂等去重 |
| `apps/desktop/src/main/main.ts:2821–2961` | 141 | `processBotIncomingMessage`：help/reset 命令、session 绑定、send+collect |
| `apps/desktop/src/main/main.ts:2963–3013` | 51 | `collectBotReply`：事件收集、permission_request 拦截 |

### 核心合约

| 文件 | 行数 | 职责 |
|------|------|------|
| `packages/core/src/bot-events.ts` | 275 | `BotMessageEvent`、`botConversationKey`、`botSourceEventKey`、allowlist、plaintext 命令 |
| `packages/core/src/bot-platform-hints.ts` | 127 | 平台交付提示、system prompt fragment |

## flow_analysis

### Flow 1: IM 入站 → Bridge → SessionManager → LLM 推理 → 出站回复

```
[Telegram/Discord/QQ/DingTalk/WeChat]
    │ 入站消息（WebSocket/长轮询/SSE）
    ▼
[Bridge.pollTelegram / handlePayload / streamLiveMessages]
    │ emitIncomingMessage(BotMessageEvent)
    ▼
[BotRegistry.onIncomingMessage]
    │ main.ts handleBotIncomingMessage
    ▼
[rememberBotSourceEvent]  ← idempotency key: platform:chatId:sourceMessageId
    │ 重复？→ return
    │ 新消息 → 进 per-conversation 队列
    ▼
[processBotIncomingMessage]
    ├─ help command? → 发送 ephemeral help reply (TTL 5min)
    ├─ reset command? (DM only) → 解绑 session，发送 ack
    ├─ permission_request occurred? → 返回审批提示
    ├─ 首次消息 → runtime.createSession(permissionMode: 'explore')
    └─ 已有 session → ensureSessionCanSend → sendMessage
    ▼
[SessionManager.sendMessage] → [AiSdkBackend]
    │ collectBotReply 收集 text_complete
    ▼
[botRegistry.sendMessage] → [平台 API send]
    ├─ 成功 → reply text delivered
    └─ 失败 → 5min TTL delivery-failed notice
```

**关键观察**：
- Bot 创建的 session 强制使用 `permissionMode: 'explore'`（`main.ts:2876`）——只允许 read + web_read，不可写文件或执行命令。这是重要的安全边界。
- Per-conversation 队列确保同一 chatId 的消息串行处理（`botConversationQueues`，`main.ts:2798`），防止并发 session create 竞态。
- `rememberBotSourceEvent` 使用大小为 1000 的 LRU-ish Map 做幂等去重，但没有 TTL 淘汰。

### Flow 2: OpenGateway Request → Auth → API 处理 → State/SSE

```
[127.0.0.1 caller]
    │ HTTP GET/POST to http://127.0.0.1:3939
    ▼
[createServer callback → handle(req, res)]
    ├─ OPTIONS → 204 CORS preflight
    ├─ /health → 200 (no auth required)
    └─ 其他端点 → isAuthorized(req, token)
        │ Bearer token 不匹配 → 401
        ▼
    [路由匹配]
    ├─ /v1/capabilities → GET 返回能力清单
    ├─ /v1/state → GET 聚合 gateways/sessions/incidents/requests state
    ├─ /v1/sessions → GET 列出所有 session
    ├─ /v1/sessions/{sessionId}/messages → POST sendMessage (202)
    ├─ /v1/sessions/{sessionId}/events → GET SSE stream
    ├─ /v1/search/thread → GET 搜索
    └─ ... → 404
    ▼
[SSE stream lifecycle]
    │ openSessionEventStream(sessionId, replayCursor, req, res)
    ├─ 写 SSE headers → 200 text/event-stream
    ├─ 注册 GatewayEventClient → 加入 eventClients Map
    ├─ replayRecentEvents (catch-up)
    └─ 等待 publishSessionEvent 推送
        │ req.on('close') → removeEventClient
```

**关键观察**：
- CORS 限制为 `http://127.0.0.1`（`open-gateway.ts:159`），外部浏览器跨域请求会被拒绝。
- Token 鉴权使用精确字符串比较 `req.headers.authorization === expected`（`open-gateway.ts:473–476`），无 HMAC 或 JWT。
- SSE 连接没有全局数量限制，仅在 `GatewayReplayState.activeStreams` 中透明上报。

## attack_surface

### 1. [HIGH] OpenGateway Token 泄露导致全量 Session 数据暴露

**Evidence**: `open-gateway.ts:473–476`
```typescript
private isAuthorized(req: IncomingMessage, token: string): boolean {
  const expected = `Bearer ${token}`;
  return token.length > 0 && req.headers.authorization === expected;
}
```

**攻击场景**：
- Token 明文存储在 `appSettings.openGateway.token` 中，持久化到磁盘的 `settings.json`。
- 任何能读取 `settings.json` 的本地进程可以获取 token，进而通过 `GET /v1/sessions/{sessionId}/messages` 读取所有对话历史、通过 SSE 实时监听所有 session 事件。
- Token 不支持轮换，不支持 scope 限制（读写不分离），一个 token 即可访问所有 API。

**影响**：本地信息泄露、对话窃听。依赖部署环境——需要本地文件读取权限或 XSS 跳板。

**可利用性**：需要本地文件系统访问权限。但是在共享桌面/多用户环境中，如果 settings 文件权限为 644，则同机其他用户可读取。

---

### 2. [HIGH] Bot 入站消息可触发无限制 Session 创建（资源耗尽）

**Evidence**: `main.ts:2867–2879`
```typescript
if (!sessionId) {
  const ready = await getReadyConnection(...);
  const summary = await runtime.createSession({
    permissionMode: 'explore',
    name: `${botDisplayLabel(message.platform)} 对话`,
    labels: ['bot', message.platform],
  });
  sessionId = summary.id;
  botConversationSessions.set(conversationKey, sessionId);
}
```

**攻击场景**：
- `botConversationSessions` 永不超时移除已绑定的 session（只在 reset 命令时删）。
- 新建 bot 会话的唯一条件是 `!sessionId`，即该 `platform:chatId` 尚未绑定——对话一旦绑定就永久有效。
- 如攻击者控制了一个 Discord 服务器并将 Bot 加入，可通过创建大量 channel 并在每个 channel 中发送消息来创建等量的 Session，填满 `store` 的磁盘空间。
- `botRecentSourceEventKeys` 大小上限为 1000（`main.ts:897`），但没有过期机制，长时间运行后可能逐出正常消息的去重 key，导致重复处理。

**影响**：磁盘空间耗尽、session store 性能退化。

**可利用性**：需要 bot 加入攻击者控制的群组/频道。对公开 Bot 真实可行。

---

### 3. [MEDIUM] OpenGateway SSE 连接不受限导致的资源耗尽

**Evidence**: `open-gateway.ts:508–538`
```typescript
private openSessionEventStream(...): void {
  // ...
  const client: GatewayEventClient = {
    response: res,
    heartbeat: setInterval(() => { ... }, OPEN_GATEWAY_EVENT_HEARTBEAT_MS),
    write(chunk) { res.write(chunk); },
  };
  const clients = this.eventClients.get(sessionId) ?? new Set<GatewayEventClient>();
  clients.add(client);
  this.eventClients.set(sessionId, clients);
  // ...
}
```

**攻击场景**：
- 无全局 SSE 连接数上限。持有有效 token 的调用方可以打开任意数量的 SSE 流（对同一或不同 sessionId）。
- 每个 SSE 连接维持一个 `setInterval` heartbeat（15s），大量连接会累积 CPU timer 开销。
- 如果请求方不发送 `Connection: close` 且不主动断开，连接会一直保持直到进程退出或 token 变更。

**影响**：CPU/memory 资源耗尽，影响主进程稳定性。

**可利用性**：需要 valid token（即需要先拿到 token）。是二次攻击路径。

---

### 4. [MEDIUM] sessionId 注入 / 路径注入通过 decodeURIComponent 传播

**Evidence**: `open-gateway.ts:352–374`
```typescript
const sessionStateMatch = url.pathname.match(/^\/v1\/sessions\/([^/]+)\/state$/);
if (sessionStateMatch) {
  const sessionId = decodeURIComponent(sessionStateMatch[1]!);
  const session = (await this.deps.listSessions())
    .find((candidate) => candidate.id === sessionId);
  writeJson(res, 200, {
    state: buildGatewaySessionState({
      session,
      messages: await this.deps.readMessages(sessionId),
      // ...
    }),
  });
}
```

**攻击场景**：
- Regex `([^/]+)` 在 `decodeURIComponent` 之前匹配，意味着 `%2F`（编码的 `/`）在 regex 阶段不会被截断，但 `decodeURIComponent` 后可能还原为 `/`。
- 虽然 `listSessions().find()` 的精确匹配使得注入的 sessionId 通常找不到对应记录（返回 404），但如果攻击者控制了 sessionId 的生成源（如 bot 创建的 session 使用攻击者提供的输入），则可能构造特殊路径。
- `redactSecrets(capReplayCursor(sessionId))` 在输出侧做了截断和红化，但输入侧没有额外验证。

**影响**：低——`listSessions().find(id)` 的精确匹配使 IDOR 难以利用，但 decodeURIComponent 的双重解码路径是不必要的复杂性。

**可利用性**：当前影响有限，但在 sessionId 生成策略变化时可能变成可 exploit 路径。

---

### 5. [MEDIUM] Bot 凭据通过 proxiedFetch 的代理隧道泄露

**Evidence**: `simple-bridge.ts:500–512`, `discord-bridge.ts:302–318`, `proxied-fetch.ts:13–69`

**攻击场景**：
- 所有 Bot 出站请求经过 `proxiedFetch`，它会透明应用系统代理配置（`resolveActiveProxy()`）。
- Telegram `telegramApi` 将 Bot Token 直接嵌入 URL：`https://api.telegram.org/bot${token}/${method}`。
- Discord/DingTalk/QQ bridge 将 token 放在 `Authorization` header 中。
- 如果配置了 HTTP 代理（非 CONNECT tunnel），Bot Token 会以明文形式通过代理服务器。代理日志可能记录完整 URL 和 header。
- WeChat iLink 的 `randomWechatUinHeader` 使用 `randomBytes(4)` 生成 header，但该 header 仅用于防重放而非鉴权。

**影响**：Bot Token 通过中间代理泄露，攻击者可仿冒 Bot 发送消息、读取消息。

**可利用性**：需要用户配置了非加密代理（HTTP proxy 而非 SOCKS5/CONNECT tunnel）。

---

### 6. [LOW] search/thread 端点缺少 query 参数长度限制

**Evidence**: `open-gateway.ts:461–468`
```typescript
if (url.pathname === '/v1/search/thread') {
  const query = url.searchParams.get('q') ?? '';
  writeJson(res, 200, { ok: true, result: await this.deps.searchThread(query) });
  return;
}
```

**攻击场景**：
- `q` 参数没有长度上限，直接传入 `runThreadSearch`（`main.ts:462–467`）。
- 如果 `runThreadSearch` 实现的搜索算法时间复杂度与输入长度正相关（如正则匹配、全文扫描），超大输入可能导致 CPU 峰值。
- `readMessages(sessionId)` 为每个 session 返回所有消息，搜索结果构建阶段可能消耗大量内存。

**影响**：DoS via 超大搜索词。影响受限于 `thread-search.ts` 的具体实现（搜索词在匹配前应经过归一化）。

**可利用性**：需要 valid token。`thread-search.ts` 层面可能有自己的输入截断——需要在搜素模块精读中确认。

---

### 7. [LOW] Plaintext 命令注入（平台端欺骗）

**Evidence**: `bot-events.ts:130–181`

```typescript
export const BOT_PLAINTEXT_RESET_COMMANDS = Object.freeze([
  'restart', 'reset', '/restart', '/reset', '/new', '/newchat',
  'new chat', '重启', '重置', '重新开始', '新对话', '新会话',
]);
export function isPlaintextResetCommand(
  message: Pick<BotMessageEvent, 'text' | 'isGroup'>,
): boolean {
  if (message.isGroup) return false;
  const trimmed = message.text.normalize('NFC').trim().toLowerCase();
  return BOT_PLAINTEXT_RESET_COMMANDS.includes(trimmed);
}
```

**攻击场景**：
- 攻击者如果控制了 Telegram Bot 的 webhook URL 或伪造了入站消息（需要突破 Telegram 的服务器端验证），可以发送 `reset` 命令解绑用户的 bot conversation，中断其对话上下文。
- 但 `isGroup` 检查阻止了群组中滥用 reset——`main.ts:2853` 在调用前已检查。
- help 命令不受影响——仅是发送帮助文本。

**影响**：低。DM 中的 reset 只影响单个 bot 对话的 session 绑定（用户可在桌面端继续原 session），不会删除数据。

**可利用性**：需要绕过平台服务器端验证，在 Telegram API 的正常使用场景下不可行。

---

### 8. [INFO] WeChat Bridge localhost 约束 + 动态 require 的风险面

**Evidence**: `wechat-bridge.ts:16–37, :14`

```typescript
const LOCAL_WECHAT_BRIDGE_HOSTS = new Set(['127.0.0.1', 'localhost', '[::1]', '::1']);
export function normalizeWechatBridgeUrl(input: string | undefined): string | null {
  const url = new URL(raw);
  if (!LOCAL_WECHAT_BRIDGE_HOSTS.has(url.hostname)) return null;
  // ...
}
const require = createRequire(import.meta.url);
// 在 renderWechatQrCode 中使用 require('qrcode')
```

**攻击场景**：
- WeChat bridge 的 URL 被强制约束为本地地址，但该约束在 `normalizeWechatBridgeUrl` 中检查 `hostname`——如果攻击者能让 DNS 解析将 `localhost` 指向外部 IP（需要本地 hosts 文件修改），则可绕过。
- `createRequire(import.meta.url)` 允许在 ESM 环境中使用 CommonJS `require`，这在 `renderWechatQrCode` 中动态加载 `qrcode` npm 包。如果 `qrcode` 包本身存在供应链攻击，则可能在主进程中执行恶意代码。

**影响**：低。localhost 劫持需要本地写入权限，`qrcode` 供应链风险属于依赖管理范畴。

**可利用性**：需要本地管理员权限或 npm 供应链攻击。

## mitigations

### Rate Limit

- **现状**：无任何速率限制。Bot 入站、OpenGateway HTTP 请求、SSE 连接均不受限。
- **建议**：
  - OpenGateway 层：按 token + path 实施 token bucket（如 100 req/min per endpoint）。
  - Bot 入站层：按 `platform:chatId` 实施滑动窗口限制（如 30 messages/min），超过后返回 `rate-limited` readiness。
  - SSE 连接：全局最大连接数（如 10），超过后返回 429。

### Permission Mode

- **现状**：Bot session 强制 `explore` 模式（只读），这是正确的防御纵深。但在 `main.ts:2876` 中是硬编码的，缺乏可配置性。
- **建议**：
  - 为 Bot session 提供可配置的 permission mode（如仅允许 `explore` 或 `ask`，永远禁止 `auto_approve`）。
  - 在 `buildSystemPrompt` 中针对 bot session 注入更严格的工具约束（`main.ts:3120` 已注入 `botPlatformHint`，但未限制工具列表）。

### Token Scoping

- **现状**：OpenGateway 使用单一 Bearer Token，授予全部 API 权限。
- **建议**：
  - 实现 token scope：只读（GET endpoints only）vs 读写（含 POST sendMessage）。
  - 支持 token 轮换：在 settings 中存储 current + previous token，允许平滑切换。
  - Token 最小长度要求（当前仅检查 `token.length > 0`）。

### SSE Limits

- **现状**：SSE 无连接数上限，heartbeat 间隔固定 15s。
- **建议**：
  - 全局最大 SSE 连接数（如 10），超过后返回 503。
  - Per-session 最大 SSE 连接数（如 3）。
  - 对空闲 SSE 连接（超过 5 分钟无事件）主动关闭。
  - 在 SSE heartbeat 发送失败（如 write 返回 false）时主动移除 client。

### Path/Input Normalization

- **现状**：`decodeURIComponent` 在输入侧使用，`capGatewayPath`（500 字符截断）+ `capReplayCursor`（256 字符截断）在输出侧使用。
- **建议**：
  - Input 侧增加 sessionId 格式验证（如仅允许 URL-safe 字符，限制长度 64）。
  - search `q` 参数限制最大长度（如 500 字符）。
  - sessionId match regex 使用更严格的字符集（如 `/^\/v1\/sessions\/([a-zA-Z0-9_-]+)\/state$/`）。

### Bot 入站防御

- **现状**：`botRecentSourceEventKeys` 做幂等去重但无过期（仅大小限制 1000）。
- **建议**：
  - 添加 TTL（如 1 小时），定期清理过期 key。
  - Per-platform+chat 入站速率限制（token bucket），防御恶意高频消息。
  - `botConversationSessions` 添加最大绑定数（如 500），超过后拒绝新绑定。
  - Allowlist（`allowedUserIds`）当前仅在 Telegram bridge 生效 — 扩展到所有 bridge。

## next_actions

1. **立即（高优先级）**：
   - 为 `open-gateway.ts` 添加全局 SSE 连接数上限（`MAX_EVENT_STREAMS = 10`）——当前代码无任何限制，是理论上可被快速利用的 DoS 向量。
   - 为 `handleBotIncomingMessage` 添加 per-conversation 速率限制，防御 bot session 创建风暴。
   - 审计 `botRecentSourceEventKeys` 是否在长期运行中会导致 OOM（1000 key 无 TTL 逐出）——建议添加 1 小时 TTL。

2. **短期（中优先级）**：
   - OpenGateway Token 添加最小长度要求（≥16 字符）和 scope 分离（read / read+write）。
   - 所有 bot bridge 入站消息在 `BotRegistry` 层添加统一的速率门控。
   - sessionId 输入侧添加格式白名单验证（字母数字 + `-_`，长度 1–64）。
   - search `q` 参数添加 500 字符上限。

3. **中期（低优先级）**：
   - Token 轮换机制：settings 支持 `currentToken` + `previousToken`，允许无缝切换。
   - Bot session permission mode 可配置化（通过 bot settings，而非硬编码 `explore`）。
   - WeChat bridge 的 `qrcode` 动态 require 替换为静态 ESM import 或 tree-shakable 加载。
   - 为 bot 消息 idempotency 添加基于时间的 LRU 淘汰（当前 Map 只按插入顺序删除最旧 key，不关心时间）。

4. **待确认（需要环境验证）**：
   - `thread-search.ts` 的 `runThreadSearch` 对大 query 的 CPU/内存行为——如果搜索使用线性扫描+正则，需要在搜索模块精读中独立评估。
   - `proxiedFetch` 在代理模式下对 Bot Token URL 嵌入的泄露风险评估——需要确认典型部署中代理类型分布（HTTP vs SOCKS5）。
