# Firmware

This document defines the firmware architecture, camera and audio services, runtime configuration, failure behavior, and local validation. The signed update contract is canonical in [OTA](ota.md).

## Architecture

| Surface | Responsibility |
| --- | --- |
| [`src/main.rs`](../src/main.rs) | Initializes the ESP32-S3 and runs camera, PDM audio, Wi-Fi, network, HTTP, and OTA tasks. |
| [`src/provision.rs`](../src/provision.rs) | Loads or replaces the two-sector journaled USB provisioning record. |
| [`src/ota.rs`](../src/ota.rs) | Confirms healthy boots and performs HTTPS A/B downloads and flash verification. |
| [`src/ota_ignore.rs`](../src/ota_ignore.rs) | Reads the local two-sector OTA ignore-version journal; it has no network or mutable runtime surface. |
| [`crates/ota-core/`](../crates/ota-core/) | Implements bounded provisioning, signed manifest, path, channel, and version policy. |
| [`crates/ov3660/`](../crates/ov3660/) | Implements OV3660 control, ESP32-S3 LCD_CAM/GDMA capture, and JPEG extraction. |

The firmware uses `esp-hal` and `esp-rtos` with Embassy. Camera frame buffers remain in PSRAM; camera and audio DMA, networking, TLS, and flash work buffers use internal memory. The Seeed Studio XIAO ESP32-S3 Sense PDM microphone uses I2S0 with DMA channel 1, GPIO42 clock, and GPIO41 data. The camera retains LCD_CAM with DMA channel 0. `esp-radio` provides WPA2 station mode, Embassy provides DHCPv4, DNS, and TCP, and reqwless plus embedded-tls provides HTTPS for OTA pulls.

## Runtime Configuration

The firmware is one universal public image and requires no `SSID`, `PASSWORD`, or `HOSTNAME_PREFIX` build variables. USB Serial/JTAG accepts this bounded newline-terminated command:

```text
P1 <stable|prerelease> <ssid-base64url-no-pad> <password-base64url-no-pad>
```

The decoded SSID is 1-32 UTF-8 bytes and the decoded WPA2-Personal password is 8-63 UTF-8 bytes. The parser rejects extra fields, padding, invalid UTF-8, unsupported tracks, oversized input, and malformed base64url. Responses never contain supplied values.

Each fixed 160-byte record binds schema, monotonic wrapping sequence, track, lengths, SSID, and password under SHA-256. The active record is the newest valid record in sectors at `0x9000` and `0xa000`; replacement writes and reads back the other sector before use. A configured device provides one total two-second replacement window at boot. Partial input, invalid commands, zero-length reads, and transport errors do not restart that deadline. An unconfigured device waits indefinitely for valid provisioning.

The DHCP hostname is `esp-cam-<suffix>`, where `suffix` is six lowercase hexadecimal characters from station MAC bytes 3-5.

## Runtime Behavior

1. The firmware initializes heaps and PSRAM, then validates flash capacity and the complete expected partition table before reading or writing provisioning sectors. It starts ESP-RTOS only after validation completes. An invalid layout is logged after the timer starts and halts startup without creating the USB provisioning interface or starting Wi-Fi, camera, HTTP, or OTA.
2. A valid layout permits the firmware to load or receive provisioning and configure WPA2 station mode.
3. The OV3660 receives a 10 MHz XCLK and is configured for Full HD (`1920x1080`) JPEG at quality 12, vertical flip, brightness `+1`, and saturation `-2`.
4. LCD_CAM and GDMA capture into a 20 KiB internal-RAM ring. Two bounded 512 KiB PSRAM buffers retain complete JPEG frames.
5. I2S0 captures 16 kHz mono signed 16-bit little-endian PCM. Eight fixed 1 KiB internal-memory chunks buffer audio; if all chunks are full, capture drops the newest chunk rather than waiting for HTTP.
6. DHCPv4 uses the derived hostname. Separate tasks drive Wi-Fi, networking, camera capture, audio capture, HTTP, and OTA.
7. Three HTTP workers each own a fixed 1 KiB receive buffer, 4 KiB transmit buffer, and TCP socket. The server therefore accepts exactly three concurrent HTTP connections. `/capture.jpg` returns the next complete JPEG. `/stream` returns MJPEG with the `frameboundary` boundary. Concurrent stream consumers compete for the camera's bounded frame buffers; the firmware does not fan out or independently duplicate streams. `GET /audio.pcm` returns an indefinite raw `application/octet-stream` PCM stream. Only one audio stream may be active; another `/audio.pcm` request returns `503 Service Unavailable`. Audio and video remain unmuxed, so `/audio.pcm` is a separate Frigate FFmpeg `audio` input and is not embedded in MJPEG recordings. `GET /status` returns a compact `application/json` object containing the compiled `version`, configured `track`, and `transfer_active` boolean. The status response contains no credentials, signing material, manifest URLs, asset paths, or asset digests. Other targets return `404 Not Found`.
8. The camera signals health only after initialization and one complete JPEG frame. Only then may the OTA task mark a `New` or `PendingVerify` image `Valid`.
9. The first update check starts 30 seconds after health confirmation. Later checks run every six hours for stable provisioning and every 60 seconds for prerelease provisioning while network configuration is available.

