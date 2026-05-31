# 第一章：Agent 协作协议

## 1. 本章要回答什么

Rive 是一个本地优先的 agent team runtime。它要协调的不是同一个 SDK 里的虚拟 agent，而是 Claude Code、Codex、Gemini CLI、OpenCode 这类已经存在的外部 CLI agent。

这些 agent 对 Rive 来说都是黑盒。Rive 不假设它们共享同一个模型厂商、同一个 SDK、同一个 MCP/ACP 实现，或者同一个内部 agent runtime。Rive v0 能可靠假设的共同能力只有一个：

```text
它们在 shell 里运行，并且能执行 PATH 上的命令。
```

本章定义的协议，就是把这个最弱共同能力变成一个可协作、可审计、可恢复的团队系统：

```text
external CLI agent
  -> `team` CLI ABI
  -> runtime enforcement
  -> Team State Substrate
  -> projections
  -> delivery / recovery / human takeover
```

这不是聊天协议，不是 prompt 约定，也不是 markdown checklist。它的核心职责是回答：

```text
什么动作可以成为 Rive runtime 里的事实？
```

## 2. 要解决的问题

多个 CLI agent 同时跑在多个终端里，并不天然构成一个团队。

如果没有协议，Rive 无法稳定回答这些问题：

- 谁正在做哪件事？
- 任务是真的派出去了，还是只是在某个终端里出现了一段文字？
- worker 是接受了任务、开始了任务、阻塞了，还是只是在自然语言里说了一句？
- 哪一次 dispatch 产生了哪个 artifact？
- 一个 task 为什么 blocked？
- 一个 work node 为什么 ready 或 done？
- runtime 崩溃、agent 丢上下文、人类接管之后，系统要从哪里恢复？

自然语言可以表达意图，但不能作为事实来源。worker 在终端里说“完成了”，不等于 dispatch 完成。orchestrator 在 markdown 里写了一个勾，不等于依赖已经满足。workspace 里出现了一个文件，也不等于 task 已经 done。

Rive 需要协议，是因为 agent 协作的事实、状态、任务图演进和恢复都必须工程化。

## 3. 设计主张

Rive v0 的协议建立在三条主张上。

第一，外部 agent 通过 CLI 进入结构化协议。

```text
black-box CLI agent -> `team` command -> runtime command
```

第二，事实、状态、投递、证据、产物和任务图都进入同一个 Team State Substrate。所有状态都来自同一套 event/projection 体系，而不是散落在终端、markdown、adapter callback 和内存里。

第三，任务拆分和完成不是自然语言结论，而是 Work Graph projection。dispatch 是执行尝试，artifact/evidence 是材料，work node 的 ready/done 必须由 graph facts、completion criteria、review/approval event 推导。

这三条合起来，Rive 要做的是：

```text
commands create events
events create projections
projections explain work
humans and agents act on structured projections
```

## 4. 为什么不是自然语言、MCP 或 markdown

### 自然语言不够

自然语言适合让 agent 理解任务、说明上下文、解释结果。但自然语言不能承担状态迁移、权限校验、幂等、冲突处理和恢复。

如果一个状态只能从“agent 好像说了什么”推出来，它就不是 protocol fact。

### MCP/ACP/A2A 不是 v0 主路径

MCP、ACP、A2A、vendor hooks 都有参考价值，也可以成为未来 adapter。但 Rive v0 的目标是协调异构外部 CLI agent，而不是要求所有 agent 先实现某个共同 agent protocol。

Rive 的原则是：

```text
adapter 可以变化，fact path 不能变化。
```

MCP/ACP/hook 如果存在，也只能编译到同一套 runtime command/event。它们不能新增第二套事实来源。

### Markdown 只能是 projection

`tasks.md` 适合给人和 agent 阅读，但不能成为 source of truth。否则 markdown、SQLite、terminal transcript 会形成多个事实源，恢复和审计都会变得含糊。

如果人类编辑 `tasks.md`，也必须先转换成 runtime 接受的 `human.*` event，才能改变 Work Graph fact。

