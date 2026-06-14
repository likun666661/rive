# Desktop Main / IPC / Credential Bridge 现状分析

> 基线：`4dd1bf1` | 对比起点：`05ca5a3` | 深度档位：`maintainer`

---

## scope

### 已读文件

| 文件 | 行数 | 角色 |
|------|------|------|
| `apps/desktop/src/main/main.ts` | 3620 | 主进程入口，所有 IPC handler 注册、`BackendRegistry`/`SessionManager` 初始化 |
| `apps/desktop/src/preload/preload.ts` | 745 | `contextBridge.exposeInMainWorld`，renderer 唯一入口 |
| `apps/desktop/src/main/credential-store.ts` | 248 | `SafeStorageCredentialStore`，7 种 secret kind、文件写入 |
| `apps/desktop/src/main/settings-ipc-helpers.ts` | 163 | `maskAppSettings`、敏感字段占位保护、bot 测试结果格式化 |
| `apps/desktop/src/main/chat-readiness.ts` | 247 | `requireReadyConnection`、`ensureSessionCanSendOrRebind`、send 前门控 |
| `apps/desktop/src/main/connection-test-status.ts` | 36 | connection test 结果 → `UpdateConnectionInput` patch |
| `packages/core/src/provider-auth.ts` | 295 | `deriveProviderAuthContract`：基于 `LlmConnection` 的 UI 状态机 |
| `packages/core/src/llm-connections.ts` | 521 | `ProviderType`、`LlmConnection`、`validateConnectionBaseUrl`、`normalizeConnectionBaseUrl` |
| `packages/core/src/connection-readiness.ts` | 147 | `isConnectionReady`：纯函数、同步判读连接可否发送（PR110a） |

### IPC handler 清单

**连接凭据类**：`connections:create`、`connections:update`、`connections:delete`、`connections:setDefault`、`connections:test`、`connections:fetchModels`、`connections:hasSecret`

**会话类**：`sessions:create`、`sessions:send`、`sessions:stop`、`sessions:setModel`、`sessions:retryTurn`、`sessions:regenerateTurn`、`sessions:branchFromTurn`

**设置类**：`settings:get`、`settings:update`、`settings:testNetworkProxy`、`settings:testBotChannel`

**OAuth 订阅类**：`claude-subscription:*`、`codex-subscription:*`、`cursor-subscription:*`、`antigravity-subscription:*`

**其他**：`onboarding:*`、`quickChat:start`、`health:getSnapshot`、`capabilities:getSnapshot`、`search:thread`、`web-search:query/test`

### Credential API 清单

| 方法 | 存储 key | 用途 |
|------|----------|------|
| `getApiKey / setApiKey` | `{slug}:apiKey` | LLM API key |
| `getOAuthToken / setOAuthToken` | `{slug}:oauthToken` | OAuth 令牌 |
| `getBotToken / setBotToken` | `settings:bot:{provider}:botToken` | 机器人 token |
| `getBotAppSecret / setBotAppSecret` | `settings:bot:{provider}:botAppSecret` | 飞书 app secret |
| `getProxyPassword / setProxyPassword` | `settings:network-proxy:proxyPassword` | 代理密码 |
| `getGatewayToken / setGatewayToken` | `settings:open-gateway:gatewayToken` | 开放网关 token |
| `getTavilyApiKey / setTavilyApiKey` | `settings:web-search:tavily:tavilyApiKey` | Tavily 搜索 key |
| `getSecret / setSecret / deleteSecret` | `{slug}:{storedKind}` | 通用接口 |

---

## problem

Desktop bridge 是 Maka 安全架构中**最脆弱的跨进程边界**。原因有三个层级：

1. **Render → Main 的攻击面不可消除**：Electron 的 `contextBridge`/`contextIsolation` 只能阻止 renderer 直接调用 Node API，但 renderer 仍然可以构造任意 IPC 参数。如果 main 端不做防御式校验，renderer 里的 XSS 或恶意依赖就可以通过 `connections:create` 注入了指向攻击者服务器的 `baseUrl`，让用户的所有 API 调用落进中间人端点。

