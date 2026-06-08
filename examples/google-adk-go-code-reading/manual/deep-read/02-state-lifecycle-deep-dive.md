# Session / Memory / Artifact 状态生命周期精读报告

> 仓库: `google/adk-go` | 基线: `81a63d8feb7d713b1731f0c740d95574eb64dafa` | 深度: implementation

---

## 1. problem

### 1.1 Session 解决什么问题

Session 是 Agent 与用户之间一次"对话线程"的**短期状态容器**。它承载：

- **对话历史** (`Events []*Event`)：用户消息、模型回复、工具调用/响应按时间序排列的完整记录。
- **会话级状态** (`State map[string]any`)：在单次会话生命周期内跨多轮调用共享的键值数据。
- **会话元数据**：`AppName`、`UserID`、`SessionID`、`LastUpdateTime`。

### 1.2 Memory 解决什么问题

Memory 提供**跨 Session 的长期知识检索**。典型场景：

- 用户在多轮会话中透露了偏好（如"我喜欢 Python"），后续新会话的 Agent 需要记起。
- 搜索返回的是 `[]Entry`，每个 Entry 包含 `Content *genai.Content`、`Author`、`Timestamp`、`CustomMetadata`。
- Memory 通过 `AddSessionToMemory` 将 Session 的 Events 灌输到长期存储，再通过 `SearchMemory` 按关键词匹配返回。

### 1.3 Artifact 解决什么问题

Artifact 是 Agent/Tool 在对话过程中**产生或引用的文件**。典型场景：

- Agent 调用代码执行工具生成了一张图表 PNG，需要持久化并返回给用户。
- 支持版本化 (`Version int64`)，每次 Save 自增版本号，Load 可以指定版本或拿最新。
- 支持 user namespace（`user:` 前缀的文件名），使其对所有 session 可见（但仍以 `AppName+UserID` 为边界）。

### 1.4 为什么不能混成一个存储

| 维度 | Session | Memory | Artifact |
|------|---------|--------|----------|
| **生命周期** | 会话创建 → 事件追加 → 会话结束/删除 | 长期存在，跨 Session 累积 | 按文件独立存在，支持版本演进 |
| **数据模型** | 有序事件列表 + KV 状态 | 语义化 Content 片段 + 关键词索引 | 文件 blob + 版本号 |
| **查询模式** | Key 精确查询 / 时间窗口 / 最近 N 条 | 全文/关键词搜索 | 文件名 + 版本号精确获取 |
| **作用域** | app + user + session | app + user（跨 session） | app + user + session（或 user 级） |
| **存储后端** | SQL/内存/Vertex AI | 内存/Vertex AI MemoryBank | 内存/GCS |
| **并发语义** | 事件追加需防 stale | 批量覆盖写入 | 版本号自增、有竞态窗口 |

三种存储的隔离避免了：将大量文件 blob 混入事件流导致 session 膨胀、将临时对话状态混入长期记忆导致记忆污染、将版本化文件的生命周期绑定到 session 的删除。

---

## 2. why_hard

### 2.1 多轮会话状态累积

每次 `AppendEvent` 不仅是追加事件，还需要：
- 将 `StateDelta` 合并到 Session State（`maps.Copy`），同时区分 app/user/session/temp 四种作用域。
- temp state 在事件追加后立即被 **trim** 掉（不持久化），但需要在当前 invocation 内可见。
- 如果 event 为 `Partial=true`（流式中间片段），必须跳过不持久化。

### 2.2 作用域状态的分层与合并

`session/session.go:163-176` 定义了三种前缀：

```
KeyPrefixApp  = "app:"   // 同一 app 下所有 user/session 共享
KeyPrefixUser = "user:"  // 同一 app 下同一 user 的所有 session 共享
KeyPrefixTemp = "temp:"  // 仅当前 invocation 有效，完事后丢弃
```

不带前缀的 key 属于 session 级。这个三层模型意味着：
- **Create 时**：`stateMap` 需要拆分为 appDelta、userDelta、sessionDelta，分别更新对应的存储。
- **Get 时**：需要先查 app state、再查 user state、最后 overlay session state（`MergeStates`）。
- **AppendEvent 时**：`StateDelta` 中带有 `app:`, `user:`, `temp:` 前缀的 key 需要路由到正确的作用域。

`sessionutils/utils.go:58-74` 的 `MergeStates` 实现了一层简单的字典合并：session 优先，然后追加带 `app:`/`user:` 前缀的 key。

### 2.3 Artifact 版本号的竞态窗口

`artifact/inmemory.go:157-160`：
```go
nextVersion := int64(1)
if internalVer, _, ok := s.find(appName, userID, sessionID, fileName); ok {
    nextVersion = internalVer + 1
}
s.set(appName, userID, sessionID, fileName, nextVersion, artifact)
```

