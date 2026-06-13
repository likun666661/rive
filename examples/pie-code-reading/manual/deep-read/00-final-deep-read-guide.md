# pie 精读技术手册总纲

> **仓库**: `pie`
> **阅读基线**: `f1c35a3`
> **深度档位**: `maintainer`
> **生成时间**: 2026-06-13
> **上游产物**: 01–09 号精读报告

---

## 1. executive_summary

本轮对 pie 仓库 (`f1c35a3`) 进行了 maintainer 级别的全栈精读，覆盖 9 个子系统，产出核心结论如下：

### 1.1 整体评估

| 维度 | 评级 | 说明 |
|------|------|------|
| **架构设计** | ✅ 优秀 | append-only JSONL + parent-pointer DAG、统一 Event/Content 模型、Hook 可插拔扩展点、纯文本标签协议——均为务实、可维护的设计选择 |
| **Provider 抽象** | ✅ 良好 | 5 个核心 Provider 的流式协议被统一为 `AssistantMessageEvent`，抽象层清晰 |
| **自动化安全** | ⚠️ 中高风险 | Sub-agent 全权限继承、TriggerRuntime 纯内存去重重启丢失、Dynamic Trigger promotion 仍走已弃用的不安全路径 |
| **数据完整性** | ⚠️ 中等风险 | JSONL 写入无原子保证、无行级容错、Sidecar 与主文件无一致性校验 |
| **测试覆盖** | ❌ 不足 | 「跨 Provider tool call 解析一致性测试」完全缺失，LSP 集成测试几乎空白，Session 超大文件场景无覆盖 |

### 1.2 本轮识别的最关键问题（Top 5）

1. **子代理工具白名单缺失**（安全 P0）：子代理继承父代理全部工具（Bash/FileWrite/NewCronJobTool），不同来源的 trigger 无法授予不同权限。
2. **JSONL 无截断恢复**（数据 P0）：崩溃导致最后一行不完整时，整个 session 无法打开。
3. **跨 Provider tool call 一致性零覆盖**（质量 P0）：5 个核心 Provider 使用各自独立的流解析路径，无任何交叉验证测试。
4. **Compaction 缺少自动触发集成**（功能 P1）：超过 context window 无自动防线，依赖外部显式调用。
5. **`PromoteSummaryWhenSummaryContains` 已弃用但仍在线上**（安全 P1）：Dynamic Trigger 的 promotion 仍走 free-form 子串匹配路径，结构化路径 `PromoteSummaryWhenResultDetailsMatch` 因 `TriggerResultDetailsBuilder` 未接入而无实际作用。

---

## 2. architecture_lessons

从本次精读覆盖的 9 个子系统中提炼的架构模式与设计经验：

### 2.1 日志驱动状态（Log-Driven State）

**来源**：Session 分支模型 (03)、Trigger 状态机 (01)、Goal 状态机 (06)

**模式**：不维护可变状态指针，而是通过追加日志条目（append-only）来编码所有状态变更。当前状态通过日志重放得出。

```
Session Leaf = 重放所有 JSONL 行得出
Trigger 审计 = 每个阶段追加一条 Custom entry
Goal 状态 = 扫描最新一条 "goal_state" entry
```

**优点**：
- 崩溃恢复零成本（状态在文件中，不在内存中）
- 多 reader 安全（各自重放）
- 历史不可篡改（追加不覆盖）

**代价**：
- 重放成本随日志增长线性上升
- 全量内存加载（`load_entries()` 读全文件到 `Vec`）
- 无收缩机制（compaction 只追加不删除）

### 2.2 Hook 可插拔扩展点（Pluggable Hook Extension）

**来源**：Goal Evaluator (06)、LSP Integration (04)、Trigger 权限钩子 (01)

**模式**：核心运行时（AgentHarness）不内置业务逻辑，而是通过 hook trait object 将控制权交给外层。Hook 自身无状态，决策通过返回值传递。

