# Maka Code Reading Dogfood

This example uses Rive to coordinate a coarse code reading pass over a local
copy of Maka, an Electron desktop assistant agent.

The reading workflow is intentionally split by architectural boundary:

- `packages/core` schema and contract layer
- `packages/runtime` model backend, tool execution, permissions, bots, and telemetry
- `packages/storage` local persistence
- `apps/desktop/src/main` and preload IPC
- `apps/desktop/src/renderer` plus `packages/ui`
- docs, notes, tests, scripts, and product roadmap
- final overview that consumes the reader outputs

Every reader node is read-only and writes one Markdown artifact. The coarse
workflow final overview node reads those artifacts and produces the architecture
map. The deep-read workflow then expands the highest-risk areas into
maintainer-level notes and a prioritized repair roadmap.

## Output

The dogfood run in this repository produces Markdown artifacts under
[`manual/`](./manual/). Start with [`manual/00-overview.md`](./manual/00-overview.md),
then read the individual reader reports as needed.

| File | Focus |
| --- | --- |
| [`manual/00-overview.md`](./manual/00-overview.md) | Cross-module architecture map, deep-read index, and next DAG |
| [`manual/01-core-contracts.md`](./manual/01-core-contracts.md) | `packages/core` schemas, events, permissions, settings, and contracts |
| [`manual/02-runtime-backends-tools.md`](./manual/02-runtime-backends-tools.md) | `packages/runtime` session manager, AI SDK backend, tools, permissions, bots, telemetry |
| [`manual/03-storage-persistence.md`](./manual/03-storage-persistence.md) | `packages/storage` stores, JSON/session persistence, settings, telemetry repos |
| [`manual/04-desktop-main-ipc.md`](./manual/04-desktop-main-ipc.md) | Electron main process, preload API, IPC, credential storage, local services |
| [`manual/05-renderer-ui.md`](./manual/05-renderer-ui.md) | Renderer UI and `packages/ui` components, assistant stream, artifact preview |
| [`manual/06-docs-tests-roadmap.md`](./manual/06-docs-tests-roadmap.md) | README, docs, historical notes, tests, scripts, and product direction |

The deep-read workflow produced a second set of artifacts under
[`manual/deep-read/`](./manual/deep-read/). Start with
[`manual/deep-read/00-final-deep-read-guide.md`](./manual/deep-read/00-final-deep-read-guide.md).
The follow-up implementation roadmap is in [`plan.md`](./plan.md).

| File | Focus |
| --- | --- |
| [`manual/deep-read/00-final-deep-read-guide.md`](./manual/deep-read/00-final-deep-read-guide.md) | Synthesized maintainer guide, P0/P1/P2 findings, roadmap, teaching outline, next DAG |
| [`manual/deep-read/01-permission-tool-safety.md`](./manual/deep-read/01-permission-tool-safety.md) | Permission matrix, `wrapToolExecute`, watchdog pause/resume, parked approvals |
| [`manual/deep-read/02-ipc-surface-security.md`](./manual/deep-read/02-ipc-surface-security.md) | Main/preload IPC handler surface, runtime validation gaps, renderer escalation paths |
| [`manual/deep-read/03-path-containment.md`](./manual/deep-read/03-path-containment.md) | Workspace path containment helpers, platform edge cases, `realpath` assumptions |
| [`manual/deep-read/04-credential-settings-security.md`](./manual/deep-read/04-credential-settings-security.md) | `safeStorage` credential store, settings secrets, migration plan |
| [`manual/deep-read/05-bot-gateway-attack-surface.md`](./manual/deep-read/05-bot-gateway-attack-surface.md) | Bot bridge and OpenGateway inbound/session/SSE/token attack surface |
| [`manual/deep-read/06-memory-gates.md`](./manual/deep-read/06-memory-gates.md) | 9-gate memory contract, runtime integration gaps, privacy gates |
| [`manual/deep-read/07-jsonl-durability.md`](./manual/deep-read/07-jsonl-durability.md) | Session JSONL durability, corruption handling, migration and recovery |
| [`manual/deep-read/08-telemetry-cost.md`](./manual/deep-read/08-telemetry-cost.md) | LLM/tool telemetry, cache and reasoning token accounting, write loss windows |
| [`manual/deep-read/09-external-tool-injection.md`](./manual/deep-read/09-external-tool-injection.md) | Rive/office/explore external tool invocation, injection and cleanup policies |
| [`manual/deep-read/10-visual-smoke-test-infra.md`](./manual/deep-read/10-visual-smoke-test-infra.md) | Visual smoke scripts, screenshot gates, accessibility and CI strategy |

