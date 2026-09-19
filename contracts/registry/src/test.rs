extern crate std;

use crate::{claim_input_digest, Consensus, Error, Registry, RegistryClient, Tally};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    Address, Bytes, BytesN, Env, Vec,
};

const ONE: i128 = 10_000_000; // 10^7 base units = 1 VRFY
const MIN_STAKE: i128 = 1_000 * ONE;
const QUORUM: u32 = 3;
const WINDOW: u32 = 60;
const UNBOND: u32 = 120;
const BPS: u32 = 5_000;
const START_LEDGER: u32 = 1_000;
const FUNDED: i128 = 10_000 * ONE;

struct Fx {
    e: Env,
    reg: RegistryClient<'static>,
    token: TokenClient<'static>,
    a: Address,
    b: Address,
    c: Address,
    d: Address,
}

/// A registry with the demo parameters, backed by a Stellar Asset Contract token (the same
/// SEP-41 interface VRFY implements), and four funded, *un-staked* verifier accounts.
fn setup() -> Fx {
    let e = Env::default();
    e.mock_all_auths();
    e.ledger().set_sequence_number(START_LEDGER);

    let token_addr = e
        .register_stellar_asset_contract_v2(Address::generate(&e))
        .address();
    let reg_id = e.register(
        Registry,
        (&token_addr, MIN_STAKE, QUORUM, WINDOW, UNBOND, BPS),
    );
    let mint = StellarAssetClient::new(&e, &token_addr);
    let [a, b, c, d] = [0; 4].map(|_| Address::generate(&e));
    for v in [&a, &b, &c, &d] {
        mint.mint(v, &FUNDED);
    }
    Fx {
        reg: RegistryClient::new(&e, &reg_id),
        token: TokenClient::new(&e, &token_addr),
        e,
        a,
        b,
        c,
        d,
    }
}

fn h(e: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(e, &[n; 32])
}

fn advance(fx: &Fx, ledgers: u32) {
    let now = fx.e.ledger().sequence();
    fx.e.ledger().set_sequence_number(now + ledgers);
}

fn stake_min(fx: &Fx, verifiers: &[&Address]) {
    for v in verifiers {
        fx.reg.stake(v, &MIN_STAKE);
    }
}

/// Staked a, b, c. a and b rebuilt `wasm` (so they agree with the chain), c rebuilt something
/// else; the window is closed. A strict 2-1 majority.
fn disputed_two_to_one(fx: &Fx, wasm: &BytesN<32>, digest: &BytesN<32>) {
    stake_min(fx, &[&fx.a, &fx.b, &fx.c]);
    let wrong = h(&fx.e, 0xEE);
    fx.reg.attest(&fx.a, wasm, digest, wasm);
    fx.reg.attest(&fx.b, wasm, digest, wasm);
    fx.reg.attest(&fx.c, wasm, digest, &wrong);
    advance(fx, WINDOW);
}

// ---- constructor -------------------------------------------------------------------------

