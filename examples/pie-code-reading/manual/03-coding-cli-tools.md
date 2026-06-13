# pie-coding-agent CLI / TUI / Tools 粗读报告

> 阅读基线：`f1c35a3`
> 深度档位：`architecture`
> 目标文件：`crates/coding-agent/`

---

## 1. problem —— CLI/TUI/tools 层要解决什么问题

`pie-coding-agent` 是 `pie` 的终端交互入口。它在 `pie-agent-core`（agent 运行时）和 `pie-ai`（LLM 流式客户端）之上构建了一整套**终端 REPL 体验**，核心职责是：

- **将终端交互、slash commands、工具执行、配置管理、会话持久化与 agent core 串联起来**，形成一个可用的 coding agent 产品。
- 提供三种 UI 模式：**全屏 TUI**（ratatui）、**Web UI**（本地 HTTP 服务）、**Headless**（非 TTY 管道模式）。
- 通过 `main.rs` 完成从 CLI 参数解析 → 模型自动检测 → 会话 cwd-scoped 管理 → AgentHarness 装配 → UI 启动的完整流水线。
- 通过 `commands.rs` 提供 28 个内置 slash commands（`/help`, `/model`, `/skills`, `/thinking`, `/save`, `/compact`, `/undo`, `/goal`, `/triggers`, `/cron`, `/login`, `/session`, `/web-connect` 等），并支持通过 `trait SlashCommand` 扩展。
- 通过 `tools/` 目录提供 11 个核心工具（read/write/edit/bash/ls/grep/find/web_fetch/web_search/git/memory）以及 skill/MCP/task 等高级工具。
- 通过 `ui/` 实现全屏 TUI（ratatui + tui-textarea），支持流式渲染、滚动、输入历史、Ctrl-C 中断、图片/剪贴板、远程 relay 连接。

一句话：**`pie-coding-agent` 是 pie 的"外壳"，负责把 agent core 的能力以人类可用的方式暴露在终端、Web 和管道中。**

---

## 2. why_hard —— 为什么难

### 2.1 交互式 TUI 的复杂度

- **流式渲染与输入并发的调度**：模型 turn 是 `!Send` future（`AgentSession::prompt` 含 `parking_lot` 跨 `.await` 的 guard），不能用 `tokio::spawn`。解决方式是 `select!` 宏在同一 event loop 中轮询 turn future + 键盘事件 + feed channel（`ui/kernel.rs:31`）。
- **三种 UI 模式共享内核**：`ReplKernel` 在 `ui/kernel.rs` 中抽象了 turn 语义（UserPrompt/AgentPrompt/PromptTemplate/Compaction），终端和 Web 前端都复用同一套执行逻辑。
- **Ctrl-C 行为冲突**：raw mode 下 Ctrl-C 是键盘事件而非 SIGINT，需要手动处理终端 raw mode 的 enter/leave（`ui/mod.rs:45-47`）。

### 2.2 历史/恢复机制

- 会话按 **cwd hash** 分桶存储（`config.rs:33-38`），`--resume` 默认恢复当前目录最近一次会话，也可通过 UUIDv7 前缀指定。
- `Resume` hydration：恢复时必须重建 agent 的 transcript tree，并在 feed 中重新渲染历史消息（`main.rs:1019-1022`）。
- 支持 `.piesession` 归档导出/导入（`main.rs:150-186`），包含 transcript + sidecar（triggers/cron），但不包含凭证和 MCP 配置。

### 2.3 文件编辑安全

- `edit::EditTool` 基于精确文本匹配替换（而非行号），要求先 `read` 再 `edit`，避免并发修改导致 corruption。
- LSP 诊断反馈：`lsp_supervisor.rs` 在 write/edit 后附加 diagnostics（`main.rs:781-786`），`after_tool_call` hook 注入。
- 权限策略：`before_tool_call` hook 实现了 `PermissionPolicy::default_for_coding_agent()`（`main.rs:751-752`），可中断危险操作。

### 2.4 Skills / MCP 加载的鸡与蛋问题

- **Skill 工具**需要在构造时拿到 `AgentHarness` 的引用才能读取 `harness.skills()`，但 harness 的构造又需要 tool list。解决：使用 `Arc<OnceCell<Arc<AgentHarness>>>`（即 `SkillHarnessCell`），在 harness 构造后、REPL 接受输入前设置（`main.rs:802-805`）。
- **MCP 工具**需要在 harness 构造前完成 server 连接和 `tools/list` handshake（`mcp_loader.rs`），工具以 `McpAgentTool` 适配器形式注入 tool registry。

