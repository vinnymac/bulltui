# bulltui task runner. Run `just` to list recipes.

# Redis URL for the local demo Valkey. Host port 6380 avoids clashing with a
# Redis already bound to 6379 (e.g. another project's stack).
demo_url := "redis://127.0.0.1:6380"

_default:
    @just --list

# Format, lint and build (no tests).
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings

# Format the code.
fmt:
    cargo fmt --all

# Run the full workspace test suite (requires Docker + Node).
test:
    cargo test --workspace

# Run only the fast unit tests (no Docker/Node needed).
test-unit:
    cargo test --workspace --lib

# Install the e2e seeder's Node dependencies (run once).
seeder-install:
    cd e2e/seeder && npm install

# Start a local Valkey and seed it with demo data.
demo: seeder-install
    docker compose up -d
    sleep 1
    node e2e/seeder/seed.mjs {{demo_url}}

# Stop and remove the local Valkey.
demo-down:
    docker compose down

# Run bulltui against the local demo Valkey. Pass --url to override.
run *ARGS:
    BULLTUI_REDIS_URL={{demo_url}} cargo run -p bulltui -- {{ARGS}}

# Headless render of the overview against the local demo Valkey.
snapshot:
    BULLTUI_REDIS_URL={{demo_url}} cargo run -q -p bulltui -- --snapshot

# Build an optimized release binary.
build:
    cargo build --release -p bulltui
