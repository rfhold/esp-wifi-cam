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

New HTTP requests receive `503 Service Unavailable` during an OTA artifact transfer, and an existing stream closes at its next frame boundary. The firmware has no update upload endpoint.

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
