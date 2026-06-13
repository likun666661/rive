# Maka 精读任务：Memory 9-Gate Runtime Enforcement

你是 Rive 的 OpenCode deep-read worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 必读源码/文档

- `{{repo_path}}/packages/core/src/memory.ts`
- `{{repo_path}}/packages/core/src/local-memory.ts`
- `{{repo_path}}/apps/desktop/src/main/local-memory-service.ts`
- `{{repo_path}}/docs/memory-threat-model.md`
- `{{repo_path}}/packages/core/src/__tests__/memory.test.ts`
- `{{repo_path}}/apps/desktop/src/main/__tests__/local-memory-service.test.ts`
- renderer memory/settings UI tests if relevant

## 输出

只允许写入 `{{output_dir}}/06-memory-gates.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/06-memory-gates.md
```

## 报告要求

必须包含：

- `scope`
- `problem`
- `source_evidence`
- `gate_matrix`：逐条列 G1-G9 或文档中的门禁规则，映射到 contract、runtime implementation、test。
- `enforcement_gaps`：哪些门只有类型/文档，没有 runtime enforcement；哪些门有测试缺口。
- `tests`
- `next_actions`

质量要求：每条门都要给源码文件和函数名；如果文档和代码命名不一致，要指出。
