# ADK 教学细纲 Final Collector

你是 final collector。目标是把 7 份章节 section 合成一份高质量中文教学手册细纲，对齐 Eino teaching manual outline 的密度、风格和可讲授性。

## 输入

- Rive repo: `{{rive_repo}}`
- Replica repo: `{{replica_repo}}`
- Output dir: `{{output_dir}}`
- Target file: `{{target_file}}`

必须阅读：
- `{{rive_repo}}/examples/eino-technical-manual/manual/teaching-manual-outline.md`
- `{{output_dir}}/01-runtime-flow-section.md`
- `{{output_dir}}/02-state-lifecycle-section.md`
- `{{output_dir}}/03-tool-system-section.md`
- `{{output_dir}}/04-callback-plugin-section.md`
- `{{output_dir}}/05-workflow-a2a-section.md`
- `{{output_dir}}/06-entrypoint-deploy-section.md`
- `{{output_dir}}/07-agent-flow-section.md`
- `{{replica_repo}}/README.md`

可以按需回看：
- `{{rive_repo}}/examples/google-adk-go-code-reading/manual/deep-read/*.md`

## 输出

写入 `{{target_file}}`，同时复制一份到 `{{output_dir}}/teaching-manual-outline.md`。

必须中文为主，保留 Go 标识符和路径原文。目标不是总结，而是一份能给 engineer 讲课/自学/复刻的细纲。请至少包含：

1. `# Google ADK Go 复刻版教学手册细纲`
2. `0. 教学路线图（90-120 分钟）`
   - 表格：段、章节、主题、建议时长、累积、转场逻辑
   - 30 分钟压缩版
   - 主线 thesis：Runner -> Agent -> Flow -> Tool/State -> Multi-Agent
3. 7 个章节，分别包含：
   - 讲解目标
   - 问题背景
   - 为什么难
   - 核心抽象
   - 复刻版代码走读
   - 演示建议
   - 容易误解点
   - 练习题
   - 代码附录表格
4. 跨章节总图：
   - Runtime chain
   - State/tool/plugin/control-plane chain
   - Multi-agent chain
5. 讲师提示：
   - 哪些地方是 replica 简化
   - 哪些地方对应 ADK Go 真代码
   - 哪些地方适合 live coding
6. 后续扩展 DAG 建议。

质量要求：
- 内容密度接近 Eino teaching outline。不要只列标题。
- 每章必须有具体代码路径和符号名。
- 每章必须解释 problem / why hard / design approach / replica implementation。
- 不要虚构未实现功能；不确定的地方写成 "replica 简化边界"。

写完后捕获 snapshot 并 report：
```sh
cp "{{target_file}}" "{{output_dir}}/teaching-manual-outline.md"
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE" --label "adk-teaching-final-outline" --dispatch "$RIVE_DISPATCH_ID" | python3 -c 'import json,sys; print(json.load(sys.stdin)["protocol"]["snapshot_id"])')
team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "adk-teaching-final-report-$(date +%s)" --artifact-ref "file:{{target_file}}" --artifact-ref "file:{{output_dir}}/teaching-manual-outline.md" --stdin < "{{target_file}}"
```
