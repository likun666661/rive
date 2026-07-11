# 第 04 章研究笔记：KV Cache、Radix 与内存

> 面向后续扩写教材的证据笔记，不是最终章节。本文将“代码事实”和“教学提案”严格分开；所有关于当前实现的陈述均给出仓库路径与符号（必要处附行号，行号以本次阅读版本为准）。

## scope

### 本笔记覆盖的代码事实

- **物理 KV 存储。** `python/minisgl/kvcache/mha_pool.py:MHAKVCache.__init__` 分配单块 CUDA 张量，形状为 `[2, num_layers, num_pages, page_size, local_kv_heads, head_dim]`；`0/1` 维分别切成 K/V，且每层存储可在 `store_kv` 中展平为 `[num_pages * page_size, local_kv_heads, head_dim]` 后按 `out_loc` 写入（第 27–37、45–56 行）。
- **两种“池”的责任边界。** `python/minisgl/scheduler/table.py:TableManager` 只分配请求表行 `table_idx`，并持有与 `page_table` 同形状、初始化为 0 的 `token_pool`（第 4–21 行）；`python/minisgl/scheduler/cache.py:CacheManager` 才持有物理页的 `free_slots`、负责分页分配、回收和前缀缓存衔接（第 15–25、42–124 行）。
- **前缀缓存的抽象合同。** `python/minisgl/kvcache/base.py:BasePrefixCache` 定义 `match_prefix`、`lock_handle`、`insert_prefix`、`evict`、`size_info` 与完整性检查；其 docstring 明确要求：`match_prefix` 返回的索引必须在对应 handle 已锁定时才可使用（第 67–135 行）。
- **两种前缀策略。** `python/minisgl/kvcache/__init__.py:SUPPORTED_CACHE_MANAGER` 注册 `"naive"` 和 `"radix"`，`create_prefix_cache` 按名称构造；`NaivePrefixCache` 始终给出长度为 0 的 handle、没有可逐出内容（`naive_cache.py:NaivePrefixCache.match_prefix/insert_prefix/size_info`，第 16–45 行）。
- **调度准入和请求装配。** `python/minisgl/scheduler/prefill.py:PrefillAdder._try_allocate_one` 执行 prefix match、容量预估、锁定、锁定后的二次容量检查，再分配表行并把命中 token/location 拷入该行（第 39–63 行）；`_add_one_req` 随后只把未命中的输入 token 拷入 `token_pool`，构造 `Req` 或 `ChunkedReq`（第 65–90 行）。

### 不在本笔记内的内容

- 不讲最终 attention kernel 如何消费 page table，也不把 `MHAKVCache` 的当前实现泛化成所有模型或所有 KV 架构；工厂处还标注了“TODO: support other variants (e.g. MLA)”（`python/minisgl/kvcache/__init__.py:create_kvcache_pool`，第 27–44 行）。
- 不写生产容量建议、命中率结论或性能承诺；本节点要求的是读码证据与教学设计，而非 benchmark。
- 不写最终扩写教材。下面的故事、演示和练习均是给后续作者的**提案**，不表示仓库已提供相应 UI、指标或自动化实验。

## teaching_goal_alignment

课程大纲第 04 章要求学生能解释“扁平 token location 而非 page id”、沿前缀命中说明 match/lock/写入/insert/re-lock/evict 生命周期，并以页粒度与锁引用计数说明复用和内存安全的折中（`.rive-artifacts/minisglang-teaching-20260710/manual/teaching-manual-outline.md` 的“04 KV Cache、Radix 与内存”部分）。建议将目标拆成下列可观察表现。

