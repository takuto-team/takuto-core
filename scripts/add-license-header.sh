#!/usr/bin/env bash
set -euo pipefail

# Idempotent SPDX license header sweep.
#
# Walks `crates/**/*.rs` and `ui/src/**/*.{ts,tsx}` and prepends
#   // SPDX-License-Identifier: FSL-1.1-ALv2
#   <blank line>
# to any file that does NOT already contain "SPDX-License-Identifier"
# or "FSL" in its first 20 lines.
#
# Safe to run repeatedly: files that already carry an FSL or SPDX header
# are skipped. Files that have a bare `Copyright (C) ...` header without
# the FSL clause are NOT skipped — the SPDX line is prepended above them.

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

SPDX_LINE='// SPDX-License-Identifier: FSL-1.1-ALv2'

added=0
skipped=0

prepend_spdx() {
  local file="$1"

  # Skip if SPDX or FSL already present in first 20 lines.
  if head -n 20 "$file" | grep -q -E 'SPDX-License-Identifier|FSL'; then
    echo "skipped: $file"
    skipped=$((skipped + 1))
    return 0
  fi

  local tmp
  tmp="$(mktemp)"
  {
    printf '%s\n\n' "$SPDX_LINE"
    cat "$file"
  } > "$tmp"
  mv "$tmp" "$file"

  echo "added: $file"
  added=$((added + 1))
}

# Rust sources.
while IFS= read -r -d '' file; do
  prepend_spdx "$file"
done < <(find crates -type f -name '*.rs' -print0)

# UI sources.
while IFS= read -r -d '' file; do
  prepend_spdx "$file"
done < <(find ui/src -type f \( -name '*.ts' -o -name '*.tsx' \) -print0)

echo ""
echo "Summary: $added added, $skipped skipped."
