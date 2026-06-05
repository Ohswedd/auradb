# syntax=docker/dockerfile:1

# ---- build stage ----
# Rust 1.90 (>= the workspace MSRV; some dependencies such as `time` require
# rustc >= 1.88). CI builds on stable; this pins a concrete builder for the image.
# Pinned to the bookworm base so the build-stage glibc matches the
# debian:bookworm-slim runtime below; an unpinned `rust:1.90-slim` tracks a newer
# Debian whose glibc the runtime image does not provide.
FROM rust:1.90-slim-bookworm AS build
# The TLS stack (ring) compiles C and assembly, so a C toolchain is required.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p auradb-cli

# ---- runtime stage ----
FROM debian:bookworm-slim AS runtime
RUN useradd --system --create-home --uid 10001 auradb \
    && mkdir -p /data && chown auradb:auradb /data
COPY --from=build /src/target/release/auradb /usr/local/bin/auradb
USER auradb
ENV AURADB_DATA_DIR=/data
VOLUME ["/data"]
EXPOSE 7171
# Liveness: ping the server over the loopback inside the container.
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD ["auradb", "status", "--addr", "127.0.0.1:7171"]
# Default command runs the server. Override it to run any other auradb command,
# for example: docker run --rm ghcr.io/ohswedd/auradb:0.4.1 auradb version
# Bind to all interfaces inside the container; publish the port with -p. AuraDB
# refuses a non-loopback bind with auth disabled, so this development image opts
# in with --allow-insecure-bind: the operator controls exposure with -p, and
# should use docker-compose.secure.yml (auth and TLS enabled) for any deployment.
CMD ["auradb", "server", "--data-dir", "/data", "--bind", "0.0.0.0", "--port", "7171", "--allow-insecure-bind"]
