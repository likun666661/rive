# Chapter 01 Expanded: Agent Runtime 全景

> 主题：CLI/TUI -> Harness -> AgentLoop -> Provider/Tool -> Session  
> 源码仓库：`/Users/likun/Desktop/workspace-for-pie-agent/pie`  
> 阅读基线：`f1c35a3`  
> 章节定位：用一次普通 coding turn，把 pie 的运行时分层一次性挂起来。

---

## 0. 本章先给一个结论

如果只看产品表面，`pie` 像是一个终端里的 coding agent：用户输入一句话，模型一边流式输出，一边调用 `read` / `edit` / `bash` 这些工具，最后给出回答。

但从源码看，`pie` 更准确的定位是一个 **本地 agent runtime**。

它不是把 prompt 直接丢给模型，而是把一次 turn 拆成了几层：

```text
pie-coding-agent
  负责产品入口：CLI 参数、TUI/Web/headless、slash command、本地工具、MCP、skills、hooks。

AgentHarness
  负责运行时装配：Session、Compaction、CostTracker、TriggerRuntime、权限 hook、目标续跑 hook。

Agent
  负责纯状态机：messages、model、thinking、tools、listener、取消令牌、steering/follow-up queue。

AgentLoop
  负责一次或多次 model/tool 循环：call_llm -> stream events -> execute_tools -> decide next turn。

pie-ai
  负责 provider 边界：把 Anthropic/OpenAI/Gemini/Bedrock 等供应商流统一成 AssistantMessageEvent。

Session JSONL
  负责持久化账本：message、model change、thinking change、compaction、custom audit、leaf。
```

这一章最重要的心智模型是：

> `pie-coding-agent` 是产品壳，`AgentHarness` 是运行时外壳，`Agent` 是 IO-free 状态机，`AgentLoop` 是 model/tool 循环，`pie-ai` 是 provider 适配层，`Session JSONL` 是可恢复账本。

理解这句话，后面再看 Provider、Session、Tool、Trigger、Goal，就不会迷路。

---

## 1. 为什么第一章要从全景开始

一个 coding agent 的难点不在“调用一次模型”。

如果只是调用模型，代码可能只有：

```text
read stdin
send messages to model
print model output
```

但 `pie` 要解决的是更工程化的问题：

1. CLI 要能选择 provider、model、thinking level、base url、session resume、image、debug、Web/TUI 模式。
2. UI 要能在模型流式输出时继续响应键盘、渲染工具进度、显示 trigger 状态、处理 control-plane approval。
3. 模型可能返回 tool calls，runtime 要先做权限判断，再执行工具，再把 tool result 放回上下文。
4. 会话要能退出后恢复，所以消息、模型切换、thinking 切换、compaction、trigger audit 都要落盘。
5. 长会话会超过 context window，所以需要 turn-boundary compaction，而不是无限重放历史。
6. 自动化 trigger 可能在用户没输入时注入任务，所以主 agent、子 agent、session、inbox 之间必须有边界。

所以第一章不能先讲 provider，也不能先讲某一个工具。它要先画全局链路：

```text
用户输入
  -> App/ReplKernel
  -> AgentHarness::prompt
  -> Agent::prompt
  -> run_agent_loop
  -> call_llm(stream_fn)
  -> pie_ai::stream_simple
  -> provider event stream
  -> AssistantMessageEvent
  -> AgentEvent / FeedUpdate
  -> execute_tools
  -> ToolResultMessage
  -> Session JSONL
```

这条链路就是本章的骨架。

---

## 2. 先看 workspace 分层

`pie` 是 Rust 2024 Cargo workspace。当前源码的协作说明把主要 crate 分成四块：

| crate | 包名 | 责任 |
|---|---|---|
| `crates/ai` | `pie-ai` | 统一 LLM streaming client、provider 实现、模型目录、OAuth、SSE/EventStream 工具 |
| `crates/agent` | `pie-agent-core` | agent runtime、Agent 状态机、AgentLoop、Harness、Session、Compaction、TriggerRuntime |
| `crates/coding-agent` | `pie-coding-agent` | `pie` CLI binary、TUI/Web、slash commands、本地工具、config、hooks、LSP、MCP adapter |
| `crates/mcp` | `pie-mcp` | 最小 MCP client：stdio/http transport、JSON-RPC、tools/list、tools/call |

这里有一个非常关键的方向：

```text
coding-agent -> agent -> ai
coding-agent -> mcp
```

也就是说：

- `crates/ai` 不知道 coding agent 是什么。
- `crates/agent` 不应该知道 CLI/TUI 怎么展示。
- `crates/coding-agent` 才是把人类交互、本地工具、MCP、skills、hooks 组装起来的产品层。

这就是为什么 `Agent` 不能直接读文件，不能直接写 JSONL，不能直接打印到终端。那些都是外层的责任。

---

## 3. 一张总图

课堂上建议先画这张图：

```text
┌─────────────────────────────────────────────────────────────────────┐
│                         pie-coding-agent                            │
│                                                                     │
│  main.rs                                                            │
│    - parse CLI args                                                 │
│    - auto detect model                                              │
│    - create/resume session                                          │
│    - load tools / skills / templates / MCP / hooks / LSP             │
│    - build AgentHarnessOptions                                      │
│    - create ui::App                                                 │
│                                                                     │
│  ui/                                                                │
│    - TUI/Web/headless                                               │
│    - FeedUpdate channel                                             │
│    - ReplKernel serialized turn slot                                │
│                                                                     │
│  tools/                                                             │
│    - read/write/edit/bash/grep/find/git/memory/...                  │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                           AgentHarness                              │
│                                                                     │
│  owns/wraps:                                                        │
│    - Agent                                                          │
│    - Session                                                        │
│    - CostTracker                                                    │
│    - Compaction settings                                            │
│    - TriggerRuntime                                                 │
│    - notification hooks                                             │
│    - before/after tool hooks                                        │
│    - on_turn_end goal hook                                          │
│                                                                     │
│  prompt():                                                          │
│    ensure_session_start                                             │
│    check_budget_cap                                                 │
│    run_auto_compaction                                              │
│    run_turn_with_continuation                                       │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                              Agent                                  │
│                                                                     │
│  IO-free state machine:                                             │
│    - AgentState { system_prompt, model, thinking, tools, messages } │
│    - listeners                                                      │
│    - steering queue / follow-up queue                               │
│    - CancellationToken                                              │
│                                                                     │
│  public API:                                                        │
│    prompt(message)                                                  │
│    prompt_many(messages)                                            │
│    continue_()                                                      │
│    abort()                                                          │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                           AgentLoop                                 │
│                                                                     │
│  drive_loop:                                                        │
│    TurnStart                                                        │
│    call_llm                                                         │
│    append assistant message                                         │
│    execute_tools                                                    │
│    append tool results                                              │
│    TurnEnd                                                          │
│    should_stop / prepare_next_turn / queues / stop_reason           │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                              pie-ai                                 │
│                                                                     │
│  stream_simple(model, context, options)                             │
│    -> provider registry                                             │
│    -> provider-specific decoder                                     │
│    -> AssistantMessageEvent stream                                  │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         Session JSONL                               │
│                                                                     │
│  line 1: JsonlSessionMetadata                                       │
│  following lines: SessionTreeEntry                                  │
│    - message                                                        │
│    - model_change                                                   │
│    - thinking_level_change                                          │
│    - compaction                                                     │
│    - custom audit                                                   │
│    - leaf                                                           │
└─────────────────────────────────────────────────────────────────────┘
```

