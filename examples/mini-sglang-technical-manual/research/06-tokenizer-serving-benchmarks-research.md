# 第 06 章研究笔记：Tokenizer、服务流与基准测试

## scope

本文为后续扩写教材准备证据，不是最终章节，也不提出或实现修复。范围是：

- 文本如何在在线服务中变为 CPU `int32` token，再变为可显示的增量字符串；
- `python/minisgl/message/` 定义的消息，如何经 ZMQ 在前端、tokenizer、scheduler 和 detokenizer 间往返；
- `/generate` 与 `/v1/chat/completions` 的 SSE / 部分 OpenAI 风格行为，以及断连取消；
- 离线 `LLM` 基准与在线 HTTP 基准各自测量的边界、指标和可观测局限。

**代码事实。** 本章的在线进程由 `python/minisgl/server/launch.py:launch_server` 启动：主 scheduler、一个 detokenizer 和零或多个 tokenizer worker 都调用 `tokenize_worker`；`num_tokenizer == 0` 时 `ServerArgs.share_tokenizer` 为真，tokenizer 地址复用 detokenizer 地址（`python/minisgl/server/launch.py:47-111`，`python/minisgl/server/args.py:21-47`）。因此“detokenizer 是另一套函数”不准确：它是同一 worker 函数在不同地址承担的消息角色。

**不在范围内。** 本文不重新讲解 scheduler 的批选择、GPU forward、KV cache 或 tensor-parallel 数值计算；仅在消息进入/离开它们的位置标出接口。也不将本实现描述为完整生产级 OpenAI 服务或通用性能结论。

## teaching_goal_alignment

课程大纲给第 06 章 12–15 分钟，要求学生区分 token 化、增量 detokenize、SSE 三层职责，理解 Unicode/文本边界缓冲，并审慎解释 TTFT、TPOT、E2E 与在线/离线基准（`.rive-artifacts/minisglang-teaching-20260710/manual/teaching-manual-outline.md:357-402`）。本研究将目标落到下列可验证证据：

| 教学目标 | 可由代码回答的问题 | 代码锚点 |
|---|---|---|
| 区分“生成 token”和“发送文本” | sampled `next_token` 何时成为 `DetokenizeMsg`，何时才成为 `UserReply.incremental_output`？ | `python/minisgl/scheduler/scheduler.py:_process_last_data`（138-167）；`python/minisgl/tokenizer/detokenize.py:DetokenizeManager.detokenize`（70-112） |
| 读懂服务控制面 | `uid`、队列、批包装与 ZMQ 序列化怎样保持请求对应关系？ | `python/minisgl/server/api_server.py:FrontendManager`（99-150）；`python/minisgl/message/{tokenizer,backend,frontend,utils}.py` |
| 不把接口名称等同于语义 | 哪些 request 字段实际传入 `SamplingParams`，流 chunk 是什么对象？ | `python/minisgl/server/api_server.py:v1_completions`（255-310）、`stream_chat_completions`（160-188） |
| 正确解释性能数字 | 测量是否包含 HTTP/SSE/排队？token 数是预算还是实测？ | `python/minisgl/benchmark/client.py:benchmark_one`（202-248）、`process_benchmark_results`（320-404）；`benchmark/offline/{bench,bench_wildchat}.py` |

这与大纲的主线一致：用户观察的是安全的文本增量与端到端时间，而非 GPU 上的整数 id；一次 token 到达也未必对应一次非空字符串 chunk（同上大纲：367-373、395-401）。

## concept_to_execution_path

### 在线服务：文本到 SSE 的一条闭环

1. **HTTP 入站与标识。** `generate` 或 `v1_completions` 调用 `FrontendManager.new_user()`，它递增 `uid_counter`，并为该 uid 建立 `ack_map` 列表和 `asyncio.Event`（`python/minisgl/server/api_server.py:109-114,228-247,255-278`）。前端经 `send_one` 将 `TokenizeMsg(uid, text, sampling_params)` 放入异步 PUSH 队列；首次发送才创建后台 `listen()` 协程（同文件：116-132）。

