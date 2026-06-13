# Maka 粗读报告：Docs / Tests / Roadmap

> 阅读基线：`335220a`
> 深度档位：`architecture`
> 报告范围：README / docs / notes / scripts / tests / package scripts

---

## 1. Problem

从 docs/notes/tests 这三层看，Maka 当前的产品和工程目标如下：

**产品目标**（来自 `README.md`、`docs/full-product-test-plan.md`、`docs/maka-capability-audit-v1.md`）：

- 做一个**本地桌面 AI 工作台（workbench）**，不是聊天 UI 套壳。Electron 桌面壳承载 React renderer，通过 Vercel AI SDK 统一对接多个模型供应商（Anthropic / OpenAI / DeepSeek / Z.ai / OpenRouter 等）。
- 当前阶段从"能聊天的 chat app"升级到有 **Artifact Workbench（右侧成果面板）、Workstation Shell（会话状态机）、Turn Control（重试/分支/回退）、Health Center（健康面板）、First-run 引导** 的成熟工具。
- 能力优先级（来自 capability audit §11）：ArtifactRecord 数据模型 → ModelCatalog 模型目录 → Session Status 会话状态机 → Turn Control 分支/重试 → Health Center → First-run → 开放网关/Memory/Voice/Search。

**工程目标**（来自测试计划 + ui-quality-plan）：

- 每个 PR 必须回答 5 个问题：Contract / User Flow / Tests / Security / Not Included。
- 每个 UI surface 必须经过 12 道门（contract / pure helper test / component contract / fixture / smoke / light+dark screenshot / narrow screenshot / empty+failure states / a11y / motion / i18n / security）。
- 核心 release gate 是三条：`smoke.md` 手动路径 + desktop unit tests（87 个）+ `check-console.mjs` 日志审计。

---

## 2. Why Hard

Maka 是本地桌面 agent，必须在一个**不可信的 Electron renderer sandbox** 里运行，同时还要做文件系统操作、API 密钥管理、模型调用、工具执行。这决定了以下每个 check 都不是过度工程化，而是硬需求：

### 2.1 Threat Model 多——因为要管密钥、文件、会话、网络

- `docs/memory-threat-model.md`：定义 9 条隐私门（默认关、手动确认、可逆删除导出、隐身模式阻断、禁止自动休眠合并、可见引用、禁止隐藏升级、provider embedding 泄露边界、renderer 不可伪造来源）。Memory 写入必须通过 `validateMemoryWriteRequest`。
- `docs/search-service-threat-model.md`：区分 6 种搜索源（thread / memory / activity / tool / web / web_fetch），本地私有源不能与外部 web 结果混排。URL scheme 白名单、domain 黑白名单、结果字节 cap、超时 abort。
- `docs/voice-threat-model.md`：语音默认关、只有 push-to-talk 和 toggle-to-record 两种模式、不能始终监听、转录可编辑后才发送、原始音频不进 telemetry 也不持久化。
- `docs/workspace-privacy-context.md`：隐身模式（incognito）的 shared type contract，所有 consumer lane（搜索、内存、语音、telemetry）必须消费同一个 `WorkspacePrivacyContext`。
- `docs/design-system.md` §8 反模式清单：禁止把原始 provider error / API key / 用户输入渲染到 UI，必须走 `generalizedErrorMessage` + `redactSecrets` 双层防线。

**根本原因**：Maka 作为本地 agent 可以直接 `rm -rf` 用户的文件。一条假 permission dialog、一个未 sanitize 的 tool output、一次路径穿越，都能引发真实后果。所以必须"默认关 + 显式确认 + 双层 redact + realpath containment + sandboxed iframe"。

### 2.2 Visual Smoke——因为桌面 UI 的主进程/renderer 分离使得常规前端测试无效

