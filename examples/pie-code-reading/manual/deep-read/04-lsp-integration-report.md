# LSP Supervisor 集成技术报告

**仓库**: `pie` | **基线**: `f1c35a3` | **深度档位**: maintainer | **日期**: 2026-06-13

---

## 1. Problem

LSP supervisor 在 `pie` coding agent 里解决的核心问题是：**在 AI agent 自动执行 write/edit 工具修改文件后，agent 无法感知它引入的语法/类型错误**。传统 IDE 中的实时诊断（红色波浪线）完全依赖人类开发者的目视反馈，而 `pie` 作为一个自主 coding agent，在写完代码后没有机制检查"我写对了吗？"。

LSP supervisor 将 Language Server Protocol (LSP) 的诊断能力注入到 agent 的工具调用闭环中，使得：

- `write` 或 `edit` 工具修改文件后，自动从对应语言的 LSP server 获取 `publishDiagnostics` 通知
- 诊断结果（错误、警告、提示）以文本形式追加到工具结果中，随下一轮 LLM 上下文一起返回
- LLM 可以基于这些诊断进行自我修复（self-correction），提升代码生成质量

简言之：**把 IDE 的红色波浪线放进 agent 的上下文窗口**。

## 2. Why Hard

LSP 集成看似简单——"启动一个 server，订阅诊断通知"——但实际有多个非平凡挑战：

### 2.1 启动延迟
LSP server（如 `rust-analyzer`、`typescript-language-server`）启动时需要进行项目索引、依赖解析、类型检查等初始化工作，耗时从数秒到数十秒不等。如果 agent 写完文件后立即查询诊断，往往只能拿到空的或不完整的结果。因此需要**异步等待 + 超时回退**策略。

### 2.2 语言探测
LSP server 的选择依赖于文件扩展名到语言 ID 的映射（`.rs` → `rust`, `.ts` → `typescript`）。这种映射需要可配置，因为：
- 不同项目使用不同的 LSP server（如 `typescript-language-server` vs `deno lsp`）
- 同一个扩展名可能绑定不同的语言
- 用户可能不想为所有语言都启用 LSP

### 2.3 诊断时机
`publishDiagnostics` 是 LSP server 主动推送的**通知（notification）**，而非请求-响应模式。agent 无法"查询当前诊断"，只能被动等待推送。在 `didOpen`/文件写入后，server 需要时间分析并推送，而 agent 不能无限等待。这就产生了"等多久？"的超时权衡。

### 2.4 工具写入并发
Agent 在一次 turn 中可能并行调用多个工具（`ToolExecutionMode::Parallel`），其中多个 write/edit 可能修改同一个文件。如果 naive 地对每个工具结果都做一次 LSP 查询，可能导致：
- 对同一文件重复 `didOpen`
- 诊断竞争（两次推送可能相互覆盖）
- 不必要的 server 负载

### 2.5 性能和噪声
- **性能**：每个 write/edit 都等待 DIAG_WAIT_MS (800ms) 会显著增加 turn 延迟，尤其在批量修改时
- **噪声**：LSP 诊断可能包含大量预存在的错误（agent 修改前的代码问题）、false positive、或无关的 lint hints。将这些全部注入 LLM 上下文可能使其困扰而非帮助

## 3. Design Approach

`pie` 的 LSP 接入采用 **最小化、懒加载、配置驱动** 的设计思路：

### 3.1 架构分层

```
┌────────────────────────────────────────────┐
│  LspSupervisor (lsp_supervisor.rs)         │
│  - 管理多个 LspClient 实例（按语言ID）       │
│  - 懒加载：首次使用某语言时才启动 server      │
│  - 暴露 as_after_tool_call() 挂钩           │
│  - 配置源：~/.pie/lsp.toml + .pie/lsp.toml  │
└──────────────┬─────────────────────────────┘
               │ 持有 Arc<LspClient>
┌──────────────▼─────────────────────────────┐
│  LspClient (lsp.rs)                        │
│  - 子进程传输（stdio）                       │
│  - JSON-RPC 2.0 (Content-Length 帧)         │
│  - initialize / didOpen / await_diagnostics  │
│  - 异步诊断收集 + 缓存                       │
└────────────────────────────────────────────┘
```

### 3.2 与 Agent Loop 的集成方式

