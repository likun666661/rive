# Maka 当前现状分析：Bot / OpenGateway / Session Abuse Regression

你是 Rive 的 OpenCode code-reading worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 当前阅读基线：`{{source_ref}}`
- 对比起点：`{{previous_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 任务

分析最近 bot/gateway abuse controls 与 session/backend 的关系。重点看外部入口如何创建或复用 session，如何避免重复消息、SSE storm、gateway token/connection 滥用，以及这些测试是否覆盖真实攻击面。

## 必读

- `{{repo_path}}/apps/desktop/src/main/open-gateway.ts`
- `{{repo_path}}/apps/desktop/src/main/main.ts`
- `{{repo_path}}/apps/desktop/src/main/project-context.ts`
- `{{repo_path}}/packages/core/src/bot-events.ts`
- `{{repo_path}}/packages/core/src/bot-platform-hints.ts`
- `{{repo_path}}/packages/runtime/src/session-manager.ts`
- `{{repo_path}}/apps/desktop/src/main/__tests__/bot-incoming-idempotency-contract.test.ts`
- `{{repo_path}}/apps/desktop/src/main/__tests__/open-gateway-sse-abuse-contract.test.ts`
- `{{repo_path}}/apps/desktop/src/main/__tests__/open-gateway.test.ts`
- 可选：`rg "bot|gateway|sse|session" {{repo_path}}/apps/desktop/src/main`

## 输出

只允许写入 `{{output_dir}}/05-bot-gateway-regression.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/05-bot-gateway-regression.md
```

## 报告要求

必须包含这些二级标题：

- `scope`：列出读过的 bot/gateway/session 文件。
- `problem`：说明 bot/gateway 为什么会把 session/backend 风险放大。
- `current_design`：说明入口、idempotency、SSE、session reuse、permission mode 的当前设计。
- `source_evidence`：表格列证据、限制、测试。
- `abuse_flow`：至少描述重复 incoming、SSE connection storm、gateway-to-session 三条链路。
- `tests`：现有测试覆盖和缺口。
- `risks`：按 P0/P1/P2 说明剩余攻击面或运营风险。
- `next_actions`：给出下一轮可执行工程动作。
