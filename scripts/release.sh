#!/bin/bash
set -euo pipefail

# Usage: ./scripts/release.sh 0.2.0

VERSION="${1:?Usage: release.sh <version>}"

echo "Releasing v${VERSION}..."

# Update Cargo.toml
sed -i '' "s/^version = \".*\"/version = \"${VERSION}\"/" Cargo.toml

# Update npm/package.json
cd npm
node -e "
const pkg = require('./package.json');
pkg.version = '${VERSION}';
require('fs').writeFileSync('./package.json', JSON.stringify(pkg, null, 2) + '\n');
"
cd ..

# Regenerate Cargo.lock with new version
cargo check --quiet

# Commit, tag, push
git add Cargo.toml npm/package.json Cargo.lock
git commit -m "release: v${VERSION}"
git tag "v${VERSION}"
git push && git push --tags

echo "Done. GitHub Actions will build binaries and publish to npm."
echo "Monitor: https://github.com/sderosiaux/chrome-agent/actions"
