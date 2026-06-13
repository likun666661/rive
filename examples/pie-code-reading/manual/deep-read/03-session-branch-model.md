# 03 — JSONL Session 分支模型 精读报告

**仓库**: `pie`
**基线**: `f1c35a3`
**深度档位**: `maintainer`
**阅读日期**: 2026-06-13

---

## 1. problem

pie 的 session 模型要解决以下状态管理问题：

| 问题 | 描述 |
|------|------|
| **Resumable Session** | 用户退出 CLI 后需要从上次断点继续。session 必须是持久化的、自描述的。 |
| **Undo（分支回退）** | 用户可以 `move_to(entry_id)` 跳回到某个历史 entry，然后从那里重新开始对话。新的 entry 不会覆盖历史——它们在 append-only JSONL 中以新的叶子形式追加。 |
| **Fork（会话分叉）** | `repo_utils::get_entries_to_fork()` 支持从某个 entry 位置分叉出全新 session，复制到该点位置的路径，生成独立的分支树。 |
| **Compaction（上下文压缩）** | 当对话历史超出模型 context window 时，需要自动摘要前缀消息，保留最近 N 个 token 的内容。compaction 生成一条 `Compaction` entry，标记 `first_kept_entry_id`。 |
| **Export / Import** | 用户可以把整个 session 导出为 `.piesession`（tar 包），含 manifest + transcript + 可选 sidecar（triggers、cron）。导入时分配新 UUID、重写 metadata、可选禁用/启用自动化。 |

核心问题是：在 **append-only** 的约束下，如何同时支持线性对话回放、历史分支、上下文压缩、以及跨机器迁移。

---

## 2. why_hard

### 2.1 Append-only JSONL

JSONL 文件一旦写入就不可修改。这意味着：
- **删除消息** 不可能。undo 不是删除，而是追加一个 `Leaf` entry 指向旧的 `target_id`。
- **修改消息** 不可能。branch summary 是一种压缩表示——它是一条独占的 entry 类型。
- 所有状态变更（leaf 移动、label 标记、compaction）都必须编码为新的 JSONL 行。

### 2.2 Parent DAG

每个 entry 都有 `parent_id: Option<String>`，在 JSONL 中构成一个有向图（DAG）。不同的 `Leaf` entry 可以指向不同的节点，所以：
- 同一棵树上可以有多条活跃路径。
- `get_path_to_root()` 从当前 leaf 沿 `parent_id` 反向追踪到 root，构建线性回放链。
- 需要检测循环（`cycle in parent chain`）。

### 2.3 多 Leaf 并发

leaf 不是写在文件头的固定指针，而是 **日志重放** 得出的：
- `Leaf { target_id }` entry 将当前活动指针移到 `target_id`。
- 任何非 Leaf entry 追加后自动成为新的 leaf。
- 重放 `current_leaf()` 需要遍历所有 entry，将其实时计算出来。

### 2.4 Compaction 的 first_kept 语义

Compaction 不是真的删除前缀，而是：
1. 追加一条 `Compaction` entry，记录 `first_kept_entry_id`。
2. `build_context()` 识别到 `Compaction` 时，跳过 `first_kept_entry_id` 之前的所有内容，插入一条摘要 placeholder。
3. 原始 JSONL 完整保留，任何人都可以通过读取文件来获取完整记录。

### 2.5 Sidecar 文件

session 不仅有 `.jsonl` 主体，还有 `.triggers.json`、`.cron.toml`、`.endpoints.json` 三个 sidecar 文件。它们和 transcript 是松散耦合的——sidecar 丢失可以退化为空，但导出/导入时必须一起打包。这增加了数据完整性风险。

---

## 3. design_approach

pie 的 session 模型采用 **append-only JSONL + parent-pointer DAG + in-log leaf** 的设计：

