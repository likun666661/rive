# Google ADK Go 粗读 Dogfood 示例

本示例是一轮真实的 Rive dogfood：用户要求 Rive 阅读
[`google/adk-go`](https://github.com/google/adk-go)，先按目录和架构边界
拆成多个 part，再用 OpenCode worker 并行粗读代码，最后由一个汇总节点产出
总纲。

粗读输出文件位于 [`manual/`](./manual/) 目录下。优先阅读
[`manual/00-overview.md`](./manual/00-overview.md)，它把六个分区的粗读结果
整理成 ADK Go 的总体架构地图、主链路、阅读顺序和下一轮精读 DAG 建议。

精读输出文件位于 [`manual/deep-read/`](./manual/deep-read/) 目录下。优先阅读
[`manual/deep-read/00-final-architecture-guide.md`](./manual/deep-read/00-final-architecture-guide.md)，
它汇总六份源码级精读报告，适合作为维护或复刻 ADK Go 架构前的技术手册。
追加的第七章
[`manual/deep-read/07-agent-flow-react-multi-agent-deep-dive.md`](./manual/deep-read/07-agent-flow-react-multi-agent-deep-dive.md)
专门深读 Agent Flow、ReAct 循环、transfer-to-agent、多 Agent 路由、策略插件和后续复刻 DAG。

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

## 精读产物

精读 DAG 已经真实跑完一轮，Rive root work `work_3de20f1f1c3f47a2834d3beccaa68d43`
最终为 `done`，graph hygiene 为 `clean`。产物如下：

| 章节 | 文件 | 关注点 |
| --- | --- | --- |
| 总纲 | [`manual/deep-read/00-final-architecture-guide.md`](./manual/deep-read/00-final-architecture-guide.md) | ADK Go 架构总图、六章索引、跨模块链路、维护者问题、下一轮 DAG |
| 第一章 | [`manual/deep-read/01-runtime-flow-deep-dive.md`](./manual/deep-read/01-runtime-flow-deep-dive.md) | Runner / Agent / LLM Flow 主循环 |
| 第二章 | [`manual/deep-read/02-state-lifecycle-deep-dive.md`](./manual/deep-read/02-state-lifecycle-deep-dive.md) | Session / Memory / Artifact 状态生命周期 |
| 第三章 | [`manual/deep-read/03-tool-system-deep-dive.md`](./manual/deep-read/03-tool-system-deep-dive.md) | Tool / Function Calling / MCP / Confirmation |
| 第四章 | [`manual/deep-read/04-callback-plugin-deep-dive.md`](./manual/deep-read/04-callback-plugin-deep-dive.md) | Callback / Plugin / Instruction 扩展机制 |
| 第五章 | [`manual/deep-read/05-workflow-a2a-deep-dive.md`](./manual/deep-read/05-workflow-a2a-deep-dive.md) | Workflow Agents / AgentTool / Remote A2A |
| 第六章 | [`manual/deep-read/06-entrypoint-deploy-deep-dive.md`](./manual/deep-read/06-entrypoint-deploy-deep-dive.md) | CLI / Server / Deploy / Telemetry / Examples |
| 第七章 | [`manual/deep-read/07-agent-flow-react-multi-agent-deep-dive.md`](./manual/deep-read/07-agent-flow-react-multi-agent-deep-dive.md) | Agent Flow / ReAct / transfer-to-agent / Multi-Agent / 复刻 DAG |

这轮 dogfood 的一个工程结论是：大规模阅读节点不适合让 OpenCode 在节点内部继续
使用自己的 `task` fan-out，否则会和 Rive 外层 DAG 争夺编排权，容易出现外层
dispatch 长时间不 report。后半程改为禁用 OpenCode 内部 `task` tool 后，Rive
作为唯一编排层，剩余节点能稳定收口。

第七章是后续追加的一轮专题 research DAG：四个 OpenCode reader 分区读取
LLM Agent ReAct loop、transfer/multi-agent routing、planner/reflection/skills、
examples/configurable patterns，再由 final writer 合成中文深读章节。它不是代码实现，
而是给 `rive-adk-go` 后续复刻 Agent Flow / ReAct / 多 Agent 能力使用的实现计划和风险清单。

## 下一轮精读 Workflow

精读 DAG 和节点 prompt 已整理成可导入的 Rive workflow package：

- [`workflows/deep-read/workflow.yaml`](./workflows/deep-read/workflow.yaml)
- [`workflows/deep-read/prompts/`](./workflows/deep-read/prompts/)

这个 workflow 继续沿用粗读的六个分区，但把每个节点的要求提升为源码级
精读：必须写出问题背景、为什么难、设计思路、源码走读、执行链路、测试证据、
风险和下一轮问题。最后的 `final-architecture-guide` 节点只消费六份精读
报告，不重新通读仓库。

快速验证：

```sh
rive workflow validate examples/google-adk-go-code-reading/workflows/deep-read
```

实例化但不启动 worker：

```sh
rive workflow run google-adk-go.deep-read \
  --command-id run-google-adk-go-deep-read-dry \
  --no-scheduler \
  --param repo_path=/Users/likun/Desktop/workspace-for-google-adk-go/adk-go \
  --param output_dir=/tmp/rive-google-adk-go-deep-read
```