## 5. 分层架构

Rive 协议分成这些层：

```text
┌────────────────────────────────────────────────────────────┐
│ External CLI Agents                                        │
│ Claude / Codex / Gemini / OpenCode                         │
│ black-box PTY sessions; can execute shell commands          │
└──────────────────────────────┬─────────────────────────────┘
                               │ shell command
                               ▼
┌────────────────────────────────────────────────────────────┐
│ Agent-facing Protocol ABI: `team`                          │
│ fixed commands / fixed args / stdin payload / JSON output   │
│ stable errors / idempotency / event_id / dispatch_id        │
└──────────────────────────────┬─────────────────────────────┘
                               │ structured runtime command
                               ▼
┌────────────────────────────────────────────────────────────┐
│ Runtime Enforcement                                         │
│ schema / auth / role / ownership / state machine / CAS       │
│ idempotency / append-only event write                       │
└──────────────────────────────┬─────────────────────────────┘
                               │ accepted fact event
                               ▼
┌────────────────────────────────────────────────────────────┐
│ Team State Substrate                                        │
│ event ledger / projections / work graph / dispatches         │
│ delivery records / artifact refs / evidence refs / recovery │
└───────────────┬──────────────────────────────┬─────────────┘
                │ projection                    │ delivery side effect
                ▼                               ▼
┌─────────────────────────────┐   ┌──────────────────────────┐
│ Human Control Plane: `hive` │   │ PTY Delivery / Transcript │
│ ps / log / graph inspect    │   │ delivery and evidence     │
│ attach / recover / approve  │   │ not source of truth       │
└─────────────────────────────┘   └──────────────────────────┘
```

## 6. 核心边界

### 6.1 External CLI Agents

外部 agent 是真实 CLI 进程，跑在 PTY 里。Rive 可以启动它们、注入环境变量、把 prompt 写进终端、观察 transcript，但不能拥有它们的内部 reasoning loop。

因此，外部 agent 的自由文本输出不能直接改变 Rive 状态。

### 6.2 `team`: agent-facing ABI

`team` 是给 agent 用的结构化协议入口，不是人类运维工具。

`team` 的要求：

- 固定 subcommand
- 固定参数
- 长文本走 stdin payload
- 输出 JSON
- 稳定 exit code
- 稳定 error code
- 写操作必须带 `command_id` / idempotency key
- 成功返回 `event_id`
- 涉及任务的动作返回 `dispatch_id` 或 `node_id`
- 错误返回 `retryable` 和 `expected_next_action`

agent 决策只能依赖结构化字段，例如：

```text
code
retryable
expected_next_action
event_id
dispatch_id
node_id
node_version
graph_version
projection
```

agent 不能解析自然语言 `message` 来决定控制流。

### 6.3 `hive`: human control plane

`hive` 是给人用的控制面。

人类可以查看团队状态、查看 graph、attach 到某个 agent、cancel、recover、approve 或 reject。但这些动作也不能绕过 runtime。人类动作同样要进入 event ledger。

```text
human action -> `hive` command -> runtime command -> event
```

### 6.4 Team State Substrate

Team State Substrate 是 Rive 的协调事实层。它不是“后台存几张表”，而是整个团队的可查询、可回放、可快照、可迁移状态空间。

它至少包含：

- command events
- dispatch facts
- delivery records
- work graph facts
- projections
- agent/run metadata
- artifact references
- evidence references
- task index projections
- approval/recovery state

只有 runtime 接受的 command event 可以改变 coordination fact。

### 6.5 Delivery and transcript

delivery 是投递事实，不是业务完成事实。

Rive 可以记录：

```text
delivery.requested
delivery.delivered
delivery.failed
```

但 `delivery.delivered` 只表示内容被投递到目标 PTY。它不表示 worker 已接受、已开始、已完成。

terminal transcript 是 evidence。它可以解释发生了什么，但不能直接推进 dispatch 或 work node 状态。

## 7. Fact、Artifact、Evidence

Rive 明确区分三类东西。

