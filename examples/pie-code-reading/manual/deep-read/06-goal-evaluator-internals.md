# /goal Evaluator 与 OnTurnEndHook 内部机制精读

> 阅读基线：`f1c35a3`
> 深度档位：maintainer

---

## 1. problem

`/goal` 是 pie 的会话级目标机制：用户设置一个**自然语言条件**（例如 "finish only after cargo test passes"），agent 需要在**不依赖用户手动干预**的前提下，自动判断条件是否已满足，并在条件满足前**反复推进**任务。

这引出的核心工作流问题是：

- **自动循环控制**：一次 `prompt` 调用传统上只跑一个 prompt cycle（用户消息 → 模型回答 → 工具调用 → 最终文本）。goal 模式下，单次循环不保证完成任务，需要**自动发起多个后续 cycle**。
- **目标完成判断**：需要在一轮结束后，由一个**独立评估器（evaluator）**审视 transcript，判断 "goal condition 是否已满足"。
- **避免无限循环**：如果模型一直无法完成任务，系统必须有一个**硬上限**（continuation cap）来终止循环，而不是永远跑下去。
- **状态持久化**：goal 状态（pursuing/paused/achieved/budget_limited）需要在 `--resume` 时恢复。

`OnTurnEndHook` 是解决这些问题的基础设施：它是一个**prompt-cycle 边界钩子**，在每轮 agent loop 结束后被调用，决定是否要继续、停止、或暂停。

---

## 2. why_hard

自动判断目标完成 + 避免无限循环 + 避免 false positive/negative 是一个经典的 agent 控制难题：

### 2.1 目标完成判断的主观性

自然语言 goal condition（如 "make sure the README has installation steps"）本质上是一个**主观语义判断**。让代码（正则、规则）判断是否满足几乎不可能——必须依赖另一个 LLM 调用作为 "裁判"（evaluator）。

但 LLM evaluator 自身也有能力边界：
- 它只能看到 **bounded transcript**（40,000 字符尾裁剪），可能遗漏前文关键证据。
- 它可能 **hallucinate** 证据——声称目标已完成，但 transcript 中并没有对应的工具输出。
- 不同 evaluator 模型（或同一模型不同温度）可能给出不同判断。

### 2.2 false positive 的风险

如果 evaluator 说 `{"ok": true}` 但目标实际上没完成，用户被告知 "goal achieved" 后可能停止工作，导致任务半途而废。pie 的策略是：
- 要求 evaluator **引用 transcript 中的显式证据**（"quote evidence from the transcript"）
- 但如果模型编造了引用，human 无法在自动循环中验证。

### 2.3 false negative 的风险

如果 evaluator 反复返回 `{"ok": false}`，agent 会一直 continue，直到**continuation cap** 耗尽。但过程中可能产生大量**无意义的模型调用和 token 消耗**，成本快速累积。

### 2.4 无限循环的边界设计

每个 problem 都需要一个上限。pie 选择了双重上限：
1. **`MAX_CONTINUATIONS = 8`**：goal 层面的软上限，达到后将 goal 状态标记为 `BudgetLimited`。
2. **`turn_continuation_cap = 25`**（默认）：Harness 层面的硬上限，由 `DEFAULT_TURN_CONTINUATION_CAP` 定义。超过此上限后，运行时直接记录 `budget_limited` 决策而**不再调用 hook**。

上限值的选择是经验性的——太少会让复杂任务提前终止，太多会浪费 token 预算。

### 2.5 evaluator 的隔离性

Evaluator 是一个 **无工具、独立 in-memory 会话**的子 agent。这意味着：
- 它不能调用工具去验证（如 `bash cargo test`）——只能读 transcript。
- 它的运行时成本**不归入父 session 的 CostTracker**，用户账面成本会低估实际消耗。
- 它的 conversation 不会被 `--resume` 恢复（使用 `MemorySessionStorage`）。

---

## 3. design_approach

pie 的 evaluator/turn-end 设计遵循以下原则：

### 3.1 架构分层

