# Stage 1: Build
FROM rust:1.93-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    libprotobuf-dev \
    libfontconfig1-dev \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies by copying only manifest files first
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release 2>/dev/null || true

# Copy the real source code, migrations, and vendored proto definitions
COPY src ./src
COPY migrations ./migrations
COPY vendor/wowsims-mop/proto ./vendor/wowsims-mop/proto
COPY vendor/wowsims-mop/ui ./vendor/wowsims-mop/ui

# Build the actual binary (touches main.rs to force rebuild)
RUN touch src/main.rs && cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim AS runtime

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    fontconfig \
    fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*

# Copy the compiled binary and migrations
COPY --from=builder /app/target/release/high-quality-bot ./high-quality-bot
COPY --from=builder /app/migrations ./migrations
COPY --from=builder /app/vendor/wowsims-mop/ui ./vendor/wowsims-mop/ui

# Create a non-root user
RUN useradd -ms /bin/bash botuser
RUN chmod -R a+rX /app/vendor/wowsims-mop/ui && chown -R botuser:botuser /app
USER botuser

ENTRYPOINT ["./high-quality-bot"]
