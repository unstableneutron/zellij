# Dockerfile for ZRP E2E testing
# Uses multi-stage build to create both server and client images

FROM rust:1.83-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    libprotobuf-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy workspace
COPY . .

# Build with remote feature enabled
RUN cargo build --release --features remote

# Build spike_client
RUN cargo build --release --example spike_client -p zellij-remote-bridge

# Build spike_server (for standalone testing)
RUN cargo build --release --example spike_server -p zellij-remote-bridge

#-----------------------------------------------------------
# Server image - runs Zellij with remote enabled
#-----------------------------------------------------------
FROM debian:bookworm-slim AS zellij-server

# Install runtime dependencies and network tools
RUN apt-get update && apt-get install -y \
    iproute2 \
    iptables \
    procps \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/zellij /usr/local/bin/zellij

# Create non-root user for running zellij
RUN useradd -m -s /bin/bash zellij
USER zellij
WORKDIR /home/zellij

# Default environment
ENV ZELLIJ_REMOTE_ADDR=0.0.0.0:4433
ENV TERM=xterm-256color

EXPOSE 4433/udp

CMD ["zellij"]

#-----------------------------------------------------------
# Client image - runs spike_client for testing
#-----------------------------------------------------------
FROM debian:bookworm-slim AS spike-client

# Install runtime dependencies and network emulation tools
RUN apt-get update && apt-get install -y \
    iproute2 \
    iptables \
    procps \
    curl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/examples/spike_client /usr/local/bin/spike_client

# Create non-root user
RUN useradd -m -s /bin/bash tester
USER tester
WORKDIR /home/tester

ENV HEADLESS=1

CMD ["spike_client"]

#-----------------------------------------------------------
# Network emulation sidecar - runs tc netem with cleanup guarantee
#-----------------------------------------------------------
FROM debian:bookworm-slim AS netem-sidecar

RUN apt-get update && apt-get install -y \
    iproute2 \
    iptables \
    procps \
    && rm -rf /var/lib/apt/lists/*

# Copy the cleanup-guaranteed network emulation script
COPY zellij-remote-tests/scripts/netem-wrapper.sh /usr/local/bin/netem-wrapper.sh
RUN chmod +x /usr/local/bin/netem-wrapper.sh

# Run as root for tc commands
WORKDIR /root

ENTRYPOINT ["/usr/local/bin/netem-wrapper.sh"]
