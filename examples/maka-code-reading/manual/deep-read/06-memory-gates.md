# Memory 9-Gate Runtime Enforcement — 精读报告

> 基线: `335220a` | 深度: maintainer | 模块: `@maka/core/memory` (PR-MEMORY-1)

---

## scope

本报告覆盖 Maka 的 Memory 子系统 **两层架构**:

1. **9-Gate 合约层** (`packages/core/src/memory.ts`): 声明型安全合约 — 封闭枚举、类型约束、`validateMemoryWriteRequest` 归一化器。PR-MEMORY-1 为 contract-only，不含持久化、嵌入、IPC 或 UI。
2. **本地 MEMORY.md 运行时层** (`packages/core/src/local-memory.ts` + `apps/desktop/src/main/local-memory-service.ts`): 实际的 Markdown 文件读写服务，通过 IPC 向 renderer 暴露。

两层之间存在**架构断隙**: 9-Gate 合约没有运行时调用方，而 MEMORY.md 服务并未集成 9-Gate 验证器。详细分析见 `enforcement_gaps`。

---

## problem

`packages/core/src/memory.ts` 定义了 9 条隐私门禁 (`22209a1b`)，但 contract 本身声明它是 "contract-only" — 不实现持久化、嵌入、Recall 工具、renderer UI 或 settings 标志。实际问题：

1. `validateMemoryWriteRequest` 是 9 道门的单一入口函数，但在**整个 `apps/` 和 main process 中没有任何调用方**。
2. 现有的 `LocalMemoryService` (MEMORY.md 文件操作) 是一个完全独立的子系统，使用自己的 `parseLocalMemoryMarkdown`/`save` 等函数，完全不经过 9-Gate 验证器。
3. 门禁 `embedding_disabled` 和 `quasi_memory_promotion_blocked` 在 `MEMORY_BLOCK_REASONS` 枚举中存在，但**没有任何代码路径发出它们** — 它们是预留的未来错误码。
4. 多个门的 enforcement 纯粹依赖 TypeScript 类型系统 (编译时)，没有运行时防护。

---

## source_evidence

| 源码文件 | 行数 | 角色 |
|---|---|---|
| `packages/core/src/memory.ts` | 562 | 9-Gate 合约定义 — 枚举、接口、`validateMemoryWriteRequest` |
| `packages/core/src/local-memory.ts` | 575 | 本地 MEMORY.md 解析/生成 — 独立于 9-Gate 的逻辑 |
| `apps/desktop/src/main/local-memory-service.ts` | 435 | MEMORY.md 文件 IO 服务 — `LocalMemoryService` 类 |
| `docs/memory-threat-model.md` | 119 | 威胁模型文档 — 9 门禁定义、边界、负面参考列表 |
| `packages/core/src/__tests__/memory.test.ts` | 622 | 9-Gate 合约测试 — 每条门均有覆盖 |
| `packages/core/src/__tests__/local-memory.test.ts` | 344 | local-memory 解析/生成测试 |
| `apps/desktop/src/main/__tests__/local-memory-service.test.ts` | 389 | LocalMemoryService IO 测试 |
| `apps/desktop/src/main/__tests__/local-memory-ui-contract.test.ts` | ~800+ | Renderer Settings UI 合约测试 (源码文本断言) |
| `apps/desktop/src/main/main.ts:1381-1422` | — | IPC handlers — 直接调用 `LocalMemoryService`，不经过 9-Gate |

---

## gate_matrix

### G1 — default-off (模式默认关闭)

| 维度 | 详情 |
|---|---|
| **合约定义** | `MEMORY_MODES = ['off', 'manual_only', 'manual_with_drafts']`，`'off'` 在索引 0 → fresh-install default semantic (`memory.ts:68`) |
| **文档落位** | `docs/memory-threat-model.md:43`: "default-off — MEMORY_MODES includes 'off'; fresh-install snapshot MUST be 'off'." |
| **运行时实现** | `validateMemoryWriteRequest` 第 1 步检查 `context.mode === 'off'` → `MemoryBlockReason='mode_off'` (`memory.ts:468-470`) |
| **测试覆盖** | `memory.test.ts:82-98` (G1 describe block): 2 条测试 — durable reject at mode=off, draft reject at mode=off, + `MEMORY_MODES[0] === 'off'` 断言 |
| **实际 runtime 调用** | ❌ 无调用方。`LocalMemoryService.getState()` 检查 `!settings.localMemory.enabled` → status='disabled'，但使用的是 `LocalMemorySettings.enabled` boolean，不是三态 `MemoryMode` 枚举。 |