fn register_with(quorum: u32, window: u32, unbond: u32, bps: u32, min_stake: i128) {
    let e = Env::default();
    let token = Address::generate(&e);
    e.register(Registry, (&token, min_stake, quorum, window, unbond, bps));
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn a_quorum_below_three_is_refused() {
    register_with(2, WINDOW, UNBOND, BPS, MIN_STAKE);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn unbonding_shorter_than_the_window_is_refused() {
    // Otherwise a dissenter could withdraw before its claim closes.
    register_with(QUORUM, WINDOW, WINDOW - 1, BPS, MIN_STAKE);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn a_zero_slash_is_refused() {
    register_with(QUORUM, WINDOW, UNBOND, 0, MIN_STAKE);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn a_slash_above_one_hundred_percent_is_refused() {
    register_with(QUORUM, WINDOW, UNBOND, 10_001, MIN_STAKE);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn a_zero_minimum_stake_is_refused() {
    register_with(QUORUM, WINDOW, UNBOND, BPS, 0);
}

#[test]
fn the_config_is_what_was_deployed() {
    let fx = setup();
    let cfg = fx.reg.config();
    assert_eq!(cfg.token, fx.token.address);
    assert_eq!(
        (
            cfg.min_stake,
            cfg.quorum,
            cfg.attest_window,
            cfg.unbond_period,
            cfg.slash_bps
        ),
        (MIN_STAKE, QUORUM, WINDOW, UNBOND, BPS)
    );
}

// ---- staking -----------------------------------------------------------------------------

#[test]
fn stake_moves_tokens_in_and_activates_at_the_minimum() {
    let fx = setup();
    fx.reg.stake(&fx.a, &(MIN_STAKE - 1));
    assert!(
        !fx.reg.is_active(&fx.a),
        "one base unit short is not active"
    );
    assert_eq!(fx.token.balance(&fx.a), FUNDED - (MIN_STAKE - 1));
    assert_eq!(fx.token.balance(&fx.reg.address), MIN_STAKE - 1);

    fx.reg.stake(&fx.a, &1);
    assert!(fx.reg.is_active(&fx.a));
    assert_eq!(fx.reg.stake_of(&fx.a).staked, MIN_STAKE);
}

#[test]
fn stake_rejects_zero_and_negative_amounts() {
    let fx = setup();
    assert_eq!(fx.reg.try_stake(&fx.a, &0), Err(Ok(Error::InvalidAmount)));
    assert_eq!(fx.reg.try_stake(&fx.a, &-5), Err(Ok(Error::InvalidAmount)));
    assert_eq!(fx.token.balance(&fx.a), FUNDED, "nothing moved");
}

#[test]
fn unstaking_leaves_the_active_stake_at_once_but_waits_to_pay_out() {
    let fx = setup();
    stake_min(&fx, &[&fx.a]);
    fx.reg.request_unstake(&fx.a, &(300 * ONE));
    let info = fx.reg.stake_of(&fx.a);
    assert_eq!((info.staked, info.unbonding), (700 * ONE, 300 * ONE));
    assert_eq!(info.release_ledger, START_LEDGER + UNBOND);
    assert!(
        !fx.reg.is_active(&fx.a),
        "below the minimum as soon as it is requested"
    );

    assert_eq!(fx.reg.try_withdraw(&fx.a), Err(Ok(Error::StillUnbonding)));
    advance(&fx, UNBOND - 1);
    assert_eq!(fx.reg.try_withdraw(&fx.a), Err(Ok(Error::StillUnbonding)));
    advance(&fx, 1);
    assert_eq!(fx.reg.withdraw(&fx.a), 300 * ONE);
    assert_eq!(fx.token.balance(&fx.a), FUNDED - MIN_STAKE + 300 * ONE);
    assert_eq!(fx.reg.stake_of(&fx.a).unbonding, 0);
}

#[test]
fn unstake_and_withdraw_refuse_what_is_not_there() {
    let fx = setup();
    stake_min(&fx, &[&fx.a]);
    assert_eq!(
        fx.reg.try_request_unstake(&fx.a, &(MIN_STAKE + 1)),
        Err(Ok(Error::InsufficientStake))
    );
    assert_eq!(
        fx.reg.try_request_unstake(&fx.a, &0),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(
        fx.reg.try_withdraw(&fx.a),
        Err(Ok(Error::NothingToWithdraw))
    );
}

// ---- attesting ---------------------------------------------------------------------------

#[test]
fn attest_needs_the_verifiers_own_authorization() {
    let fx = setup();
    stake_min(&fx, &[&fx.a]);
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));

    // Stop mocking: from here on nobody has authorized anything.
    fx.e.set_auths(&[]);
    assert!(fx.reg.try_attest(&fx.a, &wasm, &digest, &wasm).is_err());
    assert!(fx.reg.try_stake(&fx.a, &ONE).is_err());
    assert!(fx.reg.try_request_unstake(&fx.a, &ONE).is_err());

    fx.e.mock_all_auths();
    assert_eq!(
        fx.reg.consensus(&wasm, &digest),
        Consensus::NoClaim,
        "a refused call leaves no trace"
    );
    assert_eq!(fx.reg.stake_of(&fx.a).staked, MIN_STAKE);
}

#[test]
fn an_unstaked_or_under_staked_account_cannot_attest() {
    let fx = setup();
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));

    // Never staked.
    assert_eq!(
        fx.reg.try_attest(&fx.d, &wasm, &digest, &wasm),
        Err(Ok(Error::NotActive))
    );

    // Staked, but one base unit short.
    fx.reg.stake(&fx.d, &(MIN_STAKE - 1));
    assert_eq!(
        fx.reg.try_attest(&fx.d, &wasm, &digest, &wasm),
        Err(Ok(Error::NotActive))
    );

    // Was active, then started unstaking below the minimum.
    stake_min(&fx, &[&fx.a]);
    fx.reg.request_unstake(&fx.a, &ONE);
    assert_eq!(
        fx.reg.try_attest(&fx.a, &wasm, &digest, &wasm),
        Err(Ok(Error::NotActive))
    );

    assert_eq!(
        fx.reg.consensus(&wasm, &digest),
        Consensus::NoClaim,
        "none of the refusals opened a claim"
    );
}

#[test]
fn a_verifier_cannot_attest_the_same_claim_twice() {
    let fx = setup();
    stake_min(&fx, &[&fx.a]);
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));

    fx.reg.attest(&fx.a, &wasm, &digest, &wasm);
    // Neither repeating it nor changing its mind is allowed.
    assert_eq!(
        fx.reg.try_attest(&fx.a, &wasm, &digest, &wasm),
        Err(Ok(Error::AlreadyAttested))
    );
    assert_eq!(
        fx.reg.try_attest(&fx.a, &wasm, &digest, &h(&fx.e, 9)),
        Err(Ok(Error::AlreadyAttested))
    );
    assert_eq!(fx.reg.attestations(&wasm, &digest).len(), 1);
}

