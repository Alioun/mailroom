#!/usr/bin/env bash
# Runs the one-command setup's real build stage with the installed toolchains on a clone of
# this checkout's HEAD commit, which publishes the runtime into the clone's
# build/local/<commit>, then checks that runtime as the run subcommand does. It stops before
# the Keychain stages. Needs network for crates and npm packages and about 4 GiB of free
# space. No sudo, Keychain item, account or dialog.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
work="$(mktemp -d)"; work="$(cd "$work" && pwd -P)"
cleanup() {
  local built; built=$(sed -n 's/^output //p' "$work/records" 2>/dev/null || true); built="${built%/out}"
  [[ "$built" == /private/tmp/mailroom-build.* ]] && rm -rf "${built:?}"
  [[ -n "${KEEP:-}" ]] && echo "work=$work" || { chmod -R u+rwx "${work:?}"; rm -rf "${work:?}"; }
}
trap cleanup EXIT
chmod 700 "$work"
me="$(id -un)"
fail() { echo "FAIL: $1"; exit 1; }

head=$(git -C "$repo" rev-parse HEAD); checkout="$work/checkout"
git clone -q --no-hardlinks --no-checkout "$repo" "$checkout" && git -C "$checkout" checkout -q --detach "$head"
wizard="$work/wizard.sh"; sed '$d' "$repo/scripts/setup-macos-protection.sh" >"$wizard"
cat >"$work/setup.sh" <<DRV
source "$wizard"
CHECKOUT="$checkout"; TRUSTED_OWNER="$me"; RECORDS="$work/records"; TOTAL_STAGES=1
build_stage
DRV
printf '%s\t%s\n' "approved commit first. [y/N]" y >"$work/answers"
/usr/bin/python3 -I "$repo/scripts/drive-wizard.py" "$work/setup.sh" "$work/transcript" "$work/answers" 1800 \
  || fail "build and publish exit $?: $(grep -a 'build \|publish ' "$work/records" || true)"
value() { sed -n "s/^$1 //p" "$work/records"; }
grep -qx "build ok" "$work/records" && grep -qx "publish ok" "$work/records" || fail "build and publish records"
[[ "$(value commit)" == "$head" ]] || fail "the build used the checkout's HEAD commit"
[[ "$(value output)" == /private/tmp/mailroom-build.*/out ]] || fail "the build ran under /private/tmp"
[[ ! -e "$(value output)" ]] || fail "the build directory is removed after publishing"
echo "built commit $head: mailroom $(value mailroom_cdhash) mailroom-confirm $(value confirm_cdhash) ui $(value ui_digest)"

runtime="$checkout/build/local/$head"
[[ "$(ls -A "$checkout/build/local")" == "$head" ]] || fail "only the runtime is left in build/local"
[[ -z "$(git -C "$checkout" status --porcelain --untracked-files=all)" ]] || fail "the ignored runtime leaves the checkout clean"
bash -c 'source "$1"; TRUSTED_OWNER="$2"; CHECKOUT="$3"; head_runtime && verify_runtime' _ "$wizard" "$me" "$checkout" \
  || fail "the runtime verifies as run checks it"
signature=$(codesign -d --verbose=2 "$runtime/mailroom" 2>&1) || fail "signature details"
[[ "$signature" =~ flags=0x[0-9a-f]+\([a-z,-]*runtime ]] || fail "hardened runtime"
grep -qa "$(value confirm_cdhash)" "$runtime/mailroom" && grep -qa "$runtime/mailroom-confirm" "$runtime/mailroom" \
  && grep -qa "$runtime/ui" "$runtime/mailroom" || fail "mailroom pins the runtime's helper path, helper cdhash and UI path"
[[ "$(/usr/bin/env -i HOME="$HOME" "$runtime/mailroom" keychain refs --config "$work/no-config")" == status=ok ]] || fail "the runtime's mailroom runs"
[[ "$(/usr/bin/env -i HOME="$HOME" "$runtime/mailroom" keychain status x-password --keychain "$work/absent")" == status=keychain_unavailable ]] \
  || fail "the runtime's mailroom is hardened and refuses a missing explicit keychain"
echo "real build and publish: ok"
