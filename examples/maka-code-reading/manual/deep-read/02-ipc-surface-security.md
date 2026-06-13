# Maka IPC Surface Security 精读报告

> 基线：`335220a`
> 深度档位：maintainer
> 范围：`apps/desktop/src/main/main.ts` + `apps/desktop/src/preload/preload.ts` +
> 配套 guard/credential/settings-ipc-helpers + `__tests__/`

---

## scope

本报告覆盖 Maka Electron 桌面应用中 `main` 进程与 `renderer` 进程之间的
**全部 IPC handler**（通过 `ipcMain.handle` 注册）及其在 `preload.ts` 中的
暴露面（通过 `contextBridge.exposeInMainWorld`）。分析对象包括：

- 70+ 条双向参数传递通道。
- 10+ 条 event-push 通道（`webContents.send`）。
- 每条通道的输入校验、副作用类别、凭据泄漏风险和返回脱敏状态。

---

## problem

Electron 的 `contextBridge` + `contextIsolation: true` + `sandbox: true`
已将 renderer 隔离在受限沙箱中，但 **IPC handler 本身是 renderer 可触达的
唯一主进程代码入口**。renderer 中的 XSS（如被污染的 markdown 渲染、内联
脚本注入）可以通过 `window.maka.*` API 发送任意参数的 IPC 调用。如果 handler：
1. 不做参数类型/范围校验；
2. 直接透传 renderer 输入到文件系统、网络请求、shell.openPath、进程 spawn；
3. 在错误消息中返回原始凭据；
4. 在返回 payload 中包含明文 API Key / OAuth Token；

则 renderer 的任何代码执行漏洞都可能升级为主进程的本地文件读写、网络外联、
凭据窃取或系统命令执行。

---

## source_evidence

### 核心文件

| 文件 | 行数 | 角色 |
|---|---|---|
| `main.ts` | 3443 | 全部 IPC 注册 + 运行时依赖 |
| `preload.ts` | 745 | `contextBridge` 暴露的 API surface |
| `settings-ipc-helpers.ts` | 163 | 敏感字段脱敏 / 占位符替换 / 错误消息包装 |
| `open-path-guard.ts` | 71 | `app:openPath` / `skills:open` 的路径穿越守卫 |
| `credential-store.ts` | 123 | `safeStorage` 加密的凭据持久化层 |

### IPC 架构约束（`main.ts:1008-1017`）

```ts
webPreferences: {
  preload: join(import.meta.dirname, '..', 'preload', 'preload.cjs'),
  contextIsolation: true,    // window.maka via contextBridge only
  nodeIntegration: false,    // no require in renderer
  sandbox: true,             // preload runs in the renderer sandbox
  webSecurity: true,         // enforce CSP / same-origin policy
  allowRunningInsecureContent: false,
}
```

renderer 与主进程的唯一通信路径是 `ipcRenderer.invoke` →
`ipcMain.handle`。**这是所有安全分析的总边界。**

### 凭据协议（`main.ts:229` / `credential-store.ts`）

```ts
const credentialStore = createSafeStorageCredentialStore(workspaceRoot);
```

所有 API key / OAuth token 通过 Electron `safeStorage` 加密后存储在
`workspaceRoot/credentials.json` 中，读写通过内存队列序列化（`withQueue`），
写入时使用原子 rename（`write temp → rename`）。

### 测试守卫（`ipc-surface-contract.test.ts`）

```ts
// 自动扫描 main.ts 和 preload.ts，确保每一条 ipcRenderer.invoke
// 都有对应的 ipcMain.handle，反之亦然。
const mainChannels = extractChannels(main, /ipcMain\.handle\(\s*['"]([^'"]+)['"]/g);
const preloadChannels = extractChannels(preload, /ipcRenderer\.invoke\(\s*['"]([^'"]+)['"]/g);
```

---

## flow_analysis

### 请求方向（renderer → main）

```
renderer (sandbox)
  → window.maka.sessions.send(sessionId, command)
    → contextBridge proxy
      → ipcRenderer.invoke('sessions:send', sessionId, command)
══════════ IPC boundary ══════════
        → ipcMain.handle('sessions:send', handler)
          → normalizeSessionSendCommand(command)  ← 第一层校验
          → ensureSessionCanSend(sessionId)        ← 会话状态校验
          → validateRendererAttachments(...)       ← 附件审批
          → runtime.sendMessage(...)               ← 进入运行时
```

