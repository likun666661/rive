# 08 — Session 完整性审查报告

**仓库**: `pie`
**审查日期**: 2026-06-13
**基线**: `f1c35a3`
**深度档位**: `maintainer`
**依赖产物**: `03-session-branch-model.md`

---

## 1. executive_summary

pie 的 session 完整性模型基于 **append-only JSONL + parent-pointer DAG + in-log leaf** 架构。该设计从根本上避免了"修改即损坏"的风险——不存在 delete/update 操作，所有状态变更通过追加新 entry 实现。整体架构在单进程场景下具有令人满意的完整性。然而，审查发现以下核心缺口：

1. **JSONL 写入无原子保证**：并发 append 可导致行交错，崩溃可导致文件尾部截断，且无恢复机制。
2. **Compaction 对分支路径的语义不明确**：compaction entry 的 `parent_id` 始终是追加时刻的 leaf，当 leaf 通过 `move_to` 跳转后，compaction 与分支路径之间的关系缺乏形式化定义。
3. **Export/Import 的 sidecar 一致性薄弱**：JSONL 与 sidecar 之间无校验和或引用关联，sidecar 丢失后静默退化。
4. **内存与磁盘无限增长**：无归档、轮转或分页机制，长期运行的 session 可能在 MB 甚至 GB 规模下崩溃。
5. **AgentMessage 的 untagged 序列化**存在歧义风险：自定义消息可能被误解析为 LLM 消息。

**总体评级**: 架构正确但工程健壮性不足。单进程常规使用安全，边缘场景（并发、崩溃、超大 session）存在数据丢失风险。

---

## 2. integrity_model

### 2.1 Session Entry 完整性

| 检查项 | 现状 | 完整性评估 |
|--------|------|-----------|
| Entry ID 唯一性 | UUIDv7 生成，碰撞概率极低 | ✅ 良好 |
| parent_id 引用完整性 | 运行时追加不校验；导入时校验 `parse_session_jsonl` (`session_archive.rs:389-393`) | ⚠️ 运行时无防御 |
| 重复 entry id 检测 | 仅在导入时检测 (`session_archive.rs:386-388`) | ⚠️ 运行时无防御 |
| 循环检测 | `get_path_to_root` 使用 HashSet 检测循环 (`jsonl_storage.rs:222-226`) | ✅ 良好 |
| Header 行完整性 | 损坏导致整个 session 无法打开 (`jsonl_storage.rs:73-81`) | ❌ 单点故障 |
| 行级容错 | 一行解析失败 → 报 `Corrupted`，阻止全部加载 (`jsonl_storage.rs:110-113`) | ❌ 无行级容错 |
| JSONL 截断恢复 | 崩溃导致最后一行不完整 → 加载失败 | ❌ 无恢复机制 |

**代码位置与细节**:

- Entry 追加: `session.rs:440-450` — `parent_id` 取自追加时刻的 `get_leaf_id()`，不校验引用有效性。
- TOCTOU in `append_label`: `session.rs:530` — 先 `get_entry(target)` 检查存在性，再 `append_entry`，两步之间无原子保证。
- Loop detection: `jsonl_storage.rs:222-226` — `HashSet` 记录已访问 id，遇到重复 id 即返回错误。

### 2.2 Parent DAG 完整性

parent DAG 的完整性依赖以下不变式:

```
1. 每个 entry 的 parent_id 要么是 None（root），要么指向已存在的 entry
2. 从任一 entry 沿 parent_id 回溯必能到达 root（无环、无不可达节点）
3. 从 root 沿 parent_id 正向可到达所有非孤立 entry
```

| 不变式 | 保障机制 | 风险评估 |
|--------|---------|---------|
| #1 (引用有效性) | 导入时校验，运行时追加不校验 | ⚠️ 中等 |
| #2 (可回溯) | `get_path_to_root` 循环检测 | ✅ 良好 |
| #3 (全连通) | 追加时 `parent_id` 取当前 leaf → 自动连通 | ⚠️ 并发下不保证 |

