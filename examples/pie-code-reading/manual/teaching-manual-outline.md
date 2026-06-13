# pie Agent Runtime 教学手册细纲

> 合成日期：2026-06-13
> 输入 artifact：粗读 7 份报告 + 精读 10 份报告
> 源码仓库：`/Users/likun/Desktop/workspace-for-pie-agent/pie`
> 阅读基线：`f1c35a3`
> **定位：教学细纲，不是生产安全审计报告。它面向想理解、复刻或维护 pie agent runtime 的工程师。**

---

## 0. 教学路线图（120–150 分钟）

| 段 | 章节 | 主题 | 建议时长 | 累积 | 转场逻辑 |
|----|------|------|----------|------|----------|
| 1 | Ch01 | Agent Runtime 全景：CLI/TUI → Harness → AgentLoop → Provider/Tool → Session | 18 min | 18 | 开场：先跑通一次普通 coding turn，把所有层级挂在一张图上 |
| 2 | Ch02 | `pie-ai`：Provider 抽象、Streaming Event、Tool Call 统一模型 | 18 min | 36 | 从一次 turn 进入模型层："模型返回的流式事件到底长什么样？" |
| 3 | Ch03 | `pie-agent-core`：Agent 状态机、Harness、Session JSONL、Compaction | 20 min | 56 | 从模型输出回到 runtime："怎么持久化、恢复、压缩、继续？" |
| 4 | Ch04 | Tools / Permission / LSP：工具执行、控制面提示、诊断注入 | 18 min | 74 | 从 AgentLoop 到行动："模型想改文件，runtime 如何安全执行？" |
| 5 | Ch05 | Triggers / Cron / Loops / Inbox：让 agent 主动工作 | 20 min | 94 | 从交互式 agent 到自动化 agent："没人发 prompt 时它怎么工作？" |
| 6 | Ch06 | MCP / Notification / Web Relay：外部工具和远程 UI 表面 | 14 min | 108 | 从本地自动化到外部系统："外部事件和远程 viewer 如何接入？" |
| 7 | Ch07 | Goal / OnTurnEndHook：自动继续、停止条件和 evaluator | 12 min | 120 | 从一次任务到自我收敛："agent 怎么知道目标完成了？" |
| 8 | Ch08 | 风险与测试路线：Provider 一致性、Session 完整性、自动化安全 | 18 min | 138 | 收官：把前面所有能力转成可维护的测试和风险清单 |
| — | Q&A | 自由问答 + 下一轮 DAG | 12 min | 150 | 把问题转成后续 Rive 工作流 |

**主线串联**：Coding Turn → Provider Stream → Agent Harness → Tool Execution → Automation Loop → External Events → Goal Loop → Hardening Roadmap

**核心 thesis**：

1. pie 不是一个单纯 TUI，而是一个本地 agent runtime：模型、工具、会话、自动化、外部事件都在同一个 harness 中收敛。
2. pie 的可维护性来自分层：`pie-ai` 只处理模型流，`pie-agent-core` 只处理 agent 生命周期，`pie-coding-agent` 只处理用户产品面。
3. 自动化能力的关键不是 cron 本身，而是 `Trigger` 信封、去重/循环抑制、子代理隔离和结果回投 inbox。
4. 长会话能力的关键不是保存聊天记录，而是 append-only JSONL + parent DAG + compaction summary + resume context rebuild。
5. Provider 统一的难点不是 API key，而是 streaming tool-call arguments、reasoning、usage、cache、abort、retry 的跨供应商一致性。
6. pie 当前最值得继续建设的是安全/一致性测试：子代理权限白名单、JSONL 截断恢复、跨 Provider conformance、自动化 feedback-loop 防护。

### 如果现场只有 45 分钟怎么压缩

1. **Ch01 必讲**：只画全局架构图和一次 coding turn，压到 10 分钟。
2. **Ch02 只讲 Provider event 模型**：不要展开所有 provider，压到 7 分钟。
3. **Ch03 只讲 Session JSONL + compaction**：压到 8 分钟。
4. **Ch05 必讲**：这是 pie 区别于普通 coding agent 的核心，压到 10 分钟。
5. **Ch08 必讲风险 Top 5**：压到 6 分钟。
6. Ch04/Ch06/Ch07 留作课后阅读，每章给一个源码入口即可。

45 分钟核心目标：让听众记住 **"一次 coding turn 如何跑"**、**"会话如何恢复"**、**"自动化如何闭环"**、**"哪里最危险"** 四件事。

---

## Chapter 01 — Agent Runtime 全景：CLI/TUI → Harness → AgentLoop → Provider/Tool → Session（18 min）

> 核心源码：`crates/coding-agent/src/main.rs`, `crates/coding-agent/src/tui.rs`, `crates/agent/src/harness/agent_harness.rs`, `crates/agent/src/agent.rs`, `crates/agent/src/agent_loop.rs`, `crates/ai/src/stream.rs`, `crates/coding-agent/src/tools/`, `crates/agent/src/harness/session/`
> 对应材料：`manual/00-overview.md`, `manual/03-coding-cli-tools.md`, `manual/02-agent-core-runtime.md`

### 讲解目标

学完本章，听众应能：

1. 画出 pie 的五层结构：`pie-coding-agent` → `AgentHarness` → `Agent/AgentLoop` → `pie-ai` → `tools/session`。
2. 解释为什么 `Agent` 本身是 IO-free 状态机，而真正的文件系统、session、trigger、cost、tool 权限都在 Harness 和 CLI 层装配。
3. 追踪一次普通 coding turn：用户输入 → TUI/feed → Harness prompt → model stream → tool call → tool result → final answer → JSONL session。
4. 区分三种状态：模型上下文里的 messages、session JSONL 里的 entries、TUI feed 里的展示事件。
5. 解释 pie 为什么适合长期自动化：它不是只把 prompt 发给 LLM，而是有恢复、触发、成本和事件账本。

### 问题背景

一个终端 coding agent 看起来只是"用户输入一句话，模型回复一句话"。但真实工程里，一次 turn 背后至少有这些问题：

- CLI 要解析模型、provider、thinking level、resume、image、trigger poll interval 等启动参数。
- TUI 要一边展示 streaming tokens，一边展示工具执行进度、成本、诊断和后台 trigger 状态。
- 模型可能返回 tool calls，runtime 要先做权限判断，再调用本地工具，再把 tool result 喂回模型。
- 会话要能退出后恢复，所以每个消息、model change、thinking change、compaction 都要落到持久化日志。
- 长会话会超过 context window，需要自动 compaction，而不是每次完整重放。
- 自动化 trigger 可能在用户没输入时启动子代理，不能污染主会话但又要可审计。

### 为什么难

