package main

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	compose "github.com/rive/eino-compose-runtime-replica-go/compose"
)

func main() {
	fmt.Println("=== Eino Compose Runtime Replica — Chapter 1 / 2 / 3 / 4 / 5 综合示例 ===")
	fmt.Println()

	fmt.Println("========== 第一章示例 (Graph/DAG/Pregel/Info/EventLog) ==========")
	fmt.Println()
	example1_DAGBasic()
	example2_PregelWithMaxSteps()
	example3_CompileBoundary()
	example4_GraphInfo()
	example5_EventLog()

	fmt.Println()
	fmt.Println("========== 第二章示例 (FieldMapping/Workflow/Chain/Parallel/Branch) ==========")
	fmt.Println()
	example6_FieldMapping()
	example7_Workflow()
	example8_Chain()
	example9_Parallel()
	example10_Branch()
	example11_SkippedFeatures()

	fmt.Println()
	fmt.Println("========== 第三章示例 (Runnable Stream / Collect / Transform / Callback) ==========")
	fmt.Println()
	example12_RunnableStream()
	example13_StreamCollect()
	example14_StreamTransform()
	example15_CallbackTiming()

	fmt.Println()
	fmt.Println("========== I3 Bridge Adapter 示例 (RAG pipeline: Retriever + ChatModel 桥接) ==========")
	fmt.Println()
	example16_RAGPipeline()
	example17_BridgePatternExplanation()

	fmt.Println()
	fmt.Println("========== I3 Prompt/Tool Bridge 示例 (PromptTemplate → ToolCall → ToolsNode → Response) ==========")
	fmt.Println()
	example19_ToolCallingPipeline()
	example20_ToolCallingPipelineChain()

	fmt.Println()
	fmt.Println("========== 第四章示例 (Checkpoint / Interrupt / Resume) ==========")
	fmt.Println()
	example18_CheckpointInterruptResume()
}

func example1_DAGBasic() {
	fmt.Println("--- Example 1: Basic DAG Graph (AllPredecessor) ---")

	g := compose.NewGraph[string, string]()
	g.AddLambdaNode("upper", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return fmt.Sprintf("[UPPER:%s]", in), nil
	}))
	g.AddLambdaNode("reverse", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		runes := []rune(in)
		for i, j := 0, len(runes)-1; i < j; i, j = i+1, j-1 {
			runes[i], runes[j] = runes[j], runes[i]
		}
		return string(runes), nil
	}))

	g.AddEdge(compose.START, "upper")
	g.AddEdge("upper", "reverse")
	g.AddEdge("reverse", compose.END)

	r, err := g.Compile(context.Background(),
		compose.WithGraphName("basic_dag"),
		compose.WithNodeTriggerMode(compose.AllPredecessor),
	)
	if err != nil {
		fmt.Printf("Compile error: %v\n", err)
		return
	}

	result, err := r.Invoke(context.Background(), "hello world")
	if err != nil {
		fmt.Printf("Invoke error: %v\n", err)
		return
	}

	fmt.Printf("Input:  %q\n", "hello world")
	fmt.Printf("Output: %q\n", result)
	fmt.Printf("Expected: reversed upper-cased string\n\n")
}

func example2_PregelWithMaxSteps() {
	fmt.Println("--- Example 2: Pregel Graph (AnyPredecessor) with maxSteps ---")

	g := compose.NewGraph[int, int]()
	g.AddLambdaNode("increment", compose.InvokableLambda(func(ctx context.Context, in int) (int, error) {
		return in + 1, nil
	}))

	g.AddEdge(compose.START, "increment")
	g.AddEdge("increment", compose.END)

	r, err := g.Compile(context.Background(),
		compose.WithGraphName("pregel_example"),
		compose.WithNodeTriggerMode(compose.AnyPredecessor),
		compose.WithMaxRunSteps(50),
	)
	if err != nil {
		fmt.Printf("Compile error: %v\n", err)
		return
	}

	result, err := r.Invoke(context.Background(), 41)
	if err != nil {
		fmt.Printf("Invoke error: %v\n", err)
		return
	}

	fmt.Printf("Input:  %d\n", 41)
	fmt.Printf("Output: %d\n", result)
	fmt.Printf("Expected: 42\n\n")
}

func example3_CompileBoundary() {
	fmt.Println("--- Example 3: Compile Boundary ---")

	g := compose.NewGraph[string, string]()
	g.AddLambdaNode("echo", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	}))
	g.AddEdge(compose.START, "echo")
	g.AddEdge("echo", compose.END)

	r, err := g.Compile(context.Background(),
		compose.WithGraphName("compile_boundary"),
		compose.WithNodeTriggerMode(compose.AllPredecessor),
	)
	if err != nil {
		fmt.Printf("Compile error: %v\n", err)
		return
	}

	err = g.AddLambdaNode("new_node", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	}))
	if err != nil {
		fmt.Printf("Expected error after compile: %v\n", err)
	}

	_ = r
	result, err := r.Invoke(context.Background(), "boundary test")
	if err != nil {
		fmt.Printf("Invoke error: %v\n", err)
		return
	}
	fmt.Printf("Result: %q\n\n", result)
}

func example4_GraphInfo() {
	fmt.Println("--- Example 4: GraphInfo Introspection ---")

	g := compose.NewGraph[string, string]()
	g.AddLambdaNode("node_a", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	}))
	g.AddLambdaNode("node_b", compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return in, nil
	}))
	g.AddEdge(compose.START, "node_a")
	g.AddEdge("node_a", "node_b")
	g.AddEdge("node_b", compose.END)

	r, err := g.Compile(context.Background(),
		compose.WithGraphName("introspect_graph"),
		compose.WithNodeTriggerMode(compose.AllPredecessor),
	)
	if err != nil {
		fmt.Printf("Compile error: %v\n", err)
		return
	}

	_ = r

	info := g.GetGraphInfo()
	fmt.Printf("Graph Name:     %s\n", info.Name)
	fmt.Printf("Trigger Mode:   %s\n", info.TriggerMode)
	fmt.Printf("DAG Mode:       %v\n", info.DAGMode)
	fmt.Printf("Pregel Mode:    %v\n", info.PregelMode)
	fmt.Printf("Max Steps:      %d\n", info.MaxSteps)
	fmt.Printf("Num Nodes:      %d\n", info.NumNodes)
	fmt.Printf("Num Edges:      %d\n", info.NumEdges)
	fmt.Printf("Input Type:     %s\n", info.InputType)
	fmt.Printf("Output Type:    %s\n", info.OutputType)
	fmt.Println()
}

func example5_EventLog() {
	fmt.Println("--- Example 5: Event Log ---")

	el := compose.NewEventLog()
	el.LogGraphStart("event_demo")
	el.LogNodeStart("node_1", 1, "input_data")
	el.LogNodeEnd("node_1", 1, "output_data")
	el.LogNodeStart("node_2", 2, "output_data")
	el.LogNodeEnd("node_2", 2, "final_result")
	el.LogGraphEnd("event_demo", 2)

	fmt.Println("Event Log:")
	fmt.Print(el.String())
	fmt.Println()
}

// =============================================================================
// 第二章示例
// =============================================================================

