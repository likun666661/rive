# Path Containment 精读报告

## scope

本报告覆盖 Maka 桌面应用中所有 **路径围栏（path containment）** 机制，分析以下模块中的 `isInside` / `isInsideOrSamePath` 函数变体、`realpath` 调用模式、符号链接处理策略，以及路径解析的完整调用链：

- `apps/desktop/src/main/main.ts` — 工具产出物源路径解析（`resolveToolArtifactSourcePath`）及内置 `isInsideOrSamePath`
- `apps/desktop/src/main/open-path-guard.ts` — `resolveOpenPath` 的允许列表 key → 路径解析与围栏
- `apps/desktop/src/main/workspace-instructions.ts` — 工作区指令文件扫描、打开、创建的路径围栏
- `apps/desktop/src/main/office-document-tool.ts` — Office 文档读写工具的路径解析（两阶段 lstat + realpath）
- `apps/desktop/src/main/explore-agent-tool.ts` — 只读探索代理的 root 解析与目录遍历围栏
- `apps/desktop/src/main/local-memory-service.ts` — MEMORY.md 及备份文件的路径围栏
- `packages/storage/src/artifact-store.ts` — Artifact 存储的 `isSafeRelativeArtifactPath` + `resolveArtifactPath`

## problem

Maka 桌面应用以 **session cwd**（或 workspaceRoot）为信任边界。所有工具（Bash、Read、Glob、Grep、Write、Edit、OfficeDocument、ExploreAgent、Skill）以及文件打开操作（open-path-guard、workspace-instructions、local-memory）必须确保：

1. **不能读取/写入 cwd 之外的任意文件**（目录穿越 `../`）
2. **不能通过符号链接绕过围栏**（symlink escape）
3. **不能通过绝对路径离开围栏**
4. **渲染器不能提供任意路径**（仅允许 allowlisted key 或相对路径）

由于 `isInside` / `isInsideOrSamePath` 在四个文件中存在 **三种不同实现**，且各自对边界条件（Windows 跨驱动器、大小写不敏感 FS、macOS `/private/var` 软链接）处理不一致，需要统一审计。

## source_evidence

### 1. `isInsideOrSamePath` — 三处使用（main.ts:659, open-path-guard.ts:67, local-memory-service.ts:420, artifact-store.ts:256）

```ts
// 完全相同的实现，出现在 4 个文件中
function isInsideOrSamePath(root: string, target: string): boolean {
  if (target === root) return true;
  const rel = relative(root, target);
  return rel !== '' && !rel.startsWith('..') && rel !== '..'
    && !rel.includes(`..${sep}`) && !rel.startsWith(sep);
}
```

**逻辑分析：**
- `target === root` → 字符串相等直接放行（等同 `isInside` 的 `rel === ''` 路径，但更可靠，因为 `relative` 在某些平台可能对相同路径返回非空字符串）
- `rel !== ''` → 排除 `relative` 返回空串（应为上一条件覆盖，但留作防御）
- `!rel.startsWith('..')` → 拒绝 `../` 形式的父目录穿越
- `rel !== '..'` → 拒绝恰好为 `..` 的情况（`startsWith` 不会拒绝恰好为 `..` 的字符串吗？会！`'..'.startsWith('..')` 是 `true`，所以这个检查是冗余的但无害）
- `!rel.includes(`..${sep}`)` → 拒绝深层穿越如 `a/../b`
- `!rel.startsWith(sep)` → **拒绝 `relative` 返回绝对路径**的场景（Windows 跨驱动器时 `path.relative('C:\\a', 'D:\\b')` 返回 `D:\\b`）

### 2. `isInside` 变体 A — workspace-instructions.ts:218

```ts
function isInside(root: string, target: string): boolean {
  const rel = relative(root, target);
  return rel === '' || (!rel.startsWith('..') && rel !== '..' && !rel.includes(`..${sep}`));
}
```