难点不在"调用模型"，而在 **不同时间尺度的状态同时存在**：

| 时间尺度 | 例子 | 需要谁负责 |
|----------|------|------------|
| 毫秒级 | SSE chunk、tool progress、TUI redraw | `pie-ai` stream + UI feed |
| 秒级 | 一次 AgentLoop，多轮 model/tool 循环 | `Agent` / `agent_loop.rs` |
| 会话级 | messages、model change、thinking level、compaction | Session JSONL |
| 跨会话 | user config、auth、models、MCP、skills、inbox | `~/.pie/` |
| 长期自动化 | cron/trigger rules、loop state、inbox finding | trigger runtime + sidecar |

如果把这些都塞进一个 `main.rs`，系统会变成不可恢复、不可测试、不可自动化的脚本。

### 核心抽象

推荐先画这张总图：

```text
User input / CLI args
  │
  ▼
pie-coding-agent
  ├─ main.rs/config/model/session discovery
  ├─ TUI/Web/feed/render
  ├─ tools: read/write/edit/bash/git/memory/mcp_adapter/...
  ├─ commands: /model /sessions /compact /goal /cron /triggers /inbox
  └─ hooks: LSP, OTLP, lifecycle hooks
        │
        ▼
AgentHarness
  ├─ SessionStorage / JsonlSessionStorage
  ├─ Compaction / CostTracker
  ├─ TriggerRuntime / NotificationHook
  ├─ tool permission hooks
  └─ wraps Agent
        │
        ▼
Agent / AgentLoop
  ├─ state.messages
  ├─ call_llm(stream_fn)
  ├─ execute_tools
  ├─ before/after/should_stop/prepare_next_turn hooks
  └─ emit AgentEvent
        │
        ▼
pie-ai
  ├─ ApiProvider registry
  ├─ provider-specific streaming parser
  └─ AssistantMessageEvent stream
        │
        ▼
Session JSONL + TUI feed + Cost + Debug logs
```

### 源码走读路线

1. `crates/coding-agent/src/main.rs`：启动入口。讲 CLI 参数如何变成 config、model、session、harness options。
2. `crates/coding-agent/src/tui.rs` 与 `ui/`：讲 UI feed 和 headless/web/TUI 的展示边界。
3. `crates/agent/src/harness/agent_harness.rs`：讲它为什么是装配层，而不是普通 helper。
4. `crates/agent/src/agent.rs`：讲 `Agent` 是状态机：`prompt()` / `continue_()` / `abort()` / listener。
5. `crates/agent/src/agent_loop.rs`：讲核心循环：`call_llm` → stream events → execute tools → stop/continue。
6. `crates/ai/src/stream.rs`：讲 `stream_simple` 如何进入 provider registry。
7. `crates/coding-agent/src/tools/mod.rs`：讲工具集合如何注册到 Agent。
8. `crates/agent/src/harness/session/jsonl_storage.rs`：讲消息如何落盘。

### 演示建议

1. **白板画一次 ordinary coding turn**（4 min）：只画主链路，不讲自动化。
2. **投屏看 `agent_loop.rs`**（4 min）：找出 call model、tool execution、stop 判断三个位置。
3. **投屏看 session JSONL**（3 min）：运行一次短对话后看 `~/.pie/sessions/...jsonl` 里的 entry 类型。
4. **展示 `/cost` 或成本累加路径**（2 min）：说明 usage 不只是展示，而是长期自动化的预算基础。
5. **展示 `/triggers` 或 `/cron` 命令入口**（2 min）：给 Ch05 埋伏笔。

### 容易误解点

1. **"pie-coding-agent 就是整个 agent"**：不对，它只是产品壳和工具壳；核心状态机在 `crates/agent`。
2. **"AgentHarness 是可选包装"**：对简单测试可以不用，但生产能力如 session、compaction、trigger、cost 都在 Harness。
3. **"TUI feed 等于 session state"**：不对，TUI 是展示模型，session JSONL 才是持久化账本。
4. **"工具执行只是模型调用的一部分"**：工具执行是 runtime 的权限边界，不能被 provider 层吞掉。
5. **"自动化只是 cron"**：cron 只是 trigger source，真正闭环在 TriggerRuntime + sub-agent + inbox。

### 练习题

- **Q1**：用户输入 `"fix failing tests"` 后，画出从 `main.rs` 到 `SessionStorage.append_entry` 的完整路径。
- **Q2**：`Agent` 为什么不直接读写文件？这样设计对测试有什么好处？
- **Q3**：如果模型返回两个 tool calls，`agent_loop.rs` 里哪些 hook 会被触发？
- **Q4**：TUI 显示一条 tool progress，是否意味着 session JSONL 中一定有对应 entry？
- **Q5**：如果要新增一个 headless batch mode，应改 `crates/agent` 还是 `crates/coding-agent`？为什么？

### 代码附录

| 文件 | 讲解重点 | 课堂定位 |
|------|----------|----------|
| `crates/coding-agent/src/main.rs` | CLI 启动、model/session/harness 装配 | 产品入口 |
| `crates/coding-agent/src/tui.rs` | TUI feed 与交互事件 | 展示层 |
| `crates/agent/src/harness/agent_harness.rs` | session/compaction/cost/trigger 装配 | Runtime 外壳 |
| `crates/agent/src/agent.rs` | Agent 状态机 | Runtime 内核 |
| `crates/agent/src/agent_loop.rs` | model/tool 循环 | 最关键源码 |
| `crates/ai/src/stream.rs` | provider registry 调用 | 模型入口 |
| `crates/coding-agent/src/tools/` | 工具实现 | 本地行动层 |
| `crates/agent/src/harness/session/` | JSONL session | 持久化层 |

---

## Chapter 02 — `pie-ai`：Provider 抽象、Streaming Event、Tool Call 统一模型（18 min）

> 核心源码：`crates/ai/src/types.rs`, `crates/ai/src/stream.rs`, `crates/ai/src/api_registry.rs`, `crates/ai/src/providers/`, `crates/ai/src/utils/sse.rs`, `crates/ai/src/utils/json_parse.rs`, `crates/ai/src/models.rs`, `crates/ai/src/models_generated.rs`, `docs/ds4.md`
> 对应材料：`manual/01-ai-provider-streaming.md`, `manual/deep-read/05-tool-call-parsing-matrix.md`, `manual/deep-read/09-provider-conformance-test-plan.md`

### 讲解目标

学完本章，听众应能：