// example6_FieldMapping 演示 FieldMapping 的核心概念与用法
//
// # 问题
// 在 Eino 的图编排中，相邻节点的输入/输出类型往往不匹配：
//   - 前驱输出是一个大结构体，而后继只需要其中一个字段
//   - 前驱输出是单个值，但后继需要将值装配到嵌套字段中
//   - 多个前驱的不同字段需要汇聚到一个后继
//
// 传统的 AddEdge 只能传递整个输出值，无法做字段级裁剪与注入。
//
// # 解决方案
// FieldMapping 提供六个构造函数 + 自定义提取器，允许声明式指定
// 前驱字段到后继字段的映射关系，运行时自动完成提取、转换、注入。
//
// # Eino 设计启发
// 对应 Eino (CloudWeGo) 的 compose.FieldMapping，路径分隔符使用
// \x1F (Unit Separator)，支持嵌套 struct/map 的逐级穿透提取。
func example6_FieldMapping() {
	fmt.Println("--- Example 6: FieldMapping 字段映射 ---")
	fmt.Println()

	// =====================================================================
	// 6.1 构造函数概览
	// =====================================================================
	fmt.Println("# 6.1 FieldMapping 六个构造函数 + WithCustomExtractor")
	fmt.Println()
	fmt.Println("  MapFields(\"Query\", \"question\")         → 源字段 → 目标字段")
	fmt.Println("  FromField(\"Query\")                      → 提取字段作为后继整个输入")
	fmt.Println("  ToField(\"Result\")                       → 整个输出写入后继指定字段")
	fmt.Println("  MapFieldPaths(FieldPath{\"a\",\"b\"}, ...)  → 嵌套路径 → 嵌套路径")
	fmt.Println("  FromFieldPath(FieldPath{\"a\",\"b\"})       → 嵌套路径提取到整个输入")
	fmt.Println("  ToFieldPath(FieldPath{\"a\",\"b\"})         → 整个输出 → 嵌套路径")
	fmt.Println("  WithCustomExtractor(fn)                  → 自定义提取器")
	fmt.Println()

	// =====================================================================
	// 6.2 可运行示例: MapFields 实战
	// =====================================================================
	fmt.Println("# 6.2 可运行示例: Workflow + MapFields")
	fmt.Println("#   场景: 输入 SearchInput{Query, Lang, UserID},")
	fmt.Println("#         提取 Query 字段传给 process 节点（大写转换）,")
	fmt.Println("#         用 MapFields 将 process 输出和 Lang/UserID 汇聚到 END")
	fmt.Println()

	type SearchInput struct {
		Query  string
		Lang   string
		UserID int
	}

	// 使用 map[string]any 作为输出类型，FieldMapping 填充字段
	wf := compose.NewWorkflow[*SearchInput, map[string]any]()

	// 节点 process: MapFields("Query", "Query") 从 START 中提取 Query 字段
	// 保持同名 key，process 从 map[string]any 中读取
	wf.AddLambdaNode("process", compose.InvokableLambda(
		func(ctx context.Context, in map[string]any) (string, error) {
			q, _ := in["Query"].(string)
			return strings.ToUpper(q), nil
		},
	)).AddInput(compose.START, compose.MapFields("Query", "Query"))

	// END: MapFields 将 process 输出和 START 字段汇聚
	wf.End().AddInput("process",
		compose.MapFields("", "result"),
	)
	wf.End().AddInput(compose.START,
		compose.MapFields("Lang", "language"),
		compose.MapFields("UserID", "user_id"),
	)

	r, err := wf.Compile(context.Background())
	if err != nil {
		fmt.Printf("  Compile error: %v\n\n", err)
		return
	}

	result, err := r.Invoke(context.Background(), &SearchInput{
		Query:  "hello eino",
		Lang:   "zh-CN",
		UserID: 42,
	})
	if err != nil {
		fmt.Printf("  Invoke error: %v\n\n", err)
		return
	}

	fmt.Printf("  Input:  SearchInput{Query:\"hello eino\", Lang:\"zh-CN\", UserID:42}\n")
	fmt.Printf("  Output: %v\n", result)
	fmt.Println()
	fmt.Println("  # 说明:")
	fmt.Println("  #   - MapFields(\"Query\", \"Query\"): 提取 Query 字段传给 process 节点")
	fmt.Println("  #   - MapFields(\"\", \"result\"): process 全部输出写入 result 字段")
	fmt.Println("  #   - MapFields(\"Lang\", \"language\"): Lang 字段 → language")
	fmt.Println("  #   - MapFields(\"UserID\", \"user_id\"): UserID 字段 → user_id")
	fmt.Println("  #   FromField / ToField 语义:")
	fmt.Println("  #   - FromField(\"x\"): 等价于 MapFields(\"x\", \"\")，即提取 x 作为后继整个输入")
	fmt.Println("  #   - ToField(\"y\"):   等价于 MapFields(\"\", \"y\")，即整个输出写入 y 字段")
	fmt.Println("  #   - FromFieldPath / ToFieldPath: 支持嵌套 struct/map 路径穿透")
	fmt.Println()
}

// example7_Workflow 演示 Workflow 的声明式数据流编排
//
// # 问题
// 原始 Graph API 需要手动调用 AddEdge / AddControlEdge，当图变大时：
//   - 边的声明分散，难以一眼看出数据从哪里来、到哪里去
//   - 字段映射需要额外配置，代码冗长
//   - 控制依赖（仅执行顺序）与数据依赖（传递数据）混在一起
//
// # 解决方案
// Workflow 在 Graph 之上提供声明式 API：
//   - AddInput(fromNodeKey, mappings...): 一次声明该节点从哪些前驱取数据
//   - AddDependency(fromNodeKey): 纯执行依赖，不传递数据
//   - SetStaticValue(path, value): 编译时注入常量值
//   - End(): 终端的声明式输入
//
// # Eino 设计启发
// Workflow 对应 Eino 的 compose.Workflow[I,O]，支持三态依赖
// (normalDependency / noDirectDependency / branchDependency)，编译时
// 自动展开为底层 Graph，运行时零额外开销。
func example7_Workflow() {
	fmt.Println("--- Example 7: Workflow 声明式数据流编排 ---")
	fmt.Println()

	// 场景: 搜索 pipeline (enrich → process → END)
	//   - enrich: 从 START 接收 full input，添加前缀
	//   - process: 从 enrich 接收数据，计算统计信息
	//   - END: 从 process + START 汇聚结果
	fmt.Println("# 场景: 三节点搜索 pipeline (enrich → process → END)")
	fmt.Println("# 用 AddInput + MapFields 替代手动 AddEdge")
	fmt.Println()

	type SearchInput struct {
		Query    string
		Language string
	}

	wf := compose.NewWorkflow[*SearchInput, map[string]any]()

	// enrich: 接收整个输入，输出处理后的字符串
	wf.AddLambdaNode("enrich", compose.InvokableLambda(
		func(ctx context.Context, in *SearchInput) (string, error) {
			return "[ENRICHED] " + in.Query + " (lang=" + in.Language + ")", nil
		},
	)).AddInput(compose.START)

	// process: 从 enrich 获取数据，计算统计信息
	wf.AddLambdaNode("process", compose.InvokableLambda(
		func(ctx context.Context, enriched string) (map[string]any, error) {
			return map[string]any{
				"char_count": len(enriched),
				"has_prefix": strings.HasPrefix(enriched, "[ENRICHED]"),
			}, nil
		},
	)).AddInput("enrich")

	// END: 从 process 获取统计结果，从 START 保留原始 Query
	wf.End().AddInput("process",
		compose.MapFields("char_count", "count"),
		compose.MapFields("has_prefix", "prefixed"),
	)
	wf.End().AddInput(compose.START,
		compose.MapFields("Query", "original_query"),
	)

	r, err := wf.Compile(context.Background())
	if err != nil {
		fmt.Printf("  Compile error: %v\n\n", err)
		return
	}

	result, err := r.Invoke(context.Background(), &SearchInput{
		Query: "hello", Language: "zh-CN",
	})
	if err != nil {
		fmt.Printf("  Invoke error: %v\n\n", err)
		return
	}

	fmt.Printf("  Input:  SearchInput{Query:\"hello\", Language:\"zh-CN\"}\n")
	fmt.Printf("  Output: %v\n\n", result)
}

