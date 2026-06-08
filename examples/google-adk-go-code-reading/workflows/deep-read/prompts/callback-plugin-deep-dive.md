# 精读任务：Callback / Plugin / Instruction 扩展机制

你是 Rive 的 OpenCode worker。请对 `google/adk-go` 的回调、插件和指令扩展机制做只读精读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `agent/callback_context.go`
- `agent/context.go`
- `agent/llmagent/llmagent.go`
- `agent/llmagent/*callback*test.go`
- `plugin/plugin.go`
- `internal/plugininternal/plugin_manager.go`
- `internal/plugininternal/plugincontext/context.go`
- `plugin/functioncallmodifier/*.go`
- `plugin/loggingplugin/*.go`
- `plugin/retryandreflect/*.go`
- `internal/configurable/**`
- `util/instructionutil/**`
- `internal/llminternal/instruction_processor.go`
- `internal/llminternal/request_confirmation_processor.go`

同步阅读 callback/plugin/configurable 相关测试。

## 输出

只允许写入：

`{{output_dir}}/04-callback-plugin-deep-dive.md`

不要修改仓库源码。写完后捕获 snapshot，并用
`team report --status done --artifact-ref file:{{output_dir}}/04-callback-plugin-deep-dive.md`
报告。

## 报告结构

请使用中文，保留 Go 标识符和文件路径原文。报告至少包含：

1. `problem`：为什么 agent runtime 需要 callback、plugin、instruction utilities 这些横切扩展点。
2. `why_hard`：在 model/tool/agent/event 生命周期中插入逻辑为什么容易污染状态、改变控制流或造成组合冲突。
3. `design_approach`：解释 callback context、plugin manager、hook ordering、early-exit、state/artifact delta、configurable layer 的设计。
4. `code_walkthrough`：逐文件解释关键类型和 hook 点，说明每个 hook 在执行链路中的位置。
5. `extension_points`：列出扩展点地图：
   - agent before/after；
   - model before/after/error；
   - tool before/after/error；
   - event/session hooks；
   - function call modifier；
   - retry and reflect；
   - logging。
6. `tests`：测试覆盖矩阵。
7. `risks`：hook 顺序、状态隔离、插件关闭、configurable/replay、模板变量注入等风险。
8. `next_questions`：下一轮应该继续追问的 8-12 个具体问题。

## 质量要求

- 要把 callback 和 plugin 的边界讲清楚。
- 要指出哪些扩展点是修改请求，哪些是观察/诊断，哪些会改变控制流。
- 如果发现 hook 语义难以组合，请给出具体源码证据。
