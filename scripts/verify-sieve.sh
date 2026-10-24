#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo run --locked --example proof-fixtures -- .audit/sieve-fixtures
docker build --quiet -t mailroom-sieve-proof:local -f scripts/sieve/Dockerfile scripts/sieve
docker run --rm --network none --read-only --tmpfs /tmp \
  -v "$PWD/.audit/sieve-fixtures:/fixtures:ro" \
  -v "$PWD/scripts/sieve:/harness:ro" \
  mailroom-sieve-proof:local
