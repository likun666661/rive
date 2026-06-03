package compose

import (
	"errors"
	"fmt"
	"reflect"
	"strings"
)

const fieldPathSeparator = "\x1F"

var strType = reflect.TypeOf("")

// FieldPath represents a path to a nested field in a struct or map.
// Each element in the path is either a struct field name or a map key.
type FieldPath []string

func (fp FieldPath) join() string {
	return strings.Join(fp, fieldPathSeparator)
}

func splitFieldPath(path string) FieldPath {
	p := strings.Split(path, fieldPathSeparator)
	if len(p) == 1 && p[0] == "" {
		return FieldPath{}
	}
	return p
}

// FieldMapping describes a field-level data transfer.
// from and to fields use \x1F as an internal nested path separator.
type FieldMapping struct {
	fromNodeKey     string
	from            string
	to              string
	customExtractor func(input any) (any, error)
}

// MapFields creates a FieldMapping that maps a single predecessor field to a single successor field.
func MapFields(from, to string) *FieldMapping {
	return &FieldMapping{from: from, to: to}
}

// FromField creates a FieldMapping that maps a single predecessor field to the entire successor input.
func FromField(from string) *FieldMapping {
	return &FieldMapping{from: from}
}

// ToField creates a FieldMapping that maps the entire predecessor output to a single successor field.
func ToField(to string, opts ...FieldMappingOption) *FieldMapping {
	fm := &FieldMapping{to: to}
	for _, opt := range opts {
		opt(fm)
	}
	return fm
}

// MapFieldPaths creates a FieldMapping that maps a nested predecessor field path to a nested successor field path.
func MapFieldPaths(fromPath, toPath FieldPath) *FieldMapping {
	return &FieldMapping{from: fromPath.join(), to: toPath.join()}
}

// FromFieldPath creates a FieldMapping that maps a nested predecessor field path to the entire successor input.
func FromFieldPath(fromPath FieldPath) *FieldMapping {
	return &FieldMapping{from: fromPath.join()}
}

// ToFieldPath creates a FieldMapping that maps the entire predecessor output to a nested successor field path.
func ToFieldPath(toPath FieldPath, opts ...FieldMappingOption) *FieldMapping {
	fm := &FieldMapping{to: toPath.join()}
	for _, opt := range opts {
		opt(fm)
	}
	return fm
}

// FieldMappingOption is a functional option for configuring a FieldMapping.
type FieldMappingOption func(*FieldMapping)

// WithCustomExtractor sets a custom extractor function for the FieldMapping.
func WithCustomExtractor(extractor func(input any) (any, error)) FieldMappingOption {
	return func(m *FieldMapping) {
		m.customExtractor = extractor
	}
}

func (m *FieldMapping) FromNodeKey() string      { return m.fromNodeKey }
func (m *FieldMapping) FromPath() FieldPath      { return splitFieldPath(m.from) }
func (m *FieldMapping) ToPath() FieldPath        { return splitFieldPath(m.to) }
func (m *FieldMapping) TargetPath() FieldPath    { return splitFieldPath(m.to) }
func (m *FieldMapping) IsFromAll() bool          { return m.from == "" }
func (m *FieldMapping) IsToAll() bool            { return m.to == "" }
func (m *FieldMapping) HasCustomExtractor() bool { return m.customExtractor != nil }

// handlerPair is used for deferred type checking at request time.
type handlerPair struct {
	invoke func(any) (any, error)
}

type assignableType int

const (
	assignableTypeMust    assignableType = 1
	assignableTypeMustNot assignableType = 2
	assignableTypeMay     assignableType = 3
)

// errMapKeyNotFound is returned when a map key is not found during field extraction.
type errMapKeyNotFound struct {
	MapKey string
}

func (e *errMapKeyNotFound) Error() string {
	return fmt.Sprintf("key=%s", e.MapKey)
}

// errInterfaceNotValidForFieldMapping is returned when an interface value
// is not a struct, struct pointer, or map during field mapping.
type errInterfaceNotValidForFieldMapping struct {
	InterfaceType reflect.Type
	ActualType    reflect.Type
}