1. 解释 `pie-ai` 的职责：把多个 provider 的异构 streaming protocol 统一成 `AssistantMessageEvent`。
2. 区分 `Message`、`ContentBlock`、`AssistantMessageEvent`、`Usage`、`ThinkingLevel`、`Model`。
3. 说明 OpenAI Responses、Anthropic、Google、Bedrock 在 tool call arguments 解析上的差异。
4. 解释为什么 streaming tool call 比最终 JSON response 更难：partial JSON、content index、parallel calls、finish event。
5. 讲清楚 DS4/local model 的 prefix cache 优化：reasoning replay、409 retry、cache accounting。
6. 根据 provider conformance test plan 设计一组 mock-server 测试。

### 问题背景

编码 agent 的 provider 层不是简单的 `POST /chat/completions`。真实模型供应商存在这些差异：

- OpenAI Responses 有独立 response item、function call arguments delta、prompt cache。
- Anthropic Messages 使用 content block start/delta/stop，tool input delta 是碎片。
- Google Gemini 可能一次性给出 `functionCall` object，不需要 partial JSON。
- Bedrock Converse 使用 AWS EventStream 二进制 frame。
- Local DS4 需要 OpenAI-compatible surface，但对 reasoning replay 和 KV cache 非常敏感。
- 不同 provider 对 reasoning、usage、cache read/write、abort、retry、tool call ID 的定义都不一致。

### 为什么难

最难的是 **流式中间态**：

```text
provider event stream
  ├─ text delta
  ├─ reasoning/thinking delta
  ├─ tool call name appears before arguments complete
  ├─ arguments arrive as partial JSON chunks
  ├─ usage may appear at end, or per chunk, or not at all
  └─ finish reason may arrive before consumer has a valid JSON object
```

AgentLoop 不能理解每个 provider 的原始事件，所以 `pie-ai` 必须在边界上统一。

### 核心抽象

```text
SimpleStreamOptions
  ├─ model: Model
  ├─ messages: Vec<Message>
  ├─ tools: Vec<ToolDefinition>
  ├─ thinking: ThinkingLevel
  └─ abort/cost/cache options
        │
        ▼
stream_simple / stream
        │
        ▼
api_registry.resolve(model.api/provider)
        │
        ▼
ApiProvider::stream(options)
        │
        ▼
provider-specific decoder
        │
        ▼
Stream<AssistantMessageEvent>
```

统一事件模型的教学重点：

- `AssistantMessageEvent` 是 **增量事件**，不是最终消息。
- Provider 内部通常维护一个 partial assistant message，用事件不断填充。
- Tool call 需要同时表达：
  - call id；
  - tool/function name；
  - arguments delta；
  - arguments final JSON；
  - finish/stop condition。

### 源码走读路线

1. `crates/ai/src/types.rs`：先看统一类型 universe，不看 provider。
2. `crates/ai/src/stream.rs`：看 `stream_simple` 如何 resolve model/provider。
3. `crates/ai/src/api_registry.rs`：看 provider 注册表和 `RegisteredHandle`。
4. `crates/ai/src/providers/openai_responses.rs`：讲 Responses event mapping。
5. `crates/ai/src/providers/anthropic.rs`：讲 content block delta 和 input_json_delta。
6. `crates/ai/src/providers/google.rs` / `google_shared.rs`：讲一次性 `functionCall`。
7. `crates/ai/src/providers/amazon_bedrock.rs`：讲 AWS EventStream。
8. `crates/ai/src/providers/transform_messages.rs`：讲跨 provider handoff 和消息降级/规范化。
9. `crates/ai/src/utils/json_parse.rs`：讲 partial JSON 容错。
10. `docs/ds4.md`：讲本地模型 cache 和 409 recovery。

### Provider tool-call 矩阵（课堂简表）

| Provider | Tool args 到达方式 | 累积位置 | 最危险点 |
|----------|--------------------|----------|----------|
| OpenAI Responses | `function_call_arguments.delta` | response item / content index | 并行 tool call 与 `rposition()` 类定位风险 |
| Anthropic | `input_json_delta` | content block delta buffer | partial JSON 在 block stop 前不完整 |
| Google | `functionCall` object | 原子对象 | 和 stream delta 模型不一致 |
| Bedrock | `toolUse.input` eventstream | toolUse block | 二进制 AWS eventstream 解码和 JSON 合成 |
| OpenAI Completions | Chat chunk tool calls | choices delta | 多 choice / 多 tool call 合并 |

### 演示建议

1. **画一个 partial JSON 流**（3 min）：

   ```text
   {"city"
   :"San
    Francisco"}
   ```

   说明为什么每个 chunk 都不是合法 JSON，但 UI 仍希望看到 tool call 正在生成。

2. **对比 OpenAI vs Anthropic 事件流**（4 min）：投屏两个 provider parser 的核心 match 分支。
3. **展示 DS4 cache**（3 min）：解释 reasoning replay 为什么要 byte-exact，为什么 cache miss 会毁掉本地模型体验。
4. **写一个 mock-server fixture 设计**（4 min）：模拟 broken partial JSON、unicode escape、并行 tool calls。

### 容易误解点

1. **"provider 层只要返回最终 assistant message"**：不够，TUI/AgentLoop 需要 streaming delta 和 tool progress。
2. **"partial JSON 可以每次都 parse"**：不稳定，必须知道 stop condition 和容错策略。
3. **"Google 不 streaming tool args，所以更简单"**：简单的是 args，但整体仍要映射到统一事件模型。
4. **"DS4 是 OpenAI-compatible 所以不需要特殊处理"**：不对，prefix cache 和 reasoning replay 是关键。
5. **"Usage 一定在 response end 出现"**：不同 provider 差异很大，必须设计缺失值和 cache read/write 语义。

### 练习题

- **Q1**：为什么 `ToolCallDelta` 要保留原始 JSON 片段，而不是每个 chunk 都转成完整对象？
- **Q2**：如果 OpenAI Responses 同时生成两个 function call，当前实现哪些地方最可能出问题？
- **Q3**：设计一个 Anthropic `input_json_delta` fixture，覆盖 unicode 和嵌套对象。
- **Q4**：DS4 的 409 retry 应该发生在 provider 层还是 AgentLoop 层？为什么？
- **Q5**：如果新增一个 provider，它最低限度要实现哪些统一事件？

### 代码附录

| 文件 | 讲解重点 | 测试建议 |
|------|----------|----------|
| `crates/ai/src/types.rs` | `Message`, `ContentBlock`, `AssistantMessageEvent`, `Usage` | 类型快照测试 |
| `crates/ai/src/stream.rs` | stream 入口 | fake provider |
| `crates/ai/src/api_registry.rs` | provider 注册/解析 | registry 单元测试 |
| `providers/openai_responses.rs` | Responses SSE | mock SSE fixture |
| `providers/anthropic.rs` | input_json_delta | partial JSON fixture |
| `providers/google.rs` | Gemini functionCall | atomic function call fixture |
| `providers/amazon_bedrock.rs` | AWS EventStream | binary frame fixture |
| `utils/json_parse.rs` | partial JSON 修复 | fuzz / proptest |
| `docs/ds4.md` | DS4 cache 策略 | 手动 smoke |

