# Phase 6: Codex Dispatch Runner + Runner Adapter Boundary

Phase 6 的目的，是把 Phase 5 的 OpenCode runner 从“一条成功链路”升级成“可扩展 runner 机制”。

Phase 5 已经证明：

```text
dispatch -> OpenCode runner -> team status/report -> dispatch projection
```

但这仍然可能只是 OpenCode 特例。Phase 6 要验证同一套 Rive 事实模型能不能驱动第二个真实黑盒 CLI agent：Codex。

Phase 6 的结论目标是：

```text
dispatch -> runner adapter(OpenCode|Codex) -> team status/report -> dispatch projection
```

成功仍然只来自 dispatch ledger。Codex stdout、final answer、hook trace 都只能做 debug。

## 1. 为什么 Phase 6 不是 Work Graph

Work Graph 要表达任务拆分、依赖、ready/done projection 和完成约束。它应该消费 dispatch/fact/evidence，而不是替代它们。

现在还有一个更基础的问题没验证完：

```text
Rive runner 是否只适配了 OpenCode？
还是同一套 dispatch/fact/evidence/projection 规则能驱动多种外部 CLI agent？
```

如果 Phase 6 直接做 Work Graph，就会把上层 graph 语义压在一个单一 OpenCode runner 上。更稳的顺序是：

```text
1. OpenCode runner closed loop
2. Codex runner closed loop
3. runner adapter boundary 稳定
4. 再做 agent-to-agent delegation 或 Work Graph
```

所以 Phase 6 不做 Work Graph。

## 2. Phase 6 想解决的问题

Phase 6 要解决三件事。

### 2.1 第二个真实 agent

Rive 需要证明 Codex 也能被同一套协议驱动：

- Rive 创建 dispatch。
- Rive 启动真实 Codex。
- Codex 看到 Rive protocol prompt。
- Codex 调 `rive snapshot capture`。
- Codex 调 `team status/report`。
- Rive 只按 dispatch projection 判断成功。
- Codex hook trace 只做 debug。

### 2.2 RunnerAdapter 边界

OpenCode 和 Codex 的差异应该被限制在 adapter 层：

- trace install
- child process command
- required env / config flags
- prompt hints
- stdout/stderr file naming
- real e2e compatibility notes

这些差异不应该污染 dispatch/fact/evidence/idempotency 规则。

### 2.3 Codex 真实运行条件

Codex 比 OpenCode 多几个实际问题：

- Codex hooks 需要 `codex_hooks` feature flag。
- Project hooks 需要 trusted project 配置。
- Codex command shape 与 OpenCode 不同。
- Codex hook 能记录 lifecycle/tool events，但不是完整 stdout success source。

Phase 6 要把这些运行条件写进 runner，不靠人手工拼。

## 3. Non-goals

Phase 6 明确不做：

- Work Graph。
- `team send`。
- agent-to-agent delegation。
- PTY attach。
- daemon / scheduler。
- 通用任意 CLI agent runner。
- Codex app-server。
- Codex rollout trace bundle import。
- 从 Codex stdout/final answer/trace 推断成功。

## 4. User-facing command

新增：

```text
rive runner codex \
  --agent <agent-name> \
  --title <dispatch-title> \
  --command-id <idempotency-key> \
  [--agent-token <token-for-existing-agent>] \
  [--codex-bin <path>] \
  [--timeout-seconds <seconds>] \
  [--snapshot-path <path>]... \
  [--trust-project] \
  --stdin
```

示例：

```bash
cat <<'EOF' | rive runner codex \
  --agent worker-codex-demo \
  --title "create a codex file" \
  --command-id phase6-codex-demo-1 \
  --snapshot-path codex-result.txt \
  --trust-project \
  --stdin
Create codex-result.txt with one line: hello from codex.
Then capture a snapshot, send one status update, and report done.
EOF
```

`--trust-project` 的含义是：runner 可以用 Codex CLI 的 one-run config override 让本次 workspace 可信，从而加载 workspace-local hooks。MVP 不应该静默修改用户全局 `~/.codex/config.toml`。

## 5. RunnerAdapter boundary

Phase 6 应该把 Phase 5 的 OpenCode runner 拆成 shared runner core 和 adapter。

### 5.1 Shared runner core

Shared core 负责：

```text
load workspace
resolve/create worker agent
enforce existing-agent token rule
create dispatch idempotently
create run_id and debug run dir
build protocol prompt from common template + adapter hints
launch adapter process
write stdout/stderr debug files
reload dispatch projection
query trace summary
return protocol/display response
```

Shared core owns all protocol facts:

- agent token validation
- dispatch creation/idempotency
- success rule
- `dispatch_not_reported`
- `opencode/codex_exit_failed`
- stdout/final answer not counted as success
- trace not counted as success

### 5.2 Adapter trait

Suggested trait shape:

```rust
trait RunnerAdapter {
    fn kind(&self) -> &'static str;
    fn binary_label(&self) -> &'static str;
    fn default_binary(&self) -> &'static str;
    fn install_trace(&self, workspace: &Workspace) -> Result<()>;
    fn build_prompt(&self, common: &RunnerPromptContext) -> String;
    fn build_command(&self, input: RunnerCommandContext) -> Command;
    fn trace_adapter(&self) -> &'static str;
    fn missing_binary_code(&self) -> &'static str;
    fn exit_failed_code(&self) -> &'static str;
}
```