```
用户设置 /goal <condition>
        │
        ▼
┌──────────────────────────────────┐
│  goal.rs (coding-agent 层)      │
│  - GoalState 状态机              │
│  - stop_hook() 构造 OnTurnEndHook│
│  - evaluate_stop_hook() 决策引擎  │
│  - evaluator prompt 组装          │
│  - parse_decision() JSON 解析     │
└──────────────┬───────────────────┘
               │ 通过 harness.run_evaluator()
               ▼
┌──────────────────────────────────┐
│  agent_harness.rs (agent-core)   │
│  - run_evaluator() 隔离子agent    │
│  - OnTurnEndHook 生命周期管理     │
│  - run_turn_with_continuation()  │
│  - turn_continuation_cap 硬上限  │
│  - record_turn_end_decision()    │
│  - HarnessEvent::TurnEnded 事件   │
└──────────────────────────────────┘
```

### 3.2 Hook 模式而非内置逻辑

Goal 不是硬编码在 harness 中的特殊路径。它通过 `AgentHarnessOptions::on_turn_end` 注册一个 `OnTurnEndHook`，完全由外层（coding-agent）控制决策逻辑。Harness 只负责：
- 在每轮结束时调用 hook
- 实施 `TurnEndAction::Continue`（发起新一轮 prompt）
- 强制 continuation cap
- 记录审计日志和发出事件

这使 hook 成为一个**可插拔的扩展点**——任何需要 "多轮自动循环" 的功能（不限于 goal）都可以复用。

### 3.3 Evaluator 作为隔离子 Agent

`AgentHarness::run_evaluator()` 创建一个：
- `tools: []` 的 bare `Agent`
- 独立的 `MemorySessionStorage`（不污染父 session）
- 不订阅 `CostTracker`（不归入父成本）
- 接受 `CancellationToken`（可被 Ctrl-C 中断）

### 3.4 状态持久化

Goal 状态通过 `Session::append_custom("goal_state", ...)` 写为 session 的 Custom entry，支持 `--resume` 恢复。这是一个 **append-only** 日志——每次状态变更（设置、暂停、恢复、完成、清空）都追加一条新记录，最新一条即为当前状态。

### 3.5 Prompt 策略

Evaluator 的 system prompt 明确要求：
- 只使用 transcript 中的显式证据
- 返回固定 JSON 形状：`{"ok": bool, "reason": string}`
- reason 必须引用 transcript 原文
- 证据不足时返回 `{"ok": false, "reason": "insufficient evidence in transcript"}`

Continuation prompt 将 evaluator 的 reason 注入下一轮，告诉主 agent "还缺什么"，让 agent 有针对性地补全。

---

## 4. code_walkthrough

### 4.1 `crates/agent/src/types.rs` — 基础类型

- **`AgentMessage`** (`types.rs:121`)：pie-ai Message 的超集，包含 `Llm(Message)` 和 `Custom(CustomMessage)` 两种变体。Goal 状态的 persistence 通过 `Custom { custom_type: "goal_state" }` 实现。
- **`AgentEvent::TurnEnd`** (`types.rs:345`)：agent loop 内部的 "一轮结束" 事件，由 agent loop 发出。这是 per-turn 粒度的；`HarnessEvent::TurnEnded` 是 harness 层的跨 cycle 事件。
- **`ThinkingLevel`** (`types.rs:59`)：evaluator 以 `ThinkingLevel::Off` 运行，避免 reasoning token 浪费在裁判任务上。
- **`OnControlPlanePromptHook`** (`types.rs:612`)：工具调用的用户确认通道，与 evaluator 无关但共享 hook 注册模式。
- **`AgentLoopConfig`** (`types.rs:567`)：包含 `should_stop_after_turn`、`prepare_next_turn` 等回调，是 per-cycle 的控制点，与 harness 层的 `OnTurnEndHook` 形成两层控制。

### 4.2 `crates/agent/src/harness/agent_harness.rs` — 核心基础设施

#### Hook 类型定义 (lines 642–770)

- **`OnTurnEndContext`** (`agent_harness.rs:667`)：hook 的输入快照，包含：
  - `transcript: Vec<AgentMessage>` — clone 出的完整 transcript（mutex 已释放）
  - `continuation_count: u32` — 当前 prompt cycle 已被 continue 的次数
  - `last_user_prompt: Option<String>` — 最近一条 User 消息的文本，帮助 evaluator 定位原始需求