在 `mu.Lock()` 保护下是安全的，但 `gcsartifact/service.go:104-114` 明确指出：
```go
// TODO race condition, could use mutex but it's a remote resource so the issue would still occurs
// with multiple consumers, and gcs does not have transactions spanning several operations
```

GCS 版本号通过 `listVersions → max + 1` 的方式分配，多消费者有竞态。

### 2.4 云后端和并发写入

- **database/service.go** 使用 GORM 事务包裹 state delta 应用 + event 插入 + session UpdateTime 更新。关键逻辑在 `applyEvent` (L358-436)：先查 session 是否存在，再校验 `storageUpdateTime <= sessionUpdateTime`（stale session 检测），然后逐层合并 app/user/session state。
- **vertexai/service.go** 将 `Get` 拆分为并发两个请求（`errgroup`）：一个拿 session 元数据，一个拿 events，然后组装。
- **inmemory/service.go** 用 `sync.RWMutex` 保证线程安全，state 迭代前先 clone 再释放锁。

### 2.5 Memory 的简单字符串匹配局限

`memory/inmemory.go` 的 `SearchMemory` 使用**单词级交集匹配** (`checkMapsIntersect`)，没有任何语义嵌入或向量搜索。这要求 query 和存储内容有严格的关键词重叠，容易遗漏语义相近但用词不同的内容。

---

## 3. design_approach

### 3.1 分层架构

```
┌─────────────────────────────────────────────────┐
│                   agent.Run                      │
│  InvocationContext (Session + Memory + Artifacts)│
├──────────┬──────────────┬───────────────────────┤
│ session. │  memory.     │  artifact.            │
│ Service  │  Service     │  Service              │
│(interface│(interface    │(interface             │
│ 5 methods│ 2 methods    │ 6 methods             │
├──────────┼──────────────┼───────────────────────┤
│ inmemory │ inmemory     │ inmemory              │
│ database │ vertexai     │ gcsartifact           │
│ vertexai │              │                       │
└──────────┴──────────────┴───────────────────────┘
```

每个状态域都有独立的 `Service` interface 和至少一种 in-memory 实现。数据库后端（session 用 GORM / artifact 用 GCS / memory 用 Vertex AI MemoryBank）通过各自的工厂函数注入。

### 3.2 Context 注入

`internal/context/invocation_context.go` 定义 `InvocationContext` 结构体，嵌入 `context.Context`，同时挂载 `agent.Artifacts`、`agent.Memory`、`session.Session`。Agent 和 Tool 在运行时通过 `InvocationContext` 访问这三种状态服务。

`callback_context.go` 提供 `NewCallbackContextWithDelta`，将 `stateDelta` 和 `artifactDelta` 注入到 callback context 中，使 model/tool callback 可以写入 state delta 和 artifact version 追踪信息。

### 3.3 内部 helper 层

- `internal/sessionutils/utils.go`：`ExtractStateDeltas` 和 `MergeStates`，被 inmemory 和 database 后端共享。
- `internal/memory/memory.go`：`Memory` 结构体包装 `memory.Service` + AppName/UserID/SessionID，作为 `agent.Memory` 的实现。
- `internal/artifact/artifacts.go`：`Artifacts` 结构体包装 `artifact.Service`，实现 `agent.Artifacts` 接口。

### 3.4 duplications 现状

`session/session.go:305-313` 定义的 `session` (unexported)、`database/session.go:29-39` 的 `localSession`、`vertexai/session.go:29-39` 的 `localSession` **完全重复**。三个包各自定义了相同的结构体和 `state`、`events` 类型、`appendEvent`、`trimTempDeltaState`、`updateSessionState` 函数。`database/session.go:28` 和 `vertexai/session.go:28` 都标注了 `TODO ... Move to sessioninternal`。

---

## 4. code_walkthrough

### 4.1 `session/session.go` — 核心类型定义

- **`Session` interface** (L32-46)：`ID() / AppName() / UserID() / State() / Events() / LastUpdateTime()`
- **`State` interface** (L51-62)：`Get(string) (any, error)` / `Set(string, any) error` / `All() iter.Seq2[string, any]`
- **`ReadonlyState` interface** (L67-74)：只读版 State，没有 `Set`
- **`Events` interface** (L79-87)：`All() iter.Seq[*Event]` / `Len() int` / `At(i int) *Event`
- **`Event` struct** (L92-118)：嵌入 `model.LLMResponse` + `ID`、`Timestamp`、`InvocationID`、`Branch`、`Author`、`Actions`、`LongRunningToolIDs`
- **`EventActions`** (L143-160)：`StateDelta map[string]any`、`ArtifactDelta map[string]int64`、`RequestedToolConfirmations`、`SkipSummarization`、`TransferToAgent`、`Escalate`
- **`NewEvent`** (L133-140)：用 `uuid.NewString()` 生成 ID，`time.Now()` 做时间戳，初始化 `StateDelta` 和 `ArtifactDelta` 为空 map
- **`IsFinalResponse`** (L124-130)：跳过 summarization、有 LongRunningToolID、无 function call/response、非 Partial、无 trailing code execution result 时返回 true
- **State 前缀常量** (L163-176)：`KeyPrefixApp = "app:"` / `KeyPrefixUser = "user:"` / `KeyPrefixTemp = "temp:"`
- **`ErrStateKeyNotExist`** (L179)：key 不存在时返回的哨兵错误

