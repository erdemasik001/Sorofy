# Day0 — Manual Reproduction Findings

Goal: build a Soroban contract from source, record the WASM hash, and learn where
reproduction breaks — so Day1's build engine targets the real risks.

## Setup

- **Windows host:** `rustc 1.91.1`, `stellar-cli 23.2.1`, target `wasm32v1-none`.
- **WSL2 Ubuntu** (for the cross-env check, Docker-free): `rustc 1.91.1` (pinned via rustup) +
  `stellar-cli 23.2.1` (prebuilt linux-gnu binary) — versions matched to the Windows host on purpose.
- Sample contract: `stellar contract init` → `hello-world` example (default scaffold).

## Experiment 1 — same-machine reproducibility (Windows)

| Build | WASM hash | Size |
|---|---|---|
| Native build #1 (Windows) | `b68602842d3a1d169d54fe3e57c0511a774df4710553d6d4d22e653d62bf5f5b` | 660 B |
| Native build #2, clean `target/` rebuild | `b68602842d3a1d169d54fe3e57c0511a774df4710553d6d4d22e653d62bf5f5b` | 660 B |

`stellar contract build` reports the WASM hash directly in its build summary. A clean rebuild
(deleted `target/`) produced the exact same hash → the WASM is a deterministic function of
toolchain + source, not of build-time entropy on a single host.

## Experiment 2 — cross-environment determinism (Windows vs Linux/WSL2, Docker-free)

Rebuilt the **same source** (copied with its `Cargo.lock`, `target/` excluded) on Linux, with the
**toolchain versions pinned to match** the Windows build:

- `rustc 1.91.1` (installed in WSL2 via `rustup toolchain install 1.91.1`, `RUSTUP_TOOLCHAIN=1.91.1`)
- `stellar-cli 23.2.1` — identical build (commit `496ac35b…`), downloaded as the prebuilt
  `x86_64-unknown-linux-gnu` release binary
- target `wasm32v1-none`

| Environment | Host | WASM hash | Size |
|---|---|---|---|
| Windows | `x86_64-pc-windows-msvc` | `b68602842d3a1d169d54fe3e57c0511a774df4710553d6d4d22e653d62bf5f5b` | 660 B |
| **WSL2 Ubuntu** | `x86_64-unknown-linux-gnu` | `b68602842d3a1d169d54fe3e57c0511a774df4710553d6d4d22e653d62bf5f5b` | 660 B |

**Result: byte-identical (MATCH).** With the toolchain pinned, the host OS/platform did **not** change
the `wasm32v1-none` output for this contract.

### What this means for the build image (the valuable output)

- Reproducibility is driven by **toolchain versions + locked dependencies**, not the host OS. For a
  faithful rebuild the image must pin, at minimum:
  - Rust toolchain version (`rustc`/`cargo`) — via `RUSTUP_TOOLCHAIN` baked into the image.
  - `stellar-cli` version (drives build + optimization/`wasm-opt`).
  - `soroban-sdk` / all deps — via committed `Cargo.lock` + `--locked`.
  - target `wasm32v1-none`; plus any `bldopt` flags (e.g. `--optimize`).
  - → exactly what SEP-58 `bldimg` (digest-pinned, single-arch) transitively freezes.
- **Caveat:** this is a trivial `hello-world`. Non-determinism can still surface with larger contracts
  (embedded absolute paths via `--remap-path-prefix`, build scripts, codegen units, timestamps). Day1
  must re-run this check on the real target below before trusting it.

## Experiment 3 — first on-chain reproduction target (real testnet contract)

Picked a live Soroban **testnet** contract from the StellarExpert index and independently confirmed
its on-chain WASM hash via RPC (`stellar contract fetch`, `soroban-testnet.stellar.org`):

| Field | Value |
|---|---|
| `contract_id` | `CDZZZTN6RXOWY2WDJV2GLFAV76YKAIRFPNB4EABMFAVJQ5DCZIAE4DYA` |
| on-chain WASM hash | `bfab576fb405952fdeb0c502e3f662668601f3b63111bcf034c240cea4b6240d` |
| WASM size | 46 892 B |
| verification | `sha256(stellar contract fetch …)` == StellarExpert-reported hash ✅ |

**`contractmetav0` contents (read via `stellar contract info meta`):**

```
rsver:    1.95.0                                   (Rust version)
rssdkver: 22.0.11#34f7f53ae31e0fd02aab436a9872e79fa671ca02   (soroban-sdk + commit)
cliver:   25.2.0#28484880988199233a7e8e87c97cb12dac323cb3    (stellar-cli + commit)
```

**No SEP-58 fields** (`bldimg` / `bldopt` / `source_uri` / `source_sha256`) are present. This is the
common real-world case and directly motivates our roadmap:

- The contract records *toolchain hints* (rustc 1.95.0, sdk 22.0.11, cli 25.2.0) but **no source
  pointer and no build image** → we cannot rebuild it until the source is supplied out-of-band.
- → **Retroactive registry (M2)** is required to attach `source_uri` + `source_sha256` (and a vetted
  `bldimg`) to pre-SEP-58 contracts like this one before they can be verified.

## Carried into Day1

- Stand up Docker **inside Ubuntu WSL2** (Docker Desktop is broken on this machine — see project notes)
  and re-run Experiment 2 inside the pinned image to confirm the image reproduces `b68602…`.
- Reproduce `CDZZZTN6…`: needs its source (absent from metadata) + a `bldimg` matching rustc 1.95.0 /
  sdk 22.0.11 / cli 25.2.0. Use it to exercise the retroactive-registry path (supply source manually).
- `verifier-core` pipeline: resolve toolchain from image → `stellar contract build --locked` with
  declared `bldopt`, network-isolated → sha256-compare against the on-chain hash.
