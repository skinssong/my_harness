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


def resize(path: pathlib.Path) -> str:
    svg = path.read_text()

    view_box = re.search(r'viewBox="[\d.\-]+ [\d.\-]+ ([\d.]+) ([\d.]+)"', svg)
    if not view_box:
        return f"{path.name}: no viewBox, skipped"
    natural_w, natural_h = float(view_box.group(1)), float(view_box.group(2))

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
    path.write_text(svg)
    return f"{path.name}: {round(width)}x{round(height)}"


def main() -> None:
    for path in sorted(pathlib.Path(__file__).parent.glob("*.svg")):
        print(resize(path))


if __name__ == "__main__":
    main()
