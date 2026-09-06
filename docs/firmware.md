# Firmware

This document defines the current architecture, configuration, runtime behavior, safety boundaries, and validation path.

## Architecture

| Surface | Responsibility |
| --- | --- |
| [`src/main.rs`](../src/main.rs) | Initializes the ESP32-S3, starts ESP-RTOS, and runs Wi-Fi and network tasks. |
| [`Cargo.toml`](../Cargo.toml) | Declares the `no_std` firmware package and pins ESP crates to `esp-hal-v1.2.0`. |
| [`.cargo/config.toml`](../.cargo/config.toml) | Selects `xtensa-esp32s3-none-elf`, linker flags, defmt logs, and the `espflash` runner. |
| [`rust-toolchain.toml`](../rust-toolchain.toml) | Selects the Espressif `esp` Rust toolchain. |

The firmware uses `esp-hal` for hardware initialization and `esp-rtos` for the Embassy executor. `esp-radio` provides the Wi-Fi station interface. `embassy-net` provides DHCPv4.

## Compile-Time Configuration

`src/main.rs` reads each value through Rust's `env!` macro. A missing value causes a compile error.

| Variable | Use | Requirement |
| --- | --- | --- |
| `SSID` | Wi-Fi station network name | Required at compile time |
| `PASSWORD` | WPA2-Personal credential | Required at compile time |
| `HOSTNAME_PREFIX` | Stable fleet prefix for the DHCPv4 Option 12 hostname | Required at compile time; maximum 25 bytes |

The compiler embeds all three values in firmware artifacts. The password can remain recoverable from binaries and other artifacts.

The firmware derives the complete hostname at runtime as `<HOSTNAME_PREFIX>-<suffix>`. The suffix is exactly six lowercase hexadecimal characters formed directly from station MAC bytes 3, 4, and 5, in that order. For example, prefix `argus` and a station MAC ending in `a1:b2:c3` produce `argus-a1b2c3`. The prefix is limited to 25 bytes so the hyphen and six-character suffix fit Embassy's 32-byte DHCP hostname capacity.

The repository ignores `.env`. Cargo does not load `.env` without a separate shell or tool step.

## Runtime Behavior

1. The entry point selects the maximum CPU clock and initializes ESP32-S3 peripherals.
2. The firmware allocates reclaimed RAM and a second heap region.
3. ESP-RTOS starts from the first timer in `TIMG0`.
4. The radio configures a WPA2-Personal station from `SSID` and `PASSWORD`.
5. The firmware reads the station MAC from eFuse, derives the hostname from `HOSTNAME_PREFIX` and the final three MAC bytes, then sends it as DHCP Option 12 while requesting DHCPv4 configuration.
6. Separate tasks drive Wi-Fi connection management and the network stack.
7. The main task waits for IPv4 configuration and logs the acquired address and prefix through defmt.
8. After a failure or disconnect, the Wi-Fi task waits five seconds before another connection attempt.
9. After initial network configuration, the main task remains active with a 60-second sleep loop.

The current firmware does not implement camera capture, image transport, an application protocol, or a static IP path.

## Failure Behavior

- A missing compile-time variable stops compilation.
- A `HOSTNAME_PREFIX` longer than 25 bytes causes a startup panic before DHCP configuration.
- A failed fixed-capacity hostname conversion causes a startup panic.
- Wi-Fi connection failures produce defmt warnings and enter the five-second retry cycle.
- A disconnect produces a defmt warning and enters the same retry cycle.
- The main task waits indefinitely when DHCPv4 never supplies network configuration.

## Safety Boundaries

- Never commit `SSID`, `PASSWORD`, or other credential values.
- Treat firmware binaries and build artifacts as sensitive because they can retain compile-time credentials.
- Keep one-off device addresses, network names, and validation outcomes outside canonical documentation.
- Treat `cargo build`, `cargo check`, and `cargo fmt` as local commands that do not flash hardware.
- Treat `cargo run --release` as a hardware mutation because Cargo invokes the configured `espflash flash --monitor` runner.
- An agent must obtain explicit authorization for the exact connected device before it runs `cargo run --release` or `espflash`.

## Validation

Run local checks from the repository root. `cargo fmt --check` needs no firmware variables. `cargo check --release` requires `SSID`, `PASSWORD`, and `HOSTNAME_PREFIX`.

```sh
cargo fmt --check
SSID=test PASSWORD=test HOSTNAME_PREFIX=argus cargo check --release
```

Use non-production values for local checks. Do not record values in command output or documentation.

These checks validate source format and release compilation. They do not prove Wi-Fi association, DHCP service, radio performance, or hardware compatibility.
