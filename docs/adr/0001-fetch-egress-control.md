# ADR-0001: Egress control for the in-container `cargo fetch` phase (G4-b)

**Status:** Accepted — implemented and verified live (code seam + firewall rules, 2026-08-16)
**Date:** 2026-07-31
**Deciders:** Sorofy maintainer/operator (single-tenant testnet)
**Related:** [`docs/security.md`](../security.md) G4 · roadmap item 0.10 · [`reproduce.rs`](../../crates/verifier-core/src/reproduce.rs)

## Context

The retrospective security audit ([`security.md`](../security.md) status update) left one
G4 residual open after G4-a (host-side redirect re-validation) landed:

> **In-container `cargo fetch` egress is unguarded.** `guard_public_url` covers only the
> API host's own fetch. The fetch container runs `Network::Bridge` (`reproduce.rs`), and
> `cargo fetch` dials whatever git/registry hosts the attacker-controlled
> `Cargo.toml`/`Cargo.lock` name — internal addresses and the metadata endpoint included.

Concretely, the pipeline is split in two ([`reproduce.rs`](../../crates/verifier-core/src/reproduce.rs)):

| Phase | Network | Runs untrusted code? | Why it needs the network |
|---|---|---|---|
| `cargo fetch --locked` | `Bridge` (full egress) | No — downloads/unpacks only | Resolve crates.io + git deps named in `Cargo.lock` |
| `stellar contract build` | `None` | **Yes** (`build.rs`, proc macros) | — (offline; resolves from the shared `CARGO_HOME`) |

So the *build* is already network-isolated. The gap is that the *fetch* phase, which must
have egress to reach crates.io and git dependencies, will dial **any** host the submitter's
`Cargo.toml`/`Cargo.lock` names — including `169.254.169.254` (cloud metadata), `127.0.0.1`,
and RFC-1918 addresses reachable from the deploy network.

**Forces at play:**

- **Blind, but real.** No submitter code runs during fetch, and the fetched bytes are not
  reflected to the caller, so this is *blind* SSRF. But the TCP connection still reaches the
  internal endpoint — enough to probe liveness/ports, and (for the metadata endpoint) enough
  to matter if any response side-channel exists.
- **Deploy model is DooD, single-tenant.** The API ships only the `docker` CLI and drives the
  host daemon over a mounted socket ([`docker/api/Dockerfile`](../../docker/api/Dockerfile),
  [`day3-deploy-demo.md`](../day3-deploy-demo.md)). Fetch/build containers are **siblings on
  the host daemon**, not children of the API container. There is exactly one host to configure.
- **Must not break legitimate deps.** Cargo's default is the sparse HTTPS registry
  (`static.crates.io`/`index.crates.io`); git deps are typically `https://github.com/...`. Any
  control must leave those working, or every real contract build breaks.
- **Defense-in-depth, not a rewrite.** The app-layer guard (`guard_public_url`/`is_internal`)
  already defends the API host's own fetch. G4-b is about the *container's* egress, a different
  layer; the two should stack.
- **Testnet posture.** G5 (socket mount = host-root) is already accepted for single-tenant
  testnet. G4-b should match that bar: close the modelled SSRF (reaching internal addresses),
  not build multi-tenant-grade isolation now.

## Decision

Adopt **Option A — an egress-filtered network for the fetch container**, implemented as
host firewall rules (iptables `DOCKER-USER` chain) that drop traffic from a **dedicated
fetch network** to internal address ranges, applied as part of deploy (roadmap 0.7).

Supporting code change (small, in-repo, testable): give fetch containers their **own named
docker network** instead of the default bridge, so the firewall rule has a stable subnet to
target and the policy is legible (`sorofy-fetch` = "the network that may only reach the public
internet"). Keep the app-layer host guard (G4-a) unchanged — the two layers stack.

Defer **Option B (allowlisting proxy)** to multi-tenant / M2, where a positive allowlist and
per-host policy earn their keep.

## Options Considered

### Option A: Egress-filtered network (host firewall on a dedicated fetch subnet)

Put the fetch container on a dedicated docker network; on the host, add `DOCKER-USER` rules
that DROP packets from that network's subnet to internal ranges (`169.254.0.0/16`, `10/8`,
`172.16/12`, `192.168/16`, `127/8`, `100.64/10`, `::1`, `fc00::/7`, `fe80::/10`), allowing
everything else (the public internet).

| Dimension | Assessment |
|-----------|------------|
| Complexity | Low–Med — a handful of firewall rules + a named network |
| Cost | Near-zero (no extra running component) |
| Scalability | Fine for single-host; per-host config for multi-host |
| Team familiarity | High — mirrors the existing `is_internal` blocklist, one layer down |

**Pros:**
- **Protocol-agnostic.** Blocks at the packet layer, so it covers cargo's HTTPS registry,
  https/git/git+ssh dependencies, and raw DNS alike — nothing can bypass it by choosing a
  scheme the app doesn't proxy.
- **Directly closes the modelled threat.** The SSRF risk is "reach an internal address"; an
  IP-range drop is exactly that control. Same policy as `is_internal`, enforced in the kernel.
- **No trust in cargo/git cooperation.** Unlike a proxy, it does not rely on the client
  honouring `http_proxy`.
- **Single auditable artifact.** One firewall ruleset in the deploy playbook; easy to review.

**Cons:**
- **Deploy-environment config, not app code.** It is only "done" when the deploy playbook
  applies it; a fresh host without the rules is unprotected. Needs a verification step
  (a fetch to `169.254.169.254` from the fetch network must fail).
