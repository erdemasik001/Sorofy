# Day0 — Manual Reproduction Findings

Goal: build a Soroban contract from source, record the WASM hash, and learn where
reproduction breaks — so Day1's build engine targets the real risks.

## Setup (this machine)

- `rustc 1.91.1`, target `wasm32v1-none` installed
- `stellar-cli 23.2.1`
- Sample contract: `stellar contract init` → `hello-world` example (default scaffold)

## Result

| Build | WASM hash | Size |
|---|---|---|
| Native build #1 (Windows) | `b68602842d3a1d169d54fe3e57c0511a774df4710553d6d4d22e653d62bf5f5b` | 660 B |
| Native build #2, clean `target/` rebuild | `b68602842d3a1d169d54fe3e57c0511a774df4710553d6d4d22e653d62bf5f5b` | 660 B |

`stellar contract build` reports the WASM hash directly in its build summary — no separate
sha256 step needed for the local artifact.

## Findings

1. **Same machine + same toolchain reproduces identically.** A clean rebuild (deleted `target/`)
   produced the exact same hash. So the WASM is a deterministic function of the toolchain + source,
   not of build-time entropy on a single host.
2. **The determinism risk is cross-environment, not local.** What can change the hash:
   - Rust toolchain version (`rustc`/`cargo`) — pinned via `RUSTUP_TOOLCHAIN` in the image.
   - `stellar-cli` version (drives the build + optimization).
   - `soroban-sdk` / dependency versions — must be locked (`Cargo.lock` + `--locked`).
   - Target: must be `wasm32v1-none`.
   - Build flags (`bldopt`, e.g. `--optimize`).
   → This is exactly what SEP-58 `bldimg` (digest-pinned, single-arch image) freezes.
3. **Native Windows build is NOT the reference.** To prove correspondence with an on-chain WASM we
   must rebuild in the **pinned Docker image** the contract declared (`bldimg`), not on the host.
   The host build is only a smoke test that the toolchain works.

## Carried into Day1

- Real reproduction test (rebuild a **deployed testnet contract** and match its on-chain hash) needs
  the pinned Docker image + Linux — done at Day1 start inside **Ubuntu WSL2** (Docker Desktop is
  broken on this machine; see project notes).
- `verifier-core` must: resolve toolchain from image, run `stellar contract build` with `--locked`
  and the declared `bldopt` flags, network-isolated, then sha256-compare.
- Pick a testnet contract that already embeds SEP-58 `contractmetav0` (or supply metadata manually)
  as the first end-to-end reproduction target.
