# 10 — Visual Smoke / Test Infrastructure 精读报告

> 基线：`335220a` | 深度档位：maintainer | 生成：2026-06-13

---

## Scope

本报告覆盖 Maka 桌面应用的 **视觉烟雾测试 (Visual Smoke)** 与 **工程质量基础设施 (Test Infrastructure)**，聚焦以下系统：

1. **自动化截图管线**：`capture-screenshots.mjs` (驱动) + `visual-smoke-fixture.ts` (种子数据)
2. **截图差异门禁**：`diff-screenshots.mjs` (Stage 1 硬故障门禁)
3. **可访问性审计**：`check-a11y.mjs` (静态源码扫描)
4. **控制台审计**：`check-console.mjs` (console.* 调用站点白名单)
5. **构建卫生检查**：`check-stale-dist.mjs` (源码 vs 产出时间戳对比) + `check-officecli-bundle.mjs` (打包二进制校验)
6. **手动冒烟路径**：`smoke.md` (18 条 Path，含 Pass/Fail 信号)
7. **真实窗口冒烟**：`desktop-real-window-smoke.mjs` (人机交互 + 程序化检查)
8. **计划文档**：`full-product-test-plan.md` + `ui-quality-plan.md` (交付契约)

**覆盖的 UI surfaces**：含 Sidebar、Settings 全 7 个子页、ArtifactPane、PermissionDialog、Chat header、Composer、OnboardingHero、Command Palette、Toast、ErrorBoundary、SearchModal、Session status grouping、Turn controls、Plan reminders、Artifact preview registry 等 25+ 个表面。

---

## Problem

Maka 是一个 **macOS 上的 AI 桌面代理**，其核心风险不是"代码逻辑写错了"，而是 **UI 表面在用户不知情的情况下向渲染器/远端暴露敏感数据**。传统的单元测试可以验证 "辅助函数输出正确"，但无法回答以下问题：

| 风险类别 | 典型场景 | 为什么普通测试抓不到 |
|---|---|---|
| **视觉回归** | Sidebar 滚动容器修复后，Settings footer 被推出屏幕 | 只有实际渲染 + 截图比对才能发现 |
| **主题/密度漂移** | dark 模式下某组件使用了硬编码颜色 | 需要 light/dark + 990/1280 多 variant 截图 |
| **沙箱逃逸** | ArtifactPane 的 iframe 被错误地设置了 `allow-same-origin` | 静态代码分析难以追踪运行时的 DOM 属性 |
| **a11y 退化** | icon-only button 缺少 `aria-label` | ESLint 插件太重；自定义脚本轻量但需要持续维护 |
| **密码泄露** | console.log 打印了 `sk-...` API key | 即使 CI 不会渲染 UI，console 审计必须作为硬故障门禁 |
| **构建陈旧** | 开发者改了 TS 源码但 dist 没重新编译 | 纯文件时间戳比对可以零开销发现 |
| **原生窗口行为** | titlebar drag region 回归导致窗口无法拖动 | 截图完全无法覆盖；必须真人交互 |
| **Release 组件缺失** | 打包时 OfficeCLI 二进制未下载 | 需要专门的 `check-officecli-bundle.mjs` |
| **Fixture 非确定性** | 截图宿主时区 / 语言不同导致 baselines 漂移 | locale / timezone 锁定 + sidecar 元数据确保可重复 |

这套测试基础设施的 **设计哲学** 是：
1. **最小依赖**（只用 Node 22 + Electron，不加 Playwright / JSDOM）
2. **分层隔离**（fast checks 每次跑，截图捕获按需跑，真实窗口 smoke 人工触发）
3. **防御深度**（不依赖单一检查层；console / a11y / stale-dist / screenshot-diff 各自覆盖不同盲区）
4. **确定性优先**（固定时钟、锁定 locale/timezone、reduced-motion variant、隔离 user-data-dir）

---

## Source Evidence

### 1. `scripts/capture-screenshots.mjs` — 自动化截图驱动

**路径**：`scripts/capture-screenshots.mjs:1-417`

