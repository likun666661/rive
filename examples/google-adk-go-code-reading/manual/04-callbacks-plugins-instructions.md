# 第四部分：回调、插件与指令工具

## 1. 面临的问题是什么：callbacks/plugins/instructions 要解决 agent 行为定制的哪些切面？

ADK-Go 的 agent 运行遵循一个严格的生命周期循环（agent run -> model call -> tool call -> 循环），
但在实际应用中用户需要在多个切面插入自定义逻辑：

| 切面 | 定制需求 |
|---|---|
| **Agent 级别** | 在 agent 开始/结束时注入逻辑（权限检查、对话限流、状态初始化） |
| **Model 调用** | 在 LLM 请求发出前修改 prompt/tools；在响应返回后做后处理、缓存、日志 |
| **Tool 调用** | 在工具调用前拦截/修改参数；工具返回后改写结果；错误时执行降级/重试 |
| **Event 级别** | 在事件产出时做实时过滤/转发/持久化 |
| **UserMessage 级别** | 在用户消息进入广告系统时做预处理/审计 |
| **Instruction** | 运行时根据 session state 动态注入系统指令中的模板变量 |
| **全局/Lifecycle 级别** | 跨所有 agent 插入统一逻辑（如全链路日志、token 统计、合规检查） |

简而言之：**callbacks/plugins/instructions 提供了在 ADK 生命周期各阶段插入正交关注点的手段**，
且不需要修改核心 agent loop 源码。

## 2. 为什么这是问题：LLM request/response/tool call 生命周期里插入逻辑为什么需要边界？

LLM agent 的执行是**有状态的、多步的、迭代性的**。随意插入逻辑会导致：

1. **状态污染**：callback 直接修改 session state 可能让后续调用看到不一致的数据。
   ADK 通过 `callbackContextState` 的 StateDelta 隔离来缓解：
   `agent/callback_context.go:221-235` - State().Set() 写入 actions.StateDelta，
   读取时先从 delta 读再回落 session state。

2. **结果覆盖**：多个 callback 都想改写 model response / tool result，不知道谁先生效。
   ADK 采用 **early-exit 语义**：callback 链中第一个返回非 nil 的结果胜出，后续不再执行：
   `internal/plugininternal/plugin_manager.go:222-236` - RunBeforeModelCallback 的 for 循环中。

3. **错误级联**：一个 callback 的错误可能被后续 callback 吞掉或放大。
   ADK 区分了三类错误处理路径：
   - Before → 直接短路，不执行实际调用
   - After → 可改写结果或错误，early-exit
   - OnError → 仅在 model/tool 自身报错时触发，不处理 Before callback 的错误
   见 `plugin/plugin_manager_test.go:687-708` 的 `on model error callback does not process before model callback error` 测试用例。

4. **并发安全**：多个 agent 并发时，插件持有的状态可能发生竞态。
   `plugin/retryandreflect/plugin.go:61-67` 使用 `sync.Mutex` 保护 `scopedFailureCounters`。

5. **可组合性**：不同团队写的 plugin 可能冲突。PluginManager 按注册顺序执行，
   通过 `Name` 去重（`plugin_manager.go:66-68`），并保证 Close 时倒序回收。

6. **调试可见性**：插入点太多导致排障困难。
   `plugin/loggingplugin/logging_plugin.go` 是一个统一日志插件，覆盖所有 hook 点，
   起到终端调试透明度作用。

## 3. 解决思路是什么：各组件如何拆分？

### 3.1 CallbackContext —— 安全的回调沙箱

文件：`agent/callback_context.go`

CallbackContext 实现了一个**写时感知**（StateDelta + ArtifactDelta）的上下文：
- `callbackContextState` (L217-239): 写操作写入 StateDelta 且同时写 session state；
  读操作先从 StateDelta 读，miss 后回落 session state。
- `trackedArtifacts` (L243-261): 包装 Artifacts.Save()，每次保存自动记录
  版本号到 actions.ArtifactDelta，最终随 Event 落盘。

两种构造方式：
- `NewCallbackContext()` — 普通回调
- `NewCallbackContextWithArtifactTracking()` — 带 artifact 追踪
- `NewToolContext()` — 工具回调（额外支持 ToolConfirmation, SearchMemory, Human-in-the-Loop）

**设计要点**：CallbackContext 的 `Artifacts()` 返回 `trackedArtifacts`，
`State().Set()` 走 delta 路径——这意味着回调中对状态的所有修改都被记录，
Event 落盘时可追溯。

