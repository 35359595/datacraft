#!/bin/zsh
# Subsets Inter and JetBrains Mono (OFL) to the useful Unicode ranges, without hinting
# (egui doesn't use it). Source: the fonts bundled with DataGrip, or the ones given in $1.
set -euo pipefail
cd "${0:A:h}/.."
SRC="${1:-/Applications/DataGrip.app/Contents/jbr/Contents/Home/lib/fonts}"
RANGES="U+0020-007E,U+00A0-024F,U+0300-036F,U+0370-03FF,U+0400-04FF,U+2000-206F,U+20A0-20CF,U+2100-214F,U+2190-21FF,U+2200-22FF,U+2300-23FF,U+2500-25FF"
for f in Inter-Regular.otf Inter-SemiBold.otf JetBrainsMono-Regular.ttf JetBrainsMono-Bold.ttf JetBrainsMono-Italic.ttf; do
  python3 -m fontTools.subset "$SRC/$f" --unicodes="$RANGES" --no-hinting --layout-features='' \
    --name-IDs='*' --notdef-outline --output-file="assets/fonts/$f"
done
ls -la assets/fonts
