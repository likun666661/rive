package compose

import (
	"errors"
	"reflect"
	"strings"
	"testing"
)

// --- FieldPath / splitFieldPath / join ---

func TestFieldPathJoinSplitRoundtrip(t *testing.T) {
	tests := []string{
		"",
		"single",
		"field1" + fieldPathSeparator + "field2",
		"a" + fieldPathSeparator + "b" + fieldPathSeparator + "c",
	}

	for _, s := range tests {
		fp := splitFieldPath(s)
		joined := fp.join()
		if joined != s {
			t.Errorf("roundtrip failed: split(%q).join() = %q, want %q", s, joined, s)
		}
	}
}

func TestFieldPathJoinEmptyPath(t *testing.T) {
	fp := FieldPath{}
	if s := fp.join(); s != "" {
		t.Errorf("empty path join() = %q, want empty", s)
	}
}

func TestSplitFieldPathEmpty(t *testing.T) {
	fp := splitFieldPath("")
	if len(fp) != 0 {
		t.Errorf("splitFieldPath(\"\") len = %d, want 0", len(fp))
	}
}

// --- Constructors ---

func TestMapFields(t *testing.T) {
	fm := MapFields("Query", "query")
	if fm.from != "Query" {
		t.Errorf("from = %q, want Query", fm.from)
	}
	if fm.to != "query" {
		t.Errorf("to = %q, want query", fm.to)
	}
}

func TestFromField(t *testing.T) {
	fm := FromField("Name")
	if fm.from != "Name" {
		t.Errorf("from = %q, want Name", fm.from)
	}
	if fm.to != "" {
		t.Errorf("to = %q, want empty (to-all)", fm.to)
	}
	if !fm.IsToAll() {
		t.Error("FromField should be to-all")
	}
}

func TestToField(t *testing.T) {
	fm := ToField("result")
	if fm.from != "" {
		t.Errorf("from = %q, want empty (from-all)", fm.from)
	}
	if fm.to != "result" {
		t.Errorf("to = %q, want result", fm.to)
	}
	if !fm.IsFromAll() {
		t.Error("ToField should be from-all")
	}
}

func TestMapFieldPaths(t *testing.T) {
	fm := MapFieldPaths(FieldPath{"user", "name"}, FieldPath{"response", "userName"})
	if fm.from != "user"+fieldPathSeparator+"name" {
		t.Errorf("from = %q", fm.from)
	}
	if fm.to != "response"+fieldPathSeparator+"userName" {
		t.Errorf("to = %q", fm.to)
	}
}

func TestFromFieldPath(t *testing.T) {
	fm := FromFieldPath(FieldPath{"data", "result"})
	if fm.from != "data"+fieldPathSeparator+"result" {
		t.Errorf("from = %q", fm.from)
	}
	if fm.to != "" {
		t.Errorf("to = %q, want empty (to-all)", fm.to)
	}
}

func TestToFieldPath(t *testing.T) {
	fm := ToFieldPath(FieldPath{"response", "data"})
	if fm.from != "" {
		t.Errorf("from = %q, want empty (from-all)", fm.from)
	}
	if fm.to != "response"+fieldPathSeparator+"data" {
		t.Errorf("to = %q", fm.to)
	}
}

// --- Accessors ---

func TestFieldMappingAccessors(t *testing.T) {
	fm := &FieldMapping{
		fromNodeKey: "node1",
		from:        "fieldA",
		to:          "fieldB",
	}

	if fm.FromNodeKey() != "node1" {
		t.Errorf("FromNodeKey = %q, want node1", fm.FromNodeKey())
	}
	if !reflect.DeepEqual(fm.FromPath(), FieldPath{"fieldA"}) {
		t.Errorf("FromPath = %v", fm.FromPath())
	}
	if !reflect.DeepEqual(fm.ToPath(), FieldPath{"fieldB"}) {
		t.Errorf("ToPath = %v", fm.ToPath())
	}
	if !reflect.DeepEqual(fm.TargetPath(), FieldPath{"fieldB"}) {
		t.Errorf("TargetPath = %v", fm.TargetPath())
	}
	if fm.IsFromAll() {
		t.Error("IsFromAll should be false")
	}
	if fm.IsToAll() {
		t.Error("IsToAll should be false")
	}
}

