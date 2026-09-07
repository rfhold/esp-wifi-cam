# Signed A/B OTA

This document defines the canonical provisioning-independent release, manifest, transport, partition, boot, and signing contract.

## Trust And Transport

The only OTA origin is `https://git.holdenitdown.net`. Firmware constructs URLs from that constant and signed relative paths, requires HTTPS with the checked-in ISRG Root X1 DER certificate, rejects every non-success response including redirects, and requires `Content-Length` for both responses. The TLS receive buffer is 20480 bytes. The embedded TLS verifier has no trusted clock and a 4096-byte certificate-chain capacity. Ed25519 manifest verification, not TLS alone, is the release authenticity boundary.

[`keys/ota-public.der`](../keys/ota-public.der) is a 44-byte Ed25519 SubjectPublicKeyInfo DER trust anchor with SHA-256 fingerprint `6eadf451f13c0be6714da1a25eee22421a67203195d1128bfc98ba68cf12a5b7`. The firmware accepts that exact DER structure and no other algorithm. OpenBao remains the authoritative private-key store. Secret delivery may materialize the seeded key only as the release job's temporary PKCS#8 PEM input; it must never enter the repository, logs, caches, or release artifacts.

## Manifest Location And Envelope

Stable devices fetch:

```text
https://git.holdenitdown.net/rfhold/esp-wifi-cam/releases/download/stable/manifest.json
```

Prerelease devices fetch the same path with `prerelease` in place of `stable`. The complete response is one bounded signed envelope, so signed data and signature cannot come from separate fetches:

```json
{"signed":{"schema":1,"board":"seeed-xiao-esp32s3-sense","target":"xtensa-esp32s3-none-elf","track":"stable","version":"1.2.4","path":"/rfhold/esp-wifi-cam/releases/download/v1.2.4/esp-wifi-cam-1.2.4-esp32s3.bin","length":1234567,"sha256":"<64 lowercase hex>"},"signature":"<Ed25519 base64url without padding>"}
```

The envelope is at most 1536 bytes and rejects unknown or trailing fields. `length` must be nonzero and no greater than `0x330000`. The asset path must be at most 255 bytes, start with `/rfhold/esp-wifi-cam/releases/download/`, end in `.bin`, use only ASCII letters, digits, slash, dot, hyphen, and underscore, and contain no `..`, query, fragment, or doubled slash.

## Canonical Signature Input

The producer serializes the `signed` object as compact UTF-8 JSON in exactly this field order, with decimal `length`, lowercase hexadecimal `sha256`, and no whitespace:

```text
{"schema":1,"board":"...","target":"...","track":"...","version":"...","path":"...","length":123,"sha256":"..."}
```

The 64-byte Ed25519 signature covers the byte concatenation:

```text
esp-wifi-cam-ota-manifest-v1\0 || canonical signed-object UTF-8 bytes
```

The signature is encoded as unpadded base64url in the envelope. Before verification, firmware requires schema `1`, the exact board and target, the configured distribution track, an accepted semantic version, a safe path, a bounded length, and lowercase SHA-256. It reconstructs the canonical signed object and verifies with Ed25519 strict verification.

## Version Policy

- A stable device accepts only a strictly newer stable semantic version with no build metadata.
- A prerelease device accepts a strictly newer stable version or a strictly newer prerelease whose identifier is exactly `rc.N`.
- `N` is a positive unpadded decimal integer. Other prerelease forms, equal versions, downgrades, and build metadata are rejected.
- The running version is `CARGO_PKG_VERSION`; release automation must set the package version to the released semantic version before building.

The envelope `track` identifies the selected distribution manifest. A prerelease manifest may therefore point to either an accepted `rc.N` or stable version.

## Local Ignore-Version Override

