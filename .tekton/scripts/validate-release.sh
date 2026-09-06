#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

[[ ${RELEASE_KIND:-} == stable || ${RELEASE_KIND:-} == prerelease ]] || fail 'release kind is invalid'
[[ ${SOURCE_BRANCH:-} == refs/tags/* ]] || fail 'source branch is not a tag ref'
tag=${SOURCE_BRANCH#refs/tags/}
if [[ $RELEASE_KIND == stable ]]; then
  [[ $tag =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || fail 'tag is not an exact stable release tag'
else
  [[ $tag =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-rc\.([1-9][0-9]*)$ ]] || fail 'tag is not an exact RC release tag'
fi
[[ ${REVISION:-} =~ ^[0-9a-f]{40}$ ]] || fail 'revision is not a full lowercase commit SHA'
[[ $(git cat-file -t "refs/tags/$tag") == tag ]] || fail 'release tag is not annotated'
[[ $(git rev-parse "refs/tags/$tag^{commit}") == "$REVISION" ]] || fail 'tag does not resolve to the webhook revision'
git merge-base --is-ancestor "$REVISION" refs/remotes/origin/main || fail 'release revision is not reachable from origin/main'

version=${tag#v}
package_version=$(python3 - <<'PY'
import pathlib
import tomllib

document = tomllib.loads(pathlib.Path("Cargo.toml").read_text(encoding="utf-8"))
print(document["package"]["version"])
PY
)
[[ $package_version == "$version" ]] || fail 'Cargo package version does not equal the tag version'

release_root=.esp-release
rm -rf -- "$release_root"
install -d -m 0700 "$release_root"
git cat-file tag "refs/tags/$tag" | awk '
  in_body && /^-----BEGIN (PGP|SSH) SIGNATURE-----$/ { exit }
  in_body { print; next }
  /^$/ { in_body = 1 }
' > "$release_root/release-body.md"
grep -Eq '^-----BEGIN (PGP|SSH) SIGNATURE-----$' "$release_root/release-body.md" && fail 'release body contains signature material'

printf '%s' "$tag" > "$TAG_RESULT"
printf '%s' "$version" > "$VERSION_RESULT"
