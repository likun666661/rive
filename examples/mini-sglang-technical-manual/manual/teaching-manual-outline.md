# mini-sglang：面向 LLM Serving 新手的教学手册大纲

> 版本：2026-07-10。本文是 90–120 分钟课程的教师用大纲；所有“现状”均以深读综合导读为准，并用源码路径/符号作阅读锚点。标为“实验/提案”的内容是课堂活动，不代表仓库已有功能或推荐生产改动。

## 课程定位

- **受众**：会读 Python、知道 Transformer 基本组成（token、attention、KV）的工程师；尚未系统接触推理服务、批处理调度或 GPU/多卡运行时。
- **先修**：能在 Linux/CUDA 环境运行 Python；知道进程、队列/套接字、GPU stream 的基本含义。无需事先掌握 NCCL、CUDA Graph 或 Triton。
- **目标**：学生能沿一条请求解释“文本怎样变成流式 token”，辨认吞吐/延迟/显存三个主要张力，并能带着问题进入 mini-sglang 的核心文件。
- **课程论题**：LLM serving 不是“把 `model.forward()` 包进 HTTP”。它是一个以请求状态为中心、以 KV 复用和显存为约束、以异步流水和多卡确定性为边界条件的系统；模型计算只是这条链中的一个阶段。
- **事实边界**：mini-sglang 是教学/参考性质的紧凑推理实现，面向 Linux + NVIDIA CUDA。课程不把它当作完整生产服务；例如 OpenAI 兼容性只属部分实现，且导读记录了关闭、基准和流式协议方面的已知风险。

## 时间安排与教师节奏

| 模块 | 建议时间 | 产出 |
|---|---:|---|
| 开场、论题、全链路图 | 10 分钟 | 一张请求生命周期心智图 |
| 01 入口/API/进程拓扑 | 12–15 分钟 | 找到请求的第一跳和 I/O 边界 |
| 02 Req/Batch/Context 与调度 | 18–22 分钟 | 解释 prefill/decode 与 overlap |
| 03 engine/CUDA Graph/采样/分布式 | 15–18 分钟 | 解释一次 forward 的执行选择 |
| 04 KV cache/radix/内存 | 15–18 分钟 | 解释为何能复用、何时会逐出 |
| 05 模型/layer/attention/MoE/kernel | 15–18 分钟 | 连接调度元数据与模型计算 |
| 06 tokenizer/流式/server/benchmark | 12–15 分钟 | 解释用户看到的增量文本与度量 |
| 回收、练习讲评 | 8–10 分钟 | 用不变量检查全链路 |

总计约 105 分钟；讨论、现场运行或第 05 章深挖可将其延展至 120 分钟。

## 跨章节请求生命周期图（先讲，后反复回指）

```text
客户端 HTTP/SSE
  │ POST /v1/chat/completions
  ▼
FrontendManager ──ZMQ──▶ tokenizer worker ──TokenizeMsg/UserMsg──▶ scheduler rank 0
  ▲                                                                  │
  │ UserReply                                                        ├─ TP 时：原始消息 + count 广播给其他 rank
  │                                                                  ▼
  │                                                        Req → PrefillManager/DecodeManager
  │                                                                  │
  │                                            radix match/lock、表行、页分配、Batch 元数据
  │                                                                  ▼
  │      DetokenizeMsg (rank 0) ◀──────── sampled token ◀── Engine.forward_batch
  │             │                                      （eager 或 decode CUDA Graph；KV 写入）
  │             ▼
  └──── detokenizer：安全增量文本（缓冲 surrogate/CJK 等） ─────── SSE chunks

并行时间线：engine stream 执行 batch N；scheduler stream 在 CUDA event 允许后处理 batch N-1 的 D→H 结果。
```

教师提示：强调图中的“rank 0”不是一个泛化的性能结论，而是本仓库的 I/O 边界设计；其他 TP rank 被喂入相同顺序的消息以保持计算一致。

## 30 分钟压缩路线

**适用场景**：技术预览、读码导览或后续深课的导入。目标不是覆盖所有实现细节，而是让学生能用同一张图定位问题。

