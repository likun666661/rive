# 精读任务：Loop 标签提取与 Inbox 闭环

你是 Rive 的 OpenCode worker。请对 `pie` 做只读源码级精读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

- `docs/loops.md`
- `crates/coding-agent/src/triggers/cron.rs`
- `crates/coding-agent/src/inbox.rs`
- `crates/coding-agent/src/commands.rs`
- `crates/coding-agent/src/triggers/mod.rs`
- `crates/agent/src/harness/agent_harness.rs`
- 相关 cron/inbox/trigger tests

## 输出

只允许写入 `{{output_dir}}/02-loop-inbox-internals.md`。不要修改源码，不要使用 OpenCode 内置 task/fan-out。完成后执行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/02-loop-inbox-internals.md
```

## 报告结构

1. `problem`：stateful loop 与 inbox 闭环解决什么问题。
2. `why_hard`：为什么标签协议、状态文件、inbox triage、并发写入和失败恢复难。
3. `design_approach`：pie 的 loop/inbox 设计。
4. `code_walkthrough`：`cron_harness_listener`、`extract_tag_block`、`extract_tag_all`、inbox append/claim/dismiss/clear 等源码走读。
5. `parsing_edges`：标签缺失、重复、嵌套、超长、多条 inbox、模型输出污染等情况。
6. `concurrency`：JSONL inbox 的跨进程/多 loop 写入安全性分析。
7. `tests`：覆盖到的测试。
8. `risks`：生产值班自动化风险。
9. `next_questions`：下一轮问题。
