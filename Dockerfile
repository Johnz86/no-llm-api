# Offline by design: the default feature set links no HTTP client, so this image
# cannot reach a paid API even if credentials leak into its environment.
FROM rust:1.90-slim AS builder
WORKDIR /build

# Dependency-only layer: sources change far more often than the manifest.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN mkdir -p src/bin \
    && echo 'fn main() {}' > src/main.rs \
    && echo 'fn main() {}' > src/bin/regenerate_dataset.rs \
    && echo '' > src/lib.rs \
    && cargo build --release --locked --bin no-llm-api \
    && rm -rf src

COPY src ./src
COPY scenarios ./scenarios
COPY fixtures ./fixtures
COPY index.html ./index.html
# Touch so cargo rebuilds the real sources rather than reusing the stub.
RUN touch src/main.rs src/lib.rs \
    && cargo build --release --locked --bin no-llm-api --bin regenerate_dataset

# Bake the sample dataset so the container needs no writable volume.
RUN ./target/release/regenerate_dataset --output /build/data/conversations.parquet --force

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=builder /build/target/release/no-llm-api /app/no-llm-api
COPY --from=builder /build/data/conversations.parquet /app/data/conversations.parquet
COPY --from=builder /build/scenarios /app/scenarios

# Bind to all interfaces inside the container; the control plane therefore stays
# off unless it is switched on deliberately, which is decision D1.
ENV BIND_ADDRESS=0.0.0.0:8080 \
    DATASET_PATH=/app/data/conversations.parquet \
    RUST_LOG=info

EXPOSE 8080
USER nonroot:nonroot
HEALTHCHECK --interval=10s --timeout=3s --start-period=2s --retries=3 \
    CMD ["/app/no-llm-api", "health", "--url", "http://127.0.0.1:8080/ready"]
ENTRYPOINT ["/app/no-llm-api"]
