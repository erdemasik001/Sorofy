//! Sorofy verifier registry — stake, attest, conservative consensus, slash.
//!
//! Design: `docs/hackathon-design.md`. Testnet prototype, not audited.
//!
//! What this contract is and is not:
//! - Verifiers stake VRFY (a SEP-41 token) and attest the hash they rebuilt for a *claim*
//!   `(wasm_hash, input_digest)`. The contract derives the verdict from the hash itself.
//! - `consensus` is conservative: a green result needs every attester to agree.
//! - `slash` burns part of the stake of a verifier outvoted by a strict majority of a closed,
//!   quorate attestation set. Truth is decided by vote, not by proof.
//! - There are **no admin functions**. Parameters are fixed in the constructor.
#![no_std]

mod claim;
#[cfg(test)]
mod test;

pub use crate::claim::claim_input_digest;

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, panic_with_error,
    token::TokenClient, Address, BytesN, Env, IntoVal, TryFromVal, Val, Vec,
};

const DAY_IN_LEDGERS: u32 = 17280;
const INSTANCE_BUMP_AMOUNT: u32 = 7 * DAY_IN_LEDGERS;
const INSTANCE_LIFETIME_THRESHOLD: u32 = INSTANCE_BUMP_AMOUNT - DAY_IN_LEDGERS;
/// Persistent records (stakes, attestations, slash marks) are extended on every write. On a
/// testnet prototype that is enough; a long-lived deployment would need a keeper to re-bump.
const RECORD_BUMP_AMOUNT: u32 = 30 * DAY_IN_LEDGERS;
const RECORD_LIFETIME_THRESHOLD: u32 = RECORD_BUMP_AMOUNT - DAY_IN_LEDGERS;

const BPS_DENOMINATOR: i128 = 10_000;
/// Fewer than three attesters cannot form a strict majority that is also a quorum.
const MIN_QUORUM: u32 = 3;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    InvalidAmount = 1,
    NotActive = 2,
    WindowClosed = 3,
    AlreadyAttested = 4,
    InsufficientStake = 5,
    NothingToWithdraw = 6,
    StillUnbonding = 7,
    /// Window still open, quorum not reached, or no strict majority.
    ClaimNotDecided = 8,
    NotAttested = 9,
    /// The verifier attested the majority hash; there is nothing to punish.
    NotDissenter = 10,
    AlreadySlashed = 11,
    /// The verifier has no stake left (for example it already withdrew).
    NothingToSlash = 12,
    InvalidConfig = 13,
}

