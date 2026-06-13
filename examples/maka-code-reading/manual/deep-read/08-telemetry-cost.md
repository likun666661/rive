# Telemetry / Cost Completeness 精读报告

阅读基线: `335220a`
深度档位: `maintainer`

---

## scope

本报告覆盖 Maka 的遥测（telemetry）与成本（cost）完整链路，范围包括：

- **记录层**: LLM 调用记录 (`recordLlmCall`)、工具调用记录 (`recordToolInvocation`)
- **定价层**: 内置定价表 (`builtin-pricing.ts`)、用户定价覆盖 (`pricing.ts`)、成本计算 (`cost.ts`)
- **存储层**: `FileTelemetryRepo` — 基于 JSON 文件的遥测持久化 (`telemetry-repo.ts`)
- **IPC/渲染层**: `main.ts` 中的 `usage:*` IPC handlers、Usage Dashboard UI 合约
- **注入点**: `AiSdkBackend` 中 telemetry hook 的触发位置 (`ai-sdk-backend.ts`)
- **类型系统**: `LlmCallRecord`、`ToolInvocationRecord`、`UsageSummaryV2`、`UsageBucket`、`PricingConfig` (`usage-stats/types.ts`)

---

## problem

Maka 的遥测系统需要准确回答以下问题：

1. **每次 LLM 调用花费多少？** 需要 input/output/cache/reasoning token 计数 × 价格。
2. **每次工具调用消耗多少？** 需要持续时间和 bytes in/out。
3. **成本模型覆盖了哪些 token 维度？** reasoning tokens、cache read/write 是否进入计费。
4. **数据在什么条件下会丢失？** process exit、fire-and-forget、未定价模型。
5. **UI 如何展示统计？** summary、buckets、logs、daily review 的数据一致性。

当前实现存在若干 **loss windows（丢失窗口）**，下文逐一列举。

---

## source_evidence

### 受检源码清单

| 文件 | 行数 | 角色 |
|------|------|------|
| `packages/runtime/src/telemetry/record-llm-call.ts` | 44 | LLM 调用记录入口 |
| `packages/runtime/src/telemetry/record-tool-invocation.ts` | 33 | 工具调用记录入口 |
| `packages/runtime/src/telemetry/cost.ts` | 37 | 成本计算核心 |
| `packages/runtime/src/telemetry/pricing.ts` | 7 | 定价查找组合器 |
| `packages/runtime/src/telemetry/builtin-pricing.ts` | 26 | 内置定价表（12 个模型） |
| `packages/runtime/src/telemetry/types.ts` | 26 | Telemetry 类型定义 |
| `packages/runtime/src/telemetry/index.ts` | 8 | 模块导出 |
| `packages/storage/src/telemetry-repo.ts` | 305 | 文件持久化 + 查询 |
| `packages/core/src/usage-stats/types.ts` | 112 | 核心类型定义 |
| `packages/core/src/usage-stats/pricing.ts` | 167 | 定价规范化 + 校验 |
| `packages/runtime/src/ai-sdk-backend.ts` | 1181 | LLM 调用 / 工具调用发生时注入 record hook |
| `apps/desktop/src/main/main.ts:665-697` | — | AiSdkBackend 构造 + telemetry hooks 绑定 |
| `apps/desktop/src/main/main.ts:2581-2698` | — | usage IPC handlers |

### 关键类型

**LlmCallRecord** (`packages/core/src/usage-stats/types.ts:80-96`):
```typescript
interface LlmCallRecord {
  sessionId?: string;
  turnId?: string;
  connectionSlug?: string;
  providerId: string;
  modelId: string;
  inputTokens: number;          // 必填
  outputTokens: number;         // 必填
  cachedInputTokens?: number;   // 可选 — 缓存读取 tokens
  cacheWriteInputTokens?: number; // 可选 — 缓存写入 tokens
  reasoningTokens?: number;     // 可选 — 推理 tokens
  totalTokens?: number;
  latencyMs: number;
  status: 'success' | 'error' | 'aborted';
  errorClass?: string;
  startedAt: number;
}
```

**PricingConfig** (`packages/core/src/usage-stats/types.ts:72-78`):
```typescript
interface PricingConfig {
  modelKey: string;              // e.g. "anthropic:claude-sonnet-4-5"
  inputUsdPer1M: number;         // 必填
  outputUsdPer1M: number;        // 必填
  cacheReadUsdPer1M?: number;    // 可选
  cacheWriteUsdPer1M?: number;   // 可选
  // 注意：没有 reasoningUsdPer1M 字段
}
```

