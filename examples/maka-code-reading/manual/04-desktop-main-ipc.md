# Maka Desktop Main / IPC / Preload 粗读报告

> 阅读基线：`335220a`
> 深度档位：`architecture`
> 范围：`apps/desktop/src/main/` + `apps/desktop/src/preload/`

## 文件概要

| 文件 | 行数 | 职责 |
|---|---|---|
| `main.ts` | 3443 | 应用入口、IPC 注册、启动编排 |
| `preload.ts` | 745 | `contextBridge` 暴露给 renderer 的 API |
| `credential-store.ts` | 123 | Electron `safeStorage` 加密凭据持久化 |
| `settings-ipc-helpers.ts` | 163 | 敏感字段掩码、占位符保留、设置更新粘合 |
| `project-context.ts` | 72 | Git root 探测、项目路径解析 |
| `session-environment-prompt.ts` | 37 | 会话环境 prompt 片段构建 |
| `workspace-instructions.ts` | 245 | AGENTS.md / CLAUDE.md / GEMINI.md 的读取/创建/路径校验 |
| `rive-cli.ts` | 414 | rive CLI 子进程封装（命令构建、spawn、输出清洗） |
| `rive-workflow-tool.ts` | 309 | Agent 工具：RiveWorkflow → 调用 rive CLI |
| `explore-agent-tool.ts` | 1151 | Agent 工具：只读本地代码探索 worker |
| `office-document-tool.ts` | 635 | Agent 工具：officecli 读写 Office 文档 |
| `open-gateway.ts` | 1451 | 本地 HTTP SSE 网关（127.0.0.1:3939） |
| `local-memory-service.ts` | 435 | MEMORY.md 本地记忆读写/备份/恢复 |
| `onboarding-service.ts` | 232 | 新手引导状态推导 |
| `open-path-guard.ts` | 71 | 四种路径（workspace/skills/memory/project）的安全打开 |

Maka Desktop 的 main 进程共注册 **128 个 `ipcMain.handle` 通道**，全部通过 `preload.ts` 的 `contextBridge.exposeInMainWorld('maka', {...})` 暴露到 renderer。

---

## 1. problem — desktop main 层承担什么

Maka Desktop 是一个基于 Electron 的 AI 编程助手桌面客户端。desktop main 层的核心定位是 **双重边界**：

### 安全边界
- **凭据隔离**：API key、OAuth token 等敏感字符串只存在 main 进程的 Electron `safeStorage` 加密存储中（`credential-store.ts`），renderer 进程永远看不到明文。
- **文件系统边界**：renderer 不能直接读写文件。所有文件操作（打开/读取/写入/创建/导入）都通过 IPC 由 main 执行，并经过 `realpath` + `isInside` 路径 containment 校验。
- **网络边界**：AI provider 请求（Anthropic/OpenAI/Codex 等）由 main 进程中的 `SessionManager` + `BackendRegistry` 发起，renderer 不持有 provider connection 的 raw socket。
- **子进程边界**：`rive-cli.ts`（rive CLI）和 `office-document-tool.ts`（officecli）的 subprocess spawn 在 main 进程中完成，renderer 不可直接 spawn。

### OS 集成边界
- **窗口管理**：`BrowserWindow` 创建/大小/最大化/TrafficLight 按钮由 main 控制。
- **系统对话框**：`dialog.showSaveDialog` / `dialog.showOpenDialog` / `dialog.showMessageBox` 由 main 调用。
- **外部链接**：`shell.openExternal` 用于打开浏览器 URL，`shell.openPath` 用于在 Finder/资源管理器中打开文件/文件夹，均在 main 侧执行并受 `external-link-guard.ts` 保护。
- **本地 HTTP 网关**：`open-gateway.ts` 在 `127.0.0.1:3939` 上启动 `node:http` Server，提供 token-protected REST + SSE API 供本地工具链调用。

---

## 2. why_hard — 为什么这一层难做

### 2.1 Electron IPC 的信任模型
- renderer 进程运行在沙箱中（`sandbox: true`，`contextIsolation: true`，`nodeIntegration: false`），但仍可通过 preload 的 `contextBridge` 调用任意 IPC channel。
- **恶意/BUG renderer 可以伪造任意 IPC 入参**（类型可为 `unknown`），因此 main 对每个 handler 的输入必须重新做形状校验，不可信任 preload 的类型声明。
- main.ts 中的 handler 入参大量使用 `unknown` 然后手动做 `typeof` / shape guard，体现了这一设计理念。

### 2.2 safeStorage 的可用性和队列
- `safeStorage` 依赖操作系统密钥链（macOS Keychain / Windows DPAPI / Linux libsecret）。如果密钥链不可用，`isEncryptionAvailable()` 返回 false，此时凭据写操作会 throw。
- 磁盘层面使用 atomic rename（写临时文件 → rename），防止崩溃导致 `credentials.json` 损坏。
- 内部维护一个 `queue: Promise<void>` 实现串行化读写，避免并发写入竞态。

