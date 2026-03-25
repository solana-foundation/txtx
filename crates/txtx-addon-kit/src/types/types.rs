use hcl_edit::expr::Expression;
use hcl_edit::structure::Block;
use indexmap::IndexMap;
use jaq_interpret::Val;
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value as JsonValue};
use std::collections::VecDeque;
use std::fmt::{self, Debug};
use strum::Display as StrumDisplay;

use crate::helpers::hcl::{
    collect_constructs_references_from_block, collect_constructs_references_from_expression,
    visit_optional_untyped_attribute,
};
use crate::types::frontend::{LogDetails, LogEvent, StaticLogEvent};
use crate::types::ConstructDid;

use super::diagnostics::Diagnostic;
use super::{Did, EvaluatableInput};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum Value {
    Bool(bool),
    Null,
    #[serde(serialize_with = "i128_serializer")]
    Integer(i128),
    Float(f64),
    String(String),
    Array(Box<Vec<Value>>),
    Object(IndexMap<String, Value>),
    #[serde(serialize_with = "hex_serializer")]
    Buffer(Vec<u8>),
    #[serde(serialize_with = "addon_serializer")]
    #[serde(untagged)]
    Addon(AddonData),
}

impl PartialEq<Value> for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Bool(lhs), Value::Bool(rhs)) => lhs == rhs,
            (Value::Null, Value::Null) => true,
            (Value::Integer(lhs), Value::Integer(rhs)) => lhs == rhs,
            (Value::Float(lhs), Value::Float(rhs)) => lhs == rhs,
            (Value::String(lhs), Value::String(rhs)) => lhs == rhs,
            (Value::Buffer(lhs), Value::Buffer(rhs)) => lhs == rhs,
            (Value::Object(lhs), Value::Object(rhs)) => {
                if lhs.len() != rhs.len() {
                    return false;
                }
                for (k, v) in lhs.iter() {
                    if let Some(r) = rhs.get(k) {
                        if v != r {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                true
            }
            (Value::Array(lhs), Value::Array(rhs)) => {
                if lhs.len() != rhs.len() {
                    return false;
                }
                for (l, r) in lhs.iter().zip(rhs.iter()) {
                    if l != r {
                        return false;
                    }
                }
                true
            }
            (Value::Addon(lhs), Value::Addon(rhs)) => {
                if lhs.id != rhs.id {
                    return false;
                }
                lhs.bytes == rhs.bytes
            }
            _ => false,
        }
    }
}

fn i128_serializer<S>(value: &i128, ser: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    ser.serialize_str(&value.to_string())
}

fn hex_serializer<S>(bytes: &Vec<u8>, ser: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let value = format!("0x{}", hex::encode(&bytes));
    ser.serialize_str(&value)
}

fn addon_serializer<S>(addon_data: &AddonData, ser: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut map = ser.serialize_map(Some(2))?;
    map.serialize_entry("type", &addon_data.id)?;
    let value = format!("0x{}", hex::encode(&addon_data.bytes));
    map.serialize_entry("value", &value)?;
    map.end()
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ValueVisitor;

        fn decode_hex_string(value: String) -> Result<Vec<u8>, String> {
            let value = value.replace("0x", "");
            hex::decode(&value)
                .map_err(|e| format!("failed to decode hex string ({}) to bytes: {}", value, e))
        }
        impl<'de> Visitor<'de> for ValueVisitor {
            type Value = Value;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a valid Value")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut typing = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "type" => {
                            if typing.is_some() {
                                return Err(de::Error::duplicate_field("type"));
                            }
                            let the_typing = map.next_value::<String>()?;
                            if the_typing.eq("null") {
                                return Ok(Value::null());
                            }
                            typing = Some(the_typing);
                        }
                        "value" => {
                            if typing.is_none() {
                                let Some(key) = map.next_key::<String>()? else {
                                    return Err(de::Error::missing_field("type"));
                                };
                                match key.as_str() {
                                    "type" => {
                                        let the_typing = map.next_value::<String>()?;
                                        if the_typing.eq("null") {
                                            return Ok(Value::null());
                                        }
                                        typing = Some(the_typing);
                                    }
                                    unexpected => {
                                        return Err(de::Error::custom(format!(
                                            "invalid Value: unexpected key {unexpected}"
                                        )))
                                    }
                                };
                            }
                            let typing = typing.ok_or_else(|| de::Error::missing_field("type"))?;
                            match typing.as_str() {
                                "bool" => return Ok(Value::bool(map.next_value()?)),
                                "integer" => {
                                    let value: String = map.next_value()?;
                                    let i128 = value.parse().map_err(serde::de::Error::custom)?;
                                    return Ok(Value::integer(i128));
                                }
                                "float" => return Ok(Value::float(map.next_value()?)),
                                "string" => return Ok(Value::string(map.next_value()?)),
                                "null" => unreachable!(),
                                "buffer" => {
                                    let bytes =
                                        decode_hex_string(map.next_value()?).map_err(|e| {
                                            de::Error::custom(format!(
                                                "failed to deserialize buffer: {e}"
                                            ))
                                        })?;
                                    return Ok(Value::buffer(bytes));
                                }
                                "object" => return Ok(Value::object(map.next_value()?)),

                                "array" => return Ok(Value::array(map.next_value()?)),
                                other => {
                                    if other.contains("::") {
                                        let bytes =
                                            decode_hex_string(map.next_value()?).map_err(|e| {
                                                de::Error::custom(format!(
                                                    "failed to deserialize buffer: {e}"
                                                ))
                                            })?;
                                        return Ok(Value::addon(bytes, other));
                                    } else {
                                        return Err(de::Error::custom(format!(
                                            "invalid type {other}"
                                        )));
                                    }
                                }
                            }
                        }
                        unexpected => {
                            return Err(de::Error::custom(format!(
                                "invalid Value: unexpected key {unexpected}"
                            )));
                        }
                    }
                }

                Err(de::Error::custom("invalid Value: missing required key value"))
            }
        }

        deserializer.deserialize_any(ValueVisitor)
    }
}

