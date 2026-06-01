# Phase 7: Agent-to-Agent Delegation MVP

Phase 7 的目的，是把 Phase 5/6 的 runner 能力接到 `team` 协议上，让一个真实 CLI agent 可以把工作派给另一个真实 CLI agent。

Phase 5/6 已经证明：

```text
human command
  -> rive runner opencode|codex
  -> dispatch
  -> worker calls team status/report
  -> dispatch projection
```

Phase 7 要证明：

```text
real orchestrator CLI agent
  -> team send --to <worker> --runner opencode|codex --wait
  -> Rive creates a worker dispatch
  -> Rive launches the worker runner
  -> worker calls team status/report
  -> team send returns structured result to orchestrator
```

成功仍然只来自 dispatch projection。Orchestrator stdout、worker stdout、final answer、Codex/OpenCode trace 都只能做 debug。

## 1. Phase 7 想解决什么问题

现在 Rive 已经有三块底座：

1. evidence snapshot：能保存 agent 工作现场。
2. fact/dispatch ledger：能记录 agent 的结构化状态和 report。
3. runner adapters：能真实启动 OpenCode/Codex worker，并只按 dispatch projection 判断结果。

但现在派活者仍然是 human-facing `rive runner ...`。这还不是 agent team。真正的 team runtime 需要让 Orchestrator 在自己的 CLI session 里调用一个结构化 action：

```text
team send
```

然后 Rive 在 runtime 里完成：

- 鉴权：只有 orchestrator 能派活。
- 建 dispatch：创建 worker execution attempt。
- 启 worker：用 runner adapter 启动 OpenCode/Codex。
- 等 report：只看 `team report` 后的 dispatch projection。
- 回结果：给 orchestrator 一个 JSON response，而不是让它解析 worker 自然语言。

Phase 7 解决的是 **agent-to-agent delegation**，不是任务图。

## 2. 为什么 Phase 7 不是 Work Graph

Work Graph 要解决的是：

```text
objective -> task nodes -> dependencies -> completion constraints -> ready/done projection
```

Phase 7 解决的是一条 delegation edge：

```text
orchestrator -> worker dispatch -> worker report -> structured result
```

如果没有 `team send`，Work Graph 的 node 即使 ready 了，也没有可靠的 agent-to-agent 执行通道。因此顺序应该是：

```text
1. worker runners for OpenCode/Codex
2. team send delegation edge
3. Work Graph nodes bind to delegation dispatches
```

所以 Phase 7 不做 Work Graph、不做 task decomposition、不做 node done 语义。

## 3. Non-goals

Phase 7 明确不做：

- Work Graph。
- task node / dependency / ready / done projection。
- PTY attach。
- daemon / background scheduler。
- async queue。
- worker-to-worker mesh。
- arbitrary recursive delegation。
- `team send` without `--wait`。
- natural-language chat routing。
- 从 stdout/final answer/trace 推断业务成功。

## 4. Agent-facing command

新增：

```text
team send \
  --to <worker-name-or-id> \
  --runner opencode|codex \
  --title <dispatch-title> \
  --command-id <idempotency-key> \
  --wait \
  [--timeout-seconds <seconds>] \
  [--snapshot-path <path>]... \
  [--trust-project] \
  [--opencode-bin <path>] \
  [--codex-bin <path>] \
  --stdin
```

`--stdin` 是任务正文。长文本不进 argv。

`--wait` 在 v0 必须显式提供。v0 不提供后台 delegation，因为现在还没有 daemon/queue/recovery loop。

示例：

```bash
cat <<'EOF' | team send \
  --to reviewer-opencode \
  --runner opencode \
  --title "write result file" \
  --command-id orch-send-001 \
  --snapshot-path worker-result.txt \
  --wait \
  --stdin
Create worker-result.txt with exactly one line: RIVE_PHASE7_OK
Capture a snapshot for worker-result.txt, send one status update, then report done.
EOF
```

