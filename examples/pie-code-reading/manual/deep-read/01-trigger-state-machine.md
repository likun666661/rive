# 精读报告：Trigger 状态机

> 阅读基线: `f1c35a3`
> 深度档位: `maintainer`
> 阅读范围: `crates/agent/src/harness/` + `crates/coding-agent/src/triggers/`

---

## 1. problem

pie 是一个持久化对话代理。用户不仅需要"一次对话"的交互，还需要**外部事件驱动代理自动执行任务**。例如：

- MCP 服务器推送 `tools/listChanged` 通知后自动更新工具目录
- cron 定时任务到点后自动执行检查脚本
- 自然语言描述的"当某条件成立时，执行某动作"（dynamic trigger）
- MCP 自定义通知到达时注入父对话或触发子代理

`Trigger` 状态机要解决的核心问题是：**如何在事件驱动和用户交互之间建立一个可靠、可审计、可去重、可抑制循环的自动触发执行管道**。

## 2. why_hard

### 2.1 自动触发的安全边界

自动触发本质上是"代理自我修改"——一个外部事件让代理执行可能修改文件系统、调用 API 的代码。必须区分：
- 事件来自谁（`TriggerAuthority`）
- 事件的 payload 有多敏感（`PayloadVisibility`）
- 执行结果是否写入父会话（`PromoteAction`）

### 2.2 去重（dedup）

同一个事件可能被重复推送（网络重传、MCP 重连、cron 瞬间 tick 多次）。去重需要：
- 全局 `idempotency_key` 的唯一性保证
- 不同来源的去重策略不同（`ReplacementPolicy::Drop` vs `LatestReplaces` vs `Coalesce`）
- 去重窗口的时间边界管理
- 跨 MCP 服务器命名空间隔离（`mcp:{server_name}:` 前缀）

### 2.3 循环抑制（cycle suppression）

代理执行 trigger 后可能产生新的 trigger（如 agent delegate 触发），形成无限链。需要 `trace_id` 传播 + `cycle_hop_limit` 硬限制（默认 5 跳）。

### 2.4 sub-agent 的设计挑战

父 `Agent` 是单租户的，每次只能运行一个 prompt cycle。trigger 必须在独立的 task 上运行 sub-agent。这带来：
- 父/子会话隔离（子 agent memory session，丢弃后不污染父会话）
- 取消传播（`CancellationToken` 从 harness 传到子 agent）
- 结果回写（`trigger_result` audit entry + `apply_promotion`）
- 竞态：父 agent 正在 streaming 时，promotion 如何安全插入

### 2.5 promotion 的授权通道

子 agent 的输出是 free-form LLM 文本。如果 promotion 条件依赖 free-form 文本做 substring 匹配，模型可以通过改写输出绕过条件。因此设计了结构化授权通道 `PromotionCondition::AnyOf`，对结构化 `trigger_result.details` 做 JSON Pointer 匹配。

## 3. design_approach

pie 的 Trigger 系统的核心设计：

1. **传输无关的 envelope**：`Trigger` 结构体是统一的"事件信封"，不关心传输层（MCP/cron/file-watch/agent-delegate）。传输适配器负责构造 envelope。

2. **两层去重 + 循环抑制引擎**：`TriggerRuntime` 是纯内存引擎，对每个 incident 做 `idempotency_key` 去重和 `trace_id` 循环计数，返回 `EvaluationOutcome`。

3. **权限钩子（permission hook）**：`BeforeTriggerHook` 在去重通过后、动作执行前运行，可以 `Allow` / `Deny` / `Prompt`。

4. **动作钩子（action hook）**：`BeforeTriggerActionHook` 将 accepted trigger 解析为 `TriggerAction`（包含 `prompt`、`delivery` 模式、`promote` 策略）。

5. **三种交付模式**：
   - `SubAgent`（默认）：启动子 agent 执行 `prompt`
   - `InjectSummary`：不调用模型，直接注入 `payload_summary`
   - `InjectAndRun`：注入父对话并要求 embedder 运行一个父循环 turn

6. **持久化审计**：每个 trigger 生命周期阶段都写入 `SessionTreeEntry::Custom { custom_type: "trigger" }`，包含状态机转移的完整记录。

## 4. code_walkthrough

### 4.1 `crates/agent/src/harness/trigger.rs` — Envelope 与 Record 类型定义

**核心类型：**

- `Trigger`（L27-L65）：运行时事件信封。包含：
  - `source: TriggerSource` — 来源分类（`Mcp` / `Local` / `AgentDelegate`）
  - `source_kind: SourceKind` — UI 分组维度（`Local` / `Mcp`）
  - `idempotency_key: String` — **必需**的去重键
  - `replacement_policy: ReplacementPolicy` — **必需**的去重策略（`Drop` / `LatestReplaces` / `Coalesce`），缺失则反序列化失败
  - `trace_id: String` — 审计链路追踪 ID
  - `payload_visibility: PayloadVisibility` — 隐私层级（`Local` / `Shared` / `Redacted`）
  - `authority: TriggerAuthority` — 来源权限声明
  - `received_at: DateTime<Utc>` — 适配器设置的接收时间

