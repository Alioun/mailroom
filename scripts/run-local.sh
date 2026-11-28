#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

ADDRESS="http://127.0.0.1:4173"
mode=""
cargo_profile="release"
clean_ui=0
open_ui=0

usage() {
  cat <<'EOF'
Usage: ./scripts/run-local.sh [options]

Builds the UI and runs Mailroom locally at http://127.0.0.1:4173.

Modes (pick at most one):
  --local-vault          Browser master-password vault (~/.config/mailroom-web) [default]
  --demo                 Synthetic sandbox with 100 invented headers (--synthetic-accounts)
  --synthetic-accounts   Alias for --demo
  --native               Native macOS runtime flags omitted (requires native setup)

Options:
  --open                 Open http://127.0.0.1:4173 in your browser once ready (macOS)
  --debug                Run debug build instead of --release
  --clean                Reinstall ui/node_modules via npm ci before building
  -h, --help             Show this help
EOF
}

set_mode() {
  if [[ -n "$mode" && "$mode" != "$1" ]]; then
    printf 'Error: conflicting mode flags (--%s and --%s)\n' "$mode" "$1" >&2
    exit 2
  fi
  mode="$1"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --demo|--synthetic-accounts) set_mode "synthetic" ;;
    --local-vault) set_mode "local-vault" ;;
    --native) set_mode "native" ;;
    --open) open_ui=1 ;;
    --debug) cargo_profile="debug" ;;
    --clean) clean_ui=1 ;;
    -h|--help) usage; exit 0 ;;
    *)
      printf 'Error: unknown option: %s\n\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

mode="${mode:-local-vault}"

if ! command -v cargo >/dev/null 2>&1; then
  printf 'Error: Rust cargo is required (https://rustup.rs or brew install rust).\n' >&2
  exit 1
fi
if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
  printf 'Error: Node.js (>= 22.12) and npm are required.\n' >&2
  exit 1
fi
if ! node -e 'const [M,m]=process.versions.node.split(".").map(Number); if(M<22||(M===22&&m<12)) process.exit(1)'; then
  printf 'Error: Node.js >= 22.12 is required (found %s).\n' "$(node -v)" >&2
  exit 1
fi

if curl -fsS -o /dev/null "$ADDRESS/api/session" 2>/dev/null; then
  printf 'Mailroom is already running at %s\n' "$ADDRESS" >&2
  exit 1
fi

if [[ "$clean_ui" -eq 1 || ! -d ui/node_modules || ui/package-lock.json -nt ui/node_modules/.package-lock.json ]]; then
  npm --prefix ui ci
fi
npm --prefix ui run build

if [[ "$open_ui" -eq 1 && "$(uname -s)" == Darwin ]]; then
  (
    for _ in $(seq 1 150); do
      sleep 0.2
      if curl -fsS -o /dev/null "$ADDRESS/api/session" 2>/dev/null; then
        /usr/bin/open "$ADDRESS"
        exit 0
      fi
    done
  ) >/dev/null 2>&1 &
fi

cargo_args=(run --locked)
if [[ "$cargo_profile" == "release" ]]; then
  cargo_args+=(--release)
fi

mode_args=()
case "$mode" in
  local-vault) mode_args=(--local-vault) ;;
  synthetic) mode_args=(--synthetic-accounts) ;;
  native) mode_args=() ;;
esac

exec cargo "${cargo_args[@]}" -- ${mode_args[@]+"${mode_args[@]}"}