### 2.5 图片/剪贴板支持

- 通过 `--image` CLI 参数附件图片（PNG/JPEG/WebP/GIF，≤10MB，最多 10 张/消息）（`main.rs:105-108`）。
- 支持从剪贴板粘贴图片（`clipboard_image.rs`），通过 `arboard` 获取系统剪贴板内容。
- TUI 中通过 bracketed paste 检测图片 MIME 类型。

### 2.6 Web UI Parity

- `--web` flag 启动本地 HTTP 服务（默认 loopback），提供与 TUI 功能对等的 Web 界面（`ui/web.rs` + `ui/web_index.html`）。
- 通过 `--web-connect`（`/web-connect`）将本地 TUI 会话 relay 到远程 pie relay 服务，实现远程访问。
- UI 模式默认决策：本地 TTY → Web UI（`UiMode::Web`），远程 SSH → TUI（`UiMode::Tui`），非 TTY → Headless（`main.rs:1087-1098`）。

---

## 3. design_approach —— 设计思路

### 3.1 整体数据流

```
CLI args (clap)
    │
    ▼
main.rs: Cli::parse()
    │
    ├─→ config.rs: base_dir() → ~/.pie/
    │       ├─ sessions_dir_for_cwd(cwd) → ~/.pie/sessions/<cwd-hash>/
    │       └─ memory_dir() → ~/.pie/memory/
    │
    ├─→ model.rs: auto_detect_model() ← env vars / auth store
    │
    ├─→ session/mod.rs: create() / resume() → Session
    │
    ├─→ skills.rs: load_all(cwd) → skills (project + user)
    ├─→ builtin_skills.rs: resolve_builtins() → built-in skills
    ├─→ skills_state.rs: apply overlay (enable/disable)
    ├─→ mcp_loader.rs: load_all(cwd) → MCP tools + notification hooks
    ├─→ templates.rs: load_all(cwd) → prompt templates
    │
    ├─→ tools/mod.rs: default_tools() + task_tool() + skill_tool() + ...
    ├─→ tools/memory.rs: load_memory_block() → system prompt
    │
    ├─→ AgentHarnessOptions { model, tools, skills, system_prompt, stream_fn, ... }
    ├─→ AgentHarness::new(opts)
    ├─→ skill_harness_cell.set(harness)  ← 解除鸡与蛋依赖
    │
    ├─→ feed channel (feed_tx, feed_rx)  ← UI 事件总线
    ├─→ harness.subscribe(agent_listener(feed_tx))
    ├─→ harness.subscribe_harness(harness_listener(feed_tx))
    │
    ├─→ ui::App::new(AppConfig { harness, registry, feed_rx, ... })
    │
    └─→ app.run() / app.run_web()
            │
            ├─ full-screen TUI (ratatui)
            │   ├─ select! {
            │   │     event = event_stream.next()  → 键盘/鼠标处理
            │   │     update = feed_rx.recv()      → 流式渲染
            │   │     turn = poll_turn(fut)        → model turn 推进
            │   │     main_run = main_run_rx.recv() → inject-and-run trigger
            │   │     relay_prompt = relay_prompt_rx.recv() → 远程 prompt
            │   │     control_prompt = control_plane_prompt_rx.recv() → 确认弹窗
            │   │  }
            │   └─ render: feed scrolling + input box + status bar
            │
            └─ Web UI (axum)
                ├─ POST /api/prompt → kernel.run_prompt()
                ├─ GET  /api/feed → SSE stream
                └─ static: web_index.html
```

### 3.2 核心设计决策

1. **事件驱动而非直接写入 stdout**：`AgentEvent` / `HarnessEvent` → `FeedUpdate` channel → ratatui terminal writer。所有输出走单一 channel，避免并发写终端导致的乱码（`ui/listener.rs`）。

2. **Slash command 通过 `CommandOutcome` enum 注入 REPL 控制流**：command 不直接 await harness，而是返回 `RunAgentPrompt` / `RunCompaction` 等 variant，由 REPL 层统一调度，确保 Ctrl-C/Esc 能一致地中断（`commands.rs:64-112`）。

