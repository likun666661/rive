---
name: rive
description: Use Rive to coordinate local multi-agent software work through ledger-backed Work DAGs and external CLI agents. Use when an agent should translate a user objective into a Work DAG, run OpenCode/Codex workers in parallel, isolate worker file changes with git worktrees, resume scheduler runs, inspect trace/usage, or review worker outputs instead of solving everything in one agent session.
---

# Rive

Rive is a local-first agent team runtime. Treat it as a durable coordination system, not as chat: the Work DAG, dispatch ledger, facts, snapshots, worktree refs, scheduler runs, and debug traces each have different authority.

## Hard Rules

- Only Rive ledger/projection state counts as protocol truth. Do not infer success from stdout, final answers, or debug trace.
- Work topology is semantic work structure. Dispatches, delegations, scheduler node runs, and external CLI processes are execution attempts, not graph edges.
- `team report --status done` makes a work node `reviewable`; it does not make the node `done`.
- A node becomes `done` only through an explicit accept event, either `rive work accept` or a scheduler acceptance policy.
- In worktree mode, worker changes remain isolated until `rive branch commit` or scheduler `auto-committed` applies the worktree patch.
- Debug trace and usage are for diagnosis and cost inspection only. They must not drive work, dispatch, fact, or evidence state.
- Prefer OpenCode as the production runner unless the task explicitly needs Codex coverage.

## When to Use Rive

Use Rive when the request benefits from multiple coordinated agents:

- parallel investigation, such as logs plus code search;
- implementation plus independent review or validation;
- multiple ready tasks that can run concurrently;
- long work that may need `scheduler status` / `scheduler resume`;
- tasks where worker patches must be isolated and explicitly integrated.

Do not use Rive for a tiny single-file edit that one agent can finish faster without orchestration.

## Startup

Work from the repository root.

```sh
rive init .
```

Add worker agents if the workspace does not already have them. Store generated or chosen tokens securely; Rive stores token hashes and later issues run-scoped worker tokens.

```sh
rive agent add opencode-worker-a --role worker
rive agent add opencode-worker-b --role worker
```

Use git worktree isolation for production worker file mutations:

```text
--workspace-mode worktree
```

Use shared workspace mode only for controlled smoke tests or read-only flows.

## Plan the Work DAG

Create a small DAG that a human can inspect. Nodes should represent goals and acceptance criteria, not process attempts.

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
- Use review or judge nodes when results from multiple workers need synthesis.
- Keep node titles actionable and acceptance-oriented.

Create nodes and edges with idempotent command IDs:

```sh
rive work create --kind objective --title "Fix production checkout bug" --command-id plan-root-1
rive work create --kind task --title "Inspect production logs for checkout failures" --command-id plan-logs-1
rive work create --kind task --title "Inspect code path for checkout failures" --command-id plan-code-1
rive work create --kind review --title "Compare log and code findings, decide fix" --command-id plan-judge-1

rive work edge add --type decomposes-to --from <root> --to <logs_node> --command-id edge-root-logs-1
rive work edge add --type decomposes-to --from <root> --to <code_node> --command-id edge-root-code-1
rive work edge add --type decomposes-to --from <root> --to <judge_node> --command-id edge-root-judge-1
rive work edge add --type depends-on --from <judge_node> --to <logs_node> --command-id edge-judge-logs-1
rive work edge add --type depends-on --from <judge_node> --to <code_node> --command-id edge-judge-code-1
```

Inspect before running:

```sh
rive work graph inspect --root <root>
rive work inspect <node>
```

## Run the Scheduler

For most real work, let the scheduler run ready nodes with a bounded OpenCode worker pool.

Manual review mode:

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

Automation mode that integrates reported worktree patches and accepts nodes:

```sh
rive scheduler run \
  --root <root> \
  --runner opencode \
  --worker opencode-worker-a \
  --worker opencode-worker-b \
  --command-id sched-<objective>-auto-1 \
  --max-parallel 2 \
  --acceptance-mode auto-committed \
  --workspace-mode worktree \
  --timeout-seconds 900
```

