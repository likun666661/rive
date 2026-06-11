# ADK 教学细纲 Section Writer: Chapter 07 Agent Flow / ReAct / Multi-Agent

写中文教学细纲，不改源码。

## 必读

- `{{rive_repo}}/examples/eino-technical-manual/manual/teaching-manual-outline.md`
- `{{rive_repo}}/examples/google-adk-go-code-reading/manual/deep-read/07-agent-flow-react-multi-agent-deep-dive.md`
- `{{replica_repo}}/README.md`
- `{{replica_repo}}/agent/`
- `{{replica_repo}}/tool/transfer/`
- `{{replica_repo}}/tool/exitloop/`
- `{{replica_repo}}/plugin/retryreflect/`
- `{{replica_repo}}/plugin/functionmodifier/`
- `{{replica_repo}}/agent/agentconfig/`
- `{{replica_repo}}/flow/`
- `{{replica_repo}}/runner/`
- `{{replica_repo}}/cmd/demo/main.go`

## 输出

写入 `{{output_dir}}/07-agent-flow-section.md`。结构：`teaching_goal`、`core_story`、`code_walkthrough`、`demos`、`pitfalls`、`exercises`、`code_appendix`。

重点讲清楚：ReAct 不是魔法，是 model/tool/event loop；transfer_to_agent 如何把控制权交给子 agent；runner 如何从历史事件恢复 active agent；ExitLoop、Retry/Reflect、Hidden Args 分别解决什么生产问题；JSON configurable agent tree 如何把前面章节串起来。

写完后捕获 snapshot 并 report：
```sh
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE" --label "adk-teaching-ch07-agent-flow" --dispatch "$RIVE_DISPATCH_ID" | python3 -c 'import json,sys; print(json.load(sys.stdin)["protocol"]["snapshot_id"])')
team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "adk-teaching-ch07-report-$(date +%s)" --artifact-ref "file:{{output_dir}}/07-agent-flow-section.md" --stdin < "{{output_dir}}/07-agent-flow-section.md"
```
