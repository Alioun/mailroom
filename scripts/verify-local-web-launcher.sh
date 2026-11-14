#!/bin/bash
# Exercises scripts/local-web.sh with a dummy home, a throwaway git repository and fake npm and
# cargo, so nothing real is built, installed or started. Run from the checkout.
set -euo pipefail
source_script="$(cd "$(dirname "$0")" && pwd)/local-web.sh"
root="$(mktemp -d "${TMPDIR:-/tmp}/mailroom-web-launcher.XXXXXX")"
root="$(cd "$root" && pwd -P)"
# Cargo reads .cargo from every ancestor, so the build workspace stays out of any home.
build_base="$(mktemp -d /tmp/mailroom-web-launcher-build.XXXXXX)"
trap 'chmod -R u+w "$root" "$build_base" 2>/dev/null; rm -rf "$root" "$build_base"' EXIT
checks=0
pass() { checks=$((checks + 1)); printf 'ok %s\n' "$1"; }
die() { printf 'FAIL %s\n' "$1"; exit 1; }

home="$root/home" repo="$root/repo" tools="$root/tools" calls="$root/calls"
mkdir -m 700 "$home" "$tools"
: >"$calls"
mkdir -p "$repo/scripts" "$repo/ui/src"
cp "$source_script" "$repo/scripts/local-web.sh"
printf '{"name":"fixture"}\n' >"$repo/ui/package.json"
printf 'committed\n' >"$repo/ui/src/marker"
git -C "$repo" init -q
git -C "$repo" -c user.name=fixture -c user.email=fixture@example.test add -A
git -C "$repo" -c user.name=fixture -c user.email=fixture@example.test commit -qm fixture
commit="$(git -C "$repo" rev-parse HEAD)"
# A dirty working tree must not reach the build.
printf 'echo DIRTY-WORKTREE\n' >>"$repo/scripts/local-web.sh"
printf 'dirty\n' >"$repo/ui/src/marker"

cat >"$tools/node" <<EOF
#!/bin/bash
printf 'node %s\n' "\$2" >>'$calls'
env >>'$root/tool-env'
case "\$2" in
  ci) mkdir -p node_modules ;;
  run) mkdir -p dist/assets && cat src/marker >dist/index.html && printf 'body{}' >dist/assets/app.css ;;
esac
EOF
cat >"$tools/cargo" <<EOF
#!/bin/bash
printf 'cargo %s\n' "\$*" >>'$calls'
env >>'$root/tool-env'
mkdir -p "\$CARGO_TARGET_DIR/release"
{ printf '#!/bin/sh\n'; printf '# ui=%s\n' "\$MAILROOM_UI_DIR"; printf 'env >"\$HOME/exec-env"\nprintf "%%s\\\\n" "\$*" >"\$HOME/exec-args"\n'; } >"\$CARGO_TARGET_DIR/release/mailroom"
chmod 755 "\$CARGO_TARGET_DIR/release/mailroom"
EOF
printf '#!/bin/sh\n' >"$tools/rustc"
: >"$tools/npm-cli.js"
chmod 755 "$tools/node" "$tools/cargo" "$tools/rustc"

# Runs one launcher entry point as the owner would, with the fake toolchain sourced in.
launch() {
  (
    export HOME="$home" MAILROOM_WEB_TEST_CANARY=owner-environment-canary
    # shellcheck disable=SC1090
    source "$source_script"
    CARGO="$tools/cargo" RUSTC="$tools/rustc" NODE="$tools/node" NPM_CLI="$tools/npm-cli.js" BUILD_BASE="${MAILROOM_TEST_BUILD_BASE:-$build_base}"
    main "$@"
  )
}
runtime() { (export HOME="$home"; source "$source_script"; printf '%s' "$RUNTIME_HOME"); }
count() { grep -c "^$1" "$calls" || true; }
daily() { env -i HOME="$home" "$(runtime)/start"; }