**架构**：subprocess + stdout marker 模式。不使用 Playwright / CDP，只依赖 Electron 自身的 `webContents.capturePage()`。

**关键设计决策**：

- **Per-spawn 隔离 user-data-dir** (`line 237-244`)：每次 `(scenario, variant)` 组合使用 `os.tmpdir()` 下的独立目录，避免与开发者本机 Maka 窗口的 Electron singleton lock 冲突。没有这个设计，并发捕获会卡死。
- **Stdout marker 协议** (`line 144`)：正则 `/\[visual-smoke\] captured scenario=(\S+) variant=(\S+) path=(.+)$/` 是驱动脚本和主进程之间的唯一通信通道。
- **硬 60s 超时** (`line 143`)：每个 subprocess 最大存活 60 秒；超时后 `SIGKILL`。
- **250ms 轮询退出** (`line 272-277`)：一旦捕获到 marker，立刻 `SIGTERM` 子进程，不等 Electron 自然退出。
- **Local / timezone 锁定** (`line 157-168, 209-227`)：`MAKA_VISUAL_SMOKE_LOCALE`（默认 `zh`）和 `MAKA_VISUAL_SMOKE_TIMEZONE` 通过环境变量注入，确保跨宿主截图 baselines 一致。locale 通过 `resolveCaptureLocale()` 做 fail-closed 验证。
- **Sidecar 元数据** (`line 393-396`)：每张 PNG 旁边写入 `.meta.json`，记录实际使用的 locale。`diff-screenshots.mjs` 读取这些 sidecar 构建 manifest。
- **直接调用守卫** (`line 408-411`)：`isDirectInvocation` 确保 `main()` 只在被 `node` 直接执行时运行，导入 helpers（如 `resolveCaptureLocale`）不会触发 CLI 入口检查。

**Variant 矩阵** (`line 128-141`)：8 个 variant = `{light, dark} × {1280, 990} × {motion, reduced-motion}`。每个 scenario 产生 8 张 PNG。

**Scenario 覆盖** (`line 59-126`)：26 个 scenario，从 `first-run` 到 `sidebar-row-actions-visible`，覆盖：
- 首次启动、模型管理、连接错误、turn narrative
- Artifact pane（正常 + 错误 + 三种 preview 类型）
- Settings 7 个子页
- 侧边栏长列表（60 sessions）及其 SearchModal + row-actions 变体
- Stale sessions、Workstation statuses、Plan reminders
- Turn control history（primary + branch-visible + branch-orphan）

### 2. `scripts/diff-screenshots.mjs` — 截图差异门禁

**路径**：`scripts/diff-screenshots.mjs:1-409`

**Stage 1 范围**（明确声明, line 474-478）：**不做像素级对比**。只做三类硬故障检查：
- missing PNG
- corrupt PNG (magic bytes 不匹配)
- wrong dimensions (与 variant 声明的 viewport 不匹配, 含 1x/2x Retina)

**Tolerance 分层** (`line 66-81`)：
- 静态场景（`DEFAULT_SIZE_TOLERANCE`）: ±15% size drift
- 动态场景（`streaming-sidebar`, `permission-destructive`）: ±25% size drift
- Size drift 超过 tolerance 是 **warning**（不阻塞），但 `wrong_dimensions` 是 **hard fail**。

**Stable subset** (`line 91`)：`artifact-pane`, `first-run`, `artifact-errors` 三个场景被标记为 "stable"。这是 Stage 1 baseline 部署的第一批。

**Manifest 模式** (`line 200-250`)：
- `--manifest`：生成当前截图的 `manifest.json`（不含对比）
- `--update-baseline`：将当前截图提升为 baseline
- `--subset stable`：只检查 stable 子集

**Manifest schema** (`line 243-249`)：
```json
{
  "schemaVersion": 1,
  "capturedAt": "ISO-8601",
  "mainSha": "main.js sha256[:12]",
  "variants": ["light-1280-motion", ...],
  "entries": [{ scenario, variant, theme, viewport, reducedMotion, ok, locale, dimensions?, bytes?, reason? }]
}
```

