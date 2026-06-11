# ADK 教学细纲 Section Writer: Chapter 03 Tool System

写中文教学细纲，不改源码。

## 必读

- `{{rive_repo}}/examples/eino-technical-manual/manual/teaching-manual-outline.md`
- `{{rive_repo}}/examples/google-adk-go-code-reading/manual/deep-read/03-tool-system-deep-dive.md`
- `{{replica_repo}}/README.md`
- `{{replica_repo}}/tool/`
- `{{replica_repo}}/flow/`
- `{{replica_repo}}/context/`
- `{{replica_repo}}/event/`
- `{{replica_repo}}/cmd/demo/main.go`

## 输出

写入 `{{output_dir}}/03-tool-system-section.md`。结构：`teaching_goal`、`core_story`、`code_walkthrough`、`demos`、`pitfalls`、`exercises`、`code_appendix`。

重点讲清楚：declaration 和 runtime execution 为什么分离，Tool/Toolset/filtering/confirmation/streaming/long-running 分别解决什么问题，FunctionCall -> ToolContext -> FunctionResponse 的生命周期，Replica 与 ADK Go 的简化边界。要有可教学的场景和练习。

写完后捕获 snapshot 并 report：
```sh
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE" --label "adk-teaching-ch03-tool-system" --dispatch "$RIVE_DISPATCH_ID" | python3 -c 'import json,sys; print(json.load(sys.stdin)["protocol"]["snapshot_id"])')
team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "adk-teaching-ch03-report-$(date +%s)" --artifact-ref "file:{{output_dir}}/03-tool-system-section.md" --stdin < "{{output_dir}}/03-tool-system-section.md"
```