### G2 — manual confirm before durable write

| 维度 | 详情 |
|---|---|
| **合约定义** | `DurableMemoryEntry.confirmedAt: number` 必填，`validateMemoryWriteRequest:514-519` 检查 `typeof confirmedAt !== 'number' \|\| !Number.isFinite(confirmedAt) \|\| confirmedAt < 0` → `'manual_confirm_required'` |
| **文档落位** | `docs/memory-threat-model.md:44`: "Durable 'active' path requires confirmedAt." |
| **运行时实现** | 仅 `validateMemoryWriteRequest` 内部。`MemorySource + persistenceState !== 'active'` 也归入此门 (line 506-513: non-active durable source → `'manual_confirm_required'`). |
| **测试覆盖** | `memory.test.ts:104-154` (G2 describe block): 5 条测试 — `user_authored` 无 confirmedAt, `chat_extracted` 无 confirmedAt, bad confirmedAt values (NaN/Infinity/-1/string/null), source + non-active persistence, happy path. |
| **实际 runtime 调用** | ❌ 无调用方。MEMORY.md 条目使用 `origin=manual` + `status=active` 的 HTML 注释元数据，不经过 `validateMemoryWriteRequest`，没有 `confirmedAt` 字段。 |

### G3 — reversible delete/export (先有可逆操作才有自动写入)

| 维度 | 详情 |
|---|---|
| **合约定义** | v1 contract 没有 auto-write 路径。文档声明: "downstream packets MUST add delete + export shapes before adding any write driver." `memory.ts:23-25` 硬性禁止导入 IPC/storage/runtime/electron/renderer. |
| **文档落位** | `docs/memory-threat-model.md:45`: "Contract requires reversible operations exist BEFORE any auto-write capability." |
| **运行时实现** | `LocalMemoryService` 提供 `reset()` + `restoreLatestBackup()` + `restoreBackup()` — 这些是**文件级**恢复/重置操作，不是 entry 级 delete/export. |
| **测试覆盖** | `memory.test.ts:160-180` (G3 describe block): 2 条测试 — 检查导出符号中没有 `autoCommit/autoPromote/autoConsolidate` 函数; 验证 `skipConfirm` 额外字段被忽略 (validator strips extras). |
| **实际 runtime 调用** | G3 是**合约形状约束**，非运行时 enforced。`LocalMemoryService` 的文件级备份/恢复机制提供了可逆性保障，但与 entry 级 delete/export 不同。 |

### G4 — incognito read+write disable

| 维度 | 详情 |
|---|---|
| **合约定义** | `MemoryWriteRequestContext.incognitoActive` → `validateMemoryWriteRequest` 第 2 步检查 → `'incognito_active'` (`memory.ts:472-474`) |
| **文档落位** | `docs/memory-threat-model.md:46`: "incognitoActive short-circuits validator at step #2." |
| **运行时实现 (合约层)** | ❌ `validateMemoryWriteRequest` 无调用方。 |
| **运行时实现 (MEMORY.md 层)** | ✅ `LocalMemoryService.getState():34-49` — 如果 `getPrivacyContext().incognitoActive` → `status='incognito_blocked'`，返回空内容。`LocalMemoryService.save():125-127` 同理。`resolveFileForOpen()` 等文件操作也检查 incognito。 |
| **测试覆盖 (合约)** | `memory.test.ts:186-207` (G4 describe block): 3 条测试 — durable write reject, draft write reject, incognito gate 优先于 content validation. |
| **测试覆盖 (MEMORY.md)** | `local-memory-service.test.ts:339-349` — incognito 状态下 getState 返回 incognito_blocked 且不创建文件; `local-memory-service.test.ts:358-368` — incognito 下 resolveFileForOpen 返回 blocked. |
| **命名差异** | 合约层 `MemoryWriteRequestContext.incognitoActive` → `BlockReason='incognito_active'`; MEMORY.md 层 `status='incognito_blocked'`. 两种命名不一致。 |

