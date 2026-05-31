# Phase 4: Agent CLI Debug Trace MVP

Phase 4 的目的，是给 Rive 自己建立一个后期 debug 用的 agent CLI 黑匣子。

Phase 1/2/3 已经能记录 evidence、agent fact、dispatch 状态。但这些都是 Rive 协议层的事实。它们能说明 runtime 接受了什么命令、写了什么事件、dispatch 如何变化，却不能回答另一个很关键的问题：

```text
外部 code agent CLI 当时到底收到了什么？
它输出了什么？
它调用了什么工具？
它的 session / status / permission / subagent 状态怎么变？
为什么它没有调用 team？
为什么它 report 的内容和现场不一致？
```

这些问题不能靠 dispatch ledger 解决，也不能靠 evidence substrate 解决。它们属于调试外部黑盒 agent CLI 的观测问题。

Phase 4 要做的是 **Agent CLI Debug Trace MVP**：通过 Codex 和 OpenCode 自己提供的 hook / plugin / event stream，把 agent CLI 的状态、输入、输出、工具调用和生命周期事件静默记录到 Rive 的 debug trace store。

## 1. Phase 4 不是什么

这块边界必须写硬。

Debug trace 不是 Rive 协议事实源。

- trace 不生成 `fact`。
- trace 不生成 `evidence_ref`。
- trace 不推进 dispatch 状态。
- trace 不推进 task / Work Graph 状态。
- trace 不参与验收、审批、完成判断。
- trace 不作为 agent 决策的协议输入。

trace 只是 Rive 开发和排障用的内部观测材料。它可以带 `workspace_id`、`agent_id`、`run_id`、`dispatch_id`、`external_session_id` 作为 correlation labels，方便 debug 时过滤；这些 label 不是事实引用，也不改变任何业务状态。

正确关系是：

```text
Codex/OpenCode hidden adapter
  -> vendor hook/plugin/event stream
  -> rive debug trace ingest
  -> private debug trace store
  -> rive debug trace query/export
```

错误关系是：

```text
trace -> evidence_ref -> fact/report/dispatch
```

Phase 4 只做前者。

## 2. 为什么必须走 agent CLI 自己的入口

Rive 面对的是外部黑盒 CLI agent：Codex、OpenCode、Claude、Gemini 等。它们不共享 SDK，也不一定共享 MCP/ACP。但如果我们只从 Rive runtime 看命令，就会错过 agent 内部真正发生的事情。

例如：

- agent 收到了 prompt，但没有执行 `team report`。
- agent 调了 shell/tool，但 Rive 只看到最后的 report。
- agent permission 被拒绝，dispatch 看起来只是卡住。
- agent 输出了错误信息，但没有写入 fact。
- agent subagent 或子任务结束了，但父 session 没正确汇报。

因此 Phase 4 不优先做 PTY 录屏，也不把 trace 混进业务 ledger。它先使用 Codex/OpenCode 的结构化入口。这样得到的 trace 更靠近 agent CLI 内部事件，噪声比 terminal transcript 更少，语义也更稳定。

## 3. Codex Adapter

Codex 当前有三类可用入口。Phase 4 v0 先落 hook ingest，后续再加 app-server / rollout trace ingest。

### 3.1 Codex lifecycle hooks

本地 Codex 源码里有 `codex-rs/hooks`。hook 事件包括：

```text
SessionStart
UserPromptSubmit
PreToolUse
PermissionRequest
PostToolUse
PreCompact
PostCompact
SubagentStart
SubagentStop
Stop
```

hook input schema 里有：

```text
session_id
turn_id
agent_id?
agent_type?
transcript_path
cwd
model
permission_mode
hook_event_name
```

tool 相关 hook 还包含：

```text
tool_name
tool_input
tool_response
tool_use_id
```

Rive 的 Codex hook adapter 应该把这些 stdin JSON 原样转进 debug trace store：

```text
codex hook stdin JSON
  -> rive debug trace ingest --adapter codex-hook --stdin
```