2. **凭据生命周期分散**：API key 在 `credential-store.ts`（Electron safeStorage 加密），OAuth token 在 `ClaudeSubscriptionService`（独立 token 文件），bot token / proxy password 散落在 `settingsStore` 的 JSON 里（SENSITIVE_PLACEHOLDER 模式）。新后台（AI SDK backend）需要的 `apiKey` 来自 `credentialStore.getSecret(slug, 'api_key')` 再经过 `resolveConnectionSecret()` 路由：api_key 走 credential store，OAuth 走 subscription service 的 `getAccessTokenInternal()`。如果新后台的 provider 添加了一种新的 `authKind`，这条路可能完全绕开安全存储。

3. **Send 前门控不全**：`ensureSessionCanSendOrRebind` 只检查 `connection.enabled`、`hasSecret`、`model` 是否有效。它不校验 `connection.providerType` 是否确实能被 `BackendRegistry` 注册的 `AiSdkBackend` 消费。新后台如果是 `gemini-cli`（`status: 'phase3-experimental'`、`authKind: 'oauth_token'`、但不在有线 OAuth 提供者列表中），`isConnectionReady` 会返回 `oauth_subscription_not_wired`，send 路径会 crash。

---

## current_design

### 三层边界

```
Renderer (untrusted)
    |
    | contextBridge (preload.ts:119-744)
    | 暴露 ~15 个命名空间（sessions, connections, settings...）
    | 每个方法都是 ipcRenderer.invoke() 的一层薄封装
    |
    v
Main Process (trusted)
    |
    | ipcMain.handle() 注册 (main.ts:1413-2774)
    | 每个 handler 内部做：
    |   1. 参数类型校验 (normalizeConnectionSlugForIpc 等)
    |   2. baseUrl 白名单校验 (http/https only)
    |   3. 凭据查找 (credentialStore / subscription service)
    |   4. 连接可发送判读 (isConnectionReady / requireReadyConnection)
    |   5. 副作用调用 (store 写入 / runtime 调用)
    |
    v
Runtime / BackendRegistry / SessionManager (main.ts:944-949)
    |
    | backends.register('ai-sdk', ...) → AiSdkBackend
    | 传入 apiKey + connection + model → AI SDK provider
    |
    v
Credential Store (safeStorage 加密)
    |
    | credentials.json → Read → safeStorage.decryptString
    | 写入走原子 rename(tempPath, targetPath)
```

### 关键设计决策

- **Token 从不跨 IPC 边界**：OAuth token 由 `resolveConnectionSecret()` 在主进程内部解析；preload 只暴露 `getAccountState()` 这种状态快照，不返回 raw token。API key 同样如此——renderer 只能通过 `connections:hasSecret` 查询布尔值。
- **`SENSITIVE_PLACEHOLDER` 模式**：settings 中的 `proxy.password`、`botChat.channels.*.token`、`gatewayToken` 返回给 renderer 时被 `maskSensitive()` 替换；renderer 重新提交时，`preserveSensitivePlaceholders()` 检测占位符并保留原有值。
- **Slug 白名单校验**：`normalizeConnectionSlugForIpc` 强制字母数字+点号+下划线+连字符，禁止控制字符和路径穿越。
- **BaseUrl 白名单**：`normalizeConnectionBaseUrl` 只允许 `http:`/`https:` scheme。

---

## source_evidence

