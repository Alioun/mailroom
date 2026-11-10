#!/bin/bash
# Mailroom's local web setup. One command builds an exact commit once, into a runtime in the
# owner's home outside every checkout, and starts it. The browser then creates or unlocks the
# vault and adds accounts. Nothing is installed system-wide; no sudo, launch agent or Keychain.
#
# First start of a version. Naming the exact commit is the owner's approval of that version,
# and the script that runs is the one inside that commit, never the working tree:
#   git -C <checkout> show <commit>:scripts/local-web.sh | /bin/bash -s -- approve <checkout> <commit>
# Every later start, with no rebuild and nothing loaded from a checkout:
#   "$HOME/Library/Application Support/Mailroom Web Runtime/start"
#
# Status lines: status=<code>. Exit 64 usage, 65 changed or unsafe runtime, 66 not approved,
# 69 toolchain missing, 70 build failed.
set -euo pipefail
export PATH=/usr/bin:/bin LC_ALL=C
umask 077

# Trusted locations and tools. Tests override these only by sourcing this file.
CARGO=/opt/homebrew/opt/rust/bin/cargo
RUSTC=/opt/homebrew/opt/rust/bin/rustc
NODE=/usr/local/bin/node
NPM_CLI=/opt/homebrew/lib/node_modules/npm/bin/npm-cli.js
GIT=/usr/bin/git
if [[ "$(uname -s)" == Darwin ]]; then
  RUNTIME_HOME="$HOME/Library/Application Support/Mailroom Web Runtime"
  BUILD_BASE=/private/tmp
else
  RUNTIME_HOME="$HOME/.local/share/mailroom-web-runtime"
  BUILD_BASE=/tmp
fi
ADDRESS=http://127.0.0.1:4173/

fail() { printf 'status=%s\n' "$1"; exit "${2:-65}"; }

sha256() {
  if [[ -x /usr/bin/shasum ]]; then shasum -a 256 "$@"; else sha256sum "$@"; fi | cut -c1-64
}

# Every file's digest and relative path, in byte order. Anything but files and directories
# makes the tree untrustworthy.
tree_digest() {
  local tree="$1"
  [[ -z "$(find "$tree" ! -type f ! -type d -print -quit)" ]] || return 1
  (cd "$tree" && find . -type f -print | sort | while IFS= read -r file; do
    printf '%s  %s\n' "$(sha256 "$file")" "$file"
  done) | sha256
}

# Owned by this user, not a link, and writable by nobody else, throughout.
private_tree() {
  [[ -e "$1" && ! -L "$1" ]] || return 1
  [[ -z "$(find "$1" \( ! -user "$(id -u)" -o -perm -020 -o -perm -002 \) -print -quit)" ]]
}

pin() { sed -n "s/^$1=//p" "$2/pins" 2>/dev/null; }

verify_version() {
  local version="$1" commit
  commit="$(basename "$version")"
  private_tree "$version" || return 1
  [[ "$(ls -A "$version" | tr '\n' ' ')" == "local-web.sh mailroom pins ui " ]] || return 1
  [[ "$(pin commit "$version")" == "$commit" ]] || return 1
  [[ "$(sha256 "$version/mailroom")" == "$(pin mailroom "$version")" ]] || return 1
  [[ "$(sha256 "$version/local-web.sh")" == "$(pin launcher "$version")" ]] || return 1
  [[ "$(tree_digest "$version/ui")" == "$(pin ui "$version")" ]] || return 1
}

# Cargo reads .cargo/config.toml from every ancestor of its working directory.
no_outside_config() {
  local directory="$1"
  while [[ "$directory" != / ]]; do
    directory="$(dirname "$directory")"
    [[ ! -e "$directory/.cargo" ]] || return 1
  done
}

