# Maka Renderer / UI 粗读报告

> 阅读基线: `335220a` | 深度档位: `architecture` | 语言: 中文

---

## 1. problem

Maka renderer/UI 层的核心使命是：**把 agent runtime 的内部状态（streaming text delta、tool output chunk、permission request、session lifecycle、connection health、artifact file list）映射为可视化、可交互的 surface，让用户在不接触 raw event stream 的情况下理解"agent 此刻在做什么、接下来可以做什么"。**

它不是普通聊天 UI，原因有五：

1. **多路并发状态**：同一个 session 里同时存在 assistant streaming text、tool output streaming chunks、thinking (reasoning) text、pending permission request、in-flight tool activity — 必须在一个 chat surface 上并行呈现，而不是简单的 `user → assistant` 两条线。
2. **事件驱动 + 状态推导**：renderer 不直接调用 LLM；它订阅 `text_delta` / `tool_start` / `tool_output_delta` / `permission_request` / `error` / `complete` / `abort` 等 `SessionEvent` 流，从离散事件中推导出 `streamingBySession`、`thinkingBySession`、`liveToolsBySession`、`permissionBySession` 等复合状态。
3. **安全边界**：renderer 是"不可信客户端"——主进程 (`main`) 负责路径安全、凭据存储、MIME sniffing；renderer 通过 `window.maka.*` preload API 获取数据（如 `window.maka.artifacts.readText`），从不直接触碰文件系统路径。同时 renderer 在自己的 state 层做二次 `redactSecrets`、per-delta cap、per-session total cap，防御 streaming text 中可能漏过来的原始凭据。
4. **多 surface 导航**：会话列表 (Chats/Pinned/Archived 分组) + 计划提醒 (plan reminders) + 技能库 (skill library) + 每日回顾 (daily review) + 设置 modal + 键盘快捷键 modal — 同一应用在一个双向格布局中承载 5 个模块 + 3 种模态。
5. **Visual smoke / 确定性截图**：内置 `MAKA_VISUAL_SMOKE_FIXTURE` 机制可以注入预置 session/message/tool/theme 状态，冻结 `Date.now()`、暂停动画、锁定 locale/timezone，输出可 hash 对比的截图 baseline —— 这是 UI 回归测试的基础设施，也是普通聊天 UI 没有的能力。

---

## 2. why_hard

### 2.1 Streaming 文本呈现

- **类型书写效果 (`smooth-stream`)**：`useSmoothStreamContent` 用 EMA (exponential moving average) 追踪每个 stream chunk 的到达速率 (CPS, characters per second)，在 `requestAnimationFrame` 中按平滑速率逐步输出 `displayedCount` 个 grapheme。必须处理：backlog snap（积累 >800 grapheme 直接跳转）、complete flush budget（stream 结束后最多 600ms 追上）、`prefers-reduced-motion` 旁路。
- **Grapheme-aware 切片**：`Intl.Segmenter(granularity: 'grapheme')` 确保 emoji ZWJ 序列、skin-tone modifier、flag 不会被切成半个字符。
- **C1 安全修复**：smooth stream 是逐前缀渲染，如果 raw text 中含 `Authorization: Bearer sk-abcdef`，前缀 `Authorization: Bearer s` 可能在 redaction 触发前短暂漏到屏幕。解决方法：`prepareSmoothStreamText` 对完整 raw text 先跑 `redactSecrets`，再交给 smoother —— 保证每个前缀都是已脱敏的。
- **跨 delta 秘钥重扫 (`assistant-stream`)**：streaming 自然会把密文跨 delta 切分（delta N: `sk-`, delta N+1: `abcdef`）。`applyAssistantDelta` 在每个 delta 上做 per-delta redaction，然后做 post-append 全量 redaction —— 三次保证不把原始 secret 写入 React state。

### 2.2 Tool output streaming

- **实时终端输出**：工具调用 (`bash`, `python`, `browser` 等) 的 stdout/stderr 以 `tool_output_delta` chunk 流式推送，seq 单调递增，但网络可能乱序。renderer 做 dedup-by-seq + insert-sorted 确保视觉稳定性。
- **二级 redaction + cap**：`applyToolOutputChunk` 对每个 chunk 做 `redactSecrets`（工具 stderr 可能包含 bearer token / API key），单 chunk 不超过 4KB，单工具总 char ≤ 16KB，chunks 总数 ≤ 200 —— 防止失控工具冲垮 renderer memory。
- **状态提升**：`tool_start` 到达 → `pending` → `tool_output_delta` 到达 → `running` → `tool_result` → `completed/errored` → error/abort → `interrupted`。`upsertTool` 函数保证 `tool_start` 不会把已有 streaming output 的 running 工具退化为 pending。

### 2.3 Artifact preview