// example8_Chain 演示 Chain Builder 风格的线性管道
//
// # 问题
// 很多场景下，处理流程是简单的线性管道 (A → B → C)：
//   - 用 Graph 需要手动 AddEdge，重复繁琐
//   - 没有内建的 "this then that" 语义
//
// # 解决方案
// Chain[I,O] 提供 Builder 风格的 Append* 方法，自动连接节点：
//   - AppendLambda: 追加一个 Lambda 节点
//   - AppendPassthrough: 追加透传节点
//   - AppendParallel: 追加并行节点组
//   - AppendBranch: 追加条件分支
//   - AppendGraph: 追加子图
//
// 编译时自动连接 START/END，无需手动管理拓扑。
//
// # Eino 设计启发
// 对应 Eino 的 compose.Chain[I,O]，内部使用 Graph[I,O] 作为存储，
// 通过 preNodeKeys 追踪尾部节点，编译后展开为等价的底层 Graph。
func example8_Chain() {
	fmt.Println("--- Example 8: Chain Builder 线性管道 ---")
	fmt.Println()

	fmt.Println("# 场景: 文本处理管道 (lower → reverse → prefix)")
	fmt.Println()

	chain := compose.NewChain[string, string]()

	chain.
		AppendLambda(compose.InvokableLambda(
			func(ctx context.Context, in string) (string, error) {
				return strings.ToLower(in), nil
			},
		)).
		AppendLambda(compose.InvokableLambda(
			func(ctx context.Context, in string) (string, error) {
				runes := []rune(in)
				for i, j := 0, len(runes)-1; i < j; i, j = i+1, j-1 {
					runes[i], runes[j] = runes[j], runes[i]
				}
				return string(runes), nil
			},
		)).
		AppendLambda(compose.InvokableLambda(
			func(ctx context.Context, in string) (string, error) {
				return "RESULT: " + in, nil
			},
		))

	r, err := chain.Compile(context.Background())
	if err != nil {
		fmt.Printf("  Compile error: %v\n\n", err)
		return
	}

	result, err := r.Invoke(context.Background(), "Hello Chain")
	if err != nil {
		fmt.Printf("  Invoke error: %v\n\n", err)
		return
	}

	fmt.Printf("  Input:  %q\n", "Hello Chain")
	fmt.Printf("  Output: %q\n", result)
	fmt.Printf("  Expected: 字符串依次经过 lower → reverse → prefix 处理\n\n")
}

// example9_Parallel 演示 Chain 中的并行执行
//
// # 问题
// 某些场景下，需要对同一输入执行多个独立操作：
//   - 同时对文本做大写和小写转换
//   - 同时调用多个模型获取不同维度的分析
//
// Graph API 需要手动创建扇出拓扑，代码不直观。
//
// # 解决方案
// Parallel 封装并行节点组：
//   - 节点共享同一前驱输入
//   - 每个并行节点的输出用 outputKey 标注
//   - 下游节点接收 map[string]any，通过 key 区分来源
//   - 通过 Chain.AppendParallel 插入管道
//
// # Eino 设计启发
// 对应 Eino 的 compose.Parallel，支持 AddLambda / AddGraph / AddPassthrough。
// 运行时通过 goroutine 并发执行各节点，taskManager 管理并发。
func example9_Parallel() {
	fmt.Println("--- Example 9: Parallel 并行执行 ---")
	fmt.Println()

	fmt.Println("# 场景: 同时执行 upper 和 lower 两个操作，然后合并结果")
	fmt.Println()

	chain := compose.NewChain[string, string]()

	parallel := compose.NewParallel()
	parallel.
		AddLambda("upper", compose.InvokableLambda(
			func(ctx context.Context, in string) (string, error) {
				return strings.ToUpper(in), nil
			},
		)).
		AddLambda("lower", compose.InvokableLambda(
			func(ctx context.Context, in string) (string, error) {
				return strings.ToLower(in), nil
			},
		))

	chain.
		AppendPassthrough().
		AppendParallel(parallel).
		AppendLambda(compose.InvokableLambda(
			func(ctx context.Context, in map[string]any) (string, error) {
				upper := in["upper"].(string)
				lower := in["lower"].(string)
				return fmt.Sprintf("UPPER=%s | LOWER=%s", upper, lower), nil
			},
		))

	r, err := chain.Compile(context.Background())
	if err != nil {
		fmt.Printf("  Compile error: %v\n\n", err)
		return
	}

	result, err := r.Invoke(context.Background(), "Hello Parallel")
	if err != nil {
		fmt.Printf("  Invoke error: %v\n\n", err)
		return
	}

	fmt.Printf("  Input:  %q\n", "Hello Parallel")
	fmt.Printf("  Output: %q\n", result)
	fmt.Printf("  Expected: 合并 upper 和 lower 的结果\n\n")
}

// example10_Branch 演示 Chain 中的条件分支
//
// # 问题
// 实际应用中，经常需要根据输入内容选择不同的处理路径：
//   - 长文本走摘要路径，短文本走直接处理路径
//   - 不同语言走不同的翻译模型
//   - 根据用户等级选择不同的推荐策略
//
// 普通图只能静态连接所有节点，无法条件性跳过。
//
// # 解决方案
// ChainBranch 封装条件分支：
//   - 单路径分支 (NewChainBranch): 条件函数返回单个 key，选择一个分支
//   - 多路径分支 (NewChainMultiBranch): 条件函数返回 key 集合，多分支同时激活
//   - 每个分支节点通过 AddLambda/AddGraph/AddPassthrough 注册
//   - 通过 Chain.AppendBranch 插入管道
//
// # Eino 设计启发
// 对应 Eino 的 compose.ChainBranch，内部通过 GraphBranch 的
// invoke/collect 双函数实现，支持条件路由到白名单 endNodes。
func example10_Branch() {
	fmt.Println("--- Example 10: Branch 条件分支 ---")
	fmt.Println()

	fmt.Println("# 场景: 根据文本长度选择处理路径")
	fmt.Println("#   短文本 (len <= 5): 加前缀 SHORT")
	fmt.Println("#   长文本 (len > 5):  加前缀 LONG")
	fmt.Println()

	branchCond := func(ctx context.Context, in string) (string, error) {
		if len(in) > 5 {
			return "long", nil
		}
		return "short", nil
	}

	chain := compose.NewChain[string, string]()

	chain.
		AppendBranch(compose.NewChainBranch(branchCond).
			AddLambda("long", compose.InvokableLambda(
				func(ctx context.Context, in string) (string, error) {
					return "LONG:" + in, nil
				},
			)).
			AddLambda("short", compose.InvokableLambda(
				func(ctx context.Context, in string) (string, error) {
					return "SHORT:" + in, nil
				},
			)),
		).
		AppendPassthrough()

	r, err := chain.Compile(context.Background())
	if err != nil {
		fmt.Printf("  Compile error: %v\n\n", err)
		return
	}

	result1, _ := r.Invoke(context.Background(), "hello-branch")
	fmt.Printf("  Input:  %q → Output: %q (expect: LONG prefix)\n", "hello-branch", result1)

	result2, _ := r.Invoke(context.Background(), "hi")
	fmt.Printf("  Input:  %q        → Output: %q (expect: SHORT prefix)\n", "hi", result2)
	fmt.Println()
}