---

## Chapter 03 — `pie-agent-core`：Agent、Harness、Session JSONL、Compaction（20 min）

> 核心源码：`crates/agent/src/agent.rs`, `crates/agent/src/agent_loop.rs`, `crates/agent/src/types.rs`, `crates/agent/src/harness/agent_harness.rs`, `crates/agent/src/harness/session/`, `crates/agent/src/harness/compaction/`
> 对应材料：`manual/02-agent-core-runtime.md`, `manual/deep-read/03-session-branch-model.md`, `manual/deep-read/08-session-integrity-review.md`

### 讲解目标

1. 区分 `Agent` 和 `AgentHarness` 的职责边界。
2. 解释 AgentLoop 的 tool execution、steering/follow-up queue、prepare/should-stop hooks。
3. 画出 append-only JSONL session 的 entry DAG 和 leaf 重放逻辑。
4. 解释 `Leaf` entry 为什么表示 undo/move，而不是删除历史。
5. 理解 compaction entry：`summary` + `first_kept_entry_id` 如何重建上下文。
6. 识别当前 session 完整性风险：截断、header 损坏、sidecar 一致性、全量加载。

### 问题背景

Coding agent 必须支持：

- `--resume` 恢复上次会话。
- `/undo` 回到某个历史节点继续。
- `/compact` 或自动 compaction 处理超长上下文。
- `/save` 或 export/import 迁移会话。
- Trigger/goal/automation 写入 custom entries。
- TUI 重放历史消息。

这些能力不能依赖内存状态；必须靠磁盘上的可恢复结构。

### 为什么难

最容易误解的是：session 不是一条线，而是一个 append-only log 表示的 DAG。

```text
session_info
  └─ message A
      ├─ message B
      │   └─ message C
      └─ Leaf(target=A)
          └─ message D   # undo 后的新分支
```

难点：

- JSONL 只能追加，不能原地修改。
- leaf 不是文件里的固定字段，而是重放所有 entry 得出。
- compaction 不能删除原始历史，只能追加摘要 entry。
- export/import 要带 sidecar，否则 triggers/cron 等自动化状态丢失。
- 崩溃中断可能产生半行 JSONL。

### 核心抽象

`SessionTreeEntry` 是本章核心：

| Entry | 作用 | 是否参与上下文 |
|-------|------|----------------|
| `Message` | 用户/助手/工具消息 | 是 |
| `CustomMessage` | 可展示且进入上下文的自定义消息 | 是 |
| `Custom` | trigger/goal 等审计事件 | 通常否 |
| `Compaction` | 摘要压缩点 | 通过 summary 参与 |
| `BranchSummary` | 分支摘要 | 参与 |
| `ThinkingLevelChange` | thinking level 切换 | 状态 |
| `ModelChange` | model 切换 | 状态 |
| `Leaf` | 显式跳转 leaf | 控制结构 |
| `Label` | 标注 entry | 元数据 |
| `SessionInfo` | session 元信息 | 元数据 |

### 源码走读路线

1. `crates/agent/src/agent.rs`：看 `Agent` 的 state、listener、prompt/continue/abort。
2. `crates/agent/src/agent_loop.rs`：看 `run_agent_loop` 和 tool execution。
3. `crates/agent/src/harness/session/session.rs`：看 `SessionTreeEntry` 和 `build_session_context`。
4. `crates/agent/src/harness/session/jsonl_storage.rs`：看 header、append、cache、current_leaf、get_path_to_root。
5. `crates/agent/src/harness/session/jsonl_repo.rs`：看 create/open/list/delete。
6. `crates/agent/src/harness/compaction/compaction.rs`：看 compaction setting、cut point、summary generation。
7. `crates/coding-agent/src/session_archive.rs`：看 export/import。

### 白板示例：JSONL 分支

```json
{"id":"s1","createdAt":"...","cwd":"..."}
{"type":"session_info","id":"a","parentId":null,"name":"demo"}
{"type":"message","id":"b","parentId":"a","message":{"role":"user","content":"fix tests"}}
{"type":"message","id":"c","parentId":"b","message":{"role":"assistant","content":"done"}}
{"type":"leaf","id":"d","parentId":"c","targetId":"b"}
{"type":"message","id":"e","parentId":"b","message":{"role":"user","content":"actually explain first"}}
```

讲解点：

- `c` 没被删除，只是不在当前 leaf path 上。
- 当前 leaf 是 `e`。
- `get_path_to_root(e)` 得到 `a -> b -> e`。

### 演示建议

1. **手写 6 行 JSONL**（5 min）：让听众判断 current leaf。
2. **投屏 `build_session_context`**（4 min）：解释 compaction summary 如何替代前缀。
3. **演示 `/undo` 语义**（3 min）：不是删历史，而是 append leaf。
4. **讲截断风险**（3 min）：半行 JSONL 导致整个 session 打不开，为什么这是 P0。

### 容易误解点

1. **"append-only 就天然安全"**：不对，崩溃半行、header 损坏、sidecar 丢失仍会破坏恢复。
2. **"compaction 会删除旧消息"**：不会，只是在上下文构造时跳过旧路径并插入 summary。
3. **"Leaf 是一个全局变量"**：不是，是日志重放结果。
4. **"Custom entry 都会进模型上下文"**：不一定，`Custom` 和 `CustomMessage` 语义不同。
5. **"JSONL 越长只是磁盘问题"**：不对，当前实现全量加载到 `Vec`，也是内存和启动时间问题。

### 练习题

- **Q1**：给定 10 行 JSONL，画出 parent DAG 和 current leaf path。
- **Q2**：为什么 compaction 的 `first_kept_entry_id` 必须是 entry id，而不是行号？
- **Q3**：如果最后一行 JSONL 写到一半崩溃，理想恢复策略是什么？
- **Q4**：`BranchSummary` 和 `Compaction` 的区别是什么？
- **Q5**：export/import 如果漏掉 `.cron.toml`，用户会看到什么症状？

### 代码附录

| 文件 | 讲解重点 |
|------|----------|
| `agent.rs` | Agent state machine |
| `agent_loop.rs` | model/tool loop |
| `harness/session/session.rs` | entry model + context rebuild |
| `harness/session/jsonl_storage.rs` | JSONL I/O + leaf replay |
| `harness/session/jsonl_repo.rs` | repo-level session CRUD |
| `harness/compaction/compaction.rs` | summary/cut point |
| `coding-agent/src/session_archive.rs` | export/import |