LSP supervisor 通过 **`after_tool_call` hook** 挂载到 agent loop 中。这是 `pie-agent-core` 提供的标准扩展点（定义在 `crates/agent/src/types.rs:621-628`）：

```rust
pub type AfterToolCallHook = Arc<
    dyn Fn(AfterToolCallContext, CancellationToken)
        -> Pin<Box<dyn Future<Output = AfterToolCallResult> + Send>>
    + Send + Sync,
>;
```

Hook 在 agent loop 的 `execute_tool_calls` 函数中调用（`agent_loop.rs:648`），位于工具执行完成之后、`ToolExecutionEnd` 事件发射之前。Hook 返回的 `AfterToolCallResult` 可以对工具结果进行增量修改（替换 content、覆盖 details、修改 is_error/terminate）。

### 3.3 关键设计决策

| 决策 | 理由 |
|---|---|
| 仅对 `write`/`edit` 工具生效，`bash` 等其它工具忽略 | 只有文件修改工具才需要 LSP 反馈 |
| 懒加载 LSP server | 未配置某语言或从未触碰该语言的文件，不产生启动成本 |
| `OnceCell` 缓存 server 实例 | 同一语言的多次调用共享一个 server 进程 |
| `didOpen` 去重 | 对同一文件只发一次 `didOpen`，避免 server 重复处理 |
| 固定 800ms 诊断等待 | 折中：够大多数 LSP 推送，又不显著拖慢工具往返 |
| 回退到缓存诊断 | 等待超时后返回已有诊断，确保 agent 至少看到部分信息 |
| 仅保留最后 20 条诊断 | 避免上下文溢出，保持 LLM 提示紧凑 |

## 4. Code Walkthrough

### 4.1 LspClient (`crates/coding-agent/src/lsp.rs`, 308 行)

**结构** (`lsp.rs:54-63`):
```rust
pub struct LspClient {
    stdin: AsyncMutex<ChildStdin>,                    // 写管道
    next_id: AtomicU64,                                // JSON-RPC 请求 id
    inflight: Arc<Mutex<HashMap<u64, oneshot::Sender>>>, // 等待中的请求响应
    diagnostics: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>, // URI → 诊断缓存
    diag_tx: mpsc::UnboundedSender<(String, Vec<Diagnostic>)>, // 诊断事件流发送端
    diag_rx: AsyncMutex<mpsc::UnboundedReceiver<...>>, // 诊断事件流接收端
    child: AsyncMutex<Option<Child>>,                  // 服务器子进程
    request_timeout: Duration,                         // 请求超时（15 秒）
}
```

**子进程传输** (`lsp.rs:69-160`): `spawn()` 方法通过 `tokio::process::Command` 启动 LSP 服务器，接管其 stdin/stdout/stderr。stdout 通过一个独立的后台 task 持续读取 JSON-RPC 帧，stderr 由另一个后台 task 静默消耗（避免管道缓冲区满阻塞子进程）。

**JSON-RPC 帧解析** (`lsp.rs:283-308`): `read_framed()` 实现标准 LSP `Content-Length` 头解析：
1. 逐行读取头信息直到空行
2. 提取 `Content-Length: N`
3. 精确读取 N 字节作为 JSON body
4. 反序列化后返回

**诊断推送处理** (`lsp.rs:118-130`): 读泵 task 检测 `method == "textDocument/publishDiagnostics"` 消息，将诊断写入两个位置：
- `diagnostics` HashMap（按 URI 缓存的最近诊断，供查询）
- `diag_tx` channel（事件流，供 `await_diagnostics()` 等待）

**initialize 握手** (`lsp.rs:163-179`): 发送标准 LSP initialize 请求，声明 `textDocument.publishDiagnostics` 能力和 `didSave` 同步能力，随后发送 `initialized` 通知。

**didOpen 通知** (`lsp.rs:183-193`): 向 server 告知文件已打开，包含 URI、languageId 和完整文本内容。这是触发 server 分析的入口。

**await_diagnostics** (`lsp.rs:206-212`): 从 `diag_rx` channel 等待下一次诊断推送，带超时防护。超时返回 `None`，调用方回退到 `diagnostics_for()` 取缓存。

### 4.2 LspSupervisor (`crates/coding-agent/src/lsp_supervisor.rs`, 235 行)

