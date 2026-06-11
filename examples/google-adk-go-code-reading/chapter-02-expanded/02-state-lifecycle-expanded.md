# Chapter 02 - State Lifecycle: Session / Memory / Artifact 深度讲解

> 复刻工程：`/Users/likun/Desktop/workspace-for-google-adk-go/rive-adk-go/`  
> 教学细纲：`examples/google-adk-go-code-reading/manual/teaching-manual-outline.md`  
> 核心文件：`session/session.go`、`memory/service.go`、`memory/inmemory.go`、`artifact/service.go`、`artifact/inmemory.go`、`context/context.go`、`context/callback_context.go`、`runner/runner.go`  
> 建议讲解时长：14 分钟；建议自学阅读时长：60-90 分钟

Chapter 01 讲清楚了一次请求怎么穿过：

```text
Runner -> Agent -> Flow -> Model/Tool -> Event -> Session
```

Chapter 02 继续往下问一个更贴近真实 agent workflow 的问题：

**这条链路跑完以后，哪些东西应该留下？留下多久？下次请求还能不能看见？**

一个 agent 应用不是只在内存里回答一句话。它会产生很多不同性质的数据：

- 用户和模型的对话历史。
- 当前会话里的临时变量。
- 跨会话可见的用户偏好。
- 跨所有用户共享的 app 配置。
- 当前 invocation 内才有意义的 scratch data。
- 工具生成的文件、图片、报告。
- 从多次 session 中提炼出来的长期记忆。

这些都叫"状态"很容易，但它们不能放进同一个篮子里。Chapter 02 的核心就是：

**Session / Memory / Artifact 是三种不同生命周期的数据，不应该混成一个存储。**

---

## 1. 为什么 Session / Memory / Artifact 不能混成一个

先给一个业务场景：

```text
用户：帮我分析这份销售 CSV，生成一张趋势图。顺便记住我以后都喜欢用中文报告。
```

这句话会产生至少四类数据。

第一类是对话历史：

```text
user: 帮我分析 CSV...
model: 我需要读取文件并画图
tool: 读取文件结果
tool: 生成趋势图
model: 这里是分析结论
```

这属于 Session。它是这个会话线程里的事件流，下一轮对话要继续看。

第二类是会话内的 KV：

```text
topic = "sales-analysis"
current_file = "sales.csv"
```

这也属于 Session，但它不是 event，而是短期键值状态。

第三类是用户长期偏好：

```text
user:report_language = "zh-CN"
```

这不应该只存在当前 session。用户下一次开新 session 时，agent 也应该知道。

第四类是文件产物：

```text
trend.png version 1
trend.png version 2
```

这不适合塞进 event history。图片可能很大，而且同一个文件会有版本演进。

如果把这些都混进一个 store，会出现几个问题：

- Session event 里塞文件 blob，会让对话历史越来越大。
- 临时状态进入长期记忆，会污染用户画像。
- 文件版本依赖 session event 数量，会让版本管理混乱。
- 跨用户/跨 session 的隔离很难讲清。
- 一个 key 到底是当前 session 可见、当前 user 可见、还是所有 user 可见，会变得模糊。

所以 ADK Go 复刻版把它们拆成三个服务：

| 存储 | 解决什么 | 典型数据 | 查询方式 | 生命周期 |
| --- | --- | --- | --- | --- |
| Session | 当前对话线程 | events + scoped KV | sessionID 精确读取 | 会话生命周期 |
| Memory | 跨 session 长期知识 | 从 events 提取的长期记忆 | 语义/相关性搜索 | 用户长期生命周期 |
| Artifact | 文件产物 | text/blob + version | fileName + version | 文件生命周期 |

这就是本章的起点。

---

## 2. 本章代码地图

本章严格按教学大纲的第二章展开。建议按这个顺序读源码：

| 文件 | 重点 | 为什么读 |
| --- | --- | --- |
| `session/session.go` | `State`、`Session`、四个 state 前缀、`ExtractStateDeltas`、`MergeStates`、`AppendEvent`、`applyStateDelta` | Session 和 scoped state 的核心 |
| `artifact/service.go` | `Service` 接口、`SaveRequest`、`LoadRequest`、request validation | Artifact 的 API 边界 |
| `artifact/inmemory.go` | `Save`、`Load`、`List`、`Versions`、`resolveIdentity` | 版本自增和 `user:` 文件命名空间 |
| `memory/service.go` | `Service`、`SearchRequest`、`Entry` | Memory 的接口形状 |
| `memory/inmemory.go` | `AddSessionToMemory`、`SearchMemory`、`wordsIntersect` | 当前本地 stub；和原版 Memory 语义有差距 |
| `context/callback_context.go` | `callbackContextState`、`trackedArtifacts` | CallbackContext 的 write-through state 和 artifact delta |
| `runner/runner_test.go` | scoped state、temp state、artifact、memory 测试 | 行为证据 |

