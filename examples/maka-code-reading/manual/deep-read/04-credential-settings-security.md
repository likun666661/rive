# Credential / Settings Security — 深度阅读报告

> 阅读基线：`335220a` | 深度档位：`maintainer` | 只读，未修改源码

## 1. scope

本报告覆盖 Maka 桌面端中所有 Secret / 凭据类型的**输入、持久化、内存驻留、IPC 边界、消费、展示、删除**全生命周期。涉及的 Secret 类别：

| 类别 | 结构体 / 字段 | 所属存储 |
|------|-------------|---------|
| LLM API Key | `ConnectionAuth.apiKey`, `CreateConnectionInput.apiKey`, `UpdateConnectionInput.apiKey` | `credential-store.ts` (safeStorage) |
| OAuth Token (Claude/Codex/Cursor/Antigravity) | `PersistedTokens.access_token / refresh_token / id_token` | OAuth service 独立文件 (safeStorage, mode 0o600) |
| Bot Token | `BotChannelSettings.token` | `settings.json` (明文 JSON) |
| Feishu App Secret | `BotChannelSettings.appSecret` | `settings.json` (明文 JSON) |
| Proxy Password | `NetworkProxySettings.password` | `settings.json` (明文 JSON) |
| OpenGateway Token | `OpenGatewaySettings.token` | `settings.json` (明文 JSON) |
| Tavily API Key | `WebSearchProviderSettings.apiKey` | `settings.json` (明文 JSON) |

代码分析覆盖以下文件：
- `apps/desktop/src/main/credential-store.ts`
- `apps/desktop/src/main/settings-ipc-helpers.ts`
- `apps/desktop/src/main/main.ts`
- `packages/storage/src/settings-store.ts`
- `packages/storage/src/connection-store.ts`
- `packages/core/src/settings.ts`
- `packages/core/src/llm-connections.ts`
- `apps/desktop/src/main/oauth/*.ts` (8 个文件)
- 相关测试文件

---

## 2. problem

Maka 桌面端目前存在**两类 Secret 存储体系**，安全性显著不同：

### 体系 A：safeStorage 加密存储（较安全）

- **LLM API Key**：通过 `credential-store.ts` 使用 Electron `safeStorage.encryptString()` 加密后写入 `credentials.json`。键名格式 `{slug}:apiKey`，base64 编码保存。
- **OAuth Token（Claude/Codex/Cursor/Antigravity）**：各 OAuth Service 独立使用 `safeStorage.encryptString()` 加密完整 JSON 序列化的 tokens，写入 `userData/` 下的独立文件（含 `access_token`, `refresh_token`, `id_token`, `expires_at`），文件权限为 `0o600`。

### 体系 B：明文 JSON 存储（高风险）

- **Bot Token**（7个平台）：`BotChannelSettings.token` → `settings.json` 明文
- **Feishu App Secret**：`BotChannelSettings.appSecret` → `settings.json` 明文
- **Proxy Password**：`NetworkProxySettings.password` → `settings.json` 明文
- **OpenGateway Token**：`OpenGatewaySettings.token` → `settings.json` 明文
- **Tavily API Key**：`WebSearchProviderSettings.apiKey` → `settings.json` 明文

### 核心问题

**`settings.json` 是一个 JSON 文件，无加密，无权限控制**。任何拥有对 Maka 工作区目录读权限的进程/用户都可以直接读取其中的所有 Secret。`settings.json` 会被频繁读写（每次设置变更、Bot readiness 状态更新等），且通过 `FileSettingsStore.write()` 使用 `temp → rename` 原子写入，但最终文件权限与 workspace 目录默认权限一致。

**IPC 边界防护已基本到位**（`maskAppSettings` 对返回给 renderer 的 Secret 进行 mask），但**持久化层防护不均衡**：LLM API Key 和 OAuth Token 已被 safeStorage 加密；Bot Token、Proxy Password、Gateway Token、Tavily API Key 仍是明文。

---

## 3. source_evidence

