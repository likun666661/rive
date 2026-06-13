# 粗读任务：pie-agent-core Runtime / Harness / Session

你是 Rive 的 OpenCode worker。请对 `pie` 做只读粗读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `crates/agent/src/lib.rs`
- `crates/agent/src/agent.rs`
- `crates/agent/src/agent_loop.rs`
- `crates/agent/src/types.rs`
- `crates/agent/src/node.rs`
- `crates/agent/src/proxy.rs`
- `crates/agent/src/harness/`
- `crates/agent/src/harness/agent_harness.rs`
- `crates/agent/src/harness/system_prompt.rs`
- `crates/agent/src/harness/session/`
- `crates/agent/src/harness/compaction/`
- `crates/agent/src/harness/env/`
- `crates/agent/src/harness/trigger_runtime.rs`
- `crates/agent/src/harness/cost.rs`
- `crates/agent/tests/`

## 输出

只允许写入：

`{{output_dir}}/02-agent-core-runtime.md`

不要修改仓库源码。不要使用 OpenCode 内置 task/fan-out。写完后用：

```sh
team report --status done --artifact-ref file:{{output_dir}}/02-agent-core-runtime.md
```

## 报告结构

请使用中文，保留 Rust 标识符和文件路径原文。报告至少包含：

1. `problem`：agent core 层面临的问题是什么。解释一次用户请求如何进入 harness、构造 prompt/messages、调用模型、执行工具、持久化 session。
2. `why_hard`：为什么难。讨论可恢复 session、compaction、tool permission、trigger 子运行、成本统计、环境隔离和测试 harness。
3. `design_approach`：核心设计思路。画出 `Harness -> AgentLoop -> Model -> Tool/Session/Compaction -> Events` 流程图。
4. `code_walkthrough`：关键模块源码走读，说明类型/函数职责。
5. `flows`：写一条普通交互、一条 compaction/resume、一条 trigger/sub-agent 或 session storage 流。
6. `tests`：列相关测试和测试意图。
7. `risks`：未读风险、边界条件、可能技术债。
8. `next_questions`：下一轮精读问题。
