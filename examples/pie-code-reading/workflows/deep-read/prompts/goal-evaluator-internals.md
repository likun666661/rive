# 精读任务：/goal Evaluator 与 OnTurnEndHook

你是 Rive 的 OpenCode worker。请对 `pie` 做只读源码级精读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

- `crates/agent/src/harness/agent_harness.rs`
- `crates/agent/src/types.rs`
- `crates/coding-agent/src/goal.rs`
- `crates/coding-agent/src/commands.rs`
- `crates/coding-agent/src/main.rs`
- goal / turn-end / evaluator 相关测试

## 输出

只允许写入 `{{output_dir}}/06-goal-evaluator-internals.md`。不要修改源码，不要使用 OpenCode 内置 task/fan-out。完成后执行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/06-goal-evaluator-internals.md
```

## 报告结构

1. `problem`：`/goal` evaluator 和 OnTurnEndHook 要解决什么工作流问题。
2. `why_hard`：自动判断目标完成、避免无限循环、避免 false positive/negative 为什么难。
3. `design_approach`：pie 的 evaluator/turn-end 设计。
4. `code_walkthrough`：源码走读。
5. `evaluator_loop`：从用户设置 goal 到每轮结束评估的时序。
6. `false_decisions`：误判风险、transcript bounding、prompt 策略。
7. `tests`：测试覆盖。
8. `risks`：产品/工程风险。
9. `next_questions`：下一轮问题。