#[test]
fn the_same_verifier_may_attest_a_different_claim() {
    let fx = setup();
    stake_min(&fx, &[&fx.a]);
    let wasm = h(&fx.e, 1);
    fx.reg.attest(&fx.a, &wasm, &h(&fx.e, 2), &wasm);
    fx.reg.attest(&fx.a, &wasm, &h(&fx.e, 3), &wasm); // same wasm, different inputs
    assert_eq!(fx.reg.attestations(&wasm, &h(&fx.e, 2)).len(), 1);
    assert_eq!(fx.reg.attestations(&wasm, &h(&fx.e, 3)).len(), 1);
}

#[test]
fn attesting_closes_with_the_window() {
    let fx = setup();
    stake_min(&fx, &[&fx.a, &fx.b, &fx.c]);
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));

    fx.reg.attest(&fx.a, &wasm, &digest, &wasm);
    advance(&fx, WINDOW - 1);
    fx.reg.attest(&fx.b, &wasm, &digest, &wasm); // last ledger of the window: accepted
    advance(&fx, 1);
    assert_eq!(
        fx.reg.try_attest(&fx.c, &wasm, &digest, &wasm),
        Err(Ok(Error::WindowClosed))
    );
    assert_eq!(fx.reg.attestations(&wasm, &digest).len(), 2);
}

#[test]
fn attestations_come_back_in_order_with_their_ledger() {
    let fx = setup();
    stake_min(&fx, &[&fx.a, &fx.b]);
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));

    fx.reg.attest(&fx.a, &wasm, &digest, &wasm);
    advance(&fx, 5);
    fx.reg.attest(&fx.b, &wasm, &digest, &h(&fx.e, 7));

    let atts = fx.reg.attestations(&wasm, &digest);
    assert_eq!(atts.len(), 2);
    let (first, second) = (atts.get(0).unwrap(), atts.get(1).unwrap());
    assert_eq!(
        (first.verifier, first.rebuilt_hash, first.ledger),
        (fx.a.clone(), wasm, START_LEDGER)
    );
    assert_eq!(
        (second.verifier, second.rebuilt_hash, second.ledger),
        (fx.b.clone(), h(&fx.e, 7), START_LEDGER + 5)
    );
}