- `apps/desktop/tests/smoke.md` 有 **17 条手动路径**（Path 1–17），每条要求明确 Pass/Fail signal。
- `scripts/capture-screenshots.mjs` 自动 spawn Electron 子进程，每个 fixture scenario × 8 variants（light/dark × 1280/990 × motion/reduced）截图。目前有 32+ scenario × 8 variant = 256+ PNG。
- `scripts/diff-screenshots.mjs` 做 PNG 尺寸/完整性 sanity gate，缺失/损坏/尺寸错误 → hard fail。
- `scripts/desktop-real-window-smoke.mjs` 是人工介入的 native window 行为验证（窗口 resize、titlebar drag、modal keyboard trap），因为 Electron screenshot 无法验证 OS 级 hit test。

**根本原因**：Electron `sandbox: true, contextIsolation: true, nodeIntegration: false` 意味着 renderer 不能直接测试。playwright 或 JSDOM 也无法模拟 Electron BrowserWindow 的真实行为。所以必须用 fixture 种子 + 截图 + 人工 smoke 组合。

### 2.3 OfficeCLI Bundle——因为桌面应用需要跨平台 native 二进制

- `scripts/prepare-officecli.mjs` 从 GitHub release 下载对应平台/架构的 `officecli` 二进制，SHA256 校验后放入 `apps/desktop/resources/tools/`。
- `scripts/check-officecli-bundle.mjs` 验证二进制存在、可执行、版本号匹配。
- `npm run check:release` 组合 `check:stale && check:officecli-bundle`。

**根本原因**：Maka 的某些工具能力依赖外部 native 二进制，打包时必须确保它存在且版本正确。

### 2.4 Stale Dist——因为 monorepo 多 workspace 的增量编译会骗过测试

- `scripts/check-stale-dist.mjs` 按 mtime 比较每个 workspace 的 `src/*.ts(x)` 和 `dist/*.js`，源文件新于编译产物 → 退出非零。
- `scripts/clean-build.mjs` 删除所有 workspace 的 `dist/` 和 `.tsbuildinfo`。

**根本原因**：TypeScript 增量编译 + workspace 依赖链可能在 rename/remove export 后留下 stale dist，导致测试通过但实际构建失败。

### 2.5 A11y / Copy Checks——因为 renderer 在 sandbox 里运行，a11y 不可委托给浏览器插件

- `scripts/check-a11y.mjs` 走静态正则扫描 `.tsx` 源码，检测 6 条规则：icon-only 按钮缺 `aria-label`、正数 `tabIndex` 破坏 tab 序、dialog 缺 label、input 缺 label、icon-only 链接、English-only 文案未翻译。
- `scripts/check-console.mjs` 白名单制，所有 `console.*` 调用必须在 `ALLOW` map 里有理由，否则 pretest 失败。

**根本原因**：桌面应用不能用 Lighthouse。sandbox 化后也不方便走自动化 a11y tree。静态正则扫描是"minimum floor"。

---

## 3. Design Approach

### 3.1 文档体系 — "契约优先"

| 文档 | 角色 | 面向谁 |
|---|---|---|
| `README.md` | 入门 + smoke test 一行命令 | 新开发者 |
| `docs/full-product-test-plan.md` | 一个月的交付契约与测试分层定义 | @xuan（owner）, 所有人 |
| `docs/maka-capability-audit-v1.md` | 能力成熟度审计，对标 Alma/Craft | @xuan |
| `docs/design-system.md` | UI token/组件/文案/动效的**契约** | @yuejing, PR reviewer |
| `docs/ui-quality-plan.md` | 12 道 UI gate + 测试分层矩阵 | @yuejing |
| `docs/memory-threat-model.md` | Memory 9 条隐私门 + contract test 矩阵 | PR-MEMORY-1 |
| `docs/search-service-threat-model.md` | 搜索源分离 + URL/domain/CAPTCHA 边界 | PR-SEARCH-0 |
| `docs/voice-threat-model.md` | 语音 privacy-first 契约 | PR-VOICE-0 |
| `docs/workspace-privacy-context.md` | Incognito 共享类型 contract | PR-INCOGNITO-0 |
| `notes/*.md` | 历史设计线索、对标分析、bug 审计 | 所有开发者 |

