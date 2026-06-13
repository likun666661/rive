# 综合审查：Session 完整性审查

你是 Rive 的 OpenCode review worker。请优先读取上游产物，必要时少量回查源码，输出中文审查报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 输出目录：`{{output_dir}}`

## 上游产物

- `{{output_dir}}/03-session-branch-model.md`

## 输出

只允许写入 `{{output_dir}}/08-session-integrity-review.md`。不要修改源码，不要使用 OpenCode 内置 task/fan-out。完成后执行：

```sh
team report --status done --artifact-ref file:{{output_dir}}/08-session-integrity-review.md
```

## 报告结构

1. `executive_summary`
2. `integrity_model`：session entry、parent DAG、leaf、sidecar、repo/storage 的完整性模型。
3. `compaction_resume`：compaction/resume 在分支场景下的正确性审查。
4. `export_import`：`.piesession` export/import 和 sidecar 文件风险。
5. `risks`：数据损坏/丢失/无限增长/legacy 兼容风险。
6. `recommendations`：优先级排序的修复/测试建议。
