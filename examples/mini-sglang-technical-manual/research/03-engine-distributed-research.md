# 第 03 章研究笔记：Engine、CUDA Graph、采样与分布式运行时

> 面向后续教材扩写者；不是最终教材。本笔记把仓库已实现的行为标为 **[代码事实]**，把课堂组织、图示和练习标为 **[教学提案]**。除“教学提案/限制”外，每一项实现性陈述均给出路径、符号和行号锚点；行号以本次工作区版本为准。

## scope

本章解释一次已被 scheduler 选中的 `Batch` 如何在 GPU 上执行、产生下一个 token，并在 tensor parallel（TP）进程中保持通信与消息顺序一致。覆盖范围是：Engine 初始化与配置派生、CUDA Graph 捕获/补齐/回放、采样、gloo 与 NCCL/PyNCCL 的职责、ZMQ 消息封装与进程启动、环境变量，以及已实现的关闭路径。

**[代码事实]** 主要入口是 `python/minisgl/engine/engine.py:Engine`；它接收 `EngineConfig`（`python/minisgl/engine/config.py:EngineConfig`），`Scheduler` 在构造时创建它（`python/minisgl/scheduler/scheduler.py:Scheduler.__init__`, 45–76 行），并在 `_forward` 调用 `Engine.forward_batch`（227–233 行）。因此本章应承接第 02 章的“批已经形成、元数据正在准备”，而不是重复请求准入或 KV/radix 的全部细节。

**[代码事实]** 与第 01 章的边界是：`server/launch.py:launch_server` 负责创建 scheduler/tokenizer 进程；`scheduler/io.py:SchedulerIOMixin` 把 rank 0 收到的原始消息传播给其他 TP rank；Engine 只负责每个 rank 内的设备、模型、通信和 forward（`python/minisgl/server/launch.py`, 40–113 行；`python/minisgl/scheduler/io.py`, 27–65、88–122 行；`python/minisgl/engine/engine.py:Engine`, 30–110 行）。

不在本章展开的主题：具体 attention kernel、模型层、KV 页分配/逐出、HTTP/SSE 协议和 tokenizer 的文本语义。可以引用它们来说明接口，但不把它们写成第 03 章的中心。

## teaching_goal_alignment

课程大纲将第 03 章定位为 15–18 分钟模块：学生应能“描述 Engine 的初始化所有权和 `forward_batch` 的 graph/eager 分支”、“说明 CUDA Graph 是固定形状 decode 优化”，以及“区分 gloo 控制通信与 NCCL/PyNCCL GPU 数据通信”。这与 `teaching-manual-outline.md` 的“03 Engine、CUDA Graph、采样与分布式”模块及其“核心抽象/阅读顺序”一致（`.rive-artifacts/minisglang-teaching-20260710/manual/teaching-manual-outline.md`, “03 Engine、CUDA Graph、采样与分布式”节）。

建议将目标写成可观察的行为，而非术语背诵：

- **[教学提案]** 学生能从 `Engine.__init__` 画出“配置 → GPU/stream → 通信 → 模型 → KV/page table → 后端/采样 → 图捕获”的先后关系，并说出为什么不能随意调换两处。
- **[教学提案]** 给定一个 prefill 或不同大小的 decode batch，学生能依 `GraphRunner.can_use_cuda_graph` 与 `pad_batch` 判定 eager/回放/补 dummy 的分支。
- **[教学提案]** 学生能用一句准确的话区分“CPU 控制组广播消息数量”与“GPU 张量 all-reduce/all-gather”，并能指出二者在本仓库的具体实现。
- **[教学提案]** 学生能沿 `forward_batch` 说明 GPU token、异步 D→H token 与 CUDA event 为什么是三个不同的对象。

## concept_to_execution_path

### 1. 从配置到一个 rank 的 Engine

**[代码事实]** `EngineConfig` 是冻结 dataclass，含模型路径、`DistributedInfo`、dtype、并发请求上限、attention/MoE 后端、图 batch size、页大小、内存比例、通信超时、dummy weight、PyNCCL 开关和覆盖参数（`python/minisgl/engine/config.py:EngineConfig`, 15–31 行）。`hf_config` 与 `model_config` 是缓存属性；`max_seq_len` 可以被 `max_seq_len_override` 覆盖（33–47 行）。

**[代码事实]** CLI 的 `ServerArgs` 继承 scheduler/engine 配置；`parse_args` 将 `--tensor-parallel-size` 转为初始的 `DistributedInfo(0, size)`，并支持 `--disable-pynccl`、`--cuda-graph-max-bs`、`--num-pages`、`--page-size` 和 attention/MoE 参数（`python/minisgl/server/args.py:parse_args`, 86–130、148–217、251–265 行）。在 shell 模式它强制 `cuda_graph_max_bs=1`、`max_running_req=1` 和静默输出（229–234 行）。

