# 精读报告：Loop 标签提取与 Inbox 闭环

> 阅读基线：`f1c35a3` | 深度档位：`maintainer`

---

## 1. problem

### 1.1 背景

`pie` 已有 cron/trigger 能力，可以在定时或事件触发时自动执行一条 prompt。但两个关键缺陷使得自动化难以形成真正闭环：

1. **Amnesiac Cron**：每次 cron 运行都从空白上下文启动，无法知道"上次发生了什么"。用户想实现"告诉我自昨天以来 GitHub 上有什么变化"这类需求时，昨日的数据已随上次运行的上下文一起消失。
2. **无处安放的 Findings**：自动化运行的结果要么注入当前对话打断用户、要么沉入审计日志无人问津，缺少一个"收件箱"作为中间地带。

### 1.2 解决方案

`pie` 的 Loop + Inbox 架构解决这两个问题：

- **Stateful Loop**：给定时任务一个可持久化的"记忆文件"（`loop-<job-id>.md`），每次运行时注入上次笔记，结束时从模型输出中提取新笔记覆盖。
- **Triage Inbox**：全局 JSONL 文件 `~/.pie/inbox.jsonl` 作为 findings 的落点，支持 `new → claimed/dismissed` 生命周期。用户通过 `/inbox claim` 将 finding 提升为主会话中的正式 agent turn。

### 1.3 核心契约

- Loop 从不接触主聊天——stateful cron 走 `TriggerDelivery::SubAgent` 路径，输出仅通过标签协议回流。
- 标签协议是纯文本，任何可以遵循指令的模型都能参与。
- 标签提取失败不会导致运行失败——容忍畸形、截断、缺失。

---

## 2. why_hard

### 2.1 标签协议设计

为什么需要标签协议而不是 API/RPC？

- **跨模型通用性**：不同提供商的模型调用方式不同，但"回复中包含 `<tag>content</tag>`"是纯文本指令，任何模型都能遵循。
- **无 Provider 依赖**：不需要 SDK callback、webhook 或自定义 HTTP handler。
- **截断容错**：模型输出可能在流式传输中被截断（token limit / cost cap），标签可能不完整——协议必须容忍这种情况。

### 2.2 状态文件写入

- **并发写入**：多个 session 可能同时运行 cron job（例如后台 session 也有自己的 cron tick），虽然有 session 级别的 enqueue 防重叠，但状态文件本身没有跨 session 锁。
- **写入时间窗口**：状态文件的读（cron action hook）和写（cron harness listener）在不同的时刻发生，中间跨整次 agent run。
- **字符上限**：2000 字符硬上限，需要 `take(LOOP_STATE_MAX_CHARS).collect()` 截断 + 追加 `…`。

### 2.3 Inbox Triage 设计

- **跨进程写入安全**：多个 `pie` 进程可能同时往同一个 `inbox.jsonl` 追加。JSONL 行级追加天然可交错，但 status 改写（`set_status` / `dismiss_all_new`）需要串行化。
- **读-改-写竞态**：`set_status` 先 `list` 读全文件，再 `rewrite` 全量写回。跨进程场景下两个 `set_status` 可能互相覆盖。
- **损坏行处理**：`list` 通过 `filter_map(serde_json::from_str).ok()` 跳过不可解析行，永不删除它们——数据不丢失。

### 2.4 并发写入与失败恢复

- **in-process write lock**：`WRITE_LOCK: Mutex<()>` 仅保护同一进程内的 append/set_status 操作。
- **跨进程 append**：行级 JSONL append 是 O_APPEND 关键段内的原子操作（POSIX 保证 ≤ PIPE_BUF 的 write 为原子，但 JSONL 行可能超过该阈值）。
- **status rewrite 是 last-writer-wins**：文档明确标注 "acceptable for v1"。
- **状态文件无锁**：loop state `.md` 文件由 `write_loop_state` 直接覆写，跨 session 场景下存在竞态。

