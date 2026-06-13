# Chapter 06 - Entrypoint / Deploy / Telemetry 深度讲解

> 本章对应教学大纲 Chapter 06：Entrypoint / Deploy / Telemetry。
> 代码基线是本机 `rive-adk-go` 复刻版。原版 ADK 的概念会在必要处对照，但讲解以当前代码可验证行为为准。

---

## 0. 本章一句话

Chapter 06 讲的是 agent runtime 走向真实应用时最外层的生产边界：

> 同一个 agent runtime，如何用 console 调试、用 HTTP/SSE 暴露、用 dry-run deploy plan 描述云端部署、用 telemetry 记录 runner/model/tool/server 的关键行为。

前五章讲的是 runtime 内部怎么工作：

```text
Runner -> Agent -> Flow -> Model/Tool -> Event -> Session
```

第六章讲的是 runtime 外面这层：

```text
Entrypoint:
  console / web / universal launcher

Deploy:
  Cloud Run dry-run plan
  Agent Engine dry-run plan

Telemetry:
  in-memory spans/logs
  instrumentation helpers
  content capture toggle
```

本章的核心不是"怎么写 main.go"，而是：

> 入口、部署、观测都应该围绕同一套 runtime config 做协议转换，不能反向污染 Agent / Flow / Tool 的核心语义。

---

## 1. 为什么 Entrypoint / Deploy / Telemetry 是一章

一个 agent 在本地能跑，不代表它已经能成为应用。

真实使用时会出现四类问题：

```text
本地调试：
  我想在终端输入一行，看到 agent 输出。

HTTP 接入：
  前端、测试或别的服务想 POST /run 调用 agent。

云端部署：
  我要知道这个 binary 怎么打包成 Cloud Run / Agent Engine。

线上观测：
  我要知道一次 invocation、一次模型调用、一次工具执行、一次 server request 发生了什么。
```

如果每个入口都重新写一套 agent 初始化：

```text
console main:
  create agent
  create session
  create memory
  create artifact

web main:
  create agent again
  create session differently
  create memory differently

deploy main:
  guess command flags
  duplicate services

telemetry:
  deeply mixed into Flow/Tool code
```

系统会很快失控。

所以本章要建立一个边界：

```text
runtime core:
  Agent / Flow / Tool / Runner / Session

entrypoint layer:
  console / web protocol conversion

deployment layer:
  deterministic dry-run plan

observability layer:
  telemetry span/log helpers
```

这三层围绕同一个稳定协议工作：`launcher.Config`。

---

## 2. 初学者桥：一个 agent 怎么从代码变成服务

先把过程拆成四步。

### 2.1 第一步：构造 root agent

业务代码里通常先有一个 root agent：

```text
weather_bot:
  model = fake/deepseek/openai-compatible
  tools = get_weather
  flow = tool-calling loop
```

这个 agent 本身不应该知道自己将来在哪里运行：

- 终端？
- HTTP？
- Cloud Run？
- Agent Engine？
- 测试？

它只需要实现 runtime 需要的接口。

### 2.2 第二步：把 runtime 依赖放进 Config

`launcher.Config` 承载入口层需要的依赖：

```go
type Config struct {
    SessionService  runner.SessionService
    ArtifactService artifact.Service
    MemoryService   memory.Service
    AgentLoader     AgentLoader
    PluginManager   *plugin.Manager
}
```

这就是入口层和运行时层之间的稳定协议。

入口不直接 new 一堆全局单例，而是从 Config 里拿：

- root agent loader
- session service
- memory service
- artifact service
- plugin manager

### 2.3 第三步：选择入口协议

同一个 Config 可以交给不同 sublauncher：

```text
console:
  stdin line -> runner.Run -> stdout text

web:
  HTTP JSON/SSE request -> runner.Run -> JSON/SSE response

future grpc:
  gRPC request -> runner.Run -> gRPC stream
```

入口层的职责是协议转换：

```text
transport request
  -> userID/sessionID/message
  -> runner.Run
  -> event responses
  -> transport response
```

它不应该改变 agent 语义。

### 2.4 第四步：部署和观测描述外部形态

部署层在当前复刻版里不执行真实部署。

它做 dry-run plan：

```text
input config
  -> validate
  -> Dockerfile
  -> build command
  -> deploy command text
  -> local proxy / stream URL hints
```

Telemetry 层也不是完整 OTel exporter。

它做 in-memory recorder：

```text
StartInvokeAgentSpan
StartGenerateContentSpan
StartExecuteToolSpan
StartServerEventSpan
LogRequest / LogResponse / LogServerEvent
```

用来教学和测试观测结构。

---

## 3. 本章代码地图

按教学大纲，本章核心目录是：

```text
cmd/launcher/
server/adkrest/
deploy/
telemetry/
```