1. **0–4 分钟：全链路图与论题。** 区分“HTTP 到 token”与“token 到安全流式文本”；说明 prefill 与 decode 的不同形状。
2. **4–11 分钟：核心数据和调度。** 阅读 `python/minisgl/core.py` 的 `Req`、`Batch`、`Context.forward_batch`，再看 `scheduler/scheduler.py` 的 `run_forever`、`overlap_loop`、`_schedule_next_batch`。只追问：一个请求何时进入批、何时推进一 token？
3. **11–18 分钟：一次执行。** 阅读 `engine/engine.py:forward_batch` 与 `engine/graph.py` 的 `GraphRunner.replay`/`pad_batch`。结论：图只用于符合尺寸的 decode；否则走 eager。
4. **18–24 分钟：KV 与前缀。** 阅读 `kvcache/mha_pool.py`、`kvcache/radix_cache.py` 的 `match_prefix`、`lock_handle`、`evict`。用共享提示词解释命中与锁定。
5. **24–28 分钟：进入与流出。** 快速看 `server/api_server.py:FrontendManager`、`tokenizer/detokenize.py:DetokenizeManager`，回到 SSE。
6. **28–30 分钟：口头检查。** 让学生依图回答：为什么不能在 event 前读 CPU token？为何断开连接不能保证取消正在执行的 GPU forward？

压缩路线明确略过：模型权重组织、attention 后端差异、MoE 与 kernel 分层、多卡插件细节、在线 benchmark 的完整实现。这些是完整课第 03、05、06 章的展开项。

---

## 01 入口、API 与进程拓扑（12–15 分钟）

### 学习目标

- 说清在线 HTTP、交互 shell、离线 `LLM` 三个入口如何汇到 Scheduler + Engine。
- 画出 parent、tokenizer、detokenizer、每 GPU 一个 scheduler 的职责边界。
- 知道 rank 0 是前端 I/O 边界，以及启动确认屏障存在的原因。

### 问题背景

一次生成面对的是文本、GPU 和可能多个进程。Web 框架不能直接替代 token 化、GPU 调度或增量反 token 化；因此服务需要控制面消息通道及 GPU 数据面通信。

### 为什么难

- 多进程启动必须避免不安全地继承 CUDA 状态；而服务又必须在 worker 都就绪后才接流量。
- 在 TP 下，只有一个前端入口，却要让各 rank 接到完全相同的调度输入。
- “离线 API”看似不同，却复用调度核心，容易被误读为另一套推理逻辑。

### 核心抽象

- `ServerArgs`/`SchedulerConfig`/`EngineConfig`：冻结 dataclass 的配置继承与派生地址。
- `launch_server`、`start_subprocess`、ack queue：`spawn` 与就绪屏障。
- `FrontendManager`：FastAPI 到 ZMQ 的异步桥；`uid`、ack/event 映射归它管理。
- `LLM(Scheduler)`：离线模式换成进程内收发，而非另写 engine。

### 精确阅读顺序（路径/符号）

1. `python/minisgl/__main__.py`：确认 CLI 入口。
2. `python/minisgl/server/args.py`：`ServerArgs`，先标出 `num_tokenizer`、world size 与地址派生项。
3. `python/minisgl/server/launch.py`：`launch_server` → `start_subprocess` → `_run_scheduler`；找 `spawn` 与 ack 等待。
4. `python/minisgl/server/api_server.py`：`FrontendManager`、`new_user`、`listen`，再定位 `/v1/chat/completions` 路由（导读锚点为约第 255 行）。
5. `python/minisgl/llm/llm.py`：`LLM`，对照其 `offline_mode=True` 的进程内路径。

### 实验/演示

**演示 A（提案）**：教师在白板把 `world_size=1` 与 `world_size>1` 两张图并列；让学生给每个 socket/worker 加箭头。随后从 `launch_server` 数 ack 数量，验证其为 `num_tokenizers + 2`。

### 常见误解

- “每张 GPU 都各自接 HTTP。”——本实现中 rank 0 是唯一前端 I/O 边界。
- “`num_tokenizer=0` 表示没有 tokenizer。”——它表示 tokenizer 与 detokenizer 默认共享一个进程。
- “离线模式仍经过 ZMQ。”——导读表明其用内存 stub 绕过 ZMQ。

### 练习

1. 标注请求从 `new_user` 到 scheduler rank 0 前跨越了哪两类 worker。
2. 解释若某 worker 在 ack 前崩溃，当前启动流程的可观测后果；只根据代码阅读提出改进思路，不实现修改。

### 交接到下一章

入口只产生带 `uid`、token ids 和采样参数的消息。下一章从它被变成 `Req` 开始，解释为什么“收到请求”并不等于“马上 forward”。

