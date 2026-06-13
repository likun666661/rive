# Maka 精读任务：Credential / Settings Security

你是 Rive 的 OpenCode deep-read worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 必读源码

- `{{repo_path}}/apps/desktop/src/main/credential-store.ts`
- `{{repo_path}}/apps/desktop/src/main/settings-ipc-helpers.ts`
- `{{repo_path}}/apps/desktop/src/main/main.ts`
- `{{repo_path}}/packages/storage/src/settings-store.ts`
- `{{repo_path}}/packages/storage/src/connection-store.ts`
- `{{repo_path}}/packages/core/src/settings.ts`
- `{{repo_path}}/packages/core/src/llm-connections.ts`
- OAuth subscription services under `apps/desktop/src/main/oauth/` if present
- relevant tests

## 输出

只允许写入 `{{output_dir}}/04-credential-settings-security.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/04-credential-settings-security.md
```

## 报告要求

必须包含：

- `scope`
- `problem`
- `source_evidence`
- `lifecycle`：API key/OAuth token/bot token/proxy password 从输入、保存、读取、使用、展示、删除的完整生命周期。
- `risk_matrix`：明文 settings、renderer 内存、IPC 返回、日志/trace、safeStorage unavailable、atomic write failure。
- `migration_plan`：哪些字段应迁移到 credential-store，兼容旧 settings 的方案。
- `tests`

质量要求：不要输出任何真实 secret；报告只讨论字段名和代码路径。
