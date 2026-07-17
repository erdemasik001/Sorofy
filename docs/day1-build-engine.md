# Day1 — Deterministic Build Engine

Goal: turn Day0's manual reproduction into an automated engine — source in, WASM
hash out, compared against the on-chain hash. Carried forward from
[day0-reproduction-findings.md](day0-reproduction-findings.md).

## Result

`verify-core` reproduces the Day0 contract **byte-identically inside the pinned
container**, independently confirming Day0's cross-environment finding:

| Run | Source | Rebuilt hash | Verdict |
|---|---|---|---|
| Day0 manual (Windows, native) | hello-world scaffold | `b68602842d3a1d169d54fe3e57c0511a774df4710553d6d4d22e653d62bf5f5b` | baseline |
| Day0 manual (WSL2, native) | same | `b68602…` | match |
| **Day1 automated (container)** | [fixture](https://github.com/erdemasik001/stellar-verify-fixture-hello-world) at `c08333e` | **`b68602…`** (660 B) | **verified** |

The engine also discriminates, which is the part that makes it worth anything:

| Case | Expected | Rebuilt | Verdict | Exit |
|---|---|---|---|---|
| Correct source + correct hash | `b68602…` | `b68602…` | `verified` | 0 |
| Correct source, wrong expected hash | `000000…` | `b68602…` | `mismatch` | 1 |
| **Tampered source** (`Hello`→`Howdy`, one word) | `b68602…` | `2f8a8fff…` | `mismatch` | 1 |
| Original rev, tampered working tree | `b68602…` | `b68602…` | `verified` | 0 |

The last row matters: it builds the **commit**, not the working tree, and it
returns to the same hash after a detour — determinism, not just sensitivity.
`source_sha256` was byte-identical across runs too, so the source digest is
itself reproducible.

## Architecture: two phases, one CARGO_HOME

The build must not touch the network, but a contract's dependencies live on
crates.io. Those requirements are irreconcilable in a single container, so a job
is split across two that share a `CARGO_HOME` volume:

1. **fetch** — `cargo fetch --locked`, **network on**. Downloads exactly the
   versions `Cargo.lock` names. Runs none of their code.
2. **build** — `stellar contract build`, **`--network=none`**. Resolves offline
   from what phase 1 left behind.

The split is what makes isolation achievable at all: compiling runs untrusted
code (`build.rs`, proc macros), and *that* is the phase that must not reach the
network. Fetching is not. Verified empirically: the build phase cannot reach
crates.io; the fetch phase can.

`stellar contract build` has no `--locked` of its own. It does not need one —
phase 1 fails on a missing or stale `Cargo.lock`, and phase 2 can only resolve
from what phase 1 fetched. Lockfile adherence is enforced from both sides.

## Sandbox properties

- **No bind mounts.** Source enters and artifacts leave over tar-on-stdin
  (`docker cp -`). No host path is exposed to the container, and the same code
  works whether the daemon is local (Linux deploy) or reached via
  `wsl -- docker` (Windows dev box), where a Windows path would be meaningless.
- **Non-root build** (`builder`, uid 1000). Source tars are rewritten to that
  uid before staging, so the build can write `target/` without the container
  running as root. This changes no file content and cannot affect the WASM.
- **Network off** during the phase that executes submitted code.
- **Wall-clock timeout**, container killed on expiry.
- **Path traversal rejected** in source archives before `docker cp` unpacks them.
- Containers and volumes are removed on drop, including on failure.

## Friction hit (and what it teaches)

| Problem | Cause | Fix |
|---|---|---|
| `docker-credential-desktop.exe: exec format error` | Docker Desktop leftovers in `~/.docker/config.json` inside WSL; WSL cannot exec a Windows helper | `credsStore` removed (backed up). Only public images are pulled, anonymously |
| `stellar: libdbus-1.so.3 not found` | stellar-cli links dbus (keyring) even for `contract build`; the slim base lacks it | `libdbus-1-3` in the image |
| `cargo fetch` → `Permission denied` on `/cargo-home` | Docker seeds a named volume from the image's content **and ownership**; unset, it lands root-owned | `mkdir /cargo-home && chown builder` in the image, so the volume inherits it |
| Build → `Permission denied` on `/build/source/target` | `git archive` hardcodes uid 0 in its tar; `docker cp` restores that | Rewrite tar ownership to uid 1000 before staging |
| Offline build → `failed to download ahash` | `cargo fetch --target wasm32v1-none` skips host deps (proc macros, build scripts) | Fetch without `--target` (the superset the build needs) |
| "build produced 2 .wasm artifacts" | Cargo copies the artifact into `deps/` as an intermediate | Only root-level `.wasm` in the profile dir counts |

**`--remap-path-prefix` (flagged as a Day0 risk) is a non-issue**: stellar-cli
passes `--remap-path-prefix=$CARGO_HOME/registry/src=` itself, so registry paths
never reach the WASM. That is *why* the container's `CARGO_HOME` can be
`/cargo-home` and still match a host build that used `~/.cargo`.

## Not yet true (carried into Day2/Day3)

- **`bldimg` is not digest-pinned in practice.** The image is local, so it has no
  registry digest and validation runs with `--allow-unpinned-image`. SEP-58
  requires a digest; the check is implemented and enforced by default, but only
  becomes real once the image is pushed (Day3).
- **`trust_level` is hardcoded `arbitrary`.** The field is wired end-to-end so
  the API schema will not change; the allowlist is post-MVP.
- **Only tested on a trivial hello-world.** Day0's caveat stands: larger
  contracts can surface non-determinism this scaffold cannot.
- **The real testnet target `CDZZZTN6…` is still unreproducible** — no
  `source_uri` in its metadata, and its toolchain (rustc 1.95.0 / sdk 22.0.11 /
  cli 25.2.0) differs from this image's, so it needs both a source pointer and a
  second `bldimg`. This is the retroactive-registry (M2) path.
- ~~`source_uri` archive path is implemented but has not been exercised
  end-to-end against a real published tarball.~~ **Closed in Day2** — exercised
  against a real GitHub tarball (which surfaced and fixed a `pax_global_header`
  bug); see [day2-api.md](day2-api.md), "SEP-58 archive (`source_uri`) path".

## Reproducing this

The source built above is published as a standalone fixture so anyone can rerun
this end to end:
[`erdemasik001/stellar-verify-fixture-hello-world`](https://github.com/erdemasik001/stellar-verify-fixture-hello-world)
at commit `c08333e9924bfb45ee221f3edeb8ded4d4840397`.

```bash
# Build the image (Linux/WSL2; Docker Engine, not Docker Desktop)
docker build --platform linux/amd64 \
  -t sorofy/build-image:rust1.91.1-cli23.2.1 docker/build-image

# Reproduce the Day0 contract from the published fixture
cargo run -p verifier-core --bin verify-core -- \
  --repo https://github.com/erdemasik001/stellar-verify-fixture-hello-world \
  --rev c08333e9924bfb45ee221f3edeb8ded4d4840397 \
  --bldimg sorofy/build-image:rust1.91.1-cli23.2.1 --allow-unpinned-image \
  --wasm-hash b68602842d3a1d169d54fe3e57c0511a774df4710553d6d4d22e653d62bf5f5b
```

This prints `VERIFIED` with `release/hello_world.wasm (660 bytes)` and exits `0`.

The same four rows from the discrimination table above run as automated tests
against this fixture:

```bash
cargo test -p verifier-core -- --ignored
```

Exit codes: `0` verified, `1` mismatch, `2` error. On Windows the CLI shells into
WSL2 automatically; override with `VERIFY_DOCKER="wsl -d Ubuntu -- docker"`.