- **多类型路由**：`file` (plain text pre), `diff` (line-tagged add/del/hunk), `html` (sandboxed iframe `sandbox="allow-scripts"`, 外部链接计数与 status bar), `image` (registry-based MIME allowlist + base64 cap), `pdf` (embed + fallback text)。
- **Registry resolution (`artifact-preview-registry`)**：先 MIME allowlist 匹配（5 种: png/jpeg/gif/webp/avif），再 ext fallback（.png/.jpg/.jpeg/.gif/.webp/.avif），最后 `oversize` / `mime_disallowed` / `no_mime_no_ext`。SVG 明确递延——sanitizer 复杂度高。
- **L2 cap 防御**：`IMAGE_PAYLOAD_MAX_BYTES = 2MB`，base64 串长度检查 `> IMAGE_PAYLOAD_MAX_BASE64_LENGTH` 使用 O(1) string length 比，不调用 `atob` 解码。
- **路径安全边界**：artifact pane 从未组装绝对路径——所有读取走 `window.maka.artifacts.readText/readBinary`，主进程做 `realpath` 前缀检查。HTML preview `sandbox="allow-scripts"` 无 `allow-same-origin`。

### 2.4 Settings

- **多功能页面**：设置 modal 内部有 17 个 section（模型/使用统计/bot对话/网络/开放网关/关于/通用/主题/个性化/数据/账号/权限/健康/每日回顾/记忆/语音模型/联网搜索），每个 section 有自己的 `SettingsPage` 子组件。
- **凭据 UX**：`PasswordInput` 组件用于 API key 输入（不可见、可切换）；OAuth 扫描登录（微信 bot、企业微信 bridge —— 二维码轮询、状态机: fetching → waiting → expired → confirmed）。
- **状态互通**：关闭 Settings modal 后触发 `onboarding.refresh()` + `refreshMemoryActive()`，因为用户可能刚设置了默认连接或 MEMORY.md。

### 2.5 Session status

- **多状态映射**：`active | running | waiting_for_user | blocked | review | done | archived | aborted` 各有一个 `SessionStatusPresentation { label, tone, interactive }`。
- **分组顺序锁定**：`Pinned → Running → Waiting → Blocked → Active → Review → Done → Archived → Aborted`，`pinFirst` 模式将 flagged session 提升到独立的 Pinned 组。
- **Blocked reason 中文化**：`describeBlockedReason` 将 `SessionBlockedReason` 枚举（`NO_REAL_CONNECTION`, `auth`, `permission_required`, `tool_failed`, `unknown`）转为中文描述，UI 绝不经由直接暴露枚举标识符。

### 2.6 Command palette

- **混合模式**：⌘K palette 同时包含静态 action（新建对话、切换主题、打开设置…）和动态 session 搜索（fuzzy subsequence match），还通过 `useThreadSearch` 钩子接入 `window.maka.search.thread()` 跨会话内容搜索。
- **Deps 注入设计**：`buildCommandList` 接收回调注入（`onTestConnection`、`onOpenWorkspace`、`onCopyEnvSummary` 等），palette 组件本身不绑定任何 IPC —— 纯 presentational。
- **权限切换**：⌘K 可直接切 `explore → ask → execute` permission mode。

### 2.7 Accessibility

- **`useModalA11y`**：focus trap、Esc 关闭、Tab 循环、关闭后 focus 恢复 —— Settings、Command Palette、PermissionDialog、SearchModal、KeyboardHelp 共用。
- **单 tab stop 列表**：ArtifactPane list 是单 tab stop，ArrowUp/Down 导航，Enter 聚焦预览区域 —— 遵循 WAI-ARIA listbox pattern。
- **Status/live regions**：HTML artifact external link bar 用 `role="status" aria-live="polite"`，preview loading 用 `role="status"`。

---

## 3. design_approach

### 3.1 数据获取：preload → main IPC

renderer 通过 `window.maka.*` 访问主进程：

```
window.maka.sessions.list()           // 获取会话列表
window.maka.sessions.readMessages(id) // 读取消息
window.maka.sessions.subscribeEvents(id, callback)  // 订阅实时事件流
window.maka.sessions.subscribeChanges(callback)     // 订阅粗粒度变更
window.maka.connections.list()        // 获取模型连接
window.maka.settings.get()            // 获取所有设置
window.maka.artifacts.list(sessionId) // 获取 artifact 列表
window.maka.artifacts.readText(id)    // 读取文本 artifact
window.maka.artifacts.readBinary(id)  // 读取二进制 artifact
window.maka.search.thread(request)    // 跨会话内容搜索
window.maka.onboarding.getSnapshot()  // 获取 onboarding 状态
window.maka.visualSmoke.getState()    // 获取 visual smoke fixture 状态
window.maka.quickChat.start(input)    // 快速开始对话
window.maka.app.info()                // 应用/平台信息
window.maka.app.openPath(kind)        // 打开系统目录
window.maka.app.openArtifactPath(id)  // 在 Finder 中打开 artifact
window.maka.app.saveArtifactAs(id)    // 另存 artifact
window.maka.memory.getState()         // 本地记忆状态
window.maka.skills.list()             // 技能列表
window.maka.plans.list()              // 计划提醒列表
```

**关键模式**：renderer 中的 `useEffect` + subscription 模型——`Sessions:subscribeEvents(sessionId, handler)` 返回 unsubscribe 函数，在 effect cleanup 时调用。`handleEvent` 函数是 streaming / tool / permission 状态的唯一写入点（`main.tsx:2021`）。

