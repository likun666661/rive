# 第三章：Phase 1 Snapshot Evidence 测试计划

## 1. 测试目标

Phase 1 只验证 **Agent Fact Snapshot Evidence MVP**。

测试目标是确认 Rive 能把某个 workspace/path 的现场捕获成稳定、可查询、可引用的 evidence snapshot：

```text
rive CLI
  -> SnapshotCapture
  -> EvidenceWorkspace / SnapshotSource
  -> SnapshotStore + EventStore
  -> evidence_ref / snapshot_ref read model
```

本阶段不测试 Work Graph、dispatch 状态机、PTY runtime、完整 `team send/report/status`，也不要求安装 AgentFS。

## 2. 关键验收线

1. `rive init` 能创建可用 `.rive` workspace。
2. `rive snapshot capture` 能生成 `snapshot_id`、`event_id`、`manifest_hash`、manifest 文件和 blob refs。
3. manifest 记录 path、kind、size、mtime、hash、blob_ref、skipped。
4. capture 写入 `evidence.snapshot_captured` event。
5. `rive snapshot list/show` 和 `rive evidence list` 输出分为 `protocol` 和 `display`。
6. 同一目录变化前后，manifest/hash 差异可解释。
7. 无 AgentFS 安装时完整可用。
8. snapshot/evidence 不推进 dispatch/task/node/graph 业务状态。
9. Snapshot capture 核心逻辑不直接依赖本地文件目录结构。

## 3. 抽象接口测试要求

Phase 1 必须有中间抽象层，例如 `EvidenceWorkspace` / `SnapshotSource`。

上层 `SnapshotCapture` 只能依赖这个接口，不直接依赖 OS 文件系统、`.rive/evidence` 布局或 AgentFS。

接口测试要求：

- 用 fake/in-memory source 测 capture、manifest、hash、skip 逻辑。
- 用 local fs source 做端到端测试。
- Snapshot store/event store 可以独立替换或用临时目录/临时 SQLite 测试。
- 未来 AgentFS adapter 只能实现同一接口，不能绕过 event path。

建议最小接口能力：

```text
list_entries(scope, ignore_rules)
metadata(path)
read_bytes(path)
hash(path)
write_blob(bytes)
write_manifest(manifest)
resolve_ref(ref)
```

`diff` 可以先是可选能力或 no-op，不阻塞 Phase 1。

## 4. 测试分层

### Unit Tests

- ID generation：`snapshot_id`、`event_id` 格式稳定且不重复。
- Manifest hashing：相同 manifest 得到相同 hash；内容变化后 hash 变化。
- Protocol/display split：read model 的控制字段只在 `protocol`，说明字段只在 `display`。
- Skip rules：默认忽略 `.rive/`、`.git/`、`target/`、`node_modules/`、cache/build 目录和超限大文件。
- Error/skip code：不存在路径、无权限、hash 失败、大文件超限都有稳定 code。
- Fake source capture：不接触真实文件系统也能生成 manifest。

### Integration Tests

- `rive init` 创建 `.rive/rive.db`、`.rive/evidence/snapshots/`、`.rive/evidence/blobs/`、占位文件。
- `rive snapshot capture --path <dir>` 生成 manifest、blob refs、event row。
- `rive snapshot show <id>` 能读取 manifest 和 protocol/display 输出。
- `rive snapshot list` 按时间或 id 列出快照。
- `rive evidence list` 能列出 evidence event。
- 修改文件后再次 capture，manifest/hash 能体现差异。
- 删除文件后再次 capture，结果可解释。

### CLI Contract Tests

- 成功命令 exit code 为 0，输出 JSON。
- 失败命令 exit code 非 0，输出稳定 error envelope。
- JSON 输出中 `protocol` 字段可被机器读取，`display` 字段不参与测试断言里的控制流。
- `--json` 或默认 JSON 行为按实现约定固定；不要只输出人类表格。

### Negative Tests

- 未初始化 workspace 执行 capture，返回稳定错误。
- capture `.rive/` 自身时默认 skip，不递归捕获内部 evidence。
- 捕获不存在 path，返回 `not_found` 或等价稳定 code。
- 捕获无权限 path，返回 `permission_denied` 或 skipped entry。
- 大文件超过阈值时不写 blob，manifest 中出现 skipped entry。
- 同一 snapshot 不应重复写同一个 event；若后续加 idempotency，应返回原结果。

## 5. AgentFS 相关测试

Phase 1 不依赖 AgentFS。

必须有一个测试或 CI 环境约束证明：

```text
PATH 中没有 agentfs 时，rive init/capture/show/list 仍然通过。
```

如果实现预留 AgentFS importer/backend seam，只测试接口边界：

- AgentFS backend 缺失时返回 `backend_unavailable`，不影响 local backend。
- AgentFS importer 产出的内容只能成为 `evidence_ref` / `snapshot_ref`。
- AgentFS 文件变化不能直接写 dispatch/task/node/graph 状态。

不要在 Phase 1 测 AgentFS mount/run/sync。

## 6. 不变量

Phase 1 必须守住这些不变量：

- Snapshot 是 evidence，不是 business fact completion。
- `evidence.snapshot_captured` 不能改变 dispatch、task、node、graph projection。
- manifest hash 是后续引用 snapshot 的稳定依据。
- `display` 字段可以变文案，`protocol` 字段必须稳定。
- local fs backend 和 fake source 生成的 manifest 语义一致。

## 7. 建议验收命令

实现完成后，最小人工验收流程：

```bash
tmp=$(mktemp -d)
cd "$tmp"
echo "hello" > a.txt

rive init .
rive snapshot capture --path . --label before
rive snapshot list
rive snapshot show <snapshot-id>

echo "world" >> a.txt
rive snapshot capture --path . --label after
rive snapshot show <second-snapshot-id>

rive evidence list
cargo test
```

检查点：

- 两次 snapshot 的 manifest_hash 不同。
- manifest 中包含 `a.txt` 的 path、size、mtime、sha256、blob_ref。
- `.rive/` 不进入 captured files。
- event store 中有两条 `evidence.snapshot_captured`。
- 输出包含 `protocol` / `display` 两层。

## 8. Done Definition

task #21 完成条件：

- 测试计划已覆盖 Phase 1 范围和明确排除项。
- 实现任务完成后，所有 unit/integration/CLI contract tests 可执行。
- 没有 AgentFS 的环境下测试通过。
- 有 fake source 测试证明抽象接口未绑定本地目录结构。
- 所有 snapshot/evidence 测试都确认不会推进业务状态。