---

## 02 Req、Batch、Context 与调度（18–22 分钟）

### 学习目标

- 区分 `Req` 的 host/device/cache 长度，描述一次 `complete_one` 与 `append_host` 的配对。
- 区分 prefill、chunked prefill、decode，知道调度是 prefill-first。
- 解释双 stream overlap、`ForwardInput`/`ForwardOutput` 快照以及 CUDA event 的必要性。

### 问题背景

LLM 请求的 prompt 长度和输出长度不同。服务必须在有限 KV 容量下将不同请求组成批，同时把长 prompt 的计算、逐 token decode、结果处理和新请求到达交错起来。

### 为什么难

- 请求状态在 CPU token 序列、GPU 表项与 radix 前缀之间同时存在。
- 长 prompt 需要 chunk，不能把未完成的 chunk 当成可以 decode 的普通请求。
- overlap 让批 N 的执行和批 N-1 的 CPU 处理重叠；错误的读写时机可造成读取未完成 D→H 拷贝或重复释放。

### 核心抽象

- `SamplingParams`、`Req`、`Batch`、`Context`；`Context._batch` 仅在 `forward_batch` 内有效。
- `cached_len < device_len <= max_device_len`：请求推进的关键约束；`extend_len` 至少为 1。
- `PrefillManager`/`DecodeManager`：等待队列与运行队列的不同所有权。
- `ForwardInput`/`ForwardOutput`、`copy_done_event`：跨迭代的安全边界。

### 精确阅读顺序（路径/符号）

1. `python/minisgl/core.py`：`SamplingParams` → `Req.__post_init__` → `Req.complete_one`/`Req.append_host` → `Batch` → `Context.forward_batch`。
2. `python/minisgl/scheduler/scheduler.py`：`run_forever` → `overlap_loop`（导读锚点约 83–106 行）→ `_schedule_next_batch` → `_prepare_batch` → `_forward` → `_process_last_data`。
3. `python/minisgl/scheduler/prefill.py`：`ChunkedReq`、`PrefillAdder.try_add_one`、`schedule_next_batch`；找预算和第一条不可接纳请求即停止的行为。
4. `python/minisgl/scheduler/decode.py`：`DecodeManager.schedule_next_batch`，注意按 `uid` 排序。
5. `python/minisgl/scheduler/io.py`：`_recv_from_tokenizer.get` 与 rank 0 的 count/payload fan-out（导读锚点约 79、88–122 行）。
6. `python/minisgl/scheduler/cache.py`：`cache_req` 与 `lazy_free_region`，为第 04 章预热。

### 实验/演示

**演示 B（提案）**：给三张卡片：短 prompt、超出 prefill budget 的长 prompt、decode 中请求。让学生按“prefill 优先、预算限制、chunk 不采样”排出两个 batch，并在时间轴上标出 batch N 执行时处理的是哪个 batch 的结果。

### 常见误解

- “prefill 后总会立即采样。”——chunked prefill 不进入 `DecodeManager`，也不采样。
- “`complete_one` 就已把 token 放到主机输入。”——它推进 device 记账；非 chunked 请求还需恰好一次 `append_host`。
- “有 non-blocking copy 就可以立刻读。”——CPU 读取前仍需 `copy_done_event.synchronize()`。
- “调度一定寻找全局最优 packing。”——当前 prefill 的首个不可接纳项会导致 head-of-line blocking。

### 练习

1. 写出为何 `0 <= cached_len < device_len <= max_device_len` 能保证 `extend_len >= 1`。
2. 画出 abort 到达“正在 forward 的请求”时的状态位置，并说明为什么这不等价于取消 GPU kernel。
3. 找出 `_process_last_data` 中防止同一轮重复释放的意图；说明它不能替代对跨轮所有权的系统验证。

### 交接到下一章

调度得到的 `Batch` 仍只是元数据。下一章追它如何被 engine 放到正确 CUDA stream、选择 graph/eager 路径、完成采样并跨 TP 通信。

---

## 03 Engine、CUDA Graph、采样与分布式（15–18 分钟）

### 学习目标

- 描述 Engine 的初始化所有权和 `forward_batch` 的 graph/eager 分支。
- 说明 CUDA Graph 是固定形状 decode 的优化，而不是所有 forward 的通用替代。
- 区分 gloo 控制通信与 NCCL/PyNCCL GPU 数据通信；理解插件“后注册者生效”的阅读风险。