### G5 — no auto sleep consolidation

| 维度 | 详情 |
|---|---|
| **合约定义** | `MEMORY_SOURCES` 和 `MEMORY_CANDIDATE_SOURCES` 中不存在 sleep/consolidate/auto 类型的 source。添加任何一种需要扩展枚举 (contract change). |
| **文档落位** | `docs/memory-threat-model.md:47`: "NOT in MemorySource enum; NOT in MemoryCandidateSource." |
| **运行时实现** | 纯类型系统 enforced — `MemorySource` 和 `MemoryCandidateSource` 是封闭的 `as const` 数组。无法在运行时创建新 source 值而不触碰合约。 |
| **测试覆盖** | `memory.test.ts:213-225` (G5 describe block): 对 `MEMORY_SOURCES` 和 `MEMORY_CANDIDATE_SOURCES` 的每个值做 `assert.doesNotMatch(source, /sleep\|consolidat\|auto/i)`. |
| **实际 runtime 调用** | 不需要 runtime call — 类型级 enforcement 足够。但要注意: `local-memory.ts` 有 `LocalMemoryOrigin = 'manual' \| 'extracted' \| 'imported' \| 'unknown'` 是另一套独立的枚举，不受 9-Gate source 分离约束。 |

### G6 — visible citation (使用策略枚举)

| 维度 | 详情 |
|---|---|
| **合约定义** | `MEMORY_USE_POLICIES = ['never', 'cited_only']` — 不存在 `'silent'` 策略 (`memory.ts:142-143`) |
| **文档落位** | `docs/memory-threat-model.md:48`: "MemoryUsePolicy only allows 'never' or 'cited_only'. No 'silent' policy." |
| **运行时实现** | 纯类型系统 enforced — `MemoryUsePolicy` 是 `typeof MEMORY_USE_POLICIES[number]`. 添加 `'silent'` 需要修改 `as const` 数组. |
| **测试覆盖** | `memory.test.ts:231-241` (G6 describe block): 2 条测试 — 验证 enum 精确为 `['never', 'cited_only']`; 验证 `isMemoryUsePolicy` 拒绝 `'silent'/'auto'/'always'/'unrestricted'`. |
| **实际 runtime 调用** | ❌ 无调用方。`MemoryUsePolicy` 定义在 `MemoryCapabilitySnapshot.usePolicy` 中，但没有任何代码构建或消费此 snapshot。 |

### G7 — no hidden activity promotion (candidate-cannot-active)

| 维度 | 详情 |
|---|---|
| **合约定义** | `MemorySource` 与 `MemoryCandidateSource` 是 disjoint 枚举。`validateMemoryWriteRequest:488-494` 检查 `source.kind === 'candidate' && persistence === 'active'` → `'candidate_source_no_active'`. |
| **文档落位** | `docs/memory-threat-model.md:49`: "activity_observation / cu_observation are MemoryCandidateSource only." |
| **运行时实现** | `validateMemoryWriteRequest` 内部 enforced。`DurableMemoryEntry.source` 类型为 `MemorySource`, `DraftMemoryEntry.source` 类型为 `MemoryCandidateSource` — 编译时 disjoint. |
| **测试覆盖** | `memory.test.ts:247-296` (G7 describe block): 5 条测试 — 每个 candidate source + active → reject; candidate→active 优先于 mode_disallows; `manual_only` 拒绝 candidate draft; `manual_with_drafts` 接受 candidate draft; candidate + review_required 接受. |
| **实际 runtime 调用** | ❌ 无调用方。`local-memory.ts` 的 `LocalMemoryOrigin` 枚举 (`manual/extracted/imported/unknown`) 与合约的 source 分离完全独立，不经过 validator. |
| **命名差异** | 合约 `MemoryCandidateSource`: `voice_transcript, activity_observation, cu_observation, search_recall, daily_review`. local-memory `LocalMemoryOrigin`: `manual, extracted, imported, unknown`. 两套枚举语义不同，互不约束。 |

### G8 — provider+embedding leakage boundary