### 3.1 credential-store.ts — LLM API Key 加密存储

- `safeStorage.encryptString(value).toString('base64')` — 加密写入（`:89`）
- `safeStorage.decryptString(Buffer.from(encrypted, 'base64'))` — 解密读取（`:80`）
- `safeStorage.isEncryptionAvailable()` 检查失败时抛 Error（`:86`）
- 文件路径：`{workspaceRoot}/credentials.json`
- 原子写入：`{pid}.{ts}.tmp` → `rename`（`:109-111`）
- 队列化操作防止并发写冲突（`:114-118`）

### 3.2 settings-store.ts — settings.json 明文存储

- 文件路径：`{workspaceRoot}/settings.json`（`:62`）
- `write()` 使用 temp → rename 原子写入（`:219-224`）
- 队列化操作（`:226-230`）
- **无加密、无权限设置**

### 3.3 settings.ts — 数据结构定义

- `BotChannelSettings.token`（`:87`）— bot token
- `BotChannelSettings.appSecret`（`:92`）— 飞书 app secret
- `NetworkProxySettings.password`（`:47`）— 代理密码
- `OpenGatewaySettings.token`（`:227`）— 开放网关 token
- `WebSearchProviderSettings.apiKey`（内嵌在 `WebSearchSettings` 中，通过 `normalizeWebSearchSettings` 在 `:658-691` 处理）

### 3.4 settings-ipc-helpers.ts — IPC 边界 Mask 机制

- `maskAppSettings()`（`:66-116`）：将 `password`, `token`, `appSecret`, `apiKey` 替换为 `SENSITIVE_PLACEHOLDER`（`••••••••`）
- `preserveSensitivePlaceholders()`（`:14-64`）：当 renderer 提交的 patch 中是 `SENSITIVE_PLACEHOLDER` 时，用当前存储值替换
- `shouldReveal()`（`:129-131`）：只有显式提供非 placeholder 值时 reveal
- Tavily API key 在 IPC 返回时**永远不 reveal**（`:107-114`），即使 patch 中显式提供

### 3.5 main.ts — IPC Handler 中的 Secret 流

- `connections:create`（`:2323-2347`）：`apiKey` 通过 `credentialStore.setSecret()` 加密存储，**不写入 connection-store**
- `connections:update`（`:2348-2373`）：同上，空 `apiKey` 时 `deleteSecret`
- `connections:delete`（`:2374-2378`）：同时清理 `credentialStore.deleteSecret`
- `resolveConnectionSecret()`（`:400-409`）：OAuth 类型返回 service 的 `getAccessTokenInternal()`，API key 类型从 `credentialStore.getSecret()` 获取
- `settings:get`（`:2490`）：返回 `maskAppSettings(settings)` — **所有敏感字段已 mask**
- `settings:update`（`:2491-2496`）：通过 `preserveSensitivePlaceholders()` + `applySettingsRuntimeEffects()` 处理
- `web-search:query` / `web-search:test`（`:2136-2186`）：`resolveTavilyApiKey()` 从 settings + env + draftKey 优先级解析，密钥永不返回 renderer

### 3.6 OAuth Subscription Services — Token 全生命周期在主进程

**Claude** (`oauth/claude-subscription-service.ts`):
- PKCE + state 验证（`:195-321`）
- Token 存储：`safeStorage.encryptString(JSON.stringify(tokens))` → `writeFile(..., {mode: 0o600})` → `chmod(0o600)`（`:617-630`）
- Token 文件：`userData/.claude_subscription_token`
- `getAccountState()` 返回 `SubscriptionAccountState`，**不含 token 字段**（`:339-362`）
- `getAccessTokenInternal()` 仅在主进程内使用（`:488-498`）

**Codex** (`oauth/codex-subscription-service.ts`):
- 同结构，loopback callback server on port 1455（`:391-452`）
- Token 文件：`userData/.codex_subscription_token`
- `safeStorage` 加密 + `0o600` 权限（`:541-552`）

