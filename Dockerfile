# Stage 1: Build
FROM rust:1.93-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies by copying only manifest files first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release 2>/dev/null || true

# Copy the real source code and migrations
COPY src ./src
COPY migrations ./migrations

# Build the actual binary (touches main.rs to force rebuild)
RUN touch src/main.rs && cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim AS runtime

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Copy the compiled binary and migrations
COPY --from=builder /app/target/release/high-quality-bot ./high-quality-bot
COPY --from=builder /app/migrations ./migrations

# Create a non-root user
RUN useradd -ms /bin/bash botuser
USER botuser

ENTRYPOINT ["./high-quality-bot"]
