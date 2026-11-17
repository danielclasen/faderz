# The faderz-bridge image: the binary and the licences of the Rust crates
# built into it (THIRD-PARTY.txt). Built from the repository root with
# -f bridge/Dockerfile; bridge/Dockerfile.dockerignore keeps the context
# to the bridge and the licence.

# cargo-about in a stage of its own, so that it builds while the bridge
# does.
FROM ghcr.io/danielclasen/samplez-builder@sha256:92ece4452be98fff177b49fb201c8c693130c08845c42725dd5b1173d4863fba AS about

SHELL ["/bin/bash", "-euo", "pipefail", "-c"]

ARG CARGO_ABOUT_VERSION=0.9.3
# Caps cargo's parallel jobs when set.
ARG CARGO_BUILD_JOBS

RUN --mount=type=cache,id=cargo-about-registry,target=/usr/local/cargo/registry \
    cargo install --locked --root /opt/cargo-about --features cli \
      ${CARGO_BUILD_JOBS:+--jobs "$CARGO_BUILD_JOBS"} \
      "cargo-about@${CARGO_ABOUT_VERSION}" \
 && /opt/cargo-about/bin/cargo-about --version

FROM ghcr.io/danielclasen/samplez-builder@sha256:92ece4452be98fff177b49fb201c8c693130c08845c42725dd5b1173d4863fba AS build

SHELL ["/bin/bash", "-euo", "pipefail", "-c"]

# Caps cargo's parallel jobs when set.
ARG CARGO_BUILD_JOBS

WORKDIR /src/bridge
COPY LICENSE /src/LICENSE
COPY bridge ./
RUN --mount=type=cache,id=bridge-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=bridge-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=bridge-target,target=/src/bridge/target \
    cargo build --release --locked --bin faderz-bridge \
      ${CARGO_BUILD_JOBS:+--jobs "$CARGO_BUILD_JOBS"} \
 && install -Dm0755 target/release/faderz-bridge /out/usr/local/bin/faderz-bridge
COPY --from=about /opt/cargo-about /opt/cargo-about
RUN --mount=type=cache,id=bridge-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=bridge-git,target=/usr/local/cargo/git \
    install -Dm0644 /src/LICENSE /out/usr/share/licenses/faderz-bridge/LICENSE \
 && /opt/cargo-about/bin/cargo-about generate --locked --fail \
      -o /out/usr/share/licenses/faderz-bridge/THIRD-PARTY.txt about.hbs

# The binary, its licence and the licences of what is built into it,
# attached to a release.
FROM scratch AS dist
COPY --from=build /out/usr/local/bin/faderz-bridge \
     /out/usr/share/licenses/faderz-bridge/LICENSE \
     /out/usr/share/licenses/faderz-bridge/THIRD-PARTY.txt /

FROM debian:trixie-20261027-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4

SHELL ["/bin/bash", "-euo", "pipefail", "-c"]

# The OIDC provider and faderz are reached over https, verified against
# the system's CA certificates.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=build /out/ /

USER 65532:65532
ENTRYPOINT ["/usr/local/bin/faderz-bridge"]