2. **tokenize。** `tokenize_worker` 按类型把入队消息拆成 `TokenizeMsg`、`DetokenizeMsg`、`AbortMsg`（`python/minisgl/tokenizer/server.py:52-108`）。`TokenizeManager.tokenize` 对字符串调用 HF tokenizer 的 `encode(..., return_tensors="pt")`，拉平成一维并转为 `torch.int32`；对 chat message list 先调用 `apply_chat_template(..., tokenize=False, add_generation_prompt=True)`（`python/minisgl/tokenizer/tokenize.py:11-31`）。结果被包成 `UserMsg(uid, input_ids, sampling_params)` 发送给 scheduler（`python/minisgl/tokenizer/server.py:87-101`）。

3. **进入/离开 scheduler 的边界。** 在线且 rank 0 时，`SchedulerIOMixin` 从 `zmq_backend_addr` PULL `BaseBackendMsg`，并向 `zmq_detokenizer_addr` PUSH `BaseTokenizerMsg`；多 rank 时只由 rank 0 把控制消息经 PUB/SUB 转给其余 rank，并用 CPU process group 广播消息数（`python/minisgl/scheduler/io.py:27-65,88-122`）。scheduler 在 `_process_last_data` 取得 CPU 上的下一个 token，判断 `max_tokens` / EOS 结束条件，并为每个非 chunked 请求发出 `DetokenizeMsg(uid, next_token, finished)`（`python/minisgl/scheduler/scheduler.py:138-167`）。这不是“前端读 GPU token”：前端只会收到后续 `UserReply`。

4. **安全增量 detokenize。** `DetokenizeManager` 以 uid 保存 `DecodeStatus`；它累计 token id，分别 decode 从 `surr_offset` 到结尾、以及到 `read_offset` 的前缀，并以差值导出新增文本（`python/minisgl/tokenizer/detokenize.py:54-97`）。若新增结果以替换符 `�` 结尾，它不提交 surrogate/read 边界；否则提交边界。非提交分支通过 `find_printable_text` 在换行、CJK 或最后空格处分割，避免先显示可能变化的半词；已发送字符用 `sent_offset` 去重。完成时删除该 uid 状态（同文件：35-51、98-112）。因此某个 `DetokenizeMsg` 可产生空 `incremental_output`，仍是有 token 到达的正常结果。

5. **回到 HTTP。** worker 将结果组成 `UserReply(uid, incremental_output, finished)`，单条直接发送、多条用 `BatchFrontendMsg`（`python/minisgl/tokenizer/server.py:71-85`）。`FrontendManager.listen` 只把仍在 `ack_map` 中的 reply 加入队列并 set event；`wait_for_ack` 唤醒后 drain pending reply，看到最后一个 reply 的 `finished` 才删除 uid 的两张映射（`python/minisgl/server/api_server.py:116-150`）。最后流函数把它格式化为 SSE。

### 消息与编码的最小模型

**代码事实。** 这条链的应用消息是 dataclass，不是 HTTP 对象：`TokenizeMsg` 含 uid/text/sampling 参数，`UserMsg` 含一维 CPU `int32` tensor，`DetokenizeMsg` 含 sampled token 与 finished，`UserReply` 含增量字符串（`python/minisgl/message/tokenizer.py:22-43`，`backend.py:22-41`，`frontend.py:20-30`）。`serialize_type` 对一维 tensor 断言维度为 1，并将 NumPy bytes/dtype 置入 type-tagged dict；`deserialize_type` 用 `np.frombuffer(...).copy()` 再转 tensor（`python/minisgl/message/utils.py:20-35,52-69`）。队列以 msgpack 封包，PUSH/PULL 的 bind/connect 由 `create` 决定（`python/minisgl/utils/mp.py:12-30,54-81`）。

**教学提案。** 用下图让学生先标“数据形状”，再标“进程”，避免把所有箭头误读为 token 化：

