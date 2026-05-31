# 第二章：Agent Fact Snapshot Evidence MVP

## 1. 一阶段冻结范围

Rive 一阶段只做一件事：

```text
Agent Fact Snapshot Evidence MVP
```

它不是完整 Work Graph，不是完整 team/orchestrator/worker 闭环，不是 PTY runtime，也不是完整 AgentFS 接入。

一阶段目标是先建立“事实证据账本”：当未来某个 fact event、report、review 或 graph node 需要被复查时，Rive 能引用一份稳定的 agent 工作现场快照。

最小闭环：

```text
workspace/path
  -> snapshot capture
  -> Evidence Workspace Interface
  -> manifest + hashes + blobs
  -> evidence.snapshot_captured event
  -> evidence_ref / snapshot_ref
  -> show/list query
```

## 2. 为什么先做这个

Rive 的长期目标是协调多个外部 CLI agent。但在 dispatch、graph、review 之前，必须先解决一个更底层的问题：

```text
一个 agent fact 背后的现场证据怎么保存？
```

例如：

- agent 当时看到了哪些文件？
- 关键文件的 hash 是什么？
- 哪些文件变了？
- 产物在哪里？
- 这份证据能不能被未来 report/review/graph 引用？
- runtime 重启后还能不能复查？

如果这层不稳，后面的 `team report`、Work Graph `done/reviewable`、human review 都只能依赖自然语言描述。

一阶段先做 evidence substrate，是为了给后续事实系统提供可复查材料。

## 3. AgentFS 的位置

AgentFS 的 `MANUAL.md` 展示了这些能力：

- `agentfs init`
- `agentfs exec`
- `agentfs run`
- `agentfs mount`
- `agentfs fs`
- `agentfs diff`
- `agentfs timeline`
- `agentfs sync`
- `agentfs migrate`

它擅长保存 agent filesystem、overlay diff、sandbox、timeline 和可迁移 SQLite 状态。

但 Rive 一阶段不完整接入 AgentFS，也不重写 `agentfs-cli`。

AgentFS 在 Rive 里的正确位置是：

```text
optional evidence backend / importer
```

不是：

```text
fact source
graph engine
dispatch state machine
```

一阶段必须做到：

- 没有 AgentFS 也能运行。
- 本地 scan/hash/manifest 是默认实现。
- AgentFS 只作为后续 backend seam。
- `agentfs diff/timeline/fs` 未来可以导入成 `evidence_ref`。
- AgentFS 文件变化不能直接改变 dispatch、task、node、graph 状态。

## 4. Evidence Workspace Interface

一阶段必须在 snapshot capture 和底层目录/文件系统之间加抽象接口。上层 capture 逻辑不能直接绑定本地目录结构、`.rive/evidence` 文件布局或 AgentFS。

建议分成三层：

```text
SnapshotCapture
  -> EvidenceWorkspace / SnapshotSource
  -> SnapshotStore / EventStore
```

`SnapshotCapture` 只处理捕获流程、manifest 生成和 hash 规则。

`EvidenceWorkspace` 只提供和底层工作现场交互的能力：

```text
list_entries(scope, ignore_rules) -> entries
metadata(path) -> size / mtime / kind
read_bytes(path) -> stream / bytes
hash(path) -> digest
diff(base_ref?, scope) -> diff_ref / summary?   # v0 可以 no-op
```

`SnapshotStore` 负责写 manifest、blob 和 event：

```text
write_blob(bytes) -> blob_ref
write_manifest(manifest) -> manifest_ref
append_event(evidence.snapshot_captured)
resolve_ref(ref) -> readable location / bytes
```

一阶段默认实现：

```text
LocalFsEvidenceWorkspace
LocalSnapshotStore
SqliteEventStore
```

测试必须额外提供：

```text
FakeEvidenceWorkspace / InMemoryEvidenceWorkspace
```

以证明核心 capture 逻辑不依赖真实文件目录布局。

未来 AgentFS 只需要新增：

```text
AgentFsEvidenceWorkspace
```

它可以从 `agentfs fs/diff/timeline` 读取 evidence，但不能绕过 Rive event path 写 fact。

## 5. 一阶段 CLI

一阶段只需要这些命令。

```text
rive --version
rive init [workspace]
rive snapshot capture [--path <path>] [--label <label>] [--agent <id>] [--dispatch <id>]
rive snapshot list
rive snapshot show <snapshot-id>
rive evidence list
team --version
team self-check
```

`team self-check` 只用于检查 agent-facing ABI 的环境准备情况，不实现 `team send/report/status`。

## 6. Workspace Layout

`rive init` 创建：

