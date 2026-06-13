# Maka 精读维护者指南

> 阅读基线：`335220a` | 深度档位：`maintainer` | 合成时间：2026-06-13
>
> 本报告汇总 10 个上游精读节点的核心发现，去重、排序、判定优先级后输出。

---

## executive_summary

1. **双层进程架构**：Maka 是以 Electron `contextIsolation + sandbox` 为安全基座的桌面 AI 代理。main 进程暴露 70+ IPC handler、renderer 通过 `contextBridge` 调用，之间再无其他通信路径。所有安全分析的起点是 `main.ts:1008-1017` 的 `webPreferences` 配置。

2. **3×11 权限矩阵是纵深防御的核心**：`packages/core/src/permission.ts` 的 `preToolUse()` 纯函数定义了 `explore/ask/execute` × 11 个 `ToolCategory` 的 allow/prompt/block 策略。`categorizeBash()` 6 层优先级分类（privileged → fs_destructive → git_destructive → shell_unsafe → safe prefix → default unsafe）是 Bash 命令安全分类的唯一入口。

3. **StreamWatchdog 并发 pause/resume 是唯一的真实并发竞态**：`StreamWatchdog` 使用布尔 flag 而非引用计数实现 pause/resume，在 Vercel AI SDK 并行 tool call 场景下可能导致无辜 tool call 被 timeout abort。这是 P0 级别的并发 bug。

4. **凭据存储不均衡，存在两种体系**：LLM API Key 和 OAuth Token 使用 `safeStorage` 加密存储；Bot Token（7 平台）、Proxy Password、Gateway Token、Feishu App Secret、Tavily API Key 以明文存储在 `settings.json` 中。IPC 边界已通过 `maskAppSettings()` 完成遮盖，但持久化层的 gap 是 P1 级别的设计债务。

5. **Bot Bridge 与 OpenGateway 构成两条外部边界**：Bot session 强制 `explore`（只读）模式是正确的纵深防御，但缺少入站速率限制、SSE 连接数上限和 Token 轮换机制。OpenGateway Token 明文存储于 `settings.json`，与报告 04 的凭据问题形成交叉打击面。

6. **JSONL 会话存储存在单点故障**：任一行的 `JSON.parse` 失败会导致整个 session 不可读并向上传播到 UI。`list()` 静默跳过损坏 session 且无用户提示。`schemaVersion` 保持为 1，从未写回——意味着每个 session 每次读都触发 migration 代码。

7. **9-Gate Memory 合约与运行时 MEMORY.md 之间存在架构断隙**：`validateMemoryWriteRequest()` 定义的 9 条隐私门禁在 `apps/` 目录下零调用方。`LocalMemoryService` 使用独立的源/状态枚举，完全不经过 9-Gate 验证器。2 个预留错误码（`embedding_disabled`、`quasi_memory_promotion_blocked`）从未被发出。

8. **遥测存在三个数据丢失窗口**：`recordLlmCall`/`recordToolInvocation` 使用 `queueMicrotask()`（fire-and-forget）；进程退出时未 flush 写入队列；ai-sdk `result.usage` 中的 `cacheInputTokens`/`cacheWriteInputTokens`/`reasoningTokens` 未被 `AiSdkBackend` 提取，导致 cache 定价体系和 reasoning token 计费完全无法生效。

9. **三种 `isInside` 变体未统一**：`workspace-instructions.ts`、`office-document-tool.ts`/`explore-agent-tool.ts` 中有三种不同的路径围栏实现。workspace-instructions 版本缺少 `!isAbsolute(rel)` 检查，在 Windows 跨驱动器场景下存在理论绕过窗口。所有调用方都先调了 `realpath`，实际风险有限但违反防御深度原则。

10. **外部工具注入面可接受**：Rive CLI 使用 `spawn` + `shell: false` + 参数正则校验，参数注入风险低。OfficeDocument 的 abort signal 集成缺失（`execFile` 不支持 `AbortSignal`）。ExploreAgent 是纯 Node.js 实现，不调用外部二进制。

11. **视觉烟雾测试基础设施分层合理但缺像素级回归**：`diff-screenshots.mjs` 只检查 PNG 是否存在/尺寸是否正确，不做 `pixelmatch` 内容对比。Command Palette、Toast、ErrorBoundary 无 fixture scenario 覆盖。`check-console.mjs` 和 `check-a11y.mjs` 是成本极低的 fast gate。

12. **权限引擎的 `prompt` 路径存在永久阻塞风险**：`wrapToolExecute` 中的 `await verdict.parked` 无超时机制——用户关闭电脑、renderer 进程崩溃、bot bridge 无响应时，session 会永久卡住。唯一的出口是 `endTurn('aborted')`。

13. **Bot 模式 permissionMode 未被强制约束**：Bot bridge 创建的 session 虽在 `main.ts:2876` 硬编码为 `explore`，但 `SessionManager.sendMessage()` 和 BotRegistry 层没有强制校验。如果调用链中某处绕过了硬编码，bot session 可能进入 `ask` 模式并永久阻塞。

14. **IPC handler 层存在集中式参数校验缺口**：5 个 handler 共享的 `slug` 参数没有统一的格式白名单。`connections:create` 的 `apiKey` 无长度/字符校验。`memory:save` 的 `content` 无长度上限。`sessions:create` 的 `cwd` 未做路径安全校验。

15. **测试基础设施覆盖良好的核心模块**：`permission.test.ts` (336行)、`permission-engine.test.ts` (367行)、`ai-sdk-backend.test.ts` (502行) 覆盖了权限引擎的核心路径。但并行 tool call + shared watchdog 竞态、wrapToolExecute 端到端 prompt→parked→allow→impl 路径、JSONL 损坏恢复等关键场景缺少测试。

