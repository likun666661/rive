# Maka Storage / Persistence 粗读报告

> 阅读基线: `335220a`
> 仓库路径: `/Users/likun/Desktop/workspace-for-maka/maka`
> 包路径: `packages/storage/`

---

## 1. problem — Maka 需要保存哪些状态，为什么不能只存在内存里

Maka 是一个持久化桌面 AI 编程助手，以下状态必须落盘:

| 状态类别 | 存储目标 | 失持久化后果 |
|---------|---------|------------|
| **Session 会话** | 会话头（header）+ 全部消息（messages）JSONL | 用户关闭窗口后丢失全部对话历史、turn 状态、分支 lineage |
| **LLM Connection 元数据** | provider slug / 模型列表 / 测试状态 / 默认连接 | 每次启动需重新配置 provider；模型发现缓存丢失导致冷启动延迟 |
| **Settings 设置** | 外观、网络代理、通知通道 (Telegram/Bot)、onboarding 里程碑 | 用户偏好丢失；bot 通知通道 token 丢失 |
| **Artifact 产物** | 文件内容 + 元数据 (metadata.jsonl) | 工具生成的报告/图片/HTML 无法跨 session 访问或导出 |
| **Telemetry 用量记录** | LLM 调用记录 / tool 调用记录 / pricing 覆盖 | 用量统计丢失；用户无法追踪 token 消耗和成本 |
| **Plan Reminder 计划提醒** | 定时提醒的日程/投递配置/运行历史 | 重启后所有定时提醒失效 |

**不能只存内存的原因**:
1. **窗口关闭即消失** — Electron 主进程退出后内存状态全部丢失
2. **Renderer 进程隔离** — renderer 不能直接写文件，所有持久化发生在 main 进程
3. **多 workspace 隔离** — 每个 workspace 目录有独立的 `settings.json` / `sessions/` 等
4. **审计/回溯** — 用量统计和对话历史需要跨启动查询

---

## 2. why_hard — 一致性与隐私难点

### 2.1 Session/Event 一致性
- 会话文件是 **JSONL 格式** — 第 1 行 header，后续行 append-only 消息。header 更新走 **读-改-写（atomic temp + rename）**，消息走 **appendFile**。
- 并发写同一 session 的风险通过 **per-session write queue** (`withQueue`) 串行化，但 **跨 session 之间无全局锁**。
- JSONL 的 append 不保证 crash-safe：如果在 `appendFile` 中途进程崩溃，最后一行可能被截断。当前代码**无校验和/事务日志**。

### 2.2 Credential Material 与 Metadata 的分离
- **`LlmConnection` 不包含真实 secret**。API key / OAuth token 由 `packages/desktop/src/main/credential-store.ts`（不在本次范围内）通过系统密钥链（macOS Keychain / Windows Credential Manager）管理。
- `connection-store.ts` 中的 `apiKey` 出现在 `UpdateConnectionInput.patch` 中，但仅用于触发 **缓存失效逻辑**（如 `apiKey` 变更 → 清除 `lastTestStatus` 和 `models`），**不持久化 secret 本身**。
- **需要 `desktop-main-ipc` 节点交叉确认**: 加密/安全逻辑在 desktop main 层，storage 层只持久化 metadata。

### 2.3 Settings 中的敏感字段
- `settings.json` 可能包含 Telegram bot token、HTTP 代理认证信息（`username` / `password`）。这些字段**以明文 JSON 存在 workspace 目录下**，依赖文件系统权限保护。
- `testNetworkProxy()` 只在内存中校验配置格式并返回结果，**从不写回 settings.json**，不泄露已有密钥。

### 2.4 Artifact 路径遍历防护
- 产物文件存在 `workspaceRoot/artifacts/` 下，通过 `relativePath` 索引。`resolveArtifactPath()` 做了多层路径校验：
  - `isSafeRelativeArtifactPath()` 拒绝绝对路径、`..`、空段、URL scheme、null 字节
  - `realpath()` 解析符号链接后，`isInsideOrSamePath()` 确保解析后的真实路径仍在 `artifactRoot` 内
- 符号链接逃逸测试 (`artifact-store.test.ts:141`) 验证了路径守卫的有效性

### 2.5 Plan Reminder 的时序正确性
- 提醒依赖 `nextRunAt` 时间戳触发，持久化后在主进程定时扫描 `listDue()`。跨时区、夏令时、系统时间跳变的正确性依赖外部调度器，store 本身**不做时钟校正**。