func TestFieldMappingIsFromAllToAll(t *testing.T) {
	ta := ToField("x")
	if !ta.IsFromAll() {
		t.Error("ToField should be IsFromAll")
	}
	if ta.IsToAll() {
		t.Error("ToField should not be IsToAll")
	}

	fa := FromField("x")
	if fa.IsFromAll() {
		t.Error("FromField should not be IsFromAll")
	}
	if !fa.IsToAll() {
		t.Error("FromField should be IsToAll")
	}
}

func TestHasCustomExtractor(t *testing.T) {
	fm := ToField("x", WithCustomExtractor(func(input any) (any, error) {
		return input, nil
	}))
	if !fm.HasCustomExtractor() {
		t.Error("HasCustomExtractor should be true")
	}

	fm2 := ToField("x")
	if fm2.HasCustomExtractor() {
		t.Error("HasCustomExtractor should be false")
	}
}

// --- validateFieldMapping ---

func TestValidateFieldMappingSimplePass(t *testing.T) {
	type Input struct {
		Name string
		Age  int
	}
	type Output struct {
		DisplayName string
		Years       int
	}

	_, _, err := validateFieldMapping(
		reflect.TypeOf(Input{}),
		reflect.TypeOf(Output{}),
		[]*FieldMapping{
			MapFields("Name", "DisplayName"),
		},
	)
	if err != nil {
		t.Errorf("expected pass, got error: %v", err)
	}
}

func TestValidateFieldMappingFromFieldToAll(t *testing.T) {
	type Input struct {
		Name string
	}

	_, _, err := validateFieldMapping(
		reflect.TypeOf(Input{}),
		reflect.TypeOf(""),
		[]*FieldMapping{
			FromField("Name"),
		},
	)
	if err != nil {
		t.Errorf("expected pass for FromField to scalar, got error: %v", err)
	}
}

func TestValidateFieldMappingToFieldFromAll(t *testing.T) {
	type Output struct {
		Query string
	}

	_, _, err := validateFieldMapping(
		reflect.TypeOf(""),
		reflect.TypeOf(Output{}),
		[]*FieldMapping{
			ToField("Query"),
		},
	)
	if err != nil {
		t.Errorf("expected pass for ToField from scalar, got error: %v", err)
	}
}

func TestValidateFieldMappingFieldNotFound(t *testing.T) {
	type Input struct {
		X string
	}
	type Output struct {
		Y string
	}

	_, _, err := validateFieldMapping(
		reflect.TypeOf(Input{}),
		reflect.TypeOf(Output{}),
		[]*FieldMapping{
			MapFields("NonExist", "Y"),
		},
	)
	if err == nil {
		t.Error("expected error for non-existent field")
	}
	if !strings.Contains(err.Error(), "has no field") {
		t.Errorf("expected 'has no field' in error, got: %v", err)
	}
}

func TestValidateFieldMappingUnexportedField(t *testing.T) {
	type Input struct {
		inner string
	}
	type Output struct {
		Y string
	}

	_, _, err := validateFieldMapping(
		reflect.TypeOf(Input{}),
		reflect.TypeOf(Output{}),
		[]*FieldMapping{
			MapFields("inner", "Y"),
		},
	)
	if err == nil {
		t.Error("expected error for unexported field")
	}
	if !strings.Contains(err.Error(), "unexported field") {
		t.Errorf("expected 'unexported field' in error, got: %v", err)
	}
}

func TestValidateFieldMappingTypeNotAssignable(t *testing.T) {
	type Input struct {
		Age int
	}
	type Output struct {
		Name string
	}

	_, _, err := validateFieldMapping(
		reflect.TypeOf(Input{}),
		reflect.TypeOf(Output{}),
		[]*FieldMapping{
			MapFields("Age", "Name"),
		},
	)
	if err == nil {
		t.Error("expected error for non-assignable int -> string")
	}
	if !strings.Contains(err.Error(), "absolutely not assignable") {
		t.Errorf("expected 'absolutely not assignable' in error, got: %v", err)
	}
}

func TestValidateFieldMappingFromAllToAllConflict(t *testing.T) {
	type Input struct {
		Name string
	}
	type Output struct {
		Name string
	}

	// One mapping is from-all, another is to-all
	_, _, err := validateFieldMapping(
		reflect.TypeOf(Input{}),
		reflect.TypeOf(Output{}),
		[]*FieldMapping{
			ToField("X"),      // from-all
			FromField("Name"), // to-all
		},
	)
	if err == nil {
		t.Error("expected error for from-all + to-all")
	}
}

