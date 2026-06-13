# 粗读报告：Loops / Triggers / Inbox / Hooks

> 阅读基线：`f1c35a3` | 深度档位：`architecture` | 仅输出报告，不修改源码

---

## 1. problem（要解决什么问题）

`pie` 已有心跳（cron/trigger），但存在两个关键缺失：

### 1.1 无状态 cron → 无法判断"增量"

`docs/loops.md` 第 8–10 行点明：普通 cron job 是"失忆"的，每次运行从空白上下文开始。用户问"自上次以来有什么变化"无法回答，因为没有"上次"。用户手工在 prompt 里粘贴基线信息（baseline blob），但基线旋即过时。

### 1.2 自动化输出无处可去

自动化运行的结果要么打断主对话（`InjectAndRun`），要么沉入 audit log 无人问津。`docs/issues/23-loops-inbox.md` 第 18–20 行将其描述为缺少"findings land in an inbox, human triages when convenient"的路由模式。

### 产品语义

核心交互模型来自 Addy Osmani 的 "Loop Engineering"（2026）：

- **Agent 主动发现**：cron / MCP notification / dynamic trigger 触发后，agent 运行子代理完成检查工作
- **结果投到 inbox**：发现的问题（findings）不中断主对话，而是写入全局 inbox
- **人类按需 triage**：通过 `/inbox claim` 将 finding 提升为真实 agent turn；或 `/inbox dismiss` 丢弃

`docs/loops.md:3` 将其概括为："Stop prompting the agent. Build loops that prompt the agent for you."

---

## 2. why_hard（为什么难）

### 2.1 持久状态

- Loop 的状态文件（`<session>.loop-<job-id>.md`）必须在跨运行时可靠读写
- 状态上限 2000 字符（`LOOP_STATE_MAX_CHARS`，`cron.rs:729`），越界截断并标注
- 状态总是被 agent 的 `<loop-state>…</loop-state>` 标签覆盖重写，没有增量合并
- 状态文件和 cron job 生命周期绑定（`/cron remove` 时一并删除）

### 2.2 去重（Dedup）

- Runtime 侧有全局 dedup 窗口（默认 5 分钟，`TriggerRuntimeConfig::DEFAULT_DEDUP_WINDOW`）
- MCP 通知的不同方法有各自的 replacement 策略（`LatestReplaces` vs `Drop`）
- 跨 MCP server 的同名 key 需要通过 `mcp:{server_name}:` 前缀隔离（`mcp_notification_hook.rs:258-310`）
- Custom 通知必须在 `custom:` 子命名空间下，防止与 built-in key（`tools`/`resources`/`prompts`）碰撞

### 2.3 权限与隐私

- 每个 Trigger 携带 `TriggerAuthority`（`trigger.rs:117-131`），包含 `principal_id`、`credential_scope`、`allowed_source_actions`
- `PayloadVisibility` 有三种级别：`Local`（payload 不入库）、`Shared`、`Redacted`
- MCP Notification Hook 的 `render_summary` 严格要求：自定义方法的 params 绝不序列化进 `payload_summary`，只有 method name 或显式 opt-in 的 `_meta.pie_summary` 才写入
- 敏感 URI 和 dedup key 被 SHA256 哈希后使用（`safe_idempotency_segment`）
- `NewTrigger` 工具的权限分类器（`permission_classification`）始终返回 `Prompt`，且 reason 是 "value-free" 的（不包含 condition/action 的内容，防止 token 泄露到 audit）

### 2.4 成本预算

- `CostTracker`（`cost.rs:32-91`）基于 `AtomicU64`/`parking_lot::Mutex` 进行无锁/低成本累加
- 每个 assistant 回合的 `Usage` 被实时记录（input/output/cache_read/cache_write tokens + USD）
- Budget cap 在 pre-turn 和 post-turn 两个检查点执行（`docs/issues/06-token-cost-budget.md:32-37`）
- 即使是 loop 的 sub-agent 运行，其消耗也被计入 session 总成本（通过 `CostTracker::as_listener` 监听 `AgentEvent::MessageEnd`）
- 灾备模型（fallback model）可在主模型 failure 后接管同一回合

### 2.5 失败可观测

