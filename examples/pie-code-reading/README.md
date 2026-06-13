# pie Code Reading Dogfood

This example uses Rive to coordinate a coarse code reading pass over
[`pie`](https://github.com/c4pt0r/pie), a Rust terminal AI coding agent.

The reading workflow is intentionally split by architectural boundary:

- `pie-ai` provider/model/streaming runtime
- `pie-agent-core` harness, session, compaction, and lifecycle runtime
- `pie-coding-agent` CLI/TUI/tools/config surface
- triggers, cron loops, inbox, hooks, and long-running automation
- MCP client/server integration plus the `fefe-hub` worker
- roadmap/design docs and unresolved issue map
- final overview that consumes the reader outputs

Every reader node is read-only and writes one Markdown artifact. The final
overview node reads those artifacts and produces the coarse architecture map.

## Output

The dogfood run in this repository produced Markdown artifacts under
[`manual/`](./manual/). Start with [`manual/00-overview.md`](./manual/00-overview.md),
then read the individual reader reports as needed.

| File | Focus |
| --- | --- |
| [`manual/00-overview.md`](./manual/00-overview.md) | Cross-module architecture map, deep-read index, and next DAG |
| [`manual/01-ai-provider-streaming.md`](./manual/01-ai-provider-streaming.md) | `pie-ai`, providers, streaming events, DS4/local model path |
| [`manual/02-agent-core-runtime.md`](./manual/02-agent-core-runtime.md) | `pie-agent-core`, harness, session, compaction, agent loop |
| [`manual/03-coding-cli-tools.md`](./manual/03-coding-cli-tools.md) | CLI/TUI/tools/config/session user-facing surface |
| [`manual/04-automation-loops-triggers.md`](./manual/04-automation-loops-triggers.md) | Cron, stateful loops, triggers, inbox, hooks, observability |
| [`manual/05-mcp-and-fefe-hub.md`](./manual/05-mcp-and-fefe-hub.md) | MCP client/server paths and Cloudflare `fefe-hub` worker |
| [`manual/06-roadmap-docs-issues.md`](./manual/06-roadmap-docs-issues.md) | Docs/issues roadmap and product architecture signals |

## Actual Run

- Source repository: `/Users/likun/Desktop/workspace-for-pie-agent/pie`
- Source ref: `f1c35a3`
- Workflow run: `wfrun_8ca84b3e6b2f4ab5929694be0f9b13e1`
- Scheduler run: `sched_47a71dae5c114f52bb9a8070c348a697`
- Root work: `work_b208392cc8c1483788797b69aa33cb07`
- Runner: OpenCode
- Worker shape: 6 reader nodes + 1 final overview node
- Parallelism: `max_parallel=3`
- Acceptance mode: `auto-reported`
- Workspace mode: `shared`, because the workflow is read-only and writes only
  external Markdown artifacts
- Result: workflow `completed`, root work `done`, graph hygiene `clean`

## Workflow

Validate the package:

```sh
rive workflow validate examples/pie-code-reading/workflows/coarse-read
```

Run without starting workers:

```sh
rive workflow run pie.coarse-read \
  --command-id run-pie-coarse-read-dry \
  --no-scheduler \
  --param repo_path=/Users/likun/Desktop/workspace-for-pie-agent/pie \
  --param output_dir=/tmp/rive-pie-code-reading
```

Run with OpenCode workers:

```sh
rive workflow run pie.coarse-read \
  --command-id run-pie-coarse-read \
  --runner opencode \
  --worker opencode-reader-a \
  --worker opencode-reader-b \
  --worker opencode-reader-c \
  --max-parallel 3 \
  --acceptance-mode auto-reported \
  --workspace-mode shared \
  --timeout-seconds 1200 \
  --param repo_path=/Users/likun/Desktop/workspace-for-pie-agent/pie \
  --param output_dir=/tmp/rive-pie-code-reading
```

`shared` workspace mode is acceptable here because the workflow is read-only and
all writes are restricted to the external output directory. Use `worktree` for
implementation workflows.
