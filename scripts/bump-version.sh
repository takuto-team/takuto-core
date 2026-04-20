#!/usr/bin/env bash
set -euo pipefail

# Bump the Maestro version interactively.
# Updates VERSION, Cargo.toml workspace, ui/package.json, then commits, tags, and pushes.

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION_FILE="$REPO_ROOT/VERSION"

if [ ! -f "$VERSION_FILE" ]; then
  echo "ERROR: VERSION file not found at $VERSION_FILE" >&2
  exit 1
fi

CURRENT=$(cat "$VERSION_FILE" | tr -d '[:space:]')
echo "Current version: $CURRENT"

# Parse semver
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

echo ""
echo "What kind of bump?"
echo "  1) patch  ($MAJOR.$MINOR.$((PATCH + 1)))"
echo "  2) minor  ($MAJOR.$((MINOR + 1)).0)"
echo "  3) major  ($((MAJOR + 1)).0.0)"
echo ""
read -rp "Choose [1/2/3]: " CHOICE

case "$CHOICE" in
  1|patch)
    NEW="$MAJOR.$MINOR.$((PATCH + 1))"
    ;;
  2|minor)
    NEW="$MAJOR.$((MINOR + 1)).0"
    ;;
  3|major)
    NEW="$((MAJOR + 1)).0.0"
    ;;
  *)
    echo "Invalid choice." >&2
    exit 1
    ;;
esac

echo ""
echo "Bumping $CURRENT → $NEW"
echo ""

# 1. Update VERSION file
echo "$NEW" > "$VERSION_FILE"

# 2. Update Cargo.toml workspace version
sed -i.bak "s/^version = \"$CURRENT\"/version = \"$NEW\"/" "$REPO_ROOT/Cargo.toml"
rm -f "$REPO_ROOT/Cargo.toml.bak"

# 3. Update ui/package.json version
if [ -f "$REPO_ROOT/ui/package.json" ]; then
  # Use node for reliable JSON editing
  node -e "
    const fs = require('fs');
    const p = JSON.parse(fs.readFileSync('$REPO_ROOT/ui/package.json', 'utf8'));
    p.version = '$NEW';
    fs.writeFileSync('$REPO_ROOT/ui/package.json', JSON.stringify(p, null, 2) + '\n');
  "
fi

# 4. Verify Cargo.lock updates
(cd "$REPO_ROOT" && cargo check --quiet 2>/dev/null || true)

# 5. Commit
(cd "$REPO_ROOT" && git add VERSION Cargo.toml Cargo.lock ui/package.json)
(cd "$REPO_ROOT" && git commit -m "chore: bump version to $NEW")

# 6. Tag
(cd "$REPO_ROOT" && git tag "v$NEW")

# 7. Push
echo ""
read -rp "Push commit and tag v$NEW to origin? [y/N]: " PUSH
if [ "$PUSH" = "y" ] || [ "$PUSH" = "Y" ]; then
  (cd "$REPO_ROOT" && git push && git push origin "v$NEW")
  echo "Pushed v$NEW"
else
  echo "Skipped push. Run manually:"
  echo "  git push && git push origin v$NEW"
fi

echo ""
echo "Done: v$NEW"