```
OnTurnEndHook → Continue | Stop | Pause | Noop
AfterToolCallHook → 增量修改 tool result content
BeforeTriggerHook → Allow | Deny | Prompt
BeforeTriggerActionHook → TriggerAction { delivery, promote }
```

**优点**：
- 单一 harness 支持 goal、code review、lint 修复等多种扩展
- 钩子可组合（当前仅支持单 hook，但架构已为链式扩展留好空间）

**当前限制**：
- `after_tool_call` 的 content 替换语义是全量替换，多个 hook 组合时最后一个胜出
- `on_turn_end` 只支持一个 hook（`Option<OnTurnEndHook>`）

### 2.3 统一事件流（Unified Event Stream）

**来源**：Tool Call 解析矩阵 (05)

**模式**：所有 Provider 的异构流式协议被统一映射为 `AssistantMessageEvent` 枚举。每个 Provider 维护一个 `partial: AssistantMessage`，随事件逐步填充。

**关键设计决策**：
- `content_index` 作为并行内容块的位置索引
- `ToolCallDelta` 透传原始 JSON 片段（不做改写，消费者可做增量渲染）
- 各 Provider 的 JSON 累积策略不同（Anthropic/Bedrock 在 stop 时解析；OpenAI Completions 在流结束时统一解析；Google Gemini 无需累积——原子对象）

**已知差异点**：
- OpenAI Responses 使用 `rposition()` 定位最后 ToolCall（不支持并行）
- Google Gemini 在三种 delta 事件之外同时推送合成 ID

### 2.4 传输无关信封（Transport-Agnostic Envelope）

**来源**：Trigger 状态机 (01)

**模式**：`Trigger` 结构体是统一的"事件信封"，不与任何传输层耦合。传输适配器（`McpNotificationHook`、`CronNotificationHook`、`DynamicTriggerCheckHook`）负责从各自协议构造统一的 envelope。

**关键约束**：
- `idempotency_key` 必需（缺失则反序列化失败）
- `replacement_policy` 必需（决定去重策略）
- `trace_id` 用于循环抑制和审计链路追踪

### 2.5 纯文本协议（Plain-Text Protocol）

**来源**：Loop/Inbox 闭环 (02)

**模式**：模型输出通过纯文本标签（`<loop-state>`, `<inbox>`）传递结构化信息，而非 API/RPC。标签提取失败不导致运行失败。

**设计取舍**：
- ✅ 跨模型通用（任何模型可遵循）
- ✅ 无 Provider 依赖
- ❌ 无法区分"模型产出"与"模型讨论协议语法"
- ❌ 截断容错依赖 `rfind` 策略（取最后出现的标签块）

### 2.6 隔离子代理（Isolated Sub-Agent）

**来源**：Trigger 执行 (01)、Goal Evaluator (06)

**模式**：子代理使用独立的 `MemorySessionStorage`，完成后丢弃。与父代理通过以下方式通信：
- prompt 注入（action prompt → 子代理）
- 标签提取（`<loop-state>`, `<inbox>`）
- audit entry 回写（`trigger_result`, `trigger_promotion`）

**安全缺口**：子代理继承父代理完整工具集和系统提示——无 per-trigger-source 的权限隔离。

---

## 3. deep_read_index

本轮 9 份精读报告的阅读路线与适用场景：

