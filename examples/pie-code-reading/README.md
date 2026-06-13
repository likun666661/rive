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

The second dogfood run produced maintainer-level deep-read artifacts under
[`manual/deep-read/`](./manual/deep-read/). Start with
[`manual/deep-read/00-final-deep-read-guide.md`](./manual/deep-read/00-final-deep-read-guide.md).

The synthesized teaching outline is
[`manual/teaching-manual-outline.md`](./manual/teaching-manual-outline.md). It is
the human-facing course plan built from the coarse and deep-read reports.

| File | Focus |
| --- | --- |
| [`manual/teaching-manual-outline.md`](./manual/teaching-manual-outline.md) | Detailed teaching outline for explaining pie's agent runtime |
| [`manual/00-overview.md`](./manual/00-overview.md) | Cross-module architecture map, deep-read index, and next DAG |
| [`manual/01-ai-provider-streaming.md`](./manual/01-ai-provider-streaming.md) | `pie-ai`, providers, streaming events, DS4/local model path |
| [`manual/02-agent-core-runtime.md`](./manual/02-agent-core-runtime.md) | `pie-agent-core`, harness, session, compaction, agent loop |
| [`manual/03-coding-cli-tools.md`](./manual/03-coding-cli-tools.md) | CLI/TUI/tools/config/session user-facing surface |
| [`manual/04-automation-loops-triggers.md`](./manual/04-automation-loops-triggers.md) | Cron, stateful loops, triggers, inbox, hooks, observability |
| [`manual/05-mcp-and-fefe-hub.md`](./manual/05-mcp-and-fefe-hub.md) | MCP client/server paths and Cloudflare `fefe-hub` worker |
| [`manual/06-roadmap-docs-issues.md`](./manual/06-roadmap-docs-issues.md) | Docs/issues roadmap and product architecture signals |

Deep-read outputs:

| File | Focus |
| --- | --- |
| [`manual/deep-read/00-final-deep-read-guide.md`](./manual/deep-read/00-final-deep-read-guide.md) | Final maintainer guide, risk register, and follow-up DAG |
| [`manual/deep-read/01-trigger-state-machine.md`](./manual/deep-read/01-trigger-state-machine.md) | Trigger envelope, runtime state machine, side effects |
| [`manual/deep-read/02-loop-inbox-internals.md`](./manual/deep-read/02-loop-inbox-internals.md) | Stateful loop tag parsing and inbox lifecycle |
| [`manual/deep-read/03-session-branch-model.md`](./manual/deep-read/03-session-branch-model.md) | JSONL session parent DAG, compaction, resume/export integrity |
| [`manual/deep-read/04-lsp-integration-report.md`](./manual/deep-read/04-lsp-integration-report.md) | LSP supervisor and after-tool diagnostic injection |
| [`manual/deep-read/05-tool-call-parsing-matrix.md`](./manual/deep-read/05-tool-call-parsing-matrix.md) | Cross-provider streaming tool-call argument parsing |
| [`manual/deep-read/06-goal-evaluator-internals.md`](./manual/deep-read/06-goal-evaluator-internals.md) | `/goal` evaluator and `OnTurnEndHook` loop |
| [`manual/deep-read/07-automation-security-audit.md`](./manual/deep-read/07-automation-security-audit.md) | Trigger + loop automation threat model |
| [`manual/deep-read/08-session-integrity-review.md`](./manual/deep-read/08-session-integrity-review.md) | Session durability and sidecar consistency review |
| [`manual/deep-read/09-provider-conformance-test-plan.md`](./manual/deep-read/09-provider-conformance-test-plan.md) | Provider conformance mock-server and fixture plan |

## Actual Run

### Coarse Read

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

### Deep Read

- Source repository: `/Users/likun/Desktop/workspace-for-pie-agent/pie`
- Source ref: `f1c35a3`
- Workflow run: `wfrun_d78f71feae1148ac936019876a0695cc`
- Scheduler run: `sched_eba15eabdcdd44fe8ff152926de57f95`
- Root work: `work_fd69dd07af6548d6ad8e07afe55c830f`
- Runner: OpenCode
- Worker shape: 6 source deep-read nodes, 3 review nodes, 1 final guide node
- Parallelism: `max_parallel=3`
- Acceptance mode: `auto-reported`
- Workspace mode: `shared`, because the workflow is read-only and writes only
  external Markdown artifacts
- Result: workflow `completed`, root work `done`, graph hygiene `clean`, 10
  scheduler node-runs accepted, 0 scheduler failures

## Workflow

Validate the package:

```sh
rive workflow validate examples/pie-code-reading/workflows/coarse-read
rive workflow validate examples/pie-code-reading/workflows/deep-read
```

Run without starting workers:

```sh
rive workflow run pie.coarse-read \
  --command-id run-pie-coarse-read-dry \
  --no-scheduler \
  --param repo_path=/Users/likun/Desktop/workspace-for-pie-agent/pie \
  --param output_dir=/tmp/rive-pie-code-reading
```

Instantiate the deep-read DAG without starting workers:

```sh
rive workflow run pie.deep-read \
  --command-id run-pie-deep-read-dry \
  --no-scheduler \
  --param repo_path=/Users/likun/Desktop/workspace-for-pie-agent/pie \
  --param output_dir=/tmp/rive-pie-deep-read
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

Run the deep-read workflow with OpenCode workers:

```sh
rive workflow run pie.deep-read \
  --command-id run-pie-deep-read \
  --runner opencode \
  --worker opencode-reader-a \
  --worker opencode-reader-b \
  --worker opencode-reader-c \
  --max-parallel 3 \
  --acceptance-mode auto-reported \
  --workspace-mode shared \
  --timeout-seconds 2400 \
  --param repo_path=/Users/likun/Desktop/workspace-for-pie-agent/pie \
  --param output_dir=/tmp/rive-pie-deep-read
```

`shared` workspace mode is acceptable here because the workflow is read-only and
all writes are restricted to the external output directory. Use `worktree` for
implementation workflows.