---

## 3. design_approach — 各 Store 的职责边界、文件布局、读写/迁移/容错策略

### 3.1 整体架构

```
packages/storage/src/
├── index.ts                 # 统一导出
├── session-store.ts         # 会话 CRUD + JSONL 读写
├── connection-store.ts      # LLM 连接元数据 CRUD
├── settings-store.ts        # 全局设置 + 用量统计 + onboarding
├── artifact-store.ts        # 产物文件 + 元数据
├── telemetry-repo.ts        # 精粒度 LLM/tool 用量记录
├── plan-reminder-store.ts   # 定时提醒
└── __tests__/               # 7 个测试文件
```

### 3.2 文件布局 (workspaceRoot 下)

```
workspaceRoot/
├── settings.json                  # AppSettings (单文件 JSON)
├── llm-connections.json           # LlmConnection[] + defaultSlug
├── telemetry.json                 # 用量原始记录 + pricing
├── plan-reminders.json            # PlanReminder[]
├── sessions/
│   └── {session-id}/
│       └── session.jsonl          # 第1行 header JSON, 后续 append-only message JSON
└── artifacts/
    ├── metadata.jsonl             # 产物元数据行
    └── {session-id}/
        └── {artifactId}-{name}    # 实际文件内容
```

### 3.3 各 Store 职责边界

| Store | 职责 | 不负责 |
|-------|------|--------|
| `SessionStore` | Session 生命周期管理、消息 append、header 迁移 | 不负责加密、不负责 token 用量统计 |
| `ConnectionStore` | Provider 连接元数据 (slug/model/test status/default) | 不存储 secret (API key/OAuth token) |
| `SettingsStore` | 全局设置、网络代理校验、onboarding 里程碑、聚合用量统计 | 不存储精粒度 telemetry 记录 |
| `ArtifactStore` | 文件写入、元数据管理、路径安全、文本/二进制预览 | 不负责外部导出 (由 renderer 触发 IPC) |
| `TelemetryRepo` | 精粒度 LLM 调用/tool 调用记录、分桶聚合、pricing 覆盖 | 不参与实时计费，仅记录 |
| `PlanReminderStore` | 提醒 CRUD、触发记录、调度状态机 | 不负责实际通知投递 (由 scheduler 模块完成) |

### 3.4 读写策略

- **单文件 JSON (settings / connections / telemetry / reminders)**: 完整读 → 内存修改 → **atomic write**（write temp → rename）。全局 `queue: Promise<void>` 串行化所有写操作。
- **Session JSONL**: header 走 `writeAtomic`（读-改-写，temp+rename）；消息走 `appendFile`（追加写）；per-session `writeQueues` 确保同一 session 的写操作串行。
- **Artifact 文件**: 直接 `writeFile` 写文件内容；metadata `metadata.jsonl` 走 atomic write（全量重写）。

### 3.5 迁移策略

- `migrateHeader()` (`session-store.ts:272`) 处理旧 schema 升级：
  - `backend: 'claude'` → `'ai-sdk'`
  - `backend: 'pi'` / `'pi-agent'` → `'pi-agent'`
  - 缺失 `permissionMode` → `'ask'`
  - 缺失 `model` → `'default'`
  - 缺失 `status` → 根据 `isArchived` 推导
- `migrateConnectionV1ToV2()` (`@maka/core/llm-connections`) 处理连接记录从 v1 到 v2 的升级
- `normalizeFile()` (`telemetry-repo.ts:220`) 处理损坏的 telemetry JSON

### 3.6 容错策略

- **缺失文件**: `ENOENT` 时返回空结构（`[]` 或 `empty*()`）
- **损坏行**: session list 跳过无法解析的目录（`try/catch` with ignore）；reminder 通过 `normalizePersistedPlanReminder` 逐个校验并过滤
- **无效输入**: 所有 store 的 write path 都会校验 slug / ID 格式 / path 安全
- **Write queue 容错**: `withQueue` 内部 `.catch(() => {})` 保持链存活，不会因为一次失败阻塞后续写入

---

## 4. code_walkthrough — 关键 Store 文件、核心函数、数据格式、异常处理

### 4.1 SessionStore (`session-store.ts`)

**核心类**: `FileSessionStore` (line 37)

