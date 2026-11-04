#!/usr/bin/env bash
# Runs the one-command setup's real build stage with the installed toolchains from this
# checkout, then its install stage into a temporary prefix through a stand-in for sudo,
# both as the test user. It stops before the Keychain stages. Needs network for crates and
# npm packages and about 4 GiB of free space. No sudo, Keychain item, account or dialog.
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

wizard="$work/wizard.sh"; sed '$d' "$repo/scripts/setup-macos-protection.sh" >"$wizard"
mkdir -m 755 "$work/libexec"
overrides="LIBEXEC_PARENT=\"$work/libexec\"; LIBEXEC=\"$work/libexec/mailroom\"; TRUSTED_OWNER=\"$me\""
cat >"$work/install.sh" <<DRV
source "$wizard"
$overrides
RECORDS="$work/install-records"
install_phase "\$@"
DRV
cat >"$work/sudo" <<SH
#!/bin/bash
printf '%s\n' "\$*" >>"$work/sudo-argv"
[ "\$*" = -k ] && exit 0
ls -A "\${@: -4:1}/.." >"$work/handoff-contents"
shift \$((\$# - 4)); exec /bin/bash "$work/install.sh" "\$@"
SH
chmod 755 "$work/sudo"
cat >"$work/setup.sh" <<DRV
source "$wizard"
$overrides
CHECKOUT="$repo"; SCRIPT="$repo/scripts/setup-macos-protection.sh"; SUDO="$work/sudo"
RECORDS="$work/records"; TOTAL_STAGES=2
build_stage && install_stage
DRV
printf '%s\t%s\n' "approved commit first. [y/N]" y >"$work/answers"
/usr/bin/python3 -I "$repo/scripts/drive-wizard.py" "$work/setup.sh" "$work/transcript" "$work/answers" 1800 \
  || fail "build and install exit $?: $(grep -a 'build \|install ' "$work/records" || true)"
value() { sed -n "s/^$1 //p" "$work/records"; }
grep -qx "build ok" "$work/records" && grep -qx "install ok" "$work/records" || fail "build and install records"
[[ "$(value commit)" == "$(git -C "$repo" rev-parse HEAD)" ]] || fail "the build used the checkout's HEAD commit"
[[ "$(value output)" == /private/tmp/mailroom-build.*/out ]] || fail "the build ran under /private/tmp"
[[ "$(cat "$work/handoff-contents")" == out ]] || fail "only the signed outputs remain at the handoff"
[[ ! -e "$(value output)" ]] || fail "the build directory is removed after the install"
[[ "$(tail -1 "$work/sudo-argv")" == -k ]] || fail "the cached sudo login is dropped after the install"
grep -q "^-k /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p $repo/scripts/setup-macos-protection.sh install " "$work/sudo-argv" \
  || fail "sudo gets a cleared environment"
echo "built commit $(value commit): mailroom $(value mailroom_cdhash) mailroom-confirm $(value confirm_cdhash) ui $(value ui_digest)"

installed="$work/libexec/mailroom"
bash -c 'source "$1"; TRUSTED_OWNER="$2"; trusted_tree "$3"' _ "$wizard" "$me" "$installed" || fail "the installation is a trusted tree"
codesign --verify --strict -R="cdhash H\"$(value mailroom_cdhash)\"" "$installed/mailroom" || fail "installed mailroom cdhash"
signature=$(codesign -d --verbose=2 "$installed/mailroom" 2>&1) || fail "signature details"
[[ "$signature" =~ flags=0x[0-9a-f]+\([a-z,-]*runtime ]] || fail "hardened runtime"
grep -qa "$(value confirm_cdhash)" "$installed/mailroom" && grep -qa "$installed/mailroom-confirm" "$installed/mailroom" \
  || fail "mailroom pins the installed helper's path and cdhash"
[[ "$(/usr/bin/env -i HOME="$HOME" "$installed/mailroom" keychain refs --config "$work/no-config")" == status=ok ]] || fail "installed mailroom runs"
[[ "$(/usr/bin/env -i HOME="$HOME" "$installed/mailroom" keychain status x-password --keychain "$work/absent")" == status=keychain_unavailable ]] \
  || fail "installed mailroom is hardened and refuses a missing explicit keychain"
echo "real build and install: ok"