- `TriggerSource`（L74-L88）：带 `serde(tag = "kind")` 的内部标记枚举，用 `snake_case` 区分 `mcp` / `local` / `agent_delegate`。

- `TriggerState`（L187-L206）：状态机状态集合：
  - 瞬时态：`Received`、`Accepted`、`Running`
  - 终态（`is_terminal()`）：`Deduped`、`CycleSuppressed`、`PermissionDenied`、`NeedsApproval`、`Failed`、`Completed`

- `TriggerRecord`（L229-L260）：持久化的审计记录。`schema_version = 1`，`CUSTOM_TYPE = "trigger"`。包含 `received_from()` 构造器、`evaluator_decision`（opaque JSON）、`result_link`、`rule_name` 等可选字段。

- `ReplacementPolicy`（L150-L163）：`Drop`（丢弃重复） / `LatestReplaces`（最新替换） / `Coalesce`（合并）。

### 4.2 `crates/agent/src/harness/trigger_runtime.rs` — 去重 + 循环抑制引擎

**核心类型：**

- `TriggerRuntimeConfig`（L30-L54）：`dedup_window: Duration`（默认 5 分钟，最大 24 小时），`cycle_hop_limit: u32`（默认 5）。

- `EvaluationOutcome`（L59-L74）：
  - `Accept`：首次进入，可继续处理
  - `Deduped { replacement_policy, previous_trace_id }`：重复事件
  - `CycleSuppressed { hop_count }`：循环达到上限

- `TriggerRuntime`（L78-L82）：内部 `Arc<Mutex<Inner>>`，包含两个 `HashMap`：
  - `dedup: HashMap<String, DedupEntry>` — `idempotency_key` → 首次到达条目
  - `cycle: HashMap<String, CycleEntry>` — `trace_id` → 跳数计数器
  - 三个单调递增的寿命计数器：`deduped_total`、`cycle_suppressed_total`、`accepted_total`

**核心方法 `evaluate()`（L191-L244）**：

```
1. 锁定内部 mutex
2. prune_expired() — 清除超过 dedup_window 的条目（惰性修剪）
3. prune_expired_cycle() — 同样对 cycle map 做惰性修剪
4. 去重检查：inner.dedup.get(&trigger.idempotency_key)
   → 命中 → 返回 Deduped（不修改 cycle 计数器！）
5. 循环检查：inner.cycle.get(&trigger.trace_id)
   → hop_count >= cycle_hop_limit → 返回 CycleSuppressed
6. Accept 路径：
   - 插入 dedup 条目
   - 插入/递增 cycle 条目（hop_count++）
   - accepted_total++
7. 返回 Accept
```

**关键设计决策**：
- 去重检查在循环检查之前，因为重复事件"不是真实事件"（L198-L199）
- 循环抑制时不递增 hop_counter，报告"抑制前"的跳数，便于审计（L451-L457）
- 使用 `saturating_add/saturating_sub` 防止溢出
- `record_follow_up_hop()` 允许无 Trigger envelope 递增跳数（用于 agent delegate 等场景）

### 4.3 `crates/agent/src/harness/notification_hook.rs` — 传输适配器 trait

**核心抽象：**

- `NotificationHook` trait（L39-L53）：
  - `label()`：稳定标签，用于 UI 行和计数器
  - `run(sink: TriggerSink)`：驱动源，推送 `Trigger` 到 sink
  - `status()`：快照当前状态

- `NotificationHookStatus`（L104-L130）：
  - `state: HookState` — `Connected` / `Reconnecting` / `Disconnected` / `Disabled` / `AuthFailed`
  - 监控字段：`last_event_at`、`last_error`、`queued_count`、`dropped_count`、`deduped_count`
  - `requires_attention` — UI 高亮标志

- `HookError`（L65-L96）：区分 `AuthFailed`（不自动重启）和 `Disconnected`（指数退避重启）

- `TriggerSink = mpsc::UnboundedSender<Trigger>` — 无界通道（v1 设计）

### 4.4 `crates/agent/src/harness/agent_harness.rs` — AgentHarness 与 handle_trigger 管道

**`handle_trigger()` 方法（L1117-L1246）**是整个状态机的入口点：

