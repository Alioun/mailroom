#!/usr/bin/env bash
# Drives both wizard phases' real stage logic in a pseudo-terminal: the root phase with
# fake build tools, the owner phase with a signed fake mailroom. A temporary tree owned by
# the test user stands in for the root-owned locations. No sudo, Keychain or real build.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"; work="$(cd "$work" && pwd -P)"
trap '[[ -n "${KEEP:-}" ]] && echo "work=$work" || rm -rf "$work"' EXIT
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
    [ "$scenario" = shaped_secret ] && echo "ref=$(cat "$here/dummy") class=every_access"
    echo "ref=acct-password class=every_access"; echo "ref=acct-census-key class=pinned_self"; echo status=ok ;;
  "keychain legacy") echo "ref=old-census-key class=pinned_self"; echo status=ok ;;
  "keychain retrust")
    [ "$scenario" = killed ] && kill -9 $$
    [ "$scenario" = staged_planted ] && { echo status=unverified; exit 1; }
    echo status=retrusted ;;
  "keychain check-store")
    [ "$scenario" = success_exit1 ] && { echo status=verified; exit 1; }
    echo status=verified ;;
  "keychain migrate") [ "$scenario" = mismatch ] && { echo status=mismatch; exit 1; }; echo status=migrated ;;
  "keychain retire-legacy") echo status=retired ;;
  "keychain status")
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
    if once probed; then
      [ "$scenario" = probe_wrong ] && { echo status=ok; exit 0; }
      echo status=denied; exit 1
    fi
    echo status=ok ;;
  *) echo status=usage; exit 2 ;;
esac
SH

# Fake build tools for the root phase. They record the environment names they saw.
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

# drive NAME SCRIPT ANSWER_TOOLCHAINS: runs SCRIPT under bash in a pty answering prompts.
drive() {
  local root="$1" driver="$2" toolchains="$3"
  printf '%s\n' "$(cat "$root/digest")" "$dummy" "$toolchains" | /usr/bin/python3 -I -c '
import os, pty, select, signal, sys
digest, password, toolchains = sys.stdin.read().split("\n")[:3]
rules = [(b"Ready to start?", ""), (b"approval record:", digest), (b"for this build? [y/N]", toolchains),
         (b"for a while. [y/N]", ""), (b"Password: ", password), (b"Repeat it: ", password)]
pid, fd = pty.fork()
if pid == 0:
    os.execv("/bin/bash", ["/bin/bash", sys.argv[1]])
seen, answered = b"", 0
while True:
    ready, _, _ = select.select([fd], [], [], 20)
    if not ready:
        os.kill(pid, signal.SIGKILL)
        break
    try:
        chunk = os.read(fd, 4096)
    except OSError:
        break
    if not chunk:
        break
    seen += chunk
    for prompt, answer in rules:
        if prompt in seen[answered:]:
            answered = len(seen)
            os.write(fd, (answer + "\n").encode())
            break
_, status = os.waitpid(pid, 0)
open(sys.argv[2], "wb").write(seen)
sys.exit(os.waitstatus_to_exitcode(status))
' "$driver" "$root/transcript" && echo 0 >"$root/exit" || echo $? >"$root/exit"
}

# tree NAME: a frozen copy, a digest and an installed fake mailroom, all owned by the test user.
tree() {
  local root="$work/$1"; rm -rf "$root"; mkdir -p "$root/share" "$root/libexec" "$root/home" "$root/build"
  chmod 755 "$root" "$root/share" "$root/libexec" "$root/build"
  frozen="$root/share/setup-0123456789abcdef0123456789abcdef01234567"
  mkdir -p "$frozen/scripts" "$frozen/ui" "$frozen/native/macos"
  sed '$d' "$repo/scripts/setup-macos-protection.sh" >"$frozen/scripts/setup-macos-protection.sh"
  echo "approved source" >"$frozen/README"
  chmod -R go-w "$frozen"
  bash -c 'source "$1"; tree_digest "$2"' _ "$frozen/scripts/setup-macos-protection.sh" "$frozen" >"$root/digest"
  out="$root"
}
overrides() {
  cat <<DRV
source "$frozen/scripts/setup-macos-protection.sh"
SHARE="$out/share"; LIBEXEC_PARENT="$out/libexec"; LIBEXEC="$out/libexec/mailroom"; BUILD_BASE="$out/build"
TRUSTED_OWNER="$me"; HOME="$out/home"; CARGO="$work/tools/cargo"; RUSTC="$work/tools/rustc"
NODE="$work/tools/node"; NPM_CLI="$work/tools/npm-cli.js"; XCRUN="$work/tools/xcrun"
DRV
}
install_fake() {
  mkdir -m 755 "$out/libexec/mailroom"
  cp "$work/fake-mailroom" "$out/libexec/mailroom/mailroom"; cp "$work/fake.sh" "$out/libexec/mailroom/fake.sh"
  codesign -s - -f -o runtime --identifier org.mailroom.fake "$out/libexec/mailroom/mailroom" 2>/dev/null
  local cdhash; cdhash=$(codesign -d -vvv --arch "$(uname -m)" "$out/libexec/mailroom/mailroom" 2>&1 | sed -n 's/^CDHash=//p')
  printf 'MAILROOM_CONFIRM_HELPER_CDHASH=%s\nMAILROOM_CDHASH=%s\n' "$cdhash" "$cdhash" >"$out/libexec/mailroom/pins"
  echo "$1" >"$out/libexec/mailroom/scenario"; echo "$dummy" >"$out/libexec/mailroom/dummy"
  chmod -R go-w "$out/libexec/mailroom"
}
owner_run() {
  local name="$1"; tree "$name"; install_fake "$name"
  { overrides; echo "RECORDS=\"$out/records\""; echo "owner_phase \"$frozen\""; } >"$out/driver.sh"
  drive "$out" "$out/driver.sh" ""
}
records() { cat "$out/records" 2>/dev/null || true; }
calls() { cat "$out/libexec/mailroom/calls.log" 2>/dev/null || true; }
no_secret() { if grep -rqaF "$dummy" "$out" --exclude=driver.sh --exclude=dummy; then fail "$1: dummy password appeared in output or a file"; fi; ok; }
stopped() { [[ $(cat "$out/exit") != 0 ]] && grep -qa "Setup incomplete" "$out/transcript" && ! grep -qa "Setup complete" "$out/transcript" || fail "$1 stops setup"; ok; }

