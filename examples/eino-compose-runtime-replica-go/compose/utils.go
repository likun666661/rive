package compose

import "fmt"

func fmtTypeError(v any) error {
	return fmt.Errorf("branch condition: unexpected input type %T", v)
}