```
handle_trigger(trigger) → EvaluationOutcome:

1. 发出 TriggerHandlingStart 事件
2. trigger_runtime.evaluate(&trigger) → EvaluationOutcome
3. 根据 outcome 计算 state + evaluator_decision:
   - Accept:
     → 运行 before_trigger_hook → Allow/Deny/Prompt
       - Allow → state = Accepted
       - Deny → state = PermissionDenied
       - Prompt → 调用 resolve_trigger_prompt() → Allow/Deny/Timeout
         - Allow → state = Accepted
         - Deny/Timeout → state = NeedsApproval
   - Deduped → state = Deduped
   - CycleSuppressed → state = CycleSuppressed
4. 构造 TriggerRecord, state=已决定的状态
5. 序列化为 JSON → session.append_custom("trigger", ...)
   - 失败 → 发出 PersistenceError 事件
6. 发出 TriggerHandled 事件
7. 如果 state == Accepted:
   → spawn_trigger_action(trigger) — 创建 detached tokio task
8. 返回 outcome
```

**`spawn_trigger_action()`（L1259-L1304）**：捕获所有必要状态（model, tools, hooks, session...），在 detached task 中运行 `run_trigger_action()`。

**`run_trigger_action()`（L2390-L2816）**完整子代理生命周期：

```
1. 创建 CancellationToken
2. 调用 before_trigger_action_hook（或 default_for）→ TriggerAction
3. 根据 delivery 分支:
   - InjectSummary: 直接注入 payload_summary，写 trigger_result，调用 apply_promotion，返回
   - InjectAndRun: 注入 prompt 到父对话，写 trigger_result，如果父空闲则发出 TriggerRequestsMainRun，返回
   - SubAgent: 继续以下步骤
4. 在 running_triggers 注册 + 发出 TriggerExecutionStarted
5. 构建子 Agent（MemorySessionStorage，继承父 model/tools/hooks/system_prompt）
6. tokio::select! { cancel.cancelled() vs agent.prompt(action.prompt) }
7. compute_sub_agent_outcome() → (success, summary, message_count)
8. 写 trigger_result audit entry 到父 session
9. 发出 TriggerCompleted 或 TriggerFailed
10. apply_promotion() — 根据 PromoteAction 回写结果到父会话
11. 从 running_triggers 移除
```

**`apply_promotion()`（L3022-L3251+）**：

- `PromoteAction::None` → 立即返回
- `PromoteAction::PromoteSummaryNow` → 渲染模板，附加 `[Trigger {trace_id}]` 前缀
- `PromoteAction::PromoteSummaryWhenSummaryContains`（deprecated）→ 对 summary 做子串匹配
- `PromoteAction::PromoteSummaryWhenResultDetailsMatch` → 对结构化 `details` 做 `PromotionCondition::AnyOf` 评估
  - 失败 → 写 `trigger_promotion { state: "skipped" }` audit
- 如果 `promote_requires_approval = true` → 写 `state: "pending"`，发出 `PromotionPending`
- 否则 → 插入父会话：
  - 如果父正在 streaming → 通过 follow_up queue 排队
  - 如果父空闲 → 直接 `session.append_message()` + 推入 `parent_agent.state()`

**关键 HarnessEvent 类型（L35-L224）**：

- `TriggerHandlingStart` → `TriggerHandled` → `TriggerExecutionStarted` → `TriggerCompleted`/`TriggerFailed` → `TriggerPromoted`
- `TriggerPromptRequest` — 需要用户确认时发出
- `TriggerRequestsMainRun` — InjectAndRun 且父空闲时通知 embedder
- `PromotionPending` — 需要审批的 promotion
- `PersistenceError` — audit 写入失败

**`register_notification_hook()`（L1460-L1495）**：
- 为每个 hook 创建一个 `(sink, rx)` 通道对
- Driver task: `hook.run(sink)`
- Pump task: `while let Some(trigger) = rx.recv() { harness.handle_trigger(trigger).await }`

**`notification_status_snapshot()`** 返回 hooks 状态 + runtime snapshot + running triggers。

### 4.5 `crates/coding-agent/src/triggers/dynamic.rs` — 动态触发规则

**核心类型：**

- `DynamicTriggerRule`（L40-L53）：`id`、`condition`（自然语言条件）、`action`（执行指令）、`enabled`、`fire_once`（默认 true）、`promote_to_chat`、`fired_at`、`created_at`。

- `DynamicTriggerRegistry`（L59-L245）：
  - 线程安全的规则注册表（`Arc<Mutex<...>>`）
  - 支持 `add_rule` / `remove_rule` / `set_rule_enabled` / `mark_rules_fired` / `clear_rules`
  - 持久化到 JSON 文件（原子写入：write tmp + rename）
  - 全局单例 `global_registry()`

- `DynamicTriggerCheckHook`（L260-L368）：实现 `NotificationHook`，定期轮询（默认 10 分钟），当存在 enabled 规则时构造 `Trigger` 推入 sink。`idempotency_key` 用 `local:dynamic:{timestamp_ms}` 确保每次检查都是独立事件。

- `parse_trigger_rule()`（L452-L521）：支持中英文自然语言解析。中文模式：
  - `当...的时候，执行...`
  - `如果...，则...`
  - `...时，执行...`
  - `...时，则...`

  英文模式：`when ... run ...` / `if ... then ...`

