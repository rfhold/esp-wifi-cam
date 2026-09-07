use std::{
    ffi::OsString,
    fmt::{self, Write as _},
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
};

use base64ct::{Base64UrlUnpadded, Encoding};
use ed25519_dalek::{
    Signer, SigningKey,
    pkcs8::{DecodePrivateKey, EncodePublicKey},
};
use ota_core::{
    BOARD, MANIFEST_DOMAIN, MANIFEST_MAX_LEN, OTA_IGNORE_RECORD_LEN, OtaIgnoreRecord, TARGET,
    Track, encode_ota_ignore_record, validate_ignore_version,
};
use semver::Version;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const OTA_PUBLIC_KEY_SHA256: [u8; 32] = [
    0x6e, 0xad, 0xf4, 0x51, 0xf1, 0x3c, 0x0b, 0xe6, 0x71, 0x4d, 0xa1, 0xa2, 0x5e, 0xee, 0x22, 0x42,
    0x1a, 0x67, 0x20, 0x31, 0x95, 0xd1, 0x12, 0x8b, 0xfc, 0x98, 0xba, 0x68, 0xcf, 0x12, 0xa5, 0xb7,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Arguments {
    pub track: Track,
    pub version: String,
    pub asset_path: String,
    pub image_path: PathBuf,
    pub output_path: PathBuf,
    pub public_key_path: PathBuf,
    pub private_key_path: PathBuf,
    pub max_slot_length: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IgnoreRecordArguments {
    pub version: Option<String>,
    pub output_path: PathBuf,
}

#[derive(Debug)]
pub enum Error {
    Argument(&'static str),
    Image(&'static str),
    Io(std::io::Error),
    Key(&'static str),
    Manifest(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Argument(message) => write!(formatter, "argument error: {message}"),
            Self::Image(message) => write!(formatter, "image error: {message}"),
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Key(message) => write!(formatter, "key error: {message}"),
            Self::Manifest(message) => write!(formatter, "manifest error: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn parse_arguments<I, S>(arguments: I) -> Result<Arguments, Error>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut values = arguments.into_iter().map(Into::into);
    let _program = values.next();
    let mut track = None;
    let mut version = None;
    let mut asset_path = None;
    let mut image_path = None;
    let mut output_path = None;
    let mut public_key_path = None;
    let mut private_key_path = None;
    let mut max_slot_length = None;

    while let Some(flag) = values.next() {
        let flag = flag
            .into_string()
            .map_err(|_| Error::Argument("flags must be UTF-8"))?;
        let value = values
            .next()
            .ok_or(Error::Argument("every flag requires a value"))?;
        match flag.as_str() {
            "--track" => set_once(&mut track, os_string(value)?)?,
            "--version" => set_once(&mut version, os_string(value)?)?,
            "--asset-path" => set_once(&mut asset_path, os_string(value)?)?,
            "--image" => set_once(&mut image_path, PathBuf::from(value))?,
            "--output" => set_once(&mut output_path, PathBuf::from(value))?,
            "--public-key" => set_once(&mut public_key_path, PathBuf::from(value))?,
            "--private-key" => set_once(&mut private_key_path, PathBuf::from(value))?,
            "--max-slot-length" => {
                let value = os_string(value)?;
                set_once(&mut max_slot_length, parse_length(&value)?)?;
            }
            _ => return Err(Error::Argument("unknown flag")),
        }
    }

    let track = Track::try_from(required(track, "missing --track")?.as_str())
        .map_err(|_| Error::Argument("track must be stable or prerelease"))?;
    let arguments = Arguments {
        track,
        version: required(version, "missing --version")?,
        asset_path: required(asset_path, "missing --asset-path")?,
        image_path: required(image_path, "missing --image")?,
        output_path: required(output_path, "missing --output")?,
        public_key_path: required(public_key_path, "missing --public-key")?,
        private_key_path: required(private_key_path, "missing --private-key")?,
        max_slot_length: required(max_slot_length, "missing --max-slot-length")?,
    };
    validate_release_fields(&arguments)?;
    Ok(arguments)
}

pub fn parse_ignore_record_arguments<I, S>(arguments: I) -> Result<IgnoreRecordArguments, Error>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut values = arguments.into_iter().map(Into::into);
    let _program = values.next();
    let _subcommand = values.next();
    let mut version = None;
    let mut clear = false;
    let mut output_path = None;

    while let Some(flag) = values.next() {
        let flag = os_string(flag)?;
        match flag.as_str() {
            "--version" => {
                let value = values
                    .next()
                    .ok_or(Error::Argument("--version requires a value"))?;
                set_once(&mut version, os_string(value)?)?;
            }
            "--clear" => {
                if clear {
                    return Err(Error::Argument("duplicate --clear"));
                }
                clear = true;
            }
            "--output" => {
                let value = values
                    .next()
                    .ok_or(Error::Argument("--output requires a value"))?;
                set_once(&mut output_path, PathBuf::from(value))?;
            }
            _ => return Err(Error::Argument("unknown flag")),
        }
    }

    if clear == version.is_some() {
        return Err(Error::Argument(
            "provide exactly one of --version or --clear",
        ));
    }
    if let Some(version) = &version {
        validate_ignore_version(version)
            .map_err(|_| Error::Argument("version is not an accepted OTA SemVer"))?;
    }
    let output_path = required(output_path, "missing --output")?;
    if output_path.as_os_str().is_empty() {
        return Err(Error::Argument("output path must not be empty"));
    }
    Ok(IgnoreRecordArguments {
        version,
        output_path,
    })
}

pub fn generate_ignore_record(arguments: &IgnoreRecordArguments) -> Result<PathBuf, Error> {
    if let Some(version) = &arguments.version {
        validate_ignore_version(version)
            .map_err(|_| Error::Argument("version is not an accepted OTA SemVer"))?;
    }
    let version = arguments
        .version
        .as_deref()
        .map(|version| {
            version
                .try_into()
                .map_err(|_| Error::Argument("version is too long"))
        })
        .transpose()?;
    let record = encode_ota_ignore_record(&OtaIgnoreRecord {
        sequence: 1,
        version,
    })
    .map_err(|_| Error::Argument("version is not an accepted OTA SemVer"))?;
    debug_assert_eq!(record.len(), OTA_IGNORE_RECORD_LEN);
    atomic_write(&arguments.output_path, &record)?;
    Ok(arguments.output_path.clone())
}

pub fn generate_manifest(arguments: &Arguments) -> Result<PathBuf, Error> {
    generate_manifest_with_fingerprint(arguments, OTA_PUBLIC_KEY_SHA256)
}

fn generate_manifest_with_fingerprint(
    arguments: &Arguments,
    expected_fingerprint: [u8; 32],
) -> Result<PathBuf, Error> {
    validate_release_fields(arguments)?;
    let public_metadata = fs::metadata(&arguments.public_key_path)?;
    if !public_metadata.is_file() || public_metadata.len() != 44 {
        return Err(Error::Key("public key must be a 44-byte Ed25519 SPKI"));
    }
    let public_der = fs::read(&arguments.public_key_path)?;
    if public_der.len() != 44 {
        return Err(Error::Key("public key must be a 44-byte Ed25519 SPKI"));
    }
    let fingerprint: [u8; 32] = Sha256::digest(&public_der).into();
    if fingerprint != expected_fingerprint {
        return Err(Error::Key(
            "public key fingerprint does not match trust anchor",
        ));
    }

    let (image_length, image_hash) = hash_image(&arguments.image_path, arguments.max_slot_length)?;
    let private_metadata = fs::metadata(&arguments.private_key_path)?;
    if !private_metadata.is_file() || private_metadata.len() == 0 || private_metadata.len() > 4096 {
        return Err(Error::Key(
            "private key PEM must be a regular file of at most 4096 bytes",
        ));
    }
    let private_pem = Zeroizing::new(fs::read_to_string(&arguments.private_key_path)?);
    let signing_key = SigningKey::from_pkcs8_pem(private_pem.as_str())
        .map_err(|_| Error::Key("private key is not Ed25519 PKCS#8 PEM"))?;
    let derived_der = signing_key
        .verifying_key()
        .to_public_key_der()
        .map_err(|_| Error::Key("cannot encode derived public key"))?;
    if derived_der.as_bytes() != public_der {
        return Err(Error::Key(
            "private key does not match checked-in public key",
        ));
    }

    let canonical = canonical_signed(arguments, image_length, &image_hash)?;
    let mut signing_input = Vec::with_capacity(MANIFEST_DOMAIN.len() + canonical.len());
    signing_input.extend_from_slice(MANIFEST_DOMAIN);
    signing_input.extend_from_slice(canonical.as_bytes());
    let signature = signing_key.sign(&signing_input);
    signing_key
        .verifying_key()
        .verify_strict(&signing_input, &signature)
        .map_err(|_| Error::Key("generated signature did not self-verify"))?;
    let signature = Base64UrlUnpadded::encode_string(&signature.to_bytes());
    let envelope = format!("{{\"signed\":{canonical},\"signature\":\"{signature}\"}}");
    if envelope.len() > MANIFEST_MAX_LEN {
        return Err(Error::Manifest("envelope exceeds firmware limit"));
    }
    atomic_write(&arguments.output_path, envelope.as_bytes())?;
    Ok(arguments.output_path.clone())
}

fn validate_release_fields(arguments: &Arguments) -> Result<(), Error> {
    let version = Version::parse(&arguments.version)
        .map_err(|_| Error::Argument("version must be valid SemVer"))?;
    if !version.build.is_empty() {
        return Err(Error::Argument("version build metadata is not allowed"));
    }
    if !version.pre.is_empty()
        && (arguments.track == Track::Stable || !is_exact_rc(version.pre.as_str()))
    {
        return Err(Error::Argument(
            "track accepts only stable versions or exact rc.N prereleases",
        ));
    }
    validate_asset_path(&arguments.asset_path)?;
    if arguments.max_slot_length == 0 || arguments.max_slot_length > u32::MAX as u64 {
        return Err(Error::Argument(
            "maximum slot length must fit a nonzero u32",
        ));
    }
    if arguments.image_path.as_os_str().is_empty()
        || arguments.output_path.as_os_str().is_empty()
        || arguments.public_key_path.as_os_str().is_empty()
        || arguments.private_key_path.as_os_str().is_empty()
    {
        return Err(Error::Argument("file paths must not be empty"));
    }
    if arguments.output_path == arguments.image_path
        || arguments.output_path == arguments.public_key_path
        || arguments.output_path == arguments.private_key_path
    {
        return Err(Error::Argument("output path must not replace an input"));
    }
    Ok(())
}

fn validate_asset_path(path: &str) -> Result<(), Error> {
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
        return Err(Error::Argument(
            "asset path is not an allowed Forgejo release path",
        ));
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

fn hash_image(path: &Path, maximum: u64) -> Result<(u32, [u8; 32]), Error> {
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::Image("image path is not a regular file"));
    }
    let mut reader = BufReader::new(file);
    let mut buffer = [0u8; 64 * 1024];
    let mut length = 0u64;
    let mut hash = Sha256::new();
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if length == 0 && buffer[0] != 0xe9 {
            return Err(Error::Image("image is not an app-only ESP-IDF image"));
        }
        length = length
            .checked_add(read as u64)
            .ok_or(Error::Image("image length overflow"))?;
        if length > maximum {
            return Err(Error::Image("image exceeds maximum slot length"));
        }
        hash.update(&buffer[..read]);
    }
    if length == 0 {
        return Err(Error::Image("image is empty"));
    }
    let length = u32::try_from(length).map_err(|_| Error::Image("image length exceeds u32"))?;
    Ok((length, hash.finalize().into()))
}

fn canonical_signed(
    arguments: &Arguments,
    image_length: u32,
    image_hash: &[u8; 32],
) -> Result<String, Error> {
    let mut hash = String::with_capacity(64);
    for byte in image_hash {
        write!(hash, "{byte:02x}").map_err(|_| Error::Manifest("cannot encode image hash"))?;
    }
    Ok(format!(
        "{{\"schema\":1,\"board\":\"{BOARD}\",\"target\":\"{TARGET}\",\"track\":\"{}\",\"version\":\"{}\",\"path\":\"{}\",\"length\":{image_length},\"sha256\":\"{hash}\"}}",
        arguments.track.as_str(),
        arguments.version,
        arguments.asset_path
    ))
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), Error> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .ok_or(Error::Argument("output path must name a file"))?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    let write_result = (|| -> Result<(), Error> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn parse_length(value: &str) -> Result<u64, Error> {
    if let Some(hex) = value.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).map_err(|_| Error::Argument("invalid maximum slot length"))
    } else {
        value
            .parse()
            .map_err(|_| Error::Argument("invalid maximum slot length"))
    }
}

fn required<T>(value: Option<T>, message: &'static str) -> Result<T, Error> {
    value.ok_or(Error::Argument(message))
}

fn set_once<T>(slot: &mut Option<T>, value: T) -> Result<(), Error> {
    if slot.replace(value).is_some() {
        return Err(Error::Argument("duplicate flag"));
    }
    Ok(())
}

fn os_string(value: OsString) -> Result<String, Error> {
    value
        .into_string()
        .map_err(|_| Error::Argument("value must be UTF-8"))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use ed25519_dalek::{SigningKey, pkcs8::EncodePrivateKey};
    use ota_core::verify_manifest;

    use super::*;

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        directory: PathBuf,
        private_key: PathBuf,
        public_key: PathBuf,
        image: PathBuf,
        output: PathBuf,
        fingerprint: [u8; 32],
    }

    impl Fixture {
        fn new(seed: u8) -> Self {
            let directory = std::env::temp_dir().join(format!(
                "esp-wifi-cam-release-tool-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            let signing_key = SigningKey::from_bytes(&[seed; 32]);
            let private_key = directory.join("private.pem");
            let pem = signing_key.to_pkcs8_pem(Default::default()).unwrap();
            fs::write(&private_key, pem.as_bytes()).unwrap();
            let public_key = directory.join("public.der");
            let public_der = signing_key.verifying_key().to_public_key_der().unwrap();
            fs::write(&public_key, public_der.as_bytes()).unwrap();
            let image = directory.join("image.bin");
            fs::write(&image, [0xe9, 1, 2, 3]).unwrap();
            Self {
                output: directory.join("manifest.json"),
                fingerprint: Sha256::digest(public_der.as_bytes()).into(),
                directory,
                private_key,
                public_key,
                image,
            }
        }

        fn arguments(&self, track: Track, version: &str) -> Arguments {
            Arguments {
                track,
                version: version.to_owned(),
                asset_path: format!(
                    "/rfhold/esp-wifi-cam/releases/download/{version}/esp-wifi-cam.bin"
                ),
                image_path: self.image.clone(),
                output_path: self.output.clone(),
                public_key_path: self.public_key.clone(),
                private_key_path: self.private_key.clone(),
                max_slot_length: 0x330000,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }

    #[test]
    fn stable_and_prerelease_envelopes_verify_with_ota_core() {
        for (track, current, candidate) in [
            (Track::Stable, "1.2.2", "1.2.3"),
            (Track::Prerelease, "1.2.3-rc.1", "1.2.3-rc.2"),
        ] {
            let fixture = Fixture::new(candidate.len() as u8);
            let arguments = fixture.arguments(track, candidate);
            generate_manifest_with_fingerprint(&arguments, fixture.fingerprint).unwrap();
            let envelope = fs::read(&fixture.output).unwrap();
            let public_key = fs::read(&fixture.public_key).unwrap();
            let verified = verify_manifest(
                &envelope,
                &public_key,
                track,
                current,
                arguments.max_slot_length as u32,
            )
            .unwrap();
            assert_eq!(verified.version.as_str(), candidate);
        }
    }

    #[test]
    fn wrong_private_key_fails_before_output() {
        let fixture = Fixture::new(3);
        let other = Fixture::new(4);
        let mut arguments = fixture.arguments(Track::Stable, "1.0.0");
        arguments.private_key_path = other.private_key.clone();
        assert!(matches!(
            generate_manifest_with_fingerprint(&arguments, fixture.fingerprint),
            Err(Error::Key(_))
        ));
        assert!(!fixture.output.exists());
    }

    #[test]
    fn production_entry_point_requires_recorded_public_key_fingerprint() {
        let fixture = Fixture::new(7);
        let arguments = fixture.arguments(Track::Stable, "1.0.0");
        assert!(matches!(generate_manifest(&arguments), Err(Error::Key(_))));
        assert!(!fixture.output.exists());
    }

    #[test]
    fn malformed_arguments_are_rejected() {
        assert!(parse_arguments(["release-tool", "--track", "nightly"]).is_err());
        let fixture = Fixture::new(5);
        let mut arguments = fixture.arguments(Track::Stable, "1.0.0-rc.1");
        assert!(matches!(
            generate_manifest_with_fingerprint(&arguments, fixture.fingerprint),
            Err(Error::Argument(_))
        ));
        arguments.track = Track::Prerelease;
        arguments.version = "1.0.0-beta.1".to_owned();
        assert!(matches!(
            generate_manifest_with_fingerprint(&arguments, fixture.fingerprint),
            Err(Error::Argument(_))
        ));
        arguments.version = "1.0.0-rc.1".to_owned();
        arguments.asset_path = "/rfhold/esp-wifi-cam/releases/download/v1/../image.bin".to_owned();
        assert!(matches!(
            generate_manifest_with_fingerprint(&arguments, fixture.fingerprint),
            Err(Error::Argument(_))
        ));
    }

    #[test]
    fn ignore_record_cli_generates_set_and_clear_records_without_keys() {
        let directory = std::env::temp_dir().join(format!(
            "esp-wifi-cam-ignore-record-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let output = directory.join("ignore.bin");
        let set = parse_ignore_record_arguments([
            "release-tool",
            "ignore-record",
            "--version",
            "1.2.3-rc.1",
            "--output",
            output.to_str().unwrap(),
        ])
        .unwrap();
        generate_ignore_record(&set).unwrap();
        assert_eq!(fs::read(&output).unwrap().len(), OTA_IGNORE_RECORD_LEN);
        let clear = parse_ignore_record_arguments([
            "release-tool",
            "ignore-record",
            "--clear",
            "--output",
            output.to_str().unwrap(),
        ])
        .unwrap();
        generate_ignore_record(&clear).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ignore_record_cli_rejects_invalid_or_ambiguous_versions() {
        assert!(
            parse_ignore_record_arguments([
                "release-tool",
                "ignore-record",
                "--version",
                "1.2.3+build",
                "--output",
                "ignore.bin",
            ])
            .is_err()
        );
        assert!(
            parse_ignore_record_arguments([
                "release-tool",
                "ignore-record",
                "--clear",
                "--version",
                "1.2.3",
                "--output",
                "ignore.bin",
            ])
            .is_err()
        );
    }

    #[test]
    fn image_magic_empty_and_size_gates_are_enforced() {
        let fixture = Fixture::new(6);
        let arguments = fixture.arguments(Track::Stable, "1.0.0");
        fs::write(&fixture.image, []).unwrap();
        assert!(matches!(
            generate_manifest_with_fingerprint(&arguments, fixture.fingerprint),
            Err(Error::Image("image is empty"))
        ));
        fs::write(&fixture.image, [0xea]).unwrap();
        assert!(matches!(
            generate_manifest_with_fingerprint(&arguments, fixture.fingerprint),
            Err(Error::Image("image is not an app-only ESP-IDF image"))
        ));
        fs::write(&fixture.image, [0xe9, 1, 2, 3, 4]).unwrap();
        let mut too_small = arguments;
        too_small.max_slot_length = 4;
        assert!(matches!(
            generate_manifest_with_fingerprint(&too_small, fixture.fingerprint),
            Err(Error::Image("image exceeds maximum slot length"))
        ));
    }
}
