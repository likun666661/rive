# 第 02 章研究笔记：调度器中的请求生命周期

> 面向后续扩写教材作者的源码研究材料，非最终教材章节。本文中的“代码事实”只陈述可由当前仓库定位的实现；“教学提案”是课程设计建议，不能反推为项目已经实现的功能。行号以本次工作区版本为准。

## scope

### 代码事实

- 本章的中心是 `Scheduler`：它组合 `TableManager`、`CacheManager`、`DecodeManager`、`PrefillManager`，并从 `SchedulerIOMixin` 获得消息收发；构造顺序和成员可见于 `python/minisgl/scheduler/scheduler.py:45-76`，而 I/O mixin 定义于 `python/minisgl/scheduler/io.py:15-65`。
- 请求状态的基础词汇是 `SamplingParams`、`Req`、`Batch`、`Context`；它们均定义于 `python/minisgl/core.py:15-136`。`Req` 保存 CPU 上的 `input_ids`、表行 `table_idx`、缓存长度、输出上限、uid、采样参数与缓存句柄（`core.py:28-42`）。
- 本笔记覆盖请求从 `UserMsg` 入队到 prefill / chunked prefill / decode，再到 `DetokenizeMsg` 或 abort 的路径，及其表行、页表和前缀缓存资源释放。入口分派在 `Scheduler._process_one_msg`（`scheduler.py:169-198`），资源释放集中于 `_free_req_resources`（`scheduler.py:200-202`）。
- overlap 是默认运行路径：`Scheduler.run_forever` 在未设置 `ENV.DISABLE_OVERLAP_SCHEDULING` 时迭代 `overlap_loop`；关闭时转为 `normal_loop`（`scheduler.py:108-131`）。

### 教学提案

本章应只建立“一个请求是一组跨 CPU、GPU 表项、KV 页和缓存句柄的同步记账”这一心智模型；不把它讲成通用或生产级调度器的完整规范。模型层、attention 算子、CUDA Graph 内部和前缀树算法可在后续章节展开。

## teaching_goal_alignment

| 课程大纲目标 | 可教的代码事实 | 建议验收产物 |
|---|---|---|
| 区分 host/device/cache 长度 | `Req.__post_init__` 设定 `device_len` 与 `max_device_len`，并断言 `0 <= cached_len < device_len <= max_device_len`；`extend_len = device_len - cached_len`（`python/minisgl/core.py:38-50`）。 | 学生能写出三段长度各自表示什么，并从不等式推出一次 batch 至少有一个待计算 token。 |
| 描述一次 token 推进 | engine 对真实请求逐一调用 `Req.complete_one`，随后 scheduler 在 CPU 拷贝完成后调用 `Req.append_host`（`python/minisgl/engine/engine.py:191-206`; `python/minisgl/scheduler/scheduler.py:138-156`）。 | 学生画出一次 decode 前后 `cached_len/device_len/len(input_ids)` 的变化。 |
| 区分 prefill、分块 prefill、decode | `PrefillManager.schedule_next_batch` 产生 `phase="prefill"`，`DecodeManager.schedule_next_batch` 产生 `phase="decode"`；`ChunkedReq.can_decode` 固定为 false（`python/minisgl/scheduler/prefill.py:23-30,126-151`; `python/minisgl/scheduler/decode.py:32-35`）。 | 学生能解释为什么分块请求本轮不应向用户发送 sampled token。 |
| 解释 overlap 的边界 | `overlap_loop` 先在 engine stream 发起新 forward，再处理上一批；CPU 读取前等待 `copy_done.synchronize()`（`python/minisgl/scheduler/scheduler.py:83-106,138-143`）。 | 学生能标出 batch N 计算和 batch N-1 结果处理的并行关系。 |
| 以不变量审查资源回收 | 完成和 abort 都最终走 `TableManager.free` 与 `CacheManager.cache_req(..., finished=True)`；cache manager 在空闲时检查页数守恒（`scheduler.py:190-202`; `python/minisgl/scheduler/table.py:17-21`; `python/minisgl/scheduler/cache.py:81-91`）。 | 学生针对一次 EOS、一次 pending abort 和一次 decode abort 标资源所有权。 |

## concept_to_execution_path

### 1. 到达、限制和轻量排队（代码事实）

