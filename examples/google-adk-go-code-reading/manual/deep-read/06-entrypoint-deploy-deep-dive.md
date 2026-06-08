# Google ADK Go 精读：CLI / Server / Deploy / Telemetry / Examples

> 范围：`cmd/adkgo/**`、`cmd/launcher/**`、`cmd/internal/**`、`server/adkrest/**`、`server/adka2a/**`、`server/agentengine/**`、`telemetry/**`、`internal/telemetry/**`、`util/aiplatform/**`、`util/vertexai/**`、`internal/cli/**`、`examples/**`、`.github/workflows/**`、`scripts/**`。
>
> 基线：`81a63d8feb7d713b1731f0c740d95574eb64dafa`。

## problem

ADK Go 的核心问题不是“如何写一个 agent”，而是“如何把同一个 agent 运行时稳定地暴露到多种入口”。从代码看，它至少要覆盖七类入口：

1. 本地交互：`cmd/launcher/console` 用 stdin/stdout 驱动 `runner.Run`，适合开发调试。
2. 本地 Web/API：`cmd/launcher/web` 组合 `webui`、`api`、`a2a`、`pubsub`、`eventarc` 等 sublauncher，适合 demo、调试和轻量服务。
3. ADK REST：`server/adkrest` 暴露 app/session/runtime/artifact/debug API，把 `runner.Run` 转成 JSON、SSE、WebSocket。
4. A2A：`cmd/launcher/web/a2a` 和 `server/adka2a` 把 ADK agent 包成 A2A agent card 和 JSON-RPC executor。
5. Agent Engine：`server/agentengine` 和 `cmd/launcher/web/agentengine` 对齐 Vertex Reasoning Engine 的 query/streamQuery 方法模型。
6. 部署工具：`cmd/adkgo/internal/deploy/cloudrun` 和 `cmd/adkgo/internal/deploy/agentengine` 把本地 Go entry point 编译、打包、上传到 Cloud Run 或 Agent Engine。
7. 可观测性：`telemetry`、`internal/telemetry`、`cmd/launcher/internal/telemetry` 把 runner / model / tool / server trace 接入 OpenTelemetry 和 GCP exporter。

因此这层代码承担的是“产品化边界”：开发者写的 agent 代码应该尽量不关心自己是在 console、REST API、A2A、Cloud Run 还是 Agent Engine 中运行；ADK Go 则负责把运行环境差异转换成统一的 `launcher.Config`、`runner.Config`、`session.Service`、`artifact.Service`、`memory.Service`、`plugin` 和 telemetry provider。

## why_hard

这部分难点在于入口不是简单的 adapter，而是多个生命周期和协议模型叠在一起：

1. 本地 console 是长连接交互。`console.Run` 要处理 stdin EOF、SIGINT、streaming mode、会话创建、runner 生命周期和 telemetry shutdown。
2. REST/SSE/WebSocket 是 HTTP request/response 生命周期。`server/adkrest/controllers.RuntimeAPIController` 需要校验 session、创建 runner、把 `session.Event` 编码成 JSON/SSE/WebSocket frame，并且不能让一次事件编码失败递归污染后续流。
3. A2A 是任务协议。ADK 的 `session.Event` 要转成 A2A artifact/status/update event，agent metadata 要转成 agent card，错误要变成 task failed，而不是普通 HTTP 500。
4. Agent Engine 是云平台 class method 模型。它需要向 Vertex 报告 `async_create_session`、`async_get_session`、`async_list_sessions`、`async_delete_session`、`async_stream_query` 等 method metadata，还要在部署时把 method schema 写进 `ReasoningEngineSpec.ClassMethods`。
5. Cloud Run 部署是容器和 HTTP 入口模型。它要求把 entry point 静态编译成 linux/amd64 executable，生成 distroless Dockerfile，再用 `gcloud run deploy --source .` 部署，并通过本地 `gcloud run services proxy` 做鉴权代理。
6. Agent Engine 部署是 source archive + Reasoning Engine API 模型。它不是直接推镜像，而是打包 source archive、生成 Dockerfile、用 `CreateReasoningEngine` 或 `UpdateReasoningEngine` 提交 inline source。
7. 可观测性既要支持本地 OTLP，也要支持 GCP telemetry.googleapis.com。`telemetry.configure` 必须解析 ADC、quota project、resource project、GCP resource detector、OTLP endpoint，并把 trace/log provider 安装为全局 provider。
8. examples 既是测试资产，也是能力目录。`examples/README.md` 明确说这些示例偏 minimal，用来测试 one or few scenarios，而不是完整客户样例；但用户会自然把 examples 当“怎么用 ADK”的入口，所以 drift 风险很高。