- `tracing` subscriber 记录所有结构化日志（`docs/issues/14-observability.md`）
- OTLP 导出层（`otlp.rs`）— 当 `OTEL_EXPORTER_OTLP_ENDPOINT` 设置时，自动将 spans 批量 POST 到 collector
- Hook 运行失败不影响 agent turn（best-effort side effect，`hooks.rs:346-349`）
- Tag 提取（`<loop-state>` / `<inbox>`）失败不导致 run 失败——状态文件不动，inbox 不进内容，run 仍标记 completed（`docs/loops.md:85-86`）

### 2.6 与主对话上下文隔离

- Stateful cron job 使用 `TriggerDelivery::SubAgent`（`cron.rs:888`）
- Sub-agent 在独立上下文中运行，不接触主对话的消息历史
- Finding 通过 inbox 单向传递到主对话（只有当用户执行 `/inbox claim` 时）
- 普通 cron job 仍走 `InjectAndRun` 路径（`cron.rs:894`），直接注入主对话

---

## 3. design_approach（设计思路）

### 3.1 整体数据流

```
┌─────────────────────────────────────────────────────────────────┐
│                       Source Adapters                           │
│  (cron.rs / dynamic.rs / mcp_notification_hook.rs)              │
│  ┌──────────┐   ┌──────────────┐   ┌──────────────────────┐    │
│  │ CronHook │   │ DynamicHook  │   │ McpNotificationHook  │    │
│  │ (every   │   │ (every 10min)│   │ (push-driven)        │    │
│  │  30s)    │   │              │   │                      │    │
│  └────┬─────┘   └──────┬───────┘   └──────────┬───────────┘    │
│       │                │                      │                 │
│       ▼                ▼                      ▼                 │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │              TriggerSink (mpsc::unbounded)               │    │
│  └──────────────────────────┬──────────────────────────────┘    │
└─────────────────────────────┼────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                   AgentHarness (runtime)                         │
│                                                                  │
│  ┌──────────────────────┐                                       │
│  │ TriggerRuntime       │  dedup + cycle suppression            │
│  │  - dedup window      │                                       │
│  │  - cycle hop limit   │                                       │
│  └──────────┬───────────┘                                       │
│             │ Accept / Deduped / CycleSuppressed                │
│             ▼                                                    │
│  ┌──────────────────────┐                                       │
│  │ BeforeTriggerHook    │  permission evaluation                │
│  │  (Allow/Deny/Prompt) │                                       │
│  └──────────┬───────────┘                                       │
│             │ Allow                                              │
│             ▼                                                    │
│  ┌──────────────────────────────────────────────┐               │
│  │         BeforeTriggerActionHook              │               │
│  │  cron_action_hook:                           │               │
│  │    stateful? → SubAgent (loop mode)          │               │
│  │    else     → InjectAndRun                   │               │
│  │  before_trigger_action_hook (dynamic):       │               │
│  │    → SubAgent + dynamic rule evaluation      │               │
│  │  direct_inject_action_hook (MCP direct):     │               │
│  │    → InjectSummary / InjectAndRun            │               │
│  └──────────────┬───────────────────────────────┘               │
│                 │ TriggerAction { prompt, delivery }             │
│                 ▼                                                │
│  ┌──────────────────────────────────────────────┐               │
│  │         Sub-Agent / Action 执行              │               │
│  │  - SubAgent: 独立上下文，运行 agent loop      │               │
│  │  - InjectAndRun: 注入主对话上下文             │               │
│  │  - InjectSummary: 仅注入摘要文本              │               │
│  └──────────────┬───────────────────────────────┘               │
│                 │                                               │
│                 ▼                                               │
│  ┌──────────────────────────────────────────────┐               │
│  │         HarnessListener (回传处理)           │               │
│  │  cron_harness_listener:                      │               │
│  │    TriggerCompleted → extract <loop-state>   │               │
│  │                     → write state file        │               │
│  │                     → extract <inbox>         │               │
│  │                     → inbox::append()         │               │
│  │  fire_once_harness_listener (dynamic):       │               │
│  │    TriggerCompleted → mark_rules_fired()     │               │
│  └──────────────────────────────────────────────┘               │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Audit & Inbox                                 │
│                                                                  │
│  ┌────────────────────────┐    ┌──────────────────────────┐     │
│  │  SessionTreeEntry::    │    │  ~/.pie/inbox.jsonl      │     │
│  │    Custom {            │    │  (全局, JSONL 追加)      │     │
│  │      custom_type:      │    │  {                       │     │
│  │        "trigger"       │    │    "id": "inb-...",     │     │
│  │    }                   │    │    "source": "cron:...", │     │
│  │  (TriggerRecord)       │    │    "text": "...",       │     │
│  └────────────────────────┘    │    "status": "new"       │     │
│                                 │  }                       │     │
│                                 └──────────┬───────────────┘     │
│                                            │                     │
│                              用户通过 /inbox claim|dismiss       │
│                              与主对话交互                         │
└─────────────────────────────────────────────────────────────────┘
```

