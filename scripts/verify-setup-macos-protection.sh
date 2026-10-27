#!/usr/bin/env bash
# Drives the owner wizard's real stage logic in a pseudo-terminal against a signed fake
# mailroom binary, in a temporary tree standing in for the root-owned locations.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"; trap '[[ -n "${KEEP:-}" ]] && echo "work=$work" || rm -rf "$work"' EXIT
chmod 700 "$work"
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
  argv[0] = script; execv("/bin/sh", (char *[]){"/bin/sh", script, argc > 1 ? argv[1] : "", argc > 2 ? argv[2] : "", argc > 3 ? argv[3] : "", NULL});
  return 127;
}
C
/usr/bin/clang -o "$work/fake-mailroom" "$work/wrapper.c"

cat >"$work/fake.sh" <<'SH'
#!/bin/sh
here=$(dirname "$0"); scenario=$(cat "$here/scenario"); log="$here/calls.log"
echo "$*" >>"$log"
once() { [ -e "$here/$1" ] && return 1; : >"$here/$1"; }
case "$1 $2" in
  "native-check ")
    [ "$scenario" = native_denied ] && { echo outcome=denied; exit 1; }
    if once asked; then echo outcome=approved; else echo outcome=cancelled; fi ;;
  "keychain legacy") echo "ref=old-census-key class=pinned_self"; echo status=ok ;;
  "keychain migrate") [ "$scenario" = mismatch ] && { echo status=mismatch; exit 1; }; echo status=migrated ;;
  "keychain retire-legacy") echo status=retired ;;
  "keychain refs") echo "ref=acct-password class=every_access"; echo "ref=acct-census-key class=pinned_self"; echo status=ok ;;
  "keychain status")
    case "$3" in
      acct-census-key) if once key-moved; then echo "status=other_binary class=pinned_self"; else echo "status=ok class=pinned_self"; fi ;;
      acct-password) if [ -e "$here/enrolled" ]; then echo "status=ok class=every_access"; else echo "status=missing class=every_access"; exit 1; fi ;;
    esac ;;
  "keychain retrust") [ "$scenario" = killed ] && kill -9 $$; echo status=retrusted ;;
  "keychain enroll")
    stty -echo </dev/tty; printf 'Password: ' >/dev/tty; IFS= read -r a </dev/tty
    printf 'Repeat it: ' >/dev/tty; IFS= read -r b </dev/tty; stty echo </dev/tty
    [ "$a" = "$b" ] || { echo status=mismatch; exit 1; }
    printf '%s' "$a" | shasum >"$here/enrolled"
    [ "$scenario" = garbage ] && echo "password was $a"
    echo status=ok ;;
  "keychain probe")
    if once probed; then
      [ "$scenario" = probe_wrong ] && { echo status=ok; exit 0; }
      echo status=denied; exit 1
    fi
    echo status=ok ;;
  *) echo status=usage; exit 2 ;;
esac
SH

# scenario NAME OWNER DIGEST_ANSWER: runs one wizard pass and leaves its results in $out.
scenario() {
  local name="$1" owner="$2" answer="$3"
  local root="$work/$name"; mkdir -p "$root/share" "$root/libexec" "$root/home"
  chmod 755 "$root" "$root/share" "$root/libexec"
  local frozen="$root/share/setup-0123456789abcdef0123456789abcdef01234567"
  mkdir -p "$frozen/scripts" "$frozen/ui"
  sed '$d' "$repo/scripts/setup-macos-protection.sh" >"$frozen/scripts/setup-macos-protection.sh"
  echo "approved source" >"$frozen/README"
  chmod -R go-w "$frozen"
  cp "$work/fake-mailroom" "$work/fake.sh" "$root/libexec/" 2>/dev/null || true
  rm -f "$root/libexec/fake-mailroom" "$root/libexec/fake.sh"
  mkdir -p "$root/build"; cp "$work/fake-mailroom" "$root/build/mailroom"; cp "$work/fake.sh" "$root/libexec/fake.sh"
  echo "$name" >"$root/libexec/scenario"
  local digest; digest=$(MAILROOM_SETUP_TEST=1 bash -c 'source "$1"; tree_digest "$2"' _ "$frozen/scripts/setup-macos-protection.sh" "$frozen")
  [[ "$answer" == right ]] && answer="$digest"
  out="$root"
  cat >"$root/driver.sh" <<DRV
source "$frozen/scripts/setup-macos-protection.sh"
SHARE="$(cd "$root/share" && pwd -P)"; LIBEXEC="$(cd "$root/libexec" && pwd -P)"; TRUSTED_OWNER="$owner"; HOME="$root/home"
RECORDS="$root/home/.config/mailroom/native-setup-records"
privileged() { "\$@"; }
build_helpers() { install_signed "$root/build/mailroom" mailroom org.mailroom.fake && MAILROOM_CDHASH="\$SIGNED_CDHASH" && CONFIRM_CDHASH=\$MAILROOM_CDHASH; }
setup_main
DRV
  printf '%s\n' "$answer" "$dummy" | /usr/bin/python3 -I -c '
import os, pty, select, sys
digest, password = sys.stdin.read().split("\n")[:2]
rules = [(b"Ready to start?", ""), (b"approval record:", digest), (b"Build and install now?", ""),
         (b"for a while. [y/N]", ""), (b"Password: ", password), (b"Repeat it: ", password)]
pid, fd = pty.fork()
if pid == 0:
    os.execv("/bin/bash", ["/bin/bash", sys.argv[1]])
seen, answered = b"", 0
while True:
    ready, _, _ = select.select([fd], [], [], 20)
    if not ready:
        break
    try:
        chunk = os.read(fd, 4096)
    except OSError:
        break
    if not chunk:
        break
    seen += chunk
    tail = seen[answered:]
    for prompt, answer in rules:
        if prompt in tail:
            answered = len(seen)
            os.write(fd, (answer + "\n").encode())
            break
_, status = os.waitpid(pid, 0)
open(sys.argv[2], "wb").write(seen)
sys.exit(os.waitstatus_to_exitcode(status))
' "$root/driver.sh" "$root/transcript" && code=0 || code=$?
  echo "$code" >"$root/exit"
}
records() { cat "$out/home/.config/mailroom/native-setup-records" 2>/dev/null || true; }
calls() { cat "$out/libexec/calls.log" 2>/dev/null || true; }
no_secret() { if grep -rqaF "$dummy" "$out" --exclude=driver.sh; then fail "$1: dummy password appeared in output, a file or an argv log"; fi; ok; }

