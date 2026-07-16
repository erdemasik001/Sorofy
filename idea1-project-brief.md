# Soroban Contract Verification Service
### Project Brief — Idea 1

*One-liner: An open-source, multi-verifier source verification service that proves a Soroban smart contract's on-chain WASM bytes were built from the public source code shown on explorers.*

---

## 1. Problem

Today on Stellar/Soroban, when a contract is deployed to mainnet, all that lives on-chain is **opaque bytes.** A user has **no programmatic way** to confirm that the source code shown on an explorer actually compiles to those bytes.

Concrete consequences:

- **Explorers can't safely display source code.** In fact, Stellar Lab **removed** its source-code tab after a malicious WASM was shown receiving a "build verified" badge.
- **Auditors can't trust explorer-surfaced source** — they can't be sure the code they reviewed matches what was deployed.
- **The existing solution (SEP-55) is insufficient.** SEP-55 uses GitHub Actions attestations to prove "a CI ran at this commit" — but that CI can fetch external code or swap dependencies. It proves **provenance**, not **source-to-bytecode correspondence.** As OrbitLens put it, this gives "a false sense of security."

**In short:** what Etherscan/Sourcify do on Ethereum and verified-builds does on Solana has no production-grade equivalent on Soroban.

---

## 2. Solution

A service that proves source-code → deployed-WASM correspondence by **rebuilding**:

1. A developer (or explorer) submits a contract's source (tarball or git commit) plus the target WASM hash.
2. The service **rebuilds** the source in a **deterministic, isolated environment** (a digest-pinned Docker image).
3. It **byte-compares** the sha256 of the produced WASM against the on-chain WASM.
4. The result is computed **once and cached**; explorers/wallets/Lab fetch it via a free public API with a cheap lookup (no need for everyone to rebuild themselves).

This builds on SDF's newly published **SEP-58** vocabulary (the metadata needed to reproduce a WASM from source: `bldimg`, `bldopt`, `source_repo`, `source_rev`, `tarball_url`, `tarball_sha256`). SEP-58 is only the "vocabulary"; **the service, infrastructure, and integration layer are this project's job.**

---

## 3. How it works (flow)

```
Developer / Explorer
        │  source (tarball|git) + target WASM hash
        ▼
┌─────────────────────────────┐
│  Verification Service        │
│  1. read SEP-58 meta         │
│  2. bldimg (pinned Docker)   │
│  3. rebuild in sandbox       │
│  4. compare sha256           │
└─────────────────────────────┘
        │  result (verified / mismatch) + source link
        ▼
   Cache + Public API  ◄── Explorer / Wallet / Lab / stellar-cli (cheap query)
```

Multiple independent verifier instances publish the same result, so a consumer never has to trust a single party.

---

## 4. Technical stack

