# Phase 9 OpenCode Orchestrator Control 测试计划

## 1. 测试目标

Phase 9 验证真实 OpenCode orchestrator 可以通过 `team work` 控制 Work DAG，并通过 `team send --work --runner opencode --wait` 派生 OpenCode workers 完成节点。

主链路：

```text
rive runner orchestrator --runner opencode
  -> OpenCode orchestrator calls team work create/edge/inspect
  -> orchestrator calls team send --work --runner opencode --wait
  -> OpenCode worker reports snapshot/ref
  -> node becomes reviewable
  -> orchestrator accepts/reopens/creates follow-up
  -> root objective becomes done
```

本阶段所有真实 e2e 都用 OpenCode。Codex 只做既有回归，不作为 Phase 9 主路径。

## 2. 关键验收线

1. `team work` 是 agent-facing structured ABI，不是自然语言约定。
2. Work mutations are orchestrator-only。
3. Worker cannot create/accept/reopen graph nodes。
4. `team work accept` only succeeds for `reviewable` nodes。
5. `team send --work` success still comes only from worker dispatch projection。
6. Runner success comes only from root work projection `done`。
7. OpenCode stdout/final answer/debug trace do not mutate business state。
8. Replay of orchestrator runner does not relaunch OpenCode。
9. Replay of `team send --work` does not relaunch worker。
10. SWE-bench smoke uses only OpenCode and does not receive the dataset gold patch.

## 3. `team work` ABI Tests

### Orchestrator mutations

Setup:

- Initialize workspace.
- Add orchestrator agent.
- Run `team work create` with orchestrator env.

Assertions:

- command succeeds.
- response uses `protocol/display`.
- protocol includes `work_node_id`, `state`, `node_version`, `allowed_next_actions`.
- same `command_id` replay returns the same node.
- same `command_id` with changed body/title returns `idempotency_conflict`.

### Worker mutation rejection

Run the same mutation commands with worker env:

```text
team work create
team work edge add
team work accept
team work reopen
```

Expected:

- each fails with `agent_role_not_allowed` or equivalent stable code.
- no Work DAG rows are inserted or changed.

### Read behavior

Test:

- orchestrator can `team work list/show/inspect`.
- worker can `show/inspect` an assigned work node if implemented.
- worker cannot list unrelated graph data if Phase 9 implements scoped worker reads.

Read output must keep protocol/display split.

## 4. Work DAG Projection Tests

Create:

```text
root objective
investigation task
implementation task
validation review

root decomposes_to investigation
root decomposes_to implementation
root decomposes_to validation
implementation depends_on investigation
validation depends_on implementation
```

Assertions:

- investigation is `ready`.
- implementation is `blocked` with investigation in `missing_requirements`.
- validation is `blocked` with implementation in `missing_requirements`.
- after investigation report+accept, implementation becomes `ready`.
- after implementation report, implementation is `reviewable`, not `done`.
- after accept, validation becomes `ready`.

Cycle rejection must still pass:

```text
validation depends_on root
```

returns `work_graph_cycle` and changes nothing.

## 5. Orchestrator Runner Fake Tests

Use fake OpenCode binaries to make CI deterministic.

### Happy path

Fake orchestrator script:

1. calls `team work inspect <root>`.
2. creates implementation and validation nodes.
3. adds edges.
4. calls `team send --work <implementation> --runner opencode --wait`.
5. inspects implementation.
6. accepts implementation.
7. calls `team send --work <validation> --runner opencode --wait`.
8. accepts validation.
9. accepts root.

Fake worker script:

1. writes a result file.
2. captures snapshot.
3. calls `team status`.
4. calls `team report done`.

Expected:

- runner returns success.
- root is `done`.
- implementation/validation have dispatch bindings and snapshot refs.
- all fake OpenCode invocations are counted.

### Final answer does not count

Fake orchestrator prints "done" and exits without `team work accept`.

Expected:

- runner returns `work_not_done` or equivalent.
- root remains not done.
- stdout/final answer is saved only as debug/output.

### Replay no relaunch

Run `rive runner orchestrator` twice with the same command id.

Expected:

- second run returns same root/run projection.
- fake orchestrator invocation count remains 1.
- no duplicate work nodes, edges, dispatches, delegations, or refs.

### Conflicting replay

Same command id with different objective/workers/acceptance command:

- returns `idempotency_conflict`.
- does not launch OpenCode.
- does not mutate graph.

## 6. OpenCode-only Real E2E

Run a real OpenCode orchestrator with real OpenCode workers.

Objective:

```text
Create two files through two worker nodes, validate both files in a review node,
then accept the root objective.
```

Expected:

- Orchestrator is OpenCode.
- Each worker is OpenCode.
- No Codex process is launched in the main path.
- At least three work nodes are created under root.
- Workers report snapshots/refs.
- Worker reports move nodes to `reviewable`.
- Orchestrator uses `team work accept` to move nodes to `done`.
- Root becomes `done`.
- Debug trace can explain the run, but trace rows do not affect Work DAG projection.

Boundary checks:

- `%pty%` tables are still absent.
- stdout/final answer grep cannot explain state changes without corresponding ledger rows.
- OpenCode plugin ingest failures, if simulated, do not change runner outcome except trace count.

## 7. SWE-bench Lite Smoke

Candidate:

```text
dataset: princeton-nlp/SWE-bench_Lite
instance_id: pytest-dev__pytest-11143
repo: pytest-dev/pytest
base_commit: 6995257cf470d2143ad1683824962de4071c0eb7
failing test: testing/test_assertrewrite.py::TestIssue11140::test_constant_not_picked_as_module_docstring
```

Preparation:

1. Clone `https://github.com/pytest-dev/pytest`.
2. Checkout `6995257cf470d2143ad1683824962de4071c0eb7`.
3. Apply only the SWE-bench test patch for `pytest-dev__pytest-11143`.
4. Install minimal test dependencies needed for the targeted pytest test.
5. Confirm the targeted test fails before the orchestrator run.

Orchestrator objective:

```text
Solve SWE-bench Lite instance pytest-dev__pytest-11143.
Do not use any gold implementation patch.
Make this targeted test pass:
python -m pytest testing/test_assertrewrite.py::TestIssue11140::test_constant_not_picked_as_module_docstring
```

Expected flow:

- OpenCode orchestrator creates a root objective node.
- Orchestrator creates investigation, implementation, and validation/review nodes.
- Orchestrator delegates only to OpenCode workers.
- Worker implementation changes source files.
- Validation worker runs the targeted test.
- Validation report includes snapshot/ref or test output reference.
- Orchestrator accepts nodes only after inspect/test evidence.
- Root becomes `done`.
- The targeted test passes after the run.

Smoke success requires both:

1. root Work DAG projection is `done`;
2. the targeted test command exits 0.

## 8. Regression Tests

The full suite must still pass:

```text
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Regression focus:

- Phase 5 OpenCode runner still works.
- Phase 6 Codex runner still works but is not Phase 9 main path.
- Phase 7 `team send` still works.
- Phase 8 `rive work` and `team send --work` still work.
- Existing lazy Work DAG behavior does not break old no-work assertions.

## 9. Privacy / Debug Boundary

For all real runs, verify:

- debug trace rows live only in `debug_trace_*`.
- trace payloads are not written as `evidence_ref` automatically.
- facts/dispatch/work state changes have corresponding protocol commands.
- worker run-scoped tokens are not present in runner response, stdout/stderr files, or debug payload blobs.

## 10. Task Exit Criteria

Phase 9 test task is done when:

- fake orchestrator tests pass;
- real OpenCode-only DAG e2e passes;
- SWE-bench Lite smoke passes or a clearly documented environment blocker is filed;
- all regression checks pass;
- no business state is inferred from stdout/final answer/trace.