**⚠️ 差异：缺少 `!rel.startsWith(sep)` 和 `!isAbsolute(rel)` 检查。**
在 Unix 上，如果 `root` 和 `target` 在同一文件系统，`relative` 不会返回绝对路径，因此实际安全。但在 Windows 跨驱动器场景下，`relative` 可能返回绝对路径（如 `D:\outside\file`），`!rel.startsWith('..')` 不会拦截。

**实际风险降低因素：** 所有调用方在调用 `isInside` 前都先通过 `realpath` 解析了路径，所以在单驱动器场景下 `relative` 返回正常相对路径。

### 3. `isInside` 变体 B — office-document-tool.ts:627, explore-agent-tool.ts:962

```ts
function isInside(root: string, target: string): boolean {
  const rel = relative(root, target);
  return rel === '' || (rel !== '..' && !rel.startsWith(`..${sep}`) && !isAbsolute(rel));
}
```

**与 workspace-instructions 版本的区别：**
- 添加了 `!isAbsolute(rel)` — 防御 Windows 跨驱动器场景
- 缺少 `!rel.startsWith(sep)` 但有 `!isAbsolute(rel)` 覆盖了相同目的（`isAbsolute` 检查驱动器和 `/` 前缀）

### 4. `isSafeRelativeArtifactPath` — artifact-store.ts:216（无状态路径验证）

```ts
export function isSafeRelativeArtifactPath(relativePath: string): boolean {
  if (!relativePath || isAbsolute(relativePath)) return false;
  if (relativePath.includes('\0')) return false;
  if (relativePath.includes('//') || relativePath.includes('\\\\')) return false;
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(relativePath)) return false; // URL-like
  const parts = relativePath.split(/[\\/]+/);
  return parts.every((part) => part !== '' && part !== '.' && part !== '..');
}
```

**这是唯一不需要 `realpath` 的围栏函数**，因为它通过 **路径段逐个检查** 实现结构安全，不依赖 `relative()` 语义。它同时处理了：
- 绝对路径
- NUL 字节注入
- 双斜杠
- URL-like 协议前缀
- `..` 和 `.` 路径段

## containment_matrix

| 围栏函数 | 出现位置 | `rel === ''` | `target === root` | `!rel.startsWith('..')` | `rel !== '..'` | `!rel.includes(..${sep})` | `!isAbsolute(rel)` | `!rel.startsWith(sep)` | 是否 realpath | 处理 symlink | 允许 equal root | 拒绝 absolute/.. |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `isInsideOrSamePath` | main.ts, open-path-guard.ts, local-memory-service.ts, artifact-store.ts | — | ✅ | ✅ | ✅ | ✅ | — | ✅ | 调用方负责 | 调用方负责 | ✅ | ✅ |
| `isInside` (workspace-instructions) | workspace-instructions.ts:218 | ✅ | — | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ (调用方) | ✅ (调用方) | ✅ | ✅ (Unix) / ⚠️ (Windows 跨驱动器) |
| `isInside` (office/explore) | office-document-tool.ts:627, explore-agent-tool.ts:962 | ✅ | — | ✅ | ✅ | ✅ | ✅ | — | ✅ (调用方) | ✅ (调用方) | ✅ | ✅ |
| `isSafeRelativeArtifactPath` | artifact-store.ts:216 | N/A (段级检查) | N/A | N/A | N/A | N/A | ✅ (完整路径) | ✅ (完整路径) | ❌ (无需) | ❌ (需后续 realpath) | N/A | ✅ |

### 调用方 realpath 模式总结

