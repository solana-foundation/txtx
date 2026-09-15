use serde::{Deserialize, Serialize};
use serde_json::json;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_pubkey::Pubkey;
use txtx_addon_kit::types::{
    diagnostics::Diagnostic, frontend::LogDispatcher, stores::ValueStore,
};
use txtx_addon_network_svm_types::SvmValue;

use super::surfnet_update::SurfnetAccountUpdate;
use crate::constants::BLOCK_ACCOUNT_DOWNLOAD;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurfpoolBlockAccountDownload {
    pub public_key: Pubkey,
    #[serde(default)]
    pub include_owned_accounts: bool,
}

impl SurfpoolBlockAccountDownload {
    pub fn parse_value_store(values: &ValueStore) -> Result<Vec<Self>, Diagnostic> {
        let mut blocks = vec![];

        if let Some(block_entries) = values.get_value(BLOCK_ACCOUNT_DOWNLOAD) {
            let entries = block_entries.as_array().ok_or_else(|| {
                diagnosed_error!(
                    "expected '{}' to be an array of block_account_download configurations",
                    BLOCK_ACCOUNT_DOWNLOAD
                )
            })?;

            for entry in entries.iter() {
                let entry_map = entry.as_object().ok_or_else(|| {
                    diagnosed_error!(
                        "expected each '{}' entry to be a map with 'public_key' and optional 'include_owned_accounts'",
                        BLOCK_ACCOUNT_DOWNLOAD
                    )
                })?;

                let public_key_value = entry_map
                    .get("public_key")
                    .ok_or_else(|| {
                        diagnosed_error!("'public_key' is required in block_account_download")
                    })?;

                let public_key = SvmValue::to_pubkey(public_key_value).map_err(|e| {
                    diagnosed_error!("invalid public key in block_account_download: {e}")
                })?;

                let include_owned_accounts = entry_map
                    .get("include_owned_accounts")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                blocks.push(SurfpoolBlockAccountDownload {
                    public_key,
                    include_owned_accounts,
                });
            }
        }

        Ok(blocks)
    }

    pub async fn process_updates(
        blocks: Vec<Self>,
        rpc_client: &RpcClient,
        logger: &LogDispatcher,
    ) -> Result<(), Diagnostic> {
        for block in blocks {
            block.send_request(rpc_client, logger).await?;
        }
        Ok(())
    }

    async fn send_request(
        &self,
        rpc_client: &RpcClient,
        logger: &LogDispatcher,
    ) -> Result<(), Diagnostic> {
        self.update_status(logger, 0, 1);

        let params = self.to_request_params();

        crate::codec::utils::send_rpc_request_async(
            rpc_client,
            Self::rpc_method(),
            params,
        )
        .await?;

        logger.success_info(
            "Blocked Account Download",
            format!("Successfully blocked account {}", self.public_key),
        );

        Ok(())
    }
}

impl SurfnetAccountUpdate for SurfpoolBlockAccountDownload {
    fn rpc_method() -> &'static str
    where
        Self: Sized,
    {
        "surfnet_blockAccountDownload"
    }

    fn to_request_params(&self) -> serde_json::Value {
        json!([{
            "pubkey": self.public_key.to_string(),
            "includeOwnedAccounts": self.include_owned_accounts,
        }])
    }