### 2.3 本地文件访问的路径 containment
- 所有路径操作都必须经受 `realpath` 解析 + `isInside(root, target)` 检查。
- 符号链接检测：symlink 文件/目录被显式拒绝（如 `office-document-tool.ts:406-409`，`explore-agent-tool.ts:594-597`）。
- 多种来源的输入：
  - Agent tool 参数（`office-document-tool`、`explore-agent-tool`、`RiveWorkflow` 接收 agent 生成的路径参数）
  - 用户对话框选择（`context:importTextFile`、`context:importFolderOutline`）
  - 内部推导路径（`local-memory-service.ts` 基于 `workspaceRoot` 推导）

### 2.4 Workspace open/path guard 的复杂度
- `open-path-guard.ts` 允许 renderer 要求打开 4 种路径：`workspace`、`skills`、`memory`、`project`。
- `project` 路径来自 `project-context.ts` 的 Git root 探测（沿目录树向上查找 `.git`，直到文件系统根），可能指向用户 Home 目录之外的任何地方。
- renderer 不传绝对路径，只传 `key` 字符串枚举值，main 自己计算真实路径。

### 2.5 External services 的副作用
- **Rive CLI**（`rive-cli.ts`）：spawn 子进程、传递环境变量、执行任意 `rive workflow` 命令，超时 1h。需要防注入（`params` key 限制 `/^[A-Za-z0-9_.-]+$/`）、输出大小限制（2MB）。
- **officecli**（`office-document-tool.ts`）：读写 `.docx/.xlsx/.pptx` 文件，`execFile` 超时 15s，输出截断 60K chars。读操作 `permissionRequired: false`（不弹权限），写操作 `permissionRequired: true`（弹权限）。
- **OpenGateway**（`open-gateway.ts`）：在本地回环地址上监听 HTTP，Bearer token 鉴权。暴露 session list/messages/events/search，**可能被本地其他进程扫描到**。
- **OAuth subscription**（4 个 service）：Claude、Codex、Cursor、Antigravity 的 OAuth 流程都在 main 进程中完成，token 永不过 IPC 边界。`openAuthUrl` 只接受 `authRequestId`（renderer 不传 URL）。

### 2.6 Rive tool bridging 的风险
- `rive-workflow-tool.ts` 将 agent 参数直接转化为 CLI 参数传给 rive 二进制。
- rive CLI 可以执行 `workflow_run`、`scheduler_resume` 等具有外部副作用的命令。
- 虽然参数经过 zod schema 校验，但 binary path 可以来自环境变量 `MAKA_RIVE_BIN` / `RIVE_BIN`，agent 可控 `opencodeBin` / `codexBin` 路径（zod `max(2000)`）。

---

## 3. design_approach — 权限分层

### 三层架构

```
┌──────────────────────────────────────────────┐
│  Renderer (sandbox, no Node)                 │
│  · React UI 组件                             │
│  · window.maka.sessions.*() 等 API 调用       │
│  · 不能：读写文件、spawn 进程、访问密钥链     │
│  · 不能：直接发起 AI provider HTTP 请求       │
└──────────────┬───────────────────────────────┘
               │ contextBridge
               │ ipcRenderer.invoke / ipcRenderer.on
┌──────────────▼───────────────────────────────┐
│  Preload (sandbox, contextIsolation)         │
│  · 定义 window.maka API shape (TypeScript)   │
│  · 仅转发 IPC，不包含业务逻辑                │
│  · 发送侧类型约束（编译时），运行时不校验     │
└──────────────┬───────────────────────────────┘
               │ Electron IPC (序列化边界)
┌──────────────▼───────────────────────────────┐
│  Main Process (full Node + Electron)         │
│  · ipcMain.handle — 128 个通道               │
│  · SessionManager / BackendRegistry (AI 后端) │
│  · CredentialStore (safeStorage 加密凭据)     │
│  · 文件系统访问 (经 path containment 校验)    │
│  · Subprocess spawn (rive CLI / officecli)   │
│  · OpenGateway (本地 HTTP 服务器)            │
│  · OAuth subscription services               │
│  · Bot registry (Telegram/WeChat 桥接)       │
│  · 外部链接拦截 (setWindowOpenHandler)       │
└──────────────────────────────────────────────┘
```

### 哪些能力只在 main