**Pixel-level diff 路线图** (`line 13-14, 273-288`)：PR-IR-02 v3 计划引入 `pixelmatch` + `pngjs`，先在 stable subset 上 pilot，支持 calibrated tolerance + ignored dynamic regions。

### 3. `scripts/check-a11y.mjs` — 可访问性审计

**路径**：`scripts/check-a11y.mjs:1-416`

**架构**：walk `apps/` + `packages/` 下所有 `.tsx` 文件（排除 `__tests__/`, `dist/`, `node_modules/`），对每一行运行 6 条规则。

**6 条规则**：
| 规则 | 检查内容 | 为什么重要 |
|---|---|---|
| `icon-only-button` | `<button>` 只有 icon children 且无 `aria-label` | 屏幕阅读器无法读出按钮用途 |
| `positive-tabindex` | `tabIndex={N}` with N>0 | 破坏自然 DOM Tab 顺序 |
| `dialog-missing-label` | `role="dialog"` 无 `aria-label`/`aria-labelledby` | 屏幕阅读器只读 "dialog"，无上下文 |
| `input-missing-label` | `<input>`/`<textarea>` 无 label/placeholder | 用户不知道输入框的用途 |
| `icon-only-link` | `<a href>` 只有图标无文本 | 与 icon-only-button 同理 |
| `english-aria-label` | `aria-label`/`title`/`placeholder` 为纯英文 | Maka 默认中文 UI；英文 label = 未翻译 |

**豁免机制** (`line 24-25`)：`// a11y-allow: <reason>` 注释在源码行内，reviewer 可以看到豁免理由，规则本身不维护豁免列表。

**为什么不用 ESLint** (`line 10-12`)：保持工具面最小化，50 行检查不拖入 shared config + parser deps。

### 4. `scripts/check-console.mjs` — 控制台审计

**路径**：`scripts/check-console.mjs:1-127`

**风险**：`console.log(errorMessage)` 可能把 `sk-...` API key、raw provider error body、session 内部数据打印到 stdout/stderr。这些日志可能在 CI 日志中可见，或通过 Electron 的 renderer DevTools 泄露。

**机制**：文件级白名单 (`line 32-61`)。允许的文件包括：
- `error-boundary.tsx`（React 错误边界，DevTools-only）
- `main.ts`（dev-gated by `VITE_DEV_SERVER_URL` / `NODE_ENV`）
- `onboarding-service.ts`（只 log error class，不含消息体/密钥字节）
- `bot-registry.ts` / `record-llm-call.ts` / `record-tool-invocation.ts`（通过 `generalizedErrorMessage` 路由）
- 脚本自身

### 5. `scripts/check-stale-dist.mjs` — 构建卫生检查

**路径**：`scripts/check-stale-dist.mjs:1-95`

**机制**：对每个 workspace package 比较 `src/` 下最新 `.ts(x)` 的 mtime 与 `dist/` 下最新 `.js/.mjs/.cjs/index.html` 的 mtime。如果源码更新，标记为 stale。

**Desktop 特殊处理** (`line 12-16`)：`apps/desktop` 有两个独立输出目录 (`dist/main`, `dist/preload`, `dist/renderer`) 和对应的源目录。分别配对检查，避免 renderer 编辑触发 main dist 重建的误报。

### 6. `scripts/check-officecli-bundle.mjs` — OfficeCLI 打包验证

**路径**：`scripts/check-officecli-bundle.mjs:1-72`

**检查内容**：验证 `apps/desktop/resources/tools/<binary>` 存在、可读、可执行（非 Win32），且版本号与 `bundled-tools.json` 中的声明一致。只在同平台同架构时才运行实际版本检查。

### 7. `apps/desktop/src/main/visual-smoke-fixture.ts` — Fixture 种子引擎

**路径**：`apps/desktop/src/main/visual-smoke-fixture.ts:1-1934`

**核心架构**：