**关键设计决定**：
- 文档不是手册，是**契约**（contract）。`design-system.md` 开篇就说"PR 改动 UI 时可以被 reviewer 和 release-gate 审计"、"文档与代码的偏离 ≡ 契约违规"。
- threat model 也是 contract。`memory-threat-model.md` 明确说"PR-MEMORY-1 is a **contract-only** package. It MUST NOT add IPC handlers, storage repositories..."
- 每个 contract 都附带"反模式清单"（NOT-DO）和"禁止从 Alma 抄袭的模式"（Do-not-copy list）。

### 3.2 脚本体系 — "gate 即代码"

脚本不是一次性工具，而是**可复现的 CI gate**：

| 脚本 | 类型 | 触发时机 |
|---|---|---|
| `check-console.mjs` | 日志审计 | pretest |
| `check-a11y.mjs` | a11y 静态扫描 | pretest |
| `check-stale-dist.mjs` | 编译时效检测 | `npm run check:stale` |
| `check-officecli-bundle.mjs` | 二进制完整性 | `npm run check:officecli-bundle` / release |
| `capture-screenshots.mjs` | 自动截图 | `npm --workspace @maka/desktop run screenshots` |
| `diff-screenshots.mjs` | 截图 sanity gate | `screenshots:diff:stable` |
| `desktop-real-window-smoke.mjs` | 人工介入 native 验证 | PR UI-shell 改动前 |
| `clean-build.mjs` | 清理 dist | `npm run clean` |
| `prepare-officecli.mjs` | 下载二进制 | `npm run prepare:officecli` |

### 3.3 测试体系 — "分层但不重复"

测试策略分层（来自 `full-product-test-plan.md` §2 + `ui-quality-plan.md` §2）：

1. **Core unit tests** (`@maka/core`): data contracts, enum validation, permission categorization, redaction, model readiness
2. **Storage tests** (`@maka/storage`): JSONL migration, artifact metadata, credential persistence, path guard, tombstone
3. **Runtime tests** (`@maka/runtime`): SessionManager lifecycle, streaming, tool artifact derivation, cancellation, permission parking
4. **Desktop main/IPC tests** (`@maka/desktop`): chat readiness, external link guard, window state, open path guard, connection status, settings IPC, artifact IPC failure reasons
5. **Renderer pure-helper tests** (同一 workspace): state derivation, keyboard transitions, display copy matrices, turn materialization
6. **Fixture scenarios**: 每种 UI surface 的确定性种子数据
7. **Smoke paths** (`smoke.md`): 17 条端到端手动路径
8. **Visual regression** (screenshots): light/dark/narrow/reduced-motion 截图

**明确不做的测试**：
- 不用 JSDOM 做 React 渲染测试（"over-fragile; pure helper + smoke covers it"）
- 后端逻辑不在 UI workspace 测
- 网络调用在 @maka/runtime 测试中 mock

---

## 4. Code Walkthrough

### 4.1 关键 docs

- **`docs/full-product-test-plan.md`**: 一个月的交付计划（Week 1: Artifact Workbench, Week 2: ModelCatalog + Workstation Shell, Week 3: Health Center + First-run + Quick Chat, Week 4: Open Gateway + Memory + Voice + Search + MCP）。定义了 9 种 Feature Done Definition 和每个 feature 的 case 矩阵。PR checklist 模板可以被 copy 到 PR description。
- **`docs/maka-capability-audit-v1.md`**: 对标 Alma 和 Craft 的能力审计。推荐 PR 顺序分 4 波（A: Workbench foundation, B: Decision quality, C: Workflow control, D: Ecosystem）。包含"禁止抄袭模式"清单。
- **`docs/design-system.md`**: 1200+ 行，覆盖 6 色哲学、13 个组件契约（Button / Toast / Modal / PermissionDialog / Composer / SessionList / ChatHeaderAlertBadge / TurnView / ToolActivity / ModelTable / MessageCopyButton / Markdown）、12 条动效规则、z-index 阶梯、表面状态矩阵、release-gate 钩子、反模式清单。
- **`docs/ui-quality-plan.md`**: 每 UI surface 的 12 门 checklist + 30+ 个 ⚙️ 缺口。定义了 5 个 PR-IR-XX 基础设施 PR（截图管道、diff CI、a11y 断言库、reduced-motion 变体、i18n 字符串提取器）。
- **`docs/memory-threat-model.md`**: 9 条隐私门，每条都有 contract 级强制手段 + 对应测试 assert label（G1–G9）。定义了"准记忆表面排除清单"——哪些已有数据（settings.json, skills/, usage_log, session.jsonl）不能被当作 MemorySource。
- **其他 threat model**: 都是 contract-only package 的边界文档，先冻结安全形状，再实施运行时。