### 3.2 核心设计决策

| 决策 | 说明 |
|------|------|
| 纯文本协议 | `<loop-state>` / `<inbox>` 标签由 prompt 注入，模型输出解析。不依赖 provider 特性 |
| Tag 提取永不失败 | 畸形/缺失标签 = state 不动、inbox 不进、run 完成 |
| 有界设计 | State ≤ 2000 chars, entry ≤ 500 chars, 每 run 最多 16 条 finding |
| Sub-agent 隔离 | Loop 的 sub-agent 在独立上下文中运行，不污染主对话 |
| Adapter 模式 | `NotificationHook` trait 解耦 transport 与 runtime，adapter 可在 `coding-agent` 侧独立测试 |
| 全局 dedup | Runtime 侧统一处理 dedup + cycle suppression，adapter 不自己做去重 |

---

## 4. code_walkthrough（关键文件/类型/函数）

### 4.1 Trigger 信封（`crates/agent/src/harness/trigger.rs`）

核心类型 `Trigger`（第 27–65 行）是 transport 与 runtime 的边界：

```
Trigger {
    source: TriggerSource,          // Mcp | Local | AgentDelegate
    source_kind: SourceKind,        // Local | Mcp
    payload_visibility: PayloadVisibility, // Local | Shared | Redacted
    idempotency_key: String,        // 去重 key（必填）
    replacement_policy: ReplacementPolicy, // Drop | LatestReplaces | Coalesce
    trace_id: String,               // 链路追踪
    authority: TriggerAuthority,    // 权限声明
    ...
}
```

`TriggerRecord`（第 230–260 行）：持久化到 session audit 的记录，包含 `state: TriggerState` 状态机（Received → Accepted → Running → Completed / Deduped / CycleSuppressed / Failed）。

`TriggerState`（第 185–221 行）：9 种状态，`is_terminal()` 区分终态和中间态。

### 4.2 Trigger Runtime（`crates/agent/src/harness/trigger_runtime.rs`）

`TriggerRuntime` 是纯逻辑引擎，无 IO：

- `evaluate(&self, trigger: &Trigger) -> EvaluationOutcome`（第 191 行）：
  1. 先执行 dedup check：相同 `idempotency_key` 在窗口内 → `Deduped`
  2. 再执行 cycle check：`trace_id` 超过 `cycle_hop_limit` → `CycleSuppressed`
  3. 通过则 → `Accept`，记录 dedup entry + bump cycle counter
- 计数器：`accepted_total`、`deduped_total`、`cycle_suppressed_total` 永不递减
- `snapshot()` 方法提供 `TriggerRuntimeSnapshot` 用于 TUI 状态栏

### 4.3 NotificationHook trait（`crates/agent/src/harness/notification_hook.rs`）

```rust
#[async_trait]
pub trait NotificationHook: Send + Sync {
    fn label(&self) -> &str;
    async fn run(&self, sink: TriggerSink) -> Result<(), HookError>;
    fn status(&self) -> NotificationHookStatus;
}
```

`TriggerSink = mpsc::UnboundedSender<Trigger>`，多个 hook 共用同一 sink。

`NotificationHookStatus`（第 104–130 行）：包含 `HookState`（Connected / Reconnecting / Disconnected / Disabled / AuthFailed）、`dropped_count`、`last_event_at` 等。

### 4.4 Cron Hook（`crates/coding-agent/src/triggers/cron.rs`）