### 4.2 `session/service.go` — Service 接口和请求/响应类型

- **`Service` interface** (L25-32)：`Create` / `Get` / `List` / `Delete` / `AppendEvent`
- `InMemoryService()` 工厂 (L35-40)：返回 `inMemoryService`，初始化 `appState` 和 `userState`
- `CreateRequest` (L43-51)：`AppName`、`UserID`、可选的 `SessionID`、`State`
- `GetRequest` (L59-70)：`AppName`、`UserID`、`SessionID`，可选 `NumRecentEvents`、`After time.Time`
- `ListRequest` (L78-81)：`AppName`、`UserID`（可选）
- `DeleteRequest` (L89-92)：三元组

### 4.3 `session/inmemory.go` — 内存实现

**结构体** (L39-44)：
```go
type inMemoryService struct {
    mu        sync.RWMutex
    sessions  omap.Map[string, *session]  // 有序 map
    userState map[string]map[string]stateMap  // appName → userID → stateMap
    appState  map[string]stateMap            // appName → stateMap
}
```

**Create** (L46-93)：校验 `appName` 和 `userID`，空 `sessionID` 则自动生成 UUID。检查重复。将 `req.State` 通过 `sessionutils.ExtractStateDeltas` 拆分后更新 app/user state，再 `MergeStates` 后返回。

**Get** (L95-139)：校验三元组。从 omap 中查找 session。通过 `mergeStates` 合并 app/user/session 三层 state。应用 `NumRecentEvents`（取后 N 条）和 `After` 时间过滤器（二分搜索）。

**List** (L141-176)：用 `ordered.Encode` 构建 key range，扫描 omap 中匹配 `appName` 和 `userID` 的 sessions。

**AppendEvent** (L197-254)：核心逻辑。跳过 nil session/nil event/Partial event。做类型断言到 `*session`。在写锁下：从 omap 中重新获取存储的 session（防止引用过期），调用 `sess.appendEvent` → `updateSessionState` → `trimTempDeltaState`，然后将 event 深拷贝追加到存储的 session 中，最后通过 `ExtractStateDeltas` 路由状态变更。

**关键细节**：
- `trimTempDeltaState` (L429-446)：遍历 `StateDelta`，丢弃所有 `KeyPrefixTemp` 前缀的 key。
- `updateSessionState` (L448-461)：`maps.Copy(session.state, event.Actions.StateDelta)` 直接覆盖同 key。
- `copySessionWithoutStateAndEvents` (L463-472)：返回 session 的基本元数据副本，state 和 events 需重新设置。用于避免并发修改。

**session 结构体** (L305-313)：
```go
type session struct {
    id        id            // {appName, userID, sessionID}
    mu        sync.RWMutex
    events    []*Event
    state     map[string]any
    updatedAt time.Time
}
```

注意 `state` 是包内不可导出的 `state` 类型（L388-426），用共享的 `*sync.RWMutex`（指向 session.mu）做并发保护。`All()` 方法先 clone state 再释放读锁，避免迭代期间持锁。

### 4.4 `session/database/service.go` — GORM 数据库实现

**Create** (L71-138)：类似 inmemory，但用 GORM 事务包裹：先 fetch/create `storageApp` 和 `storageUser`，合并 delta，再 Insert `storageSession`。使用 `database/session.go` 中定义的 `localSession` 类型（与 `session.session` 重复）。

**Get** (L142-219)：GORM 查询 `storageSession`，再查 `storageEvent`，应用 `After` 和 `NumRecentEvents` 过滤。事件按 `timestamp DESC` 查询后**反转**为 ASC 顺序。最后 `mergeStates` 合并 app/user/session 三层 state。

**List** (L222-293)：支持按 `userID` 过滤或查全部。先查 app state，再查 user states（按需），然后逐一 merge。

**AppendEvent** (L319-354)：
1. 校验 nil Partial。`Timestamp` 截断到微秒（匹配 DB 精度）。
2. 类型断言到 `*localSession`。
3. 调用 `sess.appendEvent`（本地 state 合并）。
4. 调用 `trimTempDeltaState`。
5. 在事务中调用 `applyEvent`。