### 3.2 `packages/ui` 提供的可复用组件/stream utilities

| 文件 | 内容 |
|------|------|
| `components.tsx` | `SessionListPanel`, `ChatView`, `Composer`, `EmptyState`, `DailyReviewPanel`, `SkillLibraryPanel`, `SearchModal`, `PermissionDialog`, `useModalA11y`, `MakaUriContext`, `formatDailyReviewMarkdown`, `RelativeTime` |
| `assistant-stream.ts` | `applyAssistantDelta` — pure helper: per-delta redaction, per-delta cap, cross-delta redaction, per-session total cap (head-keep) |
| `thinking-stream.ts` (未直接列出但存在) | `applyThinkingDelta`, `applyThinkingComplete` — thinking text accumulation + cap |
| `smooth-stream.ts` | `useSmoothStreamContent` — typewriter smoothing via EMA CPS + `Intl.Segmenter` grapheme slicing; `prepareSmoothStreamText` — pre-smoothing redaction |
| `tool-output-stream.ts` | `applyToolOutputChunk` — pure helper: secondary redaction, per-chunk cap, dedup-by-seq, sort, per-tool count/total cap |
| `artifact-preview-registry.ts` | `resolvePreviewKind`, `decideImageReadOutcome`, `decideImagePostLoad`, `normalizeAllowedImageMime`, `exceedsImagePayloadCap` — pure classifiers for image artifact resolution |
| `maka-uri.ts` | `parseMakaUri`, `isMakaUri`, `isMakaUriCandidate`, `isSafeExternalScheme` — `maka://settings/<section>` 和 `maka://compose?text=...` 的 allowlist 路由器 |
| `materialize.ts` | `materializeTurns`, `materializeTools`, `materializeChat` — 从 `StoredMessage[]` + live tools 构造 `TurnViewModel[]` |
| `redact.ts` | `redactSecrets` — 全局 secret 掩码，被所有 streaming/text 路径引用 |

### 3.3 架构总览

```
┌────────────────────────────────────────────────────────┐
│  main.tsx (AppShell) — 中心状态 + IPC 绑定               │
│  ┌──────────┐ ┌────────────────┐ ┌──────────────────┐  │
│  │ settings/│ │artifact-pane   │ │session-status-*  │  │
│  │ modal    │ │+ preview       │ │+ grouping        │  │
│  └──────────┘ └────────────────┘ └──────────────────┘  │
│  ┌──────────┐ ┌────────────────┐ ┌──────────────────┐  │
│  │command-  │ │use-thread-     │ │other helpers     │  │
│  │palette   │ │search          │ │(theme, kb, etc)  │  │
│  └──────────┘ └────────────────┘ └──────────────────┘  │
│                          │                             │
│  uses ──────────────────┼───────────────────────────── │
│                          ▼                             │
│  @maka/ui (components.tsx + stream helpers)            │
│  @maka/core (types + utilities)                        │
└────────────────────────────────────────────────────────┘
```

---

## 4. code_walkthrough

### 4.1 主组件 `AppShell` (`main.tsx:250`)

`AppShell` 是 renderer 的根级状态管理容器（约 3240 行）。它的核心状态分为：

**Session + Message 状态**:
- `sessions: SessionSummary[]` — 从 `window.maka.sessions.list()` 获取，通过 `sessions:changed` 订阅更新
- `activeId: string | undefined` — 当前打开的 session
- `messages: StoredMessage[]` — 当前 session 的消息列表
- `streamingBySession: Record<string, {text, truncated}>` — 合并的 assistant streaming text + 截断标记（fixup v2: 原子 setState 替代两个独立 state）
- `thinkingBySession: Record<string, string>` — Anthropic extended thinking 文本
- `thinkingTruncatedBySession: Record<string, boolean>` — thinking 截断标记

**Live Activity 状态**:
- `liveToolsBySession: Record<string, ToolActivityItem[]>` — 实时工具活动（通过 `text_delta`/`tool_start`/`tool_result`/`permission_request`/`permission_decision_ack` 更新）
- `permissionBySession: Record<string, PermissionRequestEvent>` — 当前等待确认的权限请求
- `sessionEventHealthBySession: Record<string, SessionEventStreamSnapshot>` — 事件流健康监控

**UI Shell 状态**:
- `settingsOpen`, `helpOpen`, `paletteOpen`, `searchModalOpen` — 四种 modal 的可见性
- `navSelection: NavSelection` — 侧栏当前选中的模块 (sessions/automations/skills/daily-review)
- `themePref`, `density`, `themePalette` — 外观设置
- `appInfo`, `connections`, `defaultConnection`, `skills`, `planReminders` — 数据源

**派生状态 (useMemo)**:
- `streamingSessionIds` — 驱动侧栏 streaming pulse dot
- `staleSessionIds` — 驱动侧栏 "已过期" pill
- `sessionStatusGroups` — status-grouped sidebar (Pinned → Running → ... → Aborted)
- `chatConnectionAlert`, `chatEventStreamAlert` — chat header banners
- `turnFooterActionsByTurn`, `turnFailedReasonLabels`, `turnLineageBadgesByTurn` — per-turn footer actions

