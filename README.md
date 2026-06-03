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

## Agent Skill

- [Rive agent skill](./skills/rive/SKILL.md) — 给 Codex / OpenCode 等外部 agent 读取的 Rive 操作手册，用来把用户目标组织成 Work DAG、调度 worker、集成 worktree ref，并按 ledger/projection 回报结果。

## Examples

- [Eino technical manual dogfood](./examples/eino-technical-manual/) — 用 Rive Work DAG + OpenCode workers 阅读 CloudWeGo Eino 代码并产出技术手册的真实 dogfood 示例。
- [Sentinel reusable workflow template](./examples/workflows/sentinel-prod-debug/) — 可导入/复跑的生产 debug workflow 模板包，包含 DAG 和节点 prompt。