func (e *errInterfaceNotValidForFieldMapping) Error() string {
	return fmt.Sprintf("field mapping from an interface type, but actual type is not struct, struct ptr or map. InterfaceType= %v, ActualType= %v", e.InterfaceType, e.ActualType)
}

func newInstanceByType(typ reflect.Type) reflect.Value {
	switch typ.Kind() {
	case reflect.Map:
		return reflect.MakeMap(typ)
	case reflect.Slice, reflect.Array:
		slice := reflect.New(typ).Elem()
		slice.Set(reflect.MakeSlice(typ, 0, 0))
		return slice
	case reflect.Ptr:
		typ = typ.Elem()
		origin := reflect.New(typ)
		nested := newInstanceByType(typ)
		origin.Elem().Set(nested)
		return origin
	default:
		return reflect.New(typ).Elem()
	}
}

func instantiateIfNeeded(field reflect.Value) {
	if field.Kind() == reflect.Ptr {
		if field.IsNil() {
			field.Set(reflect.New(field.Type().Elem()))
		}
	} else if field.Kind() == reflect.Map {
		if field.IsNil() {
			field.Set(reflect.MakeMap(field.Type()))
		}
	}
}

// checkAndExtractFieldType walks a type along a field path, extracting the type at each step.
// Returns the extracted type, any remaining unchecked paths (when encountering interface), or an error.
func checkAndExtractFieldType(paths []string, typ reflect.Type) (extracted reflect.Type, remainingPaths FieldPath, err error) {
	extracted = typ
	for i, field := range paths {
		for extracted.Kind() == reflect.Ptr {
			extracted = extracted.Elem()
		}

		if extracted.Kind() == reflect.Map {
			if !strType.ConvertibleTo(extracted.Key()) {
				return nil, nil, fmt.Errorf("type[%v] is not a map with string or string alias key", extracted)
			}
			extracted = extracted.Elem()
			continue
		}

		if extracted.Kind() == reflect.Struct {
			f, ok := extracted.FieldByName(field)
			if !ok {
				return nil, nil, fmt.Errorf("type[%v] has no field[%s]", extracted, field)
			}
			if !f.IsExported() {
				return nil, nil, fmt.Errorf("type[%v] has an unexported field[%s]", extracted, field)
			}
			extracted = f.Type
			continue
		}

		if extracted.Kind() == reflect.Interface {
			return extracted, paths[i:], nil
		}

		return nil, nil, fmt.Errorf("intermediate type[%v] is not valid", extracted)
	}
	return extracted, nil, nil
}

func checkAssignable(from, to reflect.Type) assignableType {
	if from == nil {
		return assignableTypeMust
	}
	if from.AssignableTo(to) {
		return assignableTypeMust
	}
	if reflect.PtrTo(from).AssignableTo(to) {
		return assignableTypeMust
	}
	if to.Kind() == reflect.Interface {
		if from.Implements(to) || reflect.PtrTo(from).Implements(to) {
			return assignableTypeMay
		}
	}
	return assignableTypeMustNot
}

func validateStructOrMap(t reflect.Type) bool {
	switch t.Kind() {
	case reflect.Map:
		return true
	case reflect.Ptr:
		t = t.Elem()
		fallthrough
	case reflect.Struct:
		return true
	default:
		return false
	}
}

func isFromAll(mappings []*FieldMapping) bool {
	for _, m := range mappings {
		if len(m.from) == 0 && m.customExtractor == nil {
			return true
		}
	}
	return false
}

func isToAll(mappings []*FieldMapping) bool {
	for _, m := range mappings {
		if len(m.to) == 0 {
			return true
		}
	}
	return false
}

func fromFields(mappings []*FieldMapping) bool {
	for _, m := range mappings {
		if len(m.from) == 0 || m.customExtractor != nil {
			return false
		}
	}
	return true
}

