#!/usr/bin/env bash
# Drives the wizard's real stage logic in a pseudo-terminal: the one-command setup end to
# end (build with fake tools, publish into the checkout's build/local/<commit>, then the
# Keychain stages with the published fake mailroom), its reuse of a verified runtime,
# publishing on signed fake outputs, the run, keychain, maintenance and recovery phases
# with a signed fake runtime, and the ACL scanner on large and unreadable trees. Every file
# belongs to the test user. No Keychain or real build; scripts/verify-setup-real-build.sh
# runs the real build.
#
# CASES=<regex> runs only the matching sections: owner, maintenance, recovery, runtime,
# scan, setup, refusals, publish, run, driver, entry. Without it every section runs.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"; work="$(cd "$work" && pwd -P)"
# Fixtures carry ACLs, a deny-delete one included, so teardown strips them before removal.
trap '[[ -n "${KEEP:-}" ]] && echo "work=$work" || { chmod -R u+rwx "${work:?}"; chmod -R -N "${work:?}" 2>/dev/null; rm -rf "${work:?}"; }' EXIT
chmod 700 "$work"
me="$(id -un)"
dummy="nk-dummy-wizard-$RANDOM$RANDOM"
dummy_key=$(printf '%s' "$dummy" | shasum -a 256 | cut -c1-64)
passed=0
want() { [[ -z "${CASES:-}" || "$1" =~ ^($CASES)$ ]]; }
fail() { echo "FAIL: $1"; exit 1; }
ok() { passed=$((passed + 1)); }

cat >"$work/wrapper.c" <<'C'
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <stdlib.h>
int main(int argc, char **argv) {
  char script[4096]; const char *home = getenv("HOME");
  if (!home) return 126;
  snprintf(script, sizeof script, "%s/fixtures/fake.sh", home);
  char *args[argc + 2]; args[0] = "/bin/sh"; args[1] = script;
  for (int i = 1; i < argc; i++) args[i + 1] = argv[i];
  args[argc + 1] = NULL;
  execv("/bin/sh", args);
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
    case "$scenario" in
      native_denied*) echo "outcome=${scenario#native_}"; exit 1 ;;
      native_cancelled_first) echo outcome=cancelled; exit 0 ;;
      native_second_*) once asked && { echo outcome=approved; exit 0; }; echo "outcome=${scenario#native_second_}"; exit 1 ;;
    esac
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
    case "$3" in *-password-*) [ -e "$here/replaced" ] && { echo "status=ok class=every_access"; exit 0; } ;; esac
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
  "account replace-password")
    stty -echo </dev/tty; printf 'Password: ' >/dev/tty; IFS= read -r a </dev/tty
    [ "$scenario" = replace_killed ] && kill -9 $$
    printf 'Password again: ' >/dev/tty; IFS= read -r b </dev/tty; stty echo </dev/tty
    [ "$a" = "$b" ] || { echo status=mismatch; exit 1; }
    printf '%s' "$a" | shasum >"$here/replaced-password"
    sed "s/\"password_ref\":\"$3-password\"/\"password_ref\":\"$3-password-0123456789abcdef0123456789abcdef\"/" "$here/../../config/$3.json" >"$here/replaced.json"
    mv "$here/replaced.json" "$here/../../config/$3.json"; : >"$here/replaced"; echo status=replaced ;;
  "account recover-local")
    [ "$scenario" = recover_denied ] && { echo status=denied; exit 1; }
    rm -f "$here/../../config/lifecycle/$3.json"; echo status=recovered ;;
  "account rotate-key")
    [ "$scenario" = rotate_killed ] && kill -9 $$
    [ "$scenario" = rotate_shaped ] && { echo "status=$(cat "$here/dummy")"; exit 1; }
    echo status=rotated ;;
  "account forget")
    [ "$scenario" = forget_cancelled ] && { echo status=cancelled; exit 1; }
    rm -f "$here/../../config/$3.json"; echo status=forgotten ;;
  "account recovery-prepare")
    [ "$scenario" = prepare_failed ] && { echo status=storage_unavailable; exit 1; }
    mkdir -p "$here/../../config/recovery-transfer/incoming" "$here/../../config/recovery-transfer/outgoing"
    echo status=recovery_prepared ;;
  "account recovery-"*)
    stty -echo </dev/tty; printf 'Recovery key, 64 lowercase hex characters: ' >/dev/tty; IFS= read -r a </dev/tty
    [ "$scenario" = recovery_killed ] && kill -9 $$
    case "$a" in *[!0-9a-f]*) stty echo </dev/tty; echo status=invalid_input; exit 1 ;; esac
    [ ${#a} -eq 64 ] || { stty echo </dev/tty; echo status=invalid_input; exit 1; }
    b=$a
    case "$2" in recovery-capture*) printf 'Recovery key again: ' >/dev/tty; IFS= read -r b </dev/tty ;; esac
    stty echo </dev/tty
    [ "$a" = "$b" ] || { echo status=mismatch; exit 1; }
    printf '%s' "$a" | shasum >"$here/recovery-key"
    [ "$scenario" = recovery_cancelled ] && { echo status=cancelled; exit 1; }
    [ "$scenario" = recovery_auth ] && { echo status=authentication_failed; exit 1; }
    [ "$scenario" = recovery_shaped ] && { echo "status=$a"; exit 1; }
    case "$2" in recovery-capture*) echo status=captured ;; *) echo status=restored_read_only ;; esac ;;
  " ") env | cut -d= -f1 | sort >"$here/server-env"; echo "mailroom server fake"; exit 0 ;;
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
  build) printf '%s\n%s\n' "\$MAILROOM_CONFIRM_HELPER" "\$MAILROOM_UI_DIR" >"$work/tools/pin-paths"
    mkdir -p "\$CARGO_TARGET_DIR/release" && cp "$work/fake-mailroom" "\$CARGO_TARGET_DIR/release/mailroom" ;;
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

