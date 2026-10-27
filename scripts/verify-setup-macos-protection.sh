#!/usr/bin/env bash
# Drives the owner wizard end to end against fake tools in a pseudo-terminal.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"
trap '[[ -n "${KEEP:-}" ]] || rm -rf "$work"' EXIT
fakes="$work/fakes"; build="$work/build"; helpers="$work/helpers"; config="$work/config"
mkdir -p "$fakes" "$build" "$config"
dummy="nk-dummy-wizard-$RANDOM$RANDOM"
log="$work/calls.log"

cat >"$fakes/swift" <<FAKE
#!/bin/bash
echo "swift \$*" >>"$log"
for n in mailroom-confirm mailroom-keychain; do cp "$fakes/\$n" "$build/\$n"; done
FAKE
cat >"$fakes/codesign" <<FAKE
#!/bin/bash
echo "codesign \$*" >>"$log"
if [[ "\$1" == -d ]]; then name="\${@: -1}"; printf 'CDHash=%s\n' "\$(printf '%s' "\$(basename "\$name")" | shasum | cut -c1-40)" >&2; fi
FAKE
cat >"$fakes/cargo" <<FAKE
#!/bin/bash
echo "cargo \$* pins=\${MAILROOM_CONFIRM_HELPER_CDHASH:-}/\${MAILROOM_KEYCHAIN_HELPER_CDHASH:-}" >>"$log"
FAKE
cat >"$fakes/mailroom-confirm" <<'FAKE'
#!/usr/bin/python3 -I
import base64, hashlib, hmac, json, os, sys
envelope = json.loads(sys.stdin.read())
request = base64.b64decode(envelope["request"]); challenge = base64.b64decode(envelope["challenge"])
marker = os.path.join(os.path.dirname(os.path.realpath(__file__)), "asked")
outcome = "cancelled" if os.path.exists(marker) else "approved"
open(marker, "a").close()
rid = json.loads(request)["request_id"]; digest = hashlib.sha256(request).hexdigest()
message = b"mailroom-exact-action-outcome-v1\0" + json.dumps([rid, digest, outcome], separators=(",", ":")).encode()
print(json.dumps({"request_id": rid, "request_digest": digest, "outcome": outcome,
                  "authentication": hmac.new(challenge, message, hashlib.sha256).hexdigest()}), end="")
FAKE
cat >"$fakes/mailroom-keychain" <<FAKE
#!/bin/bash
echo "keychain \$*" >>"$log"
state="\$(dirname "\$0")/state"
case "\$1" in
  legacy) echo acct-old-census-key ;;
  migrate) echo status=ok ;;
  status) [[ "\$2" != acct-new-password || -e "\$state" ]] || { echo status=missing; exit 1; }
          echo "status=ok protection=every_access expected=every_access" ;;
  enroll) IFS= read -rs a </dev/tty; IFS= read -rs b </dev/tty
          [[ "\$a" == "\$b" ]] || { echo status=mismatch; exit 1; }
          printf '%s' "\$a" | shasum >"\$state"; echo "status=ok length=\${#a}" ;;
  probe) if [[ -e "\$state.probed" ]]; then echo status=ok; else touch "\$state.probed"; echo status=denied; exit 1; fi ;;
  retire-legacy) echo status=ok ;;
esac
FAKE
chmod +x "$fakes"/*
printf '{"password_ref":"acct-new-password","login":"invented@example.test"}' >"$config/acct-new.json"

answers="$work/answers"
printf '\n\n\n\n\n\n\n%s\n%s\n\n' "$dummy" "$dummy" >"$answers"
transcript="$work/transcript"
env -i HOME="$work/home" PATH="/usr/bin:/bin" TERM=dumb \
  MAILROOM_SWIFT="$fakes/swift" MAILROOM_CODESIGN="$fakes/codesign" MAILROOM_CARGO="$fakes/cargo" \
  MAILROOM_HELPERS="$helpers" MAILROOM_PINS="$work/pins.env" MAILROOM_CONFIG="$config" MAILROOM_NATIVE_BUILD="$build" \
  script -q "$transcript" /bin/bash "$repo/scripts/setup-macos-protection.sh" \
  < <(while IFS= read -r line; do sleep 0.4; printf '%s\n' "$line"; done <"$answers"; sleep 1) >/dev/null
status=$?

fail() { echo "FAIL: $1"; exit 1; }
[[ $status -eq 0 ]] || fail "wizard exit $status"
confirm_hash="$(printf '%s' mailroom-confirm | shasum | cut -c1-40)"
keychain_hash="$(printf '%s' mailroom-keychain | shasum | cut -c1-40)"
grep -qx "MAILROOM_CONFIRM_HELPER_CDHASH='$confirm_hash'" "$work/pins.env" || fail "confirm pin recorded"
grep -qx "MAILROOM_KEYCHAIN_HELPER_CDHASH='$keychain_hash'" "$work/pins.env" || fail "keychain pin recorded"
grep -qx "MAILROOM_KEYCHAIN_HELPER='$helpers/mailroom-keychain'" "$work/pins.env" || fail "keychain path recorded"
grep -q "codesign -s - -f -o runtime --identifier org.mailroom.confirm $helpers/mailroom-confirm" "$log" || fail "hardened ad-hoc signing"
grep -q "cargo build --release --locked pins=$confirm_hash/$keychain_hash" "$log" || fail "pinned release build"
[[ "$(stat -f %Lp "$helpers")" == 700 ]] || fail "private helper directory"
grep -q "outcome=approved (expected outcome=approved)" "$transcript" || fail "approved native check"
grep -q "outcome=cancelled (expected outcome=cancelled)" "$transcript" || fail "cancelled native check"
grep -q "keychain migrate acct-old-census-key" "$log" || fail "legacy migration"
! grep -q "retire-legacy" "$log" || fail "legacy originals kept by default"
grep -q "keychain enroll acct-new-password" "$log" || fail "missing password enrolled"
grep -q "status=ok length=${#dummy}" "$transcript" || fail "password reached the helper through the terminal"
grep -q "status=denied (expected status=denied)" "$transcript" || fail "denied probe"
grep -q "status=ok (expected status=ok)" "$transcript" || fail "allowed probe"
if grep -rqF "$dummy" "$work" --exclude=answers; then fail "dummy password appeared in a file, the transcript or an argv log"; fi
plan="$(bash "$repo/scripts/setup-macos-protection.sh" --plan)"
[[ "$(wc -l <<<"$plan")" -eq 6 ]] || fail "plan lists six stages"
if bash "$repo/scripts/setup-macos-protection.sh" </dev/null >/dev/null 2>&1; then fail "noninteractive run refused"; fi
echo "wizard fake run: 16 checks passed"
