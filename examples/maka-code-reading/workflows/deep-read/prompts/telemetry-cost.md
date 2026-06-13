# Maka 精读任务：Telemetry / Cost Completeness

你是 Rive 的 OpenCode deep-read worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 必读源码

- `{{repo_path}}/packages/runtime/src/telemetry/`
- `{{repo_path}}/packages/storage/src/telemetry-repo.ts`
- `{{repo_path}}/packages/core/src/usage-stats/`
- `{{repo_path}}/packages/runtime/src/ai-sdk-backend.ts`
- `{{repo_path}}/packages/runtime/src/builtin-tools.ts`
- `{{repo_path}}/apps/desktop/src/main/main.ts` usage/settings sections
- relevant tests

## 输出

只允许写入 `{{output_dir}}/08-telemetry-cost.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/08-telemetry-cost.md
```

## 报告要求

必须包含：

- `scope`
- `problem`
- `source_evidence`
- `accounting_flow`：LLM call usage -> pricing -> storage -> UI/report；tool invocation -> storage -> UI/report。
- `loss_windows`：fire-and-forget、process exit、unpriced models、reasoning/cache token support、provider-specific fields。
- `tests`
- `next_actions`

质量要求：明确 reasoning tokens/cache read/write 是否能进成本模型；不确定时给代码证据和待验证路径。