pub type AddonJsonConverter<'a> = Box<dyn Fn(&Value) -> Result<Option<JsonValue>, Diagnostic> + 'a>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunbookCompleteAdditionalInfo {
    /// The construct did that created this info
    pub construct_did: ConstructDid,
    /// The name of the construct that created this info
    pub construct_name: String,
    /// The title of the info
    pub title: String,
    /// The markdown details of the info
    pub details: String,
}

impl RunbookCompleteAdditionalInfo {
    pub const ADDON_ID: &str = "std::runbook_complete_additional_info";

    pub fn new(
        construct_did: &ConstructDid,
        construct_name: impl ToString,
        title: impl ToString,
        details: impl ToString,
    ) -> Self {
        Self {
            construct_did: construct_did.clone(),
            construct_name: construct_name.to_string(),
            title: title.to_string(),
            details: details.to_string(),
        }
    }
}

impl Into<Vec<LogEvent>> for RunbookCompleteAdditionalInfo {
    fn into(self) -> Vec<LogEvent> {
        self.details
            .split("\n")
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(LogEvent::Static(StaticLogEvent {
                        level: super::frontend::LogLevel::Info,
                        details: LogDetails {
                            message: trimmed.to_string(),
                            summary: self.title.clone(),
                        },
                        uuid: self.construct_did.as_uuid(),
                        namespace: self.construct_name.clone(),
                    }))
                }
            })
            .collect()
    }
}

impl ToFromValue for RunbookCompleteAdditionalInfo {
    fn to_value(&self) -> Value {
        let serialized = serde_json::to_vec(self).unwrap();
        Value::addon(serialized, RunbookCompleteAdditionalInfo::ADDON_ID)
    }
    fn from_value(value: &Value) -> Self {
        let AddonData { bytes, id } = value.as_addon_data().unwrap();
        if id != RunbookCompleteAdditionalInfo::ADDON_ID {
            panic!("Value is not a RunbookCompleteAdditionalInfo");
        }
        serde_json::from_slice(bytes).unwrap()
    }
}

pub trait ToFromValue {
    fn to_value(&self) -> Value;
    fn from_value(value: &Value) -> Self;
}

impl ToFromValue for Vec<RunbookCompleteAdditionalInfo> {
    fn to_value(&self) -> Value {
        let serialized = serde_json::to_vec(self).unwrap();
        Value::addon(serialized, RunbookCompleteAdditionalInfo::ADDON_ID)
    }
    fn from_value(value: &Value) -> Self {
        let AddonData { bytes, id } = value.as_addon_data().unwrap();
        if id != RunbookCompleteAdditionalInfo::ADDON_ID {
            panic!("Value is not a RunbookCompleteAdditionalInfo");
        }
        serde_json::from_slice(bytes).unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ThirdPartySignatureStatus {
    Initialized,
    Submitted,
    CheckRequested,
    Approved,
    Rejected,
}
impl ThirdPartySignatureStatus {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            ThirdPartySignatureStatus::Initialized => vec![0],
            ThirdPartySignatureStatus::Submitted => vec![1],
            ThirdPartySignatureStatus::CheckRequested => vec![2],
            ThirdPartySignatureStatus::Approved => vec![3],
            ThirdPartySignatureStatus::Rejected => vec![4],
        }
    }
    pub fn from_bytes(bytes: &[u8]) -> Self {
        match bytes {
            [0] => ThirdPartySignatureStatus::Initialized,
            [1] => ThirdPartySignatureStatus::Submitted,
            [2] => ThirdPartySignatureStatus::CheckRequested,
            [3] => ThirdPartySignatureStatus::Approved,
            [4] => ThirdPartySignatureStatus::Rejected,
            _ => panic!("Invalid bytes for ThirdPartySignatureStatus: {:?}", bytes),
        }
    }
    pub fn is_approved(&self) -> bool {
        matches!(self, ThirdPartySignatureStatus::Approved)
    }
    pub fn is_submitted(&self) -> bool {
        matches!(self, ThirdPartySignatureStatus::Submitted)
    }
    pub fn is_check_requested(&self) -> bool {
        matches!(self, ThirdPartySignatureStatus::CheckRequested)
    }
}
pub const THIRD_PARTY_SIGNATURE: &str = "std::third_party_signature";

impl Value {
    pub fn third_party_signature_initialized() -> Self {
        Value::addon(ThirdPartySignatureStatus::Initialized.to_bytes(), THIRD_PARTY_SIGNATURE)
    }

    pub fn third_party_signature_submitted() -> Self {
        Value::addon(ThirdPartySignatureStatus::Submitted.to_bytes(), THIRD_PARTY_SIGNATURE)
    }

    pub fn third_party_signature_approved() -> Self {
        Value::addon(ThirdPartySignatureStatus::Approved.to_bytes(), THIRD_PARTY_SIGNATURE)
    }

    pub fn third_party_signature_rejected() -> Self {
        Value::addon(ThirdPartySignatureStatus::Rejected.to_bytes(), THIRD_PARTY_SIGNATURE)
    }

    pub fn third_party_signature_check_requested() -> Self {
        Value::addon(ThirdPartySignatureStatus::CheckRequested.to_bytes(), THIRD_PARTY_SIGNATURE)
    }

