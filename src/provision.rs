use embassy_time::{Duration, Instant, with_timeout};
use embedded_io_async::{Read, Write};
use esp_hal::{Async, usb::usb_serial_jtag::UsbSerialJtag};
use esp_storage::FlashStorage;
use ota_core::{
    PROVISION_RECORD_LEN, Provisioning, encode_provision_record, newest_provision_record,
};

const CONFIG_A_OFFSET: u32 = 0x9000;
const CONFIG_B_OFFSET: u32 = 0xa000;
const COMMAND_MAX_LEN: usize = 256;

pub async fn load_or_provision(
    flash: &mut FlashStorage<'_>,
    usb: UsbSerialJtag<'static, Async>,
) -> Provisioning {
    let mut first = [0u8; PROVISION_RECORD_LEN];
    let mut second = [0u8; PROVISION_RECORD_LEN];
    let existing = if flash.read(CONFIG_A_OFFSET, &mut first).is_ok()
        && flash.read(CONFIG_B_OFFSET, &mut second).is_ok()
    {
        newest_provision_record(&first, &second).ok()
    } else {
        None
    };

    let (mut rx, mut tx) = usb.split();
    let mut command = [0u8; COMMAND_MAX_LEN];
    let mut used = 0usize;
    let replacement_started = existing.as_ref().map(|_| Instant::now().as_ticks());
    let replacement_window = Duration::from_secs(2).as_ticks();
    write_with_window(
        &mut tx,
        b"esp-wifi-cam provisioning ready\r\n",
        replacement_started,
        replacement_window,
    )
    .await;

    loop {
        let read = if let Some(started) = replacement_started {
            let Some(remaining) = ota_core::remaining_window_ticks(
                started,
                Instant::now().as_ticks(),
                replacement_window,
            ) else {
                return existing.as_ref().unwrap().0.clone();
            };
            match with_timeout(
                Duration::from_ticks(remaining),
                rx.read(&mut command[used..]),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => return existing.as_ref().unwrap().0.clone(),
            }
        } else {
            rx.read(&mut command[used..]).await
        };

        if let Some(started) = replacement_started
            && ota_core::remaining_window_ticks(
                started,
                Instant::now().as_ticks(),
                replacement_window,
            )
            .is_none()
        {
            return existing.as_ref().unwrap().0.clone();
        }

        match read {
            Ok(0) => continue,
            Ok(length) => used += length,
            Err(_) => {
                write_with_window(
                    &mut tx,
                    b"ERR provisioning transport\r\n",
                    replacement_started,
                    replacement_window,
                )
                .await;
                continue;
            }
        }

        if let Some(end) = command[..used].iter().position(|byte| *byte == b'\n') {
            let sequence = existing
                .as_ref()
                .map(|(record, _)| record.sequence.wrapping_add(1))
                .unwrap_or(1);
            match ota_core::parse_provision_command(&command[..=end], sequence) {
                Ok(provision) if store(flash, &provision, existing.as_ref()).is_ok() => {
                    write_with_window(
                        &mut tx,
                        b"OK provisioned\r\n",
                        replacement_started,
                        replacement_window,
                    )
                    .await;
                    flush_with_window(&mut tx, replacement_started, replacement_window).await;
                    return provision;
                }
                _ => {
                    write_with_window(
                        &mut tx,
                        b"ERR invalid command\r\n",
                        replacement_started,
                        replacement_window,
                    )
                    .await;
                    used = 0;
                }
            }
        } else if used == command.len() {
            write_with_window(
                &mut tx,
                b"ERR command too long\r\n",
                replacement_started,
                replacement_window,
            )
            .await;
            used = 0;
        }
    }
}

async fn write_with_window<W: Write>(
    writer: &mut W,
    message: &[u8],
    started: Option<u64>,
    window: u64,
) {
    if let Some(started) = started {
        let Some(remaining) = remaining_duration(started, window) else {
            return;
        };
        let _ = with_timeout(remaining, writer.write_all(message)).await;
    } else {
        let _ = writer.write_all(message).await;
    }
}

async fn flush_with_window<W: Write>(writer: &mut W, started: Option<u64>, window: u64) {
    if let Some(started) = started {
        let Some(remaining) = remaining_duration(started, window) else {
            return;
        };
        let _ = with_timeout(remaining, writer.flush()).await;
    } else {
        let _ = writer.flush().await;
    }
}

fn remaining_duration(started: u64, window: u64) -> Option<Duration> {
    ota_core::remaining_window_ticks(started, Instant::now().as_ticks(), window)
        .map(Duration::from_ticks)
}

fn store(
    flash: &mut FlashStorage<'_>,
    provision: &Provisioning,
    existing: Option<&(Provisioning, usize)>,
) -> Result<(), ()> {
    let target = match existing {
        Some((_, 0)) => CONFIG_B_OFFSET,
        _ => CONFIG_A_OFFSET,
    };
    let encoded = encode_provision_record(provision).map_err(|_| ())?;
    flash.write(target, &encoded).map_err(|_| ())?;
    let mut readback = [0u8; PROVISION_RECORD_LEN];
    flash.read(target, &mut readback).map_err(|_| ())?;
    if readback != encoded {
        return Err(());
    }
    Ok(())
}
