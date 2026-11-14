#!/usr/bin/env bash
# Regenerate the Windows application icon from the vector artwork the Linux
# and macOS bundles use. The result is committed, because the Windows runner
# has no SVG rasterizer; run this again whenever the artwork changes.

set -euo pipefail

cd "$(dirname "$0")/.."

SOURCE=data/icons/hicolor/scalable/apps/com.restartfu.Xd.svg
OUTPUT=installer/windows/xd.ico

for command in rsvg-convert magick; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "build-windows-icon.sh requires $command." >&2
    exit 1
  }
done

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# The sizes Explorer, the taskbar, Alt+Tab and the title bar ask for at 100%
# to 250% scaling. Each is rendered from the vector, not scaled from a bitmap.
frames=()
for size in 16 20 24 32 40 48 64 96 256; do
  rsvg-convert --width "$size" --height "$size" "$SOURCE" > "$WORK/$size.png"
  frames+=("$WORK/$size.png")
done

magick "${frames[@]}" "$OUTPUT"
echo "Windows icon: $OUTPUT"
