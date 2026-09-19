# Hackathon execution plan — playing for first

> **Status: plan, dated 2026-09-19 17:00.** Written for the Stellar Pro Hackathon (Scale
> track), submission 2026-09-20 12:00. This is the team's working plan: what we are building,
> who can take what in parallel, and what has to be *learned* rather than assumed.
>
> [HACKATHON.md](../HACKATHON.md) is the record of what has actually been built and stays the
> source of truth for status. This file is the plan; where they disagree, HACKATHON.md is right.

## 1. Where we already are

Not a starting point — a running service with four weeks of funded work behind it.

| | |
|---|---|
| Live service | [sorofy.site](https://sorofy.site) — deterministic rebuild engine, hardened sandbox, explorer, CI, 24 real jobs |
| On testnet | VRFY token [`CCM2LLD2…`](https://stellar.expert/explorer/testnet/contract/CCM2LLD2TAFOMQQUQK62DHCEWNO7UEYOWDXL52GF3KTDK4NMBT6HKEDW), registry [`CA4VYPAG…`](https://stellar.expert/explorer/testnet/contract/CA4VYPAGEYYOV7CJIBTCJHOGW2KAFQ4AHYEZIY2NA3NXFJGGG4XSPCFE) and a rehearsal instance |
| Registry quality | 35 unit tests, 7 of 7 deliberate mutations caught, slash burned real VRFY on testnet |
| Documented | [hackathon-design.md](hackathon-design.md), [hackathon-evidence.md](hackathon-evidence.md), [verifier-economics.md](verifier-economics.md) |

## 2. The thesis

Until now Sorofy has been positioned as a developer tool. That framing leaves the
highest-weighted judging criterion — Ecosystem Fit — at zero, and makes any anchor or DeFi
integration look bolted on.

The framing that wins instead:

> **Verification is worthless if nobody acts on it. What acts on it is money.**
> Sorofy becomes the safety layer for capital on Stellar: funds do not enter code that a
> staked, slashable verifier set has not verified.

This is not a repositioning for points. It is what the registry already says — *this contract
was built from this source, three verifiers agree, a liar loses stake* — made consequential.

## 3. The demo arc

Fix the demo first; the work streams exist to serve it. Target four minutes.

| # | Beat | Time | Criterion it hits |
|---|---|---|---|
| 1 | A deployed contract is opaque bytes. You cannot know the source matches | 20 s | Problem |
| 2 | We ran Sorofy across the Stellar DeFi stack. Here is what is verifiable today | 25 s | Ecosystem awareness |
| 3 | Three verifiers, three machines, each staked, each attesting on-chain | 45 s | Technical depth |
| 4 | One lies. Its stake burns. On-chain, permissionless, anyone can trigger it | 35 s | Economic mechanism |
| 5 | I am in Istanbul. I have lira. I want yield | 25 s | Anchor |
| 6 | I try to deposit into an unverified pool — blocked. Then the verified one — through | 45 s | Ecosystem fit + core feature |
| 7 | Verification nobody acts on is worthless. This is the first place it carries weight | 15 s | Close |

Beats 4 and 6 are what separate this from every other submission. Rehearse those two.

## 4. Work streams

Streams 1–8 are genuinely parallel. Stream 9 needs its own owner and starts immediately.

| # | Stream | People | Depends on | Done means |
|---|---|---|---|---|
| 1 | API attestation path (STEP 5) | 1–2 | — | With the flag on, a finished verification produces an on-chain attestation. **With it off, behaviour is byte-identical to today** |
| 2 | Three verifiers + cross-host determinism | 1 + laptops | 1 | Three independent attestations reach `Verified`; the determinism result is written down |
| 3 | Follower mode | 1 | 1 | A verifier attests without anyone sending it a job |
| 4 | Slash demo | 1 | 2 | A repeatable script, under 60 s |
| 5 | Verification gate (Wallets Kit + protocol) | 1–2 | — | Two paths on stage: one blocked, one through |
| 6 | Anchor SEP-24 | 1 | — | A deposit lands in the account stream 5 uses |
| 7 | Ecosystem verification sweep | 1 | — | A table of real results, whatever they turn out to be |
| 8 | On-chain gate spike (time-boxed) | 1 | — | Either a prototype or a written "not possible" |
| 9 | **Submission package** | 1 owner | all | Submitted at 11:00, not 11:59 |

## 5. The five things that separate first from third

1. **Cross-host determinism, measured.** [hackathon-design.md §11](hackathon-design.md) says
   plainly: *"Determinism across hosts is assumed, not proven here."* Run the same rebuild on
   three different machines. If the hashes match, the claim is proven rather than assumed. If
   they do not, that is an honest and publishable finding. Either outcome is worth having, and
   a team can do it where one person cannot.
2. **The ecosystem sweep.** Run Blend, Soroswap, DeFindex and Aquarius through our own engine.
   Many will likely come back `error` — no SEP-58 metadata, no reproducible build. That is not
   a disappointment, it is the measurement of the problem we exist to solve, and it is one
   slide: *this share of Stellar DeFi cannot be verified today*.
3. **On-chain slash.** No other team will have economic mechanics at all.
4. **Self-verification.** The VRFY token was reproduced by Sorofy's own engine and reported
   `VERIFIED` against its on-chain hash. One sentence, and it lands.
5. **The gate.** The only work that moves Ecosystem Fit off zero.

## 6. Five forks to resolve in the first hour

Each of these changes the plan. Learn them; do not assume them.

| Fork | Where the answer is |
|---|---|
| Is the Scale track invitation confirmed? | Organisers. Projects are evaluated only for the track selected at submission |
| Which eligible protocol — Blend v2 or DeFindex — and what is its testnet address and deposit interface? | Mentor / protocol docs |
| Is there a test anchor that handles TRY? | **11:00 Anchor Integration Workshop** |
| Can a Soroban contract read another contract's wasm hash on-chain? | Stream 8's spike |
| Stellar Skill files | HACKATHON.md records *"None cited yet"* and citing them is an explicit requirement. Log every path from now on, as it is used |

## 7. Risks and fallbacks

| Risk | Fallback |
|---|---|
| On-chain gate turns out to be impossible | The front-end gate. The demo looks the same; the on-chain version becomes roadmap item one |
| No anchor handles TRY | Demonstrate the flow against whatever test anchor exists and **do not call it a lira on-ramp**. Say: *the SEP-24 flow works end to end against a test anchor; a TRY partner is an integration question*. Judges know what testnet anchors can do — what they are scoring is whether the flow was built |
| Protocol integration stalls | The ecosystem sweep (stream 7) demonstrates ecosystem engagement on its own |
| A live demo fails on stage | Record every beat in advance, especially 4 and 6 |

## 8. The narrative

The Scale track is about extending infrastructure that already exists. The sentence to land:

> The first SCF award paid for the verification engine, and the engine works — live, on
> testnet, with real jobs behind it. This weekend we added two things: we took verification out
> of the hands of a single operator, and we put it in the path of money.

That is a different claim from "we built something at a hackathon", and it is the continuity
the jury and an SCF reviewer are listening for.

## 9. Appendix — integration points for stream 1

Surveyed 2026-09-19 against `aed66fb`, so a fresh session does not have to re-read 2,311 lines.
Verify before relying on any line number; the code moves.

**Where to hook**

| What | Where |
|---|---|
| Verification completes and is persisted | `crates/api/src/server.rs:797` — `state.db.complete(id, status, &report_json)` inside `run_job` |
| Build permit already released | `server.rs:785` — anything after this holds no build slot |
| Admission permit | `_admission` at `server.rs:774`. **Do not move or clone it into the spawned task**, and do not acquire from `state.admission` |
| Report is otherwise discarded | `server.rs:793-798` — keep `report` (or its three fields) alive for the task |

**Database**

| What | Where |
|---|---|
| Migrations — ordered, append-only, `PRAGMA user_version` | `crates/api/src/db.rs:93-111`. Add a new element; never edit element 1 |
| `verifications` table | `db.rs:95-110` |
| `fail_orphaned_pending` | `db.rs:238-248` — touches only `verifications`, only `status='pending'`. This is exactly why attestations belong in their own table |
| Row type / columns / mapping to mirror | `db.rs:50-68`, `db.rs:354-355`, `db.rs:357-384` |

**The report**

`ReproductionReport` at `crates/verifier-core/src/reproduce.rs:139-164`:

| Field | Type | Note |
|---|---|---|
| `source_sha256` | `String` | lowercase hex |
| `bldimg_digest` | `Option<String>` | the **full `repo@sha256:…` string** — split on `@sha256:` and hex-decode. `None` for a local image, in which case attestation must skip cleanly rather than fail |
| `bldopt` | `Vec<String>` | submission order matters for the claim digest |

**The claim digest** is pinned in `contracts/registry/src/claim.rs`. The API must reproduce it
byte for byte:

```
sha256( "sorofy-claim-v1" ‖ source_sha256(32) ‖ bldimg_digest(32)
        ‖ u32_be(count) ‖ for each bldopt: u32_be(len) ‖ bytes )
```

The contract's known-answer test pins this against vectors produced independently in Python.
**Use the same vectors in the API's test** — otherwise both sides merely repeat one author's
reading of the layout.

**Config** — there is no config struct; env vars are read inline in `main.rs:26-38`, convention
`SOROFY_*`, booleans as `=1` (copy the idiom at `main.rs:29`). Needed: `SOROFY_ATTEST=1`
(default off), `SOROFY_ATTEST_KEY_FILE`, `SOROFY_REGISTRY_CONTRACT_ID`. Note `main.rs:61` logs
the startup line — the key must not reach it. `AppState::new` has a fixed signature
(`server.rs:209-215`) with call sites at `main.rs:58`, `server.rs:983` and `server.rs:1220`.

**Signing** — there is no signing infrastructure in the repo at all: no `ed25519`, no soroban
client crate, no `simulateTransaction` / `sendTransaction` / `getTransaction`. `rpc.rs` is 149
lines and read-only. Shelling out to `stellar-cli` is therefore the route, **with one keystore
per instance** (`STELLAR_CONFIG_HOME`): the CLI signs an auth entry with any key in the local
keystore, not only the transaction's source account, which is a live hazard once three
verifiers share a machine.

**The registry call** — `attest(verifier, wasm_hash, input_digest, rebuilt_hash)` at
`contracts/registry/src/lib.rs:291-342`. Three of its errors are **permanent, not retryable**:
verifier not active (under-staked), attest window closed, and already attested. Retry logic
must distinguish these from transport failures.

**The public feed** — `GET /verifications` at `server.rs:828-845`, unauthenticated, returns
`{total, limit, offset, items}`. `VerificationRow` (`db.rs:50-68`) is `Serialize` only, so a
follower needs its own mirror type. `contract_id` is `Option` — rows carrying only a bare
`wasm_hash` must be skipped.