Implementation can choose a simpler shape, but the boundary matters:

```text
RunnerCore = protocol and business rules
Adapter = vendor process shape and trace setup
```

### 5.3 Existing OpenCode behavior

`rive runner opencode` should keep its current user contract. Refactor must not regress:

- fake OpenCode tests
- real OpenCode closed loop
- replay not re-executing child
- stdout-only not success
- trace debug-only boundary

## 6. Codex adapter behavior

### 6.1 Trace install

Codex adapter should reuse Phase 4:

```text
rive debug trace install codex --workspace <workspace>
```

This writes:

```text
.codex/hooks.json
.rive/debug/adapters/codex-rive-trace-hook.sh
```

Runner should ensure Codex sees:

```text
-c features.codex_hooks=true
-c projects."<workspace>".trust_level="trusted"
```

The trust override should be per invocation unless user explicitly asks to persist config later. Phase 6 should not mutate global Codex config.

### 6.2 Child process command

MVP can run Codex through `codex exec`.

Suggested shape:

```text
codex exec \
  -c features.codex_hooks=true \
  -c projects."<workspace>".trust_level="trusted" \
  --dangerously-bypass-approvals-and-sandbox \
  <prompt>
```

The exact flags should follow the installed Codex version on this machine. Phase 6 tests should use `--codex-bin` to inject fake Codex and avoid depending on local Codex for unit tests.

If Codex binary is missing:

```text
codex_not_found
```

If Codex exits non-zero:

```text
codex_exit_failed
```

If Codex exits 0 without `team report`:

```text
dispatch_not_reported
```

### 6.3 Child process environment

Codex runner injects the same Rive env contract as OpenCode:

```text
RIVE_WORKSPACE=<workspace-root>
RIVE_AGENT_ID=<agent_id>
RIVE_AGENT_TOKEN=<token>
RIVE_RUN_ID=<run_id>
RIVE_DISPATCH_ID=<dispatch_id>
PATH=<dir containing rive/team>:$PATH
```

Codex hook traces should include `RIVE_AGENT_ID`, `RIVE_RUN_ID`, and `RIVE_DISPATCH_ID` labels when possible, matching the OpenCode trace correlation added in Phase 5.

### 6.4 Prompt contract

The prompt should use the same Rive protocol rules as OpenCode, plus Codex-specific execution hints:

```text
You are running inside a Rive dispatch.
Use shell commands in this workspace.
Before reporting, capture a Rive snapshot.
Use `team status ...` for progress.
Use `team report ...` to close the dispatch.
A final natural language answer is not a Rive report.
```

If `--snapshot-path` is provided, include exact commands.

## 7. Success rule

Same as Phase 5:

```text
success = dispatch projection is reported|blocked|failed after child exits
```

Not success:

- Codex prints "done".
- Codex final answer says it completed.
- Codex hook trace includes tool output.
- stdout contains expected file content.
- snapshot exists without `team report`.

If dispatch remains `open`:

```text
dispatch_not_reported
```

## 8. Response contract

`rive runner codex` should return the same shape as OpenCode, with `runner.kind = "codex"`:

```json
{
  "protocol": {
    "runner": {
      "kind": "codex",
      "run_id": "run_...",
      "binary": "/usr/local/bin/codex",
      "exit_code": 0,
      "stdout_ref": ".rive/debug/runs/.../stdout.log",
      "stderr_ref": ".rive/debug/runs/.../stderr.log",
      "child_executed": true
    },
    "agent": {
      "agent_id": "agent_...",
      "name": "worker-codex-demo"
    },
    "dispatch": {
      "dispatch_id": "disp_...",
      "state": "reported",
      "latest_report_status": "done"
    },
    "trace": {
      "adapter": "codex-hook",
      "event_count": 8,
      "session_ids": ["..."]
    }
  },
  "display": {
    "summary": "Codex runner ended with dispatch reported."
  }
}
```

Adapter-specific field names can be additive. Protocol consumers should branch on `runner.kind`, `dispatch.state`, and structured error codes, not display text.

## 9. Failure modes

| Scenario | Expected result |
| --- | --- |
| workspace not initialized | `workspace_not_initialized` |
| Codex binary missing | `codex_not_found` |
| existing agent without token | `runner_agent_token_required` |
| wrong token | `agent_token_invalid` |
| dispatch command_id conflict | `idempotency_conflict` |
| command_id replay | no child re-execution |
| Codex exits 0 without report | `dispatch_not_reported` |
| Codex exits non-zero | `codex_exit_failed` |
| Codex times out | `codex_timeout` |
| hook install fails | runner fails before child process |
| hook ingest fails | hook should not alter Codex behavior; dispatch remains source of truth |

## 10. Boundaries after Phase 6

After Phase 6, Rive should be able to say:

```text
The same dispatch/fact/evidence/projection model drives OpenCode and Codex.
Runner differences are isolated behind adapters.
Real OpenCode and real Codex both close dispatches only through team report.
```

It still should not claim:

```text
Agents can delegate to each other.
Rive has a Work Graph.
Rive can attach to PTYs.
Rive can schedule background teams.
Rive supports every CLI agent.
```

Those are later phases. Phase 6 is about proving the runner abstraction, not adding a new coordination layer.