```
                    JSONL 文件结构
┌──────────────────────────────────────────────────┐
│ Line 1:  { "id":"<uuid>", "createdAt":"...",   │ ← JsonlSessionMetadata header
│            "cwd":"...", "path":"..." }           │
│ Line 2:  { "type":"session_info", "id":"a1",    │ ← SessionTreeEntry (variants below)
│            "parentId":null, "name":"..." }       │
│ Line 3:  { "type":"message", "id":"b1",         │
│            "parentId":"a1", "message":{...} }    │
│ Line 4:  { "type":"leaf", "id":"L1",            │ ← Leaf 显式跳转
│            "parentId":"b1", "targetId":"a1" }    │
│ Line 5:  { "type":"message", "id":"c1",         │ ← 从 a1 分叉
│            "parentId":"a1", "message":{...} }    │
│ Line 6:  { "type":"compaction", "id":"cp1",      │ ← Compaction
│            "parentId":"c1", "firstKeptEntryId":  │
│            "c1", "summary":"...", ... }           │
│ Line 7:  { "type":"message", "id":"d1",         │
│            "parentId":"cp1", "message":{...} }    │
└──────────────────────────────────────────────────┘
```

**架构分层**:

| 层次 | 模块 | 职责 |
|------|------|------|
| Entry 类型 | `session.rs:SessionTreeEntry` | 10 种 tagged variant，统一 JSONL 序列化 |
| Storage 抽象 | `jsonl_storage.rs:JsonlSessionStorage` | 文件 I/O、缓存、leaf 重放 |
| Session 门面 | `session.rs:Session` | 类型安全的 `append_*` 方法、`build_context` |
| Repo | `jsonl_repo.rs:JsonlSessionRepo` | session 文件的 create/open/list/delete |
| Compaction | `compaction/compaction.rs` | 自动压缩决策、cut point、摘要生成 |
| Export/Import | `session_archive.rs` | `.piesession` tar 包打包/解包 |

**核心设计模式**:

1. **Trait 多态**：`SessionStorage` trait 是 session 的抽象接口。`JsonlSessionStorage` 实现文件持久化，`MemorySessionStorage` 实现内存存储（测试/浏览器用）。`Session` 通过 `Arc<dyn SessionStorage>` 持有任意后端。

2. **Append-only 不变性**：没有 delete 或 update 操作。Leaf 移动通过追加 `Leaf { target_id }` entry 实现。

3. **Parent chain replay**：`get_path_to_root(leaf_id)` 从叶子沿 `parent_id` 反向追踪，再 reverse 得到 root→leaf 路径。

4. **Compaction 通过插入摘要消息**：`build_context` 遇到 `Compaction` entry 时，在消息列表中插入 `compaction_summary` 自定义消息，跳过已压缩的前缀。

---

## 4. code_walkthrough

### 4.1 `SessionTreeEntry` (session.rs:26-127)

10 种 entry 变体，全部用 `#[serde(tag = "type")]` 标记：

| Variant | 用途 | 关键字段 |
|---------|------|---------|
| `Message` | 对话消息 | `id`, `parent_id`, `message: AgentMessage` |
| `ThinkingLevelChange` | 思考深度变更 | `thinking_level: String` |
| `ModelChange` | 模型切换 | `provider`, `model_id` |
| `Compaction` | 上下文压缩记录 | `summary`, `first_kept_entry_id`, `tokens_before` |
| `BranchSummary` | 分支摘要 | `from_id`, `summary` |
| `Custom` | 自定义事件 | `custom_type`, `data` |
| `CustomMessage` | 自定义消息（参与回放） | `custom_type`, `content`, `display` |
| `Label` | 标记某个 entry | `target_id`, `label` |
| `SessionInfo` | session 元数据 | `name` |
| `Leaf` | 显式叶子位置 | `target_id` |

每个 variant 都有 `id` 和 `parent_id`，构成 DAG。

### 4.2 `build_session_context` (session.rs:259-355)

核心回放函数。输入是 `get_path_to_root` 返回的 root→leaf 路径。