    pub fn expect_third_party_signature(&self) -> ThirdPartySignatureStatus {
        if let Value::Addon(addon_data) = self {
            if addon_data.id == THIRD_PARTY_SIGNATURE {
                return ThirdPartySignatureStatus::from_bytes(&addon_data.bytes);
            }
        }
        panic!("Value is not a third party signature");
    }

    pub fn as_third_party_signature_status(&self) -> Option<ThirdPartySignatureStatus> {
        if let Value::Addon(addon_data) = self {
            if addon_data.id == THIRD_PARTY_SIGNATURE {
                return Some(ThirdPartySignatureStatus::from_bytes(&addon_data.bytes));
            }
        }
        None
    }

    pub fn as_runbook_complete_additional_info(&self) -> Option<RunbookCompleteAdditionalInfo> {
        if let Value::Addon(addon_data) = self {
            if addon_data.id == RunbookCompleteAdditionalInfo::ADDON_ID {
                return Some(RunbookCompleteAdditionalInfo::from_value(&self));
            }
        }
        None
    }
}

impl Value {
    pub fn string(value: String) -> Value {
        Value::String(value.to_string())
    }
    pub fn integer(value: i128) -> Value {
        Value::Integer(value)
    }
    pub fn float(value: f64) -> Value {
        Value::Float(value)
    }
    pub fn null() -> Value {
        Value::Null
    }
    pub fn bool(value: bool) -> Value {
        Value::Bool(value)
    }
    pub fn buffer(bytes: Vec<u8>) -> Value {
        Value::Buffer(bytes)
    }
    pub fn array(array: Vec<Value>) -> Value {
        Value::Array(Box::new(array))
    }
    pub fn object(object: IndexMap<String, Value>) -> Value {
        Value::Object(object)
    }

    pub fn addon(bytes: Vec<u8>, id: &str) -> Value {
        Value::Addon(AddonData { bytes, id: id.to_string() })
    }

    pub fn expect_string(&self) -> &str {
        match &self {
            Value::String(value) => value,
            _ => unreachable!(),
        }
    }
    pub fn expect_integer(&self) -> i128 {
        match &self {
            Value::Integer(value) => *value,
            _ => unreachable!(),
        }
    }
    pub fn expect_uint(&self) -> Result<u64, String> {
        match &self {
            Value::Integer(value) => i128_to_u64(*value),
            _ => unreachable!(),
        }
    }
    pub fn expect_float(&self) -> f64 {
        match &self {
            Value::Float(value) => *value,
            _ => unreachable!(),
        }
    }
    pub fn expect_null(&self) -> () {
        match &self {
            Value::Null => (),
            _ => unreachable!(),
        }
    }
    pub fn expect_bool(&self) -> bool {
        match &self {
            Value::Bool(value) => *value,
            _ => unreachable!(),
        }
    }
    pub fn expect_buffer_data(&self) -> &Vec<u8> {
        match &self {
            Value::Buffer(value) => &value,
            _ => unreachable!(),
        }
    }
    pub fn expect_addon_data(&self) -> &AddonData {
        match &self {
            Value::Addon(value) => &value,
            _ => unreachable!(),
        }
    }

    #[deprecated(note = "use `get_buffer_bytes_result` instead")]
    pub fn expect_buffer_bytes(&self) -> Vec<u8> {
        self.try_get_buffer_bytes_result().unwrap().expect("unable to retrieve bytes")
    }

    pub fn get_buffer_bytes_result(&self) -> Result<Vec<u8>, String> {
        self.try_get_buffer_bytes_result()?.ok_or("unable to retrieve bytes".into())
    }

    #[deprecated(note = "use `try_get_buffer_bytes_result` instead")]
    pub fn try_get_buffer_bytes(&self) -> Option<Vec<u8>> {
        let bytes = match &self {
            Value::Buffer(value) => value.clone(),
            Value::String(bytes) => {
                let bytes = if bytes.starts_with("0x") {
                    crate::hex::decode(&bytes[2..]).unwrap()
                } else {
                    crate::hex::decode(&bytes).unwrap()
                };
                bytes
            }
            Value::Array(values) => {
                values.iter().flat_map(|v| v.get_buffer_bytes_result().unwrap()).collect()
            }
            Value::Addon(addon_value) => addon_value.bytes.clone(),
            _ => return None,
        };

        Some(bytes)
    }

    pub fn try_get_buffer_bytes_result(&self) -> Result<Option<Vec<u8>>, String> {
        let bytes = match &self {
            Value::Buffer(value) => value.clone(),
            Value::String(bytes) => {
                let stripped = if bytes.starts_with("0x") { &bytes[2..] } else { &bytes[..] };
                let bytes = crate::hex::decode(stripped).map_err(|e| {
                    format!("string '{}' could not be decoded to hex bytes: {}", bytes, e)
                })?;
                bytes
            }
            Value::Array(values) => values
                .iter()
                .map(|v| v.try_get_buffer_bytes_result())
                .collect::<Result<Vec<Option<_>>, String>>()?
                .into_iter()
                .filter_map(|v| v)
                .flat_map(|v| v)
                .collect(),
            Value::Addon(addon_value) => addon_value.bytes.clone(),
            _ => return Ok(None),
        };

        Ok(Some(bytes))
    }
    pub fn expect_array(&self) -> &Box<Vec<Value>> {
        match &self {
            Value::Array(value) => value,
            _ => unreachable!(),
        }
    }

