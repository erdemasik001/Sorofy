# Day3 — Retroactive Path, Publish, Deploy-Readiness, Demo

Goal: close the MVP. Prove the differentiating claim (retroactive verification of
a contract that carries no source metadata), make `bldimg` a real registry
digest, and get the service to a deployable state. Carried forward from
[day2-api.md](day2-api.md).

**Honest headline on deploy.** PLAN Day3 item 1 wanted a live public URL. Fly.io
removed its free allowance for new orgs (card required, ~$5/mo minimum) and the
service needs a Docker daemon it can reach, so a live deploy was **deliberately
deferred** for the MVP. Everything else — the retroactive proof, the published
image, the deploy artifacts — is done and real, and the demo runs against a
local instance of the exact same binary. See "Deploy-readiness" below for what
"one command away" concretely means.

## Retroactive path — proven end-to-end (PLAN Day3 item 3)

This is the claim the whole project exists for: a contract already on-chain, with
**no source metadata**, verified by supplying its source **out-of-band**.

Day2 locked `CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6` as the
target and confirmed its `contractmetav0` carries none of the SEP-58 fields
(`bldimg`, `bldopt`, `source_uri`, `source_sha256`) — only toolchain hints. So
nothing on-chain points at its source. Day3 feeds that source back in as if a
third party supplied it, and the engine proves the match against the network's
own record of the hash.

Request to the running API — note there is **no `wasm_hash`**; only a
`contract_id` and an out-of-band archive:

```bash
curl -X POST localhost:8080/verify -H 'Content-Type: application/json' -d '{
  "contract_id": "CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6",
  "source_uri": "https://github.com/erdemasik001/sorofy-fixture-token/archive/cd68767f3b36456228b01244ecd4e6f935b5e986.tar.gz",
  "source_sha256": "1cde007365bb93f6dae9b6f2e42b0bf29364c44fa031399116a6cfafa4ede416",
  "bldimg": "sorofy/build-image:rust1.91.1-cli23.2.1"
}'
# → {"id":1,"status":"pending","wasm_hash":"47d2801e…"}   ← hash came from RPC
```

Result (`GET /verify/1`):

| Field | Value | Where it came from |
|---|---|---|
| `contract_id` | `CAZAVVTM…` | caller |
| `source` | `kind=archive`, `source_sha256=1cde0073…` | **out-of-band**, not on-chain |
| `expected_wasm_sha256` | `47d2801e115f9a064fe37a8244ef1ffcfa56877668a17f383d3189634a1bcfbd` | **resolved from the network** (`getLedgerEntries`) |
| `rebuilt_wasm_sha256` | `47d2801e…` | our containerized rebuild |
| `rebuilt_wasm_size` | 8 584 B | — |
| `result` | **verified** | byte-identical |
| `build_seconds` | 62.5 | — |
| `bldimg_digest` | `sha256:cff44167…` | resolved from the image |

Why this is the retroactive path and not a repeat of Day2's E2E case 1:

- Case 1 (Day2) supplied source as `repo`+`rev`. Here source is a **SEP-58
  archive** (`source_uri`+`source_sha256`) — the shape a retroactive submitter
  actually has when they don't own the repo, just a tarball.
- The target hash is **resolved on-chain**, not asserted by the caller. The
  submitter cannot pick the answer; they can only supply a candidate source, and
  the network decides whether it reproduces.
- The contract itself contributes **zero** verification metadata. Everything
  needed came from (a) the network's hash and (b) an externally-supplied source.
  That is exactly the pre-SEP-58 / retroactive-registry scenario from the brief.

Day0's `CDZZZTN6…` remains the *stretch* retroactive target: same mechanism, but
its source is genuinely unknown, so it's a sourcing problem, not an engine
problem. The engine side is proven here.

## Publishing the build image — real `bldimg` digest (closes a Day2 "not yet true")

Day2 ran with `SOROFY_ALLOW_UNPINNED_IMAGE=1` because `bldimg` was only a local
tag. SEP-58 wants a single-arch **manifest digest** so two verifiers pulling the
same `bldimg` are guaranteed the same bytes. The image is published to GHCR:

- Registry: `ghcr.io/erdemasik001/sorofy-build-image:rust1.91.1-cli23.2.1`
- Publish is scripted in [`docker/build-image/publish.sh`](../docker/build-image/publish.sh)
  (tag → `docker login ghcr.io` with a `write:packages` token → push → print
  digest), then the package is made public so any verifier can pull it.