**并发破坏场景**: 两个进程同时调用 `append_message`:
1. 进程 A 读取 `leaf_id = X`
2. 进程 B 读取 `leaf_id = X`
3. 进程 A 追加 entry `Y` with `parent_id = X`
4. 进程 B 追加 entry `Z` with `parent_id = X`
5. 结果: Y 和 Z 是兄弟节点（共享 parent X），这不违反不变式
6. 但如果 B 读到 X 后，A 先追加了 Y，B 再追加 Leaf → 然后 A 追加的消息的 leaf 状态就会过时

实际影响: 当前 pie 没有多进程并发写入同一 session 的用例（CLI 独占），但代码层面未做防御。

### 2.3 Leaf 完整性

Leaf 不是持久化的指针，而是**日志重放**的结果 (`jsonl_storage.rs:124-141`):

```rust
// 伪代码
for entry in entries {
    match entry {
        Leaf { target_id } => leaf = target_id,
        _ => leaf = entry.id,
    }
}
```

| 属性 | 评估 |
|------|------|
| 崩溃恢复 | ✅ 无需恢复——leaf 是从 JSONL 重放得出的 |
| 多 reader 安全 | ✅ reader 各自重放，无共享状态 |
| 重放一致性 | ✅ 给定相同的 JSONL，始终产生相同的 leaf |
| 悬空 target | ⚠️ `set_leaf_id` 不校验 `target_id` 是否存在 (`jsonl_storage.rs:167-176`) |
| Leaf 跳过 | ⚠️ 额外的 Leaf entry 可能被非 Leaf entry 覆盖（新 entry 自动成为 leaf） |

### 2.4 Sidecar 完整性

Sidecar 文件与 JSONL 之间的耦合:

| 耦合点 | 现状 | 风险 |
|--------|------|------|
| 命名关联 | 通过 file_stem 关联（`{stem}.triggers.json` 等） | ⚠️ 重命名 `.jsonl` 破坏关联 (`session/mod.rs:38-52`) |
| 一致性校验 | 无 checksum 或引用字段 | ❌ sidecar 丢失静默退化 (`session/mod.rs:298,304`) |
| 删除原子性 | 逐个删除，容忍 NotFound，其他错误提前退出 | ❌ 部分删除成功导致孤儿文件 (`session/mod.rs:171-188`) |
| JSONL→Sidecar 引用 | 无 | ❌ 无法检测 sidecar 是否是最新的 |

### 2.5 Repo / Storage 完整性

| 检查项 | 现状 | 评估 |
|--------|------|------|
| 文件命名 | `{uuidv7}.jsonl` (`jsonl_repo.rs:34`) | ✅ UUIDv7 自带时间排序 |
| ID 来源 | `file_stem` → session id (`jsonl_storage.rs:41-46`) | ⚠️ 重命名改变 id |
| 孤儿 .tmp 文件 | 导入失败时的 .tmp 无清理 (`session_archive.rs:281,476-481`) | ⚠️ repo list 不显示但占据磁盘 |
| 绝对路径 open | `open()` 支持绝对路径绕过 repo root (`jsonl_repo.rs:44-45`) | ⚠️ 路径穿越风险 |
| Memory ↔ JSONL 行为一致性 | 两个 backend 的 leaf 计算逻辑不同 (`memory_storage.rs:75` vs `jsonl_storage.rs:128-138`) | ⚠️ 可能存在行为差异 |

---

## 3. compaction_resume

### 3.1 Compaction 在分支场景下的正确性

Compaction entry 的结构:

```json
{
  "type": "compaction",
  "id": "<uuid>",
  "parentId": "<当前leaf>",
  "firstKeptEntryId": "<保留的第一条entry的id>",
  "summary": "<压缩摘要>",
  "tokensBefore": 450,
  "fromHook": true
}
```