1. `Scheduler._process_one_msg` 收到 `UserMsg` 后以 `engine.max_seq_len` 检查 prompt；没有输出空间时仅记录 warning 并返回，超出的 `sampling_params.max_tokens` 会被截断（`python/minisgl/scheduler/scheduler.py:175-189`）。因此“消息被接收”尚不表示已分配表行或 KV 页。
2. `PrefillManager.add_one_req` 仅将 `PendingReq(uid, input_ids, sampling_params)` append 到 `pending_list`（`python/minisgl/scheduler/prefill.py:121-124`）；`PendingReq` 是可带回 `ChunkedReq` 指针的轻量容器（`python/minisgl/scheduler/utils.py:14-27`）。
3. 单卡 I/O 在 blocking 时从 tokenizer 队列取一条后清空已到达消息；多 TP rank 时 rank 0 经 ZMQ 转发原始字节，且以 CPU process group 广播消息数量，非主 rank 按该数量从订阅端读取（`python/minisgl/scheduler/io.py:79-122`）。这使各 rank 接受相同数目的调度消息；只有主 rank 将结果送 tokenizer，非主 rank 的 reply 是 no-op（`io.py:124-133`）。

### 2. prefill 准入、前缀命中与分块（代码事实）

1. `_schedule_next_batch` 固定尝试 `PrefillManager.schedule_next_batch(prefill_budget)`，为空才调用 `DecodeManager.schedule_next_batch`，所以当前策略是 prefill 优先（`python/minisgl/scheduler/scheduler.py:219-225`）。`prefill_budget` 来自 `SchedulerConfig.max_extend_tokens`，默认 8192（`scheduler.py:67-72`; `python/minisgl/scheduler/config.py:14-18`）。
2. `PrefillManager` 建立 `PrefillAdder` 时，将 `DecodeManager.inflight_tokens` 作为 `reserved_size`；它包括每个 running request 的剩余输出长度和 `(page_size - 1)` 的页内余量（`python/minisgl/scheduler/prefill.py:126-136`; `python/minisgl/scheduler/decode.py:27-30`）。
3. 首次准入先检查可用表行，再用 `CacheManager.match_req` 在 `input_ids[:input_len-1]` 上匹配前缀。最后一个输入 token 被排除；此处是实现事实，教材不应把它泛化为所有 KV-cache 的必然规则（`python/minisgl/scheduler/prefill.py:39-63`; `python/minisgl/scheduler/cache.py:27-30`）。
4. 准入估算为 `extend_len + output_len`。它在 lock 前后各检查一次缓存可用量；第二次失败会 unlock 并返回 `None`。成功时才 `TableManager.allocate()`，再将命中 token 和 page indices 复制进该表行（`python/minisgl/scheduler/prefill.py:43-63`）。
5. 每个候选的 chunk 是 `min(token_budget, remain_len)`；未包含剩余 prompt 的候选构造为 `ChunkedReq`。本轮后的 `PendingReq.chunked_req` 被保留，且下一轮的 chunked pending 被置于队头（`python/minisgl/scheduler/prefill.py:65-90,139-151`）。第一个无法加入的 pending request 会 `break`，即当前实现存在队头阻塞，而非寻找全局最优组合（`prefill.py:139-150`）。

### 3. Batch 准备：表行变成 GPU 元数据（代码事实）

1. `_prepare_batch` 的固定顺序为 `graph_runner.pad_batch`、`cache_manager.allocate_paged(batch.reqs)`、positions/input/write mapping、从 `page_table[input_mapping]` 得到 `out_loc`、attention metadata、sampler args（`python/minisgl/scheduler/scheduler.py:204-217`）。
2. CUDA Graph 可用条件是 decode 且真实 batch size 不超过 `max_graph_bs`；`pad_batch` 选不小于 batch 的首个图尺寸，并以 `dummy_req` 补齐 `padded_reqs`（`python/minisgl/engine/graph.py:149-166`）。因此 padding 影响位置和 input mapping（它们遍历 `padded_reqs`），但 `_make_write_tuple` 只遍历真实 `batch.reqs`（`python/minisgl/scheduler/scheduler.py:236-267`）。
3. `CacheManager.allocate_paged` 以 `ceil(cached_len/page_size)` 到 `ceil(device_len/page_size)` 计算新增页，必要时分配页并写 page table（`python/minisgl/scheduler/cache.py:42-53,127-146`）。`TableManager.token_pool` 与 page table 同形且以 token id 0 初始化，注释明确说明这是为了 dummy request 可安全读取（`python/minisgl/scheduler/table.py:4-21`）。

