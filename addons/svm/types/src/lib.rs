#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate txtx_addon_kit;

pub mod idl;

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction::Transaction;

use txtx_addon_kit::{
    hex,
    types::{
        diagnostics::Diagnostic,
        types::{AddonData, Type, Value},
    },
};

pub use anchor_lang_idl as anchor;

pub const SVM_TRANSACTION: &str = "svm::transaction";
pub const SVM_INSTRUCTION: &str = "svm::instruction";
pub const SVM_SIGNATURE: &str = "svm::signature";
pub const SVM_BINARY: &str = "svm::binary";
pub const SVM_IDL: &str = "svm::idl";
pub const SVM_KEYPAIR: &str = "svm::keypair";
pub const SVM_PUBKEY: &str = "svm::pubkey";
pub const SVM_TRANSACTION_WITH_KEYPAIRS: &str = "svm::transaction_with_keypairs";
pub const SVM_DEPLOYMENT_TRANSACTION: &str = "svm::deployment_transaction";
pub const SVM_CLOSE_TEMP_AUTHORITY_TRANSACTION_PARTS: &str =
    "svm::close_temp_authority_transaction_parts";
pub const SVM_PAYER_SIGNED_TRANSACTION: &str = "svm::payer_signed_transaction";
pub const SVM_AUTHORITY_SIGNED_TRANSACTION: &str = "svm::authority_signed_transaction";
pub const SVM_TEMP_AUTHORITY_SIGNED_TRANSACTION: &str = "svm::temp_authority_signed_transaction";
pub const SVM_SQUAD_MULTISIG: &str = "svm::squads_multisig";
pub const SVM_U8: &str = "svm::u8";
pub const SVM_U16: &str = "svm::u16";
pub const SVM_U32: &str = "svm::u32";
pub const SVM_U64: &str = "svm::u64";
pub const SVM_U128: &str = "svm::u128";
pub const SVM_U256: &str = "svm::u256";
pub const SVM_I8: &str = "svm::i8";
pub const SVM_I16: &str = "svm::i16";
pub const SVM_I32: &str = "svm::i32";
pub const SVM_I64: &str = "svm::i64";
pub const SVM_I128: &str = "svm::i128";
pub const SVM_I256: &str = "svm::i256";
pub const SVM_F32: &str = "svm::f32";
pub const SVM_F64: &str = "svm::f64";

use std::convert::TryFrom;
use std::fmt::Debug;

pub trait ValueNumber: Sized + Debug {
    const SVM_ID: &'static str;
    fn try_from_i128(i: i128) -> Result<Self, String>;
    fn from_le_bytes(bytes: &[u8]) -> Result<Self, String>;
}

macro_rules! impl_value_number {
    ($t:ty, $id:expr) => {
        impl ValueNumber for $t {
            const SVM_ID: &'static str = $id;

            fn try_from_i128(i: i128) -> Result<Self, String> {
                <$t>::try_from(i)
                    .map_err(|e| format!("could not convert value to {}: {:?}", stringify!($t), e))
            }

            fn from_le_bytes(bytes: &[u8]) -> Result<Self, String> {
                let arr: [u8; std::mem::size_of::<$t>()] = bytes.try_into().map_err(|e| {
                    format!("could not convert bytes to {}: {:?}", stringify!($t), e)
                })?;
                Ok(<$t>::from_le_bytes(arr))
            }
        }
    };
}

impl_value_number!(u8, SVM_U8);
impl_value_number!(u16, SVM_U16);
impl_value_number!(u32, SVM_U32);
impl_value_number!(u64, SVM_U64);
impl_value_number!(u128, SVM_U128);
impl_value_number!(i8, SVM_I8);
impl_value_number!(i16, SVM_I16);
impl_value_number!(i32, SVM_I32);
impl_value_number!(i64, SVM_I64);
impl_value_number!(i128, SVM_I128);
impl ValueNumber for f32 {
    const SVM_ID: &'static str = SVM_F32;
    fn try_from_i128(i: i128) -> Result<Self, String> {
        Ok(i as f32)
    }
    fn from_le_bytes(bytes: &[u8]) -> Result<Self, String> {
        let arr: [u8; 4] = bytes.try_into().map_err(|_| "Invalid bytes for f32".to_string())?;
        Ok(f32::from_le_bytes(arr))
    }
}