**配置模型** (`lsp_supervisor.rs:27-42`):
```rust
pub struct LspConfig {
    pub language: Vec<LanguageConfig>,
}

pub struct LanguageConfig {
    pub id: String,           // "rust", "typescript"
    pub extensions: Vec<String>, // ["rs"], ["ts", "tsx"]
    pub command: String,      // "rust-analyzer"
    pub args: Vec<String>,    // []
}
```

**配置文件加载** (`lsp_supervisor.rs:78-104`): `load()` 方法从两个位置加载 TOML 配置：
1. `~/.pie/lsp.toml` (用户全局配置)
2. `<cwd>/.pie/lsp.toml` (项目级配置)

项目级配置会覆盖同名 language id 的全局配置，简单字段级替换，不深度合并。

**扩展名索引** (`lsp_supervisor.rs:52-73`): `from_config()` 将 `LanguageConfig` 展平为 `by_ext: HashMap<String, LanguageConfig>`，以扩展名为 key。例如 `extensions: ["ts", "tsx"]` 会分别插入 `"ts"` 和 `"tsx"` 两个条目，都指向同一个 `LanguageConfig`。

**懒加载 client** (`lsp_supervisor.rs:117-142`): `client_for_ext()` 方法：
1. 根据扩展名查找 `LanguageConfig`
2. 检查 `clients` HashMap 中是否已有该语言的 `OnceCell`
3. 若无，创建新 `OnceCell`，首次调用时通过 `get_or_try_init` 异步执行 `spawn → initialize` 流程
4. 一旦初始化完成，后续调用直接返回已缓存的 `Arc<LspClient>`

这种设计确保了：同语言的所有文件修改共享一个 LSP 进程；启动失败（server 未安装或崩溃）不会 panic，返回 `Option`。

**文件打开去重** (`lsp_supervisor.rs:146-158`): `ensure_open()` 方法：
1. 获取文件的 LSP client
2. 检查 `open_files` 集合中是否已有该 URI
3. 若无，读取文件内容并调用 `did_open`
4. 将 URI 插入 `open_files` 防止对同一文件重复 didOpen

**核心 hook 逻辑** (`lsp_supervisor.rs:173-212`): `attach_diagnostics()` 函数：

```
1. 若 supervisor 为空（无配置），直接返回默认结果
2. 只处理 tool_name == "write" 或 "edit"
3. 从 tool args 中提取 path 参数
4. ensure_open(path) → 触发 didOpen（若需要）+ 获取 LspClient
5. await_diagnostics(800ms) → 等待 server 推送最新诊断
6. 超时回退：diagnostics_for(uri) → 取缓存中最近诊断
7. 若有诊断，渲染为文本并追加到 tool result content
```

**诊断渲染** (`lsp_supervisor.rs:214-235`): `render_diagnostics()` 将诊断列表格式化为：
```
LSP diagnostics for /path/to/file:
  [error] 4:1: expected `;`, found `}`
  [warning] 10:5: unused variable `x`
```
最多渲染 20 条，超出部分显示 `(N more)`。

### 4.3 与 Agent Loop 的类型约定

**AfterToolCallContext** (`types.rs:502-510`):
```rust
pub struct AfterToolCallContext {
    pub assistant_message: AssistantMessage,  // 触发工具的 assistant 消息
    pub tool_call: ToolCall,                   // 当前工具调用
    pub args: serde_json::Value,               // 工具参数
    pub result: AgentToolResult,               // 工具执行结果
    pub is_error: bool,                        // 工具是否返回了错误
    pub context: AgentContext,                 // 当前 agent 上下文
}
```

**AfterToolCallResult** (`types.rs:483-489`):
```rust
pub struct AfterToolCallResult {
    pub content: Option<Vec<UserContentBlock>>,  // 替换 tool result content
    pub details: Option<serde_json::Value>,       // 覆盖 details
    pub is_error: Option<bool>,                   // 覆盖 is_error
    pub terminate: Option<bool>,                  // 覆盖 terminate
}
```
Hook 返回 `None` 的字段保持原值不变，实现部分覆盖语义。

### 4.4 上下文传递链路

**agent_loop.rs:648-675** 中的 hook 调用：
```rust
let patch = hook(ctx, cancel.clone()).await;
if let Some(content) = patch.content {
    result.content = content;    // LSP hook 替换 content
}
// ... 类似处理 details, is_error, terminate
```