- **`TurnEndAction`** (`agent_harness.rs:682`)：hook 的决策枚举：
  - `Noop`：不记录审计、不发出事件，等同于无 hook。Goal 在没有活跃 goal 时返回此项。
  - `Stop`：正常结束，记录 `decision: "stop"`。
  - `Pause { reason }`：软停止，携带原因。
  - `Continue { prompt }`：发起新一轮 prompt cycle，`prompt` 作为新的 User 消息注入。

- **`TurnEndDecision`** (`agent_harness.rs:729`)：包裹 `TurnEndAction` 和可选的 `payload`（用于透传 evaluator JSON 等元数据到审计日志）。

- **`OnTurnEndHook`** (`agent_harness.rs:763`)：函数签名 `Fn(OnTurnEndContext, CancellationToken) -> Future<Output = TurnEndDecision>`，与 `BeforeToolCallHook` 等共享相同的 `Arc<dyn Fn>` 模式。

- **`DEFAULT_TURN_CONTINUATION_CAP: u32 = 25`** (`agent_harness.rs:776`)：硬上限默认值。

#### `run_evaluator()` (lines 1974–2018)

```rust
pub async fn run_evaluator(
    &self,
    system_prompt: String,
    user_prompt: String,
    model: Model,
    thinking_level: ThinkingLevel,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<EvaluatorOutput, EvaluatorError>
```

- 创建 `AgentState`：`tools = Vec::new()`（无工具），`thinking_level = Off`（不推理）
- 使用 `Agent::new(AgentOptions { ..Default::default() })` 创建 bare agent——无 before/after_tool_call hooks
- 通过 `tokio::select!` 实现 cancel 感知：cancel 先触发则返回 `EvaluatorError::Cancelled`
- 结果 `EvaluatorOutput { last_assistant_text: Option<String> }`——文本被截断到 4 KiB

#### `run_turn_with_continuation()` (lines 1750–1880)

这是 `prompt()` / `continue_()` 的共享驱动：

```
loop:
  1. 创建 session listener，订阅 agent
  2. 执行 agent.prompt(msg) 或 agent.continue_()
  3. 取消 session listener，刷新持久化
  4. 检查 on_turn_end hook 是否存在（无 hook → 直接返回）
  5. 检查 continuation cap（超过 → budget_limited → 返回）
  6. Snapshot transcript，构造 OnTurnEndContext
  7. 设置 active_hook_cancel token，调用 hook
  8. 清除 active_hook_cancel token
  9. 匹配 hook 决策：
     - Noop → 返回
     - Stop → record_turn_end_decision("stop") → 返回
     - Pause → record_turn_end_decision("pause") → 返回
     - Continue → 递增 continuation_count，record，check budget，run auto-compaction
                   → 构造新 User 消息 → 循环回到步骤 1
```

关键点：
- `continuation_count` 使用 **saturating_add**，防止溢出
- 每次 Continue 后**重新检查 budget_cap_usd** 和**重新运行 auto-compaction**
- `active_hook_cancel` 与 `AgentHarness::abort()` 联动，使 Ctrl-C 能中断 hook（等待 evaluator 子 agent 调用时）

#### `record_turn_end_decision()` (lines 1911–1942)

- 持久化 `turn_end_decision` Custom audit entry（schema: `{decision, continuation_count, reason, next_prompt_preview, payload}`）
- 发出 `HarnessEvent::TurnEnded` 事件
- 持久化失败不 abort 循环（best-effort），通过 `PersistenceError` 事件通知

#### `AgentHarnessOptions` 中的相关字段 (lines 863–869)

- `on_turn_end: Option<OnTurnEndHook>` — 注册 hook
- `turn_continuation_cap: Option<u32>` — 覆盖硬上限

#### `HarnessEvent::TurnEnded` (lines 199–218)

包含：
- `decision: &'static str` — `"stop"` / `"pause"` / `"continue"` / `"budget_limited"`
- `continuation_count: u32` — post-decision 计数
- `reason: Option<String>` — pause/budget_limited 的原因
- `next_prompt_preview: Option<String>` — Continue 时的下一个 prompt 预览（前 80 字符）

### 4.3 `crates/coding-agent/src/goal.rs` — Goal 业务逻辑

#### 状态模型 (lines 21–61)

