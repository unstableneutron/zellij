# Zellij Agent Guidelines

## Build Commands
- Build & test all: `cargo xtask` (or `cargo x`) - runs format, build, test, clippy
- Build only: `cargo xtask build` (release: `cargo xtask build -r`)
- Run dev: `cargo xtask run` (with args: `cargo xtask run -- <args>`)
- Format: `cargo xtask format` (check: `cargo xtask format --check`)
- Lint: `cargo xtask clippy`
- Test all: `cargo xtask test`
- Single test: `cargo xtask test -- <test_name>` or `cargo test -p <crate> -- <test_name>`
- E2E tests: `docker-compose up -d && cargo xtask ci e2e --build && cargo xtask ci e2e --test`

## Architecture
- **zellij-client/**: Client-side terminal handling and user input
- **zellij-server/**: Server managing panes, tabs, sessions; core multiplexer logic
- **zellij-utils/**: Shared utilities, config parsing, IPC, error handling
- **zellij-tile/**: Plugin API for WASM plugins (plugins compile to wasm32-wasi)
- **default-plugins/**: Built-in plugins (status-bar, tab-bar, strider, session-manager, etc.)
- **src/**: Main binary entry point, CLI argument parsing
- Protobuf (.proto files) used for plugin↔host communication across WASM boundary

## Code Style
- Run `cargo x` and address all issues before commits; uses `match_block_trailing_comma = true`
- Prefer `Result<T>` over `unwrap()`; use `use zellij_utils::errors::prelude::*` and `.context()`
- Log errors with `.non_fatal()` instead of `log::error!`; logs go to `/tmp/zellij-<UID>/zellij-log/`
- Follow Conventional Commits for significant changes
