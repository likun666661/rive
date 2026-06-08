# 精读任务：Runner / Agent / LLM Flow 主循环

你是 Rive 的 OpenCode worker。请对 `google/adk-go` 做只读精读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `agent/agent.go`
- `agent/context.go`
- `agent/live.go`
- `agent/run_config.go`
- `agent/llmagent/llmagent.go`
- `runner/runner.go`
- `runner/runner_test.go`
- `runner/live_runner_test.go`
- `model/llm.go`
- `model/gemini/gemini.go`
- `model/apigee/apigee.go`
- `internal/llminternal/base_flow.go`
- `internal/llminternal/basic_processor.go`
- `internal/llminternal/contents_processor.go`
- `internal/llminternal/instruction_processor.go`
- `internal/llminternal/tools_processor.go`
- `internal/llminternal/functions.go`
- `internal/llminternal/stream_aggregator.go`
- `internal/llminternal/agent_transfer.go`
- `internal/context/invocation_context.go`
- `internal/context/readonly_context.go`

可以按需阅读相关测试和 `internal/testutil/`。

## 输出

只允许写入：

`{{output_dir}}/01-runtime-flow-deep-dive.md`

不要修改仓库源码。写完后用 `rive snapshot capture` 捕获输出文件，并用
`team report --status done --artifact-ref file:{{output_dir}}/01-runtime-flow-deep-dive.md`
报告。

## 报告结构

请使用中文，保留 Go 标识符和文件路径原文。报告至少包含这些章节：

1. `problem`：这一层面临的问题是什么。请解释一次用户请求如何变成 agent invocation、LLM 调用、工具调用、事件输出。
2. `why_hard`：为什么这是问题。重点讨论多 agent、streaming/live、tool call、state persistence、agent transfer 的复杂性。
3. `design_approach`：ADK Go 的解决思路。请画出 `Runner -> Agent -> Flow -> Model/Tool -> Event` 的文本流程图。
4. `code_walkthrough`：源码走读。必须引用关键文件和关键类型/函数，并说明每个函数在链路里的位置。
5. `execution_trace`：按顺序写一条典型执行链，覆盖 `Runner.Run`、session append、`llmAgent.run`、`BaseFlow.Run`、`runOneStep`、processor 管道、tool call、event 持久化。
6. `tests`：列出支撑这些判断的测试文件和测试意图。
7. `risks`：这一层的未读风险、TODO、可能的边界 bug。
8. `next_questions`：下一轮应该继续追问的 8-12 个具体问题。

## 质量要求

- 不要泛泛总结；每个判断都尽量绑定到源码路径。
- 不要只贴文件列表；要解释设计意图和问题背景。
- 如果发现和粗读总纲不一致的地方，要明确指出。
