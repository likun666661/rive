# 粗读报告：Docs / Issues / Product Roadmap

> 仓库: `pie` | 基线: `f1c35a3` | 深度: architecture | 阅读模式: 只读粗读

---

## 1. problem

`pie` 试图解决的核心产品/工程问题：

**做一个本地优先、可长期运行的 AI coding agent runtime**，而不是又一个 LLM 对话壳。

具体来说：

1. **本地自动化 agent**：原动力来自"用本地 DS4 模型跑长期自动化任务"，需要一个可定制、可触发、有记忆的 agent runtime。不是一次性问答，而是持续运行的自动化 loop。
2. **coding agent 框架化**：将 Rust 重写的 `pie` 做成可扩展的 agent 框架。斜杠命令、会话持久化、技能系统、MCP 工具、cron/触发、生命周期 hooks、Web relay 都在这一框架内统一。
3. **本地模型优化**：DS4（DeepSeek V4 Flash）本地服务器使用 byte-exact 前缀缓存，客户端必须保证请求流的字节级确定性才能命中 KV 缓存。这对 agent 长会话至关重要——每次 turn 只 prefills 新增 token，而非重 prefill 整段对话。
4. **自动化路由**：cron/trigger 的自动化输出需要合理的落点——既不是打断主聊天（`InjectAndRun`），也不是沉入审计日志。Loops + Triage Inbox 提供了"后台运行、人工 triage"的模式。
5. **可移植会话**：支持 session export/import，备份为单文件归档（`.piesession` tar），包含对话树、触发器、cron、compaction 等。

**一句话**：`pie` 要做一个"本地 agent runtime + coding agent + 自动化平台"，对标 Claude Code / Aider / Cursor Agent，但更侧重本地模型、长周期自动化和框架可扩展性。

---

## 2. why_hard

这些问题的交叉约束导致难度集中在几个方向：

### 2.1 Coding Agent 的基础难度

- **流式响应 + 交互 UI**：需要在 TUI 中处理异步流式 LLM 输出，同时保持输入框活跃、支持 Ctrl-C 中止、可排队 prompt。`ratatui`/`crossterm` 的 split-view 渲染、scrollback 管理、spinner 阶段展示都很复杂。
- **工具安全**：bash/edit/write 等工具必须经过权限系统审核。危险命令检测（`rm -rf /`、`sudo`、`curl|sh` 等）需要 token-aware 分类器，而非简单正则。
- **上下文管理**：长对话的 compaction（自动摘要压缩）、token 预算控制、cost tracking、`@file` 注入——这些都需要在不破坏会话一致性的前提下执行。

### 2.2 本地模型（DS4 / local model）的特殊挑战

- **Byte-exact 前缀缓存**：DS4 的 KV prefix cache 只对完全相同的字节前缀有效。任何差异（如遗漏 reasoning content、字段顺序变化）都会导致缓存失效，整个 100k token 的对话需要重新 prefill。`docs/ds4.md` 记录了三个关键修复：reasoning 重放、HTTP 409 透明重试、cache write 报告。
- **本地模型缺失能力**：本地 OpenAI-compatible 模型需要通过 `models.json` 声明其兼容性标志（`supportsStore`、`supportsReasoningEffort`、`supportsUsageInStreaming` 等），比云端模型多一层适配复杂度。
- **无服务器端缓存控制**：不像 Anthropic 有显式 `cache_control` 断点，本地缓存是全自动的，客户端只能通过保证字节精确性来利用它。

### 2.3 MCP 与通知系统的复杂性

- **双向通知**：MCP 服务器既可以提供工具（`tools/list → tools/call`），也可以推送通知（`notifications/*`）。通知需要经过 dedup（5 分钟窗口）、cycle suppression（最大 5 hop）、权限审核（`BeforeTriggerHook`）、prompt 确认（`TriggerPromptRequest`）——这一整套 pipeline 的每个环节都要有审计记录。
- **`inject_and_run` 的反馈循环**：一个 MCP 服务器既可以推送通知又可以提供工具。如果通知触发了一次 agent turn，而该 turn 又调用了同一服务器的工具，工具操作可能导致新的通知推送——形成反馈循环。Cycle suppression 无法捕获这种跨 trace 循环（因为每次 MCP push 会生成新的 `trace_id`）。设计上将其视为"信任声明"：只有显式设为 `inject_and_run` 的服务器才允许这种路径。
- **隐私边界**：MCP 原始通知参数不能持久化到聊天内容或审计中；自定义通知仅保存有界的方法风格摘要，除非服务器提供 `_meta.pie_summary`。