3. **Tool 注册与 harness 解耦**：`default_tools()` 返回无状态 tool 实例，`task_tool()` / `skill_tool()` 需要额外参数单独构造；MCP tools 通过 adapter 模式注入（`tools/mcp_adapter.rs`）。

4. **Session 按 cwd SHA256 hash 分桶**：`sessions_dir_for_cwd()` 对工作目录做 12 字符 hex hash，实现 `--resume` 自动 scoping（`config.rs:33-38`）。

5. **Skills 三层优先级**：builtin < user (`~/.pie/skills/`) < project (`<cwd>/.pie/skills/`)，project 可覆盖同名 skill。`skills_state.json` 作为运行时 enable/disable overlay，不修改 SKILL.md 文件（`skills.rs:62-67`, `skills_state.rs`）。

6. **Trigger / Cron 作为 session sidecar**：动态 trigger 和 cron job 存储在 session 旁侧文件（`.triggers.json` / `.cron.toml`），而非全局配置，确保不同 session 的自动化互不干扰（`session/mod.rs:24-32`）。

---

## 4. code_walkthrough —— 关键文件/类型/函数

### 4.1 `main.rs` (1559 行)

**入口点**：`#[tokio::main] async fn main()`

**关键类型**：
- `Cli`：clap derive parser，定义全量 CLI 参数（`main.rs:57-147`）
  - 模型选择：`--provider`, `--model`, `--base-url`, `--thinking`
  - 会话管理：`--resume`, `--continue`, `--resume-id`, `--list-sessions`, `--delete-session`
  - 输入附件：`--image`（可重复）
  - UI 控制：`--web`, `--tui`, `--web-host`, `--web-port`
  - 权限：`--yes`, `--always-allow`
  - 调试/扩展：`--debug`, `--builtin-skill`
- `CliCommand::Session { Export/Import }`：子命令，支持 `.piesession` 归档操作
- `UiMode`：`Web | Tui | Headless`（`main.rs:1072-1076`）

**关键函数**：
- `run_repl()`：核心 REPL 启动流程（`main.rs:546-1069`）
  - 模型检测 → 会话创建/恢复 → logging 初始化 → feed channel 创建
  - AgentHarness 装配：tools（default + task + skill + MCP + trigger + cron）、skills（builtin + user + project + overlay）、LSP supervisor、hooks、triggers
  - `skill_harness_cell.set(harness)` 解除循环依赖
  - notification hooks 注册（MCP + cron + dynamic trigger）
  - subscription 建立（agent events → feed updates）
  - App 启动（`app.run()` 或 `app.run_web()`）
- `auto_detect_model()` 调用链：`model::auto_detect_model()`
- `compose_system_prompt()`：构造 system prompt，包含 tool inventory 和 cwd 信息（`main.rs:1186-1197`）
- `render_base_prompt()`：提示 LLM 何时使用 cron/trigger/skill 工具（`main.rs:1202-1222`）

### 4.2 `commands.rs` (3584 行)

**关键类型**：
- `trait SlashCommand`：`name()`, `aliases()`, `description()`, `usage()`, `async run()`（`commands.rs:183-197`）
- `CommandCtx<'a>`：运行时上下文 `{ harness, session_id, log_path, tool_count, cwd }`（`commands.rs:175-181`）
- `CommandOutcome`：命令执行结果，驱动 REPL 行为（`commands.rs:66-112`）
  - `Handled` / `Quit` / `ClearScreen` / `Error(String)`
  - `AttachSkill { name }`：将 skill 附加到下一次 prompt
  - `RunAgentPrompt { prompt }`：通过 harness 执行 prompt（统一 Ctrl-C 路径）
  - `RunPromptTemplate { name, vars }`：渲染并执行模板
  - `RunCompaction { custom }`：触发 context compaction
  - `LoginSecret { provider }`：无回显输入凭证
  - `WebRelay(WebRelayAction)`：远程 relay 控制
  - `SessionImportActivation`：导入后激活 prompt
- `Registry`：`Vec<Arc<dyn SlashCommand>>` + `find(name)` + `with_builtins()`（`commands.rs:200-267`）

