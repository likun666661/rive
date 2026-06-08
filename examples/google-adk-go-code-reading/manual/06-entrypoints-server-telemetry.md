# 第六部分：CLI、Server、部署、Telemetry 与 Examples

> 对 `google/adk-go` 入口、服务端、部署、可观测性和示例层的只读代码分析。

---

## 1. 面临的问题是什么

ADK Go 是一个纯 Go 实现的 Agent Development Kit。作为 library，它提供了 `agent`、`runner`、`session`、`tool` 等核心抽象。但开发者最终需要：

1. **运行 agent**（不是只 import library）- 需要 CLI 入口
2. **通过 HTTP 暴露 agent**（REST API、Server-Sent Events、WebSocket Live）
3. **与外部 A2A 协议互通**（Agent-to-Agent 互操作）
4. **部署到 Google Cloud 平台**（Cloud Run、Agent Engine / Vertex Reasoning Engine）
5. **可观测性**（OpenTelemetry traces、logs、debug endpoints）
6. **给开发者看的使用示例**（catalog product usage）

这些不是 library core 范畴，但决定 ADK Go 能否在生产环境闭环。

---

## 2. 为什么这是问题

### 2.1 开发体验边界
- 纯 library 模式需要开发者自己写 `main()`、HTTP server、flag parsing、session storage。
- 不同运行模式（console 交互、REST API、A2A protocol、Web UI）如果由开发者各自实现，既重复又容易出错。

### 2.2 部署到 Google Cloud 的差异
- **Cloud Run**: 需要 Dockerfile、static 编译 binary、port 约定、authenticating proxy（`gcloud run services proxy`）。
- **Agent Engine / Reasoning Engine**: 需要 tar archive、`ClassMethods` 注册、ReasoningEngine API（`CreateReasoningEngine` / `UpdateReasoningEngine`）、memory bank 配置。

### 2.3 多协议端点
- **REST** (`/api/run`, `/api/run_sse`, `/api/run_live`): 不同语义（一次性响应 vs SSE 流 vs WebSocket 双向流）
- **A2A** (`/a2a/v1/invoke`, `/a2a/invoke`): JSON-RPC transport, 同时支持 A2A 0.3 和 1.0 协议版本
- **Agent Engine** (`/api/reasoning_engine`, `/api/stream_reasoning_engine`): Agent Engine 特有的 query 协议

### 2.4 Observability 边界
- OTel 初始化需要正确配置 `TracerProvider`、`LoggerProvider`、GCP 认证、resource 属性（`gcp.project_id`、`service.name`）。
- ADK REST server 还内置 debug telemetry（in-memory span/log store），通过 `/debug/trace` 端点暴露。

---

## 3. 解决思路：分层