---

## architecture_theses

### Thesis 1: 安全在渲染器边界，不在渲染器内部

Maka 的安全模型是 **"渲染器不可信"**。`contextIsolation: true` + `sandbox: true` + `nodeIntegration: false` 将渲染器隔离在受限沙箱中。所有 IPC handler（70+ 条）是渲染器可触达的唯一主进程入口。XSS 或渲染器代码执行漏洞的升级路径仅限于 IPC handler 的输入校验缺口——这意味着 **IPC handler 的输入校验是全局安全的唯一防线**，而非前端的 CSP 或 React 的 XSS 防御。

### Thesis 2: 权限矩阵是纯函数，但运行时状态是竞态的

`preToolUse()` 的纯函数设计（无副作用、无 I/O、输入→输出确定）是正确的架构决策。但 `PermissionEngine` 的运行时状态（`remembered` Set、`parked` Map）和 `StreamWatchdog` 的布尔 pause flag 引入了并发竞态。**纯函数层的正确性不能保证运行时层的正确性**——并发 pause/resume 是一个真实的例子。维护时应保持权限决策逻辑为纯函数，但运行时状态机的并发安全性需要独立的测试和验证。

### Thesis 3: 路径 containment 依赖 realpath，而 realpath 有平台差异

所有路径围栏函数的正确性依赖于 `realpath` 的一致性。macOS `/var` → `/private/var` 的 symlink、Windows 跨驱动器 `relative()` 返回绝对路径、APFS 大小写保留——这些平台行为差异意味着 **路径 containment 的正确性不能在单平台上验证**。三种 `isInside` 变体的存在和 workspace-instructions 版本缺少 `!isAbsolute(rel)` 是统一化不足的症状。

### Thesis 4: JSONL 作为会话格式的选择是"简单优先"，但耐久性代价被低估

JSONL 的优势是追加写入简单、人类可读、行级独立。但代价是：(a) 单行损坏 → 整个 session 不可读；(b) 尾行截断（进程崩溃）→ 半行 JSON 无法解析；(c) `updateHeader` 的读-改-写需要覆盖全部消息行。**这是贯穿存储层的架构债务**——任何对 session store 的修改都必须考虑 JSONL 格式的固有脆弱性。

### Thesis 5: 凭据管理是两类体系并存的历史遗留

`credential-store.ts`（safeStorage 加密）和 `settings.json`（明文 JSON）的并存反映了 Maka 的演进历史：LLM API Key 和 OAuth Token 是后来加固的；Bot Token、Proxy Password 等是早期设计且从未迁移。**任何新的 Secret 类型（如未来的 MCP server auth）必须走 credentialStore 路径**，不能再加入 settings.json。

### Thesis 6: Bot 和 OpenGateway 是两把双刃剑

Bot Bridge 让 Maka 成为可通过 IM 使用的 AI 代理（高价值功能），但入站消息不受控、Session 创建无限制、Token 通过 HTTP 代理可能泄露。OpenGateway 让本地工具链集成成为可能，但 Bearer Token 明文存储、SSE 连接无上限、无 rate limit。两条边界的 **安全投入与其攻击面不成比例**——当前代码对这两条路径的防御主要依赖硬编码的 `explore` 模式和 `isAuthorized()` 字符串比较。

### Thesis 7: 测试基础设施的分层设计是正确的，但 Layer 2 缺像素级回归

`check-console.mjs` + `check-a11y.mjs`（Layer 0，每次 push）→ 单元测试（Layer 1）→ 截图捕获+门禁（Layer 2）→ 人工 smoke（Layer 4）的分层策略是正确的资源分配。但 Layer 2 的 `diff-screenshots.mjs` 只做 PNG 存在性/尺寸检查，不做 `pixelmatch` 内容对比——这意味着 **大量视觉回归仍然依赖人工 code review 和 smoke.md 路径**。

### Thesis 8: 合约先行是好的，但合约没有运行时调用方是坏的

9-Gate Memory 合约定义了 9 条门禁、11 个活跃错误码、完整的 `validateMemoryWriteRequest` 函数和 622 行测试。但它在运行时中零调用。**这是 Maka 最典型的"设计完成但集成未完成"的模式**——contract-only 是声明阶段的合理标签，但在 `main` 分支上如果有合约定义但无运行时消费，就是不可执行的文档。

---

## top_findings

### P0 — 必须立即修复

#### F1: StreamWatchdog 并发 pause/resume 不配对（Confirmed）

- **Source**: `01-permission-tool-safety.md` §R1
- **Evidence**: `ai-sdk-backend.ts:598-600` + `stream-watchdog.ts:71-81`。`pause()` 和 `resume()` 是布尔 flag，不是引用计数。当前 `AiSdkBackend.currentWatchdog` 是共享单例。Vercel AI SDK 支持并行 tool call（同一 step 内多个 tool）。
- **Impact**: 并行 tool call 场景下，Tool A 的 permission 被 allow → `watchdog.resume()` 恢复计时 → Tool B 的 idle timeout 可能在用户仍在决策时触发 → abort 整个 stream。
- **Recommended Fix**: 将 `StreamWatchdog` 改为 `pauseCount` 引用计数，`resume()` 仅在 `pauseCount === 0` 时真正恢复。

#### F2: Permission 请求无超时机制（Confirmed）