**算法**：
1. 第一遍扫描：跟踪最新 `thinking_level`、`model`、最近 `compaction` 位置。
2. 第二遍构建消息：
   - 如果有 `compaction`：找到 `first_kept_entry_id`，跳过前面的内容，插入 compaction summary，然后追加 `first_kept` 及之后的内容。
   - 如果没有 compaction：线性追加所有消息。
   - `BranchSummary` 消息如果非空也会被注入。

**关键细节**：compaction 的 `first_kept_entry_id` 是 entry id 字符串，不是文件行号。需要在 path entries 中线性查找匹配。

### 4.3 `JsonlSessionStorage` (jsonl_storage.rs:20-261)

文件级别的存储实现。

**布局**：
- Line 1: JSON 序列化的 `JsonlSessionMetadata`（header）
- 后续行: `SessionTreeEntry` JSONL

**缓存策略**：
- `cache: Mutex<Option<Vec<SessionTreeEntry>>>`
- 第一次读取时懒加载整个文件到内存
- 每次 `append_entry` 后 `invalidate_cache()`，下一次读取重新加载

**current_leaf() 重放逻辑** (jsonl_storage.rs:124-141):
```rust
// 遍历所有 entry，Leaf 显式跳转，非 Leaf 自动成为当前 leaf
for entry in &entries {
    match entry {
        SessionTreeEntry::Leaf { target_id, .. } => leaf = target_id.clone(),
        _ => leaf = Some(entry.id().to_string()),
    }
}
```

**set_leaf_id()** (jsonl_storage.rs:167-176):
- 不修改文件，而是追加一条 `Leaf` entry
- `parent_id` 取自当前 leaf（记录从哪里跳转）
- `target_id` 是目标 entry

**get_path_to_root()** (jsonl_storage.rs:207-236):
- 从 `leaf_id` 开始，沿 `parent_id` 反向追踪
- 用 `HashSet` 检测循环
- 最后 `reverse()` 得到 root→leaf 顺序

### 4.4 `JsonlSessionRepo` (jsonl_repo.rs)

**create()** (jsonl_repo.rs:30-39):
- `mkdir_all` 创建 sessions 目录
- 文件名 = `{uuidv7}.jsonl`（UUIDv7 自带时间排序）
- 调用 `JsonlSessionStorage::create()` 写入 header

**open()** (jsonl_repo.rs:42-53):
- 支持绝对路径或相对路径（相对于 repo root）
- 调用 `JsonlSessionStorage::open()` 解析 header

**list()** (jsonl_repo.rs:56-71):
- 读取目录，过滤 `.jsonl` 后缀
- 默认按文件名升序排列（UUIDv7 保证时间顺序）

### 4.5 `Session` 门面 (session.rs:361-595)

**append_message** (session.rs:440-450):
```rust
let id = self.storage.create_entry_id().await?;  // 生成新 UUID
let parent = self.storage.get_leaf_id().await?;  // 当前 leaf = parent
self.append_typed(SessionTreeEntry::Message { id, parent_id: parent, ... }).await
```
注意：`parent_id` 取的是 **追加时刻** 的 `get_leaf_id()`。这意味着如果两条消息并发追加到同一个 session，它们可能共享同一个 parent（但这不是当前主要用例）。

**move_to** (session.rs:561-587):
- 调用 `storage.set_leaf_id(target)` → 内部追加 `Leaf` entry
- 可选附带 `BranchSummary` entry（记录跳转原因和上下文摘要）

**append_compaction** (session.rs:484-505):
- 记录 `summary`、`first_kept_entry_id`、`tokens_before`
- `from_hook` 标记是否由生命周期钩子自动触发

### 4.6 Fork 语义 (repo_utils.rs:23-68)

`get_entries_to_fork()` 支持两种切分位置：

- **`ForkPosition::Before`**（默认）：在用户消息之前分叉，`effective_leaf = parent_id`
- **`ForkPosition::At`**：在特定 entry 处精确分叉，`effective_leaf = target.id`

要求 `entry_id` 必须是用户消息。如果不是，报 `NotFound` 错误。

### 4.7 Compaction 系统 (compaction/compaction.rs)