Interpret scheduler results from `protocol`, not display text. If the run stops:

```sh
rive scheduler status --root <root>
rive scheduler status --run <scheduler_run_id>
rive work graph inspect --root <root>
```

Resume stale or incomplete scheduler work with the same workers and a new command ID:

```sh
rive scheduler resume \
  --run <scheduler_run_id> \
  --worker opencode-worker-a \
  --worker opencode-worker-b \
  --command-id resume-<objective>-1 \
  --max-parallel 2 \
  --acceptance-mode manual \
  --workspace-mode worktree \
  --timeout-seconds 900
```

## Review and Integrate Worker Output

Manual mode leaves reported worker output in `reviewable` state with a pending integration.

Inspect work and integrations:

```sh
rive work inspect <node>
rive branch list
rive branch show <integration_id>
```

Commit a worktree ref into the parent workspace, then accept the node:

```sh
rive branch commit <integration_id> --command-id commit-<node>-1
rive work accept <node> --require-committed-branch --command-id accept-<node>-1
```

Reject or abort bad output explicitly:

```sh
printf '%s\n' "Reason for rejection" | rive branch reject <integration_id> --command-id reject-<node>-1 --stdin
rive branch abort <integration_id> --command-id abort-<node>-1
```

Never manually `git apply`, merge, remove worktrees, or treat a worker's natural-language summary as integration. Rive owns integration events.

## Agent-Facing Delegation

Inside an orchestrator run, use `team send --work` only when direct delegation is needed instead of the scheduler.

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

Success still comes from the worker dispatch projection. `team send --wait` must not consider stdout, final answer, or trace as success.

## Worker Contract

When acting as a worker, modify only `$RIVE_WORKSPACE`. Rive state is in `$RIVE_STATE_WORKSPACE` when running from an isolated worktree.

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

If blocked or failed, report that explicitly:

```sh
printf '%s\n' "Why this is blocked" | team report \
  --dispatch "$RIVE_DISPATCH_ID" \
  --status blocked \
  --command-id blocked-"$RIVE_RUN_ID" \
  --stdin
```

Workers must not mutate Work DAG topology, accept nodes, commit integrations, or claim natural-language success without `team report`.

## Orchestrator Progress

The orchestrator can update the control plane without touching implementation files:

```sh
printf '%s\n' "Created two investigation nodes and one judge node." | team work note <root> \
  --kind progress \
  --command-id note-plan-1 \
  --stdin
```

Use notes for progress, decisions, blockers, risks, and validation rationale. Notes do not prove completion.

## Debug and Usage

Use trace only after a run needs inspection.

```sh
rive debug trace list --agent <agent_id>
rive debug trace list --dispatch <dispatch_id>
rive debug trace usage --root <root>
rive debug trace usage --run <run_id>
```

Usage is a debug read model. It helps estimate cost and tool activity but does not affect business state.

## Failure Handling

- `dispatch_not_reported`: inspect trace and dispatch; rerun or resume, but do not accept.
- `work_scheduler_stalled`: run `rive work graph inspect --root <root>` and address missing requirements.
- `work_graph_not_closed`: fix orphan/unconnected/reviewable nodes before root accept.
- `worktree_ref_not_committed`: commit or reject the pending integration before guarded accept.
- `worktree_backend_unavailable`: ensure the repository supports git worktrees and git is available.
- idempotency conflict: choose a new command ID only when the request is intentionally different.

## Reporting Back

Report ledger-backed results:

- root work ID and final root projection;
- scheduler run ID and state;
- important node states and pending reviewable/blocked nodes;
- dispatch IDs for worker attempts;
- integration IDs and committed refs;
- tests or validation commands run by workers;
- unresolved risks or rejected/aborted integrations.

Do not present a task as complete unless the relevant Work DAG projection is `done` and any requested external validation has passed.
