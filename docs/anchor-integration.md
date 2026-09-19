# Ecosystem integration — verification in the path of money

> **Status: design, now partly built.** Written 2026-09-19 for the Stellar Pro Hackathon
> (Scale track), rewritten the same day, and since then the **front-end gate (§4a) and the
> anchor flow (§5) have been implemented** in [`web/gate/`](../web/gate/README.md) and run
> against testnet. Everything else here — §4b above all — is still a proposal.
> [HACKATHON.md](../HACKATHON.md) remains the record of status; where the two disagree, it is
> right. §10 records what was learned while building, including where the answers were
> unwelcome.
>
> This document answers the two judging requirements Sorofy does not otherwise meet —
> integration with an eligible protocol, and an anchor / local-payments flow — and argues they
> are the same requirement. The execution plan is in [hackathon-plan.md](hackathon-plan.md);
> the multi-verifier mechanism it builds on is in [hackathon-design.md](hackathon-design.md),
> steps 3 and 4 of which are deployed.

## 1. What changed in this document, and why

The first version of this file proposed the anchor as a **stake on-ramp**: a verifier must
stake, an operator in Turkey funds that stake with lira, so the anchor is how a verifier is
born. That argument is sound and it survives below in §6. But it is small. It makes the anchor
load-bearing for *supply* — for the handful of people who run verifiers — and leaves the
protocol-integration requirement unanswered entirely.

The stronger argument runs the other way, and it starts from a sentence that is true whether or
not there is a hackathon:

> **Verification is worthless if nobody acts on it. What acts on it is money.**

Sorofy currently produces a verdict and stops. Nothing consumes it. The registry deployed in
STEP 4 says *this contract was built from this source, several staked verifiers agree, a liar
loses stake* — and then that statement sits on-chain with nothing downstream of it. Putting it
in the path of a deposit is not a repositioning for points. It is the first place the work
carries weight.

## 2. The product

> A user in Istanbul brings lira and wants yield. Their money does not enter a contract that a
> staked, slashable verifier set has not verified.

## 3. The three requirements, satisfied structurally

The handbook asks for an eligible-protocol integration, an anchor flow, and a core feature that
is load-bearing rather than cosmetic. Here each one carries load because removing it breaks the
product:

| Requirement | How it is met | Remove it and |
|---|---|---|
| **Anchor / local payments** | TRY → stablecoin via SEP-24 | the user has no balance to deposit |
| **Eligible protocol** | A deposit into Blend v2 or DeFindex | there is nothing to deposit into |
| **Core feature** | Sorofy's verdict admits or blocks the deposit | this is an ordinary DeFi front end with no reason to exist |

The three are interlocked rather than stacked. That is what *load-bearing* means, and it is
worth saying in exactly those terms in the submission.

## 4. The gate

Two versions. They look identical on stage and differ entirely in strength of claim.

### 4a. Front-end gate — the safe path

Before the deposit is signed, the interface reads consensus for the target contract from the
registry and refuses the deposit unless it is `Verified`, showing why.

Honest about what it is: a client-side check. A determined user can call the pool directly and
nothing stops them. Its value is that it is real, it is buildable with no unknowns, and it
demonstrates the composition.

### 4b. On-chain gate — the strong path

A wrapper contract that forwards a deposit only if the target's wasm hash carries `Verified`
consensus:

```
deposit_if_verified(pool, amount):
    require(registry.any_verified(wasm_hash_of(pool)))
    pool.deposit(amount)
```

This is a real guarantee rather than a UI affordance, and it is the version worth having.

**It rests on an open question.** `wasm_hash_of(pool)` requires a contract to read another
contract's executable hash from within a transaction, and it is not established that
`soroban-sdk` exposes this. Contract instances carry an `executable` field in their ledger
entry, but introspection from inside a contract is limited and may not reach it. The claim
model also needs attention: the registry files attestations under `(wasm_hash, input_digest)`,
so the gate needs a notion of "some verified claim for this hash" rather than one specific
claim.