- `GoalStatus`：`Pursuing | Paused | Achieved | BudgetLimited | Cleared`
- `GoalState`：包含 `condition`（用户设置的条件文本）、`status`、`iterations`（已执行的 continue 次数）、`last_reason`（最近一次 evaluator 的 reason）、`updated_at`
- `active()`：`Pursuing | Paused | BudgetLimited` 均视为 active（可恢复）

#### 状态读写 (lines 69–145)

- `current()`：从 session entries 中反向扫描，找最新一条 `goal_state` Custom entry
- `set()`：写入 `GoalStatus::Pursuing` 状态
- `pause()` / `resume()` / `clear()`：修改状态并追加新 entry
- 所有写操作通过 `append_state()` → `session.append_custom("goal_state", json)` 实现持久化

#### `stop_hook()` (lines 223–235)

```rust
pub fn stop_hook(harness_cell: Arc<OnceLock<Arc<AgentHarness>>>) -> OnTurnEndHook
```

- 使用 `OnceLock` 解决 chicken-and-egg 问题：hook 在 harness 构造前创建，但需要 harness 引用才能调用 `run_evaluator()`
- `main.rs` 在 harness 构造后立即 `set()` 该 cell（`main.rs:807`）
- 如果 cell 未初始化，hook 返回 `Pause { reason: "goal hook was not initialized" }`

#### `evaluate_stop_hook()` (lines 237–332) — 核心决策引擎

```
1. 读取当前 goal state（无 → Noop）
2. 检查 status 是否为 Pursuing（非 → Noop）
3. 构建 transcript（tail 截断 40,000 字符）
4. 获取当前 model
5. 调用 harness.run_evaluator()
   - Cancelled → persist_pause → Pause
   - RunError → persist_pause → Pause
   - No text → persist_pause → Pause
6. parse_decision(text) → EvaluatorDecision { ok, reason }
   - 解析失败 → persist_pause → Pause
7. 更新 iterations += 1
8. 如果 ok == true:
   - 状态 → Achieved → Stop (payload 含 goal_payload)
9. 如果 iterations >= MAX_CONTINUATIONS (8):
   - 状态 → BudgetLimited → Pause
10. 否则:
   - 构建 continuation_prompt → Continue
```

**失败安全（fail-safe）设计**：evaluator 的任何异常（取消、运行失败、无输出、JSON 解析失败）都导致 `Pause`，而非 `Continue` 或 `Stop`。这避免了在评估器不可用时无限循环。

#### Transcript 构建 (lines 147–216)

- `transcript_from_messages()`：遍历 `AgentMessage::Llm`，提取角色标签 + 文本内容
  - User → `"User: {text}"`
  - Assistant → `"Assistant: {text}"`（合并所有 text/thinking/tool_call content blocks）
  - ToolResult → `"ToolResult({tool_name} error={is_error}): {text}"`
  - Image content → 跳过
- `tail_chars()`：取最后 `max_chars` 个字符，超出部分前缀 `"[transcript truncated to last {max_chars} chars]\n"`
- `TRANSCRIPT_CHAR_LIMIT = 40_000`：在保证足够上下文和避免 prompt 过大之间平衡

#### Evaluator Prompt (lines 366–379)

**System prompt**：
- 角色定位："evaluating a stop-condition hook in pie"
- 约束："You cannot call tools. Only use explicit evidence in the transcript."
- 输出格式：严格 JSON `{"ok": bool, "reason": string}`
- fallback："insufficient evidence in transcript"

**User prompt**：
```
Goal condition:
{condition}

Conversation transcript:
{transcript}
```

#### JSON 解析 (lines 381–399)

`parse_decision()` 先尝试标准 JSON 解析，失败后尝试从文本中提取 `{...}` 块（处理模型可能包裹 markdown code fence 的情况）。验证 reason 非空。

#### Continuation Prompt (lines 401–405)

```
The current /goal is not satisfied yet.

Goal condition:
{condition}

Goal evaluator says what is missing or blocking completion:
{reason}

Continue working toward the goal. Do not claim completion until the
transcript contains explicit evidence that satisfies the condition.
```

这是关键设计——它把 evaluator 的反馈**注入下一轮 prompt**，让主 agent 知道 "还缺什么"，从而有的放矢地推进任务。