| 课程目标 | 学生应能做什么 | 对应代码证据 |
| --- | --- | --- |
| 区分逻辑位置与物理位置 | 说明 `page_table[table_idx, pos]` 被写入的是可直接用于扁平 KV storage 的 token location，而非单独的页号；并把一页起点展开成连续位置。 | `MHAKVCache.store_kv` 以 `out_loc` 索引展平 storage（`mha_pool.py:MHAKVCache.store_kv`，45–56）；`CacheManager._page_to_token` 将页起点加 offset 展开（`scheduler/cache.py:CacheManager._page_to_token`，119–124）；`_write_page_table` 将展开值 scatter 到表中（127–146）。 |
| 区分表行与物理页 | 在图上把 `TableManager.allocate` 产生的请求行和 `CacheManager._allocate` 交付的页分开，不把两者都称为“分配 cache”。 | `TableManager.allocate/free` 操作 Python `_free_slots`（`scheduler/table.py`，17–21）；`CacheManager.free_slots` 是 `arange(num_pages) * page_size` 的设备张量（`scheduler/cache.py:CacheManager.__init__`，16–21）。 |
| 说明安全复用 | 用“先 match、再 lock、再复制位置”叙述，并指出锁住的是到根的整条 radix 路径。 | 抽象合同（`kvcache/base.py:BasePrefixCache.lock_handle/match_prefix`，67–93）；准入顺序（`scheduler/prefill.py:PrefillAdder._try_allocate_one`，44–61）；路径 ref-count 处理（`kvcache/radix_cache.py:RadixPrefixCache.lock_handle`，113–130）。 |
| 说明回收和逐出 | 区分结束请求尾部释放、并发插入造成的重复区域释放，以及 free list 不足时的 LRU 驱动逐出。 | `CacheManager.cache_req` 的四段注释与 `_free` 调用（`scheduler/cache.py`，55–79）；`CacheManager._allocate`（106–113）；`RadixPrefixCache.evict`（148–175）。 |
| 用不变量检验讲解 | 写出空闲页数与缓存页数之和、页对齐以及 `SizeInfo` 两种规模的含义。 | `CacheManager.check_integrity`（81–91）；`SizeInfo.total_size`（`kvcache/base.py`，48–54）；radix 锁/解锁的账目迁移（`radix_cache.py`，113–130）。 |

## concept_to_execution_path

以下是可作为最终教材主线的“概念 → 执行”路线。带“代码事实”的每一步仅描述当前实现；“教学解释”是表述建议。

1. **代码事实：请求先做最长可用前缀查询。** `CacheManager.match_req` 断言输入非空，并把 `req.input_ids[:input_len - 1]` 交给 `prefix_cache.match_prefix`（`python/minisgl/scheduler/cache.py:CacheManager.match_req`，27–30）。因此最后一个输入 token 不参与这次可命中前缀查询。

   **教学解释（提案）：** 先让学生把“正在计算的末 token”与“已经可当作历史读写位置的前缀”画成两种颜色，避免把缓存命中误解为对全部 prompt 的魔法跳过。

2. **代码事实：命中只是候选能力，不是永久地址。** `RadixPrefixCache.match_prefix` 通过 `_tree_walk` 构造 `RadixCacheHandle(prefix_len, node)`，本身不加锁（`python/minisgl/kvcache/radix_cache.py`，132–134）；`BasePrefixCache` 的接口注释要求 caller 在使用此前返回的 tensor 前锁定 handle（`python/minisgl/kvcache/base.py`，69–88）。

   **教学解释（提案）：** 把 handle 讲成“可借阅凭证”：查询拿到书目卡，锁才把书留在书架上。不要把它讲成单纯的整数长度。

3. **代码事实：准入会在锁前、锁后分别核对容量。** `PrefillAdder._try_allocate_one` 以 `extend_len + output_len + reserved_size` 同 `cache_manager.available_size` 比较；初检通过后锁 handle，再做同一检查，二检失败就解锁并返回（`python/minisgl/scheduler/prefill.py`，44–54）。`available_size` 由“可逐出 token 数 + free list 页数 × page_size”组成（`python/minisgl/scheduler/cache.py:CacheManager.available_size`，32–34）。

   **教学解释（提案）：** 将第二次检查作为“锁定也要占资源”的反直觉点：锁会把节点从 evictable 转成 protected，故第一次预算并非锁后的预算。

4. **代码事实：命中的前缀只复制元数据，不重新分配物理页。** 准入成功后，`_try_allocate_one` 从 `handle.get_matched_indices()` 拷入新表行的 `page_entry`，并把同一前缀 token ids 拷入 `token_pool`（`python/minisgl/scheduler/prefill.py`，56–61）。未命中或本轮延展的 token ids 则在 `_add_one_req` 复制（79–81）。

5. **代码事实：新 span 按页申请，而 page table 最终存每 token 的 raw location。** `CacheManager.allocate_paged` 对每个 request 以 `ceil(cached_len/page_size)` 到 `ceil(device_len/page_size)` 得到新增页区间（`python/minisgl/scheduler/cache.py`，42–53）。`_allocate` 返回页起点，`_page_to_token` 展开为该页连续 token locations，`_write_page_table` 据此写入逻辑位置区间（106–146）。`MHAKVCache.store_kv` 使用这些 `out_loc` 给对应 layer 的展平 K/V storage 写入（`python/minisgl/kvcache/mha_pool.py`，45–56）。