这张图看起来很长，但它解决了一个初学者最大的问题：到底哪个文件负责哪件事。

---

## 4. 第一层：`main.rs` 不是模型入口，而是装配入口

源码入口是：

```text
crates/coding-agent/src/main.rs
```

`main.rs` 顶部的模块声明已经透露了它不是一个小 CLI：

```rust
mod agent_session;
mod auth;
mod commands;
mod config;
mod goal;
mod hooks;
mod inbox;
mod lsp_supervisor;
mod mcp_loader;
mod model;
mod session;
mod skills;
mod tools;
mod triggers;
mod ui;
```

它真正做的是产品装配。

### 4.1 CLI 参数先变成运行配置

`Cli` 结构体由 `clap` derive 出来，包含这些关键参数：

- `--provider`
- `--model`
- `--base-url`
- `--thinking`
- `--resume`
- `--continue`
- `--resume-id`
- `--image`
- `--builtin-skill`
- `--trigger-poll-secs`
- `--debug`
- `--yes`
- `--always-allow`
- `--web`
- `--tui`
- `--web-host`
- `--web-port`

这说明用户启动 `pie` 时，已经不是单纯传一个 prompt，而是在选择一个完整 runtime 的启动状态：

```text
模型是谁？
thinking 怎么开？
要不要恢复旧 session？
图片怎么附加？
工具权限是否自动批准？
用 TUI 还是 Web？
trigger 多久轮询？
```

### 4.2 `main()` 的主路径

`main()` 的主流程很短：

```text
print_dynamic_help_and_exit_if_requested()
Cli::parse()
current_dir()
session::open_repo(&cwd)

如果是 session export/import 子命令:
  run_cli_command()

如果是 list/delete session:
  list/delete and exit

否则:
  run_repl(cli, cwd, repo)
```

真正的 runtime 装配都在 `run_repl()`。

### 4.3 `run_repl()` 是本章第一条主线

`run_repl()` 做了这些事：

1. 决定 UI 模式：Web / Tui / Headless。
2. 加载本地模型定义。
3. 自动检测 model/provider；如果没有 key 且没有显式模型，允许以 credential-less default 启动。
4. 解析 thinking level。
5. 创建或恢复 session。
6. 初始化 logging。
7. 创建 UI feed channel。
8. 构造 stream function。
9. 加载 dynamic trigger / cron sidecar。
10. 构造 default tools、task tool、skill tool、install skill、cron/trigger tools。
11. 加载 MCP，把 MCP tools 合进 tool list。
12. 加载 memory block，组装 system prompt。
13. 加载 skills、templates、builtin skills、skills-state overlay。
14. 构造 `AgentHarnessOptions`。
15. 创建 `AgentHarness`。
16. 设置 skill/goal 的 `OnceCell<Arc<AgentHarness>>`。
17. 注册 MCP / cron / dynamic trigger notification hooks。
18. 如果是 resume，调用 `rehydrate_from_session()`。
19. 创建 `ui::App`。
20. 订阅 AgentEvent / HarnessEvent，把事件转成 FeedUpdate。
21. 启动 `app.run()` 或 `app.run_web()`。

这就是为什么我说 `main.rs` 是装配入口。

它不直接“跑模型”。它负责把运行一次 agent turn 所需的所有零件准备好。

---

## 5. 第二层：UI 不直接写 Agent，而是通过 App / ReplKernel

当前新的 UI 入口在：

```text
crates/coding-agent/src/ui/
```

历史上还有一个较老的：

```text
crates/coding-agent/src/tui.rs
```

`tui.rs` 是较简单的 line-stream renderer，仍然能看到早期设计：把 `AgentEvent` 渲染到 stdout，比如 `TextDelta` 输出 token，`ToolExecutionStart` 打印工具名，`ToolExecutionEnd` 打印工具结果。

当前主路径是 `ui::App`。

### 5.1 `AppConfig` 说明 UI 需要什么

`ui/mod.rs` 里的 `AppConfig` 包含：

```rust
pub struct AppConfig {
    pub harness: Arc<AgentHarness>,
    pub retry: RetrySettings,
    pub registry: Registry,
    pub cwd: PathBuf,
    pub session_id: String,
    pub log_path: Option<PathBuf>,
    pub tool_count: usize,
    pub history: HistoryStore,
    pub pending_images: Vec<PathBuf>,
    pub feed_rx: UnboundedReceiver<FeedUpdate>,
    pub main_run_rx: UnboundedReceiver<String>,
    pub control_plane_prompt_rx: Option<UnboundedReceiver<UiControlPlanePrompt>>,
    pub panel_status: PanelStatus,
}
```

这几个字段分别对应：

- `harness`：真正执行 prompt 的 runtime。
- `registry`：slash command 注册表。
- `feed_rx`：Agent/Harness 事件转出来的展示流。
- `main_run_rx`：inject-and-run trigger 请求主 agent 继续跑一轮。
- `control_plane_prompt_rx`：危险控制面操作需要用户批准时的弹窗输入。
- `pending_images`：启动参数里的 `--image` 附件。

UI 不应该绕过 harness 去改 agent state。

### 5.2 UI 的事件驱动原则

`ui/mod.rs` 顶部注释很清楚：

> Agent/harness events never write to stdout directly — they arrive as `FeedUpdate`s on a channel.

这意味着：

```text
AgentEvent / HarnessEvent
  -> ui::listener
  -> FeedUpdate
  -> feed_rx
  -> App render loop
```