// example11_SkippedFeatures 说明本复刻版相对于 Eino 明确跳过的能力
//
// # 跳过的原因
// 本复刻版聚焦于 Eino Compose Runtime 的核心图编译与执行引擎，
// 以下能力因依赖外部组件体系或复杂度原因暂不实现。
func example11_SkippedFeatures() {
	fmt.Println("--- Example 11: 本复刻版明确跳过的 Eino 能力 ---")
	fmt.Println()

	fmt.Println("┌─────────────────────────────────────────────────────────────────────┐")
	fmt.Println("│ 项目                               │ 跳过理由                      │")
	fmt.Println("├─────────────────────────────────────────────────────────────────────┤")
	fmt.Println("│ Provider 级组件桥接选项              │ 依赖真实模型/工具 SDK        │")
	fmt.Println("│ AddEmbeddingNode / AppendEmbedding   │ 尚未实现 Embedding 领域接口  │")
	fmt.Println("│ 图级 Stream 执行管线                  │ runner 仍以 Invoke 为主路径  │")
	fmt.Println("│ streamFieldMap 流式字段映射          │ 依赖图级 stream channel      │")
	fmt.Println("│ Stream ChainBranch                   │ 流式分支未接入 Chain Builder │")
	fmt.Println("│ 组件级 Callback 深度集成              │ 未接 Provider 级事件模型      │")
	fmt.Println("│ State 传递 (graph.state)             │ 字段定义但未使用             │")
	fmt.Println("│ 持久化/分布式 Checkpoint Store       │ 当前仅内存教学实现           │")
	fmt.Println("│ values_merge 的 StreamReader merge    │ 未接图级 stream fan-in      │")
	fmt.Println("│ 编译时类型推断 (toValidateMap)       │ 推迟到后续版本               │")
	fmt.Println("│ Graph 可视化 / DOT 导出              │ 周边工具未实现               │")
	fmt.Println("│ JSON Schema 编译校验                 │ 类型系统限制                  │")
	fmt.Println("│ Tracing / Metrics / Profiling        │ DevOps 工具不在范围内         │")
	fmt.Println("│ 外部依赖集成 (Eino 官方库)           │ 纯 Go 标准库                 │")
	fmt.Println("│ Fan-in 智能合并 (Merge 配置)         │ 当前默认 map[string]any 合并  │")
	fmt.Println("└─────────────────────────────────────────────────────────────────────┘")
	fmt.Println()
	fmt.Println("  可替代方案:")
	fmt.Println("  - ChatModel/Retriever/Prompt/Tool: 已通过 Bridge Adapter 包装为 Lambda")
	fmt.Println("  - 流式处理: Runnable 四模式、Pipe stream、Collect/Transform 教学路径已实现")
	fmt.Println("  - 回调处理: CallbackWrapper 已覆盖 OnStart/OnEnd/OnError 与流输入/输出回调副本")
	fmt.Println("  - 类型推断: 当前通过 fmtType() 返回简单类型名，复杂类型标注为 \"any\"")
	fmt.Println()
}

// =============================================================================
// 第三章示例: Runnable Stream / Collect / Transform / Callback 教学示例
//
// 注意: 本章实现 Runnable 四模式、基础 Pipe stream 与 CallbackWrapper。
// 组件桥接、图级流式执行、stream field mapping 和流式分支不在当前范围内。
// =============================================================================

// StreamReader 模拟 Eino 的 StreamReader 抽象,用于教学演示
type StreamReader[T any] struct {
	ch  chan T
	err chan error
}

func NewStreamReader[T any](capacity int) *StreamReader[T] {
	return &StreamReader[T]{
		ch:  make(chan T, capacity),
		err: make(chan error, 1),
	}
}

func (sr *StreamReader[T]) Send(v T) {
	sr.ch <- v
}

func (sr *StreamReader[T]) SendError(e error) {
	sr.err <- e
}

func (sr *StreamReader[T]) Close() {
	close(sr.ch)
}

func (sr *StreamReader[T]) Recv() (T, error) {
	select {
	case v, ok := <-sr.ch:
		if !ok {
			var zero T
			return zero, nil
		}
		return v, nil
	case e := <-sr.err:
		var zero T
		return zero, e
	}
}

// =============================================================================
// Stream 示例: 展示 composableRunnable 的 stream 回退机制
// =============================================================================

// example12_RunnableStream 演示 Runnable 的 Stream 概念
//
// # 问题
// 在 Eino 中,某些组件(如 ChatModel)支持流式输出,允许逐个 token 返回结果。
// Runnable 接口通过 composableRunnable 的 s (stream) 字段预留了此能力。
//
// # 当前状态
// composableRunnable 已支持 Invoke/Stream/Collect/Transform 四种模式,
// 并按 Eino 的降级矩阵在缺少原生模式时自动 fallback。
// 本示例展示 invoke-only Lambda 的 Stream fallback。
//
// # 教学目的
// 理解 Stream 回退机制: 当 Lambda 未设置 s 时,Stream 自动 fallback 到 Invoke。
// 这是 Eino 中 "invoke 是 stream 的子集" 设计原则的体现。
func example12_RunnableStream() {
	fmt.Println("--- Example 12: Runnable Stream 概念演示 ---")
	fmt.Println()

	fmt.Println("# 12.1 composableRunnable 四字段设计")
	fmt.Println("#")
	fmt.Println("#   composableRunnable {")
	fmt.Println("#     i: func(ctx,input) invoke 执行体")
	fmt.Println("#     s: func(ctx,input) stream 执行体")
	fmt.Println("#     c: func(ctx,stream) collect 执行体")
	fmt.Println("#     t: func(ctx,stream) transform 执行体")
	fmt.Println("#   }")
	fmt.Println("#")
	fmt.Println("#   调用路径:")
	fmt.Println("#     invoke()    → i → s → c → t")
	fmt.Println("#     stream()    → s → t → i → c")
	fmt.Println("#     collect()   → c → t → i → s")
	fmt.Println("#     transform() → t → s → c → i")
	fmt.Println()

	fmt.Println("# 12.2 InvokableLambda 仅设置 i 字段 (s == nil)")
	fmt.Println("# Stream 回退机制: 当 s==nil 时,stream() 自动 fallback 到 invoke()")
	fmt.Println()

	lambda := compose.InvokableLambda(func(ctx context.Context, in string) (string, error) {
		return "[PROCESSED] " + in, nil
	})

	g := compose.NewGraph[string, string]()
	g.AddLambdaNode("runnable_demo", lambda)
	g.AddEdge(compose.START, "runnable_demo")
	g.AddEdge("runnable_demo", compose.END)

	r, err := g.Compile(context.Background(),
		compose.WithGraphName("stream_demo"),
		compose.WithNodeTriggerMode(compose.AllPredecessor),
	)
	if err != nil {
		fmt.Printf("  Compile error: %v\n", err)
		return
	}

	result, err := r.Invoke(context.Background(), "hello")
	if err != nil {
		fmt.Printf("  Invoke error: %v\n", err)
		return
	}
	fmt.Printf("  Input:  %q\n", "hello")
	fmt.Printf("  Output: %q\n", result)
	fmt.Println()
	fmt.Println("#   源码追踪 (compose/runnable.go):")
	fmt.Println("#     func (cr *composableRunnable) stream(ctx, input) {")
	fmt.Println("#       if cr.s != nil { return cr.s(ctx, input) }  // 优先 stream")
	fmt.Println("#       if cr.t != nil { return cr.t(ctx, streamFromItems(input)) }")
	fmt.Println("#       if cr.i != nil { return streamFromItems(cr.i(ctx,input)) }")
	fmt.Println("#       if cr.c != nil { return streamFromItems(cr.c(ctx, streamFromItems(input))) }")
	fmt.Println("#     }")
	fmt.Println()
}