**[代码事实]** `Engine.__init__` 首先断言 CUDA 尚未初始化，设置进程全局 TP 信息并调整自动配置；随后选取 `cuda:{rank}`、设置 device、设置种子、创建/设为当前 CUDA stream，并创建且全局注册 `Context`（`python/minisgl/engine/engine.py:Engine.__init__`, 30–42 行）。`set_tp_info` 和 `set_global_ctx` 都拒绝第二次设置（`python/minisgl/distributed/info.py:set_tp_info`, 21–25 行；`python/minisgl/core.py:set_global_ctx`, 125–135 行）。

**[代码事实]** `_adjust_config` 虽面对 frozen config 仍以 `object.__setattr__` 改写：attention backend 的 `auto` 按 SM100/SM90/其他选择 `trtllm`、`fa,fi` 或 `fi`；TRTLLM 与不合法 page size 时改为 64；MoE 的 `auto` 改为 `fused`（`python/minisgl/engine/engine.py:_adjust_config`, 218–233 行）。这是“配置声明”和“运行时决议”两个概念，教材应明确区分。

### 2. 通信、内存、模型与运行时对象

**[代码事实]** Engine 初始化通信后先测量可用显存（`Engine.__init__`, 44–46 行）。`_init_communication` 在单卡或 `use_pynccl=True` 时初始化 gloo WORLD group 并调用 `enable_pynccl_distributed`；否则初始化 NCCL WORLD group，并另建 gloo group 作为 `tp_cpu_group`（`python/minisgl/engine/engine.py:_init_communication`, 112–137 行）。

**[代码事实]** `_sync_get_memory` 对设备同步、清空缓存、重置峰值统计，读取当前空闲显存，使用 `tp_cpu_group` 的 MIN all-reduce 得到最小/最大值；若差超过 2 GiB，记录错误后抛出 `RuntimeError`（`python/minisgl/engine/engine.py:_sync_get_memory`, 170–189 行）。注意：它构造 `[free, -free]` 并取 MIN，因此第二项取反后成为最大值；这一点适合在教师备课稿中明确，避免把函数名误读成只返回一个值。

**[代码事实]** 然后 Engine 在 meta device 与目标 dtype 语境中 `create_model`，再加载真实或 dummy 权重（`Engine.__init__`, 48–52 行；`Engine._load_weight_state_dict`, 139–146 行）。它根据启动前/模型加载后的显存差及模型维度计算每页 KV 大小，或采用 `num_page_override`，并断言最终页数大于 1（`Engine._determine_num_pages`, 148–168 行）。

**[代码事实]** `Context` 保存 page table、attention/MoE backend、KV cache 和“当前 batch”；`Context.forward_batch` 禁止嵌套、在 `finally` 清除活动 batch（`python/minisgl/core.py:Context`, 100–122 行）。Engine 创建 `num_pages + 1` 个 KV 页，其中额外页给 dummy 请求；page table 尺寸为 `(max_running_req + 1, align32(max_seq_len))`，最后一行填入 dummy 页的 raw token location（`python/minisgl/engine/engine.py:Engine.__init__`, 55–73、89–110 行；`_align_up_32`, 214–215 行）。

**[代码事实]** 模型/KV/page table 后，Engine 创建 attention backend；当模型是 MoE 时创建 MoE backend；再创建 `Sampler`，最后创建 `GraphRunner`（`python/minisgl/engine/engine.py:Engine.__init__`, 75–110 行）。这条顺序是源码构造顺序，并非泛称的所有推理引擎初始化模板。

### 3. 由 scheduler 准备到 Engine forward

**[代码事实]** `Scheduler._prepare_batch` 先调用 `engine.graph_runner.pad_batch`，再分配页、生成 positions/input/write 映射、从 page table 取 `out_loc`、让 attention backend 准备 metadata，并让 sampler 准备 batch 参数（`python/minisgl/scheduler/scheduler.py:_prepare_batch`, 204–217 行）。`_forward` 把 input token 放到 batch、调用 `Engine.forward_batch`、把 GPU token 写回 token pool 并过滤不能继续 decode 的请求（227–233 行）。

**[代码事实]** `Engine.forward_batch` 先断言当前 CUDA stream 是 Engine 自己的 stream；在 `Context.forward_batch` 中，能用 graph 就 replay，否则调用 `model.forward()`；随后仅遍历真实 `batch.reqs` 并对每个请求 `complete_one`（`python/minisgl/engine/engine.py:Engine.forward_batch`, 191–206 行）。`Req.complete_one` 令 `cached_len=device_len` 后递增 `device_len`，而 `Req.__post_init__` 约束 `0 <= cached_len < device_len <= max_device_len`（`python/minisgl/core.py:Req`, 38–54 行）。

