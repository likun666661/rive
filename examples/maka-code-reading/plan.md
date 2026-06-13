# Maka Runtime Agent Layer 计划书

> 阅读基线：Maka `335220a`
>
> 关联材料：
> - 粗读总览：[`manual/00-overview.md`](./manual/00-overview.md)
> - 精读总报告：[`manual/deep-read/00-final-deep-read-guide.md`](./manual/deep-read/00-final-deep-read-guide.md)
>
> 核心判断：借鉴 Pie Agent 的 agent kernel 思路，但不做一次性大重构。Maka 现在可以跑，产品入口也已经成形；正确路线是保留现有 `SessionManager` / IPC / UI 行为，沿着 runtime 最危险的边界一层层抽象。

## 0. 目标

Maka 的 `packages/runtime` 已经具备本地桌面 coding agent 的核心形态：

- `SessionManager` 管 session / turn 生命周期。
- `AiSdkBackend` 接 Vercel AI SDK/provider，处理模型 stream、tool call、watchdog、abort、usage。
- `PermissionEngine` + `preToolUse` 在工具执行前做 allow / prompt / block。
- builtin tools 把模型输出连接到 shell/file/search/explore/office/rive 等真实能力。
- telemetry/pricing/bot bridge 在 runtime 周边，决定成本、可观测性和多入口形态。

但从 Pie Agent 的角度看，Maka 现在还不是一个清晰的 agent kernel。它的关键职责被粘在 `AiSdkBackend` 和 `wrapToolExecute` 里：provider adapter、agent loop、tool runtime、permission policy、abort/watchdog、telemetry 都有交叉。

计划目标不是“重写一个新 runtime”，而是逐步把这些边界切出来：

```text
SessionManager
  -> AgentController
      -> ModelAdapter
      -> ToolRuntime
          -> RuntimePolicy / PermissionEngine
          -> Builtin Tools
      -> RunTrace / Telemetry
      -> WorkflowBridge(Rive)
```

这是一条方向线，不是第一阶段的改动范围。

## 1. 基本原则

### 1.1 不做 Big-Bang Rewrite

第一批改动不应该影响：

- `window.maka.*` preload API
- main/renderer IPC channel 名称
- `SessionManager.sendMessage` 外部调用形态
- session JSONL 主格式
- 用户可见 permission mode
- builtin tool 名称和基本输入输出

换句话说，Maka 的 UI、Bot、Gateway 继续按现在方式调用 runtime。

### 1.2 先修真实 runtime bug，再抽漂亮抽象

精读确认的 P0 不是风格问题：

- `StreamWatchdog` pause/resume 在并行 tool call 下可能误恢复。
- permission parked 没 timeout，可能永久等待。
- JSONL 一行坏掉可能让整个 session 不可读。
- telemetry 没提取 cache/reasoning tokens，成本统计会系统性偏低。

这些应优先于纯架构抽象。

### 1.3 每一层都要有验收标准

每个阶段都必须回答：

- 改了什么边界？
- 没改什么边界？
- 如何证明旧行为没坏？
- 如何证明新 bug 被修掉？
- 是否给下一层抽象留下了测试基础？

## 2. Pie Agent 视角下的目标抽象

### 2.1 AgentController

负责一轮 agent 执行：

- 读取 session/run state
- 调用模型
- 接收模型文本或 tool call
- 执行工具
- 把 observation 写回上下文
- 判断继续、完成、失败或中止

Maka 当前的 controller 职责散在 `SessionManager` 和 `AiSdkBackend` 中。

### 2.2 ModelAdapter

负责 provider 差异：

- AI SDK stream 格式
- model message/tool schema 转换
- usage 字段归一化
- provider error 分类

它不应该知道 permission prompt、workspace path containment 或具体工具实现。

### 2.3 ToolRuntime

负责工具生命周期：

```text
validate input
  -> classify permission
  -> apply runtime policy
  -> execute with context + abort
  -> emit observation / artifact / telemetry / trace
```

这是 Maka 最应该先抽的边界，因为 `wrapToolExecute` 是模型输出进入本机能力的闸门。

### 2.4 RuntimePolicy

集中管理 guardrail：

- permission mode
- tool category
- workspace boundary
- timeout
- token/cost budget
- tool allowlist
- bot/gateway 限制

现在这些策略有一部分在 `preToolUse`，一部分在工具内部，一部分在 main/bot/gateway 调用链里。

### 2.5 RunTrace

不是先做 checkpoint/replay，而是先把 runtime 发生过什么记录清楚：

- model call started / finished / failed
- tool call started / finished / failed
- permission decision
- permission parked / timeout
- abort reason
- usage/cost
- artifact refs