**CompactionSettings** (compaction.rs:34-54):
```rust
DEFAULT_COMPACTION_SETTINGS: {
    enabled: true,
    reserve_tokens: 16_384,    // 留给摘要 prompt + 输出的空间
    keep_recent_tokens: 20_000 // 保留的上下文大小
}
```

**should_compact** (compaction.rs:192-207):
- 触发阈值：`context_tokens > window * 4/5`（80% 窗口大小）

**find_cut_point** (compaction.rs:246-275):
- 从后往前累积 token，直到超过 `keep_recent_tokens`
- 回退到最近的 turn boundary（用户消息）

**compact()** (compaction.rs:635-699):
- 调用 `prepare_compaction` 找到 cut point
- 把前缀消息提取为 `Vec<AgentMessage>`
- 调用 `generate_summary` 生成摘要
- 如果 provider 报 context overflow，**减半预算重试**（最多 3 次）

**Token 估算** (compaction.rs:102-113):
- ASCII: ~4 字符 / token
- Non-ASCII (CJK): ~1 字符 / token
- 图片: 固定 768 tokens（匹配 Anthropic 定价近似）

**Branch Summarization** (branch_summarization.rs):
- 复用 `generate_summary`，使用专门的 prompt
- 用于 fork 时的 context 传递

### 4.8 Export/Import (session_archive.rs)

**导出流程**:
1. 读取完整 session JSONL 文本
2. 解析 header + entries，计算 SHA-256
3. 读取 sidecar 文件（triggers.json, cron.toml）
4. 生成 `manifest.json`
5. 打包为 tar，权限 `0o600`

**导入流程**:
1. 解包 tar，校验路径安全
2. 校验 manifest schema、SHA-256、entry count、active leaf
3. 分配新 UUIDv7
4. 重写 header（新 id、新 cwd、`imported_from` 来源信息）
5. 重写 sidecar（清空运行态字段：`running_trace_id`、`last_due_at`、`last_error` 等）
6. 可选禁用自动化（`enable && activate` 逻辑）
7. 原子提交：先写 `.tmp` → 校验回放 → 写 sidecar → rename，失败则全部清理

**安全措施**:
- 路径遍历检测（拒绝 `../`、绝对路径）
- 文件大小上限（manifest: 128KB, session: 50MB, sidecar: 2MB）
- SHA-256 完整性校验
- 重复 entry id 检测、悬空 parent 检测、悬空 leaf target 检测

### 4.9 Sidecar 文件 (session/mod.rs)

coding-agent 的 session helper 定义了三个 sidecar:
- `*.triggers.json` — 动态触发规则
- `*.cron.toml` — cron 作业
- `*.endpoints.json` — 公共端点

`automation_counts()` 解析 sidecar 获取启用/总数统计，用于 session 列表展示和跨 session 提示。

---

## 5. branch_examples

### Example 1: 基本 undo（move_to 回去继续对话）

```jsonl
{"type":"message","id":"0199a-001","parentId":null,"timestamp":"2026-06-13T00:00:00Z","message":{"role":"user","content":"帮我写一个排序函数"}}
{"type":"message","id":"0199a-002","parentId":"0199a-001","timestamp":"2026-06-13T00:00:05Z","message":{"role":"assistant","content":"这是快速排序的实现..."}}
{"type":"leaf","id":"0199a-L01","parentId":"0199a-002","timestamp":"2026-06-13T00:00:10Z","targetId":"0199a-001"}
{"type":"message","id":"0199a-003","parentId":"0199a-001","timestamp":"2026-06-13T00:00:15Z","message":{"role":"user","content":"用归并排序重新实现"}}
{"type":"message","id":"0199a-004","parentId":"0199a-003","timestamp":"2026-06-13T00:00:20Z","message":{"role":"assistant","content":"这是归并排序..."}}
```