---

## 3. design_approach

### 3.1 架构分层

```
[CronNotificationHook]
    │ 每 30s tick，扫描 due_jobs()
    │ 产生 Trigger 信封 → TriggerSink
    ▼
[TriggerRuntime (dedup + cycle)]
    │ evaluate → Accept/Dedup/CycleSuppressed
    ▼
[BeforeTriggerActionHook (cron_action_hook)]
    │ stateful? → SubAgent + compose_stateful_prompt()
    │          → InjectAndRun (普通 cron)
    ▼
[Sub-Agent Run] (独立 context, 不污染主对话)
    │ prompt: [loop-state] 上次笔记 + action + Output Protocol
    ▼
[HarnessEvent::TriggerCompleted] → cron_harness_listener
    │ extract_tag_block("loop-state") → write_loop_state()
    │ extract_tag_all("inbox")        → inbox::append() × N
    ▼
[Inbox JSONL] → /inbox list/claim/dismiss/clear
```

### 3.2 关键设计决策

| 决策 | 理由 |
|------|------|
| 纯文本标签协议 | 跨模型通用、无 provider 依赖 |
| 标签解析失败不失败运行 | 防御性设计，避免因模型输出格式问题丢失一次 cron 运行 |
| 状态 ≤ 2000 字符 | 防止 agent 笔记无限膨胀 |
| Inbox entry ≤ 500 字符 | 防止模型生成超长 findings 占用磁盘 |
| 每 run 最多 16 条 findings | 防止失控 loop 洪水攻击 |
| 全局 JSONL vs 每 session | Inbox 是跨 session 的"早间收件箱" |
| SubAgent vs InjectAndRun | stateful loop 用 SubAgent 保持主对话干净 |
| last-writer-wins status rewrite | v1 接受，未来可改 CAS |

---

## 4. code_walkthrough

### 4.1 `cron_harness_listener` — 标签提取与持久化的入口

**文件**：`crates/coding-agent/src/triggers/cron.rs:901-946`

```rust
pub fn cron_harness_listener(registry: CronRegistry, inbox_path: PathBuf) -> HarnessListener {
    Arc::new(move |event| match event {
        HarnessEvent::TriggerCompleted { trace_id, summary, .. } => {
            // 1. 在 mark_completed 清除 trace 绑定之前解析 job
            let job = registry.job_for_trace(&trace_id);
            registry.mark_completed(&trace_id, None);

            let (Some(job), Some(summary)) = (job, summary) else { return };
            if !job.stateful { return; }

            // 2. 提取 loop-state 标签 → 写状态文件
            if let Some(state) = extract_tag_block(&summary, "loop-state")
                && let Some(sidecar) = registry.storage_path()
            {
                let path = loop_state_path(&sidecar, &job.id);
                if let Err(err) = write_loop_state(&path, &state) {
                    tracing::warn!(error = %err, job = %job.id, "loop state write failed");
                }
            }

            // 3. 提取 inbox 标签 → 追加到 inbox.jsonl
            let source = format!("cron:{}", job.id.chars().take(13).collect::<String>());
            for finding in extract_tag_all(&summary, "inbox", INBOX_TAGS_PER_RUN) {
                if let Err(err) = crate::inbox::append(
                    &inbox_path, &source, &finding, &trace_id, &session_stem
                ) {
                    tracing::warn!(error = %err, "inbox append failed");
                }
            }
        }
        HarnessEvent::TriggerFailed { trace_id, reason } => {
            registry.mark_completed(&trace_id, Some(reason.clone()));
        }
        _ => {}
    })
}
```

**关键逻辑**：

1. **trace → job 解析必须在 `mark_completed` 之前**：`mark_completed` 会清除 `job.running_trace_id`，之后就无法通过 trace_id 反查 job。
2. **非 stateful 直接 return**：普通 cron job 走 `InjectAndRun` 路径，不需要标签提取。
3. **错误静默处理**：loop state 写入失败或 inbox append 失败均不 panic，仅打 `tracing::warn!`。运行本身已成功完成，持久化副产物失败不应影响审计记录。

