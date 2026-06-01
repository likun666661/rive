# 第十三章：Phase 6 Codex Runner 测试计划

## 1. 测试目标

Phase 6 只验证 **Codex Dispatch Runner MVP + Runner Adapter Boundary**。

测试目标是确认 Rive 的 runner 抽象不只适用于 OpenCode，也能驱动 Codex：

```text
rive runner codex
  -> real/fake Codex process
  -> rive snapshot capture
  -> team status/report
  -> dispatch projection
  -> codex hook debug trace
```

本阶段不测试 Work Graph，不测试 `team send`，不测试 agent-to-agent delegation，不测试 PTY attach。

## 2. 关键验收线

1. `rive runner opencode` 的 Phase 5 行为不能回退。
2. runner core 和 adapter boundary 要清楚：业务规则在 core，vendor 启动差异在 adapter。
3. `rive runner codex` 能创建或复用 worker agent。
4. Codex child process env 必须包含 `RIVE_WORKSPACE`、`RIVE_AGENT_ID`、`RIVE_AGENT_TOKEN`、`RIVE_RUN_ID`、`RIVE_DISPATCH_ID`。
5. runner 必须确保 Codex hook trace 可用，且不修改用户全局 Codex config。
6. Codex 调 `team status` 时，dispatch 不关闭。
7. Codex 调 `team report done|blocked|failed` 后，dispatch projection 进入对应状态。
8. runner success 只能基于 dispatch projection，不能基于 stdout、final answer 或 hook trace。
9. `command_id` replay 不二次启动 Codex。
10. @samuel 和 @jian 都需要用真实本机 Codex 跑端到端。

## 3. Unit Tests

### Adapter selection

- `runner opencode` 使用 OpenCode adapter。
- `runner codex` 使用 Codex adapter。
- adapter kind 出现在 response protocol。
- adapter-specific missing binary code 正确：`opencode_not_found` / `codex_not_found`。
- adapter-specific exit failed code 正确：`opencode_exit_failed` / `codex_exit_failed`。

### Shared runner core

这些测试应该对 OpenCode/Codex 都成立：

- workspace 未 init 返回 `workspace_not_initialized`。
- agent 不存在时创建 worker agent。
- agent 已存在且无 `--agent-token` 返回 `runner_agent_token_required`。
- agent 已存在且 token 错误返回 `agent_token_invalid`。
- dispatch `command_id` replay 不二次启动 child。
- stdout-only/final-answer-only 不算成功，返回 `dispatch_not_reported`。
- timeout 返回 adapter-specific timeout code。
- response 保持 `protocol/display` 分层。
- token 不出现在 display summary、stdout/stderr path display 或 prompt debug output。

### Codex prompt

- prompt 包含 dispatch id、task title/body、snapshot instruction。
- prompt 包含 `team status` / `team report` 的完整命令形状。
- prompt 明确自然语言 final answer 不等于 Rive report。
- `--snapshot-path` 存在时，prompt 包含 exact capture path。
- 不传 `--snapshot-path` 时，prompt 要求 Codex capture created/modified files。

### Codex command builder

fake/unit 层至少验证：

- command uses `codex exec` or selected current local Codex invocation shape。
- command includes hook feature flag / trust override when `--trust-project` is enabled。
- command does not mutate global `~/.codex/config.toml`。
- PATH prefix includes current `rive` / `team` binary directory。

## 4. Integration Tests with Fake Codex

测试应提供 fake `codex` binary/script，通过 `--codex-bin` 注入。fake binary 不依赖真实 Codex 安装。

### Happy path

fake Codex 行为：

1. 校验 `RIVE_*` env 存在。
2. 写一个结果文件。
3. 调 `rive snapshot capture --path <result-file> --label fake-codex-result`。
4. 调 `team status --dispatch "$RIVE_DISPATCH_ID" --snapshot <snapshot_id> --command-id fake-codex-status-$RIVE_RUN_ID --stdin`。
5. 调 `team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot <snapshot_id> --command-id fake-codex-report-$RIVE_RUN_ID --stdin`。
6. 可选调 `rive debug trace ingest --adapter codex-hook --stdin` 写 fake Codex hook event。
7. exit 0。

验收：

- runner exit 0。
- `protocol.runner.kind == "codex"`。
- dispatch state 是 `reported`，`latest_report_status=done`。
- facts 中有 status 和 report。
- snapshot 存在，manifest/hash 可查。
- 如果 fake 写了 trace，`debug trace list --adapter codex-hook` 可查。
- 没有 Work Graph 表或事件。

### Status-only path

fake Codex 只调用 `team status`，不调用 `team report`，exit 0。