**Therefore:** time-box a spike (stream 8 in the plan), build 4a in parallel as the guaranteed
path, and upgrade only if the spike succeeds. Do not let the strong version block the demo.

## 5. Where the money comes from

The anchor is the on-ramp for the deposit in §3. Mechanically:

| Step | Who | What happens |
|---|---|---|
| 1 | User | Connects a wallet (Stellar Wallets Kit) |
| 2 | Front end | Reads the anchor's `stellar.toml` (SEP-1) for assets and endpoints |
| 3 | User | Signs a challenge to authenticate to the anchor (SEP-10) |
| 4 | Front end | Ensures a trustline to the issuer, creating it if absent |
| 5 | Anchor | Hosts its own interactive deposit UI (SEP-24): KYC, bank details, amount |
| 6 | Anchor | On settlement, sends the asset to the user's account |
| 7 | User | Deposits through the gate (§4) into the protocol |

Steps 2–6 are classic-Stellar front-end work — a `stellar.toml` fetch, a signed challenge, a
trustline and an iframe. No contract is written for any of it. One wallet connection serves the
SEP-10 challenge, the trustline and the deposit signature, which is why Stellar Wallets Kit is
load-bearing too rather than decorative.

## 6. The stake on-ramp — the second, smaller use

The original argument, unchanged and still true. A verifier is *defined* as an account with
stake ([hackathon-design.md §3](hackathon-design.md)); stake is capital; an operator in Turkey
funds it from a lira account. The same SEP-24 flow serves it, and the same wallet signs
`stake(verifier, amount)`.

If an anchored asset is used as the stake token, note that the registry's token address is
fixed in the constructor and there is no setter — changing it means deploying a new registry.
Two instances are already deployed against VRFY, with the STEP 4 evidence recorded against
them. Options, cheapest first:

| Option | Cost | Risk |
|---|---|---|
| **A third instance** against the anchored asset | one `deploy`; the wasm is already uploaded | lowest — no existing evidence is invalidated |
| Redeploy a single registry | the STEP 4 walk-through and `contracts/deployments/testnet.json` must be redone | invalidates committed work |
| Keep VRFY for staking | none | fine — under §1 the anchor is already load-bearing through the deposit, so staking need not also depend on it |

One fact in favour of any of these: **the registry has already been tested against a Stellar
Asset Contract.** Its 35 unit tests run with a SAC as the stake token, because SAC implements
the same SEP-41 interface the registry requires, and every anchor-issued asset is reachable at
a SAC address. Pointing the registry at an anchored asset is the configuration the test suite
already exercises.

## 7. Integration surface

| SEP | Used for | Where |
|---|---|---|
| SEP-1 | Discover the anchor's assets and endpoints | Front end |
| SEP-10 | Authenticate to the anchor by signing a challenge | Front end + wallet |
| SEP-12 | KYC fields, inside the anchor's own UI | Anchor, not ours |
| SEP-24 | Interactive deposit; we open it and poll for status | Front end |
| SEP-41 | The token interface the registry already speaks; a SAC provides it | Contract, unchanged |

SEP-6 (programmatic deposit) stays out of scope, as
[hackathon-design.md §11](hackathon-design.md) records. The SEP-10 here is the *anchor's* and is
unrelated to STEP 8, which is SEP-10 for Sorofy's own `POST /verify`.

## 8. What stays free

This boundary is what keeps the project honest, and it belongs in the submission verbatim:

- **Reading a verdict stays free, anonymous and token-free.** `GET /verify/{id}`,
  `GET /verifications`, the explorer and the on-chain `attestations()` / `consensus()` calls are
  unchanged. No wallet, no payment, no KYC to ask whether a contract is verified.