**关键函数**:

| 函数 | 行号 | 功能 |
|------|------|------|
| `create()` | 45 | 生成 UUID，调用 `normalizeUserSessionName` 校验/清理会话名，写入 header |
| `readHeader()` | 128 | 读取 header，若 `connectionLocked=false` 且有 user message 则自动锁定 |
| `list()` | 98 | 遍历 `sessions/` 下所有目录，读取并过滤，按 `lastMessageAt DESC, id ASC` 排序 |
| `appendMessages()` | 152 | JSON stringify 每条消息并以 `\n` 拼接，追加到 `session.jsonl` |
| `updateHeader()` | 161 | 读取全部消息 → 替换 header 首行 → `writeAtomic` 写回 |
| `readFilePartsUnlocked()` | 224 | 读取 JSONL，第 1 行经 `migrateHeader()` 迁移，后续行解析为 `StoredMessage` |
| `writeAtomic()` | 233 | `writeFile(temp) → rename(temp, path)` 保证原子性 |
| `withQueue()` | 240 | per-session 写操作的串行化队列 |

**数据格式**: JSONL — 每行一个 JSON 对象
- 第 1 行: `SessionHeader` (序列化为 JSON)
- 后续行: `StoredMessage` 联合类型 (`user` | `assistant` | `tool_call` | `tool_result` | `permission_decision` | `token_usage` | `turn_state` | `system_note`)

**异常处理**:
- `assertSafeSessionId()`: 拒绝 `[^A-Za-z0-9_-]` 和长度 >128 的 ID
- `migrateHeader()`: 对未知 `backend` 降级为 `'fake'`
- `list()`: 孤立/损坏的 session 目录静默跳过

### 4.2 ConnectionStore (`connection-store.ts`)

**核心类**: `FileConnectionStore` (line 35)

**关键函数**:

| 函数 | 行号 | 功能 |
|------|------|------|
| `create()` | 51 | slug 唯一性校验 → 写入 `LlmConnection`；自动设为首个 default |
| `update()` | 82 | 智能缓存失效：`apiKey` / `baseUrl` 变更 → 清除 `models/modelSource/modelsFetchedAt` 和 `lastTestStatus` |
| `save()` | 140 | upsert 语义：slug 存在则覆盖，否则 push |
| `setDefault()` | 175 | 禁止将 disabled 连接设为默认 |
| `readUnlocked()` | 192 | `ENOENT` 返回空文件；每条 connection 经 `migrateConnectionV1ToV2` |

**数据格式**: 单文件 `llm-connections.json`
```json
{
  "defaultSlug": "anthropic-main",
  "connections": [LlmConnection, ...]
}
```

**缓存失效规则** (update 方法内的派生逻辑):
- `apiKey` / `baseUrl` 变更 → 清除 `models`, `modelSource`, `modelsFetchedAt`
- `apiKey` / `baseUrl` / `defaultModel` / `models` 变更 → 清除 `lastTestStatus`, `lastTestAt`, `lastTestMessage`
- `models` / `modelSource` / `modelsFetchedAt` 显式传入时 → 保留而非清除

### 4.3 SettingsStore (`settings-store.ts`)

**核心类**: `FileSettingsStore` (line 57)

**关键函数**:

| 函数 | 行号 | 功能 |
|------|------|------|
| `get()` | 65 | `normalizeSettings()` 做 schema 迁移和默认值填充 |
| `update()` | 76 | `mergeSettings(current, patch)` → atomic write |
| `usageStats()` | 164 | 读取所有 sessions 的 JSONL → 提取 `token_usage` / `tool_call` / `tool_result` 消息 → 聚合 `Summary` + `byProvider` + `byModel` + `byTool` |
| `upsertOnboardingMilestone()` | 87 | 时间戳由 store 生成（renderer 不可篡改），经 `sanitizeOnboardingMilestones` 校验去重 |
| `testNetworkProxy()` | 142 | 纯校验函数：检查 host/port/auth 配置合法性，不实际发起网络请求 |

**数据格式**: `settings.json` — `AppSettings` 对象（含 `appearance`, `network.proxy`, `notifications.channels`, `onboarding.milestones` 等）

### 4.4 ArtifactStore (`artifact-store.ts`)

**核心类**: `FileArtifactStore` (line 43)

**关键函数**:

| 函数 | 行号 | 功能 |
|------|------|------|
| `create()` | 55 | `sanitizeArtifactName()` 清洗文件名 → 写出文件 → `append()` 写元数据 |
| `readText()` | 111 | `prepareRead()` 校验存在性/大小/路径安全 → `readFile(utf8)` |
| `readBinary()` | 121 | 同上 → `readFile` → `sniffAllowedBinaryMime()` 只允许 PNG/JPEG/GIF/WEBP/PDF/SVG |
| `prepareRead()` | 144 | 核心守卫：get record → 检查 status → `resolveArtifactPath()` 防逃逸 → `stat` 检查大小 |
| `resolveArtifactPath()` | 195 | `isSafeRelativeArtifactPath()` + `realpath()` + `isInsideOrSamePath()` 三重防护 |
| `sanitizeArtifactName()` | 225 | 移除 `\ / : * ? " < > | \0`，去除首部 `.` 和 `-`，截断至 120 字符 |

**数据格式**:
- `metadata.jsonl`: 每行一个 `ArtifactRecord` JSON
- 文件: `{sessionId}/{artifactId}-{name}` 下的原始字节

### 4.5 TelemetryRepo (`telemetry-repo.ts`)

**核心类**: `FileTelemetryRepo` (line 57)

**关键函数**:

| 函数 | 行号 | 功能 |
|------|------|------|
| `insertLlmCall()` | 79 | 内存 upsert（按 id），fire-and-forget 异步写盘 |
| `logs()` | 131 | 按 range + connectionSlug + providerId + modelId + status 过滤 + DESC 排序 + 分页 |
| `buckets()` | 115 | 按 provider/model/day/hour/tool 分组聚合 |
| `summary()` | 89 | 计算 totalRequests / totalCost / tokens / cache hit / error 计数 |
| `latestLlmRuntimeProbe()` | 157 | 获取某 connection + model 的最新一次运行记录 |
| `upsertPricing()` | 169 | 自定义模型定价的增/改 |

**写入模式**: `insert*` 方法是 fire-and-forget — 不 await 写入结果，仅 enqueue。这意味着在进程退出前必须有 `await flushWrites()` 或等待 queue drain。

### 4.6 PlanReminderStore (`plan-reminder-store.ts`)

**核心类**: `FilePlanReminderStore` (line 35)

**关键函数**:

| 函数 | 行号 | 功能 |
|------|------|------|
| `create()` | 50 | `normalizeCreatePlanReminderInput` 校验 → 生成 `PlanReminder` |
| `update()` | 73 | 修改 title/note/schedule/delivery；更新 `nextRunAt` |
| `markTriggered()` | 190 | 写入 `PlanReminderRunRecord`，通过 `nextPlanReminderStateAfterTrigger` 更新状态机 |
| `listDue()` | 186 | `isPlanReminderDue(reminder, now)` 过滤 |
| `snooze()` | 130 | 延后 `nextRunAt`，限制最大 7 天 |
| `normalizePersistedPlanReminder()` | 278 | 反序列化时的严格校验：id/title/note/schedule 类型检查 + delivery 规范化 + runs 过滤 |

---

## 5. flows — 关键数据链路

### 5.1 创建 Session

```
renderer IPC → main handler → SessionStore.create(input)
  │
  ├─ normalizeUserSessionName(input.name)
  │   ├─ undefined → "New Chat"
  │   ├─ "" / whitespace-only → REJECT
  │   └─ valid string → strip control chars / bidi / zero-width → NFC → cap 80
  │
  ├─ 构建 SessionHeader { id: UUID, createdAt, lastUsedAt, status: "active", schemaVersion: 1, ... }
  │
  └─ withQueue(sessionId)
      └─ mkdir(sessionRoot/sessionId/) + writeFile(session.jsonl, JSON.stringify(header)+'\n')
```

### 5.2 Append Message (事件写入)

```
main handler → SessionStore.appendMessages(sessionId, messages[])
  │
  ├─ assertSafeSessionId(sessionId)
  │
  └─ withQueue(sessionId)
      └─ mkdir (确保目录存在)
      └─ messages.map(JSON.stringify).join('\n') + '\n'
      └─ appendFile(session.jsonl, payload, 'utf8')
          │
          └─ JSONL 追加: 每个 StoredMessage 一行，类型包括:
              user | assistant | tool_call | tool_result |
              token_usage | permission_decision | turn_state | system_note
```

