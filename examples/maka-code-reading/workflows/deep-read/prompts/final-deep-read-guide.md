# 汇总任务：Maka 精读维护者指南

你是 Rive 的 OpenCode final review worker。请优先消费前面 10 个 deep-read 节点的产物，必要时少量回查源码，输出一份中文维护者级总结。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 上游产物

请优先阅读：

- `{{output_dir}}/01-permission-tool-safety.md`
- `{{output_dir}}/02-ipc-surface-security.md`
- `{{output_dir}}/03-path-containment.md`
- `{{output_dir}}/04-credential-settings-security.md`
- `{{output_dir}}/05-bot-gateway-attack-surface.md`
- `{{output_dir}}/06-memory-gates.md`
- `{{output_dir}}/07-jsonl-durability.md`
- `{{output_dir}}/08-telemetry-cost.md`
- `{{output_dir}}/09-external-tool-injection.md`
- `{{output_dir}}/10-visual-smoke-test-infra.md`

如果某个文件缺失，请明确写入缺失情况，不要编造。

## 输出

只允许写入 `{{output_dir}}/00-final-deep-read-guide.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/00-final-deep-read-guide.md
```

## 报告要求

必须包含：

- `executive_summary`：10-15 条，说明 Maka 的真实架构骨架和本轮最重要发现。
- `architecture_theses`：用 5-8 个 thesis 总结维护 Maka 的关键原则。
- `top_findings`：按 P0/P1/P2 列发现；每条要包含 evidence、impact、recommended fix。
- `priority_roadmap`：按 1 周 / 1 月 / 1 季度组织后续工程路线。
- `teaching_outline`：如果要给新人讲 Maka，建议怎么分章节，每章读哪些文件。
- `next_dag`：下一轮可直接用 Rive 开的实现/测试/审计 DAG。

质量要求：

- 不要拼贴上游报告；要去重、排序、判断优先级。
- 明确哪些发现是 confirmed，哪些只是 hypothesis。
- 成功判断只来自 Rive artifacts 和源码证据，不使用 worker final answer 自称。