| Layer | Technology / Approach |
|---|---|
| **Deterministic build** | Digest-pinned **single-arch** Docker image (`stellar-cli` entrypoint, `wasm32v1-none` target, `--locked`, `RUSTUP_TOOLCHAIN` pinning) |
| **Isolation** | Sandboxed rebuild (isolated from host; submitted code can't exfiltrate secrets or affect other jobs) |
| **Comparison** | sha256 byte match |
| **Service / API** | Free, public, KYC-free REST API — query by contract ID or WASM hash; verify-once + cache |
| **Networks** | Mainnet + Testnet |
| **Registry** | Off-chain / on-chain registry for pre-SEP-58 (non-upgradable) contracts — retroactive verification |
| **Decentralization** | Multiple independent verifier instances; architecture that surfaces disagreement |
| **Distribution** | Open-source + self-hostable; API shaped for stellar-cli integration |

**Complexity: medium-high.** The hard part isn't the API/explorer; it's **reproduction hygiene, the sandbox infrastructure, retroactive verification, and multi-verifier decentralization.** This is essentially a supply-chain / build-infra problem — not DeFi economics — so it maps directly onto an embedded/systems + Rust/CLI background.

---

## 5. Why us — differentiation

The RFP's 3 hardest requirements are also the parts competitors do least. Differentiate here:

1. **Decentralization / multi-verifier** *(RFP "hard requirement")* — "A single hardcoded verifier does not meet the bar." An architecture where independent verifiers publish results is the least-built piece.
2. **Retroactive verification** *(RFP "priority requirement")* — an off-chain registry for pre-SEP-58 contracts that can't embed metadata. The most-skipped but highest-value requirement.
3. **Multi-dimensional trust model** — instead of a binary "verified/unverified," expose image trust levels (arbitrary / publicly auditable / SDF-maintained) + a vetted image allowlist.

---

## 6. Competitors

| Competitor | Status | Gap |
|---|---|---|
| **StellarExpert** (OrbitLens) | Runs SEP-55 attestation **live** | Has provenance, no source→bytecode proof |
| **SDF `contract-verifications`** | Experimental prototype, "do not use in production" | Not production |
| **stellar-cli** (build/verify) | WIP; build layer only | No service/registry/cross-explorer layer |
| **SoroSeal** | **Actively positioned** (deterministic build + registry + certificate) | **Closest direct competitor — watch** |

**Bottom line:** no production-grade, multi-verifier, open-source service exists yet. The gap is open.

---

## 7. Funding and model

- **Directly funded:** the SCF "Contract Source Verification Service" RFP.
- **Award:** Build Award cap of **$150K XLM**, tranches **10/20/30/40** (acceptance / MVP / Testnet / Mainnet).
- **Audit:** Mandatory third-party audit — cost covered separately by **Audit Bank**.
- **Evaluation:** No community vote; 2 delegate reviewers (a 3rd breaks ties). The need is already validated by SDF — you only have to prove "we're the right team." The most accessible path for a solo/small team.

---

## 8. Roadmap (milestones)

| Stage | Content | Estimate |
|---|---|---|
| **M0 — Acceptance** | Interest form + application (architecture + "why us") | — |
| **M1 — MVP** | Testnet verifier + public API + basic Docker reproduction | 6–8 weeks |
| **M2 — Testnet** | Decentralization (multi-verifier) + retroactive verification + registry | +4–6 weeks |
| **M3 — Mainnet** | Audit prep → Audit Bank review → production + at least 1 reference integration (ideally Stellar Lab) | +4–8 weeks |

The SCF framework already runs ~4 months (max 6) to mainnet.

---

## 9. Risks

| Risk | Type | Mitigation |
|---|---|---|
| **Competition** — the RFP winner becomes the canonical verifier (StellarExpert/SoroSeal/SDF) | Business *(biggest risk)* | Differentiation + early application + ecosystem referral |
| Rust/host non-determinism | Technical | Container (the RFP also scopes out full determinism) |
| "Reproducible ≠ faithful to source" (malicious image) | Security | Vetted image allowlist |
| SEP-58 still Draft, may change | Standard | Early-mover advantage; track the spec closely |
| SCF round timing (#43 vs #44 conflicting) | Process | Clarify via interest form |

---

## 10. First steps

1. **communityfund.stellar.org** → SCF Interest Form → RFP Track + "Contract Source Verification Service." The current round/deadline gets clarified here. (Add a referral code if you have one.)
2. Deep-read SEP-58 (v0.3.0) + discussion #1923 + the SDF `contract-verifications` prototype + stellar-cli #2506.
3. Verify a testnet contract end-to-end and see firsthand where reproduction breaks today.
4. Application package: architecture diagram (Mermaid) + milestone plan + "why this team."

---

## Sources

- SEP-58 spec (Draft v0.3.0): https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0058.md
- SCF RFP (Verification Service full text): https://stellar.gitbook.io/scf-handbook/scf-awards/build-award/rfp-track
- SCF Build Award / reward structure: https://communityfund.stellar.org/awards
- Soroban Audit Bank: https://stellar.gitbook.io/scf-handbook/supporting-programs/audit-bank
- SDF experimental verifier: https://github.com/stellar-experimental/contract-verifications
- stellar-cli reproducible builds (#2506): https://github.com/stellar/stellar-cli/issues/2506
- SoroSeal (competitor): https://www.soroseal.tech/
- Sourcify (EVM reference): https://sourcify.dev
- Solana verified builds (reference): https://solana.com/docs/programs/verified-builds