- **Blocklist, not allowlist.** Allows all public egress. A compromised-but-public host could
  still be dialled; acceptable for blind fetch SSRF, weaker than a positive allowlist.
- **DNS still resolves.** Names resolve (DNS may go to a public resolver), but the *connection*
  to an internal A/AAAA record is dropped. A DNS-only exfil channel is out of scope here.

### Option B: Allowlisting HTTP(S) proxy

Run a small forward-proxy container that only permits CONNECT/GET to allowlisted hosts (or
public IPs); point the fetch container at it via `http_proxy`/`https_proxy`/`CARGO_HTTP_PROXY`.

| Dimension | Assessment |
|-----------|------------|
| Complexity | Med–High — extra container, cargo/git env wiring, allowlist upkeep |
| Cost | A long-lived proxy process + its maintenance |
| Scalability | Good policy control; the allowlist becomes a maintenance surface |
| Team familiarity | Medium — new component in the deploy topology |

**Pros:**
- **Positive allowlist.** Deny-by-default; strongest posture — only crates.io + known git hosts
  are reachable at all.
- **Host-level, layer-7 policy.** Can log/deny by hostname, not just IP; better for audit.

**Cons:**
- **Bypassable by protocol.** git `git://`/`ssh://` deps and anything not honouring
  `http_proxy` sidestep the proxy entirely — so it still needs Option A underneath to be safe,
  making it strictly *more* than A, not instead of.
- **Allowlist breakage.** A legitimate dependency hosted off the allowlist (a self-hosted git
  crate, a mirror) fails the build until the operator amends the list — friction that lands on
  real users.
- **More moving parts** in a service whose whole security story is "keep it small and typed."

### Option C: No egress at all (pre-vendored deps) — rejected

Vendoring every dependency offline would eliminate fetch egress, but the service verifies
*arbitrary* contracts whose dependency sets are unknown ahead of time. Not viable.

## Trade-off Analysis

The core trade-off is **protocol-agnostic blocklist (A)** vs **layer-7 allowlist (B)**.

- B is a stronger *posture* but is **not self-sufficient**: because git and non-HTTP schemes
  bypass an HTTP proxy, B only actually contains egress when A sits underneath it. So the real
  choice is "A now" vs "A now + B on top". For a **single-tenant, blind-SSRF** threat, A alone
  closes the modelled risk (reaching internal addresses) with the least new surface.
- A's honest weakness is that it lives in deploy config, not Rust — it can be forgotten on a
  new host. The dedicated-network code change mitigates this (the policy target is explicit and
  named), and a deploy-time smoke test ("fetch net cannot reach 169.254.169.254") makes the
  control verifiable rather than assumed.
- B's allowlist-maintenance cost is a recurring tax paid in *user-visible build failures*,
  which is the wrong tax for a testnet MVP that wants to verify whatever people submit.

Conclusion: **A now**, structured so **B can be added later** without rework (the dedicated
fetch network is exactly where a proxy would be injected for multi-tenant/M2).

## Consequences

- **Easier:** the last G4 residual closes with no new long-lived component; the control mirrors
  `is_internal`, so its policy is already reviewed and understood.
- **Easier later:** a dedicated `sorofy-fetch` network gives a clean seam to bolt an allowlist
  proxy onto for multi-tenant (Option B) without touching the reproduction code again.
- **Harder:** deploy is no longer just `docker run` — the host needs firewall rules and a
  verification step before it can be called hardened. This must be captured in the 0.7 deploy
  playbook, not left tribal.
- **Revisit when:** the service goes multi-tenant or leaves the single-host DooD model (then
  reassess Option B and per-tenant network policy), or if a DNS-exfil channel becomes in-scope.

## Action Items

1. [x] **Code:** `Network::Named` in [`docker.rs`](../../crates/verifier-core/src/docker.rs)
       emits `--network=<name>`; the fetch phase selects it via `VERIFY_FETCH_NETWORK`
       (`reproduce.rs`, unset ⇒ default bridge), build stays `--network=none`. Unit-tested
       (flag emission + the env→network mapping) and live-verified end-to-end on a real
       `sorofy-fetch` network, with the default path confirmed unchanged. Opt-in rather than
       defaulted: the network must be pre-created (and firewalled) by deploy, and a missing
       one fails loudly instead of silently falling back to unfiltered egress.
2. [x] **Deploy (0.7):** done 2026-08-16 on the testnet host. `sorofy-fetch` runs on
       `172.28.0.0/16` with bridge `sorofy-fetch0`; `/usr/local/sbin/sorofy-egress.sh`
       installs `DOCKER-USER` DROP rules for `169.254.0.0/16`, `10.0.0.0/8`,
       `172.16.0.0/12`, `192.168.0.0/16`, `127.0.0.0/8`, `100.64.0.0/10`, plus
       `INPUT -i sorofy-fetch0 -j DROP` for traffic aimed at the host itself. Each rule is
       `-C`-guarded so re-running never duplicates, and `sorofy-egress.service` re-applies
       it `After=docker.service` on every boot — `iptables-persistent` cannot, because
       `dockerd` creates the chain after it would have restored.
3. [x] **Verification:** run live from the fetch network. `169.254.169.254` and an
       RFC-1918 address both fail; `index.crates.io/config.json`, `github.com` and the
       fixture's codeload tarball all return `200`. Recorded as smoke tests 5–7 in the
       [deploy playbook](../deploy-playbook.md).
4. [x] **Docs:** G4-b flipped to closed in [`security.md`](../security.md), noting that the
       control is deploy config plus its smoke test rather than pure code.
5. [ ] **Backlog (M2):** revisit Option B (allowlist proxy on the `sorofy-fetch` seam) for the
       multi-tenant posture.