### 推送方向（main → renderer）

```
main process
  → mainWindow?.webContents.send('sessions:changed', event)
══════════ IPC boundary ══════════
    → ipcRenderer.on('sessions:changed', listener)
      → contextBridge proxy
        → renderer handler
```

### 凭据流

```
1. renderer 提交 apiKey (字符串)
2. main: credentialStore.setSecret(slug, 'api_key', apiKey)
   → safeStorage.encryptString(value) → base64 → JSON → rename
3. renderer 请求凭据检查
   → connections:hasSecret → resolveConnectionSecret
     → credentialStore.get(slug, 'apiKey')
       → 读文件 → base64 解码 → safeStorage.decryptString
     → 返回 boolean（仅 true/false，从不返回明文）
```

**关键设计：所有 OAuth 订阅服务（Claude/Codex/Cursor/Antigravity）的 token
完全不出主进程。** OAuth 服务的 `getAccessTokenInternal()` 仅在
`resolveConnectionSecret` (`main.ts:400-409`) 中被调用，该方法
从未通过 IPC 暴露。

---

## handler_matrix

以下列出 **top 30 high-signal handlers**，按风险类别排序。

### 图例

- **输入校验**：`✅` 强校验 | `⚠️` 部分校验 | `❌` 弱/无校验
- **副作用**：`F` 文件系统 | `N` 网络 | `C` 凭据操作 | `S` shell/系统 | `W` workspace 写入
- **返回脱敏**：`✅` 已脱敏 | `❌` 可能含敏感数据 | `N/A` 不适用

