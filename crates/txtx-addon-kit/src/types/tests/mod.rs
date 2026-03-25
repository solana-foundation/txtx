use std::path::Path;
use std::path::PathBuf;

use crate::helpers::fs::FileLocation;
use crate::types::AuthorizationContext;

use super::types::{Type, Value};
use serde_json::json;
use serde_json::Value as JsonValue;
use test_case::test_case;

lazy_static! {
    static ref BYTES: Vec<u8> = hex::decode("ffffff").unwrap();
}

#[test_case(Value::string("Test".to_string()))]
#[test_case(Value::integer(1))]
#[test_case(Value::integer(-10))]
#[test_case(Value::bool(true))]
#[test_case(Value::bool(false))]
#[test_case(Value::null())]
#[test_case(Value::array(vec![Value::string("test".to_string()), Value::integer(1)]))]
#[test_case({
    let mut o = indexmap::IndexMap::new();
     o.insert("key1".to_string(), Value::string("test".to_string()));
     o.insert("key2".to_string(), Value::integer(1));
     o.insert("nested".to_string(), Value::Object(o.clone()));
     Value::Object(o)
})]
#[test_case(Value::buffer(BYTES.clone()))]
#[test_case(Value::addon(BYTES.clone(), "ns::type"))]
fn it_serdes_values(value: Value) {
    let ser = serde_json::to_string(&value).unwrap();
    println!("\nserialized: {}", ser);
    let de: Value = serde_json::from_str(&ser).unwrap();
    println!("deserialized:  {:?}\n", de);
    assert_eq!(de, value);
}

#[test_case(json!({"type": "integer", "value": "1" }))]
#[test_case(json!({"type": "integer", "value": "18446744073709551615" }))]
#[test_case(json!({"type": "integer", "value": "-10" }))]
#[test_case(json!({"type": "float", "value": 1.12 }))]
#[test_case(json!({"type": "bool", "value": false }))]
#[test_case(json!({"type": "bool", "value": true }))]
#[test_case(json!({"type": "null"}))]
#[test_case(json!({"type":"buffer","value":"0xFFFFFF"}))]
fn it_deserializes_values(val: JsonValue) {
    let _: Value = serde_json::from_value(val.clone())
        .map_err(|e| format!("failed to deserialize value {}: {}", val, e))
        .unwrap();
}

#[test]
fn it_rejects_invalid_keys() {
    match serde_json::from_value::<Value>(json!({"type": "strin", "value": "my string"})) {
        Err(e) => {
            assert_eq!(&e.to_string(), "invalid type strin");
        }
        Ok(_) => panic!("missing expected error for invalid value key"),
    }
}

#[test_case("~/home/path", dirs::home_dir().unwrap().join("home/path").to_str().unwrap())]
#[test_case("/absolute/path", "/absolute/path")]
#[test_case("./relative/path", "/workspace/./relative/path"; "current directory")]
#[test_case("../relative/path", "/workspace/../relative/path"; "parent directory")]
fn test_auth_context_get_path_from_str(path_str: &str, expected: &str) {
    let auth_context = AuthorizationContext::new(FileLocation::from_path(
        Path::new("/workspace/txtx.yml").to_path_buf(),
    ));
    let result = auth_context.get_file_location_from_path_buf(&PathBuf::from(path_str)).unwrap();
    assert_eq!(result.to_string(), expected);
}

// =============================================================================
// Type::Null and Type::TypedNull serialization/deserialization tests
// =============================================================================

/// Test that Type serialization produces expected string format
#[test_case(Type::null(), "null"; "untyped null")]
#[test_case(Type::typed_null(Type::String), "null<string>"; "typed null string")]
#[test_case(Type::typed_null(Type::Integer), "null<integer>"; "typed null integer")]
#[test_case(Type::typed_null(Type::Bool), "null<bool>"; "typed null bool")]
#[test_case(Type::typed_null(Type::Array(Box::new(Type::Integer))), "null<array[integer]>"; "typed null array")]
#[test_case(Type::typed_null(Type::typed_null(Type::String)), "null<null<string>>"; "nested typed null")]
#[test_case(Type::Array(Box::new(Type::typed_null(Type::String))), "array[null<string>]"; "array of typed null")]
fn test_type_null_to_string_format(typ: Type, expected_str: &str) {
    assert_eq!(typ.to_string(), expected_str);
}

/// Test that Type string parsing produces correct types
#[test_case("null", Type::null(); "parse untyped null")]
#[test_case("null<string>", Type::typed_null(Type::String); "parse typed null string")]
#[test_case("null<integer>", Type::typed_null(Type::Integer); "parse typed null integer")]
#[test_case("null<array[integer]>", Type::typed_null(Type::Array(Box::new(Type::Integer))); "parse typed null array")]
#[test_case("array[null<string>]", Type::Array(Box::new(Type::typed_null(Type::String))); "parse array of typed null")]
#[test_case("array[string]", Type::Array(Box::new(Type::String)); "parse array string")]
fn test_type_null_parsing(input: &str, expected: Type) {
    let parsed = Type::try_from(input.to_string()).unwrap();
    assert_eq!(parsed, expected);
}

