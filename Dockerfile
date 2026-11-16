# The faderz image: the binary with the web UI built into it, and the
# licences of everything shipped: the Rust crates (THIRD-PARTY.txt) and
# the npm packages and fonts of the UI (THIRD-PARTY-WEB.txt).
FROM node:22.22.3-trixie-slim@sha256:8cd0ffd483b64585c6d135364bea5f937ff40cd3da431789af011f9ee8d55af0 AS web

WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY api /src/api
COPY web ./
RUN npm run build && node build/check-licenses.mjs

FROM ghcr.io/danielclasen/samplez-builder@sha256:92ece4452be98fff177b49fb201c8c693130c08845c42725dd5b1173d4863fba AS build

SHELL ["/bin/bash", "-euo", "pipefail", "-c"]

ARG CARGO_ABOUT_VERSION=0.9.3
# Caps cargo's parallel jobs when set.
ARG CARGO_BUILD_JOBS

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install --locked --root /opt/cargo-about --features cli \
      ${CARGO_BUILD_JOBS:+--jobs "$CARGO_BUILD_JOBS"} \
      "cargo-about@${CARGO_ABOUT_VERSION}" \
 && /opt/cargo-about/bin/cargo-about --version

WORKDIR /src
COPY . .
COPY --from=web /src/web/dist web/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin faderz \
      ${CARGO_BUILD_JOBS:+--jobs "$CARGO_BUILD_JOBS"} \
 && install -Dm0755 target/release/faderz /out/usr/local/bin/faderz \
 && install -Dm0644 LICENSE /out/usr/share/licenses/faderz/LICENSE \
 && /opt/cargo-about/bin/cargo-about generate --locked --fail \
      -o /out/usr/share/licenses/faderz/THIRD-PARTY.txt about.hbs \
 && install -Dm0644 web/dist/THIRD-PARTY-WEB.txt /out/usr/share/licenses/faderz/THIRD-PARTY-WEB.txt

# The binary, its licence and the licences of what is built into it,
# attached to a release.
FROM scratch AS dist
COPY --from=build /out/usr/local/bin/faderz /out/usr/share/licenses/faderz/LICENSE \
     /out/usr/share/licenses/faderz/THIRD-PARTY.txt \
     /out/usr/share/licenses/faderz/THIRD-PARTY-WEB.txt /

# The slim variant of the Debian release the builder and foldz use: the
# binary needs glibc and libgcc_s, and the libmxl it loads libstdc++6,
# which the image ships for apt.
FROM debian:trixie-20261027-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4

SHELL ["/bin/bash", "-euo", "pipefail", "-c"]

COPY --from=build /opt/mxl/lib/libmxl.so.1.1 /opt/mxl/lib/libmxl.so.1.1
RUN ln -s libmxl.so.1.1 /opt/mxl/lib/libmxl.so.1 \
 && ln -s libmxl.so.1 /opt/mxl/lib/libmxl.so
COPY --from=build /opt/mxl/share/licenses /opt/mxl/share/licenses
COPY --from=build /out/ /
# The replicated state's default data directory; a deployment mounts a
# volume there.
RUN install -d -o 65532 -g 65532 /var/lib/faderz

# 65532 owns nothing in the image but the data directory.
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/faderz"]
CMD ["--config", "/etc/faderz/config.json"]
