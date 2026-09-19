# Evidence — registry contract, STEP 4

> What was actually run, and what was observed. Written 2026-09-19 on Stellar **testnet**.
> Companion to [hackathon-design.md](hackathon-design.md).
>
> **The claims below are synthetic.** The hashes are placeholders (`aaaa…`, `b1b1…`, `eeee…`),
> not the result of real rebuilds, and every verifier here is one of the four accounts on one
> laptop. This walk-through shows what the *contract* does with attestations. Real
> verifications join up with it in STEP 5–6.

## 1. Unit tests

`cargo test -p sorofy-registry` → **35 passed**. The token in unit tests is a Stellar Asset
Contract (it implements the same SEP-41 interface as VRFY); the deployed registry runs against
the real VRFY token, see §3.

The suite was checked by **mutation**: the contract was deliberately broken one way at a time,
and the tests were required to notice. All eight were caught.

| Deliberate break | Caught by |
|---|---|
| `attest` no longer calls `require_auth` | `attest_needs_the_verifiers_own_authorization` |
| A slash ignores whether the verifier is in the majority | `slash_is_refused_for_a_verifier_in_the_majority` |
| A tie counts as a strict majority (`<=` → `<`) | `slash_is_refused_on_a_two_two_tie` |
| The window closes one ledger late | `attesting_closes_with_the_window` |
| Unbonding may be shorter than the window | `unbonding_shorter_than_the_window_is_refused` |
| Green when a majority (not everyone) agrees | `any_dissent_makes_it_disputed_never_verified` and `slash_burns_the_dissenters_stake_and_deactivates_it` |
| A slash takes from unbonding before the active stake | `slash_reaches_stake_that_is_already_unbonding` |
| The same verifier may attest a claim twice | `a_verifier_cannot_attest_the_same_claim_twice` |

## 2. What was deployed