**[代码事实]** 采样结果先保留 GPU tensor，再以 `non_blocking=True` 复制至 CPU；Engine 在同一 stream 记录 CUDA event，并作为 `ForwardOutput(next_tokens_gpu, next_tokens_cpu, copy_done_event)` 返回（`python/minisgl/engine/engine.py:ForwardOutput`, 23–26 行；`Engine.forward_batch`, 202–206 行）。overlap loop 随后在处理上一批时先 `copy_done.synchronize()`，才读取 CPU token、`append_host` 和发出 `DetokenizeMsg`（`python/minisgl/scheduler/scheduler.py:_process_last_data`, 138–167 行）。

### 4. CUDA Graph：捕获一次，按合规尺寸回放

**[代码事实]** `_determine_cuda_graph_bs` 优先使用显式 `cuda_graph_bs`；否则若未指定最大值，以初始可用显存大于 80 GiB 选 256，否则选 160；小于 1 则返回空列表；默认候选为 `[1,2,4]` 加从 8 起步的 8 的倍数（`python/minisgl/engine/graph.py:_determine_cuda_graph_bs`, 49–67 行）。`GraphRunner` 用最大候选作 capture buffer 大小，空列表时跳过捕获（`GraphRunner.__init__` 与 `_capture_graphs`, 92–108、120 行）。

**[代码事实]** `GraphCaptureBuffer` 分配固定 GPU `input_ids`、`out_loc`、`positions` 和 logits；捕获时 `set_batch` 将 dummy batch 字段指向这些 buffer 切片，回放前 `copy_from` 将实时 batch 的三个输入复制进去（`python/minisgl/engine/graph.py:GraphCaptureBuffer`, 20–46 行）。

**[代码事实]** 捕获前 attention backend 接到候选 batch size；循环按从大到小的 size 创建 decode dummy batch，调用 `prepare_for_capture`，在 `Context.forward_batch` 内先 warmup 一次 `model.forward()`，再在 `torch.cuda.graph(..., stream=self.stream)` 捕获；第一个图的 memory pool 复用于后续图（`python/minisgl/engine/graph.py:GraphRunner._capture_graphs`, 105–147 行）。dummy 请求的 `table_idx` 是 `max_running_req`，其 page table 行已填 dummy 页位置（`python/minisgl/engine/engine.py:Engine.__init__`, 89–109 行）。

**[代码事实]** `can_use_cuda_graph` 的判据是“decode 且真实 batch size 不大于最大捕获 size”（`python/minisgl/engine/graph.py:GraphRunner.can_use_cuda_graph`, 149–150 行）。`pad_batch` 将合规 decode 批补到首个不小于真实 size 的已捕获尺寸；不合规则不补（160–166 行）。`replay` 以 `batch.padded_size` 选图，调用 backend 的 `prepare_for_replay`、`g.replay()`，返回只含真实请求大小的 logits（152–158 行）。因此“图是 decode-only 的固定尺寸优化”是代码事实；“所有 forward 都图化”不是。

### 5. 采样：从每请求参数到一个 token

**[代码事实]** `Sampler.prepare` 收集真实 `batch.reqs` 的 `sampling_params`。若全部 `is_greedy`，返回 `temperatures=None`；否则将温度下限夹到 `1e-6`、把无约束 `top_k` 替换为 vocab size、把 `top_p` 夹到 `[1e-6,1]`，并仅在 batch 中确有约束时创建 top-k/top-p 设备张量（`python/minisgl/engine/sample.py:Sampler.prepare`, 53–68 行；`python/minisgl/core.py:SamplingParams.is_greedy`, 15–25 行）。设备张量先建 pinned CPU tensor 后非阻塞传到 GPU（`python/minisgl/engine/sample.py:make_device_tensor`, 20–21 行）。

**[代码事实]** `Sampler.sample` 在 greedy 情形执行 `torch.argmax(logits, dim=-1)`；否则转 float 并走 `sample_impl`（`python/minisgl/engine/sample.py:Sampler.sample`, 70–75 行）。`sample_impl` 通过 `flashinfer.sampling.softmax` 产生概率，并按 top-k/top-p 是否存在选择无约束、top-k、top-p 或二者组合的 FlashInfer 函数；SM90 支持状态传给 `enable_pdl`（`python/minisgl/engine/sample.py:sample_impl`, 24–45 行）。

### 6. TP：控制面与 GPU 张量面分离