| 证据点 | 文件:行号 | 输入校验 | 错误处理 | Secret 处理 |
|--------|-----------|----------|----------|-------------|
| Slug 校验 (IPC 入口) | `main.ts:420-437` | `typeof !== 'string'`、空串、长度上限、正则 allowlist、控制字符、`..` 路径穿越 | `throw new Error` 立即中断 | 不涉及 |
| API Key 校验 (IPC 入口) | `main.ts:439-450` | `typeof !== 'string'`、长度上限 4096、控制字符拒绝 | `throw new Error`，**错误信息不回显 key 内容** | 校验后立即写入 credential store |
| BaseUrl 白名单 | `llm-connections.ts:363-455` | `typeof !== 'string'`、长度 2048、`new URL()` 构造验证、scheme `http:`/`https:` only | `{ ok: false, error }` 返回 | 不涉及 |
| Create Input 规范化 | `main.ts:452-468` | apiKey 先校验 → slug 后校验，OAuth 强置 baseUrl | `throw new Error` | apiKey 被 `normalizeConnectionApiKeyForIpc` 处理 |
| Update Input 规范化 | `main.ts:470-495` | `hasOwnProperty` 检测是否存在 apiKey、`apiKey === undefined` 保留清除语义 | `throw` 向上传播 | `apiKey` 非空则 set，为空则 delete |
| 连接可发送判读 | `connection-readiness.ts:91-125` | 纯函数，7 阶检查：fake→enabled→oauth_wired→hasSecret→model→empty_list→model_enabled | 返回 `{ready: false, reason}` | `hasSecret` 作为布尔参数传入 |
| 凭据解析路由 | `main.ts:400-409` | `providerType === 'claude-subscription'` → OAuth service；`codex-subscription` → OAuth service；默认 → credential store | 返回 `string | null` | 三种存储后端统一接口 |
| Send 前门控 | `main.ts:3208-3233` | 读 header → `ensureSessionCanSendOrRebind` → 失败则 set `blocked` 状态 | 写 session status + emit `sessions:changed` | 不直接操作 |
| settings:get masking | `settings-ipc-helpers.ts:66-116` | 对比 `revealPatch` 决定是否露出明文 | 永远不抛，只返回 masked | password/token/appSecret 被 `maskSensitive` |
| settings:update placeholder | `settings-ipc-helpers.ts:14-64` | 检测 `SENSITIVE_PLACEHOLDER` → 替换为 current 值 | 直接替换，不抛 | 新旧值交替逻辑 |
| OAuth IPC 边界 | `main.ts:1864-2192` | `isExperimentalEnabled()` 两次检查（renderer UI + main handler）、`typeof authRequestId !== 'string'` | 返回 `{ ok: false, reason: 'experimental_disabled' }` | Token 永远不返回 |
| Credential 存储 | `credential-store.ts:168-209` | `safeStorage.isEncryptionAvailable()` 检查、`ENOENT` 兜底空文件 | `throw new Error` 如果加密不可用 | 加密→base64 存储、读取解密、原子 rename 写入 |

---

## ipc_flow

### 1. Connection Credential 链路

```
Renderer                               Main
  │                                      │
  │── connections:create(input) ────────>│
  │                                      │ normalizeCreateConnectionInput(input)
  │                                      │   ├─ normalizeConnectionApiKeyForIpc(apiKey)
  │                                      │   │   ├─ typeof string
  │                                      │   │   ├─ length ≤ 4096
  │                                      │   │   └─ 无控制字符
  │                                      │   ├─ normalizeConnectionSlugForIpc(slug)
  │                                      │   │   ├─ typeof string
  │                                      │   │   ├─ 非空
  │                                      │   │   ├─ ≤ 64 字符
  │                                      │   │   ├─ [A-Za-z0-9._-]+
  │                                      │   │   ├─ 无控制字符
  │                                      │   │   └─ 无路径穿越 `..`
  │                                      │   └─ normalizeConnectionBaseUrl(baseUrl)
  │                                      │       ├─ typeof string
  │                                      │       ├─ length ≤ 2048
  │                                      │       └─ http: / https: only
  │                                      │
  │                                      │ connectionStore.create(normalizedInput)
  │                                      │ credentialStore.setSecret(slug, 'api_key', apiKey)
  │                                      │   ├─ safeStorage.isEncryptionAvailable()
  │                                      │   ├─ safeStorage.encryptString(value) → base64
  │                                      │   └─ atomic write (temp → rename)
  │<── LlmConnection (without secret) ───│
```

### 2. Session Start 链路