这说明 entrypoint/deploy 层的主要工程挑战是“保持统一运行时语义，同时允许每个入口保留自己的协议形态”。

## design_approach

### 1. `launcher` 把入口选择做成可组合 sublauncher

`cmd/launcher/launcher.go` 定义了两个关键接口：

- `Launcher`：外层入口，负责 `Execute(ctx, config, args)`。
- `SubLauncher`：可组合子入口，提供 `Keyword()`、`Parse()`、`Run()`、`CommandLineSyntax()`、`SimpleDescription()`。

`cmd/launcher/universal` 是路由器：它把第一个 argv token 解释成 sublauncher keyword；如果没有命中，就用第一个 sublauncher 作为默认入口。`cmd/launcher/full` 把 console、web、webui、a2a、pubsub、eventarc、api 全部组合起来；`cmd/launcher/prod` 则只保留 REST API 和 A2A。

这个设计的价值是入口能力可以局部扩展。新增一个 web 子协议不需要改 console，也不需要改 runner；只要实现 `web.Sublauncher` 并挂进 `web.NewLauncher(...)`。

### 2. `launcher.Config` 是入口层和运行时层之间的稳定协议

`launcher.Config` 包含：

- `SessionService`
- `ArtifactService`
- `MemoryService`
- `AgentLoader`
- `A2AOptions`
- `PluginConfig`
- `TelemetryOptions`

console、REST、A2A、Agent Engine 最终都会从这里取 agent、session、memory、artifact 和 plugin 配置，然后构造 `runner.Runner`。这让服务端代码不必知道 agent 是 hard-coded Go factory、YAML configurable agent、conformance loader，还是外部加载器。

### 3. Web launcher 是 HTTP 容器，具体功能由 subrouter 注入

`cmd/launcher/web/web.go` 本身只负责：

- 解析通用 web flag：port、read/write/idle timeout、shutdown timeout、`otel_to_cloud`。
- 创建 base router。
- 调用 active sublauncher 的 `SetupSubrouters`。
- 初始化 telemetry provider。
- 启动 `http.Server` 并处理 graceful shutdown。

具体协议由 sublauncher 注入：

- `webui`：嵌入静态 UI，并在运行时生成 `/assets/config/runtime-config.json`。
- `api`：创建 `adkrest.NewServer`，设置 CORS 和 path prefix。
- `a2a`：生成 agent card，注册 A2A JSON-RPC handler。
- `agentengine`：注册 Reasoning Engine query/streamQuery handler。
- `pubsub` / `eventarc`：作为 webhook trigger router 进入同一个 HTTP server。

这是一种“HTTP shell + protocol modules”的设计。

### 4. 部署 CLI 复用 launcher 入口，而不是另写云端 runner

Cloud Run 的 Dockerfile 最终 command 是：

```text
/app/<exec> web -port <server_port> [api] [a2a] [webui] [pubsub] [eventarc]
```

Agent Engine 的 Dockerfile 最终 command 是：

```text
/app/<exec> web -port <server_port> agentengine
```