- `before_trigger_action_hook()`（L557-L595）：构造 sub-agent prompt，包含：
  - 当前 Trigger 的 JSON（按 `payload_visibility` 决定是否包含 payload）
  - 所有 enabled 规则的 JSON
  - 评估指令（要求 sub-agent 检查每条规则的条件，对文件系统/环境/网络状态做必要检查）

- `direct_inject_action_hook()`（L609-L670）：包装 action hook，为配置了 `inject_summary_servers` 或 `inject_and_run_servers` 的 MCP 源绕过 sub-agent 路径。

- `fire_once_harness_listener()`（L672-L684）：监听 `TriggerCompleted`，从 summary 中提取 `dyn-{uuid}` 规则 ID，调用 `mark_rules_fired()`。

- **AgentTool 实现**：`NewTriggerTool`、`ListTriggersTool`、`RemoveTriggerTool`、`SetTriggerStateTool`。
  - `NewTriggerTool` 权限分类始终为 `Prompt`（持久化自我修改），reason 为 value-free（只描述字段形态，不包含字段内容）
  - `SetTriggerStateTool`：disable → `Allow`（缩小权限），enable → `Prompt`（扩大权限）

### 4.6 `crates/coding-agent/src/triggers/cron.rs` — Cron 定时任务

**核心类型：**

- `CronJob`（L36-L60）：包含 `id`、`schedule`（5 字段 cron 表达式）、`action`（自然语言指令）、`enabled`、`running_trace_id`（防止重叠）、`last_fired_at`、`last_completed_at`、`skipped_overlap_count`、`stateful`（循环模式）。

- `CronRegistry`（L70-L278）：
  - 持久化为 TOML 文件（`.cron.toml`）
  - 启动时 `clear_stale_running_state()` 清除上次异常退出的残留 running 状态
  - `due_jobs()`：
    - 解析 cron 表达式
    - `running_trace_id.is_some()` → 跳过 + `skipped_overlap_count++`
    - 否则分配新的 `trace_id`，设置 `running_trace_id`、`last_fired_at`
    - 只在状态真正变化时写 sidecar（idle session 优化）

- `CronNotificationHook`（L622-L693）：实现 `NotificationHook`，每 30 秒 tick，调用 `registry.due_jobs()` 构造 `Trigger`。

- `cron_action_hook()`（L847-L899）：
  - 非 cron trigger → 委派给 inner hook
  - `stateful = false` → `TriggerDelivery::InjectAndRun`（注入父对话执行）
  - `stateful = true` → `TriggerDelivery::SubAgent`（独立子代理，注入 loop state）

- `cron_harness_listener()`（L901-L946）：
  - `TriggerCompleted` → `mark_completed()` 清除 running_trace_id
  - stateful job → 提取 `<loop-state>...</loop-state>` 和 `<inbox>...</inbox>` 标签
  - loop state 写入 `{session}.loop-cron-{id}.md`（capped 2000 chars）
  - inbox findings 写入共享 inbox 文件

- `CronExpression` 解析器（L980-L1103）：支持 5 字段 cron（minute hour day-of-month month day-of-week），支持 `*` 通配、范围、步长、周日别名（7→0）。

- **AgentTool 实现**：`NewCronJobTool`、`ListCronJobsTool`、`RemoveCronJobTool`、`SetCronJobStateTool`。

### 4.7 `crates/coding-agent/src/triggers/mcp_notification_hook.rs` — MCP 通知适配器

**`McpNotificationHook`（L69-L175）**：
- 消费 `McpClient::take_notifications()` 的 receiver
- 将每个 `McpServerNotification` 映射为 `Trigger`

**映射规则 `map_notification()`（L182-L210）**：

| MCP method | idempotency_key | replacement_policy |
|---|---|---|
| `notifications/tools/listChanged` | `mcp:{server}:tools` | `LatestReplaces` |
| `notifications/resources/listChanged` | `mcp:{server}:resources` | `LatestReplaces` |
| `notifications/prompts/listChanged` | `mcp:{server}:prompts` | `LatestReplaces` |
| `notifications/resources/updated` | `mcp:{server}:resources:{uri}` | `LatestReplaces` |
| custom `notifications/*` | `mcp:{server}:custom:{dedup_key}` | `Drop` |

**两层命名空间设计（L21-L35）**：
- `mcp:{server_name}:` 前缀：防止不同 MCP 服务器的相同事件互相去重
- `custom:` 段：防止用户提供的 `pie_dedup_key` 与内置 slot（`tools`/`resources`/`prompts`）碰撞