### 4.4 `crates/coding-agent/src/commands.rs` — Goal 命令

- `GoalCommand` (`commands.rs:1006`)：处理 `/goal` 命令
  - 无参数：显示当前 goal 状态（调用 `print_goal_status`）
  - `pause` / `resume` / `clear`：调用对应的 goal 函数
  - `start <prompt>`：在有活跃 goal 时发起一次带提示的工作轮
  - 其他文本：视为 goal condition 设置（`/goal finish only after cargo test passes`）
- `GoalStartCommand` (`commands.rs:1092`)：`/goal-start` 快捷命令
- 命令返回 `CommandOutcome::RunAgentPrompt`，由 REPL 层通过 harness 执行

### 4.5 `crates/coding-agent/src/main.rs` — 组装

- `goal_harness_cell` (`main.rs:714`)：`Arc<OnceLock<Arc<AgentHarness>>>`，在 harness 构造前创建
- `opts.on_turn_end = Some(goal::stop_hook(goal_harness_cell.clone()))` (`main.rs:749`)：注册 goal hook
- `opts.turn_continuation_cap = Some(goal::MAX_CONTINUATIONS)` (`main.rs:750`)：设置 goal 层面的软上限
- `goal_harness_cell.set(harness.clone())` (`main.rs:807`)：在 harness 构造后立即填充（与 skill_harness_cell 一起，且有 assert 防双重设置）

### 4.6 `crates/coding-agent/src/ui/listener.rs` — 事件渲染

`map_harness_event()` (`listener.rs:213`) 处理 `HarnessEvent::TurnEnded`：
- `decision == "continue"` → `[goal continuing] {preview}` （System 级别）
- `decision == "pause" | "budget_limited"` → `[goal paused] {reason}` （Error 级别）
- `decision == "stop"` → **不渲染**（正常结束是安静的）

---

## 5. evaluator_loop

### 完整时序

```
时间线:

1. 用户: /goal finish only after cargo test passes
   → goal::set() 写入 GoalState { status: Pursuing, iterations: 0 }
   → session 持久化 goal_state entry

2. 用户: cargo build
   → harness.prompt("cargo build")
   → run_turn_with_continuation():
     ├─ agent.prompt("cargo build")    ← 第一轮 prompt cycle
     │  ├─ agent loop: 模型调用 → 工具调用(cargo build) → 返回结果
     │  └─ AgentEvent::TurnEnd
     ├─ on_turn_end hook 触发:
     │  └─ goal::evaluate_stop_hook():
     │     ├─ current(&harness) → Some(GoalState { status: Pursuing })
     │     ├─ transcript_from_messages() → 尾部40K字符
     │     ├─ harness.run_evaluator():
     │     │  ├─ 创建裸 Agent (tools=[], MemorySessionStorage)
     │     │  ├─ agent.prompt(evaluator_user_prompt)
     │     │  ├─ 模型返回: {"ok":false,"reason":"missing cargo test output"}
     │     │  └─ 返回 EvaluatorOutput { text: "..." }
     │     ├─ parse_decision() → EvaluatorDecision { ok: false, reason: "missing cargo test output" }
     │     ├─ iterations: 0 → 1 (< MAX_CONTINUATIONS=8)
     │     ├─ persist_state_best_effort() → 写入新的 goal_state (iterations=1)
     │     └─ TurnEndDecision {
     │          action: Continue { prompt: "The current /goal is not satisfied yet.\n\n..." },
     │          payload: { goal_status, ok: false, reason, iterations: 1 }
     │        }
     ├─ run_turn_with_continuation() 处理 Continue:
     │  ├─ continuation_count: 0 → 1
     │  ├─ record_turn_end_decision("continue") → audit entry + HarnessEvent::TurnEnded
     │  ├─ check_budget_cap() + run_auto_compaction()
     │  └─ 构造新 User msg: agent.prompt(continuation_prompt)
     └─ 循环回到 agent loop

3. [自动] agent 收到 continuation_prompt
   → 第二轮 prompt cycle
   → 模型理解 "需要 cargo test"
   → 调用 bash cargo test
   → 返回测试通过结果

4. on_turn_end hook 再次触发 (continuation_count=1):
   ├─ current(&harness) → GoalState { iterations: 1 }
   ├─ transcript 现在包含 cargo test 的输出
   ├─ harness.run_evaluator():
   │  └─ 模型返回: {"ok":true,"reason":"cargo test output: 'test result: ok. 5 passed; 0 failed'"}
   ├─ parse_decision() → EvaluatorDecision { ok: true }
   ├─ iterations: 1 → 2
   ├─ status → Achieved
   └─ TurnEndDecision {
        action: Stop,
        payload: { goal_status: "achieved", ok: true }
      }

5. run_turn_with_continuation() 处理 Stop:
   ├─ record_turn_end_decision("stop") → audit entry + HarnessEvent::TurnEnded
   └─ 返回 → 控制权交还用户

6. TUI 显示:
   ├─ 无 TurnEnded 渲染 (stop 是安静的)
   └─ /goal 命令显示 goal 状态为 achieved
```

