# Phase 5: OpenCode Dispatch Runner MVP

Phase 5 的目的，是把我们已经手工跑通的 Phase 3 + Phase 4 闭环，产品化成一个最小可重复的 OpenCode runner。

Phase 3 已经证明：

```text
dispatch -> team status/report -> dispatch projection
```

Phase 4 已经证明：

```text
real OpenCode CLI -> hidden trace plugin -> Rive debug trace store
```

手动闭环也已经跑通过：人类创建 agent / dispatch，手工设置 `RIVE_*` 环境变量，把 prompt 交给真实 OpenCode，OpenCode 调 `rive snapshot capture`、`team status`、`team report`，最后 Rive 能看到 dispatch reported、fact/evidence 和 debug trace。

Phase 5 要解决的问题是：**把这条手工流程变成 Rive 可以启动和复现的一条 runner 路径。**

## 1. Phase 5 想解决什么问题

现在的 Rive 已经有底层能力，但还没有“运行一个真实外部 agent 完成 dispatch”的产品入口。

用户仍然需要手动做这些事：

- 安装 OpenCode trace plugin。
- 创建 worker agent。
- 创建 dispatch。
- 保存一次性 agent token。
- 手工拼 `RIVE_WORKSPACE` / `RIVE_AGENT_ID` / `RIVE_AGENT_TOKEN` / `RIVE_RUN_ID` / `RIVE_DISPATCH_ID`。
- 手工写 prompt，要求 OpenCode 调 `team status/report`。
- 运行后再分别查 dispatch、fact、snapshot、debug trace。

这不适合作为后续开发和验收的基础。Phase 5 要把它压成一个命令：

```text
rive runner opencode -> real OpenCode process -> team report -> dispatch projection + debug trace
```

它解决的是 **repeatable real-agent execution**，不是 Work Graph。

## 2. Phase 5 不是什么

Phase 5 仍然不做这些事：

- 不做 Work Graph。
- 不做 `team send`。
- 不做 daemon / scheduler / background runtime。
- 不做 PTY attach。
- 不做通用多 agent runner。
- 不从 debug trace 推断业务状态。
- 不把 trace 变成 evidence / fact / dispatch source。
- 不要求 OpenCode 成为 Rive 的 SDK 内部 agent。

OpenCode runner 只是一个 controlled launch path。事实仍然只能通过 `team` / `rive` command 写入 Rive 的 store。

## 3. 最小用户命令

建议 v0 暴露：

```text
rive runner opencode \
  --agent <agent-name> \
  --title <dispatch-title> \
  --command-id <idempotency-key> \
  [--agent-token <token-for-existing-agent>] \
  [--opencode-bin <path>] \
  [--timeout-seconds <seconds>] \
  [--snapshot-path <path>]... \
  --stdin
```

stdin 是这次 dispatch 的真实任务说明。

示例：

```bash
cat <<'EOF' | rive runner opencode \
  --agent worker-opencode-demo \
  --title "create a hello file" \
  --command-id run-opencode-demo-1 \
  --snapshot-path hello.txt \
  --stdin
Create hello.txt with one line: hello from opencode.
Then capture a snapshot for hello.txt and report done.
EOF
```

### 为什么叫 `runner opencode`

`rive run` 后续可能承担 daemon/runtime 语义。Phase 5 先用 `runner opencode` 明确表达：这是一个一次性外部 CLI runner，不是长期 runtime。

## 4. Runner 生命周期

一次成功运行的顺序应该是：

```text
1. load workspace
2. install / ensure OpenCode trace plugin
3. create or resolve worker agent
4. create dispatch
5. build protocol prompt
6. launch real opencode process with RIVE_* env
7. wait for process exit or timeout
8. reload dispatch projection
9. query correlated debug trace summary
10. return protocol/display response
```

### 4.1 Workspace

runner 要求 workspace 已经 `rive init`。MVP 不自动 init，避免在错误目录里创建 `.rive`。

稳定错误：

```text
workspace_not_initialized
```

### 4.2 Trace plugin

runner 可以调用 Phase 4 的已有安装能力，确保 workspace-local OpenCode plugin 存在：

```text
rive debug trace install opencode --workspace <workspace>
```

安装失败应让 runner 失败，除非用户显式传 `--no-trace`。v0 可以先不提供 `--no-trace`，因为本阶段的目标就是用 trace 闭环验证真实 agent。

trace 只写 `debug_trace_*` 和 `.rive/debug/trace/payloads/`。runner 不能因为 trace 里出现成功文本就关闭 dispatch。

### 4.3 Agent 和 token

当前 Rive 只保存 agent token hash，明文 token 只在 `rive agent add` 时返回一次。因此 runner 不能从数据库取回旧 agent token。

v0 规则：

- 如果 `--agent` 不存在，runner 创建新 worker agent，并只把新 token 注入本次 OpenCode 子进程。
- 如果 `--agent` 已存在，必须传 `--agent-token`。
- 如果 agent 已存在但没传 token，返回：

```text
runner_agent_token_required
```

runner 不持久化明文 token，也不把 token 打进 display 文案。

### 4.4 Dispatch

runner 用 stdin body 创建 dispatch：

```text
rive dispatch create --target <agent> --title <title> --command-id <id> --stdin
```

