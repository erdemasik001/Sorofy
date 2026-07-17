#!/usr/bin/env bash
# Publish the digest-pinned build image to GHCR so `bldimg` resolves to a real
# registry digest (SEP-58 requires bldimg to be a single-arch manifest digest,
# not a mutable tag). Run from a shell that can reach the Docker daemon holding
# the locally-built image — on Windows that is WSL2 (`wsl -d Ubuntu -- bash`).
#
# Prereqs (one-time, interactive, done by a human — this script does not do them):
#   1. gh auth refresh -h github.com -s write:packages   # add package push scope
#   2. the image built locally:
#      docker build --platform linux/amd64 \
#        -t sorofy/build-image:rust1.91.1-cli23.2.1 docker/build-image
#
# After a successful push, make the package public once (GHCR packages default
# to private) at:
#   https://github.com/users/erdemasik001/packages/container/sorofy-build-image/settings
# so any verifier can pull the bldimg by digest.
set -euo pipefail

OWNER="erdemasik001"
LOCAL="sorofy/build-image:rust1.91.1-cli23.2.1"
REMOTE="ghcr.io/${OWNER}/sorofy-build-image:rust1.91.1-cli23.2.1"

# Fail early if the token lacks write:packages rather than at push time.
if ! gh auth token >/dev/null 2>&1; then
  echo "error: not logged in with gh. Run: gh auth login" >&2
  exit 1
fi

echo ">> logging the Docker daemon into ghcr.io as ${OWNER}"
gh auth token | docker login ghcr.io -u "${OWNER}" --password-stdin

echo ">> tagging ${LOCAL} -> ${REMOTE}"
docker tag "${LOCAL}" "${REMOTE}"

echo ">> pushing (1.5 GB, first push is slow)"
docker push "${REMOTE}"

echo ">> resolved digest:"
docker inspect --format '{{index .RepoDigests 0}}' "${REMOTE}"