重点：`patch.content` 会**完全替换**原始 tool result 的 content，而非追加。因此 LSP hook 在 `attach_diagnostics` 中做了 `content.push(diagnostic_text)`，将原始内容和诊断文本合并后再返回。

### 4.5 main.rs 中的集成点 (`crates/coding-agent/src/main.rs:780-786`)

```rust
let lsp_supervisor = Arc::new(LspSupervisor::load(&cwd).await);
let lsp_lang_count = lsp_supervisor.language_count();
if !lsp_supervisor.is_empty() {
    opts.after_tool_call = Some(lsp_supervisor::as_after_tool_call(lsp_supervisor.clone()));
}
```

关键点：
- 在 harness options 构建阶段加载 LSP 配置（异步读取两个 TOML 文件）
- 仅在配置非空时才注册 `after_tool_call` hook
- `as_after_tool_call()` 将 `Arc<LspSupervisor>` 包装为符合 `AfterToolCallHook` trait object 的异步闭包
- Hook 被 `AgentHarnessOptions` 内化后，通过 `agent_loop.rs` 的 `inner.options.after_tool_call` 在每个工具执行后自动调用
- 语言数量在启动 banner 中展示：`lsp: N language(s) configured; diagnostics attach to edit/write results`

### 4.6 Edit/Write 工具 (`edit.rs`, `write.rs`)

两者都是无状态的 `AgentTool` 实现，不感知 LSP：

- **WriteTool** (`write.rs`): 接收 `path` + `content`，创建父目录，全量覆盖写入文件。返回内容为 `"Wrote N bytes (M lines) to /path"`。
- **EditTool** (`edit.rs`): 接收 `path` + `old_string` + `new_string` [+ `replace_all`]。核心逻辑：
  1. 读取文件内容
  2. 精确匹配 `old_string`（要求唯一，除非 `replace_all=true`）
  3. 替换后写回
  4. 返回包含 `-/+` diff 预览的文本结果

LSP 反馈对它们**完全透明**——工具本身不知道 LSP 的存在，诊断是由 hook 在工具返回后、结果进入 LLM 上下文前注入的。

### 4.7 Bash 工具 (`bash.rs`)

Bash 工具不触发 LSP 反馈（`attach_diagnostics` 只匹配 `write` 和 `edit`），但值得关注的是其设计中的**子进程管理**模式，这与 `LspClient` 的 `spawn` 有相似之处：
- `setsid` + `killpg` 确保超时/取消时整个进程树被清理（6.2 节中的教训）
- stdout 和 stderr 并发 drain 防止管道死锁
- `kill_on_drop(true)` 作为兜底保护

## 5. Integration Timeline

以下是从 agent 执行工具到 LSP 诊断注入 LLM 上下文的完整时序：