重点文件：

| 文件 | 重点 | 为什么读 |
| --- | --- | --- |
| `cmd/launcher/launcher.go` | `Config`、`Launcher`、`SubLauncher`、`AgentLoader` | 入口层稳定协议 |
| `cmd/launcher/universal/universal.go` | argv keyword routing | 同一 binary 多入口 |
| `cmd/launcher/console/console.go` | stdin/stdout -> `runner.Run` | 最薄本地调试入口 |
| `server/adkrest/server.go` | `/run`、`/run_sse`、`runAgent` | HTTP JSON/SSE 入口 |
| `deploy/deploy.go` | protocols、validation、plan interface | dry-run plan 公共抽象 |
| `deploy/cloudrun.go` | `PlanCloudRun`、Dockerfile、proxy command | Cloud Run plan |
| `deploy/agentengine.go` | `PlanAgentEngine`、class methods、stream URL | Agent Engine plan |
| `telemetry/telemetry.go` | `Recorder`、`Providers`、options | in-memory span/log recorder |
| `telemetry/instrumentation.go` | span/log helpers | runner/model/tool/server 观测语义 |
| `cmd/demo/main.go` | `runChapter06` | 课堂演示 |

整体结构：

```text
launcher.Config
  -> console.Run
       -> runner.New
       -> runner.Run per input line

  -> adkrest.NewServer
       -> /run JSON
       -> /run_sse SSE
       -> shared runAgent

deploy.Config
  -> PlanCloudRun / PlanAgentEngine
       -> deterministic dry-run plan

telemetry.Recorder
  -> StartedSpan / LogRecord
       -> in-memory inspection
```

---

## 4. launcher.Config：入口层和 runtime 层的稳定协议

### 4.1 Config 承载什么

`cmd/launcher/launcher.go` 里的 `Config`：

```go
type Config struct {
    SessionService  runner.SessionService
    ArtifactService artifact.Service
    MemoryService   memory.Service
    AgentLoader     AgentLoader
    PluginManager   *plugin.Manager
}
```

字段含义：

| 字段 | 作用 |
| --- | --- |
| `SessionService` | 保存 session events/state |
| `ArtifactService` | 保存版本化 artifact |
| `MemoryService` | 跨 session 长期相关性/语义检索服务 |
| `AgentLoader` | 加载 root agent |
| `PluginManager` | 可组合 hook bundles |

这样入口层就不需要知道这些服务怎么创建。

测试 `TestConfigWithServices` 验证 Config 可以承载这些服务。

### 4.2 AgentLoader：入口不直接绑定具体 agent

`AgentLoader` 只有一个方法：

```go
type AgentLoader interface {
    RootAgent() agent.Agent
}
```

Console 和 Server 都通过它拿 root agent：

```text
rootAgent := config.AgentLoader.RootAgent()
```

这让入口层和 agent 构造解耦。

入口层不关心 root agent 是：

- LLM agent
- workflow agent
- remote agent
- 测试 agent

只要后续能转成 `runner.ExecutableAgent`。

### 4.3 PluginManager 当前是协议字段，但部分入口未完全接线

大纲把 `PluginManager` 放进 `launcher.Config`，这很合理：入口层应该能把全局 plugin manager 传给 runtime。

当前复刻版里，`Console.Run` 和 `server/adkrest.runAgent` 创建 `runner.Config` 时传了：

```go
AppName
Agent
SessionService
MemoryService
ArtifactService
```

但没有把 `config.PluginManager` 继续传入 `runner.Config`。

`TestConsoleRunWithPluginManager` 只验证带 `PluginManager` 的 Config 不会导致 console 失败，并没有验证 plugin hooks 被执行。

所以本章要讲清楚：

> `PluginManager` 是入口协议里预留/承载的 runtime 依赖；当前复刻版 console/server 路径还没有把它完整接入 runner/flow hook 执行链。教学时不要把它讲成已经在所有入口自动生效。

这是一个很适合作为后续练习的小补丁点。

---

## 5. Launcher / SubLauncher：入口能力单元

### 5.1 Launcher 接口

`Launcher` 是顶层入口：

```go
type Launcher interface {
    Execute(ctx context.Context, config *Config, args []string) error
    CommandLineSyntax() string
}
```

它负责：

- 解析 argv。
- 选择入口模式。
- 执行对应 sublauncher。

### 5.2 SubLauncher 接口

`SubLauncher` 是一个可组合入口能力：

```go
type SubLauncher interface {
    Keyword() string
    Parse(args []string) ([]string, error)
    Run(ctx context.Context, config *Config) error
    CommandLineSyntax() string
    SimpleDescription() string
}
```

一个 sublauncher 可以是：

```text
console
web
grpc
batch
worker
```

