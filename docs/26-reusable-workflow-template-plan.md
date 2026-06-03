# Phase 13: Reusable Workflow Template Plan

Phase 13 的目标，是把 Rive 从“每次临时编排一个 Work DAG”推进到“成功的多智能体协作流程可以沉淀成可重复消费的 workflow”。

用户和 agent 第一次可以一起把一个问题拆成 DAG；当这套 DAG 被证明有效后，Rive 应该能把它保存成模板。下一次用户只需要一行命令和少量参数，就能再次实例化同一套多智能体流程。

```text
first run:
  user prompt + agent planning
    -> Work DAG
    -> workers / evidence / refs / review
    -> accepted outcome
    -> promote to workflow template

repeat run:
  rive workflow run <template> --param ...
    -> instantiate Work DAG
    -> scheduler runs workers
    -> same ledger / projection / review rules
```

## 1. Problem

Rive 已经具备这些能力：

1. Work DAG 表达任务结构和依赖。
2. Scheduler 可以并发调度 ready worker。
3. OpenCode workers 可以真实执行任务。
4. Git worktree backend 可以隔离 worker patch。
5. Trace / usage / evidence / fact / branch ref 都能入账。

但当前 DAG 仍然偏一次性：

- 用户每次都要重新解释目标。
- agent 每次都要重新拆图。
- 好用的协作模式不能被复用。
- 已经验证过的 node prompt、acceptance criteria、capability policy 没有稳定版本。
- 后续 rerun 无法明确回答“这次是按哪一版流程跑的”。

这会削弱 Rive 的实战价值。只让 agent 临时多开几个 worker，和“烧更多 token 让单个 agent 自己试”差距不够大。Rive 真正有价值的地方，是把人和 agent 共同摸出来的协作路径变成可复用、可审计、可恢复、可优化的 workflow runtime。

## 2. Goal

Phase 13 要定义并实现最小 reusable workflow contract：

```text
Workflow Template
  reusable DAG schema
  parameter schema
  node templates
  edge templates
  runner / capability policy
  output contract
  acceptance policy

Workflow Run
  template version + params
  instantiated Work DAG root
  scheduler run / worker dispatches
  facts / evidence / refs / trace
  final projection and report
```

用户最终体验应该类似：

```sh
rive workflow run sentinel.prod-debug \
  --service jagent \
  --env prd \
  --since 1h \
  --max-findings 3
```

或者某个垂直封装 CLI：

```sh
sentinel-rive run --service jagent --since 1h
```

这条命令不是简单 prompt wrapper。它会实例化一套有版本、有边界、有验收规则的 Rive Work DAG，然后交给 scheduler / workers / ref integration / review projection 执行。

## 3. Non-goals

Phase 13 不做：

- 通用自动 planner。
- 动态无限循环 graph。
- 模板市场。
- 远程 worker。
- 复杂 UI。
- 从 stdout / final answer / trace 判断 workflow 成功。
- 把模板写成任意脚本执行器。

本阶段重点是 template/run 协议和最小 CLI。模板可以先是本地文件和本地 registry，不急着做云端分发。

## 4. Mental Model

需要把几个概念分清：

```text
workflow_template
  可复用的语义 DAG schema。它不是一次真实执行。

workflow_version
  template 的不可变版本。每次修改模板都生成新 version/hash。

workflow_run
  用一组参数实例化出来的一次真实运行。

node_template
  一个可复用节点配方：目标、输入、输出、工具边界、runner、验收规则。

run_node
  实例化后的真实 Work DAG node，绑定 dispatch/fact/evidence/ref。
```

一个 workflow run 必须能回答：

- 使用了哪个 `template_id`、`template_version`、`template_hash`。
- 使用了哪些参数，参数 hash 是什么。
- 实例化出了哪个 root work node。
- 每个 node template 映射到哪个 work node。
- 哪些 dispatch / evidence / workspace ref 支撑结果。
- 最终成功来自哪个 Work DAG projection，而不是来自自然语言输出。

## 5. Template Shape

