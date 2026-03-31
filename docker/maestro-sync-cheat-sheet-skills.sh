#!/usr/bin/bash
# Installed in the image; invoked only as: sudo -n /usr/bin/bash /usr/local/bin/maestro-sync-cheat-sheet-skills.sh
# (must be root so mkdir/rm/cp work on root-owned named volumes; then chown to maestro).
set -euo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
  echo "maestro-sync-cheat-sheet-skills.sh: must run as root (use sudo /usr/bin/bash $0)" >&2
  exit 1
fi

echo "[maestro] sync-cheat-sheet-skills: uid=$(id -u) euid=$(id -u)"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
curl -fsSL "https://github.com/morphet81/cheat-sheets/archive/refs/heads/main.tar.gz" | tar -xzf - -C "$TMP"
SK="$TMP/cheat-sheets-main/skills"
if [[ ! -d "$SK" ]]; then
  echo "Expected skills directory missing: $SK" >&2
  ls -la "$TMP" >&2
  exit 1
fi

DESTS=(/home/maestro/.claude/skills /home/maestro/.cursor/skills /home/maestro/.cursor/skills-cursor)
for D in "${DESTS[@]}"; do
  mkdir -p "$D"
done

while IFS= read -r -d '' path; do
  bn=$(basename "$path")
  for D in "${DESTS[@]}"; do
    rm -rf "$D/$bn"
    if [[ -d "$path" ]]; then
      mkdir -p "$D/$bn"
      cp -a "$path"/. "$D/$bn"/
    else
      cp -a "$path" "$D/$bn"
    fi
  done
done < <(find "$SK" -mindepth 1 -maxdepth 1 -print0)

chown -R maestro:maestro "${DESTS[@]}"
echo "[maestro] sync-cheat-sheet-skills: done."
