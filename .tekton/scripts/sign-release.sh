#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C
umask 077
unset http_proxy https_proxy all_proxy no_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

kind=${1:-}
version=${2:-}
[[ $kind == stable || $kind == prerelease ]] || fail 'release kind is invalid'
[[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.[1-9][0-9]*)?$ ]] || fail 'version is invalid'
if [[ $kind == stable ]]; then
  [[ $version != *-* ]] || fail 'stable release version contains a prerelease'
  tracks=(stable prerelease)
else
  [[ $version == *-rc.* ]] || fail 'prerelease version is not an RC'
  tracks=(prerelease)
fi

session_dir=/var/run/secrets/openbao-ci/session
token_file=$session_dir/token
response_file=$session_dir/firmware-signing.json
private_key=$session_dir/private-key.pem
verify_envelope=$session_dir/verify-manifest.json
openbao_config=$session_dir/openbao-curl.conf
cleanup() {
  rm -f -- "$response_file" "$private_key" "$verify_envelope" "$openbao_config"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

[[ -d $session_dir && ! -L $session_dir ]] || fail 'memory-backed OpenBao session is unavailable'
[[ -f $token_file && ! -L $token_file ]] || fail 'OpenBao token is unavailable'
grep -Eq '^[A-Za-z0-9._-]+$' "$token_file" || fail 'OpenBao token is not header-safe'
[[ -x .esp-release/tools/release-tool ]] || fail 'release tool is unavailable'
image_name="esp-wifi-cam-${version}-esp32s3.bin"
image=.esp-release/output/version/$image_name
[[ -f $image && ! -L $image ]] || fail 'release image is unavailable'

{
  printf 'header = "X-Vault-Token: '
  tr -d '\r\n' < "$token_file"
  printf '"\n'
} > "$openbao_config"
chmod 0600 "$openbao_config"
http_code=$(curl --disable --config "$openbao_config" --silent --show-error \
  --noproxy '*' --proto '=https' --max-redirs 0 --connect-timeout 10 --max-time 30 --max-filesize 16384 \
  --output "$response_file" --write-out '%{http_code}' \
  'https://openbao.holdenitdown.net/v1/kv/data/ci/esp-wifi-cam/firmware-signing') || fail 'OpenBao firmware signing key request failed'
[[ $http_code == 200 ]] || fail "OpenBao firmware signing key request returned HTTP $http_code"
jq -e '.data.data["private-key-base64"] | type == "string" and length >= 1 and length <= 8192 and test("^[A-Za-z0-9+/]*={0,2}$")' "$response_file" >/dev/null || fail 'firmware signing key field is invalid'
jq -er '.data.data["private-key-base64"]' "$response_file" | base64 --decode > "$private_key" || fail 'firmware signing key could not be decoded'
rm -f -- "$response_file"
chmod 0400 "$private_key"
[[ -s $private_key && $(stat -c '%s' "$private_key") -le 4096 ]] || fail 'decoded firmware signing key has an invalid size'

image_length=$(stat -c '%s' "$image")
image_sha256=$(sha256sum "$image" | cut -d' ' -f1)
asset_path="/rfhold/esp-wifi-cam/releases/download/v${version}/${image_name}"
for track in "${tracks[@]}"; do
  channel_dir=.esp-release/output/$track
  version_manifest=".esp-release/output/version/esp-wifi-cam-${version}-${track}-manifest.json"
  install -d -m 0700 "$channel_dir"
  printf 'Signed %s channel manifest for v%s.\n' "$track" "$version" > "$channel_dir/release-body.md"

  .esp-release/tools/release-tool \
    --track "$track" \
    --version "$version" \
    --asset-path "$asset_path" \
    --image "$image" \
    --output "$version_manifest" \
    --public-key .esp-release/output/version/ota-public.der \
    --private-key "$private_key" \
    --max-slot-length 0x330000
  .esp-release/tools/release-tool \
    --track "$track" \
    --version "$version" \
    --asset-path "$asset_path" \
    --image "$image" \
    --output "$verify_envelope" \
    --public-key .esp-release/output/version/ota-public.der \
    --private-key "$private_key" \
    --max-slot-length 0x330000
  cmp -s "$version_manifest" "$verify_envelope" || fail 'repeated signed envelope verification differed'
  install -m 0644 "$version_manifest" "$channel_dir/manifest.json"
  jq -e \
    --arg track "$track" --arg version "$version" --arg path "$asset_path" \
    --arg sha256 "$image_sha256" --argjson length "$image_length" \
    'keys == ["signature", "signed"]
     and (.signature | type == "string" and test("^[A-Za-z0-9_-]{86}$"))
     and (.signed | keys == ["board", "length", "path", "schema", "sha256", "target", "track", "version"])
     and .signed.schema == 1
     and .signed.board == "seeed-xiao-esp32s3-sense"
     and .signed.target == "xtensa-esp32s3-none-elf"
     and .signed.track == $track
     and .signed.version == $version
     and .signed.path == $path
     and .signed.length == $length
     and .signed.sha256 == $sha256' "$channel_dir/manifest.json" >/dev/null || fail 'signed envelope fields do not match the release artifacts'
  rm -f -- "$verify_envelope"
done
