# 第九章：Phase 4 Debug Trace 测试计划

## 1. 测试目标

Phase 4 只验证 **Agent CLI Debug Trace Adapter MVP**。

测试目标是确认 Rive 可以通过 Codex / OpenCode 的 hook、plugin 或 event payload，静默写入私有 debug trace store，并能用 `rive debug trace ...` 查询。Trace 只服务 Rive 后期排障，不进入协议事实层。

```text
Codex/OpenCode vendor payload
  -> rive debug trace ingest
  -> raw debug trace event
  -> normalized debug trace event
  -> debug query/export
```

本阶段不测试 evidence/fact/dispatch 状态推进，不测试 Work Graph，不测试 PTY transcript，不要求真实 Codex/OpenCode 安装。

## 2. 关键验收线

1. `rive debug trace ingest --adapter codex-hook --stdin` 能写入 raw payload 和 normalized event。
2. `rive debug trace ingest --adapter opencode-plugin --stdin` 能写入 raw payload 和 normalized event。
3. raw payload 必须有 hash/blob/ref，可复查原始 JSON。
4. unknown vendor event 必须保存，不丢失，normalized kind 为 `unknown`。
5. `rive debug trace list/show/session/export` 能查询 trace。
6. trace 可按 workspace、adapter、session、agent、dispatch label 过滤。
7. install 命令能生成 Codex/OpenCode adapter 配置/插件，并且不破坏已有用户配置。
8. trace 写入不能产生 fact/evidence/dispatch/task/graph 业务事件或 projection mutation。
9. 没有 Codex/OpenCode 安装时，fake payload ingest/query 仍可完整跑通。
10. trace store 默认本地、workspace-private，不做外部上传。

## 3. 测试分层

### Unit Tests

- Adapter enum：只接受 `codex-hook`、`opencode-plugin`，未知 adapter 返回稳定错误。
- Raw payload hash：相同 payload hash 稳定；payload 变化 hash 变化。
- Raw payload blob：blob ref 能反查原始 JSON bytes。
- Normalization：
  - Codex `SessionStart` -> `session_started`
  - Codex `UserPromptSubmit` -> `user_prompt`
  - Codex `PreToolUse` -> `tool_call_started`
  - Codex `PostToolUse` -> `tool_call_completed` 或 `tool_call_failed`
  - Codex `PermissionRequest` -> `permission_requested`
  - Codex `SubagentStart/SubagentStop` -> `subagent_started/subagent_stopped`
  - Codex `Stop` -> `session_ended`
  - OpenCode `session.created` -> `session_started`
  - OpenCode `session.status` / `session.idle` / `session.error` -> 对应 status/error/idle
  - OpenCode `message.updated` / `message.part.updated` -> `assistant_output` 或 `assistant_output_delta`
  - OpenCode `tool.execute.before/after` -> `tool_call_started/tool_call_completed`
  - OpenCode `permission.asked/replied` -> `permission_requested/permission_resolved`
  - OpenCode `command.executed` -> `command_executed`
  - unknown event -> `unknown`
- Correlation labels：session_id、turn_id、agent_id、run_id、dispatch_id、cwd 等只作为 query labels，不进入 business projection。
- Error envelope：invalid JSON、missing adapter、unsupported adapter、workspace not initialized 有稳定 error code。

### Integration Tests

- 初始化 workspace 后 ingest Codex fake payload，能在 raw/normalized trace 表中查询到。
- 初始化 workspace 后 ingest OpenCode fake payload，能在 raw/normalized trace 表中查询到。
- 多个 payload 进入同一 external session 后，`trace session <id>` 能按 sequence/time 返回完整 timeline。
- `trace list --adapter codex-hook` 只返回 Codex adapter event。
- `trace list --adapter opencode-plugin` 只返回 OpenCode adapter event。
- `trace list --agent <id>` / `--dispatch <id>` 只按 label 过滤，不要求 agent/dispatch 真存在。
- `trace show <raw_event_id>` 能展示 raw payload hash/blob/ref 和 normalized summary。
- `trace show <trace_event_id>` 能展示 normalized event，并能回链 raw payload。
- `trace export <trace_session_id>` 输出可重放 JSON/JSONL，且不包含业务 projection。

### CLI Contract Tests

- 成功命令 exit code 为 0，stdout 是 JSON envelope。
- 失败命令 exit code 非 0，并输出稳定 JSON error envelope。
- `ingest --stdin` 必须从 stdin 读取完整 payload，支持长 JSON。
- 所有 read model 保持 `protocol` / `display` 分层。
- `display.message/summary` 不参与测试控制流。
- install 命令输出 generated file paths 和 changed/skipped status。
- `trace list/show/session/export` 在空 trace store 下有稳定空结果/错误。