只要实现这些方法，就能挂进 universal launcher。

### 5.3 为什么 Parse 和 Run 分开

`Parse(args)` 负责处理该 sublauncher 自己的 flags。

`Run(ctx, config)` 负责真正执行。

这样 universal launcher 可以做统一路由：

```text
argv[0] -> sublauncher keyword
argv[1:] -> sublauncher.Parse
sublauncher.Run(ctx, config)
```

---

## 6. Universal launcher：同一 binary 多入口

### 6.1 默认路由

`universal.New(sublauncher...)` 返回一个顶层 launcher。

规则：

```text
如果没有 args:
  选第一个 sublauncher 作为默认入口

如果 args[0] 匹配某个 sublauncher.Keyword():
  选它，并把 args[1:] 交给 Parse

否则:
  unknown command error
```

这就是大纲里的：

```text
universal.New(console, web)

no args -> console.Run()
"web"   -> web.Run()
```

### 6.2 顺序决定默认入口

如果想让 web 成为默认：

```go
universal.New(web, console)
```

因为第一个 sublauncher 是默认。

测试：

- `TestDefaultSubLauncherSelection`
- `TestKeywordBasedRouting`
- `TestParseRoutesToFirstWhenNoArgs`

### 6.3 错误行为

Universal launcher 会检查：

- 没有 sublauncher：`universal: no sublaunchers registered`
- duplicate keyword：报错
- unknown command：报错并列出 available keywords
- sublauncher run error：向上返回

测试：

- `TestNoSubLaunchersError`
- `TestDuplicateKeywordsError`
- `TestUnknownCommandError`
- `TestSubLauncherRunErrorPropagation`

---

## 7. Console：最薄的 runner 包装

### 7.1 Console 做什么

`cmd/launcher/console/console.go` 的 `Run`：

```text
1. 检查 AgentLoader
2. 如果 SessionService nil，使用 runner.NewInMemorySessionService()
3. RootAgent() -> ExecutableAgent
4. runner.New(...)
5. 从 input 逐行读取
6. trim 空白，跳过空行
7. 每行调用 runner.Run(ctx, userID, sessionID, line)
8. 把 events 打印到 output
```

默认身份：

```go
defaultAppName = "console_app"
defaultUserID  = "console_user"
sessionID      = "console-session-1"
```

所以多行输入默认进入同一个 session。

### 7.2 Console 是调试入口，不是独立 runtime

Console 不重新实现 Flow。

它只是：

```text
stdin line
  -> runner.Run
  -> print model/tool/user parts
```

这就是为什么 `TestConsoleRunToolChain` 能看到完整 tool chain：

```text
user + model function call + tool response + final model text
```

Console 只是把这些 events 打印出来。

### 7.3 多条消息复用 session

`TestConsoleRunMultipleMessages` 验证：

```text
input:
  msg1
  msg2

same sessionID:
  console-session-1

session events:
  user+model
  user+model
```

所以 console 不是每行开一个全新 session。

它适合本地调试 multi-turn behavior。

### 7.4 空行跳过

Console 会：

```go
line := strings.TrimSpace(scanner.Text())
if line == "" {
    continue
}
```

`TestConsoleRunEmptyLinesSkipped` 验证空行不会触发 runner。

### 7.5 Console 边界

当前复刻版 console：

- 不解析 flags。
- 不做认证。
- 不做 streaming UI。
- 不处理复杂终端交互。

它就是教学版最薄 wrapper。

---

## 8. ADK REST Server：JSON 和 SSE 双协议

### 8.1 Server 也从 launcher.Config 拿依赖

`server/adkrest.NewServer(cfg)` 会验证：

```text
cfg != nil
cfg.AgentLoader != nil
```

它内部注册两个 endpoint：

```text
POST /run
POST /run_sse
```

并用：

```text
appName = "adkrest_app"
```

作为默认 runner app name。

测试里可以通过 `SetAppName` 改掉。

### 8.2 /run：HTTP JSON request/response

`runHandler` 做：

```text
1. 只允许 POST
2. decodeRunRequest
3. runAgent
4. EventToResponse
5. write JSON array
```

request 必填字段：

```text
appName
userId
sessionId
newMessage
```

decoder 使用：

```go
d.DisallowUnknownFields()
```

所以未知字段会报 400。

测试：

- `TestRunHandlerHappyPath`
- `TestRunHandlerMissingFields`
- `TestRunHandlerMethodNotAllowed`
- `TestMalformedRequestDoesNotCorruptLaterRequests`

### 8.3 /run_sse：SSE framing

`runSSEHandler` 做：

```text
1. 只允许 POST
2. decodeRunRequest
3. 检查 http.Flusher
4. 设置 Content-Type: text/event-stream
5. runAgent
6. 每个 event marshal JSON
7. 写 "data: <json>\n\n"
8. 每帧 Flush
```

