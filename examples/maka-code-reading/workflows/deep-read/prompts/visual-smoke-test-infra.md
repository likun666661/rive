# Maka 精读任务：Visual Smoke / Test Infrastructure

你是 Rive 的 OpenCode deep-read worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 必读源码/文档

- `{{repo_path}}/scripts/capture-screenshots.mjs`
- `{{repo_path}}/scripts/diff-screenshots.mjs`
- `{{repo_path}}/scripts/check-a11y.mjs`
- `{{repo_path}}/scripts/check-console.mjs`
- `{{repo_path}}/scripts/check-stale-dist.mjs`
- `{{repo_path}}/scripts/check-officecli-bundle.mjs`
- `{{repo_path}}/apps/desktop/src/main/visual-smoke-fixture.ts`
- `{{repo_path}}/apps/desktop/tests/smoke.md`
- `{{repo_path}}/docs/full-product-test-plan.md`
- `{{repo_path}}/docs/ui-quality-plan.md`
- package scripts

## 输出

只允许写入 `{{output_dir}}/10-visual-smoke-test-infra.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/10-visual-smoke-test-infra.md
```

## 报告要求

必须包含：

- `scope`
- `problem`
- `source_evidence`
- `coverage_matrix`：fixture scenarios、manual smoke、script checks、package scripts 到 UI surface 的映射。
- `gaps`：没有截图覆盖、没有交互测试、release gate 盲点。
- `ci_strategy`：如何把检查分成 fast/slow/stable/update-baseline。
- `next_actions`

质量要求：不要只列命令，要解释这些测试为什么能捕捉桌面 agent 的风险。
