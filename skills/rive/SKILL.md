---
name: rive
description: Use Rive to coordinate local multi-agent software work through ledger-backed Work DAGs, reusable workflow templates, node-level OpenCode/Codex runner policy, git-worktree isolation, scheduler observability, resume/retry, conflict recovery, failure classification, and neutral debug trace/usage. Use when an agent should plan a DAG, run mixed low-cost/strong worker pools, preserve artifacts and refs, inspect worker activity, recover failed/conflicted nodes, or repeat a workflow instead of solving everything in one session.
---

# Rive

Rive is a local-first runtime for multi-agent engineering work. Treat it as a
durable coordination system, not as chat: Work DAGs, workflow templates,
dispatches, facts, snapshots, worktree refs, scheduler runs, and debug traces
each have different authority.

## Core Rules

- Only Rive ledger/projection state counts as protocol truth. Do not infer
  success from stdout, final answers, or debug trace.
- Work DAG topology is semantic work structure. Dispatches, delegations,
  scheduler node runs, and external CLI processes are execution attempts, not
  graph edges.
- `team report --status done` makes a work node `reviewable`; it does not make
  the node `done`.
- A node becomes `done` only through explicit acceptance: `rive work accept`, a
  scheduler acceptance policy, or a workflow run that delegates to that policy.
- In worktree mode, worker file changes stay isolated until `rive branch commit`
  or scheduler `auto-committed` applies the patch.
- Debug trace, usage, stdout/stderr, and status activity are diagnostic only.
  They must never drive work, dispatch, fact, evidence, branch, scheduler, or
  workflow state.
- Trust the base model's exploration unless ledger state says it failed. Do not
  invent path-hit ratios, off-track/runaway judgments, or heartbeat rules.
- Prefer OpenCode for low-cost production worker pools. Use Codex when the task
  requires Codex coverage, Codex-specific behavior, or the user explicitly asks.
- In reusable workflows, put runner choice on the node when nodes need different
  agents. Use run-level `--runner` only as a default fallback.
- Do not manually edit SQLite or patch around Rive state. Use retry, resume,
  branch, work, and workflow commands.

## Capability Map

Use this map to choose the right layer:

- Evidence/facts: `rive snapshot capture/list/show`, `rive evidence list`,
  `team fact record`, `rive fact list/show`.
- Agents/dispatches: `rive agent add/list/show`,
  `rive dispatch create/list/show/cancel`, `team status/report/list`.
- Debug: `rive debug trace ingest/list/show/session/export/install/uninstall`
  and `rive debug trace usage`.
- Runners: `rive runner opencode`, `rive runner codex`,
  `rive runner orchestrator --runner opencode`.
- Work DAG: `rive work create/list/show/inspect/edge add/accept/reopen/retry`,
  `rive work graph inspect`, `team work create/edge/list/show/inspect/note`.
- Delegation/scheduler: `team send --work <work_id> --wait`,
  `rive scheduler run/status/resume`, `rive scheduler resume --failed`.
- Worktree refs/conflicts: `rive branch list/show/commit/abort/reject`,
  `rive branch conflict show/reject/retry-from-parent`.
- Workflows: `rive workflow validate/import/list/show/run/status`; node specs
  may carry `runner`, `worker`, `workspace_mode`, and `acceptance_mode`.

## When to Use Rive

Use Rive when the request benefits from durable multi-agent coordination:

- parallel investigation, such as logs plus code search plus final judge;
- implementation plus independent review, test, changelog, and merge nodes;
- long-running work that may need `scheduler status`, `scheduler resume`, or
  `work retry`;
- repeated operational workflows that should become one-command templates;
- tasks where worker patches must be isolated and explicitly integrated;
- dogfood where trace, artifact refs, and retry history matter.

Do not use Rive for a tiny one-agent edit that is faster without orchestration.

## Setup

```sh
rive init .
```

Add worker agents when the workspace does not already have them. Rive stores
token hashes and later issues run-scoped worker tokens.

```sh
rive agent add opencode-worker-a --role worker
rive agent add opencode-worker-b --role worker
rive agent add codex-worker-a --role worker
```

For production file mutation, prefer git worktree isolation:

```text
--workspace-mode worktree
```

