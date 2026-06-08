# 精读任务：CLI / Server / Deploy / Telemetry / Examples

你是 Rive 的 OpenCode worker。请对 `google/adk-go` 的入口、服务端、部署和可观测性做只读精读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `cmd/adkgo/**`
- `cmd/launcher/**`
- `cmd/internal/**`
- `server/adkrest/**`
- `server/adka2a/**`
- `server/agentengine/**`
- `telemetry/**`
- `internal/telemetry/**`
- `util/aiplatform/**`
- `util/vertexai/**`
- `internal/cli/**`
- `internal/version/**`
- `examples/**`
- `.github/workflows/**` 和 `scripts/**` 中与开发/验证/部署相关的部分

## 输出

只允许写入：

`{{output_dir}}/06-entrypoint-deploy-deep-dive.md`

不要修改仓库源码。写完后捕获 snapshot，并用
`team report --status done --artifact-ref file:{{output_dir}}/06-entrypoint-deploy-deep-dive.md`
报告。

## 报告结构

请使用中文，保留 Go 标识符和文件路径原文。报告至少包含：

1. `problem`：ADK Go 如何从 library 变成 CLI、REST server、A2A server、Agent Engine、Cloud Run 示例和可观测系统。
2. `why_hard`：本地开发、Web UI、REST/SSE/WebSocket、A2A、GCP 部署、OTel exporter 为什么需要不同边界。
3. `design_approach`：解释 launcher、server handler、deployment util、telemetry init、examples taxonomy 的分层。
4. `code_walkthrough`：逐文件走读关键入口和 server/deploy 逻辑。
5. `entrypoint_map`：画出入口地图：
   - CLI；
   - launcher；
   - REST server；
   - A2A server；
   - Agent Engine；
   - telemetry；
   - examples。
6. `tests`：测试覆盖矩阵和缺口。
7. `risks`：部署权限、Secret/API key、OTel、server protocol、example drift、CI 覆盖等。
8. `next_questions`：下一轮应该继续追问的 8-12 个具体问题。

## 质量要求

- 不要把 examples 只当教程；要把它们作为产品能力矩阵来整理。
- 要明确 server 层和 runner/agent 层的边界。
- 对部署脚本/launcher 的用户体验风险要给出具体建议。