### 4.2 事件处理 (`handleEvent` at `main.tsx:2021`)

```typescript
function handleEvent(sessionId: string, event: SessionEvent) {
  switch (event.type) {
    case 'text_delta':
      // 通过 applyAssistantDelta 做: per-delta redact, per-delta cap,
      // cross-delta redact, total cap → setStreamingBySession
    case 'text_complete': clearStreaming + refreshMessages
    case 'thinking_delta':
      // applyThinkingDelta → setThinkingBySession + setThinkingTruncatedBySession
    case 'thinking_complete':
      // applyThinkingComplete → 替换而非追加 thinking text
    case 'tool_start': upsertTool → status: pending
    case 'tool_output_delta': appendToolOutputChunk → applyToolOutputChunk
    case 'permission_request': setPermissionBySession + upsertTool
    case 'permission_decision_ack': clear permission + upsertTool (running/errored)
    case 'tool_result': upsertTool (completed/errored) + refreshMessages
    case 'error': clearStreaming + toast + markInFlightToolsInterrupted
    case 'abort': clearStreaming + markInFlightToolsInterrupted
    case 'complete': refreshSessions + refreshMessages
  }
}
```

**关键安全模式**：`text_delta` 路径在 `setStreamingBySession` 之前经过 5 层处理：
1. per-delta `redactSecrets(rawDelta)` — 单 chunk 内 secrets
2. per-delta cap (tail-keep 4KB max)
3. append to previous text
4. post-append `redactSecrets(appended)` — 跨 delta secrets
5. total cap (head-keep 256KB max + "[…后续已截断]" marker)

### 4.3 设置页 (`settings/SettingsModal.tsx`)

`SettingsModal` 是 modal shell，实际 surface 由 `SettingsSurface` 管理：
- 左栏：导航分组 (基础 / AI / 集成 / 数据与账号 / 其他)，带 nav group summary (如 "模型" 组显示 default connection name + last test status)
- 右栏：`SettingsPage` 按 `section` 值分发到 17 个子页面
- `ProvidersPanel` (`settings/ProvidersPanel.tsx`) 处理模型连接 UI —— OAuth card、API key input、模型选择、连接测试
- `AccountSettingsPage` 使用 `deriveAccountAuthActions` / `presentAccountAuthState` 展示订阅/鉴权状态
- `ThemeSettingsPage` 使用 THEME_PALETTES 的 10 种 palette (default / onedark / catppuccin-mocha / tokyo-night / nord / coral / azure / forest / dusk / sand / mono) 做 `data-maka-theme` 属性切换

### 4.4 Artifact Pane (`artifact-pane.tsx` + `artifact-preview.tsx` + `artifact-preview-registry-shell.tsx`)

- `ArtifactPane`: 默认隐藏 (`return null` when `activeRecords.length === 0`)，有 artifact 时显示 360px 可折叠侧栏
- 列表通过 `window.maka.artifacts.list(sessionId)` 和 `window.maka.artifacts.subscribeChanges` 保持同步
- 选择 artifact → `ArtifactPreview` 按 kind 路由: `FilePreview` (text pre), `DiffPreview` (line-tagged), `HtmlPreview` (sandboxed iframe), `RegistryArtifactPreview` (image registry), `PdfPreview` (embed + fallback)
- 工具栏: 在 Finder 中打开 / 另存为 / 复制文本 (仅 text kind) / 删除 (软删除 + 确认)
- a11y: 列表为单 tab stop，ArrowUp/Down/Home/End 导航，Enter 聚焦预览，Esc 折叠 pane 回到 composer

### 4.5 Assistant Stream (`packages/ui/src/assistant-stream.ts`)

```typescript
applyAssistantDelta(prev: string, rawDelta: string, options?): ApplyAssistantResult {
  // L1: per-delta redaction
  // L2: per-delta cap (tail-keep, head marker)
  // L3: append
  // L4: cross-delta redaction (重新扫 appended text)
  // L5: total cap (head-keep, tail marker)
  // short-circuit: 如果 buffer 已 capped，直接返回
}
```

cap 值: per-delta 4KB, per-session total 256KB (assistant text 从顶部阅读，所以用 head-keep)。

### 4.6 UI Tokens (`maka-tokens.css` + `styles.css`)

**6 色哲学**: `background` / `foreground` / `accent` (紫色) / `info` (琥珀) / `success` (绿色) / `destructive` (红色)。
所有衍生色通过 `color-mix(in oklch, ...)` 或 `oklch(from var(--foreground) l c h / alpha)` 生成，没有独立的 "gray" token。

**关键 token**:
- `--surface-canvas`: shell 最底层背景 (oklch(0.97 0.005 250))
- `--background`: 卡片/floating panel 背景 (oklch(0.985 0.003 250))
- `--foreground-N` (2%/3%/5%/10%/.../95%): solid mix scale
- `--border` / `--border-strong`: 低 alpha overlay
- `--ring`, `--hover`, `--active`: 交互状态