**Cursor** (`oauth/cursor-subscription-service.ts`):
- 轮询模式：`api2.cursor.sh/auth/poll` 等用户完成浏览器登录（`:330-379`）
- Token 文件：`userData/.cursor_subscription_token`
- 同样 `safeStorage` + `0o600`（`:410-420`）

**Antigravity** (`oauth/antigravity-subscription-service.ts`):
- Placeholder 状态：`GOOGLE_CLIENT_ID = ''`，`getAuthorizationUrl()` 返回失败 envelope（`:131-134`）
- Token 文件：`userData/.antigravity_subscription_token`

### 3.7 llm-connections.ts — Connection 模型不含 Secret

- `LlmConnection` 接口（`:57-76`）：无 `apiKey` 字段
- 注释明确：「Connection records are stored on disk without secrets. API keys and OAuth tokens live in the desktop credential store, keyed by connection slug.」（`:1-6`）
- `CreateConnectionInput`（`:457-464`）含 `apiKey?: string` 但仅在 create IPC handler 中用于写入 `credentialStore`

---

## 4. lifecycle

### 4.1 LLM API Key（`credentialStore` — safeStorage 加密）

| 阶段 | 行为 | 代码路径 |
|------|------|---------|
| **输入** | Renderer 通过 `connections:create` / `connections:update` IPC 发送；主进程 `credentialStore.setSecret(slug, 'api_key', apiKey)` | `main.ts:2343`, `main.ts:2368` |
| **保存** | `safeStorage.encryptString(value)` → base64 → 写入 `credentials.json`（原子 rename）；`isEncryptionAvailable()` 前置检查 | `credential-store.ts:83-91` |
| **读取** | `readFile()` → JSON.parse → Buffer.from(base64) → `safeStorage.decryptString()` | `credential-store.ts:77-81` |
| **使用** | `resolveConnectionSecret(slug)` 在 `connections:test`、`connections:fetchModels`、`getReadyConnection` 中消费 | `main.ts:400-409` |
| **展示** | **永不对 renderer 展示**；`LlmConnection` 类型不含 apiKey 字段；`connections:list` 返回的列表中无 apiKey | `llm-connections.ts:57-76` |
| **删除** | `credentialStore.deleteSecret(slug)` 删除 slug 下所有 key；或在 update 中 `deleteSecret(slug, 'api_key')` | `main.ts:2369-2376` |

### 4.2 OAuth Token（各 Subscription Service — safeStorage 独立文件）

| 阶段 | 行为 | 代码路径 |
|------|------|---------|
| **输入** | 浏览器 OAuth 流程：PKCE 生成 authorize URL → 用户浏览器完成登录 → paste code（Claude）/ loopback callback（Codex/Antigravity）/ 轮询（Cursor）→ token endpoint 交换 | `claude-subscription-service.ts:195-321` |
| **保存** | `JSON.stringify(tokens)` → `safeStorage.encryptString()` → `writeFile(path, buffer, {mode: 0o600})` → `chmod(0o600)` | `claude-subscription-service.ts:617-630` |
| **读取** | `readFile()` → `safeStorage.decryptString()` → `JSON.parse()`；解密失败自动删除损坏文件 | `claude-subscription-service.ts:632-662` |
| **使用** | `getAccessTokenInternal()` 在 `resolveConnectionSecret()` 中消费 → 注入 AI SDK `authToken` / fetch 中间件 | `main.ts:402-407` |
| **展示** | `getAccountState()` 返回 `runtimeState` + `profile`（email, displayName）+ `quota`，**不含 token 字段** | `claude-subscription-service.ts:339-362` |
| **删除** | `logout()` 清除内存缓存 + `fs.unlink(tokenFilePath)` + 清除 pending authorizations | `claude-subscription-service.ts:459-479` |
| **刷新** | `refreshTokens()` → token endpoint `refresh_token` grant → 重新保存；刷新失败**不**自动登出 | `claude-subscription-service.ts:371-388` |

