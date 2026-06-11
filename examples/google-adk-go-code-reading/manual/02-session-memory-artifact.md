# 第二部分：Session、Memory 与 Artifact 状态服务

## 1. 面临的问题是什么：会话、artifact、memory 在 agent runtime 中分别承担什么状态问题？

### 1.1 Session — 多轮对话的线性历史

Session 是 agent runtime 的**核心状态容器**，承担一个用户在一个会话线程中的所有交互历史。它不是简单的聊天记录存储，而是一个带有**分层作用域状态**的会话模型。

**三个作用域 (scoping)：**

| 作用域 | 前缀 | 共享范围 | 定义位置 |
|--------|------|----------|----------|
| App State | `app:` | 应用级别、跨所有用户和会话 | `session/session.go:164-167` |
| User State | `user:` | 用户级别、跨该用户的所有会话 | `session/session.go:173-176` |
| Temp State | `temp:` | 单次 invocation（不持久化） | `session/session.go:168-172` |
| Session State | `(无前缀)` | 仅该会话内可见 | `session/session.go:48-62` |

**Session 数据结构** (`session/session.go:32-46`):
```go
type Session interface {
    ID() string
    AppName() string
    UserID() string
    State() State          // key-value store with scoping
    Events() Events        // ordered list of Event
    LastUpdateTime() time.Time
}
```

**Event** (`session/session.go:92-118`) 是会话的基本单元，嵌入 `model.LLMResponse`（来自 genai 的内容），附带：
- `InvocationID`: 哪次调用产生的
- `Branch`: agent 树的路径，用于并行 agent 的对话隔离
- `Author`: 事件作者
- `Actions.EventActions`: **StateDelta**（增量状态变更）、**ArtifactDelta**（artifact 版本追踪）、`RequestedToolConfirmations`、`TransferToAgent`、`Escalate`

**问题核心**：Session 需要解决跨 invocation 的状态一致性。每次 invocation 会产生多个 event，每个 event 携带 `StateDelta`，这些 delta 需要在恰当的 scope 下被合并，且 `temp:` 前缀的 delta 必须在持久化前被清理。

### 1.2 Artifact — 工具产生的大文件产物

Artifact 是 agent 调用工具时产生的**版本化文件**，例如代码生成、图片生成、文档导出等。它承担以下状态问题：

- **版本管理**：同一个文件名可以有多个版本（自动递增 int64）
- **作用域分离**：普通文件在 session 级别，`user:` 前缀的文件在 user 级别（跨 session 可见）
- **存储与检索**：支持 Save / Load / Delete / List / Versions / GetArtifactVersion 六种操作

**Artifact Service 接口** (`artifact/service.go:31-47`):
```go
type Service interface {
    Save(ctx, *SaveRequest) (*SaveResponse, error)
    Load(ctx, *LoadRequest) (*LoadResponse, error)
    Delete(ctx, *DeleteRequest) error
    List(ctx, *ListRequest) (*ListResponse, error)
    Versions(ctx, *VersionsRequest) (*VersionsResponse, error)
    GetArtifactVersion(ctx, *GetArtifactVersionRequest) (*GetArtifactVersionResponse, error)
}
```

**问题核心**：Artifact 需要处理大文件的存储（binary/text），版本号的连续性（无事务保证的 blob store 存在竞态），以及 user-scoped 文件的跨 session 命名空间映射。

### 1.3 Memory — 跨 session 的长期知识

Memory 是 agent runtime 中的**长期记忆层**，从 session 中提取事件内容，建立可搜索的索引，支持**语义搜索**以在后续会话中检索相关知识。

**Memory Service 接口** (`memory/service.go:31-39`):
```go
type Service interface {
    AddSessionToMemory(ctx, session.Session) error
    SearchMemory(ctx, *SearchRequest) (*SearchResponse, error)
}
```

**问题核心**：Memory 需要解决 session 事件到 memory entry 的**提取策略**（全量 vs 增量），跨 session 的知识检索，以及不同 user/app 之间的**隔离性**。

---