### 4.2 `extract_tag_block` — 提取最后一个标签块

**文件**：`crates/coding-agent/src/triggers/cron.rs:819-826`

```rust
pub(crate) fn extract_tag_block(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.rfind(&open)?;        // 从右向左找最后一个开标签
    let rest = &text[start + open.len()..];
    let end = rest.find(&close)?;          // 找对应的闭标签
    Some(rest[..end].trim().to_string())
}
```

**设计意图**：使用 `rfind`（从右向左）而非 `find`（从左向右），提取**最后一个**匹配块。这对 `loop-state` 尤其重要——如果模型在输出中多次使用 `<loop-state>`（例如在 reasoning 中转述协议），最终以最后出现的为准。对 `inbox` 则用 `extract_tag_all` 提取全部。

### 4.3 `extract_tag_all` — 提取所有标签块

**文件**：`crates/coding-agent/src/triggers/cron.rs:829-845`

```rust
pub(crate) fn extract_tag_all(text: &str, tag: &str, max: usize) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = text;
    while out.len() < max {
        let Some(start) = rest.find(&open) else { break };
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        let body = after[..end].trim();
        if !body.is_empty() {
            out.push(body.to_string());
        }
        rest = &after[end + close.len()..];
    }
    out
}
```

**关键行为**：

- 从左向右顺序扫描，保持 findings 在模型输出中的原始顺序。
- 跳过 body 为空的标签块（`<inbox></inbox>`）。
- 硬上限 `max`（默认 `INBOX_TAGS_PER_RUN = 16`），防止模型生成数百条标签。
- 未闭合标签（`<inbox>...` 无 `</inbox>`）跳过不报错。

### 4.4 `compose_stateful_prompt` — 构造 Loop 运行提示

**文件**：`crates/coding-agent/src/triggers/cron.rs:773-779`

```
[loop-state] (your notes from the previous run of this recurring job)
<上次运行的笔记内容，或 "(first run)">
[/loop-state]

<用户的 action prompt>

Output protocol (mandatory):
- End your reply with <loop-state>notes for the next run</loop-state> — it REPLACES the saved state; keep it under 2000 characters...
- For each finding a human should act on, emit <inbox>one concise line</inbox>.
- Keep everything after the last tool call short so the tags are not truncated.
```

**设计要点**：

- `[loop-state]…[/loop-state]` 是自然语言包装，让 agent 明确知道"这是你上次的笔记"。
- `(first run)` 作为初始态标记。
- Output Protocol 是强制的——每次运行都会注入这段指令，确保模型知道产出格式。

### 4.5 `cron_action_hook` — 决定运行模式

**文件**：`crates/coding-agent/src/triggers/cron.rs:847-899`

```rust
if job.stateful {
    let state = registry.storage_path()
        .map(|sidecar| loop_state_path(&sidecar, &job.id))
        .and_then(|path| read_loop_state(&path));
    return TriggerAction {
        prompt: compose_stateful_prompt(&job.action, state.as_deref()),
        promote: PromoteAction::None,
        promote_requires_approval: false,
        delivery: TriggerDelivery::SubAgent,     // ← 关键：不注入主对话
    };
}
// 普通 cron: InjectAndRun 直接注入主对话
TriggerAction {
    prompt: job.action,
    delivery: TriggerDelivery::InjectAndRun,     // ← 普通 cron
}
```

Stateful 走 `SubAgent`：独立 context + tag 回流。普通 cron 走 `InjectAndRun`：结果出现在主对话中。

### 4.6 `read_loop_state` / `write_loop_state` — 状态文件 IO

**文件**：`crates/coding-agent/src/triggers/cron.rs:746-769`