**Dark mode**: 通过 `.dark` class 切换，`--background` 变为 `oklch(0.21 0.006 250)`, `--surface-canvas` → `oklch(0.175 0.005 250)`。

**主题 palette**: 10 种通过 `[data-maka-theme="..."]` 属性激活，覆盖基础 6 色 + user bubble color。

**Typography**: Geist Variable (主字体) + Geist Mono Variable (等宽)，含显式中文 fallback chain (PingFang SC → Hiragino Sans GB → Microsoft YaHei → Noto Sans CJK SC)。

**Reduced motion**: `data-maka-reduced-motion="true"` + `@media (prefers-reduced-motion: reduce)` 两种触发方式。

**Visual smoke**: `data-maka-visual-smoke="true"` 暂停所有动画、transition 和 caret blink，实现确定性截图。

**UI Density**: `data-ui-density="compact|comfortable|spacious"` 三档，控制 `message-gap`, `message-pad-x/y`, `composer-pad-y`, `row-pad-y`。

---

## 5. flows

### Flow 1: New Session / Send Message

```
用户输入 text → Composer.onSend(text)
  → AppShell.send(text)
  → 无 activeId? → window.maka.sessions.create({name, permissionMode})
    → 获得 sessionId, turnId=uuid
    → 乐观显示 user message
    → window.maka.sessions.send(sessionId, {type:'send', turnId, text})
    → refreshMessagesUntilTurn (poll, timeout 1200ms)
  → subscribeEvents → handleEvent 处理后续 text_delta / tool_start / ...
    → streamingBySession 实时更新 → SmoothStream → Markdown 渲染
    → text_complete → clearStreaming + refreshMessages → 持久化消息显示
```

### Flow 2: Stream Rendering

```
SessionEvent('text_delta', {text})
  → handleEvent(sessionId, event)
  → applyAssistantDelta(prev, event.text)
    L1: redactSecrets(rawDelta)
    L2: per-delta cap (4KB tail-keep)
    L3: prev + delta
    L4: redactSecrets(appended)  ← cross-delta secret catch
    L5: total cap (256KB head-keep + "[…后续已截断]")
  → setStreamingBySession({...current, [sessionId]: {text, truncated}})
  → ChatView receives `streamingText={activeStreaming}`
  → prepareSmoothStreamText(streamingText) ← pre-smoothing redaction
  → useSmoothStreamContent(preparedText, {streaming: true})
    → EMA tracks CPS → RAF advances displayedCount
  → <ReactMarkdown>{displayed}</ReactMarkdown>
    → Markdown内部 link override:
      - isMakaUriCandidate? → parseMakaUri → dispatchMakaUri or 显示broken-link
      - isSafeExternalScheme? → <a target=_blank>
      - 否则 → broken-link inline error
```

### Flow 3: Tool Output Rendering

```
SessionEvent('tool_start', {toolUseId, toolName, displayName, intent, args})
  → upsertTool(sessionId, toolUseId, {status:'pending', toolName, displayName, args})

SessionEvent('tool_output_delta', {toolUseId, seq, stream, chunk, redacted, createdAt})
  → appendToolOutputChunk(sessionId, toolUseId, chunk)
  → applyToolOutputChunk(base.outputChunks, chunk)
    → dedup-by-seq → redactSecrets(chunk) → per-chunk cap (4KB tail-keep)
    → insert-sorted → drop oldest if count>200 or totalChars>16KB
  → ChatView renders ToolOutputStream: 按 toolUseId 分组显示
    - status dot (pending/running/completed/errored/interrupted)
    - outputChunks 文本（带 redacted 标记）
    - streamingTypeBar（toolName + intent + duration + truncated pill）

SessionEvent('tool_result', {toolUseId, status:'completed'/'errored', content, durationMs})
  → upsertTool(..., {status, result:content, durationMs})
  → refreshMessages(sessionId) → tool results 写入持久化消息

如果 session error/abort:
  → markInFlightToolsInterrupted(sessionId)
  → 所有 pending/running/waiting_permission 工具 → interrupted
```

### Flow 4: Artifact Preview

```
ArtifactPane.mount → sessionId change
  → window.maka.artifacts.list(sessionId) → setRecords(next)
  → 订阅 artifacts:changed → on('created'/'deleted'/'purged') → refresh()
  → 默认选中最新 artifact

用户点击 artifact row → setSelectedId(id)
  → ArtifactPreview 按 record.kind 分支:
    - file: useTextRead → loading→ready → <pre>{text} 或 TextFailureCard
    - diff: useTextRead → <pre> with line-tagged spans (add/del/hunk/meta/ctx)
    - html: useTextRead → sandboxed <iframe srcdoc> + external link count bar
    - image: RegistryArtifactPreview
      → resolvePreviewKind(input) → {kind:'image', reason:'mime_match'|'ext_fallback'}
       或 {kind:'unsupported', reason:'...'}
      → readBinary → decideImageReadOutcome → decideImagePostLoad
      → <img src="data:<safeMime>;base64,..." />
    - pdf: useBinaryRead → <embed type="application/pdf" src="data:...base64" />

工具栏:
  → 在 Finder 中打开: window.maka.app.openArtifactPath(id)
  → 另存为: window.maka.app.saveArtifactAs(id)
  → 复制文本 (仅 text kind): window.maka.artifacts.readText(id) → clipboard
  → 删除: toast.confirm → window.maka.artifacts.delete(id) → refresh
```

