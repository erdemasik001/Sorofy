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
2. **"Testnet milestone complete (M2)"** → **Phases 1–3**. Built *for* the live
   service; not necessarily *after* it — see the phase-transition rule below.

## Funded scope — the Instawards SOW *(read this before picking work)*

The money behind the current work is a **30-day Instawards engagement**, not the
SCF M2 milestone this document is otherwise organised around. The SOW was
submitted 2026-07-18 with a suggested sprint start of **2026-07-19**, so the
window closes around **2026-08-17** unless the chapter lead started the clock
elsewhere — worth confirming, because it is roughly half spent.

It funds exactly three deliverables, and its out-of-scope list is explicit:

| SOW deliverable | Status | What is missing |
|---|---|---|
| **1. Deterministic build engine** (`verify-core` CLI, digest-pinned image, sha256 vs on-chain) | ✅ Built | Evidence only: a screenshot of a CLI run showing `verified` and a `mismatch` on altered source |
| **2. Public REST API, **live on testnet**, cached, `trust_level` in the schema | ⚠️ **Half** — the API is built and tested; it is **not deployed** | The live URL (roadmap 0.7), 2–3 real contracts verified through it, and a screenshot of a `GET` returning JSON |
| **3. Retroactive path** (source + vetted `bldimg` out-of-band, ≥1 real pre-SEP-58 contract) | ✅ Proven (day3) | Evidence only: a demo recording plus the contract id |

