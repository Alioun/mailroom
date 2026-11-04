#!/usr/bin/env bash
# Drives the wizard's real stage logic in a pseudo-terminal: the one-command setup end to
# end (build with fake tools, a fake sudo that runs the install phase as the test user, then
# the Keychain stages with the installed fake mailroom), the install phase on signed fake
# outputs, the keychain, maintenance and recovery phases with a signed fake mailroom, and
# the ACL scanner on large and unreadable trees. A temporary tree owned by the test user
# stands in for the root-owned locations. No sudo, Keychain or real build;
# scripts/verify-setup-real-build.sh runs the real build.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"; work="$(cd "$work" && pwd -P)"
trap '[[ -n "${KEEP:-}" ]] && echo "work=$work" || { chmod -R u+rwx "${work:?}"; rm -rf "${work:?}"; }' EXIT
chmod 700 "$work"
me="$(id -un)"
dummy="nk-dummy-wizard-$RANDOM$RANDOM"
dummy_key=$(printf '%s' "$dummy" | shasum -a 256 | cut -c1-64)
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

test_git() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -c user.name=t -c user.email=t@example.invalid "$@"; }
# tree NAME: a git checkout owned by the test user, with a committed copy of the wizard.
tree() {
  local root="$work/$1"; rm -rf "${root:?}"; mkdir -p "$root/libexec" "$root/home" "$root/build" "$root/config"
  chmod 755 "$root" "$root/libexec" "$root/build"
  checkout="$root/checkout"
  mkdir -p "$checkout/scripts" "$checkout/ui" "$checkout/native/macos"
  sed '$d' "$repo/scripts/setup-macos-protection.sh" >"$checkout/scripts/setup-macos-protection.sh"
  echo "approved source" >"$checkout/README"; echo '{}' >"$checkout/ui/package.json"; echo '// swift' >"$checkout/native/macos/Package.swift"
  test_git -C "$checkout" init -q && test_git -C "$checkout" add -A && test_git -C "$checkout" commit -qm tree
  out="$root"
  touch "$out/answers"
}
overrides() {
  cat <<DRV
source "$checkout/scripts/setup-macos-protection.sh"
CHECKOUT="$checkout"; SCRIPT="$checkout/scripts/setup-macos-protection.sh"; SUDO="$work/fake-sudo"
LIBEXEC_PARENT="$out/libexec"; LIBEXEC="$out/libexec/mailroom"; BUILD_BASE="$out/build"
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
  save_account
}
save_account() {
  printf '%s' '{"id":"acct","provider":"mailbox_org","login":"x@example.test","password_ref":"acct-password","local_secret_ref":"acct-census-key","generation":0,"identity_ref":null,"connection":"unverified"}' >"$out/config/acct.json"
}
records() { cat "$out/records" 2>/dev/null || true; }
calls() { cat "$out/libexec/mailroom/calls.log" 2>/dev/null || true; }
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
unpublished() { [[ ! -e "$out/config/acct2.json" && ! -e "$out/libexec/mailroom/added" ]] || fail "$1 publishes no account"; ok; }

owner_run happy; completed "owner happy path"
for step in "installation trusted" "native_approve outcome=approved" "native_cancel outcome=cancelled" \
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
interrupted() { [[ $(cat "$out/exit") != 0 && -s "$out/transcript" ]] && ! grep -qa "Setup complete" "$out/transcript" || fail "$1 stops setup"; ok; }

# Stage 7 answered yes: the new password enters at hidden prompts, the account moves to a
# fresh password reference, and the access checks follow the reloaded reference.
new_ref=acct-password-0123456789abcdef0123456789abcdef
owner_prepare replace_yes "" "" y; answer "$out" "Account ID" acct; answer "$out" "Password again: " "$dummy"; owner_go
completed "replacing a password"
grep -qx "password_replace status=replaced" <(records) || fail "record: password_replace status=replaced"; ok
grep -qx "account replace-password acct" <(calls) && [[ $(grep -c "^account " <(calls)) == 1 ]] || fail "replace-password gets only the account in argv"; ok
[[ "$(cat "$out/libexec/mailroom/replaced-password")" == "$(printf '%s' "$dummy" | shasum)" ]] || fail "the new password reaches the binary through the terminal"; ok
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
tree maint_install; install_fake happy; chmod u+w "$out/libexec/mailroom"; ln -s /etc/hosts "$out/libexec/mailroom/extra"; chmod u-w "$out/libexec/mailroom"; maint_go
grep -qx "installation untrusted" <(records) && [[ ! -e "$out/libexec/mailroom/calls.log" ]] || fail "maintenance refuses an untrusted installation"; ok; stopped maint_install

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
key_entered() { [[ "$(cat "$out/libexec/mailroom/recovery-key")" == "$(printf '%s' "$dummy_key" | shasum)" ]] || fail "$1: the key reaches the binary through the terminal"; ok; }
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