- **`resolveVisualSmokeFixture()`** (`line 151-182`)：从环境变量解析 fixture 配置，fail-closed 验证所有输入。packaged build 拒绝所有 fixture env vars。
- **`getVisualSmokeState()`** (`line 265-418`)：将 scenario + flags 映射为 `VisualSmokeState`，包含 `activeSessionId`, `openSettingsSection`, `sidebarSection`, `streamingBySession`, `permissionBySession`, 等运行时状态。
- **`seedVisualSmokeFixture()`** (`line 420-490`)：在 `workspaces/visual-smoke-<scenario>/` 下重建完整工作区状态，包括 settings.json、llm-connections.json、sessions/ 目录（含 `session.jsonl`）、artifacts/ 目录（含 `metadata.jsonl` + 实际文件）。

**关键安全约束**：
- Dev/test-only（packaged build 拒绝 fixture）
- 隔离工作区（不触碰真实用户数据）
- 伪凭证（`fixture-key-<slug>`，永不为真实 API key）
- `relativePath` 必须从 `visual-smoke-artifact/` 开始，不得为绝对路径
- Plan reminders、stale sessions、turn control history 均有专门的 on-disk 种子

**确定性保证**：
- 固定时钟：`Date.UTC(2026, 4, 22, 3, 0, 0)` (`line 101`)
- Reduced motion：`data-maka-reduced-motion="true"` CSS 路径
- Theme override：`MAKA_VISUAL_SMOKE_THEME` env → `data-maka-theme`
- Locale 锁定：`MAKA_VISUAL_SMOKE_LOCALE` env → `data-maka-visual-smoke-locale`
- Timezone 锁定：`MAKA_VISUAL_SMOKE_TIMEZONE` env → `data-maka-visual-smoke-tz`
- Auto-capture variant 验证：`[a-zA-Z0-9._-]+`，无 `/`，无 `..`

### 8. `apps/desktop/tests/smoke.md` — 手动冒烟路径

**路径**：`apps/desktop/tests/smoke.md:1-1656`

**18 条路径**：

| Path | 主题 | 类型 |
|---|---|---|
| 0 | 真实 Electron 窗口 smoke（PR-DESKTOP-SMOKE-0）| 人机交互 |
| 1 | 首次启动无真实模型 | 手动 |
| 2 | 添加连接并验证 | 手动 |
| 3 | 失败凭证在 chat header 中的展示 | 手动 |
| 4 | Streaming + 删除活跃 session 安全性 | 手动 |
| 5 | PermissionDialog destructive path | 手动 |
| 6 | ModelTable workspace (keyboard nav) | 手动 |
| 7 | Chat turn narrative (thinking block, tool summary chips) | 手动 |
| 8 | Sidebar streaming + multi-session indicator | 手动 |
| 9 | Command palette diagnostics + export | 手动 |
| 10 | Sandbox bridge sanity | 手动 |
| 11 | Artifact pane (HTML sandbox, diff, markdown, narrow, Esc) | 手动 |
| 12 | Sidebar "已过期" pill for stale sessions | 手动 |
| 13 | Artifact pane failure states + Save As | 手动 |
| 14 | Workstation sidebar status grouping | 手动 |
| 15 | Turn control contract API + UI (lineage badges, branch banner) | 手动 |
| 17 | UI trust-boundary + Settings persistence contracts (S1-S11) | 合同 |
| 18 | Computer Use overlay threat model (S12-S18) | 合同 |

Path 17 和 18 包含 **16 条合同不变式 (S1-S11 + S12-S18)**，每条都有：
- Contract invariant
- Targeted tests（node:test 文件名 + 用例名）
- Source-gate grep（reviewer 实际运行的 grep 模式）

### 9. `docs/full-product-test-plan.md` + `docs/ui-quality-plan.md` — 计划文档

**full-product-test-plan.md**：
- 定义 5 层测试（Core unit → Storage → Runtime → Desktop main/IPC → Renderer pure-helper）
- 定义 18 个 fixture scenarios 的必需要求
- 定义 5 个问题的 PR 描述模板（Contract / User Flow / Tests / Security / Not Included）
- 定义完整的 "Done" 标准