6. **代码事实：radix 树只把整页前缀变为可长期共享的条目。** `RadixPrefixCache.insert_prefix` 对输入长度 `align_down(..., page_size)`，只保留整页；新节点保存 `indices[prefix_len:].clone()`，并将新节点长度计入 `evictable_size`（`python/minisgl/kvcache/radix_cache.py`，136–146）。

   **教学解释（提案）：** 把“不足一页的尾巴”列为活跃请求的短期工作区，而非 trie 的可共享资产；这能把 page granularity 的容量浪费与安全边界一并讲清。

7. **代码事实：请求完成或继续运行会走不同的释放/保护分支。** `CacheManager.cache_req` 先 insert、解旧 handle，再释放“之前未命中、后来却已由其他请求缓存”的重复区域；若 `finished=True`，释放新 handle 长度之后的尾部；否则保存新 handle 并锁定它（`python/minisgl/scheduler/cache.py:CacheManager.cache_req`，55–79）。

8. **代码事实：free list 不够时才 demand-driven eviction。** `_allocate` 发现 `needed_pages > len(free_slots)`，向 `prefix_cache.evict` 请求差额对应的 token 数，随后仅保留 `evicted[::page_size]` 作为可再分配的页起点（`python/minisgl/scheduler/cache.py`，106–113）。`BasePrefixCache.evict` 明确允许实际逐出量大于请求量（`python/minisgl/kvcache/base.py`，108–122）。

9. **代码事实：radix eviction 从可逐出的叶子中按时间戳选择，必要时向上级联。** `RadixPrefixCache._collect_leave_nodes_for_evict` 只收集 `ref_count == 0` 的叶（190–203）；`evict` 把它们 heapify，按 `RadixTreeNode.__lt__` 的 timestamp 弹出，删除父子关联，并在父节点变为无锁叶时把它加入候选（`python/minisgl/kvcache/radix_cache.py`，83–84、148–175）。`_tree_walk` 会刷新完整命中节点的 timestamp，且部分命中 split 后也刷新共享节点的 timestamp（205–231）。

10. **代码事实：调度可把释放延后到 context 块结束。** `CacheManager.lazy_free_region` 暂时用收集页起点的 `lazy_free` 覆盖实例 `_free`，在 `finally` 中恢复并把收集项拼回 `free_slots`（`python/minisgl/scheduler/cache.py`，93–104）。这说明“从树/请求逻辑上已释放”与“当前 free list 已可立刻再分配”在该上下文中是两个时点。

## exact_source_anchors