| 维度 | 详情 |
|---|---|
| **合约定义** | `MemoryCapabilitySnapshot.embeddingProvider` 类型为字面量 `'disabled'` (`memory.ts:197`). 硬编码阻止任何 provider 连接. |
| **文档落位** | `docs/memory-threat-model.md:50`: "embeddingProvider is the literal 'disabled'." |
| **运行时实现** | 纯类型系统 — TypeScript 字面量类型 `'disabled'`. 无法赋值其他值而不破坏类型检查. |
| **测试覆盖** | `memory.test.ts:302-331` (G8 describe block): 2 条测试 — 构造 snapshot 验证 `embeddingProvider === 'disabled'`; 验证类型为 string. |
| **实际 runtime 调用** | ❌ `MemoryCapabilitySnapshot` 未被任何代码路径构造。`local-memory.ts` 的 `LocalMemoryState` 没有 `embeddingProvider` 字段. |
| **预留错误码** | `MEMORY_BLOCK_REASONS` 包含 `'embedding_disabled'` (`memory.ts:157`)，但 `validateMemoryWriteRequest` 从未发出它。在测试 `memory.test.ts:585` 中仅验证该枚举值存在。 |

### G9 — renderer cannot forge provenance/readiness

| 维度 | 详情 |
|---|---|
| **合约定义** | `MemoryWriteRequestContext.originatedFromRenderer` → `validateMemoryWriteRequest:522-527` 检查: 如果 `originatedFromRenderer=true` + memory source + active → `'renderer_provenance_forged'`. Renderer 可以 proposal drafts (candidate source), 但不能记录 `confirmedAt`. |
| **文档落位** | `docs/memory-threat-model.md:51`: "originatedFromRenderer=true blocks any durable active write." |
| **运行时实现** | `validateMemoryWriteRequest` 内部 enforced. 但需要调用方正确地设置 `originatedFromRenderer` flag — 实际上**没有调用方设置它**. |
| **测试覆盖** | `memory.test.ts:337-356` (G9 describe block): 2 条测试 — renderer durable active → reject; renderer candidate draft → accept. |
| **实际 runtime 调用** | ❌ 无调用方。`main.ts` 的 IPC handlers 不调用 `validateMemoryWriteRequest`, 因此 `originatedFromRenderer` flag 从未被设置。对于 MEMORY.md 路径, renderer 通过 IPC `memory:save` 直接传 raw content string 给 main, main 的 `LocalMemoryService.save()` 不区分 renderer/main 来源。 |

---

## enforcement_gaps

### 架构断隙: 9-Gate 合约 vs MEMORY.md 运行时

**核心 gap**: `validateMemoryWriteRequest` 在 `apps/` 目录中零调用。

```
packages/core/src/memory.ts          ← 9-Gate 合约 (contract-only)
  └── validateMemoryWriteRequest()   ← 所有 9 门的入口函数
      ├── 调用方: 无 (仅在测试中使用)
      └── apps/desktop/src/main/main.ts  ← IPC handlers
            └── localMemory.save(content)  ← 绕过 9-Gate, 直接写入 MEMORY.md
```

`main.ts:1382-1385`:
```ts
ipcMain.handle('memory:save', async (_event, content: unknown): Promise<LocalMemoryState> => {
  if (typeof content !== 'string') return localMemory.getState();
  return localMemory.save(content);  // 不经过 validateMemoryWriteRequest
});
```

### 逐门 enforcement 缺口

