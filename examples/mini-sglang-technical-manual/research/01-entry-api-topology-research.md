# 第 01 章研究笔记：入口、API 与进程拓扑

> 用途：为后续扩写教材提供经源码核对的材料，不是最终章节。本文把“代码事实”与“教学提案”分开；路径与符号是每项实现结论的阅读锚点。核对基线为课程大纲 `manual/teaching-manual-outline.md` 与先前报告 `minisglang-deep-read-20260709233312/01-entry-api-processes.md`，实现结论以当前源码为准。

## scope（范围）

### 代码事实

- 本笔记只覆盖用户如何进入系统、在线 worker 如何组织、跨进程消息如何往返，以及离线 `LLM` 如何复用调度核心；不展开 batch、KV cache、模型 forward 或 NCCL tensor 细节。入口是 `python/minisgl/__main__.py:5` 的 `launch_server()`、`python/minisgl/shell.py:4` 的 `launch_server(run_shell=True)`，离线入口是 `python/minisgl/llm/llm.py:28` 的 `LLM`。
- 三个用户表面是：（1）在线 HTTP 服务；（2）交互 shell；（3）离线 Python `LLM.generate`。前两者共享 `launch_server`、`run_api_server`、worker 拓扑，第三者继承 `Scheduler` 但设为 offline。锚点：`server/launch.py:40-44`、`server/api_server.py:411-452`、`llm/llm.py:28-37,77-98`。
- 先前报告中写作 `--shell` 的表述应在教材中改正：当前解析器注册的是 `--shell-mode`，另一个等价入口是模块 `minisgl.shell`；未见 `--shell` 参数。锚点：`server/args.py:220-234`、`shell.py:1-4`。

### 教学提案

- 将本章刻意限定为“请求尚未成为 GPU batch 之前的边界”。下章再从 `UserMsg` 进入 `Req`、prefill/decode 和执行；这样学生不会把 FastAPI、tokenizer、scheduler 混成一个函数。

## teaching_goal_alignment（与课程目标的对齐）

### 代码事实

课程大纲要求学生能说清三入口如何汇到 Scheduler + Engine、画出 parent/tokenizer/detokenizer/每 GPU scheduler 的职责，并理解 rank 0 前端 I/O 与启动屏障（`manual/teaching-manual-outline.md` 的“01 入口、API 与进程拓扑”）。当前实现支持用下列源码证据达成这些目标：

| 大纲目标 | 可用代码证据 | 可验证的结论 |
|---|---|---|
| 三入口汇合 | `__main__.py:5`；`shell.py:4`；`llm/llm.py:28-37` | HTTP/shell 走启动器；离线 `LLM` 是 `Scheduler` 子类。 |
| 每 GPU 一个 scheduler | `server/launch.py:54,59-69` | `world_size = tp_info.size`，循环为每个 rank 以替换后的 `DistributedInfo(i, world_size)` 启动一个 scheduler。 |
| tokenizer/detokenizer 职责 | `server/launch.py:71-103`；`tokenizer/server.py:67-108` | 固定启动一个 detokenizer 进程，并按 `num_tokenizer` 启动 tokenizer 进程；同一 worker 函数按消息类型分别处理 token 化、detoken 化和 abort。 |
| rank 0 I/O 边界 | `scheduler/io.py:35-65,124-133` | primary rank 建立 backend 收入和 detokenizer 输出；非 primary 不向 tokenizer 回结果。 |
| 就绪屏障 | `server/launch.py:105-111`；`launch.py:21-25`；`tokenizer/server.py:56-57` | 父进程等待 `num_tokenizers + 2` 条启动消息后才进入 uvicorn 或 shell。 |

### 教学提案

- 用“入口不同、调度核相同”作为本节可检验的中心句。学生完成后应能在图上圈出：只有在线模式跨 ZMQ；只有 HTTP 模式暴露路由；shell 不是第二套后端；离线并不是第二套 engine。

## concept_to_execution_path（概念到执行路径）

### 代码事实：配置先决定边界

