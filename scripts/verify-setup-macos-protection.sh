#!/usr/bin/env bash
# Drives the wizard's real stage logic in a pseudo-terminal: the build phase with fake
# tools, the install phase on signed fake outputs, the owner phase with a signed fake
# mailroom, the ACL scanner on large and unreadable trees, and the freeze bootstrap block
# copied from docs/macos-confirmation.md. A temporary tree owned by the test user stands
# in for the root-owned locations. No sudo, Keychain or real build;
# scripts/verify-setup-real-build.sh runs the real build.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"; work="$(cd "$work" && pwd -P)"
trap '[[ -n "${KEEP:-}" ]] && echo "work=$work" || { chmod -R u+rwx "${work:?}"; rm -rf "${work:?}"; }' EXIT
chmod 700 "$work"
me="$(id -un)"
dummy="nk-dummy-wizard-$RANDOM$RANDOM"
passed=0
fail() { echo "FAIL: $1"; exit 1; }
ok() { passed=$((passed + 1)); }

cat >"$work/wrapper.c" <<'C'
#include <libgen.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
  char script[4096]; char self[4096]; strncpy(self, argv[0], sizeof self - 1);
  snprintf(script, sizeof script, "%s/fake.sh", dirname(self));
  execv("/bin/sh", (char *[]){"/bin/sh", script, argc > 1 ? argv[1] : "", argc > 2 ? argv[2] : "", argc > 3 ? argv[3] : "", NULL});
  return 127;
}
C
/usr/bin/clang -o "$work/fake-mailroom" "$work/wrapper.c"

cat >"$work/fake.sh" <<'SH'
here=$(dirname "$0"); scenario=$(cat "$here/scenario"); log="$here/calls.log"
echo "$*" >>"$log"
once() { [ -e "$here/$1" ] && return 1; : >"$here/$1"; }
case "$1 $2" in
  "native-check ")
    [ "$scenario" = native_denied ] && { echo outcome=denied; exit 1; }
    if once asked; then echo outcome=approved; else echo outcome=cancelled; fi ;;
  "keychain refs")
    [ "$scenario" = shaped_secret ] && echo "ref=$(cat "$here/dummy")-password class=every_access"
    echo "ref=acct-password class=every_access"; echo "ref=acct-census-key class=pinned_self"
    [ -e "$here/added" ] && { echo "ref=acct2-password class=every_access"; echo "ref=acct2-census-key class=pinned_self"; }
    echo status=ok ;;
  "keychain legacy")
    echo "ref=acct-password class=every_access"; echo "ref=acct-census-key class=pinned_self"
    echo "ref=stale-census-key class=pinned_self"; echo status=ok ;;
  "keychain settle")
    [ "$scenario" = killed ] && kill -9 $$
    [ "$scenario" = staged_planted ] && { echo status=mismatch; exit 1; }
    [ "$scenario" = success_exit1 ] && { echo status=settled; exit 1; }
    [ "$scenario" = uncommitted ] && [ ! -e "$here/adopted" ] && { echo status=uncommitted; exit 1; }
    echo status=settled ;;
  "keychain adopt") : >"$here/adopted"; echo status=settled ;;
  "keychain migrate") [ "$scenario" = mismatch ] && { echo status=mismatch; exit 1; }; echo status=migrated ;;
  "keychain retire-legacy") echo status=retired ;;
  "keychain status")
    [ "$scenario" = shaped_class ] && { echo "status=missing class=pinned_self"; exit 1; }
    [ "$3" = acct2-password ] && [ -e "$here/added" ] && { echo "status=ok class=every_access"; exit 0; }
    if [ -e "$here/enrolled" ]; then echo "status=ok class=every_access"; else echo "status=missing class=every_access"; exit 1; fi ;;
  "keychain enroll")
    stty -echo </dev/tty; printf 'Password: ' >/dev/tty; IFS= read -r a </dev/tty
    printf 'Repeat it: ' >/dev/tty; IFS= read -r b </dev/tty; stty echo </dev/tty
    [ "$a" = "$b" ] || { echo status=mismatch; exit 1; }
    printf '%s' "$a" | shasum >"$here/enrolled"
    [ "$scenario" = shaped_status ] && { echo "status=$(echo "$a" | tr -cd 'a-z')"; echo status=ok; exit 0; }
    echo status=ok
    [ "$scenario" = trailing_failure ] && echo status=terminated
    exit 0 ;;
  "keychain probe")
    if once "probed-$3"; then
      [ "$scenario" = probe_wrong ] && { echo status=ok; exit 0; }
      echo status=denied; exit 1
    fi
    echo status=ok ;;
  "account add")
    stty -echo </dev/tty; printf 'Account login: ' >/dev/tty; IFS= read -r login </dev/tty
    printf 'Password: ' >/dev/tty; IFS= read -r a </dev/tty
    [ "$scenario" = add_killed ] && kill -9 $$
    printf 'Password again: ' >/dev/tty; IFS= read -r b </dev/tty; stty echo </dev/tty
    [ "$a" = "$b" ] || { echo status=mismatch; exit 1; }
    printf '%s' "$a" | shasum >"$here/added-password"
    printf '{"id":"acct2","provider":"%s","login":"%s","password_ref":"acct2-password","local_secret_ref":"acct2-census-key","generation":0,"identity_ref":null,"connection":"unverified"}' \
      "$3" "$login" >"$here/../../config/acct2.json"
    : >"$here/added"; echo status=created ;;
  *) echo status=usage; exit 2 ;;
