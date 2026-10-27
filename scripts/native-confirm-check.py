"""Shows one invented action through the native confirmation helper and checks its signed answer.

Usage: python3 -I scripts/native-confirm-check.py HELPER
Prints only the verified outcome. No account data, credential or secret is involved.
"""
import base64, hashlib, hmac, json, os, subprocess, sys

def main(helper):
    digest = "0" * 64
    action = {"purpose": "invoice_send", "account": "wizard-check", "revision": 1, "report_version": "check",
              "target_name": "nobody", "target": "nobody@example.invalid", "sending_identity": "check@example.invalid",
              "source": "none", "content_digest": digest}
    display = ("Send this invoice for account wizard-check to nobody <nobody@example.invalid> from "
               f"check@example.invalid. Exact content SHA-256 {digest}. No Undo.")
    request = json.dumps({"request_id": os.urandom(32).hex(), "protocol": "mailroom-exact-action-v1", "actions": [action],
                          "plan_digest": os.urandom(32).hex(), "display": display, "expires_after_seconds": 300,
                          "synthetic": False}, separators=(",", ":")).encode()
    challenge = os.urandom(32)
    envelope = json.dumps({"request": base64.b64encode(request).decode(), "challenge": base64.b64encode(challenge).decode()})
    run = subprocess.run([helper], input=envelope.encode(), capture_output=True, timeout=300, env={})
    if run.returncode != 0:
        print("outcome=helper-refused")
        return 1
    wire = json.loads(run.stdout)
    request_id = json.loads(request)["request_id"]
    digest = hashlib.sha256(request).hexdigest()
    message = b"mailroom-exact-action-outcome-v1\0" + json.dumps([request_id, digest, wire["outcome"]], separators=(",", ":")).encode()
    expected = hmac.new(challenge, message, hashlib.sha256).hexdigest()
    if not hmac.compare_digest(expected, wire["authentication"]) or wire["request_digest"] != digest:
        print("outcome=unauthenticated")
        return 1
    print(f"outcome={wire['outcome']}")
    return 0

if __name__ == "__main__":
    sys.exit(main(sys.argv[1]) if len(sys.argv) == 2 else 2)