// validateFieldMapping performs compile-time static checking of field mappings.
// It verifies field existence, export, and type assignability.
// Returns a handlerPair for deferred checks, unchecked source paths, or an error.
func validateFieldMapping(predecessorType, successorType reflect.Type, mappings []*FieldMapping) (
	typeHandler *handlerPair,
	uncheckedSourcePath map[string]FieldPath,
	err error,
) {
	if isFromAll(mappings) && isToAll(mappings) {
		return nil, nil, fmt.Errorf("invalid field mappings: from all fields to all, use common edge instead")
	}
	if !isToAll(mappings) && !validateStructOrMap(successorType) && successorType != reflect.TypeOf((*any)(nil)).Elem() {
		return nil, nil, fmt.Errorf("static check fail: successor input type should be struct or map, actual: %v", successorType)
	}
	if fromFields(mappings) && !validateStructOrMap(predecessorType) {
		return nil, nil, fmt.Errorf("static check fail: predecessor output type should be struct or map, actual: %v", predecessorType)
	}

	var (
		fieldCheckers map[string]func(any) (any, error)
	)

	for _, mapping := range mappings {
		successorFieldType, successorRemaining, sErr := checkAndExtractFieldType(splitFieldPath(mapping.to), successorType)
		if sErr != nil {
			return nil, nil, fmt.Errorf("static check failed for mapping from=[%s] to=[%s]: %w", mapping.from, mapping.to, sErr)
		}
		if len(successorRemaining) > 0 {
			if successorFieldType == reflect.TypeOf((*any)(nil)).Elem() {
				continue
			}
			return nil, nil, fmt.Errorf("static check failed: the successor has intermediate interface type %v", successorFieldType)
		}

		if mapping.customExtractor != nil {
			continue
		}

		predecessorFieldType, predecessorRemaining, pErr := checkAndExtractFieldType(splitFieldPath(mapping.from), predecessorType)
		if pErr != nil {
			return nil, nil, fmt.Errorf("static check failed for mapping from=[%s] to=[%s]: %w", mapping.from, mapping.to, pErr)
		}
		if len(predecessorRemaining) > 0 {
			if uncheckedSourcePath == nil {
				uncheckedSourcePath = make(map[string]FieldPath)
			}
			uncheckedSourcePath[mapping.from] = predecessorRemaining
		}

		checker := func(a any) (any, error) {
			trueInType := reflect.TypeOf(a)
			if trueInType == nil {
				switch successorFieldType.Kind() {
				case reflect.Map, reflect.Slice, reflect.Ptr, reflect.Interface:
				default:
					return nil, fmt.Errorf("runtime check failed: field is absolutely not assignable from nil to %v", successorFieldType)
				}
			} else if !trueInType.AssignableTo(successorFieldType) {
				return nil, fmt.Errorf("runtime check failed: field [%v] is absolutely not assignable to [%v]", trueInType, successorFieldType)
			}
			return a, nil
		}

		if len(predecessorRemaining) > 0 {
			if fieldCheckers == nil {
				fieldCheckers = make(map[string]func(any) (any, error))
			}
			fieldCheckers[mapping.to] = checker
		} else {
			at := checkAssignable(predecessorFieldType, successorFieldType)
			if at == assignableTypeMustNot {
				return nil, nil, fmt.Errorf("static check failed: field[%v] is absolutely not assignable to [%v]", predecessorFieldType, successorFieldType)
			}
			if at == assignableTypeMay {
				if fieldCheckers == nil {
					fieldCheckers = make(map[string]func(any) (any, error))
				}
				fieldCheckers[mapping.to] = checker
			}
		}
	}

	if len(fieldCheckers) == 0 {
		return nil, uncheckedSourcePath, nil
	}

	return &handlerPair{
		invoke: func(value any) (any, error) {
			mv, ok := value.(map[string]any)
			if !ok {
				return value, nil
			}
			for k, checkFn := range fieldCheckers {
				if v, exists := mv[k]; exists {
					checked, checkErr := checkFn(v)
					if checkErr != nil {
						return nil, checkErr
					}
					mv[k] = checked
				}
			}
			return mv, nil
		},
	}, uncheckedSourcePath, nil
}