| # | Channel | 输入参数 | 输入校验 | 副作用 | 返回脱敏 | 关键源码行 |
|---|---|---|---|---|---|---|
| 1 | `connections:create` | `CreateConnectionInput` (含 apiKey) | ⚠️ baseUrl 经 `normalizeConnectionBaseUrl` 校验；OAuth 连接走 `PROVIDER_DEFAULTS`；apiKey 未做长度/格式校验 | C (safeStorage 加密存储), W (connection store) | ⚠️ 返回不含 apiKey，但 `connection` 对象可能含 providerType → 可推断身份 | `main.ts:2323-2347` |
| 2 | `connections:update` | `slug`, `UpdateConnectionInput` (含 apiKey) | ⚠️ 同上 | C (更新/删除凭据), W (connection store) | ⚠️ 同上 | `main.ts:2348-2373` |
| 3 | `connections:test` | `slug`, `{ model? }` | ⚠️ slug 无格式校验，直接传入 `connectionStore.get` | N (testConnection 发起外部 API 调用) | ⚠️ `errorMessage` 可能含服务端返回的原始错误（虽然 main 已做 `redactSecrets`） | `main.ts:2379-2396` |
| 4 | `connections:fetchModels` | `slug` | ❌ slug 无格式校验 | N (fetchProviderModels 发起外部 HTTP 调用) | ⚠️ 返回模型列表，不含凭据 | `main.ts:2397-2423` |
| 5 | `connections:hasSecret` | `slug` | ❌ slug 无格式校验 | C (读取 safeStorage，但仅返回 boolean) | ✅ 只返回 boolean | `main.ts:2424-2426` |
| 6 | `connections:delete` | `slug` | ❌ slug 无格式校验 | C (删除凭据), W (删除连接) | N/A | `main.ts:2374-2378` |
| 7 | `sessions:send` | `sessionId`, `command` | ⚠️ command 经 `normalizeSessionSendCommand` 校验；附件经 `validateRendererAttachments` 校验 | N (LLM API 调用), W (store 写入消息) | ✅ 不返回敏感数据 | `main.ts:2220-2238` |
| 8 | `sessions:create` | `Partial<CreateSessionInput>` | ⚠️ cwd 未做路径校验（直接传入 `process.cwd()` 作为默认值），requestedSlug/model 仅做 existence check | W (createSession) | ✅ 返回 Summary 不含凭据 | `main.ts:1740-1773` |
| 9 | `sessions:setModel` | `sessionId`, `{ llmConnectionSlug, model }` | ⚠️ input 经 `normalizeSessionModelSelection` 校验（非空字符串 trim），但 slug/model 为任意字符串 | W (updateSession) | ⚠️ 返回 session summary | `main.ts:2281-2305` |
| 10 | `settings:get` | 无 | N/A | 无 | ✅ `maskAppSettings` 脱敏（apiKey → placeholder, proxy password → placeholder） | `main.ts:2490` |
| 11 | `settings:update` | `UpdateAppSettingsInput` | ⚠️ patch 经 `preserveSensitivePlaceholders` 处理（placeholder → 保留旧值），但未对任意字段做深度类型校验 | W (settings store), C (bot/network 运行时应用) | ✅ `buildSettingsUpdateResult` → `maskAppSettings` 脱敏 | `main.ts:2491-2496` |
| 12 | `settings:testNetworkProxy` | `TestProxyInput` | ⚠️ password placeholder 处理正确；proxy host/port 未做额外格式校验 | N (testProxyConnection 发起外部连接) | ⚠️ 返回 IP/Country 信息；password 脱敏为 SENSITIVE_PLACEHOLDER | `main.ts:2498-2528` |
| 13 | `settings:testBotChannel` | `BotProvider` (string) | ❌ provider 无枚举校验（任意字符串直接传入 `testRuntimeBotChannel`） | N (bot 平台 API 调用) | ✅ `toSettingsTestResult` 经 `redactSecrets` + 消息模板 | `main.ts:2529-2549` |
| 14 | `web-search:query` | `{ query, limit, provider, apiKey }` | ✅ provider 经 `isWebSearchProvider` 校验；query 经 `normalizeWebSearchQuery` 校验；limit 经 `normalizeWebSearchLimit` 校验 | N (queryTavily API 调用) | ✅ 返回搜索结果不含 apiKey | `main.ts:2136-2166` |
| 15 | `web-search:test` | `{ provider, apiKey }` | ✅ 同上 | N (queryTavily 测试调用) | ✅ 返回测试结果不含 apiKey | `main.ts:2168-2186` |
| 16 | `search:thread` | `request: unknown` | ⚠️ 传入 `runThreadSearch` 由 helper 做 object-shape guard，但类型是 `unknown` | 无网络 (本地搜索) | ✅ 返回搜索结果 | `main.ts:2188-2210` |
| 17 | `app:openPath` | `key` (string) | ✅ `resolveOpenPath` 做完整校验：key 白名单 + realpath 解析 + 路径穿越检测 (`isInsideOrSamePath`) | S (shell.openPath 打开系统文件管理器) | ✅ 只返回 `ok: true` 或 `reason` | `main.ts:1374-1380` |
| 18 | `app:openArtifactPath` | `artifactId` (string) | ✅ artifactStore.get 校验存在性 + resolveArtifactPath 做前缀 + symlink-escape check | S (shell.showItemInFolder 在 Finder 中定位文件) | ✅ 只返回 artifact name | `main.ts:1531-1565` |
| 19 | `app:saveArtifactAs` | `artifactId` (string) | ✅ 同上 | F (copyFile 写入用户选择的目标路径), S (dialog.showSaveDialog) | ✅ 只返回 saved name | `main.ts:1566-1592` |
| 20 | `context:importTextFile` | 无（通过 dialog.open） | ✅ 文件选择由 OS dialog 控制；读取后经 `readTextFilesForPromptImport` 做 size/binary check | F (readFile 读取用户选择的文件) | ✅ 返回 prompt 文本 | `main.ts:1441-1473` |
| 21 | `context:importDroppedTextFiles` | `files: Array<{name,size,type?,text}>` | ⚠️ 仅做 shape 校验（name/string, size/number, text/string），无 size 上限，无内容校验 | F (将 renderer 传来的文本作为 prompt 片段) | ✅ 返回 prompt 文本 | `main.ts:1474-1497` |
| 22 | `context:importFolderOutline` | 无（通过 dialog.open） | ✅ 文件夹选择由 OS dialog 控制 | F (readFolderOutlinesForPromptImport 遍历目录) | ✅ 返回 folder outline | `main.ts:1498-1523` |
| 23 | `memory:save` | `content: unknown` | ⚠️ 仅做 `typeof content !== 'string'` 检查，无长度上限 | W (localMemory.save 写入 MEMORY.md) | ⚠️ 返回 LocalMemoryState 含完整 content | `main.ts:1382-1385` |
| 24 | `memory:openFile` | 无 | ✅ localMemory.resolveFileForOpen 做路径守卫 | S (shell.openPath 调用系统编辑器) | ✅ 只返回 ok/message | `main.ts:1404-1409` |
| 25 | `skills:open` | `id`, `target` | ✅ resolveSkillOpenPath 做路径守卫 | S (shell.openPath) | ✅ 只返回 ok/error | `main.ts:1670-1676` |
| 26 | `skills:createStarter` | 无 | ✅ createStarterSkill 做 blocked_path / already_exists 校验 | W (写入 SKILL.md) | ⚠️ 返回 skill 信息含 path | `main.ts:1669` |
| 27 | `visualSmoke:capture` | `{ scenario, variant }` | ✅ fixture 模式开关 + `sanitizeSegment` 对 scenario/variant 做严格白名单（仅 `[a-zA-Z0-9._-]`，≤128 char，排除 `.`/`..`） | F (writeFile 写入 PNG 截图) | ✅ 返回 path（仅测试环境可用） | `main.ts:1608-1649` |
| 28 | `claude-subscription:complete-authorization` | `authRequestId`, `pasted` | ⚠️ authRequestId 校验 `typeof === 'string'`；pasted 传递给 Service 内部的 JSON.parse，无额外检验 | C (完成 OAuth 流程，存入 token), N (可能发起 token 刷新) | ✅ 返回 `SubscriptionActionResult` 不含 raw token | `main.ts:1828-1842` |
| 29 | `daily-review:saveMarkdownToFile` | `{ markdown, defaultName }` | ✅ `saveMarkdownViaDialog` 做 1MB cap + 200 char filename cap + 路径分隔符替换 + OS save dialog | F (writeFile 写入用户选择的 .md) | ✅ 只返回 ok/canceled | `main.ts:2639-2643` |
| 30 | `chat:saveConversationToFile` | `{ markdown, defaultName }` | ✅ 同上 | F (writeFile) | ✅ 同上 | `main.ts:2650-2654` |