**关键语义问题**: Compaction entry 的 `parent_id` 是追加时刻的 leaf。此后如果用户执行 `move_to` 跳回早期节点并开始新对话:

```
Entry Chain:
  root → A → B → compaction(CP) → C → D
                          ↑ parent_id = B

  然后 move_to A:
  ... → Leaf(targetId=A) → E → F

  此时 get_path_to_root(F) = [root, A, E, F]

  Compaction entry CP 不在 F 的路径上！
  所以 build_context(F) 不会看到 compaction，B 之后的内容也不会被压缩。
```

**分析**: 这种行为是**正确的**。分支 F 是一条全新的路径，和 B→CP→C→D 是并行的。F 的路径上没有 compaction entry，自然不触发压缩。如果 F 路径未来增长到需要压缩，会在 F 路径上追加新的 compaction entry。

但存在以下边缘场景:

| 场景 | 行为 | 正确性 |
|------|------|--------|
| move_to(CP) 回到 compaction entry | `get_path_to_root(CP)` 返回包含 CP 的路径，`build_context` 会应用压缩 | ✅ 正确 |
| move_to(在 CP 之后的 entry) | 路径包含 CP，压缩被应用 | ✅ 正确 |
| move_to(firstKept 之前的 entry) | 路径不包含 CP，压缩不应用 | ✅ 正确（新分支） |
| 单条路径上多次 compaction | `build_context` 扫描时取最后一个 compaction (`session.rs:300-307`)，先前的 compaction 被忽略 | ✅ 正确 |
| Compaction entry 的 parent_id 悬空 | 如果被指 parent 不在当前路径上，`get_path_to_root` 会报错 | ⚠️ 边缘 case |

### 3.2 Resume 在分支场景下的正确性

**Resume 的 leaf 重建** (`jsonl_storage.rs:124-141`):

```
重放所有 entry → 遇到 Leaf 跳转 → 非 Leaf 自动成为 leaf
```

Resume 后的路径始终是从重放得出的 leaf 到 root。这意味着:
- 如果最后一次操作是 `move_to(X)`，resume 后 leaf = X，对话从 X 继续。
- 如果最后一次操作是新增消息，resume 后 leaf = 最新消息。

**测试覆盖**: `jsonl_can_move_leaf_to_root` 验证 `move_to(None)` 后 leaf 为空；`jsonl_explicit_leaf_moves_are_overridden_by_new_entries` 验证新消息覆盖 Leaf 跳转。

**正确性**: ✅ Resume 的 leaf 重建逻辑在所有分支场景下都是正确的——leaf 是追加日志的自然结果。

### 3.3 Compaction 触发时机

Compaction 不是自动触发的。当前依赖显式调用 `compact()` (`compaction/compaction.rs:635-699`)。触发不在 `build_context` 中，也不在 event loop 中。这意味着:
- 如果长时间不调用 `compact()`，context window 会溢出且无保护。
- `build_context` 只做**回放**压缩，不做**触发**压缩。

**风险**: 如果 UI 层因任何原因未能及时调用 `compact()`，模型 API 调用可能因 context overflow 而失败。

---

## 4. export_import

### 4.1 `.piesession` 格式完整性

**导入校验链** (`session_archive.rs:224-238`):

```
schema 版本 → SHA-256 → entry_count → active_leaf_id
```

在校验通过后才解析 entries (`parse_session_jsonl`, `session_archive.rs:365-413`):

```
重复 entry id → 悬空 parent → 悬空 leaf target
```

**完整性评估**: ✅ 校验链完整，覆盖了 manifest 篡改、文件损坏、语义错误三个层面。

### 4.2 原子导入

`commit_import` (`session_archive.rs:450-483`) 实现了四阶段原子提交:

```
1. 写 .tmp 文件
2. build_context 验证可回放
3. 写 sidecar
4. rename .tmp → .jsonl
失败 → 清理 .tmp + sidecar
```

**评估**: ✅ 原子性设计良好。但存在以下风险:
- `rename` 不可跨文件系统（macOS/Linux 同卷 OK，Docker volume 边界需注意）。
- sidecar 写入和 JSONL rename 不是同一事务——如果第 4 步失败但 sidecar 已写入，JSONL 不存在但 sidecar 成为孤儿。

### 4.3 Sidecar 重写逻辑

| 字段 | 重写策略 | 风险 |
|------|---------|------|
| `rule.enabled` | `enabled && activate` (AND) | ✅ 正确：源头禁用的不应被激活 |
| `running_trace_id` | 清空 | ✅ 正确：不应继承运行态 |
| `last_due_at` | 清空 | ✅ 正确 |
| `last_error` | 清空 | ✅ 正确 |
| `skipped_overlap_count` | 清空 | ✅ 正确 |
| `fired_at` | 保留 | ✅ 正确：审计轨迹保留 |

### 4.4 已知风险

| 风险项 | 代码位置 | 影响 |
|--------|---------|------|
| `ActivateTriggers::Ask` 未实现 | `session_archive.rs:208-212` | 阻塞交互式导入 |
| 文件大小硬上限无配置 | `session_archive.rs:549-562` (50MB) | 大 session 无法导出 |
| `activate_imported` 同步 I/O | `session_archive.rs:321-358` | 在 async runtime 中调用会阻塞 |
| Schema 版本仅支持 v1 | `session_archive.rs:224-226` | 未来格式不兼容 |
| `create_archive_file` 不允许覆盖 | `session_archive.rs:489` | 重复导出失败 |
| 导入读取全量到内存 | `session_archive.rs:120` (50MB 上限) | 50MB JSONL 可产生更大内存占用 |

---

## 5. risks

### 5.1 数据损坏风险

| 风险 | 严重度 | 触发条件 | 缓解现状 |
|------|--------|---------|---------|
| JSONL 行截断（崩溃） | 🔴 高 | 写入过程中进程崩溃 | 无恢复机制 |
| 并发 append 交错 | 🟡 中 | 多进程同时写入同一 session | 无文件锁 |
| Header 行损坏 | 🔴 高 | 磁盘故障、手动编辑 | 整个 session 无法打开 |
| 单行 JSON 损坏 | 🟡 中 | 同上 | 整个文件加载失败 |
| AgentMessage untagged 歧义 | 🟡 中 | 自定义消息具有 LLM 消息形状 | 可能被误反序列化 (`types.rs:120`) |

### 5.2 数据丢失风险

| 风险 | 严重度 | 触发条件 | 当前状态 |
|------|--------|---------|---------|
| Sidecar 孤儿（删除不完整） | 🟡 中 | 删除过程中间失败 | `session/mod.rs:171-188` 提前退出 |
| Leaf target 悬空 | 🟡 中 | 手动编辑 JSONL 或并发 | 追加不校验，回放时报错 |
| 重命名导致 sidecar 断开 | 🟡 中 | 用户或工具重命名 `.jsonl` | `session/mod.rs:51` fallback 可能兜底 |
| Import 冲突（UUID 碰撞） | 🟢 低 | 极低概率 UUIDv7 碰撞 | 导入失败，无重试策略 |

### 5.3 无限增长风险

| 风险 | 严重度 | 影响 |
|------|--------|------|
| JSONL 永不收缩 | 🟡 中 | 磁盘占用持续增长，compaction 不释放空间 |
| 全量内存加载 | 🔴 高 | `load_entries` 在读全量到 `Vec`，超大 session 可 OOM |
| .tmp 文件堆积 | 🟢 低 | 导入失败产生的 .tmp 在 sessions 目录累积 |
| 无归档/轮转 | 🟡 中 | 无自动清理或容量告警机制 |