**[代码事实]** `DistributedInfo` 是冻结的 `(rank,size)`，构造时断言 `0 <= rank < size`；全局 TP 信息只能设置一次（`python/minisgl/distributed/info.py:DistributedInfo`, 6–25 行）。`DistributedCommunicator` 默认插件是 `TorchDistributedImpl`，但 `all_reduce` 和 `all_gather` 总是调用 `plugins[-1]`，即最后注册的实现（`python/minisgl/distributed/impl.py:DistributedCommunicator`, 63–70 行）。

**[代码事实]** `TorchDistributedImpl` 单卡直接返回输入；多卡使用 `dist.all_reduce(SUM)` 或 `dist.all_gather_into_tensor`（`python/minisgl/distributed/impl.py:TorchDistributedImpl`, 24–41 行）。启用 PyNCCL 时，`enable_pynccl_distributed` 为多卡调用 `init_pynccl` 并 append `PyNCCLDistributedImpl`，从而令之后的 communicator 调用路由到 PyNCCL（73–97 行）。

**[代码事实]** `init_pynccl` AOT 加载带 `-lnccl` 的 `pynccl` 模块，把 `max_size_bytes` 限制为 `ENV.PYNCCL_MAX_BUFFER_SIZE.value`，由 rank 0 生成 NCCL UID，并用 `tp_cpu_group` 的 `broadcast_object_list` 传给其他 rank，最终构造 FFI communicator（`python/minisgl/kernel/pynccl.py:_load_nccl_module`, 28–30 行；`init_pynccl`, 45–78 行）。这正是“gloo CPU group 可承担控制/UID 分发，而 PyNCCL/NCCL 承担 GPU 张量通信”的仓库内例证。

**[代码事实]** 接收用户消息时，rank 0 从 backend ZMQ PULL 读 raw bytes，再以 ZMQ PUB 发送 raw payload 给非主 rank，并以 `tp_cpu_group.broadcast` 广播此次非阻塞 drain 的消息数；非主 rank 先收 count，再对应次数从 SUB 读取（`python/minisgl/scheduler/io.py:SchedulerIOMixin._recv_msg_multi_rank0`, 88–107 行；`_recv_msg_multi_rank1`, 109–122 行）。这是调度输入顺序一致性的实现路径，不是 GPU 张量 collective。

### 7. 进程、ZMQ 与启动屏障

**[代码事实]** `launch_server` 解析 `ServerArgs` 后把 `start_subprocess` 交给 API server（`python/minisgl/server/launch.py:launch_server`, 40–45、113 行）。子函数强制 multiprocessing `spawn`，为每个 rank 复制参数并启动一个非 daemon scheduler；随后启动一个 detokenizer 和 `num_tokenizer` 个 tokenizer worker（47–103 行）。

**[代码事实]** `_run_scheduler` 在 `torch.inference_mode()` 中创建 Scheduler、在 CPU group 上 `sync_all_ranks()`，仅主 rank 向 ack queue 报“Scheduler is ready”，之后执行 `run_forever()`；捕获 `KeyboardInterrupt` 时调用 `scheduler.shutdown()`（`python/minisgl/server/launch.py:_run_scheduler`, 16–37 行）。启动方等 `num_tokenizer + 2` 条 ack，即一个主 scheduler、所有 tokenizer、一个 detokenizer（105–111 行）。

**[代码事实]** `utils/mp.py` 的队列不是 multiprocessing Queue 的通用替代，而是 msgpack 字典载荷上的 ZMQ socket 封装：同步/异步 PUSH 写入，PULL 接收/可取 raw bytes，PUB/SUB 用于原始广播；每个 `stop()` 关闭 socket 并终止 context（`python/minisgl/utils/mp.py:ZmqPushQueue`, 12–30 行；`ZmqPullQueue`, 54–81 行；`ZmqPubQueue`, 105–126 行；`ZmqSubQueue`, 129–151 行）。

### 8. 环境变量的有限、显式作用域

**[代码事实]** `ENV` 是导入时构造的 singleton。`EnvVar._init` 从名为 `MINISGL_<属性名>` 的环境变量解析，解析异常被吞掉并保留默认值（`python/minisgl/env.py:EnvVar`, 16–34 行；`EnvClassSingleton.__init__`, 58–87 行）。已声明的运行时开关包括 `FLASHINFER_USE_TENSOR_CORES`、`DISABLE_OVERLAP_SCHEDULING` 与 `PYNCCL_MAX_BUFFER_SIZE`（67–70 行）。

**[代码事实]** `ENV.DISABLE_OVERLAP_SCHEDULING` 令 `Scheduler.run_forever` 改走 `normal_loop`；默认路径走 `overlap_loop`（`python/minisgl/scheduler/scheduler.py:Scheduler.run_forever`, 120–131 行）。`PYNCCL_MAX_BUFFER_SIZE` 只在 `init_pynccl` 对其参数作上限限制（`python/minisgl/kernel/pynccl.py:init_pynccl`, 52–55 行）。因此不要将这些环境变量笼统描述为“所有运行时策略”。