- **Source**: `01-permission-tool-safety.md` §B1
- **Evidence**: `ai-sdk-backend.ts:599` 的 `await verdict.parked` 无超时。`StreamWatchdog.pause()` 抑制了 idle timeout，所以 watchdog 也不会触发。唯一出口是 `endTurn('aborted')`。
- **Impact**: Bot 模式下 `ask` mode 的 session 永久卡住。用户关闭电脑后 session 无法恢复。
- **Recommended Fix**: `Promise.race` with `setTimeout`，默认 300s，超时后基于 mode 自动 deny。

#### F3: JSONL 单行损坏导致整个 Session 不可读（Confirmed）

- **Source**: `07-jsonl-durability.md` §A
- **Evidence**: `session-store.ts:224-231` 的 `readFilePartsUnlocked` 使用 `lines.slice(1).map(line => JSON.parse(line))`，任一行解析失败即 throw。尾部截断 → `JSON.parse(partial_line)` → throw。
- **Impact**: Session 从侧边栏消失（`list()` 静默跳过），用户不知道数据丢失。恢复时 `readMessages` 异常导致 session 状态重置。
- **Recommended Fix**: 逐行 `try/catch` JSON.parse，损坏行记录 `system_note` 而非 throw。尾行截断静默丢弃。

#### F4: Cache/Reasoning Token 数据采集断链（Confirmed）

- **Source**: `08-telemetry-cost.md` §loss_windows 4-5
- **Evidence**: `ai-sdk-backend.ts:504-517` 只提取 `promptTokens`、`completionTokens`、`totalTokens`。`cachedInputTokens`、`cacheWriteInputTokens`、`reasoningTokens` 从未从 ai-sdk usage 传入 `LlmCallRecord`。
- **Impact**: Anthropic cache read/write 定价体系完全无效（`computeCost` 有逻辑但数据源为 0）。Reasoning tokens 完全不计入成本。用户看到的费用为 $0.00。
- **Recommended Fix**: 在 `AiSdkBackend.send()` finally 块中提取 ai-sdk usage 的全部 token 维度。扩展 `PricingConfig` 增加 `reasoningUsdPer1M`。

### P1 — 应在下一迭代修复

#### F5: Bot 入站消息可触发无限制 Session 创建（Confirmed）

- **Source**: `05-bot-gateway-attack-surface.md` §2
- **Evidence**: `main.ts:2867-2879`：新 bot 会话的唯一条件是 `!sessionId`。`botConversationSessions` 永不超时移除。`botRecentSourceEventKeys` 大小上限 1000 但无 TTL。
- **Impact**: 攻击者可通过创建大量 Discord channel 等触发 Session 创建风暴，耗尽磁盘空间。
- **Recommended Fix**: 添加 per-conversation 速率限制，`botConversationSessions` 最大绑定数 500，为 `botRecentSourceEventKeys` 添加 1 小时 TTL。

#### F6: OpenGateway SSE 连接数无上限（Confirmed）

- **Source**: `05-bot-gateway-attack-surface.md` §3
- **Evidence**: `open-gateway.ts:508-538`：每个 SSE 连接维持 `setInterval` heartbeat (15s)，无全局连接数上限。每个连接持有 `response` 引用，大量连接累积 CPU timer。
- **Impact**: 持有有效 token 的攻击者可打开大量 SSE 连接导致资源耗尽。
- **Recommended Fix**: 全局最大 SSE 连接数十、per-session 最大三个、空闲连接（5 分钟无事件）主动关闭。

#### F7: settings.json 明文存储 5 类 Secret（Confirmed）

- **Source**: `04-credential-settings-security.md` §2 体系 B
- **Evidence**: `settings-store.ts:219-224`：Bot Token（7 平台）、Proxy Password、OpenGateway Token、Feishu App Secret、Tavily API Key 以明文 JSON 写入 `settings.json`，无加密、无 `chmod(0o600)`。
- **Impact**: 任何有 workspace 目录读权限的本地进程可读取所有 Secret。与 F5/F6 形成交叉打击面（OpenGateway Token 在 settings.json 明文 → 获取 Token → 通过 SSE 窃听所有 Session）。
- **Recommended Fix**: 按 `04-credential-settings-security.md` §6 的 4 阶段迁移计划：扩展 credentialStore → 兼容读 → 关闭明文写 → 清理。

#### F8: 9-Gate Memory 合约零运行时调用方（Confirmed）

- **Source**: `06-memory-gates.md` §enforcement_gaps
- **Evidence**: `validateMemoryWriteRequest()` 在 `apps/` 目录下零调用。`main.ts:1382-1385` 的 `memory:save` handler 直接调用 `localMemory.save(content)`，不经过 9-Gate。G9 的 `originatedFromRenderer` flag 无 setter。
- **Impact**: 9-Gate 是文档和测试层面的安全声明，不是运行时强制执行。source-laundering（将 quasi-memory 表面内容包装为 `chat_extracted` 提交）当前无防御。
- **Recommended Fix**: PR-MEMORY-2 在 IPC handler 层集成 `validateMemoryWriteRequest`，正确设置 `originatedFromRenderer`。

#### F9: IPC `connections:create` apiKey 无长度/字符校验（Confirmed）

- **Source**: `02-ipc-surface-security.md` §R1
- **Evidence**: `main.ts:2323-2347`：`apiKey` 为任意字符串，无长度上限（可写数 MB）、无 NUL/控制字符过滤。`baseUrl` 经 `normalizeConnectionBaseUrl` 但允许 localhost/内网。
- **Impact**: XSS 可通过 `window.maka.connections.create({ apiKey: many_megabytes })` 写入任意数据到 `credentials.json`。
- **Recommended Fix**: `apiKey` 增加 4096 字符上限 + NUL/控制字符过滤。`slug` 统一白名单校验 `/^[a-zA-Z0-9._-]+$/`。