tree bad_metadata; install_fake happy; printf '%s' '{"id":"other","password_ref":"other-password","local_secret_ref":"other-census-key"}' >"$out/config/acct.json"
run_phase owner_phase
grep -qx "account_metadata unreadable" <(records) && [[ ! -e "$out/libexec/mailroom/calls.log" ]] || fail "metadata whose id differs from its file name is refused"; ok; stopped bad_metadata

tree symlink_install; install_fake happy; chmod u+w "$out/libexec/mailroom"; ln -s /etc/hosts "$out/libexec/mailroom/extra"; chmod u-w,go-w "$out/libexec/mailroom"
run_phase owner_phase
grep -qx "installation untrusted" <(records) && [[ ! -e "$out/libexec/mailroom/calls.log" ]] || fail "a symlink in the installation is refused"; ok

tree acl_install; install_fake happy; chmod +a "user:$me allow add_file" "$out/libexec/mailroom"
run_phase owner_phase
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

# One-command setup with fake tools. The fake sudo records its argv, keeps a copy of the
# handed-over outputs, applies the case's tampering, then runs the install phase as the
# test user and drops the fake mailroom's fixtures into the fresh installation. A hostile
# variable in the caller's environment must not reach the build.
cat >"$work/fake-sudo" <<'SH'
#!/bin/bash
printf '%s\n' "$*" >>"${0%/*}/sudo-log"
[ "$*" = -k ] && exit 0
argv="$*"; shift $(($# - 4)); output="$1"; root="${output%/build/*}"
printf '%s\n' "$argv" >"$root/sudo-argv"
ls -A "${output%/out}" >"$root/handoff-contents"; cp -R "$output" "$root/handoff"
case "$(cat "$root/tamper" 2>/dev/null)" in
  binary) codesign -s - -f -o runtime --identifier org.mailroom.other "$output/mailroom" 2>/dev/null ;;
  ui) echo changed >"$output/ui/index.html" ;;
  extra) echo install=ok ;;
  ok_exit1) /bin/bash "$root/install-driver.sh" "$@"; exit 1 ;;
  identity_exit0) echo install=identity; exit 0 ;;
  bogus) echo install=bogus; exit 1 ;;
esac
exec /bin/bash "$root/install-driver.sh" "$@"
SH
chmod 755 "$work/fake-sudo"