MVP 可以使用 YAML 或 JSON。建议先用 YAML 作为人可读 source，runtime 读入后写 registry ledger。

示例：

```yaml
api_version: rive.workflow/v0
id: sentinel.prod-debug
version: 1
title: "Production debug workflow"

params:
  service:
    type: string
    required: true
  env:
    type: enum
    values: ["prd", "stg"]
    default: "prd"
  since:
    type: duration
    default: "1h"
  max_findings:
    type: integer
    default: 3

defaults:
  runner: opencode
  workspace_mode: shared
  acceptance_mode: manual
  max_parallel: 3

nodes:
  p0_alerts:
    kind: task
    title: "Scan P0 alerts for {{service}}"
    body: |
      Use Sentinel to inspect P0 alerts for {{service}} in {{env}} since {{since}}.
      Return confirmed alerts, timestamps, affected panels, and exact commands.
    capability_policy:
      allow:
        - "sentinel alerts"
    output_contract:
      type: alert_scan
      required_fields: ["summary", "evidence_refs", "confirmed_findings"]

  p1_errors:
    kind: task
    title: "Scan service errors for {{service}}"
    body: |
      Use Sentinel error queries for {{service}} in {{env}} since {{since}}.
      Summarize high-signal errors and attach evidence.
    capability_policy:
      allow:
        - "sentinel errors"
        - "sentinel logs"
    output_contract:
      type: error_scan

  code_pivot:
    kind: task
    title: "Map production findings to code paths"
    body: |
      Read prior alert/error findings and inspect code for likely causes.
      Do not claim causality without linking log evidence to code evidence.
    output_contract:
      type: code_pivot

  judge:
    kind: review
    title: "Judge incident hypothesis and next action"
    body: |
      Compare alert, error, and code-pivot findings.
      Produce confirmed / likely / inconclusive findings and recommended next action.
    output_contract:
      type: incident_judgement

edges:
  - type: decomposes_to
    from: root
    to: p0_alerts
  - type: decomposes_to
    from: root
    to: p1_errors
  - type: decomposes_to
    from: root
    to: code_pivot
  - type: decomposes_to
    from: root
    to: judge
  - type: depends_on
    from: code_pivot
    to: p0_alerts
  - type: depends_on
    from: code_pivot
    to: p1_errors
  - type: depends_on
    from: judge
    to: code_pivot
```

MVP 不需要模板语言很强。先支持：

- string / enum / integer / duration 参数；
- `{{param}}` 简单渲染；
- static nodes / edges；
- bounded fanout 以后再加。

## 6. Sentinel as First Template

Sentinel 是第一个强候选，因为它天然是重复 workflow：

```text
production bug / incident question
  -> alert scan
  -> error/log scan
  -> golden signal / latency scan
  -> code pivot
  -> online recheck
  -> judge report
```

它比普通 coding task 更能体现 Rive 的价值：

- 线上观测和代码分析可以并行。
- 每个方向都有独立 evidence。
- 最后需要一个 judge 节点裁决，而不是每个 worker 各说各话。
- 同一套流程会被反复用于不同 service/env/time window。
- capability policy 很重要：某些节点只能查监控，某些节点只能读代码，judge 节点只能读前序事实，不能自己编造证据。

Sentinel workflow 的 v0 DAG 可以是：

```text
root: Debug {{service}} in {{env}} since {{since}}
  decomposes_to p0_alert_scan
  decomposes_to p1_error_scan
  decomposes_to golden_signal_scan
  decomposes_to code_pivot
  decomposes_to online_recheck
  decomposes_to final_judge

code_pivot depends_on p0_alert_scan
code_pivot depends_on p1_error_scan
online_recheck depends_on code_pivot
final_judge depends_on p0_alert_scan
final_judge depends_on p1_error_scan
final_judge depends_on golden_signal_scan
final_judge depends_on online_recheck
```

每个 node template 应该固定四个问题：

1. 面临的问题是什么。
2. 为什么这是问题。
3. 解决/调查思路是什么。
4. 当前 evidence 支持什么结论，缺什么证据。