| 能力 | 文件 | 为什么 renderer 不能做 |
|---|---|---|
| API key / OAuth token 存储 | `credential-store.ts` | `safeStorage` 只在 main process 可用；renderer 无 Node.js 权限读密钥链 |
| 文件系统读写 | `main.ts:563-598`, `workspace-instructions.ts` | renderer `sandbox: true` + `contextIsolation: true` 无 `fs` 模块；且 main 可以强制 path containment |
| AI provider HTTP 请求 | `main.ts:665-697` (AiSdkBackend) | API key 从 credentialStore 解析后注入，renderer 看不到 key |
| OAuth login flow | `oauth/` 目录 | OAuth PKCE 流程在 main 执行，`shell.openExternal` 也在 main |
| Subprocess spawn | `rive-cli.ts`, `office-document-tool.ts` | `node:child_process` 在 renderer 不可用 |
| 系统对话框 | `dialog.showSaveDialog` / `showOpenDialog` | Electron `dialog` 只在 main 可用 |
| 外部链接打开 | `shell.openExternal` / `shell.openPath` | 受 `external-link-guard.ts` 控制 |
| 本地 HTTP server | `open-gateway.ts` | `node:http` 在 renderer 不可用 |
| 窗口控制 | `BrowserWindow` | 只有 main 可以创建/操作窗口 |
| Bot 桥接 | `main.ts:2777-2961` | Bot token 存在 settings 中，网络请求在 main 执行 |

### 哪些 API 暴露给 renderer（preload.ts 定义的命名空间）

```typescript
window.maka = {
  sessions,        // 18 个方法：list/create/send/stop/readMessages/listTurns/retryTurn/...
  connections,     // 9 个方法：list/create/update/delete/test/fetchModels/hasSecret/...
  onboarding,      // 3 个方法：getSnapshot/setMilestone/clearMilestone
  quickChat,       // 1 个方法：start
  permissions,     // 1 个方法：getSnapshot
  capabilities,    // 1 个方法：getSnapshot
  health,          // 1 个方法：getSnapshot
  memory,          // 11 个方法：getState/save/reset/restore*/open*/setEnabled/...
  workspaceInstructions, // 3 个方法：getState/openFile/createFile
  context,         // 3 个方法：importTextFile/importDroppedTextFiles/importFolderOutline
  search,          // 1 个方法：thread
  gateway,         // 2 个方法：status/subscribeStatusChanges
  claudeSubscription, // 8 个方法：OAuth 流程（token 永不过 IPC）
  codexSubscription,  // 7 个方法
  cursorSubscription, // 7 个方法
  antigravitySubscription, // 7 个方法
  plans,           // 8 个方法 + 2 个事件订阅
  settings,        // 6 个方法 + bots 子命名空间（6 个方法） + bots.wechat 子命名空间（2 个方法）
  usage,           // 5 个方法：summary/buckets/logs/listPricing/putPricing/resetPricing
  dailyReview,     // 2 个方法：day/saveMarkdownToFile
  webSearch,       // 2 个方法：query/test
  appWindow,       // 2 个方法
  app,             // 4 个方法：info/openPath/openArtifactPath/saveArtifactAs
  visualSmoke,     // 2 个方法
  artifacts,       // 5 个方法 + 1 个事件订阅
  skills,          // 3 个方法：list/createStarter/open
}
```

---

## 4. code_walkthrough — 关键文件走读

### 4.1 `main.ts` (3443 行)

**启动流程** (`app.whenReady()`):
1. `seedVisualSmokeFixture` 或 `ensureBootstrapConnection()` — 从环境变量配置初始连接
2. 读取 settings → 设置代理 → 加载 telemetry → 恢复中断会话 → 应用 bot 设置 → 同步 openGateway → 创建窗口 → 刷新计划提醒

**`registerIpc()` 函数** (约 1350-2700 行) 按领域分组注册 handler：

- **sessions**: 创建/发送/停止/分支/归档/重命名/设置权限模式/设置模型/删除
  - `sessions:send` 是核心热路径：校验 `SessionCommand` → `ensureSessionCanSend` → 校验附件 → `runtime.sendMessage` → `streamEvents`
- **connections**: CRUD + test + fetchModels + hasSecret
  - `connections:create` / `connections:update` 对 `baseUrl` 做 `normalizeConnectionBaseUrl` 归一化，OAuth 连接强制使用 provider 固定端点
  - API key 保存走 `credentialStore.setSecret`
- **memory**: 委托给 `LocalMemoryService`，每个 handler 重新校验入参类型（如 `kind` 只允许 `'save' | 'reset' | 'restore'`）
- **web-search**: Tavily 搜索，API key 从 `settingsStore` 读取 `resolveTavilyApiKey`，renderer 不传 key（除非测试 draft key）
- **OAuth subscriptions**: 4 个 namespace，每个 handler 重新检查 `isExperimentalEnabled()`，返回 `experimental_disabled` 信封 fail-closed
- **plans**: 计划提醒的 CRUD + 调度定时器 + bot 投递