esac
SH

# Fake build tools for the build phase. They record the environment names they saw.
mkdir -p "$work/tools"
cat >"$work/tools/cargo" <<SH
#!/bin/sh
env | cut -d= -f1 | sort >>"$work/tools/env-names"
case "\$1" in
  fetch) exit 0 ;;
  build) mkdir -p "\$CARGO_TARGET_DIR/release" && cp "$work/fake-mailroom" "\$CARGO_TARGET_DIR/release/mailroom" ;;
esac
SH
cat >"$work/tools/node" <<SH
#!/bin/sh
env | cut -d= -f1 | sort >>"$work/tools/env-names"
case "\$2" in ci) exit 0 ;; run) mkdir -p dist && echo ui >dist/index.html ;; esac
SH
cat >"$work/tools/xcrun" <<SH
#!/bin/sh
env | cut -d= -f1 | sort >>"$work/tools/env-names"
while [ \$# -gt 0 ]; do [ "\$1" = --scratch-path ] && out="\$2"; shift; done
mkdir -p "\$out/release" && cp "$work/fake-mailroom" "\$out/release/mailroom-confirm"
SH
echo "fake npm" >"$work/tools/npm-cli.js"
cp "$work/tools/cargo" "$work/tools/rustc"
chmod 755 "$work/tools"/*

# drive ROOT DRIVER: runs DRIVER under bash in a pty, answering the prompts in ROOT/answers.
drive() {
  touch "$1/answers"
  /usr/bin/python3 -I "$repo/scripts/drive-wizard.py" "$2" "$1/transcript" "$1/answers" && echo 0 >"$1/exit" || echo $? >"$1/exit"
}
answer() { printf '%s\t%s\n' "$2" "$3" >>"$1/answers"; }

# tree NAME: a frozen copy and its digest, owned by the test user.
tree() {
  local root="$work/$1"; rm -rf "${root:?}"; mkdir -p "$root/share" "$root/libexec" "$root/home" "$root/build" "$root/config"
  chmod 755 "$root" "$root/share" "$root/libexec" "$root/build"
  frozen="$root/share/setup-0123456789abcdef0123456789abcdef01234567"
  mkdir -p "$frozen/scripts" "$frozen/ui" "$frozen/native/macos"
  sed '$d' "$repo/scripts/setup-macos-protection.sh" >"$frozen/scripts/setup-macos-protection.sh"
  echo "approved source" >"$frozen/README"
  chmod -R go-w "$frozen"
  bash -c 'source "$1"; tree_digest "$2"' _ "$frozen/scripts/setup-macos-protection.sh" "$frozen" >"$root/digest"
  out="$root"
  answer "$out" "approval record:" "$(cat "$out/digest")"
}
overrides() {
  cat <<DRV
source "$frozen/scripts/setup-macos-protection.sh"
SHARE="$out/share"; LIBEXEC_PARENT="$out/libexec"; LIBEXEC="$out/libexec/mailroom"; BUILD_BASE="$out/build"
TRUSTED_OWNER="$me"; HOME="$out/home"; CONFIG="$out/config"; ENV_FILE=/dev/null; CARGO="$work/tools/cargo"; RUSTC="$work/tools/rustc"
NODE="$work/tools/node"; NPM_CLI="$work/tools/npm-cli.js"; XCRUN="$work/tools/xcrun"; RECORDS="$out/records"
DRV
}
run_phase() { { overrides; echo "$1"; } >"$out/driver.sh"; drive "$out" "$out/driver.sh"; }
cdhash() { codesign -d -vvv --arch "$(uname -m)" "$1" 2>&1 | sed -n 's/^CDHash=//p'; }
install_fake() {
  mkdir -m 755 "$out/libexec/mailroom"
  cp "$work/fake-mailroom" "$out/libexec/mailroom/mailroom"; cp "$work/fake.sh" "$out/libexec/mailroom/fake.sh"
  codesign -s - -f -o runtime --identifier org.mailroom.fake "$out/libexec/mailroom/mailroom" 2>/dev/null
  local hash; hash=$(cdhash "$out/libexec/mailroom/mailroom")
  printf 'MAILROOM_CONFIRM_HELPER_CDHASH=%s\nMAILROOM_CDHASH=%s\n' "$hash" "$hash" >"$out/libexec/mailroom/pins"
  echo "$1" >"$out/libexec/mailroom/scenario"; echo "$dummy" >"$out/libexec/mailroom/dummy"
  chmod -R go-w "$out/libexec/mailroom"
  printf '%s' '{"id":"acct","provider":"mailbox_org","login":"x@example.test","password_ref":"acct-password","local_secret_ref":"acct-census-key","generation":0,"identity_ref":null,"connection":"unverified"}' >"$out/config/acct.json"
}
records() { cat "$out/records" 2>/dev/null || true; }
calls() { cat "$out/libexec/mailroom/calls.log" 2>/dev/null || true; }
no_secret() { if grep -rqaF "$dummy" "$out" --exclude=driver.sh --exclude=dummy --exclude=answers; then fail "$1: dummy password appeared in output or a file"; fi; ok; }
stopped() { [[ $(cat "$out/exit") != 0 ]] && grep -qa "Setup incomplete" "$out/transcript" && ! grep -qa "Setup complete" "$out/transcript" || fail "$1 stops setup"; ok; }
completed() { [[ $(cat "$out/exit") == 0 ]] && grep -qa "Setup complete" "$out/transcript" || fail "$1 completes"; ok; }

# owner_prepare NAME [ADD [PROVIDER]]: answers ADD to "Add one local account now?" and
# PROVIDER to its provider prompt, and declines every other optional step.
owner_prepare() {
  local name="$1"; tree "$name"; install_fake "$name"
  answer "$out" "Password: " "$dummy"; answer "$out" "Repeat it: " "$dummy"; answer "$out" "for a while. [y/N]" ""
  answer "$out" "already saved. [y/N]" "${2:-}"; answer "$out" "Provider: mailbox_org or gmail" "${3:-}"
  answer "$out" "password now? [y/N]" ""
  [[ "$name" == uncommitted ]] && answer "$out" "identity key of acct? [y/N]" y
  return 0
}
owner_go() { run_phase "owner_phase \"$frozen\""; }
owner_run() { owner_prepare "$@"; owner_go; }
unpublished() { [[ ! -e "$out/config/acct2.json" && ! -e "$out/libexec/mailroom/added" ]] || fail "$1 publishes no account"; ok; }

owner_run happy; completed "owner happy path"
for step in "tree_digest match" "installation trusted" "native_approve outcome=approved" "native_cancel outcome=cancelled" \
  "account_refs status=ok" "legacy_list status=ok" "settle acct status=settled" "migrate acct-password status=migrated" \
  "enroll acct-password status=ok" "deny acct-password status=denied" "allow acct-password status=ok" "protection acct-password status=ok"; do
  grep -qx "$step" <(records) || fail "record: $step"; ok
done
[[ "$(grep -n 'settle\|migrate' <(calls) | head -1)" == *settle* ]] || fail "accounts settle before legacy migration"; ok
! grep -q "migrate acct-census-key" <(calls) || fail "keys never go through migrate"; ok
! grep -q "retire-legacy" <(calls) || fail "legacy originals kept by default"; ok
! grep -qa "stale-census-key" "$out/records" "$out/transcript" || fail "a legacy name of no saved account is never shown"; ok
grep -qa "1 legacy item(s) belong to no saved account" "$out/transcript" || fail "dropped legacy names are counted"; ok
! grep -q "^account " <(calls) || fail "a declined account setup runs no account command"; ok
no_secret happy

# Stage 3 answered yes: login and password enter at the installed binary's hidden prompts,
# never in argv, output, records or files, and later stages load the new account's references.
owner_prepare add_yes y gmail
answer "$out" "Account login: " "new@example.test"; answer "$out" "Password again: " "$dummy"
owner_go; completed "adding an account"
grep -qx "account_add status=created" <(records) || fail "record: account_add status=created"; ok
grep -qx "account add gmail" <(calls) && [[ $(grep -c "^account " <(calls)) == 1 ]] || fail "account add gets only the provider in argv"; ok
[[ "$(cat "$out/libexec/mailroom/added-password")" == "$(printf '%s' "$dummy" | shasum)" ]] || fail "the password reaches the binary through the terminal"; ok
for step in "account_refs status=ok" "settle acct2 status=settled" "password acct2-password status=ok" \
  "deny acct2-password status=denied" "allow acct2-password status=ok" "protection acct2-password status=ok"; do
  grep -qx "$step" <(records) || fail "added account record: $step"; ok
done
! grep -q "enroll acct2-password" <(calls) || fail "an added password is not entered twice"; ok
no_secret add_yes

owner_run add_wrong_answer maybe; completed "an unrecognised answer declines"
! grep -q "^account " <(calls) && ! grep -q "^account_add " <(records) || fail "an unrecognised answer adds no account"; ok; unpublished add_wrong_answer
owner_prepare add_unsupported y imap; owner_go
grep -qx "account_provider unsupported" <(records) && ! grep -qE "^(account|keychain) " <(calls) || fail "an unsupported provider stops before any command"; ok; stopped add_unsupported
owner_prepare add_mismatch y; answer "$out" "Account login: " "new@example.test"; answer "$out" "Password again: " "$dummy-other"; owner_go
grep -qx "account_add status=mismatch" <(records) && grep -qx "account add mailbox_org" <(calls) && ! grep -q "^keychain" <(calls) || fail "a mismatched password stops before the Keychain stages"; ok
unpublished add_mismatch; no_secret add_mismatch; stopped add_mismatch
owner_prepare add_killed y; answer "$out" "Account login: " "new@example.test"; owner_go
grep -qx "account_add status=terminated" <(records) && ! grep -q "^keychain" <(calls) || fail "a killed account setup stops"; ok
unpublished add_killed; no_secret add_killed; stopped add_killed
owner_prepare add_interrupted y; answer "$out" "Account login: " "new@example.test"; answer "$out" "Password again: " $'\003'; owner_go
[[ $(cat "$out/exit") != 0 ]] && grep -qa "Password again: " "$out/transcript" && ! grep -qa "Setup complete" "$out/transcript" \
  && ! grep -q "^keychain" <(calls) \
  && ! grep -q "status=created" <(records) || fail "Ctrl-C at the hidden prompt stops setup"; ok
unpublished add_interrupted; no_secret add_interrupted

owner_run probe_wrong; grep -qx "deny acct-password status=ok" <(records) || fail "a silent read is recorded and is not a denial"; ok; stopped probe_wrong
owner_run trailing_failure; grep -qx "enroll acct-password status=unexpected" <(records) || fail "a failure record after success"; ok; stopped trailing
owner_run success_exit1; grep -qx "settle acct status=unexpected" <(records) || fail "success with a failing exit"; ok; stopped success_exit1
owner_run shaped_secret; grep -qx "account_refs status=unexpected" <(records) || fail "a reference outside the metadata set"; ok; no_secret shaped_secret; stopped shaped_secret
owner_run shaped_status; grep -qx "enroll acct-password status=unexpected" <(records) || fail "a record-shaped secret status"; ok; no_secret shaped_status
owner_run shaped_class; grep -qx "password acct-password status=unexpected" <(records) || fail "a class that is not the reference's own"; ok; stopped shaped_class
owner_run mismatch; grep -qx "migrate acct-password status=mismatch" <(records) && ! grep -q retire-legacy <(calls) || fail "mismatch stops before retirement"; ok
owner_run native_denied; grep -qx "native_approve outcome=denied" <(records) && ! grep -q keychain <(calls) || fail "denied native check stops first"; ok
owner_run killed; grep -qx "settle acct status=terminated" <(records) && ! grep -q "keychain enroll" <(calls) || fail "a killed command stops setup"; ok
owner_run staged_planted; grep -qx "settle acct status=mismatch" <(records) && ! grep -q "keychain migrate" <(calls) || fail "a set that fails verification stops setup"; ok; stopped staged
owner_run uncommitted; completed uncommitted
grep -qx "settle acct status=uncommitted" <(records) && grep -qx "adopt acct status=settled" <(records) || fail "an uncommitted identity is adopted only after the owner agrees"; ok

tree wrong_digest; install_fake happy; : >"$out/answers"; answer "$out" "approval record:" 0000
run_phase "owner_phase \"$frozen\""
grep -qx "tree_digest mismatch" <(records) && [[ ! -e "$out/libexec/mailroom/calls.log" ]] || fail "a changed tree is refused"; ok

tree bad_metadata; install_fake happy; printf '%s' '{"id":"other","password_ref":"other-password","local_secret_ref":"other-census-key"}' >"$out/config/acct.json"
run_phase "owner_phase \"$frozen\""
grep -qx "account_metadata unreadable" <(records) && [[ ! -e "$out/libexec/mailroom/calls.log" ]] || fail "metadata whose id differs from its file name is refused"; ok; stopped bad_metadata

tree acl_frozen; install_fake happy; chmod +a "user:$me allow write" "$frozen/README"
run_phase "owner_phase \"$frozen\""
grep -qx "frozen_copy untrusted" <(records) || fail "a file ACL granting writes is refused"; ok

tree symlink_install; install_fake happy; chmod u+w "$out/libexec/mailroom"; ln -s /etc/hosts "$out/libexec/mailroom/extra"; chmod u-w,go-w "$out/libexec/mailroom"
run_phase "owner_phase \"$frozen\""
grep -qx "installation untrusted" <(records) && [[ ! -e "$out/libexec/mailroom/calls.log" ]] || fail "a symlink in the installation is refused"; ok

tree acl_install; install_fake happy; chmod +a "user:$me allow add_file" "$out/libexec/mailroom"
run_phase "owner_phase \"$frozen\""
grep -qx "installation untrusted" <(records) || fail "an ACL on the installation is refused"; ok; stopped acl_install

# The ACL scanner consumes the whole listing. An ACL on the first entry of a listing far
# larger than a pipe buffer, and an unreadable subdirectory, both refuse.
scan="$work/scan/tree"; mkdir -p "$scan/big"; chmod 755 "$work/scan" "$scan" "$scan/big"
: >"$scan/aaa"; chmod 644 "$scan/aaa"
for i in $(seq 1 20000); do : >"$scan/big/f$i"; done; find "$scan/big" -type f -exec chmod 644 {} +
scan_ok() { bash -c 'source "$1"; TRUSTED_OWNER="$2"; trusted_tree "$3"' _ "$tree_script" "$me" "$scan"; }
tree_script="$work/tree-script.sh"; sed '$d' "$repo/scripts/setup-macos-protection.sh" >"$tree_script"
[[ $(ls -leRA "$scan" | wc -c) -gt 1000000 ]] || fail "the scan fixture outgrows a pipe buffer"; ok
scan_ok || fail "a clean large tree is trusted"; ok
chmod +a "user:$me allow write" "$scan/aaa"
! scan_ok || fail "an early ACL in a large tree is refused"; ok
chmod -N "$scan/aaa"; mkdir -m 755 "$scan/locked"; chmod 000 "$scan/locked"
! scan_ok || fail "an unreadable directory refuses the scan"; ok
chmod 755 "$scan/locked"
bash -c 'source "$1"; has_acl "$2"' _ "$tree_script" "$work/absent" || fail "an unreadable ACL counts as an ACL"; ok

# Build phase with fake tools. A hostile variable in the caller's environment must not
# reach the build, and only the signed outputs remain.
: >"$work/tools/env-names"
tree build_happy
RUSTC_WRAPPER=/tmp/evil NODE_OPTIONS=--require=/tmp/evil run_phase "build_phase \"$frozen\""
completed "build phase"
built=$(sed -n 's/^output //p' "$out/records")
[[ -f "$built/mailroom" && -f "$built/mailroom-confirm" && -f "$built/ui/index.html" ]] || fail "signed outputs"; ok
[[ "$(ls -A "$(dirname "$built")")" == out ]] || fail "intermediates are removed"; ok
grep -qx "mailroom_cdhash $(cdhash "$built/mailroom")" <(records) && grep -qx "confirm_cdhash $(cdhash "$built/mailroom-confirm")" <(records) || fail "printed identities"; ok
grep -qa "install $built" "$out/transcript" || fail "the build prints the install command"; ok
grep -qE "^(RUSTC_WRAPPER|NODE_OPTIONS|DEVELOPER_DIR|BASH_ENV)$" "$work/tools/env-names" && fail "a hostile variable reached the build"; ok
grep -qx "CARGO_HOME" "$work/tools/env-names" || fail "builds get a fresh CARGO_HOME"; ok
mailroom_hash=$(sed -n 's/^mailroom_cdhash //p' "$out/records"); confirm_hash=$(sed -n 's/^confirm_cdhash //p' "$out/records")
ui_digest=$(sed -n 's/^ui_digest //p' "$out/records")
cp -R "$built" "$work/built"

tree outside_config; mkdir "$out/build/.cargo"; run_phase "build_phase \"$frozen\""
grep -qx "build outside_config" <(records) && [[ "$(ls -A "$out/build")" == .cargo ]] || fail "a cargo config above the build is refused and its directory removed"; ok; stopped outside_config

# Install phase on the built outputs.
install_run() {
  local name="$1"; tree "$name"; rm -rf "${out:?}/out"; cp -R "$work/built" "$out/out"
  answer "$out" "mailroom cdhash:" "${2:-$mailroom_hash}"; answer "$out" "mailroom-confirm cdhash:" "$confirm_hash"; answer "$out" "UI digest:" "$ui_digest"
}
install_go() { run_phase "install_phase \"$frozen\" \"$out/out\""; }
installed() { [[ -e "$out/libexec/mailroom" ]]; }

install_run install_happy; install_go; completed "install phase"
grep -qx "install ok" <(records) || fail "install record"; ok
bash -c 'source "$1"; TRUSTED_OWNER="$2"; trusted_tree "$3"' _ "$tree_script" "$me" "$out/libexec/mailroom" || fail "installation is a trusted tree"; ok
[[ "$(cat "$out/libexec/mailroom/pins")" == "MAILROOM_CONFIRM_HELPER_CDHASH=$confirm_hash"$'\n'"MAILROOM_CDHASH=$mailroom_hash" ]] || fail "pins"; ok
[[ "$(ls -A "$out/libexec")" == mailroom ]] || fail "no staging is left behind"; ok

install_run install_wrong 0000000000000000000000000000000000000000; install_go
grep -qx "install identity" <(records) && ! installed || fail "a copy with another cdhash is refused"; ok; stopped install_wrong
install_run install_ui; echo changed >"$out/out/ui/index.html"; install_go
grep -qx "install ui_digest" <(records) && ! installed || fail "a changed UI is refused"; ok
install_run install_symlink; rm "$out/out/mailroom"; ln -s /etc/hosts "$out/out/mailroom"; install_go
grep -qx "install not_plain" <(records) && ! installed || fail "a symlinked output is copied as a link and refused"; ok
install_run install_fifo; rm "$out/out/mailroom-confirm"; mkfifo "$out/out/mailroom-confirm"; install_go
grep -qx "install not_plain" <(records) && ! installed || fail "a FIFO output is never read and is refused"; ok; stopped install_fifo
install_run install_malformed "not-a-cdhash"; install_go
grep -qx "identities malformed" <(records) && ! installed || fail "a malformed typed identity is refused"; ok; stopped install_malformed
[[ -z "$(ls -A "$work/install_wrong/libexec")" && -z "$(ls -A "$work/install_fifo/libexec")" ]] || fail "refused staging is removed"; ok
install_run planted_pins; mkdir -m 755 "$out/libexec/mailroom"; echo keep >"$out/victim"; ln -s "$out/victim" "$out/libexec/mailroom/pins"; install_go
[[ "$(cat "$out/victim")" == keep && ! -L "$out/libexec/mailroom/pins" ]] && grep -qx "install ok" <(records) || fail "a planted pins symlink is moved aside, never followed"; ok
install_run acl_parent; chmod +a "user:$me allow add_subdirectory" "$out/libexec"; install_go
grep -qx "install untrusted_parent" <(records) && ! installed || fail "an ACL on the install parent is refused"; ok; stopped acl_parent

# The freeze bootstrap, copied from the docs and run without sudo: it accepts only the
# approved digest, ignores Git replacement objects, never reuses a destination and never
# extracts into a shared or ACL-granting directory.
block="$work/freeze.sh"
awk '/^<!-- freeze-block -->$/ {on=1; next} on && /^```sh$/ {inside=1; next} inside && /^```$/ {exit} inside {print}' \
  "$repo/docs/macos-confirmation.md" >"$block"
grep -q '/usr/bin/sudo ' "$block" && grep -q -- '--no-replace-objects' "$block" || fail "the docs hold the freeze block"; ok
head_commit=$(git -C "$repo" rev-parse HEAD)
mkdir "$work/expected"; git -C "$repo" archive --format=tar "$head_commit" | tar -x -C "$work/expected"
head_digest=$(bash -c 'source "$1"; tree_digest "$2"' _ "$tree_script" "$work/expected")
freeze() {
  local base="$1" source="$2" commit="$3" digest="$4" fault="${5:-s#^##}"
  sed -e 's#/usr/bin/sudo ##' -e "s#/usr/local/share/mailroom#$base/share/mailroom#g" -e "s#\"\$HOME/projects/mailroom\"#\"$source\"#" \
    -e "s#^COMMIT=.*#COMMIT=$commit#" -e "s#^DIGEST=.*#DIGEST=$digest#" -e "$fault" "$block" >"$base/freeze-run.sh"
  bash "$base/freeze-run.sh" >"$base/freeze.out" 2>&1
}
fresh_base() { rm -rf "${work:?}/$1"; mkdir -p "$work/$1/share"; chmod 755 "$work/$1" "$work/$1/share"; echo "$work/$1"; }
base=$(fresh_base freeze_happy); freeze "$base" "$repo" "$head_commit" "$head_digest" || fail "the approved commit freezes: $(cat "$base/freeze.out")"; ok
frozen_copy="$base/share/mailroom/setup-$head_commit"
[[ "$(bash -c 'source "$1"; tree_digest "$2"' _ "$tree_script" "$frozen_copy")" == "$head_digest" ]] || fail "the frozen copy has the approved digest"; ok
bash -c 'source "$1"; TRUSTED_OWNER="$2"; trusted_tree "$3"' _ "$tree_script" "$me" "$frozen_copy" || fail "the frozen copy is a trusted tree"; ok
! freeze "$base" "$repo" "$head_commit" "$head_digest" || fail "an existing frozen copy is never reused"; ok
base=$(fresh_base freeze_wrong); ! freeze "$base" "$repo" "$head_commit" "$(printf '0%.0s' {1..64})" || fail "another digest refuses"; ok
[[ -z "$(ls -A "$base/share/mailroom")" ]] || fail "a refused freeze leaves nothing behind"; ok
base=$(fresh_base freeze_acl); mkdir -m 755 "$base/share/mailroom"; chmod +a "user:$me allow add_subdirectory" "$base/share/mailroom"
! freeze "$base" "$repo" "$head_commit" "$head_digest" && [[ -z "$(ls -A "$base/share/mailroom")" ]] || fail "a destination with a granting ACL refuses"; ok
base=$(fresh_base freeze_link); mkdir -m 755 "$base/elsewhere"; ln -s "$base/elsewhere" "$base/share/mailroom"
! freeze "$base" "$repo" "$head_commit" "$head_digest" && [[ -z "$(ls -A "$base/elsewhere")" ]] || fail "a symlinked destination refuses"; ok
base=$(fresh_base freeze_shared); chmod 777 "$base/share"
! freeze "$base" "$repo" "$head_commit" "$head_digest" || fail "a writable ancestor refuses"; ok
# Each stand-in runs the real tool, so its output looks right, then exits 1. The block must
# refuse on the status alone and leave nothing behind.
mkdir "$work/faults"
for tool in /bin/ls /usr/bin/find /usr/bin/sort; do
  printf '#!/bin/sh\n%s "$@"\nexit 1\n' "$tool" >"$work/faults/$(basename "$tool")"
done
chmod 755 "$work/faults"/*
for fault in "ls:s#/bin/ls -lde#$work/faults/ls -lde#" \
  "find:s#(/usr/bin/find \"\$fresh\"#($work/faults/find \"\$fresh\"#" \
  "sort:s#/usr/bin/sort -z#$work/faults/sort -z#"; do
  name=${fault%%:*}; base=$(fresh_base "freeze_fault_$name")
  freeze "$base" "$repo" "$head_commit" "$head_digest" "${fault#*:}" && fail "a failed $name stage freezes"
  grep -q "$work/faults/$name" "$base/freeze-run.sh" || fail "the $name fault was injected"
  [[ -z "$(ls -A "$base/share/mailroom" 2>/dev/null)" ]] || fail "a failed $name stage leaves nothing behind"; ok
done
# A replacement object swaps the setup script under the same commit name. Git honours it
# by default; the bootstrap does not.
clone="$work/clone"; git clone -q --no-local "$repo" "$clone"; git -C "$clone" checkout -q "$head_commit"
echo "echo replaced" >>"$clone/scripts/setup-macos-protection.sh"
git -C "$clone" -c user.name=t -c user.email=t@example.invalid commit -qam replaced
git -C "$clone" replace "$head_commit" "$(git -C "$clone" rev-parse HEAD)"
replaced=$(git -C "$clone" archive --format=tar "$head_commit" | tar -xO scripts/setup-macos-protection.sh)
[[ $'\n'"$replaced"$'\n' == *$'\n''echo replaced'$'\n'* ]] \
  || fail "the replacement object fixture changes plain git archive output"; ok
base=$(fresh_base freeze_replaced); freeze "$base" "$clone" "$head_commit" "$head_digest" || fail "the bootstrap ignores replacement objects: $(cat "$base/freeze.out")"; ok
! grep -q "^echo replaced$" "$base/share/mailroom/setup-$head_commit/scripts/setup-macos-protection.sh" || fail "replaced bytes were frozen"; ok

# The driver stops the whole process group on silence and on SIGTERM, so tools a
# phase started never outlive it, even ones that ignore the hangup a closed terminal sends.
for how in silence term; do
  root="$work/driver_$how"; mkdir -p "$root"; : >"$root/answers"
  marker="$((RANDOM + 1000)).$RANDOM"
  printf 'trap "" HUP\n/bin/sleep %s &\n/bin/sleep %s\n' "$marker" "$marker" >"$root/driver.sh"
  if [[ $how == silence ]]; then
    /usr/bin/python3 -I "$repo/scripts/drive-wizard.py" "$root/driver.sh" "$root/transcript" "$root/answers" 1 || true
  else
    /usr/bin/python3 -I "$repo/scripts/drive-wizard.py" "$root/driver.sh" "$root/transcript" "$root/answers" 600 &
    driver=$!
    for _ in $(seq 50); do pgrep -f "sleep $marker" >/dev/null && break; /bin/sleep 0.1; done
    kill -TERM "$driver"; wait "$driver" || true
  fi
  for _ in $(seq 20); do pgrep -f "sleep $marker" >/dev/null || break; /bin/sleep 0.1; done
  ! pgrep -f "sleep $marker" >/dev/null || fail "a $how stop leaves a descendant running"; ok
done

# Entry checks on the real file: only the exact launch command passes.
script="$repo/scripts/setup-macos-protection.sh"
[[ "$(bash "$script" --plan | wc -l)" -eq 3 ]] || fail "plan"; ok
/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash "$script" </dev/null >/dev/null 2>&1 && fail "without -p"; ok
/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin RUSTC_WRAPPER=/tmp/evil /bin/bash -p "$script" build </dev/null >/dev/null 2>&1 && fail "with a hostile variable"; ok
/usr/bin/env -i PATH=/opt/homebrew/bin:/usr/bin:/bin /bin/bash -p "$script" </dev/null >/dev/null 2>&1 && fail "with another PATH"; ok
/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p "$script" install "$work" </dev/null >/dev/null 2>&1 && fail "install as non-root"; ok
echo "wizard runs: $passed checks passed"
