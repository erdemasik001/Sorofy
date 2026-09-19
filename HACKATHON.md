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
| 2 | Design document, reward-economics projection | ✅ done, approved |
| 3 | VRFY token (SEP-41): deployed to testnet, minted to the three verifier accounts, transfer/balance/burn checked with the CLI | ✅ done |
| 4 | Registry contract: stake, attest, conservative consensus, slash. 35 unit tests, deployed to testnet, walked through end to end with the CLI | ✅ done ([evidence](docs/hackathon-evidence.md)) |
| 5 | API attestation path (flagged, asynchronous) and follower mode | ⏳ not started |
| 6 | Three verifiers (two following the first) and a slash demo script | ⏳ not started |
| 7 | Explorer rows for stakes and attestations; token-free read path. *Bonus, only after the core works:* a clearly labelled "projected rewards" panel | ⏳ not started |
| 8 | SEP-1 / SEP-10 | ⏳ not started |
| 9 | Production deploy | ⏳ only on explicit go-ahead, with a written rollback plan first |
| 10 | Final docs, architecture diagram, pitch on the official template | ⏳ not started |

## Deployed artifacts (testnet)

| Artifact | Contract ID | Note |
|---|---|---|
| VRFY token (SEP-41) | [`CCM2LLD2…HKEDW`](https://stellar.expert/explorer/testnet/contract/CCM2LLD2TAFOMQQUQK62DHCEWNO7UEYOWDXL52GF3KTDK4NMBT6HKEDW) | Soroban standard token example, pinned upstream commit, see [NOTICE](contracts/vrfy-token/NOTICE.md). wasm `3e23ccf5…`, built in the pinned image and **reproduced by Sorofy's own engine** (`VERIFIED` against the on-chain hash) |
| Registry (demo/API instance) | [`CA4VYPAG…PCFE`](https://stellar.expert/explorer/testnet/contract/CA4VYPAGEYYOV7CJIBTCJHOGW2KAFQ4AHYEZIY2NA3NXFJGGG4XSPCFE) | wasm `7304a856…`, built in the pinned image. Parameters fixed at deploy: 1,000 VRFY minimum stake, quorum 3, 60-ledger attest window, 120-ledger unbonding, 50 % slash. Kept free of test traffic |
| Registry (rehearsal instance) | [`CDACBJDL…ZXY4`](https://stellar.expert/explorer/testnet/contract/CDACBJDL7SEXSODGEAQSK5SPVHWZ7QM5PGQP37C7Y3ACYO5Q5QJQZXY4) | identical bytes; used for the STEP 4 walk-through with synthetic claims |

All contract IDs, hashes and transaction hashes live in one file:
[`contracts/deployments/testnet.json`](contracts/deployments/testnet.json) (public data only).

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

Reference material consulted so far (spec and docs pages, *not* skill files): SEP-41
([`ecosystem/sep-0041.md`](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0041.md),
v0.5.2) and the Soroban token example
([`stellar/soroban-examples/token`](https://github.com/stellar/soroban-examples/tree/main/token)).

## Findings along the way

- **`stellar-cli` signs a Soroban auth entry with any key in the local keystore, not just the
  transaction's source account.** A transaction sourced by `verifier_b` that attested *as*
  `verifier_a` therefore succeeded here, because `verifier_a`'s key is on the same machine; the
  decoded transaction shows an auth entry for `verifier_a` carrying `verifier_a`'s signature. The
  contract behaved correctly (it needed that signature and got it). It does mean a
  "wrong signer is refused on-chain" demonstration is **not possible on this machine**. That
  property is covered by a unit test that runs with auth mocking switched off, and the test was
  checked by mutation (removing `require_auth` from `attest` fails it).
- **Attestation windows are per claim**, opening at each claim's first attestation, not one
  global clock. Two of my own walk-through steps assumed otherwise and were wrong; the contract
  was right. See [the evidence](docs/hackathon-evidence.md).
- **The engine cannot verify a contract that lives in a subdirectory.** It reads the built
  wasm from a fixed `<source root>/target/wasm32v1-none/release`, and runs `cargo fetch
  --locked` at the source root without the `--manifest-path` a SEP-58 `bldopt` may carry.
  Read from the code; not yet demonstrated with a failing run. Not fixed here: it is the live
  service's engine and out of this delta's scope. The contracts are built as a tree whose
  root *is* the workspace, which the engine handles.
- **A build's bytes depend on the CLI that built it.** The same source built with the local
  stellar-cli 28.0.0 (`b8836ea6…`, optimised) and with the pinned image's 23.2.1
  (`3e23ccf5…`) differ. Only the second is reproducible by Sorofy, so that is what is deployed.

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
- **2026-09-19** — Step 3 done. VRFY token deployed and seeded with 10,000 VRFY per verifier.
  Root workspace test baseline recorded before touching the API: 60 passed, 0 failed, 9 ignored
  (the ignored ones need Docker, the pinned image and the network).
  Cold build of the token in the pinned image on this Mac (amd64 under Rosetta): 62–66 s.
- **2026-09-19** — Step 4 done. Registry contract (35 tests, 7 of 7 deliberate mutations
  caught), built in the pinned image (62 s), two instances on testnet. The deployed VRFY token
  still reproduces from the newer source tree. On testnet, `slash` burned real VRFY with no
  external signature (the registry burns its own balance), and the burned amount matched the
  arithmetic exactly.
