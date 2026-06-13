# pie-agent-core Runtime / Harness / Session 粗读报告

> 阅读基线：`f1c35a3`
> 深度档位：`architecture`

---

## 1. problem

pie-agent-core 是 pie 的 agent 运行时核心层，它需要解决的核心问题是：**如何把一个用户输入文本——经 system prompt 组装、会话状态回放、LLM 调用、工具执行、中间结果持久化——最终可靠地转化为一个可审计、可恢复、可安全的完整交互循环**。

一条典型的用户请求流程是：

```
用户输入 text
  → AgentHarness::prompt(text)
    → 1. 检查 budget cap，超出则拒绝
    → 2. auto-compaction：若 token 估算超过阈值，先对旧消息做摘要压缩
    → 3. 将 user message 写入 session（JSONL append-only）
    → 4. Agent::prompt(AgentMessage)
      → 4a. emit AgentStart → MessageStart → MessageEnd
      → 4b. drive_loop
        → call_llm: 从 state 取 system_prompt / messages / tools / model
          → transform_context 转换消息列表
          → convert_to_llm 过滤 Custom 消息 → 只保留 pie_ai::Message
          → 构造 PiContext { system_prompt, messages, tools }
          → 通过 stream_fn 调用 LLM（带 abort token、thinking level）
          → 接收 AssistantMessageEvent 流，累加消息，emit MessageStart/Update/End
        → execute_tools: 从 assistant content 中提取 tool call
          → permission_classification: 工具自分类 Allow/Prompt/Block
          → before_tool_call hook: 可 block 或通过 prompt 要求人工确认
          → on_control_plane_prompt: 需要确认时走 fail-closed deny
          → 顺序或并行执行，支持 on_update 流式回调
          → after_tool_call hook: 修改结果
          → emit ToolExecutionStart/Update/End
        → should_stop_after_turn: 决定是否退出循环
        → prepare_next_turn: 允许 mid-run 修改 model/thinking_level
        → drain steering/follow_up queue
        → 若 stop_reason=ToolUse 则继续循环
      → 4c. finalize: emit AgentEnd，重置 is_streaming
    → 5. persistence listener 将所有 MessageEnd 写入 session
    → 6. OnTurnEndHook: /goal 等 orchestrator 可决定 Continue/Stop/Pause
    → 7. 若 hook 返回 Continue，循环回到步骤 2
```

## 2. why_hard

这层之所以难，在于以下维度的正交性：

### 2.1 可恢复 Session
- Session 是 append-only JSONL，每个 entry 都有 `parent_id` 形成树形 DAG
- `SessionStorage` trait 抽象了底层存储（Memory / JSONL 文件），支持 `get_path_to_root` 沿 parent 链回放
- `build_session_context` 必须正确处理 Compaction 标记（`first_kept_entry_id`），跳过已被摘要的旧消息
- 离线回放时，ThinkingLevelChange、ModelChange 等元数据条目需被重建到 AgentState
- `rehydrate_from_session` 是 CLI `--resume` 的唯一入口

### 2.2 Compaction（自动摘要压缩）
- 当 context_tokens 超过 context_window 的 80% 时自动触发
- `find_cut_point` 向后遍历，确保切点落在 user-message turn-boundary 上
- 摘要 LLM 调用有独立的 prompt budget 管理：`summarization_prompt_budget` 预留 20% slack 防止 token 估算误差
- 若 provider 返回 context overflow，自动 halve budget 并重试（最多 3 次）
- `trim_messages_for_summary_budget` 从后向前保留消息以控制 prompt 大小
- 中文字符 (CJK) 的 token 估算与 ASCII 不同，`estimate_text_tokens` 使用 `ascii/4 + non_ascii` 的保守策略

### 2.3 Tool Permission（工具权限）
- 两层防守：
  - **PermissionClassification** (per-tool)：Allow / Prompt（需人工确认） / Block（硬拒绝）
  - **BeforeToolCallHook**：用户注入的 hook，可 block 或提供自定义 prompt
  - **OnControlPlanePromptHook**：prompt 决议通道，fail-closed deny
