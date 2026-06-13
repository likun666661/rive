# 精读任务：LSP Supervisor 集成

你是 Rive 的 OpenCode worker。请对 `pie` 做只读源码级精读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

- `crates/coding-agent/src/lsp_supervisor.rs`
- `crates/coding-agent/src/lsp.rs`
- `crates/coding-agent/src/main.rs`
- `crates/coding-agent/src/tools/edit.rs`
- `crates/coding-agent/src/tools/write.rs`
- `crates/coding-agent/src/tools/bash.rs`
- `crates/agent/src/types.rs` 中 after/before tool hook 相关类型
- LSP / edit / tool tests

## 输出

只允许写入 `{{output_dir}}/04-lsp-integration-report.md`。不要修改源码，不要使用 OpenCode 内置 task/fan-out。完成后执行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/04-lsp-integration-report.md
```

## 报告结构

1. `problem`：LSP supervisor 在 coding agent 里要解决什么问题。
2. `why_hard`：启动延迟、语言探测、诊断时机、工具写入并发、性能和噪声为什么难。
3. `design_approach`：pie 的 LSP 接入思路。
4. `code_walkthrough`：源码走读。
5. `integration_timeline`：从 edit/write/bash 到 after_tool_call diagnostic 注入的时序。
6. `performance`：启动/缓存/诊断开销和可能优化。
7. `tests`：覆盖到的测试。
8. `risks`：边界 bug 和未完成点。
9. `next_questions`：下一轮问题。