## 2. 为什么这是问题：多轮对话、工具产物、长期记忆、并发/服务边界为什么容易出错？

### 2.1 Session 中的并发写入问题

当一个 session 被多个 goroutine 并发操作时，**AppendEvent** 是核心的风险点：

- **inMemoryService** (`session/inmemory.go:197-254`): 使用 `sync.RWMutex` 保护，但先获取 `curSession.(*session)` 的类型断言和 `s.mu.Lock()` 不是原子的——session 自身也有 `sync.RWMutex`（`session` struct 在 `inmemory.go:308-313`），AppendEvent 流程中会同时获取 outer service lock 和 inner session lock
- **databaseService** (`session/database/service.go:319-354`): 使用 `gorm.DB.Transaction` 保护数据库操作，但有**乐观锁**机制——比较 `storageUpdateTime` 与 `sessionUpdateTime` 来检测 stale session（`service.go:374-382`）
- **vertexAiService** (`session/vertexai/vertexai.go:129-146`): 先将 event 写入远程 VertexAI Reasoning Engine，再本地 append，远程失败会导致不一致

**死锁测试** (`session/inmemory_test.go:81-115`): `TestInMemorySession_AppendEvent_Deadlock` 专门测试 AppendEvent 是否会导致双锁死锁。

### 2.2 State 作用域的分层合并

State 的 key 前缀系统 (`app:`, `user:`, `temp:`) 要求在以下时刻进行**拆分与合并**：

1. **Create/AppendEvent 时拆分**: `sessionutils.ExtractStateDeltas()` (`internal/sessionutils/utils.go:31-54`) 将 delta map 按前缀拆为 app/user/session 三部分
2. **Get 时合并**: `sessionutils.MergeStates()` (`internal/sessionutils/utils.go:58-74`) 将三个分立的 map 用前缀拼回一个 map 返回给客户端
3. **temp 前缀清理**: `trimTempDeltaState()` （在 `inmemory.go:429-446`, `database/session.go:152-169`, `vertexai/session.go:152-169` 三处重复实现）

**重复代码风险**: `trimTempDeltaState` 和 `updateSessionState` 在三个 package 中重复实现（inmemory.go, database/session.go, vertexai/session.go），违反了 DRY 原则。代码注释中也提到 `// TODO localSession is identical to session.session. Move to sessioninternal`（`database/session.go:28`）。

### 2.3 Artifact 的版本号竞态

**GCS 实现的竞态** (`artifact/gcsartifact/service.go:102-113`):
```go
// TODO race condition, could use mutex but it's a remote resource so the issue would still occurs
// with multiple consumers, and gcs does not have transactions spanning several operations
```
代码明确承认：当多个 consumer 同时 Save 同一文件时，`versions()` + 计算 `nextVersion` + 写入不是原子的。GCS 不支持跨对象的事务，因此版本号可能重复或乱序。

**In-memory 实现**使用 `sync.RWMutex` 保护，没有此问题。

### 2.4 Memory 的跨 user 隔离

Memory 的核心安全属性是：**不同 user 的 memory 不能互查**。这通过 `(appName, userID)` 作为 key 实现（`memory/inmemory.go:36-38`, `key struct {appName, userID string}`），测试明确验证了这种隔离（`memory/inmemory_test.go:92-131`，"no leakage for different appName" / "no leakage for different user"）。

### 2.5 服务边界：in-memory → database → cloud

三种 session service 实现有不同的持久化保证：

| 实现 | 位置 | 持久化 | 并发模型 | 可扩展性 |
|------|------|--------|---------|---------|
| InMemory | `session/inmemory.go` | 无 | sync.RWMutex | 单进程 |
| Database (GORM) | `session/database/service.go` | SQL 数据库 | DB 事务 + 乐观锁 | 多进程（共享数据库） |
| VertexAI | `session/vertexai/vertexai.go` | 远程 Reasoning Engine | 远程服务 | 无状态客户端 |