---

## Chapter 04 — Tools / Permission / LSP：行动边界和诊断反馈（18 min）

> 核心源码：`crates/coding-agent/src/tools/`, `crates/agent/src/types.rs`, `crates/agent/src/agent_loop.rs`, `crates/coding-agent/src/lsp_supervisor.rs`, `crates/coding-agent/src/lsp.rs`, `crates/coding-agent/src/main.rs`
> 对应材料：`manual/03-coding-cli-tools.md`, `manual/deep-read/04-lsp-integration-report.md`

### 讲解目标

1. 解释工具系统为什么是 coding agent 的权限边界，而不是 provider 层功能。
2. 画出 tool call 从 LLM event 到 `AgentTool::execute` 再到 tool result message 的流程。
3. 讲清楚 before/after tool hooks、control-plane prompt、permission policy 的作用。
4. 解释 LSP supervisor 为什么挂在 after_tool_call，而不是 edit 工具内部。
5. 能判断新增工具时应该放在 `crates/coding-agent/src/tools/` 还是 MCP。

### 问题背景

模型不是直接改文件，它只能提出 tool call。runtime 必须决定：

- 这个工具是否存在？
- 参数是否能被解析和验证？
- 是否需要用户确认？
- 执行时如何捕获 stdout/stderr、文件 diff、错误？
- 执行后是否要附加 LSP diagnostics？
- 结果如何回灌给模型？

### 为什么难

工具执行同时是：

- **能力边界**：文件、shell、网络、git 都可能有副作用。
- **上下文边界**：结果太长需要截断，但又要保留足够信息给模型。
- **用户信任边界**：危险操作需要确认，确认卡片不能泄露 secrets。
- **反馈边界**：LSP diagnostics 是额外反馈，不能改变 tool success/failure 的事实。

### 核心抽象

```text
AssistantMessageEvent::ToolCall
  │
  ▼
AgentLoop collects tool calls
  │
  ├─ classify permission
  ├─ before_tool_call hook / control-plane prompt
  ├─ AgentTool::execute(args)
  ├─ after_tool_call hook
  └─ ToolResultMessage
        │
        ▼
next model call
```

LSP 位置：

```text
edit/write/bash tool completes
  │
  ▼
after_tool_call hook
  │
  ├─ LspSupervisor observes changed file
  ├─ waits/queries diagnostics
  └─ appends diagnostics to tool result
```

### 源码走读路线

1. `crates/agent/src/types.rs`：看 `AgentTool`, `BeforeToolCallHook`, `AfterToolCallHook`, `PermissionClassification`。
2. `crates/agent/src/agent_loop.rs`：看工具收集、执行、结果回灌。
3. `crates/coding-agent/src/tools/mod.rs`：看默认工具集合。
4. `tools/read.rs`, `write.rs`, `edit.rs`, `bash.rs`, `git.rs`：讲核心 coding tools。
5. `tools/mcp_adapter.rs`：讲外部 MCP tool 如何变成 AgentTool。
6. `crates/coding-agent/src/lsp_supervisor.rs` 与 `lsp.rs`：讲 LSP 进程、诊断、缓存。
7. `crates/coding-agent/src/main.rs`：看 LSP hook 如何装配到 harness。

### 演示建议

1. **构造一个 edit tool call**（4 min）：画参数、权限判断、执行、result。
2. **展示危险 bash prompt**（3 min）：解释 control-plane prompt payload 为什么要 redaction-safe。
3. **展示 LSP diagnostic 注入**（4 min）：让 edit 引入错误，after hook 增加诊断。
4. **对比内置 tool 和 MCP tool**（3 min）：内置工具有本地权限语义，MCP 是协议适配。

### 容易误解点

1. **"tool call 是 provider 的职责"**：provider 只负责表达 tool call，执行必须在 runtime。
2. **"after_tool_call 可以改变执行事实"**：它可以改 result content，但不应篡改底层成功/失败事实。
3. **"LSP 应该写进 edit tool"**：这会让工具和语言服务耦合；hook 才是合适位置。
4. **"control-plane prompt 可以展示完整 args"**：不安全，可能泄露 token/secret-bearing URL。
5. **"MCP tool 等于本地工具"**：MCP 还有远端协议、通知、认证、隐私边界。

### 练习题

- **Q1**：给 `AgentTool` 新增一个 `format_file` 工具，需要实现哪些 trait/字段？
- **Q2**：为什么 `args_hash` 比完整 args 更适合作为 confirmation binding？
- **Q3**：如果 LSP 启动很慢，after_tool_call 应该阻塞多久？
- **Q4**：MCP tool 返回超长结果时，应该在哪一层截断？
- **Q5**：如何测试一个工具在 permission denied 后不会执行副作用？

### 代码附录

| 文件 | 讲解重点 |
|------|----------|
| `agent/src/types.rs` | Tool trait / hooks / permission |
| `agent/src/agent_loop.rs` | Tool execution loop |
| `coding-agent/src/tools/edit.rs` | 文件编辑 |
| `coding-agent/src/tools/bash.rs` | shell 执行 |
| `coding-agent/src/tools/mcp_adapter.rs` | MCP tool adapter |
| `coding-agent/src/lsp_supervisor.rs` | LSP 进程管理 |
| `coding-agent/src/lsp.rs` | LSP protocol |

---

## Chapter 05 — Triggers / Cron / Loops / Inbox：让 agent 主动工作（20 min）

> 核心源码：`crates/agent/src/harness/trigger.rs`, `crates/agent/src/harness/trigger_runtime.rs`, `crates/agent/src/harness/agent_harness.rs`, `crates/coding-agent/src/triggers/cron.rs`, `crates/coding-agent/src/triggers/dynamic.rs`, `crates/coding-agent/src/inbox.rs`, `docs/loops.md`
> 对应材料：`manual/04-automation-loops-triggers.md`, `manual/deep-read/01-trigger-state-machine.md`, `manual/deep-read/02-loop-inbox-internals.md`, `manual/deep-read/07-automation-security-audit.md`

### 讲解目标

1. 解释 pie 和普通 coding agent 最大的不同：它可以通过 trigger/cron/loop 主动工作。
2. 画出 `Trigger` envelope 的字段和生命周期。
3. 追踪 `handle_trigger` 从 receive 到 accepted/running/completed/failed 的状态机。
4. 讲清楚 stateful loop 的纯文本协议：`<loop-state>` 和 `<inbox>`。
5. 解释 inbox 为什么是 triage 面，而不是 chat log。
6. 识别自动化安全风险：sub-agent 权限继承、dedup 内存态、feedback loop、tag injection。

### 问题背景

用户不想每次主动问 agent：