# Builds $commit from `git archive`, with a cleared environment and a private home, cargo home
# and npm cache, against the final path of its UI. Publishes it with one rename.
build() {
  local checkout="$1" commit="$2" version="$3" work log="$RUNTIME_HOME/last-build.log"
  [[ -x "$CARGO" && -x "$RUSTC" && -x "$NODE" && -f "$NPM_CLI" ]] || fail toolchain_missing 69
  work="$(mktemp -d "$BUILD_BASE/mailroom-web-build.XXXXXX")"
  # shellcheck disable=SC2064
  trap "chmod -R u+w '$work' 2>/dev/null; rm -rf '$work'" EXIT
  no_outside_config "$work" || fail outside_config 70
  mkdir "$work/home" "$work/cargo-home" "$work/npm-cache" "$work/src" "$work/out"
  : >"$work/home/user-npmrc"
  : >"$work/home/global-npmrc"
  local clean=(env -i PATH="/usr/bin:/bin:$(dirname "$RUSTC"):$(dirname "$NODE")" HOME="$work/home"
    CARGO_HOME="$work/cargo-home" RUSTC="$RUSTC" CARGO_TARGET_DIR="$work/target"
    NPM_CONFIG_USERCONFIG="$work/home/user-npmrc" NPM_CONFIG_GLOBALCONFIG="$work/home/global-npmrc"
    NPM_CONFIG_CACHE="$work/npm-cache" NPM_CONFIG_UPDATE_NOTIFIER=false)
  ("$GIT" --no-replace-objects -C "$checkout" archive --format=tar "$commit" | tar -xf - -C "$work/src") \
    >"$log" 2>&1 || fail build_source 70
  rm -rf "${work:?}/src/ui/dist" "${work:?}/src/ui/node_modules"
  (cd "$work/src/ui" && "${clean[@]}" "$NODE" "$NPM_CLI" ci && "${clean[@]}" "$NODE" "$NPM_CLI" run build) \
    >>"$log" 2>&1 || fail build_ui 70
  (cd "$work/src" && "${clean[@]}" MAILROOM_UI_DIR="$version/ui" "$CARGO" build --release --locked) \
    >>"$log" 2>&1 || fail build_cargo 70
  cp -R "$work/src/ui/dist" "$work/out/ui"
  cp "$work/target/release/mailroom" "$work/out/mailroom"
  cp "$work/src/scripts/local-web.sh" "$work/out/local-web.sh"
  printf 'commit=%s\nmailroom=%s\nui=%s\nlauncher=%s\n' "$commit" "$(sha256 "$work/out/mailroom")" \
    "$(tree_digest "$work/out/ui")" "$(sha256 "$work/out/local-web.sh")" >"$work/out/pins"
  local incoming
  incoming="$(mktemp -d "$RUNTIME_HOME/incoming.XXXXXX")"
  cp -R "$work/out/." "$incoming"
  chmod -R a-w "$incoming/ui" "$incoming/pins" "$incoming/local-web.sh"
  chmod 500 "$incoming/mailroom"
  # Renaming a directory updates its "..", so it stays writable until it is in place.
  mv "$incoming" "$version"
  chmod 500 "$version"
  chmod -R u+w "$work" 2>/dev/null || true
  rm -rf "${work:?}"
  trap - EXIT
}

approve() {
  local checkout="${1:-}" commit="${2:-}"
  [[ "$commit" =~ ^[0-9a-f]{40}$ ]] || fail commit_not_exact 64
  [[ -d "$checkout" ]] || fail checkout_missing 64
  [[ "$("$GIT" --no-replace-objects -C "$checkout" rev-parse --verify --quiet "$commit^{commit}" 2>/dev/null)" == "$commit" ]] \
    || fail commit_unknown 64
  mkdir -p "$RUNTIME_HOME/versions"
  private_tree "$RUNTIME_HOME" || fail unsafe 65
  local version="$RUNTIME_HOME/versions/$commit"
  if [[ -e "$version" ]]; then
    verify_version "$version" || { printf 'path=%s\n' "$version"; fail changed 65; }
  else
    build "$checkout" "$commit" "$version"
    printf 'status=built\n'
  fi
  # The daily command is this version's own launcher, installed by a rename.
  cp "$version/local-web.sh" "$RUNTIME_HOME/.start.new"
  chmod 500 "$RUNTIME_HOME/.start.new"
  mv -f "$RUNTIME_HOME/.start.new" "$RUNTIME_HOME/start"
  printf '%s\n' "$commit" >"$RUNTIME_HOME/.approved.new"
  chmod 400 "$RUNTIME_HOME/.approved.new"
  mv -f "$RUNTIME_HOME/.approved.new" "$RUNTIME_HOME/approved"
  printf 'start=%s\n' "$RUNTIME_HOME/start"
  start
}

open_browser() {
  [[ -t 1 && "$(uname -s)" == Darwin ]] || return 0
  (for _ in $(seq 1 150); do
    sleep 0.2
    if curl -fsS -o /dev/null "$ADDRESS" 2>/dev/null; then
      env -i HOME="$HOME" PATH=/usr/bin:/bin /usr/bin/open "$ADDRESS"
      exit 0
    fi
  done) >/dev/null 2>&1 &
}

start() {
  local commit version
  [[ -f "$RUNTIME_HOME/approved" ]] || fail not_approved 66
  commit="$(cat "$RUNTIME_HOME/approved")"
  [[ "$commit" =~ ^[0-9a-f]{40}$ ]] || fail not_approved 66
  version="$RUNTIME_HOME/versions/$commit"
  private_tree "$RUNTIME_HOME" || fail unsafe 65
  verify_version "$version" || fail changed 65
  [[ "$(sha256 "$RUNTIME_HOME/start")" == "$(pin launcher "$version")" ]] || fail changed 65
  open_browser
  exec env -i HOME="$HOME" PATH=/usr/bin:/bin "$version/mailroom" --local-vault
}

main() {
  case "${1:-start}" in
    approve) shift; approve "$@" ;;
    start) start ;;
    *) fail usage 64 ;;
  esac
}

if [[ -z "${BASH_SOURCE[0]:-}" || "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