### BudgetLimited 路径

```
当 iterations 达到 MAX_CONTINUATIONS (8) 但 ok 仍为 false:

evaluate_stop_hook():
  ├─ state.iterations >= 8
  ├─ state.status = GoalStatus::BudgetLimited
  └─ TurnEndDecision {
       action: Pause {
         reason: "goal continuation limit reached (8); resume with /goal resume"
       }
     }

用户可执行 /goal resume 重置计数继续。
```

### 硬上限路径

```
当 turn_continuation_cap (25) 被超过：

run_turn_with_continuation():
  ├─ continuation_count >= 25
  ├─ record_turn_end_decision("budget_limited")
  └─ 直接返回（不再调用 hook）
```

---

## 6. false_decisions

### 6.1 误判风险矩阵

| 场景 | 风险类型 | 当前缓解措施 | 残余风险 |
|------|----------|-------------|---------|
| Evaluator 声称 ok 但任务未完成 | False Positive | Prompt 要求引用 transcript 证据 | Evaluator 可能 hallucinate 引用；无二次验证机制 |
| Evaluator 声称 not ok 但任务已完成 | False Negative | 无特殊处理，继续循环 | 浪费 iterations 直到 cap；用户可手动 /goal clear |
| Transcript 过大被截断 | 证据丢失 | `tail_chars(40,000)` 保留末尾 | 关键证据在头部丢失导致 false negative |
| Evaluator 模型太弱 | 判断不准 | 使用当前 active model | 便宜模型可能无法正确理解复杂条件 |
| 模型返回非标准 JSON | 解析失败 | `parse_decision` 支持从文本提取 `{...}` 块 | 极端格式仍可能解析失败 → Pause |
| Evaluator 返回空 reason | 解析失败 | `parse_decision` 显式检查 reason 非空 | 返回 Error → Pause |

### 6.2 Transcript 截断策略

`tail_chars()` 取最后 40,000 字符——这是一个**尾保留**策略。设计依据：
- 最近的消息最可能与当前 goal 状态相关
- 如果早期消息包含关键上下文（如用户原始指令），可能被截断
- `OnTurnEndContext` 提供 `last_user_prompt` 让 hook 知道最近一条用户文本，但不保留早期上下文

### 6.3 Prompt 策略分析

Evaluator system prompt 的设计细节：

1. **角色绑定**："You are evaluating a stop-condition hook in pie"——设定裁判身份
2. **能力限制**："You cannot call tools"——虽然有 `tools: []` 的代码保证，但 prompt 中也明确告知
3. **证据要求**："Only use explicit evidence in the transcript"——这是对抗 hallucination 的关键
4. **格式约束**：要求严格的 JSON——方便程序化解析
5. **fallback**："insufficient evidence in transcript"——提供标准的 "不知道" 答案，降低编造概率

Continuation prompt 的设计：
- 注入 evaluator 的 reason 让 agent 知道 "缺什么"
- 明确指令 "Do not claim completion until the transcript contains explicit evidence"——防止 agent 在 goal 未实际完成时过早声称完成

### 6.4 已知局限

- **Evaluator 成本不透明**：`run_evaluator()` 的 LLM 调用不归入父 `CostTracker`。用户看到的 `/cost` 不包含 evaluator 消耗。
- **无 evaluator 结果验证**：如果 evaluator 说 "ok: true, reason: cargo test passed"，但没有二次确认（如再次运行 test），这是单点判断。
- **MAX_CONTINUATIONS 固定为 8**：对复杂多步任务可能偏少，对简单任务可能偏多。目前没有自适应机制。