| 编号 | 报告 | 适用场景 |
|------|------|---------|
| **01** | Trigger 状态机 | 理解自动化触发管道、去重/循环抑制、权限钩子、三种交付模式；改 `handle_trigger()` 或 `spawn_trigger_action()` 之前 **必须阅读** |
| **02** | Loop/Inbox 闭环 | 理解 stateful cron、标签提取协议、inbox 生命周期；改 `cron_harness_listener()` 或 inbox 写入逻辑之前 **必须阅读** |
| **03** | Session 分支模型 | 理解 append-only JSONL + parent DAG + compaction；改 `JsonlSessionStorage` 或 `SessionTreeEntry` 之前 **必须阅读** |
| **04** | LSP 集成报告 | 理解 `LspSupervisor` + `LspClient` 架构、after_tool_call hook 集成、诊断等待策略；改 LSP 配置或工具反馈注入逻辑时 **参考阅读** |
| **05** | Tool Call 解析矩阵 | 跨 5 个 Provider 的 streaming tool call 解析路径对比、`parse_partial_json` 机制；新增 Provider 或调试 tool call 解析 bug 时 **必须阅读** |
| **06** | Goal Evaluator | 理解 `OnTurnEndHook` 基础设施、evaluator 隔离子代理、continuation cap 双层上限；改 `/goal` 命令或 turn-end 决策逻辑时 **必须阅读** |
| **07** | 自动化安全审计 | Trigger + Loop 的威胁模型、攻击面、缓解措施矩阵；做安全加固或权限设计评审时 **必须阅读** |
| **08** | Session 完整性审查 | Session/JSONL/Sidecar 的数据完整性风险、修复建议优先级矩阵；处理 session 数据丢失 bug 或设计持久化改进时 **必须阅读** |
| **09** | Provider 一致性测试计划 | 11 个 Provider 的 conformance matrix、mock server 设计、P0/P1/P2 测试用例清单；搭建 Provider 测试体系时 **必须阅读** |

### 3.1 阅读顺序建议

```
新人入门路线:
  03 Session → 01 Trigger → 02 Loop/Inbox → 06 Goal → 05 Tool Call → 04 LSP

安全评审路线:
  01 Trigger → 02 Loop/Inbox → 07 安全审计

质量提升路线:
  05 Tool Call → 09 测试计划 → 08 完整性审查
```

### 3.2 关键源码 ↔ 报告映射

| 源码位置 | 对应报告 | 重要程度 |
|----------|---------|---------|
| `crates/agent/src/harness/trigger.rs` | 01 | 🔴 核心 Envelope |
| `crates/agent/src/harness/trigger_runtime.rs` | 01, 07 | 🔴 去重/循环抑制 |
| `crates/agent/src/harness/agent_harness.rs` | 01, 06 | 🔴 handle_trigger + run_turn_with_continuation |
| `crates/coding-agent/src/triggers/cron.rs` | 01, 02 | 🔴 Cron + 标签提取 |
| `crates/coding-agent/src/inbox.rs` | 02 | 🟡 Inbox 持久化 |
| `crates/agent/src/harness/session.rs` / `jsonl_storage.rs` | 03, 08 | 🔴 Session 模型 |
| `crates/coding-agent/src/lsp_supervisor.rs` / `lsp.rs` | 04 | 🟡 LSP 集成 |
| `crates/ai/src/providers/anthropic.rs` etc. | 05, 09 | 🔴 Provider 解析 |
| `crates/ai/src/json_parse.rs` | 05, 09 | 🟡 Partial JSON |
| `crates/coding-agent/src/goal.rs` | 06 | 🟡 Goal 业务逻辑 |
| `crates/agent/src/harness/compaction/` | 03, 08 | 🟡 Compaction |
| `crates/coding-agent/src/session_archive.rs` | 03, 08 | 🟡 Export/Import |

---

## 4. risk_register

按严重度降序排列，绑定文件路径与证据来源。

### 🔴 严重（可能导致数据丢失、安全失控或功能崩溃）

| R-01 | 子代理工具白名单缺失 |
|------|---------------------|
| **描述** | 子代理（`run_trigger_action`）继承父代理全部工具（Bash、FileWrite、NewCronJobTool），不同来源的 trigger 无法区分权限。恶意 MCP 服务器推送的自定义通知可触发拥有完整 Bash 权限的子代理。 |
| **文件** | `crates/agent/src/harness/agent_harness.rs:2630-2645` |
| **证据** | 07 号报告 §3.4；代码行 `sub_state.tools = parent_tools` |
| **建议** | 为 `TriggerAction`/`BeforeTriggerActionHook` 增加 `allowed_tools` 白名单 |

