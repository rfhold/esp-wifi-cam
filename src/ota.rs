use core::{
    fmt::Write,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_net::{
    Stack,
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Timer};
use embedded_io_async::Read;
use esp_bootloader_esp_idf::{
    ota::OtaImageState,
    ota_updater::OtaUpdater,
    partitions::{AppPartitionSubType, PartitionType},
};
use esp_storage::FlashStorage;
use heapless::String;
use ota_core::{MANIFEST_MAX_LEN, Provisioning, Track, VerifiedManifest, verify_manifest};
use reqwless::{
    client::{HttpClient, TlsConfig, TlsVerify},
    request::Method,
    response::Status,
};
use sha2::{Digest, Sha256};
use static_cell::StaticCell;

const HTTPS_ORIGIN: &str = "https://git.holdenitdown.net";
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(30);
const STABLE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const PRERELEASE_CHECK_INTERVAL: Duration = Duration::from_secs(60);
const PUBLIC_KEY: &[u8] = include_bytes!("../keys/ota-public.der");
const TLS_ROOT: &[u8] = include_bytes!("isrg-root-x1.der");

pub static CAMERA_HEALTHY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static TRANSFER_ACTIVE: AtomicBool = AtomicBool::new(false);
static TCP_STATE: StaticCell<TcpClientState<1, 4096, 4096>> = StaticCell::new();

#[embassy_executor::task]
pub async fn ota_task(
    stack: Stack<'static>,
    mut flash: FlashStorage<'static>,
    provision: Provisioning,
    tls_seed: u64,
) -> () {
    if !crate::flash_layout::is_valid(&mut flash) {
        defmt::error!("OTA disabled: invalid flash or partition layout");
        return;
    }

    CAMERA_HEALTHY.wait().await;
    if confirm_running_image(&mut flash).is_err() {
        defmt::error!("OTA disabled: running image confirmation failed");
        return;
    }
    defmt::info!("Camera health confirmed for running image");
    Timer::after(FIRST_CHECK_DELAY).await;
    let check_interval = match provision.track {
        Track::Stable => STABLE_CHECK_INTERVAL,
        Track::Prerelease => PRERELEASE_CHECK_INTERVAL,
    };
    let mut tcp = TcpClient::new(stack, TCP_STATE.init(TcpClientState::new()));
    tcp.set_timeout(Some(Duration::from_secs(30)));
    let dns = DnsSocket::new(stack);

    loop {
        stack.wait_config_up().await;
        match check_and_install(&tcp, &dns, &mut flash, &provision, tls_seed).await {
            Ok(true) => esp_hal::system::software_reset(),
            Ok(false) => defmt::info!("OTA check complete: no accepted update"),
            Err(()) => defmt::warn!("OTA check failed closed"),
        }
        Timer::after(check_interval).await;
    }
}

fn confirm_running_image(flash: &mut FlashStorage<'_>) -> Result<(), ()> {
    let mut buffer = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
    let booted = {
        let table = esp_bootloader_esp_idf::partitions::read_partition_table(flash, &mut buffer)
            .map_err(|_| ())?;
        match table.booted_partition().map_err(|_| ())? {
            Some(partition) => match partition.partition_type() {
                PartitionType::App(subtype) => subtype,
                _ => return Err(()),
            },
            None => return Err(()),
        }
    };
    let mut updater = OtaUpdater::new(flash, &mut buffer).map_err(|_| ())?;
    if booted == AppPartitionSubType::Factory {
        return Ok(());
    }
    if updater.selected_partition().map_err(|_| ())? != booted {
        return Err(());
    }
    match updater.current_ota_state() {
        Ok(OtaImageState::New | OtaImageState::PendingVerify) => updater
            .set_current_ota_state(OtaImageState::Valid)
            .map_err(|_| ()),
        Ok(_) | Err(esp_bootloader_esp_idf::partitions::Error::InvalidState) => Ok(()),
        Err(_) => Err(()),
    }
}

async fn check_and_install(
    tcp: &TcpClient<'static, 1, 4096, 4096>,
    dns: &DnsSocket<'static>,
    flash: &mut FlashStorage<'_>,
    provision: &Provisioning,
    tls_seed: u64,
) -> Result<bool, ()> {
    let mut tls_read = [0u8; 20480];
    let mut tls_write = [0u8; 4096];
    let mut response_buffer = [0u8; 2048];
    let mut manifest_bytes = [0u8; MANIFEST_MAX_LEN];
    let mut manifest_url: String<160> = String::new();
    write!(
        manifest_url,
        "{HTTPS_ORIGIN}/rfhold/esp-wifi-cam/releases/download/{}/manifest.json",
        provision.track.as_str()
    )
    .map_err(|_| ())?;
    defmt::info!("OTA fetching manifest");

    let manifest_len = {
        let mut client = HttpClient::new_with_tls(
            tcp,
            dns,
            TlsConfig::new(
                tls_seed,
                &mut tls_read,
                &mut tls_write,
                TlsVerify::Certificate {
                    ca: TLS_ROOT,
                    cert: None,
                    key: None,
                },
            ),
        );
        let mut request = client
            .request(Method::GET, manifest_url.as_str())
            .await
            .map_err(log_manifest_request_error)?;
        let response = request.send(&mut response_buffer).await.map_err(|_| ())?;
        if response.status != Status::Ok
            || response.content_length.is_none()
            || response.content_length.unwrap() > MANIFEST_MAX_LEN
        {
            return Err(());
        }
        let expected = response.content_length.unwrap();
        let mut reader = response.body().reader();
        read_exact_body(&mut reader, &mut manifest_bytes[..expected]).await?;
        expected
    };
    defmt::info!("OTA manifest fetched");

    let manifest = verify_manifest(
        &manifest_bytes[..manifest_len],
        PUBLIC_KEY,
        provision.track,
        env!("CARGO_PKG_VERSION"),
        crate::flash_layout::OTA_SLOT_LEN,
    )
    .map_err(|_| ())?;
    defmt::info!("OTA manifest verified");
    install_image(
        tcp,
        dns,
        flash,
        &manifest,
        tls_seed.wrapping_add(1),
        &mut tls_read,
        &mut tls_write,
        &mut response_buffer,
    )
    .await?;
    defmt::info!("OTA image staged");
    Ok(true)
}

fn log_manifest_request_error(error: reqwless::Error) -> () {
    match error {
        reqwless::Error::Dns => defmt::warn!("OTA manifest DNS failed"),
        reqwless::Error::Network(_) => defmt::warn!("OTA manifest TCP failed"),
        reqwless::Error::Tls(_) => defmt::warn!("OTA manifest TLS failed"),
        _ => defmt::warn!("OTA manifest request failed"),
    }
}

async fn install_image(
    tcp: &TcpClient<'static, 1, 4096, 4096>,
    dns: &DnsSocket<'static>,
    flash: &mut FlashStorage<'_>,
    manifest: &VerifiedManifest,
    tls_seed: u64,
    tls_read: &mut [u8; 20480],
    tls_write: &mut [u8; 4096],
    response_buffer: &mut [u8; 2048],
) -> Result<(), ()> {
    let mut url: String<320> = String::new();
    write!(url, "{HTTPS_ORIGIN}{}", manifest.path).map_err(|_| ())?;
    let mut client = HttpClient::new_with_tls(
        tcp,
        dns,
        TlsConfig::new(
            tls_seed,
            tls_read,
            tls_write,
            TlsVerify::Certificate {
                ca: TLS_ROOT,
                cert: None,
                key: None,
            },
        ),
    );
    let mut request = client
        .request(Method::GET, url.as_str())
        .await
        .map_err(|_| ())?;
    defmt::info!("OTA fetching image");
    let response = request.send(response_buffer).await.map_err(|_| ())?;
    if response.status != Status::Ok || response.content_length != Some(manifest.length as usize) {
        return Err(());
    }
    defmt::info!("OTA image response accepted");

    let mut partition_buffer = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
    let mut updater = OtaUpdater::new(flash, &mut partition_buffer).map_err(|_| ())?;
    let (mut partition, _) = updater.next_partition().map_err(|_| ())?;
    if manifest.length as usize > partition.capacity() {
        return Err(());
    }
    partition
        .erase(0, partition.capacity() as u32)
        .map_err(|_| ())?;
    defmt::info!("OTA inactive slot erased");

    TRANSFER_ACTIVE.store(true, Ordering::Release);
    let result = async {
        let mut body = response.body().reader();
        let mut offset = 0u32;
        let mut chunk = [0u8; 4096];
        let mut readback = [0u8; 4096];
        let mut downloaded = Sha256::new();
        let mut persisted = Sha256::new();
        while offset < manifest.length {
            let length = (manifest.length - offset).min(chunk.len() as u32) as usize;
            read_exact_body(&mut body, &mut chunk[..length])
                .await
                .map_err(|_| {
                    defmt::warn!("OTA image read failed");
                })?;
            if offset == 0 && chunk[0] != 0xe9 {
                defmt::warn!("OTA image magic rejected");
                return Err(());
            }
            downloaded.update(&chunk[..length]);
            partition.write(offset, &chunk[..length]).map_err(|_| {
                defmt::warn!("OTA image write failed");
            })?;
            partition
                .read(offset, &mut readback[..length])
                .map_err(|_| {
                    defmt::warn!("OTA image readback failed");
                })?;
            if readback[..length] != chunk[..length] {
                defmt::warn!("OTA image readback mismatch");
                return Err(());
            }
            persisted.update(&readback[..length]);
            offset += length as u32;
        }
        if downloaded.finalize().as_slice() != manifest.sha256
            || persisted.finalize().as_slice() != manifest.sha256
        {
            defmt::warn!("OTA image digest mismatch");
            return Err(());
        }
        Ok(())
    }
    .await;
    TRANSFER_ACTIVE.store(false, Ordering::Release);
    result?;
    defmt::info!("OTA image digest verified");
    drop(partition);

    updater.activate_next_partition().map_err(|_| ())?;
    updater
        .set_current_ota_state(OtaImageState::New)
        .map_err(|_| ())?;
    Ok(())
}

async fn read_exact_body<R: Read>(reader: &mut R, mut output: &mut [u8]) -> Result<(), ()> {
    while !output.is_empty() {
        let read = reader.read(output).await.map_err(|_| ())?;
        if read == 0 {
            return Err(());
        }
        output = &mut output[read..];
    }
    Ok(())
}
