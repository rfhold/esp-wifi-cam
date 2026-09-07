# Firmware

This document defines the firmware architecture, camera service, runtime configuration, failure behavior, and local validation. The signed update contract is canonical in [OTA](ota.md).

## Architecture

| Surface | Responsibility |
| --- | --- |
| [`src/main.rs`](../src/main.rs) | Initializes the ESP32-S3 and runs camera, Wi-Fi, network, HTTP, and OTA tasks. |
| [`src/provision.rs`](../src/provision.rs) | Loads or replaces the two-sector journaled USB provisioning record. |
| [`src/ota.rs`](../src/ota.rs) | Confirms healthy boots and performs HTTPS A/B downloads and flash verification. |
| [`crates/ota-core/`](../crates/ota-core/) | Implements bounded provisioning, signed manifest, path, channel, and version policy. |
| [`crates/ov3660/`](../crates/ov3660/) | Implements OV3660 control, ESP32-S3 LCD_CAM/GDMA capture, and JPEG extraction. |

The firmware uses `esp-hal` and `esp-rtos` with Embassy. Camera frame buffers remain in PSRAM; camera DMA, networking, TLS, and flash work buffers use internal memory. `esp-radio` provides WPA2 station mode, Embassy provides DHCPv4, DNS, and TCP, and reqwless plus embedded-tls provides HTTPS for OTA pulls.

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
5. DHCPv4 uses the derived hostname. Separate tasks drive Wi-Fi, networking, capture, HTTP, and OTA.
6. Two HTTP workers each own a fixed 1 KiB receive buffer, 4 KiB transmit buffer, and TCP socket. The server therefore accepts exactly two concurrent HTTP connections, intended for one Frigate `/stream` client and one independent `/status` or `/capture.jpg` request. `/capture.jpg` returns the next complete JPEG. `/stream` returns MJPEG with the `frameboundary` boundary. Concurrent stream consumers compete for the camera's bounded frame buffers; the firmware does not fan out or independently duplicate streams. `GET /status` returns a compact `application/json` object containing the compiled `version`, configured `track`, and `transfer_active` boolean. The status response contains no credentials, signing material, manifest URLs, asset paths, or asset digests. Other targets return `404 Not Found`.
7. The camera signals health only after initialization and one complete JPEG frame. Only then may the OTA task mark a `New` or `PendingVerify` image `Valid`.
8. The first update check starts 30 seconds after health confirmation and later checks run every six hours while network configuration is available.

New HTTP requests return `503 Service Unavailable` while an artifact is transferring. Existing streams close at a frame boundary. Capture continues, but no new stream consumes a frame during the transfer. There is no update upload endpoint.

## Failure Behavior

- Invalid or absent provisioning fails closed before network and camera startup.
- Flash capacity other than exactly 8 MiB or a partition table that differs from the complete required layout halts startup before any provisioning-sector write.
- Missing PSRAM, camera initialization failure, or camera DMA startup failure stops startup.
- Frames larger than 512 KiB and malformed JPEG streams are dropped and parser synchronization resumes later.
- Wi-Fi failure enters a five-second retry cycle; DHCP absence leaves the main task waiting.
- TLS, HTTP status, response length, manifest, signature, policy, path, flash write, read-back, or hash failure leaves the running slot selected.
- A downloaded image is activated only after the downloaded SHA-256 and independently hashed flash read-back both match the signed digest.

## Safety Boundaries

- Never commit Wi-Fi values, tokens, signing keys, or generated provisioning records.
- The checked-in Ed25519 key is public verification material. The private key remains only in OpenBao and is never a firmware build input.
- HTTPS never falls back to plaintext. TLS pins ISRG Root X1 but does not validate certificate time; Ed25519 is the artifact authenticity root.
- Treat `cargo check`, `cargo build`, host tests, formatting, and partition conversion as local non-hardware checks.
- Treat `cargo run --release`, `espflash flash`, erase commands, and serial provisioning as hardware mutations requiring explicit device authorization.

## Validation

```sh
cargo fmt --check
cargo +stable test -p ota-core --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]'
cargo +stable test -p ov3660 --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]'
cargo check --locked --release
cargo build --locked --release
espflash partition-table --skip-update-check --to-binary --output /tmp/partitions.bin partitions.csv
espflash partition-table --skip-update-check /tmp/partitions.bin
git diff --check
```

These checks do not flash hardware. They do not prove flash capacity, bootloader rollback, Wi-Fi association, TLS interoperability with the live Forgejo chain, camera timing, PSRAM integrity, update reboot, or rollback on an exact device.