**28 个内置命令**（`with_builtins()`）：
`help`, `clear`, `skills`, `skill`, `quit/exit/q`, `model`, `thinking`, `cost`, `diag`, `template`, `save`, `compact`, `undo`, `bug-report`, `name`, `session`, `web-connect`, `web-disconnect`, `sessions`, `share`, `login`, `logout`, `find`, `history`, `goal`, `goal-start`, `triggers`, `new-trigger`, `cron`, `inbox`

**关键函数**：
- `parse(input: &str) -> Option<(String, Vec<String>)>`：slash command 解析器，支持双引号转义（`commands.rs:271-297`）
- `console::emit_line()`：输出路由（TUI → feed channel；stdout fallback）（`commands.rs:45-51`）
- `attach_skill_prompt()`：`/skill <name>` 后包装 prompt 为 Skill tool 调用（`commands.rs:841-849`）
- `print_help_with_skills()`：动态生成帮助文本，包含 skill shortcuts（`commands.rs:1134-1136`）

### 4.3 `ui/mod.rs` (3140 行)

**关键类型**：
- `AppConfig`：harness + registry + channels + panel_status 等启动参数（`ui/mod.rs:101-116`）
- `App`：全屏 TUI 的状态机（`ui/mod.rs:118-173`）
  - 字段：`kernel: ReplKernel`, `feed: Feed`, `input: TextArea`, `history: HistoryStore`, `completions: Vec<String>`, `relay: Option<RelayHandle>`, `pending_import_activation`
  - UI 状态：`scroll`, `follow`, `busy`, `spinner_frame`, `quit`
- `Feed`：滚动式对话流（`ui/feed.rs`），支持 text/thinking/tool_call/image/plain 等多种消息类型
- `FeedUpdate`：从 listener 到 App 的事件消息体（`ui/feed.rs`）
  - `TurnStart/TurnEnd`, `TextDelta/ThinkingDelta`, `ToolStart/ToolEnd/ToolProgress`, `Trigger*`, `GoalUpdate`, 等

**关键函数**：
- `App::run()`：主事件循环（`ui/mod.rs` 约 500+ 行），核心 `select!` 如下：
  ```
  select! {
      event = event_stream.next() => handle_input(event)
      update = feed_rx.recv()     => feed.push(update); render
      turn   = poll_turn(fut)     => handle_turn_result(turn)
      main_run = main_run_rx.recv() => run_inject_and_run_turn()
      relay_prompt = relay_prompt_rx.recv() => queue relay prompt
      abort = relay_abort_rx.recv() => abort current turn
      resolve = relay_resolve_rx.recv() => control-plane resolve
  }
  ```
- `App::banner()` / `App::system_line()` / `App::error_line()`：启动信息渲染
- `App::replay()`：`--resume` 时重放历史消息（`main.rs:1019-1022`）
- `App::run_web()`：切换到 axum HTTP server

### 4.4 `ui/kernel.rs` (160 行)

**关键类型**：
- `ReplKernel`：共享 REPL 执行内核（`ui/kernel.rs:69-73`）
- `TurnFut = Pin<Box<dyn Future<Output = Result<Option<String>, AgentRunError>>>>`：`!Send` future（`ui/kernel.rs:21`）
- `TurnState`：`{ fut: Option<TurnFut>, aborted: bool, prefix: &'static str }`（`ui/kernel.rs:24-29`）
- `QueuedTurn`：`UserPrompt | AgentPrompt | PromptTemplate | Compaction`（`ui/kernel.rs:36-56`）

**关键函数**：
- `poll_turn()`：轮询 turn future（`ui/kernel.rs:31-34`）
- `ReplKernel::run_prompt()`：提交 user prompt → `AgentSession::prompt()`
- `ReplKernel::run_template()`：渲染模板 → prompt
- `ReplKernel::run_compaction()`：调用 harness compaction
- `ReplKernel::abort()`：设置 cancel token → 中断当前 turn

### 4.5 `ui/listener.rs` (638 行)

**关键函数**：
- `agent_listener(tx)`：`AgentEvent` → `Vec<FeedUpdate>` 映射（`ui/listener.rs:21-30`）
  - `AgentStart` → `TurnStart`
  - `MessageUpdate::TextDelta` → `TextDelta`
  - `MessageUpdate::ThinkingDelta` → `ThinkingDelta`
  - `ToolExecutionStart` → `ToolStart { name, args }`
  - `ToolExecutionEnd` → `ToolEnd { lines, is_error }`
  - `AgentEnd` → `TurnEnd`
