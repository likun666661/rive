# JSONL Durability / Migration — 精读报告

> 仓库: `/Users/likun/Desktop/workspace-for-maka/maka`
> 阅读基线: `335220a`
> 深度档位: `maintainer`
> 输出时间: 2026-06-13

---

## scope

本报告覆盖 Maka 的 JSONL 会话持久化层 (`FileSessionStore`)、Session 类型体系、运行时恢复逻辑 (`SessionManager.recoverInterruptedSessions`)、Materializer 视图重建、以及相关的健康信号/事件流诊断代码。分析聚焦于：

- 磁盘格式 `session.jsonl` 的读写原子性、错误传播路径
- 异常行损坏、header 损坏、尾行截断、并发写、schema migration 的真实行为
- 已有测试覆盖和缺口

**不覆盖**: 非 JSONL 持久化（connection 存储、config store）、IPC 传输层的可靠性。

---

## problem

JSONL（JSON Lines）作为会话存储格式，天然存在以下耐久性风险：

1. **单行损坏**: 任一行的 JSON 解析失败会导致整个 session 不可读，异常向上传播到 UI。
2. **Header 损坏**: 第 1 行损坏意味着整个 session 无法恢复，用户看到空白或崩溃。
3. **尾行截断**: `appendFile` 在进程崩溃时可能留下半行，产生不可解析的 JSON 碎片。
4. **并发写**: 同一 session 的多 path 并发写入（如 sendMessage + stopSession）需要序列化。
5. **Schema 演进**: `schemaVersion` 字段存在（值为 1），但只有 read-time migration，没有明确的版本跳转逻辑。
6. **无修复工具**: 没有 CLI 或诊断工具可以扫描/修复损坏的 JSONL 文件。

---

## source_evidence

### 1. 存储层 — `packages/storage/src/session-store.ts`

**文件格式** (`sessionPath`, line 216–218):
```
sessions/<uuid>/session.jsonl
```
第 1 行 = `JSON.stringify(header)`，第 2 行+ = 每条 `StoredMessage` 的 JSON 序列化。行分隔符 = `\n`。

**全局写入队列** (`withQueue`, lines 240–251):
```
sessionId → Promise<void> 链
每个 session 拥有独立的写入队列，所有操作 (create/appendMessages/updateHeader/remove)
通过 `previous.then(operation)` 串行化。
```
- 链式调用保证同一 session 不会并发写。
- `.catch(() => {})` 在链上吞掉错误，保持队列存活——**意味着前一个写入失败后，后续写入可以继续，但中间状态可能不一致。**

**读操作** (`readFilePartsUnlocked`, lines 224–231):
```typescript
const text = await readFile(path, 'utf8');
const lines = text.split('\n').filter(line => line.trim().length > 0);
const header = migrateHeader(JSON.parse(lines[0]) as StoredSessionHeader);
const messages = lines.slice(1).map(line => JSON.parse(line) as StoredMessage);
```
- 空行被过滤（`filter(line => line.trim().length > 0)`）
- **没有任何 try/catch 包裹单行解析** — `JSON.parse` 对任何非法行直接 throw
- 空文件或 header 为 `undefined`（空行过滤后 `lines[0]` 为 `undefined`）抛出 `"Session is empty"`

**写原子性** (`writeAtomic`, lines 233–238):
```
写临时文件 (.tmp) → rename 覆盖原文件
```
- header 更新使用此模式，**防止 header 半写损坏**
- **不防止 rename 后磁盘缓存未刷盘**（无 `fsync`）

**append 操作** (`appendMessages`, lines 152–159):
```typescript
await import('node:fs/promises').then(fs =>
  fs.appendFile(path, payload, 'utf8'));
```
- 使用 Node.js `fs.promises.appendFile` — 底层是 `write(2)` with `O_APPEND`。
- **不保证原子性**: 如果进程在 `write` 中途崩溃，可能留下不完整行。
- 带 `\n` 前缀确保每个消息行以换行开始（但首行是 header）。

**Header Migration** (`migrateHeader`, lines 272–300):
- `backend: 'claude'` → `'ai-sdk'`
- `backend: 'pi'` → `'pi-agent'`
- `permissionMode` 无效 → `'ask'`
- `model` 非字符串/空 → `'default'`
- `status` 推导: `isArchived` → `'archived'`；有效 status → 保持；其余 → `'active'`
- `blockedReason` 仅在 status 为 `'blocked'` 且有效时保留

