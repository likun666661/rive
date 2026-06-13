# 粗读任务：MCP / fefe-hub Integration

你是 Rive 的 OpenCode worker。请对 `pie` 做只读粗读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `crates/mcp/src/`
- `crates/mcp/tests/`
- `crates/coding-agent/src/mcp_loader.rs`
- `crates/coding-agent/src/tools/mcp_adapter.rs`
- `examples/mcp-weather-python/`
- `examples/mcp-notify-python/`
- `docs/issues/08-mcp-client.md`
- `docs/issues/18-rfc-fefe-mcp-hub.md`
- `docs/issues/19-fefe-client-onboard.md`
- `docs/issues/22-web-relay.md`
- `docs/endpoints.md`
- `docs/web-ui-parity.md`
- `workers/fefe-hub/`

## 输出

只允许写入：

`{{output_dir}}/05-mcp-and-fefe-hub.md`

不要修改仓库源码。不要使用 OpenCode 内置 task/fan-out。写完后用：

```sh
team report --status done --artifact-ref file:{{output_dir}}/05-mcp-and-fefe-hub.md
```

## 报告结构

请使用中文，保留 Rust/TypeScript 标识符和文件路径原文。报告至少包含：

1. `problem`：MCP 与 fefe hub 要解决什么问题。解释本地工具协议、HTTP/stdio transport、remote hub、relay 和 endpoint 管理的角色。
2. `why_hard`：为什么难。讨论协议边界、streaming/HTTP、认证、Cloudflare Worker/D1、notification/privacy、客户端 onboarding。
3. `design_approach`：设计思路。画出 `pie MCP loader/client -> MCP server/tool -> notification` 和 `fefe hub -> endpoint/relay` 两条流程。
4. `code_walkthrough`：关键 Rust/TypeScript 文件、类型、函数。
5. `flows`：stdio MCP call、HTTP MCP call、notification hook、fefe auth/endpoint/relay 各简述。
6. `tests`：列相关测试和测试意图。
7. `risks`：安全、运维、协议兼容风险。
8. `next_questions`：下一轮精读问题。
