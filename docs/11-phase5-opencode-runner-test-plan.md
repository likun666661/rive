# 第十一章：Phase 5 OpenCode Runner 测试计划

## 1. 测试目标

Phase 5 只验证 **OpenCode Dispatch Runner MVP**。

测试目标是确认 Rive 可以用一个命令启动真实或 fake OpenCode，注入 `RIVE_*` 协议环境，驱动一次 dispatch，并在 OpenCode 调用 `team status/report` 后，从 dispatch ledger 判断结果。同时，Phase 4 debug trace 只能作为内部调试材料，不能变成业务状态来源。

```text
rive runner opencode
  -> real/fake OpenCode process
  -> rive snapshot capture
  -> team status/report
  -> dispatch projection
  -> debug trace query
```

本阶段不测试 Work Graph，不测试 `team send`，不测试 PTY attach，不把 trace 当 evidence/fact/dispatch source。

## 2. 关键验收线

1. `rive runner opencode` 能创建或复用 worker agent。
2. runner 能创建 dispatch，并把 dispatch id 注入 OpenCode prompt/env。
3. child process env 必须包含 `RIVE_WORKSPACE`、`RIVE_AGENT_ID`、`RIVE_AGENT_TOKEN`、`RIVE_RUN_ID`、`RIVE_DISPATCH_ID`。
4. runner 必须确保 OpenCode trace plugin 已安装或可用。
5. OpenCode 调用 `team status` 时，dispatch 不关闭。
6. OpenCode 调用 `team report done|blocked|failed` 后，dispatch projection 进入对应状态。
7. runner success 只能基于 dispatch projection，不能基于 stdout、final answer 或 trace。
8. OpenCode 退出 0 但没有 `team report` 时，runner 返回 `dispatch_not_reported`，dispatch 保持 open。
9. trace 只写 `debug_trace_*` 和 debug payload/run files，不写 evidence/fact/dispatch/task/graph。
10. @jian 需要用真实本机 OpenCode 做独立端到端验收。

## 3. Unit Tests

### Runner prompt

- prompt 包含 dispatch id、task title/body、protocol commands、snapshot instruction。
- `--snapshot-path` 存在时，prompt 包含具体 capture 命令。
- 不传 `--snapshot-path` 时，prompt 要求 OpenCode capture created/modified files。
- prompt 明确写出：自然语言 final answer 不等于 Rive report。

### Runner environment

- env builder 包含所有 `RIVE_*`。
- PATH 前缀包含当前 `rive` / `team` 所在目录。
- token 不出现在 display summary。
- `RIVE_RUN_ID` 稳定传入 child process，并可用于 trace correlation。

### Agent resolution

- agent 不存在时，runner 创建 worker agent 并获得 one-time token。
- agent 已存在且未传 `--agent-token`，返回 `runner_agent_token_required`。
- agent 已存在且 token 错误，返回 `agent_token_invalid`。
- runner 不持久化明文 token。

### Response contract

- 成功 response 有 `protocol.runner`、`protocol.agent`、`protocol.dispatch`、`protocol.trace`。
- 失败 response 有稳定 `code`、`retryable`、`expected_next_action`。
- `display.summary` 不参与测试控制流。

## 4. Integration Tests with Fake OpenCode

测试应提供 fake `opencode` binary/script，通过 `--opencode-bin` 注入。fake binary 不依赖真实 OpenCode 安装。

### Happy path

fake OpenCode 行为：

1. 校验 `RIVE_*` env 存在。
2. 写一个结果文件。
3. 调 `rive snapshot capture --path <result-file> --label fake-opencode-result`。
4. 调 `team status --dispatch "$RIVE_DISPATCH_ID" --snapshot <snapshot_id> --command-id fake-status-1 --stdin`。
5. 调 `team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot <snapshot_id> --command-id fake-report-1 --stdin`。
6. 可选调 `rive debug trace ingest --adapter opencode-plugin --stdin` 写一条 fake trace。
7. exit 0。

验收：

- runner exit 0。
- dispatch state 是 `reported`，`latest_report_status=done`。
- facts 中有 status 和 report。
- snapshot 存在，manifest/hash 可查。
- 如果 fake 写了 trace，`debug trace list` 可查。
- 没有 Work Graph 表或事件。

### Status does not close

fake OpenCode 只调用 `team status`，不调用 `team report`，exit 0。