**关键安全函数**:
- `resolveConnectionSecret(slug)` (行 400-409): 对于 OAuth 连接调用 `getAccessTokenInternal()`，对于 API key 连接调用 `credentialStore.getSecret()`
- `isInsideOrSamePath(root, target)` (行 659-663): 路径 containment 检查
- `sanitizeSegment(value)` (行 650-657): 限定 `[a-zA-Z0-9._-]`，防止 screenshot capture path 注入
- `resolveToolArtifactSourcePath` (行 628-641): `realpath` 双解析 + containment

### 4.2 `preload.ts` (745 行)

`contextBridge.exposeInMainWorld('maka', { ... })` 定义了完整的 renderer API surface。

每个方法签名都包含完整的 TypeScript 类型（`Promise<SessionSummary[]>` 等），提供编译时类型安全。但 **运行时没有任何校验** — 数据经过 Electron IPC 序列化后，main side 收到的是 `unknown`。

关键设计：
- 事件订阅采用 `ipcRenderer.on` → 返回 `unsubscribe` 函数
- OAuth subscription API 注释明确标注 "NEVER returns raw OAuth credentials"
- `openAuthUrl` 只接受 `authRequestId`（string），不接受 URL

### 4.3 `credential-store.ts` (123 行)

使用 Electron `safeStorage.encryptString/decryptString`，以 `credentials.json` 文件存储。

- 键格式：`${slug}:apiKey` 或 `${slug}:oauthToken`
- 写操作：先写临时文件 `credentials.json.<pid>.<timestamp>.tmp`，再 `rename` 实现 atomic write
- 读操作：base64 decode → `safeStorage.decryptString`
- 队列串行化：`withQueue` 保证读写不并发

### 4.4 `open-gateway.ts` (1451 行)

在 `127.0.0.1` 上启动 HTTP server，提供 REST API：

| 端点 | 方法 | 说明 |
|---|---|---|
| `/health` | GET | 无需 token |
| `/v1/capabilities` | GET | 能力列表 |
| `/v1/state` | GET | 综合状态（不含 payload） |
| `/v1/sessions` | GET | 会话列表 |
| `/v1/sessions/{id}/messages` | GET/POST | 读消息/发消息 |
| `/v1/sessions/{id}/events` | GET | SSE 事件流 |
| `/v1/search/thread` | GET | 本地线程搜索 |
| `/v1/incidents` | GET | 错误/中止事件聚合 |

安全措施：
- Bearer token 鉴权（`Authorization: Bearer <token>`）
- 消息正文、事件 payload 不包含在 state 端点中（`includesPayloads: false`）
- 请求追踪 (`X-Maka-Request-Id` header)
- 消息发送限制：body ≤ 16KB，text ≤ 8000 chars
- SSE heartbeat 15s

### 4.5 `local-memory-service.ts` (435 行)

管理 `MEMORY.md` 文件的完整生命周期：

- **路径安全**：所有路径操作通过 `realpath` + `isInsideOrSamePath` 校验
- **隐身模式**：`privacyContext.incognitoActive` 时拒绝读写
- **agent 读取开关**：`agentReadEnabled` 默认 false，需用户显式开启
- **备份机制**：每次 save/reset/restore 前创建 `.bak` / `.reset.bak` / `.restore.bak` 文件
- **文件权限**：`chmod(0o600)` / `chmod(0o700)`
- **safeMode**：contents 包含危险指令时进入安全模式，不向 agent 暴露条目

### 4.6 `rive-cli.ts` + `rive-workflow-tool.ts`

- `rive-cli.ts`:** 构建 CLI 参数、spawn 子进程、输出 redaction（需防 API key 泄漏到 stdout）
- `rive-workflow-tool.ts`:** Agent 工具的 zod schema 定义（`action` enum、各字段 max length、`params` key 校验、`workers` max 20、`timeoutMs` max 1h）
- 标志：`permissionRequired: true`，`categoryHint: 'custom_tool'`

### 4.7 `explore-agent-tool.ts` (1151 行)

只读本地代码探索 worker：

- **模式**: `read_only`
- **路径安全**: `realpath` + `isInside` + 符号链接拒绝（`lstat` 检查 `isSymbolicLink()`）
- **预算控制**: 文件发现上限 250、文件读取上限 80、匹配数上限 120、单文件 512KB、总字节 2MB
- **敏感文件跳过**: `.env`、`credentials.json`、`*.pem` 等不会读取内容，只报告计数
- **被忽略目录**: `.git`、`node_modules`、`dist`、`build` 等
- 标志：`permissionRequired: true`，`categoryHint: 'subagent'`

### 4.8 `office-document-tool.ts` (635 行)

通过 `officecli` 二进制读写 Office 文档：