- `harness_listener(tx)`：`HarnessEvent` → `Vec<FeedUpdate>` 映射（trigger 相关事件）（`ui/listener.rs`）
  - `TriggerHandlingStart` / `TriggerHandled` / `TriggerCompleted` / `TriggerFailed` / `TriggerExecutionStarted`
  - 静默动态 trigger 的周期性检查（no-match 时不下沉到 feed）

### 4.6 `tui.rs` (523 行)

**Legacy 输出层**：原直接写 stdout 的 colored line-stream 渲染器，现被 `ui/` 取代但保留用于：
- `render_persisted()`：恢复会话时重放历史消息（`tui.rs:452-522`）
- 测试中用于 capture ANSI escape 输出（`tui_render_e2e.rs`）

### 4.7 `readline.rs` (144 行)

- `SlashCompleter`：slash command 补全器（`readline.rs:12-58`）
- `matches(line)`：匹配输入前缀，返回候选 `/command` 列表
- 正确处理 skill shortcuts（enabled 时出现，disabled 时隐藏）、别名、已完整输入时不重复提示

### 4.8 `config.rs` (140 行)

**关键函数**：
- `base_dir()`：`$PIE_DIR` 或 `$HOME/.pie`（`config.rs:10-17`）
- `sessions_dir_for_cwd(cwd)`：`~/.pie/sessions/<cwd-hash>/`（`config.rs:21-24`）
- `cwd_hash(cwd)`：SHA256 前 6 字节 hex（`config.rs:33-38`）
- `memory_dir()`：`~/.pie/memory/`（`config.rs:27-29`）
- `parse_trigger_poll_interval_secs()`：TOML 配置解析（`config.rs:44-57`）
- `parse_relay_base_url()`：relay 服务地址配置（`config.rs:64-75`）

### 4.9 `model.rs` (171 行)

- `CANDIDATES`：优先级顺序 `[(ANTHROPIC_API_KEY, 8 providers)]`（`model.rs:9-18`）
- `auto_detect_model(override_provider, override_model)`：自动检测第一个有凭证的 provider（`model.rs:23-69`）
- `credential_less_default()`：无凭证时的 fallback model（允许 notification-only session 启动）（`model.rs:74-79`）
- `explicit_model_not_found_message()`：友好的模型未找到错误消息（`model.rs:81-122`）

### 4.10 `history.rs` (134 行)

- `HistoryStore`：`~/.pie/history` 持久化，容量 1000 条（`history.rs:12-79`）
- 相邻重复自动去重，空白行跳过
- 同步写入磁盘

### 4.11 `session/mod.rs` (681 行)

**关键函数**：
- `open_repo(cwd)`：按 cwd hash 打开 `JsonlSessionRepo`（`session/mod.rs:20-22`）
- `create(repo, cwd)` / `resume(repo, explicit_id)`：会话 CRUD（`session/mod.rs:77-96`）
- `list_entries(repo)`：列出当前 cwd 下的所有 session，含 preview（`session/mod.rs:100`）
- `delete_by_id()` / `find_path_by_id()`：精确删除
- `trigger_sidecar_path` / `cron_sidecar_path`：session 旁侧文件路径
- `automation_elsewhere_hint()`：提醒用户同 cwd 下其他 session 有自动化

### 4.12 `tools/mod.rs` (173 行)

**工具注册中心**：
- `default_tools(memory_dir)` → `[Read, Write, Edit, Bash, Ls, Grep, Find, WebFetch, WebSearch, Git, Memory]`（`tools/mod.rs:31-45`）
- `subagent_read_only_tools()` → `[Read, Ls, Grep, Find, WebFetch, Git]`（`tools/mod.rs:50-59`）
- `task_tool(model, stream_fn)` → `TaskTool`（`tools/mod.rs:63-72`）
- `skill_tool(cell)` → `SkillTool`（`tools/mod.rs:82-84`）
- `install_skill_tool(cell)` → `InstallSkillTool`（两阶段：preview → confirm）（`tools/mod.rs:91-93`）
- `skill_builder_tool(cell)` → `SkillBuilderTool`（从结构化字段渲染 SKILL.md）（`tools/mod.rs:100-102`）
- `set_skill_state_tool` / `remove_skill_tool`：运行时 skill 管理
- `new_cron_job_tool` / `list_cron_jobs_tool` / `remove_cron_job_tool` / `set_cron_job_state_tool`：cron job 工具
- `new_trigger_tool` / `list_triggers_tool` / `remove_trigger_tool` / `set_trigger_state_tool`：动态 trigger 工具