`scripts/flash.sh --ignore-ota-version <semver>` creates and writes the same fixed OTA-ignore record to both reserved config-sector addresses `0xb000` and `0xc000` after its normal guarded app flash. `<semver>` must satisfy the OTA version syntax: no build metadata, and a prerelease may only be exact `rc.N`. `scripts/flash.sh --clear-ignore-ota-version` writes an explicit clear record to both addresses instead. The flags are mutually exclusive, reject invalid values through the host release tool, require `ESPFLASH_PORT` for the explicitly authorized serial port, and fail nonzero if record generation or either flash write fails. The app flash and both record writes keep the newly flashed firmware from starting; after both writes, the script resets once and starts monitoring without another reset. Temporary host record data is removed on exit.

This is persistent local physical-operator state, not a release field or remotely configurable setting. The firmware fetches and verifies the manifest normally, including signature and every existing acceptance rule, before consulting the journal. It skips only the install when the stored and verified manifest version strings exactly match. A different valid signed candidate installs normally. Missing, corrupt, unreadable, or clear records do not suppress an update.

## Release Artifact

Build one credential-free release binary, then create an app-only ESP-IDF image:

```sh
cargo build --locked --release
espflash save-image --chip esp32s3 \
  target/xtensa-esp32s3-none-elf/release/esp-wifi-cam \
  esp-wifi-cam-1.2.4-esp32s3.bin
```

Do not use `--merge`. CI base64-decodes its seeded OpenBao value into a temporary Ed25519 PKCS#8 PEM file outside this tool, then invokes the host-only manifest producer:

```sh
cargo +stable run -p release-tool \
  --target x86_64-unknown-linux-gnu \
  --config 'unstable.build-std=[]' -- \
  --track stable \
  --version 1.2.4 \
  --asset-path /rfhold/esp-wifi-cam/releases/download/v1.2.4/esp-wifi-cam-1.2.4-esp32s3.bin \
  --image esp-wifi-cam-1.2.4-esp32s3.bin \
  --output manifest.json \
  --public-key keys/ota-public.der \
  --private-key /path/to/temporary-private-key.pem \
  --max-slot-length 0x330000
```

All eight options are required. `--track` is `stable` or `prerelease`; `--max-slot-length` accepts decimal or a `0x` hexadecimal value. The tool validates release policy and the app-only image, streams its SHA-256, checks that the PKCS#8-derived public key exactly equals the supplied DER, requires the recorded DER fingerprint, signs and self-verifies with `ed25519-dalek`, and atomically renames the bounded envelope into place. It prints only the public output path, never private key or signature content. CI must remove its temporary private PEM through its secret-handling cleanup path.

CI uploads the exact image byte sequence at the signed path and publishes the generated envelope at the selected track URL. Publishing order must make the immutable versioned asset available before replacing its track manifest.

## Pipelines-As-Code Releases

`.tekton/esp-wifi-cam.yaml` validates pull requests, pushes, and incoming runs targeting `main`. It clones the full lowercase webhook commit detached, runs formatting, host tests for `ota-core`, `release-tool`, and `ov3660`, and performs the locked release firmware check.

`.tekton/esp-wifi-cam-release.yaml` accepts only an annotated `vX.Y.Z` tag whose commit is the exact webhook revision, is reachable from `origin/main`, and equals the root Cargo package version. It publishes a non-prerelease immutable version release containing `esp-wifi-cam-${version}-esp32s3.bin`, stable and prerelease signed envelopes, and `ota-public.der`. After that succeeds, it updates both channel releases with their respective envelope as the sole `manifest.json` asset. Both envelopes point to the stable version image so prerelease devices can advance to it.

`.tekton/esp-wifi-cam-prerelease.yaml` applies the same provenance checks to exact `vX.Y.Z-rc.N` tags. It publishes a prerelease immutable version release containing the app image, prerelease signed envelope, and public DER, then updates only the `prerelease` channel release. Release notes are taken from the annotated tag body.

Version publication is rerunnable without weakening immutability. An absent version release is created. An existing version release is accepted only when its tag, target commit, title, body, prerelease and draft flags, exact asset names, asset lengths, and downloaded asset SHA-256 values match the intended local release; an exact match skips version upsert and continues to channel reconciliation. Any mismatch fails closed.