| 调用位置 | realpath 调用 |
|---|---|
| `main.ts:resolveToolArtifactSourcePath` | `realpath(cwd)` + `realpath(candidate)` — 双端解析 |
| `open-path-guard.ts:resolveOpenPath` | `realpath(workspaceRoot)` + `realpath(candidate)` — 双端解析（含 project key 单端） |
| `workspace-instructions.ts:scanWorkspaceInstructions` | `realpath(cwd)` + 每个文件 `realpath(join(root, file))` |
| `workspace-instructions.ts:resolveWorkspaceInstructionFileForOpen` | `realpath(cwd)` + `realpath(join(cwd, file))` |
| `workspace-instructions.ts:createWorkspaceInstructionFile` | `realpath(cwd)`（目标用 `join(root, file)` 未 realpath 即传给 `isInside`，但 `resolve` 时再 realpath） |
| `office-document-tool.ts:resolveOfficeDocumentPath` | `realpath(cwd)` + 两步 `lstat` → `realpath(abs)` — **最强的 symlink 防御** |
| `office-document-tool.ts:resolveNewOfficeDocumentPath` | `realpath(cwd)` + `lstat` 目标 + `lstat` 父目录 + `realpath(parent)` |
| `explore-agent-tool.ts:runReadOnlyExplore` | `realpath(cwd)` + 每个 root 的 `resolve` + `realpath` + `stat` |
| `explore-agent-tool.ts:listTextFiles::walk` | 每次递归 `join(abs, entry.name)` 后调用 `isInside`（无 realpath — 因为在 `walk` 内部只做相对拼接，根已 realpath） |
| `local-memory-service.ts:ensure` | `realpath(workspaceRoot)` + `realpath(dir)` + `realpath(file)` — 三重检查 |
| `local-memory-service.ts:restoreBackupBySelector` | `realpath(workspaceRoot)` + `realpath(backupInfo.path)` |
| `local-memory-service.ts:backupInfos` | 每个备份路径 `realpath(path)` |
| `local-memory-service.ts:resolveFileForOpen` | `realpath(workspaceRoot)` + `realpath(file)` |
| `artifact-store.ts:resolveArtifactPath` | `ensureRealDirectory(artifactRoot)` + `realpath(target)` |

## bypass_scenarios

### Scenario 1: Windows 跨驱动器 symlink escape（workspace-instructions 的 `isInside` 变体）

**状态：hypothesis needing test**

`workspace-instructions.ts:218` 中的 `isInside` 缺少 `!isAbsolute(rel)` 检查：

```ts
// workspace-instructions.ts
function isInside(root: string, target: string): boolean {
  const rel = relative(root, target);
  return rel === '' || (!rel.startsWith('..') && rel !== '..' && !rel.includes(`..${sep}`));
}
```

**路径：** 假设 Windows 上 `workspaceRoot = C:\Users\...\workspace`，cwd 通过 symlink 指向 `D:\secret`。`realpath(cwd)` 返回 `D:\secret`，`realpath(join(root, 'AGENTS.md'))` 返回 `D:\secret\AGENTS.md`。`relative(D:\secret, D:\secret\AGENTS.md)` → `AGENTS.md` ✅ 正常。

**真正的风险：** 如果 `join(root, file)` 拼接出的路径在 `realpath` 解析后落到与 root 不同的驱动器。由于 `realpath` 先于 `isInside` 调用，`relative` 在两个绝对路径之间计算，若同驱动器则正常工作。跨驱动器需要用户在另一个驱动器挂载点创建 symlink，而 `realpath` 会将其解析为目标驱动器的路径，此时 `relative` 返回绝对路径，而 `isInside`（workspace-instructions 变体）没有 `!isAbsolute(rel)` 检查。

**缓解因素：**
- macOS 上 `relative` 不会在合法路径间返回绝对路径
- `scanWorkspaceInstructions` 和 `resolveWorkspaceInstructionFileForOpen` 在调用 `isInside` 前都调用了 `realpath`
- 实际攻击需要用户手动创建跨驱动器的 cwd symlink

### Scenario 2: macOS `/private/var` vs `/var` 前缀匹配问题

**状态：confirmed by code**

macOS 上 `/var` 是 `/private/var` 的符号链接。`realpath('/var/folders/...')` 返回 `/private/var/folders/...`。

**影响分析：**
- `isInsideOrSamePath` 和 `isInside` 都依赖 `relative(root, target)` 进行前缀判断
- 如果 `root = '/var/folders/xxx/T/maka-workspace'`，`realpath(root)` 返回 `/private/var/folders/xxx/T/maka-workspace`
- `realpath(target)` 也会解析为 `/private/var/...` 前缀
- `relative('/private/var/folders/...', '/private/var/folders/.../file')` → `file` ✅ 正常

