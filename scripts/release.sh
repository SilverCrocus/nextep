#!/usr/bin/env bash
# Usage: scripts/release.sh 0.3.0
# Bumps the version, commits, tags and pushes. GitHub Actions then builds the
# signed APK and publishes the release (.github/workflows/release.yml).
set -euo pipefail

version=${1:?usage: scripts/release.sh X.Y.Z}
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must be X.Y.Z"; exit 1; }
cd "$(dirname "$0")/.."
[[ -z $(git status --porcelain) ]] || { echo "commit or stash your changes first"; exit 1; }

# Tauri reads the Android version from tauri.conf.json; Cargo from the
# workspace. Keep them in step.
sed -i "0,/^version = \".*\"/s//version = \"$version\"/" Cargo.toml
jq --arg v "$version" '.version = $v' src-tauri/tauri.conf.json > src-tauri/tauri.conf.json.tmp
mv src-tauri/tauri.conf.json.tmp src-tauri/tauri.conf.json
cargo update --workspace --quiet

git commit -am "Release v$version"
git tag "v$version"
git push origin HEAD "v$version"
echo "Pushed v$version. Follow the build with: gh run watch"
