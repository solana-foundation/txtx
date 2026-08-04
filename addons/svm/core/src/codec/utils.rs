use std::{str::FromStr, thread::sleep, time::Duration};

use solana_client::client_error::{ClientError, ClientErrorKind};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_request::RpcError;
use solana_clock::DEFAULT_MS_PER_SLOT;
use solana_pubkey::Pubkey;

use txtx_addon_kit::types::{diagnostics::Diagnostic, frontend::LogDispatcher, types::Value};
use txtx_addon_network_svm_types::anchor::types::Idl;

pub fn get_seeds_from_value(value: &Value) -> Result<Vec<Vec<u8>>, Diagnostic> {
    let seeds = value
        .as_array()
        .ok_or_else(|| diagnosed_error!("seeds must be an array"))?
        .iter()
        .map(|s| {
            let bytes = s.to_le_bytes();
            if bytes.is_empty() {
                return Err(diagnosed_error!("seed cannot be empty"));
            }
            if bytes.len() > 32 {
                if let Ok(pubkey) = Pubkey::from_str(&s.to_string()) {
                    return Ok(pubkey.to_bytes().to_vec());
                } else {
                    return Err(diagnosed_error!("seed cannot be longer than 32 bytes",));
                }
            }
            Ok(bytes)
        })
        .collect::<Result<Vec<_>, _>>()?;

    if seeds.len() > 16 {
        return Err(diagnosed_error!("seeds a maximum of 16 seeds can be used"));
    }

    Ok(seeds)
}

/// Raw bytes per `surfnet_writeProgram` call. Hex doubles the payload, so a 1 MiB
/// chunk is ~2 MiB on the wire — under the 5 MiB body cap of surfpool < 1.1.2.
const WRITE_PROGRAM_CHUNK_SIZE: usize = 1024 * 1024;

/// A chunk that fails mid-sequence leaves the program part-old, part-new, so
/// transient transport errors are worth a few attempts before giving up.
const WRITE_PROGRAM_ATTEMPTS: usize = 3;
const WRITE_PROGRAM_RETRY_DELAY: Duration = Duration::from_millis(200);

pub async fn cheatcode_deploy_program(
    rpc_client: &RpcClient,
    program_id: Pubkey,
    data: &[u8],
    upgrade_authority: Option<Pubkey>,
    logger: &LogDispatcher,
) -> Result<(), Diagnostic> {
    if data.is_empty() {
        return Err(diagnosed_error!("program binary is empty"));
    }

    let total = data.len().div_ceil(WRITE_PROGRAM_CHUNK_SIZE);
    // The server performs a read-modify-write on the program account for every call,
    // so chunks must land in order and one at a time or they'll clobber each other.
    for (i, (offset, chunk)) in write_program_chunks(data).enumerate() {
        if total > 1 {
            logger.pending_info(
                "Pending",
                format!("Writing chunk {}/{} of program {}", i + 1, total, program_id),
            );
        }

        let params = write_program_params(&program_id, chunk, offset, upgrade_authority.as_ref());
        let mut attempt = 1;
        loop {
            let result = rpc_client
                .send::<serde_json::Value>(
                    solana_client::rpc_request::RpcRequest::Custom {
                        method: "surfnet_writeProgram",
                    },
                    params.clone(),
                )
                .await;
            match result {
                Ok(_) => break,
                Err(e) if is_transient_client_error(&e) && attempt < WRITE_PROGRAM_ATTEMPTS => {
                    attempt += 1;
                    tokio::time::sleep(WRITE_PROGRAM_RETRY_DELAY).await;
                }
                Err(e) => {
                    return Err(write_program_error(
                        &e,
                        &program_id,
                        offset,
                        chunk.len(),
                        total > 1,
                    ));
                }
            }
        }
    }

    Ok(())
}

/// Io/transport failures may be momentary; JSON-RPC error responses are
/// deterministic, so retrying those only repeats the failure.
fn is_transient_client_error(e: &ClientError) -> bool {
    matches!(e.kind(), ClientErrorKind::Io(_) | ClientErrorKind::Reqwest(_))
}

/// (offset, chunk) pairs in ascending offset order.
fn write_program_chunks(data: &[u8]) -> impl Iterator<Item = (usize, &[u8])> {
    data.chunks(WRITE_PROGRAM_CHUNK_SIZE)
        .enumerate()
        .map(|(i, chunk)| (i * WRITE_PROGRAM_CHUNK_SIZE, chunk))
}

fn write_program_params(
    program_id: &Pubkey,
    chunk: &[u8],
    offset: usize,
    authority: Option<&Pubkey>,
) -> serde_json::Value {
    let mut params = vec![
        serde_json::Value::String(program_id.to_string()),
        serde_json::Value::String(txtx_addon_kit::hex::encode(chunk)),
        serde_json::Value::from(offset),
    ];
    // jsonrpc-derive pads missing trailing params, so omit rather than send null.
    if let Some(authority) = authority {
        params.push(serde_json::Value::String(authority.to_string()));
    }
    serde_json::Value::Array(params)
}