这个方向来自 Rive 和 mini-swe-agent dogfood 的经验：没有 live trace，就很难值班、恢复和解释失败。

### 2.6 WorkflowBridge

Maka 已有 Rive tool bridge。短期它还是一个 tool；长期可以作为升级路径：

```text
单 agent run 不够
  -> 构建 Rive workflow/template/run
  -> 监控 scheduler/work DAG/artifact
  -> 把结果回灌 Maka session
```

## 3. 阶段路线

## Phase 1：Runtime P0 Hardening

目标：不动产品架构，先把 runtime 最危险的 bug 修掉。

### 1.1 `StreamWatchdog` 引用计数

问题：

当前 watchdog pause/resume 语义接近布尔 flag。Vercel AI SDK 支持同一 step 内并行 tool calls，Tool A 恢复 watchdog 时 Tool B 可能仍在等 permission，导致 B 被 idle timeout 误伤。

改动：

- 把 paused boolean 改成 `pauseCount`。
- `pause()` 递增，`resume()` 递减。
- 只有 `pauseCount === 0` 时真正恢复。
- 防止负数 resume。
- mismatch 时写 debug/trace event。

验收：

- 并行两个 tool call 的测试：
  - 两个都 pause。
  - 第一个完成 resume 后 watchdog 仍暂停。
  - 第二个完成 resume 后 watchdog 才恢复。
- 单 tool call 行为不变。

### 1.2 Permission parked timeout

问题：

`wrapToolExecute` 等 `verdict.parked` 没超时。UI 崩、用户离开、bot 误进 ask mode 都会导致 session 永久卡住。

改动：

- 增加 permission timeout，默认 300 秒。
- parked promise 外层加 `Promise.race`。
- timeout 后：
  - 写 permission timeout event。
  - 自动 deny tool call。
  - 返回结构化 observation。
  - 保证 watchdog pause 被恢复/递减。

验收：

- `prompt -> parked -> timeout -> deny` 测试。
- timeout 后 watchdog 状态正确。
- bot session 不会永久 parked。

### 1.3 Tool abort 一致性

问题：

部分工具接受 `abortSignal`，部分外部 subprocess 路径没有真正响应 abort，例如 OfficeDocument 的 `execFile` 路径。

改动：

- 审计 builtin tools 是否使用 `MakaToolContext.abortSignal`。
- 对 subprocess 工具补 abort 支持。
- OfficeDocument 如确认缺口，改成 `spawn` + 手动收集 stdout/stderr + abort kill。

验收：

- turn cancel 后不会留下长时间运行的 child process。
- observation 能区分 user abort 和 tool/process failure。

### 1.4 Telemetry token 完整性

问题：

pricing 逻辑有 cache/reasoning 字段，但 `AiSdkBackend` 没完整提取 AI SDK usage，导致成本低估。

改动：

- 归一化 usage：
  - input
  - output
  - cache read
  - cache write
  - reasoning
  - total
- provider-specific 字段可暂存 metadata。
- 如需单独 reasoning 价格，扩展 `PricingConfig`。

验收：

- synthetic usage 测试覆盖全部 token 维度。
- usage UI/report 不再在 cache/reasoning 存在时显示系统性低估。

### Phase 1 不做什么

- 不引入公开 `AgentRun`。
- 不替换 `SessionManager`。
- 不重写所有工具。
- 不替换 Vercel AI SDK。
- 不改 UI 结构。

## Phase 2：围绕 `wrapToolExecute` 抽 ToolRuntime

目标：把工具生命周期从 `AiSdkBackend` 中切出来，但保留现有工具实现。

### 2.1 定义内部 ToolRuntime Contract

建议先做内部接口：

```ts
interface ToolRuntime {
  execute(request: ToolRuntimeRequest): Promise<ToolRuntimeResult>;
}

interface ToolRuntimeRequest {
  sessionId: string;
  turnId: string;
  toolName: string;
  input: unknown;
  permissionMode: PermissionMode;
  context: MakaToolContext;
}

interface ToolRuntimeResult {
  status: 'ok' | 'denied' | 'blocked' | 'aborted' | 'failed';
  observation: unknown;
  permission?: PermissionDecisionSummary;
  artifactRefs?: ArtifactRef[];
  telemetry?: ToolTelemetry;
}
```

先不要通过 IPC 暴露。

### 2.2 从 `AiSdkBackend` 搬走工具生命周期逻辑

迁移到 ToolRuntime 的职责：

- tool input validation wrapper
- permission classification
- parked approval wait/timeout
- watchdog pause/resume
- abort integration
- tool telemetry
- structured failure classification

`AiSdkBackend` 只保留 provider stream 和把 tool result 回传模型的逻辑。