Use shared workspace mode only for controlled smoke tests, read-only flows, or
cases where the user intentionally wants all workers in the same tree.

Worktrees are seeded from the current parent workspace baseline, including
accepted but uncommitted parent changes. Workers still edit only their isolated
`$RIVE_WORKSPACE`.

## Plan a Work DAG

Create a small DAG that a human can inspect. Nodes should represent goals and
acceptance criteria, not process attempts.

Common shape:

```text
root objective
  decomposes_to investigation A
  decomposes_to investigation B
  decomposes_to implementation or judge
judge depends_on A
judge depends_on B
```

Rules:

- Keep the graph acyclic.
- Use `decomposes-to` for parent-to-child structure.
- Use `depends-on` from the blocked node to its prerequisite.
- Prefer narrow worker nodes with concrete artifact expectations.
- Use judge/review nodes when multiple worker outputs need synthesis.
- Avoid a giant synthesis node unless it can itself use Rive to spawn another
  DAG; otherwise its context window becomes the bottleneck.

Inspect before running:

```sh
rive work graph inspect --root <root>
rive work inspect <node>
```

## Run the Scheduler

For most real work, run ready nodes with a bounded worker pool.

```sh
rive scheduler run \
  --root <root> \
  --runner opencode \
  --worker opencode-worker-a \
  --worker opencode-worker-b \
  --command-id sched-<objective>-1 \
  --max-parallel 2 \
  --acceptance-mode manual \
  --workspace-mode worktree \
  --timeout-seconds 900
```

Use `--acceptance-mode auto-committed` to apply reported worktree patches and
accept nodes. Codex runs use `--runner codex --worker <codex-worker>
--trust-project`; Codex gets isolated per-run `CODEX_HOME` and must not mutate
global `~/.codex/config.toml`.

When a workflow node declares runner policy, it overrides run-level defaults.
One scheduler run can execute cheap OpenCode nodes and stronger Codex
judge/merge nodes in the same DAG:

```yaml
nodes:
  implement:
    kind: task
    runner: opencode
    worker: opencode-worker-a
    workspace_mode: worktree
    acceptance_mode: auto-committed
  judge:
    kind: review
    runner: codex
    worker: codex-worker-a
    acceptance_mode: auto-reported
    consumes: [implement]
edges:
  - { type: decomposes_to, from: root, to: implement }
  - { type: decomposes_to, from: root, to: judge }
  - { type: depends_on, from: judge, to: implement }
```

`scheduler_node_runs` records each node's resolved runner, worker, workspace
mode, and acceptance mode. Retry/resume preserves node policy; changing policy
under the same command ID is an `idempotency_conflict`.

Interpret scheduler results from `protocol`, not display text:

```sh
rive scheduler status --root <root>
rive scheduler status --run <scheduler_run_id>
rive work graph inspect --root <root>
```

For live or failed runs, inspect scheduler status before deciding. It exposes
`protocol.node_runs[].activity` with prompt/stdout/stderr refs and tails, branch
ref/path, changed files, recent trace summaries, and structured
`activity.trace.samples`; failures include `failure_kind`, `retryable`,
`suggested_action`, and detail. These are neutral observability only.

## Reusable Workflow Templates

Use workflows when a successful DAG should be repeated later with one command.
A workflow package is the durable product asset:

```text
workflow.yaml
prompts/<node_template_id>.md
```

Important semantics:

- `workflow import` registers an immutable template version and has zero business
  side effects.
- `template_hash` covers the normalized workflow spec and prompt bytes.
- If prompt bytes changed, import with `--bump-if-changed`; do not assume editing
  a prompt mutates an existing version.
- `workflow run --no-scheduler` instantiates the Work DAG only.
- Plain `workflow run` instantiates the Work DAG, starts the scheduler, records
  `workflow_runs.scheduler_run_id`, and returns effective state.
- Node-level runner/worker/workspace/acceptance policy is part of the workflow
  template hash. Changing policy should create or bump a template version.
- Use `workflow status --run` for effective state and debug refs; do not read raw
  `workflow_runs.state` directly.

Typical package flow:

