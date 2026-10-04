#!/usr/bin/env bash
# Regenerate TypeScript announcement ACP types.
set -euo pipefail
shopt -s nullglob

CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Desktop consumer lives a few levels up from this crate in the workspace tree.
REPO_ROOT="$(cd "$CRATE_DIR/../../.." && pwd)"
DESKTOP_DIR="$REPO_ROOT/frontend/apps/grok-desktop"
DST="$DESKTOP_DIR/src/acp/generated"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "[generate] 1/3 exporting ts-rs bindings (cargo test --features ts) …"
(cd "$CRATE_DIR" && TS_RS_EXPORT_DIR="$TMP" cargo test --quiet --features ts)

# Never clobber the committed bindings unless the export produced some.
bindings=("$TMP"/*.ts)
[[ ${#bindings[@]} -gt 0 ]] || {
  echo "[generate] no bindings exported — aborting before touching $DST" >&2
  exit 1
}

echo "[generate] 2/3 copying ${#bindings[@]} bindings -> $DST"
mkdir -p "$DST"
rm -f "$DST"/*.ts
for f in "${bindings[@]}"; do
  {
    echo "// generated — do NOT edit by hand."
    echo "// Regenerate via generate.sh in the xai-grok-announcements package."
    cat "$f"
  } >"$DST/$(basename "$f")"
done

echo "[generate] 3/3 formatting (oxfmt) …"
(cd "$DESKTOP_DIR" && pnpm exec oxfmt --write src/acp/generated)
echo "[generate] done — ${#bindings[@]} bindings."