### 9. 关闭路径

**[代码事实]** `Scheduler.shutdown` 依次设备同步、CPU group barrier、调用 `Engine.shutdown`（`python/minisgl/scheduler/scheduler.py:Scheduler.shutdown`, 133–136 行）。`Engine.shutdown` 先销毁 CUDA graphs，再 `torch.distributed.destroy_process_group()`，最后清空 distributed 插件列表（`python/minisgl/engine/engine.py:Engine.shutdown`, 208–211 行；`python/minisgl/engine/graph.py:GraphRunner.destroy_cuda_graphs`, 168–171 行；`python/minisgl/distributed/impl.py:destroy_distributed`, 93–97 行）。graph 方法的源码注释明确要求它先于释放 NCCL 资源，以避免程序挂起（`python/minisgl/engine/graph.py`, 168–169 行）。

**[代码事实]** API lifespan 结束时，`FrontendManager.shutdown` 只停止前端的两个 ZMQ queue（`python/minisgl/server/api_server.py:FrontendManager.shutdown`, 211–222 行）。shell 路径还会枚举并 kill parent 的递归子进程（`python/minisgl/server/api_server.py:run_shell`, 399–408 行）。这些是不同入口的不同关闭行为；不要把它们合并成“服务总能优雅等待并回收所有子进程”的结论。

## exact_source_anchors

| 教材问题 | 首读锚点 | 验证锚点 |
|---|---|---|
| 配置从哪来、在哪被运行时改写？ | `python/minisgl/engine/config.py:EngineConfig` 15–55 | `python/minisgl/server/args.py:parse_args` 68–268；`engine/engine.py:_adjust_config` 218–233 |
| Engine 为什么要先占有 CUDA？ | `python/minisgl/engine/engine.py:Engine.__init__` 30–45 | `python/minisgl/core.py:set_global_ctx` 125–135；`distributed/info.py:set_tp_info` 21–25 |
| KV、dummy 页与 graph 的连接在哪里？ | `python/minisgl/engine/engine.py:Engine.__init__` 55–110 | `python/minisgl/engine/graph.py:GraphRunner._capture_graphs` 105–147 |
| 哪些 batch 可以 replay？ | `python/minisgl/engine/graph.py:GraphRunner.can_use_cuda_graph/pad_batch/replay` 149–166 | `python/minisgl/scheduler/scheduler.py:_prepare_batch` 204–217；`engine.py:forward_batch` 191–206 |
| token 怎样从 logits 出来又安全到 CPU？ | `python/minisgl/engine/sample.py:Sampler.prepare/sample` 53–75 | `python/minisgl/engine/engine.py:forward_batch` 199–206；`scheduler.py:_process_last_data` 138–167 |
| gloo、PyNCCL、插件选择各做什么？ | `python/minisgl/engine/engine.py:_init_communication` 112–137 | `python/minisgl/distributed/impl.py` 24–97；`python/minisgl/kernel/pynccl.py:init_pynccl` 45–78 |
| TP rank 如何收到同序输入？ | `python/minisgl/scheduler/io.py:_recv_msg_multi_rank0/_rank1` 88–122 | `python/minisgl/utils/mp.py:ZmqPubQueue/ZmqSubQueue` 105–151 |
| worker 如何启动、何时声称 ready？ | `python/minisgl/server/launch.py:_run_scheduler/launch_server` 16–113 | `python/minisgl/scheduler/io.py:sync_all_ranks` 76–77 |
| 关闭的正确局部顺序是什么？ | `python/minisgl/scheduler/scheduler.py:shutdown` 133–136 | `python/minisgl/engine/engine.py:shutdown` 208–211；`graph.py:destroy_cuda_graphs` 168–171 |

## invariants_and_failure_modes

### 代码直接表达的不变量