**解析**:
- Line 3: `Leaf { targetId: "0199a-001" }` 将 leaf 指针从 `0199a-002` 移回 `0199a-001`
- Line 4-5: 新对话从 `0199a-001` 分叉，形成独立分支
- DAG 结构: `001 → 002` (分支 A)，`001 → 003 → 004` (分支 B)
- `current_leaf()` = `0199a-004`
- `get_path_to_root("0199a-004")` = `[001, 003, 004]`
- `get_path_to_root("0199a-002")` = `[001, 002]`

### Example 2: compaction 后的上下文回放

```jsonl
{"type":"message","id":"0199b-001","parentId":null,"timestamp":"...","message":{"role":"user","content":"请解释 Rust 的 borrow checker"}}
{"type":"message","id":"0199b-002","parentId":"0199b-001","timestamp":"...","message":{"role":"assistant","content":"borrow checker 的核心规则是..."}}
{"type":"message","id":"0199b-003","parentId":"0199b-002","timestamp":"...","message":{"role":"user","content":"给我一个生命周期标注的例子"}}
{"type":"message","id":"0199b-004","parentId":"0199b-003","timestamp":"...","message":{"role":"assistant","content":"下面是一个带生命周期的函数..."}}
{"type":"compaction","id":"0199b-CP1","parentId":"0199b-004","timestamp":"...","summary":"用户询问 borrow checker 和生命周期，assistant 解释了 borrow checker 规则并提供了生命周期标注示例。","firstKeptEntryId":"0199b-003","tokensBefore":450,"fromHook":true}
{"type":"message","id":"0199b-005","parentId":"0199b-CP1","timestamp":"...","message":{"role":"user","content":"那 NLL 是什么？"}}
{"type":"message","id":"0199b-006","parentId":"0199b-005","timestamp":"...","message":{"role":"assistant","content":"NLL 即 Non-Lexical Lifetimes..."}}
```

**解析**:
- `build_context` 扫描到 `compaction` entry 在位置 idx
- 找到 `firstKeptEntryId="0199b-003"`，在 path entries 中定位
- 最终消息列表: `[compaction_summary msg, 003, 004, 005, 006]`
- 001 和 002 被跳过（其内容已通过摘要表达）
- compaction entry 的 `parent_id` 是 `0199b-004`，形成线性链

### Example 3: Fork + BranchSummary

```jsonl
{"type":"branch_summary","id":"0199c-BS1","parentId":"0199a-001","timestamp":"...","fromId":"0199a-001","summary":"在排序函数讨论中，已实现快速排序和归并排序两种方案。当前正在优化性能。"}
```

**场景**: 从 `0199a-001` 分叉出全新 session 时，生成一条 `BranchSummary`：
- `fromId` 指向分支点
- `summary` 由 `summarize_branch()` 生成
- 新的 session 在 `build_context` 时会注入这条摘要消息，让模型知道分支的背景

---

## 6. compaction_integrity

### 6.1 Compaction 数据完整性

**不丢消息**: Compaction 永远只追加新 entry，不修改或删除已有 entry。完整的对话历史在 JSONL 文件中始终存在。`build_context` 通过 `first_kept_entry_id` 决定哪些消息被摘要替代。

**Turn-boundary 安全**: `find_cut_point` 调用 `find_turn_start_index` 确保 cut 点落在用户消息（turn boundary）上。这避免了在 assistant 的思考/工具调用中间切断。

**Token 预算防守**:
1. `summarization_prompt_budget` 保留 20% slack（`window * 4/5`），减少因 token 估算偏差导致的 context overflow。
2. `suffix_start_for_token_budget` 对序列化后的对话做二次截断。
3. `compact()` 对 context overflow 做渐进式重试（减半预算，最多 3 次，最小 1024 tokens）。
4. `summary_output_tokens` 为 summarizer 调用设置 `max_tokens`，避免 `input + max_tokens > context_window` 的 Anthropic 硬限制。

**Compaction summary 的消息注入**: `build_context` 在消息列表最前面插入 `compaction_summary` 自定义消息（role = `compaction_summary`），这是下游 agent 理解上下文压缩的唯一标记。

### 6.2 Resume 数据完整性

