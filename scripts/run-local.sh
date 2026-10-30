#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
npm --prefix ui ci
npm --prefix ui run build
# Optimized: a personal-scale account takes seconds per request in a debug build.
exec cargo run --locked --release -- "$@"