- `ServerArgs` 是冻结 dataclass，继承 `SchedulerConfig`；`num_tokenizer == 0` 时 `share_tokenizer` 为真，`zmq_tokenizer_addr` 直接别名为 detokenizer 地址。分离时它使用另一 IPC 地址。锚点：`server/args.py:14-47`；基础地址定义在 `scheduler/config.py:14-41`。
- IPC 地址带 `_unique_suffix`，默认值是创建配置时的父进程 PID；启动 scheduler 时 `dataclasses.replace` 仅替换 `tp_info`，因此传入各子进程的配置保留同一个地址后缀。锚点：`scheduler/config.py:8-33`、`server/launch.py:59-66`。这是从代码结构得到的推论，不是跨平台部署承诺。
- `run_api_server` 先创建 `FrontendManager` 的异步 PULL/PUSH socket，随后调用 `start_backend()`；后者完成启动等待，之后才运行 uvicorn 或 shell。锚点：`server/api_server.py:430-452`、`server/launch.py:47-113`。

### 代码事实：在线请求与回复的逐段路径

```text
HTTP /generate 或 /v1/chat/completions
  -> FrontendManager.new_user + send_one(TokenizeMsg)
  -> [ZMQ PUSH/PULL] tokenizer worker
  -> TokenizeManager -> UserMsg / BatchBackendMsg
  -> [ZMQ PUSH/PULL] scheduler rank 0
  -> (TP>1：rank 0 raw PUB；CPU group 广播条数；其他 rank SUB)
  -> Scheduler 调度与 Engine
  -> rank 0 DetokenizeMsg
  -> [ZMQ PUSH/PULL] detokenizer worker
  -> UserReply / BatchFrontendMsg
  -> [ZMQ async PUSH/PULL] FrontendManager.listen
  -> ack_map + Event -> SSE 或聚合 JSON
```

- HTTP 路由把 prompt/messages 与采样参数封装为 `TokenizeMsg`；`/generate` 总是返回 SSE，`/v1/chat/completions` 按 `stream` 决定 SSE 或在 `wait_for_ack` 中聚合字符串返回 JSON。锚点：`server/api_server.py:228-247,255-310`；消息字段见 `message/tokenizer.py:35-43`。
- `FrontendManager.new_user` 分配递增 `uid`，为其建 `ack_map[uid]` 和 `asyncio.Event`；`listen` 收到每个 `UserReply` 后追加并 set event，`wait_for_ack` 在收到 `finished` 的最后回复后删除两个映射。锚点：`server/api_server.py:99-150`；`UserReply` 的字段在 `message/frontend.py:25-29`。
- `tokenize_worker` 从 listener 收集消息，按 `DetokenizeMsg`、`TokenizeMsg`、`AbortMsg` 分类。它把 token 化结果装入 `UserMsg(uid, input_ids, sampling_params)` 送 backend，把 detoken 化结果装入 `UserReply` 送 frontend，并将 abort 改为 `AbortBackendMsg`。锚点：`tokenizer/server.py:24-27,61-108`；消息定义：`message/backend.py:23-41`、`message/tokenizer.py:28-43`。
- scheduler 的 online I/O 只在 primary rank 建立 backend PULL 和 detokenizer PUSH；TP 大于 1 时，rank 0 将收到的原始 msgpack 字节经 PUB 发给其他 rank，同时通过 CPU process group 广播本轮待收原始消息的数量；其他 rank 从 SUB 接相同数量的消息。锚点：`scheduler/io.py:27-65,88-122`。rank 0 的 `send_result` 发送单个或批量 `DetokenizeMsg`，其他 rank 的方法为空操作：`scheduler/io.py:124-133`。
- scheduler 的结果处理在 `copy_done.synchronize()` 后将 token 变为 `DetokenizeMsg` 并调用 `send_result`；此处是本章可指向、但不展开 GPU/调度机制的“进入输出管道”边界。锚点：`scheduler/scheduler.py:138-167`。