```
┌─────────────────────────────────────────────────────────────────┐
│                         examples/                                │
│  quickstart, a2a, agentengine, bidi, mcp, rest, skills,         │
│  telemetry, tools/*, web, workflowagents/*, vertexai/*           │
│  (20+ main.go, 最上层用户接触面)                                  │
├─────────────────────────────────────────────────────────────────┤
│                      cmd/                                        │
│  ┌──────────┐  ┌──────────────────────────────────────┐         │
│  │ adkgo/   │  │            launcher/                 │         │
│  │ (cobra   │  │  launcher.go (interface: Launcher,   │         │
│  │  CLI)    │  │    SubLauncher, Config)              │         │
│  │          │  │  universal/ (keyword-based router)   │         │
│  │  deploy/ │  │  full/     (console+web+a2a+webui+   │         │
│  │   agent- │  │              pubsub+eventarc+api)    │         │
│  │   engine │  │  prod/     (api+a2a only)            │         │
│  │   cloud- │  │  agentengine/ (web+agentengine sub)  │         │
│  │   run    │  │  console/  (interactive REPL)        │         │
│  └──────────┘  │  web/                                │         │
│                │    a2a/    (A2A 0.3 + 1.0 handler)  │         │
│                │    api/    (REST API subrouter)      │         │
│                │    webui/  (embedded SPA dist)       │         │
│                │    agentengine/ (Agent Engine route) │         │
│                │    triggers/ (pubsub, eventarc)      │         │
│                │  internal/telemetry/  (InitAndSet-   │         │
│                │    GlobalOtelProviders)              │         │
│                └──────────────────────────────────────┘         │
├─────────────────────────────────────────────────────────────────┤
│                     server/                                      │
│  ┌────────────────────┐  ┌──────────────┐  ┌─────────────────┐  │
│  │ adkrest/            │  │ agentengine/ │  │ adka2a/         │  │
│  │  handler.go         │  │  handler.go  │  │  executor.go    │  │
│  │  ServerConfig,      │  │  NewHandler  │  │  (legacy v0.3   │  │
│  │  Server (http.Hdlr) │  │  ListClass-  │  │   compat shim)  │  │
│  │  SpanProcessor()    │  │  Methods     │  │  conversions.go │  │
│  │  LogProcessor()     │  │              │  │  v2/ (canonical │  │
│  │  controllers/       │  │ controllers/ │  │    impl)         │  │
│  │    runtime.go       │  │   agent_eng- │  │    executor.go   │  │
│  │    sessions.go      │  │   ine.go     │  │    agent_card.go │  │
│  │    apps.go          │  │   method/    │  │    parts.go      │  │
│  │    artifacts.go     │  │     create_, │  │    events.go     │  │
│  │    debug.go         │  │     get_,    │  │    processor.go  │  │
│  │  internal/          │  │     list_,   │  └─────────────────┘  │
│  │    routers/         │  │     delete_  │                       │
│  │    services/        │  │     session  │                       │
│  │    models/          │  │     stream_  │                       │
│  └────────────────────┘  │     query    │                       │
│                          └──────────────┘                       │
├─────────────────────────────────────────────────────────────────┤
│                    telemetry/                                    │
│  telemetry.go  (New, Providers, Option 接口)                     │
│  config.go     (config struct, With* options)                    │
│  setup_otel.go (GCP exporters, OTLP env vars, resource merge)    │
├─────────────────────────────────────────────────────────────────┤
│  util/aiplatform/  (HostURL, HostPortURL 工具函数)               │
│  util/vertexai/    (AgentEngineResource, SessionResource)        │
│  internal/version/ (const Version = "1.2.0")                     │
│  internal/cli/util/(FormatFlagUsage, LogStartStop, LogCommand)   │
│  internal/utils/   (FunctionCalls, TextParts, AppendInstructions)│
└─────────────────────────────────────────────────────────────────┘
```

核心设计模式:
- **Launcher 组合**: `universal.NewLauncher(subLauncher...)` 通过 keyword 路由到 console/web。`full.NewLauncher()` 就是 `universal(console, web(webui, a2a, pubsub, eventarc, api))`。
- **Web Sublauncher 组合**: `web.NewLauncher(webui, a2a, api)` 将多个子功能注册到同一个 HTTP mux。
- **Server 层只做协议翻译**: `server/adkrest` 不启动 HTTP server，只返回 `http.Handler`。启动逻辑在 `cmd/launcher/web/` 中。

---

## 4. adk-go 代码怎么落地

### 4.1 入口与部署地图