```
Time ─────────────────────────────────────────────────────────►

User                   Agent Loop              LspSupervisor        LspClient          LSP Server
 │                         │                        │                    │                  │
 │  "edit foo.rs"  ────►   │                        │                    │                  │
 │                         │                        │                    │                  │
 │                    [Turn Start]                  │                    │                  │
 │                         │                        │                    │                  │
 │                    [LLM generates                │                    │                  │
 │                     tool call]                   │                    │                  │
 │                         │                        │                    │                  │
 │                    [before_tool_call hook]        │                    │                  │
 │                         │                        │                    │                  │
 │                    [Execute EditTool]             │                    │                  │
 │                         │                        │                    │                  │
 │                    edit.rs: write                │                    │                  │
 │                    新内容 → foo.rs               │                    │                  │
 │                         │                        │                    │                  │
 │                    [after_tool_call hook]         │                    │                  │
 │                         │                        │                    │                  │
 │                    ──── as_after_tool_call() ──►  │                    │                  │
 │                         │                        │                    │                  │
 │                         │                  attach_diagnostics()       │                  │
 │                         │                        │                    │                  │
 │                         │                  ① check tool_name          │                  │
 │                         │                     == "edit" ✓             │                  │
 │                         │                        │                    │                  │
 │                         │                  ② extract "path"           │                  │
 │                         │                        │                    │                  │
 │                         │                  ③ ensure_open(foo.rs)      │                  │
 │                         │                        │                    │                  │
 │                         │                   检查 open_files:           │                  │
 │                         │                   URI 不存在 → 首次打开      │                  │
 │                         │                        │                    │                  │
 │                         │               client_for_ext("rs")          │                  │
 │                         │                        │                    │                  │
 │                         │                   检查 OnceCell:             │                  │
 │                         │                   未初始化 →                │                  │
 │                         │                   spawn+initialize ───────► │                  │
 │                         │                        │                    │                  │
 │                         │                        │        ─── spawn rust-analyzer ──►
 │                         │                        │                    │                  │
 │                         │                        │        ─── initialize(uri) ──────►
 │                         │                        │                    │    ◄─────────────
 │                         │                        │        ─── initialized ──────────►
 │                         │                        │                    │                  │
 │                         │                        │   OnceCell 写入 ✓   │                  │
 │                         │                        │                    │                  │
 │                         │                  ④ did_open(foo.rs) ──────► │                  │
 │                         │                        │                    │    ────────────►
 │                         │                        │                    │    textDocument/
 │                         │                        │                    │    didOpen
 │                         │                        │                    │                  │
 │                         │                  ⑤ 标记 open_files          │                  │
 │                         │                        │                    │                  │
 │                         │                  ⑥ await_diagnostics(800ms) │                  │
 │                         │                        │                    │                  │
 │                         │                        │                    │     等待推送...    │
 │                         │                        │                    │                  │
 │                         │                        │                    │    ◄── publishDiag
 │                         │                        │                    │   (errors/warns)
 │                         │                        │                    │                  │
 │                         │                        │                    │  更新 cache
 │                         │                        │                    │  发送 diag_tx
 │                         │                        │                    │                  │
 │                         │                        │        ◄── diag_rx.recv() ──────────
 │                         │                        │                    │                  │
 │                         │                  ⑦ 超时回退机制             │                  │
 │                         │                     若 800ms 内无推送 →     │                  │
 │                         │                     diagnostics_for(uri)     │                  │
 │                         │                     取缓存中的最近诊断       │                  │
 │                         │                        │                    │                  │
 │                         │                  ⑧ render_diagnostics()     │                  │
 │                         │                     诊断列表 → 文本         │                  │
 │                         │                        │                    │                  │
 │                         │                  ⑨ content.push(diag_text)  │                  │
 │                         │                        │                    │                  │
 │                    ◄── AfterToolCallResult ───────                     │                  │
 │                         │                        │                    │                  │
 │                    [content 已含诊断]             │                    │                  │
 │                         │                        │                    │                  │
 │                    [Emit ToolExecutionEnd]        │                    │                  │
 │                         │                        │                    │                  │
 │                    [将 tool result +             │                    │                  │
 │                     LSP 诊断一起                  │                    │                  │
 │                     追加到 LLM context]          │                    │                  │
 │                         │                        │                    │                  │
 │                    [Next LLM call                │                    │                  │
 │                     看到诊断 → 修复代码]          │                    │                  │
```

关键时序约束：
- **首次调用**：对某种语言的第一个 write/edit，LSP server 启动 + initialize 可能耗时 2-10 秒，在 `ensure_open()` 的 `client_for_ext()` 调用中完成
- **后续调用**：OnceCell 已有缓存，直接进入 didOpen → 等待诊断
- **同文件重复修改**：`open_files` 去重避免冗余 didOpen
- **并行工具调用**：多个不同文件同时 edit 时，每个文件独立走 `ensure_open → await_diagnostics`，共享同一个 LspClient

## 6. Performance

### 6.1 启动成本

| 阶段 | 成本 |
|---|---|
| TOML 配置读取 | ~1ms（两个文件的异步 I/O） |
| LSP server spawn | 10-50ms（进程创建） |
| LSP server initialize | 2-10s（`rust-analyzer` 项目索引）；0.5-2s（`typescript-language-server`） |
| OnceCell 缓存 | 首次完成后 = 0ms（直接返回 Arc 引用） |

首次写 RS 文件的端到端延迟 ≈ **spawn + init 延迟 + 800ms diag 等待**。

### 6.2 每次诊断开销

| 操作 | 耗时 |
|---|---|
| 扩展名查找 (HashMap::get) | <1μs |
| OnceCell::get_or_try_init (cached) | <1μs |
| open_files 检查 (HashMap::contains_key) | <1μs |
| didOpen（后续调用）| 0（去重跳过） |
| await_diagnostics(800ms) | 0-800ms（取决于 LSP 推送速度） |
| diagnostics_for (HashMap::get) | <1μs |
| 诊断文本渲染 | <1ms |