**ui-quality-plan.md**：
- 定义 12 门表面 gate（Contract / Pure helper test / Component contract / Fixture / Smoke path / Light+Dark / Narrow / Empty+Failure / a11y / Motion / i18n / Security）
- 定义 13 条跨表面 invariant（Focus / Keyboard / Motion / Density+Theme / Text+i18n / Boundaries / Trust hierarchy）
- 定义 25+ UI surfaces 的覆盖矩阵（每个 gate 的 ✅/⚙️/❌ 状态）
- 定义 6 个 PR-IR-XX 基础设施 PR
- 定义 Release no-go 条件

### 10. Package Scripts 集成

来自 `apps/desktop/package.json`：

```
pretest                  → check-console.mjs + check-a11y.mjs (fast gate)
test                     → build:main + node --test dist/main/**/*.test.js
screenshots              → full build + capture-screenshots.mjs --all (26×8=208 PNGs)
screenshots:single       → single scenario capture
screenshots:diff         → diff-screenshots.mjs (all scenarios)
screenshots:diff:stable  → diff-screenshots.mjs --subset stable (3×8=24 PNGs)
screenshots:baseline     → diff-screenshots.mjs --update-baseline
screenshots:baseline:stable → baseline promotion for stable subset
smoke:real-window        → desktop-real-window-smoke.mjs (human-in-the-loop)
smoke:programmatic-window → desktop-real-window-smoke.mjs --programmatic-only
```

来自 root `package.json`：

```
typecheck          → tsc --noEmit across workspaces
check:stale        → check-stale-dist.mjs
check:officecli-bundle → check-officecli-bundle.mjs
check:release      → check:stale + check:officecli-bundle
```

---

## Coverage Matrix

### Fixture Scenarios → UI Surface 映射

| Scenario | UI Surface(s) | Key Risk Mitigated |
|---|---|---|
| `first-run` | OnboardingHero, EmptyChatHero, Composer | 空白工作区渲染正确；不会错误显示 onboarding |
| `provider-workspace` | Settings · 模型, ModelTable | fetched model 列表渲染、keyboard nav、source label |
| `fallback-source` | Settings · 模型 | fallback 模型源的正确展示 |
| `fetched-empty` | Settings · 模型 | 0 models 状态不崩溃 |
| `connection-error` | Chat header alert, Settings · 账号 | 连接失败 banner + pill 颜色/文案正确 |
| `turn-narrative` | Chat surface, Turn block, Thinking block, Token chips | 多消息 turn 结构、thinking 折叠、token 摘要 |
| `artifact-pane` | ArtifactPane (html/diff/markdown), Save As | HTML sandbox iframe、diff 着色、markdown 渲染 |
| `artifact-errors` | ArtifactPane (deleted/unsupported/missing) | Tombstone 阻止读取、不支持的 MIME、文件缺失 |
| `streaming-sidebar` | Sidebar pulse dot, lastMessagePreview | Streaming indicator 优先于 unread |
| `permission-destructive` | PermissionDialog | 红色 destructive tone、Esc 禁用、"记住本轮" |
| `stale-sessions` | Sidebar stale pill, Chat header stale banner | FakeBackend/legacy 会话标记、active 行仍显示 pill |
| `settings-data` | Settings · 数据 | 数据设置页 render |
| `settings-personalization` | Settings · 个性化 | 个性化设置页 render |
| `settings-network` | Settings · 网络 | 网络设置页 render |
| `settings-bots` | Settings · 机器人对话 | Bot 设置页 render |
| `settings-about` | Settings · 关于 | 关于页 render |
| `settings-theme` | Settings · 主题 | 主题选择器 render |
| `settings-daily-review` | Settings · 每日回顾 | 每日回顾设置页 render |
| `module-skills` | Sidebar · Skills module | 技能模块 render |
| `module-daily-review` | Sidebar · Daily Review module | 每日回顾模块 render |
| `workstation-statuses` | Sidebar groups, SessionStatusIcon, Chat header badge | 8 个 status group 顺序、icon 颜色、tooltip 文案 |
| `plan-reminders` | Automations module (计划) | 首个真实产品 UI（非 placeholder） |
| `turn-control-history` | Turn footer (lineage badges, aborted marker, failed banner) | 重试/重新生成 badge 文案、失败 banner 中文副本 |
| `turn-control-branch-visible` | Chat header branch banner | `分自 ${parentName}` 渲染正确 |
| `turn-control-branch-orphan` | Chat header (no banner) | 缺失父会话时 banner 不渲染、无死链接占位 |
| `artifact-preview-image` | ArtifactPreview (image/png) | 注册表 happy path image 预览 |
| `artifact-preview-unsupported` | ArtifactPreview (image/heic) | L1 MIME 拒绝、readBinary 永不调用 |
| `artifact-preview-oversize` | ArtifactPreview (oversize) | L1 size cap 拒绝 |
| `sidebar-long-sessions` | Sidebar scroll container, Footer | 60 sessions 滚动不推 footer 出屏幕 |
| `sidebar-search-modal-open` | SearchModal shell | 搜索模态框渲染 |
| `sidebar-row-actions-visible` | Sidebar row action overlay | `:focus-within` actions 不遮盖 time meta / unread dot |