test_git() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -c user.name=t -c user.email=t@example.invalid "$@"; }
tree_script="$work/tree-script.sh"; sed '$d' "$repo/scripts/setup-macos-protection.sh" >"$tree_script"
wizard_fn() { bash -c 'source "$1"; TRUSTED_OWNER="$2"; shift 2; "$@"' _ "$tree_script" "$me" "$@"; }
# tree NAME: a git checkout owned by the test user, with a committed copy of the wizard that
# ignores /build/ as the real checkout does. The fake mailroom keeps its fixtures in
# $fx, under the test HOME.
tree() {
  local root="$work/$1"; rm -rf "${root:?}"; mkdir -p "$root/home/fixtures" "$root/build" "$root/config"
  chmod 755 "$root" "$root/build"
  checkout="$root/checkout"
  mkdir -p "$checkout/scripts" "$checkout/ui" "$checkout/native/macos"
  sed '$d' "$repo/scripts/setup-macos-protection.sh" >"$checkout/scripts/setup-macos-protection.sh"
  echo "approved source" >"$checkout/README"; echo '{}' >"$checkout/ui/package.json"; echo '// swift' >"$checkout/native/macos/Package.swift"
  echo /build/ >"$checkout/.gitignore"
  test_git -C "$checkout" init -q && test_git -C "$checkout" add -A && test_git -C "$checkout" commit -qm tree
  out="$root"; fx="$root/home/fixtures"; rt="$checkout/build/local/$(git -C "$checkout" rev-parse HEAD)"
  touch "$out/answers"
}
overrides() {
  cat <<DRV
source "$checkout/scripts/setup-macos-protection.sh"
CHECKOUT="$checkout"; BUILD_BASE="$out/build"
TRUSTED_OWNER="$me"; HOME="$out/home"; CONFIG="$out/config"; ENV_FILE=/dev/null; CARGO="$work/tools/cargo"; RUSTC="$work/tools/rustc"
NODE="$work/tools/node"; NPM_CLI="$work/tools/npm-cli.js"; XCRUN="$work/tools/xcrun"; RECORDS="$out/records"
DRV
}
run_phase() { { overrides; echo "$1"; } >"$out/driver.sh"; drive "$out" "$out/driver.sh"; }
cdhash() { codesign -d -vvv --arch "$(uname -m)" "$1" 2>&1 | sed -n 's/^CDHash=//p'; }
fixtures() { cp "$work/fake.sh" "$fx/fake.sh"; echo "$1" >"$fx/scenario"; echo "$dummy" >"$fx/dummy"; }
# fake_runtime [unhardened]: the HEAD commit's runtime with signed copies of the fake
# mailroom, a UI and matching pins, as setup publishes it.
fake_runtime() {
  mkdir -p "$checkout/build/local"; mkdir -m 755 "$rt" "$rt/ui"
  cp "$work/fake-mailroom" "$rt/mailroom"; cp "$work/fake-mailroom" "$rt/mailroom-confirm"; echo ui >"$rt/ui/index.html"
  if [[ "${1:-}" == unhardened ]]; then codesign -s - -f --identifier org.mailroom.fake "$rt/mailroom" 2>/dev/null
  else codesign -s - -f -o runtime --identifier org.mailroom.fake "$rt/mailroom" 2>/dev/null; fi
  codesign -s - -f -o runtime --identifier org.mailroom.fake-confirm "$rt/mailroom-confirm" 2>/dev/null
  printf 'COMMIT=%s\nMAILROOM_CONFIRM_HELPER_CDHASH=%s\nMAILROOM_CDHASH=%s\nUI_DIGEST=%s\n' "$(git -C "$checkout" rev-parse HEAD)" \
    "$(cdhash "$rt/mailroom-confirm")" "$(cdhash "$rt/mailroom")" "$(wizard_fn tree_digest "$rt/ui")" >"$rt/pins"
  chmod -R go-w "$rt"
}
install_fake() { fake_runtime; fixtures "$1"; save_account; }
save_account() {
  printf '%s' '{"id":"acct","provider":"mailbox_org","login":"x@example.test","password_ref":"acct-password","local_secret_ref":"acct-census-key","generation":0,"identity_ref":null,"connection":"unverified"}' >"$out/config/acct.json"
}
records() { cat "$out/records" 2>/dev/null || true; }
calls() { cat "$fx/calls.log" 2>/dev/null || true; }
no_secret() { if grep -rqaF -e "$dummy" -e "$dummy_key" "$out" --exclude=driver.sh --exclude=install-driver.sh --exclude=dummy --exclude=answers; then fail "$1: dummy password appeared in output or a file"; fi; ok; }
stopped() { [[ $(cat "$out/exit") != 0 ]] && grep -qa "Setup incomplete" "$out/transcript" && ! grep -qa "Setup complete" "$out/transcript" || fail "$1 stops setup"; ok; }
completed() { [[ $(cat "$out/exit") == 0 ]] && grep -qa "Setup complete" "$out/transcript" || fail "$1 completes"; ok; }

# owner_prepare NAME [ADD [PROVIDER [REPLACE]]]: answers ADD to "Add one local account now?",
# PROVIDER to its provider prompt and REPLACE to "Replace one saved password now?", and
# declines every other optional step.
owner_prepare() { owner_prepare_answers "$@"; install_fake "$1"; }
owner_prepare_answers() {
  local name="$1"; tree "$name"; save_account
  answer "$out" "Password: " "$dummy"; answer "$out" "Repeat it: " "$dummy"; answer "$out" "for a while. [y/N]" ""
  answer "$out" "already saved. [y/N]" "${2:-}"; answer "$out" "Provider: mailbox_org or gmail" "${3:-}"
  answer "$out" "password now? [y/N]" "${4:-}"
  [[ "$name" == uncommitted ]] && answer "$out" "identity key of acct? [y/N]" y
  return 0
}
owner_go() { run_phase owner_phase; }
owner_run() { owner_prepare "$@"; owner_go; }
unpublished() { [[ ! -e "$out/config/acct2.json" && ! -e "$fx/added" ]] || fail "$1 publishes no account"; ok; }

interrupted() { [[ $(cat "$out/exit") != 0 && -s "$out/transcript" ]] && ! grep -qa "Setup complete" "$out/transcript" || fail "$1 stops setup"; ok; }
setup_prepare() {
  owner_prepare_answers "$1"; fixtures happy
  answer "$out" "approved commit first. [y/N]" "${2-y}"
}
setup_go() { run_phase setup_phase; }
built_value() { sed -n "s/^$1 //p" "$out/records"; }
build_removed() { [[ -z "$(ls -A "$out/build")" ]] || fail "$1 removes the build directory"; ok; }
never_launched() { [[ ! -e "$fx/calls.log" ]] || fail "$1 launches no mailroom"; ok; }
not_built() { [[ ! -s "$work/tools/env-names" && -z "$(ls -A "$out/build")" && ! -e "$rt" ]] || fail "$1 builds and publishes nothing"; ok; }
again() { rm -f "$fx"/asked "$fx"/probed-* "$fx"/enrolled "$fx/calls.log"; : >"$work/tools/env-names"; }

