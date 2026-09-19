//! On-chain attestation (hackathon STEP 5): tell the registry what this verifier rebuilt.
//!
//! Off by default. With `SOROFY_ATTEST=1` a finished verification also queues a row in the
//! `attestations` outbox, and a single detached worker drains that queue by shelling out to
//! `stellar contract invoke … attest`. Everything here is arranged around one rule: **the
//! verification must not be able to notice.** In particular
//!
//! - nothing on this path runs inside `run_job`'s task except one SQLite insert, so no build
//!   slot and no admission permit is ever held for a network round trip;
//! - no failure here reaches the verification's status, its report, or its HTTP response;
//! - the queue is in the database, so an attestation queued before a restart is picked up
//!   again rather than lost — `fail_orphaned_pending` deliberately does not touch this table.
//!
//! **Signing.** There is no Soroban transaction-building crate in this repo, so signing is a
//! shell-out to `stellar-cli`. That CLI signs a Soroban auth entry with *any* key in the local
//! keystore, not only the transaction's source account (HACKATHON.md, "Findings along the
//! way"), which is a live hazard the moment several verifiers share a machine. The answer is
//! a keystore per instance: [`Signer::provision`] builds a private config home holding exactly
//! one identity and refuses to run against one that holds any other, so the CLI has nothing
//! else it *could* sign as.
//!
//! **The key** is read from the file `SOROFY_ATTEST_KEY_FILE` points at. It is never logged,
//! never put on a command line, never placed in the child's environment, and never carried in
//! an error: the only place it goes is the identity file, written 0600 inside a 0700
//! directory. Output captured from the CLI is scrubbed of anything shaped like a secret key
//! before it is stored or logged, so even a future CLI that echoed one could not leak it
//! through this path.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use verifier_core::ReproductionReport;

use crate::claim::{Claim, NoClaim};
use crate::db::Db;

/// Default Soroban network passphrase — testnet, which is all this build targets.
pub const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";

/// The single identity the per-instance keystore is allowed to hold.
const IDENTITY_ALIAS: &str = "sorofy-verifier";

/// How long one `stellar contract invoke` may take before it is killed and retried.
///
/// Generous next to a simulate-sign-submit round trip (seconds), short next to the 60-ledger
/// attest window (≈ 5 min), so a hung CLI still leaves room for a retry inside the window.
const INVOKE_TIMEOUT: Duration = Duration::from_secs(60);

/// Backoff for a failure that may yet clear: `BASE · 2^(attempts-1)`, capped.
const RETRY_BASE: Duration = Duration::from_secs(5);
const RETRY_CAP: Duration = Duration::from_secs(300);

/// Attempts before an attestation is given up on.
///
/// With the schedule above that is roughly twenty minutes of trying — well past the attest
/// window, so a claim that is still unreachable by then was never going to land. Giving up
/// writes a `rejected` row with the last error, rather than retrying a dead endpoint forever.
const MAX_ATTEMPTS: i64 = 10;

/// How long the worker sleeps when the queue is empty.
///
/// Only a backstop: [`Attestor::enqueue`] wakes it immediately, so this bounds how long a row
/// left over from a previous process waits at startup, not the latency of live work.
const IDLE_POLL: Duration = Duration::from_secs(60);

/// Everything the attestation path needs, resolved once at startup.
pub struct AttestConfig {
    /// Path to the `stellar` binary.
    pub cli: PathBuf,
    /// The registry contract to attest to.
    pub registry_id: String,
    pub rpc_url: String,
    pub network_passphrase: String,
    /// The private keystore, and the verifier address derived from the key in it.
    pub signer: Signer,
}

/// A handle to the attestation path. Cheap to clone; `None` inside means the flag is off.
///
/// With the flag off every method is a no-op that touches nothing — no database write, no
/// spawn, no log line — which is what makes the default path byte-for-byte the service that
/// ran before STEP 5.
#[derive(Clone, Default)]
pub struct Attestor(Option<Arc<Inner>>);

struct Inner {
    config: AttestConfig,
    db: Db,
    /// Wakes the worker the moment something is queued.
    wake: tokio::sync::Notify,
}

impl Attestor {
    /// The disabled attestor: what `AppState::new` holds unless `main` replaces it.
    pub fn disabled() -> Self {
        Attestor(None)
    }

    pub fn is_enabled(&self) -> bool {
        self.0.is_some()
    }

    /// The verifier address attestations are filed under, when enabled.
    pub fn verifier_address(&self) -> Option<&str> {
        self.0.as_ref().map(|i| i.config.signer.address.as_str())
    }

    /// Enable attestation and start the detached worker.
    ///
    /// The worker is spawned here rather than per job: one loop owns the retry schedule, and
    /// there is no per-job task that could accidentally capture a build or admission permit.
    pub fn start(config: AttestConfig, db: Db) -> Self {
        let inner = Arc::new(Inner {
            config,
            db,
            wake: tokio::sync::Notify::new(),
        });
        let attestor = Attestor(Some(inner.clone()));
        tokio::spawn(async move { worker(inner).await });
        attestor
    }

    /// An attestor that queues but never submits: the enqueue half with no worker behind it.
    ///
    /// For tests about what reaches the outbox — whether a job is attested at all — rather
    /// than what reaches the chain.
    #[cfg(test)]
    pub(crate) fn queue_only(db: Db) -> Self {
        Attestor(Some(Arc::new(Inner {
            config: AttestConfig {
                cli: PathBuf::from("stellar"),
                registry_id: "CA4VYPAGEYYOV7CJIBTCJHOGW2KAFQ4AHYEZIY2NA3NXFJGGG4XSPCFE".into(),
                rpc_url: "http://rpc.invalid".into(),
                network_passphrase: TESTNET_PASSPHRASE.into(),
                signer: Signer {
                    config_home: PathBuf::from("/nonexistent"),
                    address: format!("{}", stellar_strkey::ed25519::PublicKey([3u8; 32])),
                },
            },
            db,
            wake: tokio::sync::Notify::new(),
        })))
    }

