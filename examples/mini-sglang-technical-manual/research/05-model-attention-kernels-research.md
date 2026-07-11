# 第 05 章研究笔记：模型、Attention、MoE 与 Kernel

## scope

本笔记为后续扩写教材提供可核查的研究素材，不是最终章稿，也不提出仓库改动。范围严格限于模型注册与权重装载、`BaseOP` 参数树、张量并行（TP）层配对、attention 后端与 KV 存储、MoE 路由，以及自有/外部 kernel 的边界与硬件前提；与调度、CUDA Graph、radix 逐出有关的内容仅在它们改变本章执行契约时提及。

**代码事实。** 课程大纲把本章定位为把 scheduler 已准备的 position、映射和 batch 元数据变为“KV 写入 → attention 读取 → MLP/MoE → 跨卡归约”的计算路径，并明确要求学生理解 `BaseOP`、`page_size==1` 的 `fi` 限制、column→row TP 配对和 kernel 对齐条件（`.rive-artifacts/minisglang-teaching-20260710/manual/teaching-manual-outline.md`，第 05 章“核心抽象/为什么难”）。下列实现锚点以当前工作区源码为准；“教学提案”不描述已存在的运行时功能。

## teaching_goal_alignment

| 大纲学习目标 | 可由代码支撑的最小事实 | 教学上应形成的可观察解释 |
|---|---|---|
| 识别统一 decoder 骨架，避免把它误读为普通 `nn.Module` | `BaseOP.state_dict` 遍历 `__dict__` 中的 Tensor 和子 `BaseOP`；Llama 的 `LlamaModel` 由 embedding、`OPList` decoder layers、终端 norm 组成（`python/minisgl/layers/base.py:15-53`；`python/minisgl/models/llama.py:46-82`）。 | 学生能从 checkpoint 名称推到对象树，而不是寻找 PyTorch module registry。 |
| 说明 K/V 必须先入 cache 再被 attention 读取 | 三个后端均在各自 `forward` 中先调用 `self.kvcache.store_kv(...)`，随后把同 layer 的 `k_cache/v_cache` 交给 attention wrapper/kernel（`python/minisgl/attention/fa.py:48-65`；`fi.py:176-188`；`trtllm.py:49-89`）。 | 学生能解释“本轮新 token”为何不能等下一轮才写 KV。 |
| 解释 column→row 的 TP 配对和 MoE router 一致性动机 | QKV/gate-up 是输出维分片；O/down 是输入维分片并在 `tp_size>1` 时 `all_reduce`；MoE gate 使用 `LinearReplicated`，expert 输出也归约（`python/minisgl/layers/linear.py:56-127`；`python/minisgl/models/utils.py:53-75`；`python/minisgl/layers/moe.py:45-59`）。 | 学生能指出“本地 matmul 成功”不等于“TP 语义完整”。 |

**教学提案。** 本章的达标检查应是：给出一条 decode token 的简图，学生能按顺序标出 `input_ids → qkv → RoPE → store_kv → backend attention → o_proj all_reduce → MLP/MoE → lm_head`，并能为每个箭头说出一个源码锚点。这个检查是课程设计，不是仓库测试。

## concept_to_execution_path

### A. 从模型名称到可加载的对象树

**代码事实。** `create_model` 取 `model_config.architectures[0]` 并调用 `get_model_class`（`python/minisgl/models/__init__.py:7-8`）。`_MODEL_REGISTRY` 把六个 HuggingFace architecture 字符串映射到相对模块与类；`get_model_class` 通过 `importlib.import_module` 延迟导入，未知架构抛出 `ValueError`（`python/minisgl/models/register.py:5-21`）。`ModelConfig.from_hf` 对含 `text_config` 的配置下沉 text 子配置，并补回 architectures/RoPE 字段；同时填充 KV heads、head dim、MoE 字段和 RoPE 配置（`python/minisgl/models/config.py:40-86`）。

**代码事实。** 以 Llama 为完整样本：`LlamaForCausalLM.forward` 从全局 context 的 `batch.input_ids` 开始，`LlamaModel` 先做 vocab-parallel embedding，遍历 `OPList` 中每层，再做终端 RMSNorm，最后由 `ParallelLMHead` 出 logits（`python/minisgl/models/llama.py:18-82`）。Qwen3-MoE 沿相同三段式骨架，但 decoder 的 MLP 是 `MoEMLP`，attention 开启 q/k norm（`python/minisgl/models/qwen3_moe.py:18-80`）。因此“统一骨架”是代码中的相近装配方式，不是声称所有模型逐行相同。