### Manual Smoke → UI Surface 映射

| Path | Surface | 交互类型 |
|---|---|---|
| Path 1 | OnboardingHero | 查看 + 点击 tile |
| Path 2 | Settings · 模型/账号 | 添加连接 + 测试连接 + 观察 badge 变化 |
| Path 3 | Chat header alert | 修改 API key + 查看 header pill |
| Path 4 | Composer, Sidebar | Streaming + delete session |
| Path 5 | PermissionDialog | 查看 destructive dialog 布局 |
| Path 6 | ModelTable | Keyboard nav (ArrowDown/Up/Home/End) |
| Path 7 | Chat surface | Turn block 结构、thinking、token chips |
| Path 8 | Sidebar streaming dot | Streaming indicator、unread 优先级 |
| Path 9 | Command palette | ⌘K 导航 + diagnostics + export |
| Path 10 | Sandbox bridge | IPC 通道 (settings/app/connections/sessions) |
| Path 11 | ArtifactPane | HTML sandbox、diff、collapse、Esc、Save As |
| Path 12 | Sidebar stale pill | Stale session dim + pill + header banner |
| Path 13 | ArtifactPane error states | Deleted/unsupported/missing preview |
| Path 14 | Sidebar status groups | Group 顺序、icon tooltip、chat header badge |

### Script Checks → Quality Dimension 映射

| Script | 维度 | 触发方式 | 速度 |
|---|---|---|---|
| `check-console.mjs` | 安全 (console 泄露) | `pretest` (每次 `npm test` 前) | fast |
| `check-a11y.mjs` | 可访问性 | `pretest` (每次 `npm test` 前) | fast |
| `check-stale-dist.mjs` | 构建卫生 | `check:release` / CI | fast |
| `check-officecli-bundle.mjs` | Release 组件完整性 | `check:release` / CI | fast |
| `capture-screenshots.mjs` | 视觉渲染 | 手动 / CI (按需) | slow (~26×8 spawns) |
| `diff-screenshots.mjs` | 截图管线健全性 | CI (release gate) | medium |
| `desktop-real-window-smoke.mjs` | 原生窗口行为 | 人工触发 | manual |

---

## Gaps

### G1 — 无像素级视觉回归检测

`diff-screenshots.mjs` 只检查 **PNG 是否存在、尺寸是否正确、文件是否有效**。它不检查图片内容。这意味着以下回归会通过当前门禁：
- 一个组件从页面中间位移到左下角
- 按钮颜色从蓝色变成红色
- 字体大小从 14px 变成 26px
- opacity 从 1.0 变成 0.3

**计划**：PR-IR-02 v3 引入 `pixelmatch` + calibrated tolerance。但当前（截至 baseline `335220a`）尚未实现。