| 入口 | 文件/包 | 作用 |
|---|---|---|
| `adkgo` CLI tool | `cmd/adkgo/adkgo.go:16-26` | `root.Execute()` cobra root, 注册 deploy subcommands |
| `adkgo deploy cloudrun` | `cmd/adkgo/internal/deploy/cloudrun/:280-310` | 编译 Go binary → 生成 Dockerfile → `gcloud run deploy` → 启动 auth proxy |
| `adkgo deploy agentengine` | `cmd/adkgo/internal/deploy/agentengine/:416-445` | 生成 Dockerfile → tar archive → `CreateReasoningEngine` / `UpdateReasoningEngine` API |
| `adkcli` (conformance) | `cmd/internal/adkcli/main.go:36-126` | 扫描 `root_agent.yaml` → 加载 agent → 用 `full.NewLauncher()` 启动 |
| Agent 启动 (full) | `cmd/launcher/full/full.go:31-33` | `universal(console, web(webui, a2a, pubsub, eventarc, api))` |
| Agent 启动 (prod) | `cmd/launcher/prod/prod.go:29-31` | `universal(web(api, a2a))` — 无 console/webui, 生产部署用 |
| Agent Engine 启动 | `cmd/launcher/agentengine/agentengine.go:26-28` | `universal(web(agentengine))` |
| Console 交互 | `cmd/launcher/console/console.go:68-205` | 读 stdin → runner.Run() → 格式化输出到 stdout |
| Web server | `cmd/launcher/web/web.go:151-220` | HTTP server with grace shutdown + OTel init |
| REST API sublauncher | `cmd/launcher/web/api/api.go:76-108` | 创建 `adkrest.Server`, add CORS middleware, register to parent mux |
| A2A sublauncher | `cmd/launcher/web/a2a/a2a.go:85-138` | 构造 AgentCard → `adka2a.NewExecutor()` → 注册 JSON-RPC handler (v0.3 + v1.0) |
| Agent Engine sublauncher | `cmd/launcher/web/agentengine/agentengine.go:84-96` | 创建 `agentengine.NewHandler()` → 注册 POST routes |

### 4.2 Server 关键类型

| 包 | 关键类型 | 文件:行号 | 说明 |
|---|---|---|---|
| `server/adkrest` | `Server struct` | `handler.go:80-84` | 实现 `http.Handler`, holds mux router + debug telemetry |
| `server/adkrest` | `ServerConfig` | `handler.go:63-71` | SessionService, MemoryService, AgentLoader, ArtifactService, SSE timeout, PluginConfig, DebugConfig |
| `server/adkrest` | `RuntimeAPIController` | `controllers/runtime.go:37-45` | RunHandler, RunSSEHandler, RunLiveHandler (WebSocket) |
| `server/adkrest` | `DebugAPIController` | `controllers/debug.go:33-36` | EventSpanHandler, SessionSpansHandler, EventGraphHandler |
| `server/adkrest` | `SessionsAPIController` | `controllers/sessions.go:31-33` | Create/Get/List/Delete session CRUD |
| `server/adkrest` | `AppsAPIController` | `controllers/apps.go:24-26` | ListAppsHandler - list loaded agents |
| `server/adkrest` | `ArtifactsAPIController` | `controllers/artifacts.go:28-30` | List/Load/Delete artifacts with version support |
| `server/agentengine` | `NewHandler()` | `handler.go:39-75` | 创建 streaming + non-streaming dual handler |
| `server/agentengine` | `AgentEngineAPIController` | `controllers/agent_engine.go:32-37` | 基于 `classMethod` 字段 dispatch 到对应的 `MethodHandler` |
| `server/agentengine` | `MethodHandler` 接口 | `controllers/method/method.go` | Name(), Metadata(), Handle() — 5 个实现: CreateSession, GetSession, ListSession, DeleteSession, StreamQuery |
| `server/adka2a` | `Executor` (legacy) | `executor.go:139-141` | 包装 `v2.Executor`, 负责 A2A 0.3 ↔ 1.0 转换 |
| `server/adka2a/v2` | `Executor` (canonical) | `v2/executor.go` | 完整 A2A 1.0 实现, runner callback pipeline |
| `server/adka2a/v2` | `ExecutorConfig` | `v2/executor.go:95-130` | RunnerConfig, Before/After/Part 回调链, OutputMode |

### 4.3 REST API 路由表

注册于 `server/adkrest/handler.go:48-55`, 使用 `routers.Router` 接口模式:

| Route | Method | Path Pattern | Controller |
|---|---|---|---|
| CreateSession | POST | `/apps/{app_name}/users/{user_id}/sessions` | SessionsAPI |
| GetSession | GET | `/apps/{app_name}/users/{user_id}/sessions/{session_id}` | SessionsAPI |
| ListSessions | GET | `/apps/{app_name}/users/{user_id}/sessions` | SessionsAPI |
| DeleteSession | DELETE | `/apps/{app_name}/users/{user_id}/sessions/{session_id}` | SessionsAPI |
| RunAgent | POST | `/apps/{app_name}/users/{user_id}/sessions/{session_id}/run` | RuntimeAPI |
| RunAgentSSE | POST | `/apps/{app_name}/users/{user_id}/sessions/{session_id}/run_sse` | RuntimeAPI |
| RunAgentLive | GET | `/apps/{app_name}/users/{user_id}/sessions/{session_id}/run_live` | RuntimeAPI (WebSocket) |
| ListApps | GET | `/list-apps` | AppsAPI |
| EventSpan | GET | `/debug/trace/{event_id}` | DebugAPI |
| SessionSpans | GET | `/debug/session/{session_id}` | DebugAPI |
| EventGraph | GET | `/debug/graph/{app_name}/{user_id}/{session_id}/{event_id}` | DebugAPI |
| ListArtifacts | GET | `/apps/{app_name}/users/{user_id}/sessions/{session_id}/artifacts` | ArtifactsAPI |
| LoadArtifact | GET | `/apps/{app_name}/users/{user_id}/sessions/{session_id}/artifacts/{artifact_name}` | ArtifactsAPI |

### 4.4 Telemetry 层

- **入口**: `telemetry.New(ctx, opts...)` → 返回 `*Providers` (`telemetry/telemetry.go:118-124`)
- **配置**: `telemetry/config.go` 定义 `Option` 接口, 支持 `WithOtelToCloud`、`WithGcpResourceProject`、`WithSpanProcessors`、`WithTracerProvider` 等
- **GCP 导出**: `setup_otel.go:247-257` 创建带 OAuth2 TokenSource + `x-goog-user-project` header 的 OTLP HTTP exporter, 发送到 `telemetry.googleapis.com/v1/traces`
- **环境变量支持**: `OTEL_EXPORTER_OTLP_ENDPOINT`、`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`、`OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`
- **Launcher 集成**: `cmd/launcher/internal/telemetry/telemetry.go:26-33` 的 `InitAndSetGlobalOtelProviders()` 将 launcher 的 `TelemetryOptions` 与 `--otel_to_cloud` flag 合并
- **Debug telemetry**: `server/adkrest` 内部维护 `DebugTelemetry` in-memory store (`services.NewDebugTelemetryWithConfig`), 通过 `SpanProcessor()` / `LogProcessor()` 方法暴露给外部 TracerProvider
- **测试覆盖**: `telemetry/telemetry_test.go:555` 行 — 覆盖 smoke test、custom provider、resource/quota project resolution (7 个 case)、exporter configuration (7 个 case)

### 4.5 示例 Taxonomy (产品使用清单)

20 个 `main.go`, 按功能域分类:

| 域 | 示例目录 | 核心演示内容 |
|---|---|---|
| 入门 | `quickstart/` | 基础 agent + Google Search tool + launcher |
| 协议 | `a2a/` | A2A inter-agent communication |
| 协议 | `rest/` | REST API server (adkrest) |
| 协议 | `web/` | 多 agent 协作 (image_generator + llmauditor) + A2A |
| 部署 | `agentengine/` | Agent Engine 部署 pattern |
| 实时 | `bidi/` | WebSocket 双向流 + Web UI (`gemini-3.1-flash-live-preview`) |
| 实时 | `bidi/streamingtool/` | 自定义 streaming tool (iter.Seq2) |
| 实时 | `bidi/sequential/` | sequential agent + 双向流 |
| 工具 | `tools/multipletools/` | Google Search + 自定义 tool 并存 |
| 工具 | `tools/loadartifacts/` | 跨 session 加载 artifacts |
| 工具 | `tools/loadmemory/` | 跨 session memory recall |
| 工作流 | `workflowagents/sequential/` | sequentialagent |
| 工作流 | `workflowagents/sequentialCode/` | coding→review pipeline |
| 工作流 | `workflowagents/parallel/` | parallelagent 并发执行 |
| 工作流 | `workflowagents/loop/` | loopagent 循环 |
| 集成 | `mcp/` | Model Context Protocol (mcp-go SDK) |
| 集成 | `skills/` | SKILL.md-based skill toolset |
| 高级 | `toolconfirmation/` | `ctx.ToolConfirmation()` human-in-the-loop |
| 平台 | `vertexai/imagegenerator/` | Imagen model + artifact saving |
| 可观测 | `telemetry/` | OpenTelemetry tracing enabled agent |

### 4.6 测试覆盖

- **telemetry**: `telemetry/telemetry_test.go` (555 行) — 完整的 unit test coverage, 包含 smoke test、custom provider 注入、resource/project 解析的 7 种 strategy
- **server/adkrest/controllers**: `debug_test.go`、`runtime_test.go`、`sessions_test.go`
- **server/adka2a/v2**: `agent_card_test.go`、`events_test.go`、`executor_test.go`、`metadata_test.go`、`parts_test.go`、`processor_test.go`
- **cmd/launcher/web/a2a**: `a2a_test.go`
- **internal/utils**: `utils_test.go`、`schema_test.go`
- **internal**: `style_test.go`

### 4.7 未读风险

1. **Cloud Run deploy** 中的 `--ingress all --no-allow-unauthenticated` 会导致无法从公网访问; 需要确认 gcloud proxy 能正确注入认证 header
2. **Agent Engine deploy** 中的 `GOOGLE_API_KEY` secret 需要预先在 GCP Secret Manager 创建; deploy 命令假设 secret 已存在, 无自动创建逻辑
3. **adka2a v0.3 legacy** 已标记 `Deprecated`, 但 `cmd/launcher/web/a2a/` 仍同时注册 v0.3 + v1.0 handler — 移除 v0.3 的时间线不明
4. **OTel LoggerProvider** 的 GCP Cloud Logging exporter 尚未实现（代码中有 `TODO(#479)`: "Golang OTel exporter to CloudLogging is not yet available"）, 仅 traces 可以导出到 GCP
5. **`server/adkrest` 和 `server/agentengine`** 各自有独立但几乎相同的 `routers.go` 实现 (`SetupSubRouters`), 存在代码重复
6. **Launcher Config** 包含 `PluginConfig` 和 `TelemetryOptions` 但没有 lifecycle hooks (`OnStart`, `OnShutdown`), 复杂部署场景下扩展性受限
7. **adkcli** (`cmd/internal/adkcli/main.go`) 硬编码 `full.NewLauncher()`, 无 prod 模式可选
8. **examples** 目录缺少集成测试或 e2e 测试; README.md 明确区分于 `google/adk-samples` 仓库

---

## 5. 入口与部署地图（可视化）

```
  Developer writes main.go
           │
           ├─► uses full.NewLauncher()       ──► 全功能开发模式:
           │                                        console | web(webui, a2a, pubsub, eventarc, api)
           │
           ├─► uses prod.NewLauncher()       ──► 生产模式:
           │                                        web(a2a, api)
           │
           ├─► uses agentengine.NewLauncher  ──► Agent Engine 模式:
           │    ("my-engine-id")                    web(agentengine)
           │
           └─► uses adkgo CLI deploy ───────────► Google Cloud 部署:
              ├─► adkgo deploy cloudrun    Cloud Run (gcloud run deploy + proxy)
              └─► adkgo deploy agentengine Agent Engine (CreateReasoningEngine API)
```

### 运行模式协议支持矩阵

