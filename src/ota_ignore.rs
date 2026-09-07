use esp_storage::FlashStorage;
use heapless::String;
use ota_core::{OTA_IGNORE_RECORD_LEN, OTA_IGNORE_VERSION_MAX_LEN, newest_ota_ignore_record};

const OTA_IGNORE_A_OFFSET: u32 = 0xb000;
const OTA_IGNORE_B_OFFSET: u32 = 0xc000;

pub fn ignored_version(flash: &mut FlashStorage<'_>) -> Option<String<OTA_IGNORE_VERSION_MAX_LEN>> {
    let mut first = [0u8; OTA_IGNORE_RECORD_LEN];
    let mut second = [0u8; OTA_IGNORE_RECORD_LEN];
    if flash.read(OTA_IGNORE_A_OFFSET, &mut first).is_err()
        || flash.read(OTA_IGNORE_B_OFFSET, &mut second).is_err()
    {
        return None;
    }
    newest_ota_ignore_record(&first, &second)
        .ok()
        .and_then(|(record, _)| record.version)
}