| 阅读问题 | 精确锚点 | 可从该锚点证实的事实 |
| --- | --- | --- |
| KV 的张量到底怎样排布？ | `python/minisgl/kvcache/mha_pool.py:MHAKVCache.__init__`（16–37）、`store_kv`（45–56） | 物理 buffer 的维度、TP local KV heads 的计算、按扁平 location 写入。 |
| 统一抽象承诺了什么？ | `python/minisgl/kvcache/base.py:BaseKVCachePool`（10–37）、`BaseCacheHandle`（40–45）、`BasePrefixCache`（67–135） | K/V 接口、handle 的 `cached_len`/index、锁与 eviction 的安全契约。 |
| 如何选择 naive/radix？ | `python/minisgl/kvcache/__init__.py:SUPPORTED_CACHE_MANAGER`、`create_naive_cache`、`create_radix_cache`、`create_prefix_cache`（24–62） | 名称注册与构造路径。 |
| naive 到底禁用了什么？ | `python/minisgl/kvcache/naive_cache.py:NaivePrefixCache.match_prefix/insert_prefix/size_info/evict`（16–45） | 命中/插入均为 0 长度；非零 eviction 不支持。 |
| 谁拥有请求表行和 token 镜像？ | `python/minisgl/scheduler/table.py:TableManager.__init__/allocate/free`（4–21） | 一行一个 `table_idx`；`token_pool` 初始为 0，供 dummy request 亦可读。 |
| free list 的元素为何不是页号？ | `python/minisgl/scheduler/cache.py:CacheManager.__init__`（16–25）、`_page_to_token`（119–124） | free list 为页对齐的 token 起点；后续展开为每 token location。 |
| 命中怎样进入表？ | `python/minisgl/scheduler/prefill.py:PrefillAdder._try_allocate_one`（39–63） | 匹配、双检查、lock、表行分配、复制 token ids 与 matched indices 的顺序。 |
| 新页如何写入表？ | `python/minisgl/scheduler/cache.py:allocate_paged/_allocate/_write_page_table`（42–53、106–146） | 页数计算、逐出补页、pinned host 元数据和 non-blocking device scatter。 |
| radix 的键、部分匹配、split 在哪里？ | `python/minisgl/kvcache/radix_cache.py:RadixTreeNode.set_parent/get_match_len/split_at`（34–81）、`_get_key_fn`（234–237）、`_tree_walk`（205–231） | child key 取首页 token；匹配向下取整到 page size；部分匹配会 split。 |
| 锁怎样改变账目？ | `python/minisgl/kvcache/radix_cache.py:RadixPrefixCache.lock_handle/size_info`（113–130、180–185） | 从 node 到 root 的 ref-count；0↔1 时在 evictable/protected 间移动长度。 |
| 插入、重叠与尾部如何收尾？ | `python/minisgl/scheduler/cache.py:CacheManager.cache_req`（55–79）、`RadixPrefixCache.insert_prefix`（136–146） | 插入只留整页，indices clone；重复部分和 finished tail 的释放分支。 |
| 逐出是否严格等量？ | `python/minisgl/kvcache/base.py:BasePrefixCache.evict`（108–122）、`python/minisgl/kvcache/radix_cache.py:RadixPrefixCache.evict`（148–175） | 合同允许 oversupply；实现按无锁叶的 LRU 逐出并可向父级联。 |
| 当前有哪些自检？ | `python/minisgl/scheduler/cache.py:CacheManager.check_integrity/lazy_free_region`（81–104）、`python/minisgl/kvcache/radix_cache.py:RadixPrefixCache.check_integrity`（187–188） | 前者检查页数和对齐；后者现在是空实现。 |

## invariants_and_failure_modes

### 代码可直接支持的不变量

1. **handle 使用前必须锁定。** 这是 `BasePrefixCache.lock_handle`/`match_prefix` 的明确 API 约束，而非调用方约定（`python/minisgl/kvcache/base.py`，69–88）。`PrefillAdder._try_allocate_one` 在读取 `handle.get_matched_indices()` 之前已调用 `cache_manager.lock(handle)`（`scheduler/prefill.py`，52、57–61）。

2. **缓存树只持有页对齐的 prefix。** `RadixPrefixCache.insert_prefix` 先 `align_down(len(input_ids), page_size)`；`_tree_walk` 也把 matcher 返回长度下取整到同一 page size（`python/minisgl/kvcache/radix_cache.py`，136–139、217–220）。因此任何“半页共享”的讲法都不符合此实现。

3. **每个 radix 节点的 token key/value 长度一致。** `RadixTreeNode.set_key_value` 断言 `len(key) == len(value)`（`python/minisgl/kvcache/radix_cache.py`，34–38）。这是把逻辑 token 序列映射到物理 raw locations 的局部一致性条件。

4. **`free_slots` 始终以页起点表达。** 初始化即为 `arange(num_pages) * page_size`；free 和 eviction 回收都切片 `indices[::page_size]`（`python/minisgl/scheduler/cache.py`，16–21、106–117）。当 `page_size > 1`，`check_integrity` 断言所有 free slot 对 page size 取模为 0（81–91）。

5. **页数账目在 `CacheManager.check_integrity` 的模型里封闭。** 它要求 `len(free_slots) + prefix_cache.size_info.total_size // page_size == num_pages`（`python/minisgl/scheduler/cache.py`，81–89）。这只计算 free list 与 prefix cache 两侧的页，不应被误称为对所有正在运行请求或 radix 结构的全面验证。

6. **锁引用计数不得为负。** `RadixPrefixCache.lock_handle(..., unlock=True)` 每层 `ref_count -= 1` 后断言非负；从 0→1 和 1→0 的边界分别迁移长度账目（`python/minisgl/kvcache/radix_cache.py`，113–130）。

7. **必要页数不足必须由可逐出 cache 补足或失败。** `_allocate` 在 free pages 不足时请求差额并断言回收后页数足够（`python/minisgl/scheduler/cache.py`，106–113）；radix `evict` 在请求大于 `evictable_size` 时断言失败（`kvcache/radix_cache.py`，148–175）。

