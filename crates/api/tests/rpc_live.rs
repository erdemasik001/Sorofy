//! Live-network checks for the RPC lookup, against contracts whose hashes we
//! know from independent ground truth. `#[ignore]`d: they need the public
//! testnet RPC.

use api::rpc::{fetch_executable, OnChainExecutable, TESTNET_RPC};

/// The Day2 real-size determinism target: the token contract we built in the
/// pinned container and deployed ourselves (docs/day2-api.md). The network's
/// stored hash must equal what the container produced.
#[test]
#[ignore = "hits the public testnet RPC"]
fn deployed_token_contract_resolves_to_container_built_hash() {
    let exec = fetch_executable(
        TESTNET_RPC,
        "CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6",
    )
    .expect("RPC lookup should succeed")
    .expect("contract exists on testnet");
    assert_eq!(
        exec,
        OnChainExecutable::Wasm {
            wasm_hash_hex: "47d2801e115f9a064fe37a8244ef1ffcfa56877668a17f383d3189634a1bcfbd"
                .into()
        }
    );
}