为什么这样设计？

因为全屏 TUI 只有一个终端 writer。如果模型 token、工具进度、slash command 输出、trigger 状态、approval 弹窗都直接 `println!`，终端会乱。

所以 runtime 层只发事件，UI 层统一渲染。

### 5.3 UI 同时处理用户输入和模型 turn

`ui/mod.rs` 的注释指出：

```text
The model turn runs as a local future polled by the event loop's select!,
so the feed streams and the input box stays live while the assistant responds.
```

也就是说，模型 turn 不一定被粗暴 `tokio::spawn` 到一边。UI event loop 会在同一个调度面里处理：

- keyboard/mouse event
- `feed_rx.recv()`
- 正在进行的 turn future
- `main_run_rx.recv()`
- relay prompt
- control-plane prompt

这个设计和 `Agent` 的 `AlreadyStreaming` 防线是配套的：

- `Agent` 保证同一个状态机一次只能 streaming 一个 turn。
- UI/kernel 保证用户输入和 trigger 注入都走同一个 serialized run slot。

---

## 6. 第三层：`AgentHarness` 是运行时外壳

核心文件：

```text
crates/agent/src/harness/agent_harness.rs
```

开头注释直接说它是 opinionated assembly around the bare `Agent`。

它实现的不是“调用模型”本身，而是这些生产级能力：

- `Agent` + `Session` + skills catalog + compaction settings 组合。
- `prompt()` / `prompt_with_images()` / `continue_()`。
- 自动 compaction。
- `set_model()` / `set_thinking_level()` 并写入 session log。
- `fork()` / `move_to()` 分支操作。
- `prompt_from_template()`。
- runtime tool/skill 替换。
- steering / follow-up queue。
- lifecycle event subscription。
- trigger handling。
- cost tracker。
- turn-end goal continuation。

### 6.1 Harness 为什么不是普通 helper

如果 `Agent` 自己读写 session，那么它就不再是一个可测试的状态机。

如果 `main.rs` 自己到处处理 session、cost、compaction，那么产品入口会变成一个巨型脚本。

所以 pie 把边界拆成：

```text
Agent:
  管内存状态和事件，不碰文件系统。

AgentHarness:
  把 Agent 和 Session/Cost/Compaction/Trigger 接起来。

coding-agent:
  把 Harness 和 CLI/TUI/tools/config/MCP 接起来。
```

这就是 `AgentHarness` 的价值。

### 6.2 `AgentHarness::new()`

`AgentHarness::new(options)` 里先构造 `AgentState`：

```text
state.model = options.model
state.thinking_level = options.thinking_level
state.tools = options.tools
state.system_prompt = build_system_prompt(options.system_prompt, options.skills)
```

然后创建 `Agent`：

```text
Agent::new(AgentOptions {
  initial_state,
  stream_fn,
  before_tool_call,
  after_tool_call,
  on_control_plane_prompt,
  ...
})
```

再创建 `CostTracker`，并订阅内部 `Agent` 的事件：

```text
agent.subscribe(cost.as_listener())
```

所以成本统计不是主循环里到处加计数，而是通过事件监听实现。

### 6.3 `AgentHarness::prompt()`

`prompt(text)` 会把文本包装成 `AgentMessage::Llm(PiMessage::User(...))`，再进入 `prompt_with_message()`。

`prompt_with_message()` 的顺序很重要：

```text
ensure_session_start_emitted()
check_budget_cap()
run_auto_compaction()
run_turn_with_continuation(Some(user_message), last_user_prompt)
```

注意 compaction 在 user message append 之前运行。源码注释解释了原因：避免切点 split 当前 turn。

也就是说，普通用户输入不是直接进 `Agent::prompt()`，而是先经过 Harness 的预算、压缩、续跑、持久化包装。

### 6.4 Session listener 是持久化关键

`make_session_listener(session)` 会创建一个 `AgentListener`。

它监听：

- `AgentEvent::MessageEnd`：调用 `session.append_message(message)`。
- `AgentEvent::ControlPlanePromptResolved`：写入 `custom_type = "control_plane_prompt"` 的 audit entry。

所以 session 持久化是事件驱动的：

```text
AgentLoop emits MessageEnd
  -> session listener
  -> Session::append_message
  -> SessionStorage::append_entry
  -> JSONL append
```

这里有一个容易误解的点：

> `Agent.state.messages` 里的消息不是自动等于 JSONL 文件里的 entry；它们通过 listener 在 `MessageEnd` 边界同步。

这也是为什么 `prompt_reports_session_persistence_failures` 测试会专门验证：如果 append session 失败，`harness.prompt("hi")` 会返回包含 `session append message` 和底层错误的失败。

---

## 7. 第四层：`Agent` 是 IO-free 状态机

核心文件：

```text
crates/agent/src/agent.rs
```

文件注释说得非常直接：

```text
The bare Agent owns conversation state and exposes prompt / continue / subscribe / abort.
It never reaches into the filesystem — IO belongs to the harness or the caller.
```

### 7.1 `AgentInner` 持有什么

`AgentInner` 包含：

```rust
pub(crate) struct AgentInner {
    pub state: Mutex<AgentState>,
    pub listeners: Mutex<Vec<AgentListener>>,
    pub steering: Mutex<PendingMessageQueue>,
    pub follow_up: Mutex<PendingMessageQueue>,
    pub options: AgentOptions,
    pub active_cancel: Mutex<Option<CancellationToken>>,
    pub idle: Notify,
}
```

这些字段分别表示：

- `state`：当前上下文、模型、工具、streaming 状态。
- `listeners`：事件订阅者，UI、session、cost、hooks 都靠它。
- `steering`：运行中优先插入的指导消息。
- `follow_up`：当前 turn 结束后才插入的后续消息。
- `options`：stream_fn、tool hooks、context transform 等。
- `active_cancel`：当前 run 的取消令牌。
- `idle`：等待当前 run 结束的通知。

### 7.2 `Agent` 的 public API 很小

核心 API：

```rust
pub async fn prompt(&self, message: AgentMessage) -> Result<(), AgentRunError>
pub async fn prompt_many(&self, messages: Vec<AgentMessage>) -> Result<(), AgentRunError>
pub async fn continue_(&self) -> Result<(), AgentRunError>
pub fn abort(&self)
pub fn subscribe(&self, listener: AgentListener) -> impl FnOnce()
```

这里体现了一个设计克制：

