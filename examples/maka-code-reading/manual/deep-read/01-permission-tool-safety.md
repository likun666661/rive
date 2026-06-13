# Maka 精读报告：Permission / Tool Safety

> 阅读基线：`335220a` | 深度档位：`maintainer`
> 上游粗读：`00-overview.md` / `01-core-contracts.md` / `02-runtime-backends-tools.md`

---

## scope

### 已读文件

| 文件 | 行数 | 在架构中的角色 |
|------|------|---------------|
| `packages/core/src/permission.ts` | 389 | 纯函数权限矩阵：`PERMISSION_POLICY`（3×11）、`categorizeBash()`（6 层分类）、`preToolUse()`（3 步评估）、`permissionScopeKey()` |
| `packages/runtime/src/permission-engine.ts` | 249 | 运行时状态引擎：per-turn `remembered` Set、`parked` Promise 注册表、`beginTurn/endTurn/recordResponse` |
| `packages/runtime/src/ai-sdk-backend.ts` | 1181 | `wrapToolExecute()`——权限门控缝合点；`handleStreamChunk()`——provider chunk 规范化；`send()`——完整 agent 循环 |
| `packages/runtime/src/builtin-tools.ts` | 278 | 6 个内置工具的实现：Bash/Read/Write/Edit/Glob/Grep，含 `resolveWritableInsideCwd` / `resolveExistingInsideCwd` / `isInside` 路径 containment |
| `packages/runtime/src/stream-watchdog.ts` | 119 | 两阶段超时（connect/idle）+ `pause()/resume()` 用于 permission 等待期间抑制 idle timeout |
| `packages/runtime/src/__tests__/permission-engine.test.ts` | 367 | PermissionEngine 单元测试（allow/block/prompt、rememberForTurn、endTurn reject、input validation） |
| `packages/runtime/src/__tests__/ai-sdk-backend.test.ts` | 502 | AiSdkBackend 错误面测试（secret redaction、terminal failure 保留、stop→endTurn、categoryHint、subagent 并发限制、repairMakaToolCall） |
| `packages/runtime/src/__tests__/stream-watchdog.test.ts` | 139 | StreamWatchdog 独立行为（connect/idle timeout、pause/resume、stop cancel） |
| `packages/runtime/src/__tests__/builtin-tools.test.ts` | 176 | 路径 containment 测试（absolute/`..`/symlink escape for Read/Write/Edit/Glob/Grep）、Bash 流式输出/abort |
| `packages/core/src/__tests__/permission.test.ts` | 336 | `categorizeBash()` 全覆盖（safe/unsafe/destructive/privileged/pipe）、`preToolUse()` 3-mode×category 矩阵 + turnRemembered、`permissionScopeKey` |
| `packages/core/src/__tests__/permission-request-health.test.ts` | 58 | Permission 请求时效判定（fresh/stale/expired）和格式化 |

### 关键函数

| 函数 | 文件:行号 | 职责 |
|------|-----------|------|
| `preToolUse()` | `permission.ts:255` | 纯函数：classify → policy lookup → turnRemembered check → 输出 allow/block/prompt |
| `categorizeBash()` | `permission.ts:221` | 6 层优先级分类：privileged > fs_destructive > git_destructive > shell_unsafe（管道/控制符） > safe prefix > default unsafe |
| `permissionScopeKey()` | `permission.ts:299` | 生成 per-tool-intent 的 scope key（用于 turnRemembered），Bash 截断至 512 字符 |
| `PermissionEngine.evaluate()` | `permission-engine.ts:127` | 运行时包装：调用 `preToolUse()`，生成 requestId，创建 parked Promise |
| `PermissionEngine.recordResponse()` | `permission-engine.ts:197` | 路由用户响应→resolve parked→可选 `rememberForTurn` 写入 remembered Set |
| `PermissionEngine.endTurn()` | `permission-engine.ts:111` | reject 所有未决 parked Promise |
| `AiSdkBackend.wrapToolExecute()` | `ai-sdk-backend.ts:534` | 权限门控缝合点：JSONL 写入→PermissionEngine 评估→watchdog pause/resume→impl 执行→telemetry |
| `AiSdkBackend.send()` | `ai-sdk-backend.ts:264` | agent 循环入口：构建 queue/tools/watchdog/pump，yield SessionEvent |
| `AiSdkBackend.handleStreamChunk()` | `ai-sdk-backend.ts:802` | SDK chunk → SessionEvent 规范化（`text-delta`、`reasoning`、`error`，忽略 `tool-call`/`tool-result`） |
| `StreamWatchdog.pause()` / `resume()` | `stream-watchdog.ts:71-81` | 权限等待期间暂停/恢复 idle timeout |
| `resolveWritableInsideCwd()` | `builtin-tools.ts:237` | Write/Bash 路径 containment：`realpath(root)` → `resolve(root, inputPath)` → `isInside` 双重检查 |
| `resolveExistingInsideCwd()` | `builtin-tools.ts:253` | Read/Edit/Grep 路径 containment：额外 `realpath(candidate)` 防符号链接逃逸 |
| `isInside()` | `builtin-tools.ts:269` | `relative(root, target)` 不包含 `..` 且非绝对路径 |

