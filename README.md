# ESP Wi-Fi Cam

This repository contains minimal `no_std` firmware for an ESP32-S3. The firmware connects to a WPA2 network and acquires an IPv4 address through DHCPv4.

## Prerequisites

- An ESP32-S3 device
- The Espressif Rust toolchain selected by [`rust-toolchain.toml`](rust-toolchain.toml)
- `espflash` for the configured flash and monitor path
- Access to a WPA2-Personal network

## Configuration

Export these required variables before a build or compile check:

| Variable | Purpose |
| --- | --- |
| `SSID` | Wi-Fi network name |
| `PASSWORD` | WPA2-Personal password |
| `HOSTNAME` | DHCP Option 12 hostname |

The compiler embeds these values in the firmware. Credentials can remain recoverable from firmware binaries and other build artifacts.

Never commit credentials. The repository ignores `.env`, but Cargo does not load that file automatically.

## Build And Flash

Build the release firmware after the required variables exist in the shell:

```sh
cargo build --release
```

Flash and monitor an attached ESP32-S3:

```sh
cargo run --release
```

`cargo run --release` invokes the configured `espflash flash --monitor` runner. It mutates connected hardware, so an agent requires explicit authorization before use.

See [firmware documentation](docs/firmware.md) for architecture, behavior, safety boundaries, and local checks.

## Repository Map

| Path | Responsibility |
| --- | --- |
| [`src/main.rs`](src/main.rs) | Firmware entry point, Wi-Fi lifecycle, and network tasks |
| [`Cargo.toml`](Cargo.toml) | Rust package, dependency pins, and release profile |
| [`.cargo/config.toml`](.cargo/config.toml) | ESP32-S3 target, linker flags, and flash runner |
| [`rust-toolchain.toml`](rust-toolchain.toml) | Espressif Rust toolchain selection |
| [`docs/`](docs/) | Canonical firmware documentation |

## Documentation

Use the [documentation index](docs/README.md) to find authoritative technical guidance.