### 代码事实：ZMQ 边界、socket 所有权与编码

| 边界 | 地址属性与方向 | bind/create 一方（代码） | 载荷 |
|---|---|---|---|
| 前端 → tokenizer | `zmq_tokenizer_addr`；async PUSH → PULL | 分离模式前端 bind；共享模式 detokenizer bind，前端 connect。`ServerArgs.frontend_create_tokenizer_link` / `tokenizer_create_addr` | `TokenizeMsg`、`AbortMsg`，或 batch。 |
| tokenizer → scheduler 0 | `zmq_backend_addr`；PUSH → PULL | scheduler 0 bind：`SchedulerIOMixin.__init__` | `UserMsg`、`AbortBackendMsg`，或 batch。 |
| scheduler 0 → detokenizer | `zmq_detokenizer_addr`；PUSH → PULL | 分离模式 scheduler 0 bind；共享模式 detokenizer bind | `DetokenizeMsg`，或 `BatchTokenizerMsg`。 |
| detokenizer → 前端 | `zmq_frontend_addr`；PUSH → async PULL | 前端 bind | `UserReply`，或 `BatchFrontendMsg`。 |
| scheduler 0 → 非 0 scheduler | `zmq_scheduler_broadcast_addr`；PUB → SUB | rank 0 bind | 原始 backend 消息字节。 |

表中地址与创建条件的源码锚点为 `scheduler/config.py:23-33`、`server/args.py:25-47`、`server/api_server.py:430-443`、`scheduler/io.py:35-62`、`tokenizer/server.py:43-45`。`utils/mp.py:12-151` 显示 PUSH/PULL/PUB/SUB 包装器按 `create` 选择 `bind` 或 `connect`，同步和 asyncio 版本都用 msgpack；`message/utils.py:20-35,52-69` 说明自定义类型编码，且 Tensor 仅允许一维并通过 numpy bytes 往返。`UserMsg.input_ids` 注释为 CPU 一维 int32 tensor：`message/backend.py:33-36`。

### 代码事实：启动、同步与两种“ack”

- `start_subprocess` 强制 multiprocessing start method 为 `spawn`，启动 `world_size` 个 scheduler、恰好一个 detokenizer、以及 `num_tokenizer` 个 tokenizer。进程名和 target 分别见 `server/launch.py:52-103`。应说“spawn”，不应说“fork”。
- 每个 scheduler 在构造 `Scheduler` 后先执行 `sync_all_ranks()` 的 CPU group barrier，只有 primary rank 向 `ack_queue` 放入启动消息；每个 tokenizer worker 在加载 tokenizer、建立两个发送 socket及 listener 后放入启动消息。锚点：`server/launch.py:16-25`、`scheduler/io.py:76-77`、`tokenizer/server.py:43-57`。
- 父进程精确读取 `num_tokenizers + 2` 条启动 ack（primary scheduler 1、detokenizer 1、各 tokenizer 各 1），再让 `run_api_server` 继续到对外服务或 shell。锚点：`server/launch.py:105-113`、`server/api_server.py:445-452`。
- 另一种“ack”是请求生命周期中的 `UserReply`：它不是启动队列的确认，也不是 HTTP 协议 ACK；只是前端按 `uid` 缓存的增量回复，最后以 `finished` 终止等待。锚点：`server/api_server.py:106-150`、`message/frontend.py:25-29`。

### 代码事实：shell 与离线路径

