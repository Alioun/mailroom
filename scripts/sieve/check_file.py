import shutil
import subprocess
import sys
from pathlib import Path


def run(args):
    result = subprocess.run(args, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(
            f"Independent Sieve check failed ({args[0]}):\n{result.stdout}\n{result.stderr}"
        )
    return result.stdout, result.stderr


source = Path("/input/rules.sieve")
if not source.is_file():
    raise RuntimeError("Missing /input/rules.sieve")

script = Path("/tmp/rules.sieve")
compiled = Path("/tmp/rules.svbin")
shutil.copyfile(source, script)

run(["check-sieve", str(script)])
out, err = run(["sievec", "-c", "/harness/dovecot.conf", str(script), str(compiled)])
if "error:" in out.lower() or "error:" in err.lower():
    raise RuntimeError(f"Pigeonhole sievec reported errors:\n{out}\n{err}")

eml_dir = Path("/samples")
tested = 0
if eml_dir.is_dir():
    for eml in sorted(eml_dir.glob("*.eml")):
        trace = Path("/tmp/trace")
        trace.unlink(missing_ok=True)
        out, err = run(
            [
                "sieve-test",
                "-C",
                "-c",
                "/harness/dovecot.conf",
                "-t",
                str(trace),
                str(script),
                str(eml),
            ]
        )
        if "error:" in out.lower() or "error:" in err.lower():
            raise RuntimeError(f"Pigeonhole sieve-test error on {eml.name}:\n{out}\n{err}")
        tested += 1

print(
    f"PASS: check-sieve lint and Dovecot Pigeonhole sievec compiled {source.name} cleanly"
    + (f" ({tested} sample .eml message(s) dry-run without errors)." if tested else ".")
)
