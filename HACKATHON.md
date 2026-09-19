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

Every row carries what backs it: a command that was run, or a transaction hash on testnet.
Where a step was planned as two pieces and only one landed, it says **partly** and names the
piece that is missing. Re-run the test commands with
`export PATH="$HOME/.cargo/bin:$HOME/.local/node/bin:$PATH"` first.

| # | Work | Status | Evidence |
|---|---|---|---|
| 0 | Repository discovery, contradictions with the plan reported | ✅ done | [docs/hackathon-design.md](docs/hackathon-design.md) and this file |
| 1 | Toolchain and four funded testnet identities; sample contract built, deployed, invoked | ✅ done | `rustc 1.91.1`, `cargo 1.91.1`, `stellar 28.0.0` (`300aaf69`); the four accounts in [`testnet.json`](contracts/deployments/testnet.json) |
| 2 | Design document, reward-economics projection | ✅ done, approved | [hackathon-design.md](docs/hackathon-design.md), [verifier-economics.md](docs/verifier-economics.md) |
| 3 | VRFY token (SEP-41): deployed, minted to the three verifiers, transfer/balance/burn checked | ✅ done | deploy `8df956c0…`; mints `2ec63a16…`, `825c83e3…`, `0e595313…`; transfer `7e934a4a…`; burn `ca3469d1…` |
| 4 | Registry: stake, attest, conservative consensus, slash. Deployed, walked through end to end | ✅ done | `cd contracts && cargo test` → **46 passed** (registry 35 + token 11); deploy `805bff41…`; rehearsal instance `b209640a…` ([evidence](docs/hackathon-evidence.md)) |
| 5 | API attestation path (flagged, asynchronous) **and follower mode** | ⚠️ **partly** | Attestation path is built: [`crates/api/src/attest.rs`](crates/api/src/attest.rs), off unless `SOROFY_ATTEST=1`, queueing an `attestations` outbox that a detached worker drains. `cargo test --workspace` → **102 passed, 10 ignored**. **Follower mode is not implemented** — it is specified in [design §8](docs/hackathon-design.md) and has zero hits in `crates/` and `contracts/` |
| 6 | Three verifiers (two following the first) and a slash demo script | ⚠️ **partly** | Three funded verifier identities staked and attested; the rehearsal registry returns `Verified`, which its `quorum = 3` parameter cannot return on fewer. `slash` burned real VRFY on testnet and the burned amount matched the arithmetic. **No dedicated slash demo script**, and "two following the first" depends on follower mode, which does not exist |
| 7 | Explorer rows for stakes and attestations; token-free read path | ⚠️ **partly** | Token-free read path is built and was re-checked live on 2026-09-20: the gate resolves a verdict with no wallet, no account and no token ([web/gate](web/gate/README.md)). **Explorer rows were not built** — zero `stake`/`attest` hits in `crates/api/static/` |
| 8 | SEP-1 / SEP-10 | ⚠️ **partly, and rescoped** | Consumed as a **client**: the gate performs SEP-1 discovery, SEP-10 challenge signing and SEP-24 deposit against `testanchor.stellar.org` ([`web/gate/lib/anchor.js`](web/gate/lib/anchor.js)). Sorofy's own API still authenticates job submission with the shared bearer token, so SEP-10 **as a replacement for that** is not done. The "Before → after" row above stays accurate |
| 9 | Production deploy | ⛔ not done, by decision | Testnet only. No mainnet deploy was attempted and none is claimed |
| 10 | Final docs, architecture diagram, pitch on the official template | 🔄 in progress | [architecture](docs/hackathon-architecture.md) and [roadmap](docs/hackathon-roadmap.md) delivered; the deck is pending the official template |

### Checked live, not remembered

Re-run on **2026-09-20** against the live network, read-only, from the gate itself:

| Target | Read | Meaning |
|---|---|---|
| VRFY token `CCM2LLD2…HKEDW` | **`Verified`** — 1 claim, found via chain events, deployed wasm `3e23ccf5…`, claim `32f0a6e5…`, read from rehearsal registry `CDACBJDL…ZXY4` | The bytes on the ledger match source that staked verifiers rebuilt |
| Blend v2 pool `CCEBVDYM…44HGF` | **`NoClaim`** — blocked | Nobody has attested it. The gate says so in those words: not an accusation, the absence of evidence |

The deployed wasm hash the gate read matches `contracts.vrfy_token.wasm_sha256` in
[`testnet.json`](contracts/deployments/testnet.json) exactly.

## Deployed artifacts (testnet)