### 问题背景

decode 是大量小而形状重复的 GPU 操作，launch 开销显著；TP 又要求每 rank 的张量计算与通信顺序一致。服务需要同时控制低延迟和显存可预测性。

### 为什么难

- CUDA 初始化、stream 归属、图捕获、通信资源的生命周期顺序会决定正确性和可关闭性。
- graph 需要受控的 batch shape，因此要用 dummy 请求补齐；这会与页表和 dummy page 设计相连。
- 分布式性能路径不应被误读为单一固定库：通信实现按插件栈选择。

### 核心抽象

- `Engine`：拥有 CUDA 初始化；导读记录初始化会断言 CUDA 尚未预初始化。
- `forward_batch`：组织 token gather、模型 forward、`complete_one`、采样、非阻塞 D→H 与 event 记录。
- `GraphRunner`：最大 batch size 优先捕获；仅 decode、符合 graph batch size 时 replay。
- `DistributedCommunicator`：`all_reduce`/`all_gather` 使用最后一个插件；gloo CPU group 用于控制，PyNCCL/NCCL 用于 GPU 数据。

### 精确阅读顺序（路径/符号）

1. `python/minisgl/engine/config.py`：`page_size` 与 graph 相关配置的来源。
2. `python/minisgl/engine/engine.py`：`Engine.__init__` 的初始化顺序，再读 `forward_batch`（导读锚点约 191–206 行）。
3. `python/minisgl/engine/graph.py`：图捕获顺序、`pad_batch`、`GraphRunner.replay`、关闭顺序相关注释（约 168 行）。
4. `python/minisgl/engine/sample.py`：采样入口；将 greedy argmax 与 FlashInfer top-k/top-p 作为可见分支阅读。
5. `python/minisgl/distributed/impl.py`：`DistributedCommunicator`、插件追加与最后插件路由。
6. `python/minisgl/kernel/pynccl.py`：PyNCCL 通信插件的定位。
7. 回看 `python/minisgl/scheduler/io.py`：rank 0 的 gloo count 广播如何配合 ZMQ payload 广播。

### 实验/演示

**演示 C（提案）**：用 3、5、8 三种 decode batch size 的纸卡，假设已捕获 4 和 8，要求学生决定各自 eager、直接 replay 或补齐后 replay，并指出 dummy req 不得泄漏到真实分配/逐出路径。

### 常见误解

- “CUDA Graph 加速 prefill。”——导读明确将 graph 定位为 decode-only。
- “所有 rank 各自随时收请求即可。”——TP 调度依赖相同消息顺序；rank 0 广播 count 与原始 payload。
- “通信插件永远是第一个注册的实现。”——此处路由规则是最后一个插件生效。
- “关机只要销毁进程组。”——导读强调 graph → process group → communicator 的顺序。

### 练习

1. 说明为什么 `forward_batch` 必须在 engine 的 stream 上执行。
2. 用一句话区分“控制面 count 广播”与“GPU tensor all-reduce”。
3. 找到 graph padding 需要额外表行/页的原因，并把它留作第 04 章的验证问题。

### 交接到下一章

Engine 能 forward 的前提是每个 token 有可写的 KV 位置。下一章展开 page table、KV pool 与 radix 前缀树如何共同提供这种位置。

---

## 04 KV Cache、Radix 与内存（15–18 分钟）

### 学习目标

- 解释 paged KV pool 的扁平 token location 表示，避免把它简单当 page id。
- 沿一次前缀命中说明 match、lock、写入、insert/re-lock、evict 的生命周期。
- 用页粒度和锁引用计数解释 prefix reuse 与内存安全之间的折中。

### 问题背景

生成的每个 token 都需要读此前 token 的 K/V。若每请求完整复制 KV，显存和重复计算会迅速失控；相同系统提示词或对话前缀则提供了可复用机会。

### 为什么难

- cache 同时有物理池、请求页表和逻辑 radix 树，三者的所有权不同。
- 一次命中返回的索引只有在 handle 被锁定时才可安全使用；锁定会降低可逐出容量，影响准入。
- radix 的粒度与 page size 绑定，尾部 token 和整页释放的处理必须精确。

### 核心抽象