### 6.3 诊断等待策略的权衡

当前固定 800ms 等待是一个**保守但不够聪明的策略**：

- **优点**：简单、无状态、不会遗漏慢速 LSP 的推送
- **缺点**：
  - 即使 LSP 在 10ms 内推送了诊断，仍然等到 800ms 超时结束（因为 `diag_rx.recv()` 会立即返回，`tokio::time::timeout` 在数据到达时立即完成 — **实际上这是错的**。重新审阅代码：`await_diagnostics` 使用 `tokio::time::timeout(timeout, rx.recv()).await`，当 channel 有数据到达时立即返回，不会等待整个 timeout。所以实际等待时间是 min(800ms, LSP 响应时间)，这是正确的）
  - 800ms 上限意味着极慢的 LSP（如大型项目首次分析）会在未完成时回退到空缓存
  - 对批量修改中的每个文件分别等待 800ms，串行累加

### 6.4 缓存策略

`diagnostics` HashMap 按 URI 存储最近一次收到的诊断集合，用 `Mutex<HashMap<...>>` 保护。优点：
- 无额外内存开销（每个 URI 一组诊断，通常 <1KB）
- 零延迟读取（内存 HashMap）
- 自动被新推送覆盖（无内存泄漏）

### 6.5 可能的优化方向

1. **增量优化**：对已打开的文件，发送 `textDocument/didChange` 而非 `didOpen`（需要维护文件版本号）
2. **自适应超时**：根据 LSP server 类型调整等待时间（如 `rust-analyzer` 的 cargo check 通常 100-300ms）
3. **智能噪声过滤**：仅显示本次修改引入的新诊断（需要前后 diff 诊断集合）
4. **批量合并**：同一 turn 中对同文件多次修改，合并为一次 LSP 查询
5. **主动取消**：利用 CancellationToken，当 agent turn 被取消时立即中断诊断等待

## 7. Tests

### 7.1 LSP Frame 测试 (`tests/lsp_framing.rs`, 91 行)

**`lsp_client_round_trips_initialize_and_receives_diagnostics`**: 唯一真正的端到端 LSP 测试，通过 Python mock LSP server 验证：
- `spawn` → 创建子进程，连接 stdio
- `initialize` → 发送 handshake 并解析响应
- `await_diagnostics` → 等待 mock server 推送的 `publishDiagnostics`
- 诊断数据正确性：`uri == "file:///tmp/x.rs"`, `severity == 1`, `message.contains("expected")`, `range.start.line == 3`
- `shutdown` → 发送 shutdown + exit

**跳过条件**：需 Python3 在 PATH 中，否则测试跳过

### 7.2 Edit 工具单元测试 (`edit.rs:132-193`)

- `replaces_unique_substring`: 基本替换流程
- `rejects_ambiguous_match`: 多匹配拒绝（不传 replace_all）
- `replace_all_handles_multiple`: replace_all 模式正确替换所有匹配

### 7.3 Bash 工具进程管理测试 (`bash.rs:314-573`)

与此报告间接相关，展示了完善的子进程管理测试：
- `timeout_kills_child_process`: 超时后 `pgrep` 验证进程已终止
- `timeout_kills_descendant_processes`: 通过 `(sleep 60) & wait` 验证后代进程也被清理
- `cancellation_kills_child_process`: CancellationToken 触发后进程清理
- `high_volume_stderr_does_not_deadlock_stdout`: 并发 drain 防止死锁

### 7.4 缺失的测试覆盖

当前 LSP 集成**缺少**以下测试：
- `LspSupervisor` 的单元测试（配置加载、懒加载、打开去重）
- 集成测试：Harness + LspSupervisor hook → 验证诊断文本注入 tool result
- 超时回退场景（LSP server 未推送诊断时的降级行为）
- 并发写入同一文件的诊断竞争
- 空配置 / 缺失 server 时的降级行为

## 8. Risks

### 8.1 已识别风险

1. **LSP server 未安装** (`lsp_supervisor.rs:138-141`): `spawn()` 失败时，`client_for_ext` 返回 `None`，`ensure_open` 返回 `None`，`attach_diagnostics` 静默跳过。诊断丢失无提示，用户/LLM 不知道 LSP 未生效。