## 5. Who Can Send

`team send` 是 agent-facing ABI，不是 human CLI。

Actor 从 env 推导：

```text
RIVE_WORKSPACE
RIVE_AGENT_ID
RIVE_AGENT_TOKEN
RIVE_RUN_ID
```

Runtime 必须校验：

- workspace exists。
- actor agent exists。
- token valid。
- actor role is `orchestrator`。
- target agent exists。
- target role is `worker`。
- target is not deleted/offline by future policy。
- `command_id` present。
- `--wait` present。
- runner adapter is supported。

Worker 调 `team send` 必须被拒绝，错误码建议：

```text
agent_role_not_allowed
```

目标 agent 不存在：

```text
target_agent_not_found
```

目标不是 worker：

```text
target_role_invalid
```

未传 `--wait`：

```text
wait_required
```

## 6. Run-scoped Worker Token

Phase 7 有一个关键工程问题：`team send` 要启动 target worker，但 target worker 的长期 token 只在 `rive agent add` 时返回一次，runtime 后面不应该保存明文 token。

因此 v0 应新增 run-scoped token。

```text
agent_runs
  run_id
  agent_id
  token_hash
  created_by_event_id
  created_at
  expires_at?
  revoked_at?
```

认证规则扩展为：

```text
RIVE_AGENT_TOKEN can match:
  1. agent long-lived token hash
  2. active run-scoped token hash for that agent/run
```

`team send` 启动 worker 时：

1. runtime 生成 `worker_run_id`。
2. runtime 生成 worker run token。
3. runtime 只持久化 token hash。
4. runtime 把明文 token 注入 child env。
5. `team send` response 不返回 worker token。

这样 orchestrator 能派活，但不能拿到 worker token；worker 能在本 run 里调用 `team status/report`。

## 7. Delegation State and Idempotency

Phase 7 可以先不引入复杂 delegation 表，但必须保证 `team send` 是幂等的。

建议新增最小 projection：

```text
delegation_records
  command_id
  source_agent_id
  source_run_id
  target_agent_id
  worker_run_id
  dispatch_id
  runner
  request_hash
  state
  child_executed
  created_at
  completed_at?
```

状态建议：

```text
created -> child_running -> completed|dispatch_not_reported|runner_failed|timeout
```

`command_id` 规则：

- 同 actor + same `command_id` + same request hash：返回第一次 delegation 的当前/最终结果。
- replay 绝不能二次启动 worker child。
- same `command_id` + different request hash：`idempotency_conflict`。
- replay 时如果 dispatch 已 reported，直接返回 reported summary。
- replay 时如果 dispatch 仍 open，v0 可以继续 wait current dispatch；不能创建新 dispatch。

Dispatch create 本身可以使用派生 command id：

```text
dispatch_command_id = team-send:<command_id>:dispatch
```

避免和 runner 内部 command id 冲突。

## 8. Runner Core Changes

Phase 5/6 的 runner core 现在主要做 create-and-run：

```text
resolve/create worker agent
create dispatch
launch adapter
reload dispatch projection
```

Phase 7 需要拆出 run-existing-dispatch：

```text
run_existing_dispatch
  input:
    workspace
    target_agent_id
    dispatch_id
    worker_run_id
    worker_run_token
    runner adapter
    prompt context
  output:
    runner result
    dispatch projection
    trace summary
```

Shared core 仍然拥有业务规则：

- success only from dispatch projection。
- stdout/final answer/trace not success。
- `dispatch_not_reported`。
- adapter-specific not_found/exit_failed/timeout。
- stdout/stderr debug files。

Adapter 只拥有 vendor process shape：

- command shape。
- trace install。
- env/config。
- prompt hints。
- binary path。

## 9. Response Contract

`team send --wait` 返回 `protocol/display` 分层。

成功示例：