v0 可以记录：

- session start / stop
- user prompt submit
- tool use before / after
- permission request
- subagent start / stop
- transcript path
- cwd / model / permission mode

限制：

- Codex hook 不是完整 token stream。
- assistant 输出流和 command output delta 不一定能只靠 hook 拿全。
- hook 是低侵入入口，不是完整 session observer。

### 3.2 Codex app-server

Codex `app-server` 是 JSON-RPC control/event surface，支持 stdio、websocket、unix socket 等 transport。它的事件流包含：

```text
turn/started
item/started
item/completed
item/agentMessage/delta
item/commandExecution/outputDelta
turn/completed
experimentalRawEvents
```

如果后续 Rive 负责启动 Codex，app-server 是更完整的 trace 入口。它可以记录 assistant 输出 delta、command output delta、turn/item 生命周期。

Phase 4 v0 不强依赖这条，因为它要求 Rive 控制 Codex 启动方式。v0 先做 hook ingest，保留 app-server importer seam。

### 3.3 Codex rollout trace

Codex rollout trace 支持通过 `CODEX_ROLLOUT_TRACE_ROOT` 写本地 trace bundle：

```text
manifest.json
trace.jsonl
payloads/*.json
state.json?
```

它能覆盖 prompts、responses、tool inputs/outputs、terminal output、multi-agent child thread edges。这个很接近 Rive 未来想要的完整 debug bundle。

Phase 4 v0 可以先不直接 parse rollout bundle，但 debug trace store 的对象模型必须能容纳这一类离线导入。

## 4. OpenCode Adapter

OpenCode 更适合做真正的插件式 trace。

### 4.1 Local plugin

OpenCode 支持本地插件：

```text
.opencode/plugins/
~/.config/opencode/plugins/
```

插件启动时自动加载，可以订阅 event，也可以接 tool/chat/permission/shell hook。v0 推荐生成 workspace-local 插件：

```text
.opencode/plugins/rive-trace.ts
```

插件把 event/hook payload 转发给：

```text
rive debug trace ingest --adapter opencode-plugin --stdin
```

OpenCode 官方事件和源码 hook 覆盖：

```text
message.updated
message.part.updated
permission.asked
permission.replied
session.created
session.status
session.idle
session.error
session.diff
session.updated
tool.execute.before
tool.execute.after
shell.env
command.executed
chat.message
chat.params
permission.ask
```

v0 可以记录：

- session lifecycle
- message / message part updates
- tool before / after
- permission ask/reply
- shell env changes
- command executed
- status / idle / error

### 4.2 Server event stream

OpenCode `opencode serve` 暴露 `/event` Server-Sent Events。Rive 如果控制或发现 OpenCode server，可以旁路订阅统一事件流。

Phase 4 v0 不先做 server subscriber，因为 plugin ingest 更贴近“隐蔽插件”模型，也更容易按 workspace 安装。保留 SSE importer seam。

### 4.3 JSON run / export

OpenCode `run --format json` 和 `export` 可以作为测试或离线导入入口。它们不是低侵入后台 trace，但适合做 replay/importer。

## 5. Debug Trace Store

Debug trace store 是独立调试平面。它可以和 `.rive/rive.db` 同库，也可以有独立表；关键是语义上不能进入 fact/evidence/dispatch projection。

最小表：

```text
debug_trace_sessions
  trace_session_id
  workspace_id
  adapter
  external_session_id?
  agent_id?
  run_id?
  dispatch_id?
  cwd?
  started_at
  ended_at?
  metadata_json

debug_trace_raw_events
  raw_event_id
  trace_session_id?
  workspace_id
  adapter
  external_event_type
  external_event_id?
  sequence?
  received_at
  payload_hash
  payload_blob_ref

debug_trace_events
  trace_event_id
  raw_event_id
  trace_session_id?
  workspace_id
  adapter
  event_kind
  occurred_at?
  sequence?
  agent_id?
  run_id?
  dispatch_id?
  external_session_id?
  external_turn_id?
  external_tool_id?
  summary_json
```

