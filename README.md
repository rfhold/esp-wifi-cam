# ESP Wi-Fi Cam

This repository contains `no_std` firmware for a Seeed Studio XIAO ESP32-S3 Sense with an OV3660 camera. The firmware connects to a WPA2 network, captures hardware-compressed JPEG frames, and serves snapshots and an MJPEG stream over HTTP.

## Prerequisites

- A Seeed Studio XIAO ESP32-S3 Sense with its OV3660 camera expansion board
- The Espressif Rust toolchain selected by [`rust-toolchain.toml`](rust-toolchain.toml)
- `espflash` for the configured flash and monitor path
- Access to a WPA2-Personal network

## Configuration

Export these required variables before a build or compile check:

| Variable | Purpose |
| --- | --- |
| `SSID` | Wi-Fi network name |
| `PASSWORD` | WPA2-Personal password |
| `HOSTNAME_PREFIX` | Stable fleet prefix for the DHCP Option 12 hostname; maximum 25 bytes |

The compiler embeds these values in the firmware. Credentials can remain recoverable from firmware binaries and other build artifacts.

At runtime, the firmware appends a hyphen and the final three station MAC bytes as exactly six lowercase hexadecimal characters. For example, `HOSTNAME_PREFIX=argus` and a station MAC ending in `a1:b2:c3` produce `argus-a1b2c3`. The 25-byte prefix limit keeps the generated hostname within Embassy's 32-byte DHCP hostname capacity.

Never commit credentials. The repository ignores `.env`, but Cargo does not load that file automatically.

## Build And Flash

Build the release firmware after the required variables exist in the shell:

```sh
SSID=test PASSWORD=test HOSTNAME_PREFIX=argus cargo build --release
```

Flash and monitor an attached ESP32-S3:

```sh
cargo run --release
```

`cargo run --release` invokes the configured `espflash flash --monitor` runner. It mutates connected hardware, so an agent requires explicit authorization before use.

See [firmware documentation](docs/firmware.md) for architecture, behavior, safety boundaries, and local checks.

## Camera Endpoints

After DHCP completes, use the generated hostname or logged IPv4 address:

| Path | Response |
| --- | --- |
| `/capture.jpg` | One Full HD (`1920x1080`) JPEG frame |
| `/stream` | Continuous Full HD (`1920x1080`) `multipart/x-mixed-replace` MJPEG stream |

The stream is designed for ingestion by go2rtc. Because the ESP32-S3 produces JPEG rather than H.264, transcode the source on the NVR before using it for Frigate recording or detection.

## Repository Map

| Path | Responsibility |
| --- | --- |
| [`src/main.rs`](src/main.rs) | Firmware entry point, OV3660 capture, Wi-Fi lifecycle, and HTTP service |
| [`crates/ov3660/`](crates/ov3660/) | Bare-metal OV3660 sensor, ESP32-S3 LCD_CAM/GDMA capture, and JPEG parsing |
| [`Cargo.toml`](Cargo.toml) | Rust package, dependency pins, and release profile |
| [`.cargo/config.toml`](.cargo/config.toml) | ESP32-S3 target, linker flags, and flash runner |
| [`rust-toolchain.toml`](rust-toolchain.toml) | Espressif Rust toolchain selection |
| [`docs/`](docs/) | Canonical firmware documentation |

## Documentation

Use the [documentation index](docs/README.md) to find authoritative technical guidance.