func TestValidateFieldMappingInterfacePath(t *testing.T) {
	type Container struct {
		Data any
	}
	type Output struct {
		Result string
	}

	_, unchecked, err := validateFieldMapping(
		reflect.TypeOf(Container{}),
		reflect.TypeOf(Output{}),
		[]*FieldMapping{
			MapFieldPaths(FieldPath{"Data", "Value"}, FieldPath{"Result"}),
		},
	)
	if err != nil {
		t.Errorf("expected pass for interface intermediate (deferred), got error: %v", err)
	}
	// "Value" should be an unchecked path since Data is any
	dataPath := "Data" + fieldPathSeparator + "Value"
	if _, ok := unchecked[dataPath]; !ok {
		t.Errorf("expected unchecked path for %q, got %v", dataPath, unchecked)
	}
}

func TestValidateFieldMappingNestedPath(t *testing.T) {
	type Inner struct {
		Value string
	}
	type Input struct {
		Data Inner
	}
	type Output struct {
		Result string
	}

	_, _, err := validateFieldMapping(
		reflect.TypeOf(Input{}),
		reflect.TypeOf(Output{}),
		[]*FieldMapping{
			MapFieldPaths(FieldPath{"Data", "Value"}, FieldPath{"Result"}),
		},
	)
	if err != nil {
		t.Errorf("expected pass for nested path, got error: %v", err)
	}
}

func TestValidateFieldMappingSuccessorNotStructOrMap(t *testing.T) {
	type Input struct {
		X string
	}

	_, _, err := validateFieldMapping(
		reflect.TypeOf(Input{}),
		reflect.TypeOf(0),
		[]*FieldMapping{
			MapFields("X", "Y"),
		},
	)
	if err == nil {
		t.Error("expected error for non-struct/non-map successor with to-field")
	}
}

func TestValidateFieldMappingPointerTypes(t *testing.T) {
	type Input struct {
		Name string
	}
	type Output struct {
		Label string
	}

	_, _, err := validateFieldMapping(
		reflect.TypeOf(&Input{}),
		reflect.TypeOf(&Output{}),
		[]*FieldMapping{
			MapFields("Name", "Label"),
		},
	)
	if err != nil {
		t.Errorf("expected pass for pointer types, got error: %v", err)
	}
}

func TestValidateFieldMappingMapTypes(t *testing.T) {
	_, _, err := validateFieldMapping(
		reflect.TypeOf(map[string]any{}),
		reflect.TypeOf(map[string]any{}),
		[]*FieldMapping{
			MapFields("key1", "key2"),
		},
	)
	if err != nil {
		t.Errorf("expected pass for map types, got error: %v", err)
	}
}

func TestValidateFieldMappingCustomExtractorSkips(t *testing.T) {
	type Output struct {
		Result string
	}

	_, _, err := validateFieldMapping(
		reflect.TypeOf(""),
		reflect.TypeOf(Output{}),
		[]*FieldMapping{
			ToField("Result", WithCustomExtractor(func(input any) (any, error) {
				return input, nil
			})),
		},
	)
	if err != nil {
		t.Errorf("expected pass for custom extractor (skips type check), got error: %v", err)
	}
}

func TestValidateFieldMappingPredecessorNotStructOrMap(t *testing.T) {
	type Output struct {
		Y string
	}

	// FromField with scalar predecessor should fail because fromFields check
	_, _, err := validateFieldMapping(
		reflect.TypeOf(0),
		reflect.TypeOf(Output{}),
		[]*FieldMapping{
			FromField("Some"),
		},
	)
	if err == nil {
		t.Error("expected error for non-struct/non-map predecessor with from-field")
	}
}

// --- fieldMap ---

func TestFieldMapStructExtraction(t *testing.T) {
	type Input struct {
		Name string
		Age  int
	}

	fn := fieldMap([]*FieldMapping{
		MapFields("Name", "displayName"),
		MapFields("Age", "years"),
	}, false, nil)

	result, err := fn(Input{Name: "Alice", Age: 30})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result["displayName"] != "Alice" {
		t.Errorf("displayName = %v, want Alice", result["displayName"])
	}
	if result["years"] != 30 {
		t.Errorf("years = %v, want 30", result["years"])
	}
}

func TestFieldMapPointerInput(t *testing.T) {
	type Input struct {
		Name string
	}

	fn := fieldMap([]*FieldMapping{
		MapFields("Name", "name"),
	}, false, nil)

	result, err := fn(&Input{Name: "Bob"})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result["name"] != "Bob" {
		t.Errorf("name = %v, want Bob", result["name"])
	}
}

