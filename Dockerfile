# The CollabCraft service: the API, the sign-in, the socket relay and the static
# hosting for the browser builds, in one binary.
#
# Build context is the directory holding both trees, because the service takes
# path dependencies on craft-libs. The wire protocol and the provider contracts
# live there so every Craft app speaks the same ones.
#
#   docker build -f collabcraft/Dockerfile -t collabcraft:latest .
#
# pkg-config and libssl-dev are needed because rust_socketio depends on
# native-tls, which builds openssl-sys. That dependency is also why the client
# side cannot target wasm32, and why the browser peer uses the JS
# socket.io-client instead.

FROM rust:1-slim AS build
WORKDIR /src

RUN apt-get update -qq \
 && apt-get install -y -qq --no-install-recommends pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*

# Both trees, because the manifests reference each other by path and cargo
# resolves them before it reads a single source file.
COPY craft-libs ./craft-libs
COPY collabcraft ./collabcraft

WORKDIR /src/collabcraft
RUN cargo build --release --locked 2>/dev/null || cargo build --release

FROM debian:13-slim
# libssl3 for the runtime side of native-tls, ca-certificates so the identity
# provider and the cloud provider can both be verified over https.
RUN apt-get update -qq \
 && apt-get install -y -qq --no-install-recommends libssl3 ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

# Unprivileged. The service needs no ports below 1024 and writes nothing.
RUN useradd --system --no-create-home --uid 10001 collabcraft
COPY --from=build /src/collabcraft/target/release/collabcraft-relay /usr/local/bin/collabcraft
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