- `Agent` 不知道 CLI 参数。
- `Agent` 不知道 JSONL。
- `Agent` 不知道 TUI。
- `Agent` 不知道文件工具具体怎么写。
- `Agent` 只知道自己有一批 `AgentTool` trait object。

### 7.3 `guard_not_streaming()`

`prompt_many()` 和 `continue_()` 都会先调用 `guard_not_streaming()`。

如果当前 `AgentState.is_streaming = true`，返回 `AgentRunError::AlreadyStreaming`。

这条防线很重要。因为 `AgentState.messages` 是一个线性 transcript，如果两个 prompt 同时写进去，会出现：

```text
user A
user B
assistant A partial
tool result B
assistant B final
```

这种交错会直接破坏上下文。pie 的做法是：同一个 Agent 一次只能跑一个 loop；外层 UI/kernel 和 trigger channel 负责序列化输入。

---

## 8. 第五层：`AgentLoop` 是最关键源码

核心文件：

```text
crates/agent/src/agent_loop.rs
```

开头注释说它实现了：

- 从 `pie-ai` stream。
- 累积 events 到最终 assistant message。
- 工具执行。
- 4 个 lifecycle hooks：`transform_context`、`before_tool_call`、`after_tool_call`、`should_stop_after_turn`、`prepare_next_turn`。
- steering/follow-up queue。
- tool result 早停。

### 8.1 `run_agent_loop()`

用户输入进入后，`run_agent_loop()` 做这些事：

```text
create CancellationToken
state.is_streaming = true
state.error_message = None
active_cancel = token
emit AgentStart

for each new user message:
  state.messages.push(msg)
  emit MessageStart
  emit MessageEnd

drive_loop()
finalize()
```

这解释了为什么 session listener 能持久化用户消息：

- user message 被 push 进 state。
- 立刻 emit `MessageEnd`。
- session listener append JSONL。

### 8.2 `drive_loop()` 的主循环

`drive_loop()` 是本章最值得投屏看的函数。

它的伪代码是：

```text
loop:
  if cancelled:
    return Ok

  emit TurnStart

  assistant = call_llm()
  state.messages.push(assistant)
  emit MessageEnd(assistant)

  tool_results, all_terminate = execute_tools(assistant)
  for each tool_result:
    state.messages.push(tool_result)
    emit MessageStart(tool_result)
    emit MessageEnd(tool_result)

  emit TurnEnd(assistant, tool_results)

  if should_stop_after_turn hook returns true:
    return Ok

  continues = assistant.stop_reason == ToolUse

  if tool_results not empty and all_terminate:
    return Ok

  if prepare_next_turn hook returns update:
    apply update

  queued = drain steering queue
  if not continues and queued empty:
    queued = drain follow_up queue

  if queued not empty:
    append queued messages
    continue

  if not continues:
    return Ok
```

这个循环说明了一个 coding agent 的本质：

> 模型不是“回复一次”就结束；它可能因为 tool use 继续进入下一轮 model call。

一次用户输入可能展开成：

```text
User: fix tests
Assistant: tool_call(read)
ToolResult: file content
Assistant: tool_call(edit)
ToolResult: edit ok
Assistant: tool_call(bash)
ToolResult: tests pass
Assistant: final answer
```

在 `AgentState.messages` 里，这是一条连续消息链。

### 8.3 `call_llm()`

`call_llm()` 做了 provider 调用前的准备：

1. 从 state 快照出：
   - `system_prompt`
   - `messages`
   - `tools`
   - `model`
2. 可选运行 `transform_context`。
3. 把 `AgentMessage` 转成 `pie_ai::Message`。
4. 把 `AgentTool` 的 definition 转成 `pie_ai::Tool`。
5. 构造 `pie_ai::Context`。
6. 取 `stream_fn`，默认是 `default_stream_fn`。
7. 构造 `SimpleStreamOptions`，写入：
   - `session_id`
   - `abort` token
   - `reasoning/thinking`
8. 调用 stream function。
9. 消费 `AssistantMessageEvent`。

消费 stream 时，源码使用：

```rust
tokio::select! {
    biased;
    _ = cancel.cancelled() => ...
    next = stream.next() => ...
}
```

`biased` 的意义是：取消优先于下一条 provider event。这样 Ctrl-C 或 abort 来了，不需要等 provider 再吐一个 chunk。

### 8.4 `AssistantMessageEvent` 如何变成 UI 和状态

`call_llm()` 根据事件类型做不同处理：

- `Start { partial }`
  - 设置 `last_message`
  - emit `MessageStart`
  - 写入 `state.streaming_message`
- `TextDelta` / `ThinkingDelta` / `ToolCallDelta` 等增量事件
  - 更新 `last_message`
  - 更新 `state.streaming_message`
  - emit `MessageUpdate`
- `Done { message }`
  - 更新最终 `last_message`
- `Error`
  - 清空 streaming message
  - 返回错误

这解释了三种状态的区别：

| 状态 | 用途 | 是否持久化 |
|---|---|---|
| `state.streaming_message` | 当前正在流式生成的临时 assistant message | 不直接持久化 |
| `state.messages` | loop 内的真实上下文 | loop 运行中持续更新 |
| Session JSONL | 可恢复账本 | 在 `MessageEnd` 边界持久化 |

UI 看到的 token 来自 `MessageUpdate`，但 JSONL 持久化的是 `MessageEnd` 后的完整 message。

### 8.5 `execute_tools()`

模型返回 `ContentBlock::ToolCall` 时，`execute_tools()` 会处理。

大流程：

```text
collect tool calls from assistant.content
snapshot registered tools
choose Sequential or Parallel execution mode

for each tool call:
  find matched AgentTool
  prepare_arguments(raw_args)
  emit ToolExecutionStart
  run per-tool permission_classification
  maybe Block
  maybe synthesize control-plane Prompt
  run before_tool_call hook
  merge classifier prompt and hook prompt
  maybe call on_control_plane_prompt
  push PreparedCall::Run or PreparedCall::Blocked

execute prepared calls sequentially or parallel

for each outcome:
  run after_tool_call hook
  emit ToolExecutionEnd
  build ToolResultMessage
```

这说明工具执行不是 provider 的附属品，而是 runtime 的安全边界。

Provider 层只负责说“模型请求了 tool call”。真正是否允许、怎么执行、怎么审计，是 `AgentLoop` 和 Harness/CLI hooks 的事。

### 8.6 权限合并语义很硬

当前代码里有两层权限：

