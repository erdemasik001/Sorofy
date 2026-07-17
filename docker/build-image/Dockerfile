# Deterministic build image for Soroban contract verification (SEP-58 `bldimg`).
#
# Every version here is pinned on purpose: the WASM a contract compiles to is a
# function of this toolchain, so anything unpinned is a way for two verifiers to
# disagree. Day0 established (docs/day0-reproduction-findings.md) that with
# rustc + stellar-cli pinned, the host OS does not affect the output — this image
# is what freezes them.
#
# Versions match the Day0 manual reproduction so the containerized build has to
# land on the same hash: b68602842d3a1d169d54fe3e57c0511a774df4710553d6d4d22e653d62bf5f5b
#
# Built and published single-arch (linux/amd64): SEP-58 requires bldimg to
# resolve to a single-arch manifest, or the same digest could mean different
# bytes on different verifiers.

FROM docker.io/library/debian:bookworm-20250520-slim

ARG RUST_VERSION=1.91.1
ARG STELLAR_CLI_VERSION=23.2.1
ARG WASM_TARGET=wasm32v1-none

# Baked in so the toolchain cannot switch mid-build (SEP-58 verification step 5).
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION}
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# libdbus-1-3: the stellar-cli binary links against it (keyring support) and
# will not start without it, even for `contract build`.
# git: cargo needs it to resolve git-sourced dependencies during the fetch phase.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl gcc libc6-dev libdbus-1-3 git \
    && rm -rf /var/lib/apt/lists/*

# Rust, pinned to an exact version with the wasm target preinstalled.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal \
        --default-toolchain ${RUST_VERSION} --target ${WASM_TARGET} \
    && chmod -R a+w ${RUSTUP_HOME} ${CARGO_HOME}

# stellar-cli, as the prebuilt release binary for this exact version.
RUN curl -fsSL -o /tmp/stellar.tar.gz \
      "https://github.com/stellar/stellar-cli/releases/download/v${STELLAR_CLI_VERSION}/stellar-cli-${STELLAR_CLI_VERSION}-x86_64-unknown-linux-gnu.tar.gz" \
    && tar -xzf /tmp/stellar.tar.gz -C /usr/local/bin stellar \
    && rm /tmp/stellar.tar.gz \
    && chmod +x /usr/local/bin/stellar

# verifier-core runs two phases against this image: `cargo fetch` with network
# on, then `stellar contract build` with --network=none, sharing a CARGO_HOME
# volume. It sets CARGO_HOME and CARGO_NET_OFFLINE per phase.

# Builds run as a non-root user: the source we compile is untrusted.
RUN useradd --create-home --uid 1000 builder

# Create the CARGO_HOME mountpoint owned by builder. Docker seeds a fresh named
# volume from the image's content *and ownership* at the mount path, so this is
# what makes the shared volume writable by a non-root build. Without it the
# volume lands root-owned and cargo cannot populate it.
RUN mkdir -p /cargo-home /build && chown builder:builder /cargo-home /build

USER builder

# verifier-core stages the source tree under /build and sets --workdir itself.
WORKDIR /build

# Matches SEP-58's `docker run <bldimg> contract build ...` invocation shape.
ENTRYPOINT ["stellar"]
CMD ["contract", "build"]