- **只读操作** (`OfficeDocument`): `help`、`view`、`get`、`query`、`validate` — `permissionRequired: false`
- **写操作** (`OfficeDocumentEdit`): `create`、`add`、`set`、`remove` — `permissionRequired: true`，`categoryHint: 'file_write'`
- **路径安全**: 仅相对路径 → `realpath` → `isInside`、符号链接拒绝、扩展名 `.docx/.xlsx/.pptx` 白名单
- **编辑参数**: `target` selector 限制 500 chars、`props` 最多 24 项、key 校验 `/^[A-Za-z0-9_.:-]{1,80}$/`
- **输出安全**: workspace root 替换为 `<workspace>`、secrets redaction、60K chars 截断

### 4.9 `onboarding-service.ts` (232 行)

推导 `OnboardingState`：从 connections 列表 + secret 存在性 + 会话数量 + milestones 综合计算。

- secret lookup 并行执行（不序列化，性能优化）
- 错误不泄漏到 renderer（`console.warn` dev 日志，treat-as-missing）
- milestone ID 通过 `ONBOARDING_MILESTONE_IDS` 枚举校验

---

## 5. flows — 关键链路

### 5.1 App Boot 链路

```
app.whenReady()
  ├─ seedVisualSmokeFixture() | ensureBootstrapConnection()
  ├─ 读取 settings → setActiveProxy() → telemetryRepo.load()
  ├─ 重建 pricing lookup
  ├─ recoverInterruptedSessionsOnStartup()
  ├─ botRegistry.applySettings(settings.botChat)
  ├─ openGateway.sync(settings.openGateway)  // 启动本地 HTTP server
  ├─ createWindow()
  │   ├─ mkdir(workspaceRoot)
  │   ├─ ensureBundledOfficeSkills()
  │   ├─ installApplicationMenu()
  │   ├─ 恢复窗口 bounds (含多显示器 clamp)
  │   ├─ new BrowserWindow({
  │   │     preload, contextIsolation: true,
  │   │     nodeIntegration: false, sandbox: true
  │   │   })
  │   ├─ setWindowOpenHandler → 拦截 external URL
  │   ├─ will-navigate → 阻止 file:// 导航
  │   ├─ 注入 dragover/drop 阻止脚本
  │   ├─ loadURL(dev) | loadFile(packaged)
  │   └─ 注册 resize/move/close bounds 保存
  └─ refreshPlanReminderTimers()
```

**为什么 renderer 不能做这些**：
1. `BrowserWindow` 创建/操作只在 main process
2. `workspaceRoot` 路径需要 `app.getPath('userData')`，在 renderer 不可用
3. `openGateway.sync()` 启动 `node:http` server，renderer 没有 Node 权限
4. `botRegistry.applySettings()` 初始化 Telegram/WeChat bridge，凭据在 settings 中

### 5.2 Session Send 链路

```
renderer: window.maka.sessions.send(sessionId, command)
  → preload: ipcRenderer.invoke('sessions:send', sessionId, command)
  → main: ipcMain.handle('sessions:send', async (event, sessionId, command) => {
      1. normalizeSessionSendCommand(command)  // 校验 command 形状
      2. ensureSessionCanSend(sessionId)
         ├─ readHeader(sessionId)
         ├─ ensureSessionCanSendOrRebind()  // 检查连接可用性
         │   ├─ getConnection(slug)
         │   ├─ resolveConnectionSecret(slug)  // 从 credentialStore 取出 key
         │   └─ requireReadyConnection()  // 检查连接状态
         └─ 如果 rebound, emit sessions:changed
      3. validateRendererAttachments(attachments, { senderId, approvals })
         // 校验附件来源（必须是 approved 的本地路径）
      4. runtime.sendMessage(sessionId, { turnId, text, attachments })
         → BackendRegistry → AiSdkBackend → AI SDK → fetch(url, { Authorization: Bearer <apiKey> })
      5. streamEvents(sessionId, iterator, turnId)
         → 转发 session events 到 renderer (sessions:event:*) + openGateway
    })
```

**关键安全点**:
- API key 从 `credentialStore` 解析（第 3 步），不经过 IPC
- 附件必须经过 `attachment-approval` 审批，防止 renderer 伪造文件路径

### 5.3 Provider Credential Save / Test 链路