#### F10: IPC `memory:save` 无内容长度上限（Confirmed）

- **Source**: `02-ipc-surface-security.md` §R6
- **Evidence**: `main.ts:1382-1385`：`content` 仅校验 `typeof === 'string'`，可直接写入 MEMORY.md 无上限。
- **Impact**: 恶意 renderer 可写入 100MB+ 字符串 → 磁盘 I/O 拒绝服务 + 下次启动 system prompt 构建崩溃。
- **Recommended Fix**: 增加 256KB 上限。

#### F11: JSONL `updateHeader` 读-改-写无锁（Confirmed）

- **Source**: `07-jsonl-durability.md` §A
- **Evidence**: `session-store.ts:161-171`：`updateHeader` 内部 `readFilePartsUnlocked` 直接读文件，不经写队列。虽然写队列串行化保障了同一 session 内的顺序，但 `readFilePartsUnlocked` 读到的消息列表可能不是最新的。
- **Impact**: 读写交织时可能丢失新追加的消息行。
- **Recommended Fix**: `readFilePartsUnlocked` 在写队列内执行（当前已在队列内但缺少显式读锁）。

#### F12: OfficeDocument 缺少 abort signal 集成（Confirmed）

- **Source**: `09-external-tool-injection.md` §cleanup_policy
- **Evidence**: `office-document-tool.ts:587-608`：`runOfficeCli` 使用 `execFile`，不接受 `AbortSignal`。`MakaToolContext.abortSignal` 在 tool context 中可用但被忽略。
- **Impact**: 上层 abort 时 officecli 子进程不会被主动 kill，持续运行到 15s 超时。
- **Recommended Fix**: 替换为 `spawn` + 手动 pipe 捕获（类似 `rive-cli.ts`），在 abort 时主动 kill。

#### F13: 三种 `isInside` 变体未统一（Confirmed）

- **Source**: `03-path-containment.md` §containment_matrix
- **Evidence**: `workspace-instructions.ts:218` 的 `isInside` 缺少 `!isAbsolute(rel)` 检查。`office-document-tool.ts:627` 和 `explore-agent-tool.ts:962` 有但格式不同。`main.ts:659` 有 `isInsideOrSamePath`（含 `target === root` + `!rel.startsWith(sep)`）。
- **Impact**: workspace-instructions 变体在 Windows 跨驱动器场景下可能被绕过（`relative` 返回绝对路径时不会被拒绝）。
- **Recommended Fix**: 统一为 `isInsideOrSamePath` 形态，放到 `@maka/core` 共享模块。

#### F14: Bot 模式 permissionMode 未被强制约束（Confirmed）

- **Source**: `01-permission-tool-safety.md` §B2
- **Evidence**: `main.ts:2876` 硬编码 `explore`，但 `SessionManager.sendMessage()` 和 BotRegistry 层没有强制校验。没有代码路径确保 bot session 的 permissionMode 不是 `ask`。
- **Impact**: 如果未来代码重构中绕过了硬编码，bot session 进入 `ask` 模式会永久阻塞（无 UI 响应 permission）。
- **Recommended Fix**: 在 `SessionManager.sendMessage()` 或 BotRegistry 层添加断言：若是 bot 调用，强制 permissionMode 为 `explore`。

### P2 — 持续改进

#### F15: `permissionScopeKey` 512 字符截断碰撞（Confirmed）

- **Source**: `01-permission-tool-safety.md` §R2
- **Evidence**: `permission.ts:310,325`：Bash 命令的 scopeKey 截断至 512 字符。两个不同命令在前 512 字符相同时共享 rememberForTurn 授权。
- **Impact**: 碰撞极度罕见（两个命令需要前 512 字符完全相同），但截断是确定性的不完美。
- **Recommended Fix**: 使用 SHA-256 前 16 字符 + 原始前 64 字符的组合 key。

#### F16: 无像素级视觉回归检测（Confirmed）

- **Source**: `10-visual-smoke-test-infra.md` §G1
- **Evidence**: `diff-screenshots.mjs` 只检查 PNG 存在/尺寸/有效性，不检查内容。组件位移、颜色变化、字体大小变化会通过当前门禁。
- **Impact**: 大量视觉回归依赖人工 code review 和 smoke.md。
- **Recommended Fix**: PR-IR-02 v3 引入 `pixelmatch` + calibrated tolerance。

#### F17: 无交互/动态行为自动化测试（Confirmed）

- **Source**: `10-visual-smoke-test-infra.md` §G2
- **Evidence**: 所有截图是静态单帧。`Cmd+K` palette 动画、Tab 焦点顺序、streaming 渲染动画、PermissionDialog checkbox 切换、ArtifactPane Esc 关闭等完全依赖人工验证。
- **Impact**: 交互逻辑回归在 CI 中完全不可见。
- **Recommended Fix**: 探索 `capture-screenshots.mjs` 注入 keyboard event。

#### F18: Bot Token 通过 HTTP 代理泄露（Hypothesis）

- **Source**: `05-bot-gateway-attack-surface.md` §5
- **Evidence**: `simple-bridge.ts:500-512`：Telegram API URL 包含 Bot Token 原文。所有出站请求经 `proxiedFetch`，会透明应用系统代理配置。
- **Impact**: 如果用户配置了 HTTP 代理（非 CONNECT tunnel），Bot Token 以明文形式经过代理服务器。
- **Recommended Fix**: 将 Telegram Bot Token 从 URL 移至 header。对代理日志风险进行文档警示。