```text
HTTP 请求
  -> TokenizeMsg(uid, text, params) --ZMQ--> tokenizer worker
  -> UserMsg(uid, CPU int32 input_ids, params) --ZMQ--> scheduler rank 0
  -> DetokenizeMsg(uid, next_token, finished) --ZMQ--> detokenizer worker
  -> UserReply(uid, incremental_output, finished) --ZMQ--> FrontendManager
  -> SSE data: ... --> 客户端
```

当 `num_tokenizer == 0` 时，图中前后两个 worker 盒子可合并为一个进程，但仍保留两类消息及两个逻辑阶段（`python/minisgl/server/args.py:21-47`）。

### SSE、OpenAI 风格接口和取消

**代码事实。** `/generate` 输出每个 `incremental_output` 的 `data: <text>\n`，完成后输出 `data: [DONE]\n`；`/v1/chat/completions` 只有当 `stream=True` 才使用 SSE，否则仍走同一消息/解码链并汇总成 JSON（`python/minisgl/server/api_server.py:152-188,228-247,280-310`）。chat SSE 首 chunk 写 `delta.role="assistant"`，有非空文本才写 `delta.content`，完成再发送 `finish_reason="stop"` 和 `[DONE]`（同文件：160-188）。

**代码事实。** 请求模型声明了 `n`、`stop`、`presence_penalty`、`frequency_penalty`，但 handler 构建 `SamplingParams` 时仅传 `ignore_eos`、`max_tokens`、`temperature`、`top_k`、`top_p`，且旁有 “support more sampling parameters” TODO（`python/minisgl/server/api_server.py:64-83,264-276`）。流 chunk 的 `object` 是 `text_completion.chunk`、id 前缀为 `cmpl-`；非流式是 `chat.completion`、id 前缀为 `chatcmpl-`，并固定返回全零 `usage`（同文件：170-187、293-310）。这构成“部分 OpenAI 风格”，不是完整兼容性的代码证据。

**代码事实。** 每次准备产出一个 SSE chunk 前，`stream_with_cancellation` 调用 `request.is_disconnected()`；发现断连即创建 `abort_user(uid)` task 并重新抛出取消（`python/minisgl/server/api_server.py:190-200`）。`abort_user` 先 sleep 0.1 秒、删掉前端 uid 状态，再发送 `AbortMsg`（同文件：202-209）。worker 转成 `AbortBackendMsg`（`python/minisgl/tokenizer/server.py:102-108`），scheduler 依次尝试从 prefill/decode manager abort 并释放资源（`python/minisgl/scheduler/scheduler.py:190-203`）。因前端先删 uid，之后已在途的 `UserReply` 被 `listen` 跳过（`python/minisgl/server/api_server.py:116-123`）；这是 best-effort 取消，而非撤回已经执行或已经采样的 GPU 工作。

### 两条 benchmark 路径

**离线：主要观察 engine/scheduler 路径。** `LLM` 以 `SchedulerConfig(..., offline_mode=True)` 构造 scheduler；`SchedulerIOMixin.__init__` 在 offline mode 把 `receive_msg` / `send_result` 改绑为 `LLM.offline_receive_msg` / `offline_send_result` 并提前返回，所以没有 ZMQ、FastAPI、SSE 或 `DetokenizeManager`（`python/minisgl/llm/llm.py:28-97`；`python/minisgl/scheduler/io.py:27-33`）。`LLM.generate` 将 prompt 放到 `pending_requests`，运行 `run_forever()`，最后以 `tokenizer.decode(status.output_ids)` 生成最终文本（`python/minisgl/llm/llm.py:42-97`）。它适于测 batch/模型执行或离线吞吐，不能直接代表用户侧 TTFT。

**在线：测从 OpenAI 客户端到 SSE chunk 的观测时间。** `benchmark_one` 对运行中的 `/v1` OpenAI async client 强制 `stream=True`，设置 `ignore_eos=True, top_k=1, temperature=0.0`，记录发请求和每个收到的 streamed chunk 的 `perf_counter`（`python/minisgl/benchmark/client.py:202-248`）。`benchmark_one_batch` 用 `asyncio.gather` 并发发起请求；`benchmark_trace` 以 `BenchmarkTrace.timestamp` 的相对时间 `sleep` 后发射，从而重放到达间隔（同文件：251-309）。这包含客户端、HTTP、SSE、前端及服务排队/计算，但“每个 chunk”不必等于“每个 token”，因为服务会发空 delta。

