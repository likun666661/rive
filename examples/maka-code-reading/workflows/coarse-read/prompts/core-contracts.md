# Maka 粗读任务：Core Schemas / Contracts

你是 Rive 的 OpenCode reader worker。请只读代码和文档，输出中文 Markdown 报告，不要修改仓库源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `{{repo_path}}/packages/core/package.json`
- `{{repo_path}}/packages/core/src/index.ts`
- `{{repo_path}}/packages/core/src/events.ts`
- `{{repo_path}}/packages/core/src/session.ts`
- `{{repo_path}}/packages/core/src/permission.ts`
- `{{repo_path}}/packages/core/src/connections.ts`
- `{{repo_path}}/packages/core/src/llm-connections.ts`
- `{{repo_path}}/packages/core/src/runtime-inputs.ts`
- `{{repo_path}}/packages/core/src/artifacts.ts`
- `{{repo_path}}/packages/core/src/workspace.ts`
- `{{repo_path}}/packages/core/src/settings.ts`
- `{{repo_path}}/packages/core/src/memory.ts`
- `{{repo_path}}/packages/core/src/search.ts`
- `{{repo_path}}/packages/core/src/bot-events.ts`
- `{{repo_path}}/packages/core/src/__tests__/`

可以少量回查 README/docs，但不要读完整 notes 目录。

## 输出

只允许写入：

`{{output_dir}}/01-core-contracts.md`

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/01-core-contracts.md
```

## 报告要求

请用中文，保留 TypeScript 标识符和文件路径原文。报告必须包含：

1. `problem`：`packages/core` 试图解决什么问题，为什么 Maka 需要一个独立 contract/schema 层。
2. `why_hard`：跨 Electron main、renderer、runtime、storage、bot、settings 共享类型为什么容易失控。
3. `design_approach`：核心类型如何分层，哪些是 session/event，哪些是 permission/settings/provider/artifact/memory。
4. `code_walkthrough`：列出关键文件、导出对象、它们被谁消费。
5. `flows`：至少画 3 条数据流，例如 session event、permission request、provider connection、artifact preview 或 memory/search。
6. `tests`：总结现有测试覆盖了哪些 contract，哪些 contract 缺少测试。
7. `risks`：类型膨胀、隐私/权限边界、向后兼容、renderer-main contract 漂移等风险。
8. `next_questions`：给下一轮 deep read 的问题清单，问题要能变成 Rive DAG 节点。

## 质量要求

- 不要把文件列表当报告，要解释这些类型为什么存在。
- 如果某个 contract 在代码里只是类型定义，没有实际调用路径，请明确标注“需要下游节点确认”。
- 不要使用 OpenCode 内置 task/fan-out；这是一个单节点阅读任务。
