# Maka 粗读任务：Storage / Persistence

你是 Rive 的 OpenCode reader worker。请只读代码和文档，输出中文 Markdown 报告，不要修改仓库源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `{{repo_path}}/packages/storage/package.json`
- `{{repo_path}}/packages/storage/src/index.ts`
- `{{repo_path}}/packages/storage/src/session-store.ts`
- `{{repo_path}}/packages/storage/src/connection-store.ts`
- `{{repo_path}}/packages/storage/src/artifact-store.ts`
- `{{repo_path}}/packages/storage/src/settings-store.ts`
- `{{repo_path}}/packages/storage/src/telemetry-repo.ts`
- `{{repo_path}}/packages/storage/src/plan-reminder-store.ts`
- `{{repo_path}}/packages/storage/src/__tests__/`
- 相关 core 类型：`packages/core/src/session.ts`, `events.ts`, `llm-connections.ts`, `settings.ts`

## 输出

只允许写入：

`{{output_dir}}/03-storage-persistence.md`

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/03-storage-persistence.md
```

## 报告要求

请用中文，保留 TypeScript 标识符和文件路径原文。报告必须包含：

1. `problem`：Maka 需要保存哪些状态，为什么这些状态不能只存在内存里。
2. `why_hard`：session/event、provider credential metadata、settings、artifacts、telemetry、reminders 的一致性和隐私难点。
3. `design_approach`：各 store 的职责边界、文件布局、读写/迁移/容错策略。
4. `code_walkthrough`：关键 store 文件、核心函数、数据格式、异常处理。
5. `flows`：至少画 4 条链路：创建 session、append event、读取消息、保存 provider connection、artifact 写入、telemetry 记录。
6. `tests`：现有测试覆盖和明显缺口，尤其是损坏文件、并发写、原子性、迁移。
7. `risks`：credential/PII 泄漏、JSON 文件腐败、跨版本兼容、renderer 可见范围、数据目录选择。
8. `next_questions`：下一轮 durable-state 精读建议。

## 质量要求

- 注意区分 credential material 和 metadata；不要把任何真实 secret 写入报告。
- 如果某些加密/安全逻辑在 desktop main 而不在 storage，请标注需要 `desktop-main-ipc` 节点交叉确认。
- 不要使用 OpenCode 内置 task/fan-out。