- **Published digest:**
  `ghcr.io/erdemasik001/sorofy-build-image@sha256:cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588`

### Enforcement is now real, not a flag

With the image on a registry, the service runs with `allow_unpinned=false`
(production default). Re-running the same retroactive request against this
production-mode instance proves the digest gate has teeth — the enforcement lives
at [`reproduce.rs`](../crates/verifier-core/src/reproduce.rs) (`bldimg` must
contain `@sha256:`):

| `bldimg` submitted | Outcome |
|---|---|
| bare tag `sorofy/build-image:rust1.91.1-cli23.2.1` | **`error`** — `build image must be digest-pinned (image@sha256:...)`, rejected before any container |
| pinned `ghcr.io/erdemasik001/sorofy-build-image@sha256:cff44167…` | **`verified`**, 48.7 s, `rebuilt == expected == 47d2801e…` |

So the retroactive proof above, which Day2/early-Day3 ran under
`--allow-unpinned-image` against a local tag, now holds against a **registry
digest with enforcement on** — the "`bldimg` has no registry digest" item from
Day2 is closed. (The `bldimg_digest` the report echoes resolves to the same
`sha256:cff44167…`; the local daemon lists several repo aliases for the image and
reports the first, but the digest — the thing that matters — is identical.)

## Deploy-readiness (PLAN Day3 item 1 — prepared, not pushed)

The service spawns build containers, so its one hard platform requirement is a
Docker daemon it can reach. It uses **Docker-out-of-Docker**: the API ships only
the `docker` CLI and talks to a daemon over a mounted socket — simpler and less
privileged than Docker-in-Docker.

Artifacts in the repo:

- [`docker/api/Dockerfile`](../docker/api/Dockerfile) — multi-stage build of
  `sorofy-api` onto a slim runtime carrying `docker` CLI + `git` + `gzip`.
  Verified to build (`docker build -f docker/api/Dockerfile -t sorofy/api .`).
- [`fly.toml`](../fly.toml) — Fly config (volume for the SQLite cache, 2 GB VM so
  cargo builds don't OOM, digest enforcement on). Marked not-deployed.
- `.dockerignore` — keeps `target/` and the repo's git history out of the context.

**VPS path (simplest, recommended if deploying).** On any Linux host with Docker:

```bash
docker build -f docker/api/Dockerfile -t sorofy/api .
docker run -d --name sorofy-api -p 8080:8080 \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v sorofy_data:/data \
  -e SOROFY_RPC=https://soroban-testnet.stellar.org \
  sorofy/api
```

The socket mount is what lets the API launch the pinned build image as a sibling
container. No privileged DinD, no `--privileged`. (Mounting the host socket
grants the container control of the host daemon — fine for a single-tenant demo
box, and the reason auth/rate-limiting is the first hardening item, below.)

**Fly path.** Fly machines are microVMs with no ambient daemon, so a start
wrapper must launch `dockerd` inside the machine before `sorofy-api`, and the
machine needs the extra privileges dockerd wants. That's why `fly.toml` is a
starting point, not turnkey — and why the VPS path is the recommended one.

## Demo script (for the recording)

Two contrasting cases, ~2 minutes, all against a local `sorofy-api`:

1. **Retroactive verify (the money shot).** The POST above → poll `GET /verify/1`
   → `verified`, with `expected` == `rebuilt` == the on-chain hash. Narrate: "the
   contract stored no source; we supplied it separately; the network's own hash
   confirms the rebuild."
2. **Tamper → mismatch.** Re-POST with `wasm_hash` set to `1111…` (or a
   one-word-changed source) → `mismatch`, build log retained. Shows the check has
   teeth.

Exact commands live in the README "The API" section. The recording itself
(GIF/video) is captured by hand; the commands are copy-paste runnable.

## Still not true (post-MVP, brief M2/M3)

- **No live public URL** — deploy deferred (cost + DinD), artifacts ready.
- **No auth / rate limiting** — mandatory before exposing the socket-mounted
  service publicly. First item on the deploy hardening list.
- **`trust_level` hardcoded `arbitrary`** — the allowlist / multi-verifier
  surface (brief M2) is where `publicly-auditable` / `sdf-maintained` come from.
- **Single verifier** — decentralization (independent verifiers publishing and
  surfacing disagreement) is the M2 differentiator, architected for but not built.
