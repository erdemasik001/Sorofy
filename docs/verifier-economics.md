# Verifier economics — a projection, not a feature

> **PROJECTION — NOT IMPLEMENTED.** The registry contract pays no reward, holds no reward pool
> and moves no token for this purpose. Nothing here is funded, and no funding is assumed
> to exist. Every rate and number below is an **assumption** chosen to make the model readable,
> not a measurement, except where a figure is explicitly marked as measured. This document
> exists so the next step after the hackathon is designed before it is asked for.
>
> Written 2026-09-19. Companion to [hackathon-design.md](hackathon-design.md).

## 1. The gap this addresses

Staking in the current design has a cost (locked capital, slash risk) and no payoff. A rational
operator has no reason to join. Fixing that needs a source of value, which is a product
decision (who pays for verification?) more than a contract one. This is the reasoned answer,
kept out of the contract on purpose.

## 2. What it costs a verifier to take part

Per attestation, the reward `r` must cover three things:

```
r*  =  c  +  p · (slash_bps / 10000) · S  +  k · S / n
```

| Symbol | Meaning | Where the number comes from |
|---|---|---|
| `c` | Cost of running one verification | Build time is **measured**: ≈ 83–86 s per build on the 4 vCPU / 8 GB reference host ([README](../README.md), live `/metrics`). The money cost depends on the operator's host and is not stated here |
| `p` | Probability an attestation lands on the losing side and is slashed | **Assumed.** Deterministic builds make honest verifiers agree, so `p` should be near zero unless infrastructure misbehaves. Not yet measured across hosts (see the design's §11, item 9) |
| `S`, `slash_bps` | Stake, and the share burned | Contract parameters |
| `k` | Opportunity cost of locked capital per period | Assumed |
| `n` | Attestations per period | Assumed |

Illustration, with `S = 1000`, `slash_bps = 5000`, `k = 0.5 %` per period, `n = 1000` per
period, and `c` taken as one unit (all assumed):

| `p` | slash-risk term | break-even reward `r*` |
|---|---|---|
| 0.0001 | 0.05 c | ≈ 1.06 c |
| 0.001 | 0.50 c | ≈ 1.51 c |
| 0.01 | 5.00 c | ≈ 6.01 c |

The reading that matters is not the exact figures: **the slash-risk term dominates** unless
`p` is tiny. Operators will only accept a large stake if honest verification almost never ends
on the losing side, which makes cross-host determinism the thing to measure first.

## 3. Where the reward could come from

| Source | For | Against |
|---|---|---|
| Submitter fees | Self-funding | Conflicts with the funded RFP's "free, public, KYC-free" service |
| Grant-funded pool (SCF / InstAward) | Keeps reads and submissions free | Only exists if such funding is awarded; it is finite |
| Integrators sponsoring queries (explorers, wallets) | Payers are the parties who benefit from the badge | Needs integrations that do not exist yet |

None is chosen. The honest statement is that a sustainable answer probably combines a grant
pool early with integrator sponsorship later, and that this is the open question of the
project's next milestone.

## 4. How it could be paid, and what that does to behaviour

| Rule | Effect | Risk |
|---|---|---|
| Fixed reward per attestation | Simple | Farming: spam claims and Sybil identities to collect it |
| Reward only for attestations on claims that closed `Verified`/`Mismatch` (unanimous) | Pays for real, agreed work | Rewards agreement, so it encourages copying others |
| Reward split among the majority on a disputed claim | Pays for being right | The majority profits from disputes: collusion incentive |

Two constraints follow directly:

- **Reward from an external pool, never from slashed stake.** Paying the majority out of the
  slashed stake would make slashing an honest minority profitable. Slashed stake is burned
  (design §7).
- **Copying and farming need their own mitigations** — commit-reveal for the first, claim
  admission through gated submission for the second. Neither is designed here in detail.

## 5. What stake does and does not protect

Worth stating precisely, because it is easy to overclaim:

- Stake **deters a lone or minority dishonest verifier**: an outvoted attester loses part of
  it.
- Stake does **not** stop a colluding majority, which is not slashed and can burn an honest
  dissenter.
- What stops a false `Verified` is that *one honest staked attester inside the window* makes
  the claim `Disputed` (design §6, §10). That does not scale with the size of the stake.

So the stake size is a lever against Sybil identities and against minority dishonesty, not a
"security budget" that prices out an attacker. For a mainnet version the sizing question is:
how large must `quorum × min_stake` be that creating a colluding quorum of identities is not
worth the trouble, given the value a false badge could protect. That figure needs a real token
with a real price, which the testnet does not have.

## 6. Bonus: a clearly labelled panel in the explorer (only after the core works)

If time allows once attestation, consensus and slash run end to end, the explorer may show a
**"Projected rewards"** panel: each verifier's real attestation count read from the chain,
multiplied by an illustrative rate stated on the panel. It will carry a permanent
*"Projection — nothing is paid"* label, show no balance, no "claim" control and no transfer,
and appears in HACKATHON.md under *not implemented*.

## 7. What it would take to make it real

1. A decision on who pays (§3).
2. Cross-host determinism measured, to set `p`.
3. A reward-pool contract and its tests, plus an audit.
4. Commit-reveal or an equivalent, so copying is not the best strategy.
5. Real tokens with a price, i.e. a mainnet deployment — out of scope for this work.
