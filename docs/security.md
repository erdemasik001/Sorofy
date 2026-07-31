# Sorofy — Security Threat Model

> Phase 0.1 deliverable. This models the security posture of the verification
> service *before* it is exposed publicly on testnet, and derives the hardening
> requirements the rest of Phase 0 must satisfy. Grounded in the current code
> ([`docker.rs`](../crates/verifier-core/src/docker.rs),
> [`reproduce.rs`](../crates/verifier-core/src/reproduce.rs),
> [`source.rs`](../crates/verifier-core/src/source.rs),
> [`server.rs`](../crates/api/src/server.rs)).

## Why this service is a hard security problem

Sorofy does two dangerous things by design:

1. **It compiles attacker-supplied source.** A verification request names source
   the submitter controls; `stellar contract build` runs that source's
   `build.rs` and proc macros — arbitrary code — on our infrastructure.
2. **It drives the host Docker daemon.** In the deploy model the API container
   mounts `/var/run/docker.sock`, so it can create sibling containers on the
   host. Control of that socket is effectively root on the host.

The whole security story is about keeping (1) from turning into control of (2)
or the host.

## Assets

| Asset | Why it matters |
|---|---|
| **Host machine / Docker daemon** | Socket access ≈ host root; the crown jewel |
| **Verification integrity** | A wrong `verified` destroys the product's entire value |
| **Service availability** | Builds are heavyweight; the service is easy to starve |
| **The published build image** | If swapped, every verification is invalid (already mitigated by digest pinning) |
| **Other tenants' jobs** | One job must not observe or corrupt another |

## Trust boundaries

```
[ public internet ]
      │  HTTP (untrusted input: contract_id, source URI/repo, bldimg)
      ▼
[ sorofy-api ]  ── talks to ──▶ [ host Docker daemon ]  (socket mount = host-root)
      │                                   │
      │ creates                           ▼
      ▼                          [ build containers ]  ◀── run UNTRUSTED code
[ SQLite cache ]                 fetch(net on) │ build(--network=none)
```

Everything crossing "public internet → sorofy-api" is untrusted. Everything the
build container runs is untrusted. The daemon and the host are trusted and must
stay that way.

## Threat actors

- **Malicious submitter** — crafts source / URIs / `bldimg` to escape the
  sandbox, reach the host, or exfiltrate data.
- **Abuser** — floods the service with expensive builds to deny service.
- **Forger** — tries to get a wrong `verified` recorded for a contract.

## Attack surfaces

### S1 — Untrusted code execution during build *(highest severity)*

The build compiles and runs submitter code. Existing mitigations (real, in code):

- **No network during build.** The build phase runs `--network=none`
  ([`reproduce.rs`](../crates/verifier-core/src/reproduce.rs)); only the
  `cargo fetch` phase has the network, and it runs no user code.
- **Non-root.** Builds run as `builder` (uid 1000), not root.
- **No bind mounts.** Source enters and artifacts leave over tar streams
  (`docker cp -`), so no host path is exposed inside the container.
- **Ephemeral.** Containers and the per-job `CARGO_HOME` volume are force-removed
  on drop, including on failure.
- **Wall-clock timeout.** The build is killed if it exceeds its budget.

**Gap G1 — no resource limits on the build container.** `Docker::create`
([`docker.rs`](../crates/verifier-core/src/docker.rs)) sets `--network`,
`--workdir`, `--volume`, `--env`, but **no `--memory`, `--cpus`, `--pids-limit`,
or disk quota**. A hostile `build.rs` can allocate until the host OOMs, spin
unbounded CPU/threads, or fill the disk via the writable `CARGO_HOME` volume. The
wall-clock timeout bounds *time*, not *resources*. **Must fix before public
exposure.**

### S2 — Docker socket mount = host-root *(highest severity, deploy-model)*

Mounting `/var/run/docker.sock` into the API container gives it full control of
the host daemon: it could start a `--privileged` container, bind-mount `/`, and
own the host. The API code doesn't do that, but any RCE *in the API process*
would inherit that power.

