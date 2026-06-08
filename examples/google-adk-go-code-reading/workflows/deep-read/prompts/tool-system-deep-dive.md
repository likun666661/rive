# 精读任务：Tool / Function Calling / MCP / Confirmation

你是 Rive 的 OpenCode worker。请对 `google/adk-go` 的工具系统做只读精读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `tool/tool.go`
- `tool/context.go`
- `tool/functiontool/function.go`
- `tool/functiontool/streaming_function.go`
- `tool/mcptoolset/*.go`
- `tool/skilltoolset/toolset.go`
- `tool/skilltoolset/skill/*.go`
- `tool/toolconfirmation/tool_confirmation.go`
- `tool/geminitool/*.go`
- `tool/loadartifactstool/*.go`
- `tool/loadmemorytool/*.go`
- `tool/preloadmemorytool/*.go`
- `tool/exitlooptool/*.go`
- `tool/agenttool/agent_tool.go`
- `internal/toolinternal/**`
- `internal/llminternal/tools_processor.go`
- `internal/llminternal/functions.go`
- `internal/llminternal/request_confirmation_processor.go`

同步阅读 functiontool、MCP、confirmation、skilltoolset 的测试。

## 输出

只允许写入：

`{{output_dir}}/03-tool-system-deep-dive.md`

不要修改仓库源码。写完后捕获 snapshot，并用
`team report --status done --artifact-ref file:{{output_dir}}/03-tool-system-deep-dive.md`
报告。

## 报告结构

请使用中文，保留 Go 标识符和文件路径原文。报告至少包含：

1. `problem`：ADK Go 要统一哪些工具来源，它们为什么需要同一套抽象。
2. `why_hard`：schema generation、args/result 编码、streaming function、long-running、MCP、HITL confirmation、toolset filtering 为什么复杂。
3. `design_approach`：解释 `Tool`、`FunctionTool`、`StreamingFunctionTool`、`Toolset`、confirmation processor、LLM function call handling 的分层。
4. `code_walkthrough`：逐层走读关键文件，说明工具声明如何进入 LLM request，tool call 如何执行并变成 response/event。
5. `tool_lifecycle`：画出至少三条链路：
   - Go function -> schema -> model function call -> `Run` -> function response；
   - MCP list/connect/call；
   - confirmation request -> user confirm/reject -> tool execution。
6. `tests`：测试覆盖矩阵。
7. `risks`：重复确认逻辑、schema 推断边界、MCP 生命周期、streaming/long-running 不一致等。
8. `next_questions`：下一轮应该继续追问的 8-12 个具体问题。

## 质量要求

- 必须明确工具系统和 `internal/llminternal` 的连接点。
- 不要只讲接口；要讲调用链和 failure path。
- 请指出哪些工具是内置能力，哪些是适配外部协议。