### Flow 5: Provider Settings

```
⌘, or sidebar "设置" → setSettingsOpen(true) → <SettingsModal>
  → load: window.maka.settings.get() → settings state
  → left nav: 17 sections, grouped by 5 categories
  → 选择 section → SettingsPage 分支:
    models → <ProvidersPanel bridge={window.maka.connections}>
      → 获取 connections, provider defaults, oauth state
      → 用户操作: add/fill api key / test / setDefault / setModel
      → 每个操作通过 bridge → window.maka.connections.* IPC
    account → <AccountSettingsPage>
      → deriveAccountAuthActions (oauth login/logout/refresh)
      → presentAccountAuthState (status badge + error copy)
    theme → <ThemeSettingsPage>
      → toggle light/dark/auto → onThemeChange
      → palette cards (10种) → onThemePaletteChange
      → density radio group → onDensityChange
      → 所有更改即写 localStorage + 即时 CSS 属性生效

关闭 settings → closeSettings()
  → onboarding.refresh() (重新检查 first-run 状态)
  → refreshMemoryActive() (重新检查 MEMORY.md 启用状态)
```

### Flow 6: Thread Search / Status

```
sidebar "搜索" 按钮 → setSearchModalOpen(true) → <SearchModal>
  → 内部 useThreadSearch(query) hook:
    query.length < 2 → idle
    query.length >= 2 → debounce 180ms → window.maka.search.thread({query, limit:10})
      → IPC 返回 SearchResult[] 或 {ok:false, reason, message}
      → incognito_active → blocked state (显示 "搜索已在隐私模式下停用")
      → 其他错误 → error state
      → 成功 → normalizeHits (过滤无 sessionId / invalid target)
      → results state
  → 用户选择结果 → onNavigateToSession(sessionId, turnId)
    → openSessionInChat(id, turnId) → setSearchScrollTarget + setActiveId
    → ChatView 根据 scrollTargetTurn 自动滚动到对应 turn

Session status flow:
  sessions 列表 → deriveSessionStatusGroups(sessions, {pinFirst: true})
  → 状态组: Pinned → Running → Waiting → Blocked → Active → Review → Done → Archived → Aborted
  → SessionListPanel 按组渲染，每个组有 collapsible/defaultExpanded (仅 archived/aborted 可折叠)
  → 每个 row 渲染 status icon + name + time + stale/streaming badge + action overlay
```

---

## 6. tests

### 6.1 现有测试覆盖

| 测试文件 | 覆盖范围 |
|----------|---------|
| `smooth-stream.test.ts` | EMA update, frame advance, grapheme segment, backlog snap, initial display count |
| `assistant-stream.test.ts` | per-delta redact, per-delta cap, cross-delta redact, total cap, short-circuit |
| `tool-output-stream.test.ts` | dedup-by-seq, sort, redact, per-chunk cap, per-tool total cap, per-tool count cap |
| `thinking-stream.test.ts` | thinking delta apply, complete replace, truncation |
| `maka-uri.test.ts` | parse `maka://settings/...`, `maka://compose?text=...`, invalid inputs, unsafe schemes |
| `artifact-preview-registry.test.ts` | MIME match, ext fallback, oversize, mime_disallowed, no_mime_no_ext, decideImagePostLoad |
| `session-status-presentation.test.ts` | all 8 status presentations, blocked reason localized copy, aria label composition |
| `session-status-grouping.test.ts` | 8-group ordering, pinFirst pulling flagged to top, empty groups dropped |
| `chat-header-alert.test.ts` | connection alert derivation per backend/lastTest/connectionReady |
| `stale-sessions.test.ts` | stale classification (backend fake / slug not found) |
| `branch-banner.test.ts` | parent session name derivation |
| `command-palette-*.test.ts` (3 files) | session navigation contract, a11y copy contract, plan reminder contract |
| `session-row-actions-fail-soft-contract.test.ts` | error handling on row actions |
| `session-open-routing-contract.test.ts` | session open routing edge cases |
| `session-event-health.test.ts` | event stream subscription/evaluation lifecycle |
| `use-thread-search.test.ts` | normalizeHits (filtering), ticket-based stale control, idle/loading/results/blocked/error states |
| `search-modal-lifecycle-contract.test.ts` | search modal mount/unmount behavior |
| `artifact-pane-lifecycle-contract.test.ts` | pane lifecycle, list refresh |
| `artifact-pane-layout.test.ts` | pane layout behavior |
| `artifact-list-keyboard.test.ts` | ArrowUp/Down/Home/End/Enter keyboard navigation in list |
| `visual-smoke-fixture.test.ts` | fixture state injection, theme override, active session |
| `renderer-error-boundary.test.ts` | ErrorBoundary behavior |
| `renderer-startup-fail-soft-contract.test.ts` | startup failure graceful handling |
| `account-auth-ui.test.ts` | auth UI state derivation |
| `bot-settings-ui-contract.test.ts` | bot settings UI contract |
| `local-memory-ui-contract.test.ts` | local memory UI contract |
| `quick-chat.test.ts` | quick chat functionality |

