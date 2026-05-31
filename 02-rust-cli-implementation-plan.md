# 第二章：Rust CLI 实现阶段与 AgentFS 策略

## 1. 当前目标

Rive 已经确定第一阶段先做 CLI，不做 Web UI，不做 TUI，不先做 marketplace。

这一阶段的目标不是把所有 runtime 能力一次做完，而是先把协议事实路径跑通：

```text
human -> `rive` CLI -> runtime command -> Team State Store -> projection
agent -> `team` CLI -> runtime command -> Team State Store -> projection
```

从本章开始，命令命名收敛为：

- `rive`: human control plane，给人类操作和观察。
- `team`: agent-facing ABI，给外部 CLI agent 写事实。

早期讨论里的 `hive` human CLI 语义，在 Rive 实现里对应 `rive`。

## 2. AgentFS 结论

AgentFS 的 `MANUAL.md` 展示的核心能力是 agent filesystem：

- `agentfs init`: 创建 agent filesystem，可以带 copy-on-write base。
- `agentfs exec`: 在 mounted AgentFS 中执行命令。
- `agentfs run`: 在 sandbox + copy-on-write filesystem 中运行程序。
- `agentfs mount`: 挂载 filesystem。
- `agentfs fs`: 对 AgentFS 数据库做文件读写。
- `agentfs diff`: 查看 overlay changes。
- `agentfs timeline`: 查看 tool call audit log。
- `agentfs sync/migrate`: 同步和迁移数据库。

这些能力解决的是：

```text
agent 的文件系统状态、工具轨迹、sandbox、snapshot、sync 怎么管理
```

它不直接解决：

```text
agent team/work graph 的事实、权限、状态迁移、projection、恢复怎么约束
```

所以 Rive v0 不完整接入或重写 `agentfs-cli`，也不把 AgentFS 作为事实存储主路径。

Rive v0 自己实现最小 Team State Store。AgentFS 只作为后续 optional adapter：

```text
AgentFS can be workspace/artifact/evidence backend.
AgentFS cannot be fact source.
```

## 3. AgentFS Wrapper 边界

如果后续 wrap AgentFS，只允许出现在三个位置。

### 3.1 Workspace sandbox backend

Rive 可以用 `agentfs run` 或 `agentfs exec` 为某个 worker 提供 copy-on-write workspace。

```text
rive agent start reviewer --workspace-backend agentfs
```

这只影响 worker 的文件系统环境，不改变 Rive 的事实规则。

### 3.2 Artifact/evidence importer

Rive 可以读取 AgentFS 的 diff、timeline 或 filesystem path，把它们登记成 `artifact_ref` 或 `evidence_ref`。

```text
agentfs diff <session>      -> evidence_ref
agentfs timeline <session>  -> evidence_ref
agentfs fs cat <path>       -> artifact_ref/evidence_ref
```

但 artifact/evidence 登记仍然必须通过 Rive runtime command 写入 event。

### 3.3 Export/sync backend

Rive 可以把 artifact/evidence namespace 导出到 AgentFS 或 Turso sync 体系，用于迁移、复现或分享。

这属于 storage/export，不属于协议事实入口。

## 4. 禁止的 AgentFS 用法

这些路径 v0 明确禁止：

- agent 直接通过 `agentfs fs write` 改变 Rive dispatch 状态。
- AgentFS timeline 自动关闭 Rive dispatch。
- AgentFS diff 自动让 Work Graph node 进入 `done`。
- AgentFS 文件写入绕过 `team report` / `rive approve` / `rive recover`。
- Rive projection 从 AgentFS 私有状态直接推导 protocol state。

AgentFS 输出可以成为 evidence。只有 Rive runtime 接受的 event 可以成为 fact。

## 5. Rust CLI 分阶段实现

### Phase 1: Repo 和 CLI 骨架

目标：建立 Rust workspace 和最小 CLI 可运行骨架。

建议结构：

```text
Cargo.toml
crates/
  rive-core/
  rive-cli/
  team-cli/
```

初始命令：

```text
rive --version
rive init [path]
rive status
team --version
team self-check
```

`rive init` 创建：

```text
.rive/
  rive.db
  run/
  artifacts/
  evidence/
  tasks.md
  PROTOCOL.md
```