    /// Queue the attestation for a finished verification.
    ///
    /// Called from `run_job` with the job's report in hand. Does one SQLite insert and
    /// returns; the transaction itself happens on the worker. Every failure is swallowed into
    /// a log line, because there is no failure here that should be allowed to change what the
    /// caller reports about the verification.
    pub fn enqueue(&self, verification_id: i64, report: &ReproductionReport) {
        let Some(inner) = self.0.as_ref() else {
            return;
        };

        let claim = match Claim::from_report(report) {
            Ok(claim) => claim,
            // Not an error: a report that cannot form a claim has nothing to attest and
            // nothing to retry. The common case is a locally built image with no registry
            // digest, which is a local-dev path.
            Err(why @ NoClaim::NoImageDigest) => {
                tracing::info!(id = verification_id, reason = %why, "not attesting this job");
                return;
            }
            Err(why @ NoClaim::Malformed(_)) => {
                tracing::warn!(id = verification_id, reason = %why, "not attesting this job");
                return;
            }
        };

        let queued = inner.db.enqueue_attestation(
            verification_id,
            &hex::encode(claim.wasm_hash),
            &hex::encode(claim.input_digest),
            &hex::encode(claim.rebuilt_hash),
        );
        match queued {
            Ok(Some(row_id)) => {
                tracing::info!(
                    id = verification_id,
                    attestation = row_id,
                    wasm_hash = %hex::encode(claim.wasm_hash),
                    input_digest = %hex::encode(claim.input_digest),
                    "attestation queued"
                );
                inner.wake.notify_one();
            }
            // The UNIQUE on `verification_id` makes this the already-queued case.
            Ok(None) => tracing::debug!(id = verification_id, "attestation already queued"),
            Err(e) => {
                tracing::error!(id = verification_id, error = %e, "could not queue attestation")
            }
        }
    }
}

/// Drain the outbox forever: take what is due, attempt it, sleep until the next one is.
async fn worker(inner: Arc<Inner>) {
    tracing::info!(
        registry = %inner.config.registry_id,
        verifier = %inner.config.signer.address,
        "attestation worker started"
    );
    loop {
        match inner.db.due_attestation() {
            // Straight back round, so a backlog (a restart, say) drains without waiting — but
            // only when the outcome was written down. If it was not, the row is still
            // `pending` and still due, and looping now would re-submit it as fast as the
            // network allows. Sleeping first gives the database a moment to recover instead.
            Ok(Some(row)) if attempt(&inner, &row).await => continue,
            Ok(Some(_)) | Ok(None) => {}
            Err(e) => tracing::error!(error = %e, "could not read the attestation queue"),
        }

        let wait = match inner.db.seconds_until_next_attestation() {
            // A row is pending but not yet due. Clamp: `secs` can be ≤ 0 if it came due
            // between the two queries, and never sleep past the idle backstop.
            Ok(Some(secs)) => Duration::from_secs(secs.clamp(1, IDLE_POLL.as_secs() as i64) as u64),
            Ok(None) => IDLE_POLL,
            Err(e) => {
                tracing::error!(error = %e, "could not schedule the attestation worker");
                IDLE_POLL
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = inner.wake.notified() => {}
        }
    }
}

/// One submission attempt, and the bookkeeping that follows it either way.
///
/// Returns whether the outcome was recorded. `false` means the row is unchanged and will be
/// taken again, so the caller must not treat it as progress.
async fn attempt(inner: &Inner, row: &crate::db::AttestationRow) -> bool {
    let outcome = submit(&inner.config, row).await;
    let recorded = match outcome {
        Ok(tx_hash) => {
            match &tx_hash {
                Some(tx) => tracing::info!(
                    attestation = row.id,
                    id = row.verification_id,
                    tx = %tx,
                    "attestation submitted"
                ),
                // The CLI succeeded but named no transaction. The attestation is on-chain;
                // we just cannot cite it, which is worth a warning and not a retry.
                None => tracing::warn!(
                    attestation = row.id,
                    id = row.verification_id,
                    "attestation submitted, but no transaction hash could be read from the CLI"
                ),
            }
            inner.db.attestation_submitted(row.id, tx_hash.as_deref())
        }
        Err(SubmitError::Permanent(why)) => {
            tracing::warn!(
                attestation = row.id,
                id = row.verification_id,
                reason = %why,
                "attestation refused; not retrying"
            );
            inner.db.attestation_rejected(row.id, &why)
        }
        Err(SubmitError::Transient(why)) => {
            let next = row.attempts + 1;
            if next >= MAX_ATTEMPTS {
                tracing::error!(
                    attestation = row.id,
                    id = row.verification_id,
                    attempts = next,
                    reason = %why,
                    "giving up on attestation"
                );
                inner.db.attestation_rejected(
                    row.id,
                    &format!("gave up after {next} attempts; last error: {why}"),
                )
            } else {
                let delay = backoff(next);
                tracing::warn!(
                    attestation = row.id,
                    id = row.verification_id,
                    attempts = next,
                    retry_in_seconds = delay.as_secs(),
                    reason = %why,
                    "attestation failed; will retry"
                );
                inner
                    .db
                    .attestation_retry_later(row.id, &why, delay)
                    .map(|_| ())
            }
        }
    };
    match recorded {
        Ok(()) => true,
        Err(e) => {
            // The row stays `pending` and comes round again — at worst a duplicate
            // submission, which the registry answers with `AlreadyAttested` and we record as
            // a refusal.
            tracing::error!(attestation = row.id, error = %e, "could not record attestation outcome");
            false
        }
    }
}

/// `RETRY_BASE · 2^(attempts-1)`, capped at [`RETRY_CAP`].
fn backoff(attempts: i64) -> Duration {
    let shift = attempts.clamp(1, 32) as u32 - 1;
    RETRY_BASE
        .saturating_mul(1u32.checked_shl(shift).unwrap_or(u32::MAX))
        .min(RETRY_CAP)
}

/// Why one submission did not produce a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SubmitError {
    /// The registry refused on its merits. Retrying sends the same bytes into the same state
    /// and gets the same answer, so it is not retried.
    Permanent(String),
    /// Transport, process or network trouble — the claim may still land.
    Transient(String),
}

