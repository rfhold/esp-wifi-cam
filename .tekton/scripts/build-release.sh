#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

version=${1:-}
[[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.[1-9][0-9]*)?$ ]] || fail 'version is invalid'
[[ -f .esp-release/release-body.md ]] || fail 'release body is missing'
[[ $(sha256sum keys/ota-public.der | cut -d' ' -f1) == 6eadf451f13c0be6714da1a25eee22421a67203195d1128bfc98ba68cf12a5b7 ]] || fail 'OTA public key fingerprint is invalid'

unset SSID PASSWORD
rm -rf -- .esp-release/output .esp-release/tools
install -d -m 0700 .esp-release/output/version .esp-release/tools
cargo +stable build --locked --release -p release-tool --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]'
install -m 0700 target/x86_64-unknown-linux-gnu/release/release-tool .esp-release/tools/release-tool
cargo build --locked --release -p esp-wifi-cam

image_name="esp-wifi-cam-${version}-esp32s3.bin"
espflash save-image --chip esp32s3 \
  target/xtensa-esp32s3-none-elf/release/esp-wifi-cam \
  ".esp-release/output/version/$image_name"
[[ -f .esp-release/output/version/$image_name && ! -L .esp-release/output/version/$image_name ]] || fail 'app image was not created as a regular file'
[[ $(stat -c '%s' ".esp-release/output/version/$image_name") -ge 1 ]] || fail 'app image is empty'
[[ $(stat -c '%s' ".esp-release/output/version/$image_name") -le $((0x330000)) ]] || fail 'app image exceeds the OTA slot'
[[ $(od -An -tx1 -N1 ".esp-release/output/version/$image_name" | tr -d '[:space:]') == e9 ]] || fail 'app image does not have ESP-IDF app image magic'

install -m 0600 .esp-release/release-body.md .esp-release/output/version/release-body.md
install -m 0644 keys/ota-public.der .esp-release/output/version/ota-public.der