impl ValueNumber for f64 {
    const SVM_ID: &'static str = SVM_F64;
    fn try_from_i128(i: i128) -> Result<Self, String> {
        Ok(i as f64)
    }
    fn from_le_bytes(bytes: &[u8]) -> Result<Self, String> {
        let arr: [u8; 8] = bytes.try_into().map_err(|_| "Invalid bytes for f64".to_string())?;
        Ok(f64::from_le_bytes(arr))
    }
}
#[derive(Debug, Clone)]
pub struct U256(pub [u8; 32]);
impl ValueNumber for U256 {
    const SVM_ID: &'static str = SVM_U256;

    fn try_from_i128(_i: i128) -> Result<Self, String> {
        Err(format!("cannot convert i128 to {}", Self::SVM_ID))
    }

    fn from_le_bytes(bytes: &[u8]) -> Result<Self, String> {
        let arr: [u8; 32] = bytes.try_into().map_err(|_| "Invalid bytes for U256".to_string())?;
        Ok(U256(arr))
    }
}
#[derive(Debug, Clone)]
pub struct I256(pub [u8; 32]);
impl ValueNumber for I256 {
    const SVM_ID: &'static str = SVM_I256;

    fn try_from_i128(_i: i128) -> Result<Self, String> {
        Err(format!("cannot convert i128 to {}", Self::SVM_ID))
    }

    fn from_le_bytes(bytes: &[u8]) -> Result<Self, String> {
        let arr: [u8; 32] = bytes.try_into().map_err(|_| "Invalid bytes for I256".to_string())?;
        Ok(I256(arr))
    }
}

pub struct SvmValue {}

fn is_hex(str: &str) -> bool {
    decode_hex(str).map(|_| true).unwrap_or(false)
}

fn decode_hex(str: &str) -> Result<Vec<u8>, Diagnostic> {
    let stripped = if str.starts_with("0x") { &str[2..] } else { &str[..] };
    hex::decode(stripped)
        .map_err(|e| diagnosed_error!("string '{}' could not be decoded to hex bytes: {}", str, e))
}