if want owner; then
owner_run happy; completed "owner happy path"
for step in "runtime trusted" "native_approve outcome=approved" "native_cancel outcome=cancelled" \
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

# Stage 3 answered yes: login and password enter at the runtime mailroom's hidden prompts,
# never in argv, output, records or files, and later stages load the new account's references.
owner_prepare add_yes y gmail
answer "$out" "Account login: " "new@example.test"; answer "$out" "Password again: " "$dummy"
owner_go; completed "adding an account"
grep -qx "account_add status=created" <(records) || fail "record: account_add status=created"; ok
grep -qx "account add gmail" <(calls) && [[ $(grep -c "^account " <(calls)) == 1 ]] || fail "account add gets only the provider in argv"; ok
[[ "$(cat "$fx/added-password")" == "$(printf '%s' "$dummy" | shasum)" ]] || fail "the password reaches the binary through the terminal"; ok
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

# Stage 7 answered yes: the new password enters at hidden prompts, the account moves to a
# fresh password reference, and the access checks follow the reloaded reference.
new_ref=acct-password-0123456789abcdef0123456789abcdef
owner_prepare replace_yes "" "" y; answer "$out" "Account ID" acct; answer "$out" "Password again: " "$dummy"; owner_go
completed "replacing a password"
grep -qx "password_replace status=replaced" <(records) || fail "record: password_replace status=replaced"; ok
grep -qx "account replace-password acct" <(calls) && [[ $(grep -c "^account " <(calls)) == 1 ]] || fail "replace-password gets only the account in argv"; ok
[[ "$(cat "$fx/replaced-password")" == "$(printf '%s' "$dummy" | shasum)" ]] || fail "the new password reaches the binary through the terminal"; ok
for step in "deny $new_ref status=denied" "allow $new_ref status=ok" "protection $new_ref status=ok"; do
  grep -qx "$step" <(records) || fail "replaced reference record: $step"; ok
done
! grep -q "^deny acct-password " <(records) || fail "the retired reference is not checked after replacement"; ok
no_secret replace_yes
owner_prepare replace_mismatch "" "" y; answer "$out" "Account ID" acct; answer "$out" "Password again: " "$dummy-other"; owner_go
grep -qx "password_replace status=mismatch" <(records) && ! grep -q "keychain probe" <(calls) && grep -q '"password_ref":"acct-password"' "$out/config/acct.json" \
  || fail "a mismatched replacement stops and keeps the saved reference"; ok; no_secret replace_mismatch; stopped replace_mismatch
owner_prepare replace_killed "" "" y; answer "$out" "Account ID" acct; owner_go
grep -qx "password_replace status=terminated" <(records) && ! grep -q "keychain probe" <(calls) || fail "a killed replacement stops"; ok
no_secret replace_killed; stopped replace_killed
owner_prepare replace_interrupted "" "" y; answer "$out" "Account ID" acct; answer "$out" "Password again: " $'\003'; owner_go
interrupted "Ctrl-C at the replacement prompt"; grep -qa "Password again: " "$out/transcript" && ! grep -q "status=replaced" <(records) \
  && ! grep -q "keychain probe" <(calls) || fail "Ctrl-C at the replacement prompt replaces nothing"; ok; no_secret replace_interrupted
owner_prepare replace_unknown "" "" y; answer "$out" "Account ID" nobody; owner_go
grep -qx "account_selection unknown" <(records) && ! grep -q "^account " <(calls) || fail "an unknown account is refused before replacement"; ok; stopped replace_unknown

fi

if want maintenance; then
# Maintenance phase. RECONCILE ROTATE FORGET answer its three optional prompts; ID answers
# every account selection.
lifecycle() { mkdir -p "$out/config/lifecycle"; printf '{"intent":{"state":"%s"}}' "$2" >"$out/config/lifecycle/$1.json"; }
maint_prepare() {
  tree "$1"; install_fake "$1"
  answer "$out" "reconcile acct now? [y/N]" "${2:-}"; answer "$out" "local encryption key? [y/N]" "${3:-}"
  answer "$out" "forget one local account? [y/N]" "${4:-}"; answer "$out" "Account ID" "${5:-acct}"
}
maint_go() { run_phase maintenance_phase; }
no_account_command() { ! grep -q "^account " <(calls) || fail "$1 runs no account command"; ok; }

maint_prepare maint_happy y y y; lifecycle acct creating; maint_go; completed "maintenance"
for step in "recover acct status=recovered" "key_rotation status=rotated" "local_forget status=forgotten"; do
  grep -qx "$step" <(records) || fail "maintenance record: $step"; ok