- "每半小时帮我查一下 CI 有没有红。"
- "当 MCP server 通知资源变更，帮我总结一下。"
- "每天早上检查 issue/PR，把值得我看的放到 inbox。"
- "当某个本地文件出现时，运行一个检查。"

这些都要求 agent 从 **响应式聊天** 变成 **事件驱动 runtime**。

### 为什么难

自动化不是把 `cron` 和 `prompt` 拼起来这么简单。必须解决：

- 去重：同一个事件不能无限执行。
- 循环抑制：agent 触发 agent 不能炸。
- 审计：每个自动执行要有可追踪记录。
- 权限：不同来源的 trigger 应有不同能力。
- 结果路由：有些结果进父对话，有些进 inbox，有些只写审计。
- 状态：loop 需要上次运行的记忆。

### 核心抽象

`Trigger` envelope：

```text
Trigger {
  source,
  source_kind,
  idempotency_key,
  replacement_policy,
  trace_id,
  payload_summary,
  payload_visibility,
  authority,
  received_at
}
```

状态机：

```text
Received
  │
  ├─ dedup hit ───────────────▶ Deduped
  ├─ cycle limit ─────────────▶ CycleSuppressed
  └─ accepted by runtime
        │
        ├─ permission denied ─▶ PermissionDenied
        ├─ needs approval ───▶ NeedsApproval
        └─ Accepted
              │
              ▼
            Running
              │
              ├─ success ────▶ Completed
              └─ error ──────▶ Failed
```

Stateful loop：

```text
Cron due
  → Trigger
  → Sub-agent prompt includes previous <loop-state>
  → model output includes:
      <loop-state>new persistent state</loop-state>
      <inbox>finding for human triage</inbox>
  → listener extracts tags
  → writes loop-state sidecar
  → appends inbox JSONL
```

### 源码走读路线

1. `harness/trigger.rs`：`Trigger`, `TriggerState`, `ReplacementPolicy`, `TriggerRecord`。
2. `harness/trigger_runtime.rs`：`evaluate()` 去重/循环抑制。
3. `harness/agent_harness.rs`：`handle_trigger`, `spawn_trigger_action`, `run_trigger_action`, `apply_promotion`。
4. `coding-agent/src/triggers/cron.rs`：cron registry、stateful prompt、listener tag extraction。
5. `coding-agent/src/triggers/dynamic.rs`：自然语言 trigger rule。
6. `coding-agent/src/inbox.rs`：inbox append/claim/dismiss/clear。
7. `docs/loops.md`：产品语义。

### 演示建议

1. **写一个 stateful loop prompt**（4 min）：展示 `<loop-state>` 和 `<inbox>`。
2. **画 trigger runtime evaluate 流程**（4 min）：先 dedup，再 cycle，再 accept。
3. **演示 inbox triage**（3 min）：`/inbox`, `/inbox claim`, `/inbox dismiss`。
4. **安全讨论**（5 min）：MCP malicious notification 触发全权限 sub-agent，会发生什么？

### 容易误解点

1. **"Cron job 直接运行 prompt"**：实际先变成 Trigger，再经过 runtime 和 harness。
2. **"Loop state 是模型上下文自动保存"**：不是，依赖 `<loop-state>` tag 提取。
3. **"Inbox 是普通聊天消息"**：不是，它是 triage queue，和主会话分离。
4. **"Dedup 是持久化的"**：当前 TriggerRuntime 是内存态，重启会丢。
5. **"Sub-agent 是安全隔离的"**：会话隔离是有的，但工具权限目前继承父代理，存在风险。

### 练习题

- **Q1**：设计一个每日 PR review loop，它的 `<loop-state>` 应保存什么？
- **Q2**：`ReplacementPolicy::LatestReplaces` 和 `Drop` 在 MCP notification 中分别适合什么事件？
- **Q3**：为什么去重检查要在循环抑制检查之前？
- **Q4**：如果模型输出两个 `<inbox>` tag，系统应该如何处理？
- **Q5**：为 sub-agent 增加工具白名单，应改哪些类型和 hook？

### 代码附录

| 文件 | 讲解重点 |
|------|----------|
| `harness/trigger.rs` | Trigger envelope / states |
| `harness/trigger_runtime.rs` | dedup/cycle |
| `harness/agent_harness.rs` | handle/spawn/run/apply promotion |
| `coding-agent/src/triggers/cron.rs` | cron + loop state |
| `coding-agent/src/triggers/dynamic.rs` | dynamic rules |
| `coding-agent/src/inbox.rs` | triage queue |
| `docs/loops.md` | 产品语义 |

---

## Chapter 06 — MCP / Notification / Web Relay：外部工具和远程 UI 表面（14 min）

> 核心源码：`crates/mcp/src/`, `crates/coding-agent/src/mcp_loader.rs`, `crates/coding-agent/src/tools/mcp_adapter.rs`, `crates/coding-agent/src/triggers/mcp_notification_hook.rs`, `workers/fefe-hub/`, `docs/endpoints.md`, `docs/web-ui-parity.md`
> 对应材料：`manual/05-mcp-and-fefe-hub.md`, `manual/06-roadmap-docs-issues.md`

### 讲解目标

1. 解释 MCP 在 pie 中的双重角色：外部工具调用 + 外部事件通知。
2. 区分 MCP stdio transport 和 Streamable HTTP transport。
3. 追踪 `mcp_loader` 如何把 server config 转成 `McpAgentTool`。
4. 说明 MCP notification 如何进入 TriggerRuntime。
5. 解释 `fefe-hub` 的当前状态：跨 agent 服务已移除，Web Relay 是保留网络表面。

### 问题背景

本地 coding agent 不可能内置所有工具。MCP 提供了协议化扩展：

- 本地 stdio server，如 weather/notify examples。
- HTTP MCP server，如远程数据服务。
- Server push notification，如资源变更、tool list change。

同时，pie 需要远程观看和控制本地 session，这就是 Web Relay 的角色。

### 为什么难

- MCP 是 JSON-RPC，tool call 和 notification 共用连接。
- stdio 需要管理 child process 生命周期。
- HTTP/SSE 需要 reconnect、auth、stream parse。
- Notification payload 可能含敏感信息，不能直接落入 chat。
- Web Relay 涉及远程 viewer、capability URL、control-plane approval。

### 核心抽象

```text
MCP tool path:
  mcp.toml
    → mcp_loader
    → McpClient
    → tools/list
    → McpAgentTool
    → AgentLoop tool execution

MCP notification path:
  McpClient read pump
    → McpNotificationHook
    → Trigger envelope
    → TriggerRuntime
    → InjectSummary / SubAgent / InjectAndRun

Web relay path:
  local session snapshot
    → /web-connect
    → Cloudflare Durable Object relay
    → browser viewer / remote prompt / approval
```

