# Eino Technical Manual v2

This directory is the second Rive dogfood pass for an Eino technical manual.
It uses the first coarse reading notes in `docs/rive-eino-notes/` as seed
material, then splits the manual into focused chapter-level Work DAG nodes.

The intended reading pattern for every chapter is:

1. What problem the Eino code is solving.
2. Why that problem is technically hard.
3. The design idea behind the implementation.
4. Source-level walkthrough with concrete files, types, and functions.
5. Usage patterns and examples.
6. Pitfalls and edge cases.
7. What Rive can learn from the pattern.

## Chapters

| Chapter | File | Focus |
| --- | --- | --- |
| 1 | `01-compose-graph-runtime.md` | Compose graph compile/runtime model, DAG/Pregel, trigger modes, nested graph state |
| 2 | `02-workflow-chain-field-mapping.md` | Workflow, Chain, field mapping, dependency/data-flow separation |
| 3 | `03-runnable-stream-callback.md` | Runnable capability surface, stream lifecycle, callback runtime |
| 4 | `04-checkpoint-interrupt-resume.md` | Checkpoint, interrupt, resume, hierarchical execution address |
| 5 | `05-components-model-tool-prompt.md` | Component contracts for model/tool/prompt/RAG and graph integration |
| 6 | `06-schema-provider-adapters.md` | Schema model, provider adapters, streams, serialization and interop |
| 7 | `07-agent-flow-react-multiagent.md` | ReAct agent graph, host multi-agent flow, callback isolation |

## Notes

- These files are a technical guide, not upstream Eino documentation.
- The manual was produced by Rive-managed OpenCode workers reading the local
  Eino repository and reporting through the Work DAG ledger.
- Source citations are local file/function references from the checked-out
  Eino codebase.