**Explicitly out of SOW scope:** multi-verifier/decentralisation ("architected and
documented as the next milestone"), mainnet + audit, explorer/wallet UI, an
on-chain registry contract, and guaranteed determinism across all contracts.

**Consequence for this roadmap.** Phases 1–3 below are *next-milestone* work, not
funded deliverables — Phase 3 is on the SOW's out-of-scope list by name, and
Phase 2 goes far past deliverable 3, which needs one contract verified, not a
registry with a submission and moderation flow. The only funded item still open
is **0.7 (deploy) plus the three evidence artifacts**. Everything else in this
document is post-SOW.

### Delivery plan

Two of the three deliverables are already built; what remains is the deploy and
the evidence that makes the work reviewable. The evidence is a deliverable in its
own right, not paperwork: the SOW is signed off by the Ambassador Chapter Lead
and says the proof must be reviewable "with minimal technical expertise", so a
live URL and a recording carry more weight at sign-off than any amount of code.

| Block | Work | Needs a host? |
|---|---|---|
| **A — Evidence that needs nothing** | A1 screenshot of a `verify-core` run showing `verified` and, on altered source, `mismatch` (deliverable 1, complete after this) · A2 parameterise [`scripts/demo.ps1`](../scripts/demo.ps1), whose API URL is currently hardcoded to localhost, so the recording can run against the live service · A3 a one-page delivery note aimed at a non-technical reviewer | No |
| **B — Deploy** | The [playbook](deploy-playbook.md) end to end: host baseline → fetch network + egress rules → image → run → TLS → the 11 smoke tests. Smoke tests 8–9 (fixture `verified`, tampered → `mismatch`) *are* the deliverable-2 evidence, not extra work | Yes |
| **C — Evidence on the live service** | C1 screenshot of `GET /verify/{id}` returning JSON with `status` and `trust_level` · C2 verify 2–3 real testnet contracts through the live API · C3 record the demo (retroactive verify, then tamper → `mismatch`) | Yes |
| **D — Close the paperwork** | Playbook step 7: live URL into the README, G4-b + G7 closed, ADR action items 2–4 ticked, 0.7 done | Yes |

**Contract inventory for C2:** two are ready — `CAZAVVTM…` (the ~40 KB real-size
token) and the hello-world fixture (660 B). The SOW asks for 2–3, so a third is
optional; it would mean deploying another `soroban-examples` contract to testnet.

**Estimate:** ~1.5–2 working days total, of which block A (~half a day) can be
done before a host exists. The playbook has never been executed, so budget first-run
friction; and point the domain's DNS at the host early, since the TLS step cannot
issue a certificate before it resolves.

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

> **Recorded exception (2026-08-03) — Phase 1 may start while 0.7 waits.**
> The rule exists so features are not piled onto an unhardened service. It was
> never meant to block on a *purchase*: Phase 0's only open item is the deploy
> itself, which needs a host. The hardening the rule actually cares about — auth,
> rate limiting, sandbox limits, egress seam, observability, persistence — is done
> and tested.
>
> **Superseded in priority the same day** by the SOW review above: Phases 1–3 are
> not funded deliverables, and the deploy they were going to run ahead of *is*
> one. The exception stands for when the SOW is delivered; until then it is not a
> licence to start Phase 1 instead of deploying.
>
> What still holds, and is not negotiable: **nothing is exposed publicly until
> the Phase 0 gate is green.** G4-b (fetch egress) and G7 (TLS) are open, so a
> live URL before the playbook runs would be exposing a service we have already
> written down as unsafe to expose. Phase 1 work lands in the repo, not on the
> internet.
>
> Phases 2 and 3 inherit this exception; 0.7 stays the gate for *going live*, not
> for *building*.

**Scope discipline (2026-08-03).** Phase 0 grew from 8 items to 10 plus
sub-items, each addition individually justified by a real finding. That pattern
does not stop on its own, so the residual hygiene items below are explicitly
**deferred until after M2's phases**, not carried as ambient work:

| Deferred residual | Why it can wait |
|---|---|
| Symlink target validation (S5) | Recorded residual, low impact: the container is non-root and bind-mount-free, and modern `docker cp` extraction is symlink-safe |
| `cargo deny` license/source policy | `cargo audit` already covers advisories, which is the security half |
| Read-only rootfs for build containers (G6 follow-up) | Defense-in-depth on an already capability-dropped, non-root, network-less build |
| Build-queue load test | Admission bound and rate limit are unit-tested; real load needs the live host anyway |
| API reference / self-host guide / ops runbook | The playbook covers the operator path; the rest is polish before M3 |

None of these is a correctness or exposure risk on their own, and none is an M2
deliverable. Revisit as a batch once Phase 3's gate is green — or sooner if one
turns out to block a phase.

---

## Phase 0 — Deployable to testnet *(one item open: the deploy itself)*

**Goal:** the current MVP becomes a securely deployed, authenticated, observable
live testnet service. Deploy artifacts already exist ([`docker/api/Dockerfile`](../docker/api/Dockerfile),
[`fly.toml`](../fly.toml)); what's missing is the hardening that must precede any
public exposure.

### Checklist (live status)

- [x] **0.1 Security pass** — threat model ([`docs/security.md`](security.md), commit `595b2d0`)
- [x] **0.2 Auth** — bearer token on `POST /verify`, `GET` public (commit `7504dac`)
- [x] **0.3 Rate-limiting** — per-principal token-bucket rate limit + bounded admission queue on `POST /verify` (G3)
- [x] **0.4 Observability** — `GET /health` (liveness + db ping), `GET /metrics` (job counters), structured per-request logs
- [x] **0.5 Persistence hardening** — versioned schema migrations (`user_version`, forward-only, downgrade-refusing) + `VACUUM INTO` snapshots, optionally periodic via `SOROFY_BACKUP_DIR`
- [x] **0.6 Integration test lane** — `integration.yml` runs the `#[ignore]`d Docker/RPC tests (push:master + manual); `extract_wasm` selection split into offline-testable `select_wasm_from_tar`
- [ ] **0.7 Deploy to testnet host** — VPS + Docker-out-of-Docker, live URL.
  *Prepared:* a step-by-step [deploy playbook](deploy-playbook.md) (host baseline,
  egress-filtered fetch network, TLS proxy, smoke tests, rollback); the API image
  now builds `--locked`; the service shuts down gracefully and reconciles jobs
  orphaned by a restart; the missing transport-security control is modelled (G7).
  *Remaining:* a host. Everything still open here is execution, not design
- [x] **0.8 Narrative update** — README + pitch deck reflect the post-award posture: what Phase 0 delivered, and an honest "not yet true" list (deploy, fetch egress, trust levels, single verifier)
- [x] **0.9 Sandbox hardening** — build resource limits (G1) + SSRF guard (G4), surfaced by 0.1 (commit `3541656`)
- [ ] **0.10 Sandbox hardening completion (retrospective)** — G1 disk quota + `--memory-swap` ✅, G6 `--cap-drop=ALL`/`no-new-privileges` ✅, G4-a per-hop redirect re-validation ✅, G4-b fetch-egress **code seam** ✅ (`VERIFY_FETCH_NETWORK`); remaining: G4-b host firewall rules + smoke test, which land with 0.7 ([ADR-0001](adr/0001-fetch-egress-control.md))

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
> live and secure on testnet. Phases 1–3 complete M2. They are being built in
> parallel with the deploy under the recorded exception above; the gate still
> governs when any of it faces the internet.

**Primary risk:** mounting the host Docker socket grants the container control of
the host daemon. Mitigation: auth + single-tenant isolation now; evaluate
rootless/DinD later.

---

## Phase 1 — Real trust model *(queued: post-SOW)*

**Goal:** promote `trust_level` from the hardcoded `arbitrary` to real,
allowlist-backed tiers.

Not a funded deliverable — the SOW asks only that `trust_level` be *in the
response schema*, which it is. This is the cheapest of the three M2 phases (the
`TrustLevel` enum and the schema are already wired end to end, so only the lookup
and its policy are missing) and it needs no host, which makes it the natural
first item once the SOW's deploy and evidence are done.

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
- **Proof target (stretch, not the gate):** find the source of Day0's
  `CDZZZTN6…` (the one real target whose source is still unknown) and verify it
  through the registry. This is open-ended research — the source may simply not
  be public — so it must not be on the critical path. The gate below is met by
  any metadata-less contract with a third-party-supplied source; the day3
  retroactive path already proves that shape.

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
- ✅ **Determinism prerequisite (done).** Cross-verifier agreement is only
  meaningful if the git and archive paths build under the same absolute path. They
  did not: git staged at `/build/source`, an archive at `/build/<repo>-<sha>`, and
  `--remap-path-prefix` only covers `$CARGO_HOME/registry/src` (day1) — so if a
  source path ever reached the WASM, two honest verifiers fed the same commit in
  different shapes would have manufactured a false `disagreement`. Every staged
  tree is now re-rooted onto a constant (`STAGED_TOP_DIR`, `source.rs`), so the
  workdir is `/build/source` for both, and the staged tar is byte-identical
  whichever shape was fetched — unit-tested, and live-verified by the archive path
  still reproducing the fixture's on-chain hash byte-for-byte.

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
- **Supply-chain:** ✅ `cargo audit` over the locked tree — on dependency changes
  *and* weekly, since an advisory can land against code that never changed
  (`audit.yml`). Informational advisories (unmaintained/yanked) report without
  failing the lane; `cargo deny`'s license/source policy is still open.
- **CI/CD evolution:** current (fmt + clippy + test) → + integration lane →
  + `cargo audit` ✅ → + deploy pipeline.
- **Docs:** [deploy playbook](deploy-playbook.md) ✅; API reference, self-host
  guide and ops runbook still open.

Funded by the SCF Contract Source Verification Service RFP — testnet tranche (M2)
is the current target; mainnet tranche (M3) is next.