```sh
rive workflow validate examples/workflows/sentinel-prod-debug
rive workflow import examples/workflows/sentinel-prod-debug \
  --command-id workflow-import-sentinel-$(date +%Y%m%d%H%M%S) \
  --bump-if-changed

rive workflow run sentinel.prod-debug \
  --param env=prd \
  --param window=1h \
  --param slack_channel=dry-run \
  --command-id sentinel-prod-debug-$(date +%Y%m%d%H%M%S) \
  --runner codex \
  --worker codex-worker-a \
  --max-parallel 3 \
  --acceptance-mode auto-reported \
  --workspace-mode shared \
  --trust-project \
  --timeout-seconds 1800

rive workflow status --run <workflow_run_id>
```

If the template has node-level runner policy, run-level `--runner` and
`--worker` values are defaults only for nodes without explicit policy.

The Sentinel example in `examples/workflows/sentinel-prod-debug/` models:
global signal scan, parallel service investigations for `alva-backend`,
`alfs`, `jagent`, and `alva-gateway`, then a final judge/Slack draft node.
GitHub and Slack side effects are gated/dry-run by default.

For cron or VM automation, use an external wrapper example rather than baking
product-specific policy into Rive:

```text
examples/workflows/sentinel-prod-debug/run-sentinel-cron.sh
examples/workflows/sentinel-prod-debug/crontab.example
```

The wrapper should reconcile templates with `--bump-if-changed`, check
`workflow status --run`, resume incomplete runs, and avoid overlapping runs.

## Review and Integrate Worker Output

Manual mode leaves worker output in `reviewable` with pending refs.

Inspect work and integrations:

```sh
rive work inspect <node>
rive branch list
rive branch show <integration_id>
```

Commit a worktree ref into the parent workspace, then accept:

```sh
rive branch commit <integration_id> --command-id commit-<node>-1
rive work accept <node> --require-committed-branch --command-id accept-<node>-1
```

Reject or abort explicitly:

```sh
printf '%s\n' "Reason for rejection" | rive branch reject <integration_id> --command-id reject-<node>-1 --stdin
rive branch abort <integration_id> --command-id abort-<node>-1
```

If patch application conflicts, inspect the structured conflict. Parent files
must not be half-applied.

```sh
rive branch conflict show <conflict_id>
printf '%s\n' "Reject conflicting worker patch" | rive branch conflict reject <conflict_id> \
  --command-id reject-conflict-<node>-1 \
  --stdin
```

`conflict show` separates business conflict files from runtime pollution and
suggests actions. To discard the conflicted branch and rerun the same work node
from the current parent baseline:

```sh
rive branch conflict retry-from-parent <conflict_id> \
  --worker opencode-worker-a \
  --command-id retry-conflict-<node>-1 \
  --acceptance-mode auto-committed \
  --workspace-mode worktree \
  --timeout-seconds 900
```

Use `open-conflict`/manual inspection only when a human needs to review the
worktree path. Do not hide conflict handling inside a worker summary.

## Retry and Resume

Use Phase 16 recovery commands instead of manual SQL or ad hoc dispatch edits.

Retry a failed scheduler run and only rerun failed/active attempts:

```sh
rive scheduler resume \
  --run <scheduler_run_id> \
  --failed \
  --worker opencode-worker-a \
  --worker opencode-worker-b \
  --command-id resume-failed-<objective>-1 \
  --max-parallel 2 \
  --acceptance-mode auto-committed \
  --workspace-mode worktree \
  --timeout-seconds 900
```

Retry a single work node:

```sh
rive work retry <work_node_id> \
  --worker opencode-worker-a \
  --command-id retry-<work_node_id>-1 \
  --acceptance-mode manual \
  --workspace-mode worktree \
  --timeout-seconds 900
```

Recovery semantics:

- stale failed/active attempts are marked `superseded`;
- old open/blocked dispatches are cancelled;
- failure trace and refs remain inspectable;
- replay of the retry command does not start another child process;
- same command ID with different retry parameters returns
  `idempotency_conflict` before superseding/cancelling anything.

Failure records include `failure_kind`, `retryable`, `suggested_action`, and
`detail`. Common kinds include `certificate_error`, `network_error`,
`model_error`, `binary_not_found`, `timeout`, `process_exit`,
`dispatch_not_reported`, and `worktree_patch_conflict`.

Runner stdout/stderr can enrich failure diagnosis, especially no-report
environment errors, but it still never counts as success.

## Agent-Facing Delegation

