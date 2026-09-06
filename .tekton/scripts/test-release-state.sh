#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C
umask 077

root=$(mktemp -d)
cleanup() {
  rm -rf -- "$root"
}
trap cleanup EXIT

openssl genpkey -algorithm ED25519 -out "$root/signing-key.pem" >/dev/null 2>&1
openssl pkey -in "$root/signing-key.pem" -pubout -outform DER -out "$root/public.der" >/dev/null 2>&1
fingerprint=$(sha256sum "$root/public.der" | cut -d' ' -f1)

make_manifest() {
  python3 - "$root/signing-key.pem" "$1" "$2" "$3" "${4:-stable}" <<'PY'
import base64
import json
import pathlib
import subprocess
import sys
import tempfile

private_key, output, version, image_hash, track = sys.argv[1:]
signed = {
    "schema": 1,
    "board": "seeed-xiao-esp32s3-sense",
    "target": "xtensa-esp32s3-none-elf",
    "track": track,
    "version": version,
    "path": f"/rfhold/esp-wifi-cam/releases/download/v{version}/esp-wifi-cam-{version}-esp32s3.bin",
    "length": 4,
    "sha256": image_hash,
}
canonical = json.dumps(signed, separators=(",", ":")).encode("ascii")
with tempfile.TemporaryDirectory() as directory:
    input_path = pathlib.Path(directory) / "input"
    signature_path = pathlib.Path(directory) / "signature"
    input_path.write_bytes(b"esp-wifi-cam-ota-manifest-v1\0" + canonical)
    subprocess.run(
        ["openssl", "pkeyutl", "-sign", "-inkey", private_key, "-rawin", "-in", str(input_path), "-out", str(signature_path)],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    signature = base64.urlsafe_b64encode(signature_path.read_bytes()).rstrip(b"=").decode("ascii")
pathlib.Path(output).write_text(json.dumps({"signed": signed, "signature": signature}, separators=(",", ":")), encoding="ascii")
PY
}

hash_a=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
hash_b=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
make_manifest "$root/current.json" 1.2.0 "$hash_a"
make_manifest "$root/older.json" 1.1.9 "$hash_a"
make_manifest "$root/newer.json" 1.2.1 "$hash_a"
make_manifest "$root/equal-different.json" 1.2.0 "$hash_b"
cp "$root/current.json" "$root/equal-identical.json"

channel_check() {
  python3 .tekton/scripts/release-state.py channel \
    --track stable \
    --public-key "$root/public.der" \
    --expected-key-sha256 "$fingerprint" \
    --candidate "$1" \
    --current "$root/current.json"
}
expect_reject() {
  if "$@" >/dev/null 2>&1; then
    printf '%s\n' 'fixture unexpectedly passed' >&2
    exit 1
  fi
}

expect_reject channel_check "$root/older.json"
channel_check "$root/equal-identical.json"
expect_reject channel_check "$root/equal-different.json"
channel_check "$root/newer.json"

make_manifest "$root/current-rc.json" 1.3.0-rc.2 "$hash_a" prerelease
make_manifest "$root/zero-rc.json" 1.3.0-rc.0 "$hash_a" prerelease
make_manifest "$root/older-rc.json" 1.3.0-rc.1 "$hash_a" prerelease
make_manifest "$root/newer-rc.json" 1.3.0-rc.3 "$hash_a" prerelease
make_manifest "$root/stable-after-rc.json" 1.3.0 "$hash_a" prerelease
prerelease_check() {
  python3 .tekton/scripts/release-state.py channel \
    --track prerelease \
    --public-key "$root/public.der" \
    --expected-key-sha256 "$fingerprint" \
    --candidate "$1" \
    --current "$root/current-rc.json"
}
expect_reject prerelease_check "$root/zero-rc.json"
expect_reject prerelease_check "$root/older-rc.json"
prerelease_check "$root/newer-rc.json"
prerelease_check "$root/stable-after-rc.json"

install -d -m 0700 "$root/local" "$root/remote"
printf '%s' image > "$root/local/image.bin"
printf '%s' manifest > "$root/local/manifest.json"
cp "$root/local/image.bin" "$root/remote/image.bin"
cp "$root/local/manifest.json" "$root/remote/manifest.json"
printf '%s' 'Release notes.' > "$root/body.md"
cat > "$root/release.json" <<'JSON'
{"tag_name":"v1.2.0","target_commitish":"0123456789abcdef0123456789abcdef01234567","name":"ESP Wi-Fi Cam v1.2.0","body":"Release notes.","prerelease":false,"draft":false,"assets":[{"name":"image.bin","size":5},{"name":"manifest.json","size":8}]}
JSON
version_check() {
  python3 .tekton/scripts/release-state.py version \
    --release-json "$root/release.json" \
    --tag v1.2.0 \
    --revision 0123456789abcdef0123456789abcdef01234567 \
    --title 'ESP Wi-Fi Cam v1.2.0' \
    --body "$root/body.md" \
    --local-dir "$root/local" \
    --remote-dir "$root/remote" \
    image.bin manifest.json
}
version_check
printf '%s' mismatch > "$root/remote/image.bin"
expect_reject version_check
cp "$root/local/image.bin" "$root/remote/image.bin"
python3 - "$root/release.json" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
release = json.loads(path.read_text(encoding="ascii"))
release["target_commitish"] = "ffffffffffffffffffffffffffffffffffffffff"
path.write_text(json.dumps(release, separators=(",", ":")), encoding="ascii")
PY
expect_reject version_check

printf '%s\n' 'release state fixtures passed'