scenario happy "$(id -un)" right
[[ $(cat "$out/exit") == 0 ]] || fail "happy exit"; ok
grep -qa "Setup complete" "$out/transcript" || fail "happy reaches completion"; ok
for step in "tree_digest match" "build ok" "native_approve outcome=approved" "native_cancel outcome=cancelled" "migrate old-census-key status=migrated" "retrust acct-census-key status=retrusted" "enroll acct-password status=ok" "deny acct-password status=denied" "allow acct-password status=ok" "protection acct-password status=ok"; do
  grep -qx "$step" <(records) || fail "happy record: $step"; ok
done
! grep -q "retire-legacy" <(calls) || fail "legacy originals kept by default"; ok
[[ "$(stat -f %Lp "$out/libexec/mailroom")" == 755 ]] || fail "installed mode"; ok
no_secret happy

scenario probe_wrong "$(id -un)" right
[[ $(cat "$out/exit") != 0 ]] && grep -qa "Setup incomplete" "$out/transcript" || fail "a silent read stops setup"; ok
grep -qx "deny acct-password status=ok" <(records) || fail "wrong probe recorded"; ok
! grep -qa "Setup complete" "$out/transcript" || fail "no completion after a wrong probe"; ok

scenario mismatch "$(id -un)" right
grep -qx "migrate old-census-key status=mismatch" <(records) && ! grep -q retire-legacy <(calls) || fail "mismatch stops before retirement"; ok

scenario native_denied "$(id -un)" right
grep -qx "native_approve outcome=denied" <(records) && ! grep -q "keychain" <(calls) || fail "denied native check stops before Keychain steps"; ok

scenario garbage "$(id -un)" right
grep -qx "enroll acct-password status=unexpected" <(records) || fail "unexpected output becomes a fixed record"; ok
no_secret garbage

scenario killed "$(id -un)" right
grep -qx "retrust acct-census-key status=terminated" <(records) || fail "a killed helper is recorded as terminated"; ok
[[ $(cat "$out/exit") != 0 ]] && ! grep -q "keychain enroll" <(calls) || fail "a killed helper stops later stages"; ok

scenario wrong_digest "$(id -un)" 0000000000000000000000000000000000000000000000000000000000000000
grep -qx "tree_digest mismatch" <(records) && [[ ! -e "$out/libexec/mailroom" ]] || fail "a changed tree is refused before building"; ok

scenario untrusted root right
grep -qx "frozen_copy untrusted" <(records) && [[ ! -e "$out/libexec/mailroom" ]] || fail "a user-owned copy is refused"; ok

plan="$(bash "$repo/scripts/setup-macos-protection.sh" --plan)"
[[ "$(wc -l <<<"$plan")" -eq 7 ]] || fail "plan lists seven stages"; ok
if bash "$repo/scripts/setup-macos-protection.sh" </dev/null >/dev/null 2>&1; then fail "noninteractive run refused"; fi; ok
echo "wizard runs: $passed checks passed"
