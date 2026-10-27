"""Runs a wizard driver script under bash in a pseudo-terminal and answers its prompts.

Usage: python3 -I drive-wizard.py DRIVER TRANSCRIPT ANSWERS [SILENCE_SECONDS]

ANSWERS holds "prompt<TAB>answer" lines. Each prompt is answered once it appears after the
last answer. The child is killed after SILENCE_SECONDS without output (default 20). The
exit code is the child's.
"""
import os
import pty
import select
import signal
import sys

driver, transcript, answers = sys.argv[1:4]
silence = float(sys.argv[4]) if len(sys.argv) > 4 else 20
rules = [line.split("\t", 1) for line in open(answers).read().splitlines() if "\t" in line]
rules = [(prompt.encode(), answer) for prompt, answer in rules] + [(b"Ready to start?", "")]
pid, fd = pty.fork()
if pid == 0:
    os.execv("/bin/bash", ["/bin/bash", driver])
seen, answered = b"", 0
while True:
    ready, _, _ = select.select([fd], [], [], silence)
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
open(transcript, "wb").write(seen)
sys.exit(os.waitstatus_to_exitcode(status))