```
Renderer                               Main
  │                                      │
  │── sessions:create(input) ──────────>│
  │                                      │  1. requestedSlug = input.llmConnectionSlug ?? getDefault()
  │                                      │  2. getReadyConnection(requestedSlug, model)
  │                                      │     └─ requireReadyConnection(slug, deps, model)
  │                                      │         ├─ slug 为 null/fake → throw
  │                                      │         ├─ connectionStore.get(slug)
  │                                      │         ├─ resolveConnectionSecret(slug)
  │                                      │         │   ├─ OAuth? → subscription.getAccessTokenInternal()
  │                                      │         │   └─ else → credentialStore.getSecret(slug, 'api_key')
  │                                      │         └─ isConnectionReady({connection, hasSecret, model})
  │                                      │             ├─ fake_backend?
  │                                      │             ├─ !enabled?
  │                                      │             ├─ oauth_subscription_not_wired?
  │                                      │             ├─ !hasSecret && authKind !== 'none'?
  │                                      │             ├─ !model?
  │                                      │             ├─ empty_model_list?
  │                                      │             └─ model_not_enabled?
  │                                      │
  │                                      │  3. runtime.createSession(...)
  │                                      │  4. emitSessionsChanged('created')
  │<── SessionSummary ──────────────────│
```

### 3. Model Readiness 链路 (Send Path)

```
Renderer                               Main
  │                                      │
  │── sessions:send(sessionId, cmd) ───>│
  │                                      │  normalizeSessionSendCommand(cmd)
  │                                      │  ensureSessionCanSend(sessionId)
  │                                      │    ├─ store.readHeader(sessionId)
  │                                      │    └─ ensureSessionCanSendOrRebind(header, deps)
  │                                      │        ├─ assertSessionCanSend(header, deps)
  │                                      │        │   ├─ backend === 'fake'? → throw
  │                                      │        │   └─ requireReadyConnection(slug, deps)
  │                                      │        │        (同 sessions:create 的步骤 2)
  │                                      │        │
  │                                      │        └─ [catch] shouldRebindSessionToDefault?
  │                                      │            ├─ 尝试 default 连接
  │                                      │            ├─ runtime.updateSession(...)
  │                                      │            └─ return { rebound: true }
  │                                      │
  │                                      │  validateRendererAttachments(attachments)
  │                                      │  runtime.sendMessage(sessionId, { turnId, text, attachments })
  │                                      │    └─ SessionManager → BackendRegistry → AiSdkBackend
  │                                      │        └─ apiKey + model → AI SDK fetch
  │<── streamEvents (push via webContents) │
```

### 4. Settings → Runtime 传递链路

```
Renderer                               Main
  │                                      │
  │── settings:update(patch) ──────────>│
  │                                      │  normalizeSettingsPatch(patch)
  │                                      │    └─ preserveSensitivePlaceholders(patch, current)
  │                                      │        ├─ SENSITIVE_PLACEHOLDER token → current.token
  │                                      │        ├─ SENSITIVE_PLACEHOLDER appSecret → current.appSecret
  │                                      │        └─ SENSITIVE_PLACEHOLDER proxyPassword → current.password
  │                                      │
  │                                      │  settingsStore.update(normalizedPatch)
  │                                      │
  │                                      │  applySettingsRuntimeEffects(next, patch)
  │                                      │    ├─ patch.network? → setActiveProxy(proxy)
  │                                      │    ├─ patch.botChat? → botRegistry.applySettings()
  │                                      │    └─ patch.openGateway? → openGateway.sync()
  │                                      │
  │<── UpdateAppSettingsResult ─────────│ (masked, no secrets)
```

---

## tests

### 已覆盖的 contract tests

