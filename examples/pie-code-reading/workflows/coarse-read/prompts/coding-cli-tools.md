# 粗读任务：pie-coding-agent CLI / TUI / Tools

你是 Rive 的 OpenCode worker。请对 `pie` 做只读粗读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `crates/coding-agent/src/main.rs`
- `crates/coding-agent/src/cli` 相关入口（如果有）
- `crates/coding-agent/src/commands.rs`
- `crates/coding-agent/src/tui.rs`
- `crates/coding-agent/src/readline.rs`
- `crates/coding-agent/src/config.rs`
- `crates/coding-agent/src/model.rs`
- `crates/coding-agent/src/history.rs`
- `crates/coding-agent/src/session/`
- `crates/coding-agent/src/tools/`
- `crates/coding-agent/src/skills*.rs`
- `crates/coding-agent/src/mcp_loader.rs`
- `crates/coding-agent/src/ui/`
- `crates/coding-agent/tests/`
- `README.md` 的 CLI/commands 部分

## 输出

只允许写入：

`{{output_dir}}/03-coding-cli-tools.md`

不要修改仓库源码。不要使用 OpenCode 内置 task/fan-out。写完后用：

```sh
team report --status done --artifact-ref file:{{output_dir}}/03-coding-cli-tools.md
```

## 报告结构

请使用中文，保留 Rust 标识符和文件路径原文。报告至少包含：

1. `problem`：CLI/TUI/tools 层要解决什么问题。解释 pie 如何把终端交互、slash commands、工具执行、配置和 agent core 连接起来。
2. `why_hard`：为什么难。讨论交互式 TUI、历史/恢复、文件编辑安全、skills/MCP 加载、图片/剪贴板、web UI parity。
3. `design_approach`：设计思路。画出 `main/config -> session -> UI/input -> command/tool -> agent core` 流程。
4. `code_walkthrough`：关键文件/类型/函数。
5. `flows`：普通 REPL 请求、slash command、工具调用、session resume/export 各一条。
6. `tests`：列 CLI/tools/TUI 相关测试。
7. `risks`：体验和工程风险。
8. `next_questions`：下一轮精读问题。