注意：精读报告里有很多原版 Google ADK Go 的 database / VertexAI / GCS 后端细节；复刻版当前主要实现的是 in-memory 教学骨架。本章解释时以 `rive-adk-go` 当前代码为准，必要时再说明"生产版会有更多后端"。

---

## 3. 四种 state scope：同样是 key，生命周期完全不同

`session/session.go` 文件开头已经把四种 scope 写在注释里：

```go
// "app:"  — shared by all users and sessions within the same app.
// "user:" — shared across all sessions for the same user within the app.
// "temp:" — visible only during the current invocation, never persisted.
// (no prefix) — scoped to the individual session.
```

对应常量：

```go
const (
    KeyPrefixApp  = "app:"
    KeyPrefixUser = "user:"
    KeyPrefixTemp = "temp:"
)
```

可以把它们画成四层：

```text
app:env
  app 内所有用户、所有 session 都能看到

user:theme
  同一个 app 下，这个 user 的所有 session 能看到

topic
  只有当前 session 能看到

temp:scratch
  只有当前 invocation 过程中短暂可见，持久化前清掉
```

### 3.1 `app:` scope

`app:` 适合放应用级配置：

```text
app:env = "production"
app:region = "us-east-1"
```

它的共享范围最大：同一个 app 下所有 user 和 session。

在复刻版 `Service` 里，它存到：

```go
appState map[string]map[string]any // appName -> state
```

### 3.2 `user:` scope

`user:` 适合放用户偏好：

```text
user:theme = "dark"
user:lang = "zh-CN"
```

它跨 session，但不跨 user。

在复刻版 `Service` 里，它存到：

```go
userState map[string]map[string]map[string]any // appName -> userID -> state
```

### 3.3 无前缀 session scope

无前缀 key 是当前 session 私有状态：

```text
topic = "state-lifecycle"
current_file = "sales.csv"
```

它只应该出现在当前 session。另一个 session 即使同 app、同 user，也不应该看到这个 session-local key。

### 3.4 `temp:` scope

`temp:` 是最容易误解的：

```text
temp:scratch = "tmp-data"
```

它可以在 invocation 内被读写，但不能持久化。为什么需要它？

因为某些中间状态只在这次调用里有意义，比如：

- 当前工具调用的临时计算结果。
- callback 之间共享的短暂标记。
- 不应该进入 session history 的缓存。

如果它被写入 Session，下一次请求看到它就会产生污染。

测试 `TestRunnerTempStateLifecycle` 验证了三件事：

- `temp:cache` 在 Run 返回后不在 durable session state 中。
- persisted event 的 `StateDelta` 中也没有 `temp:cache`。
- `GetMergedState` 也看不到 `temp:cache`。

---

## 4. State 写入路径：从 StateDelta 到三层 store

教学大纲里要求画出这条路径：

```text
StateDelta
  -> trimTempDeltaState
  -> ExtractStateDeltas
  -> updateAppState / updateUserState / maps.Copy
```

复刻版里最核心的是 `Service.AppendEvent`：

```go
func (svc *Service) AppendEvent(sess Session, ev *event.Event) error {
    if sess == nil { ... }
    if ev == nil { ... }
    if ev.Partial {
        return nil
    }

    is, ok := sess.(*inMemorySession)
    if !ok { ... }

    svc.mu.Lock()
    defer svc.mu.Unlock()

    if len(ev.Actions.StateDelta) > 0 {
        fullDelta := ev.Actions.StateDelta
        svc.applyStateDelta(is.appName, is.userID, is.state, fullDelta)
        ev.Actions.StateDelta = trimTempDeltaState(fullDelta)
        removeTempKeysFromState(is.state)
    }

    is.events = append(is.events, ev)
    return nil
}
```

这段代码有四个关键点。

### 4.1 Partial event 不持久化，也不能改 durable state

开头：

```go
if ev.Partial {
    return nil
}
```

Partial event 是流式片段。它可以被 yield 给调用方，但不是稳定历史。它也不能借着 `StateDelta` 修改 durable state。