// example13_StreamCollect 演示 Stream Collect 模式
//
// # 问题
// Eino 中的 StreamReader 支持分块接收数据,Collect 将所有分块收集为完整结果。
// 在完整产品中,Collect 需要处理 StreamReader 的合并策略(如 append/concat)。
//
// # 当前实现
// 本示例使用教学版 StreamReader[t] 演示 Collect 概念:
//   - 模拟一个生成 5 个 token 的流式 Lambda
//   - 通过 Collect 将所有 token 收集为完整字符串
//
// # 教学目的
// 理解 Eino 中 Stream 的生产-收集模式。当前教育子集已实现 Runnable Collect
// 降级路径和基础 Pipe stream,但完整图级流式执行不在本章范围内。
func example13_StreamCollect() {
	fmt.Println("--- Example 13: Stream Collect 模式 ---")
	fmt.Println()

	fmt.Println("# 13.1 模拟流式 Lambda (输出 5 个 token)")
	fmt.Println()

	sr := NewStreamReader[string](5)

	go func() {
		defer sr.Close()
		for _, token := range []string{"He", "llo", " ", "Wor", "ld"} {
			sr.Send(token)
		}
	}()

	fmt.Println("# 13.2 Collect: 收集所有分块")
	var collected string
	for {
		token, err := sr.Recv()
		if err != nil {
			fmt.Printf("  Collect error: %v\n", err)
			return
		}
		if token == "" {
			break
		}
		fmt.Printf("  Recv token: %q\n", token)
		collected += token
	}
	fmt.Printf("  Collected: %q\n\n", collected)

	fmt.Println("# 13.3 说明")
	fmt.Println("#   - StreamReader.Recv() 每次读取一个分块")
	fmt.Println("#   - Collect 将所有分块按序拼接 (相当于 Merge.Strategy=Concat)")
	fmt.Println("#   - Eino 完整版支持 StreamReader merge 策略 (append/concat/mergeMap)")
	fmt.Println("#   - 本教育子集实现了基础 Pipe stream 与 Runnable Collect 降级")
	fmt.Println("#   - 完整图流式执行、stream field mapping 和流式分支不在范围内")
	fmt.Println()
}

// example14_StreamTransform 演示 Stream Transform 模式
//
// # 问题
// Eino 的 Transform 允许在流式处理中对每个分块应用变换函数,常用的有:
//   - 逐 chunk 转换 (如 lowercasing, masking)
//   - 带状态的流式变换 (如计数器, 滑动窗口)
//   - 缓冲批量变换 (如 chunk-batching)
//
// # 当前实现
// 本示例使用教学版 StreamReader[t] 演示 Transform:
//   - 生成 5 个 token
//   - 通过 Transform 对上每个 token 转为大写
//   - 最后通过 Collect 收集
//
// # 教学目的
// 理解 Stream 处理的管道模式: 生产 → Transform → Collect。
// 这是 Eino 中 Compose Framework 流式分支的基础概念。
func example14_StreamTransform() {
	fmt.Println("--- Example 14: Stream Transform 模式 ---")
	fmt.Println()

	fmt.Println("# 14.1 流式管道: 生产 → Transform(ToUpper) → Collect")
	fmt.Println()

	sr := NewStreamReader[string](5)

	go func() {
		defer sr.Close()
		for _, token := range []string{"he", "ll", "o ", "wo", "rld"} {
			sr.Send(token)
		}
	}()

	toUpper := func(s string) string { return strings.ToUpper(s) }

	fmt.Println("  Pipeline: StreamReader → Transform → Collect")
	var result string
	for {
		token, err := sr.Recv()
		if err != nil {
			fmt.Printf("  Error: %v\n", err)
			return
		}
		if token == "" {
			break
		}
		transformed := toUpper(token)
		fmt.Printf("  %q → Transform → %q\n", token, transformed)
		result += transformed
	}
	fmt.Printf("  Final: %q\n\n", result)

	fmt.Println("# 14.2 Transform 模式变体说明")
	fmt.Println("#   逐 chunk 变换: StreamReader[T] → Transform(fn) → StreamReader[U]")
	fmt.Println("#   带状态变换:  fn 中维护计数器/滑动窗口/状态机")
	fmt.Println("#   批量变换:    收集 N 个 chunk 后一次处理")
	fmt.Println("#   Eino 中这些由 compose.Transform 实现,当前为教学演示")
	fmt.Println()
}

// example15_CallbackTiming 演示 Callback 计时模式
//
// # 问题
// Eino 提供 OnStart / OnEnd / OnError 三类回调,用于追踪、日志、计时、熔断。
// 这是可观测性的核心机制,在完整产品中贯穿所有 Runnable 执行。
//
// # 当前实现
// 本示例使用教学版 callbackTimedRunnable 演示回调计时:
//   - OnStart: 记录开始时间
//   - OnEnd:   记录结束时间,计算耗时
//   - OnError: 记录错误
//
// 同时演示如何使用 Graph + Lambda 实现等价功能。
//
// # 教学目的
// 理解 Eino 的回调生命周期 (Start → Execute → End/Error),
// 以及计时 trace 如何作为回调实现。当前教育子集提供 CallbackWrapper,
// 但尚未接入组件桥接和完整图级 callback 初始化链。
func example15_CallbackTiming() {
	fmt.Println("--- Example 15: Callback 计时模式 ---")
	fmt.Println()

	fmt.Println("# 15.1 回调生命周期: OnStart → Invoke → OnEnd/OnError")
	fmt.Println()

	type timingCallback struct {
		onStart func(nodeKey string, input string) context.Context
		onEnd   func(nodeKey string, output string, elapsed time.Duration)
		onError func(nodeKey string, err error, elapsed time.Duration)
	}

	runWithCallback := func(
		nodeKey string,
		input string,
		fn func(context.Context, string) (string, error),
		cb timingCallback,
	) (string, error) {
		ctx := cb.onStart(nodeKey, input)
		start := time.Now()

		output, err := fn(ctx, input)
		elapsed := time.Since(start)

		if err != nil {
			cb.onError(nodeKey, err, elapsed)
			return "", err
		}
		cb.onEnd(nodeKey, output, elapsed)
		return output, nil
	}

	cb := timingCallback{
		onStart: func(nodeKey, input string) context.Context {
			fmt.Printf("  [OnStart]  node=%q  input=%q\n", nodeKey, input)
			return context.Background()
		},
		onEnd: func(nodeKey, output string, elapsed time.Duration) {
			fmt.Printf("  [OnEnd]    node=%q  output=%q  elapsed=%v\n", nodeKey, output, elapsed)
		},
		onError: func(nodeKey string, err error, elapsed time.Duration) {
			fmt.Printf("  [OnError]  node=%q  err=%v  elapsed=%v\n", nodeKey, err, elapsed)
		},
	}

	successFn := func(ctx context.Context, s string) (string, error) {
		time.Sleep(50 * time.Millisecond)
		return "[RESULT] " + s, nil
	}

	output, _ := runWithCallback("demo_node", "test_input", successFn, cb)
	_ = output
	fmt.Println()

	fmt.Println("# 15.2 使用 Graph + EventLog 实现等价的可观测性")
	fmt.Println()

	el := compose.NewEventLog()
	el.LogGraphStart("callback_demo")
	el.LogNodeStart("step_1", 1, "raw_data")
	el.LogNodeEnd("step_1", 1, "processed_data")
	el.LogGraphEnd("callback_demo", 1)
	fmt.Println("  EventLog (等效于回调日志):")
	for _, line := range strings.Split(strings.TrimSpace(el.String()), "\n") {
		fmt.Printf("  %s\n", line)
	}
	fmt.Println()

	fmt.Println("# 15.3 说明")
	fmt.Println("#   - OnStart/OnEnd/OnError 三类回调覆盖完整生命周期")
	fmt.Println("#   - 回调可用于计时 trace、日志、熔断、重试")
	fmt.Println("#   - EventLog 在 graph 级别提供了类似的可观测性")
	fmt.Println("#   - CallbackWrapper 已覆盖 Invoke/Stream/Collect/Transform 包装")
	fmt.Println("#   - Provider 级 callback 初始化链和完整图流式执行不在范围内")
	fmt.Println()
}