```
renderer: window.maka.connections.create({ name, providerType, apiKey, baseUrl })
  → main: ipcMain.handle('connections:create', async (_, input) => {
      1. normalizeCreateConnectionInput(input)
         ├─ OAuth provider → 忽略 renderer 传的 baseUrl
         ├─ 非 OAuth → normalizeConnectionBaseUrl(input.baseUrl)
      2. connectionStore.create(normalizedInput)
      3. if (apiKey) credentialStore.setSecret(slug, 'api_key', apiKey)
         ├─ safeStorage.encryptString(apiKey) → base64
         ├─ 写到临时文件 → atomic rename → credentials.json
      4. emitConnectionListChanged()
    })

renderer: window.maka.connections.test(slug)
  → main: resolveConnectionSecret(slug)
    ├─ OAuth 连接 → claudeSubscription.getAccessTokenInternal()
    ├─ API key 连接 → credentialStore.getSecret(slug, 'api_key')
    │   safeStorage.decryptString(Buffer.from(base64, 'base64'))
  → testConnection(connection, apiKey)
  → connectionStore.update(slug, connectionTestStatusPatch(result))
```

**为什么 renderer 不能做**：
1. `safeStorage` API 只在 main process 可用
2. API key 在 renderer 停留将导致 XSS/value interception 风险
3. 网络测试请求需要真实 key，renderer 不应访问

### 5.4 Open Path Guard 链路

```
renderer: window.maka.app.openPath('workspace' | 'skills' | 'memory' | 'project')
  → main: ipcMain.handle('app:openPath', async (_, key) => {
      1. resolveOpenPath({ key, workspaceRoot, projectRoot })
         ├─ 校验 key ∈ {workspace, skills, memory, project}
         ├─ 计算候选路径 (project 走 project-context.ts 的 Git root 探测)
         ├─ realpath 解析
         ├─ isInsideOrSamePath(root, target)  ← containment 检查
         └─ stat → isDirectory 校验
      2. shell.openPath(resolved.path)  // 在 Finder/Explorer 中打开
    })
```

**路径 containment 算法** (出现在多个文件中):
```typescript
function isInsideOrSamePath(root: string, target: string): boolean {
  if (target === root) return true;
  const rel = relative(root, target);
  return rel !== '' &&
    !rel.startsWith('..') &&
    rel !== '..' &&
    !rel.includes(`..${sep}`) &&
    !rel.startsWith(sep);
}
```

### 5.5 Rive Workflow Tool 链路

```
Agent → RiveWorkflow tool invocation
  → main (tool impl): buildRiveWorkflowTool().impl(args, { cwd, abortSignal, emitOutput })
    1. buildRiveCommand(args)
       ├─ zod schema 校验 (action enum, string max lengths, params key regex)
       ├─ switch (action) → 构建 CLI args 数组
       └─ 参数注入: --param key=value, --worker, --runner, etc.
    2. runRiveCli(args, { cwd, env, abortSignal, timeoutMs })
       ├─ resolveRiveBinary() → MAKA_RIVE_BIN || RIVE_BIN || 'rive'
       ├─ spawn(bin, args, { cwd, env, detached })
       ├─ stdout/stderr 累积 (2MB cap)
       ├─ redactRiveText() 实时输出
       ├─ SIGTERM → SIGKILL 超时终止
       └─ 解析 JSON envelope
    3. 返回 RiveWorkflowToolResult
       ├─ ok: true → successResult (投影 protocol/display 字段)
       └─ ok: false → failureResult (reason + message)
```

**外部副作用标记** ⚠️：
- rive CLI 可能执行 `workflow_run`（启动多 agent 工作流）
- rive CLI 可能执行 `workflow_import`（import 工作流包）
- rive CLI 可能执行 `work_retry`（重试失败的 work node）
- rive CLI 可能执行 `scheduler_resume`（恢复调度器）

### 5.6 Local Memory / Search 链路

```
renderer: window.maka.memory.save(content)
  → main: ipcMain.handle('memory:save', (_, content) => {
      1. typeof content !== 'string' → 静默返回 getState()
      2. localMemory.save(content)
         ├─ 检查隐身模式 → 拒绝
         ├─ redactSecrets(content)  // 从内容中移除疑似凭据的文本
         ├─ parseLocalMemoryMarkdown()  // 解析结构
         │   └─ safeMode 检测 → 返回 safeMode 状态
         ├─ 创建备份 (copyFile → .bak)
         ├─ 写入临时文件 → rename → chmod 0o600
         └─ 返回 getState()
    })

renderer: window.maka.search.thread(request)
  → main: runThreadSearch(request, { listSessions, readMessages, getPrivacyContext })
    └─ 本地线程搜索，query body 不写入 telemetry
```

### 5.7 Office Document Tool 链路