- **读**：`std::fs::read_to_string` + `trim()` + 按 `LOOP_STATE_MAX_CHARS (2000)` 截断 + 追加 `…`。
- **写**：`trim()` + 同上截断 + `std::fs::write`。
- **不存在时**：`read_loop_state` 返回 `None`，上游替换为 `"(first run)"`。

### 4.7 `inbox::append` — Inbox 追加

**文件**：`crates/coding-agent/src/inbox.rs:47-85`

```rust
pub fn append(path: &Path, source: &str, text: &str, trace_id: &str, session_id: &str) -> Result<InboxEntry> {
    let trimmed = text.trim();
    let text = if trimmed.chars().count() > MAX_ENTRY_TEXT_CHARS { /* cap + … */ };
    let entry = InboxEntry {
        id: format!("inb-{}", uuid::Uuid::new_v4().simple()),
        created_at: chrono::Utc::now().to_rfc3339(),
        source: source.chars().take(80).collect(),  // 来源截断 80 字符
        text,
        trace_id: trace_id.to_string(),
        session_id: session_id.to_string(),
        status: InboxStatus::New,
    };
    let _guard = WRITE_LOCK.lock();   // 进程内互斥
    // create_dir_all + append
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(entry)
}
```

**关键约束**：

- `MAX_ENTRY_TEXT_CHARS = 500`：单条 finding 上限。
- `source` 截断 80 字符：限制 origin label 长度。
- 行写入：`serde_json::to_string(entry) + "\n"`，确保每行一个完整 JSON 对象。

### 4.8 `inbox::set_status` — 状态变更

**文件**：`crates/coding-agent/src/inbox.rs:115-129`

```rust
pub fn set_status(path: &Path, id: &str, status: InboxStatus) -> Result<Option<InboxEntry>> {
    let _guard = WRITE_LOCK.lock();
    let mut entries = list(path)?;     // 读全文件
    let mut updated = None;
    for entry in &mut entries {
        if entry.id == id {
            entry.status = status.clone();
            updated = Some(entry.clone());
        }
    }
    if updated.is_some() {
        rewrite(path, &entries)?;      // 全量写回
    }
    Ok(updated)
}
```

**注意**：`list` → `rewrite` 是读-改-写操作。`rewrite` 内部将整个 `entries` 序列化后 `std::fs::write` 覆写文件。跨进程场景下存在 last-writer-wins 语义。

### 4.9 `inbox::dismiss_all_new` — 批量清除

**文件**：`crates/coding-agent/src/inbox.rs:132-146`

同样走 `list` → modify in memory → `rewrite` 路径。将全部 `New` 状态条目改为 `Dismissed`。

### 4.10 `InboxCommand` — SLASH 命令处理

**文件**：`crates/coding-agent/src/commands.rs:2492-2621`

| 子命令 | 行为 |
|--------|------|
| `/inbox` (无参) | 列出所有 `New` 条目，显示编号、id 前缀、text、source、时间 |
| `/inbox all` | 列出全部条目（含 claimed/dismissed） |
| `/inbox claim <n>` | 通过 `resolve_inbox_target` 定位 → `set_status(Claimed)` → 返回 `RunAgentPrompt` 启动主对话 agent turn |
| `/inbox dismiss <n>` | `set_status(Dismissed)` |
| `/inbox clear` | `dismiss_all_new()` |

`resolve_inbox_target` 支持两种定位：
- 数字 `n`（1-based）：按 `list_new` 顺序索引。
- id 前缀：`inb-` 开头的 id 前缀匹配。

---

## 5. parsing_edges

### 5.1 标签缺失

**场景**：模型完全没有输出 `<loop-state>` 或 `<inbox>` 标签。

**行为**：
- `extract_tag_block` 返回 `None` → 状态文件不更新，保留上次状态。
- `extract_tag_all` 返回空 `Vec` → 无 findings 进入 inbox，这是"静默运行"。
- 运行本身标记为成功完成。

### 5.2 标签截断

**场景**：模型输出在流式传输中达到 token limit，标签被截断。例如 `"x <loop-state>cut off"`。