// =============================================================================
// I3 Bridge Adapter 示例: 领域组件参与通用图运行时
//
// 核心问题:
// Graph/Workflow/Chain 运行时的基本单位是 Lambda (composableRunnable),
// 但领域组件 (Retriever, ChatModel) 有其自身的接口约定。
//
// 解决方案:
// Bridge Adapter 为每种领域组件定义轻量接口 + 适配函数 (toLambda),
// 将领域语义包装为 Lambda,既不侵入组件自身,也不侵入图运行时。
// 组件开发者按领域接口实现,通过 Bridge 即可参加图编排。
// =============================================================================

// mockRetriever is a canned retriever returning hardcoded documents.
type mockRetriever struct{}

func (r *mockRetriever) Retrieve(ctx context.Context, query string) ([]*compose.BridgeDocument, error) {
	return []*compose.BridgeDocument{
		{Content: "Rive is a local-first agent team runtime with Snapshot/WorkDAG/Dispatch systems.", Score: 0.95},
		{Content: "Eino (CloudWeGo) features compose.Graph with DAG/Pregel dual-mode execution.", Score: 0.82},
		{Content: "FieldMapping enables field-level data extraction between graph nodes.", Score: 0.71},
	}, nil
}

// mockChatModel is a canned chat model returning a hardcoded response.
type mockChatModel struct{}

func (m *mockChatModel) Generate(ctx context.Context, messages []*compose.BridgeMessage) (string, error) {
	time.Sleep(10 * time.Millisecond)
	return "Rive is a local-first agent team runtime that uses Snapshot evidence, Work DAG scheduling, and Dispatch binding to coordinate multi-agent software engineering tasks. It supports worktree-isolated workspaces and ledger-based progress tracking.", nil
}

func example16_RAGPipeline() {
	fmt.Println("--- Example 16: RAG Pipeline (Retriever → Prompt Assembly → ChatModel) ---")
	fmt.Println()

	fmt.Println("# 场景: 用户提问 → 检索相关文档 → 组装 prompt → 模型生成回答")
	fmt.Println("#")
	fmt.Println("#   拓扑:")
	fmt.Println("#     START ──┬──> retriever ──┬──> assemble ──> model ──> END")
	fmt.Println("#             │                │")
	fmt.Println("#             └────────────────┘")
	fmt.Println("#                (query 直传 + documents fan-in = FieldMapping)")
	fmt.Println("#")
	fmt.Println("#   桥接适配:")
	fmt.Println("#     mockRetriever 实现 compose.Retriever 接口")
	fmt.Println("#       → AsRetrieverNode() 将其桥接为 Workflow Lambda 节点")
	fmt.Println("#     mockChatModel 实现 compose.ChatModel 接口")
	fmt.Println("#       → AsChatModelNode() 将其桥接为 Workflow Lambda 节点")
	fmt.Println("#     promptAssembler 经 AsPromptAssemblerNode() 桥接")
	fmt.Println("#")
	fmt.Println()

	sysPrompt := "You are a technical assistant. Answer using only the provided context. Be concise and accurate."

	// Workflow[string, map[string]any]: input is a query string, output is a result map.
	wf := compose.NewWorkflow[string, map[string]any]()

	// retriever: direct data flow from START (query string)
	wf.AsRetrieverNode("retriever", &mockRetriever{}).
		AddInput(compose.START)

	// assemble: FieldMapping fan-in — query from START + documents from retriever
	wf.AsPromptAssemblerNode("assemble", sysPrompt).
		AddInput(compose.START, compose.MapFields("", "query")).
		AddInput("retriever", compose.ToField("documents"))

	// model: direct data flow from assemble
	wf.AsChatModelNode("model", &mockChatModel{}).
		AddInput("assemble")

	// END: FieldMapping outputs
	wf.End().
		AddInput("model", compose.ToField("answer")).
		AddInput(compose.START, compose.MapFields("", "original_query"))

	r, err := wf.Compile(context.Background())
	if err != nil {
		fmt.Printf("  Compile error: %v\n\n", err)
		return
	}

	result, err := r.Invoke(context.Background(), "What is Rive?")
	if err != nil {
		fmt.Printf("  Invoke error: %v\n\n", err)
		return
	}

	// FieldMapping fan-in nests values under source node keys.
	modelResult := result["model"].(map[string]any)
	startResult := result["start"].(map[string]any)

	fmt.Printf("  Query:  %q\n", "What is Rive?")
	fmt.Printf("  Answer: %q\n", modelResult["answer"])
	fmt.Printf("  Retained: original_query=%q\n", startResult["original_query"])
	fmt.Println()
	fmt.Println("#   关键点:")
	fmt.Println("#     - retriever: AddInput(START) 直连,接收 query string → 输出 []*Document")
	fmt.Println("#     - assemble: AddInput(START, MapFields) + AddInput(retriever, ToField)")
	fmt.Println("#       → FieldMapping 在两个数据源之间做字段级聚合")
	fmt.Println("#     - model: AddInput(assemble) 直连,接收 []*Message → 输出 string")
	fmt.Println("#     - 领域组件 (mockRetriever/mockChatModel) 仅实现领域接口")
	fmt.Println("#     - Bridge 适配器 (As*Node) 把它们包装成 Lambda 参与图编排")
	fmt.Println()
}