| Launcher Mode | Console | REST API | SSE | WebSocket Live | A2A v0.3 | A2A v1.0 | Web UI | PubSub | Eventarc | Agent Engine |
|---|---|---|---|---|---|---|---|---|---|---|
| full (all) | Y | Y | Y | Y | Y | Y | Y | Y | Y | N |
| prod | N | Y | Y | Y | Y | Y | N | N | N | N |
| agentengine | N | N | N | N | N | N | N | N | N | Y |

---

## 6. 深度追问

1. **`server/adkrest` 的 `Server` 为何设计为 `http.Handler` 而非直接启动 HTTP server?** 这种设计允许调用方自由选择 net/http server 配置（timeout、TLS）、注册到已有 mux, 符合 Go 惯用的中间件组合模式。但 Launcher 层中 `web/web.go` 实际上总是自己启动 server — 分离的收益是否兑现?

2. **Agent Engine 部署时, `GOOGLE_API_KEY` secret 的生命周期管理是否完整?** 代码仅在 `DeploymentSpec.SecretEnv` 中引用 `SecretRef{Secret: "GOOGLE_API_KEY", Version: "latest"}`, 不创建 secret。多版本回滚场景下 `Version: "latest"` 是否安全?

3. **`server/agentengine` 的非流式 (non-streaming) 和流式 (streaming) 为何是两套独立的 `AgentEngineAPIController`?** 它们共享同样的 route pattern logic（`/reasoning_engine` vs `/stream_reasoning_engine`），但 nonStream 注册了 4 个 session management handlers + StreamQuery, stream 只注册了 StreamQuery — 这种不对称是否故意?

4. **OTel LoggerProvider GCP export 功能缺失的影响多大?** Logs 的 GCP 导出依赖 `otlploghttp`, 但代码注释明确 "Golang OTel exporter to CloudLogging is not yet available" — 这是 Go OTel SDK 的 upstream 限制, 是否需要 fallback 方案（如直接使用 Cloud Logging client library）?

5. **A2A v0.3 兼容层何时可以移除?** `server/adka2a/executor.go` 整个 legacy Executor 的实现约 400 行, 包括 RequestContext / ExecutorContext 的双向转换, 维护成本较高。是否有计划时间线?

6. **PubSub / Eventarc triggers 的实现是否完整?** `cmd/launcher/web/triggers/` 目录存在但 sublauncher 初始化逻辑未在本次 scope 详细排查 — triggers 的 session 创建策略、concurrency 控制 (maxRuns=100) 是否有 race condition 风险?

7. **examples 为何没有 integration test?** README 区分了 "minimal/simplistic examples" 与 `google/adk-samples` 的 "complex e2e samples"。但即使是 minimal example, 缺少 `go test` 可运行的 smoke test 使得 CI 无法验证 example 是否仍能编译运行。

8. **`internal/cli/util` 中的 ANSI 颜色常量是全局变量 — 是否有并发安全问题?** `Reset`, `Red`, `Green` 等是 `var` 声明的只读 string, 但 `reprintableStream` 结构体的 `clean` 字段在 `Write` 方法中无锁修改 — concurrent writes 到同一个 stream 可能 race。

9. **`cmd/adkgo/internal/deploy` 中 cloudrun 和 agentengine 共享 flag struct 模式但无共享接口 — 能否抽取 `Deployer` 接口?** 两者都有 `computeFlags → prepareDockerfile → deploy → cleanTemp` 相似的 lifecycle, 但没有抽象成接口, 导致每个 deploy target 的 `RunE` 都是手动编排。

10. **Launcher Config 缺少 `context.Context` 传递 — 如何在 agent loader / plugin 初始化时注入 cancellation?** `launcher.Config` 是纯结构体, 不包含 context。Console launcher 自行创建 `signal.NotifyContext`, web launcher 也用 `ctx.Done()` 感知 shutdown — 但如果 plugin 初始化需要长连接（如 Redis session service）, 没有统一的 early-init context。