| R-02 | JSONL 截断无恢复机制 |
|------|---------------------|
| **描述** | 进程在 `append_entry` 写入过程中崩溃导致最后一行不完整时，`load_entries` 报 `Corrupted` 错误，整个 session 无法打开。 |
| **文件** | `crates/agent/src/harness/session/jsonl_storage.rs:110-113` |
| **证据** | 08 号报告 §5.1 |
| **建议** | 若最后一行无 `\n` 且解析失败，截断该行后继续加载（并 warn） |

| R-03 | Header 行单点故障 |
|------|------------------|
| **描述** | JSONL header 行（第 1 行）损坏导致整个 session 无法打开，丢失所有对话历史。 |
| **文件** | `crates/agent/src/harness/session/jsonl_storage.rs:73-81` |
| **证据** | 08 号报告 §2.1 |
| **建议** | 增加 header 恢复逻辑或备份机制 |

| R-04 | Feedback Loop 放大攻击 |
|------|----------------------|
| **描述** | 子代理可通过继承的 `NewCronJobTool` 创建新 cron job（分配新 `trace_id`），绕过 `cycle_hop_limit=5` 限制。可导致资源耗尽和成本放大。 |
| **文件** | `crates/coding-agent/src/triggers/cron.rs:70-278` |
| **证据** | 07 号报告 §3.2 |
| **建议** | Per-source 速率限制 + 子代理工具白名单 |

### 🟡 中等（可能导致功能异常、数据不一致或安全漏洞）

| R-05 | `PromoteSummaryWhenSummaryContains` 已弃用但在线 |
|------|------------------------------------------------|
| **描述** | Dynamic trigger 的 promotion 仍走 free-form 子串匹配路径。模型可通过改写输出绕过条件。结构化路径 `PromoteSummaryWhenResultDetailsMatch` 因 `TriggerResultDetailsBuilder` 未接入而 fail-closed 为 `Null`。 |
| **文件** | `crates/coding-agent/src/triggers/dynamic.rs:577-588`, `crates/agent/src/harness/agent_harness.rs:2722` |
| **证据** | 07 号报告 §3.3 |
| **建议** | 完成 `TriggerResultDetailsBuilder` 接入，移除弃用路径 |

| R-06 | TriggerRuntime 重启去重丢失 |
|------|--------------------------|
| **描述** | `dedup` 和 `cycle` map 为纯内存。进程重启后清空，重启前 Accept 过的事件若在 `dedup_window` 内被重推，将重新执行。 |
| **文件** | `crates/agent/src/harness/trigger_runtime.rs:191-244` |
| **证据** | 07 号报告 §3.1 |
| **建议** | 可选的持久化去重存储（至少对 `Drop` 策略的 key） |

| R-07 | 默认 Allow 策略 |
|------|----------------|
| **描述** | `AgentHarness` 未配置 `before_trigger` hook 时，所有 trigger 自动 `Accepted`。开发者容易遗漏配置。 |
| **文件** | `crates/agent/src/harness/agent_harness.rs:1313-1316` |
| **证据** | 07 号报告 §3.5 |
| **建议** | 增加 `require_before_trigger_hook` 配置项，改为 default-deny |

| R-08 | Sidecar 与 JSONL 无一致性校验 |
|------|----------------------------|
| **描述** | Sidecar 文件（triggers/Cron/endpoints）与主 JSONL 之间无 checksum 或引用字段。Sidecar 丢失后 `automation_counts` 静默退化为零，用户不知自动化规则已丢失。 |
| **文件** | `crates/coding-agent/src/session/mod.rs:298,304` |
| **证据** | 08 号报告 §2.4 |
| **建议** | 在 metalog/header 中添加 sidecar checksum |