func example17_BridgePatternExplanation() {
	fmt.Println("--- Example 17: Bridge Adapter 模式说明 ---")
	fmt.Println()
	fmt.Println("┌─────────────────────────────────────────────────────────────────────────────┐")
	fmt.Println("│                      Bridge Adapter 模式架构                               │")
	fmt.Println("├─────────────────────────────────────────────────────────────────────────────┤")
	fmt.Println("│                                                                             │")
	fmt.Println("│  领域层 (Domain)             桥接层 (Bridge)          运行时 (Runtime)      │")
	fmt.Println("│  ┌──────────────┐          ┌──────────────┐          ┌──────────────────┐  │")
	fmt.Println("│  │ Retriever    │──bridge──│ toLambda()   │──Lambda──│ Graph[I,O]       │  │")
	fmt.Println("│  │ .Retrieve()  │          │              │          │  .AddLambdaNode  │  │")
	fmt.Println("│  └──────────────┘          └──────────────┘          │  .AddEdge        │  │")
	fmt.Println("│                                                       │  .Compile()      │  │")
	fmt.Println("│  ┌──────────────┐          ┌──────────────┐          │  .Invoke()       │  │")
	fmt.Println("│  │ ChatModel    │──bridge──│ toLambda()   │──Lambda──│                  │  │")
	fmt.Println("│  │ .Generate()  │          │              │          │  Workflow[I,O]   │  │")
	fmt.Println("│  └──────────────┘          └──────────────┘          │  .AsRetrieverNode│  │")
	fmt.Println("│                                                       │  .AsChatModelNode│  │")
	fmt.Println("│  ┌──────────────┐          ┌──────────────┐          │  .AddInput()     │  │")
	fmt.Println("│  │ Tool         │──bridge──│ toLambda()   │──Lambda──│                  │  │")
	fmt.Println("│  │ .Execute()   │          │              │          │  Chain[I,O]      │  │")
	fmt.Println("│  └──────────────┘          └──────────────┘          │  .AppendLambda   │  │")
	fmt.Println("│                                                       └──────────────────┘  │")
	fmt.Println("│                                                                             │")
	fmt.Println("└─────────────────────────────────────────────────────────────────────────────┘")
	fmt.Println()
	fmt.Println("# 为什么 Bridge Adapter 让领域组件能参与通用图运行时?")
	fmt.Println()
	fmt.Println("  1. 统一合约 (Lambda):")
	fmt.Println("     Graph/Workflow/Chain 只认 Lambda (composableRunnable) 作为可执行单元。")
	fmt.Println("     Bridge 将任何一个实现领域接口的结构体包装成 Lambda,无须修改图运行时。")
	fmt.Println()
	fmt.Println("  2. 接口隔离 (Domain Interface):")
	fmt.Println("     领域组件定义自己的接口 (Retriever.Retrieve, ChatModel.Generate)。")
	fmt.Println("     实现者只需关心领域逻辑,不依赖 graph/compose 包的类型系统。")
	fmt.Println()
	fmt.Println("  3. 零侵入 (Non-intrusive):")
	fmt.Println("     bridge 函数是纯适配逻辑,不修改组件自身,不污染图运行时。")
	fmt.Println("     新增领域组件类型只需添加一个 bridge + 接口,编译时正交。")
	fmt.Println()
	fmt.Println("  4. FieldMapping 衔接 (Composition over Coupling):")
	fmt.Println("     不同组件输入输出类型不同 (string → []*Document → []*Message → string)。")
	fmt.Println("     FieldMapping 在 bridge 节点之间做字段提取、转换、注入,避免硬编码耦合。")
	fmt.Println()
	fmt.Println("  5. 三重抽象复用 (Graph / Workflow / Chain):")
	fmt.Println("     同一套 Bridge Lambda 可用于三种编排抽象:")
	fmt.Println("     - Graph: 最大灵活性,手动 AddEdge + AddLambdaNode")
	fmt.Println("     - Workflow: 声明式 AddInput + FieldMapping + As*Node 便捷方法")
	fmt.Println("     - Chain: Builder 风格 AppendLambda")
	fmt.Println()
	fmt.Println("# 扩展清单 (本教育子集未实现):")
	fmt.Println("  - StreamChatModel bridge: ChatModel.GenerateStream() → StreamableLambda")
	fmt.Println("  - Embedding bridge: Embedder.Embed() → Lambda")
	fmt.Println("  - Provider-specific tool binding options")
	fmt.Println("  - 完整的错误传递与重试语义 (callback + state 集成)")
	fmt.Println()
}

// =============================================================================
// I3 Prompt/Tool Bridge 示例: PromptTemplate → ToolCall → ToolsNode → Response
//
// 核心概念:
// 本示例演示 Tool Calling Pipeline — 让 LLM 应用能够调用外部工具:
//
//   BridgeTool          — 领域工具接口 (Name + Execute)
//   promptTemplateBridge— MessageTemplate → Lambda 适配器
//   toolsNodeBridge     — 解析 ToolCalls, 执行工具, 返回结果
//
// 工作流:
//   PromptTemplate → FakeChatModel (返回 ToolCall) → ToolsNode → FinalModel
//
// 特点:
//   - 完全确定性, 不调用任何外部模型/API
//   - 使用 compose.FakeChatModel 模拟返回 ToolCall 的模型行为
//   - 使用 compose.BridgeTool 模拟工具 (get_weather / calculator)
//   - 通过 Workflow/Graph/Chain 三种编排方式演示
// =============================================================================

// mockToolCallModel returns a canned ToolCall for get_weather.
func mockToolCallModel() *compose.FakeChatModel {
	return compose.NewFakeChatModel(compose.WithChatGenerateFunc(
		func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
			return &compose.Message{
				Role:    compose.Assistant,
				Content: "",
				ToolCalls: []compose.ToolCall{
					{
						ID:   "call_weather_001",
						Type: "function",
						Function: compose.ToolCallFunction{
							Name:      "get_weather",
							Arguments: `{"location":"Paris"}`,
						},
					},
				},
			}, nil
		},
	))
}

// mockFinalModel assembles a final answer from the tool result.
func mockFinalModel() *compose.FakeChatModel {
	return compose.NewFakeChatModel(compose.WithChatGenerateFunc(
		func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
			if len(input) == 0 {
				return compose.AssistantMessage("no input"), nil
			}
			last := input[len(input)-1]
			return &compose.Message{
				Role:    compose.Assistant,
				Content: fmt.Sprintf("Final answer based on tool results:\n%s", last.Content),
			}, nil
		},
	))
}

func example19_ToolCallingPipeline() {
	fmt.Println("--- Example 19: Tool Calling Pipeline (Workflow) ---")
	fmt.Println()

	fmt.Println("# 场景: 用户提问 → PromptTemplate → 模型返回 ToolCall → ToolsNode 执行 → 最终回答")
	fmt.Println("#")
	fmt.Println("#   拓扑 (Workflow):")
	fmt.Println("#     START ──> prompt ──> model1 ──> tools ──> model2 ──> END")
	fmt.Println("#")
	fmt.Println()

	// 1. Define the prompt template
	tmpl := compose.NewMessageTemplate("{{query}}").
		WithSystemTemplate("You are a helpful assistant. Answer concisely.")

	// 2. Create tools
	getWeather := compose.NewBridgeTool("get_weather",
		func(ctx context.Context, args map[string]any) (string, error) {
			loc, _ := args["location"].(string)
			return fmt.Sprintf("Sunny, 22°C in %s with light breeze", loc), nil
		},
	)

	// 3. Build the workflow
	wf := compose.NewWorkflow[map[string]any, *compose.Message]()

	wf.AsPromptTemplateNode("prompt", tmpl).
		AddInput(compose.START)

	wf.AddLambdaNode("model1", compose.InvokableLambda(
		func(ctx context.Context, msgs []*compose.Message) (*compose.Message, error) {
			return mockToolCallModel().Generate(ctx, msgs)
		},
	)).AddInput("prompt")

	wf.AsToolsNode("tools", getWeather).
		AddInput("model1")

	wf.AddLambdaNode("model2", compose.InvokableLambda(
		func(ctx context.Context, msg *compose.Message) (*compose.Message, error) {
			return mockFinalModel().Generate(ctx, []*compose.Message{msg})
		},
	)).AddInput("tools")

	wf.End().AddInput("model2")

	r, err := wf.Compile(context.Background())
	if err != nil {
		fmt.Printf("  Compile error: %v\n\n", err)
		return
	}

	result, err := r.Invoke(context.Background(), map[string]any{
		"query": "What is the weather in Paris?",
	})
	if err != nil {
		fmt.Printf("  Invoke error: %v\n\n", err)
		return
	}

	fmt.Printf("  Input:  %q\n", "What is the weather in Paris?")
	fmt.Printf("  Output: %s\n", result.Content)
	fmt.Println()
	fmt.Println("# 关键点:")
	fmt.Println("#   - PromptTemplate: MessageTemplate → Lambda, 输出 []*Message")
	fmt.Println("#   - model1: FakeChatModel 返回包含 ToolCall 的 Message (ToolCalls 字段)")
	fmt.Println("#   - tools: ToolsNode 解析 ToolCalls, 匹配 BridgeTool, 执行并组装结果")
	fmt.Println("#   - model2: 第二个 FakeChatModel 基于工具结果生成最终回答")
	fmt.Println("#   - 全程确定性, 无外部调用")
	fmt.Println()
}