```
Agent → OfficeDocument tool invocation (读)
  → office-document-tool: runOfficeDocumentOperation()
    1. normalizeOperation → 只允许 help/view/get/query/validate
    2. resolveOfficeDocumentPath()
       ├─ 拒绝绝对路径、包含 \0 的路径
       ├─ workspaceRoot = realpath(cwd)
       ├─ resolve → isInside 检查
       ├─ extname 必须是 .docx/.xlsx/.pptx
       ├─ lstat → 拒绝符号链接
       └─ realpath(abs) → 再次 isInside 检查
    3. buildOfficeCliArgs → ['view'|'get'|'query'|'validate', path, ...]
    4. runOfficeCliOperation → execFile('officecli', args, { timeout: 15000, maxBuffer: 512KB })
    5. 输出 sanitize → 替换 workspaceRoot → redactSecrets → cap 60K chars

Agent → OfficeDocumentEdit tool invocation (写) ⚠️
  → 相同路径安全流程
  → normalizeEditOperation → 只允许 create/add/set/remove
  → props 白名单校验（key regex, value 类型, 最多 24 项）
  → permissionRequired: true（弹用户确认）
```

---

## 6. tests — 现有测试覆盖分析

### 6.1 直接相关的 main 层测试

| 测试文件 | 覆盖内容 |
|---|---|
| `ipc-surface-contract.test.ts` | 验证 main 的 `ipcMain.handle` 与 preload 的 `ipcRenderer.invoke` 通道一一对应 |
| `credential-store` ← 无独立测试文件 | **缺失**：safeStorage 的 mock 测试 |
| `settings-ipc-helpers.test.ts` | `maskAppSettings`、`preserveSensitivePlaceholders` 的敏感字段掩码逻辑 |
| `project-context` ← 无独立测试文件 | **缺失**：Git root 探测、project path 解析 |
| `explore-agent-tool.test.ts` | explorer 的路径 containment、预算控制、敏感文件跳过 |
| `office-document-tool.test.ts` | officecli 路径校验、参数构建、输出 redaction |
| `open-gateway.test.ts` | HTTP server 鉴权、端点响应、SSE 流 |
| `local-memory-service.test.ts` | MEMORY.md 读写/备份/恢复/隐身模式 |
| `onboarding-service.test.ts` | 状态推导、milestone CRUD |
| `rive-workflow-tool.test.ts` | Rive CLI 参数构建、错误处理 |
| `open-path-guard.test.ts` | 路径 containment、key 枚举校验 |
| `workspace-instructions.test.ts` | AGENTS.md 等文件的读写/创建/路径校验 |
| `quick-chat.test.ts` | Quick Chat 流程 |
| `connection-test-status.test.ts` | 连接测试状态更新 |
| `chat-readiness.test.ts` | send 前的连接可用性检查 |
| `attachment-approval.test.ts` | 附件审批逻辑 |
| `claude-subscription-ipc-boundary.test.ts` | Claude OAuth IPC 边界：token 不泄漏到 renderer |
| `claude-subscription-experimental-gate.test.ts` | 实验特性 flag 的 fail-closed 行为 |

### 6.2 测试缺失项

| 缺失的测试领域 | 风险等级 |
|---|---|
| `credential-store.ts` 的 safeStorage 加密/解密循环 | 高 — 凭据安全核心 |
| `main.ts` `registerIpc()` handler 的入参注入测试 | 高 — 128 个 handler，逐个校验入参解析 |
| `external-link-guard.ts` 的 URL 过滤测试 | 中 — 有测试文件但需审查覆盖度 |
| `main.ts` 的 `normalizeSessionSendCommand` 形状校验 | 中 — 核心热路径 |
| `open-gateway.ts` 的 token 伪造/重放攻击 | 高 — 本地端口暴露 |
| `rive-cli.ts` 的参数注入测试（env var 覆盖、binary path） | 高 — 子进程执行任意代码 |
| `office-document-tool.ts` 编辑路径的原子写入/竞态 | 中 — 写操作的外部副作用 |
| `main.ts` 的 `sessions:send` 附件路径伪造 | 高 — 读任意文件 |
| `main.ts` 的 App boot 恢复流程 (recoverInterruptedSessionsOnStartup) | 中 — 启动崩溃恢复 |
| IPC handler 的并发调用安全性 | 中 — credentialStore 有队列保护，但其他 handler 无 |

---

## 7. risks — 风险清单

### 7.1 IPC surface 扩张风险
- **128 个 `ipcMain.handle` 通道**，每个都需要独立维护输入校验
- preload 和 main 的通道配对由 `ipc-surface-contract.test.ts` 做静态正则检查，但 **不检查类型一致性**
- 新增 handler 无自动化安全审查机制

### 7.2 Path traversal 风险
- **多个实现各自的 `isInside` / `isInsideOrSamePath`**（`main.ts:659`、`workspace-instructions.ts:218`、`office-document-tool.ts:627`、`explore-agent-tool.ts:962`、`local-memory-service.ts:420`、`open-path-guard.ts:67`），逻辑不完全一致
  - `main.ts` 版本额外检查 `rel !== '..'` 和 `!rel.includes('..${sep}')`
  - `workspace-instructions.ts` 版本缺少 `!rel.startsWith(sep)` 检查
