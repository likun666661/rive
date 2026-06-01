# Phase 8 Work DAG / Dispatch Binding 测试计划

## 1. 测试目标

Phase 8 验证 Rive 可以把 agent-to-agent delegation 绑定到一个 durable Work DAG。

目标链路：

```text
rive work create/edge
  -> DAG projection says node ready/blocked/reviewable/done
  -> team send --work <node> --wait starts worker dispatch
  -> worker calls team report with snapshot/ref
  -> work node becomes reviewable
  -> rive work accept makes node done
  -> dependent nodes become ready
```

本阶段不测试 daemon scheduler、PTY、TUI、LLM auto-planner、cyclic graph，且不允许 trace/stdout/final answer 推进 Work DAG。

## 2. 关键验收线

1. Work DAG 是 SQLite/event-backed facts，不是 Rust linked object graph。
2. Graph v0 是 DAG；cycle 必须拒绝。
3. Dispatch/delegation 是 execution binding，不是 topology edge。
4. `team report done` 只能让 node 进入 `reviewable`。
5. `rive work accept` 或 validation event 才能让 node `done`。
6. Projection 必须返回 `state / derived_from / missing_requirements / allowed_next_actions`。
7. All-predecessor semantics：所有依赖 done 后，节点才 ready。
8. 所有 graph/write commands 都有 `command_id` replay/conflict 语义。
9. `team send --work` replay 不二次启动 worker。
10. Phase 7 Codex/OpenCode real delegation 不能回退。

## 3. Schema / Store Tests

### Work node creation

- `rive work create --kind task --title ... --command-id ...` creates one work node.
- same command replay returns same node.
- same command id with different title/body returns `idempotency_conflict`.
- node has `node_version`.
- event payload includes actor, command_id, node_id, kind, title/body hash.

### Work edge creation

- `rive work edge add --type depends-on --from A --to B --command-id ...` creates one edge.
- same command replay returns same edge.
- same command id with different endpoints/type returns `idempotency_conflict`.
- missing node returns `work_node_not_found`.
- invalid edge type returns stable error.
- edge creation bumps `graph_version` or returns updated graph version in projection.

### Cycle rejection

Create:

```text
A depends_on B
B depends_on C
```

Then attempt:

```text
C depends_on A
```

Expected:

- command fails with `work_graph_cycle`.
- no edge row is inserted.
- graph projection remains unchanged.

### Version conflict

- edge add with stale `expected_graph_version` returns `graph_version_conflict`.
- node update/accept with stale `expected_node_version` returns `node_version_conflict` if version args are implemented in Phase 8.

## 4. Projection Tests

### Ready / blocked

Setup:

```text
A done
B depends_on A
C depends_on B
```

Assertions:

- B is `ready` after A done.
- C is `blocked` while B is not done.
- C `missing_requirements` includes B.
- `allowed_next_actions` for C does not include `delegate`.

### Reviewable / done

Setup:

1. create node B.
2. bind dispatch to B.
3. worker reports `done` with snapshot.

Assertions:

- B state is `reviewable`.
- `allowed_next_actions` includes `accept`, `reopen`, `delegate_again` or equivalent.
- B is not `done`.

Then:

```text
rive work accept B --command-id accept-b
```

Assertions:

- B state is `done`.
- dependencies waiting on B can become `ready`.

### Required refs

If a bound dispatch reports `done` without required snapshot/ref:

- node remains `blocked` or `needs_attention` according to implementation policy.
- `missing_requirements` includes `snapshot` / `artifact_ref`.
- node does not become `reviewable`.

## 5. Command Tests

### Human CLI

Required command coverage:

```text
rive work create
rive work edge add
rive work list
rive work show
rive work inspect
rive work accept
rive work reopen
```

Assertions:

- outputs use `protocol/display`.
- protocol fields are stable IDs/enums/versions.
- display explanation is non-normative.

### Agent CLI

`team send` with `--work`:

- orchestrator can delegate ready work node.
- worker actor cannot call `team send`.
- unknown work node returns `work_node_not_found`.
- blocked work node returns `work_node_not_ready`.
- done/cancelled/superseded node cannot be delegated unless explicit reopen/retry policy exists.
- response includes delegation, dispatch, and work projection.

`team report` with work-bound dispatch:

- validates snapshot integrity.
- stores ref binding.
- updates dispatch projection.
- updates work projection to `reviewable` when done.
- does not directly accept/done node.

## 6. Fake Integration Tests

Use fake worker binaries to keep CI deterministic.

### Single node happy path

1. `rive init`
2. add orchestrator + worker.
3. `rive work create --kind task --title "single"`
4. call `team send --work <node> --to worker --runner opencode --wait` with fake worker.
5. fake worker writes file, captures snapshot, reports done.
6. assert work node is `reviewable`.
7. `rive work accept <node>`.
8. assert work node is `done`.

### Dependency unlock

Graph:

```text
root decomposes_to A
root decomposes_to B
B depends_on A
```

Assertions:

- A is ready.
- B is blocked because A missing.
- after A report+accept, B becomes ready.

### Replay no child re-exec

Run same `team send --work --command-id same` twice.

Assertions:

- fake worker invocation count is 1.
- second response references same delegation/dispatch.
- work projection unchanged except no duplicate binding.

Then run same command id with different body/target/work node.

Assertions:

- returns `idempotency_conflict`.
- no new dispatch.
- no new work binding.

### Report does not auto-done

Fake worker reports done and writes success to stdout.

Assertions:

- dispatch is `reported/done`.
- node is `reviewable`.
- node is not `done`.
- trace/stdout presence does not change this.

## 7. Real E2E: Orchestrator with Two Workers

This is the important product proof. It should use real local Codex/OpenCode, not only fake payloads.

Suggested DAG:

```text
root objective
  -> task A: create a.txt with RIVE_PHASE8_A_OK
  -> task B: create b.txt with RIVE_PHASE8_B_OK
  -> review node: verify both files
```

Edges:

```text
root decomposes_to A
root decomposes_to B
review validates root
review depends_on A
review depends_on B
```

Real flow:

1. create graph with `rive work`.
2. launch real orchestrator agent.
3. orchestrator inspects ready nodes.
4. orchestrator calls `team send --work A --to worker-opencode --runner opencode --wait`.
5. orchestrator calls `team send --work B --to worker-codex --runner codex --wait`.
6. workers create files, snapshot, report done.
7. orchestrator or human accepts A/B.
8. review node becomes ready.
9. review worker verifies both files and reports done.
10. human/orchestrator accepts review/root as policy allows.

Assertions:

- A and B dispatches are reported/done.
- A and B nodes become reviewable after report and done only after accept.
- review node is blocked until A/B done.
- after A/B accept, review node is ready.
- trace exists for real agents but does not affect work projection.
- no PTY tables/events.
- global Codex config unchanged.

## 8. Regression Tests

Phase 8 touches `team send`, dispatch report, runner core, and store schema.

Must keep passing:

- Phase 1 snapshot tests.
- Phase 2 fact/evidence tests.
- Phase 3 dispatch/status/report tests.
- Phase 4 debug trace tests.
- Phase 5 OpenCode runner tests.
- Phase 6 Codex runner tests.
- Phase 7 delegation tests.

Specific regressions:

- `team send` without `--work` still works as Phase 7.
- `rive runner opencode` and `rive runner codex` still work.
- replay still does not relaunch child.
- stdout-only remains failure unless `team report` is accepted.
- trace remains debug-only.

## 9. DB / Boundary Checks

After Phase 8 flows:

- Work tables exist and contain expected nodes/edges/bindings.
- Dispatch/delegation tables contain execution attempts.
- Facts/snapshots contain worker report and evidence.
- Debug trace tables contain only debug trace.
- No `pty` tables/events exist.
- Work projection is derived from work/dispatch/fact/ref events, not from trace/stdout.

Recommended SQL checks:

```sql
select count(*) from work_nodes;
select count(*) from work_edges;
select count(*) from work_dispatch_bindings;
select count(*) from dispatches;
select count(*) from facts;
select count(*) from debug_trace_events;
select count(*) from sqlite_master where type='table' and name like '%pty%';
```

## 10. Acceptance

Phase 8 is accepted when:

- automatic tests pass.
- fake integration tests prove projection/idempotency/cycle rules.
- @samuel runs a real multi-worker DAG flow.
- @jian independently runs a real multi-worker DAG flow.
- both confirm success only from ledger/projection, not stdout/final answer/trace.