8. **页表 scatter 的形状必须自洽。** `_write_page_table` 用 `offset == needed_tokens` 断言元数据填充量等于已分配 token locations 数（`python/minisgl/scheduler/cache.py`，127–146）。

### 从代码结构可推导、应在教材中明确标注的失败模式

- **未锁 handle 后读取 index：可能读到已被逐出的物理位置。** 这是 `BasePrefixCache` docstring 直接警告的风险（`python/minisgl/kvcache/base.py`，69–88）。教学中不要弱化为“可能命中率下降”；它是地址有效性风险。
- **漏解锁或漏锁：容量账目会失真。** 路径节点直到 ref-count 回到 0 才从 protected 转回 evictable（`python/minisgl/kvcache/radix_cache.py:lock_handle`，113–130）。因此漏解锁会人为缩小 `available_size`，错配/重复解锁则触发负计数断言；这是根据该账目逻辑作出的推论。
- **把 raw location 当 page id：会使下一层索引语义错误。** storage 的第一维是 `num_pages * page_size`（`MHAKVCache._storage_shape`，`mha_pool.py`，37），而 `_page_to_token` 明确生成每页内的连续位置（`scheduler/cache.py`，119–124）。因此教材中若把 table entry 画成 page id，图将遗漏 in-page offset；这是由两处实现联合作出的推论。
- **把表行重用当成物理页释放：会混淆两份 allocator。** `TableManager.free` 仅归还行号（`scheduler/table.py`，20–21），而 `CacheManager._free` 才将页起点拼回 device free list（`scheduler/cache.py`，115–117）；两者必须在讲义图中有不同颜色。这是代码结构结论。
- **并发/交错插入下未释放 duplicate 区域会泄漏物理页。** `cache_req` 的注释明确指出 `[old_handle.cached_len, cached_len)` 是“prefill 时不在 cache、后来被其他请求 cache”的已分配区，必须 `_free`（`scheduler/cache.py`，55–74）。
- **把尾部当成 radix 条目会破坏整页边界。** `insert_prefix` 截断到整页，而 `cache_req` 对 finished request 释放 `new_handle.cached_len:` 之后尾部（`radix_cache.py`，136–146；`scheduler/cache.py`，75–79）。
- **预期 eviction 精确回收 N 个 token 是错误的。** 抽象合同允许 actual evict size 更大（`kvcache/base.py`，108–122）；实现以 leaf node 整段删除并在满足需求时停止（`radix_cache.py`，160–175）。
- **把 `check_integrity()` 当作 radix 验证会留下盲区。** `CacheManager.check_integrity` 会调用 prefix cache 的同名方法，但 radix 实现目前为 `pass`（`scheduler/cache.py`，81–82；`kvcache/radix_cache.py`，187–188）。因此它只能提供页数/对齐级别的检查，不能证明树结构完全正确；后半句是对空实现的直接边界说明。

## pedagogical_story

以下是**教学叙事提案**，不描述新增实现。

先给学生一个共享 system prompt 的场景：请求 A 已经写过 `[S0,S1,S2,S3,S4]`，请求 B 到达时也从相同前缀开始。将系统分成三张可移动的卡：

1. **仓库卡（物理 KV pool）**：每个页有 `page_size` 个格；格号是可写入的 raw token location。讲者可用 `MHAKVCache.store_kv` 展平 storage 的实现作为“格号直达仓库”的证据，而非把它说成抽象 page lookup（`python/minisgl/kvcache/mha_pool.py`，45–56）。
2. **索引卡（每请求表行）**：请求 B 拿到新的 `table_idx`，但前缀格号可以与 A 相同；这正是 `PrefillAdder._try_allocate_one` 把 `get_matched_indices()` 复制进新 `page_entry` 的行为（`python/minisgl/scheduler/prefill.py`，56–61）。
3. **借阅卡（radix handle）**：B 先 match 得到凭证、lock 后才可抄索引；路径上的共同节点都被 ref-count 保护（`python/minisgl/kvcache/base.py`，69–88；`kvcache/radix_cache.py`，113–130）。

然后故意让 B 的最后一个 prompt token 与 A 不同。让学生预测：相同的整页前缀可以复用，但在页边界前停止；B 的尾巴需要新页。证据是 `_tree_walk` 和 `insert_prefix` 都对长度执行 page-size 对齐（`python/minisgl/kvcache/radix_cache.py`，136–139、217–220），而 `allocate_paged` 恰从 cached/device 长度的页上取整范围新增页（`python/minisgl/scheduler/cache.py`，42–53）。