### 3.2 Plugin System —— 统一的生命周期钩子框架

文件：
- `plugin/plugin.go` — Plugin 类型定义 + Config
- `internal/plugininternal/plugin_manager.go` — PluginManager 执行引擎
- `internal/plugininternal/plugincontext/context.go` — Context key 注入

**层次结构**：

```
plugin.Plugin (统一外观)
    └── 15 个可选 callback 字段:
        OnUserMessageCallback
        OnEventCallback
        BeforeRunCallback / AfterRunCallback
        BeforeAgentCallback / AfterAgentCallback  ← 来自 agent 包
        BeforeModelCallback / AfterModelCallback / OnModelErrorCallback  ← 来自 llmagent 包
        BeforeToolCallback / AfterToolCallback / OnToolErrorCallback  ← 来自 llmagent 包
        CloseFunc
```

PluginManager 以 `context.WithValue` 注入 Context：
`plugin_manager.go:286-288` - `ToContext()` → `context.WithValue(ctx, plugincontext.PluginManagerCtxKey, cfg)`

执行策略：
1. 按注册顺序遍历 plugins
2. 对每个 callback 检查是否为 nil
3. 如果返回非 nil result/error → **early exit**，不再执行后续 plugins
4. 返回 nil, nil → 继续下一个 plugin
5. 如果所有 plugins 都返回 nil, nil → 返回 nil, nil（表示无人接管）

这确保了不同 plugin 的隔离性和有序性。

### 3.3 Function Call Modifier —— Schema 级别的请求/响应拦截

文件：`plugin/functioncallmodifier/plugin.go`

**BeforeModel**：在 LLM 请求发出前，修改匹配 tools 的 FunctionDeclaration
- `BeforeModelCallback` (L53-89): 对匹配 Predicate 的 tool，注入额外的 Args schema 和覆盖 Description
- 用于实现 skill-based orchestration：为 agent_tool/transfer_to_agent 动态添加 `skill_id`、`rationale` 参数

**AfterModel**：在 LLM 响应返回后，从 FunctionCall.Args 中剥离上述额外参数
- `AfterModelCallback` (L91-119): 对匹配 Predicate 的 FunctionCall，
  删除额外 args，将其值存入 session state（key 格式: `{functionCallID}/{argName}`）
- 这样做的好处：额外参数不影响 tool 的实际运行签名，但仍可通过 state 读取（如子 agent 读取 skill_id）

### 3.4 Retry and Reflect —— 工具错误的自我修复

文件：`plugin/retryandreflect/plugin.go`

核心机制：
1. **AfterTool + OnToolError** 两个 hook 共同实现
2. 工具执行失败时（OnToolError），不直接报错，而是计算重试次数：
   - 通过 `lock.Count[scopeKey][toolName]` 追踪每次失败
   - 在 maxRetries 内：生成 reflection prompt（反思指导 LLM 换参数/换工具重试）
   - 超过 maxRetries：返回 exceeded message（告诉 LLM 改用其他工具）或返回原始 error
3. AfterTool 中重置成功工具的失败计数
4. **不会重置** reflection response 自身的计数（通过 response_type 标记检测）

**配置选项**（L72-93）：
- `WithMaxRetries(n)` — 最大重试次数（默认 3）
- `WithErrorIfRetryExceeded(bool)` — 超限后返回原始 error 还是 exceeded 指导
- `WithTrackingScope(scope)` — 失败计数按 **invocation** 还是 **global** 计

**并发安全**：`sync.Mutex` 保护 `scopedFailureCounters`

**嵌入式模板**：
- `reflection.md` — 告诉 LLM 分析错误原因（无效参数、状态前置条件、替代方案、函数名拼写）
- `exceeded.md` — 告诉 LLM 不要再使用该工具

### 3.5 Instruction Utilities —— 模板变量注入

文件：`util/instructionutil/instruction.go`

`InjectSessionState()` 是 InstructionProvider 的配套工具：
- 输入：`ReadonlyContext` + 模板字符串
- 调用内部 `llminternal.InjectSessionState()` 解析 `{key_name}` 占位符
- 占位符支持：`{key}` 从 session state 取值；`{artifact.key}` 从 artifact 取文本；`{key?}` 可选（不存在时不报错）
- 用于实现了 `InstructionProvider` 回调的代码中：`agent/llmagent/llmagent.go:206-214`

### 3.6 Configurable Layer —— YAML 配置的回调注册

