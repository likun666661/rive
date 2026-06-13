# Maka 外部工具注入深度分析报告

> 基线：`335220a` | 深度档位：`maintainer` | 模块：外部工具注入（External Tool Injection）

---

## scope

本报告分析 Maka desktop 主进程中四个外部工具的注入架构与安全边界：

| 工具 | 源文件 |
|------|--------|
| **Rive CLI (rive-cli.ts)** | `apps/desktop/src/main/rive-cli.ts` |
| **RiveWorkflow Tool** | `apps/desktop/src/main/rive-workflow-tool.ts` |
| **OfficeDocument / OfficeDocumentEdit Tool** | `apps/desktop/src/main/office-document-tool.ts` |
| **ExploreAgent Tool** | `apps/desktop/src/main/explore-agent-tool.ts` |
| **officecli ENV / Probe（支撑模块）** | `apps/desktop/src/main/officecli-env.ts`、`officecli-probe.ts` |

每个工具都通过 `MakaTool<P, R>` 接口挂载（定义于 `packages/runtime/src/ai-sdk-backend.ts:100-121`），统一提供 `impl(args, ctx: MakaToolContext)` 入口。`MakaToolContext` 注入 `cwd`、`abortSignal`、`emitOutput`、工具调用 ID 与会话 ID。

未纳入分析的内部工具（Read / Glob / Grep / Bash 等）属于另一模块边界。

---

## problem

Maka 的 AI agent 在会话中通过 `impl` 调用外部二进制（`rive` 二进制、`officecli` 二进制），或在 Node.js 进程内执行本地文件 I/O（ExploreAgent）。核心风险域：

1. **参数注入**：用户/模型通过 tool args 投递恶意参数，最终拼入 `spawn` / `execFile` 的 `argv`。
2. **binary path injection**：通过环境变量（`MAKA_RIVE_BIN`、`RIVE_BIN`、`PATH`）或 `deps.riveBin` 控制实际执行的二进制路径。
3. **cwd escape**：路径解析中的 `..` 穿越和符号链接绕过，使工具读取/写入 cwd 外文件。
4. **symlink**：`lstat` + `realpath` 双校验防止符号链接绕过。
5. **output truncation**：stdout/stderr 捕获上限截断可能丢失错误信息或隐藏注入证据。
6. **stale child process**：`SIGTERM` → `SIGKILL` 超时/取消策略可能留下僵尸进程（特别是 detached 进程组）。

---

## source_evidence

### 1. Rive CLI（`rive-cli.ts`）

- **核心入口**：`runRiveCli(input, options)` → `spawnRive(command, options)`
- **二进制解析**：`resolveRiveBinary()`（line 180）优先级链：
  ```
  options.riveBin → env.MAKA_RIVE_BIN → env.RIVE_BIN → 'rive'（$PATH fallback）
  ```
  若候选路径含 `/`，执行 `access(candidate, X_OK)` 可执行性检查；否则依赖 `PATH` 解析。
- **argv 构建**：`buildRiveCommand(input)` 按 `action` 分发，通过 `requireString()` 强制校验关键字段（`path`、`templateId`、`commandId` 等），通过 `assertPositiveInteger()` 校验数值字段。
- **参数注入校验**：`appendParams()` 对 key 做正则 `/^[A-Za-z0-9_.-]+$/` 校验（line 381-384），值通过 `String(value)` 序列化后拼入 `--param key=value`。
- **子进程启动**：`spawn(bin, args, { shell: false, detached: platform !== 'win32' })`（line 205-211），stdin=ignore，stdout/stderr pipe。
- **输出捕获上限**：`MAX_CAPTURE_BYTES = 2 MiB`，超限触发 `requestTerminate('output_too_large')`。
- **超时**：`runRiveCli` 内 `timeoutMs ?? input.timeoutMs ?? 600_000`（10 分钟），硬上限 1h（line 148）。
- **终止策略**：`SIGTERM` → 2s → `SIGKILL`（line 227-231）；detached 进程组杀 `-child.pid`（line 324-326）。
- **红化（Redaction）**：`redactRiveText()` / `redactRiveValue()` 在 stdout/stderr 输出和 envelope 中过滤 Bearer token、API key、secret 等模式。