- **Money is downstream of verification, never a condition of it.** Nobody pays Sorofy to be
  verified and nobody pays Sorofy to read a result. What costs money is *depositing*, and that
  money goes to the protocol, not to us.
- **The gate is optional.** Anyone may deposit into any pool directly, as they do today. The
  gate is a safer path, not a toll booth.

## 9. What this does not do

- **It does not make TRY work.** Unless a partner covering Turkey is confirmed, what is
  demonstrated is the SEP-24 flow against a test anchor. Say *"the deposit flow works end to
  end against a test anchor; a TRY partner is an integration question"*. Do not say "lira
  on-ramp".
- **It adds no revenue.** Nobody pays Sorofy anything here. A verification-bounty model would
  change that and is deliberately not proposed.
- **The front-end gate is not a guarantee.** §4a is a client-side check; only §4b would bind.
  Whichever ships, say which one it is.
- **It introduces KYC, on one side only.** A user onboarding through the anchor goes through
  the anchor's KYC. Reading verification results stays anonymous, and the anchor is one way to
  obtain a balance rather than a gate on anything Sorofy controls.
- **It does not settle what a slash should do to a backed asset.** The registry burns slashed
  stake, which is correct for a valueless token and deliberate
  ([hackathon-design.md §7](hackathon-design.md): burning avoids making it profitable to slash
  an honest minority). Burning an asset backed by reserves someone else still holds is not
  correct for anything long-lived — the destination should be a dead address or a treasury.
  Mechanically fine on testnet; a roadmap item, not a build item.
- **A verified contract is not a safe contract.** The gate proves the deployed bytes match
  public source that several staked verifiers rebuilt. It says nothing about whether that
  source is any good. This limit must be stated wherever the gate is shown, or the product
  promises something it cannot deliver.

That last point is the one most likely to be tested by a judge. The answer is that verification
is a *precondition* for review, not a substitute for it: you cannot audit code you cannot prove
is running.

## 10. Open questions

Five were listed when this was written. **Three have answers now, one is narrowed, and one is
untouched.** Each answer records how it was checked, because an answer without its method is
only a better-dressed assumption. Everything below was checked against the live network on
2026-09-19. Two of the answers are inconvenient and are written down as found.

### Answered

**1 · Is there a testnet anchor that handles TRY?** *An anchor, yes. TRY, no.*

`testanchor.stellar.org` — SDF's reference server — serves SEP-1 (200), SEP-10 at `/auth`,
SEP-12 at `/sep12` and SEP-24 at `/sep24`. Its `[[CURRENCIES]]` are **SRT, USDC and native**,
and `GET /sep24/info` enables deposit for exactly those three, 1–10 units each. There is no
TRY, and the SEP-10 challenge validates against the anchor's own `SIGNING_KEY`
(`GCHLHDBO…33PR`), so the flow is real end to end.

So §9's first bullet is the wording, and it is not a hedge — it is the fact:
*the deposit flow works end to end against a test anchor; a TRY partner is an integration
question.* Anything that calls this a lira on-ramp is false.

**3 · Blend v2 or DeFindex: testnet address and deposit interface?** *Both exist. Blend v2 is
the one wired.*

| | Blend v2 | DeFindex |
|---|---|---|
| Testnet id | `CCEBVDYM…44HGF` (pool) | `CBMVK2JK…ZDWHN` (USDC vault) |
| Wasm on chain | `a41fc53d…` — matches `lendingPoolV2` in blend-utils | vault wasm `f345228d…` |
| Deposit | `submit(from, spender, to, requests)`, `Request { address, amount, request_type }`, `RequestType::Supply = 0` | `deposit(amounts_desired, amounts_min, from, invest)` |
| Reserves | `get_reserve_list()` → native, wETH, wBTC, USDC | — |

Both interfaces were read off the chain with `stellar contract info interface`, and Blend's
request type from `blend-contracts-v2/pool/src/pool/actions.rs` — not from documentation, which
is where a wrong constant would have silently performed a different action with a user's money.