- 合并语义严格：classifier 的 Prompt 不会被 hook 的 `default()` 清除；block 永远短路
- `compute_args_hash` 对 prepared args 做 canonical SHA-256，绑定 prompt 批准到具体调用
- PermissionPolicy 对 bash 工具做危险模式检测（sudo、curl|sh、rm -rf /、mkfs 等），含 token-aware 的 rm flag 检测

### 2.4 Trigger 子运行
- 外部事件（MCP push、cron 等）通过 `Trigger` 信封进入
- `TriggerRuntime` 做 dedup（5 分钟窗口）+ cycle suppression（最多 5 hops）
- 通过权限 hook → action hook → 独立 sub-agent 执行
- sub-agent 运行在 `MemorySessionStorage` 的独立 session 中，不污染父 agent
- 结果通过 `PromoteAction` 决定是否注入父 conversation

### 2.5 成本统计
- `CostTracker` 监听每个 assistant `MessageEnd`，累加 provider 返回的 Usage
- 支持 budget_cap_usd：超限后 `check_budget_cap` 直接拒绝新 prompt
- Evaluator sub-agent 不计入父 CostTracker

### 2.6 环境隔离
- `ExecutionEnv` trait 抽象文件系统和进程执行
- `NativeEnv` 实现真正的 tokio::fs + tokio::process
- 测试可用 mock 实现替换

## 3. design_approach

核心设计思路是**分层递进**，每一层职责清晰：

```
┌─────────────────────────────────────────────────────────────────────┐
│                        AgentHarness                                 │
│  ┌─────────┐  ┌─────────────┐  ┌──────────┐  ┌─────────────────┐  │
│  │ Session  │  │ Compaction  │  │  Cost    │  │ TriggerRuntime  │  │
│  │ 管理    │  │ 自动摘要    │  │  追踪    │  │ 事件去重/循环  │  │
│  └─────────┘  └─────────────┘  └──────────┘  └─────────────────┘  │
│                                                                     │
│  prompt() / continue_()                                             │
│       │                                                             │
│       ▼                                                             │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                      Agent (stateful)                       │    │
│  │  ┌──────────────────────────────────────────────────────┐   │    │
│  │  │            agent_loop (pure loop logic)               │   │    │
│  │  │                                                       │   │    │
│  │  │  drive_loop:                                          │   │    │
│  │  │    loop {                                              │   │    │
│  │  │      call_llm(model, context, tools)                  │   │    │
│  │  │        → transform_context → convert_to_llm            │   │    │
│  │  │        → stream_fn → pie_ai::stream_simple            │   │    │
│  │  │        → events: Start/Delta/Done/Error               │   │    │
│  │  │      execute_tools(assistant)                         │   │    │
│  │  │        → permission_classification                    │   │    │
│  │  │        → before_tool_call hook                        │   │    │
│  │  │        → on_control_plane_prompt                      │   │    │
│  │  │        → run_one (seq/parallel)                        │   │    │
│  │  │        → after_tool_call hook                         │   │    │
│  │  │      should_stop / prepare_next_turn / queue_drain    │   │    │
│  │  │    }                                                   │   │    │
│  │  └──────────────────────────────────────────────────────┘   │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                     │
│  Lifecycle Events:                                                  │
│    HarnessEvent (SessionStart, Compaction, Branch,                  │
│                 TriggerHandled, TurnEnded, ...)                     │
│    AgentEvent (AgentStart, TurnStart, MessageStart/Update/End,     │
│                ToolExecutionStart/Update/End,                       │
│                ControlPlanePromptResolved)                          │
└─────────────────────────────────────────────────────────────────────┘
```

**设计原则**：
- `Agent` 保持 IO-free——不接触文件系统，只管理内存状态
- `AgentHarness` 负责组装：Session、Compaction、Cost、Trigger
- 所有持久化通过 `SessionStorage` trait 抽象，支持 Memory（测试）和 JSONL（生产）
- 可注入式 hook 体系（before/after tool call, should_stop, prepare_next_turn）允许嵌入方定制行为