### 5.3 读取消息 (列出 Turns)

```
main handler → SessionStore.listTurns(sessionId)
  │
  ├─ readMessages(sessionId)
  │   ├─ readFile(session.jsonl, 'utf8')
  │   ├─ split('\n').filter(nonEmpty)
  │   ├─ lines[0] → migrateHeader(JSON.parse) → SessionHeader
  │   ├─ lines[1:] → JSON.parse → StoredMessage[]
  │   └─ 若 !header.connectionLocked && 有 user message → 自动 lock
  │
  └─ deriveTurnRecords(messages)
      └─ 按 turnId 分组 → 取每个 turn 的最后一个 turn_state →
         构建 TurnRecord { turnId, status, parentTurnId?, ... }
```

### 5.4 保存 Provider Connection

```
main handler → ConnectionStore.create({ slug, name, providerType, defaultModel })
  │
  ├─ validateSlug(slug)  // 格式校验
  │
  └─ withQueue
      ├─ readUnlocked() → ENOENT? → emptyFile
      ├─ 检查 slug 唯一性
      ├─ PROVIDER_DEFAULTS[providerType] 获取 provider 默认值
      ├─ 构建 LlmConnection（不含 apiKey/oauthToken）
      ├─ connections.push(next)
      ├─ 若 defaultSlug 为空 → 设为此 connection
      └─ writeAtomic(llm-connections.json)
```

**注意**: API key/token 的保存路径为 `desktop/main/credential-store.ts → system keychain`，不在 `connection-store.ts` 的范围内。

### 5.5 Artifact 写入

```
main handler → ArtifactStore.create({ sessionId, turnId, name, kind, content })
  │
  ├─ sanitizeArtifactName(name)
  │   └─ 移除非法文件名字符 → 首部去 '.' '-' → trim → cap 120
  │
  ├─ relativePath = "{sessionId}/{uuid}-{name}"
  ├─ mkdir(dirname(target))
  ├─ writeFile(target, content)
  ├─ stat(target) 获取 sizeBytes
  │
  └─ append(ArtifactRecord { id, sessionId, turnId, name, kind, relativePath, sizeBytes, status: "live" })
      ├─ load() metadata.jsonl (首次)
      ├─ enqueue
      │   └─ upsertById(records, record)
      │   └─ writeMetadataUnlocked → writeAtomic(metadata.jsonl)
      └─ return record
```

### 5.6 Telemetry 记录

```
main handler (LLM 调用完成) → TelemetryRepo.insertLlmCall(record)
  │
  ├─ this.file.usageRecords = upsertById(records, record)  // 内存操作
  └─ this.enqueueWrite()  // fire-and-forget 异步写盘
      └─ writeAtomic(telemetry.json)
          └─ JSON.stringify({ usageRecords, toolInvocations, pricingOverrides })
```

**注意**: `insertLlmCall` 和 `insertToolInvocation` 用 fire-and-forget 模式，不等待写盘完成。测试中通过 `flushWrites()` (setTimeout 20ms) 等待写入完成。

---

## 6. tests — 现有测试覆盖与缺口

### 6.1 现有测试

| 测试文件 | 行数 | 覆盖内容 |
|---------|------|---------|
| `session-store.test.ts` | 476 | CRUD: create/archive/unarchive/setFlagged/rename/remove；迁移: backend 升级/permissionMode 补全/model 补全/archived status；安全: traversal 拒绝；preview: system_note/tool_call 跳过/emoji 截断/附件回退；turns: turn_state 派生/legacy 推断；名称规范化: control char/bidi/零宽/空串拒绝/cap 80/surrogate pair/branch 派生 |
| `connection-store.test.ts` | 201 | test status 持久化/配置变更后失效/非配置更新保留；model cache 持久化/凭证变更后失效/显示更新保留；disabled connection 不能设为 default |
| `artifact-store.test.ts` | 179 | CRUD: create/list/get/readText/readBinary；跨实例持久化；soft delete；too_large 限制；MIME sniff 白名单；路径安全: absolute/traversal/URL-like/空路径拒绝；符号链接逃逸防护；名称消毒 |
| `telemetry-repo.test.ts` | 175 | upsert 语义；range/provider/model/status 过滤 + 分页；latestLlmRuntimeProbe；buckets: provider/model/day/hour/tool 聚合；pricing override 持久化与重载 |
| `plan-reminder-store.test.ts` | 291 | CRUD: create/list/listDue；bot delivery 持久化/legacy 回退 local；recurring 触发后保持 active；cron expression 持久化；pause/resume/delete；snooze 延迟；更新 title/schedule/delivery；run history 顺序/清理；完成态拒绝清除；无效输入拒绝 |
| `settings-store-usage.test.ts` | 118 | usageStats: model log + tool log 正确分离；displayName/turnId/provider 正确传播 |
| `settings-store-onboarding.test.ts` | 206 | upsertOnboardingMilestone: 时间戳由 store 生成/status 互斥/无效 ID 拒绝/upsert 覆盖/不扰动其他 milestone/磁盘格式正确/garbage 输入过滤；clearOnboardingMilestone: 单项删除/无效 ID 拒绝 |

