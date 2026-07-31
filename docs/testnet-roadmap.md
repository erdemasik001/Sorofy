# Sorofy — Testnet Productization Roadmap

> Status: the MVP was accepted/awarded by the SCF. This roadmap turns it from a
> deliberately-frozen MVP into a **testnet-grade product**. Mainnet, third-party
> audit, and external integrations are the *next* step (M3) and are parked here on
> purpose — see [Parked: mainnet phase](#parked--mainnet-phase-m3).
>
> Supersedes the MVP day-plan in [PLAN.md](../PLAN.md), which is complete.

## Scope

- ✅ **Current target — Testnet milestone (SCF M2):** get the service deployed to
  testnet, then complete the M2 differentiators (trust model, retroactive
  registry, decentralization).
- ⏸️ **Next step — Mainnet milestone (SCF M3):** mainnet support, Audit Bank
  review, a reference integration. Parked until M2's quality gate is green.

The target has two layers:

1. **"Deployable to testnet" (immediate)** → **Phase 0**. A live, authenticated,
   observable service. This alone meets the literal near-term goal.
2. **"Testnet milestone complete (M2)"** → **Phases 1–3**, built on top of the
   live service.

## Definition of "flawless" (the bar every phase is held to)

A phase is *done* only when it meets all of:

- **Correctness:** zero tolerance for a wrong `verified`/`mismatch`. This is the
  entire value of the service.
- **Tested:** every layer is proven — unit + integration (Docker/RPC) + e2e; the
  critical paths run automatically in CI.
- **Secure:** this service compiles untrusted code and mounts the Docker socket,
  so every phase is reviewed through a security lens.
- **Observable:** every outcome is logged/measured; an SLO is defined.
- **Reversible:** every change has a rollback path.

**Phase-transition rule:** the next phase does not start until the current
phase's quality gate is green.

---

## Phase 0 — Deployable to testnet *(immediate block)*

**Goal:** the current MVP becomes a securely deployed, authenticated, observable
live testnet service. Deploy artifacts already exist ([`docker/api/Dockerfile`](../docker/api/Dockerfile),
[`fly.toml`](../fly.toml)); what's missing is the hardening that must precede any
public exposure.

### Checklist (live status)

- [x] **0.1 Security pass** — threat model ([`docs/security.md`](security.md), commit `595b2d0`)
- [x] **0.2 Auth** — bearer token on `POST /verify`, `GET` public (commit `7504dac`)
- [x] **0.3 Rate-limiting** — per-principal token-bucket rate limit + bounded admission queue on `POST /verify` (G3)
- [ ] **0.4 Observability** — `/health`, structured logs, metrics (job count/duration/outcome)
- [ ] **0.5 Persistence hardening** — schema migrations + volume backup
- [ ] **0.6 Integration test lane** — wire the `#[ignore]`d Docker/RPC tests into CI; make `extract_wasm` artifact-selection offline-testable
- [ ] **0.7 Deploy to testnet host** — VPS + Docker-out-of-Docker, live URL; API image built `--locked` *(blocked by 0.2–0.5, 0.9, 0.10)*
- [ ] **0.8 Narrative update** — README/pitch reflect "testnet productization"
- [x] **0.9 Sandbox hardening** — build resource limits (G1) + SSRF guard (G4), surfaced by 0.1 (commit `3541656`)
- [ ] **0.10 Sandbox hardening completion (retrospective)** — audit residuals in 0.9: G1 disk quota + `--memory-swap`, G4 per-hop redirect re-validation + in-container `cargo fetch` egress, new G6 `--cap-drop=ALL`/`--security-opt=no-new-privileges` (docs/security.md status update)

**🚦 Phase 0 quality gate:** live testnet URL responds · unauthenticated `POST`
returns 401 · security pass documented · integration lane green in CI · SLO
baseline captured.

The table below is the rationale for each item.

| Work | Why it's required | Builds on |
|---|---|---|
| **Security pass (do first)** — threat model for the untrusted-build sandbox + socket-mount model | The service compiles untrusted code and controls the host daemon; the risk surface must be understood before it faces the internet | `docker.rs` isolation, `reproduce.rs` two-phase split |
| **Sandbox hardening completion (retrospective)** — close the audit residuals in the already-"done" task: G1 disk quota + `--memory-swap`; G4 per-hop redirect re-validation + in-container `cargo fetch` egress; new G6 `--cap-drop=ALL` / `--security-opt=no-new-privileges` | The sandbox-hardening task (`3541656`) under-delivered against its own threat model; these must close before public exposure | `security.md` status update; `docker.rs` `create_args`; `source.rs` guard |
| **Auth** — bearer token on `POST /verify`; `GET` stays public | Exposing a socket-mounted service without auth risks the host | axum middleware over the router |
| **Rate-limiting** — quota per token/IP | Builds are heavyweight; abuse is a DoS | build queue already caps concurrency at 2 |
| **Deploy** — testnet host (VPS + Docker-out-of-Docker), live URL; API image built `--locked` for a reproducible runtime | The target itself | `docker/api/Dockerfile`, `fly.toml`, `.dockerignore` |
| **Observability** — `/health`, structured logs, basic metrics (job count/duration/outcome) | "Flawless" has to be measurable | `tracing` is already wired |
| **Persistence hardening** — migration mechanism + volume backup | No data loss across deploy/restart | `db.rs` single table |
| **Integration test lane** — wire the `#[ignore]`d Docker/RPC tests into a real CI lane; also refactor `extract_wasm` to select from tar bytes so its artifact-selection logic is unit-testable offline | What we deploy must be proven; the artifact selector currently has zero coverage | 6 reproduction + 1 RPC ignored tests |
| **Sandbox hardening** — build resource limits (`--memory`/`--cpus`/`--pids-limit`) + SSRF egress guard | A hostile `build.rs` could OOM/fork-bomb the host; source fetch could reach internal addresses (docs/security.md G1, G4) | `docker.rs`, `source.rs` |
| **Narrative update** — README/pitch-deck reflect "testnet productization" | Post-award status | docs |

> Passing the Phase 0 gate above **meets the near-term goal**: the product is
> live and secure on testnet. Phases 1–3 complete M2 on top of it.

**Primary risk:** mounting the host Docker socket grants the container control of
the host daemon. Mitigation: auth + single-tenant isolation now; evaluate
rootless/DinD later.

---

## Phase 1 — Real trust model

**Goal:** promote `trust_level` from the hardcoded `arbitrary` to real,
allowlist-backed tiers.

- Vetted-image allowlist (config/DB): digest → `publicly-auditable` /
  `sdf-maintained`.
- Document vetting criteria (SBOM, SLSA/attestation, reproducible image) — a
  draft already lives in [`docs/sep-58-notes.md`](sep-58-notes.md).
- Replace the constant in [`reproduce.rs`](../crates/verifier-core/src/reproduce.rs)
  with an allowlist lookup. The `TrustLevel` enum and response schema are already
  wired end-to-end.

**🚦 Quality gate:** our own GHCR image resolves to `publicly-auditable`; a
digest not on the list stays `arbitrary`; both paths are unit-tested.

---

## Phase 2 — Retroactive registry *(M2 priority requirement)*

**Goal:** attach source to contracts that carry no on-chain metadata. The engine
path is already proven; the registry/service layer is what's missing.

- **Data model:** `contract_id | wasm_hash → {source_uri, source_sha256, bldimg,
  submitter, vetting_status}` in its own table.
- **Submission flow:** propose source → engine verifies → on `verified`, record
  into the registry.
- **Abuse resistance:** who may submit, spam / malicious-source protection,
  moderation.
- **Query API:** return the recorded source for a pre-SEP-58 contract.
- **Proof target:** find the source of Day0's `CDZZZTN6…` (the one real target
  whose source is still unknown) and verify it through the registry.

