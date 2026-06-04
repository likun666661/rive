# CHANGELOG — 第二章 & 第三章示例与文档更新

本文档记录对 `examples/eino-compose-runtime-replica-go` 的第二章 (FieldMapping / Workflow / Chain / Parallel / Branch) 与第三章 (Runnable Stream / Collect / Transform / Callback) 示例补全与文档更新。

---

## M1: Final merge docs — 第三章教学示例与文档补全

### 变更范围

| 文件 | 变更类型 | 说明 |
|------|---------|------|
| `cmd/example/main.go` | 新增 | 新增 4 个第三章教学示例 (example12-15),示例总数增至 15 |
| `compose/runnable.go` | 更新 | Runnable 扩展为 Invoke/Stream/Collect/Transform 四模式与 fallback 矩阵 |
| `compose/stream.go` | 新增 | 基础 Pipe stream、Copy、Merge、Concat |
| `compose/callbacks.go` | 新增 | RunInfo、HandlerBuilder、CallbackWrapper、流输入/输出回调副本 |
| `README.md` | 更新 | 新增第三章功能章节,更新架构总览、包结构说明 |
| `CHANGELOG.md` | 更新 | 本文件,记录第三章变更 |
| `FINAL_SUMMARY.md` | 更新 | 新增第三章功能摘要,明确教学子集边界 |

### 新增示例说明

#### Example 12: Runnable Stream 概念演示
- 展示 `composableRunnable` 四字段设计 (`i` / `s` / `c` / `t`)
- 说明 Invoke/Stream/Collect/Transform fallback 矩阵
- 通过 Graph + InvokableLambda 演示 Runnable[I,O].Invoke 公开 API
- 源码追踪: `compose/runnable.go` 的四模式降级逻辑

#### Example 13: Stream Collect 模式
- 基础 Pipe stream 实现: `NewPipe` / `Recv` / `Send` / `Copy` / `Merge` / `Concat`
- 模拟流式 Lambda 输出 5 个 token
- Collect 按序收集所有分块为完整结果
- 说明 Eino 完整版的 merge 策略 (append/concat/mergeMap)

#### Example 14: Stream Transform 模式
- 流式管道: `生产 → Transform(ToUpper) → Collect`
- 三种变换模式说明: 逐 chunk 变换 / 带状态变换 / 批量变换
- 教学演示,完整图流式执行不在范围内

#### Example 15: Callback 计时模式
- 回调生命周期: `OnStart → Execute → OnEnd/OnError`
- 计时 trace 实现: 记录开始时间、计算耗时
- CallbackWrapper 覆盖 Invoke/Stream/Collect/Transform 包装与流回调副本
- EventLog 在 graph 级别的等效可观测性演示

### 文档更新说明

README.md 第三章新增内容:
1. composableRunnable 四字段设计与四模式 fallback 矩阵
2. 基础 Pipe stream、Copy、Merge、Concat 实现说明
3. Collect / Transform / CallbackWrapper 教学模式
4. 明确组件桥接、图级流式执行、stream field mapping 和流式分支不在当前范围内

### 状态

- 所有测试通过 (`go test ./...`)
- 代码格式化通过 (`gofmt -w .`)
- 示例程序运行通过 (`go run ./cmd/example`)
- 15 个示例覆盖 Chapter 1 (Graph/DAG/Pregel/Info/EventLog) + Chapter 2 (FieldMapping/Workflow/Chain/Parallel/Branch) + Chapter 3 (Stream/Collect/Transform/Callback)