done
[[ "$(grep "^account " <(calls))" == $'account recover-local acct\naccount rotate-key acct\naccount forget acct' ]] || fail "maintenance argv"; ok
[[ ! -e "$out/config/acct.json" ]] || fail "a forgotten account's metadata is gone"; ok; no_secret maint_happy
maint_prepare maint_decline; maint_go; completed "declined maintenance"; no_account_command "declined maintenance"
maint_prepare maint_forgotten; lifecycle acct forgotten; maint_go; completed "a finished forget"
! grep -qa "reconcile acct now" "$out/transcript" || fail "a finished forget is not offered for recovery"; ok; no_account_command "a finished forget"
maint_prepare maint_recover_declined n; lifecycle acct replacing; maint_go
grep -qx "recover acct declined" <(records) || fail "a declined recovery is recorded"; ok; no_account_command "a declined recovery"; stopped maint_recover_declined
maint_prepare maint_state; lifecycle acct exploded; maint_go
grep -qx "lifecycle_state unknown" <(records) || fail "an unknown lifecycle state is refused"; ok; no_account_command "an unknown state"; stopped maint_state
maint_prepare maint_lifecycle_json; mkdir "$out/config/lifecycle"; echo '{' >"$out/config/lifecycle/acct.json"; maint_go
grep -qx "lifecycle_metadata unreadable" <(records) || fail "unreadable lifecycle metadata is refused"; ok; stopped maint_lifecycle_json
maint_prepare maint_lifecycle_link; mkdir "$out/config/lifecycle"; printf '{"intent":{"state":"creating"}}' >"$out/elsewhere.json"
ln -s "$out/elsewhere.json" "$out/config/lifecycle/acct.json"; maint_go
grep -qx "lifecycle_metadata unreadable" <(records) && ! grep -q "^account " <(calls) || fail "a symlinked lifecycle file is refused"; ok; stopped maint_lifecycle_link
maint_prepare maint_lifecycle_name; lifecycle "bad.name" creating; maint_go
grep -qx "lifecycle_metadata unreadable" <(records) || fail "a lifecycle file with an invalid id is refused"; ok; stopped maint_lifecycle_name
maint_prepare recover_denied y; lifecycle acct creating; maint_go
grep -qx "recover acct status=denied" <(records) || fail "a denied recovery stops"; ok; stopped recover_denied
maint_prepare maint_metadata; printf '%s' '{"id":"other","password_ref":"other-password","local_secret_ref":"other-census-key"}' >"$out/config/acct.json"; maint_go
grep -qx "account_metadata unreadable" <(records) && ! grep -q "^account " <(calls) || fail "maintenance refuses mismatched metadata"; ok; stopped maint_metadata
maint_prepare maint_rotate_unknown "" y "" nobody; maint_go
grep -qx "account_selection unknown" <(records) || fail "an unknown rotation account is refused"; ok; no_account_command "an unknown rotation account"; stopped maint_rotate_unknown
maint_prepare rotate_shaped "" y; maint_go
grep -qx "key_rotation status=unexpected" <(records) || fail "a rotation reply outside the known set"; ok; no_secret rotate_shaped; stopped rotate_shaped
maint_prepare rotate_killed "" y; maint_go
grep -qx "key_rotation status=terminated" <(records) || fail "a killed rotation stops"; ok; stopped rotate_killed
maint_prepare maint_rotate_interrupted "" y "" $'\003'; maint_go
interrupted "Ctrl-C at the rotation account prompt"; no_account_command "Ctrl-C at the rotation account prompt"
maint_prepare forget_cancelled "" "" y; maint_go
grep -qx "local_forget status=cancelled" <(records) && [[ -e "$out/config/acct.json" ]] || fail "a cancelled forget stops and keeps the account"; ok; stopped forget_cancelled
maint_prepare maint_forget_unknown "" "" y nobody; maint_go
grep -qx "account_selection unknown" <(records) || fail "an unknown forget account is refused"; ok; no_account_command "an unknown forget account"; stopped maint_forget_unknown
tree maint_install; install_fake happy; ln -s /etc/hosts "$rt/extra"; maint_go
grep -qx "runtime untrusted" <(records) && [[ ! -e "$fx/calls.log" ]] || fail "maintenance refuses an untrusted runtime"; ok; stopped maint_install
tree maint_missing; fixtures happy; save_account; maint_go
grep -qx "runtime missing" <(records) && grep -qa "Run scripts/setup.sh first" "$out/transcript" && [[ ! -e "$fx/calls.log" ]] \
  || fail "maintenance without a runtime stops and names setup"; ok; stopped maint_missing

fi

if want recovery; then
# Recovery phase. The recovery key is a dummy 64-hex value entered only at the binary's
# hidden prompts. rec_prepare NAME RUN KIND CHOSEN LEDGER KEY_READY CUSTODY BUNDLE KEY KEY_AGAIN.
bundle=$(printf 'ab%.0s' {1..32})
rec_prepare() {
  tree "$1"; install_fake "$1"
  answer "$out" "recovery action now? [y/N]" "${2:-}"; answer "$out" "Choose capture, restore or resume" "${3:-}"
  answer "$out" "separated by spaces" "${4-acct}"; answer "$out" "address ledger in this bundle? [y/N]" "${5:-}"
  answer "$out" "identical target? [y/N]" "${5:-}"; answer "$out" "available for this action? [y/N]" "${6:-y}"
  answer "$out" "separately from its key? [y/N]" "${7:-y}"; answer "$out" "without .bundle" "${8:-$bundle}"
  answer "$out" "64 lowercase hex characters: " "${9:-$dummy_key}"; answer "$out" "Recovery key again: " "${10:-$dummy_key}"
}
rec_go() { run_phase recovery_phase; }
key_entered() { [[ "$(cat "$fx/recovery-key")" == "$(printf '%s' "$dummy_key" | shasum)" ]] || fail "$1: the key reaches the binary through the terminal"; ok; }
rec_argv() { [[ "$(grep "^account " <(calls))" == "account recovery-prepare"$'\n'"$2" ]] || fail "$1 argv"; ok; }

rec_prepare rec_decline; rec_go; completed "declined recovery"; no_account_command "declined recovery"
rec_prepare rec_capture y; rec_go; completed "capture"
grep -qx "recovery_directories status=recovery_prepared" <(records) && grep -qx "recovery status=captured" <(records) \
  && grep -qx "bundle_custody owner_confirmed" <(records) || fail "capture records"; ok
rec_argv capture "account recovery-capture acct"; key_entered capture; no_secret rec_capture
rec_prepare rec_capture_ledger y "" acct y y n; rec_go
rec_argv "capture with the ledger" "account recovery-capture-with-ledger acct"; grep -qx "recovery status=captured" <(records) || fail "capture with the ledger"; ok
grep -qx "bundle_custody unconfirmed" <(records) || fail "unconfirmed custody is recorded"; ok; stopped "unconfirmed custody"
no_secret rec_capture_ledger
rec_prepare rec_restore y restore; rec_go; completed "restore"
grep -qx "recovery status=restored_read_only" <(records) || fail "restore record"; ok; rec_argv restore "account recovery-restore $bundle"; key_entered restore; no_secret rec_restore
rec_prepare rec_restore_ledger y restore "" y; rec_go; completed "restore with the ledger"; rec_argv "restore with the ledger" "account recovery-restore-with-ledger $bundle"
rec_prepare rec_resume y resume; rec_go; completed "resume"
grep -qx "recovery status=restored_read_only" <(records) || fail "resume record"; ok; rec_argv resume "account recovery-resume"; key_entered resume; no_secret rec_resume
rec_prepare prepare_failed y; rec_go
grep -qx "recovery_directories status=storage_unavailable" <(records) && [[ "$(grep "^account " <(calls))" == "account recovery-prepare" ]] || fail "a failed prepare stops"; ok; stopped prepare_failed
rec_prepare rec_action y delete; rec_go
grep -qx "recovery_action unknown" <(records) && [[ "$(grep "^account " <(calls))" == "account recovery-prepare" ]] || fail "an unknown action is refused"; ok; stopped rec_action
n=0
for chosen in "" nobody "acct acct" "acct acct acct acct acct acct acct acct acct" "ac/ct"; do
  n=$((n + 1)); rec_prepare "rec_selection_$n" y capture "$chosen"; rec_go
  grep -qx "capture_selection invalid" <(records) && ! grep -q "recovery-capture" <(calls) || fail "capture selection '$chosen' is refused"; ok; stopped "rec_selection_$n"
