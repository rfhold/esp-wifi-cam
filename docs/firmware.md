# Firmware

This document defines the current architecture, configuration, runtime behavior, safety boundaries, and validation path.

## Architecture

| Surface | Responsibility |
| --- | --- |
| [`src/main.rs`](../src/main.rs) | Initializes the XIAO ESP32-S3 Sense and runs camera, Wi-Fi, network, and HTTP tasks. |
| [`crates/ov3660/`](../crates/ov3660/) | Implements generic OV3660 SCCB control, ESP32-S3 LCD_CAM/GDMA capture, and incremental JPEG extraction. |
| [`Cargo.toml`](../Cargo.toml) | Declares the `no_std` firmware package and pins ESP crates to `esp-hal-v1.2.0`. |
| [`.cargo/config.toml`](../.cargo/config.toml) | Selects `xtensa-esp32s3-none-elf`, linker flags, defmt logs, and the `espflash` runner. |
| [`rust-toolchain.toml`](../rust-toolchain.toml) | Selects the Espressif `esp` Rust toolchain. |

The firmware uses `esp-hal` for hardware initialization and `esp-rtos` for the Embassy executor. The in-tree `ov3660` crate configures the sensor over SCCB-compatible I2C and captures its JPEG byte stream through ESP32-S3 LCD_CAM and GDMA. `esp-radio` provides the Wi-Fi station interface. `embassy-net` provides DHCPv4 and the HTTP TCP service.

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
2. The firmware allocates two internal heap regions and registers external PSRAM with the allocator. Two fixed-capacity 512 KiB JPEG frame buffers are allocated explicitly in PSRAM.
3. ESP-RTOS starts from the first timer in `TIMG0`.
4. The OV3660 receives a 10 MHz XCLK and is configured for Full HD (`1920x1080`) resolution in JPEG mode at quality 12. The XIAO module corrections enable vertical flip, set brightness to `+1`, and set saturation to `-2`.
5. LCD_CAM and GDMA capture into a 20 KiB internal-RAM DMA ring. An incremental parser assembles complete JPEG frames into the bounded PSRAM buffers and keeps the most recent available frame when the HTTP consumer falls behind. If the DMA ring fills before frame EOF, capture drops that frame, resets the camera receive path, and resynchronizes at a later frame boundary.
6. The radio configures a WPA2-Personal station from `SSID` and `PASSWORD`.
7. The firmware reads the station MAC from eFuse, derives the hostname from `HOSTNAME_PREFIX` and the final three MAC bytes, then sends it as DHCP Option 12 while requesting DHCPv4 configuration.
8. Separate tasks drive camera capture, Wi-Fi connection management, the network stack, and the HTTP server.
9. The main task waits for IPv4 configuration, logs the acquired address and prefix through defmt, then remains pending while worker tasks run.
10. After a failure or disconnect, the Wi-Fi task waits five seconds before another connection attempt.

The HTTP server listens on port 80 and serves one connection at a time:

| Path | Media type | Behavior |
| --- | --- | --- |
| `/capture.jpg` | `image/jpeg` | Sends the next complete frame and closes the connection. |
| `/stream` | `multipart/x-mixed-replace` | Sends complete JPEG frames using the `frameboundary` multipart boundary until the client disconnects. |

Other request targets return `404 Not Found`. A five-second wait without a captured frame ends an MJPEG response or returns `503 Service Unavailable` for a snapshot. The firmware does not provide authentication, TLS, RTSP, H.264 encoding, multiple simultaneous HTTP clients, or a static IP path. Keep the camera on a trusted network and perform any H.264 conversion on the NVR.

## Failure Behavior

- A missing compile-time variable stops compilation.
- A `HOSTNAME_PREFIX` longer than 25 bytes causes a startup panic before DHCP configuration.
- A failed fixed-capacity hostname conversion causes a startup panic.
- Missing PSRAM or failed fixed-capacity frame allocation causes a startup panic.
- A missing or unresponsive OV3660, failed sensor initialization, or failed camera DMA startup causes a startup panic.
- Capture DMA errors and timeouts restart capture and force JPEG parser resynchronization.
- JPEG frames larger than 512 KiB are dropped and parsing resumes at a later frame boundary.
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

Run the OV3660 host tests separately:

```sh
cargo +stable test -p ov3660 --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]'
```

These checks validate source format, release compilation, SCCB command generation, geometry configuration, and JPEG parsing. They do not prove Wi-Fi association, DHCP service, radio performance, camera signal polarity, DMA timing, PSRAM integrity, or hardware compatibility. Hardware validation requires an explicitly authorized flash followed by checks of both camera endpoints.