---

## problem

Permission / Tool Safety 子系统要解决的核心问题：

1. **Agent 具有文件系统的破坏能力**：`Bash` 工具可以执行任意 shell 命令（`rm -rf`、`sudo`、`git reset --hard`），`Write`/`Edit` 可以覆盖用户源码。用户需要粒度控制：什么模式（explore/ask/execute）下什么操作类别（11 个 ToolCategory）被允许/提示/阻止。

2. **异步权限决策不能阻塞 agent 循环**：当模型发起 tool call 需要用户确认时，permission 挂起（`await parked`）会阻塞整个 `streamText` 循环。同时 StreamWatchdog 的 idle timeout 仍在计时——需要在等待用户决策期间 `pause()` 暂停 timeout，避免误触发。

3. **Bot 模式没有 UI**：Telegram/Feishu/WeChat 等 bot bridge 接入时，没有渲染进程弹出 PermissionDialog。Bot 模式必须依赖 `explore` 或 `execute` 模式的自动决策（allow/prompt/block），`ask` 模式下的 `prompt` 会永久阻塞。

4. **崩溃恢复的数据一致性**：`tool_call` 消息先于 permission 写入 JSONL——即使进程在 permission 等待期间崩溃，materializer 也能将 orphan `tool_call` 渲染为 `interrupted` 状态。

5. **路径 containment 是最后的安全防线**：即便 permission 允许了 Write/Bash，工具 impl 层仍强制执行 cwd sandbox（`resolveWritableInsideCwd` / `resolveExistingInsideCwd`），防止 agent 通过符号链接或 `..` 逃逸到工作区外。

---

## source_evidence

### 权限矩阵与分类