**行为**：
- `extract_tag_block` 的 `rfind(open)` 找到 `<loop-state>`，但 `rest.find(close)` 找不到 `</loop-state>` → 返回 `None`。
- 静默失败，不报错。

**对应测试**：`cron.rs:1428-1431`
```rust
assert_eq!(extract_tag_block("x <loop-state>cut off", "loop-state"), None);
```

### 5.3 标签重复

**场景**：模型在不同位置多次输出 `<loop-state>` 或 `<inbox>`。

**行为**：
- `loop-state`：`extract_tag_block` 使用 `rfind` 取最后一个块，确保获取最终版本。
- `inbox`：`extract_tag_all` 收集全部块，上限 16 条。

### 5.4 标签嵌套

**场景**：模型在 `<inbox>` 内部使用了 `<loop-state>` 或反之，或者标签内嵌同名标签。

**行为**：`find(&close)` 会匹配到第一个 `</tag>`。如果模型在 body 中嵌套了标签，可能导致过早截断。但文档要求 findings 是 "one concise line"，实际使用中不太会出现。

### 5.5 空 body

**场景**：`<inbox></inbox>` 或 `<inbox>  </inbox>`。

**行为**：`body.trim().is_empty()` → 跳过，不加入结果集。

### 5.6 Inbox text 超长

**场景**：模型生成超过 500 字符的 finding。

**行为**：按字符边界截断前 500 字符，末尾追加 `…` 标记。

### 5.7 多条 Inbox 超过上限

**场景**：模型输出超过 16 条 `<inbox>` 标签。

**行为**：`extract_tag_all` 在 `out.len() < max` 条件下停止扫描，只保留前 16 条。

### 5.8 模型输出中的"污染"

**场景**：模型在非标签区域输出了看起来像标签的文本（例如在代码块中讨论 `<inbox>` 协议本身）。

**行为**：`extract_tag_all` 会将其作为正式 finding 提取。这是设计限制——纯文本协议无法区分"模型想表达一个 finding"和"模型在讨论协议语法"。可以通过 Prompt 工程减轻（要求标签只出现在回复末尾）。

### 5.9 UI 展示的标签剥离

**文件**：`crates/coding-agent/src/triggers/cron.rs:783-816`

`strip_loop_protocol_tags` 移除 `<loop-state>` 和 `<inbox>` 标签块，使 UI feed 中不显示协议格式。移除后的空白行会被折叠。

---

## 6. concurrency

### 6.1 进程内互斥锁

```rust
static WRITE_LOCK: Mutex<()> = Mutex::new(());
```

所有 inbox 写操作（`append`、`set_status`、`dismiss_all_new`）都持有此锁。防止同一进程内多个 loop 同时写入导致行交错。

### 6.2 跨进程 append 安全性

JSONL 格式天然支持行级交错：

```
{"id":"inb-aaa","text":"finding A"...}\n
{"id":"inb-bbb","text":"finding B"...}\n
```

- 进程 A 的 `file.write_all(line_a)` 和进程 B 的 `file.write_all(line_b)` 如果同时发生，结果文件包含两条完整行，交错仅发生在行级别。
- POSIX 保证 ≤ `PIPE_BUF` (通常 512-4096 字节) 的 `write` 是原子的。单条 inbox JSON 通常在 ~200 字节，远小于此阈值，因此实际场景中行不会破损。

### 6.3 跨进程 status rewrite 的安全隐患

`set_status` 和 `dismiss_all_new` 走 `list → rewrite` 路径：

```
Process A: list() → [entry1, entry2] → set entry1 → rewrite([entry1', entry2])
Process B: list() → [entry1, entry2] → set entry2 → rewrite([entry1, entry2'])
```

如果两个操作交叉执行，最终文件只保留后执行者的状态变更，先执行者的变更丢失。文档明确标注 **"acceptable for v1"**。

### 6.4 Loop State 文件并发