### 4.2 关键 notes

- **`maka-bug-flow-audit-2026-05-22.md`**: @xuan 的 13 个已修复 bug 记录（shell safe-prefix 绕过 / backend failure 留下假 session 状态 / renderer stale message 覆盖 / streaming UI 卡住 / packaged app 创建 FakeBackend / skills 未注入 prompt / Stop 未拒绝 parked permission / read tools 越界 / stream catch 丢失 turn id / prompt 缺 workspace 指令 / Health Center 缺 LLM runtime probe / Write/Edit 越界 / session store 接受路径形 id）。每个 bug 都有 commit hash、evidence、impact、fix 描述。这个文件是理解当前代码基线安全质量的关键入口。
- **`maka-memory-whitebox-contract.md`**: V0.1 已实现的透明文件 memory（`c06e13f`），V0.2 扩展（origin/status/tags/decayTtlMs/extract_memory tool），V0.3 开放问题（provider abstraction / vector search / cross-session recall / Dream Mode）。
- **`alma-time-driven-recurring-reminders-2026-05-29.md`**: 对标 Alma 的定时调度，实现 Maka 的 recurring plan reminders（once/recurring daily/weekly/monthly）。
- **`pr-oauth-subscription-0-gate.md`**: Claude subscription OAuth 的阻塞门——token 不能泄露给 renderer、必须 safeStorage 加密、logout 仅本地清理。
- **`pr-pi-agent-loop-0-plan.md`**: 替代当前 AI SDK loop 的 pi/ACP process-backed agent loop 计划，分 5 个 PR。

### 4.3 关键 scripts

| 脚本 | 行数 | 用途 |
|---|---|---|
| `capture-screenshots.mjs` | 417 | spawn Electron 子进程，env var 注入 fixture + theme + viewport + reduced-motion，等待 stdout marker 后 kill 子进程，复制 PNG 到 `screenshots/` |
| `diff-screenshots.mjs` | 409 | 对截图做 sanity check：PNG header 校验、尺寸验证（接受 1x/2x Retina）、字节大小容差警告。stable subset（artifact-pane / first-run / artifact-errors）用 `--subset stable` |
| `desktop-real-window-smoke.mjs` | 579 | 人工介入 gate：prompt reviewer 确认 12 项 OS-level 行为（resize edges、titlebar drag、modal keyboard trap 等），输出 JSON + Markdown report |
| `check-a11y.mjs` | 416 | 6 条规则静态扫描（icon-only button/link、positive tabIndex、dialog label、input label、english aria-label），支持 `// a11y-allow:` 行内例外 |
| `check-console.mjs` | 127 | `console.*` 白名单审计，8 个允许文件，未注册 → pretest fail |
| `check-stale-dist.mjs` | 95 | mtime 比较 7 组 src→dist 映射 |
| `clean-build.mjs` | 49 | 删除 13 个 dist/tsbuildinfo 路径 |
| `prepare-officecli.mjs` | 160 | 下载 officecli 二进制 + SHA256 校验 + chmod 755 |
| `check-officecli-bundle.mjs` | 72 | 验证二进制存在、可执行、版本号匹配 |

### 4.4 关键 package scripts