**list 的错误处理** (lines 111–113):
```typescript
} catch {
  // Ignore malformed session folders in the sidebar.
}
```
唯一一处显式吞掉腐坏 session 的地方——腐坏的 session 目录不会出现在侧边栏，但**不会主动修复或通知用户**。

### 2. 运行时恢复 — `packages/runtime/src/session-manager.ts`

**`recoverInterruptedSessions()`** (lines 155–184):
```
遍历所有非归档 session →
  读取消息:
    成功 → 调用 interruptedTurnRecoveries() 找 running 状态的 turn → 标记为 failed
    失败 → 如果 status 是 running/waiting_for_user → 重置为 active
```

**`interruptedTurnRecoveries()`** (lines 691–727):
- 遍历所有消息，按 turnId 分组
- 若最新 `turn_state` 状态为 `running` → 标记为 `failed(errorClass: 'app_restarted')`
- 若最新状态为 `completed` 但无 `assistant` 消息且有前置 `failed` → 也标记为 `failed`
- **不在 JSONL 中删除/修复任何数据**，只追加新的 `turn_state` 消息

### 3. Materializer — `packages/runtime/src/materializer.ts`

**`materializeSession()`** (lines 70–136):
- 两遍扫描：第一遍索引 `tool_result` 和 `permission_decision`，第二遍输出 `ChatItem`
- **孤儿 `tool_call`（无匹配 `tool_result`）状态为 `interrupted`**（line 155）
- 不抛出异常 — 即使消息序列不完整也能渲染

### 4. 健康信号 — `packages/core/src/health.ts`

- `HealthSignalLayer` 包含 `'storage'` 枚举值 (line 23)，但**代码中没有任何地方生成 `layer: 'storage'` 的健康信号**。
- HealthSnapshot 聚合了连接健康、能力健康、运行时探测——**没有存储健康**。

### 5. Session 事件流诊断 — `packages/core/src/session-event-health.ts`

- 检测事件流 staleness（15s 阈值）、连接状态、恢复状态
- **不检测 JSONL 文件层面的损坏**

### 6. Session 名称迁移 — `packages/core/src/session-name.ts`

- 9 层管道：类型守卫 → NFC 规范化 → 控制字符替换 → 双向格式字符替换 → 零宽字符移除 → 空白折叠 → trim → 空检查 → 80 码点截断
- 所有写入口 (`create`, `rename`, `branchFromTurn`) 共享同一 `normalizeUserSessionName` 入口

---

## durability_matrix

| 操作 | 方法 | 原子性 | 错误处理 | 传播路径 | 用户可见行为 |
|---|---|---|---|---|---|
| **create** | `writeFile(session.jsonl, header)` | 非原子（先 mkdir 再 writeFile） | header 构造阶段 `normalizeUserSessionName` 可 reject；IO 阶段 throw | `SessionManager.createSession` → IPC → renderer | 创建失败时对话框无法新建，错误信息透传 |
| **read (header + messages)** | `readFile → split('\n') → JSON.parse` | 读快照（不锁） | 任一行 JSON 解析失败 → throw；空文件 → throw；文件不存在 → throw | `readHeader` / `readMessages` → `SessionManager.getMessages` / `sendMessage` → renderer | **JSONL 损坏 → 整个 session 打不开**，渲染器收到异常 |
| **append (messages)** | `appendFile(path, payload)` | **非原子** — 进程崩溃可能截断尾行 | IO 异常 throw | `appendMessages` → `SessionManager.sendMessage` / `stopSession` | 尾行截断 → 下次 read 时 `JSON.parse` 失败 → session 不可读 |
| **writeAtomic (header)** | `writeFile(.tmp) → rename` | writeFile + rename 分两步；rename 在 POSIX 上是原子的 | IO 异常 throw | `updateHeader` → 各种 `SessionManager` 操作 | rename 失败 → 旧 header 保留；半写 → .tmp 残留不污染正式文件 |
| **migrate (header)** | read-time `migrateHeader()` | 内存操作，不持久化 | 无效字段静默替换为默认值；不 throw | `readFileParts` → `readHeader/readMessages/list` | 旧格式 session 正常显示；`schemaVersion` 保持为 1，不原地升级 |
| **header update** | `readFilePartsUnlocked + writeAtomic` | 读-改-写：读时无锁，有写队列串行化 | 读失败 throw；写失败 throw | `updateHeader` → 调用方 | 读-改-写期间有新 append 不包含在内 → 丢失新 append 的消息行（见下方详析） |
| **read messages** | `readFileParts` 返回全部行 | 读快照 | 任一消息行解析失败 throw | `readMessages` / `listTurns` | 同上，损坏 = session 不可读 |
| **list (sessions)** | `readdir → 逐个 readFileParts` | 跨多个独立读 | per-session try/catch，腐坏 session 被 **静默跳过** | `listSessions` → 侧边栏渲染 | 腐坏 session 从列表中消失，无任何提示 |
| **search (N/A)** | — | — | — | — | **不存在** session 内容搜索功能 |
| **usage (token)** | Token 数据嵌入 `token_usage` 消息中 | append-only，不单独存储 | 无效 token 行会阻塞整个 session 读 | Materializer 聚合 | token 统计计算正常 |