### 补充 handler（非 top 30 但具备安全信号）

| Channel | 风险点 |
|---|---|
| `onboarding:setMilestone` | id/status 为 `unknown`，透传给 Service，Service 内部校验 |
| `onboarding:clearMilestone` | 同上 |
| `quickChat:start` | `input: unknown` 透传给 `handleQuickChatStart` |
| `plans:create` | `input: unknown` 透传给 `planReminderStore.create` |
| `plans:update` | `patch: unknown` 同上 |
| `sessions:rename` | `name: string` 无长度上限，直接存进 store |
| `sessions:setFlagged` | 无额外校验 |
| `settings:bots:wechat:fetchQrcode` | 无输入参数，HTTP 调用在 main 侧完成 ✅ |
| `settings:bots:wechat:pollQrcodeStatus` | `qrToken` 校验 `typeof === 'string' && non-empty` ✅ |
| `usage:pricing:put` | `pricing: unknown` 经 `normalizePricingConfig` 严格校验 ✅ |
| `usage:pricing:reset` | `modelKey: unknown` 经 `normalizePricingModelKey` 校验 ✅ |

---

## risk_matrix

以下按风险从高到低列出 **top 10 需要加固的 handler**。

### R1: `connections:create` — apiKey 无长度/字符校验

**风险等级：HIGH**
**源码位置：`main.ts:2323-2347`**

```ts
ipcMain.handle('connections:create', async (_event, input: CreateConnectionInput) => {
  const normalizedInput = normalizeCreateConnectionInput(input);
  const connection = await connectionStore.create(normalizedInput);
  if (normalizedInput.apiKey) {
    await credentialStore.setSecret(connection.slug, 'api_key', normalizedInput.apiKey);
  }
```

**问题：**
1. `apiKey` 为任意字符串，无长度上限（可能写入数 MB 到 `credentials.json`）。
2. `apiKey` 无字符白名单校验（可包含 NUL 字节、控制字符）。
3. `slug` 字段由 store 内部生成，但 `name` 无长度/格式限制。
4. `baseUrl` 虽经 `normalizeConnectionBaseUrl` 校验，但该函数允许 localhost/内网地址（Ollama/LM Studio 场景），可能被利用做 SSRF 尝试。

