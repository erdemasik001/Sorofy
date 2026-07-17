# Runtime image for the sorofy-api service.
#
# The service does NOT compile contracts itself — it shells out to `docker` to
# run the pinned build image (docker/build-image) as a *sibling* container on
# the host daemon (Docker-out-of-Docker). So this image ships the `docker` CLI,
# not a full engine, and the host's Docker socket is mounted in at runtime:
#
#   docker run -v /var/run/docker.sock:/var/run/docker.sock ...
#
# That is deliberately simpler (and less privileged) than Docker-in-Docker: the
# API just needs to *talk to* a daemon, not be one. On a VPS that daemon is the
# host's; on Fly.io a dockerd is started inside the machine (see docs/day3).
#
# git + gzip are here because verifier-core drives them as subprocesses to fetch
# and stage `repo`/`source_uri` sources (crates/verifier-core/src/source.rs).

# ---- build stage ---------------------------------------------------------
FROM rust:1.91.1-bookworm AS build
WORKDIR /src
# Copy the whole workspace; rusqlite is `bundled` so it needs the C toolchain
# already present in the rust image.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p api --bin sorofy-api

# ---- runtime stage -------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates git gzip docker.io \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/sorofy-api /usr/local/bin/sorofy-api

# The service binds this inside the container; the platform maps a public port.
ENV SOROFY_BIND=0.0.0.0:8080
# Persisted cache lives on a mounted volume in production.
ENV SOROFY_DB=/data/sorofy.db
EXPOSE 8080

ENTRYPOINT ["sorofy-api"]