**安全措施：**
- `payload_visibility` 始终 `Local`（原始 params 不进入审计）
- `safe_idempotency_segment()`（L328-L338）：对包含疑似 token 的 key 做 SHA-256 哈希（取前 6 字节 hex）
- `render_summary()`（L354-L385）：自定义通知的 summary 只包含 method name，除非服务器通过 `_meta.pie_summary` 明确 opt-in
- `redact_notification_text()`（L217-L236）：对 `hub_agent_*`、`sk-`、`bearer`、`token` 等敏感前缀做 `[redacted]` 替换
- 无 dedup key 的自定义通知直接丢弃（`dropped_count++`）

## 5. state_machine

以下是从 Trigger 到达至完成的完整状态转移图：

```
                    ┌──────────────┐
                    │  (适配器产生  │
                    │   Trigger)   │
                    └──────┬───────┘
                           │
                           ▼
                    ┌──────────────┐
                    │   Received   │  ← 瞬时态，envelope 刚到达
                    └──────┬───────┘
                           │
                    TriggerRuntime.evaluate()
                           │
              ┌────────────┼──────────────┐
              ▼            ▼              ▼
        ┌─────────┐  ┌──────────┐  ┌────────────────┐
        │ Accept  │  │ Deduped  │  │CycleSuppressed │
        │   ↓     │  │ (terminal)│  │  (terminal)    │
        │         │  └──────────┘  └────────────────┘
        │         │
   before_trigger  (permission hook)
   ┌────┼────┐
   ▼    ▼    ▼
┌────┐ ┌───┐ ┌──────┐
│Allow│ │Deny│ │Prompt│
└──┬──┘ └─┬─┘ └──┬───┘
   │      │       │
   │      │  ┌────┼────────┐
   │      │  ▼             ▼
   │      │ Allow      Deny/Timeout
   │      │  │             │
   │      │  ▼             ▼
   │      │               │
   ▼      ▼               ▼
┌──────────┐  ┌─────────────────┐  ┌──────────────┐
│ Accepted │  │ PermissionDenied│  │ NeedsApproval│
│  (瞬时)  │  │   (terminal)    │  │  (terminal)  │
└────┬─────┘  └─────────────────┘  └──────────────┘
     │
     │ spawn_trigger_action()
     │
     ▼
┌──────────┐  (inject summary/run 路径直接跳过 Running)
│ Running  │  ← 瞬时态，子 agent 正在执行
└────┬─────┘
     │
  ┌──┴───┐
  ▼      ▼
┌──────┐ ┌────────┐
│Failed│ │Completed│
│(term)│ │(term)  │
└──────┘ └───┬────┘
             │
        apply_promotion()
             │
    ┌────────┼───────────┐
    ▼        ▼           ▼
┌──────┐ ┌────────┐ ┌──────────┐
│ None │ │Promoted│ │ Pending  │
│(noop)│ │(inject)│ │(approval)│
└──────┘ └────────┘ └──────────┘
```

**转移条件详解：**

| 转移 | 条件 |
|------|------|
| → Received | 适配器构造 Trigger 并通过 sink 推送 |
| Received → Accept | `TriggerRuntime.evaluate()` 返回 `EvaluationOutcome::Accept` |
| Received → Deduped | `idempotency_key` 在 dedup_window 内已存在 |
| Received → CycleSuppressed | `trace_id` 的 `hop_count >= cycle_hop_limit` |
| Accept → Accepted | `before_trigger_hook` 返回 `Allow` 或 `Prompt → Allow` |
| Accept → PermissionDenied | `before_trigger_hook` 返回 `Deny` |
| Accept → NeedsApproval | `before_trigger_hook` 返回 `Prompt` 且 `on_trigger_prompt` 返回 `Deny/Timeout` |
| Accepted → Running | `spawn_trigger_action()` 成功启动 detached task |
| Running → Completed | 子 agent 正常完成，summary 非空 |
| Running → Failed | 子 agent 异常退出 或 `abort_trigger()` 触发 cancel |
| Completed → Promoted | `PromoteAction` 非 `None`，render 成功，无需审批 |
| Completed → Pending | `promote_requires_approval = true` |
| Completed → (无 promotion) | `PromoteAction::None` |

**`is_terminal()` 终态判定（`trigger.rs:208-221`）**：
- `Received`、`Accepted`、`Running` → `false`
- `Deduped`、`CycleSuppressed`、`PermissionDenied`、`NeedsApproval`、`Failed`、`Completed` → `true`

## 6. side_effects

### 6.1 `handle_trigger()` 每一步的副作用

| 步骤 | 副作用 | 描述 |
|------|--------|------|
| 到达 | `emit HarnessEvent::TriggerHandlingStart` | 通知 TUI/监听器开始处理 |
| evaluate | `TriggerRuntime` 内部 map 修改 | 去重 entry 插入 / cycle hop 递增 / 计数器更新 |
| evaluate(Deduped) | `deduped_total++` | 去重计数器递增 |
| evaluate(CycleSuppressed) | `cycle_suppressed_total++` | 循环抑制计数器递增 |
| before_trigger_hook | `trigger_prompt` audit 写入（Prompt 路径） | `session.append_custom("trigger_prompt", ...)` |
| 状态决定 | `TriggerRecord` 构造 + 序列化 | 组装包含 state + evaluator_decision 的 audit record |
| 持久化 | `session.append_custom("trigger", payload)` | 写入 `SessionTreeEntry::Custom { custom_type: "trigger" }` |
| 持久化失败 | `emit HarnessEvent::PersistenceError` | 通知观察者 audit 丢失 |
| 终态到达 | `emit HarnessEvent::TriggerHandled` | 通知 TUI/follow-up 逻辑 |