### G2 — 无交互测试 (No Interaction Testing)

截图是 **静态的、单帧的**。以下动态行为完全未覆盖：
- `Cmd+K` 打开 palette 的动画和焦点行为
- `Tab` / `Shift+Tab` 的焦点顺序（只能通过 smoke.md Path 6/9 手动验证）
- Streaming 文本的逐字符渲染动画
- PermissionDialog 的 "记住本轮" checkbox 切换
- Settings modal 的 warm-switch（从账号页切换到模型页）
- ArtifactPane 的 Esc 关闭 + 焦点恢复

**部分缓解**：`smoke.md` 的 Path 6/9/10/14 覆盖了一些键盘交互，但全部依赖人工执行。

### G3 — 截图覆盖不完整

以下 UI surface **没有** fixture scenario：
- Command Palette（`smoke.md` Path 9 手动覆盖，无截图 baseline）
- Toast（无 fixture，无截图）
- Keyboard help modal（无 fixture，无截图）
- Error boundary（无 fixture；只在 crash 时出现）
- Tool result renderer（在 turn-narrative 中有间接覆盖，但不是独立 fixture）
- EmptyChatHero（在 first-run 中可能有间接覆盖）

### G4 — 无跨平台截图

所有截图在当前流程中只在 macOS (arm64) 上捕获。Windows 和 Linux 的渲染差异（字体回退、窗口 chrome、DPI 缩放）没有 baseline。

### G5 — Release Gate 盲点

当前 `check:release` 只包含 `check:stale` + `check:officecli-bundle`。Release tag 前缺少的自动检查：
- Screenshot diff 未强制运行（`screenshots:diff:stable` 不包含在 `check:release` 中）
- Typecheck 不在 `check:release` 中（在 root scripts 中独立存在）
- `pretest` (含 console + a11y) 不在 `check:release` 中

### G6 — Stale Sessions 自动化覆盖缺口

`stale-sessions` 场景有 fixture + screenshot，但 **没有自动化测试验证**：
- 发送按钮在 active + stale session 上的禁用/启用逻辑
- Auto-rebind 后的 connection slug 是否正确
- `staleSessionIds` Set 的计算是否包含所有边界情况

### G7 — Accessibility 检查有限

`check-a11y.mjs` 是 **结构性正则扫描**，不是真正的 a11y 测试：
- 无法验证 ARIA live regions 的正确性
- 无法验证 focus trap 在 modal 中的行为
- 无法验证 `:focus-visible` vs `:focus` 的实际渲染
- 无法检测动态生成的 DOM 中的 a11y 问题
- `icon-only-button` 规则只匹配单行 `<button>` 标签（多行标签覆盖较少）

---

## CI Strategy

将检查分为四个层级，按频率和成本分层执行：

### Layer 0 — Fast (每次 push / 每个 PR commit)

```bash
# < 5 秒总计
node scripts/check-console.mjs          # console.* 白名单审计
node scripts/check-a11y.mjs             # 源码 a11y 结构扫描
node scripts/check-stale-dist.mjs       # 构建卫生检查
npm run typecheck                       # TypeScript 编译检查
```

**为什么必须 fast**：这些检查发现的是"绝对错误"（泄露 console、图标按钮无 label、dist 过时）。没有误报，不需要人工审查。

### Layer 1 — Stable (merge 到 main 前，或 nightly)

```bash
# ~ 2-5 分钟
npm --workspace @maka/core test
npm --workspace @maka/storage test
npm --workspace @maka/runtime test
npm --workspace @maka/desktop test
```

**范围**：所有单元测试 + pure-helper 测试 + fixture 种子测试。

### Layer 2 — Slow (release candidate 前，或每周)

```bash
# ~ 15-20 分钟（26 scenarios × 8 variants × ~5s per spawn）
npm --workspace @maka/desktop run screenshots    # 捕获所有 208 张 PNG
npm --workspace @maka/desktop run screenshots:diff  # Stage 1 硬故障检查
```

