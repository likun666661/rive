# ADK 教学细纲 Section Writer: Chapter 06 Entrypoint / Deploy / Telemetry

写中文教学细纲，不改源码。

## 必读

- `{{rive_repo}}/examples/eino-technical-manual/manual/teaching-manual-outline.md`
- `{{rive_repo}}/examples/google-adk-go-code-reading/manual/deep-read/06-entrypoint-deploy-deep-dive.md`
- `{{replica_repo}}/README.md`
- `{{replica_repo}}/cmd/launcher/`
- `{{replica_repo}}/server/`
- `{{replica_repo}}/deploy/`
- `{{replica_repo}}/telemetry/`
- `{{replica_repo}}/cmd/demo/main.go`

## 输出

写入 `{{output_dir}}/06-entrypoint-deploy-section.md`。结构：`teaching_goal`、`core_story`、`code_walkthrough`、`demos`、`pitfalls`、`exercises`、`code_appendix`。

重点讲清楚：launcher config 如何把 agent runtime 和 I/O transport 解耦；console/web/server 如何复用同一 config；dry-run deploy plan 为什么是教学替代；telemetry span/log 模型如何围绕 runner/model/server 记录。

写完后捕获 snapshot 并 report：
```sh
SNAPSHOT_ID=$(rive snapshot capture --path "$RIVE_WORKSPACE" --label "adk-teaching-ch06-entrypoint-deploy" --dispatch "$RIVE_DISPATCH_ID" | python3 -c 'import json,sys; print(json.load(sys.stdin)["protocol"]["snapshot_id"])')
team report --dispatch "$RIVE_DISPATCH_ID" --status done --snapshot "$SNAPSHOT_ID" --command-id "adk-teaching-ch06-report-$(date +%s)" --artifact-ref "file:{{output_dir}}/06-entrypoint-deploy-section.md" --stdin < "{{output_dir}}/06-entrypoint-deploy-section.md"
```