| R-09 | Compaction 缺少自动触发集成 |
|------|--------------------------|
| **描述** | Compaction 依赖外部显式调用 `compact()`，不在 AgentHarness event loop 中。超过 context window 无自动防线。 |
| **文件** | `crates/agent/src/harness/compaction/compaction.rs:635-699` |
| **证据** | 08 号报告 §3.3 |
| **建议** | 在 `run_turn_with_continuation` 中集成自动触发 |

| R-10 | 全量内存加载超大 Session |
|------|------------------------|
| **描述** | `load_entries()` 将整个 JSONL 文件读入 `Vec<SessionTreeEntry>`。超长 session（10000+ 轮）可能 OOM。 |
| **文件** | `crates/agent/src/harness/session/jsonl_storage.rs:101-105` |
| **证据** | 08 号报告 §5.3（量化估算） |
| **建议** | 增量加载 / 路径截断（仅加载 leaf→root 路径上的条目） |

| R-11 | OpenAI Responses 并行 Tool Call 不可靠 |
|------|--------------------------------------|
| **描述** | 使用 `rposition()` 定位最后一个 `ToolCall` block，不支持多个并行 function_call 的正确 index 映射。 |
| **文件** | `crates/ai/src/providers/openai_responses.rs`（`function_call_arguments.delta` 处理） |
| **证据** | 05 号报告 §4.2, §5.2 |
| **建议** | 改为 index-based 映射 |

| R-12 | LSP Server 未安装静默跳过 |
|------|------------------------|
| **描述** | `spawn()` 失败时返回 `None`，LSP 诊断静默丢失，用户/LLM 不知道 LSP 未生效。 |
| **文件** | `crates/coding-agent/src/lsp_supervisor.rs:138-141` |
| **证据** | 04 号报告 §8.1 |
| **建议** | 至少 generate 一条 warning 消息注入 tool result |

| R-13 | Evaluator 成本不透明 |
|------|---------------------|
| **描述** | `run_evaluator()` 的 LLM 调用不归入父 `CostTracker`。`/cost` 显示与实际消耗不符。 |
| **文件** | `crates/agent/src/harness/agent_harness.rs:1974-2018` |
| **证据** | 06 号报告 §6.4 |
| **建议** | 为 evaluator 增加轻量级 CostTracker |

### 🟢 低（边缘场景或渐进恶化）

| R-14 | Inbox 跨进程 set_status last-writer-wins |
|------|----------------------------------------|
| **文件** | `crates/coding-agent/src/inbox.rs:115-129` |
| **证据** | 02 号报告 §6.3；07 号报告 §4.3 |

| R-15 | Loop State 跨进程并发写 |
|------|------------------------|
| **文件** | `crates/coding-agent/src/triggers/cron.rs:746-769` |
| **证据** | 02 号报告 §6.4 |

| R-16 | 标签注入：外部数据含 `<inbox>` 标签被误提取 |
|------|-------------------------------------------|
| **文件** | `crates/coding-agent/src/triggers/cron.rs:819-845` |
| **证据** | 07 号报告 §4.2 |

| R-17 | Cron `running_trace_id` 崩溃残留误清除 |
|------|--------------------------------------|
| **文件** | `crates/coding-agent/src/triggers/cron.rs:310-320` |
| **证据** | 01 号报告 §8.2 |

| R-18 | Dynamic Trigger `fire_once` 竞态 |
|------|---------------------------------|
| **文件** | `crates/coding-agent/src/triggers/dynamic.rs`（`mark_rules_fired` 调用路径） |
| **证据** | 01 号报告 §8.2 |

| R-19 | Inbox 无自动清理，append-only 增长 |
|------|----------------------------------|
| **文件** | `crates/coding-agent/src/inbox.rs` |
| **证据** | 02 号报告 §8.1；07 号报告 §4.5 |