**代码事实。** `process_benchmark_results` 以第一个时间差为 TTFT，以其余时间差集合为 TPOT 样本，以首尾差为 E2E，并把所有 `tics` 长度作为 `num_tokens` 用于 token/s；分位数由排序后的经验索引取得（`python/minisgl/benchmark/client.py:320-404`）。因此输出中 `num_tokens` 是“收到的时间戳数量”，不是独立从服务确认的实际生成 token 数，且空 SSE chunk 会影响该观察口径。

**代码事实。** `benchmark/offline/bench.py` 固定 256 请求，随机 input 100–1024、budgeted output 100–1024，`ignore_eos=True`，先 warmup，再以 `sum(sp.max_tokens)/wall_time` 报吞吐（10-38）。`bench_wildchat.py` 下载 WildChat 首 shard，筛 English/Chinese、非 redacted/non-toxic 的首个 user turn，使用 chat template；其 `ignore_eos=False`，并以 `LLM.generate` 返回的 `token_ids` 总数除 wall time（`benchmark/offline/bench_wildchat.py:19-65,80-139`）。前者的 token 分子是预算，后者是实际输出 token，二者不可混为同一吞吐定义。

**代码事实。** `benchmark/online/bench_simple.py` 以 seed 42 合成至多 8192 token prompt，在 `TEST_BS=[64]` 上测并发 batch（20-73）。`benchmark/online/bench_qwen.py` 下载 Qwen trace、读前 1000 项并使用 `dummy=True` 生成/切片 prompt，再以 scale `[0.4,0.5,0.6,0.7,0.8,1.6]` 重放；`scale_traces` 将相对 timestamp 乘 scale，故较小 scale 压缩到达间隔、提高 offered load（`benchmark/online/bench_qwen.py:21-51`；`python/minisgl/benchmark/client.py:407-495`）。

## exact_source_anchors

以下是扩写教材可直接复核的精确锚点；行号以本研究时工作区版本为准。

| 主题 | 源文件与符号（行） | 可支持的最小事实 |
|---|---|---|
| 共享或拆分 worker | `python/minisgl/server/args.py:ServerArgs.share_tokenizer/zmq_tokenizer_addr`（21-47）；`server/launch.py:start_subprocess`（47-111） | `num_tokenizer=0` 共享地址，启动仍有一个 detokenizer worker。 |
| token 化 | `python/minisgl/tokenizer/tokenize.py:TokenizeManager.tokenize`（14-31） | chat template 或 encode 后为一维 `int32`。 |
| 安全文本增量 | `python/minisgl/tokenizer/detokenize.py:find_printable_text`（35-51）；`DetokenizeManager.detokenize`（70-112） | 状态按 uid 保存，替换字符/CJK/空格策略可导致空增量。 |
| worker 分流 | `python/minisgl/tokenizer/server.py:tokenize_worker`（31-110） | 三类入站消息分派、单/批消息包装、reply 转发。 |
| 消息数据契约 | `python/minisgl/message/tokenizer.py`（22-43）；`backend.py`（22-41）；`frontend.py`（20-30） | 每个方向的 uid 与 payload 字段。 |
| 线缆格式 | `python/minisgl/message/utils.py:serialize_type/deserialize_type`（20-69）；`utils/mp.py:Zmq*Queue`（12-151） | 1D tensor 的类型标签、msgpack、PUSH/PULL 与 PUB/SUB。 |
| scheduler 出口 | `python/minisgl/scheduler/io.py:SchedulerIOMixin`（27-65,124-133）；`scheduler.py:_process_last_data`（138-167） | rank 0 收/发控制消息，生成 `DetokenizeMsg`。 |
| SSE 与兼容范围 | `python/minisgl/server/api_server.py:stream_*`（152-210）；`v1_completions`（255-310） | chunk 形状、字段转发、断连和 non-streaming 汇总。 |
| 离线替代 I/O | `python/minisgl/llm/llm.py:LLM`（28-97）；`scheduler/io.py`（27-33） | offline mode 旁路 ZMQ/frontend。 |
| 在线指标 | `python/minisgl/benchmark/client.py:benchmark_one`（202-248）、`benchmark_trace`（287-309）、`process_benchmark_results`（320-404） | tick 记录、负载注入与指标计算。 |
| 脚本负载 | `benchmark/offline/bench.py`（10-38）；`bench_wildchat.py`（80-139）；`benchmark/online/bench_{simple,qwen}.py` | 合成/数据集/trace 重放配置。 |