关键结构：

- `CronJob`（第 37–60 行）：包含 `stateful: bool` 字段（第 58 行），区分 loop 模式
- `CronRegistry`：`Arc<Mutex<CronRegistryState>>`，管理 job 的生命周期（增删改查 + 持久化到 TOML）
- `CronNotificationHook`（第 620 行）：每 30 秒扫描到期 job（`due_jobs`），构造 Trigger 并推入 sink

Loop 核心函数：

- `cron_action_hook`（第 847 行）：包装 `BeforeTriggerActionHook`
  - 非 stateful job → `TriggerDelivery::InjectAndRun`
  - stateful job → `TriggerDelivery::SubAgent` + 组合 prompt（含 `<loop-state>` + output protocol）
- `cron_harness_listener`（第 901 行）：监听 `HarnessEvent::TriggerCompleted`
  - 提取 `<loop-state>` 写入状态文件
  - 提取 `<inbox>` 追加到全局 inbox
- `compose_stateful_prompt`（第 773 行）：拼装 stateful prompt（前次状态 + 用户指令 + output protocol）
- `extract_tag_block` / `extract_tag_all`（第 819/829 行）：从模型输出中提取标签块

### 4.5 Dynamic Trigger（`crates/coding-agent/src/triggers/dynamic.rs`）

关键结构：

- `DynamicTriggerRule`（第 41 行）：`condition` + `action` + `fire_once` + `promote_to_chat`
- `DynamicTriggerCheckHook`（第 260 行）：每 10 分钟（可配置）触发一次 periodic check
  - 将当前启用的 rules 序列化为 JSON 注入 sub-agent prompt
  - Sub-agent 用 `render_dynamic_trigger_prompt`（第 686 行）评估条件
- `before_trigger_action_hook`（第 557 行）：包装 `BeforeTriggerActionHook`
- `fire_once_harness_listener`（第 672 行）：一次触发后禁用 fire_once 规则
- `direct_inject_action_hook`（第 609 行）：MCP server 直通注入（绕过 dynamic rules 评估）
- 中文触发词解析：`parse_trigger_rule` 支持中文 "当…的时候，执行…" / "如果…，则…" 等多种 pattern

### 4.6 MCP Notification Hook（`crates/coding-agent/src/triggers/mcp_notification_hook.rs`）

`McpNotificationHook` 将 `McpServerNotification`（来自 `McpClient`）映射为 `Trigger`：

- `map_notification`（第 182 行）：纯函数，根据 method + params 确定 `idempotency_key` 和 `replacement_policy`
- 映射表（第 12–19 行注释）：
  - `tools/listChanged` → `mcp:{server}:tools` / `LatestReplaces`
  - `resources/updated` → `mcp:{server}:resources:{uri}` / `LatestReplaces`
  - custom → `mcp:{server}:custom:{dedup_key}` / `Drop`（需要 `_meta.pie_dedup_key` 或 `_pie_dedup_key`）
- 安全问题：`safe_idempotency_segment` 对敏感/过长/含控制字符的 key 做 SHA256 哈希
- `render_summary`（第 354 行）：遵守 privacy contract，自定义 notification 不泄露 params 内容

### 4.7 Inbox（`crates/coding-agent/src/inbox.rs`）

全局 JSONL 文件 `~/.pie/inbox.jsonl`：

- `InboxEntry`（第 29–38 行）：`id`、`source`、`text`（≤500 chars）、`trace_id`、`session_id`、`status`
- `append()`（第 47 行）：追加一行 JSON
- `list()`（第 88 行）：读取所有条目，跳过损坏行
- `set_status()` / `dismiss_all_new()`（第 115/132 行）：状态变更 → 全文件重写
- 写锁：`parking_lot::Mutex`（`WRITE_LOCK`）保证进程内串行
- 容错：不可解析的行在读取时跳过，绝不删除

### 4.8 Hooks（`crates/coding-agent/src/hooks.rs`）

用户配置的 CLI hooks：