`write_loop_state` 使用 `std::fs::write` 直接覆写。跨 session 场景下存在：
- **竞态写**：两个 session 同时完成同一个 cron job 的运行（虽然 session-scope 的 `running_trace_id` 已在 due_jobs 阶段防重叠，但重启/多实例可能产生竞争）。
- **最后写入胜出**：无锁，两个 write 之间可能互相覆盖。

### 6.5 安全分析总结

| 操作 | 进程内 | 跨进程 |
|------|--------|--------|
| `inbox::append` | Mutex 保护 | JSONL 行级安全 |
| `inbox::set_status` | Mutex 保护 | last-writer-wins |
| `inbox::dismiss_all_new` | Mutex 保护 | last-writer-wins |
| `write_loop_state` | 无锁（每个 job 独立文件） | 无锁 |
| `cron_repository::due_jobs` | Mutex (`inner.lock()`) | 无锁（TOML 文件本身无并发保护） |

---

## 7. tests

### 7.1 Unit Tests（`cron.rs` 内部 `#[cfg(test)] mod tests`）

| 测试函数 | 覆盖内容 |
|----------|----------|
| `tag_extraction_handles_present_absent_truncated_and_caps` | `extract_tag_block` 存在/缺失/截断/等 + `extract_tag_all` 存在/16条上限 |
| `stateful_prompt_injects_previous_state_and_protocol` | `compose_stateful_prompt` 注入上次状态、包含协议指令、首次运行标记 `(first run)` |
| `loop_state_paths_and_write_cap` | `loop_state_path` 路径拼接 + `write_loop_state` 截断 2000 字符 + `read_loop_state` 不存在文件返回 None |
| `listener_persists_state_and_inbox_for_stateful_job_completion` | 端到端：cron job → due → TriggerCompleted event → listener 提取 loop-state 写入 `.md` + 提取 inbox 追加到 `.jsonl` + job 标记 completed |
| `due_jobs_tick_writes_sidecar_only_when_state_changed` | 空载 tick 不创建 cron.toml 文件 |
| `load_clears_stale_running_state_from_previous_process` | 重启时清除残留的 `running_trace_id` |
| `registry_rejects_oversized_action` | action 超过 `MAX_ACTION_BYTES` 被拒绝 |
| `trigger_summary_redacts_secret_like_action_text` | cron trigger payload summary 自动脱敏 API key / bearer token |
| `due_jobs_marks_running_and_skips_overlap` | 运行中的 job 再次到期时跳过并增加 `skipped_overlap_count` |
| `listener_clears_running_job_by_trace_id` | TriggerCompleted 后 `running_trace_id` 被清除 |
| `cron_action_hook_maps_cron_trigger_to_inject_and_run` | 普通 cron trigger 映射为 `InjectAndRun` |

### 7.2 Unit Tests（`inbox.rs` 内部 `#[cfg(test)] mod tests`）

| 测试函数 | 覆盖内容 |
|----------|----------|
| `append_list_claim_dismiss_round_trip` | append → list_new → set_status(Claimed) → new_count 递减 → dismiss_all_new → new_count=0 → 历史条目仍可通过 list 查看 |
| `oversized_text_is_capped_and_corrupt_lines_skipped` | 2000 字符被截断至 ≤ 501 + 损坏行被跳过不丢失数据 |

### 7.3 Integration Tests（`tests/commands.rs`）

| 测试函数 | 覆盖内容 |
|----------|----------|
| `dispatch_cron_add_lists_toggles_and_removes_job` | `/cron add` → `/cron list` → `/cron disable` → `/cron enable` → `/cron remove` 全生命周期，含 audit 记录校验 |
| `dispatch_cron_list_redacts_secret_like_action_preview` | cron list 输出脱敏 API key |
| `dispatch_cron_add_audit_redacts_secret_like_action_preview` | cron add 审计记录脱敏 |

### 7.4 测试中的 Inbox 路径隔离