## 4. code_walkthrough

### 4.1 `crates/agent/src/lib.rs`
模块声明与 public re-export。核心模块：`agent`、`agent_loop`、`types`，harness 在 `cfg(feature = "harness")` 下。

### 4.2 `crates/agent/src/agent.rs` — Agent 状态机
- `AgentInner` 持有 `Mutex<AgentState>` + listeners + steering/follow_up queue
- `PendingMessageQueue` 支持 `QueueMode::All`（一次全部）或 `OneAtATime`（逐条注入）
- `Agent::prompt()` 入口：`guard_not_streaming() → run_agent_loop`
- `Agent::continue_()` 从当前 transcript 继续（无新用户消息）
- `Agent::abort()` 通过 `CancellationToken` 取消活动 run

### 4.3 `crates/agent/src/agent_loop.rs` — 核心循环
- `run_agent_loop`：设置 `is_streaming = true` → 发射 AgentStart → 追加新消息 → drive_loop → finalize
- `call_llm`：从 state 快照 system_prompt/messages/tools/model → transform_context → convert_to_llm → 通过 stream_fn 调用 pie_ai → 用 `biased select!` 确保 abort 优先于 stream.next()
- `execute_tools`：解析 tool calls → permission_classification → before_tool_call hook → prompt 合并 → 顺序/并行执行 → after_tool_call hook → emit 事件
- `PreparedCall` 枚举：`Run { tool }` / `Blocked { result }`
- `run_one`：执行工具，若 tool 提供 on_update 回调，通过 unbounded mpsc channel + pump task 将 `AgentToolResult` 转为 `AgentEvent::ToolExecutionUpdate`
- `compute_args_hash` / `canonicalize`：canonical JSON SHA-256，用于 prompt 绑定
- `default_prompt_payload`：安全默认 payload，不含原始 args 值，只含 tool_name / args_keys / args_hash

### 4.4 `crates/agent/src/types.rs` — 类型定义
- `AgentMessage`：Llm(Message) | Custom(CustomMessage)，serde untagged
- `AgentTool` trait：definition() / label() / execution_mode() / prepare_arguments() / permission_classification() / execute()
- `AgentState`：system_prompt, model, thinking_level, tools, messages, is_streaming
- `AgentEvent`：完整生命周期事件
- Hook 类型：`BeforeToolCallHook`、`AfterToolCallHook`、`ShouldStopHook`、`PrepareNextTurnHook`、`OnControlPlanePromptHook`
- `StreamFn`、`ConvertToLlm`、`TransformContext`、`GetApiKey`

### 4.5 `crates/agent/src/harness/agent_harness.rs` — Harness 组装层
- `AgentHarnessOptions`：所有组装配置（system_prompt, model, skills, tools, session, compaction, hooks, budget_cap, trigger_runtime, on_turn_end...）
- `AgentHarness::new`：构建 system prompt（base + skills） → 创建 Agent → 挂载 CostTracker listener
- `AgentHarness::prompt`：ensure_session_start_emitted → check_budget_cap → run_auto_compaction → run_turn_with_continuation
- `run_turn_with_continuation`：挂 session listener → 运行 agent.prompt/continue_ → 卸载 listener → finish_persisted_run → OnTurnEndHook → 若 Continue 则循环
- `handle_trigger`：TriggerRuntime::evaluate → run_before_trigger_hook → resolve_trigger_prompt → 审计写入 → spawn_trigger_action
- `spawn_trigger_action`：构建独立 sub-agent harness，在独立 tokio task 中运行
- `move_to`：切换 session leaf → rehydrate_from_session → emit Branch event
- `run_auto_compaction`：从 session.branch 取真实 entry（非内存合成 id） → compact → persist Compaction entry
- `run_evaluator`：创建无 tool 的裸 Agent，用于 /goal 等评估场景

### 4.6 `crates/agent/src/harness/system_prompt.rs`
- `format_skills_for_system_prompt`：将 Skill[] 渲染为 `<skills>` XML 块，含 instruction preamble