## 7. CLI Plan

新增 commands：

```sh
rive workflow validate <path>
rive workflow import <path> --command-id <id>
rive workflow register <path> --command-id <id>
rive workflow list
rive workflow show <template_id> [--version <n>]

rive workflow run <template_id> \
  --param key=value \
  --command-id <id> \
  [--max-parallel <n>] \
  [--acceptance-mode manual|auto-reported|auto-committed]

rive workflow status <workflow_run_id>
rive workflow report <workflow_run_id>
rive workflow export <workflow_run_id>
```

对于常用模板，可以再做一个薄的垂直 CLI，但它仍然走同一条 Rive protocol path：

```sh
sentinel-rive run --service jagent --since 1h
```

这个垂直 CLI 不能绕过 Rive。它只负责准备参数，然后调用 `rive workflow run`。

## 8. Workflow Import

@kun-li 提出的关键简化是对的：一个可重复消费 workflow 的最小可重建资产，就是两类东西：

```text
1. workflow DAG
   node ids / node kinds / edges / runner policy / acceptance policy

2. node prompts
   每个 node 的 prompt template / input params / upstream output usage / output contract
```

只保存这两个，理论上就能重建 workflow。但为了让导入后的 workflow 可重复、可审计、不会漂，需要把 prompt 挂到稳定的 `node_template_id` 上，并显式声明它依赖哪些参数和前序节点输出。

### 8.1 Import Package

支持两种导入形态。

单文件：

```text
workflows/sentinel-prod-debug.yaml
```

目录包：

```text
workflows/sentinel-prod-debug/
  workflow.yaml
  prompts/
    global-signal-scan.md
    investigate-service.md
    final-judge-and-slack.md
```

单文件适合小 workflow；目录包适合 prompt 很长、需要版本审阅的真实生产 workflow。

### 8.2 Import Schema

导入文件需要把 graph 和 prompts 绑定起来：

```yaml
api_version: rive.workflow/v0
id: sentinel.prod-debug
version: 1
title: "Sentinel production debug workflow"

params:
  env:
    type: enum
    values: ["prd", "stg"]
    default: "prd"
  since:
    type: duration
    default: "1h"
  slack_channel:
    type: string
    required: true
  allow_github_write:
    type: boolean
    default: false
  allow_slack_post:
    type: boolean
    default: false

nodes:
  global_signal_scan:
    kind: task
    runner: opencode
    prompt:
      inline: |
        查 P0 alerts、golden signals、latency、error rate。
        输出 incident window、受影响服务、证据链接和不确定性。
    output_contract:
      type: global_signal_scan
      required_sections: ["signals", "evidence", "gaps"]

  investigate_alva_backend:
    kind: task
    runner: opencode
    prompt:
      file: prompts/investigate-service.md
      params:
        service: "alva-backend"
    capability_policy:
      allow:
        - "sentinel errors"
        - "sentinel logs"
        - "code.read"
        - "github.read"
      gated_allow:
        github.issue.create: "{{allow_github_write}}"
    output_contract:
      type: service_investigation
      required_sections: ["errors", "logs", "code_pivot", "issue_draft", "evidence", "gaps"]

  final_judge_and_slack:
    kind: review
    runner: opencode
    prompt:
      file: prompts/final-judge-and-slack.md
    consumes:
      - global_signal_scan
      - investigate_alva_backend
      - investigate_alfs
      - investigate_jagent
      - investigate_alva_gateway
    capability_policy:
      allow:
        - "slack.post"
      gated_allow:
        slack.post: "{{allow_slack_post}}"
    output_contract:
      type: incident_slack_report

edges:
  - type: decomposes_to
    from: root
    to: global_signal_scan
  - type: decomposes_to
    from: root
    to: investigate_alva_backend
  - type: decomposes_to
    from: root
    to: investigate_alfs
  - type: decomposes_to
    from: root
    to: investigate_jagent
  - type: decomposes_to
    from: root
    to: investigate_alva_gateway
  - type: decomposes_to
    from: root
    to: final_judge_and_slack
  - type: depends_on
    from: final_judge_and_slack
    to: global_signal_scan
  - type: depends_on
    from: final_judge_and_slack
    to: investigate_alva_backend
```