1. 每个 tool 自己的 `permission_classification()`。
2. `before_tool_call` hook，例如 coding-agent 中的 `PermissionPolicy::default_for_coding_agent()`。

合并规则有几个关键点：

- `Block` 会直接短路，不执行 hook，不运行工具。
- `Prompt` 不会被 hook 返回 default 静默清掉。
- hook 可以 block，也可以提供更丰富的 prompt payload。
- 运行时会重新绑定 `tool_call_id`、`tool_name`、`args_hash`，避免 hook 伪造 approval 绑定字段。
- 如果需要 control-plane prompt 但没有 prompt channel，fail-closed deny。

这就是一个 coding agent 对本地工具应该有的态度：模型可以提出请求，但 runtime 决定能不能做。

### 8.7 工具进度如何流回 UI

`run_one()` 执行 tool 时，会给 tool 一个 `on_update` callback。

内部用一个 unbounded mpsc channel + pump task，把 tool 的 partial update 转成：

```text
AgentEvent::ToolExecutionUpdate
```

工具返回后，会等待 pump 正常结束；如果工具错误地把 callback 留到后台任务里，runtime 用 2 秒 timeout 防止 `run_one()` 卡死。

这类细节说明 pie 不是 demo agent。它在处理工具作者“不守约”的情况。

---

## 9. 第六层：`pie-ai` 是 provider 边界

核心文件：

```text
crates/ai/src/stream.rs
```

第一章只需要看入口，不需要深挖每个 provider。

`stream_simple()` 的逻辑很短：

```text
resolve(model)
  -> providers::register_builtins::ensure()
  -> get_api_provider(&model.api)

handle.stream_simple(model, context, options)
```

如果 provider 不存在，就返回 error stream。

这就是 provider 适配层的边界：

- `AgentLoop` 不知道 Anthropic/OpenAI/Gemini 的 SSE 细节。
- `AgentLoop` 只消费 `AssistantMessageEvent`。
- `pie-ai` 负责把 provider 原始协议统一成这个事件流。

这也是 Chapter 02 要讲的内容：不同 provider 的 streaming tool-call delta 如何归一化。

---

## 10. 第七层：Session JSONL 是持久化账本

核心文件：

```text
crates/agent/src/harness/session/session.rs
crates/agent/src/harness/session/jsonl_storage.rs
```

### 10.1 JSONL 文件格式

`JsonlSessionStorage` 注释说明：

```text
line 1 is JsonlSessionMetadata header
subsequent lines are SessionTreeEntry rows
no in-place edits
leaf pointer is derived from latest leaf entry or last appended row
```

所以 session 文件不是一个会被原地修改的大 JSON。

它是 append-only：

```text
{"id":"...","createdAt":"...","cwd":"...","path":"..."}
{"type":"message","id":"...","parentId":null,...}
{"type":"message","id":"...","parentId":"...",...}
{"type":"model_change","id":"...","parentId":"...",...}
{"type":"compaction","id":"...","parentId":"...",...}
{"type":"leaf","id":"...","parentId":"...","targetId":"..."}
```

append-only 的好处是：

- 容易恢复。
- 容易审计。
- 分支可以用 parent_id 表达。
- move leaf 不需要重写历史，只要追加一个 `Leaf` entry。

### 10.2 `SessionTreeEntry`

当前 entry 类型包括：

- `Message`
- `ThinkingLevelChange`
- `ModelChange`
- `Compaction`
- `BranchSummary`
- `Custom`
- `CustomMessage`
- `Label`
- `SessionInfo`
- `Leaf`

第一章重点关注前三类：

```text
Message:
  user / assistant / tool result 等 LLM-visible 或 custom message。

ModelChange:
  /model 或 runtime 切换模型后，resume 时恢复。

ThinkingLevelChange:
  /thinking 或启动参数变化后，resume 时恢复。

Compaction:
  长会话压缩后，resume 时用 summary 替代旧历史。

Leaf:
  当前分支指针。
```

### 10.3 `build_session_context()`

恢复 session 的关键是：

```rust
pub fn build_session_context(path_entries: &[SessionTreeEntry]) -> SessionContext
```

它会重放 branch path：

- 遇到 `ThinkingLevelChange`，更新 thinking。
- 遇到 `ModelChange`，更新 model。
- 遇到 assistant message，也能从 assistant 上恢复 model/provider。
- 遇到 `Compaction`，记录最后一次 compaction index。
- 最后构造可喂给 Agent 的 `messages`。

如果存在 compaction，它会：

1. 先放入一个 `compaction_summary` custom message。
2. 从 `first_kept_entry_id` 开始保留旧消息。
3. 再追加 compaction 之后的消息。

这就是“session JSONL 不是直接等于模型上下文”的另一个例子。

JSONL 是账本，`build_context()` 才把账本重放成当前模型上下文。

---

## 11. 一次普通 coding turn 的完整走读

现在把前面所有层串起来。

假设用户在 TUI 里输入：

```text
fix failing tests
```

### 11.1 UI 收到输入

`ui::App` 在 event loop 中拿到输入，交给 `ReplKernel`。如果不是 slash command，最终会走到类似：

```text
harness.prompt("fix failing tests")
```

如果用户当前还有 `--image` 或粘贴图片，则会走 `prompt_with_images()`。

### 11.2 Harness 做运行前检查

`AgentHarness::prompt()` 把文本包装成 user message。

然后：

```text
ensure_session_start_emitted()
check_budget_cap()
run_auto_compaction()
run_turn_with_continuation(Some(user_msg), ...)
```

这里可能发生：

- 如果预算超了，直接拒绝。
- 如果上下文接近窗口阈值，先自动 compaction。
- 如果后面 goal evaluator 要继续，会由 `run_turn_with_continuation` 管。

### 11.3 Harness 临时挂 session listener

执行 Agent turn 时，Harness 会挂上 session listener。

这样 AgentLoop 中每次 `MessageEnd` 都会落盘：

```text
user message -> session message entry
assistant final message -> session message entry
tool result -> session message entry
```

### 11.4 Agent 开始 run

`Agent::prompt()` 调用 `guard_not_streaming()`。

如果当前没有其他 run：

```text
run_agent_loop(inner, vec![user_message])
```

### 11.5 AgentLoop 追加用户消息

`run_agent_loop()`：

```text
state.is_streaming = true
emit AgentStart
state.messages.push(user_message)
emit MessageStart(user_message)
emit MessageEnd(user_message)
```

UI 可以显示用户消息，session listener 可以 append user message。