**applyEvent** (L358-436)：事务内的核心：
1. 从 DB 重新获取 `storageSession`（防御并发）。
2. **Stale Session 检测** (L374-382)：比较 `storageSess.UpdateTime.UnixMicro()` 和 `session.updatedAt.UnixMicro()`。如果 storage 的更新时间 > 请求的 session 更新时间，返回 `stale session error`。
3. `extractStateDeltas` → 分别更新 `storageApp` / `storageUser` / `storageSess` 的 state。
4. `createStorageEvent` → Insert 到 DB。
5. 更新 `storageSess.UpdateTime`。

**extractStateDeltas** (L481-504)：与 `sessionutils.ExtractStateDeltas` 逻辑相同但独立实现。这又是一处代码重复。

**mergeStates** (L508-527)：与 `sessionutils.MergeStates` 逻辑相同但独立实现。

### 4.5 `session/database/session.go` & `storage_session.go`

`session.go`:
- `localSession` 结构体及方法（与 `session.session` 完全重复）。
- `appendEvent`、`updateSessionState`、`trimTempDeltaState`（与 inmemory 版本重复）。
- `events` 和 `state` 类型的包装实现。

`storage_session.go`:
- `storageSession`: GORM 模型，对应 `sessions` 表。`State` 使用 `stateMap` 类型（JSON 序列化到 DB）。
- `storageEvent`: GORM 模型，对应 `events` 表。`Actions` 以 JSON 存储，`Content`、`GroundingMetadata` 等复杂字段也用 JSON。
- `storageAppState` / `storageUserState`: 分别对应 `app_states` / `user_states` 表，独立存储（而非嵌入 session 行中），通过 appName 关联。
- `createStorageEvent` / `createEventFromStorageEvent`: 序列化/反序列化转换。

### 4.6 `session/vertexai/` — Vertex AI ReasoningEngine 后端

- `vertexai.go`: `vertexAiService` 包装 `vertexAiClient`。
- `Create`: 不能指定 SessionID（Vertex AI 自行生成）。
- `Get`: 用 `errgroup` 并发获取 session 和 events，再组装。
- `AppendEvent`: 先调用远端 API，成功后再更新本地 `localSession`。
- `session.go`: 再次完整复制了 `localSession` + `state` + `events` + `appendEvent` + `trimTempDeltaState` + `updateSessionState`。

### 4.7 `memory/service.go` — Memory 接口

```go
type Service interface {
    AddSessionToMemory(ctx context.Context, s session.Session) error
    SearchMemory(ctx context.Context, req *SearchRequest) (*SearchResponse, error)
}
```

- `SearchRequest`: `Query` + `UserID` + `AppName`
- `SearchResponse`: `Memories []Entry`
- `Entry`: `ID` + `Content *genai.Content` + `Author` + `Timestamp` + `CustomMetadata`

### 4.8 `memory/inmemory.go` — 内存关键词匹配

**存储结构** (L54-57)：
```go
type inMemoryService struct {
    mu    sync.RWMutex
    store map[key]map[sessionID][]value
    // key = {appName, userID}
    // sessionID → events
}
```

**AddSessionToMemory** (L59-107)：遍历 session 的所有 events，跳过无 Content 的。对每个 Part 的 Text 分词（以空格 split，转小写），构建 `words map[string]struct{}`。按 app+user 定位，按 sessionID 覆盖写入（最新一次覆盖之前）。

**SearchMemory** (L109-141)：将 query 分词后，在对应 app+user 的 store 中遍历所有 session 的 events，用 `checkMapsIntersect` 做词集交集。

**性能特征**：O(S × E × W)，S 为 session 数，E 为每 session 的 event 数，W 为词数。无索引结构。

### 4.9 `memory/vertexai/vertexai.go` — Vertex AI MemoryBank

- `vertexAIService` 包装 `vertexAIClient`。
- `StateKeySessionLastUpdateTime`: 如果设置，`AddSessionToMemory` 会从 session state 中读取一个 `time.Time` 值，只提交该时间之后的新 events。如果为空，提交整个 session。
- 类型断言 `tm, ok := t.(time.Time)` 失败会报错，要求 state 值必须是 `time.Time` 类型。

### 4.10 `artifact/service.go` — Artifact 接口与验证

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