**验证：** 所有 realpath 调用都是一致的（root 和 target 都经过 realpath），因此 `relative` 计算的前缀一致，不会产生误判。

**潜在问题场景：** 如果某一端调用了 `realpath` 而另一端未调用。审计发现：
- `workspace-instructions.ts:createWorkspaceInstructionFile:145` — `const target = join(root, file)` 中 `root` 已 realpath，但 `target` 是字符串拼接未 realpath 即传给 `isInside(root, target)`。此时 `isInside` 用字符串比较（`rel === ''` 检查的是 `relative(realpath(root), join(realpath(root), file))` 的结果，由于两者在同一路径树下，结果仍是正确的相对路径。但如果 `root` 中某个目录组件是 symlink……
  - **安全边界：** `root = realpath(cwd)` 已完全解析，`join(root, file)` 在解析后的路径上拼接，`relative` 计算的是两个都在 `/private/var/...` 下的路径，没有问题。

### Scenario 3: macOS case-insensitive APFS + `target === root` 字符串比较绕过

**状态：hypothesis needing test**

`isInsideOrSamePath` 的第一个检查 `if (target === root) return true` 使用 JavaScript 的严格字符串相等。在 case-insensitive APFS 上（默认配置），`/tmp/Workspace` 和 `/tmp/workspace` 指向同一目录。如果 `realpath` 返回规范化的路径（macOS 通常保留实际大小写），这不是问题。

**但如果：** 攻击者创建一个路径大小写不同的 `realpath` 结果（例如 ext4 上的区分大小写挂载被 realpath 保留了非规范形式），`target === root` 的字符串相等可能失败，然后进入 `relative` 路径。

**缓解因素：** macOS 的 `realpath` 返回文件系统的实际路径名。APFS 默认不区分大小写但保留大小写（case-preserving），所以 `realpath` 返回的是创建时的原始大小写。如果 root 和 target 确实是同一个目录，`realpath` 会返回相同的字符串。

**真正的风险：** 如果 `realpath` 返回不一致的表示（如 `/tmp/../tmp/workspace` vs `/tmp/workspace`）——但 `realpath` 会规范化路径组件，所以不会发生。

### Scenario 4: TOCTOU（Time-of-Check-Time-of-Use）在 `lstat` → `realpath` 之间

**状态：confirmed by code — 存在窗口但利用难度极高**

`office-document-tool.ts:resolveOfficeDocumentPath:400-416`：

```ts
linkStat = await lstat(abs);           // 时点 1
// ...
if (linkStat.isSymbolicLink()) { ... }  // 时点 1 判断
// ...
const actual = await realpath(abs);     // 时点 2
```

在 `lstat` 和 `realpath` 之间，文件可能被替换为 symlink。在单用户桌面应用中，这是一个极低风险的 TOCTOU 窗口，因为：
- 攻击者需要与 Maka 进程有同等文件系统访问权限
- 窗口极窄（微秒级别）
- 如果攻击者已有文件系统写入权限，有更直接的攻击路径

### Scenario 5: explore-agent `listTextFiles::walk` 中缓存的 `workspaceRoot` 参数

**状态：confirmed by code — 安全**

`listTextFiles` 接受 `workspaceRoot` 参数并在每次递归 `join` 后重新检查 `isInside(workspaceRoot, child)`（line 640）。`workspaceRoot` 是 `realpath(input.cwd)` 的结果，在整个遍历中不变。这不是绕过场景，而是正确的防御设计。

## tests

### 已有测试覆盖