Both registry instances hold **byte-identical code** (checked with `stellar contract fetch`,
sha256 `7304a856…`), built in the pinned image by Sorofy's engine. Parameters, read back from
the chain: minimum stake 1,000 VRFY, quorum 3, window 60 ledgers, unbonding 120 ledgers, slash
50 %. This walk-through used the *rehearsal* instance
([`CDACBJDL…ZXY4`](https://stellar.expert/explorer/testnet/contract/CDACBJDL7SEXSODGEAQSK5SPVHWZ7QM5PGQP37C7Y3ACYO5Q5QJQZXY4));
the demo instance is untouched. Full list: [testnet.json](../contracts/deployments/testnet.json).

## 3. Walk-through on testnet

Windows are **per claim**: each opens at that claim's first attestation and lasts 60 ledgers.

| Claim | Attestations | While the window ran | After it closed |
|---|---|---|---|
| 1 | a, b: the chain's hash · c: a different one | `Open` | **`Disputed`** (3 attesters, 2 agree) |
| 2 | a, b, c: all the chain's hash | `Open` (unanimous, still not green) | **`Verified`** |
| 3 | a, c: the chain's hash · b: a different one | — | **`Disputed`** |
| 4 | a only | — | **`Insufficient`** (1 of 3) |
| 6 | a: the chain's hash · c: a different one (a genuine 1–1) | `Open` | **`Insufficient`** (2 of 3) |

### Stake, and the slashes

- Each verifier staked 1,000 VRFY: [a](https://stellar.expert/explorer/testnet/tx/3f4c460f27a203f08be69628a46ffa987f321b0a14ae6ec40d53ef289d58059a),
  [b](https://stellar.expert/explorer/testnet/tx/f301789bf99884a6bd75dc0f47abb7176eaba864e53ea20768b37f08f4c7c96c),
  [c](https://stellar.expert/explorer/testnet/tx/c425aca23602d535ee39ac3ec45495ea15d678a4c5ad52e45fb4145bbbac93dd).
  The registry then held 30,000,000,000 base units (3,000 VRFY), and all three read as active.
- **Claim 1: `slash(c)`**, called by `admin` (anyone may call):
  [tx `42c0a240…`](https://stellar.expert/explorer/testnet/tx/42c0a240a4da5ed8a6138e795ff8b64142bc0c2ab785d51f78288ab3bef76035)
  burned **500 VRFY**. c's stake went 1,000 → 500 and c stopped being active. Claim 1 stayed
  `Disputed`; a slash does not turn a dispute green. A second `slash(c)` was refused.
- **Claim 3: `slash(b)`**, where b had 900 VRFY staked and 100 unbonding:
  [tx `dca47cc3…`](https://stellar.expert/explorer/testnet/tx/dca47cc36c51954c88fe8fdc27dad4ed29af57951606cb6e9d50e1e077444f44)
  burned 500 VRFY **from the active part first**: staked 900 → 400, unbonding still 100.
- **Re-activation:** c topped up 500 VRFY
  ([tx `78e6df88…`](https://stellar.expert/explorer/testnet/tx/78e6df8806073d77276f5d0eebcda6d7cf8fcd8c432dc13052d9aebdb6c62de0))
  and read as active again.
- **Unstaking:** b asked to withdraw 100 VRFY
  ([tx `ae4e444d…`](https://stellar.expert/explorer/testnet/tx/ae4e444d0383e47736228f9ed9529a098087ab366756077a6510a3d9d8fda124)),
  a withdraw before the release ledger was refused, and after it
  [tx `3bf0fd6e…`](https://stellar.expert/explorer/testnet/tx/3bf0fd6efa77f934cd2426ffed80813b0399b1bba38a07e6de31a9edaa322afa)
  returned the 100 VRFY to b's wallet (9,000 → 9,100).

### The burn is a real burn

The registry burned its **own** balance of the real VRFY token, and the slash call carried **no
signature from anyone but the caller**: the token's `burn` needs the holder's authorization, and
here the holder is the registry, calling as the invoker. This had been carried as an unproven
assumption since STEP 3; a unit test with auth mocking switched off also passes.

Accounting, summing every account that can hold VRFY (a, b, c, admin, both registries):

```
minted 30,005 VRFY  −  1 VRFY burned by admin (STEP 3)  −  2 slashes × 500 VRFY  =  29,004 VRFY
measured sum of balances                                                       =  29,004 VRFY   ✔
```

Nothing was redistributed: the sum of balances fell by exactly what was burned.

### Refusals

These were rejected while the transaction was being **simulated**, so they leave no transaction
and no trace on the ledger (a unit test checks that a refused attest opens no claim).

| Error | Meaning | Observed for |
|---|---|---|
| `#1` InvalidAmount | | staking 0 |
| `#2` NotActive | | attesting without stake; attesting while under-staked (b after asking to unstake) |
| `#4` AlreadyAttested | | a attesting claim 1 a second time |
| `#7` StillUnbonding | | b withdrawing before the release ledger |
| `#8` ClaimNotDecided | window open, or quorum missing, or no strict majority | slashing c while claim 1's window was open; slashing a on claim 4 (window closed, 1 attester); slashing a *and* c on claim 6 (1–1) |
| `#9` NotAttested | | slashing `admin`, who never attested |
| `#10` NotDissenter | | slashing a on claim 1, where a was in the majority: **the unjustified slash** |
| `#11` AlreadySlashed | | slashing c on claim 1 twice |

## 4. What did not go as scripted

Recorded because the alternative is a tidier story than what happened.

1. **My script assumed one clock; windows are per claim.** Claim 3's window was still open when
   c attested, so that attestation was *accepted* rather than refused as "late", and the first
   "slash b" was refused because the window was open, not because of a 1–1 split. The contract
   was right and my labels were wrong. Claim 6 was added afterwards to get a real 1–1.
2. **"b signs for a" did not demonstrate a refused signer.** The first attempt was refused only
   because a had no stake. The second *succeeded* ([tx `54dd7310…`](https://stellar.expert/explorer/testnet/tx/54dd7310100c8e47d9e274a29b78da85ce0de83c984ba21ef0284e18cd0efe85)):
   the transaction's source was b, but its auth entry was for a **and carried a's signature**,
   which `stellar-cli` had added from the keystore on the same machine. The contract did what it
   should. So a wrong-signer refusal **cannot be shown on-chain from this machine**; it rests on
   the unit test and its mutation check. An attempt to force it with `--build-only` was rejected
   as `TxMalformed` because that transaction was never simulated, which says nothing about auth,
   and is not counted as evidence.
3. That second transaction is also how **a's attestation on claim 4** got there.

## 5. What this does not show

- No real verification: every hash is a placeholder.
- No independent operator: one laptop holds all four keys.
- The unit tests do not run against the VRFY token itself, only against an SEP-41-compatible
  stand-in; the testnet run above is what covers VRFY.
- Nothing about behaviour after the 30-day storage extension lapses.
- The walk-through was driven by throw-away shell scripts that were not kept. The transaction
  hashes are the record. The reproducible artifact will be STEP 6's demo script.
