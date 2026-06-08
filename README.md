# Rive

Rive 是本地优先 agent team runtime 的体系文档。

## 章节

1. [Agent 协作协议](./docs/01-agent-collaboration-protocol.md)
2. [Rust CLI 实现阶段与 AgentFS 策略](./docs/02-rust-cli-implementation-plan.md)
3. [Phase 1 Snapshot Evidence 测试计划](./docs/03-phase1-snapshot-evidence-test-plan.md)
4. [Phase 2 Fact Ledger 设计](./docs/04-phase2-fact-ledger-design.md)
5. [Phase 2 Fact Ledger 测试计划](./docs/05-phase2-fact-ledger-test-plan.md)
6. [Phase 3 Dispatch Ledger 设计](./docs/06-phase3-dispatch-ledger-design.md)
7. [Phase 3 Dispatch Ledger 测试计划](./docs/07-phase3-dispatch-ledger-test-plan.md)
8. [Phase 4 Agent CLI Debug Trace 设计](./docs/08-phase4-agent-cli-debug-trace-design.md)
9. [Phase 4 Debug Trace 测试计划](./docs/09-phase4-debug-trace-test-plan.md)
10. [Phase 5 OpenCode Runner 设计](./docs/10-phase5-opencode-runner-design.md)
11. [Phase 5 OpenCode Runner 测试计划](./docs/11-phase5-opencode-runner-test-plan.md)
12. [Phase 6 Codex Runner 设计](./docs/12-phase6-codex-runner-design.md)
13. [Phase 6 Codex Runner 测试计划](./docs/13-phase6-codex-runner-test-plan.md)
14. [Phase 7 Agent-to-Agent Delegation 设计](./docs/14-phase7-agent-to-agent-delegation-design.md)
15. [Phase 7 Agent-to-Agent Delegation 测试计划](./docs/15-phase7-agent-to-agent-delegation-test-plan.md)
16. [Phase 8 Work DAG / Dispatch Binding 设计](./docs/16-phase8-work-dag-dispatch-binding-design.md)
17. [Phase 8 Work DAG / Dispatch Binding 测试计划](./docs/17-phase8-work-dag-dispatch-binding-test-plan.md)
18. [Phase 9 OpenCode Orchestrator Control 设计](./docs/18-phase9-opencode-orchestrator-control-design.md)
19. [Phase 9 OpenCode Orchestrator Control 测试计划](./docs/19-phase9-opencode-orchestrator-control-test-plan.md)
20. [Phase 10 Orchestrator Sandbox / Graph Hygiene / Usage 设计](./docs/20-phase10-orchestrator-sandbox-graph-hygiene-usage-design.md)
21. [Phase 10 Orchestrator Sandbox / Graph Hygiene / Usage 测试计划](./docs/21-phase10-orchestrator-sandbox-graph-hygiene-usage-test-plan.md)
22. [Phase 11 Work DAG Scheduler 设计](./docs/22-phase11-work-dag-scheduler-design.md)
23. [Phase 11 Work DAG Scheduler 测试计划](./docs/23-phase11-work-dag-scheduler-test-plan.md)
24. [Phase 12 Git worktree Workspace Branch / Ref Integration 设计](./docs/24-phase12-worktree-ref-integration-design.md)
25. [Phase 12 Git worktree Workspace Branch / Ref Integration 测试计划](./docs/25-phase12-worktree-ref-integration-test-plan.md)
26. [Phase 13 Reusable Workflow Template 计划](./docs/26-reusable-workflow-template-plan.md)

## 当前能力快照

Rive 当前已经不只是协议文档，而是可本地 dogfood 的多智能体 runtime：

- Work DAG：用 `rive work ...` / `team work ...` 建立可追踪的任务图、依赖、accept/reopen/retry 语义。
- Reusable workflow：`workflow.yaml + prompts/*.md` 可导入为 immutable template version，再用 `rive workflow run` 重复实例化和执行。
- Scheduler：`rive scheduler run/status/resume` 能按 DAG ready node 调度 worker pool，支持 retry/resume、失败分类和可观测 activity。
- Node-level runner policy：workflow node 可声明 `runner: opencode|codex`、`worker`、`workspace_mode`、`acceptance_mode`；同一个 scheduler run 内可以混合 OpenCode cheap worker 和 Codex judge/merge node。
- Worker workspace isolation：`--workspace-mode worktree` 让 worker patch 先进入隔离 worktree/ref，commit/accept 后才合入 parent workspace。
- Recovery UX：`rive scheduler resume --failed`、`rive work retry`、`rive branch conflict show/reject/retry-from-parent` 用 ledger 方式恢复失败和 patch conflict。
- Debug/usage：debug trace、stdout/stderr refs、scheduler activity、usage read model 都只做诊断，不参与成功判断。

核心边界：成功只来自 ledger/projection（Work DAG、dispatch、scheduler、workflow state），不能从 runner stdout、final answer 或 trace 推断。

## Agent Skill

- [Rive agent skill](./skills/rive/SKILL.md) — 给 Codex / OpenCode 等外部 agent 读取的 Rive 操作手册，用来把用户目标组织成 Work DAG、调度 worker、集成 worktree ref，并按 ledger/projection 回报结果。

## Runner Environment

Rive 启动 OpenCode / Codex worker 时不会假设自己来自交互式 shell，因此不要依赖 `~/.zshrc` 一定会被加载。外部 provider key、证书路径等运行时变量可以放在：

- `~/.config/rive/runner.env`
- `<workspace>/.rive/runner.env`
- `RIVE_RUNNER_ENV_FILE=/path/to/env` 指定的文件

文件格式是简单的 `KEY=value` 或 `export KEY=value`。Rive 会在启动 runner child 前加载这些变量，然后再写入自己的 `RIVE_*` 协议变量和 PATH，避免 env 文件覆盖调度协议。

## Examples

- [Eino technical manual dogfood](./examples/eino-technical-manual/) — 用 Rive Work DAG + OpenCode workers 阅读 CloudWeGo Eino 代码并产出技术手册的真实 dogfood 示例。
- [Google ADK Go code-reading dogfood](./examples/google-adk-go-code-reading/) — 用 Rive Work DAG + OpenCode workers 粗读 `google/adk-go`，按目录分区产出中文架构大纲和总纲。
- [Sentinel reusable workflow template](./examples/workflows/sentinel-prod-debug/) — 可导入/复跑的生产 debug workflow 模板包，包含 DAG 和节点 prompt。