| 测试文件 | 覆盖内容 |
|---|---|
| `open-path-guard.test.ts` | ✅ allowlisted key 解析、拒绝未知 key、拒绝 path/URL 作为 key、拒绝 symlink escape、允许 symlink root 后 normalization |
| `workspace-instructions.test.ts` | ✅ symlink escape 被 `blocked`、目录被 `not-a-file`、仅 allowlisted 文件名可解析 |
| `office-document-tool.test.ts` | ✅ `../` 穿越被 `invalid_path`、symlink 被 `symlink_escape`、不支持扩展名、目录被拒 |
| `explore-agent-tool.test.ts` | ✅ `../` root 被 `invalid_root`、symlink 内容被跳过、ignorePaths 过滤 `../` |
| `local-memory-service.test.ts` | ✅ symlink MEMORY.md 被 `error` with "outside the workspace"、symlink 备份被 `missing` |
| `artifact-store.test.ts` | ✅ `isSafeRelativeArtifactPath` 拒绝绝对/穿越/URL-like/空路径、`resolveArtifactPath` 拒绝 symlink escape |
| `builtin-tools.test.ts` | ✅ Read/Glob/Grep 拒绝绝对路径和 `../`、Write/Edit 拒绝绝对/穿越/symlink、Glob 拒绝 symlink cwd |

### 测试缺口与建议

1. **macOS `/private/var` 前缀测试**
   - 当前：无测试覆盖 `/tmp` → `/private/tmp` 的 realpath 转换
   - 建议：创建 workspace 在 `/tmp`（实际是 `/private/tmp`），验证 `isInsideOrSamePath` 和各个 resolver 的 `realpath` 一致性

2. **Windows 驱动器/前缀测试**
   - 当前：无 Windows CI 运行
   - 建议：在 CI 中加入 `relative('C:\\a', 'D:\\b')` 单元测试，验证三种 `isInside` 变体的行为差异；为 workspace-instructions 的 `isInside` 添加 `!isAbsolute(rel)` 防御

3. **大小写不敏感 FS 测试**
   - 当前：无
   - 建议：在 APFS（case-insensitive 模式）上测试 `target === root` 字符串相等是否在大小写差异时正确回退到 `relative` 路径

4. **symlink 在路径中间（非末端）的测试**
   - 当前：所有 symlink 测试都是末端（文件名是 symlink）
   - 建议：测试 `/workspace/projects/current → /workspace/projects/v2` 这种中间目录 symlink，验证 `realpath` 在 `walk` 和 `join` 之间的行为

5. **`/dev/fd/` 和 `/proc/self/fd/` 类特殊文件系统路径**
   - 当前：无测试
   - 建议：验证 `realpath` 是否能正确解析这些路径，以及它们是否能通过 `isInside` 检查

## next_actions

### 高优先级（建议修复）

1. **统一 `isInside` 实现** — 将 workspace-instructions.ts、office-document-tool.ts、explore-agent-tool.ts 中的三个 `isInside` 变体统一为单一实现（建议 `isInsideOrSamePath` 形态，包含 `!isAbsolute(rel)` 和 `!rel.startsWith(sep)` 双重防御），放到 `@maka/core` 或共享模块中

2. **为 workspace-instructions.ts 的 `isInside` 添加 `!isAbsolute(rel)` 检查** — 当前版本是唯一缺少绝对路径拒绝的变体，虽然当前调用方都使用 realpath 降低了风险，但防御深度原则要求添加

### 中优先级（建议加强）

3. **添加 Windows 跨驱动器单元测试** — 至少 mock `relative` 返回值验证三种 `isInside` 变体在绝对路径返回时的行为

4. **添加 macOS APFS case-insensitive 测试** — 验证 `realpath` 在大小写保留模式下的行为一致性

5. **审计 `explore-agent-tool::walk` 中的无 realpath 路径拼接** — 当前在递归遍历中，`join(abs, entry.name)` 后直接调用 `isInside(workspaceRoot, child)` 而不 `realpath`。虽然 `workspaceRoot` 已 realpath 且 `child` 是合法的路径拼接，但如果中间目录包含 symlink，深层子路径可能绕过。当前通过 `lstat` 跳过 symlink（`walk` 中 `entryStat.isSymbolicLink()` 检查），因此安全。但依赖 `readdir({withFileTypes:true})` 的 `isSymbolicLink()` 而非 `realpath`，建议文档化此行为

### 低优先级

6. **文档化三种 `isInside` 变体的差异和统一计划** — 在代码中添加注释标记已知差异
7. **添加 TOCTOU 防御注释** — 在 `office-document-tool.ts` 的 `lstat` → `realpath` 序列处添加注释说明窗口已评估为可接受
