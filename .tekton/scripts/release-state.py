#!/usr/bin/env python3
import argparse
import base64
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import tempfile

BOARD = "seeed-xiao-esp32s3-sense"
TARGET = "xtensa-esp32s3-none-elf"
DOMAIN = b"esp-wifi-cam-ota-manifest-v1\0"
MANIFEST_MAX_LEN = 1536
SLOT_MAX_LEN = 0x330000
VERSION_RE = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-rc\.([1-9][0-9]*))?$")
SHA_RE = re.compile(r"^[0-9a-f]{64}$")
PATH_RE = re.compile(r"^[A-Za-z0-9/._-]+$")


class PolicyError(Exception):
    pass


def fail(message: str) -> None:
    raise PolicyError(message)


def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            fail("JSON contains a duplicate key")
        result[key] = value
    return result


def load_json(path: pathlib.Path, maximum: int) -> dict[str, object]:
    data = path.read_bytes()
    if not data or len(data) > maximum:
        fail(f"{path} has an invalid size")
    try:
        value = json.loads(data, object_pairs_hook=unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PolicyError(f"{path} is not valid JSON") from error
    if not isinstance(value, dict):
        fail(f"{path} is not a JSON object")
    return value


def parse_version(value: object) -> tuple[int, int, int, int, int]:
    if not isinstance(value, str):
        fail("manifest version is not a string")
    match = VERSION_RE.fullmatch(value)
    if match is None:
        fail("manifest version is not stable SemVer or exact rc.N")
    major, minor, patch = (int(match.group(index)) for index in range(1, 4))
    rc = match.group(4)
    return (major, minor, patch, 1 if rc is None else 0, 0 if rc is None else int(rc))


def verify_manifest(
    path: pathlib.Path,
    public_key: pathlib.Path,
    expected_fingerprint: str,
    track: str,
) -> tuple[tuple[int, int, int, int, int], bytes]:
    envelope_bytes = path.read_bytes()
    if not envelope_bytes or len(envelope_bytes) > MANIFEST_MAX_LEN:
        fail("manifest envelope has an invalid size")
    envelope = load_json(path, MANIFEST_MAX_LEN)
    if set(envelope) != {"signed", "signature"}:
        fail("manifest envelope fields are invalid")
    signed = envelope["signed"]
    signature_text = envelope["signature"]
    expected_fields = ["schema", "board", "target", "track", "version", "path", "length", "sha256"]
    if not isinstance(signed, dict) or set(signed) != set(expected_fields):
        fail("signed manifest fields are invalid")
    if (
        isinstance(signed["schema"], bool)
        or not isinstance(signed["schema"], int)
        or signed["schema"] != 1
        or signed["board"] != BOARD
        or signed["target"] != TARGET
    ):
        fail("signed manifest identity is invalid")
    if signed["track"] != track:
        fail("signed manifest track is invalid")
    version = parse_version(signed["version"])
    if track == "stable" and version[3] == 0:
        fail("stable channel manifest contains a prerelease")
    asset_path = signed["path"]
    if (
        not isinstance(asset_path, str)
        or len(asset_path.encode("ascii", errors="ignore")) != len(asset_path)
        or len(asset_path) > 255
        or not asset_path.startswith("/rfhold/esp-wifi-cam/releases/download/")
        or not asset_path.endswith(".bin")
        or ".." in asset_path
        or "//" in asset_path
        or not PATH_RE.fullmatch(asset_path)
    ):
        fail("signed manifest asset path is invalid")
    length = signed["length"]
    if isinstance(length, bool) or not isinstance(length, int) or length < 1 or length > SLOT_MAX_LEN:
        fail("signed manifest image length is invalid")
    sha256 = signed["sha256"]
    if not isinstance(sha256, str) or SHA_RE.fullmatch(sha256) is None:
        fail("signed manifest image hash is invalid")
    if not isinstance(signature_text, str) or re.fullmatch(r"[A-Za-z0-9_-]{86}", signature_text) is None:
        fail("manifest signature encoding is invalid")

    public_der = public_key.read_bytes()
    if len(public_der) != 44 or hashlib.sha256(public_der).hexdigest() != expected_fingerprint:
        fail("manifest public key fingerprint is invalid")
    try:
        signature = base64.urlsafe_b64decode(signature_text + "==")
    except ValueError as error:
        raise PolicyError("manifest signature cannot be decoded") from error
    if len(signature) != 64:
        fail("manifest signature length is invalid")
    canonical_signed = {field: signed[field] for field in expected_fields}
    canonical = json.dumps(canonical_signed, separators=(",", ":"), ensure_ascii=True).encode("ascii")
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        input_path = root / "input"
        signature_path = root / "signature"
        input_path.write_bytes(DOMAIN + canonical)
        signature_path.write_bytes(signature)
        result = subprocess.run(
            [
                "openssl",
                "pkeyutl",
                "-verify",
                "-pubin",
                "-inkey",
                str(public_key),
                "-keyform",
                "DER",
                "-rawin",
                "-in",
                str(input_path),
                "-sigfile",
                str(signature_path),
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    if result.returncode != 0:
        fail("manifest signature verification failed")
    return version, envelope_bytes


def check_channel(arguments: argparse.Namespace) -> None:
    candidate_version, candidate_bytes = verify_manifest(
        arguments.candidate, arguments.public_key, arguments.expected_key_sha256, arguments.track
    )
    if arguments.current is None:
        return
    current_version, current_bytes = verify_manifest(
        arguments.current, arguments.public_key, arguments.expected_key_sha256, arguments.track
    )
    if candidate_version < current_version:
        fail("candidate channel manifest is a downgrade")
    if candidate_version == current_version and candidate_bytes != current_bytes:
        fail("equal channel versions have different signed envelope bytes")


def file_digest(path: pathlib.Path) -> tuple[int, str]:
    digest = hashlib.sha256()
    length = 0
    with path.open("rb") as source:
        while chunk := source.read(64 * 1024):
            length += len(chunk)
            digest.update(chunk)
    return length, digest.hexdigest()


def check_version(arguments: argparse.Namespace) -> None:
    release = load_json(arguments.release_json, 2 * 1024 * 1024)
    expected_metadata = {
        "tag_name": arguments.tag,
        "target_commitish": arguments.revision,
        "name": arguments.title,
        "body": arguments.body.read_text(encoding="utf-8"),
        "prerelease": arguments.prerelease,
        "draft": False,
    }
    for field, expected in expected_metadata.items():
        if release.get(field) != expected:
            fail(f"existing version release {field} does not match")
    assets = release.get("assets")
    if not isinstance(assets, list):
        fail("existing version release assets are invalid")
    expected_names = set(arguments.assets)
    actual: dict[str, dict[str, object]] = {}
    for asset in assets:
        if not isinstance(asset, dict) or not isinstance(asset.get("name"), str):
            fail("existing version release has invalid asset metadata")
        name = asset["name"]
        if name in actual:
            fail("existing version release has duplicate asset names")
        actual[name] = asset
    if set(actual) != expected_names:
        fail("existing version release asset names do not match")
    for name in sorted(expected_names):
        local = arguments.local_dir / name
        remote = arguments.remote_dir / name
        if not local.is_file() or local.is_symlink() or not remote.is_file() or remote.is_symlink():
            fail(f"existing version release asset {name} is unavailable")
        local_length, local_hash = file_digest(local)
        remote_length, remote_hash = file_digest(remote)
        size = actual[name].get("size")
        if isinstance(size, bool) or not isinstance(size, int) or size != local_length:
            fail(f"existing version release asset {name} metadata length does not match")
        if remote_length != local_length or remote_hash != local_hash:
            fail(f"existing version release asset {name} content does not match")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser()
    commands = root.add_subparsers(dest="command", required=True)
    channel = commands.add_parser("channel")
    channel.add_argument("--track", choices=("stable", "prerelease"), required=True)
    channel.add_argument("--public-key", type=pathlib.Path, required=True)
    channel.add_argument("--expected-key-sha256", required=True)
    channel.add_argument("--candidate", type=pathlib.Path, required=True)
    channel.add_argument("--current", type=pathlib.Path)
    channel.set_defaults(run=check_channel)
    version = commands.add_parser("version")
    version.add_argument("--release-json", type=pathlib.Path, required=True)
    version.add_argument("--tag", required=True)
    version.add_argument("--revision", required=True)
    version.add_argument("--title", required=True)
    version.add_argument("--body", type=pathlib.Path, required=True)
    version.add_argument("--prerelease", action="store_true")
    version.add_argument("--local-dir", type=pathlib.Path, required=True)
    version.add_argument("--remote-dir", type=pathlib.Path, required=True)
    version.add_argument("assets", nargs="+")
    version.set_defaults(run=check_version)
    return root


def main() -> int:
    try:
        arguments = parser().parse_args()
        arguments.run(arguments)
    except (OSError, PolicyError, subprocess.SubprocessError) as error:
        print(f"release-state: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
