# Index

| Path | Info |
| --- | --- |
| [`README.md`](README.md) | Human entry point, prerequisites, and build path. |
| [`src/main.rs`](src/main.rs) | Firmware implementation for the ESP32-S3 Wi-Fi station. |
| [`crates/ota-core/`](crates/ota-core/) | Host-testable provisioning, manifest verification, and update policy. |
| [`crates/release-tool/`](crates/release-tool/) | Host-only release image validation and canonical manifest signing. |
| [`docs/README.md`](docs/README.md) | Canonical documentation index. |
| [`docs/firmware.md`](docs/firmware.md) | Firmware architecture, behavior, safety boundaries, and validation. |
| [`docs/ota.md`](docs/ota.md) | Canonical signed A/B OTA and release artifact contract. |
| [`.agents/skills/firmware-build-flash/SKILL.md`](.agents/skills/firmware-build-flash/SKILL.md) | Repository-specific Docker build and host flash/monitor guidance. |
| [`.agents/skills/reviewing-changes/SKILL.md`](.agents/skills/reviewing-changes/SKILL.md) | Repository-specific read-only review guidance. |

# Hints

- Read [`docs/README.md`](docs/README.md) and [`docs/firmware.md`](docs/firmware.md) before a firmware change.
- Use `agentic-documentation` for documentation changes.
- Use `planning-changes` and `making-changes` for firmware changes.
- Use `firmware-build-flash` for the Docker build and host flash or monitor tooling path.
- Use `reviewing-changes` for repository-specific review inputs.
- Treat `cargo run --release` as a hardware mutation subject to the authorization boundary in [`docs/firmware.md`](docs/firmware.md).