| 类型 | 含义 | 是否能直接改变 coordination state |
| --- | --- | --- |
| fact event | runtime 校验通过后写入的事实，例如 dispatch created、report accepted、dependency added、approval granted | 是 |
| artifact_ref | 产物引用，例如 patch、报告、测试输出、文件路径/hash | 否，只能被 fact event 引用 |
| evidence_ref | 证据引用，例如 transcript、diff、prompt injection、raw tool output | 否，只能被 fact event 引用 |

同一个文件可以同时被作为 artifact 和 evidence 引用，但语义不同。

artifact 回答：

```text
产出了什么？
```

evidence 回答：

```text
Rive 为什么相信某件事发生过？
```

它们都不直接回答：

```text
任务是否完成？
```

完成状态必须由 projection 从事实、completion criteria、approval/review 和材料引用中推导。

## 8. Command And Event Path

所有改变状态的动作都走同一条路径：

```text
agent or human action
  -> structured command
  -> schema validation
  -> workspace / actor / token / role auth
  -> idempotency check
  -> ownership / state machine / graph CAS checks
  -> append fact event
  -> attempt delivery if needed
  -> append delivery result event
  -> recompute/update projections
  -> return structured response
```

这条路径是 Rive 的工程边界。

delivery result 也必须是 ledger event。projection 只能从 fact event 和 delivery result event 推导，不能由 delivery adapter 在 ledger 之外保留隐式状态。

没有通过这条路径的内容，可以成为 evidence，但不能成为 coordination fact。

## 9. Work Graph

`team` 解决的是 agent action 如何进入事实层。但 agent team 还需要解决另一件事：任务如何被拆分、依赖如何维护、完成条件如何被强约束。

这就是 Work Graph。

Rive 把它拆成三张相关但不同的图：

```text
Work Graph
  work node + semantic edge + completion constraints

Execution Graph
  dispatch attempts bound to work nodes

Material Graph
  artifact_ref / evidence_ref bound to node / dispatch / event
```

Work Graph 表达工作本身的结构和约束。Execution Graph 表达执行尝试。Material Graph 表达产物和证据。

这三者都在 Team State Substrate 中，但不能混成同一种 edge。

## 10. Work Graph 规则

### Task node 不是 dispatch

task node 描述目标、约束、完成标准、依赖和预期材料。dispatch 是一次由某个 agent 执行的尝试。

一个 task 可以有多次 dispatch：失败、重试、换人、并行 review 都不应该改变 task node 的定义。

### Dispatch 绑定到 node

每次执行尝试必须绑定到一个已有 node，或者通过 runtime-accepted graph mutation event 创建新 node。

dispatch 可以出现在 node projection 中，例如 `active_dispatch_id` 或 `latest_dispatch_ids`，但 dispatch 不属于 work graph topology。

### Work edge 是语义边

v0 的 work edge 应表达工作关系，例如：

```text
decomposes_to
depends_on
validates
supersedes
```

这些边表达的是任务结构和完成约束，不是执行记录。

### Material reference 不是 dependency edge

`artifact_ref` 和 `evidence_ref` 是材料引用，不是 dependency edge。artifact 出现不等于依赖已解，evidence 出现不等于任务完成。

### v0 默认 DAG/all-predecessor 语义

Rive v0 的 Work Graph 默认按 DAG/all-predecessor 语义理解 readiness：一个 node 只有在它的 blocking dependencies、completion prerequisites 和 required validation conditions 都满足后，才可以被 projection 推导为 `ready` 或 `reviewable`。

v0 不让 Work Graph 本身成为循环执行图。循环、迭代、返工和重新尝试通过显式事件表达，例如：

```text
reopen
supersede
retry
split
recover
```

这能保证 `ready`、`blocked`、`done` 的投影语义是可定义、可解释、可重放的。

### Ready 和 done 是 projection

`ready`、`blocked`、`reviewable`、`done` 都不应作为 agent/orchestrator 可自由写入字段。

它们由这些事实推导：

- graph edge
- dependency state
- completion criteria
- dispatch/report event
- artifact/evidence refs
- review/approval event
- human override/recover event

