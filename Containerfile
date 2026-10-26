# Everything builds here; nothing needs to run first.
#
#   podman build -t ghcr.io/dlotterman/iroh-answering-machine:latest .   # the image (last stage)
#   podman build --target check .                                         # fmt, clippy, tests
#   podman build --target binary -o dist .                                # dist/am, a static x86_64 binary
#   podman build --target audit .                                         # supply chain: advisories, licenses, sources,
#                                                                         #   outdated dependencies, the shipped binary
#
# Cache mounts keep the cargo registry and target directory between builds, so rebuilds take seconds.

# A pinned, stable toolchain (ANSSI DENV-STABLE): the version Cargo.lock was resolved with, so builds reproduce.
FROM docker.io/library/rust:1.99.1-bookworm AS toolchain
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl && rustup component add rustfmt clippy
# cargo-auditable (RustSec, from the Rust Secure Code WG) records the dependency tree inside the binary, so the published
# image can be checked for vulnerable crates later with `cargo audit bin`, without this repository.
RUN cargo install --locked cargo-auditable@0.7.7
ENV CARGO_TARGET_DIR=/target

FROM toolchain AS source
WORKDIR /src
COPY Cargo.toml Cargo.lock rustfmt.toml clippy.toml ./
COPY src src
COPY tests tests
# Compiled into the binary with include_str!: the manifest template and the skill.
COPY deploy/datum deploy/datum
COPY skills skills

FROM source AS check
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/target \
    cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked

# musl, so `am` is one static binary that runs on any x86_64 Linux, in or out of a container.
FROM source AS build
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/target \
    cargo auditable build --release --locked --target x86_64-unknown-linux-musl \
    && cp /target/x86_64-unknown-linux-musl/release/am /am
# The runtime image has no shell, so the directories `am` writes to are made here and copied in with their owner.
RUN mkdir -p /rootfs/state /rootfs/var/lib/answering-machine

# Supply-chain checks (ANSSI LIBS-AUDIT, LIBS-OUTDATED; tools from the Rust Secure Code WG and RustSec). Needs the
# network: the advisory database and the registry are fetched fresh.
FROM toolchain AS audit-tools
RUN cargo install --locked cargo-deny@0.20.2 cargo-audit@0.22.2 cargo-outdated@0.19.0

FROM audit-tools AS audit
WORKDIR /src
COPY --from=source /src /src
COPY deny.toml ./
COPY --from=build /am /am
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo deny check --hide-inclusion-graph \
    && cargo outdated --root-deps-only --exit-code 1 \
    && cargo audit --deny warnings --ignore RUSTSEC-2024-0436 bin /am

FROM scratch AS binary
COPY --from=build /am /am

# One image for every role: `serve` (the default, on Datum Compute), and `owner …`, `leave …`, `invites …` on agents'
# machines. The base is Red Hat's hardened image for static binaries: CA certificates, time zone data and a non-root
# user (65532), with no shell, package manager or C library. `am` is static and needs nothing else. Pinned by digest.
FROM registry.access.redhat.com/hi/static@sha256:7e14ec7af494b367c780b040122a685071f73e1f766c28f4390ab9ed2fd4e333
COPY --from=build /am /usr/local/bin/am
COPY --from=build --chown=65532:65532 /rootfs/state /state
COPY --from=build --chown=65532:65532 /rootfs/var/lib/answering-machine /var/lib/answering-machine
USER 65532
ENV AM_STATE=/state
ENTRYPOINT ["/usr/local/bin/am"]
CMD ["serve"]