Inside an orchestrator run, use `team send --work` only when direct delegation is
needed instead of the scheduler.

```sh
printf '%s\n' "Worker instructions" | team send \
  --work <work_node_id> \
  --to opencode-worker-a \
  --runner opencode \
  --title "Investigate checkout logs" \
  --command-id send-<node>-1 \
  --wait \
  --stdin
```

`team send --wait` succeeds only when the worker dispatch projection reports.
It must not consider stdout, final answer, or trace a success signal.

## Worker Contract

When acting as a worker, modify only `$RIVE_WORKSPACE`. In worktree mode, Rive
state lives in `$RIVE_STATE_WORKSPACE` and worker changes are reported through
`$RIVE_WORKSPACE_REF`.

Minimal worker completion:

```sh
SNAPSHOT_ID=$(rive snapshot capture --label done | jq -r '.protocol.snapshot_id')
printf '%s\n' "What changed and how it was checked" | team report \
  --dispatch "$RIVE_DISPATCH_ID" \
  --status done \
  --snapshot "$SNAPSHOT_ID" \
  --workspace-ref "$RIVE_WORKSPACE_REF" \
  --command-id report-"$RIVE_RUN_ID" \
  --stdin
```

If blocked or failed, report explicitly:

```sh
printf '%s\n' "Why this is blocked" | team report \
  --dispatch "$RIVE_DISPATCH_ID" \
  --status blocked \
  --command-id blocked-"$RIVE_RUN_ID" \
  --stdin
```

Workers must not accept work nodes, commit integrations, mutate orchestrator
root state, or claim natural-language success without `team report`.

## Orchestrator Control Plane

The orchestrator can update progress without touching implementation files:

```sh
printf '%s\n' "Created two investigation nodes and one judge node." | team work note <root> \
  --kind progress \
  --command-id note-plan-1 \
  --stdin
```

Use notes for progress, decisions, blockers, risks, and validation rationale.
Notes do not prove completion.

If the orchestrator is restricted to planning/control-plane work, workers should
perform workspace mutations and tests. Rive enforces this with planner PATH and
mutation audit in orchestrator runner flows.

## Debug and Usage

Use trace after a run needs inspection.

```sh
rive debug trace list --agent <agent_id>
rive debug trace list --dispatch <dispatch_id>
rive debug trace usage --root <root>
rive debug trace usage --run <run_id>
```

Use workflow status for higher-level failure inspection:

```sh
rive workflow status --run <workflow_run_id>
```

Status responses include effective state, scheduler-node prompt/stdout/stderr
refs, and recent neutral trace samples such as tool calls, session status, file
changes, command errors, and text previews. Use them to understand activity,
never to mark success or label a run off-track by heuristic.

## Common Failure Handling

- `certificate_error`: fix cert/proxy/runtime environment, then
  `scheduler resume --failed` or `work retry`.
- `network_error` or `model_error`: fix the external runner environment or
  model config, then retry.
- `binary_not_found`: fix `PATH` or pass `--opencode-bin` / `--codex-bin`.
- `dispatch_not_reported`: inspect trace and failure detail; retry or report a
  worker bug, but do not accept.
- `work_scheduler_stalled`: inspect `rive work graph inspect --root <root>` and
  resolve missing requirements.
- `work_graph_not_closed`: fix orphan, unconnected, incomplete, or unaccepted
  reviewable nodes before root accept.
- `worktree_ref_not_committed`: commit or reject the pending integration before
  guarded accept.
- `worktree_patch_conflict`: inspect `rive branch conflict show`, then
  `retry-from-parent`, reject, or manually review the branch path.
- `idempotency_conflict`: use a new command ID only when the request is
  intentionally different.

## Reporting Back

Report ledger-backed results:

- workflow run ID and effective state, when using workflows;
- root work ID and final root projection;
- scheduler run ID and state;
- important node states and pending reviewable/blocked nodes;
- dispatch IDs for worker attempts;
- integration IDs, conflict IDs, and committed refs;
- activity/failure summary from scheduler status when a run needed diagnosis;
- validation commands run by workers or final judge;
- unresolved risks, failures, rejected integrations, or retry decisions.

Do not present a task as complete unless the relevant Work DAG projection is
`done` and any requested external validation has passed.
