# SEP-58 Notes — Contract Build Reproducibility Metadata

> Source: https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0058.md
> Read: Day0. Status of spec: Draft. These are the fields our verifier consumes.

> ⚠️ **Correction vs. the project brief:** the brief referenced `source_repo`, `source_rev`,
> `tarball_url`, `tarball_sha256`. The current spec uses **`bldimg`, `bldopt`, `source_uri`,
> `source_sha256`** instead. We follow the spec's field names.

## Metadata fields

| Key | Meaning | Example | Required |
|---|---|---|---|
| `bldimg` | Fully-qualified container image used for the build, **pinned by digest** (single-arch manifest) | `docker.io/stellar/stellar-cli:26.0.0@sha256:8b45455a…` | **Yes** |
| `bldopt` | A single shell-style flag passed verbatim as one arg to the build command; **repeatable** | `--optimize`, `--manifest-path=contracts/foo/Cargo.toml`, `--features=foo,bar` | No |
| `source_sha256` | SHA-256 of the source archive's bytes | `e3b0c442…b855` | **Yes** |
| `source_uri` | URI to download the source archive (https:// expected) | `https://example.com/my-contract-v1.0.0.tar.gz` | No |

## Where the metadata lives (storage venues)

- **Primary:** contract's `contractmetav0` custom section (per SEP-46).
- **Alternative:** on-chain registry contract mapping contract ID / wasm hash → fields.
- **Fallback:** off-chain DB / verification service / community registry.
- Verifiers may combine venues; fetch fields from wherever available.
  - → This directly justifies our **retroactive registry** (M2): pre-SEP-58 contracts have no
    `contractmetav0`, so the fields come from our off-chain registry instead.

## Verification / reproduction process (the algorithm our service implements)

1. **Retrieve metadata** from the deployed wasm (`contractmetav0`) or a registry.
2. **Obtain source archive** via `source_uri` download or content-addressable lookup by `source_sha256`.
3. **Verify archive integrity**: sha256(archive bytes) == `source_sha256`.
4. **Extract archive** — must contain a single top-level directory.
5. **Resolve toolchain**: read `RUSTUP_TOOLCHAIN` from the image so the toolchain can't switch mid-build.
6. **Reproducible build** using the pinned `bldimg` digest + all `bldopt` flags, e.g.:
   ```
   docker run --rm -v "$PWD:/source" -e RUSTUP_TOOLCHAIN <bldimg> \
     contract build <bldopt entries> ...
   ```
7. **Compare hashes**: sha256(rebuilt wasm) == sha256(on-chain wasm) → verified / mismatch.

## Image trust & reproducibility (feeds our multi-dimensional trust model — brief §5)

- **Allowlist:** verifiers SHOULD restrict `bldimg` to an allowlist of independently vetted images,
  not trust arbitrary digests. → our MVP response schema carries a `trust_level` field
  (`arbitrary` / `publicly-auditable` / `sdf-maintained`).
- **Trustworthy image qualities:** CI provenance/SLSA attestations, SBOM, reproducible image builds,
  active security maintenance, stable release cadence, Rust tier-1 host, pre-installed `wasm32v1-none`.
- **Why digest pinning:** content digest prevents tag mutation and transitively pins the whole build
  stack (incl. Rust toolchain). **Single-arch manifest is mandatory** for deterministic resolution
  across verifier architectures.

## Implications for our data model

A verification request/record needs at least:
`contract_id`, `wasm_hash` (on-chain), `bldimg`, `bldopt[]`, `source_uri?`, `source_sha256`,
plus computed: `rebuilt_wasm_sha256`, `result` (verified|mismatch|pending|error), `trust_level`, `timestamp`.