也就是说部署工具没有复制 runner 逻辑；它只是把本地 entry point 编译成适合云平台启动的 binary，然后仍然通过 launcher 体系选择 web sublauncher。这个设计降低了“本地行为”和“云端行为”的语义差异。

### 5. Telemetry 分两层：public config 和 internal instrumentation

- `telemetry` public package 负责 option、provider 初始化、GCP/OTLP exporter、resource resolution。
- `internal/telemetry` 负责具体 span 语义：invoke agent、generate content、execute tool、usage token、event id、tool args/response。
- `cmd/launcher/internal/telemetry` 是 launcher 专用 glue：把 `launcher.Config.TelemetryOptions` 加上 `WithOtelToCloud`，初始化 providers 并安装全局 OTel provider。

这个分层让 runner/model/tool 层可以只关心“记录什么 span”，入口层可以只关心“是否启用 GCP export / 如何 shutdown”。

## code_walkthrough

### `cmd/adkgo/adkgo.go` 与 `cmd/adkgo/internal/root/root.go`

`cmd/adkgo/adkgo.go` 是独立 CLI binary。它通过 blank import 注册 deploy 子命令：

- `_ "google.golang.org/adk/cmd/adkgo/internal/deploy/agentengine"`
- `_ "google.golang.org/adk/cmd/adkgo/internal/deploy/cloudrun"`

然后调用 `root.Execute()`。`root.RootCmd` 是一个 `cobra.Command`，描述为 “CLI tool for use with ADK-GO”。deploy 子命令通过各自 package 的 `init()` 挂到 `root.RootCmd`。

这个路径说明：`adkgo` CLI 目前偏部署/测试辅助，不是应用运行时主入口。应用运行时主入口通常是用户自己的 `main.go`，里面调用 `full.NewLauncher()` 或 `prod.NewLauncher()`。

### `cmd/internal/adkcli/main.go`

`cmd/internal/adkcli/main.go` 是 configurable / conformance 风格入口。它从当前目录递归扫描 `root_agent.yaml`，通过 `configurable.FromConfig` 加载 agent，并把所有 agent 放进 `ConformanceAgentLoader`。

关键点：

- 它先注册 conformance callbacks 和 functions。
- 它以目录名作为 app/agent key。
- 它默认安装 `replayplugin` 和 `recordplugin`。
- 它最后调用 `full.NewLauncher().Execute(...)`。

这条路径说明 ADK Go 有一个“配置驱动 agent”的入口，适合 conformance test、record/replay、样例运行，而不要求用户手写 Go factory。

### `cmd/launcher/launcher.go`

这里的 `Config` 是 entrypoint 层最核心的结构。它把所有服务依赖和 runtime hook 统一带到 launcher/sublauncher 中。console、REST、A2A、Agent Engine 并不直接创建自己的全局单例，而是通过 `Config` 拿到服务。

`Launcher` 与 `SubLauncher` 的区别也很关键：`Launcher` 是 top-level argv parser；`SubLauncher` 是可被 `universal` 或 `web` 组合的 capability unit。

### `cmd/launcher/universal/universal.go`

`uniLauncher.parse` 的策略是：

1. 构造 keyword -> sublauncher map。
2. 默认选择第一个 sublauncher。
3. 如果第一个 argv token 是已知 keyword，则切到对应 sublauncher，并把剩余 argv 交给它解析。
4. 如果没有命中，则让默认 sublauncher 解析所有 args。

因此 `full.NewLauncher(console, web)` 中 console 是默认模式；用户直接运行 binary 且不写 `web`，就进入 console。

风险点：这种“第一个 sublauncher 作为默认”的语义非常依赖 `full.NewLauncher()` 的参数顺序。后续维护者调整顺序可能改变 CLI 默认行为，应该在文档和测试里固定。

### `cmd/launcher/console/console.go`

console launcher 做了完整的本地交互生命周期：