---

## 7. tests

### 7.1 单元测试 (`crates/coding-agent/src/goal.rs:419-437`)

| 测试 | 覆盖内容 |
|------|---------|
| `parses_json_decision_inside_text` | 验证 `parse_decision` 能从 markdown code fence 中提取 JSON（实际 LLM 常见输出格式） |
| `transcript_tail_is_bounded` | 验证 `tail_chars` 的截断行为——只保留末尾字符 |

### 7.2 集成测试 (`crates/coding-agent/tests/commands.rs`)

| 测试 | 覆盖内容 |
|------|---------|
| `dispatch_goal_sets_and_reports_session_goal` (L396) | `/goal <condition>` 设置和 `/goal` 查看 |
| `dispatch_goal_start_runs_prompt_when_goal_active` (L447) | `/goal start <prompt>` 在有活跃 goal 时发起工作轮 |
| `dispatch_goal_start_shortcut_runs_prompt_when_goal_active` (L483) | `/goal-start <prompt>` 快捷命令 |
| `dispatch_goal_start_requires_active_goal` (L519) | 无活跃 goal 时 `/goal start` 返回错误 |
| `dispatch_goal_clear_hides_current_goal` (L557) | `/goal clear` 清空 goal |
| **`goal_evaluator_false_returns_continuation_and_audits_reason`** (L585) | **核心测试**：直接调用 `goal::stop_hook`，验证 evaluator 返回 false 时 hook 返回 Continue，且 reason 被正确注入 continuation prompt |

### 7.3 Harness E2E 测试 (`crates/agent/tests/harness_e2e.rs`)

| 测试 (均从 L4959 开始的 "OnTurnEndHook" 段落) | 覆盖内容 |
|------|---------|
| `on_turn_end_hook_unset_keeps_legacy_single_cycle_behavior` (L5042) | 无 hook 时零开销——不发出 TurnEnded 事件，不写入 audit entry |
| `on_turn_end_hook_noop_writes_no_audit_no_event` (L5075) | Noop 决策不产生任何 audit/event，但 hook 确实被调用了一次 |
| `on_turn_end_hook_stop_emits_event_and_audits_payload` (L5126) | Stop 决策：发出 TurnEnded 事件 + 写入 audit entry + payload 透传正确 |
| `on_turn_end_continue_runs_second_turn_then_stops` (L5185) | Continue → Stop 两轮循环：验证 transcript 中有两轮 user/assistant 对 + audit entries 顺序正确 |
| `on_turn_end_continuation_cap_emits_budget_limited_without_invoking_hook` (L5285) | cap=2 时 hook 正好被调用 2 次，第 3 次被 runtime 拦截，记录 budget_limited |
| `run_evaluator_returns_last_assistant_text_from_isolated_sub_agent` (L5349) | evaluator 返回正确的文本 + 父 session 未被污染 |
| `run_evaluator_returns_cancelled_when_token_tripped_pre_dispatch` (L5386) | 预触发 cancel token 时 evaluator 返回 Cancelled 错误 |

### 7.4 UI 测试 (`crates/coding-agent/src/ui/listener.rs`)

| 测试 | 覆盖内容 |
|------|---------|
| `turn_end_continue_surfaces_goal_status_line` (L505) | `TurnEnded { decision: "continue" }` 渲染为 `[goal continuing]` + 预览文本 |
| `turn_end_stop_stays_quiet` (L523) | `TurnEnded { decision: "stop" }` 不产生任何 feed 输出 |

### 7.5 测试覆盖盲区

- **evaluator 返回 true 的路径**：在 command 集成测试中未直接覆盖 `goal_evaluator_true_returns_stop`（但 harness_e2e 的 Stop 测试覆盖了 Stop 决策本身）
- **BudgetLimited 后的 resume 路径**：无专门测试
- **Evaluator 使用不同模型时的行为**：所有测试使用 `faux_model()`，未测试真实模型差异
- **Transcript 截断导致的评估差异**：无针对性测试

---

## 8. risks

### 8.1 产品风险