owner_run happy
[[ $(cat "$out/exit") == 0 ]] && grep -qa "Setup complete" "$out/transcript" || fail "owner happy path"; ok
for step in "tree_digest match" "installation trusted" "native_approve outcome=approved" "native_cancel outcome=cancelled" \
  "retrust acct-census-key status=retrusted" "verify acct-census-key status=verified" "migrate old-census-key status=migrated" \
  "enroll acct-password status=ok" "deny acct-password status=denied" "allow acct-password status=ok" "protection acct-password status=ok"; do
  grep -qx "$step" <(records) || fail "record: $step"; ok
done
[[ "$(grep -n 'retrust\|migrate' <(calls) | head -1)" == *retrust* ]] || fail "earlier-build keys move before legacy migration"; ok
! grep -q "retire-legacy" <(calls) || fail "legacy originals kept by default"; ok
no_secret happy

owner_run probe_wrong; grep -qx "deny acct-password status=ok" <(records) || fail "a silent read is recorded and is not a denial"; ok; stopped probe_wrong
owner_run trailing_failure; grep -qx "enroll acct-password status=unexpected" <(records) || fail "a failure record after success"; ok; stopped trailing
owner_run success_exit1; grep -qx "verify acct-census-key status=unexpected" <(records) || fail "success with a failing exit"; ok; stopped success_exit1
owner_run shaped_secret; grep -qx "account_refs status=unexpected" <(records) || fail "a record-shaped secret reference"; ok; no_secret shaped_secret; stopped shaped_secret
owner_run shaped_status; grep -qx "enroll acct-password status=unexpected" <(records) || fail "a record-shaped secret status"; ok; no_secret shaped_status
owner_run mismatch; grep -qx "migrate old-census-key status=mismatch" <(records) && ! grep -q retire-legacy <(calls) || fail "mismatch stops before retirement"; ok
owner_run native_denied; grep -qx "native_approve outcome=denied" <(records) && ! grep -q keychain <(calls) || fail "denied native check stops first"; ok
owner_run killed; grep -qx "retrust acct-census-key status=terminated" <(records) && ! grep -q "keychain enroll" <(calls) || fail "a killed command stops setup"; ok
owner_run staged_planted; grep -qx "retrust acct-census-key status=unverified" <(records) || fail "a planted staged copy stops setup"; ok; stopped staged

tree wrong_digest; install_fake happy; echo 0000 >"$out/digest"
{ overrides; echo "RECORDS=\"$out/records\""; echo "owner_phase \"$frozen\""; } >"$out/driver.sh"; drive "$out" "$out/driver.sh" ""
grep -qx "tree_digest mismatch" <(records) && [[ ! -e "$out/libexec/mailroom/calls.log" ]] || fail "a changed tree is refused"; ok

tree acl_frozen; install_fake happy; chmod +a "user:$me allow write" "$frozen/README"
{ overrides; echo "RECORDS=\"$out/records\""; echo "owner_phase \"$frozen\""; } >"$out/driver.sh"; drive "$out" "$out/driver.sh" ""
grep -qx "frozen_copy untrusted" <(records) || fail "a file ACL granting writes is refused"; ok

