# Maka 精读任务：IPC Surface Security

你是 Rive 的 OpenCode deep-read worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 必读源码

- `{{repo_path}}/apps/desktop/src/main/main.ts`
- `{{repo_path}}/apps/desktop/src/preload/preload.ts`
- `{{repo_path}}/apps/desktop/src/main/settings-ipc-helpers.ts`
- `{{repo_path}}/apps/desktop/src/main/open-path-guard.ts`
- `{{repo_path}}/apps/desktop/src/main/credential-store.ts`
- `{{repo_path}}/apps/desktop/src/main/__tests__/`

## 输出

只允许写入 `{{output_dir}}/02-ipc-surface-security.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/02-ipc-surface-security.md
```

## 报告要求

必须包含：

- `scope`
- `problem`
- `source_evidence`
- `flow_analysis`
- `handler_matrix`：抽取 main/preload 的 IPC surface，至少列 top 30 high-signal handlers；标注输入校验、文件/网络/credential/spawn 副作用、返回是否脱敏。
- `risk_matrix`：列 top-10 需要加固的 handler，说明风险和源码证据。
- `next_actions`

质量要求：不要泛泛说 IPC 很危险。必须指出具体 handler、具体参数、具体校验方式和建议测试。