- **[代码事实]** 创建 Engine 前 CUDA 不可已初始化；同一进程的 TP info 和 global Context 只能设置一次（`python/minisgl/engine/engine.py:Engine.__init__`, 30–42 行；`python/minisgl/distributed/info.py:set_tp_info`, 21–25 行；`python/minisgl/core.py:set_global_ctx`, 128–131 行）。违反会触发断言或 `RuntimeError`，因此同一解释器内重复构造 Engine 不是此代码展示的支持路径。
- **[代码事实]** 任意 `Req` 满足 `0 <= cached_len < device_len <= max_device_len`，且 active Context 禁止嵌套 forward（`python/minisgl/core.py:Req.__post_init__`, 38–42 行；`Context.forward_batch`, 115–122 行）。违例意味着 batch 元数据或执行时序已坏，源码以断言终止，而非尝试修复。
- **[代码事实]** `Engine.forward_batch` 必须在 Engine 的 CUDA stream 上执行（`python/minisgl/engine/engine.py:Engine.forward_batch`, 191–192 行）。`Scheduler.overlap_loop` 因而在 `engine_stream_ctx` 中等待 scheduler stream 后调用 `_forward`（`python/minisgl/scheduler/scheduler.py:overlap_loop`, 83–106 行）。
- **[代码事实]** graph 回放只接受 decode 且真实 batch size 不超过上限的批；回放 map 以 padded size 为 key（`python/minisgl/engine/graph.py:GraphRunner.can_use_cuda_graph/replay/pad_batch`, 149–166 行）。若候选 size 配置不覆盖 `pad_batch` 所求的首个尺寸，`next(...)` 会失败；这是一项由代码结构可见的配置风险。
- **[代码事实]** CPU token 读取必须发生在相应 `copy_done_event.synchronize()` 之后（`python/minisgl/engine/engine.py:Engine.forward_batch`, 202–206 行；`python/minisgl/scheduler/scheduler.py:_process_last_data`, 142–152 行）。仅因复制设置为 non-blocking 并不能推出 CPU 数据可读。
- **[代码事实]** 多卡消息输入须让非主 rank 得到与主 rank 相同的 message count 和 raw payload 顺序（`python/minisgl/scheduler/io.py:_recv_msg_multi_rank0/_rank1`, 88–122 行）。否则各 rank 随后的调度/collective 顺序可能不一致；“可能不一致”是由该同步设计作出的工程推断，而非源码逐字声明。
- **[代码事实]** 关闭前应先清 graph，再销毁 process group/插件；源码注释直接把反序要求与 hang 风险关联（`python/minisgl/engine/engine.py:Engine.shutdown`, 208–211 行；`python/minisgl/engine/graph.py`, 168–171 行）。

### 值得明说的失败模式与边界

- **[代码事实]** TP rank 间空闲显存差超过 2 GiB 会使 `_sync_get_memory` 抛异常（`python/minisgl/engine/engine.py:_sync_get_memory`, 170–189 行）。教材应将其解释为启动期容量一致性检查，而不要承诺它诊断所有性能不均衡。
- **[代码事实]** 自动 KV 容量若计算后页数不大于 1 会断言失败（`python/minisgl/engine/engine.py:_determine_num_pages`, 148–168 行）。
- **[代码事实]** `EnvVar._init` 静默吞掉解析错误（`python/minisgl/env.py:EnvVar._init`, 22–28 行）。因此拼错或格式错误的环境变量可能退回默认值而没有显式报错。
- **[代码事实]** `DistributedCommunicator` 路由最后注册插件（`python/minisgl/distributed/impl.py:DistributedCommunicator`, 63–70 行），不是按“第一个可用”或显式优先级选择；任何教材图示都要画出此栈语义。
- **[代码事实]** `_run_scheduler` 只捕获 `KeyboardInterrupt` 来触发 `scheduler.shutdown`（`python/minisgl/server/launch.py:_run_scheduler`, 30–37 行）。其他异常路径、启动 ACK 永久等待的超时策略以及 parent 对所有 child 的统一回收，不能根据这些行宣称已完整实现。

## pedagogical_story

**[教学提案]** 用一个问题开场：“scheduler 已经选好 5 个正在 decode 的请求，为什么 engine 不能只是 `model.forward()`？”答案分三层逐步揭示：

1. **先让 GPU 世界一致。** 每个 rank 必须先有自己的 device/stream、同一 TP 身份和已建立的控制/数据通信；否则没有可比较的显存基线，也没有一致的 collective 顺序。
2. **把动态请求折成可重复的形状。** decode 的真实 size 5 可通过 dummy 请求补到已捕获 size，例如 8；pre-fill 或超过最大 size 则保留 eager。这里的 dummy 不是“假的用户请求”，而是给图、page table 与 buffer 提供合法占位的运行时技术。
3. **让 GPU 产物跨到 CPU 而不破坏并行。** 真正的 token 先在 GPU 出现，异步复制后由 event 定义 CPU 可读时刻；采样、D→H 和 scheduler 的上一批结果处理由此可重叠。

**[教学提案]** 将通信放在这条故事的两侧，而不是塞进 graph：左侧是 rank 0 让所有 rank 接到同序控制消息；计算内部是层所用的 GPU all-reduce/all-gather。让学生用“相同顺序”和“相同张量计算”分别归纳两条责任线。