最后让 A 完成、B 仍运行。重点不是“谁先结束”，而是“哪部分仍受锁保护”：`cache_req` 解旧 handle，插入后对未完成请求锁新 handle；完成请求释放不可纳入新 handle 的尾部（`python/minisgl/scheduler/cache.py`，67–79）。当另一请求需要新页，才触发无锁叶的 LRU eviction（`scheduler/cache.py`，106–113；`kvcache/radix_cache.py`，148–175）。这可自然收束到本章论题：**复用不是免费共享，而是用锁、页粒度和可逐出性维持地址所有权。**

## demo_or_reading_lab

### 演示 1：纸上分页映射（提案，10 分钟）

- 设 `page_size=4`，画出 free page 起点 `[0, 4, 8]`；由学生按 `CacheManager._page_to_token` 推出页起点 `4` 对应 raw locations `[4,5,6,7]`（`python/minisgl/scheduler/cache.py`，119–124）。
- 给逻辑位置 `pos=0..5` 的请求，要求学生用 `ceil(cached_len/page_size)` 与 `ceil(device_len/page_size)` 计算 `allocate_paged` 会写的页范围（`scheduler/cache.py`，42–53）。
- 检查题：若学生写“表项 `[1,1,1,1]` 代表第二页”，让他们回到 `MHAKVCache.store_kv` 的展平索引和 `_page_to_token` 纠正为 `[4,5,6,7]`（`mha_pool.py`，45–56；`scheduler/cache.py`，119–124）。

### 演示 2：共享前缀与两次容量检查（提案，12 分钟）

- 教师给定一棵以整页为边的树和一组 `evictable/protected/free_slots` 数字；学生扮演 `PrefillAdder._try_allocate_one`，依次完成 match、估算、lock、第二次估算、分配表行、复制前缀（`python/minisgl/scheduler/prefill.py`，39–63）。
- 问题：为什么第一次“容量够”不代表第二次也够？答案必须引用 `available_size` 的定义和 `lock_handle` 的账目转移（`scheduler/cache.py`，32–34；`radix_cache.py`，113–130）。
- 变体：把 cache type 换成 `naive`，要求学生预测 0 长度命中与没有可逐出 prefix 的行为，再核对 `NaivePrefixCache`（`python/minisgl/kvcache/naive_cache.py`，26–45）。

### 阅读实验：从一个真实请求跟到回收（提案，20–25 分钟）

学生以三个角色分组，每组只读一条连续路径，最后拼图：

1. **准入组：** `scheduler/prefill.py:PrefillAdder._try_allocate_one` → `_add_one_req`，记录 handle 何时锁、`table_idx` 何时取得、哪些 tensor copy 发生（第 39–90 行）。
2. **物理位置组：** `scheduler/cache.py:CacheManager.allocate_paged` → `_allocate` → `_page_to_token` → `_write_page_table`，记录 page-start 如何变成表中 token location（第 42–53、106–146 行）。
3. **缓存生命周期组：** `scheduler/cache.py:CacheManager.cache_req` → `kvcache/radix_cache.py:insert_prefix/lock_handle/evict`，画四段 region 与 LRU 候选变化（第 55–79；113–175）。

所有组的提交格式是“一个箭头图 + 每个箭头至少一个 `路径:符号`”，不得只交自然语言概述。教师可用 `CacheManager.check_integrity` 的页数式让全班做终检，但须同时指出 radix 的 `check_integrity` 为空实现（`scheduler/cache.py`，81–91；`kvcache/radix_cache.py`，187–188）。

## misconceptions

