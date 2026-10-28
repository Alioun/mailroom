"""Credential-free checks for the owner setup wrapper. No TTY, helper or Keychain opens."""
import json
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
script = (root / "scripts/setup-macos-protection.sh").read_text()
def body(name, next_name):
    return name + "() {" + script.split(name + "() {", 1)[1].split("\n" + next_name, 1)[0]

allowed = body("allowed", "valid_reference")
wrapper = body("mailroom", "note_result")
dummy = "OWNER-SETUP-DUMMY-6b92af"
results = []
with tempfile.TemporaryDirectory(prefix="account-setup-wrapper-") as directory:
    work = Path(directory)
    binary = work / "mailroom"
    binary.write_text("""#!/bin/bash
printf '%s\\n' "$@" >"$(dirname "$0")/argv"
scenario=$(<"$(dirname "$0")/scenario")
case "$scenario" in
  created) echo status=created ;;
  replaced) echo status=replaced ;;
  forgotten) echo status=forgotten ;;
  rotated) echo status=rotated ;;
  recovered) echo status=recovered ;;
  cancelled) echo status=cancelled; exit 1 ;;
  denied) echo status=denied; exit 1 ;;
  wrong_exit) echo status=created; exit 1 ;;
  extra_output) echo OWNER-SETUP-DUMMY-6b92af; echo status=created ;;
  private_stderr) echo OWNER-SETUP-DUMMY-6b92af >&2; echo status=created ;;
  shaped_output) echo status=OWNER-SETUP-DUMMY-6b92af; exit 1 ;;
esac
""")
    binary.chmod(0o700)
    driver = work / "driver.sh"
    driver.write_text("set -euo pipefail\n" + allowed + "\n" + wrapper + """
trusted_path() { return 0; }
LIBEXEC=$1
CODESIGN=/usr/bin/true
MAILROOM_CDHASH=0000000000000000000000000000000000000000
EXPECTED=; RESULT=; REFS=; DROPPED=0
mailroom account "$2" "$3"
printf '%s\\n' "$RESULT"
""")
    for command, argument, scenario, expected in [
        ("add", "gmail", "created", "status=created"),
        ("replace-password", "invented-account", "replaced", "status=replaced"),
        ("add", "gmail", "replaced", "status=unexpected"),
        ("replace-password", "invented-account", "created", "status=unexpected"),
        ("add", "gmail", "cancelled", "status=cancelled"),
        ("add", "gmail", "denied", "status=denied"),
        ("add", "gmail", "wrong_exit", "status=unexpected"),
        ("add", "gmail", "extra_output", "status=unexpected"),
        ("add", "gmail", "private_stderr", "status=created"),
        ("add", "gmail", "shaped_output", "status=unexpected"),
        ("forget", "invented-account", "forgotten", "status=forgotten"),
        ("rotate-key", "invented-account", "rotated", "status=rotated"),
        ("recover-local", "invented-account", "recovered", "status=recovered"),
        ("forget", "invented-account", "rotated", "status=unexpected"),
        ("rotate-key", "invented-account", "recovered", "status=unexpected"),
        ("recover-local", "invented-account", "forgotten", "status=unexpected"),
    ]:
        (work / "scenario").write_text(scenario)
        completed = subprocess.run(["/bin/bash", str(driver), str(work), command, argument], capture_output=True, env={"PATH":"/usr/bin:/bin", "HOME":str(work)}, timeout=5)
        assert completed.returncode == 0, "Dummy wrapper failed"
        assert completed.stdout.decode().strip() == expected, "Unexpected fixed record"
        assert dummy.encode() not in completed.stdout + completed.stderr, "Dummy upstream value escaped"
        assert (work / "argv").read_text().splitlines() == ["account", command, argument], "Credential or extra argument passed"
        results.append({"command":command,"scenario":scenario,"passed":True})

owner = script.split("owner_phase() {", 1)[1].split("\nselect_saved_account() {",1)[0]
assert '/bin/mkdir -p -m 700 "$HOME/.config/mailroom"' in script
maintenance = script.split("maintenance_phase() {",1)[1]
assert "TOTAL_STAGES=4" in maintenance and maintenance.count('stage "') == 4
assert maintenance.index("check_frozen") < maintenance.index("mailroom account recover-local") < maintenance.index("mailroom account rotate-key") < maintenance.index("mailroom account forget")
assert "TOTAL_STAGES=8" in owner and owner.count('stage "') == 8
assert owner.index('mailroom account add') < owner.index('mailroom keychain refs')
assert owner.index('expected_references ||', owner.index('mailroom account add')) < owner.index('mailroom keychain refs')
assert owner.index('expected_references ||', owner.index('mailroom account replace-password')) < owner.index('stage "Check the prompt')
connect = (root / "scripts/connect-account.sh").read_text()
assert "hidden UI field" not in connect and "Connect account privately" not in connect
assert "captures no credentials" in connect
for name in ["setup-macos-protection.sh", "connect-account.sh"]:
    assert subprocess.run(["/bin/bash", "-n", str(root / "scripts" / name)], capture_output=True, timeout=5).returncode == 0
print(json.dumps({"wrapperCases":results,"stageOrder":True,"syntax":True,"limits":"Injected unsigned dummy executable and launch verifier. No host trust, TTY, native or Keychain acceptance."}))