### 4.7 `crates/agent/src/harness/session/session.rs` — Session 数据模型
- `SessionTreeEntry`：10 种 tagged variant（Message, ThinkingLevelChange, ModelChange, Compaction, BranchSummary, Custom, CustomMessage, Label, SessionInfo, Leaf）
- `SessionStorage` trait：异步 CRUD + get_path_to_root + find_entries
- `Session` facade：typed append_* 方法 + move_to + build_context
- `build_session_context`：从 parent-chain entries 回放 messages/thinking_level/model，正确处理 Compaction 的 first_kept_entry_id

### 4.8 `crates/agent/src/harness/session/jsonl_repo.rs` — JSONL 仓库
- 文件命名：`<uuidv7>.jsonl`
- create/open/list/delete，路径解析支持绝对/相对路径

### 4.9 `crates/agent/src/harness/compaction/compaction.rs` — Compaction 引擎
- `estimate_tokens` / `estimate_text_tokens`：char-class-aware 估算，图片按 768 tokens
- `should_compact`：80% context_window 阈值
- `find_turn_start_index` / `find_cut_point`：turn-boundary-safe 切点
- `generate_summary`：调用 LLM 产生摘要，支持 prompt budget trimming
- `compact`：prepare → summarize → retry on overflow
- `serialize_conversation`：消息列表 → 紧凑文本

### 4.10 `crates/agent/src/harness/trigger_runtime.rs` — 事件去重/循环引擎
- `TriggerRuntimeConfig`：dedup_window (默认 5min, max 24h) + cycle_hop_limit (默认 5)
- `evaluate(trigger)`：先 prune → dedup 检查 → cycle 检查 → 接受并记录
- `record_follow_up_hop`：前瞻性 hop 计数（用于 tool 产生的子事件）
- `snapshot()` 返回 `TriggerRuntimeSnapshot`，用于 TUI 展示

### 4.11 `crates/agent/src/harness/cost.rs` — 成本追踪
- `CostTracker`：监听 assistant MessageEnd，累加 Usage
- `CostSnapshot`：输入/输出/缓存 token + cost，turn_count
- `one_line_summary` / `full_breakdown`：格式化输出

### 4.12 `crates/agent/src/harness/permission.rs` — 危险命令检测
- `PermissionPolicy`：bash 工具名集合 + danger regex set + predicate rules
- 覆盖：sudo、curl|sh、dd of=/dev/、mkfs、chmod 777、shutdown、git push --force、eval 管道、forkbomb
- rm 单独用 token-aware 检测：识别 -r/-R/--recursive + -f/--force 的任意组合
- 引号剥离防御：`normalize_operand` 去除单层引号 + `${HOME}` 展开

### 4.13 `crates/agent/src/harness/env/native.rs` — 原生执行环境
- `NativeEnv`：基于 tokio::fs + tokio::process
- exec 支持 timeout、abort、stdout/stderr streaming callback
- Unix 下通过 `setsid()` 创建独立进程组，`killpg(SIGKILL)` 杀整个树
- stdout/stderr 并行 drain，避免死锁

## 5. flows

### 5.1 普通交互流（prompt → assistant → stop）

```
用户输入: "list files"
  har.prompt("list files")
    → check_budget_cap: Ok
    → run_auto_compaction: context_tokens < 80% window, skip
    → run_turn_with_continuation(Some(user_msg), ...)
      → make_session_listener → agent.subscribe
      → agent.prompt(user_msg)
        → agent_loop
          → emit AgentStart
          → append user msg, emit MessageStart/End
          → drive_loop
            → emit TurnStart
            → call_llm: state → context → stream_fn → AssistantMessage
              → emit MessageStart(text_delta...) → MessageEnd
            → execute_tools: assistant.content 无 ToolCall, 返回空
            → should_stop_after_turn: 无 hook
            → stop_reason=Stop, continues=false
            → steering queue 为空, 退出 loop
          → finalize: emit AgentEnd
      → unsub listener, finish_persisted_run
      → on_turn_end hook: None → return Ok(())
  → 调用结束
```

### 5.2 Compaction + Resume 流