- `signal.NotifyContext` 处理 Ctrl-C。
- `telemetry.InitAndSetGlobalOtelProviders` 初始化 telemetry，并在 defer 中按 `shutdownTimeout` shutdown。
- 如果没有注入 `SessionService`，默认使用 `session.InMemoryService()`。
- 创建 session，创建 runner。
- 后台 goroutine 从 stdin 读行。
- 每次输入转成 `genai.NewContentFromText`，调用 `runner.Run`。
- streaming mode 默认为 auto：如果 stdout 是 terminal，则用 SSE；否则用 non-streaming。

值得注意的是，console 是对 `runner.Run` 最薄的一层包装；它几乎不做协议转换，只处理 CLI UX。

### `cmd/launcher/web/web.go`

web launcher 是真正的服务端容器：

- 解析 server timeout。
- 通过 `BuildBaseRouter()` 建 router。
- active sublauncher 逐个注册路由。
- `logger` middleware 记录 method、URI、耗时。
- 初始化 telemetry。
- 启动 `http.Server`，监听 ctx done 或 server error。

这里的设计关键是 active sublauncher 可以组合。例如 Cloud Run Dockerfile 可以同时启用 `api`、`a2a`、`webui`，而本地也可以只开 `web api` 或 `web webui api`。

### `cmd/launcher/web/api/api.go` 与 `server/adkrest`

`api.SetupSubrouters` 创建 `adkrest.NewServer`，并把 REST server 的 span/log processor 加入 `config.TelemetryOptions`。这是一个有意思的反向注入：server debug telemetry 既作为 API 能力，也作为 telemetry processor 被安装到 launcher config。

`server/adkrest/controllers.RuntimeAPIController` 负责核心 runtime API：

- `RunHandler`：decode JSON request，校验 session，创建 runner，收集事件，返回 JSON array。
- `RunSSEHandler`：设置 SSE headers 和 write deadline，逐个 event `json.Marshal` 后 `data: ...\n\n` flush。
- `RunLiveHandler`：WebSocket live run。
- `getRunner`：根据 `appName` 从 `AgentLoader` 加载 agent，并用 session/memory/artifact/plugin 构造 `runner.Runner`。

REST 层没有直接实现 agent 逻辑，它只是把 HTTP session/message/state delta 转成 `runner.Run` 参数。

### `cmd/launcher/web/a2a/a2a.go` 与 `server/adka2a`

`a2a.SetupSubrouters` 首先从 root agent 生成 agent card：

- name / description 来自 root agent。
- default input/output mode 是 `text/plain`。
- 同时暴露 A2A v1 JSON-RPC URL 和 v0 compatible URL。
- skills 来自 `adka2a.BuildAgentSkills(rootAgent)`。
- capabilities 标记 streaming。

然后它创建 `adka2a.NewExecutor`，把 `runner.Config` 交给 A2A executor。A2A executor 的关键价值是协议转换：

- A2A input parts -> GenAI parts。
- ADK `session.Event` -> A2A artifact/status event。
- executor callback 可以在 before/after event/after execute 注入行为。
- legacy v0 wrapper 通过 v2 implementation 适配。

这条路径让 ADK agent 成为跨 agent 通信协议的一等服务。

### `cmd/launcher/web/agentengine/agentengine.go` 与 `server/agentengine`

Agent Engine launcher 的 path prefix 默认 `/api`，并注册 `server/agentengine.NewHandler`。`NewHandler` 构建两个 API router：

- non-streaming reasoning engine API：session create/get/list/delete。
- streaming reasoning engine API：`async_stream_query`。

`server/agentengine.ListClassMethods` 会收集所有 method handler 的 metadata，用于部署时填充 Vertex Reasoning Engine `ClassMethods`。`stream_query` handler 支持两种 payload：完整 `StreamQueryRequest` 或简化的 text request，然后创建 runner 并以 `StreamingModeSSE` 调用 `runner.Run`，将事件按 JSONL 输出。