### 2. RiveWorkflow Tool（`rive-workflow-tool.ts`）

- **工具注册**：`buildRiveWorkflowTool(deps)` → `MakaTool<RiveWorkflowToolArgs, RiveWorkflowToolResult>`
- **permissionRequired**：`true`，`categoryHint: 'custom_tool'`
- **Zod schema 约束**（line 25-53）：
  - `path`：最大 2000 字符
  - `templateId`/`workflowRunId` 等：最大 240 字符
  - `workers`：数组最大 20 个元素，每个元素 1-200 字符
  - `maxParallel`：1-20 正整数
  - `timeoutSeconds`：1-3600
  - `timeoutMs`：1-3600000
  - `opencodeBin`/`codexBin`：最大 2000 字符
- **output projection（信息降级）**：成功时只返回 `projection`、`nodes`、`state`、`ids`，不暴露原始 `protocol`/`display` 对象（line 156-171）。失败时通过 `error.envelope?.error` 提取 `code` 和 `suggestedAction`。
- **emitOutput 红化**：command 回显经过 `displayArg()` → `redactRiveText()` 处理。
- **command 重复构建**：line 121-124 在 try 块内先构建 command 用于 display，再传入 `runRiveCli`——这导致 command 构建两次，但确保即使 `buildRiveCommand` 抛异常也能拿到已构建的部分用于错误报告。

### 3. OfficeDocument / OfficeDocumentEdit（`office-document-tool.ts`）

- **两工具分离**：
  - `OfficeDocument`：`permissionRequired: false`，只读（help/view/get/query/validate），HTML 模式故意不支持。
  - `OfficeDocumentEdit`：`permissionRequired: true`，`categoryHint: 'file_write'`，写操作（create/add/set/remove），明确声明不执行 raw/watch/batch/shell。
- **路径安全**：`resolveOfficeDocumentPath` / `resolveNewOfficeDocumentPath` 执行三层校验：
  1. 输入校验：拒绝绝对路径、空路径、含 `\0` 路径（line 386/428）
  2. `lstat` 检查是否为符号链接 → 拒绝（`symlink_escape`）
  3. `realpath` 后再次 `isInside` 检查 → 双重防逃逸
  4. 扩展名白名单：`.docx` / `.xlsx` / `.pptx`
- **execFile（非 spawn）**：`office-document-tool.ts` 使用 `execFile` 而非 `spawn`，自带 `timeout` 和 `maxBuffer`。
  - 超时：`OFFICE_DOCUMENT_TIMEOUT_MS = 15_000`
  - 输出上限：`OFFICE_DOCUMENT_MAX_BUFFER = 512 KiB`，文本显示上限 `OFFICE_DOCUMENT_OUTPUT_MAX_CHARS = 60_000`
- **env 构建**：`buildOfficeCliEnv()`（`officecli-env.ts`）：
  - 设置 `OFFICECLI_SKIP_UPDATE=1`
  - PATH 前置 bundled tools 目录
  - 大小写不敏感地清空旧 PATH key 后重建
- **输出消毒**：`sanitizeOfficeCliOutput()` 用 `<workspace>` 替换工作目录绝对路径，并通过 `redactSecrets` 全局函数二次脱敏。

### 4. ExploreAgent（`explore-agent-tool.ts`）

- **纯 Node.js 实现，无外部二进制调用**。
- **permissionRequired**：`true`，`categoryHint: 'subagent'`
- **副作用边界声明**：`mode: 'read_only'`，不写文件 / 不联网 / 不启动进程（line 290-293 notes）
- **预算体系**：
  - 候选文件：`MAX_DISCOVERED_FILES = 250`
  - 单文件大小：`MAX_FILE_BYTES = 512 KiB`
  - 总读取量：`MAX_TOTAL_BYTES = 2 MiB`
  - 最大命中数：120，最大读取文件数：80
  - 查询词：`MAX_QUERIES = 8`，roots：`MAX_ROOTS = 5`
