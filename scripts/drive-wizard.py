"""Runs a wizard driver script under bash in a pseudo-terminal and answers its prompts.

Usage: python3 -I drive-wizard.py DRIVER TRANSCRIPT ANSWERS [SILENCE_SECONDS]

ANSWERS holds "prompt<TAB>answer" lines. Each prompt is answered once it appears after the
last answer. After SILENCE_SECONDS without output (default 20), or on SIGTERM, the
driver kills the child's whole process group, which pty.fork makes a session of its own,
so build tools the child started stop with it. The exit code is the child's.
macOS answers EPERM, not ESRCH, for a group whose members are all exiting.
"""
import fcntl
import os
import pty
import select
import signal
import struct
import sys
import termios

driver, transcript, answers = sys.argv[1:4]
silence = float(sys.argv[4]) if len(sys.argv) > 4 else 20
rules = [line.split("\t", 1) for line in open(answers).read().splitlines() if "\t" in line]
rules = [(prompt.encode(), answer) for prompt, answer in rules] + [(b"Ready to start?", "")]
pid, fd = pty.fork()
if pid == 0:
    # A wide terminal keeps Readline from scrolling a long answer, which would redraw the
    # prompt and answer it a second time.
    fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 300, 0, 0))
    os.execv("/bin/bash", ["/bin/bash", driver])


def stop(*_):
    try:
        os.killpg(pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pass


signal.signal(signal.SIGTERM, lambda *_: (stop(), os.waitpid(pid, 0), os._exit(3)))
seen, answered = b"", 0
while True:
    ready, _, _ = select.select([fd], [], [], silence)
    if not ready:
        stop()
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
stop()
open(transcript, "wb").write(seen)
sys.exit(os.waitstatus_to_exitcode(status))