**验证逻辑**：每种 Request 有 `Validate()` 方法，检查必填字段（AppName/UserID/SessionID/FileName）和 Part 的内容（`Part.Text` 或 `Part.InlineData` 至少一个非空）。`SaveRequest` 额外检查 `Part` 非 nil。FileName 不允许含 `/` 或 `\`（防止路径穿越）。

`SaveRequest.Version` 字段注释表明可以指定版本号（乐观并发控制），但 inmemory 实现**忽略了请求中的 Version**，始终自增。

### 4.11 `artifact/inmemory.go` — 内存实现

**键编码** (L64-78)：使用 `rsc.io/ordered` 将 `{AppName, UserID, SessionID, FileName, Rev(Version)}` 编码为有序 key。`Rev(Version)` 将版本号反转，使最新版本排在扫描的最前面。

**Save** (L142-163)：
- 校验。
- `fileHasUserNamespace(fileName)`：如果文件名以 `user:` 开头，将 sessionID 替换为常量 `"user"`，实现跨 session 共享。
- `find` 找到最大版本号 → `+1` → `set`。

**Load** (L194-222)：如果 `version > 0` 精确加载，否则 `find` 最新版本。

**Delete** (L166-191)：如果 `version != 0` 精确删除，否则 `DeleteRange` 删除所有版本。

**List** (L225-259)：扫描 session 前缀范围 + user 前缀范围，去重后返回排序的文件名列表。

**Versions** (L262-286)：返回空则报 `fs.ErrNotExist` 错误。

### 4.12 `artifact/gcsartifact/service.go` — GCS 实现

**路径格式**：
- session 文件：`{appName}/{userID}/{sessionID}/{fileName}/{version}`
- user 文件：`{appName}/{userID}/user/{fileName}/{version}`

**Save** (L94-137)：调用内部 `versions` 拿到 max version → `+1`。写入 GCS blob，`InlineData` 写入 `ContentType`，`Text` 写入 `text/plain`。

**Load** (L185-229)：`resolveVersion` 确定目标版本，检查 blob attrs 存在性，通过 reader 读取全部内容（`io.ReadAll`）。

**Delete** (L140-182)：指定版本精确删除，否则先 list versions 再通过 `errgroup` 并发删除。

**List** (L270-293)：分别 scan session 前缀和 user 前缀，提取文件名（从路径倒数第二段）。

**已知问题**：version 分配有注释明确的竞态风险。`io.ReadAll` 不适合大文件。

### 4.13 内部 Context 和 Helper

**`internal/context/invocation_context.go`** (L27-116)：
- `InvocationContextParams` 聚合 `Artifacts`、`Memory`、`Session`、`Branch`、`Agent`、`UserContent`、`RunConfig`、`InvocationID` 等。
- `InvocationContext` 嵌入 `context.Context`，实现 `agent.InvocationContext` 接口。
- `LiveSessionResumptionHandle` 用于双向流 session 的断线重连。

**`internal/context/callback_context.go`** (L22-36)：
- `NewCallbackContext`：标准 callback context，`Artifacts().Save` 会自动追踪 `ArtifactDelta` 版本号。
- `NewCallbackContextWithDelta`：允许外部传入 `stateDelta` 和 `artifactDelta` map，用于在 callback 中追踪状态变更。

**`internal/sessionutils/utils.go`**：
- `ExtractStateDeltas`: 将 delta map 按 `app:`/`user:`/`temp:` 前缀拆分为三个 map。
- `MergeStates`: 将三层 state 合并，`app:`/`user:` 前缀加回去，session state 优先（无前缀）。

**`internal/memory/memory.go`**：`Memory` struct 实现 `agent.Memory`，转发到 `memory.Service`。

**`internal/artifact/artifacts.go`**：`Artifacts` struct 实现 `agent.Artifacts`，`Save`/`Load`/`LoadVersion`/`List` 转发到 `artifact.Service`。

---

## 5. state_lifecycle

### 5.1 用户消息进入 Session 的生命周期

```
User Input
    │
    ▼
Runner 创建 InvocationContext
    │  (注入 Session, Memory, Artifacts)
    ▼
Agent.Run(ctx)
    │
    ├── Session.State().Get(key)  ← 读取历史 state
    │
    ├── 模型调用前: 从 Session.Events() 构建历史上下文
    │
    ├── 模型返回后: 创建 Event{
    │        Actions.StateDelta = {"temp:step": val, "sk1": val2}
    │        Actions.ArtifactDelta = {"file1": 3}
    │   }
    │
    ├── CallbackContext:
    │       Artifacts().Save → artifact.Service.Save → 返回 version
    │       写入 artifactDelta map
    │
    ▼