- **路径安全**：
  - `normalizeRoots()`：默认 `['.']`，最多 5 个
  - 每个 root 通过 `resolve` + `realpath` → `isInside` 双重校验
  - `listTextFiles` 跳过符号链接（line 594-597）
- **敏感文件跳过**：`SENSITIVE_TEXT_FILE_NAMES` 集合（`.env` 系列、`.npmrc`、`.pypirc`、`credentials.json`、私钥文件等）+ `.pem`/`.key` 等扩展名检测。仅报告数量，不读取内容。
- **ignore paths 规范化**：拒绝 `.`、`..`、绝对路径、含 `\0` 路径、空路径（line 666-677）。
- **abort 支持**：多个检查点（扫描前、每文件读取前后）检查 `abortSignal.aborted`，支持部分结果返回（`canceled_partial` 终态）。

### 5. officecli-probe / officecli-env

- `probeOfficeCli()`：通过 `execFile('officecli', ['--version'], ...)` 探活，超时 1500ms。
- 返回类型 `OfficeCliProbe`：`available: true + version` 或 `available: false + reason`（missing/timeout/failed）。
- 不抛异常，所有错误通过 resolve 返回。
- `buildOfficeCliEnv()` 确保 PATH 大小写不敏感处理（line 34-37），防止 Windows/macOS PATH 残留。

---

## tool_matrix

| 维度 | RiveWorkflow | Rive CLI (底层) | OfficeDocument | OfficeDocumentEdit | ExploreAgent |
|------|-------------|-----------------|----------------|---------------------|--------------|
| **action 列表** | 8 个 high-level 命令 | 同左（透传） | help/view/get/query/validate | create/add/set/remove | N/A（内部实现） |
| **副作用等级** | 读+执行（触发远程agent） | 读+执行（子进程exit code/JSON） | 只读（officecli无写操作） | 写（创建/修改Office文件） | 只读（纯Node.js I/O） |
| **读** | ✓ | ✓ | ✓ | ✓（隐式读取后写入） | ✓ |
| **写** | ✓（workflow变更） | ✗（CLI层不直接写磁盘） | ✗ | ✓ | ✗（明确禁止） |
| **执行** | ✓（触发Rive scheduler） | ✓（子进程） | ✓（officecli子进程） | ✓（officecli子进程） | ✗ |
| **网络** | 间接（scheduler内agent） | ✗（CLI桥不直接联网） | ✗ | ✗ | ✗ |
| **多agent orchestration** | ✓ | ✗ | ✗ | ✗ | ✗ |
| **permissionRequired** | true | N/A | false | true | true |
| **二进制** | rive | rive | officecli | officecli | 无（纯Node.js） |
| **argv构建** | buildRiveCommand() → shell=false spawn | 同左 | execFile('officecli', args) | 同左 | N/A |
| **cwd** | MakaToolContext.cwd | 透传自options.cwd | cwd → realpath安全 | 同左 | cwd → realpath安全 |
| **env** | deps.env ?? process.env | 同左 | buildOfficeCliEnv() | 同左 | N/A |
| **timeout** | args.timeoutMs | 默认10min，上限1h | 15s | 15s | 无显式超时 |
| **abort清理** | SIGTERM→2s→SIGKILL | 同左 | execFile原生timeout/kill | 同左 | 检查点abort+部分结果 |
| **输出上限** | 2 MiB | 2 MiB | 512KiB buffer+60K chars | 同左 | 2 MiB总/512 KiB单文件 |

---

## injection_risks

### 1. 参数注入（Argument Injection）