这说明 Agent Engine 路径不是普通 REST API 的简单别名，而是对齐云平台所需的 class method introspection 和 streaming contract。

### `cmd/adkgo/internal/deploy/cloudrun/cloudrun.go`

Cloud Run deploy 的流程是线性的：

1. `computeFlags`：把 entry point、temp dir 转成绝对路径，并创建临时 build dir。
2. `compileEntryPoint`：在 entry point 目录下执行 `go build -ldflags "-s -w"`，环境设为 `CGO_ENABLED=0 GOOS=linux GOARCH=amd64`。
3. `prepareDockerfile`：生成 distroless Dockerfile，CMD 调用 binary 的 `web` 模式，并按 flag 拼接 `api`、`a2a`、`webui`、`pubsub`、`eventarc`。
4. `gcloudDeployToCloudRun`：执行 `gcloud run deploy`，设置 `GOOGLE_API_KEY` secret，region/project/ingress/no-allow-unauthenticated。
5. `cleanTemp`：清理临时目录。
6. `runGcloudProxy`：启动本地 authenticated proxy，并打印 Web UI/API URL。

Cloud Run 部署的产品假设是“服务默认不公开，需要本地 proxy 鉴权访问”。这对安全合理，但也意味着新手体验强依赖 gcloud login、project、region、secret 配置。

### `cmd/adkgo/internal/deploy/agentengine/agentengine.go`

Agent Engine deploy 的流程不同：

1. `computeFlags`：计算 entry point/source dir/temp/archive path，并组装 memory bank model resource name。
2. `prepareDockerfile`：生成 multi-stage Dockerfile，在 builder stage 中 `go build` entry point，在 distroless stage 中运行 `web agentengine`。
3. `createArchive`：用 `tar` 打包 source dir，排除 `.git` 和 `adkgo`，并附带 Dockerfile。
4. `gcloudDeployToAgentEngine` 或 `gcloudUpdateAgentEngine`：调用 Vertex AI Reasoning Engine client，提交 inline source archive、agent framework、env/secret env、class methods。
5. 可选 `memoryBank`：填充 `ReasoningEngineContextSpec.MemoryBankConfig`，并设置 TTL 和 generation model。

这里的核心差异是 Agent Engine 需要告诉平台“这个 agent 支持哪些 class methods”，所以部署前必须调用 `server/agentengine.ListClassMethods()`。

### `telemetry` 与 `internal/telemetry`

`telemetry/config.go` 提供 option API，比如 `WithOtelToCloud`、`WithGcpResourceProject`、`WithGcpQuotaProject`、`WithGoogleCredentials`、`WithSpanProcessors`、`WithLogRecordProcessors`、`WithGenAICaptureMessageContent`。

`telemetry/setup_otel.go` 做具体配置：

- 从 env 读取 `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT`。
- 如果启用 GCP export，就查找 ADC，并解析 quota/resource project。
- resource merge 顺序是 default -> GCP attributes/detector -> custom resource。
- OTLP endpoint env 会启用 OTLP trace/log exporter。
- GCP telemetry 会创建 `telemetry.googleapis.com/v1/traces` exporter，并设置 `x-goog-user-project`。

`internal/telemetry` 则定义 semantic spans：

- `StartInvokeAgentSpan`
- `StartGenerateContentSpan`
- `StartExecuteToolSpan`
- token usage attributes
- tool call args/response attributes
- error/status recording

这个设计避免把 GCP exporter 细节泄漏到 runner/model/tool 层。

### examples 和 scripts

`examples/README.md` 说 examples 通常 minimal，用来测试一个或几个场景，并且不同于更完整的 `google/adk-samples`。当前 examples 覆盖：

- quickstart/basic agent。
- REST / web / telemetry / tool confirmation。
- A2A。
- bidirectional streaming。
- MCP。
- workflow agents：sequential、parallel、loop。
- tools：load artifacts、load memory、multiple tools。
- Vertex AI / Agent Engine。
- web image generator。

