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
| Fixture | [`erdemasik001/sorofy-fixture-token`](https://github.com/erdemasik001/sorofy-fixture-token) @ `cd68767f3b36456228b01244ecd4e6f935b5e986` — re-verified from a clean clone of this URL |
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

### Metadata check: the deployed contract carries no SEP-58 fields

PLAN Day2 item 6 requires reading the deployed contract's `contractmetav0` and
confirming the SEP-58 fields are genuinely absent — the same finding Day0 made
on `CDZZZTN6…` ([day0-reproduction-findings.md](day0-reproduction-findings.md)).
Observed via `stellar contract info meta --id CAZAVVTM…OHRT6 --network testnet`:

```
rsver:    1.91.1                                              (Rust version)
rssdkver: 27.0.0#e5cb4b52c3da8e56fc48adfd7b85d85976c1a059     (soroban-sdk + commit)
cliver:   23.2.1#496ac35be7a7d8d923fcde9bbbc650ee593d1f6f     (stellar-cli + commit)
```

**None of the SEP-58 fields (`bldimg`, `bldopt`, `source_uri`, `source_sha256`)
are present.** Current tooling embeds toolchain hints only, exactly as Day0
found on a third-party contract — so even a contract built inside our own
pinned image deploys "unverifiable" by SEP-58 metadata alone.

**This contract is hereby locked as Day3's retroactive-path target.** Its source
is in our hands
([`erdemasik001/sorofy-fixture-token`](https://github.com/erdemasik001/sorofy-fixture-token)
@ `cd68767f3b36456228b01244ecd4e6f935b5e986`), so Day3 can feed source +
`bldimg` "as if supplied out-of-band" with no sourcing risk.

## SEP-58 archive (`source_uri`) path — now exercised end-to-end

Day1 shipped the `source_uri` path (download → sha256 check → gunzip →
single-top-dir → ownership rewrite) but flagged it "not yet exercised against a
real published tarball" ([day1-build-engine.md](day1-build-engine.md)). Day3's
retroactive scenario supplies source *as an archive*, so this path had to be
proven, not assumed. Exercising it surfaced a real bug.

**Bug found (and fixed).** Against the fixture's GitHub codeload tarball
(`…/archive/c08333e9….tar.gz`), the engine rejected the archive:

```
error: source archive must contain exactly one top-level directory, found 2
```

GitHub prepends a `pax_global_header` pseudo-entry (carrying the commit sha) to
every codeload tarball. GNU `tar` and Python's `tarfile` hide it, but the Rust
`tar` crate surfaces it as a standalone entry named `pax_global_header` — so the
SEP-58 step-4 "exactly one top-level directory" walk counted it as a second top.
This only ever bites real tarballs, which is why the git-path tests never caught
it. Fixed in [`source.rs`](../crates/verifier-core/src/source.rs) by skipping
pax extension/global entries (`is_pax_meta`) in both `single_top_dir` and
`normalize_ownership`.

**Results after the fix** (`bldimg sorofy/build-image:rust1.91.1-cli23.2.1`,
`--allow-unpinned-image`; digest resolved to
`sha256:cff44167…`):

| Case | Input | Outcome |
|---|---|---|
| Archive, correct digest | `source_uri` + `source_sha256=1cd1ff3e…`, target `b6860284…` | **VERIFIED**, exit 0, 660 B — byte-identical to the git-path build |
| Archive, wrong digest | same `source_uri`, `source_sha256=0000…` | **rejected** `SourceIntegrity` before any container, exit 2 (attributed to the source, SEP-58 step 3) |

The archive and git paths land on the *same* WASM (`b6860284…`), confirming the
staging fixups (gunzip, pax skip, ownership rewrite) don't perturb the build.
Both cases are now automated as `#[ignore]` tests alongside the four git-path
cases (`verified_archive_source_uri`, `archive_source_uri_bad_digest_rejected`
in [`reproduce_integration.rs`](../crates/verifier-core/tests/reproduce_integration.rs));
the full six-case suite passes under `cargo test -p verifier-core -- --ignored`
(299 s, Docker + WSL2). The tarball's sha256 was stable across downloads at the
time of writing, but the tests digest what they download rather than pin a
constant — the fetch→verify pipeline is what's under test, not the digest's
permanence. **This closes the Day1 "not yet true" `source_uri` item.**

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
| 2 | hello-world fixture (public GitHub URL, `repo`+`rev`) + explicit `wasm_hash` | **verified** |
| 3 | same source, `wasm_hash=1111…` | **mismatch**, build log retained |
| 4 | hello-world as `source_uri`+`source_sha256` (SEP-58 archive) + explicit `wasm_hash` | **verified** (79.7 s), `source.kind=archive` — a *third distinct source shape* through the API |
| — | valid-but-absent `contract_id` (`CAAA…BSC4`) | `404` before any build |
| — | malformed `contract_id` / missing source | `400` with reason |

**Honest scope note on PLAN Day2 item 7.** Item 7 said "2–3 different testnet
contracts verified through the API." What actually happened is narrower, and
deliberately so:

- Only **one** contract — `CAZAVVTM…` (case 1) — was verified against a hash
  **resolved from the network** (the real on-chain-lookup path). That is the row
  that proves the end-to-end claim, and it's a real-size build, not a toy.
- The other verified rows (2 and 4) use an **explicit `wasm_hash`**, not on-chain
  resolution. They exercise the *source* and *response* machinery across two
  different source shapes (`repo`+`rev` and SEP-58 `source_uri`), which is what
  case 4 was added for — a genuinely different source through the API rather than
  a second on-chain contract for its own sake.
- Day0's `CDZZZTN6…` cannot be verified at all: its source is unknown and its
  toolchain differs, so it stays the retroactive/M2 target, not an item-7 row.

So the count is "one real on-chain verification + a second source shape," not
three independent testnet contracts. Standing up more deploys would exercise the
same lookup path already covered by case 1, so we didn't inflate the number.

## Not yet true (carried into Day3)

- **`bldimg` still has no registry digest** — `SOROFY_ALLOW_UNPINNED_IMAGE=1`
  in local dev. Publishing the image (Day3) makes digest enforcement real.
- **No rate limiting / auth.** Fine behind a demo; noted for the deploy.
- `trust_level` still hardcoded `arbitrary` (allowlist is post-MVP).