### 关键风险详析

#### A. `updateHeader` 的读-改-写竞态

`updateHeader` (lines 161–171) 的模式是：
1. `readFilePartsUnlocked` — 读取 header + 全部消息
2. 构造新的 header
3. `writeAtomic` — 写入 header + 所有消息

如果在步骤 1 和 3 之间有新的消息通过 `appendMessages` 写入（虽然写队列串行化保证不会在同一个 queue 中并发，但 `readFilePartsUnlocked` 是**绕过写队列的直接文件读**），`writeAtomic` 会用旧的消息列表**覆盖**掉新 append 的消息。

**写队列的保护边界**：`withQueue` 串行化同一 session 的写操作，但 `updateHeader` 内部的 `readFilePartsUnlocked` 不在队列内。这意味着：
- 如果 `updateHeader` 的读写过程没有其他写操作入队（因为 updateHeader 本身在队列中），则安全
- **但 `readFilePartsUnlocked` 直接读文件，不通过队列**——理论上读到的消息列表可能不是最新的（如果队列里还有其他待执行的写）

实际上由于队列的 `promise.then()` 链式执行，`readFilePartsUnlocked` 会在前一个操作完成后执行，所以是安全的。但代码层面没有显式的读锁。

#### B. 异常传播链

```
JSON.parse 失败
  → readFilePartsUnlocked throw
    → readFileParts throw
      → readHeader / readMessages throw
        → SessionManager.getMessages / sendMessage throw
          → IPC handler catch / 调用方 try-catch
            → 渲染器显示错误
```

**唯一的吞掉点**：
- `session-store.ts:111–113` — `list()` 中 per-session 的 catch，静默跳过腐坏 session
- `session-manager.ts:164–169` — `recoverInterruptedSessions()` 中 readMessages 失败时重置状态
- `session-manager.ts:416–418` — `sendMessage` finally 中 updateHeader 失败被吞掉

---

## recovery_plan

### 1. 单行损坏（Line Corruption）

**当前行为**:
- `readFilePartsUnlocked` → `JSON.parse(line)` 直接 throw
- 整个 session 不可读
- `list()` 静默跳过（侧边栏不显示）
- `readHeader()` / `readMessages()` 异常传播到调用方

**设计建议**:
```typescript
// 读取时逐行容错
function readFilePartsRobust(path: string) {
  const text = await readFile(path, 'utf8');
  const lines = text.split('\n');
  const headerLine = lines.find(l => l.trim().length > 0);
  const header = migrateHeader(JSON.parse(headerLine)); // header 损坏 = 整 session 不可恢复，仍然 throw

  const messages: StoredMessage[] = [];
  for (let i = lines.indexOf(headerLine) + 1; i < lines.length; i++) {
    try {
      messages.push(JSON.parse(lines[i]));
    } catch {
      // 记录损坏行号，追加 system_note 标记数据损失
      messages.push({
        type: 'system_note',
        id: uuid(),
        ts: Date.now(),
        kind: 'error',
        data: { reason: 'corrupt_line', lineIndex: i, raw: lines[i].slice(0, 200) }
      });
    }
  }
  return { header, messages };
}
```
- **影响最小化**: 只丢单行，其余消息仍可渲染
- **用户感知**: 聊天记录中出现一条 "数据损坏" 的系统提示