done
for value in xyz "$(printf 'AB%.0s' {1..32})"; do
  rec_prepare "rec_bundle_${#value}" y restore "" "" y y "$value"; rec_go
  grep -qx "bundle_selection invalid" <(records) && ! grep -q "recovery-restore" <(calls) || fail "bundle '$value' is refused"; ok; stopped "rec_bundle_${#value}"
done
rec_prepare rec_metadata y; printf '%s' '{"id":"other"}' >"$out/config/acct.json"; rec_go
grep -qx "account_metadata unreadable" <(records) && ! grep -q "recovery-capture" <(calls) || fail "capture refuses mismatched metadata"; ok; stopped rec_metadata
rec_prepare rec_no_key y "" acct "" n; rec_go
grep -qx "recovery_key unavailable" <(records) && ! grep -q "^recovery " <(records) && [[ "$(grep "^account " <(calls))" == "account recovery-prepare" ]] || fail "a missing recovery key stops before the action"; ok; stopped rec_no_key
rec_prepare rec_mismatch y "" acct "" y y "" "$dummy_key" "$(printf '0%.0s' {1..64})"; rec_go
grep -qx "recovery status=mismatch" <(records) || fail "a mismatched key stops"; ok; no_secret rec_mismatch; stopped rec_mismatch
rec_prepare rec_invalid y resume "" "" y y "" "$dummy"; rec_go
grep -qx "recovery status=invalid_input" <(records) || fail "a key that is not 64 hex characters stops"; ok; no_secret rec_invalid; stopped rec_invalid
rec_prepare recovery_cancelled y; rec_go
grep -qx "recovery status=cancelled" <(records) || fail "a cancelled native review stops"; ok; no_secret recovery_cancelled; stopped recovery_cancelled
rec_prepare recovery_auth y restore; rec_go
grep -qx "recovery status=authentication_failed" <(records) || fail "a bundle that fails authentication stops"; ok; no_secret recovery_auth; stopped recovery_auth
rec_prepare recovery_killed y; rec_go
grep -qx "recovery status=terminated" <(records) || fail "a killed recovery stops"; ok; no_secret recovery_killed; stopped recovery_killed
rec_prepare recovery_shaped y resume; rec_go
grep -qx "recovery status=unexpected" <(records) || fail "a recovery reply that echoes the key"; ok; no_secret recovery_shaped; stopped recovery_shaped
rec_prepare rec_interrupted y "" acct "" y y "" "$dummy_key" $'\003'; rec_go
interrupted "Ctrl-C at the recovery key prompt"; grep -qa "Recovery key again: " "$out/transcript" && ! grep -q "status=captured" <(records) \
  || fail "Ctrl-C at the recovery key prompt captures nothing"; ok; no_secret rec_interrupted

fi

if want owner; then
owner_run probe_wrong; grep -qx "deny acct-password status=ok" <(records) || fail "a silent read is recorded and is not a denial"; ok; stopped probe_wrong
owner_run trailing_failure; grep -qx "enroll acct-password status=unexpected" <(records) || fail "a failure record after success"; ok; stopped trailing
owner_run success_exit1; grep -qx "settle acct status=unexpected" <(records) || fail "success with a failing exit"; ok; stopped success_exit1
owner_run shaped_secret; grep -qx "account_refs status=unexpected" <(records) || fail "a reference outside the metadata set"; ok; no_secret shaped_secret; stopped shaped_secret
owner_run shaped_status; grep -qx "enroll acct-password status=unexpected" <(records) || fail "a record-shaped secret status"; ok; no_secret shaped_status
owner_run shaped_class; grep -qx "password acct-password status=unexpected" <(records) || fail "a class that is not the reference's own"; ok; stopped shaped_class
owner_run mismatch; grep -qx "migrate acct-password status=mismatch" <(records) && ! grep -q retire-legacy <(calls) || fail "mismatch stops before retirement"; ok
owner_run native_denied; grep -qx "native_approve outcome=denied" <(records) && ! grep -q keychain <(calls) || fail "denied native check stops first"; ok
untouched() { ! grep -q -e keychain -e '^account ' <(calls) || fail "$1 runs no keychain or account command"; ok; }
for code in failed timeout expired malformed unauthenticated channel oversized launch other; do
  owner_run "native_denied_$code"; grep -qx "native_approve outcome=denied_$code" <(records) || fail "native check denied_$code is recorded"; ok
  untouched "denied_$code"; stopped "denied_$code"
done
owner_run native_denied_bogus; grep -qx "native_approve status=unexpected" <(records) || fail "an unknown denial code is unexpected"; ok
untouched "an unknown denial code"; stopped "an unknown denial code"
owner_run native_cancelled_first; grep -qx "native_approve outcome=cancelled" <(records) || fail "a cancel at the approve step is recorded"; ok
untouched "a cancel at the approve step"; stopped "a cancel at the approve step"
owner_run native_second_denied_timeout; grep -qx "native_cancel outcome=denied_timeout" <(records) || fail "a denial at the cancel step is recorded"; ok
untouched "a denial at the cancel step"; stopped "a denial at the cancel step"
owner_run killed; grep -qx "settle acct status=terminated" <(records) && ! grep -q "keychain enroll" <(calls) || fail "a killed command stops setup"; ok
owner_run staged_planted; grep -qx "settle acct status=mismatch" <(records) && ! grep -q "keychain migrate" <(calls) || fail "a set that fails verification stops setup"; ok; stopped staged
owner_run uncommitted; completed uncommitted
grep -qx "settle acct status=uncommitted" <(records) && grep -qx "adopt acct status=settled" <(records) || fail "an uncommitted identity is adopted only after the owner agrees"; ok

tree bad_metadata; install_fake happy; printf '%s' '{"id":"other","password_ref":"other-password","local_secret_ref":"other-census-key"}' >"$out/config/acct.json"
run_phase owner_phase
grep -qx "account_metadata unreadable" <(records) && [[ ! -e "$fx/calls.log" ]] || fail "metadata whose id differs from its file name is refused"; ok; stopped bad_metadata

