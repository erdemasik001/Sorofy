# Day2 — Public API, On-chain Lookup, Cache

Goal: expose the Day1 engine as a REST service backed by a result cache, resolve
on-chain hashes ourselves instead of trusting the caller, and — pulled forward
from Day3 as a risk item — prove the engine on a real-size contract deployed to
testnet. Carried forward from [day1-build-engine.md](day1-build-engine.md).

## Real-size determinism check (PLAN Day2 item 6)

Day1 only ever reproduced a 660-byte hello-world. The risk: build scripts, proc
macros, LTO and codegen surface non-determinism only at real scale. So we built
a real contract in the pinned container, deployed *that artifact*, and then made
the engine reproduce it against the network's own record of its hash.

| Step | Value |
|---|---|
| Source | `soroban-examples` token contract @ upstream `14c069c`, as a standalone package (own `Cargo.lock`, soroban-sdk 27.0.0) |
| Fixture commit | `cd68767f3b36456228b01244ecd4e6f935b5e986` (local; publishing pending) |
| Container build | 74 crates, proc macros, `lto = true`, `codegen-units = 1` → `soroban_token_contract.wasm`, 8 584 B |
| Rebuilt hash | `47d2801e115f9a064fe37a8244ef1ffcfa56877668a17f383d3189634a1bcfbd` |
| Deployed as | [`CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6`](https://stellar.expert/explorer/testnet/contract/CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6) (testnet, our own `sorofy-dev` account) |
| On-chain hash (via our RPC client) | `47d2801e…` — equal |
| `verify-core` vs on-chain hash | **VERIFIED**, exit 0, 53.9 s |

Two builds of the same commit (one to produce the deployed artifact, one during
verification) landed on identical bytes, and the on-chain hash was resolved by
our own `getLedgerEntries` lookup, not taken from the deploy output.

**Honest caveat on size:** the PLAN said "~40 KB". Modern soroban-sdk 27 emits
far smaller binaries than the sdk-22 era (Day0's 46 KB target); the token
contract now compiles to 8.5 KB. The *build complexity* the size was a proxy
for — dozens of dependencies, build scripts, proc macros, LTO — is fully
exercised. Binaries of the old size now imply an old sdk, which needs a second
`bldimg` (the retroactive path, Day3).

The `--emit-wasm` flag was added to `verify-core` for this: it writes the
rebuilt artifact to disk, so what we deployed is *by construction* bytes the
engine can land on again.

## On-chain lookup (`crates/api/src/rpc.rs`)

`contract_id` → the instance's `ContractExecutable`:

- One `getLedgerEntries` call on the instance key
  `(contract, LedgerKeyContractInstance, persistent)`; the entry's
  `ContractInstance.executable` carries the wasm hash the network actually runs.
- `Wasm { wasm_hash_hex }` is the reproduction target; `StellarAsset` means a
  built-in with nothing to rebuild; `None` means no such contract on this
  network.
- Pinned to ground truth in tests: the Day0 target `CDZZZTN6…` must resolve to
  its independently confirmed `bfab576f…`, and our deployed token to
  `47d2801e…` (`cargo test -p api -- --ignored`).

## The service (`crates/api`)

- **`POST /verify`** — SEP-58 fields (`bldimg`, `bldopt`, `source_uri`,
  `source_sha256`) plus the git extension (`repo`+`rev`) and `contract_id`.
  With a `contract_id`, the expected hash is resolved **from the network**, not
  taken from the caller — the caller cannot claim a target. Returns `202` with a
  job id; the build runs on a `spawn_blocking` task behind a 2-slot semaphore
  (container builds are heavyweight; an unbounded queue is a self-inflicted DoS).
- **`GET /verify/{id|contract_id|wasm_hash}`** — newest cached row for the key:
  `pending` / `verified` / `mismatch` / `error`, with the full engine report
  (including `trust_level` and `bldimg_digest`) once finished. Unknown keys are
  `404 not_found`.
- **Cache** — one SQLite table (`crates/api/src/db.rs`); every job is a row.
  Survives restarts: the E2E run below read a previous server process's results.
- Config via env: `SOROFY_BIND`, `SOROFY_DB`, `SOROFY_RPC`,
  `SOROFY_ALLOW_UNPINNED_IMAGE=1` (local dev; SEP-58 digests once the image is
  published on Day3).

### E2E exercised (PLAN Day2 item 7)

| # | Request | Outcome |
|---|---|---|
| 1 | `contract_id=CAZAVVTM…` + token fixture source | hash resolved on-chain → **verified** (48.9 s) |
| 2 | hello-world fixture (public GitHub URL) + explicit `wasm_hash` | **verified** |
| 3 | same source, `wasm_hash=1111…` | **mismatch**, build log retained |
| — | valid-but-absent `contract_id` (`CAAA…BSC4`) | `404` before any build |
| — | malformed `contract_id` / missing source | `400` with reason |

## Not yet true (carried into Day3)

- **`bldimg` still has no registry digest** — `SOROFY_ALLOW_UNPINNED_IMAGE=1`
  in local dev. Publishing the image (Day3) makes digest enforcement real.
- **The token fixture is local-only.** The real-size claim is third-party
  checkable only after the fixture repo is published (same argument as Day1's
  hello-world fixture).
- **No rate limiting / auth.** Fine behind a demo; noted for the deploy.
- `trust_level` still hardcoded `arbitrary` (allowlist is post-MVP).
