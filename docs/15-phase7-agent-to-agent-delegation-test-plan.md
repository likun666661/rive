# Phase 7 Agent-to-Agent Delegation 测试计划

## 1. 测试目标

Phase 7 验证 **Agent-to-Agent Delegation MVP**。

测试目标是确认真实 orchestrator CLI agent 可以通过 `team send --wait` 把任务派给真实 worker CLI agent：

```text
orchestrator agent
  -> team send --to worker --runner opencode|codex --wait
  -> Rive dispatch ledger
  -> worker runner
  -> team status/report
  -> structured result back to orchestrator
```

本阶段不测试 Work Graph，不测试 PTY，不测试 daemon，不测试 async queue。

## 2. 关键验收线

1. `team send` 是 agent-facing structured CLI，不能是人类运维命令。
2. 只有 role=`orchestrator` 的 agent 能调用 `team send`。
3. target 必须是已注册 worker。
4. worker child 使用 run-scoped token，不暴露长期 token。
5. `team send --wait` 成功只看 child dispatch projection。
6. stdout、final answer、Codex/OpenCode trace 都不能让 delegation 成功。
7. `command_id` replay 不二次启动 worker child。
8. `team send` response 必须是 `protocol/display` 分层，agent 决策只依赖 `protocol`。
9. Phase 5 OpenCode runner 和 Phase 6 Codex runner 不能回退。
10. @samuel 和 @jian 都要跑真实 agent-to-agent e2e。

## 3. Unit Tests

### Actor auth

- missing `RIVE_WORKSPACE` 返回 `missing_workspace_env` 或现有等价错误。
- missing `RIVE_AGENT_ID` 返回 `missing_agent_env` 或现有等价错误。
- unknown actor 返回 `agent_not_found`。
- wrong token 返回 `agent_token_invalid`。
- actor role=`worker` 调 `team send` 返回 `agent_role_not_allowed`。
- actor role=`orchestrator` 可以进入 send flow。

### Target validation

- target name/id 不存在返回 `target_agent_not_found`。
- target role=`orchestrator` 返回 `target_role_invalid`。
- unsupported runner 返回 `runner_not_supported`。
- missing `--wait` 返回 `wait_required`。
- missing `--command-id` 返回 stable missing command id error。
- empty stdin 返回 stable empty task body error。

### Run-scoped token

- `team send` 为 worker run 创建 run-scoped token hash。
- child env 包含 `RIVE_AGENT_ID`、`RIVE_AGENT_TOKEN`、`RIVE_RUN_ID`、`RIVE_DISPATCH_ID`。
- child 可以用 run-scoped token 调 `team status/report`。
- wrong run token 被拒绝。
- `team send` response 不包含 worker token。
- display/stdout/stderr/debug paths 不泄漏 token。

### Idempotency

- same actor + same command_id + same request replay 返回同一 delegation/dispatch。
- replay 不二次启动 child。
- replay response 中 `child_executed=false` 或等价 protocol 字段。
- same command_id + different body 返回 `idempotency_conflict`。
- same command_id + different target/runner/title 返回 `idempotency_conflict`。
- dispatch create 内部 command id 不与 runner 或 fact command id 冲突。

### Result semantics

- child calls `team report --status done` -> send returns ok with dispatch `reported/done`。
- child calls `team report --status blocked` -> send returns structured blocked result。
- child calls `team report --status failed` -> send returns structured failed result。
- child only calls `team status` -> send returns `dispatch_not_reported` and dispatch stays open。
- child prints success to stdout but never reports -> `dispatch_not_reported`。
- child trace contains final answer but no report -> `dispatch_not_reported`。
- runner exit non-zero maps to adapter-specific exit error unless dispatch was already reported and policy says projection wins.

## 4. Fake Integration Tests

Use fake runner adapters or fake `opencode`/`codex` binaries to keep CI deterministic.

### Happy path: fake orchestrator to fake worker

Setup:

1. `rive init`
2. `rive agent add orch --role orchestrator`
3. `rive agent add worker --role worker`
4. Run `team send` with orchestrator env.

Fake worker behavior:

1. Assert `RIVE_*` env exists.
2. Write result file.
3. Run `rive snapshot capture --path result.txt --label phase7-fake-worker`。
4. Run `team status --dispatch "$RIVE_DISPATCH_ID" --snapshot <snapshot_id> --command-id ... --stdin`。
5. Run `team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot <snapshot_id> --command-id ... --stdin`。

Assertions:

- `team send` exits 0。
- response protocol has `action=team.send`。
- `delegation.source_agent_id == orch`。
- `delegation.target_agent_id == worker`。
- `dispatch.state == reported`。
- `report.status == done`。
- evidence snapshot exists and is referenced by report fact。
- no Work Graph or PTY tables/events exist。