### 4. forward、采样和请求状态推进（代码事实）

1. `_forward` 从 `token_pool[input_mapping]` 填入 `batch.input_ids`，调用 `Engine.forward_batch`，把 GPU sampled token 写入 `token_pool[output_mapping]`，最后按 `can_decode` 更新 running set（`python/minisgl/scheduler/scheduler.py:227-233`）。`output_mapping` 对不能 decode 的真实请求写 `-1`（`scheduler.py:262-267`）。
2. `Engine.forward_batch` 断言当前 stream 是 engine stream，在 `Context.forward_batch(batch)` 作用域内走 graph replay 或模型 eager forward；离开作用域后对每个真实 request 执行 `complete_one`，对 `logits[:batch.size]` 采样并异步复制到 CPU，再记录 event（`python/minisgl/engine/engine.py:191-206`）。
3. `Context.forward_batch` 禁止嵌套，且 finally 一定清空 `_batch`；访问 `Context.batch` 时没有活动 batch 会断言（`python/minisgl/core.py:100-122`）。这是模型代码读取“当前 batch”时的动态作用域边界。
4. `Req.complete_one` 令 `cached_len=device_len` 后递增 `device_len`；非 chunk request 随后由 `_process_last_data` 的 `append_host(next_token)` 把 CPU token 序列补齐（`python/minisgl/core.py:52-61`; `python/minisgl/scheduler/scheduler.py:142-152`）。

### 5. 结果、继续 decode 与结束（代码事实）

1. `_process_last_data` 必须先等 `copy_done`，才读取 `next_tokens_cpu`。它忽略 `ChunkedReq`；对普通请求 append token，以 `not req.can_decode` 或 EOS（除非 `ignore_eos`）计算 finished，并组织 `DetokenizeMsg`（`python/minisgl/scheduler/scheduler.py:138-156`）。
2. 对完成 request，scheduler 从 decode manager 移除、释放资源；对“本批为 prefill 且还未结束”的普通 request，调用 `cache_req(..., finished=False)` 将可缓存前缀合并，并保留/锁住新句柄（`scheduler.py:158-167`; `python/minisgl/scheduler/cache.py:55-79`）。
3. `DecodeManager.filter_reqs` 将已有 running set 与本批 reqs 合并，再按 `can_decode` 过滤；`schedule_next_batch` 以 uid 排序后形成 decode batch（`python/minisgl/scheduler/decode.py:14-15,32-39`）。`ChunkedReq.can_decode=False` 且 `append_host` 会抛异常，正是它不被送入 decode/结果路径的双重保护（`python/minisgl/scheduler/prefill.py:23-30`）。

### 6. abort 与资源回收（代码事实）

1. `AbortBackendMsg` 先在 `PrefillManager.pending_list` 中找 uid，再在 `DecodeManager.running_reqs` 中找；找到 request 才调用 `_free_req_resources`（`python/minisgl/scheduler/scheduler.py:190-195`; `python/minisgl/scheduler/prefill.py:153-158`; `python/minisgl/scheduler/decode.py:20-25`）。因此它移除的是调度器可见容器中的请求，而该分支本身没有取消已经提交的 `Engine.forward_batch`。
2. `_free_req_resources` 总是先归还 `table_idx`，再以 `finished=True` 调 `cache_req`（`python/minisgl/scheduler/scheduler.py:200-202`）。后者插入 page 对齐前缀、unlock old handle、释放已被其他请求缓存的重叠区，以及完成请求的尾部区（`python/minisgl/scheduler/cache.py:55-79`; `python/minisgl/kvcache/radix_cache.py:136-146`）。
3. `_process_last_data` 的 `finished_reqs` 是 overlap 情况下的重复释放防护：本轮新完成集覆盖旧集，若当前 req 已在旧集则跳过 release（`python/minisgl/scheduler/scheduler.py:145-166`）。释放操作放在 `lazy_free_region` 中，暂存本轮 free 页，退出时一次性拼回 `free_slots`（`python/minisgl/scheduler/cache.py:93-105`）。

## exact_source_anchors