### 2.4 Skills 系统的设计权衡

- **多源加载**：用户技能（`~/.pie/skills/`）、项目技能（`<cwd>/.pie/skills/`）、内置技能（如 `karpathy-guidelines`）三重优先级；同名冲突规则必须在启动时确定性地解决。
- **热加载**：`InstallSkill` 和 `SkillBuilder` 工具需要在不重启 `pie` 的情况下刷新技能目录——通过 `AgentHarness::reload_skills_from_disk()` 实现，但正在进行的 turn 不受影响。
- **两阶段安全模型**：安装技能需要 `confirm: true` 显式确认；源验证做 SSRF 防御（拒绝 loopback/RFC1918/.localhost 主机名）。

### 2.5 可观测性约束

- 需要 per-session 日志（`~/.pie/logs/<session>.log`），但日志必须做 secrets 脱敏。
- `/diag` 快照需要在 100ms 内返回；`/bug-report` 生成脱敏 tar.gz。
- OTLP 导出仅当 `OTEL_EXPORTER_OTLP_ENDPOINT` 设置时激活。

### 2.6 Web Relay 的安全模型

- 能力 URL 既是 view token 也是 prompt token——泄漏意味着完全控制。
- 需要 agent key + TOFU (Trust On First Use) 模型来防止 viewer 冒充 agent 端。
- 快照大小限制（1 MiB drop）、远程 slash 命令拒绝、credentials 隔离——每层都需要在实现中显式验证。

---

## 3. design_approach

### 3.1 架构分层

```
pie-ai (crates/ai)              ← 统一流式 LLM 客户端、provider 集成、model catalog
  ↓
pie-agent-core (crates/agent)   ← agent runtime: harness, session, compaction, skills, hooks
  ↓
pie-coding-agent (crates/coding-agent) ← CLI/TUI/Web, tools, slash commands, triggers, cron
  ↓
pie-mcp (crates/mcp)            ← MCP client: stdio/HTTP transport, JSON-RPC, tools/call
```

- 严格的单向依赖：`coding-agent → agent → ai`；`mcp` 仅被 `coding-agent` 消费。
- 核心 agent（`Agent`）保持 IO-free；所有文件系统、网络适配器都在 `harness/` 或 `coding-agent` 中。

### 3.2 产品路线图（`docs/issues/00-master.md`）

按 Tier 划分为 8 个层次：