**量化估算**: 假设每条消息平均 500 tokens (~2000 字符)，频繁对话 1000 轮:
- JSONL 大小 ≈ 2MB
- 内存中 `Vec<SessionTreeEntry>` ≈ 1000 项 → 可接受
- 10000 轮对话: ~20MB JSONL → 内存压力明显
- 100000 轮: ~200MB JSONL → 可能导致 OOM

### 5.4 Legacy 兼容风险

| 风险 | 严重度 | 说明 |
|------|--------|------|
| file_stem ≠ metadata.id | 🟡 中 | 导入后分配新 UUID，但旧 session 可能存在此不一致 (`session/mod.rs:38-52`) |
| Schema v1 only | 🟢 低 | 未来 v2 格式 archive 被拒绝 (`session_archive.rs:224-226`) |
| `parent_session_path` 始终为 None | 🟢 低 | 字段预留但未使用，无兼容问题 |
| Memory backend leaf 逻辑与 JSONL 不同 | 🟢 低 | 内存中直接设 leaf，不重放；当前主要体现在效率差异 |
| 旧版 sidecar 字段缺失 | 🟢 低 | `automation_counts` 使用 let-chain 退化 (`session/mod.rs:298,304`) |

### 5.5 并发风险（补充分析）

| 场景 | 风险 | 实际影响 |
|------|------|---------|
| 两进程 append 同一 JSONL | 行交错 → 文件损坏 | 🔴 但 pie 当前单 CLI 独占 session |
| 读写并发 | 读可能看到部分写入 | 🟡 读文件时另一进程在追加 |
| `append_label` TOCTOU | 目标 entry 在检查后被删除 | 🟢 低概率 |
| `set_leaf_id` TOCTOU | `parent_id` 过时 | 🟡 虽不影响正确性但语义不精确 |
| MemoryStorage Mutex poison | `expect` → panic | 🟡 一个线程 panic 崩溃所有用户 |

---

## 6. recommendations

按优先级排序:

### 🔴 P0 — 高优先级（数据丢失风险）

1. **JSONL 截断恢复机制** (`jsonl_storage.rs:110-113`)
   - **问题**: 崩溃导致最后一行不完整 → session 无法打开。
   - **建议**: 在 `load_entries` 中，若最后一行解析失败且文件末尾无 `\n`，则截断最后一行并继续解析，同时在日志中发出警告。考虑在 `append_entry` 前先写一个临时标记或使用 `write_all` + `flush` + `fsync` 确保写入完整性。
   - **预计工作量**: 2-3 天

2. **全量内存加载改为增量/分页** (`jsonl_storage.rs:101-105`)
   - **问题**: 超长 session 的 `load_entries` 导致 OOM。
   - **建议**: 为 `get_entries` 和 `get_path_to_root` 添加可选 offset/limit，支持只加载最近 N 条 entry。至少为 `build_context` 场景实现路径截断（仅加载 leaf→root 路径上的条目）。
   - **预计工作量**: 3-5 天

3. **add_label TOCTOU 修复** (`session.rs:530`)
   - **问题**: 检查 entry 存在和追加 entry 之间无原子性。
   - **建议**: 将存在性校验移到 `append_entry` 内部（或至少在同一锁保护范围内），或者放宽校验为 best-effort（不检查 target 存在性，与 `set_leaf_id` 行为一致）。
   - **预计工作量**: 0.5 天

### 🟡 P1 — 中等优先级（健壮性提升）

4. **文件级并发锁** (`jsonl_storage.rs:182-193`)
   - **问题**: 多进程并发 append 可导致行交错。
   - **建议**: 使用 `fs2` 或 `flock` 在 `append_entry` 时获取排他文件锁。至少文档化"单进程独占 session"为设计约束。
   - **预计工作量**: 1 天

