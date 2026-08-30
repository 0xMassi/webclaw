#!/usr/bin/env bash
set -euo pipefail

tag="${1:-${GITHUB_REF_NAME:-}}"
if [[ ! "$tag" =~ ^v ]]; then
  echo "release preflight requires a v-prefixed tag" >&2
  exit 1
fi

version="${tag#v}"
# Docker tags cannot contain SemVer's optional `+build.metadata` suffix. Keep
# the shared release version portable across GitHub, Cargo, npm, and GHCR by
# supporting strict numeric SemVer with an optional prerelease suffix only.
release_version_pattern='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-((0|[1-9][0-9]*)|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)(\.((0|[1-9][0-9]*)|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*))*)?$'
if [[ ! "$version" =~ $release_version_pattern ]]; then
  echo "unsupported release tag: $tag (expected vMAJOR.MINOR.PATCH with optional prerelease identifiers)" >&2
  exit 1
fi

workspace_version="$(awk '
  /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
  /^\[/ { in_workspace_package = 0 }
  in_workspace_package && /^version = / {
    value = $3
    gsub(/"/, "", value)
    print value
    exit
  }
' Cargo.toml)"

if [[ "$workspace_version" != "$version" ]]; then
  echo "tag $tag does not match workspace version $workspace_version" >&2
  exit 1
fi

escaped_version="${version//./\\.}"
if ! grep -Eq "^## \\[$escaped_version\\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$" CHANGELOG.md; then
  echo "CHANGELOG.md has no dated [$version] release section" >&2
  exit 1
fi

cargo metadata --format-version 1 --no-deps | python3 -c '
import json
import sys

expected = sys.argv[1]
metadata = json.load(sys.stdin)
wrong = sorted(
    "{}={}".format(package["name"], package["version"])
    for package in metadata["packages"]
    if package["version"] != expected
)
if wrong:
    raise SystemExit(
        "workspace packages do not match release version "
        + expected
        + ": "
        + ", ".join(wrong)
    )
' "$version"

# Stable releases are immediately published to npm and the MCP registry, so
# the checked-in registry manifest must describe the same version. Prerelease
# tags intentionally skip those publication jobs.
if [[ "$version" != *-* ]]; then
  python3 -c '
import json
import sys

expected = sys.argv[1]
with open("packages/create-webclaw/server.json", encoding="utf-8") as source:
    manifest = json.load(source)
with open("packages/webclaw-mcp/package.json", encoding="utf-8") as source:
    launcher = json.load(source)

observed = {
    manifest.get("version"),
    *(package.get("version") for package in manifest.get("packages", [])),
    launcher.get("version"),
}
if observed != {expected}:
    raise SystemExit(
        "release, registry, and launcher versions do not all match " + expected
    )
' "$version"

  if ! grep -Fq "const RELEASE_TAG = process.env.WEBCLAW_MCP_VERSION || \"$tag\";" \
    packages/webclaw-mcp/webclaw-mcp.mjs; then
    echo "MCP launcher is not pinned to $tag" >&2
    exit 1
  fi
fi

if [[ "${GITHUB_REF_TYPE:-}" == "tag" ]]; then
  tag_commit="$(git rev-parse "${tag}^{commit}")"
  if ! git merge-base --is-ancestor "$tag_commit" origin/main; then
    echo "release tag $tag does not point to a commit on main" >&2
    exit 1
  fi
fi

echo "release metadata is consistent for $tag"