2. **初始启动阻塞首次工具调用** (时序图 step ③): 如果 agent 的首个工具调用恰是某语言文件，LSP server 的 initialize（可能 5-10 秒）会阻塞在 `OnceCell::get_or_try_init` 的 future 中。整个 tool execution pipeline 被阻塞。建议改为后台初始化 + 首次回退到缓存。

3. **诊断与修改的时序窗口** (时序图 step ⑥-⑦): 写入文件 → didOpen → 等待 800ms 是一种"猜测"策略。LSP server 可能：
   - 在 800ms 内推送了**修改前**缓存的分析结果（server 在收到 didOpen 后需要重新索引）
   - 800ms 内推送了**部分**结果（增量分析的第一次 scan 完成，但完整分析未结束）

4. **跨工具文件竞争**: 同一个 turn 中，两个并行工具分别 edit 同一个文件的不同部分。`open_files` 去重意味着第一个 edit 时发送 didOpen，第二个 edit 被跳过。如果第一个 edit 后的诊断在 800ms 内到达，第二个 edit 可能读取到"过时"的诊断（对应第一个 edit 而非第二个 edit 后的状态）。

5. **诊断覆盖工具结果**: `attach_diagnostics` 将诊断文本追加到原始 tool result content 后（`lsp_supervisor.rs:204-205`），然后整体替换。如果原始 content 包含 LLM 需要的上下文信息（如 diff preview），诊断追加不会丢失这些信息。但如果诊断列表很长（>20 条），`render_diagnostics` 截断会丢失部分信息。

6. **LSP server 崩溃**: 当前代码在 spawn/initialize/did_open 的每个步骤都使用 `?` 或 `.ok()`，但在 read pump 中，`read_framed` 失败时 read task 静默退出（`lsp.rs:134`）。后续 `await_diagnostics` 会超时返回 `None`，`diagnostics_for` 返回空。无 server 存活状态检查，无法感知 server 已死亡。

7. **CancellationToken 未传播给诊断等待** (`lsp_supervisor.rs:176`): `attach_diagnostics` 接收 `_cancel: CancellationToken` 但仅命名为 `_cancel`，实际未使用。800ms 的诊断等待不可取消，在 turn 取消时仍会等待。

### 8.2 未完成点

- **SSE/Socket 传输**: `lsp.rs` 文档注释明确注明 "v1 supports stdio servers only; SSE/socket transports defer"（第 7 行）。无法连接远程 LSP server 或使用 TCP socket server。
- **didChange 增量同步**: 仅实现了 `didOpen`+全量文本，不支持增量变更。对频繁修改的大型文件，每次都发送完整文本。
- **didClose / 资源清理**: 没有 `didClose` 通知，也没有在 session 结束时主动 shutdown LSP server（shutdown 方法已实现但无调用点）。
- **Windows 进程组管理**: `setsid` / `killpg` 仅在 Unix 平台可用，Windows 回退到 `start_kill`（仅杀直接子进程）。

## 9. Next Questions

1. **LSP 初始化能否异步化？** 在 harness 启动时后台预启动所有配置的 LSP server，避免首次工具调用时的冷启动延迟？

2. **诊断 diff 是否需要？** 仅展示本次修改引入的新诊断（与修改前的诊断 diff），减少 LSP 噪声对 LLM 的干扰？

3. **多文件修改能否批量合并 LSP 查询？** 同一 turn 对多个文件修改后，收集所有文件→等待一轮诊断推送→批量追加，减少串行等待累积？

4. **是否需要 `textDocument/didChange` 增量同步？** 对于大文件（>100KB），全量 didOpen 每次传输完整文件内容，是否需要实现增量更新以降低开销？

5. **`after_tool_call` hook 的 content 替换语义是否符合预期？** 当前 LSP hook 取原始 content → 追加诊断 → 替换 content。如果存在多个 `after_tool_call` hook，最后一个生效者会覆盖前面的修改。是否需要 hook 链式组合（chain）？

6. **是否需要 LSP 健康检查？** 当 LSP server 崩溃后，当前设计会在超时后悄悄跳过。是否需要心跳检测 + 自动重连？

7. **`await_diagnostics` 的超时是否可配置？** 当前硬编码 800ms（`lsp_supervisor.rs:24`）。不同语言/LSP server 的分析延迟差异显著，是否需要按 language id 配置？
