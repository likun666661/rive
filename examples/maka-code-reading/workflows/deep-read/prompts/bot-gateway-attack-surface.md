# Maka 精读任务：Bot Bridge / OpenGateway Attack Surface

你是 Rive 的 OpenCode deep-read worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 必读源码

- `{{repo_path}}/packages/runtime/src/bots/`
- `{{repo_path}}/apps/desktop/src/main/open-gateway.ts`
- `{{repo_path}}/apps/desktop/src/main/main.ts` bot/open-gateway sections
- `{{repo_path}}/packages/core/src/bot-events.ts`
- `{{repo_path}}/packages/core/src/bot-platform-hints.ts`
- relevant tests and docs

## 输出

只允许写入 `{{output_dir}}/05-bot-gateway-attack-surface.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/05-bot-gateway-attack-surface.md
```

## 报告要求

必须包含：

- `scope`
- `problem`
- `source_evidence`
- `flow_analysis`：IM inbound -> bridge -> SessionManager -> backend -> outbound；OpenGateway request -> auth -> session state/SSE。
- `attack_surface`：至少列 5 个攻击向量或 abuse scenario，标注 evidence 和影响。
- `mitigations`：rate limit、permission mode、token scoping、SSE limits、path/input normalization。
- `next_actions`

质量要求：区分真实可利用风险与需要部署环境确认的风险。