Mitigations: the API surface is small and typed (`deny_unknown_fields`), and the
box is single-tenant. **Accepted for a single-tenant testnet demo box**, but it
is exactly why S3/S4 (auth, abuse) are mandatory before exposure, and why
rootless Docker / a socket-proxy / true DinD is a tracked follow-up.

### S3 — Unauthenticated, un-throttled API *(must fix for Phase 0)*

Today any caller can `POST /verify` and drive a build. Existing good properties:

- **The caller cannot assert the target.** With a `contract_id`, the expected
  hash is resolved on-chain via RPC, never taken from the request
  ([`server.rs`](../crates/api/src/server.rs)) — this defends verification
  integrity (the Forger).
- **Concurrency is capped** at 2 concurrent builds by a semaphore.

**Gap G2 — no authentication.** Anyone on the internet can spend our build
capacity and exercise the socket-mounted daemon indirectly. → Phase 0.2 (auth on
`POST`; `GET` stays public, it's a cheap read).

**Gap G3 — no rate limiting / unbounded queue.** The semaphore caps *parallel*
builds, but `start_verification` inserts a row and `tokio::spawn`s a job per
request with no admission control — an attacker can enqueue unboundedly, growing
memory and the pending backlog. → Phase 0.3 (per-token/IP rate limit + a bounded
queue).

### S4 — Server-Side Request Forgery via source fetch *(medium-high)*

`fetch_archive` does `ureq::get(uri)` on a submitter-supplied URI, and `fetch_git`
runs `git clone` on a submitter-supplied repo
([`source.rs`](../crates/verifier-core/src/source.rs)), both from the API host
with the network on. **Gap G4:** a submitter can point these at
`http://169.254.169.254/…` (cloud metadata), `http://localhost:…`, or internal
addresses to probe or exfiltrate. Existing size cap (`MAX_ARCHIVE_BYTES`, 256 MB)
bounds the *body*, not the *destination*. → Restrict egress to public hosts
(deny link-local / RFC-1918 / loopback), or fetch from an egress-proxied,
network-segmented context.

### S5 — Malicious source archive *(handled; keep the tests)*

A crafted archive tries path traversal (`../`) or a many-top-dir layout to escape
the staging dir when `docker cp` unpacks it. Mitigations in
[`source.rs`](../crates/verifier-core/src/source.rs): traversal components are
rejected, the archive must have exactly one top-level directory, and the declared
`source_sha256` is checked **before** anything is unpacked (SEP-58 step 3). These
are unit-tested; the tests must stay.

### S6 — Verification integrity / cache poisoning *(handled; note)*

The recorded result must reflect a real rebuild. Defended by: on-chain hash
resolution (S3), digest-pinned `bldimg` (a bare tag is rejected before any
container), and deterministic rebuild. The trust-level allowlist (Phase 1) is a
further layer. No known gap; revisit when multi-verifier (Phase 3) lands.

## Prioritized gaps → Phase 0 requirements

| ID | Gap | Severity | Addressed by |
|---|---|---|---|
| **G1** | No memory/CPU/PID/disk limits on the build container | High | Sandbox hardening (new task) |
| **G2** | No auth on `POST /verify` | High | Phase 0.2 |
| **G3** | No rate limit / unbounded job queue | High | Phase 0.3 |
| **G4** | SSRF via submitter-supplied source URI / repo | Med-High | Sandbox hardening (new task) |
| **G5** | Socket mount = host root (tenancy) | Med | Accepted single-tenant; rootless/proxy tracked for post-M2 |

## Residual risk & decisions

- **Single-tenant deploy is a deliberate choice for the testnet phase.** The
  socket-mount model (G5) is safe only while one trusted operator owns the box.
  Multi-tenant hosting requires rootless Docker or a socket proxy — out of scope
  for M2, tracked for later.
- **Determinism ≠ faithfulness.** A reproducible build only proves "these bytes
  came from this source in this image," not that the image is honest. The image
  allowlist (Phase 1) and multi-verifier (Phase 3) address that; the threat model
  is revisited at each.

## Exit note

This pass **adds two hardening items** not in the original Phase 0 list —
G1 (build resource limits) and G4 (SSRF egress control) — folded into a new
"Sandbox hardening" task. Auth (G2) and rate-limiting (G3) proceed as planned.