两种 artifact service 实现：
| 实现 | 位置 | 后端 |
|------|------|------|
| InMemory | `artifact/inmemory.go` | 内存 ordered map |
| GCS | `artifact/gcsartifact/service.go` | Google Cloud Storage blob |

两种 memory service 实现：
| 实现 | 位置 | 后端 |
|------|------|------|
| InMemory | `memory/inmemory.go` | 内存 word-intersection stub |
| VertexAI | `memory/vertexai/vertexai.go` | VertexAI MemoryBank API |

---

## 3. 解决思路是什么：service interfaces、in-memory/database/vertex implementations、request validation 如何拆层？

### 3.1 架构分层

```
┌──────────────────────────────────────────────────────────────┐
│  agent runtime (runner/runner.go)                            │
│  - 组装 SessionService + ArtifactService + MemoryService     │
│  - 创建 InvocationContext (内含 agent.Artifacts, agent.Memory)│
└──────────────────────────────────────────────────────────────┘
                              │
        ┌─────────────────────┼─────────────────────┐
        ▼                     ▼                     ▼
┌───────────────┐   ┌────────────────┐   ┌──────────────────┐
│ Session       │   │ Artifact       │   │ Memory           │
│ Service       │   │ Service        │   │ Service          │
│ (interface)   │   │ (interface)    │   │ (interface)      │
├───────────────┤   ├────────────────┤   ├──────────────────┤
│ Create        │   │ Save           │   │ AddSessionToMem  │
│ Get           │   │ Load           │   │ SearchMemory     │
│ List          │   │ Delete         │   │                  │
│ Delete        │   │ List           │   │                  │
│ AppendEvent   │   │ Versions       │   │                  │
│               │   │ GetArtifactVer │   │                  │
└───────┬───────┘   └───────┬────────┘   └────────┬─────────┘
        │                   │                     │
   ┌────┼────┬──────┐  ┌────┼─────┐      ┌───────┼───────┐
   ▼    ▼    ▼      ▼  ▼    ▼     ▼      ▼       ▼       ▼
  InMem DB  VertexAI InMem  GCS        InMemStub VertexAI
                    UserScoped?      (word       (MemoryBank
                    ("user:" prefix)  intersect)  similarity)
```

### 3.2 Request Validation 统一模式

**Artifact Service** 使用统一的 `Validate()` 模式（`artifact/service.go`）：

```go
// SaveRequest, LoadRequest, DeleteRequest, ListRequest,
// VersionsRequest, GetArtifactVersionRequest 都实现 Validate() error
```

验证内容：
- 必填字段检查 (`AppName`, `UserID`, `SessionID`, `FileName`)
- `Part.InlineData` 或 `Part.Text` 至少一个存在（仅 SaveRequest）
- `FileName` 不能包含路径分隔符 `/` 或 `\\`

**Session Service** 的验证是**内联**的（`session/inmemory.go:47-49`），没有统一的 `Validate()` 接口，错误信息直接在各方法中生成。

### 3.3 Session State Delta 的完整生命周期

```
                          user sends message
                                │
                    runner.Run(userID, sessionID, msg)
                                │
              ┌─────────────────┤
              ▼                 ▼
         session.Get()    session.Create() (if auto-create)
              │                 │
              ▼                 ▼
    ┌── StoredSession ──────────────┐
    │                                 │
    │  ┌─ Session State (merged) ───┐│
    │  │ app:xxx  (from appState)  ││
    │  │ user:yyy (from userState) ││
    │  │ zzz      (session state)  ││
    │  └───────────────────────────┘│
    │  ┌─ Events ──────────────────┐│
    │  │ [event1, event2, ...]     ││
    │  └───────────────────────────┘│
    └───────────────────────────────┘
              │
              ▼
    agent.Run(InvocationContext)
        │  ┌─ ctx.Session().State() ─ 读写 state
        │  ┌─ ctx.Artifacts().Save() ─ 写 artifact
        │  ┌─ ctx.Memory().SearchMemory() ─ 读 memory
        │  └─ 产生 events (每步调用模型/工具)
        │
        ▼ 每个 event 携带 StateDelta
    sessionService.AppendEvent(ctx, session, event)
        │
        ├─ trimTempDeltaState(event)     ← 移除 temp: 前缀
        ├─ extractStateDeltas(delta)     ← 拆分 app/user/session
        ├─ update appState (app: prefix)
        ├─ update userState (user: prefix)
        └─ update session state + persist event
```