- 配置来源：`~/.pie/hooks.toml`（用户级）+ `<repo>/.pie/hooks.toml`（项目级，需显式启用）
- 11 种事件：`agent_start/end`、`turn_start/end`、`message_start/update/end`、`tool_start/update/end`、`compaction`
- Hook 动作：shell 命令 +/或 HTTP webhook POST
- `HookRunner`（第 33 行）：接收 `AgentEvent` / `HarnessEvent`，匹配规则并执行
- 子进程安全：Unix 下通过 `setsid()` 创建独立进程组，timeout/cancel 时 `killpg(SIGKILL)` 整棵树（`hooks.rs:428-441`）
- 环境变量注入：`PIE_HOOK_EVENT`、`PIE_TOOL_NAME`、`PIE_SESSION_ID` 等（第 749–782 行）
- Payload 截断：message summary 和 tool result 最长 2000 字符

### 4.9 OTLP（`crates/coding-agent/src/otlp.rs`）

手写的 tracing-subscriber Layer：

- 当 `OTEL_EXPORTER_OTLP_ENDPOINT` 设置时激活
- `on_new_span` 记录 span 开始时间和 attributes
- `on_close` 将完成的 span 推入 pending 队列
- 每 2 秒批量 POST 到 `/v1/traces`（fire-and-forget，不阻塞 agent）
- 服务名为 `"pie"`，版本来自 `CARGO_PKG_VERSION`

### 4.10 Cost Tracker（`crates/agent/src/harness/cost.rs`）

- `CostTracker`（第 32 行）：线程安全的累加器
- `record(&self, usage: &Usage)`（第 60 行）：saturating_add 累加各类 token 和 cost
- `as_listener()`（第 78 行）：返回 `AgentListener`，自动记录每个 assistant 消息的 usage
- `snapshot()` 提供给 `/cost` 命令和 status line 使用
- `CostSnapshot`（第 18 行）：包含 `Usage` + `turn_count`

### 4.11 Debug（`crates/coding-agent/src/debug.rs`）

LLM 调用的 debug 输出：

- `wrap_stream_fn`（第 21 行）：包装 stream function，输出 `[debug llm #N start/context/tool-call/done/error]` 行
- 自动 redact：通过 `bug_report::redact` 过滤 API key、token 等（第 227 行）
- 截断限制：最多 80 行 / 4000 字符（`bounded_preview`）

---

## 5. flows（关键执行流程）

### 5.1 Stateful Cron Loop（完整生命周期）

```
用户: /cron add --stateful "0 9 * * *" check GitHub issues

1. NewCronJobTool.execute() → global_cron_registry().add_job_full(schedule, action, stateful=true)
   → CronJob { stateful: true, ... } 写入 <session>.cron.toml

2. CronNotificationHook.run() 每 30s 调用 registry.due_jobs(last_scan, now)
   → 到期 job 被构造为 Trigger { idempotency_key: "cron:<job_id>:<due_at>" }
   → sink.send(trigger) 推入 runtime

3. TriggerRuntime.evaluate() → first admission → Accept
   → AgentHarness 持久化 TriggerRecord { state: Accepted }

4. BeforeTriggerHook → Allow（默认）

5. cron_action_hook:
   → job.stateful == true
   → 读取 loop state 文件（首次: "(first run)"）
   → compose_stateful_prompt(action, state) → 注入 [loop-state] + protocol
   → TriggerAction { delivery: SubAgent, prompt: "..." }

6. Sub-agent 在独立上下文中运行 agent loop（一次完整 turn）

7. 模型输出包含:
   <loop-state>checked issues #1-#10, new: none</loop-state>
   <inbox>PR #42 has been open for 2 weeks without review</inbox>

8. cron_harness_listener 监听到 TriggerCompleted { summary: "..." }:
   → extract_tag_block(summary, "loop-state") → write_loop_state(path, state)
   → extract_tag_all(summary, "inbox", 16) → inbox::append(inbox_path, ...)

9. 下次运行: 步骤 5 读取到前次的 loop state，注入 prompt

10. 用户: /inbox → 看到 finding #1
    /inbox claim 1 → 主对话收到 "Address this finding from loop cron-...: PR #42..."
```

### 5.2 Dynamic Trigger + Periodic Check