/// Parameters, fixed at construction. There is no setter.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub token: Address,
    /// A verifier is active while `staked >= min_stake`.
    pub min_stake: i128,
    /// Attestations a claim needs before it can be `Verified`, `Mismatch`, `Disputed`, or slashed on.
    pub quorum: u32,
    /// Ledgers, from a claim's first attestation, during which it accepts attestations.
    pub attest_window: u32,
    /// Ledgers between `request_unstake` and `withdraw`. Never shorter than `attest_window`.
    pub unbond_period: u32,
    /// Share of a dissenter's stake (staked + unbonding) burned by a slash, in basis points.
    pub slash_bps: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StakeInfo {
    /// Counts towards being active.
    pub staked: i128,
    /// Waiting out `unbond_period`; still slashable, no longer counts towards being active.
    pub unbonding: i128,
    pub release_ledger: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attestation {
    pub verifier: Address,
    pub rebuilt_hash: BytesN<32>,
    pub ledger: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tally {
    pub total: u32,
    /// Attesters holding the most common rebuilt hash.
    pub top_count: u32,
    pub distinct: u32,
}

/// What a consumer should believe about a claim. Only `Verified` is green.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Consensus {
    /// Nobody has attested this claim.
    NoClaim,
    /// The window is still running. Never green.
    Open(Tally),
    /// The window closed before `quorum` attesters showed up.
    Insufficient(Tally),
    /// Closed, quorate, every attester rebuilt the on-chain hash.
    Verified,
    /// Closed, quorate, every attester rebuilt the same hash, and it is not the on-chain one.
    Mismatch,
    /// Closed, quorate, and attesters disagree. Stays disputed even after a slash.
    Disputed(Tally),
}

#[contracttype]
enum DataKey {
    Config,
    Stake(Address),
    /// Ledger of a claim's first attestation.
    Opened(BytesN<32>, BytesN<32>),
    Attesters(BytesN<32>, BytesN<32>),
    Att(BytesN<32>, BytesN<32>, Address),
    Slashed(BytesN<32>, BytesN<32>, Address),
}

#[contractevent]
pub struct Staked {
    #[topic]
    pub verifier: Address,
    pub amount: i128,
}

#[contractevent]
pub struct UnstakeRequested {
    #[topic]
    pub verifier: Address,
    pub amount: i128,
    pub release_ledger: u32,
}

#[contractevent]
pub struct Withdrawn {
    #[topic]
    pub verifier: Address,
    pub amount: i128,
}

#[contractevent]
pub struct Attested {
    #[topic]
    pub verifier: Address,
    #[topic]
    pub wasm_hash: BytesN<32>,
    pub input_digest: BytesN<32>,
    pub rebuilt_hash: BytesN<32>,
}

#[contractevent]
pub struct Slashed {
    #[topic]
    pub verifier: Address,
    #[topic]
    pub wasm_hash: BytesN<32>,
    pub input_digest: BytesN<32>,
    pub amount: i128,
}

#[contract]
pub struct Registry;

#[contractimpl]
impl Registry {
    pub fn __constructor(
        env: Env,
        token: Address,
        min_stake: i128,
        quorum: u32,
        attest_window: u32,
        unbond_period: u32,
        slash_bps: u32,
    ) {
        // `unbond_period >= attest_window` is what keeps a dissenter's stake in the contract
        // until the claim it attested has closed and can be slashed on.
        if min_stake <= 0
            || quorum < MIN_QUORUM
            || attest_window == 0
            || unbond_period < attest_window
            || slash_bps == 0
            || i128::from(slash_bps) > BPS_DENOMINATOR
        {
            panic_with_error!(&env, Error::InvalidConfig);
        }
        env.storage().instance().set(
            &DataKey::Config,
            &Config {
                token,
                min_stake,
                quorum,
                attest_window,
                unbond_period,
                slash_bps,
            },
        );
    }

    // ---- staking -------------------------------------------------------------------------

    /// Lock `amount` VRFY. The verifier authorizes the transfer into this contract.
    pub fn stake(env: Env, verifier: Address, amount: i128) -> Result<(), Error> {
        verifier.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        bump_instance(&env);
        let cfg = read_config(&env);
        TokenClient::new(&env, &cfg.token).transfer(
            &verifier,
            env.current_contract_address(),
            &amount,
        );

        let mut info = read_stake(&env, &verifier);
        info.staked += amount;
        put(&env, &DataKey::Stake(verifier.clone()), &info);
        Staked { verifier, amount }.publish(&env);
        Ok(())
    }

    /// Start withdrawing `amount`. It leaves the active stake immediately, waits out
    /// `unbond_period`, and stays slashable in the meantime. A second request extends the wait.
    pub fn request_unstake(env: Env, verifier: Address, amount: i128) -> Result<(), Error> {
        verifier.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        bump_instance(&env);
        let cfg = read_config(&env);
        let mut info = read_stake(&env, &verifier);
        if amount > info.staked {
            return Err(Error::InsufficientStake);
        }
        info.staked -= amount;
        info.unbonding += amount;
        info.release_ledger = env.ledger().sequence() + cfg.unbond_period;
        put(&env, &DataKey::Stake(verifier.clone()), &info);
        UnstakeRequested {
            verifier,
            amount,
            release_ledger: info.release_ledger,
        }
        .publish(&env);
        Ok(())
    }

    /// Return everything that has finished unbonding.
    pub fn withdraw(env: Env, verifier: Address) -> Result<i128, Error> {
        verifier.require_auth();
        bump_instance(&env);
        let cfg = read_config(&env);
        let mut info = read_stake(&env, &verifier);
        if info.unbonding <= 0 {
            return Err(Error::NothingToWithdraw);
        }
        if env.ledger().sequence() < info.release_ledger {
            return Err(Error::StillUnbonding);
        }
        let amount = info.unbonding;
        info.unbonding = 0;
        put(&env, &DataKey::Stake(verifier.clone()), &info);
        TokenClient::new(&env, &cfg.token).transfer(
            &env.current_contract_address(),
            &verifier,
            &amount,
        );
        Withdrawn { verifier, amount }.publish(&env);
        Ok(amount)
    }

    // ---- attesting -----------------------------------------------------------------------

    /// Record the hash `verifier` rebuilt for the claim `(wasm_hash, input_digest)`.
    ///
    /// The verdict is not an argument: it is `rebuilt_hash == wasm_hash`, so a verifier cannot
    /// attest an inconsistent pair. The first attestation opens the claim's window.
    pub fn attest(
        env: Env,
        verifier: Address,
        wasm_hash: BytesN<32>,
        input_digest: BytesN<32>,
        rebuilt_hash: BytesN<32>,
    ) -> Result<(), Error> {
        verifier.require_auth();
        bump_instance(&env);
        let cfg = read_config(&env);
        if read_stake(&env, &verifier).staked < cfg.min_stake {
            return Err(Error::NotActive);
        }

        let now = env.ledger().sequence();
        let opened_key = DataKey::Opened(wasm_hash.clone(), input_digest.clone());
        match get::<u32>(&env, &opened_key) {
            Some(opened) => {
                if now >= opened + cfg.attest_window {
                    return Err(Error::WindowClosed);
                }
            }
            None => put(&env, &opened_key, &now),
        }

        let att_key = DataKey::Att(wasm_hash.clone(), input_digest.clone(), verifier.clone());
        if get::<Attestation>(&env, &att_key).is_some() {
            return Err(Error::AlreadyAttested);
        }
        put(
            &env,
            &att_key,
            &Attestation {
                verifier: verifier.clone(),
                rebuilt_hash: rebuilt_hash.clone(),
                ledger: now,
            },
        );
        let attesters_key = DataKey::Attesters(wasm_hash.clone(), input_digest.clone());
        let mut attesters: Vec<Address> = get(&env, &attesters_key).unwrap_or(Vec::new(&env));
        attesters.push_back(verifier.clone());
        put(&env, &attesters_key, &attesters);

        Attested {
            verifier,
            wasm_hash,
            input_digest,
            rebuilt_hash,
        }
        .publish(&env);
        Ok(())
    }

    // ---- reading -------------------------------------------------------------------------

    /// Every attestation on the claim, in the order it arrived.
    pub fn attestations(
        env: Env,
        wasm_hash: BytesN<32>,
        input_digest: BytesN<32>,
    ) -> Vec<Attestation> {
        read_attestations(&env, &wasm_hash, &input_digest)
    }

    /// What a consumer should believe about the claim. See [`Consensus`].
    pub fn consensus(env: Env, wasm_hash: BytesN<32>, input_digest: BytesN<32>) -> Consensus {
        let cfg = read_config(&env);
        let Some(opened) = get::<u32>(
            &env,
            &DataKey::Opened(wasm_hash.clone(), input_digest.clone()),
        ) else {
            return Consensus::NoClaim;
        };
        let atts = read_attestations(&env, &wasm_hash, &input_digest);
        let (top_hash, tally) = tally_of(&env, &atts);

        if env.ledger().sequence() < opened + cfg.attest_window {
            Consensus::Open(tally)
        } else if tally.total < cfg.quorum {
            Consensus::Insufficient(tally)
        } else if tally.distinct == 1 {
            if top_hash == wasm_hash {
                Consensus::Verified
            } else {
                Consensus::Mismatch
            }
        } else {
            Consensus::Disputed(tally)
        }
    }

    pub fn stake_of(env: Env, verifier: Address) -> StakeInfo {
        read_stake(&env, &verifier)
    }

    /// Whether `verifier` can attest right now.
    pub fn is_active(env: Env, verifier: Address) -> bool {
        read_stake(&env, &verifier).staked >= read_config(&env).min_stake
    }

    pub fn config(env: Env) -> Config {
        read_config(&env)
    }

    // ---- slashing ------------------------------------------------------------------------

    /// Burn `slash_bps` of `verifier`'s stake because a strict majority disagreed with it.
    ///
    /// Callable by anyone; no authorization is needed because the contract, not the caller,
    /// decides whether the call is valid. Succeeds only if the claim's window has closed, at
    /// least `quorum` verifiers attested, one hash is held by a strict majority, and `verifier`
    /// attested a different hash. One slash per `(claim, verifier)`. A 1-1 split, or any split
    /// with no strict majority, slashes nobody.
    ///
    /// Returns the amount burned.
    pub fn slash(
        env: Env,
        wasm_hash: BytesN<32>,
        input_digest: BytesN<32>,
        verifier: Address,
    ) -> Result<i128, Error> {
        bump_instance(&env);
        let cfg = read_config(&env);
        let Some(opened) = get::<u32>(
            &env,
            &DataKey::Opened(wasm_hash.clone(), input_digest.clone()),
        ) else {
            return Err(Error::ClaimNotDecided);
        };
        if env.ledger().sequence() < opened + cfg.attest_window {
            return Err(Error::ClaimNotDecided);
        }
        let atts = read_attestations(&env, &wasm_hash, &input_digest);
        let (top_hash, tally) = tally_of(&env, &atts);
        if tally.total < cfg.quorum || 2 * tally.top_count <= tally.total {
            return Err(Error::ClaimNotDecided);
        }

        let att = get::<Attestation>(
            &env,
            &DataKey::Att(wasm_hash.clone(), input_digest.clone(), verifier.clone()),
        )
        .ok_or(Error::NotAttested)?;
        if att.rebuilt_hash == top_hash {
            return Err(Error::NotDissenter);
        }
        let slashed_key =
            DataKey::Slashed(wasm_hash.clone(), input_digest.clone(), verifier.clone());
        if get::<bool>(&env, &slashed_key).is_some() {
            return Err(Error::AlreadySlashed);
        }

        let mut info = read_stake(&env, &verifier);
        let amount = (info.staked + info.unbonding) * i128::from(cfg.slash_bps) / BPS_DENOMINATOR;
        if amount <= 0 {
            return Err(Error::NothingToSlash);
        }
        // Take from the active stake first, so a verifier cannot keep attesting on stake it
        // has just been punished for by parking the rest in the unbonding queue.
        let from_staked = amount.min(info.staked);
        info.staked -= from_staked;
        info.unbonding -= amount - from_staked;
        put(&env, &DataKey::Stake(verifier.clone()), &info);
        put(&env, &slashed_key, &true);

        // This contract is the holder and the caller, so the token's `from.require_auth()`
        // is satisfied by the contract itself: no external signature is involved.
        TokenClient::new(&env, &cfg.token).burn(&env.current_contract_address(), &amount);

        Slashed {
            verifier,
            wasm_hash,
            input_digest,
            amount,
        }
        .publish(&env);
        Ok(amount)
    }
}

// ---- helpers -----------------------------------------------------------------------------

fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
}