| Gate | 类型/文档 | Runtime Enforcement | 缺口描述 |
|---|---|---|---|
| G1 | ✅ 类型 + 测试 | ❌ 无 | `mode='off'` 门在 `validateMemoryWriteRequest` 中实现，但无调用方触发。MEMORY.md 层有自己的 `enabled` boolean 逻辑。 |
| G2 | ✅ 类型 + 测试 | ❌ 无 | `confirmedAt` 验证逻辑只在 `validateMemoryWriteRequest` 中；MEMORY.md 条目通过 `<!-- maka-memory: ... -->` 注释设 `status=active`，无 confirmedAt 概念。 |
| G3 | ✅ 文档声明 | ⚠️ 部分 | 文件级 backup/restore 有 (`LocalMemoryService.reset/restore`)，但 entry 级 delete/export 不存在。合约要求 downstream packet 添加 delete+export shapes。 |
| G4 | ✅ 类型 + 测试 | ✅ 有 (MEMORY.md 层) | `LocalMemoryService` 正确检查 incognito。但 `incognito_active` vs `incognito_blocked` 命名不一致。 |
| G5 | ✅ 类型系统 | ✅ 类型级 | 封闭枚举，无需额外 runtime。但 `LocalMemoryOrigin` 是独立枚举，不受 G5 约束。 |
| G6 | ✅ 类型系统 | ❌ 无 | `MemoryUsePolicy` 枚举只在 contract 中存在，无消费方。实际 prompt 注入逻辑在 `buildLocalMemoryPromptBody` 中，不引用 `MemoryUsePolicy`. |
| G7 | ✅ 类型 + 测试 | ❌ 无 | `candidate_source_no_active` 检查只在 `validateMemoryWriteRequest` 中。MEMORY.md 的 `origin=extracted` 不在 candidate source 枚举中，可直接设 `status=active`. |
| G8 | ✅ 类型系统 | ❌ 无 | `embeddingProvider: 'disabled'` 是 TS 字面量锁。`embedding_disabled` 错误码预留但从未发出。 |
| G9 | ✅ 类型 + 测试 | ❌ 无 | `originatedFromRenderer` flag 无 setter。`main.ts` IPC handler 不区分 renderer/main 请求来源。 |

### 预留错误码未使用

`MEMORY_BLOCK_REASONS` 中有 2 个错误码在任何代码路径中都未发出:

| 错误码 | 定义位置 | 状态 |
|---|---|---|
| `embedding_disabled` | `memory.ts:157` | 预留，`validateMemoryWriteRequest` 不发出。仅在 `memory.test.ts:585` 中做存在性断言。 |
| `quasi_memory_promotion_blocked` | `memory.ts:158` | 预留，无任何代码路径发出。仅在 `memory.test.ts:586` 中做存在性断言。 |

### Source-laundering 防御缺口

`docs/memory-threat-model.md:84` 明确指出的 source-laundering 问题：如果下游 IPC handler 从 usage-log 等 quasi-memory 表面读取内容，将其 body 复制到 `{ source: 'chat_extracted', confirmedAt: ..., content: ... }` payload 中提交给 validator — validator 会接受它。防御责任在 "per-IPC-handler / per-store-boundary"，但当前没有任何 IPC handler 实现了 provenance gate。

### 两套枚举命名不一致

| 概念 | 9-Gate 合约 (memory.ts) | MEMORY.md 层 (local-memory.ts) |
|---|---|---|
| 来源类型 | `MemorySource` / `MemoryCandidateSource` (disjoint) | `LocalMemoryOrigin = 'manual' \| 'extracted' \| 'imported' \| 'unknown'` |
| 持久化状态 | `MemoryPersistenceState = 'draft' \| 'review_required' \| 'active'` | `LocalMemoryEntryStatus = 'active' \| 'archived'` |
| 模式 | `MemoryMode = 'off' \| 'manual_only' \| 'manual_with_drafts'` | `LocalMemorySettings.enabled: boolean` |
| 隐身阻断原因 | `MemoryBlockReason = 'incognito_active'` | `LocalMemoryState.status = 'incognito_blocked'` |

两套词汇表当前并行存在，无桥接代码。

---

## tests

### 9-Gate 合约测试 (`packages/core/src/__tests__/memory.test.ts`)

- **总测试数**: 约 45 条 (跨 12 个 describe block)
- **G1-G9 完整覆盖**: ✅ 每条门至少 2 条测试
- **Normalizer 矩阵**: ✅ `normalizeMemoryContent` (6 tests), `normalizeMemorySource` (5 tests), closed-enum normalizers (4 tests)
- **Quasi-memory exclusion**: ✅ 覆盖 `usage_log`, `settings`, `session_summary`, `skill_inject`, `workspace_instruction`, `onboarding_milestone`, `health_probe`, `visual_smoke_fixture`
- **Block reason 枚举完整性**: ✅ 验证所有 11 个活跃 emitted reasons + 2 个预留 reasons
- **Canonical return shape**: ✅ DurableMemoryEntry 和 DraftMemoryEntry 的字段验证

### MEMORY.md 层测试 (`packages/core/src/__tests__/local-memory.test.ts`)