### 2. Header 损坏（Header Corruption）

**当前行为**:
- `JSON.parse(lines[0])` 直接 throw → session 不可恢复
- `list()` 静默跳过

**设计建议**:
- 尝试从 header 备份文件恢复（如果需要，可在 `writeAtomic` 时同步写 `.header.bak`）
- 若 header 丢失，从 session 目录名（UUID）和消息内容推导最小可恢复 header
- 若无法恢复，在侧边栏显示受损标记，提供 "删除此会话" 入口
- **不应静默跳过** —— 用户不知道 session 丢了

### 3. 尾行截断（Trailing Truncation）

**当前行为**:
- `appendFile` 不保证原子性，进程崩溃可能产生不完整行
- 下次读取时 `JSON.parse(partial_line)` throw → 整个 session 不可读

**设计建议**:
```typescript
// 读取时处理尾行截断
try {
  const msg = JSON.parse(lines[i]);
  messages.push(msg);
} catch (e) {
  if (i === lines.length - 1 && (e instanceof SyntaxError)) {
    // 尾行截断：静默丢弃
    console.warn(`Trailing line in session ${sessionId} truncated, discarded`);
    break;
  }
  throw e; // 非尾行损坏仍然 throw
}
```
- **或**: `appendMessages` 写入后追加校验行（如 checksum 行），读取时验证
- **或**: 使用 `writeFile` + `appendFile` 配合 `fsync` 降低窗口

### 4. 并发写（Concurrent Write）

**当前行为**:
- 通过 `withQueue` 的 per-session promise 链实现串行化
- 队列链 `.catch(() => {})` 保证失败后队列继续

**风险**:
- 多个 `SessionManager` 实例（多窗口）共享同一 `SessionStore` → 不同 `FileSessionStore` 实例的操作不经过同一写队列
- `updateHeader` 的读-改-写模式在队列外读文件

**设计建议**:
- 考虑使用文件锁（`flock`）或跨进程写队列
- `readFilePartsUnlocked` 应在写队列内执行（当前已在队列内，line 164 在 `withQueue` 回调中调用）

### 5. Schema Migration

**当前行为**:
- `schemaVersion: 1` 硬编码，无升级逻辑
- `migrateHeader()` 只在读时应用内存迁移，不写回
- 没有版本号对应的迁移步骤表

**设计建议**:
- 定义迁移步骤映射: `const MIGRATIONS: Record<number, (header) => header> = { 2: migrateV1toV2, ... }`
- 读取时若 `stored.schemaVersion < CURRENT_VERSION`，应用所有迁移步骤后 `writeAtomic` 写回
- 写回时更新 `schemaVersion` 为 `CURRENT_VERSION`
- 现在 `schemaVersion` 停留在 1，意味着**所有 session 永远触发 migration 代码**——应写回避免重复开销

---

## tests

### 已有测试覆盖 — `packages/storage/src/__tests__/session-store.test.ts`

| 测试 | 覆盖项 | 状态 |
|---|---|---|
| `archive sets isArchived...` | archive/unarchive 行为 | ✅ |
| `new sessions default to active status` | status 字段默认值 | ✅ |
| `persists session branch lineage` | parentSessionId / branchOfTurnId | ✅ |
| `setFlagged toggles the flag` | flag 切换不污染其他字段 | ✅ |
| `rename trims whitespace...` | rename 行为 + 长度上限 | ✅ |
| `remove deletes the directory entirely` | 删除完整性 | ✅ |
| `rejects traversal-style session ids` | 路径遍历防护 | ✅ |
| `migrates legacy headers without permissionMode` | claude → ai-sdk migration | ✅ |
| `migrates legacy headers without model` | model → 'default' migration | ✅ |
| `migrates archived legacy headers` | isArchived → status migration | ✅ |
| `derives lastMessagePreview...` | 最后消息预览 | ✅ |
| `lastMessagePreview skips internal-only tails...` | 预览忽略 system_note，emoji 截断 | ✅ |
| `listTurns derives latest persisted turn states` | turn 状态推导 | ✅ |
| `listTurns projects legacy message-only turns` | 无 turn_state 的向下兼容 | ✅ |
| `normalizeUserSessionName store-boundary` 组 (6 个) | 控制字符、Bidi、零宽、undefined、空串、emoji 边界 | ✅ |