After a manifest passes all normal transport, signature, board, target, track, path, slot-length, and version-policy checks, OTA reads the local ignore-version journal in sectors `0xb000` and `0xc000`. A valid stored version skips installation only when it exactly equals the verified manifest version; a mismatch uses the normal download and A/B installation path. The journal is not provisioning data and is never accepted from HTTP, USB, or the network.

New HTTP requests return `503 Service Unavailable` while an artifact is transferring. Existing video streams close at a frame boundary; existing audio streams close after their current bounded PCM chunk. Capture continues, but no new stream consumes a frame during the transfer. There is no update upload endpoint.

## Failure Behavior

- Invalid or absent provisioning fails closed before network and camera startup.
- Flash capacity other than exactly 8 MiB or a partition table that differs from the complete required layout halts startup before any provisioning-sector write.
- Missing PSRAM, camera initialization failure, or camera DMA startup failure stops startup.
- Frames larger than 512 KiB and malformed JPEG streams are dropped and parser synchronization resumes later.
- When all eight audio chunks are occupied, the newest captured PCM chunk is dropped so capture never waits for an HTTP client.
- Wi-Fi failure enters a five-second retry cycle; DHCP absence leaves the main task waiting.
- TLS, HTTP status, response length, manifest, signature, policy, path, flash write, read-back, or hash failure leaves the running slot selected.
- An absent, corrupt, or unreadable OTA ignore journal does not suppress an otherwise accepted update. A valid explicit clear record also permits normal OTA installation.
- A downloaded image is activated only after the downloaded SHA-256 and independently hashed flash read-back both match the signed digest.

## Safety Boundaries

- Never commit Wi-Fi values, tokens, signing keys, or generated provisioning records.
- The checked-in Ed25519 key is public verification material. The private key remains only in OpenBao and is never a firmware build input.
- HTTPS never falls back to plaintext. TLS pins ISRG Root X1 but does not validate certificate time; Ed25519 is the artifact authenticity root.
- Treat `cargo check`, `cargo build`, host tests, formatting, and partition conversion as local non-hardware checks.
- Treat `cargo run --release`, `espflash flash`, erase commands, and serial provisioning as hardware mutations requiring explicit device authorization.
- `scripts/flash.sh --ignore-ota-version <semver>` and `scripts/flash.sh --clear-ignore-ota-version` are physical-operator controls only. They prevent the newly flashed app from starting while both local OTA-ignore sectors are written, then issue one controlled reset before monitoring; they are mutually exclusive, persistent across reboot and manual app flashes, and never alter OTA trust policy.

## Validation

```sh
cargo fmt --check
cargo +stable test -p ota-core --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]'
cargo +stable test -p release-tool --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]'
cargo +stable test -p ov3660 --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]'
cargo check --locked --release
cargo build --locked --release
espflash partition-table --skip-update-check --to-binary --output /tmp/partitions.bin partitions.csv
espflash partition-table --skip-update-check /tmp/partitions.bin
git diff --check
```

On a host, `rustup run stable cargo run --locked -p release-tool --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]' -- ignore-record --version <semver> --output /tmp/ota-ignore.bin` creates a credential-free set record. Replace `--version <semver>` with `--clear` to create the explicit clear record. These commands only create a local file; using the record through `scripts/flash.sh` is a hardware mutation.

Docker builds support firmware and bootloader compilation but cannot access USB. Flashing and monitoring occur on the host through [`scripts/flash.sh`](../scripts/flash.sh) after a Docker build; flashing remains a hardware mutation that requires explicit authorization for the exact device.

These checks do not flash hardware. They do not prove flash capacity, bootloader rollback, Wi-Fi association, TLS interoperability with the live Forgejo chain, camera timing, microphone capture or audio streaming, PSRAM integrity, update reboot, or rollback on an exact device.