### 6.2 `spawn_trigger_action()` 的副作用

| 步骤 | 副作用 |
|------|--------|
| action 解析 | 调用 `before_trigger_action_hook`（可能涉及外部 IO） |
| InjectSummary | `trigger_result` audit 写入（`cost_usd: 0.0`, `message_count: 0`） |
| InjectAndRun | 消息注入父会话（直接 append 或 follow_up queue） + `trigger_result` audit |
| SubAgent 启动 | `running_triggers` 注册 + `emit TriggerExecutionStarted` |
| SubAgent 执行 | 子 agent prompt cycle（模型调用、工具调用） |
| 完成 | `trigger_result` audit 写入父 session |
| 失败 | `emit TriggerFailed` + reason |
| 成功 | `emit TriggerCompleted` + summary |
| promotion | `apply_promotion()` → 模板渲染、消息注入父会话、`trigger_promotion` audit |

### 6.3 `apply_promotion()` 的副作用

| PromoteAction 变体 | 副作用 |
|-------------------|--------|
| `None` | 无 |
| `PromoteSummaryNow` | 渲染模板 → truncate → `ensure_trigger_prefix` → 插入父会话 → `trigger_promotion` audit |
| `PromoteSummaryWhenSummaryContains` | substring 匹配 → 同上 |
| `PromoteSummaryWhenResultDetailsMatch` | `PromotionCondition::evaluate(details)` → 匹配则同上；不匹配则 `trigger_promotion { state: "skipped" }` |
| `promote_requires_approval` | `trigger_promotion { state: "pending" }` + `emit PromotionPending` |
| render 失败 | `trigger_promotion { state: "failed", redaction_status: "render_error"/"forbidden_field" }` |

### 6.4 `register_notification_hook()` 的副作用

- 创建 mpsc channel 对
- Driver task: `tokio::spawn(hook.run(sink))`
- Pump task: `tokio::spawn(while rx.recv → harness.handle_trigger())`
- Hook 注册到 `notification_hooks` vec

### 6.5 Dynamic Trigger 副作用

- `DynamicTriggerCheckHook.run()`：每 `interval` 秒检查 enabled 规则数，为 0 则跳过
- `before_trigger_action_hook()`：构造包含所有规则的 sub-agent prompt
- `fire_once_harness_listener()`：`TriggerCompleted` 时从 summary 提取 `dyn-{id}` 规则 ID，调用 `mark_rules_fired()`

### 6.6 Cron 副作用

- `CronNotificationHook.run()`：每 30 秒 tick，`registry.due_jobs()` 检查到期任务
- `due_jobs()`：更新 `running_trace_id`、`last_fired_at`、`skipped_overlap_count`，持久化 sidecar
- `cron_harness_listener()`：`TriggerCompleted` 时清除 `running_trace_id`，提取 `<loop-state>` 和 `<inbox>` 标签

## 7. tests

### 7.1 单元测试

| 文件 | 测试覆盖 |
|------|---------|
| `crates/agent/src/harness/trigger.rs` | Trigger envelope serde 往返、TriggerRecord 可选字段省略、前向兼容 unknown field 容错、`is_terminal()` 终态集合验证、`CredentialScope` PascalCase 序列化、`ReplacementPolicy` snake_case 序列化、`replacement_policy` 必需字段验证 |
| `crates/agent/src/harness/trigger_runtime.rs` | 首次进入 Accept、窗口内去重返回 Deduped（含 previous_trace_id）、去重时不递增 cycle 计数器、首次到达的 ReplacementPolicy 在窗口内胜出、窗口过期后重新 Accept、`cycle_hop_limit` 触发 CycleSuppressed、`record_follow_up_hop` 独立触发循环抑制、dedup_window 截断到 24 小时、不同 trace 的循环计数独立、snapshot 计数器准确性 |
| `crates/agent/src/harness/notification_hook.rs` | HookStatus/HookState 序列化往返、HookError 各类别 display 信息区分 |
| `crates/coding-agent/src/triggers/dynamic.rs` | 中英文 trigger 规则解析、`MissingAction` 错误、JSON 文件持久化往返、多 session 存储隔离、`remove_rule` 更新文件、`fire_once` 规则 fired 后 disabled、repeat 规则 fired 后保持 enabled、`set_rule_enabled` 重新激活、`dyn-{uuid}` ID 提取、`DynamicTriggerCheckHook` 周期检查、`before_trigger_action_hook` prompt 构建、三种 `PayloadVisibility` 下 payload 的包含/排除 |
| `crates/coding-agent/src/triggers/cron.rs` | Cron 表达式解析（steps/ranges/Sunday alias）、无效 schedule 拒绝、local time 计算、registry TOML 持久化、tag 提取（`<loop-state>`/`<inbox>`）、stateful prompt 注入、loop state 路径和写 cap、listener 持久化 state+inbox、sidecar 只在状态变化时写、启动时清除 stale running state、oversized action 拒绝、summary 中敏感信息 redaction、overlap 跳过机制 |
| `crates/coding-agent/src/triggers/mcp_notification_hook.rs` | tools/listChanged→LatestReplaces 映射、resources/updated per-URI keying、`_meta.pie_dedup_key` custom 路径、`_pie_dedup_key` 向后兼容、敏感 dedup key 哈希、无 dedup key 时 dropped_count 递增、无 uri 时的防御性回退、跨服务器 key 命名空间隔离、custom keys 不与内置 slots 碰撞（`custom:` 前缀）、custom method summary 不泄漏 params 内容、`_meta.pie_summary` opt-in 摘要、opt-in 摘要仍 redact 敏感文本、sink 关闭 → `HookError::SinkClosed`、transport 关闭 → `Ok(())` + Disconnected、receiver 单次消费、初始状态为 pending |

