use std::path::Path;
use std::path::PathBuf;

use crate::helpers::fs::FileLocation;
use crate::types::AuthorizationContext;

use super::types::Value;
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

#[cfg(feature = "dirs")]
#[test_case("~/home/path", dirs::home_dir().unwrap().join("home/path").to_str().unwrap())]
fn test_auth_context_get_path_from_str_home(path_str: &str, expected: &str) {
    let auth_context = AuthorizationContext::new(FileLocation::from_path(
        Path::new("/workspace/txtx.yml").to_path_buf(),
    ));
    let result = auth_context.get_file_location_from_path_buf(&PathBuf::from(path_str)).unwrap();
    assert_eq!(result.to_string(), expected);
}

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