**Rive CLI：**
- `appendParams()` 对 key 做正则校验 `/^[A-Za-z0-9_.-]+$/`（`rive-cli.ts:381-384`），但 value 仅做 `String()` 转换。由于使用 `spawn(bin, args, { shell: false })`，单一 argv 元素不会被 shell 解释——**安全**。
- `appendSchedulerOptions` 中 `opencodeBin`、`codexBin` 作为 `--opencode-bin`/`--codex-bin` 的值传入，仅经过 Zod schema `max(2000)` 校验，无路径消毒。如果 Rive CLI 内部不安全地使用这些值（如拼接到 shell 命令），可能成为注入向量。
- `workers` 数组元素仅校验 `min(1).max(200)` 字符串，无格式约束。如果 Rive CLI 将 `--worker` 值作为 agent ID 进行沙箱逃逸，存在风险。

**OfficeDocument：**
- `props` 中的 key 校验 `/^[A-Za-z0-9_.:-]{1,80}$/`，value 通过 `String(raw)` 序列化后拼入 `--prop key=value`。`execFile` 的 `args` 数组传递，无 shell 注入风险。
- `selector`、`query`、`target` 经过 `normalizeBoundedText()` 处理：合并空白、trim、长度上限 500、拒绝 `\0`。但内容无结构校验——如果 `officecli` 内部解析器对恶意构造的 XPath/selector 存在漏洞，会被透传。

**ExploreAgent：**
- `objective`、`stoppingCondition` 仅做长度截断，内容无注入点——内部仅用于字符串匹配和报告生成。

### 2. Binary Path Injection（二进制路径注入）

**Rive CLI：**
- `resolveRiveBinary()` 优先级链中，`options.riveBin` → `MAKA_RIVE_BIN` → `RIVE_BIN` → `'rive'`。
- 如果 `candidate.includes('/')`（即指定了路径），执行 `access(candidate, X_OK)` 可执行性检查。但如果 `candidate` 不含 `/`（如 `'rive'`），则依赖 `PATH` 解析。
- **风险**：`options.env` 中可能包含被污染的 `PATH`（`deps.env ?? process.env`），攻击者可通过控制 `PATH` 前置目录投放恶意 `rive` 二进制。
- `spawnRive` 使用 `env: options.env`（`rive-cli.ts:207`），默认 `process.env`，不隔离 PATH。

**OfficeDocument / officecli-probe：**
- `buildOfficeCliEnv()` 通过 `prependBundledOfficeCliTools()` 前置 bundled tools 目录到 PATH，但仅前置，不删除系统 PATH 中的其他条目。
- 如果 bundled tools 目录不存在或为空，`officecli` 仍会从系统 PATH 解析。
- `probeOfficeCli()` 中 `execFile('officecli', ...)` 同样依赖 `buildOfficeCliEnv()` 后的 PATH。

### 3. CWD Escape（工作目录逃逸）

**Rive CLI：**
- `spawnRive` 直接使用 `options.cwd`，无 `realpath` 或 `isInside` 校验。
- 但 `cwd` 由调用方（`buildRiveWorkflowTool` 的 `impl`）传入，来自 `MakaToolContext.cwd`。**信任链依赖上游确保 cwd 可信**。

**OfficeDocument：**
- 两次 `realpath` + `isInside` 校验，路径安全较完备。
- 创建操作检查父目录是否存在、是否为符号链接。

**ExploreAgent：**
- `realpath(workspaceRoot)` 解析后，每个 root 再 `resolve` + `realpath` + `isInside`，防御完备。

### 4. Symlink Escape（符号链接逃逸）

**OfficeDocument：**
- 读取：`lstat` 检查符号链接 → 拒绝 → `realpath` 再次 `isInside`（双路径防御）。
- 写入（create）：检查目标文件不存在 → 检查父目录非符号链接 → `realpath` 父目录 → `isInside`。

**ExploreAgent：**
- `listTextFiles` 中 `lstat` 跳过符号链接（`explore-agent-tool.ts:594-597`），不跟随。

**Rive CLI：**
- 无符号链接检查——因为 Rive CLI 不直接读写文件系统中用户指定的文件路径，`path` 参数传给 `workflow_validate`/`workflow_import`，表示 workflow package 路径，由 Rive 二进制自身处理。