### 7.2 集成测试

| 文件 | 测试覆盖 |
|------|---------|
| `crates/agent/tests/harness_e2e.rs` | `handle_trigger_accept_persists_audit_custom_entry_with_accepted_state` — 验证 Accept 路径写 TriggerRecord audit + 发出正确事件；`handle_trigger_dedup_emits_deduped_state_and_persists_record` — 去重路径持久化两个 audit（Accepted + Deduped）；`handle_trigger_cycle_suppression_persists_cycle_suppressed_state` — 循环抑制持久化 CycleSuppressed；`notification_status_snapshot_reflects_trigger_runtime_counters` — snapshot 暴露运行时计数器；`handle_trigger_persistence_failure_still_returns_outcome_and_emits_error` — audit 写入失败不阻塞逻辑；`register_notification_hook_drives_pump_into_handle_trigger` — mock hook 产生 trigger 经 pump → handle_trigger；`register_notification_hook_snapshot_reflects_hook_status_state` — 状态快照反映 hook 状态；`before_trigger` 各类决策（Allow/Deny/Prompt）及其 audit 记录验证 |
| `crates/coding-agent/tests/dynamic_trigger_e2e.rs` | 自然语言对话创建 dynamic trigger → 运行时 trigger 事件 → 匹配规则 → bash 工具执行；中文/英文 cron job 创建（验证 NewCronJobTool vs NewTriggerTool 路径分流）；`promote_to_chat` 触发结果写入父对话上下文；audit-only 规则不 promotion；子 agent 继承父 skill catalog；periodic dynamic hook 周期性检查 |
| `crates/coding-agent/tests/commands.rs` | `/triggers` 状态只读；`/new-trigger` 注册动态规则；`/triggers remove <id>` 删除规则；`/triggers enable/disable` 切换规则状态；`/cron add/list/disable/enable/remove` 完整生命周期；cron list/add audit redact 敏感信息 |
| `crates/coding-agent/tests/tui_render_e2e.rs` | TriggerCompleted 渲染 `[trigger completed]`；TriggerHandlingStart 渲染 `[trigger fired]`；Deduped 状态渲染 `[trigger deduped]`；summary 不被截断；触发输出换行处理；长 summary 全量渲染；TriggerFailed 渲染 `[trigger failed]`；dynamic poll 无匹配时静默 |

## 8. risks

### 8.1 竞态条件

1. **Sub-agent promotion vs 父 agent streaming**（`agent_harness.rs:3240-3251`）：
   - promotion 时分两条路径：父 streaming → follow_up queue；父空闲 → 直接 append。两条路径的 session entry ID 来源不同（后者已知，前者未知直到 loop drain 后），`trigger_promotion` audit 的 `inserted_entry_id` 在 streaming 路径为 null。JSONL reader 必须按 `trace_id` join。
   - **风险**：如果 pump task 在父 streaming 和 idle 之间切换时恰好完成 promotion，消息可能重复或丢失。

2. **`running_triggers` 注册的时序**（L2604-L2619）：
   - 先注册 `running_triggers`，再 emit `TriggerExecutionStarted`。如果在注册和 emit 之间 snapshot 被读取，会看到 trigger 在 running 列表但还没有 ExecutionStarted 事件。

3. **Cron due_jobs 并发**（`cron.rs:200-241`）：
   - `due_jobs()` 在 `&self` 下修改内部 state（设置 `running_trace_id`、`last_fired_at`）。如果两个 tick 并发调用（虽然 cron hook 是单任务循环，但 registry 设计允许外部调用），可能竞争。

