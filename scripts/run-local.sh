#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
npm --prefix ui ci
npm --prefix ui run build
cargo build --locked
exec ./target/debug/mailroom
