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

# Record the demo GIF (requires vhs: brew install vhs; run `just demo` first).
record: build
    vhs demo.tape

# Build an optimized release binary.
build:
    cargo build --release --locked -p bulltui

# --- packaging ---------------------------------------------------------------

# Build the distroless container image.
docker-build:
    docker build -t bulltui .

# Run the container image. Pass bulltui flags, e.g. `just docker-run --url redis://host.docker.internal:6380`.
docker-run *ARGS:
    docker run --rm -it bulltui {{ARGS}}

# Generate the npm packages (main + per-platform) from built target binaries.
# Build the per-target release binaries first (e.g. `cargo zigbuild --release`
# for each triple in [package.metadata.npm].targets). Needs `cargo install cargo-npm`.
npm-pack:
    cargo npm generate --clean

# Publish the generated npm packages (needs `npm login` or NODE_AUTH_TOKEN).
npm-publish:
    cargo npm publish

# --- release -----------------------------------------------------------------
# The knot is the source of truth; GitHub is a one-way mirror that only runs the
# tag-triggered publish (.github/workflows/release.yml). So versioning happens
# here and the review PR opens on Tangled — not on the mirror.

# Record a change for the next release (choose the bump, write a summary).
changeset:
    pnpm changeset

# Open a version-bump PR on Tangled: consume changesets onto a branch (bumping
# the version + changelog) and push it, so Tangled raises the PR for review.
version-pr:
    #!/usr/bin/env bash
    set -euo pipefail
    git switch -C release/version-packages main
    pnpm install --frozen-lockfile
    pnpm run version
    git add -A
    if git diff --cached --quiet; then
        echo "No changesets to version — add one with 'just changeset' first." >&2
        exit 1
    fi
    git commit -m "chore: version packages"
    git push -f -u origin release/version-packages
    echo "Pushed release/version-packages — open its PR into main on Tangled to review + merge."

# After the version PR merges: tag main so the mirror forwards the tag to
# GitHub, where release.yml builds and publishes. Run from an up-to-date main.
release:
    #!/usr/bin/env bash
    set -euo pipefail
    git switch main
    git pull --ff-only
    version="v$(node -p "require('./npm/bulltui/package.json').version")"
    git tag "$version"
    git push origin "$version"
    echo "Tagged $version — release.yml publishes once it mirrors to GitHub."
