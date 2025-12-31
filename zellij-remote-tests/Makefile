# Makefile for ZRP E2E testing
# Provides both local and Docker-based testing options

.PHONY: help build test-local test-docker clean

SHELL := /bin/bash
PROJECT_ROOT := $(shell dirname $(CURDIR))

# Default token for testing
export ZELLIJ_REMOTE_TOKEN ?= test-token-12345

help:
	@echo "ZRP E2E Testing"
	@echo ""
	@echo "Local Testing (no Docker required):"
	@echo "  make build              - Build Zellij with remote feature"
	@echo "  make test-local         - Run local connectivity test"
	@echo "  make test-local-auth    - Test authentication"
	@echo "  make server             - Start Zellij server (background)"
	@echo "  make client             - Run spike_client"
	@echo "  make client-headless    - Run spike_client headless"
	@echo ""
	@echo "Docker Testing:"
	@echo "  make docker-build       - Build Docker images"
	@echo "  make test-docker        - Run Docker-based test"
	@echo "  make test-docker-latency PROFILE=high-rtt"
	@echo ""
	@echo "Cleanup:"
	@echo "  make clean              - Stop all test processes"
	@echo "  make docker-clean       - Remove Docker containers"

#------------------------------------------------------------------------------
# Local Testing (fastest iteration)
#------------------------------------------------------------------------------

build:
	cd $(PROJECT_ROOT) && cargo build --features remote --release
	cd $(PROJECT_ROOT) && cargo build --example spike_client --example spike_server -p zellij-remote-bridge --release

# Start server in background, save PID
server: build
	@echo "Starting Zellij server on 127.0.0.1:4433..."
	@pkill -f "zellij.*ZELLIJ_REMOTE_ADDR" 2>/dev/null || true
	@cd $(PROJECT_ROOT) && \
		ZELLIJ_REMOTE_ADDR=127.0.0.1:4433 \
		ZELLIJ_REMOTE_TOKEN=$(ZELLIJ_REMOTE_TOKEN) \
		RUST_LOG=info \
		./target/release/zellij &
	@echo "Server started. PID: $$!"
	@sleep 2
	@echo "Server should be ready. Run 'make client' to connect."

# Run interactive client
client: build
	@echo "Connecting to server..."
	cd $(PROJECT_ROOT) && \
		SERVER_URL=https://127.0.0.1:4433 \
		ZELLIJ_REMOTE_TOKEN=$(ZELLIJ_REMOTE_TOKEN) \
		./target/release/examples/spike_client

# Run headless client (for automated tests)
client-headless: build
	@echo "Running headless client..."
	cd $(PROJECT_ROOT) && \
		SERVER_URL=https://127.0.0.1:4433 \
		ZELLIJ_REMOTE_TOKEN=$(ZELLIJ_REMOTE_TOKEN) \
		HEADLESS=1 \
		timeout 10 ./target/release/examples/spike_client || true

# Full local test - uses a script with unique session names to avoid killing user sessions
test-local: build
	@./scripts/test-basic.sh $(PROJECT_ROOT)

# Test authentication - try without token
test-local-auth: build
	@./scripts/test-auth.sh $(PROJECT_ROOT)

# Test reconnection and metrics
test-local-reconnect: build
	@./scripts/test-reconnect.sh $(PROJECT_ROOT)

# Test datagrams and 0-RTT
test-local-datagram: build
	@./scripts/test-datagrams-0rtt.sh $(PROJECT_ROOT)

# Run all local tests
test-all: build
	@echo "=== Running all E2E tests ==="
	@./scripts/test-basic.sh $(PROJECT_ROOT)
	@./scripts/test-auth.sh $(PROJECT_ROOT)
	@./scripts/test-reconnect.sh $(PROJECT_ROOT)
	@./scripts/test-datagrams-0rtt.sh $(PROJECT_ROOT)
	@echo "=== All tests passed ==="

#------------------------------------------------------------------------------
# Docker Testing (for network emulation)
#------------------------------------------------------------------------------

docker-build:
	cd $(PROJECT_ROOT) && docker compose -f zellij-remote-tests/docker-compose.yml build

test-docker:
	cd $(PROJECT_ROOT) && ./zellij-remote-tests/run-tests.sh

test-docker-latency:
	cd $(PROJECT_ROOT) && ./zellij-remote-tests/run-tests.sh --profile $(PROFILE)

#------------------------------------------------------------------------------
# Cleanup
#------------------------------------------------------------------------------

clean:
	@echo "Stopping ZRP test processes only (not your real sessions)..."
	@# Only kill test sessions with zrp-test prefix
	@pkill -f "zellij.*--server.*zrp-test-" 2>/dev/null || true
	@pkill -f "spike_client" 2>/dev/null || true
	@rm -f /tmp/zellij-spike-resume-token
	@echo "Cleaned up"

docker-clean:
	cd $(PROJECT_ROOT) && docker compose -f zellij-remote-tests/docker-compose.yml \
		--profile netem --profile multi-client \
		down --remove-orphans --volumes 2>/dev/null || true