---

## accounting_flow

### 流程一：LLM Call Usage → Pricing → Storage → UI/Report

```
┌─────────────────────────────────────────────────────────────────┐
│ 1. AiSdkBackend.send() 完成 streamText                          │
│    packages/runtime/src/ai-sdk-backend.ts:504-517               │
│                                                                 │
│    this.input.recordLlmCall?.({                                 │
│      inputTokens: tokenUsage?.promptTokens ?? 0,                │
│      outputTokens: tokenUsage?.completionTokens ?? 0,           │
│      totalTokens: tokenUsage?.totalTokens,                      │
│      latencyMs, status, errorClass, startedAt, ...              │
│      // NOTE: cachedInputTokens/cacheWriteInputTokens/          │
│      //       reasoningTokens NOT passed here                   │
│    });                                                          │
│                                                                 │
│ 2. main.ts 绑定 hook                                            │
│    apps/desktop/src/main/main.ts:681                            │
│                                                                 │
│    recordLlmCall: (event) =>                                    │
│      recordLlmCall({ repo: telemetryRepo, lookupPricing },      │
│      event),                                                    │
│                                                                 │
│ 3. recordLlmCall() 处理                                        │
│    packages/runtime/src/telemetry/record-llm-call.ts:12-43      │
│                                                                 │
│    queueMicrotask(() => {                                       │
│      // 默认值填充（传入值可能为 undefined）                    │
│      cachedInputTokens = record.cachedInputTokens ?? 0          │
│      cacheWriteInputTokens = record.cacheWriteInputTokens ?? 0  │
│      reasoningTokens = record.reasoningTokens ?? 0              │
│                                                                 │
│      // 计算成本                                                │
│      costUsd = computeCost(usage, pricing).totalCost            │
│                                                                 │
│      // 写入 Repo                                              │
│      repo.insertLlmCall({ ...record, id, costUsd, ... })        │
│    });                                                          │
│                                                                 │
│ 4. computeCost() 计算成本                                      │
│    packages/runtime/src/telemetry/cost.ts:18-37                 │
│                                                                 │
│    inputCost = (inputTokens / 1M) * pricing.inputUsdPer1M      │
│    outputCost = (outputTokens / 1M) * pricing.outputUsdPer1M   │
│    cacheReadCost = pricing.cacheReadUsdPer1M ?                  │
│      (cachedInputTokens / 1M) * pricing.cacheReadUsdPer1M : 0  │
│    cacheWriteCost = pricing.cacheWriteUsdPer1M ?                │
│      (cacheWriteInputTokens / 1M) * pricing.cacheWriteUsdPer1M │
│      : 0                                                        │
│    totalCost = input + output + cacheRead + cacheWrite          │
│    // NOTE: reasoning tokens are NOT factored into cost         │
│                                                                 │
│ 5. FileTelemetryRepo 持久化                                     │
│    packages/storage/src/telemetry-repo.ts:79-81                 │
│                                                                 │
│    insertLlmCall() → upsertById → enqueueWrite() →              │
│    writeFile(temp) → rename temp → telemetry.json               │
│                                                                 │
│ 6. UI 读取 IPC                                                  │
│    apps/desktop/src/main/main.ts:2584-2660                      │
│                                                                 │
│    usage:summary → repo.summary(query)                          │
│    usage:buckets → repo.buckets(query, groupBy)                 │
│    usage:logs   → repo.logs(query, offset, limit)               │
│    daily-review:day → bundle summary + tool/model buckets       │
│                                                                 │
│ 7. 渲染层展示                                                   │
│    SettingsModal.tsx → UsageSettingsPage → UsageTable           │
│    展示：总请求数 / 总费用 / Token分布 / 延迟 / 错误率          │
└─────────────────────────────────────────────────────────────────┘
```

**关键数据流缺口**：ai-sdk `result.usage` 对象可能包含 `cachedInputTokens`、`cacheWriteInputTokens`、`reasoningTokens` 等字段（取决于 provider），但 `AiSdkBackend.send()` 只提取了 `promptTokens`、`completionTokens`、`totalTokens`。`cachedInputTokens`、`cacheWriteInputTokens`、`reasoningTokens` 从未从 ai-sdk usage 中读取。

### 流程二：Tool Invocation → Storage → UI/Report