| 主题 | 精确锚点 | 可从该处确认的内容 |
|---|---|---|
| 请求长度与推进 | `python/minisgl/core.py:28-61` — `Req.__post_init__`, `remain_len`, `extend_len`, `complete_one`, `append_host`, `can_decode` | 三长度关系、一次 forward 后的“一个 token 缝隙”、CPU 序列追加。 |
| 批与动态上下文 | `python/minisgl/core.py:71-122` — `Batch`, `Context.forward_batch` | phase、真实/填充 request 区分，以及禁止嵌套的 active batch。 |
| 循环和 streams | `python/minisgl/scheduler/scheduler.py:83-131` — `overlap_loop`, `normal_loop`, `run_forever` | 重叠循环的顺序、禁用 overlap 的分支。 |
| 结果、结束、释放 | `python/minisgl/scheduler/scheduler.py:138-167,200-233` — `_process_last_data`, `_free_req_resources`, `_prepare_batch`, `_schedule_next_batch`, `_forward` | event 等待、EOS 判断、double-free guard、prefill 优先、batch 准备与 token 回写。 |
| admission / chunk | `python/minisgl/scheduler/prefill.py:23-113,126-162` — `ChunkedReq`, `PrefillAdder`, `PrefillManager` | 分块类型的保护、锁前后两次容量检查、队头阻塞、chunk 续跑与 abort。 |
| decode 所有权 | `python/minisgl/scheduler/decode.py:9-39` — `DecodeManager` | running set、按 uid 决定 batch 顺序、inflight reservation、abort。 |
| 页与缓存资源 | `python/minisgl/scheduler/cache.py:15-146` — `CacheManager` | 页分配、前缀匹配、lock/unlock、cache/final free、lazy free 与守恒检查。 |
| 表行与 token pool | `python/minisgl/scheduler/table.py:4-21` — `TableManager` | 表行 free list、token pool 的初始值与 dummy 用途。 |
| I/O 与 TP | `python/minisgl/scheduler/io.py:15-133` — `SchedulerIOMixin` | 单/多 rank receive、count broadcast、只有 rank 0 回发。 |
| scheduler 配置 | `python/minisgl/scheduler/config.py:14-41` — `SchedulerConfig` | prefill 上限、cache type、offline mode 和 IPC 地址派生。 |
| engine 交界 | `python/minisgl/engine/engine.py:23-26,191-206` — `ForwardOutput`, `Engine.forward_batch` | 输出快照、stream 断言、采样、异步 D→H 与 event。 |
| CUDA Graph 边界 | `python/minisgl/engine/graph.py:149-170` — `can_use_cuda_graph`, `replay`, `pad_batch`, `destroy_cuda_graphs` | decode-only 判断、padding、graph 销毁先于 NCCL 资源的注释。 |

## invariants_and_failure_modes

### 不变量（代码事实及其直接推论）

| 不变量 | 代码证据 | 若破坏，直接后果 |
|---|---|---|
| `0 <= cached_len < device_len <= max_device_len` | `Req.__post_init__` 的 assert（`python/minisgl/core.py:38-42`）。 | `extend_len=device_len-cached_len` 不再保证至少为 1，位置/页分配的“本轮有工作”假设失效。 |
| 普通 request 的 device 推进与 host 追加成对 | `complete_one` 修改前两项，`_process_last_data` 在 event 后对非 `ChunkedReq` 调 `append_host`（`core.py:52-57`; `scheduler.py:142-152`）。 | 可能使 `device_len` 与 `len(input_ids)` 脱节，下一轮 token/位置读取错误。 |
| chunk request 不采样、不进入 decode | `ChunkedReq.append_host` 抛异常且 `can_decode=False`；`_process_last_data` 显式 skip；`filter_reqs` 以 `can_decode` 过滤（`prefill.py:23-30`; `scheduler.py:147-151,232`; `decode.py:14-15`）。 | 若当作普通 request，host append 会触发异常或向用户泄漏中间 chunk 结果。 |
| cache handle 在使用时受保护，失败准入不遗留 lock | admission 的 lock 后二次检查与 `unlock`；radix lock/unlock 的 ref-count 非负 assert（`prefill.py:50-54`; `python/minisgl/kvcache/radix_cache.py:113-130`）。 | 可驱逐空间统计错误；可能错误 eviction 或永久占用。 |
| page-table 写入与页数一致 | `_write_page_table` 断言所写 token 数等于 allocated 数；空闲检查断言 free pages + cache pages = num_pages（`cache.py:127-146,81-91`）。 | 页表索引/空闲列表脱节，表现为分配不足、泄漏或错误 KV 位置。 |
| CPU 读 sampled token 前等待 D→H 完成 | engine 以 non-blocking copy 后 record event；scheduler `copy_done.synchronize()` 后读取（`engine.py:202-206`; `scheduler.py:142-152`）。 | 读取尚未完成的 CPU tensor，回复 token/状态不能可信。 |
| `Context` 同时只能有一个 active batch | `Context.forward_batch` 的 nested assert 和 finally 清理（`core.py:110-122`）。 | 模型通过 global context 读取 batch 时可能看到被覆盖或泄漏的元数据。 |