## 4. Fake Payload Fixtures

测试不依赖真实 Codex/OpenCode 安装。至少准备这些 fixture：

### Codex Hook

```json
{
  "hook_event_name": "SessionStart",
  "session_id": "codex_s_1",
  "turn_id": "turn_1",
  "cwd": "/tmp/rive-demo",
  "model": "gpt-5",
  "permission_mode": "default",
  "transcript_path": "/tmp/codex/transcript.jsonl"
}
```

```json
{
  "hook_event_name": "PostToolUse",
  "session_id": "codex_s_1",
  "turn_id": "turn_1",
  "tool_name": "shell",
  "tool_use_id": "tool_1",
  "tool_input": { "cmd": "cargo test" },
  "tool_response": { "exit_code": 0, "stdout": "ok" }
}
```

### OpenCode Plugin

```json
{
  "type": "session.created",
  "session": { "id": "opencode_s_1" },
  "cwd": "/tmp/rive-demo"
}
```

```json
{
  "type": "tool.execute.after",
  "session": { "id": "opencode_s_1" },
  "tool": { "id": "tool_1", "name": "bash" },
  "output": { "exit": 0, "stdout": "ok" }
}
```

### Unknown Event

```json
{
  "type": "vendor.new.event",
  "session": { "id": "unknown_s_1" },
  "payload": { "still": "preserved" }
}
```

## 5. 手动验收流程

最小人工验收流程：

```bash
tmp=$(mktemp -d)
cd "$tmp"
rive init .

cat codex-session.json | \
  rive debug trace ingest --adapter codex-hook --stdin

cat opencode-tool.json | \
  rive debug trace ingest --adapter opencode-plugin --stdin

rive debug trace list
rive debug trace list --adapter codex-hook
rive debug trace show <trace_event_id>
rive debug trace session <trace_session_id>
rive debug trace export <trace_session_id>
```

检查点：

- raw payload hash/blob/ref 存在。
- normalized event kind 正确。
- unknown event 被保存为 `unknown`。
- session timeline 能看到同 session 的多条事件。
- filter 按 adapter/session/agent/dispatch label 工作。
- `.rive` 下没有外部上传或远程配置。

## 6. Install Template Tests

### Codex

`rive debug trace install codex --workspace <path>` 验收：

- 生成 Rive-managed Codex hook 配置或脚本。
- hook command 指向 `rive debug trace ingest --adapter codex-hook --stdin`。
- 如果目标文件已存在：
  - 不覆盖未知用户内容；或
  - 只追加 Rive-managed block；或
  - 创建备份后修改。
- 重复 install 是幂等的。
- uninstall 只移除 Rive-managed block / 文件，不删除用户内容。

### OpenCode

`rive debug trace install opencode --workspace <path>` 验收：

- 生成 `.opencode/plugins/rive-trace.ts` 或等价插件文件。
- 插件把 event/hook payload 转发到 `rive debug trace ingest --adapter opencode-plugin --stdin`。
- 不破坏已有 `.opencode/plugins` 下用户插件。
- 重复 install 是幂等的。
- uninstall 只移除 Rive-managed 插件。

## 7. Business-State Non-Mutation Tests

Phase 4 最重要的不变量：

```text
debug trace != evidence
debug trace != fact
debug trace != dispatch transition
debug trace != task/work graph state
```

测试要求：

- ingest 前后 `facts` row count 不变。
- ingest 前后 `snapshots` row count 不变。
- ingest 前后 `dispatches` row count 不变。
- 不出现 `agent.fact.recorded`、`dispatch.*`、`evidence.snapshot_captured` 业务事件。
- 不出现 `work.*`、`task.*`、`node.*`、`graph.*` event。
- trace raw payload blob ref 不出现在 evidence refs。
- `rive fact list`、`rive evidence list`、`rive dispatch list` 不展示 trace event。

## 8. Privacy / Safety Tests

- trace store 只写 workspace `.rive`。
- `export` 必须显式调用；ingest/install 不外发网络请求。
- payload 中包含 prompt/tool output 时，raw payload 可以复查，但不会默认打印全部敏感内容到 list summary。
- install 命令的 display 文案明确说明会记录 agent CLI 输入、输出和工具事件。

## 9. Done Definition

task #32 完成条件：

- 测试计划覆盖 Phase 4 范围、非范围和隐私/副作用边界。
- 实现完成后 unit/integration/CLI contract tests 全部通过。
- fake Codex hook payload ingest/query/export 通过。
- fake OpenCode plugin payload ingest/query/export 通过。
- unknown event preserved。
- install/uninstall template 不破坏已有配置。
- 无 Codex/OpenCode 安装环境下测试通过。
- trace 写入不改变 evidence/fact/dispatch/task/graph 任何业务状态。