这和 Chapter 01 的 partial 持久化边界一致。

### 4.2 fullDelta 先应用到 session state

```go
fullDelta := ev.Actions.StateDelta
svc.applyStateDelta(is.appName, is.userID, is.state, fullDelta)
```

`applyStateDelta` 先把所有 key 写入 session state：

```go
st.mu.Lock()
for k, v := range delta {
    st.data[k] = v
}
st.mu.Unlock()
```

这里包括 `app:`、`user:`、`temp:`。为什么先都写进去？

因为在当前 invocation 内，代码可能需要马上读到这些值。尤其是 callback write-through 模式下，同一步里的后续 callback 应该看到前面 callback 写入的状态。

### 4.3 再按前缀拆出 app/user/session delta

```go
appDelta, userDelta, _ := ExtractStateDeltas(delta)
svc.updateAppState(appDelta, appName)
svc.updateUserState(userDelta, appName, userID)
```

`ExtractStateDeltas` 的逻辑是：

```go
for key, value := range delta {
    if clean, ok := strings.CutPrefix(key, KeyPrefixApp); ok {
        appDelta[clean] = value
    } else if clean, ok := strings.CutPrefix(key, KeyPrefixUser); ok {
        userDelta[clean] = value
    } else if !strings.HasPrefix(key, KeyPrefixTemp) {
        sessionDelta[key] = value
    }
}
```

注意：

- `app:version` 进入 appDelta，key 变成 `version`。
- `user:pref` 进入 userDelta，key 变成 `pref`。
- `local` 进入 sessionDelta。
- `temp:scratch` 被丢弃，不进入任何 durable delta。

在 `applyStateDelta` 里，复刻版只用 appDelta 和 userDelta 更新共享 store；session-local 已经写进 `is.state`。

### 4.4 持久化前 trim temp

```go
ev.Actions.StateDelta = trimTempDeltaState(fullDelta)
removeTempKeysFromState(is.state)
```

这两句分别处理两个地方：

- persisted event 的 `StateDelta` 不应含 `temp:`。
- session durable state 里也不应残留 `temp:`。

所以 temp 的生命周期是：

```text
可以进入当前 invocation 的 state 视图
但 Run 结束/AppendEvent 持久化后必须消失
```

---

## 5. MergeStates：读状态时怎么合并 app/user/session

写入时按前缀拆分；读取时要合并回来。

`MergeStates` 的注释写得很清楚：

```go
// Overlay order: session (highest priority) -> user -> app (lowest).
// App-level keys are prefixed with "app:", user-level with "user:".
// Session-level keys are stored without a prefix.
```

实现简化后是：

```go
for k, v := range appState {
    merged["app:"+k] = v
}
for k, v := range userState {
    merged["user:"+k] = v
}
for k, v := range sessionState {
    merged[k] = v
}
```

复刻版还处理 tombstone：session state 中的 tombstone 可以隐藏下层 app/user key。这是删除语义，不是本章主线，但要知道它存在。

`Service.GetMergedState` 调用：

```go
return svc.mergeStatesForSession(sess), nil
```

而 `mergeStatesForSession` 会先拿到：

- `svc.appState[sess.appName]`
- `svc.userState[sess.appName][sess.userID]`
- session-local state

再调用 `MergeStates`。

测试 `TestRunnerStateMergeAcrossSessions` 是最好的行为说明：

1. Session A 写入：

```text
app:theme = "corp"
user:lang = "en"
topic = "from_sess1"
```

2. Session B 同 app 同 user 写入：

```text
user:font = "large"
topic = "from_sess2"
```

3. Session B 的 merged state 能看到：

```text
app:theme = "corp"
user:lang = "en"
user:font = "large"
topic = "from_sess2"
```

4. Session A 仍然看到自己的：

```text
topic = "from_sess1"
```

但也能看到 Session B 写入的 `user:font`，因为这是 user scope。

这就是四层 state scope 的实际效果。

---

## 6. CallbackContext 为什么要 write-through

教学大纲专门要求解释：

> 判断为什么 CallbackContext 的 `State()` 是 write-through 模式。

`context/callback_context.go` 的注释已经说明：

```go
// State returns a write-through state that records writes into
// actions.StateDelta and also persists them to the durable session state.
//
// Reads first check the current step's StateDelta ... and fall back to durable session state.
```

`Set` 的实现：

