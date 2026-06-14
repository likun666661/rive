# Maka 当前现状分析：Storage / JSONL Recovery / Telemetry Trace

你是 Rive 的 OpenCode code-reading worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 当前阅读基线：`{{source_ref}}`
- 对比起点：`{{previous_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 任务

分析 session event persistence、corrupt JSONL recovery、telemetry/cost persistence 与新的 `RunTrace` 如何形成或尚未形成闭环。

重点回答：如果 AI SDK stream、tool execution 或 app crash 中断，当前代码能恢复到什么程度？哪些信息是持久账本，哪些只是内存 trace？

## 必读

- `{{repo_path}}/packages/storage/src/session-store.ts`
- `{{repo_path}}/packages/storage/src/telemetry-repo.ts`
- `{{repo_path}}/packages/storage/src/artifact-store.ts`
- `{{repo_path}}/packages/runtime/src/run-trace.ts`
- `{{repo_path}}/packages/runtime/src/session-manager.ts`
- `{{repo_path}}/packages/runtime/src/tool-artifacts.ts`
- `{{repo_path}}/packages/runtime/src/tool-output-delta.ts`
- `{{repo_path}}/packages/storage/src/__tests__/session-store.test.ts`
- `{{repo_path}}/packages/storage/src/__tests__/telemetry-repo.test.ts`
- `{{repo_path}}/packages/runtime/src/__tests__/ai-sdk-backend.test.ts`

## 输出

只允许写入 `{{output_dir}}/04-storage-trace-recovery.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/04-storage-trace-recovery.md
```

## 报告要求

必须包含这些二级标题：

- `scope`：列出读过的 persistence/trace 文件。
- `problem`：说明 session persistence 和 trace recovery 为什么难。
- `current_design`：区分 durable event log、telemetry repo、artifact store、runtime trace。
- `source_evidence`：表格列证据、durability boundary、failure mode。
- `recovery_flow`：逐步描述正常写入、坏行跳过、partial session 恢复、telemetry 写入。
- `tests`：列出现有 recovery/corruption 测试和缺口。
- `risks`：重点关注 trace 不持久、JSONL tail corruption、telemetry/cache/reasoning tokens、artifact ref consistency。
- `next_actions`：提出下一步实现/测试 DAG。