After the session/runtime hardening commits, a focused current-state workflow
produced a third set of artifacts under
[`manual/session-ai-sdk-current/`](./manual/session-ai-sdk-current/). Start with
[`manual/session-ai-sdk-current/00-current-state-report.md`](./manual/session-ai-sdk-current/00-current-state-report.md).

| File | Focus |
| --- | --- |
| [`manual/session-ai-sdk-current/00-current-state-report.md`](./manual/session-ai-sdk-current/00-current-state-report.md) | Maintainer report for current session + AI SDK backend state |
| [`manual/session-ai-sdk-current/01-session-lifecycle.md`](./manual/session-ai-sdk-current/01-session-lifecycle.md) | SessionManager lifecycle, status transitions, abort/recover behavior |
| [`manual/session-ai-sdk-current/02-ai-sdk-backend-refactor.md`](./manual/session-ai-sdk-current/02-ai-sdk-backend-refactor.md) | AI SDK backend split into ModelAdapter, ToolRuntime, and RunTrace |
| [`manual/session-ai-sdk-current/03-desktop-session-bridge.md`](./manual/session-ai-sdk-current/03-desktop-session-bridge.md) | Desktop main/preload IPC, credentials, connection readiness bridge |
| [`manual/session-ai-sdk-current/04-storage-trace-recovery.md`](./manual/session-ai-sdk-current/04-storage-trace-recovery.md) | Session JSONL recovery, telemetry repo, artifact store, trace persistence gap |
| [`manual/session-ai-sdk-current/05-bot-gateway-regression.md`](./manual/session-ai-sdk-current/05-bot-gateway-regression.md) | Bot/OpenGateway abuse controls and session entry risks |

## Coarse Run

- Source repository: `/Users/likun/Desktop/workspace-for-maka/maka`
- Source ref: `335220a`
- Workflow run: `wfrun_97f4c47f379e4f6e894d0ecedeabff5b`
- Scheduler run: `sched_98d54ef93658454d8a536e0d0339d600`
- Root work: `work_3007c7a994734b06be4d7b298794851a`
- Runner: OpenCode
- Worker shape: 6 reader nodes + 1 final overview node
- Parallelism: `max_parallel=3`
- Acceptance mode: `auto-reported`
- Workspace mode: `shared`, because the workflow is read-only and writes only
  external Markdown artifacts
- Result: workflow `completed`, root work `done`, graph hygiene `clean`, 7
  scheduler node-runs accepted, 0 scheduler failures

## Deep Run

- Source repository: `/Users/likun/Desktop/workspace-for-maka/maka`
- Source ref: `335220a`
- Workflow run: `wfrun_094cfc44d665449b935549965cc46476`
- Initial scheduler run: `sched_3eeb709a829f4d65aaa4aa85fde03607`
- Recovery scheduler run: `sched_f72afcbcdfe741f7b4b02793cbf30aae`
- Root work: `work_6d70dfa7a88f44ca9c7642567523e590`
- Runner: OpenCode
- Worker shape: 10 focused reader nodes + 1 final maintainer guide node
- Parallelism: `max_parallel=3`
- Acceptance mode: `auto-reported`
- Workspace mode: `shared`, because the workflow is read-only and writes only
  external Markdown artifacts