```go
func (c *callbackContextState) Set(key string, val any) {
    if c.ctx.actions != nil && c.ctx.actions.StateDelta != nil {
        c.ctx.actions.StateDelta[key] = val
    }
    c.ctx.invocationContext.Session().State().Set(key, val)
}
```

也就是说，它同时写两处：

```text
actions.StateDelta[key] = val
session.State().Set(key, val)
```

### 6.1 为什么要写 StateDelta

因为 event 要记录这次 callback 对 state 做了什么。后续 Runner / Session append 时，需要通过 `StateDelta` 进行 scope routing 和持久化。

如果只写 session state，不写 delta，event 就失去可审计性，也无法在 append 阶段正确拆分 app/user/session。

### 6.2 为什么要写 durable session state

因为同一步里后续 callback 或 tool 可能马上要读到这个值。

测试 `TestCallbackStateWriteThrough` 验证：

- `actions.StateDelta["my_key"]` 有值。
- `ic.Session().State().Get("my_key")` 也有值。

测试 `TestCallbackStateDeltaAcrossCallbacks` 验证：

- callback 1 写入。
- callback 2 在同一步里读到 callback 1 的写入。

这就是 write-through 的意义：既记录 delta，又保证当前 invocation 立即可见。

### 6.3 代价是什么

write-through 的代价是：状态修改不是纯粹延迟到 append 阶段才生效。callback 一写，session state 视图就变了。

所以实现必须再通过 `StateDelta`、`trimTempDeltaState`、`removeTempKeysFromState` 来保证最终 durable state 不被 temp 污染。

这也是为什么 state lifecycle 比普通 map 复杂。

---

## 7. Artifact：文件产物为什么要独立版本化

Artifact 的接口在 `artifact/service.go`：

```go
type Service interface {
    Save(ctx context.Context, req *SaveRequest) (*SaveResponse, error)
    Load(ctx context.Context, req *LoadRequest) (*LoadResponse, error)
    Delete(ctx context.Context, req *DeleteRequest) error
    List(ctx context.Context, req *ListRequest) (*ListResponse, error)
    Versions(ctx context.Context, req *VersionsRequest) (*VersionsResponse, error)
    GetArtifactVersion(ctx context.Context, req *GetArtifactVersionRequest) (*GetArtifactVersionResponse, error)
}
```

为什么它不是 Session 的一部分？

因为文件和事件不是同一种数据：

- Event 是有序历史。
- Artifact 是命名文件。
- Event 追加一次就是一条历史。
- Artifact 保存同名文件时应该产生新版本。
- Event 通常较小；Artifact 可能是图片、报告、二进制 blob。

### 7.1 Save：版本自增

`artifact/inmemory.go` 的 `Save`：

```go
versions := s.store[identity]
nextVersion := int64(1)
if len(versions) > 0 {
    nextVersion = versions[len(versions)-1].version + 1
}

cp := *req.Part
s.store[identity] = append(versions, versionedPart{version: nextVersion, part: &cp})
return &SaveResponse{Version: nextVersion}, nil
```

同一个 `(app, user, session, file)` 下：

- 第一次 Save -> version 1。
- 第二次 Save -> version 2。
- 新文件重新从 version 1 开始。

测试 `TestInMemoryService_SaveAndLoadLatest`、`TestInMemoryService_LoadExplicitVersion`、`TestInMemoryService_VersionIncrementDeterministic` 都在覆盖这个行为。

### 7.2 Load：不指定 version 就读 latest

`Load` 的逻辑：

```go
if req.Version > 0 {
    for _, v := range versions {
        if v.version == req.Version {
            return &LoadResponse{Part: v.part}, nil
        }
    }
    return not found
}

latest := versions[len(versions)-1]
return &LoadResponse{Part: latest.part}, nil
```

所以：

```text
Load(file.txt, version=2) -> 精确读 v2
Load(file.txt, version=0) -> 读最新版本
```

### 7.3 `user:` artifact namespace

artifact 也有一个特殊前缀：`user:`。

```go
func fileHasUserNamespace(filename string) bool {
    return strings.HasPrefix(filename, "user:")
}

func (s *inMemoryService) resolveIdentity(app, user, session, file string) artifactIdentity {
    if fileHasUserNamespace(file) {
        session = userScopedSessionID
    }
    return artifactIdentity{appName: app, userID: user, sessionID: session, fileName: file}
}
```

如果文件名是：

```text
user:preferences.json
```

那么它不再绑定当前 session，而是绑定到伪 session：

```text
sessionID = "user"
```

效果是：同一个 app + user 下，不同 session 都能 Load 这个 artifact。

