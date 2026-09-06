#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C
umask 077
unset http_proxy https_proxy all_proxy no_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

[[ ${RELEASE_TAG:-} =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.[1-9][0-9]*)?$ ]] || fail 'release tag is invalid'
[[ ${REVISION:-} =~ ^[0-9a-f]{40}$ ]] || fail 'release revision is invalid'
[[ ${PRERELEASE:-} == true || ${PRERELEASE:-} == false ]] || fail 'prerelease value is invalid'
[[ -n ${RELEASE_TITLE:-} && -n ${RESULT_PATH:-} ]] || fail 'release metadata is incomplete'
(( $# >= 1 )) || fail 'expected version assets are missing'

session_dir=/var/run/secrets/openbao-ci/session
token_file=$session_dir/token
secret_response=$session_dir/forgejo-secret.json
release_response=$session_dir/version-release.json
remote_dir=$session_dir/version-assets
openbao_config=$session_dir/openbao-curl.conf
forgejo_token_file=$session_dir/forgejo-token
forgejo_config=$session_dir/forgejo-curl.conf
cleanup() {
  rm -rf -- "$secret_response" "$release_response" "$remote_dir" "$openbao_config" "$forgejo_token_file" "$forgejo_config"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
[[ -f $token_file && ! -L $token_file ]] || fail 'OpenBao token is unavailable'
grep -Eq '^[A-Za-z0-9._-]+$' "$token_file" || fail 'OpenBao token is not header-safe'
[[ -d ${LOCAL_DIR:-} && ! -L ${LOCAL_DIR:-} ]] || fail 'local version input is unavailable'
[[ -f ${BODY_FILE:-} && ! -L ${BODY_FILE:-} ]] || fail 'local release body is unavailable'

{
  printf 'header = "X-Vault-Token: '
  tr -d '\r\n' < "$token_file"
  printf '"\n'
} > "$openbao_config"
chmod 0600 "$openbao_config"
http_code=$(curl --disable --config "$openbao_config" --silent --show-error \
  --noproxy '*' --proto '=https' --max-redirs 0 --connect-timeout 10 --max-time 30 --max-filesize 65536 \
  --output "$secret_response" --write-out '%{http_code}' \
  'https://openbao.holdenitdown.net/v1/kv/data/ci/kuri/forgejo-release') || fail 'OpenBao Forgejo token request failed'
[[ $http_code == 200 ]] || fail "OpenBao Forgejo token request returned HTTP $http_code"
jq -e '.data.data.token | type == "string" and length >= 1 and length <= 4096 and (test("[[:space:]]") | not)' "$secret_response" >/dev/null || fail 'Forgejo token field is invalid'
jq -jr '.data.data.token' "$secret_response" > "$forgejo_token_file"
chmod 0600 "$forgejo_token_file"
grep -Eq '^[A-Za-z0-9._-]+$' "$forgejo_token_file" || fail 'Forgejo token is not header-safe'
{
  printf 'header = "Authorization: token '
  tr -d '\r\n' < "$forgejo_token_file"
  printf '"\n'
} > "$forgejo_config"
chmod 0600 "$forgejo_config"
rm -f -- "$secret_response"

http_code=$(curl --disable --config "$forgejo_config" --silent --show-error \
  --noproxy '*' --proto '=https' --max-redirs 0 --connect-timeout 10 --max-time 30 --max-filesize 2097152 \
  --output "$release_response" --write-out '%{http_code}' \
  "https://git.holdenitdown.net/api/v1/repos/rfhold/esp-wifi-cam/releases/tags/$RELEASE_TAG") || fail 'Forgejo version release lookup failed'
if [[ $http_code == 404 ]]; then
  printf '%s' true > "$RESULT_PATH"
  exit 0
fi
[[ $http_code == 200 ]] || fail "Forgejo version release lookup returned HTTP $http_code"

install -d -m 0700 "$remote_dir"
for name in "$@"; do
  [[ $name =~ ^[A-Za-z0-9._-]+$ ]] || fail 'version asset name is invalid'
  http_code=$(curl --disable --config "$forgejo_config" --silent --show-error \
    --noproxy '*' --proto '=https' --max-redirs 0 --connect-timeout 10 --max-time 1800 --max-filesize 4194304 \
    --output "$remote_dir/$name" --write-out '%{http_code}' \
    "https://git.holdenitdown.net/rfhold/esp-wifi-cam/releases/download/$RELEASE_TAG/$name") || fail 'Forgejo version asset download failed'
  [[ $http_code == 200 ]] || fail "Forgejo version asset download returned HTTP $http_code"
done

arguments=(
  version
  --release-json "$release_response"
  --tag "$RELEASE_TAG"
  --revision "$REVISION"
  --title "$RELEASE_TITLE"
  --body "$BODY_FILE"
  --local-dir "$LOCAL_DIR"
  --remote-dir "$remote_dir"
)
[[ $PRERELEASE == false ]] || arguments+=(--prerelease)
python3 .tekton/scripts/release-state.py "${arguments[@]}" "$@" || fail 'existing immutable version release does not match local intent'
printf '%s' false > "$RESULT_PATH"