### 6.2 测试缺口

1. **没有 React 组件级渲染测试**：所有测试都是 pure function 测试或 contract 测试，没有使用 jsdom/react-testing-library 的组件级渲染/交互测试。`AppShell` send/stop/permission/composer 交互路径无自动化覆盖。
2. **没有 visual regression 框架集成**：虽然内置 visual smoke fixture 机制，但没有与 Percy/Chromatic 等云视觉回归服务的集成覆盖。
3. **Composer 组件测试缺失**：composer 的 draft 持久化、file import、folder outline import、drag-and-drop、multiline 行为没有测试。
4. **Settings modal 页面级测试缺失**：17 个 settings 子页面中，只有 models/account/bot/theme 有部分 contract 测试；usage/permissions/health/about/data/open-gateway/memory/voice/network/search 等无 UI 测试。
5. **Streaming UI 行为端到端测试缺失**：smooth stream 的 React hook (`useSmoothStreamContent`) 在 node:test 中有 pure helper 测试，但 hook 本身的 React lifecycle/scheduler 行为无覆盖。
6. **ToolOutputStream 组件测试缺失**：纯 helper `applyToolOutputChunk` 有测试，但实际组件的 streaming state 显示（status dot, chunk rendering, truncation pill）没有测试。
7. **Permission dialog 交互路径缺失**：allow/deny/timed out 的 IPC 回调和 UI 反馈路径没有测试。
8. **Daily review panel 测试缺失**：`DailyReviewPanel` 组件的范围切换、翻页、加载、空状态、错误恢复路径没有测试。
9. **ArtifactPreview HTML sandbox 无安全测试**：sandbox 属性是否正确应用、外部 link 计数准确性、`srcdoc` 注入路径没有自动化测试。
10. **键盘导航整体流程测试缺失**：从 session list → chat → composer → artifact pane 的 tab/arrow key 全程导航无测试。

---

## 7. risks

### 7.1 状态同步风险

- **`sessionsRef` + `activeIdRef` + `setState` 双写**：renderer 同时维护 `useState` 和 `useRef` 副本（如 `sessionsRef`, `activeIdRef`, `messageRetryPendingRef`）。Ref 用于异步回调中获取最新值，State 驱动渲染。两者不同步是经典的 stale-closure 风险点。代码中通过 `activeIdRef.current = activeId` 在 state setter 中同步更新 ref，但耦合面大。
- **optimistic user message 与真实消息竞态**：`showOptimisticUserMessage` 创建临时消息 ID `optimistic-user-${turnId}`，`refreshMessagesUntilTurn` 轮询等待持久化消息覆盖。如果 send IPC 成功但 poll 循环被 disposed，会留下孤儿 optimistic message。

### 7.2 长输出性能

- **Markdown 渲染**：streaming assistant 回答可能很长（256KB cap），`ReactMarkdown` + `remarkGfm` + `remarkBreaks` + `rehypeHighlight` 的 re-render 在每次 smooth-stream 帧更新时都会触发，在大文档上可能掉帧。
- **`rawGraphemes = segmentGraphemes(rawText)` 在每次 rawText 变化时全量重算**：虽然注释说 "few KB" 很快，但在 256KB 上限和带大量 emoji 的情况下可能有微秒级以上的延迟。
- **`liveToolsBySession` 的 functional updater 模式**：每次 `appendToolOutputChunk` 都会调用 `applyToolOutputChunk`（遍历 chunks 列表、redact、sort)，高频输出场景下可能有 CPU 压力。

### 7.3 a11y

- **`useModalA11y` 的 `getFocusable` 使用 `offsetParent` 检查可见性**：在 fixed-position / container query 布局中 `offsetParent === null` 不总等于不可见 —— 可能导致真正可聚焦的元素被过滤。
- **sidebar row 使用 `:focus-within` 驱动 action overlay 显示**：这是视觉 hack，但不提供 ARIA 等价标记来描述 actions 的可用性——屏幕阅读器用户在行上聚焦时不会被告知有 flag/archive/rename/delete 操作可用。
- **ArtifactPane preview 区域有 `role="region" tabIndex={-1}` 但当 selected 为 null 时仍存在**：`aria-label="生成文件预览"` 在空状态下信息不准确。

### 7.4 Visible copy hygiene

- **多路 redaction 路径依赖 `redactSecrets` 函数的正确性**：text_delta, tool_output_chunk, thinking, assistant text, smooth stream, conversation export 全部经过至少一次 `redactSecrets`。如果函数有漏检模式，受影响的 surface 面非常广。
- **Conversation export (`renderConversationMarkdown`)** 明确排除 thinking block 和 tool results，但包含 `tool_intent` 和 `assistant text` 都做 secondary redaction。User text 不做 redaction —— 这是安全的（用户自己输入的），但注释应更醒目。
- **`window.maka.artifacts.readBinary` 返回的 base64 数据** 一度直接进入 React state（已修复：现在 `decideImageReadOutcome` 在 async callback 内、`setState` 前运行 cap 检查）。