### 4.13 `skills.rs` (68 行)

- `skills_dirs(cwd)` → `(project: <cwd>/.pie/skills/, user: ~/.pie/skills/)`（`skills.rs:18-22`）
- `load_all(cwd)` → `LoadedSkills { skills, diagnostics }`：先 user 后 project，project 覆盖同名（`skills.rs:33-58`）
- `dedupe_project_wins()`：按 name 去重，后插入覆盖前插入（`skills.rs:62-67`）

### 4.14 `mcp_loader.rs` (524 行)

- `McpConfig` / `ServerConfig`：TOML 配置结构，支持 `stdio` 和 `streamable_http` 两种 transport（`mcp_loader.rs:24-56`）
- `load_all(cwd)`：读取 `~/.pie/mcp.toml` + `<cwd>/.pie/mcp.toml`，spawn 每个 server，运行 `initialize` + `tools/list` handshake
- 返回 `LoadedMcp { tools, diagnostics, client_count, notification_hooks, inject_summary_servers, inject_and_run_servers }`
- `inject_summary` / `inject_and_run`：MCP server 可选直通模式，bypass 子代理直接将推送注入父聊天

---

## 5. flows —— 关键流程

### 5.1 普通 REPL 请求流程

```
1. 用户在 input box 输入 prompt，按 Enter
2. App 将输入行推送到 feed（you> prompt）
3. history.append(prompt) → 持久化到 ~/.pie/history
4. 检测是否为 slash command：
   - 是 → commands::parse(input) → registry.find(name) → command.run(argv, ctx) → CommandOutcome
   - 否 → kernel.run_prompt(prompt, skill_name, images) → AgentSession::prompt()
5. select! 中 poll_turn(fut) 推进 turn，同时 drain feed_rx：
   - TextDelta → feed 流式追加文本
   - ThinkingDelta → feed 显示 thinking block
   - ToolStart → feed 显示 ⚙ tool_name(args)
   - ToolEnd → feed 显示 tool 结果
6. TurnEnd → 状态栏更新（cost, 完成标识）
7. 循环回到等待输入
```

### 5.2 Slash command 流程（以 `/model openai:gpt-4o` 为例）

```
1. App 检测到输入以 `/` 开头
2. commands::parse("/model openai:gpt-4o") → ("model", ["openai:gpt-4o"])
3. registry.find("model") → ModelCommand
4. ModelCommand::run(["openai:gpt-4o"], ctx)
   - parse_model_spec("openai:gpt-4o") → ("openai", "gpt-4o")
   - get_model(Provider::from("openai"), "gpt-4o") → Model
   - harness.set_model(model).await
   - cprintln!("switched to openai:gpt-4o") → 输出路由到 feed
5. 返回 CommandOutcome::Handled
```

### 5.3 工具调用流程（以 `write` 为例）

```
1. LLM 返回 ToolCall { name: "write", arguments: { filePath, content } }
2. AgentHarness 触发：
   - before_tool_call hook → 权限检查（可中断）
   - ToolExecutionStart event → listener → FeedUpdate::ToolStart → feed 显示 ⚙ write(...)
   - WriteTool::execute(tool_call_id, args, cancel_token, before)
     - 检查 is_within_workspace() 安全
     - tokio::fs::write(path, content)
     - 返回 AgentToolResult { content: "Wrote N bytes..." }
   - ToolExecutionEnd event → listener → FeedUpdate::ToolEnd → feed 显示结果
   - after_tool_call hook → LSP diagnostics 注入（如果配置）
3. LLM 继续流式生成（或返回最终回复）
```

### 5.4 Session resume / export 流程

**Resume：**
```
1. pie --resume (bare) → select_resume_session()
   - list_entries(repo) → 列出所有 session
   - resume_picker::pick_blocking() → 终端选择菜单
   - 用户选择 → repo.open(path).await
2. pie --resume <id> → session::resume(repo, Some(id))
   - find_session_path(repo, files, id) → 匹配 UUIDv7 前缀
   - repo.open(chosen).await
3. harness.rehydrate_from_session().await → 重建 transcript tree
4. app.replay(ctx.messages) → 在 feed 中重放历史消息
5. REPL 正常启动
```