| 测试文件 | 核心覆盖 |
|----------|---------|
| `connection-credential-ipc-hardening-contract.test.ts` (167 行) | 10 个测试用例。验证：`IPC_CONNECTION_SLUG_MAX_LENGTH`/`IPC_CONNECTION_SECRET_MAX_LENGTH`/控制字符/allowlist 常量存在；slug 校验拒绝非安全字符+路径穿越；create/update handler 中 normalize 在 store 写入之前执行；update 清除语义（`apiKey === undefined` 允许删除）；OAuth baseUrl 强置；错误信息不泄露明文 key |
| `credential-store-contract.test.ts` (58 行) | 验证 7 种 secret kind 声明、传统 key 名 `'api_key'→'apiKey'` 兼容、Phase 1 settings secret kinds 存在、15 个 typed helpers 存在、non-secret slug 前缀确定 |
| `credential-store-secret-kinds-contract.test.ts` (143 行) | 更细粒度的验证：加密/解密流程（`safeStorage.encryptString`→base64、`isEncryptionAvailable` 检查）、`ENOENT` 兜底空文件、delete 幂等性、botSecretSlug 不使用 token/secret 内容做 key、settings 层未开始 migration |

### 未覆盖的风险面

| 风险 | 优先级 | 说明 |
|------|--------|------|
| `safeStorage` 不可用时没有降级路径 | **P0** | `credential-store.ts:176-178` 的 `set()` 检查 `isEncryptionAvailable()` 并抛出 Error。Linux 无 keychain 环境下这将导致所有凭据写入失败。没有 fallback 到基于文件系统权限的加密或用户提示。 |
| renderer 构造的超大/畸形 `baseUrl` | **P1** | `validateConnectionBaseUrl` 处理了已知攻击向量（scheme allowlist），但没有测试 `data:` URL、`//` 协议相对 URL、超长 path 对 fetch 下游的影响。依赖 AI SDK 的二次校验。 |
| OAuth 提供者扩展需手动注册 | **P1** | `isConnectionReady:102-107` 和 `resolveConnectionSecret:402-408` 都有硬编码的 `claude-subscription`/`codex-subscription` 分支。添加新的 OAuth 提供者（如 `gemini-cli`）必须在两个函数中同步修改，否则 send 路径会被 `oauth_subscription_not_wired` 拦截。 |
| `settings:get` 返回整个 settings 对象 | **P2** | 包含 `webSearch.providers.tavily.apiKey` 虽被 `maskSensitive` 处理，但如果未来新增字段未在 `maskAppSettings` 注册，可能泄露明文。 |
| contract tests 是源码扫描，不是运行时测试 | **P2** | 三个 test 文件都用 `readFileSync` 读取源码做正则匹配，验证的是"代码写对了"而非"运行时行为正确"。无法检测 `safeStorage.encryptString` 实际调用失败、文件写入竞争条件等。 |
| Slack/微信配置的 secret 仍混合在 settings JSON 中 | **P2** | `botChat.channels[provider].token` 和 `appSecret` 在 settings JSON 中以 `SENSITIVE_PLACEHOLDER` 模式存储（明文在 settings JSON 里，renderer 看到 masked 字符串）。没有迁移到 `credentialStore.setBotToken()` 的加密存储。 |

---

## risks

### P0 — 阻塞新 AI SDK backend

1. **OAuth 新提供者集成点分散**：`isConnectionReady()`（`connection-readiness.ts:102-107`）、`resolveConnectionSecret()`（`main.ts:402-408`）、`normalizeCreateConnectionInput`（`main.ts:458-461`）、`normalizeUpdateConnectionInput`（`main.ts:486-488`）四处都需要知道哪些 OAuth 提供者是"有线"的。添加 `gemini-cli` 作为发送可用后台需要修改至少三处。

2. **`safeStorage` failure 无降级**：`credential-store.ts:176` 的 `isEncryptionAvailable()` 检查在 Linux headless 或某些桌面环境失败时，**所有凭据写入都会 throw**。新后台如果依赖 `api_key` 存储将完全不可用。

3. **`credential-store.ts` 的 `CredentialKind` union 是封闭的**：当前只有 7 种 kind。如果新后台需要额外的凭据类型（比如 `certificate`、`mTLS`），需要同时添加 `StoredCredentialKind`、`CredentialKind`、`STORED_CREDENTIAL_KINDS` 数组、`toStoredKind` switch case、`CredentialStore` interface 方法，并在三个 contract test 文件中更新断言。

### P1 — 中等风险