### 2.3 统一高风险工具 observation

不必一次性改所有工具。先包一层 envelope：

```ts
{
  ok: boolean,
  tool: string,
  summary: string,
  data?: unknown,
  artifactRefs?: string[],
  error?: {
    kind: string,
    message: string,
    retryable?: boolean
  }
}
```

优先覆盖：

- shell
- file/write
- explore
- office
- rive

验收：

- `AiSdkBackend` 中不再塞满 permission/tool lifecycle。
- allow/block/prompt/timeout/abort/tool error 都能不依赖完整 AI SDK stream fixture 单测。
- 现有工具行为保持。

### Phase 2 不做什么

- 不做 checkpoint。
- 不做 UI timeline。
- 不迁移全部工具输出格式。

## Phase 3：抽最小 ModelAdapter

目标：把 provider 差异和 agent loop 差异切开。

### 3.1 定义 Maka 内部 ModelStreamEvent

```ts
type ModelStreamEvent =
  | { type: 'text_delta'; text: string }
  | { type: 'tool_call'; call: ModelToolCall }
  | { type: 'usage'; usage: NormalizedUsage }
  | { type: 'finish'; reason: string }
  | { type: 'error'; error: NormalizedModelError };
```

AI SDK 只是一个 adapter，输出 Maka 内部事件。

### 3.2 保留 `AiSdkBackend` 兼容壳

先拆内部，不大范围改名：

- `AiSdkModelAdapter`：provider call + stream normalize。
- `AiSdkBackend`：兼容当前 backend，负责 adapter + ToolRuntime + session stream glue。

### 3.3 provider error 归一化

错误种类建议固定为：

- auth
- network
- rate_limit
- model_not_found
- context_length
- invalid_tool_schema
- provider_unknown

验收：

- AI SDK 当前路径不回退。
- synthetic provider errors 能稳定分类。
- ToolRuntime 测试不依赖 provider-specific event shape。

### Phase 3 不做什么

- 不要求立刻接第二个真实 provider。
- 不接 Codex/OpenCode backend。
- 不改 settings UI。

## Phase 4：RunTrace / Runtime Telemetry

目标：让 runtime 能解释自己发生了什么。先做 trace，不做完整 replay。

### 4.1 事件类型

最小事件集：

- `run.started`
- `model.call.started`
- `model.call.finished`
- `model.call.failed`
- `tool.call.started`
- `permission.decision`
- `permission.parked`
- `permission.timeout`
- `tool.call.finished`
- `tool.call.failed`
- `run.aborted`
- `run.finished`

### 4.2 存储策略

可选方案：

1. session JSONL 旁边加 runtime trace JSONL。
2. 扩展 telemetry repo。
3. 新增一个小型 trace repo。

建议：如果 Phase 1/后续已经在修 JSONL durability，就倾向新增小型 trace repo，避免把 session 消息和 runtime 诊断继续混在一起。

### 4.3 先做开发者可见，不做复杂 UI

第一版只需要：

- export trace by session/turn。
- debug panel 展示最近 model/tool/permission event。
- session error 里包含 failure kind。

验收：

- stuck permission 能看到 parked scope 和等待时间。
- provider failure 能看到 normalized kind。
- tool abort 能看到 abort reason。
- usage UI 继续工作。

### Phase 4 不做什么

- 不做 replay。
- 不做 checkpoint resume。
- 不做跨进程 scheduler。

## Phase 5：是否引入 AgentRun

目标：到这里再判断是否值得引入 `AgentRun` 一等对象。

### 触发条件

至少出现两个条件再做：

- UI 需要一个 session 内并发多个 run。
- Bot/Gateway run 需要独立 retry/resume。
- Rive workflow node 需要映射 Maka runtime run。
- checkpoint/resume 成为产品核心。
- 第二个真实 model backend 无法放进现有 backend 结构。

### 可能形态

```ts
interface AgentRun {
  runId: string;
  sessionId: string;
  turnId: string;
  state: 'created' | 'running' | 'waiting_permission' | 'completed' | 'failed' | 'aborted';
  controller: 'ai-sdk' | 'future-react' | 'rive-workflow';
  policy: RuntimePolicy;
  traceRef: string;
}
```

### 迁移策略

- 先内部生成，不暴露给 renderer。
- 从现有 session turn state 派生。
- UI 真有收益时再接。

### Phase 5 不做什么

- 不强迁历史 session。
- 不重做整个 storage。
- 不把简单 chat session 强行变成 workflow。

## 4. 第一轮实现 DAG

如果下一步要开 Rive workflow，建议第一轮只做 Phase 1：

