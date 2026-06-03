package compose

import (
	"fmt"
	"reflect"
)

func fmtTypeError(v any) error {
	return fmt.Errorf("branch condition: unexpected input type %T", v)
}

func mergeValues(dynamic, static map[string]any) map[string]any {
	if dynamic == nil {
		return static
	}
	if static == nil {
		return dynamic
	}
	result := make(map[string]any, len(dynamic)+len(static))
	for k, v := range dynamic {
		result[k] = v
	}
	for k, v := range static {
		result[k] = v
	}
	return result
}

func applyStaticValues(input any, staticValues map[string]any) (any, error) {
	if staticValues == nil || len(staticValues) == 0 {
		return input, nil
	}

	inMap, ok := input.(map[string]any)
	if !ok {
		inMap = make(map[string]any)
	}

	result := mergeValues(inMap, staticValues)
	return result, nil
}

func convertMapToType(m map[string]any, typ reflect.Type) any {
	if typ == nil {
		return m
	}
	target := newInstanceByType(typ)
	if !target.CanAddr() {
		target = newInstanceByType(reflect.PointerTo(typ)).Elem()
	}
	for key, value := range m {
		target = assignOne(target, value, key)
	}
	return target.Interface()
}