- MHA KV buffer：逻辑形状 `[2, L, P, page_size, H, D]`，按扁平 token location 索引。
- `page_table`、`token_pool`、`free_slots`：表项保存 raw token location；free list 保存页对齐 token 起点。
- `RadixPrefixCache`：压缩 trie、最长前缀、路径到根锁定、叶子 LRU 逐出。
- dummy page：engine 分配 `num_pages + 1`，额外页不进入 free list，服务 graph padding。

### 精确阅读顺序（路径/符号）

1. `python/minisgl/kvcache/base.py`：cache handle 的有效性和“先 lock 后用”的约束。
2. `python/minisgl/kvcache/mha_pool.py`：KV buffer 形状和扁平 token 索引。
3. `python/minisgl/engine/engine.py`：`page_table` 创建与 dummy page/row（导读锚点约 66、69、89–98 行）。
4. `python/minisgl/scheduler/table.py`：`token_pool` 与 scheduler 的 token scatter 角色。
5. `python/minisgl/scheduler/cache.py`：`allocate_paged` → `cache_req` → `lazy_free_region`；查看 idle 时的 `check_integrity` 计数式（约 84、55、93 行）。
6. `python/minisgl/kvcache/radix_cache.py`：`_tree_walk` → `match_prefix` → `lock_handle` → `insert_prefix`（索引 clone）→ `evict`（叶子 LRU）。
7. 回读 `python/minisgl/scheduler/prefill.py`：两阶段准入检查为何要在 lock 前后各做一次。

### 实验/演示

**演示 D（提案）**：画出 page size 为 4 的 token 前缀 `[A,B,C,D,E]` 和另一请求 `[A,B,C,D,F]`。学生标出可按页复用的部分、不可按半页安全共享的部分，以及将 handle 从未锁改成锁定后对可逐出页的影响。

### 常见误解

- “页表条目就是 page id。”——导读说明其为 raw token location；不同 attention 后端可能需要 slice/divide 解释它。
- “match 返回索引就永久可用。”——handle 未锁时它可能被 eviction 失效。
- “radix 按任意 token 前缀复用。”——树的匹配/插入会按 `page_size` 对齐。
- “逐出刚好释放请求量。”——整节点 LRU 可能 oversupply，但不应 undersupply。

### 练习

1. 解释空闲时为何应满足 `len(free_slots) + prefix_cache.total_size/page_size == num_pages`。
2. 找出 `insert_prefix` 需要 clone indices 的理由，并描述若直接保存可变 page table 视图的风险。
3. 设计一个**实验提案**来观测多次共享前缀请求的命中率；不要声称仓库已有该指标。

### 交接到下一章

页表最终转换为 attention 的读写位置。下一章进入模型和层，解释这些由 scheduler/engine 准备的批元数据怎样被 attention、MoE 和 kernel 消费。

---

## 05 模型、层、Attention、MoE 与 Kernels（15–18 分钟）

### 学习目标

- 识别 mini-sglang 模型装配的统一 decoder 骨架，理解为何普通 `nn.Module` 直觉并不完全适用。
- 说明 K/V 必须在 attention 读取前写入 cache，以及 attention 后端与 page size 的兼容约束。
- 解释列并行到行并行的 TP 配对，以及 MoE router 一致性的基本动机。

### 问题背景

调度器并不直接计算 attention；它准备位置与映射。模型层必须把它们变成正确的 KV 写入、attention 读取、MLP/MoE 和跨卡归约，同时在不同 CUDA kernel 后端之间保持语义一致。

### 为什么难

- 模型参数树是 plain `BaseOP` 对象而不是 `nn.Module`，权重状态遍历方式不同。
- attention backend 不是可随意互换的开关：`fi` 受 `page_size==1` 约束，混合规格可让 prefill/decode 走不同后端。
- 多卡线性层的部分和必须在正确的位置 all-reduce；kernel 的对齐/整除条件往往是正确性前提而非微优化。

### 核心抽象

- `BaseOP`：通过 `__dict__` 形成 state dict 的参数树。
- 模型注册/装配：Llama、Qwen、Mistral 使用统一 decoder 结构的变体。
- attention backend：`fa`、`fi`、`trtllm`，以及形如 `"p,d"` 的 prefill/decode 混合规格。
- `store_kv`：每后端 `forward` 内先写 K/V 再运行 attention。
- TP：column-parallel 产生局部结果，row-parallel 层 `all_reduce` 汇总；MoE fused backend 与复制 router。
- kernel 分层：自有 C++/CUDA、Triton、外部库等，个别路径带对齐约束（如 `warp::copy` 的 128B）。