// ---- consensus ---------------------------------------------------------------------------

fn tally(total: u32, top_count: u32, distinct: u32) -> Tally {
    Tally {
        total,
        top_count,
        distinct,
    }
}

#[test]
fn consensus_is_never_green_while_the_window_is_open() {
    let fx = setup();
    stake_min(&fx, &[&fx.a, &fx.b, &fx.c]);
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    assert_eq!(fx.reg.consensus(&wasm, &digest), Consensus::NoClaim);

    fx.reg.attest(&fx.a, &wasm, &digest, &wasm);
    assert_eq!(
        fx.reg.consensus(&wasm, &digest),
        Consensus::Open(tally(1, 1, 1))
    );
    fx.reg.attest(&fx.b, &wasm, &digest, &wasm);
    fx.reg.attest(&fx.c, &wasm, &digest, &wasm);
    // Unanimous and quorate, but the window is still running.
    assert_eq!(
        fx.reg.consensus(&wasm, &digest),
        Consensus::Open(tally(3, 3, 1))
    );
}

#[test]
fn a_window_that_closes_short_of_quorum_is_insufficient() {
    let fx = setup();
    stake_min(&fx, &[&fx.a, &fx.b]);
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    fx.reg.attest(&fx.a, &wasm, &digest, &wasm);
    fx.reg.attest(&fx.b, &wasm, &digest, &wasm);
    advance(&fx, WINDOW);
    assert_eq!(
        fx.reg.consensus(&wasm, &digest),
        Consensus::Insufficient(tally(2, 2, 1))
    );
}

#[test]
fn unanimity_on_the_chain_hash_is_verified() {
    let fx = setup();
    stake_min(&fx, &[&fx.a, &fx.b, &fx.c]);
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    for v in [&fx.a, &fx.b, &fx.c] {
        fx.reg.attest(v, &wasm, &digest, &wasm);
    }
    advance(&fx, WINDOW);
    assert_eq!(fx.reg.consensus(&wasm, &digest), Consensus::Verified);
}

#[test]
fn unanimity_on_a_different_hash_is_a_mismatch() {
    let fx = setup();
    stake_min(&fx, &[&fx.a, &fx.b, &fx.c]);
    let (wasm, digest, rebuilt) = (h(&fx.e, 1), h(&fx.e, 2), h(&fx.e, 3));
    for v in [&fx.a, &fx.b, &fx.c] {
        fx.reg.attest(v, &wasm, &digest, &rebuilt);
    }
    advance(&fx, WINDOW);
    assert_eq!(fx.reg.consensus(&wasm, &digest), Consensus::Mismatch);
}

#[test]
fn any_dissent_makes_it_disputed_never_verified() {
    let fx = setup();
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    disputed_two_to_one(&fx, &wasm, &digest);
    // Two of three agree with the chain, and that is still not green.
    assert_eq!(
        fx.reg.consensus(&wasm, &digest),
        Consensus::Disputed(tally(3, 2, 2))
    );
}

#[test]
fn three_different_answers_are_disputed() {
    let fx = setup();
    stake_min(&fx, &[&fx.a, &fx.b, &fx.c]);
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 4));
    fx.reg.attest(&fx.a, &wasm, &digest, &h(&fx.e, 5));
    fx.reg.attest(&fx.b, &wasm, &digest, &h(&fx.e, 6));
    fx.reg.attest(&fx.c, &wasm, &digest, &h(&fx.e, 7));
    advance(&fx, WINDOW);
    assert_eq!(
        fx.reg.consensus(&wasm, &digest),
        Consensus::Disputed(tally(3, 1, 3))
    );
}