`scripts/adk-web/update-adk-web.sh` 负责从 `google/adk-web` 拉取 Web UI，Docker build 后把 `dist/agent_framework_web/browser` 拷到 `cmd/launcher/web/webui/distr/`，再由 `go:embed` 打包进 binary。这个脚本把前端 bundle 作为 vendored artifact 固化在 Go repo 中。

`.github/workflows/go.yml` 覆盖 tidy、build、race test、shuffle test、lint；`nightly.yml` 覆盖 nightly race test 和 govulncheck。

## entrypoint_map

```text
用户 main.go / cmd/internal/adkcli
        |
        v
  launcher.Config
  - AgentLoader
  - SessionService
  - ArtifactService
  - MemoryService
  - PluginConfig
  - TelemetryOptions
        |
        v
  launcher.Launcher
        |
        +-- full.NewLauncher()
        |      |
        |      +-- console.NewLauncher()
        |      |      |
        |      |      +-- runner.New -> runner.Run -> stdout/stdin
        |      |
        |      +-- web.NewLauncher(...)
        |             |
        |             +-- webui.NewLauncher()
        |             |      +-- embedded cmd/launcher/web/webui/distr
        |             |
        |             +-- api.NewLauncher()
        |             |      +-- adkrest.NewServer
        |             |      +-- JSON / SSE / WebSocket
        |             |
        |             +-- a2a.NewLauncher()
        |             |      +-- agent card
        |             |      +-- A2A JSON-RPC executor
        |             |
        |             +-- pubsub/eventarc triggers
        |
        +-- prod.NewLauncher()
        |      |
        |      +-- web api + a2a only
        |
        +-- adkgo deploy cloudrun
        |      |
        |      +-- go build linux/amd64
        |      +-- distroless Dockerfile
        |      +-- CMD: <exec> web api/a2a/webui/triggers
        |      +-- gcloud run deploy
        |      +-- gcloud run services proxy
        |
        +-- adkgo deploy agentengine
               |
               +-- source archive + Dockerfile
               +-- ReasoningEngine Create/Update
               +-- ClassMethods from server/agentengine
               +-- CMD: <exec> web agentengine
```

按入口看：

| 入口 | 主要文件 | 协议/生命周期 | 最终运行时 |
| --- | --- | --- | --- |
| CLI deploy | `cmd/adkgo/**` | cobra command | 部署构建流程 |
| local configurable CLI | `cmd/internal/adkcli/main.go` | scan `root_agent.yaml` | `full.NewLauncher` |
| console | `cmd/launcher/console` | stdin/stdout, SIGINT | `runner.Run` |
| Web shell | `cmd/launcher/web` | HTTP server + subrouter | sublauncher |
| REST API | `cmd/launcher/web/api`, `server/adkrest` | JSON/SSE/WebSocket | `runner.Run` |
| Web UI | `cmd/launcher/web/webui` | embedded static assets | REST API frontend |
| A2A | `cmd/launcher/web/a2a`, `server/adka2a` | agent card + JSON-RPC | `runner.Run` via A2A executor |
| Agent Engine | `cmd/launcher/web/agentengine`, `server/agentengine` | Reasoning Engine methods | `runner.Run` via class method |
| Cloud Run deploy | `cmd/adkgo/internal/deploy/cloudrun` | gcloud run + distroless image | `web` launcher |
| Agent Engine deploy | `cmd/adkgo/internal/deploy/agentengine` | source archive + Vertex AI API | `web agentengine` |
| Telemetry | `telemetry`, `internal/telemetry` | OTel/GCP exporter + spans | global provider + runner spans |

## tests

当前测试覆盖可以分成几类：