| R-20 | `append_label` TOCTOU |
|------|----------------------|
| **文件** | `crates/agent/src/harness/session/session.rs:530` |
| **证据** | 08 号报告 §2.1 |

---

## 5. recommended_followups

以下任务可直接转换为 Rive DAG 节点，按优先级分层：

### 🔴 P0 — 阻塞级（应立即执行）

| 编号 | 任务 | 输入报告 | 预计工作量 | 关键文件 |
|------|------|---------|-----------|---------|
| F-01 | **子代理工具白名单实现** — 在 `TriggerAction` 中增加 `allowed_tools` 字段，在 `BeforeTriggerActionHook` 层按 source 过滤工具列表 | 07, 01 | 3–5 天 | `agent_harness.rs`, `trigger.rs` |
| F-02 | **JSONL 截断恢复** — `load_entries` 中检测最后一行不完整时截断并 warn，而非报 `Corrupted` | 08 | 1–2 天 | `jsonl_storage.rs` |
| F-03 | **跨 Provider tool call 一致性测试** — 实现 `mock_server.rs` + P0 测试用例（TC-P0-01 ~ TC-P0-05） | 05, 09 | 3–5 天 | `crates/ai/tests/` |

### 🟡 P1 — 重要（建议本里程碑完成）

| 编号 | 任务 | 输入报告 | 预计工作量 |
|------|------|---------|-----------|
| F-04 | **完成 Dynamic Trigger promotion 迁移** — 接入 `TriggerResultDetailsBuilder`，移除 `PromoteSummaryWhenSummaryContains` | 07, 01 | 3–5 天 |
| F-05 | **Compaction 自动触发集成** — 在 `run_turn_with_continuation` 或 `build_context` 中集成自动 compact | 08, 03 | 2–3 天 |
| F-06 | **Sidecar 一致性校验** — 在 metalog/header 中记录 sidecar checksum，`automation_counts` 失败时 warn | 08 | 2–3 天 |
| F-07 | **TriggerRuntime 可选持久化去重** — 对 `ReplacementPolicy::Drop` 的 key 实现持久化存储 | 07, 01 | 3–5 天 |
| F-08 | **Session 增量加载** — 为 `get_entries` 添加 offset/limit，至少为 `build_context` 实现路径截断 | 08 | 3–5 天 |
| F-09 | **P1 Provider 一致性测试** — 并行 tool call、reasoning 流、usage 统计、abort/retry 扩展 | 09 | 3–5 天 |
| F-10 | **Evaluator 成本归因** — 为 `run_evaluator` 增加轻量级 CostTracker，在 `/cost` 中显示 | 06 | 1–2 天 |
| F-11 | **OpenAI Responses 并行 tool call 修复** — `rposition()` → index-based mapping | 05 | 1–2 天 |

### 🟢 P2 — 优化（可延后）

| 编号 | 任务 | 输入报告 |
|------|------|---------|
| F-12 | 删除原子性（收集所有待删除路径，失败时统一报告） | 08 |
| F-13 | `ActivateTriggers::Ask` 交互式导入实现 | 08 |
| F-14 | Inbox append 去重 + CAS 写入 + 自动清理 | 02, 07 |
| F-15 | LSP 健康检查 + 自动重连 | 04 |
| F-16 | Default-Deny 策略（`require_before_trigger_hook`） | 07 |
| F-17 | Per-source 速率限制 | 07 |
| F-18 | Loop State 完整性校验 + 文件锁 | 02, 07 |
| F-19 | Memory ↔ JSONL 行为一致性参数化测试 | 08 |
| F-20 | Provider 真实网络 Smoke Test（weekly CI） | 09 |

---

## 6. appendix

### 6.1 上游产物完整性检查