**代码事实。** 权重装载是生成器：`load_weight` 读取 safetensors，按 TP rank 切分或复制 KV projection，合并 q/k/v 为 `qkv_proj`、gate/up 为 `gate_up_proj`，并把 MoE expert tensors 堆叠为首维是 expert 的张量；尾部断言不允许未完成的 merge/expert buffer（`python/minisgl/models/weight.py:34-52,75-124`）。模型占位 Tensor 与 checkpoint 的接缝是 `BaseOP.load_state_dict`：它 `pop` 叶 Tensor，断言 shape/dtype 相同，再替换属性；顶层剩余 key 会报错（`python/minisgl/layers/base.py:32-53`）。

### B. 一层 dense decoder 的数据流与 TP 配对

**代码事实。** `RopeAttn` 组装 `LinearQKVMerged → (optional RMSNorm q/k) → AttentionLayer → LinearOProj`；`GatedMLP` 组装 `LinearColParallelMerged → silu/gelu_and_mul → LinearRowParallel`（`python/minisgl/models/utils.py:25-50,79-123`）。`AttentionLayer.forward` 依本 rank 的 q/k/v 维度 split fused QKV，在可选 q/k norm 和 RoPE 后调用全局 `ctx.attn_backend.forward(q,k,v,layer_id,ctx.batch)`（`python/minisgl/layers/attention.py:47-57`）。

**代码事实。** `LinearColParallelMerged` 将每个输出大小用 `div_even(..., tp_size)` 分片；`LinearQKVMerged` 同样把 Q 头均分，并以 `allow_replicate=True` 处理 KV 头（`python/minisgl/layers/linear.py:56-88`）。配对的 `LinearOProj` 与 `LinearRowParallel` 按输入维分片，均在多 TP rank 下对局部 `F.linear` 输出调用 communicator 的 `all_reduce`（`python/minisgl/layers/linear.py:91-127`）。这说明归约点在层内部，而非只存在于模型结尾。

**代码事实。** vocab 也遵循单独的 TP 契约：`VocabParallelEmbedding` 用 `div_ceil` 切 vocab，调用自有 `indexing` 并 all-reduce；`ParallelLMHead` 在 prefill 时依 attention metadata 的 `get_last_indices` 只取每序列最后 token，再在 TP 下 all-gather logits 并重排（`python/minisgl/layers/embedding.py:14-42,87-110`）。教材应把这条路径与 QKV/O 的“列后行”配对区分开。

### C. attention metadata、KV 存储与后端选择

**代码事实。** attention registry 注册 `trtllm`、`fi`、`fa`。`create_attention_backend` 支持单后端或恰有一个逗号的 `"prefill,decode"` 规格；不同的两端被包进 `HybridBackend`，其 `forward/prepare_metadata` 按 `batch.is_prefill` 选择，capture/replay 始终交 decode backend（`python/minisgl/attention/__init__.py:19-68`；`python/minisgl/attention/base.py:37-63`）。`BaseAttnBackend` 的接口还要求 metadata 准备和 graph capture/replay 准备方法（`python/minisgl/attention/base.py:18-34`）。

**代码事实。** KV pool 的抽象规定 `k_cache`、`v_cache`、`store_kv`、device/dtype/layer 数（`python/minisgl/kvcache/base.py:10-37`）。当前 `MHAKVCache` 按 `[2, layers, pages, page_size, local_kv_heads, head_dim]` 分配 buffer；local KV head 数也使用 replicate-aware `div_even`。它将一个 layer 的缓存展平到 token-location 视图，再用 `store_cache` 写入 `out_loc` 指定的位置（`python/minisgl/kvcache/mha_pool.py:10-56`）。因此 `out_loc` 是实际写入的索引输入；不能在本章把它笼统称作“page id”。

