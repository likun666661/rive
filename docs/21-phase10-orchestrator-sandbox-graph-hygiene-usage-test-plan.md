# Phase 10 Orchestrator Sandbox / Graph Hygiene / Usage 测试计划

## 1. 测试目标

Phase 10 验证三件事：

1. OpenCode orchestrator 可以写 control-plane facts，但不能静默直接改 source workspace。
2. Work DAG root closure 能发现 orphan / unconnected / incomplete nodes。
3. Rive 能从 OpenCode debug output 聚合 token usage。

主链路：

```text
rive runner orchestrator --runner opencode
  -> planner capability profile
  -> team work note/create/edge/inspect/send/accept
  -> worker writes files and reports refs
  -> graph hygiene clean
  -> root done
  -> usage query explains token cost
```

## 2. 关键验收线

1. Orchestrator 对 Work DAG 可写，对 implementation workspace 只读/可审计。
2. Worker 仍然可以写 workspace、跑测试、capture snapshot、report。
3. Orchestrator direct workspace mutation returns `orchestrator_workspace_mutation`。
4. Common mutation commands in planner PATH return `orchestrator_capability_denied`。
5. Root accept with root-scoped orphan nodes returns `work_graph_not_closed`。
6. `team work note` writes progress/decision/blocker events without changing completion state。
7. `rive debug trace usage` extracts OpenCode `step_finish.tokens` when available。
8. Usage/debug metrics never mutate facts/dispatch/work projection。

## 3. Capability Guard Tests

### Deny common mutation commands

Use fake OpenCode as orchestrator that tries:

```text
python -m pytest
cargo test
npm test
git commit
```

Expected:

- commands fail through planner PATH with `orchestrator_capability_denied` or stable stderr marker.
- runner result is not successful unless Work DAG still closes through legal worker delegation.
- no source file is changed by orchestrator.

### Direct file write audit

Fake orchestrator bypasses PATH wrapper:

```sh
printf hacked > source.py
```

Expected:

- post-run audit detects mutation outside `.rive/`.
- runner returns `orchestrator_workspace_mutation`.
- root work is not accepted as final success.
- audit output lists mutated path(s) in protocol/debug fields.

### Control-plane writes allowed

Fake orchestrator calls:

```text
team work create
team work edge add
team work note
team send --work --runner opencode --wait
team work accept
```

Expected:

- commands succeed under planner profile.
- Work DAG rows and notes are written.
- no workspace mutation violation is raised if only workers write files.

### Worker capabilities not restricted

Fake worker launched from restricted orchestrator attempts:

```text
write result file
run a harmless test command
rive snapshot capture
team report
```

Expected:

- worker succeeds.
- worker PATH is not planner PATH.
- worker env does not include `RIVE_ORCHESTRATOR_ROOT_WORK_ID`.

## 4. Work Note Tests

### Progress note

Run:

```text
team work note <node>
  --kind progress
  --command-id note-progress-1
  --stdin
```

Expected:

- event `work.note.recorded` exists.
- `work inspect` shows the note.
- node projection state is unchanged.
- replay returns same note.
- changed body with same command id returns `idempotency_conflict`.

### Worker note rejection

Worker env calls `team work note`.

Expected:

- rejected unless Phase 10 explicitly allows assigned-worker notes.
- if rejected, stable code `agent_role_not_allowed`.
- no note row/event is written.

## 5. Graph Hygiene Tests

### Orphan detection

Setup:

1. root objective exists.
2. orchestrator creates child A and connects root -> A.
3. orchestrator creates child B but does not connect it.
4. A is done.

Run:

```text
team work graph inspect --root <root>
```

Expected:

- `hygiene_state = dirty`.
- `orphan_nodes` includes B.
- root accept returns `work_graph_not_closed`.

### Clean root closure

Connect B or reopen/cancel it according to implementation policy.

Expected:

- graph inspect returns `hygiene_state = clean`.
- root accept can succeed once all reachable children are done.

### Incomplete reachable node

Root has connected child C that is `reviewable` but not accepted.

Expected:

- graph inspect lists C in `reviewable_unaccepted_nodes`.
- root accept returns `work_graph_not_closed`.

## 6. Usage Accounting Tests

### OpenCode token extraction

Create fake stdout JSONL under a debug run containing OpenCode-style token events:

```json
{"type":"step_finish","tokens":{"input":7730,"output":550,"reasoning":137,"cache":{"read":20480},"total":28897}}
```

Run:

```text
rive debug trace usage --run <run_id>
```

Expected:

- input/output/reasoning/cache_read/total parsed correctly.
- `non_cache_tokens = total - cache_read` or equivalent explicit field.
- trace event count still appears.

### Missing usage

Run usage against a trace/run with no token events.

Expected:

- command succeeds.
- run reports `usage_available=false` or `usage_unavailable`.
- no business state changes.

### Root/work aggregation

Given root with orchestrator run and worker dispatch run:

```text
rive debug trace usage --root <root_work_node_id>
```

Expected:

- totals include orchestrator + worker runs.
- output groups by run/agent/dispatch/work where mappings exist.

## 7. Fake Orchestrator Integration

Use fake OpenCode orchestrator + fake OpenCode worker.

Scenario:

1. orchestrator writes a progress note.
2. orchestrator creates child node.
3. orchestrator delegates to worker.
4. worker writes artifact, captures snapshot, reports done.
5. orchestrator accepts child.
6. graph hygiene clean.
7. orchestrator accepts root.
8. usage summary is available from fake JSONL.

Expected:

- runner succeeds.
- root is `done`.
- worker artifact exists.
- no orchestrator direct mutation outside `.rive/`.
- note is visible but does not itself change state.
- usage command returns totals.

## 8. Real OpenCode E2E

Run real OpenCode orchestrator and OpenCode worker with planner capability profile.

Objective:

```text
Create one implementation node, delegate to OpenCode worker, have the worker
write phase10-result.txt, then close the root cleanly.
```

Expected:

- root -> done.
- worker artifact exists.
- graph hygiene clean.
- no orphan nodes.
- no direct orchestrator source mutation.
- usage summary includes OpenCode token counts if available.
- debug trace only writes debug tables.

## 9. SWE-bench Smoke Regression

Re-run `pytest-dev__pytest-11143` or a smaller selected smoke if time/cost demands.

Expected:

- baseline targeted test fails before run.
- orchestrator does not directly modify source files.
- worker modifies `src/_pytest/assertion/rewrite.py`.
- root graph hygiene is clean.
- targeted pytest exits 0 after run.
- usage accounting reports orchestrator + worker totals.

If strict planner sandbox blocks the current orchestration flow, the result should be a clear `orchestrator_capability_denied` or `orchestrator_workspace_mutation`, not silent success.

## 10. Regression Tests

Full suite must pass:

```text
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Regression focus:

- Phase 5/6 runner behavior unchanged.
- Phase 7 delegation still works.
- Phase 8 Work DAG projection unchanged except hygiene additions.
- Phase 9 OpenCode orchestrator still works with legal delegation.
- No PTY tables introduced.
- Trace/usage does not become evidence/fact/work source.

## 11. Exit Criteria

Phase 10 test task is done when:

- fake capability guard tests pass;
- fake graph hygiene tests pass;
- usage parser/aggregation tests pass;
- real OpenCode e2e passes under planner profile;
- SWE-bench smoke either passes or reports a precise capability/hygiene blocker;
- all regression checks pass.