// ---- slash -------------------------------------------------------------------------------

#[test]
fn slash_burns_the_dissenters_stake_and_deactivates_it() {
    let fx = setup();
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    disputed_two_to_one(&fx, &wasm, &digest);
    assert_eq!(fx.token.balance(&fx.reg.address), 3 * MIN_STAKE);

    let burned = fx.reg.slash(&wasm, &digest, &fx.c);

    assert_eq!(burned, MIN_STAKE * i128::from(BPS) / 10_000);
    assert_eq!(fx.reg.stake_of(&fx.c).staked, MIN_STAKE - burned);
    assert!(
        !fx.reg.is_active(&fx.c),
        "one slash drops the verifier below the minimum"
    );
    assert_eq!(
        fx.reg.consensus(&wasm, &digest),
        Consensus::Disputed(tally(3, 2, 2)),
        "a slash never turns a dispute green"
    );

    // Burned, not redistributed: the registry lost it, nobody else gained it.
    assert_eq!(fx.token.balance(&fx.reg.address), 3 * MIN_STAKE - burned);
    let held: i128 = [&fx.a, &fx.b, &fx.c, &fx.d, &fx.reg.address]
        .iter()
        .map(|who| fx.token.balance(who))
        .sum();
    assert_eq!(held, 4 * FUNDED - burned);

    // The honest majority is untouched and still active.
    for v in [&fx.a, &fx.b] {
        assert_eq!(fx.reg.stake_of(v).staked, MIN_STAKE);
        assert!(fx.reg.is_active(v));
    }
}

/// The token's `burn(from)` requires `from`'s authorization. Here `from` is the registry
/// itself, calling as the invoker, so the call must go through with *no* signature from
/// anyone. Mocked auths would hide a failure, so this runs with mocking switched off.
#[test]
fn slash_needs_no_external_authorization() {
    let fx = setup();
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    disputed_two_to_one(&fx, &wasm, &digest);

    fx.e.set_auths(&[]);
    let burned = fx.reg.slash(&wasm, &digest, &fx.c);
    assert_eq!(burned, MIN_STAKE / 2);
}

#[test]
fn slash_is_refused_while_the_window_is_open() {
    let fx = setup();
    stake_min(&fx, &[&fx.a, &fx.b, &fx.c]);
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    fx.reg.attest(&fx.a, &wasm, &digest, &wasm);
    fx.reg.attest(&fx.b, &wasm, &digest, &wasm);
    fx.reg.attest(&fx.c, &wasm, &digest, &h(&fx.e, 9));
    // A 2-1 majority exists, but a late attester could still change it.
    assert_eq!(
        fx.reg.try_slash(&wasm, &digest, &fx.c),
        Err(Ok(Error::ClaimNotDecided))
    );
    assert_eq!(fx.reg.stake_of(&fx.c).staked, MIN_STAKE);
}

#[test]
fn slash_is_refused_without_a_quorum() {
    let fx = setup();
    stake_min(&fx, &[&fx.a, &fx.b]);
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    fx.reg.attest(&fx.a, &wasm, &digest, &wasm);
    fx.reg.attest(&fx.b, &wasm, &digest, &h(&fx.e, 9));
    advance(&fx, WINDOW);
    // A 1-1 split: nobody can say who is lying, so nobody is punished.
    assert_eq!(
        fx.reg.try_slash(&wasm, &digest, &fx.a),
        Err(Ok(Error::ClaimNotDecided))
    );
    assert_eq!(
        fx.reg.try_slash(&wasm, &digest, &fx.b),
        Err(Ok(Error::ClaimNotDecided))
    );
    assert_eq!(fx.reg.stake_of(&fx.a).staked, MIN_STAKE);
    assert_eq!(fx.reg.stake_of(&fx.b).staked, MIN_STAKE);
}