    fn update_status(&self, logger: &LogDispatcher, _index: usize, _total: usize) {
        logger.info(
            "Blocking Account Download",
            format!(
                "Blocking account {} from being downloaded from mainnet{}",
                self.public_key,
                if self.include_owned_accounts {
                    " (including owned accounts)"
                } else {
                    ""
                }
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use txtx_addon_kit::types::types::Value;

    #[test]
    fn test_block_account_download_parsing() {
        let mut value_store = ValueStore::new();

        let test_pubkey = "11111111111111111111111111111111";

        value_store.insert(
            "block_account_download".to_string(),
            Value::array(vec![
                Value::object(indexmap::indexmap! {
                    "public_key".to_string() => Value::string(test_pubkey.to_string()),
                    "include_owned_accounts".to_string() => Value::bool(true),
                })
            ])
        );

        let result = SurfpoolBlockAccountDownload::parse_value_store(&value_store);
        assert!(result.is_ok(), "Failed to parse valid config: {:?}", result.err());

        let blocks = result.unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].public_key.to_string(), test_pubkey);
        assert_eq!(blocks[0].include_owned_accounts, true);
    }

    #[test]
    fn test_block_account_download_default_include_owned() {
        let mut value_store = ValueStore::new();

        let test_pubkey = "11111111111111111111111111111111";

        value_store.insert(
            "block_account_download".to_string(),
            Value::array(vec![
                Value::object(indexmap::indexmap! {
                    "public_key".to_string() => Value::string(test_pubkey.to_string()),
                })
            ])
        );

        let result = SurfpoolBlockAccountDownload::parse_value_store(&value_store);
        assert!(result.is_ok());

        let blocks = result.unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].include_owned_accounts, false, "Should default to false");
    }

    #[test]
    fn test_block_account_download_invalid_pubkey() {
        let mut value_store = ValueStore::new();

        value_store.insert(
            "block_account_download".to_string(),
            Value::array(vec![
                Value::object(indexmap::indexmap! {
                    "public_key".to_string() => Value::string("invalid_key".to_string()),
                })
            ])
        );

        let result = SurfpoolBlockAccountDownload::parse_value_store(&value_store);
        assert!(result.is_err(), "Should fail with invalid public key");
    }

    #[test]
    fn test_block_account_download_missing_pubkey() {
        let mut value_store = ValueStore::new();

        value_store.insert(
            "block_account_download".to_string(),
            Value::array(vec![
                Value::object(indexmap::indexmap! {
                    "include_owned_accounts".to_string() => Value::bool(true),
                })
            ])
        );

        let result = SurfpoolBlockAccountDownload::parse_value_store(&value_store);
        assert!(result.is_err(), "Should fail when public_key is missing");
    }

    #[test]
    fn test_rpc_method_name() {
        assert_eq!(
            SurfpoolBlockAccountDownload::rpc_method(),
            "surfnet_blockAccountDownload"
        );
    }

    #[test]
    fn test_to_request_params_format() {
        let pubkey = Pubkey::from_str("11111111111111111111111111111111").unwrap();
        let block = SurfpoolBlockAccountDownload {
            public_key: pubkey,
            include_owned_accounts: true,
        };

        let params = block.to_request_params();

        assert!(params.is_array());
        let params_array = params.as_array().unwrap();
        assert_eq!(params_array.len(), 1);

        let first_param = &params_array[0];
        assert!(first_param.is_object());

        let obj = first_param.as_object().unwrap();
        assert_eq!(obj.get("pubkey").unwrap().as_str().unwrap(), "11111111111111111111111111111111");
        assert_eq!(obj.get("includeOwnedAccounts").unwrap().as_bool().unwrap(), true);
    }

    #[test]
    fn test_multiple_blocks_parsing() {
        let mut value_store = ValueStore::new();

        let pubkey1 = "11111111111111111111111111111111";
        let pubkey2 = "22222222222222222222222222222222";

        value_store.insert(
            "block_account_download".to_string(),
            Value::array(vec![
                Value::object(indexmap::indexmap! {
                    "public_key".to_string() => Value::string(pubkey1.to_string()),
                    "include_owned_accounts".to_string() => Value::bool(true),
                }),
                Value::object(indexmap::indexmap! {
                    "public_key".to_string() => Value::string(pubkey2.to_string()),
                    "include_owned_accounts".to_string() => Value::bool(false),
                })
            ])
        );

        let result = SurfpoolBlockAccountDownload::parse_value_store(&value_store);
        assert!(result.is_ok());

        let blocks = result.unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].public_key.to_string(), pubkey1);
        assert_eq!(blocks[0].include_owned_accounts, true);
        assert_eq!(blocks[1].public_key.to_string(), pubkey2);
        assert_eq!(blocks[1].include_owned_accounts, false);
    }
}