```
用户: "当 build 失败时，运行 cargo test --verbose"

1. 模型调用 NewTriggerTool → global_registry().add_rule("build fails", "cargo test --verbose")
   → DynamicTriggerRule { id: "dyn-abc123...", fire_once: true }

2. DynamicTriggerCheckHook.run() 每 10 分钟 tick:
   → enabled_count > 0 → 构建 Trigger { source_kind: Local, subkind: "dynamic" }
   → sink.send(trigger)

3. Runtime dedup + 接受后，before_trigger_action_hook:
   → render_dynamic_trigger_prompt(trigger, enabled_rules) 生成 prompt
   → TriggerAction { delivery: SubAgent, prompt: "{trigger JSON}\n{rules JSON}\nevaluate..." }

4. Sub-agent 用工具检查当前状态（bash, read 等），评估每个 rule 的条件

5. 如果条件匹配: sub-agent 输出 "matched dyn-abc123...: cargo test --verbose"
   → sub-agent 执行匹配 rule 的 action（调用 bash 工具）

6. Sub-agent 完成后，fire_once_harness_listener:
   → extract_dynamic_rule_ids(summary) → 找到 "dyn-abc123"
   → registry.mark_rules_fired(&["dyn-abc123"]) → rule.enabled = false, fired_at = Some(now)
```

### 5.3 MCP Notification Push

```
MCP Server (filesystem) → push "notifications/resources/updated" { "uri": "file:///src/main.rs" }

1. McpNotificationHook.run() 的 while let Some(notification) = rx.recv().await:
   → map_notification("filesystem", notification)
   → method = "notifications/resources/updated", uri = "file:///src/main.rs"
   → idempotency_key = "mcp:filesystem:resources:file:///src/main.rs"
   → replacement_policy = LatestReplaces
   → Trigger { source: Mcp { server_name: "filesystem", method: "..." }, ... }

2. sink.send(trigger) → TriggerRuntime.evaluate()
   → 如果是同一 URI 的重复推送（dedup 窗口内）→ LatestReplaces 语义，替换前一个

3. BeforeTriggerActionHook → direct_inject_action_hook（如果 server 在 inject_and_run_servers 集合中）
   → TriggerDelivery::InjectAndRun（将 summary 注入主对话，agent 在父上下文中反应）

   否则 → before_trigger_action_hook（dynamic 路径）
   → SubAgent 评估 dynamic rules

4. 如果 server 在 inject_summary_servers 中:
   → TriggerDelivery::InjectSummary（仅注入 summary，不运行模型）
```

### 5.4 Hook + OTLP 可观测链

```
用户配置 ~/.pie/hooks.toml:
[[hook]]
event = "tool_end"
tool = "bash"
command = "echo 'bash done' >> log"
webhook = "https://example.com/hook"

1. hooks::load() 在 harness 构建时加载配置
   → HookRunner { rules: [...], ... }

2. hook_runner.listener() 返回 AgentListener，订阅到 harness

3. 每次 AgentEvent::ToolExecutionEnd { tool_name: "bash", ... }:
   → HookRunner::handle_event() → EventData::from_agent_event()
   → 匹配到 tool="bash" 的规则
   → run_rule() 依次执行:
     a. run_command("sh -c 'echo bash done >> log'", payload_path)
        - 注入环境变量（PIE_HOOK_EVENT, PIE_TOOL_NAME, ...）
        - 进程组隔离（setsid）+ timeout 监控 + 超时 killpg
     b. run_webhook("https://example.com/hook", payload_json)
        - POST application/json，含 event/session_id/tool_name 等

4. Trace 侧: OtlpLayer 随 tracing subscriber 初始化
   → on_new_span 记录 span start + attributes
   → on_close 将 span JSON 推入 pending 队列
   → 每 2s flush_once → POST {resourceSpans: [...]} 到 OTLP collector
```

---

## 6. tests（测试覆盖）

### 6.1 单元测试