### 失效模式与课堂处理

| 失效模式 | 代码中的现状 | 教学上应怎样表述 |
|---|---|---|
| 过长 prompt | `max_output_len <= 0` 时仅 warning 后 return（`python/minisgl/scheduler/scheduler.py:177-183`）。 | 这是 admission drop；本段代码未在此处构造用户可见错误回复。不要称其为“安全截断后继续生成”。 |
| prefill 资源不足 | 无表行、容量预估失败、或 lock 后二次失败都让 `try_add_one` 返回 None；调度在首个失败处 break（`prefill.py:39-63,92-113,139-150`）。 | 强调这是保守 admission 与 FIFO 队头阻塞的取舍，不是最优 packing。 |
| CUDA 结果过早读取 | 引擎返回 `copy_done_event`，scheduler 先 synchronize（`engine.py:203-206`; `scheduler.py:142-143`）。 | “non-blocking copy”描述提交方式，不等于 CPU 可以立即消费。 |
| overlap 下重复释放 | 完成分支检查 `req not in self.finished_reqs`，并以 `new_finished_reqs` 覆盖集合（`scheduler.py:145-166`）。 | 将它讲成当前实现的局部防护；不能据此断言所有 abort/跨轮竞态都已被穷尽证明。 |
| abort 到达已启动 forward | abort 仅从 pending/running 容器删除并释放资源；forward 是另一个已提交调用（`scheduler.py:101-105,190-202`）。 | 把“取消排队/运行集合”与“取消已提交 GPU kernel”分开；后者不是这一段代码展示的能力。 |
| 前缀缓存泄漏或页对齐误判 | `cache_req` 对不同区段分别 free/keep；radix 插入按 `page_size` 向下对齐（`cache.py:55-79`; `radix_cache.py:136-146`）。 | 用区段图而非“命中即永远免费”讲解，并要求学生追踪 old/new handle。 |

## pedagogical_story

### 建议叙事（教学提案）

把一个 request 比作“有四本账的旅客”：

1. **行程单（host `input_ids`）**：CPU 上已知 prompt 与已确认返回的 token。
2. **GPU 工作账（`cached_len` / `device_len`）**：前者说明 KV 已覆盖到哪里，后者说明本轮 model 将看到的序列边界。
3. **座位号（`table_idx`、`page_table`、`token_pool`）**：请求不是直接拥有一整块连续 KV，而是经行号映射到页位置。
4. **公共行李寄存柜凭证（`cache_handle`）**：命中的共享前缀必须锁住，结束后才能把可共享部分归还为可驱逐或释放尾部页。

用下列时间线讲解；箭头对应代码事实，而不是时延承诺：

```text
UserMsg
  -> PendingReq                         (PrefillManager.add_one_req)
  -> Req / ChunkedReq + table_idx+handle (PrefillAdder.try_add_one)
  -> Batch(prefill)                     (PrefillManager.schedule_next_batch)
  -> prepare pages / mappings           (Scheduler._prepare_batch)
  -> Engine.forward_batch               (Req.complete_one; async D->H)
  -> [普通 Req] wait event -> append_host -> DetokenizeMsg
  -> [未结束] DecodeManager.running_reqs -> Batch(decode) -> 重复一 token
  -> [完成 / abort] TableManager.free + CacheManager.cache_req(finished=True)
```

