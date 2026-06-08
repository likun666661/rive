# 精读任务：Workflow Agents / AgentTool / Remote A2A

你是 Rive 的 OpenCode worker。请对 `google/adk-go` 的多智能体编排和远程 A2A 做只读精读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `agent/workflowagents/sequentialagent/*.go`
- `agent/workflowagents/parallelagent/*.go`
- `agent/workflowagents/loopagent/*.go`
- `tool/agenttool/agent_tool.go`
- `agent/remoteagent/a2a_agent.go`
- `agent/remoteagent/v2/*.go`
- `server/adka2a/**`
- `examples/workflowagents/**`
- `examples/a2a/**`
- 相关 tests 和 testdata

## 输出

只允许写入：

`{{output_dir}}/05-workflow-a2a-deep-dive.md`

不要修改仓库源码。写完后捕获 snapshot，并用
`team report --status done --artifact-ref file:{{output_dir}}/05-workflow-a2a-deep-dive.md`
报告。

## 报告结构

请使用中文，保留 Go 标识符和文件路径原文。报告至少包含：

1. `problem`：顺序、并行、循环、多 agent as tool、远程 A2A 分别解决什么编排问题。
2. `why_hard`：状态共享/隔离、事件流聚合、错误传播、parallel backpressure、协议兼容、remote streaming 为什么复杂。
3. `design_approach`：解释 workflow agent variants、AgentTool sandbox、remote agent client/processor、A2A server executor 的分层。
4. `code_walkthrough`：逐文件走读关键实现。
5. `orchestration_flows`：画出至少四条文本流程：
   - sequential workflow；
   - parallel workflow；
   - loop workflow；
   - agent-as-tool；
   - remote A2A client -> server executor -> runner。
6. `tests`：测试覆盖矩阵。
7. `risks`：状态同步、错误一致性、RunLive 支持、legacy A2A 版本、协议差异等。
8. `next_questions`：下一轮应该继续追问的 8-12 个具体问题。

## 质量要求

- 重点解释编排语义，而不是只列出 agent 类型。
- 对 parallel agent 的 goroutine/channel/backpressure 机制要给出源码级说明。
- 对 remote A2A 要明确 client、processor、server、executor 的职责边界。