#[test]
fn slash_is_refused_without_a_strict_majority() {
    let fx = setup();
    stake_min(&fx, &[&fx.a, &fx.b, &fx.c]);
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    fx.reg.attest(&fx.a, &wasm, &digest, &h(&fx.e, 5));
    fx.reg.attest(&fx.b, &wasm, &digest, &h(&fx.e, 6));
    fx.reg.attest(&fx.c, &wasm, &digest, &h(&fx.e, 7));
    advance(&fx, WINDOW);
    for v in [&fx.a, &fx.b, &fx.c] {
        assert_eq!(
            fx.reg.try_slash(&wasm, &digest, v),
            Err(Ok(Error::ClaimNotDecided))
        );
    }
}

#[test]
fn slash_is_refused_on_a_two_two_tie() {
    // Four attesters, quorum met, but exactly half on each side: no strict majority.
    let fx = setup();
    stake_min(&fx, &[&fx.a, &fx.b, &fx.c, &fx.d]);
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    fx.reg.attest(&fx.a, &wasm, &digest, &wasm);
    fx.reg.attest(&fx.b, &wasm, &digest, &wasm);
    fx.reg.attest(&fx.c, &wasm, &digest, &h(&fx.e, 9));
    fx.reg.attest(&fx.d, &wasm, &digest, &h(&fx.e, 9));
    advance(&fx, WINDOW);
    for v in [&fx.a, &fx.b, &fx.c, &fx.d] {
        assert_eq!(
            fx.reg.try_slash(&wasm, &digest, v),
            Err(Ok(Error::ClaimNotDecided))
        );
    }
}

#[test]
fn slash_is_refused_for_a_verifier_in_the_majority() {
    let fx = setup();
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    disputed_two_to_one(&fx, &wasm, &digest);
    // An unjustified slash: `a` agrees with the majority.
    assert_eq!(
        fx.reg.try_slash(&wasm, &digest, &fx.a),
        Err(Ok(Error::NotDissenter))
    );
    assert_eq!(
        fx.reg.try_slash(&wasm, &digest, &fx.b),
        Err(Ok(Error::NotDissenter))
    );
    assert_eq!(fx.reg.stake_of(&fx.a).staked, MIN_STAKE);
}

#[test]
fn slash_is_refused_for_someone_who_never_attested() {
    let fx = setup();
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    disputed_two_to_one(&fx, &wasm, &digest);
    fx.reg.stake(&fx.d, &MIN_STAKE); // staked, but never attested this claim
    assert_eq!(
        fx.reg.try_slash(&wasm, &digest, &fx.d),
        Err(Ok(Error::NotAttested))
    );
    // And on a claim nobody ever opened.
    assert_eq!(
        fx.reg.try_slash(&wasm, &h(&fx.e, 99), &fx.c),
        Err(Ok(Error::ClaimNotDecided))
    );
}

#[test]
fn a_verifier_is_slashed_once_per_claim() {
    let fx = setup();
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    disputed_two_to_one(&fx, &wasm, &digest);
    fx.reg.slash(&wasm, &digest, &fx.c);
    assert_eq!(
        fx.reg.try_slash(&wasm, &digest, &fx.c),
        Err(Ok(Error::AlreadySlashed))
    );
    assert_eq!(
        fx.reg.stake_of(&fx.c).staked,
        MIN_STAKE / 2,
        "no second burn"
    );
}

#[test]
fn slash_reaches_stake_that_is_already_unbonding() {
    let fx = setup();
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    stake_min(&fx, &[&fx.a, &fx.b, &fx.c]);
    fx.reg.attest(&fx.a, &wasm, &digest, &wasm);
    fx.reg.attest(&fx.b, &wasm, &digest, &wasm);
    fx.reg.attest(&fx.c, &wasm, &digest, &h(&fx.e, 0xEE));
    // The liar tries to park almost everything in the unbonding queue.
    fx.reg.request_unstake(&fx.c, &(900 * ONE));
    advance(&fx, WINDOW);

    let burned = fx.reg.slash(&wasm, &digest, &fx.c);
    assert_eq!(
        burned,
        500 * ONE,
        "half of staked + unbonding, not half of what is still active"
    );
    let info = fx.reg.stake_of(&fx.c);
    // Taken from the active stake first (100), then from the unbonding queue (400).
    assert_eq!((info.staked, info.unbonding), (0, 500 * ONE));
}