**代码事实。** `fa` 以 `batch.padded_reqs` 的 `extend_len/device_len/cached_len` 构造 seqlens 元数据；对全局 page table 以 `page_size` 步长切片并在大页时除以 page size（`python/minisgl/attention/fa.py:67-105`）。其 `forward` 先 `store_kv`，再从该 layer 的 KV cache 调用外部 `sgl_kernel.flash_attn.flash_attn_with_kvcache`；版本选择为 sm100 时 4，否则 3，源码对 FA4 Blackwell 支持留有 TODO（`fa.py:46,48-65,157-182`）。

**代码事实。** `fi` 的 `FIMetadata.__post_init__` 断言 `page_size==1`；其 forward 将 KV cache flatten 成 page length 为 1 的视图，先初始化 FlashInfer plan、写 KV，再调用 metadata wrapper 的 `run`（`python/minisgl/attention/fi.py:46-77,176-188`）。它使用 FlashInfer prefill/decode wrappers 且传 `backend="fa2"`；tensor core 使用可由 `MINISGL_FLASHINFER_USE_TENSOR_CORES` 覆盖，默认按 GQA（Q heads/KV heads）是否至少 4 决定（`fi.py:80-121,236-242`；`python/minisgl/env.py:67-84`）。连续 plan 会等待 `last_event`，因为源码注释指出复用了 pinned host staging buffer 与异步 H2D（`python/minisgl/attention/fi.py:123-166`）。

**代码事实。** `trtllm` 同样先写 KV；它按 `batch.is_prefill` 在 FlashInfer 的 `trtllm_batch_context_with_kv_cache` 与 `trtllm_batch_decode_with_kv_cache` 间选择，并有 128 MiB workspace（`python/minisgl/attention/trtllm.py:35-89`）。它的 metadata page-table 处理和 `fa` 一样允许以步长/除法适配大于 1 的 page size（`trtllm.py:91-129`）。

### D. MoE：复制 router、堆叠 expert、Triton 计算

**代码事实。** `MoEMLP` 用全尺寸的 `LinearReplicated(hidden_size, num_experts)` 得到 router logits，并将其与 hidden states 交给 `MoELayer`（`python/minisgl/models/utils.py:53-75`）。`MoELayer` 将 expert 权重保存为 `[E, 2*local_intermediate, hidden]` 的 gate-up 与 `[E, hidden, local_intermediate]` 的 down，并把 backend 输出在 TP 下 all-reduce（`python/minisgl/layers/moe.py:9-59`）。“router 在每 rank 相同”是对该 replicated layer 输入/权重相同且前序 TP all-reduce 正确这一执行前提的推论，不应表述为无条件保证。

**代码事实。** MoE registry 目前只注册 `fused`，工厂返回 `FusedMoe`（`python/minisgl/moe/__init__.py:16-27`）。`FusedMoe.forward` 先经 `fused_topk` 选择权重/id，再进入 `fused_experts_impl`（`python/minisgl/moe/fused.py:230-256`）。后者断言输入和两组权重 contiguous，允许输入 dtype 仅为 fp32/fp16/bf16；它调用 `moe_align_block_size` 使每个 expert 的 token block 对齐，执行两次 Triton fused matmul（中间插入 fused activation），再做 top-k 结果求和（`python/minisgl/moe/fused.py:127-227`）。

**代码事实。** 自有 Triton kernel 的 `fused_moe_kernel` 读取按 expert 排序的 token ids、选择相应 expert 权重 slab，在 fp32 accumulator 上累加后转换为 compute type；`moe_sum_reduce_kernel` 沿 top-k 维度累加（`python/minisgl/kernel/triton/fused_moe.py:5-47,50-193`）。Python dispatch 选择是否 K 维整除 block 并设置 masked/unmasked 路径（`python/minisgl/kernel/moe_impl.py:6-62`）。

## exact_source_anchors

下表是教材作者可直接打开的阅读锚点；“用途”是代码事实的索引，而非新增结论。

