//! Soroban RPC lookup: `contract_id` → on-chain WASM hash.
//!
//! This is the metadata-retrieval step of SEP-58's verification algorithm that
//! `verifier-core` deliberately leaves out: resolving what hash a deployed
//! contract actually points at, so the caller does not have to be trusted about
//! it. One JSON-RPC call (`getLedgerEntries`) on the contract's instance entry.

use anyhow::{anyhow, bail, Context};
use serde::Deserialize;
use stellar_xdr::{
    ContractDataDurability, ContractExecutable, ContractId, Hash, LedgerEntryData, LedgerKey,
    LedgerKeyContractData, Limits, ReadXdr, ScAddress, ScVal, WriteXdr,
};

/// The public testnet RPC endpoint (also what Day0's manual lookup used).
pub const TESTNET_RPC: &str = "https://soroban-testnet.stellar.org";

/// What a contract's instance entry says it executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnChainExecutable {
    /// Uploaded WASM, identified by the sha256 the network stores — the hash a
    /// reproduction must land on.
    Wasm { wasm_hash_hex: String },
    /// A built-in (Stellar Asset Contract) — nothing to rebuild.
    StellarAsset,
}

/// Look up what `contract_id` (a `C…` strkey) executes on-chain.
///
/// Returns `Ok(None)` if the contract does not exist on this network — a
/// well-formed query about an absent entry, distinct from a transport failure.
pub fn fetch_executable(rpc_url: &str, contract_id: &str) -> anyhow::Result<Option<OnChainExecutable>> {
    let contract = stellar_strkey::Contract::from_string(contract_id)
        .map_err(|e| anyhow!("`{contract_id}` is not a valid contract id (C… strkey): {e:?}"))?;

    // The instance entry lives under key (contract, LedgerKeyContractInstance,
    // durability=persistent) — the same triple for every contract.
    let key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(ContractId(Hash(contract.0))),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
    });
    let key_b64 = key
        .to_xdr_base64(Limits::none())
        .context("encoding ledger key")?;

    let entry_b64 = match get_ledger_entry(rpc_url, &key_b64)? {
        Some(xdr) => xdr,
        None => return Ok(None),
    };

    let data = LedgerEntryData::from_xdr_base64(&entry_b64, Limits::none())
        .context("decoding ledger entry XDR")?;
    let LedgerEntryData::ContractData(entry) = data else {
        bail!("RPC returned a non-ContractData entry for a ContractData key");
    };
    let ScVal::ContractInstance(instance) = entry.val else {
        bail!("contract instance entry does not hold a ContractInstance value");
    };

    Ok(Some(match instance.executable {
        ContractExecutable::Wasm(hash) => OnChainExecutable::Wasm {
            wasm_hash_hex: hex::encode(hash.0),
        },
        ContractExecutable::StellarAsset => OnChainExecutable::StellarAsset,
    }))
}

/// Minimal `getLedgerEntries` call: one key in, that key's XDR out.
fn get_ledger_entry(rpc_url: &str, key_b64: &str) -> anyhow::Result<Option<String>> {
    #[derive(Deserialize)]
    struct RpcResponse {
        result: Option<RpcResult>,
        error: Option<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct RpcResult {
        #[serde(default)]
        entries: Vec<RpcEntry>,
    }
    #[derive(Deserialize)]
    struct RpcEntry {
        xdr: String,
    }

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getLedgerEntries",
        "params": { "keys": [key_b64] },
    });

    let resp: RpcResponse = ureq::post(rpc_url)
        .send_json(body)
        .with_context(|| format!("POST {rpc_url} (getLedgerEntries)"))?
        .into_json()
        .context("parsing RPC response")?;

    if let Some(err) = resp.error {
        bail!("RPC error from {rpc_url}: {err}");
    }
    let result = resp.result.ok_or_else(|| anyhow!("RPC response has neither result nor error"))?;
    Ok(result.entries.into_iter().next().map(|e| e.xdr))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Day0's real testnet target: its on-chain hash was confirmed manually via
    /// `stellar contract fetch` + StellarExpert (docs/day0-reproduction-findings.md).
    /// This pins the RPC path to that same ground truth.
    #[test]
    #[ignore = "hits the public testnet RPC"]
    fn resolves_day0_target_to_its_known_hash() {
        let exec = fetch_executable(
            TESTNET_RPC,
            "CDZZZTN6RXOWY2WDJV2GLFAV76YKAIRFPNB4EABMFAVJQ5DCZIAE4DYA",
        )
        .expect("RPC lookup should succeed")
        .expect("contract exists on testnet");
        assert_eq!(
            exec,
            OnChainExecutable::Wasm {
                wasm_hash_hex:
                    "bfab576fb405952fdeb0c502e3f662668601f3b63111bcf034c240cea4b6240d".into()
            }
        );
    }

    #[test]
    fn rejects_a_malformed_contract_id() {
        assert!(fetch_executable(TESTNET_RPC, "not-a-contract-id").is_err());
    }

    #[test]
    #[ignore = "hits the public testnet RPC"]
    fn absent_contract_resolves_to_none() {
        // A syntactically valid strkey for an all-zero hash; nothing lives there.
        let id = stellar_strkey::Contract([0u8; 32]).to_string();
        let exec = fetch_executable(TESTNET_RPC, &id).expect("lookup should succeed");
        assert_eq!(exec, None);
    }
}