测试：

- `TestRunSSEHandlerHappyPath`
- `TestRunSSEMultipleEvents`
- `TestSSEErrorEvent`

注意当前复刻版不是边 runner 边流式 emit。

`runAgent` 先返回 `[]*event.Event`，然后 SSE handler 逐个写出。

所以这是 SSE framing 教学版，不是完整 live streaming runner。

### 8.4 runAgent：JSON 和 SSE 共享核心逻辑

`runAgent` 是 server 里最重要的复用点：

```text
RootAgent()
  -> ExecutableAgent
  -> sessionSvc or default InMemory
  -> runner.New
  -> runner.Run(ctx, userID, sessionID, message)
  -> events
```

JSON 和 SSE 都走这里。

这保证协议不同，但 runtime 逻辑一致。

### 8.5 Session persistence across requests

`TestSessionReuseAcrossRequests` 验证同一个 `sessionId` 连续请求会复用 session。

这和 console 多行输入复用 session 是同一个原则：

```text
transport differs
session semantics same
```

---

## 9. Deploy：dry-run plan，不是真实部署器

### 9.1 为什么用 dry-run plan

部署是最容易把教程写乱的地方。

真实部署会涉及：

- gcloud auth
- Docker build
- Cloud Run API
- Vertex AI API
- secret manager
- network
- IAM

这些不是 runtime 教学的重点。

当前复刻版选择：

```text
不执行任何外部命令
不访问网络
只生成确定性的 plan
```

这有两个好处：

1. 测试稳定。
2. 教材能把部署步骤讲清楚，而不依赖环境。

所以要反复强调：

> deploy package 是 dry-run snapshot，不是真正的 gcloud/docker 执行器。

### 9.2 Plan 接口

`deploy.Plan` 只有：

```go
type Plan interface {
    String() string
    Lines() []string
}
```

Plan 是一个可打印、可检查的部署说明。

### 9.3 validation

公共校验包括：

- `ValidateEntryPoint`: 非空且以 `.go` 结尾。
- `ValidateProjectName`: 非空。
- `ValidateRegion`: 非空。
- `ValidateServiceName`: 非空。
- `ValidateProtocols`: protocol 必须已知。
- `ValidateServerPort`: 1 到 65535。
- `ValidateAll`: 聚合多个错误。
- `StripExtension`: 去掉 `.go` 后缀得到 binary path。

测试：

- `TestValidateEntryPoint`
- `TestValidateProjectName`
- `TestValidateProtocols`
- `TestValidateAll`
- `TestStripExtension`
- `TestCloudRunPlanValidationErrors`
- `TestAgentEnginePlanValidationErrors`

---

## 10. Cloud Run Plan

### 10.1 输入 config

`CloudRunConfig`：

```go
type CloudRunConfig struct {
    EntryPoint   string
    Project      string
    Region       string
    ServiceName  string
    ServerPort   int
    ProxyPort    int
    Protocols    []Protocol
    A2AAgentURL  string
    WebUIAddress string
}
```

默认值：

```text
ServerPort   = 8080
ProxyPort    = 8081
A2AAgentURL  = http://127.0.0.1:8081
WebUIAddress = http://127.0.0.1:8081/api
```

`TestCloudRunPlanDefaults` 验证默认端口。

### 10.2 输出 plan

`CloudRunPlan` 包含：

```text
entry point
binary path
project / region / service name
server port / proxy port
protocols
A2A URL
WebUI address
Dockerfile
build command
proxy command
human-readable lines
```

`TestCloudRunPlanDeterministic` 验证关键字段和 Dockerfile markers。

`TestCloudRunPlanStringIsDeterministic` 验证相同输入输出相同 plan string。

### 10.3 Dockerfile

Cloud Run Dockerfile 是 distroless：

```text
FROM gcr.io/distroless/static-debian11
COPY <exec> /app/<exec>
EXPOSE <serverPort>
CMD ["/app/<exec>", "web", "-port", "<serverPort>", ...protocol flags]
```

根据 protocols 添加：

| Protocol | CMD flags |
| --- | --- |
| `api` | `"api", "-webui_address", "<webuiAddress>"` |
| `a2a` | `"a2a", "--a2a_agent_url", "<a2aAgentURL>"` |
| `webui` | `"webui", "--api_server_address", "<webuiAddress>"` |
| `pubsub` | `"pubsub"` |
| `eventarc` | `"eventarc"` |

`TestCloudRunPlanAllProtocols` 验证所有 protocol 都进入 Dockerfile。

### 10.4 Build / Deploy / Proxy 文本

Cloud Run plan 会生成：

```text
go build -ldflags "-s -w" -o <exec> <entrypoint>
```