**建议：**
- `apiKey` 增加 4096 字符上限 + 禁止 NUL/控制字符。
- `name` 增加 256 字符上限。
- 对本地/内网 baseUrl 增加可配置白名单（非 Ollama/LM Studio 场景禁止 `127.0.0.1`）。

---

### R2: `connections:update` — 相同的 apiKey 校验缺失

**风险等级：HIGH**
**源码位置：`main.ts:2348-2373`**

```ts
if (normalizedPatch.apiKey !== undefined) {
  if (normalizedPatch.apiKey) await credentialStore.setSecret(slug, 'api_key', normalizedPatch.apiKey);
  else await credentialStore.deleteSecret(slug, 'api_key');
}
```

**问题：** 与 R1 相同。`deleteSecret` 路径仅检查 `!apiKey`（空字符串），未防 renderer 传 `' '` 单空格 → 仍会写入 safeStorage。

**建议：** 统一 R1 的校验逻辑，空/空白字符串走 delete 分支。

---

### R3: `connections:fetchModels` — slug 无校验 + 外部 HTTP 调用

**风险等级：MEDIUM-HIGH**
**源码位置：`main.ts:2397-2423`**

```ts
ipcMain.handle('connections:fetchModels', async (_event, slug: string) => {
  const connection = await connectionStore.get(slug);
  if (!connection) throw new Error(`找不到模型连接：${slug}`);
  const apiKey = await resolveConnectionSecret(slug);
  const models = await fetchProviderModels(connection, apiKey ?? '');
```

**问题：**
1. `slug` 直接从 renderer 传入 `connectionStore.get()`，无格式校验。
2. 如果 renderer 被 XSS，攻击者可通过遍历 slug 枚举所有连接（含已删除的）。
3. `fetchProviderModels` 发起实际 HTTP 请求到外部 API，使用从 safeStorage 解密的 apiKey。

**建议：**
- 增加 slug 格式校验（`/^[a-zA-Z0-9._-]+$/`，长度 ≤128）。
- 对高频调用增加速率限制（同一 slug 30s 内最多调用 1 次）。

---

### R4: `connections:test` — 外部 HTTP 调用 + 错误消息可能泄漏信息

**风险等级：MEDIUM-HIGH**
**源码位置：`main.ts:2379-2396`**

```ts
const result = await testConnection(connection, apiKey ?? '', opts?.model);
await connectionStore.update(slug, connectionTestStatusPatch(result));
```

**问题：**
1. 与 R3 相同，slug 无校验。
2. `opts?.model` 无格式校验，直接传入 `testConnection`。
3. 连接测试失败时 `errorMessage` 可能包含上游 API 返回的原始错误信息。
4. `connectionTestStatusPatch` 将结果写入 store → 下次 `settings:get` 可能将脱敏后的状态暴露给 renderer。

**建议：**
- slug + model 格式校验。
- 对测试结果做 `redactSecrets` 后再写入 store（`connectionTestStatusPatch` 当前未做脱敏）。

---

### R5: `settings:testBotChannel` — provider 无白名单

**风险等级：MEDIUM**
**源码位置：`main.ts:2529-2549`**

```ts
ipcMain.handle('settings:testBotChannel', async (_event, provider: BotProvider) => {
  const settings = await settingsStore.get();
  const result = await testRuntimeBotChannel(provider, settings.botChat.channels[provider]);
```

**问题：**
1. `BotProvider` 类型虽为联合类型，但 IPC 层面运行时无校验（TypeScript 类型在运行时不存在）。
2. 恶意 renderer 可传 `'../../../etc/passwd'` → `settings.botChat.channels[provider]` 为 `undefined`，可能导致运行时异常或未定义行为。
3. `testRuntimeBotChannel` 内部经 `botTestErrorMessage` 做 `redactSecrets`，路径本身已安全 ✅。

**建议：**
- 增加 provider 运行时枚举校验（已知值：`telegram`, `feishu`, `qq`, `wechat`, `dingtalk`, `whatsapp` 等）。

---

### R6: `memory:save` — 无内容长度上限