测试 `TestInMemoryService_UserScopedArtifactCrossSession` 验证 session 1 保存的 `user:preferences.json` 可以从 session 2 读取。

同时，测试 `TestInMemoryService_UserScopedArtifactIsolatedByUser` 和 `TestInMemoryService_UserScopedArtifactIsolatedByApp` 验证它不会跨 user / app 泄漏。

### 7.4 ArtifactDelta：callback 保存文件时如何记录版本

`context/callback_context.go` 里有一个 `trackedArtifacts` decorator：

```go
func (t *trackedArtifacts) Save(ctx context.Context, req *artifact.SaveRequest) (*artifact.SaveResponse, error) {
    resp, err := t.inner.Save(ctx, req)
    if err != nil { ... }
    if t.actions != nil && t.actions.ArtifactDelta != nil {
        t.actions.ArtifactDelta[req.FileName] = resp.Version
    }
    return resp, nil
}
```

这和 StateDelta 是同一个思路：callback 或 tool 保存 artifact 后，event action 上要记录"哪个文件保存到了哪个版本"。

否则用户只看到"生成了报告"，但事件里没有结构化版本信息，后续就很难追踪。

---

## 8. Memory：跨 Session 的长期知识

Memory 的接口很小：

```go
type Service interface {
    AddSessionToMemory(ctx context.Context, s session.Session) error
    SearchMemory(ctx context.Context, req *SearchRequest) (*SearchResponse, error)
}
```

这说明 Memory 不是实时 session history。它要显式摄取：

```text
AddSessionToMemory(session)
```

然后再搜索：

```text
SearchMemory(appName, userID, query)
```

### 8.1 AddSessionToMemory：从事件里摄取长期记忆材料

当前本地 in-memory 实现会从 session event 中抽取文本：

```go
for _, ev := range curSession.Events() {
    if ev.Partial { continue }
    if ev.Content == nil { continue }

    words := make(map[string]struct{})
    for _, part := range ev.Content.Parts {
        if part.Text == "" { continue }
        for _, w := range strings.Fields(part.Text) {
            words[strings.ToLower(w)] = struct{}{}
        }
    }

    if len(words) == 0 { continue }

    values = append(values, entryValue{...})
}
```

它只摄取：

- 非 partial event。
- 有 content 的 event。
- 有 text 的 part。

它不会摄取 function call args，也不会摄取二进制 artifact。作为教学代码，这段能说明"Memory 来自 session 历史的显式摄取"；但它还不是原版 ADK 的完整 Memory 能力。

### 8.2 SearchMemory：原版说法是语义/相关性检索

Memory 不是按 key 精确读取，也不应该把长期记忆理解成普通字符串匹配。更贴近原版 ADK 的说法是：

- `SearchMemory` 接收自然语言 query。
- Memory service 返回和 query 相关的长期记忆条目。
- 具体检索算法由后端决定：可以是 MemoryBank、向量检索、摘要索引或其他托管记忆系统。

我在本机原版 `adk-go` 里核对到两处直接证据：

- `agent/callback_context.go` 的注释把 `SearchMemory` 描述为 semantic search。
- `memory/vertexai/vertexai_client.go` 的 VertexAI MemoryBank 后端调用 `RetrieveMemories`，参数使用 `SimilaritySearchParams.SearchQuery`。

所以教学主线应该是：

> ADK Memory 表达的是跨 session 的长期相关性/语义检索能力；用户问一个自然语言 query，运行时把相关长期记忆取回，再交给后续 agent workflow 使用。

### 8.3 当前本地实现差距：in-memory 只是临时 stub

当前本地 `memory/inmemory.go` 里还能看到一个简化实现：

```go
for _, w := range strings.Fields(req.Query) {
    queryWords[strings.ToLower(w)] = struct{}{}
}

if wordsIntersect(e.words, queryWords) {
    memories = append(memories, Entry{...})
}
```

这段只能当作本地 in-memory stub 或测试替身。它可以用来验证：

- memory entries 的生命周期。
- app/user 隔离。
- `AddSessionToMemory` 的摄取时机。

但它不应该作为 Memory 的正确产品语义来教。读者要带走的是原版 ADK 的抽象：Memory 是长期语义/相关性检索；这个本地 stub 需要替换成真正的 MemoryBank/向量/语义检索后端。

### 8.4 Memory 的隔离边界

Memory store 的 key 是：

```go
type appUserKey struct {
    appName, userID string
}
```