/// Test full serde roundtrip for Type (serialize to JSON, deserialize back)
#[test_case(Type::null(); "roundtrip untyped null")]
#[test_case(Type::typed_null(Type::String); "roundtrip typed null string")]
#[test_case(Type::typed_null(Type::Array(Box::new(Type::String))); "roundtrip typed null array")]
#[test_case(Type::typed_null(Type::typed_null(Type::Integer)); "roundtrip nested typed null")]
#[test_case(Type::Array(Box::new(Type::typed_null(Type::String))); "roundtrip array of typed null")]
fn test_type_null_serde_roundtrip(typ: Type) {
    let serialized = serde_json::to_string(&typ).unwrap();
    let deserialized: Type = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized, typ);
}

/// Test that Type::Null acts as wildcard in type compatibility matching
#[test]
fn test_type_null_wildcard_pattern() {
    // Type::Array(Box::new(Type::Null)) is used as "any array" pattern
    let any_array_pattern = Type::Array(Box::new(Type::Null));
    let string_array = Type::Array(Box::new(Type::String));
    let int_array = Type::Array(Box::new(Type::Integer));

    // The pattern match used in type_compatibility.rs
    fn is_any_array_pattern(t: &Type) -> bool {
        matches!(t, Type::Array(inner) if matches!(**inner, Type::Null | Type::TypedNull(_)))
    }

    assert!(is_any_array_pattern(&any_array_pattern));
    assert!(!is_any_array_pattern(&string_array));
    assert!(!is_any_array_pattern(&int_array));
}

// =============================================================================
// Deep nesting tests
// =============================================================================

/// Test deeply nested array types
#[test_case("array[array[string]]",
    Type::Array(Box::new(Type::Array(Box::new(Type::String))));
    "two level array nesting")]
#[test_case("array[array[array[integer]]]",
    Type::Array(Box::new(Type::Array(Box::new(Type::Array(Box::new(Type::Integer))))));
    "three level array nesting")]
fn test_deep_array_nesting(input: &str, expected: Type) {
    let parsed = Type::try_from(input.to_string()).unwrap();
    assert_eq!(parsed, expected);
    // Verify roundtrip
    assert_eq!(parsed.to_string(), input);
}

/// Test deeply nested null types
#[test_case("null<null<null<string>>>",
    Type::typed_null(Type::typed_null(Type::typed_null(Type::String)));
    "three level null nesting")]
fn test_deep_null_nesting(input: &str, expected: Type) {
    let parsed = Type::try_from(input.to_string()).unwrap();
    assert_eq!(parsed, expected);
    // Verify roundtrip
    assert_eq!(parsed.to_string(), input);
}

/// Test cross-nested types (array containing null, null containing array)
#[test_case("array[array[null<integer>]]",
    Type::Array(Box::new(Type::Array(Box::new(Type::typed_null(Type::Integer)))));
    "array of array of typed null")]
#[test_case("null<null<array[string]>>",
    Type::typed_null(Type::typed_null(Type::Array(Box::new(Type::String))));
    "null of null of array")]
#[test_case("array[null<array[null<string>]>]",
    Type::Array(Box::new(Type::typed_null(Type::Array(Box::new(Type::typed_null(Type::String))))));
    "alternating array and null nesting")]
fn test_cross_nesting(input: &str, expected: Type) {
    let parsed = Type::try_from(input.to_string()).unwrap();
    assert_eq!(parsed, expected);
    // Verify roundtrip
    assert_eq!(parsed.to_string(), input);
}

// =============================================================================
// Error handling tests
// =============================================================================

/// Test that empty inner types produce clear errors
#[test_case("null<>", "empty inner type"; "empty null inner")]
#[test_case("array[]", "empty inner type"; "empty array inner")]
#[test_case("addon()", "empty addon id"; "empty addon id")]
fn test_empty_inner_type_errors(input: &str, expected_error_contains: &str) {
    let result = Type::try_from(input.to_string());
    assert!(result.is_err(), "Expected error for input: {}", input);
    let err = result.unwrap_err();
    assert!(
        err.contains(expected_error_contains),
        "Error '{}' should contain '{}'",
        err,
        expected_error_contains
    );
}

/// Test that invalid inner types produce contextual errors
#[test_case("null<invalid_type>", "invalid type in null<invalid_type>"; "invalid null inner")]
#[test_case("array[bad_type]", "invalid type in array[bad_type]"; "invalid array inner")]
fn test_invalid_inner_type_errors(input: &str, expected_error_contains: &str) {
    let result = Type::try_from(input.to_string());
    assert!(result.is_err(), "Expected error for input: {}", input);
    let err = result.unwrap_err();
    assert!(
        err.contains(expected_error_contains),
        "Error '{}' should contain '{}'",
        err,
        expected_error_contains
    );
}

/// Test that malformed type strings produce errors
#[test_case("null<string"; "unclosed null angle bracket")]
#[test_case("array[string"; "unclosed array bracket")]
#[test_case("null<>string>"; "malformed null")]
#[test_case("unknown_type"; "completely unknown type")]
fn test_malformed_type_errors(input: &str) {
    let result = Type::try_from(input.to_string());
    assert!(result.is_err(), "Expected error for malformed input: {}", input);
}

/// Test error propagation through nested parsing
#[test]
fn test_nested_error_propagation() {
    // Error in deeply nested type should propagate with context
    let result = Type::try_from("array[null<invalid>]".to_string());
    assert!(result.is_err());
    let err = result.unwrap_err();
    // Should contain context about where the error occurred
    assert!(err.contains("array["), "Error should mention outer array context");
    assert!(err.contains("invalid"), "Error should mention the invalid type");
}