### 6.2 明显缺口

| 缺口 | 风险等级 | 说明 |
|------|---------|------|
| **损坏 JSONL 行恢复** | 高 | 当前 `readFilePartsUnlocked` (`session-store.ts:224`) 中的 `JSON.parse(line)` 若某行损坏会**整文件读取失败**，不会跳过损坏行。session list 中仅 `list()` 方法对目录级有 try/catch，但单行损坏会导致整个 session 无法加载。建议增加 per-line try/catch 或在损坏行处插入 sentinel。 |
| **并发写压力测试** | 中 | `withQueue` 串行化同一 session 的写入，但没有测试验证两个并发的 `appendMessages` 是否正确排队。`appendFile` 的 POSIX 原子性未验证（依赖 OS `O_APPEND` 语义）。 |
| **写过程中崩溃恢复** | 高 | `writeAtomic(temp → rename)` 保证了 header/metadata 单文件写入的原子性，但 `appendFile` 无原子性保证。如果 `appendFile` 中途崩溃，JSONL 最后一行可能被截断。没有 WAL 或校验和机制。 |
| **迁移覆盖** | 中 | `migrateHeader` 覆盖了 `backend: 'claude'/'ai-sdk'/'pi'/'pi-agent'/'fake'` + 缺失 `permissionMode`/`model`/`status`，但未覆盖所有旧 `backend` 值组合。`migrateConnectionV1ToV2` 在 core 中但缺乏 storage 层集成测试。 |
| **大文件性能** | 中 | `list()` 和 `usageStats()` 需要读取所有 session 的全部消息，长会话（10k+ 消息）时性能会线性退化。没有分页/索引/增量读取。 |
| **跨版本 schema 兼容** | 中 | `schemaVersion: 1` 字段已存在但未被读取或用于决策。未来 schema 升级只能通过 `try/catch` 和启发式迁移，缺乏版本驱动的迁移策略。 |
| **TelemetryRepo fire-and-forget 数据丢失** | 中 | `insertLlmCall` 和 `insertToolInvocation` 不返回 Promise，进程退出时若 queue 未 drain 会丢失记录。没有 `before-quit` 钩子或优雅关闭逻辑。 |
| **settings.json 中 PII 泄露** | 高 | `settings.json` 可能包含 Telegram bot token、代理密码等。这些字段以明文存储，无加密层。如果用户将 workspace 目录上传或备份到云存储，敏感信息可能泄露。 |
| **Artifact MIME sniff 局限** | 低 | 只支持 PNG/JPEG/GIF/WEBP/PDF/SVG 六种二进制格式，未来新增格式需要修改 `sniffAllowedBinaryMime`。 |

---

## 7. risks — 风险评估

### 7.1 Credential / PII 泄漏

| 风险 | 详情 | 严重度 |
|------|------|--------|
| `settings.json` 明文存储 | Telegram bot token、代理认证信息以明文 JSON 存在 workspace 目录 | **严重** |
| `llm-connections.json` 不含 secret | `apiKey` 仅在 `UpdateConnectionInput` 中传递但不持久化，实际 secret 在系统 keychain；但若 main 进程代码有 bug 将 `patch.apiKey` 写入磁盘则泄漏 | **低**（需代码缺陷） |
| Session JSONL 含 tool args | `ToolCallMessage.args` 可能包含通过工具参数传入的敏感信息（如数据库密码、API key） | **中** |
| **需要 `desktop-main-ipc` 节点交叉确认** | credential 加密/解密逻辑在 desktop main 的 `credential-store.ts` | — |

