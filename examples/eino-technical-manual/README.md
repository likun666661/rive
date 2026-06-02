# Eino 技术手册 Dogfood 示例

本示例是一次真实的 Rive dogfood 运行：用户要求 Rive 阅读 CloudWeGo Eino
仓库，将阅读任务拆分为 Work DAG，启动 OpenCode worker，最终产出一份详尽
的技术手册。

输出文件位于 [`manual/`](./manual/) 目录下。

## 演示内容

- 一次宽泛的研究/文档请求可以被转换为一个 Work DAG。
- 多个 OpenCode worker 可以并行阅读不同的代码区域。
- 每个 worker 产出具体的制品（artifact），而不仅仅是返回自然语言描述。
- Worker 完成后将 Work 节点移至 `reviewable` 状态；明确确认后移至 `done`。
- 当第一轮输出深度不足时，可以用更细粒度的第二轮 DAG 复用第一轮的粗略
  报告作为种子素材。

## 运行结构

最终手册由第二轮章节级 DAG 产出：

- 根节点：`work_616202594ad9431fb1bd763001620d4c`
- 调度器：`sched_1338d328fc5245f486063f823ceeef93`
- Runner：OpenCode
- 验收模式：manual
- Dogfood 工作区中的输出目录：
  `docs/rive-eino-manual-v2/`

第一轮粗粒度 DAG 产出了有用的笔记，但输出深度不足。从中得到的有用教训
是不要创建一个巨型综合节点。相反，Rive 创建了带硬性合约（hard contract）
的章节级制品节点：

- 明确的输出文件路径；
- 最低深度 / 行数要求；
- 源码文件引用；
- 问题 / 难点 / 设计思路 / 源码走读 / 模式 / 陷阱；
- 仅在文件确实存在后才可手动确认。

## 手册章节

| 章节 | 文件 |
| --- | --- |
| Compose Graph 编译/运行时模型 | [`manual/01-compose-graph-runtime.md`](./manual/01-compose-graph-runtime.md) |
| Workflow、Chain 与字段映射 | [`manual/02-workflow-chain-field-mapping.md`](./manual/02-workflow-chain-field-mapping.md) |
| Runnable、流与回调 | [`manual/03-runnable-stream-callback.md`](./manual/03-runnable-stream-callback.md) |
| Checkpoint、中断与恢复 | [`manual/04-checkpoint-interrupt-resume.md`](./manual/04-checkpoint-interrupt-resume.md) |
| 组件、模型、工具与提示契约 | [`manual/05-components-model-tool-prompt.md`](./manual/05-components-model-tool-prompt.md) |
| Schema 与提供者适配器 | [`manual/06-schema-provider-adapters.md`](./manual/06-schema-provider-adapters.md) |
| Agent 流程、ReAct 与多 Agent 宿主 | [`manual/07-agent-flow-react-multiagent.md`](./manual/07-agent-flow-react-multiagent.md) |

## 实践经验

对于文档和研究类工作，Rive 需要的是面向制品的节点，而不是一次大型汇总
传递。最强的模式是：

```text
人类目标
  -> 架构师创建聚焦的章节/研究 DAG
  -> worker 产出具体文件
  -> 根据制品质量确认/重新打开
  -> 上传或发布制品
```

这将 agent 工作从“阅读并总结”转变为基于账本（ledger）的文档生产流程。