**为什么 slow 也可以接受**：只检查 PNG 是否存在 + 尺寸是否正确。如果管线回归（capture IPC 坏了），这个 gate 第一次运行就失败。Release 前的频率足够。

### Layer 3 — Update Baseline (仅在视觉变更后)

```bash
# 人工触发，不可自动化
npm --workspace @maka/desktop run screenshots:baseline:stable
# → git diff apps/desktop/tests/screenshots-baseline/
# → 人工审查差异
# → git commit + push
```

### Layer 4 — Manual (human-in-the-loop)

```bash
npm --workspace @maka/desktop run smoke:real-window
```

**为什么必须人工**：原生窗口 resize drag region、模态框 backdrop click、物理键盘的 Tab 焦点遍历——这些在 headless 环境中无法验证。

### 建议的完整 CI Pipeline

```
PR push ──→ Layer 0 (fast checks) ──→ Layer 1 (unit tests)
                                          │
merge to main ──→ Layer 0 + Layer 1 ──→ (pass)
                                          │
release candidate ──→ Layer 0 + Layer 1 + Layer 2 (screenshots)
                          │
                          ├── fail ──→ 调查 capture 管线
                          └── pass ──→ Layer 4 (real-window smoke)
                                          │
                                          ├── fail ──→ 修复后重新 RC
                                          └── pass ──→ tag release
```

---

## Next Actions

### Immediate (本周)

1. **将 `screenshots:diff:stable` 加入 `check:release`** — 确保 release tag 前硬故障门禁运行
2. **验证所有 26 scenarios 的截图可通过**: 运行 `npm --workspace @maka/desktop run screenshots` 并检查失败场景
3. **补全 Toast + Command Palette 的 fixture scenarios** — 当前无截图 baseline

### Short-term (2-4 周)

4. **实现 PR-IR-02 v3 (pixelmatch)** — 先在 `artifact-pane/first-run/artifact-errors` 上 pilot
5. **将 `pretest` 加入 root `check:release`** — 确保 console + a11y 审计在 release 前运行
6. **补全 `stale-sessions` 自动化测试** — 覆盖 auto-rebind + staleSessionIds 计算
7. **为每个 Settings sub-page 添加 empty/failure 状态 fixture**

### Medium-term (1-2 月)

8. **实现交互截图** — 探索是否可以 `capture-screenshots.mjs` 注入 keyboard event 到 renderer
9. **Cross-platform screenshot baselines** — 在 Windows/Linux CI runner 上捕获
10. **a11y 增强** — 添加 focus-trap 验证 + modal ARIA 角色完整性检查到 `check-a11y.mjs`

### 基础设施改进

11. **Manifest-based 回归标记** — 在 `diff-screenshots.mjs` manifest 中添加 per-scenario `lastReviewedCommit`，使 reviewer 能够快速定位"上次审查后哪些 scenario 变了"
12. **Screenshot diff artifacts in CI** — PR-IR-02 v3 落地后，将 diff images 作为 CI artifacts 上传，方便 reviewer 在 PR 页面直接查看

---

## 总结

Maka 的视觉烟雾测试基础设施是一个 **分层的、防御深度的质量门禁系统**，其核心设计原则是：

1. **最小依赖**：只用 Node 22 + Electron，不加 Playwright / JSDOM / ESLint 插件
2. **确定性**：固定时钟、锁定 locale/timezone、reduced-motion variant、隔离 user-data-dir
3. **Fail-closed**：所有输入验证（scenario 名称、variant 名称、theme、locale、timezone）在失败时 fail-closed 到安全默认值
4. **分层频率**：fast checks 每次跑、screenshots 按需跑、real-window smoke 人工触发
5. **合同驱动**：不只是检查"有没有截图"，而是检查"截图管线是否完整" + "UI 契约是否遵守"

**当前最大的两个缺口**是：(1) 缺少像素级视觉回归检测（计划中的 PR-IR-02 v3）；(2) 缺少交互/动态行为的自动化测试覆盖。这两个缺口意味着大量回归仍然依赖人工 code review 和 smoke.md 路径来发现。
