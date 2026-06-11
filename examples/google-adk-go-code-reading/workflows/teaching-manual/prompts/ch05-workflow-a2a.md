# ADK 教学细纲 Section Writer: Chapter 05 Workflow / AgentTool / Remote A2A

写中文教学细纲，不改源码。

## 必读

- `{{rive_repo}}/examples/eino-technical-manual/manual/teaching-manual-outline.md`
- `{{rive_repo}}/examples/google-adk-go-code-reading/manual/deep-read/05-workflow-a2a-deep-dive.md`
- `{{replica_repo}}/README.md`
- `{{replica_repo}}/workflow/`
- `{{replica_repo}}/tool/agenttool/`
- `{{replica_repo}}/agent/remoteagent/`
- `{{replica_repo}}/runner/`
- `{{replica_repo}}/cmd/demo/main.go`

## 输出

写入 `{{output_dir}}/05-workflow-a2a-section.md`。结构：`teaching_goal`、`core_story`、`code_walkthrough`、`demos`、`pitfalls`、`exercises`、`code_appendix`。

重点讲清楚：Sequential/Parallel/Loop 是 agent-as-composition，不是外部调度器；AgentTool 是把 Agent 放进 Tool 调用边界；Remote A2A 是把外部流转换成本地 event；branch label、child session isolation、partial aggregation 的教学价值和简化边界。

写完后捕获 snapshot 并 report：
```sh
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE" --label "adk-teaching-ch05-workflow-a2a" --dispatch "$RIVE_DISPATCH_ID" | python3 -c 'import json,sys; print(json.load(sys.stdin)["protocol"]["snapshot_id"])')
team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "adk-teaching-ch05-report-$(date +%s)" --artifact-ref "file:{{output_dir}}/05-workflow-a2a-section.md" --stdin < "{{output_dir}}/05-workflow-a2a-section.md"
```
