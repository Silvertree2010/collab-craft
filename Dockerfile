# The CollabCraft service: the API, the sign-in, the socket relay and the static
# hosting for the browser builds, in one binary.
#
# Build context is this repository root:
#
#   docker build -t collabcraft:latest .
#
# The contracts in craft-libs arrive as a pinned git dependency. No sibling
# checkout is required. An earlier layout took a path dependency on a sibling
# tree and this file copied both.
#
# git is installed in the build stage because craft-cloud and craft-ui are git
# dependencies. The rust:1-slim image carries no git binary. Cargo then reports
# the pinned revision as not found. That message names the wrong cause.
#
# pkg-config and libssl-dev are needed because rust_socketio depends on
# native-tls, which builds openssl-sys. That dependency is also why the client
# side cannot target wasm32, and why the browser peer uses the JS
# socket.io-client instead.

FROM rust:1-slim AS build
WORKDIR /src

RUN apt-get update -qq \
 && apt-get install -y -qq --no-install-recommends git ca-certificates pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*

COPY . .

# No Cargo.lock is committed, so --locked fails on a first build. The fallback
# resolves and then builds. The first attempt stays for the case where a lock
# file is present and should be honoured exactly.
RUN cargo build --release --locked 2>/dev/null || cargo build --release

FROM debian:13-slim
# libssl3 for the runtime side of native-tls, ca-certificates so the identity
# provider and the cloud provider can both be verified over https.
RUN apt-get update -qq \
 && apt-get install -y -qq --no-install-recommends libssl3 ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

# Unprivileged. The service needs no ports below 1024 and writes nothing.
RUN useradd --system --no-create-home --uid 10001 collabcraft
COPY --from=build /src/target/release/collabcraft-relay /usr/local/bin/collabcraft
USER collabcraft

# Overridden by compose. Stated so the image runs on its own.
ENV COLLABCRAFT_BIND=0.0.0.0:9444
EXPOSE 9444

# curl rather than a shell builtin: /bin/sh here is dash, which has no
# /dev/tcp, and a healthcheck using it fails every interval while the service
# is perfectly fine.
HEALTHCHECK --interval=60s --timeout=10s --start-period=10s --retries=3 \
    CMD ["curl", "-fsS", "http://127.0.0.1:9444/healthz"]

ENTRYPOINT ["/usr/local/bin/collabcraft"]