#### F19: 进程退出时不 flush 遥测写入队列（Confirmed）

- **Source**: `08-telemetry-cost.md` §loss_windows 2
- **Evidence**: `main.ts:3431-3439`：`before-quit` handler 只清理 planReminderTimers、botRegistry、openGateway，不 flush telemetryRepo。
- **Impact**: 最后一个 turn 的 LLM 调用和工具调用遥测可能永久丢失。
- **Recommended Fix**: 在 `before-quit` 中 `await telemetryRepo.flush()`。暴露 `flush()` 方法。

#### F20: `categorizeBash` 引号内运算符误判（Confirmed, low severity）

- **Source**: `01-permission-tool-safety.md` §B3
- **Evidence**: `SHELL_CONTROL_PATTERNS` 中的 `/[;&|]/` 会匹配字符串常量中的 `|`（如 `echo "hello | world"` → `shell_unsafe`）。
- **Impact**: 过度保守分类（fail-safe），不会产生安全漏洞，但可能产生 unnecessary permission prompt。
- **Recommended Fix**: 添加引号感知的运算符检测，skip single/double quote 内容。

#### F21: `connections:fetchModels` slug 无格式校验（Confirmed）

- **Source**: `02-ipc-surface-security.md` §R3
- **Evidence**: `main.ts:2397-2423`：`slug` 为任意字符串，直接传入 `connectionStore.get()` + `fetchProviderModels()`。
- **Impact**: 如果 renderer 被 XSS，攻击者可通过遍历 slug 枚举所有连接。
- **Recommended Fix**: 增加 `slug` 格式白名单校验 + 30s 内同一 slug 最多调用 1 次的速率限制。

#### F22: `sessions:create` cwd 无路径安全校验（Confirmed）

- **Source**: `02-ipc-surface-security.md` §R7
- **Evidence**: `main.ts:1740-1773`：`input.cwd` 为任意字符串，直接传入 `runtime.createSession`。无 `realpath` 或目录白名单校验。
- **Impact**: 恶意 renderer 可设置 `cwd: "/etc"` → agent 可能读取敏感系统文件（如 PermissionEngine 存在绕过）。
- **Recommended Fix**: `cwd` 经 `realpath` + 系统目录黑名单校验。

---

## priority_roadmap

### 第 1 周（7 天）

| 天数 | 行动 | 涉及模块 | 关联发现 |
|------|------|----------|----------|
| 1-2 | **修复 StreamWatchdog 并发竞态**：将 pause/resume 改为引用计数，添加并发 tool call 测试用例 | `stream-watchdog.ts`, `ai-sdk-backend.ts` | F1 |
| 1-2 | **添加 Permission 超时机制**：`Promise.race` + 300s 超时，超时后基于 mode 自动 deny | `ai-sdk-backend.ts` | F2 |
| 3-4 | **JSONL 行级容错读取**：逐行 `try/catch` JSON.parse，损坏行记录 `system_note`，尾行截断静默丢弃 | `session-store.ts` | F3 |
| 3-4 | **修复 cache/reasoning token 数据采集**：从 ai-sdk usage 提取全部 token 维度，扩展 `PricingConfig` | `ai-sdk-backend.ts`, `cost.ts`, `types.ts` | F4 |
| 5 | **Bot 入站速率限制**：per-conversation token bucket + `botConversationSessions` 最大绑定数 | `main.ts` | F5 |
| 5-6 | **OpenGateway SSE 连接数上限**：全局 10 / per-session 3 / 空闲 5 分钟关闭 | `open-gateway.ts` | F6 |
| 6-7 | **Bot permissionMode 强制约束**：SessionManager/BotRegistry 层断言 | `session-manager.ts`, `bot-registry.ts` | F14 |
| 7 | **写 P0 修复的集成测试**：并行 tool call、permission 超时、JSONL 损坏恢复 | 各测试文件 | F1-F4 |

### 第 1 个月（30 天）

| 周次 | 行动 | 涉及模块 | 关联发现 |
|------|------|----------|----------|
| 2 | **启动 settings.json Secret 迁移**（Phase 1）：扩展 `credentialStore` 支持 bot_token、proxy_password、gateway_token、tavily_api_key | `credential-store.ts`, `main.ts` | F7 |
| 2 | **IPC apiKey 校验加固**：4096 字符上限 + NUL/控制字符过滤。slug 统一白名单校验函数 | `main.ts`, 新建 `normalize-slug.ts` | F9, F21 |
| 3 | **IPC memory:save 长度上限**：256KB cap。sessions:create cwd 安全校验 | `main.ts` | F10, F22 |
| 3 | **统一 `isInside` 实现**：抽取到 `@maka/core`，替换 4 处独立实现 | `packages/core/src/`, 3 个变体文件 | F13 |
| 4 | **PR-MEMORY-2 集成**：在 IPC handler 层调用 `validateMemoryWriteRequest`，设置 `originatedFromRenderer` | `main.ts`, `memory.ts` | F8 |
| 4 | **settings.json Secret 迁移 Phase 2-3**：兼容读 + 关闭明文写路径 | `settings-ipc-helpers.ts`, `settings-store.ts` | F7 |
| 4 | **OfficeDocument abort signal 集成**：替换 `execFile` 为 `spawn` + `AbortSignal` | `office-document-tool.ts` | F12 |

### 第 1 季度（90 天）