| Artifact | Contract ID | Note |
|---|---|---|
| VRFY token (SEP-41) | [`CCM2LLD2…HKEDW`](https://stellar.expert/explorer/testnet/contract/CCM2LLD2TAFOMQQUQK62DHCEWNO7UEYOWDXL52GF3KTDK4NMBT6HKEDW) | Soroban standard token example, pinned upstream commit, see [NOTICE](contracts/vrfy-token/NOTICE.md). wasm `3e23ccf5…`, built in the pinned image and **reproduced by Sorofy's own engine** (`VERIFIED` against the on-chain hash) |
| Registry (demo/API instance) | [`CA4VYPAG…PCFE`](https://stellar.expert/explorer/testnet/contract/CA4VYPAGEYYOV7CJIBTCJHOGW2KAFQ4AHYEZIY2NA3NXFJGGG4XSPCFE) | wasm `7304a856…`, built in the pinned image. Parameters fixed at deploy: 1,000 VRFY minimum stake, quorum 3, 60-ledger attest window, 120-ledger unbonding, 50 % slash. Kept free of test traffic |
| Registry (rehearsal instance) | [`CDACBJDL…ZXY4`](https://stellar.expert/explorer/testnet/contract/CDACBJDL7SEXSODGEAQSK5SPVHWZ7QM5PGQP37C7Y3ACYO5Q5QJQZXY4) | identical bytes. Carries the STEP 4 walk-through's synthetic claims **and one real claim**: the VRFY build `3e23ccf5…`, attested to quorum, which reads `Verified` on the live network (re-checked 2026-09-20). This is the instance the gate points at |

All contract IDs, hashes and transaction hashes live in one file:
[`contracts/deployments/testnet.json`](contracts/deployments/testnet.json) (public data only).

## Handbook requirements — where we stand

The handbook judges both tracks on three requirements. Recorded as they are, not as we would
like them to be.

| Requirement | Status |
|---|---|
| Integration with an existing Stellar protocol (load-bearing) | ✅ **Blend v2.** The gate reads the pool's reserves through `get_reserve_list()` and builds its deposit as Blend's own `submit(from, spender, to, requests)` with `RequestType::Supply` ([web/gate/lib/protocol.js](web/gate/lib/protocol.js)); the pool is `CCEBVDYM…44HGF`. Wallet connection is Stellar Wallets Kit 2.6.0, pinned. Load-bearing in the strict sense: without the pool call there is nothing for the gate to gate |
| Anchor / local payments | ⚠️ **Decision revised during the hackathon — built, with a named gap.** The earlier position (no fiat rail, out of scope) was reversed: the SEP-24 on-ramp is built and runs end to end against `testanchor.stellar.org` — SEP-1 discovery, SEP-10 challenge, trustline, interactive deposit — and the balance it brings is exactly what the gate then guards ([docs/anchor-integration.md](docs/anchor-integration.md), [web/gate](web/gate/README.md)). **What is not there: TRY.** The test anchor handles SRT, USDC and XLM only. No Turkish lira partner is integrated and none is claimed; that is an integration question, not a demonstrated capability |
| Core feature | ✅ by design: staking and attestation are the multi-verifier requirement of the funded RFP |
| Scale-track extras: architecture diagram, post-hackathon roadmap toward SCF/InstAward | ✅ [architecture](docs/hackathon-architecture.md) — drawn from running code, not from the design — and [roadmap](docs/hackathon-roadmap.md). Deck pending the official template |

## Not implemented, by decision

Stated up front so nothing above is read as more than it is:

- **No verifier rewards.** Staking earns nothing in this build. The reward model is a
  projection ([docs/verifier-economics.md](docs/verifier-economics.md)); no funding is assumed.
- **No economic security.** VRFY is a worthless testnet token. This is a proof of mechanism.
- **No independent operators** unless one actually joins during the event; if so it is recorded
  here, otherwise nothing is claimed.
- **No fiat rail in the sense that matters, no SEP-6, no credit system, no mainnet.** The
  SEP-24 on-ramp against the test anchor is built and works, so this line is narrower than
  it was: what is absent is a *usable* fiat path. The test anchor handles SRT, USDC and XLM;
  no Turkish lira partner exists and none is claimed.
- **No follower mode.** Specified in [design §8](docs/hackathon-design.md), not implemented.
  Independent verifiers have no mechanism for getting work, which is the honest limit on
  every "multi-verifier" sentence in this file.

## Stellar skill files used

**Nothing built here used a skill file.** The engine, the registry contract, the API
attestation path and the gate were all written before the official plugin was located, so no
retroactive attribution is made — claiming one would be exactly the kind of unverifiable
statement this file exists to avoid.

Located during the delivery pass, and recorded because the handbook asks for it:

| Repository | What it is | Used? |
|---|---|---|
| [`stellar/stellar-dev-skill`](https://github.com/stellar/stellar-dev-skill) | The official plugin — author *Stellar Development Foundation*, Apache 2.0, `stellar-dev` v1.2.0 at commit `202be80` (2026-09-14). Ships 8 skills: `smart-contracts`, `standards`, `dapp`, `assets`, `agentic-payments`, `cross-chain`, `data`, `zk-proofs` | **Pending.** Being installed to re-check the SEP claims in this file and the deck. This row will name what it actually changed, or say it changed nothing |
| [`kaankacar/stellar-build`](https://github.com/kaankacar/stellar-build) | A community bundle, not official. Its own NOTICES points at the repository above as the official upstream | **No.** Its installer writes `~/.claude/settings.json`, installs prompt-capturing hooks and wires an external MCP server — declined on delivery day as an unnecessary change to a working environment |

The `standards` skill covers SEP-0001, SEP-10, SEP-0024 and SEP-41 — four of the five standards
this delta touches. It does **not** cover SEP-58, so the SEP-58 finding below rests on our own
measurement with `stellar contract info meta`, not on any skill file.

Reference material consulted during development (spec and docs pages, *not* skill files): SEP-41
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
- **2026-09-20** — Delivery pass. Status table rebuilt so every row carries a run command or a
  transaction hash, and three rows corrected from "not started" to **partly**: the API
  attestation path exists but follower mode does not; the token-free read path exists but the
  explorer rows do not; SEP-1/SEP-10 are consumed as a client but do not replace bearer-token
  auth. Two handbook rows were stale in the other direction and now record what was built —
  Blend v2 as the load-bearing integration, and the reversed anchor decision with the TRY gap
  named. Architecture diagram and SCF roadmap added. Gate re-checked live against testnet:
  VRFY reads `Verified`, the Blend v2 pool reads `NoClaim`.
