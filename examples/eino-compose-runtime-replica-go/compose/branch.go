package compose

import "context"

type BranchCondition[I any] func(ctx context.Context, input I) (string, error)

type GraphBranch struct {
	condition func(ctx context.Context, input any) (string, error)
	branchMap map[string]bool
}

func NewGraphBranch[I any](condition BranchCondition[I], branchMap map[string]bool) *GraphBranch {
	return &GraphBranch{
		condition: func(ctx context.Context, input any) (string, error) {
			typedInput, ok := input.(I)
			if !ok {
				return "", fmtTypeError(input)
			}
			return condition(ctx, typedInput)
		},
		branchMap: branchMap,
	}
}