```
┌─────────────────────────────────────────────────────────────────┐
│ 1. AiSdkBackend.wrapToolExecute()                               │
│    packages/runtime/src/ai-sdk-backend.ts:692-705               │
│                                                                 │
│    this.input.recordToolInvocation?.({                          │
│      sessionId, turnId, toolCallId, toolName,                   │
│      providerId, modelId, durationMs, status,                   │
│      argsSummary, bytesIn, bytesOut, startedAt,                 │
│    });                                                          │
│                                                                 │
│ 2. main.ts 绑定 hook                                            │
│    apps/desktop/src/main/main.ts:682-692                        │
│                                                                 │
│    recordToolInvocation: (event) =>                             │
│      recordToolInvocation(                                      │
│        { repo: telemetryRepo },                                 │
│        event.toolName === WEB_SEARCH_TOOL_NAME                   │
│          ? { ...event, argsSummary: undefined }  ← 清洗搜索词   │
│          : event,                                               │
│      ),                                                         │
│                                                                 │
│ 3. recordToolInvocation()                                       │
│    packages/runtime/src/telemetry/record-tool-invocation.ts     │
│                                                                 │
│    queueMicrotask(() => {                                       │
│      repo.insertToolInvocation({ ...record, id, ... })          │
│    });                                                          │
│                                                                 │
│ 4. FileTelemetryRepo 持久化                                     │
│    packages/storage/src/telemetry-repo.ts:84-86                 │
│                                                                 │
│    insertToolInvocation() → upsertById → enqueueWrite()         │
│                                                                 │
│ 5. 工具调用无成本计算                                           │
│    toolBuckets() 中 costUsd 恒为 0                              │
│    telemetry-repo.ts:295                                        │
│    // 工具调用不计入费用                                        │
│                                                                 │
│ 6. UI: usage:buckets query.groupBy === 'tool' → toolBuckets     │
│    展示：调用次数 / bytes in/out / 平均延迟 / 错误率            │
│    备注：tool bucket 复用 UsageBucket 类型，inputTokens/output   │
│          Tokens 字段实际存储的是 bytesIn/bytesOut                 │
│          (telemetry-repo.ts:289-290)                             │
└─────────────────────────────────────────────────────────────────┘
```

---

## loss_windows

### 1. fire-and-forget 写入

**位置**：`record-llm-call.ts:13`、`record-tool-invocation.ts:13`

两个 recorder 都使用 `queueMicrotask()` 而非同步写入。`insertLlmCall` 和 `insertToolInvocation` 返回 `void`，内部调用 `void this.enqueueWrite()` — 不 await 写入完成。

**影响**：如果在 microtask 执行前发生未捕获异常或进程被 SIGKILL，该次记录永久丢失。

### 2. process exit 时未刷盘

**位置**：`telemetry-repo.ts:202-213`

`enqueueWrite()` 使用 promise chain (`this.queue = this.queue.then(...)`)，但进程退出时（`app.on('before-quit')` / `window-all-closed`）没有任何显式 `await telemetryRepo.flush()` 调用。

**现有证据** (`main.ts:3431-3439`)：
```typescript
app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});
app.on('before-quit', () => {
  for (const id of Array.from(planReminderTimers.keys())) clearPlanReminderTimer(id);
  void botRegistry.stopAll();
  void openGateway.stop();
});
// 没有 telemetryRepo flush
```

**影响**：最后一个 turn 的 LLM 调用和工具调用的遥测记录可能未写入磁盘。

### 3. 未定价模型（unpriced models）

**位置**：`cost.ts:19-20`、`builtin-pricing.ts:5-20`

当前内置定价表覆盖 12 个模型（Anthropic 3, OpenAI 3, Google 2, DeepSeek 2, Moonshot 1, zai-coding-plan 3）。如果用户使用不在表中的模型（例如通过自定义 OpenAI-compatible endpoint 接入的模型），`lookupPricing` 返回 `null`，`computeCost` 返回全 0 成本。

**代码证据** (`cost.ts:18-21`)：
```typescript
if (!pricing) {
  return { inputCost: 0, outputCost: 0, cacheReadCost: 0, cacheWriteCost: 0, totalCost: 0 };
}
```

用户可通过 `usage:pricing:put` 添加覆盖定价，但需要手动配置。

### 4. reasoning tokens 未进入成本模型

**数据采集侧** (`ai-sdk-backend.ts:504-517`)：

