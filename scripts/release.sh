#!/bin/bash
set -euo pipefail

# Usage: ./scripts/release.sh 0.2.0

VERSION="${1:?Usage: release.sh <version>}"

# The version is interpolated into perl programs below; constrain it to digits and dots.
case "${VERSION}" in
  *[!0-9.]* | '' | *..* | .* | *.) echo "error: version must be numeric, e.g. 0.16.0" >&2; exit 1 ;;
esac

# Preconditions, checked before anything is written. Both used to be discovered too late:
# `git tag` on an existing tag aborts AFTER the release commit is made, leaving a bumped
# tree that cannot be committed to that version again; and a dirty tree would put unrelated
# changes into the release commit.
if [ -n "$(git status --porcelain)" ]; then
  echo "error: working tree is not clean. Commit or stash first." >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/v${VERSION}" >/dev/null; then
  echo "error: tag v${VERSION} already exists locally." >&2
  exit 1
fi
remote_tag="$(git ls-remote --tags origin "v${VERSION}")" || {
  echo "error: could not reach origin to check whether tag v${VERSION} exists." >&2
  exit 1
}
if [ -n "${remote_tag}" ]; then
  echo "error: tag v${VERSION} already exists on origin." >&2
  exit 1
fi

# Needs no build, so it fails in seconds — before anything is written.
cargo fmt --all --check

echo "Releasing v${VERSION}..."

# perl, not `sed -i`: the in-place spelling differs between BSD and GNU sed, so `sed -i ''`
# made this script macOS-only.
perl -pi -e "s/^version = \".*\"/version = \"${VERSION}\"/" Cargo.toml

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
perl -pi -e "s/^# chrome-agent v.*/# chrome-agent v${VERSION}/ if \$. == 1" CLAUDE.md
perl -pi -e "s/^  version: \".*\"/  version: \"${VERSION}\"/" \
  skills/chrome-agent/SKILL.md skills/scrape-structured-data/SKILL.md
perl -pi -e "s/^chrome-agent v[0-9][0-9.]* /chrome-agent v${VERSION} /" npm/README.md

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

# Regenerate Cargo.lock with new version, then gate on the locked graph.
cargo check --quiet

# The gates. They run before the release commit, so a failure leaves an uncommitted
# bump and no tag — re-runnable after `git checkout .`. `cargo build --release` rather
# than `cargo check`: the release profile (lto, panic = "abort", codegen-units = 1) is
# what ships, and it fails differently.
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked

# Commit, tag, push
git add Cargo.toml npm/package.json Cargo.lock CLAUDE.md npm/README.md \
  skills/chrome-agent/SKILL.md skills/scrape-structured-data/SKILL.md
git commit -m "release: v${VERSION}"
git tag "v${VERSION}"
git push && git push --tags

echo "Done. GitHub Actions will build binaries and publish to npm."
echo "Monitor: https://github.com/sderosiaux/chrome-agent/actions"