```text
.rive/
  rive.db
  run/
  evidence/
    snapshots/
    blobs/
  artifacts/
  tasks.md
  PROTOCOL.md
```

一阶段中，`tasks.md` 和 `PROTOCOL.md` 可以是占位/说明文件，不参与事实推导。

## 7. Snapshot Object

snapshot 是一次证据捕获。

建议字段：

```text
snapshot_id
event_id
workspace_id
agent_id?
dispatch_id?
label?
capture_root
created_at
manifest_path
manifest_hash
file_count
total_bytes
backend = local
```

`dispatch_id` 在一阶段只是可选关联字段，不要求 dispatch 表或 dispatch 状态机存在。

## 8. Manifest

manifest 存储在 `.rive/evidence/snapshots/<snapshot_id>/manifest.json`。

建议结构：

```json
{
  "snapshot_id": "snap_...",
  "event_id": "evt_...",
  "backend": "local",
  "capture_root": "/workspace",
  "created_at": "...",
  "files": [
    {
      "path": "src/main.rs",
      "kind": "file",
      "size": 1234,
      "mtime": "...",
      "sha256": "...",
      "blob_ref": "blobs/ab/cd..."
    }
  ],
  "skipped": [
    {
      "path": "target/debug/app",
      "reason": "ignored"
    }
  ]
}
```

manifest 自身也必须有 hash。未来 fact event 引用的是 `evidence_ref` / `snapshot_id` / `manifest_hash`，而不是自由文本说明。

## 9. Evidence Event

capture 成功后写入 SQLite event。

事件类型：

```text
evidence.snapshot_captured
```

最小 payload：

```json
{
  "snapshot_id": "snap_...",
  "backend": "local",
  "capture_root": "/workspace",
  "manifest_path": ".rive/evidence/snapshots/snap_.../manifest.json",
  "manifest_hash": "sha256:...",
  "file_count": 42,
  "total_bytes": 123456,
  "agent_id": null,
  "dispatch_id": null,
  "label": "before-report"
}
```

这个 event 只创建 evidence fact。它不改变任何业务状态。

## 10. Read Model

所有输出继续沿用 protocol/display 分层。

示例：

```json
{
  "protocol": {
    "snapshot_id": "snap_...",
    "event_id": "evt_...",
    "manifest_hash": "sha256:...",
    "backend": "local",
    "capture_root": "/workspace",
    "file_count": 42,
    "total_bytes": 123456
  },
  "display": {
    "summary": "Captured 42 files from /workspace",
    "label": "before-report"
  }
}
```

agent 或后续 runtime 只能依赖 `protocol` 字段。`display` 只给人读。

## 11. Ignore / Safety Rules

一阶段必须有明确跳过语义。

默认跳过：

- `.rive/`
- `.git/`
- `target/`
- `node_modules/`
- 常见 cache/build 目录
- 超过默认大小阈值的大文件

被跳过的 path 要进入 manifest 的 `skipped` 列表，不能静默消失。

不存在路径、无权限路径、hash 失败、大文件超过阈值，都要有稳定 error/skip code。

## 12. 一阶段不做什么

明确不做：

- Work Graph projection。
- dispatch 状态机。
- `team send/report/status`。
- PTY spawn/attach/delivery。
- AgentFS 完整接入。
- AgentFS mount/run/sync 管理。
- 根据 snapshot 自动判断 done/reviewable。

## 13. 验收标准

一阶段验收：

- `rive init` 能初始化 workspace。
- `rive snapshot capture` 能对指定 path 生成 manifest、hash、blob refs。
- snapshot capture 逻辑只依赖 `EvidenceWorkspace` / `SnapshotSource` 抽象，不直接依赖本地目录结构。
- 有 fake/in-memory workspace 测试核心 capture 逻辑。
- 有 local fs backend 端到端测试。
- `rive snapshot list/show` 能查询 snapshot。
- capture 写入 `evidence.snapshot_captured` event。
- snapshot 输出有 `protocol` / `display` 分层。
- 同一目录变化前后能捕获 hash/manifest 差异。
- 无 AgentFS 安装时完整可用。
- snapshot 不改变 dispatch/task/node/graph 状态。
- 忽略路径、大文件、不存在路径、无权限路径有明确语义。
- `cargo test` 覆盖以上核心规则。

## 14. 后续阶段

只有在一阶段完成后，才进入后续阶段：

1. `team report` 引用 `snapshot_id` / `evidence_ref`。
2. dispatch event 绑定 evidence。
3. Work Graph review/done projection 引用 evidence。
4. PTY transcript capture。
5. AgentFS importer/backend。