1. CI 层：`.github/workflows/go.yml` 运行 `go mod tidy -diff`、`go build -mod=readonly -v ./...`、`go test -race -mod=readonly -v -count=1 -shuffle=on ./...` 和 golangci-lint。
2. Nightly 层：`.github/workflows/nightly.yml` 跑 race/shuffle test 和 govulncheck。
3. REST 层：`server/adkrest/controllers/runtime_test.go` 覆盖 runtime / SSE handler；`debug_test.go`、`sessions_test.go` 等覆盖 server API。
4. A2A 层：`cmd/launcher/web/a2a/a2a_test.go`、`server/adka2a/v2/*_test.go` 覆盖 agent card、events、parts、processor、executor。
5. Agent Engine 层：`server/agentengine/controllers/method/*_test.go`、helper/router test 覆盖 stream query、session methods、JSON encode。
6. Telemetry 层：`telemetry/telemetry_test.go`、`internal/telemetry/*_test.go` 覆盖 provider、converters、logger、span语义。
7. Launcher/trigger 层：`cmd/launcher/web/triggers/pubsub/*_test.go` 等覆盖 trigger route。

明显缺口：

- Cloud Run deploy 路径缺少不依赖真实 GCP 的 fake `gcloud` 集成测试。现在 `compileEntryPoint`、`prepareDockerfile`、`runGcloudProxy` 等更像手动流程，容易在 flag 拼接或 Dockerfile CMD 上 drift。
- Agent Engine deploy 路径缺少 fake ReasoningEngine client 注入点；现在 create/update 直接构造 client，单测难以隔离 archive/method metadata/request body。
- examples 缺少“示例分类矩阵”的自动检查。例如 examples README 说 full launcher 支持 console/restapi/a2a/webui，但实际代码里关键字是 `api` 而不是 `restapi`，这种文档/API drift 应被捕捉。
- Web UI vendored bundle 缺少版本记录。`update-adk-web.sh` 从 main 拉最新，输出直接覆盖 `webui/distr`，但 repo 内没有明确记录上游 commit。
- Telemetry 的 GCP export 依赖 ADC/project/env，多数错误只能在真实环境暴露，建议引入 fake credential/project resolver 的单测。

## risks

### 1. 默认 launcher 顺序会影响 CLI 行为

`universal.NewLauncher` 把第一个 sublauncher 当默认。`full.NewLauncher` 当前把 console 放第一位，所以 no-arg 是 console。如果维护者调整顺序，用户体验会变。建议给 `full.NewLauncher` 加一个明确的 no-arg/default behavior test。

### 2. Deploy command 直接调用外部命令，失败可诊断性依赖 stdout/stderr

Cloud Run path 直接执行 `go build`、`gcloud run deploy`、`gcloud run services proxy`。如果用户缺少 gcloud login、project、region、secret、IAM 或 Docker/gcloud 版本不匹配，错误会原样来自外部命令。建议对常见错误做前置检查：

- `gcloud` 是否存在。
- `gcloud auth list` 是否有 active account。
- project/region/service name 是否为空。
- Secret Manager 中 `GOOGLE_API_KEY` 是否存在。
- entry point 是否是 package main。

### 3. Cloud Run temp cleanup 在 proxy 前发生

`deployOnCloudRun` 在 `runGcloudProxy` 前调用 `cleanTemp`。这对已部署服务没问题，但如果用户想检查生成的 Dockerfile 或 build artifact，会被清掉。建议提供 `--keep_temp` debug flag。

### 4. Agent Engine deploy 缺少 source archive allow/deny list

`createArchive` 默认打包 source dir 的全部内容，只排除 `.git` 和 `adkgo`。如果用户目录里有 `.env`、credentials、large artifacts、testdata、local cache，可能被打进 archive。建议加入 `.adkignore` 或最小 allowlist，并默认排除 `.env`、`.venv`、`node_modules`、`.cache`、`*.key`、`*.pem`。