fi

if want runtime; then
tree symlink_install; install_fake happy; ln -s /etc/hosts "$rt/extra"
run_phase owner_phase
grep -qx "runtime untrusted" <(records) && [[ ! -e "$fx/calls.log" ]] || fail "a symlink in the runtime is refused"; ok
grep -qa "Delete $rt" "$out/transcript" || fail "an untrusted runtime names the directory to delete"; ok; stopped symlink_install

tree acl_install; install_fake happy; chmod +a "user:$me allow add_file" "$rt"
run_phase owner_phase
grep -qx "runtime untrusted" <(records) || fail "a granting ACL on the runtime is refused"; ok; stopped acl_install
owner_prepare deny_acl; chmod +a "group:everyone deny delete" "$rt"
run_phase owner_phase; completed "a deny-only ACL, as every home folder carries,"
grep -qx "runtime trusted" <(records) || fail "a deny-only ACL leaves the runtime trusted"; ok; chmod -N "$rt"
tree writable_runtime; install_fake happy; chmod g+w "$rt/ui/index.html"
run_phase owner_phase; grep -qx "runtime untrusted" <(records) || fail "a group-writable runtime file is refused"; ok; stopped writable_runtime
tree unhardened; fake_runtime unhardened; fixtures happy; save_account
run_phase owner_phase; grep -qx "runtime untrusted" <(records) && [[ ! -e "$fx/calls.log" ]] || fail "a mailroom without the hardened runtime is refused"; ok
tree resigned; install_fake happy; codesign -s - -f -o runtime --identifier org.mailroom.other "$rt/mailroom" 2>/dev/null
run_phase owner_phase; grep -qx "runtime untrusted" <(records) && [[ ! -e "$fx/calls.log" ]] || fail "a binary whose cdhash differs from the pins is refused"; ok
tree changed_ui; install_fake happy; echo changed >"$rt/ui/index.html"
run_phase owner_phase; grep -qx "runtime untrusted" <(records) || fail "a UI that differs from the pins is refused"; ok
tree moved_runtime; install_fake happy; echo next >"$checkout/README"; test_git -C "$checkout" commit -qam next
mv "$rt" "$checkout/build/local/$(git -C "$checkout" rev-parse HEAD)"
run_phase owner_phase; grep -qx "runtime untrusted" <(records) || fail "a runtime moved to another commit's name is refused"; ok
tree other_commit; install_fake happy; echo next >"$checkout/README"; test_git -C "$checkout" commit -qam next
run_phase owner_phase; grep -qx "runtime missing" <(records) && [[ -e "$rt/pins" ]] || fail "a new HEAD commit needs its own runtime and leaves the old one"; ok

fi

if want scan; then
# The ACL scanner consumes the whole listing. An ACL on the first entry of a listing far
# larger than a pipe buffer, and an unreadable subdirectory, both refuse.
scan="$work/scan/tree"; mkdir -p "$scan/big"; chmod 755 "$work/scan" "$scan" "$scan/big"
: >"$scan/aaa"; chmod 644 "$scan/aaa"
for i in $(seq 1 20000); do : >"$scan/big/f$i"; done; find "$scan/big" -type f -exec chmod 644 {} +
scan_ok() { wizard_fn trusted_tree "$scan"; }
[[ $(ls -leRA "$scan" | wc -c) -gt 1000000 ]] || fail "the scan fixture outgrows a pipe buffer"; ok
scan_ok || fail "a clean large tree is trusted"; ok
chmod +a "user:$me allow write" "$scan/aaa"
! scan_ok || fail "an early ACL in a large tree is refused"; ok
chmod -N "$scan/aaa"; chmod +a "group:everyone deny delete" "$scan/aaa"
scan_ok || fail "a deny-only ACL in a large tree is trusted"; ok
chmod -N "$scan/aaa"; mkdir -m 755 "$scan/locked"; chmod 000 "$scan/locked"
! scan_ok || fail "an unreadable directory refuses the scan"; ok
chmod 755 "$scan/locked"
bash -c 'source "$1"; has_acl "$2"' _ "$tree_script" "$work/absent" || fail "an unreadable ACL counts as an ACL"; ok

fi

if want setup; then
# One-command setup with fake tools. A hostile variable in the caller's environment must not
# reach the build. setup_prepare NAME [USE_COMMIT]: a checkout, a saved account, the fake
# mailroom's fixtures and the owner phase's answers. USE_COMMIT answers "Use this commit?"
# and defaults to y.

: >"$work/tools/env-names"
setup_prepare setup_happy
RUSTC_WRAPPER=/tmp/evil NODE_OPTIONS=--require=/tmp/evil setup_go; completed "one-command setup"
for step in "checkout clean" "use_commit confirmed" "build ok" "publish ok" "runtime trusted" "native_approve outcome=approved" "settle acct status=settled" \
  "migrate acct-password status=migrated" "enroll acct-password status=ok" "allow acct-password status=ok" "protection acct-password status=ok"; do
  grep -qx "$step" <(records) || fail "setup record: $step"; ok