### 源码走读路线

1. `crates/mcp/src/protocol.rs`：JSON-RPC 和 MCP types。
2. `crates/mcp/src/transport.rs`：transport trait。
3. `crates/mcp/src/stdio.rs`：child process transport。
4. `crates/mcp/src/http.rs`：HTTP/SSE transport。
5. `crates/mcp/src/client.rs`：read pump、inflight、cancel。
6. `crates/coding-agent/src/mcp_loader.rs`：配置加载。
7. `tools/mcp_adapter.rs`：MCP tool to AgentTool。
8. `triggers/mcp_notification_hook.rs`：notification to Trigger。
9. `workers/fefe-hub/src/relay.ts`：Web Relay Durable Object。

### 演示建议

1. **运行 MCP weather example**（3 min）：展示 stdio tool。
2. **画 MCP tool call 时序**（3 min）：tools/list → tools/call。
3. **画 notification 时序**（3 min）：notification → trigger。
4. **解释 fefe de-scope**（2 min）：为什么跨-agent hub 移除，只保留 web relay。

### 容易误解点

1. **"MCP 只是工具协议"**：在 pie 中，它同时也是 notification source。
2. **"Notification payload 可以直接进 chat"**：不行，隐私边界要求摘要/脱敏。
3. **"fefe-hub 是跨 agent 消息服务"**：旧路径已 410 Gone，当前保留的是 web relay。
4. **"HTTP MCP 和 stdio MCP 只是 URL 不同"**：transport、auth、reconnect、SSE 都不同。

### 练习题

- **Q1**：新增一个 MCP server 后，哪一步把它的 tool 暴露给 AgentLoop？
- **Q2**：MCP notification 缺少 dedup key 时应该怎样处理？
- **Q3**：为什么 unknown/custom notification 不应保存 raw params？
- **Q4**：Web Relay 和 MCP HTTP server 的职责有什么区别？

### 代码附录

| 文件 | 讲解重点 |
|------|----------|
| `crates/mcp/src/client.rs` | MCP client loop |
| `crates/mcp/src/stdio.rs` | child process transport |
| `crates/mcp/src/http.rs` | Streamable HTTP |
| `coding-agent/src/mcp_loader.rs` | server config |
| `tools/mcp_adapter.rs` | AgentTool adapter |
| `triggers/mcp_notification_hook.rs` | notification source |
| `workers/fefe-hub/src/relay.ts` | web relay |

---

## Chapter 07 — Goal / OnTurnEndHook：自动继续、停止条件和 Evaluator（12 min）

> 核心源码：`crates/coding-agent/src/goal.rs`, `crates/agent/src/harness/agent_harness.rs`, `crates/agent/src/types.rs`, `crates/coding-agent/src/commands.rs`
> 对应材料：`manual/deep-read/06-goal-evaluator-internals.md`

### 讲解目标

1. 解释 `/goal` 解决的产品问题：不是一次回复，而是围绕目标持续迭代。
2. 区分 AgentLoop 内部 stop condition 和 Harness 层 `OnTurnEndHook`。
3. 追踪 evaluator 子代理如何读取 bounded transcript 并判断目标是否完成。
4. 说明 continuation cap 的双层保护。
5. 识别 false positive/false negative 风险。

### 问题背景

用户常常不是问一个问题，而是设一个目标：

```text
/goal Make this repo pass tests and explain the fix.
```

这要求 agent 在每一轮结束后判断：

- 目标是否完成？
- 如果没完成，是否应该继续？
- 如果继续，下一轮 prompt 是什么？
- 如果判断错误，会不会无限循环或过早停止？

### 为什么难

目标完成判断是模糊语义问题：

- LLM 自己说 "done" 不一定真的 done。
- Tool output 可能很长，不能完整塞给 evaluator。
- Evaluator 本身不能再调用工具，否则会引入递归和副作用。
- 继续太多轮会烧 token，继续太少会半途而废。

### 核心抽象

```text
AgentLoop finishes one turn
  │
  ▼
OnTurnEndHook
  │
  ├─ build bounded transcript
  ├─ run evaluator Agent (no tools)
  ├─ parse evaluator output
  └─ TurnEndDecision:
       Continue(next_prompt)
       Stop(reason)
       Pause
       Noop
```

### 源码走读路线

1. `crates/coding-agent/src/goal.rs`：goal prompt、状态、命令语义。
2. `crates/coding-agent/src/commands.rs`：`/goal` 命令入口。
3. `crates/agent/src/types.rs`：`OnTurnEndHook`, `TurnEndDecision`。
4. `crates/agent/src/harness/agent_harness.rs`：`run_evaluator` 和 continuation cap。

### 演示建议

1. **对比普通 prompt vs `/goal`**（3 min）：普通 turn 停止后就结束，goal 会 evaluate。
2. **画 evaluator 隔离**（3 min）：no tools、bounded transcript、result parse。
3. **讨论误判**（3 min）：目标 "make tests pass" 应该看 tool output 还是 assistant text？

### 容易误解点

1. **"模型最后一句说 done 就完成"**：不可靠，需要 evaluator。
2. **"evaluator 可以再查文件"**：当前设计应是无工具评估，避免副作用。
3. **"OnTurnEndHook 是 provider 层逻辑"**：它在 harness 层，因为它需要 session/turn context。
4. **"cap 只是防成本"**：也是防无限 loop 和坏 prompt。

### 练习题

- **Q1**：为 `/goal fix tests` 设计一个 evaluator rubric。
- **Q2**：bounded transcript 应保留哪些内容，丢弃哪些内容？
- **Q3**：什么情况应该 `Pause` 而不是 `Continue`？
- **Q4**：如何测试 false positive？

### 代码附录

| 文件 | 讲解重点 |
|------|----------|
| `coding-agent/src/goal.rs` | goal 状态和 prompt |
| `coding-agent/src/commands.rs` | slash command |
| `agent/src/types.rs` | `OnTurnEndHook` |
| `agent_harness.rs` | evaluator loop |

---

## Chapter 08 — 风险与测试路线：Provider 一致性、Session 完整性、自动化安全（18 min）

> 核心材料：`manual/deep-read/00-final-deep-read-guide.md`, `manual/deep-read/07-automation-security-audit.md`, `manual/deep-read/08-session-integrity-review.md`, `manual/deep-read/09-provider-conformance-test-plan.md`

### 讲解目标

1. 把前七章学到的架构知识转成可执行的风险和测试路线。
2. 理解 Top 5 风险为什么严重，分别会造成什么用户可见问题。
3. 设计三组后续工作 DAG：自动化安全、session durability、provider conformance。
4. 解释为什么这些风险不能只靠人工 code review 发现，必须写测试/fixture。