| 月份 | 行动 | 关联发现 |
|------|------|----------|
| 月 2 | **settings.json 迁移 Phase 4**：清理残余明文，移除 `BotChannelSettings.token`/`appSecret` 类型字段 | F7 |
| 月 2 | **OpenGateway Token 强化**：最小长度 16 字符、scope 分离（read/read+write）、支持 token 轮换 | F6 |
| 月 2 | **JSONL `updateHeader` 显式读锁**：`readFilePartsUnlocked` 在写队列内执行。`schemaVersion` 写回 | F11 |
| 月 2 | **进程退出遥测 flush**：`before-quit` 中 await telemetry 写入。`HealthSignal layer: 'storage'` 落地 | F19 |
| 月 3 | **PR-IR-02 v3 pixelmatch 视觉回归**：先在 stable subset pilot，再扩展到全部 scenario | F16 |
| 月 3 | **Telegram Bot Token 从 URL 移至 header**：降低 HTTP 代理泄露风险 | F18 |
| 月 3 | **`permissionScopeKey` 截断改进**：SHA-256 组合 key | F15 |
| 月 3 | **`categorizeBash` 引号感知运算符检测** | F20 |
| 月 3 | **IPC fuzz testing 框架**：基于 `ipc-surface-contract.test.ts` 的 channel 列表自动 fuzz | F9-F10, F21-F22 |
| 月 3 | **跨平台截图 baselines**：Windows/Linux CI runner 上捕获截图 | F16 |

---

## teaching_outline

### 给新人讲解 Maka 的建议章节

#### 第 1 章：整体架构鸟瞰（1 小时）

**目标**：理解 Maka 是什么、进程拓扑、核心数据流。

**必读文件**：
- `apps/desktop/src/main/main.ts:1008-1017` — `webPreferences` 配置（安全基座）
- `apps/desktop/src/preload/preload.ts:1-50` — `contextBridge` 暴露面
- `packages/runtime/src/ai-sdk-backend.ts:100-121` — `MakaTool<P, R>` 接口定义

**关键概念**：
- Electron main/renderer 进程分离
- `contextBridge` + `contextIsolation` + `sandbox` 三层沙箱
- IPC handler 是唯一通信路径
- `MakaTool` 是工具的统一接口

---

#### 第 2 章：权限系统（2 小时）

**目标**：理解 3×11 权限矩阵、Bash 命令分类、运行时状态机。

**必读文件**：
- `packages/core/src/permission.ts:1-100` — `PERMISSION_MODES`、`TOOL_CATEGORIES`、`PERMISSION_POLICY`
- `packages/core/src/permission.ts:137-220` — `SAFE_SHELL_PREFIXES`、`PRIVILEGED_SHELL_PREFIXES`、`FS_DESTRUCTIVE_PATTERNS`、管道的分类
- `packages/core/src/permission.ts:221-298` — `categorizeBash()`、`preToolUse()`、`permissionScopeKey()`
- `packages/runtime/src/permission-engine.ts:1-130` — `PermissionEngine` 类、`TurnState` 结构
- `packages/runtime/src/permission-engine.ts:127-230` — `evaluate()`、`recordResponse()`、`endTurn()`
- `packages/runtime/src/ai-sdk-backend.ts:534-638` — `wrapToolExecute()` 权限门控缝合点

**测试文件**：
- `packages/core/src/__tests__/permission.test.ts` — `categorizeBash` 全覆盖
- `packages/runtime/src/__tests__/permission-engine.test.ts` — 状态机

---

#### 第 3 章：路径 Containment（1.5 小时）

**目标**：理解工具如何防止文件系统逃逸。

**必读文件**：
- `packages/runtime/src/builtin-tools.ts:237-277` — `resolveWritableInsideCwd`、`resolveExistingInsideCwd`、`isInside`
- `apps/desktop/src/main/office-document-tool.ts:380-440` — 两次 `lstat`+`realpath` 双重防御
- `apps/desktop/src/main/explore-agent-tool.ts:226-300,580-680` — 只读遍历的路径安全
- `packages/storage/src/artifact-store.ts:216-260` — `isSafeRelativeArtifactPath`
- `apps/desktop/src/main/open-path-guard.ts:1-71` — `resolveOpenPath` 白名单守卫

**关键认识**：
- 所有路径围栏依赖 `realpath`，而 `realpath` 有平台差异
- 三种 `isInside` 变体的差异
- macOS `/var` → `/private/var` 的 symlink 行为

---

#### 第 4 章：IPC 安全面（2 小时）

**目标**：理解 70+ IPC handler 的输入校验、副作效果、返回脱敏。

**必读文件**：
- `apps/desktop/src/main/main.ts:2200-2550` — top 30 handler 的集中区域
- `apps/desktop/src/main/settings-ipc-helpers.ts:1-131` — `maskAppSettings`、`preserveSensitivePlaceholders`
- `apps/desktop/src/main/credential-store.ts:1-123` — safeStorage 加密存储
- `apps/desktop/src/main/main.ts:2777-3013` — Bot 入站消息处理链

**安全规则**：
- 每种 handler 的输入校验级别（强/部分/弱）
- `maskAppSettings` 覆盖的所有敏感字段
- `SENSITIVE_PLACEHOLDER` 的双向协议
- 只有 4 个 OAuth token 完全不出主进程

---

#### 第 5 章：凭据全生命周期（1.5 小时）

**目标**：理解两类 Secret 存储体系的差异和迁移计划。