所以它按 `(appName, userID)` 隔离。

测试：

- `TestInMemoryService_AppScopedIsolation`：app_a 的 memory 不泄漏到 app_b。
- `TestInMemoryService_UserScopedIsolation`：user_a 的 memory 不泄漏到 user_b。

这和 user-scoped artifact 很像：都可以跨 session，但不能跨 app/user。

### 8.5 AddSessionToMemory 是覆盖，不是增量追加

`AddSessionToMemory` 最后写：

```go
v[sessionID(curSession.ID())] = values
```

这意味着同一个 session 再次 Add，会覆盖这个 session 对应的 memory entries。

测试 `TestInMemoryService_UpdatesSessionEntries` 验证：

- 第一次写入 `"first message"`。
- 第二次同 session 写入 `"updated message"`。
- 搜 `"first"` 不再能搜到。

所以复刻版 memory ingestion 是 per-session overwrite，不是 append-only。

---

## 9. 三条生命周期放在一起看

现在把 Session / Memory / Artifact 放到同一个业务流程里。

用户说：

```text
请分析 sales.csv，生成图表，并记住我喜欢中文报告。
```

运行时可能做：

```text
Session events:
  user message
  model function call
  tool result
  model final answer

Session state:
  topic = "sales-analysis"
  current_file = "sales.csv"

User state:
  user:report_language = "zh-CN"

Artifact:
  chart.png v1
  report.md v1

Memory:
  "用户喜欢中文报告"
```

下一轮同 session：

- 能看到 session history。
- 能看到 `topic`。
- 能看到 `user:report_language`。
- 能 Load 当前 session 的 chart/report。

新 session、同 user：

- 看不到旧 session 的 `topic`。
- 能看到 `user:report_language`。
- 能搜索 memory 里的偏好。
- 能读取 `user:` scoped artifact，但不能读取普通 session artifact。

另一个 user：

- 看不到这个 user 的 user state。
- 搜不到这个 user 的 memory。
- 读不到这个 user 的 artifact。

这就是三套存储存在的意义。

---

## 10. 复刻版和生产版的边界

本章需要忠实于 `rive-adk-go`，所以必须讲清当前复刻版边界。

### 10.1 Session

复刻版主要是 in-memory `session.Service`。它足够展示：

- app/user/session/temp 四种 scope。
- StateDelta 路由。
- temp 清理。
- merged state。
- partial event 不持久化。

生产级系统还会考虑：

- 数据库事务。
- stale session 检测。
- 远程 session service。
- 分布式并发写入。

这些在精读报告里有提到，但不是当前复刻源码的主线。

### 10.2 Artifact

复刻版 artifact 是 in-memory store，`mu` 保护下版本自增是确定的。

生产级 GCS/blob store 可能出现：

```text
list versions -> max + 1 -> write
```

这类远程版本号竞态，需要更强的并发控制。

### 10.3 Memory

Memory 的原版语义是长期相关性/语义检索。当前本地 in-memory 后端只是简化 stub，不能代表最终设计。

可替换的正确后端可以是：

- 向量检索。
- 摘要提取。
- 增量 ingestion。
- Vertex AI MemoryBank。

原版 `memory/vertexai` 已经展示了这个方向：它通过 Vertex AI MemoryBank 的 `RetrieveMemories` 和 `SimilaritySearchParams.SearchQuery` 做托管 similarity retrieval。

但接口层的教学意义已经足够：Session 是短期事件，Memory 是长期可搜索知识；本地 stub 要向语义/相似度检索后端演进。

---

## 11. 测试如何证明本章语义

### 11.1 State scope

`TestRunnerScopedStateMutation` 验证：

- `app:version` 能出现在 merged state。
- `user:pref` 能出现在 merged state。
- `local` 是 session-local。
- `temp:scratch` 不会进入 durable state 和 merged state。

### 11.2 跨 session merge

`TestRunnerStateMergeAcrossSessions` 验证：

- 同 app/user 下，Session B 能看到 Session A 写入的 app/user state。
- Session A 和 Session B 的无前缀 `topic` 互不覆盖。

### 11.3 temp 生命周期

`TestRunnerTempStateLifecycle` 验证：

- `temp:` key 不留在 session state。
- persisted event 的 StateDelta 也没有 temp key。

### 11.4 Artifact 版本

`TestInMemoryService_SaveAndLoadLatest`、`TestInMemoryService_LoadExplicitVersion`、`TestInMemoryService_VersionsIndependentFromEvents` 验证：