**[教学提案]** 结尾回到关闭：CUDA Graph 和 communicator 也有资源生存期，优化不是单一 `replay()` 调用；“资源创建顺序”必须能与“释放顺序”在图上对应。避免把此处写成泛化的 CUDA 教科书定律，只陈述本仓库注释和调用顺序。

## demo_or_reading_lab

### 15 分钟纸上执行实验

**[教学提案]** 不要求 GPU 或模型权重。教师给出：已捕获 `graph_bs_list=[1,2,4,8]`、三个批 `(prefill,3)`、`(decode,5)`、`(decode,10)`。学生按以下证据填写表格：

| 输入批 | 代码判定 | 结果 | 必须引用 |
|---|---|---|---|
| prefill, 3 | `is_decode` 为假 | eager，不补 graph dummy | `graph.py:GraphRunner.can_use_cuda_graph/pad_batch` |
| decode, 5 | 合规且不超过 8 | 补 3 个 dummy，回放 key 8，返回前 5 行 logits | `graph.py:GraphRunner.pad_batch/replay` |
| decode, 10 | 超过 8 | eager，不补 | `graph.py:GraphRunner.can_use_cuda_graph/pad_batch` |

接着要求他们在执行箭头后补上 `complete_one → sample → non_blocking D→H → event.record → event.synchronize → append_host`，逐项标源：`engine.py:Engine.forward_batch` 191–206、`core.py:Req.complete_one/append_host` 52–57、`scheduler.py:_process_last_data` 138–167。

### 10 分钟源码阅读实验

**[教学提案]** 两人一组，一人只读 `Engine._init_communication` 与 `kernel.init_pynccl`，另一人只读 `SchedulerIOMixin._recv_msg_multi_rank0/_rank1`。共同产出两句话：

1. “gloo group 在此处用于 ________。”须以 `engine.py` 112–137、`pynccl.py` 59–72 或 `scheduler/io.py` 100–122 作为证据。
2. “PyNCCL plugin 在此处用于 ________。”须以 `distributed/impl.py` 45–90、`kernel/pynccl.py` 45–78 作为证据。

教师检查点：学生不得把 ZMQ PUB/SUB 的 raw payload 广播说成 NCCL all-reduce，也不得把 gloo group 说成所有 GPU tensor 的唯一通道。

## misconceptions

- **[教学提案]** “CUDA Graph 加速所有 batch。”纠正：代码判据是 decode 且 size 不超过 max；其他情形在 `Engine.forward_batch` 调 eager 路径（`python/minisgl/engine/graph.py`, 149–166 行；`python/minisgl/engine/engine.py`, 191–198 行）。
- **[教学提案]** “padding 后 sampler 会为 dummy 生成用户 token。”纠正：Engine 的 `complete_one` 仅遍历 `batch.reqs`，采样 logits 也切至 `[:batch.size]`（`python/minisgl/engine/engine.py:Engine.forward_batch`, 199–203 行）。
- **[教学提案]** “non-blocking copy 等于 CPU 可以立刻读。”纠正：scheduler 显式等待 event（`python/minisgl/scheduler/scheduler.py:_process_last_data`, 142–152 行）。
- **[教学提案]** “PyNCCL 已启用就不需要 gloo。”纠正：PyNCCL 初始化本身使用传入的 CPU group 广播 NCCL UID，I/O 同步也使用 CPU group（`python/minisgl/kernel/pynccl.py:init_pynccl`, 59–72 行；`python/minisgl/scheduler/io.py`, 100–122 行）。
- **[教学提案]** “配置 frozen 所以运行时绝不会改。”纠正：`_adjust_config` 显式使用 `object.__setattr__`（`python/minisgl/engine/engine.py:_adjust_config`, 218–233 行）。
- **[教学提案]** “关掉 FastAPI 就自动优雅关闭所有 GPU worker。”纠正：API lifespan 只停前端 ZMQ queue，而 scheduler shutdown 是 KeyboardInterrupt 路径；shell 另有 kill children 的特例（`python/minisgl/server/api_server.py`, 211–222、399–408 行；`python/minisgl/server/launch.py`, 30–37 行）。

## exercises