```
# compaction 触发:
会话积累了大量消息 → prompt 前 estimate > 80% context_window
  har.run_auto_compaction()
    → session.branch(None) 取真实 session entries
    → compact(model, entries, settings)
      → find_cut_point: 从后向前累加 token 到 keep_recent_tokens, 再掉头到 turn boundary
      → prepare_compaction: 收集 entries_to_summarize
      → generate_summary(model, messages, budget)
        → serialize_conversation_for_summary_budget
        → if 超预算: trim_messages_for_summary_budget(从后保留)
        → LLM 调用 → 返回 summary
    → session.append_compaction(summary, first_kept_entry_id, tokens_before)
    → agent.state 插入 compaction_summary Custom 消息

# resume 流程:
  repo.open(session_path) → Session
  har = AgentHarness::new(opts)  # 包含 session
  har.rehydrate_from_session()
    → session.build_context()
      → session.branch(None) → get_path_to_root
      → build_session_context(path_entries)
        → 扫描 ThinkingLevelChange / ModelChange / Assistant.model
        → 遇到 Compaction: 取 summary → compaction_summary msg → 跳过前面已压缩的 entries
        → 收集所有 Message + CustomMessage
    → agent.state 恢复: messages, thinking_level, model
```

### 5.3 Trigger / Sub-agent 流

```
外部事件进入:
  hook → mpsc channel → pump task → har.handle_trigger(trigger)
    → emit TriggerHandlingStart
    → trigger_runtime.evaluate(trigger)
      → prune expired dedup entries
      → dedup check: 同 key 在窗口内 → Deduped
      → cycle check: trace_id hop >= limit → CycleSuppressed
      → Accept: 记录 dedup + cycle, 递增 counters
    → run_before_trigger_hook: 允许/拒绝/需确认
    → resolve_trigger_prompt (若需确认)
    → session.append_custom("trigger", TriggerRecord)
    → emit TriggerHandled
    → if state == Accepted: spawn_trigger_action(trigger)
      → tokio::spawn:
        → resolve action: 通过 before_trigger_action_hook 得到 TriggerAction { prompt, promote, delivery }
        → 构建 sub-harness (MemorySessionStorage, 继承 parent model/tools/thinking)
        → 注册 running_triggers
        → emit TriggerExecutionStarted
        → 运行 sub-harness.prompt(action.prompt)
        → compute_sub_agent_outcome
        → 写入 trigger_result audit entry
        → emit TriggerCompleted / TriggerFailed
        → apply_promotion: PromoteAction 决定是否注入父 session
```

## 6. tests

### 单元测试（crate 内部）

| 测试位置 | 意图 |
|---------|------|
| `agent_loop.rs` | 核心 loop 的 faux-stream 端到端测试：单轮、工具调用循环、before_tool_call veto、parallel 执行、prepare_arguments 标准化、ToolExecutionUpdate 事件、pump 超时不挂死、PermissionClassification 的 Allow/Block/Prompt 全状态、prompt 决议的 deny/allow/合并语义、binding spoofing 防御 |
| `compaction.rs` | 阈值判断、cut_point 转向边界、budget trimming、CJK 截断、overflow 重试、summarizer max_tokens 限制 |
| `permission.rs` | 危险 bash 模式 deny/allow、rm 分类器的 flag 组合、引号绕过防御 |
| `trigger_runtime.rs` | 首次 accept、窗口内 dedup、过期重 admit、cycle suppression、follow_up_hop、snapshot 计数器、窗口 clamp |
| `system_prompt.rs` | 空 skill、渲染格式 |
| `cost.rs` | 累加 usage、reset |
| `native.rs` | exec 正常完成、stdout/stderr 不追加换行、streaming callback、timeout、unix killpg 杀后台子进程、abort、高 stderr 量不阻塞 stdout |

### 集成测试（`crates/agent/tests/`）

