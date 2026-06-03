package main

import (
	"context"
	"fmt"
	"strings"

	compose "github.com/rive/eino-compose-runtime-replica-go/compose"
)

func main() {
	fmt.Println("=== Eino Compose Runtime Replica — Chapter 1 & 2 综合示例 ===")
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
	fmt.Println("│ 组件桥接 (ChatModel/Tool/Retriever)  │ 依赖外部组件接口体系         │")
	fmt.Println("│ AddChatModelNode / AppendChatModel   │ 当前仅有 Lambda 抽象         │")
	fmt.Println("│ Stream 执行 (Stream/Collect)         │ StreamReader 抽象未完成      │")
	fmt.Println("│ streamFieldMap 流式字段映射          │ 依赖完整 stream reader       │")
	fmt.Println("│ Stream ChainBranch                   │ 流式分支 Stub               │")
	fmt.Println("│ Callback 机制 (OnStart/OnEnd/OnError)│ 不在当前范围内               │")
	fmt.Println("│ State 传递 (graph.state)             │ 字段定义但未使用             │")
	fmt.Println("│ Checkpoint / Recovery                │ 可恢复执行机制不在范围内     │")
	fmt.Println("│ values_merge 的 StreamReader merge    │ StreamReader 未完成          │")
	fmt.Println("│ 编译时类型推断 (toValidateMap)       │ 推迟到后续版本               │")
	fmt.Println("│ Graph 可视化 / DOT 导出              │ 周边工具未实现               │")
	fmt.Println("│ JSON Schema 编译校验                 │ 类型系统限制                  │")
	fmt.Println("│ Tracing / Metrics / Profiling        │ DevOps 工具不在范围内         │")
	fmt.Println("│ 外部依赖集成 (Eino 官方库)           │ 纯 Go 标准库                 │")
	fmt.Println("│ Fan-in 智能合并 (Merge 配置)         │ 当前默认 map[string]any 合并  │")
	fmt.Println("└─────────────────────────────────────────────────────────────────────┘")
	fmt.Println()
	fmt.Println("  可替代方案:")
	fmt.Println("  - 组件桥接: 通过 AddLambdaNode + InvokableLambda 包装实现等价功能")
	fmt.Println("  - 流式处理: composableRunnable 已预留 s 字段，可后续扩展")
	fmt.Println("  - 类型推断: 当前通过 fmtType() 返回简单类型名，复杂类型标注为 \"any\"")
	fmt.Println()
}