### 5. Output Truncation（输出截断）

**Rive CLI：**
- `MAX_CAPTURE_BYTES = 2 MiB`，超限时触发 `requestTerminate('output_too_large')` → 子进程被杀死。
- 仅保留最后 `TAIL_CHARS = 24_000` 字符的 stdout/stderr tail，可能丢失关键错误信息的起始部分。
- `parseRiveJson` 尝试解析完整 stdout，如果 JSON 被截断（超过 2 MiB），解析必然失败。

**OfficeDocument：**
- `maxBuffer: 512 KiB`（`execFile` 选项），超限 Node.js 会 kill 子进程并报错。
- 显示文本上限 60K 字符（Unicode 字符计数），截断后附加中文提示。

### 6. Stale Child Process（僵尸子进程）

**Rive CLI：**
- `detached = platform !== 'win32'`，`spawn` 的 `detached` 选项在 Unix 上意味着子进程成为新进程组 leader（`setsid`），父进程退出不杀子进程。
- `killRiveChild` 用 `process.kill(-child.pid, signal)` 杀整个进程组——**正确**。
- 但如果 `SIGTERM` 和 `SIGKILL` 间隔内子进程 fork 了新的孙进程且脱离进程组，孙进程可能成为孤儿。
- `killTimer.unref()` 使定时器不阻止事件循环退出，但如果主进程在 SIGKILL 发送前崩溃，detached 进程可能残留。

**OfficeDocument：**
- `execFile` 由 Node.js 管理子进程生命周期，`timeout` 选项触发 kill。但如果 `officecli` 进程 fork 了子进程，这些孙进程可能不被清理。

**ExploreAgent：**
- 无子进程，仅 Node.js 异步 I/O。`abortSignal` 检查点可能留下未完成的 `readFile`/`readdir` Promise，但这些 Promise 最终会 resolve/reject 并被 GC 回收。

---

## cleanup_policy

### Rive CLI spawn 生命周期

```
spawnRive (rive-cli.ts:192-319)
  │
  ├─ spawn(bin, args, { shell: false, detached: true, stdio: [...] })
  │
  ├─ setTimeout → requestTerminate('timeout', ...)   [超时]
  ├─ abortSignal.addEventListener('abort') → requestTerminate('aborted', ...)  [取消]
  │
  ├─ requestTerminate():
  │     1. killRiveChild(child, 'SIGTERM', detached)  → process.kill(-pid, SIGTERM)
  │     2. killTimer = setTimeout → killRiveChild(child, 'SIGKILL', detached)  [2s后强制]
  │     3. killTimer.unref()
  │
  ├─ child.on('error') → fail(RiveCliError)           [进程启动失败]
  └─ child.on('close') → resolve / reject              [进程结束]
        │
        ├─ termination? → reject(termination error)
        ├─ bad JSON? → reject('bad_json')
        ├─ exitCode ≠ 0? → reject('rive_failed')
        └─ resolve(success result)
```

**特点：**
- `settled` flag 防止重复 reject/resolve
- cleanup 清除 timer 和 abort listener
- detached 进程组杀确保子进程树清理
- `unref` 确保定时器不阻止进程退出

### OfficeDocument execFile 生命周期

```
runOfficeCli (office-document-tool.ts:587-608)
  │
  ├─ runner('officecli', args, { timeout, maxBuffer, env })
  │
  ├─ callback: error → reject / resolve
  └─ child.on('error') → reject
```

**特点：**
- 依赖 Node.js `execFile` 的 `timeout` 选项管理超时
- `maxBuffer` 作为输出上限，超限同样触发 kill
- 无自定义 abort signal 集成——`execFile` 不直接支持 `AbortSignal`
- **注意**：`MakaToolContext` 提供的 `abortSignal` 在 `runOfficeCliOperation` / `runOfficeDocumentEditOperation` 中**未被使用**——如果上层 abort，正在执行的 officecli 子进程不会被主动 kill

### ExploreAgent abort 策略

