# Google ADK Go 精读 Workflow

这个 workflow package 是在粗读产物基础上设计的下一轮精读 DAG。它不是一次性
静态计划，而是可以被 Rive 导入和复跑的模板：

- `workflow.yaml` 定义精读 DAG；
- `prompts/*.md` 定义每个节点的中文精读 prompt；
- `repo_path` 指向本地 `google/adk-go` 仓库；
- `output_dir` 指向本轮精读产物目录。

## DAG 结构

```text
root
  -> runtime-flow-deep-dive
  -> state-lifecycle-deep-dive
  -> tool-system-deep-dive
  -> callback-plugin-deep-dive
  -> workflow-a2a-deep-dive
  -> entrypoint-deploy-deep-dive
  -> final-architecture-guide

final-architecture-guide depends_on all six deep-dive nodes
```

前六个节点并行阅读不同架构分区，最后一个节点只消费六份精读产物，不重新
通读仓库，避免 final 节点上下文被源码细节打爆。

## 使用方式

在 Rive 仓库中验证模板：

```sh
rive workflow validate examples/google-adk-go-code-reading/workflows/deep-read
```

导入模板：

```sh
rive workflow import examples/google-adk-go-code-reading/workflows/deep-read \
  --command-id import-google-adk-go-deep-read-v1 \
  --bump-if-changed
```

只实例化 Work DAG，不启动 scheduler：

```sh
rive workflow run google-adk-go.deep-read \
  --command-id run-google-adk-go-deep-read-dry \
  --no-scheduler \
  --param repo_path=/Users/likun/Desktop/workspace-for-google-adk-go/adk-go \
  --param output_dir=/tmp/rive-google-adk-go-deep-read
```

真实运行时建议先用 OpenCode，`auto-reported` 即可，因为所有节点都只写
`output_dir` 下的 Markdown，不需要改源码：

```sh
rive workflow run google-adk-go.deep-read \
  --command-id run-google-adk-go-deep-read-$(date +%Y%m%d%H%M%S) \
  --runner opencode \
  --worker opencode-reader-a \
  --worker opencode-reader-b \
  --worker opencode-reader-c \
  --worker opencode-reader-d \
  --max-parallel 4 \
  --acceptance-mode auto-reported \
  --workspace-mode shared \
  --timeout-seconds 1800 \
  --param repo_path=/Users/likun/Desktop/workspace-for-google-adk-go/adk-go \
  --param output_dir=/tmp/rive-google-adk-go-deep-read
```

## 产物约定

每个 prompt 都要求 worker 输出中文 Markdown，并固定输出文件名：

- `01-runtime-flow-deep-dive.md`
- `02-state-lifecycle-deep-dive.md`
- `03-tool-system-deep-dive.md`
- `04-callback-plugin-deep-dive.md`
- `05-workflow-a2a-deep-dive.md`
- `06-entrypoint-deploy-deep-dive.md`
- `00-final-architecture-guide.md`

每份精读报告都必须回答：

- 面临的问题是什么；
- 为什么这是问题；
- 解决思路是什么；
- ADK Go 代码怎么落地。

同时必须包含源码路径、关键类型/函数、调用链、测试证据、风险和后续追问。
