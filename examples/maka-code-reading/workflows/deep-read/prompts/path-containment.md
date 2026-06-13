# Maka 精读任务：Path Containment

你是 Rive 的 OpenCode deep-read worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 必读源码

- `{{repo_path}}/apps/desktop/src/main/main.ts`
- `{{repo_path}}/apps/desktop/src/main/workspace-instructions.ts`
- `{{repo_path}}/apps/desktop/src/main/office-document-tool.ts`
- `{{repo_path}}/apps/desktop/src/main/explore-agent-tool.ts`
- `{{repo_path}}/apps/desktop/src/main/local-memory-service.ts`
- `{{repo_path}}/apps/desktop/src/main/open-path-guard.ts`
- `{{repo_path}}/packages/storage/src/artifact-store.ts`
- relevant tests

## 输出

只允许写入 `{{output_dir}}/03-path-containment.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/03-path-containment.md
```

## 报告要求

必须包含：

- `scope`
- `problem`
- `source_evidence`
- `containment_matrix`：每个 `isInside` / `isInsideOrSamePath` / path resolver 的逻辑、是否 realpath、是否处理 symlink、是否允许 equal root、是否拒绝 absolute/..。
- `bypass_scenarios`：至少 3 个潜在绕过或平台差异场景；如果不可行，说明原因。
- `tests`：已有测试与建议测试，覆盖 macOS `/private/var`、Windows drive/prefix、symlink、case-insensitive FS。
- `next_actions`

质量要求：避免臆造漏洞；每个绕过场景都要标清“confirmed by code / hypothesis needing test”。