| 文件 | 意图 |
|------|------|
| `agent_loop.rs` (test) | 完整 Agent loop 行为：工具执行、hook 交互、PermissionClassification 全套 |
| `harness_e2e.rs` | Harness + Session 端到端：prompt 持久化、session 失败报告、move_to 恢复 thinking_level、skills system prompt、set_model 持久化、模板插值、rehydrate_from_session、harness 事件总线 (SessionStart/Branch)、budget cap、abort 即时解除、cost 累加、panicking listener 隔离、unsubscribe、compaction resume 回归 (#19)、trigger audit 跳过 LLM 流、cut_point 锚定 user message、branch read 失败 fallback |
| `permission_e2e.rs` | PermissionPolicy 通过 harness 拦截 bash 工具 |
| `session_storage.rs` | Memory 和 JSONL 后端的 roundtrip、跨 open 持久化、metadata、leaf 移动 |
| `skills_loader.rs` | 技能加载测试 |
| `templates_loader.rs` | 模板加载测试 |

## 7. risks

### 7.1 未读/未完全实现部分
- `agent_loop.rs` 注释提到的 TODO：`onPayload/onResponse`、`transformContext`、`getApiKey` 声明的 hook 未全部接入
- `getApiKey` hook 声明在 types 中但 agent_loop 未使用（OAuth 短令牌场景未覆盖）
- Trigger 模块注释说 `handle_trigger` 的完整 state machine 在后续 PR 中（当前 Accept 是终端，Running/Completed/Failed 在 sub-PR 中）

### 7.2 边界条件
- `PendingMessageQueue` drain 不是原子操作：多线程并发 enqueue + drain 可能丢失消息（虽然当前单线程 tokio 无此问题）
- `estimate_text_tokens` 的 char-class 估算对混合代码/注释的 token 估算可能有 20-30% 偏差，虽然已有 80% 阈值 + 20% slack 缓解
- `trim_messages_for_summary_budget` 从后向前保留，意味着旧消息优先被丢弃，这可能丢失早期关键上下文
- `Compaction` entry 中的 `first_kept_entry_id` 如果 session 文件被外部修改，resume 可能无法正确定位

### 7.3 可能技术债
- `#[allow(unused_imports)]` 在 `agent_harness.rs:25` 和 `types.rs:670` 的 `_exports_marker` 属于 workaround
- JSONL session 文件没有 compaction/vacuum 机制，持续 append 会无限增长
- `run_one` 中 pump 超时 2s 是硬编码常量，无配置项
- `node.rs` 仅 5 行，是极薄的 re-export 层
- `messages.rs` 仅有 3 个 helper 函数，但在模块结构上独立成文件

### 7.4 安全边界
- `default_prompt_payload` 已保证不泄露原始 args 值，但依赖工具作者的 `before_tool_call` hook 不自行泄露
- `on_control_plane_prompt` 无配置时 fail-closed deny，是正确的默认行为
- eval 和 exec 的进程组 kill 仅在 Unix 实现，Windows 无等效保护

## 8. next_questions

下一轮精读建议聚焦以下问题：

1. **coding-agent 层如何组装 AgentHarness？** 具体 CLI 入口如何创建 session、注册 tools、设置 system prompt、挂载 notification hooks？
2. **Tool 实现的完整契约是什么？** 除了 Bash/Read/Write/Edit 常见工具外，InstallSkill、SkillBuilder 等 control-plane 工具的 `permission_classification` 实现是怎样的？
3. **Trigger 的完整生命周期？** RFC 1 中说的 Accept → Running → Completed/Failed 状态机在 sub-PR 中如何实现？`NotificationHook` trait 的具体 transport 实现（MCP push、cron）在哪里？
4. **Prompt template 和 Skill 的加载路径？** `load_skills` / `load_templates` 在 coding-agent 层如何配置目录优先级（`~/.pie/skills` vs project `.pie/skills`）？
5. **OnTurnEndHook 的 `/goal` 实现？** `TurnEndAction::Continue` 驱动的多轮目标完成循环中，evaluator 的 prompt 是什么样的？如何判断 goal achieved？
6. **JSONL session 的并发安全？** 多进程 pie 实例同时写同一个 session 文件的保护策略？
7. **Windows 支持缺口？** proc group kill、symlink metadata 等在 Windows 上的行为？
