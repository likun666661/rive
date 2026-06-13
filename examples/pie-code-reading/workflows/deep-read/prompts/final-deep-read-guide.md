# 汇总任务：pie 精读技术手册

你是 Rive 的 OpenCode final review worker。请消费本轮所有精读产物，输出一份中文技术手册总纲。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 上游产物

- `{{output_dir}}/01-trigger-state-machine.md`
- `{{output_dir}}/02-loop-inbox-internals.md`
- `{{output_dir}}/03-session-branch-model.md`
- `{{output_dir}}/04-lsp-integration-report.md`
- `{{output_dir}}/05-tool-call-parsing-matrix.md`
- `{{output_dir}}/06-goal-evaluator-internals.md`
- `{{output_dir}}/07-automation-security-audit.md`
- `{{output_dir}}/08-session-integrity-review.md`
- `{{output_dir}}/09-provider-conformance-test-plan.md`

## 输出

只允许写入 `{{output_dir}}/00-final-deep-read-guide.md`。不要修改源码，不要使用 OpenCode 内置 task/fan-out。完成后执行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/00-final-deep-read-guide.md
```

## 报告结构

1. `executive_summary`：本轮精读的关键结论。
2. `architecture_lessons`：从 trigger、loop、session、provider、LSP、goal evaluator 看到的架构模式。
3. `deep_read_index`：每份报告的阅读路线和适用场景。
4. `risk_register`：按严重度整理风险，绑定文件路径和证据。
5. `recommended_followups`：下一轮可直接转成 Rive DAG 的任务。
6. `appendix`：上游产物完整性、缺失/矛盾点、人工复核建议。