done
mailroom_hash=$(built_value mailroom_cdhash); confirm_hash=$(built_value confirm_cdhash); ui_digest=$(built_value ui_digest)
head=$(git -C "$checkout" rev-parse HEAD)
[[ "$(built_value commit)" == "$head" ]] || fail "the build records the checkout's HEAD commit"; ok
grep -qa "  $head" "$out/transcript" || fail "the full HEAD commit is shown before the build"; ok
[[ "$rt" == "$checkout/build/local/$head" && -d "$rt" && ! -L "$rt" ]] || fail "the runtime is the checkout's build/local/<commit>"; ok
[[ "$(cat "$work/tools/pin-paths")" == "$rt/mailroom-confirm"$'\n'"$rt/ui" ]] || fail "mailroom is compiled against the runtime's helper and UI paths"; ok
[[ "$(cat "$rt/pins")" == "COMMIT=$head"$'\n'"MAILROOM_CONFIRM_HELPER_CDHASH=$confirm_hash"$'\n'"MAILROOM_CDHASH=$mailroom_hash"$'\n'"UI_DIGEST=$ui_digest" ]] || fail "setup pins"; ok
[[ "$mailroom_hash" == "$(cdhash "$rt/mailroom")" && "$confirm_hash" == "$(cdhash "$rt/mailroom-confirm")" ]] || fail "the runtime holds the binaries the build signed"; ok
[[ "$(stat -f %Su "$rt" "$rt/mailroom" "$rt/pins" | sort -u)" == "$me" ]] || fail "the runtime belongs to the owner"; ok
[[ "$(ls -A "$checkout/build/local")" == "$head" ]] || fail "no staging or alias is left beside the runtime"; ok
[[ -z "$(git -C "$checkout" status --porcelain --untracked-files=all)" ]] || fail "the ignored runtime leaves the checkout clean"; ok
build_removed "setup"
grep -qE "^(RUSTC_WRAPPER|NODE_OPTIONS|DEVELOPER_DIR|BASH_ENV)$" "$work/tools/env-names" && fail "a hostile variable reached the build"; ok
grep -qx "CARGO_HOME" "$work/tools/env-names" || fail "builds get a fresh CARGO_HOME"; ok
[[ $(grep -c "Ready to start?" "$out/transcript") == 1 ]] || fail "one run asks to start once"; ok
no_secret setup_happy
mkdir -p "$work/built"; cp -R "$rt/mailroom" "$rt/mailroom-confirm" "$rt/ui" "$work/built/"

# A second run reuses the verified runtime and builds nothing.
again; setup_go; completed "setup with a verified runtime"
grep -qx "runtime reused" <(records) && ! grep -q "^build " <(records) && grep -qx "runtime trusted" <(records) || fail "a verified runtime is reused"; ok
[[ ! -s "$work/tools/env-names" && "$(cat "$rt/pins")" == "COMMIT=$head"* ]] || fail "reuse runs no build tool and keeps the runtime"; ok
# A runtime changed after publishing is refused, never rebuilt over.
again; echo changed >"$rt/ui/index.html"; setup_go
grep -qx "runtime untrusted" <(records) && [[ ! -s "$work/tools/env-names" && "$(cat "$rt/ui/index.html")" == changed ]] \
  || fail "setup refuses a changed runtime and builds nothing"; ok; never_launched "a changed runtime"; stopped "a changed runtime"
# A runtime re-signed after the build, with consistent pins, still differs from what this run built.
setup_prepare setup_resigned
{ overrides; cat <<DRV
TOTAL_STAGES=9
build_stage || exit 1
codesign -s - -f -o runtime --identifier org.mailroom.other "\$RUNTIME/mailroom" 2>/dev/null
sed -i '' "s/^MAILROOM_CDHASH=.*/MAILROOM_CDHASH=\$(cdhash_of "\$RUNTIME/mailroom")/" "\$RUNTIME/pins"
owner_stages "\$MAILROOM_CDHASH" "\$CONFIRM_CDHASH"
DRV
} >"$out/driver.sh"; drive "$out" "$out/driver.sh"
grep -qx "publish ok" <(records) && grep -qx "runtime untrusted" <(records) || fail "pins that differ from the build are refused"; ok
never_launched "pins that differ from the build"

fi

if want refusals; then
setup_prepare setup_dirty_tracked; echo "# edit" >>"$checkout/scripts/setup-macos-protection.sh"; : >"$work/tools/env-names"; setup_go
grep -qx "checkout changed" <(records) && ! grep -qa "Use this commit" "$out/transcript" || fail "an uncommitted script change is refused before the confirmation"; ok
not_built "a changed checkout"; stopped setup_dirty_tracked
setup_prepare setup_dirty_untracked; mkdir -p "$checkout/src"; echo new >"$checkout/src/new.rs"; : >"$work/tools/env-names"; setup_go
grep -qx "checkout changed" <(records) || fail "an untracked file the build could read is refused"; ok; not_built "an untracked source"; stopped setup_dirty_untracked
setup_prepare setup_docs_changed; mkdir -p "$checkout/docs"; echo note >"$checkout/docs/n.md"; echo note >"$checkout/NOTES.md"; : >"$checkout/.DS_Store"
setup_go; completed "changes outside the build set"; grep -qx "checkout clean" <(records) || fail "docs, top-level Markdown and .DS_Store do not block"; ok
setup_prepare setup_declined ""; : >"$work/tools/env-names"; setup_go
grep -qx "use_commit declined" <(records) || fail "a declined commit is recorded"; ok; not_built "a declined commit"; stopped setup_declined
setup_prepare outside_config; mkdir "$out/build/.cargo"; setup_go
grep -qx "build outside_config" <(records) && [[ "$(ls -A "$out/build")" == .cargo && ! -e "$rt" ]] \
  || fail "a cargo config above the build is refused and its directory removed"; ok; stopped outside_config
setup_prepare setup_no_commit; rm -rf "$checkout/.git"; : >"$work/tools/env-names"; setup_go
grep -qx "build source" <(records) || fail "a checkout without a commit is refused"; ok; not_built "a refused source"; stopped setup_no_commit
setup_prepare setup_linked_build; mkdir -p "$work/elsewhere" "$checkout/build"; ln -s "$work/elsewhere" "$checkout/build/local"; setup_go
grep -qx "publish untrusted_parent" <(records) && [[ -z "$(ls -A "$work/elsewhere")" ]] \
  || fail "a symlinked build/local receives no runtime"; ok; build_removed "a refused publish"; stopped setup_linked_build

fi

if want publish; then
# Publishing the built outputs, which setup does after the build. Without the setup section,
# signed copies of the fake mailroom stand in for its outputs.
if [[ ! -d "$work/built" ]]; then
  mkdir -p "$work/built/ui"; cp "$work/fake-mailroom" "$work/built/mailroom"; cp "$work/fake-mailroom" "$work/built/mailroom-confirm"
  echo ui >"$work/built/ui/index.html"
  codesign -s - -f -o runtime --identifier org.mailroom "$work/built/mailroom" 2>/dev/null
  codesign -s - -f -o runtime --identifier org.mailroom.confirm "$work/built/mailroom-confirm" 2>/dev/null
  mailroom_hash=$(cdhash "$work/built/mailroom"); confirm_hash=$(cdhash "$work/built/mailroom-confirm"); ui_digest=$(wizard_fn tree_digest "$work/built/ui")