| 模块 | 测试内容 |
|------|----------|
| `trigger.rs` | 信封序列化往返、`PayloadVisibility`/`CredentialScope`/`TriggerSource` 的 wire 格式、`TriggerState::is_terminal` 全集、`TriggerRecord` 可选字段省略、未知字段容错、`replacement_policy` 必填性 |
| `trigger_runtime.rs` | 首次 admit、窗口内去重（返回 first arrival 的 policy）、dedup 窗口过期后重新 admit、cycle hop limit 抑制、`record_follow_up_hop` 独立 bump、无关 trace 互不干扰、snapshot 计数器 |
| `notification_hook.rs` | `NotificationHookStatus::pending()` 的序列化状态、`HookState` 各变体往返、snake_case kind tag、Error 消息区分度 |
| `inbox.rs` | 追加/列表/claim/dismiss 往返、缺失文件计为 0、文本修剪/截断、超大文本截断 + 省略号、损坏行跳过、status 重写、`dismiss_all_new` |
| `cron.rs` | Cron 表达式解析（步长/范围/Sunday 别名）、无效 schedule 拒绝、注册表持久化往返、tag 提取（存在/缺失/截断/cap）、loop-state path 计算、stateful prompt 组装（前次状态注入 + protocol）、`set_job_enabled` |
| `dynamic.rs` | 中文触发词解析（当…的时候，执行…/如果…，则…）、英文触发词、缺失 action 分隔符拒绝、规则持久化/reload、session 隔离、fire_once 标记后禁用、set_rule_enabled 重新激活、`looks_like_fixed_schedule_request` 拒绝固定调度 |
| `mcp_notification_hook.rs` | `tools/listChanged` → `LatestReplaces`、`resources/updated` 按 URI 分 key、custom 带 `_meta.pie_dedup_key` 通过、legacy `_pie_dedup_key` 兼容、自定义 key 红名化（敏感文本哈希）、无 dedup key 的 custom 被 drop、跨 server namespace 隔离、custom key 不与 built-in 碰撞、custom summary 不泄露 params、`pie_summary` opt-in、`pie_summary` redact、sink close 返回 SinkClosed、transport close 标记 Disconnected、二次 run 失败 |
| `hooks.rs` | 规则解析（跳过无效 event、跳过无 command/webhook 项）、command hook 接收 env + payload、compaction hook、webhook POST JSON、tool filter、timeout 杀死子进程树、cancel 杀死子进程树 |
| `cost.rs` | usage/cost 累加、reset 清空所有计数器 |
| `otlp.rs` | env 未设置时 `try_layer` 返回 None、`hex_random` 返回正确长度 |
| `debug.rs` | user/tool_result 中的 secret 被 redact、tool_call args 和 assistant text 中的 secret 被 redact、error message redact、debug preview 边界截断 |

### 6.2 集成测试

| 文件 | 测试覆盖 |
|------|----------|
| `tests/hooks_e2e.rs` | 构建 `AgentHarness`，加载 `hooks.toml`，运行 faux agent turn，验证 `turn_end` 事件的 webhook POST 到达 |
| `tests/dynamic_trigger_e2e.rs` | 模拟 conversation → 模型调用 `NewTrigger` 创建规则 → runtime Trigger 触发 → sub-agent 执行匹配 rule 的 bash action |
| `tests/commands.rs` | Slash 命令注册表测试：`/thinking`、`/cron`、`/inbox`、`/trigger` 等用户命令的 end-to-end 行为 |
| `tests/tools.rs` | 工具注册和执行集成测试（含 inbox、triggers 模块） |
| `harness_e2e.rs` | `handle_trigger` 的 Accept/Dedup/CycleSuppressed 路径、"trigger" CustomEntry 的 skip-fold 逻辑、`register_notification_hook` 驱动 pump → 触发 `handle_trigger`、`before_trigger` 的 Allow/Deny/Prompt 权限路径、compaction 触发逻辑 |

---

## 7. risks（生产自动化风险与未完成点）

### 7.1 高风险

1. **Inbox 并发写入竞争**（`docs/issues/23-loops-inbox.md:78-79`）：多进程同时 append 没问题（行级追加），但 status 更新是全文件重写，last-writer-wins — v1 可接受，但高并发场景可能导致 claim/dismiss 丢失
2. **Loop state 文件损坏**：如果 loop state markdown 文件被外部工具修改或截断，下次注入到 sub-agent prompt 的可能是乱码——agent 可能产生错误的"基线"
3. **Dedup 窗口基于内存**：`TriggerRuntimeConfig::MAX_DEDUP_WINDOW = 24h`，崩溃重启后 dedup map 丢失，可能导致重复触发
4. **无回退机制**：如果 tag 提取失败（如大模型不按 protocol 输出），loop 的状态会"卡住"——下次运行仍看到旧状态