Inbox 测试使用 `tempfile::tempdir()` 创建临时目录，`inbox_path = dir.path().join("inbox.jsonl")`，不会写入真实 `~/.pie/inbox.jsonl`。

---

## 8. risks

### 8.1 生产值班自动化风险

| 风险 | 等级 | 描述 |
|------|------|------|
| **标签协议不可靠** | 中 | 模型可能不遵循 Output Protocol。缺失标签时静默跳过，状态不回滚、findings 不入 inbox。值班人员可能漏掉关键告警。 |
| **截断丢失** | 中 | Token limit 截断模型输出在标签中间，`extract_tag_block` 返回 None。状态文件不更新，上次笔记作为下次上下文——可能已过时。 |
| **跨进程 inbox 写覆盖** | 低 | `set_status` 的读-改-写是 last-writer-wins。如果用户在两个终端同时 `/inbox claim` 不同条目，可能有一个操作的 status change 丢失。但实际生产中多个终端同时操作 inbox 的概率很低。 |
| **cron job 状态泄漏** | 低 | `running_trace_id` 在进程崩溃后残留，重启时 `clear_stale_running_state` 会清除。但如果 crash 发生时 job 刚完成标签提取但未 mark_completed，下次重启会误判为 stale 并清除。 |
| **无 Maker/Checker** | 高 | 当前设计缺少"审查环节"——模型生成的 findings 直接进入 inbox，没有第二层验证。一个配置错误的 loop 可能持续产出垃圾 findings。文档标注 Phase 3 将引入可选的 maker/checker 验证。 |
| **状态文件无版本控制** | 低 | Loop state 是纯 Markdown 文件，可手动编辑。但如果 agent 写出格式错误的笔记，下次运行注入错误上下文，可能导致 loop 偏离预期。 |
| **Inbox 无限增长** | 低 | 虽然有每 run 16 条上限，但没有基于时间的自动清理。长期运行可能导致 `inbox.jsonl` 变得很大。当前通过损坏行跳过 + 读全量工作，大文件可能影响 `/inbox` 响应速度。 |

### 8.2 缓解建议

- 尽快实现 Maker/Checker（Phase 3）。
- 为 `/inbox` 添加分页或时间窗口过滤。
- 增加 cron job 运行健康指标（连续缺失标签次数、inbox 产出率）暴露给监控。
- 考虑为 loop state 添加写入前校验（非空、格式检查）。

---

## 9. next_questions

1. **Maker/Checker Phase 3 的具体设计如何？** 是否有对 findings 的自动评估（例如再调用一次 LLM 判断 finding 是否值得进入 inbox）？评估标准如何定义？

2. **Inbox 的持久化规模上限策略？** 目前 append-only JSONL + 无自动清理。是否需要基于时间的裁剪策略（例如保留最近 30 天）或基于条目数的上限？

3. **Loop State 的跨 session 一致性如何保证？** 如果两个不同 session 中有相同的 cron schedule 且都 `stateful`，两个状态文件如何同步？当前设计似乎依赖 session scope 隔离。

4. **标签协议的演进方向？** 未来是否会考虑更结构化的输出契约（例如要求模型输出 JSON），还是坚持纯文本协议作为最低公分母？

5. **Inbox 的 Web/Mobile 消费路径？** 文档提到 "web inbox panel with claim/dismiss buttons" 尚未构建。Web relay 场景下的 inbox 同步策略是什么？

6. **Loop 的执行性能与模型成本？** 每次 stateful cron 运行都是独立的 SubAgent 调用（完整模型推理 + 工具执行）。是否有计划支持"轻量级 loop"（例如只解析无工具调用的纯推理）来降低频繁 loop 的成本？

7. **触发来源的扩展性？** 当前 inbox 的 `source` 字段硬编码为 `"cron:<job-id-prefix>"`。如果未来 event-based trigger 也产出入 inbox 的 findings，如何扩展 source 命名空间？