fn write_program_error(
    e: &ClientError,
    program_id: &Pubkey,
    offset: usize,
    len: usize,
    multi_chunk: bool,
) -> Diagnostic {
    if matches!(
        e.kind(),
        ClientErrorKind::RpcError(RpcError::RpcResponseError { code: -32601, .. })
    ) {
        return diagnosed_error!(
            "`surfnet_writeProgram` is not available on this surfnet; upgrade surfpool to v0.12.0 or later to use instant deployments"
        );
    }
    // A multi-chunk sequence that dies partway leaves the program part-old,
    // part-new; re-running the deployment rewrites every chunk.
    let recovery = if multi_chunk {
        "; the program may be partially written — re-run the deployment to rewrite it in full"
    } else {
        ""
    };
    diagnosed_error!(
        "`surfnet_writeProgram` RPC call failed writing {len} bytes at offset {offset} of program {program_id}: {e}{recovery}"
    )
}

pub fn cheatcode_register_idl(
    rpc_client: &solana_client::rpc_client::RpcClient,
    idl: &Idl,
) -> Result<serde_json::Value, Diagnostic> {
    let value = serde_json::to_value(idl).unwrap();
    let params = serde_json::to_value(&vec![value]).unwrap();
    send_rpc_request(rpc_client, "surfnet_registerIdl", params)
}

pub fn send_rpc_request(
    rpc_client: &solana_client::rpc_client::RpcClient,
    method: &'static str,
    params: serde_json::Value,
) -> Result<serde_json::Value, Diagnostic> {
    rpc_client
        .send::<serde_json::Value>(
            solana_client::rpc_request::RpcRequest::Custom { method },
            params,
        )
        .map_err(|e| diagnosed_error!("`{}` RPC call failed: {e}", method))
}

pub async fn cheatcode_register_idl_async(
    rpc_client: &RpcClient,
    idl: &Idl,
) -> Result<serde_json::Value, Diagnostic> {
    let value = serde_json::to_value(idl).unwrap();
    let params = serde_json::to_value(&vec![value]).unwrap();
    send_rpc_request_async(rpc_client, "surfnet_registerIdl", params).await
}

pub async fn send_rpc_request_async(
    rpc_client: &RpcClient,
    method: &'static str,
    params: serde_json::Value,
) -> Result<serde_json::Value, Diagnostic> {
    rpc_client
        .send::<serde_json::Value>(
            solana_client::rpc_request::RpcRequest::Custom { method },
            params,
        )
        .await
        .map_err(|e| diagnosed_error!("`{}` RPC call failed: {e}", method))
}

pub fn wait_n_slots(rpc_client: &solana_client::rpc_client::RpcClient, n: u64) -> u64 {
    let slot = rpc_client.get_slot().unwrap();
    loop {
        sleep(Duration::from_millis(DEFAULT_MS_PER_SLOT));
        let new_slot = rpc_client.get_slot().unwrap();
        if new_slot.saturating_sub(slot) >= n {
            return new_slot;
        }
    }
}

#[cfg(test)]
mod tests {
    use solana_pubkey::pubkey;

    use super::*;

    #[test]
    fn test_write_program_chunks_cover_binary() {
        let lengths = [
            1,
            WRITE_PROGRAM_CHUNK_SIZE - 1,
            WRITE_PROGRAM_CHUNK_SIZE,
            WRITE_PROGRAM_CHUNK_SIZE + 1,
            3 * WRITE_PROGRAM_CHUNK_SIZE + 7,
        ];

        for len in lengths {
            let data: Vec<u8> = (0..len).map(|i| (i % 256) as u8).collect();
            let chunks: Vec<(usize, &[u8])> = write_program_chunks(&data).collect();

            let mut expected_offset = 0;
            let mut reassembled = Vec::with_capacity(len);
            for (offset, chunk) in &chunks {
                assert_eq!(*offset, expected_offset);
                assert!(!chunk.is_empty());
                assert!(chunk.len() <= WRITE_PROGRAM_CHUNK_SIZE);
                reassembled.extend_from_slice(chunk);
                expected_offset += chunk.len();
            }
            assert_eq!(reassembled, data);
        }
    }

    #[test]
    fn test_write_program_chunk_fits_legacy_body_limit() {
        assert!(WRITE_PROGRAM_CHUNK_SIZE * 2 + 1024 < 5 * 1024 * 1024);
    }

    #[test]
    fn test_transient_error_classification() {
        let io_error: ClientError =
            ClientErrorKind::Io(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset"))
                .into();
        assert!(is_transient_client_error(&io_error));

        let rpc_error: ClientError = ClientErrorKind::RpcError(RpcError::RpcResponseError {
            code: -32602,
            message: "invalid params".to_string(),
            data: solana_client::rpc_request::RpcResponseErrorData::Empty,
        })
        .into();
        assert!(!is_transient_client_error(&rpc_error));
    }

    #[test]
    fn test_write_program_params_shape() {
        const PROGRAM_ID: Pubkey = pubkey!("11111111111111111111111111111111");
        const AUTHORITY: Pubkey = pubkey!("EnZsyjncjMShCUEPhz4rKnjKQ6gbPF4dkbUANZ2ngPo4");
        let chunk = vec![1u8, 2, 3, 4];

        let params = write_program_params(&PROGRAM_ID, &chunk, 128, None);
        let array = params.as_array().unwrap();
        assert_eq!(array.len(), 3);
        assert_eq!(array[0], PROGRAM_ID.to_string());
        assert_eq!(array[1].as_str().unwrap(), "01020304");
        assert_eq!(array[1].as_str().unwrap().len(), chunk.len() * 2);
        assert_eq!(array[2], 128);

        let params = write_program_params(&PROGRAM_ID, &chunk, 128, Some(&AUTHORITY));
        let array = params.as_array().unwrap();
        assert_eq!(array.len(), 4);
        assert_eq!(array[3], AUTHORITY.to_string());
    }
}