    pub fn expect_object(&self) -> &IndexMap<String, Value> {
        match &self {
            Value::Object(value) => value,
            _ => unreachable!(),
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match &self {
            Value::String(value) => Some(value),
            _ => None,
        }
    }
    pub fn as_integer(&self) -> Option<i128> {
        match &self {
            Value::Integer(value) => Some(*value),
            _ => None,
        }
    }
    pub fn as_uint(&self) -> Option<Result<u64, String>> {
        match &self {
            Value::Integer(value) => Some(i128_to_u64(*value)),
            _ => None,
        }
    }
    pub fn as_u8(&self) -> Option<Result<u8, String>> {
        match &self {
            Value::Integer(value) => {
                Some(u8::try_from(*value).map_err(|e| format!("invalid u8: {e}")))
            }
            _ => None,
        }
    }
    pub fn as_u16(&self) -> Option<Result<u16, String>> {
        match &self {
            Value::Integer(value) => {
                Some(u16::try_from(*value).map_err(|e| format!("invalid u16: {e}")))
            }
            _ => None,
        }
    }
    pub fn as_float(&self) -> Option<f64> {
        match &self {
            Value::Float(value) => Some(*value),
            _ => None,
        }
    }
    pub fn as_null(&self) -> Option<()> {
        match &self {
            Value::Null => Some(()),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match &self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }
    pub fn as_buffer_data(&self) -> Option<&Vec<u8>> {
        match &self {
            Value::Buffer(value) => Some(&value),
            _ => None,
        }
    }
    pub fn as_addon_data(&self) -> Option<&AddonData> {
        match &self {
            Value::Addon(value) => Some(&value),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&Box<Vec<Value>>> {
        match &self {
            Value::Array(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&Box<Vec<Value>>> {
        match &self {
            Value::Array(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&IndexMap<String, Value>> {
        match &self {
            Value::Object(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut IndexMap<String, Value>> {
        match self {
            Value::Object(value) => Some(value),
            _ => None,
        }
    }

    pub fn get_keys_from_object(&self, mut keys: VecDeque<String>) -> Result<Value, Diagnostic> {
        let Some(key) = keys.pop_front() else {
            return Ok(self.clone());
        };

        if let Some(ref object) = self.as_object() {
            match object.get(&key) {
                Some(val) => val.get_keys_from_object(keys),
                None => {
                    Err(Diagnostic::error_from_string(format!("missing key '{}' from object", key)))
                }
            }
        } else {
            Err(Diagnostic::error_from_string(format!("invalid key '{}' for object", key)))
        }
    }

    pub fn is_type_eq(&self, rhs: &Value) -> bool {
        match (self, rhs) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(_), Value::Bool(_)) => true,
            (Value::Integer(_), Value::Integer(_)) => true,
            (Value::Float(_), Value::Float(_)) => true,
            (Value::String(_), Value::String(_)) => true,
            (Value::Buffer(_), Value::Buffer(_)) => true,
            (Value::Object(_), Value::Object(_)) => true,
            (Value::Null, _) => false,
            (Value::Bool(_), _) => false,
            (Value::Integer(_), _) => false,
            (Value::Float(_), _) => false,
            (Value::String(_), _) => false,
            (Value::Buffer(_), _) => false,
            (Value::Object(_), _) => false,
            (Value::Array(lhs), Value::Array(rhs)) => {
                let Some(first_lhs) = lhs.first() else {
                    return false;
                };
                let Some(first_rhs) = rhs.first() else {
                    return false;
                };
                first_lhs.is_type_eq(first_rhs)
            }
            (Value::Array(_), _) => false,
            (Value::Addon(lhs), Value::Addon(rhs)) => lhs.id == rhs.id,
            (Value::Addon(_), _) => false,
        }
    }
    pub fn to_be_bytes(&self) -> Vec<u8> {
        match &self {
            Value::Buffer(bytes) => bytes.clone(),
            Value::Array(values) => {
                let mut joined = vec![];
                for value in values.iter() {
                    joined.extend(value.to_be_bytes());
                }
                joined
            }
            Value::String(bytes) => {
                let bytes = if bytes.starts_with("0x") {
                    crate::hex::decode(&bytes[2..]).unwrap()
                } else {
                    match crate::hex::decode(&bytes) {
                        Ok(res) => res,
                        Err(_) => bytes.as_bytes().to_vec(),
                    }
                };
                bytes
            }
            Value::Addon(data) => data.bytes.clone(),
            Value::Integer(value) => value.to_be_bytes().to_vec(),
            Value::Float(value) => value.to_be_bytes().to_vec(),
            Value::Bool(value) => vec![*value as u8],
            Value::Null => vec![],
            Value::Object(values) => {
                let mut joined = vec![];
                for (key, value) in values.iter() {
                    joined.extend(key.as_bytes());
                    joined.extend(value.to_be_bytes());
                }
                joined
            }
        }
    }

    pub fn to_le_bytes(&self) -> Vec<u8> {
        match &self {
            Value::Buffer(bytes) => bytes.clone(),
            Value::Array(values) => {
                let mut joined = vec![];
                for value in values.iter() {
                    joined.extend(value.to_le_bytes());
                }
                joined
            }
            Value::String(bytes) => {
                let bytes = if bytes.starts_with("0x") {
                    crate::hex::decode(&bytes[2..]).unwrap()
                } else {
                    match crate::hex::decode(&bytes) {
                        Ok(res) => res,
                        Err(_) => bytes.as_bytes().to_vec(),
                    }
                };
                bytes
            }
            Value::Addon(data) => data.bytes.clone(),
            Value::Integer(value) => value.to_le_bytes().to_vec(),
            Value::Float(value) => value.to_le_bytes().to_vec(),
            Value::Bool(value) => vec![*value as u8],
            Value::Null => vec![],
            Value::Object(values) => {
                let mut joined = vec![];
                for (key, value) in values.iter() {
                    joined.extend(key.as_bytes());
                    joined.extend(value.to_le_bytes());
                }
                joined
            }
        }
    }

    pub fn parse_and_default_to_string(value_str: &str) -> Value {
        let trim = value_str.trim();
        let value = match trim.parse::<i128>() {
            Ok(uint) => Value::integer(uint),
            Err(_) => {
                if trim.starts_with("[") || trim.starts_with("(") {
                    let values_to_parse = trim[1..trim.len() - 1].split(",").collect::<Vec<_>>();
                    let mut values = vec![];
                    for v in values_to_parse.iter() {
                        values.push(Value::parse_and_default_to_string(v));
                    }
                    Value::array(values)
                } else {
                    Value::string(trim.into())
                }
            }
        };
        value
    }

    pub fn compute_fingerprint(&self) -> Did {
        let bytes = self.to_be_bytes();
        Did::from_components(vec![bytes])
    }

    pub fn to_json(&self, addon_converters: Option<&Vec<AddonJsonConverter>>) -> JsonValue {
        let json = match self {
            Value::Bool(b) => JsonValue::Bool(*b),
            Value::Null => JsonValue::Null,
            Value::Integer(i) => JsonValue::Number(serde_json::Number::from(*i as i64)),
            Value::Float(f) => JsonValue::Number(serde_json::Number::from_f64(*f).unwrap()),
            Value::String(s) => JsonValue::String(s.to_string()),
            Value::Array(vec) => JsonValue::Array(
                vec.iter().map(|v| v.to_json(addon_converters)).collect::<Vec<JsonValue>>(),
            ),
            Value::Object(index_map) => JsonValue::Object(
                index_map
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_json(addon_converters)))
                    .collect::<Map<String, JsonValue>>(),
            ),
            Value::Buffer(vec) => JsonValue::String(format!("0x{}", hex::encode(&vec))),
            Value::Addon(addon_data) => {
                if let Some(addon_converters) = addon_converters.as_ref() {
                    let parsed_values = addon_converters
                        .iter()
                        .filter_map(|converter| converter(self).ok().flatten())
                        .collect::<Vec<_>>();
                    if let Some(parsed_value) = parsed_values.first() {
                        return parsed_value.clone();
                    }
                }
                JsonValue::String(addon_data.to_string())
            }
        };
        json
    }
}

fn i128_to_u64(i128: i128) -> Result<u64, String> {
    u64::try_from(i128).map_err(|e| format!("invalid uint: {e}"))
}
impl Value {
    pub fn to_string(&self) -> String {
        match self {
            Value::String(val) => val.clone(),
            Value::Bool(val) => val.to_string(),
            Value::Integer(val) => val.to_string(),
            Value::Float(val) => val.to_string(),
            Value::Null => "null".to_string(),
            Value::Buffer(bytes) => {
                format!("0x{}", hex::encode(&bytes))
            }
            Value::Object(obj) => {
                let mut res = "{".to_string();
                let len = obj.len();
                for (i, (k, v)) in obj.iter().enumerate() {
                    res.push_str(&format!(
                        r#""{}": {}{}"#,
                        k,
                        v.to_string(),
                        if i == (len - 1) { "" } else { "," }
                    ));
                }
                res
            }
            Value::Array(array) => {
                format!("[{}]", array.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", "))
            }
            Value::Addon(addon_value) => addon_value.to_string(),
        }
    }

    /// The same as [Value::to_string], but strings are wrapped in double quotes.
    /// This is useful for generating JSON-formatted strings.
    /// I don't know if there are side effects to the [Value::to_string] method having
    /// the double quoted strings, so I'm keeping this separate for now.
    pub fn encode_to_string(&self) -> String {
        match self {
            Value::String(val) => format!(r#""{val}""#),
            Value::Bool(val) => val.to_string(),
            Value::Integer(val) => val.to_string(),
            Value::Float(val) => val.to_string(),
            Value::Null => "null".to_string(),
            Value::Buffer(bytes) => {
                format!(r#""0x{}""#, hex::encode(&bytes))
            }
            Value::Object(obj) => {
                let mut res = "{".to_string();
                let len = obj.len();
                for (i, (k, v)) in obj.iter().enumerate() {
                    res.push_str(&format!(
                        r#"
    "{}": {}{}"#,
                        k,
                        v.encode_to_string(),
                        if i == (len - 1) { "" } else { "," }
                    ));
                }
                res.push_str(&format!(
                    r#"
}}"#
                ));
                res
            }
            Value::Array(array) => {
                format!(
                    "[{}]",
                    array.iter().map(|e| e.encode_to_string()).collect::<Vec<_>>().join(", ")
                )
            }
            Value::Addon(addon_value) => addon_value.encode_to_string(),
        }
    }

    pub fn from_jaq_value(value: &Val) -> Result<Value, String> {
        let res = match value {
            Val::Null => Value::null(),
            Val::Bool(val) => Value::bool(*val),
            Val::Num(val) => val
                .parse::<i128>()
                .map(Value::integer)
                .map_err(|e| format!("Failed to parse number: {}", e))?,
            Val::Int(val) => i128::try_from(*val)
                .map(Value::integer)
                .map_err(|e| format!("Failed to convert integer: {}", e))?,
            Val::Float(val) => Value::float(*val),
            Val::Str(val) => Value::string(val.to_string()),
            Val::Arr(val) => Value::array(
                val.iter()
                    .map(|v| Value::from_jaq_value(v))
                    .collect::<Result<Vec<Value>, String>>()?,
            ),
            Val::Obj(val) => ObjectType::from(
                val.iter()
                    .map(|(k, v)| Value::from_jaq_value(v).map(|v| (k.as_str(), v)))
                    .collect::<Result<Vec<(&str, Value)>, String>>()?,
            )
            .to_value(),
        };
        Ok(res)
    }
    pub fn get_type(&self) -> Type {
        match self {
            Value::Bool(_) => Type::Bool,
            Value::Null => Type::null(),
            Value::Integer(_) => Type::Integer,
            Value::Float(_) => Type::Float,
            Value::String(_) => Type::String,
            Value::Buffer(_) => Type::Buffer,
            Value::Object(_) => Type::Object(ObjectDefinition::arbitrary()),
            Value::Array(t) => {
                Type::Array(Box::new(t.first().unwrap_or(&Value::null()).get_type()))
            }
            Value::Addon(t) => Type::Addon(t.id.clone()),
        }
    }
}

// impl Serialize for PrimitiveValue {
//     fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
//     where
//         S: Serializer,
//     {
//         match self {
//             PrimitiveValue::String(val) => serializer.serialize_str(val),
//             PrimitiveValue::UnsignedInteger(val) => serializer.serialize_u64(*val),
//             PrimitiveValue::SignedInteger(val) => serializer.serialize_i64(*val),
//             PrimitiveValue::Float(val) => serializer.serialize_f64(*val),
//             PrimitiveValue::Bool(val) => serializer.serialize_bool(*val),
//             PrimitiveValue::Null => serializer.serialize_none(),
//             PrimitiveValue::Buffer(BufferData { bytes, typing: _ }) => {
//                 let mut s = String::from("0x");
//                 s.write_str(
//                     &bytes
//                         .iter()
//                         .map(|b| format!("{:02X}", b))
//                         .collect::<String>(),
//                 )
//                 .unwrap();
//                 serializer.serialize_str(&s)
//             }
//         }
//     }
// }

#[derive(Clone, Debug)]
pub struct ObjectType {
    map: IndexMap<String, Value>,
}
impl ObjectType {
    pub fn new() -> Self {
        ObjectType { map: IndexMap::new() }
    }

    pub fn from_map(map: IndexMap<String, Value>) -> Self {
        ObjectType { map }
    }

    pub fn from<S: ToString, T: IntoIterator<Item = (S, Value)>>(default: T) -> Self {
        let mut map = IndexMap::new();
        for (key, value) in default {
            map.insert(key.to_string(), value);
        }
        ObjectType { map }
    }

    pub fn insert(&mut self, key: &str, value: Value) -> &mut Self {
        self.map.insert(key.to_string(), value);
        self
    }

    pub fn inner(&self) -> IndexMap<String, Value> {
        self.map.clone()
    }
    pub fn to_value(&self) -> Value {
        Value::object(self.map.clone())
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct AddonData {
    pub bytes: Vec<u8>,
    pub id: String,
}
impl AddonData {
    pub fn to_string(&self) -> String {
        format!("0x{}", hex::encode(&self.bytes))
    }
    pub fn encode_to_string(&self) -> String {
        format!(r#""0x{}""#, hex::encode(&self.bytes))
    }
}

impl fmt::Debug for AddonData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // You can customize the output format here.
        f.debug_struct("AddonData").field("bytes", &self.to_string()).field("id", &self.id).finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, StrumDisplay)]
pub enum Type {
    #[strum(serialize = "bool")]
    Bool,
    #[strum(serialize = "null")]
    Null,
    #[strum(to_string = "null<{0}>")]
    TypedNull(Box<Type>),
    #[strum(serialize = "integer")]
    Integer,
    #[strum(serialize = "float")]
    Float,
    #[strum(serialize = "string")]
    String,
    #[strum(serialize = "buffer")]
    Buffer,
    #[strum(to_string = "object")]
    Object(ObjectDefinition),
    #[strum(to_string = "addon({0})")]
    Addon(String),
    #[strum(to_string = "array[{0}]")]
    Array(Box<Type>),
    #[strum(to_string = "map")]
    Map(ObjectDefinition),
}

impl Type {
    pub fn string() -> Type {
        Type::String
    }
    pub fn integer() -> Type {
        Type::Integer
    }
    pub fn float() -> Type {
        Type::Float
    }
    pub fn null() -> Type {
        Type::Null
    }
    pub fn typed_null(inner: Type) -> Type {
        Type::TypedNull(Box::new(inner))
    }
    pub fn bool() -> Type {
        Type::Bool
    }
    pub fn object(def: ObjectDefinition) -> Type {
        Type::Object(def)
    }
    pub fn strict_object(props: Vec<ObjectProperty>) -> Type {
        Type::Object(ObjectDefinition::strict(props))
    }
    pub fn arbitrary_object() -> Type {
        Type::Object(ObjectDefinition::arbitrary())
    }
    pub fn documented_arbitrary_object(props: Vec<ObjectProperty>) -> Type {
        Type::Object(ObjectDefinition::documented_arbitrary(props))
    }
    pub fn map(def: ObjectDefinition) -> Type {
        Type::Map(def)
    }
    pub fn strict_map(props: Vec<ObjectProperty>) -> Type {
        Type::Map(ObjectDefinition::strict(props))
    }
    pub fn arbitrary_map() -> Type {
        Type::Map(ObjectDefinition::arbitrary())
    }
    pub fn documented_arbitrary_map(props: Vec<ObjectProperty>) -> Type {
        Type::Map(ObjectDefinition::documented_arbitrary(props))
    }
    pub fn buffer() -> Type {
        Type::Buffer
    }
    pub fn addon(id: &str) -> Type {
        Type::Addon(id.to_string())
    }
    pub fn array(array_item_type: Type) -> Type {
        Type::Array(Box::new(array_item_type))
    }

    pub fn check_value(&self, value: &Value) -> Result<(), Diagnostic> {
        let mismatch_err = |expected: &str| {
            Diagnostic::error_from_string(format!(
                "expected {}, got {}",
                expected,
                value.get_type().to_string()
            ))
        };

        match &self {
            Type::Bool => value.as_bool().map(|_| ()).ok_or_else(|| mismatch_err("bool"))?,
            Type::Null => {
                value.as_null().map(|_| ()).ok_or_else(|| mismatch_err("null"))?
            }
            Type::TypedNull(inner) => {
                value
                    .as_null()
                    .map(|_| ())
                    .ok_or_else(|| mismatch_err(&format!("null<{}>", inner)))?
            }
            Type::Integer => {
                value.as_integer().map(|_| ()).ok_or_else(|| mismatch_err("integer"))?
            }
            Type::Float => value.as_float().map(|_| ()).ok_or_else(|| mismatch_err("float"))?,
            Type::String => value.as_string().map(|_| ()).ok_or_else(|| mismatch_err("string"))?,
            Type::Buffer => {
                value.as_buffer_data().map(|_| ()).ok_or_else(|| mismatch_err("buffer"))?
            }
            Type::Addon(addon_type) => value
                .as_addon_data()
                .map(|_| ())
                .ok_or_else(|| mismatch_err(&format!("addon type '{}'", addon_type)))?,
            Type::Array(array_type) => value
                .as_array()
                .map(|_| ())
                .ok_or_else(|| mismatch_err(&format!("array<{}>", array_type.to_string())))?,
            Type::Object(object_def) | Type::Map(object_def) => match object_def {
                ObjectDefinition::Strict(props) => {
                    let object = value.as_object().ok_or_else(|| mismatch_err("object"))?;
                    for expected_prop in props.iter() {
                        let prop_value = object.get(&expected_prop.name);
                        if expected_prop.optional && prop_value.is_none() {
                            continue;
                        }
                        let prop_value = prop_value.ok_or_else(|| {
                            Diagnostic::error_from_string(format!(
                                "missing required property '{}'",
                                expected_prop.name,
                            ))
                        })?;
                        expected_prop.typing.check_value(prop_value).map_err(|e| {
                            Diagnostic::error_from_string(format!(
                                "object property '{}': {}",
                                expected_prop.name, e.message
                            ))
                        })?;
                    }
                }
                ObjectDefinition::Arbitrary(_) => {
                    let _ = value.as_object().ok_or_else(|| mismatch_err("object"))?;
                }
                ObjectDefinition::Tuple(_) | ObjectDefinition::Enum(_) => {
                    unimplemented!("ObjectDefinition::Tuple and ObjectDefinition::Enum are not supported for runbook types");
                }
            }, //  => todo!(),
        };
        Ok(())
    }

    pub fn as_object(&self) -> Option<&ObjectDefinition> {
        match self {
            Type::Object(props) => Some(props),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Box<Type>> {
        match self {
            Type::Array(typing) => Some(typing),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&ObjectDefinition> {
        match self {
            Type::Map(props) => Some(props),
            _ => None,
        }
    }

    pub fn as_action(&self) -> Option<&String> {
        match self {
            Type::Addon(action) => Some(action),
            _ => None,
        }
    }

    /// This function will get attributes from the provided HCL block that match the input name.
    /// It will collect all expressions in the block that reference other constructs, according
    /// to the rules defined by the `Type`.
    ///
    /// For example, while most types will just get the attribute value, the `Object` and `Map` types
    /// need to look for nested blocks and properties.
    pub fn get_expressions_referencing_constructs<'a>(
        &self,
        block: &Block,
        input: Box<dyn EvaluatableInput>,
        dependencies: &mut Vec<(Option<Box<dyn EvaluatableInput>>, Expression)>,
    ) {
        let input_name = input.name();
        match self {
            Type::Map(ref object_def) => match object_def {
                ObjectDefinition::Strict(props) => {
                    for block in block.body.get_blocks(&input_name) {
                        for prop in props.iter() {
                            let res = visit_optional_untyped_attribute(&prop.name, &block);
                            if let Some(expr) = res {
                                collect_constructs_references_from_expression(
                                    &expr,
                                    Some(input.clone()),
                                    dependencies,
                                );
                            }
                        }
                    }
                }
                ObjectDefinition::Arbitrary(_) => {
                    for block in block.body.get_blocks(&input_name) {
                        collect_constructs_references_from_block(
                            block,
                            Some(input.clone()),
                            dependencies,
                        );
                    }
                }
                ObjectDefinition::Tuple(_) | ObjectDefinition::Enum(_) => {
                    unimplemented!("ObjectDefinition::Tuple and ObjectDefinition::Enum are not supported for runbook types");
                }
            },
            Type::Object(ref object_def) => {
                if let Some(expr) = visit_optional_untyped_attribute(&input_name, &block) {
                    collect_constructs_references_from_expression(
                        &expr,
                        Some(input.clone()),
                        dependencies,
                    );
                }
                match object_def {
                    ObjectDefinition::Strict(props) => {
                        for prop in props.iter() {
                            for block in block.body.get_blocks(&input_name) {
                                if let Some(expr) =
                                    visit_optional_untyped_attribute(&prop.name, &block)
                                {
                                    collect_constructs_references_from_expression(
                                        &expr,
                                        Some(input.clone()),
                                        dependencies,
                                    );
                                }
                            }
                        }
                    }
                    ObjectDefinition::Arbitrary(_) => {
                        for block in block.body.get_blocks(&input_name) {
                            collect_constructs_references_from_block(
                                block,
                                Some(input.clone()),
                                dependencies,
                            );
                        }
                    }
                    ObjectDefinition::Tuple(_) | ObjectDefinition::Enum(_) => {
                        unimplemented!("ObjectDefinition::Tuple and ObjectDefinition::Enum are not supported for runbook types");
                    }
                }
            }
            _ => {
                if let Some(expr) = visit_optional_untyped_attribute(&input_name, &block) {
                    collect_constructs_references_from_expression(&expr, Some(input), dependencies);
                }
            }
        }
    }
}

impl Default for Type {
    fn default() -> Self {
        Type::string()
    }
}
impl TryFrom<String> for Type {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let val = match value.as_str() {
            "string" => Type::String,
            "integer" => Type::Integer,
            "float" => Type::Float,
            "bool" => Type::Bool,
            "buffer" => Type::Buffer,
            "object" => Type::Object(ObjectDefinition::arbitrary()),
            other => {
                if other == "null" {
                    return Ok(Type::null());
                }
                if let Some(inner) = other.strip_prefix("null<").and_then(|s| s.strip_suffix(">"))
                {
                    if inner.is_empty() {
                        return Err("invalid type: null<> (empty inner type)".to_string());
                    }
                    let inner_type = Type::try_from(inner.to_string())
                        .map_err(|e| format!("invalid type in null<{}>: {}", inner, e))?;
                    return Ok(Type::typed_null(inner_type));
                } else if let Some(inner) =
                    other.strip_prefix("array[").and_then(|s| s.strip_suffix("]"))
                {
                    if inner.is_empty() {
                        return Err("invalid type: array[] (empty inner type)".to_string());
                    }
                    let inner_type = Type::try_from(inner.to_string())
                        .map_err(|e| format!("invalid type in array[{}]: {}", inner, e))?;
                    return Ok(Type::array(inner_type));
                } else if let Some(inner) =
                    other.strip_prefix("addon(").and_then(|s| s.strip_suffix(")"))
                {
                    if inner.is_empty() {
                        return Err("invalid type: addon() (empty addon id)".to_string());
                    }
                    Type::addon(inner)
                } else {
                    return Err(format!("invalid type: {}", other));
                }
            }
        };
        Ok(val)
    }
}

impl Serialize for Type {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Type {
    fn deserialize<D>(deserializer: D) -> Result<Type, D::Error>
    where
        D: Deserializer<'de>,
    {
        let type_str: String = serde::Deserialize::deserialize(deserializer)?;
        let t = Type::try_from(type_str).map_err(serde::de::Error::custom)?;
        Ok(t)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum ObjectDefinition {
    /// Strict object definition with a list of properties
    Strict(Vec<ObjectProperty>),
    /// Arbitrary object definition with no specific properties
    /// The optional list of object properties is used for documenting
    /// Some of the potential properties.
    Arbitrary(Option<Vec<ObjectProperty>>),
    /// Tuple variant for representing tuple types. This means that the 'name' field will be ignored.
    /// Instead, the index of the property will be used to create the tuple.
    Tuple(Vec<ObjectProperty>),
    /// Enum variant for representing enum types. This means that the value
    /// will have one of the specified properties.
    Enum(Vec<ObjectProperty>),
}

impl ObjectDefinition {
    pub fn strict(props: Vec<ObjectProperty>) -> Self {
        ObjectDefinition::Strict(props)
    }

    pub fn arbitrary() -> Self {
        ObjectDefinition::Arbitrary(None)
    }

    pub fn documented_arbitrary(props: Vec<ObjectProperty>) -> Self {
        ObjectDefinition::Arbitrary(Some(props))
    }

    pub fn tuple(props: Vec<ObjectProperty>) -> Self {
        ObjectDefinition::Tuple(props)
    }

    pub fn enum_type(props: Vec<ObjectProperty>) -> Self {
        ObjectDefinition::Enum(props)
    }

    pub fn join_documentation(&self, recursion_depth: usize) -> String {
        match self {
            ObjectDefinition::Strict(props) | ObjectDefinition::Arbitrary(Some(props)) => props
                .iter()
                .map(|prop| {
                    format!(
                        "{}- **{}**: {}",
                        " ".repeat((recursion_depth + 1) * 2),
                        prop.name,
                        prop.join_documentation(recursion_depth + 1)
                    )
                })
                .collect::<Vec<String>>()
                .join("\n"),
            ObjectDefinition::Arbitrary(None) => String::new(),
            _ => {
                // For Tuple and Enum, we don't have a specific documentation format
                // so we return an empty string.
                String::new()
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ObjectProperty {
    pub name: String,
    pub documentation: String,
    pub typing: Type,
    pub optional: bool,
    pub tainting: bool,
    pub internal: bool,
}

impl ObjectProperty {
    pub fn join_documentation(&self, recursion_depth: usize) -> String {
        match &self.typing {
            Type::Object(object_definition) => {
                format!(
                    "{} This is an object type containing the keys:\n{}",
                    self.documentation,
                    object_definition.join_documentation(recursion_depth)
                )
            }
            Type::Map(object_definition) => {
                format!(
                    "{} This is a map type containing the keys:\n{}",
                    self.documentation,
                    object_definition.join_documentation(recursion_depth)
                )
            }
            _ => self.documentation.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunbookSupervisionContext {
    pub review_input_default_values: bool,
    pub review_input_values: bool,
    pub is_supervised: bool,
}

impl RunbookSupervisionContext {
    pub fn new() -> Self {
        Self {
            review_input_default_values: false,
            review_input_values: false,
            is_supervised: false,
        }
    }
}