### 4.3 Bot Token / App Secret（`settings.json` 明文）

| 阶段 | 行为 | 代码路径 |
|------|------|---------|
| **输入** | Renderer 通过 `settings:update` IPC 发送 `BotChannelSettings.token` / `appSecret` | main.ts `settings:update` |
| **保存** | `mergeSettings()` → `FileSettingsStore.write()` 写入 `settings.json`（明文 JSON）| `settings-store.ts:76-85`, `settings.ts:441-507` |
| **读取** | `settingsStore.get()` 从 `settings.json` 读取完整 `AppSettings` | `settings-store.ts:65-74` |
| **使用** | `botRegistry.applySettings(settings.botChat)` → 各 bot bridge 消费 token | `main.ts:2722` |
| **展示** | `maskAppSettings()` 将 `token` / `appSecret` 替换为 `SENSITIVE_PLACEHOLDER`；`settings:get` 返回已 mask 结果 | `settings-ipc-helpers.ts:78-94`, `main.ts:2490` |
| **删除** | `settings:update` 中 patch token 为空字符串 → `mergeSettings` 空值覆盖 → `normalizeBotChannel` 合并 | `settings.ts:456-461`, `settings.ts:719-752` |

### 4.4 Proxy Password（`settings.json` 明文）

| 阶段 | 行为 | 代码路径 |
|------|------|---------|
| **输入** | Renderer 通过 `settings:update` IPC 发送 `network.proxy.password` | `main.ts:2491` |
| **保存** | `mergeSettings()` → 明文写入 `settings.json` | `settings-store.ts:76-85` |
| **读取** | `settingsStore.get()` → `toContractNetworkSettings()` 转换 | `main.ts:2500` |
| **使用** | `setActiveProxy(network.proxy)` 在 `applySettingsRuntimeEffects()` 中 | `main.ts:2717-2718` |
| **展示** | `maskAppSettings()` 将 `password` 替换为 `SENSITIVE_PLACEHOLDER` | `settings-ipc-helpers.ts:73-76` |
| **删除** | Patch password 为空 | 同 bot token 路径 |

### 4.5 OpenGateway Token（`settings.json` 明文）

| 阶段 | 行为 | 代码路径 |
|------|------|---------|
| **输入** | `settings:update` → `openGateway.token` | `main.ts:2491` |
| **保存** | 明文写入 `settings.json` | 同上 |
| **读取** | `settingsStore.get()` → `openGateway.sync()` | `main.ts:2725` |
| **使用** | `OpenGatewayService` 在 HTTP header 中使用 token 鉴权 | `open-gateway.ts` |
| **展示** | `maskAppSettings()` → `SENSITIVE_PLACEHOLDER` | `settings-ipc-helpers.ts:95-100` |
| **删除** | Patch token 为空 | normalize 阶段空 token 截断至 `''`（`settings.ts:708-710`） |

### 4.6 Tavily API Key（`settings.json` 明文）

| 阶段 | 行为 | 代码路径 |
|------|------|---------|
| **输入** | `settings:update` → `webSearch.providers.tavily.apiKey`；优先级：draft test key > 环境变量 `TAVILY_API_KEY` / `MAKA_TAVILY_API_KEY` > settings | `web-search/credentials.ts:18-40` |
| **保存** | 明文写入 `settings.json` | `settings-store.ts:219-224` |
| **读取** | `settingsStore.get()` → `resolveTavilyApiKey()` 优先级解析 | `web-search/credentials.ts` |
| **使用** | `queryTavily({apiKey})` 直接传给 Tavily HTTP endpoint | `main.ts:2164` |
| **展示** | maskAppSettings 中**永远不 reveal**，仅返回 `credentialSource`（env/saved/none）| `settings-ipc-helpers.ts:105-114` |

---

## 5. risk_matrix