**必读文件**：
- `apps/desktop/src/main/credential-store.ts` — safeStorage 的 encrypt/decrypt/队列/原子写入
- `packages/storage/src/settings-store.ts:60-85,219-230` — settings.json 明文读写
- `packages/core/src/settings.ts:40-100` — `BotChannelSettings`、`NetworkProxySettings` 等数据结构
- `apps/desktop/src/main/oauth/claude-subscription-service.ts:600-662` — OAuth token 的 safeStorage + 0o600 模式
- `apps/desktop/src/main/settings-ipc-helpers.ts:14-64` — placeholder 替换逻辑

**关键认识**：
- 5 类 Secret 仍在 settings.json 明文
- IPC 边界已遮盖但持久化层未加固
- 4 阶段迁移计划

---

#### 第 6 章：Bot Bridge 与 OpenGateway（1.5 小时）

**目标**：理解两条外部边界的攻击面和防御。

**必读文件**：
- `packages/runtime/src/bots/bot-registry.ts:1-198` — 生命周期管理
- `packages/runtime/src/bots/simple-bridge.ts:1-100,480-530` — Telegram 长轮询
- `apps/desktop/src/main/open-gateway.ts:1-200,460-540` — HTTP server、SSE、鉴权
- `apps/desktop/src/main/main.ts:2777-3013` — `handleBotIncomingMessage` → `processBotIncomingMessage` → `collectBotReply`
- `packages/core/src/bot-events.ts:130-181` — plaintext 命令、idempotency key

**安全规则**：
- Bot session 强制 `explore` 模式
- 入站/出站/SSE 连接无速率限制
- Token 明文存储 + 不支持轮换

---

#### 第 7 章：JSONL 持久化与恢复（1 小时）

**目标**：理解 session 存储格式的脆弱性和恢复逻辑。

**必读文件**：
- `packages/storage/src/session-store.ts:100-260` — read/write/append/migrate/list
- `packages/storage/src/session-store.ts:272-300` — `migrateHeader()`
- `packages/runtime/src/session-manager.ts:155-184,691-727` — `recoverInterruptedSessions()`、`interruptedTurnRecoveries()`
- `packages/runtime/src/materializer.ts:70-160` — `materializeSession()` 孤儿 tool_call 处理

**关键认识**：
- `list()` 静默跳过损坏 session
- read-modify-write 无显式锁
- `schemaVersion` 永远为 1

---

#### 第 8 章：Memory 系统（1 小时）

**目标**：理解 9-Gate 合约与 MEMORY.md 运行时的架构断隙。

**必读文件**：
- `packages/core/src/memory.ts:1-100` — 合约定义、枚举、接口
- `packages/core/src/memory.ts:460-530` — `validateMemoryWriteRequest()` 9 门入口
- `packages/core/src/local-memory.ts:1-100` — MEMORY.md 解析/生成
- `apps/desktop/src/main/local-memory-service.ts:1-150` — 文件 IO 服务
- `docs/memory-threat-model.md` — 威胁模型文档

**关键认识**：
- 9-Gate 合约在运行时零调用
- 两套独立枚举：`MemorySource/MemoryCandidateSource` vs `LocalMemoryOrigin`
- 2 个预留错误码从未发出

---

#### 第 9 章：遥测与成本（1 小时）

**目标**：理解 LLM 调用/工具调用的记录链路、成本计算和丢失窗口。

**必读文件**：
- `packages/runtime/src/telemetry/record-llm-call.ts` — LLM 调用记录入口
- `packages/runtime/src/telemetry/cost.ts` — 成本计算
- `packages/runtime/src/telemetry/builtin-pricing.ts` — 内置定价表
- `packages/storage/src/telemetry-repo.ts:1-310` — 文件持久化 + 查询
- `packages/runtime/src/ai-sdk-backend.ts:437-517` — backend 中的 telemetry 注入点

**关键认识**：
- `queueMicrotask` fire-and-forget
- cache/reasoning token 数据采集断链
- 进程退出无 flush

---

#### 第 10 章：外部工具注入（1 小时）

**目标**：理解 Rive CLI、OfficeDocument、ExploreAgent 的注入面和安全性。

**必读文件**：
- `apps/desktop/src/main/rive-cli.ts:180-330` — binary 解析、spawn、cleanup
- `apps/desktop/src/main/office-document-tool.ts:380-610` — 路径安全、execFile
- `apps/desktop/src/main/explore-agent-tool.ts:226-518` — 预算体系、路径安全、abort 策略

**关键认识**：
- Rive CLI 使用 `spawn` + `shell: false` — 参数注入低风险
- OfficeDocument 缺少 abort signal 集成
- ExploreAgent 是纯 Node.js — 无外部二进制风险

---

#### 第 11 章：测试基础设施（1 小时）

**目标**：理解分层 CI 策略和当前的覆盖盲区。

**必读文件**：
- `scripts/capture-screenshots.mjs:1-100,128-170,230-280` — 截图驱动
- `scripts/diff-screenshots.mjs:1-100,260-290` — 差异门禁
- `scripts/check-console.mjs` — 控制台审计
- `scripts/check-a11y.mjs` — 可访问性审计
- `apps/desktop/src/main/visual-smoke-fixture.ts:150-500` — fixture 种子引擎
- `docs/ui-quality-plan.md` — UI 质量计划

**关键认识**：
- 4 层 CI 策略（Layer 0-4）
- Layer 2 缺少 pixelmatch
- Layer 4 是唯一覆盖交互行为的方式

---

## next_dag

以下 DAG 节点可直接作为下一轮 Rive Workflow 的工作节点定义：

### 实现类节点（Implementation）

