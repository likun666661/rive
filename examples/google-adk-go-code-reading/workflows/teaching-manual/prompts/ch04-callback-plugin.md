# ADK 教学细纲 Section Writer: Chapter 04 Callback / Plugin / Instruction

写中文教学细纲，不改源码。

## 必读

- `{{rive_repo}}/examples/eino-technical-manual/manual/teaching-manual-outline.md`
- `{{rive_repo}}/examples/google-adk-go-code-reading/manual/deep-read/04-callback-plugin-deep-dive.md`
- `{{replica_repo}}/README.md`
- `{{replica_repo}}/callbackctx/`
- `{{replica_repo}}/plugin/`
- `{{replica_repo}}/instruction/`
- `{{replica_repo}}/flow/`
- `{{replica_repo}}/runner/`
- `{{replica_repo}}/cmd/demo/main.go`

## 输出

写入 `{{output_dir}}/04-callback-plugin-section.md`。结构：`teaching_goal`、`core_story`、`code_walkthrough`、`demos`、`pitfalls`、`exercises`、`code_appendix`。

重点讲清楚：callback/plugin 是横切控制面，不是业务节点；before/after model/tool/run/event 的短路语义；instruction provider 和 state interpolation；plugin ordering；怎样用 logging/cache/state mutation 做课堂演示。

写完后捕获 snapshot 并 report：
```sh
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE" --label "adk-teaching-ch04-callback-plugin" --dispatch "$RIVE_DISPATCH_ID" | python3 -c 'import json,sys; print(json.load(sys.stdin)["protocol"]["snapshot_id"])')
team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "adk-teaching-ch04-report-$(date +%s)" --artifact-ref "file:{{output_dir}}/04-callback-plugin-section.md" --stdin < "{{output_dir}}/04-callback-plugin-section.md"
```