func example20_ToolCallingPipelineChain() {
	fmt.Println("--- Example 20: Tool Calling Pipeline (Chain / Graph) ---")
	fmt.Println()

	fmt.Println("# Chain 版本: 线性管道, 自动连接 START/END")
	fmt.Println()

	// Create the tool
	calcTool := compose.NewBridgeTool("calculator",
		func(ctx context.Context, args map[string]any) (string, error) {
			expr, _ := args["expression"].(string)
			return fmt.Sprintf("Computed result for '%s' = 42", expr), nil
		},
	)

	// model1: returns a ToolCall for calculator
	model1 := compose.InvokableLambda(
		func(ctx context.Context, msgs []*compose.Message) (*compose.Message, error) {
			return compose.NewFakeChatModel(compose.WithChatGenerateFunc(
				func(ctx context.Context, input []*compose.Message) (*compose.Message, error) {
					return &compose.Message{
						Role:    compose.Assistant,
						Content: "",
						ToolCalls: []compose.ToolCall{
							{
								ID:   "call_calc_001",
								Type: "function",
								Function: compose.ToolCallFunction{
									Name:      "calculator",
									Arguments: `{"expression":"2+2"}`,
								},
							},
						},
					}, nil
				},
			)).Generate(ctx, msgs)
		},
	)

	// tools: execute the calculator tool using exported NewToolsNodeLambda
	tools := compose.NewToolsNodeLambda(calcTool)

	// model2: assemble final response
	model2 := compose.InvokableLambda(
		func(ctx context.Context, msg *compose.Message) (*compose.Message, error) {
			return compose.AssistantMessage(
				fmt.Sprintf("Answer: The tool computed → %s", msg.Content),
			), nil
		},
	)

	chain := compose.NewChain[[]*compose.Message, *compose.Message]()
	chain.AppendLambda(model1).AppendLambda(tools).AppendLambda(model2)

	r, err := chain.Compile(context.Background())
	if err != nil {
		fmt.Printf("  Compile error: %v\n\n", err)
		return
	}

	input := []*compose.Message{compose.HumanMessage("What is 2+2?")}
	result, err := r.Invoke(context.Background(), input)
	if err != nil {
		fmt.Printf("  Invoke error: %v\n\n", err)
		return
	}

	fmt.Printf("  Input:  %q\n", "What is 2+2?")
	fmt.Printf("  Output: %s\n", result.Content)
	fmt.Println()

	fmt.Println("# Graph 版本: 手动拓扑, 最大灵活性")
	fmt.Println()

	// Demonstrate the same pipeline using raw Graph
	g := compose.NewGraph[[]*compose.Message, *compose.Message]()

	model1Fn := compose.InvokableLambda(
		func(ctx context.Context, msgs []*compose.Message) (*compose.Message, error) {
			jsonArgs, _ := json.Marshal(map[string]string{"location": "Tokyo"})
			return &compose.Message{
				Role:    compose.Assistant,
				Content: "",
				ToolCalls: []compose.ToolCall{
					{
						ID:   "call_weather_002",
						Type: "function",
						Function: compose.ToolCallFunction{
							Name:      "get_weather",
							Arguments: string(jsonArgs),
						},
					},
				},
			}, nil
		},
	)

	weatherTool := compose.NewBridgeTool("get_weather",
		func(ctx context.Context, args map[string]any) (string, error) {
			loc, _ := args["location"].(string)
			return fmt.Sprintf("Cloudy, 18°C in %s", loc), nil
		},
	)

	toolsFn := compose.NewToolsNodeLambda(weatherTool)

	model2Fn := compose.InvokableLambda(
		func(ctx context.Context, msg *compose.Message) (*compose.Message, error) {
			return compose.AssistantMessage(
				fmt.Sprintf("Summary: %s", msg.Content),
			), nil
		},
	)

	g.AddLambdaNode("model1", model1Fn)
	g.AddLambdaNode("tools", toolsFn)
	g.AddLambdaNode("model2", model2Fn)
	g.AddEdge(compose.START, "model1")
	g.AddEdge("model1", "tools")
	g.AddEdge("tools", "model2")
	g.AddEdge("model2", compose.END)

	gr, err := g.Compile(context.Background(),
		compose.WithGraphName("tool_calling_graph"),
		compose.WithNodeTriggerMode(compose.AllPredecessor),
	)
	if err != nil {
		fmt.Printf("  Compile error: %v\n\n", err)
		return
	}

	graphResult, err := gr.Invoke(context.Background(), []*compose.Message{
		compose.HumanMessage("What is the weather in Tokyo?"),
	})
	if err != nil {
		fmt.Printf("  Invoke error: %v\n\n", err)
		return
	}

	fmt.Printf("  Input:  %q\n", "What is the weather in Tokyo?")
	fmt.Printf("  Output: %s\n", graphResult.Content)
	fmt.Println()
	fmt.Println("# 关键点:")
	fmt.Println("#   - Chain: AppendLambda 自动串联, 简洁直观")
	fmt.Println("#   - Graph: AddEdge 手动拓扑, 支持复杂 DAG")
	fmt.Println("#   - NewToolsNodeLambda 是导出的构造函数,BridgeTool.Execute 保持领域接口")
	fmt.Println()
}

type checkpointApprovalState struct {
	Original string
}

func example18_CheckpointInterruptResume() {
	fmt.Println("--- Example 18: Checkpoint / Interrupt / Resume ---")
	fmt.Println()
	store := compose.NewInMemoryCheckPointStore()

	g := compose.NewGraph[string, string]()
	g.AddLambdaNode("approval", compose.InvokableLambda(func(ctx context.Context, input string) (string, error) {
		wasInterrupted, _, state := compose.GetInterruptState[checkpointApprovalState](ctx)
		if !wasInterrupted {
			return "", compose.StatefulInterrupt(ctx,
				map[string]any{"reason": "need human approval"},
				checkpointApprovalState{Original: input},
			)
		}
		isResume, hasData, decision := compose.GetResumeContext[string](ctx)
		if !isResume || !hasData {
			return "", compose.StatefulInterrupt(ctx, "approval still pending", state)
		}
		return fmt.Sprintf("%s -> approved by %s", state.Original, decision), nil
	}))
	g.AddEdge(compose.START, "approval")
	g.AddEdge("approval", compose.END)

	r, err := g.Compile(context.Background(),
		compose.WithGraphName("checkpoint_example"),
		compose.WithNodeTriggerMode(compose.AllPredecessor),
	)
	if err != nil {
		fmt.Printf("  Compile error: %v\n\n", err)
		return
	}

	firstCtx := compose.WithCheckPoint(context.Background(), "example18-cp", store)
	_, err = r.Invoke(firstCtx, "draft-answer")
	info, ok := compose.ExtractInterruptInfo(err)
	if !ok || len(info.InterruptContexts) == 0 {
		fmt.Printf("  Expected interrupt, got: %v\n\n", err)
		return
	}
	interruptID := info.InterruptContexts[0].ID
	fmt.Printf("  Interrupted at: %s\n", info.InterruptContexts[0].Address.String())
	fmt.Printf("  Interrupt ID:  %s\n", interruptID)

	resumeCtx := compose.ResumeWithData(
		compose.WithCheckPoint(context.Background(), "example18-cp", store),
		interruptID,
		"operator",
	)
	result, err := r.Invoke(resumeCtx, "")
	if err != nil {
		fmt.Printf("  Resume error: %v\n\n", err)
		return
	}
	fmt.Printf("  Resume result: %s\n", result)
	fmt.Println()
}