- shell 仍以 `run_api_server` 建立 `FrontendManager` 和后端，但以 `asyncio.run(shell())` 替代 `uvicorn.run`。它从 prompt_toolkit 读行，构造 `OpenAICompletionRequest`，经 `shell_completion` 发送同样的 `TokenizeMsg`，再读 `StreamingResponse.body_iterator` 输出。锚点：`server/api_server.py:319-409,411-452`。`parse_args` 在 shell 模式将 `cuda_graph_max_bs=1`、`max_running_req=1`、`silent_output=True`：`server/args.py:229-234`。
- `LLM.__init__` 以 `DistributedInfo(0, 1)` 和 `offline_mode=True` 创建 `SchedulerConfig` 后调用父类；`SchedulerIOMixin.__init__` 因 offline 把 `receive_msg` / `send_result` 改绑到对象的 `offline_receive_msg` / `offline_send_result` 并提前返回，所以该路径不创建 ZMQ I/O。锚点：`llm/llm.py:28-40`、`scheduler/io.py:27-34`。
- 离线接收函数在进程内从 `pending_requests` 逐个 token 化、受 `prefill_budget` 限制，构造 `UserMsg`；离线发送函数只收集 token id；`generate` 捕获所有请求完成信号后一次性 `tokenizer.decode(output_ids)` 生成最终文本。锚点：`llm/llm.py:42-98`。因此离线路径复用 Scheduler/Engine，却没有在线的 ZMQ、子进程或增量 detokenizer。

## exact_source_anchors（精确源码锚点）

| 阅读顺序 | 路径：符号（行） | 要确认的问题 |
|---:|---|---|
| 1 | `python/minisgl/__main__.py:5`；`python/minisgl/shell.py:4` | 命令模块分别如何进入普通服务与 shell？ |
| 2 | `python/minisgl/server/args.py:ServerArgs`（14-51），`parse_args`（54-268） | tokenizer 共享条件、IPC 地址、TP 与 shell 参数怎样形成配置？ |
| 3 | `python/minisgl/server/launch.py:_run_scheduler`（16-37），`launch_server`（40-113） | 谁 spawn、谁 ack、父进程何时解除屏障？ |
| 4 | `python/minisgl/server/api_server.py:FrontendManager`（99-213），`generate`（228-247），`v1_completions`（255-310），`run_api_server`（411-452） | HTTP/SSE 怎样进入 ZMQ、返回如何按 uid 等待？ |
| 5 | `python/minisgl/message/tokenizer.py`、`backend.py`、`frontend.py` | 每段边界的消息族与 `uid`/`finished` 含义。 |
| 6 | `python/minisgl/utils/mp.py:Zmq*Queue`（12-151）；`message/utils.py:serialize_type` / `deserialize_type`（20-69） | bind/connect、同步/异步、msgpack 和 tensor 编码限制。 |
| 7 | `python/minisgl/tokenizer/server.py:tokenize_worker`（31-110） | 一个 worker 为什么同时具备 token 化与 detoken 化处理分支？ |
| 8 | `python/minisgl/scheduler/io.py:SchedulerIOMixin`（27-133） | rank 0 的 socket 所有权与 TP 消息一致性怎样实现？ |
| 9 | `python/minisgl/llm/llm.py:LLM`（28-98） | 离线为何是进程内 I/O 替换，而不是另一套 scheduler？ |

## invariants_and_failure_modes（不变量与失效模式）

### 代码事实

1. **配置地址一致性。** 在线相互通信的进程必须拿到同一 `ServerArgs` 地址集合；共享 tokenizer 时前端地址必须等于 detokenizer 地址，分离时必须不同。源码直接表达前者别名和后者 `assert`：`server/args.py:30-39`。这是 socket 能配对的前提。
2. **TP 消息顺序/数目一致性。** 多 rank 下 rank 0 先转发原始消息，另以 CPU group 广播本轮数量；非 0 rank 按此数量 `get()`。教材应强调“相同顺序输入”是这一 I/O 设计的约束，不能把每张 GPU 当独立 HTTP server。锚点：`scheduler/io.py:88-122`。
3. **只由 rank 0 对外产生模型回复。** primary 的 `send_result` 写 detokenizer socket，其他 rank 显式 no-op；否则同一请求会有重复输出。锚点：`scheduler/io.py:124-133`。
4. **请求回复的终止标志。** `wait_for_ack` 只有在至少一个待处理回复且最后迭代变量 `ack.finished` 为真时退出；发送路径用 scheduler 给出的 `DetokenizeMsg.finished` 生成 `UserReply.finished`。锚点：`server/api_server.py:134-150`、`tokenizer/server.py:71-85`、`scheduler/scheduler.py:151-167`。
5. **启动屏障计数。** 当前代码的期待值固定为 `num_tokenizers + 2`，而不是 `world_size + num_tokenizers + 1`，因为只有 primary scheduler ack。锚点：`server/launch.py:24-25,105-111`。
6. **在线载荷形状。** message serializer 断言 Tensor 为一维，且 `UserMsg` 约定 CPU int32；课堂实验不能向该边界假定任意维 GPU tensor。锚点：`message/utils.py:24-29,54-61`、`message/backend.py:33-36`。