| 易错说法 | 纠正表述（代码事实） | 阅读证据 |
| --- | --- | --- |
| “page table 里存 page id。” | 当前分配器先保留页起点、再展开为连续 token locations，最后写表；KV 写入将 layer storage 展平后以 `out_loc` 索引。 | `scheduler/cache.py:_page_to_token/_write_page_table`（119–146）；`kvcache/mha_pool.py:MHAKVCache.store_kv`（45–56）。 |
| “拿到 match 结果马上就能用。” | 返回 index 的有效性以锁 handle 为前提；接口文档明说未锁时它可被 eviction。 | `kvcache/base.py:BasePrefixCache.lock_handle/match_prefix`（69–93）。 |
| “一条请求占一个物理页。” | 表行与页是不同资源：一条 request 获得 `table_idx`，其 logical positions 可跨多页；页数按 cached/device length 的上取整差值计算。 | `scheduler/table.py:TableManager.allocate`（17–18）；`scheduler/cache.py:CacheManager.allocate_paged`（42–53）。 |
| “radix 能复用任意长度的共同 token。” | tree walk 的 match length 与 insert length 都向下对齐 page size，非整页尾部不插入 trie。 | `kvcache/radix_cache.py:RadixPrefixCache._tree_walk/insert_prefix`（136–146、205–231）。 |
| “lock 只锁叶子节点。” | 实现从 handle node 一直走到 root（不含 root）并改变每个节点的 ref count。 | `kvcache/radix_cache.py:RadixPrefixCache.lock_handle`（113–130）。 |
| “evict 总会精确释放请求数量。” | 接口允许实际大小超过请求量；实现以完整无锁叶节点为单位弹出。 | `kvcache/base.py:BasePrefixCache.evict`（108–122）；`kvcache/radix_cache.py:RadixPrefixCache.evict`（148–175）。 |
| “free 以后本轮任何地方都可立即重新分配。” | `lazy_free_region` 可将 `_free` 暂时变成收集，直到上下文退出才合并回 `free_slots`。 | `scheduler/cache.py:CacheManager.lazy_free_region`（93–104）。 |
| “naive 就是 radix 的慢实现。” | naive 的 match/insert 均返回 0 长度，非零 `evict` 抛出 `NotImplementedError`；它是关闭 prefix reuse 的策略，不是同一树算法的低性能版本。 | `kvcache/naive_cache.py:NaivePrefixCache`（16–45）。 |

## exercises

以下均为**教材练习提案**；标准答案只能据所列代码推导，不要求学生运行 GPU 服务。

1. **位置表追踪。** 给定 `page_size=4`、`cached_len=4`、`device_len=7`，按 `CacheManager.allocate_paged` 写出 `first_page`、`last_page`、新增页数及此页展开后的四个 raw locations。再解释为什么最后一个 logical token 的位置仍被分配。（证据：`python/minisgl/scheduler/cache.py:CacheManager.allocate_paged/_page_to_token`，42–53、119–124。）
2. **双重准入证明。** 用一个数值例子说明 lock 之后 `available_size` 会下降；指出在 `_try_allocate_one` 中若二检失败，哪一个操作回滚。（证据：`python/minisgl/scheduler/prefill.py:PrefillAdder._try_allocate_one`，44–54；`python/minisgl/kvcache/radix_cache.py:RadixPrefixCache.lock_handle`，113–130。）
3. **四区回收表。** 学生为 `cache_req` 的四个切片区间填写“原先谁拥有、现在是否 free、为何”；特别解释 duplicate region。（证据：`python/minisgl/scheduler/cache.py:CacheManager.cache_req`，55–79。）
4. **树分裂手推。** 以 page_size=2 的 token 序列构造 partial match，手画 `_tree_walk` 到 `split_at` 的 parent/child 改接，并说明为何 `split_at` 保留原 `ref_count`。（证据：`python/minisgl/kvcache/radix_cache.py:RadixTreeNode.split_at`，69–81；`RadixPrefixCache._tree_walk`，205–231。）
5. **LRU 不是 FIFO。** 标出连续两次 `_tree_walk` 中哪些完整命中节点会更新 timestamp，并预测 `evict` 的候选排序依据。（证据：`python/minisgl/kvcache/radix_cache.py:_tree_walk/__lt__/evict`，83–84、148–175、205–231。）
6. **断言审计（进阶）。** 将 `CacheManager.check_integrity` 所能验证、不能验证的条件分两列；必须发现 radix `check_integrity` 目前为空。（证据：`python/minisgl/scheduler/cache.py`，81–91；`python/minisgl/kvcache/radix_cache.py`，187–188。）

## recommended_expanded_structure

下面是后续作者可采用的**章节结构提案**，不改变当前仓库代码。