### 精确阅读顺序（路径/符号）

1. `python/minisgl/models/register.py`：模型选择/注册入口。
2. `python/minisgl/models/utils.py`：`RopeAttn`、`GatedMLP` 的组装线索。
3. `python/minisgl/layers/base.py`：`BaseOP` 与 state dict 遍历。
4. `python/minisgl/models/llama.py`：选一个完整 decoder 作为具体跟读样本；再回看其对公共层的调用。
5. `python/minisgl/layers/linear.py`：column/row parallel 线性层与 `all_reduce` 配对。
6. `python/minisgl/attention/__init__.py`：后端选择和混合 `"p,d"` 规格。
7. `python/minisgl/attention/fa.py` 或 `python/minisgl/attention/fi.py`：任选一个实际 backend 跟一次 `forward`；在 `fi.py` 查 `page_size==1` 断言。
8. `python/minisgl/kvcache/base.py` 与所选 `attention/*`：定位 `store_kv` 在 attention kernel 之前发生的顺序。
9. `python/minisgl/layers/moe.py` → `python/minisgl/moe/fused.py` → `python/minisgl/kernel/triton/fused_moe.py`：只追一条 fused MoE 路径。
10. `python/minisgl/kernel/index.py`、`store.py`、`radix.py`、`tensor.py`、`pynccl.py`：建立 kernel 责任地图；仅在需要时查看 `kernel/csrc/*` 或 `kernel/triton/*` 的被点名实现。

### 实验/演示

**演示 E（提案）**：让学生把“新 token 的 K/V”卡片放在“attention 读历史 KV”卡片之前；再给出 `page_size=1` 与更大页的两种配置，要求判断是否可直接选择 `fi`。最后用两张 GPU 卡片演示 column 局部输出经 row 侧归约成为完整结果。

### 常见误解

- “模型一定继承 `torch.nn.Module`。”——本项目的 `BaseOP` 是普通对象参数树。
- “attention 后端只影响性能。”——页大小、元数据布局和调用约束可影响可用性与正确性。
- “attention 先算完再把 KV 留给下轮。”——当前步 attention 也需要读到新写入的 K/V，故 `store_kv` 在其前。
- “TP 只在模型结尾通信。”——column→row 配对的归约嵌在层结构中。

### 练习

1. 从 `layers/linear.py` 解释为什么 column 输出的局部和需在 row 侧归约。
2. 选 `fa` 或 `fi`，写一条后端选择前必须确认的配置约束。
3. 列出一个 kernel 调优实验前应先检查的条件（例如整除或对齐），并说明这是前置条件而不是保证性能的承诺。

### 交接到下一章

模型输出的只是 token id。下一章回到文本边界：tokenizer 如何把它安全地变成流式文本，server 如何包装 SSE，benchmark 又实际测到了什么。

---

## 06 Tokenizer、Server Streaming 与 Benchmarks（12–15 分钟）

### 学习目标

- 说清 token 化、增量 detokenize 与 SSE 三层各自的职责。
- 理解为何流式文本要缓冲 surrogate、替换字符/CJK 等边界，而非每个 token 都立即裸 decode。
- 能审慎阅读在线/离线 benchmark，区分 TTFT、TPOT 与 E2E，并识别兼容性/负载注入的限制。

### 问题背景

用户关心的是可显示的 token 流与端到端时间，而不是 GPU 上的整数 id。文本编码边界会使“一个 token 一次返回”产生不完整 Unicode/文本片段；基准则容易把客户端行为、服务协议和模型计算混为一谈。

### 为什么难

- tokenizer 与 detokenizer 可以共享或拆分进程，消息的方向与第 01 章拓扑相反。
- 安全增量输出可能暂时为空；这是正确的缓冲行为，却可能与严格 SSE 客户端的预期冲突。
- OpenAI 兼容接口是部分实现，benchmark 的请求字段未必被 server 真正消费。

### 核心抽象

- `tokenize_worker`/`TokenizeManager.tokenize`：文本变为 `int32 input_ids`，再封装 `UserMsg`/`BatchBackendMsg`。
- `DetokenizeManager`：按 `uid` 保持状态，缓冲 surrogate、`�` 和部分 CJK/可打印文本边界，生成 incremental output。
- `FrontendManager.wait_for_ack`、`stream_chat_completions`、`stream_with_cancellation`：ZMQ reply 到 SSE 与断连后 best-effort abort。
- Bench：在线 OpenAI async client + trace replay；离线使用 `LLM`；TTFT/TPOT/E2E 通过经验 CDF 统计分位数。