## invariants_and_failure_modes

### 代码支持的不变量

- **uid 是整个在线请求链的关联键。** `TokenizeMsg`、`UserMsg`、`DetokenizeMsg`、`UserReply` 和三张前端状态表/映射都携带或索引它；教学图中的每一箭头必须保留 uid（`python/minisgl/message/{tokenizer,backend,frontend}.py`；`python/minisgl/server/api_server.py:99-150`）。
- **跨进程输入 token 是 CPU 一维 `int32`。** `TokenizeManager` 明确 `.view(-1).to(torch.int32)`，`UserMsg` 注释也限定 CPU 1D；序列化器对其他维度 assert（`python/minisgl/tokenizer/tokenize.py:24-30`，`python/minisgl/message/backend.py:33-36`，`utils.py:24-29`）。
- **完成信号须伴随消息流到达。** scheduler 依据 token budget/EOS 填 `finished`，detokenizer 在收到 finished 后删状态，前端以它停止并清理映射（`scheduler/scheduler.py:150-167`，`tokenizer/detokenize.py:105-110`，`server/api_server.py:134-150`）。若某环节丢失终止消息，后续状态清理与响应结束都无法由这些函数自行保证。
- **流式可显示文本是前缀，不保证每 token 有字符。** `sent_offset` 只让已经决定输出的前缀恰好发一次；非空 token 序列并不推出非空增量（`python/minisgl/tokenizer/detokenize.py:70-112`）。
- **offline mode 的结果是最终 decode，不是线上 detokenizer 的流。** `LLM.offline_send_result` 仅积累 output id，`generate` 最后 decode（`python/minisgl/llm/llm.py:71-97`）。

### 可观察失效模式与边界

- **无确认超时的启动阻塞。** `launch_server` 对 `ack_queue.get()` 做固定 `num_tokenizers + 2` 次等待，未传 timeout；任一需 ack 的 worker 未就绪会使启动停在此处（`python/minisgl/server/launch.py:105-111`）。
- **取消不是同步抢占。** 断连检查只发生在 generator 即将 yield chunk 时，abort 在异步 task 中且先等待 0.1 秒；scheduler 收到 abort 前的 forward/sample 仍可能产生 reply，之后只是在前端被丢弃（`python/minisgl/server/api_server.py:190-209,116-123`）。
- **请求过长会被静默地不进入正常回复流程。** `_process_one_msg` 对 `input_len >= max_seq_len` 只 warning 后 return；对过大 `max_tokens` 则原地缩短该消息参数（`python/minisgl/scheduler/scheduler.py:175-189`）。教材应将其列为“观察 server log 与客户端等待”的排障案例，而非声称有结构化 HTTP 错误。
- **严格客户端兼容风险。** 空 `incremental_output` 仍产生 `delta={}` 的普通 chunk；stream `object` 与非流式对象名不同，`usage` 不是实计数（`python/minisgl/server/api_server.py:160-188,293-310`）。这些是可从响应直接观察的限制。
- **字段接受不等于字段生效。** Pydantic 模型含 `stop`/penalty/`n`，但构造 `SamplingParams` 未读取它们（`python/minisgl/server/api_server.py:64-83,264-276`）。在线 benchmark 传的 `input_length_override` 也只是在 client `extra_body` 加入该键（`python/minisgl/benchmark/client.py:215-234`），而该 API 请求模型没有该字段；是否被忽略应通过实验验证，不能从 benchmark 参数名推断它已生效。
- **指标前提可能不成立。** `process_benchmark_results` 假设每个 request 至少有一个 delta（访问 `deltas[0]`）且有后续 delta 来形成 TPOT 样本（`python/minisgl/benchmark/client.py:327-360`）。很短输出、异常流或额外空 chunk 都可能使指标含义与“每个生成 token”的口头定义偏离。

