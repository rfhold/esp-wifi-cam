---
name: firmware-build-flash
description: Use for this firmware's Docker build, host flash, host Defmt monitor, or related tooling-cache troubleshooting path.
---

# Firmware Build And Flash

This skill supplies repository-specific operational inputs for the recurring build, flash, monitor, and tooling-cache path. It does not replace generic change planning or execution: direct a feature change to `planning-changes` and `making-changes`.

## Canonical Sources

- [Repository entry point](../../../README.md)
- [Firmware contract](../../../docs/firmware.md)
- [Docker Compose build environment](../../../compose.yaml)
- [Guarded host flash script](../../../scripts/flash.sh)

## Docker Build

Docker is supported for builds only. The local daemon is rootless and cannot access USB serial devices, so Docker serial flashing and monitoring are unsupported.

Run the build steps from the repository root:

```sh
docker compose build
docker compose run --rm build cargo build --locked --release
docker compose run --rm build scripts/build-bootloader.sh
```

`compose.yaml` provides only the `build` service. It mounts the source read-only, retains Docker-managed Cargo registry and Git caches, and writes workspace output only to ignored `target/` and `bootloader/build/` host paths.

## Host Hardware Operations

The mutation gate is exact: do not flash, erase, provision, or otherwise mutate hardware unless the user explicitly authorizes the exact host serial device path. Do not infer a device path from discovery output or prior observations.

After exact-device authorization, flash the Docker-built ELF from the host:

```sh
ESPFLASH_PORT=/dev/ttyACM0 ESPFLASH_SKIP_UPDATE_CHECK=true scripts/flash.sh --non-interactive target/xtensa-esp32s3-none-elf/release/esp-wifi-cam
```

`scripts/flash.sh` verifies `bootloader/build/bootloader.bin` against its checksum before it invokes `espflash`; build the bootloader before flashing.

For host-side monitor-only use, pass the ELF so `espflash` can decode Defmt logs:

```sh
ESPFLASH_PORT=/dev/ttyACM0 ESPFLASH_SKIP_UPDATE_CHECK=true espflash monitor --non-interactive --chip esp32s3 --log-format defmt --elf target/xtensa-esp32s3-none-elf/release/esp-wifi-cam
```

## Tooling Cache Troubleshooting

`bootloader/build/idf` is generated output, but may be user-owned local output. If it references a stale ESP-IDF path, do not delete it without explicit user authorization.

Do not record one-off device identifiers, network details, successful-flash results, or hardware observations in persistent documentation.