### 11.6 AgentLoop 调模型

`drive_loop()` 发射 `TurnStart`，进入 `call_llm()`。

`call_llm()` 拿到当前上下文：

```text
system_prompt
messages
tools
model
thinking_level
```

构造 `pie_ai::Context`：

```text
Context {
  system_prompt,
  messages,
  tools
}
```

然后调用：

```text
stream_fn(&model, &context, Some(&options))
```

默认情况下，这个 stream_fn 最终会进入 `pie_ai::stream_simple()`。

### 11.7 provider 返回 streaming events

`pie-ai` provider 返回 `AssistantMessageEvent`：

```text
Start(partial)
TextDelta(partial, delta)
ToolCallDelta(partial, ...)
ToolCallEnd(partial, ...)
Done(message)
```

AgentLoop 每拿到一个增量事件，就 emit：

```text
AgentEvent::MessageUpdate
```

UI listener 把它转成 `FeedUpdate`，TUI/Web 渲染。

### 11.8 如果模型请求工具

最终 assistant message 可能包含：

```text
ContentBlock::ToolCall {
  id: "call_1",
  name: "read",
  arguments: { "path": "..." }
}
```

`execute_tools()` 开始处理。

顺序是：

```text
emit ToolExecutionStart
tool.permission_classification()
before_tool_call hook
maybe on_control_plane_prompt
tool.execute()
after_tool_call hook
emit ToolExecutionEnd
build ToolResultMessage
```

ToolResultMessage 会被 push 到 `state.messages`，也会通过 `MessageEnd` 持久化到 session。

### 11.9 如果 stop_reason 是 ToolUse

如果 assistant 的 `stop_reason == ToolUse`，且工具没有要求全部 terminate，那么 `drive_loop()` 会继续下一轮。

也就是：

```text
tool result 已经进入 messages
下一次 call_llm() 会把 tool result 一起发给模型
模型根据工具结果继续思考或给 final answer
```

### 11.10 最终回答结束

当 assistant 最终 `stop_reason != ToolUse`，也没有 steering/follow-up queue，`drive_loop()` 返回。

`finalize()`：

```text
emit AgentEnd
state.is_streaming = false
active_cancel = None
idle.notify_waiters()
```

Harness 再处理：

- session listener 错误检查。
- OnTurnEndHook / goal evaluator。
- continuation cap。
- TurnEnded harness event。

一次普通 coding turn 才真正结束。

---

## 12. 三种状态一定要分清

本章最容易混淆的是“消息到底在哪里”。

### 12.1 模型上下文里的 messages

位置：

```text
AgentState.messages
```

作用：

- 下一次 `call_llm()` 会把它转成 `pie_ai::Message`。
- tool result 也会放在这里。
- steering/follow-up message 也会进入这里。

生命周期：

- 当前 Agent 实例内存中。
- resume 时由 session 重放而来。

### 12.2 Session JSONL 里的 entries

位置：

```text
~/.pie/sessions/<cwd-hash>/<uuid>.jsonl
```

作用：

- 持久化账本。
- 支持 resume。
- 支持 branch / move_to。
- 支持 compaction、custom audit、model/thinking 变更。

生命周期：

- 跨进程、跨会话保存。

注意：

- JSONL entries 不一定都进入模型上下文，例如 audit custom entry。
- compaction 会改变重放后的上下文形状。

### 12.3 TUI feed 里的展示事件

位置：

```text
ui::Feed
FeedUpdate channel
```

作用：

- 给用户展示 streaming token、工具进度、trigger 状态、系统提示。

生命周期：

- UI 当前运行期的展示状态。

注意：

- FeedUpdate 不等于 session entry。
- Tool progress update 可以显示在 UI，但不一定作为独立 message entry 持久化。

---

## 13. 为什么 `Agent` 必须 IO-free

这是本章最值得强调的设计点。

`Agent` 保持 IO-free，带来几个直接好处：

### 13.1 单元测试容易写

`crates/agent/tests/agent_loop.rs` 用 synthetic `StreamFn` 测试 AgentLoop，不需要真实 API key。

例如：

- `single_turn_no_tools_emits_lifecycle_events`
- `tool_call_loops_until_non_tool_use_stop`
- `before_tool_call_can_veto_execution`

这些测试只需要：

```text
faux model
faux stream function
faux tools
Agent::new(AgentOptions { ... })
agent.prompt(user)
```

如果 `Agent` 内部直接读写文件、直接访问 provider auth store、直接打印 UI，就很难这样测。

### 13.2 Session backend 可替换

`crates/agent/tests/session_storage.rs` 同时测试 memory 和 JSONL。

Harness 测试里经常用：

```text
MemorySessionStorage
Session::new(storage)
AgentHarness::new(opts)
```

生产环境则用：

```text
JsonlSessionRepo
JsonlSessionStorage
```

这说明 session 是 trait boundary，不是硬编码文件。

### 13.3 UI 可替换

当前有：

- full-screen TUI
- Web UI
- headless
- 老的 line-stream TUI renderer

这些 UI 都可以订阅同一种 Agent/Harness event，而不用改 AgentLoop。

### 13.4 自动化更容易隔离

Trigger 子代理可以创建独立 `MemorySessionStorage` 和独立 `Agent`，不污染父 agent 的 transcript。

如果 Agent 自己强绑定 cwd/session 文件，子代理隔离会很麻烦。

---

## 14. 当前代码和大纲里需要小心的边界

### 14.1 大纲写了 `crates/coding-agent/src/tui.rs`

当前源码中 `tui.rs` 还存在，但主路径已经是 `crates/coding-agent/src/ui/`。

教学时可以这样处理：

- 用 `ui/mod.rs` 讲当前 full-screen TUI/Web/headless 架构。
- 用 `tui.rs` 作为更简单的 AgentEvent renderer 示例。

不要让听众误以为 `tui.rs` 是唯一 UI 主入口。

### 14.2 `AgentHarness` 不是只有 session/compaction

大纲为了第一章压缩了内容，但当前 `AgentHarness` 很大，包含 trigger、promotion、goal continuation、skills reload 等。

第一章不要展开所有 trigger 状态机。只需要说：

```text
Harness 是所有跨 turn 能力的装配层；
本章只关注 prompt/session/cost/compaction；
trigger 和 goal 后面章节再展开。
```

### 14.3 `pie-ai` 只在本章看入口

本章只讲：

```text
AgentLoop -> stream_fn -> pie_ai::stream_simple -> provider registry -> AssistantMessageEvent
```

