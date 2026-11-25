#!/bin/sh
# Stage the host-only layout engine, outline converter, and matching fonts.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
src="$root/tools/mermaid"
out=${1:?usage: stage-mermaid.sh <runtime/mermaid directory>}
(cd "$src" && npm ci --ignore-scripts --no-audit --no-fund)
mkdir -p "$out"
cp "$src/render.cjs" "$src/layout.js" "$src/LICENSE-resvg.txt" "$src/node_modules/mermaid/dist/mermaid.min.js" "$out/"
cp "$src/node_modules/mermaid/LICENSE" "$out/LICENSE-mermaid.txt"
cp "$src/node_modules/@resvg/resvg-wasm/index.js" "$out/resvg.cjs"
cp "$src/node_modules/@resvg/resvg-wasm/index_bg.wasm" "$out/"
fonts="$root/crates/yaac-app/assets/fonts"
cp "$fonts/UbuntuSansNerdFontPropo-Regular.ttf" "$out/font.ttf"
cp "$fonts/UbuntuSansNerdFontPropo-Bold.ttf" "$out/bold.ttf"
cp "$fonts"/LICENSE-* "$out/"
