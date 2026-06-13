# 综合审查：Provider 一致性测试计划

你是 Rive 的 OpenCode review worker。请优先读取上游产物，必要时少量回查源码，输出中文测试计划。

## 输入

- 仓库路径：`{{repo_path}}`
- 输出目录：`{{output_dir}}`

## 上游产物

- `{{output_dir}}/05-tool-call-parsing-matrix.md`

## 输出

只允许写入 `{{output_dir}}/09-provider-conformance-test-plan.md`。不要修改源码，不要使用 OpenCode 内置 task/fan-out。完成后执行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/09-provider-conformance-test-plan.md
```

## 报告结构

1. `executive_summary`
2. `conformance_matrix`：provider × tool call/input_json_delta/reasoning/usage/cache/abort/retry。
3. `mock_server_plan`：如何构造 provider fixture 或 mock HTTP/SSE/EventStream server。
4. `test_cases`：具体测试 case，按优先级排序。
5. `risks`：测试不稳定性、真实网络依赖、provider schema 漂移。
6. `recommendations`：下一步落地建议。
