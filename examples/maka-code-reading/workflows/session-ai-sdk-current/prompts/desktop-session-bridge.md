# Maka 当前现状分析：Desktop Main / IPC / Credential Bridge

你是 Rive 的 OpenCode code-reading worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 当前阅读基线：`{{source_ref}}`
- 对比起点：`{{previous_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 任务

分析 desktop main/preload/renderer 与 runtime session/backend 之间的桥：IPC 参数校验、credential store、connection readiness、provider auth、settings 到 runtime 的传递。

重点读最近 credential/IP C hardening 和 secret kinds 扩展，判断它们是否足够支撑新的 AI SDK backend。

## 必读

- `{{repo_path}}/apps/desktop/src/main/main.ts`
- `{{repo_path}}/apps/desktop/src/preload/preload.ts`
- `{{repo_path}}/apps/desktop/src/main/credential-store.ts`
- `{{repo_path}}/apps/desktop/src/main/settings-ipc-helpers.ts`
- `{{repo_path}}/apps/desktop/src/main/chat-readiness.ts`
- `{{repo_path}}/apps/desktop/src/main/connection-test-status.ts`
- `{{repo_path}}/packages/core/src/provider-auth.ts`
- `{{repo_path}}/packages/core/src/llm-connections.ts`
- `{{repo_path}}/apps/desktop/src/main/__tests__/connection-credential-ipc-hardening-contract.test.ts`
- `{{repo_path}}/apps/desktop/src/main/__tests__/credential-store-contract.test.ts`
- `{{repo_path}}/apps/desktop/src/main/__tests__/credential-store-secret-kinds-contract.test.ts`

## 输出

只允许写入 `{{output_dir}}/03-desktop-session-bridge.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/03-desktop-session-bridge.md
```

## 报告要求

必须包含这些二级标题：

- `scope`：列出读过的文件、IPC handler、credential APIs。
- `problem`：说明 desktop bridge 为什么是安全/稳定风险点。
- `current_design`：说明 renderer -> preload -> main -> runtime/credential 的边界。
- `source_evidence`：表格列证据、输入校验、错误处理、secret handling。
- `ipc_flow`：逐步描述 connection credential / session start / model readiness 的链路。
- `tests`：新增 contract tests 覆盖了什么，哪些还只是 happy path。
- `risks`：列 P0/P1/P2，特别关注 secret kind、safeStorage failure、renderer untrusted input。
- `next_actions`：提出下一轮可执行工程动作。

避免泛泛讲 Electron 安全；必须落到 Maka 当前代码。
