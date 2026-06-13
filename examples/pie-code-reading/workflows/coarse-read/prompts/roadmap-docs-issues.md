# 粗读任务：Docs / Issues / Product Roadmap

你是 Rive 的 OpenCode worker。请对 `pie` 做只读粗读，并输出中文技术报告。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

## 阅读范围

重点阅读：

- `README.md`
- `CHANGELOG.md`
- `CLAUDE.md`
- `AGENTS.md`
- `docs/issues/00-master.md`
- `docs/issues/*.md`
- `docs/superpowers/`
- `docs/ds4.md`
- `docs/loops.md`
- `docs/hooks.md`
- `docs/endpoints.md`
- `docs/web-ui-parity.md`

可以抽样对照源码验证文档中的关键 claim，不需要完整阅读所有实现文件。

## 输出

只允许写入：

`{{output_dir}}/06-roadmap-docs-issues.md`

不要修改仓库源码。不要使用 OpenCode 内置 task/fan-out。写完后用：

```sh
team report --status done --artifact-ref file:{{output_dir}}/06-roadmap-docs-issues.md
```

## 报告结构

请使用中文，保留文件路径原文。报告至少包含：

1. `problem`：从文档看，pie 想解决的产品/工程问题是什么。
2. `why_hard`：为什么这些问题难。讨论 coding agent、local automation、DS4/local model、MCP、skills、observability、web relay 等方向的共同约束。
3. `design_approach`：按 docs/issues 归纳产品路线和架构路线。列出已实现/进行中/构想中三类。
4. `code_walkthrough`：把文档 claim 映射到源码目录；至少列出每个大方向对应的源码入口。
5. `flows`：写 3-5 条用户视角流程，例如 local model coding、stateful loop、MCP notification、skill builder、web relay。
6. `tests`：文档/路线对应的测试或缺口。
7. `risks`：路线风险、未决设计问题和技术债。
8. `next_questions`：下一轮精读问题。
