# 综合审查：自动化安全审计

你是 Rive 的 OpenCode review worker。请优先读取上游产物，必要时少量回查源码，输出中文审查报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 输出目录：`{{output_dir}}`

## 上游产物

- `{{output_dir}}/01-trigger-state-machine.md`
- `{{output_dir}}/02-loop-inbox-internals.md`

## 输出

只允许写入 `{{output_dir}}/07-automation-security-audit.md`。不要修改源码，不要使用 OpenCode 内置 task/fan-out。完成后执行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/07-automation-security-audit.md
```

## 报告结构

1. `executive_summary`
2. `threat_model`：自动触发/loop/inbox 的资产、信任边界、攻击面。
3. `trigger_risks`：去重丢失、feedback loop、prompt promotion、sub-agent 权限等。
4. `loop_inbox_risks`：loop-state 损坏、标签注入、inbox 竞态、重复告警。
5. `mitigations`：现有缓解和建议新增缓解。
6. `open_questions`：需要人工继续确认的问题。
