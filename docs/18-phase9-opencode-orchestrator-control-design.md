# Phase 9: OpenCode Orchestrator Work Graph Control MVP

Phase 9 的目标，是让一个真实 OpenCode orchestrator 接管 Phase 8 已经跑通的 Work DAG 控制流程。

Phase 8 已经证明：

```text
human creates Work DAG
  -> human/team send binds dispatch to work node
  -> worker reports with snapshot/ref
  -> node becomes reviewable
  -> human accepts node
  -> dependent nodes unlock
```

Phase 9 要证明：

```text
human gives objective
  -> Rive launches real OpenCode orchestrator
  -> orchestrator creates/updates Work DAG through `team work`
  -> orchestrator delegates ready nodes to OpenCode workers through `team send --work --wait`
  -> workers report with snapshots/refs
  -> orchestrator inspects, accepts, reopens, or creates follow-up nodes
  -> root objective becomes done
```

本阶段按 @kun-li 的要求只走 OpenCode：orchestrator 用 OpenCode，worker 也用 OpenCode。Codex runner 保持回归测试覆盖，但不作为 Phase 9 的主验证路径。

## 1. Phase 9 想解决什么问题

Rive 现在已经有：

1. Snapshot / fact / dispatch ledger。
2. OpenCode / Codex runner adapter。
3. `team send --wait` agent-to-agent delegation。
4. Work DAG + dispatch binding。

缺的是 **agent-owned graph control loop**。

现在 Work DAG 主要靠 human-facing `rive work` 操作。真实协作里，orchestrator 应该能在自己的 CLI session 里做这些事：

- 根据目标创建 root / investigation / implementation / validation / review nodes。
- 给节点加 `depends_on / validates / decomposes_to` edge。
- inspect node projection，理解 blocked/ready/reviewable 的原因。
- 对 ready node 调 `team send --work --runner opencode --wait` 派生 worker。
- 看到 worker report 后判断 refs/tests 是否足够。
- 用 `team work accept` 推进 node done，或 `team work reopen` / 新建 follow-up node。

Phase 9 解决的是 **orchestrator control plane**。它不是 automatic planner 的最终形态，但要先把控制动作全部拉进结构化 `team` ABI。

## 2. Non-goals

Phase 9 明确不做：

- daemon scheduler。
- background queue。
- TUI。
- PTY attach。
- Codex 作为本阶段主路径。
- 多 orchestrator 竞争控制同一个 DAG。
- Worker-to-worker mesh。
- 自动读取 stdout/final answer/trace 来推进 Work DAG。
- 完整 SWE-bench harness 或排名。
- 从 dataset gold patch 泄漏修复答案给 orchestrator。

Trace 仍然只是 debug 黑匣子。业务状态只来自 Rive ledger/projection。

## 3. Agent-facing `team work` ABI

Phase 8 已有 human-facing `rive work`。Phase 9 要增加 agent-facing `team work`，让 orchestrator 能用同一套事实系统修改 Work DAG。

命令形态：

```text
team work create
  --kind objective|task|check|review
  --title <title>
  --command-id <id>
  --stdin

team work edge add
  --type decomposes-to|depends-on|validates|supersedes
  --from <node>
  --to <node>
  --command-id <id>

team work list
team work show <node>
team work inspect <node>

team work accept <node>
  --command-id <id>
  --stdin

team work reopen <node>
  --command-id <id>
  --stdin
```

权限：

- Mutations are orchestrator-only: `create`, `edge add`, `accept`, `reopen`。
- Workers may read only what they need: v0 can allow `show/inspect` for assigned work nodes, but workers must not mutate graph.
- Actor identity is derived from `RIVE_WORKSPACE`, `RIVE_AGENT_ID`, `RIVE_AGENT_TOKEN`, and `RIVE_RUN_ID`; CLI args must not override actor identity.

所有写命令必须保留 Phase 8 的语义：

- `command_id` replay returns the original result。
- Same `command_id` with different request returns `idempotency_conflict`。
- Cycle write returns `work_graph_cycle` and inserts no edge。
- `team work accept` only succeeds for `reviewable` nodes。
- Projection response includes protocol fields: `state`, `derived_from`, `missing_requirements`, `allowed_next_actions`。
- Display text is non-normative。

## 4. `rive runner orchestrator`

新增 human-facing runner：

```text
rive runner orchestrator \
  --runner opencode \
  --agent <orchestrator-agent> \
  --command-id <idempotency-key> \
  --worker <worker-agent>... \
  [--acceptance-command <command>] \
  [--opencode-bin <path>] \
  [--timeout-seconds <seconds>] \
  --stdin
```

`--stdin` 是目标正文。长文本不进入 argv。

Runner responsibilities：

1. Ensure orchestrator agent exists with role `orchestrator`。
2. Create an orchestrator run with run-scoped token。
3. Create or reuse a root `objective` work node for this command_id。
4. Install OpenCode debug trace plugin for this workspace。
5. Launch OpenCode with orchestrator env:

```text
RIVE_WORKSPACE
RIVE_AGENT_ID
RIVE_AGENT_TOKEN
RIVE_RUN_ID
RIVE_ORCHESTRATOR_ROOT_WORK_ID
RIVE_AVAILABLE_WORKERS
```

6. Inject the Phase 9 orchestrator prompt template.
7. Wait for OpenCode exit.
8. Re-read root work projection.
9. Return success only if root projection is `done`.

Important boundary: runner must not decide success from orchestrator stdout, OpenCode final answer, or trace. Those are output/debug only.

Replay behavior：

- If the same `--command-id` is replayed and the orchestrator run already exists, do not relaunch OpenCode.
- Return the existing root work projection and runner record.
- If the same command id has different objective/workers/acceptance command, return `idempotency_conflict`.

## 5. Orchestrator Prompt Template

The runner should generate a prompt with this structure:

```text
You are the Rive Orchestrator for this workspace.

Goal:
<objective from stdin>

Root work node:
<RIVE_ORCHESTRATOR_ROOT_WORK_ID>

Available workers:
<worker list; all workers must use runner=opencode>

Acceptance command, if provided:
<command>

Rules:
1. Use `team work` to create and maintain a Work DAG under the root node.
2. Start with at least investigation, implementation, and validation/review nodes.
3. Use `team work inspect <node>` before delegating and after each worker report.
4. Delegate work with `team send --work <node> --runner opencode --wait --stdin`.
5. Workers must use `rive snapshot capture` and `team report`; natural language completion is not enough.
6. A reported node is only `reviewable`. Use `team work accept` only after checking artifacts, snapshots, or test output.
7. If tests fail or evidence is incomplete, use `team work reopen` or create a follow-up node. Do not rewrite history.
8. Final success requires the root objective projection to be `done`.
9. stdout/final answer/debug trace do not count as completion.
```

The prompt can include command examples, but it must not include the SWE-bench gold patch.

## 6. SWE-bench Smoke Candidate

Phase 9 final smoke should use one small SWE-bench Lite issue, not the full benchmark harness.

Chosen candidate:

```text
dataset: princeton-nlp/SWE-bench_Lite
instance_id: pytest-dev__pytest-11143
repo: pytest-dev/pytest
base_commit: 6995257cf470d2143ad1683824962de4071c0eb7
failing test: testing/test_assertrewrite.py::TestIssue11140::test_constant_not_picked_as_module_docstring
problem: assertion rewrite treats a leading numeric AST constant as if it were a module docstring
```

Why this candidate:

- Pure Python repository.
- One targeted failing test.
- Small implementation surface in pytest assertion rewrite.
- Expected fix size is small, so it is suitable for validating orchestration rather than measuring raw coding ability.

Smoke preparation:

1. Clone `pytest-dev/pytest` and checkout the base commit.
2. Apply only the SWE-bench `test_patch` for `pytest-dev__pytest-11143`.
3. Confirm the targeted test fails before the run.
4. Run Phase 9 with OpenCode orchestrator and OpenCode workers.
5. Confirm root work node is `done`.
6. Confirm the targeted test passes after the run.

Source: <https://huggingface.co/datasets/princeton-nlp/SWE-bench_Lite>

## 7. Completion Semantics

Phase 9 has two levels of success:

### Runner success

`rive runner orchestrator` succeeds only when:

- root work node exists,
- root projection is `done`,
- replay did not relaunch child processes,
- no unsupported runner was used in the main path.

### SWE-bench smoke success

The final smoke succeeds only when:

- runner success is true,
- the selected targeted test passes in the patched repository,
- test output is recorded as debug/output or report evidence,
- Work DAG state was advanced through `team work` and `team report`, not stdout/trace inference.

## 8. Expected Data Flow

```text
human objective
  -> rive runner orchestrator --runner opencode
  -> OpenCode orchestrator
  -> team work create / edge add / inspect
  -> team send --work --runner opencode --wait
  -> OpenCode worker
  -> rive snapshot capture
  -> team status / team report
  -> work node reviewable
  -> team work accept / reopen / follow-up node
  -> root objective done
```

The Work DAG remains the source of work state. Dispatch/delegation remains execution binding. Trace remains debug.

## 9. Phase 9 Done Criteria

Phase 9 is done when:

1. `team work` agent-facing ABI exists and mirrors Phase 8 semantics.
2. `rive runner orchestrator --runner opencode` can start a real OpenCode orchestrator.
3. Fake tests cover graph mutation, delegation, replay, and failure paths.
4. Real OpenCode-only e2e proves orchestrator can create DAG, delegate to OpenCode workers, accept nodes, and finish root.
5. SWE-bench Lite smoke `pytest-dev__pytest-11143` passes through the OpenCode orchestrator flow.
6. `cargo fmt --check`, `cargo test`, and `cargo clippy --all-targets -- -D warnings` pass.
7. Debug trace does not mutate evidence/fact/dispatch/work state.
