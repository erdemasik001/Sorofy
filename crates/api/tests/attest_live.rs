//! The attestation path against the real registry. `#[ignore]`d: it needs `stellar-cli`, a
//! funded key that is staked on the registry, and it **submits a real transaction**.
//!
//! What the unit tests cannot show is that the four values the API computes are the four the
//! contract accepts — the argument names, the hex encoding, the auth entry, the fee, the whole
//! round trip. That is what this covers, and it covers it through `Attestor`, the same code
//! path a finished verification takes, rather than through a hand-built command.
//!
//! ```sh
//! export SOROFY_ATTEST_KEY_FILE=…            # a file holding the verifier's S… key
//! export SOROFY_REGISTRY_CONTRACT_ID=CDAC…   # the rehearsal registry, not the demo one
//! cargo test -p api --test attest_live -- --ignored --nocapture
//! ```
//!
//! The claim is synthetic and unique per run: a random `wasm_hash` opens a claim nobody has
//! attested, so the run is repeatable and can never collide with a real verification.

use std::time::Duration;

use api::attest::{AttestConfig, Attestor, Signer, TESTNET_PASSPHRASE};
use api::db::{AttestationStatus, Db};

#[tokio::test]
#[ignore = "submits a real transaction: needs stellar-cli and a staked key (see the module docs)"]
async fn an_attestation_reaches_the_registry_and_comes_back_with_its_transaction() {
    let key_file = required("SOROFY_ATTEST_KEY_FILE");
    let registry_id = required("SOROFY_REGISTRY_CONTRACT_ID");
    let cli = std::path::PathBuf::from(
        std::env::var("SOROFY_STELLAR_CLI").unwrap_or_else(|_| "stellar".into()),
    );
    let rpc_url = std::env::var("SOROFY_RPC").unwrap_or_else(|_| api::rpc::TESTNET_RPC.to_string());

    // A keystore of this run's own, next to nothing else — the same isolation the service
    // provisions at startup.
    let config_home = std::env::temp_dir().join(format!("sorofy-attest-live-{}", nanos()));
    let signer = Signer::provision(&cli, std::path::Path::new(&key_file), &config_home)
        .expect("provisioning the keystore from the key file");
    println!("attesting as {} to {registry_id}", signer.address);

    // A claim nobody can have attested: the window opens with this transaction.
    let wasm_hash = format!("{:064x}", nanos());
    let db = Db::open_in_memory().expect("in-memory db");
    let verification = db
        .insert_pending(
            None,
            &wasm_hash,
            &serde_json::json!({"kind": "live-test"}),
            "img",
        )
        .expect("pending row");
    db.enqueue_attestation(verification, &wasm_hash, &"ab".repeat(32), &wasm_hash)
        .expect("queueing")
        .expect("a fresh claim is queued");

    let _attestor = Attestor::start(
        AttestConfig {
            cli,
            registry_id,
            rpc_url,
            network_passphrase: std::env::var("SOROFY_NETWORK_PASSPHRASE")
                .unwrap_or_else(|_| TESTNET_PASSPHRASE.to_string()),
            signer,
        },
        db.clone(),
    );

    // Simulate, sign, submit and confirm is seconds of network; give it a minute before
    // calling it a failure, and report whatever the row says if it never lands.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let row = loop {
        let row = db
            .attestation_for(verification)
            .expect("reading the row")
            .expect("the row exists");
        if row.status != AttestationStatus::Pending || std::time::Instant::now() > deadline {
            break row;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };

    let _ = std::fs::remove_dir_all(&config_home);
    assert_eq!(
        row.status,
        AttestationStatus::Submitted,
        "attestation did not land after {} attempt(s): {}",
        row.attempts,
        row.last_error.as_deref().unwrap_or("no error recorded")
    );
    let tx = row
        .tx_hash
        .expect("a submitted attestation names its transaction");
    assert_eq!(tx.len(), 64, "not a transaction hash: {tx}");
    println!("https://stellar.expert/explorer/testnet/tx/{tx}");
}

fn required(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("{name} must be set; see this file's module docs"))
}

/// Nanoseconds since the epoch — a value no earlier run of this test can have used.
fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos()
}