### 风险 Top 5

| 风险 | 严重度 | 影响 | 优先行动 |
|------|--------|------|----------|
| 子代理工具白名单缺失 | P0 安全 | MCP/cron/dynamic trigger 可能以完整工具权限执行 | 为 TriggerAction 加 allowed_tools / capability profile |
| JSONL 截断无恢复 | P0 数据 | 崩溃半行导致 session 打不开 | load_entries 忽略/截断最后坏行并告警 |
| 跨 Provider tool call 一致性零覆盖 | P0 质量 | 某 provider tool call silently broken | 建 mock SSE/EventStream conformance |
| Compaction 自动触发/完整性不足 | P1 功能 | 长会话爆 context 或摘要路径错误 | 集成 context threshold 测试 |
| Deprecated promotion path 在线 | P1 安全 | free-form substring promotion 可绕过 | 接入结构化 result details builder |

### 后续 DAG 建议

#### DAG A：Automation Capability Sandbox

```text
root: trigger capability sandbox
  ├─ design capability profiles
  ├─ implement allowed_tools in TriggerAction
  ├─ update cron/dynamic/mcp default policy
  ├─ tests: malicious MCP cannot call bash/write
  └─ review: security regression matrix
```

验收：

- MCP custom notification 默认不能执行 Bash/FileWrite。
- Cron loop 可以使用 read/search，但敏感工具需显式授权。
- Promotion 只走结构化 condition。

#### DAG B：Session Durability Hardening

```text
root: session durability
  ├─ truncated last-line recovery
  ├─ header backup or repair mode
  ├─ sidecar manifest hash/check
  ├─ huge session streaming load plan
  └─ tests: corrupt JSONL / missing sidecar / branch compaction
```

验收：

- 最后一行截断不阻止打开 session。
- Header 损坏有明确错误和 recovery hint。
- Export/import 能检测 sidecar 缺失。

#### DAG C：Provider Conformance Suite

```text
root: provider conformance
  ├─ mock OpenAI Responses SSE
  ├─ mock Anthropic input_json_delta
  ├─ mock Google functionCall
  ├─ mock Bedrock EventStream
  ├─ partial JSON fuzz cases
  └─ review: shared expected AssistantMessageEvent snapshots
```

验收：

- 每个 provider 的 tool call matrix 有 fixture。
- 并行 tool call / unicode / nested object / malformed delta 都有测试。
- Usage/cache/reasoning 缺失值行为明确。

### 教学收官话术

pie 的技术路线可以总结成一句话：

> pie 把 coding agent 拆成可持久化的 runtime：Provider 只负责把模型事件标准化，AgentCore 负责生命周期和状态机，CodingAgent 负责工具与产品面，Trigger/Loop 把 agent 从被动聊天推进到长期自动化。

但一旦 agent 开始长期自动化，系统就进入了更严格的工程要求：

- 权限必须显式；
- 状态必须可恢复；
- provider 行为必须可测试；
- 自动化结果必须可审计；
- 用户看到的不是模型自称成功，而是 runtime ledger 和可重复验证的事实。

### 练习题

- **Q1**：把 "子代理工具白名单" 拆成 4 个可并行实现/测试节点。
- **Q2**：设计一个 JSONL 截断恢复测试：输入文件、预期行为、错误提示。
- **Q3**：为 OpenAI Responses 并行 tool call 写 expected `AssistantMessageEvent` 序列。
- **Q4**：如果 trigger dedup 从内存改成持久化，需要引入哪些表/文件和 migration？
- **Q5**：为什么 "最终回复看起来正确" 不是 provider conformance 的验收标准？

### 代码附录

| 风险方向 | 文件入口 | 推荐测试 |
|----------|----------|----------|
| Trigger capability | `agent_harness.rs`, `trigger.rs` | malicious MCP / cron loop |
| JSONL recovery | `jsonl_storage.rs` | corrupt last line / header |
| Provider conformance | `providers/*`, `utils/json_parse.rs` | mock SSE/EventStream |
| Compaction integrity | `compaction.rs`, `session.rs` | branch + compaction |
| Promotion condition | `agent_harness.rs`, `dynamic.rs` | structured details |

---

## 附录 A：课程材料索引

| 类型 | 路径 | 用途 |
|------|------|------|
| 粗读总览 | `manual/00-overview.md` | 第一遍了解整体架构 |
| 精读总纲 | `manual/deep-read/00-final-deep-read-guide.md` | 风险和阅读路线 |
| Trigger | `manual/deep-read/01-trigger-state-machine.md` | Ch05 深讲 |
| Loop/Inbox | `manual/deep-read/02-loop-inbox-internals.md` | Ch05 深讲 |
| Session | `manual/deep-read/03-session-branch-model.md` | Ch03 深讲 |
| LSP | `manual/deep-read/04-lsp-integration-report.md` | Ch04 深讲 |
| Provider Tool Call | `manual/deep-read/05-tool-call-parsing-matrix.md` | Ch02 深讲 |
| Goal | `manual/deep-read/06-goal-evaluator-internals.md` | Ch07 深讲 |
| Automation Security | `manual/deep-read/07-automation-security-audit.md` | Ch08 风险 |
| Session Integrity | `manual/deep-read/08-session-integrity-review.md` | Ch08 风险 |
| Provider Test Plan | `manual/deep-read/09-provider-conformance-test-plan.md` | Ch08 测试 |

## 附录 B：推荐讲师准备清单

1. 准备一个小型 Rust 项目，用 pie 跑一次普通 coding turn。
2. 准备一个 session JSONL 样例，手动标注 parent/leaf。
3. 准备一个 fake provider event stream，展示 partial JSON。
4. 准备一个 stateful loop prompt，包含 `<loop-state>` 和 `<inbox>`。
5. 准备一张风险 Top 5 表，用于课程最后讨论。
6. 如果现场时间足够，演示一个 MCP stdio server 工具接入。

## 附录 C：下一轮可直接开 Rive Workflow 的任务

1. `pie.trigger-capability-sandbox`：实现 trigger/sub-agent 工具白名单和默认 capability profile。
2. `pie.session-durability-hardening`：实现 JSONL 截断恢复、header recovery hint、sidecar manifest check。
3. `pie.provider-conformance-suite`：实现 OpenAI/Anthropic/Google/Bedrock mock streaming fixtures。
4. `pie.loop-inbox-hardening`：加 inbox file lock、duplicate finding detection、tag parser robustness tests。
5. `pie.lsp-diagnostics-smoke`：补 LSP supervisor 的真实语言诊断 smoke 和 timeout 策略。