# setup_prepare NAME [TAMPER [INSTALL_COMMIT]]: a checkout, a saved account and the owner
# phase's answers. INSTALL_COMMIT answers "Install this commit?" and defaults to y.
setup_prepare() {
  owner_prepare_answers "$1"
  answer "$out" "approved commit first. [y/N]" "${3-y}"
  [[ -n "${2:-}" ]] && echo "$2" >"$out/tamper"
  local fixtures="$out/libexec/mailroom"
  { overrides; cat <<DRV
RECORDS="$out/install-records"
install_phase "\$@" || exit 1
cp "$work/fake.sh" "$fixtures/fake.sh"; echo happy >"$fixtures/scenario"; echo "$dummy" >"$fixtures/dummy"; chmod go-w "$fixtures"/*
if [ "\$(cat "$out/tamper" 2>/dev/null)" = pins ]; then
  printf 'MAILROOM_CONFIRM_HELPER_CDHASH=%s\nMAILROOM_CDHASH=%s\n' $(printf '1%.0s' {1..40}) $(printf '1%.0s' {1..40}) >"$fixtures/pins"
fi
DRV
  } >"$out/install-driver.sh"
}
setup_go() { run_phase setup_phase; }
built_value() { sed -n "s/^$1 //p" "$out/records"; }
build_removed() { [[ -z "$(ls -A "$out/build")" ]] || fail "$1 removes the build directory"; ok; }
never_launched() { [[ ! -e "$out/libexec/mailroom/calls.log" ]] || fail "$1 launches no mailroom"; ok; }

: >"$work/tools/env-names"; : >"$work/sudo-log"
setup_prepare setup_happy
RUSTC_WRAPPER=/tmp/evil NODE_OPTIONS=--require=/tmp/evil setup_go; completed "one-command setup"
for step in "checkout clean" "install_commit confirmed" "build ok" "install ok" "installation trusted" "native_approve outcome=approved" "settle acct status=settled" \
  "migrate acct-password status=migrated" "enroll acct-password status=ok" "allow acct-password status=ok" "protection acct-password status=ok"; do
  grep -qx "$step" <(records) || fail "setup record: $step"; ok
done
grep -qx "install ok" "$out/install-records" || fail "the install phase records its result"; ok
mailroom_hash=$(built_value mailroom_cdhash); confirm_hash=$(built_value confirm_cdhash); ui_digest=$(built_value ui_digest)
[[ "$(built_value commit)" == "$(git -C "$checkout" rev-parse HEAD)" ]] || fail "the build records the checkout's HEAD commit"; ok
grep -qa "  $(git -C "$checkout" rev-parse HEAD)" "$out/transcript" || fail "the full HEAD commit is shown before the build"; ok
[[ "$(cat "$work/sudo-log")" == "-k /usr/bin/env -i"*$'\n'"-k" ]] || fail "sudo ignores a cached login and drops it after the install"; ok
[[ "$(cat "$out/sudo-argv")" == "-k /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p $checkout/scripts/setup-macos-protection.sh install $(built_value output) $mailroom_hash $confirm_hash $ui_digest" ]] \
  || fail "sudo gets a cleared environment and the build's identities"; ok
[[ "$(cat "$out/handoff-contents")" == out ]] || fail "only the signed outputs remain at the handoff"; ok
[[ "$mailroom_hash" == "$(cdhash "$out/handoff/mailroom")" && "$confirm_hash" == "$(cdhash "$out/handoff/mailroom-confirm")" ]] || fail "recorded identities"; ok
[[ "$(cat "$out/libexec/mailroom/pins")" == "MAILROOM_CONFIRM_HELPER_CDHASH=$confirm_hash"$'\n'"MAILROOM_CDHASH=$mailroom_hash" ]] || fail "setup pins"; ok
build_removed "setup"
grep -qE "^(RUSTC_WRAPPER|NODE_OPTIONS|DEVELOPER_DIR|BASH_ENV)$" "$work/tools/env-names" && fail "a hostile variable reached the build"; ok
grep -qx "CARGO_HOME" "$work/tools/env-names" || fail "builds get a fresh CARGO_HOME"; ok
[[ $(grep -c "Ready to start?" "$out/transcript") == 1 ]] || fail "one run asks to start once"; ok
no_secret setup_happy
cp -R "$out/handoff" "$work/built"

setup_prepare setup_tampered_binary binary; setup_go
grep -qx "install identity" <(records) && grep -qx "install identity" "$out/install-records" && [[ ! -e "$out/libexec/mailroom" ]] \
  || fail "a binary re-signed after the build is refused at the handoff"; ok
never_launched "a refused handoff"; build_removed "a refused handoff"; stopped setup_tampered_binary
setup_prepare setup_tampered_ui ui; setup_go
grep -qx "install ui_digest" <(records) && [[ ! -e "$out/libexec/mailroom" ]] || fail "a UI changed after the build is refused at the handoff"; ok
never_launched "a changed UI"; stopped setup_tampered_ui
setup_prepare setup_extra_reply extra; setup_go
grep -qx "install unexpected" <(records) || fail "an install reply with an extra line is unexpected"; ok; never_launched "an extra install reply"; stopped setup_extra_reply
setup_prepare setup_other_pins pins; setup_go
grep -qx "install ok" <(records) && grep -qx "installation untrusted" <(records) || fail "pins that differ from the build are refused"; ok
never_launched "pins that differ from the build"; stopped setup_other_pins
for mode in ok_exit1 identity_exit0 bogus; do
  setup_prepare "setup_reply_$mode" "$mode"; setup_go
  grep -qx "install unexpected" <(records) || fail "install reply $mode is unexpected"; ok; never_launched "install reply $mode"; stopped "setup_reply_$mode"
done
not_built() { [[ ! -e "$out/sudo-argv" && -z "$(ls -A "$out/build")" ]] || fail "$1 builds nothing and never calls sudo"; ok; }
setup_prepare setup_dirty_tracked; echo "# edit" >>"$checkout/scripts/setup-macos-protection.sh"; setup_go
grep -qx "checkout changed" <(records) && ! grep -qa "Install this commit" "$out/transcript" || fail "an uncommitted script change is refused before the confirmation"; ok
not_built "a changed checkout"; stopped setup_dirty_tracked
setup_prepare setup_dirty_untracked; mkdir -p "$checkout/src"; echo new >"$checkout/src/new.rs"; setup_go
grep -qx "checkout changed" <(records) || fail "an untracked file the build could read is refused"; ok; not_built "an untracked source"; stopped setup_dirty_untracked
setup_prepare setup_docs_changed; mkdir -p "$checkout/docs"; echo note >"$checkout/docs/n.md"; echo note >"$checkout/NOTES.md"; : >"$checkout/.DS_Store"
setup_go; completed "changes outside the build set"; grep -qx "checkout clean" <(records) || fail "docs, top-level Markdown and .DS_Store do not block"; ok
setup_prepare setup_declined "" ""; setup_go
grep -qx "install_commit declined" <(records) || fail "a declined commit is recorded"; ok; not_built "a declined commit"; stopped setup_declined
setup_prepare outside_config; mkdir "$out/build/.cargo"; setup_go
grep -qx "build outside_config" <(records) && [[ "$(ls -A "$out/build")" == .cargo && ! -e "$out/sudo-argv" ]] \
  || fail "a cargo config above the build is refused before sudo and its directory removed"; ok; stopped outside_config
setup_prepare setup_no_commit; rm -rf "$checkout/.git"; setup_go
grep -qx "build source" <(records) && [[ ! -e "$out/sudo-argv" ]] || fail "a checkout without a commit is refused before sudo"; ok; build_removed "a refused source"; stopped setup_no_commit

# Install phase on the built outputs, as sudo starts it.
install_run() { local name="$1"; tree "$name"; rm -rf "${out:?}/out"; cp -R "$work/built" "$out/out"; typed_mailroom="${2:-$mailroom_hash}"; }
install_go() { run_phase "install_phase \"$out/out\" \"$typed_mailroom\" \"$confirm_hash\" \"$ui_digest\""; }
installed() { [[ -e "$out/libexec/mailroom" ]]; }
refused() { [[ $(cat "$out/exit") != 0 && "$(tr -d '\r' <"$out/transcript")" == "install=$1" ]] && grep -qx "install $1" <(records) && ! installed || fail "$2"; ok; }

install_run install_happy; install_go
[[ $(cat "$out/exit") == 0 && "$(tr -d '\r' <"$out/transcript")" == install=ok ]] || fail "the install phase prints only its result"; ok
grep -qx "install ok" <(records) || fail "install record"; ok
bash -c 'source "$1"; TRUSTED_OWNER="$2"; trusted_tree "$3"' _ "$tree_script" "$me" "$out/libexec/mailroom" || fail "installation is a trusted tree"; ok
[[ "$(cat "$out/libexec/mailroom/pins")" == "MAILROOM_CONFIRM_HELPER_CDHASH=$confirm_hash"$'\n'"MAILROOM_CDHASH=$mailroom_hash" ]] || fail "pins"; ok
[[ "$(ls -A "$out/libexec")" == mailroom ]] || fail "no staging is left behind"; ok

install_run install_wrong 0000000000000000000000000000000000000000; install_go; refused identity "a copy with another cdhash is refused"
install_run install_ui; echo changed >"$out/out/ui/index.html"; install_go; refused ui_digest "a changed UI is refused"
install_run install_symlink; rm "$out/out/mailroom"; ln -s /etc/hosts "$out/out/mailroom"; install_go; refused not_plain "a symlinked output is copied as a link and refused"
install_run install_fifo; rm "$out/out/mailroom-confirm"; mkfifo "$out/out/mailroom-confirm"; install_go; refused not_plain "a FIFO output is never read and is refused"
install_run install_malformed "not-a-cdhash"; install_go; refused malformed "a malformed identity is refused"
[[ -z "$(ls -A "$work/install_wrong/libexec")" && -z "$(ls -A "$work/install_fifo/libexec")" ]] || fail "refused staging is removed"; ok
install_run planted_pins; mkdir -m 755 "$out/libexec/mailroom"; echo keep >"$out/victim"; ln -s "$out/victim" "$out/libexec/mailroom/pins"; install_go
[[ "$(cat "$out/victim")" == keep && ! -L "$out/libexec/mailroom/pins" ]] && grep -qx "install ok" <(records) || fail "a planted pins symlink is moved aside, never followed"; ok
install_run acl_parent; chmod +a "user:$me allow add_subdirectory" "$out/libexec"; install_go; refused untrusted_parent "an ACL on the install parent is refused"

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
/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p "$script" install "$work" "$mailroom_hash" "$confirm_hash" "$ui_digest" </dev/null >/dev/null 2>&1 && fail "install as non-root"; ok
# The launcher clears the caller's environment, so the wizard refuses only for the missing
# terminal, and bash -p ignores BASH_ENV in the launcher itself.
printf 'touch "%s/bash-env-ran"\n' "$work" >"$work/bash-env.sh"
entry=$(BASH_ENV="$work/bash-env.sh" RUSTC_WRAPPER=/tmp/evil "$repo/scripts/setup.sh" </dev/null 2>&1) && fail "the launcher without a terminal"
[[ "$entry" == "Run scripts/setup.sh in a terminal"* && ! -e "$work/bash-env-ran" ]] || fail "the launcher clears the environment"; ok
[[ "$(cd "$repo/scripts" && ./setup.sh --plan | wc -l)" -eq 3 ]] || fail "the launcher finds the wizard beside it"; ok
echo "wizard runs: $passed checks passed"
