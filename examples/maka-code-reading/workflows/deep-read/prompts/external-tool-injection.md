# Maka 精读任务：Rive / Office / Explore External Tool Injection

你是 Rive 的 OpenCode deep-read worker。只读代码，输出中文 Markdown，不要修改 Maka 源码。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 必读源码

- `{{repo_path}}/apps/desktop/src/main/rive-cli.ts`
- `{{repo_path}}/apps/desktop/src/main/rive-workflow-tool.ts`
- `{{repo_path}}/apps/desktop/src/main/office-document-tool.ts`
- `{{repo_path}}/apps/desktop/src/main/explore-agent-tool.ts`
- `{{repo_path}}/apps/desktop/src/main/officecli-env.ts`
- `{{repo_path}}/apps/desktop/src/main/officecli-probe.ts`
- relevant tests

## 输出

只允许写入 `{{output_dir}}/09-external-tool-injection.md`。

写完后运行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/09-external-tool-injection.md
```

## 报告要求

必须包含：

- `scope`
- `problem`
- `source_evidence`
- `tool_matrix`：RiveWorkflow/Rive CLI/OfficeDocument/ExploreAgent 的 action、side-effect level、permissionRequired、argv construction、cwd/env、timeout/abort cleanup。
- `injection_risks`：参数注入、binary path injection、cwd escape、symlink, output truncation, stale child process。
- `cleanup_policy`：spawn/abort/timeout/reaping strategy。
- `next_actions`

质量要求：每个外部工具都要明确“读/写/执行/网络/多 agent orchestration”副作用。