## pedagogical_story

**建议叙事（提案）。** 从一个看似矛盾的现象开场：“模型刚生成了一个 token，浏览器为何没有新字符？”让学生先猜测网络慢，再沿实际链路发现答案：token id 是 scheduler 的输出；detokenizer 有权暂缓不安全文本；SSE 仍可带空 delta；所以“GPU token 数”“安全字符数”“SSE event 数”是三个不同计数。

第二幕将这种时间边界连接到取消：浏览器断开只让 frontend 在下一次产出前检测到状态，再把 abort 作为普通控制消息传回。它不能倒转已经在 GPU 或 ZMQ 中的工作。此处要求学生区分“客户不可见”与“计算从未发生”。证据是 `FrontendManager.abort_user` 先删 uid，而 `listen` 丢弃未知 uid（`python/minisgl/server/api_server.py:202-209,116-123`）。

第三幕转向性能数字：离线 `LLM` 切掉了 HTTP/ZMQ/SSE，适合问“这个调度/引擎在该 workload 下多快”；在线 client 则把发送到接收 chunk 的体验纳入时间，适合问“这种到达负载下客户看到多快”。两者都重要，但不能互相替代。这是大纲第 06 章“TTFT、TPOT、E2E 与兼容性/负载注入限制”目标的自然落点（`.rive-artifacts/minisglang-teaching-20260710/manual/teaching-manual-outline.md:357-412`）。

## demo_or_reading_lab

### 阅读实验：一张 uid 追踪表（提案）

1. 学生依次打开 `TokenizeMsg`、`UserMsg`、`DetokenizeMsg`、`UserReply` 定义，并在纸上写出每条的 uid 和 payload（`python/minisgl/message/{tokenizer,backend,frontend}.py`）。
2. 读 `tokenize_worker` 的三个 list comprehension，分别给三类消息涂色（`python/minisgl/tokenizer/server.py:69-108`）。
3. 从 `FrontendManager.new_user` 追到 `wait_for_ack`，指出 event 只负责唤醒、`ack_map` 才是积压 reply 的容器（`python/minisgl/server/api_server.py:109-150`）。
4. 读 `DetokenizeManager.detokenize`，填写“收到 `next_token` / 末尾是 `�` / 末尾空格或 CJK / finished”时各 offset 是否推进（`python/minisgl/tokenizer/detokenize.py:70-112`）。
5. 产出物：一张含五个箭头的消息时序图，并在 `DetokenizeMsg -> UserReply` 箭头旁写出“可为空”。

### 可运行的观测实验：验证字段和 chunk（提案）

在已有可运行 server 的前提下，使用两组相同 prompt：一组 `stream=true`，一组 `stream=false`。记录原始 SSE 行、最终 JSON、server log；比较 `object`、id 前缀、`usage`、空 delta 与最终 `[DONE]`。再分别加 `stop`、`n=2`、penalty 和未知的 `input_length_override`，以**响应、生成长度、server 端日志/插桩**判断字段是否真正影响执行。这个实验不能预设结果；静态代码只支持“当前 handler 没有把前述字段放入 `SamplingParams`”这一结论（`python/minisgl/server/api_server.py:264-276`）。

### 基准设计实验（提案）

固定模型、硬件、`max_tokens` 和并发度，至少输出两张表：

| 实验 | 保留的路径 | 应报告 | 不应声称 |
|---|---|---|---|
| `LLM.generate` | scheduler/engine 与最终 decode | wall time、实际 output ids、配置 | HTTP/SSE TTFT |
| `benchmark_one_batch` | client→HTTP→ZMQ→scheduler→SSE | TTFT/TPOT/E2E、请求数、实际响应事件 | 已隔离 GPU kernel 性能 |
| `benchmark_trace` 多 scale | 上述在线路径 + 到达间隔 | scale、trace 来源/是否 dummy、峰值 in-flight/queued | 真实生产 prompt 内容/缓存命中率已复现 |

