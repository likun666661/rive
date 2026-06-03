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

## 8. Ledger

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

## 9. Instantiation Semantics

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

## 10. Capability Policy

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

## 11. Output Contract

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

## 12. Template from Successful Run

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

## 13. Implementation Slices

### Slice A: Template File and Validation

- Add YAML parser.
- Validate params/nodes/edges.
- Validate DAG acyclic.
- Validate node ids and output contract shape.
- Add `rive workflow validate`.

### Slice B: Registry and Versioning

- Add template registry ledger.
- Add immutable template versions by hash.
- Add `workflow.template.registered` event.
- Add `rive workflow register/list/show`.

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
- Export a draft YAML from an accepted DAG.
- Keep it human-editable; do not auto-register without review.

## 14. Acceptance Criteria

Phase 13 达到以下条件时才算有用：

1. A workflow template can be validated and registered.
2. A template run records exact template version/hash and params hash.
3. Running the same template with the same command id is idempotent.
4. Running the same template with different params creates a separate run.
5. Instantiated Work DAG is inspectable before execution.
6. Scheduler can execute a workflow run with existing OpenCode workers.
7. Workflow success is derived from root Work DAG projection and output contracts.
8. Trace/usage remain debug read models only.
9. A Sentinel-style workflow can be expressed as a template.
10. A successful ad hoc DAG can be exported as a draft reusable template.

## 15. Product Boundary

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