#[test]
fn a_slash_that_comes_after_the_withdrawal_finds_nothing() {
    let fx = setup();
    let (wasm, digest) = (h(&fx.e, 1), h(&fx.e, 2));
    stake_min(&fx, &[&fx.a, &fx.b, &fx.c]);
    fx.reg.attest(&fx.a, &wasm, &digest, &wasm);
    fx.reg.attest(&fx.b, &wasm, &digest, &wasm);
    fx.reg.attest(&fx.c, &wasm, &digest, &h(&fx.e, 0xEE));
    fx.reg.request_unstake(&fx.c, &MIN_STAKE);

    // Unbonding is at least as long as the window, so the stake is still there when the claim
    // closes...
    advance(&fx, WINDOW);
    assert!(fx.reg.stake_of(&fx.c).unbonding == MIN_STAKE);
    // ...but if nobody slashes before it is released, it is gone.
    advance(&fx, UNBOND - WINDOW);
    assert_eq!(fx.reg.withdraw(&fx.c), MIN_STAKE);
    assert_eq!(
        fx.reg.try_slash(&wasm, &digest, &fx.c),
        Err(Ok(Error::NothingToSlash))
    );
}

// ---- claim digest ------------------------------------------------------------------------

fn hex32(s: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
    }
    out
}

fn opts(e: &Env, list: &[&str]) -> Vec<Bytes> {
    let mut v = Vec::new(e);
    for o in list {
        v.push_back(Bytes::from_slice(e, o.as_bytes()));
    }
    v
}

/// Expected values were computed independently in Python from the layout in `claim.rs`. The
/// API's implementation must reproduce the same vectors.
#[test]
fn the_claim_digest_matches_the_independent_reference_vectors() {
    let e = Env::default();
    let (src, img) = (h(&e, 0x11), h(&e, 0x22));
    let digest = |list: &[&str]| claim_input_digest(&e, &src, &img, &opts(&e, list));

    let two = ["--package=vrfy-token", "--optimize"];
    assert_eq!(
        digest(&two),
        BytesN::from_array(
            &e,
            &hex32("ded0494ae3e75310e4989acb8daacfa79636c7e3c211b1af884fa2376ed38381")
        )
    );
    assert_eq!(
        digest(&[]),
        BytesN::from_array(
            &e,
            &hex32("14ee749e3747cf362fffeb845000c5395c9d05055bb8f7e2fb48d41ce7e5064e")
        )
    );
    // Flag order is part of the question.
    assert_eq!(
        digest(&["--optimize", "--package=vrfy-token"]),
        BytesN::from_array(
            &e,
            &hex32("c0823d00b2b99e2f425b9f3fa283f652e315cca163ef3ad10e7fa1f408d881fb")
        )
    );
}

#[test]
fn the_claim_digest_of_the_real_vrfy_token_build() {
    // The staged-tree digest and image digest Sorofy's engine reported when it rebuilt the
    // deployed VRFY token, with the one flag it was built with.
    let e = Env::default();
    let src = BytesN::from_array(
        &e,
        &hex32("47a88a77289c02370d0f04995e6faadcea4f5fb081981ff5ee48762804c0f306"),
    );
    let img = BytesN::from_array(
        &e,
        &hex32("cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588"),
    );
    assert_eq!(
        claim_input_digest(&e, &src, &img, &opts(&e, &["--package=vrfy-token"])),
        BytesN::from_array(
            &e,
            &hex32("32f0a6e512a0259f9d4e2f5a5781cfda2af1782cf4246d9a0fa9a1f314d262fb")
        )
    );
}