| Tier | 方向 | 状态 |
|------|------|------|
| Tier 1 (daily UX) | TUI 重构、斜杠命令、权限系统、@file | **已实现** |
| Tier 2 (session/state) | continue/named session、cost tracking、session export/import | **已实现** (#20 进行中) |
| Tier 3 (sandboxing) | ~~沙盒/文件隔离~~ | **已明确取消** |
| Tier 4 (framework depth) | skills loader、harness expansion、SkillBuilder、Web relay、Loops、MCP client、subagent、builtin tools | **大部分已实现** (#9 MCP client 部分完成) |
| Tier 5 (auth/cloud) | `/login`、OAuth PKCE、Bedrock SigV4、Vertex ADC | **已实现** |
| Tier 6 (observability) | tracing、`/diag`、`/bug-report`、OTLP | **已实现** |
| Tier 7 (multimodal) | image input (`--image`、Ctrl-V 剪贴板粘贴) | **已实现** |
| Tier 8 (cross-agent) | ~~公共 MCP 跨 agent 服务~~ | **已移除** (2026-06-10) |

### 3.3 已实现 (从 CHANGELOG.md + issues 状态)

- **Tier 1**: `#2` TUI overhaul (split-view, multi-line, history, Ctrl-C abort, spinner, streaming markdown, bracketed-paste, Ctrl-V 粘贴图像); `#3` 21+ slash commands + Tab 补全; `#4` 危险 bash 检测 (11-pattern corpus + token-aware rm 分类器); `#5` @file mention (capped at 64 KiB, `<file path="...">` 块注入)
- **Tier 2**: `#6` --continue/list-all-sessions/save/name/find; `#7` CostTracker + budget_cap_usd + fallback_model; `#20` session export/import (.piesession tar archive)
- **Tier 4**: `#10 Part A` skills 双根加载器; `#11` task subagent; `#12` web_fetch/web_search/git/LSP; `#17` AgentHarness 扩展 (OnTurnEndHook, TurnEndAction, run_evaluator); `#21` SkillBuilder tool; `#22` Web relay (pie.0xfefe.me); `#23` Loops (stateful cron + triage inbox); `#9` MCP stdio + Streamable HTTP transport
- **Tier 5**: `#13` auth.json + /login + /logout + OAuth 2.0 PKCE; `#14` Bedrock SigV4 + Vertex ADC
- **Tier 6**: `#15` tracing subscriber + /diag + /bug-report + OTLP
- **Tier 7**: `#16` --image (PNG/JPEG/WebP/GIF, 10 MiB cap)
- **Framework**: InstallSkill (two-phase, SSRF guard, hot reload); trigger runtime (RFC 1: dedup/cycle/permission/prompt 完整 pipeline); PermissionCategory::ControlPlaneWrite; trigger promotion (template engine + structured authorization)

### 3.4 进行中 / 构想中

- **#20 Session export/import**: 设计完成、部分实现，待补充 sidecar bundling 和 CLI e2e 测试。
- **Loops Phase 3** (`docs/issues/23-loops-inbox.md`): maker/checker verification（第二个 subagent 对抗性审核 findings）。
- **#17 多 leaf session**: 分支摘要、`active_leaf_id`、round-trip 确定性测试待落地。
- **#9 MCP full**: tools/list + tools/call 已完成；`resources/*`、`prompts/*` 待补充。
- **#10 Part B WASM 扩展宿主**: 仅设计预留，未实现。
- **#12 LSP per-language 丰富配置**: 当前仅 per-extension，多服务器协作待做。
- **#14 Bedrock streaming + Vertex full ADC chain**: 非流式路径完成，流式待做。

### 3.5 已明确取消

- Windows 支持
- 文件系统/网络沙盒 (#8)
- 公共跨 agent MCP 服务 (#18、#19 及其 Worker 服务)
- Session-scoped public webhook endpoint (依赖已移除服务的功能)

---

## 4. code_walkthrough

将文档 claim 映射到源码目录：

### 4.1 本地模型 (DS4) 优化 → `crates/ai/src/`

- **Reasoning 重放**: provider `openai-responses` 读取 `requiresReasoningContentOnAssistantMessages` 标志，将 assistant thinking 重放为 `{"type":"reasoning"}` input item → `crates/ai/src/providers/` (具体在 openai responses 适配器中)
- **HTTP 409 透明重试**: `crates/ai/src/utils/retry.rs` — 409 加入可重试状态码集合
- **Cache write 报告**: usage 解析中加入 `cache_write_tokens` 字段 → Usage 类型中新增 `cache_write`

### 4.2 Agent Runtime → `crates/agent/src/harness/`

| 文档 claim | 源码入口 |
|---|---|
| AgentHarness 核心 (session, compaction, cost, prompts) | `agent_harness.rs` |
| Session storage (JSONL + in-memory) | `session/` |
| Compaction (自动上下文压缩) | `compaction/` |
| Cost tracking + budget cap | `cost.rs` |
| Skills 加载 + system prompt 渲染 | `skills.rs`, `system_prompt.rs` |
| Trigger runtime (RFC 1 完整 pipeline) | `trigger_runtime.rs`, `trigger.rs` |
| Permission policy (危险命令检测 + approve/deny) | `permission.rs` |
| Notification hook (MCP push → trigger 转换) | `notification_hook.rs` |
| Prompt templates | `prompt_templates.rs` |
| CLI hooks (生命周期事件) | `hooks` 模块 (在 coding-agent 中通过 `before_tool_call`/`after_tool_call` 等 hook 点消费) |

### 4.3 CLI/TUI/工具 → `crates/coding-agent/src/`

| 文档 claim | 源码入口 |
|---|---|
| 21+ 斜杠命令 (registry + completion) | `commands/` (slash 命令注册表和实现) |
| TUI (split-view, spinner, markdown, input) | `tui/` |
| 工具实现 (bash, edit, write, read, grep, find, git, ls, web_fetch, web_search, task, memory, truncate) | `tools/*.rs` |
| MCP adapter (server tools → AgentTool) | `tools/mcp_adapter.rs` |
| SkillBuilder tool | `tools/skill_builder.rs` |
| InstallSkill tool (two-phase, SSRF guard) | `tools/install_skill.rs` |
| Trigger source adapters (dynamic file/command poll) | `triggers/dynamic.rs` |
| Cron jobs (session-scoped, local time) | `triggers/cron.rs` |
| MCP notification hook adapter | `triggers/mcp_notification_hook.rs` |
| Loops (stateful cron + inbox tag 提取) | `triggers/cron.rs` (cron_harness_listener 中处理 `<loop-state>` / `<inbox>` 标签) |
| Web relay (pie.0xfefe.me) | 本地 relay client (取决于实现) + Cloudflare Worker (不在本 repo 的 Rust 代码中，可能在 `workers/fefe-hub/`) |
| Session export/import | `session_archive.rs` (`.piesession` tar 格式) |
| Web UI (`pie web` 模式) | `web.rs` / `ui/web_index.html` |

### 4.4 MCP → `crates/mcp/src/`

| 文档 claim | 源码入口 |
|---|---|
| stdio transport | `stdio.rs` |
| Streamable HTTP transport (POST + SSE) | `http.rs` |
| JSON-RPC 2.0 framing | `protocol.rs` |
| Client (inflight management, cancel) | `client.rs` |
| Transport trait | `transport.rs` |

### 4.5 存储布局 (验证 `~/.pie/` 结构)

- `~/.pie/sessions/<cwd-hash>/<uuid>.jsonl` — 会话历史
- `~/.pie/sessions/<cwd-hash>/<uuid>.triggers.json` — session-scoped 动态触发规则
- `~/.pie/sessions/<cwd-hash>/<uuid>.cron.toml` — session-scoped cron jobs
- `~/.pie/sessions/<cwd-hash>/<uuid>.loop-<job-id>.md` — loop state file
- `~/.pie/inbox.jsonl` — 全局 triage inbox
- `~/.pie/auth.json` (mode 0600) — 存储的 API keys
- `~/.pie/models.json` — 用户全局自定义模型定义
- `~/.pie/mcp.toml` — MCP 服务器配置
- `~/.pie/hooks.toml` — 用户全局 CLI hooks
- `~/.pie/config.toml` — 用户配置 (如 `[triggers] poll_interval_secs`)
- `~/.pie/logs/<session>.log` — 结构化日志
- `~/.pie/skills/<name>/SKILL.md` — 用户全局技能

---

## 5. flows

以下 5 条用户视角流程来自文档描述和源码结构的交叉验证：

### 5.1 Local model coding（使用 DS4 本地推理）

```
1. 用户启动 DS4 服务器: ./ds4-server --ctx 100000 --kv-disk-dir /tmp/ds4-kv --kv-disk-space-mb 8192
2. 配置 ~/.pie/models.json 声明 deepseek-v4-flash 模型（含 compat 标志）
3. 启动 pie: ./pie --provider ds4 --model deepseek-v4-flash --base-url http://127.0.0.1:8000/v1
4. 每轮对话:
   a. pie 发送完整历史（包含 reasoning content 重放，保证 byte-exact）
   b. DS4 从磁盘 KV checkpoint 恢复前缀缓存
   c. 仅 prefill 新增 token（`/cost` 显示 cache read >> input）
5. DS4 服务器重启: pie 透明重试 HTTP 409，恢复会话
6. 关键源码: crates/ai/src/providers/ (openai-responses), crates/ai/src/utils/retry.rs
```

### 5.2 Stateful loop (Loop Engineering)

```
1. 用户在 pie 中创建 stateful cron: /cron add --stateful "0 9 * * *" 检查 GitHub issues，报告新增/关闭项
2. 每天 09:00:
   a. cron action hook 检测到 stateful=true，切换为 SubAgent 模式 (不碰主对话)
   b. SubAgent 收到: [loop-state]上次笔记[/loop-state] + job prompt + Output protocol (loop-state/inbox 标签)
   c. SubAgent 执行工作，输出内容包含:
      - <loop-state>今天看到的 issue 基线...</loop-state>
      - <inbox>#42 新 issue: 修复内存泄漏</inbox>
   d. cron_harness_listener 提取: loop-state → 写入 state file; inbox → 追加 inbox.jsonl
3. 用户打开 pie: /inbox 查看新 findings → /inbox claim 1 将 finding 注入主对话
4. TUI 侧栏显示 Inbox: N new 徽标
5. 关键源码: crates/coding-agent/src/triggers/cron.rs (cron_harness_listener、tag 提取)
```

### 5.3 MCP notification → trigger pipeline

```
1. 用户在 ~/.pie/mcp.toml 配置 MCP 服务器 (stdio 或 streamable_http)
2. pie 启动时:
   a. mcp.toml 加载器 spawn 服务器进程
   b. register_notification_hook 创建 driver + pump 两个 tokio task
3. MCP 服务器推送 notification:
   a. pump task 调用 handle_trigger(trigger)
   b. TriggerRuntime 做 dedup (5 分钟窗口) + cycle suppression (max 5 hop)
   c. BeforeTriggerHook 做权限判断 (Allow / Deny / Prompt)
   d. OnTriggerPromptHook 做用户确认 (仅在 Prompt 路径)
   e. 审计: 写入 SessionTreeEntry::Custom { custom_type: "trigger" }
   f. 执行: spawn SubAgent (或 InjectSummary / InjectAndRun)
   g. 结果: 写入 trigger_result 审计 → 可选 promotion 到主对话
4. /triggers 命令显示运行时状态: hooks、running、audit
5. 关键源码: crates/agent/src/harness/trigger_runtime.rs, trigger.rs, notification_hook.rs
```

### 5.4 Skill Builder（从自然语言创建技能）

```
1. 用户在 pie 中对话: "帮我把这个 code review 流程存成一个技能"
2. 模型调用 SkillBuilder tool:
   a. Preview (confirm: false): 渲染 SKILL.md、验证、预览 metadata → 无副作用
   b. Confirm (confirm: true, overwrite: true): 原子写入 ~/.pie/skills/<name>/SKILL.md → hot-reload → audit
3. 技能立即可用: /skills 列表中出现新技能，/skill <name> 可附加到下一轮 prompt
4. 权限: SkillBuilder 走 ControlPlaneWrite → PermissionCategory::ControlPlaneWrite
5. 关键源码: crates/coding-agent/src/tools/skill_builder.rs, crates/agent/src/harness/skills.rs
```

### 5.5 Web relay (远程观看 + prompt)

```
1. 用户在 pie 中输入 /web-connect
2. pie:
   a. 生成 160-bit view token + agent key
   b. 通过 WSS 连接到 pie.0xfefe.me/relay/agent (Cloudflare Worker)
   c. 打印 URL: https://pie.0xfefe.me/session/<view_token>
3. 任何浏览器打开该 URL:
   a. 加载共享 viewer HTML (GET /session/<token>)
   b. SSE 订阅 /session/<token>/events → 实时接收 conversation snapshots
   c. 可提交 prompt: POST /session/<token>/prompt → Worker 转发到 agent WS
4. pie 本地: 远端 prompt 通过 run-queue 注入 (与本地 Web UI 提交同路径)
5. /web-disconnect: 关闭 WS，Worker 删除 DO 状态，后续访问返回 404
6. 安全: agent key pinning (TOFU)、view token 能力模型、credentials 不经过 relay
7. 关键源码: crates/coding-agent 中的 relay client (WebSocket 连接管理); workers/fefe-hub (Cloudflare Worker DO)
```

---

## 6. tests

### 6.1 现有测试覆盖

从 `CHANGELOG.md` 可以确认的关键测试集：

- **工作区总计**: 27 test binaries, ~225 tests, clippy `-D warnings` clean
- **Trigger runtime (RFC 1)**: 21+ 个集成测试，覆盖 dedup、cycle suppression、audit shape、promotion (5 条路径)、prompt request、execution started/completed/failed、abort、running snapshot
- **Permission system**: 25 个危险 bash 变体 (所有 short/long/separated/mixed flag 组合)、4 个 near-miss 测试
- **InstallSkill**: 11 个单元测试 (preview 无副作用、name traversal rejection、SSRF guard、overwrite/idempotent、audit shape)
- **OnTurnEndHook**: 新测试覆盖 hook unset 保持传统行为、Stop 决策、Continue 决策 (运行第二个 turn)、continuation cap
- **AgentHarness::reload_skills_from_disk**: 4 个回归测试
- **Streamable HTTP MCP**: hermetic faux HTTP/SSE 测试 (POST、SSE notification、body-cap rejection、bearer header injection)
- **AgentTool::prepare_arguments + ToolExecutionUpdate**: 2 个新集成测试
- **Session compaction**: 修复后验证 build_session_context 跳过 trigger Custom 条目

### 6.2 CI 测试策略

- 所有测试禁止真实网络调用 (CI 清除所有 provider API key 环境变量)
- 使用 faux providers + tempdirs + wiremock
- `make ci` = fmt-check + lint + test
- 专项测试: `make test-coding-agent` / `make test-agent` / `make test-ai` / `make test-mcp`

### 6.3 测试缺口

| 方向 | 缺口 |
|------|------|
| Session export/import (#20) | CLI e2e 测试待补充 (`pie session export --current --output x.piesession` 完整流程) |
| Loops Phase 3 (maker/checker) | 整个 phase 3 未实现，无测试 |
| Web relay e2e | wrangler dev 本地 e2e 测试待补充 |
| Multi-leaf session (#17) | round-trip 确定性测试待落地 |
| `/goal` session goal mode | 文档提到但测试覆盖不详 |
| 跨平台 PTY 测试 | macOS + Linux CI 矩阵覆盖，但 resize/SIGWINCH 等边界测试深度待验证 |
| 本地模型 (DS4) 集成测试 | 需要启动真实 ds4-server，文档描述为手动验证 (`/cost` 观察 cache read 数值) |

---

## 7. risks

### 7.1 路线风险

1. **单人维护者风险**: README 提到"大部分代码由 AI 编写"，CHANGELOG 中大量 feature 标注 `Owner: c4pt0r`。单个维护者承载所有设计决策、code review 和安全审计。如果维护者中断，项目有 bus factor = 1 的风险。

2. **功能范围膨胀**: master roadmap 覆盖 8 个 Tier、20+ 个子 issue，且每个子 issue 都要求详细的设计文档（Goal / Architecture / Stability / Extensibility / Performance / Testing）。实际已实现的 feature 数量远超 roadmap 标记——CHANGELOG 中 80%+ 已标记实现。继续此速度需要持续的高生产力。

3. **移除功能的遗留债务**: 公共 MCP 跨 agent 服务的代码已从 CLI 移除，但 Worker tombstone 清理是单独步骤。`docs/endpoints.md`、`docs/superpowers/` 中的文档均已归档但保留在仓库中。如果未来重新引入类似功能，需要从头设计。

4. **DS4 紧耦合风险**: `docs/ds4.md` 强调优化是 "first-class attention" 且 compat 标志通用，但项目起源和大量优化都围绕 DS4。如果 DS4 演进方向改变，或用户切换到其他本地推理引擎，修复的投入可能无法迁移。

### 7.2 未决设计问题

1. **MCP inject_and_run 反馈循环**: `CLAUDE.md` 明确描述了 `inject_and_run` 服务器可能导致的反馈循环问题，但解决方案仅是文档级别的"配置意图 + 服务器侧节流"。没有运行时层面的硬性防护。

2. **Trigger 的 approve 流程**: v1 中 `promote_requires_approval = true` 的路径是 fail-closed to pending，`/triggers approve` 命令尚未实现。design 文档标注为"deliberate security choice"——安全但不完整。

3. **多 leaf session 的 JSONL 表示**: #17 的设计提到 `active_leaf_id` 和 `branch_from(leaf, prompt)`，但当前 session JSONL 格式是线性的——多分支如何在 append-only JSONL 中表示，实现细节待验证。

4. **Web relay 的 TOFU 重 pin**: Worker DO 重启后接受它看到的第一个 agent key——"acceptable for v1"。这意味着存在短暂窗口，如果 agent 断开且攻击者在 agent 重连之前带上 view token 连接，可以重 pin agent key。文档标注为 v1 已知限制。

5. **`inject_and_run` 的隐式打断**: 当 Idle 状态下的 InjectAndRun 触发时，`TriggerRequestsMainRun` 事件被 emit 到 run channel。如果用户正在打字但未提交，这个自动 turn 可能会产生令人困惑的时间线。

### 7.3 技术债

1. **Legacy bad `firstKeptEntryId` 问题**: compaction 之前会写入永不可达的 entry ID，`--resume` 静默丢弃 pre-compaction 消息。虽已修复（`#19`），但已有会话文件可能包含损坏的 compaction 记录。文档确认有 best-effort 恢复路径，但不保证完整性。

2. **`PromoteAction::PromoteSummaryWhenSummaryContains` 已弃用**: 被结构化 authorization 替代，但 coding-agent 的 `dynamic.rs` 仍在使用（标注 `#[allow(deprecated)]`）。

3. **AgentTool::prepare_arguments 之前未调用**: 在 `#39 (PR-B)` 之前，工具一直从 assistant tool call 获取原始参数而不经 `prepare_arguments` 归一化。所有工具之前都依赖 bug-for-bug 兼容性。

4. **Session export 不包含 sidecars**: v1 `.piesession` 格式不 bundle loop state、不 bundle 技能/模板文件、不 bundle MCP config。用户必须在导入后手动重建这些。

5. **`find` 工具搜索默认路径上限**: 从 1000 → 200 (修复后)，可能对一些小项目日志场景仍然过宽；对大项目可能仍然不够紧。

---

## 8. next_questions

下一轮精读可以关注以下问题：

1. **AgentHarness 内部线程模型**：`AgentHarness` 使用 `Arc<Self>` 共享引用，而 trigger sub-agent、hook pump、cron scheduler 都持有 harness 引用。并发模型（内部锁类型、turn slot 序列化、状态变更的可见性保证）如何确保不出现竞态条件？

2. **JSONL session 的格式边界**：`SessionTreeEntry` 的 enum 变体有哪些？新加的 `Custom { custom_type: "trigger" | "trigger_result" | "trigger_promotion" | "skill_install" | "cron_control_plane" | ... }` 类型有多少？它们的序列化/反序列化保证是什么？compaction 时如何处理 Custom 条目？

3. **LSP 集成的实际深度**：工具列表中有 `ls.rs`、`find.rs`、`grep.rs`，但 LSP supervisor 的"after_tool_call hook that attaches diagnostics" 的实际效果如何？支持的 language servers 具体有哪些？如何解决 LSP 启动延迟和 agent turn 的关系？

4. **Web UI 的实际状态**：`docs/web-ui-parity.md` 描述了完整的 TUI parity gate，但当前 `pie web` 命令的实际完成度如何？SSE snapshot 的格式和 debounce 策略是什么？

5. **Skills system prompt 的 token 效率**：每个 skill 的完整 body 注入 system prompt 的开销有多大？在 10+ skills 的情况下，是否自动做 compaction 或 lazy loading？

6. **provider 层面的错误处理一致性**：`retry.rs` 的统一重试层覆盖了哪些 provider？不同 provider 的 SSE/event-stream 解析是否有统一抽象？`input_json_delta` 碎片组装是在哪个层处理的？

7. **`/goal` session goal mode 的 evaluator prompt 策略**：evaluator 用什么 prompt 判断 "是否达到目标"？transcript 是如何 bounded 后传给 evaluator 的？有 false positive/negative 的缓解措施吗？

8. **Inbox JSONL 的跨进程一致性**：文档提到 "concurrent pie processes appending is tolerated"+"rewrite races are last-writer-wins on status"。实际实现是否使用文件锁（`fs2` advisory lock）？还是完全乐观并发？