### 3.4 Artifact 的 User-Scoped 命名空间

文件名以 `user:` 开头时，artifact 从 session 级别提升到 user 级别（`artifact/inmemory.go:150-152`, `artifact/gcsartifact/service.go:71-76`）：

```
普通文件:  {appName}/{userID}/{sessionID}/{fileName}/{version}
User文件:  {appName}/{userID}/user/{fileName}/{version}
```

这使得 `user:avatar.png` 在用户所有 session 间共享。

### 3.5 Memory 与 Session 的集成点

Memory 通过 **状态键** 实现增量更新（`memory/vertexai/vertexai.go:37-41`）：

```go
// StateKeySessionLastUpdateTime: 从 Session State 中读取上次更新时间
// 只将新事件送入 MemoryBank 生成记忆
```

当前 InMemory 实现中，`AddSessionToMemory` 直接遍历所有 events，提取 text 内容并建立 word-intersection stub（`memory/inmemory.go:59-107`）。这不是 Memory 的最终产品语义；原版语义应以 VertexAI MemoryBank / semantic retrieval 为基线。

---

## 4. adk-go 代码怎么落地：关键类型/函数/文件、数据生命周期、测试覆盖、未决风险

### 4.1 关键文件与职责

| 文件 | 职责 |
|------|------|
| `session/service.go` | Session Service 接口 + 请求/响应类型定义 |
| `session/session.go` | Session 模型、Event 模型、State 接口、EventActions、作用域前缀常量 |
| `session/inmemory.go` | In-memory session service 实现（线程安全） |
| `session/database/service.go` | GORM 数据库 session service 实现 |
| `session/database/session.go` | localSession 模型 + trimTempDeltaState + updateSessionState |
| `session/database/storage_session.go` | GORM 数据模型 + JSON 序列化/反序列化 |
| `session/database/gorm_datatypes.go` | stateMap / dynamicJSON GORM 自定义类型 |
| `session/vertexai/vertexai.go` | VertexAI Reasoning Engine session service |
| `session/vertexai/session.go` | VertexAI 的 localSession + 工具函数 |
| `session/vertexai/vertexai_client.go` | VertexAI API 客户端 |
| `session/session_test/service_suite.go` | Session 通用测试套件（跨实现复用） |
| `artifact/service.go` | Artifact Service 接口 + 6 种请求 Validate() |
| `artifact/inmemory.go` | In-memory artifact 实现 (ordered map + 版本控制) |
| `artifact/gcsartifact/service.go` | GCS artifact 实现 |
| `artifact/gcsartifact/gcs_client.go` | GCS 客户端接口抽象 |
| `memory/service.go` | Memory Service 接口 |
| `memory/inmemory.go` | In-memory word-intersection stub；和原版语义检索有差距 |
| `memory/vertexai/vertexai.go` | VertexAI MemoryBank 实现 |
| `memory/vertexai/vertexai_client.go` | VertexAI MemoryBank API 客户端 |
| `internal/sessionutils/utils.go` | ExtractStateDeltas / MergeStates 公共工具 |
| `internal/memory/memory.go` | agent.Memory 接口的包装实现 |
| `internal/artifact/artifacts.go` | agent.Artifacts 接口的包装实现 |
| `internal/context/invocation_context.go` | InvocationContext 的完整实现 |
| `internal/context/callback_context.go` | CallbackContext 工厂函数 |
| `internal/context/readonly_context.go` | ReadonlyContext 实现 |
| `agent/context.go` | InvocationContext / CallbackContext / ReadonlyContext 接口定义 |
| `agent/agent.go` | Artifacts / Memory 接口定义 |
| `runner/runner.go` | Runner 组装所有服务 + 运行主循环 |

### 4.2 数据生命周期