| 节点 ID | 标题 | 依赖 | 产出合于 |
|----------|------|------|----------|
| `impl-watchdog-refcount` | 将 StreamWatchdog pause/resume 改为引用计数 | — | `stream-watchdog.ts` |
| `impl-permission-timeout` | Permission 请求添加 300s 超时自动 deny | `impl-watchdog-refcount` | `ai-sdk-backend.ts` |
| `impl-jsonl-robust-read` | JSONL 行级容错读取 + 尾行截断静默丢弃 | — | `session-store.ts` |
| `impl-telemetry-extract-all-tokens` | 从 ai-sdk usage 提取 cache/reasoning tokens | — | `ai-sdk-backend.ts`, `cost.ts`, `types.ts` |
| `impl-bot-rate-limit` | Bot 入站 per-conversation 速率限制 | — | `main.ts`, `bot-registry.ts` |
| `impl-sse-connection-limit` | OpenGateway SSE 全局/ per-session 连接上限 | — | `open-gateway.ts` |
| `impl-credential-migration-phase1` | 扩展 credentialStore 支持 5 类新 Secret | — | `credential-store.ts` |
| `impl-ipc-apikey-validation` | apiKey 4096 上限 + slug 白名单统一校验 | — | `main.ts` |
| `impl-unify-isInside` | 统一 3 种 isInside 为单一实现 | — | `packages/core/src/` |
| `impl-memory-integrate-validator` | IPC handler 层集成 validateMemoryWriteRequest | `impl-unify-isInside` | `main.ts`, `memory.ts` |
| `impl-officecli-abort` | OfficeDocument 替换 execFile 为 spawn + AbortSignal | — | `office-document-tool.ts` |
| `impl-telemetry-flush` | 进程退出前 flush telemetry 写入队列 | — | `main.ts`, `telemetry-repo.ts` |
| `impl-pixelmatch-v3` | PR-IR-02 v3 pixelmatch 视觉回归检测 | — | `diff-screenshots.mjs` |

### 测试类节点（Test）

| 节点 ID | 标题 | 依赖 | 产出合于 |
|----------|------|------|----------|
| `test-parallel-tool-watchdog` | 并行 tool call + shared watchdog 竞态测试 | `impl-watchdog-refcount` | `ai-sdk-backend.test.ts` |
| `test-permission-timeout-e2e` | wrapToolExecute 完整 prompt→parked→timeout→deny 测试 | `impl-permission-timeout` | `ai-sdk-backend.test.ts` |
| `test-jsonl-corruption-recovery` | JSONL 单行损坏 + 尾行截断恢复测试 | `impl-jsonl-robust-read` | `session-store.test.ts` |
| `test-telemetry-token-extraction` | ai-sdk usage 全 token 维度提取集成测试 | `impl-telemetry-extract-all-tokens` | `ai-sdk-backend.test.ts` |
| `test-bot-session-storm` | Bot Session 创建风暴的速率限制测试 | `impl-bot-rate-limit` | `main.test.ts` |
| `test-memory-validator-integration` | validateMemoryWriteRequest 在 IPC handler 中调用测试 | `impl-memory-integrate-validator` | `local-memory-service.test.ts` |
| `test-isInside-unified` | 统一 isInside 跨平台行为测试（含 Windows 跨驱动器 mock） | `impl-unify-isInside` | `packages/core/src/__tests__/` |
| `test-ipc-fuzz` | 基于 channel 列表的自动 fuzz 测试框架 | — | 新建 fuzz test |

### 审计类节点（Audit）

| 节点 ID | 标题 | 依赖 | 产出合于 |
|----------|------|------|----------|
| `audit-settings-secret-scan` | 扫描 settings.json 中是否仍有明文 Secret 字段 | `impl-credential-migration-phase1` | CI gate |
| `audit-console-log-leak` | 扩展 check-console.mjs 检测 Bearer token / API key 模式 | — | `check-console.mjs` |
| `audit-bot-permission-mode` | 审计所有 bot 入站路径确保 permissionMode 不是 ask | — | 审计报告 |
| `audit-ipc-return-payload` | 扫描所有 IPC 返回值确保无 apiKey/token 明文 | — | 审计报告 |

### 建议的执行顺序

```
第 1 周（并行）:
├─ impl-watchdog-refcount
├─ impl-jsonl-robust-read
├─ impl-telemetry-extract-all-tokens
└─ impl-sse-connection-limit

第 2 周（依赖第 1 周）:
├─ impl-permission-timeout (依赖 impl-watchdog-refcount)
├─ impl-bot-rate-limit
└─ test-parallel-tool-watchdog
    test-jsonl-corruption-recovery

第 3 周（并行）:
├─ impl-credential-migration-phase1
├─ impl-ipc-apikey-validation
├─ impl-unify-isInside
├─ impl-officecli-abort
└─ audit-settings-secret-scan

第 4 周（依赖第 3 周）:
├─ impl-memory-integrate-validator (依赖 impl-unify-isInside)
├─ impl-telemetry-flush
└─ test-isInside-unified
    test-memory-validator-integration

第 2 月:
├─ impl-pixelmatch-v3
├─ test-ipc-fuzz
├─ audit-console-log-leak
└─ audit-ipc-return-payload
```

---

> **报告质量说明**：本报告基于 10 个上游精读节点的源代码证据合成。所有 "Confirmed" 标记的发现均有 `file:line` 级源码引用。所有 "Hypothesis" 标记的发现明确标注需要环境验证或测试确认。未编造任何不在上游报告中的发现。优先级排序基于影响面 × 可利用性 × 修复成本的综合评估。
