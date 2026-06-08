# 汇总任务：ADK Go 精读技术手册

你是 Rive 的 final judge worker。你不需要重新通读整个仓库；你的主要输入是前六个精读节点产物。

## 输入

- 仓库路径：`{{repo_path}}`
- 阅读基线：`{{source_ref}}`
- 输出目录：`{{output_dir}}`
- 深度档位：`{{depth}}`

必须读取这些文件：

- `{{output_dir}}/01-runtime-flow-deep-dive.md`
- `{{output_dir}}/02-state-lifecycle-deep-dive.md`
- `{{output_dir}}/03-tool-system-deep-dive.md`
- `{{output_dir}}/04-callback-plugin-deep-dive.md`
- `{{output_dir}}/05-workflow-a2a-deep-dive.md`
- `{{output_dir}}/06-entrypoint-deploy-deep-dive.md`

可以按需回看粗读总纲：

- `examples/google-adk-go-code-reading/manual/00-overview.md`

## 输出

只允许写入：

`{{output_dir}}/00-final-architecture-guide.md`

不要修改仓库源码。写完后捕获 snapshot，并用
`team report --status done --artifact-ref file:{{output_dir}}/00-final-architecture-guide.md`
报告。

## 报告结构

请使用中文，保留 Go 标识符和文件路径原文。最终手册至少包含：

1. `executive_summary`：ADK Go 的架构一句话、三句话、十句话版本。
2. `architecture_map`：总体架构图，说明 Runner、Agent、Flow、Model、Tool、State、Plugin、Workflow、Server 的关系。
3. `deep_read_index`：六个精读章节的索引，每章包括：
   - 解决的问题；
   - 为什么难；
   - 核心设计；
   - 关键文件；
   - 最值得继续读的点。
4. `cross_module_flows`：至少四条跨模块链路：
   - 用户请求到 LLM event；
   - tool call 到 function response；
   - session state / artifact / memory 生命周期；
   - callback/plugin 改写 model/tool 行为；
   - workflow agent / A2A 编排。
5. `maintainer_questions`：如果要维护或复刻 ADK Go，必须先回答的关键问题清单。
6. `next_dag`：下一轮 Rive DAG 建议，要写出节点名、依赖关系、每个节点目标和产物。

## 质量要求

- 不要做空泛摘要，要把六份精读报告中的具体源码路径和结论整合起来。
- 如果六份报告之间有冲突或空白，要明确列出。
- 这个手册应该能指导下一位 engineer 开始维护或仿写 ADK Go 的核心架构。
