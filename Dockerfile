# syntax=docker/dockerfile:1

# ---- build stage ----
FROM rust:1.85-slim AS build
WORKDIR /src
# Cache dependencies first.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p auradb-cli

# ---- runtime stage ----
FROM debian:bookworm-slim AS runtime
RUN useradd --system --create-home --uid 10001 auradb
COPY --from=build /src/target/release/auradb /usr/local/bin/auradb
USER auradb
ENV AURADB_DATA_DIR=/data
VOLUME ["/data"]
EXPOSE 7171
# Default command runs the server. Override it to run any other auradb command,
# for example: docker run --rm auradb:local auradb version
# Bind to all interfaces inside the container; publish the port with -p.
CMD ["auradb", "server", "--data-dir", "/data", "--bind", "0.0.0.0", "--port", "7171"]