### 7.5 Runtime truth vs display text

- **Session status 是 runtime truth，但 renderer 有 "对用户友好" 的转换**：`describeBlockedReason` 把 `NO_REAL_CONNECTION` 映射到 "等待配置可用模型连接"，`describeTurnErrorClass` 把 `errorClass` 映射到中文短语。这些映射表是隐式 contract——如果 runtime 加了新 enum 值但 renderer 未更新映射，会 fallback 到 `"未知错误"` / `"运行中断，可重试"`。
- **`thinkingBySession` 在 `clearStreaming` 时清空**：但如果 session 在 `thinking_complete` 后、`text_complete` 前被 abort，thinking text 可能残留。代码中 `abort` case 调用了 `clearStreaming`，这也会清空 thinking，路径正确但依赖 `clearStreaming` 的 side effect 是被注释表明但非类型约束的。

### 7.6 Settings credential UX

- **API key 输入用 `PasswordInput` 组件**：组件使用 `type="password"` 可切换可见性。但 React DevTools 中 state 可能暴露明文 key（取决于 DevTools 是否打开）。
- **OAuth flow 的 `window.maka.settings.bots.wechat.fetchQrcode/pollQrcodeStatus`**：二维码 URL 和 qrToken 在 React state 中短暂存在，但注释说扫完即清。生命周期正确清理了 polling interval。
- **`refreshMemoryActive()` 在关闭 Settings 时调用**：这是合理的 (user 可能刚切换了 agentReadEnabled)，但用 `window.maka.memory.getState()` 读取本地文件状态，如果文件很大（如参考设计中的 MEMORY.md 多页），这个调用会拉取文件内容到主进程（用于判断 status 和 content.trim().length > 0），可以优化为只读 metadata。

---

## 8. next_questions

1. **ChatView 组件的完整渲染树**：当前报告聚焦 `main.tsx` AppShell 和 `packages/ui` 导出接口，未深入 `<ChatView>` 内部的 TurnView / ReasoningPanel / ToolOutputStream / Markdown 渲染细节。建议下一轮精读 `packages/ui/src/ChatView.tsx`（或等效文件），分析：turn 分组的 DOM 结构、thinking panel 的展开/折叠交互、tool card 的 streaming output 展示、permission dialog 的 UI state machine、markdown link intercept 在 ChatView 内的具体执行。

2. **Composer 组件的状态机和草稿持久化**：draft 如何通过 `draftKey={activeId ?? 'new-session'}` 持久化到 localStorage？文本文件/文件夹导入的 prompt 构建策略（截断规则、preflight 检查）在 UI 层的展示细节？Stop 按钮的 debounce/pending 机制与 Escape key 的交互？

3. **Visual smoke 基础设施**：fixture 是如何从 test 进入 `window.maka.visualSmoke.getState()` 的？capture pipeline 在 main 侧的截图/保存路径？如何与 CI 集成做像素级 diff？建议交叉确认 `desktop-main-ipc` 节点中 `visualSmoke` channel 的契约。

4. **Preload API 契约与 renderer 耦合**：`window.maka.*` 每个 API 的返回类型（`Result<T, E>` pattern）在 preload 中的类型定义？renderer 的 error handling 依赖 `generalizedErrorMessageChinese` 和 `cleanErrorMessage` 来做 message deduplication，这些 helpers 是否能涵盖所有 IPC 失败模式？建议交叉确认 `desktop-main-ipc` 节点中每个 channel 的错误形状。

5. **深层状态推进**：当用户快速切换 session（`activeId` 变化），旧的 `subscribeEvents` cleanup 和新 session 的 subscription/messages load 之间的竞态处理是否正确？`disposed` flag 模式和 `activeIdRef.current === activeId` guard 是否能覆盖所有 edge case？

6. **Performance profiling**：`useSmoothStreamContent` 的 RAF 循环在 session 切换时是否正确取消？频繁的 Markdown re-render 是否需要 `React.memo` / `shouldComponentUpdate` 优化？`materializeTurns(messages, liveTools)` 在 200+ 消息的 session 中的性能？建议用 React Profiler + Lighthouse trace 做量化分析。

7. **Bot chat 设置 UI**：Bot channel 配置的 UI 涉及多种平台（Telegram/飞书/企业微信/微信/Discord/钉钉/QQ），`BotWeChatFields` 的高级设置展开/折叠状态是否正确持久化？`WeChatScanLoginModal` 的轮询/过期/确认状态机的 UX 是否经过手动测试验证？建议交叉确认 `desktop-main-ipc` 中 `bots.*` API 的状态机。

8. **Search 模态与 Command Palette 的搜索去重**：`SearchModal` 和 `CommandPalette` 都使用 `useThreadSearch`，但它们的 lifecycle 不同（modal 条件挂载 vs palette 始终内部持有一个 hook 实例）。是否有 query 跨组件泄露的风险？建议下一轮验证 `createThreadSearchPoller` 的 ticket 机制在两个调用源下并发行为。