### 代码可见的失效模式或限制

- **启动可无限等待。** `ack_queue.get()` 没有 timeout 或对子进程 exit 状态的检查；任一预期 worker 未发 ack 时，父进程会阻塞在该调用。锚点：`server/launch.py:105-111`。这是可从代码直接观察的阻塞风险，不能表述为已实现了超时/健康检查。
- **异常路径不对称。** `_run_scheduler` 只显式捕获 `KeyboardInterrupt`；`tokenize_worker` 也只捕获 `KeyboardInterrupt`。其他初始化或循环异常不会转化成失败 ack。锚点：`server/launch.py:30-37`、`tokenizer/server.py:56-110`。这解释了为何启动屏障不等价于持续健康检查。
- **丢弃超长输入不会生成用户完成回复。** scheduler 发现 `max_output_len <= 0` 后仅记录 warning 并 return；前端对该 uid 仍会等待 `finished` 回复。锚点：`scheduler/scheduler.py:175-189`、`server/api_server.py:134-150`。教材应将其称为“由代码路径推得的挂起风险”，不要宣称已复现。
- **取消是异步请求，不是即时 GPU 取消。** HTTP 断开触发 `abort_user`，它先 sleep 0.1 秒、清理前端 map，再送 `AbortMsg`；scheduler 仅在以后处理到 `AbortBackendMsg` 时从 prefill/decode manager 查找并释放资源。锚点：`server/api_server.py:190-209`、`tokenizer/server.py:102-108`、`scheduler/scheduler.py:190-202`。因此不能教学为“断开立即停止正在运行的 forward”。
- **共享模式的吞吐隔离有限。** `num_tokenizer=0` 时一个 worker 同时处理两类消息，且 launch 给它 `local_bs=1`；这是实现结构事实，不足以单凭源码断言其端到端性能。锚点：`server/args.py:21-39`、`server/launch.py:71-87`、`tokenizer/server.py:61-72,87-108`。

### 教学提案

- 把不变量做成“代码审计卡”：学生为每一张卡配一个路径和符号，不能只用“通常的 serving 架构”作答。
- 要求学生区分三种状态：worker 已发 startup ack、HTTP 请求已入 frontend map、请求已经收到 `finished` reply。三者发生在不同队列/映射中，不能互相替代。

## pedagogical_story（教学叙事）

### 教学提案

1. **从一个看似简单的问题开始：**“把一条聊天请求交给模型，为什么要经过三个进程角色？”先展示三入口，而不展示所有源码。
2. **让学生画边界，而非背类名：**文本入站、token id 入 scheduler、单 token 回 detokenizer、增量字符串回前端；在每条箭头上写 `TokenizeMsg`、`UserMsg`、`DetokenizeMsg`、`UserReply`。
3. **引入 TP 的反直觉：**多 GPU 不意味着多 HTTP server。rank 0 是外界 I/O 闸门，其他 rank 从 PUB/SUB 与 count 广播取得等量消息；GPU tensor 通信留到后章。
4. **用“双 ack”收束：**启动 ack 让父进程等 worker；`UserReply` 让某个 uid 等生成结束。它们名字相近、语义相差很大。
5. **最后切换离线镜头：**删去所有 ZMQ 箭头，留下 `LLM(Scheduler)` 的进程内替代函数。学生应由此理解“复用核心”而非“离线较简单所以另写一套”。

