# 粗读任务：Loops / Triggers / Inbox / Hooks

你是 Rive 的 OpenCode worker。请对 `pie` 做只读粗读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `docs/loops.md`
- `docs/hooks.md`
- `docs/issues/23-loops-inbox.md`
- `docs/issues/14-observability.md`
- `docs/issues/06-token-cost-budget.md`
- `crates/coding-agent/src/triggers/`
- `crates/coding-agent/src/inbox.rs`
- `crates/coding-agent/src/hooks.rs`
- `crates/coding-agent/src/debug.rs`
- `crates/coding-agent/src/otlp.rs`
- `crates/agent/src/harness/trigger.rs`
- `crates/agent/src/harness/trigger_runtime.rs`
- `crates/agent/src/harness/notification_hook.rs`
- `crates/agent/src/harness/cost.rs`
- 相关测试：`dynamic_trigger_e2e`、`hooks_e2e`、trigger/session tests

## 输出

只允许写入：

`{{output_dir}}/04-automation-loops-triggers.md`

不要修改仓库源码。不要使用 OpenCode 内置 task/fan-out。写完后用：

```sh
team report --status done --artifact-ref file:{{output_dir}}/04-automation-loops-triggers.md
```

## 报告结构

请使用中文，保留 Rust 标识符和文件路径原文。报告至少包含：

1. `problem`：长期自动化/loops/trigger 要解决什么问题。解释“agent 主动发现并把结果投到 inbox”的产品语义。
2. `why_hard`：为什么难。讨论持久状态、去重、权限、隐私、成本预算、失败可观测、和主对话上下文隔离。
3. `design_approach`：设计思路。画出 `cron/dynamic/MCP notification -> trigger runtime -> sub-agent/action -> audit/inbox` 流程。
4. `code_walkthrough`：关键文件/类型/函数。
5. `flows`：stateful cron loop、dynamic trigger、MCP notification、hook/OTLP 任选 3 条。
6. `tests`：列相关测试和覆盖意图。
7. `risks`：生产自动化风险和未完成点。
8. `next_questions`：下一轮精读问题。
