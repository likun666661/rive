# CHANGELOG — 第二章示例与文档更新

本文档记录对 `examples/eino-compose-runtime-replica-go` 的第二章 (FieldMapping / Workflow / Chain / Parallel / Branch) 示例补全与文档更新。

---

## I4: Examples README Changelog — 第二章示例与文档

### 变更范围

| 文件 | 变更类型 | 说明 |
|------|---------|------|
| `cmd/example/main.go` | 重写 | 新增 6 个第二章示例 (example6-11),保留原有 5 个第一章示例 |
| `README.md` | 重写 | 中文文档,涵盖 FieldMapping / Workflow / Chain / Parallel / Branch 的问题-方案-设计说明 |
| `CHANGELOG.md` | 重写 | 本文件 |
| `FINAL_SUMMARY.md` | 更新 | 增加第二章功能摘要 |

### 新增示例说明

#### Example 6: FieldMapping 字段映射
- 展示六个构造函数 (`MapFields`, `FromField`, `ToField`, `MapFieldPaths`, `FromFieldPath`, `ToFieldPath`)
- 展示 `WithCustomExtractor` 自定义提取器
- 可运行示例: Workflow 中使用 `FromField` / `ToField` 实现跨节点字段级别数据传递
- 输出类型: `*SearchInput → map[string]any`,通过 FieldMapping 提取/注入字段

#### Example 7: Workflow 声明式编排
- 演示三节点 pipeline (enrich → score → END)
- 使用 `AddInput` + `FromField` 替代手动 `AddEdge`
- 多前驱汇聚到 END (score 的 Confidence + START 的 Query)
- 输出类型: `*SearchInput → *FinalOutput`

#### Example 8: Chain Builder 线性管道
- 演示 `AppendLambda` 三次构建线性管道 (lower → reverse → prefix)
- 展示 `preNodeKeys` 自动追踪尾部节点
- 输出类型: `string → string`

#### Example 9: Parallel 并行执行
- 演示 `NewParallel` + `AddLambda` 构建并行节点组
- `AppendParallel` 嵌入 Chain,合并节点通过 `map[string]any` 区分来源
- 输出类型: `string → string`

#### Example 10: Branch 条件分支
- 演示 `NewChainBranch` + `AddLambda` 构建条件分支
- `AppendBranch` 嵌入 Chain
- 短文本 (≤5) 走 short 路径,长文本 (>5) 走 long 路径
- 两次 `Invoke` 验证不同输入路由到不同分支

#### Example 11: 跳过的特性
- 列出本复刻版明确跳过的 Eino 能力及跳过理由
- 包含组件桥接、Stream 执行、Callback、Checkpoint 等 15 项

### 文档更新说明

README.md 以中文重写,核心内容包括:

1. **FieldMapping 解决的问题**:
   - 相邻节点输入/输出类型不匹配
   - 前驱输出是大结构体,后继只需一个字段
   - 多个前驱的不同字段需要汇聚

2. **Workflow 解决的问题**:
   - 手动 AddEdge 声明分散,代码冗长
   - 字段映射需额外配置
   - 控制依赖与数据依赖混在一起

3. **Chain 解决的问题**:
   - 线性管道需手动 AddEdge,重复繁琐
   - 没有内建 "this then that" 语义

4. **Parallel 解决的问题**:
   - 同一输入执行多个独立操作,需手动创建扇出拓扑

5. **Branch 解决的问题**:
   - 根据输入内容条件性选择路径,普通图只能静态连接

6. **跳过的 Eino 能力**: 组件桥接、Stream、Callback、Checkpoint、可视化等

### 状态

- 所有测试通过 (`go test ./...`)
- 代码格式化通过 (`gofmt -w .`)
- 示例程序运行通过 (`go run ./cmd/example`)
- 文档以中文编写,符合契约要求