### 8.2 边界 bug

1. **TriggerRuntime 去重 map 无限增长**：
   - `dedup_window` 截断到 24 小时（`MAX_DEDUP_WINDOW`），但 24 小时内高频事件仍可能导致大量内存使用。惰性修剪只在 `evaluate()` 时触发，如果长时间无新事件，旧条目不释放。

2. **cycle map 生命期**：`cycle` entries 使用与 `dedup` 相同的 `dedup_window` 做惰性修剪（`trigger_runtime.rs:196`），但 cycle 的含义不同——一个 trace chain 可能在很久以后才结束。如果 chain 长度大于 window，可能出现中间跳数被修剪后重新计数。

3. **`record_follow_up_hop` 与 `evaluate` 的 hop 计数不同步**：
   - `record_follow_up_hop` 可能将一个 trace 的 hop_count 推到 `cycle_hop_limit`，此后任何新 trigger 都会被 suppress。但 `record_follow_up_hop` 本身不检查去重——可能被恶意调用。

4. **MCP notification hook 的 receiver 一次性消费**（`mcp_notification_hook.rs:123`）：
   - `rx.lock().take()` 后 `run()` 只能调用一次。如果 transport 断开后重连，必须重新创建整个 hook，不能复用。

5. **Cron `running_trace_id` 不一致恢复**（`cron.rs:310-320`）：
   - `clear_stale_running_state()` 在启动时清除所有 running trace。如果同一个 session 被两个进程并发打开（不应该），第二个进程会错误清除第一个的 running 状态。

6. **Dynamic trigger `fire_once` race**：`mark_rules_fired` 在 listener 中异步调用，如果同一规则被两个并发的 trigger action 匹配，可能在 `fire_once` 检查后、`fired_at` 设置前出现竞争，导致规则被多次执行。

### 8.3 设计 TODO / 未完成项

从代码注释中识别：

1. **Sub-agent retained branches**（`agent_harness.rs:1254-1258`）：子 agent session 是 `MemorySessionStorage`，任务完成后丢弃。计划在 "sub-PR 5c" 中实现 JSONL-backed retained branches，使 `--resume <trace_id>` 可以回放子 agent transcript。

2. **Bounded back-pressure**（`notification_hook.rs:27-29`）：TriggerSink 是无界通道，计划在后续版本通过 per-source `queued_count` 水位实现背压。

3. **`PromoteSummaryWhenSummaryContains` deprecated**（`agent_harness.rs:516-523`）：计划在 "downstream PRs" 中移除，完全迁移到 `PromoteSummaryWhenResultDetailsMatch`。

4. **`before_trigger_action_hook` 的 `promote` 路径**（`dynamic.rs:577-588`）：仍使用 deprecated 的 `PromoteSummaryWhenSummaryContains`，待 "Tools-MCP's follow-up PR" 迁移。

5. **AgentDelegate 来源**（`trigger.rs:83-88`）：placeholder，当前 runtime 接受但无 rule engine 消费。

6. **Cron 模型启用拒绝**（`cron.rs:576-580`）：`SetCronJobState` tool 拒绝从模型面 enable cron job，需要用户通过 `/cron enable <id>` 确认。

## 9. next_questions

1. **Sub-agent 的 cost 归因**：当前 `trigger_result.cost_usd: null`。5b/5c 计划如何将子 agent 的成本归入父 CostTracker？是否需要独立的 budget 限制？

2. **TriggerRuntime 的并发模型**：当前是单一 `Arc<Mutex<Inner>>`，在 `evaluate()` 期间持有锁。对高频 MCP 推送（如 file-watch），锁竞争会成为瓶颈吗？是否有计划做 per-source sharding？

3. **去重 window 的持久化**：`TriggerRuntime` 的 dedup/cycle 注册表是纯内存的。进程重启后去重 window 丢失，可能导致重复事件被重新处理。是否有计划持久化去重状态？

4. **AgentDelegate 的完整实现**：RFC 2 的多 agent 拓扑何时落地？当前 `TriggerSource::AgentDelegate` 的 rule 匹配和 loop 集成路径是什么？

5. **promotion 的审批流程**：`promote_requires_approval` 当前 fail-closed-to-pending。`/triggers approve <trace_id>` 的 UI 和持久化路径在哪个 sub-PR 中实现？

6. **Sub-agent 的 `TriggerResultDetailsBuilder`**：marker tools 的 builder 集成在哪个 PR 中？当前 `details` 始终为 `Null`，`PromoteSummaryWhenResultDetailsMatch` 的 `AnyOf` 条件实际上不可用。

7. **MCP 重连时的 hook 生命周期**：当前 hook 的 receiver 是 one-shot 的。transport 断开后如何在不丢失通知的前提下重建 hook？supervisor 的 reconnect 逻辑是什么？