```json
{
  "protocol": {
    "ok": true,
    "action": "team.send",
    "command_id": "orch-send-001",
    "child_executed": true,
    "expected_next_action": "inspect_dispatch",
    "delegation": {
      "source_agent_id": "agent_orchestrator",
      "source_run_id": "run_orchestrator",
      "target_agent_id": "agent_worker",
      "worker_run_id": "run_worker",
      "dispatch_id": "disp_123",
      "runner": "opencode",
      "state": "completed"
    },
    "dispatch": {
      "dispatch_id": "disp_123",
      "state": "reported",
      "latest_report_status": "done",
      "allowed_next_actions": ["inspect_dispatch", "inspect_fact", "inspect_trace"]
    },
    "report": {
      "status": "done",
      "fact_event_id": "evt_456",
      "evidence_refs": ["snap_789"]
    },
    "trace": {
      "adapter": "opencode-plugin",
      "event_count": 42
    }
  },
  "display": {
    "summary": "worker reported done"
  }
}
```

`display.summary` 只给人读，agent 分支必须看 `protocol`。

## 10. Error Contract

Phase 7 应保留现有 error envelope：

```json
{
  "protocol": {
    "ok": false,
    "code": "dispatch_not_reported",
    "retryable": false,
    "expected_next_action": "inspect_dispatch",
    "projection": {
      "dispatch_id": "disp_123",
      "state": "open"
    }
  },
  "display": {
    "message": "worker process exited without team report"
  }
}
```

关键错误码：

```text
agent_not_found
agent_token_invalid
agent_role_not_allowed
target_agent_not_found
target_role_invalid
wait_required
runner_not_supported
idempotency_conflict
dispatch_not_reported
opencode_not_found / codex_not_found
opencode_exit_failed / codex_exit_failed
opencode_timeout / codex_timeout
```

## 11. Trace Boundary

Phase 7 会同时产生 orchestrator trace 和 worker trace，但 trace 仍然只是 debug 黑匣子。

它不能：

- 推进 dispatch state。
- 创建 fact。
- 创建 evidence。
- 让 `team send` 成功。
- 影响 Work Graph。

它可以：

- 解释 orchestrator 是否调用了 `team send`。
- 解释 worker 是否看到了 prompt。
- 解释 worker 为什么没 report。
- 帮 human debug fake/real CLI differences。

## 12. Real E2E Strategy

Phase 7 必须跑真实 agent-to-agent，而不是只跑 fake runner。

建议两条：

```text
Codex orchestrator -> OpenCode worker
OpenCode orchestrator -> Codex worker
```

为了不引入新的 top-level orchestrator runner，v0 验证可以用测试 harness 直接启动真实 orchestrator CLI，并注入 orchestrator env：

```text
RIVE_WORKSPACE
RIVE_AGENT_ID=<orchestrator>
RIVE_AGENT_TOKEN=<orchestrator-token>
RIVE_RUN_ID=<orchestrator-run-id>
```

Prompt 要求 orchestrator 调：

```text
team send --to <worker> --runner <other-adapter> --wait --stdin
```

验收只看：

- worker dispatch projection。
- status/report facts。
- snapshot evidence。
- `team send` structured response visible in orchestrator trace/debug output。

不看 orchestrator final answer 作为成功依据。

## 13. Phase 7 Completion Criteria

Phase 7 通过的标准：

1. `team send --wait` exists and is orchestrator-only。
2. `team send` creates a worker dispatch and launches OpenCode/Codex worker through shared runner core。
3. worker authenticates with run-scoped token and calls `team status/report`。
4. `team send` returns structured JSON result to orchestrator。
5. `command_id` replay does not re-execute worker child。
6. fake tests cover auth, idempotency, no-report, blocked/failed, and unsupported runner paths。
7. real Codex-orchestrator to OpenCode-worker e2e passes。
8. real OpenCode-orchestrator to Codex-worker e2e passes。
9. success only comes from dispatch projection。
10. trace remains debug-only; no Work Graph/PTY side effects.