| 路径与符号 | 用途 |
|---|---|
| `python/minisgl/models/register.py:_MODEL_REGISTRY,get_model_class` | architecture 字符串、lazy import、未知架构失败。 |
| `python/minisgl/models/config.py:ModelConfig.from_hf,is_moe` | HF/text config 归一化、GQA/MoE/RoPE 字段。 |
| `python/minisgl/models/weight.py:_shard_tensor,load_weight` | TP 切分/复制、projection fusion、expert stacking。 |
| `python/minisgl/layers/base.py:BaseOP,StateLessOP,OPList` | plain-object 参数树和 state-dict 递归。 |
| `python/minisgl/models/llama.py:LlamaDecoderLayer,LlamaModel,LlamaForCausalLM` | dense decoder 的最短端到端样本。 |
| `python/minisgl/models/utils.py:GatedMLP,MoEMLP,RopeAttn` | dense/MoE/attention 的装配接缝。 |
| `python/minisgl/layers/linear.py:LinearQKVMerged,LinearOProj,LinearRowParallel` | TP shard shape 与两处 reduce。 |
| `python/minisgl/layers/attention.py:AttentionLayer.forward` | fused qkv split、q/k norm、RoPE、后端调用。 |
| `python/minisgl/attention/__init__.py:create_attention_backend`；`attention/base.py:HybridBackend` | 后端 registry 与 `p,d` 路由/capture 规则。 |
| `python/minisgl/kvcache/mha_pool.py:MHAKVCache.store_kv` | page buffer 布局与自有 store kernel 接缝。 |
| `python/minisgl/attention/fa.py:FlashAttentionBackend`；`fi.py:FlashInferBackend`；`trtllm.py:TensorRTLLMBackend` | 三后端 metadata、KV-write-before-read、page-size/hardware/workspace 差异。 |
| `python/minisgl/layers/moe.py:MoELayer`；`moe/fused.py:FusedMoe,fused_experts_impl` | MoE 权重布局、top-k、对齐、TP reduce。 |
| `python/minisgl/kernel/utils.py:load_aot,load_jit` | 自有 C++/CUDA kernel 的 AOT/JIT 编译边界。 |
| `python/minisgl/kernel/index.py:indexing`；`store.py:store_cache`；`radix.py:fast_compare_key`；`pynccl.py:init_pynccl` | 四类自有 wrapper 的职责地图。 |
| `python/minisgl/kernel/triton/fused_moe.py:fused_moe_kernel,moe_sum_reduce_kernel` | 自有 Triton MoE kernel。 |

## invariants_and_failure_modes

### 可由源码直接核查的不变量

- **注册与权重名必须同树。** `create_model` 仅使用 architectures 的第一个值（`python/minisgl/models/__init__.py:7-8`）；`BaseOP.load_state_dict` 对每个叶子的 shape/dtype 断言并在顶层拒绝剩余 key（`python/minisgl/layers/base.py:32-53`）。不支持的 architecture、错配的 tensor shape/dtype 或未消费 checkpoint key 都会中止装载，而不是安全降级。
- **TP 可分性不是性能提示。** Q heads、MLP intermediate 和 row-parallel 输入要求 `div_even`；KV replication 仅在 `tp_size > num_kv_heads` 且 `tp_size % num_kv_heads == 0` 时允许（`python/minisgl/utils/misc.py:20-31`；`python/minisgl/layers/linear.py:63-88`）。违反会触发 assert，不能靠后端选择修复。
- **Q/K/V 局部形状须保持一致。** `AttentionLayer` 断言 `num_qo_heads % num_kv_heads == 0`，再基于本 rank head 数切 qkv（`python/minisgl/layers/attention.py:29-56`）。错误的 config、loader shard 或 TP 设置会在构造/切分而非 attention 数学里显露。
- **KV 写入必须先于读取。** `MHAKVCache.store_kv` 以 `out_loc` 为索引调用 store kernel（`python/minisgl/kvcache/mha_pool.py:45-56`）；三个后端均在调用 attention 库前进行写入（`python/minisgl/attention/fa.py:48-65`；`fi.py:176-188`；`trtllm.py:49-89`）。若将顺序交换，本轮需要的 cache 内容没有代码路径保证已可读。
- **metadata 与后端必须配套。** `FIMetadata` 强制 page size 1（`python/minisgl/attention/fi.py:46-77`）；FA/TRTLLM 对 page table 使用 page-size 步长与整除转换（`fa.py:92-105`；`trtllm.py:116-129`）。把有大页的运行配置直接配给 `fi` 会断言失败；把一种后端 metadata 交给另一种后端会触发 `isinstance` 断言（各 backend 的 `forward`）。
- **FlashInfer plan 有宿主缓冲的时间边界。** `fi` 在再次 plan 前 `last_event.synchronize()`，然后 record 新 event（`python/minisgl/attention/fi.py:123-166`）。移除此顺序会让同一 pinned staging buffer 与尚未结束的异步拷贝竞争；这是代码注释明确指出的风险。
- **MoE 输入、权重与 block 约束必须成立。** `fused_experts_impl` 要求 contiguous 输入/权重和 fp32/fp16/bf16（`python/minisgl/moe/fused.py:127-145`）；`moe_align_block_size` 使 expert token block 可整除，Triton dispatch 为非整除 K 打开 mask（`moe/fused.py:31-89`；`kernel/moe_impl.py:25-37`）。把这些当作“可选调优”会误导学生。
- **自有 store/index kernel 有数据布局约束。** `indexing` 按 entry byte size 选 1/2/4 splits（`python/minisgl/kernel/index.py:31-50`）；C++ warp `copy/reset` 静态要求每次 copy 字节数是 128 的倍数（`python/minisgl/kernel/csrc/include/minisgl/warp.cuh:40-78`）。`StoreKernel` 还验证 CUDA device、stride、dtype 和 index dtype（`python/minisgl/kernel/csrc/jit/store.cu:55-120`）。

