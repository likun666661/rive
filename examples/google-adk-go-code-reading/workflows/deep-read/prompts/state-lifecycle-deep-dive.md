# 精读任务：Session / Memory / Artifact 状态生命周期

你是 Rive 的 OpenCode worker。请对 `google/adk-go` 的状态服务层做只读精读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `session/service.go`
- `session/session.go`
- `session/inmemory.go`
- `session/database/service.go`
- `session/database/session.go`
- `session/vertexai/*.go`
- `memory/service.go`
- `memory/inmemory.go`
- `memory/vertexai/*.go`
- `artifact/service.go`
- `artifact/inmemory.go`
- `artifact/gcsartifact/*.go`
- `internal/context/invocation_context.go`
- `internal/context/callback_context.go`
- `internal/sessionutils/utils.go`
- `internal/memory/memory.go`
- `internal/artifact/artifacts.go`

请同步阅读相关测试，尤其是 state delta、append event、artifact version、request validation。

## 输出

只允许写入：

`{{output_dir}}/02-state-lifecycle-deep-dive.md`

不要修改仓库源码。写完后捕获 snapshot，并用
`team report --status done --artifact-ref file:{{output_dir}}/02-state-lifecycle-deep-dive.md`
报告。

## 报告结构

请使用中文，保留 Go 标识符和文件路径原文。报告至少包含：

1. `problem`：Session、Memory、Artifact 分别解决什么状态问题，它们为什么不能混成一个存储。
2. `why_hard`：多轮会话、作用域状态、长期记忆、工具产物版本、云后端和并发写入为什么复杂。
3. `design_approach`：ADK Go 如何用 service interface、in-memory/database/Vertex/GCS 实现、context 注入来分层。
4. `code_walkthrough`：逐文件解释关键类型、方法、校验逻辑和数据结构。
5. `state_lifecycle`：用文本时序图描述：
   - 用户消息进入 session；
   - agent/tool 写 state delta；
   - artifact save/load/list/delete；
   - memory add/search；
   - state merge / temp state cleanup。
6. `tests`：列出测试文件覆盖了哪些状态语义，哪些语义仍缺测试。
7. `risks`：并发、版本号、后端一致性、重复逻辑、API 语义风险。
8. `next_questions`：下一轮应该继续追问的 8-12 个具体问题。

## 质量要求

- 把 state scope、event append、artifact version、memory search 的边界讲清楚。
- 注意区分 public API、internal helper、后端实现。
- 如果后端行为不一致，请明确列成表格。