表中 online 指标与计数实现可回查 `python/minisgl/benchmark/client.py:202-404`；离线输出 id 可回查 `python/minisgl/llm/llm.py:71-97`。

## misconceptions

- **“一个 token 就是一段可立刻显示的字符串。”** 不成立；detokenizer 会因替换字符、词边界或 CJK 策略缓冲，且以 uid 保持 offsets（`python/minisgl/tokenizer/detokenize.py:35-112`）。
- **“detokenizer 收到 token 就直接 `decode(token)`。”** 不成立；它对累积切片做两次 `batch_decode`，并以已发送字符串 offset 求增量（同文件：81-106）。
- **“ZMQ 传的是 GPU tensor。”** 本路径将 token 化结果变为 CPU 一维 `int32`；serializer 也仅允许一维 tensor（`python/minisgl/tokenizer/tokenize.py:24-30`，`python/minisgl/message/utils.py:24-29`）。
- **“`num_tokenizer=0` 代表没有 tokenization。”** 不成立；它表示 tokenizer 请求与 detokenize 请求复用一个 worker 地址/进程角色（`python/minisgl/server/args.py:21-47`，`server/launch.py:71-103`）。
- **“断开连接立即停止 GPU 计算。”** 不成立；取消以异步 AbortMsg 传递，已在途消息会在前端被过滤（`python/minisgl/server/api_server.py:190-209`，`python/minisgl/tokenizer/server.py:102-108`）。
- **“有 `/v1/chat/completions` 就完全兼容 OpenAI。”** 不成立；字段转发、chunk object、usage 都有明确差异（`python/minisgl/server/api_server.py:64-83,160-188,264-310`）。
- **“在线 TPOT 就是 GPU 单 token decode latency。”** 不成立；代码测相邻收到 SSE chunk 的间隔，包含服务和协议行为，且 chunk 可为空（`python/minisgl/benchmark/client.py:202-248,320-360`）。
- **“所有 benchmark 吞吐分子都是实际生成 token。”** 不成立；`offline/bench.py` 用 max_tokens 预算，而 WildChat 脚本统计返回 token ids（`benchmark/offline/bench.py:33-38`；`benchmark/offline/bench_wildchat.py:123-139`）。

## exercises

以下均为教学练习提案；前两题可纯阅读完成，后三题需在可运行环境中收集证据。

1. **链路填空。** 从 `TokenizeMsg` 到 SSE 写出五种消息/载荷，标出 token id、字符串、`finished`、uid 分别首次出现在哪里。答案必须引用 `message` dataclass 和 `tokenize_worker` 的具体符号。
2. **offset 推演。** 对一个由“英文半词、空格、CJK、EOS”组成的假定 decode 序列，逐行推演 `decoded_ids/read_offset/surr_offset/sent_offset`，并解释为何中间可产生 `""`。不可假定某模型的特定 vocab。
3. **取消时序。** 画出断连发生在 (a) 第一 chunk 前、(b) 两次 chunk 之间、(c) finished reply 已进入前端后三种时序；标注本代码能保证、不能保证和无法观察的点。以 `stream_with_cancellation` / `abort_user` 为依据。
4. **兼容性证据测试。** 录制 stream/non-stream 原始响应，检验 object、id、usage 和 `stop/n` 行为。报告要区分“request 可被模型解析”“handler 转发”“scheduler 实际行为”。
5. **公平对比设计。** 写一页实验计划，使离线吞吐与在线 TTFT 不互相冒充：给出固定变量、warmup、token 计数定义、trace 是否 dummy、失败请求处理与原始 tics 保存策略。

## recommended_expanded_structure

以下是**扩写建议**，不是现有代码结构。

