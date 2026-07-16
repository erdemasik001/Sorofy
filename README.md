# Soroban Contract Verification Service

An open-source, multi-verifier source verification service that proves a Soroban smart contract's on-chain WASM bytes were built from the public source code shown on explorers.

> Status: hackathon MVP in progress. See [PLAN.md](PLAN.md) for the day-by-day build plan and [idea1-project-brief.md](idea1-project-brief.md) for the full project brief.

## Problem

On Stellar/Soroban, a deployed contract is opaque bytes — there's no programmatic way to confirm that the source code shown on an explorer actually compiles to those bytes. SEP-55 (CI attestation) proves provenance, not source-to-bytecode correspondence.

## Solution

Rebuild the contract from source in a deterministic, isolated environment (digest-pinned Docker image), byte-compare the resulting WASM's sha256 against the on-chain hash, and serve the result through a free public API.

```
Developer / Explorer
        │  source (tarball|git) + target WASM hash
        ▼
┌─────────────────────────────┐
│  Verification Service        │
│  1. read SEP-58 meta         │
│  2. bldimg (pinned Docker)   │
│  3. rebuild in sandbox       │
│  4. compare sha256           │
└─────────────────────────────┘
        │  result (verified / mismatch) + source link
        ▼
   Cache + Public API  ◄── Explorer / Wallet / Lab / stellar-cli
```

## MVP scope

- Single verification flow: source (git repo/commit) + target WASM hash → deterministic Docker rebuild → sha256 compare
- REST API: `POST /verify`, `GET /verify/{contract_id|wasm_hash}`
- Testnet only, simple result cache
- Multi-verifier decentralization and retroactive verification are architected for but out of scope for the MVP — see [PLAN.md](PLAN.md).

## Stack

Rust, Axum, Docker, Soroban RPC, `stellar-cli`.

## License

MIT