然后把“长 prompt”插入上述故事：它先变成 `ChunkedReq`，本轮只扩展 prompt KV，既不 `append_host` 也不进入 decode；`PendingReq.chunked_req` 持有其资源，下一轮从该处续跑。此叙事应同时指出实现证据：`ChunkedReq` 的两个重写和 `PrefillManager` 的队头重排（`python/minisgl/scheduler/prefill.py:23-30,96-102,139-151`）。

最后再引入 overlap：不要先画“并行魔法”，而要先让学生看到 `ForwardOutput` 含 CPU tensor 与 event（`python/minisgl/engine/engine.py:23-26`）。之后解释 scheduler stream 处理 N-1 的结果时，engine stream 执行 N；`wait_stream` 确保 engine 在开始前等待 scheduler stream 的元数据操作（`python/minisgl/scheduler/scheduler.py:98-105`）。

## demo_or_reading_lab

### 代码阅读实验（教学提案，30–40 分钟）

**材料。** 给学生仅提供以下源码锚点：`Req`（`python/minisgl/core.py:28-61`）、`PrefillAdder._try_allocate_one/_add_one_req`（`python/minisgl/scheduler/prefill.py:39-113`）、`Scheduler._prepare_batch/_forward/_process_last_data`（`python/minisgl/scheduler/scheduler.py:138-167,204-233`）及 `CacheManager.cache_req`（`python/minisgl/scheduler/cache.py:55-79`）。

**活动 A：纸上单请求状态表。** 假设普通 request 的 prompt 三 token、`cached_len=0`、`max_tokens=2`、不命中 EOS。学生只依据 `Req.complete_one` 与 `Req.append_host` 填下表；不要假定 tokenizer 或模型的具体 token 值。

| 时点 | `cached_len` | `device_len` | host `len(input_ids)` | 应引用的符号 |
|---|---:|---:|---:|---|
| 构造后 | 0 | 3 | 3 | `Req.__post_init__` |
| 第一次 engine forward 后 | 3 | 4 | 3 | `Req.complete_one` |
| CPU event 后 | 3 | 4 | 4 | `Req.append_host` |

预期讨论：第二次 decode 的 `extend_len` 为 1，而不是 0；其依据是 `cached_len=3, device_len=4`，不是“每次 decode 天然一个 token”。

**活动 B：调度纸卡。** 准备三张卡：A 为短新 prompt，B 为超过 token budget 的长 prompt，C 为 `DecodeManager.running_reqs` 中的请求。让学生执行 `_schedule_next_batch`：先尝试 prefill；B 被切为 `ChunkedReq` 时保留到 `pending_list` 前部；只有 prefill 无 batch 才轮到 C。依据为 `Scheduler._schedule_next_batch` 和 `PrefillManager.schedule_next_batch`（`scheduler.py:219-225`; `prefill.py:126-151`）。

**活动 C：两 stream 时间线。** 在白板画 batch N-1 和 N：依次标 `receive/process -> schedule -> engine.wait_stream(scheduler stream) -> _forward(N) -> _process_last_data(N-1)`。要求标出 CPU 数据什么时候能读，并在 `copy_done.synchronize()` 处写下理由（`scheduler.py:83-106,138-143`; `engine.py:202-206`）。

**活动 D：资源审计。** 任选“EOS 完成”“pending chunk abort”“decode abort”之一，学生为 `table_idx`、old handle、new handle、页尾分别标所有者。检查答案必须使用 `_free_req_resources`、`cache_req`、`abort_req` 三处源码，而非凭常识（`scheduler.py:190-202`; `cache.py:55-79`; `prefill.py:153-158`; `decode.py:20-25`）。

## misconceptions