验收：

- `cargo test` 通过。
- `rive init` 可重复运行，重复运行给出稳定结果或明确错误。
- `team self-check` 能检查 `RIVE_WORKSPACE`, `RIVE_AGENT_ID`, `RIVE_AGENT_TOKEN` 等环境变量并输出 JSON envelope。

### Phase 2: Team State Store

目标：实现最小 SQLite-backed Team State Store。

最小表：

```text
events
idempotency_keys
agents
runs
dispatches
deliveries
work_nodes
work_edges
artifact_refs
evidence_refs
```

最小能力：

- append event
- idempotency lookup
- projection recompute/read
- JSON response envelope
- protocol/display read model split

验收：

- 同一个 `command_id` 重放返回同一结果。
- event append 后 projection 可重放得到同一状态。
- `display` 字段变化不影响 protocol projection。

### Phase 3: CLI Protocol Loop Without PTY

目标：先跑通没有 PTY 的协议闭环。

Human CLI:

```text
rive agent add <name> --role orchestrator|worker
rive send <worker> --node <node-id> --message ...
rive ps
rive log
rive dispatch inspect <dispatch-id>
```

Agent CLI:

```text
team list
team report --dispatch <id> --status done|blocked|failed|review --stdin
team status --state idle|working|blocked|error --stdin
```

这个阶段的 delivery 可以先记录为 `delivery.not_configured` 或 `delivery.manual`，不启动真实 PTY。

验收：

- worker 不能 `team send`。
- orchestrator 不能 `team report` worker dispatch。
- report 必须绑定 open dispatch。
- terminal/free-text 不改变状态。
- 错误 envelope 包含 `code`, `retryable`, `expected_next_action`, `projection?`。

### Phase 4: Work Graph CLI

目标：实现 Work Graph v0 的最小对象和 projection reason contract。

命令：

```text
rive graph node add
rive graph edge add --type depends_on|decomposes_to|validates|supersedes
rive graph inspect <node-id>
rive tasks
```

规则：

- v0 默认 DAG/all-predecessor readiness。
- cycle/返工/迭代用 `reopen/supersede/retry/split/recover` event 表达。
- dispatch 绑定 node，但不是 work edge。
- artifact/evidence 可以引用 node/dispatch/event，但不自动完成 node。

验收：

- blocking dependency 未 done 时，node projection 是 `blocked`。
- 缺 artifact/review/approval 时，node projection 能返回 `missing_requirements`。
- `allowed_next_actions` 是枚举。
- `display.explanation` 不参与状态判断。

### Phase 5: Runtime / PTY Delivery

目标：启动真实外部 CLI agent，并把 `team` 注入 PATH/env。

命令：

```text
rive run
rive agent start <name> --cmd <cmd>
rive agent attach <name>
rive agent stop <name>
```

能力：

- local runtime socket
- per-run agent token
- PTY spawn/write/attach
- delivery.requested / delivered / failed event
- agent startup protocol reminder

验收：

- delivery result 是 ledger event。
- delivery failed 不回滚 business fact。
- attach 只写 human attach event，不改变 dispatch 业务状态。
- runtime 重启能从 Team State Store 恢复 projection。

### Phase 6: AgentFS Adapter

目标：在核心 CLI/state 稳定后再接 AgentFS。

初始 adapter 只做：

```text
rive agent start <name> --workspace-backend agentfs
rive artifact import --from-agentfs <session>
rive evidence import --from-agentfs-timeline <session>
```

验收：

- 没有安装 AgentFS 时，Rive core 功能完全可用。
- AgentFS diff/timeline 只能生成 artifact/evidence refs。
- AgentFS 文件写入不能绕过 runtime 改变 Work Graph projection。

## 6. 立即拆分的任务

第一轮只做 Phase 1 到 Phase 3 的 CLI MVP。Phase 4 作为紧随其后的 graph milestone。Phase 5/6 先不阻塞 CLI/state 骨架。

执行顺序：

```text
1. Rust workspace + rive/team CLI skeleton
2. SQLite Team State Store + response envelope
3. no-PTY team/rive protocol loop
4. Work Graph CLI + projection reason contract
5. PTY runtime/delivery
6. AgentFS adapter
```

