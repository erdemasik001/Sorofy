# Sorofy at the Stellar Pro Hackathon 2026 — Scale track

> Rise In × Stellar, Istanbul, 2026-09-19 → 2026-09-20. **This file is a dated delta**: it
> records only what was added during the hackathon, on top of the service that already existed
> (see [README.md](README.md)). The pre-hackathon state is `master` @ `247fa84`.
>
> **Working draft.** Every status below is what is true *now*; nothing is marked done until it
> has been run and checked. Testnet only.

## The one-sentence claim

*You do not have to trust a verifier: if it lies, it loses its stake, and you can verify that
on-chain.*

## Before → after

| | Before (`master` @ `247fa84`) | After (this hackathon, target) |
|---|---|---|
| Verifiers | A single operator's instance; no verifier identity | Accounts that stake VRFY become verifiers; several instances run |
| Result storage | The operator's SQLite file | API DB **and** a Soroban registry contract |
| A dishonest verifier | Indistinguishable from an honest one | Cannot show a green result alone (any dissent ⇒ `Disputed`); loses stake if outvoted |
| Result checkable without trusting the operator | No | Yes — read the attestations on-chain, rebuild yourself |
| How other verifiers get work | n/a | *Follower mode*: re-verify what the public feed lists, expected hash resolved from their own RPC |
| Job submission | One shared bearer token | Unchanged until the SEP-10 step |

The design in one line per layer: **truth** = deterministic rebuild anyone can repeat;
**visibility** = conservative consensus; **deterrence** = majority-based slash, burned.

Full design, the slash rule and its alternatives, and the honest limits:
**[docs/hackathon-design.md](docs/hackathon-design.md)**. Verifier reward economics, as a
*projection only*: **[docs/verifier-economics.md](docs/verifier-economics.md)**.

## Status

| # | Work | Status |
|---|---|---|
| 0 | Repository discovery, contradictions with the plan reported | ✅ done |
| 1 | Toolchain (Rust 1.91.1, `wasm32v1-none`, stellar-cli 28.0.0, Docker via OrbStack) and four funded testnet identities; sample contract built, deployed, invoked | ✅ done |
| 2 | Design document, reward-economics projection | ✍️ revised, awaiting approval |
| 3 | VRFY token (SEP-41) | ⏳ not started |
| 4 | Registry contract: stake, attest, conservative consensus, slash | ⏳ not started |
| 5 | API attestation path (flagged, asynchronous) and follower mode | ⏳ not started |
| 6 | Three verifiers (two following the first) and a slash demo script | ⏳ not started |
| 7 | Explorer rows for stakes and attestations; token-free read path. *Bonus, only after the core works:* a clearly labelled "projected rewards" panel | ⏳ not started |
| 8 | SEP-1 / SEP-10 | ⏳ not started |
| 9 | Production deploy | ⏳ only on explicit go-ahead, with a written rollback plan first |
| 10 | Final docs, architecture diagram, pitch on the official template | ⏳ not started |

## Deployed artifacts (testnet)

| Artifact | Contract ID | Note |
|---|---|---|
| VRFY token | — | not deployed yet |
| Registry | — | not deployed yet |

Contract IDs will live in one configuration file (STEP 3) and be copied here.

## Handbook requirements — where we stand

The handbook judges both tracks on three requirements. Recorded as they are, not as we would
like them to be.

| Requirement | Status |
|---|---|
| Integration with an existing Stellar protocol (load-bearing) | ❓ **not decided.** Stellar Wallets Kit is the only listed partner that fits (wallet connection for staking / sign-in); no choice made |
| Anchor / local payments (real TRY in, usable balance out) | ❌ **not planned.** Sorofy is a free, KYC-free verification service and has no honest use for a fiat rail without a payments or credit system, which is out of scope by decision. The highest-weighted criterion in Ecosystem Fit; we accept the cost and will say so in the submission |
| Core feature | ✅ by design: staking and attestation are the multi-verifier requirement of the funded RFP |
| Scale-track extras: architecture diagram, post-hackathon roadmap toward SCF/InstAward | ⏳ STEP 10 |

## Not implemented, by decision

Stated up front so nothing above is read as more than it is:

- **No verifier rewards.** Staking earns nothing in this build. The reward model is a
  projection ([docs/verifier-economics.md](docs/verifier-economics.md)); no funding is assumed.
- **No economic security.** VRFY is a worthless testnet token. This is a proof of mechanism.
- **No independent operators** unless one actually joins during the event; if so it is recorded
  here, otherwise nothing is claimed.
- **No fiat rail, SEP-6, credit system or mainnet.**

## Stellar skill files used

None cited yet. Each will be listed here, by path, when it is actually consulted during
development, as the handbook requires.

## Relationship to the existing roadmap

[docs/testnet-roadmap.md](docs/testnet-roadmap.md) describes Phase 3 (multi-verifier) as
off-chain signed attestations and lists an on-chain registry contract as out of scope for the
earlier funded engagement. This delta takes the on-chain route instead. The roadmap is left
untouched; where the two differ, this file and the design document describe what the hackathon
built.

## Delta log

- **2026-09-19** — Steps 0–1 done. Design drafted ([docs/hackathon-design.md](docs/hackathon-design.md)),
  then revised: three-layer model, conservative consensus, follower mode, slash burned rather
  than redistributed, and a reward-economics projection.