out="$(launch approve "$repo" "${commit:0:12}" || true)"
[[ "$out" == "status=commit_not_exact" ]] || die "abbreviated commit refused: $out"
out="$(launch approve "$repo" "$(printf '%040d' 0)" || true)"
[[ "$out" == "status=commit_unknown" ]] || die "unknown commit refused: $out"
out="$(/usr/bin/env -i HOME="$home" PATH=/usr/bin:/bin /usr/bin/git --no-replace-objects -C "$repo" show "$commit:scripts/local-web.sh" |
  /usr/bin/env -i HOME="$home" PATH=/usr/bin:/bin /bin/bash -s -- approve "$repo" abc || true)"
[[ "$out" == "status=commit_not_exact" ]] || die "piped launcher refuses a short commit: $out"
out="$(env -i HOME="$home" /bin/bash "$source_script" || true)"
[[ "$out" == "status=not_approved" ]] || die "daily start before approval: $out"
[[ ! -e "$home/exec-args" ]] || die "nothing started before approval"
pass "approval needs the exact full commit, and nothing starts before it"

original="$(git --no-replace-objects -C "$repo" rev-parse "$commit:scripts/local-web.sh")"
replacement="$(printf 'printf "UNREVIEWED-LAUNCHER\\n"\n' | git -C "$repo" hash-object -w --stdin)"
git -C "$repo" replace "$original" "$replacement"
[[ "$(git -C "$repo" show "$commit:scripts/local-web.sh")" == *UNREVIEWED-LAUNCHER* ]] || die "replace fixture substitutes the launcher"
printf 'printf "UNREVIEWED-STARTUP\\n"\n' >"$root/startup"
out="$(
  export BASH_ENV="$root/startup"
  uname() { printf 'UNREVIEWED-FUNCTION\n' >"$HOME/exported-function-ran"; /usr/bin/uname "$@"; }
  export -f uname
  /usr/bin/env -i HOME="$home" PATH=/usr/bin:/bin /usr/bin/git --no-replace-objects -C "$repo" show "$commit:scripts/local-web.sh" |
    /usr/bin/env -i HOME="$home" PATH=/usr/bin:/bin /bin/bash -s -- approve "$repo" abc || true
)"
[[ "$out" == "status=commit_not_exact" ]] || die "only committed launcher code ran: $out"
[[ ! -e "$home/exported-function-ran" ]] || die "the exported uname function ran"
git -C "$repo" replace -d "$original" >/dev/null
pass "the exact entry command ignores Git replacements, BASH_ENV and exported functions"

mkdir -p "$root/configured/.cargo" "$root/configured/base"
out="$(MAILROOM_TEST_BUILD_BASE="$root/configured/base" launch approve "$repo" "$commit" || true)"
[[ "$out" == "status=outside_config" && ! -s "$calls" ]] || die "a build under a .cargo directory refused: $out"
pass "a build workspace below a .cargo directory is refused before any tool runs"

out="$(launch approve "$repo" "$commit")"
[[ "$out" == *status=built* ]] || die "first approval builds: $out"
[[ "$(count 'node ci')" == 1 && "$(count 'node run')" == 1 && "$(count 'cargo build --release --locked')" == 1 ]] || die "one build: $(cat "$calls")"
[[ "$(cat "$home/exec-args")" == "--local-vault" ]] || die "starts the vault server"
leaked="$(grep -vE '^(HOME|PATH|PWD|SHLVL|_|OLDPWD)=' "$home/exec-env" || true)"
[[ -z "$leaked" ]] || die "the server got only HOME and PATH: $leaked"
grep -qx "HOME=$home" "$home/exec-env" && grep -qx 'PATH=/usr/bin:/bin' "$home/exec-env" || die "server HOME and PATH"
! grep -q owner-environment-canary "$root/tool-env" "$home/exec-env" || die "owner environment reached the build or server"
! grep -q "HOME=$home" "$root/tool-env" || die "the build ran in its own home"
pass "the first approval builds once in a cleared environment and starts only HOME and PATH"

