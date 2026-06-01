# Phase 11 Work DAG Scheduler / Parallel Worker Pool 测试计划

## 1. 测试目标

Phase 11 验证 Rive runtime 能在已有 Work DAG 上做 foreground scheduling：

```text
root Work DAG
  -> scheduler finds ready leaf nodes
  -> launches multiple OpenCode workers
  -> workers report snapshots/refs
  -> runtime accepts according to explicit policy
  -> dependent nodes unlock
  -> root projection reaches done
```

验收重点不是 OpenCode 的编码能力，而是调度协议：

- ready/blocked/reviewable/done projection 是否可信。
- 并发 worker 是否不污染 graph topology。
- replay / failure 是否不重复启动 worker。
- success 是否只来自 Work DAG / dispatch projection。

## 2. 自动测试

### Fake scheduler happy path

Setup:

```text
root
  decomposes_to A
  decomposes_to B
  decomposes_to C
C depends_on A
C depends_on B
```

Run:

```text
rive scheduler run \
  --root <root> \
  --runner opencode \
  --worker worker-a \
  --worker worker-b \
  --max-parallel 2 \
  --acceptance-mode auto-reported \
  --command-id sched-happy
```

Fake workers:

- A writes `a.txt`, snapshots, reports done。
- B writes `b.txt`, snapshots, reports done。
- C runs only after A/B accepted, writes `c.txt`, snapshots, reports done。

Expected:

- A/B are both launched before C。
- A/B/C dispatches reach `reported/done`。
- A/B/C work nodes become `done` through explicit scheduler accept events。
- root reaches `done`。
- `scheduler_run.state = completed`。

### Manual acceptance mode

Same graph, but:

```text
--acceptance-mode manual
```

Expected:

- A/B workers run and report。
- A/B become `reviewable`, not `done`。
- scheduler returns `waiting_review`。
- C remains blocked。
- no auto accept event is written。

### No-report failure

Fake worker exits 0 and prints successful final text but never calls `team report`。

Expected:

- scheduler does not mark node done。
- result is `work_scheduler_stalled` or `dispatch_not_reported` according to implementation boundary。
- stdout/final answer is present only in debug output。

### Replay no relaunch

Run the same scheduler command twice with the same `command_id`。

Expected:

- second run returns same scheduler projection。
- fake worker invocation count is unchanged。
- no duplicate dispatch/delegation/ref rows。

### Idempotency conflict

Same `command_id`, different root or worker list。

Expected:

- `idempotency_conflict`。
- no new worker launch。

### Node claim conflict

Try to schedule the same ready node through two scheduler runs.

Expected:

- only one scheduler run claims the node。
- loser returns `work_node_already_claimed` or skips the claimed node and reports stalled。
- no duplicate open dispatch for the same node。

### Graph hygiene preflight

Root scope contains an orphan/unconnected node。

Expected:

- scheduler refuses to start。
- stable code `work_graph_not_closed` or `work_scheduler_dirty_graph`。
- no worker process starts。

### Worker graph mutation rejection

Worker tries `team work create` or `team work edge add`。

Expected:

- rejected with `agent_role_not_allowed`。
- scheduler still relies on dispatch/report facts only。

### Usage/debug side effects

Run scheduler then:

```text
rive debug trace usage --root <root>
```

Expected:

- usage can aggregate scheduler/worker runs if OpenCode stdout contains token events。
- query does not write evidence/fact/dispatch/work state。

## 3. Real OpenCode E2E

Use a temporary workspace and real `/opt/homebrew/bin/opencode`。

Graph:

```text
root
  decomposes_to write-a
  decomposes_to write-b
  decomposes_to combine
combine depends_on write-a
combine depends_on write-b
```

Worker prompts:

- `write-a`: create `phase11-a.txt` with `RIVE_PHASE11_A_OK`。
- `write-b`: create `phase11-b.txt` with `RIVE_PHASE11_B_OK`。
- `combine`: verify both files exist and create `phase11-combined.txt` with `RIVE_PHASE11_COMBINED_OK`。

Run:

```text
rive scheduler run \
  --root <root> \
  --runner opencode \
  --worker worker-a \
  --worker worker-b \
  --max-parallel 2 \
  --acceptance-mode auto-reported \
  --command-id phase11-real
```

Expected:

- root reaches `done`。
- all three dispatches reach `reported/done`。
- all three nodes have snapshot/ref bindings。
- A/B are completed before combine starts。
- result files contain expected strings。
- no `%pty%` tables。
- trace exists only in debug namespace。
- token / run-scoped token leak grep is zero。

## 4. Regression

Run full suite:

```text
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Required:

- Phase 5/6 runner tests still pass。
- Phase 7 delegation tests still pass。
- Phase 8 Work DAG tests still pass。
- Phase 9 orchestrator tests still pass。
- Phase 10 sandbox/hygiene/usage tests still pass。

## 5. Non-goal Checks

Phase 11 must not introduce:

- daemon process。
- PTY tables or PTY state。
- Codex as primary scheduler path。
- auto planner changes。
- stdout/final answer/trace based state transitions。
- Git branch/commit refs as completion source。