以及：

```text
gcloud run services proxy <service> --project <project> --port <proxyPort> --region <region>
```

注意 deploy command 只是 plan 文本，不会执行。

---

## 11. Agent Engine Plan

### 11.1 输入 config

`AgentEngineConfig`：

```go
type AgentEngineConfig struct {
    EntryPoint   string
    Project      string
    Region       string
    Name         string
    ServerPort   int
    SourceDir    string
    ClassMethods []ClassMethod
    MemoryBank   bool
    MemoryModel  string
    MemoryTTL    time.Duration
}
```

默认值：

```text
ServerPort = 8080
SourceDir  = "."
```

### 11.2 Dockerfile 是 multi-stage

Agent Engine Dockerfile：

```text
FROM golang:1.24 as builder
WORKDIR /app
COPY . .
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build ...

FROM gcr.io/distroless/static-debian11
COPY --from=builder /app/<exec> /app/<exec>
EXPOSE <serverPort>
CMD ["/app/<exec>", "web", "-port", "<serverPort>", "agentengine"]
```

`TestAgentEnginePlanDeterministic` 验证 Dockerfile 包含 builder、distroless、`agentengine`。

### 11.3 Class methods

如果没有传 `ClassMethods`，plan 输出默认方法：

```text
async_create_session
async_get_session
async_list_sessions
async_delete_session
async_stream_query
```

如果传了自定义 methods，则按传入内容打印。

`TestAgentEnginePlanDefaultClassMethods` 验证默认 method。

### 11.4 Memory Bank

如果配置：

```go
MemoryBank: true
MemoryModel: "publishers/google/models/gemini-2.5-flash"
MemoryTTL: ...
```

plan 会输出 Memory Bank section。

`TestAgentEnginePlanWithMemoryBank` 验证这个行为。

注意这只是 plan 文本。

它不真的创建 Memory Bank。

### 11.5 Stream URL

Agent Engine plan 会生成：

```text
https://<region>-aiplatform.googleapis.com/v1beta1/projects/<project>/locations/<region>/reasoningEngines/<name>:streamQuery
```

`TestAgentEnginePlanDeterministic` 验证 URL 包含 region、project、name、`streamQuery`。

---

## 12. Telemetry：in-memory recorder，不是完整 OTel

### 12.1 Recorder 是线程安全累加器

`telemetry.Recorder` 持有：

```go
spans []SpanRecord
logs  []LogRecord
```

并用 mutex 保护。

它提供：

```text
Spans()
Logs()
SpanCount()
LogCount()
Reset()
CaptureMessageContent()
```

`TestRecorderConcurrency` 验证并发记录 span/log 不丢数据。

### 12.2 SpanRecord

`SpanRecord` 包含：

```go
Name       string
StartTime  time.Time
EndTime    time.Time
Attributes map[string]any
Status     string
Error      string
```

它模拟 OTel span 的形状，但不依赖 OTel SDK。

### 12.3 LogRecord

`LogRecord` 包含：

```go
EventName  string
Timestamp  time.Time
Attributes map[string]any
Body       map[string]any
```

用来记录 request/response/server event。

---

## 13. 四类 span helper

### 13.1 invoke_agent

```go
StartInvokeAgentSpan(ctx, r, agentName, agentDesc, sessionID, invocationID)
```

span name：

```text
invoke_agent <agentName>
```

attributes：

```text
gcp.vertex.agent.invocation_id
gen_ai.operation.name = invoke_agent
gen_ai.agent.description
gen_ai.agent.name
gen_ai.conversation.id
```

`TestRecorderSpanRecording` 验证这些字段。

### 13.2 generate_content

```go
StartGenerateContentSpan(ctx, r, modelName, invocationID)
```

span name：

```text
generate_content <modelName>
```

attributes：

```text
gcp.vertex.agent.invocation_id
gen_ai.operation.name = generate_content
gen_ai.request.model
```

可以再调用：

```go
SetEventID(span, eventID)
SetTokenUsage(span, promptTokens, candidatesTokens, cachedTokens, thoughtsTokens)
```

写入：

```text
gcp.vertex.agent.event_id
gen_ai.usage.input_tokens
gen_ai.usage.output_tokens
gen_ai.usage.cache_read.input_tokens
gen_ai.usage.reasoning.output_tokens
```

### 13.3 execute_tool

```go
StartExecuteToolSpan(ctx, r, toolName, args)
```

span name：

```text
execute_tool <toolName>
```

attributes：

```text
gen_ai.operation.name = execute_tool
gen_ai.tool.name
gcp.vertex.agent.tool_call_args
```

`tool_call_args` 使用 JSON 字符串存储。

如果不可序列化，`safeJSON` 返回：

```text
<not serializable>
```

### 13.4 server