**风险等级：MEDIUM**
**源码位置：`main.ts:1382-1385`**

```ts
ipcMain.handle('memory:save', async (_event, content: unknown): Promise<LocalMemoryState> => {
  if (typeof content !== 'string') return localMemory.getState();
  return localMemory.save(content);
});
```

**问题：**
1. `content` 仅在类型层面校验为 `string`，无长度上限。
2. 恶意 renderer 可提交 100MB 字符串 → `localMemory.save` 写入 MEMORY.md → 磁盘 I/O 拒绝服务 + 下次启动时 system prompt 构建可能内存溢出。
3. `LocalMemoryService.save` 内部虽然有写入逻辑，但未见明确的长度 cap。

**建议：**
- 增加 256KB 上限（`maxToolPromptChar` 为 15KB，256KB 已足够）。
- 返回 `LocalMemoryState` 时脱敏 content（当前直接返回完整 content）。

---

### R7: `sessions:create` — cwd 无路径校验

**风险等级：MEDIUM**
**源码位置：`main.ts:1740-1773`**

```ts
const cwd = input?.cwd ?? process.cwd();
// ...
const session = await runtime.createSession({
  cwd,
  backend: 'ai-sdk',
```

**问题：**
1. `input.cwd` 为任意字符串，直接传入 `runtime.createSession` → 作为 agent 的工作目录。
2. 虽然 `PermissionEngine` 通过 `isInsideOrSamePath` 限制 agent 的文件访问，但 cwd 自身未被校验。
3. 恶意 renderer 可设置 `cwd: "/"` 或 `cwd: "/etc"` → agent 可能读取敏感系统文件（如果权限引擎存在绕过）。

**建议：**
- `cwd` 经 `realpath` + `isInsideOrSamePath` 校验，仅限于已知安全目录。
- 或至少校验 `cwd` 不是系统目录（不在 `/etc`, `/proc`, `/sys`, `/var` 下）。

---

### R8: `claude-subscription:complete-authorization` — pasted 参数无校验

**风险等级：MEDIUM**
**源码位置：`main.ts:1828-1842`**

```ts
ipcMain.handle(
  'claude-subscription:complete-authorization',
  async (_event, authRequestId: unknown, pasted: unknown) => {
    if (!isSubscriptionExperimentalEnabled()) return experimentalDisabledResponse;
    if (typeof authRequestId !== 'string') { /* ... */ }
    const result = await claudeSubscription.completeAuthorization(authRequestId, pasted);
```

**问题：**
1. `pasted` 参数类型为 `unknown`，直接透传给 `claudeSubscription.completeAuthorization`。
2. Service 层虽然做 JSON.parse，但如果 renderer 被 XSS，攻击者可能注入大量数据。
3. OAuth 完成流程涉及 token exchange → credential 写入，高安全敏感度操作。

**建议：**
- `pasted` 增加 `typeof === 'string'` + 长度上限（OAuth redirect URL ≤ 8KB）。
- Codex/Cursor/Antigravity 的对应 handler 同样缺少此校验。

---

### R9: `context:importDroppedTextFiles` — 无 size 上限 + 无文件类型白名单

**风险等级：MEDIUM-LOW**
**源码位置：`main.ts:1474-1497`**

```ts
const safePayloads: DroppedTextFilePayload[] = Array.isArray(payloads)
  ? payloads.map((payload) => {
      const value = payload && typeof payload === 'object' ? (payload as Record<string, unknown>) : {};
      return {
        name: typeof value.name === 'string' ? value.name : '',
        size: typeof value.size === 'number' ? value.size : 0,
        type: typeof value.type === 'string' ? value.type : '',
        text: typeof value.text === 'string' ? value.text : '',
      };
    })
  : [];
const imported = readDroppedTextFilesForPromptImport(safePayloads);
```

**问题：**
1. 虽然 `readDroppedTextFilesForPromptImport` 内部有 `too-large` / `binary` 检测，但检查发生在 main 进程读取后。
2. `files` 数组长度无上限（攻击者可传 10000 个文件条目）。
3. 不经过 OS dialog → renderer 可伪造文件名/文件内容。

**建议：**
- 增加 `files.length ≤ 5` 上限（与 readTextFilesForPromptImport 的 `too-many-files` 一致）。
- 增加 `text.length ≤ 1MB`（单个文件）+ `total ≤ 5MB`（所有文件）限制。