- 第一次 Save 是 v1。
- 第二次 Save 是 v2。
- Load 不指定 version 读 latest。
- 版本与 session event 数量无关。

### 11.5 Artifact user namespace

`TestInMemoryService_UserScopedArtifactCrossSession` 验证 `user:` 文件跨 session 可见。

`TestInMemoryService_UserScopedArtifactIsolatedByUser` 和 `TestInMemoryService_UserScopedArtifactIsolatedByApp` 验证隔离边界。

### 11.6 Memory 搜索和隔离

`TestInMemoryService_AddAndSearchMemory` 只能验证当前 in-memory stub 的检索路径；教学时不要把它提升为 Memory 的最终语义。

`TestInMemoryService_MemorySurvivesAcrossSessions` 验证 memory 跨 session。

`TestInMemoryService_AppScopedIsolation` / `TestInMemoryService_UserScopedIsolation` 验证隔离。

`TestInMemoryService_UpdatesSessionEntries` 验证同 session ingestion 是覆盖。

### 11.7 CallbackContext write-through

`TestCallbackStateWriteThrough` 验证 Set 同时写 StateDelta 和 durable state。

`TestCallbackStateDeltaAcrossCallbacks` 验证同一步 callback 间能通过 delta 看到对方写入。

`TestArtifactSaveTracking` 验证 artifact Save 会记录到 `ArtifactDelta`。

---

## 12. 容易误解点

### 12.1 "State 就是 Session State"

不完整。State 有四种 scope：app、user、session、temp。Session state 只是无前缀那一层。

### 12.2 "`app:` / `user:` 只在共享 store 里，不进 session state"

复刻版写入时会先把 full delta 应用到 session state 视图，再路由 app/user store，之后 merged view 会按 prefix 展示。理解时不要把"当前 invocation 可见"和"最终共享存储位置"混为一谈。

### 12.3 "`temp:` 只是普通 key"

不是。`temp:` 会在持久化前从 event delta 和 session state 中清掉。

### 12.4 "Memory 只是本地 stub 能力"

不对。原版 ADK 的 Memory 抽象是长期相关性/语义检索，VertexAI MemoryBank 后端使用 similarity search。当前本地 in-memory 的 word-intersection 行为只是 stub 差距。

### 12.5 "看到本地 in-memory 测试通过，就说明 Memory 做完了"

不对。测试只能说明生命周期、隔离边界和 stub 调用链能跑通。真正对齐原版语义，还要把后端替换成 MemoryBank/向量/语义检索。

### 12.6 "Artifact List 会返回文件内容"

不会。`List` 只返回 file names。要内容必须 `Load`。

### 12.7 "Artifact 版本跟事件数量有关"

无关。Artifact 每个文件独立自增版本。

### 12.8 "`user:` artifact 可以跨用户"

不能。它只是跨 session，不跨 app/user。

### 12.9 "AddSessionToMemory 是增量追加"

复刻版不是。同一个 session 再次 Add 会覆盖该 session 的 memory entries。

### 12.10 "CallbackContext 可以只写 delta，不写 durable state"

复刻版选择 write-through，是为了同一步里的后续 callback/tool 可以马上读到更新。只写 delta 会让当前 invocation 的读取语义更复杂。

---

## 13. 课堂讲解脚本

### 第 0-2 分钟：从业务问题切入

抛问题：

```text
用户上传文件、要求生成报告、还说以后都用中文。
这些状态分别应该存在哪里？
```

引出：

- 对话历史 -> Session
- 用户偏好 -> user state / Memory
- 文件 -> Artifact
- 临时计算 -> temp state

### 第 2-5 分钟：讲三种存储不能混

画表：

| 类型 | 生命周期 | 查询方式 | 数据模型 |
| --- | --- | --- | --- |
| Session | 会话 | sessionID | events + KV |
| Memory | 长期 | 语义/相关性 query | text entries |
| Artifact | 文件 | file + version | blob/text |

### 第 5-8 分钟：讲四种 state scope

用例子：

```text
app:env = production
user:theme = dark
topic = sales-analysis
temp:scratch = tmp
```

讲 `StateDelta -> ExtractStateDeltas -> app/user/session`。

强调 `temp:` 不持久化。

### 第 8-10 分钟：讲 CallbackContext write-through

解释：

```text
Set(key, value)
  -> actions.StateDelta[key] = value
  -> session.State().Set(key, value)
```

一句话：