```go
StartServerEventSpan(ctx, r, "POST", "/run_sse")
```

span name：

```text
server POST /run_sse
```

attributes：

```text
server.operation
server.path
```

`TestServerEventSpan` 验证这个 helper。

### 13.5 End 和 EndWithError

span 开始后必须结束：

```go
span.End("OK")
span.EndWithError("ERROR", err.Error())
```

`EndWithError` 会把 `Status` 写成 `ERROR`，并记录 error string。

`TestSpanErrorRecording` 验证这个行为。

---

## 14. Telemetry logs 和内容捕获

### 14.1 默认不记录消息正文

`NewRecorder()` 默认：

```text
captureMessageContent = false
```

所以：

```go
LogRequest(ctx, r, "secret system prompt", "secret user message")
```

记录的 content 是：

```text
<elided>
```

`TestLogRequestElided` 和 `TestLogResponseElided` 验证默认 elision。

这是生产观测里非常重要的安全默认值。

### 14.2 WithCaptureMessageContent

如果显式开启：

```go
telemetry.NewRecorder(telemetry.WithCaptureMessageContent(true))
```

则 request/response 正文会写入 logs。

`TestLogRequest` 和 `TestLogResponse` 验证这个行为。

这对应 ADK / OTel 里常见的：

```text
OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT
```

语义。

### 14.3 LogRequest

```go
LogRequest(ctx, r, systemMessage, userMessages...)
```

会生成：

```text
gen_ai.system.message
gen_ai.user.message
```

### 14.4 LogResponse

```go
LogResponse(ctx, r, finishReason, content, toolCalls)
```

会生成：

```text
gen_ai.choice
```

body 里可能有：

```text
content
finish_reason
tool_calls
index
```

### 14.5 LogServerEvent

```go
LogServerEvent(ctx, r, "POST", "/run", 200, 15*time.Millisecond)
```

记录：

```text
server.request
http.method
http.path
http.status_code
http.duration_ms
```

`TestLogServerEvent` 验证字段。

---

## 15. Providers 生命周期

`Providers` 包了一层 recorder：

```go
type Providers struct {
    recorder *Recorder
}
```

提供：

```go
Init(ctx)
Shutdown(ctx)
Recorder()
```

当前复刻版行为：

- `Init` 会 `Reset` recorder。
- `Shutdown` 是 no-op，不清空数据。
- 数据仍可用于测试检查。

测试：

- `TestProvidersInitShutdown`
- `TestProvidersShutdownKeepsData`

这和真实 exporter 的 flush/shutdown 语义不同。

当前版本是教学替代：

> 保留数据，便于检查 span/log 是否正确生成。

---

## 16. 课堂演示

`cmd/demo/main.go` 的 `runChapter06` 有三个 demo。

### 16.1 demoLauncherConfig

演示：

```text
launcher.Config fields
SubLauncher interface
Universal launcher routing table
```

重点讲：

- 入口层持有 Config，不直接改 runtime。
- no args 默认第一个 sublauncher。
- keyword 选择 console/web。

### 16.2 demoDeployPlans

演示：

```text
Cloud Run dry-run plan
Agent Engine dry-run plan
Dockerfile
build command
proxy command
stream query endpoint
```

重点讲：

- deterministic dry-run。
- 不执行 gcloud/docker/network。
- CMD 调 web launcher，并追加 protocol flags。

### 16.3 demoTelemetryInstrumentation

演示：

```text
StartInvokeAgentSpan
StartServerEventSpan
runner.Run
StartGenerateContentSpan
SetTokenUsage
LogRequest
LogResponse
End spans
print recorder spans/logs
```

重点讲：

- span 是围绕 invocation/model/server 的观测结构。
- logs 默认可以 elide message content。
- Recorder 是 in-memory/thread-safe。

---

## 17. 容易误解点

### 17.1 误区：launcher.Config 是全局单例

不是。

它只是一个 struct。

每个入口可以持有自己的 Config，也可以在测试里构造自己的 Config。

### 17.2 误区：Console 和 Web 必须是两个 binary

不是。

Universal launcher 可以在同一个 binary 里通过 argv keyword 路由：

```text
no args -> console
web     -> web
```

### 17.3 误区：Deploy package 会真的部署

不会。

它只生成 dry-run plan。

真正执行 gcloud/docker/Vertex AI API 需要用户按 plan 去做。

### 17.4 误区：SSE endpoint 就是完整 streaming runtime

当前复刻版不是。

它先 `runAgent` 得到完整 events slice，再逐个写 SSE frame。

这适合教学 SSE framing，但不等于边模型生成边实时推送。

### 17.5 误区：Telemetry Recorder 就是 OpenTelemetry

不是。