### 7.2 JSON 文件腐败

| 场景 | 后果 | 防护 |
|------|------|------|
| JSONL 某行损坏 | 整文件读取失败 → session 不可用 | 无（需要 per-line try/catch） |
| `writeAtomic` 中 `rename` 失败 | temp 文件残留，下次写入可能冲突 | temp 文件名含 `process.pid` + `Date.now()` 基本唯一 |
| 磁盘满 | `writeFile` / `appendFile` 抛异常 | 未特殊处理，异常会传播到 IPC handler |
| 两个进程同时写同一 workspace | 无文件锁，可能互相覆盖 | 无防护（假设单进程运行） |

### 7.3 跨版本兼容

| 方面 | 现状 | 风险 |
|------|------|------|
| `schemaVersion: 1` | 已写入但未用于版本决策 | 未来升级需靠试探法 |
| `migrateHeader()` | 覆盖已知旧 backend 值 | 未知 backend → `'fake'`，可能丢失真实后端信息 |
| `migrateConnectionV1ToV2()` | 在 `readUnlocked()` 中调用 | V3 迁移无预留路径 |
| `normalizeSettings()` / `normalizeFile()` | 处理缺失字段/多余字段 | 多余字段被静默丢弃 |

### 7.4 Renderer 可见范围

所有 store 运行在 main 进程，renderer 通过 IPC 访问。但以下路径值得关注：
- `resolveArtifactPath()` 防止 renderer 通过 `../` 读取 workspace 外文件
- `assertSafeSessionId()` 防止 renderer 通过恶意 sessionId 进行路径遍历
- `upsertOnboardingMilestone()` 防止 renderer 伪造时间戳
- 但如果 main 进程通过 IPC 返回 `ArtifactRecord.relativePath` 给 renderer，renderer **不应**直接使用此路径（注释已标注 "never exposed as a filesystem path to renderer code"）

### 7.5 数据目录选择

workspace 目录由调用方在创建 store 时传入 (`createSessionStore(workspaceRoot)`)：
- 不同 workspace 的数据天然隔离
- 数据目录在用户可控的文件系统上，无沙箱保护
- macOS 上默认在 `~/Documents/` 或类似位置，可能被 iCloud 同步（需 `desktop-main-ipc` 确认是否设置 `com.apple.quarantine` 或排除标记）

---

## 8. next_questions — 下一轮 durable-state 精读建议

1. **Credential 层精读**: `packages/desktop/src/main/credential-store.ts` — 确认 API key / OAuth token 的实际存储路径、加密算法、keychain 操作、与 `ConnectionStore` 的交互契约。
2. **IPC Handler 与 Store 的桥接**: `packages/desktop/src/main/ipc/` — 确认哪些 IPC handler 调用哪些 store 方法，错误如何在 IPC 边界传播，renderer 收到的数据类型是否经过裁剪（不含 `relativePath` 等敏感字段）。
3. **Session 损坏恢复机制**: 是否已有 per-line JSON parse 容错，或计划实现 JSONL 修复工具。当前 `readFilePartsUnlocked` 的单点失败风险需要精读确认。
4. **TelemetryRepo 数据生命周期**: 确认 `enqueueWrite` 的 fire-and-forget 模式是否在应用退出时有 `before-quit` 钩子执行 `await flushWrites()`。`usageRecords` 是否有上限/自动清理策略。
5. **Plan Reminder 调度器集成**: `packages/desktop/src/main/plan-reminders.ts` 或类似文件 — 确认定时扫描 `listDue()` 的机制（`setInterval` vs `setTimeout` 动态调度），与 store 之间的事务边界。
6. **跨 workspace 数据共享**: 是否有 "global" 数据目录（如 `~/.maka/`）存储跨 workspace 的 settings/telemetry。当前所有数据都在 workspace 目录下，如果用户切换 workspace，settings 和 telemetry 是否可携带？
7. **Settings 敏感字段加密**: `settings.json` 中的 bot token 和代理密码是否需要迁移到系统 keychain。当前明文存储的风险评估和迁移路径。
8. **Artifact 清理策略**: 当前只有 soft delete（`status: 'deleted'`），没有物理删除或自动清理，磁盘占用会持续增长。是否有 GC 策略或用户手动清理入口？