- **总测试数**: 14 条
- **覆盖**: 解析、生成、prompt 构建、redaction、draft append、status 切换、id stability、oversized 处理、safe mode
- **覆盖 9-Gate 吗**: ❌ 不覆盖。这些测试只针对 Markdown 解析/生成的业务逻辑。

### LocalMemoryService IO 测试 (`apps/desktop/src/main/__tests__/local-memory-service.test.ts`)

- **总测试数**: 17 条
- **覆盖**: 文件创建权限 (0700/0600)、save/backup/restore/reset 生命周期、incognito blocking、symlink escape 防御、redaction、oversize safe-mode、archived entry counting
- **覆盖 9-Gate 吗**: ❌ 不覆盖。主要测试文件 IO 安全 (path traversal, symlink, permissions).

### Renderer UI 合约测试 (`apps/desktop/src/main/__tests__/local-memory-ui-contract.test.ts`)

- **总测试数**: 25+ 条
- **覆盖**: 通过 `readFile` 读取 TSX/CSS 源码做文本断言 — 验证 UI 渲染 entry 分组、truncation、filter、draft state、backup metadata、action gating、lifecycle cleanup
- **测试类型**: 源码字符串匹配 (`assert.match / assert.doesNotMatch`)，不是组件渲染测试
- **覆盖 9-Gate 吗**: ❌ 不覆盖。纯 UI 结构/文案验证。

### 测试缺口汇总

| 缺口 | 严重性 | 描述 |
|---|---|---|
| `validateMemoryWriteRequest` 集成测试 | 🔴 高 | 没有任何测试验证 IPC handler 在写入前调用 validator |
| `originatedFromRenderer` 实际设置 | 🔴 高 | G9 在测试中通过 `ctx({ originatedFromRenderer: true })` 手动设置，但 real code 中无 setter |
| `embedding_disabled` 发出路径 | 🟡 中 | 错误码存在但无代码路径/测试验证其发出 |
| `quasi_memory_promotion_blocked` 发出路径 | 🟡 中 | 同上 |
| 两套枚举桥接测试 | 🟡 中 | 无测试验证 `LocalMemoryOrigin` 值不会在 future 中绕过 `MemoryCandidateSource` 约束 |
| Source-laundering 防御测试 | 🟡 中 | 文档`memory-threat-model.md:84` 承认 source-laundering 是 downstream concern，但无测试覆盖 IPC-boundary provenance gate |

---

## next_actions

1. **PR-MEMORY-2 必须集成 `validateMemoryWriteRequest`**: 当前 contract-only 的 9-Gate 函数在 runtime 中零调用。下一个 implementation packet (PR-MEMORY-2) 必须在 IPC handler 层调用 `validateMemoryWriteRequest`，并正确设置 `originatedFromRenderer` flag。具体而言，`main.ts` 的 `memory:save` handler 应该:
   - 解析 renderer 传来的 structured payload (source, persistenceState, scope, content, confirmedAt)
   - 设置 `context.originatedFromRenderer = true`
   - 调用 `validateMemoryWriteRequest(request, context)`
   - 只在 `result.ok === true` 时持久化

2. **统一两套枚举或明确边界**: `LocalMemoryOrigin` 和 `MemorySource/MemoryCandidateSource` 是两套独立枚举。需要决策: 是废弃 `LocalMemoryOrigin` 改用 9-Gate source，还是明确文档说明两套枚举的应用边界。

3. **实现 `embedding_disabled` 和 `qusion_memory_promotion_blocked` 的实际发出路径**: 两个预留错误码需要在对应功能实现时 (embedding provider support, quasi-memory promotion gate) 加入 validator 逻辑和测试。

4. **添加 `MemoryCapabilitySnapshot` 的构造与消费**: 当前 `MemoryCapabilitySnapshot` 仅在测试中构造。runtime 需要一处设置 mode/incognitoActive/usePolicy/entryCounts 并传递给需要的 consumer。

5. **添加 IPC-boundary provenance gate 测试**: 按 `memory-threat-model.md:84` 的要求，为 "promote draft to active" 和 "summarize quasi-surface as chat_extracted" 路径添加 provenance gate 测试。

6. **补充 incognito 命名一致性**: 合约层 `incognito_active` vs MEMORY.md 层 `incognito_blocked` — 建议统一为前者或明确文档化映射关系。