```
runReadOnlyExplore (explore-agent-tool.ts:226-518)
  │
  ├─ 开始时检查 abortSignal.aborted → abortFailure
  ├─ 每个 root 解析前检查 → abortFailure
  ├─ 每个文件读取前检查 → partialAbortFailure / abortFailure
  ├─ 每个文件读取后检查 → partialAbortFailure
  │
  ├─ abortFailure:       partial=false, terminalStatus='canceled'
  └─ partialAbortFailure: 如果有已读文件/命中，返回 partial result
```

**特点：**
- 无子进程，仅通过检查点 abort Promise 链
- 部分结果保留策略：如果有 `filesInspected > 0` 或 matches，返回 `canceled_partial`
- 无法中断正在执行的 `readFile`——但 Node.js 的 `readFile` 通常很快（本地 I/O），不影响实际安全

---

## next_actions

### 高优先级

1. **OfficeDocument abort signal 集成缺失**：`runOfficeCli` 使用 `execFile`，不接受 `AbortSignal`。应替换为 `spawn` + 手动 pipe 捕获（类似 `rive-cli.ts`），以便在 tool context abort 时主动 kill officecli 子进程。当前超过 15s 超时后仍依赖 `execFile` 的 `timeout` 选项。

2. **Rive CLI PATH 污染风险**：`spawnRive` 使用 `options.env` 默认为 `process.env`，未隔离 PATH。攻击者若能控制会话环境变量，可在 PATH 中前置恶意 `rive` 二进制。建议：
   - 将 `resolveRiveBinary` 返回的绝对路径传入 spawn，而非依赖 PATH
   - 或者使用 sanitized env（如 `buildOfficeCliEnv` 模式）

3. **Rive CLI detached 进程残留风险**：`detached` 模式下，如果 `SIGKILL` 发送前 Node.js 进程崩溃，子进程成为孤儿。建议在 `spawnRive` 的 cleanup 中增加进程组追踪（如 `process.on('exit', ...)` 注册清理回调）。

### 中优先级

4. **Rive CLI opencodeBin / codexBin 透传风险**：这两个参数从 Zod schema `max(2000)` 校验后直接拼入 `--opencode-bin` / `--codex-bin`，无路径消毒。如果 Rive CLI 内部不安全使用这些路径，可能成为注入向量。建议增加 `realpath` 或 `access` 可执行性检查。

5. **输出截断数据丢失**：Rive CLI 的 `TAIL_CHARS = 24_000` 只保留尾部，可能丢失 JSON envelope 的起始部分（在 `output_too_large` 场景下 `parseRiveJson` 必然失败）。建议在超过 MAX_CAPTURE_BYTES 时保留头部 JSON 区域（前 N 字节）而非仅尾部。

6. **ExploreAgent abort 无法中断 readFile**：在极端情况下（如读取极慢的网络文件系统），`readFile` 可能长时间阻塞，且 abort 检查点在 `readFile` 前后，无法中断正在进行的 readFile。建议考虑使用 `fs.createReadStream` + 逐块 abort 检查。

### 低优先级

7. **OfficeDocument selector/query 无结构校验**：当前仅做 `normalizeBoundedText()` 处理，如果 officecli 内部解析器存在漏洞（如 XXE），会被透传。但攻击者需要先绕过 Maka tool args 的 Zod schema 校验——风险较低。

8. **officecli PATH 前置不隔离**：`prependBundledOfficeCliTools` 仅前置 bundled tools 目录，不移除系统 PATH。如果系统 PATH 中存在恶意 `officecli` 且 bundled 不可用，会 fallback 到系统版本。

9. **ExploreAgent 预算耗尽后无声降级**：当 `file_budget` 或 `match_budget` 触发时，工具返回 `ok: true` 但 `limitReasons` 非空，AI 模型可能不理解为何结果不完整。当前有结构化 `limitReasons` + notes 说明，但可进一步在 `summary` 中显式标注预算边界。

---

*报告生成时间：2026-06-13 | 基线 commit: 335220a | 深度档位: maintainer*