func TestFieldMapFullInput(t *testing.T) {
	fn := fieldMap([]*FieldMapping{
		ToField("whole"),
	}, false, nil)

	result, err := fn("hello")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result["whole"] != "hello" {
		t.Errorf("whole = %v, want hello", result["whole"])
	}
}

func TestFieldMapFromFieldToAll(t *testing.T) {
	type Input struct {
		Query string
	}

	fn := fieldMap([]*FieldMapping{
		FromField("Query"),
	}, false, nil)

	result, err := fn(Input{Query: "search term"})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	// FromField has empty "to", so the key is ""
	if result[""] != "search term" {
		t.Errorf("result[\"\"] = %v, want search term", result[""])
	}
}

func TestFieldMapNestedStructPath(t *testing.T) {
	type Inner struct {
		Value string
	}
	type Input struct {
		Data Inner
	}

	fn := fieldMap([]*FieldMapping{
		MapFieldPaths(FieldPath{"Data", "Value"}, FieldPath{"result"}),
	}, false, nil)

	result, err := fn(Input{Data: Inner{Value: "nested"}})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	resultKey := "result"
	if result[resultKey] != "nested" {
		t.Errorf("result = %v, want nested", result[resultKey])
	}
}

func TestFieldMapNestedPointerStructPath(t *testing.T) {
	type Inner struct {
		Value string
	}
	type Input struct {
		Data *Inner
	}

	fn := fieldMap([]*FieldMapping{
		MapFieldPaths(FieldPath{"Data", "Value"}, FieldPath{"result"}),
	}, false, nil)

	result, err := fn(&Input{Data: &Inner{Value: "from_ptr"}})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result["result"] != "from_ptr" {
		t.Errorf("result = %v, want from_ptr", result["result"])
	}
}