impl SvmValue {
    pub fn to_json(value: &Value) -> Result<Option<serde_json::Value>, Diagnostic> {
        let Some(AddonData { id, .. }) = value.as_addon_data() else { return Ok(None) };

        match id.as_str() {
            SVM_PUBKEY => {
                let pubkey = Self::to_pubkey(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::String(pubkey.to_string())))
            }
            SVM_SIGNATURE => {
                let signature = Self::to_signature(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::String(signature.to_string())))
            }
            SVM_U8 => {
                let value = Self::to_number::<u8>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::Number(value.into())))
            }
            SVM_U16 => {
                let value = Self::to_number::<u16>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::Number(value.into())))
            }
            SVM_U32 => {
                let value = Self::to_number::<u32>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::Number(value.into())))
            }
            SVM_U64 => {
                let value = Self::to_number::<u64>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::Number(value.into())))
            }
            SVM_U128 => {
                let value = Self::to_number::<u128>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::String(value.to_string())))
            }
            SVM_U256 => {
                let value = Self::to_number::<U256>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::String(hex::encode(&value.0))))
            }
            SVM_I8 => {
                let value = Self::to_number::<i8>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::Number(value.into())))
            }
            SVM_I16 => {
                let value = Self::to_number::<i16>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::Number(value.into())))
            }
            SVM_I32 => {
                let value = Self::to_number::<i32>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::Number(value.into())))
            }
            SVM_I64 => {
                let value = Self::to_number::<i64>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::Number(value.into())))
            }
            SVM_I128 => {
                let value = Self::to_number::<i128>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::String(value.to_string())))
            }
            SVM_I256 => {
                let value = Self::to_number::<I256>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::String(hex::encode(&value.0))))
            }
            SVM_F32 => {
                let value = Self::to_number::<f32>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::String(value.to_string())))
            }
            SVM_F64 => {
                let value = Self::to_number::<f64>(value).map_err(Diagnostic::from)?;
                Ok(Some(serde_json::Value::String(value.to_string())))
            }
            _ => Ok(None),
        }
    }

    pub fn u8(value: u8) -> Value {
        Value::addon(value.to_le_bytes().to_vec(), SVM_U8)
    }

    pub fn u16(value: u16) -> Value {
        Value::addon(value.to_le_bytes().to_vec(), SVM_U16)
    }

    pub fn u32(value: u32) -> Value {
        Value::addon(value.to_le_bytes().to_vec(), SVM_U32)
    }

    pub fn u64(value: u64) -> Value {
        Value::addon(value.to_le_bytes().to_vec(), SVM_U64)
    }

    pub fn u128(value: u128) -> Value {
        Value::addon(value.to_le_bytes().to_vec(), SVM_U128)
    }

    pub fn u256(value: [u8; 32]) -> Value {
        Value::addon(value.to_vec(), SVM_U256)
    }

    pub fn i8(value: i8) -> Value {
        Value::addon(value.to_le_bytes().to_vec(), SVM_I8)
    }

    pub fn i16(value: i16) -> Value {
        Value::addon(value.to_le_bytes().to_vec(), SVM_I16)
    }

    pub fn i32(value: i32) -> Value {
        Value::addon(value.to_le_bytes().to_vec(), SVM_I32)
    }

    pub fn i64(value: i64) -> Value {
        Value::addon(value.to_le_bytes().to_vec(), SVM_I64)
    }

    pub fn i128(value: i128) -> Value {
        Value::addon(value.to_le_bytes().to_vec(), SVM_I128)
    }

    pub fn i256(value: [u8; 32]) -> Value {
        Value::addon(value.to_vec(), SVM_I256)
    }

    pub fn f32(value: f32) -> Value {
        Value::addon(value.to_le_bytes().to_vec(), SVM_F32)
    }

    pub fn f64(value: f64) -> Value {
        Value::addon(value.to_le_bytes().to_vec(), SVM_F64)
    }

    pub fn to_number<T: ValueNumber>(value: &Value) -> Result<T, String> {
        match value {
            Value::Integer(i) => T::try_from_i128(*i),
            Value::Addon(addon_data) => {
                if addon_data.id != T::SVM_ID {
                    return Err(format!(
                        "expected addon type {}, found {}",
                        T::SVM_ID,
                        addon_data.id
                    ));
                }
                T::from_le_bytes(&addon_data.bytes)
            }
            _ => Err(format!("expected {}, found {}", T::SVM_ID, value.get_type().to_string())),
        }
    }

    pub fn transaction_from_bytes(bytes: Vec<u8>) -> Value {
        Value::addon(bytes, SVM_TRANSACTION)
    }

    pub fn transaction(transaction: &Transaction) -> Result<Value, Diagnostic> {
        let bytes = serde_json::to_vec(&transaction)
            .map_err(|e| diagnosed_error!("failed to deserialize transaction: {e}"))?;
        Ok(Value::addon(bytes, SVM_TRANSACTION))
    }

    pub fn instruction(bytes: Vec<u8>) -> Value {
        Value::addon(bytes, SVM_INSTRUCTION)
    }

    pub fn signature(bytes: Vec<u8>) -> Value {
        Value::addon(bytes, SVM_SIGNATURE)
    }

    pub fn to_signature(value: &Value) -> Result<Signature, String> {
        match value.as_string() {
            Some(s) => {
                if is_hex(s) {
                    let hex = decode_hex(s).map_err(|e| e.message)?;
                    let bytes: [u8; 64] = hex[0..64]
                        .try_into()
                        .map_err(|e| format!("could not convert value to pubkey: {e}"))?;
                    return Ok(Signature::from(bytes));
                }
                return Signature::from_str(s)
                    .map_err(|e| format!("could not convert value to pubkey: {e}"));
            }
            None => {}
        };
        let bytes = value.to_be_bytes();
        let bytes: [u8; 64] = bytes[0..64]
            .try_into()
            .map_err(|e| format!("could not convert value to pubkey: {e}"))?;
        Ok(Signature::from(bytes))
    }

    pub fn binary(bytes: Vec<u8>) -> Value {
        Value::addon(bytes, SVM_BINARY)
    }

    pub fn idl(bytes: Vec<u8>) -> Value {
        Value::addon(bytes, SVM_IDL)
    }

    pub fn keypair(bytes: Vec<u8>) -> Value {
        Value::addon(bytes, SVM_KEYPAIR)
    }

    pub fn to_keypair(value: &Value) -> Result<Keypair, Diagnostic> {
        match value {
            Value::String(s) => {
                if is_hex(s) {
                    let hex = decode_hex(s).map_err(|e| e.message)?;
                    let bytes: [u8; 32] = hex[0..32]
                        .try_into()
                        .map_err(|e| format!("could not convert value to keypair: {e}"))?;
                    return Ok(Keypair::new_from_array(bytes));
                }
                let decoded = bs58::decode(s)
                    .into_vec()
                    .map_err(|e| diagnosed_error!("could not convert value to keypair: {e}"))?;

                if decoded.len() != 64 {
                    return Err(diagnosed_error!(
                        "could not convert value to keypair: decoded length must be 64; found {}",
                        decoded.len()
                    ));
                }
                let bytes: [u8; 32] = decoded[0..32]
                    .try_into()
                    .map_err(|e| diagnosed_error!("could not convert value to keypair: {e}"))?;
                let kp = Keypair::new_from_array(bytes);
                return Ok(kp);
            }
            Value::Array(values) => {
                let mut bytes = Vec::with_capacity(values.len());
                if values.len() != 64 {
                    return Err(diagnosed_error!(
                        "could not convert value to keypair: array length must be 64; found {}",
                        values.len()
                    ));
                }
                for v in values.iter() {
                    let b = v
                        .as_u8()
                        .ok_or_else(|| {
                            diagnosed_error!(
                                "could not convert value to keypair: array contains non-u8 value"
                            )
                        })?
                        .map_err(|e| diagnosed_error!("invalid u8 in keypair array: {e}"))?;
                    bytes.push(b);
                }
                let bytes: [u8; 32] = bytes[0..32]
                    .try_into()
                    .map_err(|e| diagnosed_error!("could not convert value to keypair: {e}"))?;
                return Ok(Keypair::new_from_array(bytes));
            }
            _ => {}
        }
        let bytes = value.to_be_bytes();
        let bytes: [u8; 32] = bytes[0..32]
            .try_into()
            .map_err(|e| diagnosed_error!("could not convert value to keypair: {e}"))?;

        Ok(Keypair::new_from_array(bytes))
    }

    pub fn pubkey(bytes: Vec<u8>) -> Value {
        Value::addon(bytes, SVM_PUBKEY)
    }

    pub fn to_pubkey(value: &Value) -> Result<Pubkey, String> {
        match value.as_string() {
            Some(s) => {
                if is_hex(s) {
                    let hex = decode_hex(s).map_err(|e| e.message)?;
                    let bytes: [u8; 32] = hex[0..32]
                        .try_into()
                        .map_err(|e| format!("could not convert value to pubkey: {e}"))?;
                    return Ok(Pubkey::new_from_array(bytes));
                }
                return Pubkey::from_str(s)
                    .map_err(|e| format!("could not convert value to pubkey: {e}"));
            }
            None => {}
        };
        let bytes = value.to_be_bytes();
        let bytes: [u8; 32] = bytes[0..32]
            .try_into()
            .map_err(|e| format!("could not convert value to pubkey: {e}"))?;
        Ok(Pubkey::new_from_array(bytes))
    }

    pub fn deployment_transaction(bytes: Vec<u8>) -> Value {
        Value::addon(bytes, SVM_DEPLOYMENT_TRANSACTION)
    }

    pub fn close_temp_authority_transaction_parts(bytes: Vec<u8>) -> Value {
        Value::addon(bytes, SVM_CLOSE_TEMP_AUTHORITY_TRANSACTION_PARTS)
    }

    pub fn squads_multisig(bytes: Vec<u8>) -> Value {
        Value::addon(bytes, SVM_SQUAD_MULTISIG)
    }
}

