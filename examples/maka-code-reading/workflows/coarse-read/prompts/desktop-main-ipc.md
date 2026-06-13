# Maka 粗读任务：Desktop Main / IPC / Preload

你是 Rive 的 OpenCode reader worker。请只读代码和文档，输出中文 Markdown 报告，不要修改仓库源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `{{repo_path}}/apps/desktop/src/main/main.ts`
- `{{repo_path}}/apps/desktop/src/preload/preload.ts`
- `{{repo_path}}/apps/desktop/src/main/credential-store.ts`
- `{{repo_path}}/apps/desktop/src/main/settings-ipc-helpers.ts`
- `{{repo_path}}/apps/desktop/src/main/project-context.ts`
- `{{repo_path}}/apps/desktop/src/main/session-environment-prompt.ts`
- `{{repo_path}}/apps/desktop/src/main/workspace-instructions.ts`
- `{{repo_path}}/apps/desktop/src/main/rive-cli.ts`
- `{{repo_path}}/apps/desktop/src/main/rive-workflow-tool.ts`
- `{{repo_path}}/apps/desktop/src/main/explore-agent-tool.ts`
- `{{repo_path}}/apps/desktop/src/main/office-document-tool.ts`
- `{{repo_path}}/apps/desktop/src/main/open-gateway.ts`
- `{{repo_path}}/apps/desktop/src/main/local-memory-service.ts`
- `{{repo_path}}/apps/desktop/src/main/onboarding-service.ts`
- `{{repo_path}}/apps/desktop/src/main/oauth/` if present
- `{{repo_path}}/apps/desktop/src/main/__tests__/`

## 输出

只允许写入：

`{{output_dir}}/04-desktop-main-ipc.md`

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/04-desktop-main-ipc.md
```

## 报告要求

请用中文，保留 TypeScript/Electron 标识符和文件路径原文。报告必须包含：

1. `problem`：desktop main 层在 Maka 中承担什么，为什么它是安全边界和 OS 集成边界。
2. `why_hard`：Electron IPC、safeStorage、local file access、workspace open/path guard、external services、Rive tool bridging 的风险。
3. `design_approach`：main/preload/renderer 的权限分层；哪些能力在 main；哪些 API 暴露给 renderer。
4. `code_walkthrough`：关键 IPC handler/service/tool 文件，调用方向和输入校验。
5. `flows`：至少画 5 条链路：app boot、session send、provider credential save/test、open path guard、Rive workflow tool、local memory/search、office/document tool。
6. `tests`：现有 main 测试覆盖什么，哪些 IPC/安全边界缺少测试。
7. `risks`：IPC surface 扩张、path traversal、credential exposure、external link/open path、Rive/office tool side effects。
8. `next_questions`：下一轮 desktop security/API 精读建议。

## 质量要求

- 重点解释“renderer 为什么不能直接做这些事”。
- 对所有可能触发外部副作用的 main service 做标记。
- 不要使用 OpenCode 内置 task/fan-out。