**Export：**
```
1. pie session export --session <id> --output backup.piesession
2. session_archive::export_session(session_path, output_path, exclude_triggers)
   - 读取 .jsonl transcript
   - 收集 .triggers.json / .cron.toml sidecars（可选排除）
   - 打包为 .piesession tar.gz 归档
3. 打印 summary（entries, triggers, cron）
```

---

## 6. tests —— CLI/tools/TUI 相关测试

### 6.1 Unit tests（内联在源文件中）

| 文件 | 测试内容 |
|------|----------|
| `main.rs:1309-1558` | CLI flag 解析（`--resume` 各种变体）、auth wrapper、base-url 验证、session export/import 命令解析、UI mode 决策、trigger poll interval 读取 |
| `commands.rs`（内联） | slash command 各 handler 的单元逻辑 |
| `readline.rs:75-144` | SlashCompleter 的 filter、prefix、exact match、skill shortcuts 行为 |
| `config.rs:106-140` | trigger poll interval TOML 解析、relay base URL 解析 |
| `history.rs:81-134` | HistoryStore 的 append/dedup/cap/persist 行为 |
| `model.rs:129-171` | credential_less_default 有效性、custom model 注册与检测 |
| `tools/*.rs` | 各工具的单元测试（read/write/edit/bash 等） |

### 6.2 Integration tests (`tests/`)

| 测试文件 | 内容 |
|----------|------|
| `tests/commands.rs` (2034 行) | Slash command 集成测试，通过 `#[path = "../src/commands.rs"]` 引入源码直接测试。覆盖 `/thinking`, `/model`, `/skills`, `/save`, `/compact`, `/undo`, `/goal`, `/triggers`, `/cron`, `/login` 等 |
| `tests/tui_render_e2e.rs` (860 行) | TUI 渲染 e2e 测试。通过 `#[path = "../src/tui.rs"]` 引入源码，驱动 thinking → text → tool call → tool result → agent end 事件序列，验证 capture 的 ANSI 字节流（`strip_ansi` 后比对文本内容） |
| `tests/tools.rs` (640 行) | 工具 e2e 测试（read/write/edit/bash/ls/grep/find/web_fetch/memory/git/skill/skill_builder），通过 `#[path]` 引入工具模块 |
| `tests/cli_session.rs` | CLI session 管理命令（list/delete/resume/export/import）的 e2e 测试 |
| `tests/cli_skills.rs` | CLI skill 管理命令的 e2e 测试 |
| `tests/cli_help.rs` | CLI help/catalog 输出的 snapshot 测试 |
| `tests/web_search_e2e.rs` | WebSearch 工具 e2e |
| `tests/web_fetch_e2e.rs` | WebFetch 工具 e2e |
| `tests/task_tool_e2e.rs` | Task 子代理工具 e2e |
| `tests/export_e2e.rs` | Session export/import e2e |
| `tests/lsp_framing.rs` | LSP JSON-RPC 帧解析测试 |
| `tests/hooks_e2e.rs` | CLI hooks runner e2e |
| `tests/spinner_e2e.rs` | Spinner 组件 e2e |
| `tests/git_tool_e2e.rs` | Git 工具 e2e |
| `tests/dynamic_trigger_e2e.rs` | 动态 trigger 规则 e2e |
| `tests/bug_report_e2e.rs` | Bug report 生成 e2e |

**测试模式特点**：由于 `pie-coding-agent` 是 `bin` crate 而非 `lib`，集成测试通过 `#[path = "../src/xxx.rs"]` 直接引用源码模块，避免了重构 crate 类型的成本。测试创建隔离的 `tempfile::tempdir()` 目录和 faux `AgentHarness` 实例。

---

## 7. risks —— 体验和工程风险

### 7.1 工程风险