| 编号 | 产物文件 | 存在 | 行数 | 质量评估 |
|------|---------|------|------|---------|
| 01 | `01-trigger-state-machine.md` | ✅ | 558 | 完整，含状态转移图、副作用表、测试清单 |
| 02 | `02-loop-inbox-internals.md` | ✅ | 534 | 完整，含并发分析、解析边缘矩阵 |
| 03 | `03-session-branch-model.md` | ✅ | 464 | 完整，含 DAG 示例、compaction 时序 |
| 04 | `04-lsp-integration-report.md` | ✅ | 493 | 完整，含完整时序图、性能分析 |
| 05 | `05-tool-call-parsing-matrix.md` | ✅ | 409 | 完整，含 6 维对比表、25 个 Fuzz Case |
| 06 | `06-goal-evaluator-internals.md` | ✅ | 577 | 完整，含完整时序图、误判风险矩阵 |
| 07 | `07-automation-security-audit.md` | ✅ | 345 | 完整，含威胁模型、信任边界图、缓解措施矩阵 |
| 08 | `08-session-integrity-review.md` | ✅ | 394 | 完整，含不变式推导、量化估算、优先级建议 |
| 09 | `09-provider-conformance-test-plan.md` | ✅ | 720 | 完整，含 conformance matrix、mock server 设计、附录 fixture |

**完整性结论**：9 份报告全部存在，格式一致，内容覆盖各自子系统的主要代码路径、测试覆盖、风险和建议。未发现重大遗漏。

### 6.2 跨报告矛盾与缺失点

| 编号 | 类型 | 描述 |
|------|------|------|
| D-01 | **缺失** | Session 模型 (03) 未覆盖 `MemorySessionStorage` 的内部实现细节和与 `JsonlSessionStorage` 的行为差异对比 |
| D-02 | **缺失** | Tool Call 矩阵 (05) 未覆盖 Mistral、Azure、Codex、Vertex、Cloudflare 5 个非核心 Provider 的解析路径 |
| D-03 | **缺失** | LSP 报告 (04) 未覆盖 `textDocument/didChange` 增量同步可行性评估和 `didClose` 清理方案 |
| D-04 | **缺失** | Goal Evaluator (06) 未覆盖 evaluator 使用不同模型时的行为分析（当前注释为"使用当前 active model"） |
| D-05 | **不一致** | Trigger 报告 (01) 提到 `before_trigger_action_hook` 的 `promote` 路径使用已弃用 API；安全审计 (07) 同步标注。但两报告对迁移时间线的估计不一致（"downstream PRs" vs "Tools-MCP's follow-up PR"） |
| D-06 | **不一致** | Cron `running_trace_id` 不一致恢复：Trigger 报告 (01) §8.2 标注为"边界 bug"，Loop 报告 (02) §8.1 标注为"低风险—状态泄漏"。两者评估等级不同但场景相同 |
| D-07 | **未覆盖领域** | 本轮未阅读的子系统包括：Skills 加载机制、MCP 客户端连接管理、REPL TUI 渲染引擎、Config 管理、Session 生命周期 hooks（`LifecycleHook`）、`after_tool_call` hook 链式组合器设计 |

### 6.3 人工复核建议

以下部分需要人工确认（代码中无确定答案或有多种解释）：

| 编号 | 复核项 | 来源报告 | 说明 |
|------|--------|---------|------|
| H-01 | Sub-agent cost 归因计划 | 01 §9 | `trigger_result.cost_usd = null`，5b/5c PR 计划如何实现？ |
| H-02 | AgentDelegate 完整实现 | 01 §9 | RFC 2 多代理拓扑何时落地？当前占位代码的安全模型？ |
| H-03 | Maker/Checker Phase 3 设计 | 02 §9 | 是否有自动评估 findings 的计划？ |
| H-04 | Inbox Web/Mobile 消费路径 | 02 §9 | Web relay 场景下的 inbox 同步策略？ |
| H-05 | Compaction 触发时机 | 03 §9 | 在哪个 hook 点调用 `compact()`？当前是否有集成计划？ |
| H-06 | Session DAG 可视化 | 03 §9 | `/tree` 命令是否在 roadmap 中？ |
| H-07 | `after_tool_call` hook 链式组合 | 04 §9 | 是否有 hook chain 设计？ |
| H-08 | Evaluator 模型可配置性 | 06 §9 | 是否允许指定独立 evaluator model？ |
| H-09 | 多 goal 支持 | 06 §9 | 是否支持多个并行 goal check-list？ |
| H-10 | MCP server 身份认证程度 | 07 §6 | 防止恶意 server 伪造 `server_name` 的机制？ |
| H-11 | `ToolCallIdNormalizer` 设计方向 | 05 §9 | 是否移到 Provider trait 层？ |
| H-12 | Bedrock SigV4 签名路径 | 05 §9 | 当前仅支持 Bearer token，是否阻碍使用？ |

