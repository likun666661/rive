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

Every reader node is read-only and writes one Markdown artifact. The final
overview node reads those artifacts and produces the coarse architecture map.

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

## Actual Run

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

## Workflow

Validate the package:

```sh
rive workflow validate examples/maka-code-reading/workflows/coarse-read
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
