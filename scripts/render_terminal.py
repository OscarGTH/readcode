#!/usr/bin/env python3
"""Turn a captured ANSI stream into a terminal-window screenshot source.

The bytes are the tool's own output; this only paints them. Long prose lines wrap
at the PTY's width with a two-space continuation indent, which is how the text
reads on screen instead of running off the frame.
"""
import html
import re
import sys

ANSI = re.compile(r"\x1b\[([0-9;]*)m")
CODES = {
    "1": "b",
    "2": "d",
    "31": "red",
    "32": "green",
    "33": "yellow",
    "35": "magenta",
    "36": "cyan",
}


def classes(state):
    found = []
    if state["bold"]:
        found.append("b")
    if state["dim"]:
        found.append("d")
    if state["color"]:
        found.append(f"c-{state['color']}")
    return " ".join(found)


def cells(text):
    """[(char, classes)] with ANSI turned into CSS classes."""
    out = []
    state = {"bold": False, "dim": False, "color": None}
    pos = 0
    for match in ANSI.finditer(text):
        for char in text[pos : match.start()]:
            out.append((char, classes(state)))
        for code in filter(None, match.group(1).split(";")):
            if code == "0":
                state = {"bold": False, "dim": False, "color": None}
            elif code == "1":
                state["bold"] = True
            elif code == "2":
                state["dim"] = True
            elif code in CODES:
                state["color"] = CODES[code]
        pos = match.end()
    for char in text[pos:]:
        out.append((char, classes(state)))
    return out


def words(cells_):
    """Split into words (each keeping its trailing space) and hard breaks."""
    out = []
    current = []
    for char, style in cells_:
        if char == "\r":
            continue
        if char == "\n":
            if current:
                out.append(current)
                current = []
            out.append(None)  # hard line break
        elif char == " ":
            current.append((char, style))
            out.append(current)
            current = []
        else:
            current.append((char, style))
    if current:
        out.append(current)
    return out


def wrap(cells_, width, indent=2):
    lines = []
    current = []
    col = 0
    continued = False

    def flush(next_is_continuation):
        nonlocal current, col, continued
        while current and current[-1][0] == " ":
            current.pop()
        lines.append(current)
        current = [(" ", None)] * indent if next_is_continuation else []
        col = len(current)
        continued = next_is_continuation

    for word in words(cells_):
        if word is None:
            flush(False)
            continued = False
            continue
        while word and word[0][0] == " " and not current:
            word = word[1:]
        if not word:
            continue
        if col + len(word) > width and col > 0:
            flush(True)
            while word and word[0][0] == " ":
                word = word[1:]
        # A single token longer than the line: hard-split it rather than overflow.
        while len(word) > width - col:
            room = width - col
            current.extend(word[:room])
            col += room
            word = word[room:]
            flush(True)
        current.extend(word)
        col += len(word)

    flush(False)
    return lines


def render(lines):
    body = []
    for line in lines:
        spans = []
        for text, style in group(line):
            escaped = html.escape(text).replace(" ", "&nbsp;")
            spans.append(f'<span class="{style}">{escaped}</span>' if style else escaped)
        body.append(f'<div class="ln">{"".join(spans) or "&nbsp;"}</div>')
    return "\n".join(body)


def group(line):
    out = []
    for char, style in line:
        if out and out[-1][1] == style:
            out[-1][0] += char
        else:
            out.append([char, style])
    return [(text, style) for text, style in out]


PAGE = """<!doctype html>
<meta charset="utf-8">
<style>
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0; padding: 22px;
    background: #010409;
    font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas,
                 "Liberation Mono", monospace;
  }}
  .window {{
    width: max-content; min-width: 620px; margin: 0 auto;
    background: #0d1117;
    border: 1px solid #30363d;
    border-radius: 10px;
    overflow: hidden;
    box-shadow: 0 16px 40px rgba(1, 4, 9, .55);
  }}
  .bar {{
    display: flex; align-items: center; gap: 7px;
    padding: 10px 14px; background: #161b22;
    border-bottom: 1px solid #30363d;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
  }}
  .dot {{ width: 11px; height: 11px; border-radius: 50%; }}
  .r {{ background: #ff5f57; }} .y {{ background: #febc2e; }} .g {{ background: #28c840; }}
  .title {{ margin-left: 8px; color: #6e7681; }}
  .body {{
    padding: 15px 18px 17px;
    color: #c9d1d9;
    font-size: 13px; line-height: 1.45;
    white-space: pre;
  }}
  .ln {{ min-height: 18.9px; }}
  .b {{ color: #e6edf3; font-weight: 600; }}
  .d {{ color: #7d8590; }}
  .c-green {{ color: #3fb950; }}
  .c-red {{ color: #f85149; }}
  .c-yellow {{ color: #d29922; }}
  .c-magenta {{ color: #bc8cff; }}
  .c-cyan {{ color: #39c5cf; }}
</style>
<div class="window">
  <div class="bar">
    <span class="dot r"></span><span class="dot y"></span><span class="dot g"></span>
    <span class="title">{title}</span>
  </div>
  <div class="body">{body}</div>
</div>
"""


def main(ans_path, out_path, title, columns):
    raw = open(ans_path, "rb").read().decode("utf-8", "replace")
    lines = wrap(cells(raw), columns)
    while lines and not "".join(char for char, _ in lines[0]).strip():
        lines.pop(0)
    while lines and not "".join(char for char, _ in lines[-1]).strip():
        lines.pop()
    with open(out_path, "w") as handle:
        handle.write(PAGE.format(title=html.escape(title), body=render(lines)))
    widest = max((len(line) for line in lines), default=0)
    print(f"{out_path}: {len(lines)} lines, widest {widest} cols")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]))
