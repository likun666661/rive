# 汇总任务：Maka 当前 Session + AI SDK Backend 维护者报告

你是 Rive 的 OpenCode final review worker。优先消费前 5 个节点的产物，必要时少量回查源码，输出中文维护者报告。不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 当前阅读基线：`{{source_ref}}`
- 对比起点：`{{previous_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 上游产物

请先读：

- `{{output_dir}}/01-session-lifecycle.md`
- `{{output_dir}}/02-ai-sdk-backend-refactor.md`
- `{{output_dir}}/03-desktop-session-bridge.md`
- `{{output_dir}}/04-storage-trace-recovery.md`
- `{{output_dir}}/05-bot-gateway-regression.md`

如果某个文件缺失，明确写缺失，不要编造。

## 输出

只允许写入 `{{output_dir}}/00-current-state-report.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/00-current-state-report.md
```

## 报告要求

必须包含这些二级标题：

- `executive_summary`：10-15 条，说明当前 Maka session + AI SDK backend 的真实状态。
- `current_architecture_map`：用文字图描述 renderer/main/runtime/storage/AI SDK/backend/tool/trace 的关系。
- `recent_delta`：按 commit 或文件群说明 `{{previous_ref}}..{{source_ref}}` 解决了什么问题。
- `risk_register`：按 P0/P1/P2 列 confirmed/hypothesis 风险；每条必须有 evidence、impact、next step。
- `verification_map`：把现有 tests 映射到核心 invariants，并指出缺口。
- `recommended_next_dag`：给出下一轮 Rive DAG，最好能拆 implementation nodes、test nodes、final review node。

质量要求：

- 不要简单拼贴上游报告；要去重、排序、判断优先级。
- 明确哪些旧 deep-read 风险已经被修掉，哪些还没有。
- 必须覆盖 `session lifecycle`、`AI SDK backend refactor`、`ToolRuntime`、`ModelAdapter`、`RunTrace`、`credential IPC`、`JSONL recovery`、`bot/gateway controls`。
- 成功判断只来自 Rive artifacts 和源码证据，不使用 worker final answer 自称。
