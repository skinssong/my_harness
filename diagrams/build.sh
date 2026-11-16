#!/bin/sh
# Render every diagram from its source, then give it a definite size.
#
# Rendering from the .mmd files rather than pasting syntax into a tool call is
# the difference between a diagram that matches its source and one that only
# looked right when it was drawn.
set -e
cd "$(dirname "$0")"
MMDC="${MMDC:-/opt/homebrew/lib/node_modules/mermaid-render-mcp/node_modules/.bin/mmdc}"

# mmdc's bundled puppeteer wants a chrome-headless-shell build that is not in
# its cache here. puppeteer.json points it at the installed Chrome instead.
for source in *.mmd; do
    "$MMDC" -i "$source" -o "${source%.mmd}.svg" -b white \
        -p puppeteer.json --quiet
done

python3 size.py
