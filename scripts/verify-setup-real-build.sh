#!/usr/bin/env bash
# Runs the wizard's real build phase with the installed toolchains, then its install phase
# into a temporary prefix, both as the test user. The frozen copy is `git archive HEAD`.
# Needs network for crates and npm packages and about 4 GiB of free space. No sudo,
# Keychain item, account or dialog.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"; work="$(cd "$work" && pwd -P)"
built=""
cleanup() {
  [[ "$built" == /private/tmp/mailroom-build.* ]] && rm -rf "${built:?}"
  [[ -n "${KEEP:-}" ]] && echo "work=$work" || { chmod -R u+rwx "${work:?}"; rm -rf "${work:?}"; }
}
trap cleanup EXIT
chmod 700 "$work"
me="$(id -un)"
fail() { echo "FAIL: $1"; exit 1; }

commit=$(git -C "$repo" rev-parse HEAD)
mkdir -m 755 "$work/share" "$work/libexec"
frozen="$work/share/setup-$commit"
mkdir "$frozen"; git -C "$repo" archive --format=tar "$commit" | tar -x -C "$frozen"
chmod -R go-w "$frozen"; chmod 755 "$frozen"
wizard="$work/wizard.sh"; sed '$d' "$frozen/scripts/setup-macos-protection.sh" >"$wizard"
digest=$(bash -c 'source "$1"; tree_digest "$2"' _ "$wizard" "$frozen")

phase() {
  local name="$1" call="$2"
  cat >"$work/$name.sh" <<DRV
source "$wizard"
SHARE="$work/share"; LIBEXEC_PARENT="$work/libexec"; LIBEXEC="$work/libexec/mailroom"; TRUSTED_OWNER="$me"
RECORDS="$work/$name-records"
$call
DRV
  /usr/bin/python3 -I "$repo/scripts/drive-wizard.py" "$work/$name.sh" "$work/$name-transcript" "$work/$name-answers" 1800
}

printf '%s\t%s\n' "approval record:" "$digest" >"$work/build-answers"
phase build "build_phase \"$frozen\"" || fail "build phase exit $?: $(grep -a 'build ' "$work/build-records" || true)"
built=$(sed -n 's/^output //p' "$work/build-records"); built="${built%/out}"
[[ "$built" == /private/tmp/mailroom-build.* ]] || fail "the build ran under /private/tmp"
grep -qx "build ok" "$work/build-records" || fail "build record"
[[ "$(ls -A "$built")" == out ]] || fail "only the signed outputs remain"
value() { sed -n "s/^$1 //p" "$work/build-records"; }
echo "built mailroom $(value mailroom_cdhash) mailroom-confirm $(value confirm_cdhash) ui $(value ui_digest)"

printf '%s\t%s\n' "approval record:" "$digest" "mailroom cdhash:" "$(value mailroom_cdhash)" \
  "mailroom-confirm cdhash:" "$(value confirm_cdhash)" "UI digest:" "$(value ui_digest)" >"$work/install-answers"
phase install "install_phase \"$frozen\" \"$built/out\"" || fail "install phase exit $?: $(cat "$work/install-records")"
grep -qx "install ok" "$work/install-records" || fail "install record"

installed="$work/libexec/mailroom"
bash -c 'source "$1"; TRUSTED_OWNER="$2"; trusted_tree "$3"' _ "$wizard" "$me" "$installed" || fail "the installation is a trusted tree"
codesign --verify --strict -R="cdhash H\"$(value mailroom_cdhash)\"" "$installed/mailroom" || fail "installed mailroom cdhash"
codesign -d --verbose=2 "$installed/mailroom" 2>&1 | grep -q 'flags=.*runtime' || fail "hardened runtime"
grep -qa "$(value confirm_cdhash)" "$installed/mailroom" && grep -qa "$installed/mailroom-confirm" "$installed/mailroom" \
  || fail "mailroom pins the installed helper's path and cdhash"
[[ "$(/usr/bin/env -i HOME="$HOME" "$installed/mailroom" keychain refs --config "$work/no-config")" == status=ok ]] || fail "installed mailroom runs"
[[ "$(/usr/bin/env -i HOME="$HOME" "$installed/mailroom" keychain status x-password --keychain "$work/absent")" == status=keychain_unavailable ]] \
  || fail "installed mailroom is hardened and refuses a missing explicit keychain"
echo "real build and install: ok"
