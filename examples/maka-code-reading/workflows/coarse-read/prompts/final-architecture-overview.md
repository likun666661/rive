# 汇总任务：Maka 粗读架构总览

你是 Rive 的 OpenCode final review worker。请优先消费前面 reader 节点的产物，必要时少量回查源码，输出一份中文架构总览。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 上游产物

请优先阅读这些文件：

- `{{output_dir}}/01-core-contracts.md`
- `{{output_dir}}/02-runtime-backends-tools.md`
- `{{output_dir}}/03-storage-persistence.md`
- `{{output_dir}}/04-desktop-main-ipc.md`
- `{{output_dir}}/05-renderer-ui.md`
- `{{output_dir}}/06-docs-tests-roadmap.md`

如果某个上游文件缺失，请明确写入缺失情况，不要编造。

## 输出

只允许写入：

`{{output_dir}}/00-overview.md`

不要修改仓库源码。不要使用 OpenCode 内置 task/fan-out。写完后用：

```sh
team report --status done --artifact-ref file:{{output_dir}}/00-overview.md
```

## 报告结构

请使用中文，保留 TypeScript/Electron 标识符和文件路径原文。报告至少包含：

1. `executive_summary`：用 8-12 条总结 Maka 是什么、架构主线是什么、最值得继续精读的部分是什么。
2. `architecture_map`：画出文本架构图，覆盖 renderer/preload/main、runtime、core contracts、storage、provider/backend、tools/permission、bots/telemetry、Rive workflow tool。
3. `deep_read_index`：列下一轮精读索引表：主题、建议阅读文件、为什么值得深读、预期产物。
4. `cross_module_flows`：至少写 6 条跨模块链路：app boot、send message/model stream、tool call permission、provider credential save/test、artifact materialization、settings update、Rive workflow tool、visual smoke/release check 可任选。
5. `risks`：从上游报告汇总主要工程风险/不确定点，按安全、可靠性、产品体验、测试覆盖分类。
6. `next_dag`：给出下一轮更细 DAG 建议，节点要聚焦、可并行、带验收产物。

## 质量要求

- final overview 不是拼贴，要解释跨模块关系。
- 不要让上下文被源码细节打爆；优先综合上游产物。
- 如果 reader 报告互相矛盾，要指出矛盾并给出需要人工复核的源码路径。
