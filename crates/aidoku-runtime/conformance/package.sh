#!/usr/bin/env bash
# Builds the conformance guest and packages it as a .aix, using the layout
# observed in real community packages (ADR-0004).
set -euo pipefail
cd "$(dirname "$0")"

cargo build --release --target wasm32-unknown-unknown
OUT="${1:-nyuka-conformance.aix}"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

mkdir -p "$STAGE/Payload"
cp target/wasm32-unknown-unknown/release/nyuka_conformance.wasm "$STAGE/Payload/main.wasm"
cat > "$STAGE/Payload/source.json" <<'JSON'
{
  "info": {
    "id": "test.conformance",
    "name": "Host Conformance",
    "version": 1,
    "url": "https://localhost",
    "contentRating": 0,
    "languages": ["en"]
  }
}
JSON
echo '[]' > "$STAGE/Payload/filters.json"

rm -f "$OUT"
( cd "$STAGE" && zip -q -r - Payload ) > "$OUT"
printf 'packaged %s (%s bytes)\n' "$OUT" "$(stat -c%s "$OUT")"