### 6.4 术语对照

| 中文 | 英文 / 代码标识 |
|------|----------------|
| 状态机 | `TriggerState` enum (瞬时态/终态) |
| 去重窗口 | `dedup_window: Duration` (默认 5min) |
| 循环抑制 | `cycle_hop_limit: u32` (默认 5) |
| 权限钩子 | `BeforeTriggerHook` / `BeforeTriggerDecision` |
| 动作钩子 | `BeforeTriggerActionHook` → `TriggerAction` |
| 子代理 | Sub-Agent (独立 `MemorySessionStorage`) |
| 交付模式 | `TriggerDelivery::SubAgent` / `InjectSummary` / `InjectAndRun` |
| 结果回写 | `apply_promotion()` = `PromoteAction` |
| 状态回写 | `PromoteAction::None` / `PromoteSummaryNow` / `PromoteSummaryWhenResultDetailsMatch` |
| 标签协议 | `<loop-state>` / `<inbox>` 块 |
| 上下文压缩 | Compaction (`Compaction` entry + `firstKeptEntryId`) |
| 分支摘要 | BranchSummary (`BranchSummary` entry + `fromId`) |
| 叶子节点 | Leaf (`current_leaf()` = 日志重放结果) |
| 会话分叉 | Fork (`get_entries_to_fork()`) |
| 导出/导入 | `.piesession` (tar + manifest + SHA-256) |
| 轮结束钩子 | `OnTurnEndHook` → `TurnEndAction::Continue/Stop/Pause/Noop` |
| 评估器 | Evaluator (无工具, 独立 in-memory 子代理) |
| 连续上限 | `MAX_CONTINUATIONS = 8` (软) / `turn_continuation_cap = 25` (硬) |
| 目标状态 | `GoalStatus::Pursuing/Paused/Achieved/BudgetLimited/Cleared` |
| 偏 JSON 解析 | `parse_partial_json()` / `close_partial()` |
| 累积缓冲区 | `tool_arg_buffers: HashMap<usize, String>` (Anthropic) / `BTreeMap<u64, ToolAccum>` (Completions) |
| TOML 配置 | `~/.pie/lsp.toml` + `.pie/lsp.toml` (LSP) / `.cron.toml` (Cron) |
| 动态触发器 | `DynamicTriggerRule` + `DynamicTriggerRegistry` |
| 命名空间隔离 | `mcp:{server_name}:` 前缀 + `custom:` 段 |
| ID 合成 | Google Gemini: `{name}_{timestamp}_{counter}` |

### 6.5 元数据

- **生成工具**: OpenCode (Rive dispatch `disp_94fb3b96103044f1b9d0f2d307eb9204`)
- **节点 ID**: `work_8d42af0ee4b04f719ea7ebee811b9499`
- **输入基线**: `f1c35a3`
- **输入报告数**: 9
- **输出文件**: `00-final-deep-read-guide.md`
- **总覆盖率**: Trigger 状态机 · Loop/Inbox 闭环 · Session 分支模型 · LSP 集成 · Tool Call 解析 · Goal Evaluator · 自动化安全 · Session 完整性 · Provider 测试计划
