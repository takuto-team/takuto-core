#!/usr/bin/bash
# Run as root from entrypoint. Merges optional project skills into Claude/Cursor volume paths.
# Sources (later wins for the same top-level name):
#   1) /opt/maestro/project-skills-baked  — copied at image build from ./skills if present
#   2) /opt/maestro/project-skills-host  — bind-mounted from ./skills at compose up (often gitignored)
# Only names present under those dirs are replaced; other skill folders already on the volumes stay.
set -euo pipefail

DESTS=(/home/maestro/.claude/skills /home/maestro/.cursor/skills /home/maestro/.cursor/skills-cursor)
for D in "${DESTS[@]}"; do
  mkdir -p "$D"
done

apply_layer() {
  local SRC=$1
  [[ -d "$SRC" ]] || return 0
  [[ -n "$(ls -A "$SRC" 2>/dev/null)" ]] || return 0

  local path bn
  while IFS= read -r -d '' path; do
    bn=$(basename "$path")
    [[ "$bn" == "." || "$bn" == ".." ]] && continue
    for D in "${DESTS[@]}"; do
      rm -rf "$D/$bn"
      cp -a "$path" "$D/$bn"
    done
  done < <(find "$SRC" -mindepth 1 -maxdepth 1 -print0)
}

apply_layer /opt/maestro/project-skills-baked
apply_layer /opt/maestro/project-skills-host

chown -R maestro:maestro "${DESTS[@]}" || true

if [[ -n "$(ls -A /opt/maestro/project-skills-baked 2>/dev/null)" ]] || [[ -n "$(ls -A /opt/maestro/project-skills-host 2>/dev/null)" ]]; then
  echo "[maestro] Merged project skills into:"
  for D in "${DESTS[@]}"; do
    echo "[maestro]   $D"
  done
fi