fi
publish_run() { tree "$1"; rm -rf "${out:?}/out"; cp -R "$work/built" "$out/out"; typed_mailroom="${2:-$mailroom_hash}"; }
publish_go() {
  run_phase "head_runtime; MAILROOM_CDHASH=\"$typed_mailroom\"; CONFIRM_CDHASH=\"$confirm_hash\"; UI_DIGEST=\"$ui_digest\"
PUBLISH_FAILURE=failed; publish_runtime \"$out/out\" && PUBLISH_FAILURE=ok; echo \"publish=\$PUBLISH_FAILURE\"; [[ \$PUBLISH_FAILURE == ok ]]"
}
refused() { [[ $(cat "$out/exit") != 0 ]] && grep -qa "publish=$1" "$out/transcript" && [[ -z "$(ls -A "$checkout/build/local" 2>/dev/null)" ]] || fail "$2"; ok; }

publish_run publish_happy; publish_go
[[ $(cat "$out/exit") == 0 ]] && grep -qa "publish=ok" "$out/transcript" || fail "publishing the built outputs"; ok
wizard_fn trusted_tree "$rt" || fail "the runtime is a trusted tree"; ok
bash -c 'source "$1"; TRUSTED_OWNER="$2"; CHECKOUT="$3"; head_runtime && verify_runtime' _ "$tree_script" "$me" "$checkout" || fail "the published runtime verifies"; ok
[[ "$(ls -A "$checkout/build/local")" == "$(basename "$rt")" ]] || fail "no staging is left behind"; ok
publish_run publish_wrong 0000000000000000000000000000000000000000; publish_go; refused identity "a copy with another cdhash is refused"
publish_run publish_ui; echo changed >"$out/out/ui/index.html"; publish_go; refused ui_digest "a changed UI is refused"
publish_run publish_symlink; rm "$out/out/mailroom"; ln -s /etc/hosts "$out/out/mailroom"; publish_go; refused not_plain "a symlinked output is copied as a link and refused"
publish_run publish_fifo; rm "$out/out/mailroom-confirm"; mkfifo "$out/out/mailroom-confirm"; publish_go; refused not_plain "a FIFO output is never read and is refused"
publish_run publish_exists; mkdir -p "$rt"; echo keep >"$rt/pins"; publish_go
grep -qa "publish=exists" "$out/transcript" && [[ "$(cat "$rt/pins")" == keep && "$(ls -A "$checkout/build/local")" == "$(basename "$rt")" ]] \
  || fail "an existing runtime is never written into or replaced"; ok
publish_run publish_acl; mkdir -p "$checkout/build/local"; chmod +a "user:$me allow add_subdirectory" "$checkout/build/local"; publish_go
refused untrusted_parent "a granting ACL on build/local is refused"

fi

if want run; then
# The run subcommand starts the verified runtime's server with a cleared environment and no
# prompt, and refuses a missing or untrusted runtime.
tree serve_happy; install_fake happy; RUSTC_WRAPPER=/tmp/evil run_phase serve_phase
[[ $(cat "$out/exit") == 0 ]] && grep -qa "mailroom server fake" "$out/transcript" && grep -qx "runtime trusted" <(records) || fail "run starts the runtime's server"; ok
[[ "$(calls)" == "" && $(wc -l <"$fx/calls.log") -eq 1 ]] || fail "the server gets no argument"; ok
grep -qx HOME "$fx/server-env" && ! grep -qx RUSTC_WRAPPER "$fx/server-env" || fail "the server gets a cleared environment with HOME"; ok
! grep -qa "Ready to start?" "$out/transcript" && grep -qa "Commit $(git -C "$checkout" rev-parse HEAD)" "$out/transcript" \
  || fail "run asks nothing and names the commit"; ok
tree serve_missing; fixtures happy; run_phase serve_phase
grep -qx "runtime missing" <(records) && grep -qa "Run scripts/setup.sh first" "$out/transcript" && [[ ! -e "$checkout/build" ]] || fail "run without a runtime builds nothing and names setup"; ok
never_launched "run without a runtime"; stopped serve_missing
tree serve_resigned; install_fake happy; codesign -s - -f -o runtime --identifier org.mailroom.other "$rt/mailroom" 2>/dev/null; run_phase serve_phase
grep -qx "runtime untrusted" <(records) || fail "run refuses a re-signed mailroom"; ok; never_launched "run with a re-signed mailroom"; stopped serve_resigned

fi

if want driver; then
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

fi

if want entry; then
# Entry checks on the real file: only the exact launch command passes.
script="$repo/scripts/setup-macos-protection.sh"
[[ "$(bash "$script" --plan | wc -l)" -eq 4 ]] || fail "plan"; ok
! grep -qE 'sudo|/usr/local/libexec|EUID -eq 0' "$script" "$repo/scripts/setup.sh" || fail "setup has no root phase"; ok
/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash "$script" </dev/null >/dev/null 2>&1 && fail "without -p"; ok
/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin RUSTC_WRAPPER=/tmp/evil /bin/bash -p "$script" run </dev/null >/dev/null 2>&1 && fail "with a hostile variable"; ok
/usr/bin/env -i PATH=/opt/homebrew/bin:/usr/bin:/bin /bin/bash -p "$script" </dev/null >/dev/null 2>&1 && fail "with another PATH"; ok
entry=$(/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p "$script" install "$work" </dev/null 2>&1) && fail "the removed install phase"
[[ "$entry" == "Run scripts/setup.sh in a terminal"* ]] || fail "the removed install phase is refused"; ok
# The launcher clears the caller's environment, so the wizard refuses only for the missing
# terminal, and bash -p ignores BASH_ENV in the launcher itself.
printf 'touch "%s/bash-env-ran"\n' "$work" >"$work/bash-env.sh"
entry=$(BASH_ENV="$work/bash-env.sh" RUSTC_WRAPPER=/tmp/evil "$repo/scripts/setup.sh" </dev/null 2>&1) && fail "the launcher without a terminal"
[[ "$entry" == "Run scripts/setup.sh in a terminal"* && ! -e "$work/bash-env-ran" ]] || fail "the launcher clears the environment"; ok
entry=$("$repo/scripts/setup.sh" run </dev/null 2>&1) && fail "run without a terminal"
[[ "$entry" == "Run scripts/setup.sh in a terminal"* ]] || fail "run without a terminal is refused"; ok
[[ "$(cd "$repo/scripts" && ./setup.sh --plan | wc -l)" -eq 4 ]] || fail "the launcher finds the wizard beside it"; ok
fi
echo "wizard runs: $passed checks passed${CASES:+ in sections $CASES}"
