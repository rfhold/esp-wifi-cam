#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C
umask 077
unset http_proxy https_proxy all_proxy no_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY

tag=${1:-}
[[ $tag == stable || $tag == prerelease ]] || exit 1
candidate=${2:-}
[[ -f $candidate && ! -L $candidate ]] || exit 1
session_dir=/var/run/secrets/openbao-ci/session
token_file=$session_dir/token
secret_response=$session_dir/forgejo-secret.json
release_response=$session_dir/channel-release.json
current_manifest=$session_dir/current-manifest.json
openbao_config=$session_dir/openbao-curl.conf
forgejo_token_file=$session_dir/forgejo-token
forgejo_config=$session_dir/forgejo-curl.conf
cleanup() {
  rm -f -- "$secret_response" "$release_response" "$current_manifest" "$openbao_config" "$forgejo_token_file" "$forgejo_config"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
[[ -f $token_file && ! -L $token_file ]] || exit 1
grep -Eq '^[A-Za-z0-9._-]+$' "$token_file" || exit 1

{
  printf 'header = "X-Vault-Token: '
  tr -d '\r\n' < "$token_file"
  printf '"\n'
} > "$openbao_config"
chmod 0600 "$openbao_config"
http_code=$(curl --disable --config "$openbao_config" --silent --show-error \
  --noproxy '*' --proto '=https' --max-redirs 0 --connect-timeout 10 --max-time 30 --max-filesize 65536 \
  --output "$secret_response" --write-out '%{http_code}' \
  'https://openbao.holdenitdown.net/v1/kv/data/ci/kuri/forgejo-release')
[[ $http_code == 200 ]] || exit 1
jq -e '.data.data.token | type == "string" and length >= 1 and length <= 4096 and (test("[[:space:]]") | not)' "$secret_response" >/dev/null || exit 1
jq -jr '.data.data.token' "$secret_response" > "$forgejo_token_file"
chmod 0600 "$forgejo_token_file"
grep -Eq '^[A-Za-z0-9._-]+$' "$forgejo_token_file" || exit 1
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
  "https://git.holdenitdown.net/api/v1/repos/rfhold/esp-wifi-cam/releases/tags/$tag")
if [[ $http_code == 404 ]]; then
  python3 .tekton/scripts/release-state.py channel \
    --track "$tag" \
    --public-key keys/ota-public.der \
    --expected-key-sha256 6eadf451f13c0be6714da1a25eee22421a67203195d1128bfc98ba68cf12a5b7 \
    --candidate "$candidate"
  exit
fi
[[ $http_code == 200 ]] || exit 1
jq -e '[.assets[]?.name] | length <= 1 and all(. == "manifest.json")' "$release_response" >/dev/null || {
  printf '%s\n' 'channel release contains an unmanaged asset' >&2
  exit 1
}
if ! jq -e '[.assets[]?.name] | length == 1 and .[0] == "manifest.json"' "$release_response" >/dev/null; then
  python3 .tekton/scripts/release-state.py channel \
    --track "$tag" \
    --public-key keys/ota-public.der \
    --expected-key-sha256 6eadf451f13c0be6714da1a25eee22421a67203195d1128bfc98ba68cf12a5b7 \
    --candidate "$candidate"
  exit
fi
http_code=$(curl --disable --config "$forgejo_config" --silent --show-error \
  --noproxy '*' --proto '=https' --max-redirs 0 --connect-timeout 10 --max-time 30 --max-filesize 1536 \
  --output "$current_manifest" --write-out '%{http_code}' \
  "https://git.holdenitdown.net/rfhold/esp-wifi-cam/releases/download/$tag/manifest.json") || exit 1
[[ $http_code == 200 ]] || exit 1
python3 .tekton/scripts/release-state.py channel \
  --track "$tag" \
  --public-key keys/ota-public.der \
  --expected-key-sha256 6eadf451f13c0be6714da1a25eee22421a67203195d1128bfc98ba68cf12a5b7 \
  --candidate "$candidate" \
  --current "$current_manifest"