### 硬件与依赖边界

**代码事实。** 这里的“自有 kernel”包括：以 `tvm_ffi` 通过 `load_jit` 编译的 `index.cu/store.cu`、通过 `load_aot` 编译的 `radix.cpp/pynccl.cu`，以及 `kernel/triton/fused_moe.py`；编译 flags 含 C++20、O3 与 CUDA relaxed constexpr（`python/minisgl/kernel/utils.py:9-13,53-128`）。但 attention、norm、activation、RoPE 或 MoE 的一部分操作越过该边界：`fa` 导入 `sgl_kernel`，`fi/trtllm`、RMSNorm/activation/RoPE 导入 FlashInfer，MoE block align/top-k 也导入 `sgl_kernel`（`python/minisgl/attention/fa.py:157-182`；`fi.py:80-103`；`python/minisgl/layers/norm.py:8-37`；`activation.py:7-20`；`rotary.py:35-52`；`moe/fused.py:9-89`）。

**代码事实。** RoPE 要求 `rotary_dim == head_size`，且 head size 仅允许 64/128/256/512（`python/minisgl/layers/rotary.py:12-37`）。FA 根据 `is_sm100_supported()` 在 version 4/3 间选择，且代码对 Blackwell FA4 标有未完成 TODO（`python/minisgl/attention/fa.py:36-46,157-182`）。PyNCCL wrapper 的 dtype map 只有 fp16/bf16，要求 CUDA contiguous tensor；其内部 symmetric buffer 上限由 `MINISGL_PYNCCL_MAX_BUFFER_SIZE`（默认 1 GiB）限制（`python/minisgl/kernel/csrc/src/pynccl.cu:59-63,93-160`；`python/minisgl/kernel/pynccl.py:45-77`；`python/minisgl/env.py:67-84`）。这些均是仓库当前实现边界，不是通用 LLM serving 定律。

## pedagogical_story

**教学提案。** 用“调度器只交付地址簿，模型才执行搬运与计算”的叙事串起本章：

1. 调度章节交来 batch 的 token、position、`out_loc` 和 page-table 行；本章不重新解释它们如何分配。
2. 模型注册像按 checkpoint 标签挑选装配图；`BaseOP` 像不依赖 `nn.Module` 的零件目录。先让学生看 Llama 的一层，再看 Qwen3-MoE 的两个变体（q/k norm、MoE MLP）。
3. qkv 是本 rank 的局部件：RoPE 后不直接“算 attention”，而是先把 k/v 放到地址簿指定的共享仓库。backend 才根据 metadata 找到应读的历史和刚写的内容。
4. TP 的列并行像把“特征列”分给各卡，row/O 投影像把局部贡献加回完整 hidden state；因此 reduce 是模型层的语义步骤。
5. MoE 是在同一骨架中把 dense MLP 替成“复制的分流牌 + 多 expert 的局部计算 + 汇总”。Triton 是其中一段自有实现，FlashInfer/sgl_kernel 是另一段依赖边界，不能统称为“模型的一个 kernel”。

这个叙事应反复回指“当前实现”，避免把可替换 backend、CUDA Graph 或某个 vendor library 讲成 Transformer 的定义。

