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

# Every other place the version is written by hand. This used to be a separate
# commit somebody remembered to make (f68d9b7 for 0.10.0); when they forget, the
# skill files an agent reads to decide what the tool can do keep claiming an old
# version, and nothing fails.
sed -i '' "1s/^# chrome-agent v.*/# chrome-agent v${VERSION}/" CLAUDE.md
sed -i '' "s/^  version: \".*\"/  version: \"${VERSION}\"/" \
  skills/chrome-agent/SKILL.md skills/scrape-structured-data/SKILL.md
sed -i '' "s/^chrome-agent v[0-9][0-9.]* /chrome-agent v${VERSION} /" npm/README.md

# Regenerate Cargo.lock with new version
cargo check --quiet

# Fail rather than tag a release whose version strings disagree: a half-bumped
# tree is exactly what this script exists to prevent.
missing=""
grep -q "^version = \"${VERSION}\"" Cargo.toml || missing="${missing} Cargo.toml"
grep -q "\"version\": \"${VERSION}\"" npm/package.json || missing="${missing} npm/package.json"
grep -q "^# chrome-agent v${VERSION}$" CLAUDE.md || missing="${missing} CLAUDE.md"
grep -q "^chrome-agent v${VERSION} " npm/README.md || missing="${missing} npm/README.md"
for skill in skills/chrome-agent/SKILL.md skills/scrape-structured-data/SKILL.md; do
  grep -q "^  version: \"${VERSION}\"" "$skill" || missing="${missing} ${skill}"
done
if [ -n "${missing}" ]; then
  echo "error: version ${VERSION} did not land in:${missing}" >&2
  echo "Nothing was committed or tagged. Fix the pattern in scripts/release.sh." >&2
  exit 1
fi

# Commit, tag, push
git add Cargo.toml npm/package.json Cargo.lock CLAUDE.md npm/README.md \
  skills/chrome-agent/SKILL.md skills/scrape-structured-data/SKILL.md
git commit -m "release: v${VERSION}"
git tag "v${VERSION}"
git push && git push --tags

echo "Done. GitHub Actions will build binaries and publish to npm."
echo "Monitor: https://github.com/sderosiaux/chrome-agent/actions"