**根 `package.json`**
```bash
npm run typecheck          # 全部 workspace 类型检查
npm run build              # 全部 workspace 编译
npm run dev                # build + Electron 启动
npm run clean              # 删除所有 dist/tsbuildinfo
npm run check:stale        # 检测 stale dist
npm run prepare:officecli  # 下载 officecli 二进制
npm run check:officecli-bundle  # 验证 officecli bundle
npm run check:release      # stale + officecli 组合检查
```

**`@maka/desktop`**
```bash
npm test                   # pretest (build core/storage/runtime/ui + check-console + check-a11y) → build main → node:test
npm run pretest            # 前置检查链
npm run screenshots        # 全量截图 (~256 PNGs)
npm run screenshots:single # 单场景截图
npm run screenshots:diff:stable  # stable subset sanity gate
npm run smoke:real-window  # 人工介入 native window smoke
npm run smoke:programmatic-window  # 纯程序化 window smoke
```

**`@maka/core` / `@maka/storage` / `@maka/runtime`**
```bash
npm test                   # build → node:test dist/**/*.test.js
npm run typecheck          # tsc --noEmit
```

---

## 5. Flows

### Flow 1: Local Build / Typecheck / Test

```
npm run dev
  └─ npm run build (all workspaces)
       ├─ @maka/core:    tsc → dist/
       ├─ @maka/storage: tsc → dist/
       ├─ @maka/runtime: tsc → dist/
       ├─ @maka/ui:      tsc → dist/
       └─ @maka/desktop:
            ├─ build:main:     tsc → dist/main/
            ├─ build:preload:  esbuild → dist/preload/preload.cjs
            └─ build:renderer: vite → dist-renderer/
  └─ electron .

npm --workspace @maka/desktop test
  └─ pretest
       ├─ build @maka/core → dist/
       ├─ build @maka/storage → dist/
       ├─ build @maka/runtime → dist/
       ├─ build @maka/ui → dist/
       ├─ scripts/check-console.mjs   ← src grep console.*
       └─ scripts/check-a11y.mjs      ← src grep a11y violations
  └─ build:main (tsc → dist/main/)
  └─ node --test "dist/main/**/*.test.js"
```

### Flow 2: Visual Screenshot Smoke

```
npm --workspace @maka/desktop run screenshots
  └─ build all workspaces + desktop
  └─ node scripts/capture-screenshots.mjs --all
       └─ foreach (scenario ∈ ALL_SCENARIOS) × (variant ∈ VARIANTS):
            ├─ spawn electron . --user-data-dir=<tmp>/maka-visual-smoke-<scenario>-<variant>-<pid>
            │    env: MAKA_VISUAL_SMOKE_FIXTURE=<scenario>
            │         MAKA_VISUAL_SMOKE_AUTO_CAPTURE=<variant.name>
            │         MAKA_VISUAL_SMOKE_THEME=light|dark
            │         MAKA_VISUAL_SMOKE_WIDTH/HEIGHT=<viewport>
            │         MAKA_VISUAL_SMOKE_REDUCED_MOTION=1 (optional)
            │         MAKA_VISUAL_SMOKE_LOCALE=zh
            ├─ Renderer: 2 RAF + 400ms idle → window.maka.visualSmoke.capture()
            ├─ Main: webContents.capturePage() → write PNG → stdout "[visual-smoke] captured…"
            ├─ Driver: grep stdout marker → kill subprocess
            └─ Driver: copyFile PNG → apps/desktop/tests/screenshots/<scenario>/<variant>.png
       └─ write .meta.json sidecar with locale info

npm --workspace @maka/desktop run screenshots:diff:stable
  └─ node scripts/diff-screenshots.mjs --subset stable
       └─ foreach (artifact-pane, first-run, artifact-errors) × (8 variants):
            ├─ exists? → missing
            ├─ PNG header valid? → corrupt_png
            ├─ bytes > 1024? → too_small
            └─ dimensions match? (1x or 2x viewport) → wrong_dimensions
       └─ Soft: byte size drift vs baseline (>15% standard, >25% dynamic) → warning
```