| 风险 | 严重程度 | 影响范围 | 现状 | 证据 |
|------|---------|---------|------|------|
| **明文 settings.json 存储 5 类 Secret** | **高** | Bot Token (7平台), Proxy Password, Gateway Token, Tavily API Key, Feishu App Secret | `settings.json` 为普通 JSON 文件，无加密无权限控制；任何进程可读 | `settings-store.ts:219-224` 直接 `writeFile(path, JSON.stringify(settings))` |
| **settings.json 无文件权限控制** | **中** | 所有 settings 中的 Secret | `write()` 未调用 `chmod()`，文件权限继承父目录 | `settings-store.ts:219-224`，对比 OAuth services 的 `chmod(0o600)` |
| **Renderer 内存中 Secret 残留** | **低-中** | Bot Token, Proxy Password, Gateway Token | `maskAppSettings()` 已有效防护；但 `shouldReveal` 在用户修改同一字段时 reveal 明文值，renderer 短暂持有 | `settings-ipc-helpers.ts:129-131`, `settings-ipc-helpers.test.ts:55-72` |
| **IPC 返回路径 Secret 泄露** | **低** | 所有 Settings 中的 Secret | `settings:get` 始终返回 mask 后的 settings；Tavily API key 永不 reveal | `main.ts:2490`, `settings-ipc-helpers.ts:66-116` |
| **日志 / trace / error message 泄露** | **低** | LLM 调用错误消息 | `redactSecrets()` 在 error message 展示前过滤 Bearer token、key 模式 | `settings-ipc-helpers.ts:149`, `main.ts:1333` |
| **safeStorage 不可用** | **中** | LLM API Key 写入/读取 | `credential-store.ts:86` 抛 Error；OAuth services 同样抛 Error | `credential-store.ts:85-87`, `claude-subscription-service.ts:621-622` |
| **原子写入失败残留 temp 文件** | **极低** | credentials.json, settings.json | 使用 `temp → rename` 原子写入；rename 失败时 temp 残留但下次写入会覆盖 | `credential-store.ts:108-111`, `settings-store.ts:221-223` |
| **OAuth token 文件损坏后自动删除** | **低** | 各 OAuth Service | 解密失败时 best-effort `unlink()` 防 stuck-corrupt 状态 | `claude-subscription-service.ts:658-661` |
| **Bot token 在 readiness 更新时频写 settings.json** | **低** | settings.json 含明文 bot token | `onStatusChange` handler 在 `degraded`/`operational` 时写 `settingsStore.update()` | `main.ts:512-551` |
| **ConnectionStore 不含 Secret 但 `CreateConnectionInput` 携带 apiKey** | **低** | Connection create IPC | `apiKey` 仅在 handler 中提取并写入 credentialStore，不进入 connection 持久化 | `main.ts:2342-2344`, `llm-connections.ts:57-76` |
| **Claude Subscription Cloaked Request 携带设备指纹** | **信息** | Claude OAuth send path | `deviceId` + `accountUuid` + `sessionId` 嵌入 `metadata.user_id` JSON | `cloaked-request.ts:203-215` |

---

## 6. migration_plan

### 6.1 现状对比

| 字段 | 当前存储 | 当前加密 | 目标 |
|------|---------|---------|------|
| LLM API Key | `credentialStore` (safeStorage) | ✅ | 保持 |
| OAuth Token | 各 OAuth Service 独立文件 (safeStorage) | ✅ | 保持 |
| Bot Token | `settings.json` 明文 | ❌ | 迁移到 `credentialStore` |
| Feishu App Secret | `settings.json` 明文 | ❌ | 迁移到 `credentialStore` |
| Proxy Password | `settings.json` 明文 | ❌ | 迁移到 `credentialStore` |
| OpenGateway Token | `settings.json` 明文 | ❌ | 迁移到 `credentialStore` |
| Tavily API Key | `settings.json` 明文 | ❌ | 迁移到 `credentialStore` |

### 6.2 迁移方案

#### Phase 1：扩展 `credential-store` 支持新 Secret 类型

1. 扩展 `CredentialKind` 枚举：
   - 新增 `'bot_token'`, `'app_secret'`, `'proxy_password'`, `'gateway_token'`, `'tavily_api_key'`

