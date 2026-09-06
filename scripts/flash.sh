#!/usr/bin/env bash
set -euo pipefail

readonly BOOTLOADER="bootloader/build/bootloader.bin"
readonly CHECKSUM="bootloader/build/bootloader.bin.sha256"

if [[ ! -f "${BOOTLOADER}" || ! -f "${CHECKSUM}" ]]; then
  printf '%s\n' "refusing to flash: build and verify the rollback-enabled bootloader first" >&2
  exit 1
fi

sha256sum --check --status "${CHECKSUM}" || {
  printf '%s\n' "refusing to flash: bootloader checksum verification failed" >&2
  exit 1
}

exec espflash flash --monitor --chip esp32s3 --log-format defmt \
  --flash-mode dio --flash-freq 40mhz --flash-size 8mb \
  --partition-table partitions.csv --bootloader "${BOOTLOADER}" "$@"
