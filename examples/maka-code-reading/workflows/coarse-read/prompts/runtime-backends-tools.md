# Maka 粗读任务：Runtime / Backends / Tools

你是 Rive 的 OpenCode reader worker。请只读代码和文档，输出中文 Markdown 报告，不要修改仓库源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `{{repo_path}}/packages/runtime/package.json`
- `{{repo_path}}/packages/runtime/src/session-manager.ts`
- `{{repo_path}}/packages/runtime/src/ai-sdk-backend.ts`
- `{{repo_path}}/packages/runtime/src/fake-backend.ts`
- `{{repo_path}}/packages/runtime/src/model-factory.ts`
- `{{repo_path}}/packages/runtime/src/model-fetcher.ts`
- `{{repo_path}}/packages/runtime/src/permission-engine.ts`
- `{{repo_path}}/packages/runtime/src/builtin-tools.ts`
- `{{repo_path}}/packages/runtime/src/tool-artifacts.ts`
- `{{repo_path}}/packages/runtime/src/tool-output-delta.ts`
- `{{repo_path}}/packages/runtime/src/stream-watchdog.ts`
- `{{repo_path}}/packages/runtime/src/materializer.ts`
- `{{repo_path}}/packages/runtime/src/network/`
- `{{repo_path}}/packages/runtime/src/bots/`
- `{{repo_path}}/packages/runtime/src/telemetry/`
- `{{repo_path}}/packages/runtime/src/__tests__/`

## 输出

只允许写入：

`{{output_dir}}/02-runtime-backends-tools.md`

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/02-runtime-backends-tools.md
```

## 报告要求

请用中文，保留 TypeScript 标识符和文件路径原文。报告必须包含：

1. `problem`：runtime 层负责什么，为什么它不能放在 Electron main 或 UI 中。
2. `why_hard`：streaming model/backend、tool calls、permissions、telemetry、bots/network 组合后难在哪里。
3. `design_approach`：`SessionManager`、backend registry、AI SDK backend、fake backend、permission engine、builtin tools 的协作方式。
4. `code_walkthrough`：关键类/函数/文件，重点解释调用方向和状态边界。
5. `flows`：至少画 5 条链路：send message、model stream、tool call + permission、artifact materialization、bot bridge、telemetry/cost。
6. `tests`：已有测试覆盖和可能的缺口，尤其是 no-report/stream watchdog/permission/tool output。
7. `risks`：provider 抽象泄漏、tool 安全、proxy/network、fake backend 与 real backend 行为漂移、bot side effect。
8. `next_questions`：下一轮应该怎么精读 runtime，按可并行节点列出。

## 质量要求

- 不要只说“用了 Vercel AI SDK”，要说明 Maka 如何包裹它并转成自己的事件/工具/成本模型。
- 区分 runtime protocol truth、UI 展示、debug/telemetry。
- 不要使用 OpenCode 内置 task/fan-out。
