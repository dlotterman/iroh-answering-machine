#!/bin/sh
# For development only: run any cargo command in the Containerfile's toolchain stage, against this checkout, e.g.
# `scripts/cargo.sh fmt` (rewrites files) or `scripts/cargo.sh test -- round_trip`. Builds go through `podman build`.
set -eu
cd "$(dirname "$0")/.."
podman build -q --http-proxy=false --target toolchain -t localhost/am-toolchain . >/dev/null
podman run --rm --http-proxy=false -v "$PWD":/src:z -v am-cargo:/usr/local/cargo/registry -v am-target:/target \
  -w /src localhost/am-toolchain cargo "$@"