| 函数/常量 | 文件:行号 | 证据 |
|-----------|-----------|------|
| `PERMISSION_MODES` | `permission.ts:16` | `['explore', 'ask', 'execute'] as const`，闭合枚举 |
| `TOOL_CATEGORIES` | `permission.ts:38` | 11 个类别：`read`, `web_read`, `file_write`, `fs_destructive`, `shell_safe`, `shell_unsafe`, `git_destructive`, `network_send`, `privileged`, `custom_tool`, `subagent` |
| `PERMISSION_POLICY` | `permission.ts:62` | `Record<PermissionMode, Record<ToolCategory, PolicyDecision>>` 矩阵：explore 模式大部分 block、execute 模式大部分 allow、fs_destructive/git_destructive/privileged 在任何模式都 prompt |
| `categorizeBash()` | `permission.ts:221` | 6 层优先级：`privileged > fs_destructive > git_destructive > shell_unsafe(管道/控制符) > safe prefix > default unsafe` |
| `SAFE_SHELL_PREFIXES` | `permission.ts:137` | 14 个安全前缀（`ls`, `pwd`, `cat`, `git status` 等），故意排除 `cd` 和 `env` |
| `PRIVILEGED_SHELL_PREFIXES` | `permission.ts:166` | 13 个特权前缀（`sudo`, `chmod`, `kill`, `systemctl` 等） |
| `FS_DESTRUCTIVE_PATTERNS` | `permission.ts:184` | 11 个正则（`rm`, `rmdir`, `dd`, `shred`, `truncate`, `mkfs`, `git restore`/`git checkout --`, `find -delete`/`find -exec rm`, `xargs rm`） |
| `PIPE_DESTRUCTIVE_PATTERNS` | `permission.ts:199` | 管道到 `xargs rm/shred/...` 或 `sh/bash/zsh` |
| `SHELL_CONTROL_PATTERNS` | `permission.ts:204` | 重定向 `>>?|>|&>`、分隔符 `[;&|]`、反引号 `` ` ``、子 shell `$(`，命中后升级为 `shell_unsafe` |
| `DESTRUCTIVE_GIT_PATTERNS` | `permission.ts:211` | 6 个正则（`git reset --hard`, `push --force`, `branch -D`, `clean -fd`, `checkout .`, `rebase -i`） |

### PermissionEngine 状态管理

| 函数 | 文件:行号 | 证据 |
|------|-----------|------|
| `PermissionEngine.evaluate()` | `permission-engine.ts:127` | 调用 `preToolUse()` → 三路分支 → prompt 时创建 parked Promise 并存入 `state.parked` Map |
| `PermissionEngine.recordResponse()` | `permission-engine.ts:197` | 校验 `response.requestId` / `decision` ∈ {allow,deny} / `rememberForTurn` boolean → resolve parked → 可选写 `state.remembered` |
| `PermissionEngine.endTurn()` | `permission-engine.ts:111` | `for (const parked of state.parked.values()) parked.reject(...)` → delete turn state |
| `PermissionEngine.requireTurn()` | `permission-engine.ts:229` | 自动 begin turn（若调用者忘记 beginTurn） |
| `TurnState.remembered` | `permission-engine.ts:42` | `Set<string>`，key 为 `scopeKey`（来自 `preToolUse` 返回的 `scopeKey`） |
| `TurnState.parked` | `permission-engine.ts:44` | `Map<string, ParkedRequest>`，key 为 `requestId` |

### wrapToolExecute 权限门控（AiSdkBackend）

| 代码块 | 文件:行号 | 证据 |
|--------|-----------|------|
| ToolCallMessage 写入 JSONL（先于 permission） | `ai-sdk-backend.ts:548-558` | `await this.input.appendMessage(callMsg)`——确保崩溃恢复材料化 |
| `permissionRequired === false` 跳过 permission | `ai-sdk-backend.ts:575-576` | Read/Glob/Grep 快速路径：直接进入 impl 执行 |
| PermissionEngine.evaluate() 调用 | `ai-sdk-backend.ts:578-586` | 传入 `categoryHint`、`mode`、`args` |
| block → 合成错误 ToolResult | `ai-sdk-backend.ts:588-591` | `writeSyntheticToolResult()` + `errorReturn(verdict.reason)` |
| prompt → emit event → watchdog.pause() → await parked → watchdog.resume() | `ai-sdk-backend.ts:593-607` | **核心异步竞态点**（见 risk_matrix） |
| permission deny → 合成 "用户已拒绝" ToolResult | `ai-sdk-backend.ts:632-636` | `writeSyntheticToolResult()` + `errorReturn(reason)` |
| impl 执行 | `ai-sdk-backend.ts:655-728` | `tool.impl(args, ctx)` → `coerceResultContent()` → JSONL + event + telemetry |
| impl 异常 → `coerceTerminalFailure()` (Bash only) | `ai-sdk-backend.ts:731-773` | Bash 工具捕获 `code` + `stdout`/`stderr`，redactSecrets |
| Subagent slot 预留 | `ai-sdk-backend.ts:641-645` | `MAX_ACTIVE_SUBAGENT_TOOLS_PER_TURN = 5`，超出返回合成错误 |
| Artifact 派生（fire-and-forget） | `ai-sdk-backend.ts:707-728` | `recordToolArtifactsSafely()` 不阻塞 agent 循环 |
| Telemetry（fire-and-forget） | `ai-sdk-backend.ts:692-705` | `recordToolInvocation?.(...)` |

### StreamWatchdog 与 Permission 协同

| 代码 | 文件:行号 | 证据 |
|------|-----------|------|
| `StreamWatchdog.pause()` | `stream-watchdog.ts:71-75` | 设置 `this.paused = true`，清除 timer——非嵌套、非引用计数 |
| `StreamWatchdog.resume()` | `stream-watchdog.ts:77-81` | 设置 `this.paused = false`，调用 `markActivity()` 重置 idle timer |
| `StreamWatchdog.markActivity()` | `stream-watchdog.ts:64-69` | `this.sawActivity = true`，若未 paused 则 schedule idle timeout |
| Watchdog 创建与 connection | `ai-sdk-backend.ts:342-353` | 在 `send()` 的 pump 闭包中创建，存到 `this.currentWatchdog` |
| `currentWatchdog` 字段 | `ai-sdk-backend.ts:248` | 可空单例：`private currentWatchdog: StreamWatchdog | null = null` |

### 路径 Containment（builtin-tools）

| 函数 | 文件:行号 | 证据 |
|------|-----------|------|
| `resolveWritableInsideCwd()` | `builtin-tools.ts:237-251` | 拒绝绝对路径 → `realpath(cwd)` → `resolve(root, inputPath)` → `isInside(root, candidate)` → `realpath(dirname(candidate))` → `isInside(root, parent)` |
| `resolveExistingInsideCwd()` | `builtin-tools.ts:253-267` | 额外 `realpath(candidate)` → `isInside(root, target)` 防止符号链接逃逸 |
| `isInside()` | `builtin-tools.ts:269-272` | `relative(root, target)` 不含 `..` 且非绝对路径 |
| `assertRelativeGlobPattern()` | `builtin-tools.ts:274-277` | 拒绝绝对路径和 `..` 的 Glob pattern |

---

## flow_analysis

### 完整调用链：tool call → tool_call message → permission decision → watchdog pause/resume → execute → tool result → telemetry

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  STEP 0: streamText 自动调用                                                 │
│                                                                             │
│  Vercel AI SDK 内部循环：                                                     │
│    model generates tool_use block → ai-sdk calls tool.execute(args, ctx)    │
│    → 进入 wrapToolExecute()                                                   │
└─────────────────────────┬───────────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  STEP 1: 写入 ToolCallMessage → JSONL (ai-sdk-backend.ts:548-558)            │
│                                                                             │
│  ★ 先于 permission 写入——崩溃恢复保证。                                       │
│  callMsg = { type: 'tool_call', id: toolUseId, turnId, ts,                  │
│              toolName, displayName?, intent?, args }                        │
│  await appendMessage(callMsg)    // 阻塞直到 JSONL 落盘                        │
│  queue.push(ToolStartEvent)       // 通知 UI 开始渲染 tool card               │
└─────────────────────────┬───────────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  STEP 2: Permission 门控 (ai-sdk-backend.ts:575-638)                         │
│                                                                             │
│  if (tool.permissionRequired === false) → SKIP, goto STEP 3                 │
│    ↓                                                                        │
│  verdict = permissionEngine.evaluate({ sessionId, turnId, toolUseId,         │
│             toolName, args, categoryHint?, mode })                          │
│    │                                                                        │
│    ├─ kind='allow' → goto STEP 3                                            │
│    │                                                                        │
│    ├─ kind='block' → writeSyntheticToolResult(reason) → return errorReturn  │
│    │                                                                        │
│    └─ kind='prompt'                                                        │
│         ├─ queue.push(verdict.event)  // PermissionRequestEvent → UI        │
│         │                                                                  │
│         ├─ this.currentWatchdog?.pause()  // ★ 暂停 idle timeout            │
│         │                                                                  │
│         ├─ response = await verdict.parked  // ★ 阻塞等待用户决策            │
│         │     │                                                            │
│         │     │  [用户通过 UI 点击 Allow/Deny]                               │
│         │     │  → IPC → SessionManager.respondToPermission()               │
│         │     │  → backend.respondToPermission(decision)                    │
│         │     │  → permissionEngine.recordResponse(turnId, decision)        │
│         │     │  → parked.resolve(response)                                 │
│         │     │  → write PermissionDecisionMessage → JSONL                  │
│         │     │  → emit PermissionDecisionAckEvent → queue                  │
│         │     │                                                            │
│         ├─ this.currentWatchdog?.resume()  // ★ 恢复 idle timeout            │
│         │                                                                  │
│         ├─ decision='allow' → goto STEP 3                                  │
│         └─ decision='deny' → writeSyntheticToolResult("用户已拒绝")          │
│              → return errorReturn                                           │
└─────────────────────────┬───────────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  STEP 3: Subagent slot 预留 (ai-sdk-backend.ts:641-645)                      │
│                                                                             │
│  if (tool.categoryHint === 'subagent' && activeSubagentToolCount >= 5) {    │
│    writeSyntheticToolResult(SUBAGENT_TOOL_LIMIT_MESSAGE)                    │
│    return errorReturn                                                       │
│  }                                                                          │
│  activeSubagentToolCount++                                                  │
└─────────────────────────┬───────────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  STEP 4: 执行 tool.impl (builtin-tools.ts)                                   │
│                                                                             │
│  Bash:  spawn(command, { cwd, shell: true })                                │
│         → streaming stdout/stderr via emitOutput                            │
│         → 10MB hard cap → abort on exceed                                   │
│         → timeout / abortSignal → kill('SIGTERM')                           │
│                                                                             │
│  Write: resolveWritableInsideCwd(cwd, path) → fs.writeFile(abs, content)    │
│  Read:  resolveExistingInsideCwd(cwd, path) → fs.readFile(abs)              │
│  Edit:  resolveExistingInsideCwd(cwd, path) → read → replace → write        │
│  Glob:  assertRelativeGlobPattern → nodeGlob(pattern, { cwd: base })        │
│  Grep:  resolveExistingInsideCwd(cwd, path) → execAsync(rg ...)             │
└─────────────────────────┬───────────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  STEP 5: coerceResultContent + ToolResultMessage (ai-sdk-backend.ts:667-690) │
│                                                                             │
│  content = coerceResultContent(result)    // string → {kind:'text'}         │
│  resultMsg = { type: 'tool_result', ..., isError, content, durationMs }    │
│  await appendMessage(resultMsg)           // JSONL 落盘                      │
│  queue.push(ToolResultEvent)              // UI 更新                         │
└─────────────────────────┬───────────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  STEP 6: Telemetry (fire-and-forget) (ai-sdk-backend.ts:692-728)             │
│                                                                             │
│  recordToolInvocation({ status, durationMs, bytesIn, bytesOut, ... })       │
│  void recordToolArtifactsSafely(deriveToolArtifactCandidates, recorder)     │
│                                                                             │
│  finally: releaseSubagentSlot(tool)  // activeSubagentToolCount--           │
│                                                                             │
│  return result → back to ai-sdk, which may call next tool or generate text  │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 关键时序约束

1. **JSONL 写入顺序**：`tool_call` → `permission_decision`（如有 prompt）→ `tool_result`。这个顺序由 `wrapToolExecute` 内同步 `await` 链保证。

2. **Watchdog pause/resume 区间**：`pause()` 和 `resume()` 之间的时间取决于用户决策延迟。如果用户永远不响应，parked Promise 永不 resolve。只有当 `endTurn('aborted')` 被调用（例如 stop session）时，parked 才会 reject，catch 分支执行 `resume()`。

3. **queue.push 与 yield 的时序**：`wrapToolExecute` 直接向 `queue` push 事件，与 `send()` 中的 `for await (const ev of queue) yield ev` 形成生产者-消费者关系。`AsyncEventQueue` 保证 FIFO 顺序。

---

## risk_matrix

### 竞态 / 不变量风险

#### R1: 共享 StreamWatchdog 的并发 pause/resume 不配对 🔴

| 属性 | 详情 |
|------|------|
| **风险位置** | `ai-sdk-backend.ts:598-600` + `stream-watchdog.ts:71-81` |
| **触发条件** | ai-sdk 在同一 step 内并行调用多个 tool（Vercel AI SDK 支持 parallel tool calls），且所有 tool 都触发 `kind='prompt'` |
| **场景** | Tool A 调用 `watchdog.pause()` → Tool B 也调用 `watchdog.pause()`（paused 已是 true，无变化）→ Tool A 被用户 allow，调用 `watchdog.resume()`（paused=false 且重新 markActivity）→ **Watchdog 此时已恢复计时，而 Tool B 仍在等待用户 permission** |
| **后果** | 如果用户对 Tool B 的 permission 响应超过 `idleTimeoutMs`（120s），watchdog 会触发 `onTimeout`，abort 整个 stream——即使 Tool B 的等待是合法的 |
| **根本原因** | `StreamWatchdog` 的 pause/resume 是布尔 flag，不支持嵌套/引用计数。`AiSdkBackend.currentWatchdog` 是单例，所有 tool call 共享同一个 watchdog 实例 |
| **严重度** | 🔴 高——虽然当前 ai-sdk 并行 tool call 场景较少见，但一旦发生，会导致无辜 tool call 被 timeout abort |
| **修复方向** | 将 `pause/resume` 改为引用计数（`pauseCount`），或为每个 tool call 创建独立的 sub-watchdog |

#### R2: `permissionScopeKey` 512 字符截断导致意外授权 🟡

| 属性 | 详情 |
|------|------|
| **风险位置** | `permission.ts:310` + `permission.ts:325` |
| **触发条件** | 两个不同 Bash 命令在前 512 个规范化字符内完全相同（例如一个长路径前缀 + 不同后缀参数），用户对第一个 rememberForTurn |
| **场景** | 用户 allow `rm /very/long/path/...` 并勾选 "Remember for this turn"，第二个 `rm /very/long/path/...different` 被自动 allow |
| **后果** | 用户可能无意中授权了不同的破坏性命令 |
| **严重度** | 🟡 中——512 字符碰撞极度罕见，但截断是确定性的不完美 |
| **修复方向** | 对 Bash scopeKey 使用 SHA-256 哈希前 16 字符 + 原始前 64 字符组合，或直接使用完整命令（accept 更长的 scopeKey） |

#### R3: `endTurn` 期间 `currentWatchdog` 可能为 null 🟡

| 属性 | 详情 |
|------|------|
| **风险位置** | `ai-sdk-backend.ts:502-503` |
| **触发条件** | `cleanupAfterTurn()` 在 `send()` 的 finally 块中被调用，同时 `wrapToolExecute` 的 parked Promise 仍在等待 |
| **场景** | `send().finally` → `cleanupAfterTurn()` 将 `this.currentTurnId = null` 等 → 但 `wrapToolExecute` 的 then/catch 闭包持有对 `this.currentWatchdog` 的引用。cleanupAfterTurn 不清理 `currentWatchdog`，它由 pump 闭包的 finally 清理 |
| **分析** | 当前实现中 `currentWatchdog` 的清理在 pump 的 finally 块（`ai-sdk-backend.ts:502-503`）和 `cleanupAfterTurn`（仅清理 `currentTurnId`/`currentQueue`/`activeSubagentToolCount`/`abortController`/`aborted`）。两个清理路径是顺序的：pump 完成 → 清理 watchdog → 进入 send().finally → cleanupAfterTurn。没有竞态 |
| **严重度** | 🟢 低——当前代码路径安全，但 `currentWatchdog` 的 null 检查依赖可选链 `?.`，属于防御性编程 |

#### R4: `respondToPermission` 与 `endTurn` 的 TOCTOU 🟢

| 属性 | 详情 |
|------|------|
| **风险位置** | `permission-engine.ts:197` + `ai-sdk-backend.ts:883` |
| **场景** | 用户 Allow 的同时执行 Stop Session。IPC handler 可能先调用 `stop()`（trigger `endTurn('aborted')`）再调用 `respondToPermission()`，或反之 |
| **分析** | `endTurn('aborted')` 会 reject 所有 parked Promise（包括正在被 resolve 的）。由于 JS 单线程事件循环，两个操作不会真正并发。`recordResponse()` 先 delete 再 resolve，即使 endTurn 先 reject，parked Promise 的状态已确定（rejected），后续 resolve 无效果 |
| **严重度** | 🟢 低——JS 单线程 + Promise 单次 settle 保证 |

### 功能风险矩阵

| 维度 | 场景 | 当前行为 | 风险 |
|------|------|----------|------|
| **allow** | explore 模式 Read 工具 | `permissionRequired=false` → 直接跳过 engine，进入 impl | ✅ 安全——仍有路径 containment |
| **block** | explore 模式 Write 工具 | `preToolUse` 返回 `proceed:false, blockReason` → `writeSyntheticToolResult` + `errorReturn` | ✅ 安全——不执行 impl |
| **block** | `preToolUse` 返回三个 false（无 partialRequest） | `permission-engine.ts:144-153` defensive 转 block，reason: "invariant violated" | ✅ fail-safe |
| **prompt** | ask 模式 Write | `emit PermissionRequestEvent` → `await parked` → 用户 allow/deny | ⚠️ 见 R1 |
| **prompt** | execute 模式 rm | 即使 execute mode 也 prompt `fs_destructive` | ✅ 不可逆操作永远需确认 |
| **prompt** | 用户永不响应 | parked Promise 永久阻塞，没有超时 | 🟡 见 B1 |
| **abort** | `stop('user_stop')` | `abortController.abort()` + `endTurn('aborted')` → reject 所有 parked | ✅ parked 全部清理 |
| **abort** | `endTurn` 先于 pump.done | cleanupAfterTurn 在 send().finally 中调用，确保 pump 结束后执行 | ✅ 顺序安全 |
| **reject** | parked catch 分支 | `watchdog.resume()` + `writeSyntheticToolResult` + `errorReturn` | ✅ 正确恢复 |
| **timeout** | watchdog.connect timeout | `onTimeout` → `queue.push(ErrorEvent)` + `abortController.abort()` | ✅ 整个 stream 终止 |
| **timeout** | watchdog idle timeout（无 permission） | `onTimeout` → `queue.push(ErrorEvent)` + `abortController.abort()` | ✅ 正确 |
| **remember-for-turn** | Write path `/x` → allow + remember → 再次 Write `/x` | `scopeKey` 命中 `turnRemembered` Set → `preToolUse` 直接 allow | ✅ 精确 scope 匹配 |
| **remember-for-turn** | Write path `/x` → allow + remember → Write path `/y` | 不同 scopeKey → 重新 prompt | ✅ 不跨文件授权 |
| **remember-for-turn** | Write `/x` → remember → 同 turn 内 Edit `/x` | Write 的 scopeKey 不等于 Edit 的 scopeKey（tool name 不同）→ 重新 prompt | ✅ 不跨工具授权 |
| **remember-for-turn** | remember 不覆盖 block | `permission.ts:280-282`：即使 scopeKey 在 remembered Set 中，`decision === 'block'` 直接返回 block | ✅ 正确 |
| **bot mode** | ask mode + bot bridge | `prompt` park 永久阻塞，没有 UI 响应 | 🟡 见 B2 |
| **bot mode** | explore mode + bot bridge | 大部分工具 block，只有 Read/Glob/Grep/shell_safe/subagent allow | ✅ 只读安全 |
| **bot mode** | execute mode + bot bridge | 除 fs_destructive/git_destructive/privileged 外全部 allow | ⚠️ 用户需理解风险 |
| **permissionRequired=false** | Read/Glob/Grep | 跳过 PermissionEngine，直接 impl | ✅ 仍需路径 containment |
| **permissionRequired=false + subagent** | ExploreAgent 类 tool | 跳过 permission 门控，但仍需 subagent slot 预留 | ✅ slot 在 permission 之后检查 |
| **invalid tool name** | 模型请求未知工具 | `repairMakaToolCall` → 大小写修复或路由到 `INVALID_TOOL_NAME` tool → impl 抛出 "模型请求了不可用工具" | ✅ fail-safe |
| **concurrent subagent** | >5 个 ExploreAgent per turn | `reserveSubagentSlot` 返回 false → `writeSyntheticToolResult` | ✅ |
| **input validation** | `recordResponse` 收到畸形 decision | `permission-engine.ts:201-208` 抛出 "Invalid permission response" | ✅ fail-closed |
| **unknown requestId** | stray response | `recordResponse` 返回 null，不抛异常 | ✅ 幂等安全 |

### 发现的不安全路径

#### B1: Permission 请求无超时机制 🟡

```
wrapToolExecute() → await verdict.parked  ← 永久阻塞
```

如果用户关闭电脑、Render 进程崩溃、或 bot bridge 无超时信号，parked Promise 永不 resolve。`StreamWatchdog.pause()` 抑制了 idle timeout，所以 watchdog 也不会触发。唯一的出口是 `endTurn('aborted')`（通过 `stop('user_stop')` 或 `dispose()`）。

**影响**：bot 模式下 ask mode 的 session 会永久卡住。

**建议**：为 parked Promise 添加可配置的超时（如 5 分钟），超时后自动 reject 并执行 fallback（如 block 或自动 allow based on mode）。

#### B2: Bot 模式 PermissionMode 未强制约束 🟡

Bot bridge (`simple-bridge.ts`) 调用 `SessionManager.sendMessage()` 时，会创建新的 AiSdkBackend，其 `permissionMode` 来自 `SessionHeader`。如果 header 的 permissionMode 是 `ask`，tool 将永久等待用户 permission，bot bridge 没有 respondToPermission 的机制。

**当前无约束**：Bot 模式下没有代码强制将 permissionMode 切换为 `explore` 或 `execute`。

**建议**：在 `SessionManager.sendMessage()` 或 BotRegistry 层强制 bot session 的 permissionMode 为 `explore`（最安全）或至少不为 `ask`。

#### B3: `categorizeBash` 对带引号的命令的管道检测可能不足 🟢

```
categorizeBash('echo "|" harmless') → shell_safe  // 正确
categorizeBash("echo '|' harmless") → shell_safe  // 正确
```

`SHELL_CONTROL_PATTERNS` 中的 `/[;&|]/` 会匹配字符串常量中的 `|`（如 `echo "hello | world"` 中的 `|`）吗？当前正则 `/[;&|]/` 没有引号上下文感知。这意味着：
- `echo "hello | world"` → 命中 `|` → 分类为 `shell_unsafe`（安全但过度保守）

**影响**：🟢 低——过度分类为 `shell_unsafe` 在 explore 模式下会 block，在 ask 模式下会 prompt，不会产生安全漏洞（fail-safe）。

---

## tests

### 现有测试覆盖

| 测试文件 | 覆盖范围 | 覆盖充分度 |
|----------|----------|------------|
| `permission.test.ts` (336 行) | `categorizeBash()` 全覆盖（safe/unsafe/destructive/privileged/pipe/control/precedence）、`preToolUse()` 3-mode×category 矩阵、`turnRemembered`、`permissionScopeKey`、`PERMISSION_POLICY` 矩阵完整性 | ✅ 优秀 |
| `permission-request-health.test.ts` (58 行) | 时效判定（fresh/stale/expired）、格式化 | ✅ 充分 |
| `permission-engine.test.ts` (367 行) | allow/block/prompt 三种路径、rememberForTurn scope 隔离、endTurn reject + clear、input validation（畸形 decision）、unknown requestId/turnId、idempotent beginTurn | ✅ 优秀 |
| `stream-watchdog.test.ts` (139 行) | connect timeout、activity→idle timeout 切换、pause/resume、stop cancel | ✅ 充分 |
| `ai-sdk-backend.test.ts` (502 行) | secret redaction（error message + synthetic tool result）、terminal failure 保留 stdout/stderr、stop→endTurn reject parked、categoryHint 透传、subagent 并发限制（MAX=5）、repairMakaToolCall 大小写修复/无效路由/递归保护 | ✅ 优秀 |
| `builtin-tools.test.ts` (176 行) | Read 路径 containment（absolute/`..`/symlink escape）、Glob/Grep cwd constraint、Write 路径 containment（absolute/`..`/symlink-parent）、Edit 路径 containment、Bash 流式输出/abort | ✅ 良好 |

### 测试缺口

| 缺口 | 优先级 | 建议测试 |
|------|--------|----------|
| **T1: 并行 tool call + shared watchdog 竞态** | 🔴 P0 | 模拟两个 tool 同时 prompt 并停留，其中一个响应后验证 watchdog 未被错误恢复 |
| **T2: `wrapToolExecute` 完整 permission prompt → allow → execute 端到端** | 🔴 P0 | 当前仅测独立 `wrapToolExecute` 行为，缺少通过 mock PermissionEngine 的完整 prompt→parked→resolve→impl 流程 |
| **T3: `permissionRequired=false` + subagent slot 交互** | 🟡 P1 | 验证 `categoryHint='subagent'` + `permissionRequired=false` 的 tool 仍正确计入 slot 限制 |
| **T4: `recordResponse` 在 `endTurn` 之后的竞态** | 🟡 P1 | 模拟 stop() 与 respondToPermission() 的时序交叉：先 endTurn 再 recordResponse 应返回 null（当前已验证），但先 recordResponse 再 endTurn 应正确 resolve |
| **T5: Bot 模式 ask mode 下的 permanent block** | 🟡 P1 | 验证当 permissionMode=`ask` 且无 UI 响应时，parked Promise 的行为和 watchdog 状态 |
| **T6: `permissionScopeKey` 截断碰撞** | 🟢 P2 | 构造两个 Bash 命令在前 512 字符相同、之后不同的边界 case |
| **T7: `categorizeBash` 特殊 Unicode 和编码攻击** | 🟢 P2 | 测试带 null byte、RTL override（U+202E）、同形异义词的命令 |
| **T8: `SHELL_CONTROL_PATTERNS` 引号内误判** | 🟢 P2 | `echo "hello | world"` 应分类为 safe（`echo` 是 safe prefix），但当前正则会命中 `|` → `shell_unsafe` |
| **T9: `watchdog.resume()` 在 `stopped` 状态下的行为** | 🟢 P2 | 确认 stop() → resume() 不会重新激活已停止的 watchdog |
| **T10: `coerceTerminalFailure` 对非 Bash 工具的跳变** | 🟢 P2 | 验证 Write/Edit 错误的 error 不会被错误地当做 terminal output |

---

## next_actions

### P0（必须立即修复）

| 编号 | 问题 | 文件:行号 | 建议 |
|------|------|-----------|------|
| **P0-1** | StreamWatchdog 并发 pause/resume 不配对 | `ai-sdk-backend.ts:598-600` | 将 `StreamWatchdog` 改为引用计数：`pauseCount++/pauseCount--`，`resume()` 仅在 `pauseCount === 0` 时真正恢复。或为每个需 permission 的 tool call 创建独立 sub-watchdog，父 watchdog 仅在所有 sub-watchdog 不暂停时计时 |
| **P0-2** | Permission 请求无超时 | `ai-sdk-backend.ts:599` | 为 `await verdict.parked` 添加超时（`Promise.race` with `setTimeout`），默认 300s（5min）。超时后自动 resolve 为 deny 或基于 mode 决定（explore→deny、execute→allow） |

### P1（应在下一迭代修复）

| 编号 | 问题 | 文件:行号 | 建议 |
|------|------|-----------|------|
| **P1-1** | Bot 模式 permissionMode 未强制约束 | `session-manager.ts` / `simple-bridge.ts` | 在 BotRegistry 或 SessionManager.sendMessage() 层：若 caller 是 bot，强制将 permissionMode 切换为 `explore`（最安全）。记录 warning 日志而不是静默切换 |
| **P1-2** | 并行 tool call 竞态的测试覆盖 | `__tests__/ai-sdk-backend.test.ts` | 添加 T1 测试用例，模拟两个 tool 同时进入 prompt 分支的场景 |
| **P1-3** | `wrapToolExecute` 端到端测试 | `__tests__/ai-sdk-backend.test.ts` | 添加 T2 测试用例，覆盖 prompt→parked.allow→impl→result 完整路径 |

### P2（持续改进）

| 编号 | 问题 | 文件:行号 | 建议 |
|------|------|-----------|------|
| **P2-1** | `permissionScopeKey` 截断风险 | `permission.ts:310,325` | 对 Bash scopeKey 使用 `normalizeScopeText` 的前 64 字符 + `crypto.subtle.digest('SHA-256')` 的 hex 前 16 字符组合，消除截断碰撞 |
| **P2-2** | `categorizeBash` 引号内运算符误判 | `permission.ts:204` | 添加引号感知的运算符检测（skip content inside single/double quotes for `[;&|]` patterns），减少 excessive prompt |
| **P2-3** | `categorizeBash` Unicode 安全审计 | `permission.ts:221` | 添加 null byte 清洗、RTL override 检测、同形异义词规范化 |
| **P2-4** | Permission 队列公平性 | `permission-engine.ts:44` | 当前 `state.parked` 是 `Map`（insertion order），但用户可能先回答后弹出的 permission dialog。考虑添加 UI 侧的 request ordering hint |
| **P2-5** | 监控/告警 | 全局 | 添加 metric：`permission_parked_duration_ms` histogram、`permission_endturn_rejected_total` counter，用于生产环境观测 |

---

> **结论**：当前 Permission / Tool Safety 子系统设计扎实——`preToolUse` 纯函数 3×11 矩阵、`categorizeBash` 6 层分类、`resolveWritableInsideCwd` 双重 `realpath`+`isInside`、JSONL 先写后审的崩溃恢复策略，构成了多重纵深防御。唯一的 **真实并发竞态** 是 `StreamWatchdog.pause()/resume()` 的共享单例在并行 tool call 场景下的不配对问题（R1），这在 Vercel AI SDK 支持 parallel tool calls 的背景下应被视为 P0 风险。Permission 请求无超时（B1）和 Bot 模式 permissionMode 未强制约束（B2）是第二梯队的设计缺口。