Session.Service.AppendEvent(sess, event)
    │
    ├── 1. 校验: sess != nil, event != nil, !event.Partial
    ├── 2. 类型断言到 *session / *localSession
    ├── 3. sess.appendEvent(event)
    │       ├── updateSessionState: maps.Copy(sess.state, delta) — 包括 temp keys
    │       └── trimTempDeltaState: 从 event.Actions.StateDelta 中移除 "temp:" 前缀的 key
    │           然后将处理后的 event append 到 sess.events
    ├── 4. 持久化 event 副本到存储
    │       ├── inmemory: maps.Clone + slices.Clone → append 到存储
    │       ├── database: GORM 事务中 createStorageEvent + save session state
    │       └── vertexai: 远端 API 调用
    ├── 5. 应用 state delta 到 app/user state
    │       └── ExtractStateDeltas → updateAppState / updateUserState / maps.Copy(session.state, sessionDelta)
    ▼
完成: Invocation 内 State().All() 可见 temp keys (因为 local state 在 trim 前已合并)
     但下一个 invocation Get() 时 temp keys 不会出现 (因为持久化前已 trim)
```

### 5.2 Artifact Save / Load / List / Delete 生命周期

```
Save:
  Client.InlineData / Client.Text
    │
    ▼
  SaveRequest.Validate() → 必填字段检查 → Part 内容检查 → 文件名不含路径分隔符
    │
    ▼
  [user namespace?] → SessionID 替换为 "user"
    │
    ▼
  find(appName, userID, adjustedSessionID, fileName) → 取最大版本号
    │
    ▼
  nextVersion = maxVersion + 1
    │
    ▼
  set / GCS write(blobName=v{nextVersion})
    │
    ▼
  SaveResponse{Version: nextVersion}

Load:
  LoadRequest.Validate()
    │
    ├── version=0 → find最新版本
    └── version>0 → get精确版本
    │
    ▼
  LoadResponse{Part: *genai.Part}

Delete:
  DeleteRequest.Validate()
    │
    ├── version≠0 → 精确删除该版本
    └── version=0 → DeleteRange 删除所有版本

List:
  ListRequest.Validate()
    │
    ▼
  scan session scope → 收集 FileName
  scan user scope → 收集 FileName (user: 前缀文件)
    │
    ▼
  去重 → 排序 → ListResponse{FileNames}
```

### 5.3 Memory Add / Search 生命周期

```
AddSessionToMemory:
  Session.Events().All()
    │
    ▼
  遍历 events, 跳过 Content=nil 的
    │
    ▼
  提取 Part.Text → 分词 (空格split, 转小写)
    │
    ▼
  按 (appName, userID, sessionID) 写入 in-memory store
  [VertexAI: 调用 MemoryBank API]

SearchMemory:
  SearchRequest{Query, UserID, AppName}
    │
    ▼
  分词 query
    │
    ▼
  在 store[key{appName, userID}] 中遍历所有 session 的 events
    │
    ▼
  checkMapsIntersect(words, queryWords) → 有交集即返回
  [VertexAI: MemoryBank.search]
    │
    ▼
  SearchResponse{Memories: []Entry}
```

### 5.4 State Merge / Temp State Cleanup 机制

```
invocation 内可见的 state = session.state (已包含所有 delta, 包括 temp)

persist 时的 state delta 处理:
  1. event.Actions.StateDelta = {temp:k1, app:k2, user:k3, sk4}
  2. trimTempDeltaState → StateDelta = {app:k2, user:k3, sk4}  (temp 被移除)
  3. ExtractStateDeltas → appDelta={k2}, userDelta={k3}, sessionDelta={sk4}
  4. updateAppState(appDelta) / updateUserState(userDelta) / maps.Copy(sessionState, sessionDelta)

Get 时的 state 合并:
  1. 读 appState[appName], userState[appName][userID], sessionState
  2. MergeStates(appState, userState, sessionState)
     → 无前缀 session key 优先, 追加 "app:" + app key, 追加 "user:" + user key
