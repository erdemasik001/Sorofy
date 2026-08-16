# Sorofy — Testnet Deploy Playbook (roadmap 0.7)

> **Status: executed end to end on 2026-08-16.** The service is live at
> [`https://sorofy.site`](https://sorofy.site) and all 11 smoke tests passed. Every
> command below ran in order on a fresh Ubuntu 24.04 VPS (Contabo Cloud VPS 4,
> 4 vCPU / 8 GB / 100 GB, ext4).
>
> This is the deploy half of roadmap [0.7](testnet-roadmap.md) and it closed the two
> controls that are deploy config rather than code:
> [ADR-0001](adr/0001-fetch-egress-control.md) action items 2–4 (fetch egress, G4-b)
> and [security.md](security.md) G7 (TLS).
>
> **What the first run changed.** Four things this document did not anticipate, folded
> into the steps below so the next host does not rediscover them:
>
> - **Rapid successive SSH connections are dropped** by the provider's edge as a
>   brute-force pattern. Drive the whole deploy over one multiplexed session
>   (`ControlMaster auto` + `ControlPersist`), not a connection per command.
> - **`ufw enable` over SSH kills the session** when it reloads the ruleset, which
>   aborts the rest of a chained command. Run it detached — `systemd-run --no-block` —
>   so it completes regardless, then reconnect and verify.
> - **Smoke test 3 needs a well-formed body.** Auth resolves *after* axum's JSON
>   extractor, so `-d '{}'` returns `422`, not the `401` the test expects. A complete
>   request body with no token is the correct probe (see [security.md](security.md)).
> - **Read the SLO baseline before any restart.** `/metrics` counters are
>   process-lifetime and reset to zero; the cache does not.

## What you need before starting

| Input | Why |
|---|---|
| A Linux VPS with root, ≥4 GB RAM, ≥40 GB disk | A build gets `--memory 3g` ([`BUILD_LIMITS`](../crates/verifier-core/src/reproduce.rs)) and two may run at once; the host needs headroom above that |
| A domain (or subdomain) pointing at it | TLS certificates are issued against a name, not an IP (G7) |
| The GHCR build image digest | `ghcr.io/erdemasik001/sorofy-build-image@sha256:cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588` |

**Why a VPS and not Fly.io.** The service spawns sibling build containers, so it
needs a Docker daemon it can reach — Docker-out-of-Docker. On a VPS that is the
host's own daemon over a mounted socket. On Fly, each machine is a microVM with no
ambient daemon, so a wrapper must start `dockerd` inside it with extra privileges;
and the egress control below is a *host firewall* rule, which on Fly would have to
move inside the machine. [`fly.toml`](../fly.toml) stays as a prepared alternative,
not the recommended path.

**Code prerequisites (already landed).** The API image builds `--locked`; the
service shuts down gracefully on SIGTERM and reconciles jobs orphaned by a
restart. Without those, step 4's restart test would strand rows as permanently
`pending`.

---

## Step 1 — Host baseline

```bash
# Non-root user for the service, key-only SSH, firewall down to what we serve.
adduser --disabled-password --gecos "" sorofy
ufw default deny incoming && ufw allow 22,80,443/tcp && ufw enable

# Docker Engine from the official repo (distro packages lag).
curl -fsSL https://get.docker.com | sh
usermod -aG docker sorofy
```

> **`ufw` does not contain Docker.** Docker writes its own `iptables` rules and a
> published port bypasses `ufw` entirely — a port `ufw` reports as closed can be
> open to the internet. That is why step 4 publishes on loopback only and step 2
> filters in `DOCKER-USER`, the chain Docker consults before its own.

## Step 2 — Fetch network + egress filter *(ADR-0001 action items 2–3; closes G4-b)*

The dependency-fetch container needs the internet (crates.io, git hosts) but must
not reach anything internal: an attacker-controlled `Cargo.lock` can name any
host, and no user code needs to run for that fetch to happen. The build phase is
already `--network=none`; this step constrains the one phase that has a network.

```bash
# Deterministic bridge name so the firewall rules below can name the interface.
docker network create \
  --subnet 172.28.0.0/16 \
  -o com.docker.network.bridge.name=sorofy-fetch0 \
  sorofy-fetch
```

Check `docker network ls` first — `172.28.0.0/16` must not collide with an
existing network.

```bash
# Routed traffic out of the fetch subnet: drop everything internal.
for dst in 169.254.0.0/16 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 127.0.0.0/8 100.64.0.0/10; do
  iptables -I DOCKER-USER -s 172.28.0.0/16 -d "$dst" -j DROP
done

# Traffic to the host itself arrives on INPUT, not FORWARD, so DOCKER-USER never
# sees it. Without this a fetch container could still reach services bound on the
# host's own addresses.
iptables -I INPUT -i sorofy-fetch0 -j DROP
```

Notes that matter:

- **`169.254.0.0/16` covers `169.254.169.254`**, the cloud metadata endpoint —
  the highest-value SSRF target on any VPS.
- **`172.16.0.0/12` contains the fetch subnet itself**, so fetch containers cannot
  talk to each other either. Intended: they have no reason to.
- **DNS keeps working.** A container resolves via Docker's embedded resolver
  inside its own netns; the daemon performs the upstream lookup from the host, so
  these rules do not sit in that path. Step 6's crates.io test is what proves it.
- **IPv6.** The network is IPv4-only, so containers have no v6 egress to filter.
  If IPv6 is ever enabled on it, mirror every rule above in `ip6tables` — an
  unmirrored v6 path silently reopens the whole control.

**Persist the rules.** `iptables-persistent` restores rules at boot, but
`DOCKER-USER` is created by `dockerd`, so a restore that runs first has nowhere to
put them. Install them after Docker instead, idempotently:

```ini
# /etc/systemd/system/sorofy-egress.service
[Unit]
Description=Sorofy fetch-network egress rules
After=docker.service
Requires=docker.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/sorofy-egress.sh

[Install]
WantedBy=multi-user.target
```

`sorofy-egress.sh` runs the loop above with `iptables -C … || iptables -I …` per
rule, so re-running it never duplicates. Enable it, reboot, and re-run step 6's
egress test — a control that does not survive a reboot is not a control.

## Step 3 — Images

```bash
docker pull ghcr.io/erdemasik001/sorofy-build-image@sha256:cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588
git clone https://github.com/erdemasik001/Sorofy.git && cd Sorofy
docker build -f docker/api/Dockerfile -t sorofy/api:0.7.0 .
```

Tag with a version, never only `latest`: rollback (below) is "run the previous
tag", which needs the previous tag to still exist.

## Step 4 — Run the API

```bash
openssl rand -hex 32 > /home/sorofy/api-token   # chmod 600, this is the credential

docker run -d --name sorofy-api --restart unless-stopped \
  -p 127.0.0.1:8080:8080 \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v sorofy_data:/data \
  -e SOROFY_API_TOKEN="$(cat /home/sorofy/api-token)" \
  -e SOROFY_RPC=https://soroban-testnet.stellar.org \
  -e SOROFY_BACKUP_DIR=/data/backups \
  -e VERIFY_FETCH_NETWORK=sorofy-fetch \
  sorofy/api:0.7.0
```

- **`-p 127.0.0.1:8080:8080`** — only the proxy can reach it (G7).
- **`VERIFY_FETCH_NETWORK=sorofy-fetch`** — the fetch phase runs on the filtered
  network. The service does *not* create it: a missing network fails the run
  loudly, which is the intended behaviour (silently creating one would give
  unfiltered egress under a name implying the opposite).
- **`SOROFY_ALLOW_UNPINNED_IMAGE` stays unset** — a bare-tag `bldimg` must keep
  being rejected before any container starts.
- **No `SOROFY_BIND`** — the image already binds `0.0.0.0:8080` *inside* the
  container; the publish above is what limits exposure.
- Retention for `/data/backups` is the operator's: a cron
  `find /var/lib/docker/volumes/sorofy_data/_data/backups -mtime +30 -delete`.

## Step 5 — TLS *(closes G7)*

```bash
docker run -d --name caddy --restart unless-stopped --network host \
  -v /etc/caddy/Caddyfile:/etc/caddy/Caddyfile:ro \
  -v caddy_data:/data \
  caddy:2
```

```
# /etc/caddy/Caddyfile
sorofy.site {
    reverse_proxy 127.0.0.1:8080
}
```

Caddy obtains and renews the certificate itself. `--network host` is what lets it
reach the loopback-published API.

## Step 6 — Smoke tests = the Phase 0 quality gate

Run in order; every one must pass before the deploy counts as done.

| # | Test | Pass condition | Closes |
|---|---|---|---|
| 1 | `curl https://sorofy.site/health` | `200 {"status":"ok"}` over TLS | gate: live URL |
| 2 | `curl -sk http://<host-ip>:8080/health` | connection refused — the API is not reachable around the proxy | G7 |
| 3 | `curl -X POST https://…/verify` with a **complete body** and no token | `401` | gate: auth |
| 4 | ~25 authenticated `POST`s fired **in parallel** | the burst's first 10 pass, the rest return `429` + `Retry-After` | G3 live |
| 5 | `docker run --rm --network sorofy-fetch curlimages/curl -m 5 http://169.254.169.254/` | fails (timeout) | **G4-b** |
| 6 | same, to an RFC-1918 address on the host's LAN | fails | **G4-b** |
| 7 | same, `https://static.crates.io/` | succeeds — filtering did not sever legitimate egress or DNS | **G4-b** |
| 8 | End-to-end verify of the fixture contract (below) | `verified`, `rebuilt` == `expected` == on-chain hash | gate: correctness |
| 9 | Re-POST with `wasm_hash` tampered to `1111…` | `mismatch` | correctness |
| 10 | `docker restart sorofy-api`, then `GET` the earlier job | result survives; no row left `pending` | reversibility |
| 11 | `curl https://…/metrics` after 8–9 | record `avg_build_seconds` as the SLO baseline | gate: SLO |

The end-to-end case (8) — the same fixture the README documents, whose expected
hash comes from the network, not from us:

```bash
curl -X POST https://sorofy.site/verify \
  -H "Authorization: Bearer $(cat /home/sorofy/api-token)" \
  -H 'Content-Type: application/json' -d '{
  "contract_id": "CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6",
  "repo": "https://github.com/erdemasik001/sorofy-fixture-token",
  "rev": "cd68767f3b36456228b01244ecd4e6f935b5e986",
  "bldimg": "ghcr.io/erdemasik001/sorofy-build-image@sha256:cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588"
}'
```

Test 10 is not ceremony: a restart is what a redeploy *is*, and until the
orphaned-job sweep landed it left rows claiming `pending` forever.

Test 4 needs the requests to be **concurrent**, not merely rapid. Fired in a serial
loop each `POST` waits on a Soroban RPC round trip, so they arrive about one per
second — exactly the token-bucket refill rate, and the bucket never drains. Firing
them in parallel produced the expected shape immediately: 10 accepted, 15 `429`.
Point them at a `contract_id` that does not resolve on-chain and the accepted ones
fail at validation instead of queueing 10 real builds.

**First-run results (2026-08-16).** All 11 passed. Test 8 reproduced
`47d2801e115f9a064fe37a8244ef1ffcfa56877668a17f383d3189634a1bcfbd` — expected,
rebuilt and on-chain identical, 8 584 bytes; test 9 returned `mismatch`. Across the
run: 14 jobs, 13 `verified`, 1 `mismatch`, 0 errors, `avg_build_seconds` **86.49** —
recorded as the SLO baseline.

## Step 7 — Close the paperwork

Only after every test above passes. **All done 2026-08-16:**

- [x] [security.md](security.md): G4-b → closed (the control is deploy config plus its
      smoke test); G7 → closed, with a note on the per-site HSTS header that stands in
      for `.dev`'s TLD-wide preload. Three findings from the run recorded, including a
      new **G8** (no CSP on the explorer).
- [x] [ADR-0001](adr/0001-fetch-egress-control.md): action items 2–4 ticked.
- [x] [testnet-roadmap.md](testnet-roadmap.md): 0.7 and 0.10 → done; Phase 0 gate green,
      which is what unblocks Phase 1.
- [x] [README](../README.md): live URL added; the honest-status list re-cut against what
      is now true; the explorer documented (it was shipped but appeared in no doc).
- [x] [pitch-deck.html](pitch-deck.html) and [day3-deploy-demo.md](day3-deploy-demo.md):
      "no live public URL" and "fetch egress unfiltered" moved out of the not-yet-true
      columns.
- [x] SLO baseline from test 11 recorded: `avg_build_seconds` **86.49** over 14 jobs.

## Rollback

| Failure | Action |
|---|---|
| Bad API build | `docker stop sorofy-api && docker run … sorofy/api:<previous tag>` — the volume is untouched, so the cache survives |
| Corrupted cache | Stop the API, restore a `/data/backups/sorofy-*.db` snapshot over `sorofy.db`, restart. Snapshots are `VACUUM INTO` copies, i.e. valid databases |
| Schema regression after a downgrade | The service *refuses* to start against a newer schema rather than corrupting it — roll forward, or restore a snapshot from before the migration |
| Egress rules break legitimate fetches | `iptables -D DOCKER-USER …` restores the previous behaviour immediately; unset `VERIFY_FETCH_NETWORK` to fall back to the default bridge (unfiltered — a deliberate, temporary regression) |
| Anything worse | Stop Caddy: the service is unreachable but nothing is destroyed |

## Accepted risks at this stage

- **Socket mount = host root (G5).** Anyone who achieves RCE in the API process
  controls the daemon and therefore the host. Accepted for a single-tenant testnet
  box; rootless Docker or a socket proxy is tracked for after M2.
- **Disk quota off (G1).** `--storage-opt size=` needs a quota-capable storage
  driver, which overlay2 on ext4 is not. A build's writable layer is unbounded;
  the mitigation is host disk headroom and monitoring. Enable it if the host's
  driver supports it.
- **Single node.** No HA, no failover. The cache is one SQLite file on one volume,
  which is exactly why the snapshot job is enabled above.