每一步应在讲义脚注给出对应的 `exact_source_anchors`，并明确这段是教学组织，不是仓库自动生成的流程图。

## demo_or_reading_lab（演示或阅读实验）

### 教学提案：15 分钟“画线—验线”实验

材料：打印或投影 `ServerArgs`、`launch_server.start_subprocess`、`FrontendManager`、`tokenize_worker`、`SchedulerIOMixin`、`LLM` 的锚点段落；不要求启动 CUDA 模型。

1. **两分钟，入口分类。** 学生把 `__main__.py`、`shell.py`、`LLM.generate` 贴到“HTTP / shell / offline”三列，并在后两列标识是否起 uvicorn、是否起子进程、是否经过 ZMQ。
2. **五分钟，端点配对。** 给出 `num_tokenizer=0` 与 `num_tokenizer=2` 两张空图。学生依据 `ServerArgs` 和 queue 构造处标注每个 bind/connect 与消息类型；必须解释为什么共享时 tokenizer 地址与 detokenizer 地址相同。
3. **四分钟，TP 追踪。** 设 `tp_info.size=2`。学生从 `SchedulerIOMixin._recv_msg_multi_rank0` 与 `_recv_msg_multi_rank1` 写出“raw payload”与“count”两条不同同步线，并圈出唯一的 `send_result`。
4. **四分钟，离线改图。** 删除所有 worker 和 ZMQ 箭头，使用 `LLM.offline_receive_msg`、`offline_send_result` 和最终 `tokenizer.decode` 补上三处进程内边界。

验收问题：为什么启动 ack 数量不是 world size 加全部 worker 数？为何非流式 HTTP 仍使用 `wait_for_ack`？答案应分别引用 `launch.py:105-111` 与 `api_server.py:286-310`。

## misconceptions（常见误解）

### 代码事实与纠正

| 误解 | 代码支持的纠正 |
|---|---|
| “每一个 TP rank 都接收 HTTP。” | HTTP 路由只访问父进程的 `_GLOBAL_STATE`/`FrontendManager`，而 scheduler I/O 仅 primary 创建外部 backend/detokenizer socket；见 `api_server.py:225-310`、`scheduler/io.py:35-65`。 |
| “`num_tokenizer=0` 就不做 tokenization。” | 它让 tokenizer 地址别名 detokenizer 地址；固定 detokenizer worker 内仍创建 `TokenizeManager`，并处理 `TokenizeMsg`；见 `args.py:21-39`、`launch.py:71-87`、`tokenizer/server.py:50-54,87-101`。 |
| “shell 是直接在模型上调用。” | shell 经 `shell_completion` 调 `FrontendManager.send_one(TokenizeMsg)`，后端仍由 `run_api_server` 启动；见 `api_server.py:319-346,411-452`。 |
| “离线仍需 ZMQ 和 detokenizer worker。” | offline mode 在 I/O mixin 直接替换收发函数；`LLM` 最后自行 decode；见 `scheduler/io.py:30-33`、`llm.py:71-98`。 |
| “所有 ack 指同一件事。” | 启动 ack 是 `multiprocessing.Queue` 字符串，回复 ack 是 `ack_map` 中 `UserReply`；见 `launch.py:55-57,105-111`、`api_server.py:106-150`。 |
| “启动等待完成就表示服务始终健康。” | 当前等待只读取固定数量的 queue 项，未见 timeout/子进程存活检查；见 `launch.py:105-111`。 |

## exercises（练习）

### 教学提案

