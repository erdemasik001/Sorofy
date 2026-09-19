# Anchor integration — the stake on-ramp

> **Status: proposal, not implemented.** Written 2026-09-19 for the Stellar Pro Hackathon
> (Scale track). Nothing below exists in the repository. It is kept separate from
> [HACKATHON.md](../HACKATHON.md) so that file continues to record only what has been built.
> Testnet only.
>
> This document answers one judging requirement — *anchor / local payments* — and one design
> gap it exposes: where a verifier's stake comes from. The multi-verifier mechanism itself is
> in [hackathon-design.md](hackathon-design.md); steps 3 and 4 of it are built and deployed.

## 1. The honest starting point

Sorofy is a free, KYC-free verification service. It holds no balances, charges nothing and
settles nothing. It has no payment flow of its own, and inventing one to satisfy a scoring
criterion would be the wrong kind of integration — the handbook asks for a *load-bearing* one.

But the multi-verifier design does introduce capital, in exactly one place:

> A **verifier** is a Stellar account with ≥ `MIN_STAKE` staked.
> — [hackathon-design.md §3](hackathon-design.md)

Stake is capital at risk. Capital has to come from somewhere. For an operator in Turkey, that
somewhere is a bank account denominated in lira. That is the anchor's job, and it is the only
place in this system where an anchor belongs.

The claim is therefore narrow and checkable: **without a fiat on-ramp, a Turkish operator
cannot become a verifier.** Remove the anchor and the path is broken — not degraded, broken.

## 2. The flow

```
lira (bank)  →  anchor (SEP-24)  →  asset on Stellar  →  stake (registry)  →  verifier
```

| Step | Who | What happens |
|---|---|---|
| 1 | Operator | Connects a wallet in the Sorofy explorer (Stellar Wallets Kit) |
| 2 | Explorer | Reads the anchor's `stellar.toml` (SEP-1) to find its assets and endpoints |
| 3 | Operator | Authenticates to the anchor by signing a challenge (SEP-10) |
| 4 | Explorer | Ensures a trustline to the asset's issuer exists, creating it if not |
| 5 | Anchor | Serves its own interactive deposit UI (SEP-24): KYC, bank details, amount |
| 6 | Anchor | On settlement, sends the asset to the operator's account |
| 7 | Operator | Calls `stake(verifier, amount)` on the registry, signing with the same wallet |
| 8 | Registry | Operator is active once stake ≥ `min_stake`; may now `attest` |

Steps 2–6 are the anchor integration. They are classic-Stellar work in the explorer front end —
a `stellar.toml` fetch, a signed challenge, a trustline, and an iframe. No contract is written
for any of it.

Step 7 is the join to what already exists: the registry deployed in STEP 4.

## 3. Two integrations, both from the eligible list

| Category | Partner | Role here |
|---|---|---|
| Wallets | Stellar Wallets Kit | Signs the SEP-10 challenge, the trustline, and the `stake` invocation. One connection serves all three |
| On/off-ramp | An anchor (to be chosen — §7) | Converts lira to a stakeable asset |

Neither is cosmetic. The wallet signs the contract call that creates verifier identity; the
anchor supplies the asset that call moves.

## 4. What this does not change

This is the boundary that keeps the project honest, and it should be stated in the submission:

- **Reading a verdict stays free, anonymous and token-free.** `GET /verify/{id}`,
  `GET /verifications`, the explorer and the on-chain `attestations()` / `consensus()` calls are
  unchanged. No wallet, no payment, no KYC to ask whether a contract is verified. That is the
  public good and it is not for sale.
- **Money is on the supply side only.** It costs something to *become a verifier*, because
  stake is what makes dishonesty expensive. It costs nothing to *use* what verifiers produce.
- **The anchor is one path in, not the only one.** An operator who already holds the asset, or
  acquires it on a DEX, or is sent it by someone else, stakes exactly the same way. The registry
  does not know or care where the balance came from.

That third point matters for the KYC question in §8.

## 5. The stake asset and the registry — open decision

The registry's token address is fixed in the constructor and there is no setter; changing it
means deploying a new registry ([hackathon-design.md §3](hackathon-design.md)). Two instances
are already deployed against VRFY (`CCM2LLD2…`), and the STEP 4 walk-through evidence is
recorded against them.

| Option | Effect | Cost | Risk |
|---|---|---|---|
| **A third instance** (recommended) | The two VRFY instances stand; one more is deployed against the anchored asset | One `deploy` — the wasm is already uploaded | Lowest. No existing evidence is invalidated |
| Redeploy | A single registry, staked in the anchored asset | STEP 4 walk-through and `contracts/deployments/testnet.json` must be redone | Invalidates work in progress |
| Keep VRFY | Anchor shown in the front end only; stake remains VRFY | None | The anchor is no longer load-bearing — it becomes the decoration the handbook is scoring against |