1. **问题开场：为什么每个生成 token 都需要历史，而重复 prompt 不该重复占满显存？** 用一个共享 system prompt 的图引入“物理页、请求表、共享树”三层。
2. **第一节：从 `out_loc` 看物理 KV。** 先读 `MHAKVCache.__init__` 和 `store_kv`，建立 `[2,L,P,page,H,D]` 与扁平 raw location 的对应，明确 local KV heads 与 TP 的存在但不深讲分布式（`python/minisgl/kvcache/mha_pool.py`，16–68）。
3. **第二节：page table 和 token_pool 不是一个东西。** 用 `TableManager` 与 `CacheManager` 并排图，讲“表行 allocator”与“页 allocator”不同所有权（`python/minisgl/scheduler/table.py`，4–21；`scheduler/cache.py`，15–25）。
4. **第三节：paged allocation 的算术。** 逐行走 `allocate_paged → _allocate → _page_to_token → _write_page_table`，配 page_size=4 的手算题；将 pinned host metadata/non-blocking transfer 作为阅读注记，不夸大为已量化的性能结论（`python/minisgl/scheduler/cache.py`，42–53、106–146）。
5. **第四节：radix 的 page-granular 最长前缀。** 读 node、key function、`_tree_walk`、split；先解释 tokenizer 序列为何是 key，再解释整页对齐的边界（`python/minisgl/kvcache/radix_cache.py`，17–84、205–237）。
6. **第五节：handle 是地址有效性的契约。** 把 `match → lock → copy matched indices` 与二阶段 admission 连成一条时序线；再深入 path ref-count 与 `SizeInfo`（`kvcache/base.py`，48–93；`scheduler/prefill.py`，39–63；`radix_cache.py`，113–130）。
7. **第六节：插入、尾部、完成与并发重叠。** 以 `cache_req` 的四个 region 为核心，解释 clone、duplicate free、finished tail、继续运行时 re-lock（`scheduler/cache.py`，55–79；`radix_cache.py`，136–146）。
8. **第七节：何时逐出、怎样自证。** 讲 demand-driven leaf-LRU eviction、oversupply 和 `lazy_free_region`；最后用现有页数/对齐检查及 radix 检查为空的限制收束（`scheduler/cache.py`，81–117；`kvcache/base.py`，108–122；`radix_cache.py`，148–188）。
9. **收束实验与跨章回指。** 让学生从 prefill 准入走到 KV 写入；回接课程第 02 章的 request 长度和第 05 章 attention 消费者。该跨章安排是教学提案，具体课程目标见纲要第 02、04、05 章（`.rive-artifacts/minisglang-teaching-20260710/manual/teaching-manual-outline.md`）。

## limitations

### 代码与证据的边界

- `RadixPrefixCache.check_integrity` 是 `pass`，`reset` 抛 `NotImplementedError`；不要在扩写教材中声称 radix 树已经有完备自检或可热 reset（`python/minisgl/kvcache/radix_cache.py`，177–188）。
- `NaivePrefixCache.evict` 对非零请求抛 `NotImplementedError`；因此把 naive 用于需要通过 prefix eviction 扩容的路径是否可行，必须结合调用配置和更高层流程另行验证，本文不作泛化结论（`python/minisgl/kvcache/naive_cache.py`，29–35）。
- `BasePrefixCache` 的 `MatchResult` 只有 `cuda_handle`，并有“TODO: support HiCache”；本文只讲当前 GPU-resident 的可见接口，不推断 host/offload cache 行为（`python/minisgl/kvcache/base.py`，62–64）。
- `MHAKVCache` 的工厂注释写有支持其他变体（如 MLA）的 TODO；本文的布局图仅适用于当前 `MHAKVCache` 路径，不能用作所有模型 cache 的 API 合同（`python/minisgl/kvcache/__init__.py`，27–44）。
- `_write_page_table` 使用 pinned host 张量和 non-blocking `.to`（`python/minisgl/scheduler/cache.py`，133–146），但本笔记没有测量 stream 时序、同步点或吞吐收益；任何性能解释应以独立 profiling 证据补充。
- `CacheManager.check_integrity` 的页数式不单独列入当前活跃请求持有、延迟释放区或每个 radix 节点的双向 parent/child 完整性；这是其直接可见检查范围所限，不是对运行时所有权正确性的证明（`python/minisgl/scheduler/cache.py`，81–104；`python/minisgl/kvcache/radix_cache.py`，187–188）。

### 给扩写作者的写作边界（提案）

- 将“当前代码事实”“可视化模型”“可选改进”分栏。诸如 host cache、指标、自动化完整性审计、生产逐出策略均应标为未来讨论，不能倒写成仓库已有能力。
- 所有示意图都应同时画出 logical position、raw token location、page start、`table_idx` 四种标签；这会显著降低把“请求表行”和“物理页”混为一谈的概率。
- 逐出一节应明确反例：锁住热点前缀会牺牲当前可逐出容量。不要只用“缓存总是省内存”的线性叙述。