### 5. Web UI vendoring 从 upstream main 拉取，不可复现

`scripts/adk-web/Dockerfile` 用 GitHub refs main 触发 re-clone，`update-adk-web.sh` 没记录具体 commit。结果是同一个脚本在不同时间生成不同 bundle。建议把 upstream commit 写入 `cmd/launcher/web/webui/distr/VERSION` 或 Go embed metadata。

### 6. Telemetry content capture 是敏感开关

`OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true` 会记录消息内容。Agent Engine deploy 默认设置该 env 为 true。这对 debugging 有价值，但生产上可能带来 PII / secret / prompt leakage 风险。建议在文档和 deploy flag 中明确提示，并允许一键关闭。

### 7. REST 和 Agent Engine 是两套 API model，文档需要明确

`server/adkrest` 和 `server/agentengine` 都能 stream agent response，但请求/响应 shape 不同。用户可能以为 Agent Engine 就是 REST API 的部署版。实际上 Agent Engine 使用 class method / query / streamQuery contract，应该在文档里画清楚两者差异。

### 8. examples 的角色容易被误解

README 说 examples 是 minimal testing examples，不是客户 e2e samples。但 examples 又是 repo 内最容易被用户打开的学习入口。建议 examples 目录按能力 taxonomy 重组：

- runtime basic
- tools
- workflow agents
- streaming/bidi
- web/rest/a2a
- cloud/vertex/agentengine
- telemetry

并给每个 example 标出“覆盖能力 / 非目标 / 生产注意事项”。

## next_questions

1. `full.NewLauncher` 和 `prod.NewLauncher` 是否应该有稳定的 public API compatibility policy？如果 launcher keyword 变化，用户 binary 会直接受影响。
2. Cloud Run deploy 是否应该支持 dry-run，只生成 Dockerfile / gcloud command / proxy command，而不执行部署？
3. Agent Engine deploy 是否需要 `.adkignore`，避免把本地 secret 和大文件打进 source archive？
4. Agent Engine `ClassMethods` 是否需要版本化？如果 server method metadata 改了，老部署如何兼容？
5. `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT` 在 Agent Engine deploy 中默认 true 是否过于激进？
6. REST API、A2A、Agent Engine 三套 streaming model 是否有统一的 event schema 文档？
7. Web UI bundle 是否应该记录 upstream `google/adk-web` commit，保证 reproducible update？
8. examples 是否应该从“feature smoke”升级为“能力矩阵 + 教学路线”，并自动检查 README 中列出的 launcher keyword 是否真实存在？
9. deploy CLI 的 GCP 前置检查应该做到什么程度：只检查 flag，还是主动调用 `gcloud` / Secret Manager / IAM？
10. `cmd/internal/adkcli` 的 configurable/conformance 入口是否会成为正式用户入口？如果是，扫描 `root_agent.yaml` 的规则需要更严谨的 package/app naming。
11. `server/adkrest` 中 `AutoCreateSession`、`StateDelta`、SSE timeout 在不同前端中的推荐默认值是什么？
12. A2A legacy v0 adapter 何时可以移除，或者需要怎样的 compatibility test 来保证 v0/v1 行为一致？

## 总结

ADK Go 的 entrypoint/deploy 层不是薄 CLI，而是一套“把 agent runtime 产品化”的边界系统。它通过 `launcher.Config` 和 sublauncher 把 console、web、REST、A2A、Agent Engine、Cloud Run、telemetry 统一到同一个 runner 体系里；通过 `adkgo deploy` 把本地 entry point 变成云端服务；通过 examples 和 CI 维持能力示例与质量基线。

它现在最强的地方是分层清楚：runner 不知道自己在哪里被调用，入口层只负责协议和生命周期转换。最需要继续打磨的是部署可诊断性、production secret/telemetry 风险、examples taxonomy、Web UI vendoring 可复现性，以及 cloud deploy path 的 fake integration test。
