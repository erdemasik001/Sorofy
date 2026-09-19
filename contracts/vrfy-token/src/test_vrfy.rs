//! Tests for how this repository uses the token: as the VRFY stake asset.
//!
//! The upstream suite (`test.rs`) covers the SEP-41 mechanics. These pin down what the
//! registry will depend on: the metadata, that minting is an admin-only act, that a burn is
//! authorized by the holder alone and bounded by their balance, and that seeding several
//! verifiers keeps their balances independent.
#![cfg(test)]
extern crate std;

use crate::{contract::Token, TokenClient};
use soroban_sdk::{testutils::Address as _, Address, Env, String};

const DECIMALS: u32 = 7;
const ONE_VRFY: i128 = 10_000_000; // 10^DECIMALS base units

fn deploy<'a>(e: &Env, admin: &Address) -> TokenClient<'a> {
    let id = e.register(
        Token,
        (
            admin,
            DECIMALS,
            String::from_str(e, "Sorofy Verifier Token"),
            String::from_str(e, "VRFY"),
        ),
    );
    TokenClient::new(e, &id)
}

#[test]
fn metadata_is_vrfy_with_seven_decimals() {
    let e = Env::default();
    let token = deploy(&e, &Address::generate(&e));

    assert_eq!(token.symbol(), String::from_str(&e, "VRFY"));
    assert_eq!(token.name(), String::from_str(&e, "Sorofy Verifier Token"));
    assert_eq!(token.decimals(), 7);
}

#[test]
fn minting_needs_the_admins_authorization() {
    // No `mock_all_auths`: nobody has authorized anything, so the admin check must fail.
    let e = Env::default();
    let token = deploy(&e, &Address::generate(&e));
    let user = Address::generate(&e);

    assert!(token.try_mint(&user, &ONE_VRFY).is_err());
    assert_eq!(
        token.balance(&user),
        0,
        "a refused mint must not credit anyone"
    );
}

#[test]
fn a_burn_is_authorized_by_the_holder_alone() {
    let e = Env::default();
    e.mock_all_auths();
    let admin = Address::generate(&e);
    let holder = Address::generate(&e);
    let token = deploy(&e, &admin);
    token.mint(&holder, &(100 * ONE_VRFY));

    token.burn(&holder, &(40 * ONE_VRFY));

    // `auths()` records who had to authorize the last invocation: the holder, and only them.
    let auths = e.auths();
    assert_eq!(auths.len(), 1);
    assert_eq!(auths[0].0, holder);
    assert_eq!(token.balance(&holder), 60 * ONE_VRFY);
}

#[test]
fn a_burn_cannot_exceed_the_balance() {
    let e = Env::default();
    e.mock_all_auths();
    let holder = Address::generate(&e);
    let token = deploy(&e, &Address::generate(&e));
    token.mint(&holder, &(100 * ONE_VRFY));

    assert!(token.try_burn(&holder, &(101 * ONE_VRFY)).is_err());
    assert_eq!(
        token.balance(&holder),
        100 * ONE_VRFY,
        "a refused burn changes nothing"
    );
}

#[test]
fn seeding_three_verifiers_keeps_balances_independent() {
    let e = Env::default();
    e.mock_all_auths();
    let token = deploy(&e, &Address::generate(&e));

    let verifiers = [
        Address::generate(&e),
        Address::generate(&e),
        Address::generate(&e),
    ];
    for v in &verifiers {
        token.mint(v, &(10_000 * ONE_VRFY));
    }
    for v in &verifiers {
        assert_eq!(token.balance(v), 10_000 * ONE_VRFY);
    }

    token.transfer(&verifiers[0], &verifiers[1], &(250 * ONE_VRFY));
    assert_eq!(token.balance(&verifiers[0]), 9_750 * ONE_VRFY);
    assert_eq!(token.balance(&verifiers[1]), 10_250 * ONE_VRFY);
    assert_eq!(token.balance(&verifiers[2]), 10_000 * ONE_VRFY);
}