### Flow 3: Provider Settings / Test Connection

```
User: Settings → 模型 → Add Connection → paste API key → Save
  └─ IPC: connections:create → main/connection-store persist (safeStorage encrypt credentials)
  └─ Auto: connections:fetchModels → AiSdkBackend test fetch
       ├─ ok → Models saved with source='fetched', fetchedAt=now
       └─ fail → toast.error, models stay in source='fallback'

User: Settings → 账号 → 测试连接
  └─ IPC: connections:test → main/test-connection helper
       ├─ Creates temporary model → streamText("ping") → abort after first token
       ├─ ok → success toast "连接已验证" + latency + tested model
       │        Row badge → 已验证可用 (green/success)
       └─ fail → error toast
            ├─ 401/403 → 需要重新登录 (warning tone)
            ├─ 5xx → 连接出错 (destructive)
            ├─ timeout/network → 连接出错
            └─ ChatHeaderAlertBadge surfaces matching tone
```

### Flow 4: Release Check

```
npm run check:release
  ├─ npm run check:stale → scripts/check-stale-dist.mjs
  │    └─ foreach (core/storage/runtime/ui/desktop:main/preload/renderer):
  │         compare max mtime(src/*.ts) vs max mtime(dist/*.js)
  │         stale → exit 2
  └─ npm run check:officecli-bundle → scripts/check-officecli-bundle.mjs
       ├─ bundled-tools.json → resolve asset for platform/arch
       ├─ resources/tools/officecli exists? → missing
       ├─ file permissions (non-win32)? → not executable
       └─ officecli --version matches expected? → version mismatch

Before release tag:
  └─ npm --workspace @maka/desktop test        (pretest + node:test)
  └─ npm run typecheck                          (all workspaces)
  └─ screenshots:diff:stable                    (capture sanity)
  └─ smoke.md manual paths (currently 17 paths)
  └─ desktop-real-window-smoke.mjs              (native window gate)
```

### Flow 5: OfficeCLI Bundle

```
npm run prepare:officecli
  └─ node scripts/prepare-officecli.mjs [--platform <p> --arch <a>]
       ├─ Read apps/desktop/bundled-tools.json → officecli.version + assets
       ├─ Resolve asset for target e.g. officecli-darwin-arm64.tar.gz
       ├─ Fetch SHA256SUMS → parse → verify asset checksum exists
       ├─ Fetch binary from GitHub release (MAKA_OFFICECLI_FETCH_TIMEOUT_MS, default 300s)
       ├─ SHA256(data) === expected → Checksum mismatch
       ├─ Write → apps/desktop/resources/tools/officecli (chmod 755 on non-win32)
       └─ Verify: officecli --version → version match

npm run check:officecli-bundle  (verification gate, same script different export)
  └─ Does the bundled binary exist, is it executable, and does --version match?
```

---

## 6. Tests

### 6.1 Test Strategy 总结

| 层级 | 工具 | 覆盖对象 | 命令 |
|---|---|---|---|
| **Core unit** | `node:test` | 数据契约、枚举验证、permission 分类、redaction、model readiness、memory validation | `npm --workspace @maka/core test` |
| **Storage** | `node:test` | JSONL header migration、artifact metadata、credential persistence、path guard、tombstone | `npm --workspace @maka/storage test` |
| **Runtime** | `node:test` | SessionManager lifecycle、backend rebuild、streaming、tool artifacts、cancellation、permission parking、model fetch | `npm --workspace @maka/runtime test` |
| **Desktop main/IPC** | `node:test` | chat readiness + auto-rebind、external link guard、window state、open path guard、connection status、settings IPC、artifact IPC failure、sandbox bridge | `npm --workspace @maka/desktop test` |
| **Renderer pure helper** | `node:test` | state derivation、keyboard transitions、display copy matrices、scroll-motion-policy、session-status-presentation、branch-banner、turn-footer-actions、artifact-preview-registry | 同上（desktop workspace 中） |
| **Fixture** | `MAKA_VISUAL_SMOKE_FIXTURE=...` | 32+ 场景的确定性种子数据（first-run / provider-workspace / artifact-pane / artifact-errors / stale-sessions / workstation-statuses / turn-control-history / plan-reminders 等） | `MAKA_VISUAL_SMOKE_FIXTURE=all npm --workspace @maka/desktop run dev` |
| **Screenshot** | `capture-screenshots.mjs` + `diff-screenshots.mjs` | 每个 scenario × 8 variants PNG | `npm --workspace @maka/desktop run screenshots` |
| **Smoke (manual)** | `smoke.md` | 17 条端到端路径，每条有 Precondition / Steps / Pass signal / Fail signal | 手动执行 |
| **Real window smoke** | `desktop-real-window-smoke.mjs` | 12 项 OS 级 native 行为（programmatic 自动 + 人工） | `npm --workspace @maka/desktop run smoke:real-window` |
| **Release checks** | `check-console.mjs` + `check-stale-dist.mjs` + `check-officecli-bundle.mjs` | console 审计、stale dist、officecli bundle | `npm run check:release` |