它是 in-memory 教学替代，字段形状参考 OTel/gen_ai 语义，但没有 exporter、resource、trace context propagation。

### 17.6 误区：开启 telemetry 默认会记录 prompt/response 正文

不会。

默认 content 是 `<elided>`。

只有 `WithCaptureMessageContent(true)` 才会记录正文。

### 17.7 误区：PluginManager 在 Config 里就代表所有入口都执行 plugin

当前复刻版不能这么讲。

Config 里有字段，但 console/server 目前没有把它完整传入 runner/flow 执行链。它是稳定协议的一部分，也是未来接线点。

---

## 18. 源码行为对照表

| 行为 | 当前复刻版结论 | 证据 |
| --- | --- | --- |
| Config 承载入口依赖 | session/artifact/memory/agent loader/plugin manager | `launcher.go`、`TestConfigWithServices` |
| AgentLoader | `RootAgent()` 提供 root agent | `launcher.go`、console/server tests |
| Universal 默认入口 | 无 args 选第一个 sublauncher | `universal.go`、`TestDefaultSubLauncherSelection` |
| Universal keyword routing | `args[0]` 匹配 keyword | `TestKeywordBasedRouting` |
| Console input | 每个非空行调用 `runner.Run` | `console.go`、`TestConsoleRunMultipleMessages` |
| Console default session | nil session service 时用 in-memory | `TestConsoleDefaultSessionService` |
| Console tool chain | function call/tool response/final text 都能打印和持久化 | `TestConsoleRunToolChain` |
| REST JSON | `POST /run` 返回 event response JSON array | `TestRunHandlerHappyPath` |
| REST strict decode | unknown/malformed/missing fields 返回 400 | `decodeRunRequest`、server tests |
| SSE | `POST /run_sse` 写 `data: <json>\n\n` frames | `TestRunSSEHandlerHappyPath`、`TestRunSSEMultipleEvents` |
| Server session reuse | 同 sessionId 跨 request 复用 session | `TestSessionReuseAcrossRequests` |
| Cloud Run plan | deterministic dry-run Dockerfile/build/proxy text | `TestCloudRunPlanDeterministic` |
| Cloud Run defaults | server 8080 / proxy 8081 | `TestCloudRunPlanDefaults` |
| Cloud Run protocols | enabled protocols 进入 CMD flags | `TestCloudRunPlanAllProtocols` |
| Agent Engine plan | multi-stage Dockerfile + stream URL | `TestAgentEnginePlanDeterministic` |
| Agent Engine default class methods | 无自定义时输出默认 methods | `TestAgentEnginePlanDefaultClassMethods` |
| Agent Engine Memory Bank | 配置后输出 Memory Bank section | `TestAgentEnginePlanWithMemoryBank` |
| Telemetry spans | invoke/generate/tool/server span helpers | `TestRecorderSpanRecording`、`TestServerEventSpan` |
| Telemetry errors | `EndWithError` 记录 ERROR status/error | `TestSpanErrorRecording` |
| Telemetry content capture | 默认 `<elided>`，显式开启才记录正文 | `TestLogRequestElided`、`TestLogRequest` |
| Providers lifecycle | Init resets，Shutdown keeps data for inspection | `TestProvidersInitShutdown`、`TestProvidersShutdownKeepsData` |
| Recorder concurrency | mutex 保证并发记录数量正确 | `TestRecorderConcurrency` |

---

## 19. 建议课堂脚本

### 19.1 先讲 runtime 到应用的距离

开场问题：

```text
Agent 在测试里能跑了，怎么给用户用？
```

引出四个入口：

```text
console
HTTP JSON
SSE
deploy/telemetry
```

### 19.2 再画 Config 协议

白板画：

```text
launcher.Config
  AgentLoader
  SessionService
  MemoryService
  ArtifactService
  PluginManager

console / web / future grpc
  all consume same Config
```

然后强调：

> runtime 不感知自己在哪里被调用。

### 19.3 接着读入口代码

推荐顺序：

1. `cmd/launcher/launcher.go`
2. `cmd/launcher/universal/universal.go`
3. `cmd/launcher/console/console.go`
4. `server/adkrest/server.go`

读完后问：

```text
console 和 /run 的共同点是什么？
```

答案：

```text
都只是把输入变成 runner.Run 参数，再把 events 变成对应 transport 输出。
```

### 19.4 再读 deploy

读：

1. `deploy/deploy.go`
2. `deploy/cloudrun.go`
3. `deploy/agentengine.go`

强调：

```text
dry-run
deterministic
no external command
deployment as documentation
```

### 19.5 最后读 telemetry

读：

1. `telemetry/telemetry.go`
2. `telemetry/instrumentation.go`
3. `telemetry/telemetry_test.go`

强调：

```text
span helper 代表语义
recorder 只是教学替代
message content 默认 elide
```