1. **从用户文本到可观察结果：三个计数域。** 先分 token id、可显示字符、SSE event，给出整章问题和极简时序图。
2. **Tokenizer：chat template 与 CPU token 契约。** 阅读 `TokenizeManager.tokenize`；解释 chat list 与 raw string 的分叉、1D `int32` 约束和未批量 tokenize 的 TODO。
3. **消息总线：uid、dataclass、msgpack 与共享 worker。** 用代码锚点表解释四类主要消息、批包装、PUSH/PULL、TP rank 0 的 PUB/SUB fan-out，以及 `num_tokenizer=0` 的含义。
4. **Detokenizer：为什么要等待。** 逐步演示 DecodeStatus offsets、replacement character、空格/CJK heuristic、EOS 过滤与 finished 清理；明确 heuristic 的语言边界。
5. **前端与 SSE：部分 OpenAI 风格语义。** 分开 `/generate`、stream chat、non-stream chat；列出实际转发 sampling 参数、chunk 格式、usage 和缺失 endpoint，避免营销式“兼容”。
6. **取消和所有权。** 以 uid 删除、AbortMsg、scheduler 资源释放为主线，解释 best-effort 与已在途 reply 被过滤的可观察后果。
7. **Benchmark 的两把尺子。** 对比 offline `LLM` 与 online client/trace replay；定义 TTFT/TPOT/E2E 的当前实现口径，说明合成 prompt、dummy trace、预算 token 与实测 token。
8. **实验报告模板与限制清单。** 要求学生记录 commit/模型/硬件/配置、warmup、原始 SSE/tics、错误数、负载和 token 计数定义；用本笔记的 failure modes 做检查表。

## limitations

### 代码可见限制

- `TokenizeManager.tokenize` 明确标有“batch tokenization” TODO，当前逐条 loop（`python/minisgl/tokenizer/tokenize.py:14-30`）；不能据此声称 tokenizer 侧已有真正批 token 化优化。
- 文本 flush 规则是 borrowed heuristic：CJK 范围只覆盖代码列出的区段，其他书写系统/复杂 Unicode grapheme 的显示正确性不能由这段代码充分证明（`python/minisgl/tokenizer/detokenize.py:8-51`）。
- `FrontendManager` 没有在 handler 层提供显式 REST abort endpoint；当前取消入口是流式请求的 disconnect，shell 则使用 background task（`python/minisgl/server/api_server.py:190-209,319-347`）。
- `launch_server` 的 readiness barrier 没有 timeout（`python/minisgl/server/launch.py:105-111`），启动可靠性不能只凭“正常启动过一次”下结论。
- HTTP 请求模型对 `stop/n/penalty` 等字段的声明超出 handler 实际传递范围，非流式 `usage` 为零，且 streamed chat 对象值与其自身 non-streaming 响应不同（`python/minisgl/server/api_server.py:64-83,160-188,264-310`）。
- 在线 client 的 `input_length_override` 只是 `extra_body` 字段，而 API request model 没有同名字段；应把它视为待验证的 workload intent，不能当已强制的输入长度（`python/minisgl/benchmark/client.py:209-234`，`python/minisgl/server/api_server.py:64-83`）。
- `process_benchmark_results` 的 token/s 分子从 tics 个数得来，并非服务端 token accounting；尤其服务可能产生空 SSE delta（`python/minisgl/benchmark/client.py:320-384`，`python/minisgl/server/api_server.py:160-188`）。
- 离线和在线脚本硬编码模型、地址、样本数或 trace 下载地址（`benchmark/offline/bench.py:10-38`，`benchmark/online/bench_simple.py:34-40`，`benchmark/online/bench_qwen.py:21-45`）；结果的外推须另行记录环境与工作负载。

### 本研究的证据边界

本文基于指定仓库当前源码、既有第 06 章深读报告及课程大纲，没有实际启动模型服务、下载 benchmark 数据或测量 GPU。因此所有“会发生”的表述限于代码控制流；协议兼容性、字段是否被客户端/框架丢弃、特定 tokenizer 的字符行为以及性能数值，都应通过上述运行实验再作经验性结论。扩写教材应保留“代码事实 / 实验观察 / 教学提案”的标签，避免把建议或静态推断误写成已验证性能事实。
