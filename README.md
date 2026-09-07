# ESP Wi-Fi Cam

This repository contains `no_std` firmware for a Seeed Studio XIAO ESP32-S3 Sense with an OV3660 camera and PDM microphone. One universal public image is provisioned over USB, serves Full HD JPEG snapshots, MJPEG, and raw PCM audio, and pulls signed A/B updates from Forgejo over HTTPS.

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

Docker cannot access USB. Flashing and monitoring run on the host, not in Docker. The following command verifies the custom bootloader checksum, flashes the Docker-built ELF, and starts Defmt monitoring. It is a hardware mutation and requires explicit authorization for the exact device:

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

### Local OTA Ignore Override

A physical operator can make a manually flashed development image skip one exact signed OTA manifest version without weakening manifest verification. Set the persistent per-device override while flashing the app:

```sh
ESPFLASH_PORT=<authorized-port> ESPFLASH_SKIP_UPDATE_CHECK=true \
  scripts/flash.sh --non-interactive --ignore-ota-version <semver> \
  target/xtensa-esp32s3-none-elf/release/esp-wifi-cam
```

The override applies only after the firmware has fetched and fully verified the manifest. It skips installation only when the verified candidate version exactly equals `<semver>`; another valid signed version proceeds normally. It is local flash state, survives reboot and later manual app flashes, has no HTTP, USB, or remote configuration surface, and must be cleared explicitly:

```sh
ESPFLASH_PORT=<authorized-port> ESPFLASH_SKIP_UPDATE_CHECK=true \
  scripts/flash.sh --non-interactive --clear-ignore-ota-version \
  target/xtensa-esp32s3-none-elf/release/esp-wifi-cam
```

The two override flags are mutually exclusive. The runner keeps the newly flashed firmware from starting while it writes both sectors, then performs one controlled reset before starting its monitor. Both commands flash hardware and write local flash state, so they require the same exact-device authorization as every other use of the runner.

## Provisioning

On every boot, USB Serial/JTAG prints a provisioning-ready message. A configured device waits two seconds for a replacement command; an unconfigured device waits without starting Wi-Fi or camera services. Send one newline-terminated command:

```text
P1 <stable|prerelease> <ssid-base64url-no-pad> <password-base64url-no-pad>
```

The SSID must decode to 1-32 UTF-8 bytes. The WPA2 password must decode to 8-63 UTF-8 bytes. Replies contain only success or an error category and never echo either value. Configuration is hash-protected and journaled between two flash sectors. The DHCP hostname is `esp-cam-` followed by the final three station MAC bytes as six lowercase hexadecimal characters.

## Camera And Audio Endpoints

After DHCP completes, use the generated hostname or logged IPv4 address:

| Path | Response |
| --- | --- |
| `/capture.jpg` | One Full HD (`1920x1080`) JPEG frame |
| `/stream` | Continuous Full HD (`1920x1080`) `multipart/x-mixed-replace` MJPEG stream |
| `/audio.pcm` | Indefinite raw `application/octet-stream` stream of 16 kHz mono signed 16-bit little-endian PCM |
| `/status` | Compact JSON containing the firmware version, configured track, and OTA transfer state |

The server accepts exactly three concurrent HTTP connections. It admits only one active `/audio.pcm` stream; another audio client receives `503 Service Unavailable`. Concurrent `/stream` consumers compete for the camera's bounded frame buffers; they do not receive independently duplicated streams. New HTTP requests receive `503 Service Unavailable` during an OTA artifact transfer, video closes at its next frame boundary, and audio closes after its current bounded PCM chunk. The firmware has no update upload endpoint.

Audio and video are separate, unmuxed streams. For Frigate, use `/audio.pcm` as a distinct FFmpeg input with the `audio` role, not as part of the MJPEG input or recording stream. For example:

```yaml
ffmpeg:
  inputs:
    - path: http://camera-host/stream
      roles: [detect]
    - path: http://camera-host/audio.pcm
      input_args: -f s16le -ar 16000 -ac 1
      roles: [audio]
```

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