2. 新增便捷方法：
   - `getBotToken(provider: BotProvider)`, `setBotToken(provider, token)`
   - `getProxyPassword()`, `setProxyPassword(password)`
   - `getGatewayToken()`, `setGatewayToken(token)`
   - `getTavilyApiKey()`, `setTavilyApiKey(key)`

#### Phase 2：兼容旧 `settings.json` 的读取路径

3. **读时优先 credentialStore，fallback 到 settings.json**：
   - 在 `resolveConnectionSecret()` 级别引入统一的 Secret resolver
   - 当在 settings.json 中发现明文 Secret 时，**自动迁移到 credentialStore 并清除 settings.json 中的明文**

4. **迁移触发时机**：
   - 应用启动时（`app.whenReady()` 之后，在 `settingsStore.get()` 之后）
   - 任意 `settings:update` 时，对更新字段检查是否需要迁移

5. **迁移原子性**：
   - 先写 `credentialStore.setSecret()` 成功
   - 再 `settingsStore.update()` 清除对应明文字段
   - 如果第一步成功第二步失败，下次启动时重试（credentialStore 中有值，settings.json 中也有值，优先 credentialStore）

#### Phase 3：关闭明文写入路径

6. **修改 `settings-ipc-helpers.ts`**：
   - `maskAppSettings()` 已正确处理 — 返回 `SENSITIVE_PLACEHOLDER`
   - `preserveSensitivePlaceholders()` 已正确处理 — 将 placeholder 替换为存储值
   - 在最终 `settingsStore.update()` 之前，从 patch 中**移除所有敏感字段**，改为写入 credentialStore

7. **修改 `FileSettingsStore.write()`**：
   - 在 `write()` 中增加一层 sanitize，确保写入 JSON 时所有 Secret 字段为空字符串（防御性编程，防止调用方忘记清理）

#### Phase 4：清理

8. **移除 `BotChannelSettings` 中的 `token`/`appSecret` 字段**（类型层面）：
   - 所有消费方改为通过 `resolveConnectionSecret()` / `credentialStore.getBotToken()` 读取

9. **数据迁移后清理旧 settings.json**：
   - 在确认所有 Secret 已成功写入 credentialStore 后，清理 settings.json 中的残余明文

### 6.3 兼容性策略

- 旧版本 settings.json 中的明文 Secret 在启动迁移后自动升级
- 迁移失败不阻塞应用启动（见 Phase 2 第5条的原子性策略）
- `SENSITIVE_PLACEHOLDER` 机制保持不变 — renderer 永远只看到 "••••••••"
- `credentialStore` 的文件格式（`{values: Record<string, string>}`）保持向后兼容

---

## 7. tests

### 7.1 现有测试覆盖

| 测试文件 | 覆盖范围 |
|---------|---------|
| `apps/desktop/src/main/__tests__/settings-ipc-helpers.test.ts` (178 行) | mask/preserve placeholder、reveal 逻辑、bot test error redaction、Tavily API key 永不 reveal |
| `packages/core/src/__tests__/settings.test.ts` (734 行) | bot readiness state 安全（F1/F3 coerce）、open gateway normalize、webSearch credential versioning、theme palette fail-closed、allowedUserIds normalize |
| `apps/desktop/src/main/__tests__/web-search-credentials.test.ts` (41 行) | Tavily API key 环境变量优先级、saved/fallback 逻辑、credentialSource 一致性 |
| `packages/storage/src/__tests__/settings-store-onboarding.test.ts` | onboarding milestone upsert/clear |
| `packages/storage/src/__tests__/settings-store-usage.test.ts` | usage stats from session files |

### 7.2 测试覆盖的关键安全断言

1. **IPC 边界 Mask**（`settings-ipc-helpers.test.ts:14-28`）：
   - 验证 `proxy.password`, `botToken`, `appSecret`, `gatewayToken` 均被替换为 `SENSITIVE_PLACEHOLDER`
   - 验证原始 settings 对象不被修改（`maskAppSettings` 返回新对象）