// fieldMap returns a function that extracts field values from input according to mappings.
// allowMapKeyNotFound controls whether missing map keys are skipped or cause errors.
func fieldMap(mappings []*FieldMapping, allowMapKeyNotFound bool, uncheckedSourcePaths map[string]FieldPath) func(any) (map[string]any, error) {
	return func(input any) (map[string]any, error) {
		result := make(map[string]any, len(mappings))
		var inputValue reflect.Value

	loop:
		for _, mapping := range mappings {
			if mapping.customExtractor != nil {
				var err error
				result[mapping.to], err = mapping.customExtractor(input)
				if err != nil {
					return nil, err
				}
				continue
			}

			if len(mapping.from) == 0 {
				result[mapping.to] = input
				continue
			}

			fromPath := splitFieldPath(mapping.from)

			if !inputValue.IsValid() {
				inputValue = reflect.ValueOf(input)
			}

			var (
				pathInputValue = inputValue
				pathInputType  = inputValue.Type()
				taken          = input
			)

			for i, path := range fromPath {
				for pathInputValue.Kind() == reflect.Ptr {
					pathInputValue = pathInputValue.Elem()
				}

				if !pathInputValue.IsValid() {
					return nil, fmt.Errorf("intermediate source value on path=%v is nil for type [%v]", fromPath[:i+1], pathInputType)
				}

				if pathInputValue.Kind() == reflect.Map && pathInputValue.IsNil() {
					return nil, fmt.Errorf("intermediate source value on path=%v is nil for map type [%v]", fromPath[:i+1], pathInputType)
				}

				var err error
				taken, pathInputType, err = takeOne(pathInputValue, pathInputType, path)
				if err != nil {
					var interfaceNotValidErr *errInterfaceNotValidForFieldMapping
					if errors.As(err, &interfaceNotValidErr) {
						return nil, err
					}

					var mapKeyNotFoundErr *errMapKeyNotFound
					if errors.As(err, &mapKeyNotFoundErr) {
						if allowMapKeyNotFound {
							continue loop
						}
						return nil, err
					}

					if uncheckedSourcePaths != nil {
						uncheckedPath, ok := uncheckedSourcePaths[mapping.from]
						if ok && len(uncheckedPath) >= len(fromPath)-i {
							return nil, err
						}
					}

					return nil, err
				}

				if i < len(fromPath)-1 {
					pathInputValue = reflect.ValueOf(taken)
				}
			}

			result[mapping.to] = taken
		}

		return result, nil
	}
}

// streamFieldMap returns a stub stream mapping function.
// Stream mapping is not implemented in this replica.
func streamFieldMap(mappings []*FieldMapping, uncheckedSourcePaths map[string]FieldPath) func(any) any {
	return func(input any) any {
		panic("streamFieldMap: not implemented")
	}
}

func checkAndExtractFromField(fromField string, input reflect.Value) (reflect.Value, error) {
	f := input.FieldByName(fromField)
	if !f.IsValid() {
		return reflect.Value{}, fmt.Errorf("field mapping from a struct field, but field not found. field=%v, inputType=%v", fromField, input.Type())
	}
	if !f.CanInterface() {
		return reflect.Value{}, fmt.Errorf("field mapping from a struct field, but field not exported. field=%v, inputType=%v", fromField, input.Type())
	}
	return f, nil
}

func checkAndExtractFromMapKey(fromMapKey string, input reflect.Value) (reflect.Value, error) {
	key := reflect.ValueOf(fromMapKey)
	if input.Type().Key() != strType {
		key = key.Convert(input.Type().Key())
	}
	v := input.MapIndex(key)
	if !v.IsValid() {
		return reflect.Value{}, fmt.Errorf("field mapping from a map key, but key not found in input. %w", &errMapKeyNotFound{MapKey: fromMapKey})
	}
	return v, nil
}

// takeOne extracts a single value from a struct field or map key.
func takeOne(inputValue reflect.Value, inputType reflect.Type, from string) (taken any, takenType reflect.Type, err error) {
	switch inputValue.Kind() {
	case reflect.Map:
		f, err := checkAndExtractFromMapKey(from, inputValue)
		if err != nil {
			return nil, nil, err
		}
		return f.Interface(), f.Type(), nil
	case reflect.Struct:
		f, err := checkAndExtractFromField(from, inputValue)
		if err != nil {
			return nil, nil, err
		}
		return f.Interface(), f.Type(), nil
	default:
		if inputType.Kind() == reflect.Interface {
			return nil, nil, &errInterfaceNotValidForFieldMapping{
				InterfaceType: inputType,
				ActualType:    inputValue.Type(),
			}
		}
		return nil, nil, fmt.Errorf("when take one value from source, value not map or struct, and type not interface")
	}
}

