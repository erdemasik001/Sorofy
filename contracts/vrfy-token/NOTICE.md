# Provenance of `vrfy-token`

This contract is the **Soroban standard token example**, used as-is for the VRFY stake token.

- **Upstream:** <https://github.com/stellar/soroban-examples/tree/main/token>
- **Pinned commit:** `1f5aeb53d3db5d0e61e53f59d5e6c5ab58eaf8ce`
  ("rename token approve arg to live_until_ledger (#415)", 2026-09-09)
- **License:** Apache-2.0 — the upstream `LICENSE` is kept beside this file as
  `LICENSE-APACHE-2.0`.
- **Interface:** SEP-41 (Draft, v0.5.2). `mint` and the admin are *not* part of SEP-41; they
  are this example's addition, and here the admin is the deployer.
- **Not audited.** The upstream page points to the OpenZeppelin Stellar Contracts library for
  production use. VRFY is a testnet token with no value.

## What differs from upstream

| Change | Why |
|---|---|
| Package renamed `soroban-token-contract` → `vrfy-token` | Distinct wasm name inside this repository |
| `[profile.release]` moved to the workspace root (`../Cargo.toml`), unchanged in value | Cargo ignores profiles in member manifests |
| `mod test_vrfy;` added to `src/lib.rs`, and `src/test_vrfy.rs` added | Tests specific to how this repository uses the token |
| `Cargo.lock` taken from upstream at the pinned commit | Same dependency versions the upstream example was built with |

Everything else under `src/` is byte-identical to the pinned upstream commit.

## Lints

`cargo clippy` is **not** used as a gate for the upstream files. On this toolchain it reports
one `let_and_return` in a test-only helper (`contract.rs`), and a host-side check of a
`cdylib` reports "never used" for items that the wasm entry points do use. Neither is a
defect in the shipped contract, and changing upstream files would end the "byte-identical"
claim above. `cargo test` and `stellar contract build` are the gates.
