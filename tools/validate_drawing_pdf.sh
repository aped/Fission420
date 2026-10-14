#!/usr/bin/env bash
# External check of an exported drawing PDF with poppler (pdfinfo, pdftotext, pdftoppm):
# the file parses, has one page of the expected size, and contains the expected texts.
# Usage: tools/validate_drawing_pdf.sh <drawing.pdf> <A4|A3> [expected text ...]
# Example (after `cargo test -p rcad-commands --test drawings`):
#   tools/validate_drawing_pdf.sh artifacts/drawings/sheets/block.pdf A4 40 30 10 Ø8 "THIRD ANGLE"
# Writes <drawing>.png (100 dpi render) next to the PDF for visual inspection.
set -euo pipefail
pdf="$1"
size="$2"
shift 2
for t in pdfinfo pdftotext pdftoppm; do
  command -v "$t" >/dev/null || { echo "SKIP: $t not installed"; exit 0; }
done
info="$(pdfinfo "$pdf")"
pages="$(awk '/^Pages:/ {print $2}' <<<"$info")"
[ "$pages" = 1 ] || { echo "FAIL: $pages pages"; exit 1; }
case "$size" in
  A4) want="841.89 x 595.276" ;;
  A3) want="1190.55 x 841.89" ;;
  *) echo "unknown size $size"; exit 2 ;;
esac
grep -q "Page size: *$want pts" <<<"$info" || { echo "FAIL: page size"; grep "Page size" <<<"$info"; exit 1; }
text="$(pdftotext -layout "$pdf" -)"
for t in "$@"; do
  grep -qF -- "$t" <<<"$text" || { echo "FAIL: text '$t' not found"; exit 1; }
done
pdftoppm -r 100 -png -singlefile "$pdf" "${pdf%.pdf}"
echo "OK: $pdf ($size, ${#@} texts found; render ${pdf%.pdf}.png)"