`AiSdkBackend.send()` 的 `finally` 块中构造 `LlmCallRecord` 时，**未传递** `reasoningTokens` 字段。ai-sdk 的 `result.usage` 可能包含 reasoning token 信息（取决于 provider），但当前代码只提取 `promptTokens`、`completionTokens`、`totalTokens`。

**记录侧** (`record-llm-call.ts:17`)：
```typescript
const reasoningTokens = record.reasoningTokens ?? 0;
```
由于 backend 从不传入，默认为 0。

**定价侧** (`types.ts:72-78`)：`PricingConfig` 没有 `reasoningUsdPer1M` 字段。

**成本计算侧** (`cost.ts:18-37`)：`computeCost` 不包含 reasoning token 的计费逻辑。

**结论**：reasoning tokens **不能**进入成本模型。需要：
- `AiSdkBackend` 从 `result.usage` 提取 reasoning tokens
- `PricingConfig` 增加 `reasoningUsdPer1M` 字段
- `computeCost` 增加 reasoning 计费逻辑

### 5. cache read/write tokens 理论支持但实际不生效

**数据采集侧** (`ai-sdk-backend.ts:504-517`)：

与 reasoning tokens 相同，`cachedInputTokens` 和 `cacheWriteInputTokens` 也未从 `result.usage` 提取并传入 `LlmCallRecord`。

**记录侧** (`record-llm-call.ts:15-16`)：
```typescript
const cachedInputTokens = record.cachedInputTokens ?? 0;
const cacheWriteInputTokens = record.cacheWriteInputTokens ?? 0;
```
默认值为 0。

**定价侧**：内置定价表中 Anthropic 模型有 `cacheReadUsdPer1M` 和 `cacheWriteUsdPer1M`（例如 `claude-sonnet-4-5`: cacheRead=0.3, cacheWrite=3.75），OpenAI 模型有 `cacheReadUsdPer1M`。

**成本计算侧** (`cost.ts:24-29`)：
```typescript
const cacheReadCost = pricing.cacheReadUsdPer1M && usage.cachedInputTokens
  ? (usage.cachedInputTokens / 1_000_000) * pricing.cacheReadUsdPer1M : 0;
const cacheWriteCost = pricing.cacheWriteUsdPer1M && usage.cacheWriteInputTokens
  ? (usage.cacheWriteInputTokens / 1_000_000) * pricing.cacheWriteUsdPer1M : 0;
```
逻辑正确，但 `usage.cachedInputTokens` 和 `usage.cacheWriteInputTokens` 永远为 0。

**结论**：cache 定价体系**就绪**（builtin-pricing 有配置，computeCost 有逻辑，ui 有展示），但**数据源断链** — backend 未从 ai-sdk 提取 cache token 数据。

### 6. provider-specific fields 未提取

**位置**：`ai-sdk-backend.ts:437-448`

`TokenUsageMessage` 只存储 `input` (promptTokens) 和 `output` (completionTokens)，不含 cache tokens 或 reasoning tokens：

```typescript
const tu: TokenUsageMessage = {
  type: 'token_usage',
  id: this.newId(),
  turnId,
  ts: this.now(),
  input: usage.promptTokens ?? 0,
  output: usage.completionTokens ?? 0,
  ...(usage.totalTokens !== undefined ? {} : {}),  // ← 注意：这里构造了一个空对象，并非传递 totalTokens
};
```

`TokenUsageEvent` (`token_usage` event) 同样只有 `input` 和 `output` 两个字段。

### 7. ai-sdk usage promise 可能 reject

**位置**：`ai-sdk-backend.ts:459-461`

```typescript
} catch {
  // best-effort; ai-sdk usage promise may reject on abort
}
```

如果 `result.usage` promise reject（例如 stream abort），则 `tokenUsage` 保持 `undefined`，导致 `inputTokens` 和 `outputTokens` 都记录为 0。此时 `totalCost = 0`，但 status 仍为 `'aborted'` 或 `'error'`。这本身不算 bug（aborted 请求确实不应计费），但需注意 latencyMs 仍被记录。

### 8. 工具调用无成本模型

**位置**：`telemetry-repo.ts:295`

```typescript
costUsd: 0,
```

工具调用（Bash / Read / Write / Edit / Grep / Glob / WebSearch / ExploreAgent 等）**完全不计入成本**。`toolBuckets()` 返回的 `UsageBucket.costUsd` 恒为 0。这是设计决策（工具调用消耗的是本地 CPU/IO，非 API 费用），而非丢失窗口。

---

## tests

### 测试覆盖总览

