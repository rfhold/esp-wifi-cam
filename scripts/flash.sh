#!/usr/bin/env bash
set -euo pipefail

readonly BOOTLOADER="bootloader/build/bootloader.bin"
readonly CHECKSUM="bootloader/build/bootloader.bin.sha256"
readonly OTA_IGNORE_A_OFFSET="0xb000"
readonly OTA_IGNORE_B_OFFSET="0xc000"

ignore_version=""
clear_ignore_version=false
flash_arguments=()
while (($#)); do
  case "$1" in
    --ignore-ota-version)
      if [[ -n "${ignore_version}" || "${clear_ignore_version}" == true || $# -lt 2 ]]; then
        printf '%s\n' "refusing to flash: --ignore-ota-version requires one version and cannot be combined with --clear-ignore-ota-version" >&2
        exit 1
      fi
      ignore_version="$2"
      shift 2
      ;;
    --clear-ignore-ota-version)
      if [[ -n "${ignore_version}" || "${clear_ignore_version}" == true ]]; then
        printf '%s\n' "refusing to flash: --clear-ignore-ota-version cannot be combined with --ignore-ota-version" >&2
        exit 1
      fi
      clear_ignore_version=true
      shift
      ;;
    *)
      flash_arguments+=("$1")
      shift
      ;;
  esac
done

if [[ -n "${ignore_version}" || "${clear_ignore_version}" == true ]] && [[ -z "${ESPFLASH_PORT:-}" ]]; then
  printf '%s\n' "refusing to flash OTA ignore record: ESPFLASH_PORT must name the authorized serial port" >&2
  exit 1
fi

if [[ ! -f "${BOOTLOADER}" || ! -f "${CHECKSUM}" ]]; then
  printf '%s\n' "refusing to flash: build and verify the rollback-enabled bootloader first" >&2
  exit 1
fi

sha256sum --check --status "${CHECKSUM}" || {
  printf '%s\n' "refusing to flash: bootloader checksum verification failed" >&2
  exit 1
}

record=""
if [[ -n "${ignore_version}" || "${clear_ignore_version}" == true ]]; then
  record="$(mktemp)"
  trap 'rm -f "${record}"' EXIT
  record_arguments=(ignore-record --output "${record}")
  if [[ "${clear_ignore_version}" == true ]]; then
    record_arguments+=(--clear)
  else
    record_arguments+=(--version "${ignore_version}")
  fi
  rustup run stable cargo run --locked -p release-tool \
    --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]' -- \
    "${record_arguments[@]}"
fi

if [[ -z "${record}" ]]; then
  exec espflash flash --monitor --chip esp32s3 --log-format defmt \
    --flash-mode dio --flash-freq 40mhz --flash-size 8mb \
    --partition-table partitions.csv --bootloader "${BOOTLOADER}" "${flash_arguments[@]}"
fi

override_flash_arguments=()
for argument in "${flash_arguments[@]}"; do
  if [[ "${argument}" != "--non-interactive" ]]; then
    override_flash_arguments+=("${argument}")
  fi
done

if ((${#override_flash_arguments[@]} == 0)); then
  printf '%s\n' "refusing to flash OTA ignore record: app ELF argument is required" >&2
  exit 1
fi
flash_elf="${override_flash_arguments[${#override_flash_arguments[@]} - 1]}"
if [[ "${flash_elf}" == -* ]]; then
  printf '%s\n' "refusing to flash OTA ignore record: app ELF must be the final argument" >&2
  exit 1
fi

espflash flash --after no-reset --non-interactive --chip esp32s3 --port "${ESPFLASH_PORT}" \
  --flash-mode dio --flash-freq 40mhz --flash-size 8mb \
  --partition-table partitions.csv --bootloader "${BOOTLOADER}" "${override_flash_arguments[@]}"
espflash write-bin --after no-reset --non-interactive --chip esp32s3 --port "${ESPFLASH_PORT}" \
  "${OTA_IGNORE_A_OFFSET}" "${record}"
espflash write-bin --after no-reset --non-interactive --chip esp32s3 --port "${ESPFLASH_PORT}" \
  "${OTA_IGNORE_B_OFFSET}" "${record}"
espflash reset --non-interactive --chip esp32s3 --port "${ESPFLASH_PORT}"
exec espflash monitor --no-reset --non-interactive --chip esp32s3 --port "${ESPFLASH_PORT}" --log-format defmt \
  --elf "${flash_elf}"