1. **地址推导。** 给定 `server_port=1919`、`num_tokenizer=0`，列出五个 ZMQ 属性名，指出哪两个必相同，并说明 `distributed_addr` 的端口。要求引用 `scheduler/config.py:23-33`、`server/args.py:25-51`，不要臆造 PID 后缀的具体值。
2. **启动计数。** 令 TP world size 为 4、`num_tokenizer=3`。父进程等待几条启动 ack？哪些进程不发 ack？依据 `launch.py:16-25,105-111` 给出理由。
3. **在线追踪。** 从 `/v1/chat/completions` 发出一个 stream 请求，为每个边界写出消息类型、序列化方式、消费方。锚点至少包含 `api_server.py:255-284`、`tokenizer/server.py:67-108`、`scheduler/io.py:88-133`、`utils/mp.py:12-98`。
4. **失败分析。** 仅根据控制流，解释“tokenizer 在启动前抛异常”和“prompt 长度不留输出空间”各可能让用户观察到什么。区分确定事实与推断；引用 `launch.py:105-111`、`tokenizer/server.py:43-57`、`scheduler/scheduler.py:175-189`。
5. **离线改写图。** 画出 `LLM.generate` 的数据流，标出复用的类和被替代的通信函数；引用 `llm.py:28-98` 与 `scheduler/io.py:27-34`。

## recommended_expanded_structure（建议的扩写结构）

### 教学提案

1. **开场：三种表面，一个调度核（1–2 页）。** 先给对比表，标注 HTTP、shell、offline 的进程数与网络边界。
2. **启动前的配置契约（2 页）。** 讲 `ServerArgs` 的共享/分离 tokenizer 开关、PID 后缀与地址创建者；插入 `num_tokenizer=0/2` 对照图。
3. **parent 与 worker 拓扑（2–3 页）。** 逐步构建 spawn、scheduler rank、detokenizer、tokenizer；再讲 spawn 而不是 fork 的源码事实。
4. **一条在线请求（3–4 页）。** 按四个消息族和四个 ZMQ 数据边界讲解；单列“ZMQ 控制/载荷边界”和“TP 消息一致性”，不提前展开 GPU collective。
5. **就绪与回复不是同一个 ack（1–2 页）。** 用时序图并列 `ack_queue` 与 `ack_map/event_map`；紧接着分析无 timeout、异常不转 ack 的局限。
6. **shell 与 offline 的对照（2 页）。** shell 保留在线拓扑但撤掉 HTTP listener；offline 去除 ZMQ/worker、以方法绑定替代 I/O。明确 shell 的 `--shell-mode` 实名。
7. **阅读实验、误解、练习与交接（2 页）。** 用本笔记的实验和练习收束，再交给下一章的 `UserMsg -> Req`。

建议将每个图旁都放一行“代码事实 / 教学简化”标签。例如，拓扑图可以简化 batch 和 scheduler 内部，但不得把非 primary rank 画成独立外部回复者。

## limitations（限制）

### 代码事实的边界

- 本笔记没有运行模型、HTTP 服务或多卡 TP；所有行为结论来自静态源码阅读。因此把“可能挂起”“可能阻塞”限定为控制流风险，而非现场复现结果。
- `tokenize_worker` 虽创建 token 化与 detoken 化 manager，但实际哪个消息抵达哪个 worker 取决于地址配置和运行时消息；本笔记不把它简化为“独立 tokenizer 进程绝不含 detokenize 代码”。锚点：`tokenizer/server.py:50-70`。
- 本章仅说明 scheduler I/O 的 CPU group count 广播和 ZMQ PUB/SUB；NCCL/PyNCCL tensor collective 的选择、Engine stream、batch 调度正确性由后续章节处理。可见边界是 `scheduler/io.py:100-122` 与 `scheduler/scheduler.py:98-105`，不应从此推出完整分布式性能结论。
- HTTP 兼容性应按实际路由与字段说，不应泛称完整 OpenAI 兼容：当前可见实现包含 `/generate`、`/v1/chat/completions`、`/v1/models` 与 `/v1`，且 chat route 有“support more sampling parameters”的 TODO。锚点：`server/api_server.py:228-316`、`264-276`。

### 教学提案的边界

- 扩写教材应把本文建议的图、实验、提问明确标为教学材料，不要伪装成仓库中的诊断、超时或健康检查功能。
- 代码行号可能随版本移动；正式教材应保留“路径 + 符号”为主锚点，并在发布前以目标提交重新核对行号。
