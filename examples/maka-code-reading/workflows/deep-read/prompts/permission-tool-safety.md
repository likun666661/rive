# Maka 精读任务：Permission / Tool Safety

你是 Rive 的 OpenCode deep-read worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 必读材料

先读 coarse artifacts：

- `{{output_dir}}/../coarse/00-overview.md`（如果存在）
- `{{output_dir}}/../coarse/01-core-contracts.md`（如果存在）
- `{{output_dir}}/../coarse/02-runtime-backends-tools.md`（如果存在）

再读源码：

- `{{repo_path}}/packages/runtime/src/ai-sdk-backend.ts`
- `{{repo_path}}/packages/runtime/src/permission-engine.ts`
- `{{repo_path}}/packages/runtime/src/builtin-tools.ts`
- `{{repo_path}}/packages/core/src/permission.ts`
- `{{repo_path}}/packages/runtime/src/stream-watchdog.ts`
- relevant tests under `packages/runtime/src/__tests__` and `packages/core/src/__tests__`

## 输出

只允许写入 `{{output_dir}}/01-permission-tool-safety.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/01-permission-tool-safety.md
```

## 报告要求

必须包含这些二级标题：

- `scope`：列出读过的文件和关键函数。
- `problem`：说明 permission/tool safety 要解决的问题。
- `source_evidence`：用表格列函数/文件/行号/证据，不要只写结论。
- `flow_analysis`：逐步画出 tool call -> tool_call message -> permission decision -> watchdog pause/resume -> execute -> tool result -> telemetry。
- `risk_matrix`：覆盖 allow/block/prompt、abort/reject/timeout、remember-for-turn、bot mode、permissionRequired=false。
- `tests`：现有测试、缺口、建议新增测试。
- `next_actions`：按 P0/P1/P2 给出工程建议。

质量要求：至少给出 1 个竞态或不变量风险；如果没有发现真实问题，明确说明为什么现有代码足够安全。