**5 · Is the faucet enough to fund the demo?** *Yes, and the exposure is smaller than it
looked.*

Friendbot funds a fresh testnet account with 10,000 XLM (HTTP 200, confirmed on Horizon at
`horizon-testnet.stellar.org`). More to the point: **the gate's read path needs no balance at
all** — no wallet, no account, no token — so the *blocked* half of demo beat 6 cannot be broken
by a settlement that fails to arrive. Only the *through* half needs funds, and XLM is what it
needs.

### Narrowed

**2 · Which partner — Bridge, BlindPay, another — and does it cover Turkey?** Still open, and
deliberately not answered from second-hand sources. The Stellar Anchor Directory returns
"No ramp assets available for this location" rather than a list that could be checked. This is a
partnership question for the workshop, not one determinable from a contract, and until someone
answers it §9's first bullet stands unchanged.

### Untouched

**4 · Can a contract read another contract's wasm hash on-chain (§4b)?** No work done. It
remains stream 8's time-boxed spike, and §4a shipped in parallel exactly so the demo never
depended on the answer.

### Three nobody asked, which turned out to matter more

**The anchor's USDC is not the pool's USDC.** The anchor issues USDC from `GBBD47IF…`, whose SAC
is `CBIELTK6…`; the Blend pool's USDC reserve is `CAQCFVLO…`, a different issuer. They are not
the same token and one cannot be supplied in place of the other. **XLM is the only asset that
composes**: the anchor deposits `native` and the pool's first reserve is the native SAC
`CDLZFC3S…`. The flow uses XLM for that reason and no other. A consequence worth stating: with
XLM, §5's step 4 is vacuous, because native needs no trustline — the step is implemented for
the issued assets, where it is genuinely required, and the interface says which case it is in
rather than skipping quietly.

**Nothing on chain names a build.** `stellar contract info meta` on the Blend v2 pool returns
`source_repo: github:blend-capital/blend-contracts-v2` and stops; the VRFY token carries only
compiler versions. Neither publishes SEP-58 `bldimg` / `bldopt` / `source_sha256`. Two things
follow:

- This *is* the measurement [hackathon-plan.md §5](hackathon-plan.md) predicts. The pool the
  gate is pointed at cannot be verified today, by anyone. The gate blocking it is the finding,
  not a disappointment, and it is worth saying in those words on stage.
- The gate cannot derive a claim from a contract alone. The registry files attestations under
  `(wasm_hash, input_digest)` and offers no "anything for this hash?" query — the same gap §4b
  names, and it bites §4a too. It is solved by discovering claims from the registry's own
  `attested` events (topic 2 is the wasm hash, so the RPC filters server-side), with a
  configured build descriptor as the fallback. That scan is bounded by event retention:
  ~121 000 ledgers, roughly a week, and the query **pages** — a 16 000-ledger window returned an
  empty first page with a cursor where an 8 000-ledger one returned its events directly.

**Sorofy's own API cannot be read from a browser.** `sorofy.site` sends no
`Access-Control-Allow-Origin`, so a page on another origin cannot call `GET /verify/{hash}`.
The gate does not need it and is stronger without it: the wasm hash comes from the ledger and
the verdict from the registry, so **a lying or compromised API cannot produce a pass** — the
worst a wrong claim can do is name something nobody attested, which reads as `NoClaim` and
blocks. If the API is ever wanted in this path, adding CORS is a change to `crates/api/`, not
to the gate.

## 11. Where it fits

Streams as numbered in [hackathon-plan.md §4](hackathon-plan.md): this document is streams 5
(gate), 6 (anchor) and 8 (the on-chain spike). It depends on the registry, which is deployed,
and it is far more compelling once something has actually been attested — so streams 1 and 2
should be working before the gate is demonstrated. A gate reading an empty registry
demonstrates nothing.