```text
root: Maka runtime P0 hardening

parallel:
  A. watchdog-refcount
  B. permission-timeout
  C. telemetry-token-extraction
  D. abort-audit-and-officecli

integration:
  E. wrap-tool-execute-integration-review
       depends_on: A, B, C, D

tests:
  T1. permission-watchdog-concurrency-tests
       depends_on: A, B
  T2. telemetry-token-tests
       depends_on: C
  T3. abort-tool-tests
       depends_on: D

final:
  F. maintainer-review-and-changelog
       depends_on: E, T1, T2, T3
```

### 节点 A：watchdog-refcount

产出：

- `StreamWatchdog` reference-counted pause/resume。
- nested/concurrent pause 测试。

### 节点 B：permission-timeout

产出：

- parked permission timeout。
- timeout 后 deny observation。
- cleanup/watchdog 测试。

### 节点 C：telemetry-token-extraction

产出：

- AI SDK usage 全 token 维度归一化。
- pricing/cost 测试。

### 节点 D：abort-audit-and-officecli

产出：

- builtin tools abort 审计。
- OfficeDocument subprocess abort 修复，如确认缺口。

### 节点 E：wrap-tool-execute-integration-review

产出：

- 检查 A/B/C/D 在当前 `wrapToolExecute` 中组合正确。
- 暂不抽完整 ToolRuntime。

### Final Reviewer

产出：

- 确认没有改变 renderer/session public API。
- 写 changelog / review note。
- 跑全量测试。

## 5. 第二轮 DAG：ToolRuntime Extraction

Phase 1 通过后再开：

```text
root: Maka ToolRuntime extraction

parallel:
  A. tool-runtime-contract-design
  B. shell-file-tool-lifecycle-map
  C. explore-office-rive-tool-lifecycle-map

implementation:
  D. introduce-tool-runtime-wrapper
       depends_on: A
  E. move-permission-watchdog-telemetry
       depends_on: D, B, C

tests:
  T1. allow-block-prompt-tests
       depends_on: E
  T2. abort-and-failure-tests
       depends_on: E

final:
  F. review-api-diff-and-risk
       depends_on: T1, T2
```

## 6. 风险控制

### Public API Freeze

Phase 1-4 不改：

- preload API
- IPC channel
- settings shape
- permission mode 名称
- builtin tool 名称

### Feature Flag

不确定的行为可以临时加内部开关：

- `MAKA_RUNTIME_TOOL_TIMEOUT_MS`
- `MAKA_RUNTIME_TRACE_ENABLED`
- `MAKA_RUNTIME_TOOL_RUNTIME_V2`

开关必须有清理计划。

### Tests Before Refactor

在移动 `AiSdkBackend` 逻辑前，先补 characterization tests：

- text-only turn
- allowed tool call
- blocked tool call
- prompted/parked tool call
- aborted tool call
- provider error
- usage accounting

## 7. 明确不做

- 不创建新的 agent DSL。
- 不替换 Vercel AI SDK。
- 不一次性迁移所有 tool output。
- 不把 Rive 变成 Maka 唯一执行路径。
- 不在 JSONL durability 修好前重做 storage。
- 不在 UI 需求明确前暴露 `AgentRun`。

## 8. 成功标准

Phase 1 完成后：

- permission 不会永久 parked。
- 并行 tool call 不会误恢复 watchdog。
- cache/reasoning token 会进入成本统计。
- subprocess tool 能响应 abort。
- 现有 UI/session 行为不变。

Phase 2 完成后：

- `wrapToolExecute` 变薄。
- permission/abort/telemetry/tool failure 行为集中在 ToolRuntime。
- 高风险工具有一致 observation/failure kind。

Phase 3 完成后：

- AI SDK stream shape 被 ModelAdapter 隔离。
- provider error 分类稳定。
- runtime tests 可以使用 normalized model events。

Phase 4 完成后：

- runtime failure 能从结构化 trace 解释。
- permission/model/tool lifecycle 可查询。
- usage UI 仍兼容。

Phase 5 完成后：

- 可以基于真实需求判断 `AgentRun` 是否值得迁移成本。

## 9. 推荐第一步

下一步只做 Phase 1，不碰大抽象：

1. 为 watchdog 并发 pause/resume 写测试。
2. 改成 reference-counted watchdog。
3. 为 permission parked timeout 写测试。
4. 在 `wrapToolExecute` 加 timeout 和 cleanup。
5. 为 usage 全 token 维度写测试。
6. 补 telemetry extraction。
7. 审计并修 confirmed subprocess abort gap。

这一步收益最大：不改产品结构，但会让 Maka runtime 立刻更可靠，并为后续 ToolRuntime extraction 建好测试地基。