- Result: initial scheduler hit one OpenCode certificate failure, classified as
  `certificate_error` with `retry_after_certificate_fix`; `rive scheduler resume --failed`
  superseded the failed attempt, preserved the trace, and completed with root
  work `done`, graph hygiene `clean`, 11 accepted node-runs, and 1 superseded
  failed node-run

## Workflow

Validate the package:

```sh
rive workflow validate examples/maka-code-reading/workflows/coarse-read
rive workflow validate examples/maka-code-reading/workflows/deep-read
```

Run without starting workers:

```sh
rive workflow run maka.coarse-read \
  --command-id run-maka-coarse-read-dry \
  --no-scheduler \
  --param repo_path=/Users/likun/Desktop/workspace-for-maka/maka \
  --param output_dir=/tmp/rive-maka-code-reading
```

Run with OpenCode workers:

```sh
rive workflow run maka.coarse-read \
  --command-id run-maka-coarse-read \
  --runner opencode \
  --worker opencode-reader-a \
  --worker opencode-reader-b \
  --worker opencode-reader-c \
  --max-parallel 3 \
  --acceptance-mode auto-reported \
  --workspace-mode shared \
  --timeout-seconds 1800 \
  --param repo_path=/Users/likun/Desktop/workspace-for-maka/maka \
  --param output_dir=/tmp/rive-maka-code-reading
```

`shared` workspace mode is acceptable here because the workflow is read-only and
all writes are restricted to the external output directory. Use `worktree` for
implementation workflows.

Run the deep-read workflow:

```sh
rive workflow run maka.deep-read \
  --command-id run-maka-deep-read \
  --runner opencode \
  --worker opencode-reader-a \
  --worker opencode-reader-b \
  --worker opencode-reader-c \
  --max-parallel 3 \
  --acceptance-mode auto-reported \
  --workspace-mode shared \
  --timeout-seconds 3600 \
  --param repo_path=/Users/likun/Desktop/workspace-for-maka/maka \
  --param output_dir=/tmp/rive-maka-deep-read/deep-read \
  --param source_ref=335220a \
  --param depth=maintainer
```

Run the current session + AI SDK backend analysis workflow:

```sh
rive workflow validate examples/maka-code-reading/workflows/session-ai-sdk-current

rive workflow run maka.session-ai-sdk-current \
  --command-id run-maka-session-ai-sdk-current \
  --runner opencode \
  --worker opencode-reader-a \
  --worker opencode-reader-b \
  --worker opencode-reader-c \
  --max-parallel 3 \
  --acceptance-mode auto-reported \
  --workspace-mode shared \
  --timeout-seconds 2400 \
  --param repo_path=/Users/likun/Desktop/workspace-for-maka/maka \
  --param output_dir=/tmp/rive-maka-session-ai-sdk-current \
  --param source_ref=4dd1bf1 \
  --param previous_ref=05ca5a3 \
  --param depth=maintainer
```

## Session + AI SDK Current Run

- Source repository: `/Users/likun/Desktop/workspace-for-maka/maka`
- Source ref: `4dd1bf1`
- Previous ref: `05ca5a3`
- Workflow run: `wfrun_55c1450bf8304fb784f5238be9dbfc05`
- Scheduler run: `sched_e04c42f9b70344698a906980f54f43e5`
- Root work: `work_c30a53ce6a0741119bbdb254cf8270f9`
- Runner: OpenCode
- Worker shape: 5 focused reader nodes + 1 final current-state report node
- Parallelism: `max_parallel=3`
- Acceptance mode: `auto-reported`
- Workspace mode: `shared`, because the workflow is read-only and writes only
  external Markdown artifacts
- Result: workflow `completed`, root work `done`, graph hygiene `clean`, 6
  scheduler node-runs accepted, 0 scheduler failures
- Dogfood note: an earlier attempt used an old Rive debug binary from 2026-06-09
  and left a stale interrupted scheduler run. Rebuilding Rive before rerun fixed
  runner child collection; this run is the authoritative one.