验收：

- runner 返回 `dispatch_not_reported`。
- dispatch 仍是 open。
- status fact 存在。

### Stdout-only path

fake Codex stdout 打印：

```text
I completed the task successfully.
```

但不调用 `team report`。

验收：

- runner 返回 `dispatch_not_reported`。
- dispatch 仍 open。
- stdout/final text 不关闭 dispatch。

### Replay path

重复同一 `rive runner codex --command-id <same>` + 同一 stdin：

- 第一次启动 fake Codex。
- 第二次不启动 fake Codex。
- 第二次 response 中 `child_executed=false`。
- fake invocation count 保持 1。

同一 command_id + 不同 stdin：

- 返回 `idempotency_conflict`。

### Blocked / failed

fake Codex 调：

```text
team report --status blocked
team report --status failed
```

验收：

- dispatch projection 对应 blocked / failed。
- runner response 以 dispatch projection 为准。

## 5. Regression Tests for OpenCode

因为 Phase 6 要重构 runner core，必须重跑 Phase 5 关键用例：

- OpenCode happy path。
- OpenCode stdout-only -> `dispatch_not_reported`。
- OpenCode replay 不二次启动 child。
- OpenCode existing-agent plaintext token requirement。
- OpenCode missing binary。

这些回归可以复用 Phase 5 tests，但必须在 Phase 6 PR 里继续通过。

## 6. Real Codex E2E

@samuel 和 @jian 都要跑真实 Codex。不能只靠 fake Codex。

建议流程：

```bash
tmp=$(mktemp -d)
cd "$tmp"
rive init .

cat <<'EOF' | rive runner codex \
  --agent real-codex-worker \
  --title "create phase6 codex result file" \
  --command-id phase6-real-codex-1 \
  --snapshot-path phase6-codex-result.txt \
  --timeout-seconds 300 \
  --trust-project \
  --stdin
Create phase6-codex-result.txt with exactly this line:
RIVE_PHASE6_CODEX_OK

Capture a Rive snapshot for phase6-codex-result.txt, send one team status update,
then team report done for the dispatch.
EOF
```

检查点：

```bash
test -f phase6-codex-result.txt
grep RIVE_PHASE6_CODEX_OK phase6-codex-result.txt

rive dispatch list
rive fact list
rive evidence list
rive debug trace list --adapter codex-hook
```

必须确认：

- dispatch state 是 `reported`，report status 是 `done`。
- status/report facts 存在并绑定 dispatch。
- snapshot 包含 `phase6-codex-result.txt`。
- Codex debug trace 能看到 real hook events。
- Codex stdout/final answer 没有参与业务状态判断。
- `debug_trace_*` 没有污染 evidence/fact/dispatch 的事实来源。
- 没有 Work Graph 表或事件。

## 7. Codex Hook / Trust Tests

Phase 6 需要特别验证 Codex 的真实运行条件：

- `rive runner codex` 会安装 workspace-local Codex hook。
- one-run config 包含 hook feature flag。
- one-run config 包含 workspace trust override when `--trust-project` is set。
- 不修改用户全局 `~/.codex/config.toml`。
- hook ingest failure does not change Codex process behavior。
- hook trace labels include enough correlation if env is available: `agent_id`, `run_id`, `dispatch_id`。

如果当前本机 Codex 版本要求不同 flag 名，测试记录实际版本和兼容处理。

## 8. Business-State Non-Mutation Tests

Phase 6 继承 Phase 4/5 的边界：

```text
trace != evidence
trace != fact
trace != dispatch transition
trace != task/work graph state
stdout/final answer != report
```

测试要求：

- runner 前后 `debug_trace_*` 可增长。
- business `events/facts/dispatches/snapshots` 只因显式 `snapshot capture` / `team status/report` 变化。
- trace payload 不出现在 evidence refs。
- `rive fact list`、`rive dispatch list`、`rive evidence list` 不展示 trace event。
- no `work_%` tables。
- no `%pty%` tables。

## 9. PR / Review 验收

Phase 6 PR 需要包含：

- 设计/测试计划已在 `docs/` 下。
- `RunnerAdapter` 或等价边界已落地。
- `rive runner opencode` 无回归。
- `rive runner codex` 自动测试覆盖 fake happy path、stdout-only、replay、token/missing binary。
- `cargo fmt --check` 通过。
- `cargo test` 通过。
- `cargo clippy --all-targets -- -D warnings` 通过。
- @samuel 亲自跑过 real Codex closed loop。
- @jian 独立跑过 real Codex closed loop，并在 PR 留测试结论。

只有 fake Codex 通过，不算 Phase 6 完整闭环。