**Leaf 重放**: `current_leaf()` 从头重放整个 JSONL 文件来计算当前 leaf。这保证了：
- 进程崩溃不会丢失 leaf 状态（leaf 状态在 JSONL 中，不在内存中）。
- 多个进程可以安全地读取同一个 session 文件（只读解析不做锁定）。
- `set_leaf_id` 追加 `Leaf` entry → 持久化 leaf 变更。

**Branch 重建**: `get_path_to_root` 沿 `parent_id` 反向追踪，有循环检测保护。

### 6.3 Export 数据完整性

**校验链**:
1. Manifest 记录 `session_jsonl_sha256` → 导入时校验哈希
2. Manifest 记录 `entry_count` → 导入时校验与解析结果一致
3. Manifest 记录 `active_leaf_id` → 导入时校验（防止 manifest 被篡改导致回放错误的 leaf）

**完整性验证** (`parse_session_jsonl`, session_archive.rs:365-413):
- 检测重复 entry id
- 检测悬空 parent 引用
- 检测悬空 leaf target

**原子导入** (`commit_import`, session_archive.rs:450-483):
- 先写 `.tmp` 文件
- 通过 `staged.build_context()` 验证可回放
- 写入 sidecar
- `rename` 到最终位置
- 任何步骤失败 → 清理 `.tmp` 和 sidecar → 不产生孤儿文件

---

## 7. tests

### 7.1 单元测试位置

| 文件 | 测试内容 | 覆盖意图 |
|------|---------|---------|
| `crates/agent/tests/session_storage.rs` | Session 持久化 + 分支 E2E | 验证 memory/jsonl 双后端的一致性、持久化、leaf 移动、compaction context |
| `crates/agent/src/harness/compaction/compaction.rs` (内联 `#[cfg(test)]`) | Compaction 算法 | `should_compact` 阈值、cut point turn-boundary、summarizer prompt 截断、token 预算、CJK 估算、overflow 重试 |
| `crates/agent/src/harness/session/uuid.rs` (内联) | UUIDv7 时间排序 | 验证前后生成的 UUID 排序正确 |
| `crates/coding-agent/tests/cli_session.rs` | CLI session 生命周期 | create→persist→reopen→resume 完整链路、rehydrate 状态恢复 |
| `crates/coding-agent/tests/export_e2e.rs` | Export（`/save` 命令） | Markdown 导出、消息顺序验证 |
| `crates/coding-agent/src/session/mod.rs` (内联) | 自动化 sidecar + resume/delete | automation counts 读取、imported_from 来源追踪、sidecar 路径解析、legacy metadata 匹配 |
| `crates/coding-agent/src/session_archive.rs` (内联) | Export/Import 端到端 | manifest 校验、metadata 重写、自动化激活/禁用、原子导入回滚、路径安全检测、owner-only 权限 |

### 7.2 测试覆盖意图总结

- **持久化正确性**: jsonl 写入后 reopen 数据不丢失 (`jsonl_session_persists_across_open`)
- **分支语义**: leaf 移动后新 entry 形成独立分支 (`jsonl_explicit_leaf_moves_are_overridden_by_new_entries`)
- **Root 移动**: `move_to(None)` 将 leaf 清空，`branch(None)` 返回空 (`jsonl_can_move_leaf_to_root`)
- **Parent 链**: 从 leaf 回溯到 root 的顺序正确 (`branch_walks_parent_chain_in_root_to_leaf_order`)
- **Compaction 上下文**: 压缩后消息列表 = summary + kept + new (`compaction_summary_replaces_history_up_to_first_kept`)
- **Import/Export 完整性**: metadata 重写、sidecar 禁用、来源追踪、原子回滚 (`session_archive` 多个测试)

---

## 8. risks

### 8.1 数据损坏风险