```

---

## 6. tests

### 6.1 Session 测试

**`session/session_test/service_suite.go`** — 共享测试套件，覆盖：

| 测试 | 语义 |
|------|------|
| `Create/full_key` | 带 SessionID 创建 |
| `Create/generated_session_id` | 自动生成 ID |
| `Create/when_already_exists,_it_fails` | 重复创建失败 |
| `Get/ok` | 基本获取 |
| `Get/error_when_not_found` | 不存在的 session |
| `Get/get_session_respects_user_id` | UserID 隔离 |
| `Get/with_config_filters` | NumRecentEvents + After 过滤 |
| `List` | 按 user 过滤、返回空列表、按 app 返回全部 |
| `Delete` | 删除后 Get 失败 |
| `AppendEvent/ok` | 基本追加 |
| `AppendEvent/when_session_not_found_should_fail` | 不存在 session |
| `AppendEvent/partial_events_are_not_persisted` | Partial=true 不持久化 |
| `AppendEvent/with_bytes_content` | 二进制内容事件 |
| `AppendEvent/with_existing_events` | 事件追加顺序 |
| `AppendEvent/with_all_fields` | 全字段事件（LongRunning, Grounding, Usage, Citation, CustomMetadata） |
| `StateManagement/app_state_is_shared` | app: 前缀 state 跨用户/跨 session 可见 |
| `StateManagement/user_state_is_user_specific` | user: 前缀 state 同用户可见，不同用户隔离 |
| `StateManagement/session_state_is_not_shared` | 无前缀 state 不跨 session 泄漏 |
| `StateManagement/temp_state_is_not_persisted` | temp: 前缀 state 不持久化 |

**`session/inmemory_test.go`**：
- `TestInMemorySession_AppendEvent_Deadlock`: 测试 AppendEvent 不会死锁（曾发生在 state 写锁和 session 读锁嵌套的情况下）。
- `Test_inMemoryService_CreateConcurrentAccess`: 16 个 goroutine 并发创建同一 sessionID，验证恰好 1 成功。

**`session/database/service_test.go`**：使用 SQLite 内存数据库运行共享测试套件。

**`session/vertexai/vertexai_test.go`**：Vertex AI 后端的集成测试。

### 6.2 Memory 测试

**`memory/inmemory_test.go`** — `Test_inMemoryService_SearchMemory`：

| 测试用例 | 语义 |
|----------|------|
| `find events` | 多 session 多 event 的关键词匹配 |
| `no leakage for different appName` | app 隔离 |
| `no leakage for different user` | user 隔离 |
| `no matches` | 无匹配返回空 |
| `lookup on empty store` | 空 store 不 panic |

### 6.3 Artifact 测试

**`artifact/request_validation_test.go`**：每种 Request 类型 5-6 个用例，覆盖：
- 合法请求
- 单个必填字段缺失
- 多个必填字段缺失
- 完全空请求
- FileName 含路径分隔符（Save 额外检查 Part nil 和 Part 内容）

**`artifact/artifact_key_test.go`**：验证 `artifactKey.Encode()` 和 `Decode()` 的往返正确性。

**`internal/artifact/tests/service_suite.go`** — 共享测试套件：

| 测试 | 语义 |
|------|------|
| `Save` → `Load/latest` | 版本自增，最新版本自动加载 |
| `Save` → `Load ver=1/2` | 精确版本加载 |
| `List` | 多文件列表 |
| `Versions` | 多版本列表 |
| `Delete(version=3)` → `Load/latest` | 删除特定版本后最新版本退到 v2 |
| `Delete(all)` → `Load` 返回 `fs.ErrNotExist` | 全部删除后不存在 |
| `Delete(all)` → `Versions` 返回 `fs.ErrNotExist` | 一致 |
| `UserScoped` 系列 | `user:` 前缀文件跨 session 共享语义 |
| `Empty` 系列 | 空 service 的 Load/Versions 返回正确错误，Delete 和 List 不报错 |

### 6.4 缺失的测试

| 缺失测试 | 影响 |
|----------|------|
| **database stale session 并发场景** | `applyEvent` 中微秒级时间戳比较在真实并发下可能误判 |
| **artifact version 并发** | 两个 goroutine 同时 Save 同一文件名，inmemory 在锁保护下安全但 GCS 有竞态（代码注释也承认） |
| **Memory 的 session 覆盖写入语义** | `v[sid] = values` 直接覆盖，未测试多次 AddSessionToMemory 的行为 |
| **Memory 无并发测试** | 无 race test |
| **GCS artifact 大文件 load** | `io.ReadAll` 对大文件的内存风险无测试 |
| **Vertex AI session 的 `errgroup` 部分失败** | 无测试验证当一个并发请求失败时的行为 |
| **StateDelta 的类型安全** | state delta 的值是 `any`，无类型验证 |
| **IsFinalResponse 所有分支** | 如 SkipSummarization、LongRunningToolIDs、trailing CodeExecutionResult 未全部覆盖 |

---

## 7. risks

### 7.1 并发

| 风险 | 位置 | 描述 |
|------|------|------|
| Stale Session | `database/service.go:374-382` | 基于 UnixMicro 的时间戳比较，时钟不同步可能导致误判 |
| GCS 版本号竞态 | `gcsartifact/service.go:104` | 代码内注释承认此问题，多消费者同时 Save 会分配相同版本号 |
| Session state 读写锁粒度 | `session/inmemory.go` | `AppendEvent` 持全局写锁，高并发下可能成为瓶颈 |

### 7.2 后端行为不一致

| 行为 | InMemory | Database | Vertex AI | GCS |
|------|----------|----------|-----------|-----|
| **支持自定义 SessionID** | 是 | 是 | 否（报错） | N/A |
| **Session state 存储** | 与应用内 state 同结构 | JSON 列存储在 sessions 表，app/user state 独立表 | ReasoningEngine 远端 | N/A |
| **Event 序列化** | 原生 Go 对象 | `Actions` 为 JSON bytes | 远端序列化 | N/A |
| **不存在的 session Delete** | 返回 nil（无错误） | 返回 nil（GORM 默认） | 未明确 | N/A |
| **Memory Search** | 关键词交集 | 不支持 | MemoryBank API | N/A |
| **Artifact Save 使用 req.Version** | **忽略** | N/A | N/A | **忽略** |
| **Artifact Load 不存在** | `fs.ErrNotExist` | N/A | N/A | `fs.ErrNotExist` |
| **Artifact Versions 不存在** | `fs.ErrNotExist` | N/A | N/A | `fs.ErrNotExist` |

### 7.3 代码重复

| 重复块 | 出现位置 | 影响 |
|--------|----------|------|
| `localSession` + `state` + `events` 类型 | `session/inmemory.go`, `session/database/session.go`, `session/vertexai/session.go` | 修改需同步三处 |
| `trimTempDeltaState` | 同上三处 | 同上 |
| `updateSessionState` | 同上三处 | 同上 |
| `extractStateDeltas` | `internal/sessionutils/utils.go`, `session/database/service.go` | 逻辑相同但独立维护 |

### 7.4 API 语义风险

- **`StateDelta` 的 `maps.Copy` 行为**：同 key 直接覆盖，不提供原子 compare-and-swap，同一 invocation 内后写的覆盖先写的。
- **`ArtifactDelta` 未在 AppendEvent 中被消费**：`EventActions.ArtifactDelta` 在 `AppendEvent` 中被 clone 但没有触发任何 artifact 操作。它仅作为 event 的元数据存储，artifact 的 Save 是通过 callback context 中主动调用 `ArtifactService.Save` 完成的。
- **`SaveRequest.Version` 被所有实现忽略**：接口文档说可以指定版本号，但没有任何实现使用它做乐观锁。
- **Memory `AddSessionToMemory` 的幂等性**：每次调用都覆盖整个 session 的所有 events，不支持增量追加。

---

## 8. next_questions

1. `session/session.go:session`、`database/session.go:localSession`、`vertexai/session.go:localSession` 的重复何时会统一到 `sessioninternal` 包？统一后如何保证三个后端的 `State()` 接口行为一致？

2. Database 后端的 stale session 检测使用 `UnixMicro()` 比较，在分布式部署下（多个 server 实例），这个方案是否可靠？是否需要引入乐观锁版本号（如 integer version 或 ETag）？

3. GCS artifact 的版本号竞态是否有计划修复？例如用 GCS 的 `ifGenerationMatch` 条件写入或引入外部锁服务？

4. Memory 的 `SearchMemory` 目前是简单的关键词交集匹配。是否有计划引入 embedding / 向量搜索？Vertex AI MemoryBank 后端是否已支持语义搜索？

5. `EventActions.ArtifactDelta` 在 `AppendEvent` 中只做 clone 但没有触发实际的 artifact 操作。这个字段的预期用途是什么？是用于 audit log、replay，还是应该有 callback 自动触发？

6. `SaveRequest.Version` 字段在 inmemory 和 GCS 实现中都被忽略。它是否计划用于乐观并发控制（"save only if current version == X"）？还是已废弃？

7. Temp state（`temp:` 前缀）在 appendEvent 的本地合并（`updateSessionState`）和持久化清理（`trimTempDeltaState`）之间，线程安全是否有保证？如果多个 goroutine 同时 Set temp state 和 AppendEvent 会发生什么？

8. Memory `AddSessionToMemory` 覆盖写入的语义是否合理？当 session 有 1000 个 events，每次新增 1 个 event 时调用 `AddSessionToMemory`，会导致全部 events 重新分词和写入。是否需要增量机制？

9. Database 后端的 `extractStateDeltas` 和 `mergeStates` 与 `sessionutils` 包中的同名函数行为一致但独立维护。是否有回归测试保证两者同步？

10. Artifact 的 `user:` namespace 文件在 `inmemory.go` 中用 `sessionID = "user"` 实现，在 `gcsartifact` 中用路径前缀 `{app}/{user}/user/`。两个后端删除操作对 user 文件的隔离是否完全一致？

11. `InvocationContext` 和 `CallbackContext` 的生命周期如何管理？当 Agent 链路有多个子 Agent 时，StateDelta 和 ArtifactDelta 是共享同一个 map 还是各自独立？

12. `IsFinalResponse` 的判断逻辑依赖于 `hasTrailingCodeExecutionResult`。如果 code execution result 后还有继续的对话轮次（如用户要求修改代码），这个判断是否会错误终结 invocation？
