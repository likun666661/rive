# 第三章运行时契约：Runnable / Stream / Callback 与教育复刻版实现边界

> 基于 Eino 技术手册第三章 + 当前 Go 复刻版源码审计
> 目标读者：实施工人（I1/I2/I3/I4）及后续验证者
> 语言：中文

> 状态说明：本文最初作为第三章实现前的 baseline audit，记录了当时复刻版缺失的 Stream / Collect / Transform / Callback 能力。当前代码已经按本文后半部分的契约补齐教育子集：`compose/runnable.go` 支持 Runnable 四模式 fallback，`compose/stream.go` 提供基础 Pipe stream / Copy / Merge / Concat，`compose/callbacks.go` 提供 CallbackWrapper。本文中保留的“未实现”表格是实现前对照，不代表最终代码状态；最终功能边界以 README、FINAL_SUMMARY 和测试为准。

---

## 目录

1. [Eino 第三章完整能力清单](#1-eino-第三章完整能力清单)
2. [复刻版已实现的第三章子集](#2-复刻版已实现的第三章子集)
3. [Invoke / Stream / Collect / Transform 降级优先级规则](#3-invoke--stream--collect--transform-降级优先级规则)
4. [Stream 所有权规则（当前状态与未来要求）](#4-stream-所有权规则当前状态与未来要求)
5. [Callback 时序与 RunInfo 字段](#5-callback-时序与-runinfo-字段)
6. [明确排除的范围](#6-明确排除的范围)
7. [源码对照表](#7-源码对照表)

---

## 1. Eino 第三章完整能力清单

Eino 第三章（`03-runnable-stream-callback.md`）定义了四层协作设计：

### 第一层：Runnable 接口 + packer（`compose/runnable.go`）

```
Runnable[I,O] 接口:
├── Invoke(ctx, I, ...Option) → (O, error)
├── Stream(ctx, I, ...Option) → (*schema.StreamReader[O], error)
├── Collect(ctx, *schema.StreamReader[I], ...Option) → (O, error)
└── Transform(ctx, *schema.StreamReader[I], ...Option) → (*schema.StreamReader[O], error)
```

每个图节点被编译为 `composableRunnable`，内部存储 `i` 和 `t` 两个核心函数指针，另外两种模式（Stream / Collect）通过降级派生。`runnablePacker` 接收组件的原始函数指针（最多 4 个，部分可为 nil），通过 12 个降级函数自动填充全部 4 种模式。

### 第二层：Stream 原语

- `schema.StreamReader[T]` / `schema.StreamWriter[T]`（基于 goroutine channel）
- `schema.StreamReader.Copy(n)` — 扇出（基于 `parentStreamReader` + `childStreamReader` 链表共享缓冲）
- `schema.MergeStreamReaders` — 扇入
- `schema.StreamReaderWithConvert` — 类型转换
- 内部 `streamReader` 接口：`copy` / `getType` / `getChunkType` / `merge` / `withKey` / `close` / `toAnyStreamReader` / `mergeWithNames`
- `concatStreamReader[T]`：消费所有 chunk → `internal.ConcatItems` 拼接 → 始终关闭 reader
- `RegisterStreamChunkConcatFunc[T]`：用户注册自定义 concat 逻辑

### 第三层：Callback 引擎

- 公开 `Handler` 接口，包含 5 个阶段：
  | 阶段 | 时机 | I/O 类型 |
  |------|------|---------|
  | `TimingOnStart` | 组件运行前 | `CallbackInput`（值） |
  | `TimingOnEnd` | 组件成功后 | `CallbackOutput`（值） |
  | `TimingOnError` | 组件返回错误 | `error` |
  | `TimingOnStartWithStreamInput` | 组件接收流输入（Collect/Transform） | `*StreamReader[CallbackInput]`（副本） |
  | `TimingOnEndWithStreamOutput` | 组件产生流输出（Stream/Transform） | `*StreamReader[CallbackOutput]`（副本） |
- `RunInfo` 结构体（`callbacks/interface.go:41`）：
  | 字段 | 含义 |
  |------|------|
  | `Name` | 从 `WithNodeName` 获取的图节点名；未经 `InitCallbacks` 的独立组件为空 |
  | `Type` | 从 `components.Typer` 获取的实现标识（如 `"OpenAI"`）；回退为反射推导的类型名 |
  | `Component` | 从 `components.Component` 获取的类别常量（如 `ComponentOfChatModel`、`ComponentOfRetriever`）；图级调用固定为 `"Graph"`/`"Chain"`/`"Workflow"`，Lambda 为 `"Lambda"` |
- `TimingChecker` 可选接口：`Needed(ctx, info, timing)` 决定是否跳过流复制和 goroutine 分配
- `HandlerBuilder`：`OnStartFn` / `OnEndFn` / `OnErrorFn` / `OnStartWithStreamInputFn` / `OnEndWithStreamOutputFn`
- 全局处理器（`AppendGlobalHandlers`）先于每次调用处理器执行
- 路径范围限定回调：`WithCallbacks(WithNodePath("tool_node", "calculator")).DesignateHandler(handler)`

### 第四层：Component-to-Graph-Node 桥接

- `toChatModelNode` / `toRetrieverNode` / `toToolsNode` / `toChatTemplateNode` 等
- `toComponentNode` 统一入口：构建 `executorMeta`（component / componentImplType / isComponentCallbackEnabled）
- `parseExecutorInfoFromComponent` 检查 `components.Typer` 和 `callbacks.Checker`
- `runnableLambda` 调用 `newRunnablePacker` 填充缺失方法
- `isComponentCallbackEnabled` 标志（取反）控制 compose 层是否用 `runWithCallbacks` 包装组件方法，防止回调重复触发

### 关键生命周期机制

- `runWithCallbacks`：OnStart → 执行 → OnEnd（成功）或 OnError（失败）
- 四种模式特定包装：
  - `invokeWithCallbacks`：OnStart(值) → Invoke → OnEnd(值)
  - `streamWithCallbacks`：OnStart(值) → Stream → OnEndWithStreamOutput(流)
  - `collectWithCallbacks`：OnStartWithStreamInput(流) → Collect → OnEnd(值)
  - `transformWithCallbacks`：OnStartWithStreamInput(流) → Transform → OnEndWithStreamOutput(流)
- `initGraphCallbacks` / `initNodeCallbacks` 在每个节点执行前设置上下文中 `RunInfo`
- `executorMeta.component` / `executorMeta.componentImplType` 决定 RunInfo 的 `Component` / `Type` 字段

---

## 2. 复刻版已实现的第三章子集

以下逐行对照复刻版当前源码，标注与 Eino 第三章的覆盖情况。

### 2.1 `compose/runnable.go` — Runnable 接口

| Eino 能力 | 复刻版状态 | 源码位置 |
|-----------|-----------|---------|
| `Runnable[I,O].Invoke(ctx, input I) (O, error)` | **已实现** | `runnable.go:8-10` |
| `Runnable[I,O].Stream(...)` | **未实现** — 接口只有 Invoke，无 Stream/Collect/Transform 方法 | `runnable.go:8-10` |
| `Runnable[I,O].Collect(...)` | **未实现** | — |
| `Runnable[I,O].Transform(...)` | **未实现** | — |
| `composableRunnable`（i + t 双函数） | **部分实现** — 有 `i`（invoke）和 `s`（stream fallback），无 `t`（transform） | `runnable.go:12-15` |
| `runnablePacker` 12 个降级函数 | **未实现** — 无降级矩阵，仅有简单的 stream → invoke fallback | `runnable.go:24-36` |
| `invokeByStream` | **未实现** | — |
| `invokeByCollect` | **未实现** | — |
| `invokeByTransform` | **未实现** | — |
| `streamByTransform` | **未实现** | — |
| `streamByInvoke`（将结果包装为数组流） | **部分实现** — `stream()` 回退到 `i()` 但返回裸值，未包装为流 | `runnable.go:28-34` |
| `streamByCollect` | **未实现** | — |
| `collectByTransform` | **未实现** | — |
| `collectByInvoke` | **未实现** | — |
| `collectByStream` | **未实现** | — |
| `transformByStream` | **未实现** | — |
| `transformByCollect` | **未实现** | — |
| `transformByInvoke` | **未实现** | — |
| `Lambda` + `InvokableLambda` 构造 | **已实现** | `runnable.go:42-72` |

### 2.2 Stream 原语

| Eino 能力 | 复刻版状态 | 源码位置 |
|-----------|-----------|---------|
| `schema.StreamReader[T]` | **未实现** — 无 schema 包 | — |
| `schema.StreamWriter[T]` | **未实现** | — |
| `Copy` 扇出 | **未实现** | — |
| `MergeStreamReaders` 扇入 | **未实现** | — |
| `StreamReaderWithConvert` | **未实现** | — |
| 内部 `streamReader` 接口 | **未实现** | — |
| `concatStreamReader` | **未实现** | — |
| `RegisterStreamChunkConcatFunc` | **未实现** | — |
| `streamFieldMap` | **Stub** — panic("not implemented") | `field_mapping.go:450-454` |

### 2.3 Callback 引擎

| Eino 能力 | 复刻版状态 | 源码位置 |
|-----------|-----------|---------|
| `RunInfo`（Name / Type / Component） | **未实现** — 无 callbacks 包 | — |
| `TimingOnStart` | **未实现** | — |
| `TimingOnEnd` | **未实现** | — |
| `TimingOnError` | **未实现** | — |
| `TimingOnStartWithStreamInput` | **未实现** | — |
| `TimingOnEndWithStreamOutput` | **未实现** | — |
| `TimingChecker` 接口 | **未实现** | — |
| `HandlerBuilder` | **未实现** | — |
| `runWithCallbacks` 包装器 | **未实现** | — |
| `invokeWithCallbacks` / `streamWithCallbacks` / 等 | **未实现** | — |
| `initGraphCallbacks` / `initNodeCallbacks` | **未实现** | — |
| 全局处理器 `AppendGlobalHandlers` | **未实现** | — |
| 路径范围限定回调 `WithCallbacks(WithNodePath(...))` | **未实现** | — |

### 2.4 Event Log（复刻版特有的观测机制）

复刻版有独立于 Eino Callback 引擎的简化事件日志系统：

| 能力 | 状态 | 源码位置 |
|------|------|---------|
| `EventLog`（`sync.Mutex` 线程安全） | **已实现** | `event_log.go:35-38` |
| `EventType`（10 种） | **已实现** | `event_log.go:11-22` |
| `Event` 结构（Type / Timestamp / NodeKey / GraphName / Step / Input / Output / Error） | **已实现** | `event_log.go:24-33` |
| 与 RunInfo 字段对比：`NodeKey` ≈ `Name`（均为节点标识）；`EventLog` 无 `Type` / `Component` 字段 | — | — |

EventLog 的 10 种事件类型：
- `EventNodeStart` / `EventNodeEnd` / `EventNodeError` — 对应 OnStart / OnEnd / OnError（不含流输入/输出变体）
- `EventNodeSkipped` — Skip 传播（无 Eino 等价物，Eino 通过 `reportSkip` 处理）
- `EventGraphStart` / `EventGraphEnd` / `EventGraphError` — 图层面的生命周期（无 Eino 等价物）
- `EventChannelReady` / `EventCheckpoint` — 未使用（已定义常量但无生产代码）
- `EventMaxStepsHit` — Pregel 模式步数上限

### 2.5 Component Bridge

| Eino 能力 | 复刻版状态 | 源码位置 |
|-----------|-----------|---------|
| `toChatModelNode` / `toRetrieverNode` / 等 | **未实现** — 仅有 `Lambda` 抽象 | — |
| `toComponentNode` | **未实现** | — |
| `executorMeta`（component / componentImplType / isComponentCallbackEnabled） | **未实现** | — |
| `parseExecutorInfoFromComponent` | **未实现** | — |
| `isComponentCallbackEnabled` 取反标志 | **未实现** | — |
| 透传节点 `toPassthroughNode` | **部分实现** — Chain 内联 identity lambda，无独立 passthrough 节点类型 | `chain.go:49-53` |

### 2.6 已存在的辅助能力

| 能力 | 状态 | 源码位置 |
|------|------|---------|
| `ComponentType`（Graph / Lambda / Workflow / Chain / Unknown） | **已实现** | `types.go:19-27` |
| `GraphNodeInfo`（Name / Component） | **已实现** | `introspect.go:3-9` |
| `GraphInfo`（Name / Nodes / Edges / TriggerMode / MaxSteps） | **已实现** | `introspect.go:16-28` |

---

## 3. Invoke / Stream / Collect / Transform 降级优先级规则

### 3.1 Eino 完整降级矩阵

Eino 在 `newRunnablePacker`（`compose/runnable.go:336-400`）中实现，优先级从高到低：

#### Invoke 降级链

```
Native Invoke → invokeByStream → invokeByCollect → invokeByTransform
```

| 优先级 | 实现 | 工作原理 |
|--------|------|---------|
| 1（最高） | 原生 `i` 函数 | 直接调用 |
| 2 | `invokeByStream` | 调用 Stream → concatStreamReader 合并为单个值 |
| 3 | `invokeByCollect` | 将输入包装为单元素数组流 → 调用 Collect |
| 4（最低） | `invokeByTransform` | 将输入包装为数组流 → Transform → concatStreamReader 合并输出 |

#### Stream 降级链

```
Native Stream → streamByTransform → streamByInvoke → streamByCollect
```

| 优先级 | 实现 | 工作原理 |
|--------|------|---------|
| 1（最高） | 原生 `s` 函数 | 直接调用 |
| 2 | `streamByTransform` | 将输入包装为数组流 → Transform |
| 3 | `streamByInvoke` | 调用 Invoke → 将输出包装为单元素数组流 |
| 4（最低） | `streamByCollect` | 将输入包装为数组流 → Collect → 将输出包装为数组流 |

#### Collect 降级链

```
Native Collect → collectByTransform → collectByInvoke → collectByStream
```

| 优先级 | 实现 | 工作原理 |
|--------|------|---------|
| 1（最高） | 原生 `c` 函数 | 直接调用 |
| 2 | `collectByTransform` | Transform → concatStreamReader 合并输出 |
| 3 | `collectByInvoke` | concatStreamReader 合并输入 → Invoke |
| 4（最低） | `collectByStream` | concatStreamReader 合并输入 → Stream → concatStreamReader 合并输出 |

#### Transform 降级链

```
Native Transform → transformByStream → transformByCollect → transformByInvoke
```

| 优先级 | 实现 | 工作原理 |
|--------|------|---------|
| 1（最高） | 原生 `t` 函数 | 直接调用 |
| 2 | `transformByStream` | concatStreamReader 合并输入 → Stream |
| 3 | `transformByCollect` | Collect → 将输出包装为数组流 |
| 4（最低） | `transformByInvoke` | concatStreamReader 合并输入 → Invoke → 将输出包装为数组流 |

#### 设计理由

- **通过 Transform 调用优于通过 Stream 调用**：一次流转换 vs 两次（合并 + 展开）
- **通过 Stream 调用优于通过 Collect 调用**：一次 concat vs 两次（合并输入 + 合并输出）
- **`concatStreamReader` 是关键降级原语**：任何需要将流折叠为单值的地方都依赖它；调用方负责 `defer sr.Close()`

### 3.2 复刻版当前降级实现

复刻版的降级远更简单，仅有以下两条路径：

#### Invoke

```
Native Invoke（唯一路径，无降级）
```

- `composableRunnable.invoke()` 需要 `i` 非 nil，否则报错（`runnable.go:17-22`）
- 不支持从 Stream / Collect / Transform 降级

#### Stream

```
Native Stream → 简单 invoke fallback（无流包装）
```

```go
// compose/runnable.go:24-36
func (cr *composableRunnable) stream(ctx context.Context, input any) (any, error) {
    if cr.s != nil {
        return cr.s(ctx, input)
    }
    if cr.i != nil {
        out, err := cr.i(ctx, input)
        if err != nil {
            return nil, err
        }
        return out, nil   // ← 注意：返回裸值，未包装为 stream reader
    }
    return nil, fmt.Errorf("runnable: Stream not supported")
}
```

**关键差异**：复刻版的 `stream()` 回退到 `invoke()` 时返回的是裸值而非 `*StreamReader`。这意味着即使 `stream()` 被调用，任何期望获取 `StreamReader` 的代码都会获得错误类型。在实际使用中，图运行时的 `taskManager.submit()` 始终调用 `invoke()`（`graph_manager.go:146`），从未调用 `stream()`，因此该降级路径实际上未被使用。

#### Collect / Transform

未实现。无对应函数。

### 3.3 未来实现时的降级测试要求

当本复刻版未来实现 Stream / Collect / Transform 时，测试必须覆盖以下降级路径：

| 组件能力 | 用户调用 Invoke | 用户调用 Stream | 用户调用 Collect | 用户调用 Transform |
|----------|----------------|-----------------|------------------|---------------------|
| 仅 Invoke | 直通 | invoke → 包装为数组流 | concat输入 → Invoke | concat输入 → Invoke → 包装为数组流 |
| Invoke + Stream | 直通 | 直通 | concat输入 → Stream → concat输出 | concat输入 → Stream |
| Invoke + Stream + Collect | 直通 | 直通 | 直通 | Collect → 包装为数组流 |
| Invoke + Stream + Collect + Transform | 直通 | 直通 | 直通 | 直通 |

---

## 4. Stream 所有权规则（当前状态与未来要求）

### 4.1 当前状态

**复刻版无流机制**。当前运行时中：

- `taskManager.submit()` 同步调用 `cr.invoke(ctx, input)`（`graph_manager.go:146`）
- `invoke()` 返回裸值 `(any, error)`
- 无 `StreamReader` 抽象
- 无 goroutine leak 风险（因为无流 goroutine）

### 4.2 Eino 规范中的所有权规则（未来实现时 MUST 强制执行）

当复刻版未来引入流机制时，必须遵守以下所有权规则：

#### 规则 1：Copy 必须在第一次 Recv 之前调用

```
StreamReader.Copy(n) → N 个子 reader + 原始 reader 变为不可用
```

`Copy()` 调用 `copyStreamReaders`（`schema/stream.go:792-821`），将原始 reader 替换为 `parentStreamReader`，后者惰性从源 channel 拉取数据并通过链表共享缓冲区（`parentStreamReader` + `childStreamReader`，`schema/stream.go:784-898`）扇出给所有子 reader。

**违反后果**：在 Copy 之后对原始 reader 调用 `Recv()` → panic。

#### 规则 2：每个子 reader 必须调用 Close()

```
N 份副本 → N 次 Close()
```

`parentStreamReader.close()`（`schema/stream.go:868-881`）维护 `closedNum` 计数器。只有在所有子 reader 都调用了 `Close()` 之后，才会关闭底层的原始 reader（从而关闭 `toStream()` 中的 goroutine）。

**违反后果**：任何子 reader 未关闭 → 原始 reader 永不关闭 → 底层 channel goroutine 永久泄漏。

#### 规则 3：回调引擎的 N+1 副本模式

```
OnStartWithStreamInput / OnEndWithStreamOutput:
  cpy(n = len(handlers) + 1)
    ├── handlers[0] 获取副本 0（必须 Close）
    ├── handlers[1] 获取副本 1（必须 Close）
    ├── ...
    └── 实际消费者获取副本 N（必须 Close）
```

`OnWithStreamHandle`（`internal/callbacks/inject.go:143`）调用 `cpy(len(handlers) + 1)` 创建 N+1 份输入流的副本。

**测试要求**：验证每个流副本被消费并关闭，验证无 goroutine 泄漏。

#### 规则 4：concatStreamReader 始终 Close

`concatStreamReader[T]`（`compose/stream_concat.go:50`）开头有 `defer sr.Close()`。这是硬性要求，因为 concat 消费完整个流后将不再需要它。

### 4.3 与 FieldMapping streamFieldMap 的关系

当前 `streamFieldMap` 是 Stub（`field_mapping.go:450-454`，`panic("not implemented")`），不涉及任何所有权问题。未来实现时，`streamFieldMap` 需要接收一个 `streamReader` 并返回一个新的 `streamReader`，此时需要遵守：输入的 stream reader 由调用方负责 Close，输出的 stream reader 由下游负责 Close。

---

## 5. Callback 时序与 RunInfo 字段

### 5.1 Eino 规范中的 Callback 时序

```
节点执行生命周期（以 Invoke 模式为例）:

┌──────────────────────────────────────────────────────┐
│  initGraphCallbacks / initNodeCallbacks              │
│  → 在 context 中设置 RunInfo                         │
├──────────────────────────────────────────────────────┤
│  OnStart(ctx, input)                                 │
│    → 传递 CallbackInput（值）                          │
│    → 返回修改后的 ctx                                 │
├──────────────────────────────────────────────────────┤
│  实际执行 (Invoke)                                    │
├──────────────────────────────────────────────────────┤
│  如果成功: OnEnd(ctx, output)                         │
│    → 传递 CallbackOutput（值）                         │
│  如果失败: OnError(ctx, err)                          │
│    → 传递 error                                       │
└──────────────────────────────────────────────────────┘
```

对于流模式，时序不同：

```
Stream 执行:
  OnStart(ctx, input)          — 值输入
  → Stream(ctx, input)         — 产生 *StreamReader[O]
  → OnEndWithStreamOutput(ctx, *StreamReader[O]) — 流输出副本

Collect 执行:
  OnStartWithStreamInput(ctx, *StreamReader[I]) — 流输入副本
  → Collect(ctx, *StreamReader[I])              — 消费流
  → OnEnd(ctx, output)                          — 值输出

Transform 执行:
  OnStartWithStreamInput(ctx, *StreamReader[I]) — 流输入副本
  → Transform(ctx, *StreamReader[I])            — 产生 *StreamReader[O]
  → OnEndWithStreamOutput(ctx, *StreamReader[O]) — 流输出副本
```

### 5.2 Eino RunInfo 字段

| 字段 | 类型 | 赋值来源 | 图级值 | 组件级值 |
|------|------|---------|--------|---------|
| `Name` | `string` | `compose.WithNodeName` 的图节点名 | 图名（如 "my_graph"） | 节点 key |
| `Type` | `string` | `components.Typer.GetType()` → 回退为反射类型名 | `""` 或图类型 | 实现类型（如 "OpenAI"） |
| `Component` | `string` | `components.Componenter.GetComponentType()` | `"Graph"` / `"Chain"` / `"Workflow"` | `"ChatModel"` / `"Retriever"` / `"Lambda"` 等 |

**关键行为**：
- 未经 `InitCallbacks` 的独立组件：`Name` 为空，`Type` 为反射推导的类型名，`Component` 为组件类别。
- 图级调用（通过 `graphRunnable.Invoke`）：`Component` 固定为图类型字符串。
- `initGraphCallbacks` 在第一次执行前设置 `RunInfo`；`initNodeCallbacks` 在每个节点执行前设置。
- 上下文生命周期：`On[T]` 分发函数（`internal/callbacks/inject.go:74`）通过 `CtxRunInfoKey` 消费并存储 `RunInfo`，防止嵌套调用中的重复分发。

### 5.3 复刻版当前状态

| Eino RunInfo 能力 | 复刻版等价物 | 源码位置 |
|-------------------|-------------|---------|
| `RunInfo.Name` | `Event.NodeKey`（仅 EventLog 中） | `event_log.go:27` |
| `RunInfo.Type` | **无** | — |
| `RunInfo.Component` | `GraphNodeInfo.Component`（仅编译时元数据） | `introspect.go:5` |
| `TimingOnStart` | `EventNodeStart`（被动记录，非拦截式回调） | `event_log.go:51-53` |
| `TimingOnEnd` | `EventNodeEnd` | `event_log.go:55-57` |
| `TimingOnError` | `EventNodeError` | `event_log.go:59-61` |
| 上下文传递 RunInfo | **无** — EventLog 通过 struct 字段显式传递 | — |
| 处理器隔离（各处理器独立 ctx 链） | **无** — 无回调处理器机制 | — |

### 5.4 未来实现时的强制要求

1. `runWithCallbacks` 包装器必须在节点执行前后正确触发 OnStart / OnEnd / OnError。
2. `executorMeta`（或等价结构）必须携带 `component` 和 `componentImplType`，用于构建 `RunInfo`。
3. `isComponentCallbackEnabled` 标志（取反语义）必须防止回调重复触发。
4. 流式回调处理器（`OnStartWithStreamInput` / `OnEndWithStreamOutput`）必须：
   - 接收流副本（由引擎创建 N+1 份）
   - 在处理器中 `defer sr.Close()`
   - 不被 `TimingChecker` 拒绝时仅分配副本
5. 上下文链规则：每个处理器的 `OnStart → OnEnd` 链是独立的，不应假设上下文在不同处理器间传递。

---

## 6. 明确排除的范围

### 6.1 第三章整体排除项

以下 Eino 第三章能力**明确不在当前教育复刻版范围内**：

| 排除项 | 理由 | 替代方案 |
|--------|------|---------|
| **Runnable.Stream / Collect / Transform 方法** | 接口仅有 Invoke；Stream / Collect / Transform 需要完整的 `schema.StreamReader` 基础设施 | `composableRunnable.stream()` 提供简单 fallback（`s` → `i`），供未来扩展预留 |
| **schema.StreamReader / StreamWriter** | 无 schema 包，需要 goroutine channel + linked-list 共享缓冲区的基础设施 | — |
| **流扇出 Copy 机制** | 依赖 StreamReader，无 stream 就没有 copy 需求 | — |
| **流扇入 MergeStreamReaders** | 同上 | Fan-in 通过 `dagChannel.values` map + `mergeValuesFn` 实现值级合并 |
| **StreamReaderWithConvert 类型转换** | 同上 | — |
| **concatStreamReader / RegisterStreamChunkConcatFunc** | 同上 | — |
| **streamFieldMap 流式字段映射** | Stub（`panic("not implemented")`），需要完整的 stream reader | `fieldMap` 值级映射已完整实现 |
| **12 个降级函数（invokeByStream / streamByInvoke / 等）** | 无 Stream / Collect / Transform 原生方法，降级矩阵无基础 | 简单 `stream → invoke` fallback 已实现 |
| **Callback 引擎（5 个阶段 + RunInfo + TimingChecker + HandlerBuilder）** | 需要独立的 `callbacks/` 包、上下文链管理、`executorMeta` 组件桥接 | EventLog（10 种事件类型，线程安全）提供观测等价能力 |
| **`runWithCallbacks` / `invokeWithCallbacks` / 等包装器** | 依赖 Callback 引擎 | — |
| **`initGraphCallbacks` / `initNodeCallbacks`** | 同上 | — |
| **Component Bridge（toChatModelNode / toRetrieverNode / toComponentNode / executorMeta）** | 当前仅有 Lambda 抽象，通过 `AddLambdaNode` + `InvokableLambda` 可完成等价功能 | 通过 `Lambda` + `InvokableLambda` 完成等价功能 |
| **`parseExecutorInfoFromComponent`（components.Typer / callbacks.Checker）** | 同上 | — |
| **`isComponentCallbackEnabled` 回调重复触发防护** | 同上 | — |
| **全局处理器 `AppendGlobalHandlers`** | 依赖 Callback 引擎 | — |
| **路径范围限定回调 `WithCallbacks(WithNodePath(...))`** | 同上 | — |
| **Stream ChainBranch（流式分支）** | 无 Stream，`NewStreamChainBranch` / `NewStreamChainMultiBranch` 未定义 | 值级 `NewChainBranch` / `NewChainMultiBranch` 已完整实现 |
| **`graph.state` 运行时状态** | 字段已定义但未使用 | — |
| **Checkpoint / Recovery** | 不在范围内 | `EventLog` 记录 `EventCheckpoint` 常量但无实现 |
| **可视化 / DOT 导出** | 周边工具 | `GraphInfo` 提供拓扑自省 |

### 6.2 仅限于当前复刻版的事件日志能力

复刻版的 `EventLog` **不是** Eino Callback 引擎的简化版，而是一个独立的简化观测机制：

- `EventLog` 是**被动记录**（记录发生了什么），而非**拦截式回调**（在发生前后插入逻辑）
- EventLog 不修改 context，不修改输入/输出，不决定是否跳过执行
- EventLog 不区分"值处理"和"流处理"（因为无流）
- EventLog 通过 `sync.Mutex` 保证线程安全
- EventLog 的 `Input` / `Output` 字段直接存储 `any`，如需在生产环境使用需注意内存占用

### 6.3 与第一章 / 第二章排除项的交叉

以下排除项在 README 中已记载，与第三章相关：

| 排除项 | 首次定义 | 第三章关联 |
|--------|---------|-----------|
| Stream 执行形态 | README §"运行时不支持" | 第三章核心内容 |
| streamFieldMap 流式映射 | README §"运行时不支持" | 第三章流原语依赖 |
| Stream ChainBranch | README §"运行时不支持" | 第三章流式分支 |
| Callback 机制 (OnStart/OnEnd/OnError) | README §"运行时不支持" | 第三章回调引擎 |
| State 传递 (graph.state) | README §"运行时不支持" | 第三章 Component Bridge 依赖 |
| Checkpoint / Recovery | README §"运行时不支持" | 第三章回调与中断恢复 |
| Component 桥接 | README §"运行时不支持" | 第三层 Component Bridge |

---

## 7. 源码对照表

### 7.1 Eino 第三章关键文件 → 复刻版文件映射

| Eino 文件 | 复刻版文件 | 覆盖度 | 说明 |
|-----------|-----------|--------|------|
| `compose/runnable.go:32-37`（Runnable 接口） | `compose/runnable.go:8-10` | 25%（仅 Invoke） | 接口缺少 Stream/Collect/Transform |
| `compose/runnable.go:336-400`（newRunnablePacker） | — | 0% | 无降级矩阵 |
| `compose/runnable.go:194-334`（12 个降级函数） | — | 0% | — |
| `compose/runnable.go:402`（toGenericRunnable） | — | 0% | — |
| `schema/stream.go:747-778`（toStream） | — | 0% | — |
| `schema/stream.go:792-821`（copyStreamReaders） | — | 0% | — |
| `schema/stream.go:261-275`（Copy） | — | 0% | — |
| `compose/stream_reader.go:26`（streamReader 接口） | — | 0% | — |
| `compose/stream_concat.go:50-88`（concatStreamReader） | — | 0% | — |
| `compose/stream_concat.go:44`（RegisterStreamChunkConcatFunc） | — | 0% | — |
| `compose/field_mapping.go`（streamFieldMap） | `compose/field_mapping.go:450-454` | Stub | `panic("not implemented")` |
| `callbacks/interface.go:41`（RunInfo） | — | 0% | `GraphNodeInfo` 仅 Name + Component |
| `callbacks/interface.go:114-134`（5 个阶段） | — | 0% | EventLog 有 3 个等价事件类型 |
| `callbacks/interface.go:136-145`（TimingChecker） | — | 0% | — |
| `callbacks/handler_builder.go`（HandlerBuilder） | — | 0% | — |
| `internal/callbacks/inject.go:74`（On 分发） | — | 0% | — |
| `internal/callbacks/inject.go:163-193`（流处理器） | — | 0% | — |
| `compose/utils.go:100`（runWithCallbacks） | — | 0% | — |
| `compose/component_to_graph_node.go:29`（toComponentNode） | — | 0% | — |
| `compose/component_to_graph_node.go:93`（toChatModelNode） | — | 0% | — |
| `compose/component_to_graph_node.go:151-163`（parseExecutorInfoFromComponent） | — | 0% | — |
| `compose/graph_run.go`（runner） | `compose/graph_run.go` | 50%（无 callback 集成） | 主循环已实现，无 `runWithCallbacks` 包装 |
| `compose/event_log.go`（EventLog） | `compose/event_log.go` | 100%（独立实现） | 复刻版独有，非 Eino 等价物 |

### 7.2 复刻版 `composableRunnable` 与 Eino `composableRunnable` 的差异

| 维度 | Eino | 复刻版 |
|------|------|--------|
| 内部函数 | `i invoke` + `t transform`（Stream/Collect 从这俩派生） | `i invoke` + `s stream`（仅简单 fallback） |
| 元数据 | `inputType / outputType / optionType / isPassthrough / meta / nodeInfo` | 无 |
| 公开方法 | `Invoke / Stream / Collect / Transform` | `invoke / stream`（小写，仅内部使用） |
| Stream 函数 | 产生 `*schema.StreamReader[O]` | 产生 `any`（裸值） |
| 类型擦除 | 泛型 `runnablePacker[T, TOption]` 包装 | `any` 类型断言（`runnable.go:51-56`） |
| 由谁调用 | 图运行时的 `taskManager.submit` 根据执行模式选择调用 invoke/stream/collect/transform | `taskManager.submit` 始终调用 `invoke` |

---

## 附录 A：当复刻版需要引入第三章能力时的优先路径

如果在后续版本中需要扩展第三章能力，建议按以下顺序实施：

### Phase 1：基础流原语（最小可行）
1. 引入 `schema.StreamReader[T]` + `StreamWriter[T]`（goroutine channel 实现）
2. 实现 `Copy`（扇出）和 `MergeStreamReaders`（扇入）
3. 实现 `concatStreamReader`（流折叠为单值）

### Phase 2：Runnable 降级矩阵
1. 扩展 `Runnable[I,O]` 接口，添加 `Stream / Collect / Transform` 方法
2. 实现 `runnablePacker` 和 12 个降级函数
3. 修改 `taskManager.submit` 以根据执行模式路由到正确的 runnable 方法

### Phase 3：Callback 引擎
1. 引入 `callbacks/` 包：`Handler` / `RunInfo` / `TimingChecker` / `HandlerBuilder`
2. 实现 `runWithCallbacks` 及四种模式包装器
3. 在 `graph_run.go` 中集成 `initGraphCallbacks` / `initNodeCallbacks`

### Phase 4：Component Bridge
1. 引入 `executorMeta` 和 `toComponentNode`
2. 实现 `parseExecutorInfoFromComponent` 与 `isComponentCallbackEnabled` 标志
3. 注册 `toChatModelNode` / `toRetrieverNode` 等组件桥接函数

---

## 附录 B：关键哨兵错误与常量

| 常量 | 复刻版位置 | 值 | 说明 |
|------|-----------|----|------|
| `ErrGraphCompiled` | `types.go:39` | `"graph already compiled, cannot be modified"` | 编译锁 |
| `ErrExceedMaxSteps` | `types.go:41` | `"exceeded maximum run steps"` | Pregel 步数上限 |
| `ErrDAGHasCycle` | `types.go:42` | `"DAG graph has a cycle, cannot compile in AllPredecessor mode"` | Kahn 算法检测到环 |
| `ErrNoStartEdge` | `types.go:43` | `"no edge from START"` | 无起始边 |
| `ErrNoEndEdge` | `types.go:44` | `"no edge to END"` | 无终止边 |
| `ErrNodeNotFound` | `types.go:45` | `"node not found"` | 节点不存在 |
| `ErrNoCompiledRunnable` | `types.go:46` | `"node has no compiled runnable"` | 节点无编译后的 runnable |
| `errMapKeyNotFound` | `field_mapping.go:109-115` | `"key=%s"` | 字段映射 map key 不存在 |
| `errInterfaceNotValidForFieldMapping` | `field_mapping.go:119-126` | 结构化 | interface 类型不是 struct/map |
| `fieldPathSeparator` | `field_mapping.go:10` | `\x1F` | 字段路径分隔符（与 Eino 一致） |
| `START` / `END` | `types.go:30-31` | `"start"` / `"end"` | 虚拟端节点常量 |
| `defaultMaxSteps` | `types.go:35` | `100` | Pregel 默认最大步数 |
| `ComponentOfGraph/Lambda/Workflow/Chain/Unknown` | `types.go:21-27` | 字符串常量 | 组件类型枚举（与 Eino 基本一致） |

---

## 附录 C：关键类型签名速查

### Eino（完整接口）

```go
// compose/runnable.go:32-37
type Runnable[I, O any] interface {
    Invoke(ctx context.Context, input I, opts ...Option) (output O, err error)
    Stream(ctx context.Context, input I, opts ...Option) (output *schema.StreamReader[O], err error)
    Collect(ctx context.Context, input *schema.StreamReader[I], opts ...Option) (output O, err error)
    Transform(ctx context.Context, input *schema.StreamReader[I], opts ...Option) (output *schema.StreamReader[O], err error)
}

// callbacks/interface.go:41
type RunInfo struct {
    Name      string
    Type      string
    Component Component
}

// callbacks/interface.go:114-134
const (
    TimingOnStart
    TimingOnEnd
    TimingOnError
    TimingOnStartWithStreamInput
    TimingOnEndWithStreamOutput
)
```

### 复刻版（当前接口）

```go
// compose/runnable.go:8-10
type Runnable[I, O any] interface {
    Invoke(ctx context.Context, input I) (output O, err error)
}

// compose/runnable.go:12-15
type composableRunnable struct {
    i func(ctx context.Context, input any) (output any, err error)
    s func(ctx context.Context, input any) (output any, err error)
}

// compose/event_log.go:24-33（非 Eino 等价物）
type Event struct {
    Type      EventType
    Timestamp time.Time
    NodeKey   string
    GraphName string
    Step      int
    Input     any
    Output    any
    Error     string
}
```