- **JSONL 行损坏**: 一行 JSON 解析失败会报 `SessionErrorCode::Corrupted`，阻止整个文件加载。没有行级容错/跳过机制。
- **Header 损坏**: header 行损坏意味着 session 完全无法打开，丢失所有对话历史。
- **Parent ID 悬空**: `parse_session_jsonl` 在导入时有校验，但 `JsonlSessionStorage::open` 只在 `get_path_to_root` 时发现。日常使用中如果外部进程修改了 JSONL 文件，可能导致"not found" 错误。

### 8.2 无限增长风险

- JSONL 文件永不收缩。compaction 只追加摘要 entry，不释放磁盘空间。
- 没有自动归档或日志轮转机制。
- 长时间 session 可能产生 MB 级甚至 GB 级 JSONL 文件，每次 `load_entries()` 都要全量读入内存。

### 8.3 Legacy 兼容风险

- **file_stem ≠ metadata id**: 正常创建时 UUIDv7 文件名 = metadata.id。但导出→导入后分配新 UUID，旧 session 可能出现 name 与 metadata 不一致。`find_sessionPath` 支持 metadata id 前缀匹配来兼容此情况。
- **Schema 版本**: export archive 有 `SCHEMA = "pie.session_export.v1"`，但不支持 schema 版本迁移。未来 v2 格式的 archive 会被拒绝。
- **`parent_session_path` 字段**: session metadata 中有此字段但始终为 `None`，为未来 fork 链追踪预留。

### 8.4 Sidecar 缺失风险

- Sidecar 文件和 JSONL 之间没有硬性的一致性保证。JSONL 存在但 sidecar 丢失 → 自动化规则丢失，但 session 本身仍可正常使用。
- `automation_counts()` 对解析失败的 sidecar 静默退化为零，用户可能不知道自己的 cron jobs 或 triggers 已经丢失。
- 删除 session 时同时删除 4 个文件（jsonl + triggers + cron + endpoints），但没有事务保证——可能部分删除成功，留下孤儿 sidecar。

### 8.5 并发风险

- **写并发**: 多个进程同时对同一个 JSONL 文件 `append_entry` 会导致交错写入。`OpenOptions::append(true)` 配合 `write_all` 不是原子操作。
- **读写并发**: `invalidate_cache` + `load_entries` 之间没有文件锁。读可能看到部分写入。
- 实际使用场景中，通常只有当前 CLI 进程在写同一个 session，风险较低。

### 8.6 内存风险

- `cache: Mutex<Option<Vec<SessionTreeEntry>>>` 将整个 session 加载到内存。对于大型 session（数万条消息），内存消耗可能显著。
- `get_path_to_root` 创建 `HashSet` 和 `Vec` 副本，对于深层嵌套的 DAG 可能产生额外开销。

---

## 9. next_questions

1. **Compaction 触发时机**: 目前 `compact()` 是显式调用的，是否有计划在 AgentHarness 的 event loop 中集成自动触发？当前是否在某个 hook 点调用？

2. **Session 大小上限**: 对于频繁交互的用户，session JSONL 可能在数天内增长到数 MB。是否有计划引入日志轮转或增量加载（只读最近 N 条 entry）？

3. **Branch DAG 可视化**: 当前 leaf 模型支持多分支，但没有 UI 来展示 DAG 拓扑。`/tree` 或类似命令是否在 roadmap 中？

4. **Sidecar 事务**: 未来是否会引入 session file group 的原子操作（例如在 JSONL 中记录 sidecar 引用/checksum）来防止孤儿文件？

5. **Compaction 的质量度量**: 目前只有 token 计数和 overflow 重试。是否有计划让 agent 评估 compaction summary 的质量（如验证摘要是否准确覆盖了被压缩的内容）？

6. **Import 冲突处理**: 如果目标 sessions 目录已经存在一个  `{new_id}.jsonl`（UUIDv7 碰撞机会极低但非零），导入会失败。是否有回退策略？

7. **MemorySessionStorage 和 JsonlSessionStorage 的测试等价性**: 当前测试两个后端各自有独立测试。是否有计划引入参数化测试来系统性地验证两种实现在所有 SessionStorage 接口上的行为一致性？