**🚦 Quality gate:** a metadata-less contract returns `verified` from a
third-party-supplied source; spam/malicious submissions are rejected (tested).

---

## Phase 3 — Decentralization / multi-verifier *(M2 hard requirement)*

**Goal:** remove the need to trust a single verifier, and **surface
disagreement.** The RFP calls a single hardcoded verifier "does not meet the bar."

- Independent instances publish the same result as a **signed attestation** (new
  `crates/attestation`).
- Quorum/consensus plus an `agreement: 3/3` vs `disagreement` field in the API.
- An attestation format that a third party can independently verify.
- **Determinism prerequisite (review).** Before cross-verifier agreement is trusted,
  the git and archive source paths must build under the same absolute path. Today git
  stages at `/build/source` and archive at `/build/<repo>-<sha>`, and
  `--remap-path-prefix` only covers `$CARGO_HOME/registry/src` (day1) — so if a source
  path ever reaches the WASM, two honest verifiers fed the same source in different
  shapes would disagree, manufacturing a false `disagreement`. Not observed on the two
  contracts tested (both git and archive converged byte-for-byte); normalise the staged
  top-dir to a constant first.

**🚦 Quality gate:** ≥2 independent instances can be compared on the same
contract; a deliberately-tampered instance is flagged as `disagreement`
(tested). → **Testnet milestone (M2) complete.**

---

## Parked — mainnet phase (M3)

Out of scope for the current target; opens once M2's gate is green:

- **Mainnet support** — `SOROFY_RPC` is already env-driven, so this is config +
  testing, not a rewrite.
- **Third-party audit** — via the Soroban Audit Bank (cost covered separately).
- **Reference integration** — ideally Stellar Lab; alternatively `stellar-cli` /
  an explorer badge.
- **Production ops** — SLOs, alerting, backups, `/v1` API versioning.

---

## Quality spine *(runs through every phase)*

- **Test pyramid:** unit → integration (Docker/RPC lane) → e2e → load (build
  queue) → security.
- **Supply-chain:** `cargo audit` / `cargo deny` in CI.
- **CI/CD evolution:** current (fmt + clippy + test) → + integration lane →
  + `cargo audit` → + deploy pipeline.
- **Docs:** API reference, self-host guide, ops runbook.

Funded by the SCF Contract Source Verification Service RFP — testnet tranche (M2)
is the current target; mainnet tranche (M3) is next.
