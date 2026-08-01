#!/usr/bin/env bash
# Dump RPFTool's view of the OPEN-encrypted archives it ships as fixtures.
#
# RPFTool is a prior, independent TypeScript reader for OPEN archives. Every
# retail GTA V archive is NG-encrypted, so without this the OPEN path in
# rpf/file.rs is never exercised against anything but itself.
#
#   bash scripts/rpftool-oracle.sh
#   cargo test -p cw-core --test rpf_gate -- --ignored --nocapture
#
# Output lands in fixtures/, which is gitignored.
set -euo pipefail

RPFTOOL="${RPFTOOL:-E:/Ad Astra Tools/RPFTool}"
OUT="${OUT:-fixtures/rpftool}"

mkdir -p "$OUT"
manifest="$OUT/index.json"
: > "$manifest.tmp"

echo "[" >> "$manifest.tmp"
first=1
while IFS= read -r rpf; do
  name=$(basename "$(dirname "$rpf")")
  echo "dumping $name ($rpf)"
  (cd "$RPFTOOL" && bun run src/cli.ts list "$rpf" --json) > "$OUT/$name.json"
  [ $first -eq 0 ] && echo "," >> "$manifest.tmp"
  first=0
  printf '  {"name": "%s", "rpf": "%s"}' "$name" "$RPFTOOL/$rpf" >> "$manifest.tmp"
done < <(cd "$RPFTOOL" && find incoming -iname '*.rpf' | sort)
printf '\n]\n' >> "$manifest.tmp"

mv "$manifest.tmp" "$manifest"
echo "wrote $manifest"