---

## 20. 练习题

### 20.1 练习一：新增 grpc sublauncher

目标：

```text
实现一个 SubLauncher，Keyword() = "grpc"
```

要求：

- `Parse(args)` 接收 `-port`。
- `Run(ctx, config)` 检查 `AgentLoader`。
- 挂进 `universal.New(console, grpc)`。
- 测试 `args[0]="grpc"` 时路由到 grpc。

讨论：

- 如果把 `grpc` 放第一个，默认入口会变成什么？

### 20.2 练习二：验证 console session 复用

目标：

```text
输入两行消息
验证同一个 sessionID 内 event count 累加
```

参考：

- `TestConsoleRunMultipleMessages`

讨论：

- 如果每行都新建 session，会破坏什么能力？

### 20.3 练习三：写一个 REST bad request 测试

目标：

```text
POST /run with unknown field
expect 400
then send valid request
expect success
```

参考：

- `TestMalformedRequestDoesNotCorruptLaterRequests`

讨论：

- 为什么 decoder 要用 `DisallowUnknownFields`？

### 20.4 练习四：生成 Cloud Run plan

目标：

```text
EntryPoint = "cmd/myserver/main.go"
Protocols = api + webui + a2a
```

要求：

- 验证 Dockerfile 包含 `"api"`、`"webui"`、`"a2a"`。
- 验证 String() 对相同输入 deterministic。
- 验证不会执行任何外部命令。

### 20.5 练习五：Telemetry content capture

目标：

```text
Recorder without capture:
  LogRequest -> <elided>

Recorder with capture:
  LogRequest -> original text
```

要求：

- 同时测试 request 和 response。
- 说明为什么默认要 elide。

### 20.6 练习六：PluginManager 接线补丁设计

目标：

```text
让 launcher.Config.PluginManager 真正进入 runner/flow hook 执行路径
```

要求先回答：

- `runner.Config` 当前是否有 PluginManager 字段？
- 如果没有，应加在哪里？
- 如果 flow 已经有 PluginManager，应由 agent/runner 哪一层注入？
- 如何写测试证明 console/web entrypoint 下 plugin hook 执行了？

这个练习适合讲"协议字段"和"实际接线"的差异。

---

## 21. 自测题

1. `launcher.Config` 的五个字段分别是什么？
2. 为什么说 `launcher.Config` 是入口层和 runtime 层之间的稳定协议？
3. Universal launcher 在没有 args 时选择哪个 sublauncher？
4. Console 每一行输入最终调用哪个 runtime 方法？
5. `/run` 和 `/run_sse` 的共同核心逻辑在哪里？
6. 当前复刻版 `/run_sse` 是否是 live streaming runner？
7. Cloud Run deploy package 会不会真的执行 gcloud？
8. Cloud Run plan 的 Dockerfile CMD 为什么包含 `"web"`？
9. Agent Engine plan 的默认 class methods 有哪些？
10. Telemetry Recorder 默认会不会记录消息正文？
11. `StartExecuteToolSpan` 会记录哪些关键属性？
12. `Providers.Shutdown` 当前会不会清空 recorder 数据？

参考答案：

1. `SessionService`、`ArtifactService`、`MemoryService`、`AgentLoader`、`PluginManager`。
2. 因为 console/web/future entrypoints 都从它拿 runtime 依赖，而不直接修改 Agent/Flow/Tool。
3. 第一个注册的 sublauncher。
4. `runner.Run`。
5. `server.runAgent`。
6. 不是。当前是先收集 `[]*event.Event`，再逐个写 SSE frames。
7. 不会。它只生成 deterministic dry-run plan。
8. 因为部署后的 binary 通过 web launcher 启动 HTTP/protocol 服务。
9. `async_create_session`、`async_get_session`、`async_list_sessions`、`async_delete_session`、`async_stream_query`。
10. 默认不会，content 是 `<elided>`。
11. operation name、tool name、JSON 序列化后的 tool args。
12. 不会。当前保留数据以便测试 inspection。

---

## 22. 本章收束

Chapter 06 的核心是把 agent runtime 放进真实应用边界里，但保持边界清楚：

```text
Entrypoint:
  把 stdin / HTTP / SSE 输入转换成 runner.Run。

Deploy:
  把入口参数转换成 deterministic dry-run plan。

Telemetry:
  把 invocation/model/tool/server 语义记录成 spans/logs。
```

真正要记住的是：

> 入口层、部署层、观测层都应该围绕 runtime 的稳定协议工作，而不是把 console/web/cloud/telemetry 细节写进 Flow 或 Tool。

这就是第六章在整套 ADK Go 教材里的位置：它把前五章的 runtime 能力变成可调试、可暴露、可部署、可观测的应用形态。
