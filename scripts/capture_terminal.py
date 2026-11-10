#!/usr/bin/env python3
"""Run readcode under a real PTY and capture exactly what a terminal shows.

A screenshot has to be honest: the tool's own output, with its own colours and
the user's typing echoed by the tty, not a hand-composed transcript.

  CAPTURE_CMD="target/debug/readcode --snippet 004 --blocks" \
  CAPTURE_OUT=/tmp/asking.ans python3 rc_capture.py < answers.txt
"""
import fcntl
import os
import pty
import select
import struct
import sys
import termios
import time

COLS, ROWS = 96, 44
PROMPT_TAIL = b">\x1b[0m "
TIMEOUT = 180.0


def capture(argv, answers):
    pid, fd = pty.fork()
    if pid == 0:  # child
        os.environ["TERM"] = "xterm-256color"
        os.environ["COLUMNS"] = str(COLS)
        os.environ["LINES"] = str(ROWS)
        os.execvp(argv[0], argv)

    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    out = bytearray()
    pending = list(answers)
    deadline = time.time() + TIMEOUT

    while time.time() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.25)
        if not ready:
            continue
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            break
        if not chunk:
            break
        out.extend(chunk)
        # Answer once the prompt is on screen and nothing is half-typed.
        if pending and (out.endswith(PROMPT_TAIL) or out.endswith(b"> ")):
            time.sleep(0.35)  # let the prompt land before typing into it
            os.write(fd, pending.pop(0).encode() + b"\r")
            time.sleep(0.2)

    try:
        os.close(fd)
    except OSError:
        pass
    try:
        os.waitpid(pid, 0)
    except ChildProcessError:
        pass
    return bytes(out)


if __name__ == "__main__":
    argv = os.environ.get("CAPTURE_CMD", "target/debug/readcode").split()
    answers = [line.rstrip("\n") for line in sys.stdin if line.strip()]
    data = capture(argv, answers)
    out = os.environ.get("CAPTURE_OUT", "/tmp/capture.ans")
    with open(out, "wb") as handle:
        handle.write(data)
    print(f"captured {len(data)} bytes -> {out}", file=sys.stderr)