rt="$(runtime)"
version="$rt/versions/$commit"
[[ -x "$rt/start" && "$(cat "$rt/approved")" == "$commit" ]] || die "daily start installed"
grep -q "# ui=$version/ui" "$version/mailroom" || die "UI path compiled to the frozen runtime"
grep -q committed "$version/ui/index.html" || die "UI built from the commit"
! grep -q DIRTY-WORKTREE "$version/local-web.sh" "$rt/start" || die "launcher from the commit, not the working tree"
cmp -s "$rt/start" <(git --no-replace-objects -C "$repo" show "$commit:scripts/local-web.sh") || die "daily start is the committed launcher"
[[ -z "$(find "$version" -perm -200 -type f -print -quit)" ]] || die "frozen files are read-only"
[[ -z "$(find "$rt" \( -perm -020 -o -perm -002 \) -print -quit)" ]] || die "runtime private"
[[ -z "$(ls "$build_base")" ]] || die "build workspace removed"
[[ "$(git -C "$repo" status --porcelain | sort)" == "$(printf ' M scripts/local-web.sh\n M ui/src/marker')" ]] || die "checkout untouched"
case "$rt" in "$home"/*) ;; *) die "runtime lives in the owner's home" ;; esac
pass "the frozen runtime holds the committed launcher, binary and UI, read-only and private"

rm -f "$home/exec-args"
daily >/dev/null
[[ "$(cat "$home/exec-args")" == "--local-vault" ]] || die "daily start runs the server"
rm -f "$home/exec-args"
launch approve "$repo" "$commit" >/dev/null
[[ "$(count 'node ci')" == 1 && "$(count 'cargo build')" == 1 ]] || die "unchanged runs never rebuild"
[[ -e "$home/exec-args" ]] || die "repeated approval starts the server"
pass "daily and repeated starts verify and start without npm or cargo"

tamper() {
  local file="$1" status
  rm -f "$root/original"
  cp "$file" "$root/original"
  chmod u+w "$(dirname "$file")" "$file"
  printf 'x' >>"$file"
  rm -f "$home/exec-args"
  status=0
  out="$(daily)" || status=$?
  [[ "$out" == "status=changed" && "$status" == 65 && ! -e "$home/exec-args" ]] || die "changed $file refused: $out $status"
  cat "$root/original" >"$file"
  chmod a-w "$file"
  [[ "$file" == "$rt/start" ]] || chmod 500 "$(dirname "$file")"
  [[ "$file" == "$rt/start" ]] && chmod 500 "$file"
  daily >/dev/null
  [[ -e "$home/exec-args" ]] || die "restored $file starts again"
}
tamper "$version/mailroom"
tamper "$version/ui/index.html"
tamper "$version/local-web.sh"
tamper "$rt/start"
chmod u+w "$version"
printf 'extra' >"$version/ui-extra"
chmod 500 "$version"
rm -f "$home/exec-args"
out="$(daily || true)"
[[ "$out" == "status=changed" && ! -e "$home/exec-args" ]] || die "an extra runtime file refused: $out"
chmod u+w "$version"
rm "$version/ui-extra"
chmod 500 "$version"
chmod u+w "$version/ui"
ln -s /etc/hosts "$version/ui/link"
rm -f "$home/exec-args"
out="$(daily || true)"
# A link reads as group-writable on Linux and as a changed tree on macOS; both refuse it.
[[ "$out" =~ ^status=(changed|unsafe)$ && ! -e "$home/exec-args" ]] || die "a link in the UI tree refused: $out"
rm "$version/ui/link"
chmod a-w "$version/ui"
pass "a changed binary, UI file, pinned launcher or daily start is refused and nothing starts"

chmod g+w "$rt"
rm -f "$home/exec-args"
out="$(daily || true)"
[[ "$out" == "status=unsafe" && ! -e "$home/exec-args" ]] || die "group-writable runtime refused: $out"
chmod g-w "$rt"
daily >/dev/null
pass "a runtime others can write is refused"

printf 'PASS: %s launcher checks\n' "$checks"