Channel publication is monotonic. Before each channel upsert, CI downloads the existing `manifest.json` when present, verifies its Ed25519 signature and canonical fields with `keys/ota-public.der`, and compares the signed semantic versions under the firmware's stable or prerelease policy. A newer candidate is accepted, an older candidate is rejected, and an equal candidate is accepted only when the complete envelope bytes are identical. Forgejo release metadata is used only to enforce the channel's sole-asset shape, never to order versions.

The release build never receives Wi-Fi credentials and uses `espflash save-image` without `--merge`. Only after the app image and host release tool have built does a signing Task authenticate as `kuri-forgejo-release-v1`. It retrieves `private-key-base64` from `kv/data/ci/esp-wifi-cam/firmware-signing`, decodes the PKCS#8 PEM only in a memory-backed session volume, signs and self-verifies each envelope, and removes the temporary key. OpenBao and Forgejo HTTP authorization headers are supplied through mode-0600 curl configuration files in that memory-backed volume; token values are not exported or placed in process arguments. The build Task does not mount that session or projected identity. Forgejo publication uses the existing `forgejo-release-upsert` Task with `kv/data/ci/kuri/forgejo-release`.

The shared CI image currently has no locally recorded digest, so pipeline steps use `cr.holdenitdown.net/rfhold/general-ci:latest` with `imagePullPolicy: Always`. Remote shared release and OpenBao steps are also overridden to pull every run. The mutable image tag, the shared Task reference to the mutable homelab `main` branch, and the unavoidable check-then-upsert races between state verification and publication remain supply-chain and concurrency risks; publication must not be run concurrently for the same tag or channel.

## Partitions And Installation

[`partitions.csv`](../partitions.csv) defines this 8 MiB layout:

| Name | Offset | Size | Use |
| --- | --- | --- | --- |
| `config` | `0x9000` | `0x4000` | Provisioning journals at `0x9000`/`0xa000`; local OTA-ignore journals at reserved `0xb000`/`0xc000` |
| `otadata` | `0xd000` | `0x2000` | ESP-IDF OTA selection records |
| `phy_init` | `0xf000` | `0x1000` | PHY initialization data |
| `factory` | `0x10000` | `0x180000` | Initial recovery-capable application |
| `ota_0` | `0x1a0000` | `0x330000` | OTA application slot 0 |
| `ota_1` | `0x4d0000` | `0x330000` | OTA application slot 1 |

Before reading or writing provisioning or OTA-ignore sectors, firmware requires exactly 8 MiB of flash and validates the complete listed partition table through the same implementation used by the OTA task. The unused `0x10000` alignment gap after the factory partition permits both OTA application offsets to meet ESP-IDF's `0x10000` alignment requirement. An invalid layout halts startup without provisioning or network services. With a valid layout, OTA verifies the manifest first, applies an exact local ignore-version match only after that verification, then streams an unsuppressed signed image to the inactive slot in at most 4096-byte chunks, hashes network bytes, immediately reads each chunk back, hashes persisted bytes independently, and requires both hashes to match. Only then does it select the slot, set its state to `New`, and software-reset.

## Bootloader And Confirmation

[`scripts/build-bootloader.sh`](../scripts/build-bootloader.sh) accepts only a clean ESP-IDF checkout at commit `3ad36321ea7e6183986e19c7e05ee03052d181c3`. [`bootloader/sdkconfig.defaults`](../bootloader/sdkconfig.defaults) enables ESP32-S3, DIO, 40 MHz, 8 MiB flash, partition offset `0x8000`, reproducible metadata, and `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y`. No bootloader binary is checked in or fabricated.

[`scripts/flash.sh`](../scripts/flash.sh) refuses to flash until the build script has produced `bootloader/build/bootloader.bin` and a matching SHA-256 file. A booted OTA image remains pending until the OV3660 initializes and capture produces one complete JPEG frame; only then does firmware mark it valid. A rollback-enabled bootloader can reject an image that resets before confirmation.

The pinned `OtaUpdater` performs activation and the `New` state as two flash writes. Power loss after slot selection but before the state write can leave the new slot selected without the intended `New` marker. The current dependency does not expose enough low-level OTA record state to combine those changes safely, so activation is not claimed atomic. Download, write, read-back, and hash failures occur before either activation write.