- **`project-context.ts` 的 `findGitRoot` 沿目录树向上查找**，可能穿过工作区边界到达用户 Home 目录之外的路径
- **`isInside` 仅基于相对路径字符串比较**，不会验证 inode/device id 是否一致

### 7.3 Credential exposure 风险
- `resolveConnectionSecret` 在主进程内存中持有 API key 明文，整个 `AiSdkBackend` 生命周期内保持可访问
- `redactSecrets` 只在日志/输出 redaction 使用，不影响内存中的 key
- `credential-store.ts` 的队列在 `catch(() => {})` 时会吞掉错误，可能导致写入失败被静默忽略

### 7.4 External link / open path 风险
- `shell.openExternal` 对 `isExternalUrl` 返回 true 的 URL 不加区分地打开（允许 `http:`/`https:`/`mailto:`）
- renderer 传入的 `authRequestId` 被 OAuth service 映射到预存 URL，但 **renderer 可以通过 `openAuthUrl` + 非预期 authRequestId 尝试触发任何预存 URL**
- `shell.showItemInFolder` (artifact "在 Finder 中打开") 不经过 openPath guard

### 7.5 Rive / Office tool side effects ⚠️
- **RiveWorkflow tool** (`rive-workflow-tool.ts`):
  - agent 可以通过 `workflow_run` 触发多 worker 代码生成/执行
  - `opencodeBin` / `codexBin` 参数允许 agent 指定二进制路径（zod `max(2000)` 限制弱）
  - env var `MAKA_RIVE_BIN` / `RIVE_BIN` 可覆盖 binary path
  - 超时最长达 1h（`timeoutMs` max `60 * 60 * 1000`）
- **OfficeDocumentEdit tool** (`office-document-tool.ts`):
  - `create` 操作创建新 Office 文件（文件系统副作用）
  - `add` / `set` / `remove` 修改现有文件（数据破坏风险）
  - 虽然 `permissionRequired: true`，但一旦用户批准，agent 可以继续调用
- **OpenGateway** (`open-gateway.ts`):
  - `/v1/sessions/{id}/messages` POST 接受外部消息（sendMessage 注入）
  - 本地 HTTP server 对回环地址开放，**同机的其他进程可以扫描到端口**

### 7.6 Bot integration 风险
- **Telegram/WeChat/Feishu 桥接**：bot token 存在 settings 中，通过 `maskSensitive` 掩码后传给 renderer，但 `main.ts` 中频繁出现 token 明文的 settings 读写
- Bot 收到的消息自动创建 session 并执行 AI（`permissionMode: 'explore'`）
- `botRecentSourceEventKeys` 去重 Map 会随着消息量增长无限增长（虽有 1000 上限）

---

## 8. next_questions — 下一轮精读建议

1. **所有 IPC handler 的入参 fuzzing 审计**：逐个检查 128 个 handler 对 `unknown` 入参的校验是否完备，特别关注 `JSON.parse` 的异常处理路径。

2. **六个 `isInside` 实现的差异审计**：对比 `main.ts`、`workspace-instructions.ts`、`office-document-tool.ts`、`explore-agent-tool.ts`、`local-memory-service.ts`、`open-path-guard.ts` 中的路径 containment 逻辑，找出潜在绕过路径（如 Windows `\\?\` prefix、macOS `/private/var` 别名等）。

3. **credential-store.ts 的故障模式分析**：`catch(() => {})` 静默吞错误 → 数据丢失窗口 → 极端情况下凭据写入失败但前端显示成功。

4. **OpenGateway 的攻击面审计**：SSE stream 连接的资源耗尽（`activeEventStreams` 无上限保护）、token bruteforce（无 rate limit）、路径遍历（`sessionStateMatch` 中的 `decodeURIComponent`）。

5. **Rive CLI 的 command injection 审计**：`params` key 虽然有 `/^[A-Za-z0-9_.-]+$/` 校验，但 value 通过 `String(value)` 转字符串后直接拼入 `key=value` → 如果 rive CLI 对 `=` 后的值做 shell 展开是否有影响需要确认。

6. **OAuth subscription service 的 token 生命周期**：refresh token 的存储位置、过期处理、并发 refresh 的竞态条件。

7. **内存中凭据的存活周期**：`resolveConnectionSecret` 返回的值在 `getReadyConnection` → `AiSdkBackend` 中使用，是否在请求结束后仍可被 GC 回收。

8. **Bot 消息队列的公平性**：`botConversationQueues` 用 `Map<platform:chatId, Promise>` 串行化每个对话，但没有全局 fairness 控制。

---

> 本报告由 Rive opencode-reader-b agent 自动生成，只读了 `apps/desktop/src/main/` 和 `apps/desktop/src/preload/` 下的关键源文件和测试文件，未对仓库做任何修改。