| 常见误解 | 更精确的说法与源码依据 |
|---|---|
| “`cached_len` 等于 host 序列长度。” | 不等。`cached_len` 是 KV 已缓存长度，`device_len` 在 `complete_one` 后先加一，而 host 序列要在 `_process_last_data` 中才 append（`python/minisgl/core.py:38-57`; `python/minisgl/scheduler/scheduler.py:142-152`）。 |
| “prefill 一定马上给用户一个 token。” | 对 `ChunkedReq` 不成立：它不可 decode，且结果处理直接 skip（`python/minisgl/scheduler/prefill.py:23-30`; `python/minisgl/scheduler/scheduler.py:147-151`）。 |
| “decode 一定优先，以降低每个用户延迟。” | 当前 `_schedule_next_batch` 先调用 prefill manager，只有其为空才试 decode（`python/minisgl/scheduler/scheduler.py:219-225`）。 |
| “cache 命中不需要任何额外资源。” | 命中后仍需 lock，锁改变可驱逐/受保护统计，且 admission 在 lock 后重新检查可用量（`python/minisgl/scheduler/prefill.py:43-54`; `python/minisgl/kvcache/radix_cache.py:113-130`）。 |
| “异步复制完成后 token 已可随时读。” | 引擎只提交 non-blocking D→H 并记录 event；scheduler 显式同步 event 才消费（`python/minisgl/engine/engine.py:202-206`; `python/minisgl/scheduler/scheduler.py:142-143`）。 |
| “abort 就取消了正在执行的 GPU forward。” | abort 分支修改 pending/running 管理器并回收资源；已启动 forward 的取消语义不由这些分支实现或证明（`python/minisgl/scheduler/scheduler.py:101-105,190-202`）。 |
| “CUDA Graph 为所有 batch 服务。” | `can_use_cuda_graph` 要求 decode 且 size 不超过上限；不满足时 `Engine.forward_batch` 调模型 eager forward（`python/minisgl/engine/graph.py:149-166`; `python/minisgl/engine/engine.py:191-197`）。 |
| “每个 TP rank 都向 tokenizer 回发。” | 多 rank 情形仅主 rank 的 `_reply_tokenizer_rank0` 写队列；rank 1 handler 忽略 reply（`python/minisgl/scheduler/io.py:124-133`）。 |

## exercises

### 代码理解题（附期待证据，不附最终答案）

1. 从 `0 <= cached_len < device_len <= max_device_len` 推导 `extend_len >= 1`，并指出该表达式在哪个符号中定义。证据：`python/minisgl/core.py:38-50`。
2. 设普通请求刚完成一次 prefill 且未 EOS：找出它何时进入 `running_reqs`，何时又可能将前缀写入 cache。证据：`python/minisgl/scheduler/scheduler.py:158-164,227-233` 与 `python/minisgl/scheduler/decode.py:14-15`。
3. 为什么 `ChunkedReq` 仍可能经过 `Engine.forward_batch`，却不应执行 `append_host`？证据：`python/minisgl/scheduler/prefill.py:23-30,65-90`，`python/minisgl/engine/engine.py:199-205`，`python/minisgl/scheduler/scheduler.py:147-151`。
4. 指出 prefill admission 的两次 capacity check 分别保护什么状态变化。证据：`python/minisgl/scheduler/prefill.py:47-56` 与 `python/minisgl/kvcache/radix_cache.py:113-130`。
5. 画出 `page_size=4`、`cached_len=3`、`device_len=6` 的新增 page 区间，务必使用 `div_ceil` 公式而非直觉。证据：`python/minisgl/scheduler/cache.py:42-53`。
6. 在 overlap loop 中，为什么 `_process_last_data(last_data)` 放在当前 batch 的 `_forward` 发起之后仍不会提前读 CPU token？证据：`python/minisgl/scheduler/scheduler.py:98-106,138-143`，`python/minisgl/engine/engine.py:202-206`。
7. 比较 `PrefillManager.abort_req` 和 `DecodeManager.abort_req` 的查找容器，并说明二者都怎样汇到同一个回收动作。证据：`python/minisgl/scheduler/prefill.py:153-158`，`python/minisgl/scheduler/decode.py:20-25`，`python/minisgl/scheduler/scheduler.py:190-202`。

### 延伸设计题（教学提案，非要求改仓库）

1. 设计一个避免“首个不可 admission request 阻塞后续短 request”的策略；列出它需要重新验证的公平性、chunk 优先级和容量预留条件。
2. 为请求状态写一个可执行的模型检查/属性测试：随机交错 prefill、decode、EOS、abort，并断言表行不重复分配、缓存页数守恒、普通 request 的 host/device 配对。
3. 设计 abort acknowledgement 的用户语义：分别说明 pending、已排入 decode set、已提交 forward 三种时刻能承诺什么；不要声称能中止未由当前代码暴露的 GPU 工作。

## recommended_expanded_structure

### 教学提案：后续教材章的推荐结构

