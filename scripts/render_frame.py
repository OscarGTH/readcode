#!/usr/bin/env python3
"""Turn a TUI frame (cells from ratatui's test backend) into a screenshot page.

The cells come from `examples/frame.rs`, so the picture is the app's own drawing,
not a mock-up. The window chrome matches `render_terminal.py`, so the interactive
and line-based screenshots look like the same program.

  python3 scripts/render_frame.py /tmp/frame.json docs/frame.html "readcode · line by line"
"""
import html
import json
import sys

PAGE = """<!doctype html>
<meta charset="utf-8">
<style>
  * { box-sizing: border-box; }
  body {
    margin: 0; padding: 22px;
    background: #010409;
    font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas,
                 "Liberation Mono", monospace;
  }
  .window {
    width: max-content; margin: 0 auto;
    background: #0d1117;
    border: 1px solid #30363d;
    border-radius: 10px;
    overflow: hidden;
    box-shadow: 0 16px 40px rgba(1, 4, 9, .55);
  }
  .bar {
    display: flex; align-items: center; gap: 7px;
    padding: 10px 14px; background: #161b22;
    border-bottom: 1px solid #30363d;
    font-size: 12px;
  }
  .dot { width: 11px; height: 11px; border-radius: 50%; }
  .r { background: #ff5f57; } .y { background: #febc2e; } .g { background: #28c840; }
  .title { margin-left: 8px; color: #6e7681; }
  .body {
    padding: 12px 14px;
    color: #c9d1d9;
    font-size: 12.5px; line-height: 1.34;
    white-space: pre;
  }
  .ln { min-height: 16.75px; }
</style>
<div class="window">
  <div class="bar">
    <span class="dot r"></span><span class="dot y"></span><span class="dot g"></span>
    <span class="title">{title}</span>
  </div>
  <div class="body">{body}</div>
</div>
"""


def span(run):
    style = []
    if run.get("fg"):
        style.append(f"color:{run['fg']}")
    if run.get("bg"):
        style.append(f"background:{run['bg']}")
    if run.get("b"):
        style.append("font-weight:600")
    if run.get("i"):
        style.append("font-style:italic")
    text = html.escape(run["t"]).replace(" ", "&nbsp;")
    if not style:
        return text
    return f'<span style="{";".join(style)}">{text}</span>'


def main(frame_path, out_path, title):
    frame = json.load(open(frame_path))
    rows = []
    blanks = []
    for row in frame["rows"]:
        text = "".join(run["t"] for run in row)
        body = "".join(span(run) for run in row) or "&nbsp;"
        rows.append(f'<div class="ln">{body}</div>')
        blanks.append(not text.strip())
    # Trailing blank rows add nothing to look at.
    while rows and blanks[-1]:
        rows.pop()
        blanks.pop()
    page = PAGE.replace("{title}", html.escape(title)).replace("{body}", "".join(rows))
    with open(out_path, "w") as handle:
        handle.write(page)
    print(f"{out_path}: {len(rows)} rows of {frame['width']} cols")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], sys.argv[3])
