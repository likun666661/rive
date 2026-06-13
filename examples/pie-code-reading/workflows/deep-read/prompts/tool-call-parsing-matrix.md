# 精读任务：跨 Provider Tool Call 解析一致性

你是 Rive 的 OpenCode worker。请对 `pie` 做只读源码级精读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

- `crates/ai/src/providers/anthropic.rs`
- `crates/ai/src/providers/openai_responses.rs`
- `crates/ai/src/providers/openai_completions.rs`
- `crates/ai/src/providers/google.rs`
- `crates/ai/src/providers/google_shared.rs`
- `crates/ai/src/providers/amazon_bedrock.rs`
- `crates/ai/src/providers/transform_messages.rs`
- `crates/ai/src/utils/json_parse.rs`
- `crates/ai/src/utils/sse.rs`
- `crates/ai/src/utils/aws_eventstream.rs`
- provider/model tests

## 输出

只允许写入 `{{output_dir}}/05-tool-call-parsing-matrix.md`。不要修改源码，不要使用 OpenCode 内置 task/fan-out。完成后执行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/05-tool-call-parsing-matrix.md
```

## 报告结构

1. `problem`：不同 provider tool call arguments 解析要解决什么问题。
2. `why_hard`：streaming delta、partial JSON、provider-specific event shape、tool id/name/schema 差异为什么难。
3. `design_approach`：pie 的统一 event/content 模型。
4. `code_walkthrough`：逐 provider 路径走读。
5. `provider_matrix`：Anthropic/OpenAI Responses/OpenAI Completions/Google/Bedrock 的输入事件、累积策略、完成时机、错误处理、输出事件对比表。
6. `fuzz_cases`：建议 fuzz/fixture case，特别是 partial JSON、unicode、嵌套对象、多个并行 tool call。
7. `tests`：已有测试和缺口。
8. `risks`：跨 provider 一致性风险。
9. `next_questions`：下一轮问题。
