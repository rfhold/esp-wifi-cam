use esp_storage::FlashStorage;

pub const OTA_SLOT_LEN: u32 = 0x370000;

pub fn is_valid(flash: &mut FlashStorage<'_>) -> bool {
    if flash.capacity() != 8 * 1024 * 1024 {
        return false;
    }
    let mut buffer = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
    let Ok(table) = esp_bootloader_esp_idf::partitions::read_partition_table(flash, &mut buffer)
    else {
        return false;
    };
    let expected = [
        ("config", 1, 2, 0x9000, 0x4000),
        ("otadata", 1, 0, 0xd000, 0x2000),
        ("phy_init", 1, 1, 0xf000, 0x1000),
        ("factory", 0, 0, 0x10000, 0x100000),
        ("ota_0", 0, 0x10, 0x110000, OTA_SLOT_LEN),
        ("ota_1", 0, 0x11, 0x480000, OTA_SLOT_LEN),
    ];
    table.iter().count() == expected.len()
        && expected.iter().all(|(label, kind, subtype, offset, len)| {
            table.iter().any(|partition| {
                partition.label_as_str() == *label
                    && partition.raw_type() == *kind
                    && partition.raw_subtype() == *subtype
                    && partition.offset() == *offset
                    && partition.len() == *len
            })
        })
}