### 精确阅读顺序（路径/符号）

1. `python/minisgl/tokenizer/server.py`：`tokenize_worker`，看共享/拆分 worker 如何收发。
2. `python/minisgl/tokenizer/tokenize.py`：`TokenizeManager.tokenize`，确认 `input_ids` 的生成位置。
3. `python/minisgl/tokenizer/detokenize.py`：`DetokenizeManager`（导读锚点约 63 行），追踪缓冲和 incremental output。
4. `python/minisgl/server/api_server.py`：`FrontendManager.wait_for_ack` → `stream_chat_completions` → `stream_with_cancellation`，回看路由。
5. `python/minisgl/message/*.py`：消息类型和 msgpack `__type__` discriminator；注意导读指出 tensor 序列化只覆盖 1-D。
6. `python/minisgl/benchmark/client.py`：TTFT/TPOT/E2E 指标计算（导读锚点约 332 行）及请求构造。
7. `python/minisgl/benchmark/offline/bench.py`：离线 benchmark 通过 `LLM` 的路径。
8. 需要在线负载细节时再读 `python/minisgl/benchmark/online/*` 与 `python/minisgl/benchmark/perf.py`。

### 实验/演示

**演示 F（提案）**：准备一个会跨 token 边界产生未完成文本片段的示例（教师可用 tokenizer 输出预录数据，不承诺所有模型复现）。逐步展示 `DetokenizeManager` 可能先返回空增量、随后返回可安全显示文本；让学生区分“没有新 token”与“有 token 但暂不发字符”。

### 常见误解

- “token id 可直接安全显示。”——增量 detokenize 需要维护每个 uid 的状态与边界缓冲。
- “客户端断开就立即停止所有计算。”——abort 是 best-effort；正在 forward 的请求无法中途取消 GPU forward。
- “接口名是 OpenAI，就具备完整 OpenAI 语义。”——导读记录 `n`、`stop`、penalty 等未完整转入采样，usage 也不是实际统计。
- “在线 benchmark 的 input length override 已生效。”——导读提出它可能被 pydantic 丢弃，应先验证再信任数字。

### 练习

1. 画出 scheduler rank 0 到 SSE 的回复方向，并标出 detokenizer 的位置。
2. 给出 TTFT、TPOT、E2E 各自回答的用户体验问题；说明为何不能只报一个平均值。
3. 设计一个**实验提案**，验证在线 benchmark 的请求字段是否被 API 模型接收；必须以观察/测试为结论，不预设结果。

### 课程收束

回到全链路图：入口负责把用户意图送入系统；`Req`/`Batch` 决定何时计算；Engine 和模型把元数据变为下一 token；KV/radix 决定显存与复用；tokenizer/server 把 token 变为可消费的流。诊断任何异常时，先问它发生在这条链的哪一段、涉及哪种所有权或时间边界。

---

## 源码锚点附录（按问题而非目录索引）