5. **Sidecar 与 JSONL 的一致性校验**
   - **问题**: sidecar 丢失静默退化，用户不知情。
   - **建议**: 在 metalog 或 JSONL header 中添加 sidecar 文件的存在标记和 checksum。在 `automation_counts` 解析失败时发出警告而非静默退化。
   - **预计工作量**: 2-3 天

6. **删除原子性** (`session/mod.rs:171-188`)
   - **问题**: 一侧 sidecar 删除失败导致部分清理。
   - **建议**: 收集所有待删除路径，逐个尝试删除并记录失败项，完成后统一报告。或使用临时目录移动（先 rename 到 .trash 再清理）。
   - **预计工作量**: 1 天

7. **`ActivateTriggers::Ask` 实现** (`session_archive.rs:208-212`)
   - **问题**: 交互式导入被阻塞。
   - **建议**: 通过回调或 channel 将确认请求传递到 UI 层，或至少实现 CLI 的 stdin prompt。
   - **预计工作量**: 2 天

### 🟢 P2 — 低优先级（完善性）

8. **AgentMessage untagged 序列化审查** (`types.rs:120`)
   - **问题**: 自定义消息可能被误反序列化为 LLM 消息。
   - **建议**: 评估是否可将 `Llm(Message)` 的 tag 设为显式标记（如 role 的前缀），或改用 `#[serde(tag = "msg_type")]` 的 tagged 枚举。
   - **预计工作量**: 1 天（+ 回归测试）

9. **Compaction 自动触发集成**
   - **问题**: compaction 依赖显式调用，缺乏自动防线。
   - **建议**: 在 AgentHarness event loop 或 `build_context` 后返回是否需要 compact 的信号，让调用方在发送到模型前主动触发。
   - **预计工作量**: 2-3 天

10. **Branch summarization 预算约束** (`branch_summarization.rs:47-48`)
    - **问题**: 无 prompt_budget 和 max_output_tokens 约束，大分支可导致 overflow。
    - **建议**: 复用 compaction 中的 budget 管理逻辑，添加下限保护。
    - **预计工作量**: 1 天

11. **Import 冲突重试策略**
    - **问题**: UUIDv7 碰撞时导入失败无回退。
    - **建议**: 碰撞时重试生成新 UUID（最多 3 次），避免因极小概率事件导致导入失败。
    - **预计工作量**: 0.5 天

12. **Memory ↔ JSONL 行为一致性测试**
    - **问题**: 两个 backend 的 leaf 计算逻辑不同，可能存在行为差异。
    - **建议**: 引入参数化测试，对 `SessionStorage` trait 的所有方法在两个后端上执行相同的测试用例。
    - **预计工作量**: 2-3 天

### 测试补充建议

| 测试用例 | 覆盖场景 | 优先级 |
|---------|---------|--------|
| 崩溃恢复: 截断最后一行 | JSONL 文件以不完整行结尾 | P0 |
| 超大 session: 10万轮 | 内存/磁盘压力 | P1 |
| 并发 append: 双进程写入 | 行交错检测 | P1 |
| Sidecar 部分损坏: 只保留 triggers | 静默退化告警 | P1 |
| Import 后原子回滚: rename 前崩溃 | 无孤儿文件 | P1 |
| Compaction 后 move_to 旧节点 | 分支路径正确性 | P2 |
| 重复 entry id 运行时插入 | 重复检测 | P2 |
| AgentMessage untagged 歧义消息 | 反序列化正确性 | P2 |

---

## 附录: 审查方法

1. **静态分析**: 阅读所有 session 相关源码文件，追踪数据流和错误传播。
2. **不变式推导**: 从 append-only 约束出发，推导每个模块应满足的不变式，逐一验证。
3. **边缘场景枚举**: 针对并发、崩溃、超大文件、空文件、损坏文件等场景，模拟代码路径。
4. **上游产物交叉验证**: 对比 `03-session-branch-model.md` 的分析，确认结论一致性并补充遗漏项。
