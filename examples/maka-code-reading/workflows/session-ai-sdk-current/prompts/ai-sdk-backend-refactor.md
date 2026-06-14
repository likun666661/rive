# Maka 当前现状分析：AI SDK Backend / ModelAdapter / ToolRuntime / RunTrace

你是 Rive 的 OpenCode code-reading worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 当前阅读基线：`{{source_ref}}`
- 对比起点：`{{previous_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 任务

分析最近 `Extract runtime tool, model, and trace layers` 后，AI SDK backend 的架构是否真的从“大函数”拆成了可维护的 model/tool/trace 边界。

要回答：

- `AiSdkBackend` 现在还承担哪些责任？
- `ModelAdapter` 是否形成稳定 provider seam？
- `ToolRuntime` 是否把 permission/watchdog/tool execution/telemetry 的不变量集中起来？
- `RunTrace` 是否能作为后续 observability / replay / cost diagnosis 的骨架？
- 最近测试是否能锁住这些边界？

## 必读

- `{{repo_path}}/packages/runtime/src/ai-sdk-backend.ts`
- `{{repo_path}}/packages/runtime/src/model-adapter.ts`
- `{{repo_path}}/packages/runtime/src/model-factory.ts`
- `{{repo_path}}/packages/runtime/src/tool-runtime.ts`
- `{{repo_path}}/packages/runtime/src/run-trace.ts`
- `{{repo_path}}/packages/runtime/src/permission-engine.ts`
- `{{repo_path}}/packages/runtime/src/builtin-tools.ts`
- `{{repo_path}}/packages/runtime/src/__tests__/ai-sdk-backend.test.ts`
- `{{repo_path}}/packages/runtime/src/__tests__/model-adapter.test.ts`
- `{{repo_path}}/packages/runtime/src/__tests__/tool-runtime-extraction-contract.test.ts`

## 输出

只允许写入 `{{output_dir}}/02-ai-sdk-backend-refactor.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/02-ai-sdk-backend-refactor.md
```

## 报告要求

必须包含这些二级标题：

- `scope`：列出读过的文件和关键函数。
- `problem`：说明 AI SDK backend 为什么容易失控。
- `current_design`：画出 `SessionManager -> AiSdkBackend -> ModelAdapter/ToolRuntime/RunTrace` 的边界。
- `source_evidence`：表格列文件/函数/证据/判断。
- `call_flow`：逐步说明用户消息到 `streamText`、tool call、permission、tool result、usage/trace 的链路。
- `tests`：现有新增测试具体锁住了哪些行为。
- `risks`：列出仍可能失败的边界，例如 provider stream quirks、tool call ordering、trace write loss、cache/reasoning token。
- `next_actions`：提出下一步实现/测试 DAG。

报告要有维护者判断：哪些抽象是“真实降低复杂度”，哪些还只是搬家。