导入时，Rive 应该把所有 prompt 文件内容纳入 `template_hash`。不能只 hash `workflow.yaml`，否则 prompt 改了但版本看起来没变。

### 8.3 Import Validation

`rive workflow import` 做这些检查：

1. `api_version` 支持。
2. `id`、`version`、node id 稳定且合法。
3. 所有 prompt `file` 都存在。
4. 所有 `edges.from/to` 都指向存在的 node 或 `root`。
5. graph 是 DAG。
6. `consumes` 只能引用 predecessor 或 reachable dependency。
7. prompt declared params 都能从 workflow params 或 node-local params 解析。
8. output contract 不为空。
9. gated capability 的 gate 必须来自 boolean param。
10. template hash 覆盖 workflow spec 和 prompt bytes。

导入成功后写入 template registry。它不会立刻创建 Work DAG，也不会启动 runner：

```text
workflow package
  -> validate
  -> normalize
  -> hash spec + prompts
  -> workflow.template.registered
  -> immutable workflow_template_version
```

### 8.4 Import vs Run

这条边界要写硬：

```text
workflow import/register = 保存可复用模板
workflow run             = 用参数实例化真实 Work DAG
```

导入不会产生 dispatch、fact、snapshot、branch ref、trace。运行才会产生这些 execution / evidence / debug record。

### 8.5 Export Back to Package

为了支持“先和 agent 临时编排，跑通后沉淀模板”，需要反向导出：

```sh
rive workflow export-template --root <root_work_node_id> --output workflows/debug-template/
```

它应该生成：

```text
workflow.yaml
prompts/<node_template_id>.md
```

这一步生成的是 draft。人可以编辑、删减、抽参数，然后再 `rive workflow import`。

## 9. Ledger

Add workflow ledger/projection tables:

```text
workflow_templates
  template_id
  latest_version
  latest_hash
  title
  source_ref
  created_at
  updated_at

workflow_template_versions
  template_id
  version
  template_hash
  source_ref
  body_blob_ref
  created_at

workflow_runs
  workflow_run_id
  template_id
  template_version
  template_hash
  params_json
  params_hash
  root_work_node_id
  scheduler_run_id?
  state
  created_at
  completed_at?

workflow_run_nodes
  workflow_run_id
  node_template_id
  work_node_id
  output_contract_json
  capability_policy_json
```

Events:

```text
workflow.template.registered
workflow.run.created
workflow.run.instantiated
workflow.run.scheduler_started
workflow.run.completed
workflow.run.failed
```

Workflow state is a projection:

```text
workflow_run.state =
  root Work DAG projection
  + output contract status
  + scheduler state
```

它不能依赖 stdout、final answer 或 debug trace。

## 10. Instantiation Semantics

`rive workflow run` should:

1. Load template by id/version/hash.
2. Validate params against schema.
3. Render node titles/bodies.
4. Create root work node.
5. Create child work nodes.
6. Create edges.
7. Record `workflow_run_nodes` mapping.
8. Run scheduler if requested.
9. Return protocol fields for the run and root work projection.

关键不变量：

- `template_hash` is recorded on every run.
- Same `command_id` + same template + same params returns the same run.
- Same `command_id` + different template/params returns `idempotency_conflict`.
- Template update creates a new version; it does not mutate old runs.
- Workflow run id is stable enough to resume/status/report.

## 11. Capability Policy

Templates should not just store prompts. They should store what each node is allowed to do.

Example:

```yaml
capability_policy:
  mode: allowlist
  allow:
    - "sentinel alerts"
    - "sentinel errors"
  deny:
    - "git commit"
    - "rive work accept"
  max_iterations: 4
  max_runtime_seconds: 600
```

MVP can treat policy as prompt + audit metadata. Later it can become hard sandbox enforcement.

For Sentinel specifically:

- raw production queries should require discover/check/cost preflight;
- code-pivot nodes should cite both online evidence and code refs;
- judge nodes should not create new evidence except a synthesis report;
- secrets must come from local config/env, never template files.

## 12. Output Contract

Every reusable node needs a small output contract, otherwise reruns drift into prose.

MVP output contract can be simple:

```yaml
output_contract:
  type: code_investigation
  required_sections:
    - problem
    - why_it_matters
    - evidence
    - conclusion
    - gaps
  required_refs:
    evidence: true
    workspace_ref: false
```

Runtime can initially check only that required refs exist and that the worker reported a snapshot/ref. Later an LLM judge or schema validator can check section-level quality.

## 13. Template from Successful Run

The most interesting loop is:

```text
ad hoc successful DAG
  -> rive workflow propose-template --root <root>
  -> human edits template
  -> rive workflow register
  -> future one-line reruns
```

这意味着 Rive 可以从 dogfood 里学习，而不假装自己已经是自动 planner。模板仍然由人批准，但系统可以抽取：

- work node titles/bodies;
- edge topology;
- runner settings;
- acceptance mode;
- observed output refs;
- usage/cost profile;
- failure/retry history.

## 14. Implementation Slices

### Slice A: Template File and Validation

- Add YAML parser.
- Validate params/nodes/edges.
- Validate DAG acyclic.
- Validate node ids and output contract shape.
- Add `rive workflow validate`.
- Support both single-file import and `workflow.yaml + prompts/*.md` package import.
- Hash workflow spec plus prompt bytes into immutable `template_hash`.

### Slice B: Registry and Versioning

- Add template registry ledger.
- Add immutable template versions by hash.
- Add `workflow.template.registered` event.
- Add `rive workflow import/register/list/show`.

### Slice C: Instantiation to Work DAG

- Add `rive workflow run --no-scheduler`.
- Instantiate root, nodes, edges, and mappings.
- Add idempotency by template hash + params hash.
- Add `workflow.run.created` / `workflow.run.instantiated`.

### Slice D: Scheduler Integration

- Add `rive workflow run` with scheduler options.
- Store scheduler_run_id on workflow run.
- Add `rive workflow status/report`.
- Use existing Work DAG projection for success.

### Slice E: Sentinel Template

- Add `examples/workflows/sentinel-prod-debug.yaml`.
- Add optional `sentinel-rive` thin wrapper or documented shell entry.
- Run fake/smoke if production credentials are unavailable.

### Slice F: Template from Run

- Add `rive workflow propose-template --root <root>`.
- Add `rive workflow export-template --root <root> --output <dir>`.
- Export a draft YAML from an accepted DAG.
- Keep it human-editable; do not auto-register without review.

## 15. Acceptance Criteria

Phase 13 达到以下条件时才算有用：

1. A workflow template can be validated and registered.
2. A workflow package can import DAG plus prompt files into an immutable template version.
3. Changing any prompt changes `template_hash`.
4. Import does not create dispatch/fact/snapshot/trace side effects.
5. A template run records exact template version/hash and params hash.
6. Running the same template with the same command id is idempotent.
7. Running the same template with different params creates a separate run.
8. Instantiated Work DAG is inspectable before execution.
9. Scheduler can execute a workflow run with existing OpenCode workers.
10. Workflow success is derived from root Work DAG projection and output contracts.
11. Trace/usage remain debug read models only.
12. A Sentinel-style workflow can be expressed as a template.
13. A successful ad hoc DAG can be exported as a draft reusable template package.

## 16. Product Boundary

这个能力应该给用户这样的感觉：

```text
I once taught Rive how I debug a production service.
Now I can rerun that debug workflow for any service/time window with one command.
```

它不应该给用户这样的感觉：

```text
I pasted the same giant prompt again and hope the model remembers the process.
```

这就是 Rive 和直接使用 Codex / Claude Code 的产品差异。真正沉淀下来的资产不是某一次回答，而是一套可复用、有版本、多智能体协作、由 ledger 支撑执行的操作流程。