| 测试文件 | 覆盖内容 | 状态 |
|----------|----------|------|
| `packages/storage/src/__tests__/telemetry-repo.test.ts` (5 个测试) | upsert LLM call、logs filter、runtime probe、buckets (provider/model/day/hour/tool)、pricing override persist | ✅ 覆盖核心 CRUD |
| `packages/core/src/usage-stats/__tests__/pricing.test.ts` (20+ 个测试) | normalizePricingModelKey、normalizePricingConfig 的 object guard、必填/可选字段校验、负数/NaN/Infinity 拒绝、extra fields 剥离、IPC store-boundary 场景模拟 | ✅ 覆盖定价 gate |
| `apps/desktop/src/main/__tests__/web-search-telemetry-scrub-contract.test.ts` | WebSearch 工具 argsSummary 清洗 | ✅ 覆盖隐私清洗 |
| `apps/desktop/src/main/__tests__/settings-usage-contract.test.ts` (11 个测试) | Usage Dashboard 筛选逻辑、空状态、aria-label、详情开关、stale response 保护、session 回链 | ✅ 覆盖 UI 合约 |
| `packages/storage/src/__tests__/settings-store-usage.test.ts` | tool invocation log 不影响 model 统计 | ✅ 覆盖数据一致性 |

### 测试缺口

1. **无 `recordLlmCall` 单元测试** — 没有测试验证 `cachedInputTokens`/`cacheWriteInputTokens`/`reasoningTokens` 的默认值和传递路径。
2. **无 `computeCost` 单元测试** — 没有测试验证不同 pricing config（含/不含 cache rates，含/不含 reasoning）下的成本计算。
3. **无 `recordToolInvocation` 单元测试** — 没有测试验证 tool telemetry 的 argsSummary truncation。
4. **无 `ai-sdk-backend` telemetry 集成测试** — 没有测试验证 backend 从 ai-sdk usage 中提取了正确的 token 数据。
5. **无 process exit flush 测试** — 没有测试验证 telemetry 在进程退出前完成写入。

---

## next_actions

### P0 — 修复 cache/reasoning token 数据采集

**问题**：ai-sdk `result.usage` 中的 `cachedInputTokens`、`cacheWriteInputTokens`、`reasoningTokens` 未被 AiSdkBackend 提取。

**待验证路径**：
1. 确认 ai-sdk (`ai` package) 的 `LanguageModelV2Usage` 类型是否包含这些字段（需要检查 `ai` 的类型定义或运行时 dump）。
2. 在 `AiSdkBackend.send()` 的 finally 块 (`ai-sdk-backend.ts:504-517`) 中，将 usage 对象的对应字段传入 `LlmCallRecord`：
   ```typescript
   cachedInputTokens: tokenUsage?.cachedInputTokens ?? 0,
   cacheWriteInputTokens: tokenUsage?.cacheWriteInputTokens ?? 0,
   reasoningTokens: tokenUsage?.reasoningTokens ?? 0,
   ```
3. 在 `computeCost` 中增加 reasoning token 计费（需要同时扩展 `PricingConfig` 和 `builtin-pricing.ts`）。

### P1 — 增加 process exit flush

在 `app.on('before-quit')` 或 `app.on('will-quit')` 中，调用 telemetryRepo 的 flush 等待写入队列排空。需要在 `TelemetryRepo` 接口中暴露一个 `flush(): Promise<void>` 方法，或者在 `FileTelemetryRepo` 中跟踪 `queue` promise 并暴露给调用方。

### P2 — 增加单元测试

1. `computeCost` 单元测试（验证 reasoning token 不计入当前成本，cache hit/write 价格生效）
2. `recordLlmCall` 单元测试（验证字段默认值和传递）
3. ai-sdk-backend telemetry 集成测试（spy on recordLlmCall，验证 usage 字段的映射）

### P3 — 未定价模型提示

当 `computeCost` 返回全 0 成本时，可以在 UI 中显示 "未配置定价" 提示（而非静默显示 $0.00），引导用户通过 `usage:pricing:put` 添加覆盖。当前 UI 已支持 pricing 覆盖的 CRUD，但缺少对未定价模型的主动提示。

### P4 — TokenUsageMessage/Turn 记录增强

`TokenUsageMessage` 目前只存 `input` 和 `output` 字段，不包含 cache/reasoning tokens。如需在 session JSONL 中完整保留每轮的 token 分布，需要扩展 `TokenUsageMessage` 类型。

---

*报告生成时间: 2026-06-13*
*代码基线: 335220a*