// assignOne writes a value into a destination struct or map at the given field path.
func assignOne(destValue reflect.Value, taken any, to string) reflect.Value {
	if len(to) == 0 {
		if takenVal := reflect.ValueOf(taken); takenVal.IsValid() {
			destValue.Set(takenVal)
		}
		return destValue
	}

	toPaths := splitFieldPath(to)
	originalDestValue := destValue
	var parentMap reflect.Value
	var parentKey string

	for {
		path := toPaths[0]
		toPaths = toPaths[1:]
		if len(toPaths) == 0 {
			toSet := reflect.ValueOf(taken)

			if destValue.Type() == reflect.TypeOf((*any)(nil)).Elem() {
				existingMap, ok := destValue.Interface().(map[string]any)
				if ok {
					destValue = reflect.ValueOf(existingMap)
				} else {
					mapValue := reflect.MakeMap(reflect.TypeOf(map[string]any{}))
					destValue.Set(mapValue)
					destValue = mapValue
				}
			}

			if destValue.Kind() == reflect.Map {
				key := reflect.ValueOf(path)
				keyType := destValue.Type().Key()
				if keyType != strType {
					key = key.Convert(keyType)
				}
				if !toSet.IsValid() {
					toSet = reflect.Zero(destValue.Type().Elem())
				}
				destValue.SetMapIndex(key, toSet)
				if parentMap.IsValid() {
					parentMap.SetMapIndex(reflect.ValueOf(parentKey), destValue)
				}
				return originalDestValue
			}

			ptrValue := destValue
			for destValue.Kind() == reflect.Ptr {
				destValue = destValue.Elem()
			}
			if destValue.Kind() == reflect.Map {
				key := reflect.ValueOf(path)
				keyType := destValue.Type().Key()
				if keyType != strType {
					key = key.Convert(keyType)
				}
				if !toSet.IsValid() {
					toSet = reflect.Zero(destValue.Type().Elem())
				}
				destValue.SetMapIndex(key, toSet)
				if parentMap.IsValid() {
					parentMap.SetMapIndex(reflect.ValueOf(parentKey), ptrValue)
				}
				return originalDestValue
			}
			if toSet.IsValid() {
				field := destValue.FieldByName(path)
				field.Set(toSet)
			}
			if parentMap.IsValid() {
				parentMap.SetMapIndex(reflect.ValueOf(parentKey), ptrValue)
			}
			return originalDestValue
		}

		if destValue.Type() == reflect.TypeOf((*any)(nil)).Elem() {
			existingMap, ok := destValue.Interface().(map[string]any)
			if ok {
				destValue = reflect.ValueOf(existingMap)
			} else {
				mapValue := reflect.MakeMap(reflect.TypeOf(map[string]any{}))
				destValue.Set(mapValue)
				destValue = mapValue
			}
		}

		if destValue.Kind() == reflect.Map {
			keyValue := reflect.ValueOf(path)
			valueValue := destValue.MapIndex(keyValue)
			if !valueValue.IsValid() {
				valueValue = newInstanceByType(destValue.Type().Elem())
				destValue.SetMapIndex(keyValue, valueValue)
			}
			if parentMap.IsValid() {
				parentMap.SetMapIndex(reflect.ValueOf(parentKey), destValue)
			}
			parentMap = destValue
			parentKey = path
			destValue = valueValue
			continue
		}

		ptrValue := destValue
		for destValue.Kind() == reflect.Ptr {
			destValue = destValue.Elem()
		}

		field := destValue.FieldByName(path)
		instantiateIfNeeded(field)

		if parentMap.IsValid() {
			parentMap.SetMapIndex(reflect.ValueOf(parentKey), ptrValue)
			parentMap = reflect.Value{}
			parentKey = ""
		}
		destValue = field
	}
}

// convertTo converts a map[string]any intermediate result to the target Go type.
func convertTo(mappings map[string]any, typ reflect.Type) any {
	tValue := newInstanceByType(typ)
	if !tValue.CanAddr() {
		tValue = newInstanceByType(reflect.PointerTo(typ)).Elem()
	}
	for mapping, taken := range mappings {
		tValue = assignOne(tValue, taken, mapping)
	}
	return tValue.Interface()
}