> 既要留下可持久化 delta，又要让同一次 invocation 的后续逻辑马上读到。

### 第 10-12 分钟：讲 Artifact

讲：

- Save -> version 1。
- Save again -> version 2。
- Load version=0 -> latest。
- `user:` filename -> 跨 session。

### 第 12-14 分钟：讲 Memory

讲：

- AddSessionToMemory 摄取 session events。
- SearchMemory 的原版说法是长期相关性/语义检索。
- 按 app/user 隔离。
- 同 session 再 Add 是覆盖。
- 当前本地 in-memory 只是 stub；VertexAI MemoryBank 后端是 similarity retrieval。

最后用测试名收口。

---

## 14. 实战阅读任务

### 任务 1：追踪一次 scoped state 写入

打开 `TestRunnerScopedStateMutation`，手动追踪：

```text
state_delta:
  app:version
  user:pref
  local
  temp:scratch
```

写出每个 key 最终在哪里可见。

### 任务 2：画出 AppendEvent 状态路由

根据 `Service.AppendEvent` 画：

```text
fullDelta
  -> applyStateDelta
      -> session state
      -> ExtractStateDeltas
      -> appState/userState
  -> trimTempDeltaState
  -> removeTempKeysFromState
  -> append event
```

### 任务 3：写 artifact 版本代码

写出：

```go
Save(report.txt, "v1") -> version 1
Save(report.txt, "v2") -> version 2
Load(report.txt, Version: 1) -> "v1"
Load(report.txt, Version: 0) -> "v2"
```

### 任务 4：验证 `user:` artifact

Session 1 保存 `user:preferences.json`，Session 2 读取它。然后换 user，确认读不到。

### 任务 5：解释本地 memory stub 的局限

为什么当前本地 stub 不能代表原版 ADK 的 Memory 语义？如果要对齐 VertexAI MemoryBank / semantic retrieval，需要替换哪些地方？

---

## 15. 自测题

1. Session / Memory / Artifact 分别解决什么生命周期问题？
2. `app:`、`user:`、无前缀、`temp:` 的可见范围分别是什么？
3. `ExtractStateDeltas` 会如何处理 `temp:` key？
4. `AppendEvent` 为什么要先应用 full delta，再 trim temp？
5. `GetMergedState` 为什么要给 app/user key 加前缀？
6. CallbackContext `State().Set` 为什么要同时写 StateDelta 和 session state？
7. Artifact 第一次保存某文件时 version 是多少？第二次呢？
8. `Load` 不指定 version 时读什么？
9. `user:` artifact 能跨 session 吗？能跨 user 吗？
10. Memory 的原版设计语义是什么？当前本地 in-memory stub 的差距是什么？
11. `AddSessionToMemory` 对同一个 session 是追加还是覆盖？
12. Partial event 会进入 Memory 吗？为什么？
13. 原版 ADK Go 的 VertexAI MemoryBank 后端使用什么检索参数？

参考答案：

1. Session 管当前会话事件和短期 KV；Memory 管跨 session 长期知识；Artifact 管独立版本化文件。
2. `app:` 同 app 全局；`user:` 同 app 同 user；无前缀当前 session；`temp:` 当前 invocation。
3. 丢弃，不进入 app/user/session durable delta。
4. 为了 invocation 内先可见，再在持久化边界清理 temp。
5. 为了让 merged view 仍能区分 app/user/session 来源。
6. StateDelta 用于事件记录和后续路由；session state 用于当前 invocation 立即可见。
7. v1，v2。
8. 最新版本。
9. 能跨 session；不能跨 user/app。
10. 原版设计语义是跨 session 的长期相关性/语义检索；当前本地 in-memory stub 只是 word-intersection，不能代表最终 Memory 设计。
11. 覆盖同 session entries。
12. 不会。Partial 是流式过程片段，不是稳定长期记忆来源。
13. 它调用 Vertex AI MemoryBank 的 `RetrieveMemories`，并使用 `SimilaritySearchParams.SearchQuery`。

---

## 16. 本章一句话总结

Chapter 02 的核心不是"多写几个存储接口"，而是把 agent workflow 里的状态生命周期分清楚：Session 保存当前对话线程的事件和短期状态，Memory 保存跨 session 的长期可搜索知识，Artifact 保存独立版本化文件；StateDelta 和四种前缀负责把一次 invocation 中产生的状态更新路由到正确作用域，CallbackContext 的 write-through 则保证这些更新既能被记录，也能在当前运行中立刻可见。