func TestFieldMapMapExtraction(t *testing.T) {
	fn := fieldMap([]*FieldMapping{
		MapFields("key_a", "a"),
		MapFields("key_b", "b"),
	}, false, nil)

	input := map[string]any{
		"key_a": "val_a",
		"key_b": 42,
	}
	result, err := fn(input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result["a"] != "val_a" {
		t.Errorf("a = %v, want val_a", result["a"])
	}
	if result["b"] != 42 {
		t.Errorf("b = %v, want 42", result["b"])
	}
}

func TestFieldMapNestedMapPath(t *testing.T) {
	fn := fieldMap([]*FieldMapping{
		MapFieldPaths(FieldPath{"outer", "inner"}, FieldPath{"val"}),
	}, false, nil)

	input := map[string]any{
		"outer": map[string]any{
			"inner": "deep",
		},
	}
	result, err := fn(input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result["val"] != "deep" {
		t.Errorf("val = %v, want deep", result["val"])
	}
}

func TestFieldMapKeyNotFoundError(t *testing.T) {
	fn := fieldMap([]*FieldMapping{
		MapFields("missing", "target"),
	}, false, nil)

	_, err := fn(map[string]any{"other": 1})
	if err == nil {
		t.Error("expected error for missing map key with allow=false")
	}
}

func TestFieldMapKeyNotFoundAllow(t *testing.T) {
	fn := fieldMap([]*FieldMapping{
		MapFields("present", "ok"),
		MapFields("missing", "skip"),
	}, true, nil)

	result, err := fn(map[string]any{"present": "here"})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result["ok"] != "here" {
		t.Errorf("ok = %v, want here", result["ok"])
	}
	if _, exists := result["skip"]; exists {
		t.Error("skip should not be in result")
	}
}

func TestFieldMapNilMap(t *testing.T) {
	fn := fieldMap([]*FieldMapping{
		MapFieldPaths(FieldPath{"a", "b"}, FieldPath{"c"}),
	}, false, nil)

	_, err := fn(map[string]any{"a": map[string]any(nil)})
	if err == nil {
		t.Error("expected error for nil intermediate map")
	}
}

// --- Custom Extractor ---

func TestFieldMapCustomExtractor(t *testing.T) {
	fn := fieldMap([]*FieldMapping{
		ToField("first", WithCustomExtractor(func(input any) (any, error) {
			arr := input.([]int)
			return arr[0], nil
		})),
	}, false, nil)

	result, err := fn([]int{10, 20, 30})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result["first"] != 10 {
		t.Errorf("first = %v, want 10", result["first"])
	}
}

func TestFieldMapCustomExtractorError(t *testing.T) {
	fn := fieldMap([]*FieldMapping{
		ToField("x", WithCustomExtractor(func(input any) (any, error) {
			return nil, &errMapKeyNotFound{MapKey: "custom"}
		})),
	}, false, nil)

	_, err := fn("test")
	if err == nil {
		t.Error("expected error from custom extractor")
	}
}

// --- takeOne ---

func TestTakeOneStructField(t *testing.T) {
	type S struct {
		Name string
		Age  int
	}

	val := reflect.ValueOf(S{Name: "Test", Age: 25})
	taken, takenType, err := takeOne(val, val.Type(), "Name")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if taken != "Test" {
		t.Errorf("taken = %v, want Test", taken)
	}
	if takenType.Kind() != reflect.String {
		t.Errorf("takenType = %v, want string", takenType)
	}
}

func TestTakeOneMapKey(t *testing.T) {
	m := map[string]any{"key": 42}
	val := reflect.ValueOf(m)
	taken, takenType, err := takeOne(val, val.Type(), "key")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if taken != 42 {
		t.Errorf("taken = %v, want 42", taken)
	}
	if takenType.Kind() != reflect.Interface {
		t.Errorf("takenType = %v, want interface (any map value type)", takenType)
	}
}

func TestTakeOneMapKeyNotFound(t *testing.T) {
	m := map[string]any{}
	val := reflect.ValueOf(m)
	_, _, err := takeOne(val, val.Type(), "nonexistent")
	if err == nil {
		t.Error("expected error for missing map key")
	}
	var keyErr *errMapKeyNotFound
	if !errors.As(err, &keyErr) {
		t.Errorf("expected errMapKeyNotFound, got %T: %v", err, err)
	}
}

func TestTakeOneStructFieldNotFound(t *testing.T) {
	type S struct {
		X string
	}
	val := reflect.ValueOf(S{})
	_, _, err := takeOne(val, val.Type(), "NonExist")
	if err == nil {
		t.Error("expected error for missing struct field")
	}
}

// --- assignOne / convertTo ---

func TestConvertToStruct(t *testing.T) {
	type Output struct {
		Name string
		Age  int
	}

	result := convertTo(map[string]any{
		"Name": "Alice",
		"Age":  30,
	}, reflect.TypeOf(Output{}))

	out, ok := result.(Output)
	if !ok {
		t.Fatalf("result is not Output, got %T", result)
	}
	if out.Name != "Alice" {
		t.Errorf("Name = %v, want Alice", out.Name)
	}
	if out.Age != 30 {
		t.Errorf("Age = %v, want 30", out.Age)
	}
}

func TestConvertToPointerStruct(t *testing.T) {
	type Output struct {
		Label string
	}

	result := convertTo(map[string]any{
		"Label": "hello",
	}, reflect.TypeOf(&Output{}))

	out, ok := result.(*Output)
	if !ok {
		t.Fatalf("result is not *Output, got %T", result)
	}
	if out.Label != "hello" {
		t.Errorf("Label = %v, want hello", out.Label)
	}
}

func TestConvertToMap(t *testing.T) {
	result := convertTo(map[string]any{
		"a": "val_a",
		"b": 42,
	}, reflect.TypeOf(map[string]any{}))

	out, ok := result.(map[string]any)
	if !ok {
		t.Fatalf("result is not map[string]any, got %T", result)
	}
	if out["a"] != "val_a" {
		t.Errorf("a = %v", out["a"])
	}
	if out["b"] != 42 {
		t.Errorf("b = %v", out["b"])
	}
}

func TestConvertToNestedStruct(t *testing.T) {
	type Inner struct {
		Value string
	}
	type Output struct {
		Data Inner
	}

	result := convertTo(map[string]any{
		"Data" + fieldPathSeparator + "Value": "nested_val",
	}, reflect.TypeOf(Output{}))

	out, ok := result.(Output)
	if !ok {
		t.Fatalf("result is not Output, got %T", result)
	}
	if out.Data.Value != "nested_val" {
		t.Errorf("Data.Value = %v, want nested_val", out.Data.Value)
	}
}

func TestConvertToNestedMap(t *testing.T) {
	result := convertTo(map[string]any{
		"outer" + fieldPathSeparator + "inner": "deep",
	}, reflect.TypeOf(map[string]any{}))

	out, ok := result.(map[string]any)
	if !ok {
		t.Fatalf("result is not map[string]any, got %T", result)
	}
	inner, ok := out["outer"].(map[string]any)
	if !ok {
		t.Fatalf("outer is not map[string]any, got %T", out["outer"])
	}
	if inner["inner"] != "deep" {
		t.Errorf("outer.inner = %v, want deep", inner["inner"])
	}
}

func TestAssignOneNestedStructPath(t *testing.T) {
	type Inner struct {
		Value string
	}
	type Output struct {
		Data Inner
	}

	val := newInstanceByType(reflect.TypeOf(Output{}))
	val = assignOne(val, "hello", "Data"+fieldPathSeparator+"Value")

	out := val.Interface().(Output)
	if out.Data.Value != "hello" {
		t.Errorf("Data.Value = %v, want hello", out.Data.Value)
	}
}

// --- checkAndExtractFieldType ---

func TestCheckAndExtractFieldTypeSimple(t *testing.T) {
	type S struct {
		Name string
	}
	extracted, remaining, err := checkAndExtractFieldType([]string{"Name"}, reflect.TypeOf(S{}))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if remaining != nil {
		t.Errorf("expected no remaining paths, got %v", remaining)
	}
	if extracted.Kind() != reflect.String {
		t.Errorf("extracted = %v, want string", extracted)
	}
}

func TestCheckAndExtractFieldTypeNested(t *testing.T) {
	type Inner struct {
		Value string
	}
	type Outer struct {
		Data Inner
	}
	extracted, _, err := checkAndExtractFieldType([]string{"Data", "Value"}, reflect.TypeOf(Outer{}))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if extracted.Kind() != reflect.String {
		t.Errorf("extracted = %v, want string", extracted)
	}
}

func TestCheckAndExtractFieldTypeMap(t *testing.T) {
	extracted, _, err := checkAndExtractFieldType([]string{"key"}, reflect.TypeOf(map[string]int{}))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if extracted.Kind() != reflect.Int {
		t.Errorf("extracted = %v, want int", extracted)
	}
}

func TestCheckAndExtractFieldTypeInterface(t *testing.T) {
	type S struct {
		Data any
	}
	extracted, remaining, err := checkAndExtractFieldType([]string{"Data", "Value"}, reflect.TypeOf(S{}))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(remaining) == 0 {
		t.Error("expected remaining paths for interface, got none")
	}
	if remaining[0] != "Value" {
		t.Errorf("remaining[0] = %q, want Value", remaining[0])
	}
	_ = extracted
}

// --- streamFieldMap stub ---

func TestStreamFieldMapNotImplemented(t *testing.T) {
	defer func() {
		if r := recover(); r == nil {
			t.Error("expected panic for streamFieldMap")
		}
	}()
	fn := streamFieldMap(nil, nil)
	fn(nil)
}

// --- errors ---

func TestErrMapKeyNotFoundIs(t *testing.T) {
	e := &errMapKeyNotFound{MapKey: "test_key"}
	if e.Error() != "key=test_key" {
		t.Errorf("Error() = %q, want key=test_key", e.Error())
	}
}

func TestErrInterfaceNotValidForFieldMapping(t *testing.T) {
	e := &errInterfaceNotValidForFieldMapping{
		InterfaceType: reflect.TypeOf((*any)(nil)).Elem(),
		ActualType:    reflect.TypeOf(42),
	}
	if !strings.Contains(e.Error(), "interface") {
		t.Errorf("unexpected error message: %v", e.Error())
	}
}

// --- checkAssignable direct tests ---

func TestCheckAssignableMust(t *testing.T) {
	if at := checkAssignable(reflect.TypeOf(""), reflect.TypeOf("")); at != assignableTypeMust {
		t.Errorf("string → string should be Must, got %v", at)
	}
}

func TestCheckAssignableMustNot(t *testing.T) {
	if at := checkAssignable(reflect.TypeOf(42), reflect.TypeOf("")); at != assignableTypeMustNot {
		t.Errorf("int → string should be MustNot, got %v", at)
	}
}

func TestCheckAssignableNil(t *testing.T) {
	if at := checkAssignable(nil, reflect.TypeOf("")); at != assignableTypeMust {
		t.Errorf("nil → string should be Must, got %v", at)
	}
}

func TestCheckAssignablePtrTo(t *testing.T) {
	type S struct{ X int }
	// *S is not assignable to S, but S is assignable to S
	if at := checkAssignable(reflect.TypeOf(&S{}), reflect.TypeOf(S{})); at != assignableTypeMustNot {
		t.Errorf("*S → S should be MustNot, got %v", at)
	}
}

// --- nil pointer in struct field (accepted as nil value) ---

func TestFieldMapNilPointerStructField(t *testing.T) {
	type Inner struct {
		Value string
	}
	type Input struct {
		Data *Inner
	}

	fn := fieldMap([]*FieldMapping{
		MapFields("Data", "result"),
	}, false, nil)

	result, err := fn(&Input{Data: nil})
	if err != nil {
		t.Fatalf("expected success for nil pointer field, got error: %v", err)
	}
	// nil pointer field is extracted as a typed nil (*Inner)(nil)
	val, exists := result["result"]
	if !exists {
		t.Fatal("expected 'result' key in output")
	}
	rv := reflect.ValueOf(val)
	if rv.IsValid() && rv.Kind() == reflect.Ptr && !rv.IsNil() {
		t.Errorf("expected nil pointer, got non-nil: %v", val)
	}
}

func TestFieldMapThroughInterfaceValue(t *testing.T) {
	// When input has an `any` field containing a struct
	type Input struct {
		Payload any
	}

	// The mapping tries to extract a field from the any payload
	fn := fieldMap([]*FieldMapping{
		MapFieldPaths(FieldPath{"Payload", "Name"}, FieldPath{"name"}),
	}, false, nil)

	payload := map[string]string{"Name": "test"}
	_, err := fn(Input{Payload: payload})
	if err != nil {
		t.Fatalf("unexpected error extracting through interface: %v", err)
	}
}

func TestFieldMapThroughNilInterfaceValue(t *testing.T) {
	type Input struct {
		Payload any
	}

	fn := fieldMap([]*FieldMapping{
		MapFieldPaths(FieldPath{"Payload", "Name"}, FieldPath{"name"}),
	}, false, nil)

	_, err := fn(Input{Payload: nil})
	if err == nil {
		t.Fatal("expected error for nil interface value in fieldMap")
	}
}

// --- convertTo edge cases ---

func TestConvertToNestedMapThroughPath(t *testing.T) {
	type Inner struct {
		Value string
		Count int
	}
	type Output struct {
		Data Inner
	}

	result := convertTo(map[string]any{
		"Data" + fieldPathSeparator + "Value": "nested",
		"Data" + fieldPathSeparator + "Count": 42,
	}, reflect.TypeOf(Output{}))

	out, ok := result.(Output)
	if !ok {
		t.Fatalf("result is not Output, got %T", result)
	}
	if out.Data.Value != "nested" {
		t.Errorf("Data.Value = %v, want nested", out.Data.Value)
	}
	if out.Data.Count != 42 {
		t.Errorf("Data.Count = %v, want 42", out.Data.Count)
	}
}

func TestConvertToPointerToMap(t *testing.T) {
	result := convertTo(map[string]any{
		"a": "hello",
	}, reflect.TypeOf(&map[string]any{}))

	out, ok := result.(*map[string]any)
	if !ok {
		t.Fatalf("result is not *map[string]any, got %T", result)
	}
	if (*out)["a"] != "hello" {
		t.Errorf("a = %v", (*out)["a"])
	}
}

func TestCheckAndExtractFieldTypeDeepNested(t *testing.T) {
	type Level3 struct {
		Z string
	}
	type Level2 struct {
		Y Level3
	}
	type Level1 struct {
		X Level2
	}

	extracted, remaining, err := checkAndExtractFieldType(
		[]string{"X", "Y", "Z"},
		reflect.TypeOf(Level1{}),
	)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if extracted.Kind() != reflect.String {
		t.Errorf("extracted = %v, want string", extracted)
	}
	if remaining != nil {
		t.Errorf("expected no remaining paths, got %v", remaining)
	}
}

func TestCheckAndExtractFieldTypeDeepMap(t *testing.T) {
	extracted, remaining, err := checkAndExtractFieldType(
		[]string{"a", "b", "c"},
		reflect.TypeOf(map[string]map[string]map[string]int{}),
	)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if extracted.Kind() != reflect.Int {
		t.Errorf("extracted = %v, want int", extracted)
	}
	if remaining != nil {
		t.Errorf("expected no remaining paths, got %v", remaining)
	}
}