fn read_config(env: &Env) -> Config {
    env.storage()
        .instance()
        .get(&DataKey::Config)
        .unwrap_or_else(|| panic_with_error!(env, Error::InvalidConfig))
}

fn get<V: TryFromVal<Env, Val>>(env: &Env, key: &DataKey) -> Option<V> {
    env.storage().persistent().get(key)
}

fn put<V: IntoVal<Env, Val>>(env: &Env, key: &DataKey, value: &V) {
    env.storage().persistent().set(key, value);
    env.storage()
        .persistent()
        .extend_ttl(key, RECORD_LIFETIME_THRESHOLD, RECORD_BUMP_AMOUNT);
}

fn read_stake(env: &Env, verifier: &Address) -> StakeInfo {
    get(env, &DataKey::Stake(verifier.clone())).unwrap_or(StakeInfo {
        staked: 0,
        unbonding: 0,
        release_ledger: 0,
    })
}

fn read_attestations(
    env: &Env,
    wasm_hash: &BytesN<32>,
    input_digest: &BytesN<32>,
) -> Vec<Attestation> {
    let attesters: Vec<Address> = get(
        env,
        &DataKey::Attesters(wasm_hash.clone(), input_digest.clone()),
    )
    .unwrap_or(Vec::new(env));
    let mut out = Vec::new(env);
    for verifier in attesters.iter() {
        if let Some(att) = get::<Attestation>(
            env,
            &DataKey::Att(wasm_hash.clone(), input_digest.clone(), verifier),
        ) {
            out.push_back(att);
        }
    }
    out
}

/// The most common rebuilt hash, and the counts around it. With no attestations the hash is
/// all zeros and every count is zero; callers only look at it once a claim exists.
fn tally_of(env: &Env, atts: &Vec<Attestation>) -> (BytesN<32>, Tally) {
    let total = atts.len();
    let mut top = BytesN::from_array(env, &[0u8; 32]);
    let mut top_count = 0u32;
    let mut distinct = 0u32;
    for i in 0..total {
        let hash = atts.get(i).unwrap().rebuilt_hash;
        let mut count = 0u32;
        let mut first_seen = true;
        for j in 0..total {
            if atts.get(j).unwrap().rebuilt_hash == hash {
                count += 1;
                if j < i {
                    first_seen = false;
                }
            }
        }
        if first_seen {
            distinct += 1;
        }
        if count > top_count {
            top_count = count;
            top = hash;
        }
    }
    (
        top,
        Tally {
            total,
            top_count,
            distinct,
        },
    )
}
