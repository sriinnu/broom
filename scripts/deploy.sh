#!/bin/bash
# Build Broom.app, install to /Applications, clean up old builds.
#
#   scripts/deploy.sh  -- builds release, installs, purges old versions
#
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

# Read and bump the build counter.
BUILD_FILE=build.txt
BUILD=$(cat "$BUILD_FILE" 2>/dev/null || echo 0)
BUILD=$((BUILD + 1))
echo "$BUILD" > "$BUILD_FILE"

FULL_VERSION="${VERSION}.b$(printf '%04d' "$BUILD")"
echo "=== Broom $FULL_VERSION ==="

# Export so bundle.sh uses our version string for Info.plist.
export BROOM_FULL_VERSION="$FULL_VERSION"

# Run the existing bundle script.
echo "--- Building $FULL_VERSION ---"
scripts/bundle.sh

# The bundle script writes to dist/Broom.app. Copy it to /Applications,
# versioned, and symlink /Applications/Broom.app -> the versioned copy
# (so "open /Applications/Broom.app" always opens the latest).
echo "--- Installing to /Applications ---"
APP_VERSIONED="/Applications/Broom-${FULL_VERSION}.app"
APP_LATEST="/Applications/Broom.app"

cp -Rf dist/Broom.app "$APP_VERSIONED"
echo "  installed $APP_VERSIONED"

# Remove old versioned copies, keeping the latest.
echo "--- Cleaning old builds ---"
for old in /Applications/Broom-*.app; do
    [ "$old" = "$APP_VERSIONED" ] && continue
    [ -e "$old" ] || continue
    echo "  removing $old"
    rm -rf "$old"
done

# Symlink latest.app -> versioned.app
rm -rf "$APP_LATEST"
ln -s "Broom-${FULL_VERSION}.app" "$APP_LATEST"
echo "  linked $APP_LATEST -> Broom-${FULL_VERSION}.app"

# Clean up dist/ except the latest .app and .dmg
echo "--- Cleaning dist/ ---"
for f in dist/*.app dist/*.dmg; do
    [ -e "$f" ] || continue
    echo "  keeping $f"
done
# Remove straggler build artifacts from dist/ that aren't .app or .dmg
find dist/ -depth 1 ! -name '*.app' ! -name '*.dmg' ! -name '.gitkeep' -exec rm -rf {} + 2>/dev/null || true

echo ""
echo "Done. $FULL_VERSION deployed."
echo "  /Applications/Broom.app → Broom-${FULL_VERSION}.app"
echo "  Run: open /Applications/Broom.app"