# Eino Technical Manual Dogfood Example

This example is a real Rive dogfood run: a human asked Rive to read the
CloudWeGo Eino repository, split the reading work into a Work DAG, launch
OpenCode workers, and produce a detailed technical manual.

The output is in [`manual/`](./manual/).

## What This Demonstrates

- A broad research/documentation request can be translated into a Work DAG.
- Multiple OpenCode workers can read different code areas in parallel.
- Each worker writes a concrete artifact instead of only returning prose.
- Worker completion moves Work nodes to `reviewable`; explicit accept moves
  them to `done`.
- A second, more granular DAG can reuse coarse first-pass reports as seed
  material when the first output is too shallow.

## Run Shape

The final manual was produced by a second-pass chapter-level DAG:

- Root: `work_616202594ad9431fb1bd763001620d4c`
- Scheduler: `sched_1338d328fc5245f486063f823ceeef93`
- Runner: OpenCode
- Acceptance mode: manual
- Output directory in the dogfood workspace:
  `docs/rive-eino-manual-v2/`

The first coarse DAG produced useful notes, but the output was too shallow.
The useful lesson was not to create one huge synthesis node. Instead, Rive
created chapter-level artifact nodes with hard contracts:

- exact output file path;
- minimum depth / line count;
- source file references;
- problem / why hard / design idea / source walkthrough / patterns / pitfalls;
- manual accept only after files existed.

## Manual Chapters

| Chapter | File |
| --- | --- |
| Compose Graph compile/runtime model | [`manual/01-compose-graph-runtime.md`](./manual/01-compose-graph-runtime.md) |
| Workflow, Chain, and field mapping | [`manual/02-workflow-chain-field-mapping.md`](./manual/02-workflow-chain-field-mapping.md) |
| Runnable, streams, and callbacks | [`manual/03-runnable-stream-callback.md`](./manual/03-runnable-stream-callback.md) |
| Checkpoint, interrupt, and resume | [`manual/04-checkpoint-interrupt-resume.md`](./manual/04-checkpoint-interrupt-resume.md) |
| Components, model, tool, and prompt contracts | [`manual/05-components-model-tool-prompt.md`](./manual/05-components-model-tool-prompt.md) |
| Schema and provider adapters | [`manual/06-schema-provider-adapters.md`](./manual/06-schema-provider-adapters.md) |
| Agent flow, ReAct, and multi-agent host | [`manual/07-agent-flow-react-multiagent.md`](./manual/07-agent-flow-react-multiagent.md) |

## Practical Lesson

For documentation and research work, Rive needs artifact-oriented nodes, not
one large summarization pass. The strongest pattern is:

```text
human objective
  -> architect creates focused chapter/research DAG
  -> workers produce concrete files
  -> accept/reopen based on artifact quality
  -> upload or publish the artifacts
```

This turns agent work from "read and summarize" into a ledger-backed document
production workflow.
