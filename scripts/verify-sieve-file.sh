#!/usr/bin/env bash
set -euo pipefail
if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "Usage: $0 <path/to/rules.sieve> [optional-eml-directory]" >&2
  exit 2
fi
script_path="$(cd "$(dirname "$1")" && pwd -P)/$(basename "$1")"
if [ ! -f "$script_path" ]; then
  echo "Error: Sieve file not found: $script_path" >&2
  exit 1
fi
repo_root="$(cd "$(dirname "$0")/.." && pwd -P)"
docker build --quiet -t mailroom-sieve-proof:local -f "$repo_root/scripts/sieve/Dockerfile" "$repo_root/scripts/sieve" >/dev/null

docker_args=(
  run --rm --network none --read-only --tmpfs /tmp
  --entrypoint python3
  -v "$script_path:/input/rules.sieve:ro"
  -v "$repo_root/scripts/sieve:/harness:ro"
)
if [ "$#" -eq 2 ]; then
  samples_dir="$(cd "$2" && pwd -P)"
  docker_args+=(-v "$samples_dir:/samples:ro")
fi
docker_args+=(mailroom-sieve-proof:local /harness/check_file.py)

docker "${docker_args[@]}"