### Graph edit 要版本化

Rive 区分两个版本：

- `node_version`: 保护 node 字段、completion criteria、owner、expected artifacts 等变化
- `graph_version`: 保护 topology 变化，例如 split、merge、edge add/remove、supersede、reopen

修改事件可以携带：

```text
expected_node_version
expected_graph_version
```

这样人类和 orchestrator 并发改图时，runtime 可以通过 CAS 拒绝覆盖。

### 历史不重写

返工、重开、取消、恢复都通过事件表达：

```text
reopen
supersede
retry
cancel
recover
```

Work Graph 是 append-only facts 的 projection，不是可随意改写的计划文本。

## 11. Projection Reason Contract

projection 是协议 read model，不是 UI 细节。

每个 projected node 都应该能回答：

- 当前 protocol state 是什么？
- 这个 state 由哪些 event 推导？
- 还缺哪些 requirement？
- 下一步哪些 action 合法？
- 给人看的解释是什么？

因此所有 read model 都分成两层：

```text
protocol fields
  稳定的 enum / ID / version / boolean / projection 字段
  agent 和 orchestrator 只能依赖这些字段做分支

display fields
  给人读的 title / explanation / summary
  不参与状态迁移、agent 决策或恢复逻辑
```

示例：

```json
{
  "protocol": {
    "node_id": "node_123",
    "state": "blocked",
    "derived_from": ["evt_101", "evt_117"],
    "missing_requirements": [
      {
        "kind": "dependency",
        "node_id": "node_098"
      }
    ],
    "allowed_next_actions": [
      "inspect_dependency",
      "attach_dispatch",
      "recover",
      "cancel"
    ],
    "node_version": 7,
    "graph_version": 3
  },
  "display": {
    "title": "Implement review worker handoff",
    "explanation": "Blocked because dependency node_098 has not reached done."
  }
}
```

如果一个字段还没有稳定 enum、ID、version、CAS 或可重放推导语义，它默认属于 `display`，不能升级成 `protocol`。

这个原则适用于：

- `team` response
- graph projection
- delivery projection
- task index projection
- future TUI/API read models

## 12. 为什么这样设计

这个设计让 Rive 获得几个关键性质。

第一，黑盒 CLI agent 可以参与团队协作。Rive 不需要等待所有 agent 支持同一个 SDK 或工具协议。

第二，协作事实可以恢复。runtime 重启、终端丢失、agent 丢上下文之后，系统可以从 event/projection 里恢复，而不是从终端文本里猜。

第三，自然语言仍然有用，但不会成为隐式数据库。prompt、report message、transcript 都可以作为 payload 或 evidence 存在，但状态迁移只认结构化 event。

第四，人类可以看懂系统为什么处在某个状态。`graph inspect` 不只展示 blocked，还要展示缺哪个 dependency、缺哪个 artifact、缺哪个 review、哪个 dispatch 卡住。

第五，orchestrator 可以自动决策下一步。它不需要读一段解释文案再猜协议含义，而是读取 `allowed_next_actions`、`missing_requirements`、版本号和 projection。

最终，Rive 要把 agent 协作从“多个终端里的对话”变成“可审计的事实系统”。

## 13. 本章冻结什么

本章冻结这些原则：

- external CLI agents 通过 `team` CLI ABI 进入协议
- `team` 是 tool-like structured protocol surface，不是自然语言约定
- human control plane 和 agent protocol plane 分离
- Team State Substrate 是 coordination facts 的唯一来源
- delivery、artifact、evidence 都不能直接替代 fact event
- Work Graph 和 Execution Graph 分离
- Work Graph 的 semantic edge 和 material reference 分离
- ready/done/reviewable/blocked 是 projection，不是自由写入字段
- read model 分为 normative `protocol` fields 和 non-normative `display` fields

本章不冻结这些内容：

- Rust module layout
- database schema
- 完整 command list
- 完整 event taxonomy
- 所有 state machine
- CLI/TUI 展示细节

这些应该进入后续章节。
