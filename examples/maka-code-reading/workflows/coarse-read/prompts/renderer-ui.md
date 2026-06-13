# Maka 粗读任务：Renderer / UI

你是 Rive 的 OpenCode reader worker。请只读代码和文档，输出中文 Markdown 报告，不要修改仓库源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `{{repo_path}}/apps/desktop/src/renderer/main.tsx`
- `{{repo_path}}/apps/desktop/src/renderer/styles.css`
- `{{repo_path}}/apps/desktop/src/renderer/maka-tokens.css`
- `{{repo_path}}/apps/desktop/src/renderer/settings/` if present
- `{{repo_path}}/apps/desktop/src/renderer/command-palette.tsx`
- `{{repo_path}}/apps/desktop/src/renderer/artifact-pane.tsx`
- `{{repo_path}}/apps/desktop/src/renderer/artifact-preview.tsx`
- `{{repo_path}}/apps/desktop/src/renderer/session-status-*`
- `{{repo_path}}/apps/desktop/src/renderer/use-thread-search.ts`
- `{{repo_path}}/packages/ui/src/components.tsx`
- `{{repo_path}}/packages/ui/src/assistant-stream.ts`
- `{{repo_path}}/packages/ui/src/artifact-preview-registry.ts`
- `{{repo_path}}/packages/ui/src/smooth-stream.ts`
- `{{repo_path}}/packages/ui/src/tool-output-stream.ts`
- `{{repo_path}}/packages/ui/src/maka-uri.ts`
- relevant renderer/main tests for UI contracts

## 输出

只允许写入：

`{{output_dir}}/05-renderer-ui.md`

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/05-renderer-ui.md
```

## 报告要求

请用中文，保留 React/TypeScript/CSS 标识符和文件路径原文。报告必须包含：

1. `problem`：renderer/UI 层要呈现什么 agent runtime 状态，为什么它不是普通聊天 UI。
2. `why_hard`：streaming、tool output、artifact preview、settings、session status、command palette、accessibility 的复杂度。
3. `design_approach`：renderer 如何从 preload/main 获取数据，`packages/ui` 提供哪些可复用组件/stream utilities。
4. `code_walkthrough`：主组件、状态管理、设置页、artifact pane、assistant stream、UI tokens。
5. `flows`：至少画 5 条链路：new session/send message、stream rendering、tool output rendering、artifact preview、provider settings、thread search/status。
6. `tests`：现有 UI/contract/visual smoke 测试覆盖什么，缺口是什么。
7. `risks`：状态同步、长输出性能、a11y、visible copy hygiene、runtime truth vs display text、settings credential UX。
8. `next_questions`：下一轮 UI/UX 精读建议。

## 质量要求

- 不要只评价视觉；重点解释 UI 如何映射 agent runtime 状态。
- 如果某个 UI 行为依赖 main/preload API，请标注接口名和需要 `desktop-main-ipc` 节点交叉确认。
- 不要使用 OpenCode 内置 task/fan-out。