## demo_or_reading_lab

### 10–12 分钟白板演示（教学提案）

给学生三组卡片：`k,v,out_loc`、`store_kv`、`k_cache/v_cache + attention kernel`；两张 TP GPU 卡；以及 `page_size=1`/`page_size=16` 配置卡。

1. 让学生排出 K/V 卡片的先后顺序，并用 `fa.py:FlashAttentionBackend.forward` 或 `fi.py:FlashInferBackend.forward` 验证。
2. 让学生选择 backend：`fi` 只能接 page size 1；`fa` 与 `trtllm` 的源码有 page-table 采样/整除逻辑。要求回答“可用性约束”，不要求宣称实际性能排名。
3. 将 `LinearColParallelMerged` 的局部输出卡交给 `LinearRowParallel`，把 `all_reduce` 放在后者；用 `layers/linear.py` 验证。

### 20 分钟阅读实验（教学提案）

**目标：** 产出一页“从 HF architecture 到本轮 attention 输出”的有证据路径图。

1. 从 `models/register.py:get_model_class` 找 architecture 到类；从 `models/weight.py:load_weight` 标出一个 q/k/v 合并和一个 expert stack。
2. 以 `models/llama.py:LlamaDecoderLayer.forward` 为主线，在 `models/utils.py:RopeAttn` 和 `layers/attention.py:AttentionLayer.forward` 标注 qkv 的形状变化点。
3. 二选一阅读 `attention/fa.py` 或 `attention/fi.py`：圈出 metadata 准备、`store_kv`、库调用三处，并记录 page-size 限制。
4. 可选挑战：从 `layers/moe.py:MoELayer.forward` 追到 `moe/fused.py:fused_experts_impl`，写下为何需要对齐/contiguous，而非测量性能。

**验收（教学提案）。** 图中每个实现箭头附一个“路径:符号”；没有锚点的箭头必须标为推测或删去。

## misconceptions

- **“模型必然继承 `torch.nn.Module`。”** 不适用于此实现：参数遍历和加载由 `BaseOP` 的 `__dict__` 递归完成（`python/minisgl/layers/base.py:15-53`）。
- **“模型注册等于已经加载好权重。”** 不成立：注册/构造在 `register.py`，权重的 sharding/fusion/stacking 在独立的生成器 `weight.py:load_weight`（`python/minisgl/models/register.py:15-21`；`weight.py:75-124`）。
- **“所有 TP 层都会 all-reduce。”** 不成立：column-parallel layer 只做局部线性，O/row 层才在 TP>1 归约（`python/minisgl/layers/linear.py:56-127`）。
- **“KV 只服务下一 token。”** 不成立：各 backend 在当前 attention 调用前写本轮 K/V（`python/minisgl/attention/fa.py:48-65`；`fi.py:176-188`；`trtllm.py:49-89`）。
- **“attention backend 只影响快慢。”** 不成立：`fi` 明确断言 page size 1，hybrid 还把 prefill/decode 交给不同 backend（`python/minisgl/attention/fi.py:46-77`；`attention/base.py:37-63`）。
- **“MoE router 不通信，所以 TP 下必然正确。”** 不完整：router 是 replicated layer，但 expert down 输出仍要 all-reduce；其一致性依赖前序输入与复制参数的运行前提（`python/minisgl/models/utils.py:53-75`；`python/minisgl/layers/moe.py:45-59`）。
- **“`kernel/` 下的代码全是 CUDA kernel。”** 不成立：既有 C++ CPU radix wrapper、C++/CUDA JIT/AOT、Triton，也有调用 FlashInfer/sgl_kernel 的外部边界（`python/minisgl/kernel/radix.py:13-20`；`kernel/utils.py:53-128`；`kernel/triton/fused_moe.py:1-193`）。

## exercises

以下均为**教学提案**；答案必须引用所列源码，不能以经验判断代替。

