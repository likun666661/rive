# ADK 教学细纲 Section Writer: Chapter 02 State Lifecycle

写中文教学细纲，不改源码。

## 必读

- `{{rive_repo}}/examples/eino-technical-manual/manual/teaching-manual-outline.md`
- `{{rive_repo}}/examples/google-adk-go-code-reading/manual/deep-read/02-state-lifecycle-deep-dive.md`
- `{{replica_repo}}/README.md`
- `{{replica_repo}}/session/`
- `{{replica_repo}}/memory/`
- `{{replica_repo}}/artifact/`
- `{{replica_repo}}/context/`
- `{{replica_repo}}/runner/`
- `{{replica_repo}}/cmd/demo/main.go`

## 输出

写入 `{{output_dir}}/02-state-lifecycle-section.md`。结构：`teaching_goal`、`core_story`、`code_walkthrough`、`demos`、`pitfalls`、`exercises`、`code_appendix`。

重点讲清楚：session/memory/artifact 为什么不能混成一个 store，app/user/session/temp scope 如何路由，event actions 如何影响 durable state，artifact 版本和 memory search 如何与 runner/context 协作。要给课堂演示和练习题。

写完后捕获 snapshot 并 report：
```sh
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE" --label "adk-teaching-ch02-state-lifecycle" --dispatch "$RIVE_DISPATCH_ID" | python3 -c 'import json,sys; print(json.load(sys.stdin)["protocol"]["snapshot_id"])')
team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "adk-teaching-ch02-report-$(date +%s)" --artifact-ref "file:{{output_dir}}/02-state-lifecycle-section.md" --stdin < "{{output_dir}}/02-state-lifecycle-section.md"
```