### 6.2 测试数量（来自 bug audit note 的 verification log）

- Core: 124 tests
- Storage: 39 tests
- Runtime: 99 tests
- Desktop: 327 tests（含 main/IPC + renderer pure helper）
- **总计: 589 tests**（基线 `0afcf2e`）

### 6.3 Visual Smoke 截图数量

- 30+ fixture scenarios × 8 variants（light/dark × 1280/990 × motion/reduced）= **240+ PNGs**
- Stable gate subset: 3 scenarios（artifact-pane / first-run / artifact-errors）× 8 variants = 24 PNGs

---

## 7. Risks

### 7.1 文档与代码漂移（高风险）

- `docs/design-system.md` 是"活契约"，但组件行号引用可能过期（如 `components.tsx:77` 随文件重构而变）。
- `docs/maka-capability-audit-v1.md` 中的 `ArtifactKind` 类型与 `docs/design-system.md` §9.1 中的已不同（audit 有 8 种 kind，design-system 只有 5 种）。两者写于不同时间，反映不同设计阶段。
- `docs/full-product-test-plan.md` 的 fixture scenario 清单已有 18 个场景，但 `capture-screenshots.mjs` 的 `ALL_SCENARIOS` 有 32+ 个。计划与实现已不同步。
- **建议**：精读时需要交叉验证 docs 报告的能力与 `packages/core/src/` 的实际 type 定义。

### 7.2 notes 目录历史信息过多（中风险）

- `notes/` 下有 11 个条目，其中 6 个是 `alma-deep-dive-yuejing-round-1~6/` 目录。这些是 yuejing 对标 Alma 的深度研究，但已可能过时。
- `maka-bug-flow-audit-2026-05-22.md` 记录 13 个已修复的 bug，这是高价值的历史记录，但新团队可能不知道它们存在。
- notes 文件没有统一的 freshness 标记，部分引用已删除的 local 路径（如 `~/Downloads/alma-re/`）。

### 7.3 测试慢/脆（中风险）

- `npm --workspace @maka/desktop run screenshots` 需要 spawn 30+ × 8 = 240+ 次 Electron 子进程，每次等待 renderer settle + capture。全量截图可能需要数十分钟。
- `desktop-real-window-smoke.mjs` 需要人工介入，PR 节奏快时容易被跳过。
- `diff-screenshots.mjs` 的 pixel-level diff 尚未实现（promised in PR-IR-02 v3），当前只做尺寸/完整性 sanity check。UI layout regression 可能被漏掉。

### 7.4 Release Check 盲点（中风险）

- `check:release` 只包含 `check:stale` + `check:officecli-bundle`。没有自动运行 typecheck、lint、或 test。
- `smoke.md` 是手动路径，没有自动化强制执行。依赖 PR author 自觉。
- `check-console.mjs` 的白名单只覆盖 8 个文件。新增文件加 `console.log` 且在 dev-gate 后忘了加白名单会让你发现得很晚。