不要在第一章提前深挖 Anthropic/OpenAI/Gemini 的 tool-call streaming 差异，那是 Chapter 02 的重点。

---

## 15. 源码走读顺序

建议按这个顺序带读：

### 15.1 `crates/coding-agent/src/main.rs`

重点看：

- `Cli`
- `main()`
- `run_repl()`
- `AgentHarnessOptions::new(...)`
- `tools::default_tools(...)`
- `mcp_loader::load_all(...)`
- `ui::App::new(...)`
- `harness.agent().subscribe(...)`
- `harness.subscribe_harness(...)`
- `app.run()` / `app.run_web()`

课堂问题：

> 为什么 `skill_harness_cell` 要用 OnceCell？

答案：skill tool 构造时要进工具列表，而工具列表又要传给 Harness；但 skill tool 执行时需要访问 live Harness 的 skills snapshot。所以先构造 cell，创建 Harness 后再 set。

### 15.2 `crates/coding-agent/src/ui/mod.rs`

重点看：

- `AppConfig`
- `PanelStatus`
- `App::new`
- `feed_rx`
- `main_run_rx`
- `control_plane_prompt_rx`
- 注释中关于 serialized run slot 的说明。

课堂问题：

> 为什么 AgentEvent 不直接写 stdout？

答案：TUI/Web/headless 都需要统一消费事件；全屏 TUI 只能有一个 terminal writer；直接输出会和 ratatui 渲染冲突。

### 15.3 `crates/agent/src/harness/agent_harness.rs`

重点看：

- `AgentHarness::new`
- `prompt`
- `prompt_with_message`
- `continue_`
- `make_session_listener`
- `CostTracker` listener
- `HarnessEvent`

课堂问题：

> 为什么 compaction 在 user message append 前运行？

答案：避免切点把当前 turn 拆开。当前 user message 还没 append 时，compaction 只处理已有历史。

### 15.4 `crates/agent/src/agent.rs`

重点看：

- 文件顶部 IO-free 注释。
- `AgentInner`
- `AgentOptions`
- `prompt_many`
- `continue_`
- `guard_not_streaming`
- `abort`

课堂问题：

> `Agent` 为什么要有 steering queue 和 follow-up queue？

答案：运行中或 turn 边界可能有外部消息要注入，但不能直接并发调用 `prompt()`。queue 让注入进入同一个 loop。

### 15.5 `crates/agent/src/agent_loop.rs`

重点看：

- `run_agent_loop`
- `drive_loop`
- `call_llm`
- `execute_tools`
- `run_one`
- `emit`
- `finalize`

课堂问题：

> 模型返回 tool call 后，为什么还要再 call LLM？

答案：tool call 只是模型请求行动。工具结果回来后，模型需要看到结果，才能决定下一步或生成 final answer。

### 15.6 `crates/ai/src/stream.rs`

重点看：

- `stream_simple`
- `resolve`
- `providers::register_builtins::ensure()`
- `get_api_provider(&model.api)`

课堂问题：

> AgentLoop 为什么不直接调用 Anthropic/OpenAI？

答案：AgentLoop 只处理统一事件模型；provider 差异必须封装在 `pie-ai`。

### 15.7 `crates/agent/src/harness/session/`

重点看：

- `SessionTreeEntry`
- `SessionStorage`
- `Session::append_message`
- `Session::build_context`
- `JsonlSessionStorage::create/open/append_entry/current_leaf`

课堂问题：

> 为什么 JSONL 是 append-only？

答案：恢复、审计、分支、leaf move 都更清晰；不需要原地修改历史文件。

---

## 16. 课堂演示脚本

### 16.1 演示 1：只画 ordinary coding turn

不要一开始讲 trigger、cron、goal。

只画：

```text
User input
  -> App
  -> Harness
  -> Agent
  -> AgentLoop
  -> pie-ai
  -> Tool
  -> Session
```

讲 4 分钟即可。

### 16.2 演示 2：投屏 `agent_loop.rs`

先打开 `drive_loop()`。

让听众找三个位置：

1. `call_llm`
2. `execute_tools`
3. `if !continues { return Ok(()) }`

这三个位置就是 agent 行为的骨架。

### 16.3 演示 3：投屏 `AgentHarness::prompt`

让听众看顺序：

```text
ensure_session_start_emitted
check_budget_cap
run_auto_compaction
run_turn_with_continuation
```

这能说明 Harness 不是可有可无的 helper。

### 16.4 演示 4：投屏 session entry 类型

打开 `SessionTreeEntry` enum。

只讲：

- Message
- ModelChange
- ThinkingLevelChange
- Compaction
- Custom
- Leaf

告诉听众：

> JSONL 是账本，模型上下文是从账本重放出来的结果。

### 16.5 演示 5：跑测试而不是跑真实模型

推荐跑：

```sh
cargo test -p pie-agent-core single_turn_no_tools_emits_lifecycle_events
cargo test -p pie-agent-core tool_call_loops_until_non_tool_use_stop
cargo test -p pie-agent-core prompt_persists_user_and_assistant_to_session
cargo test -p pie-agent-core jsonl_session_persists_across_open
```

这些测试不需要真实 provider key，能证明第一章核心链路。

---

## 17. 常见误解

### 17.1 “`pie-coding-agent` 就是 agent”

不准确。

`pie-coding-agent` 是产品壳和工具壳。它负责 CLI/TUI/tools/config/MCP。

核心状态机在：

```text
crates/agent/src/agent.rs
```

核心 model/tool loop 在：

```text
crates/agent/src/agent_loop.rs
```

### 17.2 “AgentHarness 只是包装一下 Agent”

不准确。

Harness 是生产 runtime 的装配层：

- session
- compaction
- cost
- trigger
- skills
- templates
- budget
- goal continuation
- permission hooks

没有 Harness，裸 Agent 可以测试 loop，但不像完整产品。

### 17.3 “TUI feed 就是 session”

不对。

TUI feed 是展示状态。Session JSONL 是持久化账本。

一个 tool progress update 可以显示在 TUI，但不一定是独立 session message。

### 17.4 “模型调用和工具执行都属于 provider 层”

不对。

Provider 层只产生统一事件流。工具执行属于 AgentLoop/runtime，且是权限边界。

### 17.5 “resume 就是把 JSONL 全部 messages 原样塞回模型”

不对。

`Session::build_context()` 会重放 branch path，处理 model/thinking/compaction/custom message。compaction 会用 summary 替代旧历史的一部分。