1. **[教学提案]** 证明或反驳：“`GraphRunner.replay` 可以接受任意 prefill batch。”只能使用 `python/minisgl/engine/graph.py` 149–166 行，答案须区分 `can_use`、padding 与 map lookup。
2. **[教学提案]** 在一张时序图上标出 Engine stream、scheduler stream、D→H copy、`copy_done_event` 和 CPU 读取。再说明删去 `copy_done.synchronize()` 的风险，引用 `engine.py:forward_batch` 202–205 与 `scheduler.py:_process_last_data` 142–152。
3. **[教学提案]** 以 `tp_size=2,use_pynccl=True` 和 `tp_size=2,use_pynccl=False` 对比 `_init_communication` 的 process group 构造，说明哪条路径 append PyNCCL plugin。引用 `python/minisgl/engine/engine.py` 112–137 和 `python/minisgl/distributed/impl.py` 73–90。
4. **[教学提案]** 给定 `temperature=0, top_k=-1, top_p=1` 与 `temperature=0.7, top_k=50, top_p=0.9`，预测 `Sampler.prepare` 的张量字段和 `Sampler.sample` 的分支。引用 `python/minisgl/core.py:SamplingParams.is_greedy` 23–25、`python/minisgl/engine/sample.py` 53–75。
5. **[教学提案]** 查找一个“代码明确保护”的关闭顺序和一个“源码没有保证”的关闭情形；分别给路径/符号，并将后者写成限制，不得虚构实现。

## recommended_expanded_structure

**[教学提案]** 后续扩写可采用下列结构（目标为约 15–18 分钟正文 + 可选实验，而非在本笔记中直接写成成章正文）：

1. **问题与位置（1 分钟）**：从第 02 章的 `ForwardInput` 接入，画 `Scheduler._prepare_batch → _forward → Engine.forward_batch → ForwardOutput → _process_last_data`。
2. **Engine 先拥有运行时（3 分钟）**：`EngineConfig`、CUDA 未初始化断言、rank/device/stream、Context 单例、通信与显存基线；侧栏解释 frozen config 的运行时决议。
3. **初始化产物（2 分钟）**：模型 meta 构造/加载、KV 页数、raw-location page table、dummy page/row、backend 和 sampler 的构造顺序。KV 的分配算法只链接到第 04 章。
4. **一批 forward 的真实时间线（3 分钟）**：stream 断言、Context 范围、eager/graph 分支、请求推进、采样、GPU/CPU 双 token 与 event。
5. **CUDA Graph 的形状契约（3 分钟）**：候选 size、静态 buffer、由大到小 capture、warmup、dummy padding、replay；配纸上 batch-size 实验。
6. **两条 TP 通信线（2.5 分钟）**：gloo 的控制/屏障/UID 与 PyNCCL/NCCL 张量操作；rank 0 的 count + raw payload fan-out；插件最后注册者生效。
7. **启动、环境、关闭（2 分钟）**：spawn 与 ACK、三个 ENV runtime 开关的局部作用、graph→process group→plugin 的释放顺序。
8. **误解检查与练习（1.5 分钟）**：四道判断题，至少包含 “event 前不能读 CPU token” 和 “prefill 不 graph replay”。

**[教学提案]** 推荐插图仅两张：一张按时间排列的“Engine 初始化/关闭”图；一张把 CPU 控制面（ZMQ + gloo）和 GPU 数据面（模型 + PyNCCL/NCCL）分开着色的 TP 图。图注必须分别指向 `Engine.__init__/shutdown` 和 `SchedulerIOMixin`/`init_pynccl`，以防示意图超出代码事实。

## limitations

- **[代码事实]** 这是小型参考实现的阅读笔记；从本章代码只能确认本仓库的形状、顺序和接口，不能据此推广为任一 serving 系统或任一 NCCL 部署的标准做法。`EngineConfig.distributed_addr` 默认固定为 `tcp://127.0.0.1:2333`，而 `ServerArgs` 覆盖为 `server_port+1`（`python/minisgl/engine/config.py`, 53–55 行；`python/minisgl/server/args.py`, 49–51 行），尤其不应把它写成多机部署设计。
- **[代码事实]** 图大小的“80 GiB→256、否则 160”是当前 `_determine_cuda_graph_bs` 的启发式，不是已验证的性能最佳值或硬件普适阈值（`python/minisgl/engine/graph.py`, 49–67 行）。
- **[代码事实]** `_load_weight_state_dict` 的 dummy weight 分支是测试路径，不能用于阐述真实模型精度或加载性能（`python/minisgl/engine/engine.py`, 139–146 行）。
- **[代码事实]** `DistributedCommunicator.plugins` 是类级可变列表，并由 `destroy_distributed` 设为空（`python/minisgl/distributed/impl.py`, 63–67、93–97 行）；笔记不对重复初始化/关闭后的再次使用作支持性承诺。
- **[代码事实]** ZMQ 封装的 `stop()` 只展示 socket/context 关闭（`python/minisgl/utils/mp.py`, 28–30、79–81、124–126、149–151 行），不能仅据此声明消息排空、送达保证或跨进程终止协议。
- **[教学提案]** 后续作者应在无 GPU 的课堂用静态阅读/纸上实验替代真实 capture，不要把需要 CUDA、FlashInfer、NCCL、权重和兼容驱动的实跑设为唯一达标方式。
