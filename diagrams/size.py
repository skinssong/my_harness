#!/usr/bin/env python3
"""Give every rendered diagram a definite size.

Mermaid writes `width="100%"` plus a `max-width` on the svg element, which
leaves the rendered size up to the container — and in a narrow document column
that means small text. This sets explicit pixel dimensions from each diagram's
own viewBox, so a diagram always renders at the size it was drawn at, and only
shrinks if the column is genuinely narrower.

Run after rendering:

    python3 diagrams/size.py
"""

import pathlib
import re

# Diagrams taller than they are wide look absurd at their full width, so they
# get a width that keeps the text legible without three screens of scrolling.
WIDTH_OVERRIDES = {
    "arc.svg": 560,
    "s1-dataflow.svg": 620,
}


MARKER = "<!-- sized by diagrams/size.py -->"


def resize(path: pathlib.Path) -> str:
    svg = path.read_text()
    if MARKER in svg:
        return f"{path.name}: already sized"

    view_box = re.search(
        r'viewBox="([\d.\-]+) ([\d.\-]+) ([\d.]+) ([\d.]+)"', svg
    )
    if not view_box:
        return f"{path.name}: no viewBox, skipped"
    left, top = view_box.group(1), view_box.group(2)
    natural_w, natural_h = float(view_box.group(3)), float(view_box.group(4))

    # Mermaid under-measures height when the deepest node is an edge label's
    # partner, which clips its bottom border. A small tail fixes it.
    margin = max(24.0, natural_h * 0.02)
    natural_h += margin
    svg = re.sub(
        r'viewBox="[\d.\-]+ [\d.\-]+ [\d.]+ [\d.]+"',
        f'viewBox="{left} {top} {natural_w} {natural_h}"',
        svg,
        count=1,
    )

    width = WIDTH_OVERRIDES.get(path.name, natural_w)
    height = natural_h * (width / natural_w)

    # Drop the container-dependent cap, wherever it is declared on the root.
    svg = re.sub(r'max-width:\s*[\d.]+px;\s*', "", svg, count=1)
    svg = re.sub(r'<svg([^>]*?)\swidth="[^"]*"', r"<svg\1", svg, count=1)
    svg = re.sub(r'<svg([^>]*?)\sheight="[^"]*"', r"<svg\1", svg, count=1)
    svg = re.sub(
        r"<svg",
        f'<svg width="{round(width)}" height="{round(height)}"',
        svg,
        count=1,
    )
    # Stamp it, so a second run does not add a second margin.
    svg = svg.replace("<svg", f"{MARKER}<svg", 1) if MARKER not in svg else svg
    path.write_text(svg)
    return f"{path.name}: {round(width)}x{round(height)}"


def main() -> None:
    for path in sorted(pathlib.Path(__file__).parent.glob("*.svg")):
        print(resize(path))


if __name__ == "__main__":
    main()