文件：
- `internal/configurable/configurable.go` — YAML config → agent factory
- `internal/configurable/configurable_utils.go` — 全局 registry（agent/tool/toolset/callback）
- `internal/configurable/conformance/callbacks.go` — 注册 conformance 测试用的回调

**回调注册流程**（`configurable_utils.go:274-282`）：
1. 业务代码调用 `configurable.RegisterCallback(name, callback)` 注册全局回调
2. YAML 中通过 `before_agent_callbacks: [{name: "pkg.path.callback"}]` 引用
3. `llmAgentYAMLConfig.toLLMAgentConfig()` (L116-162) 中调用 `resolveCallbacks[T]()` 解析
4. `ResolveCallbackReference()` 从 `callbackRegistry` 取出，类型断言为 `T`

**Conformance testing 用途**：
- `recordplugin/` — 录制 LLM 请求/响应、Tool 调用到 YAML（用于生成黄金文件）
- `replayplugin/` — 从 YAML 回放黄金文件，做确定性回归对比

### 3.7 Logging Plugin —— 全链路调试

文件：`plugin/loggingplugin/logging_plugin.go`

覆盖全部 12 个 hook 点，每个点输出灰色 ANSI 日志（模型名、tool 名、token 用量等），
是 plugin 实现的最佳参考模板。

## 4. adk-go 代码怎么落地：关键类型/函数/文件、extension points、测试覆盖、未读风险

### 4.1 扩展点地图

| 扩展点 | 文件:行 | 注入时机 | 可否短路 |
|---|---|---|---|
| `agent.BeforeAgentCallback` | `agent/agent.go:129` | agent.run() 开始时 | 是——返回 non-nil content 则跳过 agent 执行 |
| `agent.AfterAgentCallback` | `agent/agent.go:137` | agent.run() 结束后 | 是——返回 non-nil content 则发出最终事件 |
| `llmagent.BeforeModelCallback` | `llmagent/llmagent.go:289` | 调用 model.GenerateContent 前 | 是——返回 LLMResponse 即跳过实际调用 |
| `llmagent.AfterModelCallback` | `llmagent/llmagent.go:295` | model.GenerateContent 返回后 | 是——返回新 LLMResponse 替换原始响应 |
| `llmagent.OnModelErrorCallback` | `llmagent/llmagent.go:301` | model 调用失败时（不处理 Before 的错误） | 是——返回新 LLMResponse 恢复流程 |
| `llmagent.BeforeToolCallback` | `llmagent/llmagent.go:313` | tool.Run() 调用前 | 是——返回 result 即跳过工具执行 |
| `llmagent.AfterToolCallback` | `llmagent/llmagent.go:322` | tool.Run() 返回后 | 是——返回新 result 替换原始结果 |
| `llmagent.OnToolErrorCallback` | `llmagent/llmagent.go:328` | tool 执行失败时 | 是——返回新 result 恢复 |
| `plugin.OnUserMessageCallback` | `plugin/plugin.go:161` | 用户消息进入时 | 是——返回新 Content 替换 |
| `plugin.OnEventCallback` | `plugin/plugin.go:167` | 每个 Event 产出后 | 是——返回新 Event 替换 |
| `plugin.BeforeRunCallback` | `plugin/plugin.go:163` | 整个 invocation 开始时 | 是——返回 Content 则跳过 |
| `plugin.AfterRunCallback` | `plugin/plugin.go:165` | invocation 结束时 | 否——仅副作用 |
| `plugininternal.PluginManager` | `internal/plugininternal/plugin_manager.go:38` | 由 runner 在 Context 中注入 | — |
| `configurable.RegisterCallback` | `internal/configurable/configurable_utils.go:274` | 编译期/初始化期注册 | — |
| `instructionutil.InjectSessionState` | `util/instructionutil/instruction.go:41` | InstructionProvider 回调内 | — |

### 4.2 关键类型/函数总览