2. **Tavily API Key 永不 Reveal**（`settings-ipc-helpers.test.ts:74-85`）：
   - 即使 patch 显式提供真实 apiKey，IPC 返回的 apiKey 仍为 `SENSITIVE_PLACEHOLDER`

3. **Placeholder 替换回真实值**（`settings-ipc-helpers.test.ts:87-116`）：
   - `preserveSensitivePlaceholders` 将 `SENSITIVE_PLACEHOLDER` 替换回 current 中的存储值

4. **Bot Test Error Redaction**（`settings-ipc-helpers.test.ts:131-139`）：
   - `toSettingsTestResult` 结果中不含 `sk-live-xxx` 等真实 key 模式

5. **WebSearch Credential Versioning**（`settings.test.ts:496-605`）：
   - 修改 apiKey 时 credentialVersion +1、status → 'untested'、checkedAt 清除
   - stale credentialVersion 的 test 结果被忽略

6. **Bot Readiness Coerce**（`settings.test.ts:73-203`）：
   - F1: `credentials_valid` + token 清除 → downgrade 到 `scaffolded`
   - F3: mergeSettings 清除 token 后 normalize → readiness 正确 downgrade
   - F3b: coerce 只降级不升级

### 7.3 建议补充的安全测试

1. **credential-store 单元测试**（当前缺失）：
   - `safeStorage.encryptString` + `decryptString` 往返一致性
   - `isEncryptionAvailable() === false` 时 `setSecret` 抛 Error
   - 队列串行化：并发 set/get 不会导致数据丢失
   - ENOENT 时 `getSecret` 返回 null
   - 原子写入失败时 temp 文件不干扰后续读取

2. **OAuth Service token 持久化测试**（当前缺失）：
   - token 文件 chmod 0o600 生效
   - 解密失败自动 unlink 损坏文件
   - `getAccountState()` 返回的快照不含 token 字段

3. **settings.json 中 Secret 迁移后的回退测试**（迁移后）：
   - settings.json 中旧字段仍存在时优先 credentialStore
   - 迁移失败不阻断启动

4. **IPC payload scan 测试**（建议自动化）：
   - `settings:get` 返回值扫描：所有敏感字段应为 `SENSITIVE_PLACEHOLDER` 或空字符串
   - `connections:list` 返回值扫描：不应含 apiKey
   - 所有 OAuth subscription IPC 返回值扫描：不应含 `access_token` / `refresh_token`

5. **Error message 扫描测试**：
   - `errorMessage()` / `generalizedErrorMessage` 输出中不应含 Bearer token 模式
   - `console.warn`/`console.log` 在非 dev 模式下不应输出 token

---

## 附录：IPC Channel 与 Secret 流向速查表

| IPC Channel | 发送方 | Secret 是否经过 IPC | 防护措施 |
|------------|--------|-------------------|---------|
| `settings:get` | main → renderer | 否（已 mask） | `maskAppSettings()` |
| `settings:update` | renderer → main | 是（patch 中可能含明文） | `preserveSensitivePlaceholders()` + 主进程消费后不返回 |
| `connections:list` | main → renderer | 否 | `LlmConnection` 不含 apiKey 字段 |
| `connections:create` | renderer → main | 是（apiKey 单次传输） | 立即写入 credentialStore，不返回 |
| `connections:update` | renderer → main | 是（apiKey 可能含明文） | 同上 |
| `connections:test` | main → renderer | 否 | apiKey 在 main 进程内消费 |
| `claude-subscription:get-account-state` | main → renderer | 否 | `SubscriptionAccountState` 不含 token |
| `codex-subscription:get-account-state` | main → renderer | 否 | `CodexAccountStateSnapshot` 不含 token |
| `web-search:query` | renderer → main | 是（draftKey 可选） | 主进程解析后不返回 key |
| `web-search:test` | renderer → main | 是（draftKey 可选） | 同上 |