| 风险 | 说明 | 影响 |
|------|------|------|
| **bin crate 导致测试引用脆弱** | 集成测试通过 `#[path]` 直接引用 `bin` 源码，任何新增模块依赖都需在测试中复制模块树（如 `tests/tools.rs` 需要引入 `auth.rs`, `config.rs`, `skills_state.rs`, `triggers/mod.rs` 等） | 维护成本高，新模块容易遗漏测试引用 |
| **`!Send` future 限制** | `TurnFut` 因 `parking_lot` guard 跨 `.await` 导致 `!Send`，无法 `tokio::spawn`，必须在 event loop 中轮询 | 限制了并发模型（如多个并行的 subagent），也限制了未来将 turn 执行迁移到独立 task 的可能性 |
| **SkillHarnessCell 双写 guard** | 使用 `OnceCell` + `assert!` 防止重复 set，但如果重构遗漏会导致 panic（目前仅在运行时检测，非编译时） | 隐患可控，但有 `OnceLock` 替代方案可以更安全 |
| **MCP 故障不阻塞启动** | `mcp_loader` 设计为 fail-soft，server 连接失败只产生 diagnostic warning | 意图正确，但可能导致用户误以为 server 已连接 |
| **slash command 输出路由复杂** | `cprintln!` 宏通过 `commands::console::emit_line()` 将输出路由到 feed channel 或 stdout，间接引入了全局状态 | 测试需要 `clear_sink()` 避免泄漏 |
| **Web UI 与 TUI 代码重复** | `App::run()` 和 `App::run_web()` 实现不同但功能对标，axum 的 SSE 端点和 ratatui 的 event loop 是两套独立的 push/poll 模型 | 未来 feature parity 维护成本高 |

### 7.2 体验风险

| 风险 | 说明 | 影响 |
|------|------|------|
| **Ctrl-C 行为不一致** | Raw mode 下 Ctrl-C 是键盘事件（需连按两次退出），非 raw mode 下是 SIGINT（立即退出）。从 TUI 退出到 shell 后 `reset` 可能残留终端状态 | 用户困惑，需要 `Drop` 实现确保终端恢复 |
| **session 恢复时的消息重放** | `app.replay()` 通过 `tui::render_persisted()` 渲染历史消息，但使用的是 crossterm 的命令式 API（而非 feed 结构），输出顺序可能与当前布局不一致 | 影响 resume 体验的视觉一致性 |
| **图片尺寸限制导致大图片截断** | `--image` 参数限制 10MB/张 和 10 张/消息，超过限制会报错但错误信息不够友好 | clipboard_image 粘贴可能静默失败 |
| **slash command 补全依赖 skills 快照** | `SlashCompleter::from_registry_and_skills()` 在 `App::new()` 时构造，如果后续 `/skills reload` 改变了 catalog，补全列表不会动态更新 | 补全可能显示已 removed 的 skill |
| **`always_allow` 和 `yes` 的风险** | 一旦开启，所有 `ControlPlaneWrite` 操作自动批准，包括 skill 安装和文件写入 | 需在 UI 中明确提示已开启安全旁路 |

---

## 8. next_questions —— 下一轮精读问题

1. **AgentSession / AgentHarness 的 session tree 和 branch 机制**：`/undo` 如何实现？compaction 是如何压缩 context 的？tree-based session 与 JSONL 存储之间的映射关系是什么？

2. **Task 子代理的实现细节**：`TaskTool` 如何创建隔离的子 harness？子代理的 tool set 如何限制（read-only）？子代理的上下文是否继承父代理的 skills/memory？

3. **Trigger / Cron 执行模型的完整性**：`DynamicTriggerCheckHook` 的 poll 循环、`CronNotificationHook` 的调度策略、`InjectAndRun` vs `InjectSummary` 的区别、触发后如何注入消息到 agent context？

4. **Compaction 策略**：何时触发？摘要长度控制？thinking block 如何处理？compaction 后的 transcript 如何与原始对话关联？

5. **Web UI 的完整实现**：axum 路由定义、SSE 推流机制、prompt API 与 TUI 输入的一致性保证、Web 端 tool call 确认流程？

6. **LSP Supervisor 的集成深度**：支持哪些语言？diagnostics 如何映射到 edit/write tool result 中？`after_tool_call` hook 的性能影响？

7. **Control Plane 安全模型**：`PermissionCategory` 的完整枚举、`before_tool_call` / `on_control_plane_prompt` 的双层审批机制、skill 安装的两阶段 confirm 模式的安全性分析？

8. **Error handling 和 retry 策略**：`AgentSession` 的 retry 逻辑、409 context window 溢出处理（DS4 特化）、stream 中断后的恢复策略？

9. **`pie-agent-core` 与 `pie-coding-agent` 的接口契约**：哪些类型/trait 属于 stable API？未来 bin/lib 拆分计划？