```
Session 生命周期:
  Create(AppName, UserID, [SessionID], [State]) → Session
  ├─ Get(AppName, UserID, SessionID, [filters]) → Session + Events
  ├─ AppendEvent(Session, Event) → 持久化事件 + 更新 scoped state
  ├─ List(AppName, [UserID]) → []Session
  └─ Delete(AppName, UserID, SessionID) → 删除

Artifact 生命周期:
  Save(AppName, UserID, SessionID, FileName, Part) → Version(int64)
  ├─ Load(AppName, UserID, SessionID, FileName, [Version]) → Part
  ├─ List(AppName, UserID, SessionID) → []FileName
  ├─ Versions(AppName, UserID, SessionID, FileName) → []Version
  ├─ GetArtifactVersion(...) → ArtifactVersion
  └─ Delete(AppName, UserID, SessionID, FileName, [Version]) → 删除

Memory 生命周期:
  无显式创建。由 agent runner 在调用结束后隐式触发。
  AddSessionToMemory(Session) → 存储
  SearchMemory(AppName, UserID, Query) → []Entry
```

### 4.3 测试覆盖分析

**Session 测试**:
- `session/session_test/service_suite.go` (`session/session_test/service_suite.go:1-743`): 通用测试套件，测试 Create/Get/List/Delete/AppendEvent/StateManagement 的完整 CRUD + 状态作用域 + 事件过滤
- `session/inmemory_test.go`: InMemory 实现 + 并发创建测试 + AppendEvent 死锁测试
- `session/database/service_test.go`: 使用 SQLite in-memory 运行通用测试套件
- `session/vertexai/vertexai_test.go` + testdata/*.replay: 使用 httprr recording/replay 的集成测试

**Artifact 测试**:
- `internal/artifact/tests/service_suite.go` (`internal/artifact/tests/service_suite.go:1-411`): 通用测试套件，测试 Save/Load/Delete/List/Versions 完整流程 + user-scoped artifact + 空状态操作
- `artifact/inmemory_test.go`: 对 InMemoryService 运行通用测试
- `artifact/request_validation_test.go`: 全部 6 种请求的 Validate 边界测试（必填字段 + 路径分隔符）
- `artifact/artifact_key_test.go`: artifactKey 序列化往返测试

**Memory 测试**:
- `internal/memory/memory_test.go`: 测试 AddSessionToMemory + SearchMemory stub 调用链 + 多用户隔离
- `memory/inmemory_test.go`: 测试不同 appName/userID 的隔离、空存储查询

### 4.4 未决风险

1. **重复代码**：`trimTempDeltaState`、`updateSessionState`、`localSession` 在 `inmemory.go`、`database/session.go`、`vertexai/session.go` 中三次重复实现。代码注释 (`database/session.go:28`) 指出需要 `sessioninternal` 包统一。

2. **GCS Artifact 竞态**：`gcsartifact/service.go:102-113` 的 TODO 标记确认 Save 操作存在版本号竞态。多客户端并发操作同一文件名可能产生重复版本号。

3. **Database Service 的乐观锁精度**：`service.go:374-382` 使用 `UnixMicro()` 微秒级精度比较时间。如果两个 AppendEvent 在同一微秒内到达，后到的会因 `storageUpdateTime == sessionUpdateTime` 而**不会**报 stale，可能覆盖先前写入（需要 `>` 而非 `>`）。

4. **Session 和 Event 的对象一致性问题**：`AppendEvent` 方法同时修改 session 对象的状态和 service 存储。如果其中一个失败（如 vertexai remote call 成功但本地 append 失败），会导致状态不一致。目前没有分布式事务或补偿机制。

5. **Memory 的 InMemory stub 差距**：InMemory 实现只是 word-intersection stub（`memory/inmemory.go:143-160`），不代表原版 Memory 的语义检索能力。VertexAI 实现使用 MemoryBank 的相似性搜索，教学和生产语义应以这一类后端为基线。

6. **同类型 Session 的类型断言脆弱**：`AppendEvent` 中使用 `curSession.(*session)` / `curSession.(*localSession)` 类型断言（如 `session/inmemory.go:208`），这要求调用者必须传递对应实现创建的具体类型，而不是任意的 `session.Session` 实现。

7. **Partial Event 的静默丢弃**：`event.Partial == true` 时，`AppendEvent` 静默 return nil 而不持久化（如 `session/inmemory.go:204-206`、`database/service.go:327-329`）。如果调用方依赖 AppendEvent 的持久化结果但未检查，可能导致状态丢失。

8. **VertexAI Session Service 不支持用户提供 SessionID**：`session/vertexai/vertexai.go:59-61` 明确拒绝 `req.SessionID != ""`，这限制了客户端指定会话 ID 的能力。

---

## 5. 状态生命周期图（文本形式）

### 5.1 Session 状态作用域
```
       ┌─────────────────────────────────────────┐
       │            Session State Map              │
       │                                          │
       │  ┌─────────────────────────────────────┐ │
       │  │      app:xxx  (App-Level State)     │ │ ← shared across all users/sessions
       │  │      app:yyy                        │ │
       │  ├─────────────────────────────────────┤ │
       │  │     user:xxx  (User-Level State)    │ │ ← shared across sessions of same user
       │  │     user:yyy                        │ │
       │  ├─────────────────────────────────────┤ │
       │  │     temp:xxx  (Temp State)          │ │ ← only in-memory, NOT persisted ← PURGED before AppendEvent persist
       │  │     temp:yyy                        │ │
       │  ├─────────────────────────────────────┤ │
       │  │     zzz       (Session-Level State) │ │ ← only visible in this session
       │  │     www                             │ │
       │  └─────────────────────────────────────┘ │
       └─────────────────────────────────────────┘

       Flow on AppendEvent:
         1. StateDelta arrives with keys: {app:key, user:key, temp:key, session_key}
         2. ExtractStateDeltas splits into appDelta, userDelta, sessionDelta
            (temp keys are also included in sessionDelta at this point)
         3. trimTempDeltaState removes temp: keys from EVENT's StateDelta
         4. App state is updated (global), user state is updated (per-user),
            session state is merged into session's state map
```

### 5.2 Invocation 生命周期
```
    ┌────── Invocation ─────────────────────────────────────────────────┐
    │                                                                    │
    │  1. runner.Run(userID, sessionID, userContent)                     │
    │  2. session.Get() ── 获取或创建 session                             │
    │  3. NewInvocationContext(session, artifacts, memory, agent)         │
    │  4. appendMessageToSession ── 记录用户消息 event                     │
    │                                                                     │
    │  ┌─── Agent Call Loop (agentToRun.Run) ──────────────────────────┐ │
    │  │                                                                │ │
    │  │  ┌─ BeforeAgentCallbacks                                       │ │
    │  │  │   → CallbackContext(Artifacts, State) 可读写状态和 artifact   │ │
    │  │  ├─ LLM Step 1: call model → function_call                     │ │
    │  │  │   → CallbackContext(StateDelta, ArtifactDelta)              │ │
    │  │  ├─ Tool Step: execute tool → emit event with StateDelta       │ │
    │  │  │   → ToolContext(SearchMemory, RequestConfirmation)          │ │
    │  │  ├─ LLM Step 2: summarize tool results                         │ │
    │  │  ├─ [transfer to sub-agent] ── 递归进入子 agent                  │ │
    │  │  ├─ AfterAgentCallbacks                                        │ │
    │  │  └─ final response event                                       │ │
    │  └────────────────────────────────────────────────────────────────┘ │
    │                                                                    │
    │  For each event:                                                    │
    │     sessionService.AppendEvent(ctx, session, event)                 │
    │     → trimTempDeltaState → extractStateDeltas → persist             │
    │                                                                    │
    │  5. (optional) memoryService.AddSessionToMemory(ctx, session)      │
    └────────────────────────────────────────────────────────────────────┘
```

### 5.3 Artifact 版本生命周期
```
    Save("file1", part1) → Version 1 ──┐
    Save("file1", part2) → Version 2 ──┤  同一文件多版本
    Save("file1", part3) → Version 3 ──┘

    Load("file1") → Version 3 (latest)
    Load("file1", version=1) → Version 1 (specific)

    Delete("file1", version=3) → 删除 Version 3
    Load("file1") → Version 2 (最新成为 v2)

    Delete("file1") → 删除所有版本
    Load("file1") → ErrNotExist

    List()       → ["file1", "file2", "user:avatar.png"]  (排序后)
    Versions("file1") → [1, 2, 3]
    GetArtifactVersion("file1", version=2) → {Version:2, MimeType, CanonicalURI, CreateTime}

    User-Scoped Files:
      "user:avatar.png" → 存储为 {appName}/{userID}/user/avatar.png/{version}
      跨 session 可见
```

---

## 6. 深入追问（5-10 个）

1. **State delta 合并语义**：当 `maps.Copy` 覆盖已有 key 时，如果 old value 是嵌套 map，是否会产生部分合并问题？当前实现是浅合并（整 key 覆盖），这在 Python ADK 中的行为是否一致？

2. **AppendEvent 的事务边界**：`databaseService.AppendEvent` 先调用 `sess.appendEvent`（修改本地 session），再到 `gorm.DB.Transaction` 中持久化。如果持久化中发生 stale session 错误，`sess.appendEvent` 的效果已经生效且无法回滚。是否有计划将本地 session 修改也纳入事务或增加回滚机制？

3. **Artifact 的 `user:` 前缀约定**：`fileHasUserNamespace` 在 `inmemory.go` 和 `gcsartifact/service.go` 中分别实现。是否应该将此判断逻辑移至 `artifact/service.go` 公共层，避免实现间行为不一致？

4. **MemoryService 的本地 stub 替换**：InMemory 当前只是 word-intersection stub，VertexAI MemoryBank 支持相似性搜索。是否有计划为本地实现补齐向量/embedding 或 MemoryBank-compatible 检索能力？

5. **Database Session Service 的乐观并发**：`storageUpdateTime > sessionUpdateTime` 使用 `>` 而非 `>=`。这意味着同一微秒内的两次 AppendEvent 不会被检测为 stale。这是有意设计的宽松策略，还是微秒精度不足的问题？

6. **Runner 中的 Service 可选性**：`artifactService` 和 `memoryService` 在 Runner Config 中是可选的（`runner/runner.go:50-53`），但 SessionService 是必选的。当 artifact/memory service 为 nil 时，对应的 `agent.Artifacts` / `agent.Memory` 也设为 nil，这意味着 agent 代码需要 nil 检查。是否有计划提供 no-op 实现作为默认值？

7. **VertexAI Session Service 的 AppendEvent 不提取 StateDelta**：`session/vertexai/vertexai.go:129-146` 中的 AppendEvent 直接调用 `s.client.appendEvent` 写入远程，但没有执行 `extractStateDeltas` 拆分 app/user state。这意味着 app/user 级别的 state 更新在 VertexAI 实现中可能不被正确处理。这是设计选择还是待修复的 bug？

8. **GCS Artifact Service 的并行删除**：`gcsartifact/service.go:165-181` 使用 `errgroup` 并行删除多个版本，但如果其中一个版本删除失败，已删除的版本无法回滚。GCS 没有事务支持，这是否是可接受的 trade-off？

9. **Session test suite 的 replay 机制**：`session/vertexai/testdata/*.replay` 使用 `internal/httprr` 的 HTTP recording/replay 模式。这种测试方法在 artifact 和 memory 的 GCS/VertexAI 实现中是否也可以推广使用？

10. **Memory 与 Session Service 的耦合**：`AddSessionToMemory` 接收 `session.Session` 接口，但通过 `Events().All()` 遍历事件时，调用方不知道 events 是否已经被过滤（如 `NumRecentEvents` 或 `After`）。如果 session 被 Get 时过滤了 events，送入 memory 的 events 可能不完整。这是否是预期的行为？
