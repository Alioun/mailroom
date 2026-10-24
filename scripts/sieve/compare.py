import json
from pathlib import Path
import re
import shutil
import subprocess


def run(args):
    result = subprocess.run(args, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f"Synthetic engine check failed: {args[0]}\n{result.stdout}\n{result.stderr}")
    return result.stdout, result.stderr


count = 0
for directory in sorted(Path('/fixtures').iterdir()):
    if not directory.is_dir():
        continue
    script = Path('/tmp/rules.sieve')
    shutil.copyfile(directory / 'rules.sieve', script)
    run(['check-sieve', str(script)])
    expected = json.loads((directory / 'expected.json').read_text())
    for i, decision in enumerate(expected):
        trace = Path('/tmp/trace')
        trace.unlink(missing_ok=True)
        output, errors = run(['sieve-test', '-C', '-c', '/harness/dovecot.conf', '-t', str(trace), str(script), str(directory / f'{i}.eml')])
        if 'error:' in errors.lower() or 'error:' in output.lower():
            raise RuntimeError(f"Pigeonhole reported an error in {directory.name}/{i}")
        folders = re.findall(r'store message in folder: (.+)', output)
        destination = decision['destination']
        actual = folders or ['INBOX']
        expected_folder = destination.get('name', 'INBOX')
        assert actual == [expected_folder], (directory.name, i, actual, expected_folder)
        targets = re.findall(r'redirect message to: <([^>]+)>', output)
        assert targets == [c['recipient'] for c in decision['copies']], (directory.name, i, targets)
        trace_text = trace.read_text()
        seen = '\\seen' in output.lower() or '\\seen' in trace_text.lower()
        assert seen == decision['mark_read'], (directory.name, i, 'Seen action disagreement')
        count += 1
print(f'PASS: check-sieve lint and Pigeonhole fresh-cache filing/copy/Seen semantics for {count} synthetic executions across eight variants.')
print('UNVERIFIED: persistent duplicate-cache true/expiry behavior, real provider recipient provenance, separator, capabilities and accounting acceptance.')
