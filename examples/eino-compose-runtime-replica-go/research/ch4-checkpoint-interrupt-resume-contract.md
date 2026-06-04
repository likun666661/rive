# Chapter 04 研究笔记：Checkpoint / Interrupt / Resume 教学子集

## 面临的问题

Eino 图运行时中的暂停点不是一个普通错误。节点、工具、子图和 runnable 可以嵌套执行，同一个工具还可能以不同 call id 并发出现。如果恢复时只说“继续上次失败的节点”，运行时无法知道应该把恢复数据送到哪个叶子执行点，也无法避免重新执行已经完成的副作用。

## 为什么这是问题

- 扁平节点 ID 无法区分 `tool:lookup:call_1` 与 `tool:lookup:call_2`。
- 子图中断需要向上传播，但恢复数据必须最终回到子图内部的叶子地址。
- `StreamReader` 这类一次性值不能直接放入 checkpoint，必须先物化。
- 已中断过但本轮没有被定向恢复的执行点必须重新中断，否则状态会被静默吞掉。

## Eino 的解决思路

Eino 把暂停/恢复拆成四个稳定机制：

1. **Address**：用 `AddressSegment{Type, ID, SubID}` 构成结构化执行地址。
2. **InterruptSignal tree**：内部保留树状中断信号，面向用户暴露扁平 root-cause contexts。
3. **Checkpoint maps**：将 `interruptID -> Address/State` 写入 checkpoint，恢复时重新注入上下文。
4. **Resume context**：`GetInterruptState` 回答“我之前是否中断过”，`GetResumeContext` 回答“我是否是本轮恢复目标”。

## 本复刻版落地范围

本轮实现一个教学子集，而不是完整 Eino checkpoint：

- `compose/address.go`：实现 runnable/node/tool 地址段、SubID 和地址字符串。
- `compose/interrupt.go`：实现 `Interrupt`、`StatefulInterrupt`、`CompositeInterrupt`、信号树和扁平 context。
- `compose/checkpoint.go`：实现 context 型 `CheckPointStore`、`WithCheckPoint`、in-memory store 和 stream materialization helpers。
- `compose/resume.go`：实现定向 resume data 与 typed state/data 读取 API。
- `compose/graph_run.go`：runner 自动进入 graph/node address scope；节点中断时保存 checkpoint。

## 明确不做

- 不复制 channel manager 的全量在途状态。
- 不做嵌套子图 checkpoint forward。
- 不做序列化注册、状态迁移和跨进程存储。
- 不做 ToolsNode rerun skip handler。

这些能力需要更完整的运行时状态模型。当前目标是让读者能跑通最小的“节点中断 -> 保存 checkpoint -> 指定 interrupt id 恢复”闭环，并理解 Eino 为什么要用结构化地址和 signal tree。

