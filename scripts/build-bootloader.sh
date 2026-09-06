#!/usr/bin/env bash
set -euo pipefail

readonly IDF_COMMIT="3ad36321ea7e6183986e19c7e05ee03052d181c3"
readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly BUILD_DIR="${ROOT}/bootloader/build/idf"
readonly OUTPUT_DIR="${ROOT}/bootloader/build"

if [[ -z "${IDF_PATH:-}" || ! -d "${IDF_PATH}/.git" ]]; then
  printf '%s\n' "IDF_PATH must name an ESP-IDF git checkout" >&2
  exit 1
fi
if [[ "$(git -C "${IDF_PATH}" rev-parse HEAD)" != "${IDF_COMMIT}" ]]; then
  printf '%s\n' "ESP-IDF checkout is not at required commit ${IDF_COMMIT}" >&2
  exit 1
fi
if [[ -n "$(git -C "${IDF_PATH}" status --porcelain)" ]]; then
  printf '%s\n' "ESP-IDF checkout must be clean" >&2
  exit 1
fi

source "${IDF_PATH}/export.sh"
idf.py -C "${ROOT}/bootloader" -B "${BUILD_DIR}" \
  -D SDKCONFIG="${OUTPUT_DIR}/sdkconfig" \
  -D SDKCONFIG_DEFAULTS="${ROOT}/bootloader/sdkconfig.defaults" bootloader
install -m 0644 "${BUILD_DIR}/bootloader/bootloader.bin" "${OUTPUT_DIR}/bootloader.bin"
(
  cd "${ROOT}"
  sha256sum "bootloader/build/bootloader.bin" > "bootloader/build/bootloader.bin.sha256"
)