4. **IPC handler 的 error 直接 throw 给 renderer**：`connections:create`/`connections:update` 等 handler 抛出原生 `Error` 对象。Electron 的 `ipcMain.handle` 会将 error 序列化后传回 renderer，但 error 的 `code`/`reason` 自定义属性在序列化时丢失（除非显式设置 `cause` 或传回结构化对象）。`chat-readiness.ts:216-220` 的 `chatConfigurationError` 使用了 `code` + `reason` 自定义属性，这些跨 IPC 边界可能丢失，renderer 只能看到 `message`。

5. **`hasSecret` 是布尔值但 `resolveConnectionSecret` 返回 `string | null`**：`chat-readiness.ts:78` 将 `apiKey` 转为 `typeof apiKey === 'string' && apiKey.length > 0` 的布尔值。如果 `safeStorage.decryptString` 抛出异常（畸形数据），`credentialStore.getSecret` 不会 catch ——异常会向上传播到 handler，变成未捕获的 Promise rejection。

6. **连接更新时 credential 操作与 connection store 操作不是原子的**：`connections:update` (main.ts:2434-2443) 先 `connectionStore.update`，再 `credentialStore.setSecret`。如果前者成功后者失败，连接元数据已更新但凭据未变；反之亦然。没有事务性回滚。

### P2 — 低风险 / 未来关注

7. **preload 暴露的 API 面太大**：`preload.ts` 暴露了 ~200 个方法。虽然每条路都有 main 端校验，但攻击面越大，遗漏一个未校验的 handler 的概率越高。

8. **`settings:get` 的 masking 是手工注册的**：`settings-ipc-helpers.ts:66-116` 的 `maskAppSettings` 对每个敏感字段做显式 masking。如果 `AppSettings` 新增一个敏感字段（比如 `webSearch.providers.*.apiKey`），没有编译时强制检查是否已注册 masking 逻辑。

9. **`chat-readiness.ts` 的 `sessionRebind` 只在 `connectionLocked === false` 时触发**：如果 session 之前已经有用户消息，`connectionLocked = true`，send 前的 rebind 被完全跳过。这意味着即使默认连接已经有效，旧 session 仍然会卡在 `NO_REAL_CONNECTION` 错误。

---

## next_actions

1. **为新 AI SDK backend 添加 `providerType` 集成清单**：在 `PROVIDER_DEFAULTS` 注册新 provider → 确保 `isConnectionReady` 的 wired OAuth 检查覆盖 → 确保 `resolveConnectionSecret` 路由覆盖 → 添加 contract test 验证新 slug 在 allowlist 中通过。

2. **`safeStorage` failure 降级实现**：在 `credential-store.ts` 中为 `isEncryptionAvailable() === false` 提供至少一个用户可见的提示（通过 `dialog.showErrorBox`），并在应用启动时检查一次。考虑基于文件权限（`chmod 600`）的 fallback 明文存储作为 Linux 兼容路径。

3. **将 `CredentialKind` 的扩展自动化**：从手工 switch-case 迁移到基于配置的映射，使得添加新 kind 只需新增一行记录而不需要改四处代码+三个 test 文件。

4. **编写运行时 integration test**：当前 contract tests 只做源码扫描。编写实际的 `create`→`setSecret`→`getSecret` 循环测试（在 CI 中需要 `safeStorage` mock），覆盖 `isEncryptionAvailable` 的两种分支。

5. **审计 `settings:get` 的 masking 完整性**：为 `maskAppSettings` 编写类型级检查，确保 `AppSettings` 中新增的 string 类型字段不会被遗漏。

6. **IPC error 序列化标准化**：为所有 `ipcMain.handle` 提供统一的 error envelope（`{ ok: false, code, reason, message }`），替代原生 `throw new Error`，确保 `code`/`reason` 不跨 IPC 丢失。

7. **连接事务性更新**：`connections:update` 中 `connectionStore.update` 和 `credentialStore.setSecret` 改为顺序回滚（保存旧 state，失败恢复）。或使用 connection store 的 patch-only API 先做 credential 再做 metadata。