| 风险 | 严重度 | 说明 |
|------|--------|------|
| **False positive 导致用户误信** | 高 | Evaluator 声称任务完成但实际未完成。用户停止工作后发现结果不对。目前无二次验证。 |
| **Continuation 成本不可预期** | 中 | 用户设置 goal 后，系统自动发起 N 次（最多 25 次）模型调用。用户可能未预期这些额外成本。 |
| **Evaluator 成本不透明** | 中 | `run_evaluator()` 的成本不计入 `/cost` 显示。用户账面成本与实际成本不符。 |
| **Goal 状态丢失** | 低 | 状态通过 session Custom entry 持久化，`--resume` 可恢复。但如果 session 文件损坏，goal 状态一同丢失。 |

### 8.2 工程风险

| 风险 | 严重度 | 说明 |
|------|--------|------|
| **Hook 执行阻塞主循环** | 中 | `OnTurnEndHook` 是同步等待的——evaluator 的模型调用可能耗时数秒到数十秒。在此期间，用户无法输入新 prompt。 |
| **active_hook_cancel 竞态** | 低 | `abort()` 和 hook 完成之间存在 `Mutex` 保护的 token 交换，但 abort 后的 token 状态变化与 hook 的 `.await` 有微弱竞态窗口。当前通过 `tokio::select! biased` + `CancellationToken` 处理。 |
| **MAX_CONTINUATIONS 硬编码** | 低 | `MAX_CONTINUATIONS = 8` 在 `goal.rs` 中硬编码。如果需要调整，需修改源码。可通过 `turn_continuation_cap` 间接控制但不改变 goal 自身的 `BudgetLimited` 语义。 |
| **Evaluator 模型固定为当前 active model** | 低 | 如果用户使用便宜模型作为主 agent，evaluator 也使用同一模型，可能导致判断能力不足。理想情况下应允许为 evaluator 指定独立模型。 |

### 8.3 安全风险

- **Prompt injection through condition**：用户可设置任意 goal condition 文本，该文本被注入 evaluator prompt。但由于 evaluator 无工具，唯一输出是 JSON 解析结果（`ok: bool`, `reason: string`），且 evaluator 在隔离的 in-memory 会话中运行，实际风险极低。
- **Transcript 可能包含敏感内容**：40,000 字符的 transcript 被发送给 evaluator 模型。如果 transcript 包含 API keys、密码等敏感信息，这些会泄露给模型提供商。当前无 transcript sanitization。

---

## 9. next_questions

1. **Evaluator 模型可配置性**：当前 evaluator 使用与主 agent 相同的模型。是否应该允许用户指定独立的 "evaluator model"（如使用更强的模型做裁判，用便宜的模型做执行）？

2. **自适应 continuation cap**：`MAX_CONTINUATIONS = 8` 是否应该根据 goal condition 的复杂度动态调整？例如，可以在 evaluator 的 JSON 中增加 `confidence` 字段，当模型连续多次给出低置信度的 false 时提前终止。

3. **Evaluator 二次验证**：对于 evaluator 返回 `ok: true` 的情况，是否应该增加一个二次验证步骤（如要求 evaluator 明确列出它找到的证据引用），或由主 agent 进行一次 "sanity check"？

4. **Transcript 截断策略优化**：当前 `tail_chars(40_000)` 可能丢失早期上下文。是否应该考虑更智能的截断——保留 system prompt + 初始用户消息 + 最近 N 个工具调用及其结果？

5. **Evaluator 成本归因**：是否应该给 evaluator 一个轻量级的 `CostTracker`，使 evaluator 的成本也能在 `/cost` 中显示（至少以 `evaluator_cost_usd` 字段形式）？

6. **多 goal 支持**：当前一个 session 只能有一个 goal。是否应该支持多个并行 goal（每个 goal 有独立的 evaluator 和 condition），类似于 check-list？

7. **Goal 模板/预设**：是否可以提供常用 goal 模板（如 "all tests pass", "README is complete", "build succeeds"），降低用户编写 condition 的 friction？

8. **Hook 的可组合性**：当前 `on_turn_end` 只支持一个 hook。如果有多个需要参与 turn-end 决策的组件（如 goal + 自动 code review + 自动 lint fix），是否需要 hook 链/组合器？
