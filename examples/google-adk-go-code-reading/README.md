# Google ADK Go 粗读 Dogfood 示例

本示例是一轮真实的 Rive dogfood：用户要求 Rive 阅读
[`google/adk-go`](https://github.com/google/adk-go)，先按目录和架构边界
拆成多个 part，再用 OpenCode worker 并行粗读代码，最后由一个汇总节点产出
总纲。

输出文件位于 [`manual/`](./manual/) 目录下。优先阅读
[`manual/00-overview.md`](./manual/00-overview.md)，它把六个分区的粗读结果
整理成 ADK Go 的总体架构地图、主链路、阅读顺序和下一轮精读 DAG 建议。

## 运行结构

- 源仓库：`https://github.com/google/adk-go`
- 本地阅读基线：`81a63d8feb7d713b1731f0c740d95574eb64dafa`
- Rive 根节点：`work_8ef3324112f14bb6946ca0817d848502`
- 调度器：`sched_0ef77dab3aa54433bb5a47534c5b4dbc`
- Runner：OpenCode
- Worker：6 个 reader 节点 + 1 个 final judge 节点
- 并发：`max_parallel=4`
- 验收模式：`auto-reported`
- 结果：root work `done`，graph hygiene `clean`

## 分区方式

| 分区 | 文件 | 关注点 |
| --- | --- | --- |
| 总纲 | [`manual/00-overview.md`](./manual/00-overview.md) | 总体架构、跨模块主链路、阅读路线、下一轮精读 DAG |
| 第一部分 | [`manual/01-agent-runtime-model-loop.md`](./manual/01-agent-runtime-model-loop.md) | `agent`、`llmagent`、`runner`、`model`、LLM flow |
| 第二部分 | [`manual/02-session-memory-artifact.md`](./manual/02-session-memory-artifact.md) | `session`、`memory`、`artifact` 与状态服务 |
| 第三部分 | [`manual/03-tools-function-calling-mcp.md`](./manual/03-tools-function-calling-mcp.md) | 工具接口、函数工具、MCP、Skills、人工确认 |
| 第四部分 | [`manual/04-callbacks-plugins-instructions.md`](./manual/04-callbacks-plugins-instructions.md) | 回调、插件、指令工具和可配置层 |
| 第五部分 | [`manual/05-workflow-multiagent-a2a.md`](./manual/05-workflow-multiagent-a2a.md) | Workflow Agents、多智能体编排、AgentTool、远程 A2A |
| 第六部分 | [`manual/06-entrypoints-server-telemetry.md`](./manual/06-entrypoints-server-telemetry.md) | CLI、Server、部署、Telemetry、Examples |

## 节点阅读要求

每个 reader 节点都被要求围绕四个问题输出：

- 面临的问题是什么；
- 为什么这是问题；
- 解决思路是什么；
- ADK Go 代码怎么落地。

同时，每个节点需要列出关键目录、关键类型/函数、核心执行流、测试覆盖和
后续值得继续深读的问题。这样产物不是泛泛总结，而是可以继续驱动下一轮
精读 DAG 的粗读地图。

## Dogfood 观察

- 目录级粗读适合用多个 OpenCode worker 并行跑；ADK Go 这种包边界清晰的
  Go 仓库很适合按 `agent/session/tool/plugin/workflow/server` 分区。
- final judge 不直接阅读整个仓库，只消费前面六个 part 的产物，避免上下文
  被源码细节打爆。
- 这轮有一个 worker 误把制品写到仓库根目录，收尾时已清理；正式 example
  只保留 `manual/` 下的中文产物。
- 这是一轮粗读，不等于源码审计。下一轮如果要深入，应按总纲中的建议继续
  拆分到 runner/flow、state lifecycle、tool confirmation、A2A server 等
  更细的主题。