实现上可以直接调用 dispatch service，而不是 fork 自己的 CLI。

`command_id` 仍然是 dispatch creation 的幂等键：

- 同 `command_id` + 同 payload replay 返回同一 dispatch。
- 同 `command_id` + 不同 payload 返回 `idempotency_conflict`。

### 4.5 Protocol prompt

runner 交给 OpenCode 的 prompt 必须明确告诉它这不是普通聊天，而是一次 Rive dispatch execution。

prompt 至少包含：

- workspace root
- agent name / role
- dispatch id
- task title
- task body
- 允许使用的命令
- 必须通过 `team status/report` 写回状态
- snapshot/evidence 的推荐命令
- 禁止把自然语言完成当作 report

推荐模板：

```text
You are running inside a Rive dispatch.

Dispatch:
- id: <dispatch_id>
- title: <title>

Rive protocol:
- Use `team status --dispatch <dispatch_id> --snapshot <snapshot_id> --command-id <id> --stdin`
  for progress updates. Status does not close the dispatch.
- Use `team report --dispatch <dispatch_id> --status done|blocked|failed --snapshot <snapshot_id> --command-id <id> --stdin`
  to close or block/fail the dispatch.
- Before report, capture evidence with `rive snapshot capture ...`.
- A natural language final answer is not a Rive report.

Task:
<body>
```

If `--snapshot-path` is provided, include exact capture examples:

```text
rive snapshot capture --path <path> --label <label>
```

If no snapshot path is provided, tell OpenCode to capture the files it created or modified.

### 4.6 Child process environment

runner launches OpenCode with:

```text
RIVE_WORKSPACE=<workspace-root>
RIVE_AGENT_ID=<agent_id>
RIVE_AGENT_TOKEN=<token>
RIVE_RUN_ID=<run_id>
RIVE_DISPATCH_ID=<dispatch_id>
PATH=<dir containing rive/team>:$PATH
```

`RIVE_RUN_ID` should be generated by runner:

```text
run_<uuid>
```

This lets Phase 4 trace, Phase 3 facts, and future runner logs correlate the same external process.

### 4.7 OpenCode command

MVP can run:

```text
opencode run --format json --dangerously-skip-permissions <prompt>
```

`--opencode-bin` lets tests use a fake binary and lets users override path.

If OpenCode is missing:

```text
opencode_not_found
```

If OpenCode exits non-zero:

```text
opencode_exit_failed
```

The runner should save stdout/stderr under a debug runner path, for example:

```text
.rive/debug/runs/<run_id>/stdout.jsonl
.rive/debug/runs/<run_id>/stderr.log
```

These files are debug material only, not evidence/fact/dispatch state.

### 4.8 Timeout

MVP should support:

```text
--timeout-seconds <seconds>
```

Default can be 300 seconds.

If timeout fires, runner returns:

```text
opencode_timeout
```

The dispatch remains open unless OpenCode already reported before timeout.

## 5. Success criterion

Runner success is not “OpenCode printed something good”.

Runner success means:

```text
after OpenCode exits, dispatch projection is reported|blocked|failed
```

If OpenCode exits 0 but never calls `team report`, runner returns:

```text
dispatch_not_reported
```

The response should include enough debug pointers:

```json
{
  "protocol": {
    "runner": {
      "kind": "opencode",
      "run_id": "run_...",
      "exit_code": 0,
      "stdout_ref": ".rive/debug/runs/.../stdout.jsonl",
      "stderr_ref": ".rive/debug/runs/.../stderr.log"
    },
    "agent": {
      "agent_id": "agent_...",
      "name": "worker-opencode-demo"
    },
    "dispatch": {
      "dispatch_id": "disp_...",
      "state": "reported",
      "latest_report_status": "done"
    },
    "trace": {
      "adapter": "opencode-plugin",
      "event_count": 42,
      "session_ids": ["..."]
    }
  },
  "display": {
    "summary": "OpenCode reported dispatch done."
  }
}
```

`display.summary` is non-normative. Protocol decisions use `protocol.runner`, `protocol.dispatch`, and `protocol.trace`.

## 6. Failure modes

| Scenario | Expected result |
| --- | --- |
| workspace not initialized | `workspace_not_initialized` |
| OpenCode binary missing | `opencode_not_found` |
| existing agent without token | `runner_agent_token_required` |
| wrong agent token | `agent_token_invalid` |
| dispatch command_id conflict | `idempotency_conflict` |
| OpenCode exits without report | `dispatch_not_reported`, dispatch remains open |
| OpenCode reports with invalid snapshot | team command rejects; runner later sees dispatch still open |
| trace ingest fails inside plugin | plugin swallows failure; runner still uses dispatch state as truth |
| timeout | `opencode_timeout`, dispatch projection decides current business state |

## 7. Boundaries after Phase 5

After Phase 5, Rive should be able to say:

```text
I can start a real OpenCode worker for one dispatch,
give it the structured team protocol,
record its internal debug trace,
and verify completion only through dispatch ledger.
```

It still should not claim:

```text
I can manage a team graph.
I can deliver tasks between live PTY agents.
I can infer correctness from trace.
I can run every CLI agent generically.
```

Phase 5 is the smallest repeatable real-agent execution layer. It prepares the ground for later `team send`, runner daemon, PTY attach, or Work Graph binding.