| 组件 | 位置 | 角色 |
|---|---|---|
| `CallbackContext` interface | `agent/context.go:125` | ReadonlyContext + State + Artifacts |
| `ToolContext` interface | `agent/context.go:136` | CallbackContext + FunctionCallID + HITL |
| `callbackContext` struct | `agent/callback_context.go:101` | 唯一实现，带 StateDelta/ArtifactDelta |
| `callbackContextState` | `agent/callback_context.go:217` | Delta-first 的 State 实现 |
| `trackedArtifacts` | `agent/callback_context.go:243` | 自动追踪 ArtifactSave 的装饰器 |
| `plugin.Plugin` struct | `plugin/plugin.go:78` | 统一插件定义（15 个 callback） |
| `plugin.Config` | `plugin/plugin.go:26` | 插件工厂配置 |
| `plugin.New()` | `plugin/plugin.go:50` | 插件构造器 |
| `PluginManager` struct | `internal/plugininternal/plugin_manager.go:38` | 多插件编排器 |
| `PluginManager.Run*()` | `internal/plugininternal/plugin_manager.go:76-270` | 14 个 hook 执行方法 |
| `functioncallmodifier.NewPlugin()` | `plugin/functioncallmodifier/plugin.go:36` | Before/After model 的 schema 改写 |
| `retryandreflect.New()` | `plugin/retryandreflect/plugin.go:96` | AfterTool + OnToolError 的 self-healing |
| `loggingplugin.New()` | `plugin/loggingplugin/logging_plugin.go:44` | 全 hook 日志 |
| `recordplugin.New()` | `internal/configurable/conformance/recordplugin/record_plugin.go:43` | LLM/Tool 录制 |
| `replayplugin.New()` | `internal/configurable/conformance/replayplugin/replay_plugin.go:61` | LLM/Tool 回放 |
| `injectSessionState()` | `util/instructionutil/instruction.go:41` | 模板→session state 变量解析 |
| `resolveCallbacks[T]()` | `internal/configurable/configurable.go:266` | YAML→typed callback 解析 |

### 4.3 测试覆盖

| 测试文件 | 覆盖范围 | 关键测试点 |
|---|---|---|
| `plugin/plugin_test.go` | Plugin New() 构造、CloseFunc nil 安全、字段映射 | Close 不 panic、字段正确映射 |
| `plugin/plugin_manager_test.go` | **核心测试**：Tool callbacks 和 Model callbacks 的完整 46 个测试用例 | before/after/onError 链的有序执行、early-exit、before→after 值传递、onModelError 不处理 before 错误 |
| `plugin/functioncallmodifier/plugin_test.go` | BeforeModel schema 注入、AfterModel arg 剥离与 state 存储 | 参数添加/删除、state key 格式验证、非匹配 tool 不受影响 |
| `plugin/functioncallmodifier/integration_test.go` | Gemini 真实模型集成测试（HTTP replay） | transfer_to_agent tool 端到端、calculator agent tool 端到端 |
| `plugin/retryandreflect/plugin_test.go` | Options 验证、成功重置、reflection 不重置、maxRetries、scope 隔离 | invocation vs global scope、retryCount 追踪、exceeded message 生成 |
| `agent/llmagent/llmagent_test.go` | LLMAgent 级别的 callback 集成 | Before/After/OnError 的顺序执行 |
| `internal/configurable/conformance/recordplugin/record_plugin_test.go` | 录制插件的 YAML 输出、state 管理 | |
| `internal/configurable/conformance/replayplugin/replay_plugin_test.go` | 回放插件的请求匹配、YAML 解析 | |
| `agent/llmagent/state_agent_test.go` | BeforeAgent/AfterAgent + model callbacks 的组合测试 | agent→model→tool 三级 callback 联动 |

### 4.4 未读风险

1. **Plugin close 的顺序问题**：`PluginManager.Close()` (`plugin_manager.go:273-284`) 按注册顺序 close，
   但某些 plugin 可能有依赖关系（如 retryandreflect 依赖 loggingplugin），倒序 close 更安全。
   当前实现未考虑 close 顺序。

2. **Context 泄漏**：`PluginManager.ToContext()` (`plugin_manager.go:286-288`) 使用全局 key `0` (
   `plugincontext/context.go:19`)，任何包都有可能与 `plugincontext.PluginManagerCtxKey` 冲突。

3. **AfterToolCallback 的类型签名**：`afterTool(ctx agent.ToolContext, tool tool.Tool, args, result map[string]any, err error)`
   总是传入 `err` 参数——这让 callback 必须自己处理 `err`。但文档中说 AfterTool
   是「无论成功或失败都会调用」，容易让使用者混淆何时应该返回新 result、何时应该 pass-through。

4. **callbackContext 的类型后门**：`callbackContext` 同时实现 `CallbackContext` 和 `ToolContext`，
   CallbackContext 的用户可以类型断言为 ToolContext 访问 FunctionCallID 等（虽然值为空）。
   见 `agent/callback_context.go:152-153` 的 interface assertion。

