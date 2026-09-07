# ESP Wi-Fi Cam

This repository contains `no_std` firmware for a Seeed Studio XIAO ESP32-S3 Sense with an OV3660 camera. One universal public image is provisioned over USB, serves Full HD JPEG snapshots and MJPEG, and pulls signed A/B updates from Forgejo over HTTPS.

## Prerequisites

- A Seeed Studio XIAO ESP32-S3 Sense with its OV3660 camera expansion board
- The Espressif Rust toolchain selected by [`rust-toolchain.toml`](rust-toolchain.toml)
- `espflash` for image conversion and the guarded flash runner
- An 8 MiB flash device; the model documentation specifies 8 MiB, but the exact devices remain hardware-unverified
- A rollback-enabled bootloader built from the pinned ESP-IDF source inputs in [`bootloader/`](bootloader/)

## Build

The build does not use or embed Wi-Fi credentials:

```sh
cargo build --locked --release
espflash save-image --chip esp32s3 \
  target/xtensa-esp32s3-none-elf/release/esp-wifi-cam \
  esp-wifi-cam.bin
```

The OTA artifact is the app-only `espflash save-image` output. Do not pass `--merge`.

## Container Tooling

Docker Compose provides a pinned ESP-IDF and Espressif Rust environment for firmware and custom bootloader builds. The initial image build and the first Cargo build may need network access to obtain the pinned toolchain and locked dependencies.

```sh
docker compose build
docker compose run --rm build cargo build --locked --release
docker compose run --rm build scripts/build-bootloader.sh
```

The workspace is read-only in the container. Docker-managed volumes retain Cargo registry and Git caches. The ignored host directories `target/` and `bootloader/build/` are the only writable workspace mounts, so the build ELF, bootloader, and checksum are available to host tools.

Flashing and monitoring run on the host, not in Docker. The following command verifies the custom bootloader checksum, flashes the Docker-built ELF, and starts Defmt monitoring. It is a hardware mutation and requires explicit authorization for the exact device:

```sh
ESPFLASH_PORT=/dev/ttyACM0 ESPFLASH_SKIP_UPDATE_CHECK=true \
  scripts/flash.sh --non-interactive \
  target/xtensa-esp32s3-none-elf/release/esp-wifi-cam
```

To attach a later host-side Defmt monitor without flashing, use the same ELF:

```sh
ESPFLASH_PORT=/dev/ttyACM0 ESPFLASH_SKIP_UPDATE_CHECK=true \
  espflash monitor --non-interactive --chip esp32s3 --log-format defmt \
  --elf target/xtensa-esp32s3-none-elf/release/esp-wifi-cam
```

Set `ESPFLASH_PORT` to the authorized host serial path, such as `/dev/ttyACM0`. Set `ESPFLASH_SKIP_UPDATE_CHECK=true` and use `--non-interactive` for noninteractive host operation. The flash command preserves the bootloader checksum gate in [`scripts/flash.sh`](scripts/flash.sh); build the rollback-enabled bootloader first.

`cargo run --release` invokes [`scripts/flash.sh`](scripts/flash.sh). The runner refuses to flash unless a locally built bootloader and its checksum exist, then supplies that bootloader and [`partitions.csv`](partitions.csv) to `espflash`. Flashing mutates attached hardware and requires explicit authorization.

## Provisioning

On every boot, USB Serial/JTAG prints a provisioning-ready message. A configured device waits two seconds for a replacement command; an unconfigured device waits without starting Wi-Fi or camera services. Send one newline-terminated command:

```text
P1 <stable|prerelease> <ssid-base64url-no-pad> <password-base64url-no-pad>
```

The SSID must decode to 1-32 UTF-8 bytes. The WPA2 password must decode to 8-63 UTF-8 bytes. Replies contain only success or an error category and never echo either value. Configuration is hash-protected and journaled between two flash sectors. The DHCP hostname is `esp-cam-` followed by the final three station MAC bytes as six lowercase hexadecimal characters.

## Camera Endpoints

After DHCP completes, use the generated hostname or logged IPv4 address:

| Path | Response |
| --- | --- |
| `/capture.jpg` | One Full HD (`1920x1080`) JPEG frame |
| `/stream` | Continuous Full HD (`1920x1080`) `multipart/x-mixed-replace` MJPEG stream |
| `/status` | Compact JSON containing the firmware version, configured track, and OTA transfer state |

The server accepts exactly two concurrent HTTP connections, intended for one Frigate `/stream` client and one independent `/status` or `/capture.jpg` request. Concurrent `/stream` consumers compete for the camera's bounded frame buffers; they do not receive independently duplicated streams. New HTTP requests receive `503 Service Unavailable` during an OTA artifact transfer, and an existing stream closes at its next frame boundary. The firmware has no update upload endpoint.

## Repository Map

| Path | Responsibility |
| --- | --- |
| [`src/`](src/) | Firmware entry point, provisioning, camera service, and OTA runtime |
| [`crates/ota-core/`](crates/ota-core/) | Host-testable provisioning codec, manifest verification, and release policy |
| [`crates/release-tool/`](crates/release-tool/) | Host-only canonical manifest validation and Ed25519 PKCS#8 signing tool |
| [`crates/ov3660/`](crates/ov3660/) | OV3660 sensor, ESP32-S3 LCD_CAM/GDMA capture, and JPEG parsing |
| [`partitions.csv`](partitions.csv) | Factory-plus-two-OTA 8 MiB partition layout |
| [`bootloader/`](bootloader/) | Pinned rollback-enabled ESP-IDF bootloader build inputs |
| [`.tekton/`](.tekton/) | Pipelines-as-Code validation and signed stable/RC release workflows |
| [`docs/`](docs/) | Canonical firmware and OTA contracts |

See the [documentation index](docs/README.md) for architecture, signing, release, safety, and validation details.
