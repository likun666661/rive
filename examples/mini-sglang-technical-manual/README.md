# Mini-SGLang Technical Manual

This directory contains a Rive dogfood technical manual for the local
Mini-SGLang repository. Rive split the code-reading task into a teaching
outline, six focused research reports, and six expanded chapters.

The recommended reading order is:

1. Read `manual/teaching-manual-outline.md` for the course structure.
2. Use the matching file in `research/` to verify the chapter's source-level
   claims and investigation notes.
3. Read the matching file in `expanded/` for the full tutorial chapter.

## Contents

| Chapter | Research | Expanded chapter |
| --- | --- | --- |
| 1. Entry, API, and process topology | `research/01-entry-api-topology-research.md` | `expanded/01-entry-api-topology-expanded.md` |
| 2. Scheduler and request lifecycle | `research/02-scheduler-lifecycle-research.md` | `expanded/02-scheduler-lifecycle-expanded.md` |
| 3. Engine and distributed execution | `research/03-engine-distributed-research.md` | `expanded/03-engine-distributed-expanded.md` |
| 4. KV cache and radix cache | `research/04-kvcache-radix-research.md` | `expanded/04-kvcache-radix-expanded.md` |
| 5. Models, attention, and kernels | `research/05-model-attention-kernels-research.md` | `expanded/05-model-attention-kernels-expanded.md` |
| 6. Tokenizer, serving, and benchmarks | `research/06-tokenizer-serving-benchmarks-research.md` | `expanded/06-tokenizer-serving-benchmarks-expanded.md` |

## Rive Execution Record

- Teaching-outline root: `work_f54fd2c709ee46b1943b0ef02e556a7d`
- Research DAG root: `work_e529f27031b84320ac7ed14b7aacbfab`
- Expanded-chapter DAG root: `work_67edf4d87e0d4489ad2eaf872a7bcdd6`
- Research and expanded-chapter DAGs completed at `--max-parallel 6`.

The original worktree lived outside this repository. These Markdown files are
the materialized, accepted outputs intended for browsing and reuse here.