### No-report path

Fake worker exits 0 and prints:

```text
I am done.
```

but never calls `team report`。

Assertions:

- `team send` returns `dispatch_not_reported`。
- dispatch remains open。
- stdout is saved for debug only。
- no report fact exists。

### Replay path

Run same `team send --command-id same-id` twice.

Assertions:

- fake worker invocation count is 1。
- second response references same dispatch。
- second response marks child not re-executed。

Then run same command id with different body.

Assertions:

- returns `idempotency_conflict`。
- no second dispatch。

### Role and target failures

- worker actor calling `team send` is rejected。
- unknown target rejected。
- orchestrator target rejected。
- unsupported runner rejected。
- `--wait` omitted rejected。

## 5. Regression Tests

Phase 7 touches runner core and team CLI, so existing phases must keep passing:

- Phase 1 snapshot tests。
- Phase 2 fact/evidence tests。
- Phase 3 dispatch/status/report tests。
- Phase 4 debug trace tests。
- Phase 5 OpenCode runner tests。
- Phase 6 Codex runner tests。

Specific runner regressions:

- `rive runner opencode` happy path still passes。
- `rive runner codex` happy path still passes。
- both runner replay paths still do not re-execute child。
- stdout-only remains `dispatch_not_reported`。
- Codex isolated `CODEX_HOME` behavior remains unchanged。

## 6. Real E2E: Codex Orchestrator -> OpenCode Worker

This is required. It cannot be replaced by fake payloads.

Suggested setup:

```bash
tmp=$(mktemp -d)
cd "$tmp"
rive init .

ORCH_JSON=$(rive agent add orch-codex --role orchestrator)
WORKER_JSON=$(rive agent add worker-opencode --role worker)
ORCH_ID=$(...)
ORCH_TOKEN=$(...)

rive debug trace install codex --workspace "$tmp"
rive debug trace install opencode --workspace "$tmp"
```

Launch real Codex as orchestrator with env:

```text
RIVE_WORKSPACE=$tmp
RIVE_AGENT_ID=$ORCH_ID
RIVE_AGENT_TOKEN=$ORCH_TOKEN
RIVE_RUN_ID=<orchestrator-run-id>
```

Prompt:

```text
Use `team send --to worker-opencode --runner opencode --title ... --command-id ... --wait --snapshot-path phase7-codex-to-opencode.txt --stdin`
to ask the worker to create phase7-codex-to-opencode.txt with exactly:
RIVE_PHASE7_CODEX_TO_OPENCODE_OK

After `team send` returns, summarize only the protocol result.
```

Assertions:

- OpenCode worker creates the result file。
- child dispatch state is `reported`。
- latest report status is `done`。
- status/report facts exist。
- snapshot includes result file。
- OpenCode debug trace exists for worker run。
- Codex debug trace shows orchestrator called `team send`。
- success is based on dispatch projection, not Codex final answer。
- no Work Graph/PTY tables/events exist。

## 7. Real E2E: OpenCode Orchestrator -> Codex Worker

This is also required.

Setup mirrors section 6:

- orchestrator agent role=`orchestrator`。
- worker agent role=`worker`。
- real OpenCode process runs as orchestrator。
- `team send --runner codex --wait` launches Codex worker。

Prompt should ask worker to create:

```text
phase7-opencode-to-codex.txt
RIVE_PHASE7_OPENCODE_TO_CODEX_OK
```

Assertions:

- Codex worker dispatch is `reported/done`。
- result file exists with exact content。
- snapshot evidence exists。
- status/report facts exist。
- Codex hook trace exists for worker run。
- OpenCode trace shows orchestrator called `team send`。
- global `~/.codex/config.toml` hash remains unchanged。
- no Work Graph/PTY side effects。

## 8. Debug Trace Boundary

For every fake and real e2e:

- query `debug_trace_*` to confirm trace rows exist when adapter supports them。
- query `events/facts/snapshots/dispatches` to confirm trace data is not copied into business facts。
- `rive fact list` must not show raw trace events。
- `rive evidence list` must not show trace payloads。
- dispatch success must still disappear if `team report` is removed, even when trace contains a final answer.

## 9. Manual Review Checklist

Before marking Phase 7 done, reviewer should confirm:

- `team send` code path and `rive runner` code path share runner core instead of duplicating success logic。
- run-scoped token is implemented and tested。
- worker token is not returned to orchestrator。
- idempotency preflight happens before child launch。
- replay cannot trigger external CLI side effects。
- fake tests cover failure paths before real e2e is trusted。
- real e2e was run by both @samuel and @jian。
- Phase 7 does not introduce Work Graph, PTY, daemon, or async queue.