/// Run `stellar contract invoke … attest` once; `Ok(tx_hash)` if a transaction landed.
async fn submit(
    config: &AttestConfig,
    row: &crate::db::AttestationRow,
) -> Result<Option<String>, SubmitError> {
    let mut cmd = invoke_command(config, row);
    let child = cmd.spawn().map_err(|e| {
        SubmitError::Transient(format!("could not run {}: {e}", config.cli.display()))
    })?;

    let output = match tokio::time::timeout(INVOKE_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(SubmitError::Transient(format!("stellar-cli failed: {e}"))),
        // `kill_on_drop` reaps the process as the child is dropped here.
        Err(_) => {
            return Err(SubmitError::Transient(format!(
                "stellar-cli did not finish within {}s",
                INVOKE_TIMEOUT.as_secs()
            )))
        }
    };

    let stderr = redact(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(classify(&stderr));
    }
    Ok(transaction_hash(&stderr))
}

/// The exact `stellar contract invoke … attest` this claim would be submitted with.
///
/// Separate from [`submit`] so the arguments and the environment can be asserted on without
/// running anything: this command is the whole interface to the chain, and the two properties
/// that matter — that it names *this* verifier, and that it cannot be pointed at another
/// keystore — are properties of the command, not of any particular run.
fn invoke_command(
    config: &AttestConfig,
    row: &crate::db::AttestationRow,
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(&config.cli);
    cmd.arg("contract")
        .arg("invoke")
        .arg("--id")
        .arg(&config.registry_id)
        .arg("--source-account")
        .arg(IDENTITY_ALIAS)
        .arg("--rpc-url")
        .arg(&config.rpc_url)
        .arg("--network-passphrase")
        .arg(&config.network_passphrase)
        // Always submit. Without it the CLI decides from the simulation, and a read-only
        // outcome would silently leave nothing on-chain.
        .arg("--send=yes")
        // `--config-dir` as well as the environment variable: an inherited STELLAR_CONFIG_HOME
        // must not be able to point the CLI at a keystore holding other verifiers' keys.
        .arg("--config-dir")
        .arg(&config.signer.config_home)
        .arg("--")
        .arg("attest")
        .arg("--verifier")
        .arg(&config.signer.address)
        .arg("--wasm_hash")
        .arg(&row.wasm_hash)
        .arg("--input_digest")
        .arg(&row.input_digest)
        .arg("--rebuilt_hash")
        .arg(&row.rebuilt_hash);
    config.signer.apply_env(&mut cmd);
    // No stdin: a CLI that ever asked for confirmation must fail, not block the worker.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

/// Turn a failed invocation into a verdict on whether to try again.
///
/// A contract error is deterministic: the registry looked at the claim and said no, and the
/// same bytes against the same state will get the same answer. The three that `attest` can
/// actually return are named, because an operator reading the row should not have to look up
/// an error number. Everything else — a dropped connection, an RPC 502, a rate limit — is
/// transport, and transport gets another go.
fn classify(stderr: &str) -> SubmitError {
    match contract_error_code(stderr) {
        // Error(Contract, #2) — the verifier is not staked, or is below `min_stake`. Fixing it
        // is an operator action (stake more), not something a retry loop can wait out.
        Some(2) => SubmitError::Permanent(
            "registry refused: this verifier is not active — its stake is below the minimum (#2)"
                .into(),
        ),
        // #3 — the claim's 60-ledger window closed. It can never reopen.
        Some(3) => SubmitError::Permanent(
            "registry refused: the claim's attestation window has closed (#3)".into(),
        ),
        // #4 — we already attested this claim. Expected when a retry follows a submission we
        // failed to observe; the attestation is on-chain either way.
        Some(4) => SubmitError::Permanent(
            "registry refused: this verifier has already attested this claim (#4)".into(),
        ),
        Some(code) => SubmitError::Permanent(format!(
            "registry refused with contract error #{code}; see the registry's Error enum"
        )),
        None => SubmitError::Transient(last_meaningful_line(stderr)),
    }
}

/// The `N` in a `Error(Contract, #N)` anywhere in the CLI's output.
fn contract_error_code(stderr: &str) -> Option<u32> {
    let rest = stderr.split("Error(Contract, #").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The transaction hash the CLI reported, if it named one.
///
/// Two places carry it: `Signing transaction: <hash>`, printed before submission, and the
/// explorer link printed after it succeeded. The link is preferred — it only appears once the
/// transaction is actually in, so it cannot name a transaction that was merely built.
fn transaction_hash(stderr: &str) -> Option<String> {
    fn token_after(text: &str, marker: &str) -> Option<String> {
        let rest = text.split(marker).nth(1)?.trim_start();
        let token: String = rest
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        (token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit())).then_some(token)
    }
    ["/tx/", "Signing transaction:"]
        .into_iter()
        .find_map(|marker| token_after(stderr, marker))
}

/// The last line with anything in it — what a reader wants out of a CLI's error output.
fn last_meaningful_line(stderr: &str) -> String {
    let line = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("stellar-cli failed with no output");
    // The column is read by humans and served to no one, but it is still a database column:
    // keep a runaway log from becoming the row.
    line.chars().take(500).collect()
}

/// Blank out anything shaped like a Stellar secret key.
///
/// Nothing on this path puts the key where the CLI could echo it, so this should never fire.
/// It exists because "should never" is not a property of a text stream we do not control, and
/// the output it guards is written to the database and to the log.
fn redact(text: &str) -> String {
    /// `S` then 55 base32 characters — the shape of an ed25519 secret seed strkey.
    fn is_secret(word: &str) -> bool {
        word.len() == 56
            && word.starts_with('S')
            && word[1..]
                .bytes()
                .all(|b| b.is_ascii_uppercase() || matches!(b, b'2'..=b'7'))
    }
    // Split on characters that cannot appear inside a strkey, so a key is one word however it
    // is punctuated around.
    text.split_inclusive(|c: char| !c.is_ascii_alphanumeric())
        .map(|piece| {
            let (word, tail) = match piece.chars().last() {
                Some(c) if !c.is_ascii_alphanumeric() => piece.split_at(piece.len() - c.len_utf8()),
                _ => (piece, ""),
            };
            if is_secret(word) {
                format!("S…redacted{tail}")
            } else {
                piece.to_string()
            }
        })
        .collect()
}

/// A private `stellar-cli` keystore holding exactly one key, and that key's address.
///
/// The address is derived by the CLI from the key itself, so it cannot disagree with what is
/// about to sign — which matters, because `attest(verifier, …)` names the account whose
/// authorization the registry then demands.
pub struct Signer {
    config_home: PathBuf,
    /// `G…` — the account attestations are filed under.
    pub address: String,
}

impl Signer {
    /// Build the keystore at `config_home` from the secret in `key_file`, and resolve its
    /// address.
    ///
    /// Rewritten on every start, so the file the operator points at stays the single source of
    /// truth and a rotated key takes effect on a restart.
    pub fn provision(cli: &Path, key_file: &Path, config_home: &Path) -> anyhow::Result<Signer> {
        let secret = read_secret_key(key_file)?;

        let identity_dir = config_home.join("identity");
        std::fs::create_dir_all(&identity_dir)
            .with_context(|| format!("creating keystore at {}", config_home.display()))?;
        refuse_foreign_identities(&identity_dir)?;
        restrict(config_home)?;
        restrict(&identity_dir)?;

        let identity_file = identity_dir.join(format!("{IDENTITY_ALIAS}.toml"));
        // Written before the permissions are tightened, so the window where the file exists
        // world-readable is closed by `restrict` below rather than left to the umask.
        std::fs::write(&identity_file, format!("secret_key = \"{secret}\"\n"))
            .with_context(|| format!("writing {}", identity_file.display()))?;
        restrict(&identity_file)?;
        drop(secret);

        let config_home = config_home.to_path_buf();
        let address = resolve_address(cli, &config_home)?;
        Ok(Signer {
            config_home,
            address,
        })
    }

    /// Point a child `stellar` process at this keystore and nothing else.
    fn apply_env(&self, cmd: &mut tokio::process::Command) {
        cmd.env("STELLAR_CONFIG_HOME", &self.config_home);
        // An inherited environment must not be able to redirect the account, the network or
        // the signer out from under the explicit flags.
        for name in [
            "STELLAR_ACCOUNT",
            "STELLAR_SIGN_WITH_KEY",
            "STELLAR_SIGN_WITH_LAB",
            "STELLAR_SIGN_WITH_LEDGER",
            "STELLAR_NETWORK",
            "STELLAR_RPC_URL",
            "STELLAR_NETWORK_PASSPHRASE",
            "STELLAR_CONTRACT_ID",
        ] {
            cmd.env_remove(name);
        }
    }
}

/// Ask the CLI for the address of the key just stored in `config_home`.
///
/// Deriving it rather than configuring it separately is what stops the two from disagreeing:
/// the account named in `attest(verifier, …)` is by construction the one whose key is about to
/// sign, so a mismatched pair cannot be produced by a typo in the environment.
fn resolve_address(cli: &Path, config_home: &Path) -> anyhow::Result<String> {
    let output = std::process::Command::new(cli)
        .arg("keys")
        .arg("public-key")
        .arg(IDENTITY_ALIAS)
        .arg("--config-dir")
        .arg(config_home)
        .env("STELLAR_CONFIG_HOME", config_home)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {} keys public-key", cli.display()))?;

    if !output.status.success() {
        bail!(
            "{} could not read the verifier's address: {}",
            cli.display(),
            last_meaningful_line(&redact(&String::from_utf8_lossy(&output.stderr)))
        );
    }
    let address = redact(&String::from_utf8_lossy(&output.stdout))
        .trim()
        .to_string();
    // Parse rather than trust: an address that is not a G-strkey would be rejected by the
    // registry later, at a point where the cause is far less obvious.
    stellar_strkey::ed25519::PublicKey::from_string(&address)
        .map_err(|_| anyhow::anyhow!("`{address}` is not a Stellar account address (G…)"))?;
    Ok(address)
}

/// The `S…` secret in `key_file`, validated but never echoed.
///
/// Every error here names the file and says what is wrong with it, and none of them contains
/// any part of the contents: the whole point of the file is that its bytes go nowhere except
/// the keystore.
fn read_secret_key(key_file: &Path) -> anyhow::Result<String> {
    let raw = std::fs::read_to_string(key_file)
        .with_context(|| format!("reading the attestation key from {}", key_file.display()))?;
    let secret = raw.trim().to_string();
    if secret.is_empty() {
        bail!("{} is empty", key_file.display());
    }
    stellar_strkey::ed25519::PrivateKey::from_string(&secret).map_err(|_| {
        anyhow::anyhow!(
            "{} does not hold a Stellar secret key (S…); its contents are not shown",
            key_file.display()
        )
    })?;
    Ok(secret)
}

/// Refuse a keystore that already holds somebody else's key.
///
/// This is the whole mitigation for the finding in HACKATHON.md: `stellar-cli` will sign an
/// auth entry with any key it finds here. One key means one thing it can sign as. Sharing a
/// config home between instances — or pointing this at the operator's own `~/.config/stellar`
/// — would hand every instance every verifier's key, so it is an error and not a warning.
fn refuse_foreign_identities(identity_dir: &Path) -> anyhow::Result<()> {
    let ours = format!("{IDENTITY_ALIAS}.toml");
    let entries = std::fs::read_dir(identity_dir)
        .with_context(|| format!("reading {}", identity_dir.display()))?;
    for entry in entries {
        let name = entry
            .with_context(|| format!("reading {}", identity_dir.display()))?
            .file_name();
        if name != std::ffi::OsStr::new(ours.as_str()) {
            bail!(
                "{} already holds the identity `{}`. The attestation keystore must contain \
                 only this instance's key, because stellar-cli signs an auth entry with any \
                 key it finds there — point SOROFY_ATTEST_CONFIG_HOME at a directory of this \
                 instance's own",
                identity_dir.display(),
                name.to_string_lossy(),
            );
        }
    }
    Ok(())
}

/// Owner-only permissions (0700 for a directory, 0600 for a file). A no-op off Unix.
#[cfg(unix)]
fn restrict(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path)
        .with_context(|| format!("reading permissions of {}", path.display()))?;
    let mode = if meta.is_dir() { 0o700 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("restricting permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- error classification ------------------------------------------------------------

    /// The exact shape stellar-cli 28.0.0 prints when the registry refuses during simulation
    /// (captured from a real testnet invocation against the deployed registry).
    fn refusal(code: u32) -> String {
        format!(
            "ℹ️  Simulating transaction…\n\
             ❌ error: transaction simulation failed: HostError: Error(Contract, #{code})\n\
             \n\
             Event log (newest first):\n   \
             0: [Diagnostic Event] contract:CA4VYPAG…, topics:[error, Error(Contract, #{code})], \
             data:\"escalating Ok(ScErrorType::Contract) frame-exit to Err\"\n"
        )
    }

    /// The three the brief pins as permanent: under-staked, closed window, duplicate.
    #[test]
    fn the_three_registry_refusals_are_permanent_and_named() {
        for (code, expected) in [(2, "not active"), (3, "window has closed"), (4, "already")] {
            match classify(&refusal(code)) {
                SubmitError::Permanent(why) => assert!(
                    why.contains(expected) && why.contains(&format!("#{code}")),
                    "#{code} should read plainly, got: {why}"
                ),
                other => panic!("#{code} must not be retried, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_unrecognised_contract_error_is_still_not_retried() {
        // A deterministic refusal is deterministic whether or not we have a name for it.
        match classify(&refusal(13)) {
            SubmitError::Permanent(why) => assert!(why.contains("#13"), "got: {why}"),
            other => panic!("expected a permanent refusal, got {other:?}"),
        }
    }

    #[test]
    fn transport_trouble_is_retried_and_keeps_the_reason() {
        for stderr in [
            "ℹ️  Simulating transaction…\n❌ error: error sending request for url \
             (https://soroban-testnet.stellar.org/): connection closed\n",
            "❌ error: transaction submission failed: 504 Gateway Timeout\n",
            "",
        ] {
            match classify(stderr) {
                SubmitError::Transient(why) => assert!(!why.is_empty()),
                other => panic!("expected a retry, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_host_error_that_is_not_a_contract_error_is_transport() {
        // Budget exhaustion and the like come back as other ScErrorTypes; they are not the
        // registry refusing a claim, so they get another go.
        assert!(matches!(
            classify("❌ error: HostError: Error(Budget, #2)\n"),
            SubmitError::Transient(_)
        ));
    }

    // ---- reading the transaction back ----------------------------------------------------

    /// Real stellar-cli 28.0.0 output from a successful submission.
    const SUCCESS: &str = "ℹ️  Simulating transaction…\n\
        ℹ️  Signing transaction: \
        b7c50c102cfb435711015c3b23e5e80c18864297ab60befb480d9b0b78fdf85b\n\
        🌎 Sending transaction…\n\
        ✅ Transaction submitted successfully!\n\
        🔗 https://stellar.expert/explorer/testnet/tx/\
        b7c50c102cfb435711015c3b23e5e80c18864297ab60befb480d9b0b78fdf85b\n";

    #[test]
    fn the_transaction_hash_is_read_from_the_cli_output() {
        assert_eq!(
            transaction_hash(SUCCESS).as_deref(),
            Some("b7c50c102cfb435711015c3b23e5e80c18864297ab60befb480d9b0b78fdf85b")
        );
    }

    #[test]
    fn output_without_a_hash_names_no_transaction_rather_than_inventing_one() {
        assert_eq!(transaction_hash(""), None);
        assert_eq!(
            transaction_hash("✅ Transaction submitted successfully!"),
            None
        );
        // Something hash-shaped but the wrong length is not a transaction hash.
        assert_eq!(transaction_hash("ℹ️  Signing transaction: abc123\n"), None);
    }

    // ---- the key never escapes -----------------------------------------------------------

    #[test]
    fn anything_shaped_like_a_secret_key_is_scrubbed_from_captured_output() {
        let secret = "SCJKQJ2LKMXGPGYHWFPQ5ZLQXGXGKQZ4NHLMQ7NPTTJMHVVQWZ5BFN2X";
        assert_eq!(secret.len(), 56, "fixture must be strkey-shaped");

        for text in [
            format!("error: could not use {secret} as a source"),
            format!("--source-account={secret}\n"),
            format!("[{secret}]"),
        ] {
            let scrubbed = redact(&text);
            assert!(!scrubbed.contains(secret), "leaked: {scrubbed}");
            assert!(scrubbed.contains("S…redacted"), "no marker: {scrubbed}");
        }
    }

    #[test]
    fn redaction_leaves_ordinary_output_alone() {
        // Addresses, contract ids and hashes must survive: they are the record.
        let text = "verifier GAUNIN5OOJJDWPLCIINMXHKUKIAUGQZZ3YQMWOTHULFB2VFVOPSIDCHD attested \
                    on CA4VYPAGEYYOV7CJIBTCJHOGW2KAFQ4AHYEZIY2NA3NXFJGGG4XSPCFE";
        assert_eq!(redact(text), text);
        assert_eq!(redact(SUCCESS), SUCCESS);
    }

    #[test]
    fn a_key_file_error_never_quotes_the_file() {
        let dir = TempDir::new("keyfile");
        let path = dir.0.join("key");

        std::fs::write(&path, "not-a-key-but-secret-looking").unwrap();
        let err = format!("{:#}", read_secret_key(&path).expect_err("must be refused"));
        assert!(
            !err.contains("not-a-key-but-secret-looking"),
            "leaked: {err}"
        );
        assert!(err.contains("not shown"), "unhelpful: {err}");

        std::fs::write(&path, "   \n").unwrap();
        assert!(format!("{:#}", read_secret_key(&path).expect_err("empty")).contains("is empty"));
    }

    #[test]
    fn a_valid_key_file_is_accepted_with_surrounding_whitespace() {
        let dir = TempDir::new("keyfile-ok");
        let path = dir.0.join("key");
        // The crate redacts `PrivateKey` everywhere by default — `as_unredacted` is how it
        // makes you ask for the printable form, and a test fixture is a fair place to.
        let secret = stellar_strkey::ed25519::PrivateKey([7u8; 32])
            .as_unredacted()
            .to_string()
            .as_str()
            .to_string();
        std::fs::write(&path, format!("\n  {secret}  \n")).unwrap();
        assert_eq!(read_secret_key(&path).unwrap(), secret);
    }

    // ---- keystore isolation --------------------------------------------------------------

    #[test]
    fn a_keystore_holding_another_verifiers_key_is_refused() {
        let dir = TempDir::new("shared-keystore");
        let identities = dir.0.join("identity");
        std::fs::create_dir_all(&identities).unwrap();

        // An empty keystore, and one holding only our own alias, are both fine.
        assert!(refuse_foreign_identities(&identities).is_ok());
        std::fs::write(identities.join(format!("{IDENTITY_ALIAS}.toml")), "").unwrap();
        assert!(refuse_foreign_identities(&identities).is_ok());

        // A second key is the hazard: stellar-cli would sign an auth entry with it.
        std::fs::write(identities.join("verifier_b.toml"), "").unwrap();
        let err = format!(
            "{:#}",
            refuse_foreign_identities(&identities).expect_err("must refuse")
        );
        assert!(err.contains("verifier_b.toml"), "unhelpful: {err}");
        assert!(
            err.contains("SOROFY_ATTEST_CONFIG_HOME"),
            "no fix offered: {err}"
        );
    }

    // ---- backoff -------------------------------------------------------------------------

    #[test]
    fn backoff_doubles_then_holds_at_the_cap() {
        let secs: Vec<u64> = (1..=MAX_ATTEMPTS).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [5, 10, 20, 40, 80, 160, 300, 300, 300, 300]);

        // The schedule has to outlast the 60-ledger (≈5 min) attest window, or a claim would
        // be abandoned while it could still have landed.
        let total: u64 = secs.iter().take(MAX_ATTEMPTS as usize - 1).sum();
        assert!(total > 5 * 60, "gives up too early: {total}s");
    }

    // ---- the command that reaches the chain ----------------------------------------------

    fn row(id: i64) -> crate::db::AttestationRow {
        crate::db::AttestationRow {
            id,
            verification_id: 7,
            wasm_hash: "aa".repeat(32),
            input_digest: "bb".repeat(32),
            rebuilt_hash: "cc".repeat(32),
            status: crate::db::AttestationStatus::Pending,
            tx_hash: None,
            attempts: 0,
            last_error: None,
            created_at: "2026-09-19T12:00:00Z".into(),
            updated_at: "2026-09-19T12:00:00Z".into(),
        }
    }

    fn config(cli: PathBuf, config_home: PathBuf, address: &str) -> AttestConfig {
        AttestConfig {
            cli,
            registry_id: "CA4VYPAGEYYOV7CJIBTCJHOGW2KAFQ4AHYEZIY2NA3NXFJGGG4XSPCFE".into(),
            rpc_url: "https://soroban-testnet.stellar.org".into(),
            network_passphrase: TESTNET_PASSPHRASE.into(),
            signer: Signer {
                config_home,
                address: address.into(),
            },
        }
    }

    fn fake_address() -> String {
        format!("{}", stellar_strkey::ed25519::PublicKey([3u8; 32]))
    }

    #[test]
    fn the_invocation_names_this_verifier_and_this_claim() {
        let home = PathBuf::from("/srv/sorofy/keystore");
        let address = fake_address();
        let cmd = invoke_command(
            &config(PathBuf::from("stellar"), home.clone(), &address),
            &row(1),
        );
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        // The contract call, in the shape `stellar contract invoke` wants it.
        let attest = args
            .iter()
            .position(|a| a == "attest")
            .expect("calls attest");
        assert_eq!(
            args[attest - 1],
            "--",
            "contract args come after the separator"
        );
        for (flag, value) in [
            ("--verifier", address.as_str()),
            ("--wasm_hash", &"aa".repeat(32)),
            ("--input_digest", &"bb".repeat(32)),
            ("--rebuilt_hash", &"cc".repeat(32)),
        ] {
            let at = args.iter().position(|a| a == flag).expect(flag);
            assert!(at > attest, "{flag} must be an argument of attest");
            assert_eq!(args[at + 1], value, "{flag}");
        }

        // Always submit, and always this instance's keystore — the flag beats any inherited
        // STELLAR_CONFIG_HOME, so isolation cannot be undone from the environment.
        assert!(args.contains(&"--send=yes".to_string()));
        let dir = args
            .iter()
            .position(|a| a == "--config-dir")
            .expect("pins the keystore");
        assert_eq!(PathBuf::from(&args[dir + 1]), home);
        // Signing is by the one alias in that keystore, never by a key on the command line.
        let source = args
            .iter()
            .position(|a| a == "--source-account")
            .expect("source");
        assert_eq!(args[source + 1], IDENTITY_ALIAS);
        assert!(
            !args.iter().any(|a| a.starts_with('S') && a.len() == 56),
            "no secret may appear on the command line: {args:?}"
        );
    }

    #[test]
    fn the_child_environment_is_pinned_to_this_keystore_and_stripped_of_the_rest() {
        let home = PathBuf::from("/srv/sorofy/keystore");
        let cmd = invoke_command(
            &config(PathBuf::from("stellar"), home.clone(), &fake_address()),
            &row(1),
        );
        let env: Vec<(String, Option<String>)> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();

        assert!(
            env.contains(&(
                "STELLAR_CONFIG_HOME".into(),
                Some(home.display().to_string())
            )),
            "{env:?}"
        );
        // Everything that could redirect the account, the network or the signer is removed
        // (`None`) rather than left to whatever the operator's shell happened to export.
        for name in [
            "STELLAR_ACCOUNT",
            "STELLAR_SIGN_WITH_KEY",
            "STELLAR_SIGN_WITH_LAB",
            "STELLAR_SIGN_WITH_LEDGER",
            "STELLAR_NETWORK",
            "STELLAR_RPC_URL",
            "STELLAR_NETWORK_PASSPHRASE",
            "STELLAR_CONTRACT_ID",
        ] {
            assert!(
                env.contains(&(name.into(), None)),
                "{name} must be cleared for the child: {env:?}"
            );
        }
    }

    // ---- against a stand-in CLI ----------------------------------------------------------

    /// A script standing in for `stellar`: it answers `keys public-key` with `address`, logs
    /// every invocation, and replies to anything else with `stderr` and `exit_code`.
    ///
    /// Unix only — the point is to drive the real spawn/parse path, and a `.cmd` twin would
    /// test a code path this service does not deploy on.
    #[cfg(unix)]
    fn fake_cli(dir: &Path, address: &str, stderr: &str, exit_code: i32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(dir.join("stderr"), stderr).unwrap();
        let script = dir.join("stellar");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 echo \"$*\" >> {dir}/invocations\n\
                 case \"$*\" in *public-key*) echo {address}; exit 0;; esac\n\
                 cat {dir}/stderr >&2\n\
                 exit {exit_code}\n",
                dir = dir.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    fn write_key(dir: &Path) -> PathBuf {
        let path = dir.join("verifier.key");
        let secret = stellar_strkey::ed25519::PrivateKey([9u8; 32])
            .as_unredacted()
            .to_string()
            .as_str()
            .to_string();
        std::fs::write(&path, secret).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn provisioning_writes_one_private_identity_and_derives_its_address() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("provision");
        let address = fake_address();
        let cli = fake_cli(&dir.0, &address, "", 0);
        let home = dir.0.join("keystore");

        let signer = Signer::provision(&cli, &write_key(&dir.0), &home).expect("provisions");
        assert_eq!(
            signer.address, address,
            "the address comes from the key itself"
        );

        // Exactly one identity, and it is not readable by anyone else on the box.
        let identity = home.join("identity").join(format!("{IDENTITY_ALIAS}.toml"));
        let stored = std::fs::read_to_string(&identity).unwrap();
        assert!(stored.starts_with("secret_key = \"S"), "{stored}");
        assert_eq!(
            std::fs::metadata(&identity).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(home.join("identity"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(std::fs::read_dir(home.join("identity")).unwrap().count(), 1);

        // Re-provisioning is how a rotated key takes effect, so it must not trip its own
        // "somebody else's key is in here" guard.
        assert!(Signer::provision(&cli, &write_key(&dir.0), &home).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_successful_invocation_yields_the_transaction_it_submitted() {
        let dir = TempDir::new("submit-ok");
        let address = fake_address();
        let cli = fake_cli(&dir.0, &address, SUCCESS, 0);
        let home = dir.0.join("keystore");
        Signer::provision(&cli, &write_key(&dir.0), &home).unwrap();

        let tx = submit(&config(cli, home.clone(), &address), &row(1))
            .await
            .expect("the CLI succeeded");
        assert_eq!(
            tx.as_deref(),
            Some("b7c50c102cfb435711015c3b23e5e80c18864297ab60befb480d9b0b78fdf85b")
        );

        // The claim really went out as the registry's own argument names.
        let log = std::fs::read_to_string(dir.0.join("invocations")).unwrap();
        let invoke = log
            .lines()
            .find(|l| l.contains("contract invoke"))
            .expect("invoked");
        assert!(
            invoke.contains(&format!("attest --verifier {address}")),
            "{invoke}"
        );
        assert!(
            invoke.contains(&format!("--wasm_hash {}", "aa".repeat(32))),
            "{invoke}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_refused_invocation_comes_back_as_a_permanent_refusal() {
        let dir = TempDir::new("submit-refused");
        let address = fake_address();
        let cli = fake_cli(&dir.0, &address, &refusal(4), 1);
        let home = dir.0.join("keystore");
        Signer::provision(&cli, &write_key(&dir.0), &home).unwrap();

        match submit(&config(cli, home, &address), &row(1)).await {
            Err(SubmitError::Permanent(why)) => assert!(why.contains("already attested")),
            other => panic!("expected a permanent refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_missing_cli_is_a_retry_rather_than_a_crash() {
        let dir = TempDir::new("submit-nocli");
        let missing = dir.0.join("definitely-not-here");
        match submit(&config(missing, dir.0.clone(), &fake_address()), &row(1)).await {
            Err(SubmitError::Transient(why)) => assert!(why.contains("could not run"), "{why}"),
            other => panic!("expected a retry, got {other:?}"),
        }
    }

    // ---- the worker ----------------------------------------------------------------------

    /// Wait for `check` to hold, or give up. Keeps the worker's own timing out of the test.
    #[cfg(unix)]
    async fn eventually(mut check: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if check() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_worker_drains_what_was_queued_before_it_started_and_what_arrives_after() {
        let dir = TempDir::new("worker");
        let address = fake_address();
        let cli = fake_cli(&dir.0, &address, SUCCESS, 0);
        let home = dir.0.join("keystore");
        Signer::provision(&cli, &write_key(&dir.0), &home).unwrap();

        let db = crate::db::Db::open_in_memory().unwrap();
        let verification = |n: u8| {
            db.insert_pending(None, &format!("{n:064x}"), &serde_json::json!({}), "img")
                .unwrap()
        };

        // A row left behind by a previous process. `fail_orphaned_pending` does not touch the
        // outbox, so the worker must pick this up on its own at startup.
        let restarted = verification(1);
        db.enqueue_attestation(
            restarted,
            &"aa".repeat(32),
            &"bb".repeat(32),
            &"cc".repeat(32),
        )
        .unwrap()
        .expect("queued before the worker exists");

        let attestor = Attestor::start(config(cli, home, &address), db.clone());

        // And one that arrives while the worker is already idle: the notify path, not the poll.
        let live = verification(2);
        attestor.enqueue(live, &crate::claim::tests::attestable_report());

        for (id, what) in [
            (restarted, "queued before startup"),
            (live, "queued while running"),
        ] {
            assert!(
                eventually(|| {
                    db.attestation_for(id)
                        .unwrap()
                        .is_some_and(|r| r.status == crate::db::AttestationStatus::Submitted)
                })
                .await,
                "the attestation {what} was never submitted"
            );
            let row = db.attestation_for(id).unwrap().unwrap();
            assert_eq!(
                row.tx_hash.as_deref(),
                Some("b7c50c102cfb435711015c3b23e5e80c18864297ab60befb480d9b0b78fdf85b")
            );
            assert_eq!(row.attempts, 1);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_refused_claim_is_recorded_and_left_alone() {
        let dir = TempDir::new("worker-refused");
        let address = fake_address();
        // The window closed: nothing about retrying could change the answer.
        let cli = fake_cli(&dir.0, &address, &refusal(3), 1);
        let home = dir.0.join("keystore");
        Signer::provision(&cli, &write_key(&dir.0), &home).unwrap();

        let db = crate::db::Db::open_in_memory().unwrap();
        let id = db
            .insert_pending(None, &"aa".repeat(32), &serde_json::json!({}), "img")
            .unwrap();
        let attestor = Attestor::start(config(cli, home, &address), db.clone());
        attestor.enqueue(id, &crate::claim::tests::attestable_report());

        assert!(
            eventually(|| {
                db.attestation_for(id)
                    .unwrap()
                    .is_some_and(|r| r.status == crate::db::AttestationStatus::Rejected)
            })
            .await,
            "a permanent refusal should reach the row"
        );
        let row = db.attestation_for(id).unwrap().unwrap();
        assert!(row
            .last_error
            .as_deref()
            .unwrap()
            .contains("window has closed"));
        assert_eq!(
            row.attempts, 1,
            "a permanent refusal is attempted once, not retried"
        );
        assert!(db.due_attestation().unwrap().is_none());
    }

    // ---- the flag ------------------------------------------------------------------------

    #[test]
    fn a_disabled_attestor_queues_nothing_at_all() {
        let db = crate::db::Db::open_in_memory().unwrap();
        let id = db
            .insert_pending(
                Some("CABC"),
                &"aa".repeat(32),
                &serde_json::json!({}),
                "img",
            )
            .unwrap();

        let attestor = Attestor::disabled();
        assert!(!attestor.is_enabled());
        assert_eq!(attestor.verifier_address(), None);
        // The report is the one that *would* attest cleanly, so nothing but the flag is
        // keeping this row out of the outbox.
        attestor.enqueue(id, &crate::claim::tests::attestable_report());

        assert!(db.attestation_for(id).unwrap().is_none());
    }

    /// A throwaway directory, removed on drop.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "sorofy-attest-{tag}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