### 17.6 “自动化就是 cron”

不对。

Cron 只是 trigger source 之一。真正统一的 runtime 是：

```text
Trigger envelope
  -> TriggerRuntime dedup/cycle
  -> permission/action hook
  -> sub-agent or inject path
  -> audit/promotion/inbox
```

第一章只埋伏笔，Chapter 05/07 再展开。

---

## 18. 练习题

### Q1：用户输入 `"fix failing tests"` 后，画出从 `main.rs` 到 JSONL append 的路径。

参考答案：

```text
main.rs::run_repl
  -> ui::App
  -> ReplKernel
  -> AgentHarness::prompt
  -> AgentHarness::prompt_with_message
  -> run_turn_with_continuation
  -> Agent::prompt
  -> run_agent_loop
  -> emit MessageEnd(user)
  -> make_session_listener
  -> Session::append_message
  -> JsonlSessionStorage::append_entry
```

assistant message 和 tool result 也通过 `MessageEnd` 走同样持久化路径。

### Q2：`Agent` 为什么不直接读写文件？

参考答案：

因为 `Agent` 是 IO-free 状态机。这样：

- AgentLoop 可用 synthetic stream function 测试。
- SessionStorage 可替换成 memory/jsonl。
- UI 可替换成 TUI/Web/headless。
- 子代理可以用独立 memory session 隔离。

文件读写应该在工具层或 Harness/Session 层。

### Q3：如果模型返回两个 tool calls，`agent_loop.rs` 会怎么处理？

参考答案：

`execute_tools()` 会按 assistant content 顺序收集 tool calls。然后：

- 根据全局 `ToolExecutionMode` 和 per-tool execution mode 决定 sequential 或 parallel。
- 每个 call 先权限分类、before hook、control-plane prompt。
- 再执行工具。
- 执行后跑 after hook。
- 最后生成两个 `ToolResultMessage`，按 outcome 顺序追加到 messages。

### Q4：TUI 显示一条 tool progress，是否意味着 JSONL 一定有对应 entry？

参考答案：

不一定。

tool progress 通过 `AgentEvent::ToolExecutionUpdate` 进入 `FeedUpdate`，它是展示事件。Session listener 主要持久化 `MessageEnd` 和部分 audit 事件。最终 tool result 会作为 `ToolResultMessage` 持久化，但中间 progress 不一定是独立 JSONL entry。

### Q5：如果要新增一个 headless batch mode，应改 `crates/agent` 还是 `crates/coding-agent`？

参考答案：

优先改 `crates/coding-agent`。因为 headless batch mode 是产品入口/UI 调度问题，不是 Agent 状态机问题。只有当需要新的 agent loop hook 或 runtime 能力时，才考虑改 `crates/agent`。

### Q6：为什么 `call_llm()` 里取消使用 `biased select!`？

参考答案：

为了让取消优先于 provider stream 的下一条事件。否则 provider 卡住时，用户 Ctrl-C 可能要等下一条 chunk 才生效。

### Q7：为什么 session 使用 append-only JSONL？

参考答案：

append-only 更适合审计和恢复。parent_id 能表达分支，Leaf entry 能移动当前指针，Compaction/Custom audit 都能保留历史，不需要原地修改大 JSON。

---

## 19. 自测清单

学完本章后，应该能回答：

1. `pie-coding-agent`、`AgentHarness`、`Agent`、`AgentLoop`、`pie-ai` 分别负责什么？
2. 为什么 `Agent` 要保持 IO-free？
3. 一次 tool call 为什么会导致下一次 model call？
4. `MessageUpdate` 和 `MessageEnd` 的差别是什么？
5. TUI feed、AgentState.messages、Session JSONL 三者有什么区别？
6. `AgentHarness::prompt()` 为什么要先检查 budget 和 compaction？
7. `SessionTreeEntry::Compaction` 在 resume 时如何影响上下文？
8. 工具权限为什么在 AgentLoop 层，而不是 provider 层？
9. `stream_simple()` 为什么先 resolve provider registry？
10. 新增一个 UI surface 应该接哪层事件？

---

## 20. 本章源码索引

| 文件 | 本章用途 |
|---|---|
| `crates/coding-agent/src/main.rs` | CLI 参数、模型/session 选择、tools/skills/MCP/hooks 装配、App 启动 |
| `crates/coding-agent/src/ui/mod.rs` | 当前 TUI/Web/headless App、feed channel、serialized run slot |
| `crates/coding-agent/src/tui.rs` | 简化版 AgentEvent renderer，可作为事件渲染示例 |
| `crates/coding-agent/src/tools/mod.rs` | 默认工具集合入口 |
| `crates/agent/src/harness/agent_harness.rs` | Harness 装配、prompt/continue、session listener、cost、compaction、trigger |
| `crates/agent/src/agent.rs` | IO-free Agent 状态机、prompt/continue/abort/listener |
| `crates/agent/src/agent_loop.rs` | model/tool 主循环、stream event 消费、工具执行、hook/queue |
| `crates/ai/src/stream.rs` | provider registry streaming 入口 |
| `crates/agent/src/harness/session/session.rs` | SessionTreeEntry、Session facade、build_context |
| `crates/agent/src/harness/session/jsonl_storage.rs` | append-only JSONL backend |
| `crates/agent/tests/agent_loop.rs` | AgentLoop 行为测试 |
| `crates/agent/tests/harness_e2e.rs` | Harness + Session 持久化测试 |
| `crates/agent/tests/session_storage.rs` | memory/jsonl session backend 测试 |
| `crates/coding-agent/tests/cli_session.rs` | CLI/session integration 和 rehydrate 测试 |

---

## 21. 本章一句话收束

`pie` 的第一性原理不是“终端里包了一层 LLM”，而是：

> 用 `Agent` 保存纯运行状态，用 `AgentLoop` 驱动 model/tool 循环，用 `AgentHarness` 接上 session/compaction/cost/trigger，用 `pie-coding-agent` 把它包装成可交互、可恢复、可自动化的本地 coding agent 产品。

后面的章节都是在这条主链路上局部放大：

- Chapter 02 放大 provider stream。
- Chapter 03 放大 Agent/Harness/Session/Compaction。
- Chapter 04 放大 Tools/Permission/LSP。
- Chapter 05 放大 Trigger/Cron/Loop/Inbox。
- Chapter 06 放大 MCP/Web Relay。
- Chapter 07 放大 Goal/OnTurnEndHook。
- Chapter 08 把风险和测试路线收束。
