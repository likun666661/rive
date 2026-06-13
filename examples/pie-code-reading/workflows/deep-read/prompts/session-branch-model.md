# 精读任务：JSONL Session 分支模型

你是 Rive 的 OpenCode worker。请对 `pie` 做只读源码级精读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

- `crates/agent/src/harness/session/session.rs`
- `crates/agent/src/harness/session/jsonl_repo.rs`
- `crates/agent/src/harness/session/jsonl_storage.rs`
- `crates/agent/src/harness/session/repo_utils.rs`
- `crates/agent/src/harness/compaction/`
- `crates/coding-agent/src/session/`
- `crates/coding-agent/src/session_archive.rs`
- session / compaction / export tests

## 输出

只允许写入 `{{output_dir}}/03-session-branch-model.md`。不要修改源码，不要使用 OpenCode 内置 task/fan-out。完成后执行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/03-session-branch-model.md
```

## 报告结构

1. `problem`：resumable session、undo、fork、compaction、export/import 要解决什么状态问题。
2. `why_hard`：append-only JSONL、parent DAG、多 leaf、compaction first-kept、sidecar 文件为什么复杂。
3. `design_approach`：pie 的 session model。
4. `code_walkthrough`：关键类型/函数走读。
5. `branch_examples`：给出 2-3 个 JSONL entry 示例，解释 parent_id、branch path、leaf。
6. `compaction_integrity`：compaction/resume/export 在分支场景中的数据完整性。
7. `tests`：测试文件和覆盖意图。
8. `risks`：数据损坏、无限增长、legacy 兼容、sidecar 缺失风险。
9. `next_questions`：下一轮问题。