1. **注册与树。** 给出 `architectures=["Qwen3MoeForCausalLM"]`，找出 class 选择点、MoE 判定字段、expert 权重从 checkpoint key 到 `[E,...]` 的转换位置。阅读：`models/register.py`、`models/config.py:is_moe`、`models/weight.py:_get_expert_stack_info/load_weight`。
2. **TP 配对。** 假设 `tp_size=2`，画 dense attention 的 QKV 分片、O projection 归约和 MLP gate-up/down 归约。指出哪些维度由 `div_even` 约束。阅读：`layers/linear.py`、`utils/misc.py:div_even`。
3. **后端可行性。** 给出 `page_size=8` 与 `backend="fi"`，说明会在哪个符号失败；再查 `fa` 或 `trtllm` 如何转换 page table。阅读：`attention/fi.py:FIMetadata.__post_init__`、`fa.py:prepare_metadata`、`trtllm.py:prepare_metadata`。
4. **顺序不变量。** 在任一 backend 的 `forward` 中写出“metadata → store → cache view → library call”四步，并解释将 store 移到 library call 后面的直接风险。阅读：`attention/fa.py`、`fi.py` 或 `trtllm.py`。
5. **MoE 前置条件。** 列出 `fused_experts_impl` 的三个可检查约束，并追踪 block padding 到 Triton kernel 的参数。阅读：`moe/fused.py`、`kernel/moe_impl.py`、`kernel/triton/fused_moe.py`。
6. **边界审计。** 对 `indexing`、`store_cache`、`fused_moe_kernel_triton`、FlashInfer RMSNorm 各标注“自有 wrapper/自有内核/外部库”角色。阅读：`kernel/index.py`、`kernel/store.py`、`kernel/moe_impl.py`、`layers/norm.py`。

## recommended_expanded_structure

以下是**扩写建议**，不是当前章节文本。

1. **先定边界（约 1 页）。** 复用课程总论中的“scheduler 给元数据、模型消费元数据”接口；列出本章不覆盖的调度/缓存逐出细节。
2. **模型从标签到对象（约 2 页）。** architecture registry → `ModelConfig.from_hf` → `BaseOP` tree → streaming checkpoint loader。用一张“HF 名称/权重名/对象属性名”对照表。
3. **一个 dense decoder token（约 3 页）。** 以 Llama 逐层走 embedding、fused norm、QKV、RoPE、backend、O projection、GatedMLP、LM head；在图中标出 TP reduce。
4. **attention 是 metadata + KV + backend 合约（约 3 页）。** 分开解释 `prepare_metadata`、`store_kv`、外部 wrapper；比较 fa/fi/trtllm 的 page size、capture、workspace 和硬件选择，不给无基准的性能结论。
5. **MoE 作为受约束的 MLP 替换（约 2 页）。** router、stacked experts、top-k/alignment、两次 Triton matmul、TP reduce；显式区分代码事实与“为何这样设计”的推断。
6. **kernel 边界与失败清单（约 2 页）。** 自有 C++/CUDA、Triton、sgl_kernel、FlashInfer、PyTorch；将 dtype/shape/alignment/device/事件顺序整理为前飞检查表。
7. **阅读实验与回扣（约 1 页）。** 使用本笔记的 lab；结尾回到下一章的“logits/token id 如何成为安全的流式文本”。

## limitations

**证据限制（代码事实）。** 本笔记基于静态阅读：未加载任何 HuggingFace checkpoint，未运行 CUDA/TP/MoE/attention backend，也未测量性能。因而不把源码中的 `fa2` 注释、GQA tensor-core heuristic、Triton block config 或 FA version 分支解释为跨 GPU 的性能结论（`python/minisgl/attention/fi.py:93-103,236-242`；`fa.py:46,181`；`python/minisgl/moe/fused.py:92-124`）。

**覆盖限制（代码事实）。** 当前 attention 注册表只列出 `trtllm`、`fi`、`fa`（`python/minisgl/attention/__init__.py:19-40`），MoE 注册表只列出 `fused`（`python/minisgl/moe/__init__.py:16-27`）；教材不应把它们推广为所有模型或 serving 系统的穷尽选项。`ModelConfig.is_moe` 是 `"moe" in model_type` 的字符串判定（`python/minisgl/models/config.py:36-38`），也应作为当前实现选择而非普适模型分类法讲解。

**待由后续作者/实验验证的提案。** 若要在扩写章中比较 backend 性能、验证 TP 数值等价、或把 hardware assumptions 变成部署建议，须另行在明确 GPU、CUDA、FlashInfer、sgl-kernel、NCCL 和模型版本组合上执行基准/正确性实验，并记录命令与结果；本研究笔记不能替代这些证据。
