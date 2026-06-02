package compose

import "context"

type graphNode struct {
	name string
	cr   *composableRunnable
	g    *graph
	info *GraphNodeInfo
}

func (gn *graphNode) compileIfNeeded(ctx context.Context, option *graphCompileOptions) (*composableRunnable, error) {
	if gn.cr != nil {
		return gn.cr, nil
	}
	if gn.g != nil {
		r, err := gn.g.compile(ctx)
		if err != nil {
			return nil, err
		}
		gn.cr = r.toComposableRunnable()
		return gn.cr, nil
	}
	return nil, ErrNoCompiledRunnable
}
