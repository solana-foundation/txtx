use serde::{Deserialize, Serialize};
use serde_json::json;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_pubkey::Pubkey;
use txtx_addon_kit::types::{diagnostics::Diagnostic, frontend::LogDispatcher, stores::ValueStore};
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

        let block_entries = values
            .get_value(BLOCK_ACCOUNT_DOWNLOAD)
            .map(|v| {
                v.as_map().ok_or_else(|| {
                    diagnosed_error!("'{}' must be a map type", BLOCK_ACCOUNT_DOWNLOAD)
                })
            })
            .transpose()?;

        let Some(block_entries) = block_entries else {
            return Ok(vec![]);
        };

        let mut block_entries = block_entries
            .iter()
            .map(|i| {
                i.as_object()
                    .map(|o| o.clone())
                    .ok_or(diagnosed_error!("'{}' must be a map type", BLOCK_ACCOUNT_DOWNLOAD))
            })
            .collect::<Result<Vec<_>, _>>()?;

        for (i, block_entry) in block_entries.iter_mut().enumerate() {
            let prefix = format!("failed to parse `{}` map #{}", BLOCK_ACCOUNT_DOWNLOAD, i + 1);

            let public_key = block_entry
                .swap_remove("public_key")
                .map(|p| {
                    SvmValue::to_pubkey(&p)
                        .map_err(|e| diagnosed_error!("invalid 'public_key' field: {e}"))
                })
                .ok_or_else(|| diagnosed_error!("{prefix}: missing required 'public_key'"))?
                .map_err(|e| diagnosed_error!("{prefix}: {e}"))?;

            let include_owned_accounts = block_entry
                .swap_remove("include_owned_accounts")
                .map(|v| {
                    v.as_bool().ok_or_else(|| {
                        diagnosed_error!("expected 'include_owned_accounts' field to be a boolean")
                    })
                })
                .transpose()
                .map_err(|e| diagnosed_error!("{prefix}: {e}"))?
                .unwrap_or(false);

            blocks.push(SurfpoolBlockAccountDownload { public_key, include_owned_accounts });
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

        crate::codec::utils::send_rpc_request_async(rpc_client, Self::rpc_method(), params).await?;

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
        "surfnet_offlineAccount"
    }

    fn to_request_params(&self) -> serde_json::Value {
        json!([
            self.public_key.to_string(),
            { "includeOwnedAccounts": self.include_owned_accounts }
        ])
    }

    fn update_status(&self, logger: &LogDispatcher, _index: usize, _total: usize) {
        logger.info(
            "Blocking Account Download",
            format!(
                "Blocking account {} from being downloaded from mainnet{}",
                self.public_key,
                if self.include_owned_accounts { " (including owned accounts)" } else { "" }
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use txtx_addon_kit::types::types::Value;
    use txtx_addon_kit::types::Did;

    fn new_value_store() -> ValueStore {
        ValueStore::new("test", &Did::from_components(vec![b"test"]))
    }

    #[test]
    fn test_block_account_download_parsing() {
        let mut value_store = new_value_store();

        let test_pubkey = "11111111111111111111111111111111";

        value_store.insert(
            "block_account_download",
            Value::array(vec![Value::object(txtx_addon_kit::indexmap::indexmap! {
                "public_key".to_string() => Value::string(test_pubkey.to_string()),
                "include_owned_accounts".to_string() => Value::bool(true),
            })]),
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
        let mut value_store = new_value_store();

        let test_pubkey = "11111111111111111111111111111111";

        value_store.insert(
            "block_account_download",
            Value::array(vec![Value::object(txtx_addon_kit::indexmap::indexmap! {
                "public_key".to_string() => Value::string(test_pubkey.to_string()),
            })]),
        );

        let result = SurfpoolBlockAccountDownload::parse_value_store(&value_store);
        assert!(result.is_ok());

        let blocks = result.unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].include_owned_accounts, false, "Should default to false");
    }

    #[test]
    fn test_block_account_download_invalid_pubkey() {
        let mut value_store = new_value_store();

        value_store.insert(
            "block_account_download",
            Value::array(vec![Value::object(txtx_addon_kit::indexmap::indexmap! {
                "public_key".to_string() => Value::string("invalid_key".to_string()),
            })]),
        );

        let result = SurfpoolBlockAccountDownload::parse_value_store(&value_store);
        assert!(result.is_err(), "Should fail with invalid public key");
    }

    #[test]
    fn test_block_account_download_missing_pubkey() {
        let mut value_store = new_value_store();

        value_store.insert(
            "block_account_download",
            Value::array(vec![Value::object(txtx_addon_kit::indexmap::indexmap! {
                "include_owned_accounts".to_string() => Value::bool(true),
            })]),
        );

        let result = SurfpoolBlockAccountDownload::parse_value_store(&value_store);
        assert!(result.is_err(), "Should fail when public_key is missing");
    }

    #[test]
    fn test_rpc_method_name() {
        assert_eq!(SurfpoolBlockAccountDownload::rpc_method(), "surfnet_offlineAccount");
    }

    #[test]
    fn test_to_request_params_format() {
        let pubkey = Pubkey::from_str("11111111111111111111111111111111").unwrap();
        let block =
            SurfpoolBlockAccountDownload { public_key: pubkey, include_owned_accounts: true };

        let params = block.to_request_params();

        assert!(params.is_array());
        let params_array = params.as_array().unwrap();
        assert_eq!(params_array.len(), 2);

        // The pubkey is passed as a base-58 string, as the first positional param.
        assert_eq!(params_array[0].as_str().unwrap(), "11111111111111111111111111111111");

        // The options are passed as a separate object, as the second positional param.
        let config = params_array[1].as_object().unwrap();
        assert_eq!(config.get("includeOwnedAccounts").unwrap().as_bool().unwrap(), true);
    }

    #[test]
    fn test_to_request_params_format_without_owned_accounts() {
        let pubkey = Pubkey::from_str("11111111111111111111111111111111").unwrap();
        let block =
            SurfpoolBlockAccountDownload { public_key: pubkey, include_owned_accounts: false };

        let params = block.to_request_params();
        let params_array = params.as_array().unwrap();
        assert_eq!(params_array.len(), 2);
        assert_eq!(params_array[0].as_str().unwrap(), "11111111111111111111111111111111");
        let config = params_array[1].as_object().unwrap();
        assert_eq!(config.get("includeOwnedAccounts").unwrap().as_bool().unwrap(), false);
    }

    #[test]
    fn test_multiple_blocks_parsing() {
        let mut value_store = new_value_store();

        let pubkey1 = "11111111111111111111111111111111";
        let pubkey2 = "4EXSeLGxVBpAZwq7vm6evLdewpcvE2H56fpqL2pPiLFa";

        value_store.insert(
            "block_account_download",
            Value::array(vec![
                Value::object(txtx_addon_kit::indexmap::indexmap! {
                    "public_key".to_string() => Value::string(pubkey1.to_string()),
                    "include_owned_accounts".to_string() => Value::bool(true),
                }),
                Value::object(txtx_addon_kit::indexmap::indexmap! {
                    "public_key".to_string() => Value::string(pubkey2.to_string()),
                    "include_owned_accounts".to_string() => Value::bool(false),
                }),
            ]),
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