5. **retryandreflect 的 AfterTool 不会重置 reflection 结果**：
   `plugin/retryandreflect/plugin.go:128-141` 通过 `response_type` 字符串标记来跳过重置，
   但如果其他 plugin 返回了包含 `response_type: "ERROR_HANDLED_BY_REFLECT_AND_RETRY_PLUGIN"` 的结果，
   会被误判为 reflection 结果而不重置计数。

6. **插件回调与 agent 原生回调的重复执行**：测试中 `TestModelCallbacks` 和 `TestCallTool` 都同时注册了
   agent 原生的 BeforeModelCallback 和 plugin 的 BeforeModelCallback，两者按 PluginManager 的内部顺序都执行。
   PluginManager 通过 early-exit 短路——但先执行的 plugin 返回 non-nil 后，agent 原生 callback 不会执行。
   这意味着**plugin 可以覆盖 agent 原生 callback 的行为**，需要谨慎配置。

7. **replayplugin 中的 sleep 和 FIXME**：`replay_plugin.go:385-386` 使用
   `time.Sleep(time.Duration(expectedRecording.Index) * time.Millisecond * 10)` 配合注释
   `FIXME: remove this sleep, move curIndex++ and state cond.Broadcast() to onEvent callback.`
   说明并发回放时的确定性保证还不完善。

8. **functioncallmodifier 的 state key 碰撞风险**：`plugin/functioncallmodifier/plugin.go:111` 使用
   `fmt.Sprintf("%s/%s", fc.ID, name)` 作为 state key。如果两个不同的 plugin 使用了相同的
   `{fcID}/{argName}` 模式，会导致 key 冲突。

9. **InjectSessionState 只接受 ReadonlyContext 但调用内部类型**：`util/instructionutil/instruction.go:42`
   做类型断言 `ctx.(*icontext.ReadonlyContext)`，如果用户传入自定义的 ReadonlyContext 实现会失败。

10. **Thread safety of PluginManager**: PluginManager 本身没有锁保护其 `plugins` slice。
    但 `registerPlugin` 只在 `NewPluginManager` 中调用一次（构造期），且插件列表之后不再修改，所以是安全的。
    但如果未来支持动态插件注册，需要加锁。

## 5. Deeper Follow-up Questions

1. **Plugin ordering semantics**: When both agent-native callbacks and plugin callbacks exist for the same hook point,
   what is the exact execution order? The tests suggest plugins execute first (via PluginManager), then agent-native
   callbacks — but is this contract documented and tested for all 15 hook points?

2. **What happens when a plugin's BeforeModelCallback returns both response AND error?** The test at
   `plugin_manager_test.go:536-550` verifies that error takes precedence — but what about state delta side-effects
   from the callback's `ctx.State().Set()` calls? Are they silently rolled back or do they persist in the event?

3. **Can `functioncallmodifier` and `retryandreflect` coexist for the same tool?** The ordering would determine
   whether schema args are stripped before or after retry logic sees the error. This could break expected behavior.

4. **How does `instructionutil.InjectSessionState` handle nested template expansion?** If `{key_name}` resolves
   to a string that itself contains `{another_key}`, does it get expanded recursively?

5. **Is there a mechanism to prevent plugins from writing state keys that collide with agent-defined state keys?**
   The `callbackContextState` allows `Set()` on any key — no namespacing per-plugin or per-agent.

6. **How does the `closeTimeout` in `plugininternal.PluginConfig` get used?** The field is stored on PluginManager
   but never referenced in `Close()` — is this a work-in-progress feature for graceful shutdown with deadlines?

7. **Why does `replayplugin` have its own `BeforeModelCallback` instead of using `recordplugin`'s output directly?**
   The replay plugin mocks the model entirely, making it a testing-only plugin. Could this be generalized as a
   "cached response" plugin for production use (like memoization)?

8. **What's the performance impact of having 10+ plugins registered for every tool call?** Each hook iterates
   over all plugins. The early-exit semantics help, but only if one plugin intercepts; the common case of
   "all return nil, nil" still does O(n) iteration per hook.

9. **Does `functioncallmodifier` correctly handle the case where the same tool appears in both
   `req.Config.Tools` and `req.Tools`?** The BeforeModel checks `req.Tools[decl.Name]` existence (L64-66)
   but the AfterModel only scans `llmResponse.Content.Parts` — what about streaming partial responses?

10. **Are there plans for middleware/server-side plugins (e.g., webhook-style) vs the current in-process model?**
    The PluginManager is entirely in-process. A gRPC/a2a-style plugin would enable cross-language plugins
    (like the Python ADK plugin ecosystem) but adds network latency.