### 7.5 手动 Smoke 缺口（低但持续风险）

- `smoke.md` 的 17 条路径覆盖了核心流程，但尚未覆盖所有 fixture scenario（32+ scenarios vs 17 paths）。Quick Chat、Health Center、First-run stepper、Sources/Skills/Automations 等 pending surface 的 smoke path 是空白。
- `ui-quality-plan.md` §4 矩阵显示大量 ⚙️（partial）和 ❌（missing），特别是 narrow viewport 截图、a11y assertion、motion contract 的覆盖率。

### 7.6 Threat Model 的 contract-only 风险（低风险）

- memory / search / voice / incognito 的 threat model 都是"contract-only package"，意味着它们定义了安全形状但尚未实装 IPC/runtime。如果在实施时偏离 contract，contract doc 本身不会阻止。
- `memory-threat-model.md` 自己承认了"source-laundering"风险（contract 层无法防止 handler 层把 usage-log 数据复制成 chat_extracted payload）。

---

## 8. Next Questions

从 docs/tests/roadmap 反推，下一轮精读 DAG 应该按以下优先级展开：

### 8.1 验证文档声称的能力是否在代码中存在

- `docs/full-product-test-plan.md` 声称的 9 个 feature 各有什么实际实现？逐一对照 `packages/` 和 `apps/desktop/src/` 的对应文件。
- `docs/maka-capability-audit-v1.md` 推荐的 PR 顺序（A → B → C → D）实际执行到了哪一步？`ArtifactRecord` 的 core/storage contract 是否已落地？
- `docs/design-system.md` §9 中的未实装 surface（Artifact pane / Quick Chat / Health Center / Workstation shell / First-run / Turn control / Sources-Skills-Automations / ModelCatalog）哪些已经有代码，哪些仍是空壳？

### 8.2 追踪每条 threat model 的实施状态

- `docs/memory-threat-model.md` 的 9 条隐私门中，哪些已有对应的 test assertion（G1–G9），哪些仍是"forward-looking"？
- `docs/search-service-threat-model.md` 的 normalize/allowlist/timeout 函数实际在哪个文件实现？
- `docs/workspace-privacy-context.md` 的 `WorkspacePrivacyContext` 已被哪些 consumer lane 消费？

### 8.3 追踪 bug audit note 中的修复是否覆盖全面

- `maka-bug-flow-audit-2026-05-22.md` 列出的 13 个 bug，每个的 test coverage 是否可以追溯到对应的 `*.test.ts` 文件？
- 是否还有同类 bug 的未覆盖分支（如 Write/Edit containment 的 symlink edge case）？

### 8.4 验证 notes 中的设计决策是否被代码遵守

- `maka-memory-whitebox-contract.md` 定义的 `agentReadEnabled` 默认 OFF、`incognito_blocked` 状态等是否在 `local-memory-service.ts` 中实现？
- `pr-pi-agent-loop-0-plan.md` 的 8 条 blocking gate 是否在规划中有对应的 check 点？

### 8.5 交叉验证 fixture scenario 与 smoke path 的对齐度

- `capture-screenshots.mjs` 的 `ALL_SCENARIOS` 列表与 `full-product-test-plan.md` §2.6 的 fixture scenario 表之间的差集是多少？
- 哪些 fixture scenario 没有对应的 smoke path？

### 8.6 建议的精读文件列表（优先级从高到低）

1. `packages/core/src/` — 验证 docs 中定义的所有 type/contract 是否存在、是否对齐
2. `apps/desktop/src/main/` — IPC handler 的安全实现（open-path-guard, external-link-guard, credential storage）
3. `packages/runtime/src/` — SessionManager / PermissionEngine / AiSdkBackend 的实际流程
4. `apps/desktop/src/main/visual-smoke-fixture.ts` — fixture 种子数据的覆盖完整度
5. `apps/desktop/src/main/__tests__/` — 现有 327 test cases 的具体覆盖内容
6. `packages/ui/src/components.tsx` — 组件的安全/权限/renderer sandbox 边界
