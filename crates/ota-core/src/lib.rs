#![no_std]

use base64ct::{Base64UrlUnpadded, Encoding};
use core::fmt::Write;
use ed25519_dalek::{Signature, VerifyingKey};
use heapless::String;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const BOARD: &str = "seeed-xiao-esp32s3-sense";
pub const TARGET: &str = "xtensa-esp32s3-none-elf";
pub const MANIFEST_DOMAIN: &[u8] = b"esp-wifi-cam-ota-manifest-v1\0";
pub const MANIFEST_MAX_LEN: usize = 1536;
pub const CANONICAL_MAX_LEN: usize = 768;
pub const PROVISION_RECORD_LEN: usize = 160;
pub const SSID_MAX_LEN: usize = 32;
pub const PASSWORD_MAX_LEN: usize = 63;

const PROVISION_MAGIC: &[u8; 8] = b"EWCAM-P1";
const PROVISION_SCHEMA: u8 = 1;
const ED25519_SPKI_PREFIX: &[u8; 12] = &[
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Track {
    Stable,
    Prerelease,
}

impl Track {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Prerelease => "prerelease",
        }
    }
}

impl TryFrom<&str> for Track {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "stable" => Ok(Self::Stable),
            "prerelease" => Ok(Self::Prerelease),
            _ => Err(Error::Track),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Provisioning {
    pub sequence: u32,
    pub track: Track,
    pub ssid: String<SSID_MAX_LEN>,
    pub password: String<PASSWORD_MAX_LEN>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Base64,
    Bounds,
    Canonical,
    Hash,
    Key,
    Manifest,
    Path,
    Policy,
    Provision,
    Signature,
    Track,
    Utf8,
    Version,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<'a> {
    #[serde(borrow)]
    signed: SignedManifest<'a>,
    signature: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedManifest<'a> {
    schema: u8,
    board: &'a str,
    target: &'a str,
    track: &'a str,
    version: &'a str,
    path: &'a str,
    length: u32,
    sha256: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedManifest {
    pub track: Track,
    pub version: String<48>,
    pub path: String<256>,
    pub length: u32,
    pub sha256: [u8; 32],
}

pub fn parse_provision_command(line: &[u8], sequence: u32) -> Result<Provisioning, Error> {
    let line = core::str::from_utf8(line).map_err(|_| Error::Utf8)?;
    let line = line.trim_matches(['\r', '\n']);
    let mut fields = line.split(' ');
    if fields.next() != Some("P1") {
        return Err(Error::Provision);
    }
    let track = Track::try_from(fields.next().ok_or(Error::Provision)?)?;
    let ssid_encoded = fields.next().ok_or(Error::Provision)?;
    let password_encoded = fields.next().ok_or(Error::Provision)?;
    if fields.next().is_some() {
        return Err(Error::Provision);
    }

    let mut ssid_bytes = [0u8; SSID_MAX_LEN];
    let ssid =
        Base64UrlUnpadded::decode(ssid_encoded, &mut ssid_bytes).map_err(|_| Error::Base64)?;
    let mut password_bytes = [0u8; PASSWORD_MAX_LEN];
    let password = Base64UrlUnpadded::decode(password_encoded, &mut password_bytes)
        .map_err(|_| Error::Base64)?;
    if ssid.is_empty() || password.len() < 8 {
        return Err(Error::Provision);
    }

    let ssid = core::str::from_utf8(ssid).map_err(|_| Error::Utf8)?;
    let password = core::str::from_utf8(password).map_err(|_| Error::Utf8)?;
    Ok(Provisioning {
        sequence,
        track,
        ssid: ssid.try_into().map_err(|_| Error::Bounds)?,
        password: password.try_into().map_err(|_| Error::Bounds)?,
    })
}

pub fn encode_provision_record(
    provision: &Provisioning,
) -> Result<[u8; PROVISION_RECORD_LEN], Error> {
    if provision.ssid.is_empty() || provision.password.len() < 8 {
        return Err(Error::Provision);
    }
    let mut record = [0xff; PROVISION_RECORD_LEN];
    record[..8].copy_from_slice(PROVISION_MAGIC);
    record[8] = PROVISION_SCHEMA;
    record[9] = match provision.track {
        Track::Stable => 0,
        Track::Prerelease => 1,
    };
    record[10] = provision.ssid.len() as u8;
    record[11] = provision.password.len() as u8;
    record[12..16].copy_from_slice(&provision.sequence.to_le_bytes());
    record[16..16 + provision.ssid.len()].copy_from_slice(provision.ssid.as_bytes());
    record[48..48 + provision.password.len()].copy_from_slice(provision.password.as_bytes());
    let digest = Sha256::digest(&record[..128]);
    record[128..].copy_from_slice(&digest);
    Ok(record)
}

pub fn decode_provision_record(record: &[u8]) -> Result<Provisioning, Error> {
    if record.len() != PROVISION_RECORD_LEN
        || &record[..8] != PROVISION_MAGIC
        || record[8] != PROVISION_SCHEMA
    {
        return Err(Error::Provision);
    }
    let expected = Sha256::digest(&record[..128]);
    if expected.as_slice() != &record[128..] {
        return Err(Error::Hash);
    }
    let track = match record[9] {
        0 => Track::Stable,
        1 => Track::Prerelease,
        _ => return Err(Error::Track),
    };
    let ssid_len = record[10] as usize;
    let password_len = record[11] as usize;
    if ssid_len == 0 || ssid_len > SSID_MAX_LEN || !(8..=PASSWORD_MAX_LEN).contains(&password_len) {
        return Err(Error::Provision);
    }
    let ssid = core::str::from_utf8(&record[16..16 + ssid_len]).map_err(|_| Error::Utf8)?;
    let password = core::str::from_utf8(&record[48..48 + password_len]).map_err(|_| Error::Utf8)?;
    Ok(Provisioning {
        sequence: u32::from_le_bytes(record[12..16].try_into().map_err(|_| Error::Bounds)?),
        track,
        ssid: ssid.try_into().map_err(|_| Error::Bounds)?,
        password: password.try_into().map_err(|_| Error::Bounds)?,
    })
}

pub fn newest_provision_record(
    first: &[u8],
    second: &[u8],
) -> Result<(Provisioning, usize), Error> {
    match (
        decode_provision_record(first),
        decode_provision_record(second),
    ) {
        (Ok(a), Ok(b)) if sequence_is_newer(b.sequence, a.sequence) => Ok((b, 1)),
        (Ok(a), Ok(_)) => Ok((a, 0)),
        (Ok(a), Err(_)) => Ok((a, 0)),
        (Err(_), Ok(b)) => Ok((b, 1)),
        (Err(_), Err(_)) => Err(Error::Provision),
    }
}

pub fn remaining_window_ticks(start: u64, now: u64, window: u64) -> Option<u64> {
    let elapsed = now.checked_sub(start)?;
    if elapsed < window {
        Some(window - elapsed)
    } else {
        None
    }
}

pub fn verify_manifest(
    bytes: &[u8],
    public_key_spki: &[u8],
    configured_track: Track,
    current_version: &str,
    max_image_len: u32,
) -> Result<VerifiedManifest, Error> {
    if bytes.len() > MANIFEST_MAX_LEN {
        return Err(Error::Bounds);
    }
    let (envelope, used): (Envelope<'_>, usize) =
        serde_json_core::from_slice(bytes).map_err(|_| Error::Manifest)?;
    if used != bytes.len()
        || envelope.signed.schema != 1
        || envelope.signed.board != BOARD
        || envelope.signed.target != TARGET
    {
        return Err(Error::Manifest);
    }
    let manifest_track = Track::try_from(envelope.signed.track)?;
    if manifest_track != configured_track {
        return Err(Error::Policy);
    }
    validate_path(envelope.signed.path)?;
    if envelope.signed.length == 0 || envelope.signed.length > max_image_len {
        return Err(Error::Bounds);
    }
    let hash = decode_hex_32(envelope.signed.sha256)?;
    validate_version(configured_track, current_version, envelope.signed.version)?;

    let canonical = canonical_signed(&envelope.signed)?;
    let mut message = heapless::Vec::<u8, 800>::new();
    message
        .extend_from_slice(MANIFEST_DOMAIN)
        .map_err(|_| Error::Bounds)?;
    message
        .extend_from_slice(canonical.as_bytes())
        .map_err(|_| Error::Bounds)?;

    let key = decode_spki(public_key_spki)?;
    let mut signature_bytes = [0u8; 64];
    let decoded = Base64UrlUnpadded::decode(envelope.signature, &mut signature_bytes)
        .map_err(|_| Error::Signature)?;
    if decoded.len() != signature_bytes.len() {
        return Err(Error::Signature);
    }
    let signature = Signature::from_bytes(&signature_bytes);
    VerifyingKey::from_bytes(&key)
        .map_err(|_| Error::Key)?
        .verify_strict(message.as_slice(), &signature)
        .map_err(|_| Error::Signature)?;

    Ok(VerifiedManifest {
        track: manifest_track,
        version: envelope
            .signed
            .version
            .try_into()
            .map_err(|_| Error::Bounds)?,
        path: envelope.signed.path.try_into().map_err(|_| Error::Bounds)?,
        length: envelope.signed.length,
        sha256: hash,
    })
}

fn canonical_signed(manifest: &SignedManifest<'_>) -> Result<String<CANONICAL_MAX_LEN>, Error> {
    let mut output = String::new();
    write!(
        output,
        "{{\"schema\":1,\"board\":\"{}\",\"target\":\"{}\",\"track\":\"{}\",\"version\":\"{}\",\"path\":\"{}\",\"length\":{},\"sha256\":\"{}\"}}",
        manifest.board,
        manifest.target,
        manifest.track,
        manifest.version,
        manifest.path,
        manifest.length,
        manifest.sha256
    )
    .map_err(|_| Error::Canonical)?;
    Ok(output)
}

fn validate_version(track: Track, current: &str, candidate: &str) -> Result<(), Error> {
    let current = Version::parse(current).map_err(|_| Error::Version)?;
    let candidate = Version::parse(candidate).map_err(|_| Error::Version)?;
    if candidate <= current || !candidate.build.is_empty() {
        return Err(Error::Policy);
    }
    if candidate.pre.is_empty() {
        return Ok(());
    }
    if track == Track::Stable || !is_exact_rc(candidate.pre.as_str()) {
        return Err(Error::Policy);
    }
    Ok(())
}

fn is_exact_rc(value: &str) -> bool {
    let Some(number) = value.strip_prefix("rc.") else {
        return false;
    };
    !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && !number.starts_with('0')
}

fn validate_path(path: &str) -> Result<(), Error> {
    const PREFIX: &str = "/rfhold/esp-wifi-cam/releases/download/";
    if !path.starts_with(PREFIX)
        || path.len() > 255
        || path.contains("..")
        || path.contains('?')
        || path.contains('#')
        || path.contains("//")
        || !path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_'))
        || !path.ends_with(".bin")
    {
        return Err(Error::Path);
    }
    Ok(())
}

fn decode_spki(spki: &[u8]) -> Result<[u8; 32], Error> {
    if spki.len() != 44 || &spki[..12] != ED25519_SPKI_PREFIX {
        return Err(Error::Key);
    }
    spki[12..].try_into().map_err(|_| Error::Key)
}

fn decode_hex_32(value: &str) -> Result<[u8; 32], Error> {
    if value.len() != 64 {
        return Err(Error::Hash);
    }
    let mut output = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(output)
}

fn hex_nibble(value: u8) -> Result<u8, Error> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(Error::Hash),
    }
}

fn sequence_is_newer(candidate: u32, current: u32) -> bool {
    candidate != current && candidate.wrapping_sub(current) < (1 << 31)
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::format;

    fn provision(sequence: u32, track: Track) -> Provisioning {
        Provisioning {
            sequence,
            track,
            ssid: "camera-net".try_into().unwrap(),
            password: "not-a-secret".try_into().unwrap(),
        }
    }

    #[test]
    fn provisioning_command_and_journal_round_trip() {
        let parsed =
            parse_provision_command(b"P1 prerelease Y2FtZXJhLW5ldA bm90LWEtc2VjcmV0\r\n", 7)
                .unwrap();
        assert_eq!(parsed, provision(7, Track::Prerelease));
        let older = encode_provision_record(&provision(6, Track::Stable)).unwrap();
        let newer = encode_provision_record(&parsed).unwrap();
        assert_eq!(
            newest_provision_record(&older, &newer).unwrap(),
            (parsed, 1)
        );
    }

    #[test]
    fn corrupt_journal_record_is_ignored() {
        let valid = encode_provision_record(&provision(9, Track::Stable)).unwrap();
        let mut corrupt = valid;
        corrupt[20] ^= 1;
        assert_eq!(newest_provision_record(&corrupt, &valid).unwrap().1, 1);
    }

    #[test]
    fn replacement_window_uses_one_absolute_deadline() {
        assert_eq!(remaining_window_ticks(100, 100, 20), Some(20));
        assert_eq!(remaining_window_ticks(100, 115, 20), Some(5));
        assert_eq!(remaining_window_ticks(100, 120, 20), None);
        assert_eq!(remaining_window_ticks(100, 121, 20), None);
    }

    #[test]
    fn stable_rejects_prerelease_and_downgrade() {
        assert_eq!(
            validate_version(Track::Stable, "1.2.3", "1.3.0-rc.1"),
            Err(Error::Policy)
        );
        assert_eq!(
            validate_version(Track::Stable, "1.2.3", "1.2.3"),
            Err(Error::Policy)
        );
        assert!(validate_version(Track::Stable, "1.2.3", "1.2.4").is_ok());
    }

    #[test]
    fn prerelease_accepts_only_stable_or_exact_rc() {
        assert!(validate_version(Track::Prerelease, "1.2.3-rc.1", "1.2.3-rc.2").is_ok());
        assert!(validate_version(Track::Prerelease, "1.2.3-rc.1", "1.2.3").is_ok());
        assert_eq!(
            validate_version(Track::Prerelease, "1.2.3-rc.1", "1.2.3-beta.2"),
            Err(Error::Policy)
        );
        assert_eq!(
            validate_version(Track::Prerelease, "1.2.3-rc.1", "1.2.3-rc.02"),
            Err(Error::Version)
        );
        assert_eq!(
            validate_version(Track::Prerelease, "1.2.3-rc.1", "1.3.0-rc.0"),
            Err(Error::Policy)
        );
    }

    #[test]
    fn signed_manifest_verifies_and_binds_fields() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let signed = "{\"schema\":1,\"board\":\"seeed-xiao-esp32s3-sense\",\"target\":\"xtensa-esp32s3-none-elf\",\"track\":\"stable\",\"version\":\"1.2.4\",\"path\":\"/rfhold/esp-wifi-cam/releases/download/v1.2.4/esp-wifi-cam.bin\",\"length\":1234,\"sha256\":\"0000000000000000000000000000000000000000000000000000000000000000\"}";
        let mut message = std::vec::Vec::from(MANIFEST_DOMAIN);
        message.extend_from_slice(signed.as_bytes());
        let signature = signing_key.sign(&message);
        let mut signature_buffer = [0u8; 86];
        let signature =
            Base64UrlUnpadded::encode(&signature.to_bytes(), &mut signature_buffer).unwrap();
        let envelope = format!("{{\"signed\":{signed},\"signature\":\"{signature}\"}}");
        let mut spki = [0u8; 44];
        spki[..12].copy_from_slice(ED25519_SPKI_PREFIX);
        spki[12..].copy_from_slice(signing_key.verifying_key().as_bytes());

        let verified =
            verify_manifest(envelope.as_bytes(), &spki, Track::Stable, "1.2.3", 0x370000).unwrap();
        assert_eq!(verified.version.as_str(), "1.2.4");

        let changed = envelope.replace("1234", "1235");
        assert_eq!(
            verify_manifest(changed.as_bytes(), &spki, Track::Stable, "1.2.3", 0x370000),
            Err(Error::Signature)
        );
    }

    #[test]
    fn unsafe_asset_paths_are_rejected() {
        assert_eq!(
            validate_path("/rfhold/esp-wifi-cam/releases/download/v1/../image.bin"),
            Err(Error::Path)
        );
        assert_eq!(validate_path("https://example/image.bin"), Err(Error::Path));
    }

    #[test]
    fn checked_in_trust_anchor_has_expected_shape_and_fingerprint() {
        let key = include_bytes!("../../../keys/ota-public.der");
        assert!(decode_spki(key).is_ok());
        assert_eq!(
            Sha256::digest(key).as_slice(),
            &[
                0x6e, 0xad, 0xf4, 0x51, 0xf1, 0x3c, 0x0b, 0xe6, 0x71, 0x4d, 0xa1, 0xa2, 0x5e, 0xee,
                0x22, 0x42, 0x1a, 0x67, 0x20, 0x31, 0x95, 0xd1, 0x12, 0x8b, 0xfc, 0x98, 0xba, 0x68,
                0xcf, 0x12, 0xa5, 0xb7,
            ]
        );
    }
}