`payload_blob_ref` 可以复用已有 blob 写入机制，但它不是 `evidence_ref`，只属于 debug trace namespace。

## 6. Normalized Event Kinds

v0 统一后的 event kind 先控制在小集合：

```text
session_started
session_status_changed
session_idle
session_error
session_ended
user_prompt
assistant_output
assistant_output_delta
tool_call_started
tool_call_completed
tool_call_failed
permission_requested
permission_resolved
subagent_started
subagent_stopped
command_executed
file_changed
unknown
```

normalization 的原则：

- raw payload 永远保存。
- normalized event 只做 debug 查询索引。
- unknown event 不丢弃，统一进入 `unknown`，原始 payload 仍可查。
- normalized field 不作为业务协议字段。

## 7. CLI Surface

Phase 4 命令放在 `rive debug trace` 下，避免和协议命令混淆。

```text
rive debug trace install codex --workspace <path>
rive debug trace install opencode --workspace <path>
rive debug trace uninstall codex|opencode --workspace <path>

rive debug trace ingest --adapter codex-hook --stdin
rive debug trace ingest --adapter opencode-plugin --stdin

rive debug trace list [--adapter codex|opencode] [--agent <id>] [--dispatch <id>]
rive debug trace show <trace_event_id|raw_event_id>
rive debug trace session <trace_session_id>
rive debug trace export <trace_session_id>
```

v0 implementation priority：

1. `ingest`
2. raw store
3. normalized projection
4. `list/show/session`
5. install template for Codex hook
6. install template for OpenCode plugin

`install` 生成配置或插件文件，但不能删除用户已有配置。若需要修改已有文件，必须保留备份或只追加 Rive-managed block。

## 8. Adapter Boundary

每个 adapter 都必须遵守同一条边界：

```text
vendor event payload
  -> adapter metadata
  -> raw debug trace event
  -> normalized debug trace event
```

禁止：

- adapter 直接写 `facts`。
- adapter 直接写 `dispatches`。
- adapter 直接写 `snapshots` / `evidence`。
- adapter 根据 trace 内容推断 `reported` / `done` / `failed`。
- adapter 让 agent 读取 trace 后做协议分支。

允许：

- 保存 raw payload。
- 保存 normalized debug event。
- 保存 correlation labels。
- 提供 debug query/export。
- 后续由人类开发者手动查看 trace，判断 bug 原因。

## 9. Privacy and Safety Boundary

Debug trace 会记录 prompts、tool input/output、terminal output、permission payload 等高敏内容。v0 必须默认本地保存，不做外部上传。

最小规则：

- trace store 只在 workspace 的 `.rive` 下。
- 不默认同步到 Git。
- 安装 adapter 时给出明确提示：它会记录 agent CLI 的输入、输出和工具事件。
- `export` 是显式命令。
- 后续可加 redact rules，但 v0 不把 redact 当作主功能。

## 10. Phase 4 验收线

Phase 4 通过的标准：

- Codex hook payload 可以通过 `rive debug trace ingest --adapter codex-hook --stdin` 写入 raw store。
- OpenCode plugin payload 可以通过 `rive debug trace ingest --adapter opencode-plugin --stdin` 写入 raw store。
- raw payload 有 hash/blob，可复查。
- normalized debug events 可 list/show。
- unknown vendor event 不丢失。
- install 命令能生成 Codex/OpenCode adapter 文件或配置，并且不破坏已有用户配置。
- trace event 可以按 workspace/adapter/session/agent/dispatch label 过滤。
- trace 写入不会产生 fact/evidence/dispatch/task/graph 事件。
- 无 Codex/OpenCode 安装时，ingest/query 测试仍可用 fake payload 跑通。

一句话：Phase 4 只解决 Rive 后期 debug 的观测问题，不解决协议事实问题。