**Not yet decided.** The recommendation is the third instance: it is reversible, cheap, and the
demo can simply use the anchored instance while the VRFY ones remain the development record.

One point in favour of any of these: **the registry has already been tested against a Stellar
Asset Contract.** Its 35 unit tests run with a SAC as the stake token, because SAC implements
the same SEP-41 interface the registry requires. Every asset an anchor issues is reachable at a
SAC address. Pointing the registry at an anchored asset is the configuration the test suite
already exercises, not a new one.

## 6. Integration surface

| SEP | Used for | Where |
|---|---|---|
| SEP-1 | Discover the anchor's assets and endpoints from its `stellar.toml` | Explorer |
| SEP-10 | Prove account ownership to the anchor by signing a challenge | Explorer + wallet |
| SEP-12 | KYC fields, handled inside the anchor's own UI | Anchor (not ours) |
| SEP-24 | Interactive deposit: the anchor hosts the flow, we open it and poll for status | Explorer |
| SEP-41 | The token interface the registry already speaks; a SAC provides it | Contract (unchanged) |

SEP-6 (programmatic deposit) stays out of scope, as
[hackathon-design.md §11](hackathon-design.md) already records.

Note that SEP-10 appears here for the *anchor*, and is unrelated to STEP 8, which is about
SEP-10 for Sorofy's own `POST /verify`. The two are independent.

## 7. Open questions — to be answered before any code

None of these are assumed. Each should be settled at the anchor workshop or with a mentor:

1. **Is there a testnet anchor that handles TRY?** If not, the flow is demonstrated against
   whatever test anchor is available, and the submission says so in those words.
2. **Which partner** — Bridge, BlindPay, or another from the eligible list — and does it cover
   Turkey?
3. **Does the judging panel accept a stake on-ramp as load-bearing** for a project with no
   payment product of its own? This is the question that decides whether §1 holds. Ask it
   directly rather than assuming the answer.
4. Is the testnet stablecoin faucet sufficient to fund three verifier accounts, so the STEP 6
   demo does not depend on a live deposit completing during the presentation?

## 8. What this does not do

- **It does not make TRY work.** Unless question 1 above is answered with a real partner, what
  is demonstrated is the SEP-24 flow against a test anchor. The submission must say
  "the deposit flow works end to end against a test anchor; a TRY partner is an integration
  question", and must not say "lira on-ramp".
- **It does not make the stake economically real.** A faucet-issued testnet stablecoin is not
  collateral. This remains a proof of mechanism, exactly as
  [hackathon-design.md §11](hackathon-design.md) says of VRFY.
- **It introduces KYC where there was none — on one side only.** An operator who onboards
  through the anchor goes through the anchor's KYC. This is a real change in the project's
  character and should not be glossed over. Its bounds: reading verification results remains
  anonymous and free, and §4 means the anchor is one way to obtain stake rather than a gate on
  verifier identity. The registry authenticates a key, not a person.
- **It does not resolve what a slash should do to a backed asset.** The registry burns slashed
  stake, which is correct for a valueless token and deliberate
  ([hackathon-design.md §7](hackathon-design.md): burning avoids making it profitable to slash
  an honest minority). Burning an asset backed by reserves someone else still holds is not
  correct for anything long-lived — the destination should be a dead address or a treasury.
  On testnet the burn is mechanically fine; the fix belongs in the roadmap, not in this build.
- **It adds no revenue.** Nobody pays Sorofy anything in this design. The verification bounty
  model that would change that is a separate idea and is not proposed here.

## 9. Where it would fit in the plan

Steps refer to the table in [HACKATHON.md](../HACKATHON.md).

| Step | Effect of adopting this |
|---|---|
| 5 — API attestation path, follower mode | Unchanged |
| 6 — Three verifiers, slash demo | Verifier accounts funded with the anchored asset instead of VRFY, if §5 chooses a new instance |
| 7 — Explorer rows | Gains the connect-wallet, deposit and stake path. This is where nearly all the work is |
| 10 — Final docs, diagram, pitch | The architecture diagram gains the anchor and wallet edges |

Step 7 is currently listed as partly bonus work. Adopting this proposal makes it load-bearing
for the highest-weighted judging criterion, and it should be re-prioritised accordingly — but
only after steps 5 and 6 work, because a stake on-ramp to a registry that nothing attests to
would demonstrate nothing.