验收：

- runner 返回 `dispatch_not_reported`。
- dispatch 仍是 open。
- status fact 存在。

### Blocked / failed report

fake OpenCode 分别调用：

```text
team report --status blocked
team report --status failed
```

验收：

- dispatch projection 对应 `blocked` / `failed`。
- runner response 以 dispatch projection 为准。

### OpenCode exits non-zero

fake OpenCode exit 42。

验收：

- runner 返回 `opencode_exit_failed`。
- stdout/stderr debug run files 可查。
- 如果未 report，dispatch 仍 open。

### Missing team report after successful final text

fake OpenCode stdout 打印类似：

```text
I finished the task successfully.
```

但不调用 `team report`。

验收：

- runner 仍返回 `dispatch_not_reported`。
- stdout 文本不关闭 dispatch。

### Idempotency

重复同一 runner `--command-id` + 同一 stdin：

- 不创建第二个 dispatch。
- 返回同一 dispatch projection，或稳定 replay response。

同一 `--command-id` + 不同 stdin：

- 返回 `idempotency_conflict`。

## 5. Negative Tests

| Case | Expected |
| --- | --- |
| workspace 未 init | `workspace_not_initialized` |
| `--opencode-bin` 不存在 | `opencode_not_found` |
| existing agent 没有 `--agent-token` | `runner_agent_token_required` |
| wrong existing token | `agent_token_invalid` |
| fake OpenCode report 用 invalid snapshot | team command 失败；runner 最终 `dispatch_not_reported` |
| fake OpenCode report 非 assigned dispatch | `dispatch_not_reported` 或 child failure；dispatch 不被错误 actor 关闭 |
| timeout | `opencode_timeout`，dispatch 状态由 ledger 决定 |

## 6. Debug Trace Boundary Tests

Phase 5 仍然继承 Phase 4 的边界：

```text
trace != evidence
trace != fact
trace != dispatch transition
trace != task/work graph state
```

测试要求：

- runner 前后 `debug_trace_*` 可增长。
- runner 前后业务 `events/facts/dispatches/snapshots` 只因显式 `snapshot capture` / `team status/report` 变化。
- trace payload 不出现在 evidence refs。
- `rive fact list`、`rive dispatch list`、`rive evidence list` 不展示 trace event。
- 如果 trace plugin ingest 失败，OpenCode 行为不应被 trace adapter 改变。

## 7. Manual Real OpenCode E2E

@jian 做独立验收时必须跑真实本机 OpenCode，不只跑 fake binary。

建议流程：

```bash
tmp=$(mktemp -d)
cd "$tmp"
rive init .

cat <<'EOF' | rive runner opencode \
  --agent real-opencode-worker \
  --title "create phase5 result file" \
  --command-id phase5-real-opencode-1 \
  --snapshot-path phase5-result.txt \
  --timeout-seconds 300 \
  --stdin
Create phase5-result.txt with exactly this line:
RIVE_PHASE5_OPENCODE_OK

Capture a Rive snapshot for phase5-result.txt, send one team status update,
then team report done for the dispatch.
EOF
```

检查点：

```bash
test -f phase5-result.txt
grep RIVE_PHASE5_OPENCODE_OK phase5-result.txt

rive dispatch list
rive fact list
rive evidence list
rive debug trace list --adapter opencode-plugin
```

必须确认：

- dispatch state 是 `reported`，report status 是 `done`。
- status/report facts 存在并绑定 dispatch。
- snapshot 包含 `phase5-result.txt`。
- debug trace 能看到真实 OpenCode prompt、tool calls、`team status/report` 命令输出和 final message。
- `debug_trace_*` 没有污染 evidence/fact/dispatch 的事实来源。
- 没有 Work Graph 表或事件。

## 8. PR / Review 验收

Phase 5 PR 需要包含：

- 设计/测试计划已在 `docs/` 下。
- 自动测试覆盖 fake OpenCode happy path 和关键失败路径。
- `cargo fmt --check` 通过。
- `cargo test` 通过。
- `cargo clippy --all-targets -- -D warnings` 通过。
- @samuel 亲自跑过 real OpenCode closed loop。
- @jian 独立跑过 real OpenCode closed loop，并在 PR 留测试结论。

只有 fake payload 或 fake OpenCode 通过，不算 Phase 5 完整闭环。