1. **问题导入：为什么 `model.forward()` 之外还要有调度器。** 用异长 prompt、逐 token 输出、有限 KV 页三种压力引出本章；只作为背景，不做实现断言。
2. **四本账：`Req` 的 host/device/cache/table 状态。** 紧贴 `Req.__post_init__`、`complete_one`、`append_host`（`python/minisgl/core.py:28-61`），先建立长度表。
3. **从消息到 PendingReq。** 用 `_process_one_msg`、`add_one_req`、TP I/O 展示“到达不等于获资源”（`python/minisgl/scheduler/scheduler.py:169-198`; `python/minisgl/scheduler/prefill.py:121-124`; `python/minisgl/scheduler/io.py:79-122`）。
4. **prefill admission 与前缀保护。** 展开 prefix match、双容量检查、table allocation 与 queue head blocking（`python/minisgl/scheduler/prefill.py:39-63,126-151`）。
5. **长 prompt：chunk 不等于 decode。** 逐行读 `ChunkedReq`、`PendingReq.chunked_req` 和 `filter_reqs`（`python/minisgl/scheduler/prefill.py:23-30,96-102`; `python/minisgl/scheduler/decode.py:14-15`）。
6. **Batch 如何成为一次 forward。** 讲 `_prepare_batch` 的顺序、页分配、mapping、padded requests 与 graph 边界（`python/minisgl/scheduler/scheduler.py:204-217,236-267`; `python/minisgl/scheduler/cache.py:42-53`; `python/minisgl/engine/graph.py:149-166`）。
7. **逐 token decode 与回传。** 将 `Engine.forward_batch`、event、`_process_last_data` 串成一张短时间线（`python/minisgl/engine/engine.py:191-206`; `python/minisgl/scheduler/scheduler.py:138-167`）。
8. **资源生命末期：EOS、abort、cache 归还。** 以 `_free_req_resources`、`cache_req`、`lazy_free_region`、`finished_reqs` 作审计案例（`python/minisgl/scheduler/scheduler.py:158-166,190-202`; `python/minisgl/scheduler/cache.py:55-105`）。
9. **overlap 的正确性边界。** 最后才讲双 stream 和 event；把收益与不能自动保证的竞态分开（`python/minisgl/scheduler/scheduler.py:83-106`; `python/minisgl/engine/engine.py:202-206`）。
10. **实验与不变量清单。** 采用本笔记的纸卡、状态表与资源审计，不要求学生改引擎实现。

## limitations

### 代码事实边界

- `Scheduler.run_forever` 是无限循环；本章所读的 `shutdown` 只展示同步、CPU rank barrier 与 engine shutdown，不展示外层进程生命周期或异常恢复策略（`python/minisgl/scheduler/scheduler.py:120-136`）。
- `CacheManager.check_integrity` 检查 prefix cache 与 free page 数量关系；当前 `RadixCache.check_integrity` 的方法体为 `pass`，故不能把它描述成对 radix 树结构的全面验证（`python/minisgl/scheduler/cache.py:81-91`; `python/minisgl/kvcache/radix_cache.py:187-188`）。
- I/O mixin 的 offline mode 用抽象 `offline_receive_msg` / `offline_send_result` 替换线上方法；具体离线收发实现不在本章指定 scheduler 文件中，因此不对它的队列或线程语义作结论（`python/minisgl/scheduler/io.py:27-33,70-74`）。
- 本章只据 `Engine.forward_batch` 说明 graph/eager 的选择、采样和 copy event；attention backend 如何消费 `Batch.attn_metadata`、模型如何写 KV，不在此处展开（`python/minisgl/engine/engine.py:191-206`; `python/minisgl/scheduler/scheduler.py:204-217`）。
- abort 分支在调度器层面可见的行为是从容器移除并释放资源。对于“正在 GPU 上运行时”更细的同步、内存安全和客户端确认时序，必须在扩写前结合并发测试及更广的 engine/通信实现验证，不能由本笔记单独保证（`python/minisgl/scheduler/scheduler.py:83-105,190-202`）。

### 教学材料限制（教学提案）

- 推荐用小 token 数和纸上表格；不要把真实 GPU 时间、吞吐或 latency 数字当作本仓库承诺。
- 示例必须把“源码事实”和“改进设想”标色或分栏；尤其不要把公平调度、抢占、可靠取消、全局最优 packing 写成已实现能力。
- 后续扩写若加入运行演示，应先在目标 CUDA/模型环境验证，且将环境前置条件与可复现实验命令另列，不从本研究笔记推导性能结论。
