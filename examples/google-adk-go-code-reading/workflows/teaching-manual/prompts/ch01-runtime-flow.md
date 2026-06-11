# ADK 教学细纲 Section Writer: Chapter 01 Runtime Flow

你是 Rive DAG 中的章节 writer。目标是写一份可被 final collector 直接合成教学手册的中文章节细纲，不要写源码改动。

## 输入

- Rive repo: `{{rive_repo}}`
- Replica repo: `{{replica_repo}}`
- Output dir: `{{output_dir}}`

必须阅读：
- `{{rive_repo}}/examples/eino-technical-manual/manual/teaching-manual-outline.md`，学习目标质量和结构密度。
- `{{rive_repo}}/examples/google-adk-go-code-reading/manual/deep-read/01-runtime-flow-deep-dive.md`
- `{{replica_repo}}/README.md`
- `{{replica_repo}}/runner/`
- `{{replica_repo}}/agent/`
- `{{replica_repo}}/llmagent/`
- `{{replica_repo}}/flow/`
- `{{replica_repo}}/model/`
- `{{replica_repo}}/event/`
- `{{replica_repo}}/session/`
- `{{replica_repo}}/cmd/demo/main.go`

## 输出

只写入：`{{output_dir}}/01-runtime-flow-section.md`

内容必须像教学博客细纲，不是摘要。请覆盖：
- `teaching_goal`: 这一章学完能解释/能写什么。
- `core_story`: Runner -> Agent -> Flow -> Model/Tool -> Event -> Session 的问题背景、为什么难、ADK 的解决思路、replica 怎么落地。
- `code_walkthrough`: 具体文件、关键类型/函数、推荐阅读顺序。
- `demos`: 课堂演示脚本、预期输出、讲解话术。
- `pitfalls`: 易错点和边界。
- `exercises`: 练习题，含答案要点。
- `code_appendix`: 表格列出文件、核心符号、测试文件。

写完后执行：
```sh
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE" --label "adk-teaching-ch01-runtime-flow" --dispatch "$RIVE_DISPATCH_ID" | python3 -c 'import json,sys; print(json.load(sys.stdin)["protocol"]["snapshot_id"])')
team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "adk-teaching-ch01-report-$(date +%s)" --artifact-ref "file:{{output_dir}}/01-runtime-flow-section.md" --stdin < "{{output_dir}}/01-runtime-flow-section.md"
```