### 已有测试覆盖 — `packages/runtime/src/__tests__/session-manager.test.ts`

| lines | 测试 | 状态 |
|---|---|---|
| 425–481 | 启动恢复: running turn → failed | ✅ |
| 483–498 | 启动恢复: readMessages 失败时重置 status | ✅ |

### 测试缺口

| 缺口 | 严重度 | 说明 |
|---|---|---|
| **JSONL 单行损坏** | 🔴 高 | 无测试覆盖 `JSON.parse` 对非法 JSON 行的行为 |
| **JSONL 尾行截断** | 🔴 高 | 无测试模拟 `appendFile` 截断后读取 |
| **JSONL header 损坏** | 🔴 高 | 无测试模拟 header 行 JSON 非法时的恢复 |
| **writeAtomic 与 append 交织** | 🟡 中 | 无测试验证 `updateHeader` 的读-改-写期间有新 append 的场景 |
| **schemaVersion 升级不回写** | 🟡 中 | 无测试验证 migration 后 schemaVersion 是否应更新 |
| **并发 session 写** | 🟡 中 | 无测试模拟两个 store 实例同时操作同一 session |
| **`list()` 静默跳过腐坏 session** | 🟡 中 | 无测试验证腐坏 session 确实被跳过且无通知 |
| **大消息 (>1MB) 写** | 🟢 低 | 无压力测试大消息的 append/writeAtomic 行为 |
| **磁盘满场景** | 🟢 低 | 无测试 ENOSPC 错误后的恢复行为 |
| **`writeAtomic` 的 `fsync` 缺失** | 🟢 低 | 无测试验证系统崩溃后数据完整性（需要集成测试） |
| **Health signal `layer: 'storage'`** | 🟡 中 | 枚举定义了但从未生成信号，无测试确保存储健康被上报 |

---

## next_actions

### 优先级 P0 — 立即修复

1. **行级容错读取**: 修改 `readFilePartsUnlocked`，逐行 `try/catch` JSON.parse，损坏行记录 `system_note` 而非 throw。仅 header 行损坏才 throw（session 无法恢复）。
2. **尾行截断处理**: 尾行（最后一行）解析失败时静默丢弃，非尾行按 P0-1 处理。
3. **`list()` 静默跳过 → 显式标记**: 腐坏 session 应在 `list()` 结果中返回标记（如 `corrupted: true`），UI 可显示损坏标识和修复/删除入口。

### 优先级 P1 — 应该修复

4. **schemaVersion 写回**: `migrateHeader()` 后若 header 有变更，通过 `writeAtomic` 写回并更新 `schemaVersion` 为 `CURRENT_VERSION`。当前所有 session 每次读都走 migration — 纯浪费。
5. **Health signal `layer: 'storage'` 落地**: 在 session store 层生成存储健康信号（文件可读、格式有效、无损坏行），汇入 HealthSnapshot。
6. **`writeAtomic` 添加 `fsync`**: 在 `rename` 成功后对目录做 `fsync`（或至少对父目录），降低崩溃丢数据窗口。

### 优先级 P2 — 建议改进

7. **JSONL 修复 CLI/工具**: 提供诊断命令扫描所有 session 文件，报告损坏行数、尾行截断、schema version，可选 `--repair` 模式。
8. **`appendFile` 原子性**: 评估替换为 `writeAtomic` append（读全部消息 + 追加 + 写临时文件 + rename），牺牲大 session 性能换取原子性。或采用分段文件（`.jsonl` + `.jsonl.N` 轮转）。
9. **跨进程写队列**: 如果支持多窗口，需考虑跨进程的文件锁（`proper-lockfile` 或 `flock`）。
10. **集成崩溃测试**: 使用进程 `kill -9` 模拟崩溃，验证恢复逻辑和尾行截断处理。