---

### R10: `visualSmoke:capture` — 写入路径虽安全但需确认 release 不可达

**风险等级：MEDIUM-LOW**
**源码位置：`main.ts:1608-1649`**

```ts
if (!visualSmokeFixture) return { ok: false, reason: 'not_in_fixture_mode' };
const scenario = sanitizeSegment(input?.scenario);
const variant = sanitizeSegment(input?.variant);
// ...
const filePath = join(dir, `${variant}.png`);
await writeFile(filePath, image.toPNG());
```

**问题：**
1. 写入路径在 `workspaceRoot/screenshots/<scenario>/<variant>.png`，但 `workspaceRoot` 的计算在启动时固定（`join(app.getPath('userData'), 'workspaces', ...)`）。
2. `sanitizeSegment` 白名单为 `[a-zA-Z0-9._-]` + 排除 `.`/`..` + `≤128`，安全 ✅。
3. `writeFile` 直接覆写已有文件，无竞态条件。

**建议：**
- 当前防护充分。建议在 `app.isPackaged` 环境下增加额外断言（`!app.isPackaged || visualSmokeFixture !== null`），确保生产环境绝对不可达。

---

## next_actions

### 紧急（P0 — 本周内）

1. **`connections:create` / `connections:update`**: 对 `apiKey` 增加 4096 字符上限 + NUL/控制字符过滤。这是最高风险的入口，因为任何 XSS 都可能通过 `window.maka.connections.create({ apiKey: malicious_payload })` 写入任意数据。

2. **`connections:fetchModels`**: 增加 `slug` 格式白名单校验（`/^[a-zA-Z0-9._-]+$/`），防止 renderer 遍历所有存储的连接。

3. **`connections:test`**: 错误消息增加 `redactSecrets` 后写入 store。

### 高优（P1 — 下个迭代）

4. **`memory:save`**: 增加 256KB 内容长度上限。

5. **`sessions:create`**: 增加 `cwd` 路径安全校验（`realpath` + 不在系统目录下）。

6. **`context:importDroppedTextFiles`**: 增加 `files.length ≤ 5` + `text.length ≤ 1MB` 上限。

7. **OAuth complete-authorization (4个)**: 对 `pasted` 参数增加 `string` 类型 + 长度上限。

### 中优（P2 — 后续规划）

8. **`settings:testBotChannel`**: provider 增加枚举白名单运行时校验。

9. **Slug 统一校验**: 抽取 `normalizeSlug(slug: unknown)` 公共函数，在 `connections:delete`, `connections:hasSecret`, `connections:test`, `connections:fetchModels`, `connections:update` 五个 handler 中统一使用。

10. **IPC fuzz testing**: 基于 `ipc-surface-contract.test.ts` 的 channel 列表，构建自动 fuzz 测试框架，对每个 handler 传入：
    - `null` / `undefined`
    - 空字符串
    - 超长字符串（1MB+）
    - NUL 字节
    - 路径穿越 payload (`../../../etc/passwd`)
    - JSON 注入 payload
    确保每个 handler fail-closed 而非 throw 或 crash。

11. **Telemetry 记录**: 对所有凭据操作（create/update/delete/test）增加审计日志（脱敏后），方便事后追溯异常操作。

### 确认安全（无需改动）

- `safeStorage` 凭据加密存储 ✅
- `maskAppSettings` 完整覆盖 proxy.password / bot.token / gateway.token / webSearch.apiKey ✅
- `SENSITIVE_PLACEHOLDER` 双向协议（renderer 传回 placeholder → main 替换为旧值）✅
- `preserveSensitivePlaceholders` 正确保留所有敏感字段 ✅
- `resolveOpenPath` realpath + `isInsideOrSamePath` 路径穿越守卫 ✅
- `sanitizeSegment` + `visualSmoke:capture` fixture-only 守卫 ✅
- `saveMarkdownViaDialog` 1MB cap + 路径分隔符过滤 ✅
- `contextBridge` + `contextIsolation` + `sandbox` 沙箱 ✅
- `window-open-handler` + `will-navigate` 外部链接守卫 ✅
- OAuth 4 服务的 token 完全不出主进程 ✅
