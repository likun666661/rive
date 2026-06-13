# 粗读任务：pie-ai Provider / Model / Streaming

你是 Rive 的 OpenCode worker。请对 `pie` 做只读粗读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `crates/ai/src/lib.rs`
- `crates/ai/src/types.rs`
- `crates/ai/src/stream.rs`
- `crates/ai/src/event_stream.rs`
- `crates/ai/src/models.rs`
- `crates/ai/src/models_generated.rs`
- `crates/ai/src/api_registry.rs`
- `crates/ai/src/env_api_keys.rs`
- `crates/ai/src/providers/`
- `crates/ai/src/providers/openai_responses*.rs`
- `crates/ai/src/providers/anthropic.rs`
- `crates/ai/src/providers/google*.rs`
- `crates/ai/src/providers/amazon_bedrock.rs`
- `crates/ai/src/providers/cloudflare.rs`
- `crates/ai/src/providers/simple_options.rs`
- `crates/ai/src/utils/`
- `docs/ds4.md`

可以按需阅读相关 tests/examples。

## 输出

只允许写入：

`{{output_dir}}/01-ai-provider-streaming.md`

不要修改仓库源码。不要使用 OpenCode 内置 task/fan-out。写完后用：

```sh
team report --status done --artifact-ref file:{{output_dir}}/01-ai-provider-streaming.md
```

## 报告结构

请使用中文，保留 Rust 标识符和文件路径原文。报告至少包含这些章节：

1. `problem`：这一层要解决什么问题。解释 pie 如何把不同模型供应商、Responses/Chat Completions、streaming、reasoning、usage、tool call 统一成可用接口。
2. `why_hard`：为什么难。重点讨论 provider 差异、stream event 标准化、OAuth/API key、DS4 prefix cache、retry/overflow、JSON 解析和 token/cost accounting。
3. `design_approach`：pie-ai 的解决思路。画出 `CLI/Agent -> pie-ai -> Provider -> Stream/Event -> AgentLoop` 的文本流程图。
4. `code_walkthrough`：源码走读。列关键文件、类型、函数，并说明它们在链路里的位置。
5. `flows`：写 2-3 条典型流程：OpenAI Responses streaming、Anthropic/Google provider、DS4/local OpenAI-compatible path。
6. `tests`：列支撑判断的测试/示例文件和测试意图。
7. `risks`：未读风险、TODO、可能的边界 bug。
8. `next_questions`：下一轮精读应继续追问的 8-12 个具体问题。

## 质量要求

- 不要泛泛总结；每个判断都尽量绑定源码路径。
- 要解释“为什么这样做”，不是只列出“有什么文件”。
- 对不确定点请明确标注，不要编造。