tree symlink_install; install_fake happy; chmod u+w "$out/libexec/mailroom"; ln -s /etc/hosts "$out/libexec/mailroom/extra"; chmod u-w,go-w "$out/libexec/mailroom"
{ overrides; echo "RECORDS=\"$out/records\""; echo "owner_phase \"$frozen\""; } >"$out/driver.sh"; drive "$out" "$out/driver.sh" ""
grep -qx "installation untrusted" <(records) && [[ ! -e "$out/libexec/mailroom/calls.log" ]] || fail "a symlink in the installation is refused"; ok

tree acl_install; install_fake happy; chmod +a "user:$me allow add_file" "$out/libexec/mailroom"
{ overrides; echo "RECORDS=\"$out/records\""; echo "owner_phase \"$frozen\""; } >"$out/driver.sh"; drive "$out" "$out/driver.sh" ""
grep -qx "installation untrusted" <(records) || fail "an ACL on the installation is refused"; ok

# Root phase with fake tools. A hostile variable in the caller's environment must not reach the build.
root_run() {
  local name="$1" answer="$2"; tree "$name"
  { overrides; echo "RECORDS=\"$out/records\""; echo "root_phase \"$frozen\" $me"; } >"$out/driver.sh"
  RUSTC_WRAPPER=/tmp/evil NODE_OPTIONS=--require=/tmp/evil drive "$out" "$out/driver.sh" "$answer"
}
: >"$work/tools/env-names"
root_run root_happy y
[[ $(cat "$out/exit") == 0 ]] && grep -qx "install ok" <(records) || fail "root build and install"; ok
grep -qE "^(RUSTC_WRAPPER|NODE_OPTIONS|DEVELOPER_DIR|BASH_ENV)$" "$work/tools/env-names" && fail "a hostile variable reached the build"; ok
grep -qx "CARGO_HOME" "$work/tools/env-names" || fail "builds get a fresh CARGO_HOME"; ok
[[ -f "$out/libexec/mailroom/mailroom" && -f "$out/libexec/mailroom/pins" && -f "$out/libexec/mailroom/ui/index.html" ]] || fail "installed files"; ok
bash -c 'source "$1"; TRUSTED_OWNER="$2"; trusted_tree "$3"' _ "$frozen/scripts/setup-macos-protection.sh" "$me" "$out/libexec/mailroom" || fail "installation is a trusted tree"; ok
[[ -z "$(ls -A "$out/build")" || "$(ls "$out/build")" == mailroom-install-records ]] || fail "the build directory is removed"; ok

root_run declined ""
grep -qx "toolchains declined" <(records) && [[ ! -e "$out/libexec/mailroom" ]] || fail "declined toolchains stop before building"; ok

tree outside_config; mkdir "$out/build/.cargo"
{ overrides; echo "RECORDS=\"$out/records\""; echo "root_phase \"$frozen\" $me"; } >"$out/driver.sh"; drive "$out" "$out/driver.sh" y
grep -qx "build outside_config" <(records) && [[ ! -e "$out/libexec/mailroom" ]] || fail "a cargo config above the build is refused"; ok

tree planted_pins; mkdir -m 755 "$out/libexec/mailroom"; echo keep >"$out/victim"; ln -s "$out/victim" "$out/libexec/mailroom/pins"
{ overrides; echo "RECORDS=\"$out/records\""; echo "root_phase \"$frozen\" $me"; } >"$out/driver.sh"; drive "$out" "$out/driver.sh" y
[[ "$(cat "$out/victim")" == keep && ! -L "$out/libexec/mailroom/pins" ]] && grep -qx "install ok" <(records) || fail "a planted pins symlink is moved aside, never followed"; ok

tree acl_parent; chmod +a "user:$me allow add_subdirectory" "$out/libexec"
{ overrides; echo "RECORDS=\"$out/records\""; echo "root_phase \"$frozen\" $me"; } >"$out/driver.sh"; drive "$out" "$out/driver.sh" y
grep -qx "install untrusted_parent" <(records) && [[ ! -e "$out/libexec/mailroom" ]] || fail "an ACL on the install parent is refused"; ok

# Entry checks on the real file: only the exact launch command passes.
script="$repo/scripts/setup-macos-protection.sh"
[[ "$(bash "$script" --plan | wc -l)" -eq 2 ]] || fail "plan"; ok
/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash "$script" </dev/null >/dev/null 2>&1 && fail "without -p"; ok
/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin RUSTC_WRAPPER=/tmp/evil /bin/bash -p "$script" </dev/null >/dev/null 2>&1 && fail "with a hostile variable"; ok
/usr/bin/env -i PATH=/opt/homebrew/bin:/usr/bin:/bin /bin/bash -p "$script" </dev/null >/dev/null 2>&1 && fail "with another PATH"; ok
/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /bin/bash -p "$script" install "$me" </dev/null >/dev/null 2>&1 && fail "install as non-root"; ok
echo "wizard runs: $passed checks passed"