### 7.2 中风险

5. **成本失控**：Loop sub-agent 每次运行消耗 token，如果 schedule 过于频繁（如 `* * * * *`）+ action 复杂，可能在后台烧钱。Budget cap 可缓解，但 loop 的成本可见性（是否单独列出 loop 成本）尚不明确
6. **Cycle suppression 的 trace 生命周期**：`trace_id` 的 hop counter 清理依赖 `dedup_window` 语义重用，如果 trace 的频率低于窗口，老 trace 可能永远不会被清理（虽然有 pruning，但仅在下次 evaluate 时触发）
7. **Sub-agent 结果不完整**：4 KiB summary 上限（`cron_harness_listener` 接收的 `summary` 字段）可能在模型输出很长时截断 `<loop-state>` 或 `<inbox>` 标签（`docs/loops.md:87`，`docs/issues/23-loops-inbox.md:86`）

### 7.3 未完成点（Phase 3 + 后续）

| 项目 | 状态 | 来源 |
|------|------|------|
| Maker/checker 验证 | Phase 3，独立跟踪 | `docs/loops.md:143-145` |
| Loop state 打包进 `/session export` | v2 | `docs/loops.md:146` |
| Web inbox panel（含按钮 UI） | v1 仅 sidebar badge | `docs/loops.md:147-148` |
| Worktree 隔离（parallel loops） | 不在 scope | `docs/issues/23-loops-inbox.md:110` |
| 远程遥测（OTLP export） | 已完成（`otlp.rs`） | `docs/issues/14-observability.md:82` |
| 预算 cap + 灾备模型 | 已设计，部分实现（`cost.rs`） | `docs/issues/06-token-cost-budget.md` |
| `/bug-report` 打包 | 已设计，部分实现（`bug_report.rs`） | `docs/issues/14-observability.md` |
| `AgentDelegate` 变体的 runtime 消费 | 类型就绪，无消费者 | `trigger.rs:83-87` |

---

## 8. next_questions（下一轮精读问题）

1. **`AgentHarness::handle_trigger` 的完整状态机实现**：当前 `trigger.rs` 和 `trigger_runtime.rs` 是纯类型和逻辑，实际的状态机推进代码在 `agent_harness.rs` 中（被 `harness_e2e.rs` 所测试）。下一次精读应追踪 Trigger 从 `handle_trigger` 进入到 `TriggerCompleted` 发出的每一步状态转换。

2. **`BeforeTriggerHook` 权限评估的完整链路**：`trigger.rs:250` 的 `evaluator_decision: Option<serde_json::Value>` 是 opaque JSON。实际权限评估逻辑在哪里？Rule 引擎（RFC 4）的 DSL 如何将 `TriggerSource` + `TriggerAuthority` 映射为 Allow/Deny/Prompt？

3. **Loop 与 `TriggerPromoted` 的交互**：`PromoteAction` 有多种形式（`PromoteSummaryNow` / `PromoteSummaryWhenSummaryContains` / etc.）。stateful cron loop 的 finding 如何通过 promote 机制进入主对话（如果使用了 `promote_to_chat`）？

4. **Budget cap 与 loop 的交互**：`CostTracker` 是如何跨 sub-agent 和主对话共享的？Loop sub-agent 运行时，如果触发 budget cap，是否会中断 sub-agent 还是仅影响主对话？

5. **Compaction 与 loop state 的关系**：自动 compaction 是否会影响 loop state 文件？Compact 后的上下文是否会被 loop sub-agent 继承？

6. **`AdversarialTemplate` 的审查逻辑**：Phase 3 的 maker/checker 验证——adversarial prompt 如何生成？两个 sub-agent 的结果如何对比？

7. **Session export/import 中的 trigger 数据**：`TriggerRecord` 作为 `SessionTreeEntry::Custom { custom_type: "trigger" }` 存储，`build_session_context` 中有明确的 skip 逻辑（跳过 trigger entries）。导出时这些记录如何处理？恢复时 dedup 状态如何重建？