lazy_static! {
    pub static ref ANCHOR_PROGRAM_ARTIFACTS: Type = define_strict_object_type! {
        idl: {
            documentation: "The program idl.",
            // typing: Type::addon(SVM_IDL),
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        binary: {
            documentation: "The program binary.",
            typing: Type::addon(SVM_BINARY),
            optional: false,
            tainting: false
        },
        keypair: {
            documentation: "The program keypair.",
            typing: Type::addon(SVM_KEYPAIR),
            optional: false,
            tainting: true
        },
        program_id: {
            documentation: "The program id.",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        }
    };

    pub static ref CLASSIC_RUST_PROGRAM_ARTIFACTS: Type = define_strict_object_type! {
        idl: {
            documentation: "The program idl.",
            typing: Type::string(),
            optional: true,
            tainting: true
        },
        binary: {
            documentation: "The program binary.",
            typing: Type::addon(SVM_BINARY),
            optional: false,
            tainting: false
        },
        keypair: {
            documentation: "The program keypair.",
            typing: Type::addon(SVM_KEYPAIR),
            optional: false,
            tainting: true
        },
        program_id: {
            documentation: "The program id.",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        }
    };

    pub static ref PDA_RESULT: Type = define_strict_object_type! {
        pda: {
            documentation: "The program derived address.",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        },
        bump_seed: {
            documentation: "The bump seed.",
            typing: Type::integer(),
            optional: false,
            tainting: true
        }
    };

    pub static ref INSTRUCTION_TYPE: Type = define_documented_arbitrary_map_type! {
        raw_bytes: {
            documentation: "The serialized instruction bytes. Use this field in place of the other instructions if direct instruction bytes would like to be used.",
            typing: Type::addon(SVM_INSTRUCTION),
            optional: true,
            tainting: true
        },
        program_id: {
            documentation: "The SVM address of the program being invoked. If omitted, the program_id will be pulled from the idl.",
            typing: Type::string(),
            optional: true,
            tainting: true
        },
        program_idl: {
            documentation: "The IDL of the program being invoked.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        instruction_name: {
            documentation: "The name of the instruction being invoked.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        instruction_args: {
            documentation: "The arguments to the instruction being invoked.",
            typing: Type::array(Type::string()),
            optional: false,
            tainting: true
        }
    };

    pub static ref DEPLOYMENT_TRANSACTION_SIGNATURES: Type = define_strict_object_type! {
        create_temp_authority: {
            documentation: "The signature of the create temp authority transaction.",
            typing: Type::array(Type::string()),
            optional: false,
            tainting: true
        },
        create_buffer: {
            documentation: "The signature of the create buffer transaction.",
            typing: Type::array(Type::string()),
            optional: false,
            tainting: true
        },
        write_to_buffer: {
            documentation: "The signature of the write to buffer transaction.",
            typing: Type::array(Type::string()),
            optional: false,
            tainting: true
        },
        transfer_buffer_authority: {
            documentation: "The signature of the transfer buffer authority transaction.",
            typing: Type::array(Type::string()),
            optional: true,
            tainting: true
        },
        deploy_program: {
            documentation: "The signature of the deploy program transaction.",
            typing: Type::array(Type::string()),
            optional: true,
            tainting: true
        },
        upgrade_program: {
            documentation: "The signature of the upgrade program transaction.",
            typing: Type::array(Type::string()),
            optional: true,
            tainting: true
        },
        close_temp_authority: {
            documentation: "The signature of the close temp authority transaction.",
            typing: Type::array(Type::string()),
            optional: false,
            tainting: true
        }
    };

    pub static ref SUBGRAPH_EVENT: Type = define_strict_map_type! {
        name: {
            documentation: "The name of the event, as indexed by the IDL, whose occurrences should be added to the subgraph.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        field: {
            documentation: "A map of fields to index.",
            typing: SUBGRAPH_DEFINED_FIELD.clone(),
            optional: false,
            tainting: true
        },
        intrinsic_fields: {
            documentation: indoc!{r#"A map of intrinsic fields to index. For Event subgraphs, intrinsics are:
                - `slot`(indexed): The slot in which the event was emitted.
                - `transactionSignature`(indexed): The transaction signature in which the event was emitted."#},
            typing: Type::array(SUBGRAPH_INTRINSIC_FIELD.clone()),
            optional: true,
            tainting: true
        }
    };

    pub static ref PDA_ACCOUNT_SUBGRAPH: Type = define_strict_map_type! {
        type: {
            documentation: "The type field of the account, as indexed by the IDL. This type definition will be used to parse the PDA account data.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        instruction: {
            documentation: "An instruction that contains the account to index in the subgraph.",
            typing: PDA_ACCOUNT_INSTRUCTION_SUBGRAPH.clone(),
            optional: false,
            tainting: true
        },
        field: {
            documentation: "A map of fields to index.",
            typing: SUBGRAPH_DEFINED_FIELD.clone(),
            optional: false,
            tainting: true
        },
        intrinsic_fields: {
            documentation: indoc!{r#"A map of intrinsic fields to index. For PDA subgraphs, intrinsics are:
                - `slot`(indexed): The slot in which the event was emitted.
                - `pubkey`(indexed): The public key of the account.
                - `owner`(not indexed): The owner of the account.
                - `lamports`(not indexed): The lamports of the account."#},
            typing: Type::array(SUBGRAPH_INTRINSIC_FIELD.clone()),
            optional: true,
            tainting: true
        }
    };

    pub static ref TOKEN_ACCOUNT_SUBGRAPH: Type = define_strict_map_type! {
        instruction: {
            documentation: "An instruction that contains the account to index in the subgraph.",
            typing: TOKEN_ACCOUNT_INSTRUCTION_SUBGRAPH.clone(),
            optional: false,
            tainting: true
        },
        intrinsic_fields: {
            documentation: indoc!{r#"A map of fields to index that are intrinsic to token accounts. Token Account intrinsics are:
                - `slot`(indexed): The slot in which the instruction referencing the token account was invoked.
                - `transactionSignature`(indexed): The transaction signature in which the instruction referencing the token account was invoked.
                - `pubkey`(indexed): The public key of the token account (also known as the Associated Token Address).
                - `owner`(not indexed): The owner of the token account.
                - `mint`(not indexed): The mint of the token account.
                - `tokenProgram`(not indexed): The token program id.
                - `amount`(not indexed): A string representation of the amount of tokens in the account.
                - `decimals`(not indexed): The number of decimals for the token.
                - `uiAmount`(not indexed): The amount of tokens in the account, formatted as a number with the correct number of decimals.
                - `uiAmountString`(not indexed): The amount of tokens in the account, formatted as a string with the correct number of decimals.
                - `lamports`(not indexed): The lamports of the account."#},
            typing: Type::array(SUBGRAPH_INTRINSIC_FIELD.clone()),
            optional: true,
            tainting: true
        }
    };

    pub static ref SUBGRAPH_INTRINSIC_FIELD: Type = define_strict_map_type! {
        name: {
            documentation: "The name of the intrinsic field to index.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        description: {
            documentation: "A description of the field as it should appear in the subgraph schema.",
            typing: Type::string(),
            optional: true,
            tainting: false
        },
        display_name: {
            documentation: "The name of the field as it should appear in the subgraph schema. By default the intrinsic field name will be used.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        indexed: {
            documentation: "Whether this field should be indexed in the subgraph. If true, the field will be indexed and can be used as a filter in the subgraph. Sensible defaults are provided for intrinsic fields.",
            typing: Type::bool(),
            optional: false,
            tainting: true
        }
    };

    pub static ref PDA_ACCOUNT_INSTRUCTION_SUBGRAPH: Type = define_strict_map_type! {
        name: {
            documentation: "The name of the instruction that contains the account to index in the subgraph.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        account_name: {
            documentation: "The name of the account in the instruction that contains the account to index in the subgraph.",
            typing: Type::string(),
            optional: false,
            tainting: true
        }
    };

    pub static ref TOKEN_ACCOUNT_INSTRUCTION_SUBGRAPH: Type = define_strict_map_type! {
        name: {
            documentation: "The name of the instruction that contains the token account to index in the subgraph.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        account_name: {
            documentation: "The name of the token account in the instruction.",
            typing: Type::string(),
            optional: false,
            tainting: true
        }
    };

    pub static ref SUBGRAPH_DEFINED_FIELD: Type = define_strict_map_type! {
        name: {
            documentation: "The name of the field as it should appear in the subgraph.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        description: {
            documentation: "A description of the field as it should appear in the subgraph schema.",
            typing: Type::string(),
            optional: true,
            tainting: false
        },
        idl_key: {
            documentation: "A key from the event's type in the IDL, indicating which argument from the IDL type to map to this field. By default, the field name is used.",
            typing: Type::string(),
            optional: true,
            tainting: true
        },
        indexed: {
            documentation: "Whether this field should be indexed in the subgraph. If true, the field will be indexed and can be used as a filter in the subgraph. The default is false.",
            typing: Type::bool(),
            optional: false,
            tainting: true
        }
    };

    pub static ref SET_ACCOUNT_PATCH_RAW: Type = define_strict_map_type! {
        offset: {
            documentation: "The offset at which the patch should be applied.",
            typing: Type::integer(),
            optional: false,
            tainting: true
        },
        length: {
            documentation: "The length of the patch.",
            typing: Type::integer(),
            optional: false,
            tainting: true
        },
        value: {
            documentation: "The value to write at the patch offset, interpreted according to `type`.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        type: {
            documentation: "The type of the field being patched. This is used to determine how to apply the patch. Valid values are 'u8', 'u16', 'u32', 'u64', 'u128', 'i8', 'i16', 'i32', 'i64', 'i128', 'f32', 'f64', 'pubkey', 'string', 'boolean', or 'buffer'.",
            typing: Type::string(),
            optional: false,
            tainting: true
        }
    };

    pub static ref SET_ACCOUNT_PATCH_IDL: Type = define_strict_map_type! {
        program_idl: {
            documentation: "The IDL of the program being invoked.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        account_name: {
            documentation: "The name of the account in the IDL.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        value: {
            documentation: "The value to write to the specified field, encoded according to the IDL type.",
            typing: Type::string(),
            optional: false,
            tainting: true
        },
        name: {
            documentation: "The name of the field being patched. This is used to determine which field in the account data to update.",
            typing: Type::string(),
            optional: false,
            tainting: true
        }
    };

    pub static ref SET_ACCOUNT_MAP: Type = define_strict_map_type! {
        public_key: {
            documentation: "The public key of the account to set.",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        },
        lamports: {
            documentation: "The amount of lamports the account should be set to have.",
            typing: Type::integer(),
            optional: true,
            tainting: true
        },
        data: {
            documentation: "The data to set in the account.",
            typing: Type::buffer(),
            optional: true,
            tainting: true
        },
        owner: {
            documentation: "The owner to set for the account.",
            typing: Type::addon(SVM_PUBKEY),
            optional: true,
            tainting: true
        },
        executable: {
            documentation: "The executability state to set for the account.",
            typing: Type::bool(),
            optional: true,
            tainting: false
        },
        rent_epoch: {
            documentation: "The epoch at which the account will be rent-exempt.",
            typing: Type::integer(),
            optional: true,
            tainting: false
        },
        account_path: {
            documentation: "The path to a JSON file containing the account data to set. If provided, all other fields.",
            typing: Type::string(),
            optional: true,
            tainting: true
        },
        patch_raw: {
            documentation: "A raw patch to apply to the account's existing data. Each block specifies an offset, length, value, and type to determine which bytes of the account data to replace.",
            typing: SET_ACCOUNT_PATCH_RAW.clone(),
            optional: true,
            tainting: true
        },
        patch_idl: {
            documentation: "A patch to apply to the account's existing data, where the field to update is determined by the account's IDL. If provided, the program_idl and account_name will be used to determine which field in the account data to update with the provided bytes.",
            typing: SET_ACCOUNT_PATCH_IDL.clone(),
            optional: true,
            tainting: true
        }
    };

    pub static ref SET_TOKEN_ACCOUNT_MAP: Type = define_strict_map_type! {
        public_key: {
            documentation: "The public key of the token owner account to update.",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        },
        token: {
            documentation: "The token symbol or public key for the token.",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        },
        token_program: {
            documentation: "The token program id. Valid values are `token2020`, `token2022`, or a public key.",
            typing: Type::addon(SVM_PUBKEY),
            optional: true,
            tainting: true
        },
        amount: {
            documentation: "The amount of tokens to set.",
            typing: Type::integer(),
            optional: true,
            tainting: true
        },
        delegate: {
            documentation: "The public key of the delegate to set.",
            typing: Type::addon(SVM_PUBKEY),
            optional: true,
            tainting: true
        },
        delegated_amount: {
            documentation: "The amount of tokens to delegate.",
            typing: Type::integer(),
            optional: true,
            tainting: true
        },
        close_authority: {
            documentation: "The public key of the close authority to set.",
            typing: Type::addon(SVM_PUBKEY),
            optional: true,
            tainting: true
        },
        state: {
            documentation: "The state of the token account. Valid values are `initialized`, `frozen`, or `uninitialized`.",
            typing: Type::string(),
            optional: true,
            tainting: true
        }
    };

    pub static ref CLONE_PROGRAM_ACCOUNT: Type = define_strict_map_type! {
        source_program_id: {
            documentation: "The public key of the program to clone.",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        },
        destination_program_id: {
            documentation: "The destination public key of the program.",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        }
    };

    pub static ref SET_PROGRAM_AUTHORITY: Type = define_strict_map_type! {
        program_id: {
            documentation: "The public key of the program to set the authority for.",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        },
        authority: {
            documentation: "The new authority for the program. If not provided, program's authority will be set to None.",
            typing: Type::addon(SVM_PUBKEY),
            optional: true,
            tainting: true
        }
    };

    pub static ref DEPLOY_PROGRAM: Type = define_strict_map_type! {
        program_id: {
            documentation: "The public key of the program being deployed.",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        },
        binary_path: {
            documentation: "The location of the program's .so file. (e.g. `./target/deploy/my_program.so`)",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        },
        authority: {
            documentation: "The public key of the authority over the program after it is deployed. If not provided, the program will have no authority.",
            typing: Type::addon(SVM_PUBKEY),
            optional: true,
            tainting: true
        },
        idl_path: {
            documentation: "The location of the program's IDL file. (e.g. `./target/idl/my_program.json`)",
            typing: Type::addon(SVM_PUBKEY),
            optional: true,
            tainting: true
        }
    };

    pub static ref RESET_ACCOUNT: Type = define_strict_map_type! {
        public_key: {
            documentation: "The public key of the account to reset.",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        },
        include_owned_accounts: {
            documentation: "Whether to recursively delete all accounts owned by this account. Default is false.",
            typing: Type::bool(),
            optional: true,
            tainting: false
        }
    };

    pub static ref STREAM_ACCOUNT: Type = define_strict_map_type! {
        public_key: {
            documentation: "The public key of the account to stream.",
            typing: Type::addon(SVM_PUBKEY),
            optional: false,
            tainting: true
        },
        include_owned_accounts: {
            documentation: "Whether to recursively stream all accounts owned by this account. Default is false.",
            typing: Type::bool(),
            optional: true,
            tainting: false
        }
    };
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum DeploymentTransactionType {
    PrepareTempAuthority { keypair_bytes: Vec<u8>, already_exists: bool },
    CreateBuffer { buffer_pubkey: Pubkey },
    CreateBufferAndExtendProgram { buffer_pubkey: Pubkey },
    ExtendProgram,
    ResizeBuffer,
    WriteToBuffer { is_upgrade: bool },
    TransferBufferAuthority,
    TransferProgramAuthority,
    DeployProgram,
    DeployToken,
    UpgradeProgram,
    CloseTempAuthority,
    SkipCloseTempAuthority,
    CheatcodeDeployment,
    CheatcodeUpgrade,
}

impl DeploymentTransactionType {
    pub fn to_string(&self) -> String {
        match self {
            DeploymentTransactionType::PrepareTempAuthority { .. } => "create_temp_authority",
            DeploymentTransactionType::CreateBuffer { .. } => "create_buffer",
            DeploymentTransactionType::CreateBufferAndExtendProgram { .. } => {
                "create_buffer_and_extend_program"
            }
            DeploymentTransactionType::ResizeBuffer => "resize_buffer",
            DeploymentTransactionType::ExtendProgram => "extend_program",
            DeploymentTransactionType::WriteToBuffer { .. } => "write_to_buffer",
            DeploymentTransactionType::TransferBufferAuthority => "transfer_buffer_authority",
            DeploymentTransactionType::TransferProgramAuthority => "transfer_program_authority",
            DeploymentTransactionType::DeployProgram => "deploy_program",
            DeploymentTransactionType::DeployToken => "deploy_token",
            DeploymentTransactionType::UpgradeProgram => "upgrade_program",
            DeploymentTransactionType::CloseTempAuthority => "close_temp_authority",
            DeploymentTransactionType::SkipCloseTempAuthority => "skip_close_temp_authority",
            DeploymentTransactionType::CheatcodeDeployment => "cheatcode_deployment",
            DeploymentTransactionType::CheatcodeUpgrade => "cheatcode_upgrade",
        }
        .into()
    }
}
