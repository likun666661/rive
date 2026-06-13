# 精读任务：Trigger 状态机

你是 Rive 的 OpenCode worker。请对 `pie` 做只读源码级精读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `crates/agent/src/harness/agent_harness.rs`
- `crates/agent/src/harness/trigger.rs`
- `crates/agent/src/harness/trigger_runtime.rs`
- `crates/agent/src/harness/notification_hook.rs`
- `crates/coding-agent/src/triggers/dynamic.rs`
- `crates/coding-agent/src/triggers/cron.rs`
- `crates/coding-agent/src/triggers/mcp_notification_hook.rs`
- 相关 trigger / dynamic / notification 测试

## 输出

只允许写入 `{{output_dir}}/01-trigger-state-machine.md`。不要修改源码，不要使用 OpenCode 内置 task/fan-out。完成后执行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/01-trigger-state-machine.md
```

## 报告结构

必须使用中文，保留源码标识符原文，至少包含：

1. `problem`：trigger 状态机要解决什么问题。
2. `why_hard`：为什么自动触发、去重、循环抑制、sub-agent、promotion 很难。
3. `design_approach`：pie 的核心设计。
4. `code_walkthrough`：逐文件源码走读。
5. `state_machine`：画出 Receive/Accept/Skip/Running/Completed/Failed/Promoted 等状态与转移条件。
6. `side_effects`：每一步的副作用表，包括 audit write、session entry、sub-agent spawn、inbox/promotion。
7. `tests`：测试文件和覆盖意图。
8. `risks`：边界 bug、竞态、TODO。
9. `next_questions`：下一轮问题。