| 想回答的问题 | 首选路径与符号 | 阅读时验证的代码事实 |
|---|---|---|
| 从 CLI 到多进程如何启动？ | `python/minisgl/__main__.py`; `server/launch.py:launch_server`, `start_subprocess`, `_run_scheduler`; `server/args.py:ServerArgs` | 使用 `spawn`；就绪以 ack queue 计数等待。 |
| HTTP 请求何时获得 uid、如何等回复？ | `server/api_server.py:FrontendManager`, `new_user`, `listen`, `wait_for_ack`, `stream_chat_completions` | 前端维护 uid 与 ack/event 映射，经 ZMQ 桥接。 |
| 一个请求有哪些不变量？ | `core.py:Req.__post_init__`, `complete_one`, `append_host`; `Batch`; `Context.forward_batch` | 长度关系、推进/附加配对、batch phase 与活跃 context 范围。 |
| 谁决定 prefill 或 decode？ | `scheduler/scheduler.py:run_forever`, `overlap_loop`, `_schedule_next_batch`; `scheduler/prefill.py:PrefillAdder.try_add_one`; `scheduler/decode.py:DecodeManager.schedule_next_batch` | prefill-first；超预算可 chunk；decode 按 uid 排序。 |
| 结果何时可读？ | `scheduler/scheduler.py:_process_last_data`; `engine/engine.py:forward_batch` | D→H copy 后由 `copy_done_event` 作为 host read 门。 |
| graph 何时使用？ | `engine/engine.py:forward_batch`; `engine/graph.py:GraphRunner.replay`, `pad_batch` | 仅 decode；大小命中时 replay，不足可用 dummy padding。 |
| TP 怎样保持调度一致？ | `scheduler/io.py:_recv_from_tokenizer`; `distributed/impl.py:DistributedCommunicator`; `kernel/pynccl.py` | rank 0 广播 count/payload；控制与 GPU 数据通信分工不同。 |
| KV 的物理位置和表项如何对应？ | `kvcache/mha_pool.py`; `engine/engine.py` 的 page table/dummy page；`scheduler/table.py:token_pool`; `scheduler/cache.py:allocate_paged` | pool 为分页布局；page table 使用 raw token location；dummy 资源隔离。 |
| 前缀怎样安全复用/逐出？ | `kvcache/base.py`; `kvcache/radix_cache.py:match_prefix`, `lock_handle`, `insert_prefix`, `evict`; `scheduler/cache.py:cache_req` | 锁保护 handle；按页对齐；树拥有 clone 后的 indices；叶子 LRU。 |
| 模型不使用 `nn.Module` 吗？ | `layers/base.py:BaseOP`; `models/register.py`; `models/utils.py:RopeAttn`, `GatedMLP`; `models/llama.py` | 参数树走 `__dict__`；模型有统一 decoder 组装线索。 |
| attention 与 KV 的顺序是什么？ | `attention/__init__.py`; `attention/fa.py` 或 `attention/fi.py`; `kvcache/base.py:store_kv` | backend forward 先写新 K/V 再读 attention；`fi` 要求 page size 为 1。 |
| MoE/kernel 该从哪里追？ | `layers/moe.py`; `moe/fused.py`; `kernel/triton/fused_moe.py`; `kernel/{index,store,radix,tensor}.py` | fused 是当前 MoE 后端线索；kernel 有多层实现来源与条件。 |
| 流式文本和指标在哪里？ | `tokenizer/server.py:tokenize_worker`; `tokenizer/tokenize.py:TokenizeManager.tokenize`; `tokenizer/detokenize.py:DetokenizeManager`; `benchmark/client.py`; `benchmark/offline/bench.py` | detokenize 按 uid 缓冲；在线/离线 benchmark 路径不同，指标含 TTFT/TPOT/E2E。 |

## 限制、风险提示与后续课题

- 本大纲是读码课，不是部署手册；没有提供模型下载、CUDA 安装、压测阈值或生产容量承诺。
- 源码事实仅覆盖综合导读已归纳的路径。遇到版本差异，应回到本仓库同一路径/符号核验，而非把课堂图当作 API 合同。
- 需明确告诉学生：导读记录 HTTP 非正常退出可能遗留 worker/IPC 文件，启动 ack barrier 没有超时；这是讨论可靠性的案例，不应在课堂中假称已经修复。
- 同样需保留 API/基准的限定：部分 OpenAI 字段不完整转发，流式 chunk 格式和空 delta 有兼容风险，`input_length_override` 在 benchmark 到 API 模型间可能被丢弃。任何性能结论先验证工作负载是否真的进入服务。
- radix tree 的内部完整性检查在导读中被标为未充分验证；缓存/调度实验应优先加可观测性、断言或隔离测试，避免直接用生产流量验证假设。
- 可扩展的后续专题：H1/H2 的生命周期可靠性、H4/H6 的取消/队首阻塞、H8 的 cache 可观测性、H11 的 benchmark 有效性；它们是问题清单，不是本课的既定修复方案。

## 教师结束检查单

- 学生能否用自己的话连接 `Req` 长度不变量、分页 KV 与每步 decode？
- 学生能否指出 overlap 中 event 的位置，而不把异步 copy 误当作可立即读取？
- 学生能否解释为何 rank 0 是 I/O 边界、但所有 TP rank 仍须收到一致消息？
- 学生能否将“token 已生成”和“安全的文本 chunk 已发送”分成两个阶段？
- 若答案有缺口，回到相应章节的“精确阅读顺序”，每次只沿一条路径/符号追踪。
